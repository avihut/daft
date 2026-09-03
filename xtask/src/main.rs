//! xtask - Development automation tasks for daft
//!
//! This binary provides development-time tasks that don't need to be
//! included in the distributed binary.

mod manual_test;
mod real_state_guard;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use clap_mangen::Man;
use std::fs;
use std::path::{Path, PathBuf};

/// Available daft commands that need man pages
const COMMANDS: &[&str] = &[
    "git-worktree-clone",
    "git-worktree-init",
    "git-worktree-checkout",
    "git-worktree-branch",
    "git-worktree-branch-delete",
    "git-worktree-prune",
    "git-worktree-carry",
    "git-worktree-fetch",
    "git-worktree-exec",
    "git-worktree-list",
    "git-worktree-merge",
    "git-worktree-sync",
    "git-worktree-push",
    "git-worktree-warm",
    "git-daft-repo-add",
    "git-daft-repo-info",
    "git-daft-repo-install",
    "git-daft-repo-link",
    "git-daft-repo-list",
    "git-daft-repo-move",
    "git-daft-repo-remove",
    "git-daft-repo-rename",
    "git-daft-repo-unlink",
    "git-daft-skill-install",
    "git-daft-skill-show",
    "git-daft-skill-uninstall",
    "daft-activate",
    "daft-config",
    "daft-doctor",
    "daft-env",
    "daft-file",
    "daft-hooks",
    "daft-install",
    "daft-layout",
    "daft-multi-remote",
    "daft-release-notes",
    "daft-run",
    "daft-shared",
    "daft-shell-init",
    "daft-shortcuts",
];

/// A daft verb command that maps to an existing git-worktree-* command for man page generation
struct DaftVerbEntry {
    /// The daft verb man page name, e.g., "daft-clone"
    daft_name: &'static str,
    /// The source git-worktree-* command name to derive the Command from
    source_command: &'static str,
    /// Optional override for the `about` text (None = use source command's about)
    about_override: Option<&'static str>,
}

/// Daft verb commands that need man pages derived from their git-worktree-* equivalents
const DAFT_VERBS: &[DaftVerbEntry] = &[
    DaftVerbEntry {
        daft_name: "daft-clone",
        source_command: "git-worktree-clone",
        about_override: None,
    },
    DaftVerbEntry {
        daft_name: "daft-init",
        source_command: "git-worktree-init",
        about_override: None,
    },
    DaftVerbEntry {
        daft_name: "daft-go",
        source_command: "git-worktree-checkout", // fallback only; dedicated GoArgs used via get_command_for_name
        about_override: None,
    },
    DaftVerbEntry {
        daft_name: "daft-start",
        source_command: "git-worktree-checkout", // fallback only; dedicated StartArgs used via get_command_for_name
        about_override: None,
    },
    DaftVerbEntry {
        daft_name: "daft-carry",
        source_command: "git-worktree-carry",
        about_override: None,
    },
    DaftVerbEntry {
        daft_name: "daft-update",
        source_command: "git-worktree-fetch",
        about_override: None,
    },
    DaftVerbEntry {
        daft_name: "daft-prune",
        source_command: "git-worktree-prune",
        about_override: None,
    },
    DaftVerbEntry {
        daft_name: "daft-rename",
        source_command: "git-worktree-branch",
        about_override: Some("Rename a branch and move its worktree"),
    },
    DaftVerbEntry {
        daft_name: "daft-sync",
        source_command: "git-worktree-sync",
        about_override: None,
    },
    DaftVerbEntry {
        daft_name: "daft-remove",
        source_command: "git-worktree-branch",
        about_override: None,
    },
    DaftVerbEntry {
        daft_name: "daft-list",
        source_command: "git-worktree-list",
        about_override: None,
    },
    DaftVerbEntry {
        daft_name: "daft-exec",
        source_command: "git-worktree-exec",
        about_override: None,
    },
    DaftVerbEntry {
        daft_name: "daft-merge",
        source_command: "git-worktree-merge",
        about_override: None,
    },
    DaftVerbEntry {
        daft_name: "daft-push",
        source_command: "git-worktree-push",
        about_override: None,
    },
    DaftVerbEntry {
        daft_name: "daft-warm",
        source_command: "git-worktree-warm",
        about_override: None,
    },
];

/// Get the clap Command for a given command name
fn get_command_for_name(command_name: &str) -> Option<clap::Command> {
    use clap::CommandFactory;
    match command_name {
        "git-worktree-clone" => Some(daft::commands::clone::Args::command()),
        "git-worktree-init" => Some(daft::commands::init::Args::command()),
        "git-worktree-checkout" => Some(daft::commands::checkout::Args::command()),
        "git-worktree-branch" => Some(daft::commands::worktree_branch::Args::command()),
        "git-worktree-branch-delete" => Some(daft::commands::branch_delete::Args::command()),
        "git-worktree-prune" => Some(daft::commands::prune::Args::command()),
        "git-worktree-carry" => Some(daft::commands::carry::Args::command()),
        "git-worktree-fetch" => Some(daft::commands::fetch::Args::command()),
        "git-worktree-exec" => Some(daft::commands::exec::Args::command()),
        "git-worktree-list" => Some(daft::commands::list::Args::command()),
        "git-worktree-merge" => Some(daft::commands::merge::Args::command()),
        "git-worktree-sync" => Some(daft::commands::sync::Args::command()),
        "git-worktree-push" => Some(daft::commands::push::Args::command()),
        "git-worktree-warm" => Some(daft::commands::warm::Args::command()),
        "git-daft-repo-add" => Some(daft::commands::repo::add::Args::command()),
        "git-daft-repo-info" => Some(daft::commands::repo::info::Args::command()),
        "git-daft-repo-install" => Some(daft::commands::repo::install::Args::command()),
        "git-daft-repo-link" => Some(daft::commands::repo::link::Args::command()),
        "git-daft-repo-list" => Some(daft::commands::repo::list::Args::command()),
        "git-daft-repo-move" => Some(daft::commands::repo::move_repo::Args::command()),
        "git-daft-repo-remove" => Some(daft::commands::repo::remove::Args::command()),
        "git-daft-repo-rename" => Some(daft::commands::repo::rename::Args::command()),
        "git-daft-repo-unlink" => Some(daft::commands::repo::unlink::Args::command()),
        "git-daft-skill-install" => Some(daft::commands::skill::install::Args::command()),
        "git-daft-skill-show" => Some(daft::commands::skill::show::Args::command()),
        "git-daft-skill-uninstall" => Some(daft::commands::skill::uninstall::Args::command()),
        "daft-config" => Some(daft::commands::config::ConfigArgs::command()),
        "daft-doctor" => Some(daft::commands::doctor::Args::command()),
        "daft-env" => Some(daft::commands::env::Args::command()),
        "daft-file" => Some(daft::commands::file::merge::Args::command()),
        "daft-layout" => Some(daft::commands::layout::LayoutArgs::command()),
        "daft-release-notes" => Some(daft::commands::release_notes::Args::command()),
        "daft-shared" => Some(daft::commands::shared::Args::command()),
        "daft-remove" => Some(daft::commands::worktree_branch::RemoveArgs::command()),
        "daft-rename" => Some(daft::commands::worktree_branch::RenameArgs::command()),
        "daft-go" => Some(daft::commands::checkout::GoArgs::command()),
        "daft-start" => Some(daft::commands::checkout::StartArgs::command()),
        "daft-hooks" => Some(daft::commands::hooks::Args::command()),
        "daft-install" => Some(daft::commands::install::Args::command()),
        "daft-run" => Some(daft::commands::run::Args::command()),
        "daft-multi-remote" => Some(daft::commands::multi_remote::Args::command()),
        "daft-activate" => Some(daft::commands::activate::Args::command()),
        "daft-shell-init" => Some(daft::commands::shell_init::Args::command()),
        "daft-shortcuts" => Some(daft::commands::shortcuts::Args::command()),
        _ => None,
    }
}

/// Map git-worktree-* commands to their daft verb equivalents for tip boxes in CLI docs
fn daft_verb_tip(command_name: &str) -> Option<&'static str> {
    match command_name {
        "git-worktree-clone" => Some(
            "::: tip\nThis command is also available as `daft clone`. See [daft clone](./daft-clone.md).\n:::\n",
        ),
        "git-worktree-init" => Some(
            "::: tip\nThis command is also available as `daft init`. See [daft init](./daft-init.md).\n:::\n",
        ),
        "git-worktree-checkout" => Some(
            "::: tip\nThis command is also available as `daft go` (existing branch) or `daft start`\n(new branch with `-b`). See [daft go](./daft-go.md) and\n[daft start](./daft-start.md).\n:::\n",
        ),
        "git-worktree-carry" => Some(
            "::: tip\nThis command is also available as `daft carry`. See [daft carry](./daft-carry.md).\n:::\n",
        ),
        "git-worktree-fetch" => Some(
            "::: tip\nThis command is also available as `daft update`. See [daft update](./daft-update.md).\n:::\n",
        ),
        "git-worktree-prune" => Some(
            "::: tip\nThis command is also available as `daft prune`. See [daft prune](./daft-prune.md).\n:::\n",
        ),
        "git-worktree-branch" => Some(
            "::: tip\nThis command is also available as `daft remove` (delete, use `-f` to force)\nor `daft rename` (rename with `-m`).\nSee [daft remove](./daft-remove.md) and [daft rename](./daft-rename.md).\n:::\n",
        ),
        "git-worktree-branch-delete" => Some(
            "::: warning\nThis command is deprecated. Use `git worktree-branch -d/-D` instead.\nSee [git worktree-branch](./git-worktree-branch.md).\n:::\n",
        ),
        "git-worktree-sync" => Some(
            "::: tip\nThis command is also available as `daft sync`. See [daft sync](./daft-sync.md).\n:::\n",
        ),
        "git-worktree-list" => Some(
            "::: tip\nThis command is also available as `daft list`. See [daft list](./daft-list.md).\n:::\n",
        ),
        "git-worktree-exec" => Some(
            "::: tip\nThis command is also available as `daft exec`. See [daft exec](./daft-exec.md).\n:::\n",
        ),
        "git-worktree-merge" => Some(
            "::: tip\nThis command is also available as `daft merge`. See [daft merge](./daft-merge.md).\n:::\n",
        ),
        "git-worktree-push" => Some(
            "::: tip\nThis command is also available as `daft push`. See [daft push](./daft-push.md).\n:::\n",
        ),
        "git-worktree-warm" => Some(
            "::: tip\nThis command is also available as `daft warm`. See [daft warm](./daft-warm.md).\n:::\n",
        ),
        _ => None,
    }
}

