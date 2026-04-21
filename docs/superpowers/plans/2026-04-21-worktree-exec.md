# daft worktree-exec Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a new top-level `daft worktree-exec` command (alias `daft exec`)
that runs one or more commands against one or more selected worktrees, with
single-target pass-through, multi-target list-mode UI, and hook-style windows in
verbose mode.

**Architecture:** Argument parsing lives in `src/commands/exec.rs` (thin clap
shell, mirrors `carry.rs`). Core logic — target resolution, per-worktree
pipeline runner, scheduler, renderer — lives in `src/core/worktree/exec.rs`.
Capture uses a bounded ring buffer. Parallelism uses `std::thread` (no async
runtime in the project). Signal handling uses the existing `ctrlc` crate.
Completions reuse the `RichCompletionConfig` pattern already used by `carry`,
`fetch`, and `branch`.

**Tech Stack:** Rust 2021, clap 4.5 derive, `std::process::Command`,
`std::thread`, `globset` (already a dependency), `ctrlc` (already a dependency),
`tempfile` + `assert_cmd` (dev).

**Spec:** `docs/superpowers/specs/2026-04-21-worktree-exec-design.md`

---

## Pre-flight

Before starting, read:

- `docs/superpowers/specs/2026-04-21-worktree-exec-design.md` (the approved
  design)
- `src/commands/carry.rs` (entry-point template; every new daft command matches
  this shape)
- `src/core/worktree/prune.rs:52-58` (`WorktreeEntry`) and `:822`
  (`parse_worktree_list`) — the lean worktree-enumeration helper we'll reuse
- `src/exec.rs` (the existing `-x` helper on clone/init/checkout — **do not
  reuse it**; we use different shell semantics)
- `CLAUDE.md` (critical rules: never touch global git config, never test against
  this repo)

Every task ends with a commit. Follow conventional commits: `feat(exec): …`,
`test(exec): …`, `docs(exec): …`, `chore(exec): …`. Before each commit:
`mise run fmt && mise run clippy && mise run test:unit`.

---

## Task 1: Scaffold the command, wire routing, ship a stub

**Files:**

- Create: `src/commands/exec.rs`
- Modify: `src/commands/mod.rs`
- Modify: `src/lib.rs` (the `DAFT_VERBS` array at lines 79-82)
- Modify: `src/main.rs` (add routing)
- Test: `tests/integration/test_worktree_exec.sh` (bootstrap)

- [ ] **Step 1: Write `src/commands/exec.rs` with a stub `run()` that just
      errors.**

```rust
use crate::{
    get_project_root,
    git::{should_show_gitoxide_notice, GitCommand},
    is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
    WorktreeConfig,
};
use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(name = "git-worktree-exec")]
#[command(version = crate::VERSION)]
#[command(about = "Run a command across one or more worktrees")]
#[command(long_about = r#"
Runs one or more commands against one or more selected worktrees without
changing the current directory.

Targets may be given as positional branch or worktree-directory names, or
globs against branch names (e.g. 'feat/*'). Use --all to target every
worktree in the repository. Positionals and --all are mutually exclusive.

Commands are expressed either as a literal argv after --, or as one or
more -x shell strings. The two forms are mutually exclusive. Multiple -x
values run sequentially per worktree; a failure stops that worktree but
does not stop other worktrees.

When a single worktree is targeted, stdio is fully inherited, making
interactive programs (claude, vim, fzf) work the same as if you had cd'd
into the worktree first.
"#)]
#[command(after_help = r#"EXAMPLES:
    Run a single command across all worktrees:
        daft exec --all -- npm test

    Run on specific branches (glob and exact mix):
        daft exec feat/auth 'feat/ui-*' -- cargo build

    Sequential with fail-fast:
        daft exec --all --sequential -- pnpm lint

    Pipeline of commands per worktree:
        daft exec --all -x 'mise install' -x 'pnpm build' -x 'pnpm test'

    Pass-through to an interactive program (single target):
        daft exec feat/auth -- claude

    Live "windows" output (like hooks):
        daft exec --all -v -- cargo test
"#)]
pub struct Args {
    #[arg(
        help = "Target worktree(s) by branch name, directory name, or glob"
    )]
    pub targets: Vec<String>,

    #[arg(
        long = "all",
        conflicts_with = "targets",
        help = "Target every worktree in the repository"
    )]
    pub all: bool,

    #[arg(
        short = 'x',
        long = "exec",
        value_name = "CMD",
        help = "Shell command to run (repeatable); runs via $SHELL -c"
    )]
    pub exec: Vec<String>,

    #[arg(
        long = "sequential",
        conflicts_with = "keep_going",
        help = "Run worktrees one at a time and stop on first failure"
    )]
    pub sequential: bool,

    #[arg(
        long = "keep-going",
        help = "Run worktrees one at a time and continue through failures"
    )]
    pub keep_going: bool,

    #[arg(short, long, help = "Show hook-style live windows instead of the list-mode table")]
    pub verbose: bool,

    /// Trailing command vector after `--`. Mutually exclusive with `-x`.
    #[arg(last = true, value_name = "CMD")]
    pub trailing: Vec<String>,
}

pub fn run() -> Result<()> {
    let _args = Args::parse_from(crate::get_clap_args("git-worktree-exec"));

    init_logging(_args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;
    let config = OutputConfig::new(false, _args.verbose);
    let mut output = CliOutput::new(config);

    let wt_config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: false,
    };
    let _git = GitCommand::new(wt_config.quiet).with_gitoxide(settings.use_gitoxide);
    let _project_root = get_project_root()?;

    if should_show_gitoxide_notice(settings.use_gitoxide) {
        output.warning("[experimental] Using gitoxide backend for git operations");
    }

    anyhow::bail!("daft exec is not yet implemented")
}
```

- [ ] **Step 2: Register the module in `src/commands/mod.rs`.**

Find the existing `pub mod carry;` line and add `pub mod exec;` in alphabetical
order between `pub mod docs;` and `pub mod fetch;`:

```rust
pub mod docs;
pub mod doctor;
pub mod exec;
pub mod fetch;
```

- [ ] **Step 3: Add `"exec"` to `DAFT_VERBS` in `src/lib.rs:79-82`.**

Replace:

```rust
const DAFT_VERBS: &[&str] = &[
    "adopt", "carry", "clone", "eject", "go", "init", "list", "prune", "remove", "rename", "start",
    "sync", "update",
];
```

With:

```rust
const DAFT_VERBS: &[&str] = &[
    "adopt", "carry", "clone", "eject", "exec", "go", "init", "list", "prune", "remove", "rename",
    "start", "sync", "update",
];
```

- [ ] **Step 4: Wire routing in `src/main.rs`.**

Three edits in `src/main.rs`:

**4a.** In the symlink-dispatch match (around line 82, after
`"git-worktree-sync" => commands::sync::run(),`), add:

```rust
        "git-worktree-exec" => commands::exec::run(),
```

**4b.** In the daft verb alias block (around line 141, after
`"eject" => commands::flow_eject::run(),`), add:

```rust
                    "exec" => commands::exec::run(),
```

**4c.** In the `daft worktree-<command>` block (around line 155, after
`"worktree-sync" => commands::sync::run(),`), add:

```rust
                    "worktree-exec" => commands::exec::run(),
```

- [ ] **Step 5: Verify it builds and the stub errors usefully.**

Run: `cargo build` then `cargo run -- exec --help` Expected: help text
containing "Run a command across one or more worktrees" and the EXAMPLES
section.

Run: `cargo run -- exec --all -- true` Expected:
`Error: Not inside a Git repository` (when run outside a repo) or
`Error: daft exec is not yet implemented` (inside one).

- [ ] **Step 6: Commit.**

```bash
git add src/commands/exec.rs src/commands/mod.rs src/lib.rs src/main.rs
git commit -m "feat(exec): scaffold worktree-exec command with CLI surface"
```

---

## Task 2: Unit tests for argument validation (TDD the mutual exclusions)

**Files:**

- Modify: `src/commands/exec.rs` (add `#[cfg(test)] mod tests` at bottom)

- [ ] **Step 1: Write failing tests for all mutual exclusions.**

Append to `src/commands/exec.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(argv: &[&str]) -> Result<Args, clap::Error> {
        let mut full = vec!["git-worktree-exec"];
        full.extend_from_slice(argv);
        Args::try_parse_from(full)
    }

    #[test]
    fn parses_argv_after_double_dash() {
        let args = parse(&["--all", "--", "cargo", "test"]).unwrap();
        assert!(args.all);
        assert_eq!(args.trailing, vec!["cargo", "test"]);
        assert!(args.exec.is_empty());
    }

    #[test]
    fn parses_repeated_dash_x() {
        let args = parse(&["feat/a", "-x", "mise install", "-x", "pnpm test"]).unwrap();
        assert_eq!(args.targets, vec!["feat/a"]);
        assert_eq!(args.exec, vec!["mise install", "pnpm test"]);
    }

    #[test]
    fn positionals_conflict_with_all() {
        let err = parse(&["feat/a", "--all", "--", "echo"]).unwrap_err();
        assert!(err.to_string().contains("cannot be used with"), "{err}");
    }

    #[test]
    fn sequential_conflicts_with_keep_going() {
        let err = parse(&["--all", "--sequential", "--keep-going", "--", "echo"]).unwrap_err();
        assert!(err.to_string().contains("cannot be used with"), "{err}");
    }

    #[test]
    fn accepts_glob_positionals() {
        let args = parse(&["feat/*", "fix/crash", "--", "echo"]).unwrap();
        assert_eq!(args.targets, vec!["feat/*", "fix/crash"]);
    }
}
```

- [ ] **Step 2: Run tests, verify they pass.**

Run: `cargo test --lib commands::exec::tests` Expected: all five tests pass.
(The mutual exclusions are enforced by the `conflicts_with` attributes already
declared in Task 1.)

- [ ] **Step 3: Commit.**

```bash
git add src/commands/exec.rs
git commit -m "test(exec): argument-parsing and mutual-exclusion tests"
```

---

## Task 3: Additional validation — "need at least one target" and "need at least one command"

clap's `conflicts_with` covers pairs; it does not cover "at least one of A or B
required." We enforce those in `run()`.

**Files:**

- Modify: `src/commands/exec.rs`

- [ ] **Step 1: Write failing tests.**

Append to the `tests` module:

```rust
    fn validate(args: &Args) -> anyhow::Result<()> {
        super::validate_args(args)
    }

    #[test]
    fn rejects_empty_targets_and_no_all() {
        let args = parse(&["--", "echo"]).unwrap();
        let err = validate(&args).unwrap_err();
        assert!(
            err.to_string().contains("at least one target"),
            "got: {err}"
        );
    }

    #[test]
    fn rejects_empty_command_forms() {
        let args = parse(&["--all"]).unwrap();
        let err = validate(&args).unwrap_err();
        assert!(
            err.to_string().contains("-x") || err.to_string().contains("--"),
            "got: {err}"
        );
    }

    #[test]
    fn rejects_both_command_forms() {
        let args = parse(&["--all", "-x", "echo", "--", "echo"]).unwrap();
        let err = validate(&args).unwrap_err();
        assert!(
            err.to_string().contains("cannot be combined"),
            "got: {err}"
        );
    }

    #[test]
    fn accepts_minimal_valid_argv_form() {
        let args = parse(&["--all", "--", "echo"]).unwrap();
        validate(&args).unwrap();
    }

    #[test]
    fn accepts_minimal_valid_x_form() {
        let args = parse(&["--all", "-x", "echo"]).unwrap();
        validate(&args).unwrap();
    }
```

- [ ] **Step 2: Run tests, verify they fail with "function not defined"
      errors.**

Run: `cargo test --lib commands::exec::tests` Expected: compile errors —
`validate_args` does not exist yet.

- [ ] **Step 3: Implement `validate_args` in `src/commands/exec.rs`.**

Add above the `pub fn run()`:

```rust
pub(crate) fn validate_args(args: &Args) -> anyhow::Result<()> {
    if args.targets.is_empty() && !args.all {
        anyhow::bail!(
            "at least one target or --all is required (use `daft exec --help` for examples)"
        );
    }
    if args.exec.is_empty() && args.trailing.is_empty() {
        anyhow::bail!(
            "no command given: pass `-x 'CMD'` one or more times, or `-- CMD ARGS…`"
        );
    }
    if !args.exec.is_empty() && !args.trailing.is_empty() {
        anyhow::bail!("`-x` and `-- CMD` cannot be combined in one invocation");
    }
    Ok(())
}
```

