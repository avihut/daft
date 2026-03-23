# Layout Detection and Interactive Resolution — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Detect a repository's layout from its filesystem structure and add
interactive layout selection for unmanaged repos during `daft start`.

**Architecture:** A new `detect.rs` module reverse-matches worktree paths
against known templates. The resolver chain gains `Detected` and `Unresolved`
variants. `daft layout show` gets an optional path arg and uses detection.
`daft start` prompts users when operating on unmanaged repos, using a
`dialoguer::Select` picker.

**Tech Stack:** Rust, `dialoguer` crate (new dep), existing `console` crate, git
porcelain output parsing.

**Spec:** `docs/superpowers/specs/2026-03-23-layout-detection-design.md`

---

## File Structure

| Action | File                          | Responsibility                                                                   |
| ------ | ----------------------------- | -------------------------------------------------------------------------------- |
| Create | `src/core/layout/detect.rs`   | Detection algorithm: gather worktrees, template matching, structural detection   |
| Modify | `src/core/layout/mod.rs`      | Add `pub mod detect;`                                                            |
| Modify | `src/core/layout/resolver.rs` | Replace `Default` with `Detected`/`Unresolved`, add `DetectionResult` to context |
| Modify | `src/commands/layout.rs`      | Path arg for show, detection integration, display changes                        |
| Modify | `src/commands/checkout.rs`    | Interactive flow (detect, prompt, persist, consolidate)                          |
| Modify | `Cargo.toml`                  | Add `dialoguer` dependency                                                       |

---

### Task 1: Add `dialoguer` dependency

**Files:**

- Modify: `Cargo.toml:103` (after `console` line)

- [ ] **Step 1: Add dialoguer to Cargo.toml**

Add `dialoguer` after the `console` line in `[dependencies]`:

```toml
dialoguer = "0.11"
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check` Expected: Compiles with no errors.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add dialoguer dependency for interactive layout picker"
```

---

### Task 2: Extend `LayoutSource` enum — replace `Default` with `Detected` / `Unresolved`

**Files:**

- Modify: `src/core/layout/resolver.rs`
- Modify: `src/commands/layout.rs:367-372` (source display match)

- [ ] **Step 1: Update existing tests to expect `Unresolved` instead of
      `Default`**

In `src/core/layout/resolver.rs`, find `test_default_fallback` (line ~139).
Change the assertion:

```rust
#[test]
fn test_default_fallback() {
    let global = default_global();
    let ctx = LayoutResolutionContext {
        cli_layout: None,
        repo_store_layout: None,
        yaml_layout: None,
        global_config: &global,
        detection: None,
    };
    let (layout, source) = resolve_layout(&ctx);
    assert_eq!(layout.name, DEFAULT_LAYOUT.name());
    assert_eq!(source, LayoutSource::Unresolved);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p daft --lib resolver::tests::test_default_fallback` Expected:
FAIL — `LayoutSource::Unresolved` does not exist yet.

- [ ] **Step 3: Add `DetectionResult` enum and update
      `LayoutResolutionContext`**

In `src/core/layout/resolver.rs`, add the `DetectionResult` enum and update the
context struct:

```rust
use super::{Layout, DEFAULT_LAYOUT};
use crate::core::global_config::GlobalConfig;

/// Result of filesystem-based layout detection.
#[derive(Debug, Clone)]
pub enum DetectionResult {
    /// A layout was detected from worktree paths / structure.
    Detected(Layout),
    /// Multiple layouts matched — ambiguous.
    Ambiguous,
    /// No linked worktrees to match against and no structural cues.
    NoWorktrees,
    /// Worktrees exist but no template matched.
    NoMatch,
}

/// Inputs for layout resolution.
pub struct LayoutResolutionContext<'a> {
    pub cli_layout: Option<&'a str>,
    pub repo_store_layout: Option<&'a str>,
    pub yaml_layout: Option<&'a str>,
    pub global_config: &'a GlobalConfig,
    /// Optional detection result. Pass `None` when detection is not needed
    /// (e.g., daft clone, which always knows the layout).
    pub detection: Option<DetectionResult>,
}
```

- [ ] **Step 4: Replace `Default` with `Detected` and `Unresolved` in
      `LayoutSource`**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutSource {
    Cli,
    RepoStore,
    YamlConfig,
    GlobalConfig,
    Detected,
    Unresolved,
}
```

- [ ] **Step 5: Update `resolve_layout` to use detection and `Unresolved`**

```rust
pub fn resolve_layout(ctx: &LayoutResolutionContext) -> (Layout, LayoutSource) {
    if let Some(value) = ctx.cli_layout {
        return (
            resolve_layout_string(value, ctx.global_config),
            LayoutSource::Cli,
        );
    }
    if let Some(value) = ctx.repo_store_layout {
        return (
            resolve_layout_string(value, ctx.global_config),
            LayoutSource::RepoStore,
        );
    }
    if let Some(value) = ctx.yaml_layout {
        return (
            resolve_layout_string(value, ctx.global_config),
            LayoutSource::YamlConfig,
        );
    }
    if let Some(layout) = ctx.global_config.default_layout() {
        return (layout, LayoutSource::GlobalConfig);
    }
    // Detection (priority 5)
    if let Some(DetectionResult::Detected(layout)) = &ctx.detection {
        return (layout.clone(), LayoutSource::Detected);
    }
    // Nothing resolved — return the built-in default layout but mark as Unresolved
    (DEFAULT_LAYOUT.to_layout(), LayoutSource::Unresolved)
}
```

- [ ] **Step 6: Update all existing tests to pass `detection: None`**

Every `LayoutResolutionContext` in the tests needs `detection: None` added.
Update all test functions in the `tests` module. For example:

```rust
#[test]
fn test_cli_flag_wins() {
    let global = default_global();
    let ctx = LayoutResolutionContext {
        cli_layout: Some("contained"),
        repo_store_layout: Some("sibling"),
        yaml_layout: Some("nested"),
        global_config: &global,
        detection: None,
    };
    let (layout, source) = resolve_layout(&ctx);
    assert_eq!(layout.name, "contained");
    assert_eq!(source, LayoutSource::Cli);
}
```

Do this for ALL tests in `resolver::tests`.

- [ ] **Step 7: Add test for detection priority**

