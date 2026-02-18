# Mise Tasks Restructure Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to
> implement this plan task-by-task.

**Goal:** Move all ~50 inline mise tasks from `mise.toml` to file-based tasks in
`mise-tasks/` with `:` hierarchy naming.

**Architecture:** Each task becomes an executable bash script with `#MISE`
directives for metadata. Directory nesting creates `:` namespaced names
automatically. The `mise.toml` retains only `[tools]`, `[env]`, and `[hooks]`
sections.

**Tech Stack:** mise file-based tasks, bash scripts, `#MISE` metadata directives

---

### Task 1: Create directory structure

**Files:**

- Create: `mise-tasks/` and all subdirectories

**Step 1: Create all directories**

```bash
mkdir -p mise-tasks/{clean,completions/gen,dev,docs/{cli,site},fmt/docs,lint,man,setup,test/integration/{checkout,flow,shell,unknown},validate,watch}
```

**Step 2: Verify structure**

```bash
find mise-tasks -type d | sort
```

Expected: all directories from the design doc exist.

**Step 3: Commit**

```bash
git add mise-tasks/.gitkeep  # or just commit with the first task files
```

No commit yet -- we'll commit after adding the files in the next tasks.

---

### Task 2: Create build, CI, and clippy task files

**Files:**

- Create: `mise-tasks/build`
- Create: `mise-tasks/ci`
- Create: `mise-tasks/clippy`

**Step 1: Create `mise-tasks/build`**

```bash
#!/usr/bin/env bash
#MISE description="Build Rust binaries (release mode)"
set -euo pipefail

echo "Building Rust binaries..."
cargo build --release
```

**Step 2: Create `mise-tasks/ci`**

```bash
#!/usr/bin/env bash
#MISE description="Run CI simulation"
#MISE depends=["setup", "validate", "test"]
set -euo pipefail

echo "CI simulation completed successfully"
```

**Step 3: Create `mise-tasks/clippy`**

```bash
#!/usr/bin/env bash
#MISE description="Run Rust clippy linter (zero warnings)"
set -euo pipefail

cargo clippy --tests -- -D warnings
```

**Step 4: Make all executable**

```bash
chmod +x mise-tasks/build mise-tasks/ci mise-tasks/clippy
```

---

### Task 3: Create clean task files

**Files:**

- Create: `mise-tasks/clean/_default`
- Create: `mise-tasks/clean/rust`
- Create: `mise-tasks/clean/tests`

**Step 1: Create `mise-tasks/clean/_default`**

```bash
#!/usr/bin/env bash
#MISE description="Clean up all artifacts"
#MISE depends=["clean:tests", "clean:rust", "dev:clean"]
set -euo pipefail

echo "All artifacts cleaned"
```

**Step 2: Create `mise-tasks/clean/rust`**

```bash
#!/usr/bin/env bash
#MISE description="Clean Rust build artifacts"
set -euo pipefail

echo "Cleaning Rust build artifacts..."
cargo clean
```

**Step 3: Create `mise-tasks/clean/tests`**

```bash
#!/usr/bin/env bash
#MISE description="Clean up test artifacts"
set -euo pipefail

echo "Cleaning up test artifacts..."
rm -rf /tmp/git-worktree-integration-tests
echo "Test artifacts cleaned"
```

**Step 4: Make all executable**

```bash
chmod +x mise-tasks/clean/_default mise-tasks/clean/rust mise-tasks/clean/tests
```

---

### Task 4: Create completions task files

**Files:**

- Create: `mise-tasks/completions/install`
- Create: `mise-tasks/completions/test`
- Create: `mise-tasks/completions/gen/bash`
- Create: `mise-tasks/completions/gen/zsh`
- Create: `mise-tasks/completions/gen/fish`

**Step 1: Create `mise-tasks/completions/install`**

```bash
#!/usr/bin/env bash
#MISE description="Install shell completions for bash/zsh/fish"
#MISE depends=["build"]
set -euo pipefail

echo "Installing shell completions..."
./target/release/daft completions bash --install
./target/release/daft completions zsh --install
./target/release/daft completions fish --install
echo "Completions installed successfully!"
echo ""
echo "Restart your shell or source your shell config file to enable completions"
```

**Step 2: Create `mise-tasks/completions/test`**

```bash
#!/usr/bin/env bash
#MISE description="Test shell completions"
#MISE depends=["build"]
set -euo pipefail

echo "Testing shell completions..."
if [ -f tests/integration/test_completions.sh ]; then
    tests/integration/test_completions.sh
else
    echo "Completion tests not yet implemented"
fi
```

**Step 3: Create `mise-tasks/completions/gen/bash`**

