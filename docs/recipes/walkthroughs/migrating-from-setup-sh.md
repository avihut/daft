---
title: Migrating from a bin/setup.sh ritual
description:
  Move a project's first-time setup script into per-worktree daft hooks — same
  operations, no manual run, with cleanup on remove that setup.sh never had.
pillars: [worktrees, hooks]
---

# Migrating from a bin/setup.sh ritual

## Starting state

A pnpm-workspace project with a setup script everyone runs after a fresh clone
or a major `git pull`:

```
my-app/
├── bin/setup.sh
├── compose.yaml
├── mise.toml
├── package.json
├── pnpm-lock.yaml
├── pnpm-workspace.yaml
└── scripts/codegen.sh
```

The script itself:

```bash
#!/usr/bin/env bash
# bin/setup.sh — first-time setup
set -e

echo "==> Installing toolchain..."
mise install

echo "==> Installing deps..."
pnpm install --frozen-lockfile

echo "==> Booting services..."
docker compose up -d --wait

echo "==> Generating types..."
./scripts/codegen.sh

echo "==> Running migrations..."
pnpm --filter @app/db migrate:latest

echo "==> Done. You can now: pnpm dev"
```

The README's "Getting started" section is one line: _"First time? Run
`bin/setup.sh`."_

The ritual: clone, run setup.sh, wait several minutes, hopefully nothing fails
partway through. When `package.json` or `compose.yaml` changes after a pull, run
it again. The pains compound:

- `setup.sh` is _imperative_. Re-running it after an aborted run leaves
  half-set-up state — orphaned containers, half-installed dependencies, the
  migrate step erroring against an already-partial schema.
- `setup.sh` and the README drift. Newcomers grep for "setup" and find the
  script; the README's "what to run first" line lags behind it for months.
  Whoever joined the project on the wrong week gets a stale view.
- Worktrees multiply the cost. Every new branch is a fresh setup.sh run; the
  team learns to start it before coffee.
- There's no symmetric teardown. When a worktree is removed, its compose
  containers and named volumes leak.

The reach for daft: stop maintaining a setup script. Worktree creation itself
runs the setup; worktree removal runs the teardown setup.sh never had.

## Patterns we'll thread

The five sections of `setup.sh` map to four patterns:

- **[Toolchain bootstrap](/recipes/toolchain-bootstrap)** — `mise install`
  - `pnpm install` (Step 1).
- **[Services with ports](/recipes/services-with-ports)** —
  `docker compose up` + `pnpm migrate` (Step 2).
- **[Background warmup](/recipes/background-warmup)** — `scripts/codegen.sh`
  (Step 3).
- **[Cleanup on remove](/recipes/cleanup-on-remove)** — the symmetric teardown
  setup.sh never had (Step 4).

By the end: `bin/setup.sh` is deleted, the README's setup section is two lines,
and `daft.yml` is the source of truth.

## Step 1: install tools and deps

Apply the [Toolchain bootstrap](/recipes/toolchain-bootstrap) pattern in its
pnpm shape. The two install steps from `setup.sh` become two jobs with `needs:`
enforcing the order pnpm-from-mise requires:

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: install-tools
        run: mise install

      - name: install-deps
        run: pnpm install --frozen-lockfile
        needs: [install-tools]
```

`mise install` first because pnpm itself comes from mise. `--frozen-lockfile`
because reproducibility is the whole point — the strict variant is what you want
in a hook (see the pattern's _Idempotency & safety_ table for the comparison).

`bin/setup.sh` shrinks:

```bash
#!/usr/bin/env bash
# bin/setup.sh
set -e

echo "==> Booting services..."
docker compose up -d --wait

echo "==> Generating types..."
./scripts/codegen.sh

