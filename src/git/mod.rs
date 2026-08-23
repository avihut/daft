use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

mod branch;
pub mod cancel;
mod clone;
mod config;
pub mod op_state;
pub(crate) mod oxide;
#[cfg(unix)]
pub(crate) mod process_tree;
pub mod push_porcelain;
mod refs;
mod remote;
mod stash;
mod worktree;
pub mod worktree_state;

pub use branch::{BranchTracking, UpstreamRef};
pub use config::{ConfigEntry, ConfigScope, daft_config_entries_global};
pub use refs::FirstParentCommit;
pub use remote::{PushIo, PushOptions, PushOutputTee, PushStream};

/// First git release with `merge-tree --write-tree` (the in-memory three-way
/// merge the squash probe needs).
const MERGE_TREE_MIN_VERSION: (u64, u64) = (2, 38);

static MERGE_TREE_CAPABLE: OnceLock<bool> = OnceLock::new();

/// Whether the installed git can run `merge-tree --write-tree`, probed once
/// per process.
///
/// This is daft's only runtime git-version gate. It exists because the
/// context-insensitive squash probe is an *optional extra* on top of the
/// patch-id checks: where git is too old the probe is skipped and detection
/// falls back to its previous behavior, so an unknown or unparseable version
/// must answer `false` rather than risk a confusing subcommand error.
pub(crate) fn supports_merge_tree() -> bool {
    *MERGE_TREE_CAPABLE.get_or_init(|| {
        std::process::Command::new("git")
            .arg("--version")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .is_some_and(|version| version_supports_merge_tree(&version))
    })
}

