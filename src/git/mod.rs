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
        if enabled {
            GITOXIDE_NOTICE.call_once(|| {
                eprintln!("[experimental] Using gitoxide backend for git operations");
            });
        }
        self
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
}
