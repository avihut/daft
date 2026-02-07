//! Gitoxide-based implementations of git operations.
//!
//! Each function provides a native Rust alternative to a git subprocess call.
//! These are called from `GitCommand` methods when `daft.experimental.gitoxide`
//! is enabled.

use anyhow::{Context, Result};
use gix::bstr::ByteSlice;
use gix::remote::Direction;
use gix::Repository;
use std::path::PathBuf;

// --- Group 1: Repository Discovery & State ---

/// gitoxide equivalent of `git rev-parse --git-common-dir`
pub fn rev_parse_git_common_dir(repo: &Repository) -> Result<String> {
    let common_dir = repo.common_dir();
    common_dir
        .to_str()
        .map(|s| s.to_string())
        .context("Common dir path is not valid UTF-8")
}

/// gitoxide equivalent of `git rev-parse --git-dir` (success = inside a repo)
pub fn is_inside_git_repo() -> Result<bool> {
    let cwd = std::env::current_dir().context("Failed to get current working directory")?;
    Ok(gix::discover(&cwd).is_ok())
}

/// gitoxide equivalent of `git rev-parse --is-inside-work-tree`
pub fn rev_parse_is_inside_work_tree(repo: &Repository) -> Result<bool> {
    Ok(!repo.is_bare() && repo.workdir().is_some())
}

/// gitoxide equivalent of `git rev-parse --is-bare-repository`
pub fn rev_parse_is_bare_repository(repo: &Repository) -> Result<bool> {
    Ok(repo.is_bare())
}

/// gitoxide equivalent of `git rev-parse --git-dir`
pub fn get_git_dir(repo: &Repository) -> Result<String> {
    let git_dir = repo.git_dir();
    git_dir
        .to_str()
        .map(|s| s.to_string())
        .context("Git dir path is not valid UTF-8")
}

/// gitoxide equivalent of `git rev-parse --show-toplevel`
pub fn get_current_worktree_path(repo: &Repository) -> Result<PathBuf> {
    repo.workdir()
        .map(|p| p.to_path_buf())
        .context("Not inside a worktree (bare repository)")
}

// --- Group 2: References & Branches ---

/// gitoxide equivalent of `git symbolic-ref --short HEAD`
pub fn symbolic_ref_short_head(repo: &Repository) -> Result<String> {
    let head = repo.head_ref().context("Failed to read HEAD")?;
    match head {
        Some(reference) => {
            let short = reference.name().shorten().to_string();
            Ok(short)
        }
        None => {
            anyhow::bail!("HEAD is detached or unborn");
        }
    }
}

/// gitoxide equivalent of `git show-ref --verify --quiet <ref_name>`
pub fn show_ref_exists(repo: &Repository, ref_name: &str) -> Result<bool> {
    Ok(repo.try_find_reference(ref_name)?.is_some())
}

/// gitoxide equivalent of `git for-each-ref --format=<format> <refs>`
///
/// Supports format strings containing:
/// - `%(refname:short)` - short reference name
/// - `%(refname)` - full reference name
/// - `%(objectname)` - object hash
///
/// Other format specifiers are passed through literally.
pub fn for_each_ref(repo: &Repository, format: &str, refs_prefix: &str) -> Result<String> {
    let platform = repo.references()?;
    let references = platform.prefixed(refs_prefix)?;
    let mut output = String::new();

    for reference_result in references {
        let reference =
            reference_result.map_err(|e| anyhow::anyhow!("Failed to read reference: {e}"))?;
        let full_name = reference.name().as_bstr().to_string();
        let short_name = reference.name().shorten().to_string();
        let oid = match reference.try_id() {
            Some(id) => id.to_string(),
            None => {
                // Symbolic ref - try to peel
                let mut peelable = reference;
                match peelable.peel_to_id_in_place() {
                    Ok(id) => id.to_string(),
                    Err(_) => String::new(),
                }
            }
        };

        let line = format
            .replace("%(refname:short)", &short_name)
            .replace("%(refname)", &full_name)
            .replace("%(objectname)", &oid);
        output.push_str(&line);
        output.push('\n');
    }

    Ok(output)
}

