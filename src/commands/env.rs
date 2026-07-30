//! `daft env` — deterministic per-worktree env values (#388).
//!
//! The command is the derivation function exposed: every input has a CLI
//! twin, daft.yml is just saved arguments. It is also on the shell hot path
//! (`eval "$(daft env --export)"` from `.envrc`/mise on every cd), so the
//! read path is a single in-process `gix::discover` plus one daft.yml read —
//! no subprocesses, no settings load, no catalog open unless a repo is
//! actually addressed, and stderr stays silent in the normal case.
//!
//! It also answers daft's own `DAFT_*` job variables (the ones hooks and
//! tasks receive) from live worktree state. Those live on the *inspection*
//! surfaces only — the human listing and explicit `DAFT_*` queries — which
//! may open the addressed worktree's repo. Every serializing surface
//! (`--export`, `--write`, `--format`) stays the declared set, so the eval
//! hot path keeps its single-discover budget and nothing materializes
//! ambient state that goes stale the moment the worktree moves.

use crate::core::env_values::address::parse_address;
use crate::core::env_values::{AdHocPolicy, EnvSpec, ResolvedValue, ValueContext, ValueSource};
use crate::core::slug::{slugify, worktree_slug_from};
use crate::output::emit::{self, Cell, EmitArgs, EmitPayload, Table};
use crate::output::{CliOutput, Output, OutputConfig};
use crate::styles::{cyan, dim};
use anyhow::{Context, Result, bail};
use clap::Parser;
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(name = "daft-env")]
#[command(version = crate::VERSION)]
#[command(about = "Print deterministic per-worktree env values (ports, names)")]
#[command(
    long_about = "Print deterministic per-worktree env values derived from the worktree's name.

Every value is a pure function of (salt, worktree slug, declaration) — no allocation, no registry, no daemon. The same inputs give the same answer on every machine, from every directory, even for a worktree that does not exist yet, so services in different worktrees (and different repos) can find each other with zero coordination: compute, don't communicate.

Ports hash the worktree to a contiguous block (default 16 ports in 20000-32767) and declared names take small offsets inside it, so one worktree's ports are consecutive and collisions between worktrees require whole-block hash collisions, which daft warns about. Undeclared names resolve in a disjoint ad-hoc region when the repo declares no env: schema; declaring one makes unknown names an error (--ad-hoc overrides).

The positional address is [repo:]VAR[@worktree] — 'daft env API_PORT', 'daft env backend:API_PORT' (that repo's worktree matching this one's name), 'daft env API_PORT@feat-x'. --repo/--worktree are the canonical spellings of the same qualifiers. Unlike other commands' bare --repo (which targets the default-branch worktree), env's cross-repo default is the worktree matching the current one's name: an address names a value's coordinate, not an execution target, and the matching-name convention is how one feature spans repos.

Derivation knobs (--salt, --range, --block-size, --offset) answer \"what would it be\": overriding them computes a hypothetical — useful to preview a salt change before committing it — and daft notes on stderr when the answer differs from the configured one.

Daft's own job variables are answered too, for debugging what a hook or task will see without running it. The listing shows the at-rest DAFT_* set — the seven a task receives whose values the worktree alone determines (DAFT_PROJECT_ROOT, DAFT_GIT_DIR, DAFT_REMOTE, DAFT_SOURCE_WORKTREE, DAFT_WORKTREE_PATH, DAFT_BRANCH_NAME, DAFT_IS_NEW_BRANCH) — and naming one prints its value. They are built from the same context a task runs under rather than recomputed, so the answer cannot drift from the job. Two carry a caveat for lifecycle hooks specifically: a hook's DAFT_REMOTE is its operation's remote (daft.remote, when set) and a create or remove hook's DAFT_SOURCE_WORKTREE is wherever the command was invoked, not the worktree named here.

Every other DAFT_* variable is event-scoped — DAFT_HOOK, DAFT_COMMIT, DAFT_REMOVAL_REASON, DAFT_MERGE_*, ... — and exists only while its operation runs; asking for one explains when it is set instead of guessing a value, and an unrecognized name is reported as such rather than answered. None of them are derived, so derivation flags do not apply, and serializing surfaces (--export, --write, --format) stay the declared set: they feed files and machines that keep what they read, and these values go stale the moment the worktree moves.

Declare values under env: in daft.yml (see the yaml reference). Injection: hooks, tasks, and daft exec receive declared values automatically; shells via eval \"$(daft env --export)\" (direnv/mise); file-reading tools via daft env --write (dotenv). Injected values never override variables already set in the parent environment."
)]
pub struct Args {
    /// Value address: VAR, repo:VAR, VAR@worktree, or repo:VAR@worktree.
    /// Omit to list every declared value for the addressed worktree.
    #[arg(value_name = "VAR")]
    pub address: Option<String>,

    /// Read another cataloged repository's values (canonical form of the
    /// `repo:` address prefix).
    #[arg(long, value_name = "NAME")]
    pub repo: Option<String>,

    /// Address a specific worktree by name (canonical form of the
    /// `@worktree` address suffix). Works for worktrees that do not exist
    /// yet — the value is already determined.
    #[arg(long, value_name = "NAME")]
    pub worktree: Option<String>,

    /// Override the hash salt (default: `env.salt`, else the repo directory
    /// name). Answers "what would my values be under this salt".
    #[arg(long, value_name = "SALT")]
    pub salt: Option<String>,

    /// Override the port range (default: `env.range`, else 20000-32767).
    #[arg(long, value_name = "START-END")]
    pub range: Option<String>,

    /// Override the ports-per-worktree block size (default: `env.block_size`,
    /// else 16).
    #[arg(long, value_name = "N")]
    pub block_size: Option<u16>,

