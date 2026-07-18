//! git-worktree-push - Push a branch with pre-push hooks run in its worktree
//!
//! Plain `git push` fires the shared `pre-push` hook with the cwd of
//! whatever worktree the push was invoked from — not the worktree that
//! owns the pushed branch. A hook that runs tests, lints the tree, or
//! reads worktree-local config therefore validates the wrong tree when
//! you push another worktree's branch. `daft push <branch>` resolves the
//! branch to its worktree first and runs the push from there; that
//! worktree-correct hook context is its entire reason to exist (#600).

use crate::{
    core::{
        repo::get_current_branch,
        stage::{PlanCommit, Row, StageEvent, StageId, StepKey, StepSpec},
        worktree::{
            branch_delete::display_path,
            ports::NoopStageRunner,
            push::{HookVerdict, PushAction, push_with_hooks},
        },
    },
    executor::{cli_presenter::CliPresenter, presenter::JobPresenter},
    git::GitCommand,
    is_git_repository,
    logging::init_logging,
    output::{
        CliOutput, Output, OutputConfig,
        timeline::{Timeline, TimelineMode},
    },
    settings::DaftSettings,
};
use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "git-worktree-push")]
#[command(version = crate::VERSION)]
#[command(about = "Push a branch, running pre-push hooks in its own worktree")]
#[command(long_about = r#"
Pushes a branch with the repository's shared pre-push hook running in
the pushed branch's own worktree.

Plain `git push` fires the shared pre-push hook with the working
directory of whatever worktree you invoked it from. A hook that runs
tests, lints the working tree, or reads worktree-local configuration
therefore silently validates the wrong tree when you push another
worktree's branch. This command resolves the branch to its worktree
first and runs the push from there — that is the only thing it adds
over `git push`.

The push targets the `daft.remote` remote (default: origin). A branch
with no upstream is pushed with `--set-upstream` so tracking gets
configured. A branch with no checked-out worktree is pushed from the
current directory, like plain `git push`.

Single-branch only: git fires pre-push once with one working directory,
so worktree-correct hook context is only well-defined for one branch.
"#)]
pub struct Args {
    /// Branch to push (default: the current worktree's branch)
    #[arg(value_name = "BRANCH")]
    pub branch: Option<String>,

    /// Skip the repo's pre-push hook (passes --no-verify to git push)
    #[arg(long, help = "Skip the repo's pre-push hook")]
    pub no_verify: bool,

    /// Use git push --force-with-lease
    #[arg(long, help = "Use git push --force-with-lease")]
    pub force_with_lease: bool,

    /// Be verbose; thread hook output under its rail row
    #[arg(short, long, help = "Be verbose; show detailed progress")]
    pub verbose: bool,

    /// Suppress non-essential output
    #[arg(short, long, help = "Suppress non-essential output")]
    pub quiet: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-push"));
    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;
    let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);
    let mut output = CliOutput::new(OutputConfig::new(args.quiet, args.verbose));

    run_push(&args, &settings, &git, &mut output)
}

/// The push action for one resolved branch: a branch that already tracks a
/// remote keeps a plain push; one without an upstream gets `--set-upstream`
/// so tracking is configured as a side effect (mirrors checkout's autopush).
/// `--force-with-lease` threads through either shape.
fn select_action<'a>(
    has_upstream: bool,
    remote: &'a str,
    branch: &'a str,
    force_with_lease: bool,
) -> PushAction<'a> {
    if has_upstream {
        PushAction::Sync {
            remote,
            branch,
            force_with_lease,
        }
    } else {
        PushAction::SetUpstream {
            remote,
            branch,
            force_with_lease,
        }
    }
}

