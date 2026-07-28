//! Copy-on-write replication of declared build caches into worktrees (#387).
//!
//! The independent-copy sibling of [`crate::core::shared`]: where `shared:`
//! centralizes one file in `.git/.daft/shared/` and symlinks it into every
//! worktree, `copy:` gives every worktree its own private replica of the
//! paths it declares — `target/`, `node_modules/`, `.venv/` — so a fresh
//! worktree starts warm instead of paying a full `cargo build` / `npm
//! install`. On a reflinking filesystem (APFS, btrfs, XFS `reflink=1`,
//! OpenZFS 2.2+, ReFS) that replica costs almost nothing until it diverges.
//!
//! This completes the creation-flow symmetry: carry (uncommitted changes) →
//! visitor propagation (untracked config) → `shared:` (linked config) →
//! **`copy:` (independent CoW cache copies)**. The stage runs between the
//! pre- and post-create hooks — concretely after `shared:` linking, before
//! `worktree-post-create` — so hook-driven builds hit a warm cache.
//!
//! ## Contract
//!
//! **Warn, never abort.** Deliberately the opposite of post-create hooks'
//! abort-by-default (#765). A cache copy is an optimization: a tracked entry,
//! an unreadable source, a full disk — none of them may cost the user the
//! worktree they asked for. [`copy_entries`] therefore has no `Result` on its
//! per-entry path; every failure becomes an outcome in [`CopyPathsResult`]
//! and renders as a yellow attention row.
//!
//! **Entries must be gitignored.** `git check-ignore` must pass for the entry
//! AND nothing under it may be tracked (`git ls-files <entry>` empty).
//! Copying tracked content would duplicate the working tree git is already
//! managing. The second probe is **defense in depth, not a division of
//! labour**: `check-ignore` consults the index and so already refuses a
//! force-added file and a directory with tracked content anywhere beneath it,
//! but only as a behaviour of git's exclude machinery — precisely what
//! `--no-index` turns off. Asking `ls-files` directly states the invariant
//! independently of how `check-ignore` chooses to answer.
//!
//! Both probes are **inlined here, batched per entry** ([`classify_matches`]),
//! not called through [`crate::core::git_ignore`]: that module answers about
//! one path at a time, and one path at a time is three git processes per
//! expanded match on the critical path of every worktree creation. Its
//! per-path functions remain the readable reference implementation, and the
//! copy stage's own perf test cross-checks the batched form against them.
//!
//! **One row per config ENTRY, not per expanded match.** A glob entry that
//! matches thirty directories is still one plan row; the fan-out lands in the
//! row's annotation. This keeps the plan face walk-free (no filesystem
//! traversal before the plan commits) and the reconcile keys stable.
//!
//! ## Shape
//!
//! [`read_copy_config`] resolves the section, [`expand_entries`] turns one
//! declaration into concrete paths, [`copy_entries`] does the work, and
//! [`push_copy_section`] / [`report_copy_results`] are the plan and receipt
//! halves of the rail section. Only that last pair renders anything: per-entry
//! facts live in the returned [`CopyPathsResult`] and are drawn exactly once,
//! so the same engine can be driven from the creation rail and from `daft
//! warm`'s plain line output without either duplicating the other.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use ignore::overrides::{Override, OverrideBuilder};

use crate::hooks::yaml_config::CopyFallback;

// ── Resolved configuration ────────────────────────────────────────────────

/// A `copy:` section normalized for the engine.
///
/// Both YAML spellings ([`crate::hooks::yaml_config::CopyConfig::Paths`] and
/// `::Full`) collapse into this one shape, so nothing downstream of
/// [`read_copy_config`] matches on the config enum.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResolvedCopyConfig {
    /// Declared entries in config order, with the surrounding slashes stripped
    /// (`target/`, `/target` → `target`) and duplicates removed, so a path is
    /// written exactly one way from here on — the rail label, the `StepKey`
    /// scope, and the `dst.join(entry)` all agree. Entries are
    /// worktree-root-relative and may contain glob metacharacters; see
    /// [`expand_entries`].
    pub paths: Vec<String>,
    /// What to do with an entry the filesystem cannot reflink. Defaulted to
    /// [`CopyFallback::Copy`] when the config did not say.
    pub fallback: CopyFallback,
    /// Per-entry size cap in bytes, parsed from the config's `max_size`
    /// string. `None` = uncapped. Gates the **byte-copy fallback only** — a
    /// reflink is near-free and is never size-checked.
    ///
    /// Measured as what the copy will *write*: every hard link in the source
    /// becomes an independent file at the destination, so the cap is compared
    /// against the non-deduplicated sum rather than the `du`-style figure. A
    /// pnpm or ccache tree differs by an order of magnitude between the two.
    pub max_size_bytes: Option<u64>,
    /// The `max_size` string that could not be parsed, when there was one.
    ///
    /// An unparseable cap degrades to uncapped rather than to zero — but the
    /// user has to hear about it, because **nothing on any copying path runs
    /// `validate_config`** (its only caller is `daft hooks validate`).
    /// [`copy_entries`] warns once per run when this is set.
    pub max_size_unparsed: Option<String>,
}

/// Read and normalize the `copy:` section of the config rooted at
/// `source_root`.
///
/// Returns `None` when there is no config file, when it declares no `copy:`
/// key, or when the declared list is empty — all three mean "plan no rows,
/// do no work", and callers should treat them identically.
///
/// **Reads through [`crate::hooks::yaml_config_loader::load_merged_config`]**
/// — deliberately NOT the raw single-file loader `shared:` uses
/// (`core::shared::read_shared_paths`). That means `daft.local.yml` overlays
/// and `extends:` files reach `copy:`, so a visitor can declare or override
/// the section without touching the tracked config. `shared:`'s raw-loader
/// gap is pre-existing and is tracked as its own follow-up; do not "fix" it
/// by copying its pattern here.
///
/// Normalization, in order:
/// 1. `CopyConfig::Paths` / `::Full` collapse to `ResolvedCopyConfig`.
/// 2. Every entry loses its **trailing** `/`; entries that are empty or become
///    empty are dropped. A leading `/` is deliberately *not* stripped: an
///    absolute entry is refused, not quietly reinterpreted. `/target` is what
///    cargo writes into `.gitignore` and is the natural thing to paste, so the
///    refusal carries the fix — but rewriting it here would mean
///    `copy: ["/var"]` silently copying the worktree's own `var/` and
///    reporting success over a tree the config never named.
/// 3. Duplicates are dropped, first occurrence winning. Two entries that
///    normalize to the same path would share one `StepKey`, and the second
///    planned row would be swept away as unreported at teardown.
/// 4. `fallback` defaults to [`CopyFallback::Copy`].
/// 5. `max_size` is parsed to bytes via
///    `crate::coordinator::clean_policy::parse_size` (case-insensitive,
///    binary multiples, bare integer = bytes). An unparseable value degrades
///    to `None` (uncapped) and is recorded in
///    [`ResolvedCopyConfig::max_size_unparsed`] so [`copy_entries`] can say so
///    — nothing on the copying path runs the validator.
///
/// Never returns `Err`: a config that fails to load or parse yields `None`
/// and the copy stage silently does nothing, exactly as if the key were
/// absent. Loud config diagnostics are the loader's and validator's job, and
/// they run on every command; repeating them here would double the noise on
/// every worktree creation.
pub fn read_copy_config(source_root: &Path) -> Option<ResolvedCopyConfig> {
    let copy = crate::hooks::yaml_config_loader::load_merged_config(source_root)
        .ok()
        .flatten()?
        .copy?;

    let mut paths: Vec<String> = Vec::new();
    for declared in copy.paths() {
        let normalized = normalize_entry(declared);
        if normalized.is_empty() || paths.iter().any(|seen| seen == &normalized) {
            continue;
        }
        paths.push(normalized);
    }
    if paths.is_empty() {
        return None;
    }

    let raw_max_size = copy.max_size();
    let max_size_bytes =
        raw_max_size.and_then(|raw| crate::coordinator::clean_policy::parse_size(raw).ok());

    Some(ResolvedCopyConfig {
        paths,
        fallback: copy.fallback(),
        max_size_bytes,
        // Degrading to uncapped beats degrading to zero, which would turn one
        // typo into a stage that mysteriously copies nothing — but the fact
        // travels so the run can report it.
        max_size_unparsed: match (raw_max_size, max_size_bytes) {
            (Some(raw), None) => Some(raw.to_string()),
            _ => None,
        },
    })
}

/// One declared entry, written the single way everything downstream expects:
/// no surrounding slashes.
fn normalize_entry(declared: &str) -> String {
    // Trailing slashes only. A LEADING slash is not cosmetic — it is the
    // difference between `target` and `/target`, and stripping it silently
    // rewrote a config into one the user never wrote: `copy: ["/var"]` copied
    // the worktree's own `var/` and reported a green row over a tree the
    // config never named. It is refused instead, with a hint, by
    // `containment_violation`.
    declared.trim().trim_end_matches('/').to_string()
}

// ── Entry expansion ───────────────────────────────────────────────────────

/// Expand one declared entry against the source tree into the concrete
/// worktree-root-relative paths to copy.
///
/// - A **literal** entry (no `*`, `?`, or `[`) passes straight through as a
///   single result, whether or not it exists — the existence check belongs to
///   [`copy_entries`], which reports a missing source as its own outcome.
/// - A **glob** entry is matched against `source_root` using the `ignore`
///   crate's `WalkBuilder` (already a dependency — no new one is needed) with
///   an `OverrideBuilder` carrying the pattern:
///   - **gitignore filtering off** (`git_ignore(false)`, `git_global(false)`,
///     `git_exclude(false)`, `ignore(false)`, `hidden(false)`): `copy:`
///     entries are gitignored *by definition*, so leaving git's filters on
///     would match nothing.
///   - **descent pruned below a match**: once `web/dist` matches, its
///     contents are not walked and `web/dist/assets` is not reported.
///     Copying the parent already carries the children, and a per-file
///     result set would make the annotation meaningless.
/// - Results are deduplicated and returned in a deterministic (sorted) order
///   so a rail annotation is stable across runs.
///
/// Returns an empty vector for a glob that matches nothing — the caller
/// reports that as an expected skip, not an error. Never returns `Err`:
/// walk errors are skipped entries, never a failed creation.
pub fn expand_entries(source_root: &Path, entry: &str) -> Vec<String> {
    expand_reporting(source_root, entry).matches
}

/// What expanding one entry found, including what it could not read.
pub(crate) struct Expansion {
    /// Worktree-relative matches, deduplicated and sorted.
    pub matches: Vec<String>,
    /// Places the walk could not read: the first one as `(path, detail)`, and
    /// how many there were in total.
    ///
    /// Swallowing these — the shape [`expand_entries`] is stuck with — either
    /// under-reports a green `Copied` for a tree half of which was never seen,
    /// or reports `NoMatches` for a glob whose matches were simply unreachable.
    /// Both are silent, and root-owned directories inside a container image are
    /// the common way they happen.
    pub unreadable: Vec<(String, String)>,
}

/// [`expand_entries`], keeping the walk errors it has nowhere to put.
pub(crate) fn expand_reporting(source_root: &Path, entry: &str) -> Expansion {
    if !is_glob(entry) {
        return Expansion {
            matches: vec![entry.to_string()],
            unreadable: Vec::new(),
        };
    }

    let mut builder = OverrideBuilder::new(source_root);
    // A pattern the glob compiler rejects expands to nothing; the caller
    // reports that as an expected skip rather than failing the creation.
    if builder.add(entry).is_err() {
        return Expansion {
            matches: Vec::new(),
            unreadable: Vec::new(),
        };
    }
    let Ok(overrides) = builder.build() else {
        return Expansion {
            matches: Vec::new(),
            unreadable: Vec::new(),
        };
    };
    let overrides = Arc::new(overrides);

    let root = source_root.to_path_buf();
    let prune = Arc::clone(&overrides);
    let mut walker = WalkBuilder::new(source_root);
    walker
        // `copy:` entries are gitignored by definition, so every filter that
        // exists to hide ignored files would hide exactly what we came for.
        .hidden(false)
        .parents(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .follow_links(false)
        .filter_entry(move |found| descend_into(found, &root, &prune));

    let mut matches = Vec::new();
    let mut unreadable = Vec::new();
    for result in walker.build() {
        let found = match result {
            Ok(found) => found,
            Err(err) => {
                // The offending path, not the entry: "'pkg/locked' could not
                // be read" tells the user where to look; the entry name is
                // already the row's label.
                let path = walk_error_path(&err)
                    .and_then(|p| p.strip_prefix(source_root).ok())
                    .and_then(relative_to_slash_string)
                    .unwrap_or_else(|| entry.to_string());
                unreadable.push((path, err.to_string()));
                continue;
            }
        };
        let is_dir = found.file_type().is_some_and(|t| t.is_dir());
        if !overrides.matched(found.path(), is_dir).is_whitelist() {
            continue;
        }
        let Ok(rel) = found.path().strip_prefix(source_root) else {
            continue;
        };
        match relative_to_slash_string(rel) {
            Some(rel) => matches.push(rel),
            // A name that cannot be written as a `/`-separated UTF-8 string has
            // nowhere to go: not a config entry, not a git pathspec, not a
            // `StepKey` scope. Dropping it silently would shrink the expansion
            // with nothing to show for it, so it counts as a place the walk
            // could not use.
            None => unreadable.push((
                rel.to_string_lossy().into_owned(),
                "the name is not usable as a worktree-relative UTF-8 path".to_string(),
            )),
        }
    }
    matches.sort();
    matches.dedup();
    Expansion {
        matches,
        unreadable,
    }
}

/// The path an `ignore` walk error is about, dug out of its wrapper layers.
/// The crate boxes errors inside `WithDepth`/`WithLineNumber`, so the path is
/// rarely at the top.
fn walk_error_path(err: &ignore::Error) -> Option<&Path> {
    match err {
        ignore::Error::WithPath { path, .. } => Some(path),
        ignore::Error::WithDepth { err, .. } | ignore::Error::WithLineNumber { err, .. } => {
            walk_error_path(err)
        }
        ignore::Error::Loop { child, .. } => Some(child),
        _ => None,
    }
}

/// Render a walked relative path the one way everything downstream reads it:
/// `/`-separated, whatever the platform's separator is.
///
/// On Windows a native `web\\dist` would go straight into a git pathspec and a
/// `.gitignore` comparison, so every glob entry would be refused as not
/// gitignored with a warning that named a path git had never heard of. Nothing
/// in CI catches that — `windows-check` is a `cargo check`.
///
/// `None` for the walk root itself (no components) and for any name that is not
/// UTF-8 or not a plain path component. Refusing the empty result is what keeps
/// a root match from ever being reported as an entry: the worktree root is not
/// something `copy:` may replicate.
fn relative_to_slash_string(rel: &Path) -> Option<String> {
    let mut out = String::new();
    for component in rel.components() {
        let std::path::Component::Normal(part) = component else {
            return None;
        };
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(part.to_str()?);
    }
    (!out.is_empty()).then_some(out)
}

/// True when the entry carries glob metacharacters and must be expanded
/// against the tree. Everything else is a literal path.
fn is_glob(entry: &str) -> bool {
    entry.contains(['*', '?', '['])
}

/// `filter_entry` predicate for [`expand_entries`]: prune git's own directory,
/// and prune descent below anything the pattern already matched.
///
/// Returning `false` for a directory both skips it and stops the walk from
/// descending into it, which is exactly the "prune below a match" rule — so
/// only *strict* ancestors are consulted here. The matched directory itself has
/// no matching ancestor, passes, and is yielded; its children then find it and
/// stop.
fn descend_into(found: &ignore::DirEntry, root: &Path, overrides: &Override) -> bool {
    if found.file_name() == std::ffi::OsStr::new(".git")
        && found.file_type().is_some_and(|t| t.is_dir())
    {
        return false;
    }
    let Ok(rel) = found.path().strip_prefix(root) else {
        return true;
    };
    let components: Vec<_> = rel.components().collect();
    let Some(strict_ancestors) = components.len().checked_sub(1).filter(|n| *n > 0) else {
        return true;
    };
    let mut prefix = root.to_path_buf();
    for component in &components[..strict_ancestors] {
        prefix.push(component);
        if overrides.matched(&prefix, true).is_whitelist() {
            return false;
        }
    }
    true
}

// ── Per-entry outcomes ────────────────────────────────────────────────────

/// How one copied entry was replicated — the provenance half of a
/// [`CopyOutcome::Copied`] annotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyMethod {
    /// The filesystem cloned the blocks (APFS `clonefile`, `FICLONE`, ReFS
    /// block clone). Near-free, and never size-gated.
    Reflinked,
    /// The filesystem could not reflink, and `fallback: copy` authorized a
    /// real byte copy. Size-gated by
    /// [`ResolvedCopyConfig::max_size_bytes`].
    Copied,
    /// Some of the entry cloned and some of it did not — an entry whose
    /// matches sit on different mounts, or one tree straddling a mount point.
    /// Reported honestly rather than rounded to whichever answer came first,
    /// because "reflinked" on a 40 GB byte copy is the lie that matters.
    Mixed,
}

/// Why one entry was left out. Each variant renders through [`skip_phrase`]
/// with no `skipped — ` prefix, so every one has to read as a complete clause.
///
/// Several variants carry the concrete path that provoked them. For a literal
/// entry that path *is* the entry and the phrase leaves it out; for a glob it
/// is the one match that offended, which is the only way a single row can say
/// what went wrong with a thirty-way expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// The entry does not exist in the source worktree. The quiet case: a
    /// declared cache that has simply never been built yet.
    NoSource,
    /// The entry exists but cannot be read — permissions, a symlink loop, an
    /// I/O error, an unreadable subtree found while expanding a glob. Distinct
    /// from [`Self::NoSource`] on purpose: an unreachable cache must not read
    /// as "never built yet".
    SourceUnreadable { path: String, detail: String },
    /// The entry already exists at the destination. The **idempotence**
    /// case that makes re-runs and hook-job composition safe: `daft warm`
    /// twice in a row is a no-op, and `copy:` never clobbers work a
    /// post-create hook already did.
    DestinationExists,
    /// The destination path exists but could not be read at all — permissions
    /// on a parent, an I/O error. Distinct from a shape conflict, which has
    /// established what is there; here nothing has.
    DestinationUnreadable { path: String, detail: String },
    /// git could not classify the **destination** — a different sentence from
    /// [`Self::Unclassifiable`], which is about the source. Sending someone to
    /// inspect the wrong worktree is worse than saying nothing.
    DestinationUnclassifiable { offender: String, detail: String },
    /// Something is already at the destination path, but not the same *kind*
    /// of thing as the source — a symlink where a directory belongs (what a
    /// path declared in both `shared:` and `copy:` leaves behind), a file
    /// where a tree belongs, or a dangling link. Existence alone would call
    /// that "already present" forever.
    DestinationConflict { path: String, detail: String },
    /// The entry is not gitignored, or git tracks content under it. `copy:`
    /// replicates caches, not the working tree — the attention case, because
    /// the config asked for something daft refuses to do.
    NotIgnored { offender: String },
    /// `--force` would have removed a destination that the **target** worktree
    /// tracks. The source's opinion is not enough: a path one branch gitignores
    /// can be committed content on another, and replacing it would delete work
    /// git is managing.
    TargetTracked { offender: String },
    /// git could not classify the path at all (not a repository, a probe that
    /// failed). Fail-closed — a probe that did not run is not consent — but
    /// said in its own words, because "must be gitignored" would be a
    /// diagnosis daft never actually made.
    Unclassifiable { offender: String, detail: String },
    /// The entry resolves outside the worktree, or names the worktree root
    /// itself. Refused before git is consulted: a config is not a capability
    /// to write anywhere, and `copy: ["."]` under `--force` would empty the
    /// worktree.
    Uncontained { offender: String, detail: String },
    /// Source and target are the same worktree. Nothing to do — and under
    /// `--force`, clearing each destination would delete the very caches the
    /// run was asked to replicate.
    SameWorktree,
    /// The filesystem cannot reflink this entry and `fallback: skip` said not
    /// to pay for a byte copy.
    NoReflink,
    /// The reflink probe could not run at all — it had nowhere writable to
    /// clone into. Deliberately **not** [`Self::NoReflink`]: that sentence
    /// blames the filesystem, and on APFS, where cloning is exactly what the
    /// machine does, it sends the user to change a `fallback:` knob that was
    /// never the problem. A probe that did not run also never feeds the
    /// byte-copy size gate.
    ReflinkUnprobeable { path: String, detail: String },
    /// The entry's byte-copy fallback would exceed
    /// [`ResolvedCopyConfig::max_size_bytes`]. Carries the measured size and
    /// the cap, both in bytes, so the row can quote both.
    TooLarge { size_bytes: u64, limit_bytes: u64 },
    /// A glob entry matched nothing in the source tree.
    NoMatches,
}

