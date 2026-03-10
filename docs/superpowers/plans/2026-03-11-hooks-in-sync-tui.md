# Hooks in Sync/Prune TUI Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development
> (if subagents available) or superpowers:executing-plans to implement this
> plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make worktree-pre-remove and worktree-post-remove hooks execute during
TUI-mode prune/sync, with two-tier verbosity controlling how much hook detail
the user sees.

**Architecture:** Replace `NullBridge` with a new `TuiBridge` that wraps
`HookExecutor` and sends hook lifecycle events through the existing DAG event
channel. A new `BufferingOutput` captures executor warnings without writing to
stderr. The CLI changes from `verbose: bool` to `verbose: u8`
(`ArgAction::Count`), routing `-v` to TUI-with-sub-rows and `-vv` to the
existing sequential path.

**Tech Stack:** Rust, clap (ArgAction::Count), ratatui, mpsc channels, existing
`HookExecutor` / `HookRunner` trait infrastructure.

**Spec:** `docs/superpowers/specs/2026-03-11-hooks-in-sync-tui-design.md`

---

## File Structure

### New files

| File                      | Responsibility                                                                      |
| ------------------------- | ----------------------------------------------------------------------------------- |
| `src/output/buffering.rs` | `BufferingOutput` — no-op `Output` impl that buffers warnings for post-TUI display  |
| `src/core/tui_bridge.rs`  | `TuiBridge` — `ProgressSink + HookRunner` that executes hooks and emits `DagEvent`s |

### Modified files

| File                            | Change                                                                                                                        |
| ------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| `src/commands/prune.rs`         | `verbose: bool` → `u8`, three-way dispatch, pass `hooks_config`+`tx` to orchestrator                                          |
| `src/commands/sync.rs`          | Same as prune.rs                                                                                                              |
| `src/commands/sync_shared.rs`   | `execute_prune_task` gains `hooks_config`+`tx` params; `handle_post_tui_deferred` gains `hooks_config`; post-TUI hook summary |
| `src/core/mod.rs`               | Export `TuiBridge`; `NullBridge` stays for tests                                                                              |
| `src/core/worktree/sync_dag.rs` | Add `HookStarted`/`HookCompleted` to `DagEvent`; import `HookType`                                                            |
| `src/output/mod.rs`             | Add `mod buffering; pub use buffering::BufferingOutput;`                                                                      |
| `src/output/tui/state.rs`       | Handle hook events; `hook_warned`/`hook_failed` flags on `WorktreeRow`; `HookSubRow`; `HookSummaryEntry` on `TuiState`        |
| `src/output/tui/render.rs`      | Render warning badge on status; render hook sub-rows at level 1                                                               |
| `src/output/tui/driver.rs`      | Accept `verbose` level; pass to state for sub-row decisions                                                                   |

### Test files

| File                                                | What it tests                                               |
| --------------------------------------------------- | ----------------------------------------------------------- |
| `src/output/buffering.rs` (inline `#[cfg(test)]`)   | BufferingOutput captures warnings, no-ops other methods     |
| `src/core/tui_bridge.rs` (inline `#[cfg(test)]`)    | TuiBridge sends hook events, catches FailMode::Abort errors |
| `src/output/tui/state.rs` (existing `#[cfg(test)]`) | New tests for HookStarted/HookCompleted event processing    |
| `tests/integration/test_prune.sh`                   | New: hook execution during TUI prune                        |

---

## Chunk 1: CLI Verbosity + BufferingOutput

### Task 1: Change verbose from bool to u8 in prune Args

**Files:**

- Modify: `src/commands/prune.rs:58-60` (Args struct)
- Modify: `src/commands/prune.rs:77-93` (run function)

- [ ] **Step 1: Update the `verbose` field in Args**

In `src/commands/prune.rs`, change:

```rust
#[arg(short, long, help = "Be verbose; show detailed progress")]
verbose: bool,
```

To:

```rust
#[arg(short, long, action = clap::ArgAction::Count,
      help = "Increase verbosity (-v for hook details, -vv for full sequential output)")]
verbose: u8,
```

- [ ] **Step 2: Update run() dispatch logic**

In `src/commands/prune.rs:80`, change `init_logging(args.verbose)` to
`init_logging(args.verbose >= 2)`.

Change the dispatch block (lines 88-93) from:

```rust
if std::io::IsTerminal::is_terminal(&std::io::stderr()) && !args.verbose {
    run_tui(args, settings)
} else {
    run_prune(args, settings)
}
```

To:

```rust
if !std::io::IsTerminal::is_terminal(&std::io::stderr()) || args.verbose >= 2 {
    run_prune(args, settings)
} else {
    run_tui(args, settings)
}
```

- [ ] **Step 3: Update run_prune() to use verbose >= 2**

In `src/commands/prune.rs:97`, change:

```rust
let config = OutputConfig::with_autocd(false, args.verbose, settings.autocd);
```

To:

```rust
let config = OutputConfig::with_autocd(false, args.verbose >= 2, settings.autocd);
```

- [ ] **Step 4: Run clippy and unit tests**

Run: `mise run clippy && mise run test:unit` Expected: All pass (no behavioral
change yet — `-v` still enters TUI, `-vv` enters sequential)

- [ ] **Step 5: Commit**

```bash
git add src/commands/prune.rs
git commit -m "refactor(prune): change verbose from bool to u8 for two-tier verbosity"
```

### Task 2: Change verbose from bool to u8 in sync Args

**Files:**

- Modify: `src/commands/sync.rs:63-64` (Args struct)
- Modify: `src/commands/sync.rs:88-104` (run function)

- [ ] **Step 1: Apply the same changes as Task 1 but in sync.rs**

Same pattern: `verbose: bool` → `verbose: u8` with `ArgAction::Count`. Update
`init_logging(args.verbose >= 2)`. Update dispatch logic (lines 99-104) to
three-way:

```rust
if !std::io::IsTerminal::is_terminal(&std::io::stderr()) || args.verbose >= 2 {
    run_sequential(args, settings)
} else {
    run_tui(args, settings)
}
```

Update `run_sequential` (line 108):

```rust
let config = OutputConfig::with_autocd(false, args.verbose >= 2, settings.autocd);
```

- [ ] **Step 2: Run clippy and unit tests**

Run: `mise run clippy && mise run test:unit` Expected: All pass.

- [ ] **Step 3: Commit**

```bash
git add src/commands/sync.rs
git commit -m "refactor(sync): change verbose from bool to u8 for two-tier verbosity"
```

### Task 3: Create BufferingOutput

**Files:**

- Create: `src/output/buffering.rs`
- Modify: `src/output/mod.rs` (add module declaration and re-export)

- [ ] **Step 1: Write tests for BufferingOutput**

Create `src/output/buffering.rs` with the test module first:

```rust
//! No-op `Output` implementation that buffers warnings for post-TUI display.
//!
//! Used by `TuiBridge` to satisfy `HookExecutor::execute()`'s `&mut dyn Output`
//! requirement without writing to stderr (which ratatui owns).

use super::{Output, OutputConfig};
use std::path::Path;

/// An `Output` implementation that captures warnings and discards everything else.
///
/// Designed for TUI mode where stderr is owned by ratatui. Warnings from
/// `HookExecutor` (deprecation notices, trust fingerprint mismatches) are
/// buffered and can be retrieved after the TUI exits for a post-TUI summary.
pub struct BufferingOutput {
    warnings: Vec<String>,
}

impl BufferingOutput {
    pub fn new() -> Self {
        Self {
            warnings: Vec::new(),
        }
    }

    /// Take all buffered warnings, draining the internal buffer.
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }
}

impl Output for BufferingOutput {
    fn info(&mut self, _msg: &str) {}
    fn success(&mut self, _msg: &str) {}
    fn warning(&mut self, msg: &str) {
        self.warnings.push(msg.to_string());
    }
    fn error(&mut self, _msg: &str) {}
    fn debug(&mut self, _msg: &str) {}
    fn step(&mut self, _msg: &str) {}
    fn result(&mut self, _msg: &str) {}
    fn detail(&mut self, _key: &str, _value: &str) {}
    fn list_item(&mut self, _msg: &str) {}
    fn operation_start(&mut self, _operation: &str) {}
    fn operation_end(&mut self, _operation: &str, _success: bool) {}
    fn start_spinner(&mut self, _msg: &str) {}
    fn finish_spinner(&mut self) {}
    fn cd_path(&mut self, _path: &Path) {}
    fn raw(&mut self, _msg: &str) {}

    fn is_quiet(&self) -> bool {
        false
    }

    fn is_verbose(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffers_warnings() {
        let mut output = BufferingOutput::new();
        output.warning("first warning");
        output.warning("second warning");
        let warnings = output.take_warnings();
        assert_eq!(warnings, vec!["first warning", "second warning"]);
    }

    #[test]
    fn take_warnings_drains_buffer() {
        let mut output = BufferingOutput::new();
        output.warning("warning");
        let _ = output.take_warnings();
        let warnings = output.take_warnings();
        assert!(warnings.is_empty());
    }

    #[test]
    fn discards_non_warning_messages() {
        let mut output = BufferingOutput::new();
        output.info("info");
        output.success("success");
        output.error("error");
        output.debug("debug");
        output.step("step");
        output.result("result");
        output.detail("detail");
        output.list_item("item");
        output.start_spinner("spin");
        output.finish_spinner();
        let warnings = output.take_warnings();
        assert!(warnings.is_empty());
    }
}
```

- [ ] **Step 2: Add module to output/mod.rs**

In `src/output/mod.rs`, add alongside existing module declarations:

```rust
mod buffering;
pub use buffering::BufferingOutput;
```

Note: look at the existing pattern in `mod.rs` for where modules are declared
and where public re-exports happen. Follow the same style.

- [ ] **Step 3: Run tests**

Run: `mise run test:unit` Expected: All pass including the 3 new BufferingOutput
tests.

- [ ] **Step 4: Run clippy**

Run: `mise run clippy` Expected: Zero warnings.

- [ ] **Step 5: Commit**

```bash
git add src/output/buffering.rs src/output/mod.rs
git commit -m "feat: add BufferingOutput for TUI-mode hook execution"
```

---

## Chunk 2: DagEvent Hook Variants + TuiBridge

### Task 4: Add HookStarted/HookCompleted to DagEvent

**Files:**

- Modify: `src/core/worktree/sync_dag.rs:267-285` (DagEvent enum)

- [ ] **Step 1: Add the new variants to DagEvent**

In `src/core/worktree/sync_dag.rs`, add to the `DagEvent` enum (after the
existing `AllDone` variant), and add the necessary imports:

Add to imports at top of file:

```rust
use crate::hooks::HookType;
use std::time::Duration;
```

Add variants:

```rust
/// A hook started running for a branch.
HookStarted {
    branch_name: String,
    hook_type: HookType,
},
/// A hook completed for a branch.
HookCompleted {
    branch_name: String,
    hook_type: HookType,
    success: bool,
    /// Non-zero exit with FailMode::Warn.
    warned: bool,
    duration: Duration,
    /// Captured stdout+stderr, only stored on failure/warning.
    output: Option<String>,
},
```

- [ ] **Step 2: Run clippy and tests**

Run: `mise run clippy && mise run test:unit` Expected: All pass. The new
variants are not yet used, but the enum compiles. Existing match statements on
`DagEvent` in `state.rs` will need a wildcard or explicit arms — check if clippy
warns about non-exhaustive match. If so, add
`DagEvent::HookStarted { .. } | DagEvent::HookCompleted { .. } => {}` to the
match in `TuiState::apply_event()` as a placeholder.