/// Command clusters for "See Also" links in CLI docs
fn related_commands(command_name: &str) -> Vec<&'static str> {
    match command_name {
        // Setup cluster
        "git-worktree-clone" => vec!["git-worktree-init", "git-worktree-checkout"],
        "git-worktree-init" => vec!["git-worktree-clone", "git-worktree-checkout"],
        // Branching cluster
        "git-worktree-checkout" => vec!["git-worktree-carry", "git-worktree-branch"],
        // Maintenance cluster
        "git-worktree-branch" => vec!["git-worktree-prune", "git-worktree-checkout"],
        "git-worktree-branch-delete" => vec![
            "git-worktree-branch",
            "git-worktree-prune",
            "git-worktree-checkout",
        ],
        "git-worktree-prune" => vec![
            "git-worktree-fetch",
            "git-worktree-sync",
            "git-worktree-branch",
        ],
        "git-worktree-fetch" => vec![
            "git-worktree-prune",
            "git-worktree-sync",
            "git-worktree-carry",
        ],
        "git-worktree-sync" => vec![
            "git-worktree-prune",
            "git-worktree-fetch",
            "git-worktree-push",
        ],
        "git-worktree-carry" => vec![
            "git-worktree-checkout",
            "git-worktree-fetch",
            "git-worktree-warm",
        ],
        // The materialize-into-a-worktree family: carry moves uncommitted
        // work, shared links config, warm copies caches. A reader who found
        // one of the three is usually looking for another.
        "git-worktree-warm" => vec!["git-worktree-carry", "daft-shared", "git-worktree-checkout"],
        "git-worktree-list" => vec![
            "git-worktree-checkout",
            "git-worktree-prune",
            "git-worktree-branch",
        ],
        "git-worktree-exec" => vec![
            "git-worktree-sync",
            "git-worktree-list",
            "git-worktree-carry",
        ],
        "git-worktree-merge" => vec![
            "git-worktree-list",
            "git-worktree-carry",
            "git-worktree-sync",
        ],
        "git-worktree-push" => vec!["git-worktree-sync", "git-worktree-checkout"],
        // Config cluster
        "daft-doctor" => vec!["git-worktree-clone", "git-worktree-init"],
        "daft-release-notes" => vec![],
        "daft-activate" => vec!["daft-shortcuts", "daft-shell-init"],
        "daft-shortcuts" => vec!["daft-activate", "daft-shell-init"],
        "daft-shell-init" => vec!["daft-activate", "daft-shortcuts"],
        _ => vec![],
    }
}

#[derive(Parser)]
#[command(name = "xtask")]
#[command(about = "Development automation tasks for daft")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate man pages for daft commands
    GenMan {
        /// Output directory for man pages
        #[arg(long, default_value = "man")]
        output_dir: PathBuf,

        /// Specific command to generate man page for (default: all commands)
        #[arg(long)]
        command: Option<String>,
    },

    /// Generate CLI reference markdown docs for daft commands
    GenCliDocs {
        /// Output directory for CLI docs
        #[arg(long, default_value = "docs/cli")]
        output_dir: PathBuf,

        /// Specific command to generate docs for (default: all commands)
        #[arg(long)]
        command: Option<String>,
    },

    /// Run manual test scenarios (automatic by default; use -i to step through)
    ManualTest {
        /// Scenario file(s) to run (default: all in tests/manual/scenarios/)
        #[arg(value_name = "SCENARIO")]
        scenarios: Vec<PathBuf>,

        /// Step through scenarios interactively (default is automatic, parallel-buffered run)
        #[arg(long, short = 'i')]
        interactive: bool,

        /// Increase verbosity (-v for per-check icons + captured output on pass,
        /// -vv for expanded commands + untruncated captured output).
        #[arg(short, long, action = clap::ArgAction::Count)]
        verbose: u8,

        /// Suppress per-step output; print only scenario PASS/FAIL plus the
        /// final summary (CI green-path, benches).
        #[arg(short, long, conflicts_with = "verbose")]
        quiet: bool,

        /// Jump to a specific step number (1-based)
        #[arg(long)]
        step: Option<usize>,

        /// Re-run a specific step N times (use with --step)
        #[arg(long, value_name = "N")]
        loop_count: Option<usize>,

        /// Keep test environment after completion (for debugging)
        #[arg(long)]
        keep: bool,

        /// Set up test environment only (no step execution)
        #[arg(long)]
        setup_only: bool,

        /// List available scenarios and exit
        #[arg(long)]
        list: bool,

        /// Show scenario summary without running anything
        #[arg(long)]
        show: bool,

        /// Include expectation checks in --show output
        #[arg(long, requires = "show")]
        checks: bool,

        /// Redundant — parallel execution is the default. Kept for back-compat
        /// with bench scripts; equivalent to passing no flag.
        #[arg(long)]
        parallel: bool,

        /// Cap on concurrent scenarios. Overrides --parallel and
        /// DAFT_MANUAL_TEST_JOBS. `--jobs 1` forces serial execution.
        /// Default: `available_parallelism()` (one worker per logical CPU).
        #[arg(long, short = 'j', value_name = "N")]
        jobs: Option<usize>,
    },

    /// Snapshot / verify the real daft state dirs — the test-isolation
    /// tripwire (#697). Fails if a suite run wrote the real config/state/data
    /// dirs (i.e. DAFT_*_DIR isolation was silently off, e.g. a non-dev-build
    /// binary under test).
    RealStateGuard {
        /// `snapshot` to record the fingerprint, `verify` to check it.
        #[arg(value_enum)]
        mode: real_state_guard::Mode,

        /// Fingerprint file to write (snapshot) or read (verify).
        file: PathBuf,
    },

    /// Rewrite the `daft_version:` frontmatter stamp in SKILL.md. Runs from
    /// the release.toml pre-release-hook so the stamp lands in the same
    /// release commit as the Cargo.toml bump, CHANGELOG, man pages, and CLI
    /// docs; a daft unit test pins the embedded stamp to the crate version.
    StampSkill {
        /// Version to stamp (the release hook passes `{{version}}`).
        #[arg(long)]
        version: String,

        /// The skill file to stamp.
        #[arg(long, default_value = "SKILL.md")]
        file: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::GenMan {
            output_dir,
            command,
        } => generate_man_pages(&output_dir, command.as_deref()),
        Commands::GenCliDocs {
            output_dir,
            command,
        } => generate_cli_docs(&output_dir, command.as_deref()),
        Commands::StampSkill { version, file } => stamp_skill(&file, &version),
        Commands::ManualTest {
            scenarios,
            interactive,
            verbose,
            quiet,
            step,
            loop_count,
            keep,
            setup_only,
            list,
            show,
            checks,
            parallel,
            jobs,
        } => {
            let (jobs, jobs_explicit) = resolve_jobs(jobs, parallel)?;
            let verbosity = manual_test::reporter::Verbosity::from_flags(verbose, quiet);
            manual_test::run(
                scenarios,
                interactive,
                verbosity,
                step,
                loop_count,
                keep,
                setup_only,
                list,
                show,
                checks,
                jobs,
                jobs_explicit,
            )
        }
        Commands::RealStateGuard { mode, file } => real_state_guard::run(mode, &file),
    }
}

/// Resolve the parallel-job count for the manual-test runner.
///
/// Returns `(jobs, explicit)` — the resolved worker count and whether the
/// caller asked for it specifically. The explicit bit lets the runner
/// distinguish "user explicitly wanted parallel" (worth bailing on if the
/// run is interactive) from "auto-default picked parallel" (silently fall
/// back to serial in interactive mode).
///
/// Precedence: `--jobs N` > `DAFT_MANUAL_TEST_JOBS` > `--parallel` >
/// auto-default. The first three are explicit; the auto-default is not.
/// The auto-default and `--parallel` both pick `default_cap()` =
/// `available_parallelism()`, so `--parallel` is now redundant — kept for
/// back-compat with bench scripts that still pass it.
fn resolve_jobs(jobs_flag: Option<usize>, parallel_flag: bool) -> Result<(usize, bool)> {
    if let Some(n) = jobs_flag {
        return Ok((n.max(1), true));
    }
    if let Ok(raw) = std::env::var("DAFT_MANUAL_TEST_JOBS") {
        let parsed: usize = raw
            .parse()
            .with_context(|| format!("DAFT_MANUAL_TEST_JOBS must be a usize, got {raw:?}"))?;
        return Ok((parsed.max(1), true));
    }
    if parallel_flag {
        return Ok((default_cap(), true));
    }
    Ok((default_cap(), false))
}

/// Default parallel-job count: one worker per logical CPU.
///
/// Empirically the sweet spot — on a 10-core machine, suite wall-clock
/// drops monotonically from `--jobs 1` (~7m30s) through `--jobs 4` (~2m10s)
/// to `--jobs 10` (~85-90s), then plateaus and gradually rises past that
/// as oversubscription costs dominate. Matching available_parallelism keeps
/// every core busy without scheduling thrash.
fn default_cap() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .max(1)
}

#[cfg(test)]
mod jobs_resolution_tests {
    use super::*;
    use std::sync::Mutex;

    // Serializes the env-touching tests below — cargo runs unit tests on
    // multiple threads by default and concurrent writes to the same env var
    // would race.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn resolve_jobs_flag_wins_over_env_and_parallel() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("DAFT_MANUAL_TEST_JOBS", "9");
        let (n, explicit) = resolve_jobs(Some(4), true).unwrap();
        std::env::remove_var("DAFT_MANUAL_TEST_JOBS");
        assert_eq!(n, 4);
        assert!(explicit);
    }

    #[test]
    fn resolve_jobs_env_wins_over_parallel_flag() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("DAFT_MANUAL_TEST_JOBS", "7");
        let (n, explicit) = resolve_jobs(None, true).unwrap();
        std::env::remove_var("DAFT_MANUAL_TEST_JOBS");
        assert_eq!(n, 7);
        assert!(explicit);
    }

    #[test]
    fn resolve_jobs_parallel_flag_uses_default_cap_and_is_explicit() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("DAFT_MANUAL_TEST_JOBS");
        let (n, explicit) = resolve_jobs(None, true).unwrap();
        assert_eq!(n, default_cap());
        assert!(n >= 1);
        assert!(explicit);
    }

    #[test]
    fn resolve_jobs_no_flags_picks_default_cap_implicitly() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("DAFT_MANUAL_TEST_JOBS");
        let (n, explicit) = resolve_jobs(None, false).unwrap();
        assert_eq!(n, default_cap());
        assert!(!explicit, "auto-default must report as implicit");
    }

    #[test]
    fn resolve_jobs_zero_coerced_to_one() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("DAFT_MANUAL_TEST_JOBS");
        let (n, explicit) = resolve_jobs(Some(0), false).unwrap();
        assert_eq!(n, 1);
        assert!(explicit, "--jobs 0 is still an explicit user request");
    }

    #[test]
    fn resolve_jobs_malformed_env_errors() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("DAFT_MANUAL_TEST_JOBS", "not-a-number");
        let r = resolve_jobs(None, false);
        std::env::remove_var("DAFT_MANUAL_TEST_JOBS");
        assert!(r.is_err());
    }

    #[test]
    fn default_cap_matches_available_parallelism() {
        let expected = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .max(1);
        assert_eq!(default_cap(), expected);
    }
}

