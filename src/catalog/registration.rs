//! Imperative shell for catalog registration.
//!
//! Registration is deliberately *ambient*: any command that creates or
//! touches a repo keeps the catalog current as a side effect, so `daft repo
//! add` is only ever needed for repos daft has never operated in. Two entry
//! points with different noise contracts:
//!
//! * [`register_repo`] — full-fact registration for commands that just
//!   created or converted a repo (clone, init, adopt, eject). Best-effort:
//!   catalog failures warn but never fail the parent command. Prints the
//!   auto-suffix notice so the user learns their repo's catalog name when
//!   it isn't the obvious one.
//! * [`touch_current_repo`] — cheap lazy upsert for commands merely
//!   *running inside* a repo (go, list, exec, fetch, prune). Fully silent;
//!   reads first and writes only when the catalog doesn't know the repo or
//!   its location drifted. Never called on `__complete`/shell-init hot
//!   paths.

use crate::catalog::normalize;
use crate::catalog::service::{Catalog, RegistrationFacts};
use crate::core::repo_identity::compute_repo_id_from_common_dir;
use crate::output::Output;
use crate::store::CatalogRepoRow;
use std::path::Path;

/// Build [`RegistrationFacts`] for a repo whose git common dir and project
/// root are known. Canonicalizes both paths, derives the default name from
/// the remote URL (falling back to the project dir's name), and consults
/// `origin/HEAD` for the default branch when the caller doesn't know it.
pub fn gather_facts(
    git_common_dir: &Path,
    project_root: &Path,
    remote_url: Option<String>,
    default_branch: Option<String>,
) -> anyhow::Result<RegistrationFacts> {
    let uuid = compute_repo_id_from_common_dir(git_common_dir)?;
    let canonical_root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let canonical_gcd = git_common_dir
        .canonicalize()
        .unwrap_or_else(|_| git_common_dir.to_path_buf());
    let remote_url =
        remote_url.or_else(|| crate::hooks::get_remote_url_for_git_dir(&canonical_gcd));
    let default_branch = default_branch
        .or_else(|| crate::core::remote::local_default_branch(&canonical_root, "origin"));
    let default_name = normalize::derive_default_name(remote_url.as_deref(), &canonical_root);
    Ok(RegistrationFacts {
        uuid,
        default_name,
        path: canonical_root.to_string_lossy().into_owned(),
        git_common_dir: canonical_gcd.to_string_lossy().into_owned(),
        remote_url,
        default_branch,
    })
}

/// Ambient catalog writes are disabled in in-process unit tests unless the
/// data dir is sandboxed: command-level unit tests would otherwise write
/// temp-repo entries into the developer's real catalog. Integration and
/// YAML-scenario runs always export `DAFT_DATA_DIR`, so they exercise the
/// real behavior.
fn ambient_writes_allowed() -> bool {
    !cfg!(test) || std::env::var_os("DAFT_DATA_DIR").is_some()
}

/// Register a repo in the catalog, best-effort. Failures warn; a suffixed
/// name gets a notice so the user knows what `daft go <name>` will expect.
pub fn register_repo(facts: &RegistrationFacts, output: &mut dyn Output) {
    if !ambient_writes_allowed() {
        return;
    }
    match Catalog::open_rw().and_then(|catalog| catalog.register(facts)) {
        Ok(outcome) if outcome.suffixed => {
            output.notice(&format!(
                "Cataloged as '{}' ('{}' is taken by another repo — rename with `{}`)",
                outcome.assigned_name,
                facts.default_name,
                crate::daft_cmd("repo add --name <name>"),
            ));
        }
        Ok(_) => {}
        Err(e) => {
            output.warning(&format!("Could not update the repo catalog: {e}"));
        }
    }
}

/// Silent lazy upsert for the repo the current directory sits in. All
/// errors (not in a repo, catalog locked, read-only FS…) are swallowed —
/// the catalog is a convenience index, never a blocker.
pub fn touch_current_repo() {
    if !ambient_writes_allowed() {
        return;
    }
    let _ = touch_current_repo_impl();
}

/// Preserve a repo's catalog record just before it is deleted, then mark it
/// removed. Must run **before** the git dir is destroyed: it reads
/// `daft-id` and canonicalizes live paths. Registration-then-removal means
/// even a never-cataloged repo stays addressable afterwards (`daft hooks
/// jobs --repo <name>`, `daft clone <name>`), which is the whole point of
/// retaining removed entries. Silent best-effort; if deletion subsequently
/// fails, the next in-repo command resurrects the entry via lazy touch.
pub fn note_repo_removed(bare_git_dir: &Path, project_root: &Path) {
    if !ambient_writes_allowed() {
        return;
    }
    let _ = tombstone_repo_at(bare_git_dir, project_root);
}