    /// With VAR: compute the port at this offset in the worktree's block,
    /// regardless of what the schema declares.
    #[arg(long, value_name = "N", requires = "address")]
    pub offset: Option<u16>,

    /// Emit `export NAME='value'` lines for the shell (`eval "$(daft env
    /// --export)"`).
    #[arg(long)]
    pub export: bool,

    /// Write the declared values as a dotenv file. PATH defaults to
    /// `env.write` from daft.yml; `-` writes the dotenv text to stdout.
    #[arg(long, value_name = "PATH", num_args = 0..=1, default_missing_value = "")]
    pub write: Option<String>,

    /// Resolve an undeclared name to an ad-hoc port even though this repo
    /// declares an env: schema.
    #[arg(long)]
    pub ad_hoc: bool,

    #[command(flatten)]
    pub emit: EmitArgs,
}

pub fn run() -> Result<()> {
    // Read the `-C`-stripped argv and skip argv[0] so clap sees "env" as the
    // program name (same dispatcher shape as run/install/doctor). This shape
    // parses via Args::parse_from, so `env` must NOT join DAFT_VERBS in
    // lib.rs — that array only feeds get_clap_args, which this bypasses.
    let args_raw: Vec<String> = crate::cli::argv().iter().skip(1).cloned().collect();
    let args = Args::parse_from(&args_raw);
    let mut output = CliOutput::new(OutputConfig::default());
    cmd_env(&args, &mut output)
}

/// The addressed target: a repo root plus the worktree (real or hypothetical)
/// whose values are being computed.
struct Target {
    /// Project root of the addressed repo.
    root: PathBuf,
    /// Slug the derivation runs on.
    slug: String,
    /// The worktree's path, when it exists.
    worktree_path: Option<PathBuf>,
    /// The worktree's branch, when it exists and has one.
    branch: Option<String>,
    /// Sibling worktree slugs (for the collision scan; best-effort).
    sibling_slugs: Vec<String>,
    /// Human name for notes ("worktree 'feat-x'").
    requested_name: Option<String>,
}

fn cmd_env(args: &Args, output: &mut dyn Output) -> Result<()> {
    // Address sugar and canonical flags merge; both spelling a qualifier is
    // ambiguous and refused rather than guessed.
    let parsed = match &args.address {
        Some(raw) => Some(parse_address(raw).map_err(|e| anyhow::anyhow!("{e}"))?),
        None => None,
    };
    let addr_repo = parsed.as_ref().and_then(|a| a.repo);
    let addr_wt = parsed.as_ref().and_then(|a| a.worktree);
    let var = parsed.as_ref().map(|a| a.var);
    if addr_repo.is_some() && args.repo.is_some() {
        bail!("the address names a repo and --repo is given; use one spelling");
    }
    if addr_wt.is_some() && args.worktree.is_some() {
        bail!("the address names a worktree and --worktree is given; use one spelling");
    }
    let repo_arg = addr_repo.map(str::to_string).or_else(|| args.repo.clone());
    let wt_arg = addr_wt
        .map(str::to_string)
        .or_else(|| args.worktree.clone());

    // Output-mode exclusivity, checked up front so a conflict errors before
    // any work happens.
    let structured = args.emit.is_structured();
    let modes = [args.export, args.write.is_some(), structured];
    if modes.iter().filter(|m| **m).count() > 1 {
        bail!("--export, --write, and --format are mutually exclusive");
    }
    // `--write` materializes the whole declared set as a dotenv file; there
    // is no single-variable form of it. Refusing beats the silent drop that
    // preceded it — the caller is waiting on a file that never appears, or
    // reads a stale one left by an earlier run.
    if args.write.is_some()
        && let Some(name) = var
    {
        bail!(
            "--write serializes the whole declared set to a file and takes no VAR; \
             drop '{name}', or print just that one with `{}`",
            cyan(&crate::daft_cmd(&format!("env {name}")))
        );
    }

    // One in-process discovery for the local repo. Absent local context is
    // fine when a repo qualifier points elsewhere.
    let local = discover_local();
    let target = resolve_target(&local, repo_arg.as_deref(), wt_arg.as_deref(), output)?;

    // Daft's own DAFT_* variables answer from worktree state, not
    // derivation — routed before any spec work so derivation config and
    // flags never shape the answer, a declared schema's strictness does not
    // apply to daft's namespace, and no collision scan fires for a
    // non-port question.
    if let Some(name) = var
        && name.starts_with("DAFT_")
    {
        return answer_context_var(args, &target, &local, name);
    }

    // Effective spec: daft.yml (of the addressed repo) + CLI overrides.
    let config_env = load_env_config(&target, &local);
    let default_salt = dir_name(&target.root);
    let mut spec = EnvSpec::from_config(config_env.as_ref(), &default_salt);
    let configured = spec.clone();
    if let Some(ref salt) = args.salt {
        spec.salt = salt.clone();
    }
    if let Some(ref raw) = args.range {
        spec.range = crate::hooks::yaml_config::parse_port_range(raw)
            .with_context(|| format!("invalid --range '{raw}'; expected START-END"))?;
    }
    if let Some(bs) = args.block_size {
        spec.block_size = bs;
    }
    let overridden = spec.salt != configured.salt
        || spec.range != configured.range
        || spec.block_size != configured.block_size;

    let ctx = ValueContext {
        repo: &default_salt,
        worktree_path: target.worktree_path.as_deref().and_then(Path::to_str),
        worktree_root: target.root.to_str(),
        branch: target.branch.as_deref(),
    };

    // Advisory collision scan: never changes an answer, at most one stderr
    // line, killable via DAFT_ENV_NO_COLLISION_CHECK=1.
    warn_on_block_collision(&spec, &target, output);

    match var {
        Some(name) => {
            let resolved = resolve_single(args, &spec, &target, name, &ctx)?;
            if overridden {
                note_divergence(&configured, &target, name, &resolved, &ctx, output);
            }
            if args.export {
                println!("{}", export_line(&resolved));
            } else {
                println!("{}", resolved.value);
            }
            Ok(())
        }
        None => {
            let resolved = spec.resolve_all(&target.slug, &ctx).map_err(|e| {
                anyhow::anyhow!("{e}").context("failed to resolve declared env values")
            })?;
            if overridden {
                output.notice(&format!(
                    "{} computed with derivation overrides; configured values may differ",
                    dim("note:")
                ));
            }
            if args.export {
                for value in &resolved {
                    println!("{}", export_line(value));
                }
                return Ok(());
            }
            if let Some(ref write) = args.write {
                return write_dotenv(write, &spec, &resolved, &target);
            }
            if structured {
                return emit_table(&resolved, &args.emit);
            }
            // The human listing also shows daft's own variables. Computed
            // only here — past every serializing return above — so the eval
            // hot path never pays for the extra repo reads and no machine
            // surface materializes ambient state that goes stale.
            let context = context_rows(&target, &local);
            print_human_table(&spec, &target.slug, &resolved, &context, output);
            Ok(())
        }
    }
}

