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

    // ── Rendering ────────────────────────────────────────────────────────
    //
    // `render_warm_result` is where the warn-never-abort contract meets the
    // user: every per-entry problem has to land on an output channel and
    // none of them may become a `Result`. The engine can only keep that
    // promise if the renderer does too, so the whole outcome matrix is
    // rendered here and asserted channel by channel.

    use crate::core::worktree::warm::WarmResult;
    use crate::output::TestOutput;
    use std::path::PathBuf;

    fn result_of(declared: &[&str], outcomes: Vec<CopyOutcome>) -> WarmResult {
        WarmResult {
            source: PathBuf::from("/repos/acme/main"),
            target: PathBuf::from("/repos/acme/develop"),
            source_name: "main".into(),
            target_name: "develop".into(),
            declared: declared.iter().map(|s| s.to_string()).collect(),
            outcome: CopyPathsResult { outcomes },
        }
    }

    fn render(result: &WarmResult, forced: bool) -> TestOutput {
        let mut output = TestOutput::new();
        render_warm_result(result, forced, &mut output);
        output
    }

    /// The exit-status invariant, at the only place it can be observed from a
    /// unit test: rendering is infallible by signature, so no skip, no
    /// failure, and no missing outcome can turn into a non-zero exit. A
    /// future refactor that gave this function a `Result` would fail to
    /// compile here — which is the point.
    #[test]
    fn rendering_a_failure_is_infallible_and_stays_a_warning() {
        let result = result_of(
            &["target", "node_modules"],
            vec![
                CopyOutcome::Failed {
                    entry: "target".into(),
                    detail: "permission denied".into(),
                },
                copied("node_modules", 2048),
            ],
        );

        let rendered: () = render_warm_result(&result, false, &mut TestOutput::new());
        assert_eq!(rendered, (), "rendering never reports a failure upward");

        let output = render(&result, false);
        assert!(
            output.has_warning("target: permission denied"),
            "a failed entry must be loud: {:?}",
            output.warnings()
        );
        assert!(
            !output.has_errors(),
            "but never an error — the worktree is fine, one cache is not"
        );
        assert!(
            output.has_result("Copied 1 of 2 declared paths"),
            "and the summary still lands: {:?}",
            output.results()
        );
    }

    /// Every skip reason is classified deliberately: the ones the user asked
    /// for that daft would not or could not do are yellow, the routine ones
    /// are quiet. A new `SkipReason` landing in the engine has to be added
    /// here, which is exactly the review this table deserves.
    #[test]
    fn skip_reasons_split_into_attention_and_routine_channels() {
        let attention = [
            SkipReason::NotIgnored,
            SkipReason::NoReflink,
            SkipReason::TooLarge {
                size_bytes: 4096,
                limit_bytes: 1024,
            },
        ];
        for reason in attention {
            let result = result_of(
                &["target"],
                vec![CopyOutcome::Skipped {
                    entry: "target".into(),
                    reason: reason.clone(),
                }],
            );
            let output = render(&result, false);
            assert!(
                output.has_warning("target: "),
                "{reason:?} must reach the attention channel: {:?}",
                output.entries()
            );
        }

        let routine = [
            SkipReason::NoSource,
            SkipReason::DestinationExists,
            SkipReason::NoMatches,
        ];
        for reason in routine {
            let result = result_of(
                &["target"],
                vec![CopyOutcome::Skipped {
                    entry: "target".into(),
                    reason: reason.clone(),
                }],
            );
            let output = render(&result, false);
            assert!(
                !output.has_warnings(),
                "{reason:?} is routine, not a warning: {:?}",
                output.entries()
            );
            assert!(
                output.has_info("target: "),
                "{reason:?} still leaves a receipt: {:?}",
                output.infos()
            );
        }
    }

    /// Skip lines carry the entry in front and the reason behind, with no
    /// `skipped — ` prefix — the same phrasing the rail uses, so the two
    /// surfaces read identically.
    #[test]
    fn a_skip_line_is_the_entry_then_a_standalone_phrase() {
        let result = result_of(
            &["node_modules"],
            vec![CopyOutcome::Skipped {
                entry: "node_modules".into(),
                reason: SkipReason::DestinationExists,
            }],
        );
        let output = render(&result, false);
        assert!(
            output.infos().contains(&"node_modules: already present"),
            "{:?}",
            output.infos()
        );
    }

    /// The `--force` hint is the whole reason the skip is survivable, so it
    /// appears exactly when it is actionable: something was left alone, and
    /// the user did not already pass the flag.
    #[test]
    fn the_force_hint_appears_only_when_it_would_change_something() {
        let skipped = result_of(
            &["node_modules"],
            vec![CopyOutcome::Skipped {
                entry: "node_modules".into(),
                reason: SkipReason::DestinationExists,
            }],
        );
        assert!(
            render(&skipped, false).has_info("--force"),
            "an entry left alone must point at the way to replace it"
        );
        assert!(
            !render(&skipped, true).has_info("--force"),
            "…but not when --force was already passed"
        );

        // A skip for any other reason is not a --force case: forcing would
        // not have copied it either.
        let elsewhere = result_of(
            &["node_modules"],
            vec![CopyOutcome::Skipped {
                entry: "node_modules".into(),
                reason: SkipReason::NoSource,
            }],
        );
        assert!(!render(&elsewhere, false).has_info("--force"));
    }

    /// A declared entry the engine never reported is the silent failure this
    /// command must not have: the user would read a clean summary while a
    /// cache they asked for was never attempted.
    #[test]
    fn a_declared_entry_with_no_outcome_is_called_out() {
        let result = result_of(
            &["target", "node_modules", ".venv"],
            vec![copied("target", 1024)],
        );
        let output = render(&result, false);

        assert!(
            output.has_warning("node_modules: not reported"),
            "{:?}",
            output.warnings()
        );
        assert!(output.has_warning(".venv: not reported"));
        assert!(
            output.has_result("Copied 1 of 3 declared paths"),
            "the summary counts declarations, not outcomes: {:?}",
            output.results()
        );
    }

    /// Nothing declared is not a failure and not a copy — it is advice. No
    /// summary line, because there is nothing to summarize.
    #[test]
    fn declaring_nothing_explains_itself_instead_of_summarizing() {
        let result = result_of(&[], vec![]);
        let output = render(&result, false);

        assert!(
            output.has_info("No `copy:` paths declared in 'main'"),
            "the message must name the worktree whose config was read: {:?}",
            output.infos()
        );
        assert!(
            output.has_info("daft.yml"),
            "and say where to declare them: {:?}",
            output.infos()
        );
        assert!(
            output.results().is_empty(),
            "no work, no summary: {:?}",
            output.results()
        );
        assert!(!output.has_warnings());
    }

    // ── Registration ─────────────────────────────────────────────────────
    //
    // A new command has to be listed in a dozen places, and most of those
    // lists are only self-consistent: their tests check ordering, or that
    // every listed name resolves, which stays green when an entry is missing
    // from all of them at once. These assert *presence* on the surfaces where
    // a silent omission would leave `daft warm` half-wired.

    /// `daft warm` and `git daft worktree-warm` must both be suggestible. The
    /// existing list tests only check ordering and duplicates, so they pass
    /// just as happily with the entries gone.
    #[test]
    fn warm_is_suggestible_under_both_spellings() {
        assert!(crate::suggest::DAFT_SUBCOMMANDS.contains(&"warm"));
        assert!(crate::suggest::DAFT_SUBCOMMANDS.contains(&"worktree-warm"));
    }

    /// The completion registry: the command has to be generated at all, its
    /// clap definition has to be reachable, and it has to take the rich
    /// (worktree-name) path rather than the flags-only one.
    #[test]
    fn warm_is_wired_into_the_completion_registry() {
        use crate::commands::completions::{
            COMMANDS, VERB_ALIAS_GROUPS, get_command_for_name, uses_rich_completions,
        };

        assert!(
            COMMANDS.contains(&"git-worktree-warm"),
            "without this, no completion script is generated for warm at all"
        );
        assert!(
            get_command_for_name("git-worktree-warm").is_some(),
            "the generator resolves flags through this map"
        );
        assert!(
            uses_rich_completions("git-worktree-warm"),
            "both of warm's slots name a worktree, which is the rich path"
        );
        assert!(
            VERB_ALIAS_GROUPS
                .iter()
                .any(|(verbs, cmd)| verbs.contains(&"warm") && *cmd == "git-worktree-warm"),
            "the `daft warm` alias must map to the underlying command"
        );
    }

    /// The argument surface itself. Completions, the man page, the docs page
    /// and every `copy` YAML scenario spell these out; a rename that compiled
    /// would break all four silently.
    #[test]
    fn the_argument_surface_is_the_one_every_other_surface_spells_out() {
        use clap::CommandFactory;

        let cmd = Args::command();
        let longs: Vec<_> = cmd.get_arguments().filter_map(|a| a.get_long()).collect();
        assert!(longs.contains(&"from"), "{longs:?}");
        assert!(longs.contains(&"force"), "{longs:?}");
        assert!(longs.contains(&"verbose"), "{longs:?}");

        let positionals: Vec<_> = cmd.get_positionals().map(|a| a.get_id().as_str()).collect();
        assert_eq!(
            positionals,
            ["target"],
            "warm takes exactly one optional positional — the worktree to warm"
        );
        assert!(
            !cmd.get_positionals().any(clap::Arg::is_required_set),
            "the positional defaults to the current worktree, so it is optional"
        );
        assert_eq!(cmd.get_name(), "git-worktree-warm");
    }

    /// The copied line carries the annotation, not just the entry name —
    /// "how much, how, how long" is the answer a cache copy owes the user.
    #[test]
    fn a_copied_line_carries_its_annotation() {
        let result = result_of(
            &["node_modules"],
            vec![CopyOutcome::Copied {
                entry: "node_modules".into(),
                method: CopyMethod::Reflinked,
                matches: 3,
                bytes: 1024 * 1024,
                elapsed: Duration::from_millis(1500),
            }],
        );
        let output = render(&result, false);
        assert!(
            output
                .successes()
                .contains(&"node_modules \u{2192} 3 paths · 1.0 MB · reflinked · 1.5s"),
            "{:?}",
            output.successes()
        );
    }
}
