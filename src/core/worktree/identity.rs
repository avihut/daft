//! Resolving what branch a worktree *is*, when git will not say.
//!
//! `git worktree list --porcelain` names a branch only while HEAD is attached.
//! Git detaches HEAD for the entire duration of a rebase, so for as long as an
//! interrupted rebase sits unresolved — often the exact moment someone runs
//! `daft list` to get their bearings — the porcelain reports the worktree as
//! detached and nothing else. Everything keyed on the branch name then goes
//! blank: ahead/behind, owner, branch age, PR linkage; the row sorts out of
//! place; and it gets classified as a throwaway sandbox.
//!
//! This module puts the name back, from evidence git keeps but does not print.
//!
//! # Resolution order
//!
//! 1. **Attached** — the porcelain names a branch. Always wins.
//! 2. **Recovered** — HEAD is detached, but an in-progress operation records
//!    the branch it is replaying ([`crate::git::op_state`]).
//! 3. **None** — nothing names a branch: a genuine detached checkout.
//!
//! The ordering is the whole design. Live git state cannot be stale, so it
//! outranks anything remembered; and every attached worktree claims its name
//! before any recovery is attempted, so a recovered name can never displace a
//! real checkout. (A persisted tier lands between 2 and 3 in a later commit;
//! it is the only one that can be out of date, which is exactly why it ranks
//! last.)
//!
//! An operation also *explains* a detachment even when it records no branch —
//! `git am` writes no `head-name`. Such a worktree keeps `(detached)` as its
//! name but is not a sandbox: something is happening in it.

use super::porcelain::WorktreeListEntry;
use crate::git::op_state::{OpKind, probe_op_state};
use crate::store::models::WorktreeIdentityRow;
use std::collections::{HashMap, HashSet};

/// The name shown when nothing can name the branch.
pub const DETACHED_LABEL: &str = "(detached)";

/// Where a worktree's branch identity came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentitySource {
    /// Git reports the branch as checked out.
    Attached,
    /// HEAD is detached; an in-progress operation records the branch.
    Recovered,
    /// HEAD is detached with nothing to explain it, but daft recorded what
    /// this worktree is for. The only tier that can be out of date, which is
    /// why it ranks last.
    Persisted,
    /// Nothing names a branch.
    None,
}

impl IdentitySource {
    /// Stable machine-facing name, as emitted in structured output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Attached => "attached",
            Self::Recovered => "recovered",
            Self::Persisted => "persisted",
            Self::None => "none",
        }
    }
}

/// A worktree's resolved identity and operation state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeIdentity {
    /// Display name: the branch, or [`DETACHED_LABEL`].
    pub name: String,
    /// The resolved branch, when there is one. Everything branch-keyed
    /// (ahead/behind, owner, age, upstream) should key on this rather than on
    /// the porcelain's branch — mid-rebase `refs/heads/<branch>` still exists
    /// and still points at the pre-rebase tip, so those queries stay valid.
    pub branch: Option<String>,
    pub source: IdentitySource,
    /// The operation running in this worktree, if any. Present for attached
    /// worktrees too: a merge and a cherry-pick never detach HEAD.
    pub op: Option<OpKind>,
    /// HEAD is detached and nothing explains why — a scratch checkout of a
    /// tag or SHA. Narrower than "detached": a worktree mid-operation is not
    /// a sandbox, which is what keeps it out of the dimmed, filtered-away
    /// class it used to fall into the moment a rebase started.
    ///
    /// A worktree whose name came from the persisted record is still a
    /// sandbox: the record names it, but nothing *explains the detachment*,
    /// which is what this flag is about.
    pub is_sandbox: bool,
    /// The recorded branch disagrees with the branch actually checked out.
    /// Someone checked out a different branch into this worktree, or renamed
    /// one outside `daft rename`. Derived state still wins the name — this
    /// only surfaces that the record has fallen behind.
    pub drifted: bool,
}

impl WorktreeIdentity {
    /// The identity of a worktree nothing can name.
    fn unnamed(op: Option<OpKind>) -> Self {
        Self {
            name: DETACHED_LABEL.to_string(),
            branch: None,
            source: IdentitySource::None,
            // An operation explains the detachment even when it names no
            // branch (`git am` records none), so such a worktree is not a
            // sandbox — it is mid-flight.
            is_sandbox: op.is_none(),
            op,
            drifted: false,
        }
    }
}

/// Resolve identity and operation state for each porcelain entry, consulting
/// only live git state.
///
/// Bare entries yield `None` so callers can `zip` the result with their entry
/// list positionally without filtering first.
pub fn resolve_identities(entries: &[WorktreeListEntry]) -> Vec<Option<WorktreeIdentity>> {
    resolve_identities_with(entries, &HashMap::new())
}

