---
branch: feat/progressive-adoption
---

# Progressive Adoption and Layout System

## New commands

- `daft layout list` — show available layouts
- `daft layout show` — show resolved layout for current repo
- `daft layout transform <layout>` — convert repo between layouts

## New flags

- `daft clone --layout <name|template>` — clone with a specific layout
- `git-worktree-init --layout <name|template>` — init with a specific layout
- `daft start -@ <path>` / `--at <path>` — place worktree at custom path
- `daft go -@ <path>` / `--at <path>` — same (requires creation context)

## Built-in layouts

| Name        | Template                                          | Bare |
| ----------- | ------------------------------------------------- | ---- |
| contained   | `{{ branch }}`                                    | yes  |
| sibling     | `../{{ repo }}.{{ branch \| sanitize }}`          | no   |
| nested      | `.worktrees/{{ branch \| sanitize }}`             | no   |
| centralized | `~/worktrees/{{ repo }}/{{ branch \| sanitize }}` | no   |

Default: `sibling`

## Config resolution order

1. `--layout` CLI flag
2. `repos.json` per-repo entry
3. `daft.yml` layout field
4. `~/.config/daft/config.toml` defaults.layout
5. Built-in default (sibling)

## Key behaviors

- Bare repo is inferred from template geometry, never user-facing
- `repos.json` replaces `trust.json` (auto-migrated)
- `adopt` / `eject` still work with deprecation hints
- Non-bare clone fires both `post-clone` and `worktree-post-create`
- `worktree-pre-create` reads `daft.yml` from target branch via git show
- Nested layout auto-adds `.worktrees/` to `.gitignore`
- Detached HEAD worktrees show sandbox indicator in list, skipped by prune
- `-@` on `go` requires worktree creation (fails if worktree already exists)
- Dev sandbox: `DAFT_CONFIG_DIR=.daft-sandbox/` isolates dev from real config
