//! `daft __dump-store` — internal debug helper that prints rows from
//! the per-repo coordinator store as JSON, one row per line.
//!
//! Subcommands:
//! - `repo-policy` — the single row in `repo_policy` for the repo
//!   identified by the cwd's `.git/daft-id`.
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
        bail!("usage: daft __dump-store <table>\n  tables: repo-policy");
    };
    match table.as_str() {
        "repo-policy" => dump_repo_policy(),
        other => bail!("unknown table '{other}' (supported: repo-policy)"),
    }
}

fn dump_repo_policy() -> Result<()> {
    use crate::coordinator::ports::JobsStorePort;
    let repo_hash = crate::core::repo_identity::compute_repo_id()
        .context("could not compute repo id for cwd")?;
    let state_base = crate::daft_state_dir().context("could not resolve daft state dir")?;
    let base = state_base.join("jobs").join(&repo_hash);
    let store = crate::coordinator::adapters::SqliteJobsStore::for_repo_base(&base)
        .context("open coordinator store")?;
    let policy = store.read_repo_policy(&repo_hash)?;
    let json = serde_json::to_string_pretty(&policy).context("serialize RepoPolicy")?;
    println!("{json}");
    Ok(())
}
