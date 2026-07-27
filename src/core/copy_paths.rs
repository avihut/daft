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
//! AND nothing under it may be tracked (`git ls-files <entry>` empty) — the
//! second probe catches a force-added file inside an otherwise-ignored
//! directory. Copying tracked content would duplicate the working tree git
//! is already managing.
//!
//! **One row per config ENTRY, not per expanded match.** A glob entry that
//! matches thirty directories is still one plan row; the fan-out lands in the
//! row's annotation. This keeps the plan face walk-free (no filesystem
//! traversal before the plan commits) and the reconcile keys stable.
//!
//! ## Status
//!
//! The public surface below is the API contract the parallel #387 tracks
//! build against; the function **bodies land in the engine track** and are
//! inert here (empty results, no filesystem access). Every doc comment states
//! the full behavioral contract — implement to the doc, not to the current
//! body.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

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
    /// Declared entries in config order, with trailing slashes stripped
    /// (`target/` → `target`) so a path is written exactly one way from here
    /// on — the rail label, the `StepKey` scope, and the `dst.join(entry)`
    /// all agree. Entries are worktree-root-relative and may contain glob
    /// metacharacters; see [`expand_entries`].
    pub paths: Vec<String>,
    /// What to do with an entry the filesystem cannot reflink. Defaulted to
    /// [`CopyFallback::Copy`] when the config did not say.
    pub fallback: CopyFallback,
    /// Per-entry size cap in bytes, parsed from the config's `max_size`
    /// string. `None` = uncapped. Gates the **byte-copy fallback only** — a
    /// reflink is near-free and is never size-checked.
    pub max_size_bytes: Option<u64>,
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
/// 2. Every entry loses its trailing `/`; entries that are empty or become
///    empty are dropped.
/// 3. `fallback` defaults to [`CopyFallback::Copy`].
/// 4. `max_size` is parsed to bytes via
///    `crate::coordinator::clean_policy::parse_size` (case-insensitive,
///    binary multiples, bare integer = bytes). An unparseable value — which
///    `validate_config` already reports as a config error — degrades to
///    `None` (uncapped) rather than failing the read.
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

    let paths: Vec<String> = copy
        .paths()
        .iter()
        .map(|p| p.trim_end_matches('/').to_string())
        .filter(|p| !p.is_empty())
        .collect();
    if paths.is_empty() {
        return None;
    }

    Some(ResolvedCopyConfig {
        paths,
        fallback: copy.fallback(),
        // An unparseable cap degrades to uncapped rather than to zero: the
        // validator has already told the user their `max_size` is wrong, and
        // silently gating every entry to 0 bytes would turn a typo into a
        // stage that mysteriously copies nothing.
        max_size_bytes: copy
            .max_size()
            .and_then(|raw| crate::coordinator::clean_policy::parse_size(raw).ok()),
    })
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
    if !is_glob(entry) {
        return vec![entry.to_string()];
    }

    let mut builder = OverrideBuilder::new(source_root);
    // A pattern the glob compiler rejects expands to nothing; the caller
    // reports that as an expected skip rather than failing the creation.
    if builder.add(entry).is_err() {
        return Vec::new();
    }
    let Ok(overrides) = builder.build() else {
        return Vec::new();
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
    for result in walker.build() {
        // A walk error is an unreadable subtree, not a failure: a cache we
        // cannot read is a cache we cannot copy, and creation never hinges on
        // one.
        let Ok(found) = result else { continue };
        let is_dir = found.file_type().is_some_and(|t| t.is_dir());
        if !overrides.matched(found.path(), is_dir).is_whitelist() {
            continue;
        }
        let Ok(rel) = found.path().strip_prefix(source_root) else {
            continue;
        };
        // A non-UTF-8 name cannot round-trip through a config `String`, the
        // rail label, or the `StepKey` scope — skip it rather than lose it.
        if let Some(rel) = rel.to_str() {
            matches.push(rel.to_string());
        }
    }
    matches.sort();
    matches.dedup();
    matches
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
}

