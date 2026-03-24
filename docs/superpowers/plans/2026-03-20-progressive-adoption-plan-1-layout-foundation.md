# Layout System Foundation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the layout type system, template engine, bare inference
heuristic, unified repo store, global config, and config resolution chain — all
as pure foundation code with no existing command changes.

**Architecture:** New `src/core/layout/` module with template rendering, bare
inference, and layout resolution. Existing trust store expanded into a unified
repo store (`repos.json`). New global TOML config for user defaults. `daft.yml`
gains a `layout` field. A resolver composes all sources into a single resolved
layout for any repo.

**Tech Stack:** Rust, serde, serde_json, toml (new dep), version-migrate
(existing)

**Spec:**
`docs/superpowers/specs/2026-03-20-progressive-adoption-layout-system-design.md`

---

## File Structure

### New files

| File                          | Responsibility                                                           |
| ----------------------------- | ------------------------------------------------------------------------ |
| `src/core/layout/mod.rs`      | `Layout` struct, `BuiltinLayout` enum, public API                        |
| `src/core/layout/template.rs` | Template parsing, variable substitution, `sanitize` filter               |
| `src/core/layout/bare.rs`     | Bare inference heuristic from template geometry                          |
| `src/core/layout/resolver.rs` | Config resolution chain (CLI > repos.json > daft.yml > global > default) |
| `src/core/global_config.rs`   | Global TOML config (`~/.config/daft/config.toml`)                        |

### Modified files

| File                       | Change                                                                                                          |
| -------------------------- | --------------------------------------------------------------------------------------------------------------- |
| `src/core/mod.rs`          | Add `pub mod layout;` and `pub mod global_config;`                                                              |
| `src/lib.rs`               | Re-export layout module                                                                                         |
| `src/hooks/trust.rs`       | Add `layouts` HashMap, V3-aware serialization/deserialization, `repos.json` path, atomic `trust.json` migration |
| `src/hooks/trust_dto.rs`   | Add V3 DTO (`RepoStoreV3_0_0`, `RepoEntryV3_0_0`) and V2→V3 migration                                           |
| `src/hooks/yaml_config.rs` | Add `layout: Option<String>` field to `YamlConfig`                                                              |
| `Cargo.toml`               | Add `toml` dependency                                                                                           |

---

## Task 1: Template Engine

**Files:**

- Create: `src/core/layout/mod.rs` (skeleton only — just `pub mod template;`)
- Create: `src/core/layout/template.rs`
- Modify: `src/core/mod.rs` — add `pub mod layout;`

### Steps

- [ ] **Step 1: Create module skeleton**

Create `src/core/layout/mod.rs`:

```rust
pub mod template;
```

Add to `src/core/mod.rs` after `pub mod config;`:

```rust
pub mod layout;
```

- [ ] **Step 2: Write failing tests for template rendering**

Create `src/core/layout/template.rs` with tests only:

```rust
//! Template parsing and variable substitution for layout paths.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

/// Context for resolving template variables.
pub struct TemplateContext {
    /// Absolute path to the repository root.
    pub repo_path: PathBuf,
    /// Repository directory name (last component of repo_path).
    pub repo: String,
    /// Branch name (raw, may contain slashes).
    pub branch: String,
}

/// Sanitize a branch name for use as a filesystem path component.
///
/// Replaces `/` and `\` with `-`.
pub fn sanitize(s: &str) -> String {
    todo!()
}

/// Render a template string with the given context.
///
/// Supported syntax:
/// - `{{ variable }}` — replaced with the variable value
/// - `{{ variable | sanitize }}` — replaced with sanitized value
///
/// Supported variables: `repo_path`, `repo`, `branch`
pub fn render(template: &str, ctx: &TemplateContext) -> Result<String> {
    todo!()
}

/// Resolve a rendered template path to an absolute PathBuf.
///
/// - Paths starting with `~/` are expanded to the home directory.
/// - Paths starting with `/` are absolute.
/// - Paths starting with `../` are resolved relative to `repo_path`.
/// - All other paths are resolved relative to `repo_path`.
pub fn resolve_path(rendered: &str, repo_path: &Path) -> Result<PathBuf> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_replaces_slashes() {
        assert_eq!(sanitize("feature/auth"), "feature-auth");
        assert_eq!(sanitize("fix\\bug"), "fix-bug");
        assert_eq!(sanitize("feature/nested/deep"), "feature-nested-deep");
    }

    #[test]
    fn test_sanitize_no_change_needed() {
        assert_eq!(sanitize("main"), "main");
        assert_eq!(sanitize("feature-auth"), "feature-auth");
    }

    #[test]
    fn test_render_simple_variable() {
        let ctx = TemplateContext {
            repo_path: PathBuf::from("/home/user/myproject"),
            repo: "myproject".into(),
            branch: "feature/auth".into(),
        };
        assert_eq!(render("{{ branch }}", &ctx).unwrap(), "feature/auth");
        assert_eq!(render("{{ repo }}", &ctx).unwrap(), "myproject");
    }

    #[test]
    fn test_render_with_sanitize_filter() {
        let ctx = TemplateContext {
            repo_path: PathBuf::from("/home/user/myproject"),
            repo: "myproject".into(),
            branch: "feature/auth".into(),
        };
        assert_eq!(
            render("{{ branch | sanitize }}", &ctx).unwrap(),
            "feature-auth"
        );
    }

    #[test]
    fn test_render_mixed_text_and_variables() {
        let ctx = TemplateContext {
            repo_path: PathBuf::from("/home/user/myproject"),
            repo: "myproject".into(),
            branch: "feature/auth".into(),
        };
        assert_eq!(
            render("../{{ repo }}.{{ branch | sanitize }}", &ctx).unwrap(),
            "../myproject.feature-auth"
        );
    }

    #[test]
    fn test_render_contained_layout() {
        let ctx = TemplateContext {
            repo_path: PathBuf::from("/home/user/myproject"),
            repo: "myproject".into(),
            branch: "main".into(),
        };
        assert_eq!(render("{{ branch | sanitize }}", &ctx).unwrap(), "main");
    }

    #[test]
    fn test_render_unknown_variable_errors() {
        let ctx = TemplateContext {
            repo_path: PathBuf::from("/home/user/myproject"),
            repo: "myproject".into(),
            branch: "main".into(),
        };
        assert!(render("{{ unknown }}", &ctx).is_err());
    }

    #[test]
    fn test_render_unknown_filter_errors() {
        let ctx = TemplateContext {
            repo_path: PathBuf::from("/home/user/myproject"),
            repo: "myproject".into(),
            branch: "main".into(),
        };
        assert!(render("{{ branch | unknown_filter }}", &ctx).is_err());
    }

    #[test]
    fn test_resolve_path_relative() {
        let repo = Path::new("/home/user/myproject");
        let resolved = resolve_path("main", repo).unwrap();
        assert_eq!(resolved, PathBuf::from("/home/user/myproject/main"));
    }

    #[test]
    fn test_resolve_path_parent_relative() {
        let repo = Path::new("/home/user/myproject");
        let resolved = resolve_path("../myproject.feature-auth", repo).unwrap();
        assert_eq!(
            resolved,
            PathBuf::from("/home/user/myproject.feature-auth")
        );
    }

    #[test]
    fn test_resolve_path_absolute() {
        let repo = Path::new("/home/user/myproject");
        let resolved = resolve_path("/tmp/worktrees/feature-auth", repo).unwrap();
        assert_eq!(
            resolved,
            PathBuf::from("/tmp/worktrees/feature-auth")
        );
    }

    #[test]
    fn test_resolve_path_home_expansion() {
        let repo = Path::new("/home/user/myproject");
        let resolved = resolve_path("~/worktrees/myproject/main", repo).unwrap();
        // Should start with the home directory, not literally "~"
        assert!(!resolved.starts_with("~"));
        assert!(resolved.ends_with("worktrees/myproject/main"));
    }

    #[test]
    fn test_resolve_path_dotdir() {
        let repo = Path::new("/home/user/myproject");
        let resolved = resolve_path(".worktrees/feature-auth", repo).unwrap();
        assert_eq!(
            resolved,
            PathBuf::from("/home/user/myproject/.worktrees/feature-auth")
        );
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p daft --lib layout::template -- --nocapture 2>&1 | head -30`

