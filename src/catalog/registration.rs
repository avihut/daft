//! Imperative shell for catalog registration.
//!
//! Registration is deliberately *ambient*: any command that creates or
//! touches a repo keeps the catalog current as a side effect, so `daft repo
//! add` is only ever needed for repos daft has never operated in. Two entry
//! points with different noise contracts:
//!
//! * [`register_repo`] — full-fact registration for commands that just
//!   created or converted a repo (clone, init, layout transform). Best-effort:
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
/// the remote URL (falling back to the project dir's name), and resolves the
/// default branch when the caller doesn't know it.
///
/// The branch ladder mirrors [`crate::catalog::effective_default_branch`], and
/// mirroring it is the point: `origin/HEAD` first, then the repo's own bare
/// `HEAD`. A repo published by hand (`git remote add` + `git push -u`) has no
/// `origin/HEAD` at all — that is #925 — so without the second rung every
/// registration path but `init`/`clone` gathers nothing and the row can only
/// ever be *preserved*, never *corrected*.
///
/// That distinction is why the rung belongs here rather than only at read time.
/// `update_registration` treats `None` as "the caller doesn't know" and keeps
/// what is recorded, so a *stale* branch — recorded before a `git branch -m`,
/// say — would otherwise survive every `repo add`, `repo move`, and
/// `doctor --fix` for the life of the repo, with no command able to fix it.
/// Gathering the fact turns those three back into the repair they read as.
/// Recording it also keeps the read-time rungs off per-repo fan-outs
/// (`exec --all-repos`, `fetch`), which would otherwise re-derive it every run.
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
        .or_else(|| crate::core::remote::local_default_branch(&canonical_root, "origin"))
        .or_else(|| crate::core::remote::local_head_branch(&canonical_gcd));
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

/// Catalog-only removal for a repo daft found on disk but has no live row
/// for — `repo remove`'s default when the entry is missing rather than
/// merely stale. Registration-then-tombstone is the point: a daft-managed
/// repo the catalog never saw (a fresh state dir, a repo carried over from
/// another machine) stays addressable afterwards (`daft hooks jobs --repo
/// <name>`, `daft clone <name>`), which is why removed entries are retained
/// at all. Unlike [`note_repo_removed`] this is the operation the user asked
/// for, so failures propagate. Returns the cataloged name, or `None` when the
/// repo has no identity and no row — nothing to remove.
pub fn remove_from_catalog_only(
    bare_git_dir: &Path,
    project_root: &Path,
) -> anyhow::Result<Option<String>> {
    if !ambient_writes_allowed() {
        return Ok(None);
    }
    tombstone_repo_at(bare_git_dir, project_root)
}

/// Whether [`remove_from_catalog_only`] has anything to record here — the
/// dry-run counterpart, deliberately sharing that function's two conditions
/// (a daft identity to register, or an existing row to tombstone) so the
/// preview cannot drift from the act. Read-only, and ignores the ambient-write
/// gate: a preview is not a write.
pub fn catalog_removal_would_record(bare_git_dir: &Path) -> bool {
    if read_daft_id(bare_git_dir).is_some() {
        return true;
    }
    let canonical = bare_git_dir
        .canonicalize()
        .unwrap_or_else(|_| bare_git_dir.to_path_buf());
    Catalog::open_ro()
        .ok()
        .flatten()
        .and_then(|catalog| catalog.resolve(&canonical.to_string_lossy()).ok().flatten())
        .is_some()
}

/// The repo's daft identity, if it carries one.
fn read_daft_id(bare_git_dir: &Path) -> Option<String> {
    std::fs::read_to_string(bare_git_dir.join("daft-id"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| uuid::Uuid::parse_str(s).is_ok())
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
    live_row_in(&catalog, bare_git_dir)
}

/// [`try_live_catalog_row_for`] against a handle the caller already holds, for
/// resolutions that ask the catalog more than one question and should not pay
/// for — or risk disagreeing across — a second open.
pub fn live_row_in(
    catalog: &Catalog,
    bare_git_dir: &Path,
) -> crate::catalog::service::Result<Option<CatalogRepoRow>> {
    let row = match read_daft_id(bare_git_dir) {
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
    if read_daft_id(bare_git_dir).is_some() {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn git_at(dir: &Path, args: &[&std::ffi::OsStr]) {
        let out = crate::utils::git_command_at(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .expect("git command");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A `contained`-layout repo the way `daft init` leaves one: bare common
    /// dir at `<root>/.git`, a born default branch, and no remote at all.
    fn contained_repo(parent: &Path, branch: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let os = std::ffi::OsStr::new;
        let src = parent.join("src");
        std::fs::create_dir_all(&src).unwrap();
        git_at(&src, &[os("init"), os("-q"), os("-b"), os(branch)]);
        git_at(
            &src,
            &[
                os("commit"),
                os("-q"),
                os("--allow-empty"),
                os("-m"),
                os("init"),
            ],
        );

        let root = parent.join("demo");
        std::fs::create_dir_all(&root).unwrap();
        let gcd = root.join(".git");
        git_at(
            parent,
            &[
                os("clone"),
                os("-q"),
                os("--bare"),
                src.as_os_str(),
                gcd.as_os_str(),
            ],
        );
        // `git clone --bare` leaves an `origin` pointing at the fixture. Strip
        // it: the repo under test is the #925 shape — no remote, and therefore
        // no `origin/HEAD` for the first rung to read.
        git_at(&gcd, &[os("remote"), os("remove"), os("origin")]);
        (gcd, root)
    }

    /// The rung that makes `repo add` / `repo move` / `doctor --fix` able to
    /// *correct* a row rather than only preserve it. Without it these paths
    /// gather `None`, and `update_registration`'s `COALESCE` then keeps
    /// whatever is recorded — including a value that has gone stale.
    #[test]
    fn gather_facts_reads_the_default_branch_from_a_bare_head() {
        let tmp = tempfile::tempdir().unwrap();
        let (gcd, root) = contained_repo(tmp.path(), "master");

        let facts = gather_facts(&gcd, &root, None, None).expect("gather");

        assert_eq!(
            facts.default_branch.as_deref(),
            Some("master"),
            "a repo with no origin/HEAD still declares its default branch in \
             its own bare HEAD"
        );
    }

    /// A branch rename is the case the whole rung exists for: the fact daft
    /// recorded at init is now wrong, and re-registration has to see the new
    /// name to overwrite it.
    #[test]
    fn gather_facts_follows_a_renamed_default_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let os = std::ffi::OsStr::new;
        let (gcd, root) = contained_repo(tmp.path(), "master");
        git_at(&gcd, &[os("branch"), os("-m"), os("master"), os("main")]);

        let facts = gather_facts(&gcd, &root, None, None).expect("gather");

        assert_eq!(facts.default_branch.as_deref(), Some("main"));
    }

    /// An explicitly known branch always wins — the rungs only fill a gap.
    #[test]
    fn gather_facts_prefers_the_branch_the_caller_knows() {
        let tmp = tempfile::tempdir().unwrap();
        let (gcd, root) = contained_repo(tmp.path(), "master");

        let facts = gather_facts(&gcd, &root, None, Some("release".to_string())).expect("gather");

        assert_eq!(facts.default_branch.as_deref(), Some("release"));
    }
}
