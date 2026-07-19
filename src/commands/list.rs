use crate::{
    core::{
        columns::{ColumnSelection, CommandKind, ListColumn, ResolvedColumns},
        repo::{get_current_worktree_path, get_git_common_dir, get_project_root},
        sort::SortSpec,
        worktree::list::{EntryKind, Stat, collect_branch_info, collect_worktree_info},
    },
    git::GitCommand,
    is_git_repository,
    logging::init_logging,
    output::{
        CliOutput, Output, OutputConfig,
        emit::{self, Cell, EmitArgs, EmitPayload, Table},
        format::{
            ColumnContext, compute_column_values, format_ahead_behind, format_head_status,
            format_human_size, format_remote_status, format_shorthand_age, relative_display_path,
            shorthand_from_seconds, strip_ansi,
        },
    },
    remote::get_default_branch_local,
    settings::DaftSettings,
    styles,
};
use anyhow::Result;
use chrono::Utc;
use clap::Parser;
use std::collections::HashSet;
use tabled::{
    builder::Builder,
    settings::{
        Padding, Style, Width,
        object::Columns,
        peaker::{Peaker, Priority},
    },
};
use terminal_size::{Width as TermWidth, terminal_size};

#[derive(Parser, Clone)]
#[command(name = "git-worktree-list")]
#[command(version = crate::VERSION)]
#[command(about = "List all worktrees with status information")]
#[command(long_about = r#"
Lists all worktrees in the current project with enriched status information
including uncommitted changes, ahead/behind counts vs. both the base branch
and the remote tracking branch, branch age, and last commit details.

Give a cataloged repository as the positional argument to list that
repository's worktrees from anywhere (sugar for `--repo`; the name must be
in the repo catalog). Use --all-repos to sweep every cataloged repository.

Each worktree is shown with:
  - A `>` marker for the current worktree
  - Branch name, with `✦` for the default branch
  - Relative path from the current directory
  - Ahead/behind counts vs. the base branch (e.g. +3 -1)
  - File status: +N staged, -N unstaged, ?N untracked
  - Remote tracking status: ⇡N unpushed, ⇣N unpulled
  - Branch age since creation (e.g. 3d, 2w, 5mo)
  - Last commit: shorthand age + subject (e.g. 1h fix login bug)

Ages use shorthand notation: <1m, Xm, Xh, Xd, Xw, Xmo, Xy.

Use -b / --branches to also show local branches without a worktree.
Use -r / --remotes to also show remote tracking branches.
Use -a / --all to show both (equivalent to -b -r).

Non-worktree branches are shown with dimmed styling and blank Path/Changes columns.

Use --stat lines to show line-level change counts (insertions and deletions)
instead of the default summary (commit counts for base/remote, file counts for
changes). This is slower as it requires computing diffs for each worktree.

Use --format to emit machine-readable output suitable for scripting.
Supported formats: json, ndjson, tsv, csv, yaml, toon, markdown. Use
--template '<tera>' for custom output. See the Structured Output guide
for details.

Use --columns to select which columns are shown and in what order.
  Replace mode:  --columns branch,path,age (exact set and order)
  Modifier mode: --columns -annotation,-last-commit (remove from defaults)
  Add optional:  --columns +size (add disk size column after path)
Defaults can be set in git config with daft.list.columns.

The size column is not shown by default. Add it with --columns +size to see the
disk size of each worktree folder in human-readable format (e.g. 42K, 1.3M, 2.5G).
A summary row at the bottom shows the total size across all worktrees.

The pr column shows the pull/merge request each row relates to (#123 for a
GitHub PR, !45 for a GitLab MR). It is on by default in repositories with a
GitHub or GitLab remote and disappears silently — persisting across runs —
when the forge integration is broken in a way that needs your intervention
(gh/glab missing or unauthenticated); it returns automatically once a
background refresh succeeds again. Repositories with no forge remote never
show it. Add --columns +pr to force the column regardless, or -pr to drop it.

While the pr column is shown, every open PR in the repository gets a row, not
just the ones your worktrees represent: a local branch with an open PR is
listed without --branches, and a PR with no local presence at all (a
colleague's branch, any fork PR) appears as a dimmed row built from the forge
data — fork PRs render owner:branch. Merged and closed PRs decorate existing
rows but never add one. Rows with a PR show the PR author in the Owner
column. The open-PR rows and the pr column are one unit: --columns -pr (or
the silent gate above) removes both, so prefer just your worktrees per-repo
with `git config -- daft.list.columns -pr`.

Use --sort to control the sort order. Prefix with + for ascending (default) or
- for descending. Multiple columns can be comma-separated for multi-level sort.
  Sort by branch descending:  --sort -branch
  Sort by owner then size:    --sort +owner,-size
  Most recent activity first: --sort -activity

Sortable columns: branch, path, size, age, owner, hash, activity, commit (alias:
last-commit). activity considers both commits and uncommitted file changes;
commit sorts by last commit time only. You can sort by columns not shown in
the output (e.g. --sort -size without --columns +size). Defaults can be set
with daft.list.sort.
"#)]
pub struct Args {
    /// Positional sugar for `--repo` — `daft list api`. Repo-only
    /// resolution (hard error with suggestions on a miss): the slot was
    /// previously an error, so no local meaning is shadowed, and `list`
    /// is read-only so a wrong guess costs nothing. Mutating fleet
    /// commands (update/exec/prune) deliberately do NOT get this sugar —
    /// see the repo-aware command grammar section in CLAUDE.md.
    #[arg(
        value_name = "REPO",
        conflicts_with_all = ["repo", "all_repos"],
        help = "Cataloged repository to list (same as --repo)"
    )]
    pub(crate) repo_arg: Option<String>,

    #[command(flatten)]
    pub(crate) emit: EmitArgs,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    pub(crate) verbose: bool,

    #[arg(
        short = 'b',
        long = "branches",
        help = "Also show local branches without a worktree"
    )]
    pub(crate) branches: bool,

    #[arg(
        short = 'r',
        long = "remotes",
        help = "Also show remote tracking branches"
    )]
    pub(crate) remotes: bool,

    #[arg(
        short = 'a',
        long = "all",
        help = "Show all branches (equivalent to -b -r)"
    )]
    pub(crate) all: bool,

    #[arg(
        long = "merging",
        help = "Only show worktrees with an in-progress merge"
    )]
    merging: bool,

    #[arg(
        long,
        value_enum,
        help = "Statistics mode: summary or lines (default: from git config daft.list.stat, or summary)"
    )]
    pub(crate) stat: Option<Stat>,

    #[arg(
        long,
        help = "Columns to display (comma-separated). Replace: branch,path,age. Modify defaults: +col,-col. Available: branch, path, size, base, changes, remote, pr, age, annotation, owner, hash, last-commit"
    )]
    pub(crate) columns: Option<String>,

    #[arg(
        long,
        help = "Sort order (comma-separated). +col ascending, -col descending. Columns: branch, path, size, base, changes, remote, age, owner, hash, activity, commit"
    )]
    pub(crate) sort: Option<String>,

    #[arg(
        long = "repo",
        value_name = "REPO",
        conflicts_with = "all_repos",
        help = "List another cataloged repository's worktrees"
    )]
    pub(crate) repo: Option<String>,

    #[arg(
        long = "all-repos",
        help = "List every cataloged repository's worktrees"
    )]
    pub(crate) all_repos: bool,
}

/// A row in the worktree list table.
struct TableRow {
    /// Annotation column: current marker (">") and/or default branch indicator ("✦").
    annotation: String,
    /// Branch name.
    name: String,
    /// Relative path from current directory.
    path: String,
    /// Human-readable disk size.
    size: String,
    /// Ahead/behind base branch (e.g. "+3 -1").
    base: String,
    /// Status indicators (e.g. "+3 -2 ?1").
    head: String,
    /// Ahead/behind remote tracking branch (e.g. "⇡1 ⇣2").
    remote: String,
    /// PR/MR number this branch tracks (e.g. "#123" / "!45").
    pr: String,
    /// Branch age since creation (shorthand).
    branch_age: String,
    /// Branch owner (git author email).
    owner: String,
    /// Abbreviated commit hash (7 chars).
    hash: String,
    /// Last commit: shorthand age + subject combined.
    last_commit: String,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-list"));

