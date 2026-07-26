//! Changed-files engine for hook job filtering.
//!
//! One neutral engine shared by every hook type: an operation-scoped list of
//! changed files (resolved lazily, at most once per hook fire, by a
//! [`ChangedFilesProvider`]) filtered per job by compiled glob patterns
//! ([`FileFilter`]). A job whose filtered list comes out empty is skipped as
//! a first-class outcome — patterns select *work*, not just command
//! arguments — so a docs-only change set skips the build ring instead of
//! running it against nothing.
//!
//! Matching semantics (the standard "doublestar" glob dialect, deliberately):
//! - Patterns match **repository-root-relative paths**, never basenames. A
//!   job's `root:` shifts where its command runs, not what its patterns see.
//! - `*` and `?` do not cross `/`; `**` spans zero or more directories
//!   (`**/*.rs` matches `lib.rs` and `src/lib.rs` alike).
//! - Brace alternation is supported (`*.{js,ts}`).
//! - Matching is case-sensitive.

pub mod provider;
pub use provider::{ChangedFilesProvider, run_files_command};

use anyhow::{Context, Result};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

/// Template placeholder in a job's `run:` command that expands to the job's
/// filtered changed-file list, shell-quoted and space-joined. Substituted by
/// the job adapter (not [`crate::hooks::template::substitute`]) because the
/// expansion is per-job, after glob filtering.
pub const CHANGED_FILES_TEMPLATE: &str = "{changed_files}";

/// A compiled include/exclude glob filter over a changed-file list.
///
/// Built per job from its `glob:` patterns plus its `exclude:` patterns
/// (job-level and hook-level combined). An empty include set selects every
/// file — exclude-only filtering; an empty exclude set removes nothing.
#[derive(Debug)]
pub struct FileFilter {
    include: Option<GlobSet>,
    exclude: Option<GlobSet>,
    include_patterns: Vec<String>,
    exclude_patterns: Vec<String>,
}

impl FileFilter {
    /// Compile include and exclude pattern lists. A malformed pattern is a
    /// configuration error naming the pattern.
    pub fn new(include: &[String], exclude: &[String]) -> Result<Self> {
        Ok(Self {
            include: compile(include)?,
            exclude: compile(exclude)?,
            include_patterns: include.to_vec(),
            exclude_patterns: exclude.to_vec(),
        })
    }

    /// Whether `path` survives the filter. Exclude wins over include; with no
    /// include patterns every non-excluded path matches. A leading `./` is
    /// stripped so `files:` commands emitting `./src/x.rs` still match.
    pub fn matches(&self, path: &str) -> bool {
        let path = path.strip_prefix("./").unwrap_or(path);
        if let Some(ref exclude) = self.exclude
            && exclude.is_match(path)
        {
            return false;
        }
        match self.include {
            Some(ref include) => include.is_match(path),
            None => true,
        }
    }

    /// The subset of `files` that survives the filter, order preserved.
    pub fn filter(&self, files: &[String]) -> Vec<String> {
        files.iter().filter(|f| self.matches(f)).cloned().collect()
    }

    /// Skip-reason text for a job whose filtered list came out empty.
    /// Callers with no patterns at all (bare `files:` command, bare
    /// `{changed_files}` template) phrase their own reason — this covers the
    /// pattern-bearing cases.
    pub fn empty_reason(&self) -> String {
        let include = self.include_patterns.join(", ");
        let exclude = self.exclude_patterns.join(", ");
        match (
            self.include_patterns.is_empty(),
            self.exclude_patterns.is_empty(),
        ) {
            (false, true) => format!("skip: no changed files match {include}"),
            (false, false) => {
                format!("skip: no changed files match {include} (excluding {exclude})")
            }
            (true, false) => format!("skip: all changed files excluded by {exclude}"),
            (true, true) => "skip: no changed files".to_string(),
        }
    }
}

