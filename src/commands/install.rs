use anyhow::{Context, Result};
use clap::Parser;
use std::fs;
use std::path::Path;

use crate::output::{CliOutput, Output, OutputConfig};
use crate::utils::get_current_directory;

#[derive(Parser)]
#[command(name = "daft-install")]
#[command(version = crate::VERSION)]
#[command(about = "Install a starter daft.yml in the current worktree")]
#[command(long_about = r#"
Creates a starter daft.yml at the current worktree root with a commented
skeleton covering the major sections (hooks, shared, layout). Modeled on
`lefthook install`.

If daft.yml already exists, the command refuses without modifying anything;
edit the existing file with your editor or a future `daft config` TUI.

No git side effects: daft does not write to .gitignore or .git/info/exclude.
Ignore rules are the user's responsibility.
"#)]
pub struct Args {
    #[arg(short = 'q', long = "quiet", help = "Suppress progress reporting")]
    quiet: bool,

    #[arg(short = 'v', long = "verbose", help = "Show detailed progress")]
    verbose: bool,
}

const STARTER_TEMPLATE: &str = include_str!("install/starter.yml");

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("daft-install"));
    let config = OutputConfig::new(args.quiet, args.verbose);
    let mut output = CliOutput::new(config);
    run_with_output(&mut output)
}

pub fn run_with_output(output: &mut dyn Output) -> Result<()> {
    let cwd = get_current_directory()?;
    install_starter(&cwd, output)
}

pub fn install_starter(worktree_root: &Path, output: &mut dyn Output) -> Result<()> {
    let target = worktree_root.join("daft.yml");
    if target.exists() {
        anyhow::bail!(
            "daft.yml already exists at {}. Edit it directly with your editor.",
            target.display()
        );
    }
    fs::write(&target, STARTER_TEMPLATE)
        .with_context(|| format!("Failed to write {}", target.display()))?;

    output.result(&format!("Installed daft.yml at {}", target.display()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::TestOutput;
    use tempfile::tempdir;

    #[test]
    fn test_install_creates_starter_file() {
        let dir = tempdir().unwrap();
        let mut output = TestOutput::new();
        install_starter(dir.path(), &mut output).unwrap();
        assert!(dir.path().join("daft.yml").is_file());
    }

    #[test]
    fn test_install_refuses_if_already_exists() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("daft.yml"), "hooks: {}").unwrap();
        let mut output = TestOutput::new();
        let result = install_starter(dir.path(), &mut output);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }
}
