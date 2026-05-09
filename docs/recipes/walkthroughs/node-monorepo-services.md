---
title: Node monorepo with services
description:
  Threading toolchain-bootstrap, services-with-ports, and cleanup-on- remove
  into a real Node monorepo with Postgres, Redis, MinIO, and per-worktree port
  allocation.
pillars: [worktrees, hooks]
---

# Node monorepo with services

## Starting state

A pnpm-workspace monorepo:

```
my-app/
├── apps/
│   ├── web/              # Next.js, talks to /api
│   └── api/              # Express, hits Postgres + Redis
├── packages/
│   ├── db/               # Drizzle schema + migrations
│   └── ui/               # shared React components
├── compose.yaml          # postgres + redis + minio, hardcoded ports
├── pnpm-workspace.yaml
├── package.json
└── pnpm-lock.yaml
```

The setup ritual the README describes:

1. `cp .env.example .env`, then edit `DATABASE_URL` to point at local Postgres.
2. `pnpm install --frozen-lockfile`
3. `docker compose up -d`
4. `pnpm --filter @app/db migrate:latest`
5. `pnpm dev`

Half the time someone forgets step 4 and the dev server crashes on first
request. Sooner or later someone hits **port 5432 already in use** because
`feature/auth`'s compose stack is up while they're trying to start
`feature/billing`.

Parallel worktrees can't coexist without a coordination conversation about whose
compose stack is on which port.

This walkthrough threads four patterns into one `daft.yml`:

- **[Toolchain bootstrap](/recipes/toolchain-bootstrap)** —
  `pnpm install --frozen-lockfile` per worktree, with the pnpm store shared
  across worktrees.
