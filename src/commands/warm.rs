use crate::{
    WorktreeConfig,
    core::{
        OutputSink,
        copy_paths::{CopyOutcome, CopyPathsResult},
        worktree::warm,
    },
    get_project_root,
    git::GitCommand,
    is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
};
use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(name = "git-worktree-warm")]
#[command(version = crate::VERSION)]
#[command(about = "Copy declared build caches into a worktree")]
#[command(long_about = r#"
Replicates the paths declared under `copy:` in daft.yml from one worktree
into another, so a worktree starts warm instead of rebuilding caches that
already exist next door.

This is the manual re-run of the copy stage that worktree creation performs
automatically. Use it for a worktree created before `copy:` was declared, or
to re-seed a cache that has since been rebuilt in the source worktree.

By default the worktree you are standing in is warmed from the default
branch's worktree; naming a worktree warms that one from where you stand.
Both the target and --from accept a worktree directory name, a branch name,
or a path under the project root.

Entries that already exist in the target are left alone, which makes repeat
runs a no-op; pass --force to replace them. On a filesystem that supports
copy-on-write (APFS, btrfs, XFS with reflink=1, OpenZFS 2.2+, ReFS) the copy
is near-free until the caches diverge.

Copy failures never fail the command: an entry that is tracked by git, too
large for its max_size, or unreadable is reported and the rest still copy.
"#)]
pub struct Args {
    #[arg(help = "Worktree to warm, by directory name, branch name, or path \
                  (default: the current worktree)")]
    target: Option<String>,

    #[arg(
        long = "from",
        value_name = "worktree",
        help = "Worktree to copy from, by directory name, branch name, or path \
                (default: the current worktree, or the default branch's worktree \
                when it is the target)"
    )]
    from: Option<String>,

    #[arg(
        short = 'f',
        long = "force",
        help = "Replace entries that already exist in the target worktree"
    )]
    force: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-warm"));

    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;
    let config = OutputConfig::new(false, args.verbose);
    let mut output = CliOutput::new(config);

    let wt_config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: false,
    };
    let git = GitCommand::new(wt_config.quiet).with_gitoxide(settings.use_gitoxide);
    let project_root = get_project_root()?;

    let params = warm::WarmParams {
        target: args.target,
        from: args.from,
        force: args.force,
    };

    let result = {
        let mut sink = OutputSink(&mut output);
        warm::execute(&params, &git, &project_root, &mut sink)?
    };

    render_warm_result(&result, args.force, &mut output);

    Ok(())
}

/// Render one line per declared entry, then a summary.
///
/// Plain lines, not the creation rail: `warm` is a foreground command with no
/// surrounding plan to slot rows into, and the shared-sync style ("what
/// happened to each declared thing") is what a re-run wants to read.
///
/// Nothing here escalates to a non-zero exit — the engine's warn-never-abort
/// contract reaches all the way to the process status, so a cache that could
/// not be copied never breaks a script that warms a worktree before building.
fn render_warm_result(result: &warm::WarmResult, forced: bool, output: &mut dyn Output) {
    if result.nothing_declared() {
        output.info(&format!(
            "No `copy:` paths declared in '{}'.",
            result.source_name
        ));
        output.info(&format!(
            "Declare the caches worth replicating in daft.yml, then run `{}`.",
            crate::daft_cmd("warm")
        ));
        return;
    }

    output.step(&format!(
        "Warming '{}' from '{}'",
        result.target_name, result.source_name
    ));

    for outcome in &result.outcome.outcomes {
        match outcome {
            CopyOutcome::Copied {
                entry,
                method,
                matches,
                bytes,
                elapsed,
            } => output.success(&format!(
                "{entry} \u{2192} {}",
                warm::copied_annotation(*matches, *bytes, *method, *elapsed)
            )),
            CopyOutcome::Skipped { entry, reason } => match reason {
                // The attention skips: the config asked for something daft
                // would not or could not do, so they must not read as routine.
                crate::core::copy_paths::SkipReason::NotIgnored
                | crate::core::copy_paths::SkipReason::NoReflink
                | crate::core::copy_paths::SkipReason::TooLarge { .. } => {
                    output.warning(&format!("{entry}: {}", warm::skip_phrase(reason, entry)))
                }
                _ => output.info(&format!("{entry}: {}", warm::skip_phrase(reason, entry))),
            },
            // Loud but not fatal, matching the rail's yellow attention row:
            // the worktree is fine, one of its caches is not.
            CopyOutcome::Failed { entry, detail } => output.warning(&format!("{entry}: {detail}")),
        }
    }

    // A declared entry with no outcome would otherwise vanish between the
    // config and the summary. The engine promises this cannot happen; saying
    // so out loud is what makes a broken promise visible instead of silent.
    for entry in warm::unreported(&result.declared, &result.outcome) {
        output.warning(&format!("{entry}: not reported by the copy stage"));
    }

    output.result(&summary_line(&result.outcome, result.declared.len()));

    if result.has_existing_skips() && !forced {
        output.info(&format!(
            "Entries already present were left alone; run `{}` to replace them.",
            crate::daft_cmd("warm --force")
        ));
    }
}

/// `Copied 2 of 3 declared paths (1.2 GB).` — the one line a scripted caller
/// greps for.
fn summary_line(result: &CopyPathsResult, declared: usize) -> String {
    let copied = result.copied_count();
    if copied == 0 {
        return format!(
            "Copied nothing; {declared} declared path{} needed no work.",
            plural(declared)
        );
    }
    format!(
        "Copied {copied} of {declared} declared path{} ({}).",
        plural(declared),
        warm::format_bytes(result.copied_bytes())
    )
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::copy_paths::{CopyMethod, SkipReason};
    use std::time::Duration;

    fn copied(entry: &str, bytes: u64) -> CopyOutcome {
        CopyOutcome::Copied {
            entry: entry.into(),
            method: CopyMethod::Reflinked,
            matches: 1,
            bytes,
            elapsed: Duration::from_millis(100),
        }
    }

    #[test]
    fn summary_counts_copies_against_declarations() {
        let result = CopyPathsResult {
            outcomes: vec![
                copied("target", 1024),
                CopyOutcome::Skipped {
                    entry: "node_modules".into(),
                    reason: SkipReason::DestinationExists,
                },
            ],
        };
        assert_eq!(
            summary_line(&result, 2),
            "Copied 1 of 2 declared paths (1.0 KB)."
        );
    }

    #[test]
    fn summary_stays_honest_when_nothing_was_copied() {
        // "Copied 0 of 3" reads like a failure; every one of those three may
        // simply have been already present. The no-work phrasing keeps a
        // clean re-run from looking broken.
        let result = CopyPathsResult {
            outcomes: vec![CopyOutcome::Skipped {
                entry: "target".into(),
                reason: SkipReason::NoSource,
            }],
        };
        assert_eq!(
            summary_line(&result, 3),
            "Copied nothing; 3 declared paths needed no work."
        );
        assert_eq!(
            summary_line(&result, 1),
            "Copied nothing; 1 declared path needed no work."
        );
    }

    #[test]
    fn summary_singularizes_a_lone_declaration() {
        let result = CopyPathsResult {
            outcomes: vec![copied("target", 0)],
        };
        assert_eq!(
            summary_line(&result, 1),
            "Copied 1 of 1 declared path (0 B)."
        );
    }
}
