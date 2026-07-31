//! Running a git stage when git asks for it.
//!
//! This is the hot path. It fires on every commit in every repository that
//! has daft's shims installed, including the overwhelming majority that have
//! no definition for the stage being dispatched, so the shape of the function
//! is dictated by how fast it can answer "nothing to do".

use super::{GitStage, gitdir};
use crate::executor::presenter::JobPresenter;
use crate::hooks::{HookContext, HookExecutor, HookResult};
use crate::output::Output;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Environment variable that disables daft's stage dispatch entirely.
///
/// The escape hatch for "the gate is broken and I need to commit". `git
/// commit --no-verify` already exists and is better, but it does not reach
/// through a script that invokes git several times, and a user who cannot
/// commit needs a lever that definitely works.
pub const KILL_SWITCH_VAR: &str = "DAFT_HOOKS";

/// What git handed the hook.
#[derive(Debug, Clone, Default)]
pub struct StagePayload {
    /// git's argv, minus the hook name.
    pub argv: Vec<String>,
    /// git's stdin, drained by the caller.
    pub stdin: Option<String>,
    /// `GIT_INDEX_FILE` as it was on entry.
    pub index_file: Option<PathBuf>,
}

/// The outcome of a dispatch, in the terms the exit code needs.
#[derive(Debug, Clone)]
pub struct StageRun {
    /// Whether any job ran. `false` for the fast no-op path and for a
    /// declined trust gate.
    pub ran: bool,
    /// Whether the stage passed. A stage that did not run passes.
    pub success: bool,
    /// The failing job's exit code, when there was one.
    pub exit_code: Option<i32>,
    /// Why nothing ran, when nothing did.
    pub skip_reason: Option<String>,
}

impl StageRun {
    /// The answer for a stage that had nothing to do.
    fn nothing(reason: impl Into<String>) -> Self {
        Self {
            ran: false,
            success: true,
            exit_code: None,
            skip_reason: Some(reason.into()),
        }
    }
}

/// Whether the kill switch is set to something falsy.
///
/// Accepts the spellings a person types in a hurry.
fn disabled_by_env() -> bool {
    std::env::var(KILL_SWITCH_VAR).is_ok_and(|v| matches!(v.trim(), "0" | "false" | "off" | "no"))
}