```bash
#!/usr/bin/env bash
#MISE description="Generate bash completions to /tmp/"
#MISE depends=["build"]
set -euo pipefail

echo "Generating bash completions..."
for cmd in git-worktree-clone git-worktree-init git-worktree-checkout git-worktree-checkout-branch git-worktree-prune; do
    echo "  $cmd"
    ./target/release/daft completions bash --command="$cmd" > /tmp/completion-$cmd.bash
done
echo "Bash completions generated in /tmp/"
```

**Step 4: Create `mise-tasks/completions/gen/zsh`**

```bash
#!/usr/bin/env bash
#MISE description="Generate zsh completions to /tmp/"
#MISE depends=["build"]
set -euo pipefail

echo "Generating zsh completions..."
for cmd in git-worktree-clone git-worktree-init git-worktree-checkout git-worktree-checkout-branch git-worktree-prune; do
    echo "  $cmd"
    ./target/release/daft completions zsh --command="$cmd" > /tmp/completion-_$cmd.zsh
done
echo "Zsh completions generated in /tmp/"
```

**Step 5: Create `mise-tasks/completions/gen/fish`**

```bash
#!/usr/bin/env bash
#MISE description="Generate fish completions to /tmp/"
#MISE depends=["build"]
set -euo pipefail

echo "Generating fish completions..."
for cmd in git-worktree-clone git-worktree-init git-worktree-checkout git-worktree-checkout-branch git-worktree-prune; do
    echo "  $cmd"
    ./target/release/daft completions fish --command="$cmd" > /tmp/completion-$cmd.fish
done
echo "Fish completions generated in /tmp/"
```

**Step 6: Make all executable**

```bash
chmod +x mise-tasks/completions/{install,test} mise-tasks/completions/gen/{bash,zsh,fish}
```

---

### Task 5: Create dev task files

**Files:**

- Create: `mise-tasks/dev/_default`
- Create: `mise-tasks/dev/setup`
- Create: `mise-tasks/dev/clean`
- Create: `mise-tasks/dev/verify`
- Create: `mise-tasks/dev/test`

**Step 1: Create `mise-tasks/dev/_default`**

```bash
#!/usr/bin/env bash
#MISE description="Quick development cycle: setup + verify"
#MISE depends=["dev:setup", "dev:verify"]
set -euo pipefail

echo "Development environment ready and verified!"
```

**Step 2: Create `mise-tasks/dev/setup`**

```bash
#!/usr/bin/env bash
#MISE description="Build binary and create symlinks in target/release/"
#MISE depends=["setup:rust"]
set -euo pipefail
```

Note: This task has no `run` body -- it just depends on `setup:rust`. The
original `dev-setup` was `depends = ["setup-rust"]` with no run command.

**Step 3: Create `mise-tasks/dev/clean`**

```bash
#!/usr/bin/env bash
#MISE description="Remove development symlinks (keeps binary)"
set -euo pipefail

echo "Cleaning development symlinks..."
cd target/release && rm -f \
    git-worktree-clone \
    git-worktree-init \
    git-worktree-checkout \
    git-worktree-checkout-branch \
    git-worktree-prune \
    git-worktree-carry \
    git-worktree-fetch \
    git-worktree-flow-adopt \
    git-worktree-flow-eject \
    git-daft \
    gwtclone gwtinit gwtco gwtcb gwtprune gwtcarry gwtfetch \
    gwco gwcob \
    gclone gcw gcbw gprune
echo "Symlinks removed (binary preserved)"
```

**Step 4: Create `mise-tasks/dev/verify`**

```bash
#!/usr/bin/env bash
#MISE description="Verify dev setup is working"
set -euo pipefail

echo "Verifying development setup..."
test -f target/release/daft || (echo "Binary not found" && exit 1)
test -L target/release/git-worktree-clone || (echo "Symlinks not created" && exit 1)
./target/release/daft >/dev/null 2>&1 || (echo "Direct invocation failed" && exit 1)
./target/release/git-worktree-clone --help >/dev/null 2>&1 || (echo "Symlink invocation failed" && exit 1)
echo "All checks passed"
```

**Step 5: Create `mise-tasks/dev/test`**

```bash
#!/usr/bin/env bash
#MISE description="Full dev test: setup + run all tests"
#MISE depends=["dev:setup", "test"]
set -euo pipefail

echo "Development setup tested successfully!"
```

**Step 6: Make all executable**

```bash
chmod +x mise-tasks/dev/{_default,setup,clean,verify,test}
```

---

### Task 6: Create docs task files

**Files:**

- Create: `mise-tasks/docs/cli/gen`
- Create: `mise-tasks/docs/cli/verify`
- Create: `mise-tasks/docs/site/_default`
- Create: `mise-tasks/docs/site/setup`
- Create: `mise-tasks/docs/site/build`
- Create: `mise-tasks/docs/site/preview`
- Create: `mise-tasks/docs/site/check`
- Create: `mise-tasks/docs/site/format`

**Step 1: Create `mise-tasks/docs/cli/gen`**