- [ ] **Step 3: Commit**

```bash
git add src/core/worktree/sync_dag.rs src/output/tui/state.rs
git commit -m "feat: add HookStarted/HookCompleted variants to DagEvent"
```

### Task 5: Create TuiBridge

**Files:**

- Create: `src/core/tui_bridge.rs`
- Modify: `src/core/mod.rs` (add module declaration and re-export)

- [ ] **Step 1: Write TuiBridge with tests**

Create `src/core/tui_bridge.rs`:

```rust
//! TUI-compatible bridge that executes hooks and reports via DagEvents.
//!
//! Replaces `NullBridge` in TUI-mode prune/sync workers. Hooks are executed
//! with captured output (no stderr writes), and lifecycle events are sent
//! through the DAG channel for the TUI renderer to display.

use super::{HookOutcome, HookRunner, ProgressSink};
use crate::hooks::{HookContext, HookExecutor, HooksConfig};
use crate::output::BufferingOutput;
use crate::core::worktree::sync_dag::DagEvent;
use anyhow::Result;
use std::sync::mpsc;
use std::time::Instant;

/// A combined sink that executes hooks and sends events to the TUI.
pub struct TuiBridge {
    executor: HookExecutor,
    output: BufferingOutput,
    tx: mpsc::Sender<DagEvent>,
    branch_name: String,
}

impl TuiBridge {
    /// Create a new TuiBridge.
    ///
    /// # Arguments
    /// * `hooks_config` - Hook configuration (enabled, trust, timeouts, etc.)
    /// * `tx` - Channel sender for DagEvents to the TUI renderer
    /// * `branch_name` - Branch name for identifying hook events
    pub fn new(
        hooks_config: HooksConfig,
        tx: mpsc::Sender<DagEvent>,
        branch_name: String,
    ) -> Result<Self> {
        let executor = HookExecutor::new(hooks_config)?;
        Ok(Self {
            executor,
            output: BufferingOutput::new(),
            tx,
            branch_name,
        })
    }

    /// Take any buffered warnings from the hook executor.
    ///
    /// These are warnings emitted by `HookExecutor::execute()` (deprecation
    /// notices, trust fingerprint mismatches) that could not be shown during
    /// TUI rendering.
    pub fn take_warnings(&mut self) -> Vec<String> {
        self.output.take_warnings()
    }
}

impl ProgressSink for TuiBridge {
    fn on_step(&mut self, _msg: &str) {}
    fn on_warning(&mut self, _msg: &str) {}
    fn on_debug(&mut self, _msg: &str) {}
}

impl HookRunner for TuiBridge {
    fn run_hook(&mut self, ctx: &HookContext) -> Result<HookOutcome> {
        let start = Instant::now();

        // Execute the hook, catching FailMode::Abort errors.
        // HookStarted is sent AFTER we know the hook will actually run
        // (not skipped due to trust, disabled hooks, missing files, etc.).
        let result = self.executor.execute(ctx, &mut self.output);
        let duration = start.elapsed();

        match result {
            Ok(hook_result) => {
                // Don't send events for skipped hooks (no hook files, disabled, etc.)
                // This prevents the TUI from showing "pre-remove" status for repos
                // where hooks don't exist or aren't trusted.
                if !hook_result.skipped {
                    let warned = !hook_result.success;
                    let output = if warned {
                        let mut parts = Vec::new();
                        if !hook_result.stdout.is_empty() {
                            parts.push(hook_result.stdout.trim().to_string());
                        }
                        if !hook_result.stderr.is_empty() {
                            parts.push(hook_result.stderr.trim().to_string());
                        }
                        if parts.is_empty() { None } else { Some(parts.join("\n")) }
                    } else {
                        None
                    };

                    // Send both events for non-skipped hooks. HookStarted is
                    // sent retroactively — the TUI may briefly not show the
                    // hook phase, but this is preferable to showing it for
                    // hooks that don't actually run.
                    let _ = self.tx.send(DagEvent::HookStarted {
                        branch_name: self.branch_name.clone(),
                        hook_type: ctx.hook_type,
                    });
                    let _ = self.tx.send(DagEvent::HookCompleted {
                        branch_name: self.branch_name.clone(),
                        hook_type: ctx.hook_type,
                        success: hook_result.success,
                        warned,
                        duration,
                        output,
                    });
                }

                Ok(HookOutcome {
                    success: hook_result.success,
                    skipped: hook_result.skipped,
                    skip_reason: hook_result.skip_reason.clone(),
                })
            }
            Err(e) => {
                // FailMode::Abort causes HookExecutor to bail!().
                // Catch it here and convert to a failed HookOutcome.
                let _ = self.tx.send(DagEvent::HookStarted {
                    branch_name: self.branch_name.clone(),
                    hook_type: ctx.hook_type,
                });
                let _ = self.tx.send(DagEvent::HookCompleted {
                    branch_name: self.branch_name.clone(),
                    hook_type: ctx.hook_type,
                    success: false,
                    warned: false,
                    duration,
                    output: Some(e.to_string()),
                });

                Ok(HookOutcome {
                    success: false,
                    skipped: false,
                    skip_reason: None,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::{HookType, HooksConfig};
    use std::sync::mpsc;

    #[test]
    fn tui_bridge_creation() {
        let (tx, _rx) = mpsc::channel();
        let config = HooksConfig {
            enabled: false,
            ..Default::default()
        };
        let bridge = TuiBridge::new(config, tx, "test-branch".into());
        assert!(bridge.is_ok());
    }

    #[test]
    fn skipped_hooks_send_no_events() {
        let (tx, rx) = mpsc::channel();
        let config = HooksConfig {
            enabled: false,
            ..Default::default()
        };
        let mut bridge = TuiBridge::new(config, tx, "test-branch".into()).unwrap();

        let ctx = HookContext::new(
            HookType::PreRemove,
            "prune",
            std::path::Path::new("/tmp/test"),
            std::path::Path::new("/tmp/test/.git"),
            "origin",
            std::path::Path::new("/tmp/test/main"),
            std::path::Path::new("/tmp/test/feat"),
            "feat",
        );

        let outcome = bridge.run_hook(&ctx).unwrap();
        assert!(outcome.skipped);

        // Skipped hooks should send neither HookStarted nor HookCompleted
        drop(bridge);
        let events: Vec<DagEvent> = rx.iter().collect();
        let hook_event_count = events.iter().filter(|e|
            matches!(e, DagEvent::HookStarted { .. } | DagEvent::HookCompleted { .. })
        ).count();
        assert_eq!(hook_event_count, 0, "Skipped hooks should not send any hook events");
    }

    #[test]
    fn progress_sink_is_noop() {
        let (tx, _rx) = mpsc::channel();
        let config = HooksConfig {
            enabled: false,
            ..Default::default()
        };
        let mut bridge = TuiBridge::new(config, tx, "test".into()).unwrap();
        // These should not panic
        bridge.on_step("step");
        bridge.on_warning("warning");
        bridge.on_debug("debug");
    }
}
```

