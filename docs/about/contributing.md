---
title: Contributing
description: Guidelines for contributing to daft
---

Thank you for your interest in contributing to daft!

## Development Setup

1. **Clone the repository** using daft's worktree layout:

   ```bash
   daft clone git@github.com:avihut/daft.git
   ```

2. **Build and set up the development environment:**

   ```bash
   mise run dev
   ```

   This builds the binary, creates all symlinks in `target/release/`, and
   verifies the setup.

3. **Add the binary to your PATH:**

   ```bash
   export PATH="$PWD/target/release:$PATH"
   ```

## Development Workflow

1. Create a feature branch:

   ```bash
   daft start daft-XX/feature-name
   ```

2. Make changes following the conventions below.

3. Run quality checks before committing:

   ```bash
   mise run fmt
   mise run clippy
   mise run test:unit
   ```

4. Submit a pull request targeting `master`.

## Code Quality Requirements

All PRs must pass these checks (enforced in CI):

- **Formatting:** `mise run fmt:check`
- **Linting:** `mise run clippy` (zero warnings)
- **Unit tests:** `mise run test:unit`
- **Integration tests:** `mise run test:integration` (bash + YAML, full matrix)

Run the full CI simulation locally:

```bash
mise run ci
```

## Testing

daft has two test systems that run in CI for each matrix entry (default +
gitoxide):

### YAML manual tests (preferred for new tests)

Declarative test scenarios in `tests/manual/scenarios/`. Each YAML file defines
repos to create, steps to run, and expectations to verify.

```bash
mise run test:manual                       # Run all 581 scenarios (automatic, default)
mise run test:manual checkout              # Run one command's tests (automatic)
mise run test:manual -- -i checkout:basic  # Step through one scenario interactively
mise run test:manual -- --list             # List all available scenarios
```

#### Ephemeral RAM-disk sandbox

When the dev box is IO-bound (especially running two worktree suites
concurrently), swap in the RAM-backed sandbox:

```bash
mise run test:manual:ramdisk               # All scenarios, ephemeral RAM mount
mise run test:manual:ramdisk -- -j 4       # CLI args flow through to xtask
```

The task allocates a per-run RAM volume (macOS:
`/Volumes/daft-ramdisk-test-<PID>` via `hdiutil`/`diskutil`; Linux:
`/dev/shm/daft-ramdisk-test-<PID>`), exports `DAFT_MANUAL_TEST_BASE` so the
runner uses it, and tears the mount down on EXIT, INT, or TERM. Two concurrent
shells get two independent mounts. SIGKILL leaks one mount per crash; recover
with `diskutil eject /Volumes/daft-ramdisk-test-*` (macOS) or
`rm -rf /dev/shm/daft-ramdisk-test-*` (Linux).

Configure with `DAFT_RAMDISK_SIZE_MB` (macOS only, default 4096; Linux tmpfs is
kernel-sized). This does **not** replace the default `test:manual` task — it's
an opt-in alternative. Unit tests for daft's filesystem-physical primitives
(`cow_copy`, `store::connection`) use `tempfile::tempdir()` and aren't affected
by `DAFT_MANUAL_TEST_BASE`, so real-disk coverage is preserved separately.

#### Cross-worktree shared daft binary

`mise run test:manual` (and the `:ramdisk` variant) automatically builds the
`daft` and `xtask` binaries into a content-hashed location under
`.git/.daft-shared-bin/<state-hash>/`, then points the test runner at it via
`DAFT_BINARY_DIR`. Sibling worktrees at the same source state skip the build
entirely and reuse the same binary — the most visible win is on the second
concurrent worktree run, which used to pay the full release-build cost before
any test could start.

The state hash covers HEAD plus the working-tree blob hash of every tracked
`*.rs` file, every `Cargo.toml`, and `Cargo.lock`, so editing any workspace
crate (including `term-styles`) invalidates the cache cleanly. Concurrent
populate attempts publish atomically (`rename(2)` of the staging directory) so
two simultaneous fresh-state runs never corrupt the cache — the loser cleans up
its staging dir and re-uses the winner's binary.

To bypass the shared bin for a single run (e.g. to test a hand-built binary with
custom features), set `DAFT_BINARY_DIR` explicitly:

```bash
DAFT_BINARY_DIR="$(pwd)/target/release" mise run test:manual
```

To wipe the cache entirely (e.g. accumulated state-hash directories taking disk
space):

```bash
rm -rf "$(git rev-parse --git-common-dir)/.daft-shared-bin"
```

