---
title: Services with ports
description:
  Per-worktree compose stacks that don't collide — branch-named projects,
  branch-derived ports, automatic teardown.
pillars: [worktrees, hooks]
---

# Services with ports

## Starting state

A monorepo with a `compose.yaml` that reads:

```yaml
services:
  postgres:
    image: postgres:17
    ports: ["5432:5432"]
  redis:
    image: redis:7
    ports: ["6379:6379"]
```

It works fine — for one dev at a time. The README has a "before you start" line:
_"Stop your other compose stacks first."_

On a normal week that's tolerable. On a busy week with two parallel features it
isn't: you `daft start feature/auth` while `feature/billing`'s stack is still
up, and `docker compose up` errors with **"port 5432 already in use."** You add
`-p auth-stack`, override `POSTGRES_PORT=5433`, get it working — then tomorrow
you forget which port belongs to which worktree. Three days later you're tracing
a bug against the wrong database.

The reach for daft: every worktree gets its **own** compose stack, with its own
ports, named after its branch. Two parallel worktrees coexist; the dev server in
feature/auth talks to feature/auth's Postgres, not feature/billing's.

## What changes

`compose.yaml` stops hardcoding port numbers — they come from env vars. A
`worktree-post-create` job computes per-worktree ports from the branch name and
writes them into `.envrc`, where direnv loads them on `cd`. The same job sets
`COMPOSE_PROJECT_NAME`, which prefixes every container, network, and volume with
`<repo>-<branch>` so two stacks can coexist.

A symmetric `worktree-pre-remove` job tears it all down. The full teardown
semantics live in [Cleanup on remove](/recipes/cleanup-on-remove); this page
shows the minimum needed for the create-side to be safe.

## Recipe