```bash
#!/usr/bin/env bash
#MISE description="Generate CLI reference docs to docs/cli/ directory using xtask"
set -euo pipefail

echo "Generating CLI reference docs..."
mkdir -p docs/cli
cargo run --package xtask -- gen-cli-docs --output-dir=docs/cli
bun run prettier --write 'docs/cli/*.md'
echo "CLI docs generated in docs/cli/"
```

**Step 2: Create `mise-tasks/docs/cli/verify`**

```bash
#!/usr/bin/env bash
#MISE description="Verify CLI docs are up-to-date with source"
set -euo pipefail

echo "Verifying CLI docs are up-to-date..."
cargo run --package xtask -- gen-cli-docs --output-dir=docs/cli
bun run prettier --write 'docs/cli/*.md'
git diff --exit-code docs/cli/ || (echo "CLI docs out of date. Run 'mise run docs:cli:gen' and commit." && exit 1)
echo "CLI docs are up-to-date"
```

**Step 3: Create `mise-tasks/docs/site/_default`**

```bash
#!/usr/bin/env bash
#MISE description="Start docs dev server at localhost:5173"
#MISE depends=["docs:site:setup"]
#MISE dir="docs"
set -euo pipefail

bunx vitepress dev
```

**Step 4: Create `mise-tasks/docs/site/setup`**

```bash
#!/usr/bin/env bash
#MISE description="Install docs site dependencies"
#MISE dir="docs"
set -euo pipefail

bun install
```

**Step 5: Create `mise-tasks/docs/site/build`**

```bash
#!/usr/bin/env bash
#MISE description="Build the docs site"
#MISE depends=["docs:site:setup"]
#MISE dir="docs"
set -euo pipefail

bunx vitepress build
```

**Step 6: Create `mise-tasks/docs/site/preview`**

```bash
#!/usr/bin/env bash
#MISE description="Preview the built docs site"
#MISE depends=["docs:site:build"]
#MISE dir="docs"
set -euo pipefail

bunx vitepress preview
```

**Step 7: Create `mise-tasks/docs/site/check`**

```bash
#!/usr/bin/env bash
#MISE description="Lint and format check docs site config"
#MISE depends=["docs:site:setup"]
#MISE dir="docs"
set -euo pipefail

bunx biome check
```

**Step 8: Create `mise-tasks/docs/site/format`**

```bash
#!/usr/bin/env bash
#MISE description="Lint and format docs site config (auto-fix)"
#MISE depends=["docs:site:setup"]
#MISE dir="docs"
set -euo pipefail

bunx biome check --write
```

**Step 9: Make all executable**

```bash
chmod +x mise-tasks/docs/cli/{gen,verify} mise-tasks/docs/site/{_default,setup,build,preview,check,format}
```

---

### Task 7: Create fmt task files

**Files:**

- Create: `mise-tasks/fmt/_default`
- Create: `mise-tasks/fmt/check`
- Create: `mise-tasks/fmt/docs/_default`
- Create: `mise-tasks/fmt/docs/check`

**Step 1: Create `mise-tasks/fmt/_default`**

```bash
#!/usr/bin/env bash
#MISE description="Auto-format all code (Rust + docs/YAML)"
set -euo pipefail

cargo fmt
bun run prettier --write '**/*.md' '**/*.{yml,yaml}'
```

**Step 2: Create `mise-tasks/fmt/check`**

```bash
#!/usr/bin/env bash
#MISE description="Check all code formatting (Rust + docs/YAML)"
set -euo pipefail

cargo fmt -- --check
bun run prettier --check '**/*.md' '**/*.{yml,yaml}'
```

**Step 3: Create `mise-tasks/fmt/docs/_default`**

```bash
#!/usr/bin/env bash
#MISE description="Format docs and YAML files with Prettier"
set -euo pipefail

bun run prettier --write '**/*.md' '**/*.{yml,yaml}'
```

**Step 4: Create `mise-tasks/fmt/docs/check`**

```bash
#!/usr/bin/env bash
#MISE description="Check docs and YAML formatting"
set -euo pipefail

bun run prettier --check '**/*.md' '**/*.{yml,yaml}'
```

**Step 5: Make all executable**

```bash
chmod +x mise-tasks/fmt/{_default,check} mise-tasks/fmt/docs/{_default,check}
```

---

### Task 8: Create lint task files

**Files:**

- Create: `mise-tasks/lint/_default`
- Create: `mise-tasks/lint/rust`

**Step 1: Create `mise-tasks/lint/_default`**

```bash
#!/usr/bin/env bash
#MISE description="Run all linting tools"
#MISE alias="l"
#MISE depends=["lint:rust"]
set -euo pipefail

echo "All linting completed"
```

**Step 2: Create `mise-tasks/lint/rust`**

