//! Converts hook inputs into generic [`JobSpec`] values.
//!
//! This is the boundary between the hooks input layer (YAML `JobDef`,
//! legacy script paths) and the format-agnostic executor. All
//! hook-specific resolution (command, environment, working directory,
//! RC-file sourcing, template substitution) happens here so that the
//! executor never needs to know about hook configuration details.

use super::changed_files::{ChangedFilesProvider, FileFilter, FileSources};
use super::config_merge::merge_log_configs;
use crate::executor::{JobSpec, LogConfig};
use crate::hooks::environment::{HookContext, HookEnvironment};
use crate::hooks::yaml_config::JobDef;
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Why-category of a pre-execution skip — the typed half of the skip
/// taxonomy. The human-readable `reason` string carries the detail; the
/// kind is what machine consumers (structured emit) and styling switch on.
/// Defined here, at the production site, so the taxonomy cannot drift from
/// the strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipKind {
    /// No changed file matched the job's `glob:`/`exclude:`/`files:` set.
    Glob,
    /// A `skip:`/`only:` condition held (env, ref, run, changed, bool).
    Condition,
    /// Requested by selection (`--skip-hooks`, `--skip-tag`).
    Tag,
    /// The job's platform constraint doesn't match this machine.
    Platform,
    /// Not run for a structural reason (an excluded dependency).
    NotRun,
}

impl SkipKind {
    /// Stable machine token for structured output.
    pub fn as_str(&self) -> &'static str {
        match self {
            SkipKind::Glob => "glob",
            SkipKind::Condition => "condition",
            SkipKind::Tag => "tag",
            SkipKind::Platform => "platform",
            SkipKind::NotRun => "not-run",
        }
    }
}

/// A job that was declared in YAML but filtered out before execution.
///
/// Produced by `yaml_jobs_to_specs` alongside the kept `JobSpec`s so the
/// caller can record a skipped-job entry in the log store.
#[derive(Debug, Clone)]
pub struct SkippedJob {
    pub name: String,
    pub background: bool,
    pub reason: String,
    pub kind: SkipKind,
}

/// Resolve a job's effective `background:` flag: the job's own setting wins,
/// then the hook-level default, then `false`. Centralizes the precedence rule
/// shared by kept-job spec conversion ([`yaml_jobs_to_specs`]) and the
/// `--skip-hooks` skip-attribution paths so it lives in exactly one place.
pub fn resolve_background(job_background: Option<bool>, hook_background: Option<bool>) -> bool {
    job_background.or(hook_background).unwrap_or(false)
}

/// Parsed `--skip-hooks` selectors.
///
/// Built by [`parse_skip_selectors`] from the raw CLI tokens and carried on
/// [`crate::hooks::yaml_executor::JobFilter`]. Drives the exclude side of hook
/// filtering: [`compute_skip_cascade`] consumes `names`/`tags`, while `all`
/// short-circuits the whole fire and `raw` is retained for no-match warnings.
#[derive(Debug, Default, Clone)]
pub struct SkipSelectors {
    /// `all` / `*` selector: skip every job (short-circuit the hook fire).
    pub all: bool,
    /// Job names to skip (from bare `<name>` and `job:<name>` selectors).
    pub names: Vec<String>,
    /// Tags to skip (from `tag:<tag>` selectors).
    pub tags: Vec<String>,
    /// Hook types to skip wholesale (from bare hook-type names like
    /// `worktree-post-create`), parsed into typed [`crate::hooks::HookType`]
    /// values. Matched against the current fire's hook name in the executor
    /// (via [`crate::hooks::HookType::yaml_name`]), where it short-circuits the
    /// whole hook (like `all`, but scoped to one hook type). Does NOT feed
    /// [`compute_skip_cascade`] — it is a hook-level, not job-level, selector.
    pub hook_types: Vec<crate::hooks::HookType>,
    /// Original selector tokens, retained for no-match warning attribution.
    pub raw: Vec<String>,
    /// What asked for the skip, for messages that attribute it. `None` is the
    /// `--skip-hooks` flag. Set when something else populates these selectors
    /// — an incumbent config's own exclusion variable, say — so the report
    /// names a knob the user actually turned.
    pub origin: Option<&'static str>,
}

impl SkipSelectors {
    /// True when no selectors were supplied (the common case — no
    /// `--skip-hooks` flag). Lets callers cheaply bypass the exclude path.
    pub fn is_empty(&self) -> bool {
        !self.all && self.names.is_empty() && self.tags.is_empty() && self.hook_types.is_empty()
    }

    /// What to name as the source of these skips in user-facing text.
    pub fn origin_label(&self) -> &'static str {
        self.origin.unwrap_or(SKIP_HOOKS_FLAG)
    }
}

/// The flag that requests skips by default, and the label reported for them.
pub const SKIP_HOOKS_FLAG: &str = "--skip-hooks";

/// Parse raw `--skip-hooks` tokens into a [`SkipSelectors`].
///
/// Precedence:
/// - `tag:<tag>` → tag selector
/// - `job:<name>` → name selector (escape hatch; `job:all` is the job literally
///   named `all`, NOT the wildcard; `job:worktree-post-create` is the job
///   literally named after a hook type, NOT the hook-type selector)
/// - `all` / `*` → wildcard (`all = true`)
/// - a canonical hook-type name (`worktree-post-create`, `post-clone`, …) →
///   hook-type selector (skip that whole hook). Only the canonical
///   [`crate::hooks::HookType::yaml_name`] spellings are reserved; deprecated
///   short forms (`post-create`) are treated as job names.
/// - anything else → bare name selector
pub fn parse_skip_selectors(selectors: &[String]) -> SkipSelectors {
    use crate::hooks::HookType;
    let mut out = SkipSelectors {
        raw: selectors.to_vec(),
        ..Default::default()
    };
    for sel in selectors {
        let s = sel.trim();
        if let Some(tag) = s.strip_prefix("tag:") {
            out.tags.push(tag.to_string());
        } else if let Some(name) = s.strip_prefix("job:") {
            out.names.push(name.to_string());
        } else if s == "all" || s == "*" {
            out.all = true;
        } else if let Some(ht) = HookType::from_yaml_name(s) {
            out.hook_types.push(ht);
        } else {
            out.names.push(s.to_string());
        }
    }
    out
}

/// Why a job was excluded by `--skip-hooks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipCause {
    /// Direct selector match (job name or tag).
    Requested,
    /// Cascade: this job `needs:` an excluded job. Holds the *immediate*
    /// excluded dependency (not the cascade root) so the rendered reason reads
    /// `depends on <that dep> (skipped)`.
    DependsOn(String),
}

impl SkipCause {
    /// Render the user-facing skip reason for this cause.
    ///
    /// `origin` names what asked — see [`SkipSelectors::origin_label`]. A skip
    /// attributed to a flag the user never passed is a small lie that costs
    /// them the search for where it came from.
    pub fn reason(&self, origin: &str) -> String {
        match self {
            SkipCause::Requested => format!("requested ({origin})"),
            SkipCause::DependsOn(dep) => format!("depends on {dep} (skipped)"),
        }
    }
}

/// Result of [`compute_skip_cascade`]: which jobs are excluded (and why), plus
/// selector tokens that matched nothing (for warnings).
#[derive(Debug, Default)]
pub struct SkipCascade {
    /// Excluded job name → cause. Membership lookup only; iterate the original
    /// `jobs` slice in declaration order when ordered output is needed.
    pub excluded: std::collections::HashMap<String, SkipCause>,
    /// Selector tokens (`<name>` / `tag:<tag>`) that matched no job.
    pub unmatched: Vec<String>,
}

/// Compute the set of jobs excluded by `--skip-hooks`, with attribution.
///
/// Operates on [`JobDef`]s while `needs:` is still intact — call BEFORE
/// [`yaml_jobs_to_specs`]. Does NOT handle the `all` selector; the caller
/// short-circuits that earlier (see `execute_yaml_hook_with_rc`).
///
/// Algorithm:
/// 1. **Direct pass** (declaration order): a named job is `Requested` if its
///    name is in `skip.names` OR any of its tags is in `skip.tags`.
/// 2. **Cascade pass** (BFS over reverse-`needs`): any job that `needs:` an
///    already-excluded job becomes `DependsOn(<that immediate dep>)`. The BFS
///    is seeded and scanned in declaration order so the immediate-parent
///    attribution is deterministic (e.g. the diamond case picks the
///    first-declared excluded dependency).
/// 3. **Unmatched**: each `skip.names` entry matching no `job.name`, and each
///    `skip.tags` entry present on no job, is collected for a warning.
///
/// Only named jobs participate (matching and cascade are name-keyed); unnamed
/// and `group:` jobs are out of scope here — groups carry their own skip
/// handling in [`yaml_jobs_to_specs`].
pub fn compute_skip_cascade(jobs: &[JobDef], skip: &SkipSelectors) -> SkipCascade {
    use std::collections::{HashMap, VecDeque};

    let mut excluded: HashMap<String, SkipCause> = HashMap::new();

    // ── Direct pass (declaration order) ──
    let mut seeds: VecDeque<String> = VecDeque::new();
    for job in jobs {
        let Some(name) = job.name.as_deref() else {
            continue;
        };
        let name_match = skip.names.iter().any(|n| n == name);
        let tag_match = job
            .tags
            .as_ref()
            .is_some_and(|tags| tags.iter().any(|t| skip.tags.contains(t)));
        if name_match || tag_match {
            excluded
                .entry(name.to_string())
                .or_insert(SkipCause::Requested);
            seeds.push_back(name.to_string());
        }
    }

    // ── Cascade pass (BFS over reverse-`needs`) ──
    while let Some(dep) = seeds.pop_front() {
        // Scan jobs in declaration order so the first dependent discovered keeps
        // a stable immediate-parent attribution.
        for job in jobs {
            let Some(name) = job.name.as_deref() else {
                continue;
            };
            if excluded.contains_key(name) {
                continue;
            }
            let needs_dep = job
                .needs
                .as_ref()
                .is_some_and(|needs| needs.iter().any(|n| n == &dep));
            if needs_dep {
                excluded.insert(name.to_string(), SkipCause::DependsOn(dep.clone()));
                seeds.push_back(name.to_string());
            }
        }
    }

    // ── Unmatched selectors (for warnings) ──
    let mut unmatched = Vec::new();
    for n in &skip.names {
        let matched = jobs.iter().any(|j| j.name.as_deref() == Some(n.as_str()));
        if !matched {
            unmatched.push(n.clone());
        }
    }
    for t in &skip.tags {
        let matched = jobs.iter().any(|j| {
            j.tags
                .as_ref()
                .is_some_and(|tags| tags.iter().any(|tag| tag == t))
        });
        if !matched {
            unmatched.push(format!("tag:{t}"));
        }
    }

    SkipCascade {
        excluded,
        unmatched,
    }
}

