//! Rendering a move plan for `--dry-run`.
//!
//! The plan printed here is the same value the executor consumes, so what the
//! user reads is what would happen — not a second description of it that can
//! drift. Mirrors `repo remove`'s dry-run shape: one indented line per effect,
//! verbs in the left column.

use super::plan::{Disposition, MovePlan, NamePlan};

pub fn print_plan(plan: &MovePlan) {
    println!("Would move:");
    println!(
        "  repo      {}  ->  {}",
        plan.old_root.display(),
        plan.new_root.display()
    );

    for worktree in plan.relocated() {
        println!(
            "  worktree  {}  ->  {}{}",
            worktree.old_path.display(),
            worktree.new_path.display(),
            branch_suffix(worktree.branch.as_deref())
        );
    }
    for worktree in plan.left_in_place() {
        println!(
            "  keep      {}  (stays put; its git linkage is repaired){}",
            worktree.old_path.display(),
            branch_suffix(worktree.branch.as_deref())
        );
    }

    let carried = plan
        .worktrees
        .iter()
        .filter(|w| w.disposition == Disposition::CarriedWithRoot)
        .count();
    if carried > 0 {
        println!(
            "  carried   {carried} worktree{} inside the repo directory",
            plural(carried)
        );
    }

    let repaired = plan.repair_paths().len();
    if repaired > 0 {
        println!("  repair    {repaired} worktree{}", plural(repaired));
    }

    println!(
        "  trust     re-key {}  ->  {}",
        plan.old_git_dir.display(),
        plan.new_git_dir.display()
    );
    println!("  catalog   path  ->  {}", plan.new_root.display());

    match &plan.name {
        NamePlan::Explicit(name) => {
            println!("  catalog   name  {}  ->  {name}", plan.current_name)
        }
        NamePlan::FollowsDirectory(name) => println!(
            "  catalog   name  {}  ->  {name}  (follows the directory; --name overrides)",
            plan.current_name
        ),
        NamePlan::KeptNameTaken { wanted, holder } => println!(
            "  catalog   name  {} kept — '{wanted}' is already used by the repo at {holder}",
            plan.current_name
        ),
        NamePlan::Keep => println!("  catalog   name  {} unchanged", plan.current_name),
    }

    println!("  caches    evict cached sizes");
}

fn branch_suffix(branch: Option<&str>) -> String {
    match branch {
        Some(branch) => String::from("  (") + branch + ")",
        None => String::from("  (detached)"),
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}
