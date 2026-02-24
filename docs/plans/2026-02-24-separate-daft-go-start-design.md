# Separate `daft go` / `daft start` from `git-worktree-checkout`

## Problem

`daft start` and `daft go` reuse the `git-worktree-checkout` clap `Args` struct.
This causes:

- `daft start --help` shows `git-worktree-checkout` help with irrelevant flags
  (`-b`, `--start`, `-` previous worktree)
- `daft go --help` shows `-b` (a git-style flag) and base-branch positional arg
  that only applies with `-b`
- `daft start` works by injecting `-b` into raw args before clap parsing, a
  brittle hack

`daft remove` and `daft rename` already solved this problem in
`worktree_branch.rs` with dedicated `RemoveArgs` and `RenameArgs` structs. This
design applies the same pattern to `checkout.rs`.

## Design

### New structs in `checkout.rs`

**`GoArgs`** (for `daft go`):

- `branch_name: String` — positional, allows `-` for previous worktree
- `-b` / `--create-branch` — create a new branch
- `base_branch_name: Option<String>` — positional, base branch (only with `-b`)
- `-s` / `--start` — auto-create if branch doesn't exist
- `-c` / `--carry`, `--no-carry`
- `-r` / `--remote`
- `--no-cd`
- `-x` / `--exec`
- `-q` / `-v`
- `#[command(name = "daft go")]` with tailored help text

**`StartArgs`** (for `daft start`):

- `new_branch_name: String` — positional, required
- `base_branch_name: Option<String>` — positional, optional
- `-c` / `--carry`, `--no-carry`
- `-r` / `--remote`
- `--no-cd`
- `-x` / `--exec`
- `-q` / `-v`
- `#[command(name = "daft start")]` with tailored help text
- No `-b`, no `--start`, no `-` support

### Entry points

- `run()` — unchanged, parses `Args` for `git-worktree-checkout`
- `run_go()` — new, parses `GoArgs`, delegates to shared helpers
- `run_create()` — rewritten, parses `StartArgs`, delegates to shared helpers

### Routing changes in `main.rs`

- `"go"` routes to `commands::checkout::run_go()` (was `run()`)
- `"start"` routes to `commands::checkout::run_create()` (unchanged target, new
  implementation)

### What stays the same

- Core logic in `core/worktree/checkout.rs` and `checkout_branch.rs`
- `git-worktree-checkout` `Args` struct and behavior
- Shell completions (flag names unchanged for go/start)
