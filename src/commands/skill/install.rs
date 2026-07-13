//! `daft skill install` — write the embedded agent skill to disk.
//!
//! The skill embedded in the binary always documents this binary's command
//! surface, so re-running the command after upgrading daft is the update
//! path: install == update. No network, no prompts — agents are a primary
//! caller.

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

use crate::core::repo::{WorktreePosition, resolve_worktree_position};
use crate::output::{CliOutput, Output, OutputConfig};
use crate::skill::{self, InstallOutcome};

#[derive(Parser, Debug)]
#[command(name = "git-daft-skill-install")]
#[command(version = crate::VERSION)]
#[command(about = "Install or update the agent skill for Claude Code")]
#[command(long_about = r#"
Writes the agent skill embedded in this daft binary (the repository's
SKILL.md, skill name `daft-worktree-workflow`) into a skills directory,
creating parent directories as needed.

The embedded skill is version-matched to the binary by construction, so
re-running the command after upgrading daft is also the update path:
install == update. An existing copy is always overwritten unless it is
already identical. The command is non-interactive and never touches the
network.

By default the skill lands in Claude Code's user-global skills directory
(~/.claude/skills/daft-worktree-workflow/SKILL.md). Use --project to
install into the current worktree's .claude/skills/ instead (commit it to
share the skill with everyone who clones the repo), or --dir to target
another agent's skills root; the daft-worktree-workflow folder is always
created inside the chosen root, because the folder name is what agents
resolve the skill by.

`git daft doctor` reports when an installed copy is stale relative to the
running binary, and `git daft doctor --fix` rewrites it with the same
content this command installs.
"#)]
pub struct Args {
    #[arg(
        long,
        conflicts_with = "dir",
        help = "Install into the current worktree's .claude/skills/ instead of ~/.claude/skills"
    )]
    project: bool,

    #[arg(
        long,
        value_name = "PATH",
        help = "Install under this skills root (for agents other than Claude Code)"
    )]
    dir: Option<PathBuf>,

    #[arg(short = 'q', long = "quiet", help = "Suppress the result line")]
    quiet: bool,

    #[arg(short = 'v', long = "verbose", help = "Show detailed progress")]
    verbose: bool,
}

pub fn run() -> Result<()> {
    let raw_args: Vec<String> = crate::cli::argv().to_vec();
    debug_assert!(
        raw_args.len() >= 3 && raw_args[1] == "skill" && raw_args[2] == "install",
        "skill::install::run() invoked with unexpected argv: {raw_args:?} \
         (expected `daft skill install ...`)"
    );
    let argv: Vec<String> = std::iter::once("git-daft-skill-install".to_string())
        .chain(raw_args.into_iter().skip(3))
        .collect();
    let args = Args::parse_from(argv);
    let mut output = CliOutput::new(OutputConfig::new(args.quiet, args.verbose));

    let skills_root = resolve_skills_root(&args)?;
    let (target, outcome) = skill::install_to(&skills_root)?;

    let cwd = crate::utils::get_current_directory().ok();
    let shown = crate::output::format::display_path(&target.to_string_lossy(), cwd.as_deref());
    let version = skill::embedded_version();
    match outcome {
        InstallOutcome::Installed => {
            output.result(&format!("Installed agent skill (v{version}) → {shown}"));
        }
        InstallOutcome::Updated { from } => {
            let from = from.map_or_else(|| "unstamped".to_string(), |v| format!("v{v}"));
            output.result(&format!(
                "Updated agent skill ({from} → v{version}) → {shown}"
            ));
        }
        InstallOutcome::Refreshed => {
            output.result(&format!("Refreshed agent skill (v{version}) → {shown}"));
        }
        InstallOutcome::UpToDate => {
            output.result(&format!(
                "Agent skill already up to date (v{version}) at {shown}"
            ));
        }
    }
    if args.project {
        output.notice(
            "the skill is project-local; commit .claude/skills/ to share it with collaborators",
        );
    }
    Ok(())
}

/// Resolve which skills root to install under. Exactly one of the three
/// targets applies: `--dir` verbatim, `--project` (the current worktree's
/// `.claude/skills/`), or the user-global default.
fn resolve_skills_root(args: &Args) -> Result<PathBuf> {
    if let Some(dir) = &args.dir {
        return Ok(dir.clone());
    }
    if args.project {
        let cwd = crate::utils::get_current_directory()?;
        return match resolve_worktree_position(&cwd) {
            WorktreePosition::InWorktree { root } => Ok(root.join(".claude").join("skills")),
            WorktreePosition::ContainerRoot { .. } => anyhow::bail!(
                "--project requires a worktree, and the bare container root has no work tree of its own\n  \
                 tip: cd into a worktree first, or use `{}`",
                crate::daft_cmd("skill install --dir <path>")
            ),
            WorktreePosition::NotInRepo => anyhow::bail!(
                "--project requires a Git repository\n  \
                 tip: run it from inside a worktree, or use `{}`",
                crate::daft_cmd("skill install --dir <path>")
            ),
        };
    }
    crate::skill::user_skills_root()
        .ok_or_else(|| anyhow::anyhow!("could not resolve the home directory for ~/.claude/skills"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(argv: &[&str]) -> Args {
        Args::parse_from(std::iter::once("git-daft-skill-install").chain(argv.iter().copied()))
    }

    #[test]
    fn dir_flag_wins_verbatim() {
        let a = args(&["--dir", "/tmp/some/skills"]);
        let root = resolve_skills_root(&a).unwrap();
        assert_eq!(root, PathBuf::from("/tmp/some/skills"));
    }

    #[test]
    fn project_conflicts_with_dir() {
        let err = Args::try_parse_from(["git-daft-skill-install", "--project", "--dir", "/x"]);
        assert!(err.is_err());
    }

    #[test]
    fn default_targets_user_skills_root() {
        let a = args(&[]);
        let root = resolve_skills_root(&a).unwrap();
        assert!(root.ends_with(".claude/skills"), "{root:?}");
    }
}
