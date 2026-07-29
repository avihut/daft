//! Which worktree a new one's build caches are copied *from*.
//!
//! `copy:` first shipped reusing [`resolve_source_worktree`], the resolver
//! written for **visitor config propagation**. That resolver answers "where
//! does the user's untracked `daft.yml` live?", and its answer — the worktree
//! you are standing in — is right for that question. Caches are a different
//! question. `daft start feature-b master` says which content the new worktree
//! will hold; `master`'s caches are the ones that match it, wherever the
//! command was typed. Sharing one resolver made the explicitly-named base lose
//! to cwd every time, silently.
//!
//! So candidate sources are ranked by **specificity against what the new
//! worktree will contain**:
//!
//! 1. **The anchor branch's own worktree** — same branch, same content.
//! 2. **Any worktree whose HEAD is the identical commit** — same content under
//!    a different name. Detached sandboxes and forks count: content identity
//!    is the whole criterion, and a fork is content-identical by construction.
//! 3. **The propagation source** — the worktree you are standing in. The
//!    previous behaviour, kept as the floor.
//!
//! Rank 2 ties are broken by *standing here* first — zero surprise, and the
//! tree you are in is the one you most recently built — then by **warmth**,
//! the tiebreak of last resort ([`Warmth`]).
//!
//! One deliberate asymmetry: the **configuration** is still read from the
//! propagation source. A visitor's untracked `daft.local.yml` declaring
//! `copy:` lives where *they* are standing, and reading config out of
//! `master`'s worktree instead would silently drop it. Config comes from where
//! you are; bytes come from what matches.
//!
//! Nothing here is allowed to fail a creation. A resolution that cannot be
//! computed — unlistable worktrees, an unresolvable anchor — falls back to the
//! propagation source, because the copy stage is an optimization and an
//! optimization never costs you the worktree you asked for.
//!
//! [`resolve_source_worktree`]: crate::core::worktree::checkout_branch

use crate::core::worktree::porcelain::{WorktreeListEntry, parse_worktree_list_porcelain};
use crate::git::GitCommand;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// What the new worktree will hold — the thing candidates are ranked against.
///
/// Both fields are optional because the journeys know different things: `daft
/// start feature-b master` knows a branch *and* a commit, `daft go <sha>` and
/// `daft start --fork` know only a commit, and a journey that can resolve
/// neither falls straight through to the propagation source.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CopyAnchor {
    /// The branch whose content the new worktree takes: the base branch for
    /// `daft start`, the branch being materialized for `daft go`.
    ///
    /// For `daft go` this is vacuous by construction — a branch that already
    /// has a worktree is a navigation, not a creation — and passing it costs
    /// one failed lookup rather than a special case at each call site.
    pub branch: Option<String>,
    /// The commit the new worktree will sit at.
    pub commit: Option<String>,
}

impl CopyAnchor {
    /// An anchor that ranks nothing — every journey's "I could not resolve
    /// this" case, which lands on the propagation source.
    pub fn none() -> Self {
        Self::default()
    }
}

/// Why a particular worktree was chosen, so the rail can say it.
///
/// The variants are what the user is told, so they must be honest: a tiebreak
/// is only reported when a tiebreak actually happened. A sole candidate is
/// [`SameCommit`](Self::SameCommit) even when it is also where you stand,
/// because nothing was broken in its favour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopySourceReason {
    /// Rank 1: the anchor branch has a resident worktree.
    AnchorBranch,
    /// Rank 2, uncontested: exactly one worktree sits at the anchor commit.
    SameCommit,
    /// Rank 2, tie broken by the command's own cwd.
    SameCommitStanding,
    /// Rank 2, tie broken by warmth.
    SameCommitWarmest,
    /// Rank 3: nothing more specific — the propagation source.
    Standing,
}

/// The resolved cache source: where the bytes come from, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopySource {
    /// The worktree root to copy out of.
    pub path: PathBuf,
    /// How to name it to the user — the branch when there is one, the
    /// directory name for a detached worktree.
    pub label: String,
    /// Which rung of the ladder produced it.
    pub reason: CopySourceReason,
}

