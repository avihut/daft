# Local-First Remote Sync Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make daft local-first by default — worktree management commands
(`start`, `checkout`, `remove`) no longer touch the remote unless the user opts
in via `daft config remote-sync`.

**Architecture:** Three git config toggles (`daft.checkout.fetch`,
`daft.checkout.push`, `daft.branchDelete.remote`) default to `false`. A new
`daft config remote-sync` TUI lets users toggle them. Per-invocation `--local`
and `--remote` flags override config. `daft doctor` warns when no explicit
config is set.

**Tech Stack:** Rust, clap (CLI), ratatui + crossterm (TUI), git config
(storage)

---

### Task 1: Add `checkout.fetch` setting

**Files:**

- Modify: `src/core/settings.rs`
- Test: `src/core/settings.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Add failing test for new default**

Add to the `test_default_settings` test in `src/core/settings.rs`:

```rust
// In test_default_settings():
assert!(!settings.checkout_fetch);
```

- [ ] **Step 2: Run test to verify it fails**

Run: `mise run test:unit -- --test test_default_settings` Expected: FAIL —
`checkout_fetch` field does not exist

- [ ] **Step 3: Add the setting**

In `src/core/settings.rs`, add the new setting in four locations:

In `defaults` module (after `CHECKOUT_PUSH` on line 90):

```rust
/// Default value for checkout.fetch setting.
pub const CHECKOUT_FETCH: bool = false;
```

In `keys` module (after `CHECKOUT_PUSH` on line 138):

```rust
/// Config key for checkout.fetch setting.
pub const CHECKOUT_FETCH: &str = "daft.checkout.fetch";
```

In `DaftSettings` struct (after `checkout_push` field on line 257):

```rust
/// Fetch from remote before creating worktrees.
pub checkout_fetch: bool,
```

In `Default` impl (after `checkout_push` on line 322):

```rust
checkout_fetch: defaults::CHECKOUT_FETCH,
```

In `load()` method (after the `CHECKOUT_PUSH` block around line 362):

```rust
if let Some(value) = git.config_get(keys::CHECKOUT_FETCH)? {
    settings.checkout_fetch = parse_bool(&value, defaults::CHECKOUT_FETCH);
}
```

In `load_global()` method (after the `CHECKOUT_PUSH` block around line 486):

```rust
if let Some(value) = git.config_get_global(keys::CHECKOUT_FETCH)? {
    settings.checkout_fetch = parse_bool(&value, defaults::CHECKOUT_FETCH);
}
```

Update the doc table at the top of the file to include:

```
//! | `daft.checkout.fetch` | `false` | Fetch from remote before creating worktrees |
```

- [ ] **Step 4: Run test to verify it passes**

Run: `mise run test:unit -- --test test_default_settings` Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/core/settings.rs
git commit -m "feat: add daft.checkout.fetch setting (default false)"
```

---

### Task 2: Flip `checkout.push` default to `false`

**Files:**

- Modify: `src/core/settings.rs`
- Test: `src/core/settings.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Update the failing test**

In `test_default_settings`, change:

```rust
// Old:
assert!(settings.checkout_push);
// New:
assert!(!settings.checkout_push);
```

- [ ] **Step 2: Run test to verify it fails**

Run: `mise run test:unit -- --test test_default_settings` Expected: FAIL —
`checkout_push` is still `true`

- [ ] **Step 3: Flip the default**

In `src/core/settings.rs` `defaults` module, change:

```rust
// Old:
pub const CHECKOUT_PUSH: bool = true;
// New:
pub const CHECKOUT_PUSH: bool = false;
```

Update the doc table at the top:

```
//! | `daft.checkout.push` | `false` | Push new branches to remote |
```

- [ ] **Step 4: Run test to verify it passes**

Run: `mise run test:unit -- --test test_default_settings` Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/core/settings.rs
git commit -m "feat!: flip daft.checkout.push default to false (local-first)"
```

---

### Task 3: Add `branchDelete.remote` setting

**Files:**

- Modify: `src/core/settings.rs`
- Test: `src/core/settings.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Add failing test**

In `test_default_settings`:

```rust
assert!(!settings.branch_delete_remote);
```

- [ ] **Step 2: Run test to verify it fails**

Run: `mise run test:unit -- --test test_default_settings` Expected: FAIL — field
does not exist

- [ ] **Step 3: Add the setting**

In `defaults` module:

```rust
/// Default value for branchDelete.remote setting.
pub const BRANCH_DELETE_REMOTE: bool = false;
```

In `keys` module:

```rust
/// Config key for branchDelete.remote setting.
pub const BRANCH_DELETE_REMOTE: &str = "daft.branchDelete.remote";
```

In `DaftSettings` struct (after `checkout_fetch`):

```rust
/// Delete remote branch when removing a branch/worktree.
pub branch_delete_remote: bool,
```

In `Default` impl:

```rust
branch_delete_remote: defaults::BRANCH_DELETE_REMOTE,
```

In `load()`:

```rust
if let Some(value) = git.config_get(keys::BRANCH_DELETE_REMOTE)? {
    settings.branch_delete_remote = parse_bool(&value, defaults::BRANCH_DELETE_REMOTE);
}
```

In `load_global()`:

```rust
if let Some(value) = git.config_get_global(keys::BRANCH_DELETE_REMOTE)? {
    settings.branch_delete_remote = parse_bool(&value, defaults::BRANCH_DELETE_REMOTE);
}
```

Update the doc table:

```
//! | `daft.branchDelete.remote` | `false` | Delete remote branch when removing |
```

- [ ] **Step 4: Run test to verify it passes**

Run: `mise run test:unit -- --test test_default_settings` Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/core/settings.rs
git commit -m "feat: add daft.branchDelete.remote setting (default false)"
```

