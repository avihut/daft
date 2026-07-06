---
name: daft-worktree-workflow
description:
  Guides the daft worktree workflow for compartmentalized Git development. Use
  when working in daft-managed repositories (repos with a .git/ bare directory
  and branch worktrees as sibling directories), when setting up worktree
  environment isolation, or when users ask about worktree-based workflows.
  Covers daft commands, hooks automation via daft.yml, and environment tooling
  like mise, direnv, nvm, and pyenv.
---

# daft Worktree Workflow

## Core Philosophy

daft treats each Git worktree as a **compartmentalized workspace**, not just a
branch checked out to disk. Each worktree is a fully isolated environment with
its own:

- Working files and Git index
- Build artifacts (`node_modules/`, `target/`, `venv/`, `.build/`)
- IDE state and configuration (`.vscode/`, `.idea/`)
- Environment files (`.envrc`, `.env`)
- Running processes (dev servers, watchers, test runners)
- Installed dependencies (potentially different versions per branch)

This means creating a new worktree is not just "checking out a branch" -- it is
spinning up a new development environment. Automation (via `daft.yml` hooks)
should install dependencies, configure environment tools, and prepare the
workspace so the developer can start working immediately.

Never use `git checkout` or `git switch` to change branches in a daft-managed
repo. Navigate between worktree directories instead.

## Detecting a daft-managed Repository

daft supports multiple layouts. The most common is the **contained** layout:

```
my-project/
+-- .git/                    # Bare repository (shared Git metadata)
+-- main/                    # Worktree for the default branch
|   +-- src/
|   +-- package.json
+-- feature/auth/            # Worktree for a feature branch
|   +-- src/
|   +-- package.json
+-- bugfix/login/            # Worktree for a bugfix branch
```

Other layouts you may encounter:

- **Sibling** (default): `my-project/` + `my-project.feature-auth/` as siblings
- **Nested**: `my-project/.worktrees/feature-auth/` hidden inside the repo
- **Centralized**: worktrees stored in `~/.local/share/daft/worktrees/`

Key indicators of any daft-managed repository:

- Use `git rev-parse --git-common-dir` from any worktree to find the shared Git
  directory
- Run `daft layout show` to see which layout the repo uses
- For contained layout: `.git/` at the project root is a **bare repository**
  (directory, not a file) with branch worktrees as sibling directories
- For other layouts: the main checkout looks like a normal Git repo, but
  worktrees are managed by daft elsewhere

If you see any of these patterns, the user is using daft. Apply worktree-aware
guidance throughout the session.

## Layouts

daft supports four built-in layouts that control where worktrees are placed:

| Layout        | Template                                                            | Description                           |
| ------------- | ------------------------------------------------------------------- | ------------------------------------- |
| `contained`   | `{{ repo_path }}/{{ branch }}`                                      | Worktrees inside the repo directory   |
| `sibling`     | `{{ repo }}.{{ branch \| sanitize }}`                               | Worktrees next to the repo (default)  |
| `nested`      | `{{ repo }}/.worktrees/{{ branch \| sanitize }}`                    | Worktrees in a hidden subdirectory    |
| `centralized` | `{{ daft_data_dir }}/worktrees/{{ repo }}/{{ branch \| sanitize }}` | Worktrees in a central data directory |

Layout commands:

| Command                          | Description                               |
| -------------------------------- | ----------------------------------------- |
| `daft layout show`               | Show resolved layout for the current repo |
| `daft layout list`               | List all available layouts                |
| `daft layout transform <layout>` | Convert repo to a different layout        |
| `daft layout default [layout]`   | View or set global default layout         |

Layout is selected at clone time via `--layout` flag, `daft.yml` `layout` field,
global config default, or the built-in default (sibling). Users can also define
custom layouts with templates in `~/.config/daft/config.toml`.

The `daft.yml` file can specify a `layout` field alongside hooks to set the
team-recommended layout:

```yaml
layout: contained

hooks:
  worktree-post-create:
    jobs:
      - name: install-deps
        run: npm install
```

## Invocation Forms

daft commands can be invoked in three ways:

| Form           | Example                               | Requires                                |
| -------------- | ------------------------------------- | --------------------------------------- |
| Git subcommand | `git worktree-checkout feature/auth`  | `git-worktree-checkout` symlink on PATH |
| Direct binary  | `daft worktree-checkout feature/auth` | Only the `daft` binary                  |
| Verb alias     | `daft go feature/auth`                | Only the `daft` binary                  |
| Shortcut alias | `gwtco feature/auth`                  | Shortcut symlink on PATH                |

The git subcommand form (`git worktree-*`) is what users type in their terminals
and what documentation references. Shortcuts are optional short aliases managed
via `daft activate shortcuts`.

### Verb Aliases

daft provides short verb aliases for common commands:

| Verb Alias    | Equivalent Command          |
| ------------- | --------------------------- |
| `daft go`     | `daft worktree-checkout`    |
| `daft start`  | `daft worktree-checkout -b` |
| `daft clone`  | `daft worktree-clone`       |
| `daft init`   | `daft worktree-init`        |
| `daft carry`  | `daft worktree-carry`       |
| `daft exec`   | `daft worktree-exec`        |
| `daft merge`  | `daft worktree-merge`       |
| `daft list`   | `daft worktree-list`        |
| `daft update` | `daft worktree-fetch`       |
| `daft prune`  | `daft worktree-prune`       |
| `daft remove` | `daft worktree-branch -d`   |
| `daft rename` | `daft worktree-branch -m`   |
| `daft sync`   | `daft git-worktree-sync`    |
| `daft adopt`  | `daft worktree-flow-adopt`  |
| `daft eject`  | `daft worktree-flow-eject`  |

**Agent execution rule**: When running daft commands, always use the direct
binary form (`daft <subcommand>`) or verb aliases. The git subcommand form
requires symlinks and shell wrappers that are not available in most agent shell
sandboxes. When explaining daft usage to users, reference the git subcommand
form (`git worktree-*`), verb aliases, or shortcuts, as these are what users
interact with in their configured terminals.

After creating a worktree with `daft`, the shell does not automatically `cd`
into it (that requires shell wrappers). Navigate to the new worktree using the
layout convention: `cd ../<branch-name>/` relative to any existing worktree.

When running `daft repo remove` from inside the repo being deleted, the agent's
cwd will be invalidated mid-operation. Either pass an explicit path
(`daft repo remove /path/to/repo`) and stay outside, or `cd` to a safe ancestor
first. The binary writes a redirect path to `$DAFT_CD_FILE` for shell wrappers,
but agent shells typically don't have that wrapper installed, so follow-up
commands will fail with `chdir: no such file or directory` until the agent's cwd
is fixed.

## Global Flags

All daft commands and `git-worktree-*` symlinked entries accept a top-level
`-C <path>` flag that changes the effective working directory before resolving
any path-dependent state (repo discovery, layout, hooks, `daft.yml`). Semantics
match `git -C`.

```bash
daft -C /path/to/repo list             # equivalent to: cd /path/to/repo && daft list
daft -C /path/to/repo go feature/x     # creates worktree inside /path/to/repo
git-worktree-list -C /path/to/repo     # works for symlink entries too
```

**This is the recommended pattern for agents** operating across multiple daft
worktrees in a session. It eliminates the need to `cd` between invocations and
makes each command self-contained ("do X in path Y"). Prefer `-C` over spawning
a subshell with `cd && daft …`.

Rules:

- Composes like `git -C`: `daft -C /a -C b list` is equivalent to
  `daft -C /a/b list`. Each subsequent non-absolute `-C` resolves relative to
  the previous applied cwd. **Not** "last wins".