/// Local repo context from one `gix::discover`. `None` fields degrade
/// gracefully: outside any repo everything is `None`; at a bare container
/// root there is a project root but no current worktree.
struct LocalContext {
    project_root: Option<PathBuf>,
    worktree_path: Option<PathBuf>,
    branch: Option<String>,
    repo: Option<gix::Repository>,
}

fn discover_local() -> LocalContext {
    let Ok(cwd) = std::env::current_dir() else {
        return LocalContext {
            project_root: None,
            worktree_path: None,
            branch: None,
            repo: None,
        };
    };
    let Ok(repo) = gix::discover(&cwd) else {
        return LocalContext {
            project_root: None,
            worktree_path: None,
            branch: None,
            repo: None,
        };
    };
    let common = repo.common_dir();
    let common = std::fs::canonicalize(common).unwrap_or_else(|_| common.to_path_buf());
    let project_root = common.parent().map(Path::to_path_buf);
    let worktree_path = repo
        .workdir()
        .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
    let branch = crate::git::oxide::symbolic_ref_short_head(&repo).ok();
    LocalContext {
        project_root,
        worktree_path,
        branch,
        repo: Some(repo),
    }
}

/// Resolve the addressed (repo, worktree) coordinate to a [`Target`].
fn resolve_target(
    local: &LocalContext,
    repo_arg: Option<&str>,
    wt_arg: Option<&str>,
    output: &mut dyn Output,
) -> Result<Target> {
    let current_slug = local
        .worktree_path
        .as_ref()
        .zip(local.project_root.as_ref())
        .map(|(wt, root)| worktree_slug_from(wt, root));

    match repo_arg {
        None => {
            let root = local.project_root.clone().context(
                "not inside a git repository (pass `repo:VAR` or --repo to address one)",
            )?;
            let siblings = local
                .repo
                .as_ref()
                .map(|r| local_worktrees(r, &root))
                .unwrap_or_default();
            match wt_arg {
                None => {
                    let worktree_path = local.worktree_path.clone().context(
                        "not inside a worktree; pass --worktree <name> (or VAR@worktree) \
                         to address one",
                    )?;
                    Ok(Target {
                        slug: current_slug.clone().unwrap_or_default(),
                        sibling_slugs: sibling_slugs(&siblings, &root),
                        worktree_path: Some(worktree_path),
                        branch: local.branch.clone(),
                        root,
                        requested_name: None,
                    })
                }
                Some(name) => pick_worktree(name, &root, &siblings, output).map(|mut t| {
                    t.sibling_slugs = sibling_slugs(&siblings, &root);
                    t
                }),
            }
        }
        Some(repo_name) => {
            let row = crate::catalog::resolve_repo_arg(repo_name)?;
            let root = PathBuf::from(&row.path);
            // Naming the repo you are standing in resolves locally — the
            // catalog row and the local discovery describe the same repo.
            let children: Vec<(PathBuf, Option<String>)> =
                crate::catalog::worktrees::worktree_children(&row, None)
                    .map(|cs| {
                        cs.into_iter()
                            .map(|c| (PathBuf::from(c.path), c.branch))
                            .collect()
                    })
                    .unwrap_or_default();
            let requested = wt_arg
                .map(str::to_string)
                .or_else(|| current_slug.clone())
                .context(
                    "no worktree to match: run from inside a worktree, or add @worktree \
                     (or --worktree) to the address",
                )?;
            pick_worktree(&requested, &root, &children, output).map(|mut t| {
                t.sibling_slugs = sibling_slugs(&children, &root);
                t
            })
        }
    }
}

/// Enumerate the local repo's worktrees (main + linked) in-process.
fn local_worktrees(repo: &gix::Repository, _root: &Path) -> Vec<(PathBuf, Option<String>)> {
    let mut out = Vec::new();
    if let Some(workdir) = repo.workdir() {
        let path = std::fs::canonicalize(workdir).unwrap_or_else(|_| workdir.to_path_buf());
        out.push((path, crate::git::oxide::symbolic_ref_short_head(repo).ok()));
    }
    if let Ok(proxies) = repo.worktrees() {
        for proxy in proxies {
            let Ok(base) = proxy.base() else { continue };
            let path = std::fs::canonicalize(&base).unwrap_or(base);
            if out.iter().any(|(p, _)| p == &path) {
                continue;
            }
            out.push((path, None));
        }
    }
    out
}

