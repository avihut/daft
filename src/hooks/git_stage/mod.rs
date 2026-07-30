//! Git-stage hooks — daft as the repository's git hooks manager.
//!
//! Where the rest of [`crate::hooks`] gates daft's own worktree lifecycle,
//! this module gates git's: `pre-commit`, `commit-msg`, `pre-push` and the
//! rest of the stages git dispatches out of the hooks directory. Definitions
//! share the same `daft.yml` `hooks:` block and the same job-orchestration
//! semantics, so a stage is "just another hook" everywhere downstream of
//! config resolution.
//!
//! Submodules split along the two directions the feature runs in: outward
//! (writing shims into the hooks directory so git will call daft) and inward
//! (dispatching a stage when git does).

pub mod dispatch;
pub mod gitdir;
pub mod install;
pub mod shim;

use crate::hooks::FailMode;

/// A git hook stage daft can manage.
///
/// The closed set daft dispatches. Git defines more hooks than these; the ones
/// left out are left out on purpose, and [`rejection_reason`] explains each
/// refusal at the point a config names one. Two groups are excluded:
///
/// - **Protocol hooks** (`fsmonitor-watchman`, `proc-receive`,
///   `reference-transaction`) speak line protocols on stdin/stdout with git
///   and are re-invoked mid-transaction. Running arbitrary jobs there does not
///   mean "gate the operation", it means "corrupt the conversation".
/// - **Server-side hooks** (`pre-receive`, `update`, `post-receive`,
///   `post-update`, `push-to-checkout`) fire in the receiving repository. Daft
///   manages a developer's clone; installing these would put a gate somewhere
///   the person who configured it will never see it fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GitStage {
    PreCommit,
    PreMergeCommit,
    PrepareCommitMsg,
    CommitMsg,
    PostCommit,
    ApplypatchMsg,
    PreApplypatch,
    PostApplypatch,
    PreRebase,
    PostCheckout,
    /// git's `post-merge`. Its YAML key is `git-post-merge` — see
    /// [`Self::yaml_name`].
    PostMerge,
    PrePush,
    PreAutoGc,
    PostRewrite,
    SendemailValidate,
    PostIndexChange,
}