- `-C ""` is a no-op (cwd unchanged).
- Missing/non-directory path: terse error, exit code 2.
- Relative paths in subcommand arguments resolve against the post-`-C` cwd.
- `-C` is parsed only at the very front of the argv (before the subcommand
  name), so a subcommand-local `-C` (e.g. an inner shell command in `daft exec`)
  is preserved.

## Command Reference

All commands below use the `daft` binary form for agent execution. Users know
these as `git` subcommands (e.g., `daft worktree-checkout` is
`git worktree-checkout` to the user).

### Worktree Lifecycle

| Command                                                                                                                                                                      | Description                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `daft worktree-clone <url> [--layout <LAYOUT>] [--install [--git-exclude]]`                                                                                                  | Clone a remote repository into daft's worktree layout (use `--layout` to choose: `contained`, `sibling`, `nested`, `centralized`, or custom). `--install` bootstraps a starter `daft.yml` in the new worktree(s) after cloning (same prompt/`--git-exclude` behavior as `daft repo install`), copies it into every worktree of a multi-branch clone, and implies `--trust-hooks`; it skips if the repo already ships a tracked `daft.yml`, and is rejected with `--no-checkout`.                                                              |
| `daft worktree-init <name> [--layout <LAYOUT>]`                                                                                                                              | Initialize a new local repository in worktree layout (use `--layout` to choose layout)                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `daft worktree-checkout <branch>`                                                                                                                                            | Create a worktree for an existing local or remote branch; pass `--local` to skip the remote fetch even when `daft.checkout.fetch` is enabled                                                                                                                                                                                                                                                                                                                                                                                                  |
| `daft worktree-checkout -- -`                                                                                                                                                | Switch to the previous worktree (`cd -` style toggle)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `daft worktree-checkout -s <branch>`                                                                                                                                         | Same as above, but auto-creates branch if not found (also `daft.go.autoStart`)                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `daft worktree-checkout -b <new-branch> [base]`                                                                                                                              | Create a new branch and worktree from current or specified base; by default does not push (see `daft.checkout.push`); pass `--local` to skip remote even when push is enabled                                                                                                                                                                                                                                                                                                                                                                 |
| `daft worktree-branch -d <branch>`                                                                                                                                           | Safely delete a branch: its worktree and local branch ref; remote branch is deleted only when `daft.branchDelete.remote` is enabled; pass `--local` to skip remote, `--remote` to delete only the remote branch                                                                                                                                                                                                                                                                                                                               |
| `daft worktree-branch -D <branch>`                                                                                                                                           | Force-delete a branch bypassing safety checks; for the default branch, removes worktree only (preserves branch ref and remote)                                                                                                                                                                                                                                                                                                                                                                                                                |
| `daft worktree-prune [-f] [-v\|-vv] [--stat summary\|lines]`                                                                                                                 | Remove worktrees whose remote branches have been deleted (`-v` hook details, `-vv` full sequential)                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| `daft worktree-carry <targets>`                                                                                                                                              | Transfer uncommitted changes to one or more other worktrees                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| `daft worktree-fetch [targets]`                                                                                                                                              | Update worktree branches from remote (supports refspec syntax source:destination)                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `daft worktree-branch -m <source> <new-branch>`                                                                                                                              | Rename a branch, move its worktree, and rename the remote branch                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `daft git-worktree-sync [-f] [-v\|-vv] [--rebase BRANCH [--autostash]] [--push [--force-with-lease] [--no-verify]] [--include VALUE]... [--stat summary\|lines]`             | Synchronize all worktrees: prune stale + update all + optional rebase + optional push (`-f`/`--prune-dirty` for dirty worktrees, `-v` hook details, `-vv` full sequential). By default, rebase and push apply only to branches you own (matching `git config user.email`). Use `--include` to add more branches: `unowned` for all, an email address for a teammate's branches, or a branch name for one specific branch.                                                                                                                     |
| `daft worktree-merge [SOURCE...] [--into <TARGET>] [--merge\|--squash\|--rebase\|--rebase-merge] [-s <STRAT>] [--adopt-target\|--no-adopt-target] [-y] [-r] [--set-default]` | Merge one or more source branches into a target worktree's branch. Without `--into`, the target is the current worktree. With `--into <target>`, merges into another worktree without changing directories. Multiple sources trigger octopus. `-r` removes the source worktree and branch after success. `--adopt-target` creates an ephemeral worktree when the target branch has no worktree. `--set-default` writes style and cleanup to `git config --local`. Finish with `daft worktree-merge --abort\|--continue\|--quit [<worktree>]`. |

### Adoption and Ejection

| Command                           | Description                                                |
| --------------------------------- | ---------------------------------------------------------- |
| `daft worktree-flow-adopt [path]` | Convert a traditional repository to daft's worktree layout |
| `daft worktree-flow-eject`        | Convert back to a traditional repository layout            |

### Management

| Command                                                                                          | Description                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| ------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `daft worktree-list [--format <FMT>] [-b\|-r\|-a] [--stat summary\|lines] [--columns COLS]`      | List all worktrees with branch (`✦` = default), path (relative to cwd), base ahead/behind, file status (+N staged, -N unstaged, ?N untracked), remote status (⇡N unpushed, ⇣N unpulled), branch age, and commit info. Use `-b`/`-r`/`-a` to include local/remote branches without worktrees. Use `--format json` for machine-readable output; JSON includes `is_default_branch`, `staged`, `unstaged`, `untracked`, `remote_ahead`, `remote_behind`, `branch_age`, `owner_name`, `owner_email` fields. When `user.email` is configured, output is split into two sections (your branches / other branches) based on the resolved owner per `daft.ownership.strategy`. Use `--columns owner` or `--columns +owner` to show the Owner column (author name of the resolved owner).                                                                                                                                                                                                                                                                                                                                                                                                  |
| `daft worktree-exec [TARGETS]... [--all] [-x CMD]... [-- CMD ARGS]...`                           | Run command(s) across one or more worktrees: positional/glob targets, `--all` for every worktree, `-x` for repeatable shell pipelines, trailing `--` for direct argv. Parallel by default; `--sequential`/`--keep-going` for serial modes. Multi-worktree runs show a live progress row per worktree; failed worktrees' output is dumped to stdout after all complete.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| `daft config remote-sync [--on\|--off\|--status\|--global]`                                      | Configure remote sync behavior: toggle fetch, push, and remote delete globally or per-repo; no args opens interactive TUI                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `daft layout [show\|list\|transform\|default]`                                                   | Manage worktree layouts: show current, list available, transform between layouts, set global default                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `daft hooks <subcommand>`                                                                        | Manage hooks trust and configuration (`trust`, `prompt`, `deny`, `status`, `run`, `install`, `validate`, `dump`, `migrate`, `jobs`)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `daft hooks jobs [--format <FMT>] [logs\|cancel\|retry\|prune [--dry-run] [--older-than <DUR>]]` | Manage background hook jobs: list, view logs, cancel, retry, prune old records. Listing includes a `Size` column (and `size_bytes` JSON field) for `output.log`. `prune` removes whole invocation records (invocation.json + per-job metadata + logs) past retention; supports `--dry-run` (preview) and `--older-than <DUR>` (override retention). An automatic background cleanup (`daft __clean-logs`) runs at most once every 24h, auto-disabled in CI; opt out with `DAFT_NO_LOG_CLEAN=1`. Use `--format json` (or `ndjson`/`tsv`/`csv`/`yaml`/`toon`/`markdown`) for machine-readable output; the listing is a flat table with one row per job carrying invocation context (`invocation_id`, `invocation_short`, `worktree`, `hook_type`, `trigger_command`, `invocation_created_at`).                                                                                                                                                                                                                                                                                                                                                                                     |
| `daft doctor`                                                                                    | Diagnose installation and configuration issues; `--fix` auto-repairs symlinks, shortcuts, refspecs, hooks; `--fix --dry-run` previews fixes. The Repository section's `Config` check reports the main `daft.yml`'s status (tracked team baseline / visitor / none) repo-awarely — consistently from a worktree, a worktree subdir, or the bare container root of a contained layout.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `daft activate shortcuts <subcommand>`                                                           | Manage command shortcut symlinks                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `daft repo install [--quiet\|-q] [--verbose\|-v] [--git-exclude]`                                | Write a starter `daft.yml` (commented skeleton: `hooks:`, `shared:`, `layout:`) at the worktree root, modeled on `lefthook install`. Repo-aware: from a worktree subdir it targets the worktree root; at the bare container root of a contained layout it installs across the repo's worktrees — writing the starter into the default worktree and copying it into the others, like a multi-branch `daft clone --install` (never a stray file at the inert container root); it refuses (non-zero) only outside a git repository; if a `daft.yml` already exists it reports whether that file is tracked (team baseline) or a visitor config (untracked) and stops cleanly (exit 0) without modifying it. Then, if git doesn't already ignore it, offers to add `/daft.yml` to `.git/info/exclude` (local, never committed) so a visitor config stays private — prompted on a TTY (default No), `--git-exclude` adds it without prompting (takes precedence over `--quiet`), non-interactive prints a hint; without `--git-exclude`, `--quiet` skips the check. Never touches the tracked `.gitignore`. Canonical name; `daft install` is a top-level alias for the same command. |
| `daft repo remove [<path>] [--force\|-y] [--dry-run] [-v]`                                       | Remove a Git repository entirely: git dir, every checked-out worktree, and the trust marker. Runs `worktree-pre-remove` and `worktree-post-remove` lifecycle hooks per worktree. Prompts before deletion unless `--force`. Refuses paths that are not inside a Git repository. Path defaults to cwd. Note: `worktree-post-remove` fires AFTER the worktree directory is gone — `$DAFT_WORKTREE_PATH` no longer exists at that point.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `daft shell-init <shell>`                                                                        | Generate shell integration wrappers                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `daft completions <shell>`                                                                       | Generate shell tab completions                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |

All worktree commands can be run from **any directory** within any worktree.
They find the project root automatically via `git rev-parse --git-common-dir`.

### Ad-hoc Commands vs Hooks

For ad-hoc commands across worktrees (without creating a hook), use
`daft worktree-exec`. For recurring per-worktree automation, use `daft.yml`
hooks.

### Post-setup Command Execution (`-x`/`--exec`)

The `clone`, `init`, and `checkout` commands support `-x`/`--exec` to run
commands in the new worktree immediately after setup:

```bash
daft worktree-clone https://github.com/org/repo -x 'mise install' -x 'npm run dev'
daft worktree-checkout -b my-feature -x claude
daft worktree-init my-project -x 'echo "ready"'
```

The option is repeatable -- commands run sequentially in the worktree directory,
after hooks complete. Execution stops on first failure. Interactive programs
(claude, vim) work because stdio is fully inherited.

### Cross-worktree Merges (`daft merge`)

`daft merge` (= `daft worktree-merge`) performs `git merge` without forcing you
to `git switch` into the target branch first. Use it whenever you would
otherwise cd into the target worktree, run `git merge`, and cd back.

```bash
# Merge feature/api into the current worktree's branch (like `git merge`)
# Default style is always-merge-commit (--no-ff). Pass --no-edit to skip the editor.
daft merge feature/api --no-edit

# Merge into another worktree from anywhere — shell stays put
daft merge feature/api --into main --no-edit

# Octopus: merge multiple sources into the target in one commit
daft merge feature/a feature/b feature/c --into main --no-edit

# Squash: all source commits become one commit on the target.
# An editor opens pre-populated with the squash message by default.
daft merge --squash feature/api
# Skip the editor:
daft merge --squash --no-edit feature/api
# Squash + full cleanup in one shot (removes worktree + branch):
daft merge --squash -r feature/done --into main --no-edit

# Rebase: rebase source onto target, then fast-forward (linear history)
daft merge --rebase feature/api --into main

# Rebase-merge: rebase source onto target, then create a merge commit
daft merge --rebase-merge feature/api --into main --no-edit

# Strategy flags
daft merge -s ours --into release feature/old --no-edit

# Cleanup on success: remove source worktree and branch with -r
daft merge feature/done --into main -r --no-edit

# Persist your preferred style + cleanup to git config for future merges
daft merge feature/done --into main -r --squash --set-default --no-edit

# Ephemeral target worktree: merge into a branch that has no worktree
daft merge feature/hotfix --into release/1.2 --adopt-target --no-edit
# Or auto-accept all prompts (CI-friendly):
daft merge feature/hotfix --into release/1.2 -y --no-edit
```

**When to reach for it:**

- Landing a feature branch into `main` (or another long-lived branch) while you
  keep working in the feature worktree.
- Octopus merges across many worktrees without context-switching.
- Scripting merges (`-y` auto-accepts prompts; `--adopt-target` /
  `--no-adopt-target` make adoption explicit).
- Tidy up after a merge (`-r` removes the source worktree and branch).
- Persisting workflow preferences with `--set-default`.

**Common pitfalls to communicate to the user:**

- **Default style is always-merge-commit.** Unlike plain `git merge` which
  fast-forwards when possible, `daft merge` always creates a merge commit. Use
  `--rebase` for linear (FF) history. Always pass `--no-edit` in CI or non-TTY
  contexts to avoid an editor prompt.
- **`--squash` always commits by default.** An editor opens pre-populated with
  the squash message. Pass `--no-edit` to use the message verbatim, or `-m` to
  supply your own. Use `--no-commit` to stage without committing (incompatible
  with `-r`). Without a TTY and without `--no-edit`/`-m`, daft refuses before
  merging.
- **Target must be clean.** By default `daft.merge.requireCleanTarget=true`
  refuses to merge when the target worktree has uncommitted changes. Ask the
  user to commit, stash, or carry those changes first (`daft carry <target>` is
  a natural fit).
- **Conflicts do not hijack the shell.** On conflict, daft reports the
  conflicted files and the exact `--continue` / `--abort` command — your shell
  stays where it was. Resolve in the target worktree, `git add`, then run
  `daft merge --continue [<target>]`. To bail out, run
  `daft merge --abort [<target>]`.
- **Squash-staged state.** If the squash commit editor is closed without saving,
  the squash changes remain staged (squash-staged state). Use
  `daft merge --continue` to re-open the editor, or `daft merge --abort` to
  reset the index.
- **Octopus aborts on conflict.** Multi-source merges are all-or-nothing;
  there's no mid-flight resolution.
- **`-r` removes both worktree and branch.** For regular merges, uses
  `git branch -d` (safe) semantics. For squash + commit, uses `branch -D` — safe
  because daft has content-equivalence proof. If the source branch moved during
  the editor session, cleanup is refused and a hint is shown.
- **Ephemeral target behavior.** With no worktree for the target, the default is
  to prompt. `--adopt-target` accepts without asking; `--no-adopt-target`
  refuses. Configure the default with `daft.merge.adoptTargetOnDemand` =
  `prompt` | `yes` | `no`.

