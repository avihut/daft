//! CLI-layer concerns that wrap the multicall dispatch in `main.rs`.
//!
//! Owns the program's "effective argv" — the original `std::env::args()` after
//! top-level flags (currently just `-C <path>`) have been stripped off.
//! Subcommands and helpers read from [`argv()`] instead of [`std::env::args`]
//! so the strip is universally visible.
//!
//! Split: [`argv`] holds the pure parser (unit-tested in-module). This file is
//! the imperative shell — chdir, OnceLock install, process exit on bad `-C`.
//! Functional core, imperative shell.

use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Context, Result};

pub mod argv;

static ARGV: OnceLock<Vec<String>> = OnceLock::new();

/// Parse top-level flags off `raw`, apply their side effects (`-C` chdir), and
/// install the stripped argv into the process-wide [`ARGV`] slot.
///
/// Must be called exactly once, at the top of `main()`, before any other code
/// reads argv or relies on the working directory.
///
/// Exits with code 2 (clap usage-error convention) on `-C` paths that don't
/// exist or aren't directories, matching `git -C`'s terse error style.
pub fn install_and_apply(raw: Vec<String>) -> Result<()> {
    let parsed = argv::parse_top_level_cwd(&raw).map_err(|e| match e {
        argv::ParseError::MissingPathAfterC => {
            anyhow::anyhow!("daft: -C: option requires an argument")
        }
    })?;

    for path in &parsed.chdir_paths {
        if path.as_os_str().is_empty() {
            // git's `-C ""` semantic: no-op.
            continue;
        }
        apply_chdir(path);
    }

    ARGV.set(parsed.stripped)
        .map_err(|_| anyhow::anyhow!("cli::install_and_apply called more than once"))?;
    Ok(())
}

fn apply_chdir(path: &Path) {
    if !path.is_dir() {
        eprintln!("daft: -C: '{}': not a directory", path.display());
        std::process::exit(2);
    }
    if let Err(e) =
        std::env::set_current_dir(path).with_context(|| format!("daft: -C: '{}'", path.display()))
    {
        eprintln!("{e:#}");
        std::process::exit(2);
    }
}

/// The stripped argv as installed by [`install_and_apply`].
///
/// Panics if called before [`install_and_apply`] — that would mean a code path
/// is bypassing the install, which is a programming error.
pub fn argv() -> &'static [String] {
    ARGV.get()
        .expect("cli::install_and_apply must be called before cli::argv()")
}

/// Test-only: install a known argv vector, bypassing the parser.
///
/// Panics if the slot was already set in the same process. Each test must run
/// in a fresh process, so use `#[cfg(test)]` `serial_test::serial` and prefer
/// `#[test]` functions that don't share state with this slot. For tests that
/// truly need to exercise [`argv()`], the recommended pattern is to do so once
/// per integration-test binary rather than across unit tests.
#[cfg(test)]
pub fn install_for_tests(argv: Vec<String>) {
    ARGV.set(argv)
        .expect("ARGV already set in this test process");
}
