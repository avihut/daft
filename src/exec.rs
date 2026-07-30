use anyhow::Result;

use crate::core::stage::{Row, StageEvent, StageId, StepKey, StepSpec};
use crate::core::worktree::exec::{AliasCache, CommandSpec, build_command};
use crate::output::Output;
use crate::output::timeline::Timeline;

/// Run a sequence of `-x` shell commands as part of worktree creation
/// (`daft clone -x`, `daft init -x`, `daft checkout/go/start -x`).
///
/// Routes through the same `build_command` used by `daft exec` so user
/// shortcuts (aliases, shell functions) resolve consistently. With a
/// captured alias snapshot the spawned shell skips the rc-file load
/// entirely; without one — unsupported shell, capture timeout, or first
/// run before the snapshot exists — falls back to a rc-less `$SHELL -c`.
/// The earlier implementation used `$SHELL -i -c CMD` unconditionally,
/// which made these commands unusable for users whose rc files break in
/// non-interactive contexts.
///
/// This is the off-rail path: `Plain`, `Hidden`, quiet, redirected stdout,
/// and every journey that never commits a plan (navigating to an existing
/// worktree). Invocations that DO commit a plan run the same sequence as
/// planned rail rows instead — see [`run_on_rail`].
pub fn run_exec_commands(commands: &[String], output: &mut dyn Output) -> Result<()> {
    // Nothing to run, nothing to capture: `ensure` spawns a shell to reload
    // the user's rc files whenever its snapshot has aged out, and every
    // creation journey calls through here whether or not `-x` was given.
    if commands.is_empty() {
        return Ok(());
    }
    let alias_cache = capture_aliases();

    for cmd in commands {
        output.step(&format!("Executing: {cmd}"));
        match CommandOutcome::of(spawn(cmd, alias_cache.as_ref())) {
            CommandOutcome::Ran => {}
            CommandOutcome::Exited(code) => anyhow::bail!("{}", failure_message(cmd, code)),
            CommandOutcome::Unstartable(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Where a creation journey's `-x` sequence ran.
///
/// The two arms are exclusive by construction: [`run_on_rail`] returns
/// `OffRail` without running anything when the committed plan does not carry
/// the rows, and the caller then runs the sequence the legacy way, after its
/// record line, so off-rail output stays byte-identical (#812).
pub enum ExecTail {
    /// Nothing ran. Either there are no `-x` commands, or no plan committed
    /// with their rows (Plain/Hidden/quiet/redirected, or an early-exit
    /// navigation to an existing worktree). The caller runs the sequence
    /// through [`run_exec_commands`] after its record line.
    OffRail,
    /// Ran as planned rows on the live rail, carrying the outcome the caller
    /// must still propagate — after the cd target is written, as before.
    OnRail(Result<()>),
}

impl ExecTail {
    /// Whether a command that ran on the rail failed. Drives the receipt
    /// footer: the worktree exists either way, so this never means "Failed".
    pub fn failed(&self) -> bool {
        matches!(self, Self::OnRail(Err(_)))
    }
}

/// The plan rows for a `-x` sequence (#812).
///
/// One row per `-x` **occurrence**, in the order typed, labelled with the
/// command exactly as the user wrote it — the row's identity IS its subject,
/// so the [`StageId::ExecCommand`] tense table is a fallback only. Scoped by
/// index rather than by text so that two identical `-x` strings stay two
/// distinct rows; [`step_key`] is the single place that derivation lives, and
/// [`run_on_rail`] resolves its events through the same function, so the plan
/// and the receipt cannot key differently.
///
/// A group anchor appears only for a sequence of two or more: a lone command
/// is one self-describing row, and an `exec` header above it would cost a
/// line to say nothing. The section closes its span so any row planned after
/// it stays ungrouped — today it is always last (the commands run after the
/// post-create hooks, which is where the plan ends).
pub fn plan_section(commands: &[String]) -> Vec<Row> {
    if commands.is_empty() {
        return Vec::new();
    }
    let grouped = commands.len() > 1;
    let mut rows = Vec::with_capacity(commands.len() + 2 * usize::from(grouped));
    if grouped {
        rows.push(Row::Group {
            label: "exec".into(),
        });
    }
    for (index, cmd) in commands.iter().enumerate() {
        rows.push(Row::Step(
            StepSpec::new(step_key(index)).with_label(cmd.as_str()),
        ));
    }
    if grouped {
        rows.push(Row::EndGroup);
    }
    rows
}

/// The plan key for the `index`-th `-x` command. Scoped by position, never by
/// the command text — `-x 'make' -x 'make'` is two rows, not one.
fn step_key(index: usize) -> StepKey {
    StepKey::scoped(StageId::ExecCommand, index.to_string())
}

/// Run the `-x` sequence as planned rows on a live rail, or report that no
/// rail carries them.
///
/// The rows must already be in the committed plan (the command layer appends
/// [`plan_section`] to the core's plan via `TimelineBridge::with_plan_suffix`);
/// this asks the region for each one rather than assuming, so an invocation
/// that never committed a plan — the early-exit navigations, `Plain`/`Hidden`
/// mode — falls back to the legacy path instead of firing events at rows that
/// do not exist.
///
/// Each command owns the terminal for its whole run: `-x` inherits stdin,
/// stdout and stderr and may be interactive, so the region clears around it
/// exactly as clone's install step does. Nothing inside that window may touch
/// `Output` — `suspend` holds the lock a region write would need — which is
/// why this function takes no `Output` at all.
pub fn run_on_rail(commands: &[String], timeline: &mut Timeline) -> ExecTail {
    let Some(keys) = railed_keys(commands, timeline) else {
        return ExecTail::OffRail;
    };

    let alias_cache = capture_aliases();
    let handle = timeline.handle();
    drive(commands, &keys, timeline, |cmd| {
        handle.suspend_for_prompt(|| CommandOutcome::of(spawn(cmd, alias_cache.as_ref())))
    })
}

/// What one `-x` command did.
enum CommandOutcome {
    Ran,
    /// Ran and exited non-zero, with the status the shell reported.
    Exited(i32),
    /// Never started — the shell itself could not be spawned.
    Unstartable(std::io::Error),
}

impl CommandOutcome {
    fn of(spawned: std::io::Result<std::process::ExitStatus>) -> Self {
        match spawned {
            Ok(status) if status.success() => Self::Ran,
            Ok(status) => Self::Exited(status.code().unwrap_or(1)),
            Err(e) => Self::Unstartable(e),
        }
    }
}

/// The rail loop: resolve each planned row from what running its command did,
/// and stop the sequence at the first failure — as it always has.
///
/// Split from [`run_on_rail`] on the "run one command" seam so the loop's
/// bookkeeping (which row resolves how, where the sequence stops, what the
/// caller gets back) is testable without spawning a shell.
fn drive(
    commands: &[String],
    keys: &[StepKey],
    timeline: &mut Timeline,
    mut run: impl FnMut(&str) -> CommandOutcome,
) -> ExecTail {
    let mut outcome = Ok(());

    for (cmd, key) in commands.iter().zip(keys) {
        if outcome.is_err() {
            // Say so on the rows that never ran: `finish` drops unresolved
            // rows, and a command the user typed vanishing off the receipt is
            // the one thing the plan promised not to do.
            timeline.on_stage(
                key,
                StageEvent::SkippedAttention {
                    reason: "not run".into(),
                },
            );
            continue;
        }
        timeline.on_stage(key, StageEvent::Started);
        match run(cmd) {
            CommandOutcome::Ran => {
                timeline.on_stage(key, StageEvent::Completed { annotation: None });
            }
            CommandOutcome::Exited(code) => {
                timeline.on_stage(
                    key,
                    StageEvent::Failed {
                        detail: format!("exit {code}"),
                    },
                );
                outcome = Err(anyhow::anyhow!("{}", failure_message(cmd, code)));
            }
            CommandOutcome::Unstartable(e) => {
                timeline.on_stage(
                    key,
                    StageEvent::Failed {
                        detail: format!("{e}"),
                    },
                );
                outcome = Err(e.into());
            }
        }
    }
    ExecTail::OnRail(outcome)
}

/// The committed plan's key for every command, or `None` when the rail does
/// not carry the section — no commands, no live region, or a region still
/// wearing its planning face (an early-exit navigation never commits, and
/// `region_live` alone cannot tell the two apart).
///
/// All or nothing: a partially planned section would strand rows the footer
/// then drops, so the whole sequence falls back to the off-rail path.
fn railed_keys(commands: &[String], timeline: &Timeline) -> Option<Vec<StepKey>> {
    if commands.is_empty() {
        return None;
    }
    (0..commands.len())
        .map(|index| {
            let key = step_key(index);
            // `resolve_key` falls back to the unscoped key; these rows are
            // always scoped, so an exact match is the only acceptable answer.
            timeline
                .resolve_key(key.id, key.scope.as_deref())
                .filter(|found| *found == key)
        })
        .collect()
}

/// One capture covers a whole `-x` sequence; the snapshot is shared across
/// commands so a heavy rc-file (oh-my-zsh, p10k) is paid for at most once per
/// invocation.
fn capture_aliases() -> Option<AliasCache> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
    AliasCache::ensure(&shell, false)
}

/// Spawn one `-x` command with the terminal inherited, and wait. The single
/// spawn site for both paths, so the rail cannot drift from the legacy one in
/// how a command is built or what it can reach.
fn spawn(cmd: &str, alias_cache: Option<&AliasCache>) -> std::io::Result<std::process::ExitStatus> {
    let spec = CommandSpec::Shell(cmd.to_string());
    build_command(&spec, alias_cache)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
}

/// The error text for a command that exited non-zero — one wording for both
/// paths, unchanged from before the rail existed.
fn failure_message(cmd: &str, code: i32) -> String {
    format!("Command '{cmd}' exited with status {code}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmds(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn nothing_declared_plans_nothing() {
        assert!(plan_section(&[]).is_empty());
    }

    #[test]
    fn a_lone_command_plans_one_ungrouped_row() {
        let rows = plan_section(&cmds(&["npm install"]));
        assert_eq!(rows.len(), 1, "no anchor for a single self-describing row");
        let Row::Step(spec) = &rows[0] else {
            panic!("expected a step row");
        };
        assert_eq!(spec.key.id, StageId::ExecCommand);
        assert_eq!(spec.label.as_deref(), Some("npm install"));
    }

    #[test]
    fn a_sequence_plans_an_anchored_section_labelled_as_typed() {
        let rows = plan_section(&cmds(&["make", "npm install"]));
        assert!(matches!(&rows[0], Row::Group { label } if label == "exec"));
        let labels: Vec<_> = rows
            .iter()
            .filter_map(|r| match r {
                Row::Step(spec) => spec.label.as_deref(),
                _ => None,
            })
            .collect();
        assert_eq!(labels, ["make", "npm install"], "in the order typed");
        assert!(
            matches!(rows.last(), Some(Row::EndGroup)),
            "the section closes its span so following rows stay ungrouped"
        );
    }

    #[test]
    fn identical_commands_stay_distinct_rows() {
        let rows = plan_section(&cmds(&["make", "make"]));
        let keys: Vec<_> = rows
            .iter()
            .filter_map(|r| match r {
                Row::Step(spec) => Some(spec.key.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(keys.len(), 2);
        assert_ne!(keys[0], keys[1], "scoped by position, not by text");
        assert_eq!(keys[0], step_key(0));
        assert_eq!(keys[1], step_key(1));
    }

    #[test]
    fn planned_keys_are_the_keys_the_runner_resolves() {
        // The plan and the receipt derive their keys from one function; this
        // is the weld. A row planned under a key the runner never emits would
        // sit pending until `finish` dropped it.
        let rows = plan_section(&cmds(&["a", "b", "c"]));
        let planned: Vec<_> = rows
            .iter()
            .filter_map(|r| match r {
                Row::Step(spec) => Some(spec.key.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(planned, (0..3).map(step_key).collect::<Vec<_>>());
    }

    /// A committed rail carrying `commands`' section, plus the terminal it
    /// paints on — the same InMemoryTerm setup the timeline's own
    /// persisted-sequence tests use.
    fn railed(commands: &[String]) -> (Timeline, indicatif::InMemoryTerm) {
        let term = indicatif::InMemoryTerm::new(30, 100);
        let mut timeline = Timeline::new(
            crate::output::timeline::TimelineMode::Interactive { color: false },
            false,
            "Starting feat/x",
        );
        timeline.set_test_draw_target(indicatif::ProgressDrawTarget::term_like(Box::new(
            term.clone(),
        )));
        timeline.commit_plan(crate::core::stage::PlanCommit::new(plan_section(commands)));
        (timeline, term)
    }

    #[test]
    fn the_bridge_appends_the_section_the_runner_resolves() {
        // The weld. The command layer hands `plan_section` to the bridge, the
        // core commits its own plan knowing nothing about `-x`, and the rows
        // the runner then resolves must be exactly the rows that landed —
        // appended after the core's last row, which is where the commands
        // run. A key derived twice, or a suffix dropped on the floor, shows
        // up here and nowhere else.
        use crate::core::ProgressSink;
        use crate::core::stage::{PlanCommit, StepSpec};
        use crate::hooks::{HookExecutor, HooksConfig};

        let commands = cmds(&["make", "make"]);
        let term = indicatif::InMemoryTerm::new(30, 100);
        let mut output = crate::output::TestOutput::new();
        let mut timeline = Timeline::new(
            crate::output::timeline::TimelineMode::Interactive { color: false },
            false,
            "Starting feat/x",
        );
        timeline.set_test_draw_target(indicatif::ProgressDrawTarget::term_like(Box::new(
            term.clone(),
        )));
        let executor = HookExecutor::new(HooksConfig {
            enabled: false,
            ..HooksConfig::default()
        })
        .expect("create executor");
        {
            let mut bridge = crate::core::TimelineBridge::new(
                &mut output,
                &mut timeline,
                executor,
                crate::core::settings::HookOutputConfig::default(),
            )
            .with_plan_suffix(plan_section(&commands));
            // A creation core's plan ends at the post-create hooks.
            bridge.on_plan(PlanCommit::new(vec![Row::Step(StepSpec::new(
                StepKey::new(StageId::PostCreateHooks),
            ))]));
        }

        let keys = railed_keys(&commands, &timeline);
        assert_eq!(
            keys,
            Some(vec![step_key(0), step_key(1)]),
            "both rows landed, and identical commands stayed distinct"
        );

        // And they landed AFTER the core's last row — which is where the
        // commands run: post-create hooks first, then `-x`.
        let hooks = StepKey::new(StageId::PostCreateHooks);
        timeline.on_stage(&hooks, StageEvent::Completed { annotation: None });
        drive(&commands, &keys.unwrap(), &mut timeline, |_| {
            CommandOutcome::Ran
        });
        timeline.finish("Ready in 0.1s");
        assert_eq!(
            term.contents(),
            "\u{250c}  Starting feat/x\n\
             \u{2502}\n\
             \u{2713}  post-create hooks\n\
             \u{2502}\n\
             \u{251c}\u{2500} exec\n\
             \u{2502}  \u{2713}  make\n\
             \u{2502}  \u{2713}  make\n\
             \u{2502}\n\
             \u{2514}  Ready in 0.1s"
        );
    }

    #[test]
    fn every_command_resolves_its_own_row() {
        let commands = cmds(&["make", "npm install"]);
        let (mut timeline, term) = railed(&commands);
        let tail = drive(
            &commands,
            &railed_keys(&commands, &timeline).expect("the plan carries the section"),
            &mut timeline,
            |_| CommandOutcome::Ran,
        );
        assert!(!tail.failed());
        timeline.finish("Ready in 0.1s");
        assert_eq!(
            term.contents(),
            "\u{250c}  Starting feat/x\n\
             \u{2502}\n\
             \u{251c}\u{2500} exec\n\
             \u{2502}  \u{2713}  make\n\
             \u{2502}  \u{2713}  npm install\n\
             \u{2502}\n\
             \u{2514}  Ready in 0.1s"
        );
    }

    #[test]
    fn a_failure_stops_the_sequence_and_says_so_on_the_rows_that_never_ran() {
        let commands = cmds(&["make", "npm install", "cargo test"]);
        let (mut timeline, term) = railed(&commands);
        let keys = railed_keys(&commands, &timeline).expect("the plan carries the section");
        let tail = drive(&commands, &keys, &mut timeline, |cmd| match cmd {
            "make" => CommandOutcome::Ran,
            _ => CommandOutcome::Exited(2),
        });

        let ExecTail::OnRail(Err(e)) = tail else {
            panic!("a non-zero exit is the caller's error to propagate");
        };
        assert_eq!(
            e.to_string(),
            "Command 'npm install' exited with status 2",
            "the wording the off-rail path has always used"
        );

        // The receipt: what ran, what broke, and — on the yellow `↓` face —
        // what never got its turn.
        timeline.finish("Ready with failures in 0.1s");
        assert_eq!(
            term.contents(),
            "\u{250c}  Starting feat/x\n\
             \u{2502}\n\
             \u{251c}\u{2500} exec\n\
             \u{2502}  \u{2713}  make\n\
             \u{2502}  \u{2717}  npm install  exit 2\n\
             \u{2502}  \u{2193}  cargo test   not run\n\
             \u{2502}\n\
             \u{2514}  Ready with failures in 0.1s"
        );
    }

    #[test]
    fn a_lone_command_renders_without_an_anchor() {
        let commands = cmds(&["npm install"]);
        let (mut timeline, term) = railed(&commands);
        let keys = railed_keys(&commands, &timeline).expect("the plan carries the section");
        drive(&commands, &keys, &mut timeline, |_| CommandOutcome::Ran);
        timeline.finish("Ready in 0.1s");
        assert_eq!(
            term.contents(),
            "\u{250c}  Starting feat/x\n\
             \u{2502}\n\
             \u{2713}  npm install\n\
             \u{2502}\n\
             \u{2514}  Ready in 0.1s"
        );
    }

    #[test]
    fn an_unstartable_shell_fails_its_row_and_stops_the_sequence() {
        let commands = cmds(&["make", "npm install"]);
        let (mut timeline, _term) = railed(&commands);
        let keys = railed_keys(&commands, &timeline).expect("the plan carries the section");
        let tail = drive(&commands, &keys, &mut timeline, |_| {
            CommandOutcome::Unstartable(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no such file",
            ))
        });
        assert!(tail.failed(), "a shell that never started is a failure");
    }

    #[test]
    fn an_empty_sequence_never_claims_the_rail() {
        let mut timeline = Timeline::new(
            crate::output::timeline::TimelineMode::Interactive { color: false },
            false,
            "Opening feat/x",
        );
        timeline.commit_plan(crate::core::stage::PlanCommit::new(plan_section(&[])));
        assert!(matches!(run_on_rail(&[], &mut timeline), ExecTail::OffRail));
    }

    #[test]
    fn an_uncommitted_plan_falls_back_off_rail() {
        // The early-exit navigations open a planning face and never commit;
        // the region is live, but it carries no rows to resolve.
        let mut timeline = Timeline::new(
            crate::output::timeline::TimelineMode::Interactive { color: false },
            false,
            "Opening feat/x",
        );
        timeline.open_planning();
        assert!(timeline.region_live(), "the planning face is a live region");
        assert!(
            railed_keys(&cmds(&["true"]), &timeline).is_none(),
            "a face with no committed plan carries no exec rows"
        );
    }
}
