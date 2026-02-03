use crate::{
    get_git_common_dir,
    git::GitCommand,
    hooks::{HookContext, HookExecutor, HookType, HooksConfig, RemovalReason},
    is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    remote::get_default_branch_from_remote_head,
    settings::DaftSettings,
    utils::*,
};
use anyhow::{Context, Result};
use clap::Parser;
use std::fs;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "git-worktree-flow-eject")]
#[command(version = crate::VERSION)]
#[command(about = "Convert a worktree-based repository back to traditional layout")]
#[command(long_about = r#"
WHAT THIS COMMAND DOES

Converts your worktree-based repository back to a traditional Git layout.
This removes all worktrees except one, and moves that worktree's files
back to the repository root.

  Before:                    After:
  my-project/                my-project/
  ├── .git/                  ├── .git/
  ├── main/                  ├── src/
  │   ├── src/               └── README.md
  │   └── README.md
  └── feature/auth/
      └── ...

By default, the remote's default branch (main, master, etc.) is kept.
Use --branch to specify a different branch.

HANDLING UNCOMMITTED CHANGES

- Changes in the target branch's worktree are preserved
- Other worktrees with uncommitted changes cause the command to fail
- Use --force to delete dirty worktrees (changes will be lost!)

EXAMPLES

  git worktree-flow-eject
      Eject to the default branch

  git worktree-flow-eject -b feature/auth
      Eject, keeping the feature/auth branch

  git worktree-flow-eject --force
      Eject even if other worktrees have uncommitted changes
"#)]
pub struct Args {
    #[arg(help = "Path to the repository to convert (defaults to current directory)")]
    repository_path: Option<PathBuf>,

    #[arg(
        short = 'b',
        long = "branch",
        help = "Branch to keep (defaults to remote's default branch)"
    )]
    branch: Option<String>,

    #[arg(
        short = 'f',
        long = "force",
        help = "Delete worktrees with uncommitted changes (changes will be lost!)"
    )]
    force: bool,

    #[arg(
        short = 'q',
        long = "quiet",
        help = "Operate quietly; suppress progress reporting"
    )]
    quiet: bool,

    #[arg(
        short = 'v',
        long = "verbose",
        help = "Be verbose; show detailed progress"
    )]
    verbose: bool,

    #[arg(
        long = "dry-run",
        help = "Show what would be done without making any changes"
    )]
    dry_run: bool,
}

/// Parsed worktree information from git worktree list --porcelain
#[derive(Debug, Clone)]
struct WorktreeInfo {
    path: PathBuf,
    branch: Option<String>,
    is_bare: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-flow-eject"));

    // Initialize logging based on verbose flag
    init_logging(args.verbose);

    // Load settings
    let settings = DaftSettings::load_global()?;

    let config = OutputConfig::with_autocd(args.quiet, args.verbose, settings.autocd);
    let mut output = CliOutput::new(config);

    let original_dir = get_current_directory()?;

