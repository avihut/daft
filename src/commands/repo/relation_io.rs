//! Shared plumbing for `daft repo link`/`unlink`: locating the daft.yml to
//! edit, resolving a target argument to a remote URL, writing the manifest
//! atomically, and reporting the resulting edge.
//!
//! The pure block-editing logic lives in [`crate::catalog::relations_edit`];
//! everything here is the IO and catalog resolution that surrounds it.

use anyhow::{Context, Result, anyhow, bail};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::catalog::normalize::{looks_like_remote_source, normalize_url};
use crate::catalog::relations::{RelationEntry, resolve_relations};
use crate::catalog::{Catalog, CatalogError};
use crate::hooks::yaml_config_loader::{ConfigStatus, classify_main_config};
use crate::output::{CliOutput, Output};

/// The daft.yml a link/unlink edit targets: the current worktree's config
/// file (or the path where one will be created) plus its current text.
pub struct ManifestTarget {
    /// The worktree root the manifest is anchored to (drives the
    /// tracked/untracked hint and the overlay-shadow check).
    pub root: PathBuf,
    /// The config file to write.
    pub path: PathBuf,
    /// Whether the file already exists (an in-place edit) or will be created.
    pub existed: bool,
    /// The file's current text (empty when it will be created).
    pub text: String,
}

/// Locate the manifest to edit: the current worktree's daft.yml, or the path
/// where a new one should be created. Relations are read by consumers from
/// the worktree root ([`crate::catalog::relations::current_repo_relations`]),
/// so edits are anchored there too.
pub fn locate_manifest() -> Result<ManifestTarget> {
    let root = crate::core::repo::get_current_worktree_path().context(
        "not inside a worktree — run this from a repo worktree (relations live in its daft.yml)",
    )?;
    let (path, existed, text) = match crate::hooks::yaml_config_loader::find_config_file(&root) {
        Some((path, _)) => {
            let text = fs::read_to_string(&path)
                .with_context(|| format!("could not read {}", path.display()))?;
            (path, true, text)
        }
        None => (root.join("daft.yml"), false, String::new()),
    };
    Ok(ManifestTarget {
        root,
        path,
        existed,
        text,
    })
}

/// Resolve a link/unlink target to a remote URL. Resolution order mirrors the
/// completion order: a catalog repo name (or uuid/cataloged path), then an
/// existing filesystem path to a git repo, then a bare remote URL.
pub fn resolve_target_url(needle: &str) -> Result<String> {
    // 1. Catalog: live or removed entry by name, uuid, or cataloged path.
    //    A removed entry is still a valid target — its remote URL is retained
    //    and the edge simply resolves as "not cloned".
    if let Some(catalog) = Catalog::open_ro().ok().flatten()
        && let Some(row) = catalog.resolve(needle)?
    {
        return row.remote_url.ok_or_else(|| {
            anyhow!(
                "repo '{}' has no remote URL on record — relations are keyed by remote URL, so it can't be a link target",
                row.name
            )
        });
    }

    // 2. A filesystem path to a git repo that isn't in the catalog.
    let path = Path::new(needle);
    if path.exists() {
        if let Some(url) = remote_url_at_path(path) {
            return Ok(url);
        }
        bail!(
            "'{needle}' has no `origin` remote to link to — pass the remote URL directly instead"
        );
    }

    // 3. Otherwise, take it as a remote URL when it is shaped like one.
    if looks_like_remote_source(needle) {
        return Ok(needle.trim().to_string());
    }

    Err(no_such_target(needle))
}

/// The current repo's own `origin` URL, for the self-link guard. `None` when
/// the repo has no remote (in which case a URL self-link is impossible
/// anyway).
pub fn current_repo_url() -> Option<String> {
    let git_common_dir = crate::core::repo::get_git_common_dir().ok()?;
    crate::hooks::get_remote_url_for_git_dir(&git_common_dir)
}

