//! `daft repo list` — show the repo catalog.
//!
//! Renders through the same visual language as `daft list`: a blank-style
//! table with dim-underlined headers, a cyan `>` marking the repo the user
//! is standing in, removed rows dimmed, and — with `--columns +size` on a
//! terminal — the shared live inline table ([`CatalogTable`]) shimmer-loading
//! each repo's disk-size walk as it streams in. Piped/structured output takes
//! the blocking path, mirroring `daft list`'s `should_use_live` gating.
//!
//! Column selection speaks the house `--columns` grammar (replace mode /
//! `+col,-col` modifiers) shared with list/sync/prune/clone; the repo-specific
//! column family lives in [`crate::core::columns::RepoListColumn`].

use anyhow::{Context, Result};
use clap::Parser;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use crate::catalog::Catalog;
use crate::commands::list::PriorityMaxExcept;
use crate::core::columns::{RepoColumnSelection, RepoListColumn, ResolvedColumns};
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

Use --columns to select which columns are shown and in what order.
  Replace mode:  --columns name,path,remote (exact set and order)
  Modifier mode: --columns -remote (remove from defaults)
  Add optional:  --columns +size,+branch (add to defaults)

The size column is not shown by default — it walks every repository, so it
is opt-in, same as the worktree commands. On a terminal the sizes stream in
live while the table renders immediately, with a total row summing them.
The recorded default branch is likewise opt-in (+branch); structured output
includes it by default.
"#)]
pub struct Args {
    #[arg(short = 'a', long = "all", help = "Include removed repositories")]
    all: bool,

