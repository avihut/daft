# CLAUDE.md

## Critical Rules

IMPORTANT: These rules must NEVER be violated:

1. **Never modify global git config** — Do not change global git user name,
   email, or any other global settings. This applies to both manual work and
   automated tests. Tests must use local config only.
2. **Never use this repository for testing** — This project's own git repo must
   never be used as a test subject for worktree commands. Tests must always
   create isolated temporary repositories.
3. **Don't weaken `.github/workflows/claude-pr-review.yml`** — its security
   properties are deliberate: SHA-pinned actions, `environment: claude-review`
   (master-only deployment-branch policy), `author_association == 'OWNER'`
   gating on the `/claude review` comment trigger, no `pull_request` or
   `pull_request_target` event, and `timeout-minutes`. Never re-introduce a
   `pull_request[_target]` trigger. Never reference `CLAUDE_CODE_OAUTH_TOKEN`
   from any other workflow. Never replace SHA pins with mutable refs (Dependabot
   keeps them current). Workflow changes are CODEOWNERS-gated to `@avihut`.
4. **Don't reintroduce `unsafe` in production code paths** — `src/lib.rs` uses
   `#![cfg_attr(not(test), forbid(unsafe_code))]` and `src/main.rs` uses
   `#![forbid(unsafe_code)]`. Tests can wrap `env::set_var`/`remove_var` in
   `unsafe { … }` (these became `unsafe fn` in edition 2024); production code
   cannot. If you need a libc-style primitive, use the `nix` safe wrappers
   (already a dep with `signal` + `term` features). `forbid` cannot be
   `#[allow]`-overridden at any inner scope — that's the whole point — so adding
   `#[allow(unsafe_code)]` to a function will fail to compile. If a genuine need
   arises, refactor the design (e.g. fork→spawn) rather than weakening the lint.
   When picking dependencies, prefer those with a fully safe public API — SQLite
   via `rusqlite` was chosen over LMDB via `heed` partly because
   `heed::Env::open` is `unsafe fn`.

## Safe Local Testing with Git

When manually testing git operations (e.g. cloning, worktree creation) in
scratch directories:

- **Never run `git config --global`** — not even temporarily. Use
  `git config --local` or per-command env vars instead.
- **Set test identity via environment variables**, not config:
  ```bash
  GIT_AUTHOR_NAME="Test" GIT_AUTHOR_EMAIL="test@test.com" \
  GIT_COMMITTER_NAME="Test" GIT_COMMITTER_EMAIL="test@test.com" \
  git commit -m "test"
  ```
- **Never commit to this repo's branches from scratch/temp directories** — stray
  commits from test repos must not leak onto working branches.
- **Always `cd` back to the worktree directory after testing** — a deleted temp
  directory as cwd will silently break subsequent shell commands.
- **Clean up temp directories** when done: `rm -rf /tmp/daft-test-*` or use
  `mktemp -d`.

## Build, Test & Lint Commands

```bash
mise run dev                # Build + create symlinks (quick dev setup)
mise run test               # Run all tests (unit + integration)
mise run test:unit          # Rust unit tests only
mise run test:integration   # Integration tests (bash + YAML, full matrix)
mise run test:manual                       # YAML manual tests (all scenarios, automatic)
mise run test:manual checkout              # YAML tests for one command (automatic)
mise run test:manual -- -i checkout:basic  # Step through one scenario interactively
mise run clippy             # Lint (must pass with zero warnings)
mise run fmt                # Auto-format code
mise run fmt:check          # Verify formatting
mise run ci                 # Simulate full CI locally
mise run bench:tests:integration       # Benchmark bash vs YAML (TUI)
```

IMPORTANT: Before committing, always run `mise run fmt`, `mise run clippy`, and
`mise run test:unit`. These checks are required and enforced in CI.

IMPORTANT: Every bug fix must include a regression test that reproduces the
issue. Add a YAML scenario in `tests/manual/scenarios/` or a unit test that
fails without the fix and passes with it.

## Profiling