/// Write the manifest. A brand-new file is a plain (umask-honoring) write; an
/// existing file is replaced atomically via a same-directory temp file whose
/// permissions are copied from the original (tempfile defaults to 0600, which
/// would otherwise strip group/other read from a committed, shared file).
pub fn write_manifest(target: &ManifestTarget, new_text: &str) -> Result<()> {
    if !target.existed {
        return fs::write(&target.path, new_text)
            .with_context(|| format!("could not create {}", target.path.display()));
    }

    let dir = target.path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("could not create a temp file in {}", dir.display()))?;
    tmp.write_all(new_text.as_bytes())?;
    tmp.as_file().sync_all()?;
    if let Ok(meta) = fs::metadata(&target.path) {
        let _ = tmp.as_file().set_permissions(meta.permissions());
    }
    tmp.persist(&target.path)
        .with_context(|| format!("could not write {}", target.path.display()))?;
    Ok(())
}

/// Print the primary result line for an edge, mirroring `repo info`'s
/// Relations rendering: `<verb>: <label> [kind] → <local path | not cloned>`.
pub fn report_edge(output: &mut CliOutput, verb: &str, entry: &RelationEntry) {
    let kind = entry
        .kind
        .as_deref()
        .map(|k| format!(" [{k}]"))
        .unwrap_or_default();
    output.result(&format!(
        "{verb}: {}{kind} → {}",
        entry.label(),
        resolve_edge_target(entry)
    ));
}

/// After a successful write, tell the user whether the file is shared yet, and
/// warn when a local override shadows the change (`expect_present` is whether
/// the edited edge should now be visible in the merged config: true after a
/// link, false after an unlink).
pub fn post_write_hint(output: &mut CliOutput, root: &Path, key: &str, expect_present: bool) {
    match classify_main_config(root) {
        ConfigStatus::Tracked => {
            output.notice("edited daft.yml — commit it to share this change with your team");
        }
        ConfigStatus::Visitor | ConfigStatus::Missing => {
            output
                .notice("daft.yml isn't tracked yet — commit it to share relations with your team");
        }
    }

    if let Ok(Some(merged)) = crate::hooks::yaml_config_loader::load_merged_config(root) {
        let present = merged
            .relations
            .unwrap_or_default()
            .iter()
            .any(|e| normalize_url(&e.url) == key);
        if present != expect_present {
            output.warning(
                "a local override (daft.local.yml or an `extends` file) replaces `relations:` — \
                 this change won't take effect until that override is reconciled",
            );
        }
    }
}

/// Resolve an edge to a display target: the cloned repo's path when the URL
/// matches a live catalog entry, else a "not cloned" hint with a clone command.
fn resolve_edge_target(entry: &RelationEntry) -> String {
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|cwd| std::fs::canonicalize(cwd).ok());
    let rows = Catalog::open_ro()
        .ok()
        .flatten()
        .and_then(|catalog| catalog.list(false).ok())
        .unwrap_or_default();
    let resolved = resolve_relations(std::slice::from_ref(entry), &rows);
    match resolved.first().and_then(|r| r.repo.as_ref()) {
        Some(repo) => crate::output::format::display_path(&repo.path, cwd.as_deref()),
        None => format!(
            "not cloned — `{}`",
            crate::daft_cmd(&format!("clone {}", entry.url))
        ),
    }
}

/// `git -C <path> remote get-url origin`, with the ambient `GIT_*` vars
/// cleared so `-C` is authoritative (see the test-hygiene note in CLAUDE.md).
fn remote_url_at_path(path: &Path) -> Option<String> {
    let output = crate::utils::git_command_at(path)
        .args(["remote", "get-url", "origin"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Build the "no such target" error, with did-you-mean suggestions from live
/// catalog names when the argument looked like a (mistyped) name.
fn no_such_target(needle: &str) -> anyhow::Error {
    if let Some(catalog) = Catalog::open_ro().ok().flatten()
        && let CatalogError::NotFound { suggestions, .. } = catalog.not_found(needle)
        && !suggestions.is_empty()
    {
        return anyhow!(
            "no repo '{needle}' in the catalog, and it isn't a path or remote URL\n  \
             did you mean: {}",
            suggestions.join(", ")
        );
    }
    anyhow!(
        "no repo '{needle}' in the catalog, and it isn't a path or remote URL — \
         pass a catalog name, a repo path, or a remote URL"
    )
}