/// Match `name` (slugified) against enumerated worktrees; fall back to a
/// hypothetical worktree of that name — the value is determined either way.
fn pick_worktree(
    name: &str,
    root: &Path,
    children: &[(PathBuf, Option<String>)],
    output: &mut dyn Output,
) -> Result<Target> {
    let wanted = slugify(name);
    let hit = children
        .iter()
        .find(|(path, _)| worktree_slug_from(path, root) == wanted);
    match hit {
        Some((path, branch)) => Ok(Target {
            root: root.to_path_buf(),
            slug: wanted,
            worktree_path: Some(path.clone()),
            branch: branch.clone(),
            sibling_slugs: Vec::new(),
            requested_name: Some(name.to_string()),
        }),
        None => {
            output.notice(&format!(
                "{} no worktree named '{name}' exists {} — computing for it anyway \
                 (values are determined before creation)",
                dim("note:"),
                dim(&format!("under {}", root.display())),
            ));
            Ok(Target {
                root: root.to_path_buf(),
                slug: wanted,
                worktree_path: None,
                branch: None,
                sibling_slugs: Vec::new(),
                requested_name: Some(name.to_string()),
            })
        }
    }
}

fn sibling_slugs(children: &[(PathBuf, Option<String>)], root: &Path) -> Vec<String> {
    children
        .iter()
        .map(|(path, _)| worktree_slug_from(path, root))
        .collect()
}

/// Load the addressed repo's `env:` config. Local target: the merged config
/// of the current worktree. Cross-repo (or bare-root): the matched worktree's
/// merged config when one exists, else any enumerable worktree is not
/// guessed — the tracked config is read through the bare repo.
fn load_env_config(
    target: &Target,
    local: &LocalContext,
) -> Option<crate::hooks::yaml_config::EnvConfig> {
    let from_worktree = |wt: &Path| {
        crate::hooks::yaml_config_loader::load_merged_config(wt)
            .ok()
            .flatten()
            .and_then(|c| c.env)
    };
    if let Some(ref wt) = target.worktree_path {
        return from_worktree(wt);
    }
    // Hypothetical worktree: read the tracked config from the local worktree
    // when the target is this repo (same tracked file), else give up and
    // derive with defaults — still deterministic, still useful.
    if local.project_root.as_deref() == Some(target.root.as_path())
        && let Some(ref wt) = local.worktree_path
    {
        return from_worktree(wt);
    }
    None
}

fn resolve_single(
    args: &Args,
    spec: &EnvSpec,
    target: &Target,
    name: &str,
    ctx: &ValueContext<'_>,
) -> Result<ResolvedValue> {
    // --offset is the raw calculator: block base + N, declaration ignored.
    if let Some(offset) = args.offset {
        if offset >= spec.block_size {
            bail!(
                "--offset {offset} does not fit a block of {} ports",
                spec.block_size
            );
        }
        let port = spec
            .block_base(&target.slug)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            + offset;
        return Ok(ResolvedValue {
            name: name.to_string(),
            value: port.to_string(),
            source: ValueSource::DeclaredPort { offset },
        });
    }
    let policy = if args.ad_hoc {
        AdHocPolicy::Allow
    } else {
        AdHocPolicy::UnlessDeclaredSchema
    };
    spec.resolve_one(&target.slug, name, ctx, policy)
        .map_err(|e| {
            use crate::core::env_values::DerivationError;
            match e {
                DerivationError::UnknownName { ref name } => anyhow::anyhow!(
                    "'{name}' is not declared in this repo's env: section\n  \
                 declare it in daft.yml, or pass --ad-hoc for a derived one-off port \
                 (`{}`)",
                    cyan(&crate::daft_cmd(&format!("env {name} --ad-hoc")))
                ),
                other => anyhow::anyhow!("{other}"),
            }
        })
}

/// When derivation overrides are active, say — on stderr — whether the
/// answer differs from the configured derivation for the same query.
fn note_divergence(
    configured: &EnvSpec,
    target: &Target,
    name: &str,
    resolved: &ResolvedValue,
    ctx: &ValueContext<'_>,
    output: &mut dyn Output,
) {
    let base = configured.resolve_one(&target.slug, name, ctx, AdHocPolicy::Allow);
    if let Ok(base) = base
        && base.value != resolved.value
    {
        output.notice(&format!(
            "{} override changes the answer: configured derivation gives {}",
            dim("note:"),
            base.value
        ));
    }
}

