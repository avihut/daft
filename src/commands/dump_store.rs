//! `daft __dump-store` — internal debug helper that prints rows from
//! the per-repo coordinator store as JSON, one row per line.
//!
//! Subcommands:
//! - `repo-policy` — the single row in `repo_policy` for the repo
//!   identified by the cwd's `.git/daft-id`.
//! - `invocations` — every row in `invocations` for that repo, one JSON
//!   object per line (includes the trust-skip records from #596).
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
        bail!("usage: daft __dump-store <table>\n  tables: repo-policy, invocations");
    };
    match table.as_str() {
        "repo-policy" => dump_repo_policy(),
        "invocations" => dump_invocations(),
        other => bail!("unknown table '{other}' (supported: repo-policy, invocations)"),
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
