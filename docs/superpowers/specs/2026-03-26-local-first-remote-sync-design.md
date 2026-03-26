# Local-First Remote Sync

## Problem

daft currently synchronizes with the remote on every mutating worktree command
-- `start`/`checkout` fetches before creating worktrees and pushes new branches,
`branch-delete`/`remove` deletes the remote branch. This makes initial adoption
harder for users who just want daft for worktree layout management and prefer
doing all git remote work themselves.

## Design

### Engagement ladder

daft supports a progressive engagement model:

1. **Worktree layout manager** -- user runs `start`, `go`, `remove`. daft never
   touches the network. User handles fetch, pull, push, rebase with git
   directly.
2. **Local maintenance** -- user adds `prune` and `sync` to their workflow.
   These commands are inherently remote and the user opts in by choosing to run
   them.
3. **Full sync** -- user enables remote-sync settings. Mutating commands
   (`start`, `remove`) keep local and remote in lockstep automatically.

The default is rung 1 (local-first). Users climb the ladder by running more
commands or configuring remote-sync.

### Settings

Three git config keys control remote operations in worktree management commands.
All default to `false` (local-first):

| Key                        | Default | Controls                                    |
| -------------------------- | ------- | ------------------------------------------- |
| `daft.checkout.fetch`      | `false` | Fetch from remote before creating worktrees |
| `daft.checkout.push`       | `false` | Push new branches to remote                 |
| `daft.branchDelete.remote` | `false` | Delete remote branch when removing          |

`daft.checkout.push` already exists but its default flips from `true` to
`false`. The other two are new.

`daft.checkout.upstream` remains a separate, independent toggle. It is a local
operation (sets the tracking ref) and is useful regardless of remote-sync mode.

### Commands unaffected

These commands are inherently remote-facing and stay as-is:

- **`clone`** -- always hits the remote
- **`prune`** -- fetches with `--prune` to detect gone branches, cleans up
  locally
- **`sync`** -- fetch + update + optional push/rebase
- **`update`** -- pulls from upstream
- **`go`** (without `--start`) -- purely local, already no remote operations
- **`list`** -- purely local

### The `daft config` command

`daft config` is a new top-level command with subcommands. The first subcommand
is `remote-sync`. The command is designed as a broader namespace for managing
daft settings in the future.

### The `daft config remote-sync` TUI

A navigable TUI where items are toggled in place:

```
$ daft config remote-sync

 Remote Sync                          local config
 ─────────────────────────────────────────────────
 › ● Full sync
   ○ Local only
   ○ Custom
     ├ [ ] Fetch before checkout
     ├ [ ] Push new branches
     └ [ ] Delete remote branches

 ↑↓ navigate  space toggle  enter confirm  q cancel
```

**Behavior:**

- **Full sync** checks all three sub-items.
- **Local only** clears all three sub-items.
- **Custom** unlocks individual toggles; navigate into them and toggle with
  space.
- The radio group and checkboxes stay in sync -- manually checking all three
  moves the radio to "Full sync"; clearing all moves it to "Local only."
- Current values load from git config on launch, so re-running shows existing
  settings.
- **enter** writes config and exits. **q** cancels without saving.

**Scope:** Writes to local git config (per-repo) by default. `--global` flag
switches to global config (the header updates to show "global config").

**Non-interactive shortcuts** for scripting:

- `daft config remote-sync --on` -- enables all three, no prompts
- `daft config remote-sync --off` -- disables all three, no prompts
- `daft config remote-sync --status` -- prints current effective values

### Per-invocation overrides

Hard overrides that bypass config for a single invocation.

**`branch-delete` / `remove`:**

- `--local` -- only delete worktree + local branch, skip remote deletion (even
  if `branchDelete.remote` is on)
- `--remote` -- only delete the remote branch, keep local worktree and branch.
  Errors if the branch has no remote tracking branch.

**`start` / `checkout -b` / `go --start`:**

- `--local` -- create branch and worktree locally, skip both fetch and push
  (even if `checkout.fetch` and `checkout.push` are on)

### Migration

This is a minor version bump. `daft doctor` shows a one-time note when none of
the three remote-sync keys are explicitly set in git config:

```
ℹ  Remote sync defaults have changed — daft no longer fetches,
   pushes, or deletes remote branches by default.
   Run `daft config remote-sync` to configure your preference.
```

The message goes away once any key is explicitly set or the user runs
`daft config remote-sync`.