/// Answer a `DAFT_*` query: a live value for at-rest variables, an
/// explanation of when it exists for event-scoped ones, and a suggestion
/// for typos. Exit is non-zero whenever no value is printed, so a script
/// probing for one fails the same way its own environment lookup would.
fn answer_context_var(
    args: &Args,
    target: &Target,
    local: &LocalContext,
    name: &str,
) -> Result<()> {
    use crate::hooks::DaftVarKind;

    if args.ad_hoc
        || args.offset.is_some()
        || args.salt.is_some()
        || args.range.is_some()
        || args.block_size.is_some()
    {
        bail!(
            "'{name}' is one of daft's own variables, read from worktree state — \
             derivation flags (--salt, --range, --block-size, --offset, --ad-hoc) \
             do not apply"
        );
    }
    // Serialization stays the declared set, as the help, the man page and
    // the docs all promise. (`--write` is already refused for any VAR in
    // `cmd_env`; this is the `--export` half.) An `export DAFT_BRANCH_NAME=…`
    // line appended to an .envrc pins a value that is wrong after the next
    // checkout — the staleness that keeps these out of serialized output.
    if args.export {
        bail!(
            "--export serializes declared values only — '{name}' is daft's own, \
             read live from worktree state\n  print it with `{}`",
            cyan(&crate::daft_cmd(&format!("env {name}")))
        );
    }

    match crate::hooks::daft_var_kind(name) {
        DaftVarKind::Unknown => {
            let names = crate::hooks::daft_var_names();
            let similar = crate::suggest::find_similar_commands(name, &names);
            let hint = similar
                .first()
                .map(|s| format!("\n  did you mean {s}?"))
                .unwrap_or_default();
            bail!(
                "'{name}' is not a variable daft sets{hint}\n  \
                 declared values use bare names (`{}`); daft's own are listed by \
                 `{}` and in daft-hooks(5)",
                cyan(&crate::daft_cmd("env WEBAPP_PORT")),
                cyan(&crate::daft_cmd("env"))
            );
        }
        DaftVarKind::EventScoped { when } => {
            bail!("'{name}' has no value at rest — it is set only {when}");
        }
        DaftVarKind::AtRest => {
            let rows = context_rows(target, local);
            let Some(row) = rows.into_iter().find(|r| r.name == name) else {
                // AtRest but absent. Each cause gets its own message —
                // guessing one of them is how a user ends up debugging the
                // wrong thing.
                if target.worktree_path.is_none() {
                    bail!(
                        "worktree '{}' does not exist yet — {name} exists once it is created",
                        target.requested_name.as_deref().unwrap_or(&target.slug)
                    );
                }
                bail!(
                    "could not read worktree '{}' as a git worktree, so {name} is \
                     unknown — is its .git link intact?",
                    target.slug
                );
            };
            println!("{}", row.value);
            Ok(())
        }
    }
}

/// Daft's own `DAFT_*` variables as they stand for the addressed worktree.
///
/// Built by constructing the very [`HookContext`] a task would run under and
/// reading [`HookEnvironment::from_context`] — not by re-deriving the values.
/// That is deliberate: this is a debugging view of the job environment, so
/// anything it models by hand can drift from what jobs actually receive, and
/// every such divergence is a confidently-wrong answer. The rows are filtered
/// to `AtRest` (the event-scoped names would need a run to be meaningful) and
/// ordered by the classifier's own table, since `vars()` is a `HashMap`.
///
/// [`HookContext`]: crate::hooks::HookContext
/// [`HookEnvironment::from_context`]: crate::hooks::HookEnvironment::from_context
fn context_rows(target: &Target, local: &LocalContext) -> Vec<ResolvedValue> {
    use crate::hooks::HookContext;

    let row = |name: &str, value: String| ResolvedValue {
        name: name.to_string(),
        value,
        source: ValueSource::Context,
    };

    let repo = open_target_repo(target, local);
    let git_dir = repo.as_ref().map(|repo| {
        let common = repo.common_dir();
        std::fs::canonicalize(common).unwrap_or_else(|_| common.to_path_buf())
    });

    // Hypothetical worktree (or no repo handle): there is no context to
    // build, so emit the strict subset whose values do not depend on the
    // worktree at all — `from_context` copies both straight off the context.
    // Deliberately not a second model of the environment: that is the drift
    // this function exists to avoid.
    let (Some(wt), Some(git_dir)) = (target.worktree_path.as_ref(), git_dir.as_ref()) else {
        let mut rows = vec![row("DAFT_PROJECT_ROOT", target.root.display().to_string())];
        if let Some(git_dir) = git_dir {
            rows.push(row("DAFT_GIT_DIR", git_dir.display().to_string()));
        }
        return rows;
    };

    // The task surface, verbatim: same remote fallback and same branch
    // resolution as the `for_task` call in commands/run.rs. A lifecycle hook
    // may carry a different remote (its operation's `daft.remote`) and a
    // different DAFT_SOURCE_WORKTREE (wherever the command was invoked) —
    // documented on the command, not papered over here.
    // The task name is immaterial: it only feeds DAFT_TASK, which is
    // event-scoped and filtered out by the projection.
    rows_from_context(&HookContext::for_task(
        "",
        &target.root,
        git_dir,
        "origin",
        wt,
        task_branch_name(repo.as_ref()),
    ))
}

/// Project a job's environment onto the rows `daft env` shows: the `AtRest`
/// names, in the classifier's order (`vars()` is a `HashMap`).
fn rows_from_context(ctx: &crate::hooks::HookContext) -> Vec<ResolvedValue> {
    use crate::hooks::{DaftVarKind, HookEnvironment, daft_var_kind, daft_var_names};

    let env = HookEnvironment::from_context(ctx);
    daft_var_names()
        .into_iter()
        .filter(|name| daft_var_kind(name) == DaftVarKind::AtRest)
        .filter_map(|name| {
            env.get(name).map(|value| ResolvedValue {
                name: name.to_string(),
                value: value.to_string(),
                source: ValueSource::Context,
            })
        })
        .collect()
}

/// Open the addressed worktree's repository, or `None` when it cannot be
/// opened as *that* worktree.
///
/// `gix::discover` ascends, so a worktree whose `.git` link is missing or
/// broken would otherwise be answered from the enclosing project root's bare
/// repo — reporting the bare repo's HEAD as the worktree's branch. The
/// workdir check rejects that: a handle is accepted only when it actually
/// describes the path that was asked about.
fn open_target_repo(target: &Target, local: &LocalContext) -> Option<gix::Repository> {
    // Reuse the initial discovery when it already describes the target.
    if target.worktree_path.is_some() && target.worktree_path == local.worktree_path {
        return local.repo.clone();
    }
    let Some(wt) = target.worktree_path.as_ref() else {
        // Hypothetical worktree: only the repo-level facts are wanted, and
        // an ascent from the project root stays inside the same repo.
        if local.project_root.as_deref() == Some(target.root.as_path()) {
            return local.repo.clone();
        }
        return gix::discover(&target.root).ok();
    };
    let repo = gix::discover(wt).ok()?;
    // Canonicalize both sides: the discovery reports the real path while the
    // caller may hold a symlinked one (/tmp vs /private/tmp on macOS).
    let found = repo.workdir()?;
    let found = std::fs::canonicalize(found).unwrap_or_else(|_| found.to_path_buf());
    let asked = std::fs::canonicalize(wt).unwrap_or_else(|_| wt.clone());
    (found == asked).then_some(repo)
}

