use crate::{
    WorktreeConfig,
    core::{
        OutputSink,
        copy_paths::{CopyOutcome, CopyPathsResult},
        worktree::warm,
    },
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
    // Not `core::repo::get_project_root`: that runs a bare `git rev-parse`,
    // which an inherited GIT_DIR retargets, while every worktree lookup below
    // goes through `git_command_at`, which clears it. Inside a git-driven hook
    // the two answer for different repositories. See `warm::current_worktree`.
    let project_root = warm::locate_project_root()?;

    let params = warm::WarmParams {
        target: args.target,
        from: args.from,
        force: args.force,
        remote: settings.remote.clone(),
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
    // A config that would not load is not a config that declares nothing, and
    // only one of the two is the user's fault to fix. Saying "declare some
    // paths" to someone whose `copy:` block is right there — but mistyped —
    // sends them to add a key that already exists.
    if let Some(detail) = &result.config_error {
        output.warning(&format!(
            "could not read the configuration in '{}': {detail}",
            result.source_name
        ));
        output.warning(&format!(
            "nothing was copied into '{}'; run `{}` to see what is wrong",
            result.target_name,
            crate::daft_cmd("hooks validate")
        ));
        return;
    }

    // `nothing_declared` alone would swallow outcomes the engine reported for
    // entries no longer in `declared` — a mismatch worth seeing, not hiding.
    if result.nothing_declared() && result.outcome.is_empty() {
        output.info(&format!(
            "No `copy:` paths declared in '{}' — nothing to copy into '{}'.",
            result.source_name, result.target_name
        ));
        output.info(&format!(
            "Declare the caches worth replicating in daft.yml, then run `{}`.",
            crate::daft_cmd("warm")
        ));
        return;
    }

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
                    output.warning(&warm::skip_line(entry, reason))
                }
                _ => output.info(&warm::skip_line(entry, reason)),
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

    output.result(&summary_line(result));

    if result.has_existing_skips() && !forced {
        output.info(&format!(
            "Entries already present were left alone; run `{}` to replace them.",
            crate::daft_cmd("warm --force")
        ));
    }
}

/// The one line a scripted caller greps for:
/// `Copied 1 of 2 declared paths (1.0 KB) into 'develop' from 'main'; 1 failed.`
///
/// Two properties it must never lose:
///
/// **It names both worktrees.** Every other mention of the resolved pair is
/// verbose-only, so at default verbosity — including a destructive `--force`
/// run — this is the user's only chance to notice that `wt-main` resolved by
/// directory name when they meant the worktree on branch `main`.
///
/// **It counts what went wrong.** Branching on "copied nothing" alone let a run
/// where every entry *failed* print `needed no work` on stdout while the
/// failures went to stderr — and under `--force`, "no work" would be describing
/// deleted caches. The no-work phrasing is reserved for the genuinely quiet
/// case; anything else is counted out loud.
fn summary_line(result: &warm::WarmResult) -> String {
    let outcome = &result.outcome;
    let declared = result.declared.len();
    let copied = outcome.copied_count();
    let failed = failure_count(result);
    let attention = attention_count(outcome);
    let pair = format!(
        " into '{}' from '{}'",
        result.target_name, result.source_name
    );

    let head = if copied > 0 {
        format!(
            "Copied {copied} of {declared} declared path{} ({}){pair}",
            plural(declared),
            warm::format_bytes(outcome.copied_bytes())
        )
    } else if failed == 0 && attention == 0 {
        return format!(
            "Copied nothing{pair}; {declared} declared path{} needed no work.",
            plural(declared)
        );
    } else {
        format!(
            "Copied 0 of {declared} declared path{}{pair}",
            plural(declared)
        )
    };

    let mut notes = Vec::new();
    if failed > 0 {
        notes.push(format!("{failed} failed"));
    }
    if attention > 0 {
        notes.push(format!("{attention} needed attention"));
    }
    if notes.is_empty() {
        format!("{head}.")
    } else {
        format!("{head}; {}.", notes.join(", "))
    }
}

/// Entries that were meant to be copied and were not, through no choice of
/// daft's: an engine error, or a declaration the engine never answered for at
/// all (which is a broken promise, and counts the same to the user).
fn failure_count(result: &warm::WarmResult) -> usize {
    result
        .outcome
        .outcomes
        .iter()
        .filter(|o| matches!(o, CopyOutcome::Failed { .. }))
        .count()
        + warm::unreported(&result.declared, &result.outcome).len()
}

