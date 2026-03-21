//! Bare repository inference from template geometry.

/// Infer whether a layout template requires a bare repository.
///
/// Rules:
/// 1. If `explicit_bare` is provided, use it (custom layout override).
/// 2. Template starts with `{{ repo_path }}/` — bare required (worktrees are
///    placed inside the repo root, which conflicts with a working tree).
/// 3. Everything else — not bare (worktrees are outside the repo).
pub fn infer_bare(template: &str, explicit_bare: Option<bool>) -> bool {
    if let Some(bare) = explicit_bare {
        return bare;
    }

    template.starts_with("{{ repo_path }}/")
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

    // Rule 2: repo_path-anchored → bare
    #[test]
    fn test_contained_is_bare() {
        assert!(infer_bare("{{ repo_path }}/{{ branch }}", None));
    }
    #[test]
    fn test_repo_path_subdir_is_bare() {
        assert!(infer_bare(
            "{{ repo_path }}/worktrees/{{ branch | sanitize }}",
            None
        ));
    }

    // Rule 3: everything else → not bare
    #[test]
    fn test_sibling_not_bare() {
        assert!(!infer_bare("{{ repo }}.{{ branch | sanitize }}", None));
    }
    #[test]
    fn test_nested_not_bare() {
        assert!(!infer_bare(
            "{{ repo }}/.worktrees/{{ branch | sanitize }}",
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
    #[test]
    fn test_daft_data_dir_path_not_bare() {
        assert!(!infer_bare(
            "{{ daft_data_dir }}/worktrees/{{ repo }}/{{ branch | sanitize }}",
            None
        ));
    }
    #[test]
    fn test_repo_name_relative_not_bare() {
        assert!(!infer_bare("{{ repo }}/{{ branch | sanitize }}", None));
    }
}
