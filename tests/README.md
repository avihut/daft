# Test Suite

Two test systems: **bash integration tests** (legacy) and **YAML manual tests**
(declarative, preferred for new tests).

## Structure

```
tests/
├── integration/              # Bash integration tests
│   ├── test_all.sh           # Master runner (all suites)
│   ├── test_framework.sh     # Shared assertions and helpers
│   └── test_*.sh             # Per-command test suites
├── manual/
│   ├── scenarios/            # YAML test scenarios
│   │   ├── checkout/         # One directory per command
│   │   ├── clone/
│   │   ├── init/
│   │   └── ...               # 18 command directories
│   └── fixtures/
│       └── repos/            # Shared repo templates
│           └── standard-remote.yml
└── README.md
```

## Running Tests

```bash
# All tests (unit + integration matrix)
mise run test

# Unit tests only
mise run test:unit

# Bash integration tests (full matrix: default + gitoxide)
mise run test:integration

# YAML manual tests (all 252 scenarios, automatic — default)
mise run test:manual

# YAML tests for a specific command (automatic)
mise run test:manual tests/manual/scenarios/checkout/

# YAML tests by namespace (automatic)
mise run test:manual checkout:basic

# Interactive mode (step through with prompts)
mise run test:manual -- -i checkout:basic

# List all available scenarios
mise run test:manual -- --list
```

### Verbosity ladder

Per #518, the runner output has four levels (`-q` / default / `-v` / `-vv`):

| Flag   | Pass footer | Fail footer | Cleanup line | Header + path | Per-step lines | Check icons | Capture on fail   | Capture on pass   | Expanded `$ command` |
| ------ | ----------- | ----------- | ------------ | ------------- | -------------- | ----------- | ----------------- | ----------------- | -------------------- |
| `-q`   | no          | yes         | fail only    | no            | no             | no          | no (summary only) | no                | no                   |
| (none) | yes         | yes         | fail only    | no            | no             | no          | no (summary only) | no                | no                   |
| `-v`   | yes         | yes         | fail only    | yes           | yes            | no          | first 20 lines    | no                | no                   |
| `-vv`  | yes         | yes         | fail only    | yes           | yes            | yes         | full, no line cap | full, no line cap | yes                  |

Default sits where `cargo test`, vitest, and pytest sit: one footer line per
scenario, with the duration + optional `(slow)` annotation doing at-a-glance
outlier work. Old default detail (header, dim path, per-step
`[N/M] name ... ok | FAIL`, inline capture on fail) lives at `-v`; "firehose
everything, uncapped" lives at `-vv`. `-q` collapses passing scenarios entirely
— only failed footers + the cleanup line + the shared summary block surface.

The "Cleanup line" column is the `Cleaned up test environment.` message — it
emits flush against the footer on a failed scenario at every verbosity (`-q`
included), and is suppressed entirely on green to avoid noise on the happy path.
The "Capture on fail" cells say "no (summary only)" at `-q` / default because
the inline per-step capture is suppressed there, but the end-of-run failures
block still carries the full capture so the reader gets the failure payload
either way.

Each scenario ends with a `✓` / `✗` footer carrying its wall-clock duration
(`✓ name  142ms` / `✗ name  2.3s`). Scenarios over 5s get a `(slow)` annotation
so slow tests are visible at a glance even at default verbosity. Per-step
captured output (at `-v` upward) is rendered as separate `--- stdout ---` and
`--- stderr ---` blocks so the reader can tell which stream produced the noise.

At the end of a run the summary shows a `⎯⎯⎯ Failed Scenarios (N) ⎯⎯⎯` banner
(when there are failures) followed by numbered entries — each with the
scenario-relative `path:line` location pointer on its own line
(terminal-clickable), a `❯` marker on the failing step, the failed assertions,
and any captured stdout/stderr. Then come separate `Scenarios:` / `Steps:`
lines, `Duration:`, an optional `parallel jobs: N` suffix, and a `Reproduce:`
block with one `mise run test:manual -- <token>` per failure.

### TTY live progress region

On a TTY, the runner shows a pinned live region at the bottom of the terminal
during a parallel run: one row per in-flight scenario (carrying scenario name
with step counter, step name, and a yellow `(slow)` annotation once elapsed
crosses 5s) plus a summary bar (`⠋  [42/252]  4 running  ◆  1 failed  ◆  0:23`).
Completed scenarios stream their per-tier scrollback content above the region in
**completion order** as they finish.

