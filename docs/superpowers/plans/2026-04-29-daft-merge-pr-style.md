# daft merge — PR-style redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reshape `daft merge` to a GitHub-PR-style 4-style enum (`merge` |
`squash` | `rebase` | `rebase-merge`) with a 2-outcome cleanup enum (`keep` |
`remove-branch`) and an inline `--set-default` flag. Hard cutover on the legacy
flag and config surfaces. Add rebase mechanics (new code path).

**Architecture:** New `MergeStyle` and `CleanupKind` enums in
`src/core/worktree/merge.rs` replace the old `FfMode` + `squash: bool` pair and
the `Args.remove + Args.and_branch` pair. The CLI exposes
booleans-per-style/outcome (mutually exclusive). Settings get two new keys
(`daft.merge.style`, `daft.merge.cleanup`) and lose four old keys. Rebase
mechanics introduce a new phase that runs `git rebase` in the source's worktree
(with ephemeral source adoption when needed) before falling through to FF or
merge-commit. `--set-default` writes via `git config --local`. Pre-merge /
post-merge hook semantics are already correct in code; this plan adds tests and
docs to lock the contract.

**Tech Stack:** Rust 1.x, clap (derive), `git` shell-out, `serial_test`, YAML
manual scenario harness (`tests/manual/scenarios/`), `mise` task runner.

**Spec:** `docs/superpowers/specs/2026-04-29-daft-merge-pr-style-design.md`

---

## Test setup conventions

Tests in `src/core/worktree/merge.rs::tests` follow these patterns — match them
when adapting the test code in this plan:

- `use std::process::Command as ShellCommand;` is the alias used for raw git
  invocations in tests. There is **no** `git.run(...)` or `git.run_capture(...)`
  method on `GitCommand` — the test patterns shell out via `ShellCommand`.
- `init_repo(tmp.path())` initializes a repo. It returns `()`. Default branch is
  **`main`** (not `master`); `init -b main` is hardcoded in the helper.
- Identity is set via env vars (`GIT_AUTHOR_NAME=Test`,
  `GIT_AUTHOR_EMAIL=test@test.com`, `GIT_COMMITTER_*` matching) on every commit
  invocation. The helper does this for the initial commit; subsequent commits in
  tests must also set these env vars or they'll fail in CI environments without
  git config.
- `CwdGuard::new()` (RAII) saves and restores cwd. Tests that mutate cwd MUST
  hold a `CwdGuard` to avoid breaking subsequent tests.
- `serial_test::serial` is required on tests that touch shared filesystem state
  or invoke commands that depend on cwd.
- The `GitCommand` constructor is `GitCommand::new(quiet: bool)`; tests
  typically pass `true` to suppress output.
- Captured-output assertions:
  `ShellCommand::new("git").args([...]).current_dir(path).output().unwrap().stdout`
  returns `Vec<u8>`; convert with
  `String::from_utf8_lossy(&out.stdout).trim().to_string()`.

**For each test in the slices below**, if a helper named in the test code (e.g.,
`add_unique_commit_to_branch`, `write_file_and_commit`) does not yet exist,
write it as a small private fn at the top of the new tests using the conventions
above. Example:

```rust
fn add_commit(path: &Path, branch: &str, file: &str, content: &str) -> String {
    ShellCommand::new("git")
        .args(["checkout", branch])
        .current_dir(path)
        .status()
        .unwrap();
    std::fs::write(path.join(file), content).unwrap();
    ShellCommand::new("git")
        .args(["add", file])
        .current_dir(path)
        .status()
        .unwrap();
    ShellCommand::new("git")
        .args(["commit", "-q", "-m", &format!("add {file}")])
        .current_dir(path)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .status()
        .unwrap();
    String::from_utf8(
        ShellCommand::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(path)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string()
}
```

Replace any plan test using `git.run(...)` or `git.run_capture(...)` with a
`ShellCommand` invocation following this shape. Replace any test using a
`init_repo` that returns `(git, _project_root)` with `init_repo(tmp.path())`
(returns unit) followed by a separate `let git = GitCommand::new(true);`.

Default branch in these tests is `main`, so any branch references in the plan
that assume `master` (e.g., the rebase test that does `git checkout master`)
should use `main` instead.

For settings tests in `src/core/settings.rs::tests`: locate the existing test
patterns there (search for `DaftSettings::load` or equivalent) and follow them.
The plan's settings tests assume a `DaftSettings::load_with_git(&git, path)` API
— if the actual API differs, adapt the call site (the assertion logic stays).

---

## Dependencies between slices

```
Slice 1 (types) ─┬─→ Slice 2 (settings) ─┐
                 │                        ├─→ Slice 4 (mechanics dispatch) ─→ Slice 5 (rebase)
                 └─→ Slice 3 (CLI flags) ─┘                                         │
                                                                                    ↓
                                                                            Slice 6 (set-default)
                                                                                    ↓
                                                                            Slice 7 (docs + YAML)
```

Slices 2 and 3 can run in parallel after 1, but most subagents will run them
sequentially. Each slice ends with a passing build (`mise run fmt`,
`mise run clippy`, `mise run test:unit`). Slice 7 adds the manual YAML coverage;
earlier slices add unit coverage only.

---

## Slice 1 — Foundation types

Add `MergeStyle` and `CleanupKind` enums with clap `ValueEnum` derivation,
`Display`, and conversion helpers. No field changes yet — these are purely
additive.

### Task 1.1: Add MergeStyle enum

**Files:**

- Modify: `src/core/worktree/merge.rs` — add the enum and helpers near the
  existing `FfMode` declaration (around line 220).

- [ ] **Step 1: Write the failing test** for `MergeStyle::as_str()` and
      `Display`.

Append to the existing `mod tests` block at the bottom of
`src/core/worktree/merge.rs`:

```rust
#[test]
fn merge_style_as_str_round_trips_value_enum() {
    use clap::ValueEnum;
    assert_eq!(MergeStyle::Merge.as_str(), "merge");
    assert_eq!(MergeStyle::Squash.as_str(), "squash");
    assert_eq!(MergeStyle::Rebase.as_str(), "rebase");
    assert_eq!(MergeStyle::RebaseMerge.as_str(), "rebase-merge");

    assert_eq!(
        MergeStyle::from_str("rebase-merge", true).unwrap(),
        MergeStyle::RebaseMerge
    );
    assert_eq!(
        MergeStyle::from_str("merge", true).unwrap(),
        MergeStyle::Merge
    );
    assert!(MergeStyle::from_str("bogus", true).is_err());
}

#[test]
fn merge_style_display_matches_as_str() {
    assert_eq!(format!("{}", MergeStyle::Merge), "merge");
    assert_eq!(format!("{}", MergeStyle::RebaseMerge), "rebase-merge");
}
```

- [ ] **Step 2: Run tests to verify they fail (compile error: undefined
      symbol)**

```
mise run test:unit 2>&1 | tail -20
```

Expected: compile failure with "cannot find type `MergeStyle`".

- [ ] **Step 3: Add the enum and impls** in `src/core/worktree/merge.rs` after
      the `FfMode` declaration (around line 224).

```rust
/// Named merge style. Selected via CLI flag (`--merge` / `--squash` /
/// `--rebase` / `--rebase-merge`) or `daft.merge.style` config key.
///
/// Maps to git mechanics:
/// * `Merge`       — `git merge --no-ff` (always creates a merge commit).
/// * `Squash`      — `git merge --squash` followed by `git commit`.
/// * `Rebase`      — `git rebase <target> <source>` followed by `git merge --ff-only`.
/// * `RebaseMerge` — `git rebase <target> <source>` followed by `git merge --no-ff`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, clap::ValueEnum, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
#[clap(rename_all = "kebab-case")]
pub enum MergeStyle {
    Merge,
    Squash,
    Rebase,
    RebaseMerge,
}

impl MergeStyle {
    pub fn as_str(&self) -> &'static str {
        match self {
            MergeStyle::Merge => "merge",
            MergeStyle::Squash => "squash",
            MergeStyle::Rebase => "rebase",
            MergeStyle::RebaseMerge => "rebase-merge",
        }
    }

    /// Returns true if this style produces a merge commit message and may
    /// invoke `$EDITOR`. Used for editor pause/resume gating.
    pub fn produces_merge_commit_message(&self) -> bool {
        matches!(self, MergeStyle::Merge | MergeStyle::Squash | MergeStyle::RebaseMerge)
    }

    /// Returns true if this style invokes the rebase phase (whether or not
    /// a merge commit follows). Used for finish-mode dispatch and source
    /// worktree adoption.
    pub fn uses_rebase(&self) -> bool {
        matches!(self, MergeStyle::Rebase | MergeStyle::RebaseMerge)
    }
}

impl std::fmt::Display for MergeStyle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```
mise run test:unit 2>&1 | grep -E "merge_style|test result" | tail -5
```

Expected: both tests pass.

- [ ] **Step 5: Run lint and format**

```
mise run fmt && mise run clippy 2>&1 | tail -5
```

Expected: zero warnings.

- [ ] **Step 6: Commit**

```bash
git add src/core/worktree/merge.rs
git commit -m "feat(merge): add MergeStyle enum"
```

### Task 1.2: Add CleanupKind enum

**Files:**

- Modify: `src/core/worktree/merge.rs` — add the enum near `MergeStyle`.

- [ ] **Step 1: Write the failing test**

Append to `mod tests`:

```rust
#[test]
fn cleanup_kind_as_str_round_trips_value_enum() {
    use clap::ValueEnum;
    assert_eq!(CleanupKind::Keep.as_str(), "keep");
    assert_eq!(CleanupKind::RemoveBranch.as_str(), "remove-branch");

    assert_eq!(
        CleanupKind::from_str("remove-branch", true).unwrap(),
        CleanupKind::RemoveBranch
    );
    assert!(CleanupKind::from_str("bogus", true).is_err());
}
```

- [ ] **Step 2: Verify failure**

```
mise run test:unit 2>&1 | tail -10
```

Expected: compile error "cannot find type `CleanupKind`".

- [ ] **Step 3: Add the enum** after `MergeStyle` in
      `src/core/worktree/merge.rs`.

```rust
/// What happens to the source after a successful merge.
///
/// `Keep`         — Source worktree and branch survive untouched (default).
/// `RemoveBranch` — Source worktree is removed AND source branch is deleted
///                  locally; the local/remote sync follows the existing
///                  `branch.deleteRemote` config (so `--remove-branch`
///                  with `branch.deleteRemote=true` also pushes
///                  `git push origin --delete <branch>`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, clap::ValueEnum, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
#[clap(rename_all = "kebab-case")]
pub enum CleanupKind {
    Keep,
    RemoveBranch,
}

impl CleanupKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            CleanupKind::Keep => "keep",
            CleanupKind::RemoveBranch => "remove-branch",
        }
    }
}

