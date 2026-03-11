// These modules are public API for incremental development; suppress
// dead-code warnings until the rest of the runner consumes them.
#[allow(dead_code)]
pub mod env;
#[allow(dead_code)]
pub mod repo_gen;
#[allow(dead_code)]
pub mod runner;
#[allow(dead_code)]
pub mod schema;

use anyhow::Result;
use std::path::PathBuf;

pub fn run(
    _scenarios: Vec<PathBuf>,
    _no_interactive: bool,
    _step: Option<usize>,
    _loop_count: Option<usize>,
    _keep: bool,
    list: bool,
) -> Result<()> {
    if list {
        return list_scenarios();
    }
    anyhow::bail!("manual-test not yet implemented")
}

fn list_scenarios() -> Result<()> {
    anyhow::bail!("list not yet implemented")
}