/// Run `stage` in the repository containing `cwd`.
///
/// Shared by all three entrances — the shim's `daft __hook`, an explicit
/// `daft hooks run <stage>`, and the push adapter running the stage itself —
/// so a stage cannot behave differently depending on who invoked it. That
/// equivalence is the whole premise of the push integration: daft may run
/// `pre-push` and suppress git's dispatch only if the two are the same run.
///
/// `bypass_trust` is set only by explicit invocation, where typing the
/// command is the consent the trust gate would otherwise ask for.
pub fn run_git_stage(
    stage: GitStage,
    cwd: &Path,
    payload: StagePayload,
    presenter: Arc<dyn JobPresenter>,
    output: &mut dyn Output,
    bypass_trust: bool,
) -> Result<StageRun> {
    // ── the fast paths, in order of how often they hit ─────────────────

    if super::guard_blocks(stage.git_hook_filename()) {
        return Ok(StageRun::nothing("already running this stage"));
    }
    if disabled_by_env() {
        return Ok(StageRun::nothing(format!(
            "{KILL_SWITCH_VAR} is set to off"
        )));
    }
    let Some(dirs) = gitdir::discover(cwd) else {
        return Ok(StageRun::nothing("not inside a git repository"));
    };

    // Config before settings: reading `daft.yml` is one file read, while
    // loading the hooks configuration opens the repository. The common case
    // in a repository with shims installed is a stage nobody defined, and it
    // must cost the file read and nothing more.
    let native = match crate::hooks::yaml_config_loader::load_merged_config(&dirs.worktree_root) {
        Ok(config) => config,
        Err(e) => {
            // A broken config must not brick commits. Warn and stand down —
            // the same stance the legacy-script fallback takes, and the
            // difference between "fix your YAML" and "you cannot commit".
            output.warning(&format!(
                "daft: could not read the hooks config, skipping the {stage} hook: {e:#}"
            ));
            return Ok(StageRun::nothing("hooks config could not be read"));
        }
    };
    let mut unsupported = Vec::new();
    let Some((config, took_over)) =
        resolve_config(&dirs.worktree_root, native, output, &mut unsupported)?
    else {
        return Ok(StageRun::nothing("no hooks config"));
    };
    let payload_for_lfs = payload.clone();

    // git-lfs is chained before daft's own jobs, and before the "is this
    // stage defined?" check — because superseding its hook file is what
    // installing did, and a repository whose LFS pointers stop being
    // resolved on checkout has a much worse day than one whose gate is slow.
    // `skip_lfs: true` opts out for repositories that wire it another way.
    if config.skip_lfs != Some(true)
        && let Some(failure) = chain_git_lfs(stage, &dirs.worktree_root, &payload_for_lfs)?
    {
        return Ok(failure);
    }

    let Some(hook_def) = config.hooks.get(stage.yaml_name()).cloned() else {
        return Ok(StageRun::nothing(format!("no {stage} definition")));
    };

    // A takeover that quietly runs less than the file says is worse than one
    // that refuses: the repository believes it has the gates its config
    // describes. Reported here rather than at resolution time so the notes
    // appear once, on the stage that is actually running, instead of four
    // times per commit as each stage of the commit dispatches.
    for note in &unsupported {
        output.warning(note);
    }

    // ── there is work; set up properly ─────────────────────────────────

    let branch = current_branch(&dirs.worktree_root);
    let ctx = HookContext::new(
        crate::hooks::HookType::Git(stage),
        "__hook",
        &dirs.worktree_root,
        &dirs.common_dir,
        "origin",
        &dirs.worktree_root,
        &dirs.worktree_root,
        &branch,
    )
    .with_stage_payload(payload.argv, payload.stdin)
    .with_index_file(payload.index_file)
    // The guard rides the hook environment, so every job — and everything a
    // job spawns — carries it. That is exactly the reach it needs: a shim
    // re-entered by a job reads the inherited environment, not daft's job
    // map. Scoped to this stage only, so a `pre-commit` job that commits
    // still fires `commit-msg`, as it would under the manager daft replaces.
    .with_derived_env(
        [(
            super::STAGE_GUARD_VAR.to_string(),
            stage.git_hook_filename().to_string(),
        )]
        .into(),
    );

    let hooks_config = crate::core::settings::load_hooks_config()?;
    let hook_mode = hook_mode_for(stage);
    let mut executor = HookExecutor::new(hooks_config)?
        .with_bypass_trust(bypass_trust)
        .with_hook_execution_mode(hook_mode);
    // A gate whose jobs were detached is not a slow gate, it is no gate:
    // git reads the shim's exit status and nothing else, so a job still
    // starting up in the coordinator when daft exits gets no say at all.
    // Announced because the config asked for something daft is declining to
    // do — and only where it actually declined, so an observer stage's
    // `background:` job, which is honoured, says nothing.
    //
    // Phrased about the declaration rather than about this run, because the
    // list is read before glob/skip filtering: a `background:` job that a
    // `glob:` then filters out never runs at all, and "running `x` inline"
    // would be a claim daft cannot back.
    if hook_mode.is_foreground() {
        for name in promoted_background_jobs(&hook_def) {
            output.notice(&format!(
                "{stage}: ignoring `background: true` on `{name}` — git waits on \
                 this hook, so a detached job could not gate the operation."
            ));
        }
    }
    // The incumbent's own job-exclusion variable, honoured only while running
    // its config — someone mid-assessment still has it in their shell, and it
    // should keep meaning what it meant.
    if took_over {
        let excluded = crate::hooks::incumbent::lefthook::excluded_jobs();
        if !excluded.is_empty() {
            executor = executor.with_job_filter(crate::hooks::yaml_executor::JobFilter {
                skip: crate::hooks::job_adapter::SkipSelectors {
                    names: excluded.clone(),
                    raw: excluded,
                    origin: Some(crate::hooks::incumbent::lefthook::EXCLUDE_VAR),
                    ..Default::default()
                },
                ..Default::default()
            });
        }
    }

    // The executor is handed the resolved config rather than re-reading the
    // worktree: in takeover mode there is no `daft.yml` on disk that would
    // reproduce it.
    let executor = executor.with_resolved_config(config);
    match executor.execute(&ctx, output, presenter) {
        Ok(result) => Ok(into_run(result)),
        // A gate that failed under `abort` arrives as an error carrying the
        // exit code, because that is how the executor reports "this must stop
        // the operation". It is a verdict, not a malfunction, so it becomes a
        // failed `StageRun` rather than propagating — the caller's job is to
        // turn it into git's exit status, and only a genuine malfunction
        // should read as "daft could not run the check".
        Err(e) => match crate::hooks::HookAborted::from_error(&e) {
            Some(aborted) => Ok(StageRun {
                ran: true,
                success: false,
                exit_code: Some(aborted.exit_code),
                skip_reason: None,
            }),
            None => Err(e),
        },
    }
}

