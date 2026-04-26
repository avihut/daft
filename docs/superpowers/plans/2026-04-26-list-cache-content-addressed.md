# Content-Addressed Cache for `daft list` Slow Cells — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make warm-cache `daft list` runs near-instant for ahead/behind,
line-stat, and last-commit cells by caching their results under a
`(sha-pair) → result` key, so repeat runs hit JSON files instead of forking
`git rev-list` / `git diff` / `git log`.

**Architecture:** Add a tiny on-disk cache layer at
`<git-common-dir>/.daft/cache/<kind>/<key>.json`, mirroring worktrunk's design.
Each cell becomes "resolve key SHAs → try cache → on miss, compute and write."
The cache is **content-addressed** — the key fully captures the inputs, so a hit
is provably correct (no TTL, no invalidation needed). Cells whose inputs include
working-tree state (Size, Changes) are out of scope; they keep streaming as
today and show the `·` skeleton glyph until computed.

**Tech Stack:** Rust 2021, `serde` + `serde_json` (already deps), `std::fs`, no
new crates.

**Why this preserves the "skeleton over stale" rule:** content-addressed keys
make staleness impossible — any state change that would invalidate a result also
changes the key, producing a cache miss. A cache hit is byte-identical to a
fresh fork of the underlying git command. Cells that _can't_ be
content-addressed (Size, Changes) are deliberately excluded.

---

## File Structure

| Path                                                     | Responsibility                                                                                                                                                                                                                                                           |
| -------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `src/core/cache.rs` _(new)_                              | Filesystem primitives: `cache_dir`, `read_json`, `write_json`, `clear_kind`. Torn-write tolerance, silent-failure write policy. Mirrors worktrunk's `cache.rs`.                                                                                                          |
| `src/core/mod.rs` _(modify)_                             | Add `pub mod cache;`                                                                                                                                                                                                                                                     |
| `src/core/worktree/cell_cache.rs` _(new)_                | Per-cell wrappers: `cached_base_ahead_behind`, `cached_remote_ahead_behind`, `cached_last_commit`, `cached_base_lines`, `cached_remote_lines`. Each resolves the SHAs needed for its key, calls `read_json`, falls back to the underlying compute fn, writes the result. |
| `src/core/worktree/mod.rs` _(modify)_                    | Add `pub(crate) mod cell_cache;`                                                                                                                                                                                                                                         |
| `src/core/worktree/list_stream.rs` _(modify)_            | Replace direct calls to `get_ahead_behind`, `get_upstream_ahead_behind`, `get_commit_metadata`, `get_base_line_counts`, `get_remote_line_counts` with the `cell_cache::cached_*` wrappers.                                                                               |
| `tests/manual/scenarios/list/cache-warm-hit.yml` _(new)_ | YAML scenario: run `daft list` twice, assert cache files are written on first run and read on second.                                                                                                                                                                    |

The cache module is intentionally tiny and lives at `core/cache.rs` rather than
under `core/worktree/` so other commands (`prune`, `sync`) can adopt it later
without import gymnastics. Per-cell logic lives next to the streaming collector
that uses it.

---

## Task 1: Add cache primitives module

**Files:**

- Create: `src/core/cache.rs`
- Modify: `src/core/mod.rs:1-30`
- Test: `src/core/cache.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing tests**

In `src/core/cache.rs`, add the following at the bottom of the file (you'll
create the file in step 3 — for now just sketch the test bodies in a scratch
buffer; we'll write them in place once the module exists):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Foo {
        n: u32,
        s: String,
    }

    #[test]
    fn read_returns_none_for_missing_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("missing.json");
        let read: Option<Foo> = read_json(&path);
        assert!(read.is_none());
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("entry.json");
        let v = Foo { n: 7, s: "hello".into() };
        write_json(&path, &v);
        let got: Option<Foo> = read_json(&path);
        assert_eq!(got, Some(v));
    }

    #[test]
    fn read_returns_none_for_corrupt_json() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, b"{not-json").unwrap();
        let got: Option<Foo> = read_json(&path);
        assert!(got.is_none());
    }

    #[test]
    fn write_creates_missing_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a").join("b").join("c").join("e.json");
        write_json(&path, &Foo { n: 1, s: "x".into() });
        assert!(path.exists());
    }

    #[test]
    fn cache_dir_uses_kind_under_git_common() {
        let dir = TempDir::new().unwrap();
        let p = cache_dir_for(dir.path(), "ahead-behind");
        assert_eq!(p, dir.path().join(".daft").join("cache").join("ahead-behind"));
    }

    #[test]
    fn clear_kind_removes_directory_contents() {
        let dir = TempDir::new().unwrap();
        let kind_dir = cache_dir_for(dir.path(), "k");
        std::fs::create_dir_all(&kind_dir).unwrap();
        std::fs::write(kind_dir.join("a.json"), "{}").unwrap();
        std::fs::write(kind_dir.join("b.json"), "{}").unwrap();
        clear_kind(dir.path(), "k").unwrap();
        let entries: Vec<_> = std::fs::read_dir(&kind_dir).unwrap().collect();
        assert!(entries.is_empty());
    }
}
```

