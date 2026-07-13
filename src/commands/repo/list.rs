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
use crate::catalog::worktrees::{WorktreeChild, worktree_children};
use crate::commands::list::PriorityMaxExcept;
use crate::core::columns::{RepoColumnSelection, RepoListColumn, ResolvedColumns};
use crate::output::emit::{self, Cell, EmitArgs, EmitPayload, Table};
use crate::output::format::{display_path, format_human_size};
use crate::output::tui::{
    CatalogEvent, CatalogRepoCells, CatalogTable, CatalogWorktreeCells, LiveScreen, TuiRenderer,
    tree_glyph,
};
use crate::output::{CliOutput, Output, OutputConfig};
use crate::store::{CatalogRepoRow, RepoSizeRow};
use crate::styles;
use std::collections::HashMap;

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
  Add optional:  --columns +size,+layout,+branch (add to defaults)

The size column is not shown by default — it walks every repository, so it
is opt-in, same as the worktree commands. On a terminal the sizes stream in
live while the table renders immediately, with a total row summing them.
The recorded worktree layout (+layout) and default branch (+branch) are
likewise opt-in; structured output includes both by default.

With --worktrees, each repository expands into its worktrees — one tree line
per worktree with its branch and checkout path. Structured output then nests
a worktrees array per repository in place of the count, which narrows the
supported formats to json, yaml, toon, and markdown.
"#)]
pub struct Args {
    #[arg(short = 'a', long = "all", help = "Include removed repositories")]
    all: bool,