```rust
#[test]
fn test_detection_after_global_config() {
    let global = default_global();
    let detected_layout = Layout {
        name: "contained".to_string(),
        template: "{{ repo_path }}/{{ branch }}".to_string(),
        bare: None,
    };
    let ctx = LayoutResolutionContext {
        cli_layout: None,
        repo_store_layout: None,
        yaml_layout: None,
        global_config: &global,
        detection: Some(DetectionResult::Detected(detected_layout)),
    };
    let (layout, source) = resolve_layout(&ctx);
    assert_eq!(layout.name, "contained");
    assert_eq!(source, LayoutSource::Detected);
}

#[test]
fn test_detection_loses_to_repo_store() {
    let global = default_global();
    let detected_layout = Layout {
        name: "contained".to_string(),
        template: "{{ repo_path }}/{{ branch }}".to_string(),
        bare: None,
    };
    let ctx = LayoutResolutionContext {
        cli_layout: None,
        repo_store_layout: Some("sibling"),
        yaml_layout: None,
        global_config: &global,
        detection: Some(DetectionResult::Detected(detected_layout)),
    };
    let (layout, source) = resolve_layout(&ctx);
    assert_eq!(layout.name, "sibling");
    assert_eq!(source, LayoutSource::RepoStore);
}

#[test]
fn test_ambiguous_detection_falls_to_unresolved() {
    let global = default_global();
    let ctx = LayoutResolutionContext {
        cli_layout: None,
        repo_store_layout: None,
        yaml_layout: None,
        global_config: &global,
        detection: Some(DetectionResult::Ambiguous),
    };
    let (_layout, source) = resolve_layout(&ctx);
    assert_eq!(source, LayoutSource::Unresolved);
}
```

- [ ] **Step 8: Update `cmd_show` in layout.rs — change `Default` to
      `Unresolved`**

In `src/commands/layout.rs`, find the `source_display` match (line ~367):

```rust
let source_display = match source {
    LayoutSource::Cli => "CLI flag",
    LayoutSource::RepoStore => "repo setting",
    LayoutSource::YamlConfig => "daft.yml",
    LayoutSource::GlobalConfig => "global config",
    LayoutSource::Detected => "detected",
    LayoutSource::Unresolved => "default",  // temporary — will be refined in Task 5
};
```

- [ ] **Step 9: Update `resolve_checkout_layout` in checkout.rs**

Add `detection: None` to the `LayoutResolutionContext` construction at line
~488:

```rust
let (layout, _source) = resolve_layout(&LayoutResolutionContext {
    cli_layout: None,
    repo_store_layout: repo_store_layout.as_deref(),
    yaml_layout: yaml_layout.as_deref(),
    global_config: &global_config,
    detection: None,
});
```

- [ ] **Step 10: Run all tests**

Run: `cargo test -p daft --lib resolver::tests` Expected: ALL PASS.

Run: `mise run clippy` Expected: Zero warnings.

- [ ] **Step 11: Commit**

```bash
git add src/core/layout/resolver.rs src/commands/layout.rs src/commands/checkout.rs
git commit -m "refactor(layout): replace LayoutSource::Default with Detected/Unresolved"
```

---

### Task 3: Create `detect.rs` — worktree gathering and template matching

**Files:**

- Create: `src/core/layout/detect.rs`
- Modify: `src/core/layout/mod.rs` (add `pub mod detect;`)

- [ ] **Step 1: Write the test for worktree parsing**

Create `src/core/layout/detect.rs` with tests at the bottom:

```rust
//! Layout detection from filesystem structure.
//!
//! Reverse-matches worktree paths against known templates to detect the layout
//! of a repository that has no explicit layout configured.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{BuiltinLayout, Layout};
use crate::core::global_config::GlobalConfig;
use crate::core::layout::resolver::DetectionResult;
use crate::core::layout::template::{render, resolve_path, TemplateContext};
use crate::core::multi_remote::path::build_template_context;

/// Information about a single worktree gathered from `git worktree list --porcelain`.
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub is_main: bool,
}

/// Parse `git worktree list --porcelain` output into `WorktreeInfo` entries.
pub fn parse_worktree_list(porcelain: &str) -> Vec<WorktreeInfo> {
    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;
    let mut is_bare = false;

    for line in porcelain.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            // Save previous entry if any
            if let Some(path) = current_path.take() {
                if !is_bare {
                    worktrees.push(WorktreeInfo {
                        path,
                        branch: current_branch.take(),
                        is_main: worktrees.is_empty(),
                    });
                }
            }
            current_path = Some(PathBuf::from(path));
            current_branch = None;
            is_bare = false;
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            current_branch = branch_ref.strip_prefix("refs/heads/").map(String::from);
        } else if line == "bare" {
            is_bare = true;
        } else if line == "detached" {
            current_branch = None;
        }
    }
    // Don't forget the last entry
    if let Some(path) = current_path {
        if !is_bare {
            worktrees.push(WorktreeInfo {
                path,
                branch: current_branch,
                is_main: worktrees.is_empty(),
            });
        }
    }

    worktrees
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_worktree_list_basic() {
        let porcelain = "\
worktree /home/user/myproject
branch refs/heads/main

worktree /home/user/myproject.develop
branch refs/heads/develop

";
        let wts = parse_worktree_list(porcelain);
        assert_eq!(wts.len(), 2);
        assert_eq!(wts[0].path, PathBuf::from("/home/user/myproject"));
        assert_eq!(wts[0].branch.as_deref(), Some("main"));
        assert!(wts[0].is_main);
        assert_eq!(wts[1].path, PathBuf::from("/home/user/myproject.develop"));
        assert_eq!(wts[1].branch.as_deref(), Some("develop"));
        assert!(!wts[1].is_main);
    }

    #[test]
    fn test_parse_worktree_list_detached_head() {
        let porcelain = "\
worktree /home/user/myproject
branch refs/heads/main

worktree /home/user/myproject.detached
detached

";
        let wts = parse_worktree_list(porcelain);
        assert_eq!(wts.len(), 2);
        assert!(wts[1].branch.is_none());
    }

    #[test]
    fn test_parse_worktree_list_bare_entry_skipped() {
        let porcelain = "\
worktree /home/user/myproject
bare

worktree /home/user/myproject/main
branch refs/heads/main

";
        let wts = parse_worktree_list(porcelain);
        assert_eq!(wts.len(), 1);
        assert_eq!(wts[0].branch.as_deref(), Some("main"));
        assert!(wts[0].is_main);
    }
}
```

- [ ] **Step 2: Add `pub mod detect;` to `src/core/layout/mod.rs`**

After line 4 (`pub mod transform;`), add:

```rust
pub mod detect;
```

- [ ] **Step 3: Run parse tests**

Run: `cargo test -p daft --lib layout::detect::tests` Expected: ALL PASS.

- [ ] **Step 4: Write test for template matching**

Add to `detect.rs` above the `tests` module:

```rust
/// Score how well a layout matches the observed worktree paths.
struct DetectionScore {
    layout: Layout,
    matches: usize,
    total: usize,
}

/// Count the number of `|` filter operators in a template.
fn filter_count(template: &str) -> usize {
    let mut count = 0;
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after_open = &rest[start + 2..];
        if let Some(end) = after_open.find("}}") {
            let expr = &after_open[..end];
            count += expr.matches('|').count();
            rest = &after_open[end + 2..];
        } else {
            break;
        }
    }
    count
}

/// Attempt to match worktree paths against a list of candidate layouts.
///
/// Returns `DetectionResult::Detected` if exactly one layout matches (after
/// filtering by bare compatibility). Returns `Ambiguous` if multiple distinct
/// layouts match different worktrees. Returns `NoMatch` if nothing matched.
pub fn match_templates(
    worktrees: &[WorktreeInfo],
    project_root: &Path,
    is_bare: bool,
    candidates: &[Layout],
) -> DetectionResult {
    let branchable: Vec<&WorktreeInfo> = worktrees
        .iter()
        .filter(|wt| wt.branch.is_some() && !wt.is_main)
        .collect();

    if branchable.is_empty() {
        return DetectionResult::NoMatch;
    }

    let total = branchable.len();
    let mut scores: Vec<DetectionScore> = Vec::new();

    for layout in candidates {
        // Filter by bare compatibility
        if layout.needs_bare() != is_bare {
            continue;
        }

        let mut matches = 0;
        for wt in &branchable {
            let branch = wt.branch.as_deref().unwrap();
            let ctx = build_template_context(project_root, branch);
            if let Ok(rendered) = render(&layout.template, &ctx) {
                if let Ok(expected_path) = resolve_path(&rendered, &ctx.repo_path) {
                    if wt.path == expected_path {
                        matches += 1;
                    }
                }
            }
        }

        if matches > 0 {
            scores.push(DetectionScore {
                layout: layout.clone(),
                matches,
                total,
            });
        }
    }

    if scores.is_empty() {
        return DetectionResult::NoMatch;
    }

    if scores.len() == 1 {
        return DetectionResult::Detected(scores.into_iter().next().unwrap().layout);
    }

    // Multiple layouts matched — check if they all have the same score
    // (templates produce identical paths for the branches present).
    let all_same_score = scores.iter().all(|s| s.matches == scores[0].matches);
    if all_same_score {
        // Tiebreaker: fewer filter operators wins, then builtin order.
        scores.sort_by_key(|s| filter_count(&s.layout.template));
        return DetectionResult::Detected(scores.into_iter().next().unwrap().layout);
    }

    // Different layouts matched different worktrees — ambiguous.
    DetectionResult::Ambiguous
}
```

Add tests:

```rust
#[test]
fn test_match_templates_sibling_detected() {
    let worktrees = vec![
        WorktreeInfo {
            path: PathBuf::from("/home/user/myproject"),
            branch: Some("main".into()),
            is_main: true,
        },
        WorktreeInfo {
            path: PathBuf::from("/home/user/myproject.develop"),
            branch: Some("develop".into()),
            is_main: false,
        },
    ];
    let candidates: Vec<Layout> = BuiltinLayout::all()
        .iter()
        .map(|b| b.to_layout())
        .collect();
    let result = match_templates(
        &worktrees,
        Path::new("/home/user/myproject"),
        false,
        &candidates,
    );
    match result {
        DetectionResult::Detected(layout) => assert_eq!(layout.name, "sibling"),
        other => panic!("Expected Detected, got {:?}", other),
    }
}

#[test]
fn test_match_templates_contained_detected() {
    let worktrees = vec![
        WorktreeInfo {
            path: PathBuf::from("/home/user/myproject/main"),
            branch: Some("main".into()),
            is_main: true,
        },
        WorktreeInfo {
            path: PathBuf::from("/home/user/myproject/develop"),
            branch: Some("develop".into()),
            is_main: false,
        },
    ];
    let candidates: Vec<Layout> = BuiltinLayout::all()
        .iter()
        .map(|b| b.to_layout())
        .collect();
    let result = match_templates(
        &worktrees,
        Path::new("/home/user/myproject"),
        true,
        &candidates,
    );
    match result {
        DetectionResult::Detected(layout) => assert_eq!(layout.name, "contained"),
        other => panic!("Expected Detected, got {:?}", other),
    }
}

#[test]
fn test_match_templates_no_linked_worktrees_returns_no_match() {
    let worktrees = vec![WorktreeInfo {
        path: PathBuf::from("/home/user/myproject"),
        branch: Some("main".into()),
        is_main: true,
    }];
    let candidates: Vec<Layout> = BuiltinLayout::all()
        .iter()
        .map(|b| b.to_layout())
        .collect();
    let result = match_templates(
        &worktrees,
        Path::new("/home/user/myproject"),
        false,
        &candidates,
    );
    assert!(matches!(result, DetectionResult::NoMatch));
}

#[test]
fn test_match_templates_contained_tiebreaker_prefers_fewer_filters() {
    // When branches have no slashes, contained and contained-flat produce
    // the same paths. Tiebreaker should prefer contained (0 filters vs 1).
    let worktrees = vec![
        WorktreeInfo {
            path: PathBuf::from("/home/user/myproject/main"),
            branch: Some("main".into()),
            is_main: true,
        },
        WorktreeInfo {
            path: PathBuf::from("/home/user/myproject/develop"),
            branch: Some("develop".into()),
            is_main: false,
        },
    ];
    let candidates: Vec<Layout> = BuiltinLayout::all()
        .iter()
        .map(|b| b.to_layout())
        .collect();
    // Both contained (bare) and contained-flat (bare) match, but
    // contained-classic (non-bare) is filtered out by bare compatibility.
    let result = match_templates(
        &worktrees,
        Path::new("/home/user/myproject"),
        true,
        &candidates,
    );
    match result {
        DetectionResult::Detected(layout) => assert_eq!(layout.name, "contained"),
        other => panic!("Expected Detected(contained), got {:?}", other),
    }
}

#[test]
fn test_filter_count() {
    assert_eq!(filter_count("{{ repo_path }}/{{ branch }}"), 0);
    assert_eq!(filter_count("{{ repo_path }}/{{ branch | repo }}"), 1);
    assert_eq!(filter_count("{{ repo }}.{{ branch | sanitize }}"), 1);
    assert_eq!(
        filter_count("{{ repo_path }}/{{ branch | repo | sanitize }}"),
        2
    );
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p daft --lib layout::detect::tests` Expected: ALL PASS.

- [ ] **Step 6: Commit**

```bash
git add src/core/layout/detect.rs src/core/layout/mod.rs
git commit -m "feat(layout): add detection module with template matching"
```

---

### Task 4: Add structural detection to `detect.rs`

**Files:**

