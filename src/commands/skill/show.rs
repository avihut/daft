//! `daft skill show` — print the embedded agent skill.
//!
//! Piped or redirected, the output is the raw SKILL.md bytes and nothing
//! else, so redirects compose: `daft skill show > <root>/daft-worktree-workflow/SKILL.md`
//! is the manual install path for agents daft has no flag for, and is
//! version-matched to the binary by construction (unlike a curl of master).
//!
//! Shown in a terminal, the same content is rendered with daft's markdown
//! skin and paged (like `daft release-notes`) so a human can read all ~860
//! lines comfortably. The TTY gate is what keeps the two audiences apart: a
//! machine consuming the pipe gets source, a human gets a readable page.

use anyhow::{Context, Result};
use clap::Parser;
use std::io::{ErrorKind, IsTerminal, Write};

#[derive(Parser, Debug)]
#[command(name = "git-daft-skill-show")]
#[command(version = crate::VERSION)]
#[command(about = "Print the embedded agent skill")]
#[command(long_about = r#"
Prints the agent skill embedded in this daft binary (the repository's
SKILL.md, skill name `daft-worktree-workflow`).

In a terminal the skill is rendered with daft's markdown styling and shown
through a pager; piped or redirected it is emitted raw, with no decoration
and no color, so it composes:

    daft skill show > <skills-root>/daft-worktree-workflow/SKILL.md

installs the skill manually for an agent whose skills directory daft does
not know, byte-identical to what `git daft skill install` would write. The
printed copy carries the `daft_version` frontmatter stamp of this binary,
so manual installs stay covered by the `git daft doctor` freshness check.

Pass --no-pager to print the rendered skill straight to the terminal
without a pager.
"#)]
pub struct Args {
    /// Print rendered output directly instead of through a pager.
    #[arg(long)]
    no_pager: bool,
}

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
    let args = Args::parse_from(argv);

    // Redirects and pipes MUST receive raw bytes: `daft skill show >
    // .../SKILL.md` is a documented install path that has to stay
    // byte-identical to the embedded skill, and an agent or `| head`
    // consuming the pipe wants source, not ANSI. Rendering + paging is a
    // convenience reserved for a human reading it in a terminal.
    if !std::io::stdout().is_terminal() {
        return write_raw(crate::skill::SKILL_MD);
    }

    let rendered = crate::output::markdown::render(crate::skill::SKILL_MD);
    if args.no_pager {
        write_raw(&rendered)
    } else {
        crate::output::pager::display_with_pager(&rendered);
        Ok(())
    }
}

/// Write `content` to stdout, treating a consumer closing the pipe early
/// (`| head`) as success rather than an error — matching the structured-emit
/// path's behavior.
fn write_raw(content: &str) -> Result<()> {
    let mut stdout = std::io::stdout();
    match stdout
        .write_all(content.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Err(e) if e.kind() == ErrorKind::BrokenPipe => Ok(()),
        other => other.context("could not write the skill to stdout"),
    }
}