    init_logging(args.verbose);

    // Fleet scopes work from anywhere; the single-repo form needs a repo.
    // The positional and --repo are clap-exclusive, so `or` never merges.
    let repo_needle = args.repo.as_ref().or(args.repo_arg.as_ref());
    if repo_needle.is_some() || args.all_repos {
        // --all-repos runs the blocking renderer once per repo. Under
        // structured emit that would write one document per repo, interleaved
        // with the `── name ──` fleet dividers — not a single parseable
        // document. Reject it instead of emitting corruption; single-repo
        // `--repo <name>` is fine (one repo → one document). Aggregated fleet
        // structured output would be a separate feature. (#357 C2)
        if args.all_repos && args.emit.is_structured() {
            anyhow::bail!(
                "daft list --all-repos does not support structured output \
                 (--format/--template): it would emit one document per repo. Run \
                 it per repo (`daft list --repo <name> --format …`) or drop \
                 --all-repos."
            );
        }
        if is_git_repository()? {
            crate::catalog::touch_current_repo();
        }
        let scope = match repo_needle {
            Some(needle) => crate::catalog::fleet::FleetScope::Single(needle.clone()),
            None => crate::catalog::fleet::FleetScope::AllRepos,
        };
        let mut output = crate::output::CliOutput::new(crate::output::OutputConfig::default());
        // Always the blocking renderer — one live TUI per repo would churn.
        let outcome = crate::catalog::fleet::for_each_repo(
            scope,
            /* current_repo_last */ false,
            &mut output,
            |_row| run_blocking(args.clone()),
        )?;
        return outcome.into_result();
    }

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }
    crate::catalog::touch_current_repo();

    // Settings are loaded inside `run_live`/`run_blocking`, co-located with each
    // path's `GitCommand` so they share a single repo discovery (#584).
    if should_use_live(&args) {
        crate::commands::list_live::run_live(args)
    } else {
        run_blocking(args)
    }
}

fn should_use_live(args: &Args) -> bool {
    use std::io::IsTerminal;
    !args.emit.is_structured()
        && std::env::var_os("DAFT_NO_LIVE").is_none()
        && std::io::stdout().is_terminal()
}

/// Resolve the base branch to compare against, honoring `daft.remote` (not a
/// hardcoded `origin`) with a `master` fallback. Both list paths route through
/// this so they can't drift again (#597).
pub(crate) fn resolve_base_branch(
    git_common_dir: &std::path::Path,
    settings: &DaftSettings,
) -> String {
    get_default_branch_local(git_common_dir, &settings.remote, settings.use_gitoxide)
        .unwrap_or_else(|_| "master".to_string())
}

/// Apply the forge-visibility gate to a resolved column set: the
/// default-sourced `pr` column silently drops when the repo names no forge,
/// or when the last background refresh hit a deep failure (gh/glab missing,
/// dead auth, lost repo access) — and silently returns once a refresh
/// succeeds again; both verdicts persist in the repo's coordinator store.
/// A `pr` the user *named* (replace-mode `--columns`/`daft.list.columns`, or
/// a `+pr` modifier) always stays: config-recorded refs render without any
/// forge. Returns the effective columns plus the gate whenever `pr` was in
/// play at all — callers reuse it for the refresh spawn and the live table's
/// seed state, so health is read once per invocation.
pub(crate) fn gate_pr_column(
    columns: &[ListColumn],
    columns_input: Option<&str>,
    git: &GitCommand,
    git_common_dir: &std::path::Path,
) -> (
    Vec<ListColumn>,
    Option<crate::commands::forge_cache::ForgeGate>,
) {
    if !columns.contains(&ListColumn::Pr) {
        return (columns.to_vec(), None);
    }
    let repo_hash =
        crate::core::repo_identity::compute_repo_id_from_common_dir(git_common_dir).ok();
    // One store open for both the health gate and the PR-cache lookup the
    // render needs (`gate.lookup`), instead of a second open at row-decoration
    // time.
    let gate = crate::commands::forge_cache::forge_gate_and_lookup(git, repo_hash);
    let mut effective = columns.to_vec();
    if !columns_input.is_some_and(pr_explicitly_selected) && !gate.column_visible() {
        effective.retain(|c| *c != ListColumn::Pr);
    }
    (effective, Some(gate))
}

/// Whether a `--columns`/`daft.list.columns` spec names the `pr` column
/// itself (replace-mode token or `+pr` modifier) — as opposed to inheriting
/// it from the defaults, which is what the visibility gate may override.
fn pr_explicitly_selected(input: &str) -> bool {
    input
        .split(',')
        .map(str::trim)
        .any(|t| t.eq_ignore_ascii_case("pr") || t.eq_ignore_ascii_case("+pr"))
}

/// The branch/ref universe the open-PR row plan is computed against — read
/// once per list, and captured by the live path's refresh poll so the
/// mid-run reconcile plans against the same universe the seed did.
#[derive(Clone)]
pub(crate) struct PrRowContext {
    pub local_branches: HashSet<String>,
    pub branch_refs:
        std::collections::HashMap<String, crate::core::worktree::forge_ref::ForgeBranchRef>,
}

pub(crate) fn pr_row_context(git: &GitCommand) -> PrRowContext {
    let local_branches: HashSet<String> = git
        .for_each_ref("%(refname:short)", "refs/heads/")
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(str::to_string)
        .collect();
    let branch_refs = crate::core::worktree::pr_rows::parse_branch_forge_refs(
        &git.branch_merge_refs().unwrap_or_default(),
    );
    PrRowContext {
        local_branches,
        branch_refs,
    }
}

/// The default open-PR rows for both list paths: local branches surfaced
/// because an open PR heads there (enriched exactly like `--branches` rows,
/// with their tracking ref attached for `by_ref` decoration), plus rows
/// synthesized from the cache for PRs with no local presence. The caller
/// merges these into the row set, applies `apply_pr_owners`, and re-sorts.
#[allow(clippy::too_many_arguments)]
pub(crate) fn collect_pr_rows(
    git: &GitCommand,
    lookup: &crate::core::worktree::forge_ref::ForgePrLookup,
    ctx: &PrRowContext,
    worktree_branches: &HashSet<String>,
    show_local: bool,
    base_branch: &str,
    stat: Stat,
    project_root: &std::path::Path,
    ownership_strategy: crate::core::ownership::OwnershipStrategy,
    user_email: Option<&str>,
    remote_name: &str,
) -> Result<Vec<crate::core::worktree::list::WorktreeInfo>> {
    use crate::core::worktree::pr_rows;

    let plan = pr_rows::plan_pr_rows(
        lookup,
        worktree_branches,
        &ctx.local_branches,
        &ctx.branch_refs,
        show_local,
    );

    let mut rows = Vec::new();
    if !plan.surface_local.is_empty() {
        let only: HashSet<String> = plan.surface_local.iter().cloned().collect();
        let mut surfaced = collect_branch_info(
            git,
            base_branch,
            stat,
            true,
            false,
            worktree_branches,
            Some(&only),
            project_root,
            ownership_strategy,
            user_email,
            remote_name,
        )?;
        for info in &mut surfaced {
            // Fork-tracking branches decorate through `by_ref`; the branch
            // enrichment doesn't read tracking config, so attach it here.
            info.forge_ref = ctx.branch_refs.get(&info.name).copied();
        }
        rows.extend(surfaced);
    }
    rows.extend(plan.synthesized);
    Ok(rows)
}