- [ ] **Step 2: Run the tests to confirm the file doesn't exist yet**

Run: `cargo test --lib core::cache 2>&1 | tail -10` Expected:
`error[E0432]: unresolved import \`crate::core::cache\`` (the module doesn't
exist yet).

- [ ] **Step 3: Create `src/core/cache.rs`**

```rust
//! On-disk JSON cache primitives shared by the per-cell cache wrappers.
//!
//! Lives under `<git-common-dir>/.daft/cache/<kind>/`. Each kind owns its
//! filename scheme (typically `{sha1}-{sha2}.json` for content-addressed pair
//! caches) and its struct shape. This module owns only the filesystem
//! mechanics so torn-write semantics and the silent-failure write policy live
//! in one place.
//!
//! # Torn-write semantics
//!
//! Writes use `fs::write` directly, not temp-file-plus-rename. A crash mid-
//! write produces a truncated file at the expected path, which `read_json`
//! rejects as corrupt JSON — indistinguishable from a cache miss from the
//! caller's perspective.
//!
//! # Error policy
//!
//! - `read_json` returns `None` on any failure (missing file, I/O error,
//!   corrupt JSON). Corrupt JSON is logged at debug.
//! - `write_json` degrades silently — a failed write just means the next
//!   read recomputes.
//! - `clear_kind` propagates I/O errors so a future `daft cache clear`
//!   command could report failures.

use serde::{de::DeserializeOwned, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// The directory for a named cache kind under the given git common dir.
pub fn cache_dir_for(git_common_dir: &Path, kind: &str) -> PathBuf {
    git_common_dir.join(".daft").join("cache").join(kind)
}

/// Read and deserialize a JSON cache entry. Returns `None` on any failure.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let json = fs::read_to_string(path).ok()?;
    match serde_json::from_str::<T>(&json) {
        Ok(value) => Some(value),
        Err(e) => {
            log::debug!("cache: corrupt entry at {}: {}", path.display(), e);
            None
        }
    }
}

/// Serialize and write a JSON cache entry, creating parent dirs as needed.
/// Degrades silently on any failure.
pub fn write_json<T: Serialize>(path: &Path, value: &T) {
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            log::debug!("cache: mkdir {} failed: {}", parent.display(), e);
            return;
        }
    }
    let json = match serde_json::to_string(value) {
        Ok(j) => j,
        Err(e) => {
            log::debug!("cache: serialize {} failed: {}", path.display(), e);
            return;
        }
    };
    if let Err(e) = fs::write(path, json) {
        log::debug!("cache: write {} failed: {}", path.display(), e);
    }
}

/// Delete every regular file in the cache kind's directory. Errors propagate
/// so callers can report partial failures.
pub fn clear_kind(git_common_dir: &Path, kind: &str) -> std::io::Result<()> {
    let dir = cache_dir_for(git_common_dir, kind);
    let read = match fs::read_dir(&dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in read {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Wire up the module**

In `src/core/mod.rs`, add `pub mod cache;` in alphabetical order (after
`pub mod columns;` line if present, otherwise at the appropriate alpha
position).

```rust
// Look for the existing `pub mod ...` block in core/mod.rs and add:
pub mod cache;
```

- [ ] **Step 5: Run the tests to confirm they pass**

Run: `cargo test --lib core::cache 2>&1 | tail -10` Expected:
`test result: ok. 6 passed; 0 failed; 0 ignored`

- [ ] **Step 6: Run clippy + fmt**

Run: `mise run clippy && mise run fmt` Expected: clippy clean, fmt applies no
changes (or only this file).

- [ ] **Step 7: Commit**

```bash
git add src/core/cache.rs src/core/mod.rs
git commit -m "feat(cache): add JSON cache primitives under .git/.daft/cache/ (#402)"
```

---

## Task 2: Add SHA-resolution helpers

The cell cache wrappers need each branch's HEAD SHA, the base branch's HEAD SHA,
and (for remote cells) the upstream's HEAD SHA. Add small batched helpers so
each lookup is O(1) `git` forks across the whole list, not O(N).

**Files:**

- Create: `src/core/worktree/cell_cache.rs` (skeleton; subsequent tasks add
  per-cell wrappers)
- Modify: `src/core/worktree/mod.rs` (add `pub(crate) mod cell_cache;`)
- Test: `src/core/worktree/cell_cache.rs` (inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing tests**

In `src/core/worktree/cell_cache.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_ref_sha_returns_some_for_head() {
        // Use the integration-test repo helper.
        let repo = test_helpers::TempRepo::new_with_commits(1);
        let sha = resolve_ref_sha(repo.path(), "HEAD");
        assert!(sha.is_some());
        assert_eq!(sha.as_deref().map(str::len), Some(40));
    }

    #[test]
    fn resolve_ref_sha_returns_none_for_unknown_ref() {
        let repo = test_helpers::TempRepo::new_with_commits(1);
        let sha = resolve_ref_sha(repo.path(), "refs/heads/does-not-exist");
        assert!(sha.is_none());
    }
}
```

You'll need to confirm whether a `test_helpers::TempRepo` helper already exists.
If it doesn't, check `tests/integration/` for an existing helper or skip these
unit tests and rely on the smoke scenario in Task 8 instead — leave a
`// TODO(#402): add unit tests once a TempRepo helper exists` and don't fake it.

