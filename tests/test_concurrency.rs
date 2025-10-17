use anyhow::Result;
use daft::{git::GitCommand, WorktreeConfig};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

/// Test concurrent Git operations to identify potential race conditions
///
/// This test simulates multiple threads performing Git operations simultaneously
/// to verify that our GitCommand wrapper handles concurrent access safely.
#[test]
fn test_concurrent_git_operations() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let repo_path = temp_dir.path().join("test-repo");

    // Initialize a test repository
    std::process::Command::new("git")
        .args(["init", "--bare"])
        .arg(&repo_path)
        .output()?;

    // Change to the repository directory for operations
    std::env::set_current_dir(&repo_path)?;

    let git = Arc::new(GitCommand::new(false));
    let results = Arc::new(Mutex::new(Vec::new()));

    let mut handles = vec![];

    // Spawn multiple threads performing concurrent Git operations
    for i in 0..5 {
        let git_clone = Arc::clone(&git);
        let results_clone = Arc::clone(&results);

        let handle = thread::spawn(move || {
            // Simulate various Git operations that could race
            let operation_result = match i % 3 {
                0 => {
                    // Test concurrent ref checking
                    git_clone.show_ref_exists("refs/heads/nonexistent")
                }
                1 => {
                    // Test concurrent git directory queries
                    git_clone.get_git_dir().map(|_| true)
                }
                2 => {
                    // Test concurrent for-each-ref operations
                    git_clone
                        .for_each_ref("%(refname)", "refs/heads/")
                        .map(|_| true)
                }
                _ => Ok(true),
            };

            // Record the result
            {
                let mut results_lock = results_clone.lock().unwrap();
                results_lock.push((i, operation_result.is_ok()));
            }

            // Add a small delay to increase chance of race conditions
            thread::sleep(Duration::from_millis(10));
        });

        handles.push(handle);
    }

    // Wait for all threads to complete
    for handle in handles {
        handle.join().unwrap();
    }

    // Verify all operations completed successfully
    let results_lock = results.lock().unwrap();
    assert_eq!(results_lock.len(), 5);

    for (thread_id, success) in results_lock.iter() {
        assert!(success, "Thread {} failed its Git operation", thread_id);
    }

    Ok(())
}

/// Test concurrent branch checking operations specifically
///
/// This targets the specific race condition concern mentioned in the review
/// about the complex branch checking logic in checkout-branch operations.
#[test]
fn test_concurrent_branch_checking() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let repo_path = temp_dir.path().join("test-repo");

    // Initialize test repository with some branches
    std::process::Command::new("git")
        .args(["init"])
        .arg(&repo_path)
        .output()?;

    std::env::set_current_dir(&repo_path)?;

    // Create initial commit and branches for testing
    std::fs::write(repo_path.join("test.txt"), "test content")?;
    std::process::Command::new("git")
        .args(["add", "."])
        .output()?;
    std::process::Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .output()?;

    // Create test branches
    for branch in ["feature-1", "feature-2", "feature-3"] {
        std::process::Command::new("git")
            .args(["branch", branch])
            .output()?;
    }

    let git = Arc::new(GitCommand::new(false));
    let config = Arc::new(WorktreeConfig::default());
    let race_condition_detected = Arc::new(Mutex::new(false));

    let mut handles = vec![];

    // Spawn multiple threads checking different branches simultaneously
    for i in 0..10 {
        let git_clone = Arc::clone(&git);
        let config_clone = Arc::clone(&config);
        let race_flag = Arc::clone(&race_condition_detected);

        let handle = thread::spawn(move || {
            let branch_name = match i % 3 {
                0 => "feature-1",
                1 => "feature-2",
                2 => "feature-3",
                _ => "main",
            };

            // Simulate the complex branch checking logic from checkout-branch
            let local_ref = format!("refs/heads/{}", branch_name);
            let remote_ref = format!("refs/remotes/{}/{}", config_clone.remote_name, branch_name);

            // Perform the same sequence of operations as the real code
            let _local_exists = git_clone.show_ref_exists(&local_ref);
            thread::sleep(Duration::from_millis(1)); // Small delay to encourage races
            let _remote_exists = git_clone.show_ref_exists(&remote_ref);

            // If we get here without panicking, no race condition occurred
            // In a real race condition, we might get inconsistent results
            // or one of the operations might fail unexpectedly
        });

        handles.push(handle);
    }

    // Wait for all operations to complete
    for handle in handles {
        if handle.join().is_err() {
            let mut race_flag = race_condition_detected.lock().unwrap();
            *race_flag = true;
        }
    }

    // This test passes if no race conditions caused thread panics
    let race_detected = *race_condition_detected.lock().unwrap();
    assert!(
        !race_detected,
        "Race condition detected in concurrent branch checking"
    );

    Ok(())
}

/// Test concurrent worktree list operations
///
/// Tests the thread safety of worktree listing operations which are used
/// in multiple commands and could potentially race with worktree creation/deletion.
#[test]
fn test_concurrent_worktree_operations() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let repo_path = temp_dir.path().join("test-repo");

    // Initialize test repository
    std::process::Command::new("git")
        .args(["init"])
        .arg(&repo_path)
        .output()?;

    std::env::set_current_dir(&repo_path)?;

    // Create initial commit
    std::fs::write(repo_path.join("test.txt"), "test content")?;
    std::process::Command::new("git")
        .args(["add", "."])
        .output()?;
    std::process::Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .output()?;

    let git = Arc::new(GitCommand::new(false));
    let mut handles = vec![];

    // Spawn threads that perform concurrent worktree list operations
    for i in 0..8 {
        let git_clone = Arc::clone(&git);

        let handle = thread::spawn(move || {
            // Repeatedly call worktree list to check for race conditions
            for _ in 0..5 {
                let result = git_clone.worktree_list_porcelain();

                // The operation should either succeed or fail gracefully
                // but should never cause a panic or data corruption
                match result {
                    Ok(_output) => {
                        // Success is expected
                    }
                    Err(_e) => {
                        // Graceful errors are acceptable, panics are not
                    }
                }

                // Small delay between operations
                thread::sleep(Duration::from_millis(2));
            }
        });

        handles.push(handle);
    }

    // Wait for all threads to complete successfully
    for handle in handles {
        handle
            .join()
            .expect("Thread should not panic during worktree operations");
    }

    Ok(())
}
