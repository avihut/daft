use crate::{
    get_current_branch, get_git_common_dir,
    git::GitCommand,
    hooks::{HookContext, HookExecutor, HookType, HooksConfig, TrustLevel},
    is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
    utils::*,
};
use anyhow::{Context, Result};
use clap::Parser;
use std::fs;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "git-worktree-flow-adopt")]
#[command(version = crate::VERSION)]
#[command(about = "Convert a traditional repository to worktree-based layout")]
#[command(long_about = r#"
WHAT THIS COMMAND DOES

Converts your existing Git repository from the traditional layout to daft's
worktree-based layout. After conversion:

  Before:                    After:
  my-project/                my-project/
  ├── .git/                  ├── .git/        (bare repository)
  ├── src/                   └── main/        (worktree)
  └── README.md                  ├── src/
                                 └── README.md

Your uncommitted changes (staged and unstaged) are preserved in the new
worktree. The command is safe to run - if anything fails, your repository
is restored to its original state.

ABOUT THE WORKTREE WORKFLOW

The worktree workflow eliminates Git branch switching friction by giving
each branch its own directory. Instead of switching branches within a
single directory, you navigate between directories - each containing
a different branch.

BENEFITS

- No more stashing: Each branch has its own working directory
- Parallel development: Work on multiple branches simultaneously
- Persistent context: Each worktree keeps its own IDE state, terminal
  history, and environment (.envrc, node_modules, etc.)
- Instant switching: Just cd to another directory
- Safe experimentation: Changes in one worktree never affect another

HOW TO WORK WITH IT

After adopting, use these commands:

  git worktree-checkout <branch>
      Check out an existing branch into a new worktree

  git worktree-checkout-branch <new-branch>
      Create a new branch and worktree from current branch

  git worktree-checkout-branch-from-default <new-branch>
      Create a new branch from the remote's default branch

  git worktree-prune
      Clean up worktrees for merged/deleted branches

Your directory structure grows as you work:

  my-project/
  ├── .git/
  ├── main/              # Default branch
  ├── feature/auth/      # Feature branch
  └── bugfix/login/      # Bugfix branch

REVERTING

To convert back to a traditional layout, use git-worktree-flow-eject(1).
"#)]
pub struct Args {
    #[arg(help = "Path to the repository to convert (defaults to current directory)")]
    repository_path: Option<PathBuf>,

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
        long = "trust-hooks",
        help = "Trust the repository and allow hooks to run without prompting"
    )]
    trust_hooks: bool,

    #[arg(long = "no-hooks", help = "Do not run any hooks from the repository")]
    no_hooks: bool,

    #[arg(
        long = "dry-run",
        help = "Show what would be done without making any changes"
    )]
    dry_run: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-flow-adopt"));

    // Initialize logging based on verbose flag
    init_logging(args.verbose);

    if args.trust_hooks && args.no_hooks {
        anyhow::bail!("--trust-hooks and --no-hooks cannot be used together.");
    }

    // Load settings
    let settings = DaftSettings::load_global()?;

    let config = OutputConfig::with_autocd(args.quiet, args.verbose, settings.autocd);
    let mut output = CliOutput::new(config);

    let original_dir = get_current_directory()?;

    if let Err(e) = run_adopt(&args, &settings, &mut output) {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    Ok(())
}

