# Hooks in Sync/Prune TUI

## Problem

The ratatui TUI introduced for `daft sync` and `daft prune` (PR #285) uses
`NullBridge` to skip all hook execution. This means `worktree-pre-remove` and
`worktree-post-remove` hooks silently do not run when the TUI is active (stderr
is a TTY, no `--verbose` flag). Hooks must always run when trusted and present.

The root cause is that the existing hook renderer (`HookProgressRenderer`) uses
indicatif to write directly to stderr, which conflicts with ratatui's exclusive
control of the terminal. The two rendering systems cannot coexist.

## Solution

1. Replace `NullBridge` with a `TuiBridge` that actually executes hooks,
   captures their output, and reports results back through the DAG event
   channel.
2. Introduce two-tier verbosity (`-v` and `-vv`) to give users control over how
   much hook detail they see.

## Verbosity Levels

| Flag   | Level | Name            | Behavior                                                                                                 |
| ------ | ----- | --------------- | -------------------------------------------------------------------------------------------------------- |
| (none) | 0     | Default         | TUI with granular status labels for hook phases. Post-TUI detail on warning/failure only.                |
| `-v`   | 1     | Verbose TUI     | TUI with hook sub-rows showing individual hook outcomes and timings. Post-TUI detail on warning/failure. |
| `-vv`  | 2     | Full sequential | No TUI. Sequential execution with full indicatif hook renderer. Current `--verbose` behavior.            |

### CLI Change

Both `sync` and `prune` change `verbose` from `bool` to `u8` with
`action = clap::ArgAction::Count`:

```rust
#[arg(short, long, action = clap::ArgAction::Count,
      help = "Increase verbosity (-v for hook details, -vv for full sequential output)")]
verbose: u8,
```

`init_logging` is called with `args.verbose >= 2` (only full sequential mode
enables debug logging, matching current behavior where `-v` enabled it).

### Decision Logic

The existing condition in both `sync.rs` and `prune.rs`:

```rust
if std::io::IsTerminal::is_terminal(&std::io::stderr()) && !args.verbose {
```

Changes to:

```rust
if !std::io::IsTerminal::is_terminal(&std::io::stderr()) || args.verbose >= 2 {
    run_sequential()        // no TUI, full indicatif hook output
} else if args.verbose == 1 {
    run_tui(sub_rows=true)  // TUI + hook sub-rows
} else {
    run_tui(sub_rows=false) // TUI + granular status labels only
}
```

## Level 0 (Default): Granular Status Labels

Hooks execute. The status column cycles through sub-steps as they happen:

```
STATUS            BRANCH       PATH
checkmark up to date      master       master
spinner pre-remove      feat/old     feat-old
spinner removing        feat/stale   feat-stale
  waiting         feat/new     feat-new
```

After completion (all hooks succeeded):

```
STATUS            BRANCH       PATH
checkmark up to date      master       master
dash pruned          feat/old     feat-old
dash pruned          feat/stale   feat-stale
checkmark updated         feat/new     feat-new
```

Hook warned (status gets warning badge):

```
dash pruned warning        feat/old     feat-old
```

Hook failed with FailMode::Abort (prune skipped):

```
cross hook failed     feat/old     feat-old
```

Post-TUI output (only on warning/failure):

```
warning worktree-pre-remove warned for feat/old (exit 1):
  cleanup.sh: /tmp/cache not found, skipping
```

If all hooks succeeded: no post-TUI output.

## Level 1 (-v): TUI with Hook Sub-Rows

Same TUI table, but worktrees that have hooks get indented sub-rows beneath them
showing each hook's status and timing.

During execution:

```
STATUS            BRANCH       PATH
checkmark up to date      master       master
spinner post-remove     feat/old     feat-old
  |- pre-remove    checkmark 120ms
  \- post-remove   spinner
spinner pre-remove      feat/stale   feat-stale
  \- pre-remove    spinner
  waiting         feat/new     feat-new
```

Completed:

```
STATUS            BRANCH       PATH
checkmark up to date      master       master
dash pruned          feat/old     feat-old
  |- pre-remove    checkmark 120ms
  \- post-remove   checkmark 85ms
dash pruned          feat/stale   feat-stale
  |- pre-remove    checkmark 200ms
  \- post-remove   checkmark 90ms
checkmark updated         feat/new     feat-new
```

Worktrees without hooks (untrusted repo, no hook files) show no sub-rows.

Hook warned:

```
dash pruned warning        feat/stale   feat-stale
  |- pre-remove    warning 200ms  exit 1
  \- post-remove   checkmark 90ms
```

Post-TUI output follows the same rules as level 0 (only on warning/failure).

### Viewport Height for Sub-Rows

The ratatui inline viewport height is fixed at construction time
(`driver.rs:40-41`). At level 1, each worktree with hooks adds up to 2 extra
rows (one per hook type). The viewport must be pre-allocated to accommodate
this.

When `verbose == 1`, the TUI renderer pre-computes the maximum number of
sub-rows by checking which worktrees have hooks (via `hook_exists()` checks for
`PreRemove` and `PostRemove`) and adds those to `extra_rows`. This check is
cheap (filesystem stat calls) and happens before the TUI starts. Worktrees
discovered after fetch (gone branches) may also have hooks, but since their
worktree directory is about to be removed, the hook existence check can be done
at task execution time. For these, the `extra_rows` budget includes a
conservative estimate (2 per gone branch when `verbose == 1`).

## Level 2 (-vv): Full Sequential

Exactly what `--verbose` does today. Falls back to `run_sequential()` /
`run_prune()` with the full indicatif-based hook renderer, no TUI.

## Hook Execution Architecture

### TuiBridge

A new struct replacing `NullBridge` in `execute_prune_task()`. It implements
both `ProgressSink` and `HookRunner`.

```rust
pub struct TuiBridge {
    executor: HookExecutor,
    output: BufferingOutput,     // no-op Output that buffers warnings
    tx: mpsc::Sender<DagEvent>,  // channel to send hook events to TUI
    branch_name: String,         // for identifying hook events
}
```

**`ProgressSink`**: messages are discarded (TUI handles display).

**`HookRunner::run_hook()`**: executes the hook via `HookExecutor::execute()`,
captures stdout/stderr/exit code/duration, emits `HookStarted`/`HookCompleted`
events through the channel, and returns the `HookOutcome`.

**`BufferingOutput`**: a new `Output` implementation that
`HookExecutor::execute()` requires as `&mut dyn Output`. It is a no-op for
display purposes (does not write to stderr) but buffers warnings (deprecation
warnings, trust fingerprint mismatches) so they can be included in the post-TUI
summary. It also handles the `finish_spinner()` call in `execute_legacy()` as a
no-op.

**Handling `FailMode::Abort`**: `HookExecutor::execute()` returns `Err(...)`
when a hook fails with `FailMode::Abort` (via `anyhow::bail!` in
`handle_hook_failure()`). `TuiBridge::run_hook()` catches this error, extracts
the message, emits a `HookCompleted` event with `success: false`, and converts
it to `Ok(HookOutcome { success: false, ... })`. This prevents the error from
propagating up and killing the worker thread. The calling code in
`run_removal_hook()` (`prune.rs:748`) already handles non-success outcomes by
emitting a warning — with `TuiBridge`, this warning goes to the no-op
`ProgressSink`, and the real failure detail is in the `HookCompleted` event.

**No stderr writes**: hook processes have their stdout/stderr piped (captured),
not inherited. This preserves ratatui's exclusive terminal control.

### Channel Access

`execute_prune_task()` gains a new parameter:

```rust
pub fn execute_prune_task(
    // ... existing params ...
    hooks_config: &HooksConfig,
    tx: &mpsc::Sender<DagEvent>,
) -> (TaskStatus, TaskMessage)
```

The `tx` sender comes from the orchestrator closure in `run_tui()`, which
already has access to it. `hooks_config` is passed from the `DaftSettings`
loaded at command startup. Both `sync.rs` and `prune.rs` orchestrator closures
are updated to pass these through.

### New DagEvent Variants

```rust
DagEvent::HookStarted {
    branch_name: String,
    hook_type: HookType,    // from src/hooks/mod.rs
}

DagEvent::HookCompleted {
    branch_name: String,
    hook_type: HookType,
    success: bool,
    warned: bool,            // non-zero exit with FailMode::Warn
    duration: Duration,
    output: Option<String>,  // captured stdout+stderr, only stored on failure/warning
}
```

`HookType` is used directly (not a `String`). This adds an import of
`crate::hooks::HookType` into `sync_dag.rs`. The `core::worktree` module does
not currently depend on `hooks`, but this is acceptable: `HookType` is a simple
enum with no heavy dependencies, and `sync_dag.rs` is already a higher-level
orchestration module (not pure business logic).

### TuiState Processing

Level 0:

- `HookStarted` updates the worktree row's status label (e.g.,
  `Active("pre-remove")`)