`pre-merge` and `post-merge` hooks (see the Hooks section below) fire around the
merge with `DAFT_MERGE_*` env vars describing sources, target, mode, strategy,
and result.

## Shell Integration

Shell integration is important because the daft binary creates worktrees
internally, but the parent shell stays in the original directory. Shell wrappers
solve this by reading the CD target from a temp file (`DAFT_CD_FILE`) and
running `cd` in the parent shell.

```bash
# Bash / Zsh -- add to ~/.bashrc or ~/.zshrc
eval "$(daft shell-init bash)"

# Fish -- add to ~/.config/fish/config.fish
daft shell-init fish | source

# With short aliases (gwco, gwcob, gwcobd) -- gwcob maps to checkout -b
eval "$(daft shell-init bash --aliases)"
```

Disable auto-cd per-command with `--no-cd` or globally with
`git config daft.autocd false`.

## Hooks System (daft.yml)

Hooks automate worktree lifecycle events. The recommended approach is a
`daft.yml` file at the repository root.

### Hook Types

| Hook                   | Trigger                                         | Runs From                   |
| ---------------------- | ----------------------------------------------- | --------------------------- |
| `post-clone`           | After `daft worktree-clone`                     | New default branch worktree |
| `worktree-pre-create`  | Before new worktree is added                    | Source worktree             |
| `worktree-post-create` | After new worktree is created                   | New worktree                |
| `worktree-pre-remove`  | Before worktree is removed                      | Worktree being removed      |
| `worktree-post-remove` | After worktree is removed                       | Current worktree            |
| `pre-merge`            | After pre-flight checks, before the merge runs  | Target worktree             |
| `post-merge`           | After the merge completes (success or conflict) | Target worktree             |

`worktree-pre-remove` and `worktree-post-remove` also fire when `daft merge -r`
cleans up a source worktree after a successful merge. In that context
`DAFT_COMMAND=merge` (not `branch-delete`), so scripts can distinguish merge
cleanup from a standalone `daft remove`.

During `daft worktree-clone`, hooks fire in this order: `post-clone` first
(one-time repo bootstrap), then `worktree-post-create` (per-worktree setup).
This lets `post-clone` install foundational tools that `worktree-post-create`
may depend on.

`pre-merge` aborts the merge on failure (default fail mode: `abort`);
`post-merge` logs warnings on failure but never rolls back the merge (default:
`warn`). Both expose `DAFT_MERGE_*` env vars: `SOURCES`, `TARGET_BRANCH`,
`TARGET_PATH`, `MODE` (`merge`/`ff`/`squash`/`octopus`), `STRATEGY`,
`EPHEMERAL`, `CROSS_WORKTREE`, `SOURCE_SHAS` (space-separated SHA list of source
tips captured before merge). `post-merge` additionally gets `RESULT`
(`success`/`conflict`/`already-up-to-date`/`aborted`), `COMMIT_SHA`,
`CONFLICTED_FILES` (newline-separated), and `PROMOTED_FROM_EPHEMERAL`.
`RESULT=aborted` fires when a squash commit is abandoned (editor closed without
saving, pre-commit hook fail, GPG-sign fail); `COMMIT_SHA` is empty in this
case. Neither hook fires when the merge is a no-op (already up to date).

### daft.yml Format

```yaml
min_version: "1.5.0" # Optional: minimum daft version
hooks:
  worktree-post-create:
    parallel: true # Run jobs concurrently (default)
    jobs:
      - name: install-deps
        run: npm install
      - name: setup-env
        run: cp .env.example .env
```

### Config File Locations (first match wins)

`daft.yml`, `daft.yaml`, `.daft.yml`, `.daft.yaml`, `.config/daft.yml`,
`.config/daft.yaml`

Additionally: `daft.local.yml` for machine-specific overrides (not committed),
and per-hook files like `worktree-post-create.yml`. The deprecated name
`daft-local.yml` is still accepted for one release cycle but generates a warning
and a doctor notice; prefer `daft.local.yml`.

### Execution Modes

Set one per hook (default is `parallel`):

| Mode     | Field            | Behavior                          |
| -------- | ---------------- | --------------------------------- |
| Parallel | `parallel: true` | All jobs run concurrently         |
| Piped    | `piped: true`    | Sequential; stop on first failure |
| Follow   | `follow: true`   | Sequential; continue on failure   |

### Job Fields

```yaml
- name: job-name # Display name and dependency reference
  description: "Install npm dependencies" # Human-readable description
  run: "npm install" # Inline command (or use script: "setup.sh")
  runner: "bash" # Interpreter for script files
  root: "frontend" # Working directory relative to worktree
  env: # Extra environment variables
    NODE_ENV: development
  tags: ["build"] # Tags for filtering
  skip: CI # Skip when $CI is set
  only: DEPLOY_ENABLED # Only run when $DEPLOY_ENABLED is set
  os: linux # Target OS: macos, linux, windows (or list)
  arch: x86_64 # Target arch: x86_64, aarch64 (or list)
  needs: [install-npm] # Wait for these jobs to complete first
  tracks: [path, branch] # Worktree attributes this job depends on (for move hooks)
  interactive: true # Needs TTY (forces sequential)
  priority: 1 # Lower runs first
  fail_text: "Setup failed" # Custom failure message
  background: true # Run in the background (non-blocking)
  background_output: log # "log" (default) or "silent"
  log: # Log configuration
    retention: "7d" # How long to keep logs
    path: "./logs/job.log" # Custom log path (absolute or relative)
```

### Job Dependencies

```yaml
hooks:
  worktree-post-create:
    jobs:
      - name: install-npm
        run: npm install
      - name: install-pip
        run: pip install -r requirements.txt
      - name: build
        run: npm run build
        needs: [install-npm]
      - name: test
        run: npm test
        needs: [build, install-pip]
```

Independent jobs (`install-npm`, `install-pip`) run in parallel. Dependent jobs
wait for their dependencies.

### Background Jobs

Jobs with `background: true` run asynchronously after the command returns, so
the user can start working while long-running tasks complete. A coordinator
process manages background jobs and writes output to log files.

```yaml
hooks:
  worktree-post-create:
    jobs:
      - name: install deps
        run: pnpm install
      - name: warm build cache
        run: cargo build
        background: true
        needs: [install deps]
```

Key behaviors:

- Background jobs participate in the DAG. If a foreground job depends on a
  background job, the background job is promoted to foreground automatically.
- `needs:` between background jobs is honored at runtime — the coordinator
  schedules them in topological wave order. A background job with
  `needs: [other-bg-job]` does not start until `other-bg-job` has terminated.
- If a `needs:` dependency fails or is cancelled, the dependent background job
  does not run; it is recorded as `Skipped` in `daft hooks jobs` listings.
- `background: true` can be set at the hook level as a default for all jobs.
- `DAFT_NO_BACKGROUND_JOBS=1` promotes all background jobs to foreground (useful
  in CI or for debugging).
- `daft hooks jobs` lists, cancels, retries, and prunes old background-job
  records.
- When a worktree is removed, running background jobs for it are cancelled.

When generating `daft.yml` configurations, mark jobs as `background: true` when
they warm caches, pre-build, or perform other tasks whose results are not needed
immediately.

### Groups

A job can contain a nested group with its own execution mode:

```yaml
- name: checks
  group:
    parallel: true
    jobs:
      - name: lint
        run: cargo clippy
      - name: format
        run: cargo fmt --check
```

### Template Variables

Available in `run` commands:

| Variable              | Description                              |
| --------------------- | ---------------------------------------- |
| `{branch}`            | Target branch name                       |
| `{worktree_path}`     | Path to the target worktree              |
| `{worktree_root}`     | Project root directory                   |
| `{source_worktree}`   | Path to the source worktree              |
| `{git_dir}`           | Path to the `.git` directory             |
| `{remote}`            | Remote name (usually `origin`)           |
| `{job_name}`          | Name of the current job                  |
| `{base_branch}`       | Base branch (for checkout -b commands)   |
| `{repository_url}`    | Repository URL (for post-clone)          |
| `{default_branch}`    | Default branch name (for post-clone)     |
| `{old_worktree_path}` | Previous worktree path (move hooks only) |
| `{old_branch}`        | Previous branch name (move hooks only)   |

### Skip and Only Conditions

```yaml
skip: CI # Skip when env var is truthy
skip: true # Always skip
skip:
  - merge # Skip during merge
  - rebase # Skip during rebase
  - ref: "release/*" # Skip if branch matches glob
  - env: SKIP_HOOKS # Skip if env var is truthy
  - run: "test -f .skip-hooks" # Skip if command exits 0
    desc: "Skip file exists" # Human-readable reason for the skip

only:
  - env: DEPLOY_ENABLED # Only run when env var is set
  - ref: "main" # Only run on main branch
```

### Trust Management

Hooks from untrusted repos do not run automatically. Manage trust with:

```bash
daft hooks trust        # Allow hooks to run
daft hooks prompt       # Prompt before each execution
daft hooks deny         # Never run hooks (default)
daft hooks status       # Check current trust level
daft hooks install      # Scaffold a daft.yml with placeholders
daft hooks validate     # Validate configuration syntax
daft hooks dump         # Show fully merged configuration
daft hooks run <type>   # Manually run a hook (bypasses trust)
```

When a command skips hooks because the repo is untrusted, it prints one plain
stderr notice — e.g.
`Untrusted repo — 2 daft.yml hooks not run: worktree-pre-create, worktree-post-create`
— naming the skipped hooks and suggesting `daft hooks trust` (suppressed by an
explicit `--skip-hooks`). It is an untagged notice, not a `warning:`, because an
untrusted repo declining to run hooks is by design. Each skip is recorded; a
later `daft hooks trust` lists precise replay commands
(`daft hooks run post-clone` / `daft hooks run worktree-post-create`) for the
worktrees whose setup hooks never ran — run them inside each listed worktree. If
an agent sees that notice, trusting and replaying is the way to get the worktree
into its fully set-up state.

### Manual Hook Execution

Run hooks on demand, bypassing trust checks (the user is explicitly invoking):

```bash
daft hooks run worktree-post-create              # Run all jobs
daft hooks run worktree-post-create --job "mise" # Run a single job
daft hooks run worktree-post-create --tag setup  # Run jobs tagged "setup"
daft hooks run worktree-post-create --dry-run    # Preview without executing
daft hooks run worktree-post-create --verbose    # Show skipped jobs with reasons
```

Use cases: re-running after a failure, iterating during hook development, or
bootstrapping existing worktrees that predate the hooks config.

### Skipping Hooks Per-Invocation (`--skip-hooks`)

The worktree-creating commands (`daft start`, `daft go`,
`git worktree-checkout`, `git worktree-checkout-branch`) and
`git worktree-clone` / `git worktree-flow-adopt` accept `--skip-hooks` to
exclude jobs for one run (repeatable / comma-separated):

```bash
daft start feat/x --skip-hooks all          # skip every hook (replaces the old --no-hooks)
daft start feat/x --skip-hooks worktree-post-create  # skip one whole hook by name
daft start feat/x --skip-hooks lint          # skip the lint job AND its dependents
daft start feat/x --skip-hooks tag:heavy     # skip every heavy-tagged job AND dependents
daft start feat/x --skip-hooks tag:heavy,lint
git worktree-clone <url> --skip-hooks all    # clone without running any hooks
git worktree-clone <url> --skip-hooks post-clone  # clone, run worktree hooks but not post-clone
```

Selectors: `all` / `*` (every job), `<hook>` (a whole hook by its canonical
`daft.yml` key, e.g. `worktree-post-create` / `post-clone`), `tag:<tag>` (tagged
jobs + dependents), `<name>` (a job + its dependents), `job:<name>`
(explicit-name escape hatch). A bare token resolves in order: wildcard → hook
type → job name; tags need the `tag:` prefix. A hook-type selector that names a
hook the command never fires is a silent no-op (no error, no warning).

Key behavior — the **downstream cascade**: skipping a job also skips every job
that `needs:` it (transitively), because running a dependent against a
deliberately-skipped dependency is broken. Upstream dependencies are untouched.
Excluded jobs are reported as skipped with a reason, not dropped silently; a
selector matching nothing warns and the run proceeds.

`--skip-hooks` is the **exclusion** counterpart to `daft hooks run --job/--tag`
(which is an inclusion filter). `--skip-hooks all` cannot be combined with
`--trust-hooks`; a partial skip (`tag:`/`<job>`) still runs your own hooks and
remains compatible with `--trust-hooks`.

### Git pre-push Hooks on daft Pushes

Separate from daft's own hooks: every daft-initiated `git push` honors the
repo's git-level `pre-push` hook (native `.git/hooks` or `core.hooksPath`
managers like lefthook/husky/pre-commit). The hook run is reported as a
`pre-push` phase. A failing hook blocks the push and the command exits non-zero
— any worktree it created or moved is still completed and usable, and the error
names the manual recovery command. Exception: the automatic upstream push on
`daft start`/`go -b` runs the hook only when it introduces new commits;
branching off a fully-pushed base is a ref-only push and skips the hook (config
`daft.checkout.pushVerify`: `auto` default, `always`, `never`). Pass
`--no-verify` to the pushing command (`daft sync --push`, `daft start`/`go -b`,
`daft rename`, `daft remove`, `daft multi-remote move`) to skip the hook for one
invocation. `--skip-hooks` does NOT affect git-level hooks — it only filters
daft's own jobs.

### Move Hooks

When a worktree is moved (rename via `daft worktree-branch -m`, layout transform
via `daft layout transform`, or adopt via `daft worktree-flow-adopt`), daft
replays identity-tracked hooks to tear down the old environment and set up the
new one.

**Flow:** `worktree-pre-remove` (old identity) -> `worktree-post-remove` (old
identity) -> move on disk -> `worktree-pre-create` (new identity) ->
`worktree-post-create` (new identity). Only tracked jobs run.

**`tracks` field:** Declares which attributes a job depends on.

```yaml
- name: link-output
  run: ln -sf {worktree_path}/dist /opt/builds/current
  tracks: [path] # Re-runs when worktree path changes

- name: set-branch-env
  run: echo "BRANCH={branch}" > .env.branch
  tracks: [branch] # Re-runs when branch name changes

- name: install-deps
  run: npm install
  # No tracks -- skipped during moves
```

**Implicit tracking:** If `tracks` is omitted, daft infers it from template
usage -- `{worktree_path}` implies `path`, `{branch}`/`{worktree_branch}`
implies `branch`. Explicit `tracks` overrides inference.

**Dependency pull-in:** Jobs listed in `needs` of a tracked job are included in
the move even if not tracked themselves.

**Failure handling:** Hook failures during moves produce warnings, not errors.
The move always completes.

**Move-only template variables:** `{old_worktree_path}`, `{old_branch}` --
available only when `DAFT_IS_MOVE` is `true`.

**Move-only environment variables:** `DAFT_IS_MOVE` (`true` during move hooks),
`DAFT_OLD_WORKTREE_PATH`, `DAFT_OLD_BRANCH_NAME`.

