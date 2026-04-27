# Live List Population — Phase 3: Migrate prune/sync to Streaming Seed

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development to implement this plan.

**Goal:** Replace prune.rs and sync.rs's blocking `collect_worktree_info`
pre-seed with the streaming collector running concurrently with the
orchestrator. Cells fill in live during fetch/prune/update/rebase/push phases.
Spinners removed.

**Architecture:** Drop
`let infos = if needs_spinner { spinner + collect_worktree_info } else { collect_worktree_info };`
blocks from `prune.rs:run_tui` and `sync.rs::run_tui`. Replace with cheap
porcelain parse → seed `LiveTable` → `list_stream::spawn(ALL fields, Collector)`
running concurrently with the existing DAG orchestrator. Add
`spawn_post_fetch_refresh` (sibling of `spawn_post_task_refresh`) called after
`Fetch` phase completes — spawns `list_stream::spawn(REMOTE_DERIVED, PostFetch)`
for all branches. Integration tests run with `DAFT_NO_LIVE=1` (no behavior
change for non-TTY paths).

**Tech Stack:** Same as Phase 1/2 — Rust, ratatui, mpsc, no new deps.

**Spec:** `docs/superpowers/specs/2026-04-25-live-list-population-design.md`

**Phase 3 of 3.** Builds on Phase 1 (PR #410) + Phase 2 (same PR).

---

## File Structure

**Modified files:**

- `src/commands/prune.rs` — drop the `needs_spinner` block in `run_tui()` (lines
  ~221-254). Replace with cheap porcelain seed + collector spawn. After Fetch
  phase, spawn post-fetch refresh.
- `src/commands/sync.rs` — same pattern as prune.rs in `run_tui()` (lines
  ~417-440). Add `spawn_post_fetch_refresh` helper sibling to the existing
  `spawn_post_task_refresh`. Call it from the orchestrator thread after the
  Fetch phase signals completion.
- `src/commands/sync_shared.rs` — `run_fetch_phase` already emits Fetch phase
  events. No changes here unless the orchestrator needs a hook to know when
  fetch is done.
- `src/commands/list.rs` — no changes (Phase 2 already covered `daft list`).
- `tests/integration/run.sh` (or whatever the test driver is) — set
  `DAFT_NO_LIVE=1` for prune/sync scenarios so golden outputs stay stable. (Or
  set per-scenario in YAML headers.)

**No new files.**

---

## Task 1: Add `spawn_post_fetch_refresh` helper to sync.rs

**Files:**

- Modify: `src/commands/sync.rs`

Sibling of the existing `spawn_post_task_refresh`. Spawns the collector with
`FieldSet::REMOTE_DERIVED` and `PatchSource::PostFetch` for every branch in the
`worktree_map` after the Fetch phase completes. Used by both `daft sync` and
`daft prune` (extracted to a shared module if convenient — but starting in
sync.rs is fine; move later if both need it).

- [ ] **Step 1: Implement the helper**

In `src/commands/sync.rs`, near `spawn_post_task_refresh`:

```rust
/// After the Fetch phase completes, re-run the streaming collector
/// against `REMOTE_DERIVED` fields for every worktree branch. Patches
/// arrive as `PatchSource::PostFetch` so `LiveTable` can suppress any
/// stale `Collector` patches on the same fields. Blocks on join() so
/// patches land before the orchestrator dispatches per-branch tasks.
fn spawn_post_fetch_refresh(
    worktree_map: &HashMap<String, (PathBuf, bool)>,
    settings: &Arc<DaftSettings>,
    base_branch: &str,
    user_email: Option<&str>,
    stat: Stat,
    tx: &mpsc::Sender<DagEvent>,
) {
    let targets: Vec<list_stream::CollectorTarget> = worktree_map.iter()
        .map(|(branch_name, (path, _is_main))| list_stream::CollectorTarget {
            branch_name: branch_name.clone(),
            path: Some(path.clone()),
            kind: EntryKind::Worktree,
            is_detached: false,
        })
        .collect();
    if targets.is_empty() { return; }
    let ctx = Arc::new(list_stream::CollectorContext {
        use_gitoxide: settings.use_gitoxide,
        base_branch: base_branch.to_string(),
        remote_name: settings.remote.clone(),
        ownership_strategy: settings.ownership_strategy,
        user_email: user_email.map(|s| s.to_string()),
    });
    let handle = list_stream::spawn(
        list_stream::CollectorRequest {
            targets,
            fields: FieldSet::REMOTE_DERIVED,
            stat,
            source: PatchSource::PostFetch,
            ctx,
        },
        tx.clone(),
    );
    handle.join();
}
```

- [ ] **Step 2: Call it from sync's orchestrator thread after Fetch succeeds**

In `src/commands/sync.rs::run_tui`'s orchestrator-thread closure, find where
`Fetch` task completes successfully. Add the call there. (Look for the pattern
that today's code uses to detect Fetch completion — likely via
`match task.id { TaskId::Fetch => ... }` after `execute_fetch_task` returns
`TaskStatus::Succeeded`.)

- [ ] **Step 3: Verify build + unit tests**

Run: `cargo build --lib && cargo test --lib` Expected: clean build, all tests
pass (no behavior change yet — helper exists but until Phase 3 Task 3 lands the
streaming collector seed, the `LiveTable` doesn't show changes).

- [ ] **Step 4: Commit**

```bash
git add src/commands/sync.rs
git commit -m "feat(sync): add spawn_post_fetch_refresh for live remote-tracking refresh (#402)"
```

---

## Task 2: Replace sync's blocking pre-seed with streaming collector

**Files:**

- Modify: `src/commands/sync.rs`

Drop the `needs_spinner` blocking block (lines ~417-440). Replace with cheap
porcelain seed + spawn the streaming collector concurrently with the
orchestrator. The collector emits patches that `LiveTable` consumes via the
existing channel.

- [ ] **Step 1: Remove the spinner block**

In `src/commands/sync.rs::run_tui`, delete lines ~417-440 (the
`let needs_spinner = ...; let worktree_infos = if needs_spinner { ... } else { ... };`
block).

Replace with:

```rust
// Cheap porcelain seed — branches + paths + is_default known instantly.
let worktree_entries = prune::parse_worktree_list(&git)?;
let worktree_infos: Vec<WorktreeInfo> = worktree_entries.iter()
    .filter(|e| !e.is_bare)
    .map(|e| {
        let mut info = WorktreeInfo::empty(e.branch.as_deref().unwrap_or(""));
        info.path = Some(e.path.clone());
        info.is_default_branch = e.branch.as_deref() == Some(base_branch.as_str());
        info.is_current = current_path.as_deref() == Some(e.path.as_path());
        info
    })
    .collect();
```

(The `worktree_map` building below already uses `worktree_entries` — no
duplication.)

- [ ] **Step 2: Spawn the streaming collector after the orchestrator starts**

After `let (tx, rx) = mpsc::channel();` and the
`orchestrator_handle = thread::spawn(...)` line, add:

```rust
// Streaming collector: cells fill in concurrently with orchestrator.
let collector_targets: Vec<list_stream::CollectorTarget> = worktree_infos.iter()
    .map(|i| list_stream::CollectorTarget {
        branch_name: i.name.clone(),
        path: i.path.clone(),
        kind: EntryKind::Worktree,
        is_detached: false,
    })
    .collect();
let collector_ctx = Arc::new(list_stream::CollectorContext {
    use_gitoxide: settings.use_gitoxide,
    base_branch: base_branch.clone(),
    remote_name: settings.remote.clone(),
    ownership_strategy: settings.ownership_strategy,
    user_email: user_email.clone(),
});
let collector_handle = list_stream::spawn(
    list_stream::CollectorRequest {
        targets: collector_targets,
        fields: FieldSet::ALL,
        stat,
        source: PatchSource::Collector,
        ctx: collector_ctx,
    },
    tx.clone(),
);
```

After `renderer.run()?` returns and before exiting, drain the collector:

```rust
collector_handle.join();
```

- [ ] **Step 3: Verify build + tests with DAFT_NO_LIVE=1**

Run:

```bash
cargo build --bin daft
DAFT_NO_LIVE=1 cargo test --lib
DAFT_NO_LIVE=1 mise run test:integration -- --filter sync
```

Expected: all tests pass.

- [ ] **Step 4: Manual smoke test**

Run `daft sync` against a real repo. Observe: spinner is gone, table appears
instantly, cells fill in as fetch + per-branch tasks complete.

- [ ] **Step 5: Commit**

```bash
git add src/commands/sync.rs
git commit -m "feat(sync): replace blocking pre-seed with streaming collector (#402)"
```

---

## Task 3: Wire post-fetch refresh into sync's task closure

**Files:**

- Modify: `src/commands/sync.rs`

In sync's orchestrator-thread closure, after the `Fetch` task succeeds, call
`spawn_post_fetch_refresh(...)` so `REMOTE_DERIVED` cells reflect post-fetch
reality.

- [ ] **Step 1: Add the call**

Find the `match &task.id { TaskId::Fetch => ... }` arm (or wherever the fetch
completion handler runs). After the existing fetch logic and before the
`(status, message, outcomes)` return, add:

```rust
if status == TaskStatus::Succeeded {
    spawn_post_fetch_refresh(
        &shared_worktree_map,
        &orch_settings,
        &orch_base_branch,
        orch_user_email.as_deref(),
        orch_stat,
        &tx_for_tasks,
    );
}
```

(Use the existing `Arc`-cloned variables that the closure captures —
`shared_worktree_map`, `orch_settings`, etc.)

- [ ] **Step 2: Verify**

Run: `cargo test --lib` Expected: all tests pass.

Run: `DAFT_NO_LIVE=1 mise run test:integration -- --filter sync` Expected: all
tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/commands/sync.rs
git commit -m "feat(sync): refresh remote-derived cells after fetch (#402)"
```

---

## Task 4: Replace prune's blocking pre-seed with streaming collector

**Files:**

- Modify: `src/commands/prune.rs`

Same migration as sync (Task 2 of this phase) but for `prune.rs::run_tui`.

- [ ] **Step 1: Remove the spinner block**

In `src/commands/prune.rs::run_tui`, delete lines ~220-254 (the `needs_spinner`
block).

Replace with the same cheap porcelain seed pattern from sync's Task 2.

- [ ] **Step 2: Spawn the streaming collector after orchestrator starts**

Add the same collector spawn pattern after the orchestrator thread is created.

- [ ] **Step 3: Verify build**

Run: `cargo build --bin daft` Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add src/commands/prune.rs
git commit -m "feat(prune): replace blocking pre-seed with streaming collector (#402)"
```

---

## Task 5: Wire post-fetch refresh into prune's task closure

**Files:**

- Modify: `src/commands/prune.rs`

Same as sync's Task 3.

- [ ] **Step 1: Make `spawn_post_fetch_refresh` callable from prune.rs**

Either: (a) Move `spawn_post_fetch_refresh` from sync.rs to a new shared
location like `src/commands/sync_shared.rs` (and import from both). (b)
Duplicate it inside prune.rs.

Pick (a) — it's a clear shared helper and `sync_shared.rs` already exists.

- [ ] **Step 2: Add the call after Fetch in prune's orchestrator**

In `src/commands/prune.rs::run_tui`'s orchestrator thread, find where `Fetch`
succeeds and add the same call as sync's Task 3.

- [ ] **Step 3: Verify**

Run: `cargo test --lib` Run:
`DAFT_NO_LIVE=1 mise run test:integration -- --filter prune` Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add src/commands/prune.rs src/commands/sync.rs src/commands/sync_shared.rs
git commit -m "feat(prune): refresh remote-derived cells after fetch (#402)"
```

---

## Task 6: Set `DAFT_NO_LIVE=1` for integration tests

**Files:**

- Modify: `tests/integration/run.sh` (or similar test driver)
- Possibly: `tests/manual/scenarios/**/*.yml` — per-scenario env var if a global
  setting doesn't fit

Integration tests rely on stable golden output. The new live UX introduces
tick-based updates and timing variance that can break golden assertions. Setting
`DAFT_NO_LIVE=1` in the test environment forces the existing one-shot path.

- [ ] **Step 1: Identify the test driver**

Read `tests/integration/run.sh` (or the equivalent driver). Find where the
`daft` binary is invoked.

- [ ] **Step 2: Set the env var**

Either prepend `DAFT_NO_LIVE=1` to each `daft` invocation, or set it once at the
top of the driver (`export DAFT_NO_LIVE=1`).

For the YAML manual scenarios, check if they support per-scenario env vars. If
yes, add `env: DAFT_NO_LIVE=1` to each list/prune/sync scenario header. If no,
set it globally.

- [ ] **Step 3: Run the full integration suite**

Run: `mise run test:integration` Expected: all tests pass (with `DAFT_NO_LIVE=1`
set in the environment).

- [ ] **Step 4: Commit**

```bash
git add tests/integration/ tests/manual/scenarios/
git commit -m "test(integration): set DAFT_NO_LIVE=1 for stable golden output (#402)"
```

---

## Task 7: Final verification + push

- [ ] **Step 1: Full CI matrix**

Run: `mise run ci` Expected: zero warnings, all unit + integration tests pass.

- [ ] **Step 2: Manual smoke test of all three commands**

```bash
daft list                # rows appear instantly, cells fill live
daft prune               # no spinner, table appears instantly
daft sync                # no spinner, table fills live as phases progress
DAFT_NO_LIVE=1 daft list # one-shot rendering as before
```

- [ ] **Step 3: Push**

```bash
git push origin HEAD
```

- [ ] **Step 4: Update PR description**

Use `gh pr edit 410 --body "$(cat <<'EOF' ...)"` to add Phase 2 + 3 sections to
the PR body.

---

## Self-Review Notes

Performed inline before save:

- **Spec coverage**: Phase 3 deliverables map to tasks: streaming seed for prune
  (T4) and sync (T2), post-fetch refresh (T3, T5), `DAFT_NO_LIVE=1` opt-out for
  tests (T6), spinner removal (T2, T4 — removing `needs_spinner` blocks).
- **Placeholders**: none.
- **Type consistency**: `spawn_post_fetch_refresh`, `collector_handle`,
  `collector_targets` — used consistently.
- **Scope**: Phase 3 only.
- **Ambiguity**: Task 5's location of `spawn_post_fetch_refresh` (sync.rs vs
  sync_shared.rs) is decided in step 1 (move to sync_shared.rs).