/// gitoxide equivalent of `git branch -vv`
///
/// Returns output similar to `git branch -vv`, with tracking information.
pub fn branch_list_verbose(repo: &Repository) -> Result<String> {
    let platform = repo.references()?;
    let references = platform.prefixed("refs/heads/")?;
    let mut output = String::new();

    // Get current branch name for the * marker
    let current_branch = repo
        .head_ref()
        .ok()
        .flatten()
        .map(|r| r.name().shorten().to_string());

    for reference_result in references {
        let mut reference =
            reference_result.map_err(|e| anyhow::anyhow!("Failed to read reference: {e}"))?;
        let branch_name = reference.name().shorten().to_string();
        let is_current = current_branch.as_deref() == Some(&branch_name);
        let marker = if is_current { '*' } else { ' ' };

        let oid = match reference.peel_to_id_in_place() {
            Ok(id) => id.to_hex().to_string(),
            Err(_) => "?".repeat(7),
        };
        let short_oid = if oid.len() > 7 { &oid[..7] } else { &oid };

        // Try to get tracking info
        let remote_key = format!("branch.{branch_name}.remote");
        let tracking = repo
            .config_snapshot()
            .string(&remote_key)
            .map(|remote| {
                let merge_key = format!("branch.{branch_name}.merge");
                let merge = repo
                    .config_snapshot()
                    .string(&merge_key)
                    .map(|m| {
                        let m_str = m.to_string();
                        m_str
                            .strip_prefix("refs/heads/")
                            .unwrap_or(&m_str)
                            .to_string()
                    })
                    .unwrap_or_default();
                format!("[{remote}/{merge}]")
            })
            .unwrap_or_default();

        let line = format!("{marker} {branch_name:<20} {short_oid} {tracking}\n");
        output.push_str(&line);
    }

    Ok(output)
}

// --- Group 3: Config Reading ---

/// gitoxide equivalent of `git config --get <key>`
pub fn config_get(repo: &Repository, key: &str) -> Result<Option<String>> {
    let config = repo.config_snapshot();
    Ok(config.string(key).map(|v| v.to_string()))
}

/// gitoxide equivalent of `git config --global --get <key>`
///
/// Opens a standalone repository to read global config only.
pub fn config_get_global(key: &str) -> Result<Option<String>> {
    // Use git's global config by opening config from environment
    // This reads ~/.gitconfig and XDG config
    let config = gix::config::File::from_globals().context("Failed to read global git config")?;
    // gix::config::File::string() takes key as impl AsKey
    Ok(config.string(key).map(|v| v.to_string()))
}

// --- Group 4: Status & Commit Info ---

/// gitoxide equivalent of `git status --porcelain` (checking for non-empty output)
pub fn has_uncommitted_changes(repo: &Repository) -> Result<bool> {
    // Use gix status to check for any changes
    let status = repo
        .status(gix::progress::Discard)
        .context("Failed to get repository status")?;
    let mut iter = status
        .into_index_worktree_iter(Vec::<gix::bstr::BString>::new())
        .context("Failed to iterate status")?;

    // If there's at least one item, there are uncommitted changes
    if let Some(item) = iter.next() {
        let _item = item.context("Failed to read status item")?;
        return Ok(true);
    }

    Ok(false)
}

/// gitoxide equivalent of `git rev-list --count <range>`
///
/// Supports ranges like "A..B" and "A...B".
pub fn rev_list_count(repo: &Repository, range: &str) -> Result<u32> {
    // Parse the range - could be "A..B", "A...B", or a single ref
    if let Some((from_str, to_str)) = range.split_once("..") {
        let to_str = to_str.strip_prefix('.').unwrap_or(to_str);

        let to_id = repo
            .rev_parse_single(to_str.as_bytes())
            .with_context(|| format!("Failed to resolve '{to_str}'"))?;

        let from_id = repo
            .rev_parse_single(from_str.as_bytes())
            .with_context(|| format!("Failed to resolve '{from_str}'"))?;

        let mut count: u32 = 0;
        let walk = repo
            .rev_walk([to_id.detach()])
            .all()
            .context("Failed to start revision walk")?;

        let from_oid = from_id.detach();
        for info_result in walk {
            let info = info_result.context("Failed during revision walk")?;
            if info.id == from_oid {
                break;
            }
            count += 1;
        }

        Ok(count)
    } else {
        // Single ref - count all ancestors
        let id = repo
            .rev_parse_single(range.as_bytes())
            .with_context(|| format!("Failed to resolve '{range}'"))?;

        let mut count: u32 = 0;
        let walk = repo
            .rev_walk([id.detach()])
            .all()
            .context("Failed to start revision walk")?;

        for info_result in walk {
            let _info = info_result.context("Failed during revision walk")?;
            count += 1;
        }

        Ok(count)
    }
}