/// Escape roff-sensitive characters. `clap_mangen` applies the same
/// treatment to text it renders, so post-inserted sections should match.
fn roff_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('-', "\\-")
}

/// Build a `.SH EXAMPLES` section from `(comment, command)` pairs and splice
/// it into a rendered man page just before `.SH SUBCOMMANDS`, following the
/// indented-command style already used elsewhere in the top-level page.
fn insert_examples_section(roff: &str, examples: &[(&str, &str)]) -> String {
    if examples.is_empty() {
        return roff.to_string();
    }

    let mut section = String::new();
    section.push_str(".SH EXAMPLES\n");
    for (i, (comment, cmd)) in examples.iter().enumerate() {
        if i > 0 {
            section.push_str(".PP\n");
        }
        section.push_str(&roff_escape(comment));
        section.push('\n');
        section.push_str(".PP\n");
        section.push('\t');
        section.push_str(&roff_escape(cmd));
        section.push('\n');
    }

    // Insert before `.SH SUBCOMMANDS` when present; otherwise append.
    if let Some(idx) = roff.find("\n.SH SUBCOMMANDS") {
        let (before, after) = roff.split_at(idx + 1);
        format!("{before}{section}{after}")
    } else {
        let mut out = roff.to_string();
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&section);
        out
    }
}

/// Top-level man-page examples. Rendered into `.SH EXAMPLES` by
/// [`insert_examples_section`], one block per `(comment, command)` pair.
const DAFT_MAN_EXAMPLES: &[(&str, &str)] = &[
    (
        "Clone a repository into a worktree-based layout:",
        "daft clone git@github.com:user/repo.git",
    ),
    (
        "Initialize a new repository in the worktree layout:",
        "daft init my-project",
    ),
    (
        "Create a new branch with its own worktree:",
        "daft start feature/login",
    ),
    (
        "Switch to an existing branch's worktree:",
        "daft go feature/login",
    ),
    ("List all worktrees with status info:", "daft list"),
    (
        "Update the current worktree from its remote:",
        "daft update",
    ),
    (
        "Sync every worktree with its remote (prune + update all):",
        "daft sync",
    ),
    (
        "Transfer uncommitted changes to another worktree:",
        "daft carry feature/login",
    ),
    (
        "Delete a branch and its worktree:",
        "daft remove feature/login",
    ),
    (
        "Rename a branch and move its worktree:",
        "daft rename feature/login feature/auth",
    ),
];

/// Build a top-level `daft` clap Command with all subcommands for man page generation.
fn build_top_level_command() -> clap::Command {
    use clap::CommandFactory;

    clap::Command::new("daft")
        .about("A Git extensions toolkit for worktree-based development")
        .long_about(
            "daft is a comprehensive Git extensions toolkit that enhances developer \
             workflows with powerful worktree management. It provides both git-style \
             subcommands (git worktree-clone, git worktree-checkout, ...) and short \
             verb aliases (daft clone, daft go, daft start, ...).\n\n\
             Run 'daft' with no arguments to see a categorized command overview.\n\n\
             To enable automatic cd into new worktrees, add the shell integration:\n\n\
             \teval \"$(daft shell-init zsh)\"\n\n\
             See daft-shell-init(1) for details.",
        )
        .subcommand_required(false)
        // Top-level `-C <path>` flag (issue #519). The flag is actually parsed
        // pre-clap in `daft::cli::install_and_apply`; declaring it here only
        // affects man-page generation and `--help` output so the global option
        // is documented in the standard OPTIONS section.
        .arg(
            clap::Arg::new("cwd")
                .short('C')
                .value_name("path")
                .global(false)
                .num_args(1)
                .help(
                    "Run as if started in <path> instead of the current working directory. \
                     Applied before subcommand dispatch, layout resolution, and hook discovery. \
                     Multiple `-C` flags compose, matching `git -C` semantics.",
                ),
        )
        // Setup commands
        .subcommand(daft::commands::clone::Args::command().name("clone"))
        .subcommand(daft::commands::init::Args::command().name("init"))
        // Branching commands
        .subcommand(daft::commands::checkout::GoArgs::command().name("go"))
        .subcommand(daft::commands::checkout::StartArgs::command().name("start"))
        // Sharing commands
        .subcommand(daft::commands::carry::Args::command().name("carry"))
        .subcommand(daft::commands::warm::Args::command().name("warm"))
        // Maintenance commands
        .subcommand(daft::commands::list::Args::command().name("list"))
        .subcommand(daft::commands::worktree_branch::RemoveArgs::command().name("remove"))
        .subcommand(daft::commands::worktree_branch::RenameArgs::command().name("rename"))
        .subcommand(daft::commands::prune::Args::command().name("prune"))
        .subcommand(daft::commands::fetch::Args::command().name("update"))
        .subcommand(daft::commands::sync::Args::command().name("sync"))
        // Configuration commands
        .subcommand(daft::commands::shared::Args::command().name("shared"))
        .subcommand(daft::commands::hooks::Args::command().name("hooks"))
        .subcommand(daft::commands::layout::LayoutArgs::command().name("layout"))
        .subcommand(daft::commands::multi_remote::Args::command().name("multi-remote"))
        .subcommand(daft::commands::config::ConfigArgs::command().name("config"))
        .subcommand(daft::commands::install::Args::command().name("install"))
        .subcommand(
            clap::Command::new("file")
                .about("Manage YAML config files")
                .subcommand(daft::commands::file::merge::Args::command().name("merge")),
        )
        .subcommand(daft::commands::doctor::Args::command().name("doctor"))
        .subcommand(daft::commands::shell_init::Args::command().name("shell-init"))
        .subcommand(daft::commands::activate::Args::command().name("activate"))
        .subcommand(daft::commands::shortcuts::Args::command().name("shortcuts"))
        .subcommand(daft::commands::release_notes::Args::command().name("release-notes"))
}

/// Generate man pages and write to a directory
fn generate_man_pages(output_dir: &PathBuf, command: Option<&str>) -> Result<()> {
    fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "Failed to create output directory: {}",
            output_dir.display()
        )
    })?;

    let commands_to_generate: Vec<&str> = if let Some(cmd) = command {
        // "daft" is handled separately as the top-level man page
        if cmd == "daft" {
            vec![]
        } else {
            vec![cmd]
        }
    } else {
        COMMANDS.to_vec()
    };

    for command_name in &commands_to_generate {
        let cmd = get_command_for_name(command_name)
            .with_context(|| format!("Unknown command: {command_name}"))?;

        let man = Man::new(cmd);
        let mut buffer = Vec::new();
        man.render(&mut buffer)?;

        let filename = format!("{command_name}.1");
        let file_path = output_dir.join(&filename);

        fs::write(&file_path, &buffer)
            .with_context(|| format!("Failed to write man page: {}", file_path.display()))?;

        eprintln!("Generated: {}", file_path.display());
    }

    // Generate man pages for daft verb commands
    let daft_verbs_to_generate: Vec<&DaftVerbEntry> = if let Some(cmd) = command {
        DAFT_VERBS.iter().filter(|v| v.daft_name == cmd).collect()
    } else {
        DAFT_VERBS.iter().collect()
    };

    for verb in &daft_verbs_to_generate {
        // If the verb has its own dedicated Args struct, use it directly;
        // otherwise derive from the source git-worktree-* command.
        let cmd = if let Some(direct) = get_command_for_name(verb.daft_name) {
            direct
        } else {
            let mut derived = get_command_for_name(verb.source_command)
                .with_context(|| format!("Unknown source command: {}", verb.source_command))?;
            derived = derived.name(verb.daft_name);
            if let Some(about) = verb.about_override {
                derived = derived.about(about);
            }
            derived
        };

        let man = Man::new(cmd);
        let mut buffer = Vec::new();
        man.render(&mut buffer)?;

        let filename = format!("{}.1", verb.daft_name);
        let file_path = output_dir.join(&filename);

        fs::write(&file_path, &buffer)
            .with_context(|| format!("Failed to write man page: {}", file_path.display()))?;

        eprintln!("Generated: {}", file_path.display());
    }

    // Generate top-level daft.1 man page
    let should_generate_top_level = match command {
        Some("daft") => true,
        Some(_) => false,
        None => true,
    };

    if should_generate_top_level {
        let cmd = build_top_level_command();
        let man = Man::new(cmd);
        let mut buffer = Vec::new();
        man.render(&mut buffer)?;

        let roff = String::from_utf8(buffer).context("Generated man page is not valid UTF-8")?;
        let roff = insert_examples_section(&roff, DAFT_MAN_EXAMPLES);

        let file_path = output_dir.join("daft.1");
        fs::write(&file_path, roff.as_bytes())
            .with_context(|| format!("Failed to write man page: {}", file_path.display()))?;

        eprintln!("Generated: {}", file_path.display());
    }

    eprintln!("\nMan pages generated in: {}", output_dir.display());
    Ok(())
}

/// Generate CLI reference markdown docs and write to a directory
fn generate_cli_docs(output_dir: &PathBuf, command: Option<&str>) -> Result<()> {
    fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "Failed to create output directory: {}",
            output_dir.display()
        )
    })?;

    let commands_to_generate: Vec<&str> = if let Some(cmd) = command {
        vec![cmd]
    } else {
        COMMANDS.to_vec()
    };

    for command_name in commands_to_generate {
        let cmd = get_command_for_name(command_name)
            .with_context(|| format!("Unknown command: {command_name}"))?;

        let markdown = render_command_markdown(command_name, &cmd);

        let filename = format!("{command_name}.md");
        let file_path = output_dir.join(&filename);

        fs::write(&file_path, &markdown)
            .with_context(|| format!("Failed to write CLI doc: {}", file_path.display()))?;

        eprintln!("Generated: {}", file_path.display());
    }

    eprintln!("\nCLI docs generated in: {}", output_dir.display());
    Ok(())
}