- Modify: `src/core/layout/detect.rs`

- [ ] **Step 1: Write structural detection tests**

Add these tests to the `tests` module:

```rust
#[test]
fn test_structural_detect_contained_bare_with_child_worktree() {
    // Bare repo at /home/user/myproject/.git, worktree at /home/user/myproject/main
    let result = detect_structure(
        Path::new("/home/user/myproject/.git"),
        Path::new("/home/user/myproject"),
        true,  // core.bare
        &[WorktreeInfo {
            path: PathBuf::from("/home/user/myproject/main"),
            branch: Some("main".into()),
            is_main: true,
        }],
    );
    match result {
        DetectionResult::Detected(layout) => assert_eq!(layout.name, "contained"),
        other => panic!("Expected Detected(contained), got {:?}", other),
    }
}

#[test]
fn test_structural_detect_contained_classic() {
    // Non-bare repo at /home/user/myproject/main/.git, worktree at /home/user/myproject/main
    let result = detect_structure(
        Path::new("/home/user/myproject/main/.git"),
        Path::new("/home/user/myproject"),
        false,  // core.bare
        &[WorktreeInfo {
            path: PathBuf::from("/home/user/myproject/main"),
            branch: Some("main".into()),
            is_main: true,
        }],
    );
    match result {
        DetectionResult::Detected(layout) => assert_eq!(layout.name, "contained-classic"),
        other => panic!("Expected Detected(contained-classic), got {:?}", other),
    }
}

#[test]
fn test_structural_detect_nested_has_worktrees_dir() {
    // Non-bare repo at /home/user/myproject/.git, .worktrees/ exists
    // We test this via the public detect function, but for unit testing
    // the structural logic directly:
    let result = detect_structure_with_worktrees_dir(
        Path::new("/home/user/myproject/.git"),
        Path::new("/home/user/myproject"),
        false,
        &[WorktreeInfo {
            path: PathBuf::from("/home/user/myproject"),
            branch: Some("main".into()),
            is_main: true,
        }],
        true,  // .worktrees/ directory exists
    );
    match result {
        DetectionResult::Detected(layout) => assert_eq!(layout.name, "nested"),
        other => panic!("Expected Detected(nested), got {:?}", other),
    }
}

#[test]
fn test_structural_detect_plain_clone_no_detection() {
    // Non-bare repo at /home/user/myproject/.git, main worktree is the root
    let result = detect_structure(
        Path::new("/home/user/myproject/.git"),
        Path::new("/home/user/myproject"),
        false,
        &[WorktreeInfo {
            path: PathBuf::from("/home/user/myproject"),
            branch: Some("main".into()),
            is_main: true,
        }],
    );
    assert!(matches!(result, DetectionResult::NoWorktrees));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p daft --lib layout::detect::tests::test_structural` Expected:
FAIL — `detect_structure` does not exist yet.

- [ ] **Step 3: Implement structural detection**

Add to `detect.rs` (above the `tests` module):

```rust
/// Structural detection when template matching is inconclusive.
///
/// Uses the position of `.git`, `core.bare`, and main worktree path to
/// identify the layout family.
pub fn detect_structure(
    git_common_dir: &Path,
    project_root: &Path,
    is_bare: bool,
    worktrees: &[WorktreeInfo],
) -> DetectionResult {
    let has_worktrees_dir = project_root.join(".worktrees").is_dir();
    detect_structure_with_worktrees_dir(
        git_common_dir,
        project_root,
        is_bare,
        worktrees,
        has_worktrees_dir,
    )
}

/// Testable version that takes `.worktrees/` existence as a parameter.
pub fn detect_structure_with_worktrees_dir(
    git_common_dir: &Path,
    project_root: &Path,
    is_bare: bool,
    worktrees: &[WorktreeInfo],
    has_worktrees_dir: bool,
) -> DetectionResult {
    let main_wt = worktrees.iter().find(|wt| wt.is_main);
    let Some(main_wt) = main_wt else {
        return DetectionResult::NoWorktrees;
    };

    // Contained-classic: .git is inside a named subdirectory, non-bare,
    // and the subdirectory name matches the branch.
    if !is_bare {
        if let (Some(git_parent), Some(branch)) =
            (git_common_dir.parent(), main_wt.branch.as_deref())
        {
            let git_parent_name = git_parent
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            // .git is inside a subdir of project_root, and that subdir
            // is named after the branch
            if git_parent != project_root
                && git_parent.parent() == Some(project_root)
                && git_parent_name == branch
            {
                return DetectionResult::Detected(
                    BuiltinLayout::ContainedClassic.to_layout(),
                );
            }
        }
    }

    // Contained (bare): worktrees are direct children of repo root
    if is_bare {
        if let Some(branch) = main_wt.branch.as_deref() {
            let expected_child = project_root.join(branch);
            if main_wt.path == expected_child {
                return DetectionResult::Detected(
                    BuiltinLayout::Contained.to_layout(),
                );
            }
        }
    }

    // Main worktree IS the repo root
    if main_wt.path == project_root {
        // Nested: .worktrees/ directory exists
        if has_worktrees_dir {
            return DetectionResult::Detected(BuiltinLayout::Nested.to_layout());
        }
        // Plain git clone — cannot distinguish sibling/nested/centralized
        return DetectionResult::NoWorktrees;
    }

    DetectionResult::NoWorktrees
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p daft --lib layout::detect::tests` Expected: ALL PASS.

- [ ] **Step 5: Commit**

```bash
git add src/core/layout/detect.rs
git commit -m "feat(layout): add structural detection for single-worktree repos"
```

---

### Task 5: Create the public `detect_layout` entry point

**Files:**

- Modify: `src/core/layout/detect.rs`

- [ ] **Step 1: Write test for the main entry point**

```rust
#[test]
fn test_detect_layout_sibling_from_porcelain() {
    let porcelain = "\
worktree /home/user/myproject
branch refs/heads/main

worktree /home/user/myproject.develop
branch refs/heads/develop

";
    let result = detect_layout_from_porcelain(
        porcelain,
        Path::new("/home/user/myproject/.git"),
        Path::new("/home/user/myproject"),
        false,
        &GlobalConfig::default(),
    );
    match result {
        DetectionResult::Detected(layout) => assert_eq!(layout.name, "sibling"),
        other => panic!("Expected Detected(sibling), got {:?}", other),
    }
}

#[test]
fn test_detect_layout_plain_clone_no_worktrees() {
    let porcelain = "\
worktree /home/user/myproject
branch refs/heads/main

";
    let result = detect_layout_from_porcelain(
        porcelain,
        Path::new("/home/user/myproject/.git"),
        Path::new("/home/user/myproject"),
        false,
        &GlobalConfig::default(),
    );
    assert!(matches!(result, DetectionResult::NoWorktrees));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p daft --lib layout::detect::tests::test_detect_layout`