impl GitStage {
    /// Every stage, in the order they read best in help and status output:
    /// commit family, patch family, then the rest by when they fire.
    pub fn all() -> &'static [GitStage] {
        use GitStage::*;
        &[
            PreCommit,
            PreMergeCommit,
            PrepareCommitMsg,
            CommitMsg,
            PostCommit,
            ApplypatchMsg,
            PreApplypatch,
            PostApplypatch,
            PreRebase,
            PostCheckout,
            PostMerge,
            PrePush,
            PreAutoGc,
            PostRewrite,
            SendemailValidate,
            PostIndexChange,
        ]
    }

    /// The filename git dispatches this stage under, inside the hooks
    /// directory. This is the shim's name on disk.
    pub fn git_hook_filename(self) -> &'static str {
        use GitStage::*;
        match self {
            PreCommit => "pre-commit",
            PreMergeCommit => "pre-merge-commit",
            PrepareCommitMsg => "prepare-commit-msg",
            CommitMsg => "commit-msg",
            PostCommit => "post-commit",
            ApplypatchMsg => "applypatch-msg",
            PreApplypatch => "pre-applypatch",
            PostApplypatch => "post-applypatch",
            PreRebase => "pre-rebase",
            PostCheckout => "post-checkout",
            PostMerge => "post-merge",
            PrePush => "pre-push",
            PreAutoGc => "pre-auto-gc",
            PostRewrite => "post-rewrite",
            SendemailValidate => "sendemail-validate",
            PostIndexChange => "post-index-change",
        }
    }

    /// The key this stage is written under in `daft.yml`'s `hooks:` block.
    ///
    /// Identical to [`Self::git_hook_filename`] except for [`Self::PostMerge`]:
    /// daft already has a lifecycle hook named `post-merge` that fires after
    /// `daft merge`, and one YAML key cannot mean two events. The git stage
    /// takes the qualified spelling `git-post-merge`; the file on disk is
    /// still `post-merge`, because that name is git's to choose.
    pub fn yaml_name(self) -> &'static str {
        match self {
            GitStage::PostMerge => "git-post-merge",
            other => other.git_hook_filename(),
        }
    }

    /// Look up a stage by its `daft.yml` key.
    pub fn from_yaml_name(name: &str) -> Option<Self> {
        GitStage::all()
            .iter()
            .copied()
            .find(|s| s.yaml_name() == name)
    }

    /// Look up a stage by the filename git dispatched.
    pub fn from_git_filename(name: &str) -> Option<Self> {
        GitStage::all()
            .iter()
            .copied()
            .find(|s| s.git_hook_filename() == name)
    }

    /// The camelCase subsection under `daft.hooks.` in git config, matching
    /// the lifecycle hooks' convention (`daft.hooks.preCommit.failMode`).
    pub fn config_key(self) -> &'static str {
        use GitStage::*;
        match self {
            PreCommit => "preCommit",
            PreMergeCommit => "preMergeCommit",
            PrepareCommitMsg => "prepareCommitMsg",
            CommitMsg => "commitMsg",
            PostCommit => "postCommit",
            ApplypatchMsg => "applypatchMsg",
            PreApplypatch => "preApplypatch",
            PostApplypatch => "postApplypatch",
            PreRebase => "preRebase",
            PostCheckout => "postCheckout",
            // Qualified like the YAML key, and for the same reason: the
            // lifecycle hook already owns `postMerge`.
            PostMerge => "gitPostMerge",
            PrePush => "prePush",
            PreAutoGc => "preAutoGc",
            PostRewrite => "postRewrite",
            SendemailValidate => "sendemailValidate",
            PostIndexChange => "postIndexChange",
        }
    }

    /// Whether git aborts the operation when this hook exits non-zero.
    ///
    /// The single table behind [`Self::default_fail_mode`]. Defaulting a
    /// stage git ignores to `abort` would invent a gate that does not exist —
    /// daft would refuse a commit that git was going to make anyway — so the
    /// default tracks what git actually does with the exit code.
    pub fn aborts_operation(self) -> bool {
        use GitStage::*;
        match self {
            PreCommit | PreMergeCommit | PrepareCommitMsg | CommitMsg | ApplypatchMsg
            | PreApplypatch | PreRebase | PrePush | PreAutoGc | SendemailValidate => true,
            PostCommit | PostApplypatch | PostCheckout | PostMerge | PostRewrite
            | PostIndexChange => false,
        }
    }

    /// The default for `daft.hooks.<stage>.failMode`.
    pub fn default_fail_mode(self) -> FailMode {
        if self.aborts_operation() {
            FailMode::Abort
        } else {
            FailMode::Warn
        }
    }

    /// Whether git feeds this stage a payload on stdin that jobs may need.
    ///
    /// `pre-push` gets one `<local-ref> <local-oid> <remote-ref> <remote-oid>`
    /// line per ref; `post-rewrite` gets `<old-sha> <new-sha> [extra]`. The
    /// dispatcher drains stdin for these and republishes it — both to jobs
    /// that declare `use_stdin:` and as a `DAFT_*` variable — because a hook
    /// process can only read stdin once.
    pub fn expects_stdin(self) -> bool {
        matches!(self, GitStage::PrePush | GitStage::PostRewrite)
    }
}

impl std::fmt::Display for GitStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.yaml_name())
    }
}

/// Git hooks daft deliberately does not manage, each with the reason a config
/// naming one is refused rather than ignored.
///
/// Ignoring would be worse than refusing: a repo that writes `pre-receive:`
/// jobs and sees no error believes it has a gate. Returns `None` for names
/// that are not git hooks at all — those get the did-you-mean treatment
/// instead.
pub fn rejection_reason(name: &str) -> Option<&'static str> {
    match name {
        "fsmonitor-watchman" | "proc-receive" | "reference-transaction" => Some(
            "this is a protocol hook: git holds a line-by-line conversation with it on \
             stdin/stdout and may re-invoke it mid-transaction, so running jobs there \
             would corrupt the exchange rather than gate it",
        ),
        "pre-receive" | "update" | "post-receive" | "post-update" | "push-to-checkout" => Some(
            "this is a server-side hook: it fires in the repository being pushed to, not \
             in this clone, so a gate configured here would never run for you",
        ),
        _ => None,
    }
}

