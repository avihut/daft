---
title: Migrating from lefthook
description:
  Run your existing lefthook.yml with daft, then convert it — what translates,
  what does not, and how to back out.
---

# Migrating from lefthook

Daft's hook schema was forked from lefthook's documented interface, so most of a
`lefthook.yml` means the same thing in daft. That makes the migration two steps
rather than a rewrite: run your existing file with daft to see whether it works
for you, then convert it when you are sure.

## Step 1 — run your existing config

```
daft hooks install
```

In a repository with a `lefthook.yml` and no git stages in `daft.yml`, install
takes over: daft becomes the thing git calls, and it runs your existing config
unchanged. lefthook's hook files are moved aside to `<stage>.pre-daft`.

Nothing is written to the tracked tree. `daft hooks uninstall` restores the
repository byte for byte, lefthook's hook files included, so this is a
reversible thing to try on a Friday.

Install reports anything in the file daft will not do, and so does each run —
see [what does not translate](#what-does-not-translate).

Your existing habits keep working while you assess: `LEFTHOOK=0` disables the
run, and `LEFTHOOK_EXCLUDE=job1,job2` skips jobs. Both are honoured only while
daft is running a lefthook config; once you have converted, daft's own
`DAFT_HOOKS=0` and `--skip-hooks` take over.

## Step 2 — convert it

```
daft hooks import --dry-run   # see what it would write
daft hooks import
```

This writes the same definitions into `daft.yml`, appending to it rather than
reserializing — comments and formatting in an existing config survive.

The import takes effect immediately: a git stage defined in `daft.yml` wins over
the lefthook file for **every** stage, not just the ones you imported. That is
deliberate. Running the two side by side would make a half-finished migration
behave like neither end of it.

`lefthook.yml` is not deleted. Removing your config is your decision, and the
import already made it inert — the command prints the `git rm` for when you are
ready.

## What translates unchanged

Hook bodies, almost entirely:

- `parallel:`, `piped:`, `follow:`
- `jobs:` with `name`, `run` (including the per-OS map form), `script`,
  `runner`, `root`, `glob`, `exclude`, `files`, `env`, `tags`, `fail_text`,
  `interactive`, `priority`, `needs`, `group`, `stage_fixed`, `file_types`,
  `use_stdin`
- `commands:` (the older map form)
- `skip:` / `only:` in every form — booleans, env names, `ref:`, `run:`,
  `merge`, `rebase`
- `exclude_tags:`, `files:`, `setup:`, `fail_on_changes:`
- Top level: `min_version` (see below), `colors`, `no_tty`, `rc`, `output`,
  `source_dir`, `source_dir_local`, `templates`, `skip_lfs`
- `lefthook-local.yml`, applied as an overlay exactly as you would expect

File placeholders keep their spellings: `{staged_files}`, `{push_files}`,
`{all_files}`, `{files}`, and the quoting variants `"{files}"` / `'{files}'`.

Two things get new names on the way in, and `import` rewrites them for you:

| lefthook           | daft              | why                                                   |
| ------------------ | ----------------- | ----------------------------------------------------- |
| `post-merge:`      | `git-post-merge:` | daft already has a `post-merge` hook for `daft merge` |
| a custom hook name | a `tasks:` entry  | it is not a git event; run it with `daft run <name>`  |

## What does not translate

**`remotes:`** — pulling hook definitions from another repository at run time.
Daft will not fetch and execute remote configuration. Reported on install and on
every run rather than quietly dropped: a repository that believes it has gates
it does not have is worse off than one that knows.

**`extends:`** — not followed when running a lefthook config directly. Inline
the fragments, or import, since daft's own `extends:` does work.

**`min_version:`** — reported and ignored. It constrains the tool that wrote the
file, and daft is not that tool; enforcing it would fail every takeover of a
pinned config.

**Server-side and protocol hooks** — `pre-receive`, `update`, `post-receive`,
`post-update`, `push-to-checkout`, `fsmonitor-watchman`, `proc-receive`,
`reference-transaction`. Daft manages a developer's clone, and the protocol
hooks hold line-by-line conversations with git that arbitrary jobs would corrupt
rather than gate.

**TOML and JSON configs** — daft reads YAML only, and says so rather than
reporting that no config was found.

**`assert_lefthook_installed:`, `lefthook:`, `rc_local:`** — these manage
lefthook's own installation, which is not a thing that survives the migration.

## What you get that is new

- Stages report **per job**, not as one opaque subprocess — including during
  `daft push`, which runs `pre-push` itself.
- Every run is recorded: `daft hooks jobs`, `daft hooks jobs logs <name>`.
- The same config drives daft's worktree lifecycle hooks and `daft merge` gates,
  so a repository has one hooks file rather than two.
- A trust model: hooks in an unfamiliar clone do not run until you say so, and
  the skip does not block your commit.
- Long file lists are split across execs instead of failing with `E2BIG`.

## Backing out

```
daft hooks uninstall
```

Every hook daft displaced comes back, including lefthook's. If you also ran
`import`, delete the `hooks:` block it appended — its provenance comment marks
where it starts.

## Running both

Don't. Whichever tool owns the hooks directory is the one git calls, and having
lefthook's shims back while daft's config is live means your gates run once,
from whichever file that tool reads. `daft hooks status` reports which config is
in force.