---

### Task 4: Make checkout fetch conditional

**Files:**

- Modify: `src/core/worktree/checkout.rs`
- Modify: `src/commands/checkout.rs`

- [ ] **Step 1: Add `checkout_fetch` to `CheckoutParams`**

In `src/core/worktree/checkout.rs`, add to `CheckoutParams` struct (after
`checkout_upstream` on line 71):

```rust
/// Whether to fetch from remote before creating the worktree.
pub checkout_fetch: bool,
```

- [ ] **Step 2: Gate the fetch call**

In `src/core/worktree/checkout.rs`, replace line 207:

```rust
// Old:
let fetch_failed = !fetch_branch(git, &params.remote_name, &params.branch_name, sink);

// New:
let fetch_failed = if params.checkout_fetch {
    !fetch_branch(git, &params.remote_name, &params.branch_name, sink)
} else {
    false
};
```

- [ ] **Step 3: Wire `checkout_fetch` from settings into params**

In `src/commands/checkout.rs`, add to the `CheckoutParams` construction at line
695:

```rust
checkout_fetch: settings.checkout_fetch,
```

- [ ] **Step 4: Run tests**

Run: `mise run test:unit` Expected: PASS (all compilation and unit tests)

Run: `mise run clippy` Expected: PASS with zero warnings

- [ ] **Step 5: Commit**

```bash
git add src/core/worktree/checkout.rs src/commands/checkout.rs
git commit -m "feat: make checkout fetch conditional on daft.checkout.fetch"
```

---

### Task 5: Make checkout-branch fetch conditional

**Files:**

- Modify: `src/core/worktree/checkout_branch.rs`
- Modify: `src/commands/checkout.rs`

- [ ] **Step 1: Add `checkout_fetch` to `CheckoutBranchParams`**

In `src/core/worktree/checkout_branch.rs`, add to `CheckoutBranchParams` (after
`checkout_push` on line 38):

```rust
/// Whether to fetch from remote before creating the worktree.
pub checkout_fetch: bool,
```

- [ ] **Step 2: Gate the fetch call**

In `src/core/worktree/checkout_branch.rs`, replace line 107:

```rust
// Old:
fetch_remote(git, &params.remote_name, sink);

// New:
if params.checkout_fetch {
    fetch_remote(git, &params.remote_name, sink);
}
```

- [ ] **Step 3: Wire into params construction**

In `src/commands/checkout.rs` `run_create_branch()`, add to
`CheckoutBranchParams` construction at line 767:

```rust
checkout_fetch: settings.checkout_fetch,
```

- [ ] **Step 4: Run tests**