- [ ] **Step 2: Add module to core/mod.rs**

In `src/core/mod.rs`, add:

```rust
mod tui_bridge;
pub use tui_bridge::TuiBridge;
```

Keep `NullBridge` — it is still used by tests and other contexts.

- [ ] **Step 3: Run tests**

Run: `mise run test:unit` Expected: All pass including the 3 new TuiBridge
tests.

- [ ] **Step 4: Run clippy**

Run: `mise run clippy` Expected: Zero warnings.

- [ ] **Step 5: Commit**

```bash
git add src/core/tui_bridge.rs src/core/mod.rs
git commit -m "feat: add TuiBridge for hook execution in TUI mode"
```

---

## Chunk 3: TuiState Hook Event Processing + Rendering

### Task 6: Add hook state to WorktreeRow and process hook events

**Files:**

- Modify: `src/output/tui/state.rs`

- [ ] **Step 1: Add hook-related types and fields**

In `src/output/tui/state.rs`, add these types and update `WorktreeRow` and
`TuiState`:

```rust
use crate::hooks::HookType;
use std::time::Duration;

/// Status of a single hook sub-row (for -v mode).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookSubStatus {
    Running,
    Succeeded(Duration),
    Warned(Duration),
    Failed(Duration),
}

/// A hook sub-row displayed beneath a worktree row in -v mode.
#[derive(Debug, Clone)]
pub struct HookSubRow {
    pub hook_type: HookType,
    pub status: HookSubStatus,
}

/// Entry for the post-TUI hook summary (printed after TUI exits on warning/failure).
#[derive(Debug, Clone)]
pub struct HookSummaryEntry {
    pub branch_name: String,
    pub hook_type: HookType,
    pub success: bool,
    pub warned: bool,
    pub duration: Duration,
    pub output: Option<String>,
}
```

Add to `WorktreeRow`:

```rust
pub struct WorktreeRow {
    pub info: WorktreeInfo,
    pub status: WorktreeStatus,
    pub hook_warned: bool,
    pub hook_failed: bool,
    pub hook_sub_rows: Vec<HookSubRow>,
}
```

Add to `TuiState`:

```rust
pub struct TuiState {
    // ... existing fields ...
    pub hook_summaries: Vec<HookSummaryEntry>,
    pub show_hook_sub_rows: bool,  // true when verbose == 1
}
```

Update `TuiState::new()` to initialize the new fields (`hook_summaries`,
`show_hook_sub_rows`). Update **all** `WorktreeRow` construction sites to
include `hook_warned: false`, `hook_failed: false`, `hook_sub_rows: Vec::new()`.
This includes both the initial construction in `TuiState::new()` (line 75-79)
AND the auto-creation for discovered branches in `apply_event` (line 101-104).

- [ ] **Step 2: Handle hook events in apply_event**

Add to the match in `TuiState::apply_event()`:

```rust
DagEvent::HookStarted { branch_name, hook_type } => {
    if let Some(row) = self.find_row_mut(branch_name) {
        // Update status label to show current hook phase
        row.status = WorktreeStatus::Active(hook_type.filename().to_string());
        // Add sub-row if in verbose TUI mode
        if self.show_hook_sub_rows {
            row.hook_sub_rows.push(HookSubRow {
                hook_type: *hook_type,
                status: HookSubStatus::Running,
            });
        }
    }
}
DagEvent::HookCompleted {
    branch_name,
    hook_type,
    success,
    warned,
    duration,
    output,
} => {
    if let Some(row) = self.find_row_mut(branch_name) {
        if *warned {
            row.hook_warned = true;
        }
        if !success {
            row.hook_failed = true;
        }
        // Update sub-row status
        if self.show_hook_sub_rows {
            if let Some(sub) = row.hook_sub_rows.iter_mut().rfind(|s| s.hook_type == *hook_type) {
                sub.status = if *warned {
                    HookSubStatus::Warned(*duration)
                } else if *success {
                    HookSubStatus::Succeeded(*duration)
                } else {
                    HookSubStatus::Failed(*duration)
                };
            }
        }
    }
    // Accumulate for post-TUI summary if non-success
    if *warned || !success {
        self.hook_summaries.push(HookSummaryEntry {
            branch_name: branch_name.clone(),
            hook_type: *hook_type,
            success: *success,
            warned: *warned,
            duration: *duration,
            output: output.clone(),
        });
    }
}
```

- [ ] **Step 3: Write tests for hook event processing**

Add to the existing `mod tests` in `state.rs`:

```rust
#[test]
fn hook_started_updates_status_label() {
    let mut state = make_test_state();
    state.apply_event(&DagEvent::HookStarted {
        branch_name: "feat/old".into(),
        hook_type: HookType::PreRemove,
    });
    let row = state.worktrees.iter().find(|w| w.info.name == "feat/old").unwrap();
    assert_eq!(row.status, WorktreeStatus::Active("worktree-pre-remove".into()));
}

#[test]
fn hook_completed_warn_sets_flag() {
    let mut state = make_test_state();
    state.apply_event(&DagEvent::HookCompleted {
        branch_name: "feat/old".into(),
        hook_type: HookType::PreRemove,
        success: true,
        warned: true,
        duration: Duration::from_millis(200),
        output: Some("warning output".into()),
    });
    let row = state.worktrees.iter().find(|w| w.info.name == "feat/old").unwrap();
    assert!(row.hook_warned);
    assert_eq!(state.hook_summaries.len(), 1);
}

#[test]
fn hook_completed_success_no_summary() {
    let mut state = make_test_state();
    state.apply_event(&DagEvent::HookCompleted {
        branch_name: "feat/old".into(),
        hook_type: HookType::PreRemove,
        success: true,
        warned: false,
        duration: Duration::from_millis(100),
        output: None,
    });
    let row = state.worktrees.iter().find(|w| w.info.name == "feat/old").unwrap();
    assert!(!row.hook_warned);
    assert!(!row.hook_failed);
    assert!(state.hook_summaries.is_empty());
}

#[test]
fn hook_sub_rows_populated_when_verbose() {
    let mut state = make_test_state();
    state.show_hook_sub_rows = true;
    state.apply_event(&DagEvent::HookStarted {
        branch_name: "feat/old".into(),
        hook_type: HookType::PreRemove,
    });
    let row = state.worktrees.iter().find(|w| w.info.name == "feat/old").unwrap();
    assert_eq!(row.hook_sub_rows.len(), 1);
    assert_eq!(row.hook_sub_rows[0].hook_type, HookType::PreRemove);
    assert_eq!(row.hook_sub_rows[0].status, HookSubStatus::Running);

    state.apply_event(&DagEvent::HookCompleted {
        branch_name: "feat/old".into(),
        hook_type: HookType::PreRemove,
        success: true,
        warned: false,
        duration: Duration::from_millis(120),
        output: None,
    });
    let row = state.worktrees.iter().find(|w| w.info.name == "feat/old").unwrap();
    assert_eq!(row.hook_sub_rows[0].status, HookSubStatus::Succeeded(Duration::from_millis(120)));
}
```

Note: `make_test_state()` needs updating to initialize the new fields. Also add
`use crate::hooks::HookType;` and `use std::time::Duration;` to the test module
imports.

- [ ] **Step 4: Run tests**

Run: `mise run test:unit` Expected: All pass including the 4 new state tests.

- [ ] **Step 5: Run clippy**

Run: `mise run clippy` Expected: Zero warnings.

- [ ] **Step 6: Commit**

```bash
git add src/output/tui/state.rs
git commit -m "feat: process HookStarted/HookCompleted events in TuiState"
```

### Task 7: Render hook status and sub-rows

**Files:**

- Modify: `src/output/tui/render.rs`

- [ ] **Step 1: Add warning badge to pruned status**

In `render_status_cell()` (`render.rs:290`), modify the `FinalStatus::Pruned`
arm to check for warning/failure flags. The function needs access to the
`WorktreeRow`'s `hook_warned` and `hook_failed` flags, so its signature needs to
change from taking `&WorktreeStatus` to taking `&WorktreeRow`.

Update `FinalStatus::Pruned` rendering:

```rust
FinalStatus::Pruned => {
    if wt.hook_failed {
        Cell::from(Line::from(Span::styled(
            format!("{CROSS} hook failed"),
            Style::default().fg(Color::Red),
        )))
    } else if wt.hook_warned {
        Cell::from(Line::from(vec![
            Span::styled(format!("{DASH} pruned "), Style::default().fg(Color::Red)),
            Span::styled("\u{26A0}", Style::default().fg(Color::Yellow)),
        ]))
    } else {
        Cell::from(Line::from(Span::styled(
            format!("{DASH} pruned"),
            Style::default().fg(Color::Red),
        )))
    }
}
```

Note: `render_status_cell` currently takes `&WorktreeStatus`. To access
`hook_warned` and `hook_failed`, change it to take `&WorktreeRow` instead.
Update the call site in `render_cell()` (line 249) from
`render_status_cell(&wt.status, tick)` to `render_status_cell(wt, tick)`.

Also add a `FinalStatus::HookFailed` variant if that is cleaner than checking
`hook_failed` on the row. The spec uses `cross hook failed` as a distinct
status, which suggests a dedicated `FinalStatus` variant. Evaluate which
approach is cleaner — adding a variant means updating `map_final_status()` too.
If using the flag approach, add `hook_warned` and `hook_failed` checks in the
Pruned arm as shown above.

- [ ] **Step 2: Render hook sub-rows in the table**

In `render_table()` (`render.rs:55`), the rows vector is built from
`state.worktrees`. When `state.show_hook_sub_rows` is true, after each worktree
row that has `hook_sub_rows`, insert additional `Row`s:

```rust
let rows: Vec<Row> = state
    .worktrees
    .iter()
    .zip(row_vals.iter())
    .flat_map(|(wt, vals)| {
        let main_cells: Vec<Cell> = columns
            .iter()
            .map(|col| render_cell(col, wt, vals, state.tick, state.stat))
            .collect();
        let mut result = vec![Row::new(main_cells)];

        // Add hook sub-rows if present
        if state.show_hook_sub_rows && !wt.hook_sub_rows.is_empty() {
            for (i, sub) in wt.hook_sub_rows.iter().enumerate() {
                let is_last = i == wt.hook_sub_rows.len() - 1;
                let prefix = if is_last { "\u{2514} " } else { "\u{251C} " };
                let sub_row = render_hook_sub_row(sub, prefix, state.tick);
                result.push(sub_row);
            }
        }

        result
    })
    .collect();
```

