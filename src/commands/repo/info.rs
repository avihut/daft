//! `daft repo info` — one catalog entry in detail.

use anyhow::{Context, Result};
use clap::Parser;
use std::path::Path;

use crate::catalog::Catalog;
use crate::catalog::worktrees::WorktreeChild;
use crate::output::emit::{self, EmitArgs, EmitPayload};
use crate::output::format::display_path;
use crate::output::tui::tree_glyph;
use crate::output::{CliOutput, Output, OutputConfig};
use crate::store::CatalogRepoRow;

#[derive(Parser, Debug)]
#[command(name = "git-daft-repo-info")]
#[command(version = crate::VERSION)]
#[command(about = "Show a repository's catalog entry")]
#[command(long_about = r#"
Shows a repository's catalog entry: name, status, location, remote, default
branch, recorded worktree layout, its worktrees (branch and checkout path
per line), and any daft.yml relations resolved against the catalog. The
repository may be addressed by catalog name, path, or uuid; with no
argument the repo containing the current directory is shown.

Paths render relative to your working directory when that form is shorter
(same rule as `git daft repo list`). Identity plumbing lives in structured
output only: `--format json` carries every recorded field — uuid, git
common dir, raw canonical paths, registration timestamps — plus the
worktrees as a `{branch, path}` array.

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
    let children = crate::catalog::worktrees::worktree_children(&row, None);
    // Layout lookup stays at the command layer — the catalog module keeps
    // its zero-TrustDatabase invariant (see catalog/mod.rs).
    let trust_db = crate::hooks::TrustDatabase::load().ok();
    let layout = trust_db
        .as_ref()
        .and_then(|db| db.get_layout(Path::new(&row.git_common_dir)))
        .map(String::from);

    if args.emit.is_structured() {
        let payload = EmitPayload::Document(document(
            &row,
            layout.as_deref(),
            children.as_deref(),
            &relations,
        ));
        return emit::emit_and_handle("repo info", payload, &args.emit, &mut std::io::stdout())
            .map_err(|e| anyhow::anyhow!("{e}"));
    }

    let cwd = std::env::current_dir()
        .ok()
        .and_then(|cwd| std::fs::canonicalize(cwd).ok());
    output.raw(&card(
        &row,
        layout.as_deref(),
        children.as_deref(),
        &relations,
        cwd.as_deref(),
    ));
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
    layout: Option<&str>,
    children: Option<&[WorktreeChild]>,
    relations: &[crate::catalog::relations::ResolvedRelation],
) -> serde_json::Value {
    serde_json::json!({
        "name": row.name,
        "uuid": row.uuid,
        "path": row.path,
        "git_common_dir": row.git_common_dir,
        "remote_url": row.remote_url,
        "default_branch": row.default_branch,
        "layout": layout,
        // Raw canonical paths, null branch when detached, null list when
        // the repo couldn't be opened — same shape as `repo list -w`.
        "worktrees": children.map(|children| children.iter().map(|c| serde_json::json!({
            "branch": c.branch,
            "path": c.path,
        })).collect::<Vec<_>>()),
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

/// Label column width: the longest label ("Default branch") plus a
/// three-space gutter.
const LABEL_WIDTH: usize = 17;

/// The human card, one field per line, trailing newline included —
/// `output.raw()` is `print!`, so the card brings its own newlines.
///
/// The card answers "what does daft know about this repo, and what's in
/// it": paths follow the display rule (cwd-relative when no longer than
/// the tilde form) and the worktrees expand as a tree, mirroring
/// `repo list --worktrees`. Identity plumbing (uuid, git common dir,
/// registration timestamps) is structured-output-only.
fn card(
    row: &CatalogRepoRow,
    layout: Option<&str>,
    children: Option<&[WorktreeChild]>,
    relations: &[crate::catalog::relations::ResolvedRelation],
    cwd: Option<&Path>,
) -> String {
    let field = |label: &str, value: &str| format!("{label:<LABEL_WIDTH$}{value}");
    let mut lines = vec![field("Name", &row.name)];
    let status = match row.removed_at {
        Some(t) => format!("removed {}", t.format("%Y-%m-%d %H:%M UTC")),
        None => "live".to_string(),
    };
    lines.push(field("Status", &status));
    lines.push(field("Path", &display_path(&row.path, cwd)));
    lines.push(field("Remote", row.remote_url.as_deref().unwrap_or("-")));
    lines.push(field(
        "Default branch",
        row.default_branch.as_deref().unwrap_or("-"),
    ));
    lines.push(field("Layout", layout.unwrap_or("-")));

    // `-` when the repo can't be opened (stale path, removed entry) —
    // same signal as the repo list Worktrees column.
    let count = children
        .map(|c| c.len().to_string())
        .unwrap_or_else(|| "-".to_string());
    lines.push(field("Worktrees", &count));
    if let Some(children) = children {
        let branch_width = children
            .iter()
            .map(|c| c.branch_label().chars().count())
            .max()
            .unwrap_or(0);
        for (i, child) in children.iter().enumerate() {
            lines.push(format!(
                "  {}{:<branch_width$}   {}",
                tree_glyph(i, children.len()),
                child.branch_label(),
                display_path(&child.path, cwd),
            ));
        }
    }

    if !relations.is_empty() {
        lines.push("Relations".to_string());
        for relation in relations {
            let kind = relation
                .entry
                .kind
                .as_deref()
                .map(|k| format!(" [{k}]"))
                .unwrap_or_default();
            let target = match &relation.repo {
                Some(repo) => display_path(&repo.path, cwd),
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

    fn child(branch: Option<&str>, path: &str) -> WorktreeChild {
        WorktreeChild {
            branch: branch.map(String::from),
            path: path.to_string(),
            current: false,
        }
    }

    /// Regression: the first shipped renderer emitted every field through
    /// `output.raw()` (`print!`, no newline), fusing the whole card into a
    /// single line. Substring scenario assertions can't see line boundaries,
    /// so the line structure is guarded here.
    #[test]
    fn card_emits_one_line_per_field() {
        let card = card(&row(), Some("contained"), None, &[], None);
        assert!(card.ends_with('\n'), "card must end with a newline");
        let lines: Vec<&str> = card.trim_end().lines().collect();
        assert_eq!(lines.len(), 7, "7 fields, one line each: {card:?}");
        assert!(lines[0].starts_with("Name"));
        assert!(lines[5].starts_with("Layout"));
        assert!(lines[6].starts_with("Worktrees        -"));
    }

    /// Identity plumbing is structured-output-only: the human card answers
    /// "what is this repo and what's in it", not "what are its internals".
    #[test]
    fn card_keeps_plumbing_out_of_the_human_view() {
        let card = card(&row(), None, None, &[], None);
        for plumbing in ["UUID", "Git dir", "Registered", "0195-test"] {
            assert!(!card.contains(plumbing), "{plumbing} leaked into: {card}");
        }
        let json = document(&row(), None, None, &[]);
        assert_eq!(json["uuid"], "0195-test", "JSON keeps the plumbing");
        assert_eq!(json["git_common_dir"], "/tmp/x/api/.git");
    }

    /// The worktrees expand as a tree under the count — same glyphs and
    /// order contract as `repo list --worktrees`, detached labeled.
    #[test]
    fn card_expands_worktrees_as_a_tree() {
        let children = vec![
            child(Some("main"), "/tmp/x/api/main"),
            child(Some("feat/rates"), "/tmp/x/api/feat/rates"),
            child(None, "/tmp/x/api/parked"),
        ];
        let card = card(&row(), None, Some(&children), &[], None);
        let lines: Vec<&str> = card.trim_end().lines().collect();
        assert_eq!(lines.len(), 10, "7 fields + 3 children: {card:?}");
        assert!(lines[6].starts_with("Worktrees        3"));
        assert!(lines[7].starts_with("  ├ main"));
        assert!(lines[8].starts_with("  ├ feat/rates"));
        assert!(
            lines[9].starts_with("  └ (detached)"),
            "last child gets the corner glyph and detached label: {:?}",
            lines[9]
        );
        assert!(lines[9].ends_with("/tmp/x/api/parked"));
    }

    /// Structured output mirrors `repo list -w`: a worktrees array with raw
    /// paths and a null branch for detached HEADs, plus the recorded layout.
    #[test]
    fn document_carries_layout_and_worktrees() {
        let children = vec![
            child(Some("main"), "/tmp/x/api/main"),
            child(None, "/tmp/x/api/parked"),
        ];
        let json = document(&row(), Some("contained"), Some(&children), &[]);
        assert_eq!(json["layout"], "contained");
        assert_eq!(json["worktrees"][0]["branch"], "main");
        assert_eq!(json["worktrees"][0]["path"], "/tmp/x/api/main");
        assert!(json["worktrees"][1]["branch"].is_null());

        let unopenable = document(&row(), None, None, &[]);
        assert!(
            unopenable["worktrees"].is_null(),
            "null (not []) when the repo can't be opened"
        );
    }
}
