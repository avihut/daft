---
title: Troubleshooting recipes
description:
  Common symptoms when running daft hooks — what each error usually means and
  which pattern documents the fix.
pillars: [hooks]
---

# Troubleshooting recipes

Symptoms specific to hooks and recipes — typically "the hook ran (or didn't),
and the result wasn't what I expected." For general daft issues (install,
layout, "command not found"), see
[About → Troubleshooting](/about/troubleshooting).

## `daft start` hangs or takes forever

A `worktree-post-create` job is running synchronously when it should be
backgrounded. Common culprits: `cargo build`, `pnpm exec vite optimize`, codegen
scripts that take more than a few seconds. The hook waits for each job to
complete before returning control to your shell.

The fix is `background: true` on the slow job:

```yaml
- name: warmup-build
  run: cargo build --workspace
  background: true
  needs: [fetch-deps]
```

The job continues running after `daft start` returns; the worktree is usable for
other commands while the warmup completes. See
[Background warmup](/recipes/background-warmup) for the full pattern, including
which warmups to skip in CI.

## Hook trust prompt fires every time

`daft.yml` is in a state daft considers untrusted. Two common reasons:

- **The remote URL changed** (`git remote set-url`, switching forks). Trust
  state is keyed on the remote URL; a change downgrades trust to "prompt."
  Re-trust with `git daft-hooks trust`.
- **CI runners are ephemeral.** Each run starts with no trust state, so the
  prompt fires every job. Add `git daft-hooks trust --all` with
  `DAFT_NONINTERACTIVE=1` _before_ any `daft hooks run` step.

If the prompt fires _during_ normal `daft start` on the same machine where you
previously trusted the file, `daft.yml` itself was modified — which is the
system working as designed. Re-trust is required after content changes. See
[Trust & security](/hooks/trust-and-security).

## Hook ran but env vars aren't set in my shell

A job's per-job `env:` block sets variables for that job only — they don't
propagate to the parent shell, even though the hook ran in the worktree. The
hook environment and the shell environment are separate.

If you need a value in your shell after worktree creation:

- **Best:** export it from `.envrc` (loaded by direnv on `cd`).
- **OK:** the hook seeds `.envrc` with the value (write-once, idempotent).
- **Don't:** assume per-job `env:` reaches your shell. It doesn't.

For mise's `[env]` block, the values are exported by mise's shell activation —
also at `cd` time, not at hook time. See
[Env vars & secrets](/recipes/env-vars-and-secrets) for the deeper
hook-time-vs-shell-time distinction.

## Port already in use after `daft remove`

A `worktree-pre-remove` job didn't run, or didn't complete cleanly. Containers
leaked; their ports remain bound. Check:

- Is `worktree-pre-remove` defined in `daft.yml`? `daft remove` only fires the
  hook if you've defined it.
- Did it succeed? `daft hooks log show` from the worktree _before_ removing it —
  afterwards the logs are at `~/.local/state/daft/logs/`.
- Is `COMPOSE_PROJECT_NAME` set the same way in pre-remove as in post-create? A
  mismatch means the down command targets the wrong containers (typically the
  empty default project — and silently succeeds).

To clean up after a failed pre-remove:
`docker ps -a --filter name=<project>-<branch>` finds the leaked containers;
`docker rm -f` removes them. See [Cleanup on remove](/recipes/cleanup-on-remove)
for how to wire pre-remove correctly.

## "daft.yml is untrusted; refusing to run hooks"

The default-deny posture: hooks don't run until you explicitly trust them. After
any change to `daft.yml`:

```bash
git daft-hooks trust
```

This is intentional — see [Trust & security](/hooks/trust-and-security) for the
threat model. For automated environments (CI, ephemeral dev sandboxes), use
`git daft-hooks trust --all` with `DAFT_NONINTERACTIVE=1` set.

## Hook works locally but fails in CI

Three common divergences:

1. **The hook needs a value direnv exports locally.** CI doesn't run direnv.
   Move the loading into per-job `env:` blocks (sourced from `.env` directly) or
   into the workflow's own `env:` for CI-specific values.
2. **The hook depends on a tool installed by mise locally.** CI needs
   `mise install` as one of the hook's first jobs (or a mise-installing step in
   the workflow before `daft hooks run`).
3. **The trust step is missing.** CI runs see an untrusted `daft.yml`; jobs
   silently don't run. Always trust before running: `git daft-hooks trust --all`
   plus `DAFT_NONINTERACTIVE=1`.

See [CI parity](/recipes/ci-parity) for the canonical workflow shape.

## Where to next

- **[Trust & security](/hooks/trust-and-security)** — the trust model behind
  several of the symptoms above; deeper than the snippets here.
- **[Lifecycle hooks](/hooks/lifecycle)** — when each hook fires relative to
  user-visible operations.
- **[Anti-pattern: shared mutable state](/recipes/anti-patterns/shared-mutable-state)**
  — many port and volume collisions trace back to sharing what should be
  per-worktree.
