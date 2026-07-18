//! Read/write access to the per-repo forge-PR cache — the `forge_prs` table
//! in the repo's coordinator store. Powers the `daft list --columns +pr`
//! decoration (PR number + open/merged/CI fate) and `pr:`/`mr:` tab
//! completion.
//!
//! Everything here is **best-effort**, mirroring `size_cache`: the cache is a
//! display/completion accelerator, never a source of truth (the forge is), so
//! every failure — missing store, busy DB, a refresh gap — degrades to "no
//! cache" rather than surfacing an error. Reads never materialize the store.
//!
//! Refresh triggers (never the Tab path, never the `list` render path):
//! - **write-through** when `daft go pr:N` resolves a PR (we hold its data),
//! - **background** ([`spawn_background_refresh`]) after remote-touching
//!   commands and on every `daft list` with the `pr` column in play (a
//!   default in forge-capable repos) — detached `daft __refresh-forge`, one
//!   `gh pr list` per repo. One refresh at a time: concurrent invocations
//!   attach to the in-flight one rather than stacking calls, but there is
//!   no freshness window — every list re-verifies.
//!
//! Each refresh also records **forge health** ([`ForgeGate`]): a deep failure
//! (missing tool, dead auth, lost repo access) silently hides the default
//! `pr` column until a later refresh — or a successful `pr:` checkout —
//! proves the forge reachable again.

use crate::core::worktree::forge_ref::ForgeRefKind;
use crate::forge::{PrListEntry, RemoteRefInfo};
use crate::git::GitCommand;
use crate::store::Pool;
use crate::store::models::{ForgeHealthRow, ForgePrRow};
use crate::store::paths;
use crate::store::repos::{ForgeHealthRepo, ForgePrsRepo, with_write_txn};
use std::process::{Command, Stdio};

/// Age past which an *unconcluded* refresh attempt is presumed dead (a
/// crashed child) rather than in flight. Freshness is deliberately not
/// time-gated — every `daft list` re-verifies, so a status never renders
/// without an active verification behind it; the only spawn guard is
/// one-refresh-at-a-time, and this horizon is what keeps a crashed child's
/// start stamp from blocking that guard forever.
const REFRESH_LIVENESS_SECS: i64 = 60;

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