### Environment Variables in Hooks

All hooks receive: `DAFT_HOOK`, `DAFT_COMMAND`, `DAFT_PROJECT_ROOT`,
`DAFT_GIT_DIR`, `DAFT_REMOTE`, `DAFT_SOURCE_WORKTREE`.

Worktree hooks add: `DAFT_WORKTREE_PATH`, `DAFT_BRANCH_NAME`.

Creation hooks add: `DAFT_IS_NEW_BRANCH`, `DAFT_BASE_BRANCH`.

Clone hooks add: `DAFT_REPOSITORY_URL`, `DAFT_DEFAULT_BRANCH`.

Removal hooks add: `DAFT_REMOVAL_REASON` (`remote-deleted`, `manual`, or
`ejecting`).

Move hooks add: `DAFT_IS_MOVE`, `DAFT_OLD_WORKTREE_PATH`,
`DAFT_OLD_BRANCH_NAME`.

## Environment Tool Detection and Setup

When working in a daft repo, detect environment tools by their marker files and
suggest `daft.yml` hooks to automate setup for new worktrees.

### Tool Detection Table

| Tool       | Marker Files                                      | Purpose                                                 |
| ---------- | ------------------------------------------------- | ------------------------------------------------------- |
| mise       | `.mise.toml`, `.tool-versions`                    | Polyglot runtime manager (Node, Python, Ruby, Go, etc.) |
| direnv     | `.envrc`                                          | Per-directory environment variables                     |
| nvm / fnm  | `.nvmrc`, `.node-version`                         | Node.js version management                              |
| pyenv      | `.python-version`                                 | Python version management                               |
| rbenv      | `.ruby-version`                                   | Ruby version management                                 |
| Docker     | `Dockerfile`, `docker-compose.yml`, `compose.yml` | Containerized development                               |
| npm        | `package.json`, `package-lock.json`               | Node.js dependencies                                    |
| yarn       | `yarn.lock`                                       | Node.js dependencies (Yarn)                             |
| pnpm       | `pnpm-lock.yaml`                                  | Node.js dependencies (pnpm)                             |
| pip        | `requirements.txt`, `pyproject.toml`, `setup.py`  | Python dependencies                                     |
| Cargo      | `Cargo.toml`, `Cargo.lock`                        | Rust dependencies                                       |
| Go modules | `go.mod`, `go.sum`                                | Go dependencies                                         |
| Bundler    | `Gemfile`, `Gemfile.lock`                         | Ruby dependencies                                       |
| Composer   | `composer.json`, `composer.lock`                  | PHP dependencies                                        |

### Hook Jobs for Detected Tools

**mise** (if `.mise.toml` or `.tool-versions` exists):

```yaml
- name: mise-install
  run: mise install
```

**direnv** (if `.envrc` exists):

```yaml
- name: direnv-allow
  run: direnv allow .
```

**nvm** (if `.nvmrc` or `.node-version` exists):

```yaml
- name: nvm-use
  run: |
    export NVM_DIR="$HOME/.nvm"
    [ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh"
    nvm install
```

**npm** (if `package.json` exists):

```yaml
- name: npm-install
  run: npm install
```

**pip with venv** (if `requirements.txt` exists):

```yaml
- name: pip-install
  run: |
    python -m venv .venv
    . .venv/bin/activate
    pip install -r requirements.txt
```

**Cargo** (if `Cargo.toml` exists):

```yaml
- name: cargo-build
  run: cargo build
```

### Comprehensive Example

For a project using mise, direnv, and npm:

```yaml
hooks:
  post-clone:
    piped: true
    jobs:
      - name: mise-install
        run: mise install
      - name: direnv-allow
        run: direnv allow .
      - name: npm-install
        run: npm install
        needs: [mise-install]
      - name: setup-env
        run: cp .env.example .env

  worktree-post-create:
    jobs:
      - name: mise-install
        run: mise install
      - name: direnv-allow
        run: direnv allow .
      - name: npm-install
        run: npm install
        needs: [mise-install]
```

## Suggesting Tooling for Repos That Lack It

When a daft-managed repo has no `daft.yml`, check for dependency files and
suggest creating one. A `worktree-post-create` hook that installs dependencies
is the most impactful automation to add.

### Starter Configurations

**Node.js project** (detected via `package.json`):

```yaml
hooks:
  worktree-post-create:
    jobs:
      - name: install-deps
        run: npm install
```

**Python project** (detected via `requirements.txt` or `pyproject.toml`):

```yaml
hooks:
  worktree-post-create:
    jobs:
      - name: install-deps
        run: |
          python -m venv .venv
          . .venv/bin/activate
          pip install -r requirements.txt
```

**Rust project** (detected via `Cargo.toml`):

```yaml
hooks:
  worktree-post-create:
    jobs:
      - name: build
        run: cargo build
```

**Go project** (detected via `go.mod`):

```yaml
hooks:
  worktree-post-create:
    jobs:
      - name: download-deps
        run: go mod download
```

When suggesting `daft.yml`, also remind the user to trust the repo:
`daft hooks trust`.

## Workflow Guidance for Agents

When working in a daft-managed repository, apply these translations:

| User intent                       | Correct daft approach                                                                              |
| --------------------------------- | -------------------------------------------------------------------------------------------------- |
| "Create a branch"                 | `daft worktree-checkout -b <name>` -- creates branch + worktree + pushes                           |
| "Branch from main"                | `daft worktree-checkout -b <name> main` -- branches from the specified base                        |
| "Switch to branch X"              | Navigate to the worktree directory: `cd ../X/`                                                     |
| "Go back"                         | `daft worktree-checkout -- -` -- toggles to the previous worktree                                  |
| "Check out a PR"                  | `daft worktree-checkout <branch>` -- creates worktree for existing branch                          |
| "Delete a branch"                 | `daft worktree-branch -d <branch>` -- removes worktree, local branch, and remote tracking branch   |
| "Clean up branches"               | `daft worktree-prune` -- removes worktrees for deleted remote branches                             |
| "Wrong branch"                    | `daft worktree-carry <correct-branch>` -- moves uncommitted changes                                |
| "Update from remote"              | `daft worktree-fetch` -- updates current or specified worktrees (use source:dest for cross-branch) |
| "Run my build on these worktrees" | `daft worktree-exec feat/a feat/b -- <cmd>` or `daft exec --all -- <cmd>` for every worktree       |
| "Adopt existing repo"             | `daft worktree-flow-adopt` -- converts traditional repo to daft layout                             |

### Per-worktree Isolation

Each worktree has its **own** `node_modules/`, `.venv/`, `target/`, etc. When a
new worktree is created without `daft.yml` hooks, dependencies are not installed
automatically. If the user creates a new worktree and encounters
missing-dependency errors, the fix is to run the appropriate install command in
that worktree (e.g., `npm install`, `pip install -r requirements.txt`).

### Navigating Worktrees

From any worktree, sibling worktrees are at `../<branch-name>/`. The project
root (containing `.git/`) is at `..` relative to any top-level worktree. Use
`git rev-parse --git-common-dir` to programmatically find the root.

### Modifying Shared Files

`.gitignore` and CI configuration live in each worktree independently (they are
part of the Git-tracked content). Changes to these files in one worktree must be
committed and merged to propagate to other worktrees.

`daft.yml` is different: its propagation behavior depends on its git tracking
status. When `daft.yml` is tracked (committed), changes propagate via git like
any other file. When `daft.yml` is untracked — a **visitor configuration** —
daft propagates it automatically through the three events below, so git is never
involved.