    if let Err(e) = run_eject(&args, &settings, &mut output) {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    Ok(())
}

fn run_eject(args: &Args, settings: &DaftSettings, output: &mut dyn Output) -> Result<()> {
    // Change to repository path if provided
    if let Some(ref repo_path) = args.repository_path {
        if !repo_path.exists() {
            anyhow::bail!("Repository path does not exist: {}", repo_path.display());
        }
        change_directory(repo_path)?;
    }

    // We might be in a worktree or at the project root
    // Try to find the project root
    // Need to canonicalize because get_git_common_dir() may return a relative path
    let git_dir = get_git_common_dir().context("Not inside a Git repository")?;
    let git_dir = std::fs::canonicalize(&git_dir)
        .with_context(|| format!("Could not canonicalize git dir: {}", git_dir.display()))?;
    let project_root = git_dir
        .parent()
        .context("Could not determine project root")?
        .to_path_buf();

    // Change to project root
    change_directory(&project_root)?;

    // Validate we're in a git repository
    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let git = GitCommand::new(output.is_quiet()).with_gitoxide(settings.use_gitoxide);

    // Check if in worktree layout
    if !is_worktree_layout(&git)? {
        anyhow::bail!(
            "Repository is not in worktree layout.\n\
             Use git-worktree-flow-adopt to convert a traditional repository to worktree layout."
        );
    }

    // Parse worktrees
    let worktrees = parse_worktrees(&git)?;
    let non_bare_worktrees: Vec<_> = worktrees.iter().filter(|wt| !wt.is_bare).collect();

    if non_bare_worktrees.is_empty() {
        anyhow::bail!(
            "No worktrees found. Cannot convert to traditional layout without at least one worktree."
        );
    }

    output.step(&format!("Found {} worktrees", non_bare_worktrees.len()));
    for wt in &non_bare_worktrees {
        let branch_display = wt.branch.as_deref().unwrap_or("(detached)");
        output.step(&format!("  - {} ({})", wt.path.display(), branch_display));
    }

    // Helper to check if a branch has a worktree
    let find_worktree_for_branch = |branch: &str| -> Option<&WorktreeInfo> {
        non_bare_worktrees
            .iter()
            .find(|wt| wt.branch.as_ref().map(|b| b == branch).unwrap_or(false))
            .copied()
    };

    // Determine target branch and worktree
    let (target_branch, target_worktree) = if let Some(ref branch) = args.branch {
        // User explicitly specified a branch - must exist
        match find_worktree_for_branch(branch) {
            Some(wt) => (branch.clone(), wt.clone()),
            None => {
                let available_branches: Vec<_> = non_bare_worktrees
                    .iter()
                    .filter_map(|wt| wt.branch.as_ref())
                    .collect();
                anyhow::bail!(
                    "No worktree found for branch '{}'. Available branches: {}",
                    branch,
                    available_branches
                        .iter()
                        .map(|b| format!("'{}'", b))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    } else {
        // No branch specified - try default, then main/master, then first available
        let default_branch =
            get_default_branch_from_remote_head(&settings.remote, settings.use_gitoxide).ok();

        // Try remote's default branch first
        if let Some(ref branch) = default_branch {
            if let Some(wt) = find_worktree_for_branch(branch) {
                (branch.clone(), wt.clone())
            } else {
                // Default branch doesn't have a worktree, try main/master
                if let Some(wt) = find_worktree_for_branch("main") {
                    ("main".to_string(), wt.clone())
                } else if let Some(wt) = find_worktree_for_branch("master") {
                    ("master".to_string(), wt.clone())
                } else {
                    // Fall back to first available worktree
                    let first_wt = non_bare_worktrees.first().unwrap();
                    let branch = first_wt
                        .branch
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string());
                    (branch, (*first_wt).clone())
                }
            }
        } else {
            // No remote default, try main/master, then first available
            if let Some(wt) = find_worktree_for_branch("main") {
                ("main".to_string(), wt.clone())
            } else if let Some(wt) = find_worktree_for_branch("master") {
                ("master".to_string(), wt.clone())
            } else {
                // Fall back to first available worktree
                let first_wt = non_bare_worktrees.first().unwrap();
                let branch = first_wt
                    .branch
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string());
                (branch, (*first_wt).clone())
            }
        }
    };

    output.step(&format!("Target branch to keep: '{target_branch}'"));

    output.step(&format!(
        "Target worktree: '{}'",
        target_worktree.path.display()
    ));

    // Check for dirty worktrees
    let mut dirty_worktrees = Vec::new();
    for wt in &non_bare_worktrees {
        if wt.path == target_worktree.path {
            continue; // Skip target worktree for dirty check (we'll preserve those changes)
        }

        // Check if worktree has uncommitted changes
        let prev_dir = get_current_directory()?;
        if change_directory(&wt.path).is_ok() && git.has_uncommitted_changes().unwrap_or(false) {
            dirty_worktrees.push(wt);
        }
        change_directory(&prev_dir).ok();
    }

    if !dirty_worktrees.is_empty() && !args.force {
        let dirty_list: Vec<String> = dirty_worktrees
            .iter()
            .map(|wt| {
                let branch = wt.branch.as_deref().unwrap_or("(detached)");
                format!("  - {} ({})", wt.path.display(), branch)
            })
            .collect();

        anyhow::bail!(
            "The following worktrees have uncommitted changes:\n{}\n\n\
             Use --force to delete these worktrees anyway (changes will be lost!).\n\
             Or commit/stash changes in these worktrees first.",
            dirty_list.join("\n")
        );
    }

    // Check if target worktree has changes
    let prev_dir = get_current_directory()?;
    change_directory(&target_worktree.path)?;
    let target_has_changes = git.has_uncommitted_changes()?;
    change_directory(&prev_dir)?;

    if target_has_changes {
        output.step("Target worktree has uncommitted changes - will preserve them");
    }

    if args.dry_run {
        output.step("[DRY RUN] Would perform the following actions:");
        for wt in &non_bare_worktrees {
            if wt.path != target_worktree.path {
                let branch = wt.branch.as_deref().unwrap_or("(detached)");
                output.step(&format!(
                    "[DRY RUN] Remove worktree '{}' ({})",
                    wt.path.display(),
                    branch
                ));
            }
        }
        output.step(&format!(
            "[DRY RUN] Move files from '{}' to '{}'",
            target_worktree.path.display(),
            project_root.display()
        ));
        output.step("[DRY RUN] Convert to non-bare repository");
        output.result(&format!(
            "Would convert to traditional layout with branch '{}'",
            target_branch
        ));
        return Ok(());
    }

    // Stash changes in target worktree if any
    let stash_message = "daft-flow-eject: temporary stash for conversion";
    if target_has_changes {
        output.step("Stashing changes in target worktree...");
        change_directory(&target_worktree.path)?;
        git.stash_push_with_untracked(stash_message)
            .context("Failed to stash changes")?;
        change_directory(&project_root)?;
    }

    // Remove non-target worktrees
    for wt in &non_bare_worktrees {
        if wt.path == target_worktree.path {
            continue;
        }

        let branch = wt.branch.as_deref().unwrap_or("unknown");

        // Run pre-remove hook
        if let Err(e) = run_pre_remove_hook(
            &project_root,
            &git_dir,
            &settings.remote,
            &target_worktree.path,
            &wt.path,
            branch,
            output,
        ) {
            output.warning(&format!("Pre-remove hook failed for {}: {}", branch, e));
        }

        output.step(&format!(
            "Removing worktree '{}' ({})...",
            wt.path.display(),
            branch
        ));

        if let Err(e) = git.worktree_remove(&wt.path, args.force) {
            output.error(&format!(
                "Failed to remove worktree '{}': {}",
                wt.path.display(),
                e
            ));
            // Try to clean up directory manually
            if wt.path.exists() {
                if let Err(e) = fs::remove_dir_all(&wt.path) {
                    output.warning(&format!("Could not remove worktree directory: {}", e));
                }
            }
        }

        // Run post-remove hook
        if let Err(e) = run_post_remove_hook(
            &project_root,
            &git_dir,
            &settings.remote,
            &target_worktree.path,
            &wt.path,
            branch,
            output,
        ) {
            output.warning(&format!("Post-remove hook failed for {}: {}", branch, e));
        }
    }

    // Move files from target worktree to project root
    // Use a staging directory to avoid conflicts when the branch name matches
    // a directory inside the worktree (e.g., branch "test" with a "test/" dir inside)
    output.step(&format!(
        "Moving files from '{}' to '{}'...",
        target_worktree.path.display(),
        project_root.display()
    ));

    let entries_to_move: Vec<PathBuf> = fs::read_dir(&target_worktree.path)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .map(|name| name != ".git")
                .unwrap_or(false)
        })
        .collect();

