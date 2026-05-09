---
title: Node monorepo with services
description:
  End-to-end daft setup for a Node monorepo with docker-compose services — pnpm
  install, port allocation per worktree, cleanup on remove.
pillars: [worktrees, hooks]
---

# Node monorepo with services

This walkthrough sets up a real-world Node.js monorepo where each daft worktree
gets:

1. Its own `node_modules/` (shared via pnpm's store, not duplicated on disk).
2. Its own per-worktree Postgres, Redis, and S3-compatible store (MinIO) on
   stable, collision-free ports.
3. A `.envrc` seeded with everything the dev server needs — DATABASE_URL
   pointing at this worktree's Postgres, ports for the dev server.
4. Full teardown when the worktree is removed — containers stopped, volumes
   deleted, ports released.

Two devs working on `feature/billing` and `feature/auth` get two fully isolated
stacks, no port collisions, no shared databases. This is where daft's lifecycle
automation pays off most.

## What you're building

A pnpm-workspace monorepo with shape:

```
my-app/
├── apps/
│   ├── web/         # Next.js, talks to the API
│   └── api/         # Express, talks to Postgres + Redis
├── packages/
│   ├── db/          # Drizzle schema + migrations
│   └── ui/          # Shared React components
├── compose.yaml     # postgres + redis + minio
├── pnpm-workspace.yaml
├── package.json
└── pnpm-lock.yaml
```

The dev experience target: `daft start feature/x`, wait a moment, `pnpm dev`
runs and everything works.

## Patterns used

- **[Toolchain bootstrap](/recipes/toolchain-bootstrap)** —
  `pnpm install --frozen-lockfile` per worktree, with the pnpm store shared.
- **[Services with ports](/recipes/services-with-ports)** — compose stack per
  worktree with `COMPOSE_PROJECT_NAME` and per-worktree ports.
- **[Cleanup on remove](/recipes/cleanup-on-remove)** — symmetric teardown:
  `docker compose down -v`.
- **[Env vars & secrets](/recipes/env-vars-and-secrets)** — port allocation
  written into `.envrc`.

## Step 1: install dependencies

Start with the simplest possible hook:

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: install-deps
        run: pnpm install --frozen-lockfile
```

`--frozen-lockfile` is non-negotiable — without it, two worktrees created in
close succession can rewrite `pnpm-lock.yaml` differently, and the next
`git pull` sees mysterious lockfile churn.

Configure pnpm's store to be shared (one-time, machine-wide):

```bash
pnpm config set store-dir ~/.pnpm-store
```

This means each worktree's `node_modules/` is a directory of hardlinks into a
single content-addressable store. Disk usage stays roughly constant as you add
worktrees.

```bash
git add daft.yml pnpm-lock.yaml
git commit -m "chore(daft): install workspace deps on worktree create"
git daft-hooks trust

daft start feature/scratch
# → cd into new worktree
ls node_modules        # populated, ~hardlink-fast
pnpm test              # works against this worktree's deps
```

::: tip Don't share `node_modules/` directly Sharing the pnpm **store** is safe
(content-addressed, immutable). Sharing `node_modules/` itself is a corruption
hazard — see
[Anti-pattern: shared mutable state](/recipes/anti-patterns/shared-mutable-state).
:::

## Step 2: allocate ports per worktree

Two worktrees both running `pnpm dev` would fight for port 3000. And their
backing services would fight for 5432 (Postgres), 6379 (Redis), 9000 (MinIO).
Derive a stable port range from the branch name:

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: allocate-ports
        run: |
          BASE=$((30000 + $(echo -n "$DAFT_BRANCH_NAME" | cksum | cut -d' ' -f1) % 1000 * 10))
          cat > .envrc <<EOF
          # Allocated by daft for branch $DAFT_BRANCH_NAME
          export PORT_WEB=$BASE
          export PORT_API=$((BASE + 1))
          export PORT_POSTGRES=$((BASE + 2))
          export PORT_REDIS=$((BASE + 3))
          export PORT_MINIO=$((BASE + 4))
          EOF
          direnv allow .

      - name: install-deps
        run: pnpm install --frozen-lockfile
        needs: [allocate-ports]
```

The hash gives every branch a stable 10-port range. `feature/billing` always
lands on the same range; sibling worktrees never collide. `direnv` activates the
vars when you `cd` into the worktree.

Update `apps/web/next.config.js` and `apps/api/src/server.ts` to read their port
from `process.env.PORT_WEB` / `process.env.PORT_API`.

## Step 3: boot the services

Now wire up the compose stack. The compose file uses env-var interpolation for
ports, and `COMPOSE_PROJECT_NAME` keeps containers namespaced:

```yaml
# compose.yaml
services:
  postgres:
    image: postgres:17
    ports: ["${PORT_POSTGRES}:5432"]
    environment:
      POSTGRES_USER: dev
      POSTGRES_PASSWORD: dev
      POSTGRES_DB: app
    volumes: [pgdata:/var/lib/postgresql/data]
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U dev"]
      interval: 5s

  redis:
    image: redis:7
    ports: ["${PORT_REDIS}:6379"]

  minio:
    image: minio/minio:latest
    command: server /data --console-address ':9001'
    ports: ["${PORT_MINIO}:9000"]
    environment:
      MINIO_ROOT_USER: dev
      MINIO_ROOT_PASSWORD: devsecret
    volumes: [miniodata:/data]

volumes:
  pgdata:
  miniodata:
```

Add the boot job:

```yaml
# daft.yml — add to worktree-post-create
- name: services-up
  run: docker compose up -d --wait
  needs: [allocate-ports, install-deps]
  env:
    COMPOSE_PROJECT_NAME: ${DAFT_REPO_NAME:-app}-${DAFT_BRANCH_NAME//\//-}
```

`COMPOSE_PROJECT_NAME` prefixes every container, network, and volume. Two
worktrees end up with `app-feature-billing-postgres-1` and
`app-feature-auth-postgres-1` — totally separate containers, totally separate
volumes.

`--wait` blocks until containers report healthy, so the hook completes only when
Postgres can actually accept connections.

::: warning DATABASE_URL belongs in `.envrc`, not the compose file Step 2's
`allocate-ports` writes the ports. Add a step that writes the worktree's
DATABASE_URL too:

```bash
echo "export DATABASE_URL=postgres://dev:dev@localhost:\$PORT_POSTGRES/app" >> .envrc
```

This way your app code reads `DATABASE_URL` once and doesn't have to build it
from PORT_POSTGRES every time. :::

## Step 4: run migrations

Services are up but the database is empty. Run migrations as part of the
bootstrap:

```yaml
- name: migrate
  run: pnpm --filter @app/db migrate:latest
  needs: [services-up]
  env:
    DATABASE_URL: postgres://dev:dev@localhost:${PORT_POSTGRES}/app
```

Each worktree gets a freshly-migrated DB on creation. No more "did I remember to
run migrations?" — the hook did, every time.

For seed data, add a job after `migrate` with `needs: [migrate]`.

## Step 5: cleanup on remove

The reverse:

```yaml
# daft.yml — add new top-level hook
hooks:
  # ... worktree-post-create as above ...
  worktree-pre-remove:
    jobs:
      - name: services-down
        run: docker compose down -v --remove-orphans
        env:
          COMPOSE_PROJECT_NAME: ${DAFT_REPO_NAME:-app}-${DAFT_BRANCH_NAME//\//-}
```

`-v` deletes the worktree's volumes (pgdata, miniodata). `--remove-orphans`
catches any container the team added later that the running stack doesn't know
about.

Test it:

```bash
daft remove feature/scratch
# Containers stopped, volumes gone. Disk freed.
```

::: warning Don't put cleanup in `worktree-post-remove` The worktree directory
(and its `compose.yaml`) is gone in post-remove. Pre-remove is the last chance
to read the worktree's compose file. See
[Cleanup on remove → don't put cleanup in post-remove](/recipes/cleanup-on-remove#idempotency-safety).
:::

## Step 6: optional — parallelize with profiles

Heavy stacks (search index, vector DB, billing emulator) only some devs need.
Use compose profiles + per-job `only:` conditions:

```yaml
# compose.yaml
services:
  meilisearch:
    image: getmeili/meilisearch:v1.13
    ports: ["${PORT_MEILI:-30099}:7700"]
    profiles: ["search"]
```

```yaml
# daft.yml
- name: services-up
  run: docker compose --profile search up -d --wait
  only:
    env: { ENABLE_SEARCH: "1" }
- name: services-up-default
  run: docker compose up -d --wait
  skip:
    env: { ENABLE_SEARCH: "1" }
```

Devs who need search export `ENABLE_SEARCH=1` (in their personal
`mise.local.toml` or shell rc); everyone else gets the lean stack.

## Final `daft.yml`

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: allocate-ports
        run: |
          BASE=$((30000 + $(echo -n "$DAFT_BRANCH_NAME" | cksum | cut -d' ' -f1) % 1000 * 10))
          cat > .envrc <<EOF
          export PORT_WEB=$BASE
          export PORT_API=$((BASE + 1))
          export PORT_POSTGRES=$((BASE + 2))
          export PORT_REDIS=$((BASE + 3))
          export PORT_MINIO=$((BASE + 4))
          export DATABASE_URL="postgres://dev:dev@localhost:\$PORT_POSTGRES/app"
          EOF
          direnv allow .

      - name: install-deps
        run: pnpm install --frozen-lockfile
        needs: [allocate-ports]

      - name: services-up
        run: docker compose up -d --wait
        needs: [allocate-ports, install-deps]
        env:
          COMPOSE_PROJECT_NAME: ${DAFT_REPO_NAME:-app}-${DAFT_BRANCH_NAME//\//-}

      - name: migrate
        run: pnpm --filter @app/db migrate:latest
        needs: [services-up]
        env:
          DATABASE_URL: postgres://dev:dev@localhost:${PORT_POSTGRES}/app

  worktree-pre-remove:
    jobs:
      - name: services-down
        run: docker compose down -v --remove-orphans
        env:
          COMPOSE_PROJECT_NAME: ${DAFT_REPO_NAME:-app}-${DAFT_BRANCH_NAME//\//-}
```

Four post-create jobs (with `needs:` enforcing order) and one pre-remove. Every
dev on the team gets the same setup; every worktree is fully isolated.

## What you got

Before:

- `git checkout feature/x` → `pnpm install` (slow because lockfile drifted) →
  `docker compose up` (fails: port already in use) → manually edit ports → fight
  with database state → forget to run migrations → "works on my machine"
  debugging session.
- "Two devs running parallel features" wasn't really possible without a
  30-minute setup conversation.

After:

- `daft start feature/x` → ~30 seconds for cold create (mostly compose pulls),
  instant for warm — and you're in a worktree with its own postgres, its own
  redis, its own ports, freshly migrated.
- Two devs on parallel features just work. No port coordination, no shared dev
  DB, no merge conflicts in `compose.yaml`.
- `daft remove feature/x` (or `daft prune`) leaves nothing on disk: no orphaned
  containers, no leaked volumes, no zombie ports.

## Where to next

- **[Services with ports](/recipes/services-with-ports)** — full reference for
  the compose pattern, including profiles, podman, and multi-file compose.
- **[Cleanup on remove](/recipes/cleanup-on-remove)** — pre-remove semantics,
  `DAFT_REMOVAL_REASON`, and per-removal-reason logic.
- **[CI parity](/recipes/ci-parity)** — running this same `daft.yml` in CI for
  integration tests with the same compose stack.
- **[Sharing caches across worktrees](/recipes/sharing-caches)** — pnpm store,
  Turborepo cache, and what's safe to share.
- **[Anti-pattern: shared mutable state](/recipes/anti-patterns/shared-mutable-state)**
  — why sharing `node_modules` directly (rather than the pnpm store) is a
  corruption hazard.