/// Which config the stages come from: daft's own, or an incumbent taken over.
///
/// Reaching this function is itself the consent: a shim only exists in the
/// hooks directory because someone ran `daft hooks install`. The paths that
/// have no such proof — `manages_stage`, deciding whether a push should take
/// daft's route — check the install sentinel instead, because there a
/// `lefthook.yml` alone must never make daft start running it.
fn resolve_config(
    root: &Path,
    native: Option<crate::hooks::yaml_config::YamlConfig>,
    output: &mut dyn Output,
    unsupported: &mut Vec<String>,
) -> Result<Option<(crate::hooks::yaml_config::YamlConfig, bool)>> {
    use crate::hooks::incumbent::{self, StageSource};

    match incumbent::resolve_stage_source(root, native.as_ref()) {
        StageSource::Native | StageSource::None => Ok(native.map(|c| (c, false))),
        StageSource::Incumbent(path) => {
            // Its own kill switch is honoured here and only here: a
            // repository that has migrated to daft's keys should not still be
            // steerable by the old tool's variables.
            if incumbent::lefthook::disabled_by_env() {
                return Ok(None);
            }
            let spec = match incumbent::detect(root) {
                Ok(Some(spec)) => spec,
                Ok(None) => return Ok(native.map(|c| (c, false))),
                Err(e) => {
                    output.warning(&format!("daft: could not read {}: {e:#}", path.display()));
                    return Ok(None);
                }
            };
            unsupported.extend(
                spec.unsupported
                    .iter()
                    .map(|note| format!("{}: {note}", spec.path.display())),
            );
            Ok(Some((spec.config, true)))
        }
    }
}

/// Stages git-lfs installs a hook for, and which daft therefore superseded.
const LFS_STAGES: &[GitStage] = &[
    GitStage::PrePush,
    GitStage::PostCheckout,
    GitStage::PostCommit,
    GitStage::PostMerge,
];

/// Run `git lfs <stage>` when this repository uses LFS.
///
/// Installing displaced git-lfs's hook file, so daft owes it the call. Left
/// undone, `git lfs pre-push` never runs and large files silently stop being
/// uploaded — a repository-corrupting failure that surfaces on somebody
/// else's clone, days later.
///
/// Returns `Some(failure)` when the chained call failed and the stage should
/// stop; `None` when there was nothing to do or it succeeded.
fn chain_git_lfs(
    stage: GitStage,
    worktree: &Path,
    payload: &StagePayload,
) -> Result<Option<StageRun>> {
    if !LFS_STAGES.contains(&stage) || !repo_uses_lfs(worktree) {
        return Ok(None);
    }

    let mut command = crate::utils::git_command_at(worktree);
    command.arg("lfs").arg(stage.git_hook_filename());
    command.args(&payload.argv);
    // The same stdin git would have given it. `pre-push`'s ref block is the
    // whole of what `git lfs pre-push` reads.
    let status = match payload.stdin.as_deref() {
        Some(text) => {
            use std::io::Write;
            let mut child = command
                .stdin(std::process::Stdio::piped())
                .spawn()
                .context("failed to run git lfs")?;
            if let Some(mut pipe) = child.stdin.take() {
                let _ = pipe.write_all(text.as_bytes());
            }
            child.wait().context("failed to wait for git lfs")?
        }
        None => command
            .stdin(std::process::Stdio::null())
            .status()
            .context("failed to run git lfs")?,
    };

    if status.success() {
        return Ok(None);
    }
    Ok(Some(StageRun {
        ran: true,
        success: false,
        exit_code: status.code(),
        skip_reason: None,
    }))
}