And wire it into `run()` right after the `Args::parse_from` call:

```rust
    let _args = Args::parse_from(crate::get_clap_args("git-worktree-exec"));
    validate_args(&_args)?;
```

- [ ] **Step 4: Run tests, verify they pass.**

Run: `cargo test --lib commands::exec::tests` Expected: all eight tests in the
module pass.

- [ ] **Step 5: Commit.**

```bash
git add src/commands/exec.rs
git commit -m "feat(exec): validate target and command-form requirements"
```

---

## Task 4: Core module scaffolding — `src/core/worktree/exec.rs` types

**Files:**

- Create: `src/core/worktree/exec.rs`
- Modify: `src/core/worktree/mod.rs`

- [ ] **Step 1: Create `src/core/worktree/exec.rs` with the core types.**

```rust
//! Core logic for `daft worktree-exec`.
//!
//! Target resolution, per-worktree command pipeline, scheduler, and the
//! `ExecReport` data type that the command layer renders. No IO to stdout
//! lives here; renderers are separate.

use std::path::PathBuf;
use std::time::Duration;

/// A worktree that has been matched by the user's selectors and is ready
/// to receive commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub worktree_path: PathBuf,
    pub branch_name: String,
}

/// One command in a per-worktree pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandSpec {
    /// Direct argv exec, e.g. from `-- CMD ARGS…`. First element is the
    /// program; remaining are its arguments. Never touches a shell.
    Argv(Vec<String>),
    /// Shell-parsed string, e.g. from `-x 'CMD'`. Run via `$SHELL -c`,
    /// falling back to `sh -c` if `$SHELL` is unset.
    Shell(String),
}

impl CommandSpec {
    /// A short one-line representation of the command for UI display.
    pub fn display(&self) -> String {
        match self {
            CommandSpec::Argv(parts) => parts.join(" "),
            CommandSpec::Shell(s) => s.clone(),
        }
    }
}

/// How the scheduler fans work out across worktrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecMode {
    /// All worktrees concurrently. Default.
    Parallel,
    /// One worktree at a time; stop on the first failing worktree.
    Sequential,
    /// One worktree at a time; continue through failures.
    KeepGoing,
}

/// Outcome of running the full pipeline on one worktree.
#[derive(Debug)]
pub struct WorktreeOutcome {
    pub target: ResolvedTarget,
    /// Index into the pipeline of the last command attempted (0-based). If
    /// the first command failed, this is 0. If all succeeded, this equals
    /// pipeline.len() - 1.
    pub last_command_index: usize,
    /// Exit code of the last command attempted. 0 when all succeeded.
    pub exit_code: i32,
    /// Wall-clock duration from spawn of first command to finish of last.
    pub elapsed: Duration,
    /// Captured stdout+stderr, truncated at `OUTPUT_CAP_BYTES` keeping the tail.
    pub captured_output: Vec<u8>,
    /// Whether the worktree was cancelled by SIGINT before finishing.
    pub cancelled: bool,
}

impl WorktreeOutcome {
    pub fn succeeded(&self) -> bool {
        self.exit_code == 0 && !self.cancelled
    }
}

/// The aggregate result of an exec invocation used by renderers and the
/// outer command layer.
#[derive(Debug, Default)]
pub struct ExecReport {
    pub outcomes: Vec<WorktreeOutcome>,
    pub orphan_branches_skipped: Vec<String>,
}

impl ExecReport {
    /// 0 if every worktree succeeded, 1 otherwise. Single-target
    /// pass-through never builds an `ExecReport`; it propagates the child
    /// exit code directly.
    pub fn aggregate_exit_code(&self) -> i32 {
        if self.outcomes.iter().all(|o| o.succeeded()) {
            0
        } else {
            1
        }
    }
}

/// Output-capture cap per worktree (ring buffer keeps the tail). Internal
/// constant — not user-configurable in v1.
pub const OUTPUT_CAP_BYTES: usize = 1 * 1024 * 1024;
```

- [ ] **Step 2: Expose the module in `src/core/worktree/mod.rs`.**

Insert `pub mod exec;` in alphabetical order (between `pub mod clone;` and
`pub mod fetch;`):

```rust
pub mod clone;
pub mod exec;
pub mod fetch;
```

- [ ] **Step 3: Verify it builds.**

Run: `cargo build` Expected: clean build (the new module is unused but
compiles).

- [ ] **Step 4: Commit.**

```bash
git add src/core/worktree/exec.rs src/core/worktree/mod.rs
git commit -m "feat(exec): core types (ResolvedTarget, CommandSpec, ExecMode, ExecReport)"
```

---

## Task 5: Ring buffer for bounded output capture

**Files:**

- Modify: `src/core/worktree/exec.rs`

A ring buffer that keeps the last N bytes written. Writer is `std::io::Write` so
it drops into `Command::stdout(Stdio::piped())` read-loops without fuss.

- [ ] **Step 1: Write failing unit tests.**

Append to `src/core/worktree/exec.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_everything_under_cap() {
        let mut r = TailBuffer::new(16);
        r.extend(b"hello world");
        assert_eq!(r.tail(), b"hello world");
    }

    #[test]
    fn ring_keeps_only_tail_over_cap() {
        let mut r = TailBuffer::new(4);
        r.extend(b"abcdefghij");
        assert_eq!(r.tail(), b"ghij");
    }

    #[test]
    fn ring_exact_cap_boundary() {
        let mut r = TailBuffer::new(4);
        r.extend(b"abcd");
        assert_eq!(r.tail(), b"abcd");
        r.extend(b"e");
        assert_eq!(r.tail(), b"bcde");
    }

    #[test]
    fn ring_multi_extend_accumulates_tail() {
        let mut r = TailBuffer::new(5);
        r.extend(b"aaa");
        r.extend(b"bbb");
        r.extend(b"ccc");
        assert_eq!(r.tail(), b"bbccc");
    }
}
```

- [ ] **Step 2: Run tests — expect "TailBuffer not found".**

Run: `cargo test --lib core::worktree::exec::tests` Expected: compile errors.

- [ ] **Step 3: Implement `TailBuffer`.**

Add below the constants in `src/core/worktree/exec.rs`:

```rust
/// Byte tail-buffer: writes are appended; when total exceeds `cap`, the
/// oldest bytes are dropped so that only the last `cap` bytes remain.
///
/// Does not attempt to preserve UTF-8 boundaries. Callers that need string
/// output should `String::from_utf8_lossy(buf.tail())`.
pub struct TailBuffer {
    buf: Vec<u8>,
    cap: usize,
}

impl TailBuffer {
    pub fn new(cap: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap.min(64 * 1024)),
            cap,
        }
    }

    pub fn extend(&mut self, bytes: &[u8]) {
        if bytes.len() >= self.cap {
            // New chunk alone fills (or overfills) the cap.
            let start = bytes.len() - self.cap;
            self.buf.clear();
            self.buf.extend_from_slice(&bytes[start..]);
            return;
        }
        self.buf.extend_from_slice(bytes);
        if self.buf.len() > self.cap {
            let drop = self.buf.len() - self.cap;
            self.buf.drain(..drop);
        }
    }

    pub fn tail(&self) -> &[u8] {
        &self.buf
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.buf
    }
}
```

- [ ] **Step 4: Run tests.**

Run: `cargo test --lib core::worktree::exec::tests` Expected: all four
TailBuffer tests pass.

- [ ] **Step 5: Commit.**

```bash
git add src/core/worktree/exec.rs
git commit -m "feat(exec): TailBuffer for bounded per-worktree output capture"
```

---

## Task 6: Target resolution — exact matches

The resolver takes positionals + `--all` + the worktree list and returns an
ordered, de-duplicated `Vec<ResolvedTarget>`. Exact branch name takes precedence
over directory name (matches `daft worktree-carry` behavior).

**Files:**

- Modify: `src/core/worktree/exec.rs`

- [ ] **Step 1: Write failing tests.**

Append to the `tests` module:

```rust
    use std::path::PathBuf;

    /// A lean snapshot of a worktree for resolver tests. Produced from
    /// `parse_worktree_list` in production; built by hand in tests.
    #[derive(Clone, Debug)]
    pub struct WorktreeSnapshot {
        pub path: PathBuf,
        pub branch: Option<String>,
    }

    fn snap(path: &str, branch: &str) -> WorktreeSnapshot {
        WorktreeSnapshot {
            path: PathBuf::from(path),
            branch: Some(branch.to_string()),
        }
    }

    fn all_worktrees() -> Vec<WorktreeSnapshot> {
        vec![
            snap("/r/master", "master"),
            snap("/r/feat-a", "feat/a"),
            snap("/r/feat-b", "feat/b"),
            snap("/r/fix-crash", "fix/crash"),
        ]
    }

    #[test]
    fn exact_branch_name_resolves() {
        let wts = all_worktrees();
        let got = resolve_targets(&["feat/a".into()], false, &wts).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].branch_name, "feat/a");
        assert_eq!(got[0].worktree_path, PathBuf::from("/r/feat-a"));
    }

    #[test]
    fn exact_dir_name_resolves_when_branch_miss() {
        let wts = all_worktrees();
        let got = resolve_targets(&["feat-b".into()], false, &wts).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].branch_name, "feat/b");
    }

    #[test]
    fn branch_takes_precedence_over_dir_name_collision() {
        // A worktree whose dir name collides with another worktree's branch.
        let wts = vec![
            snap("/r/x",   "branch-x"), // dir name "x", branch "branch-x"
            snap("/r/y",   "x"),        // branch "x", dir name "y"
        ];
        let got = resolve_targets(&["x".into()], false, &wts).unwrap();
        assert_eq!(got[0].branch_name, "x");
        assert_eq!(got[0].worktree_path, PathBuf::from("/r/y"));
    }

    #[test]
    fn dedupe_by_path() {
        let wts = all_worktrees();
        let got = resolve_targets(&["feat/a".into(), "feat-a".into()], false, &wts).unwrap();
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn preserves_user_order() {
        let wts = all_worktrees();
        let got = resolve_targets(
            &["feat/b".into(), "feat/a".into()],
            false,
            &wts,
        )
        .unwrap();
        assert_eq!(got[0].branch_name, "feat/b");
        assert_eq!(got[1].branch_name, "feat/a");
    }
```

- [ ] **Step 2: Run tests — expect compile errors.**

Run: `cargo test --lib core::worktree::exec::tests::exact` Expected:
`WorktreeSnapshot` / `resolve_targets` not found.

- [ ] **Step 3: Implement `resolve_targets` (exact-match only for now; globs in
      Task 7).**

Below the `TailBuffer` impl in `src/core/worktree/exec.rs`, add:

```rust
/// Minimal worktree view that `resolve_targets` needs. In production we
/// build this from `crate::core::worktree::prune::parse_worktree_list`,
/// which already gives us `(path, branch)`.
#[derive(Clone, Debug)]
pub struct WorktreeSnapshot {
    pub path: std::path::PathBuf,
    pub branch: Option<String>,
}

/// Errors surfaced during target resolution. Kept as a plain enum so
/// callers render friendly messages rather than parsing anyhow strings.
#[derive(Debug)]
pub enum ResolveError {
    NoTargets,
    AllWithPositionals,
    Unmatched(Vec<String>),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::NoTargets => {
                write!(f, "no targets: pass one or more branch/dir names, a glob, or --all")
            }
            ResolveError::AllWithPositionals => {
                write!(f, "--all cannot be combined with positional targets")
            }
            ResolveError::Unmatched(toks) => {
                write!(f, "no worktree matched: {}", toks.join(", "))
            }
        }
    }
}

impl std::error::Error for ResolveError {}

pub fn resolve_targets(
    positionals: &[String],
    all: bool,
    worktrees: &[WorktreeSnapshot],
) -> Result<Vec<ResolvedTarget>, ResolveError> {
    if positionals.is_empty() && !all {
        return Err(ResolveError::NoTargets);
    }
    if !positionals.is_empty() && all {
        return Err(ResolveError::AllWithPositionals);
    }

    if all {
        let mut out: Vec<ResolvedTarget> = worktrees
            .iter()
            .filter_map(|w| w.branch.as_ref().map(|b| ResolvedTarget {
                worktree_path: w.path.clone(),
                branch_name: b.clone(),
            }))
            .collect();
        out.sort_by(|a, b| a.branch_name.cmp(&b.branch_name));
        return Ok(out);
    }

    let mut out: Vec<ResolvedTarget> = Vec::new();
    let mut seen_paths: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    let mut unmatched: Vec<String> = Vec::new();

    for tok in positionals {
        let mut matched = false;

        // 1. Exact branch match.
        for wt in worktrees {
            if let Some(branch) = &wt.branch {
                if branch == tok && seen_paths.insert(wt.path.clone()) {
                    out.push(ResolvedTarget {
                        worktree_path: wt.path.clone(),
                        branch_name: branch.clone(),
                    });
                    matched = true;
                    break;
                }
            }
        }
        if matched {
            continue;
        }

        // 2. Exact directory-name match.
        for wt in worktrees {
            let dir_name = wt
                .path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if dir_name == tok && seen_paths.insert(wt.path.clone()) {
                if let Some(branch) = &wt.branch {
                    out.push(ResolvedTarget {
                        worktree_path: wt.path.clone(),
                        branch_name: branch.clone(),
                    });
                    matched = true;
                    break;
                }
            }
        }

        if !matched {
            unmatched.push(tok.clone());
        }
    }

    if !unmatched.is_empty() {
        return Err(ResolveError::Unmatched(unmatched));
    }

    Ok(out)
}
```