```bash
#!/usr/bin/env bash
#MISE description="Run Rust linting (clippy + fmt + shellcheck)"
set -euo pipefail

echo "Running Rust linting..."
cargo clippy --tests -- -D warnings
cargo fmt --check
if command -v shellcheck >/dev/null 2>&1; then
    shellcheck tests/integration/*.sh
else
    echo "shellcheck not available, skipping integration test lint..."
fi
```

**Step 3: Make all executable**

```bash
chmod +x mise-tasks/lint/{_default,rust}
```

---

### Task 9: Create man task files

**Files:**

- Create: `mise-tasks/man/gen`
- Create: `mise-tasks/man/install`
- Create: `mise-tasks/man/verify`

**Step 1: Create `mise-tasks/man/gen`**

```bash
#!/usr/bin/env bash
#MISE description="Generate man pages to man/ directory using xtask"
set -euo pipefail

echo "Generating man pages..."
mkdir -p man
cargo run --package xtask -- gen-man --output-dir=man
echo "Man pages generated in man/"
```

**Step 2: Create `mise-tasks/man/install`**

```bash
#!/usr/bin/env bash
#MISE description="Install pre-generated man pages to system location"
#MISE depends=["man:gen"]
set -euo pipefail

echo "Installing man pages..."
mkdir -p ~/.local/share/man/man1
cp man/*.1 ~/.local/share/man/man1/
echo "Man pages installed to ~/.local/share/man/man1/"
echo ""
echo "Note: You may need to run 'mandb' or restart your shell for man to find the new pages"
```

**Step 3: Create `mise-tasks/man/verify`**

```bash
#!/usr/bin/env bash
#MISE description="Verify man pages are up-to-date with source"
set -euo pipefail

echo "Verifying man pages are up-to-date..."
cargo run --package xtask -- gen-man --output-dir=man
git diff --exit-code man/ || (echo "Man pages out of date. Run 'mise run man:gen' and commit." && exit 1)
echo "Man pages are up-to-date"
```

**Step 4: Make all executable**

```bash
chmod +x mise-tasks/man/{gen,install,verify}
```

---

### Task 10: Create setup task files

**Files:**

- Create: `mise-tasks/setup/_default`
- Create: `mise-tasks/setup/rust`

**Step 1: Create `mise-tasks/setup/_default`**

```bash
#!/usr/bin/env bash
#MISE description="Setup development environment"
#MISE depends=["setup:rust"]
set -euo pipefail

echo "Installing lefthook git hooks..."
lefthook install
echo "Development environment setup completed"
```

**Step 2: Create `mise-tasks/setup/rust`**

Copy the full contents from the current `tasks.setup-rust` in
`mise.toml:302-349`. This is the large script that builds, creates symlinks, and
prints binary size.

```bash
#!/usr/bin/env bash
#MISE description="Setup Rust development environment"
#MISE depends=["build"]
set -euo pipefail

echo "Setting up Rust development environment..."
chmod +x tests/integration/*.sh
echo "Creating symlinks in target/release/..."
cd target/release
# Core command symlinks
ln -sf daft git-worktree-clone
ln -sf daft git-worktree-init
ln -sf daft git-worktree-checkout
ln -sf daft git-worktree-checkout-branch
ln -sf daft git-worktree-prune
ln -sf daft git-worktree-carry
ln -sf daft git-worktree-fetch
ln -sf daft git-worktree-flow-adopt
ln -sf daft git-worktree-flow-eject
ln -sf daft git-daft
# Git-style shortcuts (default for development)
ln -sf daft gwtclone
ln -sf daft gwtinit
ln -sf daft gwtco
ln -sf daft gwtcb
ln -sf daft gwtprune
ln -sf daft gwtcarry
ln -sf daft gwtfetch
cd ../..
echo "Development environment ready!"
echo ""
# Cross-platform binary size display
if [[ "$OSTYPE" == "darwin"* ]]; then
    size=$(stat -f '%z' target/release/daft 2>/dev/null || echo "0")
else
    size=$(stat -c '%s' target/release/daft 2>/dev/null || echo "0")
fi
echo "Binary size: $(awk "BEGIN {printf \"%.0f KB\", $size/1024}")"
echo ""
echo "Add to PATH for Git integration:"
echo "  export PATH=\"$PWD/target/release:\$PATH\""
echo ""
echo "Quick test:"
echo "  ./target/release/daft"
echo "  ./target/release/git-worktree-clone --help"
echo "  ./target/release/gwtco --help  # Git-style shortcut"
```

**Step 3: Make all executable**

```bash
chmod +x mise-tasks/setup/{_default,rust}
```

---

### Task 11: Create test task files (top-level)

**Files:**

- Create: `mise-tasks/test/_default`
- Create: `mise-tasks/test/unit`
- Create: `mise-tasks/test/verbose`
- Create: `mise-tasks/test/perf`

**Step 1: Create `mise-tasks/test/_default`**

