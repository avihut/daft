---
title: CI parity
description:
  Run the same daft.yml in CI as you do locally — one source of truth for "how
  this project sets up," with rules for what to skip.
pillars: [hooks]
---

# CI parity

## Starting state

The repo has a `daft.yml` with six `worktree-post-create` jobs: `mise install`,
`pnpm install`, codegen, `services-up`, `migrate`, warmup. It also has
`.github/workflows/test.yml` with five steps that do roughly the same thing — in
a slightly different order, sometimes with subtly different commands. There's a
comment near the top of test.yml: _"Keep in sync with daft.yml."_

Someone adds `protoc-gen-go` to the codegen job in `daft.yml` and doesn't update
the workflow. CI breaks; the failing job's logs point at a missing binary inside
a generated module. Or a PR bumps mise's pinned Node version, and reviewers are
arguing whether to "also update the CI matrix." Whether the workflow gets the
right edit is a coin flip — and that's the part that scares you.

The reach for daft: stop maintaining two parallel descriptions of "how this
project sets up." Make CI run the same hooks the worktree does — same
`daft.yml`, same job orchestration, same env contract.

## What changes

The CI workflow shrinks to four logical steps: install daft, trust the hooks,
run `daft hooks run worktree-post-create`, run tests. The dep install, codegen,
service boot — all of it — moves out of the workflow file into the same
`daft.yml` your local worktrees already use.

Adding a step (a new mise tool, a new compose service) updates **one** file and
applies to local AND CI. The "keep in sync" comment goes away because there's
nothing to keep in sync.

## Recipe

```yaml
# .github/workflows/test.yml
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install daft
        run: curl -fsSL https://daft.avihu.dev/install.sh | bash

      - name: Trust hooks
        run: git daft-hooks trust --all
        env:
          DAFT_NONINTERACTIVE: "1"

      - name: Run worktree-post-create hooks
        run: daft hooks run worktree-post-create

      - name: Run tests
        run: pnpm test
```

`daft hooks run worktree-post-create` invokes the same jobs the
`worktree-post-create` hook fires locally. Whatever your contributors' worktrees
do on `daft start`, CI does too — using the same `daft.yml`, the same `needs:`
graph, the same env vars.

`DAFT_NONINTERACTIVE=1` tells daft never to prompt. `git daft-hooks trust --all`
pre-trusts the hooks (they won't run otherwise — see
[Trust & security](/hooks/trust-and-security)).

## Variants

By **CI vendor** — the daft contract is the same; only the workflow syntax
differs.

### GitLab CI

```yaml
# .gitlab-ci.yml
test:
  image: ubuntu:24.04
  variables:
    DAFT_NONINTERACTIVE: "1"
  before_script:
    - apt-get update && apt-get install -y curl git
    - curl -fsSL https://daft.avihu.dev/install.sh | bash
    - git daft-hooks trust --all
    - daft hooks run worktree-post-create
  script:
    - pnpm test
```

### Generic shell-based CI (Buildkite, Jenkins, CircleCI)

For any CI without a daft-specific helper, the pattern is the same: install
daft, trust hooks, run hooks, run tests.

```bash
curl -fsSL https://daft.avihu.dev/install.sh | bash
export DAFT_NONINTERACTIVE=1
git daft-hooks trust --all
daft hooks run worktree-post-create
pnpm test
```

## Skipping local-only steps in CI

Some hook jobs make sense locally but not in CI:

- **`direnv allow`** — no interactive shell in CI; direnv-loaded vars come from
  the workflow's `env:` instead.
- **`op signin` / interactive vault unlocks** — CI uses its own secret store,
  not your 1Password.
- **Backgrounded warmups** — sometimes. See decision rule below.

Use `skip:` to gate these:

```yaml
- name: seed-envrc
  run: |
    cp .envrc.example .envrc
    direnv allow .
  skip:
    env: { CI: "true" }
```

Most CI providers set `CI=true` automatically; the few that don't, set it
yourself in the workflow.

The flip side — jobs that **only** make sense in CI — use `only:`:

```yaml
- name: ci-coverage-config
  run: ./scripts/setup-coverage-reporter.sh
  only:
    env: { CI: "true" }
```

### Decision rule for warmups in CI

A [Background warmup](/recipes/background-warmup) is correct to skip in CI when
its only purpose is "first interactive command is fast" — the cache it primes is
per-worktree (`target/` in Rust, `.vite/` in Vite) and CI's runner is ephemeral,
so priming serves no one.

A warmup is correct to **run** in CI when it primes a shared, content-addressed
cache that the test step also benefits from — sccache, the Go build cache (when
CI persists `~/.cache/go-build`), Gradle's configuration cache (when CI persists
it). The "warmup" work isn't wasted: it's exactly what the test step needs to be
fast.

Concretely:

```yaml
# Skip — per-worktree cache, CI doesn't reuse it
- name: warmup-vite
  run: pnpm exec vite optimize --force
  background: true
  skip: { env: { CI: "true" } }

# Run — primes a shared sccache that the next cargo build will hit
- name: warmup-build
  run: cargo build --workspace
  background: true
  env:
    RUSTC_WRAPPER: sccache
```

## CI-specific env vars and secrets

Local hooks fetch secrets from a vault or sops. CI hooks fetch from CI's secret
store (GitHub Actions secrets, GitLab variables). The hook stays the same; the
**source** changes.

GitHub Actions:

```yaml
- name: Run worktree-post-create hooks
  run: daft hooks run worktree-post-create
  env:
    DATABASE_URL: ${{ secrets.CI_DATABASE_URL }}
    API_KEY: ${{ secrets.CI_API_KEY }}
```

If the local hook has a `seed-envrc-from-1password` step that doesn't exist in
CI, gate with `skip: { env: { CI: "true" } }` and inject the vars directly via
the workflow's `env:`.

## Layer caching with a daft-aware base image

The fastest CI builds reuse a base image that already has the toolchain and
dependencies installed. Bake the install into your CI image:

```dockerfile
# .ci/Dockerfile
FROM ubuntu:24.04
RUN apt-get update && apt-get install -y curl git build-essential
RUN curl -fsSL https://daft.avihu.dev/install.sh | bash
COPY mise.toml /tmp/mise.toml
RUN mise install

WORKDIR /workspace
COPY package.json pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
```

Then in CI, skip the cached jobs by tag:

```yaml
- name: Run hooks (skip already-cached steps)
  run: daft hooks run worktree-post-create
  env:
    DAFT_SKIP_TAGS: "tools-install,deps-install"
```

Tag the cached jobs in `daft.yml`:

```yaml
- name: install-tool-versions
  run: mise install
  tags: [tools-install]

- name: install-deps
  run: pnpm install --frozen-lockfile
  tags: [deps-install]
```

## Idempotency & safety

CI-specific concerns on top of the local idempotency story:

**Trust state is per-CI-host.** `git daft-hooks trust --all` writes trust to the
runner's local state. Ephemeral runners trust on every run (correct); persistent
runners trust once during provisioning.

**No interactive prompts.** `DAFT_NONINTERACTIVE=1` tells daft to fail fast
instead of prompting. Always set it. A daft waiting on stdin in CI is a daft
that times out 6 minutes later.

::: warning Don't run pre-remove hooks in CI CI runners create a worktree
implicitly via `actions/checkout`; they don't `daft remove` it. If you've wired
pre-remove cleanup that destroys real state (a production database snapshot, an
external registry entry), **never** fire it from CI. Pre-remove is for
developer-machine teardown, not build-server cleanup. :::

## Where to next

- **[Toolchain bootstrap](/recipes/toolchain-bootstrap)** — the primary
  candidate for CI parity. The same `pnpm install --frozen-lockfile` is one
  source of truth for "how this project installs."
- **[Trust & security](/hooks/trust-and-security)** — why hooks need trust, how
  `--all` works, and what happens with untrusted hooks.
- **[YAML reference](/hooks/yaml-reference)** — `tags`, `skip`, `only` schema
  for splitting hook execution by environment.