- [ ] **Step 4: Run tests.**

Run: `cargo test --lib core::worktree::exec::tests` Expected: all tests in the
`tests` module pass (both the earlier TailBuffer ones and the new resolver
ones).

- [ ] **Step 5: Commit.**

```bash
git add src/core/worktree/exec.rs
git commit -m "feat(exec): exact-match target resolution"
```

---

## Task 7: Target resolution — globs + orphan-branch tracking

**Files:**

- Modify: `src/core/worktree/exec.rs`

Globs use `globset` (already a dependency). A token is a glob iff it contains
`*`, `?`, or `[`. Glob matching is against branch names only. Glob expansions
are sorted by branch name. If a glob matches one or more branches that have no
worktree, those "orphans" are reported as skipped — but are not counted as
unmatched (so the run proceeds). If a glob matches nothing at all (neither
worktree nor orphan), it is an error.

- [ ] **Step 1: Write failing tests.**

Append to the `tests` module:

```rust
    fn snap_no_wt(branch: &str) -> WorktreeSnapshot {
        // A branch that exists in the repo but has no worktree checked out.
        // For test purposes we represent this as a snapshot whose path is
        // the git-dir — we check presence of branch + "has worktree" via
        // path validity. Prefer a dedicated field in production code; tests
        // use a sentinel path.
        WorktreeSnapshot { path: PathBuf::from("::orphan::"), branch: Some(branch.to_string()) }
    }

    fn all_with_orphans() -> Vec<WorktreeSnapshot> {
        let mut v = all_worktrees();
        v.push(snap_no_wt("feat/orphan-1"));
        v.push(snap_no_wt("feat/orphan-2"));
        v
    }

    fn resolve_report(
        positionals: &[String],
        all: bool,
        worktrees: &[WorktreeSnapshot],
    ) -> (Vec<ResolvedTarget>, Vec<String>) {
        let (tgts, orphans) =
            resolve_targets_with_orphans(positionals, all, worktrees).unwrap();
        (tgts, orphans)
    }

    #[test]
    fn glob_matches_branches_alphabetically() {
        let wts = all_worktrees();
        let (got, orphans) = resolve_report(&["feat/*".into()], false, &wts);
        assert_eq!(
            got.iter().map(|t| t.branch_name.as_str()).collect::<Vec<_>>(),
            vec!["feat/a", "feat/b"]
        );
        assert!(orphans.is_empty());
    }

    #[test]
    fn glob_skips_orphan_branches_but_reports_them() {
        let wts = all_with_orphans();
        let (got, orphans) = resolve_report(&["feat/*".into()], false, &wts);
        let branches: Vec<_> = got.iter().map(|t| t.branch_name.as_str()).collect();
        assert_eq!(branches, vec!["feat/a", "feat/b"]);
        assert_eq!(orphans, vec!["feat/orphan-1", "feat/orphan-2"]);
    }

    #[test]
    fn glob_that_matches_nothing_is_error() {
        let wts = all_worktrees();
        let err = resolve_targets_with_orphans(&["zzz*".into()], false, &wts).unwrap_err();
        assert!(matches!(err, ResolveError::Unmatched(_)));
    }

    #[test]
    fn glob_that_only_matches_orphans_is_error_not_silent_skip() {
        // "feat/orphan-*" only matches branches with no worktree; nothing
        // actionable, so we error rather than run zero commands silently.
        let wts = all_with_orphans();
        let err = resolve_targets_with_orphans(&["feat/orphan-*".into()], false, &wts)
            .unwrap_err();
        assert!(matches!(err, ResolveError::Unmatched(_)));
    }
```

- [ ] **Step 2: Run tests — expect "resolve_targets_with_orphans not found".**

Run: `cargo test --lib core::worktree::exec::tests::glob` Expected: compile
errors.

- [ ] **Step 3: Implement glob-aware resolution.**

The orphan concept requires distinguishing "has a worktree" from "is just a
branch in the repo." Add a helper method on `WorktreeSnapshot` and a new public
function:

```rust
impl WorktreeSnapshot {
    /// True if this snapshot represents a branch that has a real checked-out
    /// worktree. Orphan branches (branch exists in refs but no worktree)
    /// are represented with `path` pointing to an unusable sentinel.
    pub fn has_worktree(&self) -> bool {
        // In production, `parse_worktree_list` only ever returns real
        // worktrees; orphan branches are collected separately and fed in
        // with the sentinel path "::orphan::" from the command-layer
        // helper. Resolve on the sentinel to distinguish the two.
        self.path.to_str() != Some("::orphan::")
    }
}

fn is_glob(tok: &str) -> bool {
    tok.contains('*') || tok.contains('?') || tok.contains('[')
}

/// Like `resolve_targets`, but supports globs and reports orphan
/// branches (branches matched by a glob that have no worktree). Orphans
/// are surfaced so the renderer can print a one-line warning; they do not
/// count as unmatched for error purposes *unless* a glob matched nothing
/// actionable at all.
pub fn resolve_targets_with_orphans(
    positionals: &[String],
    all: bool,
    worktrees: &[WorktreeSnapshot],
) -> Result<(Vec<ResolvedTarget>, Vec<String>), ResolveError> {
    if positionals.is_empty() && !all {
        return Err(ResolveError::NoTargets);
    }
    if !positionals.is_empty() && all {
        return Err(ResolveError::AllWithPositionals);
    }

    if all {
        let mut out: Vec<ResolvedTarget> = worktrees
            .iter()
            .filter(|w| w.has_worktree())
            .filter_map(|w| w.branch.as_ref().map(|b| ResolvedTarget {
                worktree_path: w.path.clone(),
                branch_name: b.clone(),
            }))
            .collect();
        out.sort_by(|a, b| a.branch_name.cmp(&b.branch_name));
        return Ok((out, Vec::new()));
    }

    use globset::{Glob, GlobSetBuilder};
    let mut out: Vec<ResolvedTarget> = Vec::new();
    let mut seen_paths: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    let mut orphans: Vec<String> = Vec::new();
    let mut unmatched: Vec<String> = Vec::new();

    for tok in positionals {
        if is_glob(tok) {
            let glob = match Glob::new(tok) {
                Ok(g) => g,
                Err(_) => {
                    unmatched.push(tok.clone());
                    continue;
                }
            };
            let mut set_builder = GlobSetBuilder::new();
            set_builder.add(glob);
            let set = match set_builder.build() {
                Ok(s) => s,
                Err(_) => {
                    unmatched.push(tok.clone());
                    continue;
                }
            };

            let mut snapshot_branches: Vec<(&WorktreeSnapshot, &String)> = worktrees
                .iter()
                .filter_map(|w| w.branch.as_ref().map(|b| (w, b)))
                .filter(|(_, b)| set.is_match(b))
                .collect();
            snapshot_branches.sort_by(|a, b| a.1.cmp(b.1));

            let mut actionable_this_glob: usize = 0;
            let mut orphans_this_glob: Vec<String> = Vec::new();

            for (wt, branch) in snapshot_branches {
                if !wt.has_worktree() {
                    orphans_this_glob.push(branch.clone());
                } else if seen_paths.insert(wt.path.clone()) {
                    out.push(ResolvedTarget {
                        worktree_path: wt.path.clone(),
                        branch_name: branch.clone(),
                    });
                    actionable_this_glob += 1;
                } else {
                    // Already pulled in by an earlier positional; still
                    // counts as "this glob produced something actionable."
                    actionable_this_glob += 1;
                }
            }

            if actionable_this_glob == 0 {
                // Either matched nothing, or matched only orphans. Both
                // are errors; don't report orphans as "skipped" because
                // the run isn't happening.
                unmatched.push(tok.clone());
            } else {
                orphans.extend(orphans_this_glob);
            }
            continue;
        }

        // Exact fallthrough: reuse the non-glob resolver's logic.
        let sub = resolve_targets(std::slice::from_ref(tok), false, worktrees);
        match sub {
            Ok(exact) => {
                for t in exact {
                    if seen_paths.insert(t.worktree_path.clone()) {
                        out.push(t);
                    }
                }
            }
            Err(ResolveError::Unmatched(ref toks)) => {
                unmatched.extend_from_slice(toks);
            }
            Err(other) => return Err(other),
        }
    }

    if !unmatched.is_empty() {
        return Err(ResolveError::Unmatched(unmatched));
    }

    Ok((out, orphans))
}
```

- [ ] **Step 4: Run tests.**

Run: `cargo test --lib core::worktree::exec::tests` Expected: all prior tests +
the four new glob tests pass.

- [ ] **Step 5: Commit.**

```bash
git add src/core/worktree/exec.rs
git commit -m "feat(exec): glob-aware target resolution with orphan-branch reporting"
```

---

## Task 8: Worktree snapshot adapter — wire up production enumeration

Convert `crate::core::worktree::prune::parse_worktree_list` output into
`Vec<WorktreeSnapshot>`, plus enumerate orphan branches. This is the one piece
bridging production git data to the pure resolver.

**Files:**

- Modify: `src/core/worktree/exec.rs`

- [ ] **Step 1: Write the adapter.**

Append to `src/core/worktree/exec.rs`:

```rust
use crate::git::GitCommand;

/// Build a `Vec<WorktreeSnapshot>` from the repo's current worktree list
/// plus all local branches. Branches without an associated worktree are
/// included as orphan snapshots (sentinel path "::orphan::"), which
/// `resolve_targets_with_orphans` filters during glob expansion.
pub fn collect_snapshot(git: &GitCommand) -> anyhow::Result<Vec<WorktreeSnapshot>> {
    use crate::core::worktree::prune::parse_worktree_list;

    let wt_entries = parse_worktree_list(git)?;
    let mut snaps: Vec<WorktreeSnapshot> = Vec::new();
    let mut branches_with_worktrees: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    for entry in &wt_entries {
        if let Some(branch) = &entry.branch {
            branches_with_worktrees.insert(branch.clone());
            snaps.push(WorktreeSnapshot {
                path: entry.path.clone(),
                branch: Some(branch.clone()),
            });
        }
    }

    // Orphan branches: local branches that have no worktree. These are
    // only included for glob-expansion reporting; they carry the
    // "::orphan::" sentinel so `has_worktree()` returns false.
    let local = git
        .list_local_branches()
        .unwrap_or_default();
    for branch in local {
        if !branches_with_worktrees.contains(&branch) {
            snaps.push(WorktreeSnapshot {
                path: std::path::PathBuf::from("::orphan::"),
                branch: Some(branch),
            });
        }
    }

    Ok(snaps)
}
```

- [ ] **Step 2: Confirm `GitCommand::list_local_branches` exists (or pick the
      closest helper).**

Run:
`grep -n "pub fn list_local_branches\|pub fn local_branches\|branches_local" src/git/*.rs`

Expected: one exact match. If the method is named differently (e.g.
`local_branches`, `list_branches`), edit `collect_snapshot` to use the correct
name. If nothing exists, add a minimal helper to `src/git/refs.rs`:

```rust
pub fn list_local_branches(&self) -> anyhow::Result<Vec<String>> {
    let output = self
        .command()
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
        .output()
        .context("git for-each-ref failed")?;
    let s = String::from_utf8(output.stdout).context("non-utf8 branch list")?;
    Ok(s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
}
```