/// Keep a PR URL only when it's a plausible web link that can be embedded in
/// an OSC 8 hyperlink without breaking out of the sequence: http(s) scheme,
/// no control characters (ESC/BEL terminate an OSC payload early) and no
/// whitespace. Anything else persists as the empty string, which readers
/// treat as "no link". Runs at the persistence boundary like
/// [`sanitize_title`].
pub(crate) fn sanitize_url(url: &str) -> String {
    let clean = url.trim();
    let ok = (clean.starts_with("https://") || clean.starts_with("http://"))
        && !clean.chars().any(|c| c.is_control() || c.is_whitespace());
    if ok { clean.to_string() } else { String::new() }
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
/// matches (`by_branch`) take same-repo open and merged PRs — open beats
/// merged, newer number wins — while fork branches with colliding names and
/// closed-unmerged PRs never map; inbound decorations (`by_ref`) cover every
/// cached row so a checked-out PR shows its fate regardless of state. Same
/// rules as the SQL in `ForgePrsRepo::by_head_branch`, applied here because
/// one bulk read beats a query per row.
pub fn load_lookup(repo_hash: &str) -> crate::core::worktree::forge_ref::ForgePrLookup {
    use crate::core::worktree::forge_ref::{
        CiStatus, ForgeBranchRef, ForgePrLookup, OpenPr, PrDecoration, PrStatus,
    };

    let mut lookup = ForgePrLookup::default();
    // `read_prs` orders open-first, newest-number-first within a state, so a
    // first-wins insert per branch realizes the open-beats-merged priority —
    // and `open` comes out newest-number-first.
    for row in read_prs(repo_hash) {
        let kind = match row.kind.as_str() {
            "pr" => ForgeRefKind::GithubPr,
            "mr" => ForgeRefKind::GitlabMr,
            _ => continue,
        };
        let forge_ref = ForgeBranchRef::new(kind, row.number);
        let ci = row.ci_status.as_deref().and_then(CiStatus::parse);
        let decoration = PrDecoration {
            r: forge_ref,
            status: Some(PrStatus::from_state_and_ci(&row.state, ci)),
            url: (!row.url.is_empty()).then(|| row.url.clone()),
            author: (!row.author.is_empty()).then(|| row.author.clone()),
        };
        if matches!(row.state.as_str(), "open" | "merged") && !row.is_cross_repo {
            lookup
                .by_branch
                .entry(row.head_branch.clone())
                .or_insert_with(|| decoration.clone());
        }
        if row.state == "open" {
            lookup.open.push(OpenPr {
                decoration: decoration.clone(),
                head_branch: row.head_branch.clone(),
                is_cross_repo: row.is_cross_repo,
                head_repo_owner: row.head_repo_owner.clone(),
                title: row.title.clone(),
                updated_at: row.updated_at,
            });
        }
        lookup.by_ref.insert(forge_ref, decoration);
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
            url: sanitize_url(&entry.url),
            // Logins render into the Owner column and the fork row's
            // `owner:branch` cell — same boundary rule as titles.
            author: sanitize_title(&entry.author),
            head_repo_owner: sanitize_title(&entry.head_repo_owner),
            updated_at: entry.updated_at,
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
/// A successful resolve also proves the forge reachable, so it flips the
/// repo healthy (restoring a hidden `pr` column) without claiming a
/// snapshot. Best-effort; never delays or fails the checkout.
pub fn persist_resolved(info: &RemoteRefInfo) {
    let Ok(repo_hash) = crate::core::repo_identity::compute_repo_id() else {
        return;
    };
    let _ = with_health_writer(&repo_hash, ForgeHealthRepo::record_healthy);
    let row = ForgePrRow {
        repo_hash: repo_hash.clone(),
        kind: info.kind.tag().to_string(),
        number: info.number,
        title: sanitize_title(&info.title),
        state: info.state.clone(),
        head_branch: info.source_branch.clone(),
        is_cross_repo: info.is_cross_repo,
        ci_status: None,
        url: sanitize_url(&info.url),
        author: sanitize_title(&info.author),
        // The single-PR resolve doesn't carry these; the next snapshot
        // refresh fills them (same treatment as ci_status above).
        head_repo_owner: String::new(),
        updated_at: None,
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

/// The repo-level verdict deciding whether forge-derived UI is in play:
/// capability (does this repo name a forge at all) plus persisted health
/// (did the last refresh die a death only the user can fix). Computed once
/// per command and shared by `daft list`'s column gate, the refresh spawn,
/// and the live table's seed state.
#[derive(Debug, Clone)]
pub struct ForgeGate {
    /// The repo names a forge — a known remote host or an explicit
    /// `daft.forge.platform` override.
    pub capable: bool,
    pub repo_hash: Option<String>,
    /// Persisted health as of the gate read. `None` when no refresh ever
    /// ran (or the store is unreadable) — which fails open, i.e. healthy.
    pub health: Option<ForgeHealthRow>,
}

impl ForgeGate {
    /// Whether the *default-sourced* `pr` column shows. Capability is the
    /// instant local signal (a repo with no forge remote never grows the
    /// column); a persisted deep failure silently hides it until a later
    /// refresh succeeds. An explicit `+pr` bypasses this entirely.
    pub fn column_visible(&self) -> bool {
        self.capable && self.health.as_ref().is_none_or(|h| h.healthy)
    }

    /// Whether any snapshot was ever taken. `false` drives the PR column's
    /// first-load skeleton while the first refresh is in flight.
    pub fn ever_succeeded(&self) -> bool {
        self.health
            .as_ref()
            .is_some_and(|h| h.succeeded_at.is_some())
    }

    /// A refresh is currently running: an attempt started recently (a stale
    /// unconcluded stamp means a crashed child, not a live one; signed age,
    /// so a skewed future stamp reads as running rather than spawning
    /// forever) and hasn't stamped a conclusion. New invocations attach to
    /// it instead of double-spawning — its verdict is seconds away and
    /// answers them all, and the live table treats it exactly like a
    /// refresh it spawned itself.
    pub fn refresh_in_flight(&self) -> bool {
        let Some(health) = &self.health else {
            return false;
        };
        let Some(started) = health.started_at else {
            return false;
        };
        let concluded = health.finished_at.is_some_and(|f| f >= started);
        !concluded
            && chrono::Utc::now().signed_duration_since(started)
                < chrono::Duration::seconds(REFRESH_LIVENESS_SECS)
    }
}

/// Read the repo's forge gate: local capability plus persisted health. The
/// health read is skipped for incapable repos (their store has nothing to
/// say about a forge they don't have).
pub fn forge_gate(git: &GitCommand, repo_hash: Option<String>) -> ForgeGate {
    let capable = crate::forge::repo_forge_capable(git);
    let health = match (&repo_hash, capable) {
        (Some(hash), true) => read_health(hash),
        _ => None,
    };
    ForgeGate {
        capable,
        repo_hash,
        health,
    }
}

/// The repo's persisted forge health. `None` on any error and when the
/// coordinator store doesn't exist yet — reading is pure (never
/// materializes a store) and fails open, i.e. healthy.
pub fn read_health(repo_hash: &str) -> Option<ForgeHealthRow> {
    let state_dir = crate::daft_state_dir().ok()?;
    let db_path = state_dir
        .join(paths::JOBS_SUBDIR)
        .join(repo_hash)
        .join(paths::COORDINATOR_DB);
    if !db_path.exists() {
        return None;
    }
    let pool = Pool::open(&db_path).ok()?;
    let conn = pool.reader().ok()?;
    ForgeHealthRepo::get(&conn).ok().flatten()
}

fn with_health_writer(
    repo_hash: &str,
    f: impl FnOnce(&rusqlite::Connection) -> crate::store::error::Result<()>,
) -> anyhow::Result<()> {
    let db_path = paths::for_repo(repo_hash)?;
    let pool = Pool::open(&db_path)?;
    let conn = pool.writer()?;
    conn.busy_timeout(std::time::Duration::from_millis(
        crate::store::connection::READER_BUSY_TIMEOUT_MS as u64,
    ))?;
    f(&conn)?;
    Ok(())
}

// Best-effort health stamps around a snapshot refresh. Failures are
// swallowed: health is advisory display state, never worth failing a
// refresh over.
fn record_refresh_started(repo_hash: &str) {
    let _ = with_health_writer(repo_hash, |c| {
        ForgeHealthRepo::record_started(c, chrono::Utc::now())
    });
}

fn record_refresh_success(repo_hash: &str) {
    let _ = with_health_writer(repo_hash, |c| {
        ForgeHealthRepo::record_success(c, chrono::Utc::now())
    });
}

fn record_refresh_failure(repo_hash: &str, deep_kind: Option<&str>) {
    let _ = with_health_writer(repo_hash, |c| {
        ForgeHealthRepo::record_failure(c, chrono::Utc::now(), deep_kind)
    });
}

/// Spawn a detached `daft __refresh-forge` for the repo the cwd is in —
/// the fire-and-forget form for remote-touching commands (update/sync),
/// which builds its own [`ForgeGate`]. The caller's command completes
/// regardless of the verdict.
pub fn spawn_background_refresh() {
    if crate::should_skip_background_tasks(crate::cli::argv()) {
        return;
    }
    let git = GitCommand::new(true);
    let repo_hash = crate::core::repo_identity::compute_repo_id().ok();
    spawn_background_refresh_gated(&forge_gate(&git, repo_hash));
}

/// Spawn a detached refresh against an already-computed gate ( `daft list`,
/// which needs the gate for column visibility anyway). Skipped — returning
/// `false` — for agent/test invocations ([`crate::should_skip_background_tasks`]:
/// they must never fan out network work), for repos that name no forge, and
/// while another refresh is already in flight (the caller attaches to that
/// one — its verdict is the same fresh data). There is deliberately no
/// freshness window beyond the in-flight guard: every invocation
/// re-verifies, however recently the last refresh concluded, so a rendered
/// status always has an active verification behind it.
pub fn spawn_background_refresh_gated(gate: &ForgeGate) -> bool {
    if crate::should_skip_background_tasks(crate::cli::argv()) {
        return false;
    }
    if !gate.capable || gate.refresh_in_flight() {
        return false;
    }
    spawn_inner().is_ok()
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
/// snapshot, with the outcome recorded as forge health. Errors are silently
/// swallowed by the caller (the process is detached; there is nowhere useful
/// to report) — health *is* the error channel: a deep failure (missing tool,
/// dead auth, lost repo access) marks the repo unhealthy, which silently
/// hides the default `pr` column until a later refresh succeeds.
pub fn run_refresh_forge() -> anyhow::Result<()> {
    // Detach from the parent's session/TTY per the spawn-self contract.
    nix::unistd::setsid().ok();
    let project_root = crate::core::repo::get_project_root()?;
    let repo_hash = crate::core::repo_identity::compute_repo_id()?;
    // Stamped before the fetch: the start stamp is the in-flight guard's
    // key, so a slow gh can't let a second `daft list` pile on a second
    // concurrent refresh (it attaches to this one instead).
    record_refresh_started(&repo_hash);
    let git = GitCommand::new(true);
    let config = crate::forge::ForgeConfig::load(&git);
    match crate::forge::fetch_snapshot(&git, &project_root, &config) {
        Ok((kind, entries)) => {
            // Snapshot first, then the success stamp — the live table's poll
            // concludes on the stamp, so the data is always there when it
            // reloads the lookup.
            persist_snapshot(&repo_hash, kind, &entries);
            record_refresh_success(&repo_hash);
            Ok(())
        }
        Err(err) => {
            let deep = crate::forge::classify_unavailable(&err).map(|k| k.kind_str());
            record_refresh_failure(&repo_hash, deep);
            Err(err)
        }
    }
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
            head_repo_owner: String::new(),
            updated_at: None,
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
    fn sanitize_url_keeps_web_links_and_drops_hostile_input() {
        assert_eq!(
            sanitize_url("https://github.com/acme/widget/pull/5"),
            "https://github.com/acme/widget/pull/5"
        );
        assert_eq!(
            sanitize_url("  https://gitlab.com/g/r/-/merge_requests/4 "),
            { "https://gitlab.com/g/r/-/merge_requests/4" }
        );
        // OSC-breaking control chars, embedded whitespace, non-web schemes.
        assert_eq!(sanitize_url("https://evil.com/\x1b]0;owned\x07"), "");
        assert_eq!(sanitize_url("https://a.com/b c"), "");
        assert_eq!(sanitize_url("file:///etc/passwd"), "");
        assert_eq!(sanitize_url("javascript:alert(1)"), "");
        assert_eq!(sanitize_url(""), "");
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
        assert!(read_health("never-seen").is_none());
    }

    fn gate(capable: bool, health: Option<ForgeHealthRow>) -> ForgeGate {
        ForgeGate {
            capable,
            repo_hash: Some("repo".into()),
            health,
        }
    }

    fn health_row(healthy: bool, started_secs_ago: Option<i64>, succeeded: bool) -> ForgeHealthRow {
        let now = chrono::Utc::now();
        ForgeHealthRow {
            healthy,
            error_kind: (!healthy).then(|| "unauthenticated".into()),
            started_at: started_secs_ago.map(|s| now - chrono::Duration::seconds(s)),
            finished_at: None,
            succeeded_at: succeeded.then_some(now),
        }
    }

    #[test]
    fn gate_hides_the_default_column_for_incapable_or_unhealthy_repos() {
        // No forge remote → never visible, regardless of health.
        assert!(!gate(false, None).column_visible());
        // Capable with no history → optimistic (visible until proven deep-broken).
        assert!(gate(true, None).column_visible());
        assert!(gate(true, Some(health_row(true, None, true))).column_visible());
        // A persisted deep failure hides it…
        assert!(!gate(true, Some(health_row(false, None, false))).column_visible());
        // …until a success restores it (health flips back).
        assert!(gate(true, Some(health_row(true, None, true))).column_visible());
    }

    #[test]
    fn gate_sees_a_concurrent_refresh_as_in_flight() {
        // Started recently, not concluded → in flight (attach, don't stack).
        let mut h = health_row(true, Some(5), true);
        h.finished_at = None;
        assert!(gate(true, Some(h)).refresh_in_flight());

        // A skewed future start stamp reads as in flight rather than
        // spawning forever until the clock catches up.
        let mut h = health_row(true, Some(-30), true);
        h.finished_at = None;
        assert!(gate(true, Some(h)).refresh_in_flight());

        // A stale unconcluded stamp is a crashed child, not a live refresh.
        let mut h = health_row(true, Some(300), true);
        h.finished_at = None;
        assert!(!gate(true, Some(h)).refresh_in_flight());

        assert!(!gate(true, None).refresh_in_flight());
    }

    /// Freshness is deliberately not time-gated: a refresh that concluded
    /// seconds ago must not block the next invocation from re-verifying —
    /// every `daft list` gets a new call.
    #[test]
    fn gate_lets_every_invocation_reverify_once_the_last_refresh_concluded() {
        let now = chrono::Utc::now();
        let mut h = health_row(true, Some(5), true);
        h.finished_at = Some(now);
        assert!(!gate(true, Some(h)).refresh_in_flight());
    }

    #[test]
    fn gate_first_load_state_keys_off_ever_succeeded() {
        assert!(!gate(true, None).ever_succeeded());
        assert!(!gate(true, Some(health_row(true, Some(5), false))).ever_succeeded());
        assert!(gate(true, Some(health_row(true, Some(5), true))).ever_succeeded());
    }

    #[test]
    #[serial]
    fn refresh_health_stamps_round_trip() {
        let _guard = crate::store::paths::IsolatedStateDir::new();
        let repo = "forge-health-repo";

        record_refresh_started(repo);
        let h = read_health(repo).unwrap();
        assert!(h.healthy);
        assert!(h.started_at.is_some());
        assert_eq!(h.finished_at, None);

        record_refresh_failure(repo, Some("missing-tool"));
        let h = read_health(repo).unwrap();
        assert!(!h.healthy);
        assert_eq!(h.error_kind.as_deref(), Some("missing-tool"));

        record_refresh_success(repo);
        let h = read_health(repo).unwrap();
        assert!(h.healthy);
        assert_eq!(h.error_kind, None);
        assert!(h.succeeded_at.is_some());
    }

    #[test]
    #[serial]
    fn load_lookup_prioritizes_outbound_and_keeps_inbound_fate() {
        use crate::core::worktree::forge_ref::{
            CiStatus as Ci, ForgeBranchRef, ForgeRefKind as K, PrStatus,
        };
        let _guard = crate::store::paths::IsolatedStateDir::new();
        let repo = "forge-repo-4";

        let mut fork = entry(8, "feat/x", Some(CiStatus::Fail));
        fork.is_cross_repo = true;
        // A reused branch: PR 5 merged there before PR 7 was opened.
        let mut old_merged = entry(5, "feat/x", None);
        old_merged.state = "merged".into();
        let mut merged = entry(6, "feat/done", Some(CiStatus::Pass));
        merged.state = "merged".into();
        let mut closed = entry(4, "feat/abandoned", None);
        closed.state = "closed".into();
        persist_snapshot(
            repo,
            ForgeRefKind::GithubPr,
            &[
                entry(7, "feat/x", Some(CiStatus::Pass)),
                fork,
                old_merged,
                merged,
                closed,
            ],
        );

        let lookup = load_lookup(repo);

        // Outbound: the open same-repo PR beats the fork PR and the merged one.
        let d = &lookup.by_branch["feat/x"];
        assert_eq!(d.r, ForgeBranchRef::new(K::GithubPr, 7));
        assert_eq!(d.status, Some(PrStatus::Ci(Ci::Pass)));
        assert_eq!(
            d.url.as_deref(),
            Some("https://github.com/acme/widget/pull/7")
        );

        // A merged PR decorates its branch when nothing open shadows it…
        assert_eq!(lookup.by_branch["feat/done"].status, Some(PrStatus::Merged));
        // …but closed-unmerged never does.
        assert!(!lookup.by_branch.contains_key("feat/abandoned"));

        // Inbound fate is available for every cached row, fork/closed included.
        assert_eq!(
            lookup.by_ref[&ForgeBranchRef::new(K::GithubPr, 8)].status,
            Some(PrStatus::Ci(Ci::Fail))
        );
        assert_eq!(
            lookup.by_ref[&ForgeBranchRef::new(K::GithubPr, 6)].status,
            Some(PrStatus::Merged)
        );
        assert_eq!(
            lookup.by_ref[&ForgeBranchRef::new(K::GithubPr, 4)].status,
            Some(PrStatus::Closed)
        );

        // Row seeds: exactly the open PRs (fork included), newest first, with
        // the author carried for the Owner cell.
        let numbers: Vec<u32> = lookup.open.iter().map(|p| p.decoration.r.number).collect();
        assert_eq!(numbers, vec![8, 7], "open PRs only, newest number first");
        assert!(lookup.open[0].is_cross_repo);
        assert_eq!(lookup.open[0].decoration.author.as_deref(), Some("octocat"));
    }
}
