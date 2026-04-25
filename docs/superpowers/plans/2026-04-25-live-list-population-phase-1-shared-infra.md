# Live List Population — Phase 1: Shared Infrastructure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the shared infrastructure for live cell-by-cell population —
streaming collector, typed patch events, `LiveTable` widget extracted from the
existing TUI state — with **no user-visible behavior change**. Existing
prune/sync integration tests must pass unchanged at the end of this phase.

**Architecture:** Introduce a `FieldSet` bitmask that bridges three layers
(collector subsets, patch deltas, sort dependencies). Add a phase-agnostic
streaming collector that takes a `FieldSet` + branch list and emits
`WorktreeInfoUpdated { patch, source }` events through the existing `DagEvent`
channel. Extract the worktree-rows core of today's `TuiState` into a new
`LiveTable` widget that owns row rendering, sort, partition, patch application,
and loading glyphs. Rename today's `TuiState` → `OperationTableState`; have it
embed a `LiveTable` and continue to handle phase + hook events itself. Replace
`TaskCompleted::updated_info` with orchestrator-emitted
`WorktreeInfoUpdated { source: PostTask }` patches.

**Tech Stack:** Rust, ratatui 0.30, crossterm 0.29, std::sync::mpsc,
std::thread::scope. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-04-25-live-list-population-design.md`

**Phase 1 of 3.** Phases 2 (`daft list` live UX) and 3 (migrate prune/sync to
streaming seed) ship under separate plans, written when those phases begin. This
phase is verifiable by existing tests passing; it adds infrastructure but
changes no command's behavior.

---

## File Structure

**New files:**

- `src/core/worktree/info_field.rs` — `FieldSet` u32 newtype with const members
  and standard set operations. Module re-exported from
  `src/core/worktree/mod.rs`.
- `src/core/worktree/list_stream.rs` — Streaming collector. Public API:
  `CollectorRequest`, `CollectorTarget`, `CollectorContext`, `CollectorHandle`,
  `spawn(req, tx) -> CollectorHandle`.
- `src/output/tui/live_table.rs` — `LiveTable` widget extracted from `TuiState`.
  Owns worktree rows, sort, partition, patch application, loading-glyph state.
  Public API: `LiveTable::new`, `apply_event`, `tick`, `render`, `into_rows`,
  `LiveTableConfig`.

**Modified files:**

- `src/core/worktree/mod.rs` — re-export `info_field` and `list_stream` modules.
- `src/core/worktree/list.rs` — add
  `WorktreeInfo::apply_patch(&mut self, &WorktreeInfoPatch) -> FieldSet`.
- `src/core/sort.rs` — add `SortSpec::required_fields(&self) -> FieldSet`.
- `src/core/worktree/sync_dag.rs` — add `WorktreeInfoPatch` enum, `PatchSource`
  enum, `DagEvent::WorktreeInfoUpdated`, `DagEvent::WorktreeInfoCollectionDone`.
  Remove `TaskCompleted::updated_info` field.
- `src/output/tui/state.rs` — rename `TuiState` → `OperationTableState`; move
  worktree-rows logic into `LiveTable`; forward
  `WorktreeInfoUpdated`/`WorktreeInfoCollectionDone` to the embedded
  `LiveTable`; existing phase/hook event arms unchanged. Update all field
  accesses across the project.
- `src/output/tui/operation_table.rs` — update field/type names from the rename;
  add `pin_default_branch` and `partition_by_owner` to `TableConfig`, default to
  current behavior (true / true).
- `src/output/tui/driver.rs` — update `TuiRenderer` references to use
  `OperationTableState` instead of `TuiState`.
- `src/output/tui/render.rs`, `src/output/tui/presenter.rs` — update field
  accesses for the rename + LiveTable extraction.
- `src/output/tui/mod.rs` — re-export `LiveTable`, `LiveTableConfig`; re-export
  `OperationTableState` (formerly `TuiState`).
- `src/commands/sync.rs` — orchestrator: replace each
  `TaskCompleted::updated_info: Some(Box<WorktreeInfo>)` emission with a matched
  set of `DagEvent::WorktreeInfoUpdated { source: PostTask(phase) }` events for
  the fields each task touches (table in spec).
- `src/commands/prune.rs` — same: prune tasks remove the row, no patch needed;
  the field on `TaskCompleted` is removed entirely.
- `src/core/worktree/sync_dag.rs` — DagExecutor's task closure no longer returns
  `Option<Box<WorktreeInfo>>`; signature updated; tests adjusted.

**Tests:**

- `src/core/worktree/info_field.rs` — unit: bitwise ops, `intersects`,
  `contains`, predefined subsets.
- `src/core/sort.rs` — unit: `required_fields` per `SortColumn` variant.
- `src/core/worktree/list.rs` — unit: `apply_patch` truth table per
  `WorktreeInfoPatch` variant.
- `src/core/worktree/list_stream.rs` — unit + fixture-repo integration: spawn
  collector against the existing test fixtures, assert the multiset of patches
  emitted for a given `FieldSet`.
- `src/output/tui/live_table.rs` — unit: `apply_event` for `WorktreeInfoUpdated`
  (sort re-eval, partition shuffle, stale-source suppression),
  `WorktreeInfoCollectionDone` (loading glyphs cleared).
- `src/output/tui/state.rs` — existing `TuiState` tests adapted to the rename +
  LiveTable boundary.
- `src/core/worktree/sync_dag.rs` — existing channel-based tests adapted to the
  new task closure signature.

---

## Task 1: `FieldSet` newtype + tests

**Files:**

- Create: `src/core/worktree/info_field.rs`
- Modify: `src/core/worktree/mod.rs`

A u32-backed bitmask. No `bitflags` dep — just const members + manual ops. This
is the shared vocabulary for collector subsets, patch deltas, and sort
dependencies.

- [ ] **Step 1: Write the failing test**

Create `src/core/worktree/info_field.rs`:

```rust
//! Bitmask identifying fields of `WorktreeInfo` for the live-population
//! pipeline. Used in three places: (1) the streaming collector takes one as
//! input to scope its work, (2) `WorktreeInfo::apply_patch` returns one to
//! signal which fields changed, (3) `SortSpec::required_fields` returns one
//! to declare its sort dependencies.

use std::ops::{BitAnd, BitOr, BitOrAssign, Not};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldSet(u32);

impl FieldSet {
    pub const EMPTY: Self = Self(0);

    pub const BASE_AHEAD_BEHIND:   Self = Self(1 << 0);
    pub const REMOTE_AHEAD_BEHIND: Self = Self(1 << 1);
    pub const CHANGES:             Self = Self(1 << 2);
    pub const LAST_COMMIT:         Self = Self(1 << 3);
    pub const BRANCH_AGE:          Self = Self(1 << 4);
    pub const OWNER:               Self = Self(1 << 5);
    pub const BASE_LINES:          Self = Self(1 << 6);
    pub const CHANGES_LINES:       Self = Self(1 << 7);
    pub const REMOTE_LINES:        Self = Self(1 << 8);
    pub const SIZE:                Self = Self(1 << 9);
    pub const MTIME:               Self = Self(1 << 10);

    /// Fields whose values can change after a `git fetch`.
    pub const REMOTE_DERIVED: Self = Self(
        Self::REMOTE_AHEAD_BEHIND.0 | Self::REMOTE_LINES.0,
    );

    /// Fields whose values can change after any per-branch task
    /// (Update / Rebase / Push). Used by the orchestrator for post-task
    /// re-runs.
    pub const VOLATILE: Self = Self(
        Self::BASE_AHEAD_BEHIND.0
            | Self::REMOTE_AHEAD_BEHIND.0
            | Self::CHANGES.0
            | Self::LAST_COMMIT.0
            | Self::BASE_LINES.0
            | Self::CHANGES_LINES.0
            | Self::REMOTE_LINES.0,
    );

