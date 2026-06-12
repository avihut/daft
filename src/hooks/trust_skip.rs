//! Untrusted-hook skip notices and replay records (#596).
//!
//! When the trust gate blocks a hook fire, two things must happen:
//!
//! 1. **Notice** — one contextual stderr warning per command invocation,
//!    naming the skipped hooks and suggesting `git daft hooks trust`. The
//!    notice is emitted here, from the executor's trust-Deny arms, so every
//!    command and both config shapes (`daft.yml`, `.daft/hooks/`) are
//!    covered centrally — no per-caller wiring to forget.
//! 2. **Record** — a `status = 'skipped'` row in the `invocations` table so
//!    `git daft hooks trust` can later suggest replaying exactly the hooks
//!    that never ran. A row means "the most recent fire of this
//!    (worktree, hook type) was blocked by trust"; it is deleted via
//!    [`clear_skips`] the moment the gate next passes for that pair.
//!
//! # Once-per-command dedup
//!
//! "Once per command invocation" is tracked in a process-global registry
//! keyed by git dir — one daft process is one command, and several commands
//! construct multiple `HookExecutor`s per invocation (merge builds three;
//! multi-branch clone builds one per satellite), so executor-instance state
//! cannot dedupe.
//!
//! **Test contract:** there is deliberately no reset helper. Unit tests run
//! in parallel threads sharing this registry; isolation comes from keying —
//! every test must use a unique (tempdir) git dir and must never assert on
//! global warning counts across git dirs.
//!
//! # TUI deferral
//!
//! In TUI mode the executor writes to a `BufferingOutput` whose warnings
//! never reach the user. Emitting there would both hide the notice and mark
//! it "shown", suppressing the one later chance at visibility. Instead,
//! when [`Output::live_warnings`] is false the notice accumulates as
//! pending state, and the command calls [`flush_pending_notice`] after the
//! TUI exits to emit a single aggregated warning on the real stderr.

use crate::coordinator::ports::JobsStorePort;
use crate::hooks::HookContext;
use crate::hooks::HookType;
use crate::output::Output;
use crate::store::models::InvocationRow;
use crate::store::models::invocation::{INVOCATION_STATUS_SKIPPED, SKIP_REASON_UNTRUSTED};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

/// Where the skipped hook definitions came from, with the names the notice
/// should list.
pub enum SkipSource {
    /// YAML config. `configured_hooks` are the lifecycle hook names defined
    /// in the loaded config (not just the one that fired), so a checkout
    /// that skips both create hooks names both in its single notice.
    Yaml { configured_hooks: Vec<String> },
    /// Legacy `.daft/hooks/` scripts discovered for the current hook type.
    Scripts { hook_files: Vec<String> },
}

#[derive(Default)]
struct NoticeState {
    /// A warning has reached a real stderr for this git dir.
    displayed: bool,
    /// Accumulated while the output was buffered (TUI mode).
    pending: Option<NoticeContent>,
}

#[derive(Default, Clone)]
struct NoticeContent {
    /// Hook names to list (yaml names or script filenames).
    names: BTreeSet<String>,
    /// Branches the skips applied to. Empty for live notices (the branch is
    /// obvious from command context); populated for aggregated TUI flushes.
    branches: BTreeSet<String>,
    /// All skips came from legacy scripts (wording switches to
    /// `.daft/hooks/`). Any YAML skip flips this off.
    legacy: bool,
    /// Which `git daft hooks run <hook>` replay to suggest, if any.
    replay: Option<String>,
}

impl NoticeContent {
    fn merge(&mut self, other: NoticeContent) {
        self.names.extend(other.names);
        self.branches.extend(other.branches);
        self.legacy = self.legacy && other.legacy;
        // Post-create wins over post-clone: it is the per-worktree setup
        // replay, which is what aggregated (multi-worktree) notices need.
        self.replay = match (self.replay.take(), other.replay) {
            (Some(a), Some(b)) => {
                if a == "worktree-post-create" || b == "worktree-post-create" {
                    Some("worktree-post-create".to_string())
                } else {
                    Some(a.max(b))
                }
            }
            (a, b) => a.or(b),
        };
    }
}

fn registry() -> &'static Mutex<HashMap<PathBuf, NoticeState>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, NoticeState>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn registry_key(git_dir: &Path) -> PathBuf {
    git_dir
        .canonicalize()
        .unwrap_or_else(|_| git_dir.to_path_buf())
}

