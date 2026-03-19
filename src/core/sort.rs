//! Sort specification for the `--sort` flag on list/sync/prune commands.
//!
//! Defines sortable columns, direction, multi-key sort specs, and the parser.
//! Separate from the display `ListColumn` enum because sorting supports
//! aliases (e.g., `activity`/`commit`) and excludes composite display columns
//! (annotation, base, changes, remote).

use crate::core::columns::ListColumn;
use crate::core::worktree::list::WorktreeInfo;
use std::cmp::Ordering;

/// A column that can be used in a `--sort` specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SortColumn {
    /// Sort by branch name (case-insensitive).
    Branch,
    /// Sort by worktree path.
    Path,
    /// Sort by disk size.
    Size,
    /// Sort by branch creation age.
    Age,
    /// Sort by branch owner email (case-insensitive).
    Owner,
    /// Sort by overall activity: `max(last_commit_timestamp, working_tree_mtime)`.
    /// Considers both committed and uncommitted work.
    Activity,
    /// Sort by last commit timestamp only (ignores uncommitted changes).
    /// Aliases: `commit`, `last-commit`.
    LastCommit,
}

impl SortColumn {
    /// All valid CLI sort column names, for use in error messages.
    pub fn valid_names() -> &'static str {
        "branch, path, size, age, owner, activity, commit (alias: last-commit)"
    }

    /// Map this sort column to the corresponding display column, if any.
    ///
    /// Returns `None` for `Activity` since it's a composite signal (commit +
    /// working tree mtime) that doesn't correspond to a single display column.
    pub fn to_list_column(self) -> Option<ListColumn> {
        match self {
            Self::Branch => Some(ListColumn::Branch),
            Self::Path => Some(ListColumn::Path),
            Self::Size => Some(ListColumn::Size),
            Self::Age => Some(ListColumn::Age),
            Self::Owner => Some(ListColumn::Owner),
            Self::LastCommit => Some(ListColumn::LastCommit),
            Self::Activity => None,
        }
    }

    /// Parse a sort column name (case-insensitive).
    fn parse(name: &str) -> Result<Self, String> {
        match name.trim().to_lowercase().as_str() {
            "branch" => Ok(Self::Branch),
            "path" => Ok(Self::Path),
            "size" => Ok(Self::Size),
            "age" => Ok(Self::Age),
            "owner" => Ok(Self::Owner),
            "activity" => Ok(Self::Activity),
            "commit" | "last-commit" => Ok(Self::LastCommit),
            _ => Err(format!(
                "unknown sort column '{}'\n  sortable columns: {}",
                name.trim(),
                Self::valid_names()
            )),
        }
    }
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

/// A single sort criterion: column + direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortKey {
    pub column: SortColumn,
    pub direction: SortDirection,
}

impl SortKey {
    /// Compare two `WorktreeInfo` entries by this key's column and direction.
    ///
    /// `None` values always sort last, regardless of sort direction.
    fn compare(&self, a: &WorktreeInfo, b: &WorktreeInfo) -> Ordering {
        match self.column {
            SortColumn::Branch => {
                // Branch name is always present, just apply direction directly.
                let ord = a.name.to_lowercase().cmp(&b.name.to_lowercase());
                self.apply_direction(ord)
            }
            SortColumn::Path => self.compare_optional(a.path.as_deref(), b.path.as_deref()),
            SortColumn::Size => self.compare_optional(a.size_bytes, b.size_bytes),
            SortColumn::Age => {
                // "Ascending age" = youngest first (smallest displayed age at
                // top, i.e., most recently created = largest timestamp first).
                self.compare_optional_reversed(
                    a.branch_creation_timestamp,
                    b.branch_creation_timestamp,
                )
            }
            SortColumn::Owner => {
                let a_owner = a.owner_email.as_deref().map(|s| s.to_lowercase());
                let b_owner = b.owner_email.as_deref().map(|s| s.to_lowercase());
                self.compare_optional(a_owner.as_deref(), b_owner.as_deref())
            }
            SortColumn::Activity => {
                // Overall activity = max(last_commit, working_tree_mtime).
                // "Ascending activity" = most active first.
                let a_ts = max_optional(a.last_commit_timestamp, a.working_tree_mtime);
                let b_ts = max_optional(b.last_commit_timestamp, b.working_tree_mtime);
                self.compare_optional_reversed(a_ts, b_ts)
            }
            SortColumn::LastCommit => {
                // Pure git: last commit timestamp only.
                // "Ascending" = most recent commit first.
                self.compare_optional_reversed(a.last_commit_timestamp, b.last_commit_timestamp)
            }
        }
    }