    /// All fields. Used by the initial Collector run.
    pub const ALL: Self = Self(u32::MAX);

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub const fn intersects(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for FieldSet {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self { Self(self.0 | rhs.0) }
}

impl BitOrAssign for FieldSet {
    fn bitor_assign(&mut self, rhs: Self) { self.0 |= rhs.0; }
}

impl BitAnd for FieldSet {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self { Self(self.0 & rhs.0) }
}

impl Not for FieldSet {
    type Output = Self;
    fn not(self) -> Self { Self(!self.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_contains_nothing_and_intersects_nothing() {
        assert!(FieldSet::EMPTY.is_empty());
        assert!(!FieldSet::EMPTY.contains(FieldSet::SIZE));
        assert!(!FieldSet::EMPTY.intersects(FieldSet::SIZE));
    }

    #[test]
    fn or_combines_members() {
        let s = FieldSet::SIZE | FieldSet::OWNER;
        assert!(s.contains(FieldSet::SIZE));
        assert!(s.contains(FieldSet::OWNER));
        assert!(!s.contains(FieldSet::CHANGES));
    }

    #[test]
    fn intersects_returns_true_for_any_overlap() {
        let a = FieldSet::SIZE | FieldSet::OWNER;
        let b = FieldSet::OWNER | FieldSet::CHANGES;
        assert!(a.intersects(b));
    }

    #[test]
    fn intersects_returns_false_for_disjoint_sets() {
        let a = FieldSet::SIZE;
        let b = FieldSet::OWNER;
        assert!(!a.intersects(b));
    }

    #[test]
    fn remote_derived_subset_contains_only_remote_fields() {
        assert!(FieldSet::REMOTE_DERIVED.contains(FieldSet::REMOTE_AHEAD_BEHIND));
        assert!(FieldSet::REMOTE_DERIVED.contains(FieldSet::REMOTE_LINES));
        assert!(!FieldSet::REMOTE_DERIVED.contains(FieldSet::SIZE));
        assert!(!FieldSet::REMOTE_DERIVED.contains(FieldSet::CHANGES));
    }

    #[test]
    fn volatile_subset_excludes_size_and_owner_and_age() {
        assert!(!FieldSet::VOLATILE.contains(FieldSet::SIZE));
        assert!(!FieldSet::VOLATILE.contains(FieldSet::OWNER));
        assert!(!FieldSet::VOLATILE.contains(FieldSet::BRANCH_AGE));
    }

    #[test]
    fn all_contains_every_known_member() {
        for member in [
            FieldSet::BASE_AHEAD_BEHIND, FieldSet::REMOTE_AHEAD_BEHIND,
            FieldSet::CHANGES, FieldSet::LAST_COMMIT, FieldSet::BRANCH_AGE,
            FieldSet::OWNER, FieldSet::BASE_LINES, FieldSet::CHANGES_LINES,
            FieldSet::REMOTE_LINES, FieldSet::SIZE, FieldSet::MTIME,
        ] {
            assert!(FieldSet::ALL.contains(member));
        }
    }
}
```

Add to `src/core/worktree/mod.rs`:

```rust
pub mod info_field;
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test --lib core::worktree::info_field` Expected: all 7 tests pass.

- [ ] **Step 3: Run clippy + fmt**

Run: `mise run fmt && mise run clippy` Expected: zero warnings, zero diff.

- [ ] **Step 4: Commit**

```bash
git add src/core/worktree/info_field.rs src/core/worktree/mod.rs
git commit -m "feat(core): add FieldSet bitmask for live-population pipeline (#402)"
```

---

## Task 2: `SortSpec::required_fields`

**Files:**

- Modify: `src/core/sort.rs`

Maps each sort column to the `FieldSet` needed to compute it. Used by
`LiveTable` to decide whether an arriving patch should trigger a re-sort.

- [ ] **Step 1: Write the failing test**

Append to `src/core/sort.rs`:

```rust
#[cfg(test)]
mod required_fields_tests {
    use super::*;
    use crate::core::worktree::info_field::FieldSet;

    fn fields_for(input: &str) -> FieldSet {
        SortSpec::parse(input).unwrap().required_fields()
    }

    #[test]
    fn branch_path_hash_require_no_dynamic_fields() {
        // These sort by data already present from porcelain.
        assert_eq!(fields_for("branch"), FieldSet::EMPTY);
        assert_eq!(fields_for("path"),   FieldSet::EMPTY);
        assert_eq!(fields_for("hash"),   FieldSet::LAST_COMMIT);
    }

    #[test]
    fn size_requires_size() {
        assert_eq!(fields_for("size"), FieldSet::SIZE);
    }

    #[test]
    fn age_requires_branch_age() {
        assert_eq!(fields_for("age"), FieldSet::BRANCH_AGE);
    }

    #[test]
    fn owner_requires_owner() {
        assert_eq!(fields_for("owner"), FieldSet::OWNER);
    }

    #[test]
    fn last_commit_requires_last_commit() {
        assert_eq!(fields_for("commit"), FieldSet::LAST_COMMIT);
    }

    #[test]
    fn activity_requires_last_commit_and_mtime() {
        assert_eq!(
            fields_for("activity"),
            FieldSet::LAST_COMMIT | FieldSet::MTIME,
        );
    }

    #[test]
    fn base_changes_remote_each_require_their_cluster() {
        assert_eq!(fields_for("base"),    FieldSet::BASE_AHEAD_BEHIND);
        assert_eq!(fields_for("changes"), FieldSet::CHANGES);
        assert_eq!(fields_for("remote"),  FieldSet::REMOTE_AHEAD_BEHIND);
    }

    #[test]
    fn multi_key_unions_required_fields() {
        // +owner,-size: requires both OWNER and SIZE.
        let f = fields_for("+owner,-size");
        assert!(f.contains(FieldSet::OWNER));
        assert!(f.contains(FieldSet::SIZE));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib core::sort::required_fields_tests` Expected: compile
error — `required_fields` does not exist.

- [ ] **Step 3: Implement `required_fields`**

In `src/core/sort.rs`, inside `impl SortSpec` (after `compare`):

```rust
/// Returns the set of `WorktreeInfo` fields needed to evaluate this sort
/// spec. Used by `LiveTable` to skip re-sorts when a patch lands on a
/// cell unrelated to the current sort order.
pub fn required_fields(&self) -> crate::core::worktree::info_field::FieldSet {
    use crate::core::worktree::info_field::FieldSet;
    let mut acc = FieldSet::EMPTY;
    for key in &self.keys {
        acc |= match key.column {
            SortColumn::Branch     => FieldSet::EMPTY,
            SortColumn::Path       => FieldSet::EMPTY,
            SortColumn::Size       => FieldSet::SIZE,
            SortColumn::Age        => FieldSet::BRANCH_AGE,
            SortColumn::Owner      => FieldSet::OWNER,
            SortColumn::Hash       => FieldSet::LAST_COMMIT,
            SortColumn::Activity   => FieldSet::LAST_COMMIT | FieldSet::MTIME,
            SortColumn::LastCommit => FieldSet::LAST_COMMIT,
            SortColumn::Base       => FieldSet::BASE_AHEAD_BEHIND,
            SortColumn::Changes    => FieldSet::CHANGES,
            SortColumn::Remote     => FieldSet::REMOTE_AHEAD_BEHIND,
        };
    }
    acc
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib core::sort::required_fields_tests` Expected: all 8 tests
pass.

- [ ] **Step 5: Commit**

```bash
git add src/core/sort.rs
git commit -m "feat(core): add SortSpec::required_fields for live re-sort gating (#402)"
```

---

## Task 3: `WorktreeInfoPatch` and `PatchSource` enums

**Files:**

- Modify: `src/core/worktree/sync_dag.rs`

Typed cell-cluster deltas plus the source tag used for staleness suppression in
`LiveTable`. No producer or consumer wiring yet — this task introduces the types
only, so downstream tasks have a stable target.

- [ ] **Step 1: Add the enums**

In `src/core/worktree/sync_dag.rs`, near the existing `DagEvent` enum
declaration, add:

```rust
use crate::core::worktree::list::BranchOwner;

/// A typed delta over `WorktreeInfo`. Each variant maps 1:1 to one
/// underlying git/FS call cluster in the streaming collector.
#[derive(Debug, Clone)]
pub enum WorktreeInfoPatch {
    BaseAheadBehind(Option<(usize, usize)>),
    RemoteAheadBehind(Option<(usize, usize)>),
    Changes { staged: usize, unstaged: usize, untracked: usize },
    LastCommit {
        timestamp: Option<i64>,
        hash: Option<String>,
        subject: String,
    },
    BranchAge(Option<i64>),
    Owner(Option<BranchOwner>),
    BaseLines(Option<(usize, usize)>),
    ChangesLines {
        staged: (usize, usize),
        unstaged: (usize, usize),
    },
    RemoteLines(Option<(usize, usize)>),
    Size(Option<u64>),
    Mtime(Option<i64>),
}

/// Why a patch was emitted. `LiveTable` uses this to suppress stale
/// patches: a `Collector` patch arriving after a `PostFetch` patch covering
/// the same field on the same branch is dropped. Priority order:
/// `PostTask > PostFetch > Collector`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchSource {
    Collector,
    PostFetch,
    PostTask(OperationPhase),
}

impl PatchSource {
    /// Higher = more authoritative. Used for staleness suppression.
    pub fn priority(self) -> u8 {
        match self {
            Self::Collector  => 0,
            Self::PostFetch  => 1,
            Self::PostTask(_) => 2,
        }
    }
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check --lib` Expected: compiles cleanly (the types are unused but
valid).

- [ ] **Step 3: Add a smoke test**

In `src/core/worktree/sync_dag.rs` `#[cfg(test)] mod tests` (existing):

```rust
#[test]
fn patch_source_priority_ordering() {
    assert!(PatchSource::PostTask(OperationPhase::Push).priority()
            > PatchSource::PostFetch.priority());
    assert!(PatchSource::PostFetch.priority()
            > PatchSource::Collector.priority());
}
```

- [ ] **Step 4: Run tests**

Run:
`cargo test --lib core::worktree::sync_dag::tests::patch_source_priority_ordering`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add src/core/worktree/sync_dag.rs
git commit -m "feat(core): add WorktreeInfoPatch and PatchSource types (#402)"
```

---

## Task 4: `WorktreeInfo::apply_patch`

**Files:**

- Modify: `src/core/worktree/list.rs`

Maps a typed patch into in-place mutations on `WorktreeInfo`, returning the
`FieldSet` of fields whose value actually changed.

- [ ] **Step 1: Write the failing test**

Append to the existing `#[cfg(test)] mod tests` in `src/core/worktree/list.rs`
(the test module `build_emit_table_*` lives in `src/commands/list.rs`; the right
module here is the one inside `src/core/worktree/list.rs` — create one if
missing):

```rust
#[cfg(test)]
mod apply_patch_tests {
    use super::*;
    use crate::core::worktree::info_field::FieldSet;
    use crate::core::worktree::sync_dag::WorktreeInfoPatch;

    fn empty_info() -> WorktreeInfo {
        WorktreeInfo::empty("test")
    }

    #[test]
    fn base_ahead_behind_some_fills_both_and_returns_the_field() {
        let mut info = empty_info();
        let touched = info.apply_patch(&WorktreeInfoPatch::BaseAheadBehind(Some((3, 1))));
        assert_eq!(info.ahead, Some(3));
        assert_eq!(info.behind, Some(1));
        assert_eq!(touched, FieldSet::BASE_AHEAD_BEHIND);
    }

    #[test]
    fn base_ahead_behind_none_clears_both() {
        let mut info = empty_info();
        info.ahead = Some(5);
        info.behind = Some(2);
        let touched = info.apply_patch(&WorktreeInfoPatch::BaseAheadBehind(None));
        assert_eq!(info.ahead, None);
        assert_eq!(info.behind, None);
        assert_eq!(touched, FieldSet::BASE_AHEAD_BEHIND);
    }

    #[test]
    fn changes_fills_three_fields() {
        let mut info = empty_info();
        let touched = info.apply_patch(&WorktreeInfoPatch::Changes {
            staged: 2, unstaged: 1, untracked: 4,
        });
        assert_eq!((info.staged, info.unstaged, info.untracked), (2, 1, 4));
        assert_eq!(touched, FieldSet::CHANGES);
    }

    #[test]
    fn last_commit_fills_three_fields() {
        let mut info = empty_info();
        let touched = info.apply_patch(&WorktreeInfoPatch::LastCommit {
            timestamp: Some(1700000000),
            hash: Some("abc1234".into()),
            subject: "fix bug".into(),
        });
        assert_eq!(info.last_commit_timestamp, Some(1700000000));
        assert_eq!(info.last_commit_hash, Some("abc1234".into()));
        assert_eq!(info.last_commit_subject, "fix bug");
        assert_eq!(touched, FieldSet::LAST_COMMIT);
    }

    #[test]
    fn size_fills_size_bytes() {
        let mut info = empty_info();
        let touched = info.apply_patch(&WorktreeInfoPatch::Size(Some(2048)));
        assert_eq!(info.size_bytes, Some(2048));
        assert_eq!(touched, FieldSet::SIZE);
    }

    #[test]
    fn changes_lines_fills_four_fields() {
        let mut info = empty_info();
        let touched = info.apply_patch(&WorktreeInfoPatch::ChangesLines {
            staged: (10, 2),
            unstaged: (5, 1),
        });
        assert_eq!(info.staged_lines_inserted, Some(10));
        assert_eq!(info.staged_lines_deleted, Some(2));
        assert_eq!(info.unstaged_lines_inserted, Some(5));
        assert_eq!(info.unstaged_lines_deleted, Some(1));
        assert_eq!(touched, FieldSet::CHANGES_LINES);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib core::worktree::list::apply_patch_tests` Expected:
compile error — `apply_patch` does not exist.

- [ ] **Step 3: Implement `apply_patch`**

Add to `impl WorktreeInfo` in `src/core/worktree/list.rs` (near the existing
helper methods around line 181):

```rust
/// Apply a typed patch in place. Returns the `FieldSet` of fields whose
/// value changed (the caller — typically `LiveTable` — uses this to
/// decide whether to re-sort).
pub fn apply_patch(
    &mut self,
    patch: &crate::core::worktree::sync_dag::WorktreeInfoPatch,
) -> crate::core::worktree::info_field::FieldSet {
    use crate::core::worktree::info_field::FieldSet;
    use crate::core::worktree::sync_dag::WorktreeInfoPatch as P;

    match patch {
        P::BaseAheadBehind(v) => {
            (self.ahead, self.behind) = match v {
                Some((a, b)) => (Some(*a), Some(*b)),
                None => (None, None),
            };
            FieldSet::BASE_AHEAD_BEHIND
        }
        P::RemoteAheadBehind(v) => {
            (self.remote_ahead, self.remote_behind) = match v {
                Some((a, b)) => (Some(*a), Some(*b)),
                None => (None, None),
            };
            FieldSet::REMOTE_AHEAD_BEHIND
        }
        P::Changes { staged, unstaged, untracked } => {
            self.staged = *staged;
            self.unstaged = *unstaged;
            self.untracked = *untracked;
            FieldSet::CHANGES
        }
        P::LastCommit { timestamp, hash, subject } => {
            self.last_commit_timestamp = *timestamp;
            self.last_commit_hash = hash.clone();
            self.last_commit_subject = subject.clone();
            FieldSet::LAST_COMMIT
        }
        P::BranchAge(v) => {
            self.branch_creation_timestamp = *v;
            FieldSet::BRANCH_AGE
        }
        P::Owner(v) => {
            self.owner = v.clone();
            FieldSet::OWNER
        }
        P::BaseLines(v) => {
            (self.base_lines_inserted, self.base_lines_deleted) = match v {
                Some((i, d)) => (Some(*i), Some(*d)),
                None => (None, None),
            };
            FieldSet::BASE_LINES
        }
        P::ChangesLines { staged, unstaged } => {
            self.staged_lines_inserted = Some(staged.0);
            self.staged_lines_deleted = Some(staged.1);
            self.unstaged_lines_inserted = Some(unstaged.0);
            self.unstaged_lines_deleted = Some(unstaged.1);
            FieldSet::CHANGES_LINES
        }
        P::RemoteLines(v) => {
            (self.remote_lines_inserted, self.remote_lines_deleted) = match v {
                Some((i, d)) => (Some(*i), Some(*d)),
                None => (None, None),
            };
            FieldSet::REMOTE_LINES
        }
        P::Size(v) => {
            self.size_bytes = *v;
            FieldSet::SIZE
        }
        P::Mtime(v) => {
            self.working_tree_mtime = *v;
            FieldSet::MTIME
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib core::worktree::list::apply_patch_tests` Expected: all 6
tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/core/worktree/list.rs
git commit -m "feat(core): add WorktreeInfo::apply_patch (#402)"
```

---

## Task 5: New `DagEvent` variants

**Files:**

- Modify: `src/core/worktree/sync_dag.rs`

Add the two new variants. Existing `apply_event` callsites in
`src/output/tui/state.rs` will need ignore-arms added so the project still
compiles; the actual handling lands in Task 9.

- [ ] **Step 1: Extend `DagEvent`**

In `src/core/worktree/sync_dag.rs`, inside the `pub enum DagEvent` declaration,
append:

```rust
    /// A patch landed for `branch_name` from `source`. Carries one cluster
    /// of cells produced by a single underlying git/FS call.
    WorktreeInfoUpdated {
        branch_name: String,
        patch: WorktreeInfoPatch,
        source: PatchSource,
    },

    /// The initial `source=Collector` run completed. Subset re-runs
    /// (`PostFetch`, `PostTask`) do not emit this — they end silently.
    WorktreeInfoCollectionDone,
```

- [ ] **Step 2: Add no-op ignore arms in `TuiState::apply_event`**

In `src/output/tui/state.rs`, inside the `match event { ... }` in `apply_event`,
add at the end (before the final `}` of the match):

```rust
            DagEvent::WorktreeInfoUpdated { .. } => {
                // Forwarded to LiveTable in Task 9 — ignore for now.
            }
            DagEvent::WorktreeInfoCollectionDone => {
                // Handled in Task 9.
            }
```

- [ ] **Step 3: Verify compilation**

Run: `cargo build` Expected: compiles cleanly.

- [ ] **Step 4: Run full unit test suite**

Run: `mise run test:unit` Expected: all existing tests pass (no behavior
change).

- [ ] **Step 5: Commit**

```bash
git add src/core/worktree/sync_dag.rs src/output/tui/state.rs
git commit -m "feat(core): add WorktreeInfoUpdated and WorktreeInfoCollectionDone events (#402)"
```

---

## Task 6: Streaming collector — types and skeleton

**Files:**

- Create: `src/core/worktree/list_stream.rs`
- Modify: `src/core/worktree/mod.rs`

Public API surface only: `CollectorRequest`, `CollectorTarget`,
`CollectorContext`, `CollectorHandle`, `spawn`. Worker logic in Task 7.

- [ ] **Step 1: Create `list_stream.rs` with the public types**

Create `src/core/worktree/list_stream.rs`:

```rust
//! Streaming collector for `WorktreeInfo` cells.
//!
//! Spawns one worker thread per branch, each running cluster calls in a
//! fixed cheap-first order and emitting `DagEvent::WorktreeInfoUpdated`
//! patches into a shared channel. Cancellation is cooperative between
//! cluster calls. Re-runnable: callers invoke `spawn` again with a
//! narrower `FieldSet` and a different `PatchSource` to drive post-fetch
//! and post-task refreshes.

use crate::{
    core::{
        ownership::OwnershipStrategy,
        worktree::{
            info_field::FieldSet,
            list::EntryKind,
            sync_dag::{DagEvent, PatchSource},
        },
    },
    git::GitCommand,
};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread::{self, JoinHandle},
};

use super::list::Stat;

#[derive(Debug, Clone)]
pub struct CollectorTarget {
    /// Branch name. `""` for detached (sandbox) entries.
    pub branch_name: String,
    pub path: Option<PathBuf>,
    pub kind: EntryKind,
    pub is_detached: bool,
}

pub struct CollectorContext {
    pub git: GitCommand,
    pub base_branch: String,
    pub remote_name: String,
    pub ownership_strategy: OwnershipStrategy,
    pub user_email: Option<String>,
}

pub struct CollectorRequest {
    pub targets: Vec<CollectorTarget>,
    pub fields: FieldSet,
    pub stat: Stat,
    pub source: PatchSource,
    pub ctx: Arc<CollectorContext>,
}

pub struct CollectorHandle {
    cancel: Arc<AtomicBool>,
    handles: Vec<JoinHandle<()>>,
    /// Collector-only sentinel sender (kept alive by handle so the
    /// completion event fires only after all workers have observably
    /// joined or cancelled).
    sentinel: Option<(mpsc::Sender<DagEvent>, PatchSource)>,
}

impl CollectorHandle {
    /// Request cooperative cancellation. Workers exit between cluster calls.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Wait for all workers to finish. Emits
    /// `DagEvent::WorktreeInfoCollectionDone` if and only if the spawning
    /// run was `source=Collector`.
    pub fn join(mut self) {
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
        if let Some((tx, source)) = self.sentinel.take() {
            if matches!(source, PatchSource::Collector) {
                let _ = tx.send(DagEvent::WorktreeInfoCollectionDone);
            }
        }
    }
}

/// Spawn workers for the request. Workers stream patches into `tx`.
/// The caller MUST call `CollectorHandle::join` (or drop the handle, which
/// silently joins) for the completion sentinel to fire.
pub fn spawn(req: CollectorRequest, tx: mpsc::Sender<DagEvent>) -> CollectorHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let CollectorRequest { targets, fields, stat, source, ctx } = req;

    let mut handles = Vec::with_capacity(targets.len());
    for target in targets {
        let tx = tx.clone();
        let ctx = Arc::clone(&ctx);
        let cancel = Arc::clone(&cancel);
        handles.push(thread::spawn(move || {
            run_worker(target, fields, stat, source, ctx, cancel, tx);
        }));
    }

    CollectorHandle {
        cancel,
        handles,
        sentinel: Some((tx, source)),
    }
}

fn run_worker(
    target: CollectorTarget,
    _fields: FieldSet,
    _stat: Stat,
    _source: PatchSource,
    _ctx: Arc<CollectorContext>,
    _cancel: Arc<AtomicBool>,
    _tx: mpsc::Sender<DagEvent>,
) {
    // Cluster calls land in Task 7. For now: no-op.
    let _ = target;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_request_emits_only_completion_sentinel() {
        let (tx, rx) = mpsc::channel();
        let ctx = Arc::new(CollectorContext {
            git: GitCommand::new(false),
            base_branch: "master".into(),
            remote_name: "origin".into(),
            ownership_strategy: OwnershipStrategy::default(),
            user_email: None,
        });
        let handle = spawn(
            CollectorRequest {
                targets: vec![],
                fields: FieldSet::ALL,
                stat: Stat::Summary,
                source: PatchSource::Collector,
                ctx,
            },
            tx,
        );
        handle.join();

        let events: Vec<DagEvent> = rx.iter().collect();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], DagEvent::WorktreeInfoCollectionDone));
    }

    #[test]
    fn post_fetch_run_does_not_emit_completion_sentinel() {
        let (tx, rx) = mpsc::channel();
        let ctx = Arc::new(CollectorContext {
            git: GitCommand::new(false),
            base_branch: "master".into(),
            remote_name: "origin".into(),
            ownership_strategy: OwnershipStrategy::default(),
            user_email: None,
        });
        let handle = spawn(
            CollectorRequest {
                targets: vec![],
                fields: FieldSet::REMOTE_DERIVED,
                stat: Stat::Summary,
                source: PatchSource::PostFetch,
                ctx,
            },
            tx,
        );
        handle.join();

        let events: Vec<DagEvent> = rx.iter().collect();
        assert_eq!(events.len(), 0);
    }
}
```

Add to `src/core/worktree/mod.rs`:

```rust
pub mod list_stream;
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib core::worktree::list_stream` Expected: 2 tests pass.

- [ ] **Step 3: Run clippy + fmt**

Run: `mise run fmt && mise run clippy` Expected: zero warnings.

- [ ] **Step 4: Commit**

```bash
git add src/core/worktree/list_stream.rs src/core/worktree/mod.rs
git commit -m "feat(core): add streaming collector skeleton (#402)"
```

---

## Task 7: Streaming collector — worker cluster calls

**Files:**

- Modify: `src/core/worktree/list_stream.rs`

Implement the per-worker loop. Cluster order:
`BASE_AHEAD_BEHIND → CHANGES → LAST_COMMIT → BRANCH_AGE → OWNER → REMOTE_AHEAD_BEHIND`
then `Stat::Lines` clusters then `SIZE → MTIME`. Each cluster call uses the
existing helper from `src/core/worktree/list.rs` (cross-module visibility is
needed — make the helpers `pub(crate)` if not already).

- [ ] **Step 1: Make `list.rs` helpers crate-visible**

In `src/core/worktree/list.rs`, change the visibility of the following free
functions from `fn` to `pub(crate) fn`:

- `get_ahead_behind` (line 301)
- `get_commit_metadata` (line 330)
- `get_branch_creation_timestamp` (line 445)
- `count_changed_files` (line 492)
- `get_upstream_ahead_behind` (line 547)
- `count_changed_lines` (line 590)
- `get_base_line_counts` (line 616)
- `get_remote_line_counts` (line 639)
- `compute_directory_size` (line 660)
- `max_mtime_of_files` (line 723)

Also change the `ChangedFiles` struct and any of its public fields used by
`count_changed_files` to `pub(crate)`.

Run: `cargo build` Expected: compiles cleanly.

- [ ] **Step 2: Implement the worker loop**

Replace the no-op `run_worker` body in `src/core/worktree/list_stream.rs` with:

```rust
fn run_worker(
    target: CollectorTarget,
    fields: FieldSet,
    stat: Stat,
    source: PatchSource,
    ctx: Arc<CollectorContext>,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<DagEvent>,
) {
    use crate::core::ownership;
    use crate::core::worktree::list::{
        count_changed_files, count_changed_lines, compute_directory_size,
        get_ahead_behind, get_base_line_counts, get_branch_creation_timestamp,
        get_commit_metadata, get_remote_line_counts, get_upstream_ahead_behind,
        max_mtime_of_files,
    };
    use crate::core::worktree::sync_dag::WorktreeInfoPatch as P;

    macro_rules! emit {
        ($patch:expr) => {{
            if cancel.load(Ordering::Relaxed) { return; }
            let _ = tx.send(DagEvent::WorktreeInfoUpdated {
                branch_name: target.branch_name.clone(),
                patch: $patch,
                source,
            });
        }};
    }

    let path = target.path.as_deref();

    // 1. BASE_AHEAD_BEHIND (skip detached)
    if fields.contains(FieldSet::BASE_AHEAD_BEHIND) && !target.is_detached {
        if let Some(p) = path {
            let v = get_ahead_behind(&ctx.base_branch, &target.branch_name, p);
            emit!(P::BaseAheadBehind(v));
        }
    }

    // 2. CHANGES
    if fields.contains(FieldSet::CHANGES) {
        if let Some(p) = path {
            let c = count_changed_files(p);
            emit!(P::Changes {
                staged: c.staged, unstaged: c.unstaged, untracked: c.untracked
            });
        }
    }

    // 3. LAST_COMMIT
    if fields.contains(FieldSet::LAST_COMMIT) {
        if let Some(p) = path {
            let (timestamp, hash, subject) = get_commit_metadata(p, &ctx.git);
            emit!(P::LastCommit { timestamp, hash, subject });
        }
    }

    // 4. BRANCH_AGE (skip detached)
    if fields.contains(FieldSet::BRANCH_AGE) && !target.is_detached {
        if let Some(p) = path {
            let v = get_branch_creation_timestamp(&target.branch_name, p);
            emit!(P::BranchAge(v));
        }
    }

    // 5. OWNER (skip detached)
    if fields.contains(FieldSet::OWNER) && !target.is_detached {
        if let Some(p) = path {
            let owner = ownership::resolve_owner_with_fallbacks(
                &ctx.base_branch,
                &target.branch_name,
                p,
                ctx.ownership_strategy,
                ctx.user_email.as_deref(),
                Some(&ctx.remote_name),
            );
            emit!(P::Owner(owner));
        }
    }

    // 6. REMOTE_AHEAD_BEHIND (skip detached)
    if fields.contains(FieldSet::REMOTE_AHEAD_BEHIND) && !target.is_detached {
        if let Some(p) = path {
            let v = get_upstream_ahead_behind(&target.branch_name, p);
            emit!(P::RemoteAheadBehind(v));
        }
    }

    // 7. Stat::Lines clusters
    if matches!(stat, Stat::Lines) {
        if fields.contains(FieldSet::BASE_LINES) && !target.is_detached {
            if let Some(p) = path {
                let v = get_base_line_counts(&ctx.base_branch, &target.branch_name, p);
                emit!(P::BaseLines(v));
            }
        }
        if fields.contains(FieldSet::CHANGES_LINES) {
            if let Some(p) = path {
                let (s, u) = count_changed_lines(p);
                emit!(P::ChangesLines { staged: s, unstaged: u });
            }
        }
        if fields.contains(FieldSet::REMOTE_LINES) && !target.is_detached {
            if let Some(p) = path {
                let v = get_remote_line_counts(&target.branch_name, p);
                emit!(P::RemoteLines(v));
            }
        }
    }

    // 8. SIZE (slowest cluster)
    if fields.contains(FieldSet::SIZE) {
        if let Some(p) = path {
            emit!(P::Size(compute_directory_size(p)));
        }
    }

    // 9. MTIME
    if fields.contains(FieldSet::MTIME) {
        if let Some(p) = path {
            // Re-count just to get the path list — cheap relative to mtime walk.
            let c = count_changed_files(p);
            if !c.paths.is_empty() {
                emit!(P::Mtime(max_mtime_of_files(p, &c.paths)));
            } else {
                emit!(P::Mtime(None));
            }
        }
    }
}
```

- [ ] **Step 3: Add a fixture-repo integration test**

Use the existing test-fixture pattern from `src/core/worktree/list.rs` tests if
one exists; otherwise scaffold a minimal one:

```rust
#[cfg(test)]
mod fixture_tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn init_temp_repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        Command::new("git").arg("init").arg("-q").current_dir(p).status().unwrap();
        Command::new("git").args(["config", "user.email", "test@test.com"])
            .current_dir(p).status().unwrap();
        Command::new("git").args(["config", "user.name", "test"])
            .current_dir(p).status().unwrap();
        std::fs::write(p.join("README"), "hello").unwrap();
        Command::new("git").args(["add", "."]).current_dir(p).status().unwrap();
        Command::new("git").args(["commit", "-q", "-m", "init"])
            .current_dir(p).status().unwrap();
        Command::new("git").args(["branch", "-M", "master"])
            .current_dir(p).status().unwrap();
        dir
    }

    #[test]
    fn collector_emits_changes_and_last_commit_for_a_real_repo() {
        let dir = init_temp_repo();
        let (tx, rx) = mpsc::channel();
        let ctx = Arc::new(CollectorContext {
            git: GitCommand::new(false),
            base_branch: "master".into(),
            remote_name: "origin".into(),
            ownership_strategy: OwnershipStrategy::default(),
            user_email: Some("test@test.com".into()),
        });
        let target = CollectorTarget {
            branch_name: "master".into(),
            path: Some(dir.path().to_path_buf()),
            kind: EntryKind::Worktree,
            is_detached: false,
        };
        let fields = FieldSet::CHANGES | FieldSet::LAST_COMMIT;
        let handle = spawn(
            CollectorRequest {
                targets: vec![target],
                fields,
                stat: Stat::Summary,
                source: PatchSource::Collector,
                ctx,
            },
            tx,
        );
        handle.join();

        let events: Vec<DagEvent> = rx.iter().collect();
        let patches: Vec<_> = events.iter()
            .filter_map(|e| match e {
                DagEvent::WorktreeInfoUpdated { patch, .. } => Some(patch),
                _ => None,
            })
            .collect();

        assert!(patches.iter().any(|p|
            matches!(p, crate::core::worktree::sync_dag::WorktreeInfoPatch::Changes { .. })
        ));
        assert!(patches.iter().any(|p|
            matches!(p, crate::core::worktree::sync_dag::WorktreeInfoPatch::LastCommit { .. })
        ));
        // Did NOT request SIZE — must not appear.
        assert!(!patches.iter().any(|p|
            matches!(p, crate::core::worktree::sync_dag::WorktreeInfoPatch::Size(_))
        ));
        assert!(matches!(events.last(), Some(DagEvent::WorktreeInfoCollectionDone)));
    }
}
```

(If `tempfile` is not yet a dev-dependency, check `Cargo.toml` and add
`tempfile = "3"` under `[dev-dependencies]` only.)

- [ ] **Step 4: Run tests**

Run: `cargo test --lib core::worktree::list_stream` Expected: all collector
tests pass.

- [ ] **Step 5: Run clippy + full unit suite**

Run: `mise run clippy && mise run test:unit` Expected: zero warnings, all tests
pass.

- [ ] **Step 6: Commit**

```bash
git add src/core/worktree/list_stream.rs src/core/worktree/list.rs Cargo.toml Cargo.lock
git commit -m "feat(core): implement streaming collector worker (#402)"
```

---

## Task 8: Stale-source suppression helper

**Files:**

- Modify: `src/core/worktree/sync_dag.rs`

Helper used by `LiveTable` in Task 9 to decide whether to apply a patch based on
the per-(branch, field) source priority log.

- [ ] **Step 1: Add the helper type**

Append to `src/core/worktree/sync_dag.rs`:

```rust
use crate::core::worktree::info_field::FieldSet;
use std::collections::HashMap;

