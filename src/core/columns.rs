//! Column definitions for the `--columns` flag on list/sync/prune commands.
//!
//! Defines the user-facing column names, canonical ordering, and per-command
//! default sets. Separate from the TUI `Column` enum which includes TUI-only
//! variants like `Status`.

use std::fmt;
use std::str::FromStr;

/// A column that can be selected via `--columns` on list, sync, and prune.
///
/// Variants are ordered by canonical display position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListColumn {
    /// Current/default branch annotation markers.
    Annotation,
    /// Branch name.
    Branch,
    /// Worktree path.
    Path,
    /// Disk size of the worktree folder.
    Size,
    /// Ahead/behind base branch.
    Base,
    /// Local changes (staged/unstaged/untracked).
    Changes,
    /// Ahead/behind remote tracking branch.
    Remote,
    /// Branch age since creation.
    Age,
    /// Branch owner (from git author email).
    Owner,
    /// Abbreviated commit hash (7 chars) of the worktree HEAD.
    Hash,
    /// Last commit age + subject.
    LastCommit,
}

impl ListColumn {
    /// All columns in canonical display order.
    pub fn all() -> &'static [ListColumn] {
        &[
            ListColumn::Annotation,
            ListColumn::Branch,
            ListColumn::Path,
            ListColumn::Size,
            ListColumn::Base,
            ListColumn::Changes,
            ListColumn::Remote,
            ListColumn::Age,
            ListColumn::Owner,
            ListColumn::Hash,
            ListColumn::LastCommit,
        ]
    }

    /// The default column set for the list command.
    /// Size is excluded — it must be explicitly added via `--columns +size`.
    pub fn list_defaults() -> &'static [ListColumn] {
        &[
            ListColumn::Annotation,
            ListColumn::Branch,
            ListColumn::Path,
            ListColumn::Base,
            ListColumn::Changes,
            ListColumn::Remote,
            ListColumn::Age,
            ListColumn::Owner,
            ListColumn::LastCommit,
        ]
    }

    /// The default column set for sync and prune commands.
    /// (Status is pinned separately by TUI code, not included here.)
    /// Size is excluded — it must be explicitly added via `--columns +size`.
    pub fn tui_defaults() -> &'static [ListColumn] {
        Self::list_defaults()
    }

    pub fn clone_defaults() -> &'static [ListColumn] {
        &[
            ListColumn::Branch,
            ListColumn::Path,
            ListColumn::Base,
            ListColumn::Age,
            ListColumn::LastCommit,
        ]
    }

    /// Canonical display position (used to order columns in modifier mode).
    pub fn canonical_position(self) -> u8 {
        match self {
            Self::Annotation => 1,
            Self::Branch => 2,
            Self::Path => 3,
            Self::Size => 4,
            Self::Base => 5,
            Self::Changes => 6,
            Self::Remote => 7,
            Self::Age => 8,
            Self::Owner => 9,
            Self::Hash => 10,
            Self::LastCommit => 11,
        }
    }

    /// CLI-facing name for this column.
    pub fn cli_name(self) -> &'static str {
        match self {
            Self::Annotation => "annotation",
            Self::Branch => "branch",
            Self::Path => "path",
            Self::Size => "size",
            Self::Base => "base",
            Self::Remote => "remote",
            Self::Changes => "changes",
            Self::Age => "age",
            Self::Owner => "owner",
            Self::Hash => "hash",
            Self::LastCommit => "last-commit",
        }
    }

    /// All valid CLI column names, for use in error messages.
    pub fn valid_names() -> String {
        Self::all()
            .iter()
            .map(|c| c.cli_name())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl fmt::Display for ListColumn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.cli_name())
    }
}

impl FromStr for ListColumn {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "annotation" => Ok(Self::Annotation),
            "branch" => Ok(Self::Branch),
            "path" => Ok(Self::Path),
            "size" => Ok(Self::Size),
            "base" => Ok(Self::Base),
            "remote" => Ok(Self::Remote),
            "changes" => Ok(Self::Changes),
            "age" => Ok(Self::Age),
            "owner" => Ok(Self::Owner),
            "hash" => Ok(Self::Hash),
            "last-commit" => Ok(Self::LastCommit),
            _ => Err(format!(
                "unknown column '{}'\n  valid columns: {}",
                s.trim(),
                Self::valid_names()
            )),
        }
    }
}