/// Called from the executor's trust-Deny arms: emit (or defer) the
/// once-per-command notice, then best-effort record the skip row. The
/// warning never depends on the store write succeeding.
pub fn notify_and_record(ctx: &HookContext, source: SkipSource, output: &mut dyn Output) {
    let content = notice_content(ctx, &source);
    let key = registry_key(&ctx.git_dir);
    {
        let mut reg = registry().lock().unwrap_or_else(PoisonError::into_inner);
        let state = reg.entry(key).or_default();
        if !state.displayed {
            if output.live_warnings() {
                output.warning(&format_notice(&content, !crate::hints::hints_disabled()));
                state.displayed = true;
            } else {
                match &mut state.pending {
                    Some(pending) => pending.merge(content),
                    None => state.pending = Some(content),
                }
            }
        }
    }
    record_skip(ctx, SKIP_REASON_UNTRUSTED);
}

/// Post-TUI flush: if skips were deferred for `git_dir` and no warning has
/// reached a real stderr yet, emit one aggregated notice. No-op otherwise.
pub fn flush_pending_notice(git_dir: &Path, output: &mut dyn Output) {
    let key = registry_key(git_dir);
    let content = {
        let mut reg = registry().lock().unwrap_or_else(PoisonError::into_inner);
        match reg.get_mut(&key) {
            Some(state) if !state.displayed => {
                let pending = state.pending.take();
                if pending.is_some() {
                    state.displayed = true;
                }
                pending
            }
            _ => None,
        }
    };
    if let Some(content) = content {
        output.warning(&format_notice(&content, !crate::hints::hints_disabled()));
    }
}

/// Best-effort write of a `status = 'skipped'` invocation row. Failures are
/// reported on raw stderr (same precedent as the yaml executor's
/// invocation-meta write) — never via `output.warning`, which in TUI mode
/// would vanish into the buffer and hide the degradation.
pub fn record_skip(ctx: &HookContext, reason: &str) {
    if let Err(e) = try_record_skip(ctx, reason) {
        eprintln!(
            "daft: failed to record skipped {} hook: {e}",
            ctx.hook_type.yaml_name()
        );
    }
}

fn try_record_skip(ctx: &HookContext, reason: &str) -> anyhow::Result<()> {
    // `create = true` always yields a store on success; the `else` arm is
    // unreachable but must not panic on a best-effort path.
    let Some(store) = open_store(ctx, true)? else {
        return Ok(());
    };
    let repo_hash = repo_hash(ctx)?;
    store.record_skipped_invocation(&InvocationRow {
        repo_hash,
        invocation_id: crate::coordinator::log_store::generate_invocation_id(),
        trigger_command: ctx.command.clone(),
        hook_type: ctx.hook_type.yaml_name().to_string(),
        worktree: ctx.branch_name.clone(),
        created_at: chrono::Utc::now(),
        coordinator_pid: None,
        status: INVOCATION_STATUS_SKIPPED.to_string(),
        skip_reason: Some(reason.to_string()),
    })
}

/// Best-effort delete of skip rows for `(repo, hook type, branch)`. Called
/// by the executor whenever the trust gate passes (Allow, prompt accepted,
/// or bypass) — the row's meaning is "the most recent fire was blocked",
/// so a passing gate invalidates it even if the hook then fails or its
/// `skip:` condition fires.
pub fn clear_skips(ctx: &HookContext) {
    if let Err(e) = try_clear_skips(ctx) {
        eprintln!(
            "daft: failed to clear skipped-{} records: {e}",
            ctx.hook_type.yaml_name()
        );
    }
}

fn try_clear_skips(ctx: &HookContext) -> anyhow::Result<()> {
    // No DB → no rows. Skipping early also avoids manufacturing per-repo
    // state directories on every trusted hook fire of a fresh repo.
    let Some(store) = open_store(ctx, false)? else {
        return Ok(());
    };
    let repo_hash = repo_hash(ctx)?;
    store.clear_skipped_invocations(&repo_hash, ctx.hook_type.yaml_name(), &ctx.branch_name)
}

fn repo_hash(ctx: &HookContext) -> anyhow::Result<String> {
    crate::core::repo_identity::compute_repo_id_from_common_dir(&ctx.git_dir)
}

