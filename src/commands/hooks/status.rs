use super::{find_project_hooks, styled_trust_level};
use crate::hooks::{
    yaml_config, yaml_config_loader, HookType, TrustDatabase, TrustLevel,
    DEPRECATED_HOOK_REMOVAL_VERSION, PROJECT_HOOKS_DIR,
};
use crate::styles::{bold, cyan, dim, green, red, yellow};
use crate::{get_current_worktree_path, get_git_common_dir, is_git_repository};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Show trust status and available hooks.
pub(super) fn cmd_status(path: &Path, short: bool) -> Result<()> {
    // Resolve the path to absolute
    let abs_path = path
        .canonicalize()
        .with_context(|| format!("Path does not exist: {}", path.display()))?;

    // Change to that directory temporarily to run git commands
    let original_dir = std::env::current_dir()?;
    std::env::set_current_dir(&abs_path)
        .with_context(|| format!("Cannot change to directory: {}", abs_path.display()))?;

    // Ensure we're in a git repository
    let result = (|| -> Result<()> {
        if !is_git_repository()? {
            anyhow::bail!("Not in a git repository: {}", abs_path.display());
        }

        let git_dir = get_git_common_dir()?;
        let db = TrustDatabase::load().context("Failed to load trust database")?;
        let trust_level = db.get_trust_level(&git_dir);
        let is_explicit = db.has_explicit_trust(&git_dir);

        // Determine path type and display
        let project_root = git_dir.parent().context("Invalid git directory")?;
        let is_repo_root = abs_path == project_root;
        let worktree_root = get_current_worktree_path().ok();
        let path_type = if is_repo_root {
            "repository"
        } else if worktree_root.as_deref() == Some(&abs_path) {
            "worktree"
        } else if worktree_root.is_some() {
            "subdirectory"
        } else {
            "unknown"
        };

        // Find shell script hooks
        let hooks = find_project_hooks(&git_dir)?;

        // Find YAML-configured hooks
        let yaml_cfg = find_yaml_config_for_status(&git_dir, worktree_root.as_deref())
            .ok()
            .flatten();
        let yaml_hook_names: Vec<String> = yaml_cfg
            .as_ref()
            .map(|c| {
                let mut names: Vec<String> = c.hooks.keys().cloned().collect();
                names.sort();
                names
            })
            .unwrap_or_default();

        if short {
            // Short format: PATH (type), optional repo line, then (LEVEL) hooks
            println!("{} {}", abs_path.display(), dim(&format!("({path_type})")));
            if !is_repo_root {
                println!("{} {}", project_root.display(), dim("(repository)"));
            }
            // Combine shell hook names and YAML hook names (deduped)
            let mut all_names: Vec<String> = hooks
                .iter()
                .filter_map(|h| h.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .collect();
            for name in &yaml_hook_names {
                if !all_names.contains(name) {
                    all_names.push(name.clone());
                }
            }
            all_names.sort();
            let hooks_str = if all_names.is_empty() {
                "none".to_string()
            } else {
                all_names.join(", ")
            };
            println!("{hooks_str} ({})", styled_trust_level(trust_level));
        } else {
            // Full format
            println!("{} {}", abs_path.display(), dim(&format!("({path_type})")));
            if !is_repo_root {
                println!("{} {}", project_root.display(), dim("(repository)"));
            }
            println!();

            // Trust status
            let trust_source = if is_explicit {
                String::new()
            } else {
                format!(" {}", dim("(default)"))
            };
            println!(
                "{} {}{}",
                bold("Trust level:"),
                styled_trust_level(trust_level),
                trust_source
            );
            println!("  {}", dim(trust_level_description(trust_level)));
            println!();

            // YAML hooks section
            if !yaml_hook_names.is_empty() {
                println!("{} {}:", bold("Hooks configured in"), cyan("daft.yml"));
                for name in &yaml_hook_names {
                    println!("  - {}", cyan(name));
                }
                if !hooks.is_empty() {
                    println!();
                }
            }

            // Shell script hooks section
            if hooks.is_empty() {
                if yaml_hook_names.is_empty() {
                    println!("{} {}:", bold("No hooks found in"), cyan(PROJECT_HOOKS_DIR));
                    println!(
                        "  {}",
                        dim("(Create scripts in .daft/hooks/ or configure daft.yml)")
                    );
                }
            } else {
                println!("{} {}:", bold("Shell hooks in"), cyan(PROJECT_HOOKS_DIR));
                for hook in &hooks {
                    let name = hook.file_name().unwrap_or_default().to_string_lossy();
                    let executable = is_executable(hook);
                    let status = if executable {
                        String::new()
                    } else {
                        format!(" {}", red("(not executable)"))
                    };
                    println!("  - {}{status}", cyan(&name));
                }
            }

            // Check for deprecated hook filenames among discovered shell hooks
            let deprecated_hooks: Vec<_> = hooks
                .iter()
                .filter_map(|hook_path| {
                    let name = hook_path.file_name()?.to_str()?;
                    let hook_type = HookType::from_filename(name)?;
                    let dep = hook_type.deprecated_filename()?;
                    if name == dep {
                        Some((dep, hook_type.filename()))
                    } else {
                        None
                    }
                })
                .collect();

            if !deprecated_hooks.is_empty() {
                println!();
                println!("{}", yellow("Deprecated hook names detected:"));
                for (old_name, new_name) in &deprecated_hooks {
                    println!("  {} -> {}", red(old_name), green(new_name));
                }
                println!("  Run '{}' to rename them.", cyan("git daft hooks migrate"));
                println!(
                    "  {}",
                    dim(&format!(
                        "Deprecated names will stop working in daft v{}.",
                        DEPRECATED_HOOK_REMOVAL_VERSION
                    ))
                );
            }

            println!();

            // Show commands with relative path
            // If we're inside the repo, "." works since trust resolves the git common dir
            let path_arg = if original_dir.starts_with(project_root) || original_dir == project_root
            {
                ".".to_string()
            } else {
                relative_path(&original_dir, project_root)
                    .display()
                    .to_string()
            };

            match trust_level {
                TrustLevel::Deny => {
                    println!("{}", bold("To enable hooks:"));
                    println!("  {}", cyan(&format!("git daft hooks trust {path_arg}")));
                    println!("  {}", cyan(&format!("git daft hooks prompt {path_arg}")));
                }
                TrustLevel::Prompt | TrustLevel::Allow => {
                    println!("{}", bold("To revoke trust:"));
                    println!(
                        "  {}  {}",
                        cyan(&format!("git daft hooks deny {path_arg}")),
                        dim("(explicitly deny)")
                    );
                    println!(
                        "  {}  {}",
                        cyan(&format!("git daft hooks trust reset {path_arg}")),
                        dim("(remove trust entry)")
                    );
                }
            }
        }

        Ok(())
    })();

    // Restore original directory
    std::env::set_current_dir(&original_dir)?;

    result
}

/// Get a human-readable description for a trust level.
fn trust_level_description(level: TrustLevel) -> &'static str {
    match level {
        TrustLevel::Deny => "Hooks will NOT run for this repository.",
        TrustLevel::Prompt => "You will be prompted before each hook execution.",
        TrustLevel::Allow => "Hooks will run automatically without prompting.",
    }
}

