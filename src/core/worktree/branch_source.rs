//! Branch source resolution for the clone command.
//!
//! Provides `BranchSource` (the user's intent) and `BranchPlan` (the resolved
//! plan of which worktrees to create).

/// Unified branch selection for the clone command.
#[derive(Debug, Clone)]
pub enum BranchSource {
    /// No -b, no --all-branches: just the default branch.
    Default,
    /// Single -b <branch>: one explicit branch (today's behavior).
    Single(String),
    /// Multiple -b flags: explicit list of branches.
    Multiple(Vec<String>),
    /// --all-branches: discover all remote branches.
    All,
}

/// Resolved plan for which worktrees to create.
#[derive(Debug, Clone)]
pub struct BranchPlan {
    /// Branch for the base worktree (non-bare layouts only).
    pub base: Option<String>,
    /// Branches for satellite worktrees.
    pub satellites: Vec<String>,
    /// Which worktree to cd into after clone.
    pub cd_target: Option<String>,
    /// Branches that weren't found on remote.
    pub not_found: Vec<String>,
}

/// Replaces `HEAD`/`@` tokens with the actual default branch name and
/// deduplicates while preserving first-occurrence order.
pub fn expand_default_tokens(branches: &[String], default_branch: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for b in branches {
        let resolved = if b == "HEAD" || b == "@" {
            default_branch.to_string()
        } else {
            b.clone()
        };
        if seen.insert(resolved.clone()) {
            result.push(resolved);
        }
    }
    result
}

impl BranchSource {
    /// Maps from clap args to a `BranchSource`.
    pub fn from_args(branches: &[String], all_branches: bool) -> Self {
        if all_branches {
            return Self::All;
        }
        match branches.len() {
            0 => Self::Default,
            1 => Self::Single(branches[0].clone()),
            _ => Self::Multiple(branches.to_vec()),
        }
    }

