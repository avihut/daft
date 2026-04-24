//! git-worktree-merge - Merge branches across worktrees
//!
//! Mirrors git merge semantics when --into is omitted; enables
//! cross-worktree merges (merge <source>... into <target> from any
//! worktree) when --into is supplied. Finish commands (--abort,
//! --continue, --quit) take an optional positional <worktree|branch>
//! argument, default to CWD.

use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(name = "git-worktree-merge")]
#[command(version = crate::VERSION)]
#[command(about = "Merge branches across worktrees")]
#[command(long_about = r#"
Merges one or more source branches into a target worktree's branch.

When --into is omitted, the target is the current worktree's branch,
mirroring `git merge`. When --into <target> is supplied, the merge is
performed against that worktree's branch from wherever you are.

Multiple sources invoke git's octopus strategy, announced explicitly.

Finish commands (--abort, --continue, --quit) take an optional positional
<worktree|branch>; default to the current worktree's branch.
"#)]
pub struct Args {
    /// Source branches/commits to merge. Two or more invoke octopus.
    #[arg(value_name = "SOURCE", num_args = 1..)]
    pub sources: Vec<String>,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    pub verbose: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-merge"));
    crate::logging::init_logging(args.verbose);

    if !crate::is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    if args.sources.is_empty() {
        anyhow::bail!("specify at least one source to merge");
    }

    let cwd = std::env::current_dir()?;
    let params = crate::core::worktree::merge::StartParams {
        sources: args.sources,
    };
    let outcome = crate::core::worktree::merge::execute_start(&cwd, &params)?;

    if outcome.already_up_to_date {
        println!("Already up to date.");
    } else if outcome.conflicted {
        anyhow::bail!("merge conflicted — resolve then run `daft merge --continue`");
    } else {
        println!("Merge complete.");
    }
    Ok(())
}