    // Use staging directory inside .git to avoid path conflicts
    let staging_dir = git_dir.join("daft-eject-staging");
    fs::create_dir_all(&staging_dir).with_context(|| {
        format!(
            "Failed to create staging directory: {}",
            staging_dir.display()
        )
    })?;

    // Move files from worktree to staging first
    for entry in &entries_to_move {
        let file_name = entry.file_name().context("Could not get file name")?;
        let dest = staging_dir.join(file_name);

        fs::rename(entry, &dest).with_context(|| {
            format!(
                "Failed to move '{}' to staging: {}",
                entry.display(),
                dest.display()
            )
        })?;
    }

    // Remove .git file from worktree (it's a file pointing to the actual .git)
    let worktree_git_file = target_worktree.path.join(".git");
    if worktree_git_file.exists() {
        fs::remove_file(&worktree_git_file).ok();
    }

    // Remove the now-empty worktree directory
    if target_worktree.path.exists() {
        output.step(&format!(
            "Removing worktree directory '{}'...",
            target_worktree.path.display()
        ));
        fs::remove_dir_all(&target_worktree.path).ok();
    }

    // Now move files from staging to project root (safe because worktree dir is gone)
    let staged_entries: Vec<PathBuf> = fs::read_dir(&staging_dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .collect();

    for entry in &staged_entries {
        let file_name = entry.file_name().context("Could not get file name")?;
        let dest = project_root.join(file_name);

        fs::rename(entry, &dest).with_context(|| {
            format!(
                "Failed to move '{}' to '{}'",
                entry.display(),
                dest.display()
            )
        })?;
    }

    // Clean up staging directory
    fs::remove_dir(&staging_dir).ok();

    // Remove worktree registration
    let worktrees_dir = git_dir.join("worktrees");
    if worktrees_dir.exists() {
        output.step("Cleaning up worktree registrations...");
        fs::remove_dir_all(&worktrees_dir).ok();
    }

    // Convert .git to non-bare repository
    output.step("Converting to non-bare repository...");
    git.config_set("core.bare", "false")
        .context("Failed to set core.bare to false")?;

    // Reset the index to match HEAD
    // This is needed because:
    // 1. We moved files from the worktree to the root
    // 2. The bare repo had no index
    // 3. We need to populate the index to match HEAD without touching files
    output.step(&format!("Setting up index for branch '{target_branch}'..."));

    // First, set HEAD to the target branch if not already
    let head_result = std::process::Command::new("git")
        .args([
            "symbolic-ref",
            "HEAD",
            &format!("refs/heads/{target_branch}"),
        ])
        .current_dir(&project_root)
        .output()
        .context("Failed to set HEAD")?;

    if !head_result.status.success() {
        let stderr = String::from_utf8_lossy(&head_result.stderr);
        output.warning(&format!("git symbolic-ref warning: {}", stderr.trim()));
    }

    // Now reset to populate the index without touching working files
    let reset_result = std::process::Command::new("git")
        .args(["reset", "--mixed", "HEAD"])
        .current_dir(&project_root)
        .output()
        .context("Failed to reset index")?;

    if !reset_result.status.success() {
        let stderr = String::from_utf8_lossy(&reset_result.stderr);
        output.warning(&format!("git reset warning: {}", stderr.trim()));
    }

    // Restore stashed changes if any
    if target_has_changes {
        output.step("Restoring uncommitted changes...");
        if let Err(e) = git.stash_pop() {
            output.warning(&format!("Could not restore stashed changes: {e}"));
            output.warning("Your changes are still in the stash. Run 'git stash pop' manually.");
        }
    }

    // Change to the project root
    change_directory(&project_root)?;

    output.result(&format!(
        "Converted to traditional layout on branch '{}'",
        target_branch
    ));

    output.cd_path(&project_root);

    Ok(())
}

/// Check if the repository is in worktree layout.
fn is_worktree_layout(git: &GitCommand) -> Result<bool> {
    // Check if core.bare is true
    if let Ok(Some(bare_value)) = git.config_get("core.bare") {
        if bare_value.to_lowercase() == "true" {
            // Check if there are any worktrees registered
            let worktree_output = git.worktree_list_porcelain()?;
            let worktree_count = worktree_output
                .lines()
                .filter(|line| line.starts_with("worktree "))
                .count();

            return Ok(worktree_count > 0);
        }
    }

    Ok(false)
}

/// Parse worktree list --porcelain output into structured data
fn parse_worktrees(git: &GitCommand) -> Result<Vec<WorktreeInfo>> {
    let output = git.worktree_list_porcelain()?;
    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;
    let mut is_bare = false;

    for line in output.lines() {
        if let Some(path_str) = line.strip_prefix("worktree ") {
            // Save previous worktree if any
            if let Some(path) = current_path.take() {
                worktrees.push(WorktreeInfo {
                    path,
                    branch: current_branch.take(),
                    is_bare,
                });
            }
            current_path = Some(PathBuf::from(path_str));
            current_branch = None;
            is_bare = false;
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            current_branch = branch_ref.strip_prefix("refs/heads/").map(String::from);
        } else if line == "bare" {
            is_bare = true;
        } else if line.is_empty() {
            // End of worktree entry
        }
    }

    // Don't forget the last worktree
    if let Some(path) = current_path.take() {
        worktrees.push(WorktreeInfo {
            path,
            branch: current_branch.take(),
            is_bare,
        });
    }

    Ok(worktrees)
}

fn run_pre_remove_hook(
    project_root: &PathBuf,
    git_dir: &PathBuf,
    remote_name: &str,
    source_worktree: &PathBuf,
    worktree_path: &PathBuf,
    branch_name: &str,
    output: &mut dyn Output,
) -> Result<()> {
    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;

    let ctx = HookContext::new(
        HookType::PreRemove,
        "eject",
        project_root,
        git_dir,
        remote_name,
        source_worktree,
        worktree_path,
        branch_name,
    )
    .with_removal_reason(RemovalReason::Ejecting);

    executor.execute(&ctx, output)?;

    Ok(())
}

fn run_post_remove_hook(
    project_root: &PathBuf,
    git_dir: &PathBuf,
    remote_name: &str,
    source_worktree: &PathBuf,
    worktree_path: &PathBuf,
    branch_name: &str,
    output: &mut dyn Output,
) -> Result<()> {
    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;

    let ctx = HookContext::new(
        HookType::PostRemove,
        "eject",
        project_root,
        git_dir,
        remote_name,
        source_worktree,
        worktree_path,
        branch_name,
    )
    .with_removal_reason(RemovalReason::Ejecting);

    executor.execute(&ctx, output)?;

    Ok(())
}