Expected: FAIL.

- [ ] **Step 3: Implement the entry point**

```rust
/// Main detection entry point — takes porcelain output (for testability).
///
/// Tries template matching first. Falls back to structural detection.
pub fn detect_layout_from_porcelain(
    porcelain: &str,
    git_common_dir: &Path,
    project_root: &Path,
    is_bare: bool,
    global_config: &GlobalConfig,
) -> DetectionResult {
    let worktrees = parse_worktree_list(porcelain);

    if worktrees.is_empty() {
        return DetectionResult::NoWorktrees;
    }

    // Build candidate list: builtins + custom layouts from global config
    let mut candidates: Vec<Layout> = BuiltinLayout::all()
        .iter()
        .map(|b| b.to_layout())
        .collect();
    if let Some(custom) = global_config.custom_layouts() {
        candidates.extend(custom);
    }

    // All-detached check: if no linked worktree has a branch, skip
    // template matching and go straight to structural detection.
    let has_branchable_linked = worktrees
        .iter()
        .any(|wt| !wt.is_main && wt.branch.is_some());

    if has_branchable_linked {
        let result = match_templates(&worktrees, project_root, is_bare, &candidates);
        if matches!(result, DetectionResult::Detected(_)) {
            return result;
        }
    }

    // Fallback to structural detection
    detect_structure(git_common_dir, project_root, is_bare, &worktrees)
}

/// Live detection — calls `git worktree list --porcelain` and `git config`.
///
/// Derives `project_root` from `git_common_dir` and `core.bare`.
pub fn detect_layout(
    git_common_dir: &Path,
    global_config: &GlobalConfig,
) -> DetectionResult {
    use crate::git::GitCommand;

    let git = GitCommand::new(true);

    // Read core.bare
    let is_bare = git
        .config_get("core.bare")
        .ok()
        .flatten()
        .is_some_and(|v| v.to_lowercase() == "true");

    // Derive project_root
    let project_root = if is_bare {
        git_common_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| git_common_dir.to_path_buf())
    } else {
        // For non-bare, check if this might be contained-classic
        // (.git inside a branch subdir inside a wrapper).
        // project_root = grandparent of .git if wrapped, else parent.
        let parent = git_common_dir.parent().unwrap_or(git_common_dir);
        // If parent != the worktree root (i.e., there's a wrapper above),
        // use the grandparent. We'll let structural detection sort out
        // which is correct.
        parent.to_path_buf()
    };

    // Get worktree list
    let porcelain = match git.worktree_list_porcelain() {
        Ok(p) => p,
        Err(_) => return DetectionResult::NoWorktrees,
    };

    // Try with direct parent first
    let result = detect_layout_from_porcelain(
        &porcelain,
        git_common_dir,
        &project_root,
        is_bare,
        global_config,
    );

    // If no match and non-bare, also try grandparent as project_root
    // (contained-classic case: .git is at wrapper/branch/.git)
    if matches!(result, DetectionResult::NoWorktrees | DetectionResult::NoMatch) && !is_bare {
        if let Some(grandparent) = project_root.parent() {
            if grandparent != project_root {
                let retry = detect_layout_from_porcelain(
                    &porcelain,
                    git_common_dir,
                    grandparent,
                    is_bare,
                    global_config,
                );
                if matches!(retry, DetectionResult::Detected(_)) {
                    return retry;
                }
            }
        }
    }

    result
}
```

- [ ] **Step 4: Add `custom_layouts()` method to `GlobalConfig`**

In `src/core/global_config.rs`, add this method to the `impl GlobalConfig` block
(after the existing `resolve_layout_by_name` method):

```rust
/// Returns all custom layouts defined in `[layouts.*]` config sections.
pub fn custom_layouts(&self) -> Vec<Layout> {
    self.layouts
        .iter()
        .map(|(name, def)| Layout {
            name: name.clone(),
            template: def.template.clone(),
            bare: def.bare,
        })
        .collect()
}
```

Update the call site in `detect_layout_from_porcelain`:

```rust
candidates.extend(global_config.custom_layouts());
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p daft --lib layout::detect::tests` Expected: ALL PASS.

- [ ] **Step 6: Commit**

```bash
git add src/core/layout/detect.rs src/core/global_config.rs
git commit -m "feat(layout): add detect_layout entry point with structural fallback"
```

---

### Task 6: Add optional path argument to `daft layout show`

**Files:**

- Modify: `src/commands/layout.rs`

- [ ] **Step 1: Add path argument to Show subcommand**

In `src/commands/layout.rs`, the `LayoutCommand` enum has `Show` as a unit
variant. Change it to accept args:

```rust
#[derive(Subcommand)]
enum LayoutCommand {
    /// List all available layouts
    List,
    /// Show the resolved layout for the current repo
    Show(ShowArgs),
    /// Transform the current repo to a different layout
    Transform(TransformArgs),
    /// View or set the global default layout
    Default(DefaultArgs),
}

#[derive(Args)]
struct ShowArgs {
    /// Path to a git repository (defaults to current directory)
    path: Option<PathBuf>,
}
```

- [ ] **Step 2: Update the `run()` dispatch to pass the new args**

Find the `match` in `run()` that dispatches to `cmd_show`. Change:

```rust
Some(LayoutCommand::Show(args)) => cmd_show(&args, &mut output),
None => cmd_show(&ShowArgs { path: None }, &mut output),
```

- [ ] **Step 3: Update `cmd_show` to accept `ShowArgs` and use the path**