impl CopySource {
    /// The provenance clause the rail appends to the `copied paths` group.
    ///
    /// Always names the source; adds the qualifier only when the resolution
    /// was something other than "the obvious one", since a qualifier on every
    /// row is noise that stops being read.
    pub fn provenance_phrase(&self) -> String {
        let label = &self.label;
        match self.reason {
            CopySourceReason::AnchorBranch | CopySourceReason::Standing => {
                format!("from '{label}'")
            }
            CopySourceReason::SameCommit => format!("from '{label}' · same commit"),
            CopySourceReason::SameCommitStanding => {
                format!("from '{label}' · same commit, where you are")
            }
            CopySourceReason::SameCommitWarmest => {
                format!("from '{label}' · same commit, warmest")
            }
        }
    }
}

/// How warm a candidate's declared caches look.
///
/// The tiebreak of last resort among worktrees holding *identical* content,
/// where nothing about git can separate them and the only remaining question
/// is which one has actually been built in.
///
/// Ordered by presence first: a worktree that has never built has nothing to
/// give, however recently something in it was touched. Only then by recency.
/// `derive(Ord)` gives exactly that, in field order — do not reorder the
/// fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Warmth {
    /// How many declared entries exist at all in this worktree.
    pub present: usize,
    /// Newest modification time across those entries, in seconds since the
    /// epoch. An ordering key, not a timestamp anyone reads.
    pub newest: u64,
}

/// Score a candidate worktree's declared caches.
///
/// Two deliberate approximations, both bought for the same reason — this runs
/// on the worktree-creation hot path and a `target/` is routinely gigabytes:
///
/// - **Depth one.** A directory's own mtime does not move when a build
///   rewrites files *inside* a subdirectory, so the entry alone would report
///   every cargo tree as equally cold. Its immediate children are read too,
///   which catches `target/debug` being rebuilt. Deeper would mean walking the
///   tree, which is precisely the cost `copy:` exists to avoid.
/// - **Globs are skipped.** Scoring `**/dist/` means expanding it, and
///   expansion is a full walk. A config whose entries are all globs therefore
///   scores zero everywhere and the tie falls through to path order, which is
///   deterministic and honest — the rail reports no warmth tiebreak, because
///   none happened.
pub fn measure_warmth(root: &Path, declared: &[String]) -> Warmth {
    let mut warmth = Warmth::default();

    for entry in declared {
        if entry.contains(['*', '?', '[']) {
            continue;
        }
        let path = root.join(entry);
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        warmth.present += 1;
        warmth.newest = warmth.newest.max(mtime_secs(&meta));

        if meta.is_dir()
            && let Ok(children) = std::fs::read_dir(&path)
        {
            for child in children.flatten() {
                if let Ok(child_meta) = std::fs::symlink_metadata(child.path()) {
                    warmth.newest = warmth.newest.max(mtime_secs(&child_meta));
                }
            }
        }
    }

    warmth
}

