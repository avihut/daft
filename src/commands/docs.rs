/// Documentation command for `git daft`
///
/// Shows comprehensive daft documentation, available commands, and project links
use anyhow::Result;

pub fn run() -> Result<()> {
    print!(
        r#"
╔════════════════════════════════════════════════════════════════╗
║                    daft - Git Extensions Toolkit                ║
╚════════════════════════════════════════════════════════════════╝

A comprehensive toolkit that extends Git functionality to enhance
developer workflows. Starting with powerful worktree management and
expanding to provide a suite of Git extensions that eliminate friction.

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
WORKTREE COMMANDS
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

  git worktree-clone <repository-url>
      Clone a repository with the worktree workflow structure
      Examples:
        git worktree-clone https://github.com/user/repo.git
        git worktree-clone --quiet git@github.com:user/repo.git
        git worktree-clone --all-branches https://github.com/user/repo.git

  git worktree-init <repository-name>
      Initialize a new repository with worktree structure
      Examples:
        git worktree-init my-project
        git worktree-init --initial-branch main my-project

  git worktree-checkout <branch-name>
      Checkout an existing branch into a new worktree
      Examples:
        git worktree-checkout feature/new-feature
        git worktree-checkout bugfix/critical-fix

  git worktree-checkout-branch <new-branch-name> [base-branch]
      Create a new branch in a new worktree
      Examples:
        git worktree-checkout-branch feature/auth
        git worktree-checkout-branch feature/ui develop

  git worktree-checkout-branch-from-default <new-branch-name>
      Create a new branch from remote's default branch in a new worktree
      Examples:
        git worktree-checkout-branch-from-default hotfix/security

  git worktree-prune
      Remove worktrees for branches that have been deleted remotely
      Examples:
        git worktree-prune

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
HOOKS MANAGEMENT
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

  git daft hooks status
      Show trust status and available hooks for current repository

  git daft hooks trust [--prompt]
      Trust current repository to run hooks
      Examples:
        git daft hooks trust           # Allow hooks to run automatically
        git daft hooks trust --prompt  # Require confirmation before each hook

  git daft hooks untrust
      Revoke trust for current repository

  git daft hooks list
      List all trusted repositories

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
KEY FEATURES
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

  • One worktree per branch - work on multiple branches simultaneously
  • No more git checkout or stashing - each branch has its own directory
  • Lifecycle hooks - run custom scripts on worktree create/remove
  • Smart branch detection - automatically detects main/master/develop
  • Robust error handling - automatic cleanup on failures

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
WORKFLOW EXAMPLE
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

  $ git worktree-clone git@github.com:user/my-project.git
  $ cd my-project/main

  $ git worktree-checkout-branch feature/auth
  # Now: my-project/feature/auth/ directory created

  $ git worktree-checkout-branch bugfix/login
  # Now: my-project/bugfix/login/ directory created

  # Your project structure:
  # my-project/
  # ├── .git/
  # ├── main/
  # ├── feature/auth/
  # └── bugfix/login/

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
HELP & DOCUMENTATION
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

  For detailed help on any command, use --help:
    git worktree-clone --help
    git worktree-checkout --help
    git worktree-init --help
    etc.

  Documentation: https://github.com/avihut/daft
  Report Issues:  https://github.com/avihut/daft/issues
  License:        MIT

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

"#
    );
    println!("Built with Rust 🦀  |  Version {}\n", daft::VERSION);
    Ok(())
}
