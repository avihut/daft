//! Default open-PR rows for `daft list`.
//!
//! In a forge-capable repo the list surfaces every open PR, not only the ones
//! a worktree already represents: a local branch with an open PR gets its
//! branch row shown without `--branches`, and a PR with no local presence at
//! all — a colleague's origin branch, any fork PR — gets a row synthesized
//! purely from the forge-PR cache. The rows ride the `pr` column's effective
//! visibility: `--columns -pr` (or the silent capability/health gate) removes
//! the column and the rows as one unit.
//!
//! Pure planning logic — no git, no store. The command layer supplies the
//! branch/ref universe and enriches what this module says to surface.

use std::collections::{HashMap, HashSet};

use super::forge_ref::{ForgeBranchRef, ForgePrLookup, OpenPr};
use super::list::{EntryKind, WorktreeInfo};
use crate::core::ownership::BranchOwner;

/// How each open PR not already represented by a visible row becomes one.
#[derive(Debug, Default)]
pub struct PrRowPlan {
    /// Local branches (without worktrees) to surface as ordinary branch rows
    /// because an open PR heads there — same-repo PRs matched by branch name,
    /// fork PRs matched by the branch's recorded tracking ref (a `pr:N`
    /// checkout whose worktree was since removed). The caller enriches these
    /// exactly like `--branches` rows. Empty when `--branches` already shows
    /// every local branch.
    pub surface_local: Vec<String>,
    /// Rows synthesized purely from cache data — open PRs with no local
    /// presence, including every fork PR not tracked by a local branch.
    pub synthesized: Vec<WorktreeInfo>,
}

/// Decide, for every open PR in the snapshot, whether it is already
/// represented by a visible row, should surface a local branch, or needs a
/// synthesized row.
///
/// - `worktree_branches` — branch names holding worktrees (always visible).
/// - `local_branches` — every local branch name.
/// - `branch_refs` — each branch's recorded PR/MR tracking ref
///   (`branch.<name>.merge`), for matching fork PRs, which never match by
///   name (a stranger's colliding branch name must not be conflated with a
///   local branch).
/// - `show_local` — `--branches` is active, so every local branch is already
///   a visible row.
pub fn plan_pr_rows(
    lookup: &ForgePrLookup,
    worktree_branches: &HashSet<String>,
    local_branches: &HashSet<String>,
    branch_refs: &HashMap<String, ForgeBranchRef>,
    show_local: bool,
) -> PrRowPlan {
    let mut visible_names: HashSet<&str> = worktree_branches.iter().map(String::as_str).collect();
    if show_local {
        visible_names.extend(local_branches.iter().map(String::as_str));
    }
    let mut visible_refs: HashSet<ForgeBranchRef> = branch_refs
        .iter()
        .filter(|(name, _)| visible_names.contains(name.as_str()))
        .map(|(_, r)| *r)
        .collect();
    // Reverse map for fork PRs: which (hidden) local branch tracks this ref?
    let ref_to_branch: HashMap<ForgeBranchRef, &str> = branch_refs
        .iter()
        .filter(|(name, _)| local_branches.contains(*name))
        .map(|(name, r)| (*r, name.as_str()))
        .collect();

    let mut plan = PrRowPlan::default();
    let mut surfaced: HashSet<&str> = HashSet::new();
    for p in &lookup.open {
        if visible_refs.contains(&p.decoration.r) {
            continue; // a visible row tracks this exact PR (`pr:N` checkout)
        }
        if p.is_cross_repo {
            // Fork PRs match by tracking ref only, never by branch name.
            match ref_to_branch.get(&p.decoration.r) {
                Some(branch) if !surfaced.contains(branch) => {
                    plan.surface_local.push((*branch).to_string());
                    surfaced.insert(branch);
                    visible_refs.insert(p.decoration.r);
                }
                Some(_) => {}
                None => plan.synthesized.push(synthesized_row(p)),
            }
            continue;
        }
        let head = p.head_branch.as_str();
        if visible_names.contains(head) || surfaced.contains(head) {
            continue; // decorates the existing row by name
        }
        if local_branches.contains(head) {
            plan.surface_local.push(head.to_string());
            surfaced.insert(head);
            if let Some(r) = branch_refs.get(head) {
                visible_refs.insert(*r);
            }
        } else {
            plan.synthesized.push(synthesized_row(p));
        }
    }
    plan
}