Expected: compilation errors from `todo!()` panics or similar.

- [ ] **Step 4: Implement `sanitize`**

```rust
pub fn sanitize(s: &str) -> String {
    s.replace('/', "-").replace('\\', "-")
}
```

- [ ] **Step 5: Implement `render`**

Parse `{{ variable }}` and `{{ variable | filter }}` patterns using a simple
state machine or regex. Replace each match with the resolved value.

Implementation approach:

```rust
pub fn render(template: &str, ctx: &TemplateContext) -> Result<String> {
    let mut result = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(start) = rest.find("{{") {
        result.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        let end = after_open
            .find("}}")
            .ok_or_else(|| anyhow::anyhow!("Unclosed template expression in: {template}"))?;
        let expr = after_open[..end].trim();
        let value = resolve_expression(expr, ctx)?;
        result.push_str(&value);
        rest = &after_open[end + 2..];
    }
    result.push_str(rest);
    Ok(result)
}

fn resolve_expression(expr: &str, ctx: &TemplateContext) -> Result<String> {
    let parts: Vec<&str> = expr.splitn(2, '|').map(|s| s.trim()).collect();
    let var_name = parts[0];
    let filter = parts.get(1).copied();

    let raw_value = match var_name {
        "repo_path" => ctx.repo_path.to_string_lossy().to_string(),
        "repo" => ctx.repo.clone(),
        "branch" => ctx.branch.clone(),
        _ => bail!("Unknown template variable: {var_name}"),
    };

    match filter {
        None => Ok(raw_value),
        Some("sanitize") => Ok(sanitize(&raw_value)),
        Some(f) => bail!("Unknown template filter: {f}"),
    }
}
```

- [ ] **Step 6: Implement `resolve_path`**

```rust
pub fn resolve_path(rendered: &str, repo_path: &Path) -> Result<PathBuf> {
    if rendered.starts_with('/') {
        return Ok(PathBuf::from(rendered));
    }
    if rendered.starts_with("~/") {
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;
        return Ok(home.join(&rendered[2..]));
    }
    // Relative path (including ../ and ./ prefixes) — resolve against repo_path
    Ok(repo_path.join(rendered))
}
```

After joining, normalize the path to remove `..` components:

```rust
/// Normalize a path by resolving `.` and `..` components without filesystem access.
fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                if !components.is_empty() {
                    components.pop();
                }
            }
            std::path::Component::CurDir => {}
            _ => components.push(component),
        }
    }
    components.iter().collect()
}

pub fn resolve_path(rendered: &str, repo_path: &Path) -> Result<PathBuf> {
    if rendered.starts_with('/') {
        return Ok(normalize_path(Path::new(rendered)));
    }
    if rendered.starts_with("~/") {
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;
        return Ok(normalize_path(&home.join(&rendered[2..])));
    }
    Ok(normalize_path(&repo_path.join(rendered)))
}
```

Update the sibling test assertions to expect normalized paths:

```rust
// In test_resolve_path_parent_relative:
assert_eq!(resolved, PathBuf::from("/home/user/myproject.feature-auth"));
// In test_sibling_worktree_path:
assert_eq!(path, PathBuf::from("/home/user/myproject.feature-auth"));
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p daft --lib layout::template -- --nocapture`

Expected: all tests pass.

- [ ] **Step 8: Run clippy and fmt**

Run: `mise run fmt && mise run clippy`

Expected: no warnings.

- [ ] **Step 9: Commit**

```bash
git add src/core/layout/ src/core/mod.rs
git commit -m "feat(layout): add template engine with variable substitution and sanitize filter"
```

---

## Task 2: Bare Inference Heuristic

**Files:**

- Create: `src/core/layout/bare.rs`
- Modify: `src/core/layout/mod.rs` — add `pub mod bare;`

### Steps

- [ ] **Step 1: Write failing tests**

Create `src/core/layout/bare.rs`:

```rust
//! Bare repository inference from template geometry.
//!
//! Determines whether a layout template requires a bare repository based on
//! where it places worktrees relative to the repo root.

use anyhow::Result;

/// Infer whether a layout template requires a bare repository.
///
/// Rules:
/// 1. If `explicit_bare` is provided, use it (custom layout override).
/// 2. Starts with `../`, `/`, or `~/` — not bare (worktrees outside repo).
/// 3. First path segment starts with `.` — not bare (hidden directory).
/// 4. Otherwise — bare required (worktrees would conflict with working tree).
pub fn infer_bare(template: &str, explicit_bare: Option<bool>) -> bool {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Rule 1: explicit override
    #[test]
    fn test_explicit_bare_true() {
        assert!(infer_bare("anything", Some(true)));
    }

    #[test]
    fn test_explicit_bare_false() {
        assert!(!infer_bare("{{ branch | sanitize }}", Some(false)));
    }

    // Rule 2: outside repo
    #[test]
    fn test_parent_relative_not_bare() {
        assert!(!infer_bare(
            "../{{ repo }}.{{ branch | sanitize }}",
            None
        ));
    }

    #[test]
    fn test_absolute_path_not_bare() {
        assert!(!infer_bare(
            "/tmp/worktrees/{{ repo }}/{{ branch | sanitize }}",
            None
        ));
    }

    #[test]
    fn test_home_path_not_bare() {
        assert!(!infer_bare(
            "~/worktrees/{{ repo }}/{{ branch | sanitize }}",
            None
        ));
    }

    // Rule 3: hidden directory
    #[test]
    fn test_hidden_dir_not_bare() {
        assert!(!infer_bare(".worktrees/{{ branch | sanitize }}", None));
    }

    #[test]
    fn test_hidden_dir_nested_not_bare() {
        assert!(!infer_bare(
            ".trees/{{ repo }}/{{ branch | sanitize }}",
            None
        ));
    }

    // Rule 4: bare required
    #[test]
    fn test_direct_child_is_bare() {
        assert!(infer_bare("{{ branch | sanitize }}", None));
    }

    #[test]
    fn test_visible_subdir_is_bare() {
        assert!(infer_bare("worktrees/{{ branch | sanitize }}", None));
    }

    #[test]
    fn test_nested_visible_is_bare() {
        assert!(infer_bare(
            "trees/{{ repo }}/{{ branch | sanitize }}",
            None
        ));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p daft --lib layout::bare -- --nocapture 2>&1 | head -20`