/// [`resolve_identities`], plus the persisted records as a last-resort tier
/// and a cross-check for drift.
///
/// `records` is keyed by private-gitdir id (see
/// [`super::identity_store::read_identities`]). It is consulted only where
/// live git state has said nothing, and it never overrides an operation: the
/// record can be stale, an in-progress rebase cannot.
pub fn resolve_identities_with(
    entries: &[WorktreeListEntry],
    records: &HashMap<String, WorktreeIdentityRow>,
) -> Vec<Option<WorktreeIdentity>> {
    // Pass 1: every attached worktree claims its branch name, before any
    // recovery runs. Recovery then cannot hand a name to a second row and
    // produce two rows claiming one branch — which would break everything
    // keyed by row name downstream (live-table patch routing, the
    // `--branches` dedup, the size cache's per-branch slug).
    let claimed: HashSet<&str> = entries
        .iter()
        .filter(|e| !e.is_bare)
        .filter_map(|e| e.branch.as_deref())
        .collect();

    entries
        .iter()
        .map(|entry| {
            if entry.is_bare {
                return None;
            }

            if let Some(branch) = &entry.branch {
                // Cross-check, never an override: a record that disagrees
                // with a live checkout is the thing that is wrong.
                let drifted = recorded_branch(records, &entry.path)
                    .is_some_and(|recorded| recorded != *branch);
                return Some(WorktreeIdentity {
                    name: branch.clone(),
                    branch: Some(branch.clone()),
                    source: IdentitySource::Attached,
                    // A merge or cherry-pick keeps HEAD attached, so an
                    // attached worktree can still be mid-operation.
                    op: probe_op_state(&entry.path).map(|s| s.kind),
                    is_sandbox: false,
                    drifted,
                });
            }

            let Some(state) = probe_op_state(&entry.path) else {
                // Nothing live explains this detachment. Fall back to what
                // daft recorded the worktree was for — the row keeps its
                // name and its branch-keyed cells, and stays a sandbox,
                // because being *named* and being *explained* are different
                // things.
                return Some(match recorded_branch(records, &entry.path) {
                    Some(branch) if !claimed.contains(branch.as_str()) => WorktreeIdentity {
                        name: branch.clone(),
                        branch: Some(branch),
                        source: IdentitySource::Persisted,
                        op: None,
                        is_sandbox: true,
                        drifted: false,
                    },
                    // A record naming a branch that is genuinely checked out
                    // elsewhere is stale. Report the disagreement rather than
                    // duplicating the name onto two rows.
                    Some(_) => WorktreeIdentity {
                        drifted: true,
                        ..WorktreeIdentity::unnamed(None)
                    },
                    None => WorktreeIdentity::unnamed(None),
                });
            };

            match state.branch {
                // Recovered — unless an attached worktree already holds that
                // name, in which case the record is the stale one and the
                // real checkout wins.
                Some(branch) if !claimed.contains(branch.as_str()) => Some(WorktreeIdentity {
                    name: branch.clone(),
                    branch: Some(branch),
                    source: IdentitySource::Recovered,
                    op: Some(state.kind),
                    is_sandbox: false,
                    drifted: false,
                }),
                _ => Some(WorktreeIdentity::unnamed(Some(state.kind))),
            }
        })
        .collect()
}