/// Decide whether `git --version` output names a release at or past
/// [`MERGE_TREE_MIN_VERSION`]. Distribution suffixes ride along after the
/// numbers (`2.39.5 (Apple Git-154)`, `2.45.0.windows.1`) and are ignored;
/// anything that does not parse answers `false`.
fn version_supports_merge_tree(version_output: &str) -> bool {
    let Some(rest) = version_output.trim().strip_prefix("git version ") else {
        return false;
    };
    let mut numbers = rest
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .split('.');
    let (Some(Ok(major)), Some(Ok(minor))) = (
        numbers.next().map(str::parse::<u64>),
        numbers.next().map(str::parse::<u64>),
    ) else {
        return false;
    };
    (major, minor) >= MERGE_TREE_MIN_VERSION
}

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
    /// Repository handles discovered by the gix arm, keyed by the working
    /// directory each was discovered from (#868). One `GitCommand` lives
    /// across the `chdir`s a worktree walk makes, and a worktree-scoped
    /// question (current branch, toplevel, git dir) must be answered for
    /// the worktree the process is standing in — exactly what the subprocess
    /// arm gets for free from `git`'s own cwd discovery. Keying by cwd keeps
    /// the discover-once property a command relies on (#584) while a walk
    /// that returns to a worktree reuses its handle.
    gix_repos: Mutex<HashMap<PathBuf, gix::ThreadSafeRepository>>,
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
            gix_repos: Mutex::new(HashMap::new()),
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

    /// The gitoxide repository the process working directory is inside,
    /// discovered lazily and cached per working directory (see `gix_repos`).
    /// Returns a thread-local Repository handle.
    ///
    /// Asks `current_dir()` on every call on purpose: a cached handle is
    /// only reused for the directory it was discovered from, so a command
    /// that `chdir`s between worktrees gets each worktree's own answer rather
    /// than the first one's (#868) — the property every `git` child process
    /// has for free, since each discovers from its own cwd.
    pub(crate) fn gix_repo(&self) -> Result<gix::Repository> {
        let cwd = std::env::current_dir().context("Failed to get current working directory")?;
        self.gix_repo_at(&cwd)
    }

    /// The cached handle for `dir`, discovering on first use.
    ///
    /// The mutex is interior mutability behind `&self`, not contention:
    /// gix is built without its `parallel` feature, so `ThreadSafeRepository`
    /// holds `Rc`s and `GitCommand` is `!Sync` — no two threads ever share
    /// one (`list_stream.rs` builds a command per worker for exactly that
    /// reason). Holding it across `discover` is therefore deadlock-free by
    /// construction.
    fn gix_repo_at(&self, dir: &Path) -> Result<gix::Repository> {
        let shared = {
            let mut repos = self
                .gix_repos
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match repos.get(dir) {
                // `ThreadSafeRepository` is a bundle of `Arc`s: cloning it out
                // keeps the lock scoped to the lookup.
                Some(ts) => ts.clone(),
                None => {
                    let ts = gix::ThreadSafeRepository::discover(dir)
                        .context("Failed to discover git repository via gitoxide")?;
                    #[cfg(test)]
                    DISCOVER_COUNT.with(|c| c.set(c.get() + 1));
                    repos.insert(dir.to_path_buf(), ts.clone());
                    ts
                }
            }
        };
        Ok(shared.to_thread_local())
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

        let git = GitCommand::new(false);
        assert!(!git.quiet);
    }

    #[test]
    fn merge_tree_gate_accepts_2_38_and_newer() {
        for version in [
            "git version 2.38.0",
            "git version 2.39.5 (Apple Git-154)",
            "git version 2.45.0.windows.1",
            "git version 3.0.0",
            "git version 2.55.0\n",
        ] {
            assert!(
                version_supports_merge_tree(version),
                "{version} should be capable"
            );
        }
    }

    #[test]
    fn merge_tree_gate_rejects_older_and_unparseable() {
        for version in [
            "git version 2.37.9",
            "git version 2.5.0",
            "git version 1.9.1",
            // Anything the parser can't read must fail closed: the probe is
            // an optional extra, never worth a confusing subcommand error.
            "git version banana",
            "git version 2",
            "",
            "some other tool 9.9.9",
        ] {
            assert!(
                !version_supports_merge_tree(version),
                "{version} should not be capable"
            );
        }
    }

    use crate::test_support::CwdGuard;

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

    /// #883: a bare `GitCommand` now answers from the gix arm, so the four
    /// `src/core/repo.rs` wrappers (`is_git_repository`, `get_git_common_dir`,
    /// `get_current_worktree_path`, `get_current_branch`) do too. Pin that
    /// the gix answers are what `git` itself says in every state they are
    /// asked from — a worktree root, a subdirectory, a linked worktree, a
    /// bare repository, an unborn branch, and outside any repository.
    #[test]
    #[serial_test::serial]
    fn bare_git_command_answers_match_the_git_cli() {
        for var in GIT_ENV_VARS {
            unsafe {
                std::env::remove_var(var);
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let main_wt = base.join("main");
        std::fs::create_dir(&main_wt).unwrap();
        git_at(&main_wt, &["init", "-b", "main"]);
        git_at(&main_wt, &["commit", "--allow-empty", "-m", "seed"]);
        let sub = main_wt.join("src").join("deep");
        std::fs::create_dir_all(&sub).unwrap();
        let feat_wt = base.join("feat");
        git_at(
            &main_wt,
            &["worktree", "add", "-b", "feat", feat_wt.to_str().unwrap()],
        );
        let bare = base.join("bare.git");
        git_at(
            &base,
            &[
                "clone",
                "--quiet",
                "--bare",
                main_wt.to_str().unwrap(),
                "bare.git",
            ],
        );
        let unborn = base.join("unborn");
        std::fs::create_dir(&unborn).unwrap();
        git_at(&unborn, &["init", "-b", "fresh"]);
        let outside = base.join("outside");
        std::fs::create_dir(&outside).unwrap();

        // What git says, asked at `cwd`.
        let cli = |cwd: &std::path::Path, args: &[&str]| -> Option<String> {
            let out = crate::utils::git_command_at(cwd)
                .args(args)
                .output()
                .unwrap();
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        };
        let canon = |cwd: &std::path::Path, p: &str| cwd.join(p).canonicalize().unwrap();

        let _cwd_guard = CwdGuard::new();
        let git = GitCommand::new(true);

        for cwd in [&main_wt, &sub, &feat_wt, &bare, &unborn] {
            std::env::set_current_dir(cwd).unwrap();
            let here = cwd.display();
            reset_discover_count();

            assert!(git.is_inside_git_repo().unwrap(), "{here}: inside a repo");
            assert!(cli(cwd, &["rev-parse", "--git-dir"]).is_some());

            let expected_common =
                canon(cwd, &cli(cwd, &["rev-parse", "--git-common-dir"]).unwrap());
            let common = git.rev_parse_git_common_dir().unwrap();
            assert_eq!(canon(cwd, &common), expected_common, "{here}: common dir");

            let expected_top = cli(cwd, &["rev-parse", "--show-toplevel"]);
            let top = git.get_current_worktree_path().ok();
            assert_eq!(
                top.map(|p| p.canonicalize().unwrap()),
                expected_top.map(|p| canon(cwd, &p)),
                "{here}: toplevel"
            );

            let expected_branch = cli(cwd, &["symbolic-ref", "--short", "HEAD"]);
            assert_eq!(
                git.symbolic_ref_short_head().ok(),
                expected_branch,
                "{here}: current branch"
            );
            // The answers above came from gix — one discovery for this cwd,
            // shared by the three handle-backed questions — not from a
            // subprocess arm that would trivially agree with the CLI.
            assert_eq!(discover_count(), 1, "{here}: answers came from gix");
        }

        std::env::set_current_dir(&outside).unwrap();
        assert!(!git.is_inside_git_repo().unwrap(), "outside: not a repo");
        assert!(cli(&outside, &["rev-parse", "--git-dir"]).is_none());
    }

    /// #868 regression: one `GitCommand` reused across a `chdir` must answer
    /// worktree-scoped questions for the worktree the process is standing
    /// in. The predecessor cached a single discovery per process, so every
    /// later worktree inherited the first one's branch, toplevel, git dir,
    /// and dirtiness — silently, and only on the gix arm.
    #[test]
    #[serial_test::serial]
    fn gix_arm_answers_for_the_worktree_the_process_stands_in() {
        for var in GIT_ENV_VARS {
            unsafe {
                std::env::remove_var(var);
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let main_wt = base.join("main");
        std::fs::create_dir(&main_wt).unwrap();
        git_at(&main_wt, &["init", "-b", "main"]);
        git_at(&main_wt, &["commit", "--allow-empty", "-m", "seed"]);
        let feat_wt = base.join("feat");
        git_at(
            &main_wt,
            &["worktree", "add", "-b", "feat", feat_wt.to_str().unwrap()],
        );
        // Only the linked worktree is dirty, so a stale handle is caught by
        // the status question too, not just by ref/path questions.
        std::fs::write(feat_wt.join("wip.txt"), "wip").unwrap();

        let _cwd_guard = CwdGuard::new();
        let git = GitCommand::new(true);

        // Discover from the main worktree first…
        std::env::set_current_dir(&main_wt).unwrap();
        reset_discover_count();
        assert_eq!(git.symbolic_ref_short_head().unwrap(), "main");
        assert_eq!(git.get_current_worktree_path().unwrap(), main_wt);
        assert!(!git.has_uncommitted_changes().unwrap());
        let main_git_dir = git.get_git_dir().unwrap();
        assert_eq!(discover_count(), 1);

        // …then walk into the linked one with the same command: every
        // worktree-scoped answer must follow the cwd.
        std::env::set_current_dir(&feat_wt).unwrap();
        assert_eq!(
            git.symbolic_ref_short_head().unwrap(),
            "feat",
            "current branch must be the worktree's own, not the first one's"
        );
        assert_eq!(git.get_current_worktree_path().unwrap(), feat_wt);
        assert!(
            git.has_uncommitted_changes().unwrap(),
            "dirtiness must be read from the worktree the process stands in"
        );
        assert_ne!(
            git.get_git_dir().unwrap(),
            main_git_dir,
            "a linked worktree has its own git dir"
        );
        assert!(git.rev_parse_is_inside_work_tree().unwrap());
        assert!(!git.rev_parse_is_bare_repository().unwrap());
        assert_eq!(discover_count(), 2, "a new cwd is a new discovery");

        // Returning to a worktree reuses its handle rather than discovering
        // again — the cache is keyed by cwd, not dropped.
        std::env::set_current_dir(&main_wt).unwrap();
        assert_eq!(git.symbolic_ref_short_head().unwrap(), "main");
        assert_eq!(
            discover_count(),
            2,
            "revisiting a worktree must reuse its cached handle"
        );
    }

    /// #733 remote-probe routing, pinned from a fresh bare clone (the state
    /// daft's clone flow probes from). Each probe must reach the right
    /// backend:
    ///
    /// 1. Single-ref existence (`ls_remote_branch_exists`) is CLI-always —
    ///    it hands `refs/heads/<branch>` to the server for a one-ref answer,
    ///    which gix can't express (its ref prefixes come from the remote's
    ///    refspecs), so even a configured remote name takes no gix path.
    /// 2. URL-shaped probes — single-ref, symref, and bulk listing alike —
    ///    are CLI too: the gix arm derives its ref-prefix filter from
    ///    configured fetch refspecs, and an ad-hoc URL remote has none.
    /// 3. Bulk listing (`list_remote_branches`) of a configured remote whose
    ///    refspec covers `refs/heads/*` takes the gix network arm — a fresh
    ///    bare clone has no local remote-tracking refs, so it must reach the
    ///    server rather than read empty local refs and declare every branch
    ///    missing (the multi-branch-clone regression).
    #[test]
    #[serial_test::serial]
    fn fresh_bare_clone_remote_probes_pick_the_right_backend() {
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

        let git = GitCommand::new(true);

        // Existence probe is CLI even for a configured name: it must find the
        // branch and take no gix path.
        reset_discover_count();
        assert!(git.ls_remote_branch_exists("origin", "develop").unwrap());
        assert_eq!(
            discover_count(),
            0,
            "single-ref existence must never take the gix arm"
        );

        // URL/path-shaped remote: also CLI, still finds the branch.
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

        // Bulk listing of the configured remote takes the gix network arm and
        // must reach the server — refs/remotes/origin/* is empty until the
        // first fetch, so a local-ref answer would be "no branches" (the
        // multi-branch-clone regression).
        reset_discover_count();
        let listed = git.list_remote_branches("origin").unwrap();
        assert!(
            listed.contains(&"develop".to_string()),
            "fresh bare clone must list remote branches from the network, got {listed:?}"
        );
        assert_eq!(
            discover_count(),
            1,
            "bulk listing of a covering-refspec remote uses the gix arm"
        );

        // Bulk listing of a URL-shaped remote: no configured refspecs to
        // derive a ref prefix from, so `gix_repo_for_remote` declines and the
        // CLI arm answers — from the server, finding the same branch. This is
        // the capability fallback that outlived the backend switch (#883).
        let listed = git.list_remote_branches(&src_url).unwrap();
        assert!(
            listed.contains(&"develop".to_string()),
            "URL-shaped bulk listing must still reach the server via the CLI arm, got {listed:?}"
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