impl std::fmt::Display for CleanupKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
```

- [ ] **Step 4: Run tests** — both `MergeStyle` and `CleanupKind` round-trip.

```
mise run test:unit 2>&1 | grep -E "cleanup_kind|merge_style|test result" | tail -10
```

Expected: all pass.

- [ ] **Step 5: Lint + format**

```
mise run fmt && mise run clippy 2>&1 | tail -5
```

Expected: zero warnings.

- [ ] **Step 6: Commit**

```bash
git add src/core/worktree/merge.rs
git commit -m "feat(merge): add CleanupKind enum"
```

---

## Slice 2 — Settings cutover

Replace the four legacy keys (`merge.ff`, `merge.squash`,
`merge.postMerge.removeSourceWorktree`,
`merge.postMerge.alsoRemoveSourceBranch`) with two new keys (`merge.style`,
`merge.cleanup`). Update the `DaftSettings` struct, defaults module, key
constants, and both YAML and git-config loaders.

### Task 2.1: Add merge_style + merge_cleanup settings fields

**Files:**

- Modify: `src/core/settings.rs` — defaults module (line 148), keys module (line
  264), DaftSettings struct (line 441), Default impl (line 522), YAML loader
  (line 694), git-config loader (line 860).

- [ ] **Step 1: Write the failing test** — append to `src/core/settings.rs`
      `mod tests`:

```rust
#[test]
fn merge_style_default_is_merge() {
    let s = DaftSettings::default();
    assert_eq!(s.merge_style, crate::core::worktree::merge::MergeStyle::Merge);
}

#[test]
fn merge_cleanup_default_is_keep() {
    let s = DaftSettings::default();
    assert_eq!(s.merge_cleanup, crate::core::worktree::merge::CleanupKind::Keep);
}

#[test]
fn merge_style_loads_from_git_config() {
    let tmp = tempfile::tempdir().unwrap();
    let git = init_repo(tmp.path());
    git.run(&["config", "--local", "daft.merge.style", "rebase"])
        .unwrap();

    let s = DaftSettings::load_with_git(&git, tmp.path()).unwrap();
    assert_eq!(s.merge_style, crate::core::worktree::merge::MergeStyle::Rebase);
}

#[test]
fn merge_cleanup_loads_from_git_config() {
    let tmp = tempfile::tempdir().unwrap();
    let git = init_repo(tmp.path());
    git.run(&["config", "--local", "daft.merge.cleanup", "remove-branch"])
        .unwrap();

    let s = DaftSettings::load_with_git(&git, tmp.path()).unwrap();
    assert_eq!(
        s.merge_cleanup,
        crate::core::worktree::merge::CleanupKind::RemoveBranch
    );
}
```

If `init_repo` and `load_with_git` aren't already test helpers, locate the
equivalents — search `mod tests` in `src/core/settings.rs` for the patterns the
existing tests use (e.g., `setup_repo()`, `DaftSettings::load_from(...)`). Adapt
the test invocation to whatever the existing harness provides; the assertions
stay the same.

- [ ] **Step 2: Verify failure**

```
mise run test:unit 2>&1 | grep -E "merge_style|merge_cleanup|cannot find" | tail -10
```

Expected: compile error — `merge_style` and `merge_cleanup` not yet on
`DaftSettings`.

- [ ] **Step 3: Add the new defaults** in the `defaults` module
      (`src/core/settings.rs:147` area):

```rust
pub const MERGE_STYLE: crate::core::worktree::merge::MergeStyle =
    crate::core::worktree::merge::MergeStyle::Merge;
pub const MERGE_CLEANUP: crate::core::worktree::merge::CleanupKind =
    crate::core::worktree::merge::CleanupKind::Keep;
```

- [ ] **Step 4: Add the new key constants** in the `keys` module
      (`src/core/settings.rs:264` area):

```rust
pub const MERGE_STYLE: &str = "daft.merge.style";
pub const MERGE_CLEANUP: &str = "daft.merge.cleanup";
```

- [ ] **Step 5: Add the new struct fields** to `DaftSettings`
      (`src/core/settings.rs:441` area). Place them adjacent to the
      merge-related fields:

```rust
/// Selected merge style — replaces the legacy `merge_ff` + `merge_squash`
/// combination. See [`MergeStyle`] for variants.
pub merge_style: crate::core::worktree::merge::MergeStyle,
/// Selected post-merge cleanup outcome. See [`CleanupKind`] for variants.
pub merge_cleanup: crate::core::worktree::merge::CleanupKind,
```

- [ ] **Step 6: Wire defaults** in the `Default` impl (around line 522):

```rust
merge_style: defaults::MERGE_STYLE,
merge_cleanup: defaults::MERGE_CLEANUP,
```

- [ ] **Step 7: Wire the git-config loader** (around line 860). Add after the
      existing `MERGE_FF` loader block:

```rust
if let Some(value) = git.config_get(keys::MERGE_STYLE)? {
    settings.merge_style = match value.as_str() {
        "merge" => crate::core::worktree::merge::MergeStyle::Merge,
        "squash" => crate::core::worktree::merge::MergeStyle::Squash,
        "rebase" => crate::core::worktree::merge::MergeStyle::Rebase,
        "rebase-merge" => crate::core::worktree::merge::MergeStyle::RebaseMerge,
        _ => defaults::MERGE_STYLE,
    };
}
if let Some(value) = git.config_get(keys::MERGE_CLEANUP)? {
    settings.merge_cleanup = match value.as_str() {
        "keep" => crate::core::worktree::merge::CleanupKind::Keep,
        "remove-branch" => crate::core::worktree::merge::CleanupKind::RemoveBranch,
        _ => defaults::MERGE_CLEANUP,
    };
}
```

- [ ] **Step 8: Run tests to verify they pass**

```
mise run test:unit 2>&1 | grep -E "merge_style|merge_cleanup|test result" | tail -10
```

Expected: all four new tests pass.

- [ ] **Step 9: Lint + format**

```
mise run fmt && mise run clippy 2>&1 | tail -5
```

Expected: zero warnings.

- [ ] **Step 10: Commit**

```bash
git add src/core/settings.rs
git commit -m "feat(settings): add merge_style and merge_cleanup keys"
```

### Task 2.2: Remove legacy merge config keys

Now that the new keys land, delete the four legacy keys. Delete every reference
(defaults, keys, struct fields, Default impl, loaders, every test that asserts
the old fields). The build will be broken until
`effective_flags_from_args_and_settings` is updated in slice 3 — that's the
trade-off. Keep this task atomic so the broken intermediate state doesn't
persist past one commit.

**Files:**

- Modify: `src/core/settings.rs`

- [ ] **Step 1: Audit all references**

```
grep -n "merge_ff\|merge_squash\|MERGE_FF\|MERGE_SQUASH\|merge_post_merge_remove_source_worktree\|merge_post_merge_also_remove_source_branch\|MERGE_POST_MERGE_REMOVE_SOURCE_WORKTREE\|MERGE_POST_MERGE_ALSO_REMOVE_SOURCE_BRANCH" src/core/settings.rs
```

Note every line; you'll delete all of them.

- [ ] **Step 2: Delete the defaults**

In `src/core/settings.rs:148-178`, delete `MERGE_FF`, `MERGE_SQUASH`,
`MERGE_POST_MERGE_REMOVE_SOURCE_WORKTREE`,
`MERGE_POST_MERGE_ALSO_REMOVE_SOURCE_BRANCH` constants.

- [ ] **Step 3: Delete the key constants**

In `src/core/settings.rs:264-310`, delete `MERGE_FF`, `MERGE_SQUASH`,
`MERGE_POST_MERGE_REMOVE_SOURCE_WORKTREE`,
`MERGE_POST_MERGE_ALSO_REMOVE_SOURCE_BRANCH`.

- [ ] **Step 4: Delete the struct fields**

In `src/core/settings.rs:441-491` area, delete:

- `pub merge_ff: crate::core::worktree::merge::FfMode,`
- `pub merge_squash: bool,`
- `pub merge_post_merge_remove_source_worktree: bool,`
- `pub merge_post_merge_also_remove_source_branch: bool,`

- [ ] **Step 5: Delete the Default impl entries**

In the `Default for DaftSettings` impl, remove the four `merge_ff`,
`merge_squash`, `merge_post_merge_remove_source_worktree`,
`merge_post_merge_also_remove_source_branch` lines.

- [ ] **Step 6: Delete the YAML and git-config loader blocks**

Around line 694 (YAML loader) and 860 (git config loader), remove the loader
blocks for the four deleted keys.

- [ ] **Step 7: Delete tests asserting old fields**

```
grep -n "merge_ff\|merge_squash\|merge_post_merge" src/core/settings.rs
```

Each remaining match (most likely in `mod tests`) needs its surrounding test
deleted or rewritten.

- [ ] **Step 8: Build expected to fail at this point**

```
mise run clippy 2>&1 | tail -20
```

Expected: errors in `src/commands/merge.rs` referencing `args.no_ff`,
`args.squash`, `settings.merge_squash`, etc. Note them — slice 3 will fix.

- [ ] **Step 9: Don't commit yet** — leave staged so the next slice (CLI flags)
      can land in the same broken-state-to-fixed-state cycle. If the subagent
      prefers a coherent commit, defer the commit until the end of slice 3 task
      3.4 (where the build returns to green). Mark this task complete only when
      slice 3 also lands.

Status flag: subagent should NOT mark this task complete or commit. The commit
happens at the end of slice 3.

---

## Slice 3 — CLI flag surface cutover

Refactor the `Args` struct in `src/commands/merge.rs`: remove the legacy flags,
add the new ones, and rewrite `effective_flags_from_args_and_settings` to derive
`style: MergeStyle` and `cleanup: CleanupKind` from the new args plus settings.
Restore the build.

### Task 3.1: Remove legacy CLI flags from Args

**Files:**

- Modify: `src/commands/merge.rs:140-244` — Args struct field declarations.

- [ ] **Step 1: Locate and delete the legacy flag fields**

In `src/commands/merge.rs:143-244`, delete:

```rust
#[arg(long = "ff", conflicts_with_all = ["no_ff", "ff_only"])]
pub ff: bool,

#[arg(long = "no-ff", conflicts_with_all = ["ff", "ff_only"])]
pub no_ff: bool,

#[arg(long = "ff-only", conflicts_with_all = ["ff", "no_ff"])]
pub ff_only: bool,

#[arg(long = "squash", conflicts_with = "no_squash")]
pub squash: bool,

#[arg(long = "no-squash", conflicts_with = "squash")]
pub no_squash: bool,

#[arg(short = 'r', long = "remove")]
pub remove: bool,