- `HookCompleted` with warning sets a `hook_warned: bool` flag on the
  `WorktreeRow`
- `HookCompleted` with failure sets a `hook_failed: bool` flag
- Hook output is stored in a `Vec<HookSummaryEntry>` on `TuiState` for the
  post-TUI summary

Level 1:

- `HookStarted` adds/updates a sub-row beneath the worktree row (a new
  `HookSubRow` struct in `WorktreeRow`)
- `HookCompleted` finalizes the sub-row with checkmark/warning/cross and timing
- Sub-rows are rendered as additional `Row`s in the table, indented with tree
  characters

Both levels:

- `HookCompleted` events with `output: Some(...)` are accumulated for the
  post-TUI summary

### Post-TUI Hook Summary

After the TUI finishes, if any hooks warned or failed, a summary prints:

```
Hooks:
  feat/old: worktree-pre-remove warned (exit 1, 200ms)
    cleanup.sh: /tmp/cache not found, skipping
  feat/stale: worktree-pre-remove failed (exit 1, 150ms)
    setup.sh: critical error
    Prune was aborted for this branch.
```

Hook results are collected from `TuiState` after the render loop completes.
Buffered warnings from `BufferingOutput` (deprecation warnings, trust
fingerprint mismatches) are also printed in this section if present.