Add the `render_hook_sub_row` helper:

```rust
fn render_hook_sub_row(sub: &HookSubRow, prefix: &str, tick: usize) -> Row<'static> {
    let name = sub.hook_type.filename();
    let status_span = match &sub.status {
        HookSubStatus::Running => {
            let spinner = SPINNER_FRAMES[tick % SPINNER_FRAMES.len()];
            Span::styled(
                spinner.to_string(),
                Style::default().fg(Color::Yellow),
            )
        }
        HookSubStatus::Succeeded(d) => Span::styled(
            format!("{CHECKMARK} {}ms", d.as_millis()),
            Style::default().fg(Color::Green),
        ),
        HookSubStatus::Warned(d) => Span::styled(
            format!("\u{26A0} {}ms", d.as_millis()),
            Style::default().fg(Color::Yellow),
        ),
        HookSubStatus::Failed(d) => Span::styled(
            format!("{CROSS} {}ms", d.as_millis()),
            Style::default().fg(Color::Red),
        ),
    };

    // Sub-rows span the entire width, rendered as a single cell
    let line = Line::from(vec![
        Span::styled(format!("  {prefix}"), Style::default().add_modifier(Modifier::DIM)),
        Span::styled(format!("{name} "), Style::default().add_modifier(Modifier::DIM)),
        status_span,
    ]);

    Row::new(vec![Cell::from(line)])
}
```

Note: Sub-rows use a single cell spanning the first column. The table's
`Constraint` layout may need adjustment — investigate whether ratatui allows
cells that span multiple columns, or whether this needs a different layout
approach (e.g., rendering sub-rows outside the `Table` widget). If spanning is
not straightforward, place the sub-row content in the Status column and leave
other columns empty.

- [ ] **Step 3: Run clippy and tests**

Run: `mise run clippy && mise run test:unit` Expected: All pass. Visual
rendering is best verified by manual testing in a later task.

- [ ] **Step 4: Commit**

```bash
git add src/output/tui/render.rs
git commit -m "feat: render hook warning badges and sub-rows in TUI"
```

---

## Chunk 4: Wire Everything Together

### Task 8: Update execute_prune_task to use TuiBridge

**Files:**

- Modify: `src/commands/sync_shared.rs:26-83` (execute_prune_task)

- [ ] **Step 1: Add hooks_config and tx parameters**

Update the `execute_prune_task` signature to accept the new parameters:

```rust
pub fn execute_prune_task(
    branch_name: &str,
    settings: &DaftSettings,
    project_root: &std::path::Path,
    git_dir: &std::path::Path,
    remote_name: &str,
    source_worktree: &std::path::Path,
    worktree_map: &HashMap<String, (PathBuf, bool)>,
    is_bare_layout: bool,
    current_wt_path: &Option<PathBuf>,
    current_branch: &Option<String>,
    force: bool,
    hooks_config: &HooksConfig,
    tx: &std::sync::mpsc::Sender<DagEvent>,
) -> (TaskStatus, TaskMessage)
```

- [ ] **Step 2: Replace NullBridge with TuiBridge**

Replace `let mut sink = NullBridge;` (line 56) with:

```rust
let mut sink = match TuiBridge::new(
    hooks_config.clone(),
    tx.clone(),
    branch_name.to_string(),
) {
    Ok(bridge) => bridge,
    Err(e) => {
        return (
            TaskStatus::Failed,
            TaskMessage::Failed(format!("failed to initialize hooks: {e}")),
        );
    }
};
```

Update imports at the top of `sync_shared.rs` to use `TuiBridge` instead of
`NullBridge`:

```rust
use crate::core::TuiBridge;
use crate::hooks::HooksConfig;
```

Remove `NullBridge` from the imports (but keep it imported if
`handle_post_tui_deferred` still uses it — that gets updated in Task 9).

- [ ] **Step 3: Run clippy**

Run: `mise run clippy` Expected: Errors in `prune.rs` and `sync.rs` because the
call sites for `execute_prune_task` don't pass the new parameters yet. That's
expected — we'll fix them in the next steps.

- [ ] **Step 4: Commit (WIP — does not compile yet)**

```bash
git add src/commands/sync_shared.rs
git commit -m "wip: update execute_prune_task to use TuiBridge"
```

### Task 9: Update handle_post_tui_deferred to use CommandBridge

**Files:**

- Modify: `src/commands/sync_shared.rs:120-163` (handle_post_tui_deferred)

- [ ] **Step 1: Add hooks_config parameter and use CommandBridge**

Update the function signature:

```rust
pub fn handle_post_tui_deferred(
    deferred_branch: &std::sync::Arc<std::sync::Mutex<Option<String>>>,
    settings: &DaftSettings,
    project_root: &std::path::Path,
    git_dir: std::path::PathBuf,
    source_worktree: std::path::PathBuf,
    worktree_map: &HashMap<String, (PathBuf, bool)>,
    force: bool,
    hooks_config: &HooksConfig,
)
```

Replace `let mut sink = NullBridge;` (line 146) with:

```rust
let config = OutputConfig::with_autocd(false, false, settings.autocd);
let mut cli_output = CliOutput::new(config);
let executor = HookExecutor::new(hooks_config.clone())
    .unwrap_or_else(|_| HookExecutor::new(HooksConfig { enabled: false, ..Default::default() }).unwrap());
let mut sink = CommandBridge::new(&mut cli_output, executor);
```

Add the necessary imports (`OutputConfig`, `CliOutput`, `HookExecutor`,
`CommandBridge`).

After `handle_deferred_prune`, the `sink` is dropped and `cli_output` is
available again for the cd_path output that follows.