fn run_blocking(args: Args) -> Result<()> {
    // Construct the body `GitCommand` first and load settings through it so the
    // repo is discovered once and reused for the command body (#584).
    let git = GitCommand::new(false);
    let settings = DaftSettings::load_with(&git)?;
    // Resolve the base branch before the `settings` field-moves below, since it
    // borrows `&settings` (honoring `daft.remote` rather than a hardcoded remote).
    let git_common_dir = get_git_common_dir()?;
    let base_branch = resolve_base_branch(&git_common_dir, &settings);
    let stat = args.stat.unwrap_or(settings.list_stat);
    let columns_input = args.columns.or(settings.list_columns);
    let resolved = match columns_input {
        Some(ref input) => {
            ColumnSelection::parse(input, CommandKind::List).map_err(|e| anyhow::anyhow!("{e}"))?
        }
        None => ResolvedColumns::defaults(ListColumn::list_defaults()),
    };
    let sort_input = args.sort.or(settings.list_sort);
    let sort_spec = match sort_input {
        Some(ref input) => SortSpec::parse(input)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .with_stat(stat),
        None => SortSpec::default_sort().with_stat(stat),
    };
    let compute_mtime = sort_spec.needs_mtime();
    let git = git.with_gitoxide(settings.use_gitoxide);
    // The forge gate may silently drop a default-sourced `pr` column from the
    // printed table. Structured emit keeps the ungated set instead: its
    // schema must stay stable across repos and health states, so `pr_*`
    // fields simply carry nulls where the table would hide the column.
    let (table_columns, forge_gate) = gate_pr_column(
        &resolved.columns,
        columns_input.as_deref(),
        &git,
        &git_common_dir,
    );
    let emit_columns = &resolved.columns;
    let has_size = resolved.columns.contains(&ListColumn::Size) || sort_spec.needs_size();
    let has_pr = if args.emit.is_structured() {
        emit_columns.contains(&ListColumn::Pr)
    } else {
        table_columns.contains(&ListColumn::Pr)
    };
    let user_email: Option<String> = git.config_get("user.email").ok().flatten();
    let current_path = get_current_worktree_path()
        .ok()
        .and_then(|p| p.canonicalize().ok());
    let project_root = get_project_root()?;

    let cwd = std::env::current_dir().unwrap_or_else(|_| project_root.clone());
    let show_local = args.branches || args.all;
    let show_remote = args.remotes || args.all;
    let needs_spinner = stat == Stat::Lines || show_local || show_remote || has_size;

    let mut infos = if needs_spinner {
        let mut output = CliOutput::new(OutputConfig::new(false, args.verbose));
        let msg = if stat == Stat::Lines {
            "Computing line statistics..."
        } else if has_size && !show_local && !show_remote {
            "Computing worktree sizes..."
        } else {
            "Collecting branch information..."
        };
        output.start_spinner(msg);
        let result = collect_worktree_info(
            &git,
            &base_branch,
            current_path.as_deref(),
            stat,
            has_size,
            compute_mtime,
            has_pr,
            settings.ownership_strategy,
            user_email.as_deref(),
            &settings.remote,
            crate::core::size_walk::resolve_jobs(settings.list_size_concurrency),
        )?;
        if show_local || show_remote {
            let worktree_branches: HashSet<String> =
                result.iter().map(|i| i.name.clone()).collect();
            let branch_infos = collect_branch_info(
                &git,
                &base_branch,
                stat,
                show_local,
                show_remote,
                &worktree_branches,
                None,
                &project_root,
                settings.ownership_strategy,
                user_email.as_deref(),
                &settings.remote,
            )?;
            let mut merged = result;
            merged.extend(branch_infos);
            merged.sort_by(|a, b| {
                a.kind
                    .section_order()
                    .cmp(&b.kind.section_order())
                    .then_with(|| sort_spec.compare(a, b))
            });
            output.finish_spinner();
            merged
        } else {
            output.finish_spinner();
            let mut result = result;
            sort_spec.sort(&mut result);
            result
        }
    } else {
        let mut result = collect_worktree_info(
            &git,
            &base_branch,
            current_path.as_deref(),
            stat,
            has_size,
            compute_mtime,
            has_pr,
            settings.ownership_strategy,
            user_email.as_deref(),
            &settings.remote,
            crate::core::size_walk::resolve_jobs(settings.list_size_concurrency),
        )?;
        sort_spec.sort(&mut result);
        result
    };

    // Warm the size cache from the blocking walk too, so piped / non-TTY
    // (`DAFT_NO_LIVE`) runs leave the same last-known sizes a later live run
    // seeds from. Write-only — blocking prints once, so there's no stale
    // render to seed. Every `size_bytes` here was freshly walked (no seeding
    // on this path); the helper stat-guards each path and is best-effort.
    // Runs before the `--merging` display filter so all walked sizes persist.
    if has_size
        && let Ok(repo_hash) =
            crate::core::repo_identity::compute_repo_id_from_common_dir(&git_common_dir)
    {
        let fresh = infos
            .iter()
            // Sandboxes all report name "(detached)" and would collide on the
            // (repo_hash, branch_slug) cache key — don't cache them (review).
            .filter(|info| !info.is_sandbox)
            .filter_map(|info| Some((info.name.clone(), info.path.clone()?, info.size_bytes?)));
        crate::commands::size_cache::persist_worktree_sizes(&repo_hash, fresh);
    }

    // Kick the detached refresh whenever `pr` was in play at all — including
    // when the gate just hid the column: the probe is what detects a repaired
    // auth and silently restores it on a later run.
    if let Some(gate) = &forge_gate {
        crate::commands::forge_cache::spawn_background_refresh_gated(gate);
    }
    // Forge-PR decoration for the cells actually rendered. The blocking path
    // prints once, so it serves the cache snapshot as-is; the no-stale-status
    // display contract lives in the live table, which can update mid-run.
    let forge_lookup = if has_pr {
        // Reuse the lookup read alongside the health gate (one store open).
        forge_gate.as_ref().and_then(|g| g.lookup.clone())
    } else {
        None
    };

    // Default open-PR rows ride the pr column's visibility: every open PR the
    // table doesn't already represent gets a row — a local branch surfaced, or
    // a row synthesized from the cache. `has_pr` keys off the same set the
    // output uses: the health-gated `table_columns` for the printed table (so
    // `--columns -pr` or the silent gate removes column and rows together), the
    // ungated `emit_columns` for structured output (so the row set — like the
    // schema — stays stable across forge health).
    if has_pr && let Some(lookup) = &forge_lookup {
        let worktree_branches: HashSet<String> = infos
            .iter()
            .filter(|i| i.kind == EntryKind::Worktree)
            .map(|i| i.name.clone())
            .collect();
        let ctx = pr_row_context(&git);
        let pr_rows = collect_pr_rows(
            &git,
            lookup,
            &ctx,
            &worktree_branches,
            show_local,
            &base_branch,
            stat,
            &project_root,
            settings.ownership_strategy,
            user_email.as_deref(),
            &settings.remote,
        )?;
        infos.extend(pr_rows);
        if show_remote {
            let synthesized: HashSet<String> = infos
                .iter()
                .filter(|i| i.kind == EntryKind::ForgePr)
                .map(|i| i.name.clone())
                .collect();
            infos.retain(|i| {
                !crate::core::worktree::pr_rows::remote_row_subsumed(&i.name, i.kind, &synthesized)
            });
        }
        crate::core::worktree::pr_rows::apply_pr_owners(&mut infos, lookup);
        infos.sort_by(|a, b| {
            a.kind
                .section_order()
                .cmp(&b.kind.section_order())
                .then_with(|| sort_spec.compare(a, b))
        });
    }

    if args.merging {
        infos.retain(|info| {
            info.path.as_ref().is_some_and(|p| {
                matches!(
                    crate::core::worktree::merge::detect_in_progress(p),
                    Ok(Some(crate::core::worktree::merge::InProgressOp::Merge))
                )
            })
        });
    }

    let now = Utc::now().timestamp();

    if args.emit.is_structured() {
        let table = build_emit_table(
            &infos,
            &project_root,
            &cwd,
            stat,
            emit_columns,
            now,
            forge_lookup.as_ref(),
        );
        return emit::emit_and_handle(
            "git-worktree-list",
            EmitPayload::Tabular(table),
            &args.emit,
            &mut std::io::stdout(),
        )
        .map_err(|e| anyhow::anyhow!("{e}"));
    }

    print_table(
        &infos,
        &project_root,
        &cwd,
        stat,
        &table_columns,
        &sort_spec,
        forge_lookup.as_ref(),
    );
    Ok(())
}

