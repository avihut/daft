//! Read/write access to the per-repo forge-PR cache — the `forge_prs` table
//! in the repo's coordinator store. Powers the `daft list --columns +pr`
//! decoration (PR number + CI glyph) and `pr:`/`mr:` tab completion.
//!
//! Everything here is **best-effort**, mirroring `size_cache`: the cache is a
//! display/completion accelerator, never a source of truth (the forge is), so
//! every failure — missing store, busy DB, a refresh gap — degrades to "no
//! cache" rather than surfacing an error. Reads never materialize the store.
//!
//! Refresh triggers (never the Tab path, never the `list` render path):
//! - **write-through** when `daft go pr:N` resolves a PR (we hold its data),
//! - **background** ([`spawn_background_refresh`]) after remote-touching
//!   commands and when `daft list` explicitly renders the `pr` column —
//!   detached `daft __refresh-forge`, one `gh pr list` per repo.

use crate::core::worktree::forge_ref::ForgeRefKind;
use crate::forge::{PrListEntry, RemoteRefInfo};
use crate::git::GitCommand;
use crate::store::Pool;
use crate::store::models::ForgePrRow;
use crate::store::paths;
use crate::store::repos::{ForgePrsRepo, with_write_txn};
use std::process::{Command, Stdio};

/// Strip a forge title down to terminal-safe text: control characters
/// (including ESC, killing ANSI sequences, and the tab/newline that would
/// corrupt the completion protocol's field/line framing) become spaces, runs
/// of whitespace collapse. Titles are attacker-influenced — anyone can open a
/// PR against a public repo — and every reader renders them into a terminal
/// or a shell completion stream, so this runs at the persistence boundary and
/// readers trust the store.
pub(crate) fn sanitize_title(title: &str) -> String {
    title
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Every cached PR/MR for `repo_hash`, open first (the completion order).
/// Empty on any error and — deliberately — when the coordinator store doesn't
/// exist yet: reading is pure, only persists create the store. Runs on the
/// reader pool (300ms busy_timeout) so completion/list paths fail fast
/// instead of blocking the shell.
pub fn read_prs(repo_hash: &str) -> Vec<ForgePrRow> {
    read_inner(repo_hash).unwrap_or_default()
}

fn read_inner(repo_hash: &str) -> Option<Vec<ForgePrRow>> {
    let state_dir = crate::daft_state_dir().ok()?;
    let db_path = state_dir
        .join(paths::JOBS_SUBDIR)
        .join(repo_hash)
        .join(paths::COORDINATOR_DB);
    // Same WAL-safe read shape as size_cache::read_inner: existence-gate so a
    // pure read never materializes a store, then the pool's read-write
    // bootstrap (a checkpointed db with no -wal/-shm sidecars is
    // SQLITE_CANTOPEN under a bare read-only open).
    if !db_path.exists() {
        return None;
    }
    let pool = Pool::open(&db_path).ok()?;
    let conn = pool.reader().ok()?;
    ForgePrsRepo::list_for_repo(&conn, repo_hash).ok()
}

/// Build the PR-column lookup for `repo_hash` from the cache. Outbound
/// matches (`by_branch`) take only open same-repo PRs — a stranger's fork
/// branch with a colliding name or a merged PR must not decorate a local
/// branch; inbound CI (`ci_by_ref`) covers every cached row so a checked-out
/// PR shows its CI regardless of state. Same filters as the SQL in
/// `ForgePrsRepo::by_head_branch`, applied here because one bulk read beats
/// a query per row.
pub fn load_lookup(repo_hash: &str) -> crate::core::worktree::forge_ref::ForgePrLookup {
    use crate::core::worktree::forge_ref::{CiStatus, ForgeBranchRef, ForgePrLookup};

    let mut lookup = ForgePrLookup::default();
    for row in read_prs(repo_hash) {
        let kind = match row.kind.as_str() {
            "pr" => ForgeRefKind::GithubPr,
            "mr" => ForgeRefKind::GitlabMr,
            _ => continue,
        };
        let forge_ref = ForgeBranchRef::new(kind, row.number);
        let ci = row.ci_status.as_deref().and_then(CiStatus::parse);
        lookup.ci_by_ref.insert(forge_ref, ci);
        if row.state == "open" && !row.is_cross_repo {
            lookup
                .by_branch
                .insert(row.head_branch.clone(), (forge_ref, ci));
        }
    }
    lookup
}

/// Replace the cached snapshot for one `(repo, kind)` with `entries`, in a
/// single transaction. Best-effort: creates the store if missing, swallows
/// any failure. Titles are sanitized here.
pub fn persist_snapshot(repo_hash: &str, kind: ForgeRefKind, entries: &[PrListEntry]) {
    let fetched_at = chrono::Utc::now();
    let rows: Vec<ForgePrRow> = entries
        .iter()
        .map(|entry| ForgePrRow {
            repo_hash: repo_hash.to_string(),
            kind: entry.kind.tag().to_string(),
            number: entry.number,
            title: sanitize_title(&entry.title),
            state: entry.state.clone(),
            head_branch: entry.head_branch.clone(),
            is_cross_repo: entry.is_cross_repo,
            ci_status: entry.ci_status.map(|s| s.as_str().to_string()),
            url: entry.url.clone(),
            author: entry.author.clone(),
            fetched_at,
        })
        .collect();
    let _ = persist_inner(repo_hash, kind.tag(), &rows);
}

fn persist_inner(repo_hash: &str, kind: &str, rows: &[ForgePrRow]) -> anyhow::Result<()> {
    let db_path = paths::for_repo(repo_hash)?;
    let pool = Pool::open(&db_path)?;
    let mut conn = pool.writer()?;
    // Fail fast instead of holding an interactive command for the writer
    // pool's full timeout when a coordinator holds the write lock — the next
    // refresh simply supersedes this one (same rationale as size_cache).
    conn.busy_timeout(std::time::Duration::from_millis(
        crate::store::connection::READER_BUSY_TIMEOUT_MS as u64,
    ))?;
    with_write_txn(&mut conn, |tx| {
        ForgePrsRepo::replace_snapshot(tx, repo_hash, kind, rows)
    })?;
    Ok(())
}

/// Write-through from a `daft go pr:N` resolve: we already hold that PR's
/// fresh metadata, so record it without waiting for a wholesale refresh. CI
/// status is unknown on this path (the single-PR resolve carries no check
/// rollup) — recorded as `None`, corrected by the next snapshot refresh.
/// Best-effort; never delays or fails the checkout.
pub fn persist_resolved(info: &RemoteRefInfo) {
    let Ok(repo_hash) = crate::core::repo_identity::compute_repo_id() else {
        return;
    };
    let row = ForgePrRow {
        repo_hash: repo_hash.clone(),
        kind: info.kind.tag().to_string(),
        number: info.number,
        title: sanitize_title(&info.title),
        state: info.state.clone(),
        head_branch: info.source_branch.clone(),
        is_cross_repo: info.is_cross_repo,
        ci_status: None,
        url: info.url.clone(),
        author: info.author.clone(),
        fetched_at: chrono::Utc::now(),
    };
    let _ = persist_one_inner(&repo_hash, &row);
}

fn persist_one_inner(repo_hash: &str, row: &ForgePrRow) -> anyhow::Result<()> {
    let db_path = paths::for_repo(repo_hash)?;
    let pool = Pool::open(&db_path)?;
    let conn = pool.writer()?;
    conn.busy_timeout(std::time::Duration::from_millis(
        crate::store::connection::READER_BUSY_TIMEOUT_MS as u64,
    ))?;
    ForgePrsRepo::upsert(&conn, row)?;
    Ok(())
}

/// Spawn a detached `daft __refresh-forge` for the repo the cwd is in.
/// Fire-and-forget: the caller's command completes regardless. Gated by
/// [`crate::should_skip_background_tasks`] so agent/test invocations and
/// coordinator children never fan out network work.
pub fn spawn_background_refresh() {
    if crate::should_skip_background_tasks(crate::cli::argv()) {
        return;
    }
    let _ = spawn_inner();
}

fn spawn_inner() -> anyhow::Result<()> {
    // canonicalize() is load-bearing: invoked via a symlink (git-worktree-sync
    // et al.), current_exe() is the symlink and spawning it would dispatch the
    // child through that command's arm instead of `daft __refresh-forge`.
    let exe = std::env::current_exe()?.canonicalize()?;
    Command::new(exe)
        .arg("__refresh-forge")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

/// Entry point for the `daft __refresh-forge` background process: one
/// `gh pr list` / `glab api` listing for the cwd's repo, persisted as the new
/// snapshot. Errors are silently swallowed by the caller (the process is
/// detached; there is nowhere useful to report).
pub fn run_refresh_forge() -> anyhow::Result<()> {
    // Detach from the parent's session/TTY per the spawn-self contract.
    nix::unistd::setsid().ok();
    let project_root = crate::core::repo::get_project_root()?;
    let repo_hash = crate::core::repo_identity::compute_repo_id()?;
    let git = GitCommand::new(true);
    let config = crate::forge::ForgeConfig::load(&git);
    let (kind, entries) = crate::forge::fetch_open_snapshot(&git, &project_root, &config)?;
    persist_snapshot(&repo_hash, kind, &entries);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::CiStatus;
    use serial_test::serial;

    fn entry(number: u32, head: &str, ci: Option<CiStatus>) -> PrListEntry {
        PrListEntry {
            kind: ForgeRefKind::GithubPr,
            number,
            title: format!("feat: change {number}"),
            state: "open".into(),
            head_branch: head.into(),
            is_cross_repo: false,
            ci_status: ci,
            url: format!("https://github.com/acme/widget/pull/{number}"),
            author: "octocat".into(),
        }
    }

    #[test]
    fn sanitize_strips_controls_and_collapses_whitespace() {
        assert_eq!(
            sanitize_title("fix:\tbad\r\ntitle \x1b[31mred\x1b[0m  end"),
            "fix: bad title [31mred [0m end"
        );
        assert_eq!(sanitize_title("  plain title  "), "plain title");
    }

    #[test]
    #[serial]
    fn persist_snapshot_then_read_round_trips() {
        let _guard = crate::store::paths::IsolatedStateDir::new();
        let repo = "forge-repo-1";
        persist_snapshot(
            repo,
            ForgeRefKind::GithubPr,
            &[
                entry(7, "feat/x", Some(CiStatus::Pass)),
                entry(9, "feat/y", None),
            ],
        );

        let rows = read_prs(repo);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].number, 9, "open PRs come newest-first");
        assert_eq!(rows[1].ci_status.as_deref(), Some("pass"));
        assert_eq!(rows[0].ci_status, None);
    }

    #[test]
    #[serial]
    fn snapshot_replaces_vanished_prs() {
        let _guard = crate::store::paths::IsolatedStateDir::new();
        let repo = "forge-repo-2";
        persist_snapshot(repo, ForgeRefKind::GithubPr, &[entry(7, "feat/x", None)]);
        persist_snapshot(repo, ForgeRefKind::GithubPr, &[entry(9, "feat/y", None)]);

        let numbers: Vec<u32> = read_prs(repo).iter().map(|r| r.number).collect();
        assert_eq!(numbers, vec![9], "a merged-and-gone PR leaves the cache");
    }

    #[test]
    #[serial]
    fn persisted_titles_are_sanitized() {
        let _guard = crate::store::paths::IsolatedStateDir::new();
        let repo = "forge-repo-3";
        let mut evil = entry(7, "feat/x", None);
        evil.title = "evil\x1b]0;owned\x07\ntitle".into();
        persist_snapshot(repo, ForgeRefKind::GithubPr, &[evil]);

        let rows = read_prs(repo);
        assert!(
            !rows[0].title.chars().any(|c| c.is_control()),
            "stored title must be terminal-safe, got {:?}",
            rows[0].title
        );
    }

    #[test]
    #[serial]
    fn read_missing_store_is_empty_and_creates_nothing() {
        let _guard = crate::store::paths::IsolatedStateDir::new();
        assert!(read_prs("never-seen").is_empty());
    }

    #[test]
    #[serial]
    fn load_lookup_filters_outbound_but_keeps_inbound_ci() {
        use crate::core::worktree::forge_ref::{CiStatus as Ci, ForgeBranchRef, ForgeRefKind as K};
        let _guard = crate::store::paths::IsolatedStateDir::new();
        let repo = "forge-repo-4";

        let mut fork = entry(8, "feat/x", Some(CiStatus::Fail));
        fork.is_cross_repo = true;
        let mut merged = entry(6, "feat/done", Some(CiStatus::Pass));
        merged.state = "merged".into();
        persist_snapshot(
            repo,
            ForgeRefKind::GithubPr,
            &[entry(7, "feat/x", Some(CiStatus::Pass)), fork, merged],
        );

        let lookup = load_lookup(repo);

        // Outbound: only the open same-repo PR decorates the branch.
        let (r, ci) = lookup.by_branch["feat/x"];
        assert_eq!(r, ForgeBranchRef::new(K::GithubPr, 7));
        assert_eq!(ci, Some(Ci::Pass));
        assert!(
            !lookup.by_branch.contains_key("feat/done"),
            "merged PRs don't decorate branches"
        );

        // Inbound CI is available for every cached row, fork/merged included.
        assert_eq!(
            lookup.ci_by_ref[&ForgeBranchRef::new(K::GithubPr, 8)],
            Some(Ci::Fail)
        );
        assert_eq!(
            lookup.ci_by_ref[&ForgeBranchRef::new(K::GithubPr, 6)],
            Some(Ci::Pass)
        );
    }
}
