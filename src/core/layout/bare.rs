//! Bare repository inference from template geometry.

/// Infer whether a layout template requires a bare repository.
///
/// Rules:
/// 1. If `explicit_bare` is provided, use it (custom layout override).
/// 2. Template starts with `{{ repo_path }}/` — worktrees are inside the repo:
///    a. Next segment starts with `.` — not bare (hidden directory, gitignored).
///    b. Otherwise — bare required (worktrees conflict with working tree).
/// 3. Everything else — not bare (worktrees are outside the repo: sibling,
///    centralized, or absolute paths).
pub fn infer_bare(template: &str, explicit_bare: Option<bool>) -> bool {
    if let Some(bare) = explicit_bare {
        return bare;
    }

    // Templates anchored to repo_path place worktrees inside the repo
    const REPO_PATH_PREFIX: &str = "{{ repo_path }}/";
    if let Some(rest) = template.strip_prefix(REPO_PATH_PREFIX) {
        // Check the first segment after repo_path/
        let first_segment = rest.split('/').next().unwrap_or("");
        // Hidden directory (e.g., .worktrees/) → not bare, gitignored
        if first_segment.starts_with('.') {
            return false;
        }
        // Visible child of repo_path → bare required
        return true;
    }

    // Everything else: relative paths (sibling), absolute paths, ~/paths
    // All place worktrees outside the repo → not bare
    false
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
        assert!(!infer_bare("{{ repo_path }}/{{ branch }}", Some(false)));
    }

    // Rule 2: repo_path-anchored templates
    #[test]
    fn test_contained_is_bare() {
        assert!(infer_bare("{{ repo_path }}/{{ branch }}", None));
    }
    #[test]
    fn test_nested_hidden_not_bare() {
        assert!(!infer_bare(
            "{{ repo_path }}/.worktrees/{{ branch | sanitize }}",
            None
        ));
    }
    #[test]
    fn test_repo_path_visible_subdir_is_bare() {
        assert!(infer_bare(
            "{{ repo_path }}/worktrees/{{ branch | sanitize }}",
            None
        ));
    }

    // Rule 3: everything else is not bare
    #[test]
    fn test_sibling_not_bare() {
        assert!(!infer_bare("{{ repo }}.{{ branch | sanitize }}", None));
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
    #[test]
    fn test_repo_name_relative_not_bare() {
        assert!(!infer_bare("{{ repo }}/{{ branch | sanitize }}", None));
    }
}