/// Passthrough context for [`yaml_jobs_to_specs`]: values sourced from the
/// outer hook execution but threaded unchanged into every job spec.
///
/// Tests pass `&JobAdapterContext::default()` since they exercise spec
/// translation in isolation; production wraps the relevant hook-level
/// values from `execute_yaml_hook_with_rc`.
#[derive(Debug, Clone, Copy)]
pub struct JobAdapterContext<'a> {
    /// Path to an RC file whose `source` command is prepended to every job
    /// command — opt-in shell setup like `~/.bashrc`.
    pub rc: Option<&'a str>,
    /// Hook-level `background:` default applied when a job doesn't set its
    /// own.
    pub hook_background: Option<bool>,
    /// Top-level `log:` config from `daft.yml`, merged into each job's
    /// `log_config` so cleanup policies inherit repo-wide defaults.
    pub repo_log: Option<&'a LogConfig>,
    /// Default timeout stamped on each produced [`JobSpec`]. Lifecycle hooks
    /// use `Some(JobSpec::DEFAULT_TIMEOUT)` (the `Default`); `daft run` tasks
    /// pass `None` so long-running processes aren't force-killed.
    pub default_timeout: Option<std::time::Duration>,
    /// The file lists this hook fire can offer, consulted by file-aware jobs
    /// (`glob:`/`exclude:`/`{files}` and friends) and `changed:` rules. A
    /// hook with no source at all makes a file-aware job a config error
    /// unless it brings its own `files:` command.
    pub file_sources: Option<&'a FileSources>,
    /// Hook-level `exclude:` patterns, appended to every file-aware job's
    /// exclude set. Never makes a job file-aware by itself.
    pub hook_exclude: &'a [String],
    /// Top-level `templates:` fragments, expanded in every `run:` before the
    /// other template variables.
    pub templates: Option<&'a HashMap<String, String>>,
    /// The stage's stdin, replayed into jobs that declare `use_stdin:`.
    pub stage_stdin: Option<&'a str>,
    /// Repository root, for `file_types:` checks — the coordinate system
    /// every file list already uses.
    pub repo_root: Option<&'a Path>,
}

impl Default for JobAdapterContext<'_> {
    fn default() -> Self {
        // Preserve the historical hook default so the many
        // `&JobAdapterContext::default()` test callers keep 300s timeouts.
        Self {
            rc: None,
            hook_background: None,
            repo_log: None,
            default_timeout: Some(JobSpec::DEFAULT_TIMEOUT),
            file_sources: None,
            hook_exclude: &[],
            templates: None,
            stage_stdin: None,
            repo_root: None,
        }
    }
}

impl JobAdapterContext<'_> {
    /// The provider a bare file reference (`glob:`, `changed:`, `{files}`)
    /// resolves to, or `None` when this hook has no default list.
    fn default_files(&self) -> Option<&ChangedFilesProvider> {
        self.file_sources?.default_provider()
    }
}

/// Convert YAML job definitions into format-agnostic [`JobSpec`] values.
///
/// Each [`JobDef`] is resolved into a concrete command string, merged
/// environment, and working directory. This function handles:
/// - Command resolution (run, script, runner, args, platform-specific variants)
/// - Changed-file filtering (`glob:`/`exclude:`/`files:`, `{changed_files}`)
/// - Environment variable merging (hook env + per-job env)
/// - Working directory resolution (job `root` relative to base working dir)
/// - RC file sourcing (prepends `source <rc> &&` to commands)
/// - Template variable substitution
///
/// Jobs with platform-specific `run` maps that have no entry for the
/// current OS are silently excluded.
///
/// Errors are configuration or resolution failures that must abort the hook
/// (malformed glob, `changed:`/`glob:` without a changed-file source, a
/// failing `files:` command) — never conditions a skip can express.
///
/// **Note:** Group jobs (`job.group.is_some()`) are skipped in this
/// initial implementation. They will be handled when the yaml_executor
/// is fully migrated to the generic executor.
pub fn yaml_jobs_to_specs(
    jobs: &[JobDef],
    ctx: &HookContext,
    hook_env: &HashMap<String, String>,
    source_dir: &str,
    working_dir: &Path,
    adapter: &JobAdapterContext<'_>,
) -> Result<(Vec<JobSpec>, Vec<SkippedJob>)> {
    let rc = adapter.rc;
    let hook_background = adapter.hook_background;
    let repo_log = adapter.repo_log;
    let default_timeout = adapter.default_timeout;
    let mut kept: Vec<JobSpec> = Vec::new();
    let mut skipped: Vec<SkippedJob> = Vec::new();

    // Nesting is rewritten away before anything else looks at the list: the
    // executor schedules a flat set with `needs:` edges and has no notion of
    // a group, so every later stage sees `lint/eslint`, not `lint`.
    let flattened = super::groups::flatten(jobs)?;
    let jobs: &[JobDef] = &flattened;

    for job in jobs {
        let name = job.name.clone().unwrap_or_else(|| "(unnamed)".to_string());
        let declared_background = resolve_background(job.background, hook_background);

        if super::yaml_executor::is_platform_skip(job) {
            skipped.push(SkippedJob {
                name,
                background: declared_background,
                reason: format!(
                    "skip: platform-specific run has no entry for {}",
                    std::env::consts::OS
                ),
                kind: SkipKind::Platform,
            });
            continue;
        }

        if let Some(ref skip) = job.skip
            && let Some(info) =
                super::conditions::should_skip(skip, working_dir, adapter.default_files())?
        {
            skipped.push(SkippedJob {
                name,
                background: declared_background,
                reason: info.reason,
                kind: SkipKind::Condition,
            });
            continue;
        }

        if let Some(ref only) = job.only
            && let Some(info) =
                super::conditions::should_only_skip(only, working_dir, adapter.default_files())?
        {
            skipped.push(SkippedJob {
                name,
                background: declared_background,
                reason: info.reason,
                kind: SkipKind::Condition,
            });
            continue;
        }

        let cmd = super::yaml_executor::resolve_command(job, ctx, Some(&name), source_dir);
        let cmd = expand_templates(&cmd, adapter.templates);

        // Recorded before substitution: the filtered list is injected below,
        // after which the markers are gone.
        let injects_changed_files = !super::changed_files::find_placeholders(&cmd).is_empty();

        let (cmd, extra_chunks) =
            match apply_file_filter(job, &name, &cmd, ctx, working_dir, adapter)? {
                FileFilterOutcome::Run {
                    command,
                    extra_chunks,
                } => (command, extra_chunks),
                FileFilterOutcome::Skip(reason) => {
                    skipped.push(SkippedJob {
                        name,
                        background: declared_background,
                        reason,
                        kind: SkipKind::Glob,
                    });
                    continue;
                }
            };

        let with_rc = |c: String| match rc {
            Some(rc_path) => format!("source {rc_path} && {c}"),
            None => c,
        };
        let cmd = with_rc(cmd);
        let extra_chunks: Vec<String> = extra_chunks.into_iter().map(with_rc).collect();

        let mut env = hook_env.clone();
        if let Some(ref job_env) = job.env {
            // Substitute template variables in user-authored env *values* (keys
            // are left verbatim). The computed hook env — DAFT_* plus
            // ctx.extra_env — was merged above and is never substituted. This
            // is the single surface that gives both lifecycle hooks and `daft
            // run` tasks env-value templating (e.g. `api-{worktree_slug}`) (#708).
            env.extend(
                job_env
                    .iter()
                    .map(|(k, v)| (k.clone(), super::template::substitute(v, ctx, Some(&name)))),
            );
        }

        // `root:` supports template variables (e.g. `{merge_source_path}` to
        // run a merge ring in the source worktree). Resolution is
        // FAIL-CLOSED: a `{merge_…}` placeholder that survives substitution
        // (no worktree for the source, multiple sources, a non-merge hook)
        // aborts the hook — a gate job must never silently run in the wrong
        // directory. An absolute substituted root replaces the base entirely
        // (`PathBuf::join` semantics).
        let wd = if let Some(ref root) = job.root {
            let resolved_root = super::template::substitute(root, ctx, Some(&name));
            if resolved_root.contains("{merge_") {
                bail!(
                    "job '{name}': root: '{root}' references a merge worktree path that is \
                     not available here (the source has no worktree, the merge has multiple \
                     sources, or this is not a merge hook) — refusing to run the job in an \
                     undefined directory"
                );
            }
            working_dir.join(resolved_root)
        } else {
            working_dir.to_path_buf()
        };

        // `{changed_files}` yields repository-root-relative paths — the
        // coordinate system every glob pattern uses — so the job has to run
        // where those paths resolve. A `root:` naming a worktree root (what
        // `root: "{merge_source_path}"` gives) is fine; a subdirectory is
        // not: `root: web` with `glob: "web/**"` hands eslint
        // `web/src/a.ts` from cwd `<repo>/web`, which looks for
        // `web/web/src/a.ts` and fails on exactly the changes the job was
        // written to lint. Refuse at spec-build time rather than rebasing
        // silently — the same fail-closed stance as the `root:` templating
        // check above.
        if injects_changed_files && wd != working_dir && !is_repository_root(&wd) {
            bail!(
                "job '{name}': {{changed_files}} expands to repository-root-relative \
                 paths, but root: '{}' runs the job in a subdirectory where they do not \
                 resolve — drop the root:, point it at a worktree root, or give the job \
                 its own list with `files:`",
                job.root.as_deref().unwrap_or_default()
            );
        }

        // `use_stdin:` replays the payload git handed the stage. Two things
        // already own stdin for their own reasons, so combining is a config
        // error rather than a race decided by whichever wins.
        let stdin = match job.use_stdin {
            Some(true) => {
                if job.interactive == Some(true) || declared_background {
                    bail!(
                        "job '{name}': use_stdin cannot be combined with {} — both need \
                         stdin for something else",
                        if job.interactive == Some(true) {
                            "interactive"
                        } else {
                            "background"
                        }
                    );
                }
                adapter.stage_stdin.map(str::to_string)
            }
            _ => None,
        };

        kept.push(JobSpec {
            name,
            command: cmd,
            extra_chunks,
            stdin,
            working_dir: wd,
            env,
            description: job.description.clone(),
            needs: job.needs.clone().unwrap_or_default(),
            interactive: job.interactive == Some(true),
            fail_text: job.fail_text.clone(),
            timeout: default_timeout,
            background: declared_background,
            background_output: job.background_output.clone(),
            log_config: merge_job_log(job.log.clone(), repo_log),
            tags: job.tags.clone().unwrap_or_default(),
        });
    }

    // Remove needs: references to skipped jobs so dependent jobs don't
    // fail DAG construction. A skipped dependency is vacuously satisfied —
    // it was never going to run, and we shouldn't block dependents on it.
    let skipped_names: std::collections::HashSet<&str> =
        skipped.iter().map(|s| s.name.as_str()).collect();
    for spec in &mut kept {
        spec.needs.retain(|n| !skipped_names.contains(n.as_str()));
    }

    Ok((kept, skipped))
}

