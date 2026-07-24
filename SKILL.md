---
name: daft-worktree-workflow
description:
  Guides the daft worktree workflow for compartmentalized Git development. Use
  when working in daft-managed repositories (repos with a .git/ bare directory
  and branch worktrees as sibling directories), when setting up worktree
  environment isolation, or when users ask about worktree-based workflows.
  Covers daft commands, hooks automation via daft.yml, and environment tooling
  like mise, direnv, nvm, and pyenv.
daft_version: "1.23.1"
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

Creating a worktree is spinning up a new development environment, not just
"checking out a branch". Split that work along two lifecycles:

- **Provision on create.** `daft.yml` lifecycle hooks do finite, idempotent,
  unattended setup — install dependencies, copy env files, configure environment
  tools — so the developer can start working immediately.
- **Serve on demand.** Long-running, attended processes — dev servers,
  `docker compose` stacks, watchers — belong in **tasks**, started explicitly
  with `daft run`. Booting a backend stack in every worktree you only ever read
  wastes resources and invites port collisions.

The same job schema powers both; only the trigger differs (a lifecycle event vs.
an explicit `daft run`). See Tasks (`daft run`) below.

Never use `git checkout` or `git switch` to change branches in a daft-managed
repo. Navigate between worktree directories instead.

## Detecting a daft-Managed Repository

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
```

Key indicators of any daft-managed repository:

- `git rev-parse --git-common-dir` from any worktree finds the shared Git
  directory
- `daft layout show` reports which layout the repo uses
- Contained layout: `.git/` at the project root is a **bare repository**
  (directory, not a file) with branch worktrees as siblings
- Other layouts: the main checkout looks like a normal Git repo, but daft
  manages worktrees elsewhere

If you see any of these patterns, the user is using daft. Apply worktree-aware
guidance throughout the session.

Four built-in layouts control where worktrees are placed:

| Layout        | Template                                                            | Description                           |
| ------------- | ------------------------------------------------------------------- | ------------------------------------- |
| `contained`   | `{{ repo_path }}/{{ branch }}`                                      | Worktrees inside the repo directory   |
| `sibling`     | `{{ repo }}.{{ branch \| sanitize }}`                               | Worktrees next to the repo (default)  |
| `nested`      | `{{ repo }}/.worktrees/{{ branch \| sanitize }}`                    | Worktrees in a hidden subdirectory    |
| `centralized` | `{{ daft_data_dir }}/worktrees/{{ repo }}/{{ branch \| sanitize }}` | Worktrees in a central data directory |

Manage layouts with `daft layout show` / `list` / `transform <layout>` /
`default [layout]`. Layout is selected at clone time by `--layout`, then the
`daft.yml` `layout:` field, then the global default, then the built-in default
(sibling); custom template layouts live in `~/.config/daft/config.toml`.

`daft.yml` can pin the team-recommended layout alongside hooks:

```yaml
layout: contained

hooks:
  worktree-post-create:
    jobs:
      - name: install-deps
        run: npm install
