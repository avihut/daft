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
    let config = match crate::hooks::yaml_config_loader::load_merged_config(&dirs.worktree_root) {
        Ok(Some(config)) => config,
        Ok(None) => return Ok(StageRun::nothing("no daft.yml")),
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
    let executor = HookExecutor::new(hooks_config)?.with_bypass_trust(bypass_trust);

    let _ = hook_def;
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
        assert_eq!(run.skip_reason.as_deref(), Some("no daft.yml"));
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
}