/// Outcome of the changed-file stage for one job: run the expanded command
/// (plus any follow-on chunks), or skip with a reason.
enum FileFilterOutcome {
    Run {
        command: String,
        extra_chunks: Vec<String>,
    },
    Skip(String),
}

/// Apply the job's changed-file declaration, if any.
///
/// A job is *file-aware* when it declares `glob:`, `exclude:`, or `files:`,
/// or its resolved command uses `{changed_files}`. For such a job the file
/// list is resolved (the job's own `files:` command, else the hook's
/// operation source), filtered by `glob:` plus the combined job- and
/// hook-level `exclude:` patterns, and:
/// - an empty result is a first-class skip (mirroring the boundary rule:
///   nothing this job cares about changed — even when the command references
///   no file template);
/// - a non-empty result replaces `{changed_files}` with the shell-quoted,
///   space-joined list.
///
/// Everything else is an error: file-awareness without any source, a
/// malformed pattern, a failing `files:` command, or an unresolvable
/// operation diff. A gate filter must fail loudly, never silently run (or
/// silently skip) the wrong set.
fn apply_file_filter(
    job: &JobDef,
    name: &str,
    cmd: &str,
    ctx: &HookContext,
    working_dir: &Path,
    adapter: &JobAdapterContext<'_>,
) -> Result<FileFilterOutcome> {
    let placeholders = super::changed_files::find_placeholders(cmd);
    let file_aware = job.glob.is_some()
        || job.exclude.is_some()
        || job.files.is_some()
        || !placeholders.is_empty();
    if !file_aware {
        return Ok(FileFilterOutcome::Run {
            command: cmd.to_string(),
            extra_chunks: Vec::new(),
        });
    }

    let include: Vec<String> = job
        .glob
        .as_ref()
        .map(|g| g.as_slice().to_vec())
        .unwrap_or_default();
    let mut exclude: Vec<String> = job.exclude.clone().unwrap_or_default();
    exclude.extend(adapter.hook_exclude.iter().cloned());
    let filter = FileFilter::new(&include, &exclude).with_context(|| format!("job '{name}'"))?;

    // `file_types:` narrows what the globs selected — where a file lives
    // versus what it is. Applied inside the same filtering step so an empty
    // result is the same first-class skip.
    let types: Vec<super::file_types::FileType> = match job.file_types {
        Some(ref declared) => declared
            .as_slice()
            .iter()
            .map(|raw| super::file_types::FileType::parse(raw))
            .collect::<Result<Vec<_>>>()
            .with_context(|| format!("job '{name}': file_types"))?,
        None => Vec::new(),
    };
    let type_root = adapter.repo_root.unwrap_or(working_dir);
    let narrow = |files: Vec<String>| super::file_types::filter(type_root, &files, &types);

    // A job's own `files:` overrides every source for that job — including a
    // placeholder that names one. Nothing else would make sense: the job
    // declared where its list comes from.
    let own_files: Option<Vec<String>> = match job.files {
        Some(ref files_cmd) => {
            let files_cmd = super::template::substitute(files_cmd, ctx, Some(name));
            Some(
                super::changed_files::run_files_command(&files_cmd, working_dir)
                    .with_context(|| format!("job '{name}': files: command failed"))?,
            )
        }
        None => None,
    };

    // Resolve each placeholder from the source it names, filtered by this
    // job's patterns. `{files}` and `{changed_files}` carry `Operation`,
    // which the sources set maps to whatever this hook's default is.
    let mut resolved: Vec<(super::changed_files::Placeholder, Vec<String>)> =
        Vec::with_capacity(placeholders.len());
    for placeholder in &placeholders {
        let files = match own_files {
            Some(ref files) => files.clone(),
            None => resolve_source(placeholder.kind, name, ctx, adapter)?,
        };
        let matched = narrow(filter.filter(&files));
        if matched.is_empty() {
            return Ok(FileFilterOutcome::Skip(empty_reason(
                &filter,
                &include,
                &exclude,
                job.files.is_some(),
                Some(placeholder.kind),
            )));
        }
        resolved.push((*placeholder, matched));
    }

    // A job that is file-aware by pattern alone (no placeholder) still has to
    // consult its source: patterns select *work*, so nothing matching means
    // nothing to do.
    if placeholders.is_empty() {
        let files = match own_files {
            Some(files) => files,
            None => resolve_source(
                super::changed_files::SourceKind::Operation,
                name,
                ctx,
                adapter,
            )?,
        };
        if narrow(filter.filter(&files)).is_empty() {
            return Ok(FileFilterOutcome::Skip(empty_reason(
                &filter,
                &include,
                &exclude,
                job.files.is_some(),
                None,
            )));
        }
        return Ok(FileFilterOutcome::Run {
            command: cmd.to_string(),
            extra_chunks: Vec::new(),
        });
    }

    let mut chunks = super::changed_files::expand_and_chunk(cmd, &resolved);

    // `stage_fixed:` re-stages what the job touched, so a formatter's edits
    // land in the commit being made instead of appearing as a surprise diff
    // afterwards. It is a chunk rather than a `&&` tail because it must run
    // once for the whole file set, after every chunk of the command itself —
    // and because a chunked `git add` would hit the same argv limit.
    if job.stage_fixed == Some(true) {
        let files: Vec<String> = resolved
            .iter()
            .flat_map(|(_, files)| files.iter().cloned())
            .collect();
        if !files.is_empty() {
            chunks.push(format!("git add -- {}", crate::utils::quote_argv(&files)));
        }
    }

    let command = chunks.remove(0);
    Ok(FileFilterOutcome::Run {
        command,
        extra_chunks: chunks,
    })
}

/// Expand `templates:` fragments in a command.
///
/// One pass, longest name first so `{lint}` cannot shadow `{lint_strict}`.
/// A fragment that names another fragment is left as-is: recursive expansion
/// turns a config typo into a hang, and nothing in a command fragment needs
/// it.
fn expand_templates(command: &str, templates: Option<&HashMap<String, String>>) -> String {
    let Some(templates) = templates.filter(|t| !t.is_empty()) else {
        return command.to_string();
    };
    let mut names: Vec<&String> = templates.keys().collect();
    names.sort_by_key(|n| std::cmp::Reverse(n.len()));

    let mut out = command.to_string();
    for name in names {
        let token = format!("{{{name}}}");
        if out.contains(&token) {
            out = out.replace(&token, &templates[name]);
        }
    }
    out
}

/// The file list behind one placeholder kind, or a configuration error
/// naming what is missing.
fn resolve_source(
    kind: super::changed_files::SourceKind,
    name: &str,
    ctx: &HookContext,
    adapter: &JobAdapterContext<'_>,
) -> Result<Vec<String>> {
    use super::changed_files::SourceKind;

    let hook_label = ctx
        .task_name
        .as_deref()
        .unwrap_or_else(|| ctx.hook_type.yaml_name());

    let provider = match kind {
        SourceKind::Operation => adapter.default_files(),
        other => adapter.file_sources.and_then(|s| s.get(other)),
    };
    let Some(provider) = provider else {
        // Fail loudly rather than resolving to an empty list: a gate that
        // silently checks nothing passes green on exactly the change it was
        // written to catch.
        match kind {
            SourceKind::Operation => bail!(
                "job '{name}': glob:/exclude:/{{files}} need a file list, and '{hook_label}' \
                 does not provide one — add a `files:` command to the job, name a source \
                 explicitly ({}), or drop the file filter",
                SourceKind::all()
                    .iter()
                    .filter(|k| **k != SourceKind::Operation)
                    .map(|k| k.placeholder())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            other => bail!(
                "job '{name}': {} names {}, which '{hook_label}' cannot supply",
                other.placeholder(),
                other.describe()
            ),
        };
    };
    Ok(provider
        .files()
        .with_context(|| format!("job '{name}'"))?
        .to_vec())
}

/// Skip text for a job whose filtered list came out empty.
fn empty_reason(
    filter: &FileFilter,
    include: &[String],
    exclude: &[String],
    has_files_command: bool,
    kind: Option<super::changed_files::SourceKind>,
) -> String {
    if !include.is_empty() || !exclude.is_empty() {
        return filter.empty_reason();
    }
    if has_files_command {
        return "skip: files: command returned no files".to_string();
    }
    match kind {
        Some(kind) if kind != super::changed_files::SourceKind::Operation => {
            format!("skip: no {}", kind.describe())
        }
        _ => "skip: no changed files".to_string(),
    }
}

/// Convert legacy hook script paths into [`JobSpec`] values.
///
/// Each script path becomes a single job that runs the script directly
/// (not via `sh -c`). The environment includes all daft hook variables.
pub fn scripts_to_specs(
    hook_paths: &[PathBuf],
    env: &HookEnvironment,
    working_dir: &Path,
) -> Vec<JobSpec> {
    hook_paths
        .iter()
        .map(|path| {
            let name = path
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| "(unknown)".to_string());

            JobSpec {
                name,
                command: path.to_string_lossy().into_owned(),
                working_dir: working_dir.to_path_buf(),
                env: env.vars().clone(),
                ..Default::default()
            }
        })
        .collect()
}

