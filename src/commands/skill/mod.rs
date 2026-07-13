//! `daft skill` subcommand category.
//!
//! Verbs: `install` (write the embedded agent skill to a skills directory —
//! install doubles as update), `uninstall` (remove it), and `show` (print the
//! embedded skill to stdout). The skill content itself lives in
//! `crate::skill`; this module is only the CLI surface.

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::core::repo::{WorktreePosition, resolve_worktree_position};

pub mod install;
pub mod show;
pub mod uninstall;

/// Dispatch entry from the top-level main.
pub fn run() -> Result<()> {
    let args: Vec<String> = crate::cli::argv().to_vec();
    let sub = args.get(2).map(String::as_str).unwrap_or("");
    match sub {
        "install" => install::run(),
        "uninstall" => uninstall::run(),
        "show" => show::run(),
        "" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        "--version" | "-V" => {
            println!("daft skill {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => crate::suggest::handle_unknown_subcommand(
            "daft skill",
            other,
            crate::suggest::DAFT_SKILL_SUBCOMMANDS,
        ),
    }
}

fn print_help() {
    println!(
        "daft skill — the daft agent skill ({}), embedded v{}",
        crate::skill::SKILL_DIR_NAME,
        crate::skill::embedded_version()
    );
    println!();
    println!("Subcommands:");
    println!("  install     Install or update the agent skill for Claude Code (~/.claude/skills)");
    println!("  uninstall   Remove the installed agent skill");
    println!("  show        Print the embedded SKILL.md to stdout");
}

/// Resolve which skills root an `install`/`uninstall` acts on. Exactly one of
/// the three targets applies: `--dir` verbatim, `--project` (the current
/// worktree's `.claude/skills/`), or the user-global default. `verb` names the
/// command in the error tips (`skill install` / `skill uninstall`), so both
/// verbs share one resolution path and cannot drift.
pub(super) fn resolve_skills_root(
    project: bool,
    dir: Option<&Path>,
    verb: &str,
) -> Result<PathBuf> {
    if let Some(dir) = dir {
        return Ok(dir.to_path_buf());
    }
    if project {
        let cwd = crate::utils::get_current_directory()?;
        return match resolve_worktree_position(&cwd) {
            WorktreePosition::InWorktree { root } => Ok(root.join(".claude").join("skills")),
            WorktreePosition::ContainerRoot { .. } => anyhow::bail!(
                "--project requires a worktree, and the bare container root has no work tree of its own\n  \
                 tip: cd into a worktree first, or use `{}`",
                crate::daft_cmd(&format!("{verb} --dir <path>"))
            ),
            WorktreePosition::NotInRepo => anyhow::bail!(
                "--project requires a Git repository\n  \
                 tip: run it from inside a worktree, or use `{}`",
                crate::daft_cmd(&format!("{verb} --dir <path>"))
            ),
        };
    }
    crate::skill::user_skills_root()
        .ok_or_else(|| anyhow::anyhow!("could not resolve the home directory for ~/.claude/skills"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn dir_flag_wins_verbatim() {
        let root = resolve_skills_root(false, Some(Path::new("/tmp/some/skills")), "skill install")
            .unwrap();
        assert_eq!(root, PathBuf::from("/tmp/some/skills"));
    }

    #[test]
    #[serial]
    fn default_targets_user_skills_root() {
        // The dev/test shell (shared-env.sh) exports DAFT_SKILLS_DIR, which
        // cfg!(test) would honor — clear it so this exercises the real
        // ~/.claude fallback. #[serial] keeps it from racing the other env
        // tests (same reason lib.rs's DAFT_*_DIR default tests are serial).
        unsafe {
            std::env::remove_var(crate::skill::SKILLS_DIR_ENV);
        }
        let root = resolve_skills_root(false, None, "skill install").unwrap();
        assert!(root.ends_with(".claude/skills"), "{root:?}");
    }
}
