---
title: Services with ports
description:
  Per-worktree compose stacks that don't collide — derived ports, slug-named
  projects, automatic teardown.
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
ports, named after the worktree. Two parallel worktrees coexist; the dev server
in feature/auth talks to feature/auth's Postgres, not feature/billing's.

## What changes

`compose.yaml` stops hardcoding port numbers — they come from env vars. The
ports themselves come from a declared `env:` section in `daft.yml`:
[`daft env`](/reference/cli/daft-env) derives each worktree's values as a pure
function of the worktree's name, so `feature/auth` gets the same ports on every
machine, every restart, forever — no allocation script, no registry, no
bookkeeping job. Each worktree hashes to its own contiguous block, and the
declared names take consecutive offsets inside it.

Hooks and tasks receive the declared values in their environment automatically,
so the compose jobs need no port plumbing at all. The same section declares
`COMPOSE_PROJECT_NAME`, which prefixes every container, network, and volume with
`<repo>-<worktree-slug>` so two stacks can coexist.

A symmetric `worktree-pre-remove` job tears it all down. The full teardown
semantics live in [Cleanup on remove](/recipes/cleanup-on-remove); this page
shows the minimum needed for the create-side to be safe.

## Recipe

One `env:` declaration plus a boot job and its matching teardown:

```yaml
# daft.yml
env:
  salt: myapp # pin it: values become identical on every machine
  ports:
    - PORT_POSTGRES # offset 0 in this worktree's block
    - PORT_REDIS # offset 1
  values:
    COMPOSE_PROJECT_NAME: "myapp-{worktree_slug}"

hooks:
  worktree-post-create:
    jobs:
      - name: services-up
        run: docker compose up -d --wait

  worktree-pre-remove:
    jobs:
      - name: services-down
        run: docker compose down -v --remove-orphans
```

`compose.yaml`:

```yaml
services:
  postgres:
    image: postgres:17
    ports: ["127.0.0.1:${PORT_POSTGRES}:5432"]
    volumes: [pgdata:/var/lib/postgresql/data]
  redis:
    image: redis:7
    ports: ["127.0.0.1:${PORT_REDIS}:6379"]
volumes:
  pgdata:
```

Piece by piece:

1. **`env.ports`** declares the port names. Each worktree hashes to its own
   16-port block in 20000–32767 (below the OS ephemeral ranges, above the
   3000–9999 zone dev tools squat on), and declared names take consecutive
   offsets — one worktree's ports read like `23952, 23953`, easy to hold in your
   head while debugging. Ask anytime with `daft env PORT_POSTGRES`, from any
   directory, even for a worktree you haven't created yet.
2. **`env.values`** renders `COMPOSE_PROJECT_NAME` per worktree — the prefix
   that turns `postgres-1` into `myapp-feature-auth-postgres-1`, isolating
   containers, networks, and volumes per worktree.
3. **`services-up`** needs no `env:` block: hooks receive every declared value
   automatically. `--wait` blocks until the containers report healthy.
4. **`services-down -v --remove-orphans`** is the symmetric pre-remove: stop
   containers, delete the worktree's volumes, sweep stragglers.
5. Publishing on `127.0.0.1:` keeps dev services off your LAN interface —
   hygiene that costs nothing.

For interactive `docker compose` commands from your shell — `docker compose ps`
showing _this_ worktree's containers — load the same values with direnv:

```bash
# .envrc
eval "$(daft env --export)"
watch_file daft.yml daft.local.yml
```

Two parallel worktrees now coexist. `daft start feature/billing` while
feature/auth is up gets a different port block, a different project name, and a
different set of volumes — no collisions, no manual overrides. If two worktree
names ever hash to the same block, `daft env` warns on stderr; rename one or set
a different `salt:`.

::: warning Migrating from the branch-hash script

Earlier versions of this recipe hashed `$DAFT_BRANCH_NAME` with `cksum` into
30000–39990 from an `allocate-ports` hook. The declared `env:` section replaces
that job entirely — delete it — but the derived ports are different numbers
(worktree-keyed, 20000–32767). Anything that memorized the old ports (debug
configs, bookmarked URLs) needs the new values from `daft env` once.

:::

## Variants by starting state

By **starting state** — what your `compose.yaml` looks like before adopting
daft. The Recipe above is the green-field shape; here's what changes if you're
adopting an existing stack.

### Green-field

The Recipe above is the full shape. `compose.yaml` is yours; you control the
port surface; you write `${PORT_POSTGRES}:5432` from the ground up; the `env:`
section owns the numbers. Two parallel worktrees coexist with disjoint ports and
disjoint container names.

### Adopt-existing

