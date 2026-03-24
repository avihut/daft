# Command Integration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the layout system (from Plan 1) into all existing commands so
daft works with any layout — contained, sibling, nested, centralized, or custom.

**Architecture:** Replace the hard-coded `calculate_worktree_path()` with
layout-aware path computation. Update clone to support both bare and non-bare
repos. Add `--layout` flag to clone, `--at` flag to start/go, and new `layout`
subcommands. Make adopt/eject aliases for `layout transform`.

**Tech Stack:** Rust, clap (existing), existing test infrastructure

**Spec:**
`docs/superpowers/specs/2026-03-20-progressive-adoption-layout-system-design.md`

**Depends on:** Plan 1 (layout foundation) — all complete.

---

## File Structure

### New files

| File                     | Responsibility                                               |
| ------------------------ | ------------------------------------------------------------ |
| `src/commands/layout.rs` | `layout list`, `layout show`, `layout transform` subcommands |

### Modified files

| File                                   | Change                                                                           |
| -------------------------------------- | -------------------------------------------------------------------------------- |
| `src/core/multi_remote/path.rs`        | Add layout-aware path computation alongside existing `calculate_worktree_path()` |
| `src/commands/checkout.rs`             | Add `--at` flag to Args/GoArgs/StartArgs, resolve layout, pass to core           |
| `src/core/worktree/checkout.rs`        | Accept layout in `CheckoutParams`, use layout path computation                   |
| `src/core/worktree/checkout_branch.rs` | Accept layout in params, use layout path computation                             |
| `src/commands/clone.rs`                | Add `--layout` flag, support non-bare clone path                                 |
| `src/core/worktree/clone.rs`           | Add layout to `CloneParams`, branch on bare/non-bare                             |
| `src/commands/list.rs`                 | Add off-template indicator and sandbox marker columns                            |
| `src/commands/prune.rs`                | Skip detached HEAD sandboxes                                                     |
| `src/commands/flow_adopt.rs`           | Rewrite as alias for `layout transform contained`                                |
| `src/commands/flow_eject.rs`           | Rewrite as alias for `layout transform sibling`                                  |
| `src/main.rs`                          | Add `layout` subcommand routing                                                  |
| `src/commands/mod.rs`                  | Add `pub mod layout;`                                                            |
| `src/lib.rs`                           | Add verb alias for layout if needed                                              |

---

## Task 1: Layout-Aware Worktree Path Computation

**Files:**

- Modify: `src/core/multi_remote/path.rs`

### Description

Add a new function `calculate_worktree_path_from_layout()` that computes
worktree paths using the layout template system. The existing
`calculate_worktree_path()` must remain for backward compatibility during the
transition — it will be deprecated once all callers are migrated.

### Steps

- [ ] **Step 1: Read current `src/core/multi_remote/path.rs`**

Understand the existing `calculate_worktree_path()` signature and all callers.

- [ ] **Step 2: Write tests for layout-aware path computation**

```rust
#[cfg(test)]
mod layout_tests {
    use super::*;
    use crate::core::layout::{BuiltinLayout, Layout, TemplateContext};

    #[test]
    fn test_contained_layout_path() {
        let layout = BuiltinLayout::Contained.to_layout();
        let ctx = TemplateContext {
            repo_path: PathBuf::from("/home/user/myproject"),
            repo: "myproject".into(),
            branch: "feature/auth".into(),
        };
        let path = layout.worktree_path(&ctx).unwrap();
        assert_eq!(path, PathBuf::from("/home/user/myproject/feature-auth"));
    }

    #[test]
    fn test_sibling_layout_path() {
        let layout = BuiltinLayout::Sibling.to_layout();
        let ctx = TemplateContext {
            repo_path: PathBuf::from("/home/user/myproject"),
            repo: "myproject".into(),
            branch: "feature/auth".into(),
        };
        let path = layout.worktree_path(&ctx).unwrap();
        assert_eq!(path, PathBuf::from("/home/user/myproject.feature-auth"));
    }
}
```

