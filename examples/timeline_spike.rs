//! TTY spike for the plan-execute rail timeline (#651).
//!
//! Drives a fake `daft start` rail plus a REAL embedded hook phase through
//! the actual `CliPresenter::embedded` path — the succinct rail rows by
//! default, the threaded log under `hooks-verbose` — so the indicatif
//! splice mechanics can be verified on a live terminal.
//!
//! Run on a real TTY:
//! ```sh
//! cargo run --example timeline_spike                        # succinct hook rows
//! cargo run --example timeline_spike -- hooks-verbose       # threaded log
//! cargo run --example timeline_spike -- hooks-fail          # ✗ row + dump after footer
//! cargo run --example timeline_spike -- hooks-verbose-fail  # inline evidence + exit fact
//! cargo run --example timeline_spike -- fail                # mid-plan failure teardown
//! cargo run --example timeline_spike -- skip                # attention-skip row
//! ```
//! (`DAFT_TESTING` must be unset; a non-TTY stderr renders nothing.)

use daft::core::stage::{PlanCommit, Row, StageEvent, StageId, StepKey, StepSpec};
use daft::executor::cli_presenter::CliPresenter;
use daft::executor::presenter::JobPresenter;
use daft::output::timeline::{Timeline, TimelineMode, warning_line};
use daft::settings::HookOutputConfig;
use std::thread::sleep;
use std::time::Duration;

fn plan() -> PlanCommit {
    let mut rows = vec![
        Row::Step(StepSpec::new(StepKey::new(StageId::Fetch)).with_annotation("origin")),
        Row::Step(StepSpec::new(StepKey::new(StageId::Tracking))),
        Row::Step(StepSpec::new(StepKey::new(StageId::CreateBranch))),
        Row::Step(StepSpec::new(StepKey::new(StageId::CheckOut))),
        Row::Step(
            StepSpec::new(StepKey::new(StageId::CreateWorktree))
                .with_annotation("../daft-652/cool-feature"),
        ),
        Row::Step(StepSpec::new(StepKey::new(StageId::Carry))),
        Row::Step(StepSpec::new(StepKey::new(StageId::Push)).with_annotation("\u{2192} origin")),
    ];
    daft::core::shared::push_shared_section(
        &mut rows,
        &[
            ".env".to_string(),
            ".claude/settings.json".to_string(),
            "conf/dev.toml".to_string(),
        ],
    );
    rows.push(Row::Step(
        StepSpec::new(StepKey::new(StageId::PostCreateHooks)).with_annotation("2 jobs"),
    ));
    PlanCommit::new(rows).with_header_annotation("\u{2190} master")
}

fn run_step(tl: &mut Timeline, id: StageId, millis: u64, annotation: Option<&str>) {
    let key = StepKey::new(id);
    tl.on_stage(&key, StageEvent::Started);
    sleep(Duration::from_millis(millis));
    tl.on_stage(
        &key,
        StageEvent::Completed {
            annotation: annotation.map(str::to_string),
        },
    );
}

fn drive_hook_phase(tl: &Timeline, verbose: bool, fail_install: bool) {
    let config = HookOutputConfig {
        verbose,
        ..Default::default()
    };
    let presenter =
        CliPresenter::embedded(&config, tl.handle(), StepKey::new(StageId::PostCreateHooks));

    presenter.on_phase_start("worktree-post-create", Some("daft-652/cool-feature"));
    // A --skip-hooks exclusion: yellow row on the rail, skip line in the block.
    presenter.on_job_skipped(
        "lint-setup",
        "requested (--skip-hooks)",
        Duration::ZERO,
        false,
        None,
    );
    // Paragraph-long description: must truncate at the terminal edge, never
    // wrap and tear the region (the {wide_msg} contract).
    presenter.on_job_start(
        "bun-install",
        Some(
            "Install JS dependencies and write .direnv/deps_hash so direnv \
             skips its own redundant install when this worktree is later \
             entered. Without that marker the first shell entry re-runs the \
             whole dependency resolution a second time.",
        ),
        Some("bun install"),
    );
    presenter.on_job_start("prepare-db", None, Some("./scripts/prepare-db.sh"));
    for i in 0..12 {
        presenter.on_job_output("bun-install", &format!("resolving package cluster {i}"));
        if i % 3 == 0 {
            presenter.on_job_output("prepare-db", &format!("applying migration {}", i / 3));
        }
        sleep(Duration::from_millis(180));
    }
    presenter.on_job_success("prepare-db", Duration::from_millis(2100));
    sleep(Duration::from_millis(600));
    if fail_install {
        presenter.on_job_output("bun-install", "error: lockfile out of date");
        presenter.on_job_failure("bun-install", Duration::from_millis(2900));
        // The runner follows every failure with this line; the rail
        // suppresses it (the ✗ row + deferred dump carry the fact).
        presenter.on_message("Job 'bun-install' failed (exit code: 1)");
    } else {
        presenter.on_job_success("bun-install", Duration::from_millis(2900));
    }
    presenter.on_job_background("check-todos", Some("scan for TODOs"));
    presenter.on_phase_complete(Duration::from_millis(3000));
}