// --- Group 5: Remote Info (local data) ---

/// gitoxide equivalent of `git remote`
pub fn remote_list(repo: &Repository) -> Result<Vec<String>> {
    Ok(repo
        .remote_names()
        .iter()
        .map(|name| name.to_string())
        .collect())
}

/// gitoxide equivalent of `git remote get-url <remote>`
pub fn remote_get_url(repo: &Repository, remote_name: &str) -> Result<String> {
    let remote = repo
        .find_remote(remote_name)
        .with_context(|| format!("Remote '{remote_name}' not found"))?;
    let url = remote
        .url(Direction::Fetch)
        .context("Remote has no fetch URL")?;
    Ok(url.to_bstring().to_string())
}

// --- Group 6: Remote Network ---
//
// NOTE: These functions require a real, discovered Repository — they cannot
// work with an ephemeral bare repo because gitoxide's `ref_map()` does not
// properly negotiate refs with anonymous remotes on freshly-initialized repos.
// When no local repo exists (e.g. during clone), the callers in git.rs fall
// through to the git CLI subprocess path instead.

/// gitoxide equivalent of `git ls-remote --symref <remote_url> HEAD`
///
/// Returns output formatted like git's ls-remote --symref output:
/// ```text
/// ref: refs/heads/main\tHEAD
/// <oid>\tHEAD
/// ```
pub fn ls_remote_symref(repo: &Repository, remote_url: &str) -> Result<String> {
    let remote = repo
        .remote_at(remote_url)
        .context("Failed to create remote")?;

    let connection = remote
        .connect(Direction::Fetch)
        .context("Failed to connect to remote")?;

    let (ref_map, _outcome) = connection
        .ref_map(gix::progress::Discard, Default::default())
        .context("Failed to get ref map from remote")?;

    let mut output = String::new();

    for remote_ref in &ref_map.remote_refs {
        match remote_ref {
            gix::protocol::handshake::Ref::Symbolic {
                full_ref_name,
                target,
                object,
                ..
            } => {
                if full_ref_name.as_bstr() == "HEAD" {
                    output.push_str(&format!("ref: {target}\tHEAD\n"));
                    output.push_str(&format!("{object}\tHEAD\n"));
                }
            }
            gix::protocol::handshake::Ref::Direct {
                full_ref_name,
                object,
            } => {
                if full_ref_name.as_bstr() == "HEAD" {
                    output.push_str(&format!("{object}\tHEAD\n"));
                }
            }
            _ => {}
        }
    }

    Ok(output)
}

/// gitoxide equivalent of `git ls-remote --heads <remote> [refs/heads/<branch>]`
///
/// Returns output formatted like git's ls-remote output:
/// ```text
/// <oid>\trefs/heads/branch-name
/// ```
pub fn ls_remote_heads(repo: &Repository, remote: &str, branch: Option<&str>) -> Result<String> {
    // Try to find a configured remote first, then fall back to URL
    let remote_obj = match repo.try_find_remote(remote) {
        Some(Ok(r)) => r,
        _ => repo.remote_at(remote).context("Failed to create remote")?,
    };

    let connection = remote_obj
        .connect(Direction::Fetch)
        .context("Failed to connect to remote")?;

    let (ref_map, _outcome) = connection
        .ref_map(gix::progress::Discard, Default::default())
        .context("Failed to get ref map from remote")?;

    let mut output = String::new();

    let filter_ref = branch.map(|b| format!("refs/heads/{b}"));

    for remote_ref in &ref_map.remote_refs {
        let (name, oid) = match remote_ref {
            gix::protocol::handshake::Ref::Direct {
                full_ref_name,
                object,
            } => (full_ref_name.to_string(), object.to_string()),
            gix::protocol::handshake::Ref::Symbolic {
                full_ref_name,
                object,
                ..
            } => (full_ref_name.to_string(), object.to_string()),
            gix::protocol::handshake::Ref::Peeled {
                full_ref_name, tag, ..
            } => (full_ref_name.to_string(), tag.to_string()),
            gix::protocol::handshake::Ref::Unborn {
                full_ref_name,
                target,
            } => (full_ref_name.to_string(), target.to_string()),
        };

        if !name.starts_with("refs/heads/") {
            continue;
        }

        if let Some(ref filter) = filter_ref {
            if name != *filter {
                continue;
            }
        }

        output.push_str(&format!("{oid}\t{name}\n"));
    }

    Ok(output)
}

