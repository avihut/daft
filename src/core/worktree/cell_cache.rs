//! Cache wrappers for the slow `daft list` cells.
//!
//! Each public `cached_*` function (added in subsequent tasks):
//!   1. Resolves the SHAs that fully define the cell's inputs.
//!   2. Reads the cache entry keyed by those SHAs.
//!   3. On hit, returns the cached value.
//!   4. On miss, calls the underlying compute fn, writes the result, returns
//!      it.
//!
//! Cells covered (all are pure functions of their key SHAs):
//!   - `cached_base_ahead_behind`   key: `(base_sha, head_sha)`
//!   - `cached_remote_ahead_behind` key: `(head_sha, upstream_sha)`
//!   - `cached_last_commit`         key: `head_sha`
//!   - `cached_base_lines`          key: `(base_sha, head_sha)`
//!   - `cached_remote_lines`        key: `(head_sha, upstream_sha)`
//!
//! Cells deliberately not cached *here* — this cache only holds values that
//! are provably-correct pure functions of their key SHAs, so a hit needs no
//! TTL or invalidation:
//!   - Changes (staged/unstaged/untracked counts): working-tree-dependent, no
//!     SHA captures it.
//!   - Branch age, owner: cheap enough already.
//!   - Size (filesystem walk): not a pure function of any SHA — nested content
//!     changes don't move a tracked ref — so it can't live in a
//!     provably-correct cache. It instead has its own *stale-then-refresh*
//!     cache in the store (`worktree_sizes` / `repo_sizes`): the walk always
//!     re-runs, and the tri-state Size cell renders the last-known value dim
//!     until the fresh figure supersedes it, making the staleness honest
//!     rather than hidden. See `commands::size_cache`.

use crate::core::cache;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Resolve a ref to its 40-char SHA via `git rev-parse`. Returns `None` on
/// any failure (unknown ref, git missing, non-utf8 output, malformed SHA).
pub(super) fn resolve_ref_sha(worktree_path: &Path, refname: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", refname])
        .current_dir(worktree_path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(s)
    } else {
        None
    }
}

/// Path for a SHA-pair cache entry: `<git-common>/.daft/cache/<kind>/<a>-<b>.json`.
/// Caller decides ordering (typically the natural input order, not sorted).
pub(super) fn pair_key_path(git_common_dir: &Path, kind: &str, a: &str, b: &str) -> PathBuf {
    cache::cache_dir_for(git_common_dir, kind).join(format!("{a}-{b}.json"))
}

/// Path for a single-SHA cache entry: `<git-common>/.daft/cache/<kind>/<sha>.json`.
pub(super) fn single_key_path(git_common_dir: &Path, kind: &str, sha: &str) -> PathBuf {
    cache::cache_dir_for(git_common_dir, kind).join(format!("{sha}.json"))
}

/// Cached wrapper for `(base..head)` ahead/behind counts.
///
/// Cache key: `(base_sha, head_sha)`. The result is a pure function of these
/// two SHAs, so a hit is provably correct — no TTL or invalidation needed.
/// `None` results are NOT cached (the underlying git command may have failed
/// transiently; we want the next run to retry).
pub(crate) fn cached_base_ahead_behind<F>(
    git_common_dir: &Path,
    base_sha: &str,
    head_sha: &str,
    compute: F,
) -> Option<(usize, usize)>
where
    F: FnOnce() -> Option<(usize, usize)>,
{
    let path = pair_key_path(git_common_dir, "base-ahead-behind", base_sha, head_sha);
    if let Some(v) = cache::read_json::<(usize, usize)>(&path) {
        return Some(v);
    }
    let computed = compute()?;
    cache::write_json(&path, &computed);
    Some(computed)
}

