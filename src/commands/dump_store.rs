//! `daft __dump-store` — internal debug helper that prints rows from
//! the per-repo coordinator store as JSON, one row per line.
//!
//! Subcommands:
//! - `repo-policy` — the single row in `repo_policy` for the repo
//!   identified by the cwd's `.git/daft-id`.
//! - `visitor-seeds` — every visitor-config seed row for the repo, one
//!   JSON object per line (`branch_slug`, `filename`, `content`,
//!   timestamps).
//! - `invocations` — every row in `invocations` for that repo, one JSON
//!   object per line (includes the trust-skip records from #596).
//! - `worktree-sizes` — every cached worktree size for the cwd's repo, one
//!   JSON object per line (`branch_slug`, `worktree_path`, `size_bytes`,
//!   `measured_at`). Feeds the size-cache scenarios (#668).
//! - `repo-sizes` — every cached repo size from the global catalog, one
//!   JSON object per line (`uuid`, `repo_path`, `size_bytes`,
//!   `measured_at`).
//!
//! The output is JSON-serialized [`crate::coordinator::clean_policy::RepoPolicy`]
//! (or `{}` when no row exists). Field names match the historical
//! `repo-policy.json` shape so manual-test scenarios that previously
//! grep'd the sidecar continue to work after the cutover.
//!
//! This subcommand is hidden (the `__` prefix excludes it from help
//! output and CLI completions). It exists to give scripts and YAML
//! scenarios a stable read path into the store without re-implementing
//! the deserialization themselves.

use anyhow::{Context, Result, bail};

pub fn run() -> Result<()> {
    let args: Vec<String> = crate::cli::argv().to_vec();
    let Some(table) = args.get(2) else {
        bail!(
            "usage: daft __dump-store <table>\n  tables: repo-policy, visitor-seeds, invocations, worktree-sizes, repo-sizes"
        );
    };
    match table.as_str() {
        "repo-policy" => dump_repo_policy(),
        "visitor-seeds" => dump_visitor_seeds(),
        "invocations" => dump_invocations(),
        "worktree-sizes" => dump_worktree_sizes(),
        "repo-sizes" => dump_repo_sizes(),
        other => {
            bail!(
                "unknown table '{other}' (supported: repo-policy, visitor-seeds, invocations, worktree-sizes, repo-sizes)"
            )
        }
    }
}

fn open_store_for_cwd() -> Result<(String, crate::coordinator::adapters::SqliteJobsStore)> {
    let repo_hash = crate::core::repo_identity::compute_repo_id()
        .context("could not compute repo id for cwd")?;
    let state_base = crate::daft_state_dir().context("could not resolve daft state dir")?;
    let base = state_base.join("jobs").join(&repo_hash);
    let store = crate::coordinator::adapters::SqliteJobsStore::for_repo_base(&base)
        .context("open coordinator store")?;
    Ok((repo_hash, store))
}

fn dump_repo_policy() -> Result<()> {
    use crate::coordinator::ports::JobsStorePort;
    let (repo_hash, store) = open_store_for_cwd()?;
    let policy = store.read_repo_policy(&repo_hash)?;
    let json = serde_json::to_string_pretty(&policy).context("serialize RepoPolicy")?;
    println!("{json}");
    Ok(())
}

fn dump_visitor_seeds() -> Result<()> {
    let git_common_dir = crate::core::repo::get_git_common_dir()
        .context("could not determine git common dir for cwd")?;
    let Some(seeds) = crate::hooks::visitor_seeds::SeedsContext::open(&git_common_dir) else {
        bail!("visitor-seed store unavailable for this repo");
    };
    for row in seeds.list_seeds() {
        let json = serde_json::json!({
            "branch_slug": row.branch_slug,
            "filename": row.filename,
            "content": row.content,
            "seeded_at": row.seeded_at.to_rfc3339(),
            "updated_at": row.updated_at.to_rfc3339(),
        });
        println!("{json}");
    }
    Ok(())
}

fn dump_worktree_sizes() -> Result<()> {
    let (repo_hash, store) = open_store_for_cwd()?;
    let conn = store
        .pool()
        .reader()
        .context("checkout store reader for dump")?;
    let rows = crate::store::repos::WorktreeSizesRepo::list_for_repo(&conn, &repo_hash)?;
    for row in rows {
        let json = serde_json::json!({
            "branch_slug": row.branch_slug,
            "worktree_path": row.worktree_path,
            "size_bytes": row.size_bytes,
            "measured_at": row.measured_at.to_rfc3339(),
        });
        println!("{json}");
    }
    Ok(())
}

fn dump_repo_sizes() -> Result<()> {
    // The repo-size cache lives in the global catalog, not a per-repo store.
    let Some(catalog) = crate::catalog::Catalog::open_ro()? else {
        return Ok(()); // no catalog yet → no rows
    };
    let rows = catalog.list_repo_sizes()?;
    for row in rows {
        let json = serde_json::json!({
            "uuid": row.uuid,
            "repo_path": row.repo_path,
            "size_bytes": row.size_bytes,
            "measured_at": row.measured_at.to_rfc3339(),
        });
        println!("{json}");
    }
    Ok(())
}

fn dump_invocations() -> Result<()> {
    let (repo_hash, store) = open_store_for_cwd()?;
    // `__dump-store` is an application-boundary debug tool — the raw pool
    // checkout is the sanctioned escape hatch for read paths the port
    // doesn't (and shouldn't) surface.
    let conn = store
        .pool()
        .reader()
        .context("checkout store reader for dump")?;
    let rows = crate::store::repos::InvocationsRepo::list_by_repo(&conn, &repo_hash)?;
    for row in rows {
        let json = serde_json::json!({
            "invocation_id": row.invocation_id,
            "trigger_command": row.trigger_command,
            "hook_type": row.hook_type,
            "worktree": row.worktree,
            "created_at": row.created_at.to_rfc3339(),
            "status": row.status,
            "skip_reason": row.skip_reason,
        });
        println!("{json}");
    }
    Ok(())
}
