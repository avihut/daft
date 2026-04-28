//! `daft repo remove` — remove a Git repository and all its worktrees.

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "git-daft-repo-remove")]
#[command(version = crate::VERSION)]
#[command(about = "Remove a Git repository and all its worktrees")]
#[command(long_about = r#"
Removes a Git repository identified by <path> (or the current directory if no
path is given), including the bare git directory and every checked-out
worktree. For each worktree, the worktree-pre-remove and worktree-post-remove
lifecycle hooks are run when the repository is daft-managed and trusted.

Hook failures do not abort removal; failed hooks are summarized after the
operation completes. The repo is removed regardless.

Refuses to operate on paths that are not inside a Git repository.
"#)]
pub struct Args {
    #[arg(help = "Path to the repo or any directory inside it (default: cwd)")]
    pub path: Option<PathBuf>,

    #[arg(short = 'y', long = "force", help = "Skip the confirmation prompt")]
    pub force: bool,

    #[arg(
        long = "dry-run",
        help = "Print what would be removed without touching anything"
    )]
    pub dry_run: bool,

    #[arg(
        short,
        long,
        action = clap::ArgAction::Count,
        help = "Increase verbosity (-v hook details, -vv full sequential output)"
    )]
    pub verbose: u8,
}

pub fn run() -> Result<()> {
    // Build clap argv: program name + everything after `daft repo remove`.
    // `daft repo` is a subcommand category (like `daft setup shortcuts`), so
    // `crate::get_clap_args` does not recognize it; we rebuild argv manually.
    let argv: Vec<String> = std::iter::once("git-daft-repo-remove".to_string())
        .chain(std::env::args().skip(3))
        .collect();
    let args = Args::parse_from(argv);
    run_with_args(&args)
}

pub(crate) fn run_with_args(args: &Args) -> Result<()> {
    use crate::core::worktree::remove_repo::{enumerate_worktrees, resolve_repo};

    let target = resolve_repo(args.path.as_deref())?;
    let worktrees = enumerate_worktrees(&target)?;

    if args.dry_run {
        print_plan(&target, &worktrees);
        return Ok(());
    }

    anyhow::bail!("interactive removal not yet implemented");
}

fn print_plan(
    target: &crate::core::worktree::remove_repo::RepoTarget,
    worktrees: &[crate::core::worktree::remove_repo::WorktreeEntry],
) {
    println!("Would remove:");
    for w in worktrees {
        let label = w.branch.as_deref().unwrap_or("(detached)");
        println!("  worktree  {}  ({})", w.path.display(), label);
    }
    println!("  bare      {}", target.bare_git_dir.display());
    println!("  trust DB entry for {}", target.bare_git_dir.display());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn make_repo_with_worktree(tmp: &std::path::Path) -> std::path::PathBuf {
        Command::new("git")
            .arg("init")
            .arg("-q")
            .arg(tmp)
            .status()
            .unwrap();
        std::fs::write(tmp.join("README"), b"hi").unwrap();
        Command::new("git")
            .current_dir(tmp)
            .args(["add", "."])
            .status()
            .unwrap();
        Command::new("git")
            .current_dir(tmp)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t.com")
            .args(["commit", "-q", "-m", "init"])
            .status()
            .unwrap();
        let wt = tmp.join("wt-feat");
        Command::new("git")
            .current_dir(tmp)
            .args(["worktree", "add", wt.to_str().unwrap(), "-b", "feat"])
            .status()
            .unwrap();
        wt
    }

    #[test]
    fn dry_run_does_not_touch_filesystem() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = make_repo_with_worktree(tmp.path());

        let args = Args {
            path: Some(tmp.path().to_path_buf()),
            force: false,
            dry_run: true,
            verbose: 0,
        };
        run_with_args(&args).unwrap();

        assert!(tmp.path().join(".git").exists(), "bare git dir must remain");
        assert!(wt.exists(), "worktree must remain");
    }
}