Benchmarking (prove a change is faster) uses the existing `mise run bench:*` /
`bench:tests:*` infra and `DAFT_MANUAL_TEST_EMIT_TIMING`. For _profiling_ (find
where the time goes) and any perf optimization work, read the `profiling-daft`
skill (`.claude/skills/profiling-daft/`): it covers the macOS toolchain
(samply/hyperfine, why dtrace is out), idle-gating, the shared-bin/
`DAFT_BINARY_DIR` A/B trick, and a baseline map of the suite's runtime. Build
for a profiler with `cargo build --profile profiling`.

## Test Hygiene

**Run `git` subprocesses through `crate::utils::git_command_at(dir)`.**
Inherited `GIT_DIR` silently overrides `-C <dir>` for repo discovery, so a query
like "is this file tracked in _this_ dir's repo?" retargets the parent's repo
when daft runs inside a git hook (pre-push, pre-commit, post-checkout) — and
`mise run test:unit` from a hook inherits the same vars. The helper clears the
relevant `GIT_*` vars so `-C` is authoritative.

**Redirect both pipes on every `git` `.status()` invocation — including in
tests.** Use `.stdout(Stdio::null()).stderr(Stdio::null())` or `.output()`. `-q`
only silences stdout for some subcommands and never touches stderr, so it is not
a substitute. Production two-stage probes that expect
`fatal: not a git repository` as a negative signal must suppress the stderr at
the call site.

## XDG Conventions

This project follows the XDG Base Directory Specification. Use the `dirs` crate
for cross-platform path resolution. Never hardcode `~/` paths for config or data
storage.

## Dependency Cooldown

7-day age gate across mise tools, Bun (`bunfig.toml`), Dependabot version PRs,
and a CI lockfile-age check (`scripts/check-lockfile-age.sh`). Dependabot
security PRs bypass automatically.

When adding a package, pin to a version ≥7 days old: `cargo add foo@<version>`
or `bun add foo@<version>`. To bypass, add an entry (with `# why:` rationale) to
`.dep-age-allowlist`, `bunfig.toml` `minimumReleaseAgeExcludes`, or
`cooldown.toml`; emergency CI override is `ALLOW_FRESH_DEPS=1`.

## Database & Storage

**SQLite via `rusqlite` is daft's canonical structured-data store.** All new
persistent state goes through `src/store/`. The shape, security defaults, and
migration runner are all there; consumers wire it up through
`src/coordinator/ports/` + `src/coordinator/adapters/` rather than touching
SQLite directly.

- Do not introduce other embedded databases (redb, sled, fjall, RocksDB, persy).
  The redb store this PR replaced is the cautionary tale: it took a
  process-exclusive file lock that didn't fit daft's coordinator + CLI access
  pattern, which forced a `meta.json` dual-write as a compensating workaround.
  See
  `~/.claude/projects/-Users-avihu-Projects-daft/memory/project_redb_concurrency_mismatch.md`.
- `rusqlite` uses the `bundled` feature so SQLite is compiled from source into
  the daft binary (~+1 MB on release builds, +C compiler at build time). The
  trade-off is intentional: every installation gets the same SQLite version, no
  system-library skew, WAL mode works out of the box.
- WAL mode does not work over network filesystems (NFS, SMB). Daft state must
  live on a local filesystem.
- The pool has two halves: a writer pool sized 1 (enforces WAL's single-writer
  ordering, cheaper than waiting on `SQLITE_BUSY`) and a multi-slot reader pool
  opened `SQLITE_OPEN_READ_ONLY` with a tight `busy_timeout` (300 ms) so
  CLI/completion paths fail fast instead of blocking the user's shell.
- Future registries (trust, repo catalog) will migrate to this layer in their
  own PRs.

## MSRV Policy

MSRV = **`latest_stable - 2`**. Wide enough to keep distro and corporate
installs working, narrow enough that we can adopt new stdlib/language features
~3 months after stabilization.

The version is declared in `Cargo.toml` `rust-version` and the workspace member
`term-styles/Cargo.toml`; other places that mention it (`mise.toml`,
`.github/workflows/test.yml`, `README.md`,
`docs/getting-started/installation.md`) must match — `rg '1\.\d+'` after a bump
finds the stragglers.

