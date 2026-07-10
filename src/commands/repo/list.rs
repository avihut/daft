//! `daft repo list` — show the repo catalog.

use anyhow::{Context, Result};
use clap::Parser;

use crate::catalog::Catalog;
use crate::output::emit::{self, Cell, EmitArgs, EmitPayload, Table};
use crate::output::{CliOutput, Output, OutputConfig};
use crate::store::CatalogRepoRow;

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

    if args.emit.is_structured() {
        let payload = build_payload(&rows);
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

    render_table(&rows, &mut output);
    Ok(())
}

fn build_payload(rows: &[CatalogRepoRow]) -> EmitPayload {
    let mut table = Table::new(["name", "default_branch", "path", "remote_url", "removed_at"]);
    for row in rows {
        table = table.row([
            Cell::str(&row.name),
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
        ]);
    }
    EmitPayload::Tabular(table)
}

fn render_table(rows: &[CatalogRepoRow], output: &mut dyn Output) {
    let headers = ("NAME", "BRANCH", "PATH");
    let name_w = rows
        .iter()
        .map(|r| display_name(r).len())
        .chain([headers.0.len()])
        .max()
        .unwrap_or(0);
    let branch_w = rows
        .iter()
        .map(|r| r.default_branch.as_deref().unwrap_or("-").len())
        .chain([headers.1.len()])
        .max()
        .unwrap_or(0);

    output.raw(&format!(
        "{:<name_w$}  {:<branch_w$}  {}",
        headers.0, headers.1, headers.2
    ));
    for row in rows {
        output.raw(&format!(
            "{:<name_w$}  {:<branch_w$}  {}",
            display_name(row),
            row.default_branch.as_deref().unwrap_or("-"),
            row.path,
        ));
    }
}

fn display_name(row: &CatalogRepoRow) -> String {
    if row.removed_at.is_some() {
        format!("{} (removed)", row.name)
    } else {
        row.name.clone()
    }
}
