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
            push::{PushAction, push_with_hooks},
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

The push targets the branch's own upstream remote when it has one,
falling back to the `daft.remote` remote (default: origin) otherwise —
and a branch with no upstream is pushed with `--set-upstream` so
tracking gets configured. A branch with no checked-out worktree is
pushed from the current directory, like plain `git push`.

Only local branches can be pushed: tags and other refs are rejected
rather than handed to git as if they were branches.

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

/// Where the shared `pre-push` hook will run, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HookCwd {
    /// The pushed branch's own worktree — the command's entire point.
    Worktree(PathBuf),
    /// No worktree is checked out on the branch. Push it by refname from the
    /// invoking directory, exactly like plain `git push` — #600 calls this
    /// out as a non-error, which is why the seam's cwd is an `Option`.
    None,
    /// A worktree is registered for the branch but its directory is gone.
    /// `git worktree list` keeps reporting the entry (as `prunable`) until
    /// someone prunes it, and pushing from a path that no longer exists
    /// fails every git call with exit 128 — so fall back to the invoking
    /// directory rather than breaking a push plain git would have made.
    Stale(PathBuf),
}

/// Classify what `find_worktree_for_branch` recorded. The registered path is
/// only usable if it is still a directory; `push_single_worktree` guards the
/// batch path with the same `is_dir` check.
fn classify_worktree(recorded: Option<PathBuf>) -> HookCwd {
    match recorded {
        Some(path) if path.is_dir() => HookCwd::Worktree(path),
        Some(path) => HookCwd::Stale(path),
        None => HookCwd::None,
    }
}

/// The remote this push targets.
///
/// A branch's own upstream wins over `daft.remote`: `daft push` is plain
/// `git push` plus worktree-correct hook cwd, so it must not republish a
/// fork's branch to daft's default remote just because the branch happens to
/// live in a daft-managed repo. `daft.remote` is the fallback for a branch
/// that has no upstream yet — the case where git has no opinion either.
fn select_remote(tracking_remote: Option<&str>, configured: &str) -> String {
    tracking_remote.unwrap_or(configured).to_string()
}

