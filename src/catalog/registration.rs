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
    let _ = note_repo_removed_impl(bare_git_dir, project_root);
}

fn note_repo_removed_impl(bare_git_dir: &Path, project_root: &Path) -> anyhow::Result<()> {
    let daft_id = std::fs::read_to_string(bare_git_dir.join("daft-id"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| uuid::Uuid::parse_str(s).is_ok());

    let uuid = if daft_id.is_some() {
        // Repo has an identity: make sure the catalog knows its final facts
        // (registers it if daft never cataloged it) before the tombstone.
        let facts = gather_facts(bare_git_dir, project_root, None, None)?;
        let catalog = Catalog::open_rw()?;
        catalog.register(&facts)?;
        Some(facts.uuid)
    } else {
        // No identity file — nothing to preserve unless a stale row points
        // here; look it up while the path still canonicalizes.
        let canonical = bare_git_dir
            .canonicalize()
            .unwrap_or_else(|_| bare_git_dir.to_path_buf());
        Catalog::open_ro()?
            .and_then(|catalog| catalog.resolve(&canonical.to_string_lossy()).ok().flatten())
            .map(|row| row.uuid)
    };

    if let Some(uuid) = uuid {
        Catalog::open_rw()?.mark_removed(&uuid)?;
    }
    Ok(())
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
