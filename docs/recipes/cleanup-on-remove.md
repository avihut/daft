---
title: Cleanup on remove
description:
  Tear down state symmetrically when a worktree is removed — volumes,
  registries, ports, daemons.
pillars: [worktrees, hooks]
---

# Cleanup on remove

> Every resource a worktree creates needs a path back out. Containers, volumes,
> named processes, registry entries, cache directories — all of these accumulate
> if nothing tears them down. `worktree-pre-remove` is the symmetric mirror of
> `worktree-post-create`: whatever the create hook brought into existence, the
> pre-remove hook puts back.

## When to reach for this

- Your `worktree-post-create` hook starts services, allocates resources, or
  registers the worktree somewhere.
- Stale containers, half-released ports, or orphaned volumes have ever surprised
  you a week after deleting a branch.
- You want `daft prune` (or `daft remove`) to leave nothing behind on disk or in
  any external system.

If your post-create hook only does `pnpm install` and `cargo fetch` — work
that's confined to the worktree directory itself — you can skip this pattern.
The worktree directory deletion is the cleanup.

## Minimal recipe

```yaml
# daft.yml
hooks:
  worktree-pre-remove:
    jobs:
      - name: services-down
        run: docker compose down -v
        env:
          COMPOSE_PROJECT_NAME: ${DAFT_REPO_NAME:-app}-${DAFT_BRANCH_NAME//\//-}
```

`worktree-pre-remove` runs **before** the worktree directory is deleted, so the
hook still has access to the worktree's files (compose.yaml, .envrc, etc.). Its
env includes `DAFT_REMOVAL_REASON` (`manual`, `remote-deleted`, or `ejecting`) —
useful when you want different behavior for a `daft prune` cleanup vs a
`daft repo remove`.

The default fail mode is `warn`: a failed teardown logs but does not block
worktree removal. That's the right default — a stuck container shouldn't prevent
you from cleaning up.

## Variants

### Compose teardown with volumes

The pairing for [Services with ports](/recipes/services-with-ports). The `-v`
flag is critical:

```yaml
- name: services-down
  run: docker compose down -v --remove-orphans
  env:
    COMPOSE_PROJECT_NAME: ${DAFT_REPO_NAME}-${DAFT_BRANCH_NAME//\//-}
```

Without `-v`, named volumes survive. With it, the worktree's state-bearing
volumes (postgres data, redis dump, MinIO buckets) get destroyed along with the
containers. `--remove-orphans` catches containers from compose files that were
edited or removed while the stack was up.

### Native processes by PID file

If `worktree-post-create` started long-lived processes outside compose, track
them via a PID file in the worktree:

```yaml
# In post-create:
- name: dev-server
  run: |
    ./bin/myserver --port "$PORT_APP" &
    echo $! > .daft/dev-server.pid
  background: true
```

```yaml
# In pre-remove:
- name: stop-dev-server
  run: |
    if [ -f .daft/dev-server.pid ]; then
      kill "$(cat .daft/dev-server.pid)" 2>/dev/null || true
    fi
```

`|| true` keeps the hook from failing if the process already died.

### Cleanup by port

If you can't get a PID, fall back to killing by port:

```yaml
- name: free-port
  run: |
    lsof -ti tcp:"$PORT_APP" | xargs -r kill 2>/dev/null || true
```

`lsof -ti` lists PIDs holding the port; `xargs -r` skips if empty. Works on
macOS and Linux.

### External registry deregistration

If `worktree-post-create` registered the worktree somewhere external — a Consul
KV entry, an OAuth callback URL, a CDN purge config — deregister it on remove:

```yaml
- name: consul-deregister
  run: |
    consul kv delete "dev/$DAFT_BRANCH_NAME"
- name: cdn-purge
  run: |
    curl -X DELETE "$CDN_API/zones/dev-${DAFT_BRANCH_NAME//\//-}"
```

These can fail safely (the entry might already be gone) — the default `warn`
fail mode is the right behavior.

### Per-removal-reason logic

`DAFT_REMOVAL_REASON` lets you handle different removal contexts:

```yaml
- name: archive-state
  run: |
    case "$DAFT_REMOVAL_REASON" in
      remote-deleted)
        # Branch was deleted on remote — archive any uncommitted experiments
        tar czf "$HOME/daft-archives/$(basename "$DAFT_WORKTREE_PATH").tar.gz" .
        ;;
      manual|ejecting)
        # User-driven removal — assume they know what they're doing, skip archive
        ;;
    esac
- name: services-down
  run: docker compose down -v
  env:
    COMPOSE_PROJECT_NAME: ${DAFT_REPO_NAME}-${DAFT_BRANCH_NAME//\//-}
```

Reason values: `remote-deleted` (auto-detected by `daft prune` / `daft sync`),
`manual` (explicit `daft remove`), `ejecting` (the worktree is being un-managed
by daft, not removed from disk). See
[Lifecycle hooks → Removal](/hooks/lifecycle#removal-remove-hooks-only).

### Run pre-remove jobs in parallel

Pre-remove jobs default to parallel — most teardowns are independent
(containers, registry entries, log shipping):

```yaml
worktree-pre-remove:
  parallel: true # default
  jobs:
    - name: services-down
      run: docker compose down -v
    - name: cdn-purge
      run: ...
    - name: consul-deregister
      run: ...
```

If teardown ordering matters (e.g., must shut down the app server before the
database), use `piped: true` or `needs:`.

## Idempotency & safety

Pre-remove hooks should tolerate already-clean state:

| Pattern                                                  | Idempotent?        |
| -------------------------------------------------------- | ------------------ |
| `docker compose down` (containers don't exist)           | ✓ — exits 0        |
| `kill $(cat .daft/dev-server.pid)` (PID gone)            | ✗ — failing kill   |
| `kill $(cat .daft/dev-server.pid) 2>/dev/null \|\| true` | ✓                  |
| `consul kv delete` (entry gone)                          | ✗ — exits non-zero |
| `consul kv delete ... \|\| true`                         | ✓                  |

The pattern: every cleanup command gets `|| true` (or equivalent suppression),
unless the failure genuinely indicates a real problem worth surfacing.

::: warning Don't put cleanup in `worktree-post-remove` instead
`worktree-post-remove` runs **after** the worktree directory is gone. Compose
files, PID files, .envrc — all unreachable. Pre-remove is the last chance to
read worktree-local state. Post-remove is for global cleanup that doesn't need
the worktree's files. :::

## Composes well with

- **[Services with ports](/recipes/services-with-ports)** — the paired
  create-side. Always together; never just one.
- **[Toolchain bootstrap](/recipes/toolchain-bootstrap)** — usually doesn't need
  cleanup (deps are inside the worktree dir, removed with it). The exception is
  global side effects like a schema-registered database — clean those up here.
- **[Lifecycle hooks → Move hooks](/hooks/lifecycle#move-move-hooks-only)** —
  `daft rename` triggers move hooks, not remove hooks. If your cleanup deletes
  data that should survive a rename, gate it with `DAFT_IS_MOVE`:

```yaml
- name: services-down
  run: docker compose down -v
  skip:
    env: { DAFT_IS_MOVE: "true" }
```

## Anti-patterns

- **No pre-remove at all** — orphaned containers and ports accumulate.
  `docker system df` slowly grows; ports randomly fail; eventually you reset
  Docker Desktop in frustration.
- **Cleanup in `worktree-post-remove`** — too late; the worktree is gone. See
  the warning above.
- **Pre-remove that aborts on failure** — overriding the `warn` default to
  `abort` means a stuck container or unreachable registry blocks worktree
  removal. Don't do this; the worktree directory cleanup is more important than
  perfect external cleanup.

## See also

- **[Lifecycle hooks → Removal](/hooks/lifecycle#removal-remove-hooks-only)** —
  `worktree-pre-remove` and `worktree-post-remove` env vars,
  `DAFT_REMOVAL_REASON` semantics
- **[Lifecycle hooks → Move](/hooks/lifecycle#move-move-hooks-only)** — why
  renames don't fire remove hooks, and how to gate cleanup
- **[Job orchestration](/hooks/job-orchestration)** — `parallel`, `piped`,
  `needs` for ordered teardowns
