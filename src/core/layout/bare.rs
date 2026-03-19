//! Bare repository inference from template geometry.

/// Infer whether a layout template requires a bare repository.
///
/// Rules:
/// 1. If `explicit_bare` is provided, use it (custom layout override).
/// 2. Starts with `../`, `/`, or `~/` — not bare (worktrees outside repo).
/// 3. First path segment starts with `.` — not bare (hidden directory).
/// 4. Otherwise — bare required (worktrees would conflict with working tree).
pub fn infer_bare(template: &str, explicit_bare: Option<bool>) -> bool {
    if let Some(bare) = explicit_bare {
        return bare;
    }
    if template.starts_with("../") || template.starts_with('/') || template.starts_with("~/") {
        return false;
    }
    let first_segment = template.split('/').next().unwrap_or("");
    if first_segment.starts_with('.') {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_explicit_bare_true() {
        assert!(infer_bare("anything", Some(true)));
    }
    #[test]
    fn test_explicit_bare_false() {
        assert!(!infer_bare("{{ branch | sanitize }}", Some(false)));
    }
    #[test]
    fn test_parent_relative_not_bare() {
        assert!(!infer_bare("../{{ repo }}.{{ branch | sanitize }}", None));
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
        assert!(infer_bare("trees/{{ repo }}/{{ branch | sanitize }}", None));
    }
}