Note: `Layout::worktree_path()` already exists from Plan 1. This task validates
that it integrates correctly with the path computation layer and adds a helper
function that builds the `TemplateContext` from repo state.

- [ ] **Step 3: Add helper function to build TemplateContext**

```rust
use crate::core::layout::TemplateContext;

/// Build a TemplateContext from repository information.
pub fn build_template_context(
    repo_path: &Path,
    branch_name: &str,
) -> TemplateContext {
    let repo = repo_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();
    TemplateContext {
        repo_path: repo_path.to_path_buf(),
        repo,
        branch: branch_name.to_string(),
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p daft --lib multi_remote -- --nocapture`

- [ ] **Step 5: Run clippy and fmt, commit**

```bash
mise run fmt && mise run clippy
git add src/core/multi_remote/path.rs
git commit -m "feat(layout): add layout-aware worktree path computation helper"
```

---

## Task 2: Update Clone for Layout Support

**Files:**

- Modify: `src/commands/clone.rs` — add `--layout` flag
- Modify: `src/core/worktree/clone.rs` — support non-bare clone path

### Description

The clone command currently hardcodes `git clone --bare`. With layout support:

- Resolve the layout from: `--layout` flag > global config default > `sibling`
- If layout `needs_bare()` → bare clone + create first worktree (existing path)
- If layout does NOT need bare → regular `git clone` + store layout in
  repos.json
- Store the chosen layout in `repos.json` via `TrustDatabase::set_layout()`

### Steps

- [ ] **Step 1: Read `src/commands/clone.rs` and `src/core/worktree/clone.rs`**

- [ ] **Step 2: Add `--layout` flag to clone Args**

In `src/commands/clone.rs`, add to the Args struct:

```rust
    /// Worktree layout to use for this repository.
    ///
    /// Accepts a named layout (contained, sibling, nested, centralized)
    /// or a custom template string.
    #[arg(long, value_name = "LAYOUT")]
    pub layout: Option<String>,
```

- [ ] **Step 3: Add layout to CloneParams**

In `src/core/worktree/clone.rs`, add to `CloneParams`:

```rust
    /// Resolved layout for this clone.
    pub layout: Layout,
```

- [ ] **Step 4: Update clone command to resolve layout**

In `src/commands/clone.rs`, before creating `CloneParams`:

```rust
use daft::core::global_config::GlobalConfig;
use daft::core::layout::resolver::{resolve_layout, LayoutResolutionContext, LayoutSource};

let global_config = GlobalConfig::load().unwrap_or_default();
let (layout, _source) = resolve_layout(&LayoutResolutionContext {
    cli_layout: args.layout.as_deref(),
    repo_store_layout: None, // New clone, no repo store entry yet
    yaml_layout: None,       // Can't read daft.yml before clone
    global_config: &global_config,
});
```

- [ ] **Step 5: Implement non-bare clone path in core**

In `src/core/worktree/clone.rs`, modify `execute()`:

```rust
if params.layout.needs_bare() {
    // Existing bare clone path
    git.clone_bare(&params.repository_url, &git_dir, ...)?;
    // ... create worktree as today
} else {
    // New: regular clone
    git.clone_regular(&params.repository_url, &parent_dir, ...)?;
    // No worktree creation needed — the clone IS the working tree
}
```

This requires adding `clone_regular()` to the git command wrapper (or using the
existing clone mechanism).

- [ ] **Step 6: Store layout in repos.json after clone**

After successful clone, save the layout:

```rust
let mut trust_db = TrustDatabase::load()?;
trust_db.set_layout(&git_dir, layout.name.clone());
trust_db.save()?;
```

- [ ] **Step 7: Write integration test**

Add a YAML test scenario in `tests/manual/scenarios/clone/` that validates
cloning with `--layout sibling` produces a regular (non-bare) repo.

- [ ] **Step 8: Run tests**

Run: `mise run test:unit && mise run clippy`

- [ ] **Step 9: Commit**

```bash
git add src/commands/clone.rs src/core/worktree/clone.rs src/git/
git commit -m "feat(layout): add --layout flag to clone with non-bare clone support"
```

