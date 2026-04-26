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
//! Cells deliberately not cached (working-tree-dependent, would risk staleness):
//!   - Changes (staged/unstaged/untracked counts)
//!   - Size (filesystem walk)
//!   - Branch age, owner (cheap enough already)

// Helpers below are scaffolding for the `cached_*` wrappers added in
// subsequent batches. Suppress dead_code until then.
#![allow(dead_code)]

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
}