- **[Env vars & secrets](/recipes/env-vars-and-secrets#per-worktree-derived-values)**
  — per-worktree ports and `DATABASE_URL` written into `.envrc`.
- **[Services with ports](/recipes/services-with-ports)** — compose stack per
  worktree, branch-named, isolated volumes.
- **[Cleanup on remove](/recipes/cleanup-on-remove)** — symmetric teardown when
  the worktree goes away.

By the end: `daft start feature/x` returns with a fully-set-up worktree —
`node_modules/`, dedicated services, fresh-migrated DB. Parallel feature
branches just work.

## Step 1: install deps

Apply [Toolchain bootstrap](/recipes/toolchain-bootstrap) for the pnpm case:

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: install-deps
        run: pnpm install --frozen-lockfile
```

Configure pnpm's store to be shared once, machine-wide (so each worktree's
`node_modules/` is hardlinks into one store, not duplicated disk):

```bash
pnpm config set store-dir ~/.pnpm-store
```

```bash
git add daft.yml pnpm-lock.yaml
git commit -m "chore(daft): install workspace deps on worktree create"
git daft-hooks trust

daft start feature/scratch
ls node_modules        # populated, hardlink-fast
pnpm test              # works against this worktree's deps
```

::: tip Don't share `node_modules/` directly

Sharing the pnpm **store** is safe (content-addressed, immutable). Sharing
`node_modules/` itself is a corruption hazard — see
[Anti-pattern: shared mutable state](/recipes/anti-patterns/shared-mutable-state).

:::

## Step 2: allocate ports + DATABASE_URL

Two worktrees both running `pnpm dev` would fight for port 3000. Their backing
services would fight for 5432, 6379, 9000. Apply the branch-name-hash idea from
[Env vars & secrets](/recipes/env-vars-and-secrets#per-worktree-derived-values)
— and write `DATABASE_URL` at the same time, derived from the port:

```yaml
# daft.yml
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
    export DATABASE_URL="postgres://dev:dev@localhost:\$PORT_POSTGRES/app"
    EOF
    direnv allow .

- name: install-deps
  run: pnpm install --frozen-lockfile
  needs: [allocate-ports]
```

Two things are happening here that are easy to miss:

1. The `.envrc` is generated, not committed — it's per-worktree and gitignored
   (`echo .envrc >> .gitignore` once).
2. `DATABASE_URL` references `\$PORT_POSTGRES` (note the backslash) so the
   `.envrc` contains a literal `$PORT_POSTGRES` reference. When direnv loads the
   file, the shell expands it against the already-exported `PORT_POSTGRES`. App
   code reads `DATABASE_URL` once; you don't have to build it from PORT_POSTGRES
   every time.

Update `apps/web/next.config.js` and `apps/api/src/server.ts` to read their port
from `process.env.PORT_WEB` / `PORT_API`.

## Step 3: boot the services

Wire up the compose stack. The compose file uses env-var interpolation for
ports; `COMPOSE_PROJECT_NAME` keeps containers namespaced. This is
[Services with ports](/recipes/services-with-ports) applied to the project's
three-service stack:

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

```yaml
# daft.yml — add to worktree-post-create
- name: services-up
  run: docker compose up -d --wait
  needs: [allocate-ports, install-deps]
  env:
    COMPOSE_PROJECT_NAME: ${DAFT_REPO_NAME:-app}-${DAFT_BRANCH_NAME//\//-}
    PORT_POSTGRES: ${PORT_POSTGRES}
    PORT_REDIS: ${PORT_REDIS}
    PORT_MINIO: ${PORT_MINIO}
```

`COMPOSE_PROJECT_NAME` makes feature-A's containers `app-feature-a-postgres-1`
and feature-B's `app-feature-b-postgres-1` — entirely separate. `--wait` blocks
until Postgres reports healthy, so `services-up` only completes when the DB can
actually accept connections.

## Step 4: run migrations

Services are up but the database is empty. Run migrations as the final
synchronous step:

```yaml
- name: migrate
  run: pnpm --filter @app/db migrate:latest
  needs: [services-up]
```

`DATABASE_URL` already exports from `.envrc`, but daft job env doesn't inherit
from direnv — re-export it via the job's `env:` if your migrate script needs it:

```yaml
- name: migrate
  run: pnpm --filter @app/db migrate:latest
  needs: [services-up]
  env:
    DATABASE_URL: postgres://dev:dev@localhost:${PORT_POSTGRES}/app
```

For seed data, add a `needs: [migrate]` job after this one.

## Step 5: cleanup on remove

The reverse, applying [Cleanup on remove](/recipes/cleanup-on-remove):

```yaml
# daft.yml — add a new top-level hook
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

```bash
daft remove feature/scratch
docker ps -a --filter name=app-feature-scratch    # empty
docker volume ls --filter name=app-feature-scratch  # empty
```

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
          PORT_POSTGRES: ${PORT_POSTGRES}
          PORT_REDIS: ${PORT_REDIS}
          PORT_MINIO: ${PORT_MINIO}

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

Four post-create jobs (with `needs:` enforcing order) and one pre-remove. The
same config applies to every dev on the team, every worktree.

## What you got

Before:

- `git checkout feature/x` → `pnpm install` (slow because lockfile drifted) →
  `docker compose up` (fails: port already in use) → manual port-edit → fight
  with database state → forget to run migrations → "works on my machine"
  debugging.
- "Two devs on parallel features" wasn't really possible without a 30-minute
  setup conversation.
- The README's five steps had to be repeated to every newcomer.

After:

- `daft start feature/x` returns with the worktree fully wired: `node_modules`
  from the shared pnpm store, dedicated postgres / redis / minio on isolated
  ports, fresh migrations applied, ready for `pnpm dev`.
- Parallel feature branches just work — no port coordination, no shared dev DB,
  no merge conflicts in `compose.yaml`.
- `daft remove feature/x` (or `daft prune`) leaves nothing on disk: no orphaned
  containers, no leaked volumes, no zombie ports.

## Where to next

- **[Services with ports](/recipes/services-with-ports)** — for compose
  profiles, podman, multi-file compose, and the variants this walkthrough didn't
  use.
- **[CI parity](/recipes/ci-parity)** — running this same `daft.yml` in CI for
  integration tests with the same compose stack.
- **[Walkthroughs → Python/uv with mise + sops](/recipes/walkthroughs/python-uv-secrets)**
  — a different shape: declarative env layered with imperative hook-fetched
  secrets.
