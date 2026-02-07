# CLAUDE.md

## Critical Rules

IMPORTANT: These rules must NEVER be violated:

1. **Never modify global git config** — Do not change global git user name, email, or any other global settings. This applies to both manual work and automated tests. Tests must use local config only.
2. **Never use this repository for testing** — This project's own git repo must never be used as a test subject for worktree commands. Tests must always create isolated temporary repositories.

## Build, Test & Lint Commands

```bash
mise run dev                # Build + create symlinks (quick dev setup)
mise run test               # Run all tests (unit + integration)
mise run test-unit          # Rust unit tests only
mise run test-integration   # Integration tests only
mise run clippy             # Lint (must pass with zero warnings)
mise run fmt                # Auto-format code
mise run fmt-check          # Verify formatting
mise run ci                 # Simulate full CI locally
```

IMPORTANT: Before committing, always run `mise run fmt`, `mise run clippy`, and `mise run test-unit`. These checks are required and enforced in CI.

## Architecture

**Multicall binary**: All commands route through a single `daft` binary (`src/main.rs`). The binary examines `argv[0]` to determine which command was invoked, then dispatches to the matching module in `src/commands/`. Symlinks like `git-worktree-clone → daft` enable Git subcommand discovery. Shortcut aliases (e.g., `gwtco`) are resolved in `src/shortcuts.rs` before routing.

**Shell integration**: `daft shell-init` generates shell wrappers that set `DAFT_SHELL_WRAPPER=1`. When set, commands output `__DAFT_CD__:/path` markers that wrappers parse to `cd` into new worktrees.

**Hooks system**: Lifecycle hooks in `.daft/hooks/` with trust-based security. Hook types: `post-clone`, `post-init`, `worktree-pre-create`, `worktree-post-create`, `worktree-pre-remove`, `worktree-post-remove`. Old names without `worktree-` prefix are deprecated (removed in v2.0.0).

## Branch Naming & PRs

- Branch names: `daft-<issue number>/<shortened issue name>`
- PRs target `master` and are always **squash merged** (linear history required)
- PR titles use conventional commit format: `feat: add dark mode toggle`
- Issue references go in PR body, not title: `Fixes #42`

## Commits

[Conventional Commits](https://www.conventionalcommits.org/) format: `<type>[scope]: <description>`

Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `chore`, `ci`

## Release Process

Uses [release-plz](https://release-plz.dev/): push to master → Release PR auto-created → merge it → GitHub Release + tag → binary builds. All commits produce patch bumps; edit `Cargo.toml` in the Release PR for minor/major bumps.

## Adding a New Command

1. Create `src/commands/<name>.rs` with clap `Args` struct (include `about`, `long_about`, `arg(help)` attributes for man pages)
2. Add module to `src/commands/mod.rs`
3. Add routing in `src/main.rs`
4. Add to `COMMANDS` array and `get_command_for_name()` in `xtask/src/main.rs`
5. Add to help output in `src/commands/docs.rs` (`get_command_categories()`)
6. Run `mise run gen-man` and commit the generated man page
7. Add integration tests in `tests/integration/` following existing patterns

## Man Pages

Pre-generated in `man/` and committed. Regenerate after changing command help text:

```bash
mise run gen-man      # Generate/update man pages
mise run verify-man   # Check if man pages are up-to-date (also runs in CI)
```

## Documentation Site

`docs/` contains the project documentation (VitePress/Markdown). Update when adding or changing user-facing features.

- `docs/getting-started/` — installation, quick start, shell integration
- `docs/guide/` — in-depth guides (hooks, configuration, workflow, shortcuts)
- `docs/cli/` — one reference page per command, follow `docs/cli/daft-doctor.md` as template
- Every page needs `title` and `description` YAML frontmatter
- No emoji in docs