echo "==> Running migrations..."
pnpm --filter @app/db migrate:latest
```

The README still says "first time, run `bin/setup.sh`" — for now.

## Step 2: services and migrations

Apply [Services with ports](/recipes/services-with-ports). The compose stack and
the migration step go together: services come up, the DB is migrated,
`COMPOSE_PROJECT_NAME` namespaces containers and volumes by branch.

```yaml
# daft.yml — append to worktree-post-create
- name: services-up
  run: docker compose up -d --wait
  needs: [install-deps]
  env:
    COMPOSE_PROJECT_NAME: ${DAFT_REPO_NAME:-app}-${DAFT_BRANCH_NAME//\//-}

- name: migrate
  run: pnpm --filter @app/db migrate:latest
  needs: [services-up]
```

`COMPOSE_PROJECT_NAME` is the upgrade `setup.sh` couldn't do without out-of-band
coordination: every worktree's containers, networks, and volumes are prefixed
with the branch name, so `dev` containers don't pollute `master`.

::: tip Parallel worktrees if compose hardcodes ports

If `compose.yaml` still has `5432:5432`, two worktrees can't both have Postgres
up — host-side ports collide. You get container-name isolation but not port
isolation. See
[Services with ports → Adopt-existing](/recipes/services-with-ports#adopt-existing)
for the upgrade path: port-variable-ize `compose.yaml`, allocate per-worktree
ports from the branch hash, and parallel worktrees coexist.

:::

`bin/setup.sh` shrinks again:

```bash
#!/usr/bin/env bash
# bin/setup.sh
set -e

echo "==> Generating types..."
./scripts/codegen.sh
```

One section left.

## Step 3: codegen as background

Apply [Background warmup](/recipes/background-warmup) for the codegen step.
`scripts/codegen.sh` produces TypeScript types from a GraphQL schema; the dev
server consumes them when it starts, but the worktree is usable for unrelated
commands (running tests in a different package, opening files, reading code)
immediately. Backgrounding gets `daft start` to return faster.

```yaml
# daft.yml — append to worktree-post-create
- name: codegen
  run: ./scripts/codegen.sh
  needs: [install-deps]
  background: true
```

`needs: [install-deps]` because codegen imports from `node_modules`. No
`needs: [services-up]` because the schema is a static file; if your codegen
reads from a running service, swap that in and consider whether backgrounding
still makes sense.

`bin/setup.sh` is now empty. Delete it:

```bash
git rm bin/setup.sh
```

Update the README:

````markdown
## Getting started

```bash
daft start feature/yours
pnpm dev
```
````

````

Two lines. The "First time? Run bin/setup.sh" instruction is gone — the
`daft start` command runs it.

## Step 4: cleanup on remove

The reverse, applying [Cleanup on remove](/recipes/cleanup-on-remove).
When a worktree goes away, the compose stack and its volumes go with it.
`setup.sh` never had a counterpart for this:

```yaml
# daft.yml — new top-level hook
worktree-pre-remove:
  jobs:
    - name: services-down
      run: docker compose down -v --remove-orphans
      env:
        COMPOSE_PROJECT_NAME: ${DAFT_REPO_NAME:-app}-${DAFT_BRANCH_NAME//\//-}
````

`-v` deletes the worktree's named volumes (`pgdata`, etc.). `--remove-orphans`
catches any container the team added later that the running stack doesn't know
about. The `COMPOSE_PROJECT_NAME` value matches the create-side exactly — so
down targets the same containers up created.

## Final daft.yml

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: install-tools
        run: mise install

      - name: install-deps
        run: pnpm install --frozen-lockfile
        needs: [install-tools]

      - name: services-up
        run: docker compose up -d --wait
        needs: [install-deps]
        env:
          COMPOSE_PROJECT_NAME: ${DAFT_REPO_NAME:-app}-${DAFT_BRANCH_NAME//\//-}

      - name: codegen
        run: ./scripts/codegen.sh
        needs: [install-deps]
        background: true

      - name: migrate
        run: pnpm --filter @app/db migrate:latest
        needs: [services-up]

  worktree-pre-remove:
    jobs:
      - name: services-down
        run: docker compose down -v --remove-orphans
        env:
          COMPOSE_PROJECT_NAME: ${DAFT_REPO_NAME:-app}-${DAFT_BRANCH_NAME//\//-}
```

Five jobs in post-create with `needs:` enforcing order, one job in pre-remove.
The whole `setup.sh` is gone; `daft.yml` is the source of truth.

## What you got

Before:

- `git checkout feature/x` → `bin/setup.sh` → 4 minutes of waiting, hoping no
  step fails partway through. Re-runs left orphan containers; the team had a
  `docker rm $(docker ps -aq)` alias for cleanup.
- New contributors had to find the README, find the script, run it, and hope
  they didn't have to ctrl-C halfway through.
- README and `bin/setup.sh` drifted. Anyone who joined the project saw whichever
  happened to be wrong on the day they joined.

After:

- `daft start feature/x` returns when the worktree is ready. Codegen finishes in
  the background a few seconds later.
- One source of truth (`daft.yml`) for "how this project sets up." The README's
  setup section is two lines.
- `daft remove feature/x` cleans up everything `daft.yml` brought into existence
  — no orphan containers, no leaked volumes, no zombie ports.

## Where to next

- **[Walkthroughs → Node monorepo with services](/recipes/walkthroughs/node-monorepo-services)**
  — same operations green-field, with per-worktree port allocation for full
  parallel-worktree support.
- **[CI parity](/recipes/ci-parity)** — apply the same `daft.yml` to your CI
  workflow so local and CI are one source of truth.
- **[Adopting from direnv](/recipes/adopting-from-direnv)** — sibling adoption
  recipe for teams whose existing setup is `.envrc` files rather than a script.