/// Build the display row for an open PR with no local presence. Everything it
/// shows comes from the cache: the branch cell (fork PRs render
/// `owner:branch` — per-fork namespaces must not read as local names), the
/// PR title standing in for the last-commit subject, the PR's last activity
/// for age, and the PR author as owner. `forge_ref` carries the PR identity
/// so `ForgePrLookup::decorate` resolves the cell through `by_ref` — the
/// display name is not a lookup key.
pub fn synthesized_row(p: &OpenPr) -> WorktreeInfo {
    let mut info = WorktreeInfo::empty(&p.display_branch());
    info.kind = EntryKind::ForgePr;
    info.forge_ref = Some(p.decoration.r);
    info.last_commit_timestamp = p.updated_at.map(|t| t.timestamp());
    info.last_commit_subject = p.title.clone();
    info.owner = pr_owner(p.decoration.author.as_deref());
    info
}

/// Wherever a PR decoration attaches to a row, prefer the forge's author for
/// the Owner cell over branch-history deduction — the forge's answer to
/// "whose PR is this" is canonical where the history walk is a heuristic.
/// Rows without a decoration (or a cache miss) keep their deduced owner.
pub fn apply_pr_owners(infos: &mut [WorktreeInfo], lookup: &ForgePrLookup) {
    for info in infos.iter_mut() {
        if let Some(author) = lookup
            .decorate(&info.name, info.forge_ref)
            .and_then(|d| d.author)
        {
            info.owner = pr_owner(Some(&author));
        }
    }
}

fn pr_owner(author: Option<&str>) -> Option<BranchOwner> {
    author.map(|name| BranchOwner {
        name: name.to_string(),
        email: String::new(),
        is_current_user: false,
    })
}

/// Under `--remotes`, a synthesized PR row subsumes its branch's remote row:
/// `feat-x  #7` and `origin/feat-x` are two spellings of the same branch, and
/// the PR row is the richer one. `synthesized` holds the PR rows' display
/// names — fork rows (`owner:branch`) never match a remote short name, which
/// is correct: the fork's branch is not `origin/<branch>`.
pub fn remote_row_subsumed(name: &str, kind: EntryKind, synthesized: &HashSet<String>) -> bool {
    kind == EntryKind::RemoteBranch
        && name
            .strip_prefix("origin/")
            .is_some_and(|short| synthesized.contains(short))
}

/// A concluded forge refresh delivered to the live table: the fresh lookup
/// (statuses become authoritative, cells re-derive) plus the row-set
/// reconcile computed against it — both land in the same repaint, during the
/// table's settle hold.
#[derive(Debug, Clone)]
pub struct ForgePrRowsRefresh {
    pub lookup: ForgePrLookup,
    /// Synthesized rows for open PRs the seed didn't know — new since the
    /// cached snapshot, or a cold cache's very first snapshot.
    pub add_rows: Vec<WorktreeInfo>,
    /// Names of PR-sourced rows whose PR is no longer open. Only rows that
    /// exist *because* of a PR are ever dropped — user-requested
    /// `--branches` rows are not in the seeded set and thus never named.
    pub drop_rows: Vec<String>,
}

/// Diff the fresh snapshot's row plan against the rows the seed created.
///
/// `seeded_pr_rows` holds the names of rows that exist because of a PR
/// (surfaced branches + synthesized rows). Additions are synthesized rows
/// only: a fresh PR on an existing local branch needs git enrichment the
/// live table can't do mid-run, so it surfaces on the next list instead.
pub fn reconcile_pr_rows(
    lookup: ForgePrLookup,
    worktree_branches: &HashSet<String>,
    local_branches: &HashSet<String>,
    branch_refs: &HashMap<String, ForgeBranchRef>,
    show_local: bool,
    seeded_pr_rows: &HashSet<String>,
) -> ForgePrRowsRefresh {
    let plan = plan_pr_rows(
        &lookup,
        worktree_branches,
        local_branches,
        branch_refs,
        show_local,
    );
    let mut fresh_names: HashSet<String> = plan.surface_local.into_iter().collect();
    let mut add_rows = Vec::new();
    for info in plan.synthesized {
        fresh_names.insert(info.name.clone());
        if !seeded_pr_rows.contains(&info.name) {
            add_rows.push(info);
        }
    }
    let drop_rows = seeded_pr_rows
        .iter()
        .filter(|n| !fresh_names.contains(*n))
        .cloned()
        .collect();
    ForgePrRowsRefresh {
        lookup,
        add_rows,
        drop_rows,
    }
}