To check:

Run:
`grep -rn "TempRepo\|temp_repo\|fn new_with_commits" src/ tests/ 2>&1 | head -5`

If no helper exists, skip the unit test for `resolve_ref_sha` and rely on
integration coverage in Task 8 instead.

- [ ] **Step 2: Create `src/core/worktree/cell_cache.rs`**

```rust
//! Cache wrappers for the slow `daft list` cells.
//!
//! Each public `cached_*` function:
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

use crate::core::cache;
use std::path::Path;
use std::process::Command;

/// Resolve a ref to its 40-char SHA via `git rev-parse`. Returns `None` on
/// any failure (unknown ref, git missing, non-utf8 output, etc.).
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

/// Build the path for a SHA-pair cache entry: `<git-common>/.daft/cache/<kind>/<a>-<b>.json`.
/// SHAs are passed in their natural order — caller decides whether to sort.
pub(super) fn pair_key_path(git_common_dir: &Path, kind: &str, a: &str, b: &str) -> std::path::PathBuf {
    cache::cache_dir_for(git_common_dir, kind).join(format!("{a}-{b}.json"))
}

/// Build the path for a single-SHA cache entry: `<git-common>/.daft/cache/<kind>/<sha>.json`.
pub(super) fn single_key_path(git_common_dir: &Path, kind: &str, sha: &str) -> std::path::PathBuf {
    cache::cache_dir_for(git_common_dir, kind).join(format!("{sha}.json"))
}
```

- [ ] **Step 3: Wire up the module**

In `src/core/worktree/mod.rs`, add (alphabetically positioned among the existing
`mod` declarations):

```rust
pub(crate) mod cell_cache;
```

- [ ] **Step 4: Run any unit tests you wrote in Step 1**