#[arg(short = 'b', long = "and-branch", requires = "remove")]
pub and_branch: bool,
```

Also remove all references to these field names from the finish-mode
`conflicts_with_all` lists (search for `"ff"`, `"no_ff"`, `"ff_only"`,
`"squash"`, `"no_squash"`, `"remove"`, `"and_branch"` in `Args` and remove each
from the lists where they appear).

- [ ] **Step 2: Don't run tests yet** — the build is still broken; we'll
      reintroduce flags in 3.2.

### Task 3.2: Add new CLI flags

**Files:**

- Modify: `src/commands/merge.rs` — `Args` struct.

- [ ] **Step 1: Add the new style booleans** in the same general location where
      `--squash` used to live:

```rust
// --- Merge style (mutually exclusive; default = merge) ---

/// Explicit merge style — always create a merge commit. This is the default;
/// the flag exists for canceling a config-set default style.
#[arg(
    long = "merge",
    conflicts_with_all = ["squash", "rebase", "rebase_merge"],
)]
pub style_merge: bool,

/// Squash style — collapse source's commits into one squashed commit on target.
#[arg(
    long = "squash",
    conflicts_with_all = ["style_merge", "rebase", "rebase_merge"],
)]
pub squash: bool,

/// Rebase style — rebase source onto target, then fast-forward (linear, preserves commits).
#[arg(
    long = "rebase",
    conflicts_with_all = ["style_merge", "squash", "rebase_merge"],
)]
pub rebase: bool,

/// Rebase-merge style — rebase source onto target, then create a merge commit.
#[arg(
    long = "rebase-merge",
    conflicts_with_all = ["style_merge", "squash", "rebase"],
)]
pub rebase_merge: bool,

// --- Post-merge cleanup (start-mode only; mutually exclusive; default = keep) ---

/// Remove the source worktree and delete the source branch. The local/remote
/// behavior follows `branch.deleteRemote` (defaults to local-only).
#[arg(
    short = 'r',
    long = "remove-branch",
    conflicts_with = "keep_branch",
)]
pub remove_branch: bool,

/// Explicit keep — for canceling a config-set `merge.cleanup = remove-branch`.
#[arg(
    long = "keep-branch",
    conflicts_with = "remove_branch",
)]
pub keep_branch: bool,

// --- Defaults persistence ---

/// Write the resolved style/cleanup choices to `git config --local` after
/// the merge succeeds. Useful for promoting an invocation's preferences as
/// the new repo defaults.
#[arg(long = "set-default")]
pub set_default: bool,
```

- [ ] **Step 2: Update finish-mode `conflicts_with_all` lists** to reference the
      new style/cleanup flag names. In each of the three finish-mode flags
      (`--abort`, `--continue`, `--quit`), the `conflicts_with_all` list
      previously included old names like `"ff"`, `"squash"`, `"remove"`,
      `"and_branch"`. Replace those with `"style_merge"`, `"squash"`,
      `"rebase"`, `"rebase_merge"`, `"remove_branch"`, `"keep_branch"`,
      `"set_default"`.

Concrete example for the `--abort` block (apply the same shape to `--continue`
and `--quit`):

```rust
#[arg(
    long = "abort",
    conflicts_with_all = [
        "continue_merge", "quit",
        "message", "file", "edit", "no_edit", "cleanup",
        "style_merge", "squash", "rebase", "rebase_merge",
        "commit", "no_commit",
        "signoff", "no_signoff",
        "strategy", "strategy_options",
        "gpg_sign", "no_gpg_sign",
        "verify_signatures", "no_verify_signatures",
        "allow_unrelated_histories",
        "stat", "no_stat",
        "adopt_target", "no_adopt_target", "yes",
        "remove_branch", "keep_branch", "set_default",
    ],
)]
pub abort: bool,
```

For `--continue`, the commit-composing flags (`message`, `file`, `edit`,
`no_edit`, `cleanup`) are NOT in the conflicts list (they're forwarded to
`git commit` for squash-staged finish). The legacy `--continue` block already
handled this; preserve that pattern with the new flag names.

- [ ] **Step 3: Add per-flag conflict rules for rebase**

Append to the existing flag declarations (modify each in place):

For `-m / --message`, `-F / --file`, `--edit`, `--no-edit`, `--cleanup`,
`--commit`, `--no-commit`: add `conflicts_with_all = ["rebase"]` (or extend an
existing `conflicts_with` list to include `"rebase"`).

For `--allow-unrelated-histories`: add
`conflicts_with_all = ["rebase", "rebase_merge"]`.

Example for `-m`:

```rust
#[arg(short = 'm', value_name = "MSG", conflicts_with = "rebase")]
pub message: Option<String>,
```

Example for `--allow-unrelated-histories`:

```rust
#[arg(long = "allow-unrelated-histories", conflicts_with_all = ["rebase", "rebase_merge"])]
pub allow_unrelated_histories: bool,
```

- [ ] **Step 4: Build still broken — `effective_flags_from_args_and_settings`
      references the old field names.** Move to 3.3.

### Task 3.3: Refactor EffectiveFlags struct

**Files:**

- Modify: `src/core/worktree/merge.rs:240-265` — `EffectiveFlags` struct.

- [ ] **Step 1: Replace the `ff` and `squash` fields with `style`**

In `EffectiveFlags`:

```rust
#[derive(Debug, Clone)]
pub struct EffectiveFlags {
    pub message: Option<String>,
    pub file: Option<PathBuf>,
    pub edit: Option<bool>,
    pub cleanup: Option<String>,
    pub style: MergeStyle,
    pub commit: Option<bool>,
    pub signoff: Option<bool>,
    pub strategy: Option<String>,
    pub strategy_options: Vec<String>,
    pub gpg_sign: Option<GpgSign>,
    pub verify_signatures: Option<bool>,
    pub allow_unrelated_histories: bool,
    pub stat: Option<bool>,
}

impl Default for EffectiveFlags {
    fn default() -> Self {
        Self {
            message: None,
            file: None,
            edit: None,
            cleanup: None,
            style: MergeStyle::Merge,
            commit: None,
            signoff: None,
            strategy: None,
            strategy_options: Vec::new(),
            gpg_sign: None,
            verify_signatures: None,
            allow_unrelated_histories: false,
            stat: None,
        }
    }
}
```

(Rust's `derive(Default)` on enums needs `#[default]` annotations; explicit impl
is simpler here.)

- [ ] **Step 2: Update `squash_would_open_editor`** (line 280):

```rust
impl EffectiveFlags {
    /// Returns `true` when this merge style would need to open an editor
    /// to compose a commit message.
    pub fn would_open_editor(&self) -> bool {
        self.style.produces_merge_commit_message()
            && !matches!(self.commit, Some(false))
            && self.message.is_none()
            && self.file.is_none()
            && !matches!(self.edit, Some(false))
    }
}
```

Also rename callers from `squash_would_open_editor` to `would_open_editor`.
Search:

```
grep -n "squash_would_open_editor" src/
```

Replace each call site.

- [ ] **Step 3: Refactor `render_flags`** (line 307) for the new style enum.

```rust
pub fn render_flags(flags: &EffectiveFlags) -> Vec<String> {
    let is_squash = matches!(flags.style, MergeStyle::Squash);
    let mut out: Vec<String> = Vec::new();

    if !is_squash {
        if let Some(m) = &flags.message {
            out.extend(["-m".into(), m.clone()]);
        }
        if let Some(f) = &flags.file {
            out.extend(["-F".into(), f.display().to_string()]);
        }
        match flags.edit {
            Some(true) => out.push("--edit".into()),
            Some(false) => out.push("--no-edit".into()),
            None => {}
        }
        if let Some(c) = &flags.cleanup {
            out.extend(["--cleanup".into(), c.clone()]);
        }
    }

    // Style → ff/squash flags. Rebase styles are handled BEFORE this argv
    // (the rebase phase runs first); render_flags is only called for the
    // merge phase, where Rebase becomes --ff-only (post-rebase ff) and
    // RebaseMerge becomes --no-ff (post-rebase merge commit).
    match flags.style {
        MergeStyle::Merge => out.push("--no-ff".into()),
        MergeStyle::Squash => out.push("--squash".into()),
        MergeStyle::Rebase => out.push("--ff-only".into()),
        MergeStyle::RebaseMerge => out.push("--no-ff".into()),
    }

    match flags.commit {
        Some(true) => out.push("--commit".into()),
        Some(false) => out.push("--no-commit".into()),
        None => {}
    }

    if !is_squash {
        match flags.signoff {
            Some(true) => out.push("--signoff".into()),
            Some(false) => out.push("--no-signoff".into()),
            None => {}
        }
    }

    if let Some(s) = &flags.strategy {
        out.extend(["-s".into(), s.clone()]);
    }
    for x in &flags.strategy_options {
        out.extend(["-X".into(), x.clone()]);
    }

    if !is_squash {
        match &flags.gpg_sign {
            Some(GpgSign::Default) => out.push("-S".into()),
            Some(GpgSign::KeyId(k)) => out.push(format!("-S{k}")),
            Some(GpgSign::Disabled) => out.push("--no-gpg-sign".into()),
            None => {}
        }
    }

    match flags.verify_signatures {
        Some(true) => out.push("--verify-signatures".into()),
        Some(false) => out.push("--no-verify-signatures".into()),
        None => {}
    }

    if flags.allow_unrelated_histories {
        out.push("--allow-unrelated-histories".into());
    }

    match flags.stat {
        Some(true) => out.push("--stat".into()),
        Some(false) => out.push("--no-stat".into()),
        None => {}
    }

    out
}
```

- [ ] **Step 4: Delete the now-unused `FfMode` enum** at line 220 if no other
      code references it.

```
grep -n "FfMode" src/
```

If the only remaining hits are in the merge.rs declaration site itself and dead
callers in tests, delete the enum and update tests. If anything outside
`src/core/worktree/merge.rs` still references it, leave the enum and add a
`#[deprecated]` note (this is unlikely; the enum was added for the old slice).

- [ ] **Step 5: Update `render_flags` tests**

Find existing render_flags tests
(`grep -n "fn render_flags\|render_flags(" src/core/worktree/merge.rs`). Rewrite
the test bodies to use the new `style: MergeStyle` field instead of `ff:` and
`squash:`. Concrete shape for one assertion:

```rust
#[test]
fn render_flags_merge_emits_no_ff() {
    let flags = EffectiveFlags {
        style: MergeStyle::Merge,
        ..EffectiveFlags::default()
    };
    let argv = render_flags(&flags);
    assert!(argv.contains(&"--no-ff".to_string()));
    assert!(!argv.contains(&"--squash".to_string()));
}

#[test]
fn render_flags_squash_emits_squash() {
    let flags = EffectiveFlags {
        style: MergeStyle::Squash,
        ..EffectiveFlags::default()
    };
    let argv = render_flags(&flags);
    assert!(argv.contains(&"--squash".to_string()));
    assert!(!argv.contains(&"--no-ff".to_string()));
}

#[test]
fn render_flags_rebase_emits_ff_only() {
    let flags = EffectiveFlags {
        style: MergeStyle::Rebase,
        ..EffectiveFlags::default()
    };
    let argv = render_flags(&flags);
    assert!(argv.contains(&"--ff-only".to_string()));
}

#[test]
fn render_flags_rebase_merge_emits_no_ff() {
    let flags = EffectiveFlags {
        style: MergeStyle::RebaseMerge,
        ..EffectiveFlags::default()
    };
    let argv = render_flags(&flags);
    assert!(argv.contains(&"--no-ff".to_string()));
}
```

Replace any existing `render_flags_*_squash_*` tests that asserted on the old
`squash: Option<bool>` shape with style-based equivalents.

### Task 3.4: Rewrite effective_flags_from_args_and_settings

**Files:**

- Modify: `src/commands/merge.rs:267-404` — the resolver.

- [ ] **Step 1: Replace the body** with the new style/cleanup resolution:

```rust
fn effective_flags_from_args_and_settings(
    args: &Args,
    settings: &DaftSettings,
) -> crate::core::worktree::merge::EffectiveFlags {
    use crate::core::worktree::merge::{EffectiveFlags, GpgSign, MergeStyle};

    // style: CLI wins over settings; default = MergeStyle::Merge.
    // The four CLI booleans are clap-enforced mutually exclusive, so at most
    // one is true. None set → fall back to settings, which defaults to Merge.
    let style = if args.style_merge {
        MergeStyle::Merge
    } else if args.squash {
        MergeStyle::Squash
    } else if args.rebase {
        MergeStyle::Rebase
    } else if args.rebase_merge {
        MergeStyle::RebaseMerge
    } else {
        settings.merge_style
    };

    // commit: CLI wins; else only emit when settings overrides to false.
    let commit = if args.commit {
        Some(true)
    } else if args.no_commit || !settings.merge_commit {
        Some(false)
    } else {
        None
    };

    // edit: CLI wins; -y/--yes implies --no-edit; settings provides Option<bool>.
    let edit = if args.edit {
        Some(true)
    } else if args.no_edit || args.yes {
        Some(false)
    } else {
        settings.merge_edit
    };

    let signoff = if args.signoff {
        Some(true)
    } else if args.no_signoff {
        Some(false)
    } else if settings.merge_signoff {
        Some(true)
    } else {
        None
    };

    let gpg_sign = if args.no_gpg_sign {
        Some(GpgSign::Disabled)
    } else if let Some(k) = &args.gpg_sign {
        if k.is_empty() {
            Some(GpgSign::Default)
        } else {
            Some(GpgSign::KeyId(k.clone()))
        }
    } else if let Some(k) = &settings.merge_gpg_sign {
        if k.is_empty() {
            Some(GpgSign::Default)
        } else {
            Some(GpgSign::KeyId(k.clone()))
        }
    } else {
        None
    };

    let verify_signatures = if args.verify_signatures {
        Some(true)
    } else if args.no_verify_signatures {
        Some(false)
    } else if settings.merge_verify_signatures {
        Some(true)
    } else {
        None
    };

    let stat = if args.stat {
        Some(true)
    } else if args.no_stat {
        Some(false)
    } else {
        None
    };

    let strategy = args
        .strategy
        .clone()
        .or_else(|| settings.merge_strategy.clone());

    let mut strategy_options = settings.merge_strategy_options.clone();
    strategy_options.extend(args.strategy_options.iter().cloned());

    let allow_unrelated_histories =
        args.allow_unrelated_histories || settings.merge_allow_unrelated_histories;

    EffectiveFlags {
        message: args.message.clone(),
        file: args.file.clone(),
        edit,
        cleanup: args.cleanup.clone(),
        style,
        commit,
        signoff,
        strategy,
        strategy_options,
        gpg_sign,
        verify_signatures,
        allow_unrelated_histories,
        stat,
    }
}
```

- [ ] **Step 2: Add a helper to derive CleanupKind from Args + Settings**

In `src/commands/merge.rs` near the resolver:

```rust
/// Resolve the post-merge cleanup outcome from CLI flags and settings.
/// CLI flags win; clap's mutual exclusion guarantees at most one is true.
fn effective_cleanup_from_args_and_settings(
    args: &Args,
    settings: &DaftSettings,
) -> crate::core::worktree::merge::CleanupKind {
    use crate::core::worktree::merge::CleanupKind;
    if args.remove_branch {
        CleanupKind::RemoveBranch
    } else if args.keep_branch {
        CleanupKind::Keep
    } else {
        settings.merge_cleanup
    }
}
```

- [ ] **Step 3: Update the cleanup wiring in `run()`**

Find the existing cleanup-vs-no-commit guard (`src/commands/merge.rs:520-545`
area) and the cleanup invocation site that uses
`args.remove`/`args.and_branch`/`settings.merge_post_merge_*`. Replace with:

```rust
let cleanup_kind = effective_cleanup_from_args_and_settings(&args, &settings);

// Pre-flight cleanup-vs-no-commit guard. Cleanup requires a committed merge.
if matches!(flags.commit, Some(false))
    && cleanup_kind == crate::core::worktree::merge::CleanupKind::RemoveBranch
{
    anyhow::bail!(
        "--no-commit / daft.merge.commit=false is incompatible with cleanup \
         (--remove-branch / daft.merge.cleanup=remove-branch); cleanup requires a committed merge."
    );
}
```

Then locate the post-merge cleanup loop (`src/commands/merge.rs:806-880` area,
where `plan_cleanup` is called and per-item `branch_delete::execute` runs).
Replace the entry-condition guard from the old
`effective_remove || effective_and_branch` style with
`cleanup_kind == CleanupKind::RemoveBranch`. Inside the loop, remove the
per-item `force_delete` reading from `CleanupItem` if it was branching on the
old flags; the new code passes uniform `force=true` and
`delete_remote=settings.branch_delete_remote`.

Concrete shape for the cleanup invocation:

```rust
if cleanup_kind == crate::core::worktree::merge::CleanupKind::RemoveBranch {
    // [existing plan_cleanup invocation produces `plan: Vec<CleanupItem>`]
    for item in plan {
        let bd_params = crate::core::worktree::branch_delete::BranchDeleteParams {
            // ... existing fields (sources, project_root, etc.) ...
            delete_remote: settings.branch_delete_remote,
            keep_local_branch: item.branch_name.is_none(),
            force: true,
            command_label: "merge".to_string(),
            // ... whatever else BranchDeleteParams requires ...
        };
        crate::core::worktree::branch_delete::execute(&bd_params, &mut bridge)?;
    }
}
```

- [ ] **Step 4: Run the full build to verify the cutover landed**

```
mise run clippy 2>&1 | tail -30
```

Expected: zero errors, zero warnings.

- [ ] **Step 5: Run the full test suite**

```
mise run test:unit 2>&1 | grep -E "test result|FAIL" | tail -10
```

Expected: all tests pass.

- [ ] **Step 6: Run formatting**

```
mise run fmt
```

Expected: no changes (or only whitespace).

- [ ] **Step 7: Commit slices 2 + 3 together**

The build broke in slice 2 task 2.2 and is now restored. Commit the combined
cutover as a single feat commit:

```bash
git add -A
git commit -m "feat(merge)!: cutover to MergeStyle/CleanupKind on settings + CLI

Replaces merge_ff, merge_squash, merge_post_merge_remove_source_worktree,
merge_post_merge_also_remove_source_branch with merge_style and merge_cleanup.
CLI: removes --ff, --no-ff, --ff-only, --squash, --no-squash, --remove,
--and-branch; adds --merge, --squash, --rebase, --rebase-merge,
--remove-branch, --keep-branch, --set-default. EffectiveFlags.style replaces
ff/squash. render_flags maps each style to its argv shape. No backwards
compatibility (pre-1.0 surface)."
```

Note: the `feat(merge)!` exclamation marks the breaking change in the commit
subject for changelog tooling, even though we skip a separate `BREAKING CHANGE:`
body line per the project's pre-1.0 policy.

---

## Slice 4 — Mechanics dispatch (Merge + Squash)

`MergeStyle::Merge` and `MergeStyle::Squash` already work via `render_flags`.
This slice verifies the default-behavior change (no flag → `--no-ff`, always
merge commit) lands correctly across the existing in-worktree and ref-only
paths, and adds regression tests.

### Task 4.1: Verify Merge style default behavior with isolated repo test

**Files:**

- Modify: `src/core/worktree/merge.rs` — append to `mod tests`.

- [ ] **Step 1: Find the test helpers**

```
grep -n "fn init_repo\|fn setup_worktree\|fn add_unique_commit" src/core/worktree/merge.rs | head -10
```

Note the helper names and signatures. Use them in the test below; substitute the
actual names if different.

- [ ] **Step 2: Write a passing-by-construction integration-style unit test**

```rust
#[test]
#[serial_test::serial]
fn default_merge_style_creates_merge_commit() {
    use crate::core::worktree::merge::{
        execute_start, EffectiveFlags, MergeStyle, NullHookRunner, StartParams,
    };

    let tmp = tempfile::tempdir().unwrap();
    let (git, _project_root) = init_repo(tmp.path());

    // Set up: master with one commit, feat branch with one extra commit.
    git.run(&["checkout", "-b", "feat"]).unwrap();
    add_unique_commit_to_branch(&git, "feat");
    git.run(&["checkout", "master"]).unwrap();

    let flags = EffectiveFlags {
        style: MergeStyle::Merge,
        ..EffectiveFlags::default()
    };
    let params = StartParams {
        sources: vec!["feat".to_string()],
        target: None,
        flags,
        ..StartParams::default()
    };

    let outcome = execute_start(&params, &git, tmp.path(), &mut NullHookRunner).unwrap();
    assert!(!outcome.failed);
    assert!(!outcome.already_up_to_date);

    // Verify HEAD has 2 parents (the merge commit landed even though FF was possible).
    let parents = git
        .run_capture(&["rev-list", "--parents", "-n", "1", "HEAD"])
        .unwrap();
    let parent_count = parents.trim().split_whitespace().count() - 1; // first token is HEAD itself
    assert_eq!(parent_count, 2, "default style should produce a merge commit, not FF");
}
```

If `add_unique_commit_to_branch` doesn't exist, use whatever helper makes a
commit on the named branch (search for similar patterns earlier in `mod tests`).

- [ ] **Step 3: Run the test**

```
mise run test:unit -- default_merge_style_creates_merge_commit 2>&1 | tail -10
```

Expected: PASS (the implementation already handles this via `render_flags` from
slice 3).

- [ ] **Step 4: Lint + format**

