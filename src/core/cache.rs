//! On-disk JSON cache primitives shared by per-cell cache wrappers.
//!
//! Lives under `<git-common-dir>/.daft/cache/<kind>/`. Each kind owns its
//! filename scheme (typically `{sha1}-{sha2}.json` for content-addressed pair
//! caches) and its struct shape. This module owns only the filesystem
//! mechanics so torn-write semantics and the silent-failure write policy
//! live in one place.
//!
//! # Torn-write semantics
//!
//! Writes use `fs::write` directly, not temp-file-plus-rename. A crash mid-
//! write produces a truncated file at the expected path, which `read_json`
//! rejects as corrupt JSON — indistinguishable from a cache miss.
//!
//! # Error policy
//!
//! - `read_json` returns `None` on any failure (missing file, I/O error,
//!   corrupt JSON).
//! - `write_json` degrades silently — a failed write means the next read
//!   recomputes.
//! - `clear_kind` propagates I/O errors so a future `daft cache clear`
//!   command could report failures.

use serde::{Serialize, de::DeserializeOwned};
use std::fs;
use std::path::{Path, PathBuf};

/// The directory for a named cache kind under the given git common dir.
pub fn cache_dir_for(git_common_dir: &Path, kind: &str) -> PathBuf {
    git_common_dir.join(".daft").join("cache").join(kind)
}

/// Read and deserialize a JSON cache entry. Returns `None` on any failure.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let json = fs::read_to_string(path).ok()?;
    serde_json::from_str::<T>(&json).ok()
}

/// Serialize and write a JSON cache entry, creating parent dirs as needed.
/// Degrades silently on any failure.
pub fn write_json<T: Serialize>(path: &Path, value: &T) {
    if let Some(parent) = path.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return;
    }
    let Ok(json) = serde_json::to_string(value) else {
        return;
    };
    let _ = fs::write(path, json);
}

/// Delete every regular file in the cache kind's directory. Errors propagate
/// so callers can report partial failures. Missing directories are a no-op.
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
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
        let v = Foo {
            n: 7,
            s: "hello".into(),
        };
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
        write_json(
            &path,
            &Foo {
                n: 1,
                s: "x".into(),
            },
        );
        assert!(path.exists());
    }

    #[test]
    fn cache_dir_uses_kind_under_git_common() {
        let dir = TempDir::new().unwrap();
        let p = cache_dir_for(dir.path(), "ahead-behind");
        assert_eq!(
            p,
            dir.path().join(".daft").join("cache").join("ahead-behind")
        );
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

    #[test]
    fn clear_kind_is_noop_when_dir_missing() {
        let dir = TempDir::new().unwrap();
        // Don't create the kind dir at all.
        assert!(clear_kind(dir.path(), "nonexistent").is_ok());
    }
}
