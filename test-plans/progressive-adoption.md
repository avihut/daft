---
branch: feat/progressive-adoption
---

# Progressive Adoption and Layout System

## Layout resolution

- [ ] Default layout is sibling when no config exists
- [ ] `daft clone --layout contained <url>` creates bare repo with worktree
      children
- [ ] `daft clone --layout sibling <url>` creates regular clone
- [ ] `daft clone --layout nested <url>` creates regular clone
- [ ] `daft clone <url>` without --layout uses the resolved default (sibling)
- [ ] Global config `defaults.layout` overrides built-in default
- [ ] repos.json per-repo layout overrides global config
- [ ] daft.yml `layout` field is used as team suggestion (lower priority than
      repos.json)

## Clone workflows

### Sibling layout

- [ ] `daft clone --layout sibling <url>` creates a regular (non-bare) clone
- [ ] Files are in the repo root, not in a branch subdirectory
- [ ] `daft start feature/test` from inside the clone creates
      `<parent>/repo.feature-test/`
- [ ] `daft list` from inside the clone shows both worktrees
- [ ] `daft go main` navigates back to the original clone

### Contained layout

- [ ] `daft clone --layout contained <url>` creates bare repo + first worktree
- [ ] Project root has `.git/` (bare) and `main/` (or default branch name)
- [ ] `daft start feature/test` creates `<project>/feature/test/`
- [ ] `daft list` shows both worktrees
- [ ] `daft go main` navigates back

### Nested layout

- [ ] `daft clone --layout nested <url>` creates a regular clone
- [ ] `daft start feature/test` creates `.worktrees/feature-test/` inside the
      repo
- [ ] `.gitignore` is updated with `.worktrees/` entry
- [ ] Creating a second worktree does not duplicate the `.gitignore` entry
- [ ] `git status` does not show `.worktrees/` as untracked

## Layout subcommands

### `daft layout list`

- [ ] Shows all 4 built-in layouts: contained, sibling, nested, centralized
- [ ] Shows template and bare status for each
- [ ] Marks the current default with an indicator
- [ ] Custom layouts from global config appear in the list

### `daft layout show`

- [ ] Shows the resolved layout name, template, and bare status
- [ ] Shows which config level it came from (repos.json, daft.yml, global,
      default)
- [ ] Works from inside a worktree of a cloned repo

### `daft layout transform`

- [ ] `daft layout transform contained` converts a regular repo to bare +
      worktrees
- [ ] `daft layout transform sibling` converts a bare repo back to regular
- [ ] repos.json is updated with the new layout after transform
- [ ] `daft layout show` reflects the new layout after transform

## --at flag

### `daft start -@ <path> <branch>`

- [ ] Creates worktree at the specified path instead of the layout-computed path
- [ ] `-@` works as a shorthand for `--at`
- [ ] Worktree appears in `daft list`
- [ ] `daft remove <branch>` removes the worktree at the custom path

### `daft go -@ <path> <branch>`

- [ ] Succeeds when branch exists but has no worktree yet
- [ ] Succeeds with `--start` when branch does not exist
- [ ] Fails with error when worktree already exists for the branch
- [ ] Fails with error when branch not found and --start is not active
- [ ] Error message suggests using --start when branch doesn't exist

### `daft go` with autoStart config

- [ ] `daft go -@ <path> <branch>` succeeds when `daft.go.autoStart=true` and
      branch doesn't exist

## Adopt / Eject deprecation

- [ ] `daft adopt` still works and converts to contained layout
- [ ] `daft adopt` shows deprecation hint pointing to
      `daft layout transform contained`
- [ ] `daft eject` still works and converts to sibling
- [ ] `daft eject` shows deprecation hint pointing to
      `daft layout transform sibling`
- [ ] repos.json is updated with layout after adopt/eject

## Layout persistence

- [ ] Clone with --layout stores the layout in repos.json
- [ ] Subsequent `daft start` uses the stored layout without --layout flag
- [ ] `daft layout show` shows the stored layout and source as repos.json

## Post-clone layout reconciliation

- [ ] Clone a repo that has `layout: contained` in daft.yml without --layout
      flag
- [ ] repos.json is updated with the daft.yml layout suggestion
- [ ] A hint is shown if the resolved layout differs from daft.yml

## Hooks across layouts

- [ ] `post-clone` hook fires for both bare and non-bare clones
- [ ] `worktree-post-create` fires for non-bare clones (in addition to
      post-clone)
- [ ] `worktree-post-create` fires when creating worktrees in any layout
- [ ] `worktree-pre-create` reads daft.yml from the target branch (not source
      worktree)

## Sandbox (detached HEAD)

- [ ] Sandbox indicator appears in `daft list` for detached HEAD worktrees
- [ ] `daft prune` skips detached HEAD worktrees

## Dev sandbox

- [ ] `DAFT_CONFIG_DIR` is set to `.daft-sandbox/` in the dev environment
- [ ] Running `daft clone` from a dev build uses `.daft-sandbox/` for config
- [ ] The user's real `~/.config/daft/` is not modified during dev testing
- [ ] `.daft-sandbox/` starts empty (first-use experience)
