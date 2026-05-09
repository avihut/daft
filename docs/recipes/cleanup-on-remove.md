---
title: Cleanup on remove
description:
  The symmetric mirror of worktree-post-create — tear down services, release
  ports, deregister state when a worktree goes away.
pillars: [worktrees, hooks]
---

# Cleanup on remove

## Starting state

Your [Services with ports](/recipes/services-with-ports) recipe boots a compose
stack per worktree — that part works fine. Worktrees come and go.

Today's housekeeping turn:

```bash
$ docker ps -a | wc -l
      23
$ docker volume ls | wc -l
      14
$ du -sh ~/Library/Containers/com.docker.docker/Data/vms/0/data
12G
```

Twenty-three stopped containers from worktrees that don't exist anymore.
Fourteen volumes (postgres, redis, minio) that nobody owns. Twelve gigabytes on
disk that nobody asked for. Add to that a half-released port from a backgrounded
Node dev server — `lsof -i :3000` finds a process from a feature branch that no
longer exists.

The create hook starts services. There's no symmetric pre-remove hook, so when
the worktree directory got deleted, `docker compose` was never told. Containers
stop but stay around. Volumes hang on. Ports leak.

The reach for daft: **every resource a worktree creates needs a path back out.**
If your post-create starts a service, registers a webhook, allocates a port, or
writes to a global registry, the pre-remove hook puts each of those back.

## What changes

A `worktree-pre-remove` hook becomes the symmetric mirror of
`worktree-post-create`. It runs **before** the worktree directory is deleted,
while the hook still has access to `compose.yaml`, `.envrc`, and any
per-worktree files. Whatever the create hook brought into existence, this hook
unmakes.

If your post-create only does work confined to the worktree directory
(`pnpm install`, `cargo fetch`), you can skip this pattern. The directory delete
is the cleanup.

## Recipe

The pairing for `services-with-ports`:

```yaml
# daft.yml
hooks:
  worktree-pre-remove:
    jobs:
      - name: services-down
        run: docker compose down -v --remove-orphans
        env:
          COMPOSE_PROJECT_NAME: ${DAFT_REPO_NAME:-app}-${DAFT_BRANCH_NAME//\//-}
```

`-v` deletes the worktree's volumes (postgres data, redis dump, MinIO buckets).
`--remove-orphans` catches containers from compose files that were edited or
removed while the stack was up.

The default fail mode is `warn`: a failed teardown logs but doesn't block
worktree removal. That's the right default — a stuck container shouldn't prevent
the directory from being deleted, because next time you'd just have a stuck
container _and_ a half-removed worktree.

`DAFT_REMOVAL_REASON` is set by the runtime to `manual`, `remote-deleted`, or
`ejecting` — useful when you want different behavior for an auto-prune cleanup
vs an explicit `daft remove`. See the per-removal-reason variant below.

## Variants

By **resource type** — what specifically needs cleanup.

### Native processes by PID file

If `worktree-post-create` started a long-lived process outside compose (a
backgrounded Node dev server, a Go binary), track it via a PID file inside the
worktree:

```yaml
# In worktree-post-create:
- name: dev-server
  run: |
    ./bin/myserver --port "$PORT_APP" &
    echo $! > .daft/dev-server.pid
  background: true
```

```yaml
# In worktree-pre-remove:
- name: stop-dev-server
  run: |
    if [ -f .daft/dev-server.pid ]; then
      kill "$(cat .daft/dev-server.pid)" 2>/dev/null || true
    fi
```

The `|| true` keeps the hook from failing if the process already died (the
common case — most teardowns find the process already gone).

### Cleanup by port

If you can't get a PID, fall back to killing whoever holds the port:

```yaml
- name: free-port
  run: |
    lsof -ti tcp:"$PORT_APP" | xargs -r kill 2>/dev/null || true
```