```

## Running daft

### Execution rule

Run and recommend daft commands using the short verbs exactly as written in this
file: `daft go`, `daft start`, `daft clone`, `daft init`, `daft carry`,
`daft exec`, `daft run`, `daft merge`, `daft list`, `daft update`, `daft prune`,
`daft remove`, `daft rename`, `daft sync`, `daft push`, `daft adopt`,
`daft eject`, plus the noun groups `daft hooks ...`, `daft repo ...`,
`daft layout ...`, `daft config ...`, `daft doctor`, and `daft skill ...`.
Invoke the `daft` binary directly.

Never run or emit the alternate spellings some users have configured:
`git worktree-*` subcommands, `git-worktree-*` binaries, long `daft worktree-*`
names, `git daft ...`, or shortcut aliases such as `gwtco`. They depend on
symlinks and shell wrappers that are usually absent from agent shells, and the
daft verbs are the canonical register. This applies to commands you execute and
to commands you write in explanations, docs, scripts, and `daft.yml` suggestions
alike.

### Recognizing user vocabulary

Users may still type those alternate spellings in their own terminals. Translate
what they say into daft verbs; respond and act in daft verbs. It is fine to
acknowledge their form once ("`gwtco` runs `daft go`").

| User says or types                                   | Means                               |
| ---------------------------------------------------- | ----------------------------------- |
| `git worktree-checkout`, `gwtco`, `gwco`, `gcw`      | `daft go`                           |
| `git worktree-checkout -b`, `gwtcb`, `gwcob`, `gcbw` | `daft start`                        |
| `gwtcm`, `gwtcbm`, `gwcobd`, `gcbdw`                 | `daft start` off the default branch |
| `git worktree-clone`, `gwtclone`, `gclone`           | `daft clone`                        |
| `git worktree-init`, `gwtinit`                       | `daft init`                         |
| `git worktree-carry`, `gwtcarry`                     | `daft carry`                        |
| `git worktree-exec`                                  | `daft exec`                         |
| `git worktree-merge`                                 | `daft merge`                        |
| `git worktree-list`, `gwtls`                         | `daft list`                         |
| `git worktree-fetch`, `gwtfetch`                     | `daft update`                       |
| `git worktree-prune`, `gwtprune`, `gprune`           | `daft prune`                        |
| `git worktree-branch -d` / `-D`, `gwtbd`             | `daft remove` / `daft remove -f`    |
| `git worktree-branch -m`, `gwtrn`                    | `daft rename`                       |
| `git worktree-sync`, `gwtsync`                       | `daft sync`                         |
| `git worktree-push`, `gwtpush`                       | `daft push`                         |
| `git worktree-flow-adopt` / `-eject`                 | `daft adopt` / `daft eject`         |
| `git daft <noun> ...` (e.g. `git daft hooks trust`)  | `daft <noun> ...`                   |

The long `daft worktree-<name>` spellings map the same way. Shortcut aliases are
optional symlinks users manage with `daft activate shortcuts`; never execute
them yourself.

### If a documented command is rejected

If `daft` rejects a command or flag documented here (unknown subcommand,
unexpected argument), do not fall back to raw `git worktree` plumbing.

1. Re-discover the real surface: `daft --help`, then `daft <command> --help`.
2. The installed copy of this skill may be stale relative to the installed
   binary. Refresh it with `daft skill install`, which writes the
   version-matched skill embedded in the `daft` binary (compare `daft --version`
   with `daft_version` in this file's frontmatter).
3. Proceed with the syntax `--help` reports.

### Operating across worktrees: `-C <path>`

Every daft command accepts a top-level `-C <path>` flag that changes the
effective working directory before any path-dependent state is resolved (repo
discovery, layout, hooks, `daft.yml`). Semantics match `git -C`.

```bash
daft -C /path/to/repo list           # equivalent to: cd /path/to/repo && daft list
daft -C /path/to/repo go feature/x   # creates the worktree inside that repo
```

This is the recommended pattern for agents working across multiple worktrees:
each command is self-contained ("do X in path Y") with no `cd` juggling. Rules:
repeated flags compose like `git -C` (`-C /a -C b` means `/a/b` — not "last
wins"); relative arguments resolve against the post-`-C` cwd; `-C` is parsed
only at the front of the argv, so an inner `-C` in a `daft exec` shell command
is preserved.

### daft does not change your shell's directory

The daft binary cannot `cd` the parent shell. After creating a worktree,
navigate to it explicitly — sibling worktrees live at `../<branch>/` relative to
any worktree. (Users with shell integration installed get automatic cd via
`DAFT_CD_FILE` wrappers; agent shells do not have those wrappers.)

When a user asks why their terminal did not follow a new worktree, point them at
shell integration: `eval "$(daft shell-init bash)"` in `~/.bashrc` or
`~/.zshrc`, `daft shell-init fish | source` for fish. Opt out per command with
`--no-cd` or globally with `git config daft.autocd false`. Agents recommend
these lines; they never eval them.

### `daft repo remove` invalidates your cwd

Running `daft repo remove` from inside the repo being deleted invalidates the
agent's cwd mid-operation. Either pass an explicit path
(`daft repo remove /path/to/repo`) and stay outside, or `cd` to a safe ancestor
first. The binary writes a redirect path to `$DAFT_CD_FILE` for shell wrappers,
but agent shells typically lack that wrapper, so follow-up commands fail with
`chdir: no such file or directory` until the cwd is fixed.

## Command Reference

All commands run from any directory inside any worktree; daft finds the project
root via `git rev-parse --git-common-dir`.

### Worktree Lifecycle

| Command                                                                                                                                      | Description                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| -------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `daft clone <url> [--layout <LAYOUT>] [--install [--git-exclude]]`                                                                           | Clone a remote repository into worktree layout. `--install` bootstraps a starter `daft.yml` after cloning (copied into every worktree of a multi-branch clone, implies `--trust-hooks`; skipped if the repo ships a tracked `daft.yml`; rejected with `--no-checkout`).                                                                                                                                                                                                                                 |
| `daft init <name> [--layout <LAYOUT>]`                                                                                                       | Initialize a new local repository in worktree layout                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| `daft go <branch>`                                                                                                                           | Create/enter a worktree for an existing local or remote branch; `--local` skips the remote fetch even when `daft.checkout.fetch` is enabled                                                                                                                                                                                                                                                                                                                                                             |
| `daft go pr:<number>`                                                                                                                        | Check out a GitHub PR or GitLab MR (`mr:<number>`, or a pasted PR/MR URL) into a worktree on its source branch, configured to pull from the PR head. Fork-aware; resolves via the `gh`/`glab` CLI, which must be installed and authenticated (`daft doctor` reports). The platform is detected from the remote (`pr:`/`mr:` are aliases); `daft.forge.platform` overrides for ambiguous remotes. Works cross-repo from anywhere: `daft go <repo> pr:<number>` checks the PR out in that cataloged repo. |
| `daft go -`                                                                                                                                  | Switch to the previous worktree (`cd -` style toggle)                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| `daft go -s <branch>`                                                                                                                        | Same, but auto-creates the branch if not found (also `daft.go.autoStart`)                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `daft start <branch> [base]`                                                                                                                 | Create a new branch and worktree from the current or specified base; does not push by default (`daft.checkout.push`); `--local` skips remote even when push is enabled. A leading cataloged-repo name creates the branch in that repo instead — see the Repo Catalog table.                                                                                                                                                                                                                             |
| `daft remove <branch>`                                                                                                                       | Safely delete a branch: its worktree and local branch ref; the remote branch only when `daft.branchDelete.remote` is enabled; `--local` skips remote, `--remote` deletes only the remote branch                                                                                                                                                                                                                                                                                                         |
| `daft remove -f <branch>`                                                                                                                    | Force-delete bypassing safety checks; for the default branch, removes the worktree only (preserves branch ref and remote)                                                                                                                                                                                                                                                                                                                                                                               |
| `daft prune [-f] [-v\|-vv]`                                                                                                                  | Remove worktrees whose remote branches were deleted AND that are verified merged (ancestor or squash); gone-but-unmerged branches are kept unless forced. `-v` hook details, `-vv` full sequential                                                                                                                                                                                                                                                                                                      |
| `daft carry <targets>`                                                                                                                       | Transfer uncommitted changes to one or more other worktrees                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `daft update [targets]`                                                                                                                      | Update worktree branches from remote; refspec syntax `source:destination` for cross-branch updates                                                                                                                                                                                                                                                                                                                                                                                                      |
| `daft rename <source> <new-branch>`                                                                                                          | Rename a branch, move its worktree, and rename the remote branch                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `daft sync [-f] [--rebase BRANCH [--autostash]] [--push [--force-with-lease] [--no-verify] [--jobs N] [--no-throttle]] [--include VALUE]...` | Prune stale worktrees + update all + optional rebase + optional push. Rebase and push apply only to branches you own by default; `--include` widens (`unowned`, an email, or a branch name). `-f`/`--prune-dirty` includes dirty worktrees. Parallel hook-bearing pushes are memory-governed (`--jobs N` caps concurrency, `--no-throttle` disables). First Ctrl+C cancels gracefully (partial results print, exit 130); a second force-kills.                                                          |
| `daft push [branch] [--no-verify] [--force-with-lease]`                                                                                      | Push one branch with the repo's git `pre-push` hook running in that branch's own worktree — the command's whole point. Defaults to the current branch; targets the branch's own upstream remote, falling back to `daft.remote` (origin) when it has none — and then sets upstream; a branch with no worktree pushes from the current directory. Single-branch by design (use `daft sync --push` for a fleet push).                                                                                      |
| `daft merge ...`                                                                                                                             | Merge branches across worktrees without `git switch` — see Merging Across Worktrees below                                                                                                                                                                                                                                                                                                                                                                                                               |
| `daft adopt [path]`                                                                                                                          | Convert a traditional repository to daft's worktree layout                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `daft eject`                                                                                                                                 | Convert back to a traditional repository layout                                                                                                                                                                                                                                                                                                                                                                                                                                                         |

### Management

| Command                                                                         | Description                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| ------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `daft list [--format <FMT>] [-b\|-r\|-a] [--columns COLS] [--sort COLS]`        | List worktrees: branch (`✦` = default), path, base ahead/behind, file status, remote status, age, owner, commit. In forge repos the default listing also includes a row per open PR (see Machine-Readable Output; `--columns -pr` for worktrees only). `-b`/`-r`/`-a` include local/remote branches without worktrees. Output contract and JSON fields: see Machine-Readable Output.                                                                      |
| `daft exec [TARGETS]... [--all] [-x CMD]... [-- CMD ARGS]...`                   | Run command(s) across worktrees: positional/glob targets or `--all`; `-x` repeatable shell pipelines; trailing `--` for direct argv. Parallel by default (`--sequential`/`--keep-going` for serial); failed worktrees' captured output is dumped after the run, `-v` dumps successful ones too. On an interactive terminal, runs render as a live plan-then-execute rail.                                                                                 |
| `daft run [<task>] [<args>...] [--list] [--job <name>] [--tag <tag>]`           | Run a named task from `daft.yml`'s top-level `tasks:` section in the current worktree; bare `daft run` runs the reserved `run` task, and words after the task name forward to it as arguments (a first word naming no task forwards everything to `run`). Output streams live, there is no execution timeout, and Ctrl+C cancels (twice force-kills). Executes even in an untrusted repo — explicit invocation counts as consent. See Tasks (`daft run`). |
| `daft config remote-sync [--on\|--off\|--status\|--global]`                     | Toggle fetch, push, and remote-delete behavior globally or per-repo; no args opens an interactive TUI                                                                                                                                                                                                                                                                                                                                                     |
| `daft layout [show\|list\|transform\|default]`                                  | Manage worktree layouts                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| `daft hooks <subcommand>`                                                       | Manage hooks trust and configuration (`trust`, `prompt`, `deny`, `status`, `run`, `install`, `validate`, `dump`, `migrate`, `jobs`)                                                                                                                                                                                                                                                                                                                       |
| `daft hooks jobs [logs\|cancel\|retry\|prune [--dry-run] [--older-than <D>]]`   | Manage background hook jobs: list (with a `Size` column), view logs, cancel, retry, prune old records. Automatic cleanup runs at most once every 24h (off in CI; opt out with `DAFT_NO_LOG_CLEAN=1`). JSON shape: see Machine-Readable Output.                                                                                                                                                                                                            |
| `daft doctor`                                                                   | Diagnose installation and configuration issues; `--fix` auto-repairs, `--fix --dry-run` previews. The Repository `Config` check reports the main `daft.yml`'s status (tracked / visitor / none) repo-awarely.                                                                                                                                                                                                                                             |
| `daft skill install [--project\|--dir <path>]`                                  | Install or update this agent skill from the copy embedded in the daft binary (default `~/.claude/skills/`; `--project` targets the worktree's `.claude/skills/`). Re-running updates in place. `daft skill show` prints the embedded skill to stdout.                                                                                                                                                                                                     |
| `daft repo install [--git-exclude]`                                             | Write a starter `daft.yml` at the worktree root — see Bootstrapping a config below. `daft install` is a top-level alias.                                                                                                                                                                                                                                                                                                                                  |
| `daft repo remove [<path>\|--repo <name>] [--keep-files] [--force] [--dry-run]` | Remove a repository entirely: git dir, every worktree, trust marker. Runs `worktree-pre-remove`/`worktree-post-remove` per worktree (`post-remove` fires AFTER the directory is gone). Prompts unless `--force`. `--keep-files` drops the catalog entry only; with `--repo` it also retires a stale entry. Cwd caveat: see Running daft.                                                                                                                  |
| `daft shell-init <shell>`                                                       | Generate shell integration wrappers (auto-cd)                                                                                                                                                                                                                                                                                                                                                                                                             |
| `daft completions <shell>`                                                      | Generate shell tab completions                                                                                                                                                                                                                                                                                                                                                                                                                            |

For ad-hoc commands across worktrees use `daft exec`; for named tasks committed
in `daft.yml` (dev servers, compose stacks) use `daft run`; for recurring
lifecycle automation use `daft.yml` hooks.

### Repo Catalog and the Graph

daft keeps a machine-local **repo catalog** — every repo it touches registers
automatically (clone, init, adopt, or any daft command run inside it). Names
derive from the remote URL; collisions auto-suffix (`api`, `api-2`).

| Command                                                    | Description                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| ---------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `daft repo add [<path>] [--name <name>]`                   | Explicitly register a repo (only needed for repos daft never touched) or rename the current entry. Explicit `--name` collisions error; automatic registration auto-suffixes.                                                                                                                                                                                                                                                                                                                                                                                                 |
| `daft repo list [--all] [--worktrees]`                     | List cataloged repos (name, worktree count, path, remote). `--all` includes removed entries; `--worktrees` expands each repo into a tree; `--columns +size/+layout/+branch` adds columns; `--format json` adds default branch.                                                                                                                                                                                                                                                                                                                                               |
| `daft repo info [<repo>]`                                  | One entry in full: status, path, remote, default branch, layout, worktrees, resolved relations. Accepts a name, uuid, or path — `.`, a subdirectory, or any worktree resolves to its enclosing repo; `--format json` adds identity plumbing.                                                                                                                                                                                                                                                                                                                                 |
| `daft repo link <target> [--name <label>] [--kind <kind>]` | Declare a relation from the current repo to `<target>` (catalog name, repo path, or remote URL — uncloned URLs allowed): writes a deduped entry to the worktree's `daft.yml`. Re-linking is a no-op; `--name`/`--kind` update in place; self-links are refused.                                                                                                                                                                                                                                                                                                              |
| `daft repo unlink <target>`                                | Remove a relation from the current worktree's `daft.yml`, matched by label first, then resolved URL. A missing edge is a no-op (exit 0).                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| `daft go <repo>`                                           | Jump to another cataloged repo's default-branch worktree. Local resolution wins: a branch named like a repo shadows it (use `--repo`). A catalog match beats `daft.go.autoStart`. Works outside any git repo.                                                                                                                                                                                                                                                                                                                                                                |
| `daft go <repo> <branch>`                                  | Open a branch's worktree in another repo (created on demand); `daft go --repo <name> [-b <branch> [base]]` is the explicit form. After a hop, `daft go -` returns to the source worktree.                                                                                                                                                                                                                                                                                                                                                                                    |
| `daft start <repo> <branch> [base]`                        | Create a NEW branch in another cataloged repo, based on its default branch unless `[base]` is given. Local-first, first match wins: an existing local branch named `<repo>` keeps the local reading; a `<branch>` slot that already resolves to a ref here is read as a base (so `daft start api release-2` creates local branch `api`); naming your own repo stays local. `daft start --repo <name> <branch>` is the explicit form that always crosses. The destination is announced before any work; the target's trust gates its hooks; `-x` runs there, `-c` is refused. |
| `daft exec --repo <name> \| --all-repos [...]`             | Run commands in another repo's worktrees, or across every cataloged repo's default-branch worktree (rows labeled `repo:branch`).                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `daft exec --related [...]`                                | Run across the current repo and its relations, targeting each repo's worktree for the current branch; repos lacking it are skipped with a notice.                                                                                                                                                                                                                                                                                                                                                                                                                            |
| `daft start <branch> --with-related`                       | Create the same branch in the current repo and every related repo (each from its own default branch). Related repos must be cloned; hooks run only where trusted; `--carry`/`-x` stay local. `daft start <repo> <branch> --with-related` roots the fan-out at that repo's manifest instead.                                                                                                                                                                                                                                                                                  |
| `daft list\|update\|prune\|doctor --repo <name>`           | Run the command in another cataloged repo from anywhere; `--all-repos` sweeps every live entry. `daft list <repo>` is positional sugar (repo-only resolution — a miss is a hard error, never a branch fallback).                                                                                                                                                                                                                                                                                                                                                             |
| `daft hooks jobs --repo <name\|path\|uuid>`                | Inspect another repo's hook-job history — including removed repos, whose logs stay addressable.                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `daft clone <name>`                                        | Re-clone a cataloged (typically removed) repo from its recorded remote URL. `daft repo remove` tombstones the entry, so removal is reversible.                                                                                                                                                                                                                                                                                                                                                                                                                               |

Cross-repo edges are committed in `daft.yml` under a top-level `relations:` key
— `url:` required (matched against the catalog by normalized URL, so the
manifest is portable), `name:`/`kind:` optional, edges directed. Manage them
with `daft repo link`/`daft repo unlink` rather than hand-editing:

```yaml
relations:
  - url: git@github.com:acme/api-client.git
    name: client
    kind: consumer