Expected: `todo!()` panic.

- [ ] **Step 3: Implement `infer_bare`**

```rust
pub fn infer_bare(template: &str, explicit_bare: Option<bool>) -> bool {
    if let Some(bare) = explicit_bare {
        return bare;
    }

    // Rule 2: outside repo
    if template.starts_with("../") || template.starts_with('/') || template.starts_with("~/") {
        return false;
    }

    // Rule 3: hidden directory (first segment starts with '.')
    let first_segment = template.split('/').next().unwrap_or("");
    if first_segment.starts_with('.') {
        return false;
    }

    // Rule 4: bare required
    true
}
```

- [ ] **Step 4: Add `pub mod bare;` to `src/core/layout/mod.rs`**

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p daft --lib layout::bare -- --nocapture`

Expected: all tests pass.

- [ ] **Step 6: Run clippy and fmt, commit**

```bash
mise run fmt && mise run clippy
git add src/core/layout/bare.rs src/core/layout/mod.rs
git commit -m "feat(layout): add bare inference heuristic from template geometry"
```

---

## Task 3: Layout Types and Built-in Definitions

**Files:**

- Modify: `src/core/layout/mod.rs` — add `Layout` struct and `BuiltinLayout`
  enum

### Steps

- [ ] **Step 1: Write failing tests in `src/core/layout/mod.rs`**

Add to `mod.rs` after the module declarations:

```rust
use std::path::{Path, PathBuf};

use anyhow::Result;

pub use template::TemplateContext;

/// A worktree layout definition.
///
/// Layouts control where worktrees are placed on disk. Each layout has a name,
/// a template string for path computation, and an optional explicit bare flag.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// Layout name (e.g., "contained", "sibling", or a custom name).
    pub name: String,
    /// Template string for worktree path computation.
    pub template: String,
    /// Explicit bare override. If `None`, bare is inferred from template.
    pub bare: Option<bool>,
}

/// Built-in layout presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinLayout {
    Contained,
    Sibling,
    Nested,
    Centralized,
}

/// The default layout for daft when no configuration is set.
pub const DEFAULT_LAYOUT: BuiltinLayout = BuiltinLayout::Sibling;

impl Layout {
    /// Whether this layout requires a bare repository.
    pub fn needs_bare(&self) -> bool {
        todo!()
    }

    /// Compute the worktree path for a branch in this layout.
    pub fn worktree_path(&self, ctx: &TemplateContext) -> Result<PathBuf> {
        todo!()
    }
}

impl BuiltinLayout {
    /// Convert to a Layout definition.
    pub fn to_layout(self) -> Layout {
        todo!()
    }