```
mise run fmt && mise run clippy 2>&1 | tail -5
```

Expected: zero warnings.

- [ ] **Step 5: Commit**

```bash
git add src/core/worktree/merge.rs
git commit -m "test(merge): pin default style to always-merge-commit"
```

### Task 4.2: Verify Squash style produces single-commit history

**Files:**

- Modify: `src/core/worktree/merge.rs` — append to `mod tests`.

- [ ] **Step 1: Write the test**

```rust
#[test]
#[serial_test::serial]
fn squash_style_produces_single_commit_on_target() {
    use crate::core::worktree::merge::{
        execute_start, EffectiveFlags, MergeStyle, NullHookRunner, StartParams,
    };

    let tmp = tempfile::tempdir().unwrap();
    let (git, _project_root) = init_repo(tmp.path());

    git.run(&["checkout", "-b", "feat"]).unwrap();
    add_unique_commit_to_branch(&git, "feat");
    add_unique_commit_to_branch(&git, "feat");
    git.run(&["checkout", "master"]).unwrap();

    let flags = EffectiveFlags {
        style: MergeStyle::Squash,
        edit: Some(false),                             // skip $EDITOR
        message: Some("Squashed feat".to_string()),
        ..EffectiveFlags::default()
    };
    let params = StartParams {
        sources: vec!["feat".to_string()],
        target: None,
        flags,
        ..StartParams::default()
    };

    let outcome = execute_start(&params, &git, tmp.path(), &mut NullHookRunner).unwrap();
    assert!(!outcome.failed);

    // Squash produces a single commit with one parent (target's previous tip).
    let parents = git
        .run_capture(&["rev-list", "--parents", "-n", "1", "HEAD"])
        .unwrap();
    let parent_count = parents.trim().split_whitespace().count() - 1;
    assert_eq!(parent_count, 1, "squash should produce a single-parent commit");
}
```

- [ ] **Step 2: Run the test**

```
mise run test:unit -- squash_style_produces_single_commit_on_target 2>&1 | tail -10
```

Expected: PASS.

- [ ] **Step 3: Lint + format + commit**

```
mise run fmt && mise run clippy
git add src/core/worktree/merge.rs
git commit -m "test(merge): pin squash style to single-parent result"
```

---

## Slice 5 — Rebase mechanics

This is the largest slice. Adds a rebase phase that runs before the merge phase
for `Rebase` and `RebaseMerge` styles. Adds finish-mode dispatch via
`detect_in_progress_state` to handle `--continue/--abort/--quit` correctly when
a rebase (rather than a merge) is in progress.

### Task 5.1: Add detect_in_progress_state helper

**Files:**

- Modify: `src/core/worktree/merge.rs` — add new function.

- [ ] **Step 1: Write the failing test**

Append to `mod tests`:

```rust
#[test]
fn detect_state_returns_none_for_clean_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".git")).unwrap();

    let state = detect_in_progress_state(tmp.path());
    assert!(state.is_none());
}

#[test]
fn detect_state_returns_merge_for_merge_head() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
    std::fs::write(tmp.path().join(".git/MERGE_HEAD"), "deadbeef").unwrap();

    assert_eq!(detect_in_progress_state(tmp.path()), Some(InProgressState::Merge));
}

#[test]
fn detect_state_returns_rebase_for_rebase_merge_dir() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".git/rebase-merge")).unwrap();

    assert_eq!(
        detect_in_progress_state(tmp.path()),
        Some(InProgressState::Rebase)
    );
}

#[test]
fn detect_state_returns_rebase_for_rebase_apply_dir() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".git/rebase-apply")).unwrap();

    assert_eq!(
        detect_in_progress_state(tmp.path()),
        Some(InProgressState::Rebase)
    );
}
```

- [ ] **Step 2: Verify failure** — `InProgressState` and
      `detect_in_progress_state` don't exist yet.

```
mise run test:unit 2>&1 | grep "detect_state\|cannot find" | tail -5
```

Expected: compile errors.

- [ ] **Step 3: Add the type and function** in `src/core/worktree/merge.rs` near
      other state-detection helpers (search for `detect_in_progress` to find the
      existing merge-state detector):

```rust
/// On-disk state of an in-progress merge or rebase, used to dispatch
/// finish-mode commands (`--continue` / `--abort` / `--quit`) to the right
/// git subcommand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InProgressState {
    Merge,
    Rebase,
}

/// Detect whether a merge or rebase is in progress in the worktree at `path`.
///
/// Looks for git's standard state files. Returns `None` if neither is found.
pub fn detect_in_progress_state(path: &Path) -> Option<InProgressState> {
    let git_dir = path.join(".git");
    if git_dir.join("MERGE_HEAD").exists() {
        return Some(InProgressState::Merge);
    }
    if git_dir.join("rebase-merge").is_dir() || git_dir.join("rebase-apply").is_dir() {
        return Some(InProgressState::Rebase);
    }
    None
}
```

Note: this is a coarser variant of the existing `detect_in_progress` (which the
codebase uses to bail when a merge is already mid-flight). Keep both — the
existing one returns a richer description; this one is tailored for finish-mode
dispatch.

- [ ] **Step 4: Run tests**

```
mise run test:unit -- detect_state 2>&1 | tail -10
```

Expected: all four tests pass.

- [ ] **Step 5: Lint + format + commit**

```
mise run fmt && mise run clippy
git add src/core/worktree/merge.rs
git commit -m "feat(merge): add detect_in_progress_state for finish-mode dispatch"
```

### Task 5.2: Implement Rebase phase in execute_start

**Files:**

- Modify: `src/core/worktree/merge.rs:1121` — `execute_start` and downstream
  `execute_start_in_worktree`.

- [ ] **Step 1: Write the failing test**

Append to `mod tests`:

```rust
#[test]
#[serial_test::serial]
fn rebase_style_produces_linear_history() {
    use crate::core::worktree::merge::{
        execute_start, EffectiveFlags, MergeStyle, NullHookRunner, StartParams,
    };

    let tmp = tempfile::tempdir().unwrap();
    let (git, _project_root) = init_repo(tmp.path());

    git.run(&["checkout", "-b", "feat"]).unwrap();
    let feat_sha = add_unique_commit_to_branch(&git, "feat");
    git.run(&["checkout", "master"]).unwrap();
    add_unique_commit_to_branch(&git, "master"); // make target diverge so FF is impossible without rebase

    let flags = EffectiveFlags {
        style: MergeStyle::Rebase,
        ..EffectiveFlags::default()
    };
    let params = StartParams {
        sources: vec!["feat".to_string()],
        target: None,
        flags,
        ..StartParams::default()
    };

    let outcome = execute_start(&params, &git, tmp.path(), &mut NullHookRunner).unwrap();
    assert!(!outcome.failed, "rebase merge should succeed: {outcome:?}");

    // Linear history: HEAD has one parent.
    let parents = git
        .run_capture(&["rev-list", "--parents", "-n", "1", "HEAD"])
        .unwrap();
    let parent_count = parents.trim().split_whitespace().count() - 1;
    assert_eq!(parent_count, 1, "rebase style should produce linear history");

    // The original feat SHA is NOT on master (it was rebased; the new commit has a different SHA).
    let log = git.run_capture(&["log", "--pretty=%H"]).unwrap();
    assert!(!log.contains(&feat_sha), "rebased commits get new SHAs");
}
```

- [ ] **Step 2: Verify failure**

```
mise run test:unit -- rebase_style_produces_linear_history 2>&1 | tail -15
```

Expected: failure — current `execute_start_in_worktree` invokes `git merge`
directly with the rendered flags, and `--ff-only` against a divergent feat will
fail.

- [ ] **Step 3: Add a rebase phase to `execute_start_in_worktree`**

In `src/core/worktree/merge.rs:1263` area, modify `execute_start_in_worktree` to
invoke the rebase phase before `git merge` for rebase styles. Add this near the
start of the function, after the pre-merge hook fires and before the
`argv = vec!["merge"...]` assembly:

```rust
fn execute_start_in_worktree(
    params: &StartParams,
    resolved: &ResolvedTarget,
    path: PathBuf,
    cross_worktree: bool,
    source_shas: &[String],
    hooks: &mut dyn HookRunner,
) -> Result<StartOutcome> {
    let pre_ctx = MergeHookContext::for_pre_with_shas(
        &params.sources,
        resolved,
        &params.flags,
        false,
        cross_worktree,
        source_shas,
    );
    hooks.fire_pre_merge(&pre_ctx)?;

    // Rebase phase: for Rebase / RebaseMerge styles, replay source's commits
    // onto target's tip before invoking the merge phase. Single-source only;
    // multi-source rebase is rejected up front.
    if params.flags.style.uses_rebase() {
        if params.sources.len() != 1 {
            anyhow::bail!(
                "rebase styles ({}) require exactly one source; got {}",
                params.flags.style,
                params.sources.len()
            );
        }
        let source = &params.sources[0];
        run_rebase_phase(source, &resolved.branch, &path).with_context(|| {
            format!(
                "rebase phase failed for source '{}' onto target '{}'",
                source, resolved.branch
            )
        })?;
    }

    // Merge phase: assemble argv from render_flags. For rebase styles,
    // render_flags emits --ff-only or --no-ff, which works because the
    // rebase phase advanced source's tip to be reachable from target's tip
    // (Rebase) or kept it as a separate branch tip (RebaseMerge).
    let mut argv: Vec<String> = vec!["merge".to_string()];
    argv.extend(render_flags(&params.flags));
    argv.extend(params.sources.iter().cloned());

    let merge_result = Command::new("git")
        .args(&argv)
        .current_dir(&path)
        .output()
        .with_context(|| format!("failed to invoke `git merge` in '{}'", path.display()))?;
    // ... existing capture / failure-handling logic continues unchanged ...
```

Then add the helper near other private helpers in the file:

```rust
/// Run `git rebase <target> <source>` in `target_path`. The source branch's
/// ref is updated in-place (its tip points at the rebased commits after
/// success). Conflicts leave the rebase in progress; the caller's finish-mode
/// path resumes via `detect_in_progress_state`.
fn run_rebase_phase(source: &str, target: &str, target_path: &Path) -> Result<()> {
    // Run the rebase from the target's worktree. `git rebase <upstream> <branch>`
    // checks out <branch>, replays its commits on top of <upstream>, and updates
    // the branch ref. The current HEAD shifts during rebase; the merge phase
    // restores target's HEAD via `git checkout` (handled by execute_start_in_worktree
    // when control returns there — the post-rebase `git merge` is invoked from
    // target's worktree, so we need to switch back).
    let status = Command::new("git")
        .args(["rebase", target, source])
        .current_dir(target_path)
        .status()
        .with_context(|| format!("failed to invoke `git rebase {} {}`", target, source))?;
    if !status.success() {
        anyhow::bail!(
            "git rebase {} {} failed; resolve conflicts then run `daft merge --continue`",
            target,
            source
        );
    }
    // Restore target's HEAD so the subsequent `git merge` runs from the right ref.
    let restore = Command::new("git")
        .args(["checkout", target])
        .current_dir(target_path)
        .status()
        .with_context(|| format!("failed to checkout target '{}' after rebase", target))?;
    if !restore.success() {
        anyhow::bail!("failed to checkout target '{}' after rebase phase", target);
    }
    Ok(())
}
```

