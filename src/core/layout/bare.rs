//! Bare repository inference from template geometry.

/// Infer whether a layout template requires a bare repository.
///
/// Rules (in order):
/// 1. If `explicit_bare` is provided, use it (custom layout override).
/// 2. Template contains the `repo` filter — **not bare** (the default branch
///    is a regular clone, i.e., "wrapped non-bare" mode).
/// 3. Template starts with `{{ repo_path }}/` — bare required (worktrees are
///    placed inside the repo root, which conflicts with a working tree).
/// 4. Everything else — not bare (worktrees are outside the repo).
pub fn infer_bare(template: &str, explicit_bare: Option<bool>) -> bool {
    if let Some(bare) = explicit_bare {
        return bare;
    }

    if has_repo_filter(template) {
        return false;
    }

    template.starts_with("{{ repo_path }}/")
}

/// Check whether a template uses the `repo` filter in any expression.
///
/// This parses `{{ ... }}` expressions and checks if any of them contain
/// `repo` as a filter (after the first `|`). This is used by bare inference
/// to detect wrapped non-bare layouts like `contained-classic`.
pub fn has_repo_filter(template: &str) -> bool {
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        if let Some(end) = after.find("}}") {
            let expr = after[..end].trim();
            let parts: Vec<&str> = expr.split('|').map(|s| s.trim()).collect();
            if parts.iter().skip(1).any(|p| *p == "repo") {
                return true;
            }
        }
        rest = &after[after.find("}}").map(|e| e + 2).unwrap_or(after.len())..];
    }
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

    // Rule 2: repo filter → not bare
    #[test]
    fn test_contained_classic_not_bare() {
        assert!(!infer_bare("{{ repo_path }}/{{ branch | repo }}", None));
    }
    #[test]
    fn test_repo_filter_chained_not_bare() {
        assert!(!infer_bare(
            "{{ repo_path }}/{{ branch | repo | sanitize }}",
            None
        ));
    }
    #[test]
    fn test_repo_filter_reverse_chain_not_bare() {
        assert!(!infer_bare(
            "{{ repo_path }}/{{ branch | sanitize | repo }}",
            None
        ));
    }

    // Rule 3: repo_path-anchored → bare
    #[test]
    fn test_contained_is_bare() {
        assert!(infer_bare("{{ repo_path }}/{{ branch }}", None));
    }
    #[test]
    fn test_contained_flat_is_bare() {
        assert!(infer_bare("{{ repo_path }}/{{ branch | sanitize }}", None));
    }
    #[test]
    fn test_repo_path_subdir_is_bare() {
        assert!(infer_bare(
            "{{ repo_path }}/worktrees/{{ branch | sanitize }}",
            None
        ));
    }

    // Rule 4: everything else → not bare
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

    // has_repo_filter tests
    #[test]
    fn test_has_repo_filter_basic() {
        assert!(has_repo_filter("{{ branch | repo }}"));
    }
    #[test]
    fn test_has_repo_filter_chained() {
        assert!(has_repo_filter("{{ branch | repo | sanitize }}"));
    }
    #[test]
    fn test_has_repo_filter_absent() {
        assert!(!has_repo_filter("{{ branch | sanitize }}"));
    }
    #[test]
    fn test_has_repo_filter_in_full_template() {
        assert!(has_repo_filter("{{ repo_path }}/{{ branch | repo }}"));
    }
    #[test]
    fn test_has_repo_filter_not_variable() {
        // "repo" as a variable name, not a filter
        assert!(!has_repo_filter("{{ repo }}.{{ branch | sanitize }}"));
    }
}