/// Compile a pattern list into a [`GlobSet`], or `None` when empty so the
/// no-pattern case costs nothing. `literal_separator` gives the doublestar
/// dialect: `*`/`?` stop at `/` while `**` crosses it.
fn compile(patterns: &[String]) -> Result<Option<GlobSet>> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .with_context(|| format!("invalid glob pattern '{pattern}'"))?;
        builder.add(glob);
    }
    Ok(Some(builder.build().context("failed to compile glob set")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn filter(include: &[&str], exclude: &[&str]) -> FileFilter {
        FileFilter::new(&strs(include), &strs(exclude)).unwrap()
    }

    #[test]
    fn star_does_not_cross_directories() {
        let f = filter(&["*.rs"], &[]);
        assert!(f.matches("lib.rs"));
        assert!(!f.matches("src/lib.rs"), "* must not cross /");
    }

    #[test]
    fn doublestar_spans_zero_or_more_directories() {
        // The dialect decision: `**/*.rs` matches a root-level file too
        // (zero directories), unlike legacy matchers where `**` means 1+.
        let f = filter(&["**/*.rs"], &[]);
        assert!(f.matches("lib.rs"));
        assert!(f.matches("src/lib.rs"));
        assert!(f.matches("a/b/c/lib.rs"));

        let g = filter(&["src/**/*.rs"], &[]);
        assert!(g.matches("src/lib.rs"), "zero-directory case must match");
        assert!(g.matches("src/a/b/lib.rs"));
        assert!(!g.matches("other/lib.rs"));
    }

    #[test]
    fn trailing_doublestar_matches_subtree() {
        let f = filter(&["docs/**"], &[]);
        assert!(f.matches("docs/index.md"));
        assert!(f.matches("docs/a/b.md"));
        assert!(!f.matches("docs"));
        assert!(!f.matches("src/docs.rs"));
    }

    #[test]
    fn brace_alternation() {
        let f = filter(&["Cargo.{toml,lock}"], &[]);
        assert!(f.matches("Cargo.toml"));
        assert!(f.matches("Cargo.lock"));
        assert!(!f.matches("Cargo.rs"));
    }

    #[test]
    fn matching_is_case_sensitive() {
        let f = filter(&["*.RS"], &[]);
        assert!(!f.matches("lib.rs"));
        assert!(f.matches("LIB.RS"));
    }

    #[test]
    fn exclude_wins_over_include() {
        let f = filter(&["src/**"], &["src/generated/**"]);
        assert!(f.matches("src/lib.rs"));
        assert!(!f.matches("src/generated/api.rs"));
    }

    #[test]
    fn no_include_patterns_selects_everything_not_excluded() {
        let f = filter(&[], &["docs/**", "*.md"]);
        assert!(f.matches("src/lib.rs"));
        assert!(!f.matches("docs/index.md"));
        assert!(!f.matches("README.md"));
    }

    #[test]
    fn leading_dot_slash_is_stripped() {
        let f = filter(&["src/**"], &[]);
        assert!(f.matches("./src/lib.rs"));
    }

    #[test]
    fn filter_preserves_order() {
        let f = filter(&["*.rs", "src/**"], &[]);
        let files = strs(&["z.rs", "docs/x.md", "a.rs", "src/m.rs"]);
        assert_eq!(f.filter(&files), strs(&["z.rs", "a.rs", "src/m.rs"]));
    }

    #[test]
    fn invalid_pattern_is_a_config_error_naming_the_pattern() {
        let err = FileFilter::new(&strs(&["src/[unclosed"]), &[]).unwrap_err();
        assert!(
            format!("{err:#}").contains("src/[unclosed"),
            "error should name the bad pattern: {err:#}"
        );
    }

    #[test]
    fn empty_reason_phrasings() {
        assert_eq!(
            filter(&["src/**"], &[]).empty_reason(),
            "skip: no changed files match src/**"
        );
        assert_eq!(
            filter(&["src/**"], &["docs/**"]).empty_reason(),
            "skip: no changed files match src/** (excluding docs/**)"
        );
        assert_eq!(
            filter(&[], &["docs/**"]).empty_reason(),
            "skip: all changed files excluded by docs/**"
        );
        assert_eq!(filter(&[], &[]).empty_reason(), "skip: no changed files");
    }
}