Run: `cargo test --lib core::worktree::cell_cache 2>&1 | tail -10` Expected:
passes (or, if you skipped per Step 1's note, no tests run for this module yet).

- [ ] **Step 5: Build the whole crate**

Run: `cargo build --bin daft 2>&1 | tail -10` Expected: clean build, no
warnings.

- [ ] **Step 6: Commit**

```bash
git add src/core/worktree/cell_cache.rs src/core/worktree/mod.rs
git commit -m "feat(cache): add SHA-resolution helpers for cell-cache wrappers (#402)"
```

---

## Task 3: Cache wrapper for `cached_base_ahead_behind`

**Files:**

- Modify: `src/core/worktree/cell_cache.rs` (add the wrapper + tests)
- Test: same file

- [ ] **Step 1: Write the failing test**

Append to `src/core/worktree/cell_cache.rs` (inside `#[cfg(test)] mod tests`):

```rust
#[test]
fn cached_base_ahead_behind_writes_and_reads_back() {
    use tempfile::TempDir;

    let common = TempDir::new().unwrap();
    let common_dir = common.path();

    // First call: simulate compute returning Some((3, 1)). It should be
    // written to the cache.
    let out = cached_base_ahead_behind(
        common_dir, "abc1234", "def5678",
        || Some((3, 1)),
    );
    assert_eq!(out, Some((3, 1)));

    // The cache file should now exist.
    let path = pair_key_path(common_dir, "base-ahead-behind", "abc1234", "def5678");
    assert!(path.exists(), "cache file at {} not written", path.display());

    // Second call with a panicking compute fn: cache hit should bypass it.
    let out2 = cached_base_ahead_behind(
        common_dir, "abc1234", "def5678",
        || panic!("compute called on cache hit"),
    );
    assert_eq!(out2, Some((3, 1)));
}

#[test]
fn cached_base_ahead_behind_does_not_cache_none() {
    use tempfile::TempDir;

    let common = TempDir::new().unwrap();
    let common_dir = common.path();

    // Compute returns None (e.g., git command failed). Don't poison the cache.
    let out = cached_base_ahead_behind(
        common_dir, "abc1234", "def5678",
        || None,
    );
    assert_eq!(out, None);

    let path = pair_key_path(common_dir, "base-ahead-behind", "abc1234", "def5678");
    assert!(!path.exists(), "None should not be cached");
}
```

- [ ] **Step 2: Run the test to confirm it fails**

Run:
`cargo test --lib core::worktree::cell_cache::tests::cached_base_ahead_behind 2>&1 | tail -10`
Expected: compile error (`cached_base_ahead_behind` not found) — that's the
failure we want.

- [ ] **Step 3: Implement the wrapper**

Append to `src/core/worktree/cell_cache.rs` (above the `#[cfg(test)]` block):

```rust
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
```

- [ ] **Step 4: Run the tests to confirm they pass**

Run: `cargo test --lib core::worktree::cell_cache 2>&1 | tail -10` Expected:
`test result: ok. <N> passed; 0 failed`

- [ ] **Step 5: Commit**

```bash
git add src/core/worktree/cell_cache.rs
git commit -m "feat(cache): add cached_base_ahead_behind wrapper (#402)"
```

---

## Task 4: Cache wrapper for `cached_remote_ahead_behind`

Same shape as Task 3 with a different cache kind name. Distinct from base
because the inputs (head + upstream) are different and it deserves its own kind
subdir for human inspection / future per-kind clear.

**Files:**

- Modify: `src/core/worktree/cell_cache.rs`

- [ ] **Step 1: Add the test**

Append to the `#[cfg(test)] mod tests` block:

```rust
#[test]
fn cached_remote_ahead_behind_writes_and_reads_back() {
    use tempfile::TempDir;

    let common = TempDir::new().unwrap();
    let common_dir = common.path();

    let out = cached_remote_ahead_behind(
        common_dir, "head1234", "upst5678",
        || Some((2, 4)),
    );
    assert_eq!(out, Some((2, 4)));

    let path = pair_key_path(common_dir, "remote-ahead-behind", "head1234", "upst5678");
    assert!(path.exists());

    let out2 = cached_remote_ahead_behind(
        common_dir, "head1234", "upst5678",
        || panic!("compute called on cache hit"),
    );
    assert_eq!(out2, Some((2, 4)));
}
```

- [ ] **Step 2: Confirm test fails (compile error)**

Run:
`cargo test --lib core::worktree::cell_cache::tests::cached_remote_ahead_behind 2>&1 | tail -10`
Expected: compile error (`cached_remote_ahead_behind` not found).

- [ ] **Step 3: Implement the wrapper**

Append above the `#[cfg(test)]` block in `src/core/worktree/cell_cache.rs`:

```rust
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
    let path = pair_key_path(git_common_dir, "remote-ahead-behind", head_sha, upstream_sha);
    if let Some(v) = cache::read_json::<(usize, usize)>(&path) {
        return Some(v);
    }
    let computed = compute()?;
    cache::write_json(&path, &computed);
    Some(computed)
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib core::worktree::cell_cache 2>&1 | tail -10` Expected: all
tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/core/worktree/cell_cache.rs
git commit -m "feat(cache): add cached_remote_ahead_behind wrapper (#402)"
```

---

## Task 5: Cache wrapper for `cached_last_commit`

Single-SHA key (HEAD SHA → `(timestamp, hash, subject)`). The cached `hash`
value will always equal the key SHA, but storing it keeps the result-shape
identical to `get_commit_metadata`'s return type, so the call sites don't need
to special-case.

**Files:**

- Modify: `src/core/worktree/cell_cache.rs`

- [ ] **Step 1: Add the test**

Append to the `#[cfg(test)] mod tests` block:

```rust
#[test]
fn cached_last_commit_writes_and_reads_back() {
    use tempfile::TempDir;

    let common = TempDir::new().unwrap();
    let common_dir = common.path();

    let computed = (Some(1700000000_i64), Some("abc1234".to_string()), "first commit".to_string());

    let out = cached_last_commit(common_dir, "abc1234", || computed.clone());
    assert_eq!(out, computed);

    let path = single_key_path(common_dir, "last-commit", "abc1234");
    assert!(path.exists());

    // Cache hit short-circuits the compute closure.
    let out2 = cached_last_commit(common_dir, "abc1234", || panic!("hit"));
    assert_eq!(out2, computed);
}

#[test]
fn cached_last_commit_does_not_cache_when_timestamp_missing() {
    // If git returned no timestamp, the compute most likely failed. Don't
    // poison the cache.
    use tempfile::TempDir;

    let common = TempDir::new().unwrap();
    let common_dir = common.path();
    let bad = (None, None, String::new());

    let out = cached_last_commit(common_dir, "abc1234", || bad.clone());
    assert_eq!(out, bad);

    let path = single_key_path(common_dir, "last-commit", "abc1234");
    assert!(!path.exists());
}
```

- [ ] **Step 2: Confirm test fails**

Run:
`cargo test --lib core::worktree::cell_cache::tests::cached_last_commit 2>&1 | tail -10`
Expected: compile error.

- [ ] **Step 3: Implement the wrapper**

Append above the `#[cfg(test)]` block:

```rust
/// Cached wrapper for `(timestamp, hash, subject)` of the commit at `head_sha`.
///
/// Cache key: `head_sha`. The cached `hash` field will always equal the key,
/// but we store it for shape-compatibility with `get_commit_metadata`'s
/// return type. Empty / `None` results are not cached (treated as failures).
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
    // Treat "no timestamp" as a failed lookup — don't poison the cache.
    if computed.0.is_some() {
        cache::write_json(&path, &computed);
    }
    computed
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test --lib core::worktree::cell_cache 2>&1 | tail -10` Expected: all
pass.

- [ ] **Step 5: Commit**

```bash
git add src/core/worktree/cell_cache.rs
git commit -m "feat(cache): add cached_last_commit wrapper (#402)"
```

---

## Task 6: Cache wrapper for `cached_base_lines`

**Files:**

- Modify: `src/core/worktree/cell_cache.rs`

- [ ] **Step 1: Add the test**

Append to the test block:

```rust
#[test]
fn cached_base_lines_writes_and_reads_back() {
    use tempfile::TempDir;

    let common = TempDir::new().unwrap();
    let common_dir = common.path();

    let computed = (Some(120_usize), Some(45_usize));
    let out = cached_base_lines(common_dir, "base", "head", || computed);
    assert_eq!(out, computed);

    let path = pair_key_path(common_dir, "base-lines", "base", "head");
    assert!(path.exists());

    let out2 = cached_base_lines(common_dir, "base", "head", || panic!("hit"));
    assert_eq!(out2, computed);
}
```

- [ ] **Step 2: Confirm fail**

Run:
`cargo test --lib core::worktree::cell_cache::tests::cached_base_lines 2>&1 | tail -10`
Expected: compile error.

- [ ] **Step 3: Implement**

Append above `#[cfg(test)]`:

```rust
/// Cached wrapper for `(inserted, deleted)` line counts in `base..head`.
///
/// Cache key: `(base_sha, head_sha)`. Only writes the cache when at least
/// one of the two values is `Some` (treats fully-`None` as failure).
pub(crate) fn cached_base_lines<F>(
    git_common_dir: &Path,
    base_sha: &str,
    head_sha: &str,
    compute: F,
) -> (Option<usize>, Option<usize>)
where
    F: FnOnce() -> (Option<usize>, Option<usize>),
{
    let path = pair_key_path(git_common_dir, "base-lines", base_sha, head_sha);
    if let Some(v) = cache::read_json::<(Option<usize>, Option<usize>)>(&path) {
        return v;
    }
    let computed = compute();
    if computed.0.is_some() || computed.1.is_some() {
        cache::write_json(&path, &computed);
    }
    computed
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test --lib core::worktree::cell_cache 2>&1 | tail -10` Expected: all
pass.

- [ ] **Step 5: Commit**

```bash
git add src/core/worktree/cell_cache.rs
git commit -m "feat(cache): add cached_base_lines wrapper (#402)"
```

---

## Task 7: Cache wrapper for `cached_remote_lines`

Same shape as Task 6 with a different kind directory.

**Files:**

- Modify: `src/core/worktree/cell_cache.rs`

- [ ] **Step 1: Add the test**

```rust
#[test]
fn cached_remote_lines_writes_and_reads_back() {
    use tempfile::TempDir;

    let common = TempDir::new().unwrap();
    let common_dir = common.path();

    let computed = (Some(7_usize), Some(2_usize));
    let out = cached_remote_lines(common_dir, "head", "upstream", || computed);
    assert_eq!(out, computed);

    let path = pair_key_path(common_dir, "remote-lines", "head", "upstream");
    assert!(path.exists());

    let out2 = cached_remote_lines(common_dir, "head", "upstream", || panic!("hit"));
    assert_eq!(out2, computed);
}
```

- [ ] **Step 2: Confirm fail**

Run:
`cargo test --lib core::worktree::cell_cache::tests::cached_remote_lines 2>&1 | tail -10`
Expected: compile error.

- [ ] **Step 3: Implement**

```rust
/// Cached wrapper for upstream line counts. Cache key: `(head_sha, upstream_sha)`.
pub(crate) fn cached_remote_lines<F>(
    git_common_dir: &Path,
    head_sha: &str,
    upstream_sha: &str,
    compute: F,
) -> (Option<usize>, Option<usize>)
where
    F: FnOnce() -> (Option<usize>, Option<usize>),
{
    let path = pair_key_path(git_common_dir, "remote-lines", head_sha, upstream_sha);
    if let Some(v) = cache::read_json::<(Option<usize>, Option<usize>)>(&path) {
        return v;
    }
    let computed = compute();
    if computed.0.is_some() || computed.1.is_some() {
        cache::write_json(&path, &computed);
    }
    computed
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test --lib core::worktree::cell_cache 2>&1 | tail -10` Expected: all
pass.

- [ ] **Step 5: Commit**

```bash
git add src/core/worktree/cell_cache.rs
git commit -m "feat(cache): add cached_remote_lines wrapper (#402)"
```

---

## Task 8: Wire wrappers into the streaming collector

This task replaces the direct calls inside `run_branch_clusters` (or whatever
the per-target slow-cluster fn is currently named) with `cached_*` calls. The
collector resolves required SHAs once per target before the cluster.

**Files:**

- Modify: `src/core/worktree/list_stream.rs:130-260` (the `run_target` cluster
  body — exact line numbers may shift; search for `BASE_AHEAD_BEHIND` /
  `LAST_COMMIT` / `BASE_LINES` / `REMOTE_LINES`)

- [ ] **Step 1: Read the current `run_target` (or equivalent) implementation**

Run:
`grep -n "fn run_target\|fn process_target\|fn cluster_for\|FieldSet::BASE_AHEAD_BEHIND" src/core/worktree/list_stream.rs | head -10`

Open the file and read the cluster body — you need to know the surrounding
context (where `target`, `git`, `ctx` come from, how patches are emitted) before
making edits.

- [ ] **Step 2: Add `git_common_dir` to `CollectorContext`**

In `src/core/worktree/list_stream.rs`, find the `CollectorContext` struct
definition. Add a new field:

```rust
pub struct CollectorContext {
    pub use_gitoxide: bool,
    pub base_branch: String,
    pub remote_name: String,
    pub ownership_strategy: OwnershipStrategy,
    pub user_email: Option<String>,
    pub git_common_dir: std::path::PathBuf, // NEW
}
```

- [ ] **Step 3: Pass `git_common_dir` from every caller**

Find every place that constructs `CollectorContext`. As of this branch there are
at least three: `commands/list_live.rs`, `commands/sync.rs`, and
`commands/prune.rs` (search with `grep`).

Run: `grep -rn "CollectorContext {" src/`

For each construction site, add the new field. The git common dir is already
available via `crate::core::repo::get_git_common_dir()?` — every caller already
runs in a context where this is fetched (e.g., `commands/list_live.rs:86` calls
it). Add the field initializer using the existing variable.

- [ ] **Step 4: Replace direct compute calls with cached wrappers**

In the cluster body, where `BASE_AHEAD_BEHIND` is computed (currently around
line 165-170), change:

```rust
if fields.contains(FieldSet::BASE_AHEAD_BEHIND) && !target.is_detached {
    if let Some(p) = path {
        let v = get_ahead_behind(&ctx.base_branch, &target.branch_name, p);
        emit!(P::BaseAheadBehind(v));
    }
}
```

…to:

```rust
if fields.contains(FieldSet::BASE_AHEAD_BEHIND) && !target.is_detached {
    if let Some(p) = path {
        // Resolve the two key SHAs. If either lookup fails we skip the cache
        // entirely and fall back to the direct compute path so we at least
        // emit *something* for the cell.
        let base_sha = crate::core::worktree::cell_cache::resolve_ref_sha(p, &ctx.base_branch);
        let head_sha = crate::core::worktree::cell_cache::resolve_ref_sha(p, "HEAD");
        let v = match (base_sha, head_sha) {
            (Some(b), Some(h)) => crate::core::worktree::cell_cache::cached_base_ahead_behind(
                &ctx.git_common_dir, &b, &h,
                || get_ahead_behind(&ctx.base_branch, &target.branch_name, p),
            ),
            _ => get_ahead_behind(&ctx.base_branch, &target.branch_name, p),
        };
        emit!(P::BaseAheadBehind(v));
    }
}
```

Apply the analogous transform to:

- `LAST_COMMIT` block (around line 185): resolve `HEAD` SHA →
  `cached_last_commit`
- `REMOTE_AHEAD_BEHIND` block (around line 220): resolve `HEAD` and
  `<branch>@{upstream}` SHAs → `cached_remote_ahead_behind`
- `BASE_LINES` block (around line 229): resolve base + HEAD →
  `cached_base_lines`
- `REMOTE_LINES` block (around line 244): resolve HEAD + upstream →
  `cached_remote_lines`

For `REMOTE_*`, use `&format!("{}@{{upstream}}", target.branch_name)` as the
upstream refname (or `<branch>` if a more specific refname is already known to
the cluster).

- [ ] **Step 5: Build**

Run: `cargo build --bin daft 2>&1 | tail -10` Expected: clean build.

- [ ] **Step 6: Run the unit suite**

Run: `mise run test:unit 2>&1 | grep -E "test result|FAILED" | tail -10`
Expected: all tests pass.

- [ ] **Step 7: Run clippy + fmt**

Run: `mise run clippy && mise run fmt && mise run fmt:check 2>&1 | tail -3`
Expected: clean.

- [ ] **Step 8: Manual smoke test**

```bash
# In a scratch repo (not this one — see CLAUDE.md):
TMP=$(mktemp -d)
cd "$TMP" && git init -q && \
  git -c user.email=t@t -c user.name=t commit --allow-empty -q -m initial && \
  git -c user.email=t@t -c user.name=t commit --allow-empty -q -m second
# Run daft list (ratherthan the live UI; we want to confirm cache writes)
DAFT_NO_LIVE=1 /Users/avihu/Projects/daft/daft-402/feat/live-list-population/target/debug/daft list
ls .git/.daft/cache/
# Expected: directories like base-ahead-behind/, last-commit/, etc., with .json files inside.
# Run a second time:
DAFT_NO_LIVE=1 /Users/avihu/Projects/daft/daft-402/feat/live-list-population/target/debug/daft list
# Expected: same files (mtimes unchanged for SHAs that didn't move; new files for new SHAs).
cd / && rm -rf "$TMP"
```

- [ ] **Step 9: Commit**

```bash
git add src/core/worktree/list_stream.rs src/commands/list_live.rs src/commands/sync.rs src/commands/prune.rs
git commit -m "feat(cache): wire cell-cache wrappers into list_stream collector (#402)"
```

---

## Task 9: Integration smoke test

Add a YAML scenario that asserts the cache files appear after a `daft list` run.

**Files:**

- Create: `tests/manual/scenarios/list/cache-warm-hit.yml`

- [ ] **Step 1: Read the YAML test schema reference**

Run: `head -80 tests/README.md`

Note the schema fields used by existing list scenarios.

- [ ] **Step 2: Find a similar existing scenario to model after**

Run: `ls tests/manual/scenarios/list/ | head -10`

Open the closest existing list scenario and read it.

- [ ] **Step 3: Write the new scenario**

Create `tests/manual/scenarios/list/cache-warm-hit.yml`:

```yaml
name: list-cache-warm-hit
description: |
  Verifies that running `daft list` populates the on-disk cache under
  .git/.daft/cache/ and that a second run hits those cache files.

setup:
  # A tiny repo with one extra commit so base..head ahead/behind has
  # something non-trivial to cache.
  - shell: |
      git init -q .
      git config user.email t@t
      git config user.name t
      git commit -q --allow-empty -m initial
      git checkout -q -b feature
      git commit -q --allow-empty -m feature-commit

steps:
  - name: First list run populates cache
    run: daft list
    env:
      DAFT_NO_LIVE: "1"
    assert:
      exit_code: 0
      shell: |
        # At least one cache kind dir should exist with at least one .json file.
        find .git/.daft/cache -type f -name '*.json' | head -1 | grep -q .

  - name: Second list run hits cache
    run: daft list
    env:
      DAFT_NO_LIVE: "1"
    assert:
      exit_code: 0
```

If the YAML schema in this repo differs from the example above, adapt the field
names but keep the semantics: setup repo → run list → assert cache files exist →
run list again → assert success.

- [ ] **Step 4: Run the new scenario**

Run: `mise run test:manual -- --ci list:cache-warm-hit` Expected: PASS.

- [ ] **Step 5: Run the broader list scenario set to catch regressions**

Run: `mise run test:manual -- --ci list 2>&1 | tail -20` Expected: all list
scenarios pass.

- [ ] **Step 6: Commit**

```bash
git add tests/manual/scenarios/list/cache-warm-hit.yml
git commit -m "test(list): assert .daft/cache entries materialize after run (#402)"
```

---

## Task 10: Update SKILL.md and docs

The cache directory is a new on-disk artifact that AI agents and users should be
aware of. Document it briefly.

**Files:**

- Modify: `SKILL.md`
- Modify: `docs/guide/configuration.md` (if there's an existing section on state
  directories)

- [ ] **Step 1: Find the right place in SKILL.md**

Run: `grep -n "cache\|state\|.git/" SKILL.md | head -20`

Look for an existing "State directories" / "Files daft writes" section. If none,
add a new short section.

- [ ] **Step 2: Add the cache section**

Insert (or update an existing "State files" section):

```markdown
### Cache files

`daft list` writes content-addressed JSON caches under
`<git-common-dir>/.daft/cache/<kind>/` to make repeat runs fast. Each entry is
keyed by the SHAs that fully define its inputs (e.g. `(base_sha, head_sha)` for
ahead/behind counts), so a cache hit is provably correct — no TTL or manual
invalidation is needed. The cache is safe to delete at any time; `daft list`
will re-populate it on the next run.

Cached cells: base/remote ahead-behind, base/remote line stats, last-commit
metadata. Working-tree-dependent cells (`Changes`, `Size`) are NOT cached
because their inputs aren't capturable as a SHA.
```

- [ ] **Step 3: Update `docs/guide/configuration.md` if it has a state-files
      section**

Run: `grep -n "state\|cache\|.daft" docs/guide/configuration.md`

If there's a relevant section, mirror the SKILL.md addition there. If not, skip
— don't fabricate one.

- [ ] **Step 4: Commit**

```bash
git add SKILL.md docs/guide/configuration.md 2>/dev/null
git commit -m "docs: describe new .daft/cache state directory (#402)"
```

---

## Task 11: Final verification

- [ ] **Step 1: Full unit suite**

Run: `mise run test:unit 2>&1 | grep -E "test result|FAILED" | tail -10`
Expected: all pass, count includes the new cache + cell_cache tests.

- [ ] **Step 2: Clippy + fmt**

Run: `mise run clippy && mise run fmt:check` Expected: both clean.

- [ ] **Step 3: Manual perf check**

```bash
TMP=$(mktemp -d)
cd "$TMP" && git init -q && \
  git -c user.email=t@t -c user.name=t commit --allow-empty -q -m initial && \
  git checkout -q -b feature && \
  git -c user.email=t@t -c user.name=t commit --allow-empty -q -m feature-commit
# Cold cache run
time DAFT_NO_LIVE=1 /Users/avihu/Projects/daft/daft-402/feat/live-list-population/target/debug/daft list >/dev/null
# Warm cache run — should be visibly faster
time DAFT_NO_LIVE=1 /Users/avihu/Projects/daft/daft-402/feat/live-list-population/target/debug/daft list >/dev/null
cd / && rm -rf "$TMP"
```

Expected: warm run notably faster than cold (no hard threshold; even a few
hundred ms savings on the small repo is fine — the win scales with worktree
count).

- [ ] **Step 4: Push the branch**

```bash
git push
```

---

## Self-Review

**Spec coverage:**

- ✅ Cache primitives (Task 1)
- ✅ SHA resolution (Task 2)
- ✅ Cache wrappers for the five SHA-pair / single-SHA cells (Tasks 3-7)
- ✅ Wired into collector (Task 8)
- ✅ Skeleton-over-stale preserved (no cache covers Size or Changes — both still
  stream)
- ✅ Integration test (Task 9)
- ✅ Docs (Task 10)
- ✅ Final verification (Task 11)

**Placeholder scan:** No "TBD"; tests/code shown in full; "exact line numbers
may shift, search for X" used where the file is large and current numbers may
drift between today and execution — paired with concrete `grep` commands so the
engineer can locate the right spot.

**Type consistency:** `cached_*` fns mirror their underlying `get_*` return
types exactly:

- `cached_base_ahead_behind` → `Option<(usize, usize)>` (matches
  `get_ahead_behind`)
- `cached_remote_ahead_behind` → `Option<(usize, usize)>` (matches
  `get_upstream_ahead_behind`)
- `cached_last_commit` → `(Option<i64>, Option<String>, String)` (matches
  `get_commit_metadata`)
- `cached_base_lines` → `(Option<usize>, Option<usize>)` (matches
  `get_base_line_counts`)
- `cached_remote_lines` → `(Option<usize>, Option<usize>)` (matches
  `get_remote_line_counts`)

This means call-site changes are 1-line replacements wrapped in a closure — no
shape conversion.