```

### Post-setup command execution (`-x`/`--exec`)

`daft clone`, `daft init`, `daft go`, and `daft start` accept repeatable
`-x`/`--exec` commands that run sequentially in the new worktree after hooks
complete, stopping on first failure. Interactive programs work — stdio is fully
inherited.

```bash
daft clone https://github.com/org/repo -x 'mise install'
daft start my-feature -x claude
```

Use `-x` for finite setup steps. To start a long-running process (a dev server,
a compose stack, a watcher), define a task and run it on demand with `daft run`
— see Tasks (`daft run`).

## Merging Across Worktrees (`daft merge`)

`daft merge` performs `git merge` without forcing you to `git switch` into the
target branch: land a feature into `main` while staying in your worktree,
octopus-merge several sources, or script merges (`-y` auto-accepts prompts).

```bash
daft merge feature/api --no-edit                      # into the current worktree's branch
daft merge feature/api --into main --no-edit          # into another worktree; shell stays put
daft merge feat/a feat/b --into main --no-edit        # octopus: several sources, one commit
daft merge --squash feature/api --no-edit             # squash all source commits into one
daft merge --rebase feature/api --into main           # rebase source, then fast-forward (linear)
daft merge --rebase-merge feature/api --into main --no-edit  # rebase, then merge commit
daft merge -s ours --into release feature/old --no-edit      # git merge strategy flags
daft merge feature/done --into main -r --no-edit      # remove source worktree + branch on success
daft merge feature/done --into main -r --squash --set-default --no-edit  # persist style + cleanup
daft merge feature/hotfix --into release/1.2 --adopt-target --no-edit    # ephemeral target worktree
daft merge --continue|--abort|--quit [<worktree>]     # finish or bail out
```

Pitfalls to communicate to the user:

- **Default style is always-merge-commit** (never fast-forward, unlike plain
  `git merge`). Use `--rebase` for linear history. Always pass `--no-edit` in CI
  or non-TTY contexts to avoid an editor prompt.
- **`--squash` commits by default**, opening an editor pre-populated with the
  squash message. `--no-edit` uses it verbatim, `-m` supplies your own,
  `--no-commit` stages without committing (incompatible with `-r`). Without a
  TTY and without `--no-edit`/`-m`, daft refuses before merging.
- **The target must be clean** (`daft.merge.requireCleanTarget`, default true).
  Commit, stash, or `daft carry <target>` the changes first.
- **Conflicts do not hijack the shell**: daft reports the conflicted files and
  the exact command to finish. Resolve in the target worktree, `git add`, then
  `daft merge --continue [<target>]`; bail with `daft merge --abort [<target>]`.
- **Squash-staged state**: closing the squash editor without saving leaves the
  changes staged. `daft merge --continue` re-opens the editor;
  `daft merge --abort` resets the index.
- **Octopus aborts on conflict** — multi-source merges are all-or-nothing.
- **`-r` removes both worktree and branch.** Regular merges use safe
  `git branch -d` semantics; squash uses force-delete backed by daft's
  content-equivalence proof. If the source branch moved during the editor
  session, cleanup is refused with a hint.
- **Ephemeral targets**: when the target branch has no worktree, daft prompts;
  `--adopt-target` accepts, `--no-adopt-target` refuses, and
  `daft.merge.adoptTargetOnDemand` (`prompt`/`yes`/`no`) sets the default.

`pre-merge` and `post-merge` hooks fire around the merge with `DAFT_MERGE_*` env
vars (see Hook Types below).

## Hooks System (daft.yml)

Hooks automate worktree lifecycle events, configured in a `daft.yml` file at the
repository root.

### Hook Types

| Hook                   | Trigger                                         | Runs From                   |
| ---------------------- | ----------------------------------------------- | --------------------------- |
| `post-clone`           | After `daft clone`                              | New default branch worktree |
| `worktree-pre-create`  | Before new worktree is added                    | Source worktree             |
| `worktree-post-create` | After new worktree is created                   | New worktree                |
| `worktree-pre-remove`  | Before worktree is removed                      | Worktree being removed      |
| `worktree-post-remove` | After worktree is removed                       | Current worktree            |
| `pre-merge`            | After pre-flight checks, before the merge runs  | Target worktree             |
| `post-merge`           | After the merge completes (success or conflict) | Target worktree             |

`worktree-pre-remove`/`worktree-post-remove` also fire when `daft merge -r`
cleans up a merged source worktree; there `DAFT_COMMAND=merge` (not
`branch-delete`), so scripts can tell merge cleanup from a standalone
`daft remove`.

During `daft clone`, `post-clone` fires first (one-time repo bootstrap), then
`worktree-post-create` (per-worktree setup) — so `post-clone` can install
foundational tools the per-worktree hooks depend on.

`pre-merge` aborts the merge on failure; `post-merge` warns but never rolls
back. Both expose `DAFT_MERGE_*` env vars: `SOURCES`, `TARGET_BRANCH`,
`TARGET_PATH`, `MODE` (`merge`/`ff`/`squash`/`octopus`), `STRATEGY`,
`EPHEMERAL`, `CROSS_WORKTREE`, `SOURCE_SHAS` (source tips captured before the
merge). `post-merge` adds `RESULT`
(`success`/`conflict`/`already-up-to-date`/`aborted`), `COMMIT_SHA`,
`CONFLICTED_FILES`, `PROMOTED_FROM_EPHEMERAL`. `RESULT=aborted` fires when a
squash commit is abandoned (editor closed without saving, pre-commit hook fail,
GPG-sign fail) and `COMMIT_SHA` is then empty. Neither hook fires on a no-op
merge (already up to date).

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
`.config/daft.yaml`. Additionally: `daft.local.yml` for machine-specific
overrides (not committed) and per-hook files like `worktree-post-create.yml`.
The deprecated name `daft-local.yml` still works for one release cycle but warns
(and doctor flags it); prefer `daft.local.yml`.

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
  tracks: [path, branch] # Worktree attributes this job depends on (move hooks)
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
```