/// Tombstone one already-resolved catalog row — the write behind
/// `repo remove`'s default. Explicit user request, so failures propagate
/// (unlike [`note_repo_removed`], which rides along with `--purge`).
pub fn mark_row_removed(uuid: &str) -> anyhow::Result<()> {
    if !ambient_writes_allowed() {
        return Ok(());
    }
    Ok(Catalog::open_rw()?.mark_removed(uuid)?)
}

/// The live catalog row for the repo whose git dir is `bare_git_dir`, if
/// any. Read-only, and swallows an outage — for callers that only want a
/// hint. Anything gating a mutation wants [`try_live_catalog_row_for`].
pub fn live_catalog_row_for(bare_git_dir: &Path) -> Option<CatalogRepoRow> {
    try_live_catalog_row_for(bare_git_dir).ok().flatten()
}

/// [`live_catalog_row_for`] with the outage kept separate from the answer.
///
/// `Ok(None)` means the catalog was asked and had nothing live to say: no
/// catalog file yet, no row for this repo, or a row that has been tombstoned
/// (a removed repo is deliberately *absence*, not an error — callers fall
/// back to git exactly as they would for a repo daft never cataloged).
/// `Err` means the catalog could not be asked at all — the store failed to
/// open, or its schema is newer than this binary understands, which a sibling
/// worktree running a newer daft build reaches by migrating the shared
/// catalog out from under an older one.
///
/// That distinction is what lets a caller gating a *mutation* fail closed
/// (CLAUDE.md's repo-aware command grammar: a store error must never be
/// silently reinterpreted as "no such repo", which turns an outage into a
/// wrong action). Callers that only want a hint keep using
/// [`live_catalog_row_for`] and its collapsed `Option`.
pub fn try_live_catalog_row_for(
    bare_git_dir: &Path,
) -> crate::catalog::service::Result<Option<CatalogRepoRow>> {
    let Some(catalog) = Catalog::open_ro()? else {
        return Ok(None);
    };
    let daft_id = std::fs::read_to_string(bare_git_dir.join("daft-id"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| uuid::Uuid::parse_str(s).is_ok());
    let row = match daft_id {
        Some(id) => catalog.get_by_uuid(&id)?,
        None => {
            let canonical = bare_git_dir
                .canonicalize()
                .unwrap_or_else(|_| bare_git_dir.to_path_buf());
            catalog.resolve(&canonical.to_string_lossy())?
        }
    };
    Ok(row.filter(|r| r.removed_at.is_none()))
}

fn tombstone_repo_at(bare_git_dir: &Path, project_root: &Path) -> anyhow::Result<Option<String>> {
    let daft_id = std::fs::read_to_string(bare_git_dir.join("daft-id"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| uuid::Uuid::parse_str(s).is_ok());

    if daft_id.is_some() {
        // Repo has an identity: make sure the catalog knows its final facts
        // (registers it if daft never cataloged it) before the tombstone. One
        // writer handle covers both writes — a single logical removal
        // shouldn't build two pools and run the migration check twice.
        let facts = gather_facts(bare_git_dir, project_root, None, None)?;
        let catalog = Catalog::open_rw()?;
        let outcome = catalog.register(&facts)?;
        catalog.mark_removed(&facts.uuid)?;
        Ok(Some(outcome.assigned_name))
    } else {
        // No identity file — nothing to preserve unless a stale row points
        // here; look it up read-only (an uncataloged repo never creates the
        // catalog) while the path still canonicalizes.
        let canonical = bare_git_dir
            .canonicalize()
            .unwrap_or_else(|_| bare_git_dir.to_path_buf());
        let Some((uuid, name)) = Catalog::open_ro()?
            .and_then(|catalog| catalog.resolve(&canonical.to_string_lossy()).ok().flatten())
            .map(|row| (row.uuid, row.name))
        else {
            return Ok(None);
        };
        Catalog::open_rw()?.mark_removed(&uuid)?;
        Ok(Some(name))
    }
}

fn touch_current_repo_impl() -> anyhow::Result<()> {
    let git_common_dir = crate::core::repo::get_git_common_dir()?;
    let project_root = crate::core::repo::get_project_root()?;
    let uuid = compute_repo_id_from_common_dir(&git_common_dir)?;

    // Fast path: one read-only probe. Remote-URL and default-branch drift
    // are deliberately not checked here (each would cost a git subprocess
    // per command); they refresh on the next full registration.
    let canonical_root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.clone());
    let canonical_gcd = git_common_dir
        .canonicalize()
        .unwrap_or_else(|_| git_common_dir.clone());
    if let Ok(Some(catalog)) = Catalog::open_ro()
        && let Ok(Some(row)) = catalog.get_by_uuid(&uuid)
        && row.removed_at.is_none()
        && row.path == canonical_root.to_string_lossy()
        && row.git_common_dir == canonical_gcd.to_string_lossy()
    {
        return Ok(());
    }

    // Unknown or drifted: gather the full facts and write.
    let facts = gather_facts(&git_common_dir, &project_root, None, None)?;
    Catalog::open_rw()?.register(&facts)?;
    Ok(())
}