/// Cached wrapper for upstream ahead/behind counts.
///
/// Cache key: `(head_sha, upstream_sha)`. Pure function — hit is provably
/// correct. `None` results are not cached.
pub(crate) fn cached_remote_ahead_behind<F>(
    git_common_dir: &Path,
    head_sha: &str,
    upstream_sha: &str,
    compute: F,
) -> Option<(usize, usize)>
where
    F: FnOnce() -> Option<(usize, usize)>,
{
    let path = pair_key_path(
        git_common_dir,
        "remote-ahead-behind",
        head_sha,
        upstream_sha,
    );
    if let Some(v) = cache::read_json::<(usize, usize)>(&path) {
        return Some(v);
    }
    let computed = compute()?;
    cache::write_json(&path, &computed);
    Some(computed)
}

/// Cached wrapper for `(timestamp, hash, subject)` of the commit at `head_sha`.
///
/// Cache key: `head_sha`. The cached `hash` field always equals the key, but
/// we store it for shape-compatibility with `get_commit_metadata`'s return
/// type. Empty / `None` timestamp results are not cached (treated as failures).
pub(crate) fn cached_last_commit<F>(
    git_common_dir: &Path,
    head_sha: &str,
    compute: F,
) -> (Option<i64>, Option<String>, String)
where
    F: FnOnce() -> (Option<i64>, Option<String>, String),
{
    let path = single_key_path(git_common_dir, "last-commit", head_sha);
    if let Some(v) = cache::read_json::<(Option<i64>, Option<String>, String)>(&path) {
        return v;
    }
    let computed = compute();
    if computed.0.is_some() {
        cache::write_json(&path, &computed);
    }
    computed
}

/// Cached wrapper for `(inserted, deleted)` line counts in `base..head`.
///
/// Cache key: `(base_sha, head_sha)`. The result is a pure function of the
/// two SHAs. `None` results are not cached (treated as transient compute
/// failures — next run retries).
pub(crate) fn cached_base_lines<F>(
    git_common_dir: &Path,
    base_sha: &str,
    head_sha: &str,
    compute: F,
) -> Option<(usize, usize)>
where
    F: FnOnce() -> Option<(usize, usize)>,
{
    let path = pair_key_path(git_common_dir, "base-lines", base_sha, head_sha);
    if let Some(v) = cache::read_json::<(usize, usize)>(&path) {
        return Some(v);
    }
    let computed = compute()?;
    cache::write_json(&path, &computed);
    Some(computed)
}

