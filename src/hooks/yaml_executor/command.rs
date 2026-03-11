use crate::hooks::environment::HookContext;
use crate::hooks::executor::HookResult;
use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

/// Build the full set of environment variables for a hook command by merging
/// the daft hook environment with any extra variables from the job definition.
fn build_env(extra_env: &HashMap<String, String>, ctx: &HookContext) -> HashMap<String, String> {
    let hook_env = crate::hooks::environment::HookEnvironment::from_context(ctx);
    let mut env = hook_env.vars().clone();
    env.extend(extra_env.iter().map(|(k, v)| (k.clone(), v.clone())));
    env
}

/// Convert a generic [`CommandResult`](crate::executor::command::CommandResult)
/// into a hooks-specific [`HookResult`].
fn command_result_to_hook_result(cr: crate::executor::command::CommandResult) -> HookResult {
    if cr.success {
        HookResult {
            success: true,
            exit_code: cr.exit_code,
            stdout: cr.stdout,
            stderr: cr.stderr,
            skipped: false,
            skip_reason: None,
            skip_ran_command: false,
            platform_skip: false,
        }
    } else {
        HookResult::failed(cr.exit_code.unwrap_or(-1), cr.stdout, cr.stderr)
    }
}

/// Run a shell command, capture its output, and optionally stream lines
/// through the provided channel.
///
/// Thin wrapper around [`crate::executor::command::run_command`] that builds
/// the hook environment and converts the result to [`HookResult`].
pub(crate) fn run_shell_command_with_callback(
    cmd: &str,
    extra_env: &HashMap<String, String>,
    working_dir: &Path,
    ctx: &HookContext,
    timeout: Duration,
    line_sender: Option<std::sync::mpsc::Sender<String>>,
) -> Result<HookResult> {
    let env = build_env(extra_env, ctx);
    let cr = crate::executor::command::run_command(cmd, &env, working_dir, timeout, line_sender)?;
    Ok(command_result_to_hook_result(cr))
}

/// Run a command with stdin/stdout inherited (for interactive jobs).
///
/// Thin wrapper around [`crate::executor::command::run_command_interactive`]
/// that builds the hook environment and converts the result to [`HookResult`].
pub(crate) fn run_interactive_command(
    cmd: &str,
    extra_env: &HashMap<String, String>,
    working_dir: &Path,
    ctx: &HookContext,
) -> Result<HookResult> {
    let env = build_env(extra_env, ctx);
    let cr = crate::executor::command::run_command_interactive(cmd, &env, working_dir)?;
    Ok(command_result_to_hook_result(cr))
}