fn main() {
    let scenario = std::env::args().nth(1).unwrap_or_default();

    let mut tl = Timeline::new(
        TimelineMode::Interactive {
            color: daft::styles::colors_enabled_stderr(),
        },
        false,
        "Starting daft-652/cool-feature",
    );
    tl.commit_plan(plan());
    sleep(Duration::from_millis(900));

    run_step(&mut tl, StageId::Fetch, 700, None);
    run_step(&mut tl, StageId::Tracking, 250, None);
    // The three-way base selection resolved mid-plan; the branch row
    // records the ref it picked.
    tl.on_stage(
        &StepKey::new(StageId::CreateBranch),
        StageEvent::Note("\u{2190} origin/master".into()),
    );
    run_step(&mut tl, StageId::CreateBranch, 400, None);
    run_step(&mut tl, StageId::CheckOut, 300, None);

    if scenario == "fail" {
        let key = StepKey::new(StageId::CreateWorktree);
        tl.on_stage(&key, StageEvent::Started);
        sleep(Duration::from_millis(700));
        tl.on_stage(
            &key,
            StageEvent::Failed {
                detail: "destination exists and is not empty".into(),
            },
        );
        tl.abort("Failed after 1.4s");
        eprintln!("error: Failed to create git worktree: destination exists");
        return;
    }

    run_step(&mut tl, StageId::CreateWorktree, 600, None);

    // A no-op resolution removes its row from the rail (nothing to carry).
    sleep(Duration::from_millis(400));
    tl.on_stage(&StepKey::new(StageId::Carry), StageEvent::SkippedSilent);

    // A warning arriving mid-run must land above the live bars.
    tl.println_above(&warning_line("remote is 3 commits ahead of local master"));

    let push = StepKey::new(StageId::Push);
    tl.on_stage(&push, StageEvent::Started);
    sleep(Duration::from_millis(1300));
    tl.on_stage(
        &push,
        StageEvent::Completed {
            annotation: Some("\u{2192} origin/daft-652/cool-feature".into()),
        },
    );

    // The shared-files section: two links land, one declared path was never
    // collected — its row says so instead of vanishing.
    sleep(Duration::from_millis(350));
    tl.on_stage(
        &StepKey::scoped(StageId::SharedFile, ".env"),
        StageEvent::Completed { annotation: None },
    );
    sleep(Duration::from_millis(200));
    tl.on_stage(
        &StepKey::scoped(StageId::SharedFile, ".claude/settings.json"),
        StageEvent::Completed { annotation: None },
    );
    tl.on_stage(
        &StepKey::scoped(StageId::SharedFile, "conf/dev.toml"),
        StageEvent::SkippedAttention {
            reason:
                "'conf/dev.toml' missing from shared storage \u{2014} `daft shared sync` collects it"
                    .into(),
        },
    );

    let hooks_fail = matches!(scenario.as_str(), "hooks-fail" | "hooks-verbose-fail");
    if scenario == "skip" {
        tl.on_stage(
            &StepKey::new(StageId::PostCreateHooks),
            StageEvent::SkippedAttention {
                reason: "repo not trusted".into(),
            },
        );
    } else {
        drive_hook_phase(&tl, scenario.starts_with("hooks-verbose"), hooks_fail);
    }

    sleep(Duration::from_millis(300));
    // A failed hook job under failMode=warn does not fail the command; the
    // deferred failure detail prints below this footer.
    let footer = if hooks_fail {
        format!("Finished with failures in {}", tl.elapsed_display())
    } else {
        format!("Ready in {}", tl.elapsed_display())
    };
    tl.finish(&footer);
}