/// The outcome of one declared `copy:` entry.
///
/// Exactly one outcome per config entry, whatever the entry expanded to —
/// the plan's row identity is the entry, so the outcome's must be too.
/// A partially-successful glob (four matches copied, one failed) reports
/// [`CopyOutcome::Failed`] with the failing path named: a row that claimed
/// success while a cache is missing would be worse than a loud partial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyOutcome {
    /// The entry was replicated. `matches` is how many expanded paths were
    /// copied (1 for a literal entry), `bytes` their total apparent size, and
    /// `elapsed` the wall time — together they compose the row's annotation
    /// (`**/dist/ → 3 paths · 1.2 GB · reflinked · 0.3s`).
    Copied {
        entry: String,
        method: CopyMethod,
        matches: usize,
        bytes: u64,
        elapsed: Duration,
        /// Places the expansion could not read. A glob whose walk hit an
        /// unreadable subtree still copies everything it *did* find — refusing
        /// the lot would trade a partial cache for none at all — but the row
        /// has to say the expansion was incomplete, or a green tick would
        /// claim a completeness nobody established.
        unreadable: usize,
    },
    /// The entry was deliberately left out; see [`SkipReason`].
    Skipped {
        entry: String,
        reason: SkipReason,
        /// Places the expansion could not read, exactly as on
        /// [`CopyOutcome::Copied`].
        ///
        /// A partial glob that copies today and reports `already present`
        /// tomorrow would otherwise announce the shortfall once and hide it
        /// forever after — and `already present` is the most reassuring row
        /// the stage has.
        unreadable: usize,
    },
    /// The copy was attempted and something went wrong (I/O error, permission
    /// denied, disk full). **Not fatal** — it renders as an attention row and
    /// creation continues. `detail` is the error chain, already stringified.
    Failed { entry: String, detail: String },
}

impl CopyOutcome {
    /// The config entry this outcome belongs to — the `StepKey` scope, and
    /// therefore the join between an outcome and its planned row.
    pub fn entry(&self) -> &str {
        match self {
            Self::Copied { entry, .. }
            | Self::Skipped { entry, .. }
            | Self::Failed { entry, .. } => entry,
        }
    }
}

/// Result of running the copy stage over every declared entry.
///
/// One [`CopyOutcome`] per entry in [`ResolvedCopyConfig::paths`] order.
/// Mirrors `core::shared::LinkSharedResult`: a plain receipt bag, no
/// rendering, no `Result` — the stage cannot fail as a whole.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopyPathsResult {
    pub outcomes: Vec<CopyOutcome>,
}

impl CopyPathsResult {
    /// True when nothing was attempted (no config, or an empty section).
    pub fn is_empty(&self) -> bool {
        self.outcomes.is_empty()
    }

    /// How many entries were actually replicated — the number `daft warm`'s
    /// summary line reports.
    pub fn copied_count(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| matches!(o, CopyOutcome::Copied { .. }))
            .count()
    }

    /// Total bytes replicated across every copied entry.
    pub fn copied_bytes(&self) -> u64 {
        self.outcomes
            .iter()
            .filter_map(|o| match o {
                CopyOutcome::Copied { bytes, .. } => Some(*bytes),
                _ => None,
            })
            .sum()
    }
}

// ── The copy stage ────────────────────────────────────────────────────────

/// Copy every declared entry from `source` into `target`.
///
/// Called during worktree creation after `shared:` linking and before the
/// post-create hooks, and again by `daft warm` for a manual re-run. Pure
/// work — no rendering; rail call sites report through
/// [`report_copy_results`].
///
/// Per entry, in order (the first condition that holds wins):
///
/// 1. **Expand** via [`expand_entries`]. No matches →
///    [`SkipReason::NoMatches`]; a walk that could not finish →
///    [`SkipReason::SourceUnreadable`].
/// 2. **Containment**, before git is asked: an entry resolving outside the
///    worktree or onto the worktree root → [`SkipReason::Uncontained`].
/// 3. **Source missing** → [`SkipReason::NoSource`]; present but unreadable →
///    [`SkipReason::SourceUnreadable`].
/// 4. **Gitignored check** against `source`, batched over the whole entry:
///    `git check-ignore` must pass for every match AND `git ls-files` must
///    come back empty. A match that fails → [`SkipReason::NotIgnored`]; a
///    probe that could not run → [`SkipReason::Unclassifiable`], because a
///    failed probe is not consent. The second probe is defense in depth, not
///    a division of labour — see [`crate::core::git_ignore`]. Run git through
///    `crate::utils::git_command_at(source)` with both pipes nulled — an
///    inherited `GIT_DIR` silently overrides `-C`, and a stray
///    `fatal: not a git repository` on stderr would corrupt the rail.
/// 5. **Destination shape.** Absent → copy it. Present and the same kind of
///    thing → [`SkipReason::DestinationExists`], the idempotence case.
///    Present but a different kind — a symlink where a directory belongs, a
///    file where a tree belongs → [`SkipReason::DestinationConflict`], because
///    existence alone would wear "already present" forever.
/// 6. **Probe reflink per match**, by cloning the first regular file under it
///    into the nearest existing ancestor of *its own* destination. Probing
///    beats naming the filesystem, and probing per match beats probing once:
///    an entry's matches can sit on different mounts, and one sample answering
///    for all of them is how a byte copy gets labelled `reflinked` with the
///    size gate never consulted.
/// 7. **Gate the byte-copying matches only.** `fallback: skip` →
///    [`SkipReason::NoReflink`]; their summed write size over
///    [`ResolvedCopyConfig::max_size_bytes`] → [`SkipReason::TooLarge`]. A
///    clone is free and never counts towards the cap, even on a mixed entry.
/// 8. **Copy**, reporting [`CopyMethod`] from what the copier actually did.
///
/// `force` (`daft warm --force`) replaces an existing destination instead of
/// skipping it — but only after two more checks, and only at the last moment:
///
/// * the **target** must not track what is about to be deleted
///   ([`SkipReason::TargetTracked`]). A path one branch gitignores can be
///   committed content on another, and the source's opinion says nothing about
///   the destination.
/// * the removal happens in step 8, **after** the gate. Every refusal above
///   must be able to fire with the destination intact, or `--force` would
///   report a skip over a cache it had already destroyed.
///
/// A `source` and `target` that name the same directory are refused outright
/// ([`SkipReason::SameWorktree`]): a no-op under a normal run, and under
/// `force` it would delete the very caches it was asked to replicate.
///
/// **Never returns `Err`, and one entry's failure never affects another.**
/// Wrap each entry so an I/O error becomes [`CopyOutcome::Failed`] and the
/// loop continues. Creation must not fail because a cache did not copy.
///
/// `sink` is for **out-of-band** narration only — `on_step` / `on_debug`
/// progress under `-v`, and `on_warning` for anything not attributable to a
/// single entry. Per-entry facts belong in the returned outcomes and are
/// rendered exactly once, by [`report_copy_results`]; warning them here as
/// well would print every skip twice (once as a stderr line, once as a rail
/// row) and tear the live region.
///
/// The source tree is read live and is not quiesced — an entry being written
/// while it is copied yields a torn snapshot. That is accepted (a build cache
/// is regenerable); it must be documented, not defended against.
pub fn copy_entries(
    source: &Path,
    target: &Path,
    config: &ResolvedCopyConfig,
    force: bool,
    sink: &mut impl crate::core::ProgressSink,
) -> CopyPathsResult {
    // Stage-level, not entry-level: the cap applies to every entry, so saying
    // it once is honest and saying it per row would be noise. It has to be said
    // at all because nothing on this path runs `validate_config` — its only
    // caller is `daft hooks validate`, which nobody runs during a checkout.
    if let Some(raw) = &config.max_size_unparsed {
        sink.on_warning(&format!(
            "copy: could not read max_size {raw:?} — entries are uncapped this run; \
             `{}` explains the config",
            crate::daft_cmd("hooks validate")
        ));
    }

    // Copying a worktree into itself is a no-op request — and under `force` a
    // destructive one, because clearing each destination would clear the
    // source. Refuse it here rather than trusting every caller's source and
    // target resolution never to land on the same directory.
    if is_same_directory(source, target) {
        return CopyPathsResult {
            outcomes: config
                .paths
                .iter()
                .map(|entry| CopyOutcome::Skipped {
                    entry: entry.clone(),
                    reason: SkipReason::SameWorktree,
                    unreadable: 0,
                })
                .collect(),
        };
    }

    // Ancestors before descendants, regardless of declaration order.
    //
    // Ordering is by declared depth, so it does not catch a glob that expands
    // SHALLOWER than it is written (`x/y` then `**/**/x`, where the second
    // reaches `x`). That case is deliberately out of scope: both entries want
    // the same kind of thing in the same place, so the loser reports
    // `already present` — a dim, accurate row over a destination that exists
    // and is of the right shape. What it does not guarantee is that the
    // destination holds everything the broader entry would have brought, and
    // content-completeness across overlapping declarations is #201's territory
    // (delta detection), which composes on top of this rather than inside it.
    // `copy: [web/dist, web]` otherwise has the first entry manufacture an
    // empty `target/web` on its way to `target/web/dist`, and the second then
    // finds its destination "already present" — so everything else under
    // `web/` never copies, on creation and on every warm after it. Shallower
    // first is enough to make the containing entry win the race with its own
    // descendant; outcomes are reordered back to declaration order below so
    // the plan's rows still line up.
    let mut order: Vec<usize> = (0..config.paths.len()).collect();
    order.sort_by_key(|&i| config.paths[i].matches('/').count());

    let mut outcomes: Vec<Option<CopyOutcome>> = vec![None; config.paths.len()];
    for i in order {
        outcomes[i] = Some(copy_one(
            source,
            target,
            &config.paths[i],
            config,
            force,
            sink,
        ));
    }
    CopyPathsResult {
        outcomes: outcomes.into_iter().flatten().collect(),
    }
}

/// Whether two paths name the same directory, resolving symlinks. Falls back to
/// a literal comparison when either side cannot be canonicalized — a path that
/// does not resolve is not the one we are standing in.
fn is_same_directory(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// One entry's whole story, with the error boundary around it.
///
/// The boundary is the point of the split: below it every step may use `?`
/// freely, and above it an I/O error is just this row's outcome. That is what
/// makes "one entry's failure never affects another" structural rather than
/// something each step has to remember.
fn copy_one(
    source: &Path,
    target: &Path,
    entry: &str,
    config: &ResolvedCopyConfig,
    force: bool,
    sink: &mut impl crate::core::ProgressSink,
) -> CopyOutcome {
    match copy_one_inner(source, target, entry, config, force, sink) {
        Ok(outcome) => outcome,
        Err(err) => CopyOutcome::Failed {
            entry: entry.to_string(),
            // `{:#}` renders anyhow's whole chain on one line — the row has
            // one line to explain itself.
            detail: format!("{err:#}"),
        },
    }
}

/// One match, with everything decided about it.
struct Planned {
    rel: String,
    /// How this match's bytes will move, decided by its own probe rather than
    /// one taken on the entry's behalf.
    reflinks: bool,
    /// Bytes the copy will write — hard links counted every time, because the
    /// copy materializes each one.
    bytes: u64,
    /// A destination to clear first (`--force`), already proven safe to remove.
    remove_first: bool,
}

fn copy_one_inner(
    source: &Path,
    target: &Path,
    entry: &str,
    config: &ResolvedCopyConfig,
    force: bool,
    sink: &mut impl crate::core::ProgressSink,
) -> Result<CopyOutcome> {
    let started = Instant::now();
    let skipped = |reason, unreadable| {
        Ok(CopyOutcome::Skipped {
            entry: entry.to_string(),
            reason,
            unreadable,
        })
    };

    // ── 1. Expand ────────────────────────────────────────────────────────
    let Expansion {
        matches,
        unreadable,
    } = expand_reporting(source, entry);
    // An unreadable subtree must not be swallowed — silently walking past it
    // under-reports a green `Copied`, and root-owned directories inside a
    // container image are the common way it happens. But it must not cost the
    // matches that WERE found either: refusing the lot trades a partial cache
    // for none at all. Found matches are copied and the shortfall rides in the
    // annotation; only a walk that found nothing becomes the entry's outcome.
    if matches.is_empty() {
        let shortfall = unreadable.len();
        return match unreadable.into_iter().next() {
            Some((path, detail)) => {
                skipped(SkipReason::SourceUnreadable { path, detail }, shortfall)
            }
            None => skipped(SkipReason::NoMatches, 0),
        };
    }
    // Every refusal from here on carries it, so the shortfall cannot go quiet
    // just because the entry did not copy this time. (The two refusals above
    // pass their own count, which is all of it or none.)
    let unreadable_count = unreadable.len();

    // ── 2. Containment ───────────────────────────────────────────────────
    // Before git, and in its own words: a refusal here is not a diagnosis
    // about tracking.
    for rel in &matches {
        if let Some(detail) = containment_violation(rel) {
            return skipped(
                SkipReason::Uncontained {
                    offender: rel.clone(),
                    detail,
                },
                unreadable_count,
            );
        }
    }

    // ── 3. Source presence ───────────────────────────────────────────────
    // A glob only yields paths that exist; a literal is passed through
    // unchecked, so this is where "never built yet" is discovered — and where
    // an unreachable cache is told apart from an absent one.
    let mut present = Vec::new();
    for rel in &matches {
        match fs::symlink_metadata(source.join(rel)) {
            Ok(_) => present.push(rel.clone()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return skipped(
                    SkipReason::SourceUnreadable {
                        path: rel.clone(),
                        detail: err.to_string(),
                    },
                    unreadable_count,
                );
            }
        }
    }
    if present.is_empty() {
        return skipped(SkipReason::NoSource, unreadable_count);
    }

    // ── 4. The gitignored-only invariant, batched ────────────────────────
    // One violation disqualifies the whole entry: copying the clean half of a
    // declaration whose other half is tracked would be a success row over a
    // half-done job.
    match classify_matches(source, &present)? {
        Classification::Ignored => {}
        Classification::Tracked { offender } => {
            return skipped(SkipReason::NotIgnored { offender }, unreadable_count);
        }
        Classification::Unknown { offender, detail } => {
            return skipped(
                SkipReason::Unclassifiable { offender, detail },
                unreadable_count,
            );
        }
    }

    // ── 5. Destination shape, and what `--force` may clear ───────────────
    let mut planned: Vec<Planned> = Vec::new();
    for rel in &present {
        let src = source.join(rel);
        let dst = target.join(rel);
        let dst_meta = match fs::symlink_metadata(&dst) {
            Ok(meta) => Some(meta),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => {
                return skipped(
                    SkipReason::DestinationUnreadable {
                        path: rel.clone(),
                        detail: err.to_string(),
                    },
                    unreadable_count,
                );
            }
        };

        if let Some(dst_meta) = dst_meta {
            // Existence is not enough. A symlink where a directory belongs
            // (what a path declared in both `shared:` and `copy:` leaves), a
            // file where a tree belongs, or a dangling link would all wear
            // "already present" forever.
            let src_kind = path_kind(
                &fs::symlink_metadata(&src)
                    .with_context(|| format!("reading metadata of {}", src.display()))?,
            );
            let dst_kind = path_kind(&dst_meta);
            if src_kind != dst_kind {
                return skipped(
                    SkipReason::DestinationConflict {
                        path: rel.clone(),
                        detail: format!("a {dst_kind} where the source is a {src_kind}"),
                    },
                    unreadable_count,
                );
            }
            if !force {
                continue; // the idempotence case
            }
            // `--force` is the only path that destroys anything, so it is the
            // only one that has to ask the TARGET what it is about to delete.
            // The source's gitignore says nothing about the target: `docs/` can
            // be ignored on the branch being copied from and committed on the
            // branch being copied into, and replacing it would delete work git
            // is managing.
            match tracked_in(target, rel) {
                Ok(false) => {}
                Ok(true) => {
                    return skipped(
                        SkipReason::TargetTracked {
                            offender: rel.clone(),
                        },
                        unreadable_count,
                    );
                }
                // Fail closed: a probe that could not run is not permission to
                // delete. The target of a real run is always a worktree.
                Err(detail) => {
                    return skipped(
                        SkipReason::DestinationUnclassifiable {
                            offender: rel.clone(),
                            detail,
                        },
                        unreadable_count,
                    );
                }
            }
            planned.push(Planned {
                rel: rel.clone(),
                reflinks: false,
                bytes: 0,
                remove_first: true,
            });
            continue;
        }

        planned.push(Planned {
            rel: rel.clone(),
            reflinks: false,
            bytes: 0,
            remove_first: false,
        });
    }
    // Every match already there: the idempotence case. A glob with *some*
    // destinations present still copies the rest — skipping them would leave
    // those caches permanently missing after one interrupted run.
    if planned.is_empty() {
        return skipped(SkipReason::DestinationExists, unreadable_count);
    }

    // ── 6. Measure and probe, per match — only if anything will read them ─
    //
    // The gate below is a no-op under the default bare-list config
    // (`fallback: copy`, no `max_size`), and measuring means walking every
    // declared cache tree in full. On a reflinking filesystem that walk is the
    // dominant cost of the entire stage — several `node_modules` worth of
    // `lstat` spent computing a number nothing then reads. Nothing is lost by
    // skipping it: the row's method and byte count come from what the copier
    // reports, not from here.
    let gate_can_fire = gate_can_fire_for(config);
    if gate_can_fire {
        let sources: Vec<PathBuf> = planned.iter().map(|p| source.join(&p.rel)).collect();
        for (item, bytes) in planned.iter_mut().zip(measure_all(&sources)) {
            item.bytes = bytes;
        }
        for item in &mut planned {
            match reflinks_into(&source.join(&item.rel), &target.join(&item.rel), target)? {
                Ok(reflinks) => item.reflinks = reflinks,
                // Fail closed and say why: an unwritable destination is a real
                // problem with a real cause, and the copy would fail on it
                // anyway.
                Err(detail) => {
                    return skipped(
                        SkipReason::ReflinkUnprobeable {
                            path: item.rel.clone(),
                            detail,
                        },
                        unreadable_count,
                    );
                }
            }
        }

        // ── 7. The gate ──────────────────────────────────────────────────
        // Only the byte-copying matches are weighed: a clone is free, and a
        // cap written to stop an expensive copy has no business refusing a
        // free one.
        let byte_copy_bytes: u64 = planned
            .iter()
            .filter(|p| !p.reflinks)
            .map(|p| p.bytes)
            .sum();
        if let Some(reason) = gate_byte_copies(
            planned.iter().any(|p| !p.reflinks),
            byte_copy_bytes,
            config.fallback,
            config.max_size_bytes,
        ) {
            return skipped(reason, unreadable_count);
        }
    }

    // ── 8. Copy ──────────────────────────────────────────────────────────
    // Removal happens here and nowhere earlier. Every refusal above — a
    // missing source, a tracked entry on either side, `fallback: skip`, a cap
    // — must be able to fire with the destination still intact, or `--force`
    // would report a skip while having destroyed the cache.
    // Only quote a size that was actually measured — `0 B` on every entry
    // under the default config is a worse `-v` line than no size at all.
    sink.on_debug(&if gate_can_fire {
        format!(
            "copy: {entry} — {} path(s), {}",
            planned.len(),
            format_bytes(planned.iter().map(|p| p.bytes).sum::<u64>()),
        )
    } else {
        format!("copy: {entry} — {} path(s)", planned.len())
    });

    let mut stats = crate::cow_copy::CopyStats::default();
    for item in &planned {
        let dst = target.join(&item.rel);
        if item.remove_first
            && let Err(err) = remove_tree(&dst)
        {
            // `remove_dir_all` unlinks as it descends, so a failure can leave
            // a gutted destination — which reads as "already present" from
            // then on, exactly like a partial copy. Same sentence, same reason.
            return Err(note_surviving_remains(
                &dst,
                err.context(format!("clearing {}", dst.display())),
            ));
        }
        match replicate(&source.join(&item.rel), &dst) {
            Ok(item_stats) => {
                stats.reflinked += item_stats.reflinked;
                stats.copied += item_stats.copied;
                stats.bytes += item_stats.bytes;
            }
            Err(err) => return Err(discard_partial(&dst, err)),
        }
    }

    Ok(CopyOutcome::Copied {
        entry: entry.to_string(),
        method: method_of(&stats),
        matches: planned.len(),
        bytes: stats.bytes,
        elapsed: started.elapsed(),
        unreadable: unreadable_count,
    })
}

/// Whether [`gate_byte_copies`] can reach any outcome at all for this config.
///
/// `false` for the bare-list default (`fallback: copy`, no `max_size`), which
/// is what makes skipping the measurement safe: nothing downstream reads a
/// number the gate will not consult.
fn gate_can_fire_for(config: &ResolvedCopyConfig) -> bool {
    config.max_size_bytes.is_some() || config.fallback == CopyFallback::Skip
}

/// Whether the byte-copying part of an entry is allowed to proceed.
///
/// Pure, and separate from [`copy_one_inner`] for one reason: no single
/// filesystem can reach all of its outcomes. A dev machine on APFS always
/// clones and never sees these arms at all; Linux CI on ext4 or tmpfs never
/// clones and never sees the free path. Only a decision that takes the probe's
/// answer as an argument can be held to both at once.
///
/// Only the matches that will actually be byte-copied are weighed. A clone is
/// free, and a cap written to stop an expensive copy has no business refusing
/// a free one — including on a mixed entry, where the cloned half must not
/// count towards the cap the copied half is measured against.
fn gate_byte_copies(
    any_byte_copy: bool,
    byte_copy_bytes: u64,
    fallback: CopyFallback,
    max_size_bytes: Option<u64>,
) -> Option<SkipReason> {
    if !any_byte_copy {
        return None;
    }
    if fallback == CopyFallback::Skip {
        return Some(SkipReason::NoReflink);
    }
    match max_size_bytes {
        Some(limit) if byte_copy_bytes > limit => Some(SkipReason::TooLarge {
            size_bytes: byte_copy_bytes,
            limit_bytes: limit,
        }),
        _ => None,
    }
}

/// What a copy actually turned out to be, from what the copier reported.
///
/// Derived, not predicted: `reflink_or_copy` decides file by file, so a tree
/// straddling a mount point really can come back part-cloned. Rounding that to
/// whichever answer the probe gave would put "reflinked" on a real byte copy.
fn method_of(stats: &crate::cow_copy::CopyStats) -> CopyMethod {
    match (stats.reflinked, stats.copied) {
        (0, 0) => CopyMethod::Reflinked, // nothing but dirs and links: no bytes moved
        (_, 0) => CopyMethod::Reflinked,
        (0, _) => CopyMethod::Copied,
        _ => CopyMethod::Mixed,
    }
}

/// Why an entry may not be copied at all, before git is consulted. `None` when
/// it is a plain worktree-relative path.
///
/// A config is not a capability to write anywhere. Three refusals, kept
/// distinct because they send the user somewhere different:
///
/// * an **absolute** path;
/// * any `..` component, **wherever it sits**. Counting depth is not enough:
///   `link/../x` stays at depth 1 lexically — and git normalizes it the same
///   lexical way, so `check-ignore` would answer about a path that does not
///   exist — while the kernel resolves `link` as a symlink and lands wherever
///   it points, outside the worktree. No legitimate cache entry contains `..`,
///   so the blanket refusal costs nothing;
/// * a path that resolves to the **worktree root** (`.`, and the empty
///   string), whose `target.join(...)` is the destination worktree itself and
///   which `--force` would therefore empty.
fn containment_violation(relpath: &str) -> Option<String> {
    use std::path::Component;

    let path = Path::new(relpath);
    let mut depth = 0usize;
    for component in path.components() {
        match component {
            Component::ParentDir => {
                return Some(
                    "contains '..' and may resolve outside the worktree — not copied".to_string(),
                );
            }
            Component::RootDir | Component::Prefix(_) => {
                return Some(absolute_entry_hint(relpath));
            }
            Component::CurDir => {}
            Component::Normal(_) => depth += 1,
        }
    }
    (depth == 0).then(|| "names the worktree itself — not copied".to_string())
}

/// The refusal for an absolute entry, carrying the fix.
///
/// `/target` is what cargo writes into `.gitignore`, so a `copy:` list pasted
/// from one is the likeliest way an absolute entry appears — and "is an
/// absolute path" alone leaves the user to guess that dropping one character
/// is the answer. Shared with `daft hooks validate`, which catches it at
/// authoring time.
pub fn absolute_entry_hint(entry: &str) -> String {
    let anchored = entry.trim_start_matches('/').trim_end_matches('/');
    if anchored.is_empty() {
        return "is an absolute path — entries are relative to the worktree root".to_string();
    }
    format!(
        "is an absolute path — drop the leading '/' to anchor at the worktree root \
         (write '{anchored}')"
    )
}

/// Which kind of thing a path is, for the destination-shape check.
fn path_kind(meta: &fs::Metadata) -> &'static str {
    let ftype = meta.file_type();
    if ftype.is_symlink() {
        "symlink"
    } else if ftype.is_dir() {
        "directory"
    } else if ftype.is_file() {
        "file"
    } else {
        "special file"
    }
}

