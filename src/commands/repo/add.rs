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

use crate::catalog::{Catalog, RegistrationFacts, RegistrationOutcome};
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
    let (assigned, outcome) = register_with_name(&catalog, &facts, args.name.as_deref())?;

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

/// Register `facts`, applying an optional explicit `--name`. All-or-nothing: an
/// invalid name, or one already claimed by a *different* repo, is refused
/// before `register()` writes anything — so a rejected `--name` never strands
/// the repo in the catalog under an auto-suffixed name. Returns the finally
/// assigned name and the registration outcome (for the created/restored verb).
fn register_with_name(
    catalog: &Catalog,
    facts: &RegistrationFacts,
    requested_name: Option<&str>,
) -> Result<(String, RegistrationOutcome)> {
    if let Some(requested) = requested_name {
        crate::catalog::normalize::validate_catalog_name(requested)
            .map_err(|reason| anyhow::anyhow!("invalid --name '{requested}': {reason}"))?;
        if let Some(existing) = catalog
            .resolve_live_name(requested)
            .context("could not check the requested name")?
            && existing.uuid != facts.uuid
        {
            anyhow::bail!(
                "the name '{requested}' is already used by the repo at {}\n  \
                 tip: pick a different name, or rename that repo first with `{}` from inside it",
                existing.path,
                crate::daft_cmd("repo add --name <name>")
            );
        }
    }

    let outcome = catalog
        .register(facts)
        .context("could not register the repository")?;
    let mut assigned = outcome.assigned_name.clone();
    if let Some(requested) = requested_name
        && requested != assigned
    {
        // Pre-checked free above; a concurrent grab is the only NameTaken race.
        catalog
            .rename(&facts.uuid, requested)
            .context("could not apply --name")?;
        assigned = requested.to_string();
    }
    Ok((assigned, outcome))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::paths;
    use tempfile::TempDir;

    fn catalog(tmp: &TempDir) -> Catalog {
        Catalog::open_rw_at(&paths::catalog_db_under(tmp.path()).unwrap()).unwrap()
    }

    fn facts(uuid: &str, name: &str, path: &str) -> RegistrationFacts {
        RegistrationFacts {
            uuid: uuid.into(),
            default_name: name.into(),
            path: path.into(),
            git_common_dir: format!("{path}/.git"),
            remote_url: Some(format!("git@example.com:org/{name}.git")),
            default_branch: Some("main".into()),
        }
    }

    #[test]
    fn registers_a_new_repo_under_its_derived_name() {
        let tmp = TempDir::new().unwrap();
        let cat = catalog(&tmp);
        let (assigned, outcome) =
            register_with_name(&cat, &facts("u1", "api", "/w/api"), None).unwrap();
        assert_eq!(assigned, "api");
        assert!(outcome.created);
    }

    #[test]
    fn applies_a_free_explicit_name() {
        let tmp = TempDir::new().unwrap();
        let cat = catalog(&tmp);
        let (assigned, _) =
            register_with_name(&cat, &facts("u1", "api", "/w/api"), Some("backend")).unwrap();
        assert_eq!(assigned, "backend");
        assert!(cat.resolve_live_name("backend").unwrap().is_some());
    }

    #[test]
    fn a_taken_name_is_refused_before_registering() {
        // #357 C7: a --name collision must be all-or-nothing — the rejected
        // repo must NOT be left cataloged under an auto-suffixed name.
        let tmp = TempDir::new().unwrap();
        let cat = catalog(&tmp);
        register_with_name(&cat, &facts("u1", "api", "/w/api"), None).unwrap();

        let err = register_with_name(&cat, &facts("u2", "web", "/w/web"), Some("api")).unwrap_err();
        assert!(err.to_string().contains("already used"), "{err}");
        assert!(cat.resolve_live_name("web").unwrap().is_none());
        assert!(cat.resolve_live_name("web-2").unwrap().is_none());
        assert!(cat.get_by_uuid("u2").unwrap().is_none());
    }

    #[test]
    fn an_invalid_name_is_refused_before_registering() {
        let tmp = TempDir::new().unwrap();
        let cat = catalog(&tmp);
        let err =
            register_with_name(&cat, &facts("u1", "api", "/w/api"), Some("-bad")).unwrap_err();
        assert!(err.to_string().contains("invalid --name"), "{err}");
        assert!(cat.get_by_uuid("u1").unwrap().is_none());
    }

    #[test]
    fn renames_an_existing_repo_to_a_free_name() {
        let tmp = TempDir::new().unwrap();
        let cat = catalog(&tmp);
        register_with_name(&cat, &facts("u1", "api", "/w/api"), None).unwrap();
        let (assigned, outcome) =
            register_with_name(&cat, &facts("u1", "api", "/w/api"), Some("backend")).unwrap();
        assert_eq!(assigned, "backend");
        assert!(!outcome.created);
        assert!(cat.resolve_live_name("api").unwrap().is_none());
        assert!(cat.resolve_live_name("backend").unwrap().is_some());
    }
}