/// The branch a `branch.<name>.merge` ref names, when it is not `<branch>`
/// itself. `feat` tracking `origin/main` makes the implicit `feat:feat`
/// refspec surprising — plain `git push` would refuse it under the default
/// `push.default=simple` — so the plan says out loud where the push lands.
fn divergent_upstream_branch(branch: &str, merge_ref: Option<&str>) -> Option<String> {
    merge_ref
        .and_then(|r| r.strip_prefix("refs/heads/"))
        .filter(|tracked| *tracked != branch)
        .map(str::to_string)
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

    // `daft push` pushes a *local branch* — that is what owns a worktree and
    // what the hook guarantee is defined over. Without this check any ref
    // that merely resolves goes straight to `git push`: `daft push v1.0.0`
    // publishes the *tag*, exits 0, and reports a successful branch push
    // while the worktree-correct hook context silently never applied.
    if !git.show_ref_exists(&format!("refs/heads/{branch}"))? {
        anyhow::bail!(
            "No local branch named '{branch}'.\n\
             `{}` pushes a local branch from its own worktree — to push a tag or any \
             other ref, use `git push {} {branch}`.",
            crate::daft_cmd("push"),
            settings.remote,
        );
    }

    // The command's whole job: the pushed branch's worktree is the cwd the
    // shared pre-push hook must run in.
    let hook_cwd = classify_worktree(git.find_worktree_for_branch(&branch)?);
    let invoking_dir =
        std::env::current_dir().context("Could not determine the current directory")?;
    let cwd: PathBuf = match &hook_cwd {
        HookCwd::Worktree(path) => path.clone(),
        HookCwd::None | HookCwd::Stale(_) => invoking_dir.clone(),
    };

    let tracking_remote = git.get_branch_tracking_remote_from(&branch, &cwd)?;
    let remote = select_remote(tracking_remote.as_deref(), &settings.remote);
    let has_upstream = tracking_remote.is_some();
    let divergent_upstream = divergent_upstream_branch(
        &branch,
        git.get_branch_merge_ref_from(&branch, &cwd)?.as_deref(),
    );
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
    if let Some(tracked) = &divergent_upstream {
        rows.push(Row::Note {
            text: format!(
                "'{branch}' tracks {remote}/{tracked} \u{2014} pushing to {remote}/{branch}"
            ),
        });
    }
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
    match &hook_cwd {
        HookCwd::Worktree(path) => {
            timeline.on_stage(
                &resolve_key,
                StageEvent::Completed {
                    annotation: Some(display_path(path)),
                },
            );
        }
        HookCwd::None => {
            timeline.on_stage(
                &resolve_key,
                StageEvent::SkippedAttention {
                    reason: "no worktree \u{2014} pushing from the current directory".to_string(),
                },
            );
        }
        HookCwd::Stale(path) => {
            timeline.on_stage(
                &resolve_key,
                StageEvent::SkippedAttention {
                    reason: format!(
                        "worktree {} is gone \u{2014} pushing from the current directory",
                        display_path(path)
                    ),
                },
            );
        }
    }

    // Legacy parity for the plain/hidden modes (the rail no-ops there): the
    // resolved cwd is the command's story, so plain output states it too.
    // Gated on the same predicate as the result lines below — keying this one
    // on interactivity alone drops it from a redirected-stdout run that still
    // records the push.
    if !timeline.replaces_stdout_record() {
        if let Some(tracked) = &divergent_upstream {
            output.info(&format!(
                "'{branch}' tracks '{remote}/{tracked}' \u{2014} pushing to '{remote}/{branch}'"
            ));
        }
        match &hook_cwd {
            HookCwd::Worktree(path) => output.info(&format!(
                "Pushing '{branch}' to '{remote}' from '{}'",
                path.display()
            )),
            HookCwd::None => output.info(&format!(
                "'{branch}' has no checked-out worktree \u{2014} pushing from the current directory"
            )),
            HookCwd::Stale(path) => output.info(&format!(
                "The worktree recorded for '{branch}' ({}) is gone \u{2014} pushing from the \
                 current directory. Run `{}` to clear the stale entry.",
                path.display(),
                crate::daft_cmd("prune")
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
            // Attribution is the shared verdict's call, not this call site's
            // (`rail_detail` and `failure_cause` draw the same line): daft
            // cannot tell a gate refusal from an unreachable remote, nor a
            // remote rejection from a lease git refused locally.
            timeline.on_stage(
                &push_key,
                StageEvent::Failed {
                    detail: outcome.hook.rail_detail().to_string(),
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
    fn a_registered_worktree_whose_directory_is_gone_is_stale() {
        // `git worktree list` keeps reporting an entry after someone deletes
        // the directory (it shows as `prunable`). Using that path as the cwd
        // fails every git call with exit 128 — a push plain git would have
        // made — so it must classify as Stale, not Worktree.
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("feat");
        std::fs::create_dir(&live).unwrap();
        let gone = dir.path().join("deleted");

        assert_eq!(
            classify_worktree(Some(live.clone())),
            HookCwd::Worktree(live)
        );
        assert_eq!(
            classify_worktree(Some(gone.clone())),
            HookCwd::Stale(gone),
            "a recorded-but-missing directory must not become the push cwd"
        );
        assert_eq!(classify_worktree(None), HookCwd::None);
    }

    #[test]
    fn the_branchs_own_upstream_outranks_daft_remote() {
        // A fork's branch tracking `upstream` must not be republished to
        // `daft.remote` just because daft defaults there; daft.remote is the
        // fallback for a branch git has no opinion about.
        assert_eq!(select_remote(Some("upstream"), "origin"), "upstream");
        assert_eq!(select_remote(None, "origin"), "origin");
    }

    #[test]
    fn a_differently_named_upstream_is_surfaced() {
        // `feat` tracking `origin/main` makes the implicit feat:feat refspec
        // surprising (plain `git push` refuses it under push.default=simple),
        // so the plan says where the push actually lands.
        assert_eq!(
            divergent_upstream_branch("feat", Some("refs/heads/main")),
            Some("main".to_string())
        );
        assert_eq!(
            divergent_upstream_branch("feat", Some("refs/heads/feat")),
            None,
            "the ordinary case is not worth a note"
        );
        assert_eq!(divergent_upstream_branch("feat", None), None);
        assert_eq!(
            divergent_upstream_branch("feat", Some("refs/tags/v1")),
            None,
            "only branch upstreams are compared by branch name"
        );
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