/// Tracks which `PatchSource` last wrote each (branch, field) pair.
/// Used by `LiveTable` to suppress patches arriving from a lower-priority
/// source after a higher-priority source has already filled a field.
#[derive(Debug, Default)]
pub struct PatchSourceLog {
    last_writer: HashMap<String, Vec<(FieldSet, PatchSource)>>,
}

impl PatchSourceLog {
    /// Returns `true` if `source` is allowed to write `fields` for `branch`.
    /// Updates internal state to record the new write.
    pub fn try_admit(
        &mut self,
        branch: &str,
        fields: FieldSet,
        source: PatchSource,
    ) -> bool {
        let entries = self.last_writer.entry(branch.to_string()).or_default();
        // If any existing entry overlaps with `fields` and has a strictly
        // higher priority, reject.
        for (existing_fields, existing_source) in entries.iter() {
            if existing_fields.intersects(fields)
                && existing_source.priority() > source.priority()
            {
                return false;
            }
        }
        // Admit. Record (fields, source); we don't bother garbage-collecting
        // overlapping entries — `intersects` checks above are O(entries) and
        // the entry count per branch is bounded by the number of patch
        // clusters (~11).
        entries.push((fields, source));
        true
    }
}

#[cfg(test)]
mod patch_source_log_tests {
    use super::*;