/// Merge a per-job [`LogConfig`] with the repo-level default, with per-job
/// fields taking precedence. Either or both may be `None`; the result is
/// `None` only when both are.
///
/// Used by [`yaml_jobs_to_specs`] so top-level `log:` defaults in `daft.yml`
/// (e.g. `max_total_size`, `keep_last`, `stale_running_after`) flow into
/// each job's `log_config` and reach `build_repo_policy`.
fn merge_job_log(per_job: Option<LogConfig>, repo: Option<&LogConfig>) -> Option<LogConfig> {
    match (per_job, repo) {
        (None, None) => None,
        (Some(j), None) => Some(j),
        (None, Some(r)) => Some(r.clone()),
        (Some(j), Some(r)) => Some(merge_log_configs(j, r.clone())),
    }
}

/// Whether `dir` is the root of a git worktree — `.git` present, as either
/// a directory (main worktree) or a file (linked worktree).
fn is_repository_root(dir: &Path) -> bool {
    dir.join(".git").exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::HookType;
    use crate::hooks::yaml_config::{GroupDef, RunCommand};
    use std::collections::HashMap;

    fn make_ctx() -> HookContext {
        HookContext::new(
            HookType::PostCreate,
            "checkout",
            "/project",
            "/project/.git",
            "origin",
            "/project/main",
            "/project/feature/new",
            "feature/new",
        )
    }

    /// `{changed_files}` under a subdirectory `root:` must be refused, not
    /// silently handed paths that do not resolve from the job's cwd.
    #[test]
    fn changed_files_under_a_subdirectory_root_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        std::fs::create_dir_all(tmp.path().join("web")).unwrap();

        let jobs = vec![JobDef {
            name: Some("lint".into()),
            run: Some(RunCommand::Simple("eslint {changed_files}".into())),
            root: Some("web".into()),
            files: Some("echo web/src/a.ts".into()),
            ..Default::default()
        }];
        let ctx = make_ctx();
        let adapter = JobAdapterContext::default();
        let err = yaml_jobs_to_specs(&jobs, &ctx, &HashMap::new(), ".daft", tmp.path(), &adapter)
            .expect_err("a subdirectory root with {changed_files} must be refused");
        let msg = format!("{err:#}");
        assert!(msg.contains("repository-root-relative"), "{msg}");
        assert!(msg.contains("files:"), "{msg}");
    }

    /// The shipped idiom — a `root:` that is itself a worktree root — keeps
    /// working, because repo-relative paths resolve there.
    #[test]
    fn changed_files_under_a_worktree_root_is_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        let source_wt = tmp.path().join("source-wt");
        std::fs::create_dir_all(source_wt.join(".git")).unwrap();

        let jobs = vec![JobDef {
            name: Some("lint".into()),
            run: Some(RunCommand::Simple("eslint {changed_files}".into())),
            root: Some(source_wt.to_string_lossy().into_owned()),
            files: Some("echo src/a.ts".into()),
            ..Default::default()
        }];
        let ctx = make_ctx();
        let adapter = JobAdapterContext::default();
        let (specs, _) =
            yaml_jobs_to_specs(&jobs, &ctx, &HashMap::new(), ".daft", tmp.path(), &adapter)
                .expect("an absolute worktree root must be accepted");
        assert_eq!(specs.len(), 1);
        assert!(
            specs[0].command.contains("src/a.ts"),
            "{}",
            specs[0].command
        );
    }

    // ── yaml_jobs_to_specs ──────────────────────────────────────────────

    #[test]
    fn simple_job_maps_all_fields() {
        let ctx = make_ctx();
        let mut job_env = HashMap::new();
        job_env.insert("MY_VAR".into(), "hello".into());

        let jobs = vec![JobDef {
            name: Some("install".into()),
            run: Some(RunCommand::Simple("npm install".into())),
            description: Some("Install deps".into()),
            needs: Some(vec!["fetch".into()]),
            interactive: Some(true),
            fail_text: Some("install failed".into()),
            env: Some(job_env.clone()),
            root: None,
            ..Default::default()
        }];

        let hook_env = HashMap::new();
        let (specs, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &hook_env,
            ".daft",
            Path::new("/project"),
            &JobAdapterContext::default(),
        )
        .unwrap();

        assert_eq!(specs.len(), 1);
        let s = &specs[0];
        assert_eq!(s.name, "install");
        assert_eq!(s.command, "npm install");
        assert_eq!(s.description.as_deref(), Some("Install deps"));
        assert_eq!(s.needs, vec!["fetch"]);
        assert!(s.interactive);
        assert_eq!(s.fail_text.as_deref(), Some("install failed"));
        assert_eq!(s.env.get("MY_VAR").unwrap(), "hello");
        assert_eq!(s.working_dir, PathBuf::from("/project"));
        assert_eq!(s.timeout, Some(JobSpec::DEFAULT_TIMEOUT));
    }

    #[test]
    fn platform_skip_excludes_job() {
        let ctx = make_ctx();
        // Create a platform-specific run map with only an OS that is NOT the
        // current one. On macOS the current OS is "macos", on Linux it is
        // "linux". We build a map that contains only the *other* OS so the
        // job is always skipped regardless of which platform runs the test.
        let other_os = if cfg!(target_os = "macos") {
            crate::hooks::yaml_config::TargetOs::Linux
        } else if cfg!(target_os = "linux") {
            crate::hooks::yaml_config::TargetOs::Macos
        } else {
            // Windows or other — just use Linux as the non-matching OS.
            crate::hooks::yaml_config::TargetOs::Linux
        };

        let mut platform_map = HashMap::new();
        platform_map.insert(
            other_os,
            crate::hooks::yaml_config::PlatformRunCommand::Simple("echo other".into()),
        );

        let jobs = vec![JobDef {
            name: Some("os-specific".into()),
            run: Some(RunCommand::Platform(platform_map)),
            ..Default::default()
        }];

        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext::default(),
        )
        .unwrap();
        assert!(
            kept.is_empty(),
            "platform-mismatched job should be excluded"
        );
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].name, "os-specific");
        assert!(skipped[0].reason.contains("platform"));
    }

    #[test]
    fn rc_file_prepended_to_command() {
        let ctx = make_ctx();
        let jobs = vec![JobDef {
            name: Some("build".into()),
            run: Some(RunCommand::Simple("cargo build".into())),
            ..Default::default()
        }];

        let (specs, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/project"),
            &JobAdapterContext {
                rc: Some("~/.bashrc"),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].command, "source ~/.bashrc && cargo build");
    }

    #[test]
    fn working_dir_resolved_from_job_root() {
        let ctx = make_ctx();
        let jobs = vec![JobDef {
            name: Some("test".into()),
            run: Some(RunCommand::Simple("cargo test".into())),
            root: Some("packages/core".into()),
            ..Default::default()
        }];

        let (specs, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/project"),
            &JobAdapterContext::default(),
        )
        .unwrap();

        assert_eq!(specs.len(), 1);
        assert_eq!(
            specs[0].working_dir,
            PathBuf::from("/project/packages/core")
        );
    }

    #[test]
    fn env_merging_job_wins() {
        let ctx = make_ctx();
        let mut hook_env = HashMap::new();
        hook_env.insert("SHARED".into(), "from-hook".into());
        hook_env.insert("HOOK_ONLY".into(), "yes".into());

        let mut job_env = HashMap::new();
        job_env.insert("SHARED".into(), "from-job".into());
        job_env.insert("JOB_ONLY".into(), "yes".into());

        let jobs = vec![JobDef {
            name: Some("test".into()),
            run: Some(RunCommand::Simple("echo test".into())),
            env: Some(job_env),
            ..Default::default()
        }];

        let (specs, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &hook_env,
            ".daft",
            Path::new("/project"),
            &JobAdapterContext::default(),
        )
        .unwrap();

        assert_eq!(specs.len(), 1);
        let env = &specs[0].env;
        assert_eq!(env.get("SHARED").unwrap(), "from-job", "job env should win");
        assert_eq!(env.get("HOOK_ONLY").unwrap(), "yes");
        assert_eq!(env.get("JOB_ONLY").unwrap(), "yes");
    }

    #[test]
    fn job_env_values_are_template_substituted() {
        let ctx = make_ctx(); // branch feature/new, worktree /project/feature/new
        let hook_env = HashMap::new();

        let mut job_env = HashMap::new();
        job_env.insert("COMPOSE_PROJECT_NAME".into(), "api-{worktree_slug}".into());
        job_env.insert("ON_BRANCH".into(), "{branch}".into());
        job_env.insert("LITERAL".into(), "no-templates-here".into());

        let jobs = vec![JobDef {
            name: Some("backend".into()),
            run: Some(RunCommand::Simple("docker compose up".into())),
            env: Some(job_env),
            ..Default::default()
        }];

        let (specs, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &hook_env,
            ".daft",
            Path::new("/project"),
            &JobAdapterContext::default(),
        )
        .unwrap();

        let env = &specs[0].env;
        assert_eq!(env.get("COMPOSE_PROJECT_NAME").unwrap(), "api-feature-new");
        assert_eq!(env.get("ON_BRANCH").unwrap(), "feature/new");
        assert_eq!(env.get("LITERAL").unwrap(), "no-templates-here");
    }

    #[test]
    fn computed_hook_env_values_are_not_substituted() {
        // The computed hook env (DAFT_* + extra_env) is merged verbatim; a
        // brace-bearing value must not be reinterpreted as a template.
        let ctx = make_ctx();
        let mut hook_env = HashMap::new();
        hook_env.insert("SOME_COMPUTED".into(), "{branch}".into());

        let jobs = vec![JobDef {
            name: Some("j".into()),
            run: Some(RunCommand::Simple("true".into())),
            ..Default::default()
        }];
        let (specs, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &hook_env,
            ".daft",
            Path::new("/project"),
            &JobAdapterContext::default(),
        )
        .unwrap();

        assert_eq!(specs[0].env.get("SOME_COMPUTED").unwrap(), "{branch}");
    }

    #[test]
    fn needs_mapping() {
        let ctx = make_ctx();
        let jobs = vec![
            JobDef {
                name: Some("a".into()),
                run: Some(RunCommand::Simple("true".into())),
                ..Default::default()
            },
            JobDef {
                name: Some("b".into()),
                run: Some(RunCommand::Simple("true".into())),
                needs: Some(vec!["a".into()]),
                ..Default::default()
            },
        ];

        let (specs, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext::default(),
        )
        .unwrap();

        assert_eq!(specs.len(), 2);
        assert!(specs[0].needs.is_empty());
        assert_eq!(specs[1].needs, vec!["a"]);
    }

    #[test]
    fn unnamed_job_gets_default_name() {
        let ctx = make_ctx();
        let jobs = vec![JobDef {
            run: Some(RunCommand::Simple("echo hi".into())),
            ..Default::default()
        }];

        let (specs, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext::default(),
        )
        .unwrap();

        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "(unnamed)");
    }

    #[test]
    fn group_jobs_flatten_into_prefixed_specs() {
        // These used to be reported as "not yet supported by the generic
        // executor" and dropped. They now flatten into the DAG the executor
        // already speaks, so the group's name survives only as a prefix.
        let ctx = make_ctx();
        let jobs = vec![
            JobDef {
                name: Some("normal".into()),
                run: Some(RunCommand::Simple("echo ok".into())),
                ..Default::default()
            },
            JobDef {
                name: Some("grouped".into()),
                group: Some(GroupDef {
                    parallel: Some(true),
                    jobs: Some(vec![JobDef {
                        name: Some("inner".into()),
                        run: Some(RunCommand::Simple("echo inner".into())),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ];

        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext::default(),
        )
        .unwrap();

        assert!(skipped.is_empty(), "{skipped:?}");
        let names: Vec<&str> = kept.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["normal", "grouped/inner"]);
    }

    #[test]
    fn platform_skip_produces_skipped_job_entry() {
        use crate::hooks::yaml_config::{PlatformRunCommand, TargetOs};
        let mut run_map = HashMap::new();
        let other_os = if cfg!(target_os = "macos") {
            TargetOs::Linux
        } else {
            TargetOs::Macos
        };
        run_map.insert(other_os, PlatformRunCommand::Simple("echo other".into()));

        let jobs = vec![JobDef {
            name: Some("platform-only".to_string()),
            run: Some(RunCommand::Platform(run_map)),
            ..Default::default()
        }];

        let ctx = make_ctx();
        let env = HashMap::new();
        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &env,
            "/src",
            std::path::Path::new("/work"),
            &JobAdapterContext::default(),
        )
        .unwrap();

        assert!(kept.is_empty());
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].name, "platform-only");
        assert!(skipped[0].reason.contains("platform"));
    }

    #[test]
    fn an_empty_group_fails_the_hook_rather_than_vanishing() {
        // A group with no jobs is a config the author got wrong. Reporting it
        // as a skipped job would read as "daft decided not to run this",
        // which is a different and much more forgivable thing.
        let jobs = vec![JobDef {
            name: Some("my-group".to_string()),
            group: Some(GroupDef::default()),
            ..Default::default()
        }];

        let ctx = make_ctx();
        let env = HashMap::new();
        let err = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &env,
            "/src",
            std::path::Path::new("/work"),
            &JobAdapterContext::default(),
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("my-group"), "{msg}");
        assert!(msg.contains("no jobs"), "{msg}");
    }

    #[test]
    fn per_job_skip_true_produces_skipped_entry() {
        use crate::hooks::yaml_config::{JobDef, SkipCondition};
        let jobs = vec![JobDef {
            name: Some("gated".to_string()),
            run: Some(crate::hooks::yaml_config::RunCommand::Simple(
                "echo gated".to_string(),
            )),
            skip: Some(SkipCondition::Bool(true)),
            ..Default::default()
        }];

        let ctx = make_ctx();
        let env = HashMap::new();
        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &env,
            "/src",
            std::path::Path::new("/work"),
            &JobAdapterContext::default(),
        )
        .unwrap();

        assert!(kept.is_empty());
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].name, "gated");
        assert_eq!(skipped[0].reason, "skip: true");
    }

    #[test]
    fn per_job_skip_false_keeps_job() {
        use crate::hooks::yaml_config::{JobDef, SkipCondition};
        let jobs = vec![JobDef {
            name: Some("always-runs".to_string()),
            run: Some(crate::hooks::yaml_config::RunCommand::Simple(
                "echo go".to_string(),
            )),
            skip: Some(SkipCondition::Bool(false)),
            ..Default::default()
        }];

        let ctx = make_ctx();
        let env = HashMap::new();
        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &env,
            "/src",
            std::path::Path::new("/work"),
            &JobAdapterContext::default(),
        )
        .unwrap();

        assert_eq!(kept.len(), 1);
        assert!(skipped.is_empty());
    }

    #[test]
    fn kept_spec_needs_are_stripped_of_skipped_dependencies() {
        use crate::hooks::yaml_config::{JobDef, SkipCondition};
        let jobs = vec![
            JobDef {
                name: Some("skipped-dep".to_string()),
                run: Some(crate::hooks::yaml_config::RunCommand::Simple(
                    "echo nope".to_string(),
                )),
                skip: Some(SkipCondition::Bool(true)),
                ..Default::default()
            },
            JobDef {
                name: Some("dependent".to_string()),
                run: Some(crate::hooks::yaml_config::RunCommand::Simple(
                    "echo yes".to_string(),
                )),
                needs: Some(vec!["skipped-dep".to_string()]),
                ..Default::default()
            },
        ];

        let ctx = make_ctx();
        let env = HashMap::new();
        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &env,
            "/src",
            std::path::Path::new("/work"),
            &JobAdapterContext::default(),
        )
        .unwrap();

        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, "dependent");
        assert!(
            kept[0].needs.is_empty(),
            "needs: list should have been stripped of the skipped dependency, but got {:?}",
            kept[0].needs
        );
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].name, "skipped-dep");
    }

    #[test]
    fn per_job_only_false_produces_skipped_entry() {
        use crate::hooks::yaml_config::{JobDef, OnlyCondition};
        let jobs = vec![JobDef {
            name: Some("conditional".to_string()),
            run: Some(crate::hooks::yaml_config::RunCommand::Simple(
                "echo cond".to_string(),
            )),
            only: Some(OnlyCondition::Bool(false)),
            ..Default::default()
        }];

        let ctx = make_ctx();
        let env = HashMap::new();
        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &env,
            "/src",
            std::path::Path::new("/work"),
            &JobAdapterContext::default(),
        )
        .unwrap();

        assert!(kept.is_empty());
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].reason, "only: false");
    }

    // ── changed-file filtering (glob / exclude / files) ─────────────────

    use crate::hooks::yaml_config::StringOrList;

    fn provider(files: &[&str]) -> FileSources {
        FileSources::preresolved(files.iter().map(|s| s.to_string()).collect())
    }

    fn glob_job(name: &str, run: &str, glob: &[&str]) -> JobDef {
        JobDef {
            name: Some(name.to_string()),
            run: Some(RunCommand::Simple(run.to_string())),
            glob: Some(StringOrList::List(
                glob.iter().map(|s| s.to_string()).collect(),
            )),
            ..Default::default()
        }
    }

    /// Build specs for one job against the given sources, in a scratch dir.
    fn specs_with(job: JobDef, sources: &FileSources) -> Result<(Vec<JobSpec>, Vec<SkippedJob>)> {
        yaml_jobs_to_specs(
            std::slice::from_ref(&job),
            &make_ctx(),
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext {
                file_sources: Some(sources),
                ..Default::default()
            },
        )
    }

    fn run_job(name: &str, run: &str) -> JobDef {
        JobDef {
            name: Some(name.to_string()),
            run: Some(RunCommand::Simple(run.to_string())),
            ..Default::default()
        }
    }

    #[test]
    fn files_and_changed_files_expand_to_the_same_list() {
        let sources = provider(&["a.rs", "b c.rs"]);
        for template in ["{files}", "{changed_files}"] {
            let (kept, _) =
                specs_with(run_job("lint", &format!("fmt {template}")), &sources).unwrap();
            assert_eq!(kept[0].command, "fmt a.rs 'b c.rs'", "{template}");
        }
    }

    #[test]
    fn quoting_variants_render_per_path() {
        let sources = provider(&["a.rs", "b.rs"]);
        let (kept, _) = specs_with(run_job("j", "grep TODO \"{files}\""), &sources).unwrap();
        assert_eq!(kept[0].command, "grep TODO \"a.rs\" \"b.rs\"");
    }

    #[test]
    fn a_job_files_command_overrides_every_named_source() {
        // The job said where its list comes from; a placeholder naming a
        // source cannot mean "ignore that".
        let sources = provider(&["from-hook.rs"]);
        let job = JobDef {
            files: Some("printf 'from-job.rs\\n'".into()),
            ..run_job("j", "fmt {staged_files}")
        };
        let (kept, _) = specs_with(job, &sources).unwrap();
        assert_eq!(kept[0].command, "fmt from-job.rs");
    }

    #[test]
    fn naming_a_source_the_hook_cannot_supply_is_a_loud_error() {
        // Not an empty list: a gate that silently checks nothing passes green
        // on exactly the change it was written to catch.
        let sources = provider(&["a.rs"]);
        let err = specs_with(run_job("j", "fmt {staged_files}"), &sources).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("{staged_files}"), "{msg}");
        assert!(msg.contains("cannot supply"), "{msg}");
    }

    #[test]
    fn an_oversized_expansion_becomes_follow_on_chunks() {
        let files: Vec<String> = (0..20_000).map(|i| format!("src/f{i:06}.rs")).collect();
        let sources = FileSources::preresolved(files);
        let (kept, _) = specs_with(run_job("fmt", "rustfmt {files}"), &sources).unwrap();

        assert_eq!(kept.len(), 1, "chunking must not split the job row");
        assert!(!kept[0].extra_chunks.is_empty());
        for command in std::iter::once(&kept[0].command).chain(kept[0].extra_chunks.iter()) {
            assert!(command.len() <= crate::hooks::changed_files::MAX_EXPANDED_COMMAND_BYTES);
        }
    }

    #[test]
    fn a_chunked_job_keeps_its_rc_prefix_on_every_chunk() {
        // A later chunk that skipped `source <rc>` would run in a different
        // shell environment than the first — the same command, different PATH.
        let files: Vec<String> = (0..20_000).map(|i| format!("src/f{i:06}.rs")).collect();
        let sources = FileSources::preresolved(files);
        let (kept, _) = yaml_jobs_to_specs(
            &[run_job("fmt", "rustfmt {files}")],
            &make_ctx(),
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext {
                file_sources: Some(&sources),
                rc: Some("~/.bashrc"),
                ..Default::default()
            },
        )
        .unwrap();

        assert!(!kept[0].extra_chunks.is_empty());
        for command in std::iter::once(&kept[0].command).chain(kept[0].extra_chunks.iter()) {
            assert!(
                command.starts_with("source ~/.bashrc && rustfmt "),
                "{command}"
            );
        }
    }

    #[test]
    fn templates_expand_in_run_commands() {
        let templates: HashMap<String, String> = [
            ("lint".to_string(), "eslint --max-warnings 0".to_string()),
            (
                "lint_strict".to_string(),
                "eslint --max-warnings 0 --strict".to_string(),
            ),
        ]
        .into_iter()
        .collect();

        let (kept, _) = yaml_jobs_to_specs(
            &[
                run_job("a", "{lint} src"),
                run_job("b", "{lint_strict} src"),
            ],
            &make_ctx(),
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext {
                templates: Some(&templates),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(kept[0].command, "eslint --max-warnings 0 src");
        // Longest name first, so `{lint}` cannot eat the prefix of
        // `{lint_strict}` and leave `_strict` dangling.
        assert_eq!(kept[1].command, "eslint --max-warnings 0 --strict src");
    }

    #[test]
    fn stage_fixed_appends_a_git_add_chunk() {
        // A formatter's edits have to reach the index, or the commit lands
        // unformatted and the user sees a surprise diff afterwards.
        let sources = provider(&["a.rs", "b.rs"]);
        let job = JobDef {
            stage_fixed: Some(true),
            ..run_job("fmt", "rustfmt {files}")
        };
        let (kept, _) = specs_with(job, &sources).unwrap();
        assert_eq!(kept[0].command, "rustfmt a.rs b.rs");
        assert_eq!(kept[0].extra_chunks, vec!["git add -- a.rs b.rs"]);
    }

    #[test]
    fn use_stdin_replays_the_stage_payload() {
        let sources = provider(&["a.rs"]);
        let job = JobDef {
            use_stdin: Some(true),
            ..run_job("check", "read-refs")
        };
        let (kept, _) = yaml_jobs_to_specs(
            &[job],
            &make_ctx(),
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext {
                file_sources: Some(&sources),
                stage_stdin: Some("refs/heads/f aaa refs/heads/f bbb"),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            kept[0].stdin.as_deref(),
            Some("refs/heads/f aaa refs/heads/f bbb")
        );
    }

    #[test]
    fn use_stdin_conflicts_with_the_things_that_own_stdin() {
        for (field, marker) in [("interactive", "interactive"), ("background", "background")] {
            let mut job = run_job("j", "cat");
            job.use_stdin = Some(true);
            if field == "interactive" {
                job.interactive = Some(true);
            } else {
                job.background = Some(true);
            }
            let err = specs_with(job, &provider(&["a.rs"])).unwrap_err();
            let msg = format!("{err:#}");
            assert!(msg.contains("use_stdin"), "{msg}");
            assert!(msg.contains(marker), "{msg}");
        }
    }

    #[test]
    fn glob_job_skipped_when_no_changed_file_matches() {
        let ctx = make_ctx();
        let p = provider(&["docs/index.md", "README.md"]);
        let jobs = vec![glob_job("build", "cargo check", &["src/**", "Cargo.*"])];

        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext {
                file_sources: Some(&p),
                ..Default::default()
            },
        )
        .unwrap();

        assert!(kept.is_empty());
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].name, "build");
        assert_eq!(
            skipped[0].reason,
            "skip: no changed files match src/**, Cargo.*"
        );
    }

    #[test]
    fn glob_job_runs_when_a_changed_file_matches() {
        let ctx = make_ctx();
        let p = provider(&["src/lib.rs", "docs/index.md"]);
        let jobs = vec![glob_job("build", "cargo check", &["src/**"])];

        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext {
                file_sources: Some(&p),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(kept.len(), 1);
        assert!(skipped.is_empty());
        assert_eq!(kept[0].command, "cargo check");
    }

    #[test]
    fn changed_files_template_expands_to_quoted_filtered_list() {
        let ctx = make_ctx();
        let p = provider(&["src/a.rs", "src/with space.rs", "docs/x.md"]);
        let jobs = vec![glob_job("lint", "lint {changed_files}", &["src/**"])];

        let (kept, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext {
                file_sources: Some(&p),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].command, "lint src/a.rs 'src/with space.rs'");
    }

    #[test]
    fn bare_changed_files_template_with_empty_change_set_skips() {
        let ctx = make_ctx();
        let p = provider(&[]);
        let jobs = vec![JobDef {
            name: Some("fmt".into()),
            run: Some(RunCommand::Simple("fmt {changed_files}".into())),
            ..Default::default()
        }];

        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext {
                file_sources: Some(&p),
                ..Default::default()
            },
        )
        .unwrap();

        assert!(kept.is_empty());
        assert_eq!(skipped[0].reason, "skip: no changed files");
    }

    #[test]
    fn files_command_overrides_the_hook_source() {
        let ctx = make_ctx();
        let dir = tempfile::tempdir().unwrap();
        // The hook source would say docs-only; the job's own files: command
        // wins and produces a matching file.
        let p = provider(&["docs/index.md"]);
        let jobs = vec![JobDef {
            name: Some("own-list".into()),
            run: Some(RunCommand::Simple("check {changed_files}".into())),
            files: Some("printf 'src/gen.rs\\n'".into()),
            glob: Some(StringOrList::Single("src/**".into())),
            ..Default::default()
        }];

        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            dir.path(),
            &JobAdapterContext {
                file_sources: Some(&p),
                ..Default::default()
            },
        )
        .unwrap();

        assert!(skipped.is_empty());
        assert_eq!(kept[0].command, "check src/gen.rs");
    }

    #[test]
    fn empty_files_command_output_skips_with_its_own_reason() {
        let ctx = make_ctx();
        let dir = tempfile::tempdir().unwrap();
        let jobs = vec![JobDef {
            name: Some("nothing".into()),
            run: Some(RunCommand::Simple("true".into())),
            files: Some("true".into()),
            ..Default::default()
        }];

        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            dir.path(),
            &JobAdapterContext::default(),
        )
        .unwrap();

        assert!(kept.is_empty());
        assert_eq!(skipped[0].reason, "skip: files: command returned no files");
    }

    #[test]
    fn failing_files_command_is_an_error_not_a_skip() {
        let ctx = make_ctx();
        let dir = tempfile::tempdir().unwrap();
        let jobs = vec![JobDef {
            name: Some("broken".into()),
            run: Some(RunCommand::Simple("true".into())),
            files: Some("exit 7".into()),
            ..Default::default()
        }];

        let err = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            dir.path(),
            &JobAdapterContext::default(),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("broken"), "{err:#}");
    }

    #[test]
    fn glob_without_any_source_is_a_loud_config_error() {
        // A lifecycle hook (no provider) whose job declares glob: and brings
        // no files: command must fail the fire, never silently run or skip.
        let ctx = make_ctx();
        let jobs = vec![glob_job("gated", "echo hi", &["src/**"])];

        let err = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext::default(),
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("gated"), "{msg}");
        assert!(msg.contains("worktree-post-create"), "{msg}");
        assert!(msg.contains("files:"), "{msg}");
    }

    #[test]
    fn invalid_glob_pattern_is_an_error_naming_the_job() {
        let ctx = make_ctx();
        let p = provider(&["src/lib.rs"]);
        let jobs = vec![glob_job("bad", "true", &["src/[oops"])];

        let err = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext {
                file_sources: Some(&p),
                ..Default::default()
            },
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("bad"), "{msg}");
        assert!(msg.contains("src/[oops"), "{msg}");
    }

    #[test]
    fn exclude_only_job_skips_on_docs_only_change_set() {
        // The docs-only pattern: exclude alone selects "anything else", so a
        // change set fully inside the excludes skips the job.
        let ctx = make_ctx();
        let p = provider(&["docs/guide.md", "README.md"]);
        let jobs = vec![JobDef {
            name: Some("build".into()),
            run: Some(RunCommand::Simple("cargo check".into())),
            exclude: Some(vec!["docs/**".into(), "*.md".into()]),
            ..Default::default()
        }];

        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext {
                file_sources: Some(&p),
                ..Default::default()
            },
        )
        .unwrap();

        assert!(kept.is_empty());
        assert_eq!(
            skipped[0].reason,
            "skip: all changed files excluded by docs/**, *.md"
        );
    }

    #[test]
    fn hook_level_exclude_appends_to_file_aware_jobs_only() {
        let ctx = make_ctx();
        let p = provider(&["docs/guide.md"]);
        let hook_exclude = vec!["docs/**".to_string()];
        let jobs = vec![
            // File-aware: glob matches the doc file, but the hook-level
            // exclude removes it → skip.
            glob_job("aware", "true", &["**/*.md"]),
            // Not file-aware: hook-level exclude alone must not skip it.
            JobDef {
                name: Some("plain".into()),
                run: Some(RunCommand::Simple("true".into())),
                ..Default::default()
            },
        ];

        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext {
                file_sources: Some(&p),
                hook_exclude: &hook_exclude,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, "plain");
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].name, "aware");
        assert_eq!(
            skipped[0].reason,
            "skip: no changed files match **/*.md (excluding docs/**)"
        );
    }

    #[test]
    fn glob_skip_feeds_needs_stripping_like_other_skips() {
        let ctx = make_ctx();
        let p = provider(&["docs/x.md"]);
        let jobs = vec![
            glob_job("build", "true", &["src/**"]),
            JobDef {
                name: Some("test".into()),
                run: Some(RunCommand::Simple("true".into())),
                needs: Some(vec!["build".into()]),
                ..Default::default()
            },
        ];

        let (kept, skipped) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext {
                file_sources: Some(&p),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(skipped[0].name, "build");
        assert_eq!(kept.len(), 1);
        assert!(
            kept[0].needs.is_empty(),
            "needs on a glob-skipped dep must be stripped"
        );
    }

    // ── root: templating (fail-closed merge paths) ──────────────────────

    fn merge_hook_ctx(source_path: &str) -> HookContext {
        let extra: std::collections::BTreeMap<String, String> = [(
            "DAFT_MERGE_SOURCE_PATH".to_string(),
            source_path.to_string(),
        )]
        .into();
        HookContext::new(
            HookType::PreMerge,
            "merge",
            "/project",
            "/project/.git",
            "origin",
            "/project/main",
            "/project/main",
            "main",
        )
        .with_extra_env(extra)
    }

    #[test]
    fn root_merge_source_path_template_resolves_to_absolute_root() {
        let ctx = merge_hook_ctx("/project/feat");
        let jobs = vec![JobDef {
            name: Some("ring".into()),
            run: Some(RunCommand::Simple("cargo check".into())),
            root: Some("{merge_source_path}".into()),
            ..Default::default()
        }];

        let (kept, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/project/main"),
            &JobAdapterContext::default(),
        )
        .unwrap();

        // Absolute substituted root replaces the base (PathBuf::join).
        assert_eq!(kept[0].working_dir, PathBuf::from("/project/feat"));
    }

    #[test]
    fn root_merge_template_unresolvable_fails_closed() {
        // Empty DAFT_MERGE_SOURCE_PATH (no worktree / multi-source): the
        // placeholder survives substitution and the hook must abort, never
        // silently run the job in the hook's own cwd.
        let ctx = merge_hook_ctx("");
        let jobs = vec![JobDef {
            name: Some("ring".into()),
            run: Some(RunCommand::Simple("cargo check".into())),
            root: Some("{merge_source_path}".into()),
            ..Default::default()
        }];

        let err = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/project/main"),
            &JobAdapterContext::default(),
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("ring"), "{msg}");
        assert!(msg.contains("undefined directory"), "{msg}");
    }

    #[test]
    fn root_plain_relative_path_still_joins() {
        let ctx = merge_hook_ctx("/project/feat");
        let jobs = vec![JobDef {
            name: Some("sub".into()),
            run: Some(RunCommand::Simple("true".into())),
            root: Some("packages/core".into()),
            ..Default::default()
        }];
        let (kept, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/project/main"),
            &JobAdapterContext::default(),
        )
        .unwrap();
        assert_eq!(
            kept[0].working_dir,
            PathBuf::from("/project/main/packages/core")
        );
    }

    // ── scripts_to_specs ────────────────────────────────────────────────

    #[test]
    fn test_background_fields_pass_through() {
        use crate::hooks::yaml_config::{BackgroundOutput, LogConfig};

        let jobs = vec![JobDef {
            name: Some("bg-job".to_string()),
            run: Some(RunCommand::Simple("echo hi".to_string())),
            background: Some(true),
            background_output: Some(BackgroundOutput::Silent),
            log: Some(LogConfig {
                retention: Some("14d".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        }];

        let ctx = make_ctx();
        let (specs, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext::default(),
        )
        .unwrap();
        assert_eq!(specs.len(), 1);
        assert!(specs[0].background);
        assert_eq!(specs[0].background_output, Some(BackgroundOutput::Silent));
        assert_eq!(
            specs[0].log_config.as_ref().unwrap().retention,
            Some("14d".to_string())
        );
    }

    #[test]
    fn scripts_name_from_filename() {
        let ctx = make_ctx();
        let env = HookEnvironment::from_context(&ctx);
        let paths = vec![
            PathBuf::from("/project/.daft/hooks/worktree-post-create"),
            PathBuf::from("/home/user/.config/daft/hooks/post-clone"),
        ];

        let specs = scripts_to_specs(&paths, &env, Path::new("/project/feature/new"));

        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "worktree-post-create");
        assert_eq!(specs[1].name, "post-clone");
    }

    #[test]
    fn scripts_command_is_full_path() {
        let ctx = make_ctx();
        let env = HookEnvironment::from_context(&ctx);
        let paths = vec![PathBuf::from("/project/.daft/hooks/post-clone")];

        let specs = scripts_to_specs(&paths, &env, Path::new("/project"));

        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].command, "/project/.daft/hooks/post-clone");
    }

    #[test]
    fn scripts_working_dir_passed_through() {
        let ctx = make_ctx();
        let env = HookEnvironment::from_context(&ctx);
        let paths = vec![PathBuf::from("/some/hook")];
        let wd = Path::new("/custom/dir");

        let specs = scripts_to_specs(&paths, &env, wd);

        assert_eq!(specs[0].working_dir, PathBuf::from("/custom/dir"));
    }

    #[test]
    fn scripts_env_from_hook_environment() {
        let ctx = make_ctx();
        let env = HookEnvironment::from_context(&ctx);
        let paths = vec![PathBuf::from("/some/hook")];

        let specs = scripts_to_specs(&paths, &env, Path::new("/tmp"));

        assert_eq!(
            specs[0].env.get("DAFT_HOOK").map(String::as_str),
            Some("worktree-post-create")
        );
        assert_eq!(
            specs[0].env.get("DAFT_BRANCH_NAME").map(String::as_str),
            Some("feature/new")
        );
    }

    #[test]
    fn scripts_defaults_for_optional_fields() {
        let ctx = make_ctx();
        let env = HookEnvironment::from_context(&ctx);
        let paths = vec![PathBuf::from("/some/hook")];

        let specs = scripts_to_specs(&paths, &env, Path::new("/tmp"));

        let s = &specs[0];
        assert!(!s.interactive);
        assert!(s.needs.is_empty());
        assert!(s.fail_text.is_none());
        assert!(s.description.is_none());
        assert_eq!(s.timeout, Some(JobSpec::DEFAULT_TIMEOUT));
    }

    // ── repo-level log merge ─────────────────────────────────────────────
    //
    // Top-level `log:` defaults in `daft.yml` (max_total_size, keep_last,
    // stale_running_after, plus retention/max_log_size) must propagate into
    // each job's `log_config`, with per-job blocks taking precedence.

    #[test]
    fn merges_repo_log_into_jobs_without_per_job_log() {
        use crate::executor::LogConfig;

        let jobs = vec![JobDef {
            name: Some("build".to_string()),
            run: Some(RunCommand::Simple("cargo build".to_string())),
            log: None,
            ..Default::default()
        }];

        let repo_log = LogConfig {
            max_total_size: Some("1GB".to_string()),
            keep_last: Some(5),
            stale_running_after: Some("48h".to_string()),
            ..Default::default()
        };

        let ctx = make_ctx();
        let (kept, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext {
                repo_log: Some(&repo_log),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(kept.len(), 1);
        let lc = kept[0]
            .log_config
            .as_ref()
            .expect("log_config should be Some when repo-level log is provided");
        assert_eq!(lc.max_total_size.as_deref(), Some("1GB"));
        assert_eq!(lc.keep_last, Some(5));
        assert_eq!(lc.stale_running_after.as_deref(), Some("48h"));
    }

    #[test]
    fn per_job_log_overrides_repo_log() {
        use crate::executor::LogConfig;

        let jobs = vec![JobDef {
            name: Some("build".to_string()),
            run: Some(RunCommand::Simple("cargo build".to_string())),
            log: Some(LogConfig {
                retention: Some("1d".to_string()),
                max_log_size: Some("1MB".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        }];

        let repo_log = LogConfig {
            retention: Some("30d".to_string()),
            max_total_size: Some("1GB".to_string()),
            keep_last: Some(3),
            ..Default::default()
        };

        let ctx = make_ctx();
        let (kept, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext {
                repo_log: Some(&repo_log),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(kept.len(), 1);
        let lc = kept[0].log_config.as_ref().unwrap();
        // Per-job overrides win
        assert_eq!(
            lc.retention.as_deref(),
            Some("1d"),
            "per-job retention should override repo-level"
        );
        assert_eq!(lc.max_log_size.as_deref(), Some("1MB"));
        // Repo-level fills in unset per-job fields
        assert_eq!(
            lc.max_total_size.as_deref(),
            Some("1GB"),
            "repo-level max_total_size should fill in"
        );
        assert_eq!(lc.keep_last, Some(3), "repo-level keep_last should fill in");
    }

    #[test]
    fn no_repo_log_keeps_per_job_log_unchanged() {
        use crate::executor::LogConfig;

        let jobs = vec![JobDef {
            name: Some("build".to_string()),
            run: Some(RunCommand::Simple("cargo build".to_string())),
            log: Some(LogConfig {
                retention: Some("7d".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        }];

        let ctx = make_ctx();
        let (kept, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext::default(),
        )
        .unwrap();

        assert_eq!(kept.len(), 1);
        let lc = kept[0].log_config.as_ref().unwrap();
        assert_eq!(lc.retention.as_deref(), Some("7d"));
        assert!(lc.max_total_size.is_none());
    }

    #[test]
    fn no_log_at_either_level_yields_none() {
        let jobs = vec![JobDef {
            name: Some("build".to_string()),
            run: Some(RunCommand::Simple("cargo build".to_string())),
            log: None,
            ..Default::default()
        }];

        let ctx = make_ctx();
        let (kept, _) = yaml_jobs_to_specs(
            &jobs,
            &ctx,
            &HashMap::new(),
            ".daft",
            Path::new("/tmp"),
            &JobAdapterContext::default(),
        )
        .unwrap();

        assert_eq!(kept.len(), 1);
        assert!(kept[0].log_config.is_none());
    }

    // ── parse_skip_selectors ────────────────────────────────────────────

    fn strs(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_bare_name_goes_to_names() {
        let s = parse_skip_selectors(&strs(&["lint"]));
        assert!(!s.all);
        assert_eq!(s.names, vec!["lint"]);
        assert!(s.tags.is_empty());
    }

    #[test]
    fn parse_job_prefix_is_escape_hatch() {
        // `job:all` is a literal job name, never the wildcard.
        let s = parse_skip_selectors(&strs(&["job:all"]));
        assert!(!s.all);
        assert_eq!(s.names, vec!["all"]);
    }

    #[test]
    fn parse_wildcard_star_and_all() {
        assert!(parse_skip_selectors(&strs(&["all"])).all);
        assert!(parse_skip_selectors(&strs(&["*"])).all);
    }

    #[test]
    fn parse_tag_prefix() {
        let s = parse_skip_selectors(&strs(&["tag:heavy"]));
        assert_eq!(s.tags, vec!["heavy"]);
        assert!(s.names.is_empty());
    }

    #[test]
    fn parse_mixed_selectors() {
        let s = parse_skip_selectors(&strs(&["tag:heavy", "lint", "job:build"]));
        assert!(!s.all);
        assert_eq!(s.tags, vec!["heavy"]);
        assert_eq!(s.names, vec!["lint", "build"]);
        assert!(s.hook_types.is_empty());
        assert_eq!(s.raw.len(), 3);
    }

    #[test]
    fn parse_hook_type_name_is_reserved() {
        let s = parse_skip_selectors(&strs(&["worktree-post-create"]));
        assert!(!s.all);
        assert_eq!(s.hook_types, vec![crate::hooks::HookType::PostCreate]);
        assert!(s.names.is_empty());
        assert!(s.tags.is_empty());
        // post-clone and the other canonical names are reserved too.
        assert_eq!(
            parse_skip_selectors(&strs(&["post-clone"])).hook_types,
            vec![crate::hooks::HookType::PostClone]
        );
    }

    #[test]
    fn parse_job_prefix_escapes_hook_type_name() {
        // `job:worktree-post-create` is a job literally named after the hook
        // type, NOT the hook-type selector.
        let s = parse_skip_selectors(&strs(&["job:worktree-post-create"]));
        assert!(s.hook_types.is_empty());
        assert_eq!(s.names, vec!["worktree-post-create"]);
    }

    #[test]
    fn parse_deprecated_hook_short_form_is_a_job_name() {
        // Deprecated short forms (`post-create`) are NOT reserved — only the
        // canonical yaml_name spellings are. A bare `post-create` is a job name.
        let s = parse_skip_selectors(&strs(&["post-create"]));
        assert!(s.hook_types.is_empty());
        assert_eq!(s.names, vec!["post-create"]);
    }

    // ── compute_skip_cascade ────────────────────────────────────────────

    /// The issue's worked-example graph:
    ///   install            (no needs)
    ///   build  needs install, tags [heavy]
    ///   test   needs build
    ///   lint   needs install
    fn worked_example_jobs() -> Vec<JobDef> {
        vec![
            JobDef {
                name: Some("install".into()),
                ..Default::default()
            },
            JobDef {
                name: Some("build".into()),
                needs: Some(vec!["install".into()]),
                tags: Some(vec!["heavy".into()]),
                ..Default::default()
            },
            JobDef {
                name: Some("test".into()),
                needs: Some(vec!["build".into()]),
                ..Default::default()
            },
            JobDef {
                name: Some("lint".into()),
                needs: Some(vec!["install".into()]),
                ..Default::default()
            },
        ]
    }

    #[test]
    fn cascade_name_seed_pulls_transitive_dependents() {
        let jobs = worked_example_jobs();
        let skip = parse_skip_selectors(&strs(&["install"]));
        let c = compute_skip_cascade(&jobs, &skip);

        assert_eq!(c.excluded.get("install"), Some(&SkipCause::Requested));
        assert_eq!(
            c.excluded.get("build"),
            Some(&SkipCause::DependsOn("install".into()))
        );
        assert_eq!(
            c.excluded.get("test"),
            Some(&SkipCause::DependsOn("build".into()))
        );
        assert_eq!(
            c.excluded.get("lint"),
            Some(&SkipCause::DependsOn("install".into()))
        );
        assert_eq!(c.excluded.len(), 4);
        assert!(c.unmatched.is_empty());
    }

    #[test]
    fn cascade_tag_seed_drops_only_subtree() {
        let jobs = worked_example_jobs();
        let skip = parse_skip_selectors(&strs(&["tag:heavy"]));
        let c = compute_skip_cascade(&jobs, &skip);

        // build matches tag:heavy directly; test depends on build.
        assert_eq!(c.excluded.get("build"), Some(&SkipCause::Requested));
        assert_eq!(
            c.excluded.get("test"),
            Some(&SkipCause::DependsOn("build".into()))
        );
        // install and lint are untouched (upstream / sibling).
        assert!(!c.excluded.contains_key("install"));
        assert!(!c.excluded.contains_key("lint"));
        assert_eq!(c.excluded.len(), 2);
    }

    #[test]
    fn cascade_immediate_parent_attribution() {
        // test reports its immediate excluded dep (build), not the root (install).
        let jobs = worked_example_jobs();
        let skip = parse_skip_selectors(&strs(&["install"]));
        let c = compute_skip_cascade(&jobs, &skip);
        assert_eq!(
            c.excluded.get("test"),
            Some(&SkipCause::DependsOn("build".into()))
        );
    }

    #[test]
    fn cascade_diamond_is_deterministic() {
        // `app` needs two directly-excluded jobs; declaration-order-first wins.
        let jobs = vec![
            JobDef {
                name: Some("a".into()),
                ..Default::default()
            },
            JobDef {
                name: Some("b".into()),
                ..Default::default()
            },
            JobDef {
                name: Some("app".into()),
                needs: Some(vec!["b".into(), "a".into()]),
                ..Default::default()
            },
        ];
        let skip = parse_skip_selectors(&strs(&["a", "b"]));
        let c = compute_skip_cascade(&jobs, &skip);
        // Seeds enqueued in declaration order: a, then b. `a` is processed
        // first and claims `app`.
        assert_eq!(
            c.excluded.get("app"),
            Some(&SkipCause::DependsOn("a".into()))
        );
    }

    #[test]
    fn cascade_terminates_on_cyclic_needs() {
        // A needs B, B needs A. Skipping `a` cascades to `b` (b needs a); when
        // the BFS then rediscovers `a` via b's edge, the `excluded.contains_key`
        // guard refuses to re-enqueue it, so the walk terminates. (A `needs:`
        // cycle is later rejected by the DAG builder; the cascade runs first and
        // must not hang on it.) The test completing — not hanging — is the
        // cycle-safety proof the doc comment claims.
        let jobs = vec![
            JobDef {
                name: Some("a".into()),
                needs: Some(vec!["b".into()]),
                ..Default::default()
            },
            JobDef {
                name: Some("b".into()),
                needs: Some(vec!["a".into()]),
                ..Default::default()
            },
        ];
        let skip = parse_skip_selectors(&strs(&["a"]));
        let c = compute_skip_cascade(&jobs, &skip);
        assert_eq!(c.excluded.get("a"), Some(&SkipCause::Requested));
        assert_eq!(c.excluded.get("b"), Some(&SkipCause::DependsOn("a".into())));
        assert_eq!(c.excluded.len(), 2);
    }

    #[test]
    fn cascade_unmatched_selectors_collected() {
        let jobs = worked_example_jobs();
        let skip = parse_skip_selectors(&strs(&["ghost", "tag:nope"]));
        let c = compute_skip_cascade(&jobs, &skip);
        assert!(c.excluded.is_empty());
        assert!(c.unmatched.contains(&"ghost".to_string()));
        assert!(c.unmatched.contains(&"tag:nope".to_string()));
    }

    #[test]
    fn cascade_upstream_is_untouched() {
        // Skipping a leaf (test) must not remove its upstream deps.
        let jobs = worked_example_jobs();
        let skip = parse_skip_selectors(&strs(&["test"]));
        let c = compute_skip_cascade(&jobs, &skip);
        assert_eq!(c.excluded.get("test"), Some(&SkipCause::Requested));
        assert_eq!(c.excluded.len(), 1);
        assert!(!c.excluded.contains_key("install"));
        assert!(!c.excluded.contains_key("build"));
        assert!(!c.excluded.contains_key("lint"));
    }

    #[test]
    fn cause_reason_strings() {
        assert_eq!(
            SkipCause::Requested.reason(SKIP_HOOKS_FLAG),
            "requested (--skip-hooks)"
        );
        assert_eq!(
            SkipCause::DependsOn("build".into()).reason(SKIP_HOOKS_FLAG),
            "depends on build (skipped)"
        );
    }

    #[test]
    fn a_skip_reports_the_knob_that_actually_asked_for_it() {
        // Running an incumbent config, its own exclusion variable populates
        // the same selectors daft's flag does. Attributing the skip to
        // `--skip-hooks` would send the user looking for a flag nobody passed.
        let selectors = SkipSelectors {
            names: vec!["fmt".into()],
            raw: vec!["fmt".into()],
            origin: Some("LEFTHOOK_EXCLUDE"),
            ..Default::default()
        };
        assert_eq!(
            SkipCause::Requested.reason(selectors.origin_label()),
            "requested (LEFTHOOK_EXCLUDE)"
        );

        let from_the_flag = SkipSelectors {
            names: vec!["fmt".into()],
            ..Default::default()
        };
        assert_eq!(from_the_flag.origin_label(), SKIP_HOOKS_FLAG);
    }
}