/// Which command is requesting column selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    List,
    Sync,
    Prune,
    Clone,
}

/// The resolved column list with mode information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedColumns {
    pub columns: Vec<ListColumn>,
    /// True if the user explicitly chose columns (replace mode).
    /// False if they used modifier mode or if defaults are used.
    pub explicit: bool,
}

impl ResolvedColumns {
    pub fn defaults(defaults: &[ListColumn]) -> Self {
        Self {
            columns: defaults.to_vec(),
            explicit: false,
        }
    }
}

/// Parses and resolves a `--columns` value into a concrete column list.
pub struct ColumnSelection;

impl ColumnSelection {
    /// Parse a comma-separated column spec into a resolved column list.
    pub fn parse(input: &str, command: CommandKind) -> Result<ResolvedColumns, String> {
        let tokens: Vec<&str> = input.split(',').map(|s| s.trim()).collect();
        if tokens.is_empty() || tokens.iter().all(|t| t.is_empty()) {
            return Err("no columns specified".to_string());
        }

        let has_modifier = tokens
            .iter()
            .any(|t| t.starts_with('+') || t.starts_with('-'));
        let has_plain = tokens
            .iter()
            .any(|t| !t.starts_with('+') && !t.starts_with('-') && !t.is_empty());

        if has_modifier && has_plain {
            return Err("cannot mix column names with +/- modifiers\n  \
                 use either replace mode:   --columns branch,path,age\n  \
                 or modifier mode:          --columns -annotation,-remote"
                .to_string());
        }

        if has_modifier {
            let columns = Self::parse_modifier(&tokens, command)?;
            Ok(ResolvedColumns {
                columns,
                explicit: false,
            })
        } else {
            let columns = Self::parse_replace(&tokens, command)?;
            Ok(ResolvedColumns {
                columns,
                explicit: true,
            })
        }
    }

    fn parse_replace(tokens: &[&str], command: CommandKind) -> Result<Vec<ListColumn>, String> {
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for token in tokens {
            if token.is_empty() {
                continue;
            }
            Self::check_status_token(token, command)?;
            let col: ListColumn = token.parse()?;
            if !seen.insert(col) {
                return Err(format!("duplicate column '{}'", col.cli_name()));
            }
            result.push(col);
        }

        if result.is_empty() {
            return Err("no columns specified".to_string());
        }

        Ok(result)
    }

    fn parse_modifier(tokens: &[&str], command: CommandKind) -> Result<Vec<ListColumn>, String> {
        let defaults = match command {
            CommandKind::List => ListColumn::list_defaults(),
            CommandKind::Sync | CommandKind::Prune => ListColumn::tui_defaults(),
            CommandKind::Clone => ListColumn::clone_defaults(),
        };
        let mut active: std::collections::HashSet<ListColumn> = defaults.iter().copied().collect();

        for token in tokens {
            if token.is_empty() {
                continue;
            }
            let (prefix, name) = if let Some(rest) = token.strip_prefix('+') {
                ('+', rest)
            } else if let Some(rest) = token.strip_prefix('-') {
                ('-', rest)
            } else {
                return Err(format!("expected +/- prefix on '{token}'"));
            };

            Self::check_status_token(name, command)?;
            let col: ListColumn = name.parse()?;

            match prefix {
                '+' => {
                    active.insert(col);
                }
                '-' => {
                    active.remove(&col);
                }
                _ => unreachable!(),
            }
        }

        if active.is_empty() {
            let modifiers = tokens.join(",");
            return Err(format!(
                "no columns remaining after applying modifiers\n  modifiers: {modifiers}"
            ));
        }

        let mut result: Vec<ListColumn> = active.into_iter().collect();
        result.sort_by_key(|c| c.canonical_position());
        Ok(result)
    }