`.github/workflows/msrv-staleness-check.yml` runs monthly and opens a tracking
issue when the gap exceeds 2 minor versions; it auto-closes when a bump PR
lands. **It does not auto-bump.** Bumps surface clippy-lint escalations and
transitive-dep MSRV conflicts that need human triage, so always run
`rustup run <new> cargo check --all-targets` and `mise run clippy` before
landing.

## Architecture

**Strategic intent lives in [ARCHITECTURE.md](./ARCHITECTURE.md).** That file
owns the _why_ and the long-term direction: hexagonal at subsystem boundaries,
functional core inside domain modules, vertical slice at the CLI command layer,
the future crate decomposition, and the native agentic IDE spin-out that
motivates ports at crate edges. This section owns operational detail (current
patterns, conventions, hard rules) — when introducing a new subsystem or making
architectural decisions, read both. ARCHITECTURE.md tells you which direction to
point; this section tells you the conventions to follow getting there.

**Multicall binary**: All commands route through a single `daft` binary
(`src/main.rs`). The binary examines `argv[0]` to determine which command was
invoked, then dispatches to the matching module in `src/commands/`. Symlinks
like `git-worktree-clone → daft` enable Git subcommand discovery. Shortcut
aliases (e.g., `gwtco`) are resolved in `src/shortcuts.rs` before routing.

**Shell integration**: `daft shell-init` generates shell wrappers that create a
temp file and pass its path via `DAFT_CD_FILE`. When set, commands write the cd
target to that file, and the wrapper reads it after the command finishes to `cd`
into new worktrees. Stdout flows directly to the terminal.

IMPORTANT: Any command that adds, removes, moves, or otherwise changes the
filesystem layout the user is working inside MUST be wired into the shell
wrapper's cd-redirect path. The user's cwd may become invalid (worktree deleted,
renamed, moved between layouts), and the binary cannot fix that on its own —
only the parent shell can `cd`. Concretely, when adding such a command:

1. Have the binary write the redirect target to `DAFT_CD_FILE` when set, and
   fall back to a `Run \`cd ...\`` eprintln otherwise (so users without the
   wrapper still get a hint).
2. Add the subcommand verb to the `daft()` wrapper in
   `src/commands/shell_init.rs` — both the bash/zsh `case` and the fish `case` —
   alongside existing entries like `layout|repo`. New separate binaries
   (symlinks like `git-worktree-*`) route through `__daft_wrapper`
   automatically; new daft _subcommands_ do not, and must be added by name.
3. Cover the wrapper integration with a regression test in
   `tests/integration/test_shell_init.sh` that sources the wrapper, runs the
   command, and asserts `builtin pwd` lands in the expected place. The YAML
   scenarios under `tests/manual/scenarios/` only exercise the binary directly —
   they cannot catch a missing wrapper case.

The `daft repo remove` field-test bug (binary wrote DAFT_CD_FILE correctly but
the wrapper had no `repo)` case) is the canonical example.

**Shell-eval'd commands are on the hot path**: `daft shell-init` and
`daft completions <shell>` both emit shell code that users `eval` from their rc
files (`~/.bashrc`, `~/.zshrc`, etc.), so they run on every interactive shell
startup. Their codepaths must remain extremely lean: no extra subprocess calls,
file IO, network requests, or background-process spawns. Stderr output is also
problematic — `eval` only captures stdout, so any stderr (e.g., the update
banner) leaks straight into the user's terminal. The same applies to the
`__complete` tab-completion helper, which fires on every Tab keypress. Any
startup-time background work in `src/main.rs` (currently the update check and
trust prune) must be gated through `daft::skip_startup_tasks_for`, which covers
`shell-init`, `completions`, and `__*` background tasks. Add new commands with
similar constraints to that helper rather than introducing a parallel gate.

**Background processes (spawn pattern, not fork)**: Long-running daemon-like
work uses the spawn-self pattern, not `fork()`. The coordinator
(`src/coordinator/process.rs::spawn_coordinator`), `__check-update`,
`__prune-trust`, and `__clean-logs` all share this contract:

1. Parent serializes state to a 0600-perms tempfile via `tempfile::Builder` when
   state passing is needed (trivial spawns skip this).
