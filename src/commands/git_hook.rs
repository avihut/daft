//! `daft __hook <git-hook-name> [args…]` — what a shim calls.
//!
//! Internal, and named with the `__` prefix so it inherits the startup-task
//! suppression every hidden verb gets: no update check, no trust prune, no
//! background spawns on a path that fires once per commit.

use crate::executor::cli_presenter::CliPresenter;
use crate::hooks::git_stage::{GitStage, dispatch};
use crate::output::{CliOutput, OutputConfig};
use std::io::Read;

/// Run the stage named by `args[0]`, with `args[1..]` as git's arguments.
///
/// Never returns: the exit code is the hook's answer to git, and getting it
/// right is the entire contract. Exit 0 means "carry on".
pub fn run(args: &[String]) -> ! {
    let Some(name) = args.first() else {
        eprintln!("daft __hook: expected a git hook name");
        std::process::exit(2);
    };
    let Some(stage) = GitStage::from_git_filename(name) else {
        // Reachable only through a shim daft wrote for a stage this binary no
        // longer manages — an install/binary mismatch, not a user error. Say
        // so and get out of the way rather than blocking the operation.
        eprintln!(
            "daft __hook: '{name}' is not a stage daft manages; reinstall with `daft hooks install`"
        );
        std::process::exit(0);
    };

    // Capture the index before anything else runs `git`: `git_command_at`
    // scrubs `GIT_INDEX_FILE` so `-C` decides which repository is read, and
    // during `git commit -a` that variable is the only pointer to the index
    // actually being committed.
    let index_file = std::env::var_os("GIT_INDEX_FILE").map(std::path::PathBuf::from);

    // Drain stdin for the stages that get one. A process can only read its
    // stdin once, so this has to happen before any job could try.
    let stdin = stage.expects_stdin().then(read_stdin).flatten();

    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("daft __hook: cannot read the current directory: {e}");
            std::process::exit(0);
        }
    };

    let mut output = CliOutput::new(OutputConfig::default());
    let hooks_output = crate::core::settings::load_hooks_config()
        .map(|c| c.output)
        .unwrap_or_default();
    let presenter = CliPresenter::auto(&hooks_output);

    let payload = dispatch::StagePayload {
        argv: args[1..].to_vec(),
        stdin,
        index_file,
    };
    let run = match dispatch::run_git_stage(stage, &cwd, payload, presenter, &mut output, false) {
        Ok(run) => run,
        Err(e) => {
            // An infrastructure failure — not the gate's verdict. Report it
            // and let the operation proceed: daft failing to run a check is
            // not evidence the check would have failed.
            eprintln!("daft: could not run the {stage} hook: {e:#}");
            std::process::exit(0);
        }
    };

    // The fail mode is already resolved by the time this returns: a stage
    // configured `warn` reports its failure and comes back successful, so
    // there is nothing left to decide here.
    if run.success {
        std::process::exit(0);
    }
    // A failing gate exits with the job's own code so anything reading git's
    // exit status sees what actually happened.
    std::process::exit(run.exit_code.unwrap_or(1));
}

/// Read stdin to a string, or `None` when there is nothing readable.
fn read_stdin() -> Option<String> {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf).ok()?;
    (!buf.trim().is_empty()).then_some(buf)
}
