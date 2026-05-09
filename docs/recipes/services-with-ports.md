---
title: Services with ports
description:
  Boot compose stacks per worktree without collision — deterministic naming,
  port allocation, host wiring.
pillars: [worktrees, hooks]
---

# Services with ports

> Per-worktree services are where daft's lifecycle automation pays off the most.
> Each worktree boots its own Postgres, Redis, MinIO — whatever the stack needs
> — with names and ports that don't collide with sibling worktrees. Two
> `docker compose up` invocations in two worktrees coexist; the dev server in
> feature-A talks to feature-A's database, not feature-B's.

## When to reach for this

- Your dev environment depends on services that maintain state (database, queue,
  object store) and you need each feature branch to have its own.
- You've hit "feature-A's tests trashed feature-B's data" or "port 5432 is
  already in use" at least once.
- You're using docker compose, podman, or running long-lived processes.

## Minimal recipe

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: install-deps
        run: pnpm install --frozen-lockfile
      - name: services-up
        run: docker compose up -d
        needs: [install-deps]
        env:
          COMPOSE_PROJECT_NAME: ${DAFT_REPO_NAME:-app}-${DAFT_BRANCH_NAME//\//-}
          POSTGRES_PORT: ${PORT_POSTGRES}
          REDIS_PORT: ${PORT_REDIS}

  worktree-pre-remove:
    jobs:
      - name: services-down
        run: docker compose down -v
        env:
          COMPOSE_PROJECT_NAME: ${DAFT_REPO_NAME:-app}-${DAFT_BRANCH_NAME//\//-}
```

Compose file:

```yaml
# compose.yaml
services:
  postgres:
    image: postgres:17
    ports: ["${POSTGRES_PORT}:5432"]
    volumes: [pgdata:/var/lib/postgresql/data]
  redis:
    image: redis:7
    ports: ["${REDIS_PORT}:6379"]
volumes:
  pgdata:
```

What's happening, piece by piece:

1. **`COMPOSE_PROJECT_NAME`** prefixes every container, network, and volume with
   `<repo>-<branch>`. Containers from feature-A land as
   `myapp-feature-a-postgres-1`; from feature-B as `myapp-feature-b-postgres-1`.
   No collisions.
2. **Port env vars** (`POSTGRES_PORT`, `REDIS_PORT`) are interpolated into the
   compose file. They come from somewhere reproducible per worktree (next
   section).
3. **Pre-remove tear-down** stops containers and **deletes their volumes**
   (`-v`) so a removed worktree leaves no orphaned data on disk.

## Allocating ports per worktree

Compose collisions happen at the host port. The fix: derive a port range
deterministically from the worktree, write it into `.envrc`, and let direnv
export it.

```yaml
- name: allocate-ports
  run: |
    # Hash the branch name to a stable per-worktree base port
    BASE=$((30000 + $(echo -n "$DAFT_BRANCH_NAME" | cksum | cut -d' ' -f1) % 1000 * 10))
    cat > .envrc <<EOF
    export PORT_POSTGRES=$BASE
    export PORT_REDIS=$((BASE + 1))
    export PORT_APP=$((BASE + 2))
    EOF
    direnv allow .
```

This gives every worktree a 10-port range starting at a stable offset.
`feature/auth` always lands on the same range; `feature/billing` on a different
one. No central registry, no race conditions on `daft start`, and dev URLs are
stable as you move between worktrees.

For background on the env-var seeding pattern, see
[Env vars & secrets → Per-worktree port via the branch name](/recipes/env-vars-and-secrets#per-worktree-port-via-the-branch-name).

## Variants

### Compose with profiles

Heavy stacks often want optional services (a search index, a message queue) that
not every dev needs all the time. Use compose profiles:

```yaml
# compose.yaml
services:
  postgres: { ... }
  meilisearch:
    image: getmeili/meilisearch:v1.13
    profiles: ["search"]
```

```yaml
# daft.yml — only boot search if SEARCH=1 in env
- name: services-up
  run: docker compose --profile search up -d
  only: { env: { SEARCH: "1" } }
```

### Podman

`podman compose` (or the podman-managed `docker-compose` shim) reads the same
compose files. Substitute `podman compose` for `docker compose` in the hook.
Podman runs rootless by default — port allocations under 1024 need extra config,
so stick to high ports (which we're doing anyway).

### Native processes (no containers)

Sometimes a heavy stack is overkill — a single Go service in dev mode is fine
running directly. Allocate a port and start it as a backgrounded job:

```yaml
- name: dev-server
  run: ./bin/myserver --port "$PORT_APP"
  background: true
  needs: [install-deps]
```

The pre-remove hook should kill the process — covered in
[Cleanup on remove](/recipes/cleanup-on-remove).

### Adopt-an-existing-stack

If your team already has a `docker-compose.yml` that doesn't use
`COMPOSE_PROJECT_NAME`, you can adopt it without changing the file — set
`COMPOSE_PROJECT_NAME` in the `env:` of the hook job (as above), or in
`mise.toml` `[env]`. Existing `docker compose` invocations from your shell still
need the var set, so direnv-loading it is usually the smoothest:

```bash
# .envrc, seeded by the hook
export COMPOSE_PROJECT_NAME="myapp-${DAFT_BRANCH_NAME//\//-}"
```

### Multiple compose files

Real projects often split compose across files (`compose.yaml` for core
services, `compose.dev.yaml` for dev-only overrides):

```yaml
- name: services-up
  run: docker compose -f compose.yaml -f compose.dev.yaml up -d
  env:
    COMPOSE_PROJECT_NAME: ${DAFT_REPO_NAME}-${DAFT_BRANCH_NAME//\//-}
```

`COMPOSE_FILE=compose.yaml:compose.dev.yaml` in `.envrc` is an alternative — it
lets bare `docker compose` commands from your shell pick up the same files
without needing the `-f` flags.

## Idempotency & safety

`docker compose up -d` is idempotent in the right ways:

- Already-running containers stay running.
- Stopped containers restart.
- Image pulls happen on first run, are skipped after.
- Volume mounts persist across restarts (good — the data survives hook re-runs).

But `docker compose down -v` is **destructive**: the `-v` flag deletes volumes.
That's correct for `worktree-pre-remove` (you want the worktree to leave nothing
behind), but it would be wrong for any hook that runs during normal worktree
life. Don't put `down -v` in `worktree-post-create` or anywhere that re-runs.

::: warning Don't share volumes across worktrees Naming collisions are solved by
`COMPOSE_PROJECT_NAME`, but if you ever name a volume **externally** with
`external: true`, two worktrees can both mount it — and corrupt each other's
data. See
[Anti-pattern: shared mutable state](/recipes/anti-patterns/shared-mutable-state).
:::

## Composes well with

- **[Toolchain bootstrap](/recipes/toolchain-bootstrap)** — install comes first,
  services follow. Use `needs:` to enforce order so the app's dev server has its
  deps before talking to fresh services.
- **[Env vars & secrets](/recipes/env-vars-and-secrets)** — port allocation
  lives there when it stands alone; here it's part of the full compose-aware
  picture.
- **[Cleanup on remove](/recipes/cleanup-on-remove)** — the pre-remove half of
  this pattern. Always paired; never just create services without tearing them
  down.
- **[CI parity](/recipes/ci-parity)** — the same compose file (with CI-side
  env-var injection) runs in CI too. `COMPOSE_PROJECT_NAME` is what makes
  parallel CI matrix runs not collide.

## Anti-patterns

- **No `COMPOSE_PROJECT_NAME`** — every worktree's containers fight for the
  default name (`<dirname>-<service>`), and `docker compose down` in worktree A
  can take out worktree B's containers if their dirnames collide.
- **External (shared) volumes** for mutable data — see warning above and
  [the dedicated anti-pattern page](/recipes/anti-patterns/shared-mutable-state).
- **Hardcoded ports in `compose.yaml`** — fine in CI (single project),
  catastrophic in dev (every worktree fighting for 5432). Always parameterize
  via env vars.
- **Skipping the pre-remove hook** — orphaned containers and volumes pile up;
  `docker system df` slowly grows; eventually you wonder why Docker Desktop is
  using 50 GB.

## See also

- **[Lifecycle hooks](/hooks/lifecycle)** — `worktree-post-create` and
  `worktree-pre-remove` reference; env vars passed to hooks
- **[Cleanup on remove](/recipes/cleanup-on-remove)** — the symmetric teardown
  pattern
- **[Sharing caches across worktrees](/recipes/sharing-caches)** — what IS safe
  to share between worktrees (image cache, image layers — both Docker handles
  automatically)
- **[Walkthroughs → Node monorepo with services](/recipes/walkthroughs/node-monorepo-services)**
  — this pattern threaded into a complete project setup