---

## Task 3: Update Checkout/Start/Go for Layout Support

**Files:**

- Modify: `src/commands/checkout.rs` — resolve layout, pass to core
- Modify: `src/core/worktree/checkout.rs` — accept layout in params
- Modify: `src/core/worktree/checkout_branch.rs` — accept layout in params

### Description

Replace `calculate_worktree_path()` calls in checkout and checkout_branch with
layout-aware path computation. The layout is resolved in the command layer and
passed down via the params struct.

### Steps

- [ ] **Step 1: Add layout to CheckoutParams and CheckoutBranchParams**

In `src/core/worktree/checkout.rs`:

```rust
pub struct CheckoutParams {
    // ... existing fields ...
    pub layout: Option<Layout>,
}
```

Similarly for `CheckoutBranchParams` in `checkout_branch.rs`.

When `layout` is `Some`, use `layout.worktree_path()`. When `None`, fall back to
existing `calculate_worktree_path()` for backward compatibility.

- [ ] **Step 2: Update path computation in checkout.rs execute()**

Replace the `calculate_worktree_path()` call (around line 106) with:

```rust
let worktree_path = if let Some(ref layout) = params.layout {
    let ctx = build_template_context(project_root, &params.branch_name);
    layout.worktree_path(&ctx)?
} else {
    calculate_worktree_path(project_root, &params.branch_name, &remote_for_path, params.multi_remote_enabled)
};
```

- [ ] **Step 3: Do the same in checkout_branch.rs execute()**

- [ ] **Step 4: Update command layer to resolve and pass layout**

In `src/commands/checkout.rs`, in `run_checkout()` and `run_create_branch()`:

```rust
let global_config = GlobalConfig::load().unwrap_or_default();
let trust_db = TrustDatabase::load().unwrap_or_default();
let yaml_layout = /* load daft.yml if it exists, extract layout field */;

let (layout, _) = resolve_layout(&LayoutResolutionContext {
    cli_layout: None, // No --layout flag on checkout yet
    repo_store_layout: trust_db.get_layout(&git_dir),
    yaml_layout: yaml_layout.as_deref(),
    global_config: &global_config,
});
```

- [ ] **Step 5: Graceful degradation**

If the resolved layout `needs_bare()` but the repo is not bare (detected via
`git config core.bare`), log a warning and use the layout template anyway
(resolving `repo_path` as the non-bare repo root). Suggest
`daft layout transform`.

- [ ] **Step 6: Run existing tests**

Run: `mise run test:unit && mise run test:integration`

Ensure all existing checkout tests still pass with the `layout: None` fallback.

- [ ] **Step 7: Commit**

```bash
git add src/commands/checkout.rs src/core/worktree/checkout.rs src/core/worktree/checkout_branch.rs
git commit -m "feat(layout): layout-aware worktree path computation in checkout commands"
```

---

## Task 4: Add `--at` Flag and Sandbox Support

**Files:**

- Modify: `src/commands/checkout.rs` — add `--at` flag to Args, GoArgs,
  StartArgs
- Modify: `src/core/worktree/checkout.rs` — use `--at` path override
- Modify: `src/core/worktree/checkout_branch.rs` — same

### Description

Add `--at <path>` flag that overrides the template for worktree placement. When
`--at` is used without a branch name, create a detached HEAD sandbox.

### Steps

- [ ] **Step 1: Add `--at` to all Args structs**

```rust
    /// Place the worktree at a specific path instead of using the layout template.
    #[arg(long, value_name = "PATH")]
    pub at: Option<PathBuf>,
```

Add to `Args`, `GoArgs`, and `StartArgs`.

- [ ] **Step 2: Update core checkout to accept path override**

Add `at_path: Option<PathBuf>` to `CheckoutParams` and `CheckoutBranchParams`.
When set, use this path instead of the layout-computed path.

- [ ] **Step 3: Implement detached HEAD sandbox**

When `--at` is provided but no branch name is given (or via a new `--sandbox`
semantic), create the worktree in detached HEAD mode:

```rust
git.worktree_add_detached(at_path, "HEAD")?;
```

The sandbox worktree has no branch associated with it.

- [ ] **Step 4: Write tests**

Test that `--at /tmp/my-worktree feature-branch` creates a worktree at the
specified path. Test that `--at /tmp/sandbox` without a branch creates a
detached HEAD worktree.

- [ ] **Step 5: Run tests, commit**

```bash
mise run test:unit && mise run clippy
git add src/commands/checkout.rs src/core/worktree/
git commit -m "feat(layout): add --at flag for custom worktree placement and detached HEAD sandboxes"
```

---

## Task 5: Update List with Off-Template and Sandbox Indicators

**Files:**

- Modify: `src/commands/list.rs` — add indicators
- Modify: `src/core/worktree/list.rs` — add layout comparison field

### Description

When a worktree is not at its layout-expected path, show an indicator. Detached
HEAD sandboxes get a distinct marker. Worktrees placed with `--at` get an `--at`
indicator (not the off-template warning).

### Steps

- [ ] **Step 1: Add layout fields to WorktreeInfo**

In `src/core/worktree/list.rs`, add to `WorktreeInfo`:

```rust
    /// Whether this worktree is at its template-expected path.
    pub at_expected_path: bool,
    /// Whether this is a detached HEAD sandbox.
    pub is_sandbox: bool,
```

- [ ] **Step 2: Compute expected path during collection**

When collecting worktree info, resolve the repo's layout and compute the
expected path for each worktree. Compare actual vs expected. Detached HEAD
worktrees are sandboxes.

- [ ] **Step 3: Add indicators to list display**

In the annotation or a new column, show:

- Nothing if at expected path
- A marker (e.g., icon or text) if off-template
- A sandbox marker for detached HEAD worktrees

- [ ] **Step 4: Run tests, commit**

```bash
mise run test:unit && mise run clippy
git add src/commands/list.rs src/core/worktree/list.rs
git commit -m "feat(layout): add off-template and sandbox indicators to list"
```

---

## Task 6: Update Prune to Skip Sandboxes

**Files:**

- Modify: `src/commands/prune.rs` or `src/core/worktree/prune.rs`

### Description

Detached HEAD sandboxes should not be pruned. The prune logic currently removes
worktrees whose remote tracking branch has been deleted. Sandboxes have no
branch, so they should be filtered out early.

### Steps

- [ ] **Step 1: Read prune implementation**

- [ ] **Step 2: Add sandbox skip logic**

Filter out detached HEAD worktrees before identifying candidates for pruning.
These worktrees have no branch name in `git worktree list --porcelain` output
(they show `HEAD <sha>` instead of `branch refs/heads/...`).

- [ ] **Step 3: Write test**

- [ ] **Step 4: Run tests, commit**

```bash
mise run test:unit && mise run clippy
git add src/core/worktree/prune.rs
git commit -m "feat(layout): skip detached HEAD sandboxes during prune"
```

---

## Task 7: Create Layout Subcommands

**Files:**

- Create: `src/commands/layout.rs`
- Modify: `src/main.rs` — add routing
- Modify: `src/commands/mod.rs` — add module

### Description

Add `daft layout list`, `daft layout show`, and `daft layout transform`
subcommands.

### Steps

- [ ] **Step 1: Create `src/commands/layout.rs`**

Use clap subcommands:

```rust
#[derive(clap::Parser)]
struct Args {
    #[command(subcommand)]
    command: LayoutCommand,
}

#[derive(clap::Subcommand)]
enum LayoutCommand {
    /// List all available layouts
    List,
    /// Show the resolved layout for the current repo
    Show,
    /// Transform the current repo to a different layout
    Transform(TransformArgs),
}

#[derive(clap::Args)]
struct TransformArgs {
    /// Target layout name or template
    layout: String,
    /// Force transform even with uncommitted changes
    #[arg(short, long)]
    force: bool,
}
```

- [ ] **Step 2: Implement `layout list`**

Display all built-in layouts plus custom layouts from global config, showing
name, template, and whether bare is inferred.