- [ ] **Step 4: Run the test**

```
mise run test:unit -- rebase_style_produces_linear_history 2>&1 | tail -15
```

Expected: PASS.

- [ ] **Step 5: Add a RebaseMerge variant test**

```rust
#[test]
#[serial_test::serial]
fn rebase_merge_style_produces_merge_commit_after_rebase() {
    use crate::core::worktree::merge::{
        execute_start, EffectiveFlags, MergeStyle, NullHookRunner, StartParams,
    };

    let tmp = tempfile::tempdir().unwrap();
    let (git, _project_root) = init_repo(tmp.path());

    git.run(&["checkout", "-b", "feat"]).unwrap();
    add_unique_commit_to_branch(&git, "feat");
    git.run(&["checkout", "master"]).unwrap();
    add_unique_commit_to_branch(&git, "master");

    let flags = EffectiveFlags {
        style: MergeStyle::RebaseMerge,
        edit: Some(false),
        message: Some("Merged after rebase".into()),
        ..EffectiveFlags::default()
    };
    let params = StartParams {
        sources: vec!["feat".to_string()],
        target: None,
        flags,
        ..StartParams::default()
    };

    let outcome = execute_start(&params, &git, tmp.path(), &mut NullHookRunner).unwrap();
    assert!(!outcome.failed);

    let parents = git
        .run_capture(&["rev-list", "--parents", "-n", "1", "HEAD"])
        .unwrap();
    let parent_count = parents.trim().split_whitespace().count() - 1;
    assert_eq!(parent_count, 2, "rebase-merge produces a merge commit");
}
```

- [ ] **Step 6: Run both rebase tests**

```
mise run test:unit -- rebase_ 2>&1 | tail -10
```

Expected: both PASS.

- [ ] **Step 7: Lint + format + commit**

```
mise run fmt && mise run clippy
git add src/core/worktree/merge.rs
git commit -m "feat(merge): add rebase phase for Rebase and RebaseMerge styles"
```

### Task 5.3: Wire finish-mode dispatch via detect_in_progress_state

**Files:**

- Modify: `src/core/worktree/merge.rs:2313` — `execute_finish`.

- [ ] **Step 1: Write the failing test**

Append to `mod tests`:

```rust
#[test]
#[serial_test::serial]
fn finish_mode_continue_resumes_rebase() {
    use crate::core::worktree::merge::{
        execute_finish, FinishMode, NullHookRunner, EffectiveFlags, MergeStyle,
    };

    let tmp = tempfile::tempdir().unwrap();
    let (git, _project_root) = init_repo(tmp.path());

    // Set up a conflicting rebase so we can test --continue resumption.
    git.run(&["checkout", "-b", "feat"]).unwrap();
    write_file_and_commit(&git, "x.txt", "from feat", "feat change");
    git.run(&["checkout", "master"]).unwrap();
    write_file_and_commit(&git, "x.txt", "from master", "master change");

    // Begin a rebase that will conflict.
    let _ = git.run(&["rebase", "master", "feat"]); // expected to fail with conflict

    // Resolve the conflict.
    std::fs::write(tmp.path().join("x.txt"), "resolved").unwrap();
    git.run(&["add", "x.txt"]).unwrap();

    // Now invoke daft's finish-mode --continue with rebase-aware dispatch.
    let result = execute_finish(
        FinishMode::Continue,
        None,
        &EffectiveFlags::default(),
        &git,
        tmp.path(),
        &mut NullHookRunner,
    );
    assert!(result.is_ok(), "finish --continue should resume rebase: {result:?}");

    // After successful resumption, no rebase state should remain.
    assert!(!tmp.path().join(".git/rebase-merge").exists());
    assert!(!tmp.path().join(".git/rebase-apply").exists());
}
```

If `write_file_and_commit` doesn't exist as a helper, write a small inline
alternative or locate the analogous helper.

- [ ] **Step 2: Verify failure**

```
mise run test:unit -- finish_mode_continue_resumes_rebase 2>&1 | tail -15
```

Expected: failure — current `execute_finish` runs `git merge --continue`
regardless of state.

- [ ] **Step 3: Refactor `execute_finish` to dispatch**

Locate `execute_finish` (line ~2313). Add a state-detection branch at the top of
the function. Concrete shape:

```rust
pub fn execute_finish(
    mode: FinishMode,
    target: Option<&str>,
    flags: &EffectiveFlags,
    git: &GitCommand,
    project_root: &Path,
    hooks: &mut dyn HookRunner,
) -> Result<()> {
    // Resolve worktree path for the target (existing logic stays).
    let path = resolve_finish_target_path(target, git, project_root)?;

    // Dispatch based on what's actually in progress.
    match detect_in_progress_state(&path) {
        Some(InProgressState::Rebase) => {
            execute_finish_rebase(mode, &path)
        }
        Some(InProgressState::Merge) | None => {
            // None falls through to the existing merge finish path; if
            // nothing is in progress, that path will surface a clear error.
            execute_finish_merge(mode, target, flags, git, project_root, hooks)
        }
    }
}
```

Wrap the existing `execute_finish` body in a private `execute_finish_merge`
function (rename in place), then add the rebase branch:

```rust
fn execute_finish_rebase(mode: FinishMode, path: &Path) -> Result<()> {
    let subcommand = match mode {
        FinishMode::Continue => "--continue",
        FinishMode::Abort => "--abort",
        FinishMode::Quit => "--quit",
    };
    let status = Command::new("git")
        .args(["rebase", subcommand])
        .current_dir(path)
        .status()
        .with_context(|| format!("failed to invoke `git rebase {subcommand}`"))?;
    if !status.success() {
        anyhow::bail!("git rebase {subcommand} failed");
    }
    Ok(())
}
```

If the existing `execute_finish` doesn't have a path-resolution helper that's
standalone, inline its prelude into the dispatch level.

- [ ] **Step 4: Run the rebase finish test**

```
mise run test:unit -- finish_mode_continue_resumes_rebase 2>&1 | tail -10
```

Expected: PASS.

- [ ] **Step 5: Run the full test suite to catch regressions**

```
mise run test:unit 2>&1 | grep -E "test result|FAIL" | tail -10
```

Expected: all pass; no regressions to existing finish-mode merge tests.

- [ ] **Step 6: Lint + format + commit**

```
mise run fmt && mise run clippy
git add src/core/worktree/merge.rs
git commit -m "feat(merge): dispatch finish-mode via detect_in_progress_state"
```

---

## Slice 6 — `--set-default` writer + output rendering

Add a small writer that issues `git config --local daft.merge.style <v>` and
`git config --local daft.merge.cleanup <v>`. Wire it into `run()` between the
merge step and the cleanup phase. Add an `Output` method to render the "Updated
repository defaults" notice in a discrete style.

### Task 6.1: Add write_default_settings function

**Files:**

- Create: `src/core/worktree/merge_set_default.rs` (small, focused module).
- Modify: `src/core/worktree/mod.rs` — add the new module.

- [ ] **Step 1: Write the failing test**

Create `src/core/worktree/merge_set_default.rs`:

```rust
//! Writes daft's merge.style and merge.cleanup defaults to git config --local.
//!
//! Used by the `--set-default` flag on `daft merge` to promote the current
//! invocation's preferences as the new repo defaults. Best-effort: failures
//! surface as warnings to the caller; the merge result is unaffected.

use crate::core::worktree::merge::{CleanupKind, MergeStyle};
use crate::git::GitCommand;
use anyhow::{Context, Result};
use std::path::Path;

/// Write `daft.merge.style` and `daft.merge.cleanup` to git config --local
/// in the repo containing `project_root`. Both keys are always written
/// (idempotent).
pub fn write_default_settings(
    git: &GitCommand,
    project_root: &Path,
    style: MergeStyle,
    cleanup: CleanupKind,
) -> Result<()> {
    let _ = project_root; // reserved for future per-worktree config; today --local is enough
    git.run(&["config", "--local", "daft.merge.style", style.as_str()])
        .with_context(|| "failed to write daft.merge.style")?;
    git.run(&["config", "--local", "daft.merge.cleanup", cleanup.as_str()])
        .with_context(|| "failed to write daft.merge.cleanup")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::worktree::merge::{CleanupKind, MergeStyle};

    fn init_repo(path: &Path) -> GitCommand {
        let git = GitCommand::new(false);
        std::process::Command::new("git")
            .args(["init", "--initial-branch=master"])
            .current_dir(path)
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "--local", "user.name", "Test"])
            .current_dir(path)
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "--local", "user.email", "test@test.com"])
            .current_dir(path)
            .status()
            .unwrap();
        git
    }

    #[test]
    #[serial_test::serial]
    fn write_default_settings_persists_both_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let _ = init_repo(tmp.path());
        let git = GitCommand::new(false);
        // GitCommand needs to operate in tmp. If GitCommand has a `with_cwd`
        // setter or if its API expects working directory injection, use that.
        // Otherwise, set CWD for the test (RAII via std::env::set_current_dir
        // is the existing pattern in `mod tests` — find and follow it).
        let _guard = crate::test_support::CwdGuard::push(tmp.path()).unwrap();

        write_default_settings(
            &git,
            tmp.path(),
            MergeStyle::RebaseMerge,
            CleanupKind::RemoveBranch,
        )
        .unwrap();

        let style = std::process::Command::new("git")
            .args(["config", "--local", "--get", "daft.merge.style"])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&style.stdout).trim(), "rebase-merge");

        let cleanup = std::process::Command::new("git")
            .args(["config", "--local", "--get", "daft.merge.cleanup"])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&cleanup.stdout).trim(), "remove-branch");
    }
}
```

If `crate::test_support::CwdGuard` doesn't exist, locate the equivalent helper
used by other tests in this branch (search for `CwdGuard` in `src/`). The
earlier rich-output slices added it for parallel-test cwd protection. Use
whatever name and API are present.

- [ ] **Step 2: Add the module declaration** in `src/core/worktree/mod.rs` after
      `pub mod merge;`:

```rust
pub mod merge_set_default;
```

- [ ] **Step 3: Verify the test compiles and passes**

```
mise run test:unit -- write_default_settings_persists_both_keys 2>&1 | tail -10
```

Expected: PASS.