/// Whether this repository uses git-lfs.
///
/// Two independent signals because either alone has a false negative: the
/// filter is configured by `git lfs install` (which a fresh clone may not
/// have run yet) and `.git/lfs` appears once anything is fetched.
fn repo_uses_lfs(worktree: &Path) -> bool {
    let Some(dirs) = gitdir::discover(worktree) else {
        return false;
    };
    if dirs.common_dir.join("lfs").exists() {
        return true;
    }
    crate::utils::git_command_at(worktree)
        .args(["config", "--get", "filter.lfs.process"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The branch `worktree` is on, or an empty string when it is detached or
/// unborn. Empty is the contract everywhere else in the hook context.
fn current_branch(worktree: &Path) -> String {
    crate::utils::git_command_at(worktree)
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// How a stage's jobs execute, which git never gets to say.
///
/// `--hooks <mode>` is a flag on daft's own commands; git invokes the shim
/// with the arguments its hook protocol defines and nothing else. So the
/// decision is made here, from the one property that settles it: whether git
/// is going to act on this hook's exit status.
///
/// A gate is [`Foreground`](crate::hooks::HookMode::Foreground). Git blocks
/// on the shim and reads its exit status; a job handed to the coordinator
/// outlives that read, so `background: true` under `pre-commit` does not
/// make the gate asynchronous, it deletes it — same class of silent
/// weakening as a takeover that runs less than its file says.
///
/// An observer keeps [`Auto`](crate::hooks::HookMode::Auto). The commit is
/// already made, the merge already landed; deferring work past it changes
/// *when* it happens and nothing else, which is the bargain `background:`
/// exists to make. Promoting there would take away the one place under a git
/// hook where the declaration is honest.
fn hook_mode_for(stage: GitStage) -> crate::hooks::HookMode {
    if stage.aborts_operation() {
        crate::hooks::HookMode::Foreground
    } else {
        crate::hooks::HookMode::Auto
    }
}

/// The names of jobs whose `background: true` [`hook_mode_for`] is about to
/// override.
///
/// Names what the config *declares*, not what will run: a job listed here can
/// still be filtered out by its own `glob:` or by `LEFTHOOK_EXCLUDE` before it
/// reaches a spec. The notice is worded to match.
///
/// Reads the definition rather than the built specs so the notice can be
/// emitted before the executor runs — and returns names, not a count,
/// because "which job" is the only part that tells someone what to edit.
/// Resolution goes through [`resolve_background`] so this cannot drift from
/// what the adapter will actually stamp on the spec, hook-level default
/// included. The unnamed fallback matches the label the rail will show.
///
/// [`resolve_background`]: crate::hooks::job_adapter::resolve_background
fn promoted_background_jobs(hook_def: &crate::hooks::yaml_config::HookDef) -> Vec<String> {
    use crate::hooks::job_adapter::resolve_background;
    let named = |job: &crate::hooks::yaml_config::JobDef, fallback: &str| {
        job.name.clone().unwrap_or_else(|| fallback.to_string())
    };
    let mut names: Vec<String> = hook_def
        .jobs
        .iter()
        .flatten()
        .filter(|job| resolve_background(job.background, hook_def.background))
        .map(|job| named(job, "(unnamed)"))
        .chain(
            hook_def
                .commands
                .iter()
                .flatten()
                .filter(|(_, cmd)| resolve_background(cmd.background, hook_def.background))
                // The map key is the job's name — `to_job_def` stamps it over
                // anything the value carries.
                .map(|(name, _)| name.clone()),
        )
        .collect();
    names.sort();
    names
}

/// Translate a hook result into the dispatch's terms.
fn into_run(result: HookResult) -> StageRun {
    if result.skipped {
        return StageRun {
            ran: false,
            success: true,
            exit_code: None,
            skip_reason: result.skip_reason,
        };
    }
    StageRun {
        ran: true,
        success: result.success,
        exit_code: result.exit_code,
        skip_reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::presenter::NullPresenter;
    use crate::output::TestOutput;
    use serial_test::serial;
    use std::fs;
    use tempfile::tempdir;

    /// Set/clear a variable around a test. `env::set_var` is `unsafe fn` in
    /// edition 2024; tests may wrap it, production code may not — which is
    /// why the guard itself rides the hook environment instead.
    fn set_var(name: &str, value: Option<&str>) {
        unsafe {
            match value {
                Some(v) => std::env::set_var(name, v),
                None => std::env::remove_var(name),
            }
        }
    }

    fn run_in(dir: &Path, stage: GitStage) -> StageRun {
        let mut output = TestOutput::new();
        run_git_stage(
            stage,
            dir,
            StagePayload::default(),
            NullPresenter::arc(),
            &mut output,
            false,
        )
        .unwrap()
    }

    #[test]
    #[serial]
    fn outside_a_repository_nothing_runs() {
        let tmp = tempdir().unwrap();
        let run = run_in(tmp.path(), GitStage::PreCommit);
        assert!(!run.ran);
        assert!(run.success);
        assert_eq!(
            run.skip_reason.as_deref(),
            Some("not inside a git repository")
        );
    }

    #[test]
    #[serial]
    fn a_repository_with_no_config_does_nothing() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("repo");
        fs::create_dir_all(root.join(".git")).unwrap();
        let run = run_in(&root, GitStage::PreCommit);
        assert!(!run.ran);
        assert!(run.success);
        assert_eq!(run.skip_reason.as_deref(), Some("no hooks config"));
    }

    #[test]
    #[serial]
    fn a_config_without_this_stage_does_nothing() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("repo");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(
            root.join("daft.yml"),
            "hooks:\n  pre-push:\n    jobs:\n      - name: t\n        run: \"true\"\n",
        )
        .unwrap();
        let run = run_in(&root, GitStage::PreCommit);
        assert!(!run.ran);
        assert_eq!(run.skip_reason.as_deref(), Some("no pre-commit definition"));
    }

    #[test]
    #[serial]
    fn a_broken_config_warns_rather_than_blocking_the_commit() {
        // "Fix your YAML" and "you cannot commit" must not be the same event.
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("repo");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(
            root.join("daft.yml"),
            "hooks:\n  pre-commit:\n    jobs: [[[\n",
        )
        .unwrap();

        let mut output = TestOutput::new();
        let run = run_git_stage(
            GitStage::PreCommit,
            &root,
            StagePayload::default(),
            NullPresenter::arc(),
            &mut output,
            false,
        )
        .unwrap();
        assert!(!run.ran);
        assert!(run.success, "a broken config must not fail the operation");
        assert!(
            output.warnings().iter().any(|w| w.contains("pre-commit")),
            "{:?}",
            output.warnings()
        );
    }

    #[test]
    #[serial]
    fn a_taken_over_config_actually_reaches_the_executor() {
        // Regression: the dispatcher resolved the incumbent config and then
        // let the executor load its own, which found no `daft.yml` and
        // concluded no hook was defined — a takeover that installed cleanly,
        // reported the right source, and gated nothing.
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("repo");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(
            root.join("lefthook.yml"),
            "pre-commit:\n  jobs:\n    - name: t\n      run: \"true\"\n",
        )
        .unwrap();

        let run = run_git_stage(
            GitStage::PreCommit,
            &root,
            StagePayload::default(),
            NullPresenter::arc(),
            &mut TestOutput::new(),
            // Explicit-invocation semantics: the trust gate is not what is
            // under test here.
            true,
        )
        .unwrap();
        assert!(run.ran, "the incumbent's pre-commit job must run: {run:?}");
        assert!(run.success);
    }

    #[test]
    #[serial]
    fn the_guard_stands_the_same_stage_down() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("repo");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(
            root.join("daft.yml"),
            "hooks:\n  pre-commit:\n    jobs:\n      - name: t\n        run: \"true\"\n",
        )
        .unwrap();

        set_var(super::super::STAGE_GUARD_VAR, Some("pre-commit"));
        let run = run_in(&root, GitStage::PreCommit);
        assert!(!run.ran);
        assert_eq!(
            run.skip_reason.as_deref(),
            Some("already running this stage")
        );

        // A different stage is unaffected — that is the whole point of
        // scoping the guard to one stage.
        let other = run_in(&root, GitStage::CommitMsg);
        assert_eq!(
            other.skip_reason.as_deref(),
            Some("no commit-msg definition")
        );
        set_var(super::super::STAGE_GUARD_VAR, None);
    }

    #[test]
    #[serial]
    fn the_kill_switch_stands_every_stage_down() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("repo");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(
            root.join("daft.yml"),
            "hooks:\n  pre-commit:\n    jobs:\n      - name: t\n        run: \"true\"\n",
        )
        .unwrap();

        for value in ["0", "false", "off", "no"] {
            set_var(KILL_SWITCH_VAR, Some(value));
            let run = run_in(&root, GitStage::PreCommit);
            assert!(!run.ran, "{value}");
            assert!(run.success, "{value}");
        }
        // Anything else leaves dispatch alone — `DAFT_HOOKS=1` must not read
        // as "disabled".
        set_var(KILL_SWITCH_VAR, Some("1"));
        assert_ne!(
            run_in(&root, GitStage::PreCommit).skip_reason.as_deref(),
            Some("DAFT_HOOKS is set to off")
        );
        set_var(KILL_SWITCH_VAR, None);
    }

    #[test]
    #[serial]
    fn a_running_stage_puts_the_guard_in_every_job_environment() {
        // The guard has to reach anything a job spawns, because a re-entered
        // shim reads the inherited environment — not daft's job map. Riding
        // the hook environment is what gives it that reach without daft
        // mutating its own process (`set_var` is unsafe in edition 2024).
        let ctx = HookContext::new(
            crate::hooks::HookType::Git(GitStage::PreCommit),
            "__hook",
            "/p",
            "/p/.git",
            "origin",
            "/p",
            "/p",
            "main",
        )
        .with_derived_env(
            [(
                super::super::STAGE_GUARD_VAR.to_string(),
                "pre-commit".to_string(),
            )]
            .into(),
        );
        let env = crate::hooks::HookEnvironment::from_context(&ctx);
        assert_eq!(env.get(super::super::STAGE_GUARD_VAR), Some("pre-commit"));
    }

    /// The gate/observer split, over the whole stage set rather than the two
    /// interesting members — a stage added later inherits an answer here
    /// whether or not anyone remembers this file exists.
    #[test]
    fn a_stage_git_acts_on_runs_inline_and_the_rest_keep_their_declaration() {
        for &stage in GitStage::all() {
            let expected = if stage.aborts_operation() {
                crate::hooks::HookMode::Foreground
            } else {
                crate::hooks::HookMode::Auto
            };
            assert_eq!(hook_mode_for(stage), expected, "{stage}");
            // …and the same property the executor reads for `--hooks
            // background`, so the two cannot answer differently.
            assert_eq!(
                crate::hooks::HookType::Git(stage).precedes_more_daft_work(),
                stage.aborts_operation(),
                "{stage}"
            );
        }
    }

    /// The acceptance criterion: `background: true` must not buy a job its
    /// way past a gate.
    ///
    /// Detached, this job's failure would be logged and notified while the
    /// hook returned successful and the shim exited 0 — the commit lands,
    /// ungated. Inline, it folds into the hook outcome, `pre-commit`'s
    /// default `abort` fail mode turns that into a non-zero exit, and git
    /// refuses the commit. Asserted on the outcome rather than on a marker
    /// file because only the inline path can produce it at all.
    #[test]
    #[serial]
    fn a_background_job_cannot_buy_its_way_past_a_gate() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("repo");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(
            root.join("daft.yml"),
            "hooks:\n  pre-commit:\n    jobs:\n      - name: gate\n        \
             background: true\n        run: \"false\"\n",
        )
        .unwrap();

        let mut output = TestOutput::new();
        let run = run_git_stage(
            GitStage::PreCommit,
            &root,
            StagePayload::default(),
            NullPresenter::arc(),
            &mut output,
            true,
        )
        .unwrap();

        assert!(run.ran, "{run:?}");
        assert!(
            !run.success,
            "a failing gate job must fail the stage even when it declared \
             `background: true`: {run:?}"
        );
        assert!(
            output.notices().iter().any(|m| m.contains("gate")),
            "the override must name the job it applied to: {:?}",
            output.notices()
        );
    }

    #[test]
    fn only_the_jobs_that_declared_background_are_named() {
        let def: crate::hooks::yaml_config::HookDef = serde_yaml::from_str(
            "background: true\njobs:\n  - name: inherits\n    run: \"true\"\n  \
             - name: opts-out\n    background: false\n    run: \"true\"\n",
        )
        .unwrap();
        assert_eq!(promoted_background_jobs(&def), vec!["inherits".to_string()]);

        // The older `commands:` map form is named by its key, and a hook with
        // nothing detached says nothing at all.
        let commands: crate::hooks::yaml_config::HookDef = serde_yaml::from_str(
            "commands:\n  fmt:\n    background: true\n    run: \"true\"\n  \
             lint:\n    run: \"true\"\n",
        )
        .unwrap();
        assert_eq!(
            promoted_background_jobs(&commands),
            vec!["fmt".to_string()],
            "the map key is the job name"
        );
        let quiet: crate::hooks::yaml_config::HookDef =
            serde_yaml::from_str("jobs:\n  - name: t\n    run: \"true\"\n").unwrap();
        assert!(promoted_background_jobs(&quiet).is_empty());
    }
}