/// What the gitignored-only invariant decided about a set of matches.
enum Classification {
    Ignored,
    Tracked { offender: String },
    Unknown { offender: String, detail: String },
}

/// How many pathnames, and how many bytes of them, go to git at a time.
///
/// Both limits exist to keep a batch small enough that **neither** direction
/// can block:
///
/// * `check-ignore --stdin` echoes the ignored paths back, so the parent is
///   writing to one pipe while the child writes to another. Handing it the
///   whole list and only then reading deadlocks the moment either pipe fills —
///   64 KB on Linux, less on macOS — and a `node_modules` glob reaches that at
///   a few thousand paths.
/// * `ls-files` takes its pathspecs as argv, where the ceiling is `ARG_MAX`.
///   Crossing it fails the entry outright ("Argument list too long"), which on
///   a big repo means a cache that can never be copied.
///
/// A few hundred paths per batch sits orders of magnitude under both, and the
/// batch count is what the invocation cost is proportional to: two git
/// processes per batch, not per match.
const PROBE_BATCH_PATHS: usize = 256;
const PROBE_BATCH_BYTES: usize = 4096;

/// Split matches into batches neither pipe nor argv can choke on.
fn probe_batches(matches: &[String]) -> Vec<&[String]> {
    let mut batches = Vec::new();
    let mut start = 0;
    let mut bytes = 0;
    for (i, rel) in matches.iter().enumerate() {
        // +1 for the NUL separator / argv terminator.
        let cost = rel.len() + 1;
        if i > start && (i - start >= PROBE_BATCH_PATHS || bytes + cost > PROBE_BATCH_BYTES) {
            batches.push(&matches[start..i]);
            start = i;
            bytes = 0;
        }
        bytes += cost;
    }
    if start < matches.len() {
        batches.push(&matches[start..]);
    }
    batches
}

/// Both halves of the gitignored-only invariant for a whole entry, in two git
/// invocations per batch instead of three per match.
///
/// Per-match probing spawned `rev-parse` + `check-ignore` + `ls-files` for
/// every expanded path — around ninety processes for a thirty-match glob, on
/// the critical path of every worktree creation.
///
/// The two probes stay two: `check-ignore` consults the index and so already
/// refuses tracked content, but only as a behaviour of git's exclude machinery
/// — exactly what `--no-index` turns off. `ls-files` states the invariant
/// independently. Both arms fail **closed**: anything other than a clean answer
/// is `Unknown`, because a probe that did not run is not consent.
fn classify_matches(worktree: &Path, matches: &[String]) -> Result<Classification> {
    for batch in probe_batches(matches) {
        match classify_batch(worktree, batch)? {
            Classification::Ignored => {}
            decided => return Ok(decided),
        }
    }
    Ok(Classification::Ignored)
}

fn classify_batch(worktree: &Path, matches: &[String]) -> Result<Classification> {
    use std::io::Write;
    use std::process::Stdio;

    if matches.is_empty() {
        return Ok(Classification::Ignored);
    }
    let unknown = |detail: String| {
        Ok(Classification::Unknown {
            offender: matches[0].clone(),
            detail,
        })
    };

    let mut child = crate::utils::git_command_at(worktree)
        .args(["check-ignore", "-z", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("running git check-ignore")?;
    {
        // Dropped before `wait_with_output`, which closes the pipe and lets
        // the child finish. The batch is small enough that this write cannot
        // block on a full stdout pipe — see `PROBE_BATCH_BYTES`.
        let mut stdin = child.stdin.take().context("git check-ignore stdin")?;
        for rel in matches {
            stdin.write_all(rel.as_bytes())?;
            stdin.write_all(b"\0")?;
        }
    }
    let probe = child
        .wait_with_output()
        .context("running git check-ignore")?;
    // 0 = at least one path is ignored, 1 = none are. Anything else (128 — not
    // a repository, an unreadable parent) means git could not answer.
    if !matches!(probe.status.code(), Some(0 | 1)) {
        return unknown(format!(
            "git check-ignore {}",
            exit_description(&probe.status)
        ));
    }
    let ignored: std::collections::HashSet<String> = String::from_utf8_lossy(&probe.stdout)
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if let Some(offender) = matches.iter().find(|rel| !ignored.contains(*rel)) {
        return Ok(Classification::Tracked {
            offender: offender.clone(),
        });
    }

    let tracked = crate::utils::git_command_at(worktree)
        .args(["ls-files", "-z", "--"])
        .args(matches)
        .stderr(Stdio::null())
        .output()
        .context("running git ls-files")?;
    if !tracked.status.success() {
        return unknown(format!(
            "git ls-files {}",
            exit_description(&tracked.status)
        ));
    }
    if let Some(first) = String::from_utf8_lossy(&tracked.stdout)
        .split('\0')
        .find(|s| !s.is_empty())
        .map(str::to_string)
    {
        // Name the match that covers the tracked path, not the raw file: the
        // row is about a declaration, and `web/dist` is what the user wrote.
        let offender = matches
            .iter()
            .find(|rel| first == **rel || first.starts_with(&format!("{rel}/")))
            .cloned()
            .unwrap_or(first);
        return Ok(Classification::Tracked { offender });
    }

    Ok(Classification::Ignored)
}

/// How a git probe ended, for a reason string.
fn exit_description(status: &std::process::ExitStatus) -> String {
    status.code().map_or_else(
        || "ended on a signal".to_string(),
        |c| format!("exited {c}"),
    )
}

/// Does the worktree at `worktree` track anything at or under `relpath`?
///
/// The destination-side half of the invariant, and deliberately a narrower
/// question than [`classify_matches`] asks of the source. `copy:` requires its
/// *source* to be gitignored; of the *target* it only needs to know that
/// nothing is about to be deleted out from under git. An untracked, unignored
/// file at the destination is the user's own scratch work — `--force` may
/// replace it — but a committed one may not be touched.
///
/// `Err` when git could not answer, and the caller fails closed on it.
fn tracked_in(worktree: &Path, relpath: &str) -> std::result::Result<bool, String> {
    let out = crate::utils::git_command_at(worktree)
        .args(["ls-files", "-z", "--", relpath])
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|err| format!("git ls-files could not run: {err}"))?;
    if !out.status.success() {
        return Err(format!(
            "git ls-files exited {}",
            out.status
                .code()
                .map_or_else(|| "on a signal".to_string(), |c| c.to_string())
        ));
    }
    Ok(!out.stdout.is_empty())
}

/// Throw away whatever a failed copy managed to land, and return the error the
/// caller should report.
///
/// `cow_copy::copy_dir` creates the destination first and populates it as it
/// walks, so a mid-walk failure (an unreadable subtree, a full disk, a symlink
/// it cannot recreate) leaves a **partial tree** behind. Left there, that tree
/// is indistinguishable from a finished one: the next creation and every `daft
/// warm` would see the destination exist, report `already present`, and skip —
/// masking a half-copied cache forever, silently, because warn-never-abort
/// means nothing ever raises. Clearing it is what makes the next run a retry.
///
/// Best-effort by nature: if the cleanup fails too, both facts go into the same
/// `Failed` detail, because a surviving partial tree is the more dangerous half
/// and has to be said out loud.
fn discard_partial(dst: &Path, err: anyhow::Error) -> anyhow::Error {
    if fs::symlink_metadata(dst).is_err() {
        return err; // nothing landed
    }
    match remove_tree(dst) {
        Ok(()) => err,
        Err(cleanup) => note_surviving_remains(dst, err.context(format!("{cleanup:#}"))),
    }
}

/// If anything is still at `dst`, say so — in the same words for both ways it
/// can happen.
///
/// A failed copy and a failed `--force` removal leave the same hazard behind: a
/// path that exists, is not what anyone asked for, and will be read as
/// "already present" by every later run. The detail is the only place that can
/// be said, because warn-never-abort means nothing raises.
fn note_surviving_remains(dst: &Path, err: anyhow::Error) -> anyhow::Error {
    if fs::symlink_metadata(dst).is_err() {
        return err;
    }
    anyhow::anyhow!(
        "{err:#}; what is left at {} will be mistaken for a finished copy",
        dst.display()
    )
}

/// Remove a path and everything under it — the one removal helper, shared by
/// `--force`'s pre-copy clear and [`discard_partial`]'s cleanup.
///
/// Both need the same second chance: `cow_copy` reproduces source modes
/// faithfully, so a mode-000 directory in the cache becomes a mode-000
/// directory in the copy, and `remove_dir_all` has to read each directory to
/// recurse. A half-removed `--force` destination is the same failure as a
/// half-written one — it reads as "already present" from then on.
fn remove_tree(path: &Path) -> Result<()> {
    if remove_existing(path).is_ok() {
        return Ok(());
    }
    unlock_tree(path);
    remove_existing(path)
}

/// Restore owner read/write/execute on every directory in a doomed tree,
/// top-down so each level can be read to reach the next.
///
/// Only ever runs on a tree daft is about to delete, and never follows symlinks
/// out of it (`symlink_metadata` reports a link as a non-directory), so it
/// cannot widen permissions anywhere the copy did not already write.
#[cfg(unix)]
fn unlock_tree(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let Ok(meta) = fs::symlink_metadata(path) else {
        return;
    };
    if !meta.is_dir() {
        return;
    }
    let mode = meta.permissions().mode();
    if mode & 0o700 != 0o700 {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o700));
    }
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        unlock_tree(&entry.path());
    }
}

#[cfg(not(unix))]
fn unlock_tree(_path: &Path) {}

/// Remove one existing path, once.
fn remove_existing(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)
        .with_context(|| format!("reading metadata of {}", path.display()))?;
    // `symlink_metadata` never reports a symlink as a directory, so a symlink
    // to a directory is unlinked rather than recursively deleted.
    if meta.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
    .with_context(|| format!("removing {}", path.display()))
}

/// Copy one concrete path, preserving what it is, and report what it cost.
///
/// Directories and regular files go through [`crate::cow_copy`] (reflink where
/// the filesystem allows, byte copy where it does not). A symlink is recreated
/// rather than dereferenced — a `.venv` or `node_modules` that *is* a link
/// should stay one, and following it would copy a tree living outside the
/// worktree entirely.
fn replicate(src: &Path, dst: &Path) -> Result<crate::cow_copy::CopyStats> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let ftype = fs::symlink_metadata(src)
        .with_context(|| format!("reading metadata of {}", src.display()))?
        .file_type();

    if ftype.is_symlink() {
        replicate_symlink(src, dst)?;
        Ok(crate::cow_copy::CopyStats::default())
    } else if ftype.is_dir() {
        crate::cow_copy::copy_dir_reporting(src, dst)
    } else if ftype.is_file() {
        crate::cow_copy::copy_file_reporting(src, dst)
    } else {
        anyhow::bail!("{} is not a file, directory, or symlink", src.display())
    }
}

#[cfg(unix)]
fn replicate_symlink(src: &Path, dst: &Path) -> Result<()> {
    // The entry IS the link, so its "copied tree" is the link itself and any
    // target at all is outside it: a relative `.venv -> ../.venvs/proj` gets
    // re-based onto the destination's depth rather than copied verbatim into a
    // dangling one. See `cow_copy::rebased_link_target`.
    let link = crate::cow_copy::rebased_link_target(src, dst, src)?;
    std::os::unix::fs::symlink(link, dst)
        .with_context(|| format!("creating symlink {}", dst.display()))
}

#[cfg(not(unix))]
fn replicate_symlink(src: &Path, _dst: &Path) -> Result<()> {
    // Same stance as `cow_copy::copy_dir`: recreating a link here needs the
    // file-vs-dir distinction up front plus elevated privileges.
    anyhow::bail!(
        "symlink replication is not supported on this platform: {}",
        src.display()
    )
}

/// What copying `src` will write.
///
/// Hard links are counted **every time**, unlike `du` and unlike `daft list`:
/// the copier materializes each link as an independent file, so a pnpm or
/// ccache tree costs several times its measured size at the destination. Gating
/// `max_size` on the deduplicated figure is how a 500 MB measurement lands 5 GB.
///
/// Directories go through the shared bounded parallel walker
/// ([`crate::core::size_walk`]) — the same `readdir`/`lstat` budget `daft list`
/// uses — so measuring a `node_modules` cannot oversubscribe the device. An
/// unmeasurable root contributes nothing rather than aborting the copy; the
/// copy itself will fail honestly if the tree is truly unreadable.
fn measure_all(paths: &[PathBuf]) -> Vec<u64> {
    let mut sizes = vec![0u64; paths.len()];
    let mut dirs = Vec::new();
    let mut dir_slots = Vec::new();
    for (slot, path) in paths.iter().enumerate() {
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => {
                dirs.push(path.clone());
                dir_slots.push(slot);
            }
            Ok(meta) => sizes[slot] = meta.len(),
            Err(_) => {}
        }
    }
    if !dirs.is_empty() {
        // ONE walk for the whole entry. The walker's budget is a bounded pool
        // built per call, so measuring a thirty-way glob one match at a time
        // spun up (and tore down) thirty pools against the same device.
        let jobs = crate::core::size_walk::resolve_jobs(None);
        let walked = crate::core::size_walk::walk_all_sized(
            &dirs,
            None,
            jobs,
            crate::core::size_walk::HardLinks::CountEveryLink,
        );
        for (slot, size) in dir_slots.into_iter().zip(walked) {
            sizes[slot] = size.unwrap_or(0);
        }
    }
    sizes
}

/// Can this match's bytes be cloned into its own destination?
///
/// Three answers, not two. `Ok(true)`/`Ok(false)` are the filesystem's verdict;
/// `Err` means the probe never ran — an unwritable destination, no directory to
/// probe in. Collapsing that third case into "cannot reflink" reported
/// `no reflink support — fallback: skip` on APFS, where reflink support is
/// exactly what the machine has, and fed a cloneable entry to the byte-copy
/// size gate it should never have reached.
///
/// Answered by attempting it, not by naming the filesystem: `copy:` entries can
/// straddle mount points (an externally-mounted `node_modules`, a `target/` on
/// a scratch volume), and reflink support is a property of the *pair* of
/// locations — which is why the probe lands next to where this match's copy
/// will land, not in the worktree root.
///
/// A match with no regular file under it — an empty cache, a tree of only
/// directories and symlinks — has no bytes to clone and so cannot fail to
/// clone. It reports `true`, which keeps a free copy from being refused by
/// `fallback: skip`. A match that could not be *read* is a different answer and
/// propagates as an error rather than passing for clonable.
fn reflinks_into(
    src: &Path,
    dst: &Path,
    target_root: &Path,
) -> Result<std::result::Result<bool, String>> {
    let Some(sample) = first_regular_file(src)? else {
        return Ok(Ok(true));
    };
    let Some(probe_dir) = nearest_existing_dir(dst, target_root) else {
        return Ok(Err(format!(
            "no directory to probe in below {}",
            target_root.display()
        )));
    };
    Ok(attempt_reflink(&sample, probe_dir))
}

/// The closest existing ancestor of `path`, never climbing above `boundary` —
/// where a probe can land without creating anything.
///
/// Creating the destination's parents up front would manufacture exactly the
/// empty ancestor stubs that make a later entry read "already present". The
/// boundary is what keeps the search from walking out of the target worktree
/// entirely when the target itself is missing: a scratch file must never be
/// written above the tree daft was asked to fill, and answering "cannot clone"
/// is the correct response to a destination that is not there.
fn nearest_existing_dir<'a>(path: &'a Path, boundary: &Path) -> Option<&'a Path> {
    let mut candidate = path.parent();
    while let Some(dir) = candidate {
        if !dir.starts_with(boundary) {
            return None;
        }
        if dir.is_dir() {
            return Some(dir);
        }
        candidate = dir.parent();
    }
    None
}

/// Clone `sample` into a scratch name inside `dst_dir` and immediately unlink
/// it. The clone shares blocks, so the probe costs no space even when the
/// sample is large; the temp file's guard removes it even on a panic.
fn attempt_reflink(sample: &Path, dst_dir: &Path) -> std::result::Result<bool, String> {
    let guard = tempfile::Builder::new()
        .prefix(".daft-reflink-probe-")
        .tempfile_in(dst_dir)
        .map_err(|err| {
            format!(
                "could not write a probe file in {}: {err}",
                dst_dir.display()
            )
        })?;
    // `reflink` needs an absent destination; the guard still owns the name and
    // unlinks whatever is there when it drops.
    fs::remove_file(guard.path())
        .map_err(|err| format!("could not clear the probe file: {err}"))?;
    Ok(reflink_copy::reflink(sample, guard.path()).is_ok())
}

/// The first regular file at or under `path`, for the reflink probe.
///
/// `Ok(None)` means there is genuinely nothing clonable (an empty tree, or one
/// of only directories and symlinks). `Err` means the walk could not finish and
/// found nothing — which must not be mistaken for "nothing to clone", or an
/// unreadable tree would skip the size gate and be labelled `reflinked`.
fn first_regular_file(path: &Path) -> Result<Option<PathBuf>> {
    let meta = fs::symlink_metadata(path)
        .with_context(|| format!("reading metadata of {}", path.display()))?;
    if meta.is_file() {
        return Ok(Some(path.to_path_buf()));
    }
    if !meta.is_dir() {
        return Ok(None);
    }
    let mut walk_error = None;
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        match entry {
            Ok(entry) if entry.file_type().is_file() => return Ok(Some(entry.into_path())),
            Ok(_) => {}
            // Remember it, but keep looking: a sample found elsewhere answers
            // the question, and the copy will hit the same unreadable spot and
            // fail honestly.
            Err(err) => walk_error = Some(err),
        }
    }
    match walk_error {
        Some(err) => Err(anyhow::Error::new(err).context(format!("reading {}", path.display()))),
        None => Ok(None),
    }
}