- [ ] **Step 4: Lint + format + commit**

```
mise run fmt && mise run clippy
git add src/core/worktree/merge_set_default.rs src/core/worktree/mod.rs
git commit -m "feat(merge): add write_default_settings for --set-default"
```

### Task 6.2: Add defaults_updated rendering on Output

**Files:**

- Modify: the appropriate `output` module file. Find via:

```
grep -rn "fn result\|impl Output" src/output/ | head -10
```

The cyan/blue rendering should match existing styles in this file (e.g.,
`result()` for green success). Locate the existing methods to match the API
shape.

- [ ] **Step 1: Write a smoke test in the test renderer harness**

Find the existing test harness
(`grep -rn "TestRenderer\|test_output" src/output/ | head -5`). Adapt the test
below to that harness; the assertion shape is invariant.

```rust
#[test]
fn defaults_updated_renders_expected_line() {
    let renderer = TestRenderer::new();
    let output = Output::for_testing(&renderer);
    output.defaults_updated(MergeStyle::Squash, CleanupKind::RemoveBranch);
    let captured = renderer.captured();
    assert!(
        captured.contains("Updated repository defaults: merge.style=squash, merge.cleanup=remove-branch"),
        "expected defaults_updated line, got: {captured}"
    );
}
```

- [ ] **Step 2: Verify failure**

```
mise run test:unit -- defaults_updated_renders 2>&1 | tail -10
```

Expected: compile failure or test failure ("method not found").

- [ ] **Step 3: Implement `defaults_updated` on `Output`**

In the same file, add the method (matching the existing method shape — e.g., if
other rendering methods take `&self` and use a styled writer, follow that
pattern):

```rust
/// Render the "Updated repository defaults" line that follows a successful
/// merge invocation with --set-default. Cyan/blue style to distinguish from
/// success/warn/error.
pub fn defaults_updated(&self, style: MergeStyle, cleanup: CleanupKind) {
    let line = format!(
        "Updated repository defaults: merge.style={}, merge.cleanup={}",
        style, cleanup
    );
    self.styled_line(crate::output::Color::Cyan, &line);
}
```

Adjust the styled-line call to match whatever the existing API looks like.
Common patterns: `self.write_styled(Color::Cyan, &line)` or
`self.println_styled(Style::Info, &line)`. Search the file for the closest
analogue.

- [ ] **Step 4: Run the test**

```
mise run test:unit -- defaults_updated_renders 2>&1 | tail -10
```

Expected: PASS.

- [ ] **Step 5: Lint + format + commit**

```
mise run fmt && mise run clippy
git add -A
git commit -m "feat(output): add defaults_updated rendering"
```

### Task 6.3: Wire --set-default into run()

**Files:**

- Modify: `src/commands/merge.rs` — `run()`.

- [ ] **Step 1: Locate the post-merge cleanup site**

Find where the cleanup loop runs (after the merge step succeeds, before the
final summary). This is roughly `src/commands/merge.rs:806-880`. The wiring goes
IMMEDIATELY before the cleanup loop, AFTER the merge step has succeeded.

- [ ] **Step 2: Add the dispatch**

Insert before the cleanup loop:

```rust
// --set-default: persist the invocation's style + cleanup as repo defaults.
// Best-effort; failure to write surfaces a warning, doesn't fail the merge.
if args.set_default {
    let style = flags.style;
    let cleanup = cleanup_kind;
    match crate::core::worktree::merge_set_default::write_default_settings(
        &git,
        &project_root,
        style,
        cleanup,
    ) {
        Ok(()) => output.defaults_updated(style, cleanup),
        Err(e) => output.warn(&format!("failed to update repository defaults: {e}")),
    }
}
```

The `output.warn` method already exists (used in this branch for cleanup
warnings). If it's named differently (e.g., `output.warning`), use that.

- [ ] **Step 3: Add an end-to-end-ish test**

A unit test in `src/commands/merge.rs::tests` that exercises the full `run()`
path is too heavy. Cover this in the YAML scenarios in slice 7 instead
(`set-default-writes-config.yml`).

- [ ] **Step 4: Build + lint**

```
mise run fmt && mise run clippy 2>&1 | tail -5
mise run test:unit 2>&1 | grep -E "test result|FAIL" | tail -3
```

Expected: zero warnings, all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/commands/merge.rs
git commit -m "feat(merge): wire --set-default into the post-merge flow"
```

---

## Slice 7 — Docs + manual YAML scenarios

Final slice: documentation, manual YAML test coverage, regression updates.

### Task 7.1: Add core YAML scenarios

**Files:**

- Create: nine scenario files under `tests/manual/scenarios/merge/`.

For each scenario, write a YAML file. Reference one existing scenario in the
directory to match the exact harness schema (search for an `.yml` file already
in `tests/manual/scenarios/merge/` and copy its top-level structure: `setup`,
`commands`, `assertions` — the actual field names depend on the harness in
`xtask/src/manual_test/`).

- [ ] **Step 1: Create `style-merge.yml`**

```yaml
name: "merge style produces always-merge-commit by default"
description:
  "No-flag invocation produces --no-ff merge commit, not git's default
  ff-when-possible."
setup:
  - "git checkout -b feat"
  - "echo feat > x.txt"
  - "git add x.txt"
  - "git commit -m 'feat: add x'"
  - "git checkout master"
commands:
  - "daft merge feat"
assertions:
  output_contains:
    - "Merged feat into master"
  exit_code: 0
  shell:
    - command: "git rev-list --parents -n 1 HEAD | awk '{print NF-1}'"
      expected: "2"
```

- [ ] **Step 2: Create `style-squash.yml`**

```yaml
name: "squash style produces single-parent commit"
setup:
  - "git checkout -b feat"
  - "echo a > a.txt; git add a.txt; git commit -m 'a'"
  - "echo b > b.txt; git add b.txt; git commit -m 'b'"
  - "git checkout master"
commands:
  - "daft merge --squash --no-edit -m 'Squashed feat' feat"
assertions:
  output_contains:
    - "Squashed feat into master"
  exit_code: 0
  shell:
    - command: "git rev-list --parents -n 1 HEAD | awk '{print NF-1}'"
      expected: "1"
```

- [ ] **Step 3: Create `style-rebase.yml`**

```yaml
name: "rebase style produces linear history"
setup:
  - "git checkout -b feat"
  - "echo feat > x.txt; git add x.txt; git commit -m 'feat: x'"
  - "git checkout master"
  - "echo master > y.txt; git add y.txt; git commit -m 'master: y'"
commands:
  - "daft merge --rebase feat"
assertions:
  exit_code: 0
  shell:
    - command: "git rev-list --parents -n 1 HEAD | awk '{print NF-1}'"
      expected: "1" # linear: single parent
    - command: "git log --pretty=%s | head -3 | tr '\\n' ',' "
      expected_contains: "feat: x"
```

- [ ] **Step 4: Create `style-rebase-merge.yml`**

```yaml
name: "rebase-merge produces merge commit after rebase"
setup:
  - "git checkout -b feat"
  - "echo feat > x.txt; git add x.txt; git commit -m 'feat: x'"
  - "git checkout master"
  - "echo master > y.txt; git add y.txt; git commit -m 'master: y'"
commands:
  - "daft merge --rebase-merge --no-edit -m 'Merged after rebase' feat"
assertions:
  exit_code: 0
  shell:
    - command: "git rev-list --parents -n 1 HEAD | awk '{print NF-1}'"
      expected: "2"
```

- [ ] **Step 5: Create `cleanup-keep.yml`**

```yaml
name: "default cleanup keeps source worktree and branch"
setup:
  - "daft worktree-create feat"
  - "cd ../feat && echo feat > x.txt && git add x.txt && git commit -m 'feat: x'"
commands:
  - "daft merge feat"
assertions:
  exit_code: 0
  shell:
    - command: "git branch --list feat | wc -l | tr -d ' '"
      expected: "1" # branch survives
    - command: "test -d ../feat && echo yes || echo no"
      expected: "yes" # worktree survives
```

- [ ] **Step 6: Create `cleanup-remove-branch-local.yml`**

```yaml
name: "remove-branch deletes branch locally when branch.deleteRemote=false"
setup:
  - "git config --local branch.deleteRemote false"
  - "daft worktree-create feat"
  - "cd ../feat && echo feat > x.txt && git add x.txt && git commit -m 'feat: x'"
commands:
  - "daft merge --remove-branch feat"
assertions:
  exit_code: 0
  shell:
    - command: "git branch --list feat | wc -l | tr -d ' '"
      expected: "0" # branch gone
    - command: "test -d ../feat && echo yes || echo no"
      expected: "no" # worktree gone
```

- [ ] **Step 7: Create `set-default-writes-config.yml`**

```yaml
name: "--set-default persists style and cleanup to git config"
setup:
  - "git checkout -b feat"
  - "echo feat > x.txt; git add x.txt; git commit -m 'feat: x'"
  - "git checkout master"
commands:
  - "daft merge --squash --remove-branch --set-default --no-edit -m 'Squashed'
    feat"
assertions:
  output_contains:
    - "Updated repository defaults"
    - "merge.style=squash"
    - "merge.cleanup=remove-branch"
  exit_code: 0
  shell:
    - command: "git config --local --get daft.merge.style"
      expected: "squash"
    - command: "git config --local --get daft.merge.cleanup"
      expected: "remove-branch"
```

- [ ] **Step 8: Create `pre-merge-warn-override-allows-merge.yml`**

```yaml
name: "pre-merge fail-mode=warn override permits the merge to proceed"
setup:
  - |
    mkdir -p .daft/hooks
    cat > .daft/hooks/pre-merge.yml <<'YAML'
    fail-mode: warn
    run: |
      echo "pre-merge would block" >&2
      exit 1
    YAML
  - "daft hooks trust .daft/hooks/pre-merge.yml"
  - "git checkout -b feat"
  - "echo feat > x.txt; git add x.txt; git commit -m 'feat: x'"
  - "git checkout master"
commands:
  - "daft merge feat"
assertions:
  output_contains:
    - "warning" # warn-mode hook surfaces a warning
    - "Merged feat into master" # but the merge still happens
  exit_code: 0
```

Adapt the hooks-config schema if the harness uses a different on-disk layout —
search `tests/manual/scenarios/` for an existing pre-merge hook scenario that
already works, and mimic its file paths.

- [ ] **Step 9: Create `post-merge-fires-after-warn-override.yml`**

```yaml
name: "post-merge fires even when pre-merge was warn-overridden"
setup:
  - |
    mkdir -p .daft/hooks
    cat > .daft/hooks/pre-merge.yml <<'YAML'
    fail-mode: warn
    run: |
      echo "pre-merge would block" >&2
      exit 1
    YAML
  - |
    cat > .daft/hooks/post-merge.yml <<'YAML'
    run: |
      echo "POST_MERGE_FIRED=yes" > /tmp/daft-test-post-merge-marker
    YAML
  - "daft hooks trust .daft/hooks/pre-merge.yml .daft/hooks/post-merge.yml"
  - "git checkout -b feat"
  - "echo feat > x.txt; git add x.txt; git commit -m 'feat: x'"
  - "git checkout master"