#### Visitor configuration (untracked `daft.yml`)

`classify_main_config(worktree_root)` distinguishes three states: `Tracked`,
`Visitor` (untracked), and `Missing`. The classification runs
`git ls-files --error-unmatch` against the resolved config path; if git cannot
answer (not a repo, binary missing), the fallback is `Tracked` to avoid
surprising the user with implicit visitor behavior.

**Propagation contract (seed provenance).** Every time daft writes an untracked
daft file into a worktree — branch-out propagation, starter installs,
consolidation — it records the written content as that worktree's **seed** in
the per-repo SQLite store. At every later lifecycle point the on-disk copy is
classified against its seed: **pristine** (byte-identical — nobody touched it),
**refined** (edited since seeding — real user data), or **no-seed**
(pre-provenance worktree or hand-authored file; treated like refined). A refined
copy whose content the target already covers counts as _subsumed_ and behaves
like pristine. The rules per command:

1. **Branch-out** (worktree create). Before `worktree-post-create` hooks fire,
   daft copies in-scope untracked daft files verbatim from the source worktree
   into the new worktree and seeds them.

2. **`daft merge`**. A pristine/subsumed source copy is skipped outright — the
   target's config is never touched by a stale snapshot. A refined source is
   merged **three-way** (seed as base) into the target before the git merge
   (atomic: the target's files are restored if the merge fails), and the adopted
   key paths are announced. Keys changed on both sides are conflicts: daft
   prompts for a side interactively and aborts before the git merge in
   non-interactive runs, pointing at `daft file merge`.

3. **Worktree removal** (`daft remove`, branch-delete). Pristine/subsumed copies
   are deleted silently with the worktree. Refined copies stop the removal:
   interactively daft offers consolidate / discard / abort (with a key-level
   summary); non-interactively it refuses and suggests `daft file merge`.
   Forcing (`-f`/`-D`) means **discard** — the files are stashed under
   `<git-common-dir>/.daft/discarded/<branch>/` and the target worktree is never
   written.

4. **`daft prune` / `daft sync`**. Batch commands never prompt: pristine copies
   prune cleanly, refined ones keep their worktree with an end-of-run summary
   pointing at `daft file merge`; `--force` discards to the stash. Prune
   additionally verifies the branch is actually merged (ancestor or squash)
   before deleting anything — a deleted remote alone no longer destroys local
   state; gone-but-unmerged branches are kept unless forced.

The default-branch worktree's config is only ever written by an announced
`daft merge` consolidation or an explicit `daft file merge` — never by a removal
path.

**Collision (visitor `daft.yml` meets an incoming tracked `daft.yml`)** is
deferred to the `daft pull` command (issue #493). Doctor surfaces the
classification status as informational so the user can act before a collision
occurs.

#### `daft file merge` — on-disk config merge

`daft file merge <TARGET> <SOURCE>` (or collapsed: `daft file merge <SOURCE>`)
consolidates one daft file into another. When the source is a worktree-root daft
file with seed provenance, the merge is **three-way** against the seed: only
genuine refinements move, a key-level preview prints first ("will adopt: …",
"conflicting keys: …"), the target is backed up to
`<git-common-dir>/.daft/backups/file-merge/` before writing, and conflicting
keys require a side — an interactive prompt, `-y` for source-wins, or a non-zero
abort listing the keys in non-interactive runs. A source that adds nothing
reports "nothing to adopt" and leaves the target untouched. Without provenance
the legacy two-way merge applies (source wins on conflicts), guarded by an
untracked-target confirmation (`--yes`/`--force` bypasses). After a successful
merge the source file is deleted unless `--keep-source` is passed (a kept source
is re-seeded as consolidated). Use `daft file merge` to consolidate visitor
configs before removing a worktree or to promote a visitor config to a team
baseline.

#### `daft repo install` — bootstrap a visitor configuration

`daft repo install` creates a starter `daft.yml` at the worktree root (commented
skeleton with `hooks:`, `shared:`, `layout:` sections). Because `daft.yml` is a
per-worktree file, install is repo-aware: run from a worktree subdirectory it
targets the worktree root; run at the bare container root of a contained layout
it installs across the repo's worktrees — writing the starter into the default
worktree and copying it into the others, like a multi-branch
`daft clone --install` — and never leaves a stray file at the inert container
root. It refuses (non-zero) only when run outside a git repository. If a
`daft.yml` already exists it does not hard-error: it reports whether that file
is tracked (a committed team baseline) or a visitor config (untracked, private
to this clone) and stops, suggesting `daft.local.yml` for personal overrides.

After writing the file, if git doesn't already ignore it, daft offers to add
`/daft.yml` to `.git/info/exclude` — a local, per-clone exclude that is never
committed, keeping a visitor config invisible to teammates. On a TTY it prompts
(default No); `--git-exclude` adds the entry without prompting (it takes
precedence over `--quiet`, adding silently); non-interactive runs (scripts,
hooks, CI) change nothing and print a copy-pasteable hint; without
`--git-exclude`, `--quiet` skips the check entirely. daft never touches the
tracked `.gitignore` — for a team baseline you commit `daft.yml` instead of
excluding it.

`daft repo install` is the canonical name; `daft install` is a top-level alias
that runs the exact same command (kept so lefthook-style discovery works). Use
whichever reads better in context.

## Shortcuts

daft supports three shortcut styles as symlink aliases for faster terminal use:

| Style         | Shortcuts                                                                                                | Example              |
| ------------- | -------------------------------------------------------------------------------------------------------- | -------------------- |
| Git (default) | `gwtclone`, `gwtinit`, `gwtco`, `gwtcb`, `gwtbd`, `gwtprune`, `gwtcarry`, `gwtfetch`, `gwtls`, `gwtsync` | `gwtco feature/auth` |
| Shell         | `gwco`, `gwcob`                                                                                          | `gwco feature/auth`  |
| Legacy        | `gclone`, `gcw`, `gcbw`, `gprune`                                                                        | `gcw feature/auth`   |

The `gwtcb`, `gwcob`, and `gcbw` shortcuts map to `git-worktree-checkout -b`
(branch creation mode).

Default-branch shortcuts (`gwtcm`, `gwtcbm`, `gwcobd`, `gcbdw`) are available
via shell integration only (`daft shell-init`). They resolve the remote's
default branch dynamically and use `git-worktree-checkout -b`.

Shell integration also provides `gwtrn` (maps to `daft rename`) and `gwtsync`
(maps to `git-worktree-sync`) as shell functions with cd behavior.

Manage with `daft activate shortcuts list`, `enable <style>`, `disable <style>`,
`only <style>`.

When a user asks how to use daft more efficiently, mention shortcuts as a
convenience option. Agents should never execute shortcuts directly -- always use
the `daft` binary form (see [Invocation Forms](#invocation-forms)).

## Configuration Reference

Key `git config` settings:

**Local-first defaults**: daft does not contact the remote by default. Remote
operations are opt-in via `daft config remote-sync` or the individual settings
below. Use `daft config remote-sync --on` to re-enable all remote operations at
once, or `--local` on any command to suppress remote operations for a single
invocation.

| Key                         | Default               | Description                                                                                                                                      |
| --------------------------- | --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `daft.autocd`               | `true`                | CD into new worktrees via shell wrappers                                                                                                         |
| `daft.remote`               | `"origin"`            | Default remote name                                                                                                                              |
| `daft.checkout.fetch`       | `false`               | Fetch from remote before checking out an existing branch                                                                                         |
| `daft.checkout.push`        | `false`               | Push new branches to remote after creation                                                                                                       |
| `daft.branchDelete.remote`  | `false`               | Delete the remote branch when removing a local branch                                                                                            |
| `daft.checkout.upstream`    | `true`                | Set upstream tracking                                                                                                                            |
| `daft.checkout.carry`       | `false`               | Carry uncommitted changes on checkout                                                                                                            |
| `daft.checkoutBranch.carry` | `true`                | Carry uncommitted changes on branch creation                                                                                                     |
| `daft.update.args`          | `"--ff-only"`         | Default pull arguments for update (same-branch mode)                                                                                             |
| `daft.prune.cdTarget`       | `"root"`              | Where to cd after pruning (`root` or `default-branch`)                                                                                           |
| `daft.list.stat`            | `"summary"`           | Statistics mode for list (`summary` or `lines`)                                                                                                  |
| `daft.sync.stat`            | `"summary"`           | Statistics mode for sync (`summary` or `lines`)                                                                                                  |
| `daft.prune.stat`           | `"summary"`           | Statistics mode for prune (`summary` or `lines`)                                                                                                 |
| `daft.list.columns`         | (all columns)         | Default columns for `daft list` (same syntax as `--columns`)                                                                                     |
| `daft.sync.columns`         | (all columns)         | Default columns for `daft sync` summary table                                                                                                    |
| `daft.prune.columns`        | (all columns)         | Default columns for `daft prune` summary table                                                                                                   |
| `daft.list.sort`            | `"+branch"`           | Default sort order for `daft list` (same syntax as `--sort`)                                                                                     |
| `daft.sync.sort`            | `"+branch"`           | Default sort order for `daft sync`                                                                                                               |
| `daft.prune.sort`           | `"+branch"`           | Default sort order for `daft prune`                                                                                                              |
| `daft.go.autoStart`         | `false`               | Auto-create worktree when branch not found in go                                                                                                 |
| `daft.hooks.enabled`        | `true`                | Master switch for hooks                                                                                                                          |
| `daft.hooks.defaultTrust`   | `"deny"`              | Default trust for unknown repos                                                                                                                  |
| `daft.hooks.timeout`        | `300`                 | Hook timeout in seconds                                                                                                                          |
| `daft.ownership.strategy`   | `"recency-plurality"` | Strategy for deducing branch ownership from `base..branch` commits. Values: `tip`, `any`, `first`, `plurality`, `majority`, `recency-plurality`. |

## Branch ownership

Branch ownership in `list` / `sync` / `prune` is deduced from the `base..branch`
commit range using a user-configurable strategy (git config
`daft.ownership.strategy`, default `recency-plurality`). The Owner column shows
the author name of the resolved owner. `daft sync --rebase`/`--push` only
operate on branches owned by you (matching `user.email`). Use
`--include <email>` / `--include unowned` to override.

Available strategies:

- **`tip`** — author of the newest commit. Simple, but flips on any drive-by
  commit.
- **`any`** — you own the branch if any commit in range is yours.
- **`first`** — author of the oldest commit ("who started this branch").
- **`plurality`** — author with the most commits in range.
- **`majority`** — author with strictly more than 50% of commits; no owner
  otherwise.
- **`recency-plurality`** (default) — highest recency-weighted score; each
  commit at rank `k` from tip contributes weight `1/(k+1)`. Robust to drive-by
  commits while still favoring recent work.

JSON `"owner"` field is `{name, email}` or `null` (breaking change: the field
previously carried just the tip-author email string).

## Column Selection (`--columns`)

The `list`, `sync`, and `prune` commands support a `--columns` flag to control
which columns appear in the output table and in what order.

### Valid column names

**Default columns** (shown unless removed): `annotation`, `branch`, `path`,
`base`, `changes`, `remote`, `age`, `owner`, `last-commit`

**Optional columns** (must be explicitly added): `size`, `hash`

The `size` column shows the disk size of each worktree folder in human-readable
format (e.g. `42K`, `1.3M`, `2.5G`). When visible, a summary footer row displays
the total size across all worktrees.

The `hash` column shows the abbreviated (7-char) commit hash of each worktree's
HEAD commit.

### Two modes

**Replace mode** — provide an exact comma-separated list; only those columns
appear, in that order:

```bash
daft list --columns branch,path,age
daft sync --columns branch,path,status,age   # status is always pinned on sync/prune
```

**Modifier mode** — prefix columns with `+` (add) or `-` (remove) to adjust the
defaults:

```bash
daft list --columns -annotation,-last-commit   # remove two columns
daft list --columns +base,-age                 # add base, remove age
daft list --columns +size                      # add optional size column
```

Modifier mode is detected automatically when every entry starts with `+` or `-`.

### Pinned columns on sync and prune

The `status` column (showing pruned/updated/skipped) is always displayed on
`sync` and `prune` and cannot be controlled via `--columns`.

### Persistent defaults via git config

Set a default so you never need to pass the flag manually:

```bash
git config daft.list.columns "branch,path,age"
git config daft.sync.columns "-annotation,-last-commit"
git config daft.prune.columns "branch,path,age"
```

The `--columns` flag overrides the git config value for that invocation.

## Sorting (`--sort`)

The `list`, `sync`, and `prune` commands support a `--sort` flag to control the
sort order of the output.

### Sortable columns

`branch`, `path`, `size`, `age`, `owner`, `hash`, `activity`, `commit`

`activity` considers both committed and uncommitted file changes (working tree
mtime). `commit` (alias: `last-commit`) sorts by last commit time only.

### Syntax

Prefix with `+` (ascending, the default) or `-` (descending). Multiple columns
can be comma-separated for multi-level sort:

```bash
daft list --sort branch            # ascending by branch name (default)
daft list --sort -activity         # most recent activity first (commits + uncommitted)
daft list --sort -commit           # most recent commit first (ignores uncommitted)
daft list --sort +owner,-size      # by owner ascending, then size descending
```

You can sort by columns not shown in the output (e.g., `--sort -size` without
`--columns +size`). The sort data is collected automatically.

### Persistent defaults via git config

```bash
git config daft.list.sort "-activity"
git config daft.sync.sort "+owner,-size"
git config daft.prune.sort "+branch"
```

The `--sort` flag overrides the git config value for that invocation.

## Structured output

Seven daft commands emit machine-readable output via `--format`:

- Flat-list: `list`, `hooks trust list`, `layout list`
- Document: `release-notes`
- Matrix: `shared status`
- Sectioned: `multi-remote status`, `hooks run` (listing mode)

Valid formats: `json`, `ndjson`, `tsv`, `csv`, `yaml`, `toon`, `markdown`.
Unsupported combinations print a clear error listing the supported set.

Use `--template '<tera-template>'` for custom output. Tera syntax: `{{ var }}`,
`{% for x in items %}...{% endfor %}`, `{% if %}...{% endif %}`.

## Cache files

`daft list` writes content-addressed JSON caches under
`<git-common-dir>/.daft/cache/<kind>/` so warm-cache runs avoid re-forking slow
git commands. Each entry's filename embeds the SHAs that fully define its inputs
(e.g. `<base_sha>-<head_sha>.json` for ahead/behind counts), so a cache hit is
provably correct — there is no TTL or manual invalidation. The cache is safe to
delete at any time; daft will re-populate it on the next run.

Cached cells: base/remote ahead-behind, base/remote line stats, last-commit
metadata. Working-tree-dependent cells (`Changes`, `Size`) are NOT cached
because their inputs cannot be captured as a SHA — they always recompute and
show the `·` skeleton glyph until the result arrives.
