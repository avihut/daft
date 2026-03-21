use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

/// Context for resolving template variables.
pub struct TemplateContext {
    pub repo_path: PathBuf,
    pub repo: String,
    pub branch: String,
}

/// Sanitize a branch name for use as a filesystem path component.
/// Replaces `/` and `\` with `-`.
pub fn sanitize(s: &str) -> String {
    let mut out = s.to_string();
    out = out.replace('/', "-");
    out = out.replace('\\', "-");
    out
}

/// Render a template string with the given context.
/// Supported: `{{ variable }}` and `{{ variable | sanitize }}`
/// Variables: `repo_path`, `repo`, `branch`
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
        "daft_data_dir" => crate::daft_data_dir()?.to_string_lossy().to_string(),
        _ => bail!("Unknown template variable: {var_name}"),
    };

    match filter {
        None => Ok(raw_value),
        Some("sanitize") => Ok(sanitize(&raw_value)),
        Some(f) => bail!("Unknown template filter: {f}"),
    }
}

/// Resolve a rendered template path to an absolute PathBuf.
///
/// - Absolute paths (starting with `/`) are used as-is.
/// - Home-relative paths (starting with `~/`) expand to the home directory.
/// - Relative paths are resolved against the **parent directory** of
///   `repo_path` — i.e., the directory that contains the repository.
///
/// Templates that use `{{ repo_path }}` render to absolute paths and bypass
/// the relative resolution entirely. Templates that use `{{ repo }}` produce
/// relative paths like `myrepo.feature-auth` which resolve next to the repo.
///
/// All paths are normalized (`..` components resolved without filesystem access).
pub fn resolve_path(rendered: &str, repo_path: &Path) -> Result<PathBuf> {
    if rendered.starts_with('/') {
        return Ok(normalize_path(Path::new(rendered)));
    }
    if let Some(rest_of_path) = rendered.strip_prefix("~/") {
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;
        return Ok(normalize_path(&home.join(rest_of_path)));
    }
    // Relative paths resolve against the parent of repo_path
    let parent = repo_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Repository path has no parent directory"))?;
    Ok(normalize_path(&parent.join(rendered)))
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::env;

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
    fn test_render_sibling_template() {
        let ctx = TemplateContext {
            repo_path: PathBuf::from("/home/user/myproject"),
            repo: "myproject".into(),
            branch: "feature/auth".into(),
        };
        assert_eq!(
            render("{{ repo }}.{{ branch | sanitize }}", &ctx).unwrap(),
            "myproject.feature-auth"
        );
    }

    #[test]
    fn test_render_contained_template() {
        let ctx = TemplateContext {
            repo_path: PathBuf::from("/home/user/myproject"),
            repo: "myproject".into(),
            branch: "main".into(),
        };
        assert_eq!(
            render("{{ repo_path }}/{{ branch }}", &ctx).unwrap(),
            "/home/user/myproject/main"
        );
    }

    #[test]
    #[serial]
    fn test_render_centralized_template() {
        env::set_var("DAFT_DATA_DIR", "/tmp/daft-test-data");
        let ctx = TemplateContext {
            repo_path: PathBuf::from("/home/user/myproject"),
            repo: "myproject".into(),
            branch: "feature/auth".into(),
        };
        let rendered = render(
            "{{ daft_data_dir }}/worktrees/{{ repo }}/{{ branch | sanitize }}",
            &ctx,
        )
        .unwrap();
        assert_eq!(
            rendered,
            "/tmp/daft-test-data/worktrees/myproject/feature-auth"
        );
        env::remove_var("DAFT_DATA_DIR");
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
    fn test_resolve_path_relative_sibling() {
        let repo = Path::new("/home/user/myproject");
        let resolved = resolve_path("myproject.feature-auth", repo).unwrap();
        assert_eq!(resolved, PathBuf::from("/home/user/myproject.feature-auth"));
    }

    #[test]
    fn test_resolve_path_relative_subdir() {
        let repo = Path::new("/home/user/myproject");
        let resolved = resolve_path("myproject/main", repo).unwrap();
        assert_eq!(resolved, PathBuf::from("/home/user/myproject/main"));
    }

    #[test]
    fn test_resolve_path_absolute() {
        let repo = Path::new("/home/user/myproject");
        let resolved = resolve_path("/tmp/worktrees/feature-auth", repo).unwrap();
        assert_eq!(resolved, PathBuf::from("/tmp/worktrees/feature-auth"));
    }

    #[test]
    fn test_resolve_path_home_expansion() {
        let repo = Path::new("/home/user/myproject");
        let resolved = resolve_path("~/worktrees/myproject/main", repo).unwrap();
        assert!(!resolved.starts_with("~"));
        assert!(resolved.ends_with("worktrees/myproject/main"));
    }

    #[test]
    fn test_resolve_path_absolute_with_repo_path() {
        // Templates using {{ repo_path }} render to absolute paths
        let repo = Path::new("/home/user/myproject");
        let resolved = resolve_path("/home/user/myproject/.worktrees/feature-auth", repo).unwrap();
        assert_eq!(
            resolved,
            PathBuf::from("/home/user/myproject/.worktrees/feature-auth")
        );
    }
}