Independent jobs run in parallel; dependent jobs wait for their dependencies.

### Background Jobs

Jobs with `background: true` run asynchronously after the command returns, so
the user can start working while long-running tasks complete. A coordinator
process manages them and writes output to log files.

- Background jobs participate in the DAG: a foreground job depending on a
  background job promotes it to foreground automatically.
- `needs:` between background jobs is honored — the coordinator schedules them
  in topological wave order.
- If a dependency fails or is cancelled, the dependent job is recorded as
  `Skipped` in `daft hooks jobs` listings.
- `background: true` at the hook level sets the default for all its jobs.
- `DAFT_NO_BACKGROUND_JOBS=1` promotes everything to foreground (CI, debugging).
- `daft hooks jobs` lists, cancels, retries, and prunes records; removing a
  worktree cancels its running background jobs.

When generating `daft.yml`, mark jobs `background: true` when they warm caches,
pre-build, or do other work whose results are not needed immediately.

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

Available in job `run`/`script` commands and in job `env:` values, for lifecycle
hooks and `daft run` tasks alike:

| Variable              | Description                              |
| --------------------- | ---------------------------------------- |
| `{branch}`            | Target branch name                       |
| `{worktree_path}`     | Path to the target worktree              |
| `{worktree_root}`     | Project root directory                   |
| `{worktree_slug}`     | Sanitized worktree name (`[a-z0-9-]`)    |
| `{source_worktree}`   | Path to the source worktree              |
| `{git_dir}`           | Path to the `.git` directory             |
| `{remote}`            | Remote name (usually `origin`)           |
| `{job_name}`          | Name of the current job                  |
| `{base_branch}`       | Base branch (branch-creating commands)   |
| `{repository_url}`    | Repository URL (post-clone)              |
| `{default_branch}`    | Default branch name (post-clone)         |
| `{old_worktree_path}` | Previous worktree path (move hooks only) |
| `{old_branch}`        | Previous branch name (move hooks only)   |

