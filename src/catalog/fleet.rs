//! Fleet iteration: run a repo-scoped action across cataloged repos.
//!
//! The mechanism mirrors the `-C <path>` global flag — resolve a catalog
//! entry to its path, chdir, run the existing single-repo logic, restore —
//! so fleet flags stay thin wrappers around each command's current-repo
//! body. Per-repo failures are collected, not cascaded: a fleet sweep
//! reports what broke and keeps going (callers exit non-zero when
//! `failed` is non-empty).

use crate::output::Output;
use crate::store::CatalogRepoRow;
use anyhow::Result;
use std::path::Path;

/// Which repos a fleet invocation covers.
pub enum FleetScope {
    /// `--repo <needle>` — exactly one, resolved loudly.
    Single(String),
    /// `--all-repos` — every live catalog entry.
    AllRepos,
}

#[derive(Default)]
pub struct FleetOutcome {
    /// Repos the action actually ran in.
    pub ran: usize,
    /// `(name, reason)` for repos skipped before running (stale path…).
    pub skipped: Vec<(String, String)>,
    /// `(name, error)` for repos where the action itself failed.
    pub failed: Vec<(String, anyhow::Error)>,
}

impl FleetOutcome {
    /// Bail when any repo failed — the standard fleet exit policy.
    pub fn into_result(self) -> Result<()> {
        if self.failed.is_empty() {
            return Ok(());
        }
        anyhow::bail!("failed in {} repo(s)", self.failed.len())
    }
}

/// Run `action` inside every repo in `scope`. Prints a `── name ──` header
/// between repos on multi-repo runs, warns on skips (never silent), and
/// reports each failure as it happens. `current_repo_last` reorders the
/// sweep so the repo containing the cwd runs last — required when the
/// action can invalidate the cwd (prune) so the cd-redirect semantics of
/// the current repo stay exactly as in a single-repo run.
pub fn for_each_repo(
    scope: FleetScope,
    current_repo_last: bool,
    output: &mut dyn Output,
    mut action: impl FnMut(&CatalogRepoRow) -> Result<()>,
) -> Result<FleetOutcome> {
    let mut rows = match &scope {
        FleetScope::Single(needle) => vec![crate::catalog::resolve_repo_arg(needle)?],
        FleetScope::AllRepos => {
            // open_ro contract: a transient open error degrades to "no
            // catalog" (the empty-catalog bail below), never a hard failure.
            let rows = match crate::catalog::Catalog::open_ro().ok().flatten() {
                Some(catalog) => catalog.list(false)?,
                None => Vec::new(),
            };
            if rows.is_empty() {
                anyhow::bail!(
                    "the repo catalog is empty — clone a repo or run `{}` first",
                    crate::daft_cmd("repo add")
                );
            }
            rows
        }
    };

    if current_repo_last && let Ok(git_dir) = crate::core::repo::get_git_common_dir() {
        let canonical = git_dir
            .canonicalize()
            .unwrap_or(git_dir)
            .to_string_lossy()
            .into_owned();
        rows.sort_by_key(|row| row.git_common_dir == canonical);
    }

    let multi = rows.len() > 1;
    let original = crate::utils::get_current_directory()?;
    let mut outcome = FleetOutcome::default();

    for row in &rows {
        let path = Path::new(&row.path);
        if !path.is_dir() {
            output.warning(&format!(
                "skipped '{}' (path missing: {})",
                row.name, row.path
            ));
            outcome
                .skipped
                .push((row.name.clone(), format!("path missing: {}", row.path)));
            continue;
        }
        if multi {
            output.raw(&format!("── {} ──", row.name));
        }
        crate::utils::change_directory(path)?;
        // `action` is assumed not to unwind; a panic here would skip the
        // restore below and strand the process in `path`.
        let result = action(row);
        // Best-effort restore: the action may have deleted `original` (e.g.
        // `prune --all-repos` removing the cwd's own worktree — which
        // `current_repo_last` orders to run last). A failed restore must not
        // turn a fully successful sweep into a failure; the next iteration
        // chdir's by absolute path and the shell cd-redirect owns the user's
        // final cwd.
        let _ = crate::utils::change_directory(&original);
        match result {
            Ok(()) => outcome.ran += 1,
            Err(e) => {
                output.warning(&format!("'{}' failed: {e}", row.name));
                outcome.failed.push((row.name.clone(), e));
            }
        }
    }
    Ok(outcome)
}
