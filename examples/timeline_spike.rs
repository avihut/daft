//! TTY spike for the plan-execute rail timeline (#651).
//!
//! Drives a fake `daft start` rail plus a REAL embedded hook block (the
//! actual `CliPresenter::embedded` → `HookProgressRenderer` path, with
//! parallel jobs, descriptions, and rolling output tails) so the
//! indicatif splice mechanics can be verified on a live terminal before any
//! command migrates.
//!
//! Run on a real TTY:
//! ```sh
//! cargo run --example timeline_spike            # happy path with hooks
//! cargo run --example timeline_spike -- fail    # mid-plan failure teardown
//! cargo run --example timeline_spike -- skip    # attention-skip row
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

fn drive_hook_block(tl: &Timeline) {
    let config = HookOutputConfig::default();
    let presenter =
        CliPresenter::embedded(&config, tl.handle(), StepKey::new(StageId::PostCreateHooks));

    presenter.on_phase_start("worktree-post-create", Some("daft-652/cool-feature"));
    presenter.on_job_start(
        "bun-install",
        Some("Install JS dependencies"),
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
    presenter.on_job_success("bun-install", Duration::from_millis(2900));
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

    // The shared-files section: two links complete, one row (declared but
    // never collected) vanishes silently under its anchor.
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
        StageEvent::SkippedSilent,
    );

    if scenario == "skip" {
        tl.on_stage(
            &StepKey::new(StageId::PostCreateHooks),
            StageEvent::SkippedAttention {
                reason: "repo not trusted".into(),
            },
        );
    } else {
        drive_hook_block(&tl);
    }

    sleep(Duration::from_millis(300));
    let footer = format!("Ready in {}", tl.elapsed_display());
    tl.finish(&footer);
}
