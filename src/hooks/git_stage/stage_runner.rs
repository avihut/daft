//! The [`StageRunner`] adapter — daft running a stage around its own push.
//!
//! Declared by #599 and left `Noop` until now. With it wired, `daft push`
//! runs the repository's `pre-push` definition itself, as first-class job
//! rows, and then pushes with git's hook dispatch suppressed so nothing fires
//! twice. That substitution is only legitimate because the stage daft runs is
//! the *same run* git's shim would have produced — same entry point, same
//! payload, same trust gate.

use super::{GitStage, dispatch, gitdir, install::Sentinel};
use crate::core::worktree::ports::{PushRef, StageOutcome, StageRunner};
use crate::executor::presenter::JobPresenter;
use crate::hooks::incumbent::{self, StageSource};
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;

/// The adapter daft's push paths use.
pub struct GitStageRunner;

/// The process-wide instance.
///
/// A unit struct, so this is a convenience rather than a cache — the probe
/// below is cheap enough not to need one, and a cache would have to be
/// invalidated when a config changes mid-process, which `daft sync` does
/// across many worktrees.
pub fn runner() -> &'static GitStageRunner {
    &GitStageRunner
}

impl StageRunner for GitStageRunner {
    /// Whether daft should run this stage itself rather than letting git
    /// dispatch it.
    ///
    /// Called on every push, and capped by its port contract at stat-level
    /// probes — so this reads files and never spawns a process. Four
    /// conditions, each of which failing means "let git do it":
    ///
    /// 1. **Not already inside this stage.** Otherwise a `pre-push` job that
    ///    pushes would recurse.
    /// 2. **The stage is defined somewhere.** No definition, nothing to run.
    /// 3. **For a taken-over config, the shims must be installed.** A
    ///    `lefthook.yml` alone must never flip daft's pushes to Path B: that
    ///    repository's own hooks already fire through git, and running them
    ///    twice — once by daft, once by git — is the failure mode. The
    ///    install sentinel is the record of someone having asked.
    /// 4. **The repository must be trusted.** `StageOutcome.skipped` is never
    ///    read by the push path, so declining here is the only channel an
    ///    untrusted repository has: answering `false` degrades to Path A,
    ///    where git still runs whatever hooks are installed and daft's stage
    ///    simply does not happen.
    fn manages_stage(&self, stage: &str, repo_cwd: &Path) -> bool {
        if super::guard_blocks(stage) {
            return false;
        }
        let Some(stage_kind) = GitStage::from_git_filename(stage) else {
            return false;
        };
        let Some(dirs) = gitdir::discover(repo_cwd) else {
            return false;
        };

        let native = crate::hooks::yaml_config_loader::load_merged_config(&dirs.worktree_root)
            .ok()
            .flatten();
        let defined = match incumbent::resolve_stage_source(&dirs.worktree_root, native.as_ref()) {
            StageSource::Native => native
                .as_ref()
                .is_some_and(|c| c.hooks.contains_key(stage_kind.yaml_name())),
            StageSource::Incumbent(_) => {
                Sentinel::exists(&dirs)
                    && incumbent::detect(&dirs.worktree_root).is_ok_and(|spec| {
                        spec.is_some_and(|s| s.config.hooks.contains_key(stage_kind.yaml_name()))
                    })
            }
            StageSource::None => false,
        };
        if !defined {
            return false;
        }

        // Raw trust level: the verified check spawns git to read the remote
        // URL, which this path may not do. A repository whose fingerprint has
        // drifted is caught by `run_stage`, which re-verifies before running
        // anything.
        crate::hooks::TrustDatabase::load()
            .unwrap_or_default()
            .get_trust_level(&dirs.common_dir)
            != crate::hooks::TrustLevel::Deny
    }

