# Separate `daft go` / `daft start` from `git-worktree-checkout` — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to
> implement this plan task-by-task.

**Goal:** Give `daft go` and `daft start` their own clap Args structs and entry
points so their `--help` output is tailored and they don't leak irrelevant flags
from `git-worktree-checkout`.

**Architecture:** Follow the pattern already established by
`RemoveArgs`/`RenameArgs` in `worktree_branch.rs`. Add `GoArgs` and `StartArgs`
to `checkout.rs`, each with dedicated `run_go()`/`run_create()` entry points
that parse their own args and delegate to the existing shared helper functions
(`run_checkout`, `run_create_branch`, `run_go_previous`). Update routing,
completions, and man page generation.

**Tech Stack:** Rust, clap derive, shell completion generators
(bash/zsh/fish/fig)

---

### Task 1: Add `GoArgs` struct and `run_go()` entry point

**Files:**

- Modify: `src/commands/checkout.rs`

**Step 1: Add the `GoArgs` struct after the existing `Args` struct (after
line 112)**

```rust
/// Daft-style args for `daft go`. Separate from `Args` so that `--help`
/// shows only the flags relevant to navigating worktrees.
#[derive(Parser)]
#[command(name = "daft go")]
#[command(version = crate::VERSION)]
#[command(about = "Open an existing branch in a worktree")]
#[command(long_about = r#"
Opens a worktree for an existing local or remote branch. The worktree is
placed at the project root level as a sibling to other worktrees, using the
branch name as the directory name.

If the branch exists only on the remote, a local tracking branch is created
automatically. If the branch exists both locally and on the remote, the local
branch is checked out and upstream tracking is configured.

If a worktree for the specified branch already exists, no new worktree is
created; the working directory is changed to the existing worktree instead.

Use '-' as the branch name to switch to the previous worktree, similar to
'cd -'. Repeated 'daft go -' toggles between the two most recent worktrees.

With -b, creates a new branch and worktree in a single operation. The new
branch is based on the current branch, or on <base-branch> if specified.
Prefer 'daft start' for creating new branches.

With -s (--start), if the specified branch does not exist locally or on the
remote, a new branch and worktree are created automatically. This can also
be enabled permanently with the daft.go.autoStart git config option.

Lifecycle hooks from .daft/hooks/ are executed if the repository is trusted.
See daft-hooks(1) for hook management.
"#)]
pub struct GoArgs {
    #[arg(
        help = "Branch to open; use '-' for previous worktree",
        allow_hyphen_values = true
    )]
    branch_name: String,

    #[arg(
        help = "Base branch for -b (defaults to current branch)"
    )]
    base_branch_name: Option<String>,

    #[arg(
        short = 'b',
        long = "create-branch",
        help = "Create a new branch (prefer 'daft start' instead)"
    )]
    create_branch: bool,

    #[arg(
        short = 's',
        long = "start",
        help = "Create a new worktree if the branch does not exist"
    )]
    start: bool,

    #[arg(short, long, help = "Operate quietly; suppress progress reporting")]
    quiet: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,

    #[arg(
        short = 'c',
        long = "carry",
        help = "Apply uncommitted changes from the current worktree to the new one"
    )]
    carry: bool,

    #[arg(long, help = "Do not carry uncommitted changes")]
    no_carry: bool,

    #[arg(
        short = 'r',
        long = "remote",
        help = "Remote for worktree organization (multi-remote mode)"
    )]
    remote: Option<String>,

    #[arg(long, help = "Do not change directory to the new worktree")]
    no_cd: bool,

    #[arg(
        short = 'x',
        long = "exec",
        help = "Run a command in the worktree after setup completes (repeatable)"
    )]
    exec: Vec<String>,
}
```

**Step 2: Add the `run_go()` entry point**

Replace the existing `run()` comment at line 114 to keep it for
`git-worktree-checkout` only. Add a new `run_go()` function:

```rust
/// Entry point for `daft go`.
pub fn run_go() -> Result<()> {
    let mut raw = crate::get_clap_args("daft-go");
    raw[0] = "daft go".to_string();
    let go_args = GoArgs::parse_from(raw);

    // Validate: base_branch_name only valid with -b
    if go_args.base_branch_name.is_some() && !go_args.create_branch {
        anyhow::bail!("<BASE_BRANCH_NAME> can only be used with -b/--create-branch");
    }

    // Convert to the internal Args and delegate
    let args = Args {
        branch_name: go_args.branch_name,
        base_branch_name: go_args.base_branch_name,
        create_branch: go_args.create_branch,
        quiet: go_args.quiet,
        verbose: go_args.verbose,
        carry: go_args.carry,
        no_carry: go_args.no_carry,
        remote: go_args.remote,
        no_cd: go_args.no_cd,
        exec: go_args.exec,
        start: go_args.start,
    };
    run_with_args(args)
}
```

