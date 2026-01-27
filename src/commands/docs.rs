/// Documentation command for `git daft`
///
/// Shows daft commands in git-style help format
use anyhow::Result;
use std::path::Path;

pub fn run() -> Result<()> {
    // Detect how we were invoked
    let program_path = std::env::args()
        .next()
        .unwrap_or_else(|| "daft".to_string());
    let program_name = Path::new(&program_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("daft");

    let via_git = program_name == "git-daft";

    if via_git {
        println!("usage: git worktree-<command> [<args>]");
        println!("   or: git-worktree-<command> [<args>]");
    } else {
        println!("usage: git-worktree-<command> [<args>]");
        println!("   or: git worktree-<command> [<args>]");
    }

    print!(
        r#"
These are common daft commands used in various situations:

start a worktree-based repository
   worktree-clone    Clone a repository into worktree structure
   worktree-init     Initialize a new worktree-based repository

work on branches (each branch gets its own directory)
   worktree-checkout              Check out existing branch into new worktree
   worktree-checkout-branch       Create new branch in new worktree
   worktree-checkout-branch-from-default
                                  Create new branch from default in new worktree

share changes across worktrees
   worktree-carry    Carry uncommitted changes to other worktrees

maintain your worktrees
   worktree-prune    Remove worktrees for remotely-deleted branches
   worktree-fetch    Fetch and update all worktrees

manage hooks
   daft hooks        Manage repository hook trust settings

"#
    );

    if via_git {
        println!("'git worktree-<command> --help' to read about a specific command.");
    } else {
        println!("'git-worktree-<command> --help' to read about a specific command.");
    }
    println!("See https://github.com/avihut/daft for documentation.");

    Ok(())
}
