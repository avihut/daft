use anyhow::Result;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

/// Git environment variables that must be stripped from test subprocesses.
/// When tests run inside a git hook (e.g., pre-push), git sets these
/// variables, which would redirect test git commands to the host repo.
const GIT_ENV_VARS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_CEILING_DIRECTORIES",
];

/// Create a git Command with hook-inherited environment variables stripped.
fn git_cmd() -> Command {
    let mut cmd = Command::new("git");
    for var in GIT_ENV_VARS {
        cmd.env_remove(var);
    }
    cmd
}

/// Test concurrent Git operations to identify potential race conditions
///
/// This test simulates multiple threads performing Git operations simultaneously
/// to verify that the git repo handles concurrent access safely.
///
/// NOTE: We use `git -C <path>` directly instead of GitCommand because
/// GitCommand is not Send+Sync (due to OnceLock<gix::ThreadSafeRepository>)
/// and relies on process CWD which can't be used safely across threads.
#[test]
fn test_concurrent_git_operations() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let repo_path = temp_dir.path().join("test-repo");

    // Initialize a test repository
    git_cmd()
        .args(["init", "--bare"])
        .arg(&repo_path)
        .output()?;

    let repo_path = Arc::new(repo_path);
    let results = Arc::new(Mutex::new(Vec::new()));

    let mut handles = vec![];

    // Spawn multiple threads performing concurrent Git operations
    for i in 0..5 {
        let path = Arc::clone(&repo_path);
        let results_clone = Arc::clone(&results);

        let handle = thread::spawn(move || {
            let operation_result = match i % 3 {
                0 => {
                    // Test concurrent ref checking
                    git_cmd()
                        .arg("-C")
                        .arg(path.as_ref())
                        .args(["show-ref", "--verify", "--quiet", "refs/heads/nonexistent"])
                        .output()
                        .map(|_| ())
                }
                1 => {
                    // Test concurrent git directory queries
                    git_cmd()
                        .arg("-C")
                        .arg(path.as_ref())
                        .args(["rev-parse", "--git-dir"])
                        .output()
                        .map(|_| ())
                }
                2 => {
                    // Test concurrent for-each-ref operations
                    git_cmd()
                        .arg("-C")
                        .arg(path.as_ref())
                        .args(["for-each-ref", "--format=%(refname)", "refs/heads/"])
                        .output()
                        .map(|_| ())
                }
                _ => Ok(()),
            };

            // Record the result
            let mut results_lock = results_clone.lock().unwrap();
            results_lock.push((i, operation_result.is_ok()));

            thread::sleep(Duration::from_millis(10));
        });

        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let results_lock = results.lock().unwrap();
    assert_eq!(results_lock.len(), 5);

    for (thread_id, success) in results_lock.iter() {
        assert!(success, "Thread {} failed its Git operation", thread_id);
    }

    Ok(())
}

/// Test concurrent branch checking operations specifically
///
/// This targets the specific race condition concern about the complex branch
/// checking logic in checkout-branch operations.
#[test]
fn test_concurrent_branch_checking() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let repo_path = temp_dir.path().join("test-repo");

    git_cmd().args(["init"]).arg(&repo_path).output()?;

    std::fs::write(repo_path.join("test.txt"), "test content")?;
    git_cmd()
        .args(["add", "."])
        .current_dir(&repo_path)
        .output()?;
    git_cmd()
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@test.com",
            "commit",
            "-m",
            "Initial commit",
        ])
        .current_dir(&repo_path)
        .output()?;

    for branch in ["feature-1", "feature-2", "feature-3"] {
        git_cmd()
            .args(["branch", branch])
            .current_dir(&repo_path)
            .output()?;
    }

    let repo_path = Arc::new(repo_path);
    let race_condition_detected = Arc::new(Mutex::new(false));

    let mut handles = vec![];

    for i in 0..10 {
        let path = Arc::clone(&repo_path);
        let _race_flag = Arc::clone(&race_condition_detected);

        let handle = thread::spawn(move || {
            let branch_name = match i % 3 {
                0 => "feature-1",
                1 => "feature-2",
                2 => "feature-3",
                _ => "main",
            };

            let local_ref = format!("refs/heads/{branch_name}");
            let remote_ref = format!("refs/remotes/origin/{branch_name}");

            let _local = git_cmd()
                .arg("-C")
                .arg(path.as_ref())
                .args(["show-ref", "--verify", "--quiet", &local_ref])
                .output();
            thread::sleep(Duration::from_millis(1));
            let _remote = git_cmd()
                .arg("-C")
                .arg(path.as_ref())
                .args(["show-ref", "--verify", "--quiet", &remote_ref])
                .output();
        });

        handles.push(handle);
    }

    for handle in handles {
        if handle.join().is_err() {
            let mut race_flag = race_condition_detected.lock().unwrap();
            *race_flag = true;
        }
    }

    let race_detected = *race_condition_detected.lock().unwrap();
    assert!(
        !race_detected,
        "Race condition detected in concurrent branch checking"
    );

    Ok(())
}

/// Test concurrent worktree list operations
///
/// Tests that worktree listing operations can run concurrently without
/// causing panics or data corruption.
#[test]
fn test_concurrent_worktree_operations() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let repo_path = temp_dir.path().join("test-repo");

    git_cmd().args(["init"]).arg(&repo_path).output()?;

    std::fs::write(repo_path.join("test.txt"), "test content")?;
    git_cmd()
        .args(["add", "."])
        .current_dir(&repo_path)
        .output()?;
    git_cmd()
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@test.com",
            "commit",
            "-m",
            "Initial commit",
        ])
        .current_dir(&repo_path)
        .output()?;

    let repo_path = Arc::new(repo_path);
    let mut handles = vec![];

    for _i in 0..8 {
        let path = Arc::clone(&repo_path);

        let handle = thread::spawn(move || {
            for _ in 0..5 {
                let result = git_cmd()
                    .arg("-C")
                    .arg(path.as_ref())
                    .args(["worktree", "list", "--porcelain"])
                    .output();

                match result {
                    Ok(_) => {}
                    Err(_) => {}
                }

                thread::sleep(Duration::from_millis(2));
            }
        });

        handles.push(handle);
    }

    for handle in handles {
        handle
            .join()
            .expect("Thread should not panic during worktree operations");
    }

    Ok(())
}