/// Parse the `pre-push` stdin block into refs.
///
/// Git's format is one `<local-ref> <local-oid> <remote-ref> <remote-oid>`
/// line per ref. Lines with the wrong field count are dropped rather than
/// erroring: the block is generated by git, so a malformed line means a git
/// version writing something this parser has not met, and losing one ref from
/// a file list is a better failure than refusing the push.
///
/// The inverse of what [`StageRunner::run_stage`] synthesizes on the
/// daft-runs-the-stage path, which is why both live behind one shape —
/// daft-managed and git-dispatched runs must see the same refs.
///
/// [`StageRunner::run_stage`]: crate::core::worktree::ports::StageRunner::run_stage
pub fn parse_push_refs(stdin: &str) -> Vec<crate::core::worktree::ports::PushRef> {
    use crate::core::worktree::ports::PushRef;
    stdin
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let [local_ref, local_oid, remote_ref, remote_oid] = fields[..] else {
                return None;
            };
            Some(PushRef {
                local_ref: local_ref.to_string(),
                local_oid: local_oid.to_string(),
                remote_ref: remote_ref.to_string(),
                remote_oid: remote_oid.to_string(),
            })
        })
        .collect()
}

/// Render refs into the exact stdin block git feeds a `pre-push` hook.
///
/// The other half of [`parse_push_refs`]: when daft runs the stage itself it
/// must hand jobs a payload indistinguishable from git's, or a `pre-push`
/// definition would behave differently depending on who invoked it.
pub fn render_push_refs(refs: &[crate::core::worktree::ports::PushRef]) -> String {
    refs.iter()
        .map(|r| {
            format!(
                "{} {} {} {}",
                r.local_ref, r.local_oid, r.remote_ref, r.remote_oid
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Environment variable naming the git stage daft is currently executing.
///
/// Set for the duration of a stage run — on the dispatching process, on every
/// job, and around [`StageRunner::run_stage`] — and honoured **same-stage
/// only**: a shim, a `__hook` entry, or a `manages_stage` probe stands down
/// when the value equals its own stage and proceeds otherwise.
///
/// Scoping it to one stage rather than to "any daft-spawned git" is
/// deliberate, and it is what drop-in fidelity costs. A `pre-commit` job that
/// commits, or a `pre-push` job that checks out a branch, must fire the
/// repo's `commit-msg` and `post-checkout` definitions exactly as an
/// incumbent manager would; a blanket guard would silently make daft-run
/// stages weaker than the same config run by the tool it replaced. The narrow
/// guard still closes the only loop that matters — a stage re-entering
/// itself.
///
/// [`StageRunner::run_stage`]: crate::core::worktree::ports::StageRunner::run_stage
pub const STAGE_GUARD_VAR: &str = "DAFT_STAGE_GUARD";

/// Whether a run of `stage` is already in progress in this process tree.
///
/// The single question every guard site asks. `stage` is the git hook
/// filename (`pre-commit`, `pre-push`, …), matching what
/// [`guard_value_for`] writes.
pub fn guard_blocks(stage: &str) -> bool {
    std::env::var(STAGE_GUARD_VAR).is_ok_and(|active| active == stage)
}

/// The `(name, value)` pair marking `stage` as in progress, for callers that
/// set it on a child process rather than on themselves.
///
/// Production code must not reach for `std::env::set_var` to do this — it is
/// an `unsafe fn` in edition 2024, and this crate forbids `unsafe` outside
/// tests. Pass the pair to `Command::env` instead.
pub fn guard_value_for(stage: &str) -> (&'static str, String) {
    (STAGE_GUARD_VAR, stage.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Set/clear the guard around a closure. `env::set_var` is `unsafe fn` in
    /// edition 2024; tests may wrap it, production code may not.
    fn with_guard<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
        let prior = std::env::var(STAGE_GUARD_VAR).ok();
        unsafe {
            match value {
                Some(v) => std::env::set_var(STAGE_GUARD_VAR, v),
                None => std::env::remove_var(STAGE_GUARD_VAR),
            }
        }
        let out = f();
        unsafe {
            match prior {
                Some(v) => std::env::set_var(STAGE_GUARD_VAR, v),
                None => std::env::remove_var(STAGE_GUARD_VAR),
            }
        }
        out
    }

    #[test]
    #[serial]
    fn unset_guard_blocks_nothing() {
        with_guard(None, || {
            assert!(!guard_blocks("pre-commit"));
            assert!(!guard_blocks("pre-push"));
        });
    }

    #[test]
    #[serial]
    fn guard_blocks_only_its_own_stage() {
        with_guard(Some("pre-commit"), || {
            assert!(guard_blocks("pre-commit"));
            // The whole point: a pre-commit job that commits still gets its
            // commit-msg hook, exactly as it would under any other manager.
            assert!(!guard_blocks("commit-msg"));
            assert!(!guard_blocks("post-commit"));
        });
    }

    #[test]
    fn guard_value_names_the_stage() {
        let (name, value) = guard_value_for("pre-push");
        assert_eq!(name, STAGE_GUARD_VAR);
        assert_eq!(value, "pre-push");
    }

    #[test]
    fn every_stage_round_trips_through_both_name_tables() {
        for &stage in GitStage::all() {
            assert_eq!(
                GitStage::from_git_filename(stage.git_hook_filename()),
                Some(stage)
            );
            assert_eq!(GitStage::from_yaml_name(stage.yaml_name()), Some(stage));
        }
    }

    #[test]
    fn stage_names_and_config_keys_are_unique() {
        let mut names: Vec<&str> = GitStage::all().iter().map(|s| s.yaml_name()).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate yaml_name");

        let mut keys: Vec<&str> = GitStage::all().iter().map(|s| s.config_key()).collect();
        keys.sort_unstable();
        let count = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), count, "duplicate config_key");
    }

    #[test]
    fn git_post_merge_is_qualified_in_config_but_not_on_disk() {
        // The collision this exists to resolve: daft's own `post-merge`
        // lifecycle hook already owns the plain key.
        assert_eq!(GitStage::PostMerge.yaml_name(), "git-post-merge");
        assert_eq!(GitStage::PostMerge.config_key(), "gitPostMerge");
        // …but git chooses the filename, so the shim keeps git's spelling.
        assert_eq!(GitStage::PostMerge.git_hook_filename(), "post-merge");
        assert_eq!(
            GitStage::from_git_filename("post-merge"),
            Some(GitStage::PostMerge)
        );
        // And the qualified key must not resolve as a filename.
        assert_eq!(GitStage::from_git_filename("git-post-merge"), None);
    }

    #[test]
    fn no_stage_name_collides_with_a_lifecycle_hook_name() {
        for &stage in GitStage::all() {
            assert_eq!(
                crate::hooks::yaml_config::LIFECYCLE_HOOK_NAMES
                    .iter()
                    .find(|n| **n == stage.yaml_name()),
                None,
                "{stage} collides with a lifecycle hook name",
            );
        }
    }

    #[test]
    fn fail_mode_default_follows_what_git_does_with_the_exit_code() {
        // Gates: git aborts, so daft aborts.
        for stage in [
            GitStage::PreCommit,
            GitStage::CommitMsg,
            GitStage::PrePush,
            GitStage::PreRebase,
        ] {
            assert!(stage.aborts_operation(), "{stage}");
            assert_eq!(stage.default_fail_mode(), FailMode::Abort, "{stage}");
        }
        // Observers: git ignores the exit code. Aborting here would invent a
        // gate that does not exist.
        for stage in [
            GitStage::PostCommit,
            GitStage::PostCheckout,
            GitStage::PostMerge,
            GitStage::PostIndexChange,
        ] {
            assert!(!stage.aborts_operation(), "{stage}");
            assert_eq!(stage.default_fail_mode(), FailMode::Warn, "{stage}");
        }
    }

    #[test]
    fn only_the_two_payload_stages_expect_stdin() {
        let with_stdin: Vec<&str> = GitStage::all()
            .iter()
            .filter(|s| s.expects_stdin())
            .map(|s| s.yaml_name())
            .collect();
        assert_eq!(with_stdin, vec!["pre-push", "post-rewrite"]);
    }

    #[test]
    fn refused_hooks_explain_themselves_and_managed_ones_do_not() {
        for name in [
            "fsmonitor-watchman",
            "proc-receive",
            "reference-transaction",
        ] {
            assert!(
                rejection_reason(name).is_some_and(|r| r.contains("protocol hook")),
                "{name}"
            );
        }
        for name in [
            "pre-receive",
            "update",
            "post-receive",
            "post-update",
            "push-to-checkout",
        ] {
            assert!(
                rejection_reason(name).is_some_and(|r| r.contains("server-side hook")),
                "{name}"
            );
        }
        // A managed stage is not a refusal, and neither is a typo — those get
        // did-you-mean treatment instead.
        assert_eq!(rejection_reason("pre-commit"), None);
        assert_eq!(rejection_reason("pre-comit"), None);
    }
}