    #[test]
    fn collector_then_post_fetch_admits_post_fetch() {
        let mut log = PatchSourceLog::default();
        assert!(log.try_admit("a", FieldSet::REMOTE_AHEAD_BEHIND, PatchSource::Collector));
        assert!(log.try_admit("a", FieldSet::REMOTE_AHEAD_BEHIND, PatchSource::PostFetch));
    }

    #[test]
    fn post_fetch_then_collector_rejects_collector() {
        let mut log = PatchSourceLog::default();
        assert!(log.try_admit("a", FieldSet::REMOTE_AHEAD_BEHIND, PatchSource::PostFetch));
        assert!(!log.try_admit("a", FieldSet::REMOTE_AHEAD_BEHIND, PatchSource::Collector));
    }

    #[test]
    fn disjoint_field_sets_do_not_block_each_other() {
        let mut log = PatchSourceLog::default();
        assert!(log.try_admit("a", FieldSet::SIZE, PatchSource::PostFetch));
        // Different field — Collector still allowed.
        assert!(log.try_admit("a", FieldSet::CHANGES, PatchSource::Collector));
    }

    #[test]
    fn different_branches_are_independent() {
        let mut log = PatchSourceLog::default();
        assert!(log.try_admit("a", FieldSet::SIZE, PatchSource::PostFetch));
        assert!(log.try_admit("b", FieldSet::SIZE, PatchSource::Collector));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib core::worktree::sync_dag::patch_source_log_tests`
Expected: 4 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/core/worktree/sync_dag.rs
git commit -m "feat(core): add PatchSourceLog for stale-patch suppression (#402)"
```

---

## Task 9: Extract `LiveTable` from `TuiState`

**Files:**

- Create: `src/output/tui/live_table.rs`
- Modify: `src/output/tui/state.rs`
- Modify: `src/output/tui/mod.rs`
- Modify: `src/output/tui/render.rs`

Move the worktree-rows portion of `TuiState` into `LiveTable`. `TuiState` keeps
phases + hook summaries and embeds a `LiveTable` for rows. Render code that
touches rows moves to `LiveTable::render_rows`; phase/hook render stays in
`render.rs` and reads through the embedded `LiveTable`.

> **Scope note for the engineer:** This is the biggest task in the plan (~250
> LOC moved). Approach in three sub-commits:
>
> 1. Create `LiveTable` with the moved fields and methods (no consumer rewires
>    yet); add ignore-arms in `TuiState::apply_event` for the new events;
>    `LiveTable` is dead code at this point.
> 2. Rewire `TuiState` to embed `LiveTable`; update `TuiState::new` to construct
>    the inner `LiveTable`; update field accesses across `render.rs` and any
>    other consumers.
> 3. Wire `WorktreeInfoUpdated` and `WorktreeInfoCollectionDone` arms in
>    `TuiState::apply_event` to forward to `LiveTable::apply_event`.
>
> Each sub-commit must compile and pass `mise run test:unit`.

- [ ] **Step 1: Create `LiveTable` with moved fields**

Create `src/output/tui/live_table.rs`:

```rust
//! Worktree-rows widget shared by `daft list`, `daft prune`, and `daft sync`.
//!
//! Owns: row collection, sort, owner-partition, column selection, patch
//! application, loading-glyph state. Knows nothing about phases or hook
//! sub-rows — those live in the wrapping `OperationTable` / `TuiState`.

use crate::{
    core::{
        sort::SortSpec,
        worktree::{
            info_field::FieldSet,
            list::{EntryKind, Stat, WorktreeInfo},
            sync_dag::{DagEvent, PatchSourceLog},
        },
    },
    output::tui::columns::Column,
};
use std::path::PathBuf;

use super::state::WorktreeRow;  // existing struct stays where it is

#[derive(Clone)]
pub struct LiveTableConfig {
    pub stat: Stat,
    pub columns: Option<Vec<Column>>,
    pub columns_explicit: bool,
    pub sort_spec: Option<SortSpec>,
    /// `true` for prune/sync (anchor on the operation's trunk),
    /// `false` for `daft list` (pure --sort).
    pub pin_default_branch: bool,
    /// `true` for prune/sync (split rows by `is_owned`),
    /// `false` for `daft list` (no partition).
    pub partition_by_owner: bool,
    pub project_root: PathBuf,
    pub cwd: PathBuf,
}

pub struct LiveTable {
    pub rows: Vec<WorktreeRow>,
    pub cfg: LiveTableConfig,
    pub pending_resort: bool,
    pub collection_complete: bool,
    pub source_log: PatchSourceLog,
    /// Per-row bitmask of "patches received". Used to render the loading
    /// glyph for cells that are `None` AND have not yet received a patch.
    pub received_patches: Vec<FieldSet>,
    /// Index of the first row in the unowned section (or `None` if no
    /// partition). Recomputed on each `tick` when `partition_by_owner` is
    /// true.
    pub unowned_start_index: Option<usize>,
}

impl LiveTable {
    pub fn new(seed: Vec<WorktreeInfo>, cfg: LiveTableConfig) -> Self {
        let received_patches = vec![FieldSet::EMPTY; seed.len()];
        let rows: Vec<WorktreeRow> = seed.into_iter()
            .map(WorktreeRow::idle)  // helper added below
            .collect();
        let mut t = Self {
            rows, cfg, pending_resort: true, collection_complete: false,
            source_log: PatchSourceLog::default(),
            received_patches,
            unowned_start_index: None,
        };
        t.resort_and_repartition();
        t
    }

    pub fn apply_event(&mut self, event: &DagEvent) {
        match event {
            DagEvent::WorktreeInfoUpdated { branch_name, patch, source } => {
                let touched = match self.find_row_idx(branch_name) {
                    Some(idx) => {
                        // Compute the FieldSet this patch claims to write
                        // (without mutating yet) so we can consult the
                        // source log first.
                        let claim = patch_field_claim(patch);
                        if !self.source_log.try_admit(branch_name, claim, *source) {
                            return;
                        }
                        let touched = self.rows[idx].info.apply_patch(patch);
                        self.received_patches[idx] |= touched;
                        touched
                    }
                    None => return,
                };
                if let Some(spec) = &self.cfg.sort_spec {
                    if touched.intersects(spec.required_fields()) {
                        self.pending_resort = true;
                    }
                }
                if self.cfg.partition_by_owner && touched.contains(FieldSet::OWNER) {
                    self.pending_resort = true;
                }
            }
            DagEvent::WorktreeInfoCollectionDone => {
                self.collection_complete = true;
                self.pending_resort = true;
            }
            _ => { /* not our concern — phase/hook events handled by wrapper */ }
        }
    }

    pub fn tick(&mut self) {
        if self.pending_resort {
            self.resort_and_repartition();
            self.pending_resort = false;
        }
    }

    fn find_row_idx(&self, branch: &str) -> Option<usize> {
        self.rows.iter().position(|r| r.info.name == branch)
    }

    fn resort_and_repartition(&mut self) {
        let pin = self.cfg.pin_default_branch;
        let sort_spec = self.cfg.sort_spec.clone();
        // Stable sort to preserve relative order on ties.
        let mut indexed: Vec<usize> = (0..self.rows.len()).collect();
        indexed.sort_by(|&a, &b| {
            let ra = &self.rows[a]; let rb = &self.rows[b];
            // 1. Default branch first if pinned.
            if pin {
                let da = u8::from(!ra.info.is_default_branch);
                let db = u8::from(!rb.info.is_default_branch);
                let c = da.cmp(&db);
                if c != std::cmp::Ordering::Equal { return c; }
            }
            // 2. Kind ordering: Worktree < LocalBranch < RemoteBranch.
            let kind = |k: &EntryKind| match k {
                EntryKind::Worktree => 0,
                EntryKind::LocalBranch => 1,
                EntryKind::RemoteBranch => 2,
            };
            let c = kind(&ra.info.kind).cmp(&kind(&rb.info.kind));
            if c != std::cmp::Ordering::Equal { return c; }
            // 3. User sort spec, or alphabetical fallback.
            match &sort_spec {
                Some(spec) => spec.compare(&ra.info, &rb.info),
                None => ra.info.name.to_lowercase().cmp(&rb.info.name.to_lowercase()),
            }
        });

        // Apply the permutation to both rows and received_patches.
        let mut new_rows: Vec<WorktreeRow> = Vec::with_capacity(self.rows.len());
        let mut new_recv: Vec<FieldSet> = Vec::with_capacity(self.received_patches.len());
        for &i in &indexed {
            new_rows.push(std::mem::replace(&mut self.rows[i], WorktreeRow::placeholder()));
            new_recv.push(self.received_patches[i]);
        }
        self.rows = new_rows;
        self.received_patches = new_recv;

        // Recompute partition index.
        self.unowned_start_index = if self.cfg.partition_by_owner {
            self.rows.iter().position(|r| r.info.owner.is_none())
        } else {
            None
        };
    }

    /// True when the cell for `field` on `row_idx` should render the
    /// loading glyph. Only meaningful while !collection_complete.
    pub fn is_cell_loading(&self, row_idx: usize, field: FieldSet) -> bool {
        !self.collection_complete && !self.received_patches[row_idx].contains(field)
    }
}

fn patch_field_claim(patch: &crate::core::worktree::sync_dag::WorktreeInfoPatch) -> FieldSet {
    use crate::core::worktree::sync_dag::WorktreeInfoPatch as P;
    match patch {
        P::BaseAheadBehind(_)   => FieldSet::BASE_AHEAD_BEHIND,
        P::RemoteAheadBehind(_) => FieldSet::REMOTE_AHEAD_BEHIND,
        P::Changes { .. }       => FieldSet::CHANGES,
        P::LastCommit { .. }    => FieldSet::LAST_COMMIT,
        P::BranchAge(_)         => FieldSet::BRANCH_AGE,
        P::Owner(_)             => FieldSet::OWNER,
        P::BaseLines(_)         => FieldSet::BASE_LINES,
        P::ChangesLines { .. }  => FieldSet::CHANGES_LINES,
        P::RemoteLines(_)       => FieldSet::REMOTE_LINES,
        P::Size(_)              => FieldSet::SIZE,
        P::Mtime(_)             => FieldSet::MTIME,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::worktree::sync_dag::{PatchSource, WorktreeInfoPatch};

    fn cfg() -> LiveTableConfig {
        LiveTableConfig {
            stat: Stat::Summary,
            columns: None,
            columns_explicit: false,
            sort_spec: None,
            pin_default_branch: true,
            partition_by_owner: false,
            project_root: PathBuf::from("/tmp"),
            cwd: PathBuf::from("/tmp"),
        }
    }

    fn info(name: &str) -> WorktreeInfo {
        WorktreeInfo::empty(name)
    }

    #[test]
    fn collection_done_sets_collection_complete() {
        let mut t = LiveTable::new(vec![info("a")], cfg());
        assert!(!t.collection_complete);
        t.apply_event(&DagEvent::WorktreeInfoCollectionDone);
        assert!(t.collection_complete);
    }

    #[test]
    fn updated_event_for_unknown_branch_is_ignored() {
        let mut t = LiveTable::new(vec![info("a")], cfg());
        t.apply_event(&DagEvent::WorktreeInfoUpdated {
            branch_name: "b".into(),
            patch: WorktreeInfoPatch::Size(Some(123)),
            source: PatchSource::Collector,
        });
        assert_eq!(t.rows[0].info.size_bytes, None);
    }

    #[test]
    fn patch_applied_marks_received_for_loading_glyph() {
        let mut t = LiveTable::new(vec![info("a")], cfg());
        assert!(t.is_cell_loading(0, FieldSet::SIZE));
        t.apply_event(&DagEvent::WorktreeInfoUpdated {
            branch_name: "a".into(),
            patch: WorktreeInfoPatch::Size(Some(123)),
            source: PatchSource::Collector,
        });
        assert!(!t.is_cell_loading(0, FieldSet::SIZE));
        assert_eq!(t.rows[0].info.size_bytes, Some(123));
    }

    #[test]
    fn collector_patch_is_dropped_after_post_fetch_for_same_field() {
        let mut t = LiveTable::new(vec![info("a")], cfg());
        t.apply_event(&DagEvent::WorktreeInfoUpdated {
            branch_name: "a".into(),
            patch: WorktreeInfoPatch::RemoteAheadBehind(Some((5, 0))),
            source: PatchSource::PostFetch,
        });
        // Collector patch arriving late must be ignored.
        t.apply_event(&DagEvent::WorktreeInfoUpdated {
            branch_name: "a".into(),
            patch: WorktreeInfoPatch::RemoteAheadBehind(Some((1, 1))),
            source: PatchSource::Collector,
        });
        assert_eq!(t.rows[0].info.remote_ahead, Some(5));
        assert_eq!(t.rows[0].info.remote_behind, Some(0));
    }
}
```

- [ ] **Step 2: Add `WorktreeRow::idle` and `placeholder` helpers**

In `src/output/tui/state.rs`, inside `impl WorktreeRow` (add the impl block if
missing), append:

```rust
impl WorktreeRow {
    pub(crate) fn idle(info: WorktreeInfo) -> Self {
        Self {
            info,
            status: WorktreeStatus::Idle,
            prev_terminal_status: None,
            hook_warned: false,
            hook_failed: false,
            hook_sub_rows: Vec::new(),
            failure_reason: None,
        }
    }

    /// Used during the resort permutation. Replaced before any read.
    pub(crate) fn placeholder() -> Self {
        Self::idle(WorktreeInfo::empty(""))
    }
}
```

- [ ] **Step 3: Re-export `LiveTable`**

In `src/output/tui/mod.rs`, add:

```rust
pub mod live_table;
pub use live_table::{LiveTable, LiveTableConfig};
```

- [ ] **Step 4: Run unit tests**

Run: `cargo test --lib output::tui::live_table` Expected: 4 tests pass.

- [ ] **Step 5: Commit (sub-commit 1 of 3)**

```bash
git add src/output/tui/live_table.rs src/output/tui/state.rs src/output/tui/mod.rs
git commit -m "feat(tui): introduce LiveTable widget (#402)"
```

- [ ] **Step 6: Embed `LiveTable` inside `TuiState`**

In `src/output/tui/state.rs`:

1. Remove the following fields from `TuiState`: `worktrees`, `project_root`,
   `cwd`, `stat`, `columns`, `columns_explicit`, `unowned_start_index`,
   `sort_spec`. (They live inside the embedded `LiveTable` now.)
2. Add `pub live: LiveTable` to `TuiState`.
3. Update `TuiState::new` to build a
   `LiveTableConfig { pin_default_branch: true, partition_by_owner: true, ... }`
   (preserves today's prune/sync behavior) and call
   `LiveTable::new(worktree_infos, cfg)`.
4. Update `TuiState::tick` to delegate to `self.live.tick()`.

For every external read of the moved fields (in `src/output/tui/render.rs`,
`src/output/tui/operation_table.rs`, etc.), replace `state.worktrees` with
`state.live.rows`, `state.cwd` with `state.live.cfg.cwd`, etc.

Run: `cargo build` Expected: compiles after all references are updated.

Run: `mise run test:unit` Expected: all existing tests pass.

- [ ] **Step 7: Commit (sub-commit 2 of 3)**

```bash
git add -u
git commit -m "refactor(tui): embed LiveTable inside TuiState (#402)"
```

- [ ] **Step 8: Wire row events from `TuiState::apply_event`**

In `src/output/tui/state.rs`, replace the no-op arms added in Task 5 with:

```rust
            DagEvent::WorktreeInfoUpdated { .. }
            | DagEvent::WorktreeInfoCollectionDone => {
                self.live.apply_event(event);
            }
```

Run: `mise run test:unit && mise run clippy` Expected: zero warnings, all tests
pass.

- [ ] **Step 9: Commit (sub-commit 3 of 3)**

```bash
git add src/output/tui/state.rs
git commit -m "feat(tui): forward WorktreeInfo events from TuiState to LiveTable (#402)"
```

---

## Task 10: Add `pin_default_branch` and `partition_by_owner` to `TableConfig`

**Files:**

- Modify: `src/output/tui/operation_table.rs`

`OperationTable` is the public wrapper used by prune/sync today. Surfacing the
new config knobs here lets future callers (and `daft list` in Phase 2) pick
non-default behavior. Defaults preserve today's behavior.

- [ ] **Step 1: Extend `TableConfig`**

In `src/output/tui/operation_table.rs`, add to `pub struct TableConfig`:

```rust
    /// Pin the default branch to the first row regardless of `--sort`.
    /// Defaults to `true` (prune/sync behavior). `daft list` will set
    /// `false` in Phase 2.
    pub pin_default_branch: bool,
    /// Split rows into "owned" and "unowned" sections by `info.owner`.
    /// Defaults to `true` (prune/sync behavior). `daft list` will set
    /// `false` in Phase 2.
    pub partition_by_owner: bool,
```

Update `OperationTable::run` to thread these into the `LiveTableConfig`
constructed inside `TuiState::new`. (May require widening `TuiState::new`'s
parameter list — do that, and update callers.)

- [ ] **Step 2: Update prune/sync callsites to set defaults explicitly**

In `src/commands/prune.rs` and `src/commands/sync.rs`, where
`TableConfig { ... }` is constructed, add:

```rust
            pin_default_branch: true,
            partition_by_owner: true,
```

- [ ] **Step 3: Run tests**

Run: `mise run test:unit` Expected: all existing tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/output/tui/operation_table.rs src/commands/prune.rs src/commands/sync.rs src/output/tui/state.rs
git commit -m "feat(tui): add pin_default_branch and partition_by_owner config (#402)"
```

---

## Task 11: Remove `TaskCompleted::updated_info`; sync/prune emit `PostTask` patches

**Files:**

- Modify: `src/core/worktree/sync_dag.rs`
- Modify: `src/commands/sync.rs`
- Modify: `src/commands/prune.rs`
- Modify: `src/core/worktree/sync_dag.rs` (DagExecutor task closure signature)

The orchestrator stops sending whole `WorktreeInfo` snapshots; it sends typed
`PostTask` patches for the fields each task touches.

| Task     | Fields re-emitted as `PostTask(phase)`                    |
| -------- | --------------------------------------------------------- |
| `Update` | `BASE_AHEAD_BEHIND`, `LAST_COMMIT`, `CHANGES`             |
| `Rebase` | `BASE_AHEAD_BEHIND`, `LAST_COMMIT`, `REMOTE_AHEAD_BEHIND` |
| `Push`   | `REMOTE_AHEAD_BEHIND`                                     |
| `Prune`  | (row removed; no patch)                                   |

- [ ] **Step 1: Drop `updated_info` from `TaskCompleted`**

In `src/core/worktree/sync_dag.rs`, remove the
`updated_info: Option<Box<WorktreeInfo>>` field from `DagEvent::TaskCompleted`.
Update the `DagExecutor::run` task closure signature to drop the matching
return-tuple element. Update internal sites that unwrap the field to ignore it.

- [ ] **Step 2: Emit `PostTask` patches from sync's per-task handler**

In `src/commands/sync.rs`, the orchestrator already builds an
`Arc<DaftSettings>` plus the per-branch
`worktree_map: HashMap<String, (PathBuf, bool)>`. Construct a small helper and
call it from each per-branch task closure (Update, Rebase, Push):

```rust
fn spawn_post_task_refresh(
    branch_name: &str,
    phase: OperationPhase,
    fields: FieldSet,
    worktree_map: &HashMap<String, (PathBuf, bool)>,
    settings: &Arc<DaftSettings>,
    base_branch: &str,
    user_email: Option<&str>,
    stat: Stat,
    tx: &mpsc::Sender<DagEvent>,
) {
    let Some((path, _is_main)) = worktree_map.get(branch_name) else { return };
    let target = list_stream::CollectorTarget {
        branch_name: branch_name.to_string(),
        path: Some(path.clone()),
        kind: EntryKind::Worktree,
        is_detached: false,
    };
    let ctx = Arc::new(list_stream::CollectorContext {
        git: GitCommand::new(false).with_gitoxide(settings.use_gitoxide),
        base_branch: base_branch.to_string(),
        remote_name: settings.remote.clone(),
        ownership_strategy: settings.ownership_strategy,
        user_email: user_email.map(|s| s.to_string()),
    });
    let handle = list_stream::spawn(
        list_stream::CollectorRequest {
            targets: vec![target],
            fields,
            stat,
            source: PatchSource::PostTask(phase),
            ctx,
        },
        tx.clone(),
    );
    handle.join();   // block briefly so patches land before the next task starts
}
```

Call it from the Update / Rebase / Push branches of `executor.run`'s task
closure with the field sets from the table above. Prune is a no-op (row
removed).

(`handle.join()` here is a deliberate small blocking wait — the per-task refresh
is a few git calls and we want its patches to land before the next task starts
so `LiveTable` doesn't briefly show stale values. If this shows up as a hot spot
in benchmarks later, switch to dropping the handle and accepting the brief
staleness window.)

- [ ] **Step 3: Apply the same to prune (no-op for Prune task)**

In `src/commands/prune.rs`, remove any `updated_info` construction. The prune
task removes the row; emit no patch.

- [ ] **Step 4: Update tests in `src/core/worktree/sync_dag.rs`**

Existing `let events: Vec<DagEvent> = rx.iter().collect()` tests around line 776
need to drop assertions about `updated_info`. Replace with assertions about the
absence of the field and the new pattern.

- [ ] **Step 5: Run unit + integration suite**

Run: `mise run test:unit` Expected: all unit tests pass.

Run: `mise run test:integration` Expected: all existing prune/sync integration
tests pass with the new event shape (behavior unchanged).

- [ ] **Step 6: Commit**

```bash
git add src/core/worktree/sync_dag.rs src/commands/sync.rs src/commands/prune.rs
git commit -m "refactor(orchestrator): replace TaskCompleted::updated_info with PostTask patches (#402)"
```

---

## Task 12: Final verification

- [ ] **Step 1: Run the full CI matrix locally**

Run: `mise run ci` Expected: zero warnings, all unit + integration tests pass.

- [ ] **Step 2: Manual smoke test**

Run: `mise run dev` then in another terminal:

```bash
cd /tmp && daft list   # should behave exactly as before today
daft prune             # should behave exactly as before today
daft sync              # should behave exactly as before today
```

Expected: identical UX to before this branch. No spinners removed yet (that
lands in Phase 2/3); no new live behavior visible.

- [ ] **Step 3: Push branch and open PR**

```bash
git push -u origin daft-402/feat/live-list-population
gh pr create --title "feat: shared infrastructure for live list population (#402)" \
  --body "$(cat <<'EOF'
## Summary

Phase 1 of 3 for #402. Adds shared infrastructure for the live cell-by-cell
population work without changing any user-visible behavior. Phases 2
(`daft list` live UX) and 3 (migrate prune/sync to streaming seed) ship in
follow-up PRs.

What lands:
- `FieldSet` bitmask, `WorktreeInfoPatch`, `PatchSource`
- `DagEvent::WorktreeInfoUpdated`, `DagEvent::WorktreeInfoCollectionDone`
- `WorktreeInfo::apply_patch`, `SortSpec::required_fields`
- Streaming collector (`src/core/worktree/list_stream.rs`)
- `LiveTable` widget extracted from `TuiState`
- `TaskCompleted::updated_info` removed; orchestrator now emits `PostTask`
  patches with the same end-state information

Existing prune/sync integration tests pass unchanged.

Spec: `docs/superpowers/specs/2026-04-25-live-list-population-design.md`

## Test plan
- [x] `mise run test:unit` passes
- [x] `mise run test:integration` passes
- [x] `mise run clippy` zero warnings
- [x] Manual smoke: `daft list`, `daft prune`, `daft sync` behave as before

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Expected: PR opened against master, CI green.

---

## Self-Review Notes

Performed inline before save:

- **Spec coverage**: every Phase 1 deliverable in the spec maps to one or more
  tasks (FieldSet=T1, required_fields=T2, patches+source=T3, apply_patch=T4,
  DagEvent variants=T5, collector=T6+T7, source-log=T8, LiveTable=T9, config
  knobs=T10, orchestrator migration=T11, verification=T12).
- **Placeholders**: none. All steps include exact code + commands.
- **Type consistency**: `FieldSet`, `WorktreeInfoPatch`, `PatchSource`,
  `LiveTable`, `LiveTableConfig`, `CollectorRequest`, `CollectorTarget`,
  `CollectorContext`, `CollectorHandle`, `PatchSourceLog` are used consistently
  across tasks.
- **Scope**: Phase 1 only. Phases 2 and 3 explicitly deferred to separate plans,
  written when those phases begin.
- **Ambiguity**: Task 9 calls out the three sub-commit decomposition explicitly
  so the engineer doesn't try to land a single 250-LOC commit.