Run: `mise run test:unit && mise run clippy` Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/core/worktree/checkout_branch.rs src/commands/checkout.rs
git commit -m "feat: make checkout-branch fetch conditional on daft.checkout.fetch"
```

---

### Task 6: Make branch-delete remote deletion conditional

**Files:**

- Modify: `src/core/worktree/branch_delete.rs`
- Modify: `src/commands/worktree_branch.rs`
- Modify: `src/commands/branch_delete.rs`

- [ ] **Step 1: Add `delete_remote` to `BranchDeleteParams`**

In `src/core/worktree/branch_delete.rs`, add to `BranchDeleteParams` (after
`remote_name` on line 26):

```rust
/// Whether to delete the remote branch.
pub delete_remote: bool,
```

- [ ] **Step 2: Gate the remote deletion**

In `src/core/worktree/branch_delete.rs` `delete_single_branch()`, change the
condition at line 728:

```rust
// Old:
if !branch.worktree_only {

// New:
if !branch.worktree_only && params.delete_remote {
```

Note: `params` is accessible through `ctx`. Check how `params` is passed to
`delete_single_branch` — it receives a `BranchDeleteContext` which has
`params: &'a BranchDeleteParams`. So the condition becomes:

```rust
if !branch.worktree_only && ctx.params.delete_remote {
```

Verify that `BranchDeleteContext` has a `params` field. If the function receives
`params` directly, use `params.delete_remote`.

- [ ] **Step 3: Wire into params in `worktree_branch.rs`**

In `src/commands/worktree_branch.rs` `run_branch_delete()` at line 335:

```rust
let params = branch_delete::BranchDeleteParams {
    branches: branches.to_vec(),
    force,
    use_gitoxide: settings.use_gitoxide,
    is_quiet: quiet,
    remote_name: settings.remote.clone(),
    prune_cd_target: settings.prune_cd_target,
    delete_remote: settings.branch_delete_remote,
};
```

- [ ] **Step 4: Wire into params in `branch_delete.rs`**

In `src/commands/branch_delete.rs` `run_branch_delete()` at line 78:

```rust
let params = branch_delete::BranchDeleteParams {
    branches: args.branches.clone(),
    force: args.force,
    use_gitoxide: settings.use_gitoxide,
    is_quiet: args.quiet,
    remote_name: settings.remote.clone(),
    prune_cd_target: settings.prune_cd_target,
    delete_remote: settings.branch_delete_remote,
};
```

- [ ] **Step 5: Run tests**

Run: `mise run test:unit && mise run clippy` Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/core/worktree/branch_delete.rs src/commands/worktree_branch.rs src/commands/branch_delete.rs
git commit -m "feat: make branch-delete remote deletion conditional on daft.branchDelete.remote"
```

---

### Task 7: Add `--local` flag to `start`, `checkout`, and `go`

**Files:**

- Modify: `src/commands/checkout.rs`

- [ ] **Step 1: Add `--local` to `Args`**

In `src/commands/checkout.rs` `Args` struct, add after the `at` field (line
122):

```rust
#[arg(long, help = "Skip all remote operations (no fetch, no push)")]
local: bool,
```

- [ ] **Step 2: Add `--local` to `GoArgs`**

In `GoArgs` struct, add after the `at` field (line 219):

```rust
#[arg(long, help = "Skip all remote operations (no fetch, no push)")]
local: bool,
```

- [ ] **Step 3: Add `--local` to `StartArgs`**

In `StartArgs` struct, add after the `at` field (line 284):

```rust
#[arg(long, help = "Skip all remote operations (no fetch, no push)")]
local: bool,
```

- [ ] **Step 4: Wire through `run_go()`**

In `run_go()` (line 299), add `local` to the `Args` conversion:

```rust
let args = Args {
    // ... existing fields ...
    local: go_args.local,
};
```

- [ ] **Step 5: Wire through `run_start()`**

In `run_start()` (line 322), add `local` to the `Args` conversion:

```rust
let args = Args {
    // ... existing fields ...
    local: start_args.local,
};
```

- [ ] **Step 6: Apply override in `run_checkout()`**

In `run_checkout()`, modify the `CheckoutParams` construction (line 695).
Replace the `checkout_fetch` line to respect `--local`:

```rust
checkout_fetch: if args.local { false } else { settings.checkout_fetch },
```

- [ ] **Step 7: Apply override in `run_create_branch()`**

In `run_create_branch()`, modify the `CheckoutBranchParams` construction (line
767). Override both fetch and push:

```rust
checkout_push: if args.local { false } else { settings.checkout_push },
checkout_fetch: if args.local { false } else { settings.checkout_fetch },
```

- [ ] **Step 8: Run tests**

Run: `mise run test:unit && mise run clippy` Expected: PASS

- [ ] **Step 9: Commit**

```bash
git add src/commands/checkout.rs
git commit -m "feat: add --local flag to start, checkout, and go commands"
```

---

### Task 8: Add `--local` and `--remote` flags to `remove` and `branch-delete`

**Files:**

- Modify: `src/commands/worktree_branch.rs`
- Modify: `src/commands/branch_delete.rs`
- Modify: `src/core/worktree/branch_delete.rs`

- [ ] **Step 1: Add `remote_only` to `BranchDeleteParams`**

In `src/core/worktree/branch_delete.rs` `BranchDeleteParams`, add after
`delete_remote`:

```rust
/// Only delete the remote branch, keep local worktree and branch.
pub remote_only: bool,
```

- [ ] **Step 2: Implement `remote_only` logic in `delete_single_branch`**

In `delete_single_branch()`, after the remote deletion block (around line 751),
gate the worktree removal (Step 3) and branch deletion (Step 4) with:

```rust
if ctx.params.remote_only {
    // Verify a remote tracking branch exists
    if branch.remote_name.is_none() || branch.remote_branch_name.is_none() {
        result.errors.push(format!(
            "Branch '{}' has no remote tracking branch",
            branch.name
        ));
    }
    return result;
}
```

Also, when `remote_only` is true, force `delete_remote` to true for the remote
deletion block. Adjust the remote deletion condition:

```rust
if !branch.worktree_only && (ctx.params.delete_remote || ctx.params.remote_only) {
```

- [ ] **Step 3: Add flags to `RemoveArgs`**

In `src/commands/worktree_branch.rs` `RemoveArgs`, add:

```rust
#[arg(long, help = "Only delete locally, keep remote branch")]
local: bool,

#[arg(long, conflicts_with = "local", help = "Only delete the remote branch, keep local worktree and branch")]
remote: bool,
```

- [ ] **Step 4: Add flags to `worktree_branch::Args`**

In `src/commands/worktree_branch.rs` `Args`, add (alongside the existing delete
flags):

```rust
#[arg(long, help = "Only delete locally, keep remote branch")]
local: bool,

#[arg(long, conflicts_with = "local", help = "Only delete the remote branch, keep local worktree and branch")]
remote: bool,
```

- [ ] **Step 5: Add flags to `branch_delete::Args`**

In `src/commands/branch_delete.rs` `Args`, add:

```rust
#[arg(long, help = "Only delete locally, keep remote branch")]
local: bool,

#[arg(long, conflicts_with = "local", help = "Only delete the remote branch, keep local worktree and branch")]
remote: bool,
```

- [ ] **Step 6: Wire flags through `worktree_branch::run_remove()`**

In `run_remove()`, pass the flags to `run_branch_delete()`. The function
signature needs updating to accept `local` and `remote`:

```rust
run_branch_delete(
    &remove_args.branches,
    remove_args.force,
    remove_args.quiet,
    remove_args.local,
    remove_args.remote,
    &mut output,
    &settings,
)
```

- [ ] **Step 7: Update `run_branch_delete()` signature and params**

```rust
fn run_branch_delete(
    branches: &[String],
    force: bool,
    quiet: bool,
    local_only: bool,
    remote_only: bool,
    output: &mut dyn Output,
    settings: &DaftSettings,
) -> Result<()> {
    let params = branch_delete::BranchDeleteParams {
        branches: branches.to_vec(),
        force,
        use_gitoxide: settings.use_gitoxide,
        is_quiet: quiet,
        remote_name: settings.remote.clone(),
        prune_cd_target: settings.prune_cd_target,
        delete_remote: if local_only { false } else if remote_only { true } else { settings.branch_delete_remote },
        remote_only,
    };
```

- [ ] **Step 8: Wire flags through `worktree_branch::run_with_args()`**

In the delete branch of `run_with_args()`, pass the flags:

```rust
run_branch_delete(
    &args.branches,
    args.force_delete,
    args.quiet,
    args.local,
    args.remote,
    &mut output,
    &settings,
)?;
```

- [ ] **Step 9: Wire flags through `branch_delete.rs::run_branch_delete()`**

```rust
let params = branch_delete::BranchDeleteParams {
    branches: args.branches.clone(),
    force: args.force,
    use_gitoxide: settings.use_gitoxide,
    is_quiet: args.quiet,
    remote_name: settings.remote.clone(),
    prune_cd_target: settings.prune_cd_target,
    delete_remote: if args.local { false } else if args.remote { true } else { settings.branch_delete_remote },
    remote_only: args.remote,
};
```

- [ ] **Step 10: Run tests**

Run: `mise run test:unit && mise run clippy` Expected: PASS

- [ ] **Step 11: Commit**

```bash
git add src/core/worktree/branch_delete.rs src/commands/worktree_branch.rs src/commands/branch_delete.rs
git commit -m "feat: add --local and --remote flags to remove and branch-delete"
```

---

### Task 9: Create `daft config` command with `remote-sync` TUI

**Files:**

- Create: `src/commands/config.rs`
- Modify: `src/commands/mod.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Create `src/commands/config.rs` with `remote-sync` subcommand
      routing**

```rust
use anyhow::Result;
use clap::Parser;

mod remote_sync;

#[derive(Parser)]
#[command(name = "daft config")]
#[command(version = crate::VERSION)]
#[command(about = "Manage daft configuration")]
pub struct Args {
    #[command(subcommand)]
    command: ConfigCommand,
}

#[derive(clap::Subcommand)]
enum ConfigCommand {
    /// Configure remote sync behavior
    RemoteSync(remote_sync::Args),
}

pub fn run() -> Result<()> {
    let raw = crate::get_clap_args("daft-config");
    // Skip "config" from args since it's already consumed by main.rs routing
    let args: Vec<String> = std::env::args().collect();
    let sub_args: Vec<String> = if args.len() > 2 {
        args[2..].to_vec()
    } else {
        vec![]
    };

    if sub_args.is_empty() || sub_args[0] == "--help" || sub_args[0] == "-h" {
        // Show config help
        use clap::CommandFactory;
        Args::command().print_help()?;
        println!();
        return Ok(());
    }

    match sub_args[0].as_str() {
        "remote-sync" => remote_sync::run(&sub_args[1..]),
        _ => {
            anyhow::bail!(
                "Unknown config subcommand: '{}'\n\nUsage: daft config remote-sync",
                sub_args[0]
            );
        }
    }
}
```

Note: The routing approach should match the existing pattern in `main.rs` —
manual dispatch, not clap subcommand parsing. Adjust the implementation based on
how other subcommand modules (like `setup`/`shortcuts`) actually work. The key
point is that `daft config remote-sync` routes to `remote_sync::run()`.

- [ ] **Step 2: Register the module**

In `src/commands/mod.rs`, add:

```rust
pub mod config;
```

- [ ] **Step 3: Add routing in `main.rs`**

In `src/main.rs`, in the `match args[1].as_str()` block under the
`"git-daft" | "daft"` arm (around line 100), add before the `"setup"` entry:

```rust
"config" => commands::config::run(),
```

- [ ] **Step 4: Run to verify routing works**

Run: `cargo build && ./target/debug/daft config --help` Expected: Shows config
help with `remote-sync` subcommand listed

- [ ] **Step 5: Commit**

```bash
git add src/commands/config.rs src/commands/mod.rs src/main.rs
git commit -m "feat: add daft config command skeleton with remote-sync routing"
```

---

### Task 10: Implement `remote-sync` TUI

**Files:**

- Create: `src/commands/config/remote_sync.rs`

This task implements the ratatui inline TUI for `daft config remote-sync`.

- [ ] **Step 1: Create the module with Args and non-interactive shortcuts**

Create `src/commands/config/remote_sync.rs`:

```rust
use anyhow::Result;
use clap::Parser;

use crate::git::GitCommand;
use crate::settings::{defaults, keys};

#[derive(Parser)]
#[command(name = "daft config remote-sync")]
#[command(about = "Configure remote sync behavior")]
pub struct Args {
    #[arg(long, help = "Enable all remote sync operations")]
    on: bool,

    #[arg(long, conflicts_with = "on", help = "Disable all remote sync operations")]
    off: bool,

    #[arg(long, help = "Show current remote sync settings")]
    status: bool,

    #[arg(long, help = "Write to global git config instead of local")]
    global: bool,
}

/// Current state of the three remote-sync toggles.
struct SyncState {
    fetch: bool,
    push: bool,
    delete_remote: bool,
}

impl SyncState {
    fn load(global: bool) -> Result<Self> {
        let git = GitCommand::new(true);
        let get = |key: &str| -> Result<Option<String>> {
            if global {
                git.config_get_global(key)
            } else {
                git.config_get(key)
            }
        };

        Ok(Self {
            fetch: get(keys::CHECKOUT_FETCH)?
                .map(|v| crate::settings::parse_bool(&v, defaults::CHECKOUT_FETCH))
                .unwrap_or(defaults::CHECKOUT_FETCH),
            push: get(keys::CHECKOUT_PUSH)?
                .map(|v| crate::settings::parse_bool(&v, defaults::CHECKOUT_PUSH))
                .unwrap_or(defaults::CHECKOUT_PUSH),
            delete_remote: get(keys::BRANCH_DELETE_REMOTE)?
                .map(|v| crate::settings::parse_bool(&v, defaults::BRANCH_DELETE_REMOTE))
                .unwrap_or(defaults::BRANCH_DELETE_REMOTE),
        })
    }

    fn all_on(&self) -> bool {
        self.fetch && self.push && self.delete_remote
    }

    fn all_off(&self) -> bool {
        !self.fetch && !self.push && !self.delete_remote
    }

    fn save(&self, global: bool) -> Result<()> {
        let git = GitCommand::new(true);
        let set = |key: &str, value: bool| -> Result<()> {
            if global {
                git.config_set_global(key, &value.to_string())
            } else {
                git.config_set(key, &value.to_string())
            }
        };
        set(keys::CHECKOUT_FETCH, self.fetch)?;
        set(keys::CHECKOUT_PUSH, self.push)?;
        set(keys::BRANCH_DELETE_REMOTE, self.delete_remote)?;
        Ok(())
    }
}

pub fn run(args: &[String]) -> Result<()> {
    let parsed = Args::parse_from(
        std::iter::once("daft config remote-sync".to_string()).chain(args.iter().cloned()),
    );

    if parsed.status {
        return show_status(parsed.global);
    }

    if parsed.on {
        let state = SyncState {
            fetch: true,
            push: true,
            delete_remote: true,
        };
        state.save(parsed.global)?;
        println!("✓ Remote sync enabled (fetch, push, delete-remote)");
        return Ok(());
    }

    if parsed.off {
        let state = SyncState {
            fetch: false,
            push: false,
            delete_remote: false,
        };
        state.save(parsed.global)?;
        println!("✓ Remote sync disabled (local-only mode)");
        return Ok(());
    }

    run_tui(parsed.global)
}

fn show_status(global: bool) -> Result<()> {
    let state = SyncState::load(global)?;
    let scope = if global { "global" } else { "local" };
    println!("Remote sync settings ({scope} config):");
    println!(
        "  Fetch before checkout     {}",
        if state.fetch { "on" } else { "off" }
    );
    println!(
        "  Push new branches         {}",
        if state.push { "on" } else { "off" }
    );
    println!(
        "  Delete remote branches    {}",
        if state.delete_remote { "on" } else { "off" }
    );
    Ok(())
}
```

Note: `parse_bool` in settings.rs is currently private. Task 10 Step 3 makes it
`pub(crate)` so this module can use it.

Also, `GitCommand` may not have `config_set` / `config_set_global` methods yet.
Check and add them if needed (they should call
`git config [--global] <key> <value>`).

- [ ] **Step 2: Implement the TUI**

Add the `run_tui` function to `remote_sync.rs`. This uses ratatui's inline
viewport (same pattern as sync/prune TUI):

```rust
fn run_tui(global: bool) -> Result<()> {
    use crossterm::{
        event::{self, Event, KeyCode, KeyEventKind},
        terminal::{disable_raw_mode, enable_raw_mode},
    };
    use ratatui::{
        backend::CrosstermBackend,
        layout::{Constraint, Layout},
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::Paragraph,
        Terminal,
    };

    let mut state = SyncState::load(global)?;

    // Menu items: 0=Full sync, 1=Local only, 2=Custom, 3=Fetch, 4=Push, 5=Delete
    let mut cursor: usize = if state.all_on() {
        0
    } else if state.all_off() {
        1
    } else {
        2
    };
    let mut custom_expanded = !state.all_on() && !state.all_off();

    enable_raw_mode()?;
    let mut terminal = Terminal::with_options(
        CrosstermBackend::new(std::io::stderr()),
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Inline(10),
        },
    )?;

    loop {
        terminal.draw(|frame| {
            let scope_label = if global { "global config" } else { "local config" };
            let radio = |selected: bool| if selected { "●" } else { "○" };
            let check = |on: bool| if on { "[✓]" } else { "[ ]" };

            let is_full = state.all_on();
            let is_local = state.all_off();
            let is_custom = !is_full && !is_local;

            let highlight = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
            let normal = Style::default();
            let dim = Style::default().fg(Color::DarkGray);

            let mut lines = vec![
                Line::from(""),
                Line::from(vec![
                    Span::styled(" Remote Sync", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw("  "),
                    Span::styled(scope_label, dim),
                ]),
                Line::from(Span::styled(
                    " ─────────────────────────────────────────────────",
                    dim,
                )),
            ];

            let items = [
                (0, format!(" {} Full sync", radio(is_full))),
                (1, format!(" {} Local only", radio(is_local))),
                (2, format!(" {} Custom", radio(is_custom))),
            ];

            for (idx, text) in &items {
                let prefix = if cursor == *idx { " ›" } else { "  " };
                let style = if cursor == *idx { highlight } else { normal };
                lines.push(Line::from(Span::styled(format!("{prefix}{text}"), style)));
            }

            if custom_expanded || is_custom {
                let sub_items = [
                    (3, format!("     ├ {} Fetch before checkout", check(state.fetch))),
                    (4, format!("     ├ {} Push new branches", check(state.push))),
                    (5, format!("     └ {} Delete remote branches", check(state.delete_remote))),
                ];
                for (idx, text) in &sub_items {
                    let style = if cursor == *idx { highlight } else { normal };
                    let prefix = if cursor == *idx { " ›" } else { "  " };
                    lines.push(Line::from(Span::styled(
                        format!("{prefix}{text}"),
                        style,
                    )));
                }
            }

            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                " ↑↓ navigate  space toggle  enter confirm  q cancel",
                dim,
            )));

            let paragraph = Paragraph::new(lines);
            frame.render_widget(paragraph, frame.area());
        })?;

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let max_item = if custom_expanded || (!state.all_on() && !state.all_off()) {
                5
            } else {
                2
            };
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if cursor > 0 {
                        cursor -= 1;
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if cursor < max_item {
                        cursor += 1;
                    }
                }
                KeyCode::Char(' ') => match cursor {
                    0 => {
                        state.fetch = true;
                        state.push = true;
                        state.delete_remote = true;
                        custom_expanded = false;
                    }
                    1 => {
                        state.fetch = false;
                        state.push = false;
                        state.delete_remote = false;
                        custom_expanded = false;
                    }
                    2 => {
                        custom_expanded = !custom_expanded;
                    }
                    3 => {
                        state.fetch = !state.fetch;
                    }
                    4 => {
                        state.push = !state.push;
                    }
                    5 => {
                        state.delete_remote = !state.delete_remote;
                    }
                    _ => {}
                },
                KeyCode::Enter => {
                    // Apply the same action as space for the current item, then save
                    match cursor {
                        0 => {
                            state.fetch = true;
                            state.push = true;
                            state.delete_remote = true;
                        }
                        1 => {
                            state.fetch = false;
                            state.push = false;
                            state.delete_remote = false;
                        }
                        _ => {}
                    }
                    break;
                }
                KeyCode::Char('q') | KeyCode::Esc => {
                    disable_raw_mode()?;
                    terminal.clear()?;
                    println!("Cancelled");
                    return Ok(());
                }
                _ => {}
            }
        }
    }

    disable_raw_mode()?;
    terminal.clear()?;

    state.save(global)?;

    if state.all_on() {
        println!("✓ Remote sync enabled (full sync)");
    } else if state.all_off() {
        println!("✓ Remote sync disabled (local only)");
    } else {
        println!("✓ Remote sync configured (custom)");
    }
    Ok(())
}
```

This is a starting point — the exact rendering will need refinement based on the
existing TUI patterns in `src/output/tui/`. The key behaviors are:

- Radio group (Full sync / Local only / Custom) with checkboxes
- Arrow key navigation, space to toggle, enter to confirm, q to cancel
- Inline viewport (10 lines)

- [ ] **Step 3: Ensure `parse_bool` is accessible**

In `src/core/settings.rs`, change:

```rust
// Old:
fn parse_bool(value: &str, default: bool) -> bool {
// New:
pub(crate) fn parse_bool(value: &str, default: bool) -> bool {
```

- [ ] **Step 4: Ensure `config_set` / `config_set_global` exist on
      `GitCommand`**

Check `src/git/mod.rs` for these methods. If they don't exist, add them:

```rust
pub fn config_set(&self, key: &str, value: &str) -> Result<()> {
    self.run_git(&["config", key, value])?;
    Ok(())
}

pub fn config_set_global(&self, key: &str, value: &str) -> Result<()> {
    self.run_git(&["config", "--global", key, value])?;
    Ok(())
}
```

- [ ] **Step 5: Run tests**

Run: `mise run test:unit && mise run clippy` Expected: PASS

- [ ] **Step 6: Manual test**

Run: `cargo build && ./target/debug/daft config remote-sync --status` Expected:
Shows current settings

Run: `cargo build && ./target/debug/daft config remote-sync --on` Expected:
Enables all, prints confirmation

Run: `cargo build && ./target/debug/daft config remote-sync --off` Expected:
Disables all, prints confirmation

Run: `cargo build && ./target/debug/daft config remote-sync` Expected: TUI
appears, navigation and toggling works

- [ ] **Step 7: Commit**

```bash
git add src/commands/config/ src/core/settings.rs src/git/mod.rs
git commit -m "feat: implement daft config remote-sync TUI"
```

---

### Task 11: Add `daft doctor` migration check

**Files:**

- Modify: `src/doctor/repository.rs`
- Modify: `src/commands/doctor.rs`

- [ ] **Step 1: Add the check function**

In `src/doctor/repository.rs`, add:

```rust
/// Check if remote-sync settings have been explicitly configured.
///
/// Shows a one-time informational note when none of the three remote-sync
/// keys are set, so users know the defaults have changed.
pub fn check_remote_sync_config(ctx: &RepoContext) -> CheckResult {
    use crate::git::GitCommand;
    use crate::settings::keys;

    let git = GitCommand::new(true);

    let has_fetch = git.config_get(keys::CHECKOUT_FETCH).ok().flatten().is_some()
        || git.config_get_global(keys::CHECKOUT_FETCH).ok().flatten().is_some();
    let has_push = git.config_get(keys::CHECKOUT_PUSH).ok().flatten().is_some()
        || git.config_get_global(keys::CHECKOUT_PUSH).ok().flatten().is_some();
    let has_delete = git.config_get(keys::BRANCH_DELETE_REMOTE).ok().flatten().is_some()
        || git.config_get_global(keys::BRANCH_DELETE_REMOTE).ok().flatten().is_some();

    if has_fetch || has_push || has_delete {
        CheckResult::pass(
            "Remote sync".to_string(),
            "Remote sync settings are configured".to_string(),
        )
    } else {
        CheckResult::warning(
            "Remote sync".to_string(),
            "Remote sync defaults have changed — daft no longer fetches, pushes, or deletes remote branches by default".to_string(),
        )
        .with_suggestion("Run `daft config remote-sync` to configure your preference.".to_string())
    }
}
```

- [ ] **Step 2: Register the check**

In `src/commands/doctor.rs` `run_repository_checks()`, add:

```rust
repository::check_remote_sync_config(ctx),
```

- [ ] **Step 3: Run tests**

Run: `mise run test:unit && mise run clippy` Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/doctor/repository.rs src/commands/doctor.rs
git commit -m "feat: add daft doctor check for remote-sync configuration"
```

---

### Task 12: Register `config` in help, completions, and xtask

**Files:**

- Modify: `src/commands/docs.rs`
- Modify: `src/commands/completions/mod.rs`
- Modify: `src/commands/completions/bash.rs`
- Modify: `src/commands/completions/zsh.rs`
- Modify: `src/commands/completions/fish.rs`
- Modify: `xtask/src/main.rs`
- Modify: `src/suggest.rs` (DAFT_SUBCOMMANDS)

- [ ] **Step 1: Add to help output**

In `src/commands/docs.rs`, add the import for `config` at line 10 and add an
entry in the "manage daft configuration" category (around line 97):

```rust
CommandEntry {
    display_name: "daft config",
    command: config::Args::command(),
},
```

- [ ] **Step 2: Add to `DAFT_SUBCOMMANDS`**

Find `DAFT_SUBCOMMANDS` in `src/suggest.rs` and add `"config"` to the array.

- [ ] **Step 3: Add to completions**

In `src/commands/completions/mod.rs`, add `"config"` to the relevant completion
lists if needed. In `bash.rs`, `zsh.rs`, and `fish.rs`, add `config` to the
top-level subcommand lists in the hardcoded completion strings.

- [ ] **Step 4: Add to xtask for man pages (if applicable)**

In `xtask/src/main.rs`, add `"daft-config"` to `COMMANDS` array and
`get_command_for_name()` if the config command should have a man page. If
subcommands have their own man pages, add those too.

- [ ] **Step 5: Generate man pages**

Run: `mise run man:gen`

- [ ] **Step 6: Run verification**

Run: `mise run clippy && mise run fmt:check && mise run man:verify` Expected:
PASS

- [ ] **Step 7: Commit**

```bash
git add src/commands/docs.rs src/suggest.rs src/commands/completions/ xtask/ man/
git commit -m "feat: register daft config in help, completions, and man pages"
```

---

### Task 13: Add YAML test scenarios for local-first behavior

**Files:**

- Create: `tests/manual/scenarios/checkout/local-first-checkout.yml`
- Create: `tests/manual/scenarios/checkout/local-first-start.yml`
- Create: `tests/manual/scenarios/branch-delete/local-first-remove.yml`

- [ ] **Step 1: Create checkout local-first scenario**

Create `tests/manual/scenarios/checkout/local-first-checkout.yml`:

```yaml
name: Checkout local-first (no fetch)
description: With default settings, checkout should not fetch from remote

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Checkout develop branch (local-first default, no fetch)
    run: git-worktree-checkout develop
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0
      dirs_exist:
        - "$WORK_DIR/test-repo/develop"
      is_git_worktree:
        - dir: "$WORK_DIR/test-repo/develop"
          branch: develop
```

- [ ] **Step 2: Create start local-first scenario**

Create `tests/manual/scenarios/checkout/local-first-start.yml`:

```yaml
name: Start local-first (no push)
description: With default settings, start should not push new branch to remote

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Start a new branch (local-first default, no push)
    run: daft-start my-feature
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0
      dirs_exist:
        - "$WORK_DIR/test-repo/my-feature"
      is_git_worktree:
        - dir: "$WORK_DIR/test-repo/my-feature"
          branch: my-feature

  - name: Verify branch was NOT pushed to remote
    run: git ls-remote --heads $REMOTE_TEST_REPO my-feature
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      stdout_is: ""
```

- [ ] **Step 3: Create remove local-first scenario**

Create `tests/manual/scenarios/branch-delete/local-first-remove.yml`:

```yaml
name: Remove local-first (no remote delete)
description: With default settings, remove should not delete remote branch

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Checkout develop branch
    run: git-worktree-checkout develop
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0

  - name: Remove develop worktree (local-first default, keep remote)
    run: daft-remove -f develop
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      dirs_not_exist:
        - "$WORK_DIR/test-repo/develop"

  - name: Verify remote branch still exists
    run: git ls-remote --heads $REMOTE_TEST_REPO develop
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      stdout_contains: "refs/heads/develop"
```

- [ ] **Step 4: Run the scenarios**

Run: `mise run test:manual -- --ci checkout:local-first-checkout` Run:
`mise run test:manual -- --ci checkout:local-first-start` Run:
`mise run test:manual -- --ci branch-delete:local-first-remove` Expected: All
PASS

- [ ] **Step 5: Commit**

```bash
git add tests/manual/scenarios/
git commit -m "test: add YAML scenarios for local-first default behavior"
```

---

### Task 14: Add YAML test scenarios for `--local` and `--remote` flags

**Files:**

- Create: `tests/manual/scenarios/checkout/local-flag.yml`
- Create: `tests/manual/scenarios/branch-delete/local-flag.yml`
- Create: `tests/manual/scenarios/branch-delete/remote-flag.yml`

- [ ] **Step 1: Create `--local` flag scenario for start**

Create `tests/manual/scenarios/checkout/local-flag.yml`:

```yaml
name: Start with --local flag overrides config
description:
  The --local flag skips fetch and push even when remote sync is enabled

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Enable remote sync
    run: git config daft.checkout.push true
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Start with --local flag (should NOT push despite config)
    run: daft-start --local my-local-feature
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0
      dirs_exist:
        - "$WORK_DIR/test-repo/my-local-feature"

  - name: Verify branch was NOT pushed
    run: git ls-remote --heads $REMOTE_TEST_REPO my-local-feature
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      stdout_is: ""
```

- [ ] **Step 2: Create `--local` flag scenario for remove**

Create `tests/manual/scenarios/branch-delete/local-flag.yml`:

```yaml
name: Remove with --local flag keeps remote
description:
  The --local flag skips remote branch deletion even when remote sync is enabled

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Enable remote sync
    run: git config daft.branchDelete.remote true
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Checkout develop
    run: git-worktree-checkout develop
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0

  - name: Remove with --local flag (should NOT delete remote)
    run: daft-remove -f --local develop
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      dirs_not_exist:
        - "$WORK_DIR/test-repo/develop"

  - name: Verify remote branch still exists
    run: git ls-remote --heads $REMOTE_TEST_REPO develop
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      stdout_contains: "refs/heads/develop"
```

- [ ] **Step 3: Create `--remote` flag scenario for remove**

Create `tests/manual/scenarios/branch-delete/remote-flag.yml`:

```yaml
name: Remove with --remote flag deletes only remote
description:
  The --remote flag only deletes the remote branch, keeping local worktree

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Checkout develop
    run: git-worktree-checkout develop
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0

  - name: Remove with --remote flag (should ONLY delete remote)
    run: daft-remove --remote develop
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      dirs_exist:
        - "$WORK_DIR/test-repo/develop"

  - name: Verify remote branch was deleted
    run: git ls-remote --heads $REMOTE_TEST_REPO develop
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      stdout_is: ""
```

- [ ] **Step 4: Run the scenarios**

Run: `mise run test:manual -- --ci checkout:local-flag` Run:
`mise run test:manual -- --ci branch-delete:local-flag` Run:
`mise run test:manual -- --ci branch-delete:remote-flag` Expected: All PASS

- [ ] **Step 5: Commit**

```bash
git add tests/manual/scenarios/
git commit -m "test: add YAML scenarios for --local and --remote flag overrides"
```

---

### Task 15: Update documentation

**Files:**

- Modify: `docs/guide/configuration.md` (or equivalent config docs page)
- Modify: `docs/cli/` (command reference pages for affected commands)
- Modify: `SKILL.md` (if it exists — per CLAUDE.md instructions)

- [ ] **Step 1: Update configuration guide**

Add a "Remote Sync" section to the configuration guide documenting the three
settings, defaults, and `daft config remote-sync` command.

- [ ] **Step 2: Update CLI reference for affected commands**

Update the command reference pages to document:

- `start`/`checkout`/`go`: new `--local` flag
- `remove`/`branch-delete`: new `--local` and `--remote` flags
- `daft config remote-sync`: new command page

- [ ] **Step 3: Update SKILL.md if it exists**

If `SKILL.md` exists in the repo root, update it to reflect the new local-first
defaults and the config command.

- [ ] **Step 4: Run docs checks**

Run: `mise run docs:site:build` Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add docs/ SKILL.md
git commit -m "docs: document local-first remote sync settings and config command"
```

---

### Task 16: Final verification

- [ ] **Step 1: Run full CI locally**

Run: `mise run ci` Expected: All checks pass (fmt, clippy, unit tests,
integration tests, man page verification)

- [ ] **Step 2: Run full integration test suite**

Run: `mise run test:integration` Expected: PASS — existing tests should still
work since clone-based tests set up their own config

- [ ] **Step 3: Run all manual test scenarios**

Run: `mise run test:manual -- --ci` Expected: All scenarios pass including new
local-first scenarios

- [ ] **Step 4: Final commit if any fixes needed**

```bash
git add -A
git commit -m "fix: address CI feedback for local-first remote sync"
```