    /// Get the layout name as a string.
    pub fn name(&self) -> &'static str {
        todo!()
    }

    /// Look up a built-in layout by name.
    pub fn from_name(name: &str) -> Option<Self> {
        todo!()
    }

    /// All built-in layouts.
    pub fn all() -> &'static [BuiltinLayout] {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_contained_is_bare() {
        let layout = BuiltinLayout::Contained.to_layout();
        assert!(layout.needs_bare());
        assert_eq!(layout.name, "contained");
    }

    #[test]
    fn test_builtin_sibling_not_bare() {
        let layout = BuiltinLayout::Sibling.to_layout();
        assert!(!layout.needs_bare());
        assert_eq!(layout.name, "sibling");
    }

    #[test]
    fn test_builtin_nested_not_bare() {
        let layout = BuiltinLayout::Nested.to_layout();
        assert!(!layout.needs_bare());
        assert_eq!(layout.name, "nested");
    }

    #[test]
    fn test_builtin_centralized_not_bare() {
        let layout = BuiltinLayout::Centralized.to_layout();
        assert!(!layout.needs_bare());
        assert_eq!(layout.name, "centralized");
    }

    #[test]
    fn test_builtin_from_name() {
        assert_eq!(
            BuiltinLayout::from_name("contained"),
            Some(BuiltinLayout::Contained)
        );
        assert_eq!(
            BuiltinLayout::from_name("sibling"),
            Some(BuiltinLayout::Sibling)
        );
        assert_eq!(BuiltinLayout::from_name("unknown"), None);
    }

    #[test]
    fn test_builtin_all() {
        let all = BuiltinLayout::all();
        assert_eq!(all.len(), 4);
    }

    #[test]
    fn test_contained_worktree_path() {
        let layout = BuiltinLayout::Contained.to_layout();
        let ctx = TemplateContext {
            repo_path: PathBuf::from("/home/user/myproject"),
            repo: "myproject".into(),
            branch: "feature/auth".into(),
        };
        let path = layout.worktree_path(&ctx).unwrap();
        assert_eq!(path, PathBuf::from("/home/user/myproject/feature-auth"));
    }

    #[test]
    fn test_sibling_worktree_path() {
        let layout = BuiltinLayout::Sibling.to_layout();
        let ctx = TemplateContext {
            repo_path: PathBuf::from("/home/user/myproject"),
            repo: "myproject".into(),
            branch: "feature/auth".into(),
        };
        let path = layout.worktree_path(&ctx).unwrap();
        assert_eq!(
            path,
            PathBuf::from("/home/user/myproject/../myproject.feature-auth")
        );
    }

    #[test]
    fn test_nested_worktree_path() {
        let layout = BuiltinLayout::Nested.to_layout();
        let ctx = TemplateContext {
            repo_path: PathBuf::from("/home/user/myproject"),
            repo: "myproject".into(),
            branch: "feature/auth".into(),
        };
        let path = layout.worktree_path(&ctx).unwrap();
        assert_eq!(
            path,
            PathBuf::from("/home/user/myproject/.worktrees/feature-auth")
        );
    }

    #[test]
    fn test_custom_layout_with_explicit_bare_false() {
        let layout = Layout {
            name: "visible-subdir".into(),
            template: "worktrees/{{ branch | sanitize }}".into(),
            bare: Some(false),
        };
        assert!(!layout.needs_bare());
    }

    #[test]
    fn test_default_layout_is_sibling() {
        assert_eq!(DEFAULT_LAYOUT, BuiltinLayout::Sibling);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p daft --lib layout -- --nocapture 2>&1 | head -20`

Expected: `todo!()` panics.

- [ ] **Step 3: Implement all methods**

```rust
impl Layout {
    pub fn needs_bare(&self) -> bool {
        bare::infer_bare(&self.template, self.bare)
    }

    pub fn worktree_path(&self, ctx: &TemplateContext) -> Result<PathBuf> {
        let rendered = template::render(&self.template, ctx)?;
        template::resolve_path(&rendered, &ctx.repo_path)
    }
}

impl BuiltinLayout {
    pub fn to_layout(self) -> Layout {
        Layout {
            name: self.name().to_string(),
            template: self.template().to_string(),
            bare: None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Contained => "contained",
            Self::Sibling => "sibling",
            Self::Nested => "nested",
            Self::Centralized => "centralized",
        }
    }

    fn template(&self) -> &'static str {
        match self {
            Self::Contained => "{{ branch | sanitize }}",
            Self::Sibling => "../{{ repo }}.{{ branch | sanitize }}",
            Self::Nested => ".worktrees/{{ branch | sanitize }}",
            Self::Centralized => "~/worktrees/{{ repo }}/{{ branch | sanitize }}",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "contained" => Some(Self::Contained),
            "sibling" => Some(Self::Sibling),
            "nested" => Some(Self::Nested),
            "centralized" => Some(Self::Centralized),
            _ => None,
        }
    }

    pub fn all() -> &'static [BuiltinLayout] {
        &[
            Self::Contained,
            Self::Sibling,
            Self::Nested,
            Self::Centralized,
        ]
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p daft --lib layout -- --nocapture`

Expected: all layout tests pass (including template and bare tests from
earlier).

- [ ] **Step 5: Run clippy and fmt, commit**

```bash
mise run fmt && mise run clippy
git add src/core/layout/mod.rs
git commit -m "feat(layout): add Layout struct and built-in layout definitions"
```

---

## Task 4: Global TOML Config

**Files:**

- Modify: `Cargo.toml` — add `toml` dependency
- Create: `src/core/global_config.rs`
- Modify: `src/core/mod.rs` — add `pub mod global_config;`

### Steps

- [ ] **Step 1: Add `toml` dependency**

Add to `[dependencies]` in `Cargo.toml`:

```toml
toml = "0.8"
```

Run: `cargo check -p daft` to verify it compiles.

- [ ] **Step 2: Write failing tests**

Create `src/core/global_config.rs`:

```rust
//! Global daft configuration file (~/.config/daft/config.toml).
//!
//! Stores user-wide defaults and custom layout definitions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use super::layout::{BuiltinLayout, Layout};

/// Parsed global configuration.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct GlobalConfig {
    /// Default settings.
    pub defaults: GlobalDefaults,
    /// Custom layout definitions.
    pub layouts: HashMap<String, CustomLayoutDef>,
}

/// Default settings section.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct GlobalDefaults {
    /// Default layout name or inline template.
    pub layout: Option<String>,
}

/// Custom layout definition in config.
#[derive(Debug, Clone, Deserialize)]
pub struct CustomLayoutDef {
    /// Template string for worktree path computation.
    pub template: String,
    /// Explicit bare override (optional).
    pub bare: Option<bool>,
}

impl GlobalConfig {
    /// Load the global config from the default location.
    ///
    /// Returns a default config if the file does not exist.
    pub fn load() -> Result<Self> {
        let path = Self::default_path()?;
        Self::load_from(&path)
    }

    /// Load the global config from a specific path.
    pub fn load_from(path: &Path) -> Result<Self> {
        todo!()
    }

    /// Get the default path for the global config file.
    pub fn default_path() -> Result<PathBuf> {
        Ok(crate::daft_config_dir()?.join("config.toml"))
    }

    /// Look up a layout by name: check custom layouts first, then built-ins.
    ///
    /// If the name matches a custom layout, returns it. If it matches a
    /// built-in, returns the built-in. Otherwise returns None.
    pub fn resolve_layout_by_name(&self, name: &str) -> Option<Layout> {
        todo!()
    }

    /// Get the default layout, if configured.
    ///
    /// Returns the resolved Layout if `defaults.layout` is set and resolves
    /// to a known layout or valid inline template. Returns None if not set.
    pub fn default_layout(&self) -> Option<Layout> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_nonexistent_returns_default() {
        let config = GlobalConfig::load_from(Path::new("/nonexistent/config.toml")).unwrap();
        assert!(config.defaults.layout.is_none());
        assert!(config.layouts.is_empty());
    }