`{worktree_slug}` is the worktree's name relative to the project root,
lowercased and reduced to `[a-z0-9-]` (max 63 chars) — safe for `docker compose`
project names, DB schema names, and temp dirs. Keyed off the worktree, not the
branch, so it is unique per worktree. Use it to make per-worktree names
collision-free: `COMPOSE_PROJECT_NAME: "api-{worktree_slug}"`.

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
    desc: "Skip file exists" # Human-readable reason

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
— naming the skipped hooks and suggesting `daft hooks trust`. Each skip is
recorded; a later `daft hooks trust` lists precise replay commands
(`daft hooks run post-clone`, `daft hooks run worktree-post-create`) for the
worktrees whose setup never ran — run them inside each listed worktree. If you
see that notice, trusting and replaying is the way to get the worktree into its
fully set-up state.

### Manual Hook Execution

Run hooks on demand, bypassing trust (the user is explicitly invoking):

```bash
daft hooks run worktree-post-create              # Run all jobs
daft hooks run worktree-post-create --job "mise" # Run a single job
daft hooks run worktree-post-create --tag setup  # Run jobs tagged "setup"
daft hooks run worktree-post-create --dry-run    # Preview without executing
daft hooks run worktree-post-create --verbose    # Show skipped jobs + reasons
```

Use cases: re-running after a failure, iterating during hook development,
bootstrapping worktrees that predate the hooks config.

### Skipping Hooks Per-Invocation (`--skip-hooks`)

The worktree-creating commands (`daft start`, `daft go`, `daft clone`,
`daft adopt`) accept `--skip-hooks` to exclude jobs for one run (repeatable or
comma-separated):

```bash
daft start feat/x --skip-hooks all           # skip every hook
daft start feat/x --skip-hooks tag:heavy,lint # skip tagged + named jobs
daft clone <url> --skip-hooks post-clone     # clone, run worktree hooks only
```

Selectors: `all`/`*`, `<hook>` (a whole hook by its canonical `daft.yml` key,
e.g. `worktree-post-create`), `tag:<tag>`, `<name>` (a job), `job:<name>`
(explicit escape hatch). A bare token resolves wildcard → hook type → job name;
tags need the `tag:` prefix. Naming a hook the command never fires is a silent
no-op.

Key behavior — the **downstream cascade**: skipping a job also skips every job
that `needs:` it (transitively); upstream dependencies are untouched. Excluded
jobs are reported as skipped with a reason, never dropped silently; a selector
matching nothing warns and the run proceeds. `--skip-hooks` is the exclusion
counterpart to `daft hooks run --job/--tag`. `--skip-hooks all` cannot be
combined with `--trust-hooks`; partial skips can.

### Git pre-push Hooks on daft Pushes

Separate from daft's own hooks: every daft-initiated `git push` honors the
repo's git-level `pre-push` hook (native `.git/hooks` or `core.hooksPath`
managers like lefthook/husky/pre-commit), reported as a `pre-push` phase. A
failing hook blocks the push and the command exits non-zero — any worktree it
created is still completed and usable, and the error names the recovery command.
Exceptions — pushes that provably carry no content skip the hook by default
(`daft.pushVerify`: `auto` default, `always`, `never`): the automatic upstream
push on `daft start`/`daft go -b` runs the hook only when it introduces new
commits, and remote-branch deletes (`daft remove` with remote deletion on,
`daft rename`'s old-name cleanup, `daft merge`'s post-merge cleanup,
`daft multi-remote move --delete-old`) skip it outright since a delete pushes
zero objects. Set `daft.pushVerify always` when pre-push hooks enforce ref
policy (e.g. protected-branch delete guards); `daft.checkout.pushVerify`
overrides the base for the upstream push alone. Note that `never` is
unconditional, unlike `auto`/`always` which decide per push: setting
`daft.pushVerify never` to quiet deletes also disarms the hook on an upstream
push that does carry commits, so re-arm it with `daft.checkout.pushVerify auto`.
Pass `--no-verify` to the pushing command to skip the hook once — every pushing
command accepts it except `daft merge`, whose cleanup delete is governed by
`daft.pushVerify` alone. `--skip-hooks` does NOT affect git-level hooks — it
only filters daft's own jobs.

To push a branch from outside its worktree with the hook still running in the
RIGHT tree, use `daft push <branch>`: it resolves the branch's worktree and runs
the push (and therefore the shared hook) from there. Plain `git push` would run
the hook in whatever worktree you happen to be in.