Your team has been running `compose.yaml` for months. You don't want to
coordinate a "pull and re-up" with everyone today just to add daft. You can
layer daft on top without editing the file — but how much isolation you get
depends on what `compose.yaml` already looks like.

**Case 1 — `compose.yaml` already uses env-var ports.** Common in projects that
did the right thing early. The existing `compose.yaml` has
`${PORT_POSTGRES:-5432}:5432` (a default, with override). Drop in the
green-field Recipe as-is — the existing defaults stay correct for
one-worktree-at-a-time, and the declared ports take over for parallel worktrees.

**Case 2 — `compose.yaml` hardcodes ports.** `5432:5432` everywhere. You get
container, network, and volume isolation via `COMPOSE_PROJECT_NAME`, but
host-side ports still collide: two worktrees can't both have Postgres up; the
second `docker compose up` fails with "port already in use." This is still a
worthwhile adoption — a single worktree at a time gets clean isolation (no `dev`
containers polluting `master`), and the team can port-variable-ize the file
later as a smaller, separate PR. The minimum for this case:

```yaml
# daft.yml
env:
  values:
    COMPOSE_PROJECT_NAME: "myapp-{worktree_slug}"

hooks:
  worktree-post-create:
    jobs:
      - name: services-up
        run: docker compose up -d --wait

  worktree-pre-remove:
    jobs:
      - name: services-down
        run: docker compose down -v --remove-orphans
```

When the team is ready, port-variable-ize `compose.yaml` (`5432:5432` →
`${PORT_POSTGRES}:5432`), add the `ports:` list, and graduate to the green-field
Recipe.

## Variants by runtime

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
    ports: ["127.0.0.1:${PORT_MEILI}:7700"]
    profiles: ["search"]
```

```yaml
# daft.yml — declare the extra port; only boot search if SEARCH=1 in env
env:
  ports:
    - PORT_POSTGRES
    - PORT_REDIS
    - PORT_MEILI
hooks:
  worktree-post-create:
    jobs:
      - name: services-up
        run: docker compose --profile search up -d --wait
        only: { env: { SEARCH: "1" } }
```

Devs who need search export `SEARCH=1` in their personal `mise.local.toml` or
shell rc; everyone else gets the lean stack.

### Podman

`podman compose` reads the same compose files. Substitute it for
`docker compose` in the hook. Podman runs rootless by default — port allocations
under 1024 need extra config, so stick to high ports (which the derived range
already guarantees).

### Native processes (no containers)

Sometimes a heavy stack is overkill. A single Go service in dev mode is fine
running directly. But a foreground dev server is a _serve on demand_ concern,
not provisioning — put it in a `tasks:` block and start it with
[`daft run`](/reference/cli/daft-run) rather than backgrounding it from a hook:

```yaml
env:
  ports:
    - PORT_APP
tasks:
  run:
    jobs:
      - name: dev-server
        run: ./bin/myserver --port "$PORT_APP"
```

Tasks receive the declared values automatically — `$PORT_APP` is just there.
`daft run` streams the server's output live and stops it on Ctrl+C — no PID
file, no pre-remove kill hook, and no server booting in every worktree you only
ever read. If you truly need the process to outlive the command, keep the
backgrounded-hook approach and kill it from the pre-remove hook — covered in
[Cleanup on remove → native processes by PID file](/recipes/cleanup-on-remove#native-processes-by-pid-file).

### Multi-file compose

Real projects often split compose across files (`compose.yaml` for core
services, `compose.dev.yaml` for dev-only overrides):

```yaml
- name: services-up
  run: docker compose -f compose.yaml -f compose.dev.yaml up -d --wait
```

Setting `COMPOSE_FILE=compose.yaml:compose.dev.yaml` in `.envrc` is an
alternative — bare `docker compose` commands from your shell pick up the same
files without needing `-f` every time.

## Idempotency & safety

`docker compose up -d` is idempotent in the right ways:

- Already-running containers stay running
- Stopped containers restart
- Image pulls happen on first run, skipped after
- Named volumes persist across restarts (so the data survives a hook re-run,
  which is what you want)

The derived ports are deterministic, which changes one habit: **turn off your
tools' auto-increment port fallbacks** (`strictPort: true` in Vite and friends).
Once ports are stable, a bind failure should be a loud error naming a real
conflict — not a silent `+1` that reintroduces the drift you just eliminated.

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
- **[Env vars & secrets](/recipes/env-vars-and-secrets)** — layering derived
  values with real secrets and per-worktree `.env` files.
- **[Walkthroughs → Node monorepo with services](/recipes/walkthroughs/node-monorepo-services)**
  — this pattern threaded into a complete project setup, with migrations,
  multiple services, and DATABASE_URL wiring.