**Step 3: Build to verify it compiles**

Run: `cargo build 2>&1 | head -20` Expected: compiles successfully

**Step 4: Commit**

```
feat(go): add dedicated GoArgs for daft go
```

---

### Task 2: Add `StartArgs` struct and rewrite `run_create()`

**Files:**

- Modify: `src/commands/checkout.rs`

**Step 1: Add the `StartArgs` struct after `GoArgs`**

```rust
/// Daft-style args for `daft start`. Separate from `Args` so that `--help`
/// shows only the flags relevant to creating new branches.
#[derive(Parser)]
#[command(name = "daft start")]
#[command(version = crate::VERSION)]
#[command(about = "Create a new branch and worktree")]
#[command(long_about = r#"
Creates a new branch and a corresponding worktree in a single operation. The
worktree is placed at the project root level as a sibling to other worktrees,
using the branch name as the directory name.

The new branch is based on the current branch, or on <base-branch> if
specified. After creating the branch locally, it is pushed to the remote and
upstream tracking is configured (unless disabled via daft.checkoutBranch.push).

This command can be run from anywhere within the repository.

Lifecycle hooks from .daft/hooks/ are executed if the repository is trusted.
See daft-hooks(1) for hook management.
"#)]
pub struct StartArgs {
    #[arg(help = "Name of the new branch to create")]
    new_branch_name: String,

    #[arg(help = "Branch to use as the base (defaults to current branch)")]
    base_branch_name: Option<String>,

    #[arg(short, long, help = "Operate quietly; suppress progress reporting")]
    quiet: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,

    #[arg(
        short = 'c',
        long = "carry",
        help = "Apply uncommitted changes from the current worktree to the new one"
    )]
    carry: bool,

    #[arg(long, help = "Do not carry uncommitted changes")]
    no_carry: bool,

    #[arg(
        short = 'r',
        long = "remote",
        help = "Remote for worktree organization (multi-remote mode)"
    )]
    remote: Option<String>,

    #[arg(long, help = "Do not change directory to the new worktree")]
    no_cd: bool,

    #[arg(
        short = 'x',
        long = "exec",
        help = "Run a command in the worktree after setup completes (repeatable)"
    )]
    exec: Vec<String>,
}
```

**Step 2: Rewrite `run_create()` to parse `StartArgs` instead of injecting
`-b`**

```rust
/// Entry point for `daft start`.
pub fn run_create() -> Result<()> {
    let mut raw = crate::get_clap_args("daft-start");
    raw[0] = "daft start".to_string();
    let start_args = StartArgs::parse_from(raw);

    // Convert to the internal Args and delegate
    let args = Args {
        branch_name: start_args.new_branch_name,
        base_branch_name: start_args.base_branch_name,
        create_branch: true, // daft start always creates
        quiet: start_args.quiet,
        verbose: start_args.verbose,
        carry: start_args.carry,
        no_carry: start_args.no_carry,
        remote: start_args.remote,
        no_cd: start_args.no_cd,
        exec: start_args.exec,
        start: false, // Not applicable for start
    };
    run_with_args(args)
}
```

**Step 3: Build to verify it compiles**

Run: `cargo build 2>&1 | head -20` Expected: compiles successfully

**Step 4: Commit**

```
feat(start): add dedicated StartArgs for daft start
```

---

### Task 3: Update routing in `main.rs`

**Files:**

- Modify: `src/main.rs`

**Step 1: Change the `"go"` route from `run()` to `run_go()`**

At line 89, change:

```rust
"go" => commands::checkout::run_go(),
```

The `"start"` line (90) already calls `run_create()` which is now rewritten.

**Step 2: Build and smoke test**

Run: `cargo build && cargo run -- start --help` Expected: shows `daft start`
help with `new_branch_name` and `base_branch_name` positionals, no `-b`, no
`--start`

Run: `cargo run -- go --help` Expected: shows `daft go` help with branch name,
`-s`/`--start`, `-b`, no mention of `git-worktree-checkout`

**Step 3: Commit**

```
refactor: route daft go/start to dedicated entry points
```

---

### Task 4: Update shell completions

**Files:**

