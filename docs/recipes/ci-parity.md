---
title: CI parity
description:
  Run the same daft.yml in CI as you do locally — trust in non-interactive
  contexts, layer caching, and skipping local-only steps.
pillars: [hooks]
---

# CI parity

> The hooks that bootstrap your local worktree are also the hooks CI should run
> before any test. If `daft.yml` says "install deps, run codegen, boot services"
> locally, CI saying "install deps via a custom workflow step" guarantees drift.
> Run the same hooks from CI and the contract holds.

## When to reach for this

- Your `worktree-post-create` does real setup (deps, codegen, services) and you
  want CI to do the same setup, the same way.
- You've debugged a "passes locally, fails in CI" issue that turned out to be
  different setup paths.
- You want adding a step to local setup (a new mise tool, a new compose service)
  to automatically apply to CI without a parallel CI workflow edit.

## Minimal recipe — GitHub Actions

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
        run: |
          git daft-hooks trust --all
        env:
          DAFT_NONINTERACTIVE: "1"

      - name: Run worktree-post-create hooks
        run: daft hooks run worktree-post-create

      - name: Run tests
        run: pnpm test
```

`daft hooks run worktree-post-create` invokes the same jobs the
`worktree-post-create` hook fires locally. Whatever your contributors' worktrees
do on `daft start`, CI does too — using the same `daft.yml`, the same job
orchestration, the same env vars.

`DAFT_NONINTERACTIVE=1` tells daft never to prompt. `git daft-hooks trust --all`
pre-trusts the hooks (they won't run otherwise — see
[Trust & security](/hooks/trust-and-security)).

## Variants

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

### Generic shell-based CI

For Buildkite, Jenkins, CircleCI, or any other CI without a daft-specific
helper, the pattern is the same: install daft, trust hooks, run hooks, run
tests.

```bash
curl -fsSL https://daft.avihu.dev/install.sh | bash
export DAFT_NONINTERACTIVE=1
git daft-hooks trust --all
daft hooks run worktree-post-create
pnpm test
```

### Skipping local-only steps in CI

Some hook jobs make sense locally but not in CI — `direnv allow`, interactive
`op signin`, dev-server warmups. Use the `skip:` field:

```yaml
- name: seed-envrc
  run: |
    cp .envrc.example .envrc
    direnv allow .
  skip:
    env: { CI: "true" }

- name: warmup-build
  run: cargo build
  background: true
  skip:
    env: { CI: "true" }
```

Most CI providers set `CI=true` automatically. For the few that don't, set it
yourself in the workflow.

The flip side: some jobs only make sense in CI. Use `only:`:

```yaml
- name: ci-coverage-config
  run: ./scripts/setup-coverage-reporter.sh
  only:
    env: { CI: "true" }
```

### CI-specific env vars and secrets

Local hooks fetch secrets from a vault or sops. CI hooks should fetch from CI's
secret store (GitHub Actions secrets, GitLab variables). The hook itself stays
the same; the env vars come from a different source.

GitHub Actions:

```yaml
- name: Run worktree-post-create hooks
  run: daft hooks run worktree-post-create
  env:
    DATABASE_URL: ${{ secrets.CI_DATABASE_URL }}
    API_KEY: ${{ secrets.CI_API_KEY }}
```

If your local hook has a `seed-envrc-from-1password` step that doesn't exist in
CI, gate with `skip: { env: { CI: "true" } }` and let the CI workflow inject
vars directly.

### Layer caching with a daft-aware base image

The fastest CI builds reuse a base image that already has mise tools and
dependencies installed. Bake the install into your CI image:

```dockerfile
# .ci/Dockerfile
FROM ubuntu:24.04
RUN apt-get update && apt-get install -y curl git build-essential
RUN curl -fsSL https://daft.avihu.dev/install.sh | bash
COPY mise.toml /tmp/mise.toml
RUN mise install  # Pre-install tool versions

# Final layer: cache dep installs
WORKDIR /workspace
COPY package.json pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
```

Then in CI:

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

`DAFT_SKIP_TAGS` (or hook-level `exclude_tags`) lets CI skip jobs whose work is
already done in the base image.

## Idempotency & safety

Hooks run in CI need the same idempotency story as locally — trust the
strict-lockfile commands, guard cleanup steps with `|| true` where appropriate.
The CI-specific concerns:

**Trust state is per-CI-host.** `git daft-hooks trust --all` writes trust to the
CI runner's local state. Runners are usually ephemeral, so trusting on every run
is correct; persistent runners should trust once during runner provisioning.

**No interactive prompts.** `DAFT_NONINTERACTIVE=1` (set as an env var) tells
daft to fail fast instead of prompting. Always set it. A daft that's waiting for
stdin in CI is a daft that times out.

**Don't share secret-fetch hooks across local and CI.** A `worktree-post-create`
job that runs `op inject` works locally (where 1Password CLI is installed and
the user is signed in) but fails in CI. Use `skip:`/`only:` to split.

::: warning Don't run pre-remove hooks in CI CI runners create a worktree
implicitly via `actions/checkout`; they don't `daft remove` it. If you wired
pre-remove cleanup that destroys a real database or external state, **never**
trigger it from CI. Pre-remove is for developer-machine teardown, not CI
cleanup. :::

## Composes well with

- **[Toolchain bootstrap](/recipes/toolchain-bootstrap)** — the primary
  candidate for CI parity. Same `pnpm install --frozen-lockfile` step; one
  source of truth for "how this project sets up."
- **[Declarative envs](/recipes/declarative-envs)** — `mise install` before
  `pnpm install` works the same locally and in CI.
- **[Services with ports](/recipes/services-with-ports)** — compose parity in CI
  is great for integration tests; pair with `COMPOSE_PROJECT_NAME` based on the
  CI run ID instead of the branch to avoid collisions in matrix runs.
- **[Env vars & secrets](/recipes/env-vars-and-secrets)** — different source per
  environment (vault locally, CI secrets in workflow).

## Anti-patterns

- **Duplicating setup in a custom CI workflow** — `pnpm install` in `daft.yml`
  for local AND in `.github/workflows/test.yml` for CI. Two sources of truth,
  drift waiting to happen. Pick the daft hook.
- **Forgetting `DAFT_NONINTERACTIVE=1`** — daft prompts for trust, CI hangs, the
  build times out 6 minutes later. Always set it.
- **Running pre-remove in CI** — see warning above.

## See also

- **[Lifecycle hooks](/hooks/lifecycle)** — `worktree-post-create` reference,
  env-var conventions
- **[Trust & security](/hooks/trust-and-security)** — why trust matters, how
  `--all` works, what happens with untrusted hooks
- **[YAML reference](/hooks/yaml-reference)** — `tags`, `exclude_tags`, `skip`,
  `only` schema details