/// The branch daft recorded for the worktree at `path`, if any.
fn recorded_branch(
    records: &HashMap<String, WorktreeIdentityRow>,
    path: &std::path::Path,
) -> Option<String> {
    if records.is_empty() {
        return None;
    }
    let id = super::identity_store::worktree_id_for(path)?;
    records.get(&id).map(|row| row.branch.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// A worktree directory whose `.git` is a real directory, so the probe
    /// reads state files placed under it.
    fn worktree(root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        std::fs::create_dir_all(path.join(".git")).unwrap();
        path
    }

    fn rebasing(path: &Path, head_name: Option<&str>) {
        let dir = path.join(".git/rebase-merge");
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(name) = head_name {
            std::fs::write(dir.join("head-name"), format!("{name}\n")).unwrap();
        }
    }

    fn entry(path: &Path, branch: Option<&str>) -> WorktreeListEntry {
        WorktreeListEntry {
            path: path.to_path_buf(),
            branch: branch.map(str::to_string),
            is_bare: false,
            is_detached: branch.is_none(),
        }
    }

    fn resolve_one(entry: WorktreeListEntry) -> WorktreeIdentity {
        resolve_identities(&[entry]).remove(0).unwrap()
    }

    #[test]
    fn attached_worktrees_keep_their_name() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = worktree(tmp.path(), "main");
        let id = resolve_one(entry(&wt, Some("main")));

        assert_eq!(id.name, "main");
        assert_eq!(id.branch.as_deref(), Some("main"));
        assert_eq!(id.source, IdentitySource::Attached);
        assert!(!id.is_sandbox);
        assert_eq!(id.op, None);
    }

    /// The bug this module exists for: mid-rebase the porcelain says nothing,
    /// and the row used to become an anonymous sandbox.
    #[test]
    fn a_rebasing_worktree_keeps_its_identity_and_is_not_a_sandbox() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = worktree(tmp.path(), "feat");
        rebasing(&wt, Some("refs/heads/feat/x"));

        let id = resolve_one(entry(&wt, None));
        assert_eq!(id.name, "feat/x");
        assert_eq!(id.branch.as_deref(), Some("feat/x"));
        assert_eq!(id.source, IdentitySource::Recovered);
        assert_eq!(id.op, Some(OpKind::Rebase));
        assert!(
            !id.is_sandbox,
            "an operation explains the detachment — this is not a scratch checkout"
        );
    }

    /// An operation with no recorded branch still explains the detachment.
    #[test]
    fn an_operation_without_a_branch_is_still_not_a_sandbox() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = worktree(tmp.path(), "amming");
        let dir = wt.join(".git/rebase-apply");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("applying"), "").unwrap();

        let id = resolve_one(entry(&wt, None));
        assert_eq!(id.name, DETACHED_LABEL);
        assert_eq!(id.branch, None);
        assert_eq!(id.op, Some(OpKind::Am));
        assert!(!id.is_sandbox);
    }

    #[test]
    fn a_plain_detached_checkout_is_still_a_sandbox() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = worktree(tmp.path(), "scratch");

        let id = resolve_one(entry(&wt, None));
        assert_eq!(id.name, DETACHED_LABEL);
        assert_eq!(id.source, IdentitySource::None);
        assert_eq!(id.op, None);
        assert!(id.is_sandbox, "nothing explains this detachment");
    }

    /// A merge keeps HEAD attached, so identity comes from the porcelain and
    /// the operation rides along.
    #[test]
    fn an_attached_worktree_can_still_be_mid_operation() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = worktree(tmp.path(), "main");
        std::fs::write(wt.join(".git/MERGE_HEAD"), "deadbeef\n").unwrap();

        let id = resolve_one(entry(&wt, Some("main")));
        assert_eq!(id.name, "main");
        assert_eq!(id.source, IdentitySource::Attached);
        assert_eq!(id.op, Some(OpKind::Merge));
        assert!(!id.is_sandbox);
    }

    /// Attached checkouts claim their names before recovery runs, so a name
    /// can never be claimed twice. Everything downstream that keys rows by
    /// name depends on this.
    #[test]
    fn an_attached_checkout_outranks_a_recovered_claim_on_the_same_name() {
        let tmp = tempfile::tempdir().unwrap();
        let attached = worktree(tmp.path(), "real");
        let stale = worktree(tmp.path(), "stale");
        // The detached worktree's operation claims a branch that is genuinely
        // checked out elsewhere.
        rebasing(&stale, Some("refs/heads/feat/x"));

        let ids = resolve_identities(&[entry(&stale, None), entry(&attached, Some("feat/x"))]);

        let recovered = ids[0].as_ref().unwrap();
        assert_eq!(
            recovered.name, DETACHED_LABEL,
            "the real checkout owns the name; this row must not duplicate it"
        );
        assert_eq!(
            recovered.op,
            Some(OpKind::Rebase),
            "the operation still shows"
        );
        assert!(!recovered.is_sandbox);

        assert_eq!(ids[1].as_ref().unwrap().name, "feat/x");
        assert_eq!(ids[1].as_ref().unwrap().source, IdentitySource::Attached);
    }

    #[test]
    fn bare_entries_resolve_to_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let bare = WorktreeListEntry {
            path: tmp.path().join(".git"),
            branch: None,
            is_bare: true,
            is_detached: false,
        };
        assert_eq!(resolve_identities(&[bare]), vec![None]);
    }

    #[test]
    fn results_line_up_positionally_with_the_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let main = worktree(tmp.path(), "main");
        let bare = WorktreeListEntry {
            path: tmp.path().join(".git"),
            branch: None,
            is_bare: true,
            is_detached: false,
        };
        let ids = resolve_identities(&[bare, entry(&main, Some("main"))]);
        assert_eq!(ids.len(), 2);
        assert!(ids[0].is_none());
        assert_eq!(ids[1].as_ref().unwrap().name, "main");
    }
    // ── persisted records (fallback + drift) ────────────────────────────────

    fn record(id: &str, branch: &str) -> (String, WorktreeIdentityRow) {
        (
            id.to_string(),
            WorktreeIdentityRow {
                repo_hash: "repo".into(),
                worktree_id: id.into(),
                branch: branch.into(),
                worktree_path: String::from("/tmp/") + id,
                updated_at: chrono::Utc::now(),
            },
        )
    }

    /// A linked worktree, so it has a private-gitdir id records can key on.
    fn linked(root: &Path, id: &str) -> PathBuf {
        let private = root.join("repo/.git/worktrees").join(id);
        std::fs::create_dir_all(&private).unwrap();
        let worktree = root.join(id);
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".git"),
            String::from("gitdir: ") + private.to_str().unwrap() + "\n",
        )
        .unwrap();
        worktree
    }

    /// The case the record exists for: detached with nothing to explain it.
    #[test]
    fn an_unexplained_detachment_falls_back_to_the_record() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = linked(tmp.path(), "wt-a");
        let records = HashMap::from([record("wt-a", "feat/x")]);

        let id = resolve_identities_with(&[entry(&wt, None)], &records)
            .remove(0)
            .unwrap();
        assert_eq!(id.name, "feat/x");
        assert_eq!(id.branch.as_deref(), Some("feat/x"));
        assert_eq!(id.source, IdentitySource::Persisted);
        assert!(
            id.is_sandbox,
            "the record names it, but nothing explains the detachment"
        );
        assert!(!id.drifted);
    }

    /// Live state always wins: an operation names the branch, so the record
    /// is never consulted — even when it disagrees.
    #[test]
    fn an_operation_outranks_the_record() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = linked(tmp.path(), "wt-a");
        // A linked worktree's state files live in its private gitdir, not
        // under `<worktree>/.git` — which is a file here.
        let head_name = tmp
            .path()
            .join("repo/.git/worktrees/wt-a/rebase-merge/head-name");
        std::fs::create_dir_all(head_name.parent().unwrap()).unwrap();
        std::fs::write(&head_name, "refs/heads/feat/live\n").unwrap();
        let records = HashMap::from([record("wt-a", "feat/stale")]);

        let id = resolve_identities_with(&[entry(&wt, None)], &records)
            .remove(0)
            .unwrap();
        assert_eq!(id.name, "feat/live");
        assert_eq!(id.source, IdentitySource::Recovered);
    }

    #[test]
    fn an_attached_branch_outranks_the_record_and_flags_drift() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = linked(tmp.path(), "wt-a");
        let records = HashMap::from([record("wt-a", "feat/recorded")]);

        let id = resolve_identities_with(&[entry(&wt, Some("hotfix/urgent"))], &records)
            .remove(0)
            .unwrap();
        assert_eq!(id.name, "hotfix/urgent", "the live checkout wins the name");
        assert_eq!(id.source, IdentitySource::Attached);
        assert!(id.drifted, "the record disagrees and should say so");
    }

    #[test]
    fn a_record_that_agrees_is_not_drift() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = linked(tmp.path(), "wt-a");
        let records = HashMap::from([record("wt-a", "feat/x")]);

        let id = resolve_identities_with(&[entry(&wt, Some("feat/x"))], &records)
            .remove(0)
            .unwrap();
        assert!(!id.drifted);
    }

    /// The collision the guard exists for, and the only way to actually
    /// construct it: a stale record naming a branch that is genuinely checked
    /// out somewhere else. Two rows must never claim one name — everything
    /// downstream keys rows by name.
    #[test]
    fn a_stale_record_cannot_duplicate_a_live_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let detached = linked(tmp.path(), "wt-a");
        let attached = linked(tmp.path(), "wt-b");
        let records = HashMap::from([record("wt-a", "feat/x")]);

        let ids = resolve_identities_with(
            &[entry(&detached, None), entry(&attached, Some("feat/x"))],
            &records,
        );

        let stale = ids[0].as_ref().unwrap();
        assert_eq!(
            stale.name, DETACHED_LABEL,
            "the real checkout owns the name"
        );
        assert_eq!(stale.source, IdentitySource::None);
        assert!(stale.drifted, "the disagreement is still worth surfacing");

        assert_eq!(ids[1].as_ref().unwrap().name, "feat/x");
        assert!(!ids[1].as_ref().unwrap().drifted);
    }

    #[test]
    fn no_record_leaves_a_detached_worktree_exactly_as_before() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = linked(tmp.path(), "wt-a");

        let id = resolve_identities_with(&[entry(&wt, None)], &HashMap::new())
            .remove(0)
            .unwrap();
        assert_eq!(id.name, DETACHED_LABEL);
        assert_eq!(id.source, IdentitySource::None);
        assert!(id.is_sandbox);
        assert!(!id.drifted);
    }
}