- Modify: `src/commands/completions/mod.rs`
- Modify: `src/commands/completions/bash.rs`
- Modify: `src/commands/completions/zsh.rs`
- Modify: `src/commands/completions/fish.rs`

**Step 1: Update `VERB_ALIAS_GROUPS` in `mod.rs` (line 28)**

Change:

```rust
(&["go", "start"], "git-worktree-checkout"),
```

to separate entries:

```rust
(&["go"], "daft-go"),
(&["start"], "daft-start"),
```

**Step 2: Add `"daft-go"` and `"daft-start"` to the `COMMANDS` array (line 37)**

Add after the existing entries:

```rust
"daft-go",
"daft-start",
```

**Step 3: Add entries in `get_command_for_name` (line 52)**

```rust
"daft-go" => Some(crate::commands::checkout::GoArgs::command()),
"daft-start" => Some(crate::commands::checkout::StartArgs::command()),
```

**Step 4: Update bash completions in `bash.rs`**

At line 135-139, change the `go|start)` case to separate cases:

```bash
go)
    COMP_WORDS=("daft-go" "${COMP_WORDS[@]:2}")
    COMP_CWORD=$((COMP_CWORD - 1))
    _daft_go
    return 0
    ;;
start)
    COMP_WORDS=("daft-start" "${COMP_WORDS[@]:2}")
    COMP_CWORD=$((COMP_CWORD - 1))
    _daft_start
    return 0
    ;;
```

**Step 5: Update zsh completions in `zsh.rs`**

At line 148-152, change the `go|start)` case to separate cases:

```zsh
go)
    words=("daft-go" "${(@)words[3,-1]}")
    CURRENT=$((CURRENT - 1))
    __daft_go_impl
    return
    ;;
start)
    words=("daft-start" "${(@)words[3,-1]}")
    CURRENT=$((CURRENT - 1))
    __daft_start_impl
    return
    ;;
```

**Step 6: Update fish completions in `fish.rs`**

At line 173, change:

```fish
complete -c daft -n '__fish_seen_subcommand_from go start carry update' -f -a "(daft __complete git-worktree-checkout '' 2>/dev/null)"
```

to:

```fish
complete -c daft -n '__fish_seen_subcommand_from go' -f -a "(daft __complete daft-go '' 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from start' -f -a "(daft __complete daft-start '' 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from carry update' -f -a "(daft __complete git-worktree-checkout '' 2>/dev/null)"
```

Also update the `needs_branch_completion` match at line 9 to include `"daft-go"`
and `"daft-start"`.

**Step 7: Build to verify**

Run: `cargo build 2>&1 | head -20` Expected: compiles successfully

**Step 8: Commit**

```
refactor(completions): separate go/start completion targets
```

---

### Task 5: Update xtask man page generation

**Files:**

- Modify: `xtask/src/main.rs`

**Step 1: Add `"daft-go"` and `"daft-start"` to the xtask
`get_command_for_name`**

At line 141, before `_ => None`, add:

```rust
"daft-go" => Some(daft::commands::checkout::GoArgs::command()),
"daft-start" => Some(daft::commands::checkout::StartArgs::command()),
```

**Step 2: Verify man page generation picks up the new structs**

The `DAFT_VERBS` entries for `daft-go` and `daft-start` already exist (lines
53-62). The man page generation logic at line 337 already checks
`get_command_for_name(verb.daft_name)` first and uses the dedicated Args struct
if found. So adding `daft-go` and `daft-start` to `get_command_for_name` is
sufficient — no changes to `DAFT_VERBS` needed.

**Step 3: Generate man pages and verify**

Run: `mise run man:gen`

Verify `daft-go --help` and `daft-start --help` look correct: Run:
`cargo run -- go --help` Run: `cargo run -- start --help`

**Step 4: Commit**

```
refactor(man): generate dedicated man pages for daft go/start
```

---

### Task 6: Run tests and lint

**Files:** None (verification only)

**Step 1: Format**

Run: `mise run fmt`

**Step 2: Clippy**

Run: `mise run clippy` Expected: zero warnings

**Step 3: Unit tests**

Run: `mise run test:unit` Expected: all pass

**Step 4: Integration tests**

Run: `mise run test:integration` Expected: all pass (existing tests exercise
`git-worktree-checkout` which is unchanged)

**Step 5: Man page verification**

Run: `mise run man:verify` Expected: all man pages up-to-date

**Step 6: Final commit if any fixups were needed**

```
fix: address lint/test issues from go/start separation
```