```rust
/// RAII guard that restores the working directory on drop.
struct CwdGuard {
    original: Option<PathBuf>,
}

impl CwdGuard {
    fn new(target: &Path) -> Result<Self> {
        let original = std::env::current_dir().ok();
        std::env::set_current_dir(target)
            .with_context(|| format!("Cannot cd to {}", target.display()))?;
        Ok(Self { original })
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        if let Some(ref old) = self.original {
            let _ = std::env::set_current_dir(old);
        }
    }
}

fn cmd_show(args: &ShowArgs, output: &mut dyn Output) -> Result<()> {
    // Temporarily cd to target dir if provided, for git commands.
    // The CwdGuard restores CWD on drop (including early returns via ?).
    let _guard = if let Some(ref path) = args.path {
        let resolved = std::fs::canonicalize(path)
            .with_context(|| format!("Cannot resolve path: {}", path.display()))?;
        Some(CwdGuard::new(&resolved)?)
    } else {
        None
    };

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository. Run this command from within a repo.");
    }

    let global_config = GlobalConfig::load().unwrap_or_default();
    let git_dir = get_git_common_dir()?;
    let trust_db = TrustDatabase::load().unwrap_or_default();

    let yaml_layout: Option<String> = get_current_worktree_path()
        .ok()
        .and_then(|wt| yaml_config_loader::load_merged_config(&wt).ok().flatten())
        .and_then(|cfg| cfg.layout);

    let repo_store_layout = trust_db.get_layout(&git_dir).map(String::from);

    // Run detection only when no explicit layout is configured
    let detection = if repo_store_layout.is_none() && yaml_layout.is_none() {
        Some(crate::core::layout::detect::detect_layout(
            &git_dir,
            &global_config,
        ))
    } else {
        None
    };

    let (layout, source) = resolve_layout(&LayoutResolutionContext {
        cli_layout: None,
        repo_store_layout: repo_store_layout.as_deref(),
        yaml_layout: yaml_layout.as_deref(),
        global_config: &global_config,
        detection: detection.clone(),
    });

    // Display
    match source {
        LayoutSource::Unresolved => {
            let msg = match &detection {
                Some(DetectionResult::NoWorktrees) | None => {
                    "No layout (no worktrees to detect from)"
                }
                Some(DetectionResult::NoMatch) => {
                    "Unknown layout (worktrees don't match any known template)"
                }
                Some(DetectionResult::Ambiguous) => {
                    "Unknown layout (worktrees match multiple templates)"
                }
                Some(DetectionResult::Detected(_)) => unreachable!(),
            };
            output.info(msg);
        }
        _ => {
            let source_display = match source {
                LayoutSource::Cli => "CLI flag",
                LayoutSource::RepoStore => "repo setting",
                LayoutSource::YamlConfig => "daft.yml",
                LayoutSource::GlobalConfig => "global config",
                LayoutSource::Detected => "detected",
                LayoutSource::Unresolved => unreachable!(),
            };
            let use_color = styles::colors_enabled();
            let template_display = if use_color {
                highlight_template(&layout.template)
            } else {
                layout.template.clone()
            };
            output.info(&format!(
                "{} {} {}",
                bold(&layout.name),
                template_display,
                dim(&format!("({source_display})"))
            ));
        }
    }

    Ok(())
}
```

Note: The CWD-switching approach is pragmatic and matches how git itself works.
A future refactor could extract a `GitContext::at(path)` to avoid global state.

- [ ] **Step 4: Run existing show tests**

Run: `mise run test:manual -- --ci layout:show` Expected: PASS (backward
compatible).

- [ ] **Step 5: Commit**

```bash
git add src/commands/layout.rs
git commit -m "feat(layout): add optional path argument to daft layout show"
```

---

### Task 7: Add multi-remote detection skip

**Files:**

- Modify: `src/core/layout/detect.rs`

- [ ] **Step 1: Write test for multi-remote skip**

Add to `detect.rs` tests:

```rust
#[test]
fn test_detect_layout_skips_multi_remote() {
    let porcelain = "\
worktree /home/user/myproject
branch refs/heads/main

worktree /home/user/myproject.develop
branch refs/heads/develop

";
    let result = detect_layout_from_porcelain(
        porcelain,
        Path::new("/home/user/myproject/.git"),
        Path::new("/home/user/myproject"),
        false,
        &GlobalConfig::default(),
        true,  // multi_remote_enabled
    );
    assert!(matches!(result, DetectionResult::NoMatch));
}
```

- [ ] **Step 2: Add `multi_remote_enabled` parameter to detection functions**

Update `detect_layout_from_porcelain` signature to accept
`multi_remote_enabled: bool`. At the top of the function, add:

```rust
if multi_remote_enabled {
    return DetectionResult::NoMatch;
}
```

Update `detect_layout` to check for multi-remote mode. Load `DaftSettings` or
check `yaml_config` for multi-remote configuration, then pass the flag through:

```rust
let settings = crate::settings::DaftSettings::load_global().unwrap_or_default();
let multi_remote_enabled = settings.multi_remote_enabled;
```

- [ ] **Step 3: Update all existing call sites of
      `detect_layout_from_porcelain`**

Add `false` for multi_remote_enabled in all unit tests that call
`detect_layout_from_porcelain` directly.

- [ ] **Step 4: Run tests**

Run: `cargo test -p daft --lib layout::detect::tests` Expected: ALL PASS.

- [ ] **Step 5: Commit**

```bash
git add src/core/layout/detect.rs
git commit -m "feat(layout): skip detection for multi-remote repos"
```

---

### Task 8: YAML integration tests for detection and path arg

**Files:**

- Create: `tests/manual/scenarios/layout/detect-show-plain-clone.yml`
- Create: `tests/manual/scenarios/layout/detect-show-contained.yml`
- Create: `tests/manual/scenarios/layout/detect-show-sibling.yml`
- Create: `tests/manual/scenarios/layout/detect-show-nested.yml`
- Create: `tests/manual/scenarios/layout/detect-show-contained-classic.yml`
- Create: `tests/manual/scenarios/layout/detect-show-path-arg.yml`

- [ ] **Step 1: Create plain clone detection test**

`tests/manual/scenarios/layout/detect-show-plain-clone.yml`:

```yaml
name: Layout show on plain git clone
description: daft layout show reports no layout for a plain git clone

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone with plain git
    run: git clone $REMOTE_TEST_REPO test-repo
    expect:
      exit_code: 0

  - name: Show layout
    run: NO_COLOR=1 daft layout show 2>&1
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0
      output_contains:
        - "No layout"
```

- [ ] **Step 2: Create contained detection test**

`tests/manual/scenarios/layout/detect-show-contained.yml`:

```yaml
name: Layout show detects contained layout
description: >
  daft layout show detects the contained layout on a repo set up without daft's
  knowledge

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Set up contained repo manually
    run: |
      git clone --bare $REMOTE_TEST_REPO test-repo/.git
      cd test-repo
      git worktree add main main
      git worktree add develop develop
    expect:
      exit_code: 0

  - name: Show layout detects contained
    run: NO_COLOR=1 daft layout show 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "contained"
        - "detected"
```

- [ ] **Step 3: Create sibling detection test**

`tests/manual/scenarios/layout/detect-show-sibling.yml`:

```yaml
name: Layout show detects sibling layout
description: >
  A repo with sibling-pattern worktrees created manually is detected as sibling

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone normally
    run: git clone $REMOTE_TEST_REPO test-repo
    expect:
      exit_code: 0

  - name: Add worktree in sibling pattern
    run: cd test-repo && git worktree add ../test-repo.develop develop
    expect:
      exit_code: 0

  - name: Show layout detects sibling
    run: NO_COLOR=1 daft layout show 2>&1
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0
      output_contains:
        - "sibling"
        - "detected"
```

