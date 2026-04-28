//! `daft repo remove` — remove a Git repository and all its worktrees.

use anyhow::{bail, Result};
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
    let trailing: Vec<String> = std::env::args().skip(3).collect();
    let argv = std::iter::once("git-daft-repo-remove".to_string()).chain(trailing);
    let _args = Args::parse_from(argv);
    bail!("daft repo remove: not yet implemented");
}