- [ ] **Step 3: Build + run `cargo test --lib core::worktree::exec`.**

Run: `cargo test --lib core::worktree::exec` Expected: all existing tests still
pass. No new tests for `collect_snapshot` (it's an IO adapter tested via the
integration + YAML suites).

- [ ] **Step 4: Commit.**

```bash
git add src/core/worktree/exec.rs src/git/refs.rs
git commit -m "feat(exec): worktree snapshot adapter over parse_worktree_list"
```

---

## Task 9: Per-worktree pipeline runner — argv form, capture, stop-on-failure

**Files:**

- Modify: `src/core/worktree/exec.rs`

- [ ] **Step 1: Write failing tests that use real processes in a
      `tempfile::TempDir`.**

Append to the `tests` module:

```rust
    use tempfile::TempDir;

    fn dummy_target(dir: &TempDir, branch: &str) -> ResolvedTarget {
        ResolvedTarget {
            worktree_path: dir.path().to_path_buf(),
            branch_name: branch.to_string(),
        }
    }

    #[test]
    fn runs_single_argv_command_captures_output() {
        let dir = TempDir::new().unwrap();
        let target = dummy_target(&dir, "master");
        let spec = CommandSpec::Argv(vec!["echo".into(), "hi".into()]);
        let outcome = run_pipeline(&target, &[spec]).unwrap();
        assert_eq!(outcome.exit_code, 0);
        assert!(
            String::from_utf8_lossy(&outcome.captured_output).contains("hi"),
            "captured: {:?}",
            outcome.captured_output
        );
    }

    #[test]
    fn stops_pipeline_on_first_failure() {
        let dir = TempDir::new().unwrap();
        let target = dummy_target(&dir, "master");
        let pipeline = vec![
            CommandSpec::Argv(vec!["false".into()]),
            CommandSpec::Argv(vec!["echo".into(), "should-not-run".into()]),
        ];
        let outcome = run_pipeline(&target, &pipeline).unwrap();
        assert_ne!(outcome.exit_code, 0);
        assert_eq!(outcome.last_command_index, 0);
        assert!(
            !String::from_utf8_lossy(&outcome.captured_output).contains("should-not-run"),
            "second command must not run after the first failed"
        );
    }

    #[test]
    fn cwd_is_worktree_path() {
        let dir = TempDir::new().unwrap();
        let target = dummy_target(&dir, "master");
        let spec = CommandSpec::Argv(vec!["pwd".into()]);
        let outcome = run_pipeline(&target, &[spec]).unwrap();
        let out = String::from_utf8_lossy(&outcome.captured_output);
        let expected = dir.path().canonicalize().unwrap();
        let got = std::path::PathBuf::from(out.trim()).canonicalize().unwrap();
        assert_eq!(got, expected);
    }

    #[test]
    fn injects_env_vars() {
        let dir = TempDir::new().unwrap();
        let target = dummy_target(&dir, "feat/abc");
        let spec = CommandSpec::Argv(vec![
            "sh".into(),
            "-c".into(),
            "printf %s \"$DAFT_BRANCH_NAME\"".into(),
        ]);
        let outcome = run_pipeline(&target, &[spec]).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&outcome.captured_output).trim(),
            "feat/abc"
        );
    }
```

- [ ] **Step 2: Run tests — expect `run_pipeline` not found.**

Run: `cargo test --lib core::worktree::exec::tests::runs` Expected: compile
errors.

- [ ] **Step 3: Implement `run_pipeline`.**

Add to `src/core/worktree/exec.rs` (above `#[cfg(test)]`):

```rust
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Instant;

/// Run the pipeline against a single worktree with stdout+stderr captured
/// into a tail buffer. Stops on first failing command. Does not render any
/// UI — pure function returning a `WorktreeOutcome`.
///
/// `cancel_flag` (future: when scheduler wires in signal handling) will
/// short-circuit the loop between commands; for now this function runs
/// straight through.
pub fn run_pipeline(
    target: &ResolvedTarget,
    pipeline: &[CommandSpec],
) -> anyhow::Result<WorktreeOutcome> {
    let start = Instant::now();
    let mut tail = TailBuffer::new(OUTPUT_CAP_BYTES);
    let mut exit_code: i32 = 0;
    let mut last_index: usize = 0;

    for (idx, spec) in pipeline.iter().enumerate() {
        last_index = idx;
        let mut cmd = build_command(spec);
        cmd.current_dir(&target.worktree_path)
            .env("DAFT_WORKTREE_PATH", &target.worktree_path)
            .env("DAFT_BRANCH_NAME", &target.branch_name)
            .env("DAFT_COMMAND", "exec")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn()?;
        let mut out = child.stdout.take().expect("stdout piped");
        let mut err = child.stderr.take().expect("stderr piped");

        // Drain stdout then stderr sequentially. A future task wires a
        // dual-drain for live rendering; for the runner's correctness
        // (capture of both streams, exit code) sequential drains suffice.
        let mut buf = [0u8; 8192];
        loop {
            match out.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => tail.extend(&buf[..n]),
                Err(e) => return Err(e.into()),
            }
        }
        loop {
            match err.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => tail.extend(&buf[..n]),
                Err(e) => return Err(e.into()),
            }
        }

        let status = child.wait()?;
        exit_code = status.code().unwrap_or(-1);
        if exit_code != 0 {
            break;
        }
    }

    Ok(WorktreeOutcome {
        target: target.clone(),
        last_command_index: last_index,
        exit_code,
        elapsed: start.elapsed(),
        captured_output: tail.into_inner(),
        cancelled: false,
    })
}

fn build_command(spec: &CommandSpec) -> Command {
    match spec {
        CommandSpec::Argv(parts) => {
            let mut c = Command::new(&parts[0]);
            if parts.len() > 1 {
                c.args(&parts[1..]);
            }
            c
        }
        CommandSpec::Shell(s) => {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
            let mut c = Command::new(shell);
            c.arg("-c").arg(s);
            c
        }
    }
}
```

Also inject the two remaining env vars that are always available. Add these
calls inside the `for` loop, before `cmd.spawn()`:

```rust
        if let Ok(root) = crate::get_project_root() {
            cmd.env("DAFT_PROJECT_ROOT", &root);
        }
        if let Ok(gd) = crate::get_git_common_dir() {
            cmd.env("DAFT_GIT_DIR", &gd);
        }
```

(If `crate::get_git_common_dir` doesn't exist under that name, use whichever
helper already wraps `git rev-parse --git-common-dir` — grep `src/lib.rs` and
`src/git/` for `git_common_dir`.)

- [ ] **Step 4: Run tests.**

Run:
`cargo test --lib core::worktree::exec::tests::runs && cargo test --lib core::worktree::exec::tests::stops && cargo test --lib core::worktree::exec::tests::cwd && cargo test --lib core::worktree::exec::tests::injects`
Expected: all four pass.

- [ ] **Step 5: Commit.**

```bash
git add src/core/worktree/exec.rs
git commit -m "feat(exec): per-worktree pipeline runner with bounded capture"
```

---

## Task 10: Per-worktree pipeline runner — shell form (`-x`)

**Files:**

- Modify: `src/core/worktree/exec.rs`

- [ ] **Step 1: Write a failing test for shell semantics.**

Append to the `tests` module:

```rust
    #[test]
    fn shell_form_runs_via_sh() {
        let dir = TempDir::new().unwrap();
        let target = dummy_target(&dir, "master");
        // Pipeline + env expansion only works in the shell form.
        let spec = CommandSpec::Shell("echo $((1+2))".into());
        let outcome = run_pipeline(&target, &[spec]).unwrap();
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(
            String::from_utf8_lossy(&outcome.captured_output).trim(),
            "3"
        );
    }

    #[test]
    fn shell_form_does_not_use_interactive_flag() {
        // Regression guard: the legacy src/exec.rs passes -i which loads
        // rcfiles. `daft exec -x` must NOT do that — the test passes an
        // env var with a value that an rcfile would likely clobber, and
        // asserts we see the outer env.
        std::env::set_var("DAFT_EXEC_TEST_MARKER", "outer-value");
        let dir = TempDir::new().unwrap();
        let target = dummy_target(&dir, "master");
        let spec = CommandSpec::Shell("echo $DAFT_EXEC_TEST_MARKER".into());
        let outcome = run_pipeline(&target, &[spec]).unwrap();
        std::env::remove_var("DAFT_EXEC_TEST_MARKER");
        assert_eq!(
            String::from_utf8_lossy(&outcome.captured_output).trim(),
            "outer-value"
        );
    }
```

- [ ] **Step 2: Run tests.**

Run: `cargo test --lib core::worktree::exec::tests::shell` Expected: both pass
(the shell-form implementation from Task 9 already handles this correctly —
these tests are regression guards).

- [ ] **Step 3: Commit.**

```bash
git add src/core/worktree/exec.rs
git commit -m "test(exec): shell-form regression tests (-c not -i, arithmetic expansion)"
```

---

## Task 11: Scheduler — parallel mode

**Files:**

- Modify: `src/core/worktree/exec.rs`

- [ ] **Step 1: Write failing tests.**

Append to `tests`:

```rust
    #[test]
    fn parallel_runs_all_targets_and_aggregates() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();
        let targets = vec![
            ResolvedTarget { worktree_path: dir1.path().into(), branch_name: "a".into() },
            ResolvedTarget { worktree_path: dir2.path().into(), branch_name: "b".into() },
        ];
        let pipeline = vec![CommandSpec::Argv(vec!["echo".into(), "ok".into()])];
        let report = run_scheduler(&targets, &pipeline, ExecMode::Parallel).unwrap();
        assert_eq!(report.outcomes.len(), 2);
        assert!(report.outcomes.iter().all(|o| o.succeeded()));
        assert_eq!(report.aggregate_exit_code(), 0);
    }

    #[test]
    fn parallel_reports_failures_via_aggregate() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();
        let targets = vec![
            ResolvedTarget { worktree_path: dir1.path().into(), branch_name: "a".into() },
            ResolvedTarget { worktree_path: dir2.path().into(), branch_name: "b".into() },
        ];
        let pipeline = vec![CommandSpec::Argv(vec!["false".into()])];
        let report = run_scheduler(&targets, &pipeline, ExecMode::Parallel).unwrap();
        assert_eq!(report.aggregate_exit_code(), 1);
        assert!(report.outcomes.iter().all(|o| !o.succeeded()));
    }
```

- [ ] **Step 2: Run tests — expect `run_scheduler` not found.**

Run: `cargo test --lib core::worktree::exec::tests::parallel` Expected: compile
errors.

- [ ] **Step 3: Implement `run_scheduler`.**

```rust
use std::sync::Arc;
use std::thread;

/// Run the pipeline across all targets in the requested mode. Returns the
/// aggregated report. Rendering is the caller's responsibility.
pub fn run_scheduler(
    targets: &[ResolvedTarget],
    pipeline: &[CommandSpec],
    mode: ExecMode,
) -> anyhow::Result<ExecReport> {
    match mode {
        ExecMode::Parallel => run_parallel(targets, pipeline),
        ExecMode::Sequential => run_sequential(targets, pipeline, false),
        ExecMode::KeepGoing => run_sequential(targets, pipeline, true),
    }
}

fn run_parallel(
    targets: &[ResolvedTarget],
    pipeline: &[CommandSpec],
) -> anyhow::Result<ExecReport> {
    let pipeline = Arc::new(pipeline.to_vec());
    let handles: Vec<_> = targets
        .iter()
        .cloned()
        .map(|t| {
            let p = Arc::clone(&pipeline);
            thread::spawn(move || run_pipeline(&t, &p))
        })
        .collect();

    let mut outcomes: Vec<WorktreeOutcome> = Vec::new();
    for h in handles {
        match h.join() {
            Ok(Ok(o)) => outcomes.push(o),
            Ok(Err(e)) => return Err(e),
            Err(panic) => return Err(anyhow::anyhow!("worker thread panicked: {:?}", panic)),
        }
    }

    // Preserve targets' resolved order for stable UI rendering.
    outcomes.sort_by_key(|o| {
        targets
            .iter()
            .position(|t| t.worktree_path == o.target.worktree_path)
            .unwrap_or(usize::MAX)
    });

    Ok(ExecReport { outcomes, orphan_branches_skipped: Vec::new() })
}

fn run_sequential(
    targets: &[ResolvedTarget],
    pipeline: &[CommandSpec],
    keep_going: bool,
) -> anyhow::Result<ExecReport> {
    let mut outcomes = Vec::with_capacity(targets.len());
    for t in targets {
        let outcome = run_pipeline(t, pipeline)?;
        let succeeded = outcome.succeeded();
        outcomes.push(outcome);
        if !succeeded && !keep_going {
            break;
        }
    }
    Ok(ExecReport { outcomes, orphan_branches_skipped: Vec::new() })
}
```