/// Determine which logical column groups are active for emit output.
struct EmitColumns {
    branch: bool,
    annotation: bool,
    path: bool,
    size: bool,
    base: bool,
    base_lines: bool,
    changes: bool,
    changes_lines: bool,
    remote: bool,
    remote_lines: bool,
    pr: bool,
    age: bool,
    owner: bool,
    hash: bool,
    last_commit: bool,
}

impl EmitColumns {
    fn compute(is_default: bool, selected: &[ListColumn], stat: Stat) -> Self {
        let has = |col: ListColumn| is_default || selected.contains(&col);
        let has_lines = stat == Stat::Lines;
        Self {
            branch: has(ListColumn::Branch),
            annotation: has(ListColumn::Annotation),
            path: has(ListColumn::Path),
            size: selected.contains(&ListColumn::Size),
            base: has(ListColumn::Base),
            base_lines: has(ListColumn::Base) && has_lines,
            changes: has(ListColumn::Changes),
            changes_lines: has(ListColumn::Changes) && has_lines,
            remote: has(ListColumn::Remote),
            remote_lines: has(ListColumn::Remote) && has_lines,
            // Opt-in like size/hash (explicit selection only, never in defaults).
            pr: selected.contains(&ListColumn::Pr),
            age: has(ListColumn::Age),
            owner: has(ListColumn::Owner),
            hash: selected.contains(&ListColumn::Hash),
            last_commit: has(ListColumn::LastCommit),
        }
    }

    fn headers(&self) -> Vec<String> {
        let mut h = Vec::new();
        if self.branch {
            h.push("kind".into());
            h.push("name".into());
        }
        if self.annotation {
            h.push("is_current".into());
            h.push("is_default_branch".into());
            h.push("is_sandbox".into());
        }
        if self.path {
            h.push("path".into());
        }
        if self.size {
            h.push("size_bytes".into());
            h.push("size".into());
        }
        if self.base {
            h.push("ahead".into());
            h.push("behind".into());
        }
        if self.base_lines {
            h.push("base_lines_inserted".into());
            h.push("base_lines_deleted".into());
        }
        if self.changes {
            h.push("staged".into());
            h.push("unstaged".into());
            h.push("untracked".into());
        }
        if self.changes_lines {
            h.push("staged_lines_inserted".into());
            h.push("staged_lines_deleted".into());
            h.push("unstaged_lines_inserted".into());
            h.push("unstaged_lines_deleted".into());
        }
        if self.remote {
            h.push("remote_ahead".into());
            h.push("remote_behind".into());
        }
        if self.remote_lines {
            h.push("remote_lines_inserted".into());
            h.push("remote_lines_deleted".into());
        }
        if self.pr {
            h.push("pr_kind".into());
            h.push("pr_number".into());
            h.push("pr_state".into());
            h.push("ci_status".into());
            h.push("pr_url".into());
        }
        if self.age {
            h.push("branch_age".into());
        }
        if self.owner {
            h.push("owner_name".into());
            h.push("owner_email".into());
        }
        if self.hash {
            h.push("hash".into());
        }
        if self.last_commit {
            h.push("last_commit_age".into());
            h.push("last_commit_subject".into());
        }
        h
    }
}

fn build_emit_table(
    infos: &[crate::core::worktree::list::WorktreeInfo],
    project_root: &std::path::Path,
    cwd: &std::path::Path,
    stat: Stat,
    selected_columns: &[ListColumn],
    now: i64,
    forge_lookup: Option<&crate::core::worktree::forge_ref::ForgePrLookup>,
) -> Table {
    let is_default_columns = selected_columns == ListColumn::list_defaults();
    let cols = EmitColumns::compute(is_default_columns, selected_columns, stat);
    let headers = cols.headers();
    let mut table = Table::new(headers);

    for info in infos {
        let mut row: Vec<Cell> = Vec::new();

        if cols.branch {
            let kind_str = match info.kind {
                EntryKind::Worktree => "worktree",
                EntryKind::LocalBranch => "branch",
                EntryKind::RemoteBranch => "remote",
                EntryKind::ForgePr => "pr",
            };
            row.push(Cell::str(kind_str));
            row.push(Cell::str(&info.name));
        }
        if cols.annotation {
            row.push(Cell::bool(info.is_current));
            row.push(Cell::bool(info.is_default_branch));
            row.push(Cell::bool(info.is_sandbox));
        }
        if cols.path {
            let rel_path = info
                .path
                .as_ref()
                .map(|p| relative_display_path(p, project_root, cwd));
            match rel_path {
                Some(p) => row.push(Cell::str(p)),
                None => row.push(Cell::null()),
            }
        }
        if cols.size {
            match info.size_bytes {
                Some(b) => {
                    row.push(Cell::Int(b as i64));
                    row.push(Cell::str(format_human_size(b)));
                }
                None => {
                    row.push(Cell::null());
                    row.push(Cell::null());
                }
            }
        }
        if cols.base {
            row.push(
                info.ahead
                    .map(|v| Cell::Int(v as i64))
                    .unwrap_or(Cell::null()),
            );
            row.push(
                info.behind
                    .map(|v| Cell::Int(v as i64))
                    .unwrap_or(Cell::null()),
            );
        }
        if cols.base_lines {
            row.push(
                info.base_lines_inserted
                    .map(|v| Cell::Int(v as i64))
                    .unwrap_or(Cell::null()),
            );
            row.push(
                info.base_lines_deleted
                    .map(|v| Cell::Int(v as i64))
                    .unwrap_or(Cell::null()),
            );
        }
        if cols.changes {
            row.push(Cell::Int(info.staged as i64));
            row.push(Cell::Int(info.unstaged as i64));
            row.push(Cell::Int(info.untracked as i64));
        }
        if cols.changes_lines {
            row.push(
                info.staged_lines_inserted
                    .map(|v| Cell::Int(v as i64))
                    .unwrap_or(Cell::null()),
            );
            row.push(
                info.staged_lines_deleted
                    .map(|v| Cell::Int(v as i64))
                    .unwrap_or(Cell::null()),
            );
            row.push(
                info.unstaged_lines_inserted
                    .map(|v| Cell::Int(v as i64))
                    .unwrap_or(Cell::null()),
            );
            row.push(
                info.unstaged_lines_deleted
                    .map(|v| Cell::Int(v as i64))
                    .unwrap_or(Cell::null()),
            );
        }
        if cols.remote {
            row.push(
                info.remote_ahead
                    .map(|v| Cell::Int(v as i64))
                    .unwrap_or(Cell::null()),
            );
            row.push(
                info.remote_behind
                    .map(|v| Cell::Int(v as i64))
                    .unwrap_or(Cell::null()),
            );
        }
        if cols.remote_lines {
            row.push(
                info.remote_lines_inserted
                    .map(|v| Cell::Int(v as i64))
                    .unwrap_or(Cell::null()),
            );
            row.push(
                info.remote_lines_deleted
                    .map(|v| Cell::Int(v as i64))
                    .unwrap_or(Cell::null()),
            );
        }
        if cols.pr {
            use crate::core::worktree::forge_ref::{PrDecoration, PrStatus};
            let decoration = match forge_lookup {
                Some(lookup) => lookup.decorate(&info.name, info.forge_ref),
                None => info.forge_ref.map(PrDecoration::bare),
            };
            match decoration {
                Some(d) => {
                    row.push(Cell::str(d.r.kind.tag()));
                    row.push(Cell::Int(d.r.number as i64));
                    row.push(match d.status {
                        Some(PrStatus::Merged) => Cell::str("merged"),
                        Some(PrStatus::Closed) => Cell::str("closed"),
                        Some(PrStatus::Open | PrStatus::Ci(_)) => Cell::str("open"),
                        // Config-recorded ref with no cache row: the PR's
                        // current state is unknown, not "open".
                        None => Cell::null(),
                    });
                    row.push(match d.status {
                        Some(PrStatus::Ci(ci)) => Cell::str(ci.as_str()),
                        _ => Cell::null(),
                    });
                    row.push(match d.url {
                        Some(url) => Cell::str(url),
                        None => Cell::null(),
                    });
                }
                None => {
                    for _ in 0..5 {
                        row.push(Cell::null());
                    }
                }
            }
        }
        if cols.age {
            let branch_age = info
                .branch_creation_timestamp
                .map(|ts| shorthand_from_seconds(now - ts))
                .unwrap_or_default();
            row.push(Cell::str(branch_age));
        }
        if cols.owner {
            match &info.owner {
                Some(o) => {
                    row.push(Cell::str(&o.name));
                    row.push(Cell::str(&o.email));
                }
                None => {
                    row.push(Cell::null());
                    row.push(Cell::null());
                }
            }
        }
        if cols.hash {
            match &info.last_commit_hash {
                Some(h) => row.push(Cell::str(h)),
                None => row.push(Cell::null()),
            }
        }
        if cols.last_commit {
            let last_commit_age = info
                .last_commit_timestamp
                .map(|ts| shorthand_from_seconds(now - ts))
                .unwrap_or_default();
            row.push(Cell::str(last_commit_age));
            row.push(Cell::str(&info.last_commit_subject));
        }

        table = table.row(row);
    }

    // Append a TOTAL summary row when the size column is active.
    if cols.size {
        let total_bytes: u64 = infos
            .iter()
            .filter(|i| i.kind == EntryKind::Worktree)
            .filter_map(|i| i.size_bytes)
            .sum();

        let header_count = table.headers.len();
        let mut total_row: Vec<Cell> = vec![Cell::null(); header_count];

        // Find the column indices for path, size_bytes, and size.
        let headers = &table.headers;
        if let Some(idx) = headers.iter().position(|h| h == "path") {
            total_row[idx] = Cell::str("TOTAL");
        }
        if let Some(idx) = headers.iter().position(|h| h == "size_bytes") {
            total_row[idx] = Cell::Int(total_bytes as i64);
        }
        if let Some(idx) = headers.iter().position(|h| h == "size") {
            total_row[idx] = Cell::str(format_human_size(total_bytes));
        }

        table = table.row(total_row);
    }

    table
}