/// gitoxide equivalent of `git ls-remote --heads <remote> refs/heads/<branch>`
/// Returns true if the branch exists on the remote.
pub fn ls_remote_branch_exists(repo: &Repository, remote_name: &str, branch: &str) -> Result<bool> {
    let output = ls_remote_heads(repo, remote_name, Some(branch))?;
    Ok(!output.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::process::Command;
    use tempfile::tempdir;

    /// Helper to create a test repo with a commit.
    /// Sets CWD to the temp dir before opening with gix, because gix::open
    /// internally calls std::env::current_dir() which can fail if other tests
    /// (e.g., init tests) have changed CWD to a since-deleted temp directory.
    fn create_test_repo() -> (tempfile::TempDir, Repository) {
        let dir = tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap();

        // Set CWD to the temp dir so gix::open can resolve current_dir()
        std::env::set_current_dir(&path).unwrap();

        // Initialize with git CLI for reliable setup
        Command::new("git")
            .args(["init", "-b", "main"])
            .arg(&path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(&path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(&path)
            .output()
            .unwrap();

        // Create initial commit
        std::fs::write(path.join("file.txt"), "hello").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(&path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(&path)
            .output()
            .unwrap();

        let repo = gix::open(&path).unwrap();
        (dir, repo)
    }

    #[test]
    #[serial]
    fn test_rev_parse_git_common_dir() {
        let (_dir, repo) = create_test_repo();
        let result = rev_parse_git_common_dir(&repo).unwrap();
        assert!(result.ends_with(".git"));
    }

    #[test]
    #[serial]
    fn test_is_inside_git_repo() {
        // Create a test repo and set CWD to it, then verify detection works
        let (_dir, _repo) = create_test_repo();
        let result = is_inside_git_repo().unwrap();
        assert!(result);
    }

    #[test]
    #[serial]
    fn test_rev_parse_is_inside_work_tree() {
        let (_dir, repo) = create_test_repo();
        assert!(rev_parse_is_inside_work_tree(&repo).unwrap());
    }

    #[test]
    #[serial]
    fn test_rev_parse_is_bare_repository() {
        let (_dir, repo) = create_test_repo();
        assert!(!rev_parse_is_bare_repository(&repo).unwrap());
    }

    #[test]
    #[serial]
    fn test_get_git_dir() {
        let (_dir, repo) = create_test_repo();
        let result = get_git_dir(&repo).unwrap();
        assert!(result.ends_with(".git"));
    }

    #[test]
    #[serial]
    fn test_get_current_worktree_path() {
        let (dir, repo) = create_test_repo();
        let result = get_current_worktree_path(&repo).unwrap();
        // On macOS, /var is a symlink to /private/var, so we canonicalize both sides
        let expected = dir.path().canonicalize().unwrap();
        let actual = result.canonicalize().unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    #[serial]
    fn test_symbolic_ref_short_head() {
        let (_dir, repo) = create_test_repo();
        let result = symbolic_ref_short_head(&repo).unwrap();
        assert_eq!(result, "main");
    }

    #[test]
    #[serial]
    fn test_show_ref_exists() {
        let (_dir, repo) = create_test_repo();
        assert!(show_ref_exists(&repo, "refs/heads/main").unwrap());
        assert!(!show_ref_exists(&repo, "refs/heads/nonexistent").unwrap());
    }

    #[test]
    #[serial]
    fn test_for_each_ref() {
        let (_dir, repo) = create_test_repo();
        let result = for_each_ref(&repo, "%(refname:short)", "refs/heads/").unwrap();
        assert!(result.contains("main"));
    }

    #[test]
    #[serial]
    fn test_config_get() {
        let (_dir, repo) = create_test_repo();
        let result = config_get(&repo, "user.email").unwrap();
        assert_eq!(result, Some("test@test.com".to_string()));

        let result = config_get(&repo, "nonexistent.key").unwrap();
        assert!(result.is_none());
    }

    #[test]
    #[serial]
    fn test_has_uncommitted_changes_clean() {
        let (_dir, repo) = create_test_repo();
        let result = has_uncommitted_changes(&repo).unwrap();
        assert!(!result);
    }

    #[test]
    #[serial]
    fn test_has_uncommitted_changes_dirty() {
        let (dir, repo) = create_test_repo();
        // Modify a tracked file
        std::fs::write(dir.path().join("file.txt"), "modified").unwrap();
        let result = has_uncommitted_changes(&repo).unwrap();
        assert!(result);
    }

    #[test]
    #[serial]
    fn test_remote_list_empty() {
        let (_dir, repo) = create_test_repo();
        let result = remote_list(&repo).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    #[serial]
    fn test_worktree_path_matches_git_in_bare_layout() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();

        // Create bare repo layout like daft does
        let git_dir = root.join(".git");
        Command::new("git")
            .args(["init", "--bare"])
            .arg(&git_dir)
            .output()
            .unwrap();

        // Add main worktree
        let main_wt = root.join("main");
        Command::new("git")
            .args(["worktree", "add", "--orphan", "-b", "main"])
            .arg(&main_wt)
            .current_dir(&git_dir)
            .output()
            .unwrap();

        // Create initial commit
        std::fs::write(main_wt.join("file.txt"), "hello").unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(&main_wt)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(&main_wt)
            .output()
            .unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(&main_wt)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(&main_wt)
            .output()
            .unwrap();

        // Add linked worktree
        let test_wt = root.join("test-1");
        Command::new("git")
            .args(["worktree", "add", "-b", "test-1"])
            .arg(&test_wt)
            .arg("main")
            .current_dir(&git_dir)
            .output()
            .unwrap();

        // Set CWD to the linked worktree and discover with gix
        // Using current_dir() (absolute) rather than "." to ensure absolute paths
        std::env::set_current_dir(&test_wt).unwrap();
        let cwd = std::env::current_dir().unwrap();
        let repo = gix::ThreadSafeRepository::discover(&cwd)
            .unwrap()
            .to_thread_local();
        let gix_workdir = repo.workdir().unwrap().to_path_buf();

        // Get path from git rev-parse --show-toplevel
        let git_output = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(&test_wt)
            .output()
            .unwrap();
        let git_toplevel = PathBuf::from(String::from_utf8(git_output.stdout).unwrap().trim());

        // Get path from git worktree list --porcelain
        let wt_list_output = Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(&test_wt)
            .output()
            .unwrap();
        let wt_list_str = String::from_utf8(wt_list_output.stdout).unwrap();
        let porcelain_path = wt_list_str
            .lines()
            .find(|l| l.starts_with("worktree ") && l.contains("test-1"))
            .map(|l| PathBuf::from(l.strip_prefix("worktree ").unwrap()))
            .unwrap();

        eprintln!("gix workdir:          {:?}", gix_workdir);
        eprintln!("git --show-toplevel:  {:?}", git_toplevel);
        eprintln!("git worktree list:    {:?}", porcelain_path);

        assert_eq!(
            gix_workdir, git_toplevel,
            "gix workdir doesn't match git show-toplevel"
        );
        assert_eq!(
            gix_workdir, porcelain_path,
            "gix workdir doesn't match git worktree list porcelain"
        );
    }

    #[test]
    #[serial]
    fn test_rev_list_count() {
        let (dir, _repo) = create_test_repo();
        // Add another commit
        std::fs::write(dir.path().join("file2.txt"), "hello2").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "second"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        // Re-open to see new commit
        let path = dir.path().canonicalize().unwrap();
        std::env::set_current_dir(&path).unwrap();
        let repo = gix::open(&path).unwrap();
        let count = rev_list_count(&repo, "HEAD").unwrap();
        assert_eq!(count, 2);
    }
}