On non-TTY (CI logs, redirected output, `cargo run`, `| cat`), the region is
suppressed entirely and output reverts to today's behavior: input-order drain at
end, no live bar. CI logs stay byte-identical. The non-TTY path is also forced
by `CI=1` or `NO_PROGRESS=1` even when stderr is a TTY (some CI runners flag
stderr as a TTY but bar redraws still pollute the logs).

The region is orthogonal to verbosity — all four tiers (`-q` / default / `-v` /
`-vv`) get it on TTY. `-q` benefits the most, since its scrollback is silent on
a fully green run; the bar is the entire heartbeat. Failures still surface in
scrollback at `-q` (fail footer + cleanup line) so the contract holds.

## Benchmarks

```bash
# TUI benchmark: bash vs YAML side-by-side (live table with spinners)
mise run bench:tests:integration

# Parallel mode (bash + YAML for same suite run concurrently)
mise run bench:tests:integration -- --parallel

# YAML-only benchmark with timing
mise run bench:tests:manual
```

## YAML Manual Test Framework

### Scenario format

Each `.yml` file in `tests/manual/scenarios/` defines a test scenario:

```yaml
name: Checkout basic
description: Checkout existing branch from cloned repo

repos:
  - name: test-repo
    use_fixture: standard-remote # shared template

steps:
  - name: Clone the repository
    run: git-worktree-clone $REMOTE_TEST_REPO
    expect:
      exit_code: 0
      dirs_exist:
        - "$WORK_DIR/test-repo/main"

  - name: Checkout develop branch
    run: git-worktree-checkout develop
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0
      dirs_exist:
        - "$WORK_DIR/test-repo/develop"
      is_git_worktree:
        - dir: "$WORK_DIR/test-repo/develop"
          branch: develop
```

### Available assertions

| Assertion             | Description                                      |
| --------------------- | ------------------------------------------------ |
| `exit_code`           | Expected exit code                               |
| `dirs_exist`          | Directories that must exist                      |
| `files_exist`         | Files that must exist                            |
| `files_not_exist`     | Files that must NOT exist                        |
| `file_contains`       | File must contain substring                      |
| `file_not_contains`   | File must NOT contain substring                  |
| `output_contains`     | Command stdout+stderr must contain substring     |
| `output_not_contains` | Command stdout+stderr must NOT contain substring |
| `is_git_worktree`     | Directory is a git worktree on expected branch   |
| `branch_exists`       | Branch exists in a repo                          |

### Variables

| Variable         | Description                                                           |
| ---------------- | --------------------------------------------------------------------- |
| `$WORK_DIR`      | Sandbox working directory                                             |
| `$BASE_DIR`      | Sandbox root (parent of work/ and remotes/)                           |
| `$BINARY_DIR`    | Path to built daft binaries                                           |
| `$REMOTE_<NAME>` | Path to generated bare repo (name uppercased, hyphens to underscores) |

### Shared fixtures

Place reusable repo templates in `tests/manual/fixtures/repos/`. Reference them
with `use_fixture`:

```yaml
repos:
  - name: my-repo
    use_fixture: standard-remote # loads standard-remote.yml with {{NAME}} substitution
```

### Path convention

All assertion paths and `cwd` values must use `$WORK_DIR/` prefix for correct
resolution:

```yaml
# Correct
dirs_exist:
  - "$WORK_DIR/test-repo/develop"

# Wrong (resolves relative to cwd, causes double-nesting)
dirs_exist:
  - "test-repo/develop"
```

## Adding New Tests

### YAML scenario (preferred)

1. Create `tests/manual/scenarios/<command>/<test-name>.yml`
2. Use `standard-remote` fixture or define inline repos
3. Run: `mise run test:manual <command>:<test-name>`

### Bash integration test

1. Add test function to `tests/integration/test_<command>.sh`
2. Register in `run_<command>_tests()` function
3. Run: `cd tests/integration && bash ./test_<command>.sh`

## CI Integration

The GitHub Actions workflow runs both test systems for each matrix entry
(default + gitoxide config):

1. Bash: `test_all.sh` with `GIT_CONFIG_GLOBAL`
2. YAML: `xtask manual-test` with same config
3. Shell completions: `test_completions.sh`
4. Help commands: verify all binaries respond to `--help`