    fn check_status_token(name: &str, command: CommandKind) -> Result<(), String> {
        if name.trim().to_lowercase() == "status" {
            match command {
                CommandKind::List => {
                    // Falls through to unknown column error via FromStr
                }
                CommandKind::Sync | CommandKind::Prune | CommandKind::Clone => {
                    return Err("'status' column cannot be controlled on this command\n  \
                         it is always shown as the first column"
                        .to_string());
                }
            }
        }
        Ok(())
    }
}

/// A column that can appear in rich branch completions, controlled by
/// `daft.completions.branches.columns`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletionColumn {
    /// Relative age of the branch tip commit.
    Age,
    /// Author name of the branch tip commit.
    Author,
    /// Worktree path (only shown for worktree group entries).
    Path,
    /// Append `*` to branch name if the worktree has modified tracked files.
    TrackedChanges,
    /// Append `?` to branch name if the worktree has untracked files.
    Untracked,
}

impl CompletionColumn {
    /// All columns in canonical display order.
    pub fn all() -> &'static [CompletionColumn] {
        &[
            CompletionColumn::Age,
            CompletionColumn::Author,
            CompletionColumn::Path,
            CompletionColumn::TrackedChanges,
            CompletionColumn::Untracked,
        ]
    }

    /// The default column set (matches pre-config behavior).
    pub fn completion_defaults() -> &'static [CompletionColumn] {
        &[
            CompletionColumn::Age,
            CompletionColumn::Author,
            CompletionColumn::Path,
        ]
    }

    /// Canonical display position (used to order columns in modifier mode).
    pub fn canonical_position(self) -> u8 {
        match self {
            Self::Age => 1,
            Self::Author => 2,
            Self::Path => 3,
            Self::TrackedChanges => 4,
            Self::Untracked => 5,
        }
    }

    /// CLI-facing name for this column.
    pub fn cli_name(self) -> &'static str {
        match self {
            Self::Age => "age",
            Self::Author => "author",
            Self::Path => "path",
            Self::TrackedChanges => "tracked-changes",
            Self::Untracked => "untracked",
        }
    }

    /// All valid CLI column names, for use in error messages.
    pub fn valid_names() -> String {
        Self::all()
            .iter()
            .map(|c| c.cli_name())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl fmt::Display for CompletionColumn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.cli_name())
    }
}

impl FromStr for CompletionColumn {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "age" => Ok(Self::Age),
            "author" => Ok(Self::Author),
            "path" => Ok(Self::Path),
            "tracked-changes" => Ok(Self::TrackedChanges),
            "untracked" => Ok(Self::Untracked),
            _ => Err(format!(
                "unknown completion column '{}'\n  valid columns: {}",
                s.trim(),
                Self::valid_names()
            )),
        }
    }
}

/// Parser for `daft.completions.branches.columns` config values.
pub struct CompletionColumnSelection;

impl CompletionColumnSelection {
    /// Parse a comma-separated column spec into a resolved column list.
    /// Supports replace mode (`age,path`) and modifier mode (`+tracked-changes,-author`).
    pub fn parse(input: &str) -> Result<Vec<CompletionColumn>, String> {
        let tokens: Vec<&str> = input.split(',').map(|s| s.trim()).collect();
        if tokens.is_empty() || tokens.iter().all(|t| t.is_empty()) {
            return Err("no columns specified".to_string());
        }

        let has_modifier = tokens
            .iter()
            .any(|t| t.starts_with('+') || t.starts_with('-'));
        let has_plain = tokens
            .iter()
            .any(|t| !t.starts_with('+') && !t.starts_with('-') && !t.is_empty());

        if has_modifier && has_plain {
            return Err("cannot mix column names with +/- modifiers\n  \
                 use either replace mode:   age,path,tracked-changes\n  \
                 or modifier mode:          +tracked-changes,-author"
                .to_string());
        }

        if has_modifier {
            Self::parse_modifier(&tokens)
        } else {
            Self::parse_replace(&tokens)
        }
    }