Parallel `sync --push` hook runs are memory-governed (#678): a governor caps
concurrent hook-bearing pushes (default `max(2, cores/4)`; `--jobs N` overrides,
`--no-throttle` disables), learns each hook's peak memory across runs, throttles
admissions under memory pressure (rows show `held: memory`), and under sustained
pressure freezes then kills-and-retries the newest push rather than let the
machine swap. Each push unit also gets a wall-clock budget
(`daft.sync.pushTimeout`, default 30m) so a hung hook cannot wedge the sync.
`daft.sync.pushHookStrategy batched` pushes every branch in one `git push` so
the hook fires once with all refs (one refusal fails the whole batch).

### Move Hooks

When a worktree moves (`daft rename`, `daft layout transform`, `daft adopt`),
daft replays identity-tracked hooks to tear down the old environment and set up
the new one: `worktree-pre-remove` + `worktree-post-remove` (old identity) →
move on disk → `worktree-pre-create` + `worktree-post-create` (new identity).
Only tracked jobs run.

```yaml
- name: link-output
  run: ln -sf {worktree_path}/dist /opt/builds/current
  tracks: [path] # Re-runs when the worktree path changes

- name: install-deps
  run: npm install
  # No tracks -- skipped during moves
```

If `tracks` is omitted, daft infers it from template usage (`{worktree_path}`
implies `path`; `{branch}` implies `branch`); explicit `tracks` overrides. Jobs
listed in a tracked job's `needs` are pulled into the move even if untracked.
Hook failures during moves warn but never block — the move always completes.
Move-only template variables `{old_worktree_path}`/`{old_branch}` and env vars
`DAFT_IS_MOVE`, `DAFT_OLD_WORKTREE_PATH`, `DAFT_OLD_BRANCH_NAME` are available.

### Environment Variables in Hooks

All hooks receive: `DAFT_HOOK`, `DAFT_COMMAND`, `DAFT_PROJECT_ROOT`,
`DAFT_GIT_DIR`, `DAFT_REMOTE`, `DAFT_SOURCE_WORKTREE`. Worktree hooks add
`DAFT_WORKTREE_PATH`, `DAFT_BRANCH_NAME`; creation hooks `DAFT_IS_NEW_BRANCH`,
`DAFT_BASE_BRANCH`; clone hooks `DAFT_REPOSITORY_URL`, `DAFT_DEFAULT_BRANCH`;
removal hooks `DAFT_REMOVAL_REASON` (`remote-deleted`, `manual`, `ejecting`);
move hooks `DAFT_IS_MOVE`, `DAFT_OLD_WORKTREE_PATH`, `DAFT_OLD_BRANCH_NAME`.

## Tasks (`daft run`)

Tasks are named, user-invoked job groups — the **serve on demand** half of the
workflow. They live under a top-level `tasks:` section in `daft.yml` (a sibling
of `hooks:`) and run only when asked:

```yaml
hooks:
  worktree-post-create:
    jobs:
      - name: install
        run: pnpm install # provision: finite, idempotent, unattended

tasks:
  run: # reserved default — bare `daft run`
    parallel: true
    jobs:
      - name: backend
        run: docker compose up
        env:
          COMPOSE_PROJECT_NAME: "api-{worktree_slug}"
      - name: web
        run: pnpm dev
        root: frontend
  seed-db: # `daft run seed-db`
    jobs:
      - name: seed
        run: ./scripts/seed.sh
```

- `daft run` — runs the reserved task named `run`
- `daft run <name>` — runs any task
- `daft run <name> <args>...` — words after the task name are shell-escaped and
  appended to the task's command (requires the task to resolve to a single
  foreground job; narrow multi-job tasks with `--job`)
- `daft run <args>...` — a first word naming no task forwards every word to the
  reserved `run` task (an unknown first word errors only when no `run` task
  exists to receive it); `daft run -- <args>...` forces forwarding past the name
  match
- Everything after the first word passes through verbatim, flags included —
  `daft run`'s own flags go before the task name
- `daft run --list` — lists the tasks with job counts
- `daft run --job <name>` / `--tag <tag>` — run a subset of the task's jobs

Tasks reuse the full job schema — `parallel`/`piped`/`follow`, `needs`, groups,
`env`, `root`, `skip`/`only`, `interactive`, `background`. Differences from
lifecycle hooks:

- **No execution timeout.** A task runs until it exits or is cancelled (hook
  jobs keep the 300s default) — the right home for dev servers and watchers.
- **Foreground and attended.** A single-job task passes the terminal straight
  through — the job's raw output, no wrapper. A multi-job task renders one live
  row per job with the logs threaded beneath. Ctrl+C cancels the job tree
  gracefully, a second Ctrl+C force-kills. There is no detach flag; jobs marked
  `background: true` still dispatch to the coordinator as usual (visible in
  `daft hooks jobs`).
- **Trust.** An explicit `daft run` counts as consent: it executes even in an
  untrusted repo, printing a `daft hooks trust` hint (same rule as
  `daft hooks run`).
- Task jobs receive `DAFT_TASK=<name>` in place of `DAFT_HOOK`, with
  `DAFT_COMMAND=run`.

