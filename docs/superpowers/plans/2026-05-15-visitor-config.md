# Visitor Configuration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add visitor-classified `daft.yml` and `daft.local.yml` with
daft-managed propagation across the worktree lifecycle, plus `daft install` and
`daft file merge` commands, so users can adopt daft fully without committing to
upstream.

**Architecture:** Visitor/team classification is a runtime property derived from
git tracking status. The existing recursive-merge functions (`merge_configs`,
`merge_hook_defs`, `merge_log_configs`) back both the load-time overlay stack
and the new on-disk `daft file merge`. Propagation copies in-scope untracked
daft files between worktrees on branch-out, on `daft merge` (atomic
write-merge-restore), and on remote-merge detection, with worktree removal as a
safety boundary against lost propagation. Collision resolution between visitor
and tracked `daft.yml` is explicitly deferred to the future `daft pull` (#493).

**Tech Stack:** Rust 2024 edition, clap (parser), serde_yaml, anyhow, tracing.
Test harness: built-in `cargo test`, bash integration tests under
`tests/integration/`, YAML manual scenarios under `tests/manual/scenarios/`.

**Spec:**
[docs/superpowers/specs/2026-05-15-visitor-config-design.md](../specs/2026-05-15-visitor-config-design.md)

**Related issues:** [#335](https://github.com/avihut/daft/issues/335) (this
feature), [#493](https://github.com/avihut/daft/issues/493) (deferred
collision-resolution work).

**Before-commit checklist (every commit):**

```bash
mise run fmt && mise run clippy && mise run test:unit
```

All three must pass. CI enforces them. The implementer should run them at every
commit step in this plan even when not spelled out, and only commit on success.

---

## Phase 1 — Loader foundation

### Task 1.1: Extend `find_local_config` to support `daft.local.yml` (preferred) and `daft-local.yml` (deprecated)

**Files:**

- Modify: `src/hooks/yaml_config_loader.rs:57-76` (the existing
  `find_local_config` function)
- Test: same file, `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing tests**

Add these tests to the existing `mod tests` block in
`src/hooks/yaml_config_loader.rs`:

```rust
#[test]
fn test_find_local_config_prefers_dot_infix() {
    let dir = tempdir().unwrap();
    let main_config = dir.path().join("daft.yml");
    write_file(dir.path(), "daft.yml", "hooks: {}");
    write_file(dir.path(), "daft.local.yml", "hooks: {}");
    write_file(dir.path(), "daft-local.yml", "hooks: {}");

    let local = find_local_config(&main_config).unwrap();
    assert_eq!(local, dir.path().join("daft.local.yml"));
}

#[test]
fn test_find_local_config_falls_back_to_dash_infix() {
    let dir = tempdir().unwrap();
    let main_config = dir.path().join("daft.yml");
    write_file(dir.path(), "daft.yml", "hooks: {}");
    write_file(dir.path(), "daft-local.yml", "hooks: {}");

    let local = find_local_config(&main_config).unwrap();
    assert_eq!(local, dir.path().join("daft-local.yml"));
}

#[test]
fn test_find_local_config_dot_prefix_main_dot_infix_local() {
    let dir = tempdir().unwrap();
    let main_config = dir.path().join(".daft.yml");
    write_file(dir.path(), ".daft.yml", "hooks: {}");
    write_file(dir.path(), ".daft.local.yml", "hooks: {}");

    let local = find_local_config(&main_config).unwrap();
    assert_eq!(local, dir.path().join(".daft.local.yml"));
}

#[test]
fn test_find_local_config_yaml_extension_dot_infix() {
    let dir = tempdir().unwrap();
    let main_config = dir.path().join("daft.yaml");
    write_file(dir.path(), "daft.yaml", "hooks: {}");
    write_file(dir.path(), "daft.local.yaml", "hooks: {}");

    let local = find_local_config(&main_config).unwrap();
    assert_eq!(local, dir.path().join("daft.local.yaml"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:
`cargo test --lib find_local_config_prefers_dot_infix find_local_config_falls_back_to_dash_infix find_local_config_dot_prefix_main_dot_infix_local find_local_config_yaml_extension_dot_infix`

Expected: all four tests FAIL (the new preferred dot-infix names are not yet
recognized).

- [ ] **Step 3: Replace `find_local_config` with the new lookup logic**

Replace lines 54-76 in `src/hooks/yaml_config_loader.rs` with:

```rust
/// Find the local override config file for the given main config.
///
/// Searches in priority order:
///   1. Preferred dot-infix name (e.g. `daft.local.yml`)
///   2. Deprecated dash-infix alias (e.g. `daft-local.yml`)
///
/// When the deprecated alias is used, emits a `tracing::warn` so users see
/// the rename suggestion at load time.
///
/// Returns the path if found.
pub fn find_local_config(main_config: &Path) -> Option<PathBuf> {
    let parent = main_config.parent()?;
    let filename = main_config.file_name()?.to_str()?;

    let (stem, ext) = if let Some(s) = filename.strip_suffix(".yaml") {
        (s, ".yaml")
    } else if let Some(s) = filename.strip_suffix(".yml") {
        (s, ".yml")
    } else {
        return None;
    };

    let preferred = parent.join(format!("{stem}.local{ext}"));
    if preferred.is_file() {
        return Some(preferred);
    }

    let deprecated = parent.join(format!("{stem}-local{ext}"));
    if deprecated.is_file() {
        tracing::warn!(
            "deprecated local config name '{}' — rename to '{}.local{}'",
            deprecated.display(),
            stem,
            ext
        );
        return Some(deprecated);
    }

    None
}
```

- [ ] **Step 4: Run all `find_local_config_*` tests to verify they pass**

Run: `cargo test --lib find_local_config`

Expected: all `find_local_config_*` tests PASS, including the existing
`test_find_local_config`, `test_find_local_config_dot_prefix`, and
`test_find_local_config_none` (the existing tests use the dash-infix alias which
is still supported).

- [ ] **Step 5: Commit**

```bash
mise run fmt && mise run clippy && mise run test:unit
git add src/hooks/yaml_config_loader.rs
git commit -m "feat(config): support daft.local.yml as preferred local override name"
```

---

### Task 1.2: Add `classify_main_config` helper

**Files:**

- Modify: `src/hooks/yaml_config_loader.rs` (add new public helper + tests)

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `src/hooks/yaml_config_loader.rs`:

```rust
#[test]
fn test_classify_main_config_missing() {
    let dir = tempdir().unwrap();
    assert_eq!(classify_main_config(dir.path()), ConfigStatus::Missing);
}

#[test]
fn test_classify_main_config_visitor_untracked() {
    let dir = tempdir().unwrap();
    // Initialize a git repo
    Command::new("git").args(["init"]).arg(dir.path()).output().unwrap();
    Command::new("git")
        .arg("-C").arg(dir.path())
        .args(["config", "user.email", "test@test.com"])
        .output().unwrap();
    Command::new("git")
        .arg("-C").arg(dir.path())
        .args(["config", "user.name", "Test"])
        .output().unwrap();
    write_file(dir.path(), "daft.yml", "hooks: {}");
    // daft.yml exists in worktree but is NOT tracked → visitor
    assert_eq!(classify_main_config(dir.path()), ConfigStatus::Visitor);
}

#[test]
fn test_classify_main_config_tracked() {
    let dir = tempdir().unwrap();
    Command::new("git").args(["init"]).arg(dir.path()).output().unwrap();
    Command::new("git")
        .arg("-C").arg(dir.path())
        .args(["config", "user.email", "test@test.com"])
        .output().unwrap();
    Command::new("git")
        .arg("-C").arg(dir.path())
        .args(["config", "user.name", "Test"])
        .output().unwrap();
    write_file(dir.path(), "daft.yml", "hooks: {}");
    Command::new("git")
        .arg("-C").arg(dir.path())
        .args(["add", "daft.yml"])
        .output().unwrap();
    Command::new("git")
        .arg("-C").arg(dir.path())
        .args(["commit", "-m", "add"])
        .output().unwrap();

    assert_eq!(classify_main_config(dir.path()), ConfigStatus::Tracked);
}

#[test]
fn test_classify_main_config_no_git_falls_back_to_tracked() {
    // Conservative fallback: if git can't answer, treat as tracked
    let dir = tempdir().unwrap();
    write_file(dir.path(), "daft.yml", "hooks: {}");
    // No git init here — git ls-files will fail
    assert_eq!(classify_main_config(dir.path()), ConfigStatus::Tracked);
}
```

- [ ] **Step 2: Run tests to verify they fail to compile**

Run: `cargo test --lib classify_main_config 2>&1 | head -30`

Expected: compilation errors — `ConfigStatus` and `classify_main_config` are not
defined.

- [ ] **Step 3: Add the type and function**

Append to `src/hooks/yaml_config_loader.rs` (before the `#[cfg(test)]` block):

```rust
/// Runtime classification of the main `daft.yml` based on git tracking status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigStatus {
    /// `daft.yml` is committed to the repo (team config).
    Tracked,
    /// `daft.yml` exists in the worktree but is not tracked (visitor config).
    Visitor,
    /// No `daft.yml` discovered at any candidate path.
    Missing,
}

/// Classify the main daft config in `worktree_root` by tracking status.
///
/// Returns `Missing` if no candidate file exists. Otherwise runs
/// `git ls-files --error-unmatch <relative-path>` and maps the exit status:
/// success → `Tracked`, failure → `Visitor`. If the git invocation itself
/// errors (no git binary, not inside a repo), returns `Tracked` as a
/// conservative fallback — we'd rather not implicitly treat a file as visitor
/// when we can't confirm.
pub fn classify_main_config(worktree_root: &Path) -> ConfigStatus {
    let (path, _location) = match find_config_file(worktree_root) {
        Some(found) => found,
        None => return ConfigStatus::Missing,
    };

    let relative = match path.strip_prefix(worktree_root) {
        Ok(p) => p,
        Err(_) => return ConfigStatus::Tracked,
    };

    let status = Command::new("git")
        .arg("-C")
        .arg(worktree_root)
        .args(["ls-files", "--error-unmatch"])
        .arg(relative)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match status {
        Ok(s) if s.success() => ConfigStatus::Tracked,
        Ok(_) => ConfigStatus::Visitor,
        Err(_) => ConfigStatus::Tracked,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib classify_main_config`

Expected: all four tests PASS.

- [ ] **Step 5: Commit**

```bash
mise run fmt && mise run clippy && mise run test:unit
git add src/hooks/yaml_config_loader.rs
git commit -m "feat(config): add classify_main_config helper for visitor classification"
```

---

## Phase 2 — `daft install` command

### Task 2.1: Scaffold the `install` command module

**Files:**

- Create: `src/commands/install.rs`
- Modify: `src/commands/mod.rs` (register new module)

- [ ] **Step 1: Create the module skeleton**

Create `src/commands/install.rs`:

```rust
use anyhow::{Context, Result};
use clap::Parser;
use std::fs;
use std::path::PathBuf;

use crate::output::{CliOutput, Output, OutputConfig};
use crate::utils::get_current_directory;

#[derive(Parser)]
#[command(name = "daft-install")]
#[command(version = crate::VERSION)]
#[command(about = "Install a starter daft.yml in the current worktree")]
#[command(long_about = r#"
Creates a starter daft.yml at the current worktree root with a commented
skeleton covering the major sections (hooks, shared, layout). Modeled on
`lefthook install`.

If daft.yml already exists, the command refuses without modifying anything;
edit the existing file with your editor or a future `daft config` TUI.

No git side effects: daft does not write to .gitignore or .git/info/exclude.
Ignore rules are the user's responsibility.
"#)]
pub struct Args {
    #[arg(short = 'q', long = "quiet", help = "Suppress progress reporting")]
    quiet: bool,

    #[arg(short = 'v', long = "verbose", help = "Show detailed progress")]
    verbose: bool,
}

const STARTER_TEMPLATE: &str = include_str!("install/starter.yml");

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("daft-install"));
    let config = OutputConfig::new(args.quiet, args.verbose);
    let mut output = CliOutput::new(config);
    run_with_output(&mut output)
}

pub fn run_with_output(output: &mut dyn Output) -> Result<()> {
    let cwd = get_current_directory()?;
    install_starter(&cwd, output)
}

pub fn install_starter(worktree_root: &PathBuf, output: &mut dyn Output) -> Result<()> {
    let target = worktree_root.join("daft.yml");
    if target.exists() {
        anyhow::bail!(
            "daft.yml already exists at {}. Edit it directly with your editor.",
            target.display()
        );
    }
    fs::write(&target, STARTER_TEMPLATE)
        .with_context(|| format!("Failed to write {}", target.display()))?;

    output.result(&format!("Installed daft.yml at {}", target.display()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::TestOutput;
    use tempfile::tempdir;

    #[test]
    fn test_install_creates_starter_file() {
        let dir = tempdir().unwrap();
        let mut output = TestOutput::new();
        install_starter(&dir.path().to_path_buf(), &mut output).unwrap();
        assert!(dir.path().join("daft.yml").is_file());
    }

    #[test]
    fn test_install_refuses_if_already_exists() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("daft.yml"), "hooks: {}").unwrap();
        let mut output = TestOutput::new();
        let result = install_starter(&dir.path().to_path_buf(), &mut output);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }
}
```

- [ ] **Step 2: Create the starter template**

Create directory `src/commands/install/` and file
`src/commands/install/starter.yml`:

```yaml
# daft.yml — daft configuration for this clone.
#
# Status:
#   - This file is your local visitor configuration unless committed to git.
#   - Once tracked, it becomes the team baseline and applies to all clones.
#   - For personal overrides on top of a tracked daft.yml, create daft.local.yml
#     (must remain untracked; doctor will flag it as a smell if tracked).
#
# Sections below are commented placeholders. Uncomment and edit to enable.

# layout: sibling
# Layouts: sibling | nested | contained | centralized | <custom>

# shared: []
# Files/directories shared across worktrees via symlink (e.g. ".env").

# hooks:
#   post-clone:
#     jobs:
#       - name: example
#         run: echo "hello from daft"
#
#   worktree-post-create:
#     jobs:
#       - name: example
#         run: echo "new worktree created"
```

- [ ] **Step 3: Register the module**

Modify `src/commands/mod.rs`. Add to the alphabetically appropriate spot:

```rust
pub mod install;
```

- [ ] **Step 4: Run tests to verify**

Run: `cargo test --lib commands::install`

Expected: both tests PASS.

- [ ] **Step 5: Commit**

```bash
mise run fmt && mise run clippy && mise run test:unit
git add src/commands/install.rs src/commands/install/ src/commands/mod.rs
git commit -m "feat(install): scaffold daft install command"
```

---

### Task 2.2: Wire `daft install` into the multicall binary

**Files:**

- Modify: `src/main.rs`
- Modify: `xtask/src/main.rs` (COMMANDS array)
- Modify: `src/commands/docs.rs`

- [ ] **Step 1: Find the existing routing pattern**

Open `src/main.rs` and locate where subcommands dispatch. Look for patterns
matching on subcommand strings like `"repo"`, `"config"`, etc. Note the
convention — direct symlinked commands (like `git-worktree-init`) and
`daft <verb>` subcommands have different routing.

- [ ] **Step 2: Add the routing**

Add a match arm in `src/main.rs` that dispatches `"install"` as a `daft <verb>`
subcommand to `commands::install::run()`. Mirror the pattern used by
`commands::shared::run()` or another single-verb command.

- [ ] **Step 3: Add to xtask COMMANDS**

In `xtask/src/main.rs`, add an entry for `"install"` to the `COMMANDS` array and
the `get_command_for_name()` function so completions and man-page generation
include it.

- [ ] **Step 4: Add to help output**

In `src/commands/docs.rs`, add `install` to the appropriate category in
`get_command_categories()` (likely "Configuration" or "Setup").

- [ ] **Step 5: Smoke test from the binary**

```bash
cargo build
cd /tmp && mkdir daft-install-test && cd daft-install-test
git init
"$OLDPWD/target/debug/daft" install
ls -la daft.yml
cd .. && rm -rf daft-install-test
```

Expected: `daft.yml` is created with the starter template.

- [ ] **Step 6: Commit**

```bash
mise run fmt && mise run clippy && mise run test:unit
git add src/main.rs xtask/src/main.rs src/commands/docs.rs
git commit -m "feat(install): route daft install through the multicall binary"
```

---

### Task 2.3: YAML manual scenarios for `daft install`

**Files:**

- Create: `tests/manual/scenarios/install/basic.yaml`
- Create: `tests/manual/scenarios/install/refuse-on-existing.yaml`

- [ ] **Step 1: Author the basic scenario**

Create `tests/manual/scenarios/install/basic.yaml`. Mirror the schema used by an
existing scenario in `tests/manual/scenarios/` (read one such as
`tests/manual/scenarios/checkout/basic.yaml` first to copy the structure).

The scenario should:

1. Initialize a fresh repo (no daft.yml).
2. Run `daft install`.
3. Assert `daft.yml` now exists at the worktree root and contains the starter
   template marker comment.

- [ ] **Step 2: Author the refuse-on-existing scenario**

Create `tests/manual/scenarios/install/refuse-on-existing.yaml`:

1. Initialize a fresh repo and pre-create a `daft.yml` with arbitrary content.
2. Run `daft install`.
3. Assert the command exits non-zero and that `daft.yml` content is unchanged
   (didn't get overwritten by the starter).

- [ ] **Step 3: Run the scenarios**

Run: `mise run test:manual -- --ci install`

Expected: both scenarios pass.

- [ ] **Step 4: Commit**

```bash
git add tests/manual/scenarios/install/
git commit -m "test(install): yaml manual scenarios for daft install"
```

---

## Phase 3 — `daft file merge` command

### Task 3.1: Scaffold the `file` command bucket

**Files:**

- Create: `src/commands/file/mod.rs`
- Modify: `src/commands/mod.rs`

- [ ] **Step 1: Create the dispatcher**

Create `src/commands/file/mod.rs`:

```rust
pub mod merge;

use anyhow::Result;

pub fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let file_idx = args.iter().position(|a| a == "file").unwrap_or(1);
    let sub_args: Vec<String> = args[(file_idx + 1)..].to_vec();

    if sub_args.is_empty() {
        show_usage();
        return Ok(());
    }

    match sub_args[0].as_str() {
        "merge" => merge::run(&sub_args[1..]),
        "--help" | "-h" => {
            show_usage();
            Ok(())
        }
        other => {
            anyhow::bail!(
                "Unknown file subcommand: '{}'\n\nUsage: daft file merge <TARGET> <SOURCE>",
                other
            );
        }
    }
}

fn show_usage() {
    eprintln!("Usage: daft file <subcommand>");
    eprintln!();
    eprintln!("Available subcommands:");
    eprintln!("  merge    Recursively merge a daft YAML file into another");
    eprintln!();
    eprintln!("Run 'daft file <subcommand> --help' for details.");
}
```

- [ ] **Step 2: Add `pub mod file;` to `src/commands/mod.rs`**

Insert `pub mod file;` in the alphabetically appropriate spot in
`src/commands/mod.rs`.

- [ ] **Step 3: Verify it compiles (merge module is empty stub for now)**

Create a stub `src/commands/file/merge.rs`:

```rust
use anyhow::Result;

pub fn run(_args: &[String]) -> Result<()> {
    anyhow::bail!("daft file merge: not yet implemented")
}
```

Run: `cargo build`

Expected: compiles successfully.

- [ ] **Step 4: Commit**

```bash
mise run fmt && mise run clippy
git add src/commands/file/ src/commands/mod.rs
git commit -m "feat(file): scaffold daft file command bucket"
```

---

### Task 3.2: Implement `daft file merge` core logic

**Files:**

- Modify: `src/commands/file/merge.rs`

- [ ] **Step 1: Write the failing tests**

Replace `src/commands/file/merge.rs` test scaffold by adding this test module at
the bottom of the file (after the implementation in subsequent steps). For now,
write the tests first to a separate spot you'll bring in next:

Plan-step content (the tests you will eventually paste in once you write the
impl in Step 2 — but write them now in the file before the impl so they fail):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs;

    fn write(dir: &std::path::Path, name: &str, content: &str) {
        fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn test_merge_adds_new_hook_from_source() {
        let dir = tempdir().unwrap();
        write(dir.path(), "target.yml",
            "hooks:\n  post-clone:\n    jobs:\n      - name: a\n        run: echo a\n");
        write(dir.path(), "source.yml",
            "hooks:\n  worktree-post-create:\n    jobs:\n      - name: b\n        run: echo b\n");

        merge_files(
            &dir.path().join("target.yml"),
            &dir.path().join("source.yml"),
            MergeOptions { keep_source: false, yes: true },
        ).unwrap();

        let merged = fs::read_to_string(dir.path().join("target.yml")).unwrap();
        assert!(merged.contains("post-clone"));
        assert!(merged.contains("worktree-post-create"));
        assert!(!dir.path().join("source.yml").exists(), "source should be deleted by default");
    }

    #[test]
    fn test_merge_keep_source() {
        let dir = tempdir().unwrap();
        write(dir.path(), "target.yml", "hooks: {}");
        write(dir.path(), "source.yml", "hooks: {}");

        merge_files(
            &dir.path().join("target.yml"),
            &dir.path().join("source.yml"),
            MergeOptions { keep_source: true, yes: true },
        ).unwrap();

        assert!(dir.path().join("source.yml").exists(), "source should be kept");
    }

    #[test]
    fn test_merge_source_wins_on_conflict() {
        let dir = tempdir().unwrap();
        write(dir.path(), "target.yml",
            "hooks:\n  post-clone:\n    jobs:\n      - name: lint\n        run: echo target\n");
        write(dir.path(), "source.yml",
            "hooks:\n  post-clone:\n    jobs:\n      - name: lint\n        run: echo source\n");

        merge_files(
            &dir.path().join("target.yml"),
            &dir.path().join("source.yml"),
            MergeOptions { keep_source: true, yes: true },
        ).unwrap();

        let merged = fs::read_to_string(dir.path().join("target.yml")).unwrap();
        assert!(merged.contains("echo source"));
        assert!(!merged.contains("echo target"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail to compile**

Run: `cargo test --lib commands::file::merge 2>&1 | head -30`

Expected: compilation errors — `merge_files` and `MergeOptions` not defined yet.

- [ ] **Step 3: Implement the merge logic**

Replace the stub `src/commands/file/merge.rs` with:

```rust
use anyhow::{Context, Result};
use clap::Parser;
use std::fs;
use std::path::{Path, PathBuf};

use crate::hooks::yaml_config_loader::{merge_configs, parse_yaml_config_str};

#[derive(Parser)]
#[command(name = "daft-file-merge")]
#[command(about = "Recursively merge a daft YAML file into another")]
#[command(long_about = r#"
Merges <SOURCE> into <TARGET> using recursive YAML semantics:

  - Scalar fields in source override target if set.
  - Hooks merge by name (no full replacement).
  - Named jobs replace target jobs with the same name; unnamed jobs from
    source are appended.
  - Source wins on every conflict.

Source file is deleted on success unless --keep-source is set.

If the target is currently untracked (visitor file), the command prompts for
confirmation before writing, since undoing an unversioned write is manual.
Pass --yes or --force to skip the prompt for scripting.

Argument forms:
  daft file merge <TARGET> <SOURCE>     explicit form
  daft file merge <SOURCE>              implied target = daft.yml in cwd
"#)]
pub struct CliArgs {
    /// First positional: target file, or source if it's the only positional given.
    first: PathBuf,

    /// Optional second positional: when present, `first` is target and this is source.
    second: Option<PathBuf>,

    #[arg(long = "keep-source", help = "Keep the source file after merging")]
    keep_source: bool,

    #[arg(short = 'y', long = "yes", alias = "force",
        help = "Skip confirmation prompts (for scripting)")]
    yes: bool,
}

pub struct MergeOptions {
    pub keep_source: bool,
    pub yes: bool,
}

pub fn run(args: &[String]) -> Result<()> {
    let parsed = CliArgs::try_parse_from(
        std::iter::once("daft-file-merge".to_string())
            .chain(args.iter().cloned()),
    )?;

    let (target, source) = match parsed.second {
        Some(src) => (parsed.first, src),
        None => {
            let cwd = std::env::current_dir().context("Failed to read cwd")?;
            (cwd.join("daft.yml"), parsed.first)
        }
    };

    merge_files(
        &target,
        &source,
        MergeOptions { keep_source: parsed.keep_source, yes: parsed.yes },
    )
}

pub fn merge_files(target: &Path, source: &Path, opts: MergeOptions) -> Result<()> {
    if !source.is_file() {
        anyhow::bail!("source file not found: {}", source.display());
    }
    if target == source {
        anyhow::bail!("target and source are the same file: {}", target.display());
    }

    // Confirm if target is untracked, unless --yes was passed.
    if !opts.yes && target.is_file() && is_target_untracked(target)? {
        if !prompt_continue(target)? {
            eprintln!("Aborted by user.");
            return Ok(());
        }
    }

    let source_str = fs::read_to_string(source)
        .with_context(|| format!("Failed to read source {}", source.display()))?;
    let source_config = parse_yaml_config_str(&source_str)
        .with_context(|| format!("Failed to parse source {}", source.display()))?;

    let base_config = if target.is_file() {
        let target_str = fs::read_to_string(target)
            .with_context(|| format!("Failed to read target {}", target.display()))?;
        parse_yaml_config_str(&target_str)
            .with_context(|| format!("Failed to parse target {}", target.display()))?
    } else {
        // Target doesn't exist — start from empty and write the source content.
        Default::default()
    };

    let merged = merge_configs(base_config, source_config);
    let merged_str = serde_yaml::to_string(&merged)
        .context("Failed to serialize merged config")?;

    fs::write(target, merged_str)
        .with_context(|| format!("Failed to write target {}", target.display()))?;

    if !opts.keep_source {
        fs::remove_file(source)
            .with_context(|| format!("Failed to delete source {}", source.display()))?;
    }

    Ok(())
}

fn is_target_untracked(target: &Path) -> Result<bool> {
    let dir = target.parent().unwrap_or(Path::new("."));
    let relative = target.file_name()
        .ok_or_else(|| anyhow::anyhow!("target has no filename"))?;

    let status = std::process::Command::new("git")
        .arg("-C").arg(dir)
        .args(["ls-files", "--error-unmatch"])
        .arg(relative)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match status {
        Ok(s) => Ok(!s.success()),
        Err(_) => Ok(false), // conservative: not in a git repo → not "untracked" in the visitor sense
    }
}

fn prompt_continue(target: &Path) -> Result<bool> {
    eprintln!(
        "Target {} appears to be untracked. Undoing this merge is manual.",
        target.display()
    );
    eprintln!("Continue? [y/N]");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(matches!(input.trim().to_lowercase().as_str(), "y" | "yes"))
}
```

- [ ] **Step 4: Append the test module to the end of
      `src/commands/file/merge.rs`**

Paste the test code from Step 1.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib commands::file::merge`

Expected: all three tests PASS.

- [ ] **Step 6: Commit**

```bash
mise run fmt && mise run clippy && mise run test:unit
git add src/commands/file/merge.rs
git commit -m "feat(file): implement daft file merge with --keep-source and confirm prompt"
```

---

### Task 3.3: Route `daft file` through the multicall binary

**Files:**

- Modify: `src/main.rs`
- Modify: `xtask/src/main.rs`
- Modify: `src/commands/docs.rs`

- [ ] **Step 1: Add the dispatch arm**

In `src/main.rs`, add a match arm for `"file"` that calls
`commands::file::run()`. Pattern after `"config"` dispatch (which routes to
`commands::config::run()`).

- [ ] **Step 2: Add to xtask COMMANDS**

In `xtask/src/main.rs`, add `"file"` entry mirroring how `"config"` is
registered.

- [ ] **Step 3: Add to help output**

In `src/commands/docs.rs`, add the `file` verb under a sensible category (likely
"Configuration" alongside `config`).

- [ ] **Step 4: Smoke test**

```bash
cargo build
cd /tmp && mkdir daft-file-test && cd daft-file-test
git init
echo 'hooks:
  post-clone:
    jobs:
      - name: a
        run: echo a' > daft.yml
echo 'hooks:
  worktree-post-create:
    jobs:
      - name: b
        run: echo b' > extra.yml
"$OLDPWD/target/debug/daft" file merge daft.yml extra.yml --yes
cat daft.yml
ls extra.yml  # should fail — source deleted
cd .. && rm -rf daft-file-test
```

Expected: `daft.yml` contains both hooks; `extra.yml` does not exist after the
merge.

- [ ] **Step 5: Commit**

```bash
mise run fmt && mise run clippy && mise run test:unit
git add src/main.rs xtask/src/main.rs src/commands/docs.rs
git commit -m "feat(file): route daft file through the multicall binary"
```

---

### Task 3.4: YAML manual scenarios for `daft file merge`

**Files:**

- Create: `tests/manual/scenarios/file-merge/explicit-form.yaml`
- Create: `tests/manual/scenarios/file-merge/collapsed-form.yaml`
- Create: `tests/manual/scenarios/file-merge/keep-source.yaml`
- Create: `tests/manual/scenarios/file-merge/source-wins.yaml`

- [ ] **Step 1: Author scenarios**

For each scenario file, follow the structure of an existing scenario (read e.g.
`tests/manual/scenarios/checkout/basic.yaml` for reference). Each scenario
should:

1. **explicit-form.yaml**: Two YAML files with disjoint hooks. Run
   `daft file merge target.yml source.yml --yes`. Assert merged content + source
   deleted.

2. **collapsed-form.yaml**: A `daft.yml` and a `local.yml`. Run
   `daft file merge local.yml --yes`. Assert `daft.yml` now contains content
   from `local.yml` and `local.yml` is deleted.

3. **keep-source.yaml**: Same as explicit-form but with `--keep-source`. Assert
   source still exists post-merge.

4. **source-wins.yaml**: Both files define a `post-clone` hook with the same job
   name but different `run:` values. Run merge with `--yes`. Assert merged file
   contains the source's `run:` value.

- [ ] **Step 2: Run the scenarios**

Run: `mise run test:manual -- --ci file-merge`

Expected: all four scenarios pass.

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/file-merge/
git commit -m "test(file): yaml manual scenarios for daft file merge"
```

---

## Phase 4 — Branch-out propagation

### Task 4.1: Add the propagation helper

**Files:**

- Create: `src/hooks/visitor_propagation.rs`
- Modify: `src/hooks/mod.rs` (register module — check existing structure first)

- [ ] **Step 1: Find the hooks module structure**

Read `src/hooks/mod.rs` to see how submodules are registered. Note whether
`pub mod` declarations are alphabetically sorted.

- [ ] **Step 2: Write the failing tests**

Create `src/hooks/visitor_propagation.rs`:

```rust
//! Propagation of in-scope untracked daft files between worktrees.
//!
//! "In-scope" files for v1:
//!   - `daft.yml` if currently visitor (untracked) in the source worktree.
//!   - `daft.local.yml` (always treated as untracked overlay).
//!
//! Propagation writes the *resolved* content (source overlaid onto target's
//! existing content) into the target. Source wins on conflicts.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::hooks::yaml_config_loader::{
    classify_main_config, merge_configs, parse_yaml_config_str, ConfigStatus,
};
use crate::hooks::yaml_config::YamlConfig;

/// The two filenames v1 propagation handles. (find_local_config handles the
/// deprecated alias as a fallback; for *writing* propagation, we always use
/// the preferred dot-infix name.)
const VISITOR_DAFT_YML: &str = "daft.yml";
const VISITOR_DAFT_LOCAL_YML: &str = "daft.local.yml";

/// Result of a single propagation run.
#[derive(Debug, Default)]
pub struct PropagationResult {
    pub files_propagated: Vec<String>,
    pub files_skipped: Vec<String>,
}

/// Propagate in-scope untracked daft files from `source` worktree to
/// `target` worktree. The resolved content (source overlaid on target's
/// existing content) is written to the target.
pub fn propagate(source: &Path, target: &Path) -> Result<PropagationResult> {
    let mut result = PropagationResult::default();

    // daft.yml: only if source classifies as visitor
    if matches!(classify_main_config(source), ConfigStatus::Visitor) {
        propagate_one(source, target, VISITOR_DAFT_YML, &mut result)?;
    } else {
        result.files_skipped.push(VISITOR_DAFT_YML.to_string());
    }

    // daft.local.yml: always propagated if it exists in source
    let source_local = source.join(VISITOR_DAFT_LOCAL_YML);
    if source_local.is_file() {
        propagate_one(source, target, VISITOR_DAFT_LOCAL_YML, &mut result)?;
    }

    Ok(result)
}

fn propagate_one(
    source: &Path,
    target: &Path,
    filename: &str,
    result: &mut PropagationResult,
) -> Result<()> {
    let src_path = source.join(filename);
    let tgt_path = target.join(filename);

    if !src_path.is_file() {
        return Ok(());
    }

    let src_str = fs::read_to_string(&src_path)
        .with_context(|| format!("Failed to read source {}", src_path.display()))?;
    let src_cfg = parse_yaml_config_str(&src_str)
        .with_context(|| format!("Failed to parse source {}", src_path.display()))?;

    let base_cfg: YamlConfig = if tgt_path.is_file() {
        let tgt_str = fs::read_to_string(&tgt_path)
            .with_context(|| format!("Failed to read target {}", tgt_path.display()))?;
        parse_yaml_config_str(&tgt_str)
            .with_context(|| format!("Failed to parse target {}", tgt_path.display()))?
    } else {
        Default::default()
    };

    let merged = merge_configs(base_cfg, src_cfg);
    let merged_str = serde_yaml::to_string(&merged)
        .with_context(|| format!("Failed to serialize merged {}", filename))?;

    fs::write(&tgt_path, merged_str)
        .with_context(|| format!("Failed to write target {}", tgt_path.display()))?;

    result.files_propagated.push(filename.to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    fn init_git(dir: &Path) {
        Command::new("git").args(["init"]).arg(dir).output().unwrap();
        Command::new("git").arg("-C").arg(dir)
            .args(["config", "user.email", "t@t.com"]).output().unwrap();
        Command::new("git").arg("-C").arg(dir)
            .args(["config", "user.name", "T"]).output().unwrap();
    }

    #[test]
    fn test_propagate_visitor_daft_yml() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let tgt = dir.path().join("tgt");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&tgt).unwrap();
        init_git(&src);
        init_git(&tgt);

        fs::write(src.join("daft.yml"),
            "hooks:\n  post-clone:\n    jobs:\n      - name: a\n        run: echo a\n")
            .unwrap();

        let result = propagate(&src, &tgt).unwrap();
        assert!(result.files_propagated.contains(&"daft.yml".to_string()));
        assert!(tgt.join("daft.yml").is_file());
    }

    #[test]
    fn test_propagate_skips_tracked_daft_yml() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let tgt = dir.path().join("tgt");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&tgt).unwrap();
        init_git(&src);
        init_git(&tgt);

        fs::write(src.join("daft.yml"), "hooks: {}").unwrap();
        Command::new("git").arg("-C").arg(&src).args(["add", "daft.yml"]).output().unwrap();
        Command::new("git").arg("-C").arg(&src).args(["commit", "-m", "add"]).output().unwrap();

        let result = propagate(&src, &tgt).unwrap();
        assert!(!result.files_propagated.contains(&"daft.yml".to_string()));
        assert!(result.files_skipped.contains(&"daft.yml".to_string()));
    }

    #[test]
    fn test_propagate_daft_local_yml_always() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let tgt = dir.path().join("tgt");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&tgt).unwrap();
        init_git(&src);
        init_git(&tgt);

        fs::write(src.join("daft.local.yml"),
            "hooks:\n  worktree-post-create:\n    jobs:\n      - name: x\n        run: echo x\n")
            .unwrap();

        let result = propagate(&src, &tgt).unwrap();
        assert!(result.files_propagated.contains(&"daft.local.yml".to_string()));
        assert!(tgt.join("daft.local.yml").is_file());
    }

    #[test]
    fn test_propagate_merges_with_existing_target() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let tgt = dir.path().join("tgt");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&tgt).unwrap();
        init_git(&src);
        init_git(&tgt);

        fs::write(src.join("daft.yml"),
            "hooks:\n  post-clone:\n    jobs:\n      - name: src\n        run: echo src\n")
            .unwrap();
        fs::write(tgt.join("daft.yml"),
            "hooks:\n  worktree-post-create:\n    jobs:\n      - name: tgt\n        run: echo tgt\n")
            .unwrap();

        propagate(&src, &tgt).unwrap();

        let merged = fs::read_to_string(tgt.join("daft.yml")).unwrap();
        assert!(merged.contains("post-clone"));
        assert!(merged.contains("worktree-post-create"));
    }
}
```

- [ ] **Step 3: Register the module**

Add `pub mod visitor_propagation;` to `src/hooks/mod.rs` in alphabetical
position.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib hooks::visitor_propagation`

Expected: all four tests PASS.

- [ ] **Step 5: Commit**

```bash
mise run fmt && mise run clippy && mise run test:unit
git add src/hooks/visitor_propagation.rs src/hooks/mod.rs
git commit -m "feat(visitor): add propagation helper for in-scope untracked daft files"
```

---

### Task 4.2: Wire propagation into the worktree-checkout-branch flow

**Files:**

- Modify: `src/core/worktree/checkout_branch.rs` (or wherever the post-create
  insertion point lives)

- [ ] **Step 1: Locate the right insertion point**

Read `src/core/worktree/checkout_branch.rs` (or `checkout.rs`) and find where
the new worktree is materialized by git but **before** the call that runs the
user's `worktree-post-create` hooks. The propagation must happen after the
worktree exists on disk and before user hooks fire (so user hooks can read the
propagated files).

- [ ] **Step 2: Write a failing integration test**

Create or extend a bash integration test in
`tests/integration/test_visitor_propagation.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
source tests/integration/lib.sh

# Setup: bare repo with one worktree containing a visitor daft.yml
TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

cd $TMPDIR
daft init test-repo --layout sibling
cd test-repo/master

cat > daft.yml <<'EOF'
hooks:
  worktree-post-create:
    jobs:
      - name: marker
        run: echo "from-visitor"
EOF

# Branch out a new worktree
daft worktree-checkout-branch feat/foo

# Expected: feat/foo worktree should have a copy of daft.yml
if [ ! -f "../feat/foo/daft.yml" ]; then
  echo "FAIL: daft.yml not propagated to new worktree"
  exit 1
fi

if ! grep -q "from-visitor" "../feat/foo/daft.yml"; then
  echo "FAIL: propagated daft.yml does not contain expected content"
  exit 1
fi

echo "PASS"
```

Run: `bash tests/integration/test_visitor_propagation.sh`

Expected: FAIL ("daft.yml not propagated to new worktree").

- [ ] **Step 3: Add the propagation call**

In the worktree-checkout-branch flow (after worktree materialization, before
user hooks), call:

```rust
match crate::hooks::visitor_propagation::propagate(&source_worktree_path, &new_worktree_path) {
    Ok(result) => {
        for f in &result.files_propagated {
            tracing::info!("propagated {} into new worktree", f);
        }
    }
    Err(e) => {
        tracing::warn!("visitor-config propagation failed: {}", e);
        // Don't fail the worktree creation — propagation is best-effort here.
    }
}
```

Use the worktree daft was invoked from as the source. If daft isn't being
invoked from inside a worktree (rare), skip propagation gracefully.

- [ ] **Step 4: Run the integration test to verify it passes**

Run: `bash tests/integration/test_visitor_propagation.sh`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
mise run fmt && mise run clippy && mise run test:unit
git add src/core/worktree/ tests/integration/test_visitor_propagation.sh
git commit -m "feat(visitor): propagate untracked daft files on worktree-checkout-branch"
```

---

### Task 4.3: Wire propagation into `git-worktree-init` and `git-worktree-clone` flows

**Files:**

- Modify: `src/core/worktree/init.rs` and `src/core/worktree/clone.rs` (find the
  appropriate insertion point, similar to checkout-branch)

- [ ] **Step 1: Note: clone creates a fresh repo**

For `git-worktree-clone`, there is no source worktree (the user is bootstrapping
from a remote URL). Propagation does nothing — there's nothing to propagate
from. Confirm no code change is needed for `clone.rs` (just verify by
inspection).

- [ ] **Step 2: For `init.rs`, also note the same**

`git-worktree-init` creates a brand-new local repo. No source worktree. Confirm
by inspection that no code change is needed.

- [ ] **Step 3: For any other worktree-creating commands** (e.g.,
      `multi_remote`-style adoption flows)

Audit the `src/core/worktree/` directory. If any other entry point creates a new
worktree from an existing one (not a fresh clone), add the same propagation call
as in Task 4.2. Otherwise document that this audit was done by adding a comment
near the propagation call in `checkout_branch.rs`:

```rust
// Propagation entry points: checkout-branch creates worktrees from existing
// ones (this site). clone/init create fresh repos with no source worktree
// and intentionally do not propagate.
```

- [ ] **Step 4: Commit (only if there were code changes; otherwise skip)**

```bash
mise run fmt && mise run clippy && mise run test:unit
git add src/core/worktree/
git commit -m "feat(visitor): document propagation entry points across worktree creation"
```

---

## Phase 5 — `daft merge` atomic propagation

### Task 5.1: Atomic save/write/restore helper

**Files:**

- Modify: `src/hooks/visitor_propagation.rs` — add `propagate_atomic`

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `src/hooks/visitor_propagation.rs`:

```rust
#[test]
fn test_propagate_atomic_restores_on_failure() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    let tgt = dir.path().join("tgt");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&tgt).unwrap();
    init_git(&src);
    init_git(&tgt);

    fs::write(src.join("daft.yml"),
        "hooks:\n  post-clone:\n    jobs:\n      - run: echo src\n").unwrap();
    fs::write(tgt.join("daft.yml"),
        "hooks:\n  post-clone:\n    jobs:\n      - run: echo tgt-original\n").unwrap();

    let tgt_original = fs::read_to_string(tgt.join("daft.yml")).unwrap();

    // Run an atomic propagation that fails inside the action callback.
    let result = propagate_atomic(&src, &tgt, || {
        anyhow::bail!("simulated merge failure")
    });

    assert!(result.is_err());

    // Target file should be restored to its original content.
    let tgt_now = fs::read_to_string(tgt.join("daft.yml")).unwrap();
    assert_eq!(tgt_now, tgt_original, "target file should be restored on failure");
}

#[test]
fn test_propagate_atomic_persists_on_success() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    let tgt = dir.path().join("tgt");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&tgt).unwrap();
    init_git(&src);
    init_git(&tgt);

    fs::write(src.join("daft.yml"),
        "hooks:\n  worktree-post-create:\n    jobs:\n      - run: echo src\n").unwrap();
    fs::write(tgt.join("daft.yml"),
        "hooks:\n  post-clone:\n    jobs:\n      - run: echo tgt\n").unwrap();

    propagate_atomic(&src, &tgt, || Ok(())).unwrap();

    let merged = fs::read_to_string(tgt.join("daft.yml")).unwrap();
    assert!(merged.contains("worktree-post-create"));
    assert!(merged.contains("post-clone"));
}
```

- [ ] **Step 2: Run tests to see them fail to compile**

Run:
`cargo test --lib hooks::visitor_propagation::tests::test_propagate_atomic 2>&1 | head -10`

Expected: errors — `propagate_atomic` not defined.

- [ ] **Step 3: Add `propagate_atomic`**

Add to `src/hooks/visitor_propagation.rs`:

```rust
/// Save target's in-scope daft files, propagate from source, run `action`,
/// and restore the saved content if `action` returns an error.
///
/// Used by `daft merge` so that a failed git merge leaves the target
/// worktree's untracked daft files in their pre-merge state.
pub fn propagate_atomic<F>(source: &Path, target: &Path, action: F) -> Result<PropagationResult>
where
    F: FnOnce() -> Result<()>,
{
    let saved: Vec<(PathBuf, Option<String>)> = [VISITOR_DAFT_YML, VISITOR_DAFT_LOCAL_YML]
        .iter()
        .map(|f| {
            let p = target.join(f);
            let content = if p.is_file() { fs::read_to_string(&p).ok() } else { None };
            (p, content)
        })
        .collect();

    let result = propagate(source, target)?;

    match action() {
        Ok(()) => Ok(result),
        Err(e) => {
            // Restore on failure.
            for (path, original) in &saved {
                match original {
                    Some(content) => {
                        let _ = fs::write(path, content);
                    }
                    None => {
                        // File didn't exist originally — remove it if propagation created it.
                        let _ = fs::remove_file(path);
                    }
                }
            }
            Err(e)
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib hooks::visitor_propagation::tests::test_propagate_atomic`

Expected: both tests PASS.

- [ ] **Step 5: Commit**

```bash
mise run fmt && mise run clippy && mise run test:unit
git add src/hooks/visitor_propagation.rs
git commit -m "feat(visitor): add atomic propagation helper with rollback"
```

---

### Task 5.2: Wire `propagate_atomic` into `daft merge`

**Files:**

- Modify: `src/commands/merge.rs`

- [ ] **Step 1: Locate the merge orchestration**

Open `src/commands/merge.rs` and find the function that performs the git merge
against the target worktree. Look for `git merge`-style invocations or the
wrapper that bridges to `src/core/worktree/merge.rs` (or wherever the merge core
lives).

- [ ] **Step 2: Wrap the merge call**

Refactor the merge call so that the section running the actual git merge is
wrapped inside `propagate_atomic`:

```rust
use crate::hooks::visitor_propagation::propagate_atomic;

let result = propagate_atomic(&source_worktree, &target_worktree, || {
    perform_git_merge(/* ... existing arguments ... */)
})?;

for f in &result.files_propagated {
    tracing::info!("daft merge propagated {} into target", f);
}
```

`source_worktree` is the worktree of the branch being merged in;
`target_worktree` is the worktree of the receiving branch. If either branch has
no worktree, skip propagation (call the merge directly).

- [ ] **Step 3: Add an integration test for failure rollback**

Add a bash integration test at
`tests/integration/test_daft_merge_visitor_rollback.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
source tests/integration/lib.sh

TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

cd $TMPDIR
# Set up a repo with master and a branch that intentionally conflicts on
# a tracked file.
daft init test-repo --layout sibling
cd test-repo/master
echo "v1" > shared.txt
git add shared.txt
git commit -m "v1"

daft worktree-checkout-branch feat/conflict
cd ../feat/conflict
echo "v2-from-feat" > shared.txt
git commit -am "v2"

cd ../master
echo "v2-from-master" > shared.txt
git commit -am "conflict"

# Create an untracked visitor daft.yml in master with marker content.
cat > daft.yml <<'EOF'
hooks:
  post-clone:
    jobs:
      - run: echo master-original
EOF
ORIGINAL=$(cat daft.yml)

# Attempt daft merge feat/conflict into master — should fail with conflict.
if daft merge feat/conflict; then
  echo "FAIL: merge unexpectedly succeeded"
  exit 1
fi

# daft.yml in master should be restored to its pre-merge state.
NOW=$(cat daft.yml)
if [ "$ORIGINAL" != "$NOW" ]; then
  echo "FAIL: daft.yml was not restored after failed merge"
  diff <(echo "$ORIGINAL") <(echo "$NOW")
  exit 1
fi

echo "PASS"
```

Run: `bash tests/integration/test_daft_merge_visitor_rollback.sh`

Expected: PASS — the failed merge restores `daft.yml`.

- [ ] **Step 4: Commit**

```bash
mise run fmt && mise run clippy && mise run test:unit
git add src/commands/merge.rs tests/integration/test_daft_merge_visitor_rollback.sh
git commit -m "feat(visitor): atomic propagation around daft merge with rollback"
```

---

## Phase 6 — Remote-merge propagation

### Task 6.1: Gating check before remote-merge detection

**Files:**

- Modify: `src/core/worktree/branch_delete.rs`

- [ ] **Step 1: Locate the merge-detection code path**

In `src/core/worktree/branch_delete.rs`, find the existing function that
determines whether a branch has been merged (used to decide safe-to-delete).
Look for a function name containing `merged`, `is_merged`, or similar.

- [ ] **Step 2: Add the gated propagation call**

Where the merge-detection result indicates "merged", before (or alongside) the
existing safety-related logic, add a gated call:

```rust
// Gate cheapest-first: skip entirely if the source worktree doesn't have
// in-scope untracked daft files to propagate.
let source_has_inscope = source_worktree_path.is_dir() && {
    use crate::hooks::yaml_config_loader::{classify_main_config, ConfigStatus};
    matches!(classify_main_config(&source_worktree_path), ConfigStatus::Visitor)
        || source_worktree_path.join("daft.local.yml").is_file()
};

if source_has_inscope {
    // Identify the merge target's worktree and run propagation there.
    if let Some(target_wt) = find_merge_target_worktree(&merge_target_branch)? {
        let _ = crate::hooks::visitor_propagation::propagate(
            &source_worktree_path,
            &target_wt,
        );
    }
}
```

Use the existing layout-resolution helpers (`find_merge_target_worktree` or its
real name in this codebase — explore `src/core/worktree/` for the helper that
resolves a branch to its worktree path).

- [ ] **Step 3: Add a YAML manual scenario**

Create `tests/manual/scenarios/visitor-propagation/remote-merge-detection.yaml`
that:

1. Sets up a repo with master and a `feat/x` worktree.
2. Adds a visitor `daft.local.yml` to the `feat/x` worktree.
3. Simulates a remote merge of `feat/x` into master (using local commands that
   mimic the upstream merge: push to master directly, then detect via the
   prune/branch-delete safety check).
4. Runs the daft command that triggers merge detection.
5. Asserts that master's worktree now has the propagated `daft.local.yml`.

(Mirror the structure of an existing scenario for the exact YAML schema.)

Run: `mise run test:manual -- --ci visitor-propagation:remote-merge-detection`

Expected: pass.

- [ ] **Step 4: Commit**

```bash
mise run fmt && mise run clippy && mise run test:unit
git add src/core/worktree/branch_delete.rs tests/manual/scenarios/visitor-propagation/
git commit -m "feat(visitor): propagate on remote-merge detection, gated on in-scope files"
```

---

## Phase 7 — Worktree-removal safety boundary

### Task 7.1: Divergence detection

**Files:**

- Modify: `src/hooks/visitor_propagation.rs` — add `has_inscope_divergence`

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` in `src/hooks/visitor_propagation.rs`:

```rust
#[test]
fn test_divergence_when_target_missing_source_present() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    let tgt = dir.path().join("tgt");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&tgt).unwrap();
    init_git(&src);
    init_git(&tgt);

    fs::write(src.join("daft.local.yml"), "hooks: {}").unwrap();

    assert!(has_inscope_divergence(&src, &tgt).unwrap());
}

#[test]
fn test_no_divergence_when_both_missing() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    let tgt = dir.path().join("tgt");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&tgt).unwrap();
    init_git(&src);
    init_git(&tgt);

    assert!(!has_inscope_divergence(&src, &tgt).unwrap());
}

#[test]
fn test_no_divergence_when_content_matches() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    let tgt = dir.path().join("tgt");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&tgt).unwrap();
    init_git(&src);
    init_git(&tgt);

    fs::write(src.join("daft.local.yml"), "hooks: {}").unwrap();
    fs::write(tgt.join("daft.local.yml"), "hooks: {}").unwrap();

    assert!(!has_inscope_divergence(&src, &tgt).unwrap());
}

#[test]
fn test_divergence_when_content_differs() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    let tgt = dir.path().join("tgt");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&tgt).unwrap();
    init_git(&src);
    init_git(&tgt);

    fs::write(src.join("daft.local.yml"),
        "hooks:\n  post-clone:\n    jobs:\n      - run: echo src\n").unwrap();
    fs::write(tgt.join("daft.local.yml"),
        "hooks:\n  post-clone:\n    jobs:\n      - run: echo tgt\n").unwrap();

    assert!(has_inscope_divergence(&src, &tgt).unwrap());
}
```

- [ ] **Step 2: Implement `has_inscope_divergence`**

Add to `src/hooks/visitor_propagation.rs`:

```rust
/// Does the source worktree have in-scope untracked daft files whose content
/// differs from the target worktree's corresponding file?
///
/// Returns false if source has no in-scope files (nothing to lose).
/// Returns true if any in-scope file is present in source but absent in target,
/// or if both are present and the content differs.
pub fn has_inscope_divergence(source: &Path, target: &Path) -> Result<bool> {
    for filename in [VISITOR_DAFT_YML, VISITOR_DAFT_LOCAL_YML] {
        // For daft.yml, only consider it in-scope if the source classifies as visitor.
        if filename == VISITOR_DAFT_YML
            && !matches!(classify_main_config(source), ConfigStatus::Visitor)
        {
            continue;
        }

        let src_path = source.join(filename);
        let tgt_path = target.join(filename);

        if !src_path.is_file() {
            continue;
        }

        if !tgt_path.is_file() {
            return Ok(true);
        }

        let src_str = fs::read_to_string(&src_path)?;
        let tgt_str = fs::read_to_string(&tgt_path)?;
        if src_str != tgt_str {
            return Ok(true);
        }
    }

    Ok(false)
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test --lib hooks::visitor_propagation::tests::test_divergence`

Expected: all four divergence tests PASS.

- [ ] **Step 4: Commit**

```bash
mise run fmt && mise run clippy && mise run test:unit
git add src/hooks/visitor_propagation.rs
git commit -m "feat(visitor): add in-scope file divergence detection"
```

---

### Task 7.2: Wire safety boundary into worktree-removal paths

**Files:**

- Modify: the worktree-removal command paths (typically
  `src/core/worktree/branch_delete.rs` and/or `src/core/worktree/prune.rs`, plus
  their command-layer entry points in `src/commands/branch_delete.rs` and
  `src/commands/prune.rs`)

- [ ] **Step 1: Locate the removal paths**

Identify the function(s) that destroy a worktree directory. These typically live
in `src/core/worktree/branch_delete.rs` (single branch delete) and
`src/core/worktree/prune.rs` (bulk cleanup). Note: `src/commands/repo/remove.rs`
removes the entire repo; that's broader scope, do NOT add the safety boundary
there.

- [ ] **Step 2: Add a `--force` flag to the removal command(s)**

In each removal command's `clap::Args`, add:

```rust
#[arg(long = "force",
    help = "Skip divergence-of-untracked-daft-files safety check")]
