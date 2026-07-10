//! `daft repo info` — one catalog entry in detail.

use anyhow::{Context, Result};
use clap::Parser;

use crate::catalog::Catalog;
use crate::output::emit::{self, EmitArgs, EmitPayload};
use crate::output::{CliOutput, Output, OutputConfig};
use crate::store::CatalogRepoRow;

#[derive(Parser, Debug)]
#[command(name = "git-daft-repo-info")]
#[command(version = crate::VERSION)]
#[command(about = "Show a repository's catalog entry")]
#[command(long_about = r#"
Shows a repository's catalog entry: name, location, remote, default branch,
identity, and removed-state. The repository may be addressed by catalog
name, path, or uuid; with no argument the repo containing the current
directory is shown.

Removed repositories resolve too — their entries are retained so job logs
stay addressable and `git daft clone <name>` can restore them.
"#)]
pub struct Args {
    #[arg(
        value_name = "REPO",
        help = "Catalog name, path, or uuid (default: the current repo)"
    )]
    needle: Option<String>,

    #[command(flatten)]
    emit: EmitArgs,
}

pub fn run() -> Result<()> {
    let raw_args: Vec<String> = crate::cli::argv().to_vec();
    debug_assert!(
        raw_args.len() >= 3 && raw_args[1] == "repo" && raw_args[2] == "info",
        "repo::info::run() invoked with unexpected argv: {raw_args:?} \
         (expected `daft repo info ...`)"
    );
    let argv: Vec<String> = std::iter::once("git-daft-repo-info".to_string())
        .chain(raw_args.into_iter().skip(3))
        .collect();
    let args = Args::parse_from(argv);
    let mut output = CliOutput::new(OutputConfig::default());

    let Some(catalog) = Catalog::open_ro().context("could not open the repo catalog")? else {
        anyhow::bail!(
            "the repo catalog is empty — clone a repo or run `{}` first",
            crate::daft_cmd("repo add")
        );
    };

    let row = match &args.needle {
        Some(needle) => catalog
            .resolve(needle)?
            .ok_or_else(|| not_found_error(&catalog, needle))?,
        None => {
            let git_dir = crate::core::repo::get_git_common_dir()
                .context("not inside a git repository — pass a repo name or path")?;
            let canonical = git_dir.canonicalize().unwrap_or(git_dir);
            let needle = canonical.to_string_lossy().into_owned();
            catalog
                .resolve(&needle)?
                .ok_or_else(|| not_found_error(&catalog, &needle))?
        }
    };

    if args.emit.is_structured() {
        let payload = EmitPayload::Document(document(&row));
        return emit::emit_and_handle("repo info", payload, &args.emit, &mut std::io::stdout())
            .map_err(|e| anyhow::anyhow!("{e}"));
    }

    render(&row, &mut output);
    Ok(())
}

fn not_found_error(catalog: &Catalog, needle: &str) -> anyhow::Error {
    let err = catalog.not_found(needle);
    if let crate::catalog::CatalogError::NotFound { suggestions, .. } = &err
        && !suggestions.is_empty()
    {
        return anyhow::anyhow!("{err}\n  did you mean: {}", suggestions.join(", "));
    }
    err.into()
}

fn document(row: &CatalogRepoRow) -> serde_json::Value {
    serde_json::json!({
        "name": row.name,
        "uuid": row.uuid,
        "path": row.path,
        "git_common_dir": row.git_common_dir,
        "remote_url": row.remote_url,
        "default_branch": row.default_branch,
        "created_at": row.created_at.to_rfc3339(),
        "updated_at": row.updated_at.to_rfc3339(),
        "removed_at": row.removed_at.map(|t| t.to_rfc3339()),
    })
}

fn render(row: &CatalogRepoRow, output: &mut dyn Output) {
    output.raw(&format!("Name:            {}", row.name));
    let status = match row.removed_at {
        Some(t) => format!("removed {}", t.format("%Y-%m-%d %H:%M UTC")),
        None => "live".to_string(),
    };
    output.raw(&format!("Status:          {status}"));
    output.raw(&format!("Path:            {}", row.path));
    output.raw(&format!("Git dir:         {}", row.git_common_dir));
    output.raw(&format!(
        "Remote:          {}",
        row.remote_url.as_deref().unwrap_or("-")
    ));
    output.raw(&format!(
        "Default branch:  {}",
        row.default_branch.as_deref().unwrap_or("-")
    ));
    output.raw(&format!("UUID:            {}", row.uuid));
    output.raw(&format!(
        "Registered:      {}",
        row.created_at.format("%Y-%m-%d %H:%M UTC")
    ));
}