/// Like `Priority::max(true)` but never picks the excluded column indices.
///
/// `Priority::max(true)` shrinks the widest column first when `Width::truncate`
/// runs. That's fine for descriptive columns (Branch/Path/LastCommit) but the
/// Size column's TOTAL summary cell (`"10.2G"`, `"127.4G"`) gets truncated mid-
/// suffix when it becomes a shrink candidate (#501). `Width::increase`/MinWidth
/// doesn't help — `Width::truncate` derives its per-column floors from
/// `EmptyRecords` (padding only), so MinWidth pads cells but doesn't pin a
/// shrink floor. Excluding the column from the candidate set is the only
/// surgical fix.
///
/// Honors `mins` so the shrink loop terminates cleanly when every non-excluded
/// column has hit its floor.
///
/// Note: the other fixed-width metric columns (`Hash`, `Changes`, `Base`,
/// `Remote`, `Age`, `Owner`) have the same class of bug under `Priority::max`
/// in very narrow terminals — the TUI's `fit_widths_to_available` only
/// shrinks `{Branch, Path, LastCommit}`. Excluding more columns here would
/// be a separate scope.
pub(crate) struct PriorityMaxExcept {
    pub(crate) excluded: Vec<usize>,
}

impl Peaker for PriorityMaxExcept {
    fn peak(&mut self, mins: &[usize], values: &[usize]) -> Option<usize> {
        values
            .iter()
            .zip(mins.iter())
            .enumerate()
            .filter(|(i, _)| !self.excluded.contains(i))
            .filter(|(_, (w, m))| **w > **m)
            .max_by_key(|(_, (w, _))| **w)
            .map(|(i, _)| i)
    }
}

