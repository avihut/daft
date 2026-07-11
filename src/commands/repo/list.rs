//! `daft repo list` — show the repo catalog.
//!
//! Renders through the same visual language as `daft list`: a blank-style
//! table with dim-underlined headers, a cyan `>` marking the repo the user
//! is standing in, removed rows dimmed, and — with `--sizes` on a terminal —
//! the shared live inline table ([`CatalogTable`]) shimmer-loading each
//! repo's disk-size walk as it streams in. Piped/structured output takes the
//! blocking path, mirroring `daft list`'s `should_use_live` gating.

use anyhow::{Context, Result};
use clap::Parser;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use crate::catalog::Catalog;
use crate::commands::list::PriorityMaxExcept;
use crate::core::worktree::list::compute_directory_size;
use crate::output::emit::{self, Cell, EmitArgs, EmitPayload, Table};
use crate::output::format::format_human_size;
use crate::output::tui::{CatalogEvent, CatalogRepoCells, CatalogTable, LiveScreen, TuiRenderer};
use crate::output::{CliOutput, Output, OutputConfig};
use crate::store::CatalogRepoRow;
use crate::styles;

#[derive(Parser, Debug)]
#[command(name = "git-daft-repo-list")]
#[command(version = crate::VERSION)]
#[command(about = "List repositories in the repo catalog")]
#[command(long_about = r#"
Lists the repositories daft knows about. The catalog fills itself: cloning,
initializing, adopting, or running daft commands inside a repo registers it
automatically; `git daft repo add` registers one manually.

Removed repositories keep a catalog entry (so their job logs stay
addressable and `git daft clone <name>` can restore them); show them with
--all.
"#)]
pub struct Args {
    #[arg(short = 'a', long = "all", help = "Include removed repositories")]
    all: bool,

    #[arg(
        long = "sizes",
        help = "Add a disk-usage column (walks every repository, like `git daft list --columns +size`)"
    )]
    sizes: bool,

    #[command(flatten)]
    emit: EmitArgs,

    #[arg(short = 'q', long = "quiet", help = "Suppress progress reporting")]
    quiet: bool,
}

pub fn run() -> Result<()> {
    let raw_args: Vec<String> = crate::cli::argv().to_vec();
    debug_assert!(
        raw_args.len() >= 3 && raw_args[1] == "repo" && raw_args[2] == "list",
        "repo::list::run() invoked with unexpected argv: {raw_args:?} \
         (expected `daft repo list ...`)"
    );
    let argv: Vec<String> = std::iter::once("git-daft-repo-list".to_string())
        .chain(raw_args.into_iter().skip(3))
        .collect();
    let args = Args::parse_from(argv);
    let mut output = CliOutput::new(OutputConfig::new(args.quiet, false));

    let rows = match Catalog::open_ro().context("could not open the repo catalog")? {
        Some(catalog) => catalog.list(args.all)?,
        None => Vec::new(),
    };

    let cells = build_display_cells(&rows);

    if args.emit.is_structured() {
        // Structured consumers get everything in one shot — walks run
        // synchronously when requested.
        let sizes = args.sizes.then(|| compute_sizes(&rows));
        let payload = build_payload(&rows, &cells, sizes.as_deref());
        return emit::emit_and_handle("repo list", payload, &args.emit, &mut std::io::stdout())
            .map_err(|e| anyhow::anyhow!("{e}"));
    }

    if rows.is_empty() {
        output.info("No repositories in the catalog yet.");
        output.info(&format!(
            "Repos are cataloged automatically by clone and init; `{}` registers one manually.",
            crate::daft_cmd("repo add")
        ));
        return Ok(());
    }

    // Live inline table only when the size walks give it something to
    // stream — every other cell is seeded synchronously, so without
    // --sizes a static table is already final. Gating mirrors
    // `list::should_use_live`.
    if args.sizes && use_live_table() {
        return run_live(cells, &rows);
    }

    let sizes = args.sizes.then(|| compute_sizes(&rows));
    let term_width = terminal_size::terminal_size().map(|(w, _)| w.0 as usize);
    println!(
        "{}",
        build_blocking_table(
            &cells,
            sizes.as_deref(),
            styles::colors_enabled(),
            term_width
        )
    );
    Ok(())
}

/// Mirrors `list::should_use_live` minus the structured check (handled
/// earlier): live rendering wants a real terminal and no `DAFT_NO_LIVE`.
fn use_live_table() -> bool {
    std::env::var_os("DAFT_NO_LIVE").is_none() && std::io::stdout().is_terminal()
}