/// Parse the output of `git config --get-regexp '^branch\..*\.merge$'` into
/// each branch's forge tracking ref. Non-forge merge refs (`refs/heads/...`)
/// drop out; branch names containing dots survive (the key is everything
/// between `branch.` and the final `.merge`).
pub fn parse_branch_forge_refs(config_lines: &str) -> HashMap<String, ForgeBranchRef> {
    config_lines
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(' ')?;
            let branch = key.strip_prefix("branch.")?.strip_suffix(".merge")?;
            let r = ForgeBranchRef::parse_merge_ref(value.trim())?;
            Some((branch.to_string(), r))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::worktree::forge_ref::{ForgeRefKind, PrDecoration, PrStatus};

    fn open_pr(number: u32, head: &str, fork: bool) -> OpenPr {
        let r = ForgeBranchRef::new(ForgeRefKind::GithubPr, number);
        OpenPr {
            decoration: PrDecoration {
                status: Some(PrStatus::Open),
                author: Some(format!("author{number}")),
                ..PrDecoration::bare(r)
            },
            head_branch: head.to_string(),
            is_cross_repo: fork,
            head_repo_owner: if fork {
                format!("owner{number}")
            } else {
                String::new()
            },
            title: format!("feat: change {number}"),
            updated_at: None,
        }
    }

    fn lookup_with(open: Vec<OpenPr>) -> ForgePrLookup {
        ForgePrLookup {
            open,
            ..ForgePrLookup::default()
        }
    }

    fn set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn worktree_and_shown_branches_absorb_same_repo_prs() {
        let lookup = lookup_with(vec![
            open_pr(7, "in-worktree", false),
            open_pr(6, "local-only", false),
            open_pr(5, "nowhere", false),
        ]);
        let plan = plan_pr_rows(
            &lookup,
            &set(&["in-worktree"]),
            &set(&["in-worktree", "local-only"]),
            &HashMap::new(),
            false,
        );
        assert_eq!(plan.surface_local, vec!["local-only"]);
        let names: Vec<&str> = plan.synthesized.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["nowhere"]);
        assert_eq!(plan.synthesized[0].kind, EntryKind::ForgePr);
    }

    #[test]
    fn branches_flag_absorbs_local_branches_without_surfacing() {
        let lookup = lookup_with(vec![open_pr(6, "local-only", false)]);
        let plan = plan_pr_rows(
            &lookup,
            &set(&[]),
            &set(&["local-only"]),
            &HashMap::new(),
            true,
        );
        assert!(plan.surface_local.is_empty(), "-b already shows the row");
        assert!(plan.synthesized.is_empty());
    }

    #[test]
    fn fork_prs_never_match_local_branches_by_name() {
        // alice's fork PR from her `main` must not be absorbed by (or
        // surface) your local `main` — it gets its own owner-prefixed row.
        let lookup = lookup_with(vec![open_pr(9, "main", true)]);
        let plan = plan_pr_rows(
            &lookup,
            &set(&["main"]),
            &set(&["main"]),
            &HashMap::new(),
            false,
        );
        assert!(plan.surface_local.is_empty());
        assert_eq!(plan.synthesized[0].name, "owner9:main");
    }

    #[test]
    fn fork_prs_match_by_tracking_ref() {
        // A `pr:N` checkout records the fork ref in branch config. With the
        // worktree present the row absorbs the PR; with only the branch left
        // the branch is surfaced instead of synthesizing a duplicate.
        let lookup = lookup_with(vec![open_pr(9, "patch-1", true)]);
        let refs = HashMap::from([(
            "patch-1".to_string(),
            ForgeBranchRef::new(ForgeRefKind::GithubPr, 9),
        )]);

        let absorbed = plan_pr_rows(
            &lookup,
            &set(&["patch-1"]),
            &set(&["patch-1"]),
            &refs,
            false,
        );
        assert!(absorbed.surface_local.is_empty());
        assert!(absorbed.synthesized.is_empty());

        let surfaced = plan_pr_rows(&lookup, &set(&[]), &set(&["patch-1"]), &refs, false);
        assert_eq!(surfaced.surface_local, vec!["patch-1"]);
        assert!(surfaced.synthesized.is_empty());
    }

    #[test]
    fn two_forks_from_the_same_branch_name_both_synthesize() {
        let lookup = lookup_with(vec![
            open_pr(9, "patch-1", true),
            open_pr(8, "patch-1", true),
        ]);
        let plan = plan_pr_rows(&lookup, &set(&[]), &set(&[]), &HashMap::new(), false);
        let names: Vec<&str> = plan.synthesized.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["owner9:patch-1", "owner8:patch-1"]);
    }

    #[test]
    fn synthesized_row_maps_cache_fields_into_cells() {
        let mut p = open_pr(9, "feat/x", false);
        p.updated_at = Some(
            chrono::DateTime::parse_from_rfc3339("2026-07-18T09:30:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        );
        let info = synthesized_row(&p);
        assert_eq!(info.kind, EntryKind::ForgePr);
        assert_eq!(
            info.forge_ref,
            Some(ForgeBranchRef::new(ForgeRefKind::GithubPr, 9))
        );
        assert_eq!(info.last_commit_subject, "feat: change 9");
        assert_eq!(info.owner.as_ref().unwrap().name, "author9");
        assert!(info.last_commit_timestamp.is_some());
        assert!(info.path.is_none());
    }

    #[test]
    fn apply_pr_owners_prefers_the_forge_author_and_keeps_deduced_otherwise() {
        let mut lookup = lookup_with(vec![]);
        let r = ForgeBranchRef::new(ForgeRefKind::GithubPr, 7);
        lookup.by_branch.insert(
            "feat/x".into(),
            PrDecoration {
                author: Some("alice".into()),
                ..PrDecoration::bare(r)
            },
        );

        let deduced = BranchOwner {
            name: "History Name".into(),
            email: "h@x".into(),
            is_current_user: true,
        };
        let mut with_pr = WorktreeInfo::empty("feat/x");
        with_pr.owner = Some(deduced.clone());
        let mut without_pr = WorktreeInfo::empty("feat/other");
        without_pr.owner = Some(deduced.clone());
        let mut infos = vec![with_pr, without_pr];

        apply_pr_owners(&mut infos, &lookup);
        assert_eq!(infos[0].owner.as_ref().unwrap().name, "alice");
        assert_eq!(infos[1].owner.as_ref().unwrap().name, "History Name");
    }

    #[test]
    fn reconcile_adds_new_and_drops_closed_pr_rows_only() {
        // Seeded this run: synthesized rows for PRs 9 (fork) and 5. Fresh
        // snapshot: 9 still open, 5 closed, 11 brand new.
        let lookup = lookup_with(vec![
            open_pr(11, "new-work", false),
            open_pr(9, "patch-1", true),
        ]);
        let seeded: HashSet<String> = set(&["owner9:patch-1", "old-branch"]);
        let refresh = reconcile_pr_rows(
            lookup,
            &set(&["main"]),
            &set(&[]),
            &HashMap::new(),
            false,
            &seeded,
        );
        let added: Vec<&str> = refresh.add_rows.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(added, vec!["new-work"]);
        assert_eq!(refresh.drop_rows, vec!["old-branch".to_string()]);
    }

    #[test]
    fn reconcile_cold_cache_first_snapshot_adds_everything_foreign() {
        // Cold cache: nothing seeded. The first snapshot's synthesized rows
        // all insert; a PR heading an existing local branch is NOT inserted
        // mid-run (it needs git enrichment — it surfaces on the next list).
        let lookup = lookup_with(vec![
            open_pr(9, "patch-1", true),
            open_pr(6, "local-only", false),
        ]);
        let refresh = reconcile_pr_rows(
            lookup,
            &set(&["main"]),
            &set(&["local-only"]),
            &HashMap::new(),
            false,
            &HashSet::new(),
        );
        let added: Vec<&str> = refresh.add_rows.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(added, vec!["owner9:patch-1"]);
        assert!(refresh.drop_rows.is_empty());
    }

    #[test]
    fn reconcile_never_names_rows_it_did_not_seed() {
        // A `--branches` row for a branch whose PR just closed is a user-
        // requested row, not a PR-sourced one — it is not in the seeded set
        // and must survive the reconcile untouched.
        let refresh = reconcile_pr_rows(
            lookup_with(vec![]),
            &set(&["main"]),
            &set(&["some-branch"]),
            &HashMap::new(),
            true,
            &HashSet::new(),
        );
        assert!(refresh.add_rows.is_empty());
        assert!(refresh.drop_rows.is_empty());
    }

    #[test]
    fn synthesized_rows_subsume_their_remote_row_under_dash_r() {
        let synthesized = set(&["feat-x", "owner9:patch-1"]);
        assert!(remote_row_subsumed(
            "origin/feat-x",
            EntryKind::RemoteBranch,
            &synthesized
        ));
        assert!(
            !remote_row_subsumed("origin/other", EntryKind::RemoteBranch, &synthesized),
            "unrelated remote rows stay"
        );
        assert!(
            !remote_row_subsumed("origin/patch-1", EntryKind::RemoteBranch, &synthesized),
            "a fork row's branch is not origin's — the remote row is a different branch"
        );
        assert!(
            !remote_row_subsumed("feat-x", EntryKind::LocalBranch, &synthesized),
            "only remote rows are ever subsumed"
        );
    }

    #[test]
    fn parses_branch_forge_refs_including_dotted_names() {
        let out = "branch.main.merge refs/heads/main\n\
                   branch.pr.test.merge refs/pull/7/head\n\
                   branch.mr-branch.merge refs/merge-requests/45/head\n";
        let refs = parse_branch_forge_refs(out);
        assert_eq!(refs.len(), 2, "plain refs/heads entries drop out");
        assert_eq!(
            refs["pr.test"],
            ForgeBranchRef::new(ForgeRefKind::GithubPr, 7)
        );
        assert_eq!(
            refs["mr-branch"],
            ForgeBranchRef::new(ForgeRefKind::GitlabMr, 45)
        );
    }
}