commands:
  - "daft merge feat"
assertions:
  exit_code: 0
  shell:
    - command: "cat /tmp/daft-test-post-merge-marker"
      expected_contains: "POST_MERGE_FIRED=yes"
```

- [ ] **Step 10: Run the YAML scenarios**

```
mise run test:manual -- --ci merge 2>&1 | tail -30
```

Expected: all new scenarios pass; existing scenarios in `merge/` still pass.

- [ ] **Step 11: Commit**

```bash
git add tests/manual/scenarios/merge/
git commit -m "test(merge): add YAML scenarios for PR-style flag/cleanup matrix"
```

### Task 7.2: Update or remove regression scenarios

Existing scenarios under `tests/manual/scenarios/merge/` likely reference
`--no-ff`, `-rb`, etc. Update each to the new flag set.

- [ ] **Step 1: Find affected scenarios**

```
grep -lE -- "--no-ff\b|--ff-only\b|--ff\b|--no-squash\b|-rb\b|--remove\b|--and-branch\b" tests/manual/scenarios/merge/*.yml
```

- [ ] **Step 2: Update each match**

For each file, swap:

- `--no-ff` → (no flag; default is now always-merge-commit)
- `--ff-only` → `--rebase`
- `--ff` → (no flag)
- `--no-squash` → `--merge` (or remove if redundant)
- `-rb` → `-r` (note: `-r` semantics changed — now means full cleanup)
- `--remove` (worktree-only) → if the test only wanted worktree removal, this is
  no longer expressible; either delete the scenario or rewrite to use
  `daft remove` afterward.
- `--and-branch` → fold into `-r` / `--remove-branch`

If a scenario specifically tested the dropped behavior (e.g.,
FF-when-possible-default), delete it and add a note in the commit message.

- [ ] **Step 3: Run the manual scenarios end-to-end**

```
mise run test:manual -- --ci merge 2>&1 | tail -20
```

Expected: all scenarios in `merge/` pass.

- [ ] **Step 4: Commit**

```bash
git add tests/manual/scenarios/merge/
git commit -m "test(merge): update regression scenarios for new flag set"
```

### Task 7.3: Update CLI documentation

**Files:**

- Modify: `docs/cli/daft-merge.md`.

- [ ] **Step 1: Read the current page**

```
cat docs/cli/daft-merge.md | head -120
```

- [ ] **Step 2: Replace the flags section** with the new flag list. Match the
      doc's existing style. Reproduce verbatim from the spec's "Surface" → "CLI
      flags" section (the four style booleans, the two cleanup flags,
      `--set-default`).

Sample replacement block (adapt to the page's existing markdown conventions):

````markdown
## Merge styles

`daft merge` supports four named merge styles, mutually exclusive on the command
line:

| Flag             | Behavior                                                         |
| ---------------- | ---------------------------------------------------------------- |
| `--merge`        | (Default.) Always create a merge commit (`git merge --no-ff`).   |
| `--squash`       | Collapse source's commits into a single squash commit on target. |
| `--rebase`       | Rebase source onto target, then fast-forward (linear history).   |
| `--rebase-merge` | Rebase source onto target, then create a merge commit.           |

Default style is configurable via `daft.merge.style` (git config) or
`merge.style` in `daft.yml`. CLI flags override config.

## Cleanup

After a successful merge, source disposition is controlled by:

| Flag                    | Outcome                                                  |
| ----------------------- | -------------------------------------------------------- |
| (no flag)               | (Default.) Source worktree and branch survive.           |
| `-r`, `--remove-branch` | Remove source worktree AND delete source branch (local). |
| `--keep-branch`         | Explicit keep — for canceling a config-set default.      |

When `branch.deleteRemote=true` is configured, `--remove-branch` also issues
`git push origin --delete <branch>`.

## Persisting defaults

`--set-default` writes the invocation's style and cleanup choices to
`git config --local` after the merge succeeds:

```bash
daft merge --squash --remove-branch --set-default
# → daft.merge.style=squash, daft.merge.cleanup=remove-branch
```
````

````

- [ ] **Step 3: Remove sections referencing the old flags** (`--ff`, `--no-ff`, `--ff-only`, `-rb`, `--and-branch`). The doc had a fast-forward control section; delete it.

- [ ] **Step 4: Add a "Migration from earlier daft versions" callout**

```markdown
## Migration

Earlier versions exposed `--ff`, `--no-ff`, `--ff-only`, `--squash`,
`--no-squash`, `--remove`, and `-b/--and-branch`. These are gone.
Replacements:

| Old                              | New                                          |
|----------------------------------|----------------------------------------------|
| `--no-ff`                        | (no flag; default behavior)                  |
| `--ff` / no flag                 | (no flag; default is now always-merge-commit) |
| `--ff-only`                      | `--rebase`                                   |
| `--squash`                       | `--squash` (preserved)                       |
| `--no-squash`                    | `--merge` / `--rebase` / `--rebase-merge`    |
| `--remove` (worktree only)       | (no equivalent; run `daft remove` separately) |
| `-rb` / `-r --and-branch`        | `-r` / `--remove-branch`                     |

Config keys `daft.merge.ff`, `daft.merge.squash`,
`daft.merge.postMerge.removeSourceWorktree`,
`daft.merge.postMerge.alsoRemoveSourceBranch` are removed. Use
`daft.merge.style` and `daft.merge.cleanup` instead.
````

- [ ] **Step 5: Build the docs locally**

```
mise run docs:site:build 2>&1 | tail -10
```

Expected: build succeeds; no broken links.

- [ ] **Step 6: Commit**

```bash
git add docs/cli/daft-merge.md
git commit -m "docs(merge): update CLI ref for PR-style redesign"
```

### Task 7.4: Update hooks guide

**Files:**

- Modify: `docs/guide/hooks.md`.

- [ ] **Step 1: Add a "Merge hooks behave like PR checks" subsection**

Insert near the existing pre-merge / post-merge documentation:

```markdown
## Merge hooks behave like PR checks

`pre-merge` defaults to `fail-mode: abort` — a failing hook stops the merge
before any state changes, just like a failing CI check on a GitHub PR. This is
overridable per-hook (`fail-mode: warn`) as a manual escape hatch when you know
what you're doing and want to plow through.

`post-merge` fires whenever the merge actually happened, regardless of pre-merge
mode. If you override pre-merge to `warn` and the hook still fails (warning
only), the merge proceeds; if the merge then succeeds, post-merge runs. This
preserves orthogonal post-merge use cases — notifications, release-note
generation, project-wide announcements — that should fire on success regardless
of pre-merge state.

`post-merge` does **not** fire if:

- Pre-merge aborted (no merge attempted).
- The merge had conflicts and `--continue` has not yet completed.
- The merge was aborted via `daft merge --abort`.
- Squash style staged changes but the commit step was aborted.

When the user later runs `daft merge --continue` and that produces a commit,
post-merge fires at that point.

Cleanup (`worktree-pre-remove`, `worktree-post-remove`) is a separate phase that
follows post-merge. See "Cleanup hooks during merge" below.
```

- [ ] **Step 2: Verify the existing "Cleanup hooks during merge" subsection
      still describes correct behavior** — that subsection landed in the
      previous slice and should still be accurate. Cross-check it for references
      to `-rb`; if found, update to `-r` / `--remove-branch`.

- [ ] **Step 3: Build docs**

```
mise run docs:site:build 2>&1 | tail -10
```

- [ ] **Step 4: Commit**

```bash
git add docs/guide/hooks.md
git commit -m "docs(hooks): clarify merge hook semantics for PR-check style"
```

### Task 7.5: Update SKILL.md

**Files:**

- Modify: `SKILL.md`.

- [ ] **Step 1: Find the merge-related section**

```
grep -n "daft merge\|--squash\|merge style\|-rb\|--remove" SKILL.md
```

- [ ] **Step 2: Update the prose** to reflect the new flag set. Replace any
      references to `-rb`, `--no-ff`, `--ff-only` with the new equivalents. Add
      a brief mention of merge styles:

```markdown
### Merging

`daft merge <source>` merges into the current worktree's branch (or
`--into <target>`). Four styles are available via mutually-exclusive flags:

- `--merge` (default) — always merge commit.
- `--squash` — collapse to one commit.
- `--rebase` — linear history (rebase + ff).
- `--rebase-merge` — linear source history with a merge commit boundary.

Cleanup: `-r` / `--remove-branch` removes source worktree and branch after a
successful merge. The local/remote behavior follows `branch.deleteRemote`.

`--set-default` promotes the invocation's style + cleanup as the new repo
defaults via `git config --local`.
```

- [ ] **Step 3: Commit**

```bash
git add SKILL.md
git commit -m "docs(skill): update SKILL.md for merge PR-style"
```

### Task 7.6: Regenerate man pages

**Files:**

- Modify: `man/daft-merge.1`, `man/git-worktree-merge.1`.

- [ ] **Step 1: Regenerate**

```
mise run man:gen
```

Expected: regenerated man pages in `man/`.

- [ ] **Step 2: Verify**

```
mise run man:verify
```

Expected: passes (man pages match clap's current help output).

- [ ] **Step 3: Commit**

```bash
git add man/
git commit -m "chore(merge): regenerate man pages for new flag set"
```

### Task 7.7: Final regression sweep

- [ ] **Step 1: Full local CI**

```
mise run ci 2>&1 | tail -30
```

Expected: all checks green.

- [ ] **Step 2: Full manual scenarios**

```
mise run test:manual -- --ci 2>&1 | tail -30
```

Expected: every scenario passes.

- [ ] **Step 3: Push branch and verify CI**

```
git push --force-with-lease
gh run watch
```

Expected: GitHub CI green.

- [ ] **Step 4: Final commit if anything was touched in step 3**

If any docs or test fixtures changed during the regression sweep, commit them.
Otherwise this task is a no-op verifier.

```bash
git status --short
# If clean: nothing to commit; the slice is complete.
```

---

## Out-of-scope (do NOT pull into this plan)

These belong in follow-up tickets per the spec:

1. PR conversations / discussion threads.
2. PR reviews / approval gates.
3. PR labels / tags.
4. `merge.removeRemoteBranch` standalone config (decoupled from
   `branch.deleteRemote`).
5. `--set-default --user` (global scope).
6. `--set-default` saving additional flags (signoff, strategy, etc.).
7. Strict FF precondition flag (replacement for dropped `--ff-only`).