    /// Resolves this source into a `BranchPlan`.
    pub fn resolve(
        &self,
        default_branch: &str,
        is_bare: bool,
        remote_branches: &[&str],
    ) -> BranchPlan {
        match self {
            BranchSource::Default => {
                if is_bare {
                    BranchPlan {
                        base: None,
                        satellites: vec![default_branch.to_string()],
                        cd_target: Some(default_branch.to_string()),
                        not_found: vec![],
                    }
                } else {
                    BranchPlan {
                        base: Some(default_branch.to_string()),
                        satellites: vec![],
                        cd_target: Some(default_branch.to_string()),
                        not_found: vec![],
                    }
                }
            }

            BranchSource::Single(branch) => {
                let not_found = if remote_branches.contains(&branch.as_str()) {
                    vec![]
                } else {
                    vec![branch.clone()]
                };
                if is_bare {
                    BranchPlan {
                        base: None,
                        satellites: vec![branch.clone()],
                        cd_target: Some(branch.clone()),
                        not_found,
                    }
                } else {
                    BranchPlan {
                        base: Some(branch.clone()),
                        satellites: vec![],
                        cd_target: Some(branch.clone()),
                        not_found,
                    }
                }
            }

            BranchSource::Multiple(list) => {
                let expanded = expand_default_tokens(list, default_branch);

                let mut valid: Vec<String> = vec![];
                let mut not_found: Vec<String> = vec![];
                for b in &expanded {
                    if remote_branches.contains(&b.as_str()) {
                        valid.push(b.clone());
                    } else {
                        not_found.push(b.clone());
                    }
                }

                if is_bare {
                    // cd_target = first valid branch from original order
                    let cd_target = expanded
                        .iter()
                        .find(|b| remote_branches.contains(&b.as_str()))
                        .cloned();
                    BranchPlan {
                        base: None,
                        satellites: valid,
                        cd_target,
                        not_found,
                    }
                } else {
                    // For non-bare with 2+ branches requested: inject default
                    // into base. "2+ branches requested" means the original
                    // expanded list has 2+ entries.
                    let base = if valid.contains(&default_branch.to_string()) {
                        Some(default_branch.to_string())
                    } else if expanded.len() >= 2 {
                        // Inject default branch as base when 2+ branches were
                        // requested, even if none of the requested branches
                        // are valid.
                        Some(default_branch.to_string())
                    } else {
                        // Single branch requested — use it or None
                        valid.first().cloned()
                    };

                    let satellites: Vec<String> = valid
                        .iter()
                        .filter(|b| Some(b.as_str()) != base.as_deref())
                        .cloned()
                        .collect();

                    // cd_target = first valid branch from original (expanded)
                    // order that isn't filtered out. Fall back to base.
                    let cd_target = expanded
                        .iter()
                        .find(|b| remote_branches.contains(&b.as_str()))
                        .cloned()
                        .or_else(|| base.clone());

                    BranchPlan {
                        base,
                        satellites,
                        cd_target,
                        not_found,
                    }
                }
            }

            BranchSource::All => {
                if remote_branches.is_empty() {
                    return BranchPlan {
                        base: None,
                        satellites: vec![],
                        cd_target: None,
                        not_found: vec![],
                    };
                }

                if is_bare {
                    BranchPlan {
                        base: None,
                        satellites: remote_branches.iter().map(|s| s.to_string()).collect(),
                        cd_target: None,
                        not_found: vec![],
                    }
                } else {
                    // base = default if present, else first alphabetical
                    let base = if remote_branches.contains(&default_branch) {
                        default_branch.to_string()
                    } else {
                        let mut sorted = remote_branches.to_vec();
                        sorted.sort_unstable();
                        sorted[0].to_string()
                    };

                    let satellites: Vec<String> = remote_branches
                        .iter()
                        .filter(|&&b| b != base.as_str())
                        .map(|s| s.to_string())
                        .collect();

                    BranchPlan {
                        cd_target: Some(base.clone()),
                        base: Some(base),
                        satellites,
                        not_found: vec![],
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_source_non_bare() {
        let plan = BranchSource::Default.resolve("main", false, &["main", "develop"]);
        assert_eq!(plan.base, Some("main".into()));
        assert!(plan.satellites.is_empty());
        assert_eq!(plan.cd_target, Some("main".into()));
        assert!(plan.not_found.is_empty());
    }

    #[test]
    fn single_source_non_bare() {
        let plan =
            BranchSource::Single("develop".into()).resolve("main", false, &["main", "develop"]);
        assert_eq!(plan.base, Some("develop".into()));
        assert!(plan.satellites.is_empty());
        assert_eq!(plan.cd_target, Some("develop".into()));
    }

    #[test]
    fn multiple_source_non_bare_injects_default() {
        let plan = BranchSource::Multiple(vec!["feat-a".into(), "feat-b".into()]).resolve(
            "main",
            false,
            &["main", "feat-a", "feat-b"],
        );
        assert_eq!(plan.base, Some("main".into()));
        assert_eq!(plan.satellites, vec!["feat-a", "feat-b"]);
        assert_eq!(plan.cd_target, Some("feat-a".into()));
    }

    #[test]
    fn multiple_source_non_bare_default_already_listed() {
        let plan = BranchSource::Multiple(vec!["main".into(), "feat-a".into()]).resolve(
            "main",
            false,
            &["main", "feat-a"],
        );
        assert_eq!(plan.base, Some("main".into()));
        assert_eq!(plan.satellites, vec!["feat-a"]);
        assert_eq!(plan.cd_target, Some("main".into()));
    }

    #[test]
    fn multiple_source_bare_no_injection() {
        let plan = BranchSource::Multiple(vec!["feat-a".into(), "feat-b".into()]).resolve(
            "main",
            true,
            &["main", "feat-a", "feat-b"],
        );
        assert_eq!(plan.base, None);
        assert_eq!(plan.satellites, vec!["feat-a", "feat-b"]);
        assert_eq!(plan.cd_target, Some("feat-a".into()));
    }

    #[test]
    fn head_and_at_tokens_resolved() {
        let expanded = expand_default_tokens(&["HEAD".into(), "feat-a".into(), "@".into()], "main");
        assert_eq!(expanded, vec!["main", "feat-a"]);
    }

    #[test]
    fn missing_branches_collected() {
        let plan = BranchSource::Multiple(vec!["feat-a".into(), "typo".into()]).resolve(
            "main",
            true,
            &["main", "feat-a"],
        );
        assert_eq!(plan.satellites, vec!["feat-a"]);
        assert_eq!(plan.not_found, vec!["typo"]);
    }

    #[test]
    fn cd_target_skips_missing_branches() {
        let plan = BranchSource::Multiple(vec!["typo".into(), "feat-a".into()]).resolve(
            "main",
            true,
            &["main", "feat-a"],
        );
        assert_eq!(plan.cd_target, Some("feat-a".into()));
    }

    #[test]
    fn all_source_non_bare() {
        let plan = BranchSource::All.resolve("main", false, &["main", "develop", "feat-a"]);
        assert_eq!(plan.base, Some("main".into()));
        assert_eq!(plan.satellites, vec!["develop", "feat-a"]);
        assert_eq!(plan.cd_target, Some("main".into()));
    }

    #[test]
    fn all_source_bare() {
        let plan = BranchSource::All.resolve("main", true, &["main", "develop", "feat-a"]);
        assert_eq!(plan.base, None);
        assert_eq!(plan.satellites, vec!["main", "develop", "feat-a"]);
    }

    #[test]
    fn multiple_all_invalid_non_bare_cd_falls_back_to_base() {
        let plan = BranchSource::Multiple(vec!["typo-a".into(), "typo-b".into()]).resolve(
            "main",
            false,
            &["main"],
        );
        assert_eq!(plan.base, Some("main".into()));
        assert_eq!(plan.cd_target, Some("main".into()));
        assert_eq!(plan.not_found, vec!["typo-a", "typo-b"]);
    }

    #[test]
    fn all_source_default_absent_non_bare_falls_back_to_first_alpha() {
        let plan = BranchSource::All.resolve("main", false, &["develop", "feat-a"]);
        assert_eq!(plan.base, Some("develop".into()));
        assert_eq!(plan.satellites, vec!["feat-a"]);
        assert_eq!(plan.cd_target, Some("develop".into()));
    }

    #[test]
    fn multiple_empty_remote_branches() {
        let plan = BranchSource::Multiple(vec!["feat-a".into()]).resolve("main", false, &[]);
        assert_eq!(plan.not_found, vec!["feat-a"]);
    }

    #[test]
    fn all_source_empty_remote_branches() {
        let plan = BranchSource::All.resolve("main", false, &[]);
        assert!(plan.satellites.is_empty());
        assert!(plan.base.is_none());
    }
}