    #[test]
    fn test_parse_minimal_config() {
        let toml_str = r#"
[defaults]
layout = "contained"
"#;
        let config: GlobalConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.defaults.layout, Some("contained".into()));
    }

    #[test]
    fn test_parse_with_custom_layouts() {
        let toml_str = r#"
[defaults]
layout = "my-custom"

[layouts.my-custom]
template = "../.worktrees/{{ repo }}/{{ branch | sanitize }}"

[layouts.visible-subdir]
template = "worktrees/{{ branch | sanitize }}"
bare = false
"#;
        let config: GlobalConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.defaults.layout, Some("my-custom".into()));
        assert_eq!(config.layouts.len(), 2);
        assert_eq!(
            config.layouts["my-custom"].template,
            "../.worktrees/{{ repo }}/{{ branch | sanitize }}"
        );
        assert_eq!(config.layouts["visible-subdir"].bare, Some(false));
    }

    #[test]
    fn test_resolve_builtin_layout() {
        let config = GlobalConfig::default();
        let layout = config.resolve_layout_by_name("sibling").unwrap();
        assert_eq!(layout.name, "sibling");
        assert!(!layout.needs_bare());
    }

    #[test]
    fn test_resolve_custom_layout() {
        let toml_str = r#"
[layouts.my-custom]
template = "../.worktrees/{{ repo }}/{{ branch | sanitize }}"
"#;
        let config: GlobalConfig = toml::from_str(toml_str).unwrap();
        let layout = config.resolve_layout_by_name("my-custom").unwrap();
        assert_eq!(layout.name, "my-custom");
        assert!(!layout.needs_bare());
    }

    #[test]
    fn test_custom_layout_overrides_builtin_name() {
        let toml_str = r#"
[layouts.sibling]
template = "custom/{{ branch | sanitize }}"
"#;
        let config: GlobalConfig = toml::from_str(toml_str).unwrap();
        let layout = config.resolve_layout_by_name("sibling").unwrap();
        assert_eq!(layout.template, "custom/{{ branch | sanitize }}");
    }

    #[test]
    fn test_resolve_unknown_returns_none() {
        let config = GlobalConfig::default();
        assert!(config.resolve_layout_by_name("nonexistent").is_none());
    }

    #[test]
    fn test_default_layout_when_set() {
        let toml_str = r#"
[defaults]
layout = "contained"
"#;
        let config: GlobalConfig = toml::from_str(toml_str).unwrap();
        let layout = config.default_layout().unwrap();
        assert_eq!(layout.name, "contained");
    }

    #[test]
    fn test_default_layout_when_not_set() {
        let config = GlobalConfig::default();
        assert!(config.default_layout().is_none());
    }

    #[test]
    fn test_default_layout_inline_template() {
        let toml_str = r#"
[defaults]
layout = "custom/{{ branch | sanitize }}"
"#;
        let config: GlobalConfig = toml::from_str(toml_str).unwrap();
        let layout = config.default_layout().unwrap();
        assert_eq!(layout.template, "custom/{{ branch | sanitize }}");
        assert_eq!(layout.name, "custom/{{ branch | sanitize }}");
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p daft --lib global_config -- --nocapture 2>&1 | head -20`

- [ ] **Step 4: Implement `GlobalConfig`**

```rust
impl GlobalConfig {
    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config from {}", path.display()))?;
        toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config from {}", path.display()))
    }

    pub fn resolve_layout_by_name(&self, name: &str) -> Option<Layout> {
        // Custom layouts take precedence
        if let Some(custom) = self.layouts.get(name) {
            return Some(Layout {
                name: name.to_string(),
                template: custom.template.clone(),
                bare: custom.bare,
            });
        }
        // Fall back to built-in
        BuiltinLayout::from_name(name).map(|b| b.to_layout())
    }

    pub fn default_layout(&self) -> Option<Layout> {
        let name = self.defaults.layout.as_deref()?;
        // Try named layout first
        if let Some(layout) = self.resolve_layout_by_name(name) {
            return Some(layout);
        }
        // Treat as inline template
        Some(Layout {
            name: name.to_string(),
            template: name.to_string(),
            bare: None,
        })
    }
}
```

- [ ] **Step 5: Add `pub mod global_config;` to `src/core/mod.rs`**

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p daft --lib global_config -- --nocapture`

- [ ] **Step 7: Run clippy and fmt, commit**

```bash
mise run fmt && mise run clippy
git add Cargo.toml Cargo.lock src/core/global_config.rs src/core/mod.rs
git commit -m "feat(layout): add global TOML config with layout defaults and custom layouts"
```

---

## Task 5: Unified Repo Store (V3 Migration)

**Files:**

- Modify: `src/hooks/trust.rs` — add `layouts` field, custom V3 serialization,
  `repos.json` path, atomic migration
- Modify: `src/hooks/trust_dto.rs` — add V3 DTO and V2→V3 migration

This task does NOT rename `TrustDatabase` → `RepoStore` yet (that would break
every call site). The in-memory model keeps
`repositories: HashMap<String, TrustEntry>` plus a new
`layouts: HashMap<String, String>`. However, `save_to()` writes V3-shaped JSON
(nested `trust`/`layout` per entry) and `load_from()` can parse both V2 and V3
formats. This means the on-disk format matches the spec schema while existing
code keeps working.

### Steps

- [ ] **Step 1: Add V3 DTO and migration in `trust_dto.rs`**

Add after existing V2 code, before `#[cfg(test)]`:

```rust
/// V3: Unified repo store with trust + layout.
#[derive(Debug, Clone, Serialize, Deserialize, Versioned)]
#[versioned(version = "3.0.0")]
pub struct RepoStoreV3_0_0 {
    #[serde(default)]
    pub repositories: HashMap<String, RepoEntryV3_0_0>,
    #[serde(default)]
    pub patterns: Vec<TrustPattern>,
}

/// Repository entry in V3 schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoEntryV3_0_0 {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trust: Option<TrustEntryV2_0_0>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
}

/// Migration: V2 -> V3 (wrap trust in sub-object, add layout field).
impl MigratesTo<RepoStoreV3_0_0> for TrustDatabaseV2_0_0 {
    fn migrate(self) -> RepoStoreV3_0_0 {
        let repositories = self
            .repositories
            .into_iter()
            .map(|(path, entry)| {
                (path, RepoEntryV3_0_0 { trust: Some(entry), layout: None })
            })
            .collect();
        RepoStoreV3_0_0 { repositories, patterns: self.patterns }
    }
}

/// Convert V3 DTO to domain model.
impl IntoDomain<TrustDatabase> for RepoStoreV3_0_0 {
    fn into_domain(self) -> TrustDatabase {
        let mut repositories = HashMap::new();
        let mut layouts = HashMap::new();

        for (path, entry) in self.repositories {
            if let Some(trust) = entry.trust {
                repositories.insert(
                    path.clone(),
                    TrustEntry {
                        level: trust.level,
                        granted_at: trust.granted_at,
                        granted_by: trust.granted_by,
                        fingerprint: trust.fingerprint,
                    },
                );
            }
            if let Some(layout) = entry.layout {
                layouts.insert(path, layout);
            }
        }

        TrustDatabase {
            version: 3,
            default_level: TrustLevel::Deny,
            repositories,
            layouts,
            patterns: self.patterns,
        }
    }
}
```

Add DTO tests:

```rust
#[test]
fn test_v2_to_v3_migration() {
    let v2 = TrustDatabaseV2_0_0 { /* same as existing test */ };
    let v3: RepoStoreV3_0_0 = v2.migrate();
    assert_eq!(v3.repositories.len(), 1);
    let entry = v3.repositories.get("/path/to/repo/.git").unwrap();
    assert!(entry.trust.is_some());
    assert!(entry.layout.is_none());
}

#[test]
fn test_v3_into_domain() {
    let v3 = RepoStoreV3_0_0 {
        repositories: {
            let mut map = HashMap::new();
            map.insert("/repo/.git".to_string(), RepoEntryV3_0_0 {
                trust: Some(TrustEntryV2_0_0 {
                    level: TrustLevel::Allow,
                    granted_at: 1738060200,
                    granted_by: "user".to_string(),
                    fingerprint: None,
                }),
                layout: Some("contained".to_string()),
            });
            map
        },
        patterns: vec![],
    };
    let db: TrustDatabase = v3.into_domain();
    assert_eq!(db.version, 3);
    assert_eq!(db.get_trust_level(Path::new("/repo/.git")), TrustLevel::Allow);
    assert_eq!(db.get_layout(Path::new("/repo/.git")), Some("contained"));
}
```

