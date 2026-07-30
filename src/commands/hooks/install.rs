//! `daft hooks install` / `daft hooks uninstall` — making git call daft.
//!
//! The install is the whole of daft's footprint as a hooks manager: shim
//! files under `.git/hooks/`, a record beside them, and nothing in the
//! tracked tree. That is what makes trying daft a reversible decision.

use crate::hooks::git_stage::gitdir::{self, GitDirs};
use crate::hooks::git_stage::install::{self as stage_install, InstallBlocker, StagePlan};
use crate::output::Output;
use crate::styles::{bold, cyan, dim, green, yellow};
use anyhow::{Context, Result};

/// Install shims for every stage daft manages.
pub(super) fn cmd_install(force: bool, output: &mut dyn Output) -> Result<()> {
    let dirs = resolve_dirs()?;
    let hooks_path = configured_hooks_path(&dirs);

    let saved_hooks_path = match stage_install::hooks_path_blocker(hooks_path.as_deref(), &dirs) {
        None => None,
        Some(InstallBlocker::HooksPath { path, in_worktree }) if force => {
            unset_hooks_path(&dirs)?;
            output.warning(&format!(
                "core.hooksPath was {}; unset so git will look in its default hooks \
                 directory{}. `{}` puts it back.",
                bold(&path),
                if in_worktree {
                    " — the repository's tracked hooks will stop running for you"
                } else {
                    ""
                },
                cyan(&crate::daft_cmd("hooks uninstall"))
            ));
            Some(path)
        }
        Some(InstallBlocker::HooksPath { path, in_worktree }) => {
            // Installing anyway would write files git never looks at, which
            // is worse than refusing: the shims would be there, `status`
            // would look right, and no gate would ever fire.
            anyhow::bail!(
                "core.hooksPath is set to '{path}', so git will not look in the directory \
                 daft installs into.{}\n\nRe-run with --force to unset it (uninstall \
                 restores it), or point core.hooksPath at git's default.",
                if in_worktree {
                    " That path is inside the working tree, so it is probably a tracked \
                     hooks directory shared with the rest of the team — unsetting it \
                     stops those hooks running for you."
                } else {
                    ""
                }
            );
        }
    };

    let binary = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .context("could not resolve the daft binary's path")?;

    let outcomes = stage_install::install(&dirs, &binary, force, saved_hooks_path)?;

    let skipped: Vec<_> = outcomes
        .iter()
        .filter(|o| matches!(o.plan, StagePlan::Skip { .. }))
        .collect();
    let backed_up: Vec<_> = outcomes
        .iter()
        .filter(|o| matches!(o.plan, StagePlan::BackUp { .. } | StagePlan::ForceBackUp))
        .collect();
    let written = outcomes.len() - skipped.len();

    output.success(&format!(
        "Installed git hooks for {} {} in {}",
        written,
        if written == 1 { "stage" } else { "stages" },
        dim(&dirs.default_hooks_dir().display().to_string())
    ));

    for outcome in &backed_up {
        let manager = match outcome.plan {
            StagePlan::BackUp { manager } => manager,
            _ => "the previous hook",
        };
        output.info(&format!(
            "  {} {} moved aside to {}{}",
            dim("·"),
            bold(outcome.stage.git_hook_filename()),
            outcome.stage.git_hook_filename(),
            dim(&format!("{} ({manager})", stage_install::BACKUP_SUFFIX))
        ));
    }
    for outcome in &skipped {
        output.info(&format!(
            "  {} {} left alone: {}",
            yellow("!"),
            bold(outcome.stage.git_hook_filename()),
            match &outcome.plan {
                StagePlan::Skip { reason } => reason.as_str(),
                _ => "",
            }
        ));
    }
    if !skipped.is_empty() {
        output.info(&format!(
            "  {} re-run with {} to back those up and install anyway.",
            dim("Tip:"),
            cyan("--force")
        ));
    }

    output.info("");
    output.info(&format!(
        "Stages run from your {} — nothing is baked into the hook files, so \
         editing it takes effect immediately.",
        bold("daft.yml hooks:")
    ));
    output.info(&format!(
        "Undo with {}.",
        cyan(&crate::daft_cmd("hooks uninstall"))
    ));
    Ok(())
}

/// Remove daft's shims and restore whatever they displaced.
pub(super) fn cmd_uninstall(output: &mut dyn Output) -> Result<()> {
    let dirs = resolve_dirs()?;
    let report = stage_install::uninstall(&dirs)?;

    if report.removed.is_empty() && report.left_alone.is_empty() {
        output.info("No daft hook shims are installed here.");
        return Ok(());
    }

    output.success(&format!(
        "Removed {} daft hook {}",
        report.removed.len(),
        if report.removed.len() == 1 {
            "shim"
        } else {
            "shims"
        }
    ));
    if !report.restored.is_empty() {
        output.info(&format!(
            "  {} restored: {}",
            green("·"),
            report.restored.join(", ")
        ));
    }
    for name in &report.left_alone {
        output.info(&format!(
            "  {} {} was not written by daft — left in place",
            yellow("!"),
            bold(name)
        ));
    }
    if let Some(path) = report.restore_hooks_path {
        restore_hooks_path(&dirs, &path)?;
        output.info(&format!(
            "  {} core.hooksPath restored to {}",
            green("·"),
            bold(&path)
        ));
    }
    Ok(())
}

/// The git directories for the worktree the command was run in.
fn resolve_dirs() -> Result<GitDirs> {
    let cwd = std::env::current_dir().context("could not read the current directory")?;
    gitdir::discover(&cwd).context("not inside a git repository — run this from a worktree")
}

/// The repository's `core.hooksPath`, if any.
fn configured_hooks_path(dirs: &GitDirs) -> Option<String> {
    let output = crate::utils::git_command_at(&dirs.worktree_root)
        .args(["config", "--get", "core.hooksPath"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn unset_hooks_path(dirs: &GitDirs) -> Result<()> {
    let status = crate::utils::git_command_at(&dirs.worktree_root)
        .args(["config", "--unset-all", "core.hooksPath"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("failed to unset core.hooksPath")?;
    // Exit 5 is "nothing to unset", which is fine.
    if !status.success() && status.code() != Some(5) {
        anyhow::bail!("failed to unset core.hooksPath");
    }
    Ok(())
}

fn restore_hooks_path(dirs: &GitDirs, value: &str) -> Result<()> {
    let status = crate::utils::git_command_at(&dirs.worktree_root)
        .args(["config", "core.hooksPath", value])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("failed to restore core.hooksPath")?;
    if !status.success() {
        anyhow::bail!("failed to restore core.hooksPath to '{value}'");
    }
    Ok(())
}