Positioning: `daft exec` runs an ad-hoc command you type on the spot; `daft run`
runs a named task committed in `daft.yml` (npm's `exec`/`run` split). Prefer a
task over hand-launching a server when one is defined.

## Environment Tools and Suggested Automation

When working in a daft repo, detect environment tools by their marker files and
suggest `daft.yml` hooks that automate setup for new worktrees.

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

A combined example for a project using mise, direnv, and npm — `post-clone` runs
once at clone, `worktree-post-create` for every new worktree:

```yaml
hooks:
  post-clone:
    piped: true
    jobs:
      - name: mise-install
        run: mise install
      - name: direnv-allow
        run: direnv allow .
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

Most tools are one-liners (`mise install`, `direnv allow .`, `npm install`,
`cargo build`, `go mod download`). Two need care:

```yaml
# nvm is a shell function -- source it first
- name: nvm-use
  run: |
    export NVM_DIR="$HOME/.nvm"
    [ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh"
    nvm install

# Python: create the venv before installing into it
- name: pip-install
  run: |
    python -m venv .venv
    . .venv/bin/activate
    pip install -r requirements.txt
```

When a daft-managed repo has no `daft.yml`, check for dependency markers and
suggest creating one — a `worktree-post-create` hook that installs dependencies
is the highest-impact automation. After suggesting a `daft.yml`, remind the user
to trust the repo: `daft hooks trust`.

## Workflow Guidance for Agents

When working in a daft-managed repository, apply these translations:

| User intent                       | Correct daft approach                                                                         |
| --------------------------------- | --------------------------------------------------------------------------------------------- |
| "Create a branch"                 | `daft start <name>` — creates branch + worktree                                               |
| "Branch from main"                | `daft start <name> main` — branches from the specified base                                   |
| "Create a branch in another repo" | `daft start <repo> <name>` — creates it there; shell lands in the worktree                    |
| "Switch to branch X"              | Navigate to the worktree directory: `cd ../X/`                                                |
| "Go back"                         | `daft go -` — toggles to the previous worktree                                                |
| "Check out a PR"                  | `daft go pr:<number>` — fork-aware, via `gh`/`glab`; also `mr:<number>` or a pasted PR/MR URL |
| "Delete a branch"                 | `daft remove <branch>` — removes worktree + local branch                                      |
| "Clean up branches"               | `daft prune` — removes worktrees for deleted, merged remote branches                          |
| "Wrong branch"                    | `daft carry <correct-branch>` — moves uncommitted changes                                     |
| "Update from remote"              | `daft update` — updates current or specified worktrees (`source:dest` too)                    |
| "Merge my branch"                 | `daft merge <branch> --into main --no-edit`                                                   |
| "Run my build on these worktrees" | `daft exec feat/a feat/b -- <cmd>` or `daft exec --all -- <cmd>`                              |
| "Start the dev server / stack"    | `daft run` — runs the reserved `run` task from `daft.yml`, if one exists                      |
| "Adopt existing repo"             | `daft adopt` — converts a traditional repo to daft layout                                     |

### Per-worktree Isolation

Each worktree has its **own** `node_modules/`, `.venv/`, `target/`, etc. A new
worktree created without `daft.yml` hooks has nothing installed — if the user
hits missing-dependency errors there, run the appropriate install command in
that worktree (`npm install`, `pip install -r requirements.txt`, ...).

### Navigating Worktrees

From any worktree, sibling worktrees are at `../<branch-name>/` and the project
root is at `..`. Use `git rev-parse --git-common-dir` to find the root
programmatically.

### Modifying Shared Files

Tracked files (`.gitignore`, CI config) live in each worktree independently;
changes propagate only by committing and merging. `daft.yml` is different when
it is untracked — a **visitor configuration** — in which case daft itself
propagates it:

- **Three states**: tracked / visitor (untracked) / missing. Every untracked
  daft file daft writes is recorded as that worktree's **seed**; the on-disk
  copy is later classified as pristine (untouched), refined (edited — real user
  data), or no-seed (treated like refined).
- **Branch-out**: creating a worktree copies in-scope untracked daft files from
  the source worktree before `worktree-post-create` hooks fire.
- **`daft merge`**: pristine source copies are skipped; a refined source is
  merged three-way (seed as base) into the target before the git merge,
  atomically, announcing adopted keys. Keys changed on both sides prompt
  interactively; non-interactive runs abort pointing at `daft file merge`.
- **Removal** (`daft remove`): pristine copies delete silently; refined copies
  stop the removal — interactively offering consolidate / discard / abort;
  non-interactively refusing with a `daft file merge` suggestion. Forcing (`-f`)
  discards to `<git-common-dir>/.daft/discarded/<branch>/`.
- **`daft prune` / `daft sync`** never prompt: pristine copies prune cleanly,
  refined ones keep their worktree with an end-of-run summary pointing at
  `daft file merge`; `--force` discards to the stash.
- The default-branch worktree's config is only written by an announced merge
  consolidation or an explicit `daft file merge` — never by a removal path.
  Doctor surfaces each config's classification as informational.

### `daft file merge` — consolidate configs

`daft file merge <TARGET> <SOURCE>` (or `daft file merge <SOURCE>`) consolidates
one daft file into another. With seed provenance the merge is three-way: only
genuine refinements move, a key-level preview prints first, the target is backed
up to `<git-common-dir>/.daft/backups/file-merge/`, and conflicting keys need a
side (interactive prompt, `-y` for source-wins, or a non-zero abort listing the
keys). Without provenance, a legacy two-way merge applies (source wins). The
source file is deleted afterward unless `--keep-source`. Use it to consolidate
visitor configs before removing a worktree or to promote one to a team baseline.

### `daft repo install` — bootstrap a config

`daft repo install` writes a starter `daft.yml` (commented skeleton: `hooks:`,
`shared:`, `layout:`) at the worktree root. It is repo-aware: from a worktree
subdir it targets the worktree root; at the bare container root of a contained
layout it installs across the repo's worktrees (never a stray file at the inert
container root); it refuses only outside a git repository. If a `daft.yml`
already exists it reports whether that file is tracked (team baseline) or a
visitor config and stops cleanly (exit 0). It then offers to add `/daft.yml` to
`.git/info/exclude` (local, never committed) so a visitor config stays private:
prompted on a TTY (default No), `--git-exclude` adds it without prompting,
non-interactive runs print a copy-pasteable hint. It never touches the tracked
`.gitignore`. `daft install` is a top-level alias for the same command.

## Machine-Readable Output

Commands emitting structured output via `--format`: flat-list (`list`,
`hooks trust list`, `layout list`), document (`release-notes`), matrix
(`shared status`), sectioned (`multi-remote status`, `hooks run` listing mode).
Valid formats: `json`, `ndjson`, `tsv`, `csv`, `yaml`, `toon`, `markdown`.
`--template '<tera-template>'` renders custom output (`{{ var }}`, `{% for %}`,
`{% if %}`).

**`daft list` output contract**: table columns show branch (`✦` marks the
default branch), path (relative to cwd), base ahead/behind, file status (`!N`
conflicted, `+N` staged, `-N` unstaged, `?N` untracked), remote status (`⇡N`
unpushed, `⇣N` unpulled), branch age, owner, and commit info. When `user.email`
is configured, output splits into two sections (your branches / other branches)
per the resolved owner. `--format json` fields include `is_default_branch`,
`staged`, `unstaged`, `untracked`, `conflicted`, `operation`, `identity_source`,
`remote_ahead`, `remote_behind`, `branch_age`, `owner_name`, `owner_email`; the
`owner` field is `{name, email}` or `null`.

**Paused operations and detached HEAD**: a worktree keeps its branch name while
git has HEAD detached to run an operation — mid-rebase the row still names the
branch and keeps its branch-keyed fields, rather than reporting `(detached)`.
`operation` names the paused operation (`rebase`, `merge`, `cherry-pick`,
`revert`, `bisect`, `am`, or `null`), `conflicted` counts unresolved files
(never double-counted as staged and unstaged), and `identity_source` says how
the name was established: `attached` (checked out), `recovered` (read from the
paused operation), `persisted` (daft's record of what the worktree was created
for), or `none`. `is_sandbox` is true only for a detached checkout that no
operation explains — so a rebasing worktree is **not** a sandbox. The `status`
column (`--columns +status`) renders the same state as words
(`rebasing · 2 conflicts`, `rebasing · resolved`, `detached @ <sha>`,
`drifted`). `daft list --merging` filters to worktrees mid-_merge_ only, not any
operation.

**`daft hooks jobs` output**: a flat table with one row per job carrying
invocation context (`invocation_id`, `invocation_short`, `worktree`,
`hook_type`, `trigger_command`, `invocation_created_at`) plus `size_bytes` for
the job's `output.log`.

**Column selection (`--columns`)** on `list`/`sync`/`prune`: default columns
`annotation`, `branch`, `path`, `base`, `changes`, `remote`, `age`, `owner`,
`last-commit`; optional `size` (adds a total footer) and `hash`. `daft list`
also offers `status` (paused operation and conflict state in words), which sorts
before `branch`; it is list-only, since sync and prune pin their own
task-progress column of that name. On all three, `pr` is also a default
(`#N`/`!N` for PR/MR checkouts and for local branches with an open or merged PR)
— but only in repos with a GitHub/GitLab remote, and daft silently drops it
while the forge integration is broken in a way needing user action (gh/glab
missing or unauthenticated; it returns after a successful refresh — do not treat
the column's absence as an error). `--columns +pr` forces it regardless. While
the `pr` column shows, `daft list` also adds a row for every open PR the table
doesn't already represent (`sync`/`prune` show the column on their worktree rows
without adding rows): PR-bearing local branches appear without `-b`
(`"kind": "branch"` in json), and PRs with no local presence — colleagues'
branches, fork PRs — appear as rows built from forge data (`"kind": "pr"`; fork
rows named `owner:branch`; the PR title in the commit-subject field).
Merged/closed PRs decorate rows but never add one. Branch and PR rows report the
PR author as their owner; worktree rows keep the locally deduced owner (name and
email). Expect these extra rows when parsing default list output — filter by
`kind`, or pass `--columns -pr`, which removes the rows and the column as one
unit, for a worktrees-only listing. In piped/`NO_COLOR` output — what agents
read — the PR's cached fate trails as a glyph: `✓`/`✗`/`●` CI pass/fail/running,
`◆` merged, `○` closed; in a color terminal the number's color carries the same
states instead (green/red/yellow, purple merged, dim closed). The cache
refreshes in the background via `daft update`/`daft sync` and on listing with
the column; prefer `--format json` (`pr_state`, `ci_status`, `pr_url` fields —
present in the default schema even when the table hides the column) over parsing
glyphs. Two modes — replace (`--columns branch,path,age`: exactly those, in
order) and modifier (`--columns +size,-age`: adjust defaults; auto-detected when
every entry starts with `+`/`-`). The `status` column is always pinned on
`sync`/`prune`. Persistent defaults: `git config daft.<cmd>.columns`.

**Sorting (`--sort`)** on `list`/`sync`/`prune`: columns `branch`, `path`,
`size`, `age`, `owner`, `hash`, `activity`, `commit`; prefix `+` ascending
(default) / `-` descending; comma-separate for multi-level
(`--sort +owner,-size`). `activity` counts committed and uncommitted changes;
`commit` (alias `last-commit`) only commit time. Sorting by hidden columns
works. Persistent defaults: `git config daft.<cmd>.sort`.

**Caches**: `daft list` writes content-addressed JSON caches under
`<git-common-dir>/.daft/cache/` — safe to delete at any time; daft re-populates.
`Changes`/`Size` cells always recompute and show the `·` skeleton glyph until
the result arrives.

## Configuration Reference

**Local-first defaults**: daft does not contact the remote by default. Remote
operations are opt-in via `daft config remote-sync --on` (or the individual keys
below); `--local` on any command suppresses remote operations for a single
invocation.

| Key                              | Default               | Description                                                                                                                                                                 |
| -------------------------------- | --------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `daft.autocd`                    | `true`                | CD into new worktrees via shell wrappers                                                                                                                                    |
| `daft.remote`                    | `"origin"`            | Default remote name                                                                                                                                                         |
| `daft.checkout.fetch`            | `false`               | Fetch from remote before checking out an existing branch                                                                                                                    |
| `daft.checkout.push`             | `false`               | Push new branches to remote after creation                                                                                                                                  |
| `daft.pushVerify`                | `"auto"`              | Base: run git pre-push hooks on daft's suppressible pushes — remote-branch deletes and the upstream push (`auto`, `always`, `never`; `never` is unconditional)              |
| `daft.checkout.pushVerify`       | `daft.pushVerify`     | Checkout-scoped override of `daft.pushVerify` for the automatic upstream push                                                                                               |
| `daft.branchDelete.remote`       | `false`               | Delete the remote branch when removing a local branch                                                                                                                       |
| `daft.checkout.upstream`         | `true`                | Set upstream tracking                                                                                                                                                       |
| `daft.checkout.carry`            | `false`               | Carry uncommitted changes on checkout                                                                                                                                       |
| `daft.checkoutBranch.carry`      | `true`                | Carry uncommitted changes on branch creation                                                                                                                                |
| `daft.update.args`               | `"--ff-only"`         | Default pull arguments for update (same-branch mode)                                                                                                                        |
| `daft.prune.cdTarget`            | `"root"`              | Where to cd after pruning (`root` or `default-branch`)                                                                                                                      |
| `daft.go.autoStart`              | `false`               | Auto-create worktree when branch not found in `daft go`                                                                                                                     |
| `daft.forge.platform`            | (detected)            | Forge platform for `pr:`/`mr:` checkout (`github`, `gitlab`); unset detects from the remote URL                                                                             |
| `daft.forge.githubCli`           | `"gh"`                | GitHub CLI binary used for PR resolution (Enterprise wrappers)                                                                                                              |
| `daft.forge.gitlabCli`           | `"glab"`              | GitLab CLI binary used for MR resolution                                                                                                                                    |
| `daft.forge.hostname`            | (CLI default)         | Self-hosted / Enterprise forge hostname, passed to the CLI as `--hostname`                                                                                                  |
| `daft.merge.requireCleanTarget`  | `true`                | Refuse to merge into a target worktree with uncommitted changes                                                                                                             |
| `daft.merge.adoptTargetOnDemand` | `"prompt"`            | Ephemeral-target behavior for `daft merge` (`prompt`, `yes`, `no`)                                                                                                          |
| `daft.hooks.enabled`             | `true`                | Master switch for hooks                                                                                                                                                     |
| `daft.hooks.defaultTrust`        | `"deny"`              | Default trust for unknown repos                                                                                                                                             |
| `daft.hooks.timeout`             | `300`                 | Hook timeout in seconds                                                                                                                                                     |
| `daft.<cmd>.stat`                | `"summary"`           | Statistics mode (`summary` or `lines`) for `list`/`sync`/`prune`                                                                                                            |
| `daft.<cmd>.columns`             | (all columns)         | Default columns for `list`/`sync`/`prune` (same syntax as `--columns`)                                                                                                      |
| `daft.list.sizeConcurrency`      | (CPU count)           | Max concurrent directory-size walks for `--columns +size` (both `daft list` and `daft repo list`); lower on slow/network filesystems (env `DAFT_SIZE_WALK_JOBS` overrides). |
| `daft.<cmd>.sort`                | `"+branch"`           | Default sort order for `list`/`sync`/`prune` (same syntax as `--sort`)                                                                                                      |
| `daft.ownership.strategy`        | `"recency-plurality"` | Branch-ownership strategy: `tip`, `any`, `first`, `plurality`, `majority`, `recency-plurality`                                                                              |
| `daft.sync.pushTimeout`          | `"30m"`               | Wall-clock budget per sync push unit (git + pre-push hook); `off` disables                                                                                                  |
| `daft.sync.pushHookStrategy`     | `"per-branch"`        | Pre-push hook cadence for sync pushes (`per-branch` or `batched`)                                                                                                           |
| `daft.governor.mode`             | `"auto"`              | Memory-aware governor for parallel pre-push hooks (`auto` or `off`)                                                                                                         |
| `daft.governor.jobs`             | `"auto"`              | Cap on concurrent hook-bearing pushes (`auto` = `max(2, cores/4)`, or a number)                                                                                             |
| `daft.governor.memoryReserve`    | `"auto"`              | Memory headroom the governor keeps free (`auto` = max(10% RAM, 2G), a size, or `NN%`)                                                                                       |
| `daft.governor.jobserver`        | `"auto"`              | Shared POSIX jobserver export to hooks (`auto` or `off`)                                                                                                                    |

**Branch ownership** scopes the two-section split in `daft list` and limits
`daft sync --rebase`/`--push` to branches you own (matching `user.email`);
`--include <email>`/`--include unowned` overrides. Ownership is deduced from the
`base..branch` commit range by the configured strategy — the default
`recency-plurality` weights each commit by recency, staying robust to drive-by
commits.