```bash
#!/usr/bin/env bash
#MISE description="Run all tests (unit + integration)"
#MISE alias="t"
#MISE depends=["test:unit", "test:integration"]
set -euo pipefail

echo "All tests completed"
```

**Step 2: Create `mise-tasks/test/unit`**

```bash
#!/usr/bin/env bash
#MISE description="Run Rust unit tests"
set -euo pipefail

echo "Running Rust unit tests..."
cargo test --lib --tests
```

**Step 3: Create `mise-tasks/test/verbose`**

```bash
#!/usr/bin/env bash
#MISE description="Run tests with verbose output"
#MISE depends=["build"]
#MISE env={VERBOSE = "1"}
set -euo pipefail

echo "Running integration tests with verbose output..."
cd tests/integration && ./test_all.sh
```

**Step 4: Create `mise-tasks/test/perf`**

```bash
#!/usr/bin/env bash
#MISE description="Run performance tests"
#MISE depends=["build"]
set -euo pipefail

echo "Running integration performance tests..."
cd tests/integration && time ./test_all.sh
```

**Step 5: Make all executable**

```bash
chmod +x mise-tasks/test/{_default,unit,verbose,perf}
```

---

### Task 12: Create test:integration task files

**Files:**

- Create: `mise-tasks/test/integration/_default`
- Create: `mise-tasks/test/integration/matrix`
- Create: `mise-tasks/test/integration/gitoxide`
- Create: `mise-tasks/test/integration/verbose`
- Create: `mise-tasks/test/integration/clone`
- Create: `mise-tasks/test/integration/init`
- Create: `mise-tasks/test/integration/prune`
- Create: `mise-tasks/test/integration/config`
- Create: `mise-tasks/test/integration/hooks`
- Create: `mise-tasks/test/integration/fetch`
- Create: `mise-tasks/test/integration/setup`
- Create: `mise-tasks/test/integration/checkout/_default`
- Create: `mise-tasks/test/integration/checkout/branch`
- Create: `mise-tasks/test/integration/flow/adopt`
- Create: `mise-tasks/test/integration/flow/eject`
- Create: `mise-tasks/test/integration/shell/init`
- Create: `mise-tasks/test/integration/unknown/command`

**Step 1: Create `mise-tasks/test/integration/_default`**

```bash
#!/usr/bin/env bash
#MISE description="Run integration tests (full matrix)"
#MISE depends=["dev:setup"]
set -euo pipefail

cargo run --package xtask -- test-matrix
```

**Step 2: Create `mise-tasks/test/integration/matrix`**

```bash
#!/usr/bin/env bash
#MISE description="Run integration tests for specific matrix entry"
#MISE depends=["dev:setup"]
#USAGE arg "<entry>" help="Matrix entry name (e.g., 'default', 'gitoxide')"
set -euo pipefail

entry="${usage_entry:?Usage: mise run test:integration:matrix <entry-name>}"
cargo run --package xtask -- test-matrix --entry "$entry"
```

**Step 3: Create `mise-tasks/test/integration/gitoxide`**

```bash
#!/usr/bin/env bash
#MISE description="Run integration tests with gitoxide backend"
#MISE depends=["dev:setup"]
set -euo pipefail

cargo run --package xtask -- test-matrix --entry gitoxide
```

**Step 4: Create `mise-tasks/test/integration/verbose`**

```bash
#!/usr/bin/env bash
#MISE description="Run integration tests with verbose output"
#MISE depends=["build"]
#MISE env={VERBOSE = "1"}
set -euo pipefail

echo "Running integration tests with verbose output..."
cd tests/integration && ./test_all.sh
```

**Step 5: Create individual integration test files**

Each follows the same pattern. Create these files:

`mise-tasks/test/integration/clone`:

```bash
#!/usr/bin/env bash
#MISE description="Run integration clone tests"
#MISE depends=["build"]
set -euo pipefail

echo "Running integration clone tests..."
cd tests/integration && ./test_clone.sh
```

`mise-tasks/test/integration/init`:

```bash
#!/usr/bin/env bash
#MISE description="Run integration init tests"
#MISE depends=["build"]
set -euo pipefail

echo "Running integration init tests..."
cd tests/integration && ./test_init.sh
```

`mise-tasks/test/integration/prune`:

```bash
#!/usr/bin/env bash
#MISE description="Run integration prune tests"
#MISE depends=["build"]
set -euo pipefail

echo "Running integration prune tests..."
cd tests/integration && ./test_prune.sh
```

`mise-tasks/test/integration/config`:

```bash
#!/usr/bin/env bash
#MISE description="Run integration config tests"
#MISE depends=["build"]
set -euo pipefail

echo "Running integration config tests..."
cd tests/integration && ./test_config.sh
```

`mise-tasks/test/integration/hooks`:

```bash
#!/usr/bin/env bash
#MISE description="Run integration hooks tests"
#MISE depends=["build"]
set -euo pipefail

echo "Running integration hooks tests..."
cd tests/integration && ./test_hooks.sh
```

`mise-tasks/test/integration/fetch`:

```bash
#!/usr/bin/env bash
#MISE description="Run integration fetch tests"
#MISE depends=["build"]
set -euo pipefail

echo "Running integration fetch tests..."
cd tests/integration && ./test_fetch.sh
```

`mise-tasks/test/integration/setup`:

```bash
#!/usr/bin/env bash
#MISE description="Run integration setup tests"
#MISE depends=["build"]
set -euo pipefail

echo "Running integration setup tests..."
cd tests/integration && ./test_setup.sh
```

`mise-tasks/test/integration/checkout/_default`:

```bash
#!/usr/bin/env bash
#MISE description="Run integration checkout tests"
#MISE depends=["build"]
set -euo pipefail

echo "Running integration checkout tests..."
cd tests/integration && ./test_checkout.sh
```

`mise-tasks/test/integration/checkout/branch`:

```bash
#!/usr/bin/env bash
#MISE description="Run integration checkout-branch tests"
#MISE depends=["build"]
set -euo pipefail

echo "Running integration checkout-branch tests..."
cd tests/integration && ./test_checkout_branch.sh
```

`mise-tasks/test/integration/flow/adopt`:

```bash
#!/usr/bin/env bash
#MISE description="Run integration flow-adopt tests"
#MISE depends=["build"]
set -euo pipefail

echo "Running integration flow-adopt tests..."
cd tests/integration && ./test_flow_adopt.sh
```

`mise-tasks/test/integration/flow/eject`:

```bash
#!/usr/bin/env bash
#MISE description="Run integration flow-eject tests"
#MISE depends=["build"]
set -euo pipefail

echo "Running integration flow-eject tests..."
cd tests/integration && ./test_flow_eject.sh
```

`mise-tasks/test/integration/shell/init`:

```bash
#!/usr/bin/env bash
#MISE description="Run integration shell-init tests"
#MISE depends=["build"]
set -euo pipefail

echo "Running integration shell-init tests..."
cd tests/integration && ./test_shell_init.sh
```

`mise-tasks/test/integration/unknown/command`:

```bash
#!/usr/bin/env bash
#MISE description="Run integration unknown-command tests"
#MISE depends=["build"]
set -euo pipefail

echo "Running integration unknown-command tests..."
cd tests/integration && ./test_unknown_command.sh
```

**Step 6: Make all executable**

```bash
chmod +x mise-tasks/test/integration/{_default,matrix,gitoxide,verbose,clone,init,prune,config,hooks,fetch,setup}
chmod +x mise-tasks/test/integration/checkout/{_default,branch}
chmod +x mise-tasks/test/integration/flow/{adopt,eject}
chmod +x mise-tasks/test/integration/shell/init
chmod +x mise-tasks/test/integration/unknown/command
```

---

### Task 13: Create validate task files

**Files:**

- Create: `mise-tasks/validate/_default`
- Create: `mise-tasks/validate/rust`

**Step 1: Create `mise-tasks/validate/_default`**

```bash
#!/usr/bin/env bash
#MISE description="Validate all code"
#MISE depends=["validate:rust"]
set -euo pipefail

echo "All validation completed"
```

**Step 2: Create `mise-tasks/validate/rust`**

```bash
#!/usr/bin/env bash
#MISE description="Validate Rust code and integration tests"
set -euo pipefail

echo "Validating Rust code and integration tests..."
cargo check
for script in tests/integration/*.sh; do
    if [ -f "$script" ]; then
        echo "Validating $script"
        bash -n "$script" || exit 1
    fi
done
echo "Rust code and integration tests validated successfully"
```

**Step 3: Make all executable**

```bash
chmod +x mise-tasks/validate/{_default,rust}
```

---

### Task 14: Create watch task files

**Files:**

- Create: `mise-tasks/watch/_default`
- Create: `mise-tasks/watch/unit`
- Create: `mise-tasks/watch/clippy`
- Create: `mise-tasks/watch/check`

**Step 1: Create `mise-tasks/watch/_default`**

```bash
#!/usr/bin/env bash
#MISE description="Watch and run unit tests on changes (alias for watch:unit)"
#MISE depends=["watch:unit"]
set -euo pipefail
```

**Step 2: Create `mise-tasks/watch/unit`**

```bash
#!/usr/bin/env bash
#MISE description="Watch and run unit tests on changes"
set -euo pipefail

if command -v cargo-watch >/dev/null 2>&1; then
    cargo watch -c -x "test --lib"
else
    echo "cargo-watch not installed. Install with: cargo install cargo-watch"
    exit 1
fi
```

**Step 3: Create `mise-tasks/watch/clippy`**