- [ ] **Step 2: Run DTO tests**

Run: `cargo test -p daft --lib hooks::trust_dto -- --nocapture`

- [ ] **Step 3: Update `TrustDatabase` in `trust.rs`**

Add `layouts` field (private, not serialized directly — V3 serialization is
custom):

```rust
pub struct TrustDatabase {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub default_level: TrustLevel,
    #[serde(default)]
    pub repositories: HashMap<String, TrustEntry>,
    /// Per-repository layout overrides.
    /// Not serialized directly — V3 save/load merges this with repositories.
    #[serde(skip)]
    pub(crate) layouts: HashMap<String, String>,
    #[serde(default)]
    pub patterns: Vec<TrustPattern>,
}
```

Update `default_version` to return `3`. Update `Default::default()` to include
`layouts: HashMap::new()` and `version: 3`.

Add layout accessor methods:

```rust
pub fn get_layout(&self, git_dir: &Path) -> Option<&str> {
    let canonical = git_dir
        .canonicalize()
        .unwrap_or_else(|_| git_dir.to_path_buf());
    self.layouts.get(&*canonical.to_string_lossy()).map(|s| s.as_str())
}

pub fn set_layout(&mut self, git_dir: &Path, layout: String) {
    let canonical = git_dir
        .canonicalize()
        .unwrap_or_else(|_| git_dir.to_path_buf());
    self.layouts.insert(canonical.to_string_lossy().to_string(), layout);
}

pub fn repos_path() -> Result<PathBuf> {
    Ok(crate::daft_config_dir()?.join("repos.json"))
}
```

- [ ] **Step 4: Update `save_to` to write V3 JSON**

Replace direct serde serialization with V3-shaped output:

```rust
pub fn save_to(&self, path: &Path) -> Result<()> {
    use super::trust_dto::{RepoEntryV3_0_0, RepoStoreV3_0_0, TrustEntryV2_0_0};

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Build V3 structure: merge repositories and layouts into unified entries
    let mut entries: HashMap<String, RepoEntryV3_0_0> = HashMap::new();

    for (path_key, trust_entry) in &self.repositories {
        entries.entry(path_key.clone()).or_insert_with(|| RepoEntryV3_0_0 {
            trust: None, layout: None,
        }).trust = Some(TrustEntryV2_0_0 {
            level: trust_entry.level,
            granted_at: trust_entry.granted_at,
            granted_by: trust_entry.granted_by.clone(),
            fingerprint: trust_entry.fingerprint.clone(),
        });
    }

    for (path_key, layout) in &self.layouts {
        entries.entry(path_key.clone()).or_insert_with(|| RepoEntryV3_0_0 {
            trust: None, layout: None,
        }).layout = Some(layout.clone());
    }

    let v3 = RepoStoreV3_0_0 { repositories: entries, patterns: self.patterns.clone() };

    // Wrap in a version envelope
    let json = serde_json::json!({
        "version": 3,
        "repositories": serde_json::to_value(&v3.repositories)?,
        "patterns": serde_json::to_value(&v3.patterns)?,
    });

    let contents = serde_json::to_string_pretty(&json)?;
    fs::write(path, contents)?;
    Ok(())
}
```

- [ ] **Step 5: Update `load_from` for V3 and atomic migration**

```rust
pub fn load_from(path: &Path) -> Result<Self> {
    use super::trust_dto::{RepoStoreV3_0_0, TrustDatabaseV1_0_0, TrustDatabaseV2_0_0};
    use version_migrate::{IntoDomain, MigratesTo};

    if !path.exists() {
        return Ok(Self::default());
    }

    let contents = fs::read_to_string(path)?;
    let json: serde_json::Value = serde_json::from_str(&contents)?;
    let stated_version = json.get("version").and_then(|v| v.as_u64()).unwrap_or(1) as u32;

    match stated_version {
        3 => {
            let v3: RepoStoreV3_0_0 = serde_json::from_value(json)?;
            Ok(v3.into_domain())
        }
        _ => {
            // V1 or V2 — use existing detection logic
            let actual_version = detect_schema_version(&json, stated_version);
            let db = match actual_version {
                1 => {
                    let v1: TrustDatabaseV1_0_0 = serde_json::from_value(json)?;
                    let v2: TrustDatabaseV2_0_0 = v1.migrate();
                    let v3: RepoStoreV3_0_0 = v2.migrate();
                    v3.into_domain()
                }
                _ => {
                    let v2: TrustDatabaseV2_0_0 = serde_json::from_value(json)?;
                    let v3: RepoStoreV3_0_0 = v2.migrate();
                    v3.into_domain()
                }
            };

            // Atomic migration: write repos.json, then remove trust.json
            if path.file_name().and_then(|n| n.to_str()) == Some("trust.json") {
                let repos_path = path.with_file_name("repos.json");
                let tmp_path = path.with_file_name("repos.json.tmp");
                db.save_to(&tmp_path)?;
                fs::rename(&tmp_path, &repos_path)?;
                let _ = fs::remove_file(path); // Best-effort removal
            } else {
                db.save_to(path)?;
            }

            Ok(db)
        }
    }
}
```

Update `default_path()`:

```rust
pub fn default_path() -> Result<PathBuf> {
    let config_dir = crate::daft_config_dir()?;
    let repos_path = config_dir.join("repos.json");
    if repos_path.exists() { return Ok(repos_path); }
    let trust_path = config_dir.join("trust.json");
    if trust_path.exists() { return Ok(trust_path); }
    Ok(repos_path)
}
```

Update `save()` to always target `repos.json`:

```rust
pub fn save(&self) -> Result<()> {
    Self::repos_path().and_then(|p| self.save_to(&p))
}
```

- [ ] **Step 6: Update `prune()` to clean layouts too**

```rust
pub fn prune(&mut self) -> Vec<String> {
    let stale: Vec<String> = self.repositories.keys()
        .chain(self.layouts.keys())
        .filter(|path| !Path::new(path.as_str()).exists())
        .cloned()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    for key in &stale {
        self.repositories.remove(key);
        self.layouts.remove(key);
    }
    stale
}
```

- [ ] **Step 7: Write trust.rs tests**

