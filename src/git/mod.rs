use anyhow::{Context, Result};
use std::sync::{Once, OnceLock};

mod branch;
mod clone;
mod config;
pub(crate) mod oxide;
mod refs;
mod remote;
mod stash;
mod worktree;

static GITOXIDE_NOTICE: Once = Once::new();

/// Returns true (once per process) if the gitoxide experimental notice should be shown.
/// Safe to call multiple times; only the first call returns true.
pub fn should_show_gitoxide_notice(use_gitoxide: bool) -> bool {
    if use_gitoxide {
        let mut fired = false;
        GITOXIDE_NOTICE.call_once(|| {
            fired = true;
        });
        return fired;
    }
    false
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

pub struct GitCommand {
    pub(crate) quiet: bool,
    pub(crate) use_gitoxide: bool,
    pub(crate) gix_repo: OnceLock<gix::ThreadSafeRepository>,
}

impl GitCommand {
    pub fn new(quiet: bool) -> Self {
        Self {
            quiet,
            use_gitoxide: false,
            gix_repo: OnceLock::new(),
        }
    }

    pub fn with_gitoxide(mut self, enabled: bool) -> Self {
        self.use_gitoxide = enabled;
        self
    }

    /// Returns true (once per process) if the gitoxide notice should be shown.
    pub fn take_gitoxide_notice(&self) -> bool {
        should_show_gitoxide_notice(self.use_gitoxide)
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

        // Git env vars (set when tests run under a git hook) would redirect
        // discovery to the host repo — strip them from process + subprocess so
        // `gix::discover` resolves the temp repo below. Only safe under #[serial].
        const GIT_ENV_VARS: &[&str] = &[
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_COMMON_DIR",
            "GIT_CEILING_DIRECTORIES",
        ];
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
}