/// Probe whether `dir`'s filesystem can clone blocks, for `daft doctor`.
///
/// The same attempt the copy stage makes, against a scratch file of daft's own
/// so the answer never depends on what happens to be in the tree. `None` when
/// the probe could not run at all (an unwritable directory) — a different
/// answer from "no reflink support", and doctor says so.
pub fn probe_reflink_support(dir: &Path) -> Option<bool> {
    let sample = tempfile::Builder::new()
        .prefix(".daft-reflink-probe-")
        .tempfile_in(dir)
        .ok()?;
    // Clone something rather than nothing: a zero-length file is a case some
    // filesystems shortcut, which would not answer the question asked.
    fs::write(sample.path(), b"daft reflink probe").ok()?;
    attempt_reflink(sample.path(), dir).ok()
}

// ── Plan / report ─────────────────────────────────────────────────────────

/// Append the copied-paths section to a creation plan: a dim group anchor
/// plus one row per declared **entry**, each carrying the entry as its fixed
/// label, closed with an `EndGroup` so the ungrouped rows that follow (the
/// post-create hooks) never adopt the anchor (#651). No-op when nothing is
/// declared.
///
/// Mirrors `core::shared::push_shared_section` exactly, including the
/// no-filesystem-access rule: the plan commits before any mutation, so a glob
/// entry is planned as itself, unexpanded. `planned` is
/// [`ResolvedCopyConfig::paths`], and every string in it must later reach
/// [`report_copy_results`] as a `planned` element so no row is orphaned.
pub fn push_copy_section(rows: &mut Vec<crate::core::stage::Row>, planned: &[String]) {
    use crate::core::stage::{Row, StageId, StepKey, StepSpec};

    if planned.is_empty() {
        return;
    }
    rows.push(Row::Group {
        label: "copied paths".into(),
    });
    for entry in planned {
        rows.push(Row::Step(
            StepSpec::new(StepKey::scoped(StageId::CopyPath, entry.as_str()))
                .with_label(entry.as_str()),
        ));
    }
    rows.push(Row::EndGroup);
}

/// Report copy outcomes as stage events against the planned copied-paths
/// section (#651).
///
/// Every declared entry leaves a receipt row — the section answers "what
/// happened to each declared cache?":
///
/// | Outcome | Event | Face |
/// |---|---|---|
/// | [`CopyOutcome::Copied`] | `Completed { annotation }` | green `✓`, annotated `3 paths · 1.2 GB · reflinked · 0.3s` |
/// | [`SkipReason::NoSource`] / [`SkipReason::DestinationExists`] / [`SkipReason::NoMatches`] | `SkippedExpected` | dim |
/// | every other [`SkipReason`] | `SkippedAttention` | yellow |
/// | [`CopyOutcome::Failed`] | `SkippedAttention` | yellow |
///
/// The dim three are the stage working as designed — a cache that has not been
/// built, a worktree already warm, a glob that legitimately matched nothing.
/// Everything else is the config asking for something that did not happen.
///
/// **Never `Failed`.** A `Failed` face says the operation the user asked for
/// did not happen; here the operation is the worktree, and it did. A cache
/// that did not copy is an attention skip, which is why the engine's
/// warn-never-abort contract and this table have to agree.
///
/// Skip reasons render without a `skipped — ` prefix (the timeline exempts
/// [`StageId::CopyPath`](crate::core::stage::StageId::CopyPath) along with
/// `SharedFile`), so each one must read as a complete phrase — e.g.
/// `must be gitignored — tracked content is never copied`,
/// `nothing to copy yet`, `already present`,
/// `2.1 GB — over the 1 GB max_size`. The row's label supplies the entry, so
/// the phrases never name it; see [`skip_phrase`].
///
/// A planned entry that produced no outcome at all resolves as
/// `SkippedSilent` and its row is removed — the finished rail lists only
/// entries that actually resolved.
pub fn report_copy_results(
    result: &CopyPathsResult,
    planned: &[String],
    sink: &mut impl crate::core::ProgressSink,
) {
    use crate::core::stage::{StageEvent, StageId, StepKey};

    for outcome in &result.outcomes {
        let event = match outcome {
            CopyOutcome::Copied {
                method,
                matches,
                bytes,
                elapsed,
                unreadable,
                ..
            } => StageEvent::Completed {
                annotation: Some(copied_annotation(
                    *matches,
                    *bytes,
                    *method,
                    *elapsed,
                    *unreadable,
                )),
            },
            CopyOutcome::Skipped {
                entry,
                reason,
                unreadable,
            } => {
                let reason = with_shortfall(skip_phrase(entry, reason), *unreadable);
                // The split is "did the config ask for something daft refused
                // to do?" — an unbuilt cache or an already-warm worktree is
                // the stage working as designed; a tracked entry, an
                // un-reflinkable one under `fallback: skip`, and an oversized
                // one are all the config not getting what it asked for.
                match reason_needs_attention(outcome) {
                    true => StageEvent::SkippedAttention { reason },
                    false => StageEvent::SkippedExpected { reason },
                }
            }
            // Never a `Failed` face: a `Failed` row says the operation the
            // user asked for did not happen, and the operation here is the
            // worktree, which did.
            CopyOutcome::Failed { detail, .. } => StageEvent::SkippedAttention {
                reason: failure_phrase(detail),
            },
        };
        sink.on_stage(&StepKey::scoped(StageId::CopyPath, outcome.entry()), event);
    }

    for entry in planned {
        if !result.outcomes.iter().any(|o| o.entry() == entry) {
            sink.on_stage(
                &StepKey::scoped(StageId::CopyPath, entry.as_str()),
                StageEvent::SkippedSilent,
            );
        }
    }
}

/// Whether a skip is the yellow face (the config asked for something that did
/// not happen) or the dim one (nothing to do).
///
/// Public because `daft warm` splits its plain lines down the same seam — the
/// warning channel versus the quiet one — and counts the yellow ones in its
/// summary. Re-listing the quiet three there would mean a fourteenth
/// `SkipReason` silently landing in the wrong channel on one surface only.
pub fn reason_needs_attention(outcome: &CopyOutcome) -> bool {
    let CopyOutcome::Skipped { reason, .. } = outcome else {
        return false;
    };
    // Only three skips are the stage working as designed: a cache that has not
    // been built, a worktree that is already warm, and a glob that legitimately
    // matched nothing. Everything else is the config asking for something that
    // did not happen, and has to be able to say so in yellow.
    !matches!(
        reason,
        SkipReason::NoSource | SkipReason::DestinationExists | SkipReason::NoMatches
    )
}

// ── Shared phrasing ───────────────────────────────────────────────────────
//
// The builders below are the ONE place a copy outcome becomes words. Two very
// different surfaces render the same facts — the creation rail's section rows
// and `daft warm`'s plain per-entry lines — and a second copy of these strings
// would drift the moment either was edited alone. Both consume these.
//
// The division between them is which surface already names the entry:
//
// * [`skip_phrase`] is the **bare clause**, for anywhere the entry is already
//   on screen. The rail row's label *is* the entry, so a phrase that quoted it
//   again would render `node_modules  'node_modules' must be gitignored…`.
// * [`qualified_phrase`] adds the entry back, for a flat stderr line that has
//   no label to lean on.
//
// A phrase names a path only when it is *not* the entry — the one match of a
// thirty-way glob that offended, which is the only way one row can say what
// went wrong with the whole expansion.

/// The bare clause for one skipped entry — never naming the entry itself.
///
/// `CopyPath` is exempt from the timeline's `skipped — ` prefix (beside
/// `SharedFile`), so every arm still has to read as a complete clause:
/// `already present`, `nothing to copy yet`, `2.1 GB — over the 1 GB max_size`.
///
/// `entry` is passed so a phrase can tell "the entry itself offended" from "one
/// of its matches did", not so it can be printed.
pub fn skip_phrase(entry: &str, reason: &SkipReason) -> String {
    // `web/dist` for a glob that expanded to it; nothing at all when the
    // offending path IS the entry and naming it would stutter.
    let named = |offender: &str| {
        if offender == entry {
            String::new()
        } else {
            format!("'{offender}' ")
        }
    };

    match reason {
        SkipReason::NoSource => "nothing to copy yet".to_string(),
        SkipReason::SourceUnreadable { path, detail } => {
            format!("{}could not be read — {detail}", named(path))
        }
        SkipReason::DestinationExists => "already present".to_string(),
        SkipReason::DestinationConflict { path, detail } => {
            format!("{}already present as {detail} — not replaced", named(path))
        }
        // Says nothing about presence, because nothing was established: the
        // path could not be read at all.
        SkipReason::DestinationUnreadable { path, detail } => {
            format!(
                "{}the destination could not be read — {detail}",
                named(path)
            )
        }
        SkipReason::DestinationUnclassifiable { offender, detail } => format!(
            "{}the destination could not be classified by git — {detail}",
            named(offender)
        ),
        // Leads with the requirement, not the diagnosis. The one variant covers
        // a tracked entry, an untracked-but-unignored one, and an ignored
        // directory holding force-added content — so a bare "is tracked" would
        // be false for the commonest cause of it: a cache the user simply
        // forgot to gitignore. Naming the rule is true in every case and says
        // what to change, while still carrying the word a reader looks for.
        SkipReason::NotIgnored { offender } => format!(
            "{}must be gitignored — tracked content is never copied",
            named(offender)
        ),
        SkipReason::TargetTracked { offender } => format!(
            "{}is tracked in this worktree — refusing to replace it",
            named(offender)
        ),
        // Reads as a complete clause with or without a path: a literal entry
        // leaves no object gap behind "classify".
        SkipReason::Unclassifiable { offender, detail } => {
            format!(
                "{}could not be classified by git — {detail}",
                named(offender)
            )
        }
        // The detail carries its own ending here: the absolute-path refusal
        // ends in a remedy, and appending "— not copied" after it would put
        // three dashes in one line.
        SkipReason::Uncontained { offender, detail } => format!("{}{detail}", named(offender)),
        SkipReason::SameWorktree => "source and target are the same worktree".to_string(),
        SkipReason::NoReflink => "no reflink support — fallback: skip".to_string(),
        // Never "no reflink support": the filesystem was never asked.
        SkipReason::ReflinkUnprobeable { path, detail } => format!(
            "{}could not be tested for reflink support — {detail}",
            named(path)
        ),
        // Size first, then the cap it broke: the row is answering "why not?",
        // and the entry's own weight is the fact the user needs to act on.
        SkipReason::TooLarge {
            size_bytes,
            limit_bytes,
        } => format!(
            "{} — over the {} max_size",
            format_bytes(*size_bytes),
            format_bytes(*limit_bytes)
        ),
        SkipReason::NoMatches => "matched nothing".to_string(),
    }
}

/// The bare clause for an entry whose copy broke.
pub fn failure_phrase(detail: &str) -> String {
    format!("failed — {detail}")
}

/// Append the unreadable-places count to a skip's clause.
///
/// The same fact [`copied_annotation`] carries, in the one other place it can
/// be said. A partial glob that copies today and reports `already present`
/// tomorrow would otherwise announce the shortfall once and hide it forever —
/// and `already present` is the most reassuring row the stage has.
pub fn with_shortfall(phrase: String, unreadable: usize) -> String {
    if unreadable == 0 {
        return phrase;
    }
    format!("{phrase} · {unreadable} unreadable")
}

/// Put the entry back in front of a bare clause, for surfaces with no row label
/// to carry it.
///
/// The rail prints `node_modules` in one column and the clause in another; a
/// stderr line has one column, so `warning: no reflink support — fallback: skip`
/// would never say *which* cache. This is the rule both surfaces share, which
/// is why it is public: `daft warm` prints flat lines and needs the same one.
pub fn qualified_phrase(entry: &str, phrase: &str) -> String {
    format!("{entry}: {phrase}")
}

/// The completed entry's annotation: `3 paths · 1.2 GB · reflinked · 0.3s`.
///
/// `matches` counts expanded **paths**, not directories: an entry can name a
/// file, and a glob can match both. The count is dropped for a single match
/// (the row's label already names it) and the duration is dropped when it would
/// round to `0.0s`, which a reflink of a warm tree usually does. Size and method
/// always survive, because together they answer the only question the row
/// raises: did this cost anything?
///
/// `unreadable` appends `· N unreadable` when the expansion could not read
/// everywhere it looked. The row stays green — what was found was copied — but
/// a bare tick would claim a completeness nobody established.
pub fn copied_annotation(
    matches: usize,
    bytes: u64,
    method: CopyMethod,
    elapsed: Duration,
    unreadable: usize,
) -> String {
    let mut parts = Vec::new();
    if matches > 1 {
        parts.push(format!("{matches} paths"));
    }
    parts.push(format_bytes(bytes));
    parts.push(method_word(method).to_string());
    let seconds = elapsed.as_secs_f64();
    if seconds >= 0.05 {
        parts.push(format!("{seconds:.1}s"));
    }
    // The completeness caveat rides here rather than turning the row yellow:
    // what was found really was copied, and a green tick with `2 unreadable`
    // beside it claims exactly as much as happened.
    if unreadable > 0 {
        parts.push(format!("{unreadable} unreadable"));
    }
    parts.join(" · ")
}

// ── Rendering with no live region ─────────────────────────────────────────

/// The plain stderr line for one `CopyPath` stage event — `Copied <entry>` for
/// a completion, `warning: <reason>` for an attention skip, `None` for
/// everything else.
///
/// The rail is not always there. Under `--quiet`, in a pipe, in CI, and in the
/// YAML runner's non-TTY sandbox, no live region owns the terminal — and those
/// are precisely the places where nobody is watching a screen to notice a
/// missing row. Without this line the copy stage's warn-never-abort contract
/// would be *warn-invisible* exactly where it matters most: a tracked entry, an
/// oversized one, or a failed copy would leave no trace anywhere.
///
/// Mirrors [`crate::core::shared::legacy_shared_stage_line`], including which
/// events stay quiet: completions and attention skips speak, expected skips and
/// silent resolutions do not. A receipt for every declared entry is the rail's
/// job — a log only carries what changed or went wrong.
pub fn legacy_copy_stage_line(
    key: &crate::core::stage::StepKey,
    event: &crate::core::stage::StageEvent,
    use_color: bool,
) -> Option<String> {
    use crate::core::stage::{StageEvent, StageId};
    use crate::styles;

    if key.id != StageId::CopyPath {
        return None;
    }
    let entry = key.scope.as_deref()?;
    match event {
        StageEvent::Completed { annotation } => {
            let summary = annotation
                .as_deref()
                .map(|a| format!(" ({a})"))
                .unwrap_or_default();
            Some(if use_color {
                format!("{}Copied{} {entry}{summary}", styles::GREEN, styles::RESET)
            } else {
                format!("Copied {entry}{summary}")
            })
        }
        StageEvent::SkippedAttention { reason } => {
            let body = qualified_phrase(entry, reason);
            Some(if use_color {
                format!("{}warning:{} {body}", styles::YELLOW, styles::RESET)
            } else {
                format!("warning: {body}")
            })
        }
        _ => None,
    }
}

/// Legacy stderr rendering for one `CopyPath` stage event, byte-identical to
/// [`legacy_copy_stage_line`]. Sinks that route stage events call this when no
/// live region owns the terminal (Plain mode, quiet, tests); expected skips and
/// silent resolutions print nothing.
pub fn render_copy_stage_fallback(
    key: &crate::core::stage::StepKey,
    event: &crate::core::stage::StageEvent,
) {
    let use_color = crate::styles::colors_enabled_stderr();
    if let Some(line) = legacy_copy_stage_line(key, event, use_color) {
        eprintln!("{line}");
    }
}

/// How a copy happened, in one word, for annotations and `-v` narration.
fn method_word(method: CopyMethod) -> &'static str {
    match method {
        CopyMethod::Reflinked => "reflinked",
        CopyMethod::Copied => "copied",
        CopyMethod::Mixed => "part reflinked",
    }
}