/// Render a clap Command to a markdown CLI reference page.
fn render_command_markdown(command_name: &str, cmd: &clap::Command) -> String {
    let mut md = String::new();

    let about = cmd.get_about().map(|s| s.to_string()).unwrap_or_default();

    let long_about = cmd
        .get_long_about()
        .map(|s| s.to_string())
        .unwrap_or_default();

    // Display name: convert "git-worktree-clone" → "git worktree-clone" for git commands,
    // "daft-doctor" → "daft doctor" for daft commands
    let display_name = if let Some(suffix) = command_name.strip_prefix("git-") {
        format!("git {suffix}")
    } else if let Some(suffix) = command_name.strip_prefix("daft-") {
        format!("daft {suffix}")
    } else {
        command_name.to_string()
    };

    // Frontmatter
    md.push_str("---\n");
    md.push_str(&format!("title: {command_name}\n"));
    md.push_str(&format!("description: {about}\n"));
    md.push_str("---\n\n");

    // Title
    md.push_str(&format!("# {display_name}\n\n"));
    md.push_str(&format!("{about}\n\n"));

    // Daft verb tip box (for git-worktree-* commands)
    if let Some(tip) = daft_verb_tip(command_name) {
        md.push_str(tip);
        md.push('\n');
    }

    // Description
    let description = long_about.trim();
    if !description.is_empty() {
        md.push_str("## Description\n\n");
        md.push_str(description);
        md.push_str("\n\n");
    }

    // Usage line
    md.push_str("## Usage\n\n");
    md.push_str("```\n");
    md.push_str(&build_usage_string(command_name, cmd, &display_name));
    md.push_str("\n```\n\n");

    // Positional arguments
    let args_table = render_arguments_table(cmd.get_arguments(), "## Arguments");
    md.push_str(&args_table);

    // Options (non-positional arguments)
    let opts_table = render_options_table(cmd.get_arguments(), "## Options");
    md.push_str(&opts_table);

    // Subcommands
    let subcommands: Vec<_> = cmd
        .get_subcommands()
        .filter(|sub| sub.get_name() != "help")
        .collect();

    if !subcommands.is_empty() {
        md.push_str("## Subcommands\n\n");

        for sub in &subcommands {
            let sub_name = sub.get_name();
            let sub_about = sub.get_about().map(|s| s.to_string()).unwrap_or_default();

            md.push_str(&format!("### {sub_name}\n\n"));
            if !sub_about.is_empty() {
                md.push_str(&format!("{sub_about}\n\n"));
            }

            // Render long_about for subcommands (matches top-level behavior)
            let sub_long_about = sub
                .get_long_about()
                .map(|s| s.to_string())
                .unwrap_or_default();
            let sub_description = sub_long_about.trim();
            if !sub_description.is_empty() {
                md.push_str(&format!("{sub_description}\n\n"));
            }

            // Usage line for subcommand
            md.push_str("```\n");
            md.push_str(&build_usage_string(
                command_name,
                sub,
                &format!("{display_name} {sub_name}"),
            ));
            md.push_str("\n```\n\n");

            // Subcommand positional arguments
            let sub_args = render_arguments_table(sub.get_arguments(), "#### Arguments");
            md.push_str(&sub_args);

            // Subcommand options
            let sub_opts = render_options_table(sub.get_arguments(), "#### Options");
            md.push_str(&sub_opts);
        }
    }

    // Global options
    md.push_str("## Global Options\n\n");
    md.push_str("| Option | Description |\n");
    md.push_str("|--------|-------------|\n");
    md.push_str("| `-h`, `--help` | Print help information |\n");
    md.push_str("| `-V`, `--version` | Print version information |\n");
    md.push('\n');

    // Structured Output section for emit-enabled commands
    if let Some(section) = structured_output_section(command_name) {
        md.push_str(&section);
    }

    // See Also
    let related = related_commands(command_name);
    if !related.is_empty() {
        md.push_str("## See Also\n\n");
        for related_cmd in &related {
            md.push_str(&format!("- [{related_cmd}](./{related_cmd}.md)\n"));
        }
        md.push('\n');
    }

    md
}

/// Returns the per-command "Structured Output" section for emit-enabled
/// commands. Body text is hand-curated to highlight realistic pipelines;
/// updates should track the support matrix in `daft::output::emit::dispatch`.
fn structured_output_section(command_name: &str) -> Option<String> {
    let body = match command_name {
        "git-worktree-list" => {
            "`git worktree-list` supports machine-readable output via `--format`: `json`,\n\
             `ndjson`, `tsv`, `csv`, `yaml`, `toon`, `markdown`, plus `--template <tera>`\n\
             for custom output.\n\n\
             ```sh\n\
             # Two columns for awk / cut\n\
             daft list --format tsv --no-headers | cut -f2,5\n\n\
             # Pipe to jq\n\
             daft list --format json | jq '.[] | select(.is_current == true)'\n\n\
             # Custom template\n\
             daft list --template '{% for r in items %}{{ r.name }} -> {{ r.path }}\n\
             {% endfor %}'\n\
             ```\n"
        }
        "daft-release-notes" => {
            "`daft release-notes` supports machine-readable output via `--format`: `json`,\n\
             `yaml`, `toon`, `markdown`, plus `--template <tera>` for custom output.\n\n\
             ```sh\n\
             # Markdown prose, paste-ready for GitHub release\n\
             daft release-notes 1.2.0 --format markdown\n\n\
             # Versions as JSON for tooling\n\
             daft release-notes --format json | jq '.[0].version'\n\
             ```\n"
        }
        "daft-hooks" => {
            "`daft hooks trust list` and `daft hooks run` (listing mode) support\n\
             machine-readable output via `--format`, plus `--template <tera>` for custom\n\
             output.\n\n\
             `hooks trust list` supports: `json`, `ndjson`, `tsv`, `csv`, `yaml`, `toon`, `markdown`.\n\n\
             `hooks run` (listing mode) supports: `json`, `yaml`, `toon`, `markdown`.\n\n\
             ```sh\n\
             # List trusted repos as TSV for scripting\n\
             daft hooks trust list --format tsv\n\n\
             # List hook run results as JSON\n\
             daft hooks run --format json\n\
             ```\n"
        }
        "daft-layout" => {
            "`daft layout list` supports machine-readable output via `--format`: `json`,\n\
             `ndjson`, `tsv`, `csv`, `yaml`, `toon`, `markdown`, plus `--template <tera>`\n\
             for custom output.\n\n\
             ```sh\n\
             # Layout names and templates as TSV\n\
             daft layout list --format tsv | cut -f1,3\n\n\
             # List layouts as JSON for tooling\n\
             daft layout list --format json\n\
             ```\n"
        }
        "daft-multi-remote" => {
            "`daft multi-remote status` supports machine-readable output via `--format`:\n\
             `json`, `yaml`, `toon`, `markdown`, plus `--template <tera>` for custom output.\n\n\
             ```sh\n\
             # Multi-remote configuration as YAML\n\
             daft multi-remote status --format yaml\n\
             ```\n"
        }
        "daft-shared" => {
            "`daft shared status` supports machine-readable output via `--format`: `json`,\n\
             `ndjson`, `tsv`, `csv`, `yaml`, `toon`, `markdown`, plus `--template <tera>`\n\
             for custom output.\n\n\
             ```sh\n\
             # Shared file state as TSV (long-form: one row per file per worktree)\n\
             daft shared status --format tsv\n\n\
             # Wide pivot table in markdown for quick visual reading\n\
             daft shared status --format markdown\n\
             ```\n"
        }
        _ => return None,
    };
    let mut section = String::from("## Structured Output\n\n");
    section.push_str(body);
    section.push_str("\nSee the [Output Formats guide](/reference/output-formats) for format details\nand Tera syntax.\n\n");
    Some(section)
}

/// Render a markdown table of positional arguments.
///
/// Returns an empty string if there are no positional arguments (excluding help/version).
fn render_arguments_table<'a>(args: impl Iterator<Item = &'a clap::Arg>, heading: &str) -> String {
    let positionals: Vec<_> = args
        .filter(|a| a.is_positional() && a.get_id() != "version" && a.get_id() != "help")
        .collect();

    if positionals.is_empty() {
        return String::new();
    }

    let mut md = String::new();
    md.push_str(&format!("{heading}\n\n"));
    md.push_str("| Argument | Description | Required |\n");
    md.push_str("|----------|-------------|----------|\n");

    for arg in &positionals {
        let id = arg.get_id().as_str();
        let value_name = arg
            .get_value_names()
            .and_then(|v| v.first().map(|s| s.to_string()))
            .unwrap_or_else(|| id.to_uppercase());

        let help = arg.get_help().map(|s| s.to_string()).unwrap_or_default();
        let required = if arg.is_required_set() { "Yes" } else { "No" };

        md.push_str(&format!("| `<{value_name}>` | {help} | {required} |\n"));
    }
    md.push('\n');

    md
}

/// Render a markdown table of non-positional options.
///
/// Returns an empty string if there are no options (excluding help/version).
fn render_options_table<'a>(args: impl Iterator<Item = &'a clap::Arg>, heading: &str) -> String {
    let options: Vec<_> = args
        .filter(|a| !a.is_positional() && a.get_id() != "version" && a.get_id() != "help")
        .collect();

    if options.is_empty() {
        return String::new();
    }

    let mut md = String::new();
    md.push_str(&format!("{heading}\n\n"));
    md.push_str("| Option | Description | Default |\n");
    md.push_str("|--------|-------------|----------|\n");

    for arg in &options {
        let mut opt_str = String::new();
        if let Some(short) = arg.get_short() {
            opt_str.push_str(&format!("-{short}"));
        }
        if let Some(long) = arg.get_long() {
            if !opt_str.is_empty() {
                opt_str.push_str(", ");
            }
            opt_str.push_str(&format!("--{long}"));
        }

        // Add value name if the option takes a value (skip for boolean flags)
        let is_bool_flag = matches!(
            arg.get_action(),
            clap::ArgAction::SetTrue | clap::ArgAction::SetFalse | clap::ArgAction::Count
        );
        if !is_bool_flag {
            if let Some(value_names) = arg.get_value_names() {
                if !value_names.is_empty() {
                    let name = &value_names[0];
                    opt_str.push_str(&format!(" <{name}>"));
                }
            }
        }

        let help = arg.get_help().map(|s| s.to_string()).unwrap_or_default();

        let defaults: Vec<_> = arg
            .get_default_values()
            .iter()
            .map(|v| v.to_string_lossy().to_string())
            .collect();
        let default_str = if defaults.is_empty() {
            String::new()
        } else {
            format!("`{}`", defaults.join(", "))
        };

        md.push_str(&format!("| `{opt_str}` | {help} | {default_str} |\n"));
    }
    md.push('\n');

    md
}