- [ ] **Step 3: Implement `layout show`**

Resolve the current repo's layout using the config chain. Display which level
(CLI, repos.json, daft.yml, global, default) it came from.

- [ ] **Step 4: Implement `layout transform`**

This is the most complex part — generalized adopt/eject:

- Determine current layout (bare or non-bare, current path structure)
- Determine target layout (parse argument)
- If transitioning bare→non-bare or non-bare→bare, reuse existing `flow_adopt` /
  `flow_eject` core logic
- If transitioning between same-bareness layouts, move worktrees
- Update `repos.json` with new layout
- Update git-internal worktree registrations

For the initial implementation, support the most common transitions:

- non-bare → contained (what `adopt` does today)
- contained → non-bare (what `eject` does today)
- Leave non-bare↔non-bare worktree moves for a follow-up.

- [ ] **Step 5: Add routing in main.rs**

In the daft subcommand match block, add:

```rust
"layout" => commands::layout::run(),
```

- [ ] **Step 6: Add to mod.rs**

```rust
pub mod layout;
```

- [ ] **Step 7: Run tests, commit**

```bash
mise run test:unit && mise run clippy
git add src/commands/layout.rs src/commands/mod.rs src/main.rs
git commit -m "feat(layout): add layout list, show, and transform subcommands"
```

---

## Task 8: Make Adopt/Eject Aliases

**Files:**

- Modify: `src/commands/flow_adopt.rs`
- Modify: `src/commands/flow_eject.rs`

### Description

`adopt` becomes an alias for `layout transform contained`. `eject` becomes an
alias for `layout transform sibling`.

Keep the existing commands working but delegate to the layout transform logic
internally. Show a deprecation hint pointing users to the new commands.

### Steps

- [ ] **Step 1: Update adopt to delegate**

In `src/commands/flow_adopt.rs`, after parsing args, delegate to layout
transform with `contained` as the target layout. Print a hint:

```
Hint: `daft adopt` is now `daft layout transform contained`
```

- [ ] **Step 2: Update eject to delegate**

Same for eject → `layout transform sibling`.

- [ ] **Step 3: Preserve backward compatibility**

Ensure existing adopt/eject flags (`--trust-hooks`, `--no-hooks`, etc.) are
passed through correctly. The layout transform command should accept these.

- [ ] **Step 4: Run tests, commit**

```bash
mise run test:unit && mise run test:integration
git add src/commands/flow_adopt.rs src/commands/flow_eject.rs
git commit -m "refactor: make adopt/eject aliases for layout transform"
```

---

## Task 9: Auto-Gitignore for Nested Layout

**Files:**

- Modify: `src/core/worktree/checkout.rs` or a new utility

### Description

When a non-bare layout places worktrees inside the repo (e.g., `nested` uses
`.worktrees/`), daft should add the worktree directory to `.gitignore` when
creating the first worktree.

### Steps

- [ ] **Step 1: Detect when auto-gitignore is needed**

After creating a worktree, check:

- Layout is non-bare
- Worktree path is inside `repo_path`
- The parent directory of the worktree is not already in `.gitignore`

- [ ] **Step 2: Add to `.gitignore` idempotently**

```rust
fn ensure_gitignore_entry(repo_path: &Path, pattern: &str) -> Result<()> {
    let gitignore = repo_path.join(".gitignore");
    if gitignore.exists() {
        let contents = fs::read_to_string(&gitignore)?;
        if contents.lines().any(|line| line.trim() == pattern) {
            return Ok(()); // Already present
        }
    }
    let mut file = fs::OpenOptions::new()
        .create(true).append(true).open(&gitignore)?;
    writeln!(file, "{pattern}")?;
    Ok(())
}
```

For the `nested` layout, the pattern would be `.worktrees/`.

- [ ] **Step 3: Write test**

- [ ] **Step 4: Run tests, commit**

```bash
mise run test:unit && mise run clippy
git add src/core/worktree/
git commit -m "feat(layout): auto-gitignore worktree directory for non-bare in-repo layouts"
```