/// Stream the per-repo disk-size walks into the shared live table. One
/// walker thread per repo (the walks are independent and IO-bound); a
/// supervisor joins them and emits the `Done` sentinel while the renderer
/// listens.
fn run_live(cells: Vec<CatalogRepoCells>, rows: &[CatalogRepoRow]) -> Result<()> {
    let (tx, rx) = mpsc::channel::<CatalogEvent>();
    let cancel = Arc::new(AtomicBool::new(false));

    let mut workers = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let tx = tx.clone();
        let cancel = Arc::clone(&cancel);
        let path = PathBuf::from(&row.path);
        workers.push(std::thread::spawn(move || {
            // The walk itself is not interruptible; the flag is checked
            // before starting so queued walks stop promptly on Ctrl-C.
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let bytes = compute_directory_size(&path);
            let _ = tx.send(CatalogEvent::Size { index, bytes });
        }));
    }

    // The Done sentinel must fire while the renderer is listening, so the
    // worker join happens on its own thread (mirroring list_live's
    // collector-join dance).
    let join_thread = std::thread::spawn(move || {
        for worker in workers {
            let _ = worker.join();
        }
        let _ = tx.send(CatalogEvent::Done);
    });

    // SIGINT fallback for when raw mode fails to enable (see list_live):
    // flips the same cancel flag the renderer polls. `ctrlc::set_handler`
    // is process-global and can only be installed once; swallow the error
    // so nested invocations and tests don't panic.
    let signal_cancel = Arc::clone(&cancel);
    let _ = ctrlc::set_handler(move || {
        signal_cancel.store(true, Ordering::Relaxed);
    });

    // Raw mode routes Ctrl-C into the render loop as a key event; the RAII
    // guard restores cooked mode on every exit path.
    let _raw_guard = crate::output::tui::enable_raw_mode_guard();
    let renderer = TuiRenderer::new(CatalogTable::new(cells), rx).with_cancel_signal(cancel);
    let screen = renderer.run()?;

    // On normal completion the walkers are already done and the join returns
    // immediately. On cancellation, walks may still be mid-flight — skip the
    // join so the user gets an instant prompt back; the OS reaps the
    // read-only walker threads when the process exits.
    if !screen.is_cancelled() {
        let _ = join_thread.join();
    }
    Ok(())
}

/// Worktree count for a repo: the main working tree (if any) plus linked
/// worktrees — what `git worktree list` would show. `None` when the repo
/// can't be opened (stale path, removed repo).
fn worktree_count(git_common_dir: &Path) -> Option<usize> {
    let repo = gix::open(git_common_dir).ok()?;
    let linked = repo.worktrees().map(|w| w.len()).unwrap_or(0);
    Some(linked + usize::from(repo.workdir().is_some()))
}

/// Abbreviate `$HOME` to `~` for display. Structured output keeps raw paths.
fn tilde_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = Path::new(path).strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() {
            "~".to_string()
        } else {
            format!("~/{}", rest.display())
        };
    }
    path.to_string()
}

/// The canonical git-common-dir of the repo the user is standing in, if any.
fn current_git_common_dir() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let repo = gix::discover(&cwd).ok()?;
    std::fs::canonicalize(repo.common_dir()).ok()
}

fn build_display_cells(rows: &[CatalogRepoRow]) -> Vec<CatalogRepoCells> {
    let current_gcd = current_git_common_dir();
    rows.iter()
        .map(|row| {
            let current = current_gcd.as_deref().is_some_and(|cur| {
                std::fs::canonicalize(&row.git_common_dir).is_ok_and(|gcd| gcd == cur)
            });
            let name = if row.removed_at.is_some() {
                format!("{} (removed)", row.name)
            } else {
                row.name.clone()
            };
            CatalogRepoCells {
                current,
                removed: row.removed_at.is_some(),
                name,
                worktrees: worktree_count(Path::new(&row.git_common_dir)),
                path: tilde_path(&row.path),
                remote: row.remote_url.clone(),
            }
        })
        .collect()
}

fn compute_sizes(rows: &[CatalogRepoRow]) -> Vec<Option<u64>> {
    rows.iter()
        .map(|row| compute_directory_size(Path::new(&row.path)))
        .collect()
}

fn build_payload(
    rows: &[CatalogRepoRow],
    cells: &[CatalogRepoCells],
    sizes: Option<&[Option<u64>]>,
) -> EmitPayload {
    let mut headers = vec![
        "name",
        "worktrees",
        "default_branch",
        "path",
        "remote_url",
        "removed_at",
    ];
    if sizes.is_some() {
        headers.push("size_bytes");
    }
    let mut table = Table::new(headers);
    for (i, row) in rows.iter().enumerate() {
        let mut record = vec![
            Cell::str(&row.name),
            cells[i]
                .worktrees
                .map(|n| Cell::int(n as i64))
                .unwrap_or(Cell::Null),
            row.default_branch
                .as_deref()
                .map(Cell::str)
                .unwrap_or(Cell::Null),
            Cell::str(&row.path),
            row.remote_url
                .as_deref()
                .map(Cell::str)
                .unwrap_or(Cell::Null),
            row.removed_at
                .map(|t| Cell::str(t.to_rfc3339()))
                .unwrap_or(Cell::Null),
        ];
        if let Some(sizes) = sizes {
            record.push(sizes[i].map(|b| Cell::int(b as i64)).unwrap_or(Cell::Null));
        }
        table = table.row(record);
    }
    EmitPayload::Tabular(table)
}

