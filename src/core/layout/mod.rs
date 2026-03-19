pub mod bare;
pub mod template;

use std::path::PathBuf;

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
}
