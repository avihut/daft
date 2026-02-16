//! Shortcut definitions and utilities for daft commands.
//!
//! This module provides centralized shortcut aliases for daft commands.
//! Shortcuts come in three styles:
//!
//! - **Git style** (default): `gwtclone`, `gwtco`, `gwtcb`, etc.
//! - **Shell style**: `gwco`, `gwcob`, `gwcobd`
//! - **Legacy style**: `gclone`, `gcw`, `gcbw`, etc.
//!
//! Users can enable/disable styles via `daft setup shortcuts`.

use std::fmt;
use std::str::FromStr;

/// Available shortcut styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShortcutStyle {
    /// Git-focused style: gwtclone, gwtco, gwtcb, etc.
    Git,
    /// Shell-focused style: gwco, gwcob, gwcobd
    Shell,
    /// Legacy style from older versions: gclone, gcw, gcbw, etc.
    Legacy,
}

impl ShortcutStyle {
    /// Returns all available shortcut styles.
    pub fn all() -> &'static [ShortcutStyle] {
        &[
            ShortcutStyle::Git,
            ShortcutStyle::Shell,
            ShortcutStyle::Legacy,
        ]
    }

    /// Returns the style name as a string.
    pub fn name(&self) -> &'static str {
        match self {
            ShortcutStyle::Git => "git",
            ShortcutStyle::Shell => "shell",
            ShortcutStyle::Legacy => "legacy",
        }
    }

    /// Returns a description of the style.
    pub fn description(&self) -> &'static str {
        match self {
            ShortcutStyle::Git => "Git worktree focused (gwtclone, gwtco, gwtcm, gwtcb, ...)",
            ShortcutStyle::Shell => "Shell-friendly short aliases (gwco, gwcob)",
            ShortcutStyle::Legacy => "Legacy style from older versions (gclone, gcw, gcbw, ...)",
        }
    }
}

impl fmt::Display for ShortcutStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl FromStr for ShortcutStyle {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "git" => Ok(ShortcutStyle::Git),
            "shell" => Ok(ShortcutStyle::Shell),
            "legacy" => Ok(ShortcutStyle::Legacy),
            _ => Err(format!("Unknown shortcut style: {s}")),
        }
    }
}

/// A shortcut alias mapping.
#[derive(Debug, Clone)]
pub struct Shortcut {
    /// The alias name (e.g., "gwtco")
    pub alias: &'static str,
    /// The full command name (e.g., "git-worktree-checkout")
    pub command: &'static str,
    /// The style this shortcut belongs to
    pub style: ShortcutStyle,
}

/// All available shortcuts across all styles.
///
/// Note: gwtcm, gwtcbm, gwcobd, and gcbdw are shell-init-only shortcuts
/// that resolve the default branch dynamically. They are not included here
/// because they cannot be implemented as simple symlink aliases.
pub const SHORTCUTS: &[Shortcut] = &[
    // Git style (8 shortcuts)
    Shortcut {
        alias: "gwtclone",
        command: "git-worktree-clone",
        style: ShortcutStyle::Git,
    },
    Shortcut {
        alias: "gwtinit",
        command: "git-worktree-init",
        style: ShortcutStyle::Git,
    },
    Shortcut {
        alias: "gwtco",
        command: "git-worktree-checkout",
        style: ShortcutStyle::Git,
    },
    Shortcut {
        alias: "gwtcb",
        command: "git-worktree-checkout-branch",
        style: ShortcutStyle::Git,
    },
    Shortcut {
        alias: "gwtprune",
        command: "git-worktree-prune",
        style: ShortcutStyle::Git,
    },
    Shortcut {
        alias: "gwtcarry",
        command: "git-worktree-carry",
        style: ShortcutStyle::Git,
    },
    Shortcut {
        alias: "gwtfetch",
        command: "git-worktree-fetch",
        style: ShortcutStyle::Git,
    },
    Shortcut {
        alias: "gwtbd",
        command: "git-worktree-branch-delete",
        style: ShortcutStyle::Git,
    },
    // Shell style (2 shortcuts)
    Shortcut {
        alias: "gwco",
        command: "git-worktree-checkout",
        style: ShortcutStyle::Shell,
    },
    Shortcut {
        alias: "gwcob",
        command: "git-worktree-checkout-branch",
        style: ShortcutStyle::Shell,
    },
    // Legacy style (4 shortcuts)
    Shortcut {
        alias: "gclone",
        command: "git-worktree-clone",
        style: ShortcutStyle::Legacy,
    },
    Shortcut {
        alias: "gcw",
        command: "git-worktree-checkout",
        style: ShortcutStyle::Legacy,
    },
    Shortcut {
        alias: "gcbw",
        command: "git-worktree-checkout-branch",
        style: ShortcutStyle::Legacy,
    },
    Shortcut {
        alias: "gprune",
        command: "git-worktree-prune",
        style: ShortcutStyle::Legacy,
    },
];

