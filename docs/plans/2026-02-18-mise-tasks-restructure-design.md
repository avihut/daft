# Mise Tasks Restructure Design

## Problem

All ~50 mise tasks are defined inline in `mise.toml` (686 lines) using flat `-`
separated names like `test-integration-clone`. This makes the file unwieldy and
doesn't leverage mise's file-based task features.

## Solution

Move all tasks from inline `mise.toml` definitions to file-based tasks in
`mise-tasks/`. Directory nesting automatically creates `:` namespaced names.
Each task becomes an executable script with `#MISE` metadata directives.

## Directory Structure

```
mise-tasks/
├── build                           → build
├── ci                              → ci
├── clippy                          → clippy
│
├── clean/
│   ├── _default                    → clean
│   ├── rust                        → clean:rust
│   └── tests                       → clean:tests
│
├── completions/
│   ├── install                     → completions:install
│   ├── test                        → completions:test
│   └── gen/
│       ├── bash                    → completions:gen:bash
│       ├── fish                    → completions:gen:fish
│       └── zsh                     → completions:gen:zsh
│
├── dev/
│   ├── _default                    → dev
│   ├── clean                       → dev:clean
│   ├── setup                       → dev:setup
│   ├── test                        → dev:test
│   └── verify                      → dev:verify
│
├── docs/
│   ├── cli/
│   │   ├── gen                     → docs:cli:gen
│   │   └── verify                  → docs:cli:verify
│   └── site/
│       ├── _default                → docs:site
│       ├── build                   → docs:site:build
│       ├── check                   → docs:site:check
│       ├── format                  → docs:site:format
│       ├── preview                 → docs:site:preview
│       └── setup                   → docs:site:setup
│
├── fmt/
│   ├── _default                    → fmt
│   ├── check                       → fmt:check
│   └── docs/
│       ├── _default                → fmt:docs
│       └── check                   → fmt:docs:check
│
├── lint/
│   ├── _default                    → lint
│   └── rust                        → lint:rust
│
├── man/
│   ├── gen                         → man:gen
│   ├── install                     → man:install
│   └── verify                      → man:verify
│
├── setup/
│   ├── _default                    → setup
│   └── rust                        → setup:rust
│
├── test/
│   ├── _default                    → test
│   ├── unit                        → test:unit
│   ├── verbose                     → test:verbose
│   ├── perf                        → test:perf
│   └── integration/
│       ├── _default                → test:integration
│       ├── checkout/
│       │   ├── _default            → test:integration:checkout
│       │   └── branch              → test:integration:checkout:branch
│       ├── clone                   → test:integration:clone
│       ├── config                  → test:integration:config
│       ├── fetch                   → test:integration:fetch
│       ├── flow/
│       │   ├── adopt               → test:integration:flow:adopt
│       │   └── eject               → test:integration:flow:eject
│       ├── gitoxide                → test:integration:gitoxide
│       ├── hooks                   → test:integration:hooks
│       ├── init                    → test:integration:init
│       ├── matrix                  → test:integration:matrix
│       ├── prune                   → test:integration:prune
│       ├── setup                   → test:integration:setup
│       ├── shell/
│       │   └── init                → test:integration:shell:init
│       ├── unknown/
│       │   └── command             → test:integration:unknown:command
│       └── verbose                 → test:integration:verbose
│
├── validate/
│   ├── _default                    → validate
│   └── rust                        → validate:rust
│
└── watch/
    ├── _default                    → watch
    ├── check                       → watch:check
    ├── clippy                      → watch:clippy
    └── unit                        → watch:unit
```

## Naming Changes