```bash
#!/usr/bin/env bash
#MISE description="Watch and run clippy + unit tests on changes"
set -euo pipefail

if command -v cargo-watch >/dev/null 2>&1; then
    cargo watch -c -x "clippy -- -D warnings" -x "test --lib"
else
    echo "cargo-watch not installed. Install with: cargo install cargo-watch"
    exit 1
fi
```

**Step 4: Create `mise-tasks/watch/check`**

```bash
#!/usr/bin/env bash
#MISE description="Watch and run cargo check on changes"
set -euo pipefail

if command -v cargo-watch >/dev/null 2>&1; then
    cargo watch -c -x check
else
    echo "cargo-watch not installed. Install with: cargo install cargo-watch"
    exit 1
fi
```

**Step 5: Make all executable**

```bash
chmod +x mise-tasks/watch/{_default,unit,clippy,check}
```

---

### Task 15: Strip tasks from mise.toml

**Files:**

- Modify: `mise.toml` -- remove everything from line 27 to end (all `[tasks.*]`
  sections)

**Step 1: Replace `mise.toml` contents**

Keep only lines 1-25 (tools, env, hooks). Remove all `[tasks.*]` definitions.

The resulting `mise.toml` should be:

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

# Auto-install JS dependencies (Prettier) when package.json changes
[hooks]
enter = """
mkdir -p .mise
hashfile=".mise/deps_hash"
current_hash=$(cat package.json bun.lock 2>/dev/null | md5 -q 2>/dev/null || cat package.json bun.lock 2>/dev/null | md5sum | cut -d' ' -f1)
if [ ! -d "node_modules" ] || [ ! -f "$hashfile" ] || [ "$(cat "$hashfile" 2>/dev/null)" != "$current_hash" ]; then
  echo "[mise] Installing JS dependencies..."
  bun install --frozen-lockfile 2>/dev/null || bun install
  current_hash=$(cat package.json bun.lock 2>/dev/null | md5 -q 2>/dev/null || cat package.json bun.lock 2>/dev/null | md5sum | cut -d' ' -f1)
  echo "$current_hash" > "$hashfile"
fi
"""
```

**Step 2: Verify mise discovers new tasks**

Run: `mise tasks`

Expected: All tasks appear with `:` naming (build, ci, clean, clean:rust,
clean:tests, clippy, completions:gen:bash, ..., watch:unit).

**Step 3: Verify a simple task runs**

Run: `mise run clippy`

Expected: Clippy runs successfully.

---

### Task 16: Update lefthook.yml

**Files:**

- Modify: `lefthook.yml:6,9,12,15,45`

**Step 1: Update task name references**

Replace these lines:

- Line 6: `run: mise run fmt-check` → `run: mise run fmt:check`
- Line 9: `run: mise run clippy` → `run: mise run clippy` (unchanged)
- Line 12: `run: mise run verify-man` → `run: mise run man:verify`
- Line 15: `run: mise run verify-cli-docs` → `run: mise run docs:cli:verify`
- Line 45: `run: mise run test-unit` → `run: mise run test:unit`

---

### Task 17: Update CI workflow

**Files:**

- Modify: `.github/workflows/test.yml:38,41,90,134`

**Step 1: Update task name references**

- Line 38: `run: mise run clippy` → unchanged
- Line 41: `run: mise run test-unit` → `run: mise run test:unit`
- Line 90: `echo "Run 'mise run gen-man' locally..."` →
  `echo "Run 'mise run man:gen' locally..."`
- Line 134: `echo "Run 'mise run gen-cli-docs' locally..."` →
  `echo "Run 'mise run docs:cli:gen' locally..."`

---

### Task 18: Update CLAUDE.md

**Files:**

- Modify: `CLAUDE.md:37-48,96,105-106,132-136`

**Step 1: Update Build, Test & Lint Commands section**

Replace the commands block (lines 37-44):

```bash
mise run dev                # Build + create symlinks (quick dev setup)
mise run test               # Run all tests (unit + integration)
mise run test:unit          # Rust unit tests only
mise run test:integration   # Integration tests only
mise run clippy             # Lint (must pass with zero warnings)
mise run fmt                # Auto-format code
mise run fmt:check          # Verify formatting
mise run ci                 # Simulate full CI locally
```

Update line 47-48:

```
IMPORTANT: Before committing, always run `mise run fmt`, `mise run clippy`, and
`mise run test:unit`. These checks are required and enforced in CI.
```

**Step 2: Update Adding a New Command section**

Line 96: `mise run gen-man` → `mise run man:gen`

**Step 3: Update Man Pages section**

Lines 105-106:

```bash
mise run man:gen        # Generate/update man pages
mise run man:verify     # Check if man pages are up-to-date (also runs in CI)
```

**Step 4: Update Docs Site section**

Lines 132-136:

```bash
mise run docs:site          # Dev server at localhost:5173
mise run docs:site:build    # Build the site
mise run docs:site:preview  # Preview built site
mise run docs:site:check    # Lint config with Biome
mise run docs:site:format   # Auto-fix config with Biome
```

---

### Task 19: Update CONTRIBUTING.md (root)

**Files:**

- Modify: `CONTRIBUTING.md:96-98,107-109,118-122`

**Step 1: Update Development Workflow section**

Lines 96-98:

```bash
mise run fmt
mise run clippy
mise run test
```

(these are unchanged)

**Step 2: Update Code Quality Requirements**

Line 109: `mise run fmt-check` → `mise run fmt:check`

**Step 3: Update Testing section**

Lines 121-122:

```bash
mise run test:unit          # Rust unit tests
mise run test:integration   # End-to-end tests
```

---

### Task 20: Update docs/contributing.md

**Files:**

- Modify: `docs/contributing.md:44-47,55-58,63,111-113,127`

**Step 1: Update quality check commands**

Lines 44-46:

```bash
mise run fmt
mise run clippy
mise run test:unit
```

**Step 2: Update Code Quality Requirements**

Lines 55-58:

```
- **Formatting:** `mise run fmt:check`
- **Linting:** `mise run clippy` (zero warnings)
- **Unit tests:** `mise run test:unit`
- **Integration tests:** `mise run test:integration`
```

Line 63: unchanged (`mise run ci`)

**Step 3: Update Testing section**

Lines 111-113:

```bash
mise run test              # Run all tests
mise run test:unit         # Rust unit tests only
mise run test:integration  # End-to-end tests
```

**Step 4: Update Adding a New Command section**

Line 127: `mise run gen-man` → `mise run man:gen` and `mise run gen-cli-docs` →
`mise run docs:cli:gen`

---

### Task 21: Update RELEASING.md and SETUP_RELEASE.md

**Files:**

- Modify: `RELEASING.md:9-10,162-171,383-384`
- Modify: `SETUP_RELEASE.md:183-192`

**Step 1: Update RELEASING.md**

Line 9: `mise run test` → unchanged Line 10: `mise run clippy`,
`mise run fmt-check` → `mise run fmt:check` Lines 162-171:

```bash
mise run test
# ...
mise run clippy
# ...
mise run fmt:check
# ...
mise run build
```

Line 383: `mise run clippy` → unchanged Line 384: `mise run fmt-check` →
`mise run fmt:check`

**Step 2: Update SETUP_RELEASE.md**

Lines 183-192: Same pattern -- `fmt-check` → `fmt:check`, rest unchanged.

---

### Task 22: Update tests/README.md

**Files:**

- Modify: `tests/README.md:39-54`

**Step 1: Update Running Tests section**

```bash
# Run all tests (unit + integration)
mise run test