- [ ] **Step 4: Run tests.**

Run: `cargo test --lib core::worktree::exec::tests::parallel` Expected: both
tests pass.

- [ ] **Step 5: Commit.**

```bash
git add src/core/worktree/exec.rs
git commit -m "feat(exec): parallel scheduler with stable-order aggregated report"
```

---

## Task 12: Scheduler — sequential and keep-going

**Files:**

- Modify: `src/core/worktree/exec.rs`

- [ ] **Step 1: Write failing tests.**

Append to `tests`:

```rust
    #[test]
    fn sequential_stops_after_first_failure() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();
        let dir3 = TempDir::new().unwrap();
        let targets = vec![
            ResolvedTarget { worktree_path: dir1.path().into(), branch_name: "a".into() },
            ResolvedTarget { worktree_path: dir2.path().into(), branch_name: "b".into() },
            ResolvedTarget { worktree_path: dir3.path().into(), branch_name: "c".into() },
        ];
        // fail on b, check that c never ran.
        let pipeline_ok = vec![CommandSpec::Argv(vec!["true".into()])];
        let pipeline_fail = vec![CommandSpec::Argv(vec!["false".into()])];
        let (p1, p2, p3) = (pipeline_ok.clone(), pipeline_fail, pipeline_ok);
        // Helper: build a mixed pipeline run by interleaving with per-target
        // "which pipeline" selector. We approximate with a small custom
        // driver since the scheduler takes one pipeline.
        let _ = (p1, p2, p3);

        // Simpler test: a pipeline whose first command uses the branch
        // name to decide exit code.
        let pipeline = vec![CommandSpec::Shell(
            r#"case "$DAFT_BRANCH_NAME" in b) exit 1;; *) exit 0;; esac"#.into(),
        )];
        let report = run_scheduler(&targets, &pipeline, ExecMode::Sequential).unwrap();
        assert_eq!(report.outcomes.len(), 2, "third target must not have run");
        assert_eq!(report.outcomes[0].target.branch_name, "a");
        assert_eq!(report.outcomes[1].target.branch_name, "b");
        assert_eq!(report.aggregate_exit_code(), 1);
    }

    #[test]
    fn keep_going_runs_all_despite_failures() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();
        let dir3 = TempDir::new().unwrap();
        let targets = vec![
            ResolvedTarget { worktree_path: dir1.path().into(), branch_name: "a".into() },
            ResolvedTarget { worktree_path: dir2.path().into(), branch_name: "b".into() },
            ResolvedTarget { worktree_path: dir3.path().into(), branch_name: "c".into() },
        ];
        let pipeline = vec![CommandSpec::Shell(
            r#"case "$DAFT_BRANCH_NAME" in b) exit 1;; *) exit 0;; esac"#.into(),
        )];
        let report = run_scheduler(&targets, &pipeline, ExecMode::KeepGoing).unwrap();
        assert_eq!(report.outcomes.len(), 3);
        assert_eq!(report.aggregate_exit_code(), 1);
    }
```

- [ ] **Step 2: Run tests.**

Run:
`cargo test --lib core::worktree::exec::tests::sequential core::worktree::exec::tests::keep`
Expected: both pass (implementation was already added in Task 11).

- [ ] **Step 3: Commit.**

```bash
git add src/core/worktree/exec.rs
git commit -m "test(exec): sequential-stop and keep-going scheduler behavior"
```

---

## Task 13: Command-layer wiring — plumb everything into `run()`

**Files:**

- Modify: `src/commands/exec.rs`

- [ ] **Step 1: Rewrite `run()` end-to-end (no UI yet — we print plain ASCII
      summary to verify plumbing).**

Replace the current stub `run()` with:

```rust
pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-exec"));
    validate_args(&args)?;

    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;
    let config = OutputConfig::new(false, args.verbose);
    let mut output = CliOutput::new(config);

    let wt_config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: false,
    };
    let git = GitCommand::new(wt_config.quiet).with_gitoxide(settings.use_gitoxide);
    let _project_root = get_project_root()?;

    if should_show_gitoxide_notice(settings.use_gitoxide) {
        output.warning("[experimental] Using gitoxide backend for git operations");
    }

    use crate::core::worktree::exec as core;

    let snaps = core::collect_snapshot(&git)?;
    let (targets, orphans) = core::resolve_targets_with_orphans(&args.targets, args.all, &snaps)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if !orphans.is_empty() {
        output.warning(&format!(
            "Skipped {} orphan branch(es) (no worktree): {}",
            orphans.len(),
            orphans.join(", ")
        ));
    }

    let pipeline: Vec<core::CommandSpec> = if !args.trailing.is_empty() {
        vec![core::CommandSpec::Argv(args.trailing.clone())]
    } else {
        args.exec
            .iter()
            .map(|s| core::CommandSpec::Shell(s.clone()))
            .collect()
    };

    // Single-target pass-through deferred to Task 14.

    let mode = if args.keep_going {
        core::ExecMode::KeepGoing
    } else if args.sequential {
        core::ExecMode::Sequential
    } else {
        core::ExecMode::Parallel
    };

    let report = core::run_scheduler(&targets, &pipeline, mode)?;

    // Placeholder summary — replaced by list-mode renderer in Task 15.
    for outcome in &report.outcomes {
        let tag = if outcome.succeeded() { "OK" } else { "FAIL" };
        println!(
            "[{tag}] {} ({:.2}s) exit={}",
            outcome.target.branch_name,
            outcome.elapsed.as_secs_f64(),
            outcome.exit_code
        );
    }

    std::process::exit(report.aggregate_exit_code());
}
```

- [ ] **Step 2: Delete the old stub tests that asserted on the "not yet
      implemented" error, if any.**

Run: `cargo test --lib commands::exec` Expected: all prior arg-parsing and
validation tests still pass. Zero tests reference `"not yet implemented"`.

- [ ] **Step 3: Smoke-test by hand against a real throwaway repo.**

```bash
cd /tmp && rm -rf daft-smoke && mkdir daft-smoke && cd daft-smoke
<path>/daft worktree-init smoke --layout contained
cd smoke/master
<path>/daft worktree-checkout -b feat-a
cd ../master
<path>/daft exec --all -- pwd
```

Expected: two `[OK]` lines, one per worktree, each showing a distinct `pwd`
output in captured rows. Exit 0.

```bash
<path>/daft exec --all -- false
```

Expected: two `[FAIL]` lines. Exit 1.

Clean up: `cd /tmp && rm -rf daft-smoke`.

- [ ] **Step 4: Commit.**

```bash
git add src/commands/exec.rs
git commit -m "feat(exec): end-to-end scheduler plumbing with plain-text summary"
```

---

## Task 14: Single-target pass-through (mode A)

**Files:**

- Modify: `src/commands/exec.rs`

- [ ] **Step 1: Insert pass-through branch before the scheduler call.**

In `run()`, immediately after the `let pipeline = …;` block and before the
`let mode = …;` block, add:

```rust
    // Mode A: single-target pass-through. Inherit stdio; propagate exit
    // code verbatim; never render a UI. Handles `daft exec <single> -- claude`
    // and similar interactive cases without any flag ceremony.
    if targets.len() == 1 {
        let target = &targets[0];
        for spec in &pipeline {
            let mut cmd = match spec {
                core::CommandSpec::Argv(parts) => {
                    let mut c = std::process::Command::new(&parts[0]);
                    if parts.len() > 1 {
                        c.args(&parts[1..]);
                    }
                    c
                }
                core::CommandSpec::Shell(s) => {
                    let shell =
                        std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
                    let mut c = std::process::Command::new(shell);
                    c.arg("-c").arg(s);
                    c
                }
            };
            cmd.current_dir(&target.worktree_path)
                .env("DAFT_WORKTREE_PATH", &target.worktree_path)
                .env("DAFT_BRANCH_NAME", &target.branch_name)
                .env("DAFT_COMMAND", "exec")
                .stdin(std::process::Stdio::inherit())
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit());

            let status = cmd.status()?;
            if !status.success() {
                std::process::exit(status.code().unwrap_or(1));
            }
        }
        std::process::exit(0);
    }
```

- [ ] **Step 2: Smoke-test manually.**

```bash
cd /tmp/daft-smoke/master  # or re-create per Task 13 step 3
<path>/daft exec master -- echo interactive-test
```

Expected: output `interactive-test` printed directly (no `[OK]` prefix, no
table). Exit 0.

```bash
<path>/daft exec master -- false
```

Expected: exit code 1.

```bash
<path>/daft exec master -- cat
```

Expected: `cat` runs and reads from your terminal until you ctrl-D (proves stdin
is inherited).

- [ ] **Step 3: Commit.**

```bash
git add src/commands/exec.rs
git commit -m "feat(exec): single-target stdio pass-through"
```

---

## Task 15: List-mode renderer (mode B) — commands header + per-worktree rows

This is the user-visible multi-target UI. Keep the first cut deliberately
minimal: render the commands list up top, then one line per worktree printed in
order as it completes, with the status sigil, branch name, elapsed time, and —
on failure — the failing command and exit code. No in-place terminal repaint;
just line-by-line append.

**Files:**

- Create: `src/core/worktree/exec/list_renderer.rs` _(new submodule)_
- Modify: `src/core/worktree/exec.rs` → `src/core/worktree/exec/mod.rs` (split
  into a directory module)
- Modify: `src/commands/exec.rs`

- [ ] **Step 1: Split the core module into a directory.**

Run:

```bash
mkdir src/core/worktree/exec
git mv src/core/worktree/exec.rs src/core/worktree/exec/mod.rs
```

- [ ] **Step 2: Create `src/core/worktree/exec/list_renderer.rs`.**

```rust
//! List-mode renderer for `daft exec` multi-target runs.
//!
//! Prints a commands header once, then one line per worktree as each
//! completes. Not interactive; no in-place terminal repainting.

use super::{CommandSpec, ExecReport, WorktreeOutcome};

/// Abstraction that lets tests capture output to a string rather than
/// stdout. Production callers pass `&mut std::io::stdout()`.
pub trait Sink: std::io::Write {}
impl<T: std::io::Write> Sink for T {}

pub fn render_header<W: Sink>(sink: &mut W, pipeline: &[CommandSpec]) -> std::io::Result<()> {
    writeln!(sink, "────────────────────────────────────────────────────────────")?;
    writeln!(sink, "Commands")?;
    for (i, spec) in pipeline.iter().enumerate() {
        writeln!(sink, "  {}. {}", i + 1, spec.display())?;
    }
    writeln!(sink, "────────────────────────────────────────────────────────────")?;
    writeln!(sink, "Worktrees")?;
    Ok(())
}

pub fn render_outcome<W: Sink>(
    sink: &mut W,
    outcome: &WorktreeOutcome,
    pipeline: &[CommandSpec],
) -> std::io::Result<()> {
    let sigil = if outcome.cancelled {
        "⊘"
    } else if outcome.succeeded() {
        "✓"
    } else {
        "✗"
    };
    let elapsed = format!("{:.1}s", outcome.elapsed.as_secs_f64());
    if outcome.succeeded() {
        writeln!(
            sink,
            "  {sigil}  {:<24} ({elapsed})",
            outcome.target.branch_name
        )?;
    } else {
        let cmd_desc = pipeline
            .get(outcome.last_command_index)
            .map(|s| s.display())
            .unwrap_or_default();
        writeln!(
            sink,
            "  {sigil}  {:<24} ({elapsed})   {cmd_desc} → exit {}",
            outcome.target.branch_name, outcome.exit_code
        )?;
    }
    Ok(())
}

pub fn render_failed_output_dump<W: Sink>(
    sink: &mut W,
    report: &ExecReport,
    pipeline: &[CommandSpec],
) -> std::io::Result<()> {
    for outcome in &report.outcomes {
        if outcome.succeeded() {
            continue;
        }
        let cmd_desc = pipeline
            .get(outcome.last_command_index)
            .map(|s| s.display())
            .unwrap_or_default();
        writeln!(
            sink,
            "─── {} ── {cmd_desc} → exit {} ────────────────────────────",
            outcome.target.branch_name, outcome.exit_code
        )?;
        sink.write_all(&outcome.captured_output)?;
        if !outcome.captured_output.ends_with(b"\n") {
            writeln!(sink)?;
        }
    }
    Ok(())
}
```