/// Open the per-repo store, honoring `ctx.state_dir` (tests) over
/// `daft_state_dir()` (production). With `create = false`, returns
/// `Ok(None)` when the DB file does not exist instead of creating it.
fn open_store(
    ctx: &HookContext,
    create: bool,
) -> anyhow::Result<Option<crate::coordinator::adapters::SqliteJobsStore>> {
    use crate::coordinator::adapters::SqliteJobsStore;
    use crate::store::paths::{COORDINATOR_DB, JOBS_SUBDIR};

    let repo_hash = repo_hash(ctx)?;
    let state_base = match &ctx.state_dir {
        Some(p) => p.clone(),
        None => crate::daft_state_dir()?,
    };
    if create {
        // Canonical resolver: creates the per-repo dir and rejects
        // symlink escapes.
        let db_path = crate::store::paths::for_repo_under(&state_base, &repo_hash)?;
        let base = db_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("coordinator DB path has no parent"))?;
        Ok(Some(SqliteJobsStore::for_repo_base(base)?))
    } else {
        let base = state_base.join(JOBS_SUBDIR).join(&repo_hash);
        if !base.join(COORDINATOR_DB).exists() {
            return Ok(None);
        }
        Ok(Some(SqliteJobsStore::for_repo_base(&base)?))
    }
}

fn notice_content(ctx: &HookContext, source: &SkipSource) -> NoticeContent {
    let (names, legacy): (BTreeSet<String>, bool) = match source {
        SkipSource::Yaml { configured_hooks } => {
            (configured_hooks.iter().cloned().collect(), false)
        }
        SkipSource::Scripts { hook_files } => (hook_files.iter().cloned().collect(), true),
    };
    let replay = match ctx.hook_type {
        HookType::PreCreate | HookType::PostCreate => names
            .contains("worktree-post-create")
            .then(|| "worktree-post-create".to_string()),
        HookType::PostClone => names
            .contains("post-clone")
            .then(|| "post-clone".to_string()),
        _ => None,
    };
    NoticeContent {
        names,
        // The branch is only named in aggregated (deferred) notices; a live
        // notice prints inside the command that names the branch already.
        branches: BTreeSet::from([ctx.branch_name.clone()]),
        legacy,
        replay,
    }
}

/// Cap a name list at four entries, then "and N more".
fn capped_list(names: &BTreeSet<String>, more_word: &str) -> String {
    const CAP: usize = 4;
    let shown: Vec<&str> = names.iter().take(CAP).map(String::as_str).collect();
    let mut out = shown.join(", ");
    if names.len() > CAP {
        out.push_str(&format!(" and {} more {more_word}", names.len() - CAP));
    }
    out
}

/// Like [`capped_list`], but hook names come out in lifecycle order
/// (pre-create before post-create, …) rather than the set's alphabetical
/// order, which reads backwards for hooks. Names that aren't lifecycle
/// hooks (legacy script filenames with suffixes, deprecated names) keep
/// their alphabetical position at the end.
fn capped_hook_list(names: &BTreeSet<String>, more_word: &str) -> String {
    let lifecycle_pos = |name: &str| {
        HookType::from_yaml_name(name)
            .and_then(|ht| HookType::all().iter().position(|h| *h == ht))
            .unwrap_or(usize::MAX)
    };
    let mut ordered: Vec<&String> = names.iter().collect();
    ordered.sort_by_key(|name| (lifecycle_pos(name), name.as_str()));

    const CAP: usize = 4;
    let shown: Vec<&str> = ordered.iter().take(CAP).map(|s| s.as_str()).collect();
    let mut out = shown.join(", ");
    if ordered.len() > CAP {
        out.push_str(&format!(" and {} more {more_word}", ordered.len() - CAP));
    }
    out
}