fn run_adopt(args: &Args, settings: &DaftSettings, output: &mut dyn Output) -> Result<()> {
    // Change to repository path if provided
    if let Some(ref repo_path) = args.repository_path {
        if !repo_path.exists() {
            anyhow::bail!("Repository path does not exist: {}", repo_path.display());
        }
        change_directory(repo_path)?;
    }

    // Validate we're in a git repository
    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let git = GitCommand::new(output.is_quiet()).with_gitoxide(settings.use_gitoxide);

    // Check if already in worktree layout
    if is_worktree_layout(&git)? {
        anyhow::bail!(
            "Repository is already in worktree layout.\n\
             Use git-worktree-checkout or git-worktree-checkout-branch to create new worktrees."
        );
    }

    // Get current branch
    let current_branch = get_current_branch().context("Could not determine current branch")?;
    output.step(&format!("Current branch: '{current_branch}'"));

    // Get the project root (parent of .git)
    // Need to canonicalize because get_git_common_dir() may return a relative path
    let git_dir = get_git_common_dir()?;
    let git_dir = std::fs::canonicalize(&git_dir)
        .with_context(|| format!("Could not canonicalize git dir: {}", git_dir.display()))?;
    let project_root = git_dir
        .parent()
        .context("Could not determine project root")?
        .to_path_buf();

    output.step(&format!("Project root: '{}'", project_root.display()));

    // Check for uncommitted changes
    let has_changes = git.has_uncommitted_changes()?;
    if has_changes {
        output.step("Uncommitted changes detected - will preserve them");
    }

    // Calculate new worktree path
    let worktree_path = project_root.join(&current_branch);

    if args.dry_run {
        output.step("[DRY RUN] Would perform the following actions:");
        output.step(&format!(
            "[DRY RUN] Move all files to '{}'",
            worktree_path.display()
        ));
        output.step("[DRY RUN] Convert .git to bare repository");
        output.step(&format!(
            "[DRY RUN] Register worktree for branch '{current_branch}'"
        ));
        if has_changes {
            output.step("[DRY RUN] Restore uncommitted changes in new worktree");
        }
        output.result(&format!(
            "Would convert to worktree layout with branch '{}' at '{}'",
            current_branch,
            worktree_path.display()
        ));
        return Ok(());
    }

    // Stash changes if any
    let stash_message = "daft-flow-adopt: temporary stash for conversion";
    if has_changes {
        output.step("Stashing uncommitted changes...");
        git.stash_push_with_untracked(stash_message)
            .context("Failed to stash changes")?;
    }

    // Ensure we're at the project root
    change_directory(&project_root)?;

    // Get list of all files/directories to move (everything except .git)
    let entries_to_move: Vec<PathBuf> = fs::read_dir(&project_root)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .map(|name| name != ".git")
                .unwrap_or(false)
        })
        .collect();

    if entries_to_move.is_empty() {
        output.step("No files to move (empty repository)");
    } else {
        output.step(&format!(
            "Moving {} items to worktree...",
            entries_to_move.len()
        ));
    }

    // Use a staging directory inside .git to avoid path conflicts.
    // This handles the case where branch name is "feature/something" and there's
    // already a "feature" directory in the project - we can't create feature/something
    // and then try to move feature into it (would be moving a dir into its own subdir).
    let staging_dir = git_dir.join("daft-adopt-staging");
    fs::create_dir_all(&staging_dir).with_context(|| {
        format!(
            "Failed to create staging directory: {}",
            staging_dir.display()
        )
    })?;

    // Move all files to the staging directory first
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

    // Now create the worktree directory (safe because project root is empty except .git)
    fs::create_dir_all(&worktree_path).with_context(|| {
        format!(
            "Failed to create worktree directory: {}",
            worktree_path.display()
        )
    })?;

    // Move all files from staging to the worktree directory
    let staged_entries: Vec<PathBuf> = fs::read_dir(&staging_dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .collect();

    for entry in &staged_entries {
        let file_name = entry.file_name().context("Could not get file name")?;
        let dest = worktree_path.join(file_name);

        fs::rename(entry, &dest).with_context(|| {
            format!(
                "Failed to move '{}' to '{}'",
                entry.display(),
                dest.display()
            )
        })?;
    }

    // Clean up the staging directory
    fs::remove_dir(&staging_dir).ok();

    // Convert .git to bare repository
    output.step("Converting to bare repository...");
    git.config_set("core.bare", "true")
        .context("Failed to set core.bare")?;

    // Remove the index file from the bare repo (bare repos don't need an index)
    let bare_index = git_dir.join("index");
    if bare_index.exists() {
        fs::remove_file(&bare_index).ok();
    }

    // Setup fetch refspec for remote tracking
    let settings = DaftSettings::load_global()?;
    output.step("Setting up fetch refspec for remote tracking...");
    if let Err(e) = git.setup_fetch_refspec(&settings.remote) {
        output.warning(&format!("Could not set fetch refspec: {e}"));
    }

    // Create .git file in worktree pointing to the actual .git directory
    let gitdir_content = format!("gitdir: {}", git_dir.display());
    let worktree_git_file = worktree_path.join(".git");
    fs::write(&worktree_git_file, gitdir_content)
        .context("Failed to create .git file in worktree")?;

    // Register the worktree with git
    output.step(&format!(
        "Registering worktree for branch '{current_branch}'..."
    ));

    // We need to add worktree info to the bare repo
    // Git worktrees are tracked in .git/worktrees/<name>/
    // Sanitize branch name for directory (replace / with -)
    let worktree_name = current_branch.replace('/', "-");
    let worktrees_dir = git_dir.join("worktrees").join(&worktree_name);
    fs::create_dir_all(&worktrees_dir).context("Failed to create worktrees directory")?;

    // Write gitdir file pointing to the worktree's .git file
    let gitdir_path = worktrees_dir.join("gitdir");
    fs::write(&gitdir_path, format!("{}\n", worktree_git_file.display()))
        .context("Failed to write gitdir file")?;

    // Write HEAD file
    let head_path = worktrees_dir.join("HEAD");
    fs::write(&head_path, format!("ref: refs/heads/{current_branch}\n"))
        .context("Failed to write HEAD file")?;

    // Write commondir file (required for git to find the shared refs)
    let commondir_path = worktrees_dir.join("commondir");
    fs::write(&commondir_path, "../..\n").context("Failed to write commondir file")?;

    // Update .git file in worktree to point to worktrees subdirectory
    let correct_gitdir = format!("gitdir: {}", worktrees_dir.display());
    fs::write(&worktree_git_file, correct_gitdir)
        .context("Failed to update .git file in worktree")?;

    // Change to the new worktree
    change_directory(&worktree_path)?;

    // Initialize the index for this worktree
    // The worktree needs its own index file to track the working tree state
    output.step("Initializing worktree index...");
    let reset_result = std::process::Command::new("git")
        .args(["reset", "--mixed", "HEAD"])
        .current_dir(&worktree_path)
        .output()
        .context("Failed to initialize worktree index")?;

    if !reset_result.status.success() {
        let stderr = String::from_utf8_lossy(&reset_result.stderr);
        output.warning(&format!("git reset warning: {}", stderr.trim()));
    }

    // Restore stashed changes if any
    if has_changes {
        output.step("Restoring uncommitted changes...");
        if let Err(e) = git.stash_pop() {
            output.warning(&format!("Could not restore stashed changes: {e}"));
            output.warning("Your changes are still in the stash. Run 'git stash pop' manually.");
        }
    }

    // Run post-clone hook
    run_post_adopt_hook(
        args,
        &project_root,
        &git_dir,
        &settings.remote,
        &worktree_path,
        &current_branch,
        output,
    )?;

    output.result(&format!(
        "Converted to worktree layout. Working directory: '{}/{}'",
        project_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("repository"),
        current_branch
    ));

    output.cd_path(&get_current_directory()?);

    Ok(())
}