/// Why one entry was left out. Each variant is a self-contained phrase in
/// [`report_copy_results`] — no `skipped — ` prefix is added, so the reason
/// must read as a complete clause on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// The entry does not exist in the source worktree. The quiet case: a
    /// declared cache that has simply never been built yet.
    NoSource,
    /// The entry already exists at the destination. The **idempotence**
    /// case that makes re-runs and hook-job composition safe: `daft warm`
    /// twice in a row is a no-op, and `copy:` never clobbers work a
    /// post-create hook already did. `daft warm --force` removes the
    /// destination first, so this outcome does not arise there.
    DestinationExists,
    /// The entry is tracked by git, or something under it is (a force-added
    /// file inside an ignored directory). `copy:` replicates caches, not the
    /// working tree — the attention case, because the config asked for
    /// something daft refuses to do.
    NotIgnored,
    /// The filesystem cannot reflink this entry and `fallback: skip` said not
    /// to pay for a byte copy.
    NoReflink,
    /// The entry's byte-copy fallback would exceed
    /// [`ResolvedCopyConfig::max_size_bytes`]. Carries the measured size and
    /// the cap, both in bytes, so the row can say how far over it was.
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
    /// (`**/dist/ → 3 dirs · 1.2 GB · reflinked · 0.3s`).
    Copied {
        entry: String,
        method: CopyMethod,
        matches: usize,
        bytes: u64,
        elapsed: Duration,
    },
    /// The entry was deliberately left out; see [`SkipReason`].
    Skipped { entry: String, reason: SkipReason },
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
///    [`SkipReason::NoMatches`].
/// 2. **Source missing** → [`SkipReason::NoSource`].
/// 3. **Gitignored check** against `source`: `git check-ignore` must pass AND
///    `git ls-files <entry>` must be empty. Either probe failing →
///    [`SkipReason::NotIgnored`]. Run git through
///    `crate::utils::git_command_at(source)` with both pipes nulled — an
///    inherited `GIT_DIR` silently overrides `-C`, and a stray
///    `fatal: not a git repository` on stderr would corrupt the rail.
/// 4. **Destination exists** → [`SkipReason::DestinationExists`]. This is
///    what makes the stage idempotent; `force` skips this check and removes
///    the destination first.
/// 5. **Probe reflink** by attempting `reflink_copy::reflink` on the first
///    regular file under the entry (into a temporary destination that is
///    removed immediately). Probing beats querying the filesystem type: the
///    same `copy:` entry can straddle mount points, and the attempt is the
///    only honest answer.
/// 6. **Reflink available** → `cow_copy::copy_dir` / `cow_copy::copy_file`,
///    reported as [`CopyMethod::Reflinked`], never size-gated.
/// 7. **No reflink**, `fallback: skip` → [`SkipReason::NoReflink`].
/// 8. **No reflink**, `fallback: copy` → pre-walk the entry's apparent size;
///    over [`ResolvedCopyConfig::max_size_bytes`] →
///    [`SkipReason::TooLarge`], otherwise byte-copy through the same
///    `cow_copy` entry points (which degrade per file correctly) and report
///    [`CopyMethod::Copied`].
///
/// `force` (`daft warm --force`) removes an existing destination entry before
/// copying instead of skipping it — `cow_copy::copy_dir` requires an absent
/// destination.
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
    let _ = (source, target, config, force, sink);
    CopyPathsResult::default()
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
/// | [`CopyOutcome::Copied`] | `Completed { annotation }` | green `✓`, annotated `3 dirs · 1.2 GB · reflinked · 0.3s` |
/// | [`SkipReason::NoSource`] / [`SkipReason::DestinationExists`] / [`SkipReason::NoMatches`] | `SkippedExpected` | dim |
/// | [`SkipReason::NotIgnored`] / [`SkipReason::NoReflink`] / [`SkipReason::TooLarge`] | `SkippedAttention` | yellow |
/// | [`CopyOutcome::Failed`] | `SkippedAttention` | yellow |
///
/// **Never `Failed`.** A `Failed` face says the operation the user asked for
/// did not happen; here the operation is the worktree, and it did. A cache
/// that did not copy is an attention skip, which is why the engine's
/// warn-never-abort contract and this table have to agree.
///
/// Skip reasons render without a `skipped — ` prefix (the timeline exempts
/// [`StageId::CopyPath`](crate::core::stage::StageId::CopyPath) along with
/// `SharedFile`), so each one must read as a complete phrase — e.g.
/// `'target' is tracked — not copied`, `nothing to copy yet`,
/// `already present`, `2.1 GB over the 1 GB max_size`.
///
/// A planned entry that produced no outcome at all resolves as
/// `SkippedSilent` and its row is removed — the finished rail lists only
/// entries that actually resolved.
pub fn report_copy_results(
    result: &CopyPathsResult,
    planned: &[String],
    sink: &mut impl crate::core::ProgressSink,
) {
    let _ = (result, planned, sink);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Write `contents` at `path`, creating parents. Fixtures only.
    fn write(path: &Path, contents: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    /// A bare filesystem fixture: no git, no store, no worktrees — just a
    /// source directory and a target directory. Everything the copy engine
    /// does apart from the gitignored probe works on paths alone.
    fn fs_fixture() -> (TempDir, PathBuf, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("source");
        let target = tmp.path().join("target");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        (tmp, source, target)
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
                },
                CopyOutcome::Skipped {
                    entry: "node_modules".into(),
                    reason: SkipReason::DestinationExists,
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
}
