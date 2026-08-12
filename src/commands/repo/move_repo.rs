//! `daft repo move` — relocate a repository, and everything keyed to its path.
//!
//! The verb exists because `mv` cannot do this. A repo's identity lives inside
//! its git dir and travels with it, but git's worktree linkage, the trust
//! grant, the layout override and the recorded worktree paths are all keyed by
//! absolute path, and none of them report an error when they break. The
//! catalog heals its own path lazily, which is what makes the rest of the
//! damage invisible.

use anyhow::{Context, Result, bail};
use clap::Parser;
use std::path::{Path, PathBuf};

use crate::catalog::Catalog;
use crate::core::layout::Layout;
use crate::core::layout::detect::detect_layout;
use crate::core::layout::resolver::{LayoutResolutionContext, resolve_layout};
use crate::core::repo_move::{self, NamePlan};
use crate::core::worktree::remove_repo::{RepoTarget, enumerate_worktrees, resolve_repo};
use crate::global_config::GlobalConfig;
use crate::hooks::{TrustDatabase, yaml_config_loader};
use crate::output::{CliOutput, Output, OutputConfig};

#[derive(Parser, Debug)]
#[command(name = "git-daft-repo-move")]
#[command(version = crate::VERSION)]
#[command(about = "Move a repository, keeping its worktrees, trust and catalog entry intact")]
#[command(long_about = r#"
Relocates a repository on disk and moves everything keyed to its old path
along with it: git's worktree linkage, the trust grant, the layout override,
the catalog entry, and recorded worktree paths.

<DEST> follows `mv` semantics — an existing directory means "move into it",
anything else names the new repository directory outright. The parent has to
exist already.

Worktrees the layout placed outside the repository directory move too. Under
the default `sibling` layout that is all of them, so moving only the
repository directory would strand every worktree it has. A worktree you placed
somewhere yourself stays where you put it; its git linkage is still repaired.

--name also updates the catalog name. Without it the name follows the
directory when it was derived from the old one, and is left alone when you
chose it yourself.

Refuses before touching anything if the destination is occupied, sits inside
the repository, or lives on another filesystem.
"#)]
pub struct Args {
    #[arg(
        value_name = "REPO",
        help = "Repository to move: a catalog name, uuid, or path"
    )]
    pub repo: String,

    #[arg(
        value_name = "DEST",
        help = "Where to move it: a new path, or an existing directory to move into"
    )]
    pub dest: PathBuf,

    #[arg(
        long,
        value_name = "NAME",
        help = "Catalog name for the repository after the move"
    )]
    pub name: Option<String>,

    #[arg(
        long = "dry-run",
        help = "Print the full plan without touching anything"
    )]
    pub dry_run: bool,

    #[arg(short = 'q', long = "quiet", help = "Suppress progress reporting")]
    pub quiet: bool,

    #[arg(short = 'v', long = "verbose", help = "Show detailed progress")]
    pub verbose: bool,
}

pub fn run() -> Result<()> {
    let raw_args: Vec<String> = crate::cli::argv().to_vec();
    debug_assert!(
        raw_args.len() >= 3 && raw_args[1] == "repo" && raw_args[2] == "move",
        "repo::move_repo::run() invoked with unexpected argv: {raw_args:?} \
         (expected `daft repo move ...`)"
    );
    let argv: Vec<String> = std::iter::once("git-daft-repo-move".to_string())
        .chain(raw_args.into_iter().skip(3))
        .collect();
    run_with_args(&Args::parse_from(argv))
}

pub(crate) fn run_with_args(args: &Args) -> Result<()> {
    let mut output = CliOutput::new(OutputConfig::new(args.quiet, args.verbose));

    // Local-or-global, like `repo remove`: this command commonly runs from
    // outside any repo, where a plain `DaftSettings::load()` would fail.
    let settings = crate::core::settings::DaftSettings::load_local_or_global()?;
    let (target, cataloged) = resolve_target(&args.repo, settings.use_gitoxide)?;

    let current_name = cataloged
        .as_ref()
        .map(|row| row.name.clone())
        .or_else(|| {
            target
                .project_root
                .file_name()
                .and_then(|n| n.to_str())
                .map(String::from)
        })
        .unwrap_or_else(|| args.repo.clone());
    let own_uuid = cataloged.as_ref().map(|row| row.uuid.clone());

    let layout = resolve_repo_layout(&target);
    let worktrees = enumerate_worktrees(&target, settings.use_gitoxide)
        .context("could not list the repository's worktrees")?;

    // Name collisions are checked against the catalog here so planning itself
    // stays free of IO. A probe failure is fatal, never "assume it's free":
    // silently taking a name another repo holds is the kind of wrong the
    // catalog exists to prevent.
    let catalog = Catalog::open_ro().context("could not open the repo catalog")?;
    let holder = |name: &str| -> Option<String> {
        let row = catalog.as_ref()?.resolve_live_name(name).ok().flatten()?;
        (Some(&row.uuid) != own_uuid.as_ref()).then_some(row.path)
    };

    let plan = repo_move::build(repo_move::MoveRequest {
        target: &target,
        dest: &args.dest,
        explicit_name: args.name.as_deref(),
        current_name: &current_name,
        layout: &layout,
        worktrees: &worktrees,
        name_holder: &holder,
    })?;

    if args.dry_run {
        repo_move::print_plan(&plan);
        return Ok(());
    }

    // Announce the resolved destination before anything happens — the repo was
    // addressed by name, so the user has not necessarily seen either path.
    output.step(&format!(
        "Moving '{current_name}' → {}",
        plan.new_root.display()
    ));

    // Captured before the move: afterwards the directory may be gone and
    // `current_dir` fails outright.
    let cwd = std::env::current_dir().ok();

    let outcome = repo_move::apply(&plan)?;
    redirect_cwd(&plan, cwd.as_deref());
    report(&mut output, &plan, &outcome);

    if outcome.degraded {
        bail!("the repository moved, but some of its state did not follow — see above");
    }
    Ok(())
}