`lsof -ti` lists PIDs holding the port; `xargs -r` skips the kill if the list is
empty. Works on macOS and Linux.

### External registry deregistration

If `worktree-post-create` registered the worktree somewhere external — a Consul
KV entry, a webhook URL, a CDN purge config — deregister it on remove:

```yaml
- name: consul-deregister
  run: consul kv delete "dev/$DAFT_BRANCH_NAME" || true

- name: cdn-purge
  run: curl -X DELETE "$CDN_API/zones/dev-${DAFT_BRANCH_NAME//\//-}" || true
```

The trailing `|| true` matters — the entry might already be gone (a prior
cleanup attempt, an external sync), and you'd rather have a failed-but-completed
cleanup than a half-removed worktree blocked on a stale 404.

### Per-removal-reason logic

`DAFT_REMOVAL_REASON` lets you handle different removal contexts differently:

```yaml
- name: archive-state
  run: |
    case "$DAFT_REMOVAL_REASON" in
      remote-deleted)
        # Branch was deleted on remote — archive any uncommitted experiments
        tar czf "$HOME/daft-archives/$(basename "$DAFT_WORKTREE_PATH").tar.gz" .
        ;;
      manual|ejecting)
        # User-driven removal — skip archive (they meant it)
        ;;
    esac
```

Reason values:

- `remote-deleted` — auto-detected by `daft prune` / `daft sync`
- `manual` — explicit `daft remove`
- `ejecting` — the worktree is being un-managed by daft, not deleted

See [Lifecycle hooks → Removal](/hooks/lifecycle#removal-remove-hooks-only) for
the full table.

## Idempotency & safety

The hard rule: **`worktree-pre-remove`, never `worktree-post-remove`, for
anything that needs the worktree's files**.

::: warning Cleanup goes in `pre-remove`, not `post-remove`
`worktree-post-remove` runs **after** the worktree directory is gone. By that
point `compose.yaml`, `.envrc`, PID files — all unreachable. Pre-remove is the
last chance to read worktree-local state. Post-remove is for genuinely global
cleanup that doesn't need the worktree's files (an external log shipper, a
metrics flush). If your cleanup touches _anything_ inside the worktree, it
belongs in pre-remove. :::

Pre-remove jobs run in parallel by default — most teardowns are independent
(containers, registry entries, log shipping). If teardown ordering matters (must
shut down the app server before the database), use `piped: true` or `needs:`.

Cleanup commands should tolerate already-clean state:

| Pattern                                        | Idempotent?        |
| ---------------------------------------------- | ------------------ |
| `docker compose down` (containers don't exist) | yes — exits 0      |
| `kill $(cat .pid)` (PID gone)                  | no — non-zero exit |
| `kill $(cat .pid) 2>/dev/null \|\| true`       | yes                |
| `consul kv delete` (entry gone)                | no — non-zero exit |
| `consul kv delete ... \|\| true`               | yes                |

The shape: every cleanup command gets `|| true` (or stderr suppression), unless
a non-zero exit _genuinely_ indicates a real problem worth surfacing.

`daft rename` triggers move hooks, not remove hooks. If your cleanup deletes
data that should survive a rename, gate it explicitly:

```yaml
- name: services-down
  run: docker compose down -v
  skip:
    env: { DAFT_IS_MOVE: "true" }
```

See [Lifecycle hooks → Move](/hooks/lifecycle#move-move-hooks-only).

## Where to next

- **[Services with ports](/recipes/services-with-ports)** — the paired
  create-side. The two are always written together; never just one.
- **[Lifecycle hooks → Removal](/hooks/lifecycle#removal-remove-hooks-only)** —
  full reference for `DAFT_REMOVAL_REASON`, `DAFT_IS_MOVE`, and the difference
  between pre-remove and post-remove.
- **[Job orchestration](/hooks/job-orchestration)** — `parallel`, `piped`,
  `needs` for ordered teardowns when sequence matters.
