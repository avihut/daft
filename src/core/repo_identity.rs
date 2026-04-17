//! Repository identity management for the log store and coordinator socket.
//!
//! Every repo that daft touches is assigned a stable UUID v7 stored at
//! `<git-common-dir>/daft-id`. This ID keys the on-disk log store and
//! coordinator socket, so it survives repo moves and is destroyed when the
//! repo itself is deleted. Re-cloning at the same path produces a fresh
//! identity and a clean log-store view.

use anyhow::{Context, Result};
use std::io::{ErrorKind, Read, Write};
use std::path::Path;
use uuid::Uuid;

const IDENTITY_FILE: &str = "daft-id";

pub fn compute_repo_id() -> Result<String> {
    let git_common_dir = crate::core::repo::get_git_common_dir()
        .context("Could not determine git common dir. Are you inside a git repository?")?;
    compute_repo_id_from_common_dir(&git_common_dir)
}

pub fn compute_repo_id_from_common_dir(git_common_dir: &Path) -> Result<String> {
    let id_path = git_common_dir.join(IDENTITY_FILE);
    loop {
        if let Some(id) = read_existing_id(&id_path)? {
            return Ok(id);
        }
        if let Some(id) = try_create_new(&id_path)? {
            return Ok(id);
        }
        // Raced with another process — loop back and read what they wrote.
    }
}

fn read_existing_id(path: &Path) -> Result<Option<String>> {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("Failed to open {}", path.display()));
        }
    };
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        // Empty file means a prior write crashed mid-flight. Remove it so the
        // next call to try_create_new can claim the path with create_new(true).
        drop(file);
        std::fs::remove_file(path)
            .or_else(|e| {
                if e.kind() == ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(e)
                }
            })
            .with_context(|| format!("Failed to remove empty identity file {}", path.display()))?;
        return Ok(None);
    }
    match Uuid::parse_str(trimmed) {
        Ok(uuid) => Ok(Some(uuid.hyphenated().to_string())),
        Err(_) => anyhow::bail!(
            "Corrupt repo identity file at {}. Delete it to regenerate \
             (this will orphan existing job logs for this repo).",
            path.display()
        ),
    }
}

fn try_create_new(path: &Path) -> Result<Option<String>> {
    let uuid = Uuid::now_v7();
    let s = uuid.hyphenated().to_string();

    let parent = path
        .parent()
        .context("Repo identity path has no parent directory")?;

    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("Failed to create temp file in {}", parent.display()))?;
    tmp.write_all(s.as_bytes())
        .with_context(|| "Failed to write UUID to temp file")?;

    match tmp.persist_noclobber(path) {
        Ok(_) => Ok(Some(s)),
        Err(e) if e.error.kind() == ErrorKind::AlreadyExists => Ok(None),
        Err(e) => Err(anyhow::Error::from(e.error))
            .with_context(|| format!("Failed to persist {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn creates_file_when_absent() {
        let tmp = TempDir::new().unwrap();
        let id = compute_repo_id_from_common_dir(tmp.path()).unwrap();
        assert!(tmp.path().join(IDENTITY_FILE).exists());
        assert_eq!(Uuid::parse_str(&id).unwrap().hyphenated().to_string(), id);
    }

    #[test]
    fn reuses_existing_file() {
        let tmp = TempDir::new().unwrap();
        let first = compute_repo_id_from_common_dir(tmp.path()).unwrap();
        let second = compute_repo_id_from_common_dir(tmp.path()).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn generated_id_is_version_7() {
        let tmp = TempDir::new().unwrap();
        let id = compute_repo_id_from_common_dir(tmp.path()).unwrap();
        let uuid = Uuid::parse_str(&id).unwrap();
        assert_eq!(uuid.get_version_num(), 7);
    }

    #[test]
    fn distinct_common_dirs_yield_distinct_ids() {
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        let id_a = compute_repo_id_from_common_dir(a.path()).unwrap();
        let id_b = compute_repo_id_from_common_dir(b.path()).unwrap();
        assert_ne!(id_a, id_b);
    }

    #[test]
    fn empty_file_is_overwritten() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(IDENTITY_FILE), "").unwrap();
        let id = compute_repo_id_from_common_dir(tmp.path()).unwrap();
        assert!(!id.is_empty());
        assert_eq!(Uuid::parse_str(&id).unwrap().get_version_num(), 7);
    }

    #[test]
    fn corrupt_contents_produce_error() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(IDENTITY_FILE), "not-a-uuid").unwrap();
        let result = compute_repo_id_from_common_dir(tmp.path());
        assert!(result.is_err());
        let msg = format!("{:#}", result.unwrap_err());
        assert!(
            msg.contains("Corrupt repo identity"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn concurrent_creation_converges_on_single_id() {
        use std::sync::Arc;
        use std::thread;
        for iteration in 0..32 {
            let tmp = Arc::new(TempDir::new().unwrap());
            let handles: Vec<_> = (0..32)
                .map(|_| {
                    let tmp_clone = Arc::clone(&tmp);
                    thread::spawn(move || {
                        compute_repo_id_from_common_dir(tmp_clone.path()).unwrap()
                    })
                })
                .collect();
            let ids: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            let unique: std::collections::HashSet<_> = ids.iter().collect();
            assert_eq!(
                unique.len(),
                1,
                "iteration {iteration}: concurrent calls disagreed: {ids:?}"
            );
        }
    }
}
