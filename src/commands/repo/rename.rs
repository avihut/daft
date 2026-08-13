//! `daft repo rename` — change the name a repository answers to.
//!
//! Metadata only, and deliberately a separate verb from `repo move`: renaming
//! a catalog entry costs nothing, while renaming a directory invalidates the
//! user's shell, their editor windows, and anything holding the old absolute
//! path. One verb that quietly did both would be a footgun.
//!
//! The capability already existed as `repo add --name`. Nobody looks for a
//! rename under `add`, which is the whole reason this verb is here.

use anyhow::{Context, Result, bail};
use clap::Parser;

use crate::catalog::Catalog;
use crate::output::{CliOutput, Output, OutputConfig};

#[derive(Parser, Debug)]
#[command(name = "git-daft-repo-rename")]
#[command(version = crate::VERSION)]
#[command(about = "Rename a repository's catalog entry")]
#[command(long_about = r#"
Changes the name a repository answers to in the repo catalog — the name taken
by `git daft go <repo>`, `git daft list <repo>` and `--repo <name>`.

Nothing on disk changes: the directory keeps its name and no worktree moves.
To relocate the repository itself, use `git daft repo move`, which can rename
the catalog entry in the same operation with --name.

This is the same operation as `git daft repo add --name <name>`.

Not to be confused with `git daft rename`, which renames a worktree and its
branch — the same way `git daft repo remove` sits beside `git daft remove`.
"#)]
pub struct Args {
    #[arg(
        value_name = "REPO",
        help = "Repository to rename: a catalog name, uuid, or path"
    )]
    pub repo: String,

    #[arg(value_name = "NEW_NAME", help = "The name it should answer to")]
    pub new_name: String,

    #[arg(short = 'q', long = "quiet", help = "Suppress progress reporting")]
    pub quiet: bool,

    #[arg(short = 'v', long = "verbose", help = "Show detailed progress")]
    pub verbose: bool,
}

pub fn run() -> Result<()> {
    let raw_args: Vec<String> = crate::cli::argv().to_vec();
    debug_assert!(
        raw_args.len() >= 3 && raw_args[1] == "repo" && raw_args[2] == "rename",
        "repo::rename::run() invoked with unexpected argv: {raw_args:?} \
         (expected `daft repo rename ...`)"
    );
    let argv: Vec<String> = std::iter::once("git-daft-repo-rename".to_string())
        .chain(raw_args.into_iter().skip(3))
        .collect();
    run_with_args(&Args::parse_from(argv))
}

pub(crate) fn run_with_args(args: &Args) -> Result<()> {
    let mut output = CliOutput::new(OutputConfig::new(args.quiet, args.verbose));

    crate::catalog::normalize::validate_catalog_name(&args.new_name)
        .map_err(|reason| anyhow::anyhow!("invalid name '{}': {reason}", args.new_name))?;

    // Renaming is a catalog operation, so the repo has to be in the catalog —
    // unlike `repo move`, there is no filesystem fallback to give a name to.
    let row = crate::catalog::resolve_repo_arg_missing_ok(&args.repo)?;

    if row.name == args.new_name {
        output.result(&format!("'{}' already has that name", row.name));
        return Ok(());
    }

    // Pre-checked so the message matches the one `repo add --name` raises,
    // rather than surfacing a bare store error.
    let catalog = Catalog::open_rw().context("could not open the repo catalog")?;
    if let Some(existing) = catalog
        .resolve_live_name(&args.new_name)
        .context("could not check the requested name")?
        && existing.uuid != row.uuid
    {
        bail!(
            "the name '{}' is already used by the repo at {}\n  \
             tip: pick a different name, or rename that repo first with `{}`",
            args.new_name,
            existing.path,
            crate::daft_cmd(&format!("repo rename {} <name>", existing.name))
        );
    }

    catalog
        .rename(&row.uuid, &args.new_name)
        .context("could not rename the repository")?;

    output.result(&format!("Renamed '{}' → '{}'", row.name, args.new_name));
    Ok(())
}