Two `worktree-post-create` jobs (allocate ports, then boot services) plus the
matching teardown:

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: allocate-ports
        run: |
          BASE=$((30000 + $(echo -n "$DAFT_BRANCH_NAME" | cksum | cut -d' ' -f1) % 1000 * 10))
          cat > .envrc <<EOF
          export PORT_POSTGRES=$BASE
          export PORT_REDIS=$((BASE + 1))
          EOF
          direnv allow .

      - name: services-up
        run: docker compose up -d --wait
        needs: [allocate-ports]
        env:
          COMPOSE_PROJECT_NAME: ${DAFT_REPO_NAME:-app}-${DAFT_BRANCH_NAME//\//-}
          PORT_POSTGRES: ${PORT_POSTGRES}
          PORT_REDIS: ${PORT_REDIS}

  worktree-pre-remove:
    jobs:
      - name: services-down
        run: docker compose down -v --remove-orphans
        env:
          COMPOSE_PROJECT_NAME: ${DAFT_REPO_NAME:-app}-${DAFT_BRANCH_NAME//\//-}
```

`compose.yaml`:

```yaml
services:
  postgres:
    image: postgres:17
    ports: ["${PORT_POSTGRES}:5432"]
    volumes: [pgdata:/var/lib/postgresql/data]
  redis:
    image: redis:7
    ports: ["${PORT_REDIS}:6379"]
volumes:
  pgdata:
```

Piece by piece:

1. **`allocate-ports`** hashes `$DAFT_BRANCH_NAME` to a stable 10-port range
   starting at 30000–39990. `feature/auth` always lands on the same range;
   `feature/billing` lands on a different one. No central registry, no races.
   The result writes to `.envrc` so direnv exports the vars on the next `cd`.
2. **`services-up`** boots compose with `COMPOSE_PROJECT_NAME` set — the prefix
   that turns `postgres-1` into `myapp-feature-auth-postgres-1`, isolating
   containers, networks, and volumes per worktree. The per-job `env:` re-exports
   the ports because hooks don't inherit from `.envrc`.
3. **`--wait`** on `docker compose up` blocks until the containers report
   healthy, so the hook only completes when Postgres can actually accept
   connections.
4. **`services-down -v --remove-orphans`** is the symmetric pre-remove: stop
   containers, delete the worktree's volumes, sweep stragglers.

Two parallel worktrees now coexist. `daft start feature/billing` while
feature/auth is up gets a different port range, a different project name, and a
different set of volumes — no collisions, no manual overrides.

## Variants

By **runtime** — different ways to boot the same shape of stack.

### Compose profiles for optional services

Heavy stacks often want optional services (a search index, a message queue) that
not every dev needs all the time. Use compose profiles:

```yaml
# compose.yaml
services:
  postgres: { ... }
  meilisearch:
    image: getmeili/meilisearch:v1.13
    ports: ["${PORT_MEILI:-30099}:7700"]
    profiles: ["search"]
```

```yaml
# daft.yml — only boot search if SEARCH=1 in env
- name: services-up
  run: docker compose --profile search up -d --wait
  only: { env: { SEARCH: "1" } }
```

Devs who need search export `SEARCH=1` in their personal `mise.local.toml` or
shell rc; everyone else gets the lean stack.

### Podman

`podman compose` reads the same compose files. Substitute it for
`docker compose` in the hook. Podman runs rootless by default — port allocations
under 1024 need extra config, so stick to high ports (which the recipe is
already doing).

### Native processes (no containers)

Sometimes a heavy stack is overkill. A single Go service in dev mode is fine
running directly. Allocate a port, start the process as a backgrounded job:

```yaml
- name: dev-server
  run: ./bin/myserver --port "$PORT_APP"
  background: true
  needs: [install-deps, allocate-ports]
```

The pre-remove hook should kill the process — covered in
[Cleanup on remove → native processes by PID file](/recipes/cleanup-on-remove#native-processes-by-pid-file).

### Multi-file compose

Real projects often split compose across files (`compose.yaml` for core
services, `compose.dev.yaml` for dev-only overrides):

```yaml
- name: services-up
  run: docker compose -f compose.yaml -f compose.dev.yaml up -d --wait
  env:
    COMPOSE_PROJECT_NAME: ${DAFT_REPO_NAME}-${DAFT_BRANCH_NAME//\//-}
```

Setting `COMPOSE_FILE=compose.yaml:compose.dev.yaml` in `.envrc` is an
alternative — bare `docker compose` commands from your shell pick up the same
files without needing `-f` every time.

### Adopting an existing stack

If your team already has `compose.yaml` without `COMPOSE_PROJECT_NAME`, adopt it
without changing the file: set the var in the hook's `env:` (as the Recipe does)
and additionally seed it into `.envrc` so interactive `docker compose` commands
pick it up too:

```bash
# .envrc — seeded by allocate-ports
export COMPOSE_PROJECT_NAME="myapp-${DAFT_BRANCH_NAME//\//-}"
```

## Idempotency & safety

`docker compose up -d` is idempotent in the right ways:

- Already-running containers stay running
- Stopped containers restart
- Image pulls happen on first run, skipped after
- Named volumes persist across restarts (so the data survives a hook re-run,
  which is what you want)

`docker compose down -v` is **destructive**: the `-v` flag deletes volumes.
That's correct in `worktree-pre-remove` (the worktree should leave nothing
behind), and **wrong** anywhere that re-runs during normal worktree life. Don't
put `down -v` in `worktree-post-create` or in any hook that fires more than
once.

::: warning Don't share volumes across worktrees

`COMPOSE_PROJECT_NAME` solves naming collisions. But if a volume is declared
`external: true` with a fixed name, two worktrees can both mount it — and
corrupt each other's data. Postgres won't recover from that gracefully. See
[Anti-pattern: shared mutable state](/recipes/anti-patterns/shared-mutable-state).

:::

## Where to next

- **[Cleanup on remove](/recipes/cleanup-on-remove)** — the symmetric pre-remove
  pattern, plus what to do when teardown isn't just a `compose down` (PID files,
  ports, external registries).
- **[Env vars & secrets](/recipes/env-vars-and-secrets)** — the deeper
  port-allocation story (and where the branch-name-hash idea comes from).
- **[Walkthroughs → Node monorepo with services](/recipes/walkthroughs/node-monorepo-services)**
  — this pattern threaded into a complete project setup, with migrations,
  multiple services, and DATABASE_URL wiring.