fn format_notice(content: &NoticeContent, show_hints: bool) -> String {
    // Continuation lines align under the 9-char "warning: " prefix the CLI
    // output adds to the first line.
    const INDENT: &str = "         ";

    let multi_branch = content.branches.len() > 1;
    let branch_clause = if multi_branch {
        format!(" for {}", capped_list(&content.branches, "worktrees"))
    } else {
        String::new()
    };

    let mut first = if content.legacy {
        if content.names.len() == 1 {
            let name = content.names.iter().next().expect("len checked");
            format!(
                ".daft/hooks/{name} was NOT run{branch_clause} — this repository isn't trusted."
            )
        } else {
            format!(
                ".daft/hooks/ scripts ({}) were NOT run{branch_clause} — this repository isn't trusted.",
                capped_hook_list(&content.names, "scripts")
            )
        }
    } else if content.names.len() == 1 {
        let name = content.names.iter().next().expect("len checked");
        format!(
            "daft.yml defines a {name} hook that was NOT run{branch_clause} — this repository isn't trusted."
        )
    } else {
        format!(
            "daft.yml defines hooks ({}) that were NOT run{branch_clause} — this repository isn't trusted.",
            capped_hook_list(&content.names, "hooks")
        )
    };

    if show_hints {
        first.push_str(&format!(
            "\n{INDENT}To run hooks here, trust this repository:  git daft hooks trust"
        ));
        if let Some(replay) = &content.replay {
            // Labels are padded so both suggestion commands start at the
            // same column (43 chars after the indent, matching the trust
            // line above).
            let (label, suffix) = match (replay.as_str(), multi_branch) {
                ("post-clone", _) => ("Then replay the clone's setup:             ", ""),
                (_, true) => (
                    "Then replay each worktree's setup:         ",
                    "   (run inside each worktree)",
                ),
                (_, false) => ("Then replay this worktree's setup:         ", ""),
            };
            first.push_str(&format!(
                "\n{INDENT}{label}git daft hooks run {replay}{suffix}"
            ));
        }
    }
    first
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::{BufferingOutput, TestOutput};
    use tempfile::TempDir;

    /// Each test gets a unique git dir (the registry key) and a private
    /// state dir, per the module's test contract.
    fn test_ctx(
        git_dir: &Path,
        state_dir: &Path,
        hook_type: HookType,
        branch: &str,
    ) -> HookContext {
        HookContext::new(
            hook_type,
            "checkout",
            git_dir.parent().unwrap_or(git_dir),
            git_dir,
            "origin",
            git_dir.parent().unwrap_or(git_dir),
            git_dir.parent().unwrap_or(git_dir).join(branch),
            branch,
        )
        .with_state_dir(state_dir)
    }

    fn yaml_source() -> SkipSource {
        SkipSource::Yaml {
            configured_hooks: vec![
                "worktree-pre-create".to_string(),
                "worktree-post-create".to_string(),
            ],
        }
    }

    #[test]
    fn warns_once_per_git_dir_and_names_all_configured_hooks() {
        let tmp = TempDir::new().unwrap();
        let git_dir = tmp.path().join("repo/.git");
        std::fs::create_dir_all(&git_dir).unwrap();
        let state = tmp.path().join("state");

        let mut output = TestOutput::new();
        let pre = test_ctx(&git_dir, &state, HookType::PreCreate, "feat/x");
        notify_and_record(&pre, yaml_source(), &mut output);
        let post = test_ctx(&git_dir, &state, HookType::PostCreate, "feat/x");
        notify_and_record(&post, yaml_source(), &mut output);

        let warnings = output.warnings();
        assert_eq!(warnings.len(), 1, "second Deny hit must be deduped");
        // Hint lines depend on the ambient DAFT_NO_HINTS; assert only the
        // env-independent first line here (hints are covered by the pure
        // format tests below).
        assert!(warnings[0].contains("worktree-pre-create"));
        assert!(warnings[0].contains("worktree-post-create"));
    }

    #[test]
    fn format_notice_yaml_multi_includes_trust_and_replay_hints() {
        let content = NoticeContent {
            names: BTreeSet::from([
                "worktree-pre-create".to_string(),
                "worktree-post-create".to_string(),
            ]),
            branches: BTreeSet::from(["feat/x".to_string()]),
            legacy: false,
            replay: Some("worktree-post-create".to_string()),
        };
        let msg = format_notice(&content, true);
        assert!(msg.starts_with("daft.yml defines hooks ("));
        assert!(msg.contains("git daft hooks trust"));
        assert!(msg.contains("git daft hooks run worktree-post-create"));
        assert!(
            !msg.contains(" for feat/x"),
            "single-branch notices do not name the branch"
        );
    }

    #[test]
    fn buffered_output_defers_until_flush_then_flushes_once() {
        let tmp = TempDir::new().unwrap();
        let git_dir = tmp.path().join("repo/.git");
        std::fs::create_dir_all(&git_dir).unwrap();
        let state = tmp.path().join("state");

        let mut buffered = BufferingOutput::new();
        let a = test_ctx(&git_dir, &state, HookType::PostCreate, "feat/a");
        notify_and_record(&a, yaml_source(), &mut buffered);
        let b = test_ctx(&git_dir, &state, HookType::PostCreate, "feat/b");
        notify_and_record(&b, yaml_source(), &mut buffered);
        assert!(
            buffered.take_warnings().is_empty(),
            "buffered sink must not receive the notice"
        );

        let mut real = TestOutput::new();
        flush_pending_notice(&git_dir, &mut real);
        let warnings = real.warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("feat/a"), "aggregate names branches");
        assert!(warnings[0].contains("feat/b"));

        let mut again = TestOutput::new();
        flush_pending_notice(&git_dir, &mut again);
        assert!(again.warnings().is_empty(), "flush is one-shot");
    }

    #[test]
    fn live_warning_suppresses_later_flush() {
        let tmp = TempDir::new().unwrap();
        let git_dir = tmp.path().join("repo/.git");
        std::fs::create_dir_all(&git_dir).unwrap();
        let state = tmp.path().join("state");

        let mut output = TestOutput::new();
        let ctx = test_ctx(&git_dir, &state, HookType::PostClone, "main");
        notify_and_record(
            &ctx,
            SkipSource::Yaml {
                configured_hooks: vec!["post-clone".to_string()],
            },
            &mut output,
        );
        assert_eq!(output.warnings().len(), 1);

        let mut after = TestOutput::new();
        flush_pending_notice(&git_dir, &mut after);
        assert!(after.warnings().is_empty());
    }

    #[test]
    fn records_skip_row_and_clear_removes_it() {
        use crate::coordinator::ports::JobsStorePort;

        let tmp = TempDir::new().unwrap();
        let git_dir = tmp.path().join("repo/.git");
        std::fs::create_dir_all(&git_dir).unwrap();
        let state = tmp.path().join("state");

        let ctx = test_ctx(&git_dir, &state, HookType::PostCreate, "feat/x");
        record_skip(&ctx, SKIP_REASON_UNTRUSTED);

        let repo_hash =
            crate::core::repo_identity::compute_repo_id_from_common_dir(&git_dir).unwrap();
        let store = open_store(&ctx, false).unwrap().expect("db exists");
        let rows = store.list_skipped_invocations(&repo_hash).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].hook_type, "worktree-post-create");
        assert_eq!(rows[0].worktree, "feat/x");
        assert_eq!(rows[0].trigger_command, "checkout");
        assert_eq!(rows[0].skip_reason.as_deref(), Some(SKIP_REASON_UNTRUSTED));

        // Repeated skip replaces, not accumulates.
        record_skip(&ctx, SKIP_REASON_UNTRUSTED);
        assert_eq!(store.list_skipped_invocations(&repo_hash).unwrap().len(), 1);

        clear_skips(&ctx);
        assert!(
            store
                .list_skipped_invocations(&repo_hash)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn clear_without_db_is_a_silent_noop_and_creates_nothing() {
        let tmp = TempDir::new().unwrap();
        let git_dir = tmp.path().join("repo/.git");
        std::fs::create_dir_all(&git_dir).unwrap();
        let state = tmp.path().join("state");

        let ctx = test_ctx(&git_dir, &state, HookType::PostCreate, "feat/x");
        clear_skips(&ctx);
        assert!(
            !state.exists(),
            "clear on a fresh repo must not manufacture state dirs"
        );
    }

    #[test]
    fn format_notice_legacy_single_and_hints_off() {
        let content = NoticeContent {
            names: BTreeSet::from(["worktree-pre-remove".to_string()]),
            branches: BTreeSet::from(["feat/x".to_string()]),
            legacy: true,
            replay: None,
        };
        let with_hints = format_notice(&content, true);
        assert!(with_hints.starts_with(".daft/hooks/worktree-pre-remove was NOT run"));
        assert!(with_hints.contains("git daft hooks trust"));
        assert!(
            !with_hints.contains("hooks run"),
            "remove hooks get no replay line"
        );

        let without = format_notice(&content, false);
        assert!(!without.contains("git daft hooks trust"));
        assert_eq!(without.lines().count(), 1);
    }

    #[test]
    fn format_notice_caps_long_lists() {
        let content = NoticeContent {
            names: (1..=6).map(|i| format!("hook-{i}")).collect(),
            branches: BTreeSet::new(),
            legacy: false,
            replay: None,
        };
        let msg = format_notice(&content, false);
        assert!(msg.contains("and 2 more hooks"));
        assert!(!msg.contains("hook-5"));
    }
}