# Run only unit tests
mise run test:unit

# Run only integration tests
mise run test:integration

# Run specific test suites
mise run test:integration:clone
mise run test:integration:init
mise run test:integration:checkout

# Run with verbose output
mise run test:verbose
mise run test:integration:verbose
```

---

### Task 23: Update docs/getting-started/installation.md

**Files:**

- Modify: `docs/getting-started/installation.md:156`

**Step 1: Update command reference**

Line 156: `mise run dev-setup` → `mise run dev:setup`

---

### Task 24: Update agent configs

**Files:**

- Modify: `.claude/agents/rust-architect.md:98,101,104,107`
- Modify: `.claude/agents/code-reviewer.md:71`

**Step 1: Update rust-architect.md**

Lines 98-107:

```bash
mise run fmt
# ...
mise run clippy
# ...
mise run test:unit
# ...
mise run test:integration:[command]
```

(Only line 104 and 107 change: `test-unit` → `test:unit`,
`test-integration-[command]` → `test:integration:[command]`)

**Step 2: Update code-reviewer.md**

Line 71: unchanged (`mise run clippy`)

---

### Task 25: Verify everything works end-to-end

**Step 1: List all tasks**

Run: `mise tasks`

Expected: All tasks listed with `:` naming, descriptions visible, no old `-`
names.

**Step 2: Run a dependency chain**

Run: `mise run dev:verify` (or `mise run clippy` if already built)

Expected: Task resolves and executes correctly.

**Step 3: Check lefthook still works**

Run: `lefthook run pre-commit`

Expected: All hooks run with new task names.

**Step 4: Commit**

```bash
git add mise-tasks/ mise.toml lefthook.yml CLAUDE.md CONTRIBUTING.md \
  docs/contributing.md tests/README.md RELEASING.md SETUP_RELEASE.md \
  .github/workflows/test.yml docs/getting-started/installation.md \
  .claude/agents/rust-architect.md .claude/agents/code-reviewer.md
git commit -m "chore: reorganize mise tasks into file-based structure with : hierarchy"
```