fn mtime_secs(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Resolve the cache source, doing the IO.
///
/// `standing` is the propagation source (what `resolve_source_worktree`
/// returned) and doubles as the floor: every failure path lands there.
/// `destination` is the worktree being written into, excluded from the
/// candidates so `daft warm` can never resolve its own target as its source.
pub fn resolve(
    git: &GitCommand,
    anchor: &CopyAnchor,
    standing: &Path,
    destination: &Path,
    declared: &[String],
) -> CopySource {
    let Ok(porcelain) = git.worktree_list_porcelain() else {
        return standing_source(&[], standing);
    };
    let entries = parse_worktree_list_porcelain(&porcelain);
    rank(&entries, anchor, standing, destination, &|path| {
        measure_warmth(path, declared)
    })
}

/// The ranking itself: pure, so every rung and both tiebreaks are testable
/// without a filesystem — including the warmth tie, which is otherwise a
/// mtime race to provoke.
pub fn rank(
    entries: &[WorktreeListEntry],
    anchor: &CopyAnchor,
    standing: &Path,
    destination: &Path,
    warmth_of: &dyn Fn(&Path) -> Warmth,
) -> CopySource {
    // The bare entry has no working tree to read, and the destination must
    // never be its own source.
    let usable: Vec<&WorktreeListEntry> = entries
        .iter()
        .filter(|e| !e.is_bare && !same_path(&e.path, destination))
        .collect();

    // ── Rank 1: the anchor branch's own worktree ─────────────────────────
    //
    // Matched on the porcelain `branch` field rather than through
    // `find_worktree_for_branch`, which additionally rescues a worktree
    // paused mid-rebase (git detaches HEAD to replay). A tree mid-rebase is
    // a poor thing to copy caches out of, so not rescuing it here is the
    // wanted behaviour, not an oversight.
    if let Some(branch) = anchor.branch.as_deref()
        && let Some(entry) = usable.iter().find(|e| e.branch.as_deref() == Some(branch))
    {
        return source_of(entry, CopySourceReason::AnchorBranch);
    }

    // ── Rank 2: any worktree holding the identical commit ────────────────
    if let Some(commit) = anchor.commit.as_deref() {
        let mut at_commit: Vec<&WorktreeListEntry> = usable
            .iter()
            .copied()
            .filter(|e| e.head.as_deref() == Some(commit))
            .collect();
        // Deterministic base order, so an unbreakable tie still resolves the
        // same way on every machine and every run.
        at_commit.sort_by(|a, b| a.path.cmp(&b.path));

        match at_commit.len() {
            0 => {}
            1 => return source_of(at_commit[0], CopySourceReason::SameCommit),
            _ => {
                if let Some(entry) = at_commit.iter().find(|e| same_path(&e.path, standing)) {
                    return source_of(entry, CopySourceReason::SameCommitStanding);
                }

                let mut scored: Vec<(Warmth, &WorktreeListEntry)> =
                    at_commit.iter().map(|e| (warmth_of(&e.path), *e)).collect();
                // Stable sort on a path-ordered input: equal warmth keeps
                // path order.
                scored.sort_by_key(|(warmth, _)| std::cmp::Reverse(*warmth));

                // Only claim a warmth tiebreak when warmth actually broke it.
                let decided = scored[0].0 > scored[1].0;
                let reason = if decided {
                    CopySourceReason::SameCommitWarmest
                } else {
                    CopySourceReason::SameCommit
                };
                return source_of(scored[0].1, reason);
            }
        }
    }

    // ── Rank 3: the propagation source ───────────────────────────────────
    standing_source(entries, standing)
}

fn source_of(entry: &WorktreeListEntry, reason: CopySourceReason) -> CopySource {
    CopySource {
        path: entry.path.clone(),
        label: label_of(entry),
        reason,
    }
}

fn standing_source(entries: &[WorktreeListEntry], standing: &Path) -> CopySource {
    let label = entries
        .iter()
        .find(|e| same_path(&e.path, standing))
        .map(label_of)
        .unwrap_or_else(|| dir_name(standing));
    CopySource {
        path: standing.to_path_buf(),
        label,
        reason: CopySourceReason::Standing,
    }
}

fn label_of(entry: &WorktreeListEntry) -> String {
    entry
        .branch
        .clone()
        .unwrap_or_else(|| dir_name(&entry.path))
}

fn dir_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Whether two paths name the same directory, resolving symlinks.
///
/// macOS `/tmp` is a symlink to `/private/tmp`, and git's porcelain reports
/// the resolved form while a cwd-derived path may not, so a plain `==` would
/// call a worktree and itself different.
pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    fn canonical(path: &Path) -> PathBuf {
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    }
    canonical(a) == canonical(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, branch: Option<&str>, head: Option<&str>) -> WorktreeListEntry {
        WorktreeListEntry {
            path: PathBuf::from(path),
            branch: branch.map(str::to_string),
            is_bare: false,
            is_detached: branch.is_none(),
            head: head.map(str::to_string),
        }
    }

    fn cold(_: &Path) -> Warmth {
        Warmth::default()
    }

    /// The bug this module exists to fix: `daft start feature-b master` run
    /// from inside feature-a used to copy feature-a's caches.
    #[test]
    fn the_named_base_branch_outranks_the_worktree_you_are_standing_in() {
        let entries = vec![
            entry("/p/feature-a", Some("feature-a"), Some("aaa")),
            entry("/p/master", Some("master"), Some("mmm")),
        ];
        let anchor = CopyAnchor {
            branch: Some("master".into()),
            commit: Some("mmm".into()),
        };

        let resolved = rank(
            &entries,
            &anchor,
            Path::new("/p/feature-a"),
            Path::new("/p/feature-b"),
            &cold,
        );

        assert_eq!(resolved.path, PathBuf::from("/p/master"));
        assert_eq!(resolved.label, "master");
        assert_eq!(resolved.reason, CopySourceReason::AnchorBranch);
    }

    #[test]
    fn an_identical_tip_stands_in_when_the_branch_has_no_worktree() {
        // `master` itself has no worktree, but a release worktree sits at
        // exactly its commit — same tree, different name.
        let entries = vec![
            entry("/p/feature-a", Some("feature-a"), Some("aaa")),
            entry("/p/release", Some("release"), Some("mmm")),
        ];
        let anchor = CopyAnchor {
            branch: Some("master".into()),
            commit: Some("mmm".into()),
        };

        let resolved = rank(
            &entries,
            &anchor,
            Path::new("/p/feature-a"),
            Path::new("/p/new"),
            &cold,
        );

        assert_eq!(resolved.path, PathBuf::from("/p/release"));
        assert_eq!(resolved.reason, CopySourceReason::SameCommit);
    }

    #[test]
    fn a_detached_sandbox_is_an_eligible_identical_tip() {
        let entries = vec![
            entry("/p/feature-a", Some("feature-a"), Some("aaa")),
            entry("/p/sandbox-3", None, Some("mmm")),
        ];
        let anchor = CopyAnchor {
            branch: None,
            commit: Some("mmm".into()),
        };

        let resolved = rank(
            &entries,
            &anchor,
            Path::new("/p/feature-a"),
            Path::new("/p/new"),
            &cold,
        );

        assert_eq!(resolved.path, PathBuf::from("/p/sandbox-3"));
        assert_eq!(
            resolved.label, "sandbox-3",
            "a detached worktree is named by its directory — it has no branch"
        );
    }

    #[test]
    fn a_tie_at_the_same_commit_prefers_where_you_are_standing() {
        let entries = vec![
            entry("/p/a", Some("a"), Some("mmm")),
            entry("/p/b", Some("b"), Some("mmm")),
            entry("/p/c", Some("c"), Some("mmm")),
        ];
        let anchor = CopyAnchor {
            branch: None,
            commit: Some("mmm".into()),
        };

        // `/p/c` would lose on path order and is scored stone cold; standing
        // in it still wins, because the tie is broken before warmth is asked.
        let resolved = rank(
            &entries,
            &anchor,
            Path::new("/p/c"),
            Path::new("/p/new"),
            &|_| Warmth::default(),
        );

        assert_eq!(resolved.path, PathBuf::from("/p/c"));
        assert_eq!(resolved.reason, CopySourceReason::SameCommitStanding);
    }

    #[test]
    fn a_tie_you_are_not_part_of_is_broken_by_warmth() {
        let entries = vec![
            entry("/p/a", Some("a"), Some("mmm")),
            entry("/p/b", Some("b"), Some("mmm")),
        ];
        let anchor = CopyAnchor {
            branch: None,
            commit: Some("mmm".into()),
        };

        // `/p/b` loses on path order and wins on warmth.
        let resolved = rank(
            &entries,
            &anchor,
            Path::new("/p/elsewhere"),
            Path::new("/p/new"),
            &|path| {
                if path == Path::new("/p/b") {
                    Warmth {
                        present: 2,
                        newest: 100,
                    }
                } else {
                    Warmth {
                        present: 1,
                        newest: 999,
                    }
                }
            },
        );

        assert_eq!(
            resolved.path,
            PathBuf::from("/p/b"),
            "presence outranks recency: a tree with more caches beats a \
             fresher-but-emptier one"
        );
        assert_eq!(resolved.reason, CopySourceReason::SameCommitWarmest);
    }

    #[test]
    fn an_unbreakable_tie_resolves_deterministically_and_claims_no_tiebreak() {
        let entries = vec![
            entry("/p/z", Some("z"), Some("mmm")),
            entry("/p/a", Some("a"), Some("mmm")),
        ];
        let anchor = CopyAnchor {
            branch: None,
            commit: Some("mmm".into()),
        };

        let resolved = rank(
            &entries,
            &anchor,
            Path::new("/p/elsewhere"),
            Path::new("/p/new"),
            &cold,
        );

        assert_eq!(resolved.path, PathBuf::from("/p/a"), "path order decides");
        assert_eq!(
            resolved.reason,
            CopySourceReason::SameCommit,
            "nothing was broken in its favour, so the rail must not say it was"
        );
    }

    #[test]
    fn nothing_matches_and_the_propagation_source_is_the_floor() {
        let entries = vec![entry("/p/feature-a", Some("feature-a"), Some("aaa"))];
        let anchor = CopyAnchor {
            branch: Some("master".into()),
            commit: Some("mmm".into()),
        };

        let resolved = rank(
            &entries,
            &anchor,
            Path::new("/p/feature-a"),
            Path::new("/p/new"),
            &cold,
        );

        assert_eq!(resolved.path, PathBuf::from("/p/feature-a"));
        assert_eq!(resolved.label, "feature-a");
        assert_eq!(resolved.reason, CopySourceReason::Standing);
    }

    #[test]
    fn the_destination_is_never_its_own_source() {
        // `daft warm` on a worktree that is itself at the anchor commit: the
        // target must be excluded or the copy would be a self-copy.
        let entries = vec![
            entry("/p/target", Some("target"), Some("mmm")),
            entry("/p/other", Some("other"), Some("mmm")),
        ];
        let anchor = CopyAnchor {
            branch: Some("target".into()),
            commit: Some("mmm".into()),
        };

        let resolved = rank(
            &entries,
            &anchor,
            Path::new("/p/elsewhere"),
            Path::new("/p/target"),
            &cold,
        );

        assert_eq!(resolved.path, PathBuf::from("/p/other"));
    }

    #[test]
    fn the_bare_entry_is_never_a_candidate() {
        let mut bare = entry("/p/.git", None, Some("mmm"));
        bare.is_bare = true;
        bare.is_detached = false;
        let entries = vec![bare, entry("/p/main", Some("main"), Some("mmm"))];
        let anchor = CopyAnchor {
            branch: None,
            commit: Some("mmm".into()),
        };

        let resolved = rank(
            &entries,
            &anchor,
            Path::new("/p/elsewhere"),
            Path::new("/p/new"),
            &cold,
        );

        assert_eq!(resolved.path, PathBuf::from("/p/main"));
    }

    #[test]
    fn an_anchorless_journey_lands_on_the_propagation_source() {
        let entries = vec![entry("/p/feature-a", Some("feature-a"), Some("aaa"))];

        let resolved = rank(
            &entries,
            &CopyAnchor::none(),
            Path::new("/p/feature-a"),
            Path::new("/p/new"),
            &cold,
        );

        assert_eq!(resolved.reason, CopySourceReason::Standing);
    }

    #[test]
    fn provenance_phrases_name_the_source_and_qualify_only_a_real_tiebreak() {
        let make = |reason| CopySource {
            path: PathBuf::from("/p/x"),
            label: "master".into(),
            reason,
        };
        assert_eq!(
            make(CopySourceReason::AnchorBranch).provenance_phrase(),
            "from 'master'"
        );
        assert_eq!(
            make(CopySourceReason::Standing).provenance_phrase(),
            "from 'master'"
        );
        assert_eq!(
            make(CopySourceReason::SameCommit).provenance_phrase(),
            "from 'master' · same commit"
        );
        assert_eq!(
            make(CopySourceReason::SameCommitStanding).provenance_phrase(),
            "from 'master' · same commit, where you are"
        );
        assert_eq!(
            make(CopySourceReason::SameCommitWarmest).provenance_phrase(),
            "from 'master' · same commit, warmest"
        );
    }

    #[test]
    fn warmth_counts_presence_first_then_recency_one_level_down() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        // An entry that exists but is empty, and one that does not exist.
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join("target/debug/binary"), b"x").unwrap();

        let warmth = measure_warmth(root, &["target".to_string(), "node_modules".to_string()]);
        assert_eq!(warmth.present, 1, "only one of the two entries exists");
        assert!(
            warmth.newest > 0,
            "a child one level down carries the mtime the directory itself does not"
        );

        // A glob contributes nothing — expanding it would be a full walk.
        let globbed = measure_warmth(root, &["**/dist".to_string()]);
        assert_eq!(globbed, Warmth::default());
    }

    #[test]
    fn warmth_orders_by_presence_before_recency() {
        let fuller = Warmth {
            present: 2,
            newest: 1,
        };
        let fresher = Warmth {
            present: 1,
            newest: u64::MAX,
        };
        assert!(fuller > fresher);
    }
}
