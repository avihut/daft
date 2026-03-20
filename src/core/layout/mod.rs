pub mod bare;
pub mod resolver;
pub mod template;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;

pub use template::TemplateContext;

/// A worktree layout definition.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub name: String,
    pub template: String,
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
    pub fn needs_bare(&self) -> bool {
        bare::infer_bare(&self.template, self.bare)
    }

    pub fn worktree_path(&self, ctx: &TemplateContext) -> Result<PathBuf> {
        let rendered = template::render(&self.template, ctx)?;
        template::resolve_path(&rendered, &ctx.repo_path)
    }
}

/// Ensure `pattern` is present in `<repo_path>/.gitignore`.
///
/// Appends the pattern (on its own line) if it is not already present.
/// Creates `.gitignore` if the file does not exist yet.
pub fn ensure_gitignore_entry(repo_path: &Path, pattern: &str) -> Result<()> {
    let gitignore = repo_path.join(".gitignore");
    if gitignore.exists() {
        let contents = fs::read_to_string(&gitignore)?;
        if contents.lines().any(|line| line.trim() == pattern) {
            return Ok(()); // Already present
        }
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&gitignore)?;
    writeln!(file, "{pattern}")?;
    Ok(())
}

/// Add the worktree parent directory to `.gitignore` when the worktree
/// lives inside the repository root (e.g. the `nested` layout).
///
/// Only acts when:
/// - `layout` is provided and does **not** require a bare repo
/// - `worktree_path` starts with `project_root` (i.e. it is inside the repo)
///
/// The pattern added is the first path component of `worktree_path` relative
/// to `project_root` with a trailing `/` — for example `.worktrees/`.
pub fn auto_gitignore_if_needed(
    project_root: &Path,
    worktree_path: &Path,
    layout: Option<&Layout>,
) -> Result<()> {
    let Some(layout) = layout else {
        return Ok(());
    };
    if layout.needs_bare() {
        return Ok(());
    }
    let Ok(relative) = worktree_path.strip_prefix(project_root) else {
        return Ok(()); // worktree is outside the repo
    };
    let first_component = relative
        .components()
        .next()
        .map(|c| c.as_os_str().to_string_lossy().into_owned());
    let Some(dir_name) = first_component else {
        return Ok(());
    };
    let pattern = format!("{dir_name}/");
    ensure_gitignore_entry(project_root, &pattern)
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
        assert_eq!(BuiltinLayout::all().len(), 4);
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
        assert_eq!(path, PathBuf::from("/home/user/myproject.feature-auth"));
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

    // ── ensure_gitignore_entry tests ────────────────────────────────────────

    #[test]
    fn test_ensure_gitignore_creates_file_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        ensure_gitignore_entry(dir.path(), ".worktrees/").unwrap();
        let contents = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(contents.lines().any(|l| l.trim() == ".worktrees/"));
    }

    #[test]
    fn test_ensure_gitignore_appends_when_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let gi = dir.path().join(".gitignore");
        fs::write(&gi, "node_modules/\n").unwrap();
        ensure_gitignore_entry(dir.path(), ".worktrees/").unwrap();
        let contents = fs::read_to_string(&gi).unwrap();
        assert!(contents.contains("node_modules/"));
        assert!(contents.contains(".worktrees/"));
    }

    #[test]
    fn test_ensure_gitignore_does_not_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let gi = dir.path().join(".gitignore");
        fs::write(&gi, ".worktrees/\n").unwrap();
        ensure_gitignore_entry(dir.path(), ".worktrees/").unwrap();
        let contents = fs::read_to_string(&gi).unwrap();
        let count = contents
            .lines()
            .filter(|l| l.trim() == ".worktrees/")
            .count();
        assert_eq!(count, 1);
    }

    // ── auto_gitignore_if_needed tests ──────────────────────────────────────

    #[test]
    fn test_auto_gitignore_nested_layout() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        let worktree = project_root.join(".worktrees").join("feature-auth");
        let layout = BuiltinLayout::Nested.to_layout();
        auto_gitignore_if_needed(project_root, &worktree, Some(&layout)).unwrap();
        let contents = fs::read_to_string(project_root.join(".gitignore")).unwrap();
        assert!(contents.lines().any(|l| l.trim() == ".worktrees/"));
    }

    #[test]
    fn test_auto_gitignore_no_op_when_layout_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        let worktree = project_root.join(".worktrees").join("feature-auth");
        auto_gitignore_if_needed(project_root, &worktree, None).unwrap();
        assert!(!project_root.join(".gitignore").exists());
    }

    #[test]
    fn test_auto_gitignore_no_op_for_bare_layout() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        // Contained layout: bare=true, worktrees are direct children of repo
        let layout = BuiltinLayout::Contained.to_layout();
        assert!(layout.needs_bare());
        let worktree = project_root.join("feature-auth");
        auto_gitignore_if_needed(project_root, &worktree, Some(&layout)).unwrap();
        assert!(!project_root.join(".gitignore").exists());
    }

    #[test]
    fn test_auto_gitignore_no_op_for_outside_repo() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path().join("repo");
        let worktree = dir.path().join("sibling");
        let layout = BuiltinLayout::Sibling.to_layout();
        auto_gitignore_if_needed(&project_root, &worktree, Some(&layout)).unwrap();
        assert!(!project_root.join(".gitignore").exists());
    }

    #[test]
    fn test_auto_gitignore_custom_non_bare_in_repo() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        let worktree = project_root.join("wt").join("feature-x");
        let layout = Layout {
            name: "custom".into(),
            template: "wt/{{ branch | sanitize }}".into(),
            bare: Some(false),
        };
        auto_gitignore_if_needed(project_root, &worktree, Some(&layout)).unwrap();
        let contents = fs::read_to_string(project_root.join(".gitignore")).unwrap();
        assert!(contents.lines().any(|l| l.trim() == "wt/"));
    }
}
