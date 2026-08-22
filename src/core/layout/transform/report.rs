//! The words `daft layout transform` says before it does anything: the plan
//! line, and the refusal that names every blocker at once.
//!
//! Everything here is plain text — no ANSI, ever. The refusal travels through
//! `anyhow` into `Error: …`, whose rendering daft does not control, and the
//! plan line is a one-line summary whose every accent would have to answer
//! "what state is this signalling?" — the answer is none. Scenarios assert on
//! these strings verbatim.

use std::path::Path;

use super::plan::{TransformOp, TransformPlan};
use super::preflight::{Blocker, BlockerKind, ProbeReason};
use super::state::{ClassifiedWorktree, LayoutState, WorktreeDisposition};
use crate::core::copy_paths::format_bytes;
use crate::core::worktree::list::ChangedFiles;
use crate::git::op_state::OpKind;
use crate::output::format::display_path;

// ── Tree summaries ───────────────────────────────────────────────────────

/// "5 modified, 2 staged, 1 untracked, 1 conflicted" — `None` when clean.
///
/// `modified` is what `git status` calls an unstaged change (`staged` is
/// reported separately). Zero counts are omitted; all four words are
/// adjectives, so nothing pluralizes.
pub fn tree_summary(c: &ChangedFiles) -> Option<String> {
    let parts: Vec<String> = [
        (c.unstaged, "modified"),
        (c.staged, "staged"),
        (c.untracked, "untracked"),
        (c.conflicted, "conflicted"),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, word)| format!("{n} {word}"))
    .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

/// [`tree_summary`], with "clean" instead of `None` — for picker rows.
pub fn tree_summary_or_clean(c: &ChangedFiles) -> String {
    tree_summary(c).unwrap_or_else(|| "clean".to_string())
}

// ── Plan line ────────────────────────────────────────────────────────────

fn short_oid(oid: &str) -> &str {
    &oid[..oid.len().min(7)]
}

/// A path as the plan line shows it: relative to the project root with a
/// trailing `/` when inside it, the `~`/cwd-relative form otherwise.
fn place(path: &Path, project_root: &Path, cwd: Option<&Path>) -> String {
    match path.strip_prefix(project_root) {
        Ok(rel) if !rel.as_os_str().is_empty() => format!("{}/", rel.display()),
        _ => display_path(&path.to_string_lossy(), cwd),
    }
}

