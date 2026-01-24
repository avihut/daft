//! First-run hints for improved user experience.
//!
//! This module provides a system for showing one-time hints to users
//! who may not have completed setup for optional features like shell
//! integration.

use crate::output::Output;
use crate::SHELL_WRAPPER_ENV;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::PathBuf;

/// Environment variable to suppress all hints.
pub const NO_HINTS_ENV: &str = "DAFT_NO_HINTS";

/// State file for tracking which hints have been shown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HintsState {
    /// Schema version for future migrations.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Set of hint IDs that have been shown.
    #[serde(default)]
    pub shown: HashSet<String>,
}

fn default_version() -> u32 {
    1
}

impl Default for HintsState {
    fn default() -> Self {
        Self {
            version: 1,
            shown: HashSet::new(),
        }
    }
}

impl HintsState {
    /// Load the hints state from the default location.
    pub fn load() -> Result<Self> {
        let path = Self::default_path()?;
        Self::load_from(&path)
    }

    /// Load the hints state from a specific path.
    pub fn load_from(path: &PathBuf) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(path)
            .with_context(|| format!("Failed to read hints state from {}", path.display()))?;

        serde_json::from_str(&contents)
            .with_context(|| format!("Failed to parse hints state from {}", path.display()))
    }

    /// Save the hints state to the default location.
    pub fn save(&self) -> Result<()> {
        let path = Self::default_path()?;
        self.save_to(&path)
    }

    /// Save the hints state to a specific path.
    pub fn save_to(&self, path: &PathBuf) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {}", parent.display()))?;
        }

        let contents =
            serde_json::to_string_pretty(self).context("Failed to serialize hints state")?;

        fs::write(path, contents)
            .with_context(|| format!("Failed to write hints state to {}", path.display()))?;

        Ok(())
    }

    /// Get the default path for the hints state file.
    pub fn default_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir().context("Could not determine config directory")?;
        Ok(config_dir.join("daft").join("hints.json"))
    }

    /// Check if a hint has been shown.
    pub fn has_shown(&self, hint_id: &str) -> bool {
        self.shown.contains(hint_id)
    }

    /// Mark a hint as shown.
    pub fn mark_shown(&mut self, hint_id: &str) {
        self.shown.insert(hint_id.to_string());
    }
}

/// Hint identifier for shell integration.
pub const SHELL_INTEGRATION_HINT: &str = "shell-integration";

/// Check if hints are globally disabled via environment variable.
pub fn hints_disabled() -> bool {
    env::var(NO_HINTS_ENV).is_ok()
}

/// Check if the shell wrapper is active.
pub fn shell_wrapper_active() -> bool {
    env::var(SHELL_WRAPPER_ENV).is_ok()
}

/// Show the shell integration hint if appropriate.
///
/// This should be called after a successful worktree creation operation.
/// The hint is shown only if:
/// - Hints are not disabled via DAFT_NO_HINTS
/// - The shell wrapper is not active (DAFT_SHELL_WRAPPER not set)
/// - The hint hasn't been shown before
/// - Quiet mode is not enabled
///
/// Returns Ok(true) if the hint was shown, Ok(false) otherwise.
pub fn maybe_show_shell_hint(output: &mut dyn Output) -> Result<bool> {
    // Skip if hints are disabled
    if hints_disabled() {
        return Ok(false);
    }

    // Skip if quiet mode
    if output.is_quiet() {
        return Ok(false);
    }

    // Skip if shell wrapper is already active - user has already set it up
    if shell_wrapper_active() {
        return Ok(false);
    }

    // Load state and check if we've shown this hint before
    let mut state = HintsState::load().unwrap_or_default();
    if state.has_shown(SHELL_INTEGRATION_HINT) {
        return Ok(false);
    }

    // Show the hint
    output.info("");
    output.info("hint: Enable shell integration to auto-cd into new worktrees:");
    output.info("  eval \"$(daft shell-init bash)\"   # Add to ~/.bashrc or ~/.zshrc");
    output.info("  daft shell-init fish | source    # Add to ~/.config/fish/config.fish");
    output.info("Run 'daft shell-init --help' for more options.");
    output.info(&format!("To suppress hints: export {NO_HINTS_ENV}=1"));

    // Mark as shown and save
    state.mark_shown(SHELL_INTEGRATION_HINT);
    // Don't fail the command if we can't save state - the hint might show again next time
    // but that's acceptable behavior
    let _ = state.save();

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_hints_state_default() {
        let state = HintsState::default();
        assert_eq!(state.version, 1);
        assert!(state.shown.is_empty());
    }

    #[test]
    fn test_hints_state_mark_shown() {
        let mut state = HintsState::default();
        assert!(!state.has_shown("test-hint"));
        state.mark_shown("test-hint");
        assert!(state.has_shown("test-hint"));
    }

    #[test]
    fn test_hints_state_save_and_load() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().join("hints.json");

        let mut state = HintsState::default();
        state.mark_shown("test-hint");
        state.save_to(&path).unwrap();

        let loaded = HintsState::load_from(&path).unwrap();
        assert!(loaded.has_shown("test-hint"));
    }

    #[test]
    fn test_hints_state_load_missing_file() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().join("nonexistent.json");

        let state = HintsState::load_from(&path).unwrap();
        assert!(state.shown.is_empty());
    }
}
