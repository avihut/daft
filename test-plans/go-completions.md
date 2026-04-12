---
branch: fix/go-completions
---

# daft go Completion Overhaul

## Setup

- [ ] Install the current build via `mise run dev`.
- [ ] In a test repo with >= 3 worktrees, >= 2 local-only branches, and >= 5
      remote-only branches, open a new zsh shell and a new bash shell.

## Group ordering

- [ ] `daft go <TAB>` in zsh lists worktrees first, then local branches, then
      remote branches, in that order.
- [ ] Same in bash (flat list but preserving the order).
- [ ] Same in fish.
- [ ] The current worktree's branch does NOT appear in the worktree group.

## Descriptions and colors

- [ ] zsh: each entry shows a relative age ("3 days ago", etc.) in the
      description column.
- [ ] zsh: worktree entries are bright green, local are bright blue, remote are
      dim gray.
- [ ] fish: each entry shows `<age> · <group>` in the description.
- [ ] bash: no descriptions, but no flags leaked into the branch list.

## Flag gating

- [ ] `daft go -<TAB>` in zsh shows ONLY flags, no branches.
- [ ] `daft go -<TAB>` in bash shows ONLY flags, no branches.
- [ ] `daft go <TAB>` (no dash) shows ONLY branches, no flags.

## Fetch-on-miss + spinner

- [ ] Find a remote-only branch that's NOT in your local `refs/remotes/` (ask
      someone to push a branch, or delete your local remote ref with
      `git update-ref -d refs/remotes/origin/<branch>`).
- [ ] Type `daft go <prefix-of-that-branch><TAB>`. Expected: a braille-dot
      spinner with "Fetching refs from origin..." appears on the terminal for
      the duration of the fetch, then clears.
- [ ] After the fetch completes, the completion list now includes the remote
      branch.
- [ ] Immediately type the same completion again. Expected: no spinner this time
      (cooldown).
- [ ] Wait 30+ seconds and retry. Expected: spinner reappears.
- [ ] `git config daft.go.fetchOnMiss false` — expected: spinner never appears,
      regardless of cooldown.
- [ ] Reset with `git config --unset daft.go.fetchOnMiss`.

## Multi-remote mode

- [ ] Enable multi-remote via `daft multi-remote enable`.
- [ ] `daft go <TAB>` — remote-only entries now show `<remote>/<branch>`
      verbatim instead of stripped form.
- [ ] Disable multi-remote again via `daft multi-remote disable`.

## Non-interactive invocation

- [ ] `daft __complete daft-go "" --position 1 | head` inside a repository emits
      tab-separated lines with three columns each.
- [ ] The same command with `--fetch-on-miss` and a non-matching prefix does NOT
      draw a spinner (no /dev/tty when stdout is piped) and still emits any
      matching output.