- [ ] **Step 3: Expose the renderer module from
      `src/core/worktree/exec/mod.rs`.**

Add at the top of the (now-)moved file:

```rust
pub mod list_renderer;
```

- [ ] **Step 4: Write a unit test capturing to a `Vec<u8>`.**

Append to the `tests` module in `src/core/worktree/exec/mod.rs`:

```rust
    #[test]
    fn list_renderer_header_and_rows_and_failed_dump() {
        use super::list_renderer::{render_header, render_outcome, render_failed_output_dump};

        let pipeline = vec![CommandSpec::Argv(vec!["cargo".into(), "test".into()])];
        let outcomes = vec![
            WorktreeOutcome {
                target: ResolvedTarget {
                    worktree_path: "/r/master".into(),
                    branch_name: "master".into(),
                },
                last_command_index: 0,
                exit_code: 0,
                elapsed: std::time::Duration::from_millis(800),
                captured_output: b"ok\n".to_vec(),
                cancelled: false,
            },
            WorktreeOutcome {
                target: ResolvedTarget {
                    worktree_path: "/r/feat-dirty".into(),
                    branch_name: "feat/dirty".into(),
                },
                last_command_index: 0,
                exit_code: 101,
                elapsed: std::time::Duration::from_millis(1200),
                captured_output: b"panicked!\n".to_vec(),
                cancelled: false,
            },
        ];
        let report = ExecReport { outcomes, orphan_branches_skipped: vec![] };

        let mut out: Vec<u8> = Vec::new();
        render_header(&mut out, &pipeline).unwrap();
        for o in &report.outcomes {
            render_outcome(&mut out, o, &pipeline).unwrap();
        }
        render_failed_output_dump(&mut out, &report, &pipeline).unwrap();

        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("Commands"), "missing Commands header");
        assert!(s.contains("1. cargo test"), "missing pipeline row");
        assert!(s.contains("✓") && s.contains("master"), "missing success row");
        assert!(s.contains("✗") && s.contains("feat/dirty"), "missing fail row");
        assert!(s.contains("exit 101"), "missing exit code");
        assert!(s.contains("panicked!"), "missing failed output dump");
    }
```

- [ ] **Step 5: Run the test.**

Run: `cargo test --lib core::worktree::exec::tests::list_renderer` Expected:
passes.

- [ ] **Step 6: Wire the renderer into `commands/exec.rs`.**

Replace the placeholder summary loop in `run()` (the
`for outcome in &report.outcomes { let tag = … }` block) with:

```rust
    let stdout = std::io::stdout();
    let mut sink = stdout.lock();
    core::list_renderer::render_header(&mut sink, &pipeline)?;
    for outcome in &report.outcomes {
        core::list_renderer::render_outcome(&mut sink, outcome, &pipeline)?;
    }
    core::list_renderer::render_failed_output_dump(&mut sink, &report, &pipeline)?;
    drop(sink);

    std::process::exit(report.aggregate_exit_code());
```

- [ ] **Step 7: Smoke-test.**

```bash
<path>/daft exec --all -- pwd       # in a 2+ worktree repo
<path>/daft exec --all -- false
```

Expected: pretty table with ✓/✗ rows. Failing case dumps the captured output of
each failing worktree after the rows.

- [ ] **Step 8: Commit.**

```bash
git add src/core/worktree/exec src/commands/exec.rs
git commit -m "feat(exec): list-mode renderer with commands header, rows, failure dump"
```

---

## Task 16: Verbose windows mode (mode C) — investigate and mirror

The spec acknowledges this as "extract or mirror, whichever is lower risk" at
implementation time. Investigate first.

**Files:**

- Modify: `src/core/worktree/exec/mod.rs`
- Possibly modify: files under `src/output/hook_progress/`,
  `src/hooks/yaml_executor/`

- [ ] **Step 1: Inspect the hooks progress renderer.**

Read:

- `src/output/hook_progress/mod.rs`
- `src/output/hook_progress/interactive.rs`
- `src/hooks/yaml_executor/mod.rs` (entry points that wire the presenter)

Identify the smallest public type that accepts a stream of "job-started /
job-finished / job-output" events and renders live windows.

- [ ] **Step 2: Decide: extract or mirror.**

- If the hooks renderer can be called with a synthetic `HookOutputConfig` and
  fed events from a channel that `daft exec` emits, extract a thin shim named
  `src/core/worktree/exec/windows_renderer.rs` that wraps it.
- If the renderer is entangled with hook-specific context (`HookDef`, trust
  checks, ...) in ways that would require significant refactoring to reuse,
  mirror only the output-panel logic. The mirror can be a dramatically reduced
  subset: one panel per running pipeline step, ratatui-backed, collapsing as
  commands finish. Budget: ≤300 lines; if it grows past that, reconsider
  extraction.

Document the decision in a commit message:
`feat(exec): windows-mode renderer via <extraction|mirror>: <one-sentence rationale>`.

- [ ] **Step 3: Add the renderer and wire it behind `args.verbose`.**

The renderer must expose exactly this entry point so wiring stays identical
whichever path (extraction or mirror) you take:

```rust
// In src/core/worktree/exec/windows_renderer.rs (or shim)
pub fn run_with_live_windows(
    targets: &[ResolvedTarget],
    pipeline: &[CommandSpec],
    mode: ExecMode,
) -> anyhow::Result<ExecReport>;
```

(Task 17 will widen this to take a `&CancelFlag` alongside the equivalent
widening of `run_scheduler`. Leave the signature cancel-free for now.)

**Minimum viable mirror (fallback if extraction is too invasive).** ~200 lines
of `ratatui` + `crossterm` already in the project. One "window" per running
(worktree × command index) pair. Each window is a `ratatui::widgets::Paragraph`
showing the last N lines of captured output (N = 10). Windows layout via
`ratatui::layout::Layout` with `Direction::Vertical`. When a pair finishes, that
window collapses to a single-line status row. Between spawn and finish, read
stdout/stderr in a background thread per pair and push bytes onto a
`std::sync::mpsc::Sender`; the renderer thread drains the channel each tick.
Reuse `TailBuffer` for capture — the same buffer ends up in the returned
`ExecReport` so the failed-output dump works.

Wire it into `commands/exec.rs` after target resolution, before the list-mode
call:

```rust
    let report = if args.verbose {
        core::windows_renderer::run_with_live_windows(&targets, &pipeline, mode)?
    } else {
        core::run_scheduler(&targets, &pipeline, mode)?
    };

    // (list-mode header + rows are only for !args.verbose; the failed-output
    // dump runs in both modes so the terminal scrollback holds the failure.)
    let stdout = std::io::stdout();
    let mut sink = stdout.lock();
    if !args.verbose {
        core::list_renderer::render_header(&mut sink, &pipeline)?;
        for outcome in &report.outcomes {
            core::list_renderer::render_outcome(&mut sink, outcome, &pipeline)?;
        }
    }
    core::list_renderer::render_failed_output_dump(&mut sink, &report, &pipeline)?;
    drop(sink);

    std::process::exit(report.aggregate_exit_code());
```

- [ ] **Step 4: Minimal test.**

Integration test via YAML scenario in Task 22 covers this end-to-end. Skip unit
tests for the TUI — they have low return per line of test code.

- [ ] **Step 5: Commit.**

```bash
git add -A
git commit -m "feat(exec): windows-mode (verbose) renderer <rationale>"
```

---

## Task 17: SIGINT handling

On first SIGINT, propagate SIGTERM to all running children, wait, render final
state. On second SIGINT, escalate to SIGKILL.

**Files:**

- Modify: `src/core/worktree/exec/mod.rs`

- [ ] **Step 1: Thread a shared cancel flag through the scheduler.**

Add near the top of `src/core/worktree/exec/mod.rs`:

```rust
use std::sync::atomic::{AtomicUsize, Ordering};

/// 0 = normal, 1 = soft-cancel (SIGTERM children), 2 = hard-cancel (SIGKILL).
pub struct CancelFlag(AtomicUsize);

impl CancelFlag {
    pub fn new() -> Self { Self(AtomicUsize::new(0)) }
    pub fn level(&self) -> usize { self.0.load(Ordering::SeqCst) }
    pub fn escalate(&self) {
        // 0 → 1 → 2; saturating at 2.
        let cur = self.0.load(Ordering::SeqCst);
        if cur < 2 {
            self.0.store(cur + 1, Ordering::SeqCst);
        }
    }
}
```

Change `run_pipeline` to take `&CancelFlag`; check between commands in the
pipeline and between read-buffer iterations. On `level() >= 1`, send `SIGTERM`
(unix) / `TerminateProcess` (windows — out of scope, keep behind `cfg(unix)` and
no-op elsewhere). On `level() >= 2`, `child.kill()`.

Update `run_scheduler` / `run_parallel` / `run_sequential` signatures to accept
`&CancelFlag` and forward.

- [ ] **Step 2: Wire `ctrlc::set_handler` in `commands/exec.rs`.**

```rust
    let cancel = std::sync::Arc::new(core::CancelFlag::new());
    let cancel_handler = std::sync::Arc::clone(&cancel);
    ctrlc::set_handler(move || {
        cancel_handler.escalate();
    })
    .ok(); // If another handler is already installed, leave it alone.
```

Pass `&cancel` into `run_scheduler`.

- [ ] **Step 3: Write a smoke test (not CI-safe for unit tests — run
      manually).**

Create `tests/manual/scenarios/worktree-exec/sigint-cancels.yml` in Task 22.
Verify manually:

```bash
<path>/daft exec --all -- sh -c 'sleep 30' &
pid=$!
sleep 1
kill -INT $pid
wait $pid
echo "exit: $?"
```

Expected: rapid exit (<5s), cancelled rows shown, non-zero exit.

- [ ] **Step 4: Commit.**

```bash
git add src/core/worktree/exec src/commands/exec.rs
git commit -m "feat(exec): SIGINT propagation with SIGTERM-then-SIGKILL escalation"
```

---

## Task 18: Completions — registry wiring

All five registries. No new completion logic yet; just routing.

**Files:**

- Modify: `src/commands/completions/mod.rs`
- Modify: `xtask/src/main.rs`

- [ ] **Step 1: In `src/commands/completions/mod.rs`:**

**18a.** Append to `VERB_ALIAS_GROUPS` (line 27-40, inside the slice):

```rust
    (&["exec"], "git-worktree-exec"),
```

**18b.** Append to `COMMANDS` (line 43-59):

```rust
    "git-worktree-exec",
```

**18c.** Append to `get_command_for_name` match (line 62-81):

```rust
        "git-worktree-exec" => Some(crate::commands::exec::Args::command()),
```

**18d.** Append `"git-worktree-exec"` to the `uses_rich_completions` match (line
85-96):

```rust
            | "git-worktree-exec"
```

**18e.** Add an assertion to the `rich_commands_use_compadd_v_groups_in_zsh`
test and to all sibling tests at the bottom of the file (search for
`rich_commands = [` — three locations). Each should gain `"git-worktree-exec"`
so rich-completion regressions are caught automatically.

- [ ] **Step 2: In `xtask/src/main.rs`:**

**18f.** Append to `COMMANDS` (line 18-41):

```rust
    "git-worktree-exec",
```

**18g.** Append to `get_command_for_name` match (line 140-172):

```rust
        "git-worktree-exec" => Some(daft::commands::exec::Args::command()),
```

**18h.** Append a new `DaftVerbEntry` to `DAFT_VERBS` (line 54-120):

```rust
    DaftVerbEntry {
        daft_name: "daft-exec",
        source_command: "git-worktree-exec",
        about_override: None,
    },
```

**18i.** Add `"git-worktree-exec" =>` arm to `daft_verb_tip` (line 176-215):

```rust
        "git-worktree-exec" => Some(
            "::: tip\nThis command is also available as `daft exec`. See [daft exec](./daft-exec.md).\n:::\n",
        ),
```

**18j.** Add `"git-worktree-exec"` to `related_commands` (line 218-275). At
minimum:

```rust
        "git-worktree-exec" => vec!["git-worktree-sync", "git-worktree-list", "git-worktree-carry"],
```

- [ ] **Step 3: Run the completion-related tests.**

Run: `cargo test --lib commands::completions` Expected: all pass, including the
three "rich commands" tests that now exercise `"git-worktree-exec"`.

- [ ] **Step 4: Commit.**

```bash
git add src/commands/completions/mod.rs xtask/src/main.rs
git commit -m "chore(exec): register worktree-exec in completions and xtask registries"
```

---

## Task 19: Completions — backend (`__complete`) dispatch + CONFIG_EXEC

**Files:**

- Modify: `src/commands/complete.rs`

- [ ] **Step 1: Add `CONFIG_EXEC`.**

Around `src/commands/complete.rs:1054-1066` (where `CONFIG_CARRY` and
`CONFIG_FETCH` are defined), insert:

```rust
const CONFIG_EXEC: RichCompletionConfig = RichCompletionConfig {
    include_worktrees: true,
    include_local: false,
    include_remote: false,
    exclude_current: false,
};
```

- [ ] **Step 2: Add the dispatch arm.**

In the big `match (command, position)` at line 82, add between `CONFIG_FETCH`
and `CONFIG_BRANCH` arms:

```rust
        // git-worktree-exec: worktree-only completions, current worktree included
        ("git-worktree-exec", _) => Ok(format_entries_as_strings(&complete_rich_branches(
            word,
            &CONFIG_EXEC,
        )?)),
```

- [ ] **Step 3: Run full completion tests.**

Run: `cargo test --lib commands::completions commands::complete` Expected: all
pass.

- [ ] **Step 4: Commit.**

```bash
git add src/commands/complete.rs
git commit -m "feat(exec): rich branch-name completions (worktree-only)"
```

---

## Task 20: Completions — umbrella `daft` verb-alias dispatch per shell

The `DAFT_BASH_COMPLETIONS`, `DAFT_ZSH_COMPLETIONS`, and fish umbrella
string-literals each contain a case statement dispatching `daft <verb>` to the
underlying command's completer. `exec` needs to be added to each.

**Files:**

- Modify: `src/commands/completions/bash.rs`
- Modify: `src/commands/completions/zsh.rs`
- Modify: `src/commands/completions/fish.rs`

- [ ] **Step 1: Bash — add `exec)` verb dispatch.**

In `src/commands/completions/bash.rs`, find the existing `case "${words[1]}"`
block inside `DAFT_BASH_COMPLETIONS` (around line 383-450). After the `carry)`
entry (lines 398-403), insert:

```bash
            exec)
                COMP_WORDS=("git-worktree-exec" "${COMP_WORDS[@]:2}")
                COMP_CWORD=$((COMP_CWORD - 1))
                _git_worktree_exec
                return 0
                ;;
```

- [ ] **Step 2: Zsh — add the equivalent case.**

In `src/commands/completions/zsh.rs`, find the `DAFT_ZSH_COMPLETIONS` literal.
Search for the `carry)` case pattern and add `exec)` one below, dispatching to
`__git_worktree_exec_impl` (whatever the per-command impl function is named —
match the `carry` pattern exactly).

- [ ] **Step 3: Fish — add the equivalent case.**

In `src/commands/completions/fish.rs`, find the `generate_daft_fish_completions`
function output. For each of the verb rows matching `carry`, add a sibling row
for `exec` that forwards to `git-worktree-exec` completions. Match exactly the
pattern you see for `carry`.

- [ ] **Step 4: Run the completion tests and verify the generated strings
      contain the new dispatch.**

Run: `cargo test --lib commands::completions` Expected: all pass.

Append a quick regression test at the bottom of
`src/commands/completions/mod.rs` tests module:

```rust
    #[test]
    fn umbrella_shells_dispatch_exec_verb() {
        let bash = bash::DAFT_BASH_COMPLETIONS;
        assert!(bash.contains("exec)"), "bash umbrella must dispatch `exec` verb");
        assert!(bash.contains("_git_worktree_exec"), "bash umbrella must call per-command completer");

        let combined_zsh = format!(
            "{}\n{}",
            zsh::generate_zsh_completion_string("git-worktree-exec").unwrap(),
            zsh::DAFT_ZSH_COMPLETIONS,
        );
        assert!(combined_zsh.contains("exec)"), "zsh umbrella must dispatch `exec`");

        let fish = fish::generate_daft_fish_completions();
        assert!(
            fish.contains("git-worktree-exec") || fish.contains(" exec "),
            "fish umbrella must reference exec verb"
        );
    }
```

Run it:
`cargo test --lib commands::completions::tests::umbrella_shells_dispatch_exec_verb`
Expected: passes.

- [ ] **Step 5: Commit.**

```bash
git add src/commands/completions
git commit -m "feat(exec): umbrella daft-verb completion dispatch in bash/zsh/fish"
```

---

## Task 21: Docs — categories, CLI reference pages, man pages

**Files:**

- Modify: `src/commands/docs.rs`
- Create/generate: `man/git-worktree-exec.1`, `man/daft-exec.1`,
  `docs/cli/git-worktree-exec.md`, `docs/cli/daft-exec.md`
- Create: `docs/guide/running-commands-across-worktrees.md`

- [ ] **Step 1: Add to `src/commands/docs.rs` categories.**

**21a.** In `get_daft_categories()` (line 49), create a new category between
"share changes across worktrees" (line 95-102) and "start a worktree-based
repository" (line 103):

```rust
        CommandCategory {
            title: "run commands across worktrees",
            layout: CategoryLayout::List,
            commands: vec![CommandEntry {
                display_name: "exec",
                command: exec::Args::command(),
            }],
        },
```

Add `exec` to the `use crate::commands::{…}` import at the top of the file (line
11-14).

**21b.** In `get_git_daft_categories()` (line 168), create the mirror category
after "share changes across worktrees" (line 197-203):

```rust
        CommandCategory {
            title: "run commands across worktrees",
            layout: CategoryLayout::List,
            commands: vec![CommandEntry {
                display_name: "worktree-exec",
                command: exec::Args::command(),
            }],
        },
```

- [ ] **Step 2: Regenerate man pages.**

Run: `mise run man:gen` Expected: new files `man/git-worktree-exec.1` and
`man/daft-exec.1` created. The EXAMPLES section we put in `after_help` (Task 1)
appears in both.

Verify: `mise run man:verify` Expected: passes (man pages match clap output).

- [ ] **Step 3: Regenerate CLI docs.**

Run: `cargo xtask gen-cli-docs --command git-worktree-exec` Expected:
`docs/cli/git-worktree-exec.md` created from clap metadata.

For the daft-verb form (`docs/cli/daft-exec.md`), hand-author a short companion
page — check `docs/cli/daft-carry.md` for template. Keep it brief; it
cross-links to the git-form page.

- [ ] **Step 4: Write the narrative guide.**

Create `docs/guide/running-commands-across-worktrees.md`:

````markdown
---
title: Running commands across worktrees
description:
  Use daft exec to run one or more commands against one or many worktrees
  without cd-ing into them.
---

# Running commands across worktrees

`daft exec` runs a command against one or more worktrees without changing your
current directory. It's the right tool when you want to:

- Run a test or build on a specific branch without switching to it
- Fan-out a lint or format pass across every branch in flight
- Execute a pipeline of setup commands across many worktrees at once

## The basics

```bash
daft exec feat/auth -- cargo test
daft exec --all -- pnpm lint
daft exec 'feat/*' -- npm test
```
````

Positional arguments can be branch names, worktree directory names, or globs
against branch names. `--all` expands to every worktree. The `--` separator
marks the boundary between daft's flags and the command you want to run —
everything after it is forwarded verbatim.

## Multiple commands

Pass `-x` one or more times to run a pipeline of commands sequentially per
worktree. If any command in the pipeline fails, that worktree's pipeline stops;
other worktrees are unaffected.

```bash
daft exec --all -x 'mise install' -x 'pnpm build' -x 'pnpm test'
```

`-x` and `--` are mutually exclusive. Use `-x` for pipelines, `--` for single
commands whose own flags would otherwise collide with daft's.

## Parallel vs sequential

By default, worktrees run in parallel. Use `--sequential` to run them one at a
time (stopping on first failure), or `--keep-going` to run every worktree even
after failures:

```bash
daft exec --all -- cargo test               # parallel, default
daft exec --all --sequential -- cargo test  # one at a time, stop on first fail
daft exec --all --keep-going -- cargo test  # one at a time, don't stop
```

## Single-target pass-through

When your selectors resolve to exactly one worktree, daft hands stdio through
directly. Interactive programs work:

```bash
daft exec feat/auth -- claude
daft exec feat/auth -- vim src/main.rs
```

No UI renders; the child's exit code is propagated verbatim.

## Viewing output

In multi-target runs, successful worktrees' output is discarded; failed
worktrees' output is dumped at the end. Use `-v` to see everything live in
hook-style windows:

```bash
daft exec --all -v -- cargo test
```

## Relationship to other commands

| Use case                                          | Command                                         |
| ------------------------------------------------- | ----------------------------------------------- |
| Run once, ad-hoc, across many worktrees           | `daft exec`                                     |
| Run every time a worktree is created              | `daft.yml` `worktree-post-create` hook          |
| Run once per command invocation on a new worktree | `-x` flag on `daft clone` / `init` / `checkout` |

## See also

- [daft exec](../cli/daft-exec.md) /
  [git worktree-exec](../cli/git-worktree-exec.md) — CLI reference
- [Hooks](./hooks.md) — recurring per-worktree automation via `daft.yml`

````

- [ ] **Step 5: Commit.**

```bash
git add src/commands/docs.rs man/git-worktree-exec.1 man/daft-exec.1 docs/cli/git-worktree-exec.md docs/cli/daft-exec.md docs/guide/running-commands-across-worktrees.md
git commit -m "docs(exec): help-text categories, man pages, CLI reference, guide"
````

---

## Task 22: YAML manual test scenarios

**Files:**

- Create: `tests/manual/scenarios/worktree-exec/single-target-passthrough.yml`
- Create: `tests/manual/scenarios/worktree-exec/multi-target-parallel.yml`
- Create: `tests/manual/scenarios/worktree-exec/glob-and-all.yml`
- Create: `tests/manual/scenarios/worktree-exec/failure-dump.yml`
- Create: `tests/manual/scenarios/worktree-exec/x-pipeline-stops-on-failure.yml`
- Create: `tests/manual/scenarios/worktree-exec/sequential-and-keep-going.yml`
- Create: `tests/manual/scenarios/worktree-exec/unmatched-positional.yml`
- Create: `tests/manual/scenarios/worktree-exec/orphan-branch-skip.yml`
- Create: `tests/manual/scenarios/worktree-exec/verbose-windows.yml`

**Note on directory name.** Keep distinct from the existing
`tests/manual/scenarios/exec/` (which tests the legacy `-x` on
clone/init/checkout). `worktree-exec/` makes the two unambiguous.

- [ ] **Step 1: Write `single-target-passthrough.yml`.**

```yaml
name: "exec single-target passthrough"
description:
  "daft exec with exactly one target inherits stdio and propagates exit"

repos:
  - name: exec-single
    use_fixture: standard-remote

steps:
  - name: "Clone the repository"
    run: "git-worktree-clone --layout contained $REMOTE_EXEC_SINGLE"
    expect: { exit_code: 0 }

  - name: "Run a single command against the default worktree"
    run: "git-worktree-exec main -- sh -c 'echo hello > exec_marker.txt'"
    cwd: "$WORK_DIR/exec-single/main"
    expect:
      exit_code: 0
      files_exist:
        - "$WORK_DIR/exec-single/main/exec_marker.txt"
      file_contains:
        - path: "$WORK_DIR/exec-single/main/exec_marker.txt"
          content: "hello"

  - name: "Single-target command failure propagates exit code"
    run: "git-worktree-exec main -- false"
    cwd: "$WORK_DIR/exec-single/main"
    expect:
      exit_code: 1
```

- [ ] **Step 2: Write `multi-target-parallel.yml`.**

```yaml
name: "exec multi-target parallel happy path"
description: "daft exec across 3 worktrees runs in parallel and reports success"

repos:
  - name: exec-parallel
    use_fixture: standard-remote

