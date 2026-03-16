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
    /// Ahead/behind base branch.
    Base,
    /// Local changes (staged/unstaged/untracked).
    Changes,
    /// Ahead/behind remote tracking branch.
    Remote,
    /// Branch age since creation.
    Age,
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
            ListColumn::Base,
            ListColumn::Changes,
            ListColumn::Remote,
            ListColumn::Age,
            ListColumn::LastCommit,
        ]
    }

    /// The default column set for the list command.
    pub fn list_defaults() -> &'static [ListColumn] {
        Self::all()
    }

    /// The default column set for sync and prune commands.
    /// (Status is pinned separately by TUI code, not included here.)
    pub fn tui_defaults() -> &'static [ListColumn] {
        Self::all()
    }

    /// Canonical display position (used to order columns in modifier mode).
    pub fn canonical_position(self) -> u8 {
        match self {
            Self::Annotation => 1,
            Self::Branch => 2,
            Self::Path => 3,
            Self::Base => 4,
            Self::Changes => 5,
            Self::Remote => 6,
            Self::Age => 7,
            Self::LastCommit => 8,
        }
    }

    /// CLI-facing name for this column.
    pub fn cli_name(self) -> &'static str {
        match self {
            Self::Annotation => "annotation",
            Self::Branch => "branch",
            Self::Path => "path",
            Self::Base => "base",
            Self::Remote => "remote",
            Self::Changes => "changes",
            Self::Age => "age",
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
            "base" => Ok(Self::Base),
            "remote" => Ok(Self::Remote),
            "changes" => Ok(Self::Changes),
            "age" => Ok(Self::Age),
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
                CommandKind::Sync | CommandKind::Prune => {
                    return Err("'status' column cannot be controlled on this command\n  \
                         it is always shown as the first column"
                        .to_string());
                }
            }
        }
        Ok(())
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
}