- [ ] **Step 4: Create nested detection test**

`tests/manual/scenarios/layout/detect-show-nested.yml`:

```yaml
name: Layout show detects nested layout
description: >
  A repo with .worktrees/ directory is detected as nested

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone normally
    run: git clone $REMOTE_TEST_REPO test-repo
    expect:
      exit_code: 0

  - name: Add worktree in nested pattern
    run: cd test-repo && git worktree add .worktrees/develop develop
    expect:
      exit_code: 0

  - name: Show layout detects nested
    run: NO_COLOR=1 daft layout show 2>&1
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0
      output_contains:
        - "nested"
        - "detected"
```

- [ ] **Step 5: Create contained-classic detection test**

`tests/manual/scenarios/layout/detect-show-contained-classic.yml`:

```yaml
name: Layout show detects contained-classic layout
description: >
  A repo with .git inside a branch subdir is detected as contained-classic

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Set up contained-classic manually
    run: |
      mkdir test-repo
      cd test-repo
      git clone $REMOTE_TEST_REPO main
    expect:
      exit_code: 0

  - name: Show layout detects contained-classic
    run: NO_COLOR=1 daft layout show 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "contained-classic"
        - "detected"
```

- [ ] **Step 6: Create path argument test**

`tests/manual/scenarios/layout/detect-show-path-arg.yml`:

```yaml
name: Layout show with path argument
description: >
  daft layout show accepts a path argument and reports the layout of that repo

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone with contained layout
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Show layout using path argument from different directory
    run: NO_COLOR=1 daft layout show $WORK_DIR/test-repo/main 2>&1
    cwd: "$WORK_DIR"
    expect:
      exit_code: 0
      output_contains:
        - "contained"
        - "repo setting"
```

- [ ] **Step 7: Run all detection tests**

Run: `mise run test:manual -- --ci layout:detect-show` Expected: ALL PASS.

- [ ] **Step 8: Commit**

```bash
git add tests/manual/scenarios/layout/detect-show-*.yml
git commit -m "test(layout): add YAML scenarios for layout detection"
```

---

### Task 9: Interactive flow in `daft start` — detection + prompt + persist

**Files:**

- Modify: `src/commands/checkout.rs`

- [ ] **Step 1: Update `resolve_checkout_layout` to return source and run
      detection**

```rust
fn resolve_checkout_layout(
    git: &GitCommand,
    output: &mut dyn Output,
) -> (crate::core::layout::Layout, LayoutSource) {
    let global_config = GlobalConfig::load().unwrap_or_default();
    let git_dir = get_git_common_dir().ok();
    let trust_db = TrustDatabase::load().unwrap_or_default();

    let yaml_layout: Option<String> = get_current_worktree_path()
        .ok()
        .and_then(|wt| yaml_config_loader::load_merged_config(&wt).ok().flatten())
        .and_then(|cfg| cfg.layout);

    let repo_store_layout = git_dir
        .as_ref()
        .and_then(|d| trust_db.get_layout(d).map(String::from));

    // Run detection only if no explicit layout is set
    let detection = if repo_store_layout.is_none() && yaml_layout.is_none() {
        git_dir.as_ref().map(|d| {
            crate::core::layout::detect::detect_layout(d, &global_config)
        })
    } else {
        None
    };

    let (layout, source) = resolve_layout(&LayoutResolutionContext {
        cli_layout: None,
        repo_store_layout: repo_store_layout.as_deref(),
        yaml_layout: yaml_layout.as_deref(),
        global_config: &global_config,
        detection,
    });

    // Graceful degradation warning
    let is_bare = git
        .config_get("core.bare")
        .ok()
        .flatten()
        .is_some_and(|v| v.to_lowercase() == "true");
    if layout.needs_bare() && !is_bare {
        output.warning(&format!(
            "Layout '{}' works best with a bare repository. \
             Consider running `daft layout transform` to convert.",
            layout.name
        ));
    }

    (layout, source)
}
```

Note: the `detection` variable from `git_dir.map(...)` produces
`Option<DetectionResult>`. When `git_dir` is `None`, detection is `None`. When
`git_dir` is `Some`, detection is `Some(detect_layout(...))`.

- [ ] **Step 2: Add the interactive layout flow function**

Add a new function in `checkout.rs`:

```rust
use std::io::IsTerminal;
use crate::core::layout::{BuiltinLayout, Layout};
use crate::core::layout::resolver::LayoutSource;

/// Interactive layout resolution for unmanaged repos.
///
/// Returns the layout to use and whether to persist it.
fn interactive_layout_resolution(
    layout: &Layout,
    source: LayoutSource,
    output: &mut dyn Output,
) -> Result<(Layout, bool)> {
    // Non-interactive: use what we have
    if !std::io::stdin().is_terminal() || std::env::var("DAFT_TESTING").is_ok() {
        return Ok((layout.clone(), source == LayoutSource::Detected));
    }

    match source {
        // Known layout — use it
        LayoutSource::Cli
        | LayoutSource::RepoStore
        | LayoutSource::YamlConfig
        | LayoutSource::GlobalConfig => Ok((layout.clone(), false)),

        // Detected — confirm with user
        LayoutSource::Detected => {
            output.info(&format!(
                "Detected layout: {}",
                crate::styles::bold(&layout.name),
            ));

            let confirmed = dialoguer::Confirm::with_theme(
                &dialoguer::theme::ColorfulTheme::default(),
            )
            .with_prompt("Use this layout for future worktrees?")
            .default(true)
            .interact()?;

            if confirmed {
                Ok((layout.clone(), true))
            } else {
                let chosen = show_layout_picker(Some(layout))?;
                Ok((chosen, true))
            }
        }

        // Unresolved — pick a layout
        LayoutSource::Unresolved => {
            // Flow A: no worktrees (plain git clone, first daft start)
            // Silently use default — don't prompt
            // We detect this because the layout IS the default and source is Unresolved
            let git = crate::git::GitCommand::new(true);
            let has_linked_worktrees = git
                .worktree_list_porcelain()
                .ok()
                .map(|p| {
                    crate::core::layout::detect::parse_worktree_list(&p)
                        .iter()
                        .any(|wt| !wt.is_main)
                })
                .unwrap_or(false);

            if !has_linked_worktrees {
                // Flow A: silent default
                return Ok((layout.clone(), true));
            }

            // Flow C: unknown layout with existing worktrees
            output.info("Found worktrees in an unrecognized arrangement.");
            output.info("Choose a layout for new worktrees:");
            let chosen = show_layout_picker(None)?;
            Ok((chosen, true))
        }
    }
}

/// Show an interactive layout picker using dialoguer::Select.
fn show_layout_picker(preselect: Option<&Layout>) -> Result<Layout> {
    let global_config = GlobalConfig::load().unwrap_or_default();
    let mut layouts: Vec<Layout> = BuiltinLayout::all()
        .iter()
        .map(|b| b.to_layout())
        .collect();
    layouts.extend(global_config.custom_layouts());

    let items: Vec<String> = layouts
        .iter()
        .map(|l| format!("{:<20}{}", l.name, l.template))
        .collect();

    let default_idx = preselect
        .and_then(|pre| layouts.iter().position(|l| l.name == pre.name))
        .unwrap_or(0);

    let selection = dialoguer::Select::with_theme(
        &dialoguer::theme::ColorfulTheme::default(),
    )
    .items(&items)
    .default(default_idx)
    .interact()?;

    Ok(layouts[selection].clone())
}
```

