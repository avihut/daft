//! Layout configuration resolution chain.
//!
//! Resolution order:
//! 1. CLI `--layout` flag
//! 2. Per-repo store (repos.json)
//! 3. daft.yml `layout` field (team convention)
//! 4. Global config `defaults.layout`
//! 5. Built-in default (sibling)

use super::{Layout, DEFAULT_LAYOUT};
use crate::core::global_config::GlobalConfig;

/// Inputs for layout resolution.
pub struct LayoutResolutionContext<'a> {
    pub cli_layout: Option<&'a str>,
    pub repo_store_layout: Option<&'a str>,
    pub yaml_layout: Option<&'a str>,
    pub global_config: &'a GlobalConfig,
}

/// Which level of the config chain provided the resolved layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutSource {
    Cli,
    RepoStore,
    YamlConfig,
    GlobalConfig,
    Default,
}

/// Resolve a layout from the configuration chain.
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

/// Resolve a layout string (name or inline template) using global config for lookups.
fn resolve_layout_string(value: &str, global_config: &GlobalConfig) -> Layout {
    if let Some(layout) = global_config.resolve_layout_by_name(value) {
        return layout;
    }
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
        let global: GlobalConfig = toml::from_str(
            r#"
[defaults]
layout = "nested"
"#,
        )
        .unwrap();
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
        let global: GlobalConfig = toml::from_str(
            r#"
[defaults]
layout = "my-team"

[layouts.my-team]
template = "../.worktrees/{{ repo }}/{{ branch | sanitize }}"
"#,
        )
        .unwrap();
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