force: bool,
```

- [ ] **Step 3: Insert the safety check**

Before destroying the worktree directory, if the branch has been (locally or
remotely) merged into another branch:

```rust
use crate::hooks::visitor_propagation::has_inscope_divergence;

if !args.force {
    if let Some(merge_target_wt) = find_merge_target_worktree(branch)? {
        if has_inscope_divergence(&this_worktree, &merge_target_wt)? {
            anyhow::bail!(
                "Untracked daft files in {} have diverged from the merge target {}.\n\
                 Run `daft file merge <target>/daft.local.yml {}/daft.local.yml` to \
                 consolidate, or pass --force to remove anyway.",
                this_worktree.display(),
                merge_target_wt.display(),
                this_worktree.display(),
            );
        }
    }
}
```

If the branch has no merge target (unmerged branch), skip the check — git's own
unmerged-branch protection already applies.

- [ ] **Step 4: Add a YAML manual scenario**

Create
`tests/manual/scenarios/visitor-propagation/refuse-remove-on-divergence.yaml`
covering:

1. Set up `feat/x` worktree merged into master.
2. After the merge, modify `daft.local.yml` only in `feat/x` (creating
   divergence).
3. Attempt `daft worktree-branch-delete feat/x`.
4. Assert non-zero exit + error message mentioning divergence.
5. Attempt again with `--force`.
6. Assert success and worktree gone.

Run:
`mise run test:manual -- --ci visitor-propagation:refuse-remove-on-divergence`

Expected: passes both phases.

- [ ] **Step 5: Commit**

```bash
mise run fmt && mise run clippy && mise run test:unit
git add src/commands/branch_delete.rs src/commands/prune.rs src/core/worktree/ tests/manual/scenarios/visitor-propagation/
git commit -m "feat(visitor): refuse worktree removal when untracked daft files diverged"
```

---

## Phase 8 — Doctor checks

### Task 8.1: Tracked `daft.local.yml` smell

**Files:**

- Modify: `src/doctor/hooks_checks.rs`

- [ ] **Step 1: Read the existing checks**

Open `src/doctor/hooks_checks.rs` and locate the existing config-source check
(around lines 47-87). Note the helper functions and the output pattern.

- [ ] **Step 2: Write a failing test**

Add a test to `src/doctor/hooks_checks.rs` (or an adjacent test module):

```rust
#[test]
fn test_doctor_flags_tracked_daft_local_yml() {
    let dir = tempdir().unwrap();
    Command::new("git").args(["init"]).arg(dir.path()).output().unwrap();
    Command::new("git").arg("-C").arg(dir.path())
        .args(["config", "user.email", "t@t.com"]).output().unwrap();
    Command::new("git").arg("-C").arg(dir.path())
        .args(["config", "user.name", "T"]).output().unwrap();
    std::fs::write(dir.path().join("daft.local.yml"), "hooks: {}").unwrap();
    Command::new("git").arg("-C").arg(dir.path())
        .args(["add", "daft.local.yml"]).output().unwrap();
    Command::new("git").arg("-C").arg(dir.path())
        .args(["commit", "-m", "add"]).output().unwrap();

    let report = check_tracked_local_smell(dir.path());
    assert!(report.is_smell, "tracked daft.local.yml should be flagged");
}
```

- [ ] **Step 3: Implement `check_tracked_local_smell`**

Add to `src/doctor/hooks_checks.rs`:

```rust
pub struct SmellReport {
    pub is_smell: bool,
    pub message: Option<String>,
}