    #[arg(
        long = "columns",
        help = "Columns to display (comma-separated). Replace: name,path,remote. Modify defaults: +col,-col. Available: annotation, name, worktrees, branch, path, size, remote"
    )]
    columns: Option<String>,

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

    // Validate the column spec before any catalog IO so a typo fails fast.
    let resolved = match args.columns.as_deref() {
        Some(input) => RepoColumnSelection::parse(input).map_err(|e| anyhow::anyhow!("{e}"))?,
        None => ResolvedColumns::defaults(RepoListColumn::repo_list_defaults()),
    };
    let columns = resolved.columns;
    let has_size = columns.contains(&RepoListColumn::Size);

    let rows = match Catalog::open_ro().context("could not open the repo catalog")? {
        Some(catalog) => catalog.list(args.all)?,
        None => Vec::new(),
    };

    let cells = build_display_cells(&rows);

    if args.emit.is_structured() {
        // Structured consumers get everything in one shot — walks run
        // synchronously when requested.
        let sizes = has_size.then(|| compute_sizes(&rows));
        let payload = build_payload(&rows, &cells, sizes.as_deref(), &columns);
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
    // stream — every other cell is seeded synchronously, so without the
    // size column a static table is already final. Gating mirrors
    // `list::should_use_live`.
    if has_size && use_live_table() {
        return run_live(cells, &rows, columns);
    }

    let sizes = has_size.then(|| compute_sizes(&rows));
    let term_width = terminal_size::terminal_size().map(|(w, _)| w.0 as usize);
    println!(
        "{}",
        build_blocking_table(
            &cells,
            sizes.as_deref(),
            &columns,
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
fn run_live(
    cells: Vec<CatalogRepoCells>,
    rows: &[CatalogRepoRow],
    columns: Vec<RepoListColumn>,
) -> Result<()> {
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
    let renderer =
        TuiRenderer::new(CatalogTable::new(cells, columns), rx).with_cancel_signal(cancel);
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
                branch: row.default_branch.clone(),
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
    columns: &[RepoListColumn],
) -> EmitPayload {
    // Mirrors `daft list`'s emit semantics: the default column set emits the
    // full cheap field set — including default_branch, which has no default
    // table column — while a customized selection narrows the fields to
    // match. size_bytes appears only when the size column was selected;
    // removed_at is row status, not a column, and is always present.
    let is_default = columns == RepoListColumn::repo_list_defaults();
    let has = |col: RepoListColumn| is_default || columns.contains(&col);

    let mut headers = Vec::new();
    if has(RepoListColumn::Name) {
        headers.push("name");
    }
    if has(RepoListColumn::Worktrees) {
        headers.push("worktrees");
    }
    if has(RepoListColumn::Branch) {
        headers.push("default_branch");
    }
    if has(RepoListColumn::Path) {
        headers.push("path");
    }
    if has(RepoListColumn::Remote) {
        headers.push("remote_url");
    }
    headers.push("removed_at");
    if sizes.is_some() {
        headers.push("size_bytes");
    }

    let mut table = Table::new(headers);
    for (i, row) in rows.iter().enumerate() {
        let mut record = Vec::new();
        if has(RepoListColumn::Name) {
            record.push(Cell::str(&row.name));
        }
        if has(RepoListColumn::Worktrees) {
            record.push(
                cells[i]
                    .worktrees
                    .map(|n| Cell::int(n as i64))
                    .unwrap_or(Cell::Null),
            );
        }
        if has(RepoListColumn::Branch) {
            record.push(
                row.default_branch
                    .as_deref()
                    .map(Cell::str)
                    .unwrap_or(Cell::Null),
            );
        }
        if has(RepoListColumn::Path) {
            record.push(Cell::str(&row.path));
        }
        if has(RepoListColumn::Remote) {
            record.push(
                row.remote_url
                    .as_deref()
                    .map(Cell::str)
                    .unwrap_or(Cell::Null),
            );
        }
        record.push(
            row.removed_at
                .map(|t| Cell::str(t.to_rfc3339()))
                .unwrap_or(Cell::Null),
        );
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
    columns: &[RepoListColumn],
    use_color: bool,
    term_width: Option<usize>,
) -> String {
    use tabled::builder::Builder;
    use tabled::settings::peaker::Priority;
    use tabled::settings::{Padding, Style, Width, object::Columns};

    // The annotation column collapses when no row is current, keeping piped
    // output free of a phantom leading column.
    let annotation =
        columns.contains(&RepoListColumn::Annotation) && cells.iter().any(|c| c.current);
    let data_columns: Vec<RepoListColumn> = columns
        .iter()
        .copied()
        .filter(|c| *c != RepoListColumn::Annotation)
        .collect();
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
    headers.extend(data_columns.iter().map(|c| {
        let label = c.header_label();
        if use_color {
            styles::dim_underline(label)
        } else {
            label.to_string()
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
        for col in &data_columns {
            record.push(match col {
                RepoListColumn::Annotation => unreachable!("filtered above"),
                RepoListColumn::Name => dim(&cell.name, cell.removed),
                RepoListColumn::Worktrees => dim(
                    &cell
                        .worktrees
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    cell.removed,
                ),
                RepoListColumn::Branch => dim(cell.branch.as_deref().unwrap_or("-"), cell.removed),
                RepoListColumn::Path => dim(&cell.path, cell.removed),
                RepoListColumn::Size => dim(
                    &sizes
                        .and_then(|s| s[i])
                        .map(format_human_size)
                        .unwrap_or_else(|| "-".to_string()),
                    cell.removed,
                ),
                // Remote is reference info, not the primary signal — always dim.
                RepoListColumn::Remote => dim(cell.remote.as_deref().unwrap_or("-"), true),
            });
        }
        builder.push_record(record);
    }

    // TOTAL footer under the Size column, matching `daft list`.
    let size_idx = data_columns
        .iter()
        .position(|c| *c == RepoListColumn::Size)
        .map(|p| p + usize::from(annotation));
    if let (Some(sizes), Some(size_idx)) = (sizes, size_idx) {
        let total: u64 = sizes.iter().filter_map(|s| *s).sum();
        let total_cell = dim(&format_human_size(total), true);
        let cols_total = data_columns.len() + usize::from(annotation);
        let mut separator: Vec<String> = vec![String::new(); cols_total];
        builder.push_record(separator.clone());
        separator[size_idx] = total_cell;
        builder.push_record(separator);
    }

    let mut table = builder.build();
    table.with(Style::blank());
    table.modify(Columns::first(), Padding::new(1, 0, 0, 0));

    if let Some(width) = term_width {
        match size_idx {
            // Exclude the Size column from the shrink candidate set — its
            // TOTAL summary cell can be wider than any data cell and gets
            // truncated otherwise (#501, same as `daft list`).
            Some(size_idx) if sizes.is_some() => table.with(
                Width::truncate(width)
                    .suffix("...")
                    .priority(PriorityMaxExcept {
                        excluded: vec![size_idx],
                    }),
            ),
            _ => table.with(
                Width::truncate(width)
                    .suffix("...")
                    .priority(Priority::max(true)),
            ),
        };
    }

    let rendered = table.to_string();
    if !use_color {
        return rendered;
    }

    // Background-highlight the current repo's row, matching the live tables
    // (worktree and catalog both paint `CURRENT_ROW_BG_INDEX`). Line 0 is the
    // header, data row i renders on line 1 + i, and any TOTAL footer lines
    // fall past the cell count, so they never match.
    rendered
        .lines()
        .enumerate()
        .map(|(i, line)| {
            let current = i
                .checked_sub(1)
                .and_then(|idx| cells.get(idx))
                .is_some_and(|c| c.current);
            if current {
                styles::paint_current_row(line)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
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
            branch: Some("main".to_string()),
            path: format!("~/src/{name}"),
            remote: Some(format!("git@example.com:acme/{name}.git")),
        }
    }

    fn defaults() -> Vec<RepoListColumn> {
        RepoListColumn::repo_list_defaults().to_vec()
    }

    fn defaults_plus_size() -> Vec<RepoListColumn> {
        RepoColumnSelection::parse("+size").unwrap().columns
    }

    /// Regression: the first shipped renderer emitted every line through
    /// `output.raw()` (`print!`, no newline), fusing header and rows into a
    /// single line. The table must be one line per row plus the header.
    #[test]
    fn blocking_table_emits_header_and_one_line_per_row() {
        let table = build_blocking_table(
            &[cells("alpha", false, false), cells("beta", false, false)],
            None,
            &defaults(),
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
        assert!(!lines[0].contains("Branch"), "Branch column is opt-in");
        assert!(lines[1].contains("alpha"));
        assert!(lines[1].contains("git@example.com:acme/alpha.git"));
        assert!(lines[2].contains("beta"));
    }

    #[test]
    fn blocking_table_marks_the_current_repo() {
        let table = build_blocking_table(
            &[cells("alpha", true, false), cells("beta", false, false)],
            None,
            &defaults(),
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
        let table = build_blocking_table(
            &[cells("alpha", false, false)],
            None,
            &defaults(),
            false,
            None,
        );
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
            &defaults_plus_size(),
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

    /// The size column's canonical slot is between Path and Remote, so the
    /// TOTAL cell must land mid-row, not in the last column.
    #[test]
    fn blocking_table_total_lands_under_the_size_column() {
        let table = build_blocking_table(
            &[cells("alpha", false, false)],
            Some(&[Some(1024)]),
            &defaults_plus_size(),
            false,
            None,
        );
        let lines: Vec<&str> = table.lines().collect();
        let header = lines[0];
        let size_start = header.find("Size").expect("Size header present");
        let total_start = lines[3].find("1K").expect("total rendered");
        assert!(
            total_start.abs_diff(size_start) <= 4,
            "total should sit under the Size header (size at {size_start}, total at {total_start}): {table:?}"
        );
        assert!(
            header.find("Remote").expect("Remote header present") > size_start,
            "Remote renders after Size: {header:?}"
        );
    }

    #[test]
    fn blocking_table_replace_mode_narrows_columns() {
        let table = build_blocking_table(
            &[cells("alpha", true, false)],
            None,
            &[RepoListColumn::Name, RepoListColumn::Path],
            false,
            None,
        );
        let lines: Vec<&str> = table.lines().collect();
        assert!(lines[0].contains("Name") && lines[0].contains("Path"));
        assert!(
            !lines[0].contains("Remote") && !lines[0].contains("Worktrees"),
            "replace mode drops unselected columns: {:?}",
            lines[0]
        );
        assert!(
            !lines[1]
                .trim_start()
                .starts_with(styles::CURRENT_WORKTREE_SYMBOL),
            "annotation dropped when not selected: {:?}",
            lines[1]
        );
    }

    #[test]
    fn blocking_table_branch_column_is_opt_in() {
        let columns = RepoColumnSelection::parse("+branch").unwrap().columns;
        let table =
            build_blocking_table(&[cells("alpha", false, false)], None, &columns, false, None);
        let lines: Vec<&str> = table.lines().collect();
        assert!(lines[0].contains("Branch"), "header: {:?}", lines[0]);
        assert!(lines[1].contains("main"), "row: {:?}", lines[1]);
    }

    /// Full parity with `daft list`: the current repo's row carries the
    /// dark-gray background in colored output, continuous across the styled
    /// cells (every RESET re-asserts it).
    #[test]
    fn blocking_table_paints_current_row_background_when_colored() {
        let table = build_blocking_table(
            &[cells("alpha", true, false), cells("beta", false, false)],
            None,
            &defaults(),
            true,
            None,
        );
        let lines: Vec<&str> = table.lines().collect();
        assert!(
            lines[1].starts_with(styles::CURRENT_ROW_BG),
            "current row painted: {:?}",
            lines[1]
        );
        assert!(
            lines[1].contains(&format!("{}{}", styles::RESET, styles::CURRENT_ROW_BG)),
            "background re-asserted after cell resets: {:?}",
            lines[1]
        );
        assert!(lines[1].ends_with(styles::RESET));
        assert!(
            !lines[0].contains(styles::CURRENT_ROW_BG),
            "header unpainted: {:?}",
            lines[0]
        );
        assert!(
            !lines[2].contains(styles::CURRENT_ROW_BG),
            "other rows unpainted: {:?}",
            lines[2]
        );
    }

    #[test]
    fn blocking_table_shows_dash_for_missing_count_size_and_remote() {
        let mut cell = cells("alpha", false, true);
        cell.worktrees = None;
        cell.remote = None;
        let table =
            build_blocking_table(&[cell], Some(&[None]), &defaults_plus_size(), false, None);
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

    fn sample_row() -> CatalogRepoRow {
        CatalogRepoRow {
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
        }
    }

    fn payload_headers(payload: EmitPayload) -> Vec<String> {
        let EmitPayload::Tabular(table) = payload else {
            panic!("expected tabular payload");
        };
        table.headers
    }

    #[test]
    fn payload_default_columns_emit_the_full_field_set() {
        let row = sample_row();
        let cell = cells("alpha", false, false);

        let headers = payload_headers(build_payload(
            std::slice::from_ref(&row),
            std::slice::from_ref(&cell),
            None,
            &defaults(),
        ));
        assert_eq!(
            headers,
            [
                "name",
                "worktrees",
                "default_branch",
                "path",
                "remote_url",
                "removed_at"
            ]
        );

        let headers = payload_headers(build_payload(
            std::slice::from_ref(&row),
            std::slice::from_ref(&cell),
            Some(&[Some(42)]),
            &defaults_plus_size(),
        ));
        assert_eq!(headers.last().map(String::as_str), Some("size_bytes"));
    }

    /// Mirrors `daft list`'s EmitColumns: a customized selection narrows the
    /// emitted fields; removed_at rides along as row status.
    #[test]
    fn payload_narrows_to_a_customized_selection() {
        let row = sample_row();
        let cell = cells("alpha", false, false);

        let headers = payload_headers(build_payload(
            std::slice::from_ref(&row),
            std::slice::from_ref(&cell),
            None,
            &[RepoListColumn::Name, RepoListColumn::Path],
        ));
        assert_eq!(headers, ["name", "path", "removed_at"]);
    }
}