| Old name                           | New name                            |
| ---------------------------------- | ----------------------------------- |
| `test-unit`                        | `test:unit`                         |
| `test-all`                         | (dropped, `test` is canonical)      |
| `test-integration`                 | `test:integration`                  |
| `test-integration-clone`           | `test:integration:clone`            |
| `test-integration-checkout`        | `test:integration:checkout`         |
| `test-integration-checkout-branch` | `test:integration:checkout:branch`  |
| `test-integration-init`            | `test:integration:init`             |
| `test-integration-prune`           | `test:integration:prune`            |
| `test-integration-shell-init`      | `test:integration:shell:init`       |
| `test-integration-setup`           | `test:integration:setup`            |
| `test-integration-config`          | `test:integration:config`           |
| `test-integration-hooks`           | `test:integration:hooks`            |
| `test-integration-fetch`           | `test:integration:fetch`            |
| `test-integration-flow-adopt`      | `test:integration:flow:adopt`       |
| `test-integration-flow-eject`      | `test:integration:flow:eject`       |
| `test-integration-unknown-command` | `test:integration:unknown:command`  |
| `test-integration-matrix`          | `test:integration:matrix`           |
| `test-integration-gitoxide`        | `test:integration:gitoxide`         |
| `test-integration-verbose`         | `test:integration:verbose`          |
| `test-verbose`                     | `test:verbose`                      |
| `test-perf`                        | `test:perf`                         |
| `test-perf-integration`            | (dropped, `test:perf` is canonical) |
| `fmt-check`                        | `fmt:check`                         |
| `fmt-docs`                         | `fmt:docs`                          |
| `fmt-docs-check`                   | `fmt:docs:check`                    |
| `dev-setup`                        | `dev:setup`                         |
| `dev-clean`                        | `dev:clean`                         |
| `dev-verify`                       | `dev:verify`                        |
| `dev-test`                         | `dev:test`                          |
| `gen-man`                          | `man:gen`                           |
| `verify-man`                       | `man:verify`                        |
| `install-man`                      | `man:install`                       |
| `gen-cli-docs`                     | `docs:cli:gen`                      |
| `verify-cli-docs`                  | `docs:cli:verify`                   |
| `gen-completions-bash`             | `completions:gen:bash`              |
| `gen-completions-zsh`              | `completions:gen:zsh`               |
| `gen-completions-fish`             | `completions:gen:fish`              |
| `install-completions`              | `completions:install`               |
| `test-completions`                 | `completions:test`                  |
| `docs:site`                        | `docs:site`                         |
| `docs:site-setup`                  | `docs:site:setup`                   |
| `docs:site-build`                  | `docs:site:build`                   |
| `docs:site-preview`                | `docs:site:preview`                 |
| `docs:site-check`                  | `docs:site:check`                   |
| `docs:site-format`                 | `docs:site:format`                  |
| `clean-tests`                      | `clean:tests`                       |
| `clean-rust`                       | `clean:rust`                        |
| `setup-rust`                       | `setup:rust`                        |
| `lint-rust`                        | `lint:rust`                         |
| `validate-rust`                    | `validate:rust`                     |
| `watch-unit`                       | `watch:unit`                        |
| `watch-clippy`                     | `watch:clippy`                      |
| `watch-check`                      | `watch:check`                       |

## Dropped Tasks

These alias-only tasks are removed. Their functionality is preserved via the
`alias` field on the canonical task.

- `build-rust` (alias for `build`)
- `test-rust` (alias for `test:integration`)
- `test-all` (alias for `test`)
- `b` (use `alias="b"` on `build`)
- `default` (use `alias="default"` on `test`)

## mise.toml After Restructure

```toml
# mise configuration for daft
# Run `mise tasks` to see available tasks

[tools]
rust = "stable"
lefthook = "latest"
bun = "latest"

[env]
INTEGRATION_TESTS_DIR = "tests/integration"
INTEGRATION_TEMP_DIR = "/tmp/git-worktree-integration-tests"

[hooks]
enter = """..."""
```

No `[tasks]` section. Tasks are auto-discovered from `mise-tasks/`.

## Task File Format

Each task is an executable script with `#MISE` metadata:

```bash
#!/usr/bin/env bash
#MISE description="Run unit tests"
#MISE depends=["build"]
set -euo pipefail

echo "Running unit tests..."
cargo test --lib --tests
```

## Files Requiring Task Name Updates

- `CLAUDE.md` -- build/test/lint command reference
- `lefthook.yml` -- pre-commit and pre-push hooks
- `.github/workflows/test.yml` -- CI workflow
- `CONTRIBUTING.md` -- contributor guide
- `docs/contributing.md` -- docs contributor guide
- `RELEASING.md` -- release process
- `SETUP_RELEASE.md` -- release setup
- `tests/README.md` -- test documentation
- `.claude/agents/rust-architect.md` -- agent config
- `.claude/agents/code-reviewer.md` -- agent config
- `docs/getting-started/installation.md` -- mentions `dev-setup`
- Internal task references (`depends` fields referencing old names)
