//! Helpers shared across unit-test modules.
//!
//! Compiled only under `cfg(test)`. Anything here is test scaffolding, never a
//! production code path — put runtime helpers in the module that owns them.

use std::path::{Path, PathBuf};

/// Restores the process working directory on drop, even on panic.
///
/// The working directory is process-global and unit tests share one process,
/// so a test that moves it must both hold this guard and be `#[serial]`.
/// Without the guard a failing assertion strands every later test in a
/// since-deleted tempdir, where `std::env::current_dir` — and so every git
/// subprocess that resolves a repo from cwd — fails for reasons that have
/// nothing to do with the test that reports them.
pub struct CwdGuard {
    original: PathBuf,
}

impl Default for CwdGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl CwdGuard {
    /// Remember the current directory, staying where we are.
    pub fn new() -> Self {
        Self {
            original: std::env::current_dir().expect("cwd readable at test start"),
        }
    }

    /// Remember the current directory, then move to `target`.
    pub fn enter(target: &Path) -> Self {
        let guard = Self::new();
        std::env::set_current_dir(target).expect("cd into test directory");
        guard
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        // Best-effort: the original directory may itself have been removed
        // (a tempdir the test tore down). Landing on temp_dir still leaves the
        // process somewhere readable, which is what later tests need.
        if std::env::set_current_dir(&self.original).is_err() {
            let _ = std::env::set_current_dir(std::env::temp_dir());
        }
    }
}