2. Parent calls `std::env::current_exe()?.canonicalize()?` — the
   `canonicalize()` is **load-bearing**. Without it, when the parent was invoked
   via a symlink (e.g. `git-worktree-checkout-branch`), `current_exe` returns
   the symlink path. Spawning that path means the new daft process dispatches
   through the symlink's command arm (here: checkout-branch), and clap then
   rejects `__<name> <state-file>` as positional args — the spawned child
   silently fails to start. Canonicalize lands on the real `daft` binary so the
   multicall arm dispatches correctly.
3. Parent spawns `daft __<name> [<state-file>]` via `Command::new(canonical)`
   with `Stdio::null()` for stdin/stdout/stderr and any required env vars set
   via `Command::env(…)` — NOT `std::env::set_var` in the parent (that's
   `unsafe fn` in edition 2024).
4. Child reads + unlinks state on entry (best-effort: don't error on missing
   tempfile; rely on bytes already in memory).
5. Child calls `nix::unistd::setsid()` early to detach from the parent's
   session/TTY.
6. Child runs its work and `process::exit`s.

`fork()` is **not** an option for new background work — `libc::fork()` is
intrinsically `unsafe fn` (POSIX async-signal-safety rules) and conflicts with
the `forbid(unsafe_code)` policy in Critical Rule #4. Spawn-self is the pattern.

When debugging a spawned daemon's startup, `Stdio::null()` on stderr hides
panics. Temporarily replace it with
`Stdio::from(File::open("/tmp/daft-X-debug.log")?)`, run the failing test,
revert before commit. Don't ship the debug redirect.

**Store / Ports / Adapters / Domain pattern (coordinator)**: The coordinator
follows hexagonal architecture so domain logic stays pure and the data layer can
be swapped without rewriting business code.

```
Application       commands/hooks/jobs, commands/dump_store, IPC dispatch
   │
   ▼ depends on
Domain            coordinator/domain/  (pure logic, no SQL, no syscalls)
   │
   ▼ talks through traits in
Ports             coordinator/ports/{store, clock, process}
   ▲
   │ implemented by
Adapters          coordinator/adapters/{sqlite_store, system_clock, unix_process}
   │
   ▼ uses
Store             src/store/{connection, pool, migrate, models, repos, env_scrub}
```

Hard rules:

- **Domain never imports `rusqlite`, `nix`, or `std::env`**. It talks to the
  outside world through ports only. This is what makes the layer unit-testable
  without spawning processes or touching disk.
- **Store never imports domain types**. Returns its own typed row structs
  (`JobRow`, `InvocationRow`, `RepoPolicyRow`). Whatever shape consumers want on
  top — `JobMeta`, `JobView`, etc. — lives in the adapter or above.
- **Adapters are the only translation layer**. They wrap repos + apply
  cross-cutting concerns (env scrub, status enum ↔ TEXT conversion, policy merge
  semantics). New cross-cutting concerns go here, not in the store.

Canonical examples to study before adding a new feature:
`coordinator/domain/reconcile.rs` (pure logic, six fake-adapter tests),
`coordinator/adapters/sqlite_store.rs` (port impl + env scrub at the persistence
boundary), `coordinator/ports/store.rs` (trait surface).

**YAML test runner port (`xtask/src/manual_test/`)**: One port —
`CommandExecutor` in `executor.rs`, daft adapter in `daft_executor.rs` — between
the runner core (`runner.rs`, `sandbox.rs`, `executor.rs`) and daft-specific
command execution. Future #509 sub-tasks plug in as adapter changes, not
runner-core changes.

Hard rules:

- **Runner core never names daft.** Greppable check:
  `rg 'daft::|DAFT_' xtask/src/manual_test/{runner,sandbox,executor}.rs` returns
  zero matches outside docstrings. If a change can't be done as an adapter
  change, widen the port deliberately — don't leak.
- **One port, not several.** Sandbox provisioning, fixture handling, and step
  evaluation are candidates for future ports. Extracting them speculatively is
  the trap; wait for a downstream PR with a concrete reason.