- [ ] **Step 3: Wire the interactive flow into `run_checkout` and
      `run_create_branch`**

In `run_checkout` (line ~529), change:

```rust
let layout = resolve_checkout_layout(&git, output);
```

to:

```rust
let (resolved_layout, source) = resolve_checkout_layout(&git, output);
let (layout, should_persist) =
    interactive_layout_resolution(&resolved_layout, source, output)?;

// Persist layout choice to repos.json
if should_persist {
    if let Ok(git_dir) = get_git_common_dir() {
        let mut trust_db = TrustDatabase::load().unwrap_or_default();
        trust_db.set_layout(&git_dir, layout.name.clone());
        let _ = trust_db.save();
    }
}
```

Do the same in `run_create_branch` (line ~592).

- [ ] **Step 4: Run existing checkout tests to verify no regression**

Run: `mise run test:manual -- --ci checkout` Expected: ALL PASS. (Tests set
`DAFT_TESTING=1` which makes prompts non-interactive.)

- [ ] **Step 5: Commit**

```bash
git add src/commands/checkout.rs
git commit -m "feat(layout): interactive layout resolution in daft start"
```

---

### Task 10: Consolidation prompt

**Files:**

- Modify: `src/commands/checkout.rs`

- [ ] **Step 1: Add consolidation prompt after layout picker**

After the layout picker returns in the `Detected` (rejected) and `Unresolved`
flows, add a consolidation prompt:

```rust
/// Ask whether to consolidate existing worktrees to the chosen layout.
fn maybe_consolidate(
    chosen_layout: &Layout,
    output: &mut dyn Output,
) -> Result<()> {
    if !std::io::stdin().is_terminal() || std::env::var("DAFT_TESTING").is_ok() {
        return Ok(());
    }

    let git = crate::git::GitCommand::new(true);
    let porcelain = git.worktree_list_porcelain()?;
    let worktrees = crate::core::layout::detect::parse_worktree_list(&porcelain);
    let linked_count = worktrees.iter().filter(|wt| !wt.is_main).count();

    if linked_count == 0 {
        return Ok(());
    }

    let prompt = format!(
        "Consolidate {} existing worktree{} to match \"{}\" layout?",
        linked_count,
        if linked_count == 1 { "" } else { "s" },
        chosen_layout.name,
    );

    let consolidate = dialoguer::Confirm::with_theme(
        &dialoguer::theme::ColorfulTheme::default(),
    )
    .with_prompt(prompt)
    .default(false)
    .interact()?;

    if consolidate {
        output.info(&format!(
            "Run `daft layout transform {}` to consolidate.",
            chosen_layout.name,
        ));
        // Note: we don't run the transform inline because it's a complex
        // operation that should be explicit. Instead, we tell the user
        // the command to run.
    }

    Ok(())
}
```

- [ ] **Step 2: Call consolidation in the interactive flow**

In `interactive_layout_resolution`, after the layout picker returns in the
`Detected` (rejected) branch:

```rust
if confirmed {
    Ok((layout.clone(), true))
} else {
    let chosen = show_layout_picker(Some(layout))?;
    maybe_consolidate(&chosen, output)?;
    Ok((chosen, true))
}
```

And in the `Unresolved` Flow C branch:

```rust
let chosen = show_layout_picker(None)?;
maybe_consolidate(&chosen, output)?;
Ok((chosen, true))
```

- [ ] **Step 3: Run tests**

Run: `mise run test:manual -- --ci checkout` Expected: ALL PASS.

- [ ] **Step 4: Commit**

```bash
git add src/commands/checkout.rs
git commit -m "feat(layout): add consolidation prompt after layout selection"
```

---

### Task 11: Update existing tests and verify full suite

**Files:**

- Modify: `tests/manual/scenarios/layout/show-source.yml` (if it checks
  "default" source)

- [ ] **Step 1: Check if any existing YAML tests match on "default" source
      text**

Search for `"default"` in layout test scenarios. Any test that previously
expected `(default)` in output may need updating.

- [ ] **Step 2: Run full test suite**

Run: `mise run clippy` Expected: Zero warnings.

Run: `mise run test:unit` Expected: ALL PASS.

Run: `mise run test:manual -- --ci layout` Expected: ALL PASS.

- [ ] **Step 3: Fix any regressions**

If any test fails because it expected `(default)` output, update it to expect
either `(detected)` or `No layout` depending on the scenario.

- [ ] **Step 4: Commit any test fixes**

```bash
git add -A
git commit -m "test(layout): update existing tests for Detected/Unresolved sources"
```

---

### Task 12: Update completions for `daft layout show` path argument

**Files:**

- Modify: `src/commands/completions/bash.rs`
- Modify: `src/commands/completions/zsh.rs`
- Modify: `src/commands/completions/fish.rs`

- [ ] **Step 1: Add directory completion for `layout show` in bash**

In the layout-show completion case in `bash.rs`, add directory completion
(typically `COMPREPLY=($(compgen -d -- "$cur"))`).

- [ ] **Step 2: Add directory completion for `layout show` in zsh**

In `zsh.rs`, add `_files -/` for the path argument.

- [ ] **Step 3: Add directory completion for `layout show` in fish**

In `fish.rs`, add `__fish_complete_directories` for layout show.

- [ ] **Step 4: Run clippy and test**

Run: `mise run clippy` Expected: Zero warnings.

- [ ] **Step 5: Regenerate man pages**

The `layout show` command now accepts a `[path]` argument, so man pages need
regenerating:

Run: `mise run man:gen`

- [ ] **Step 6: Run final verification**

Run: `mise run clippy` Expected: Zero warnings.

Run: `mise run test:unit` Expected: ALL PASS.

Run: `mise run man:verify` Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/commands/completions/ man/
git commit -m "feat(completions): add directory completion for daft layout show path"
```