steps:
  - run: "git-worktree-clone --layout contained $REMOTE_EXEC_PARALLEL"
    expect: { exit_code: 0 }
  - run: "git-worktree-checkout -b feat-a"
    cwd: "$WORK_DIR/exec-parallel/main"
    expect: { exit_code: 0 }
  - run: "git-worktree-checkout -b feat-b"
    cwd: "$WORK_DIR/exec-parallel/main"
    expect: { exit_code: 0 }
  - name: "Run echo across all worktrees"
    run: "git-worktree-exec --all -- sh -c 'echo ran > marker'"
    cwd: "$WORK_DIR/exec-parallel/main"
    expect:
      exit_code: 0
      files_exist:
        - "$WORK_DIR/exec-parallel/main/marker"
        - "$WORK_DIR/exec-parallel/feat-a/marker"
        - "$WORK_DIR/exec-parallel/feat-b/marker"
```

- [ ] **Step 3: Write `glob-and-all.yml`, `failure-dump.yml`,
      `x-pipeline-stops-on-failure.yml`, `sequential-and-keep-going.yml`,
      `unmatched-positional.yml`, `orphan-branch-skip.yml`,
      `verbose-windows.yml`.**

Follow the same skeleton. Each file asserts via `expect.exit_code`,
`expect.files_exist`/`files_not_exist` (or equivalent "file_does_not_exist" if
supported — fall back to exit-code-only when not), and (for failure-dump)
`expect.stdout_contains` / `expect.stderr_contains` if supported. If an
assertion verb you need doesn't exist in the YAML schema, fall back to a shell
step that greps captured output and exits non-zero on mismatch:

```yaml
- name: "Assert that feat/a failure is dumped"
  run:
    "git-worktree-exec --all -- false > /tmp/out 2>&1 || true; grep -q 'feat/a'
    /tmp/out && grep -q 'exit 1' /tmp/out"
  expect: { exit_code: 0 }
```

Pattern-match existing scenarios in `tests/manual/scenarios/sync/` or
`tests/manual/scenarios/carry/` for assertion vocabulary — don't invent new
keys.

- [ ] **Step 4: Run all scenarios.**

Run: `mise run test:manual -- --ci worktree-exec` Expected: all scenarios pass.

- [ ] **Step 5: Commit.**

```bash
git add tests/manual/scenarios/worktree-exec
git commit -m "test(exec): YAML scenarios for worktree-exec UX matrix"
```

---

## Task 23: Bash integration test

**Files:**

- Create: `tests/integration/test_worktree_exec.sh`
- Modify: `tests/integration/test_all.sh`

- [ ] **Step 1: Write the test harness.**

```bash
#!/bin/bash
# Integration tests for `daft worktree-exec` / `daft exec`.
# Keep distinct from test_exec.sh which covers the legacy -x option on
# clone/init/checkout.

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

test_exec_single_target_pwd_in_worktree_cwd() {
    create_test_remote "exec-single" || return 1
    git-worktree-clone --layout contained "$REMOTE_EXEC_SINGLE" || return 1
    cd exec-single/main || return 1

    local out
    out=$(git-worktree-exec main -- pwd)
    if [[ "$out" != *"exec-single/main"* ]]; then
        log_error "single-target pwd wrong: $out"
        return 1
    fi
}

test_exec_all_runs_everywhere() {
    create_test_remote "exec-all" || return 1
    git-worktree-clone --layout contained "$REMOTE_EXEC_ALL" || return 1
    cd exec-all/main || return 1
    git-worktree-checkout -b feat-a || return 1
    cd ../main || return 1

    git-worktree-exec --all -- sh -c 'echo hi > marker' || return 1

    assert_file_exists "$WORK_DIR/exec-all/main/marker" "main marker present" || return 1
    assert_file_exists "$WORK_DIR/exec-all/feat-a/marker" "feat-a marker present" || return 1
}

test_exec_sequential_stops_on_failure() {
    create_test_remote "exec-seq" || return 1
    git-worktree-clone --layout contained "$REMOTE_EXEC_SEQ" || return 1
    cd exec-seq/main || return 1
    git-worktree-checkout -b feat-a || return 1
    git-worktree-checkout -b feat-b || return 1
    cd ../main || return 1

    # Fail on feat-a via branch-name check; expect feat-b never runs.
    git-worktree-exec --all --sequential -x \
        'case "$DAFT_BRANCH_NAME" in feat-a) exit 1;; *) echo did > marker;; esac'
    local exit_code=$?
    if [[ $exit_code -eq 0 ]]; then
        log_error "expected non-zero exit"; return 1
    fi

    # The resolved order is sorted (main, feat-a, feat-b).
    # "main" runs first and should have written marker.
    # "feat-a" fails; "feat-b" must NOT have run.
    assert_file_exists "$WORK_DIR/exec-seq/main/marker" "main ran" || return 1
    if [[ -f "$WORK_DIR/exec-seq/feat-b/marker" ]]; then
        log_error "feat-b marker should not exist after sequential stop"
        return 1
    fi
}

test_exec_keep_going_runs_all_despite_failure() {
    create_test_remote "exec-keep" || return 1
    git-worktree-clone --layout contained "$REMOTE_EXEC_KEEP" || return 1
    cd exec-keep/main || return 1
    git-worktree-checkout -b feat-a || return 1
    git-worktree-checkout -b feat-b || return 1
    cd ../main || return 1

    git-worktree-exec --all --keep-going -x \
        'case "$DAFT_BRANCH_NAME" in feat-a) exit 1;; *) echo did > marker;; esac'
    local exit_code=$?
    if [[ $exit_code -eq 0 ]]; then
        log_error "expected non-zero exit"; return 1
    fi
    assert_file_exists "$WORK_DIR/exec-keep/main/marker" "main ran" || return 1
    assert_file_exists "$WORK_DIR/exec-keep/feat-b/marker" "feat-b ran despite feat-a failure" || return 1
}

test_exec_unmatched_positional_errors() {
    create_test_remote "exec-err" || return 1
    git-worktree-clone --layout contained "$REMOTE_EXEC_ERR" || return 1
    cd exec-err/main || return 1

    if git-worktree-exec zzzzz -- echo; then
        log_error "expected error for unmatched positional"
        return 1
    fi
}

# --- Driver ---

setup

run_test "exec single-target pwd uses worktree cwd" test_exec_single_target_pwd_in_worktree_cwd
run_test "exec --all fans out to every worktree" test_exec_all_runs_everywhere
run_test "exec --sequential stops on first failure" test_exec_sequential_stops_on_failure
run_test "exec --keep-going continues through failures" test_exec_keep_going_runs_all_despite_failure
run_test "exec unmatched positional errors out" test_exec_unmatched_positional_errors

print_summary
```

- [ ] **Step 2: Add to `tests/integration/test_all.sh`.**

Find the line that sources `test_carry.sh` (or similar) and add
`source "$(dirname "$0")/test_worktree_exec.sh"` in alphabetical order. If the
file uses a loop over `test_*.sh`, it'll pick up the new test automatically —
verify by searching for the pattern. Don't add it twice if the loop already
covers it.

- [ ] **Step 3: Run.**

Run: `mise run test:integration` Expected: new tests pass alongside all existing
tests.

- [ ] **Step 4: Commit.**

```bash
git add tests/integration/test_worktree_exec.sh tests/integration/test_all.sh
git commit -m "test(exec): bash integration tests for worktree-exec command"
```

---

## Task 24: SKILL.md + CHANGELOG.md

**Files:**

- Modify: `SKILL.md`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Update `SKILL.md`.**

**24a.** In the Command Reference / Management table (around the sections
starting at "### Management" and "### Worktree Lifecycle"), add a new row under
Management after `daft sync`:

```
| `daft worktree-exec [TARGETS]... [--all] [-x CMD]... [-- CMD ARGS]...` | Run command(s) across one or more worktrees: positional/glob targets, `--all` for every worktree, `-x` for repeatable shell pipelines, trailing `--` for direct argv. Parallel by default; `--sequential`/`--keep-going` for serial modes; `-v` for live hook-style windows. |
```

**24b.** In the Invocation Forms verb-aliases table, add:

```
| `daft exec`   | `daft worktree-exec`        |
```

**24c.** In "Workflow Guidance for Agents," add a row:

```
| "Run my build on these worktrees" | `daft worktree-exec feat/a feat/b -- <cmd>` or `daft exec --all -- <cmd>` for every worktree |
```

**24d.** In "Per-worktree Isolation" or a new short subsection under the
Management heading, add a note: "For ad-hoc commands across worktrees (without
creating a hook), use `daft worktree-exec`. For recurring per-worktree
automation, use `daft.yml` hooks."

- [ ] **Step 2: Update `CHANGELOG.md`.**

Under the unreleased / next-version section:

```markdown
### Added

- `daft worktree-exec` (alias `daft exec`) runs one or more commands across one
  or more worktrees. Supports glob targets, `--all`,
  parallel/sequential/keep-going modes, single-target stdio pass-through,
  list-mode output table, and hook-style live windows with `-v`.
```

- [ ] **Step 3: Commit.**

```bash
git add SKILL.md CHANGELOG.md
git commit -m "docs(exec): update SKILL.md and CHANGELOG for worktree-exec"
```

---

## Task 25: Final verification

**Files:** none

- [ ] **Step 1: Run all CI commands in order.**

```bash
mise run fmt
mise run clippy
mise run test:unit
mise run test:integration
mise run man:verify
mise run test:manual -- --ci
```

Expected: every command exits 0. `clippy` produces zero warnings.

- [ ] **Step 2: Review the full PR diff for leftover TODOs, debug prints, or
      skipped tests.**

Run: `git log --oneline master..HEAD` → confirm commit chain tells the feature
story. Run: `git diff master --stat` → verify only the expected files touched.

- [ ] **Step 3: Smoke-test the full happy path one more time.**

```bash
cd /tmp && rm -rf daft-final-smoke && mkdir daft-final-smoke && cd daft-final-smoke
daft worktree-init smoke --layout contained
cd smoke/master
daft worktree-checkout -b feat-x
daft worktree-checkout -b feat-y
cd ../master

daft exec --all -- pwd                     # pretty table, 3 ✓ rows
daft exec feat/x feat/y -- echo hi         # 2-row table
daft exec 'feat/*' -- echo hello           # glob match
daft exec master -- echo single-inherit    # pass-through, no table
daft exec --all -x 'false' -x 'echo skip'  # failure dump shows exit 1, no "skip"
daft exec --all -v -- sleep 1              # windows mode
daft exec nonexistent -- echo              # unmatched error
daft exec --all --sequential -- sh -c '...' # sequential mode
```

All should behave as documented in the spec.

Clean up: `cd /tmp && rm -rf daft-final-smoke`.

- [ ] **Step 4: Open the PR.**

Title: `feat: add daft worktree-exec for running commands across worktrees`
Body: summary + test plan. Link the spec:
`docs/superpowers/specs/2026-04-21-worktree-exec-design.md`.

---

## Appendix: Files touched

**Created:**

- `src/commands/exec.rs`
- `src/core/worktree/exec/mod.rs` (was `src/core/worktree/exec.rs` pre-Task-15)
- `src/core/worktree/exec/list_renderer.rs`
- `src/core/worktree/exec/windows_renderer.rs` (per Task 16 decision)
- `man/git-worktree-exec.1`, `man/daft-exec.1`
- `docs/cli/git-worktree-exec.md`, `docs/cli/daft-exec.md`
- `docs/guide/running-commands-across-worktrees.md`
- `tests/manual/scenarios/worktree-exec/*.yml`
- `tests/integration/test_worktree_exec.sh`

**Modified:**

- `src/commands/mod.rs`
- `src/core/worktree/mod.rs`
- `src/lib.rs` (`DAFT_VERBS`)
- `src/main.rs` (three routing edits)
- `src/commands/complete.rs` (`CONFIG_EXEC`, dispatch arm)
- `src/commands/completions/mod.rs` (`VERB_ALIAS_GROUPS`, `COMMANDS`,
  `get_command_for_name`, `uses_rich_completions`, tests)
- `src/commands/completions/bash.rs`, `zsh.rs`, `fish.rs` (umbrella verb
  dispatch)
- `src/commands/docs.rs` (two new categories)
- `src/git/refs.rs` (only if `list_local_branches` needed adding in Task 8)
- `xtask/src/main.rs` (`COMMANDS`, `DAFT_VERBS`, `get_command_for_name`,
  `daft_verb_tip`, `related_commands`)
- `tests/integration/test_all.sh`
- `SKILL.md`, `CHANGELOG.md`
