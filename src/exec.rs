use anyhow::Result;
use std::process::Command;

use crate::output::Output;

pub fn run_exec_commands(commands: &[String], output: &mut dyn Output) -> Result<()> {
    for cmd in commands {
        output.step(&format!("Executing: {cmd}"));
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
        let status = Command::new(&shell)
            .arg("-i")
            .arg("-c")
            .arg(cmd)
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