fn print_table(
    infos: &[crate::core::worktree::list::WorktreeInfo],
    project_root: &std::path::Path,
    cwd: &std::path::Path,
    stat: Stat,
    selected_columns: &[ListColumn],
    sort_spec: &SortSpec,
    forge_lookup: Option<&crate::core::worktree::forge_ref::ForgePrLookup>,
) {
    if infos.is_empty() {
        let _ = crate::commands::list_empty::print(
            &mut std::io::stdout(),
            crate::styles::colors_enabled(),
        );
        return;
    }

    let use_color = styles::colors_enabled();

    // Print "Sorted by" summary when column headers alone can't convey the sort.
    if sort_spec.needs_summary_line(selected_columns) {
        let parts: Vec<String> = sort_spec
            .keys
            .iter()
            .enumerate()
            .map(|(rank, key)| {
                let arrow = SortSpec::arrow(key.direction);
                let name = key.column.display_name();
                if use_color {
                    let color_index = match rank {
                        0 => 255,
                        1 => 249,
                        _ => 243,
                    };
                    format!("{name} \x1b[38;5;{color_index}m{arrow}\x1b[0m")
                } else {
                    format!("{name} {arrow}")
                }
            })
            .collect();
        let label = if use_color {
            styles::dim("Sorted by")
        } else {
            "Sorted by".to_string()
        };
        println!(" {label} {}", parts.join(", "));
        println!();
    }
    let now = Utc::now().timestamp();

    // Determine which annotation types exist across all rows
    let has_any_current = infos.iter().any(|i| i.is_current);
    let has_any_default = infos.iter().any(|i| i.is_default_branch);
    let has_any_sandbox = infos.iter().any(|i| i.is_sandbox);

    let col_ctx = ColumnContext {
        project_root,
        cwd,
        now,
        stat,
        forge_prs: forge_lookup,
        colors: use_color,
    };

    // Pre-compute plain column values for alignment and reuse
    let col_vals: Vec<_> = infos
        .iter()
        .map(|i| compute_column_values(i, &col_ctx))
        .collect();

    // Max visible width of commit ages (for subject alignment)
    let max_commit_age_width = col_vals
        .iter()
        .map(|v| v.last_commit_age.len())
        .max()
        .unwrap_or(0);

    let rows: Vec<TableRow> = infos
        .iter()
        .zip(col_vals.iter())
        .map(|(info, vals)| {
            // Build annotation: ">" first (cyan), then "✦" (bright purple),
            // then "○" (dim) for sandbox
            let mut annotation = String::new();
            if has_any_current {
                if info.is_current {
                    if use_color {
                        annotation.push_str(&styles::cyan(styles::CURRENT_WORKTREE_SYMBOL));
                    } else {
                        annotation.push_str(styles::CURRENT_WORKTREE_SYMBOL);
                    }
                } else {
                    annotation.push(' ');
                }
                if has_any_default || has_any_sandbox {
                    annotation.push(' ');
                }
            }
            if has_any_default {
                if info.is_default_branch {
                    if use_color {
                        annotation.push_str(&styles::bright_purple(styles::DEFAULT_BRANCH_SYMBOL));
                    } else {
                        annotation.push_str(styles::DEFAULT_BRANCH_SYMBOL);
                    }
                } else if info.is_sandbox {
                    if use_color {
                        annotation.push_str(&styles::dim(styles::SANDBOX_SYMBOL));
                    } else {
                        annotation.push_str(styles::SANDBOX_SYMBOL);
                    }
                } else {
                    annotation.push(' ');
                }
            } else if has_any_sandbox {
                if info.is_sandbox {
                    if use_color {
                        annotation.push_str(&styles::dim(styles::SANDBOX_SYMBOL));
                    } else {
                        annotation.push_str(styles::SANDBOX_SYMBOL);
                    }
                } else {
                    annotation.push(' ');
                }
            }

            // Stat::Lines mode overrides base/changes/remote with line-level counts
            let (base, head, remote) = if stat == Stat::Lines {
                let base = format_ahead_behind(
                    info.base_lines_inserted,
                    info.base_lines_deleted,
                    use_color,
                );

                let ins = info.staged_lines_inserted.unwrap_or(0)
                    + info.unstaged_lines_inserted.unwrap_or(0);
                let del = info.staged_lines_deleted.unwrap_or(0)
                    + info.unstaged_lines_deleted.unwrap_or(0);
                let mut parts = Vec::new();
                if ins > 0 {
                    let text = format!("+{ins}");
                    if use_color {
                        parts.push(styles::green(&text));
                    } else {
                        parts.push(text);
                    }
                }
                if del > 0 {
                    let text = format!("-{del}");
                    if use_color {
                        parts.push(styles::red(&text));
                    } else {
                        parts.push(text);
                    }
                }
                if info.untracked > 0 {
                    let text = format!("?{}", info.untracked);
                    if use_color {
                        parts.push(styles::dim(&text));
                    } else {
                        parts.push(text);
                    }
                }
                let head = parts.join(" ");

                let remote = format_ahead_behind(
                    info.remote_lines_inserted,
                    info.remote_lines_deleted,
                    use_color,
                );

                (base, head, remote)
            } else {
                (
                    format_ahead_behind(info.ahead, info.behind, use_color),
                    format_head_status(info.staged, info.unstaged, info.untracked, use_color),
                    format_remote_status(info.remote_ahead, info.remote_behind, use_color),
                )
            };

            let branch_age = format_shorthand_age(info.branch_creation_timestamp, now, use_color);

            // Combine last commit age + subject, with age right-padded for alignment
            let commit_age = format_shorthand_age(info.last_commit_timestamp, now, use_color);
            let last_commit = if vals.last_commit_age.is_empty() {
                vals.last_commit_subject.clone()
            } else if vals.last_commit_subject.is_empty() {
                commit_age
            } else {
                let pad = " ".repeat(max_commit_age_width - vals.last_commit_age.len());
                format!("{commit_age}{pad} {}", vals.last_commit_subject)
            };

            // PR cell: in color mode the status rides in the number's color
            // (green/red/yellow CI, purple merged, dim closed) and the cell
            // links to the PR via OSC 8; colorless mode already carries the
            // status as a glyph in the text.
            let pr = if vals.pr.is_empty() || !use_color {
                vals.pr.clone()
            } else {
                use crate::core::worktree::forge_ref::{PrStatus, PrStatusColor};
                // Same status→slot mapping as the live renderer (semantic_color).
                let colored = match vals.pr_status.and_then(PrStatus::semantic_color) {
                    Some(PrStatusColor::Pass) => styles::green(&vals.pr),
                    Some(PrStatusColor::Fail) => styles::red(&vals.pr),
                    Some(PrStatusColor::Pending) => styles::yellow(&vals.pr),
                    Some(PrStatusColor::Merged) => styles::bright_purple(&vals.pr),
                    Some(PrStatusColor::Closed) => styles::dim(&vals.pr),
                    None => vals.pr.clone(),
                };
                match &vals.pr_url {
                    Some(url) => styles::hyperlink(&colored, url),
                    None => colored,
                }
            };

            let is_non_worktree = info.kind != EntryKind::Worktree;
            if use_color && is_non_worktree {
                TableRow {
                    annotation,
                    name: styles::dim(&vals.branch),
                    path: styles::dim(&vals.path),
                    size: if vals.size.is_empty() {
                        vals.size.clone()
                    } else {
                        styles::dim(&vals.size)
                    },
                    base: if base.is_empty() {
                        base
                    } else {
                        styles::dim(&strip_ansi(&base))
                    },
                    head: if head.is_empty() {
                        head
                    } else {
                        styles::dim(&strip_ansi(&head))
                    },
                    remote: if remote.is_empty() {
                        remote
                    } else {
                        styles::dim(&strip_ansi(&remote))
                    },
                    pr: if vals.pr.is_empty() {
                        vals.pr.clone()
                    } else {
                        // Row-level de-emphasis wins over status color for
                        // non-worktree rows, but the link is orthogonal.
                        match &vals.pr_url {
                            Some(url) => styles::hyperlink(&styles::dim(&vals.pr), url),
                            None => styles::dim(&vals.pr),
                        }
                    },
                    branch_age: if branch_age.is_empty() {
                        branch_age
                    } else {
                        styles::dim(&strip_ansi(&branch_age))
                    },
                    owner: if vals.owner.is_empty() {
                        vals.owner.clone()
                    } else {
                        styles::dim(&vals.owner)
                    },
                    hash: if vals.hash.is_empty() {
                        vals.hash.clone()
                    } else {
                        styles::dim(&vals.hash)
                    },
                    last_commit: if last_commit.is_empty() {
                        last_commit
                    } else {
                        styles::dim(&strip_ansi(&last_commit))
                    },
                }
            } else {
                TableRow {
                    annotation,
                    name: vals.branch.clone(),
                    path: vals.path.clone(),
                    size: vals.size.clone(),
                    base,
                    head,
                    remote,
                    pr,
                    branch_age,
                    owner: vals.owner.clone(),
                    hash: vals.hash.clone(),
                    last_commit,
                }
            }
        })
        .collect();

    let mut builder = Builder::new();

    // Build header from selected columns, with sort direction indicators
    let col_headers: Vec<(&str, ListColumn)> = selected_columns
        .iter()
        .filter(|c| **c != ListColumn::Annotation)
        .map(|c| {
            let label = match c {
                ListColumn::Branch => "Branch",
                ListColumn::Path => "Path",
                ListColumn::Size => "Size",
                ListColumn::Base => "Base",
                ListColumn::Changes => "Changes",
                ListColumn::Remote => "Remote",
                ListColumn::Pr => "PR",
                ListColumn::Age => "Age",
                ListColumn::Owner => "Owner",
                ListColumn::Hash => "Hash",
                ListColumn::LastCommit => "Commit",
                ListColumn::Annotation => unreachable!(),
            };
            (label, *c)
        })
        .collect();

    let show_annotations = selected_columns.contains(&ListColumn::Annotation)
        && (has_any_current || has_any_default || has_any_sandbox);

    // Format a header cell: dim+underline for label, sort arrow with
    // brightness gradient based on sort priority rank.
    let format_header = |label: &str, col: ListColumn| -> String {
        let indicator = sort_spec.direction_indicator(col);
        if use_color {
            match indicator {
                Some((arrow, rank)) => {
                    // 256-color grayscale: 232 (darkest) to 255 (brightest).
                    let color_index = match rank {
                        0 => 255, // bright white
                        1 => 249, // light gray
                        _ => 243, // medium gray
                    };
                    format!(
                        "{} \x1b[38;5;{color_index}m{arrow}\x1b[0m",
                        styles::dim_underline(label)
                    )
                }
                None => styles::dim_underline(label),
            }
        } else {
            match indicator {
                Some((arrow, _)) => format!("{label} {arrow}"),
                None => label.to_string(),
            }
        }
    };

    let header: Vec<String> = if show_annotations {
        std::iter::once("".to_string())
            .chain(col_headers.iter().map(|(h, c)| format_header(h, *c)))
            .collect()
    } else {
        col_headers
            .iter()
            .map(|(h, c)| format_header(h, *c))
            .collect()
    };
    builder.push_record(header);
    for row in &rows {
        let data_cols: Vec<&str> = col_headers
            .iter()
            .map(|(_, c)| match c {
                ListColumn::Branch => row.name.as_str(),
                ListColumn::Path => row.path.as_str(),
                ListColumn::Size => row.size.as_str(),
                ListColumn::Base => row.base.as_str(),
                ListColumn::Changes => row.head.as_str(),
                ListColumn::Remote => row.remote.as_str(),
                ListColumn::Pr => row.pr.as_str(),
                ListColumn::Age => row.branch_age.as_str(),
                ListColumn::Owner => row.owner.as_str(),
                ListColumn::Hash => row.hash.as_str(),
                ListColumn::LastCommit => row.last_commit.as_str(),
                ListColumn::Annotation => unreachable!(),
            })
            .collect();
        if show_annotations {
            let mut record = vec![row.annotation.as_str()];
            record.extend(data_cols);
            builder.push_record(record);
        } else {
            builder.push_record(data_cols);
        }
    }

    // Summary footer row for the Size column
    if selected_columns.contains(&ListColumn::Size) {
        let total_bytes: u64 = infos
            .iter()
            .filter(|i| i.kind == EntryKind::Worktree)
            .filter_map(|i| i.size_bytes)
            .sum();
        let total_size = format_human_size(total_bytes);
        let total_styled = if use_color {
            styles::dim(&total_size)
        } else {
            total_size
        };
        let footer: Vec<String> = if show_annotations {
            std::iter::once(String::new())
                .chain(col_headers.iter().map(|(_, c)| {
                    if *c == ListColumn::Size {
                        total_styled.clone()
                    } else {
                        String::new()
                    }
                }))
                .collect()
        } else {
            col_headers
                .iter()
                .map(|(_, c)| {
                    if *c == ListColumn::Size {
                        total_styled.clone()
                    } else {
                        String::new()
                    }
                })
                .collect()
        };
        // Empty separator row
        let empty: Vec<String> = footer.iter().map(|_| String::new()).collect();
        builder.push_record(empty);
        builder.push_record(footer);
    }

    let mut table = builder.build();
    table.with(Style::blank());
    table.modify(Columns::first(), Padding::new(1, 0, 0, 0));

    if let Some((TermWidth(width), _)) = terminal_size() {
        let width = width as usize;
        // When the Size column is shown, exclude it from the shrink candidate
        // set — its TOTAL summary cell can be wider than any data cell and
        // gets truncated otherwise (#501). See `PriorityMaxExcept`.
        match size_column_index(selected_columns, show_annotations) {
            Some(idx) => table.with(Width::truncate(width).suffix("...").priority(
                PriorityMaxExcept {
                    excluded: vec![idx],
                },
            )),
            None => table.with(
                Width::truncate(width)
                    .suffix("...")
                    .priority(Priority::max(true)),
            ),
        };
    }

    println!(
        "{}",
        paint_current_rows(&table.to_string(), infos, use_color)
    );
}