/// Build the usage string for a command.
fn build_usage_string(command_name: &str, cmd: &clap::Command, display_name: &str) -> String {
    let mut parts = vec![display_name.to_string()];

    // Check if there are any non-positional, non-built-in options
    let has_options = cmd
        .get_arguments()
        .any(|a| !a.is_positional() && a.get_id() != "version" && a.get_id() != "help");

    if has_options {
        parts.push("[OPTIONS]".to_string());
    }

    // Add positional arguments
    for arg in cmd.get_arguments() {
        if !arg.is_positional() || arg.get_id() == "version" || arg.get_id() == "help" {
            continue;
        }

        let value_name = arg
            .get_value_names()
            .and_then(|v| v.first().map(|s| s.to_string()))
            .unwrap_or_else(|| arg.get_id().as_str().to_uppercase());

        if arg.is_required_set() {
            parts.push(format!("<{value_name}>"));
        } else {
            parts.push(format!("[{value_name}]"));
        }
    }

    // Check for trailing var arg (like fetch's -- PULL_ARGS)
    let _ = command_name; // suppress unused warning; reserved for future use

    parts.join(" ")
}

/// Rewrite (or insert) the `daft_version:` stamp in a SKILL.md's
/// frontmatter. Invoked by the release.toml pre-release-hook with the
/// version being released.
fn stamp_skill(file: &Path, version: &str) -> Result<()> {
    let content =
        fs::read_to_string(file).with_context(|| format!("could not read {}", file.display()))?;
    let stamped = stamp_frontmatter(&content, version).with_context(|| {
        format!(
            "{} has no ----delimited frontmatter to stamp",
            file.display()
        )
    })?;
    if stamped == content {
        println!(
            "{} already stamped with daft_version {version}",
            file.display()
        );
        return Ok(());
    }
    fs::write(file, &stamped).with_context(|| format!("could not write {}", file.display()))?;
    println!("Stamped {} with daft_version {version}", file.display());
    Ok(())
}