Note: The existing code after `handle_deferred_prune` creates its own
`CliOutput` for the cd_path message. Restructure so the same `CliOutput` is
used, or keep separate instances if the borrow checker requires it. The key
point is: the `CommandBridge` wrapping `cli_output` must be dropped before
`cli_output` is used again for cd_path output. Use a scoped block:

```rust
{
    let mut sink = CommandBridge::new(&mut cli_output, executor);
    cd_target = prune::handle_deferred_prune(&ctx, branch_name, worktree_map, &params, &mut sink);
}
// sink dropped, cli_output available again
```

- [ ] **Step 2: Remove NullBridge import from sync_shared.rs if no longer used**

- [ ] **Step 3: Commit (WIP)**

```bash
git add src/commands/sync_shared.rs
git commit -m "wip: update handle_post_tui_deferred to use CommandBridge"
```

### Task 10: Update prune.rs orchestrator to pass new params

**Files:**

- Modify: `src/commands/prune.rs:150-306` (run_tui)

- [ ] **Step 1: Load HooksConfig and pass to execute_prune_task**

In `run_tui()`, create hooks config (all commands in the codebase use the
default constructor directly):

```rust
let hooks_config = HooksConfig::default();
```

In the orchestrator closure where `execute_prune_task` is called (around line
256-268), pass the new parameters. The closure already has access to `tx` (the
channel sender). Pass `&hooks_config` and `&tx`:

```rust
TaskId::Prune(ref branch) => {
    let (status, msg) = sync_shared::execute_prune_task(
        branch,
        &settings,
        &project_root,
        &git_dir,
        &remote_name,
        &source_worktree,
        &worktree_map,
        is_bare_layout,
        &current_wt_path,
        &current_branch,
        force,
        &hooks_config,
        &tx,
    );
    (status, msg, None)
}
```

- [ ] **Step 2: Pass hooks_config to handle_post_tui_deferred**

Update the call to `handle_post_tui_deferred` (around line 292-300) to pass
`&hooks_config`.

- [ ] **Step 3: Pass verbose level to TuiState**

Pass `args.verbose` to `TuiState::new()` so it can set `show_hook_sub_rows`.
Update `TuiState::new()` signature to accept `verbose: u8` and set
`show_hook_sub_rows: verbose >= 1`.

- [ ] **Step 4: Add post-TUI hook summary**

After `handle_post_tui_deferred` and before `check_tui_failures`, print the hook
summary if any entries exist:

```rust
if !final_state.hook_summaries.is_empty() {
    eprintln!();
    eprintln!("Hooks:");
    for entry in &final_state.hook_summaries {
        let status_word = if entry.warned { "warned" } else { "failed" };
        eprintln!(
            "  {}: {} {} ({}, {}ms)",
            entry.branch_name,
            entry.hook_type.filename(),
            status_word,
            if entry.warned { "continuing" } else { "aborted" },
            entry.duration.as_millis(),
        );
        if let Some(ref output) = entry.output {
            for line in output.lines() {
                eprintln!("    {line}");
            }
        }
    }
}
```

- [ ] **Step 5: Run clippy and tests**

Run: `mise run clippy && mise run test:unit` Expected: Prune path compiles. Sync
path may still have errors (next task).

- [ ] **Step 6: Commit**

```bash
git add src/commands/prune.rs src/output/tui/state.rs
git commit -m "feat(prune): wire TuiBridge into TUI orchestrator"
```

### Task 11: Update sync.rs orchestrator to pass new params

**Files:**

- Modify: `src/commands/sync.rs:142-391` (run_tui)

- [ ] **Step 1: Apply same changes as Task 10 for sync**

Same pattern: load `hooks_config`, pass `&hooks_config` and `&tx` to
`execute_prune_task` call (around line 301-313), pass `&hooks_config` to
`handle_post_tui_deferred` (if sync uses it), pass verbose to TuiState, add
post-TUI hook summary.

- [ ] **Step 2: Run full test suite**

Run: `mise run clippy && mise run test:unit` Expected: All pass. Both prune and
sync TUI paths now compile with TuiBridge.

- [ ] **Step 3: Commit**

```bash
git add src/commands/sync.rs
git commit -m "feat(sync): wire TuiBridge into TUI orchestrator"
```

### Task 12: Update TuiRenderer to account for sub-row viewport height

**Files:**

- Modify: `src/output/tui/driver.rs`

- [ ] **Step 1: Accept verbose level and compute extra rows**

Update `TuiRenderer::new()` or add a builder method so that when
`show_hook_sub_rows` is true, the viewport height accounts for potential
sub-rows. The `with_extra_rows()` method already exists — compute the extra rows
needed:

In the caller (prune.rs/sync.rs), before creating the TuiRenderer, compute extra
rows. Note: `gone_branches` are discovered after fetch (inside the orchestrator
thread), so they are not available at TuiRenderer creation time. Use a
conservative upper bound based on the total worktree count:

```rust
let hook_extra_rows = if args.verbose >= 1 {
    // Budget 2 sub-rows per worktree (pre-remove + post-remove).
    // Not all worktrees will have hooks, but ratatui inline viewport
    // cannot grow after creation, so over-allocate. Extra empty rows
    // at the bottom are harmless.
    (worktree_infos.len() as u16) * 2
} else {
    0
};
```

Pass this to
`TuiRenderer::new(...).with_extra_rows(existing_extra + hook_extra_rows)`. The
`existing_extra` comes from the current `extra_rows` computation (if any).

- [ ] **Step 2: Run clippy and tests**

Run: `mise run clippy && mise run test:unit` Expected: All pass.

- [ ] **Step 3: Commit**

```bash
git add src/output/tui/driver.rs src/commands/prune.rs src/commands/sync.rs
git commit -m "feat: compute viewport extra rows for hook sub-rows"
```

---