    /// Compare two optional values, with `None` always sorting last.
    ///
    /// Direction is applied only to the `Some`-vs-`Some` comparison.
    fn compare_optional<T: Ord>(&self, a: Option<T>, b: Option<T>) -> Ordering {
        match (a, b) {
            (Some(a), Some(b)) => self.apply_direction(a.cmp(&b)),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    }

    /// Like `compare_optional` but with the natural ordering reversed.
    ///
    /// Ascending = largest value first. Used for columns where the semantic
    /// meaning of "ascending" is the opposite of the raw value ordering
    /// (e.g., "ascending activity" = most recent first = largest timestamp).
    fn compare_optional_reversed<T: Ord>(&self, a: Option<T>, b: Option<T>) -> Ordering {
        match (a, b) {
            (Some(a), Some(b)) => self.apply_direction(b.cmp(&a)),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    }

    fn apply_direction(&self, ord: Ordering) -> Ordering {
        match self.direction {
            SortDirection::Ascending => ord,
            SortDirection::Descending => ord.reverse(),
        }
    }
}

/// Return the maximum of two optional values, or whichever is `Some`.
fn max_optional(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// A complete sort specification (one or more sort keys in priority order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortSpec {
    pub keys: Vec<SortKey>,
}

impl SortSpec {
    /// The default sort: branch name ascending.
    pub fn default_sort() -> Self {
        Self {
            keys: vec![SortKey {
                column: SortColumn::Branch,
                direction: SortDirection::Ascending,
            }],
        }
    }

    /// Parse a comma-separated sort specification string.
    ///
    /// Each token is optionally prefixed with `+` (ascending) or `-` (descending).
    /// No prefix defaults to ascending.
    ///
    /// # Examples
    ///
    /// ```text
    /// "branch"            → branch ascending
    /// "+branch,-size"     → branch ascending, then size descending
    /// "-activity"         → last commit timestamp descending (most recent first)
    /// "commit"            → alias for activity ascending
    /// ```
    pub fn parse(input: &str) -> Result<Self, String> {
        let tokens: Vec<&str> = input.split(',').map(|s| s.trim()).collect();
        if tokens.is_empty() || tokens.iter().all(|t| t.is_empty()) {
            return Err("no sort columns specified".to_string());
        }

        let mut keys = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for token in &tokens {
            if token.is_empty() {
                continue;
            }

            let (direction, name) = if let Some(rest) = token.strip_prefix('+') {
                (SortDirection::Ascending, rest)
            } else if let Some(rest) = token.strip_prefix('-') {
                (SortDirection::Descending, rest)
            } else {
                (SortDirection::Ascending, *token)
            };

            if name.trim().is_empty() {
                return Err(format!("empty column name in sort spec '{token}'"));
            }

            let column = SortColumn::parse(name)?;

            if !seen.insert(column) {
                return Err(format!("duplicate sort column '{}'", name.trim()));
            }

            keys.push(SortKey { column, direction });
        }

        if keys.is_empty() {
            return Err("no sort columns specified".to_string());
        }

        Ok(Self { keys })
    }

    /// Return the sort direction indicator for a display column, if that
    /// column is part of this sort spec.
    ///
    /// Arrows follow terminal visual flow: ascending (smallest first) flows
    /// downward `↓`, descending (largest first) flows upward `↑`.
    ///
    /// Returns `Some("↓")` for ascending, `Some("↑")` for descending,
    /// or `None` if the column is not being sorted.
    pub fn direction_indicator(&self, col: ListColumn) -> Option<&'static str> {
        self.keys.iter().find_map(|key| {
            if key.column.to_list_column() == Some(col) {
                Some(match key.direction {
                    SortDirection::Ascending => "\u{2193}",  // ↓
                    SortDirection::Descending => "\u{2191}", // ↑
                })
            } else {
                None
            }
        })
    }

    /// Whether this sort spec requires size data to be collected.
    pub fn needs_size(&self) -> bool {
        self.keys.iter().any(|k| k.column == SortColumn::Size)
    }

    /// Whether this sort spec requires working tree mtime to be collected.
    pub fn needs_mtime(&self) -> bool {
        self.keys.iter().any(|k| k.column == SortColumn::Activity)
    }

    /// Sort a slice of `WorktreeInfo` in place according to this specification.
    pub fn sort(&self, infos: &mut [WorktreeInfo]) {
        infos.sort_by(|a, b| self.compare(a, b));
    }

    /// Compare two `WorktreeInfo` entries according to this specification.
    ///
    /// Useful for callers that need to wrap this comparison with additional
    /// criteria (e.g., TUI default-branch pinning, kind grouping).
    pub fn compare(&self, a: &WorktreeInfo, b: &WorktreeInfo) -> Ordering {
        for key in &self.keys {
            let ord = key.compare(a, b);
            if ord != Ordering::Equal {
                return ord;
            }
        }
        Ordering::Equal
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Create a minimal `WorktreeInfo` for testing with just name.
    fn info(name: &str) -> WorktreeInfo {
        WorktreeInfo::empty(name)
    }

    /// Create a `WorktreeInfo` with name and size.
    fn info_with_size(name: &str, size: Option<u64>) -> WorktreeInfo {
        let mut i = WorktreeInfo::empty(name);
        i.size_bytes = size;
        i
    }

    /// Create a `WorktreeInfo` with name and last commit timestamp.
    fn info_with_activity(name: &str, ts: Option<i64>) -> WorktreeInfo {
        let mut i = WorktreeInfo::empty(name);
        i.last_commit_timestamp = ts;
        i
    }

    /// Create a `WorktreeInfo` with name and owner email.
    fn info_with_owner(name: &str, owner: Option<&str>) -> WorktreeInfo {
        let mut i = WorktreeInfo::empty(name);
        i.owner_email = owner.map(|s| s.to_string());
        i
    }

    /// Create a `WorktreeInfo` with name and branch creation timestamp.
    fn info_with_age(name: &str, ts: Option<i64>) -> WorktreeInfo {
        let mut i = WorktreeInfo::empty(name);
        i.branch_creation_timestamp = ts;
        i
    }

    /// Create a `WorktreeInfo` with name and path.
    fn info_with_path(name: &str, path: Option<&str>) -> WorktreeInfo {
        let mut i = WorktreeInfo::empty(name);
        i.path = path.map(PathBuf::from);
        i
    }

    // ── Parsing tests ──────────────────────────────────────────────────

    #[test]
    fn test_parse_single_column_no_prefix() {
        let spec = SortSpec::parse("branch").unwrap();
        assert_eq!(spec.keys.len(), 1);
        assert_eq!(spec.keys[0].column, SortColumn::Branch);
        assert_eq!(spec.keys[0].direction, SortDirection::Ascending);
    }

    #[test]
    fn test_parse_ascending_prefix() {
        let spec = SortSpec::parse("+branch").unwrap();
        assert_eq!(spec.keys[0].direction, SortDirection::Ascending);
    }

    #[test]
    fn test_parse_descending_prefix() {
        let spec = SortSpec::parse("-branch").unwrap();
        assert_eq!(spec.keys[0].direction, SortDirection::Descending);
    }

    #[test]
    fn test_parse_multiple_keys() {
        let spec = SortSpec::parse("+owner,-size").unwrap();
        assert_eq!(spec.keys.len(), 2);
        assert_eq!(spec.keys[0].column, SortColumn::Owner);
        assert_eq!(spec.keys[0].direction, SortDirection::Ascending);
        assert_eq!(spec.keys[1].column, SortColumn::Size);
        assert_eq!(spec.keys[1].direction, SortDirection::Descending);
    }

    #[test]
    fn test_parse_activity_and_last_commit() {
        let spec = SortSpec::parse("activity").unwrap();
        assert_eq!(spec.keys[0].column, SortColumn::Activity);

        for name in &["commit", "last-commit"] {
            let spec = SortSpec::parse(name).unwrap();
            assert_eq!(
                spec.keys[0].column,
                SortColumn::LastCommit,
                "'{name}' should map to LastCommit"
            );
        }
    }

    #[test]
    fn test_activity_and_last_commit_are_distinct() {
        // Both can coexist in a sort spec since they're different columns
        let spec = SortSpec::parse("activity,commit").unwrap();
        assert_eq!(spec.keys[0].column, SortColumn::Activity);
        assert_eq!(spec.keys[1].column, SortColumn::LastCommit);
    }

    #[test]
    fn test_parse_case_insensitive() {
        let spec = SortSpec::parse("Branch").unwrap();
        assert_eq!(spec.keys[0].column, SortColumn::Branch);
        let spec = SortSpec::parse("SIZE").unwrap();
        assert_eq!(spec.keys[0].column, SortColumn::Size);
    }

    #[test]
    fn test_parse_whitespace_trimmed() {
        let spec = SortSpec::parse(" branch , -size ").unwrap();
        assert_eq!(spec.keys.len(), 2);
        assert_eq!(spec.keys[0].column, SortColumn::Branch);
        assert_eq!(spec.keys[1].column, SortColumn::Size);
    }

    #[test]
    fn test_parse_unknown_column_error() {
        let err = SortSpec::parse("foo").unwrap_err();
        assert!(err.contains("unknown sort column 'foo'"), "Got: {err}");
        assert!(err.contains("sortable columns"), "Got: {err}");
    }

    #[test]
    fn test_parse_non_sortable_column_error() {
        for name in &["annotation", "base", "changes", "remote"] {
            let err = SortSpec::parse(name).unwrap_err();
            assert!(
                err.contains("unknown sort column"),
                "'{name}' should not be sortable, got: {err}"
            );
        }
    }

    #[test]
    fn test_parse_duplicate_column_error() {
        let err = SortSpec::parse("branch,-branch").unwrap_err();
        assert!(err.contains("duplicate"), "Got: {err}");
    }

    #[test]
    fn test_parse_duplicate_alias_error() {
        let err = SortSpec::parse("commit,-last-commit").unwrap_err();
        assert!(err.contains("duplicate"), "Got: {err}");
    }

    #[test]
    fn test_parse_empty_input_error() {
        assert!(SortSpec::parse("").is_err());
        assert!(SortSpec::parse("  ").is_err());
        assert!(SortSpec::parse(",").is_err());
    }

    #[test]
    fn test_parse_bare_direction_prefix_error() {
        let err = SortSpec::parse("+").unwrap_err();
        assert!(err.contains("empty column name"), "Got: {err}");
        let err = SortSpec::parse("-").unwrap_err();
        assert!(err.contains("empty column name"), "Got: {err}");
    }

    // ── Default sort ───────────────────────────────────────────────────

    #[test]
    fn test_default_sort() {
        let spec = SortSpec::default_sort();
        assert_eq!(spec.keys.len(), 1);
        assert_eq!(spec.keys[0].column, SortColumn::Branch);
        assert_eq!(spec.keys[0].direction, SortDirection::Ascending);
    }

    // ── needs_size ─────────────────────────────────────────────────────

    #[test]
    fn test_needs_size_true() {
        assert!(SortSpec::parse("size").unwrap().needs_size());
        assert!(SortSpec::parse("branch,-size").unwrap().needs_size());
    }

    #[test]
    fn test_needs_size_false() {
        assert!(!SortSpec::parse("branch").unwrap().needs_size());
        assert!(!SortSpec::parse("activity").unwrap().needs_size());
    }

    #[test]
    fn test_needs_mtime_true() {
        assert!(SortSpec::parse("activity").unwrap().needs_mtime());
        assert!(SortSpec::parse("branch,activity").unwrap().needs_mtime());
    }

    #[test]
    fn test_needs_mtime_false() {
        assert!(!SortSpec::parse("branch").unwrap().needs_mtime());
        assert!(!SortSpec::parse("commit").unwrap().needs_mtime());
        assert!(!SortSpec::parse("last-commit").unwrap().needs_mtime());
    }

    // ── Sorting tests ──────────────────────────────────────────────────

    #[test]
    fn test_sort_by_branch_ascending() {
        let spec = SortSpec::parse("branch").unwrap();
        let mut infos = vec![info("charlie"), info("alpha"), info("bravo")];
        spec.sort(&mut infos);
        let names: Vec<&str> = infos.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "bravo", "charlie"]);
    }

