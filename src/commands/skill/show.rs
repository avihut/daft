//! `daft skill show` — print the embedded agent skill to stdout.
//!
//! The output is the raw SKILL.md bytes and nothing else, so redirects
//! compose: `daft skill show > <skills-root>/daft-worktree-workflow/SKILL.md`
//! is the manual install path for agents daft has no flag for, and is
//! version-matched to the binary by construction (unlike a curl of master).

use anyhow::{Context, Result};
use clap::Parser;
use std::io::Write;

#[derive(Parser, Debug)]
#[command(name = "git-daft-skill-show")]
#[command(version = crate::VERSION)]
#[command(about = "Print the embedded agent skill to stdout")]
#[command(long_about = r#"
Prints the agent skill embedded in this daft binary (the repository's
SKILL.md, skill name `daft-worktree-workflow`) to stdout, with no
decoration and no color.

Use it to inspect exactly what `git daft skill install` would write, or to
install the skill manually for an agent whose skills directory daft does
not know:

    daft skill show > <skills-root>/daft-worktree-workflow/SKILL.md

The printed copy carries the `daft_version` frontmatter stamp of this
binary, so manual installs stay covered by the `git daft doctor` freshness
check.
"#)]
pub struct Args {}

pub fn run() -> Result<()> {
    let raw_args: Vec<String> = crate::cli::argv().to_vec();
    debug_assert!(
        raw_args.len() >= 3 && raw_args[1] == "skill" && raw_args[2] == "show",
        "skill::show::run() invoked with unexpected argv: {raw_args:?} \
         (expected `daft skill show ...`)"
    );
    let argv: Vec<String> = std::iter::once("git-daft-skill-show".to_string())
        .chain(raw_args.into_iter().skip(3))
        .collect();
    let _args = Args::parse_from(argv);

    // Raw dump; a consumer closing the pipe early (`| head`) is success,
    // not an error — matching the structured-emit path's behavior.
    let mut stdout = std::io::stdout();
    match stdout
        .write_all(crate::skill::SKILL_MD.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        other => other.context("could not write the skill to stdout"),
    }
}