/// The branch name a task would receive, mirroring `commands/run.rs`'s
/// `get_current_branch().unwrap_or_else(|_| "HEAD")`.
///
/// `git symbolic-ref --short HEAD` — which that path shells out to — succeeds
/// on an **unborn** HEAD (the branch is named, it just has no commit yet) and
/// fails only when HEAD is detached. gix's `head_ref()` returns `None` for
/// both, so matching on `head().kind` is what keeps a fresh clone of an empty
/// repo from being reported as branchless.
///
/// Note daft has two representations of "no branch": a task gets the literal
/// `HEAD` (this path), while a sandbox worktree's create/remove hooks get an
/// empty `DAFT_BRANCH_NAME` plus `DAFT_COMMIT`. Unifying them is a change to
/// the hook contract, not to this view of it.
fn task_branch_name(repo: Option<&gix::Repository>) -> String {
    let detached = "HEAD".to_string();
    let Some(repo) = repo else { return detached };
    let Ok(head) = repo.head() else {
        return detached;
    };
    match head.kind {
        gix::head::Kind::Symbolic(ref reference) => reference.name.shorten().to_string(),
        gix::head::Kind::Unborn(ref name) => name.shorten().to_string(),
        gix::head::Kind::Detached { .. } => detached,
    }
}

fn warn_on_block_collision(spec: &EnvSpec, target: &Target, output: &mut dyn Output) {
    if spec.ports.is_empty() || target.sibling_slugs.is_empty() {
        return;
    }
    if std::env::var_os("DAFT_ENV_NO_COLLISION_CHECK").is_some_and(|v| v == "1") {
        return;
    }
    let Ok(collisions) = spec.block_collisions(&target.slug, &target.sibling_slugs) else {
        return;
    };
    if let Some(first) = collisions.first() {
        let base = spec.block_base(&target.slug).ok();
        output.warning(&format!(
            "worktree '{first}' hashes to the same port block{}; ports overlap — \
             consider renaming one worktree or setting a different env.salt",
            base.map(|b| format!(" ({b}-{})", b + spec.block_size - 1))
                .unwrap_or_default()
        ));
    }
}

/// `export NAME='value'`, single-quoted with `'\''` escaping — safe for
/// `eval` in every POSIX shell.
fn export_line(value: &ResolvedValue) -> String {
    format!(
        "export {}='{}'",
        value.name,
        value.value.replace('\'', r"'\''")
    )
}

/// Dotenv line: bare when the value is boring, double-quoted otherwise.
fn dotenv_line(value: &ResolvedValue) -> String {
    let boring = value
        .value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./:".contains(c));
    if boring {
        format!("{}={}", value.name, value.value)
    } else {
        format!(
            "{}=\"{}\"",
            value.name,
            value.value.replace('\\', r"\\").replace('"', "\\\"")
        )
    }
}

fn write_dotenv(
    write_arg: &str,
    spec: &EnvSpec,
    resolved: &[ResolvedValue],
    target: &Target,
) -> Result<()> {
    let mut body = String::from(
        "# Generated by daft env --write — derived per-worktree values.\n\
         # Deterministic: regenerate anytime; do not commit.\n",
    );
    for value in resolved {
        body.push_str(&dotenv_line(value));
        body.push('\n');
    }

    // Explicit path wins; empty string means bare `--write` → env.write.
    let rel = if write_arg.is_empty() {
        spec.write
            .clone()
            .context("no write target: pass one (`--write .env`) or set `env.write` in daft.yml")?
    } else {
        write_arg.to_string()
    };
    if rel == "-" {
        print!("{body}");
        return Ok(());
    }
    let rel_path = Path::new(&rel);
    if rel_path.is_absolute() {
        bail!("--write target must be worktree-relative (got '{rel}')");
    }
    if rel_path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        bail!("--write target must not escape the worktree ('{rel}')");
    }
    let root = target.worktree_path.as_ref().with_context(|| {
        format!(
            "worktree '{}' does not exist; --write needs a real worktree \
             (or use `--write -` for stdout)",
            target
                .requested_name
                .as_deref()
                .unwrap_or(target.slug.as_str())
        )
    })?;
    let dest = root.join(rel_path);

    // Refuse to overwrite a tracked file — a generated dotenv must never
    // clobber committed content. (Not the eval path: a git subprocess is
    // fine in an explicit mutation.)
    let tracked = crate::utils::git_command_at(root)
        .args(["ls-files", "--error-unmatch", "--", &rel])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if tracked {
        bail!(
            "'{rel}' is tracked in git; refusing to overwrite committed content \
             (pick an untracked, gitignored target)"
        );
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    // Temp-file + rename in the destination directory: readers never see a
    // half-written file.
    let dir = dest.parent().unwrap_or(root);
    let tmp = tempfile::NamedTempFile::new_in(dir).context("failed to create temp file")?;
    std::fs::write(tmp.path(), &body).context("failed to write dotenv content")?;
    tmp.persist(&dest)
        .with_context(|| format!("failed to move dotenv into place at {}", dest.display()))?;
    eprintln!("wrote {} ({} values)", dest.display(), resolved.len());
    Ok(())
}

