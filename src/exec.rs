use anyhow::Result;

use crate::core::worktree::exec::{AliasCache, CommandSpec, build_command};
use crate::output::Output;

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
pub fn run_exec_commands(commands: &[String], output: &mut dyn Output) -> Result<()> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
    // One capture covers the whole `-x` sequence; the snapshot is
    // shared across commands so a heavy rc-file (oh-my-zsh, p10k) is
    // paid for at most once per invocation.
    let alias_cache = AliasCache::ensure(&shell, false);

    for cmd in commands {
        output.step(&format!("Executing: {cmd}"));
        let spec = CommandSpec::Shell(cmd.clone());
        let status = build_command(&spec, alias_cache.as_ref())
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()?;
        if !status.success() {
            let code = status.code().unwrap_or(1);
            anyhow::bail!("Command '{}' exited with status {}", cmd, code);
        }
    }
    Ok(())
}