    fn parse_replace(tokens: &[&str]) -> Result<Vec<CompletionColumn>, String> {
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for token in tokens {
            if token.is_empty() {
                continue;
            }
            let col: CompletionColumn = token.parse()?;
            if !seen.insert(col) {
                return Err(format!("duplicate column '{}'", col.cli_name()));
            }
            result.push(col);
        }

        if result.is_empty() {
            return Err("no columns specified".to_string());
        }

        Ok(result)
    }

    fn parse_modifier(tokens: &[&str]) -> Result<Vec<CompletionColumn>, String> {
        let defaults = CompletionColumn::completion_defaults();
        let mut active: std::collections::HashSet<CompletionColumn> =
            defaults.iter().copied().collect();

        for token in tokens {
            if token.is_empty() {
                continue;
            }
            let (prefix, name) = if let Some(rest) = token.strip_prefix('+') {
                ('+', rest)
            } else if let Some(rest) = token.strip_prefix('-') {
                ('-', rest)
            } else {
                return Err(format!("expected +/- prefix on '{token}'"));
            };

            let col: CompletionColumn = name.parse()?;

            match prefix {
                '+' => {
                    active.insert(col);
                }
                '-' => {
                    active.remove(&col);
                }
                _ => unreachable!(),
            }
        }

        if active.is_empty() {
            let modifiers = tokens.join(",");
            return Err(format!(
                "no columns remaining after applying modifiers\n  modifiers: {modifiers}"
            ));
        }

        let mut result: Vec<CompletionColumn> = active.into_iter().collect();
        result.sort_by_key(|c| c.canonical_position());
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_column_cli_names() {
        assert_eq!(
            "annotation".parse::<ListColumn>().unwrap(),
            ListColumn::Annotation
        );
        assert_eq!("branch".parse::<ListColumn>().unwrap(), ListColumn::Branch);
        assert_eq!("path".parse::<ListColumn>().unwrap(), ListColumn::Path);
        assert_eq!("size".parse::<ListColumn>().unwrap(), ListColumn::Size);
        assert_eq!("base".parse::<ListColumn>().unwrap(), ListColumn::Base);
        assert_eq!("remote".parse::<ListColumn>().unwrap(), ListColumn::Remote);
        assert_eq!(
            "changes".parse::<ListColumn>().unwrap(),
            ListColumn::Changes
        );
        assert_eq!("age".parse::<ListColumn>().unwrap(), ListColumn::Age);
        assert_eq!(
            "last-commit".parse::<ListColumn>().unwrap(),
            ListColumn::LastCommit
        );
    }

    #[test]
    fn test_list_column_display_roundtrip() {
        for col in ListColumn::all() {
            let name = col.cli_name();
            assert_eq!(name.parse::<ListColumn>().unwrap(), *col);
        }
    }

    #[test]
    fn test_list_column_canonical_position() {
        let all = ListColumn::all();
        for (i, col) in all.iter().enumerate() {
            assert!(
                i == 0 || col.canonical_position() > all[i - 1].canonical_position(),
                "Columns must be in ascending canonical position order"
            );
        }
    }

    #[test]
    fn test_unknown_column_parse_fails() {
        assert!("unknown".parse::<ListColumn>().is_err());
        assert!("status".parse::<ListColumn>().is_err());
        assert!("".parse::<ListColumn>().is_err());
    }

    #[test]
    fn test_replace_mode() {
        let resolved = ColumnSelection::parse("branch,path,age", CommandKind::List).unwrap();
        assert_eq!(
            resolved.columns,
            vec![ListColumn::Branch, ListColumn::Path, ListColumn::Age]
        );
        assert!(resolved.explicit);
    }

    #[test]
    fn test_replace_mode_custom_order() {
        let resolved = ColumnSelection::parse("age,branch,path", CommandKind::List).unwrap();
        assert_eq!(
            resolved.columns,
            vec![ListColumn::Age, ListColumn::Branch, ListColumn::Path]
        );
        assert!(resolved.explicit);
    }

    #[test]
    fn test_modifier_mode_subtract() {
        let resolved =
            ColumnSelection::parse("-annotation,-last-commit", CommandKind::List).unwrap();
        let expected: Vec<ListColumn> = ListColumn::list_defaults()
            .iter()
            .copied()
            .filter(|c| *c != ListColumn::Annotation && *c != ListColumn::LastCommit)
            .collect();
        assert_eq!(resolved.columns, expected);
        assert!(!resolved.explicit);
    }

    #[test]
    fn test_modifier_mode_canonical_order() {
        let r1 = ColumnSelection::parse("-last-commit,-annotation", CommandKind::List).unwrap();
        let r2 = ColumnSelection::parse("-annotation,-last-commit", CommandKind::List).unwrap();
        assert_eq!(r1.columns, r2.columns);
    }

    #[test]
    fn test_defaults_exclude_size() {
        assert!(!ListColumn::list_defaults().contains(&ListColumn::Size));
        assert!(!ListColumn::tui_defaults().contains(&ListColumn::Size));
        assert!(ListColumn::all().contains(&ListColumn::Size));
    }

    #[test]
    fn test_modifier_add_size() {
        let resolved = ColumnSelection::parse("+size", CommandKind::List).unwrap();
        assert!(resolved.columns.contains(&ListColumn::Size));
        // Size should appear after Path and before Base
        let size_pos = resolved
            .columns
            .iter()
            .position(|c| *c == ListColumn::Size)
            .unwrap();
        let path_pos = resolved
            .columns
            .iter()
            .position(|c| *c == ListColumn::Path)
            .unwrap();
        let base_pos = resolved
            .columns
            .iter()
            .position(|c| *c == ListColumn::Base)
            .unwrap();
        assert!(size_pos > path_pos);
        assert!(size_pos < base_pos);
    }

    #[test]
    fn test_modifier_add_idempotent() {
        let resolved = ColumnSelection::parse("+branch,+path", CommandKind::List).unwrap();
        assert_eq!(resolved.columns, ListColumn::list_defaults().to_vec());
    }

    #[test]
    fn test_modifier_subtract_nondefault_idempotent() {
        let resolved = ColumnSelection::parse("-annotation", CommandKind::List).unwrap();
        assert!(!resolved.columns.contains(&ListColumn::Annotation));
    }

    #[test]
    fn test_mixed_mode_error() {
        let err = ColumnSelection::parse("branch,+age", CommandKind::List).unwrap_err();
        assert!(err.contains("cannot mix"), "Got: {err}");
    }

    #[test]
    fn test_empty_result_error() {
        let all_minus: String = ListColumn::all()
            .iter()
            .map(|c| format!("-{}", c.cli_name()))
            .collect::<Vec<_>>()
            .join(",");
        let err = ColumnSelection::parse(&all_minus, CommandKind::List).unwrap_err();
        assert!(err.contains("no columns remaining"), "Got: {err}");
    }

    #[test]
    fn test_unknown_column_error() {
        let err = ColumnSelection::parse("branch,foo", CommandKind::List).unwrap_err();
        assert!(err.contains("unknown column 'foo'"), "Got: {err}");
    }

    #[test]
    fn test_duplicate_replace_error() {
        let err = ColumnSelection::parse("branch,path,branch", CommandKind::List).unwrap_err();
        assert!(err.contains("duplicate"), "Got: {err}");
    }

    #[test]
    fn test_duplicate_modifier_idempotent() {
        let resolved = ColumnSelection::parse("+branch,+branch", CommandKind::List).unwrap();
        assert_eq!(resolved.columns, ListColumn::list_defaults().to_vec());
    }

    #[test]
    fn test_whitespace_trimmed() {
        let resolved = ColumnSelection::parse("branch , path , age", CommandKind::List).unwrap();
        assert_eq!(
            resolved.columns,
            vec![ListColumn::Branch, ListColumn::Path, ListColumn::Age]
        );
    }

    #[test]
    fn test_status_on_sync_errors() {
        let err = ColumnSelection::parse("status,branch", CommandKind::Sync).unwrap_err();
        assert!(err.contains("cannot be controlled"), "Got: {err}");
    }

    #[test]
    fn test_status_modifier_on_prune_errors() {
        let err = ColumnSelection::parse("+status", CommandKind::Prune).unwrap_err();
        assert!(err.contains("cannot be controlled"), "Got: {err}");
    }

    #[test]
    fn test_status_on_list_unknown() {
        let err = ColumnSelection::parse("status,branch", CommandKind::List).unwrap_err();
        assert!(err.contains("unknown column 'status'"), "Got: {err}");
    }

    #[test]
    fn test_defaults_exclude_hash() {
        assert!(!ListColumn::list_defaults().contains(&ListColumn::Hash));
        assert!(!ListColumn::tui_defaults().contains(&ListColumn::Hash));
        assert!(ListColumn::all().contains(&ListColumn::Hash));
    }

    #[test]
    fn test_modifier_add_hash() {
        let resolved = ColumnSelection::parse("+hash", CommandKind::List).unwrap();
        assert!(resolved.columns.contains(&ListColumn::Hash));
        // Hash should appear after Owner and before LastCommit
        let hash_pos = resolved
            .columns
            .iter()
            .position(|c| *c == ListColumn::Hash)
            .unwrap();
        let owner_pos = resolved
            .columns
            .iter()
            .position(|c| *c == ListColumn::Owner)
            .unwrap();
        let last_commit_pos = resolved
            .columns
            .iter()
            .position(|c| *c == ListColumn::LastCommit)
            .unwrap();
        assert!(hash_pos > owner_pos);
        assert!(hash_pos < last_commit_pos);
    }

    #[test]
    fn test_hash_cli_name_roundtrip() {
        let col: ListColumn = "hash".parse().unwrap();
        assert_eq!(col, ListColumn::Hash);
        assert_eq!(col.cli_name(), "hash");
    }

    // CompletionColumn tests

    #[test]
    fn completion_column_defaults() {
        assert_eq!(
            CompletionColumn::completion_defaults(),
            &[
                CompletionColumn::Age,
                CompletionColumn::Author,
                CompletionColumn::Path
            ]
        );
    }

    #[test]
    fn completion_column_parse_replace() {
        let cols = CompletionColumnSelection::parse("age,path").unwrap();
        assert_eq!(cols, vec![CompletionColumn::Age, CompletionColumn::Path]);
    }

    #[test]
    fn completion_column_parse_replace_with_status() {
        let cols =
            CompletionColumnSelection::parse("age,author,path,tracked-changes,untracked").unwrap();
        assert_eq!(
            cols,
            vec![
                CompletionColumn::Age,
                CompletionColumn::Author,
                CompletionColumn::Path,
                CompletionColumn::TrackedChanges,
                CompletionColumn::Untracked,
            ]
        );
    }

    #[test]
    fn completion_column_parse_modifier_add() {
        let cols = CompletionColumnSelection::parse("+tracked-changes").unwrap();
        assert!(cols.contains(&CompletionColumn::TrackedChanges));
        // Defaults are preserved
        assert!(cols.contains(&CompletionColumn::Age));
        assert!(cols.contains(&CompletionColumn::Author));
        assert!(cols.contains(&CompletionColumn::Path));
    }

    #[test]
    fn completion_column_parse_modifier_remove() {
        let cols = CompletionColumnSelection::parse("-author").unwrap();
        assert!(!cols.contains(&CompletionColumn::Author));
        assert!(cols.contains(&CompletionColumn::Age));
        assert!(cols.contains(&CompletionColumn::Path));
    }

    #[test]
    fn completion_column_parse_unknown() {
        let err = CompletionColumnSelection::parse("bogus").unwrap_err();
        assert!(err.contains("unknown completion column"));
    }

    #[test]
    fn completion_column_parse_mixed_mode_error() {
        let err = CompletionColumnSelection::parse("age,+tracked-changes").unwrap_err();
        assert!(err.contains("cannot mix"));
    }

    #[test]
    fn completion_column_parse_duplicate_error() {
        let err = CompletionColumnSelection::parse("age,age").unwrap_err();
        assert!(err.contains("duplicate"));
    }
}