/// Structured output is the **declared set only** — the same boundary
/// `--export` and `--write` draw, and for the same reason: `--format json`
/// is consumed by machines that store what they read, and daft's own
/// variables are ambient state that goes stale the moment the worktree
/// moves. They stay on the inspection surface (the human listing), which is
/// read once and thrown away.
fn emit_table(resolved: &[ResolvedValue], emit_args: &EmitArgs) -> Result<()> {
    let mut table = Table::new(vec![
        "name".to_string(),
        "value".to_string(),
        "source".to_string(),
        "offset".to_string(),
    ]);
    for value in resolved {
        table.rows.push(vec![
            Cell::str(&value.name),
            Cell::str(&value.value),
            Cell::str(value.source.label()),
            match value.source {
                ValueSource::DeclaredPort { offset } => Cell::int(i64::from(offset)),
                _ => Cell::null(),
            },
        ]);
    }
    emit::emit_and_handle(
        "daft-env",
        EmitPayload::Tabular(table),
        emit_args,
        &mut std::io::stdout(),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

fn print_human_table(
    spec: &EnvSpec,
    slug: &str,
    resolved: &[ResolvedValue],
    context: &[ResolvedValue],
    output: &mut dyn Output,
) {
    // Sections get their own column widths: context values are long paths,
    // and one shared width would pad every port row out to them. Widths
    // count chars, not bytes — `{:<w$}` pads by chars, so a byte width
    // over-pads any value with non-ASCII in it.
    let print_rows = |rows: &[ResolvedValue], output: &mut dyn Output| {
        let width = |s: &str| s.chars().count();
        let name_w = rows.iter().map(|v| width(&v.name)).max().unwrap_or(4);
        let value_w = rows.iter().map(|v| width(&v.value)).max().unwrap_or(5);
        for value in rows {
            let kind = match value.source {
                ValueSource::DeclaredPort { offset } => format!("port +{offset}"),
                ValueSource::Value => "value".to_string(),
                ValueSource::AdHoc => "ad-hoc".to_string(),
                ValueSource::Context => "context".to_string(),
            };
            output.info(&format!(
                "{:<name_w$}  {:<value_w$}  {}",
                value.name,
                value.value,
                dim(&kind),
            ));
        }
    };

    if resolved.is_empty() {
        output.info("No env values declared in daft.yml.");
        output.info(&format!(
            "Any name still resolves ad-hoc: {} — or declare an env: section \
             for a stable block.",
            cyan(&crate::daft_cmd("env MY_PORT"))
        ));
    } else {
        print_rows(resolved, output);
        if !spec.ports.is_empty()
            && let Ok(base) = spec.block_base(slug)
        {
            output.info(&dim(&format!(
                "block {base}-{}  worktree '{slug}'  salt '{}'",
                base + spec.block_size - 1,
                spec.salt
            )));
        }
    }

    // Daft's own variables — what a job here would additionally receive.
    // Labelled: with an empty schema this section is the entire output, and
    // unheaded it reads as the declared values rather than daft's own.
    if !context.is_empty() {
        output.info("");
        output.info(&dim(
            "daft's own variables, as a job here would receive them:",
        ));
        print_rows(context, output);
    }
}

fn dir_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn val(name: &str, value: &str) -> ResolvedValue {
        ResolvedValue {
            name: name.into(),
            value: value.into(),
            source: ValueSource::Value,
        }
    }

    #[test]
    fn export_lines_survive_hostile_values() {
        assert_eq!(
            export_line(&val("A_PORT", "23952")),
            "export A_PORT='23952'"
        );
        // Single quotes are closed, escaped, reopened — eval-safe.
        assert_eq!(
            export_line(&val("X", "it's; rm -rf /")),
            r"export X='it'\''s; rm -rf /'"
        );
    }

    #[test]
    fn dotenv_lines_quote_only_when_needed() {
        assert_eq!(dotenv_line(&val("A_PORT", "23952")), "A_PORT=23952");
        assert_eq!(
            dotenv_line(&val("URL", "http://localhost:23952/x")),
            "URL=http://localhost:23952/x"
        );
        assert_eq!(
            dotenv_line(&val("MSG", "hello world \"x\"")),
            "MSG=\"hello world \\\"x\\\"\""
        );
    }

    #[test]
    fn output_mode_exclusivity_is_enforced() {
        let args = Args::parse_from(["daft-env", "--export", "--write", ".env"]);
        let mut out = CliOutput::new(OutputConfig::default());
        let err = cmd_env(&args, &mut out).unwrap_err();
        assert!(err.to_string().contains("mutually exclusive"));
    }

    #[test]
    fn address_and_flag_qualifiers_conflict() {
        let args = Args::parse_from(["daft-env", "backend:API_PORT", "--repo", "other"]);
        let mut out = CliOutput::new(OutputConfig::default());
        let err = cmd_env(&args, &mut out).unwrap_err();
        assert!(err.to_string().contains("one spelling"));
    }

    fn fake_target(worktree: Option<&str>, branch: Option<&str>) -> Target {
        Target {
            root: PathBuf::from("/p"),
            slug: "feat".into(),
            worktree_path: worktree.map(PathBuf::from),
            branch: branch.map(str::to_string),
            sibling_slugs: Vec::new(),
            requested_name: None,
        }
    }

    fn no_local() -> LocalContext {
        LocalContext {
            project_root: None,
            worktree_path: None,
            branch: None,
            repo: None,
        }
    }

    /// **The weld.** What `daft env` shows must be a projection of the
    /// environment a job actually receives — not a second model of it.
    /// Every `AtRest` name is present, with byte-identical values, and
    /// nothing event-scoped leaks in.
    ///
    /// The previous hand-rolled builder is why `DAFT_BRANCH_NAME`,
    /// `DAFT_COMMIT` and `DAFT_REMOTE` could each drift from the job
    /// surface at once. This test is what makes that class of bug fail
    /// here instead of in someone's hook.
    #[test]
    fn context_rows_are_a_projection_of_the_job_environment() {
        use crate::hooks::{DaftVarKind, HookContext, HookEnvironment, daft_var_kind};

        let ctx = HookContext::for_task("dev", "/p", "/p/.git", "origin", "/p/feat", "feat");
        let env = HookEnvironment::from_context(&ctx);
        let rows = rows_from_context(&ctx);

        for row in &rows {
            assert_eq!(
                env.get(&row.name),
                Some(row.value.as_str()),
                "{} is shown with a value no job would receive",
                row.name
            );
            assert_eq!(
                daft_var_kind(&row.name),
                DaftVarKind::AtRest,
                "{} is shown at rest but is event-scoped",
                row.name
            );
            assert_eq!(row.source, ValueSource::Context);
        }
        // …and nothing at rest is missing.
        let shown: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        for name in crate::hooks::daft_var_names() {
            if daft_var_kind(name) == DaftVarKind::AtRest {
                assert!(shown.contains(&name), "{name} is at rest but not shown");
            }
        }
    }

    /// A worktree that will not open as a git worktree yields only the
    /// repo-level facts. A job cannot run there at all, so reporting the
    /// paths it "would" receive would be an answer about a job that can
    /// never happen — and `answer_context_var` says so by name.
    #[test]
    fn context_rows_without_a_repo_handle_stop_at_repo_facts() {
        let rows = context_rows(&fake_target(Some("/p/feat"), Some("feat")), &no_local());
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["DAFT_PROJECT_ROOT"]);
        assert!(rows.iter().all(|r| r.source == ValueSource::Context));
    }

    /// A hypothetical worktree answers only what exists without it.
    #[test]
    fn context_rows_for_a_hypothetical_worktree_stop_at_repo_facts() {
        let rows = context_rows(&fake_target(None, None), &no_local());
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["DAFT_PROJECT_ROOT"]);
    }

    /// An unopenable worktree is diagnosed as such, not as "on a branch".
    #[test]
    fn an_unreadable_worktree_says_so_rather_than_guessing() {
        let args = Args::parse_from(["daft-env", "DAFT_BRANCH_NAME"]);
        let err = answer_context_var(
            &args,
            &fake_target(Some("/p/feat"), Some("feat")),
            &no_local(),
            "DAFT_BRANCH_NAME",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("could not read worktree"), "{msg}");
        assert!(msg.contains(".git link"), "{msg}");
    }

    /// DAFT_* + a derivation flag is a category error, not a computation.
    #[test]
    fn daft_vars_refuse_derivation_flags() {
        let args = Args::parse_from(["daft-env", "DAFT_BRANCH_NAME", "--ad-hoc"]);
        let err = answer_context_var(
            &args,
            &fake_target(Some("/p/feat"), Some("feat")),
            &no_local(),
            "DAFT_BRANCH_NAME",
        )
        .unwrap_err();
        assert!(err.to_string().contains("do not apply"));
    }

    /// Event-scoped variables explain themselves instead of guessing.
    #[test]
    fn event_scoped_daft_vars_say_when_they_exist() {
        let args = Args::parse_from(["daft-env", "DAFT_REMOVAL_REASON"]);
        let err = answer_context_var(
            &args,
            &fake_target(Some("/p/feat"), Some("feat")),
            &no_local(),
            "DAFT_REMOVAL_REASON",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no value at rest"), "{msg}");
        assert!(msg.contains("remove hooks"), "{msg}");
    }

    /// A typo'd DAFT_ name suggests the nearest real one.
    #[test]
    fn unknown_daft_vars_suggest_the_nearest_real_one() {
        let args = Args::parse_from(["daft-env", "DAFT_BRANCH_NAM"]);
        let err = answer_context_var(
            &args,
            &fake_target(Some("/p/feat"), Some("feat")),
            &no_local(),
            "DAFT_BRANCH_NAM",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not a variable daft sets"), "{msg}");
        assert!(msg.contains("DAFT_BRANCH_NAME"), "{msg}");
    }

    /// DAFT_COMMIT is event-scoped: only a branchless worktree's own
    /// create/remove hooks set it, and no task ever receives it. Listing it
    /// at rest with a live OID invited jobs to read a variable that is
    /// always empty for them.
    #[test]
    fn daft_commit_is_event_scoped_not_at_rest() {
        let args = Args::parse_from(["daft-env", "DAFT_COMMIT"]);
        let err = answer_context_var(
            &args,
            &fake_target(Some("/p/feat"), Some("feat")),
            &no_local(),
            "DAFT_COMMIT",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no value at rest"), "{msg}");
        assert!(msg.contains("tasks never receive it"), "{msg}");
    }

    /// Serializing surfaces stay the declared set — all of them. `--export`
    /// is refused for daft's own variables and `--write` takes no VAR at
    /// all; the docs promise both, and a silently-ignored `--write` leaves
    /// the caller reading a file that was never written.
    #[test]
    fn serializing_flags_refuse_daft_vars() {
        let args = Args::parse_from(["daft-env", "DAFT_BRANCH_NAME", "--export"]);
        let err = answer_context_var(
            &args,
            &fake_target(Some("/p/feat"), Some("feat")),
            &no_local(),
            "DAFT_BRANCH_NAME",
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("serializes declared values only"),
            "{err}"
        );

        // Refused before any discovery, so this never reads a real repo.
        let args = Args::parse_from(["daft-env", "WEBAPP_PORT", "--write", ".env"]);
        let mut out = CliOutput::new(OutputConfig::default());
        let err = cmd_env(&args, &mut out).unwrap_err();
        assert!(err.to_string().contains("takes no VAR"), "{err}");
    }
}