/// A byte count in the copy stage's voice — `42 B`, `1.5 KB`, `2.1 GB` — with
/// a whole number left whole (`1 GB`, never `1.0 GB`), because a `max_size` the
/// user wrote as `1GB` should be quoted back to them the way they wrote it.
///
/// Public because `daft warm`'s summary totals the same bytes these phrases
/// quote, in the same output: a second formatter would print `1.0 GB` two
/// lines under this one's `1 GB`.
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    const TB: f64 = GB * 1024.0;

    let (value, unit) = match bytes as f64 {
        b if b >= TB => (b / TB, "TB"),
        b if b >= GB => (b / GB, "GB"),
        b if b >= MB => (b / MB, "MB"),
        b if b >= KB => (b / KB, "KB"),
        _ => return format!("{bytes} B"),
    };
    let rendered = format!("{value:.1}");
    format!(
        "{} {unit}",
        rendered.strip_suffix(".0").unwrap_or(&rendered)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::NullSink;
    use crate::core::git_ignore::IgnoreStatus;
    use tempfile::TempDir;

    /// Assert the sole outcome is a skip of a given shape, without pinning the
    /// paths the variant carries — those are covered by the phrase tests.
    macro_rules! assert_sole_skip {
        ($result:expr, $pattern:pat) => {
            match sole_skip(&$result) {
                $pattern => {}
                other => panic!("expected {}, got {other:?}", stringify!($pattern)),
            }
        };
    }

    /// Write `contents` at `path`, creating parents. Fixtures only.
    fn write(path: &Path, contents: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    /// A bare filesystem fixture: no git, no store, no worktrees — just a
    /// source directory and a target directory. Enough for expansion, which
    /// only ever asks the filesystem questions.
    fn fs_fixture() -> (TempDir, PathBuf, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("source");
        let target = tmp.path().join("target");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        (tmp, source, target)
    }

    /// The same fixture with the source turned into an isolated git repo whose
    /// `.gitignore` holds `ignore_rules` (never this project's repo — CLAUDE.md
    /// Critical Rule #2). The gitignored-only invariant is a git question, so
    /// the copy stage cannot be exercised without one; the destination stays a
    /// plain directory because the engine only ever joins paths onto it.
    fn repo_fixture(ignore_rules: &str) -> (TempDir, PathBuf, PathBuf) {
        let (tmp, source, target) = fs_fixture();
        // BOTH sides are repositories, because both are in a real run: the
        // target is another worktree of the same project, and `--force` has to
        // ask it what it tracks before deleting anything.
        for root in [&source, &target] {
            let out = crate::utils::git_command_at(root)
                .args(["init", "-q", "-b", "main"])
                .output()
                .expect("git init");
            assert!(out.status.success(), "git init failed");
            write(&root.join(".gitignore"), ignore_rules.as_bytes());
        }
        (tmp, source, target)
    }

    fn config(paths: &[&str]) -> ResolvedCopyConfig {
        ResolvedCopyConfig {
            paths: paths.iter().map(|p| (*p).to_string()).collect(),
            fallback: CopyFallback::Copy,
            max_size_bytes: None,
            max_size_unparsed: None,
        }
    }

    fn run(source: &Path, target: &Path, config: &ResolvedCopyConfig) -> CopyPathsResult {
        copy_entries(source, target, config, false, &mut NullSink)
    }

    /// The skip reason of a single-outcome result, or a panic naming what it
    /// actually was.
    fn sole_skip(result: &CopyPathsResult) -> SkipReason {
        match result.outcomes.as_slice() {
            [CopyOutcome::Skipped { reason, .. }] => reason.clone(),
            other => panic!("expected one skip, got {other:?}"),
        }
    }

    // ── read_copy_config ─────────────────────────────────────────────────

    #[test]
    fn read_copy_config_normalizes_both_yaml_forms() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // No config file at all.
        assert_eq!(read_copy_config(root), None);

        // A config without the key.
        write(&root.join("daft.yml"), b"hooks: {}\n");
        assert_eq!(read_copy_config(root), None);

        // Bare list: trailing slashes stripped, knobs defaulted.
        write(
            &root.join("daft.yml"),
            b"copy:\n  - target/\n  - node_modules/\n",
        );
        assert_eq!(
            read_copy_config(root),
            Some(ResolvedCopyConfig {
                paths: vec!["target".into(), "node_modules".into()],
                fallback: CopyFallback::Copy,
                max_size_bytes: None,
                max_size_unparsed: None,
            })
        );

        // Full map: knobs honored, `max_size` parsed to bytes.
        write(
            &root.join("daft.yml"),
            b"copy:\n  paths: [target/]\n  fallback: skip\n  max_size: 5GB\n",
        );
        assert_eq!(
            read_copy_config(root),
            Some(ResolvedCopyConfig {
                paths: vec!["target".into()],
                fallback: CopyFallback::Skip,
                max_size_bytes: Some(5 * 1024 * 1024 * 1024),
                max_size_unparsed: None,
            })
        );
    }

    #[test]
    fn read_copy_config_treats_an_empty_section_as_absent() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        write(&root.join("daft.yml"), b"copy: []\n");
        assert_eq!(read_copy_config(root), None);

        // Entries that normalize away are dropped, and an entry list that is
        // *entirely* slashes is indistinguishable from no section at all.
        write(&root.join("daft.yml"), b"copy:\n  - \"/\"\n  - target/\n");
        assert_eq!(
            read_copy_config(root).unwrap().paths,
            vec!["target".to_string()]
        );
    }

    #[test]
    fn read_copy_config_degrades_an_unparseable_max_size_to_uncapped() {
        // The validator already reported this as a config error; capping every
        // entry at zero would turn one typo into a silently inert stage.
        let tmp = TempDir::new().unwrap();
        write(
            &tmp.path().join("daft.yml"),
            b"copy:\n  paths: [target]\n  max_size: five gigabytes\n",
        );
        assert_eq!(read_copy_config(tmp.path()).unwrap().max_size_bytes, None);
    }

    #[test]
    fn read_copy_config_reads_through_the_merged_loader() {
        // The whole reason `copy:` does not reuse `shared:`'s raw single-file
        // read: a visitor's daft.local.yml must be able to declare the section,
        // and an overlay restating it replaces paths AND knobs wholesale.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(
            &root.join("daft.yml"),
            b"copy:\n  paths: [target]\n  fallback: skip\n",
        );
        write(&root.join("daft.local.yml"), b"copy:\n  - node_modules/\n");

        let resolved = read_copy_config(root).unwrap();
        assert_eq!(resolved.paths, vec!["node_modules".to_string()]);
        assert_eq!(
            resolved.fallback,
            CopyFallback::Copy,
            "the base's fallback: skip must not leak through a wholesale replace"
        );
    }

    #[test]
    fn read_copy_config_is_none_for_an_unparseable_config() {
        // Loud config diagnostics are the loader's and validator's job; the
        // copy stage stays silent and does nothing.
        let tmp = TempDir::new().unwrap();
        write(&tmp.path().join("daft.yml"), b"copy: [unterminated\n");
        assert_eq!(read_copy_config(tmp.path()), None);
    }

    // ── expand_entries ───────────────────────────────────────────────────

    #[test]
    fn expand_entries_passes_literals_through_untouched() {
        let (_tmp, source, _target) = fs_fixture();
        // Existence is deliberately not consulted — `copy_entries` owns that
        // decision so a missing cache reports as its own outcome.
        assert_eq!(
            expand_entries(&source, "target"),
            vec!["target".to_string()]
        );
        assert_eq!(
            expand_entries(&source, "crates/core/target"),
            vec!["crates/core/target".to_string()]
        );
    }

    #[test]
    fn expand_entries_matches_globs_against_the_source_tree() {
        let (_tmp, source, _target) = fs_fixture();
        write(&source.join("web/dist/app.js"), b"//");
        write(&source.join("api/dist/server.js"), b"//");
        write(&source.join("docs/README.md"), b"#");

        assert_eq!(
            expand_entries(&source, "**/dist"),
            vec!["api/dist".to_string(), "web/dist".to_string()],
            "results are repo-root-relative and deterministically ordered"
        );
    }

    #[test]
    fn expand_entries_prunes_descent_below_a_match() {
        // `**/dist` must report `web/dist`, never `web/dist/assets` too:
        // copying the parent already carries the child, and a per-file result
        // set would make the row's annotation meaningless.
        let (_tmp, source, _target) = fs_fixture();
        write(&source.join("web/dist/assets/dist/deep.js"), b"//");

        assert_eq!(
            expand_entries(&source, "**/dist"),
            vec!["web/dist".to_string()]
        );
    }

    #[test]
    fn expand_entries_returns_nothing_when_a_glob_matches_nothing() {
        let (_tmp, source, _target) = fs_fixture();
        write(&source.join("src/main.rs"), b"fn main() {}");
        assert!(expand_entries(&source, "**/dist").is_empty());
    }

    #[test]
    fn expand_entries_finds_hidden_and_gitignored_shaped_entries() {
        // Every ignore filter is off by design: `copy:` entries are gitignored
        // by definition, and `.venv` is hidden.
        let (_tmp, source, _target) = fs_fixture();
        write(&source.join(".gitignore"), b"*\n");
        write(&source.join(".venv/bin/python"), b"#!");
        write(&source.join("svc/.venv/bin/python"), b"#!");

        assert_eq!(
            expand_entries(&source, "**/.venv"),
            vec![".venv".to_string(), "svc/.venv".to_string()]
        );
    }

    #[test]
    fn expand_entries_never_descends_into_the_git_directory() {
        let (_tmp, source, _target) = fs_fixture();
        write(&source.join(".git/objects/dist/pack"), b"x");
        write(&source.join("web/dist/app.js"), b"//");

        assert_eq!(
            expand_entries(&source, "**/dist"),
            vec!["web/dist".to_string()]
        );
    }

    // ── copy_entries: the gitignored-only invariant ──────────────────────

    #[test]
    fn copy_entries_copies_a_gitignored_directory() {
        let (_tmp, source, target) = repo_fixture("/target\n");
        write(&source.join("target/debug/app"), b"binary");
        write(&source.join("target/.rustc_info.json"), b"{}");

        let result = run(&source, &target, &config(&["target"]));

        let [
            CopyOutcome::Copied {
                entry,
                matches,
                bytes,
                ..
            },
        ] = result.outcomes.as_slice()
        else {
            panic!("expected one copy, got {:?}", result.outcomes);
        };
        assert_eq!(entry, "target");
        assert_eq!(*matches, 1, "a literal entry is one match");
        assert_eq!(*bytes, b"binary".len() as u64 + b"{}".len() as u64);
        assert_eq!(
            fs::read(target.join("target/debug/app")).unwrap(),
            b"binary"
        );
        assert!(
            !target.join("target").is_symlink(),
            "`copy:` gives the worktree its own replica, never a link"
        );
    }

    #[test]
    fn copy_entries_refuses_a_tracked_entry() {
        // Nothing in .gitignore, so `src` is plainly visible to git.
        let (_tmp, source, target) = repo_fixture("");
        write(&source.join("src/main.rs"), b"fn main() {}");

        let result = run(&source, &target, &config(&["src"]));

        assert_sole_skip!(result, SkipReason::NotIgnored { .. });
        assert!(
            !target.join("src").exists(),
            "a refused entry leaves nothing behind"
        );
    }

    #[test]
    fn copy_entries_refuses_a_force_added_file_under_an_ignored_directory() {
        // The invariant's second half: the directory is ignored, but git is
        // managing a file inside it, so copying would duplicate tracked
        // content into the new worktree.
        let (_tmp, source, target) = repo_fixture("/node_modules\n");
        write(&source.join("node_modules/pkg/index.js"), b"//");
        let out = crate::utils::git_command_at(&source)
            .args(["add", "-f", "node_modules/pkg/index.js"])
            .output()
            .expect("git add");
        assert!(out.status.success());

        let result = run(&source, &target, &config(&["node_modules"]));

        assert_sole_skip!(result, SkipReason::NotIgnored { .. });
        assert!(!target.join("node_modules").exists());
    }

    #[test]
    fn copy_entries_refuses_an_entry_that_reaches_outside_the_worktree() {
        // A config is not a capability to write anywhere. Nothing that climbs
        // out of the source worktree gets copied, whatever git would say
        // about it.
        let (tmp, source, target) = repo_fixture("");
        write(&tmp.path().join("outside/secret"), b"not yours");
        // `sub/` exists so the second spelling resolves on disk — otherwise it
        // would be refused as a missing source and prove nothing.
        fs::create_dir_all(source.join("sub")).unwrap();

        for escape in ["../outside", "sub/../../outside"] {
            let result = run(&source, &target, &config(&[escape]));
            assert_sole_skip!(result, SkipReason::Uncontained { .. });
        }
        assert!(!target.join("outside").exists());
        assert!(!tmp.path().join("target/outside").exists());

        assert!(containment_violation("/etc").is_some());
        assert!(containment_violation("target/debug").is_none());
    }

    // ── copy_entries: outcomes ───────────────────────────────────────────

    #[test]
    fn copy_entries_skips_an_entry_that_was_never_built() {
        let (_tmp, source, target) = repo_fixture("/target\n");
        let result = run(&source, &target, &config(&["target"]));
        assert_eq!(sole_skip(&result), SkipReason::NoSource);
    }

    #[test]
    fn copy_entries_skips_a_glob_that_matched_nothing() {
        let (_tmp, source, target) = repo_fixture("dist\n");
        let result = run(&source, &target, &config(&["**/dist"]));
        assert_eq!(sole_skip(&result), SkipReason::NoMatches);
    }

    #[test]
    fn copy_entries_is_idempotent_and_force_re_copies() {
        // Idempotence is what makes `daft warm` twice a no-op, and what keeps
        // the creation stage from clobbering work a post-create hook already
        // did. `--force` is the deliberate way out.
        let (_tmp, source, target) = repo_fixture("/target\n");
        write(&source.join("target/app"), b"v1");

        let first = run(&source, &target, &config(&["target"]));
        assert!(matches!(
            first.outcomes.as_slice(),
            [CopyOutcome::Copied { .. }]
        ));

        write(&source.join("target/app"), b"v2");
        let second = run(&source, &target, &config(&["target"]));
        assert_eq!(sole_skip(&second), SkipReason::DestinationExists);
        assert_eq!(
            fs::read(target.join("target/app")).unwrap(),
            b"v1",
            "a skip must not touch what is already there"
        );

        let forced = copy_entries(&source, &target, &config(&["target"]), true, &mut NullSink);
        assert!(matches!(
            forced.outcomes.as_slice(),
            [CopyOutcome::Copied { .. }]
        ));
        assert_eq!(fs::read(target.join("target/app")).unwrap(), b"v2");
    }

    #[test]
    fn copy_entries_refuses_to_copy_a_worktree_into_itself() {
        // `--force` clears each destination before copying. When source and
        // target are the same directory that clears the source, so a mis-typed
        // `daft warm . --from .` would delete the caches it was asked to
        // replicate. The engine refuses instead of trusting the caller.
        let (_tmp, source, _target) = repo_fixture("/target\n");
        write(&source.join("target/app"), b"precious");

        let result = copy_entries(&source, &source, &config(&["target"]), true, &mut NullSink);

        assert_eq!(sole_skip(&result), SkipReason::SameWorktree);
        assert_eq!(fs::read(source.join("target/app")).unwrap(), b"precious");
    }

    #[test]
    fn copy_entries_copies_the_matches_a_partially_present_glob_still_needs() {
        // One row per entry, but a glob whose destinations are half there must
        // still fill the other half — skipping wholesale would leave those
        // caches permanently missing after one interrupted run.
        let (_tmp, source, target) = repo_fixture("dist\n");
        write(&source.join("web/dist/app.js"), b"web");
        write(&source.join("api/dist/server.js"), b"api");
        write(&target.join("web/dist/app.js"), b"already here");

        let result = run(&source, &target, &config(&["**/dist"]));

        let [CopyOutcome::Copied { matches, .. }] = result.outcomes.as_slice() else {
            panic!("expected one copy, got {:?}", result.outcomes);
        };
        assert_eq!(*matches, 1, "only the missing half was copied");
        assert_eq!(
            fs::read(target.join("web/dist/app.js")).unwrap(),
            b"already here"
        );
        assert_eq!(fs::read(target.join("api/dist/server.js")).unwrap(), b"api");
    }

    #[test]
    fn copy_entries_handles_file_and_symlink_entries_not_only_directories() {
        let (_tmp, source, target) = repo_fixture("/cache.bin\n/link\n");
        write(&source.join("cache.bin"), b"blob");
        write(&source.join("real/inner"), b"inner");
        #[cfg(unix)]
        std::os::unix::fs::symlink("real", source.join("link")).unwrap();

        #[cfg(unix)]
        let entries = config(&["cache.bin", "link"]);
        #[cfg(not(unix))]
        let entries = config(&["cache.bin"]);

        let result = run(&source, &target, &entries);

        assert!(
            result
                .outcomes
                .iter()
                .all(|o| matches!(o, CopyOutcome::Copied { .. })),
            "{:?}",
            result.outcomes
        );
        assert_eq!(fs::read(target.join("cache.bin")).unwrap(), b"blob");
        #[cfg(unix)]
        {
            // Recreated, not dereferenced: a cache that *is* a link stays one.
            assert!(target.join("link").is_symlink());
            // And it still resolves. `real/` lives outside the copied entry
            // (the entry is the link), so the text is re-based onto the
            // destination's position rather than copied verbatim into a
            // dangling link. Resolution is the invariant; the exact text is
            // whatever reaching the same target requires.
            assert_eq!(
                fs::read(target.join("link/inner")).unwrap(),
                b"inner",
                "the link resolves to the same tree, via {:?}",
                fs::read_link(target.join("link")).unwrap()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn copy_entries_isolates_a_failing_entry_from_the_rest() {
        // Warn, never abort: the worktree the user asked for exists either
        // way, so one broken cache must cost exactly one row and the entries
        // after it must still run.
        use std::os::unix::fs::PermissionsExt;

        let (_tmp, source, target) = repo_fixture("/target\n/web\n");
        write(&source.join("web/dist/app.js"), b"web");
        write(&source.join("target/app"), b"binary");
        // An unreadable subtree INSIDE the entry: the copy starts, walks into
        // it, and fails partway. (A destination-side permission problem is
        // caught earlier now, by the reflink probe, without destroying
        // anything — a different test.)
        let locked = source.join("web/dist/locked");
        fs::create_dir_all(&locked).unwrap();
        write(&locked.join("inner"), b"x");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let premise_holds = fs::read_dir(&locked).is_err();

        let result = run(&source, &target, &config(&["web/dist", "target"]));

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        if !premise_holds {
            return;
        }

        let [failed, copied] = result.outcomes.as_slice() else {
            panic!("expected two outcomes, got {:?}", result.outcomes);
        };
        let CopyOutcome::Failed { entry, detail } = failed else {
            panic!("expected a failure, got {failed:?}");
        };
        assert_eq!(entry, "web/dist");
        assert!(!detail.is_empty(), "a failure has to say what broke");
        assert!(
            matches!(copied, CopyOutcome::Copied { entry, .. } if entry == "target"),
            "the entry after the failure still ran: {copied:?}"
        );
        assert_eq!(fs::read(target.join("target/app")).unwrap(), b"binary");
    }

    #[test]
    fn copy_entries_reports_a_destination_of_the_wrong_shape_rather_than_calling_it_present() {
        // Existence alone is not "already present". A path declared in both
        // `shared:` and `copy:` leaves a symlink where the copy needs a
        // directory; a stale file can sit where a tree belongs. Either would
        // wear a dim `already present` forever, and the cache would never
        // arrive.
        let (_tmp, source, target) = repo_fixture("/cache\n");
        write(&source.join("cache/a.bin"), b"payload");
        write(&target.join("cache"), b"a file, not the tree");

        let result = run(&source, &target, &config(&["cache"]));

        assert_sole_skip!(result, SkipReason::DestinationConflict { .. });
        assert_eq!(
            fs::read(target.join("cache")).unwrap(),
            b"a file, not the tree",
            "a conflict is reported, never silently replaced"
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_entries_reports_a_symlinked_destination_rather_than_calling_it_present() {
        // The `shared:` + `copy:` double-declaration leak, concretely: the
        // shared stage linked the path, and the copy stage must not report a
        // link as a finished replica.
        let (_tmp, source, target) = repo_fixture("/cache\n");
        write(&source.join("cache/a.bin"), b"payload");
        fs::create_dir_all(target.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink("elsewhere", target.join("cache")).unwrap();

        assert_sole_skip!(
            run(&source, &target, &config(&["cache"])),
            SkipReason::DestinationConflict { .. }
        );
        assert!(target.join("cache").is_symlink(), "the link is left alone");
    }

    #[cfg(unix)]
    #[test]
    fn copy_entries_clears_a_partial_destination_when_an_entry_fails() {
        // `copy_dir` creates the destination and fills it as it walks, so a
        // mid-walk failure leaves a partial tree. Left there it is
        // indistinguishable from a finished copy: every later run would report
        // `already present` and skip, masking a half-copied cache forever —
        // and warn-never-abort means nothing would ever raise about it.
        use std::os::unix::fs::PermissionsExt;

        let (_tmp, source, target) = repo_fixture("/target\n");
        write(&source.join("target/keep.txt"), b"ok");
        let locked = source.join("target/locked");
        fs::create_dir_all(&locked).unwrap();
        write(&locked.join("inner.txt"), b"unreadable");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        // chmod 000 does not stop a root euid, and some filesystems ignore
        // perms. If this process can still read the directory the premise is
        // void — skip rather than assert falsely.
        let premise_holds = fs::read_dir(&locked).is_err();

        let first = run(&source, &target, &config(&["target"]));
        let destination_survived = target.join("target").exists();
        // A rerun while the cause persists: it must retry and fail again, not
        // find leftovers and call them warm.
        let second = run(&source, &target, &config(&["target"]));

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        if !premise_holds {
            return;
        }

        assert!(
            matches!(first.outcomes.as_slice(), [CopyOutcome::Failed { .. }]),
            "{:?}",
            first.outcomes
        );
        assert!(
            !destination_survived,
            "a failed copy must not leave a partial tree behind"
        );
        assert!(
            matches!(second.outcomes.as_slice(), [CopyOutcome::Failed { .. }]),
            "the rerun retried instead of reporting `already present`: {:?}",
            second.outcomes
        );

        // With the cause gone, the retry that the cleanup made possible
        // actually completes — which is the whole point of clearing it.
        let third = run(&source, &target, &config(&["target"]));
        assert!(
            matches!(third.outcomes.as_slice(), [CopyOutcome::Copied { .. }]),
            "{:?}",
            third.outcomes
        );
        assert_eq!(fs::read(target.join("target/keep.txt")).unwrap(), b"ok");
        assert_eq!(
            fs::read(target.join("target/locked/inner.txt")).unwrap(),
            b"unreadable"
        );
    }

    #[test]
    fn copy_entries_produces_exactly_one_outcome_per_declared_entry() {
        // The join back to the plan: the rail planned one row per declaration,
        // so the outcomes have to answer one per declaration, in order.
        let (_tmp, source, target) = repo_fixture("dist\n/target\n");
        write(&source.join("web/dist/a.js"), b"a");
        write(&source.join("api/dist/b.js"), b"b");
        write(&source.join("target/app"), b"x");

        let declared = config(&["**/dist", "target", "never-built"]);
        let result = run(&source, &target, &declared);

        assert_eq!(
            result
                .outcomes
                .iter()
                .map(CopyOutcome::entry)
                .collect::<Vec<_>>(),
            ["**/dist", "target", "never-built"]
        );
        let [CopyOutcome::Copied { matches, .. }, ..] = result.outcomes.as_slice() else {
            panic!("expected the glob to copy, got {:?}", result.outcomes);
        };
        assert_eq!(*matches, 2, "the fan-out lands in the annotation, not rows");
        assert_eq!(result.copied_count(), 2);
    }

    // ── plan_method: the reflink / fallback / size decision ──────────────

    #[test]
    fn gate_byte_copies_never_size_gates_a_reflink() {
        // Cloning blocks is free, so a cap written to stop an expensive byte
        // copy must not refuse it — whatever the entry weighs.
        assert_eq!(
            gate_byte_copies(false, 100 * 1024, CopyFallback::Copy, Some(1)),
            None
        );
        assert_eq!(
            gate_byte_copies(false, u64::MAX, CopyFallback::Skip, Some(0)),
            None,
            "fallback describes what happens when reflink is unavailable"
        );
    }

    #[test]
    fn gate_byte_copies_honors_fallback_skip_when_reflink_is_unavailable() {
        assert_eq!(
            gate_byte_copies(true, 10, CopyFallback::Skip, None),
            Some(SkipReason::NoReflink)
        );
    }

    #[test]
    fn gate_byte_copies_gates_the_byte_copy_on_max_size() {
        assert_eq!(
            gate_byte_copies(true, 2048, CopyFallback::Copy, Some(1024)),
            Some(SkipReason::TooLarge {
                size_bytes: 2048,
                limit_bytes: 1024,
            })
        );
        // At the limit, not over it.
        assert_eq!(
            gate_byte_copies(true, 1024, CopyFallback::Copy, Some(1024)),
            None
        );
        assert_eq!(
            gate_byte_copies(true, u64::MAX, CopyFallback::Copy, None),
            None,
            "no cap means no cap"
        );
    }

    // ── Reporting ────────────────────────────────────────────────────────

    #[test]
    fn report_copy_results_maps_outcomes_and_sweeps_planned_leftovers() {
        use crate::core::RecordingStageSink;
        use crate::core::stage::StageEvent;

        let result = CopyPathsResult {
            outcomes: vec![
                CopyOutcome::Copied {
                    entry: "target".into(),
                    method: CopyMethod::Reflinked,
                    matches: 1,
                    bytes: 1_288_490_189,
                    elapsed: Duration::from_millis(300),
                    unreadable: 0,
                },
                CopyOutcome::Skipped {
                    entry: "node_modules".into(),
                    reason: SkipReason::NoSource,
                    unreadable: 0,
                },
                CopyOutcome::Skipped {
                    entry: ".venv".into(),
                    reason: SkipReason::DestinationExists,
                    unreadable: 0,
                },
                CopyOutcome::Skipped {
                    entry: "**/dist".into(),
                    reason: SkipReason::NoMatches,
                    unreadable: 0,
                },
                CopyOutcome::Skipped {
                    entry: "src".into(),
                    reason: SkipReason::NotIgnored {
                        offender: "src".into(),
                    },
                    unreadable: 0,
                },
                CopyOutcome::Skipped {
                    entry: "big".into(),
                    reason: SkipReason::NoReflink,
                    unreadable: 0,
                },
                CopyOutcome::Skipped {
                    entry: "huge".into(),
                    reason: SkipReason::TooLarge {
                        size_bytes: 2_254_857_830,
                        limit_bytes: 1024 * 1024 * 1024,
                    },
                    unreadable: 0,
                },
                CopyOutcome::Failed {
                    entry: "broken".into(),
                    detail: "permission denied".into(),
                },
            ],
        };
        let planned = vec!["target".to_string(), "dropped".to_string()];
        let mut sink = RecordingStageSink::default();
        report_copy_results(&result, &planned, &mut sink);

        let events: Vec<_> = sink
            .events
            .iter()
            .map(|(k, e)| (k.scope.clone().unwrap(), e.clone()))
            .collect();
        assert_eq!(
            events,
            vec![
                (
                    "target".into(),
                    StageEvent::Completed {
                        annotation: Some("1.2 GB · reflinked · 0.3s".into()),
                    }
                ),
                // Nothing to do: dim receipt rows.
                (
                    "node_modules".into(),
                    StageEvent::SkippedExpected {
                        reason: "nothing to copy yet".into(),
                    }
                ),
                (
                    ".venv".into(),
                    StageEvent::SkippedExpected {
                        reason: "already present".into(),
                    }
                ),
                (
                    "**/dist".into(),
                    StageEvent::SkippedExpected {
                        reason: "matched nothing".into(),
                    }
                ),
                // The config asked for something that did not happen: yellow.
                (
                    "src".into(),
                    StageEvent::SkippedAttention {
                        reason: "must be gitignored — tracked content is never copied".into(),
                    }
                ),
                (
                    "big".into(),
                    StageEvent::SkippedAttention {
                        reason: "no reflink support — fallback: skip".into(),
                    }
                ),
                (
                    "huge".into(),
                    StageEvent::SkippedAttention {
                        reason: "2.1 GB — over the 1 GB max_size".into(),
                    }
                ),
                // Never a `Failed` face — the worktree the user asked for
                // exists; only a cache did not.
                (
                    "broken".into(),
                    StageEvent::SkippedAttention {
                        reason: "failed — permission denied".into(),
                    }
                ),
                // Planned but no outcome at all: the row is removed.
                ("dropped".into(), StageEvent::SkippedSilent),
            ]
        );
    }

    /// The no-live-region path. Quiet runs, pipes, CI, and the YAML runner's
    /// non-TTY sandbox never build a rail, so without these lines a tracked
    /// entry or a failed copy would leave no trace at all — warn-never-abort
    /// turning into warn-invisible exactly where nobody is watching a screen.
    #[test]
    fn legacy_copy_stage_line_speaks_for_completions_and_attention_skips() {
        use crate::core::stage::{StageEvent, StageId, StepKey};

        let key = StepKey::scoped(StageId::CopyPath, "cache");
        let line = |event| legacy_copy_stage_line(&key, &event, false);

        assert_eq!(
            line(StageEvent::Completed {
                annotation: Some("1.2 GB · reflinked".into()),
            }),
            Some("Copied cache (1.2 GB · reflinked)".to_string())
        );
        assert_eq!(
            line(StageEvent::Completed { annotation: None }),
            Some("Copied cache".to_string())
        );

        // The phrase already quotes the entry: prefixing it again would stutter.
        assert_eq!(
            line(StageEvent::SkippedAttention {
                reason: skip_phrase(
                    "cache",
                    &SkipReason::NotIgnored {
                        offender: "cache".into(),
                    },
                ),
            }),
            Some(
                "warning: cache: must be gitignored — tracked content is never copied".to_string()
            )
        );
        // The phrase does not name itself, and a flat line has no row label to
        // lean on — so it gets one.
        assert_eq!(
            line(StageEvent::SkippedAttention {
                reason: skip_phrase("cache", &SkipReason::NoReflink),
            }),
            Some("warning: cache: no reflink support — fallback: skip".to_string())
        );
        assert_eq!(
            line(StageEvent::SkippedAttention {
                reason: failure_phrase("disk full"),
            }),
            Some("warning: cache: failed — disk full".to_string())
        );

        // Expected skips and silent resolutions stay quiet, exactly as the
        // shared-file fallback does: a receipt per declared entry is the rail's
        // job, and a log carries only what changed or went wrong.
        for event in [
            StageEvent::SkippedExpected {
                reason: skip_phrase("cache", &SkipReason::DestinationExists),
            },
            StageEvent::SkippedSilent,
            StageEvent::Started,
        ] {
            assert_eq!(line(event.clone()), None, "{event:?}");
        }

        // Another stage's rows are not this renderer's business.
        assert_eq!(
            legacy_copy_stage_line(
                &StepKey::scoped(StageId::SharedFile, ".env"),
                &StageEvent::Completed { annotation: None },
                false,
            ),
            None
        );
    }

    #[test]
    fn report_copy_results_is_silent_when_nothing_was_planned() {
        use crate::core::RecordingStageSink;

        let mut sink = RecordingStageSink::default();
        report_copy_results(&CopyPathsResult::default(), &[], &mut sink);
        assert!(sink.events.is_empty());
    }

    #[test]
    fn copied_annotation_drops_the_noise_and_keeps_the_facts() {
        // One match: the row's label already names it.
        assert_eq!(
            copied_annotation(
                1,
                1024 * 1024,
                CopyMethod::Reflinked,
                Duration::from_millis(1),
                0
            ),
            "1 MB · reflinked",
            "a duration that would round to 0.0s says nothing"
        );
        assert_eq!(
            copied_annotation(3, 1024, CopyMethod::Copied, Duration::from_millis(1200), 0),
            "3 paths · 1 KB · copied · 1.2s",
            "the count is expanded PATHS — an entry can name a file"
        );
    }

    #[test]
    fn format_bytes_leaves_whole_numbers_whole() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1 GB");
        assert_eq!(format_bytes(2_254_857_830), "2.1 GB");
        assert_eq!(format_bytes(1024u64.pow(4)), "1 TB");
    }

    /// The full phrase set, spelled out, on both surfaces.
    ///
    /// These strings are a contract, not an implementation detail: the creation
    /// rail renders them as row annotations, `daft warm` renders them as flat
    /// lines, and a YAML scenario asserts on the tracked-entry one. Pinning
    /// them is what makes an accidental reword a failing test rather than two
    /// silently diverged surfaces.
    #[test]
    fn skip_phrases_are_the_pinned_shared_wording() {
        let cases = [
            (SkipReason::NoSource, "nothing to copy yet"),
            (SkipReason::DestinationExists, "already present"),
            (SkipReason::NoMatches, "matched nothing"),
            (
                SkipReason::NotIgnored {
                    offender: "cache".into(),
                },
                "must be gitignored — tracked content is never copied",
            ),
            (
                SkipReason::TargetTracked {
                    offender: "cache".into(),
                },
                "is tracked in this worktree — refusing to replace it",
            ),
            (
                SkipReason::Unclassifiable {
                    offender: "cache".into(),
                    detail: "git check-ignore exited 128".into(),
                },
                "could not be classified by git — git check-ignore exited 128",
            ),
            // The destination's own probes speak about the destination: a
            // dst-side git failure must not send anyone to inspect the source.
            (
                SkipReason::DestinationUnclassifiable {
                    offender: "cache".into(),
                    detail: "git ls-files exited 128".into(),
                },
                "the destination could not be classified by git — git ls-files exited 128",
            ),
            (
                SkipReason::DestinationUnreadable {
                    path: "cache".into(),
                    detail: "Permission denied (os error 13)".into(),
                },
                "the destination could not be read — Permission denied (os error 13)",
            ),
            (
                SkipReason::Uncontained {
                    offender: "cache".into(),
                    detail: "resolves outside the worktree — not copied".into(),
                },
                "resolves outside the worktree — not copied",
            ),
            // The absolute-path refusal carries its own remedy: `/target` is
            // what cargo writes into `.gitignore`, so this is the likeliest
            // way an absolute entry appears and the likeliest place a bare
            // "is an absolute path" would leave someone stuck.
            (
                SkipReason::Uncontained {
                    offender: "cache".into(),
                    detail: absolute_entry_hint("/target"),
                },
                "is an absolute path — drop the leading '/' to anchor at the worktree root \
                 (write 'target')",
            ),
            (
                SkipReason::SourceUnreadable {
                    path: "cache".into(),
                    detail: "permission denied".into(),
                },
                "could not be read — permission denied",
            ),
            (
                SkipReason::DestinationConflict {
                    path: "cache".into(),
                    detail: "a symlink where the source is a directory".into(),
                },
                "already present as a symlink where the source is a directory — not replaced",
            ),
            (
                SkipReason::SameWorktree,
                "source and target are the same worktree",
            ),
            (SkipReason::NoReflink, "no reflink support — fallback: skip"),
            // Never the sentence above: the filesystem was never asked, and on
            // APFS blaming it sends the user to a knob that is not the problem.
            (
                SkipReason::ReflinkUnprobeable {
                    path: "cache".into(),
                    detail: "could not write a probe file in /w/feature: read-only".into(),
                },
                "could not be tested for reflink support — could not write a probe file \
                 in /w/feature: read-only",
            ),
            (
                SkipReason::TooLarge {
                    size_bytes: 2_254_857_830,
                    limit_bytes: 1024 * 1024 * 1024,
                },
                "2.1 GB — over the 1 GB max_size",
            ),
        ];

        for (reason, expected) in &cases {
            let phrase = skip_phrase("cache", reason);
            assert_eq!(&phrase, expected, "{reason:?}");
            // `CopyPath` is exempt from the timeline's `skipped — ` prefix, so
            // a phrase written to follow one would render as a fragment.
            assert!(
                !phrase.starts_with("skipped"),
                "{reason:?} must not restate the prefix it is exempt from: {phrase}"
            );
            // NEITHER surface may print the entry twice. On the rail the row's
            // label already IS the entry, so the bare phrase must not name it;
            // on a flat line the entry appears exactly once, from the qualifier.
            assert!(
                !phrase.contains("cache"),
                "{reason:?} stutters against its own row label: {phrase}"
            );
            assert_eq!(
                qualified_phrase("cache", &phrase).matches("cache").count(),
                1,
                "{reason:?} names the entry more than once on a flat line"
            );
        }

        // The refusal still carries the word a reader searches for.
        assert!(
            skip_phrase(
                "cache",
                &SkipReason::NotIgnored {
                    offender: "cache".into()
                }
            )
            .contains("tracked")
        );
        // A glob names the ONE match that offended — the only way a single row
        // can explain a thirty-way expansion.
        assert_eq!(
            skip_phrase(
                "**/dist",
                &SkipReason::NotIgnored {
                    offender: "web/dist".into()
                }
            ),
            "'web/dist' must be gitignored — tracked content is never copied"
        );
        assert_eq!(failure_phrase("disk full"), "failed — disk full");
        assert_eq!(
            qualified_phrase("cache", "already present"),
            "cache: already present"
        );
    }

    #[test]
    fn push_copy_section_plans_anchor_and_entry_labeled_rows() {
        use crate::core::stage::{Row, StageId};

        let mut rows = Vec::new();
        push_copy_section(&mut rows, &[]);
        assert!(rows.is_empty(), "nothing declared, nothing planned");

        push_copy_section(&mut rows, &["target".to_string(), "**/dist".to_string()]);
        assert!(matches!(&rows[0], Row::Group { label } if label == "copied paths"));

        let Row::Step(spec) = &rows[1] else {
            panic!("expected step row");
        };
        assert_eq!(spec.key.id, StageId::CopyPath);
        assert_eq!(spec.key.scope.as_deref(), Some("target"));
        assert_eq!(spec.label.as_deref(), Some("target"));

        // A glob entry is planned as itself: one row per DECLARATION, never
        // per expanded match (the plan face must stay walk-free).
        let Row::Step(spec) = &rows[2] else {
            panic!("expected step row");
        };
        assert_eq!(spec.key.scope.as_deref(), Some("**/dist"));
        assert_eq!(spec.label.as_deref(), Some("**/dist"));

        assert!(
            matches!(rows.last(), Some(Row::EndGroup)),
            "the section closes its span so following rows stay ungrouped"
        );
        assert_eq!(rows.len(), 4);
    }

    #[test]
    fn copy_paths_result_summarizes_its_outcomes() {
        let result = CopyPathsResult {
            outcomes: vec![
                CopyOutcome::Copied {
                    entry: "target".into(),
                    method: CopyMethod::Reflinked,
                    matches: 1,
                    bytes: 1_024,
                    elapsed: Duration::from_millis(3),
                    unreadable: 0,
                },
                CopyOutcome::Skipped {
                    entry: "node_modules".into(),
                    reason: SkipReason::DestinationExists,
                    unreadable: 0,
                },
                CopyOutcome::Failed {
                    entry: ".venv".into(),
                    detail: "permission denied".into(),
                },
            ],
        };
        assert!(!result.is_empty());
        assert_eq!(result.copied_count(), 1);
        assert_eq!(result.copied_bytes(), 1_024);
        assert_eq!(
            result
                .outcomes
                .iter()
                .map(CopyOutcome::entry)
                .collect::<Vec<_>>(),
            ["target", "node_modules", ".venv"],
            "entry() is the join back to the planned row's scope"
        );
        assert!(CopyPathsResult::default().is_empty());
    }

    // ─────────────────────────────────────────────────────────────────────
    // Coverage pass: the contract clauses above that no test yet held to
    // account — decision-matrix cells no single filesystem can reach, the
    // "first condition that holds wins" ordering where two of them hold at
    // once, and the safety guards whose failure mode is destructive rather
    // than merely wrong.
    // ─────────────────────────────────────────────────────────────────────

    /// Run one git command in a fixture repo, with identity supplied by
    /// environment (never config — CLAUDE.md forbids touching git config) and
    /// both pipes captured so nothing leaks into the test output.
    fn git(dir: &Path, args: &[&str]) {
        let out = crate::utils::git_command_at(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .expect("git command");
        assert!(out.status.success(), "git {args:?} failed");
    }

    /// The names directly inside `dir`, sorted — for asserting what a copy
    /// (or a refusal) left behind.
    /// What `dir` contains, minus the repository scaffolding every fixture
    /// worktree carries — these tests ask what the COPY put there.
    fn entries_of(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .map(|d| {
                d.filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .filter(|name| name != ".git" && name != ".gitignore")
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    // ── read_copy_config: the remaining shapes ───────────────────────────

    #[test]
    fn read_copy_config_treats_a_full_form_with_no_paths_as_absent() {
        // The bare-list empty case is covered above; the map form has its own
        // route to the same place, and knobs must not resurrect an empty
        // declaration into a section that plans rows for nothing.
        let tmp = TempDir::new().unwrap();
        write(
            &tmp.path().join("daft.yml"),
            b"copy:\n  paths: []\n  fallback: skip\n  max_size: 1GB\n",
        );
        assert_eq!(read_copy_config(tmp.path()), None);
    }

    #[test]
    fn read_copy_config_parses_a_quoted_byte_count() {
        // `parse_size` takes a bare integer as bytes, and the config field is
        // a String — so the YAML has to quote it. (An *unquoted* integer is a
        // YAML int, which the `Option<String>` field rejects: see the coverage
        // report's note on that spelling.)
        let tmp = TempDir::new().unwrap();
        write(
            &tmp.path().join("daft.yml"),
            b"copy:\n  paths: [target]\n  max_size: \"1048576\"\n",
        );
        assert_eq!(
            read_copy_config(tmp.path()).unwrap().max_size_bytes,
            Some(1024 * 1024)
        );
    }

    // ── expand_entries: the remaining input shapes ───────────────────────

    #[test]
    fn expand_entries_matches_files_not_only_directories() {
        // Nothing in the design says a `copy:` entry names a directory — a
        // single `.sccache.db` or a `*.bin` fan-out is a cache too, and the
        // override matcher has to be asked with the right is_dir answer for
        // those to match at all.
        let (_tmp, source, _target) = fs_fixture();
        write(&source.join("cache/a.bin"), b"a");
        write(&source.join("deep/nested/b.bin"), b"b");
        write(&source.join("cache/notes.txt"), b"not a match");

        assert_eq!(
            expand_entries(&source, "**/*.bin"),
            vec!["cache/a.bin".to_string(), "deep/nested/b.bin".to_string()]
        );
    }

    #[test]
    fn expand_entries_treats_a_metacharacter_free_entry_as_a_path_not_a_pattern() {
        // `dist` is a literal, so it means `<root>/dist` and nothing else —
        // it does not go looking for `web/dist` the way `**/dist` does. The
        // split is `is_glob`, and getting it wrong in either direction (a
        // literal that searches, a pattern that does not) silently changes
        // what every config in the wild means.
        let (_tmp, source, _target) = fs_fixture();
        write(&source.join("web/dist/app.js"), b"//");

        assert_eq!(expand_entries(&source, "dist"), vec!["dist".to_string()]);
        assert_eq!(
            expand_entries(&source, "**/dist"),
            vec!["web/dist".to_string()]
        );
    }

    #[test]
    fn expand_entries_yields_nothing_for_a_pattern_the_glob_compiler_rejects() {
        // `[` opens a character class that is never closed. The entry is a
        // glob by `is_glob`'s reckoning but cannot compile, and an
        // uncompilable pattern must expand to nothing (reported as an expected
        // skip) rather than panic or fail the creation.
        let (_tmp, source, target) = fs_fixture();
        write(&source.join("cache/a.bin"), b"a");

        assert!(expand_entries(&source, "[").is_empty());
        assert_eq!(
            sole_skip(&run(&source, &target, &config(&["["]))),
            SkipReason::NoMatches
        );
    }

    // ── copy_entries: which condition wins when two of them hold ─────────

    #[test]
    fn copy_entries_refuses_a_tracked_entry_before_noticing_the_destination() {
        // Steps 3 and 4 of the documented order both apply here. The refusal
        // has to win: `already present` is a dim "nothing to do" row, and
        // reporting it would hide a config the user has to fix — permanently,
        // because the destination only gets more present from here.
        let (_tmp, source, target) = repo_fixture("");
        write(&source.join("src/main.rs"), b"fn main() {}");
        git(&source, &["add", "src/main.rs"]);
        write(&target.join("src/main.rs"), b"a previous copy");

        let result = run(&source, &target, &config(&["src"]));

        assert_sole_skip!(result, SkipReason::NotIgnored { .. });
        assert_eq!(
            fs::read(target.join("src/main.rs")).unwrap(),
            b"a previous copy",
            "a refusal touches nothing"
        );
    }

    #[test]
    fn copy_entries_reports_a_present_destination_before_weighing_the_entry() {
        // Steps 4 and 8 both apply: the destination is there AND the entry is
        // over the cap. The idempotence skip wins, because there is no copy
        // left to gate — a `2 KB over the 1 KB max_size` warning about a cache
        // that is already warm would send the user to fix nothing.
        let (_tmp, source, target) = repo_fixture("/target\n");
        write(&source.join("target/app"), b"0123456789");
        write(&target.join("target/app"), b"already warm");

        let result = copy_entries(
            &source,
            &target,
            &ResolvedCopyConfig {
                paths: vec!["target".into()],
                fallback: CopyFallback::Copy,
                max_size_bytes: Some(1),
                max_size_unparsed: None,
            },
            false,
            &mut NullSink,
        );

        assert_eq!(sole_skip(&result), SkipReason::DestinationExists);
    }

    #[test]
    fn copy_entries_reports_a_tracked_entry_that_is_gone_as_never_built() {
        // Steps 2 and 3 both apply: git tracks `src`, and it is not on disk.
        // Absence wins — `nothing to copy yet` is the true statement, and the
        // gitignored-only invariant has nothing to protect when there is no
        // content to duplicate.
        let (_tmp, source, target) = repo_fixture("");
        write(&source.join("src/main.rs"), b"fn main() {}");
        git(&source, &["add", "src/main.rs"]);
        fs::remove_dir_all(source.join("src")).unwrap();

        assert_eq!(
            sole_skip(&run(&source, &target, &config(&["src"]))),
            SkipReason::NoSource
        );
    }

    // ── copy_entries: the gitignored-only invariant's remaining causes ───

    #[test]
    fn copy_entries_refuses_an_entry_git_holds_in_its_index() {
        // The companion to the untracked-but-visible refusal above: this one
        // is genuinely `Tracked`, the status the classifier reaches only after
        // its second probe. Both must refuse, and only a fixture that stages
        // the file proves the second one does.
        let (_tmp, source, target) = repo_fixture("");
        write(&source.join("vendor/lib.rs"), b"pub fn f() {}");
        git(&source, &["add", "vendor/lib.rs"]);
        assert_eq!(
            crate::core::git_ignore::git_ignore_status(&source, "vendor"),
            IgnoreStatus::Tracked,
            "fixture precondition: the entry is tracked, not merely visible"
        );

        assert_sole_skip!(
            run(&source, &target, &config(&["vendor"])),
            SkipReason::NotIgnored { .. }
        );
        assert!(!target.join("vendor").exists());
    }

    #[test]
    fn copy_entries_refuses_everything_when_git_cannot_answer_at_all() {
        // The fourth cause, and the one a validator is most likely to get
        // backwards: the probe did not run. Outside a repository
        // `git_ignore_status` reports `Unknown` and `has_tracked_under` reports
        // `false` — a pair that reads exactly like "not tracked, nothing to
        // worry about" if the check is written as "refuse when tracked"
        // instead of "copy only when ignored". A failed probe is not consent.
        let (_tmp, source, target) = fs_fixture();
        write(&source.join("cache/a.bin"), b"a");
        assert_eq!(
            crate::core::git_ignore::git_ignore_status(&source, "cache"),
            IgnoreStatus::Unknown,
            "fixture precondition: no repository, so git cannot answer"
        );

        assert_sole_skip!(
            run(&source, &target, &config(&["cache"])),
            SkipReason::Unclassifiable { .. }
        );
        assert!(
            entries_of(&target).is_empty(),
            "an unanswerable probe copies nothing: {:?}",
            entries_of(&target)
        );
    }

    // ── copy_entries: what the sink is and is not told ───────────────────

    #[test]
    fn copy_entries_tells_the_sink_nothing_it_will_report_later() {
        // The engine's half of "rendered exactly once". Per-entry facts travel
        // in the returned outcomes and are drawn by `report_copy_results`; a
        // `sink.on_warning` here as well would print every skip twice — once
        // as a stderr line, once as a rail row — and tear the live region
        // between them. Only `on_debug` (`-v` narration, which this sink drops)
        // belongs to the engine.
        use crate::core::RecordingStageSink;

        let (_tmp, source, target) = repo_fixture("/target\n");
        write(&source.join("target/app"), b"binary");
        write(&source.join("src/main.rs"), b"fn main() {}");

        let mut sink = RecordingStageSink::default();
        // One of each shape the stage can produce: a copy, a refusal, and a
        // missing source.
        let result = copy_entries(
            &source,
            &target,
            &config(&["target", "src", "never-built"]),
            false,
            &mut sink,
        );

        assert_eq!(result.outcomes.len(), 3);
        assert!(sink.warnings.is_empty(), "{:?}", sink.warnings);
        assert!(sink.steps.is_empty(), "{:?}", sink.steps);
        assert!(
            sink.events.is_empty(),
            "the engine reports no stage events of its own: {:?}",
            sink.events
        );
    }

    // ── copy_entries: entry shapes ───────────────────────────────────────

    #[cfg(unix)]
    #[test]
    fn copy_entries_sees_through_a_symlink_to_the_same_worktree() {
        // The same-directory refusal resolves symlinks, and has to: a source
        // and target that reach one directory by two names is exactly what a
        // `--from` resolved through a symlinked worktree path looks like, and
        // under `force` a missed detection clears the source's own caches
        // before "copying" them.
        let (tmp, source, _target) = repo_fixture("/target\n");
        write(&source.join("target/app"), b"precious");
        let alias = tmp.path().join("alias");
        std::os::unix::fs::symlink(&source, &alias).unwrap();

        let result = copy_entries(&source, &alias, &config(&["target"]), true, &mut NullSink);

        assert_eq!(sole_skip(&result), SkipReason::SameWorktree);
        assert_eq!(fs::read(source.join("target/app")).unwrap(), b"precious");
    }

    #[test]
    fn copy_entries_reports_a_nested_declaration_as_already_present() {
        // `copy: [a, a/b]` — the second entry is inside the first, so copying
        // `a` already carried it. It must resolve as the idempotence skip
        // rather than copying the subtree a second time on top of itself
        // (which `copy_dir` would refuse) or reporting a failure.
        let (_tmp, source, target) = repo_fixture("/a\n");
        write(&source.join("a/top.txt"), b"top");
        write(&source.join("a/b/deep.txt"), b"deep");

        let result = run(&source, &target, &config(&["a", "a/b"]));

        let [CopyOutcome::Copied { entry, .. }, second] = result.outcomes.as_slice() else {
            panic!("expected a copy then a skip, got {:?}", result.outcomes);
        };
        assert_eq!(entry, "a");
        assert_eq!(
            second,
            &CopyOutcome::Skipped {
                entry: "a/b".into(),
                reason: SkipReason::DestinationExists,
                unreadable: 0,
            }
        );
        assert_eq!(fs::read(target.join("a/b/deep.txt")).unwrap(), b"deep");
        assert_eq!(entries_of(&target.join("a")), ["b", "top.txt"]);
    }

    // ── copy_entries: force ──────────────────────────────────────────────

    #[test]
    fn copy_entries_force_copies_an_absent_destination_like_any_other() {
        // `force` means "do not stop at a destination that exists", not "a
        // destination must exist" — the common `daft warm --force` case is a
        // worktree where half the caches were never copied at all.
        let (_tmp, source, target) = repo_fixture("/target\n");
        write(&source.join("target/app"), b"fresh");

        let result = copy_entries(&source, &target, &config(&["target"]), true, &mut NullSink);

        assert!(
            matches!(result.outcomes.as_slice(), [CopyOutcome::Copied { .. }]),
            "{:?}",
            result.outcomes
        );
        assert_eq!(fs::read(target.join("target/app")).unwrap(), b"fresh");
    }

    #[cfg(unix)]
    #[test]
    fn an_unwritable_destination_is_named_as_such_and_costs_nothing() {
        // Two lies in one, before the probe tri-stated: an unwritable
        // destination on APFS reported `no reflink support — fallback: skip`,
        // blaming a filesystem that clones perfectly well and sending the user
        // to a knob that was never the problem. And under `--force` it got
        // there only after the removal had already run.
        //
        // The probe runs when the size gate can fire — the config that has a
        // reason to ask what a copy will cost.
        use std::os::unix::fs::PermissionsExt;

        let (_tmp, source, target) = repo_fixture("/cache\n");
        write(&source.join("cache/a.bin"), b"new");
        write(&target.join("cache/old.bin"), b"the existing cache");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o500)).unwrap();
        // chmod does not stop a root euid, and some filesystems ignore perms.
        let premise_holds = fs::create_dir(target.join("probe")).is_err();

        let gated = ResolvedCopyConfig {
            paths: vec!["cache".to_string()],
            fallback: CopyFallback::Skip,
            max_size_bytes: None,
            max_size_unparsed: None,
        };
        let result = copy_entries(&source, &target, &gated, true, &mut NullSink);

        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        if !premise_holds {
            return;
        }

        assert_sole_skip!(result, SkipReason::ReflinkUnprobeable { .. });
        let reason = sole_skip(&result);
        let SkipReason::ReflinkUnprobeable { detail, .. } = &reason else {
            unreachable!()
        };
        assert!(
            detail.contains("probe file"),
            "the row has to name the real cause: {detail}"
        );
        assert!(
            !skip_phrase("cache", &reason).contains("no reflink support"),
            "a probe that never ran must not blame the filesystem"
        );
        assert!(
            target.join("cache/old.bin").is_file(),
            "refusing before the removal is the point: --force destroyed the cache \
             and then reported a skip"
        );
    }

    #[cfg(unix)]
    #[test]
    fn skipping_the_measurement_does_not_make_a_broken_destination_quiet() {
        // Under the default bare-list config the size gate cannot fire, so the
        // measurement and probe are skipped entirely — the whole point of that
        // guard. This is what keeps the saving honest: the same unwritable
        // destination still fails loudly, from the copy itself, naming what it
        // left behind.
        use std::os::unix::fs::PermissionsExt;

        let (_tmp, source, target) = repo_fixture("/cache\n");
        write(&source.join("cache/a.bin"), b"new");
        write(&target.join("cache/old.bin"), b"the existing cache");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o500)).unwrap();
        let premise_holds = fs::create_dir(target.join("probe")).is_err();

        let result = copy_entries(&source, &target, &config(&["cache"]), true, &mut NullSink);

        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        if !premise_holds {
            return;
        }

        let [CopyOutcome::Failed { entry, detail }] = result.outcomes.as_slice() else {
            panic!("expected one loud failure, got {:?}", result.outcomes);
        };
        assert_eq!(entry, "cache");
        assert!(
            detail.contains("mistaken for a finished copy"),
            "what survives has to be called out: {detail}"
        );
    }

    #[test]
    fn copy_entries_refuses_an_absolute_entry_before_force_removes_anything() {
        // The containment guard's real weight is here, not in the plain run.
        // `target.join("/abs")` IS `/abs` — an absolute entry names the same
        // place on both sides — so under `force` an entry that got past the
        // guard would hand `remove_existing` a path outside both worktrees and
        // delete it. The guard runs before the removal loop; this is what
        // holds that ordering in place.
        let (tmp, source, target) = repo_fixture("");
        write(&tmp.path().join("outside/precious"), b"not yours");
        let absolute = tmp.path().join("outside").to_string_lossy().to_string();

        let result = copy_entries(&source, &target, &config(&[&absolute]), true, &mut NullSink);

        assert_sole_skip!(result, SkipReason::Uncontained { .. });
        assert_eq!(
            fs::read(tmp.path().join("outside/precious")).unwrap(),
            b"not yours",
            "an absolute entry must not reach the removal loop"
        );
        assert!(entries_of(&target).is_empty());
    }

    // ── copy_entries: partial-destination cleanup across a fan-out ───────

    #[cfg(unix)]
    #[test]
    fn copy_entries_clears_only_the_failing_match_of_a_glob() {
        // One entry, several matches, one of them broken. The cleanup has to
        // be surgical: the matches that landed are finished caches and must
        // survive, while the one that broke must be cleared so the next run
        // retries it instead of reporting `already present` over a half-copied
        // tree. Getting this wrong in either direction is silent — the entry
        // reports the same `Failed` row either way.
        use std::os::unix::fs::PermissionsExt;

        let (_tmp, source, target) = repo_fixture("dist\n");
        write(&source.join("api/dist/server.js"), b"api");
        write(&source.join("web/dist/app.js"), b"web");
        let locked = source.join("web/dist/locked");
        fs::create_dir_all(&locked).unwrap();
        write(&locked.join("inner.js"), b"unreadable");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let premise_holds = fs::read_dir(&locked).is_err();

        // `api/dist` sorts first, so it is copied before `web/dist` breaks.
        let first = run(&source, &target, &config(&["**/dist"]));
        let survivor = fs::read(target.join("api/dist/server.js"));
        let cleared = !target.join("web/dist").exists();
        // A rerun while the cause persists must retry the failing match.
        let second = run(&source, &target, &config(&["**/dist"]));

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        if !premise_holds {
            return;
        }
        assert!(
            matches!(first.outcomes.as_slice(), [CopyOutcome::Failed { .. }]),
            "{:?}",
            first.outcomes
        );
        assert_eq!(
            survivor.unwrap(),
            b"api",
            "the match that finished is a warm cache — cleanup must not take it"
        );
        assert!(
            cleared,
            "the partial match must not survive as a finished one"
        );
        assert!(
            matches!(second.outcomes.as_slice(), [CopyOutcome::Failed { .. }]),
            "the rerun retried the failing match: {:?}",
            second.outcomes
        );

        // Cause gone: the retry the cleanup made possible completes, and the
        // entry reports only the match it still had to do.
        let third = run(&source, &target, &config(&["**/dist"]));
        let [CopyOutcome::Copied { matches, .. }] = third.outcomes.as_slice() else {
            panic!("expected the retry to copy, got {:?}", third.outcomes);
        };
        assert_eq!(*matches, 1, "the already-warm match was not copied twice");
        assert_eq!(
            fs::read(target.join("web/dist/locked/inner.js")).unwrap(),
            b"unreadable"
        );
        assert_eq!(fs::read(target.join("api/dist/server.js")).unwrap(), b"api");
    }

    // ── The reflink probe ────────────────────────────────────────────────

    #[test]
    fn copy_entries_leaves_no_probe_file_in_the_destination() {
        // The probe clones a sample into the destination and unlinks it. A
        // leaked `.daft-reflink-probe-*` would land in the user's brand-new
        // worktree — and, being untracked and unignored, in `git status`.
        let (_tmp, source, target) = repo_fixture("/target\n");
        write(&source.join("target/app"), b"binary");

        run(&source, &target, &config(&["target"]));

        assert_eq!(
            entries_of(&target),
            ["target"],
            "the probe cleans up after itself"
        );
    }

    #[test]
    fn copy_entries_copies_a_tree_with_no_regular_file_in_it() {
        // The probe answers "can these bytes be cloned?" by cloning one of
        // them — and an empty cache, or one of only directories and symlinks,
        // has none to offer. It reports clonable, which is what keeps a copy
        // that costs nothing from being refused by `fallback: skip` or by a
        // cap it cannot possibly exceed.
        let (_tmp, source, target) = repo_fixture("/empty\n/links\n");
        fs::create_dir_all(source.join("empty/inner")).unwrap();
        fs::create_dir_all(source.join("links")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("../elsewhere", source.join("links/dangling")).unwrap();

        #[cfg(unix)]
        let declared = ["empty", "links"];
        #[cfg(not(unix))]
        let declared = ["empty"];

        let result = copy_entries(
            &source,
            &target,
            &ResolvedCopyConfig {
                paths: declared.iter().map(|p| (*p).to_string()).collect(),
                // The two knobs that would refuse a byte copy: neither may
                // fire, because nothing here has to be copied byte-wise.
                fallback: CopyFallback::Skip,
                max_size_bytes: Some(0),
                max_size_unparsed: None,
            },
            false,
            &mut NullSink,
        );

        assert!(
            result.outcomes.iter().all(|o| matches!(
                o,
                CopyOutcome::Copied {
                    method: CopyMethod::Reflinked,
                    ..
                }
            )),
            "{:?}",
            result.outcomes
        );
        assert!(target.join("empty/inner").is_dir());
        #[cfg(unix)]
        assert!(target.join("links/dangling").is_symlink());
    }

    // ── plan_method: the cells the existing matrix leaves open ───────────

    #[test]
    fn gate_byte_copies_refuses_on_fallback_skip_without_consulting_the_cap() {
        // Both refusals apply: no reflink, and over the cap. `NoReflink` wins,
        // and it has to — `fallback: skip` says this tree is not worth byte
        // copying at any size, so `2.1 GB over the 1 GB max_size` would send
        // the user to raise a cap that changes nothing.
        assert_eq!(
            gate_byte_copies(true, 4096, CopyFallback::Skip, Some(1024)),
            Some(SkipReason::NoReflink)
        );
        assert_eq!(
            gate_byte_copies(true, 0, CopyFallback::Skip, Some(0)),
            Some(SkipReason::NoReflink)
        );
    }

    #[test]
    fn gate_byte_copies_gate_trips_one_byte_over_and_not_before() {
        // The cap is inclusive: `max_size: 1KB` admits a 1024-byte entry and
        // refuses a 1025-byte one. An off-by-one here is invisible on every
        // tree except the one that sits exactly on the boundary.
        assert_eq!(
            gate_byte_copies(true, 1025, CopyFallback::Copy, Some(1024)),
            Some(SkipReason::TooLarge {
                size_bytes: 1025,
                limit_bytes: 1024,
            })
        );
        assert_eq!(
            gate_byte_copies(true, 1023, CopyFallback::Copy, Some(1024)),
            None
        );
        // A zero cap still admits an entry with no bytes in it: the rule is
        // "over the cap", and nothing is not over anything.
        assert_eq!(gate_byte_copies(true, 0, CopyFallback::Copy, Some(0)), None);
        assert_eq!(
            gate_byte_copies(true, 1, CopyFallback::Copy, Some(0)),
            Some(SkipReason::TooLarge {
                size_bytes: 1,
                limit_bytes: 0,
            })
        );
    }

    // ── Phrasing and formatting boundaries ───────────────────────────────

    #[test]
    fn format_bytes_switches_units_at_the_multiple_not_before() {
        // The unit boundary is where a size the user recognizes (`1 KB`,
        // `1 GB`) has to come back out the way they wrote it — this is the
        // function that quotes their own `max_size` back at them.
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1 KB");
        // One byte over rounds back to the whole unit rather than growing a
        // misleading `1.0009` — a single decimal is the whole point.
        assert_eq!(format_bytes(1025), "1 KB");
        assert_eq!(format_bytes(1024 * 1024 - 1), "1024 KB");
        assert_eq!(format_bytes(1024 * 1024), "1 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024 - 1), "1024 MB");
        // TB is the last unit: bigger values keep counting in it rather than
        // inventing one.
        assert_eq!(format_bytes(u64::MAX), "16777216 TB");
    }

    #[test]
    fn copied_annotation_counts_paths_from_two_and_times_from_a_tenth() {
        // The two thresholds the annotation is built on, each held at its
        // edge: the count appears from the second path, and the duration from
        // the point where one decimal can show it.
        let annotate = |matches, millis| {
            copied_annotation(
                matches,
                2048,
                CopyMethod::Copied,
                Duration::from_millis(millis),
                0,
            )
        };
        assert_eq!(annotate(2, 0), "2 paths · 2 KB · copied");
        assert_eq!(annotate(1, 0), "2 KB · copied");
        assert_eq!(annotate(1, 49), "2 KB · copied", "0.0s says nothing");
        assert_eq!(annotate(1, 50), "2 KB · copied · 0.1s");
    }

    #[test]
    fn exactly_one_stage_renderer_speaks_for_a_key() {
        // Both no-live-region dispatch sites — `route_stage` and the default
        // `ProgressSink::on_stage` — call the shared-file renderer and the
        // copied-path one back to back for every event, and rely on each
        // returning `None` for the other's keys. If either ever stopped
        // filtering, every shared link and every copied cache would print
        // twice, in two voices.
        use crate::core::stage::{StageEvent, StageId, StepKey};

        let events = [
            StageEvent::Completed {
                annotation: Some("1 KB · reflinked".into()),
            },
            StageEvent::SkippedAttention {
                reason: "failed — disk full".into(),
            },
        ];
        for event in &events {
            assert_eq!(
                crate::core::shared::legacy_shared_stage_line(
                    &StepKey::scoped(StageId::CopyPath, "target"),
                    event,
                    false,
                ),
                None,
                "the shared-file renderer answered for a copied-path key: {event:?}"
            );
            assert!(
                legacy_copy_stage_line(&StepKey::scoped(StageId::CopyPath, "target"), event, false)
                    .is_some(),
                "the copied-path renderer must answer for its own: {event:?}"
            );
            assert_eq!(
                legacy_copy_stage_line(&StepKey::scoped(StageId::SharedFile, ".env"), event, false,),
                None,
                "the copied-path renderer answered for a shared-file key: {event:?}"
            );
        }
    }

    /// A count, not a clock. This repo has a long history of timing-flake
    /// postmortems, and "batched is faster than per-match" is exactly the shape
    /// that goes red on a loaded CI box while the code is fine. The invariant
    /// that actually matters is countable: git runs twice per BATCH, and the
    /// batch count is a pure function of the input.
    #[test]
    fn the_gitignore_probe_costs_two_git_invocations_per_batch() {
        let (_tmp, source, _target) = repo_fixture("dist\n");
        for i in 0..30 {
            write(&source.join(format!("pkg{i}/dist/app.js")), b"//");
        }
        let matches = expand_entries(&source, "**/dist");
        assert_eq!(matches.len(), 30, "fixture precondition");

        // Thirty short paths fit in one batch: two git processes for the whole
        // entry, where per-match probing spawned ninety.
        assert_eq!(probe_batches(&matches).len(), 1);
        assert!(matches!(
            classify_matches(&source, &matches).unwrap(),
            Classification::Ignored
        ));

        // And the batched verdict agrees with the per-path reference
        // implementation it replaced — the reason `core::git_ignore` keeps its
        // readable one-path-at-a-time functions.
        for rel in &matches {
            assert_eq!(
                crate::core::git_ignore::git_ignore_status(&source, rel),
                IgnoreStatus::Ignored
            );
            assert!(!crate::core::git_ignore::has_tracked_under(&source, rel));
        }
    }

    #[test]
    fn probe_batches_stay_under_the_pipe_and_argv_ceilings() {
        // The two limits that make chunking necessary at all: `check-ignore
        // --stdin` writes its answers back down a pipe while we are still
        // writing to it, and `ls-files` takes its pathspecs as argv.
        let many: Vec<String> = (0..8_000).map(|i| format!("pkg{i:05}/dist")).collect();
        let batches = probe_batches(&many);

        assert!(batches.len() > 1, "8,000 paths cannot be one batch");
        assert_eq!(
            batches.iter().map(|b| b.len()).sum::<usize>(),
            many.len(),
            "every path lands in exactly one batch"
        );
        for batch in &batches {
            assert!(batch.len() <= PROBE_BATCH_PATHS);
            let bytes: usize = batch.iter().map(|p| p.len() + 1).sum();
            assert!(
                bytes <= PROBE_BATCH_BYTES + PROBE_BATCH_PATHS,
                "a batch must stay far below any pipe buffer: {bytes} bytes"
            );
        }
        // A single path longer than the byte budget still gets its own batch
        // rather than being dropped.
        let huge = vec!["x".repeat(PROBE_BATCH_BYTES * 2)];
        assert_eq!(probe_batches(&huge).len(), 1);
        assert!(probe_batches(&[]).is_empty());
    }

    /// The deadlock, reproduced. Writing the whole NUL list to
    /// `check-ignore --stdin` and only then draining stdout blocks forever once
    /// either pipe fills — 64 KB on Linux, less on macOS — which a
    /// `node_modules` glob reaches at a few thousand paths. The old shape hung
    /// here; the batched one returns.
    #[test]
    fn thousands_of_matches_classify_without_deadlocking() {
        let (_tmp, source, target) = repo_fixture("/cache\n");
        let mut matches = Vec::new();
        for i in 0..8_000 {
            let rel = format!("cache/pkg{i:05}");
            fs::create_dir_all(source.join(&rel)).unwrap();
            matches.push(rel);
        }
        // Well past the 64 KB Linux pipe buffer (macOS's is smaller still), so
        // the unbatched write blocks with nobody draining the answers.
        let payload: usize = matches.iter().map(|m| m.len() + 1).sum();
        assert!(payload > 64 * 1024, "fixture precondition: {payload} bytes");

        assert!(matches!(
            classify_matches(&source, &matches).unwrap(),
            Classification::Ignored
        ));

        // And through the whole stage, where the same list is also handed to
        // `ls-files` as argv — the ARG_MAX half of the same problem.
        let result = run(&source, &target, &config(&["cache"]));
        assert!(
            matches!(result.outcomes.as_slice(), [CopyOutcome::Copied { .. }]),
            "{:?}",
            result.outcomes
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_dotdot_through_a_symlink_is_refused_however_it_is_spelled() {
        // Depth-counting is not containment. `link/../x` stays at depth 1
        // lexically, and git normalizes it the same lexical way — so
        // `check-ignore` answers about a path that does not exist while the
        // kernel resolves `link` as a symlink and lands wherever it points.
        // No legitimate cache entry contains `..`, so the refusal is blanket.
        let (tmp, source, target) = repo_fixture("*\n");
        write(&tmp.path().join("outside/precious"), b"not yours");
        std::os::unix::fs::symlink(tmp.path().join("outside"), source.join("link")).unwrap();

        for spelling in ["link/../outside", "../outside", "a/../../outside"] {
            let result = copy_entries(&source, &target, &config(&[spelling]), true, &mut NullSink);
            assert_sole_skip!(result, SkipReason::Uncontained { .. });
        }
        assert_eq!(
            fs::read(tmp.path().join("outside/precious")).unwrap(),
            b"not yours"
        );

        // The two refusals stay distinct: one is about escaping, the other
        // about naming the worktree itself.
        assert!(
            containment_violation("link/../x")
                .unwrap()
                .contains("outside the worktree")
        );
        assert!(
            containment_violation(".")
                .unwrap()
                .contains("worktree itself")
        );
    }

    #[test]
    fn the_reflink_probe_never_writes_above_the_target_root() {
        // The probe lands in the nearest EXISTING ancestor of the match's
        // destination. Without a boundary that search walks straight out of a
        // missing target worktree and drops a scratch file in whatever sits
        // above it — the layout container, or the user's home.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("worktree");
        // Target absent entirely: nothing above it is a candidate.
        assert_eq!(
            nearest_existing_dir(&target.join("cache/inner"), &target),
            None
        );
        // Target present: its own root is the floor, and is used.
        fs::create_dir_all(&target).unwrap();
        assert_eq!(
            nearest_existing_dir(&target.join("cache/inner"), &target),
            Some(target.as_path())
        );
    }

    // ── Partial expansion: copy what was found, say what was not ─────────

    #[cfg(unix)]
    #[test]
    fn a_glob_that_found_matches_copies_them_and_counts_what_it_could_not_read() {
        // Refusing everything because one subtree was unreadable trades a
        // partial cache for none at all. The matches that WERE found are
        // copied; the shortfall rides in the annotation rather than turning
        // the row yellow, because what was copied really was copied.
        use std::os::unix::fs::PermissionsExt;

        let (_tmp, source, target) = repo_fixture("dist\n");
        write(&source.join("readable/dist/app.js"), b"bundle");
        fs::create_dir_all(source.join("locked/pkg/dist")).unwrap();
        fs::set_permissions(source.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
        let premise_holds = fs::read_dir(source.join("locked")).is_err();

        let result = run(&source, &target, &config(&["**/dist"]));

        fs::set_permissions(source.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
        if !premise_holds {
            return;
        }

        let [
            CopyOutcome::Copied {
                matches,
                unreadable,
                ..
            },
        ] = result.outcomes.as_slice()
        else {
            panic!(
                "expected the found match to copy, got {:?}",
                result.outcomes
            );
        };
        assert_eq!(*matches, 1);
        assert!(*unreadable >= 1, "the shortfall has to be counted");
        assert_eq!(
            fs::read(target.join("readable/dist/app.js")).unwrap(),
            b"bundle"
        );

        // And it reaches the row, on a green face.
        assert!(
            copied_annotation(1, 6, CopyMethod::Reflinked, Duration::ZERO, 2)
                .ends_with("· 2 unreadable")
        );
    }

    #[test]
    fn a_removal_or_copy_that_leaves_wreckage_behind_says_so() {
        // A failed copy and a failed `--force` removal leave the same hazard:
        // a path that exists, is not what anyone asked for, and reads as
        // "already present" to every later run. Warn-never-abort means the
        // detail is the only place that can be said.
        //
        // Unit-level because the end-to-end route is now largely closed: an
        // unwritable destination is refused by the reflink probe before any
        // removal runs (see `an_unwritable_destination_is_named_as_such_...`),
        // which is the better outcome, not a gap.
        let tmp = TempDir::new().unwrap();
        let gone = tmp.path().join("vanished");
        let stays = tmp.path().join("wreckage");
        fs::create_dir_all(&stays).unwrap();

        let clean = note_surviving_remains(&gone, anyhow::anyhow!("boom"));
        assert_eq!(
            format!("{clean:#}"),
            "boom",
            "nothing survived, nothing to add"
        );

        let dirty = note_surviving_remains(&stays, anyhow::anyhow!("boom"));
        assert!(
            format!("{dirty:#}").contains("mistaken for a finished copy"),
            "a surviving destination has to be called out: {dirty:#}"
        );
    }

    #[test]
    fn an_absolute_entry_is_refused_not_quietly_rewritten() {
        // Stripping the leading `/` turned `copy: ["/var"]` into `copy: ["var"]`
        // — a green row over the worktree's own `var/`, a tree the config never
        // named — and made the documented absolute-path refusal unreachable on
        // Unix. The entry survives normalization intact and is refused, with
        // the one-character fix in the phrase.
        let tmp = TempDir::new().unwrap();
        write(
            &tmp.path().join("daft.yml"),
            b"copy:\n  - /var\n  - target/\n",
        );
        assert_eq!(
            read_copy_config(tmp.path()).unwrap().paths,
            vec!["/var".to_string(), "target".to_string()],
            "a leading slash is not cosmetic; only a trailing one is"
        );

        let (_tmp, source, target) = repo_fixture("*\n");
        write(&source.join("var/decoy.bin"), b"the worktree's own var/");

        let result = run(&source, &target, &config(&["/var"]));

        assert_sole_skip!(result, SkipReason::Uncontained { .. });
        assert!(
            !target.join("var").exists(),
            "an absolute entry must not be reinterpreted as a relative one"
        );
        let SkipReason::Uncontained { detail, .. } = sole_skip(&result) else {
            unreachable!()
        };
        assert!(
            detail.contains("drop the leading '/'") && detail.contains("write 'var'"),
            "the refusal has to carry the fix: {detail}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_entry_that_is_an_escaping_link_is_rebased_onto_the_destination() {
        // A `.venv -> ../.venvs/proj` entry copied verbatim resolves from the
        // source's depth and dangles from the destination's. daft's own
        // contained layout produces exactly that pair of depths.
        let tmp = TempDir::new().unwrap();
        write(&tmp.path().join(".venvs/proj/bin/python"), b"#!");
        let source = tmp.path().join("main");
        let target = tmp.path().join("feature/login");
        for root in [&source, &target] {
            fs::create_dir_all(root).unwrap();
            let out = crate::utils::git_command_at(root)
                .args(["init", "-q", "-b", "main"])
                .output()
                .unwrap();
            assert!(out.status.success());
            write(&root.join(".gitignore"), b"/.venv\n");
        }
        std::os::unix::fs::symlink("../.venvs/proj", source.join(".venv")).unwrap();
        assert!(source.join(".venv/bin/python").exists(), "fixture premise");

        let result = run(&source, &target, &config(&[".venv"]));

        assert!(
            matches!(result.outcomes.as_slice(), [CopyOutcome::Copied { .. }]),
            "{:?}",
            result.outcomes
        );
        assert!(
            target.join(".venv").is_symlink(),
            "still a link, not a copy"
        );
        assert!(
            target.join(".venv/bin/python").exists(),
            "the link must reach the same interpreter from its new depth, got {:?}",
            fs::read_link(target.join(".venv")).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn force_replacing_a_shared_link_leaves_one_that_still_resolves() {
        // The `shared:` + `copy:` double declaration. The shared stage links
        // the path into both worktrees; `warm --force` then replaces the
        // target's correct link with a copy of the source's text — which,
        // between worktrees at different depths, dangles. It was reported
        // `✓ 0 B · reflinked`: a green row over a broken link.
        let tmp = TempDir::new().unwrap();
        let shared = tmp.path().join(".git/.daft/shared");
        write(&shared.join(".env"), b"SECRET=1");
        let source = tmp.path().join("main");
        let target = tmp.path().join("feature/login");
        for root in [&source, &target] {
            fs::create_dir_all(root).unwrap();
            let out = crate::utils::git_command_at(root)
                .args(["init", "-q", "-b", "main"])
                .output()
                .unwrap();
            assert!(out.status.success());
            write(&root.join(".gitignore"), b"/.env\n");
        }
        // Each worktree's own correct relative link to the shared file.
        std::os::unix::fs::symlink("../.git/.daft/shared/.env", source.join(".env")).unwrap();
        std::os::unix::fs::symlink("../../.git/.daft/shared/.env", target.join(".env")).unwrap();
        assert_eq!(fs::read(target.join(".env")).unwrap(), b"SECRET=1");

        let result = copy_entries(&source, &target, &config(&[".env"]), true, &mut NullSink);

        assert!(
            matches!(result.outcomes.as_slice(), [CopyOutcome::Copied { .. }]),
            "{:?}",
            result.outcomes
        );
        assert_eq!(
            fs::read(target.join(".env")).unwrap(),
            b"SECRET=1",
            "the replacement link must still reach the shared file, got {:?}",
            fs::read_link(target.join(".env")).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_partial_expansion_keeps_reporting_its_shortfall_after_it_is_warm() {
        // The count rode only on `Copied`, so a partial glob announced the
        // unreadable half once and then reported `already present` — the most
        // reassuring row the stage has — on every run after it, forever.
        use std::os::unix::fs::PermissionsExt;

        let (_tmp, source, target) = repo_fixture("dist\n");
        write(&source.join("readable/dist/app.js"), b"bundle");
        fs::create_dir_all(source.join("locked/pkg/dist")).unwrap();
        fs::set_permissions(source.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
        let premise_holds = fs::read_dir(source.join("locked")).is_err();

        let first = run(&source, &target, &config(&["**/dist"]));
        // Second run: everything found is now present, and the shortfall is
        // unchanged.
        let second = run(&source, &target, &config(&["**/dist"]));

        fs::set_permissions(source.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
        if !premise_holds {
            return;
        }

        let [CopyOutcome::Copied { unreadable, .. }] = first.outcomes.as_slice() else {
            panic!("expected the found match to copy, got {:?}", first.outcomes);
        };
        assert!(*unreadable >= 1);

        let [
            CopyOutcome::Skipped {
                reason, unreadable, ..
            },
        ] = second.outcomes.as_slice()
        else {
            panic!(
                "expected an already-present skip, got {:?}",
                second.outcomes
            );
        };
        assert_eq!(*reason, SkipReason::DestinationExists);
        assert!(
            *unreadable >= 1,
            "the shortfall must survive the outcome that hides it best"
        );
        // And it reaches both surfaces, from the one helper.
        let phrase = with_shortfall(skip_phrase("**/dist", reason), *unreadable);
        assert!(
            phrase.contains("already present") && phrase.contains("unreadable"),
            "{phrase}"
        );
    }

    #[test]
    fn a_name_that_cannot_be_spelled_relatively_counts_as_unreadable() {
        // Dropping it silently shrank the expansion with nothing to show for
        // it. It is not a match and never can be, so it is a place the walk
        // could not use.
        assert_eq!(
            with_shortfall("already present".into(), 0),
            "already present"
        );
        assert_eq!(
            with_shortfall("already present".into(), 2),
            "already present · 2 unreadable"
        );
        assert_eq!(relative_to_slash_string(Path::new("")), None);
    }

    #[test]
    fn the_default_config_never_walks_the_cache_to_compute_a_number_nobody_reads() {
        // `fallback: copy` with no `max_size` — the bare-list default — cannot
        // reach any gate outcome, so measuring and probing are pure cost. On a
        // reflinking filesystem that walk dominates the whole stage.
        assert!(!gate_can_fire_for(&config(&["target"])));

        let capped = ResolvedCopyConfig {
            max_size_bytes: Some(1024),
            ..config(&["target"])
        };
        assert!(gate_can_fire_for(&capped), "a cap has to be weighed");

        let skipping = ResolvedCopyConfig {
            fallback: CopyFallback::Skip,
            ..config(&["target"])
        };
        assert!(
            gate_can_fire_for(&skipping),
            "`fallback: skip` needs the probe's answer even with no cap"
        );

        // Nothing is lost: the row's method and size come from the copier.
        let (_tmp, source, target) = repo_fixture("/target\n");
        write(&source.join("target/app"), b"0123456789");
        let copied = run(&source, &target, &config(&["target"]));
        let [CopyOutcome::Copied { bytes, .. }] = copied.outcomes.as_slice() else {
            panic!("expected a copy, got {:?}", copied.outcomes);
        };
        assert_eq!(
            *bytes, 10,
            "reported from what was written, not from a pre-walk"
        );
    }

    // ── Force-path safety: what may be destroyed, and when ───────────────

    #[test]
    fn force_refuses_to_delete_content_the_target_worktree_tracks() {
        // The invariant runs against the SOURCE, and the source's opinion says
        // nothing about the destination. Branch `experiment` gitignores
        // `docs/`; branch `main` has it committed. `daft warm --from
        // experiment --force` standing in main would otherwise pass the source
        // probe and delete main's tracked, committed `docs/`, replacing it with
        // generated output.
        let (_tmp, source, target) = fs_fixture();
        for root in [&source, &target] {
            let out = crate::utils::git_command_at(root)
                .args(["init", "-q", "-b", "main"])
                .output()
                .unwrap();
            assert!(out.status.success());
        }
        // Source: docs/ is a gitignored build artifact.
        write(&source.join(".gitignore"), b"/docs\n");
        write(&source.join("docs/generated.html"), b"<generated>");
        // Target: docs/ is committed content.
        write(&target.join("docs/handbook.md"), b"# the real thing");
        git(&target, &["add", "docs/handbook.md"]);
        git(&target, &["commit", "-q", "-m", "docs"]);

        let result = copy_entries(&source, &target, &config(&["docs"]), true, &mut NullSink);

        assert_sole_skip!(result, SkipReason::TargetTracked { .. });
        assert_eq!(
            fs::read(target.join("docs/handbook.md")).unwrap(),
            b"# the real thing",
            "--force must never delete content the target worktree tracks"
        );
    }

    #[test]
    fn force_removes_nothing_when_the_entry_is_going_to_be_skipped_anyway() {
        // Removal happens only once a copy is certain to proceed. Any refusal
        // that can still fire after it — the cap, `fallback: skip`, a missing
        // source — would otherwise report a dim skip over a destroyed cache:
        // net loss wearing a "nothing happened" face.
        let (_tmp, source, target) = repo_fixture("/cache\n");
        write(&source.join("cache/big.bin"), &vec![7u8; 4096]);
        write(&target.join("cache/old.bin"), b"the existing cache");

        // A cap far below the entry's size. Only reachable end-to-end where the
        // filesystem cannot clone — on APFS the cap is never consulted at all,
        // and the copy correctly proceeds — so this arm runs on Linux CI and
        // the gate below carries it everywhere else.
        let capped = ResolvedCopyConfig {
            paths: vec!["cache".to_string()],
            fallback: CopyFallback::Copy,
            max_size_bytes: Some(1),
            max_size_unparsed: None,
        };
        let reflinks = probe_reflink_support(&target).unwrap_or(true);
        let result = copy_entries(&source, &target, &capped, true, &mut NullSink);
        if !reflinks {
            assert_sole_skip!(result, SkipReason::TooLarge { .. });
            assert!(
                target.join("cache/old.bin").is_file(),
                "a capped entry reported a skip over a destroyed cache"
            );
        }

        // A source that is not there at all: the same ordering, one gate
        // earlier, and reachable on every filesystem.
        let missing = config(&["never-built"]);
        write(&target.join("never-built/keep.bin"), b"still here");
        copy_entries(&source, &target, &missing, true, &mut NullSink);
        assert!(
            target.join("never-built/keep.bin").is_file(),
            "a missing source must not have cleared the destination first"
        );

        // And the gate that decides it, in isolation — the only way to see the
        // `fallback: skip` and cap arms on a filesystem that always clones.
        assert_eq!(
            gate_byte_copies(true, 4096, CopyFallback::Copy, Some(1)),
            Some(SkipReason::TooLarge {
                size_bytes: 4096,
                limit_bytes: 1
            })
        );
    }

    // ── Overlapping declarations ─────────────────────────────────────────

    #[test]
    fn overlapping_declarations_copy_the_whole_tree_in_either_order() {
        // `copy: [web/dist, web]`: copying `web/dist` first manufactures an
        // empty `target/web` on the way, and `web` then finds its destination
        // "already present" — so everything else under `web/` never arrives,
        // on creation and on every warm after it. Ancestors are processed
        // first regardless of declaration order.
        for declared in [["web/dist", "web"], ["web", "web/dist"]] {
            let (_tmp, source, target) = repo_fixture("/web\n");
            write(&source.join("web/dist/app.js"), b"bundle");
            write(&source.join("web/cache/blob.bin"), b"blob");

            let result = run(&source, &target, &config(&declared));

            assert_eq!(
                fs::read(target.join("web/cache/blob.bin")).unwrap(),
                b"blob",
                "declared {declared:?}: the containing entry copied its whole tree"
            );
            assert_eq!(fs::read(target.join("web/dist/app.js")).unwrap(), b"bundle");
            // One outcome per declaration, still in declaration order.
            assert_eq!(
                result
                    .outcomes
                    .iter()
                    .map(CopyOutcome::entry)
                    .collect::<Vec<_>>(),
                declared,
                "outcomes follow the declaration order, not the processing order"
            );
        }
    }

    // ── The cap the validator never gets to report ───────────────────────

    #[test]
    fn an_unreadable_max_size_warns_once_and_leaves_entries_uncapped() {
        // Nothing on this path runs `validate_config` — its only caller is
        // `daft hooks validate` — so a typo'd cap would otherwise evaporate in
        // silence. Degrading to uncapped is right; degrading quietly is not.
        let tmp = TempDir::new().unwrap();
        write(
            &tmp.path().join("daft.yml"),
            b"copy:\n  paths: [target, node_modules]\n  max_size: five gigabytes\n",
        );
        let resolved = read_copy_config(tmp.path()).unwrap();
        assert_eq!(resolved.max_size_bytes, None);
        assert_eq!(
            resolved.max_size_unparsed.as_deref(),
            Some("five gigabytes")
        );

        let (_tmp2, source, target) = repo_fixture("/target\n");
        let mut sink = crate::core::RecordingStageSink::default();
        copy_entries(&source, &target, &resolved, false, &mut sink);

        assert_eq!(
            sink.warnings.len(),
            1,
            "one warning for the stage, not one per entry: {:?}",
            sink.warnings
        );
        assert!(
            sink.warnings[0].contains("five gigabytes") && sink.warnings[0].contains("max_size"),
            "the warning has to quote what could not be read: {}",
            sink.warnings[0]
        );
    }

    // ── Destination-root containment ─────────────────────────────────────

    /// An entry that resolves to the destination *root* is refused outright.
    ///
    /// Two routes reached it: `.` (and `sub/..`, and the empty string), whose
    /// `target.join(...)` IS the destination worktree; and a glob's walk root,
    /// which used to be reported as an empty-string match so `copy: ["*"]`
    /// expanded to `["", …]`. Under `--force` either one handed the whole
    /// worktree to the removal step.
    ///
    /// Both were previously saved only by accident — `check-ignore -- ""`
    /// exits 128, and `ls-files -- .` is non-empty in any repo with a commit —
    /// so the fixture here removes the second accident (a repository with
    /// nothing tracked yet) and asserts containment holds on its own.
    #[test]
    fn an_entry_that_resolves_to_the_destination_root_is_refused() {
        let (_tmp, source, target) = repo_fixture("*\n");
        write(&source.join("target/app"), b"cache");
        write(&target.join("precious.txt"), b"the user's work");

        let result = copy_entries(&source, &target, &config(&["."]), true, &mut NullSink);

        assert!(
            target.join("precious.txt").is_file(),
            "the copy stage emptied the destination worktree — it now holds {:?}",
            entries_of(&target)
        );
        assert_sole_skip!(result, SkipReason::Uncontained { .. });

        // Every spelling of "the worktree itself", refused before git is asked.
        for entry in [".", "", "sub/.."] {
            assert!(
                containment_violation(entry).is_some(),
                "{entry:?} names the worktree root"
            );
        }

        // The same hazard by the other route: the walk's root is not one of
        // its own matches, and an empty relative path is not a path.
        write(&source.join("cache/a.bin"), b"a");
        assert!(
            !expand_entries(&source, "*").iter().any(String::is_empty),
            "a root-matching glob expanded to the destination root itself: {:?}",
            expand_entries(&source, "*")
        );
    }
}