```rust
#[test]
fn test_set_and_get_layout() {
    let mut db = TrustDatabase::default();
    let git_dir = Path::new("/path/to/repo/.git");
    assert!(db.get_layout(git_dir).is_none());
    db.set_layout(git_dir, "contained".to_string());
    assert_eq!(db.get_layout(git_dir), Some("contained"));
}

#[test]
fn test_layout_survives_v3_round_trip() {
    let temp_dir = tempdir().unwrap();
    let path = temp_dir.path().join("repos.json");

    let mut db = TrustDatabase::default();
    db.set_trust_level(Path::new("/project/.git"), TrustLevel::Allow);
    db.set_layout(Path::new("/project/.git"), "sibling".to_string());
    db.save_to(&path).unwrap();

    // Verify V3 JSON shape
    let contents = std::fs::read_to_string(&path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&contents).unwrap();
    assert_eq!(json["version"], 3);
    let repo_entry = &json["repositories"]["/project/.git"];
    assert!(repo_entry["trust"]["level"].is_string());
    assert_eq!(repo_entry["layout"], "sibling");

    // Verify round-trip
    let loaded = TrustDatabase::load_from(&path).unwrap();
    assert_eq!(loaded.get_layout(Path::new("/project/.git")), Some("sibling"));
    assert_eq!(loaded.get_trust_level(Path::new("/project/.git")), TrustLevel::Allow);
}

#[test]
fn test_prune_cleans_layouts() {
    let mut db = TrustDatabase::default();
    db.set_layout(Path::new("/nonexistent/.git"), "contained".to_string());
    let removed = db.prune();
    assert!(!removed.is_empty());
    assert!(db.get_layout(Path::new("/nonexistent/.git")).is_none());
}

#[test]
fn test_atomic_trust_json_migration() {
    let temp_dir = tempdir().unwrap();
    let trust_path = temp_dir.path().join("trust.json");
    let repos_path = temp_dir.path().join("repos.json");

    let v2_json = r#"{
        "version": 2,
        "default_level": "deny",
        "repositories": {
            "/project/.git": {
                "level": "allow",
                "granted_at": 1738060200,
                "granted_by": "user",
                "fingerprint": "https://github.com/user/repo.git"
            }
        },
        "patterns": []
    }"#;
    std::fs::write(&trust_path, v2_json).unwrap();

    let db = TrustDatabase::load_from(&trust_path).unwrap();
    assert_eq!(db.version, 3);
    assert_eq!(db.get_trust_level(Path::new("/project/.git")), TrustLevel::Allow);

    // trust.json should be removed, repos.json should exist
    assert!(repos_path.exists());
    assert!(!trust_path.exists());

    // repos.json should be V3 format
    let contents = std::fs::read_to_string(&repos_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&contents).unwrap();
    assert_eq!(json["version"], 3);
    assert!(json["repositories"]["/project/.git"]["trust"].is_object());
}
```

- [ ] **Step 8: Run all trust tests**

Run: `cargo test -p daft --lib hooks::trust -- --nocapture`

Expected: all tests pass (existing + new). Note: some existing tests that check
`db.version == 2` will need updating to `3`.

- [ ] **Step 9: Run full unit test suite**

Run: `mise run test:unit`

Expected: all pass. If any existing tests relied on `version: 2` or the V2 JSON
shape, update them.

- [ ] **Step 10: Run clippy and fmt, commit**

```bash
mise run fmt && mise run clippy
git add src/hooks/trust.rs src/hooks/trust_dto.rs
git commit -m "feat(layout): unified repo store with V3 schema, layout field, and atomic trust.json migration"
```

---

## Task 6: daft.yml Layout Field

**Files:**

- Modify: `src/hooks/yaml_config.rs` — add `layout` field to `YamlConfig`

### Steps

- [ ] **Step 1: Add `layout` field to `YamlConfig` struct**

In `src/hooks/yaml_config.rs`, add after the `source_dir_local` field:

```rust
    /// Layout suggestion for this repository.
    ///
    /// Accepts a named layout (e.g., "contained") or an inline template string.
    /// This is a team convention that can be overridden by the user's local
    /// config in repos.json.
    pub layout: Option<String>,
```

- [ ] **Step 2: Write test**