/// The one line printed before anything happens — what moves, what it
/// carries, and what the default branch is left with.
///
/// Shape: `<what moves> · <what it carries> · [<copy fact> ·] <default branch>`
/// — each segment only when it says something.
pub fn plan_line(
    source: &LayoutState,
    classified: &[ClassifiedWorktree],
    plan: &TransformPlan,
    layout_name: &str,
    copy_bytes: Option<u64>,
    cwd: Option<&Path>,
) -> String {
    if plan.ops.len() == 1 {
        return format!("Already in the '{layout_name}' layout; nothing to move.");
    }

    let root = classified
        .iter()
        .find(|cw| cw.disposition == WorktreeDisposition::Root);
    let root_moves =
        root.is_some_and(|cw| !super::plan::paths_equivalent(&cw.current_path, &cw.target_path));
    let carried_for = |cw: &ClassifiedWorktree| {
        plan.carried
            .iter()
            .find(|c| c.from == cw.current_path || c.from == canonical(&cw.current_path))
            .map(|c| c.counts.clone())
    };

    let mut segments: Vec<String> = Vec::new();

    if let (Some(cw), true) = (root, root_moves) {
        let dest = place(&cw.target_path, &source.project_root, cwd);
        let head = source
            .worktrees
            .iter()
            .find(|wt| wt.is_root)
            .and_then(|wt| wt.head.as_deref());
        if source.is_bare {
            // A bare source has no main working tree yet: its pivot *becomes*
            // one. Kept verbatim — scenarios assert it.
            segments.push(format!(
                "'{}' becomes the main working tree at {}",
                cw.label(),
                display_path(&cw.target_path.to_string_lossy(), cwd)
            ));
        } else {
            match &cw.branch {
                Some(b) => segments.push(format!("main working tree on '{b}' → {dest}")),
                None => segments.push(format!(
                    "main working tree (detached at {}) → {dest}",
                    head.map(short_oid).unwrap_or("HEAD")
                )),
            }
        }
        if let Some(summary) = carried_for(cw).as_ref().and_then(tree_summary) {
            segments.push(format!("{summary} carried along"));
        }
    } else {
        let moves = plan
            .ops
            .iter()
            .filter(|op| matches!(op, TransformOp::MoveWorktree { .. }))
            .count();
        segments.push(format!(
            "{moves} worktree{} relocated",
            if moves == 1 { "" } else { "s" }
        ));
        let dirty = classified
            .iter()
            .filter(|cw| cw.disposition == WorktreeDisposition::Conforming)
            .filter(|cw| !super::plan::paths_equivalent(&cw.current_path, &cw.target_path))
            .filter(|cw| carried_for(cw).is_some_and(|c| tree_summary(&c).is_some()))
            .count();
        if dirty > 0 {
            segments.push(format!(
                "{dirty} carr{} uncommitted work",
                if dirty == 1 { "ies" } else { "y" }
            ));
        }
    }

    if plan.copies_across_volumes() {
        match copy_bytes {
            Some(bytes) => segments.push(format!("copied across volumes, {}", format_bytes(bytes))),
            None => segments.push("copied across volumes".to_string()),
        }
    }

    // The #873 fact: the root role follows the main working tree, so when it
    // is on a branch other than the default, say what the default is left with.
    if let Some(cw) = root
        && cw.branch.as_deref() != Some(source.default_branch.as_str())
    {
        let default_has_worktree = source
            .worktrees
            .iter()
            .any(|wt| wt.branch.as_deref() == Some(source.default_branch.as_str()));
        if default_has_worktree {
            segments.push(format!(
                "'{}' keeps its own worktree",
                source.default_branch
            ));
        } else {
            segments.push(format!("'{}': no worktree", source.default_branch));
        }
    }

    segments.join(" · ")
}

fn canonical(path: &Path) -> std::path::PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

// ── Blocker report ───────────────────────────────────────────────────────

fn op_noun(op: OpKind) -> &'static str {
    match op {
        OpKind::Rebase => "rebase",
        OpKind::Am => "patch apply (git am)",
        OpKind::Merge => "merge",
        OpKind::CherryPick => "cherry-pick",
        OpKind::Revert => "revert",
        OpKind::Bisect => "bisect",
    }
}

/// The two git subcommands that settle an operation, in both directions.
fn settle(op: OpKind) -> (&'static str, &'static str) {
    match op {
        OpKind::Rebase => ("rebase --continue", "rebase --abort"),
        OpKind::Am => ("am --continue", "am --abort"),
        OpKind::Merge => ("merge --continue", "merge --abort"),
        OpKind::CherryPick => ("cherry-pick --continue", "cherry-pick --abort"),
        OpKind::Revert => ("revert --continue", "revert --abort"),
        OpKind::Bisect => ("bisect good   (or: bisect bad)", "bisect reset"),
    }
}

