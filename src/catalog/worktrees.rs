//! Worktree enumeration for catalog rows — the shared source behind
//! `repo list --worktrees` and the `repo info` card, so the tree and any
//! count derived from it can never disagree between commands.

use std::path::{Path, PathBuf};

use crate::core::worktree::remove_repo::{RepoTarget, enumerate_worktrees};
use crate::store::CatalogRepoRow;

/// One enumerated worktree of a catalog repo, in raw form: canonical path
/// for structured payloads and current-worktree matching. Display copies
/// (relativized/tilde paths) are derived by the caller.
#[derive(Debug)]
pub struct WorktreeChild {
    pub branch: Option<String>,
    pub path: String,
    pub current: bool,
}

impl WorktreeChild {
    /// Human label for the branch column; detached HEADs have none.
    pub fn branch_label(&self) -> &str {
        self.branch.as_deref().unwrap_or("(detached)")
    }
}

/// Enumerate one catalog repo's worktrees, ordered for display. `None`
/// when the repo can't be opened (stale path, removed entry) — callers
/// show `-`/`null` rather than an empty tree.
pub fn worktree_children(
    row: &CatalogRepoRow,
    current_workdir: Option<&Path>,
) -> Option<Vec<WorktreeChild>> {
    // Synthetic target built from the recorded catalog paths, for
    // enumeration only — it skips `resolve_repo`'s canonicalize contract,
    // so don't hand it to the removal machinery.
    let target = RepoTarget {
        bare_git_dir: PathBuf::from(&row.git_common_dir),
        project_root: PathBuf::from(&row.path),
    };
    let mut children: Vec<WorktreeChild> = enumerate_worktrees(&target, true)
        .ok()?
        .into_iter()
        .map(|entry| WorktreeChild {
            current: current_workdir.is_some_and(|cur| cur == entry.path.as_path()),
            branch: entry.branch,
            path: entry.path.to_string_lossy().into_owned(),
        })
        .collect();
    sort_children(&mut children, row.default_branch.as_deref());
    Some(children)
}

/// Deterministic child order: the repo's default branch first, the rest by
/// branch name, detached worktrees last. (gix enumerates linked worktrees by
/// admin-dir name — the last path segment, so `feature/x` sorts as `x` — not
/// a stable user-facing order.)
pub fn sort_children(children: &mut [WorktreeChild], default_branch: Option<&str>) {
    children.sort_by(|a, b| {
        let rank = |c: &WorktreeChild| match c.branch.as_deref() {
            Some(branch) if Some(branch) == default_branch => 0,
            Some(_) => 1,
            None => 2,
        };
        (rank(a), a.branch.as_deref()).cmp(&(rank(b), b.branch.as_deref()))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn child(branch: Option<&str>) -> WorktreeChild {
        WorktreeChild {
            branch: branch.map(String::from),
            path: String::new(),
            current: false,
        }
    }

    #[test]
    fn sort_children_pins_default_branch_first_detached_last() {
        let mut children = vec![
            child(None),
            child(Some("zeta")),
            child(Some("main")),
            child(Some("alpha")),
        ];
        sort_children(&mut children, Some("main"));
        let order: Vec<Option<&str>> = children.iter().map(|c| c.branch.as_deref()).collect();
        assert_eq!(order, vec![Some("main"), Some("alpha"), Some("zeta"), None]);
    }

    #[test]
    fn detached_children_carry_a_label() {
        assert_eq!(child(None).branch_label(), "(detached)");
        assert_eq!(child(Some("main")).branch_label(), "main");
    }
}