Add to the test module in `yaml_config.rs` (or create one if there isn't one).
If there's no test module in this file, check the test patterns in the hooks
module and follow them. A simple round-trip test:

```rust
#[test]
fn test_yaml_config_with_layout() {
    let yaml = r#"
layout: contained
hooks:
  post-clone:
    jobs:
      - run: echo hello
"#;
    let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.layout, Some("contained".into()));
}

#[test]
fn test_yaml_config_without_layout() {
    let yaml = r#"
hooks:
  post-clone:
    jobs:
      - run: echo hello
"#;
    let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.layout, None);
}

#[test]
fn test_yaml_config_with_inline_template_layout() {
    let yaml = r#"
layout: "../.worktrees/{{ repo }}/{{ branch | sanitize }}"
hooks: {}
"#;
    let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(
        config.layout,
        Some("../.worktrees/{{ repo }}/{{ branch | sanitize }}".into())
    );
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p daft --lib hooks::yaml_config -- --nocapture`

Expected: tests pass (the field is just an `Option<String>` with serde default).

- [ ] **Step 4: Run clippy and fmt, commit**

```bash
mise run fmt && mise run clippy
git add src/hooks/yaml_config.rs
git commit -m "feat(layout): add layout field to daft.yml configuration"
```

---

## Task 7: Config Resolution Chain

**Files:**

- Create: `src/core/layout/resolver.rs`
- Modify: `src/core/layout/mod.rs` — add `pub mod resolver;`

### Steps

- [ ] **Step 1: Write failing tests**

Create `src/core/layout/resolver.rs`:

```rust
//! Layout configuration resolution chain.
//!
//! Resolution order:
//! 1. CLI `--layout` flag
//! 2. Per-repo store (repos.json)
//! 3. daft.yml `layout` field (team convention)
//! 4. Global config `defaults.layout`
//! 5. Built-in default (sibling)

use std::path::Path;

use anyhow::Result;

use super::{BuiltinLayout, Layout, DEFAULT_LAYOUT};
use crate::core::global_config::GlobalConfig;

/// Inputs for layout resolution.
///
/// Each field represents one level of the resolution chain. The resolver checks
/// them in order and returns the first that resolves to a valid Layout.
pub struct LayoutResolutionContext<'a> {
    /// CLI `--layout` flag value (highest priority).
    pub cli_layout: Option<&'a str>,
    /// Layout from per-repo store (repos.json), looked up by git_dir.
    pub repo_store_layout: Option<&'a str>,
    /// Layout from daft.yml `layout` field.
    pub yaml_layout: Option<&'a str>,
    /// Global config (for defaults and custom layout definitions).
    pub global_config: &'a GlobalConfig,
}

/// Resolve a layout from the configuration chain.
///
/// Returns the resolved Layout and which level it came from (for diagnostics
/// in `daft layout show`).
pub fn resolve_layout(ctx: &LayoutResolutionContext) -> (Layout, LayoutSource) {
    todo!()
}

/// Which level of the config chain provided the resolved layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutSource {
    /// From `--layout` CLI flag.
    Cli,
    /// From per-repo store (repos.json).
    RepoStore,
    /// From daft.yml in the repository.
    YamlConfig,
    /// From global config file.
    GlobalConfig,
    /// Built-in default.
    Default,
}

/// Resolve a layout string (name or inline template) using the global config
/// for name lookups.
fn resolve_layout_string(value: &str, global_config: &GlobalConfig) -> Layout {
    // Try named layout (custom or built-in)
    if let Some(layout) = global_config.resolve_layout_by_name(value) {
        return layout;
    }
    // Treat as inline template
    Layout {
        name: value.to_string(),
        template: value.to_string(),
        bare: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_global() -> GlobalConfig {
        GlobalConfig::default()
    }

    #[test]
    fn test_cli_flag_wins() {
        let global = default_global();
        let ctx = LayoutResolutionContext {
            cli_layout: Some("contained"),
            repo_store_layout: Some("sibling"),
            yaml_layout: Some("nested"),
            global_config: &global,
        };
        let (layout, source) = resolve_layout(&ctx);
        assert_eq!(layout.name, "contained");
        assert_eq!(source, LayoutSource::Cli);
    }

    #[test]
    fn test_repo_store_second() {
        let global = default_global();
        let ctx = LayoutResolutionContext {
            cli_layout: None,
            repo_store_layout: Some("nested"),
            yaml_layout: Some("sibling"),
            global_config: &global,
        };
        let (layout, source) = resolve_layout(&ctx);
        assert_eq!(layout.name, "nested");
        assert_eq!(source, LayoutSource::RepoStore);
    }

    #[test]
    fn test_yaml_config_third() {
        let global = default_global();
        let ctx = LayoutResolutionContext {
            cli_layout: None,
            repo_store_layout: None,
            yaml_layout: Some("contained"),
            global_config: &global,
        };
        let (layout, source) = resolve_layout(&ctx);
        assert_eq!(layout.name, "contained");
        assert_eq!(source, LayoutSource::YamlConfig);
    }

    #[test]
    fn test_global_config_fourth() {
        let global: GlobalConfig = toml::from_str(r#"
[defaults]
layout = "nested"
"#).unwrap();
        let ctx = LayoutResolutionContext {
            cli_layout: None,
            repo_store_layout: None,
            yaml_layout: None,
            global_config: &global,
        };
        let (layout, source) = resolve_layout(&ctx);
        assert_eq!(layout.name, "nested");
        assert_eq!(source, LayoutSource::GlobalConfig);
    }

    #[test]
    fn test_default_fallback() {
        let global = default_global();
        let ctx = LayoutResolutionContext {
            cli_layout: None,
            repo_store_layout: None,
            yaml_layout: None,
            global_config: &global,
        };
        let (layout, source) = resolve_layout(&ctx);
        assert_eq!(layout.name, DEFAULT_LAYOUT.name());
        assert_eq!(source, LayoutSource::Default);
    }

    #[test]
    fn test_inline_template_from_cli() {
        let global = default_global();
        let ctx = LayoutResolutionContext {
            cli_layout: Some("custom/{{ branch | sanitize }}"),
            repo_store_layout: None,
            yaml_layout: None,
            global_config: &global,
        };
        let (layout, source) = resolve_layout(&ctx);
        assert_eq!(layout.template, "custom/{{ branch | sanitize }}");
        assert_eq!(source, LayoutSource::Cli);
    }

    #[test]
    fn test_inline_template_from_yaml() {
        let global = default_global();
        let ctx = LayoutResolutionContext {
            cli_layout: None,
            repo_store_layout: None,
            yaml_layout: Some("../custom/{{ branch | sanitize }}"),
            global_config: &global,
        };
        let (layout, source) = resolve_layout(&ctx);
        assert_eq!(layout.template, "../custom/{{ branch | sanitize }}");
        assert_eq!(source, LayoutSource::YamlConfig);
    }

    #[test]
    fn test_inline_template_from_repo_store() {
        let global = default_global();
        let ctx = LayoutResolutionContext {
            cli_layout: None,
            repo_store_layout: Some(".trees/{{ branch | sanitize }}"),
            yaml_layout: None,
            global_config: &global,
        };
        let (layout, source) = resolve_layout(&ctx);
        assert_eq!(layout.template, ".trees/{{ branch | sanitize }}");
        assert_eq!(source, LayoutSource::RepoStore);
    }

    #[test]
    fn test_custom_layout_from_global_config() {
        let global: GlobalConfig = toml::from_str(r#"
[defaults]
layout = "my-team"

[layouts.my-team]
template = "../.worktrees/{{ repo }}/{{ branch | sanitize }}"
"#).unwrap();
        let ctx = LayoutResolutionContext {
            cli_layout: None,
            repo_store_layout: None,
            yaml_layout: None,
            global_config: &global,
        };
        let (layout, source) = resolve_layout(&ctx);
        assert_eq!(layout.name, "my-team");
        assert_eq!(
            layout.template,
            "../.worktrees/{{ repo }}/{{ branch | sanitize }}"
        );
        assert_eq!(source, LayoutSource::GlobalConfig);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p daft --lib layout::resolver -- --nocapture 2>&1 | head -20`

- [ ] **Step 3: Implement `resolve_layout`**

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

    (DEFAULT_LAYOUT.to_layout(), LayoutSource::Default)
}
```

- [ ] **Step 4: Add `pub mod resolver;` to `src/core/layout/mod.rs`**

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p daft --lib layout::resolver -- --nocapture`

- [ ] **Step 6: Run all unit tests to verify nothing is broken**

Run: `mise run test:unit`

Expected: all existing tests still pass plus the new ones.

- [ ] **Step 7: Run clippy and fmt, commit**

```bash
mise run fmt && mise run clippy
git add src/core/layout/resolver.rs src/core/layout/mod.rs
git commit -m "feat(layout): add config resolution chain (CLI > repo store > daft.yml > global > default)"
```

---

## Task 8: Re-export and Wire Up

**Files:**

- Modify: `src/lib.rs` — re-export layout module

### Steps

- [ ] **Step 1: Add re-exports to `src/lib.rs`**

Add after the existing re-exports:

```rust
pub use self::core::global_config;
pub use self::core::layout;
```

- [ ] **Step 2: Run full test suite**

Run: `mise run test:unit`

Expected: all tests pass.

- [ ] **Step 3: Run clippy**

Run: `mise run clippy`

Expected: zero warnings.

- [ ] **Step 4: Commit**

```bash
mise run fmt
git add src/lib.rs
git commit -m "feat(layout): re-export layout and global config modules"
```
