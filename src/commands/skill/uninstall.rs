//! `daft skill uninstall` — remove an installed agent skill.
//!
//! The inverse of `install`, and safe by construction: `crate::skill::remove_from`
//! only deletes a `daft-worktree-workflow/SKILL.md` whose frontmatter marks it
//! as the daft skill, and only removes the containing directory when nothing
//! else is left inside it — a user's own files beside the skill are never
//! touched. A missing skill is a no-op, not an error, so it can be run blindly.

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

use crate::output::{CliOutput, Output, OutputConfig};
use crate::skill::{self, RemoveOutcome};

#[derive(Parser, Debug)]
#[command(name = "git-daft-skill-uninstall")]
#[command(version = crate::VERSION)]
#[command(about = "Remove the installed agent skill")]
#[command(long_about = r#"
Removes an agent skill previously written by `git daft skill install` (the
daft-worktree-workflow skill).

By default it removes the user-global copy
(~/.claude/skills/daft-worktree-workflow/). Use --project to remove the
current worktree's .claude/skills/ copy, or --dir to target another
agent's skills root.

Removal is safe by construction: only a SKILL.md whose frontmatter marks
it as the daft skill is deleted, and the daft-worktree-workflow directory
is removed only when nothing else is left inside it, so files you keep
beside the skill are preserved. A missing skill is a no-op, not an error.
"#)]
pub struct Args {
    #[arg(
        long,
        conflicts_with = "dir",
        help = "Remove from the current worktree's .claude/skills/ instead of ~/.claude/skills"
    )]
    project: bool,

    #[arg(
        long,
        value_name = "PATH",
        help = "Remove from this skills root (for agents other than Claude Code)"
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
        raw_args.len() >= 3 && raw_args[1] == "skill" && raw_args[2] == "uninstall",
        "skill::uninstall::run() invoked with unexpected argv: {raw_args:?} \
         (expected `daft skill uninstall ...`)"
    );
    let argv: Vec<String> = std::iter::once("git-daft-skill-uninstall".to_string())
        .chain(raw_args.into_iter().skip(3))
        .collect();
    let args = Args::parse_from(argv);
    let mut output = CliOutput::new(OutputConfig::new(args.quiet, args.verbose));

    let skills_root =
        super::resolve_skills_root(args.project, args.dir.as_deref(), "skill uninstall")?;
    let (dir, outcome) = skill::remove_from(&skills_root)?;

    let cwd = crate::utils::get_current_directory().ok();
    let shown = crate::output::format::display_path(&dir.to_string_lossy(), cwd.as_deref());
    match outcome {
        RemoveOutcome::Removed {
            version,
            dir_removed,
        } => {
            let v = version.map_or_else(|| "unstamped".to_string(), |v| format!("v{v}"));
            output.result(&format!("Removed agent skill ({v}) → {shown}"));
            if !dir_removed {
                output.notice(&format!("kept {shown} — it still contains other files"));
            }
        }
        RemoveOutcome::NotInstalled => {
            output.result(&format!(
                "No agent skill installed at {shown} (nothing to remove)"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_conflicts_with_dir() {
        let err = Args::try_parse_from(["git-daft-skill-uninstall", "--project", "--dir", "/x"]);
        assert!(err.is_err());
    }
}