/// Cached wrapper for upstream line counts. Cache key: `(head_sha, upstream_sha)`.
/// `None` results are not cached.
pub(crate) fn cached_remote_lines<F>(
    git_common_dir: &Path,
    head_sha: &str,
    upstream_sha: &str,
    compute: F,
) -> Option<(usize, usize)>
where
    F: FnOnce() -> Option<(usize, usize)>,
{
    let path = pair_key_path(git_common_dir, "remote-lines", head_sha, upstream_sha);
    if let Some(v) = cache::read_json::<(usize, usize)>(&path) {
        return Some(v);
    }
    let computed = compute()?;
    cache::write_json(&path, &computed);
    Some(computed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_key_path_uses_dash_separator() {
        let p = pair_key_path(Path::new("/tmp/x"), "kind", "aaa", "bbb");
        assert!(p.ends_with("aaa-bbb.json"));
    }

    #[test]
    fn single_key_path_uses_sha_filename() {
        let p = single_key_path(Path::new("/tmp/x"), "kind", "abc");
        assert!(p.ends_with("abc.json"));
    }

    #[test]
    fn resolve_ref_sha_returns_none_for_nonrepo_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let sha = resolve_ref_sha(dir.path(), "HEAD");
        assert!(
            sha.is_none(),
            "non-repo dir should return None, got {sha:?}"
        );
    }

    #[test]
    fn cached_base_ahead_behind_writes_and_reads_back() {
        use tempfile::TempDir;

        let common = TempDir::new().unwrap();
        let common_dir = common.path();

        let out = cached_base_ahead_behind(common_dir, "abc1234", "def5678", || Some((3, 1)));
        assert_eq!(out, Some((3, 1)));

        let path = pair_key_path(common_dir, "base-ahead-behind", "abc1234", "def5678");
        assert!(
            path.exists(),
            "cache file at {} not written",
            path.display()
        );

        let out2 = cached_base_ahead_behind(common_dir, "abc1234", "def5678", || {
            panic!("compute called on cache hit")
        });
        assert_eq!(out2, Some((3, 1)));
    }

    #[test]
    fn cached_base_ahead_behind_does_not_cache_none() {
        use tempfile::TempDir;

        let common = TempDir::new().unwrap();
        let common_dir = common.path();

        let out = cached_base_ahead_behind(common_dir, "abc1234", "def5678", || None);
        assert_eq!(out, None);

        let path = pair_key_path(common_dir, "base-ahead-behind", "abc1234", "def5678");
        assert!(!path.exists(), "None should not be cached");
    }

    #[test]
    fn cached_remote_ahead_behind_writes_and_reads_back() {
        use tempfile::TempDir;

        let common = TempDir::new().unwrap();
        let common_dir = common.path();

        let out = cached_remote_ahead_behind(common_dir, "head1234", "upst5678", || Some((2, 4)));
        assert_eq!(out, Some((2, 4)));

        let path = pair_key_path(common_dir, "remote-ahead-behind", "head1234", "upst5678");
        assert!(path.exists());

        let out2 = cached_remote_ahead_behind(common_dir, "head1234", "upst5678", || {
            panic!("compute called on cache hit")
        });
        assert_eq!(out2, Some((2, 4)));
    }

    #[test]
    fn cached_last_commit_writes_and_reads_back() {
        use tempfile::TempDir;

        let common = TempDir::new().unwrap();
        let common_dir = common.path();

        let computed = (
            Some(1700000000_i64),
            Some("abc1234".to_string()),
            "first commit".to_string(),
        );

        let out = cached_last_commit(common_dir, "abc1234", || computed.clone());
        assert_eq!(out, computed);

        let path = single_key_path(common_dir, "last-commit", "abc1234");
        assert!(path.exists());

        let out2 = cached_last_commit(common_dir, "abc1234", || panic!("hit"));
        assert_eq!(out2, computed);
    }

    #[test]
    fn cached_last_commit_does_not_cache_when_timestamp_missing() {
        use tempfile::TempDir;

        let common = TempDir::new().unwrap();
        let common_dir = common.path();
        let bad = (None, None, String::new());

        let out = cached_last_commit(common_dir, "abc1234", || bad.clone());
        assert_eq!(out, bad);

        let path = single_key_path(common_dir, "last-commit", "abc1234");
        assert!(!path.exists());
    }

    #[test]
    fn cached_base_lines_writes_and_reads_back() {
        use tempfile::TempDir;

        let common = TempDir::new().unwrap();
        let common_dir = common.path();

        let computed = Some((120_usize, 45_usize));
        let out = cached_base_lines(common_dir, "base", "head", || computed);
        assert_eq!(out, computed);

        let path = pair_key_path(common_dir, "base-lines", "base", "head");
        assert!(path.exists());

        let out2 = cached_base_lines(common_dir, "base", "head", || panic!("hit"));
        assert_eq!(out2, computed);
    }

    #[test]
    fn cached_base_lines_does_not_cache_none() {
        use tempfile::TempDir;

        let common = TempDir::new().unwrap();
        let common_dir = common.path();

        let out = cached_base_lines(common_dir, "base", "head", || None);
        assert_eq!(out, None);

        let path = pair_key_path(common_dir, "base-lines", "base", "head");
        assert!(!path.exists(), "None should not be cached");
    }

    #[test]
    fn cached_remote_lines_writes_and_reads_back() {
        use tempfile::TempDir;

        let common = TempDir::new().unwrap();
        let common_dir = common.path();

        let computed = Some((7_usize, 2_usize));
        let out = cached_remote_lines(common_dir, "head", "upstream", || computed);
        assert_eq!(out, computed);

        let path = pair_key_path(common_dir, "remote-lines", "head", "upstream");
        assert!(path.exists());

        let out2 = cached_remote_lines(common_dir, "head", "upstream", || panic!("hit"));
        assert_eq!(out2, computed);
    }
}