If all hooks succeeded: no post-TUI output.

### Thread Safety

`HookExecutor` needs trust DB access and config. These are read-only after
initialization. Each call to `execute_prune_task()` creates its own `TuiBridge`
with its own `HookExecutor`. The `mpsc::Sender<DagEvent>` is `Clone + Send`, so
each worker thread gets a clone.

### Prompt Callbacks

In TUI mode, the prompt callback for untrusted repos cannot work (no interactive
stdin access while ratatui owns the terminal). Behavior:

- Trusted repos (`TrustLevel::Allow`): hooks run.
- Untrusted repos (`TrustLevel::Deny`): hooks are skipped (existing behavior).
- Prompt-level repos (`TrustLevel::Prompt`): hooks are skipped in TUI mode with
  a post-TUI notice suggesting `git daft hooks trust`. This matches the current
  `NullBridge` behavior for this edge case but makes it visible.

## Deferred Branch Handling

`handle_post_tui_deferred()` in `sync_shared.rs` handles pruning the current
worktree after the TUI finishes. It currently uses `NullBridge`. Since this code
runs after the TUI has exited (ratatui terminal is dropped), it can safely use
`CommandBridge` with the full hook renderer writing to stderr.

The function signature changes to accept `HooksConfig`:

```rust
pub fn handle_post_tui_deferred(
    // ... existing params ...
    hooks_config: &HooksConfig,
)
```

Inside, it creates a `CommandBridge` with a `HookExecutor` and a `CliOutput`,
giving the deferred prune full hook support with the standard indicatif
rendering.

## Scope

This design covers `worktree-pre-remove` and `worktree-post-remove` hooks in the
prune and sync TUI paths. The `sync` command's update and rebase phases do not
trigger hooks (no create/remove lifecycle events), so they are unaffected.