/// Pure rewrite: replace a top-level `daft_version:` line inside the
/// `---`-delimited frontmatter, or insert one before the closing delimiter.
/// `None` when the content has no frontmatter.
fn stamp_frontmatter(content: &str, version: &str) -> Option<String> {
    let rest = content.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    let body = &rest[..end];
    let stamp_line = format!("daft_version: \"{version}\"");

    // Top-level key only (no indentation), so a nested `daft_version` in
    // some future frontmatter map can never be clobbered by accident.
    let new_body = if body.lines().any(|l| l.starts_with("daft_version:")) {
        body.lines()
            .map(|l| {
                if l.starts_with("daft_version:") {
                    stamp_line.as_str()
                } else {
                    l
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        format!("{body}\n{stamp_line}")
    };

    Some(format!("---\n{new_body}{}", &rest[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_frontmatter_replaces_existing_key() {
        let content = "---\nname: x\ndaft_version: \"1.18.0\"\n---\n\n# Body\n";
        let stamped = stamp_frontmatter(content, "1.19.0").unwrap();
        assert_eq!(
            stamped,
            "---\nname: x\ndaft_version: \"1.19.0\"\n---\n\n# Body\n"
        );
    }

    #[test]
    fn stamp_frontmatter_inserts_missing_key_before_close() {
        let content = "---\nname: x\n---\n\n# Body\n";
        let stamped = stamp_frontmatter(content, "1.19.0").unwrap();
        assert_eq!(
            stamped,
            "---\nname: x\ndaft_version: \"1.19.0\"\n---\n\n# Body\n"
        );
    }

    #[test]
    fn stamp_frontmatter_is_idempotent() {
        let content = "---\nname: x\ndaft_version: \"1.19.0\"\n---\n\n# Body\n";
        assert_eq!(stamp_frontmatter(content, "1.19.0").unwrap(), content);
    }

    #[test]
    fn stamp_frontmatter_rejects_missing_frontmatter() {
        assert!(stamp_frontmatter("# No frontmatter\n", "1.19.0").is_none());
    }

    #[test]
    fn stamp_frontmatter_ignores_indented_keys() {
        let content = "---\nname: x\nmeta:\n  daft_version: \"9.9.9\"\n---\n\n# Body\n";
        let stamped = stamp_frontmatter(content, "1.19.0").unwrap();
        // The nested key is untouched; a top-level stamp is inserted.
        assert!(stamped.contains("  daft_version: \"9.9.9\""));
        assert!(stamped.contains("\ndaft_version: \"1.19.0\"\n---\n"));
    }

    #[test]
    fn stamp_skill_stamps_the_repo_skill_shape() {
        // The real SKILL.md must always be stampable — this guards the
        // release hook against a frontmatter reshape breaking the splice.
        let repo_skill = include_str!("../../SKILL.md");
        let stamped = stamp_frontmatter(repo_skill, "9.9.9").unwrap();
        assert!(stamped.contains("daft_version: \"9.9.9\""));
    }

    #[test]
    fn test_all_commands_have_valid_handlers() {
        for command_name in COMMANDS {
            assert!(
                get_command_for_name(command_name).is_some(),
                "Command '{}' has no handler",
                command_name
            );
        }
    }

    #[test]
    fn test_unknown_command_returns_none() {
        assert!(get_command_for_name("unknown-command").is_none());
    }

    #[test]
    fn test_insert_examples_section_splices_before_subcommands() {
        let roff = ".SH OPTIONS\nstuff\n.SH SUBCOMMANDS\nsubs\n";
        let out = insert_examples_section(
            roff,
            &[("Clone into a worktree layout:", "daft clone <url>")],
        );
        let ex_idx = out.find(".SH EXAMPLES").expect("EXAMPLES section missing");
        let sub_idx = out.find(".SH SUBCOMMANDS").expect("SUBCOMMANDS missing");
        assert!(ex_idx < sub_idx, "EXAMPLES must precede SUBCOMMANDS");
        // Hyphens escaped (matches clap_mangen's output style).
        assert!(out.contains("daft clone \\<url\\>") || out.contains("daft clone <url>"));
        assert!(out.contains("worktree layout"));
    }

    #[test]
    fn test_insert_examples_section_empty_is_noop() {
        let roff = ".SH OPTIONS\nx\n.SH SUBCOMMANDS\n";
        assert_eq!(insert_examples_section(roff, &[]), roff);
    }

    #[test]
    fn test_insert_examples_section_escapes_hyphens() {
        let out = insert_examples_section(
            ".SH SUBCOMMANDS\n",
            &[("Start a long-lived branch:", "daft start feature-x")],
        );
        assert!(out.contains("long\\-lived"));
        assert!(out.contains("feature\\-x"));
    }

    #[test]
    fn test_man_page_generation() {
        let temp_dir = std::env::temp_dir().join("xtask-test-man");
        let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
        fs::create_dir_all(&temp_dir).unwrap();

        // Test generating a single man page
        generate_man_pages(&temp_dir, Some("git-worktree-clone")).unwrap();

        let man_file = temp_dir.join("git-worktree-clone.1");
        assert!(man_file.exists(), "Man page was not generated");

        let content = fs::read_to_string(&man_file).unwrap();
        assert!(content.contains(".TH"), "Man page missing .TH header");

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_all_man_pages_generation() {
        let temp_dir = std::env::temp_dir().join("xtask-test-all-man");
        let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
        fs::create_dir_all(&temp_dir).unwrap();

        // Test generating all man pages
        generate_man_pages(&temp_dir, None).unwrap();

        // Verify all expected man pages exist
        for command_name in COMMANDS {
            let man_file = temp_dir.join(format!("{command_name}.1"));
            assert!(
                man_file.exists(),
                "Man page for '{}' was not generated",
                command_name
            );

            let content = fs::read_to_string(&man_file).unwrap();
            assert!(
                content.contains(".TH"),
                "Man page for '{}' missing .TH header",
                command_name
            );
        }

        // Verify all daft verb man pages exist
        for verb in DAFT_VERBS {
            let man_file = temp_dir.join(format!("{}.1", verb.daft_name));
            assert!(
                man_file.exists(),
                "Man page for daft verb '{}' was not generated",
                verb.daft_name
            );

            let content = fs::read_to_string(&man_file).unwrap();
            assert!(
                content.contains(".TH"),
                "Man page for daft verb '{}' missing .TH header",
                verb.daft_name
            );
        }

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_cli_docs_generation() {
        let temp_dir = std::env::temp_dir().join("xtask-test-cli-docs");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        // Test generating a single CLI doc
        generate_cli_docs(&temp_dir, Some("git-worktree-clone")).unwrap();

        let doc_file = temp_dir.join("git-worktree-clone.md");
        assert!(doc_file.exists(), "CLI doc was not generated");

        let content = fs::read_to_string(&doc_file).unwrap();
        assert!(content.contains("---"), "CLI doc missing frontmatter");
        assert!(
            content.contains("# git worktree-clone"),
            "CLI doc missing title"
        );
        assert!(
            content.contains("## Usage"),
            "CLI doc missing Usage section"
        );
        assert!(
            content.contains("## Options"),
            "CLI doc missing Options section"
        );
        assert!(
            content.contains("## See Also"),
            "CLI doc missing See Also section"
        );

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_all_cli_docs_generation() {
        let temp_dir = std::env::temp_dir().join("xtask-test-all-cli-docs");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        // Test generating all CLI docs
        generate_cli_docs(&temp_dir, None).unwrap();

        // Verify all expected CLI docs exist
        for command_name in COMMANDS {
            let doc_file = temp_dir.join(format!("{command_name}.md"));
            assert!(
                doc_file.exists(),
                "CLI doc for '{}' was not generated",
                command_name
            );

            let content = fs::read_to_string(&doc_file).unwrap();
            assert!(
                content.contains("---"),
                "CLI doc for '{}' missing frontmatter",
                command_name
            );
            assert!(
                content.contains("## Usage"),
                "CLI doc for '{}' missing Usage section",
                command_name
            );
        }

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_related_commands_returns_entries() {
        let related = related_commands("git-worktree-clone");
        assert!(!related.is_empty());
        assert!(related.contains(&"git-worktree-init"));
    }

    #[test]
    fn test_related_commands_unknown_returns_empty() {
        let related = related_commands("unknown-command");
        assert!(related.is_empty());
    }
}

/// The settings-registry drift gate.
///
/// `src/core/settings_spec.rs` is only useful if it stays complete: a
/// `daft.*` key that exists in the code but has no registry row is invisible
/// in `daft config` forever, and nobody notices because everything still
/// compiles. This module is what notices.
///
/// It lives in xtask so it rides the existing `xtask-test` CI job, the
/// `xtask-tests` pre-merge ring, and `mise run test:xtask` — one check, three
/// surfaces already in parity, no new wiring. (Contrast the `repos-no-format`
/// grep gate, which is reachable only from `mise run lint` and therefore
/// never blocks a merge.)
#[cfg(test)]
mod config_registry_drift {
    use daft::commands::config::resolve::keys_match;
    use daft::core::settings_spec::all_specs;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    /// Final segments that make a `daft.<x>` literal a *filename* rather than
    /// a config key.
    ///
    /// `daft.yml` and `daft.local.yml` are why this exists: dozens live in
    /// the config loaders, doctor, and seed paths, and every one matches the
    /// key shape exactly. The list is only ever consulted for literals the
    /// registry does not already claim, so a real key whose last segment
    /// happens to look like an extension is still checked.
    const FILE_EXTENSIONS: &[&str] = &["yml", "yaml", "toml", "json", "js", "md", "lock", "sh"];

    /// The file that holds the registry itself — excluded from the
    /// "somebody actually reads this key" search, since it names every const
    /// by construction.
    const REGISTRY_FILE: &str = "settings_spec.rs";

    /// Key-shaped literals that are deliberately not settings, each with the
    /// reason it earns an exemption.
    ///
    /// The usual way to opt out is a hyphen — `daft.no-such-key` is not key
    /// shaped, so the scan ignores it. These are the cases where that does not
    /// work because the fixture's whole point is to be indistinguishable from
    /// a real key. An entry that stops appearing in the source is a test
    /// failure, so this list cannot quietly rot.
    const NOT_A_SETTING: &[(&str, &str)] = &[
        (
            "daft.checkoutbranch.carry",
            "mis-cased subsection fixture: git stores this as a separate, inert \
             key, and the resolver has to report it rather than credit the real row",
        ),
        (
            "daft.merge.stile",
            "did-you-mean fixture: one letter off a real key on purpose, so it \
             cannot be spelled with a hyphen without defeating the test",
        ),
    ];

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask/ sits directly under the repo root")
            .to_path_buf()
    }

    fn rust_sources() -> Vec<PathBuf> {
        walkdir::WalkDir::new(repo_root().join("src"))
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "rs"))
            .map(|entry| entry.path().to_path_buf())
            .collect()
    }

    /// Every `"daft.<key-shaped>"` string literal in `text`.
    ///
    /// Requiring the quotes is what keeps prose out: doc comments spell keys
    /// in backticks, and the dynamic per-hook key is built by
    /// `format!("daft.hooks.{hook}.{setting}")`, whose braces the shape test
    /// rejects.
    fn config_key_literals(text: &str) -> Vec<String> {
        let mut found = Vec::new();
        let mut cursor = 0;

        while let Some(offset) = text[cursor..].find("\"daft.") {
            let open = cursor + offset + 1;
            let Some(len) = text[open..].find('"') else {
                break;
            };
            let literal = &text[open..open + len];
            if is_key_shaped(literal) {
                found.push(literal.to_string());
            }
            cursor = open + len + 1;
        }

        found
    }

    /// `daft.` followed by dot-separated alphanumeric segments — the shape
    /// every real key has. Notably excludes hyphens, so a test fixture that
    /// deliberately is not a setting can say so by spelling itself
    /// `daft.no-such-key`.
    fn is_key_shaped(literal: &str) -> bool {
        let Some(rest) = literal.strip_prefix("daft.") else {
            return false;
        };
        rest.starts_with(|c: char| c.is_ascii_alphabetic())
            && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
    }

    fn is_filename(literal: &str) -> bool {
        literal
            .rsplit('.')
            .next()
            .is_some_and(|ext| FILE_EXTENSIONS.contains(&ext))
    }

    /// Every key the registry claims, including the deprecated spellings it
    /// still honours.
    fn registry_keys() -> Vec<String> {
        let mut keys = Vec::new();
        for spec in all_specs() {
            keys.push(spec.key.to_string());
            if let Some(alias) = &spec.deprecated_alias {
                keys.push(alias.to_string());
            }
        }
        keys
    }

    /// Whether the registry claims `literal`, compared the way git compares
    /// config keys.
    ///
    /// Exact string matching would flag `daft.checkout.PUSHVERIFY` as an
    /// orphan even though git reads it as `daft.checkout.pushVerify` — the
    /// trailing value name is case-insensitive. Borrowing the resolver's own
    /// comparison keeps the gate and the runtime honest about the same rule,
    /// and still catches a mis-cased *subsection*, which genuinely is a
    /// different key.
    fn registry_claims(keys: &[String], literal: &str) -> bool {
        keys.iter().any(|key| keys_match(key, literal))
    }

    /// `"daft.autocd" -> "AUTOCD"` for every const in `settings.rs`'s `keys`
    /// module, parsed rather than duplicated so the mapping cannot drift.
    fn key_consts() -> HashMap<String, String> {
        let source = std::fs::read_to_string(repo_root().join("src/core/settings.rs"))
            .expect("src/core/settings.rs is readable");

        let mut consts = HashMap::new();
        for line in source.lines() {
            let trimmed = line.trim();
            let Some(rest) = trimmed.strip_prefix("pub const ") else {
                continue;
            };
            let Some((name, tail)) = rest.split_once(": &str") else {
                continue;
            };
            let Some(start) = tail.find('"') else {
                continue;
            };
            let Some(end) = tail[start + 1..].find('"') else {
                continue;
            };
            let literal = &tail[start + 1..start + 1 + end];
            if literal.starts_with("daft.") {
                consts.insert(literal.to_string(), name.trim().to_string());
            }
        }
        consts
    }

    /// Every `daft.*` key literal in the codebase has a registry row.
    ///
    /// This is the direction that matters: it fires when a future PR adds a
    /// config key and forgets the row.
    #[test]
    fn every_config_key_in_the_codebase_has_a_registry_row() {
        let known = registry_keys();
        let mut orphans: Vec<(String, String)> = Vec::new();

        for path in rust_sources() {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for literal in config_key_literals(&text) {
                // Registry membership is checked before the filename rule so
                // a real key can never be waved through as a file.
                if registry_claims(&known, &literal)
                    || is_filename(&literal)
                    || NOT_A_SETTING.iter().any(|(key, _)| *key == literal)
                {
                    continue;
                }
                orphans.push((literal, path.display().to_string()));
            }
        }

        orphans.sort();
        orphans.dedup();

        assert!(
            orphans.is_empty(),
            "these `daft.*` keys have no row in src/core/settings_spec.rs:\n{}\n\n\
             Add a SettingSpec row so the key shows up in `daft config`. If the \
             literal is deliberately not a setting — a fixture for the \
             unrecognized-key diagnostic, say — spell it with a hyphen \
             (`daft.no-such-key`) so it does not look like one.",
            orphans
                .iter()
                .map(|(key, file)| format!("  {key}  ({file})"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    /// Every registry row's key is one something actually reads.
    ///
    /// Guards the other failure mode: a row for a phantom setting, which
    /// would document a knob that does nothing. The search is for the
    /// qualified `::NAME` spelling, which excludes the `pub const` definition
    /// itself, and it skips the registry file because that names every const
    /// by construction.
    ///
    /// Two consts share the name `ENABLED` (`keys::hooks` and
    /// `keys::multi_remote`), so for those the check proves one of the pair
    /// is read rather than both. Both are, and the alternative — tracking
    /// module nesting through the parse — buys strictness nothing needs.
    #[test]
    fn every_registry_key_is_read_by_something() {
        let consts = key_consts();
        let sources: Vec<(PathBuf, String)> = rust_sources()
            .into_iter()
            .filter(|path| !path.ends_with(REGISTRY_FILE))
            .filter_map(|path| {
                let text = std::fs::read_to_string(&path).ok()?;
                Some((path, text))
            })
            .collect();

        let mut unread = Vec::new();

        for spec in all_specs() {
            let Some(name) = consts.get(spec.key.as_ref()) else {
                // Per-hook and non-git rows have no const of their own.
                continue;
            };
            let needle = format!("::{name}");
            let read = sources.iter().any(|(_, text)| {
                text.match_indices(&needle).any(|(at, _)| {
                    let after = at + needle.len();
                    !text[after..]
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_alphanumeric() || c == '_')
                })
            });
            if !read {
                unread.push(format!("  {} (keys::{name})", spec.key));
            }
        }

        unread.sort();
        assert!(
            unread.is_empty(),
            "these registry rows name a key nothing reads:\n{}\n\n\
             Either wire the setting up, or drop the row — a documented knob \
             that does nothing is worse than an undocumented one.",
            unread.join("\n")
        );
    }

    /// An exemption that no longer applies is worse than no exemption: it
    /// silently widens the gate for whatever key-shaped literal appears next
    /// under that name.
    #[test]
    fn every_exemption_is_still_in_use() {
        let sources: Vec<String> = rust_sources()
            .into_iter()
            .filter_map(|path| std::fs::read_to_string(path).ok())
            .collect();

        for (key, reason) in NOT_A_SETTING {
            assert!(!reason.trim().is_empty(), "{key}: exemptions need a reason");
            assert!(
                sources
                    .iter()
                    .any(|text| config_key_literals(text).iter().any(|lit| lit == key)),
                "{key} is exempted but no longer appears in src/ — drop the entry"
            );
        }
    }

    #[test]
    fn the_literal_scanner_reads_code_not_prose() {
        // Quoted keys are found...
        assert_eq!(
            config_key_literals(r#"git.config_get("daft.autocd")"#),
            vec!["daft.autocd".to_string()]
        );
        // ...backticked prose in a doc comment is not.
        assert!(config_key_literals("/// Set via `daft.checkout.push`.").is_empty());
        // ...and neither is the dynamic per-hook format string.
        assert!(config_key_literals(r#"format!("daft.hooks.{hook}.{setting}")"#).is_empty());
        // A key-shaped literal in the middle of other code still is.
        assert_eq!(
            config_key_literals(r#"let a = "x"; let b = "daft.merge.style"; let c = "y";"#),
            vec!["daft.merge.style".to_string()]
        );
    }

    #[test]
    fn filenames_are_recognised_and_keys_are_not() {
        assert!(is_filename("daft.yml"));
        assert!(is_filename("daft.local.yml"));
        assert!(is_filename("daft.yaml"));
        assert!(is_filename("daft.js"));
        assert!(!is_filename("daft.autocd"));
        assert!(!is_filename("daft.merge.style"));
    }

    #[test]
    fn hyphenated_placeholders_are_not_key_shaped() {
        assert!(is_key_shaped("daft.autocd"));
        assert!(is_key_shaped("daft.hooks.output.tailLines"));
        assert!(!is_key_shaped("daft.no-such-key"));
        assert!(!is_key_shaped("daft."));
        assert!(!is_key_shaped("daft.9lives"));
    }
}

/// Drift gate for the merge gate itself.
///
/// `.github/workflows/test.yml` ends in a `ci-gate` job that fans in every
/// other job and is the one status check the `master` ruleset requires. A job
/// that is not in its `needs:` is a job whose failure does not block a merge —
/// and nothing else would notice: the workflow still runs, the gate still goes
/// green. These tests parse the workflow and hold the two invariants that make
/// the gate mean something: `needs:` names every other job, and the gate runs
/// unconditionally (`if: always()`), because a skipped required check is a
/// passing one.
///
/// Same home as `config_registry_drift`, for the same reason: the `xtask-test`
/// CI job, the `xtask-tests` pre-merge ring, and `mise run test:xtask` already
/// run it — one check, three surfaces, no new wiring.
#[cfg(test)]
mod ci_gate_drift {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    const WORKFLOW: &str = ".github/workflows/test.yml";
    const GATE: &str = "ci-gate";

    fn workflow() -> serde_yaml::Mapping {
        let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask/ sits directly under the repo root")
            .join(WORKFLOW);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} is readable: {e}", path.display()));
        let doc: serde_yaml::Value = serde_yaml::from_str(&text)
            .unwrap_or_else(|e| panic!("{} parses as YAML: {e}", path.display()));
        doc.get("jobs")
            .and_then(serde_yaml::Value::as_mapping)
            .cloned()
            .expect("test.yml has a `jobs:` mapping")
    }

    fn job_ids(jobs: &serde_yaml::Mapping) -> BTreeSet<String> {
        jobs.keys()
            .map(|k| k.as_str().expect("job ids are strings").to_string())
            .collect()
    }

    #[test]
    fn ci_gate_needs_every_job() {
        let jobs = workflow();
        let gate = jobs
            .get(GATE)
            .unwrap_or_else(|| panic!("{WORKFLOW} has a `{GATE}` job"));
        let needs: BTreeSet<String> = gate
            .get("needs")
            .and_then(serde_yaml::Value::as_sequence)
            .unwrap_or_else(|| panic!("`{GATE}` has a `needs:` list"))
            .iter()
            .map(|v| v.as_str().expect("needs entries are job ids").to_string())
            .collect();

        let mut expected = job_ids(&jobs);
        expected.remove(GATE);

        let missing: Vec<_> = expected.difference(&needs).cloned().collect();
        let unknown: Vec<_> = needs.difference(&expected).cloned().collect();
        assert!(
            missing.is_empty() && unknown.is_empty(),
            "`{GATE}` in {WORKFLOW} must need every other job, exactly.\n\
             missing from needs: {missing:?}\n\
             in needs but not a job: {unknown:?}\n\
             A job the gate does not need can fail without blocking the merge; \
             add it to `{GATE}.needs` (or remove the stale entry)."
        );
    }

    #[test]
    fn ci_gate_always_runs() {
        let jobs = workflow();
        let cond = jobs
            .get(GATE)
            .and_then(|g| g.get("if"))
            .and_then(serde_yaml::Value::as_str)
            .unwrap_or_default()
            .trim()
            .trim_start_matches("${{")
            .trim_end_matches("}}")
            .trim()
            .to_string();
        assert_eq!(
            cond, "always()",
            "`{GATE}` must run on `if: always()`: with `!cancelled()` (or no `if:` at \
             all) a cancelled or failed upstream job leaves the gate skipped, and a \
             skipped required status check counts as passing."
        );
    }

    #[test]
    fn no_workflow_level_path_filter() {
        // A workflow-level `paths:` filter means the workflow does not start
        // at all for a non-matching PR — so `ci-gate` never reports and the
        // PR can never merge. Path gating belongs to the `changes` job.
        let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask/ sits directly under the repo root")
            .join(WORKFLOW);
        let text = std::fs::read_to_string(&path).expect("test.yml is readable");
        let doc: serde_yaml::Value = serde_yaml::from_str(&text).expect("test.yml parses");
        // `on:` parses as the YAML 1.1 boolean `true` under serde_yaml.
        let on = doc
            .get("on")
            .or_else(|| doc.get(serde_yaml::Value::Bool(true)))
            .expect("test.yml has an `on:` trigger block");
        let pr = on
            .get("pull_request")
            .expect("test.yml triggers on pull_request");
        for key in ["paths", "paths-ignore"] {
            assert!(
                pr.get(key).is_none(),
                "test.yml must not use a workflow-level `pull_request.{key}` filter: a \
                 filtered-out run never starts, so the required `{GATE}` check stays \
                 Expected forever. Gate per job through the `changes` job instead."
            );
        }
    }
}

/// The multicall symlink farm must match the binary's argv[0] table, and
/// every install path must build it from the one list (#903).
#[cfg(test)]
mod multicall_farm_drift {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    const LIB: &str = "mise-tasks/setup/_rust_symlink_lib.sh";
    const MAIN: &str = "src/main.rs";
    const WORKFLOW: &str = ".github/workflows/test.yml";

    /// The install paths that cannot source a bash list and therefore keep
    /// their own copy: the window in each file that holds the names, as
    /// (file, opening marker, closing marker).
    const INSTALLERS: &[(&str, &str, &str)] = &[
        // cargo-dist generates the Homebrew formula and the shell/msi
        // installers from this — it is what `brew install` actually ships.
        ("dist-workspace.toml", "[dist.bin-aliases]", "\n["),
        ("flake.nix", "for cmd in", "; do"),
        // The distro packages. Install *and* removal: a name added to one
        // and not the other leaves a dangling /usr/bin symlink behind.
        ("packaging/deb/postinst", "for cmd in", "; do"),
        ("packaging/deb/prerm", "for cmd in", "; do"),
        ("packaging/aur/PKGBUILD", "for cmd in", "; do"),
        // The opening delimiter is part of the marker: splitting on a bare
        // `"""` would end the window on the quote that starts it.
        ("Cargo.toml", "post_install_script = \"\"\"", "\"\"\""),
        ("Cargo.toml", "pre_uninstall_script = \"\"\"", "\"\"\""),
        // `daft doctor` checks and repairs the farm; a name it does not
        // know is one `doctor --fix` never creates while reporting all
        // symlinks present.
        (
            "src/doctor/installation.rs",
            "const EXPECTED_SYMLINKS",
            "];",
        ),
    ];

    /// Farm entries that deliberately have no dispatch arm, and why. An
    /// entry here is a bug someone decided not to fix yet — not a blessing.
    const ORPHANS: &[(&str, &str)] = &[(
        "git-worktree-checkout-branch",
        "#904 — installed by the Homebrew formula and flake.nix too; \
         restoring the arm or retiring the name is a deliberate call",
    )];

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask/ sits directly under the repo root")
            .to_path_buf()
    }

    fn read(rel: &str) -> String {
        let path = repo_root().join(rel);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} is readable: {e}", path.display()))
    }

    /// Is this a name the multicall table dispatches on (as opposed to a
    /// shortcut alias, which `src/shortcuts.rs` resolves to one of these)?
    fn is_multicall_name(name: &str) -> bool {
        name.starts_with("git-worktree-") || name.starts_with("daft-") || name == "git-daft"
    }

    /// The `daft_multicall_symlinks=( … )` array, multicall names only.
    fn farm() -> BTreeSet<String> {
        let text = read(LIB);
        let body = text
            .split_once("daft_multicall_symlinks=(")
            .unwrap_or_else(|| panic!("{LIB} declares `daft_multicall_symlinks=(`"))
            .1;
        let body = body
            .split_once("\n)")
            .unwrap_or_else(|| panic!("{LIB}'s symlink array is closed by a `)` on its own line"))
            .0;
        body.lines()
            .map(|line| line.split('#').next().unwrap_or("").trim())
            .filter(|name| !name.is_empty())
            .filter(|name| is_multicall_name(name))
            .map(str::to_string)
            .collect()
    }

    /// The names `src/main.rs` dispatches on, read from the top-level arms
    /// of `match resolved { … }` only — the nested subcommand match inside
    /// the `git-daft` arm is indented deeper and is not a symlink table.
    fn dispatch_arms() -> BTreeSet<String> {
        let text = read(MAIN);
        let body = text
            .split_once("let result = match resolved {")
            .unwrap_or_else(|| panic!("{MAIN} dispatches through `match resolved`"))
            .1;
        let body = body
            .split_once("\n        _ =>")
            .unwrap_or_else(|| panic!("{MAIN}'s multicall match ends in a catch-all arm"))
            .0;

        let mut arms = BTreeSet::new();
        for line in body.lines() {
            // Top-level arms sit at exactly eight spaces; anything deeper
            // belongs to a nested match. A long or-pattern is wrapped by
            // rustfmt onto continuation lines that start with `|`, so the
            // arm's `=>` may be on a later line than its first name —
            // reading only `"…" … =>` lines would drop both names silently.
            let Some(rest) = line.strip_prefix("        ") else {
                continue;
            };
            if rest.starts_with(' ') || !(rest.starts_with('"') || rest.starts_with('|')) {
                continue;
            }
            let patterns = rest.split_once("=>").map_or(rest, |(p, _)| p);
            let found: Vec<&str> = patterns
                .split('"')
                .skip(1)
                .step_by(2)
                .filter(|name| is_multicall_name(name))
                .collect();
            assert!(
                !found.is_empty() || !patterns.contains("git-worktree-"),
                "the {MAIN} arm scanner saw a line it could not read a name out of — the \
                 source shape changed and this gate would silently under-report:\n{line}"
            );
            arms.extend(found.into_iter().map(str::to_string));
        }
        assert!(
            arms.len() > 10,
            "the {MAIN} arm scanner found only {} names — it stopped matching the source \
             shape, so this whole gate is vacuous",
            arms.len()
        );
        arms
    }

    #[test]
    fn every_dispatch_arm_has_a_symlink() {
        let missing: Vec<_> = dispatch_arms().difference(&farm()).cloned().collect();
        assert!(
            missing.is_empty(),
            "{MAIN} answers to {missing:?}, but {LIB} does not create the symlink(s), so no \
             install path ships the command. Add them to `daft_multicall_symlinks`."
        );
    }

    #[test]
    fn every_symlink_has_a_dispatch_arm() {
        let arms = dispatch_arms();
        let exempt: BTreeSet<String> = ORPHANS.iter().map(|(n, _)| n.to_string()).collect();
        let orphaned: Vec<_> = farm()
            .difference(&arms)
            .filter(|name| !exempt.contains(*name))
            .cloned()
            .collect();
        assert!(
            orphaned.is_empty(),
            "{LIB} creates {orphaned:?}, which {MAIN} has no arm for — invoking one prints \
             \"Unknown command\" and exits 1. Add the arm, or drop the name from the farm."
        );
    }

    /// Same spirit as the settings registry's exemption check: a documented
    /// orphan that stopped being one must lose its exemption, or the next
    /// real orphan hides behind it.
    #[test]
    fn every_orphan_exemption_is_still_needed() {
        let (arms, farm) = (dispatch_arms(), farm());
        for (name, why) in ORPHANS {
            assert!(
                farm.contains(*name),
                "{name} is exempted here but {LIB} no longer creates it — drop the \
                 ORPHANS entry ({why})"
            );
            assert!(
                !arms.contains(*name),
                "{name} now has a dispatch arm in {MAIN} — drop the ORPHANS entry ({why})"
            );
        }
    }

    /// Multicall names inside one installer's window. Tokenised rather than
    /// parsed so the same reader works on TOML strings and bare Nix words.
    fn installer_names(file: &str, open: &str, close: &str) -> BTreeSet<String> {
        let text = read(file);
        let body = text
            .split_once(open)
            .unwrap_or_else(|| {
                panic!(
                    "{file} still contains `{open}` — this gate reads the \
                 symlink list from that window and cannot find it"
                )
            })
            .1;
        let body = body
            .split_once(close)
            .unwrap_or_else(|| panic!("{file}'s symlink list is no longer closed by `{close}`"))
            .0;

        let names: BTreeSet<String> = body
            .split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
            .filter(|token| is_multicall_name(token))
            .map(str::to_string)
            .collect();
        assert!(
            names.len() > 10,
            "found only {} multicall names in {file} — the window moved and this gate went \
             vacuous",
            names.len()
        );
        names
    }

    /// The farm list is bash, so CI and the shell suite can source it; the
    /// installers cannot, so they are gated instead of unified. A command
    /// missing here is one a real `brew install` / `nix build` never ships.
    #[test]
    fn every_dispatch_arm_ships_in_every_installer() {
        let arms = dispatch_arms();
        let exempt: BTreeSet<String> = ORPHANS.iter().map(|(n, _)| n.to_string()).collect();
        for (file, open, close) in INSTALLERS {
            let shipped = installer_names(file, open, close);
            let missing: Vec<_> = arms.difference(&shipped).cloned().collect();
            assert!(
                missing.is_empty(),
                "{file} does not install {missing:?}, so users of that install path get \
                 \"command not found\" for a command {MAIN} answers to."
            );
            let unknown: Vec<_> = shipped
                .difference(&arms)
                .filter(|name| !exempt.contains(*name))
                .cloned()
                .collect();
            assert!(
                unknown.is_empty(),
                "{file} installs {unknown:?}, which {MAIN} has no arm for — those symlinks \
                 print \"Unknown command\" and exit 1."
            );
        }
    }

    /// CI must build the farm from the same list, not from a copy of it.
    #[test]
    fn ci_sources_the_shared_symlink_list() {
        let text = read(WORKFLOW);
        let doc: serde_yaml::Value = serde_yaml::from_str(&text)
            .unwrap_or_else(|e| panic!("{WORKFLOW} parses as YAML: {e}"));
        let steps = doc
            .get("jobs")
            .and_then(|j| j.get("integration-tests"))
            .and_then(|j| j.get("steps"))
            .and_then(serde_yaml::Value::as_sequence)
            .unwrap_or_else(|| panic!("{WORKFLOW} has an `integration-tests` job with steps"));
        let setup = steps
            .iter()
            .find(|s| s.get("name").and_then(serde_yaml::Value::as_str) == Some("Set up binary"))
            .unwrap_or_else(|| {
                panic!("{WORKFLOW}'s integration-tests job has a `Set up binary` step")
            });
        let run = setup
            .get("run")
            .and_then(serde_yaml::Value::as_str)
            .expect("`Set up binary` is a run: step");

        assert!(
            run.contains("_rust_symlink_lib.sh") && run.contains("create_daft_symlinks"),
            "`Set up binary` must source {LIB} and call create_daft_symlinks, so CI's farm \
             cannot drift from the dev one (#903). Step body:\n{run}"
        );
        assert!(
            !run.contains("for cmd in git-worktree-"),
            "`Set up binary` hand-copies the symlink list again — that is the drift {LIB} \
             exists to prevent (#903). Step body:\n{run}"
        );
    }
}

/// Drift gate for the daily tool-upgrade job (#926).
///
/// `.github/workflows/mise-tool-updates.yml` is the only thing keeping the
/// pins in `mise.toml` current, and its characteristic failure is doing
/// nothing while reporting success. It has now silently no-op'd twice from two
/// unrelated causes — a missing `--bump` (#803), then an empty
/// `MISE_MINIMUM_RELEASE_AGE` (#926) — and neither was visible from the
/// outside: the job goes green in fifteen seconds and opens no PR, which is
/// also exactly what a genuinely up-to-date day looks like. Nothing else in
/// the repo can tell those two apart, so these tests hold the properties that
/// separate them.
///
/// Same home as `ci_gate_drift` / `multicall_farm_drift`, for the same reason:
/// the `xtask-test` CI job, the `xtask-tests` pre-merge ring, and
/// `mise run test:xtask` already run it — one check, three surfaces, no new
/// wiring.
#[cfg(test)]
mod mise_upgrade_drift {
    use std::path::{Path, PathBuf};

    const WORKFLOW: &str = ".github/workflows/mise-tool-updates.yml";
    const ENV_KNOB: &str = "MISE_MINIMUM_RELEASE_AGE";

    fn workflow() -> serde_yaml::Value {
        let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask/ sits directly under the repo root")
            .join(WORKFLOW);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} is readable: {e}", path.display()));
        serde_yaml::from_str(&text)
            .unwrap_or_else(|e| panic!("{} parses as YAML: {e}", path.display()))
    }

    /// The body of the step that runs `mise upgrade`.
    fn upgrade_run() -> String {
        let doc = workflow();
        let steps = doc
            .get("jobs")
            .and_then(|j| j.get("upgrade"))
            .and_then(|j| j.get("steps"))
            .and_then(serde_yaml::Value::as_sequence)
            .unwrap_or_else(|| panic!("{WORKFLOW} has an `upgrade` job with steps"))
            .clone();
        steps
            .iter()
            .filter_map(|s| s.get("run").and_then(serde_yaml::Value::as_str))
            .find(|r| r.contains("mise upgrade"))
            .unwrap_or_else(|| panic!("{WORKFLOW}'s `upgrade` job runs `mise upgrade`"))
            .to_string()
    }

    /// Is `key` used as a mapping key anywhere in the document? Workflow-level
    /// `env:`, job-level, and step-level all land here, so the check does not
    /// depend on where someone puts the block.
    fn set_as_mapping_key(node: &serde_yaml::Value, key: &str) -> bool {
        match node {
            serde_yaml::Value::Mapping(m) => m
                .iter()
                .any(|(k, v)| k.as_str() == Some(key) || set_as_mapping_key(v, key)),
            serde_yaml::Value::Sequence(s) => s.iter().any(|v| set_as_mapping_key(v, key)),
            _ => false,
        }
    }

    #[test]
    fn cooldown_override_is_never_an_env_var() {
        assert!(
            !set_as_mapping_key(&workflow(), ENV_KNOB),
            "{WORKFLOW} must not set `{ENV_KNOB}` through `env:` — pass \
             `--minimum-release-age` on the command line instead.\n\
             \n\
             An Actions `env:` value is always *set*. The false branch of a \
             `${{{{ inputs.x && '0' || '' }}}}` expression therefore exports the \
             variable as the empty string rather than omitting it, and mise \
             cannot parse \"\" as a duration: it WARNs instead of erroring, \
             resolves no version list for any tool, and `--bump` finds nothing \
             to bump. The job stays green having done nothing — that is #926, \
             which cost every pin four months of drift and silently neutered \
             #804's `--bump` fix along the way.\n\
             \n\
             Only an *unset* variable falls through to mise.toml's `[settings] \
             minimum_release_age`. Spelling the 7d default here instead would \
             duplicate a value mise.toml owns, which is its own drift bug."
        );
    }

    #[test]
    fn upgrade_bumps_pins_and_wires_the_bypass_as_a_flag() {
        let run = upgrade_run();
        assert!(
            run.contains("--bump"),
            "`mise upgrade` must keep `--bump` in {WORKFLOW}: every tool in \
             mise.toml is pinned exactly, and an exact pin admits exactly one \
             version, so a bare `mise upgrade` can never move anything (#803). \
             Do not drop it without also unpinning the tools. Step body:\n{run}"
        );
        assert!(
            run.contains("--minimum-release-age") && run.contains("inputs.bypass_cooldown"),
            "the `bypass_cooldown` input must reach `mise upgrade` as \
             `--minimum-release-age`, or the workflow_dispatch input is a lie: \
             it only runs on manual dispatch, so an inert bypass would go \
             unnoticed indefinitely — exactly how #926 survived four months. \
             Discriminating check when editing this: with the cooldown the job \
             offers the newest release older than 7d, and with the bypass it \
             offers the newest release outright. If both agree, the flag is \
             doing nothing. Step body:\n{run}"
        );
        assert!(
            !run.contains(ENV_KNOB),
            "the cooldown knob must not reach mise as an inline env prefix \
             either: `{ENV_KNOB}=\"\" mise upgrade` sets it to the empty \
             string for that command and breaks version resolution exactly \
             the way the `env:` block did (#926). The parsed-YAML check in \
             `cooldown_override_is_never_an_env_var` cannot see it — an \
             inline prefix is part of the `run:` string, not a mapping key — \
             so it is checked here. Step body:\n{run}"
        );
    }

    #[test]
    fn unresolved_version_lists_are_fatal() {
        let run = upgrade_run();
        assert!(
            run.contains("Failed to resolve tool version list"),
            "the upgrade step must fail when mise reports \
             `Failed to resolve tool version list`. mise WARNs rather than \
             errors there, and a run that resolved nothing is otherwise \
             indistinguishable from a run that had nothing to do — both are \
             green and open no PR, which is how #926 hid for four months and \
             #803's no-op hid before it. Grep the step's own output and exit \
             non-zero.\n\
             \n\
             Match that sentinel specifically, not WARNs in general: `newer \
             <tool> release X ignored by minimum_release_age` is also a WARN \
             and fires on healthy runs. Step body:\n{run}"
        );
    }
}