    /// Run the stage, with the payload git would have supplied.
    ///
    /// The refs are rendered into the exact block git writes to a `pre-push`
    /// hook's stdin. Anything less and a definition would behave differently
    /// depending on whether daft or git invoked it, which would make the
    /// whole substitution unsound.
    fn run_stage(
        &self,
        stage: &str,
        worktree_cwd: Option<&Path>,
        refs: &[PushRef],
        presenter: Arc<dyn JobPresenter>,
    ) -> Result<StageOutcome> {
        let Some(stage_kind) = GitStage::from_git_filename(stage) else {
            return Ok(StageOutcome {
                success: true,
                skipped: true,
                reason: Some(format!("daft does not manage the {stage} stage")),
            });
        };
        let cwd = match worktree_cwd {
            Some(cwd) => cwd.to_path_buf(),
            None => std::env::current_dir()?,
        };

        // The two arguments git passes a `pre-push` hook. `origin` is the
        // only remote daft pushes to, and a job reading them is asking what
        // git would have said, not what daft prefers.
        let remote = "origin".to_string();
        let url = remote_url(&cwd, &remote).unwrap_or_default();
        let payload = dispatch::StagePayload {
            argv: vec![remote, url],
            stdin: (!refs.is_empty()).then(|| super::render_push_refs(refs)),
            index_file: None,
        };

        let mut output = crate::output::CliOutput::new(crate::output::OutputConfig::default());
        let run = dispatch::run_git_stage(
            stage_kind,
            &cwd,
            payload,
            presenter,
            &mut output,
            // Not bypassed: a push is not the explicit consent that typing
            // `daft hooks run pre-push` is.
            false,
        )?;

        Ok(StageOutcome {
            success: run.success,
            skipped: !run.ran,
            reason: run.skip_reason,
        })
    }
}

/// The push URL for the configured remote, for the second hook argument.
///
/// Best-effort: a job that reads `DAFT_PUSH_REMOTE_URL` gets an empty string
/// rather than a failed push if the remote cannot be resolved.
fn remote_url(cwd: &Path, remote: &str) -> Option<String> {
    let output = crate::utils::git_command_at(cwd)
        .args(["remote", "get-url", remote])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::fs;
    use tempfile::tempdir;

    fn repo_with(files: &[(&str, &str)]) -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("repo");
        fs::create_dir_all(root.join(".git")).unwrap();
        for (name, body) in files {
            fs::write(root.join(name), body).unwrap();
        }
        (tmp, root)
    }

    const PRE_PUSH: &str =
        "hooks:\n  pre-push:\n    jobs:\n      - name: t\n        run: \"true\"\n";

    #[test]
    #[serial]
    fn an_undefined_stage_is_left_to_git() {
        let (_tmp, root) = repo_with(&[("daft.yml", PRE_PUSH)]);
        assert!(!runner().manages_stage("pre-commit", &root));
    }

    #[test]
    #[serial]
    fn a_stage_daft_does_not_manage_is_left_to_git() {
        let (_tmp, root) = repo_with(&[("daft.yml", PRE_PUSH)]);
        assert!(!runner().manages_stage("pre-receive", &root));
    }

    #[test]
    #[serial]
    fn outside_a_repository_nothing_is_managed() {
        let tmp = tempdir().unwrap();
        assert!(!runner().manages_stage("pre-push", tmp.path()));
    }

    #[test]
    #[serial]
    fn an_incumbent_config_alone_does_not_flip_the_push_path() {
        // The failure this prevents: that repository's hooks already fire
        // through git, so taking Path B without an install would run them
        // twice — once by daft, once by git.
        let (_tmp, root) = repo_with(&[(
            "lefthook.yml",
            "pre-push:\n  jobs:\n    - name: t\n      run: \"true\"\n",
        )]);
        assert!(!runner().manages_stage("pre-push", &root));
    }

    #[test]
    #[serial]
    fn a_stage_already_running_is_not_re_entered() {
        let (_tmp, root) = repo_with(&[("daft.yml", PRE_PUSH)]);
        unsafe { std::env::set_var(super::super::STAGE_GUARD_VAR, "pre-push") };
        let managed = runner().manages_stage("pre-push", &root);
        unsafe { std::env::remove_var(super::super::STAGE_GUARD_VAR) };
        assert!(!managed, "a pre-push job that pushes must not recurse");
    }

    #[test]
    fn refs_render_into_gits_exact_stdin_block() {
        // The substitution is only sound if daft's payload is
        // indistinguishable from git's.
        let refs = vec![
            PushRef {
                local_ref: "refs/heads/f".into(),
                local_oid: "aaa".into(),
                remote_ref: "refs/heads/f".into(),
                remote_oid: "bbb".into(),
            },
            PushRef {
                local_ref: "(delete)".into(),
                local_oid: "0".repeat(40),
                remote_ref: "refs/heads/gone".into(),
                remote_oid: "ccc".into(),
            },
        ];
        let rendered = super::super::render_push_refs(&refs);
        assert_eq!(
            rendered,
            format!(
                "refs/heads/f aaa refs/heads/f bbb\n(delete) {} refs/heads/gone ccc",
                "0".repeat(40)
            )
        );
        // …and parsing it back yields the same refs, so the shim path and
        // this one cannot drift.
        let parsed = super::super::parse_push_refs(&rendered);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].local_ref, "refs/heads/f");
        assert_eq!(parsed[1].remote_ref, "refs/heads/gone");
    }
}