    #[test]
    fn test_sort_by_branch_descending() {
        let spec = SortSpec::parse("-branch").unwrap();
        let mut infos = vec![info("charlie"), info("alpha"), info("bravo")];
        spec.sort(&mut infos);
        let names: Vec<&str> = infos.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["charlie", "bravo", "alpha"]);
    }

    #[test]
    fn test_sort_by_branch_case_insensitive() {
        let spec = SortSpec::parse("branch").unwrap();
        let mut infos = vec![info("Charlie"), info("alpha"), info("Bravo")];
        spec.sort(&mut infos);
        let names: Vec<&str> = infos.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "Bravo", "Charlie"]);
    }

    #[test]
    fn test_sort_by_size_ascending() {
        let spec = SortSpec::parse("size").unwrap();
        let mut infos = vec![
            info_with_size("a", Some(300)),
            info_with_size("b", Some(100)),
            info_with_size("c", Some(200)),
        ];
        spec.sort(&mut infos);
        let sizes: Vec<Option<u64>> = infos.iter().map(|i| i.size_bytes).collect();
        assert_eq!(sizes, vec![Some(100), Some(200), Some(300)]);
    }

    #[test]
    fn test_sort_by_size_descending() {
        let spec = SortSpec::parse("-size").unwrap();
        let mut infos = vec![
            info_with_size("a", Some(300)),
            info_with_size("b", Some(100)),
            info_with_size("c", Some(200)),
        ];
        spec.sort(&mut infos);
        let sizes: Vec<Option<u64>> = infos.iter().map(|i| i.size_bytes).collect();
        assert_eq!(sizes, vec![Some(300), Some(200), Some(100)]);
    }

    #[test]
    fn test_sort_none_values_last_ascending() {
        let spec = SortSpec::parse("size").unwrap();
        let mut infos = vec![
            info_with_size("a", None),
            info_with_size("b", Some(100)),
            info_with_size("c", Some(200)),
        ];
        spec.sort(&mut infos);
        let names: Vec<&str> = infos.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["b", "c", "a"]);
    }

    #[test]
    fn test_sort_none_values_last_descending() {
        let spec = SortSpec::parse("-size").unwrap();
        let mut infos = vec![
            info_with_size("a", None),
            info_with_size("b", Some(100)),
            info_with_size("c", Some(200)),
        ];
        spec.sort(&mut infos);
        let names: Vec<&str> = infos.iter().map(|i| i.name.as_str()).collect();
        // Descending: 200, 100, then None last
        assert_eq!(names, vec!["c", "b", "a"]);
    }

    #[test]
    fn test_sort_by_activity_ascending() {
        // Ascending activity = most active (most recent) first
        let spec = SortSpec::parse("activity").unwrap();
        let mut infos = vec![
            info_with_activity("old", Some(1000)),
            info_with_activity("newest", Some(3000)),
            info_with_activity("middle", Some(2000)),
        ];
        spec.sort(&mut infos);
        let names: Vec<&str> = infos.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["newest", "middle", "old"]);
    }

    #[test]
    fn test_sort_by_activity_descending() {
        // Descending activity = least active (oldest) first
        let spec = SortSpec::parse("-activity").unwrap();
        let mut infos = vec![
            info_with_activity("old", Some(1000)),
            info_with_activity("newest", Some(3000)),
            info_with_activity("middle", Some(2000)),
        ];
        spec.sort(&mut infos);
        let names: Vec<&str> = infos.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["old", "middle", "newest"]);
    }

    #[test]
    fn test_sort_by_activity_considers_mtime() {
        // "stale-commit" has an old commit but recent uncommitted changes (high mtime).
        // "fresh-commit" has a recent commit but no uncommitted changes.
        // Activity ascending = most active first, so stale-commit wins due to mtime.
        let spec = SortSpec::parse("activity").unwrap();
        let mut stale_commit = info_with_activity("stale-commit", Some(1000));
        stale_commit.working_tree_mtime = Some(5000); // recent file edits
        let fresh_commit = info_with_activity("fresh-commit", Some(4000));
        // fresh-commit has no mtime (clean worktree)

        let mut infos = vec![fresh_commit, stale_commit];
        spec.sort(&mut infos);
        let names: Vec<&str> = infos.iter().map(|i| i.name.as_str()).collect();
        // stale-commit has max(1000, 5000) = 5000 > 4000, so it's more active
        assert_eq!(names, vec!["stale-commit", "fresh-commit"]);
    }

    #[test]
    fn test_sort_by_last_commit_ignores_mtime() {
        // Same data as above but using "commit" sort — should ignore mtime.
        let spec = SortSpec::parse("commit").unwrap();
        let mut stale_commit = info_with_activity("stale-commit", Some(1000));
        stale_commit.working_tree_mtime = Some(5000);
        let fresh_commit = info_with_activity("fresh-commit", Some(4000));

        let mut infos = vec![fresh_commit, stale_commit];
        spec.sort(&mut infos);
        let names: Vec<&str> = infos.iter().map(|i| i.name.as_str()).collect();
        // commit only: 4000 > 1000, so fresh-commit is more active
        assert_eq!(names, vec!["fresh-commit", "stale-commit"]);
    }

    #[test]
    fn test_sort_by_owner() {
        let spec = SortSpec::parse("owner").unwrap();
        let mut infos = vec![
            info_with_owner("a", Some("charlie@test.com")),
            info_with_owner("b", Some("alpha@test.com")),
            info_with_owner("c", Some("bravo@test.com")),
        ];
        spec.sort(&mut infos);
        let owners: Vec<&str> = infos
            .iter()
            .map(|i| i.owner_email.as_deref().unwrap())
            .collect();
        assert_eq!(
            owners,
            vec!["alpha@test.com", "bravo@test.com", "charlie@test.com"]
        );
    }

    #[test]
    fn test_sort_by_age_ascending() {
        // Ascending age = youngest first (smallest displayed age = most recent creation)
        let spec = SortSpec::parse("age").unwrap();
        let mut infos = vec![
            info_with_age("newest", Some(3000)),
            info_with_age("oldest", Some(1000)),
            info_with_age("middle", Some(2000)),
        ];
        spec.sort(&mut infos);
        let names: Vec<&str> = infos.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["newest", "middle", "oldest"]);
    }

    #[test]
    fn test_sort_by_age_descending() {
        // Descending age = oldest first (largest displayed age)
        let spec = SortSpec::parse("-age").unwrap();
        let mut infos = vec![
            info_with_age("newest", Some(3000)),
            info_with_age("oldest", Some(1000)),
            info_with_age("middle", Some(2000)),
        ];
        spec.sort(&mut infos);
        let names: Vec<&str> = infos.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["oldest", "middle", "newest"]);
    }

    #[test]
    fn test_sort_by_path() {
        let spec = SortSpec::parse("path").unwrap();
        let mut infos = vec![
            info_with_path("a", Some("/z/worktree")),
            info_with_path("b", Some("/a/worktree")),
            info_with_path("c", Some("/m/worktree")),
        ];
        spec.sort(&mut infos);
        let paths: Vec<&str> = infos
            .iter()
            .map(|i| i.path.as_ref().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(paths, vec!["/a/worktree", "/m/worktree", "/z/worktree"]);
    }

    #[test]
    fn test_multi_key_sort() {
        let spec = SortSpec::parse("+owner,-size").unwrap();
        let mut infos = vec![
            {
                let mut i = info_with_owner("a", Some("bob@test.com"));
                i.size_bytes = Some(200);
                i
            },
            {
                let mut i = info_with_owner("b", Some("alice@test.com"));
                i.size_bytes = Some(100);
                i
            },
            {
                let mut i = info_with_owner("c", Some("alice@test.com"));
                i.size_bytes = Some(300);
                i
            },
            {
                let mut i = info_with_owner("d", Some("bob@test.com"));
                i.size_bytes = Some(400);
                i
            },
        ];
        spec.sort(&mut infos);
        let names: Vec<&str> = infos.iter().map(|i| i.name.as_str()).collect();
        // alice group (size desc: 300, 100), then bob group (size desc: 400, 200)
        assert_eq!(names, vec!["c", "b", "d", "a"]);
    }

    #[test]
    fn test_compare_method() {
        let spec = SortSpec::parse("-branch").unwrap();
        let a = info("alpha");
        let b = info("bravo");
        assert_eq!(spec.compare(&a, &b), Ordering::Greater);
        assert_eq!(spec.compare(&b, &a), Ordering::Less);
        assert_eq!(spec.compare(&a, &a), Ordering::Equal);
    }
}