## Chunk 5: Integration Tests + Cleanup

### Task 13: Run fmt, clippy, and full test suite

- [ ] **Step 1: Format**

Run: `mise run fmt`

- [ ] **Step 2: Run clippy**

Run: `mise run clippy` Expected: Zero warnings. Fix any issues.

- [ ] **Step 3: Run full test suite**

Run: `mise run test` Expected: All unit and integration tests pass.

- [ ] **Step 4: Commit any fixes**

### Task 14: Add integration test for hook execution during prune

**Files:**

- Modify: `tests/integration/test_prune.sh`

- [ ] **Step 1: Add test_prune_hooks_execute_in_tui**

Add a new test function that:

1. Creates a repo with worktrees
2. Adds trusted hooks (pre-remove and post-remove) that write marker files
3. Simulates a branch deletion on the remote
4. Runs `daft prune` (which uses TUI mode)
5. Verifies the marker files were created (proving hooks ran)

Follow the existing test patterns in `test_prune.sh`. Key patterns to follow:

- Use `setup_prune_test` helper to create the test environment
- Use `DAFT_TESTING=1` to suppress hook renderer output
- Trust the repo with `git daft hooks trust`
- Create hooks in `.daft/hooks/` with correct permissions

```bash
test_prune_hooks_execute_in_tui() {
    local test_dir
    test_dir=$(setup_prune_test "hooks-tui")

    local bare_dir="$test_dir/bare"
    local main_wt="$test_dir/bare/main"

    # Trust the repo
    cd "$main_wt"
    git daft hooks trust

    # Create pre-remove hook that writes a marker
    mkdir -p "$main_wt/.daft/hooks"
    cat > "$main_wt/.daft/hooks/worktree-pre-remove" << 'HOOK'
#!/bin/bash
touch "${DAFT_PROJECT_ROOT}/.pre-remove-ran-${DAFT_BRANCH_NAME}"
HOOK
    chmod +x "$main_wt/.daft/hooks/worktree-pre-remove"

    # Create post-remove hook
    cat > "$main_wt/.daft/hooks/worktree-post-remove" << 'HOOK'
#!/bin/bash
touch "${DAFT_PROJECT_ROOT}/.post-remove-ran-${DAFT_BRANCH_NAME}"
HOOK
    chmod +x "$main_wt/.daft/hooks/worktree-post-remove"

    # Commit the hooks so they exist in all worktrees
    git add .daft/hooks/
    GIT_AUTHOR_NAME="Test" GIT_AUTHOR_EMAIL="test@test.com" \
    GIT_COMMITTER_NAME="Test" GIT_COMMITTER_EMAIL="test@test.com" \
    git commit -m "Add hooks"
    git push origin main

    # Create a feature branch + worktree, push, then delete remote branch
    cd "$main_wt"
    git worktree add ../feat-hook feat-hook 2>/dev/null || \
        git worktree add -b feat-hook ../feat-hook
    cd ../feat-hook
    GIT_AUTHOR_NAME="Test" GIT_AUTHOR_EMAIL="test@test.com" \
    GIT_COMMITTER_NAME="Test" GIT_COMMITTER_EMAIL="test@test.com" \
    git commit --allow-empty -m "feat commit"
    git push origin feat-hook

    # Delete the remote branch
    git push origin --delete feat-hook

    # Run prune
    cd "$main_wt"
    git worktree-prune 2>&1 || true

    # Verify hooks ran
    assert_file_exists "$bare_dir/.pre-remove-ran-feat-hook" \
        "pre-remove hook should have run"
    assert_file_exists "$bare_dir/.post-remove-ran-feat-hook" \
        "post-remove hook should have run"

    # Cleanup
    rm -rf "$test_dir"
    pass "prune hooks execute in TUI mode"
}
```

Note: Adjust the test setup based on how existing prune tests create their
environment. Check `setup_prune_test` for the exact helper patterns. The hook
environment variables (`DAFT_PROJECT_ROOT`, `DAFT_BRANCH`) should be verified
against `src/hooks/environment.rs` to use the correct variable names.

- [ ] **Step 2: Register the new test in run_prune_tests**

Add `test_prune_hooks_execute_in_tui` to the `run_prune_tests` function at the
bottom of `test_prune.sh`.

- [ ] **Step 3: Run integration tests**

Run: `mise run test:integration` Expected: All pass including the new hook test.

- [ ] **Step 4: Commit**

```bash
git add tests/integration/test_prune.sh
git commit -m "test: add integration test for hook execution during TUI prune"
```

### Task 15: Final verification and cleanup

- [ ] **Step 1: Run the full CI suite locally**

Run: `mise run ci` Expected: All checks pass (fmt, clippy, unit tests,
integration tests, man page verification, CLI docs verification).

- [ ] **Step 2: Remove plan and spec files from the branch**

The spec and plan files in `docs/superpowers/` should not be committed to the
main branch. Remove them:

```bash
git rm -r docs/superpowers/
git commit -m "chore: remove implementation plan files"
```

- [ ] **Step 3: Verify man pages are up to date**

The `--verbose` flag help text changed. Regenerate man pages:

```bash
mise run man:gen
```

If they changed, commit them:

```bash
git add man/
git commit -m "docs: update man pages for two-tier verbosity"
```

- [ ] **Step 4: Update shell completions if needed**

The `-v` flag behavior changed but the flag name didn't. Shell completions for
`--verbose` should still work. Verify:

```bash
mise run man:verify
```

- [ ] **Step 5: Update docs/cli/ pages for prune and sync**

Update `docs/cli/daft-prune.md` and `docs/cli/daft-sync.md` (if they exist) to
document the two-tier verbosity: `-v` for hook details, `-vv` for full
sequential output.

- [ ] **Step 6: Update SKILL.md**

Update the command table in `SKILL.md` to mention the new verbosity levels for
prune and sync.