/// The refusal: every blocker, each with where it is, why it blocks, and the
/// exact commands that settle it — then one retry line.
pub fn render_blockers(
    blockers: &[Blocker],
    layout_name: &str,
    source: &LayoutState,
    cwd: Option<&Path>,
) -> String {
    let n = blockers.len();
    let mut out = format!(
        "Cannot transform to '{layout_name}' — {n} condition{} to settle first:\n",
        if n == 1 { "" } else { "s" }
    );
    let retry = crate::daft_cmd(&format!("layout transform {layout_name}"));
    let show = |p: &Path| display_path(&p.to_string_lossy(), cwd);

    for b in blockers {
        out.push('\n');
        let wt = b.worktree_path.as_deref().map(show);
        let git_in = |args: &str| match &wt {
            Some(w) => format!("git -C {w} {args}"),
            None => format!("git {args}"),
        };
        match &b.kind {
            BlockerKind::OperationInProgress {
                op,
                progress,
                marker,
            } => {
                let where_ = b
                    .git_dir
                    .as_deref()
                    .map(|g| format!("{}/{marker}", show(g)))
                    .unwrap_or_else(|| format!(".git/{marker}"));
                let progress = match progress {
                    Some(p) if p.total > 0 && p.done > 0 => {
                        format!(", {} of {} applied", p.done, p.total)
                    }
                    Some(p) if p.total > 0 => format!(", {} left to apply", p.total),
                    _ => String::new(),
                };
                out.push_str(&format!(
                    "  {} has a {} in progress — {where_}{progress}.\n",
                    wt.as_deref().unwrap_or("this worktree"),
                    op_noun(*op)
                ));
                out.push_str(&format!(
                    "  The transform moves this worktree's git state; a paused {} cannot be\n  carried across that move.\n",
                    op_noun(*op)
                ));
                let (cont, abort) = settle(*op);
                out.push_str(&format!("      continue:  {}\n", git_in(cont)));
                out.push_str(&format!("      abort:     {}\n", git_in(abort)));
            }
            BlockerKind::IndexLocked { lock } => {
                out.push_str(&format!(
                    "  {} has {} — another git process is writing there.\n",
                    wt.as_deref().unwrap_or("this worktree"),
                    show(lock)
                ));
                out.push_str(
                    "  The transform moves that index; taking it out from under a live git\n  process would corrupt both.\n",
                );
                out.push_str("      wait:    let the other git finish\n");
                out.push_str(&format!(
                    "      or:      rm {}   (only if no git is running there)\n",
                    show(lock)
                ));
            }
            BlockerKind::Submodules { paths } => {
                let names: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
                match b.reason {
                    Some(ProbeReason::Relocation) => out.push_str(&format!(
                        "  {} contains submodules ({}), and git will not move a working tree\n  that contains submodules.\n",
                        wt.as_deref().unwrap_or("this worktree"),
                        names.join(", ")
                    )),
                    _ => out.push_str(&format!(
                        "  {} has {} checked-out submodule{} ({}). Their .git pointers are\n  relative to a git dir the move invalidates.\n",
                        wt.as_deref().unwrap_or("this worktree"),
                        paths.len(),
                        if paths.len() == 1 { "" } else { "s" },
                        names.join(", ")
                    )),
                }
                out.push_str(&format!(
                    "      deinit:  {}\n",
                    git_in("submodule deinit --all")
                ));
                out.push_str(
                    "      after:   git submodule update --init   (from the worktree's new path)\n",
                );
            }
            BlockerKind::RegistrationLocked { reason } => {
                let because = reason
                    .as_deref()
                    .map(|r| format!(" (\"{r}\")"))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "  {} is locked{because} and the transform moves it; git worktree move refuses a\n  locked worktree.\n",
                    wt.as_deref().unwrap_or("this worktree")
                ));
                out.push_str(&format!(
                    "      unlock:  {}\n",
                    git_in(&format!("worktree unlock {}", wt.as_deref().unwrap_or(".")))
                ));
            }
            BlockerKind::MissingPivot { candidates } => {
                out.push_str(&format!(
                    "  The default branch '{}' has no worktree, so the '{layout_name}' layout needs one of\n  these worktrees at {}. Which one is a choice daft will not make for you.\n",
                    source.default_branch,
                    show(&source.project_root)
                ));
                let bw = candidates.iter().map(|c| c.branch.len()).max().unwrap_or(0);
                let paths: Vec<String> = candidates.iter().map(|c| show(&c.path)).collect();
                let pw = paths.iter().map(String::len).max().unwrap_or(0);
                for (c, p) in candidates.iter().zip(paths.iter()) {
                    out.push_str(&format!(
                        "      {:<bw$}  {:<pw$}  {}\n",
                        c.branch,
                        p,
                        tree_summary_or_clean(&c.counts)
                    ));
                }
                out.push_str(&format!("      name one:  {retry} --pivot <branch>\n"));
            }
            BlockerKind::MissingAs { derived, commit } => {
                out.push_str(&format!(
                    "  The main working tree at {} is detached at {}, so the '{layout_name}'\n  layout has no branch to name its directory.\n",
                    show(&source.project_root),
                    short_oid(commit)
                ));
                out.push_str(&format!("      name it:  {retry} --as <dir>\n"));
                out.push_str(&format!(
                    "      or:       {retry} -y   (accepts '{derived}')\n"
                ));
            }
            BlockerKind::NeedsCopyConfirm { bytes, moves } => {
                let dest = moves
                    .first()
                    .and_then(|(_, to)| to.parent())
                    .map(show)
                    .unwrap_or_else(|| "the destination".to_string());
                out.push_str(&format!(
                    "  {dest} is a different volume from {}, so {} worktree{} ({}) would be copied,\n  not renamed — interruptible, and the source is kept until the copy is verified.\n",
                    show(&source.project_root),
                    moves.len(),
                    if moves.len() == 1 { "" } else { "s" },
                    format_bytes(*bytes)
                ));
                out.push_str(&format!("      accept:  {retry} -y\n"));
            }
        }
    }

    if blockers.iter().any(Blocker::is_settle_first) {
        out.push_str(&format!("\n  then: {retry}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::preflight::{OpProgress, PivotCandidate};
    use super::super::state::WorktreeEntry;
    use super::*;
    use std::path::PathBuf;

    fn counts(unstaged: usize, staged: usize, untracked: usize, conflicted: usize) -> ChangedFiles {
        ChangedFiles {
            staged,
            unstaged,
            untracked,
            conflicted,
            paths: vec![],
        }
    }

    #[test]
    fn tree_summary_words_and_order() {
        assert_eq!(tree_summary(&counts(0, 0, 0, 0)), None);
        assert_eq!(tree_summary_or_clean(&counts(0, 0, 0, 0)), "clean");
        assert_eq!(
            tree_summary(&counts(5, 0, 1, 0)).as_deref(),
            Some("5 modified, 1 untracked")
        );
        assert_eq!(
            tree_summary(&counts(3, 2, 1, 1)).as_deref(),
            Some("3 modified, 2 staged, 1 untracked, 1 conflicted")
        );
        assert_eq!(
            tree_summary(&counts(1, 0, 0, 0)).as_deref(),
            Some("1 modified")
        );
    }

    fn state(
        is_bare: bool,
        default_branch: &str,
        worktrees: Vec<(Option<&str>, &str, bool)>,
    ) -> LayoutState {
        LayoutState {
            git_dir: PathBuf::from("/repo/.git"),
            is_bare,
            default_branch: default_branch.into(),
            project_root: PathBuf::from("/repo"),
            worktrees: worktrees
                .into_iter()
                .map(|(b, p, r)| WorktreeEntry {
                    branch: b.map(str::to_string),
                    head: Some("779c1ab3f8e2aaaa".into()),
                    path: PathBuf::from(p),
                    is_root: r,
                })
                .collect(),
        }
    }

    fn classified(branch: Option<&str>, from: &str, to: &str, root: bool) -> ClassifiedWorktree {
        ClassifiedWorktree {
            branch: branch.map(str::to_string),
            current_path: PathBuf::from(from),
            target_path: PathBuf::from(to),
            disposition: if root {
                WorktreeDisposition::Root
            } else {
                WorktreeDisposition::Conforming
            },
        }
    }

    fn plan_with(ops: Vec<TransformOp>, carried: Vec<(&str, ChangedFiles)>) -> TransformPlan {
        TransformPlan {
            ops,
            skipped: vec![],
            description: String::new(),
            carried: carried
                .into_iter()
                .map(|(from, c)| super::super::plan::CarriedState {
                    branch: None,
                    from: PathBuf::from(from),
                    to: PathBuf::from("/x"),
                    counts: c,
                })
                .collect(),
        }
    }

    #[test]
    fn plan_line_nest_with_dirt_and_no_default_worktree() {
        let source = state(
            false,
            "master",
            vec![(Some("task/local-docker"), "/repo", true)],
        );
        let cw = vec![classified(
            Some("task/local-docker"),
            "/repo",
            "/repo/task/local-docker",
            true,
        )];
        let plan = plan_with(
            vec![
                TransformOp::NestFromRoot {
                    branch: Some("task/local-docker".into()),
                    root_path: "/repo".into(),
                    subdir_path: "/repo/task/local-docker".into(),
                },
                TransformOp::ValidateIntegrity,
            ],
            vec![("/repo", counts(5, 0, 1, 0))],
        );
        assert_eq!(
            plan_line(&source, &cw, &plan, "contained", None, None),
            "main working tree on 'task/local-docker' → task/local-docker/ · 5 modified, 1 untracked carried along · 'master': no worktree"
        );
    }

    #[test]
    fn plan_line_collapse_keeps_the_becomes_phrase_and_omits_a_clean_middle() {
        let source = state(
            true,
            "main",
            vec![(Some("develop"), "/repo/develop", false)],
        );
        let cw = vec![classified(Some("develop"), "/repo/develop", "/repo", true)];
        let plan = plan_with(
            vec![
                TransformOp::CollapseIntoRoot {
                    branch: Some("develop".into()),
                    worktree_path: "/repo/develop".into(),
                    root_path: "/repo".into(),
                },
                TransformOp::ValidateIntegrity,
            ],
            vec![("/repo/develop", counts(0, 0, 0, 0))],
        );
        let line = plan_line(&source, &cw, &plan, "sibling", None, None);
        assert!(
            line.starts_with("'develop' becomes the main working tree at "),
            "{line}"
        );
        assert!(line.ends_with(" · 'main': no worktree"), "{line}");
        assert!(!line.contains("carried"), "{line}");
    }

    #[test]
    fn plan_line_moves_only_and_noop() {
        let source = state(
            false,
            "main",
            vec![(Some("main"), "/repo", true), (Some("a"), "/repo.a", false)],
        );
        let cw = vec![
            classified(Some("main"), "/repo", "/repo", true),
            classified(Some("a"), "/repo.a", "/repo/.worktrees/a", false),
        ];
        let plan = plan_with(
            vec![
                TransformOp::MoveWorktree {
                    branch: Some("a".into()),
                    from: "/repo.a".into(),
                    to: "/repo/.worktrees/a".into(),
                    strategy: crate::core::fs_volume::MoveStrategy::Rename,
                },
                TransformOp::ValidateIntegrity,
            ],
            vec![("/repo.a", counts(1, 0, 0, 0))],
        );
        assert_eq!(
            plan_line(&source, &cw, &plan, "nested", None, None),
            "1 worktree relocated · 1 carries uncommitted work"
        );
        let noop = plan_with(vec![TransformOp::ValidateIntegrity], vec![]);
        assert_eq!(
            plan_line(&source, &cw, &noop, "sibling", None, None),
            "Already in the 'sibling' layout; nothing to move."
        );
    }

    #[test]
    fn plan_line_detached_main_and_copy_segment() {
        let source = state(false, "main", vec![(None, "/repo", true)]);
        let cw = vec![classified(None, "/repo", "/repo/sandbox", true)];
        let plan = plan_with(
            vec![
                TransformOp::NestFromRoot {
                    branch: None,
                    root_path: "/repo".into(),
                    subdir_path: "/repo/sandbox".into(),
                },
                TransformOp::MoveWorktree {
                    branch: Some("a".into()),
                    from: "/repo.a".into(),
                    to: "/vol/a".into(),
                    strategy: crate::core::fs_volume::MoveStrategy::CopyThenRemove,
                },
                TransformOp::ValidateIntegrity,
            ],
            vec![],
        );
        assert_eq!(
            plan_line(&source, &cw, &plan, "contained", Some(4_617_089_843), None),
            "main working tree (detached at 779c1ab) → sandbox/ · copied across volumes, 4.3 GB · 'main': no worktree"
        );
    }

    fn blocker(kind: BlockerKind, wt: &str, git_dir: &str, reason: ProbeReason) -> Blocker {
        Blocker {
            kind,
            worktree_path: Some(PathBuf::from(wt)),
            git_dir: Some(PathBuf::from(git_dir)),
            branch: None,
            reason: Some(reason),
        }
    }

    #[test]
    fn two_blockers_render_in_one_report_with_one_retry_line() {
        let source = state(true, "main", vec![]);
        let blockers = vec![
            blocker(
                BlockerKind::OperationInProgress {
                    op: OpKind::Rebase,
                    progress: Some(OpProgress { done: 3, total: 7 }),
                    marker: "rebase-merge",
                },
                "/proj/api",
                "/proj/api/.git",
                ProbeReason::RoleChange,
            ),
            blocker(
                BlockerKind::RegistrationLocked {
                    reason: Some("running a long build".into()),
                },
                "/proj/web",
                "/proj/.git/worktrees/web",
                ProbeReason::Relocation,
            ),
        ];
        let text = render_blockers(&blockers, "contained", &source, None);
        assert!(
            text.starts_with("Cannot transform to 'contained' — 2 conditions to settle first:"),
            "{text}"
        );
        assert!(
            text.contains(
                "/proj/api has a rebase in progress — /proj/api/.git/rebase-merge, 3 of 7 applied."
            ),
            "{text}"
        );
        assert!(
            text.contains("continue:  git -C /proj/api rebase --continue"),
            "{text}"
        );
        assert!(
            text.contains("abort:     git -C /proj/api rebase --abort"),
            "{text}"
        );
        assert!(
            text.contains("/proj/web is locked (\"running a long build\")"),
            "{text}"
        );
        assert!(
            text.contains("git -C /proj/web worktree unlock /proj/web"),
            "{text}"
        );
        assert_eq!(text.matches("then: ").count(), 1, "{text}");
        assert!(text.contains("layout transform contained\n"), "{text}");
        assert!(!text.contains('\x1b'));
    }

    #[test]
    fn flag_shaped_blockers_have_no_then_line() {
        let source = state(true, "main", vec![]);
        let blockers = vec![Blocker::repo_wide(BlockerKind::MissingPivot {
            candidates: vec![
                PivotCandidate {
                    branch: "develop".into(),
                    path: "/repo/develop".into(),
                    counts: counts(5, 0, 1, 0),
                },
                PivotCandidate {
                    branch: "task/local-docker".into(),
                    path: "/repo/task/local-docker".into(),
                    counts: counts(0, 0, 0, 0),
                },
            ],
        })];
        let text = render_blockers(&blockers, "sibling", &source, None);
        assert!(text.contains("1 condition to settle first"), "{text}");
        assert!(text.contains("'main' has no worktree"), "{text}");
        assert!(text.contains("--pivot <branch>"), "{text}");
        assert!(text.contains("5 modified, 1 untracked"), "{text}");
        assert!(text.contains("clean"), "{text}");
        assert!(!text.contains("then:"), "{text}");

        let source = state(false, "main", vec![(None, "/repo", true)]);
        let text = render_blockers(
            &[Blocker::repo_wide(BlockerKind::MissingAs {
                derived: "779c1ab3f8e2".into(),
                commit: "779c1ab3f8e2aaaa".into(),
            })],
            "contained",
            &source,
            None,
        );
        assert!(text.contains("is detached at 779c1ab"), "{text}");
        assert!(
            text.contains("has no branch to name its directory"),
            "{text}"
        );
        assert!(text.contains("--as <dir>"), "{text}");
        assert!(text.contains("-y   (accepts '779c1ab3f8e2')"), "{text}");
    }

    #[test]
    fn copy_confirm_blocker_names_the_volume_and_minus_y() {
        let source = state(false, "main", vec![]);
        let text = render_blockers(
            &[Blocker::repo_wide(BlockerKind::NeedsCopyConfirm {
                bytes: 4_617_089_843,
                moves: vec![("/repo.a".into(), "/Volumes/X/worktrees/repo/a".into())],
            })],
            "centralized",
            &source,
            None,
        );
        assert!(text.contains("is a different volume from"), "{text}");
        assert!(text.contains("copied,\n  not renamed"), "{text}");
        assert!(text.contains("4.3 GB"), "{text}");
        assert!(text.contains("transform centralized -y"), "{text}");
    }
}
