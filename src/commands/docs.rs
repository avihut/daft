/// Documentation command for `git daft`
///
/// Shows daft commands in git-style help format
use anyhow::Result;

pub fn run() -> Result<()> {
    print!(
        r#"usage: git daft <command> [<args>]

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

'git <command> --help' to read about a specific command.
See https://github.com/avihut/daft for documentation.
"#
    );
    Ok(())
}