/// Background-highlight the current worktree's row in the rendered blocking
/// table, matching the live table's `CURRENT_ROW_BG_INDEX` row style. Line 0
/// is the header, data row i renders on line 1 + i, and the TOTAL footer
/// lines fall past `infos`, so they never match.
fn paint_current_rows(
    rendered: &str,
    infos: &[crate::core::worktree::list::WorktreeInfo],
    use_color: bool,
) -> String {
    if !use_color {
        return rendered.to_string();
    }
    rendered
        .lines()
        .enumerate()
        .map(|(i, line)| {
            let current = i
                .checked_sub(1)
                .and_then(|idx| infos.get(idx))
                .is_some_and(|info| info.is_current);
            if current {
                styles::paint_current_row(line)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The visible column index of `Size` in the rendered blocking table, or `None`
/// if the user didn't select it. Accounts for the leading annotation column
/// when annotations are shown.
fn size_column_index(selected_columns: &[ListColumn], show_annotations: bool) -> Option<usize> {
    let pos = selected_columns
        .iter()
        .filter(|c| **c != ListColumn::Annotation)
        .position(|c| *c == ListColumn::Size)?;
    Some(if show_annotations { pos + 1 } else { pos })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #597 regression: list must resolve the base branch via the configured
    /// `daft.remote`, not a hardcoded `origin`. Both HEAD files exist so it stays
    /// on the deterministic file-read path (no ambient git, no `#[serial]`).
    #[test]
    fn resolve_base_branch_honors_configured_remote_not_hardcoded_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let gcd = tmp.path();
        // get_default_branch_local reads the symref via `git symbolic-ref`, so
        // set up a real bare repo (production always passes a real common dir)
        // and write each remote's HEAD as an actual symbolic ref.
        let run_git = |args: &[&str]| {
            let out = crate::utils::git_command_at(gcd)
                .args(args)
                .output()
                .expect("git command");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run_git(&["init", "-q", "--bare"]);
        for (remote, branch) in [("upstream", "main"), ("origin", "wrongdefault")] {
            run_git(&[
                "symbolic-ref",
                &format!("refs/remotes/{remote}/HEAD"),
                &format!("refs/remotes/{remote}/{branch}"),
            ]);
        }

        // Honors settings.remote: a repo configured with `upstream` resolves to
        // upstream's default branch.
        let upstream = DaftSettings {
            remote: "upstream".into(),
            ..Default::default()
        };
        assert_eq!(resolve_base_branch(gcd, &upstream), "main");

        // Hardcoding `origin` (the #597 bug) would have resolved the wrong branch.
        let origin = DaftSettings {
            remote: "origin".into(),
            ..Default::default()
        };
        assert_eq!(resolve_base_branch(gcd, &origin), "wrongdefault");
    }

    /// The visibility gate only overrides a *default-sourced* `pr` column —
    /// naming it (replace mode or `+pr`, CLI or config) must always win.
    #[test]
    fn pr_explicit_selection_detection() {
        assert!(pr_explicitly_selected("pr"));
        assert!(pr_explicitly_selected("branch,pr,age"));
        assert!(pr_explicitly_selected("+pr"));
        assert!(pr_explicitly_selected("+size, +PR"));
        // Removing it, or naming other columns, is not asking for it.
        assert!(!pr_explicitly_selected("-pr"));
        assert!(!pr_explicitly_selected("+size"));
        assert!(!pr_explicitly_selected("branch,path"));
        // Token match, not substring match.
        assert!(!pr_explicitly_selected("prune"));
    }

    #[test]
    fn build_emit_table_headers_match_default_columns() {
        let selected = ListColumn::list_defaults();
        let table = build_emit_table(
            &[],
            std::path::Path::new("/tmp/proj"),
            std::path::Path::new("/tmp/proj"),
            Stat::Summary,
            selected,
            0,
            None,
        );
        // Default columns include: kind, name, is_current, is_default_branch,
        // is_sandbox, path, ahead, behind, staged, unstaged, untracked,
        // remote_ahead, remote_behind, branch_age, last_commit_age,
        // last_commit_subject, owner_name, owner_email.
        assert!(table.headers.contains(&"name".to_string()));
        assert!(table.headers.contains(&"ahead".to_string()));
        assert!(table.headers.contains(&"path".to_string()));
        assert!(table.headers.contains(&"owner_name".to_string()));
        assert!(table.headers.contains(&"owner_email".to_string()));
        // Size column is NOT in defaults; should not appear.
        assert!(!table.headers.contains(&"size_bytes".to_string()));
        // Hash column is NOT in defaults; should not appear.
        assert!(!table.headers.contains(&"hash".to_string()));
        // Empty infos means no rows.
        assert_eq!(table.rows.len(), 0);
    }

    /// Full parity with the live table: the blocking renderer paints the
    /// current worktree's line with the shared row background; header,
    /// other rows, and the no-color path stay untouched.
    #[test]
    fn paint_current_rows_highlights_only_the_current_worktree_line() {
        use crate::core::worktree::list::{EntryKind, WorktreeInfo};
        let info = |name: &str, is_current: bool| WorktreeInfo {
            kind: EntryKind::Worktree,
            name: name.to_string(),
            path: Some(std::path::PathBuf::from("/tmp/proj")),
            is_current,
            is_default_branch: false,
            ahead: None,
            behind: None,
            staged: 0,
            unstaged: 0,
            untracked: 0,
            remote_ahead: None,
            remote_behind: None,
            last_commit_timestamp: None,
            last_commit_hash: None,
            last_commit_subject: String::new(),
            branch_creation_timestamp: None,
            base_lines_inserted: None,
            base_lines_deleted: None,
            staged_lines_inserted: None,
            staged_lines_deleted: None,
            unstaged_lines_inserted: None,
            unstaged_lines_deleted: None,
            remote_lines_inserted: None,
            remote_lines_deleted: None,
            owner: None,
            size_bytes: None,
            working_tree_mtime: None,
            is_sandbox: false,
            forge_ref: None,
        };
        let infos = [info("main", true), info("feat", false)];
        let rendered = "  Branch  Path\n  main    /tmp/proj\n  feat    /tmp/proj";

        let painted = paint_current_rows(rendered, &infos, true);
        let lines: Vec<&str> = painted.lines().collect();
        assert!(
            !lines[0].contains(styles::CURRENT_ROW_BG),
            "header: {lines:?}"
        );
        assert!(
            lines[1].starts_with(styles::CURRENT_ROW_BG),
            "current row: {lines:?}"
        );
        assert!(
            !lines[2].contains(styles::CURRENT_ROW_BG),
            "other row: {lines:?}"
        );

        assert_eq!(
            paint_current_rows(rendered, &infos, false),
            rendered,
            "no-color output is untouched"
        );
    }

    #[test]
    fn build_emit_table_size_column_adds_total_row() {
        use crate::core::worktree::list::{EntryKind, WorktreeInfo};
        let info = WorktreeInfo {
            kind: EntryKind::Worktree,
            name: "main".to_string(),
            path: Some(std::path::PathBuf::from("/tmp/proj")),
            is_current: true,
            is_default_branch: true,
            ahead: None,
            behind: None,
            staged: 0,
            unstaged: 0,
            untracked: 0,
            remote_ahead: None,
            remote_behind: None,
            last_commit_timestamp: None,
            last_commit_hash: None,
            last_commit_subject: String::new(),
            branch_creation_timestamp: None,
            base_lines_inserted: None,
            base_lines_deleted: None,
            staged_lines_inserted: None,
            staged_lines_deleted: None,
            unstaged_lines_inserted: None,
            unstaged_lines_deleted: None,
            remote_lines_inserted: None,
            remote_lines_deleted: None,
            owner: None,
            size_bytes: Some(1024),
            working_tree_mtime: None,
            is_sandbox: false,
            forge_ref: None,
        };
        let selected = &[ListColumn::Branch, ListColumn::Path, ListColumn::Size];
        let table = build_emit_table(
            &[info],
            std::path::Path::new("/tmp/proj"),
            std::path::Path::new("/tmp/proj"),
            Stat::Summary,
            selected,
            0,
            None,
        );
        assert!(table.headers.contains(&"size_bytes".to_string()));
        // One data row + one TOTAL row.
        assert_eq!(table.rows.len(), 2);
        // Last row's path cell should be "TOTAL".
        let total_row = table.rows.last().unwrap();
        let path_idx = table.headers.iter().position(|h| h == "path").unwrap();
        assert_eq!(total_row[path_idx], Cell::str("TOTAL"));
        let size_bytes_idx = table
            .headers
            .iter()
            .position(|h| h == "size_bytes")
            .unwrap();
        assert_eq!(total_row[size_bytes_idx], Cell::Int(1024));
    }

    #[test]
    fn size_column_index_returns_position_among_visible_columns() {
        // Annotation is filtered out before col_headers, so the index for Size
        // is computed against the post-filter column list, then offset by 1
        // when the annotation column is shown.
        let cols = &[
            ListColumn::Branch,
            ListColumn::Path,
            ListColumn::Size,
            ListColumn::Age,
        ];
        assert_eq!(size_column_index(cols, false), Some(2));
        assert_eq!(size_column_index(cols, true), Some(3));
    }

    #[test]
    fn size_column_index_is_none_when_size_unselected() {
        let cols = &[ListColumn::Branch, ListColumn::Path];
        assert_eq!(size_column_index(cols, false), None);
        assert_eq!(size_column_index(cols, true), None);
    }

    #[test]
    fn size_column_index_skips_annotation_in_position_count() {
        // Annotation in selected_columns is filtered out before col_headers
        // is built, so the visible position of Size shifts down by one when
        // Annotation appears earlier in the list.
        let cols = &[ListColumn::Annotation, ListColumn::Branch, ListColumn::Size];
        // show_annotations=false: annotation column not shown, Size is at 1.
        assert_eq!(size_column_index(cols, false), Some(1));
        // show_annotations=true: leading annotation column adds 1.
        assert_eq!(size_column_index(cols, true), Some(2));
    }

    /// `PriorityMaxExcept` underpins the #501 fix: pick the widest column to
    /// shrink, but skip excluded indices and stop when every non-excluded
    /// column has hit its floor.
    #[test]
    fn priority_max_except_picks_widest_non_excluded() {
        let mut p = PriorityMaxExcept { excluded: vec![1] };
        // values=[10, 20, 15], col 1 (widest) is excluded → pick col 2 (15).
        assert_eq!(p.peak(&[0, 0, 0], &[10, 20, 15]), Some(2));
    }

    #[test]
    fn priority_max_except_returns_none_when_all_at_min() {
        let mut p = PriorityMaxExcept { excluded: vec![1] };
        // Non-excluded columns 0,2 are at their mins → terminate.
        assert_eq!(p.peak(&[5, 0, 5], &[5, 30, 5]), None);
    }

    #[test]
    fn priority_max_except_skips_excluded_even_when_widest() {
        let mut p = PriorityMaxExcept { excluded: vec![0] };
        // Excluded col 0 is by far the widest; must still pick col 2.
        assert_eq!(p.peak(&[0, 0, 0], &[100, 5, 8]), Some(2));
    }

    /// The TUI's natural-width computation passes
    /// `format_human_size(total).chars().count()` as the Size column's extra
    /// width (#501). Internal whitespace would split the cell across the
    /// column boundary, so pin the contract: the formatted total has no
    /// spaces.
    #[test]
    fn format_human_size_has_no_internal_whitespace() {
        for bytes in [0, 1024, 1024u64.pow(2), 11 * 1024u64.pow(3)] {
            let s = format_human_size(bytes);
            assert!(
                !s.contains(char::is_whitespace),
                "format_human_size({bytes}) = {s:?} must not contain whitespace"
            );
        }
    }

    #[test]
    fn list_empty_print_produces_expected_content() {
        // Direct smoke test of the helper used by `print_table`'s empty branch.
        // (Capturing real stdout from `print_table` itself isn't worth the
        // refactor — `tests/manual/scenarios/list/empty-bare.yml` covers the
        // end-to-end dispatch.)
        use crate::commands::list_empty;
        let mut buf = Vec::new();
        list_empty::print(&mut buf, false).expect("print failed");
        let s = String::from_utf8(buf).expect("non-utf8");
        assert!(s.contains("No worktrees yet."));
        assert!(s.contains("daft go <branch>"));
        assert!(s.contains("daft start <branch>"));
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;

    fn make_args(structured: bool) -> Args {
        // Construct an Args via clap's parser so the EmitArgs flatten resolves
        // correctly without us having to know its internal field shape.
        let mut argv = vec!["git-worktree-list"];
        if structured {
            argv.push("--format");
            argv.push("json");
        }
        Args::parse_from(argv)
    }

    #[test]
    fn should_use_live_returns_false_for_structured_output() {
        let args = make_args(true);
        // Even if other conditions are favorable, structured output forces
        // blocking. Whatever the TTY/env give us, the result must be false.
        assert!(!should_use_live(&args));
    }

    /// `daft list api` is positional sugar for `--repo api` (go-shape grammar
    /// for a read-only command). The two spellings are clap-exclusive with
    /// each other and with --all-repos — a contradiction, never a merge.
    #[test]
    fn repo_positional_is_exclusive_sugar_for_the_repo_flag() {
        let args = Args::parse_from(["git-worktree-list", "api"]);
        assert_eq!(args.repo_arg.as_deref(), Some("api"));
        assert!(args.repo.is_none());

        let flag = Args::parse_from(["git-worktree-list", "--repo", "api"]);
        assert_eq!(flag.repo.as_deref(), Some("api"));
        assert!(flag.repo_arg.is_none());

        assert!(Args::try_parse_from(["git-worktree-list", "api", "--repo", "webapp"]).is_err());
        assert!(Args::try_parse_from(["git-worktree-list", "api", "--all-repos"]).is_err());
    }

    #[test]
    fn should_use_live_respects_daft_no_live_env_var() {
        // Note: setting env vars in tests is process-global; tests in the same
        // binary may run in parallel. We save/restore to avoid leaking state to
        // other tests that read DAFT_NO_LIVE.
        let prev = std::env::var_os("DAFT_NO_LIVE");
        unsafe {
            std::env::set_var("DAFT_NO_LIVE", "1");
        }
        let args = make_args(false);
        assert!(!should_use_live(&args));
        match prev {
            Some(v) => unsafe { std::env::set_var("DAFT_NO_LIVE", v) },
            None => unsafe { std::env::remove_var("DAFT_NO_LIVE") },
        }
    }
}