/// Resolves a shortcut alias to its full command name.
///
/// Returns the original name if no shortcut matches.
pub fn resolve(name: &str) -> &str {
    for shortcut in SHORTCUTS {
        if shortcut.alias == name {
            return shortcut.command;
        }
    }
    name
}

/// Returns all shortcuts for a given style.
pub fn shortcuts_for_style(style: ShortcutStyle) -> Vec<&'static Shortcut> {
    SHORTCUTS.iter().filter(|s| s.style == style).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_resolve_git_style() {
        assert_eq!(resolve("gwtclone"), "git-worktree-clone");
        assert_eq!(resolve("gwtco"), "git-worktree-checkout");
        assert_eq!(resolve("gwtcb"), "git-worktree-checkout-branch");
        assert_eq!(resolve("gwtprune"), "git-worktree-prune");
        assert_eq!(resolve("gwtcarry"), "git-worktree-carry");
        assert_eq!(resolve("gwtfetch"), "git-worktree-fetch");
        assert_eq!(resolve("gwtbd"), "git-worktree-branch-delete");
        assert_eq!(resolve("gwtinit"), "git-worktree-init");
    }

    #[test]
    fn test_resolve_shell_style() {
        assert_eq!(resolve("gwco"), "git-worktree-checkout");
        assert_eq!(resolve("gwcob"), "git-worktree-checkout-branch");
    }

    #[test]
    fn test_resolve_legacy_style() {
        assert_eq!(resolve("gclone"), "git-worktree-clone");
        assert_eq!(resolve("gcw"), "git-worktree-checkout");
        assert_eq!(resolve("gcbw"), "git-worktree-checkout-branch");
        assert_eq!(resolve("gprune"), "git-worktree-prune");
    }

    #[test]
    fn test_resolve_unknown() {
        assert_eq!(resolve("unknown"), "unknown");
        assert_eq!(resolve("git-worktree-clone"), "git-worktree-clone");
        assert_eq!(resolve("daft"), "daft");
    }

    #[test]
    fn test_no_duplicate_aliases() {
        let aliases: Vec<&str> = SHORTCUTS.iter().map(|s| s.alias).collect();
        let unique: HashSet<&str> = aliases.iter().copied().collect();
        assert_eq!(
            aliases.len(),
            unique.len(),
            "Found duplicate aliases in SHORTCUTS"
        );
    }

    #[test]
    fn test_all_shortcuts_map_to_valid_commands() {
        let valid_commands = [
            "git-worktree-clone",
            "git-worktree-init",
            "git-worktree-checkout",
            "git-worktree-checkout-branch",
            "git-worktree-prune",
            "git-worktree-carry",
            "git-worktree-branch-delete",
            "git-worktree-fetch",
        ];

        for shortcut in SHORTCUTS {
            assert!(
                valid_commands.contains(&shortcut.command),
                "Shortcut '{}' maps to invalid command '{}'",
                shortcut.alias,
                shortcut.command
            );
        }
    }

    #[test]
    fn test_shell_only_shortcuts_not_in_shortcuts() {
        // gwtcm, gwtcbm, gwcobd, gcbdw are shell-init-only shortcuts
        assert_eq!(resolve("gwtcm"), "gwtcm");
        assert_eq!(resolve("gwtcbm"), "gwtcbm");
        assert_eq!(resolve("gwcobd"), "gwcobd");
        assert_eq!(resolve("gcbdw"), "gcbdw");
    }

    #[test]
    fn test_shortcuts_for_style() {
        let git_shortcuts = shortcuts_for_style(ShortcutStyle::Git);
        assert_eq!(git_shortcuts.len(), 8);

        let shell_shortcuts = shortcuts_for_style(ShortcutStyle::Shell);
        assert_eq!(shell_shortcuts.len(), 2);

        let legacy_shortcuts = shortcuts_for_style(ShortcutStyle::Legacy);
        assert_eq!(legacy_shortcuts.len(), 4);
    }

    #[test]
    fn test_style_from_str() {
        assert_eq!(ShortcutStyle::from_str("git").unwrap(), ShortcutStyle::Git);
        assert_eq!(ShortcutStyle::from_str("Git").unwrap(), ShortcutStyle::Git);
        assert_eq!(ShortcutStyle::from_str("GIT").unwrap(), ShortcutStyle::Git);
        assert_eq!(
            ShortcutStyle::from_str("shell").unwrap(),
            ShortcutStyle::Shell
        );
        assert_eq!(
            ShortcutStyle::from_str("legacy").unwrap(),
            ShortcutStyle::Legacy
        );
        assert!(ShortcutStyle::from_str("invalid").is_err());
    }

    #[test]
    fn test_style_display() {
        assert_eq!(ShortcutStyle::Git.to_string(), "git");
        assert_eq!(ShortcutStyle::Shell.to_string(), "shell");
        assert_eq!(ShortcutStyle::Legacy.to_string(), "legacy");
    }
}