- **Intentional exception:** `DAFT_MANUAL_TEST_BASE`,
  `DAFT_MANUAL_TEST_EMIT_TIMING`, `DAFT_MANUAL_TEST_JOBS`, and
  `DAFT_MANUAL_TEST_DEBUG_DISPATCH` are the runner's own config knobs and live
  in `mod.rs` / `main.rs`. Renaming the namespace is a spin-out concern, not a
  coupling concern.

**Design language**: `xtask/src/manual_test/reporter/CLAUDE.md` constrains the
output's appearance (color budget, hierarchy, iconography, microcopy, rhythm).
Travels with the runner spin-out — do not inline its rules here.

**Hooks system**: Lifecycle hooks in `.daft/hooks/` with trust-based security.
Hook types: `post-clone`, `worktree-pre-create`, `worktree-post-create`,
`worktree-pre-remove`, `worktree-post-remove`. Old names without `worktree-`
prefix are deprecated (removed in v2.0.0).

**TUI navigation**: All interactive TUI interfaces that support arrow key
navigation must also support Vim-style `hjkl` keys.

## Branch Naming & PRs

- Branch names: `daft-<issue number>/<shortened issue name>`
- PRs target `master` and are always **squash merged** (linear history required)
- PR titles use conventional commit format: `feat: add dark mode toggle`
- Issue references go in PR body, not title: `Fixes #42`

### PR Tagging

Every PR must be tagged with:

- **Assignee**: repository owner (`avihut`)
- **Label**: matches the conventional commit type (`feat`, `fix`, `docs`,
  `refactor`, `style`, `perf`, `test`, `chore`, `ci`)
- **Milestone**: `Public Launch` (current active milestone)

### Labels

Labels live on orthogonal axes — keep them from blurring:

- **Kind** (issues): `bug`, `enhancement`, `documentation` — what the issue
  _is_.
- **Type** (PRs, conventional): `feat`, `fix`, `docs`, `perf`, `refactor`,
  `chore`, `ci`, `style`, `test` — what the change _does_ (matches the commit
  type).
- **Area** (`area:*`): the code / conflict _zone_ a change touches —
  `area:worktree`, `area:hooks`, `area:store`, `area:coordinator`, `area:git`,
  `area:layout`, `area:config`, `area:commands`, `area:completions`,
  `area:output`, `area:docs`, `area:ci`, `area:test-runner`, `area:term-styles`
  (canonical list + globs: `.github/labeler.yml`). The `area:` prefix is
  load-bearing — it keeps zones a distinct axis and avoids colliding with the
  like-named `hooks`/`docs`/`ci` labels on other axes.
- **Topic / triage**: `security`, `audit`, `dependencies`, `release`;
  `high-priority`, `good first issue`, `help wanted`.

**`area:*` is the parallelization + conflict signal.** PRs are auto-labeled by
zone from their diff (`.github/labeler.yml`); label _issues_ by their
**predicted** zone at triage. A PR carrying more zones than its issue predicted
flags scope creep; two open PRs sharing a zone flag a likely rebase; the same
zone on two in-progress items means serialize them.

**Drift rule (same spirit as Shell Completions below):** `.github/labeler.yml`
is the _only_ place the glob → zone map lives. When you add a top-level `src/`
module or a new workspace crate, add its glob there and create the matching
`area:*` label. Never duplicate the glob list into this file or the docs — they
describe the scheme and point at `labeler.yml`.

## Commits