/// Entries daft deliberately declined — tracked content, an unreflinkable
/// filesystem under `fallback: skip`, a cache over its `max_size`. Not
/// failures, but not "no work" either: the config asked for something that did
/// not happen.
fn attention_count(result: &CopyPathsResult) -> usize {
    use crate::core::copy_paths::SkipReason;
    result
        .outcomes
        .iter()
        .filter(|o| {
            matches!(
                o,
                CopyOutcome::Skipped {
                    reason: SkipReason::NotIgnored
                        | SkipReason::NoReflink
                        | SkipReason::TooLarge { .. },
                    ..
                }
            )
        })
        .count()
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

    fn failed(entry: &str) -> CopyOutcome {
        CopyOutcome::Failed {
            entry: entry.into(),
            detail: "permission denied".into(),
        }
    }

    fn skipped(entry: &str, reason: SkipReason) -> CopyOutcome {
        CopyOutcome::Skipped {
            entry: entry.into(),
            reason,
        }
    }

    #[test]
    fn summary_counts_copies_against_declarations() {
        let result = result_of(
            &["target", "node_modules"],
            vec![
                copied("target", 1024),
                skipped("node_modules", SkipReason::DestinationExists),
            ],
        );
        assert_eq!(
            summary_line(&result),
            "Copied 1 of 2 declared paths (1.0 KB) into 'develop' from 'main'."
        );
    }

    #[test]
    fn summary_stays_honest_when_nothing_was_copied() {
        // "Copied 0 of 3" reads like a failure; every one of those three may
        // simply have been already present. The no-work phrasing keeps a
        // clean re-run from looking broken — but it is reserved for runs where
        // nothing actually went wrong (see the failure tests below).
        let result = result_of(
            &["target", "node_modules", ".venv"],
            vec![
                skipped("target", SkipReason::NoSource),
                skipped("node_modules", SkipReason::DestinationExists),
                skipped(".venv", SkipReason::NoMatches),
            ],
        );
        assert_eq!(
            summary_line(&result),
            "Copied nothing into 'develop' from 'main'; 3 declared paths needed no work."
        );

        let lone = result_of(&["target"], vec![skipped("target", SkipReason::NoSource)]);
        assert_eq!(
            summary_line(&lone),
            "Copied nothing into 'develop' from 'main'; 1 declared path needed no work."
        );
    }

    #[test]
    fn summary_singularizes_a_lone_declaration() {
        let result = result_of(&["target"], vec![copied("target", 0)]);
        assert_eq!(
            summary_line(&result),
            "Copied 1 of 1 declared path (0 B) into 'develop' from 'main'."
        );
    }

    /// The pair is the whole point of this line at default verbosity: with
    /// path > branch > directory-name tiering, `warm wt-main` can resolve to a
    /// worktree the user did not mean, and under `--force` that is a wipe. The
    /// summary is the only unconditional mention of which two worktrees were
    /// involved, so it is asserted in every shape the line can take.
    #[test]
    fn every_summary_shape_names_both_worktrees() {
        let shapes = [
            result_of(&["target"], vec![copied("target", 10)]),
            result_of(&["target"], vec![skipped("target", SkipReason::NoSource)]),
            result_of(&["target"], vec![failed("target")]),
            result_of(&["target"], vec![skipped("target", SkipReason::NotIgnored)]),
            result_of(&["target"], vec![]),
        ];
        for shape in &shapes {
            let line = summary_line(shape);
            assert!(
                line.contains("'develop'") && line.contains("'main'"),
                "summary must name target and source: {line}"
            );
        }
    }

    /// The regression this whole branch exists for: every entry failed, so the
    /// per-entry warnings went to stderr and the summary — on stdout, where a
    /// script looks — said the run "needed no work". Under `--force` that
    /// sentence would be describing caches daft had just deleted.
    #[test]
    fn an_all_failed_run_is_never_reported_as_no_work() {
        let result = result_of(
            &["target", "node_modules", ".venv"],
            vec![failed("target"), failed("node_modules"), failed(".venv")],
        );
        let line = summary_line(&result);
        assert_eq!(
            line,
            "Copied 0 of 3 declared paths into 'develop' from 'main'; 3 failed."
        );
        assert!(!line.contains("needed no work"));
    }

    #[test]
    fn a_partial_run_counts_both_halves() {
        let result = result_of(
            &["target", "node_modules"],
            vec![copied("target", 1024), failed("node_modules")],
        );
        assert_eq!(
            summary_line(&result),
            "Copied 1 of 2 declared paths (1.0 KB) into 'develop' from 'main'; 1 failed."
        );
    }

    /// A deliberate refusal is not a failure and not "no work" either — the
    /// config asked for something that did not happen, and the summary has to
    /// keep those three states apart.
    #[test]
    fn attention_skips_are_counted_separately_from_failures() {
        let result = result_of(
            &["target", "node_modules", ".venv"],
            vec![
                skipped("target", SkipReason::NotIgnored),
                skipped(
                    "node_modules",
                    SkipReason::TooLarge {
                        size_bytes: 4096,
                        limit_bytes: 1024,
                    },
                ),
                failed(".venv"),
            ],
        );
        assert_eq!(
            summary_line(&result),
            "Copied 0 of 3 declared paths into 'develop' from 'main'; \
             1 failed, 2 needed attention."
        );
    }

    /// A declaration the engine never answered for is indistinguishable from a
    /// failure to the user — the cache they asked for is not there — so it is
    /// counted in the same column rather than silently dropping out of the
    /// arithmetic.
    #[test]
    fn an_unreported_declaration_counts_as_a_failure() {
        let result = result_of(&["target", "node_modules"], vec![copied("target", 2048)]);
        assert_eq!(
            summary_line(&result),
            "Copied 1 of 2 declared paths (2.0 KB) into 'develop' from 'main'; 1 failed."
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
            config_error: None,
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
            // Matched on the entry name alone, not on a `target: ` prefix:
            // self-identifying phrases drop the prefix (see
            // `a_self_identifying_phrase_is_not_prefixed_with_its_own_entry`).
            assert!(
                output.has_warning("target"),
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

    /// The lie this branch removes: a `copy:` block that would not parse used
    /// to render as "No `copy:` paths declared … Declare the caches worth
    /// replicating", telling a user whose block is right in front of them to
    /// go and write it. `daft warm` is the command reached for to debug
    /// exactly this, so it must say what failed and where to look.
    #[test]
    fn a_config_that_will_not_load_is_reported_instead_of_called_absent() {
        let mut result = result_of(&[], vec![]);
        result.config_error = Some("invalid type: string \"symlink\"".into());

        let output = render(&result, false);
        assert!(
            output.has_warning("could not read the configuration in 'main'"),
            "{:?}",
            output.entries()
        );
        assert!(
            output.has_warning("hooks validate"),
            "point at the command that explains it: {:?}",
            output.warnings()
        );
        assert!(
            !output.infos().iter().any(|i| i.contains("Declare the")),
            "a broken config must not be described as an empty one: {:?}",
            output.infos()
        );
        assert!(
            output.results().is_empty(),
            "nothing ran, so nothing is summarized: {:?}",
            output.results()
        );
    }

    /// The other half: a genuinely empty config still gets the hint that tells
    /// a new user what to do next.
    #[test]
    fn a_genuinely_absent_section_still_gets_the_declare_hint() {
        let output = render(&result_of(&[], vec![]), false);
        assert!(
            output.has_info("No `copy:` paths declared in 'main'"),
            "{:?}",
            output.infos()
        );
        assert!(
            output.infos().iter().any(|i| i.contains("Declare the")),
            "{:?}",
            output.infos()
        );
        // Even here, both ends are named — the user asked to warm something.
        assert!(
            output.infos().iter().any(|i| i.contains("'develop'")),
            "the target must be named even when nothing happened: {:?}",
            output.infos()
        );
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

    /// The rail renders `copy:` skips with no `skipped — ` prefix, so some
    /// phrases name the entry themselves to stay legible there. `warm` adds a
    /// prefix of its own, and prefixing a self-identifying phrase produced
    /// `node_modules: 'node_modules' is tracked — not copied`.
    ///
    /// Interim rule: a phrase opening with a quote is already qualified. The
    /// engine track is exporting an explicit classification to replace the
    /// sniff at merge; this test is what will prove the replacement kept the
    /// same output.
    #[test]
    fn a_self_identifying_phrase_is_not_prefixed_with_its_own_entry() {
        let result = result_of(
            &["node_modules"],
            vec![CopyOutcome::Skipped {
                entry: "node_modules".into(),
                reason: SkipReason::NotIgnored,
            }],
        );
        let output = render(&result, false);
        assert!(
            output
                .warnings()
                .contains(&"'node_modules' is tracked — not copied"),
            "the entry must appear exactly once: {:?}",
            output.warnings()
        );
        assert!(
            !output
                .warnings()
                .iter()
                .any(|w| w.starts_with("node_modules: '")),
            "stutter: {:?}",
            output.warnings()
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
