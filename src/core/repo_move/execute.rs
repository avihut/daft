//! Applying a move plan.
//!
//! Order is load-bearing. Directories move first and roll back as a unit if
//! any rename fails; git's linkage is repaired immediately afterwards, because
//! between the two every relocated worktree reads as `prunable` and a prune in
//! that window would delete its administrative entry. Only then does the
//! path-keyed metadata follow.
//!
//! Once the files have moved, nothing rolls back. A metadata step that fails
//! is reported with the command to re-run: rolling the directories back at
//! that point would undo a repair that already succeeded, which is a worse
//! state than the one being corrected.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::plan::{MovePlan, NamePlan};
use crate::catalog::Catalog;
use crate::core::worktree::identity_store::IdentityStore;
use crate::hooks::TrustDatabase;

/// What actually happened.
#[derive(Debug, Default)]
pub struct MoveOutcome {
    pub repaired: usize,
    /// The trust grant and the layout override are reported apart: they are
    /// keyed alike, but only the first is a statement about hooks running.
    pub trust_rekeyed: bool,
    pub layout_rekeyed: bool,
    pub renamed_to: Option<String>,
    /// Problems that did not stop the move but that the user has to know
    /// about. Rendered after the summary.
    pub warnings: Vec<String>,
    /// Whether something the repo's correctness depends on failed — git
    /// linkage or the catalog. The caller exits non-zero on this.
    pub degraded: bool,
}

impl MoveOutcome {
    fn warn(&mut self, message: String) {
        self.warnings.push(message);
    }

    fn fail(&mut self, message: String) {
        self.degraded = true;
        self.warnings.push(message);
    }
}

/// Carry out `plan`. The directories move or nothing does; after that, every
/// remaining step reports rather than aborts.
pub fn apply(plan: &MovePlan) -> Result<MoveOutcome> {
    move_directories(plan)?;

    let mut outcome = MoveOutcome::default();
    repair_worktrees(plan, &mut outcome);
    rekey_trust(plan, &mut outcome);
    // Ahead of the catalog, and not inside it: the recorded paths are this
    // repo's own live data and need nothing from the catalog, so gating them on
    // a catalog write would let one outage strand both. The remedy printed for
    // a catalog failure (`daft repo add` from the new location) does not
    // rewrite these rows, so a skip here would be permanent.
    rewrite_recorded_paths(plan);
    update_catalog(plan, &mut outcome);
    Ok(outcome)
}

/// Rename the root, then each worktree the layout places outside it.
///
/// All-or-nothing: a failure part-way renames everything already done back
/// where it came from, using the plan's own record. Re-enumerating is not an
/// option — the root has moved, so the repo cannot be resolved to ask git
/// anything.
fn move_directories(plan: &MovePlan) -> Result<()> {
    let moves = plan.directory_moves();
    let mut done: Vec<(&Path, &Path)> = Vec::with_capacity(moves.len());
    let mut created: Vec<PathBuf> = Vec::new();

    for (from, to) in moves {
        // Worktree destinations are computed by the layout, not typed by the
        // user, so daft owns them and creates them — `centralized` re-roots
        // every worktree under `<data>/worktrees/<repo-name>/`, a directory
        // that does not exist yet when the move also renames the repo. The
        // *root's* parent is deliberately not created: that one the user named,
        // and preflight already refused it with `mv`'s "no such directory".
        if to != plan.new_root
            && let Some(parent) = to.parent()
            && !parent.is_dir()
        {
            if let Err(e) = std::fs::create_dir_all(parent) {
                rollback(&done, &created);
                return Err(anyhow::Error::new(e)).with_context(|| {
                    String::from("could not create ") + &parent.display().to_string()
                });
            }
            created.push(parent.to_path_buf());
        }
        match std::fs::rename(from, to) {
            Ok(()) => done.push((from, to)),
            Err(e) => {
                rollback(&done, &created);
                let hint = if e.kind() == std::io::ErrorKind::CrossesDevices {
                    "\n  a move across filesystems is a copy; copy it yourself, then run `"
                        .to_string()
                        + &crate::daft_cmd("repo add")
                        + "` from the new location"
                } else {
                    String::new()
                };
                return Err(anyhow::Error::new(e)).with_context(|| {
                    String::from("could not move ")
                        + &from.display().to_string()
                        + " to "
                        + &to.display().to_string()
                        + &hint
                });
            }
        }
    }
    Ok(())
}

/// Undo the renames that did happen, newest first.
///
/// Best-effort by necessity: this already runs on a failure path, and there is
/// nothing left to try if the reverse rename also fails. It is reported rather
/// than swallowed, because a partial rollback is exactly the state a user must
/// be told about.
fn rollback(done: &[(&Path, &Path)], created: &[PathBuf]) {
    for (from, to) in done.iter().rev() {
        if let Err(e) = std::fs::rename(to, from) {
            eprintln!(
                "warning: could not move {} back to {}: {e}",
                to.display(),
                from.display()
            );
        }
    }
    // Drop the directories this move made, newest first. `remove_dir` refuses a
    // non-empty directory, which is the check we want: anything still in there
    // predates us or failed to roll back, and must not be deleted.
    for dir in created.iter().rev() {
        let _ = std::fs::remove_dir(dir);
    }
}