[Conventional Commits](https://www.conventionalcommits.org/) format:
`<type>[scope]: <description>`

Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `chore`, `ci`

## Release Process

Uses [cargo-release](https://github.com/crate-ci/cargo-release) wired into
`.github/workflows/release-flow.yml` to mimic the release-plz UX it replaced
(see #540 for the migration motivation: release-plz's `cargo package` codepath
was broken against our root-package workspace with an unpublished internal
crate, upstream release-plz#2595).

Mechanics: every push to master runs two jobs.

1. `tag-merged-release` — if HEAD's subject is `chore: release vX.Y.Z` (i.e.
   someone just merged the release PR), tag the commit and push the tag.
   `release.yml` (cargo-dist) then builds the binary release.
2. `maintain-release-pr` — compute the next version from conventional commits
   via `git-cliff --bumped-version`. If releasable commits exist since the last
   tag, force-rebuild a `release-pr` branch on top of master via
   `cargo release <next> --no-tag --no-push` (bumps Cargo.toml + xtask path-dep,
   runs the `pre-release-hook` in `release.toml` to regenerate CHANGELOG.md +
   man pages + CLI docs, commits `chore: release vNEXT`), and open/update the
   release PR. If no releasable commits remain, close any stale release PR.

Conventional-commit-driven bump (per `cliff.toml`): `feat` → minor, `fix`/`perf`
→ patch, `feat!`/`BREAKING CHANGE:` → major, `ci`/`deps`/`docs` → no bump.

Manual version override: `workflow_dispatch` exposes an `override_version` input
(Actions → Release flow → Run workflow → fill `1.14.3`) for one-off overrides —
the input is per-run and the next push to master returns to auto-compute. Useful
when conventional-commit detection picks a different bump than you want (e.g.
demote a `feat:` from minor to patch).

## Adding a New Command

1. Create `src/commands/<name>.rs` with clap `Args` struct (include `about`,
   `long_about`, `arg(help)` attributes for man pages)
2. Add module to `src/commands/mod.rs`
3. Add routing in `src/main.rs`
4. Add to `COMMANDS` array and `get_command_for_name()` in `xtask/src/main.rs`
5. Add to help output in `src/commands/docs.rs` (`get_command_categories()`)
6. Run `mise run man:gen` and commit the generated man page
7. Add YAML test scenarios in `tests/manual/scenarios/<name>/` (see
   `tests/README.md` for schema reference)
8. Add bash integration tests in `tests/integration/` following existing
   patterns
9. **If the command can change the layout the user's cwd lives inside** (creates
   / removes / moves / renames worktrees or repos), wire it into the shell
   wrapper: write `DAFT_CD_FILE` from the binary AND add the verb to the
   `daft()` wrapper in `src/commands/shell_init.rs` (both bash/zsh and fish).
   See the Shell integration section for the full contract.

## Adding a New DB-backed Feature

Every persistent-state feature lands through the same store / ports / adapters
spine. Follow the same order so the layering stays clean and the security
defaults (PRAGMAs, perms, env scrub) come for free.

1. **Schema first.** Add a `.sql` file in `src/store/migrations/` with the next
   sequence number, e.g. `002_trust_registry.sql`. Never edit a shipped
   migration in place; only append. Add a unit test in `store::migrate::tests`
   that asserts the new tables exist after `to_latest`.
2. **Typed row model.** Add a struct in `src/store/models/<thing>.rs` — one Rust
   field per SQL column, no JSON blobs.
3. **Repo with parameterized queries.** Add a struct in
   `src/store/repos/<things>.rs` exposing methods that take
   `&rusqlite::Connection`. All SQL must be parameterized via `params![...]` — a
   CI grep-gate disallows `format!` inside `src/store/repos/` so the rule can't
   drift.
4. **Port trait** (if the feature has business logic). Declare it in the
   consuming subsystem's `ports/` directory — for the coordinator that's
   `src/coordinator/ports/`. Surface store row models (or domain types if
   appropriate), never raw `Connection`s.
5. **Adapter.** Bridge the port to the repos. This is where cross-cutting
   concerns belong (env scrub on persistence, RepoPolicy ↔ RepoPolicyRow
   conversion, …).
6. **Wire the adapter.** Either inject it into the consuming function directly
   (`process.rs` style) or construct it at the application boundary (CLI command
   handler, IPC dispatch).
7. **Unit-test the domain logic with mock adapters** that implement the port
   traits in tests. `coordinator/domain/reconcile.rs` is the canonical example —
   six tests against fake `Store + Process + Clock`.
8. **The security defaults are non-bypassable** — every consumer goes through
   `src/store/connection.rs::bring_up`, which enforces the PRAGMA set, refuses
   foreign `application_id` files, and tightens file/dir perms to 0600/0700. Do
   not duplicate or weaken those checks.

## Shell Completions

Shell completions live in `src/commands/completions/`. When adding, removing, or
renaming commands, verbs, or arguments, update **all** of these:

- `mod.rs` — `COMMANDS`, `VERB_ALIAS_GROUPS`, `get_command_for_name()`
- `bash.rs` — `DAFT_BASH_COMPLETIONS` (verb alias cases, top-level subcommand
  list)
- `zsh.rs` — `DAFT_ZSH_COMPLETIONS` (verb alias cases, top-level subcommand
  list)
- `fish.rs` — `DAFT_FISH_COMPLETIONS` (subcommand registrations, branch
  completion triggers), verb alias flag comment
- `fig.rs` — Fig/Amazon Q spec generation

Flag completions for `git-worktree-*` commands are auto-generated from clap
`Args` structs, but the **hardcoded string constants** in bash/zsh/fish contain
verb names and subcommand lists that must be updated manually.

## Man Pages

Pre-generated in `man/` and committed. Regenerate after changing command help
text:

```bash
mise run man:gen      # Generate/update man pages
mise run man:verify   # Check if man pages are up-to-date (also runs in CI)
```

## Test Plans

Manual test plans live in `test-plans/`. Each file is a markdown checklist tied
to a branch via YAML frontmatter:

```markdown
---
branch: feat/progressive-adoption
---

# Progressive Adoption

## Layout resolution

- [ ] Default layout is sibling when no config exists
- [ ] CLI --layout flag overrides config
```

- **File name**: descriptive feature name, not the branch name
  (`progressive-adoption.md`, not `feat-progressive-adoption.md`)
- **`branch:` frontmatter**: must match the full branch name — used by the
  sandbox `test-plan` command to auto-resolve the plan for the current worktree
- **Committed to the repo**: serves as documentation of what was manually tested
- In the sandbox: `test-plan` opens the current branch's plan in treemd,
  `test-plan <name>` opens a specific plan by filename

## Documentation Site

`docs/` contains the project documentation (VitePress/Markdown). Update when
adding or changing user-facing features.

- `docs/getting-started/` — installation, quick start, shell integration
- `docs/worktrees/` — Worktrees pillar (Overview + How-tos + per-pillar
  Reference)
- `docs/hooks/` — Hooks pillar (Overview + How-tos + per-pillar Reference)
- `docs/recipes/` — Cookbook (patterns, walkthroughs, adoption recipes,
  references); see `.claude/skills/writing-recipes/SKILL.md` for shape rules
- `docs/reference/` — CLI ref, configuration, output formats, agent skill
  (consolidated); follow `docs/reference/cli/daft-doctor.md` as a CLI-page
  template
- `docs/about/` — meta (why-daft, glossary, FAQ, troubleshooting, comparison,
  contributing, changelog)
- Every page needs `title` and `description` YAML frontmatter
- No emoji in docs
- **Update `SKILL.md`** when changes affect how an agent should interact with
  daft — new or removed commands, changed feature behavior, configuration format
  changes (e.g., hooks moving from shell scripts to YAML), renamed hook types,
  new template variables, etc. The skill is what teaches AI coding agents to use
  daft correctly.
- **Update `.claude/skills/writing-recipes/SKILL.md`** when making structural
  changes to how recipes are written — new sections, new conventions, changed
  shape requirements, new style rules. The skill is what guides future recipe
  authors (human or agent), and divergence between the skill and the recipes is
  the silent way the conventions rot.

### Docs Site (VitePress)

The docs site at `daft.avihu.dev` is built from `docs/` using VitePress + Biome,
with Bun as the package manager.

```bash
mise run docs:site          # Dev server at localhost:5173
mise run docs:site:build    # Build the site
mise run docs:site:preview  # Preview built site
mise run docs:site:check    # Lint config with Biome
mise run docs:site:format   # Auto-fix config with Biome
```

- **Prettier** (root): `*.{md,yml,yaml}` files everywhere
- **Biome** (docs): `docs/.vitepress/config.ts` and `docs/.vitepress/theme/`
  only
- Auto-deploys to Cloudflare Pages on push to `master` when `docs/**` changes
- **Playwright screenshots**: Save to `.playwright-mcp/` directory (gitignored),
  not the project root. Use filename like `.playwright-mcp/screenshot.png`.
