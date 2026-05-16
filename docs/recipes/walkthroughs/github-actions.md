---
title: GitHub Actions with daft hooks
description:
  Reduce a sprawling .github/workflows/test.yml to four logical steps that
  consume the same daft.yml local devs use — one source of truth for setup, no
  "keep in sync" comment.
pillars: [hooks]
---

# GitHub Actions with daft hooks

## Starting state

A Rust workspace that adopted daft locally a while back. Filesystem:

```
myapp/
├── Cargo.toml          # workspace
├── Cargo.lock
├── daft.yml            # toolchain-bootstrap + warmup + cleanup
├── mise.toml           # rust 1.84, sccache 0.8
└── .github/
    └── workflows/
        └── test.yml    # 12 steps, growing
```

`daft.yml` does what every dev does on `daft start` — install mise, fetch
crates, kick off a background `cargo build`, drop the worktree on remove.
`test.yml` does most of the same work in subtly different order, with subtly
different commands:

```yaml
# .github/workflows/test.yml — abridged
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: jdx/mise-action@v2
      - name: Cache cargo registry
        uses: actions/cache@v4
        with: { path: ~/.cargo/registry, key: ... }
      - name: Cache target
        uses: actions/cache@v4
        with: { path: target, key: ... }
      - name: cargo fetch
        run: cargo fetch --locked
      - name: Format + clippy
        run: |
          cargo fmt --check
          cargo clippy --all-targets -- -D warnings
      - name: Build
        run: cargo build --workspace --all-targets
      - name: Test
        run: cargo test --workspace
      - name: Install protoc-gen-go
        run: go install google.golang.org/protobuf/cmd/protoc-gen-go@latest
      - name: Codegen
        run: ./scripts/codegen.sh
```

A comment near the top: _"Keep in sync with daft.yml — install steps must
match."_

The ritual: when adding a new dep, edit both files; when bumping a tool version,
edit both files. PR review catches the mismatch maybe half the time. Last sprint
someone added `protoc-gen-go` to the codegen job in `daft.yml` and didn't add it
to `test.yml`; CI broke; the PR cycle wasted a day chasing a missing-binary
error from a generated module.

The reach for daft: stop maintaining two parallel descriptions of "how this
project sets up." Make CI run the same hooks the worktree does — same
`daft.yml`, same job orchestration, same env contract.

## Patterns we'll thread

This walkthrough applies the [CI parity](/recipes/ci-parity) pattern, with
explicit reference to:

- **[Toolchain bootstrap](/recipes/toolchain-bootstrap)** — the install half of
  `daft.yml` is exactly what CI needs first.
- **[Background warmup](/recipes/background-warmup)** — and the `ci-parity`
  decision rule for which warmups skip in CI vs which run.

By the end: `test.yml` is four logical steps; the cache/install/build work
disappears because `daft hooks run` does that work locally and in CI; the "keep
in sync" comment is gone because there's nothing to keep in sync.

## Step 1: install daft

The first thing CI needs is the `daft` binary:

```yaml
# .github/workflows/test.yml
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install daft
        run: curl -fsSL https://daft.avihu.dev/install.sh | bash
```

The install script writes the binary to a location already on the GitHub Actions
runner's `PATH`. No extra `$GITHUB_PATH` step required. Pin to a specific daft
version once the workflow is stable; for the first port-over, `master` is fine.

## Step 2: trust hooks

Hooks default to `deny`. CI runners are ephemeral, so no trust state survives
between jobs — each run trusts the hooks fresh:

```yaml
- name: Trust hooks
  run: git daft-hooks trust --all
  env:
    DAFT_NONINTERACTIVE: "1"
```

`--all` trusts every hook in the repo; `DAFT_NONINTERACTIVE=1` tells daft to
fail fast rather than prompt — a daft waiting on stdin in CI is a daft that
times out 6 minutes later.

For self-hosted runners with persistent state, trust once at provisioning rather
than per run. See [Trust & security](/hooks/trust-and-security) for the per-host
trust file's semantics.

## Step 3: run the hook

The whole install/fetch/codegen flow becomes one command:

```yaml
- name: Run worktree-post-create hooks
  run: daft hooks run worktree-post-create
```

`daft hooks run worktree-post-create` invokes the same jobs the hook fires
locally — same `daft.yml`, same `needs:` graph, same env vars. Adding a dep to
`daft.yml` automatically applies to CI on the next push.

For the warmup-skip-in-CI decision: jobs that prime per-worktree caches (cargo's
`target/`, Vite's `.vite/`) waste CI's runner time because the runner is
ephemeral and won't reuse the cache. Add `skip: { env: { CI: "true" } }` to
those jobs in `daft.yml` — most CI providers set `CI=true` automatically, so the
same `daft.yml` runs warmups locally and skips them in CI. Jobs that prime
_shared_ caches (sccache, the Go build cache when CI persists
`~/.cache/go-build`) should _not_ skip — the warmup is exactly what the test
step then reads from. Full decision rule in
[CI parity](/recipes/ci-parity#decision-rule-for-warmups-in-ci).

## Final test.yml

```yaml
# .github/workflows/test.yml
name: Test
on: [push, pull_request]
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

      - name: Format + lint + test
        run: |
          cargo fmt --check
          cargo clippy --all-targets -- -D warnings
          cargo test --workspace
```

Five steps, ~25 lines. The cache/install/build/codegen/protoc work that took
eight steps in the old workflow now lives in `daft.yml`, where it's also the
source of truth for local development.

## What you got

Before:

- `test.yml` had 12 steps. Six of them duplicated `daft.yml`'s
  install/build/codegen logic, in subtly different order.
- A "_keep in sync with daft.yml_" comment near the top of the workflow that PR
  reviewers routinely missed.
- `protoc-gen-go` was added to `daft.yml` but missed in `test.yml`; CI broke;
  the PR cycle wasted a day on a missing-binary error.

After:

- `test.yml` is five steps. Three set up daft (install, trust, run hooks); one
  is checkout; one runs the actual tests.
- Adding a new dep updates _one_ file — `daft.yml`. CI picks it up
  automatically.
- The "keep in sync" comment is gone because there's nothing to keep in sync.

## Where to next

- **[CI parity](/recipes/ci-parity)** — the principle this walkthrough applies,
  plus GitLab and generic shell-based CI variants if you're not on GitHub
  Actions.
- **[Trust & security](/hooks/trust-and-security)** — `--all` semantics,
  per-host trust storage, and what changes when CI runners are persistent rather
  than ephemeral.
- **[Walkthroughs → Rust binary with debug warmup](/recipes/walkthroughs/rust-binary)**
  — the local-side counterpart this walkthrough's `daft.yml` was built from.
