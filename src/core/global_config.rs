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
    pub defaults: GlobalDefaults,
    pub layouts: HashMap<String, CustomLayoutDef>,
}

/// Default settings section.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct GlobalDefaults {
    pub layout: Option<String>,
}

/// Custom layout definition in config.
#[derive(Debug, Clone, Deserialize)]
pub struct CustomLayoutDef {
    pub template: String,
    pub bare: Option<bool>,
}

impl GlobalConfig {
    pub fn load() -> Result<Self> {
        let path = Self::default_path()?;
        Self::load_from(&path)
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config from {}", path.display()))?;
        toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config from {}", path.display()))
    }

    pub fn default_path() -> Result<PathBuf> {
        Ok(crate::daft_config_dir()?.join("config.toml"))
    }

    /// Look up a layout by name: custom layouts first, then built-ins.
    pub fn resolve_layout_by_name(&self, name: &str) -> Option<Layout> {
        if let Some(custom) = self.layouts.get(name) {
            return Some(Layout {
                name: name.to_string(),
                template: custom.template.clone(),
                bare: custom.bare,
            });
        }
        BuiltinLayout::from_name(name).map(|b| b.to_layout())
    }

    /// Set the default layout in the config file.
    ///
    /// Reads the existing config file, updates or adds `defaults.layout`,
    /// and writes it back. Preserves all other content.
    pub fn set_default_layout(layout_name: &str) -> Result<()> {
        let path = Self::default_path()?;

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let contents = if path.exists() {
            std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read config from {}", path.display()))?
        } else {
            String::new()
        };

        let new_line = format!("layout = \"{}\"", layout_name);

        let updated = if contents.contains("layout = ") {
            // Replace existing layout line
            let mut result = String::new();
            for line in contents.lines() {
                if line.trim().starts_with("layout = ") || line.trim().starts_with("layout=") {
                    result.push_str(&new_line);
                } else {
                    result.push_str(line);
                }
                result.push('\n');
            }
            result
        } else if contents.contains("[defaults]") {
            // Add layout under existing [defaults] section
            contents.replace("[defaults]", &format!("[defaults]\n{new_line}"))
        } else {
            // No [defaults] section — add it
            let mut result = contents;
            if !result.is_empty() && !result.ends_with('\n') {
                result.push('\n');
            }
            result.push_str(&format!("[defaults]\n{new_line}\n"));
            result
        };

        std::fs::write(&path, updated)
            .with_context(|| format!("Failed to write config to {}", path.display()))
    }

    /// Get the default layout, if configured.
    pub fn default_layout(&self) -> Option<Layout> {
        let name = self.defaults.layout.as_deref()?;
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
