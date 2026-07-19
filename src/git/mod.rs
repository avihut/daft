use anyhow::{Context, Result};
use std::sync::OnceLock;

mod branch;
pub mod cancel;
mod clone;
mod config;
pub(crate) mod oxide;
#[cfg(unix)]
pub(crate) mod process_tree;
pub mod push_porcelain;
mod refs;
mod remote;
mod stash;
mod worktree;

pub use remote::{PushIo, PushOptions, PushOutputTee, PushStream};

// Per-thread count of `gix::discover()` calls (test-only probe).
//
// Used by the shared-`GitCommand` regression test to assert a command shares a
// single repo discovery across its settings load, hooks-config load, and body
// rather than re-discovering per throwaway instance (#584). Thread-local keeps
// it isolated under parallel `cargo test`.
#[cfg(test)]
thread_local! {
    pub(crate) static DISCOVER_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Reset the per-thread discover counter (test-only).
#[cfg(test)]
pub(crate) fn reset_discover_count() {
    DISCOVER_COUNT.with(|c| c.set(0));
}

/// Read the per-thread discover counter (test-only).
#[cfg(test)]
pub(crate) fn discover_count() -> usize {
    DISCOVER_COUNT.with(|c| c.get())
}

/// Sync-push supervision extras carried on [`GitCommand`] (#678), the same
/// way the cancel flag rides it: `execute_push_task` constructs one
/// `GitCommand` per push unit, so per-unit observers attach here without
/// widening the whole push call chain (`PushOptions`, `push_with_hooks`,
/// `push_single_worktree` stay untouched, and every non-sync push site is
/// byte-identical).
#[derive(Default)]
pub(crate) struct PushSupervision {
    /// Receives the `git push` root pid right after spawn (the resource
    /// governor's unit registry).
    pub(crate) on_spawn: Option<std::sync::Arc<dyn Fn(u32) + Send + Sync>>,
    /// Wall-clock budget per push unit (`daft.sync.pushTimeout`). A fresh
    /// [`cancel::UnitClock`] is armed for every `git push` this command
    /// runs — the sequential engine reuses one `GitCommand` across
    /// branches, so the budget must be per-invocation, not per-command.
    /// Expiry tears the unit's tree down; the push fails with a timeout
    /// hint.
    pub(crate) timeout: Option<std::time::Duration>,
    /// Receives each freshly armed unit clock (paired with `on_spawn`'s
    /// pid) so the resource governor can pause it during a freeze —
    /// frozen time must not count against the budget (#678 stage 3).
    pub(crate) on_clock:
        Option<std::sync::Arc<dyn Fn(std::sync::Arc<cancel::UnitClock>) + Send + Sync>>,
    /// Extra environment for the `git push` subprocess — the governor's
    /// shared jobserver export, inherited by the pre-push hook (#678).
    pub(crate) env: Vec<(String, String)>,
}

pub struct GitCommand {
    pub(crate) quiet: bool,
    /// Whether dispatched ops take the gix arm. Constructed `false`: the
    /// settings resolver (`DaftSettings::use_gitoxide`, default on since
    /// #733) is the sole opt-in source via `with_gitoxide`, so call sites
    /// that never thread the setting stay on the subprocess backend.
    pub(crate) use_gitoxide: bool,
    pub(crate) gix_repo: OnceLock<gix::ThreadSafeRepository>,
    /// Shared cancellation flag observed by the long-running subprocess
    /// seams (fetch/pull/rebase/push). `None` keeps those seams
    /// cancel-unaware; commands that own a Ctrl+C handler (sync) inject
    /// their flag here so every worker-thread git call inherits it.
    pub(crate) cancel: Option<std::sync::Arc<cancel::CancelFlag>>,
    /// Sync-push supervision extras (governor observers). `None` for every
    /// non-sync caller.
    pub(crate) push_supervision: Option<PushSupervision>,
}

impl GitCommand {
    pub fn new(quiet: bool) -> Self {
        Self {
            quiet,
            use_gitoxide: false,
            gix_repo: OnceLock::new(),
            cancel: None,
            push_supervision: None,
        }
    }

    /// Attach sync-push supervision extras (#678). Only `run_push` reads
    /// them; other subprocess seams ignore the field entirely.
    pub(crate) fn with_push_supervision(mut self, supervision: PushSupervision) -> Self {
        self.push_supervision = Some(supervision);
        self
    }

    pub fn with_gitoxide(mut self, enabled: bool) -> Self {
        self.use_gitoxide = enabled;
        self
    }

    /// Attach a shared cancel flag, opting this command's subprocess
    /// seams (fetch/pull/rebase/push) into supervision: each child gets
    /// its own process group, escalations tear the tree down by pgid,
    /// and a job-control stop (background-group tty read) surfaces as
    /// [`cancel::NeedsTerminal`]. Without a flag the seams keep classic
    /// blocking behavior in the caller's group — terminal auth prompts
    /// and Ctrl+C reach them exactly as before cancellation existed.
    pub fn with_cancel(mut self, cancel: std::sync::Arc<cancel::CancelFlag>) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// The injected cancel flag, in the borrowed form the subprocess
    /// helpers take.
    pub(crate) fn cancel_flag(&self) -> Option<&cancel::CancelFlag> {
        self.cancel.as_deref()
    }

    /// Whether an attached cancel flag has gone active. Cheap enough to
    /// poll at the top of a per-worktree loop so sequential engines stop
    /// scheduling new work the moment a cancel lands (rather than
    /// fast-failing every remaining worktree through a torn-down subprocess).
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel_flag()
            .is_some_and(cancel::CancelFlag::is_cancelled)
    }

    /// Lazily discover and open the git repository via gitoxide.
    /// Returns a thread-local Repository handle.
    pub(crate) fn gix_repo(&self) -> Result<gix::Repository> {
        if let Some(ts) = self.gix_repo.get() {
            return Ok(ts.to_thread_local());
        }
        let cwd = std::env::current_dir().context("Failed to get current working directory")?;
        let ts = gix::ThreadSafeRepository::discover(&cwd)
            .context("Failed to discover git repository via gitoxide")?;
        #[cfg(test)]
        DISCOVER_COUNT.with(|c| c.set(c.get() + 1));
        // If another thread raced us via set(), that's fine - use whichever won
        let _ = self.gix_repo.set(ts);
        Ok(self
            .gix_repo
            .get()
            .expect("OnceLock should be set")
            .to_thread_local())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Git env vars (set when tests run under a git hook) that would redirect
    /// repo discovery to the host repo instead of a test's temp repo. The
    /// `#[serial]` tests below strip them from the *process*; their git
    /// subprocesses are scrubbed by `git_at` instead.
    const GIT_ENV_VARS: &[&str] = &[
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
        "GIT_CEILING_DIRECTORIES",
    ];

    /// Seed a test repo by running `git` in `cwd`, asserting it succeeded.
    ///
    /// Goes through `crate::utils::git_command_at` — the helper CLAUDE.md's
    /// Test Hygiene rule mandates — so subprocesses get the same eight
    /// discovery vars stripped that production strips. A hand-rolled list
    /// drifts from it silently; the one this replaced had already lost
    /// `GIT_NAMESPACE`, which would have re-scoped every seeded ref when the
    /// suite runs from a hook that exports it.
    ///
    /// `commit.gpgsign=false` keeps seeding working for developers who sign
    /// commits globally and run `cargo test` / an IDE runner directly, where
    /// the suite's `GIT_CONFIG_COUNT` scrub in `_state_guard_lib.sh` is
    /// absent. Asserting the status turns a failed seed into "git commit
    /// failed: <stderr>" instead of a bare missing-ref assertion later.
    fn git_at(cwd: &std::path::Path, args: &[&str]) {
        let out = crate::utils::git_command_at(cwd)
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn test_git_command_new() {
        let git = GitCommand::new(true);
        assert!(git.quiet);
        assert!(!git.use_gitoxide);

        let git = GitCommand::new(false);
        assert!(!git.quiet);
        assert!(!git.use_gitoxide);
    }

    #[test]
    fn test_git_command_with_gitoxide() {
        let git = GitCommand::new(false).with_gitoxide(true);
        assert!(!git.quiet);
        assert!(git.use_gitoxide);

        let git = GitCommand::new(true).with_gitoxide(false);
        assert!(git.quiet);
        assert!(!git.use_gitoxide);
    }

    /// Restores the working directory on drop — even on panic — so a failing
    /// assertion in a cwd-changing `#[serial]` test can't strand a sibling test
    /// in a since-deleted tempdir. Mirrors the guard in the merge/branch-delete
    /// tests (the codebase has no shared test-util home for it yet).
    struct CwdGuard {
        original: std::path::PathBuf,
    }

    impl CwdGuard {
        fn new() -> Self {
            Self {
                original: std::env::current_dir().expect("cwd readable at test start"),
            }
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            // Best-effort: if the original cwd is gone, fall back to temp_dir so
            // subsequent tests can at least read cwd.
            if std::env::set_current_dir(&self.original).is_err() {
                let _ = std::env::set_current_dir(std::env::temp_dir());
            }
        }
    }

    /// #584 regression: a command that shares one `GitCommand` across its
    /// settings load, hooks-config load, and body must discover the repo
    /// exactly once — not once per throwaway instance. Guards against any
    /// future change that reintroduces per-call discovery.
    #[test]
    #[serial_test::serial]
    fn shared_git_command_discovers_repo_once() {
        use crate::core::settings::{DaftSettings, load_hooks_config_with};

        // Strip discovery-redirecting env vars so `gix::discover` resolves
        // the temp repo below. Only safe under #[serial].
        for var in GIT_ENV_VARS {
            unsafe {
                std::env::remove_var(var);
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap();
        let mut init = std::process::Command::new("git");
        for var in GIT_ENV_VARS {
            init.env_remove(var);
        }
        init.args(["init", "-b", "main"])
            .arg(&path)
            .current_dir(&path)
            .output()
            .unwrap();

        // Restore cwd on drop (even on panic) so a failure here can't strand a
        // sibling #[serial] test in this since-deleted tempdir.
        let _cwd_guard = CwdGuard::new();
        std::env::set_current_dir(&path).unwrap();

        // Shared: one instance across all three config-reading phases.
        reset_discover_count();
        let git = GitCommand::new(true);
        let _settings = DaftSettings::load_with(&git).unwrap();
        let _hooks = load_hooks_config_with(&git).unwrap();
        let _ = git.config_get("user.email");
        let shared = discover_count();

        // Contrast: three independent instances (the pre-#584 pattern) each
        // discover — proves the probe increments and that sharing is the cause.
        reset_discover_count();
        let _settings = DaftSettings::load_with(&GitCommand::new(true)).unwrap();
        let _hooks = load_hooks_config_with(&GitCommand::new(true)).unwrap();
        let _ = GitCommand::new(true).config_get("user.email");
        let separate = discover_count();

        assert_eq!(
            shared, 1,
            "shared GitCommand must discover the repo exactly once"
        );
        assert_eq!(
            separate, 3,
            "independent instances each discover (guards the probe)"
        );
    }

    /// #733 opt-out: a command with `use_gitoxide` false must take the
    /// subprocess arm for every dispatched op. Every gix arm goes through
    /// `gix_repo()`, so the discover counter doubles as a "was any gix path
    /// taken" probe: zero discoveries ⇒ zero gix paths.
    #[test]
    #[serial_test::serial]
    fn gitoxide_opt_out_takes_no_gix_path() {
        for var in GIT_ENV_VARS {
            unsafe {
                std::env::remove_var(var);
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap();
        git_at(&path, &["init", "-b", "main"]);
        git_at(&path, &["commit", "--allow-empty", "-m", "seed"]);

        let _cwd_guard = CwdGuard::new();
        std::env::set_current_dir(&path).unwrap();

        // Opt-out: dispatched ops must never reach gix.
        reset_discover_count();
        let git = GitCommand::new(true).with_gitoxide(false);
        assert!(git.show_ref_exists("refs/heads/main").unwrap());
        assert_eq!(git.symbolic_ref_short_head().unwrap(), "main");
        assert!(git.rev_parse_is_inside_work_tree().unwrap());
        assert_eq!(discover_count(), 0, "opt-out command must take no gix path");

        // Control: the same ops with gitoxide on discover exactly once —
        // proves the probe observes these ops and the arms actually differ.
        reset_discover_count();
        let git = GitCommand::new(true).with_gitoxide(true);
        assert!(git.show_ref_exists("refs/heads/main").unwrap());
        assert_eq!(git.symbolic_ref_short_head().unwrap(), "main");
        assert!(git.rev_parse_is_inside_work_tree().unwrap());
        assert_eq!(
            discover_count(),
            1,
            "gix-backed command shares one discovery"
        );
    }

    /// #733 graduation regressions in the remote-probe family, pinned from a
    /// fresh bare clone (the state daft's clone flow probes from):
    ///
    /// 1. URL-shaped ls-remote probes must take the subprocess arm — the gix
    ///    arm derives its protocol-v2 ref-prefix filter from the configured
    ///    remote's fetch refspecs, so an ad-hoc URL remote yields an empty
    ///    ref map. A configured remote name with refspecs keeps gix.
    /// 2. `list_remote_branches` must answer from the network — a fresh bare
    ///    clone has no `refs/remotes/<remote>/` refs, and the local-ref arm
    ///    that once served it declared every branch missing, so multi-branch
    ///    clone created no worktrees.
    #[test]
    #[serial_test::serial]
    fn fresh_bare_clone_remote_probes_fall_back_to_cli() {
        for var in GIT_ENV_VARS {
            unsafe {
                std::env::remove_var(var);
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();

        // A "remote" with a develop branch…
        let src = base.join("src");
        std::fs::create_dir(&src).unwrap();
        git_at(&src, &["init", "-b", "main"]);
        git_at(&src, &["commit", "--allow-empty", "-m", "seed"]);
        git_at(&src, &["branch", "develop"]);

        // …and a bare clone probing it, with origin's fetch refspec
        // configured the way daft's own clone sets it up (bare clones have
        // none by default).
        let src_url = src.to_str().unwrap().to_owned();
        git_at(
            &base,
            &["clone", "--quiet", "--bare", &src_url, "probe.git"],
        );
        let probe = base.join("probe.git");
        git_at(
            &probe,
            &[
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/remotes/origin/*",
            ],
        );

        let _cwd_guard = CwdGuard::new();
        std::env::set_current_dir(&probe).unwrap();

        let git = GitCommand::new(true).with_gitoxide(true);

        // Configured name with refspecs: the gix arm answers (and discovers).
        reset_discover_count();
        assert!(git.ls_remote_branch_exists("origin", "develop").unwrap());
        assert_eq!(discover_count(), 1, "name-shaped probe stays on gix");

        // URL/path-shaped remote: must bypass gix and still find the branch.
        assert!(
            git.ls_remote_branch_exists(&src_url, "develop").unwrap(),
            "URL-shaped probe must find the branch via the CLI arm"
        );

        // Symref by URL (clone's default-branch detection) is CLI-only.
        let symref = git.ls_remote_symref(&src_url).unwrap();
        assert!(
            symref.contains("refs/heads/main"),
            "symref must expose remote HEAD, got: {symref}"
        );

        // Fresh bare clone: refs/remotes/origin/* is empty until the first
        // fetch, so the listing must come from the network instead of
        // declaring every branch missing (the multi-branch-clone regression).
        let listed = git.list_remote_branches("origin").unwrap();
        assert!(
            listed.contains(&"develop".to_string()),
            "unfetched bare clone must list remote branches from the network, got {listed:?}"
        );
    }

    /// #733 review: the gix ls-remote gate must check that a remote's fetch
    /// refspecs *cover* `refs/heads/`, not merely that it has some refspec.
    ///
    /// gix builds its protocol-v2 ref-prefix filter from those refspecs, so
    /// a narrow single-branch refspec — what `git clone --single-branch` and
    /// `--depth` leave behind, and a state `daft doctor` only warns about —
    /// makes the server advertise that one branch and every other branch
    /// read as absent from the remote. `daft prune` then treated a live
    /// upstream as gone (deleting the worktree and local ref of a merged
    /// branch) and `daft checkout <branch>` refused to create a worktree for
    /// a branch that exists.
    #[test]
    #[serial_test::serial]
    fn narrow_fetch_refspec_takes_the_cli_arm() {
        for var in GIT_ENV_VARS {
            unsafe {
                std::env::remove_var(var);
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();

        // A remote carrying two branches…
        let src = base.join("src");
        std::fs::create_dir(&src).unwrap();
        git_at(&src, &["init", "-b", "main"]);
        git_at(&src, &["commit", "--allow-empty", "-m", "seed"]);
        git_at(&src, &["branch", "develop"]);

        // …and a clone that only ever tracks `main`, the way
        // `git clone --single-branch` configures it.
        let src_url = src.to_str().unwrap().to_owned();
        git_at(
            &base,
            &["clone", "--quiet", "--bare", &src_url, "probe.git"],
        );
        let probe = base.join("probe.git");
        let set_refspec = |spec: &str| {
            git_at(&probe, &["config", "remote.origin.fetch", spec]);
        };
        set_refspec("+refs/heads/main:refs/remotes/origin/main");

        let _cwd_guard = CwdGuard::new();
        std::env::set_current_dir(&probe).unwrap();

        // A narrow refspec must not engage gix: its ref map would hold only
        // `main`, hiding `develop` behind a "not found on remote".
        reset_discover_count();
        let listed = GitCommand::new(true)
            .with_gitoxide(true)
            .list_remote_branches("origin")
            .unwrap();
        assert!(
            listed.contains(&"develop".to_string()),
            "a branch outside the fetch refspec must still be listed, got {listed:?}"
        );

        // Control: widen the refspec and gix takes over again — proving the
        // gate discriminates on coverage rather than disabling the arm.
        set_refspec("+refs/heads/*:refs/remotes/origin/*");
        reset_discover_count();
        let listed = GitCommand::new(true)
            .with_gitoxide(true)
            .list_remote_branches("origin")
            .unwrap();
        assert!(
            listed.contains(&"develop".to_string()) && listed.contains(&"main".to_string()),
            "wildcard refspec must list every head, got {listed:?}"
        );
        assert_eq!(
            discover_count(),
            1,
            "a heads-covering refspec keeps the gix arm"
        );
    }
}