/// Point git at the new locations.
///
/// Every worktree is named explicitly, at its post-move path. A bare
/// `git worktree repair` is a silent no-op here: it finds worktrees through
/// the administrative `gitdir` files, which still point at paths that no
/// longer exist. Left-in-place worktrees are in the list too — they did not
/// move, but their `.git` gitlink points into the old git dir.
///
/// Subprocess `git`: gitoxide has no equivalent, and this is a deliberate
/// decision rather than an accidental one.
fn repair_worktrees(plan: &MovePlan, outcome: &mut MoveOutcome) {
    let paths = plan.repair_paths();
    if paths.is_empty() {
        return;
    }
    let mut cmd = crate::utils::git_command_at(&plan.new_root);
    cmd.arg("--git-dir").arg(&plan.new_git_dir);
    cmd.args(["worktree", "repair"]);
    for path in &paths {
        cmd.arg(path);
    }

    match cmd.output() {
        Ok(out) if out.status.success() => outcome.repaired = paths.len(),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            outcome.fail(
                String::from("git could not repair the worktrees: ")
                    + &stderr
                    + "\n  the directories moved; re-run the repair with:\n    git --git-dir "
                    + &plan.new_git_dir.display().to_string()
                    + " worktree repair <path>...",
            );
        }
        Err(e) => {
            outcome.fail(String::from("could not run `git worktree repair`: ") + &e.to_string())
        }
    }
}

/// Carry the trust grant and layout override to the new git-dir key.
///
/// Losing these is the worst of the silent failures a plain `mv` causes: hooks
/// stop running with nothing but a skip warning to show for it.
fn rekey_trust(plan: &MovePlan, outcome: &mut MoveOutcome) {
    // The closure's own answer, not a re-read of the registry: an entry sitting
    // at the destination key could be one this repo brought or one stranded
    // there by whatever used to occupy that path, and reporting "trust carried"
    // for the latter would be a false reassurance about the one thing a plain
    // `mv` silently destroys. `update_if` rather than `update` so a repo that
    // never had a grant does not get a `repos.json` created for it.
    let mut moved = crate::hooks::Rekeyed::default();
    let result = TrustDatabase::update_if(|db| {
        moved = db.rekey_repo(&plan.old_trust_key, &plan.new_git_dir);
        Ok(moved.changed())
    });
    match result {
        Ok(()) => {
            outcome.trust_rekeyed = moved.trust;
            outcome.layout_rekeyed = moved.layout;
        }
        Err(e) => outcome.warn(
            String::from("could not move the trust grant to the new location: ")
                + &e.to_string()
                + "\n  re-grant it with `"
                + &crate::daft_cmd("hooks trust")
                + "` from the repo",
        ),
    }
}

/// Update the catalog's path, then apply the name.
///
/// `register` keys on the repo's uuid — which travels inside the git dir — so
/// this updates the existing row's `path` and `git_common_dir` in place rather
/// than adding a second one, and its `retire_live_at_path` transition clears
/// any stale live entry recorded at the destination.
fn update_catalog(plan: &MovePlan, outcome: &mut MoveOutcome) {
    let facts = match crate::catalog::gather_facts(&plan.new_git_dir, &plan.new_root, None, None) {
        Ok(facts) => facts,
        Err(e) => {
            outcome.fail(
                String::from("could not read the moved repository: ")
                    + &e.to_string()
                    + "\n  the catalog still points at the old path; run `"
                    + &crate::daft_cmd("repo add")
                    + "` from the new location",
            );
            return;
        }
    };

    let catalog = match Catalog::open_rw() {
        Ok(catalog) => catalog,
        Err(e) => {
            outcome.fail(
                String::from("could not open the repo catalog: ")
                    + &e.to_string()
                    + "\n  the catalog still points at the old path; run `"
                    + &crate::daft_cmd("repo add")
                    + "` from the new location",
            );
            return;
        }
    };

    if let Err(e) = catalog.register(&facts) {
        outcome.fail(
            String::from("could not update the catalog entry: ")
                + &e.to_string()
                + "\n  run `"
                + &crate::daft_cmd("repo add")
                + "` from the new location",
        );
        return;
    }

    if let Some(new_name) = plan.name.rename_to() {
        match catalog.rename(&facts.uuid, new_name) {
            Ok(()) => outcome.renamed_to = Some(new_name.to_string()),
            Err(e) => outcome.warn(
                String::from("moved, but could not rename the catalog entry: ") + &e.to_string(),
            ),
        }
    }
    if let NamePlan::KeptNameTaken { wanted, holder } = &plan.name {
        outcome.warn(
            String::from("kept the name '")
                + &plan.current_name
                + "': '"
                + wanted
                + "' is already used by the repo at "
                + holder,
        );
    }

    evict_caches(&catalog, &facts.uuid);
}

/// Re-point recorded worktree identities. Live data, not a cache: a stale row
/// makes `daft list` name a worktree by a directory that no longer exists.
///
/// Runs after the repair so the private-gitdir id can be read back off each
/// moved worktree.
fn rewrite_recorded_paths(plan: &MovePlan) {
    let Some(store) = IdentityStore::open(&plan.new_git_dir) else {
        return;
    };
    for worktree in &plan.worktrees {
        store.rewrite_path(&worktree.new_path);
    }
}

/// Drop both size caches. Every cached figure is keyed to a path that no
/// longer exists, and a miss just re-walks.
fn evict_caches(catalog: &Catalog, uuid: &str) {
    crate::commands::size_cache::evict_repo_sizes(uuid);
    let _ = catalog.delete_repo_size(uuid);
}