/// Find YAML-configured hooks for the status display.
///
/// Checks the given worktree first, then falls back to searching worktree
/// subdirectories of the project root (for the bare-clone case where the
/// caller is at the repo root rather than inside a worktree).
fn find_yaml_config_for_status(
    git_dir: &Path,
    worktree_root: Option<&Path>,
) -> Result<Option<yaml_config::YamlConfig>> {
    if let Some(wt) = worktree_root {
        if let Ok(Some(config)) = yaml_config_loader::load_merged_config(wt) {
            return Ok(Some(config));
        }
    }

    // Fall back: search worktree subdirectories of the project root
    let project_root = git_dir.parent().context("Invalid git directory")?;
    for entry in std::fs::read_dir(project_root)
        .into_iter()
        .flatten()
        .flatten()
    {
        let path = entry.path();
        if path.is_dir() && path.file_name().map(|n| n != ".git").unwrap_or(false) {
            if let Ok(Some(config)) = yaml_config_loader::load_merged_config(&path) {
                return Ok(Some(config));
            }
        }
    }

    Ok(None)
}

/// Check if a file is executable.
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true // Assume executable on non-Unix
}

/// Compute the shortest relative path from `from` to `to`.
///
/// Returns "." if they are the same directory.
fn relative_path(from: &Path, to: &Path) -> PathBuf {
    if from == to {
        return PathBuf::from(".");
    }

    // If `to` is a descendant of `from`, strip the prefix
    if let Ok(rel) = to.strip_prefix(from) {
        return rel.to_path_buf();
    }

    // If `from` is a descendant of `to`, go up with ".."
    if let Ok(rel) = from.strip_prefix(to) {
        let mut path = PathBuf::new();
        for _ in rel.components() {
            path.push("..");
        }
        return path;
    }

    // Find common ancestor and build relative path
    let from_components: Vec<_> = from.components().collect();
    let to_components: Vec<_> = to.components().collect();

    // Find common prefix length
    let common_len = from_components
        .iter()
        .zip(to_components.iter())
        .take_while(|(a, b)| a == b)
        .count();

    // Build path: go up from `from` to common ancestor, then down to `to`
    let mut path = PathBuf::new();
    for _ in common_len..from_components.len() {
        path.push("..");
    }
    for component in &to_components[common_len..] {
        path.push(component);
    }

    if path.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        path
    }
}