/// Detect whether `daft.local.yml` (or any alias) is tracked in git — a
/// repo "smell" because the file is intended as a personal overlay.
pub fn check_tracked_local_smell(worktree_root: &Path) -> SmellReport {
    let candidates = [
        "daft.local.yml", "daft.local.yaml", ".daft.local.yml", ".daft.local.yaml",
        "daft-local.yml", "daft-local.yaml", ".daft-local.yml", ".daft-local.yaml",
    ];

    for name in &candidates {
        let path = worktree_root.join(name);
        if !path.is_file() {
            continue;
        }
        let status = std::process::Command::new("git")
            .arg("-C").arg(worktree_root)
            .args(["ls-files", "--error-unmatch", name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if let Ok(s) = status
            && s.success() {
            return SmellReport {
                is_smell: true,
                message: Some(format!(
                    "{} is tracked. It should be an untracked personal overlay. \
                     Run: git rm --cached {} && add it to .gitignore",
                    name, name
                )),
            };
        }
    }

    SmellReport { is_smell: false, message: None }
}
```

- [ ] **Step 4: Wire into doctor output**

In the existing doctor command flow (likely `src/commands/doctor.rs`), call
`check_tracked_local_smell` and add the result as a warning line if `is_smell`.

- [ ] **Step 5: Run tests to verify**

Run: `cargo test --lib doctor`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
mise run fmt && mise run clippy && mise run test:unit
git add src/doctor/hooks_checks.rs src/commands/doctor.rs
git commit -m "feat(doctor): flag tracked daft.local.yml as a repo smell"
```

---

### Task 8.2: Deprecated alias notice

**Files:**

- Modify: `src/doctor/hooks_checks.rs`

- [ ] **Step 1: Write the failing test**

Add to `src/doctor/hooks_checks.rs` tests:

```rust
#[test]
fn test_doctor_notices_deprecated_dash_alias() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("daft-local.yml"), "hooks: {}").unwrap();
    let notice = check_deprecated_local_alias(dir.path());
    assert!(notice.is_some());
}

#[test]
fn test_doctor_no_notice_when_preferred_name() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("daft.local.yml"), "hooks: {}").unwrap();
    let notice = check_deprecated_local_alias(dir.path());
    assert!(notice.is_none());
}
```

- [ ] **Step 2: Implement `check_deprecated_local_alias`**

Add to `src/doctor/hooks_checks.rs`:

```rust
/// Return a notice if a deprecated `daft-local.yml`-style alias exists in
/// the worktree root. Soft notice; not an error.
pub fn check_deprecated_local_alias(worktree_root: &Path) -> Option<String> {
    let aliases = [
        ("daft-local.yml", "daft.local.yml"),
        ("daft-local.yaml", "daft.local.yaml"),
        (".daft-local.yml", ".daft.local.yml"),
        (".daft-local.yaml", ".daft.local.yaml"),
    ];

    for (deprecated, preferred) in &aliases {
        if worktree_root.join(deprecated).is_file() {
            return Some(format!(
                "{} uses a deprecated name. Rename to {} (the dash-infix form \
                 will be removed in a future release).",
                deprecated, preferred
            ));
        }
    }
    None
}
```

- [ ] **Step 3: Wire into doctor**

In `src/commands/doctor.rs`, call `check_deprecated_local_alias` and emit the
notice (as an info-level line, not a warning).

- [ ] **Step 4: Run tests**

Run: `cargo test --lib doctor::hooks_checks::tests`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
mise run fmt && mise run clippy && mise run test:unit
git add src/doctor/hooks_checks.rs src/commands/doctor.rs
git commit -m "feat(doctor): notice for deprecated daft-local.yml alias"
```

---

### Task 8.3: Visitor classification info line

**Files:**

- Modify: `src/doctor/hooks_checks.rs`, `src/commands/doctor.rs`

- [ ] **Step 1: Extend the existing config-source check**

Locate the existing config-source check in `src/doctor/hooks_checks.rs:57-87`.
Modify it (or add a sibling helper) to include a line about classification using
`classify_main_config`:

```rust
use crate::hooks::yaml_config_loader::{classify_main_config, ConfigStatus};

pub fn describe_main_config_status(worktree_root: &Path) -> String {
    match classify_main_config(worktree_root) {
        ConfigStatus::Tracked => "daft.yml is tracked (team baseline)".to_string(),
        ConfigStatus::Visitor => "daft.yml is untracked (visitor configuration)".to_string(),
        ConfigStatus::Missing => "no daft.yml found".to_string(),
    }
}
```

- [ ] **Step 2: Wire into doctor's output**

In `src/commands/doctor.rs`, emit `describe_main_config_status(...)` as an info
line in the existing hooks/config-source section.

- [ ] **Step 3: Write a basic test**

```rust
#[test]
fn test_describe_status_missing() {
    let dir = tempdir().unwrap();
    let desc = describe_main_config_status(dir.path());
    assert!(desc.contains("no daft.yml"));
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test --lib doctor`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
mise run fmt && mise run clippy && mise run test:unit
git add src/doctor/hooks_checks.rs src/commands/doctor.rs
git commit -m "feat(doctor): show visitor/tracked classification for daft.yml"
```

---

## Phase 9 — Completions, man pages, documentation

### Task 9.1: Update shell completions

**Files:**

- Modify: `src/commands/completions/mod.rs` (`COMMANDS`, `VERB_ALIAS_GROUPS`,
  `get_command_for_name`)
- Modify: `src/commands/completions/bash.rs` (`DAFT_BASH_COMPLETIONS`)
- Modify: `src/commands/completions/zsh.rs` (`DAFT_ZSH_COMPLETIONS`)
- Modify: `src/commands/completions/fish.rs` (`DAFT_FISH_COMPLETIONS`)
- Modify: `src/commands/completions/fig.rs` (Fig spec)

- [ ] **Step 1: Add `install` and `file` to each completion source**

In each of the five files above, register the two new top-level subcommands. For
`file`, also register the `merge` subcommand. Mirror the existing patterns
(`config` is a close analogue for `file`; any other single-verb command is the
model for `install`).

- [ ] **Step 2: Regenerate and verify**

Run:
`mise run dev && daft completions bash > /tmp/daft-bash.completion && grep -E '(install|file)' /tmp/daft-bash.completion`

Expected: both verbs appear in the completion output.

Repeat for zsh and fish.

- [ ] **Step 3: Commit**

```bash
mise run fmt && mise run clippy && mise run test:unit
git add src/commands/completions/
git commit -m "feat(completions): register daft install and daft file"
```

---

### Task 9.2: Regenerate man pages

**Files:**

- Generate: `man/daft-install.1`, `man/daft-file.1`
- Possibly modify: existing man index pages if any

- [ ] **Step 1: Run the man-gen task**

Run: `mise run man:gen`

Expected: new man pages appear in `man/`. Verify by
`ls man/daft-install.1 man/daft-file.1`.

- [ ] **Step 2: Verify man pages render**

Run: `man -l man/daft-install.1` and `man -l man/daft-file.1` to spot-check
formatting.

- [ ] **Step 3: Run the man-verify check**

Run: `mise run man:verify`

Expected: passes (the generated man pages are up to date with the help text).

- [ ] **Step 4: Commit**

```bash
git add man/
git commit -m "docs(man): regenerate for daft install and daft file"
```

---

### Task 9.3: CLI reference pages

**Files:**

- Create: `docs/reference/cli/daft-install.md`
- Create: `docs/reference/cli/daft-file.md`

- [ ] **Step 1: Read an existing CLI reference for the template**

Open `docs/reference/cli/daft-doctor.md` (the template called out in CLAUDE.md).
Note the YAML frontmatter (`title`, `description`) and overall section
structure.

- [ ] **Step 2: Write `daft-install.md`**

Cover synopsis, description, options, examples (creating a starter,
refuse-on-existing), and a "See also" linking to `daft file merge` and the
visitor-configuration glossary entry.

- [ ] **Step 3: Write `daft-file.md`**

Cover the namespace (with the future `diff`/`validate`/`edit` siblings noted as
not-yet-shipped), then a `daft file merge` subsection with explicit form,
collapsed form, flags (`--keep-source`, `--yes`), confirmation behavior, and
examples.

- [ ] **Step 4: Verify docs site renders**

Run: `mise run docs:site` and visit
`http://localhost:5173/reference/cli/daft-install` and
`http://localhost:5173/reference/cli/daft-file`.

Expected: both pages render without errors.

- [ ] **Step 5: Commit**

```bash
git add docs/reference/cli/daft-install.md docs/reference/cli/daft-file.md
git commit -m "docs(cli): reference pages for daft install and daft file"
```

---

### Task 9.4: Glossary, FAQ, SKILL.md, recipe

**Files:**

- Modify: `docs/about/glossary.md`
- Modify: `docs/about/faq.md`
- Modify: `SKILL.md`
- Create: `docs/recipes/visitor-adoption.md` (verify the existing recipes
  directory naming convention first; use a path that matches)

- [ ] **Step 1: Glossary entry**

Add a "Visitor configuration" entry to `docs/about/glossary.md` (alphabetical
position):

> **Visitor configuration** — A `daft.yml` whose tracking status is "untracked".
> Daft treats untracked daft files (`daft.yml`, `daft.local.yml`) as personal
> artifacts and propagates them between worktrees on branch-out, on
> `daft merge`, and on remote-merge detection. See `daft install` to bootstrap a
> visitor configuration.

- [ ] **Step 2: FAQ entry**

Add to `docs/about/faq.md`:

> ### Do I have to commit `daft.yml` to use daft?
>
> No. Run `daft install` to create a `daft.yml` for your own use and add it to
> your `.gitignore` (or `.git/info/exclude`). Daft will treat the file as a
> visitor configuration: it stays out of git, but daft still propagates it
> between worktrees through your normal development workflow. See
> [visitor configuration](../about/glossary.md#visitor-configuration).

- [ ] **Step 3: SKILL.md update**

Add (or extend an existing) section in `SKILL.md` covering:

- How `classify_main_config` distinguishes visitor vs tracked.
- That `daft.local.yml` is the preferred name (deprecating `daft-local.yml`).
- The propagation contract: branch-out copy, atomic `daft merge` resolution,
  remote-merge detection, worktree-removal safety boundary.
- That `daft file merge` is the on-disk equivalent of the load-time overlay
  merge.
- Explicit note that collision resolution between visitor and tracked `daft.yml`
  is deferred to `daft pull` (#493).

- [ ] **Step 4: Adoption recipe**

Create `docs/recipes/visitor-adoption.md` following the rules in
`.claude/skills/writing-recipes/SKILL.md` (read it first). Walk a unilateral
adopter through:

1. Running `daft install`.
2. Adding `daft.yml` to a personal ignore (e.g., `.git/info/exclude`).
3. Customizing hooks.
4. Branching out (showing the propagation in action).
5. Optional later step: promoting the visitor file to a tracked baseline via
   `daft file merge` (with the caveat about the deferred collision design).

- [ ] **Step 5: Verify docs site builds clean**

Run: `mise run docs:site:build`

Expected: build succeeds.

- [ ] **Step 6: Commit**

```bash
git add docs/about/glossary.md docs/about/faq.md SKILL.md docs/recipes/visitor-adoption.md
git commit -m "docs: visitor configuration glossary, faq, recipe, and skill update"
```

---

## Final verification

### Task F.1: Full CI simulation

- [ ] Run `mise run ci`. All checks must pass.
- [ ] Run `mise run test:manual -- --ci` (full matrix). All scenarios must pass.
- [ ] Open `daft doctor` against a test repo that has a visitor `daft.yml`, a
      tracked `daft.local.yml`, and a deprecated `daft-local.yml`. All three new
      doctor messages should appear correctly.
- [ ] Verify the `daft install` smoke flow and the `daft file merge` smoke flow
      against `/tmp` test repos.

### Task F.2: Update CHANGELOG

The next release-plz Release PR will pick up the conventional-commit messages
and produce a CHANGELOG entry automatically. No manual CHANGELOG edit is needed.

---

## Out-of-scope (tracked elsewhere)

- Visitor-vs-tracked `daft.yml` collision detection and resolution — issue
  [#493](https://github.com/avihut/daft/issues/493) (`daft pull` command).
- Symlink-based propagation akin to shared files — future evolution.
- Cross-clone visitor-config mirror (XDG) — no current motivating use case.
- A `daft config` TUI screen — separate work; this plan reserves the `daft file`
  namespace specifically so the TUI evolution doesn't have to detangle from file
  operations.