The cache lives under `$(git rev-parse --git-common-dir)/.daft-shared-bin/` (the
repo's shared `.git/` directory — the bare repo at the project root in daft's
worktree layout, not the per-worktree `.git/` file), so it goes away with the
project, never under XDG state.

To add a new test, create a `.yml` file in the appropriate command directory:

```yaml
name: Checkout basic
repos:
  - name: test-repo
    use_fixture: standard-remote
steps:
  - name: Clone and checkout
    run: git-worktree-clone $REMOTE_TEST_REPO
    expect:
      exit_code: 0
      dirs_exist:
        - "$WORK_DIR/test-repo/main"
```

See `tests/README.md` for the full schema reference (assertions, variables,
fixtures, path conventions).

### Bash integration tests

Shell-based tests in `tests/integration/`. The master runner `test_all.sh`
executes all suites.

```bash
mise run test:integration              # Full matrix (default + gitoxide)
cd tests/integration && bash test_clone.sh   # Run one suite directly
```

### Dev sandbox

The sandbox provides an isolated environment for manual testing of daft
commands. Each worktree gets its own sandbox with a clean git identity and
locally-built binaries on PATH.

```bash
mise run sandbox                       # Enter the sandbox shell
mise run sandbox:setup                 # Install sandbox shell function into RC file
mise run sandbox:clean                 # Remove sandbox for this worktree
```

Inside the sandbox you can set up test scenarios and explore them interactively:

```bash
# Set up a test scenario and cd into its working directory
mise run test:manual -- --setup-only checkout:basic

# Reset the test environment to its initial state
mise run test:manual -- --setup-only --step 1 checkout:basic
```

### Benchmarks

Compare bash and YAML test performance with a live TUI table:

```bash
mise run bench:tests:integration               # Sequential (default)
mise run bench:tests:integration -- --parallel  # Parallel bash+YAML per suite
```

## Commit Messages

This project uses [Conventional Commits](https://www.conventionalcommits.org/)
for automatic changelog generation.

Format: `<type>[scope]: <description>`

| Type       | Description                  |
| ---------- | ---------------------------- |
| `feat`     | New feature                  |
| `fix`      | Bug fix                      |
| `docs`     | Documentation changes        |
| `style`    | Code style (no logic change) |
| `refactor` | Code refactoring             |
| `perf`     | Performance improvement      |
| `test`     | Adding/updating tests        |
| `chore`    | Maintenance tasks            |
| `ci`       | CI/CD changes                |

Examples:

```bash
feat(checkout): add --force flag for overwriting worktrees
fix: resolve branch name parsing for names with slashes
docs: update installation instructions
```

## Pull Request Guidelines

- PR titles use conventional commit format: `feat: add dark mode toggle`
- Issue references go in the PR body, not the title: `Fixes #42`
- All PRs target `master` and are squash-merged

## Branch Naming

Follow the convention: `daft-<issue-number>/<short-description>`

```
daft-42/dark-mode
daft-15/branch-search
```

## Adding a New Command

1. Create `src/commands/<name>.rs` with a clap `Args` struct (include `about`,
   `long_about`, `arg(help)` attributes)
2. Add the module to `src/commands/mod.rs`
3. Add routing in `src/main.rs`
4. Add to `COMMANDS` array and `get_command_for_name()` in `xtask/src/main.rs`
5. Add to help output in `src/commands/docs.rs` (`get_command_categories()`)
6. Run `mise run man:gen` and `mise run docs:cli:gen` and commit the generated
   files
7. Add YAML test scenarios in `tests/manual/scenarios/<name>/`
8. Add bash integration tests in `tests/integration/`

## License

daft is dual-licensed under MIT (LICENSE-MIT) and Apache-2.0 (LICENSE-APACHE) to
match Rust-ecosystem norms (Apache-2.0 carries an explicit patent grant).

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the project by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.

### Dependency licenses

daft enforces a permissive-license allowlist on every transitive crate via
[`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny). The policy lives in
[`deny.toml`](https://github.com/avihut/daft/blob/master/deny.toml) at the repo
root and is gated in CI (`.github/workflows/test.yml`) on every PR that touches
`Cargo.toml`, `Cargo.lock`, or `deny.toml`. The same check also fails the PR on
any open RustSec advisory or a dep sourced from an unexpected registry.

If a new dependency carries a license not on the allowlist (or you need to
ignore an advisory), add a `[licenses.exceptions]` or `advisories.ignore` entry
to `deny.toml` with a `# why:` comment explaining the rationale. Run
`mise run deny` locally to verify before pushing.
