//! Command suggestion support for unknown subcommands.
//!
//! Provides Levenshtein distance matching and error messages that mirror
//! git's "did you mean?" behavior.

/// All subcommands available via `daft <subcmd>`.
pub const DAFT_SUBCOMMANDS: &[&str] = &[
    "branch",
    "completions",
    "doctor",
    "hooks",
    "multi-remote",
    "release-notes",
    "setup",
    "shell-init",
    "worktree-carry",
    "worktree-checkout",
    "worktree-checkout-branch",
    "worktree-checkout-branch-from-default",
    "worktree-clone",
    "worktree-fetch",
    "worktree-flow-adopt",
    "worktree-flow-eject",
    "worktree-init",
    "worktree-prune",
];

/// Compute Levenshtein edit distance between two strings.
fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_len = a.len();
    let b_len = b.len();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    // Two-row optimization: only keep current and previous rows.
    let mut prev: Vec<usize> = (0..=b_len).collect();
    let mut curr = vec![0; b_len + 1];

    for (i, ca) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.chars().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(curr[j] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_len]
}

/// Find known commands similar to `input`, sorted by edit distance (closest first).
///
/// Returns at most 5 suggestions. Only includes commands whose edit distance
/// is within a reasonable threshold.
pub fn find_similar_commands<'a>(input: &str, known: &[&'a str]) -> Vec<&'a str> {
    let mut candidates: Vec<(&str, usize)> = known
        .iter()
        .filter_map(|&cmd| {
            let dist = levenshtein_distance(input, cmd);
            if dist == 0 {
                return None; // exact match already routed
            }
            let max_len = input.len().max(cmd.len());
            let threshold = 3.max(max_len / 3);
            if dist <= threshold {
                Some((cmd, dist))
            } else {
                None
            }
        })
        .collect();

    candidates.sort_by_key(|&(_, dist)| dist);
    candidates.truncate(5);
    candidates.into_iter().map(|(cmd, _)| cmd).collect()
}

/// Print an error message for an unknown subcommand and exit with code 1.
///
/// Mirrors git's error format:
/// ```text
/// daft: 'foo' is not a daft command. See 'daft --help'.
///
/// The most similar command is
///     setup
/// ```
pub fn handle_unknown_subcommand(label: &str, unknown_cmd: &str, known: &[&str]) -> ! {
    eprintln!("{label}: '{unknown_cmd}' is not a {label} command. See '{label} --help'.");

    let suggestions = find_similar_commands(unknown_cmd, known);
    if !suggestions.is_empty() {
        eprintln!();
        if suggestions.len() == 1 {
            eprintln!("The most similar command is");
        } else {
            eprintln!("The most similar commands are");
        }
        for s in &suggestions {
            eprintln!("\t{s}");
        }
    }

    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Levenshtein distance tests ---

    #[test]
    fn test_levenshtein_identical() {
        assert_eq!(levenshtein_distance("abc", "abc"), 0);
    }

    #[test]
    fn test_levenshtein_empty() {
        assert_eq!(levenshtein_distance("", "abc"), 3);
        assert_eq!(levenshtein_distance("abc", ""), 3);
        assert_eq!(levenshtein_distance("", ""), 0);
    }

    #[test]
    fn test_levenshtein_single_edit() {
        // substitution
        assert_eq!(levenshtein_distance("cat", "car"), 1);
        // insertion
        assert_eq!(levenshtein_distance("cat", "cats"), 1);
        // deletion
        assert_eq!(levenshtein_distance("cats", "cat"), 1);
    }

    #[test]
    fn test_levenshtein_multiple_edits() {
        assert_eq!(levenshtein_distance("kitten", "sitting"), 3);
        assert_eq!(levenshtein_distance("saturday", "sunday"), 3);
    }

    #[test]
    fn test_levenshtein_completely_different() {
        assert_eq!(levenshtein_distance("abc", "xyz"), 3);
    }

    // --- find_similar_commands tests ---

    #[test]
    fn test_find_similar_typo() {
        let known = &["setup", "shell-init", "hooks"];
        let suggestions = find_similar_commands("steup", known);
        assert_eq!(suggestions, vec!["setup"]);
    }

    #[test]
    fn test_find_similar_close_match() {
        let known = &["branch", "hooks", "setup"];
        let suggestions = find_similar_commands("hook", known);
        assert_eq!(suggestions, vec!["hooks"]);
    }

    #[test]
    fn test_find_similar_no_match() {
        let known = &["branch", "hooks", "setup"];
        let suggestions = find_similar_commands("completely-unrelated-xyzzy", known);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_find_similar_exact_match_excluded() {
        let known = &["setup", "hooks"];
        let suggestions = find_similar_commands("setup", known);
        assert!(!suggestions.contains(&"setup"));
    }

    #[test]
    fn test_find_similar_sorted_by_distance() {
        let known = &["worktree-clone", "worktree-close", "worktree-carry"];
        let suggestions = find_similar_commands("worktree-cloen", known);
        // "worktree-clone" (dist 2) should come before "worktree-close" (dist 3)
        assert!(!suggestions.is_empty());
        assert_eq!(suggestions[0], "worktree-clone");
    }

    #[test]
    fn test_find_similar_max_five() {
        // Create many similar strings
        let known = &["aa", "ab", "ac", "ad", "ae", "af", "ag", "ah", "ai", "aj"];
        let suggestions = find_similar_commands("a", known);
        assert!(suggestions.len() <= 5);
    }

    // --- Subcommand list consistency tests ---

    #[test]
    fn test_daft_subcommands_sorted() {
        let mut sorted = DAFT_SUBCOMMANDS.to_vec();
        sorted.sort();
        assert_eq!(
            DAFT_SUBCOMMANDS,
            &sorted[..],
            "DAFT_SUBCOMMANDS should be in alphabetical order"
        );
    }

    #[test]
    fn test_daft_subcommands_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for cmd in DAFT_SUBCOMMANDS {
            assert!(seen.insert(cmd), "Duplicate subcommand: {cmd}");
        }
    }
}