/// Check if the repository is already in worktree layout.
/// Returns true if:
/// - The .git is a bare repository AND
/// - There are worktrees registered
fn is_worktree_layout(git: &GitCommand) -> Result<bool> {
    // Check if core.bare is true
    if let Ok(Some(bare_value)) = git.config_get("core.bare") {
        if bare_value.to_lowercase() == "true" {
            // Check if there are any worktrees registered
            let worktree_output = git.worktree_list_porcelain()?;
            // A bare repo with worktrees will have multiple "worktree" entries
            let worktree_count = worktree_output
                .lines()
                .filter(|line| line.starts_with("worktree "))
                .count();

            // Bare repo with at least one worktree means it's in worktree layout
            return Ok(worktree_count > 0);
        }
    }

    Ok(false)
}

#[allow(clippy::too_many_arguments)]
fn run_post_adopt_hook(
    args: &Args,
    project_root: &PathBuf,
    git_dir: &PathBuf,
    remote_name: &str,
    worktree_path: &PathBuf,
    branch_name: &str,
    output: &mut dyn Output,
) -> Result<()> {
    // Skip hooks if --no-hooks flag is set
    if args.no_hooks {
        output.step("Skipping hooks (--no-hooks flag)");
        return Ok(());
    }

    let hooks_config = HooksConfig::default();
    let mut executor = HookExecutor::new(hooks_config)?;

    // If --trust-hooks flag is set, trust the repository first
    if args.trust_hooks {
        output.step("Trusting repository for hooks (--trust-hooks flag)");
        executor.trust_repository(git_dir, TrustLevel::Allow)?;
    }

    // Build the hook context using PostClone hook type
    // (adopt is similar to clone - initial setup of worktree layout)
    let ctx = HookContext::new(
        HookType::PostClone,
        "adopt",
        project_root,
        git_dir,
        remote_name,
        worktree_path,
        worktree_path,
        branch_name,
    )
    .with_new_branch(false);

    // Execute the hook
    let result = executor.execute(&ctx, output)?;

    // If hooks were skipped due to trust, show notice
    if result.skipped {
        if let Some(reason) = &result.skip_reason {
            if reason == "Repository not trusted" {
                executor.check_hooks_notice(worktree_path, git_dir, output);
            }
        }
    }

    Ok(())
}