fn run_push(
    args: &Args,
    settings: &DaftSettings,
    git: &GitCommand,
    output: &mut dyn Output,
) -> Result<()> {
    let branch = match &args.branch {
        Some(branch) => branch.clone(),
        None => get_current_branch()?,
    };
    let remote = settings.remote.clone();

    // The command's whole job: the pushed branch's worktree is the cwd the
    // shared pre-push hook must run in. `None` (no checked-out worktree) is
    // the ticket's explicit non-error: push by refname from the invoking
    // directory, exactly like plain `git push`.
    let worktree = git.find_worktree_for_branch(&branch)?;
    let invoking_dir =
        std::env::current_dir().context("Could not determine the current directory")?;
    let cwd: PathBuf = worktree.clone().unwrap_or_else(|| invoking_dir.clone());

    let has_upstream = git
        .get_branch_tracking_remote_from(&branch, &cwd)?
        .is_some();
    let verify = !args.no_verify;
    let hook_present = git.pre_push_hook_exists(&cwd);

    let hooks_config = crate::core::settings::load_hooks_config_with(git)?;
    let hook_output_config = hooks_config.output.with_cli_verbose(output.is_verbose());

    // Plan-execute rail (#651). The pre-push gate embeds under the active
    // Push row on the interactive rail; off the rail, keep the legacy #599
    // presentation (block renderer only when a hook could actually fire).
    let mut timeline = Timeline::new(
        TimelineMode::auto(output.is_quiet()),
        output.is_verbose(),
        format!("Pushing {branch}"),
    );
    let interactive = timeline.is_interactive();
    let presenter: Option<Arc<dyn JobPresenter>> = if interactive {
        Some(CliPresenter::embedded_for_stage(
            &hook_output_config,
            timeline.handle(),
            StageId::Push,
        ))
    } else if hook_present && verify {
        Some(CliPresenter::auto(&hook_output_config))
    } else {
        None
    };

    let resolve_key = StepKey::new(StageId::ResolveWorktree);
    let push_key = StepKey::new(StageId::Push);
    let mut rows = vec![Row::Step(StepSpec::new(resolve_key.clone()))];
    if args.no_verify && hook_present {
        rows.push(Row::Note {
            text: "pre-push hooks skipped \u{2014} requested (--no-verify)".to_string(),
        });
    }
    rows.push(Row::Step(
        StepSpec::new(push_key.clone()).with_annotation(format!("\u{2192} {remote}/{branch}")),
    ));
    timeline
        .commit_plan(PlanCommit::new(rows).with_header_annotation(format!("\u{2192} {remote}")));

    // Resolution already happened; the row records where the hook will run.
    match &worktree {
        Some(path) => {
            timeline.on_stage(
                &resolve_key,
                StageEvent::Completed {
                    annotation: Some(display_path(path)),
                },
            );
        }
        None => {
            timeline.on_stage(
                &resolve_key,
                StageEvent::SkippedAttention {
                    reason: "no worktree \u{2014} pushing from the current directory".to_string(),
                },
            );
        }
    }

    // Legacy parity for the plain/hidden modes (the rail no-ops there): the
    // resolved cwd is the command's story, so plain output states it too.
    if !interactive {
        match &worktree {
            Some(path) => output.info(&format!(
                "Pushing '{branch}' to '{remote}' from '{}'",
                path.display()
            )),
            None => output.info(&format!(
                "'{branch}' has no checked-out worktree \u{2014} pushing from the current directory"
            )),
        }
    }

    // Order is load-bearing: the Push row must be Active before the push so
    // the pre-push phase gate-embeds beneath it (begin_hook_embed).
    timeline.on_stage(&push_key, StageEvent::Started);
    let action = select_action(has_upstream, &remote, &branch, args.force_with_lease);
    let outcome = match push_with_hooks(
        git,
        action,
        &cwd,
        verify,
        &NoopStageRunner,
        presenter.as_ref(),
        Some(hook_present),
    ) {
        Ok(outcome) => outcome,
        Err(e) => {
            // Spawn-level failure (cancel, exec error): the push row never
            // resolved — abort persists it as not-run.
            timeline.abort(&format!("Failed after {}", timeline.elapsed_display()));
            return Err(e);
        }
    };

    match outcome.failure {
        None if outcome.up_to_date => {
            timeline.on_stage(
                &push_key,
                StageEvent::SkippedExpected {
                    reason: "already up to date".to_string(),
                },
            );
            if timeline.region_live() {
                timeline.finish(&format!("Done in {}", timeline.elapsed_display()));
            }
            if !timeline.replaces_stdout_record() {
                output.success(&format!("'{branch}' is already up to date on '{remote}'"));
            }
            Ok(())
        }
        None => {
            timeline.on_stage(&push_key, StageEvent::Completed { annotation: None });
            if timeline.region_live() {
                timeline.finish(&format!("Pushed in {}", timeline.elapsed_display()));
            }
            if !timeline.replaces_stdout_record() {
                output.success(&format!("Pushed '{branch}' to '{remote}'"));
            }
            Ok(())
        }
        Some(msg) => {
            // The rail detail must not blame the hook for a push it let
            // through (HookVerdict::failure_cause draws the same line).
            let detail = match outcome.hook {
                HookVerdict::Rejected => "pre-push gate refused (see below)",
                HookVerdict::Passed => "remote rejected (see below)",
                _ => "failed (see below)",
            };
            timeline.on_stage(
                &push_key,
                StageEvent::Failed {
                    detail: detail.to_string(),
                },
            );
            // Deferred hook output (the failure dump) drains below the
            // footer during teardown; the bail lands after the rail closes.
            timeline.abort(&format!("Failed after {}", timeline.elapsed_display()));
            let hint = if outcome.hook.no_verify_might_help() {
                " Re-run with --no-verify to bypass the hook."
            } else {
                ""
            };
            anyhow::bail!(
                "Could not push '{branch}' to '{remote}': {msg} ({}).{hint}",
                outcome.hook.failure_cause()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_branch_pushes_plain() {
        let action = select_action(true, "origin", "feat/x", false);
        assert!(matches!(
            action,
            PushAction::Sync {
                remote: "origin",
                branch: "feat/x",
                force_with_lease: false,
            }
        ));
    }

    #[test]
    fn upstreamless_branch_sets_upstream() {
        // The decided ergonomics (#600): no upstream → `--set-upstream`, so
        // tracking is configured as a side effect like checkout's autopush.
        let action = select_action(false, "origin", "feat/x", false);
        assert!(matches!(
            action,
            PushAction::SetUpstream {
                remote: "origin",
                branch: "feat/x",
                force_with_lease: false,
            }
        ));
    }

    #[test]
    fn force_with_lease_threads_through_both_shapes() {
        // A user's --force-with-lease must never be silently dropped —
        // including on the SetUpstream shape (the seam grew the field for
        // exactly this).
        assert!(matches!(
            select_action(true, "origin", "b", true),
            PushAction::Sync {
                force_with_lease: true,
                ..
            }
        ));
        assert!(matches!(
            select_action(false, "origin", "b", true),
            PushAction::SetUpstream {
                force_with_lease: true,
                ..
            }
        ));
    }
}