/// Build the blocking (non-live) table — the same house style as `daft
/// list`'s `print_table`: `Style::blank`, dim-underlined Title-case headers,
/// a leading pad, annotation column only when a row is current, removed rows
/// dimmed, and terminal-width truncation that never eats the Size column's
/// TOTAL cell.
fn build_blocking_table(
    cells: &[CatalogRepoCells],
    sizes: Option<&[Option<u64>]>,
    use_color: bool,
    term_width: Option<usize>,
) -> String {
    use tabled::builder::Builder;
    use tabled::settings::peaker::Priority;
    use tabled::settings::{Padding, Style, Width, object::Columns};

    let annotation = cells.iter().any(|c| c.current);
    let dim = |s: &str, apply: bool| -> String {
        if apply && use_color {
            styles::dim(s)
        } else {
            s.to_string()
        }
    };

    let mut headers: Vec<String> = Vec::new();
    if annotation {
        headers.push(String::new());
    }
    let mut labels = vec!["Name", "Worktrees", "Path", "Remote"];
    if sizes.is_some() {
        labels.push("Size");
    }
    headers.extend(labels.iter().map(|l| {
        if use_color {
            styles::dim_underline(l)
        } else {
            (*l).to_string()
        }
    }));

    let mut builder = Builder::new();
    builder.push_record(headers);
    for (i, cell) in cells.iter().enumerate() {
        let mut record: Vec<String> = Vec::new();
        if annotation {
            record.push(if cell.current {
                if use_color {
                    styles::cyan(styles::CURRENT_WORKTREE_SYMBOL)
                } else {
                    styles::CURRENT_WORKTREE_SYMBOL.to_string()
                }
            } else {
                String::new()
            });
        }
        record.push(dim(&cell.name, cell.removed));
        record.push(dim(
            &cell
                .worktrees
                .map(|n| n.to_string())
                .unwrap_or_else(|| "-".to_string()),
            cell.removed,
        ));
        record.push(dim(&cell.path, cell.removed));
        // Remote is reference info, not the primary signal — always dim.
        record.push(dim(cell.remote.as_deref().unwrap_or("-"), true));
        if let Some(sizes) = sizes {
            record.push(dim(
                &sizes[i]
                    .map(format_human_size)
                    .unwrap_or_else(|| "-".to_string()),
                cell.removed,
            ));
        }
        builder.push_record(record);
    }

    // TOTAL footer under the Size column, matching `daft list`.
    if let Some(sizes) = sizes {
        let total: u64 = sizes.iter().filter_map(|s| *s).sum();
        let total_cell = dim(&format_human_size(total), true);
        let data_cols = 4 + usize::from(annotation);
        let mut separator: Vec<String> = vec![String::new(); data_cols + 1];
        builder.push_record(separator.clone());
        separator[data_cols] = total_cell;
        builder.push_record(separator);
    }

    let mut table = builder.build();
    table.with(Style::blank());
    table.modify(Columns::first(), Padding::new(1, 0, 0, 0));

    if let Some(width) = term_width {
        match sizes.is_some() {
            // Exclude the Size column from the shrink candidate set — its
            // TOTAL summary cell can be wider than any data cell and gets
            // truncated otherwise (#501, same as `daft list`).
            true => {
                let size_idx = 4 + usize::from(annotation);
                table.with(
                    Width::truncate(width)
                        .suffix("...")
                        .priority(PriorityMaxExcept {
                            excluded: vec![size_idx],
                        }),
                )
            }
            false => table.with(
                Width::truncate(width)
                    .suffix("...")
                    .priority(Priority::max(true)),
            ),
        };
    }

    table.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(name: &str, current: bool, removed: bool) -> CatalogRepoCells {
        CatalogRepoCells {
            current,
            removed,
            name: name.to_string(),
            worktrees: Some(3),
            path: format!("~/src/{name}"),
            remote: Some(format!("git@example.com:acme/{name}.git")),
        }
    }

    /// Regression: the first shipped renderer emitted every line through
    /// `output.raw()` (`print!`, no newline), fusing header and rows into a
    /// single line. The table must be one line per row plus the header.
    #[test]
    fn blocking_table_emits_header_and_one_line_per_row() {
        let table = build_blocking_table(
            &[cells("alpha", false, false), cells("beta", false, false)],
            None,
            false,
            None,
        );
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(
            lines.len(),
            3,
            "header + 2 data rows, got {}: {table:?}",
            lines.len()
        );
        for label in ["Name", "Worktrees", "Path", "Remote"] {
            assert!(
                lines[0].contains(label),
                "header missing {label}: {:?}",
                lines[0]
            );
        }
        assert!(!lines[0].contains("Size"), "Size column is opt-in");
        assert!(lines[1].contains("alpha"));
        assert!(lines[1].contains("git@example.com:acme/alpha.git"));
        assert!(lines[2].contains("beta"));
    }

    #[test]
    fn blocking_table_marks_the_current_repo() {
        let table = build_blocking_table(
            &[cells("alpha", true, false), cells("beta", false, false)],
            None,
            false,
            None,
        );
        let lines: Vec<&str> = table.lines().collect();
        assert!(
            lines[1]
                .trim_start()
                .starts_with(styles::CURRENT_WORKTREE_SYMBOL),
            "current repo row carries the marker: {:?}",
            lines[1]
        );
        assert!(
            !lines[2]
                .trim_start()
                .starts_with(styles::CURRENT_WORKTREE_SYMBOL),
            "other rows do not: {:?}",
            lines[2]
        );
    }

    #[test]
    fn blocking_table_annotation_column_absent_when_no_current() {
        let table = build_blocking_table(&[cells("alpha", false, false)], None, false, None);
        let lines: Vec<&str> = table.lines().collect();
        assert!(
            lines[0].trim_start().starts_with("Name"),
            "without a current repo the first column is Name: {:?}",
            lines[0]
        );
    }

    #[test]
    fn blocking_table_sizes_column_renders_values_and_total() {
        let table = build_blocking_table(
            &[cells("alpha", false, false), cells("beta", false, false)],
            Some(&[Some(1024 * 1024), Some(1024 * 1024)]),
            false,
            None,
        );
        let lines: Vec<&str> = table.lines().collect();
        // header + 2 rows + separator + total
        assert_eq!(lines.len(), 5, "expected total footer: {table:?}");
        assert!(lines[0].contains("Size"));
        assert!(lines[1].contains("1.0M"));
        assert!(
            lines[4].contains("2.0M"),
            "TOTAL sums the walks: {:?}",
            lines[4]
        );
    }

    #[test]
    fn blocking_table_shows_dash_for_missing_count_size_and_remote() {
        let mut cell = cells("alpha", false, true);
        cell.worktrees = None;
        cell.remote = None;
        let table = build_blocking_table(&[cell], Some(&[None]), false, None);
        let lines: Vec<&str> = table.lines().collect();
        assert!(lines[1].contains("alpha"));
        let dashes = lines[1].matches('-').count();
        assert!(
            dashes >= 3,
            "missing count, remote, and size all render '-': {:?}",
            lines[1]
        );
    }

    #[test]
    fn tilde_path_abbreviates_home_and_passes_others_through() {
        if let Some(home) = dirs::home_dir() {
            let inside = format!("{}/src/thing", home.display());
            assert_eq!(tilde_path(&inside), "~/src/thing");
            assert_eq!(tilde_path(&home.display().to_string()), "~");
        }
        assert_eq!(tilde_path("/opt/elsewhere"), "/opt/elsewhere");
    }

    #[test]
    fn worktree_count_is_none_outside_a_repo() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(worktree_count(tmp.path()), None);
    }

    #[test]
    fn payload_includes_worktrees_and_optional_size_bytes() {
        let row = CatalogRepoRow {
            uuid: "0198c0de-0000-7000-8000-000000000000".to_string(),
            name: "alpha".to_string(),
            path: "/tmp/alpha".to_string(),
            git_common_dir: "/tmp/alpha/.git".to_string(),
            remote_url: Some("git@example.com:acme/alpha.git".to_string()),
            remote_url_normalized: Some("example.com/acme/alpha".to_string()),
            default_branch: Some("main".to_string()),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            removed_at: None,
        };
        let cell = cells("alpha", false, false);

        let payload = build_payload(
            std::slice::from_ref(&row),
            std::slice::from_ref(&cell),
            None,
        );
        let EmitPayload::Tabular(table) = payload else {
            panic!("expected tabular payload");
        };
        assert_eq!(
            table.headers,
            [
                "name",
                "worktrees",
                "default_branch",
                "path",
                "remote_url",
                "removed_at"
            ]
        );

        let payload = build_payload(
            std::slice::from_ref(&row),
            std::slice::from_ref(&cell),
            Some(&[Some(42)]),
        );
        let EmitPayload::Tabular(table) = payload else {
            panic!("expected tabular payload");
        };
        assert_eq!(table.headers.last().map(String::as_str), Some("size_bytes"));
    }
}