/// Catalog first, filesystem second.
///
/// A name or uuid can only come from the catalog; a path may name a repo daft
/// has never operated in, and those are movable too — the move registers the
/// result. The catalog probe **fails closed**: an unreadable store is an
/// error, never a silent downgrade to "not cataloged, treat it as a path",
/// which would turn an outage into a move of whatever directory shares the
/// name.
fn resolve_target(
    needle: &str,
    use_gitoxide: bool,
) -> Result<(RepoTarget, Option<crate::store::models::CatalogRepoRow>)> {
    let catalog = Catalog::open_ro().context("could not open the repo catalog")?;

    if let Some(catalog) = &catalog
        && let Some(row) = catalog.resolve(needle)?
    {
        if row.removed_at.is_some() {
            bail!(
                "repository '{}' was removed from the catalog; restore it with `{}`",
                row.name,
                crate::daft_cmd(&format!("clone {}", row.name))
            );
        }
        if !Path::new(&row.path).is_dir() {
            bail!(
                "catalog entry '{}' points at '{}', which no longer exists\n  \
                 tip: if it moved without daft, run `{}` from its new location",
                row.name,
                row.path,
                crate::daft_cmd("repo add")
            );
        }
        let target = resolve_repo(Some(Path::new(&row.path)), use_gitoxide)?;
        return Ok((target, Some(row)));
    }

    // Not in the catalog. A path still resolves; a bare word does not — there
    // is nothing to guess against, and guessing here would move a directory.
    let as_path = Path::new(needle);
    if as_path.is_dir() {
        let target = resolve_repo(Some(as_path), use_gitoxide)?;
        return Ok((target, None));
    }

    match &catalog {
        Some(catalog) => Err(anyhow::anyhow!(
            "{}\n  tip: `{}` shows known repos",
            catalog.not_found(needle),
            crate::daft_cmd("repo list")
        )),
        None => bail!(
            "the repo catalog is empty — clone a repo or run `{}` first",
            crate::daft_cmd("repo add")
        ),
    }
}

/// The layout in force for `target`, resolved through the normal chain.
///
/// Best-effort: the layout only decides which *out-of-tree* worktrees belong to
/// the constellation, and guessing wrong there leaves a worktree where it is
/// (still repaired, still working) rather than breaking anything.
fn resolve_repo_layout(target: &RepoTarget) -> Layout {
    let global_config = GlobalConfig::load().unwrap_or_default();
    let repo_store_layout = TrustDatabase::load()
        .ok()
        .and_then(|db| db.get_layout(&target.bare_git_dir).map(String::from));
    let yaml_layout = yaml_config_loader::load_merged_config(&target.project_root)
        .ok()
        .flatten()
        .and_then(|cfg| cfg.layout);

    let (layout, _) = resolve_layout(&LayoutResolutionContext {
        cli_layout: None,
        repo_store_layout: repo_store_layout.as_deref(),
        yaml_layout: yaml_layout.as_deref(),
        global_config: &global_config,
        detection: Some(detect_layout(&target.bare_git_dir, &global_config)),
    });
    layout
}

/// Rescue the shell when its directory just moved out from under it.
///
/// Only the parent shell can `cd`, so the binary writes the target and the
/// wrapper acts on it. A cwd outside the move is left alone — relocating a
/// user who was standing somewhere else is user-hostile.
fn redirect_cwd(plan: &repo_move::MovePlan, cwd: Option<&Path>) {
    let Some(cwd) = cwd else {
        return;
    };
    let Some(landing) = plan.map_cwd(cwd) else {
        return;
    };
    if let Ok(cd_file) = std::env::var(crate::CD_FILE_ENV) {
        let _ = std::fs::write(&cd_file, format!("{}\n", landing.display()));
    } else {
        eprintln!(
            "Run `cd {}` (your working directory moved with the repository)",
            landing.display()
        );
    }
}

fn report(output: &mut CliOutput, plan: &repo_move::MovePlan, outcome: &repo_move::MoveOutcome) {
    output.result(&format!(
        "Moved '{}' → {}",
        plan.current_name,
        plan.new_root.display()
    ));

    let relocated = plan.relocated().count();
    if relocated > 0 {
        output.detail("worktrees moved", &relocated.to_string());
    }
    let left = plan.left_in_place().count();
    if left > 0 {
        output.detail("worktrees left in place", &left.to_string());
    }
    if outcome.repaired > 0 {
        output.detail("worktrees repaired", &outcome.repaired.to_string());
    }
    if outcome.trust_rekeyed {
        output.detail("trust", "carried to the new location");
    }
    if outcome.layout_rekeyed {
        output.detail("layout override", "carried to the new location");
    }
    match (&outcome.renamed_to, &plan.name) {
        (Some(name), NamePlan::FollowsDirectory(_)) => output.notice(&format!(
            "renamed the catalog entry to '{name}' to match the directory; pass --name to choose differently"
        )),
        (Some(name), _) => output.detail("catalog name", name),
        _ => {}
    }
    for warning in &outcome.warnings {
        output.warning(warning);
    }
}
