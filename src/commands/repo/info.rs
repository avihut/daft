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

    let relations = relations_of(&row, &catalog);

    if args.emit.is_structured() {
        let payload = EmitPayload::Document(document(&row, &relations));
        return emit::emit_and_handle("repo info", payload, &args.emit, &mut std::io::stdout())
            .map_err(|e| anyhow::anyhow!("{e}"));
    }

    render(&row, &relations, &mut output);
    Ok(())
}

/// The repo's relations manifest, resolved against the catalog. Read from
/// the repo's representative worktree (daft.yml is per-worktree); a
/// removed or unreadable repo simply has none to show.
fn relations_of(
    row: &CatalogRepoRow,
    catalog: &Catalog,
) -> Vec<crate::catalog::relations::ResolvedRelation> {
    if row.removed_at.is_some() {
        return Vec::new();
    }
    let root = std::path::Path::new(&row.path);
    let Some(worktree) = crate::core::repo::find_representative_worktree(root) else {
        return Vec::new();
    };
    let entries = crate::hooks::yaml_config_loader::load_merged_config(&worktree)
        .ok()
        .flatten()
        .and_then(|config| config.relations)
        .unwrap_or_default();
    if entries.is_empty() {
        return Vec::new();
    }
    let rows = catalog.list(false).unwrap_or_default();
    crate::catalog::relations::resolve_relations(&entries, &rows)
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

fn document(
    row: &CatalogRepoRow,
    relations: &[crate::catalog::relations::ResolvedRelation],
) -> serde_json::Value {
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
        "relations": relations.iter().map(|r| serde_json::json!({
            "url": r.entry.url,
            "name": r.entry.name,
            "kind": r.entry.kind,
            "resolved_path": r.repo.as_ref().map(|repo| repo.path.clone()),
        })).collect::<Vec<_>>(),
    })
}

fn render(
    row: &CatalogRepoRow,
    relations: &[crate::catalog::relations::ResolvedRelation],
    output: &mut dyn Output,
) {
    output.raw(&card(row, relations));
}

/// The human card, one field per line, trailing newline included —
/// `output.raw()` is `print!`, so the card brings its own newlines.
fn card(row: &CatalogRepoRow, relations: &[crate::catalog::relations::ResolvedRelation]) -> String {
    let mut lines = vec![format!("Name:            {}", row.name)];
    let status = match row.removed_at {
        Some(t) => format!("removed {}", t.format("%Y-%m-%d %H:%M UTC")),
        None => "live".to_string(),
    };
    lines.push(format!("Status:          {status}"));
    lines.push(format!("Path:            {}", row.path));
    lines.push(format!("Git dir:         {}", row.git_common_dir));
    lines.push(format!(
        "Remote:          {}",
        row.remote_url.as_deref().unwrap_or("-")
    ));
    lines.push(format!(
        "Default branch:  {}",
        row.default_branch.as_deref().unwrap_or("-")
    ));
    lines.push(format!("UUID:            {}", row.uuid));
    lines.push(format!(
        "Registered:      {}",
        row.created_at.format("%Y-%m-%d %H:%M UTC")
    ));

    if !relations.is_empty() {
        lines.push("Relations:".to_string());
        for relation in relations {
            let kind = relation
                .entry
                .kind
                .as_deref()
                .map(|k| format!(" [{k}]"))
                .unwrap_or_default();
            let target = match &relation.repo {
                Some(repo) => repo.path.clone(),
                None => format!(
                    "not cloned — `{}`",
                    crate::daft_cmd(&format!("clone {}", relation.entry.url))
                ),
            };
            lines.push(format!("  {}{kind} → {target}", relation.entry.label()));
        }
    }

    let mut card = lines.join("\n");
    card.push('\n');
    card
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> CatalogRepoRow {
        CatalogRepoRow {
            uuid: "0195-test".to_string(),
            name: "api".to_string(),
            path: "/tmp/x/api".to_string(),
            git_common_dir: "/tmp/x/api/.git".to_string(),
            remote_url: Some("git@example.com:acme/api.git".to_string()),
            remote_url_normalized: None,
            default_branch: Some("main".to_string()),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            removed_at: None,
        }
    }

    /// Regression: the first shipped renderer emitted every field through
    /// `output.raw()` (`print!`, no newline), fusing the whole card into a
    /// single line. Substring scenario assertions can't see line boundaries,
    /// so the line structure is guarded here.
    #[test]
    fn card_emits_one_line_per_field() {
        let card = card(&row(), &[]);
        assert!(card.ends_with('\n'), "card must end with a newline");
        let lines: Vec<&str> = card.trim_end().lines().collect();
        assert_eq!(lines.len(), 8, "8 fields, one line each: {card:?}");
        assert!(lines[0].starts_with("Name:"));
        assert!(lines[7].starts_with("Registered:"));
    }
}