    #[arg(
        short = 'w',
        long = "worktrees",
        help = "Expand each repository with its worktrees"
    )]
    worktrees: bool,

    #[arg(
        long = "columns",
        help = "Columns to display (comma-separated). Replace: name,path,remote. Modify defaults: +col,-col. Available: annotation, name, worktrees, layout, branch, path, size, remote"
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
    // Field set for structured output: the full default set unless the user
    // gave a replace-mode selection. A modifier like `+size` keeps the
    // defaults (explicit == false), so it must NOT narrow the JSON fields.
    let is_default_fields = !resolved.explicit;
    let columns = resolved.columns;
    let has_size = columns.contains(&RepoListColumn::Size);

    let rows = match Catalog::open_ro().context("could not open the repo catalog")? {
        Some(catalog) => catalog.list(args.all)?,
        None => Vec::new(),
    };

    let location = current_location();
    let children = args
        .worktrees
        .then(|| collect_worktree_children(&rows, location.workdir.as_deref()));
    let cells = build_display_cells(&rows, children.as_deref(), &location);

    if args.emit.is_structured() {
        // Structured consumers get everything in one shot — walks run
        // synchronously when requested.
        let sizes = has_size.then(|| compute_sizes(&rows, resolve_size_jobs()));
        // Warm the size cache from the synchronous walk too (write-only), so
        // structured/piped runs leave the same last-known sizes a later live
        // run seeds from.
        if let Some(sizes) = sizes.as_deref() {
            persist_repo_sizes(&rows, sizes);
        }
        let payload = match &children {
            Some(children) => build_document_payload(
                &rows,
                &cells,
                children,
                sizes.as_deref(),
                &columns,
                is_default_fields,
            ),
            None => build_payload(&rows, &cells, sizes.as_deref(), &columns, is_default_fields),
        };
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

    let sizes = has_size.then(|| compute_sizes(&rows, resolve_size_jobs()));
    // Warm the size cache from the blocking walk too (write-only), matching
    // the live path and the structured branch above.
    if let Some(sizes) = sizes.as_deref() {
        persist_repo_sizes(&rows, sizes);
    }
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

    let paths: Vec<PathBuf> = rows.iter().map(|row| PathBuf::from(&row.path)).collect();

    // One coordinator thread runs the shared bounded walker over every repo,
    // streaming each repo's size the moment its walk finishes, then the Done
    // sentinel. Replaces the old unbounded thread-per-repo spawn: total
    // disk-metadata concurrency is now capped at the shared job budget, and a
    // single deep repo also parallelises internally. The walker checks `cancel`
    // cooperatively between directories. The Done sentinel must fire while the
    // renderer is listening, so this all runs on its own thread.
    let walk_cancel = Arc::clone(&cancel);
    let join_thread = std::thread::spawn(move || {
        crate::core::size_walk::walk_streaming(
            &paths,
            Some(&walk_cancel),
            resolve_size_jobs(),
            |index, bytes| {
                let _ = tx.send(CatalogEvent::Size { index, bytes });
            },
        );
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

    // Size cache: seed each repo's Size cell with its last-known value
    // (rendered dim/stale) so the column shows a figure instantly while the
    // walk refreshes it. Keyed by catalog uuid, aligned to `rows` by index.
    let cache = read_repo_size_cache();
    let stale: Vec<Option<u64>> = rows.iter().map(|r| cache.get(&r.uuid).copied()).collect();

    // Raw mode routes Ctrl-C into the render loop as a key event; the RAII
    // guard restores cooked mode on every exit path.
    let _raw_guard = crate::output::tui::enable_raw_mode_guard();
    let renderer = TuiRenderer::new(
        CatalogTable::new(cells, columns).with_stale_sizes(stale),
        rx,
    )
    .with_cancel_signal(cancel);
    let screen = renderer.run()?;

    // Persist the freshly-walked sizes so the next run seeds from them. Only
    // cells that actually loaded this run are written (a stale value that
    // never refreshed keeps its `measured_at`); the helper stat-guards each
    // path so a removed repo can't clobber a good cached size with `Some(0)`.
    persist_repo_sizes(rows, &screen.loaded_sizes());

    // On normal completion the walkers are already done and the join returns
    // immediately. On cancellation, walks may still be mid-flight — skip the
    // join so the user gets an instant prompt back; the OS reaps the
    // read-only walker threads when the process exits.
    if !screen.is_cancelled() {
        let _ = join_thread.join();
    }
    Ok(())
}

/// Last-known repo sizes keyed by catalog uuid, for seeding the live table's
/// Size cells up front. Best-effort: a missing catalog, or an old one without
/// the `repo_sizes` table, yields an empty map (today's shimmer).
fn read_repo_size_cache() -> HashMap<String, u64> {
    Catalog::open_ro()
        .ok()
        .flatten()
        .and_then(|cat| cat.list_repo_sizes().ok())
        .map(|rows| rows.into_iter().map(|r| (r.uuid, r.size_bytes)).collect())
        .unwrap_or_default()
}

/// Persist freshly-walked repo sizes (`loaded` is index-aligned with `rows`).
/// Batched in one transaction; best-effort — a store/write failure is
/// swallowed.
fn persist_repo_sizes(rows: &[CatalogRepoRow], loaded: &[Option<u64>]) {
    let to_persist = size_rows_to_persist(rows, loaded, chrono::Utc::now());
    if to_persist.is_empty() {
        return;
    }
    if let Ok(catalog) = Catalog::open_rw() {
        let _ = catalog.upsert_repo_sizes(&to_persist);
    }
}

/// Pure core of [`persist_repo_sizes`]: map index-aligned `(row, walked-size)`
/// pairs to the rows worth persisting. Keeps only `Some` sizes (a cell that
/// never walked is skipped) whose repo path still exists — the **stat-guard**,
/// so the walk's `Some(0)` for a vanished repo can't clobber a good cached
/// value. Split out so the filtering is testable without touching the catalog.
fn size_rows_to_persist(
    rows: &[CatalogRepoRow],
    loaded: &[Option<u64>],
    measured_at: chrono::DateTime<chrono::Utc>,
) -> Vec<RepoSizeRow> {
    rows.iter()
        .zip(loaded)
        .filter_map(|(row, size)| {
            let size_bytes = (*size)?;
            if !crate::commands::size_cache::should_persist(Path::new(&row.path)) {
                return None;
            }
            Some(RepoSizeRow {
                uuid: row.uuid.clone(),
                repo_path: row.path.clone(),
                size_bytes,
                measured_at,
            })
        })
        .collect()
}

/// Worktree count for a repo: the main working tree (if any) plus linked
/// worktrees — what `git worktree list` would show. `None` when the repo
/// can't be opened (stale path, removed repo).
fn worktree_count(git_common_dir: &Path) -> Option<usize> {
    let repo = gix::open(git_common_dir).ok()?;
    let linked = repo.worktrees().map(|w| w.len()).unwrap_or(0);
    Some(linked + usize::from(repo.workdir().is_some()))
}

/// Where the user is standing: the canonical cwd (anchor for relative
/// display paths), the canonical git-common-dir of the enclosing repo
/// (matched against catalog rows), and the canonical root of the enclosing
/// worktree (matched against enumerated children). The repo fields are
/// `None` outside any repo; `workdir` alone is `None` when standing in a
/// bare git dir — the row highlight then stays on the repo line.
struct CurrentLocation {
    cwd: Option<PathBuf>,
    git_common_dir: Option<PathBuf>,
    workdir: Option<PathBuf>,
}

fn current_location() -> CurrentLocation {
    // Canonicalized so comparisons and relativization survive macOS's
    // `/tmp` → `/private/tmp` symlink (catalog rows store canonical paths).
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|cwd| std::fs::canonicalize(cwd).ok());
    let discovered = cwd.as_ref().and_then(|cwd| gix::discover(cwd).ok());
    let git_common_dir = discovered
        .as_ref()
        .and_then(|repo| std::fs::canonicalize(repo.common_dir()).ok());
    let workdir = discovered
        .as_ref()
        .and_then(|repo| repo.workdir())
        .and_then(|w| std::fs::canonicalize(w).ok());
    CurrentLocation {
        cwd,
        git_common_dir,
        workdir,
    }
}

/// Enumerate every repo's worktrees (`--worktrees`). A repo that can't be
/// opened (stale path, removed entry) yields `None`, mirroring
/// `worktree_count` — the table shows `-` and structured output `null`.
fn collect_worktree_children(
    rows: &[CatalogRepoRow],
    current_workdir: Option<&Path>,
) -> Vec<Option<Vec<WorktreeChild>>> {
    rows.iter()
        .map(|row| worktree_children(row, current_workdir))
        .collect()
}

fn build_display_cells(
    rows: &[CatalogRepoRow],
    children: Option<&[Option<Vec<WorktreeChild>>]>,
    location: &CurrentLocation,
) -> Vec<CatalogRepoCells> {
    let cwd = location.cwd.as_deref();
    // One read-only load serves every row's layout lookup. The repo store
    // (repos.json) is where clone/adopt/`layout set` record each repo's
    // layout; repos daft never laid out simply have no entry.
    let trust_db = crate::hooks::TrustDatabase::load().ok();
    rows.iter()
        .enumerate()
        .map(|(i, row)| {
            let current = location.git_common_dir.as_deref().is_some_and(|cur| {
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
                // With children enumerated, the count derives from them —
                // one repo open instead of two, and the Worktrees column
                // can never disagree with the tree below it.
                worktrees: match children {
                    Some(children) => children[i].as_ref().map(Vec::len),
                    None => worktree_count(Path::new(&row.git_common_dir)),
                },
                layout: trust_db
                    .as_ref()
                    .and_then(|db| db.get_layout(Path::new(&row.git_common_dir)))
                    .map(String::from),
                branch: row.default_branch.clone(),
                path: display_path(&row.path, cwd),
                remote: row.remote_url.clone(),
                children: children
                    .and_then(|children| children[i].as_deref())
                    .unwrap_or_default()
                    .iter()
                    .map(|c| CatalogWorktreeCells {
                        current: c.current,
                        branch: c.branch.clone(),
                        path: display_path(&c.path, cwd),
                    })
                    .collect(),
            }
        })
        .collect()
}

/// Resolve the size-walk concurrency for `daft repo list`, honoring
/// `daft.list.sizeConcurrency`. Read from *global* git config: repo list is a
/// cross-repo command that can run outside any repo (like `repo remove`), so
/// there is no single local repo whose config would apply. `DAFT_SIZE_WALK_JOBS`
/// still overrides via `resolve_jobs`, and a missing/invalid config falls back
/// to `available_parallelism()`.
fn resolve_size_jobs() -> usize {
    let settings = crate::core::settings::DaftSettings::load_global().unwrap_or_default();
    size_jobs_for(&settings)
}

/// Pure core of [`resolve_size_jobs`]: derive the walk budget from settings so
/// the "repo list honors `daft.list.sizeConcurrency`" wiring is testable
/// without touching (forbidden) global git config. `DAFT_SIZE_WALK_JOBS` still
/// wins inside `resolve_jobs`.
fn size_jobs_for(settings: &crate::core::settings::DaftSettings) -> usize {
    crate::core::size_walk::resolve_jobs(settings.list_size_concurrency)
}

fn compute_sizes(rows: &[CatalogRepoRow], jobs: usize) -> Vec<Option<u64>> {
    let paths: Vec<PathBuf> = rows.iter().map(|row| PathBuf::from(&row.path)).collect();
    crate::core::size_walk::walk_all(&paths, None, jobs)
}

/// Whether a structured field is emitted: the full default set unless the user
/// gave a replace-mode column selection, in which case only the selected
/// columns appear. Shared by the tabular and document payload builders so the
/// two structured shapes can't drift on which fields they carry. A modifier
/// selection (e.g. `+size`) keeps `is_default` true, so it adds a column
/// without dropping any default field.
fn field_present(
    columns: &[RepoListColumn],
    is_default: bool,
) -> impl Fn(RepoListColumn) -> bool + '_ {
    move |col| is_default || columns.contains(&col)
}

fn build_payload(
    rows: &[CatalogRepoRow],
    cells: &[CatalogRepoCells],
    sizes: Option<&[Option<u64>]>,
    columns: &[RepoListColumn],
    is_default: bool,
) -> EmitPayload {
    // Mirrors `daft list`'s emit semantics: the default field set includes
    // default_branch (which has no default table column); a replace-mode
    // selection narrows to match. size_bytes appears only when the size column
    // was selected; removed_at is row status, not a column, always present.
    let has = field_present(columns, is_default);

    let mut headers = Vec::new();
    if has(RepoListColumn::Name) {
        headers.push("name");
    }
    if has(RepoListColumn::Worktrees) {
        headers.push("worktrees");
    }
    if has(RepoListColumn::Layout) {
        headers.push("layout");
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
        if has(RepoListColumn::Layout) {
            record.push(
                cells[i]
                    .layout
                    .as_deref()
                    .map(Cell::str)
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

/// The `--worktrees` structured payload: the same repo fields as the tabular
/// emit (narrowed by a customized `--columns` the same way), except
/// `worktrees` becomes the enumerated array — `{branch, path}` per worktree,
/// raw paths, `null` branch for a detached HEAD — or `null` when the repo
/// couldn't be opened. Arrays don't fit `Cell`, hence the Document shape;
/// tabular-only formats (tsv/csv/ndjson) are rejected by the emit dispatch
/// with the standard supported-formats error.
fn build_document_payload(
    rows: &[CatalogRepoRow],
    cells: &[CatalogRepoCells],
    children: &[Option<Vec<WorktreeChild>>],
    sizes: Option<&[Option<u64>]>,
    columns: &[RepoListColumn],
    is_default: bool,
) -> EmitPayload {
    use serde_json::{Map, Value, json};

    let has = field_present(columns, is_default);

    let repos: Vec<Value> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mut obj = Map::new();
            if has(RepoListColumn::Name) {
                obj.insert("name".into(), json!(row.name));
            }
            // The array is the point of --worktrees: always emitted, even
            // under a narrowed column selection.
            obj.insert(
                "worktrees".into(),
                match &children[i] {
                    Some(list) => Value::Array(
                        list.iter()
                            .map(|c| json!({ "branch": c.branch, "path": c.path }))
                            .collect(),
                    ),
                    None => Value::Null,
                },
            );
            if has(RepoListColumn::Layout) {
                obj.insert("layout".into(), json!(cells[i].layout));
            }
            if has(RepoListColumn::Branch) {
                obj.insert("default_branch".into(), json!(row.default_branch));
            }
            if has(RepoListColumn::Path) {
                obj.insert("path".into(), json!(row.path));
            }
            if has(RepoListColumn::Remote) {
                obj.insert("remote_url".into(), json!(row.remote_url));
            }
            obj.insert(
                "removed_at".into(),
                row.removed_at
                    .map(|t| json!(t.to_rfc3339()))
                    .unwrap_or(Value::Null),
            );
            if let Some(sizes) = sizes {
                obj.insert("size_bytes".into(), json!(sizes[i]));
            }
            Value::Object(obj)
        })
        .collect();
    EmitPayload::Document(Value::Array(repos))
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

    // Children anchor their tree glyph in the Name column; without it there
    // is nowhere coherent to hang them, so they are omitted.
    let name_selected = data_columns.contains(&RepoListColumn::Name);
    // Which rendered data line carries the current-row background: the
    // current worktree child when rendered (`--worktrees`), else its repo's
    // own row — exactly one line either way. Pushed in lockstep with the
    // records so index k maps to rendered line 1 + k.
    let mut paint_rows: Vec<bool> = Vec::new();

    let mut builder = Builder::new();
    builder.push_record(headers);
    for (i, cell) in cells.iter().enumerate() {
        let current_child_rendered = name_selected && cell.children.iter().any(|c| c.current);
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
                RepoListColumn::Layout => dim(cell.layout.as_deref().unwrap_or("-"), cell.removed),
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
        paint_rows.push(cell.current && !current_child_rendered);

        if name_selected {
            let child_total = cell.children.len();
            for (ci, child) in cell.children.iter().enumerate() {
                let mut child_record: Vec<String> = Vec::new();
                if annotation {
                    child_record.push(String::new());
                }
                for col in &data_columns {
                    child_record.push(match col {
                        RepoListColumn::Name => format!(
                            "{}{}",
                            dim(tree_glyph(ci, child_total), true),
                            dim(child.branch_label(), cell.removed),
                        ),
                        // Child paths are reference info under the repo's own
                        // path — always dim, like Remote.
                        RepoListColumn::Path => dim(&child.path, true),
                        _ => String::new(),
                    });
                }
                builder.push_record(child_record);
                paint_rows.push(child.current);
            }
        }
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

    // Background-highlight the current row, matching the live tables
    // (worktree and catalog both paint `CURRENT_ROW_BG_INDEX`). Line 0 is
    // the header, data line 1 + k maps to `paint_rows[k]` (repos and their
    // worktree children were pushed in lockstep with it), and any TOTAL
    // footer lines fall past the vec, so they never match.
    rendered
        .lines()
        .enumerate()
        .map(|(i, line)| {
            let paint = i
                .checked_sub(1)
                .and_then(|idx| paint_rows.get(idx))
                .copied()
                .unwrap_or(false);
            if paint {
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
            layout: Some("contained".to_string()),
            branch: Some("main".to_string()),
            path: format!("~/src/{name}"),
            remote: Some(format!("git@example.com:acme/{name}.git")),
            children: Vec::new(),
        }
    }

    fn child(branch: Option<&str>, current: bool) -> CatalogWorktreeCells {
        CatalogWorktreeCells {
            current,
            branch: branch.map(String::from),
            path: format!("~/src/x/{}", branch.unwrap_or("detached")),
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

    #[test]
    fn blocking_table_layout_column_is_opt_in() {
        let table = build_blocking_table(
            &[cells("alpha", false, false)],
            None,
            &defaults(),
            false,
            None,
        );
        assert!(
            !table.lines().next().unwrap().contains("Layout"),
            "layout column is opt-in: {table:?}"
        );

        let columns = RepoColumnSelection::parse("+layout").unwrap().columns;
        let mut unrecorded = cells("beta", false, false);
        unrecorded.layout = None;
        let table = build_blocking_table(
            &[cells("alpha", false, false), unrecorded],
            None,
            &columns,
            false,
            None,
        );
        let lines: Vec<&str> = table.lines().collect();
        assert!(lines[0].contains("Layout"), "header: {:?}", lines[0]);
        assert!(lines[1].contains("contained"), "row: {:?}", lines[1]);
        assert!(
            lines[2].contains('-'),
            "unrecorded layout renders '-': {:?}",
            lines[2]
        );
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

    /// `--worktrees`: children interleave as grid rows under their repo —
    /// glyph + branch in the Name column, path in the Path column, the last
    /// child closing the tree with `└`.
    #[test]
    fn blocking_table_expands_worktree_children() {
        let mut alpha = cells("alpha", false, false);
        alpha.children = vec![child(Some("main"), false), child(Some("feat/login"), false)];
        let table = build_blocking_table(
            &[alpha, cells("beta", false, false)],
            None,
            &defaults(),
            false,
            None,
        );
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 5, "header + 2 repos + 2 children: {table:?}");
        assert!(lines[1].contains("alpha"));
        assert!(
            lines[2].contains("\u{251C} main") && lines[2].contains("~/src/x/main"),
            "first child continues the tree with its path: {:?}",
            lines[2]
        );
        assert!(
            lines[3].contains("\u{2514} feat/login"),
            "last child closes the tree: {:?}",
            lines[3]
        );
        assert!(lines[4].contains("beta"), "next repo follows the children");
        let header = lines[0];
        let path_col = header.find("Path").expect("Path header");
        let child_path = lines[2].find("~/src/x/main").expect("child path rendered");
        assert!(
            child_path.abs_diff(path_col) <= 2,
            "child path aligns under the Path column (path at {path_col}, child at {child_path}): {table:?}"
        );
    }

    #[test]
    fn blocking_table_detached_child_renders_placeholder() {
        let mut alpha = cells("alpha", false, false);
        alpha.children = vec![child(None, false)];
        let table = build_blocking_table(&[alpha], None, &defaults(), false, None);
        assert!(
            table
                .lines()
                .nth(2)
                .unwrap()
                .contains("\u{2514} (detached)"),
            "detached HEAD renders the placeholder: {table:?}"
        );
    }

    /// Children hang off the Name column; a selection without it renders
    /// plain repo rows only.
    #[test]
    fn blocking_table_children_require_the_name_column() {
        let mut alpha = cells("alpha", false, false);
        alpha.children = vec![child(Some("main"), false)];
        let table = build_blocking_table(
            &[alpha],
            None,
            &[RepoListColumn::Path, RepoListColumn::Remote],
            false,
            None,
        );
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 2, "header + repo row only: {table:?}");
        assert!(!table.contains('\u{2514}'), "no tree glyphs: {table:?}");
    }

    /// With children rendered, the row highlight moves to the current
    /// worktree's line; the repo row keeps only the `>` marker. Exactly one
    /// line is painted.
    #[test]
    fn blocking_table_paints_the_current_child_not_the_repo() {
        let mut alpha = cells("alpha", true, false);
        alpha.children = vec![child(Some("main"), false), child(Some("feat/login"), true)];
        let table = build_blocking_table(&[alpha], None, &defaults(), true, None);
        let lines: Vec<&str> = table.lines().collect();
        assert!(
            !lines[1].contains(styles::CURRENT_ROW_BG),
            "repo row cedes the highlight: {:?}",
            lines[1]
        );
        assert!(
            lines[1].contains(styles::CURRENT_WORKTREE_SYMBOL),
            "repo row keeps the marker: {:?}",
            lines[1]
        );
        assert!(
            lines[3].starts_with(styles::CURRENT_ROW_BG),
            "current worktree line painted: {:?}",
            lines[3]
        );
        let painted = lines
            .iter()
            .filter(|l| l.contains(styles::CURRENT_ROW_BG))
            .count();
        assert_eq!(painted, 1, "exactly one highlighted line: {table:?}");
    }

    /// Standing in the repo but not in any enumerated worktree (bare git
    /// dir) keeps the highlight on the repo row.
    #[test]
    fn blocking_table_paint_falls_back_to_the_repo_row() {
        let mut alpha = cells("alpha", true, false);
        alpha.children = vec![child(Some("main"), false)];
        let table = build_blocking_table(&[alpha], None, &defaults(), true, None);
        let lines: Vec<&str> = table.lines().collect();
        assert!(
            lines[1].starts_with(styles::CURRENT_ROW_BG),
            "repo row keeps the highlight: {:?}",
            lines[1]
        );
        assert!(
            !lines[2].contains(styles::CURRENT_ROW_BG),
            "children stay unpainted: {:?}",
            lines[2]
        );
    }

    /// `--worktrees` structured output: Document array with a nested
    /// worktree list per repo — raw paths, null branch when detached, null
    /// list when the repo couldn't be opened — and the array survives a
    /// narrowed column selection.
    #[test]
    fn document_payload_nests_worktree_arrays() {
        let row = sample_row();
        let cell = cells("alpha", false, false);
        let children = vec![Some(vec![
            WorktreeChild {
                branch: Some("main".to_string()),
                path: "/tmp/alpha/main".to_string(),
                current: false,
            },
            WorktreeChild {
                branch: None,
                path: "/tmp/alpha/detached".to_string(),
                current: false,
            },
        ])];

        let payload = build_document_payload(
            std::slice::from_ref(&row),
            std::slice::from_ref(&cell),
            &children,
            None,
            &defaults(),
            true,
        );
        let EmitPayload::Document(value) = payload else {
            panic!("expected a document payload");
        };
        let repo = &value.as_array().expect("array of repos")[0];
        assert_eq!(repo["name"], "alpha");
        assert_eq!(repo["layout"], "contained");
        assert_eq!(repo["default_branch"], "main");
        assert_eq!(repo["worktrees"][0]["branch"], "main");
        assert_eq!(repo["worktrees"][0]["path"], "/tmp/alpha/main");
        assert!(repo["worktrees"][1]["branch"].is_null());

        // A narrowed selection still carries the worktree array.
        let payload = build_document_payload(
            std::slice::from_ref(&row),
            std::slice::from_ref(&cell),
            &children,
            None,
            &[RepoListColumn::Name],
            false,
        );
        let EmitPayload::Document(value) = payload else {
            panic!("expected a document payload");
        };
        let repo = &value.as_array().unwrap()[0];
        assert!(repo.get("path").is_none(), "narrowing still applies");
        assert_eq!(repo["worktrees"].as_array().map(Vec::len), Some(2));

        // Unopenable repo: worktrees is null, not an empty array.
        let payload = build_document_payload(
            std::slice::from_ref(&row),
            std::slice::from_ref(&cell),
            &[None],
            None,
            &defaults(),
            true,
        );
        let EmitPayload::Document(value) = payload else {
            panic!("expected a document payload");
        };
        assert!(value.as_array().unwrap()[0]["worktrees"].is_null());
    }

    #[test]
    fn worktree_count_is_none_outside_a_repo() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(worktree_count(tmp.path()), None);
    }

    #[test]
    fn size_rows_to_persist_applies_stat_guard_and_skips_unwalked() {
        // present repo with a fresh size → persisted; a vanished path (the
        // walk's Some(0) case) → skipped by the stat-guard; a never-walked
        // None → skipped.
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().to_string_lossy().into_owned();
        let gone = dir.path().join("removed").to_string_lossy().into_owned();

        let mut present = sample_row();
        present.uuid = "u-present".into();
        present.path = existing.clone();
        let mut vanished = sample_row();
        vanished.uuid = "u-gone".into();
        vanished.path = gone;
        let mut unwalked = sample_row();
        unwalked.uuid = "u-unwalked".into();
        unwalked.path = existing;

        let rows = vec![present, vanished, unwalked];
        let now = chrono::Utc::now();
        let out = size_rows_to_persist(&rows, &[Some(4096), Some(0), None], now);

        assert_eq!(out.len(), 1, "only the present, walked repo persists");
        assert_eq!(out[0].uuid, "u-present");
        assert_eq!(out[0].size_bytes, 4096);
        assert_eq!(out[0].measured_at, now);
    }

    #[test]
    fn repo_list_size_jobs_honor_the_config() {
        use crate::core::settings::DaftSettings;

        // `DAFT_SIZE_WALK_JOBS` wins inside `resolve_jobs`; only assert the
        // config path when it's absent so a stray override can't flip this.
        if std::env::var_os("DAFT_SIZE_WALK_JOBS").is_some() {
            return;
        }
        // Regression: `daft repo list` used to pass `resolve_jobs(None)`,
        // silently ignoring `daft.list.sizeConcurrency`. The configured value
        // must now flow through to the walk budget.
        let with_config = DaftSettings {
            list_size_concurrency: Some(3),
            ..Default::default()
        };
        assert_eq!(
            size_jobs_for(&with_config),
            3,
            "repo list must honor daft.list.sizeConcurrency"
        );
        // No config → auto (available_parallelism), always >= 1.
        assert!(size_jobs_for(&DaftSettings::default()) >= 1);
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
            true,
        ));
        assert_eq!(
            headers,
            [
                "name",
                "worktrees",
                "layout",
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
            true,
        ));
        assert_eq!(headers.last().map(String::as_str), Some("size_bytes"));
    }

    /// #357 C4: a modifier `--columns +size` is not a replace selection
    /// (`explicit == false`), so structured output must keep the full default
    /// field set — layout and default_branch included — and merely add size,
    /// not silently drop them. Ties the parse → explicit → is_default wiring
    /// to the emitted field set.
    #[test]
    fn modifier_columns_keep_default_structured_fields() {
        let resolved = RepoColumnSelection::parse("+size").unwrap();
        assert!(!resolved.explicit, "a modifier is not a replace selection");
        let row = sample_row();
        let cell = cells("alpha", false, false);
        let headers = payload_headers(build_payload(
            std::slice::from_ref(&row),
            std::slice::from_ref(&cell),
            Some(&[Some(42)]),
            &resolved.columns,
            !resolved.explicit,
        ));
        assert_eq!(
            headers,
            [
                "name",
                "worktrees",
                "layout",
                "default_branch",
                "path",
                "remote_url",
                "removed_at",
                "size_bytes"
            ]
        );
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
            false,
        ));
        assert_eq!(headers, ["name", "path", "removed_at"]);
    }
}
