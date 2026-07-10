//! `daft repo add` — explicitly register a repository in the catalog.
//!
//! The catalog fills itself: clone/init/adopt register, and any daft
//! command running inside a repo lazily upserts it. `repo add` exists for
//! the two cases ambience can't cover — a repo daft has never operated in,
//! and renaming an entry (`--name`). Unlike ambient registration this path
//! is loud: catalog failures are errors, and `--name` collisions refuse
//! rather than auto-suffix.

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

use crate::catalog::{Catalog, CatalogError};
use crate::core::settings::DaftSettings;
use crate::core::worktree::remove_repo::resolve_repo;
use crate::output::{CliOutput, Output, OutputConfig};

#[derive(Parser, Debug)]
#[command(name = "git-daft-repo-add")]
#[command(version = crate::VERSION)]
#[command(about = "Register a repository in the repo catalog")]
#[command(long_about = r#"
Registers a repository in daft's repo catalog — the machine-local registry
behind cross-repo commands like `git daft go <repo>` and `git daft repo list`.

The catalog is normally maintained automatically: cloning, initializing, or
running any daft command inside a repo keeps its entry current. Reach for
`repo add` to register a repository daft has never operated in, or to rename
an entry with --name.

Names must be unique among live entries. Automatic registration resolves
collisions by suffixing (`api-2`); an explicit --name that is already taken
is an error instead.
"#)]
pub struct Args {
    #[arg(
        value_name = "PATH",
        help = "Repository to register (default: the repo containing the current directory)"
    )]
    path: Option<PathBuf>,

    #[arg(
        long,
        value_name = "NAME",
        help = "Catalog name for the repo; renames it when already registered"
    )]
    name: Option<String>,

    #[arg(short = 'q', long = "quiet", help = "Suppress progress reporting")]
    quiet: bool,

    #[arg(short = 'v', long = "verbose", help = "Show detailed progress")]
    verbose: bool,
}

pub fn run() -> Result<()> {
    let raw_args: Vec<String> = crate::cli::argv().to_vec();
    debug_assert!(
        raw_args.len() >= 3 && raw_args[1] == "repo" && raw_args[2] == "add",
        "repo::add::run() invoked with unexpected argv: {raw_args:?} \
         (expected `daft repo add ...`)"
    );
    let argv: Vec<String> = std::iter::once("git-daft-repo-add".to_string())
        .chain(raw_args.into_iter().skip(3))
        .collect();
    let args = Args::parse_from(argv);
    let mut output = CliOutput::new(OutputConfig::new(args.quiet, args.verbose));

    let settings = DaftSettings::load().unwrap_or_default();
    let target = resolve_repo(args.path.as_deref(), settings.use_gitoxide)?;
    let facts =
        crate::catalog::gather_facts(&target.bare_git_dir, &target.project_root, None, None)
            .context("could not gather repository facts")?;

    let catalog = Catalog::open_rw().context("could not open the repo catalog")?;
    let outcome = catalog
        .register(&facts)
        .context("could not register the repository")?;

    let mut assigned = outcome.assigned_name.clone();
    if let Some(requested) = &args.name
        && *requested != assigned
    {
        match catalog.rename(&facts.uuid, requested) {
            Ok(()) => assigned = requested.clone(),
            Err(err @ CatalogError::NameTaken { .. }) => {
                anyhow::bail!(
                    "{err}\n  tip: pick a different name, or rename the other repo first \
                     with `{}` from inside it",
                    crate::daft_cmd("repo add --name <name>")
                );
            }
            Err(err) => return Err(err.into()),
        }
    }

    let verb = if outcome.created {
        "Registered"
    } else if outcome.resurrected {
        "Restored"
    } else {
        "Refreshed"
    };
    output.result(&format!("{verb} '{assigned}' → {}", facts.path));
    if outcome.suffixed && args.name.is_none() {
        output.notice(&format!(
            "'{}' was taken by another repo; pass `--name <name>` to choose differently",
            facts.default_name
        ));
    }
    Ok(())
}
