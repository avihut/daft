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

    pub const BASE_AHEAD_BEHIND: Self = Self(1 << 0);
    pub const REMOTE_AHEAD_BEHIND: Self = Self(1 << 1);
    pub const CHANGES: Self = Self(1 << 2);
    pub const LAST_COMMIT: Self = Self(1 << 3);
    pub const BRANCH_AGE: Self = Self(1 << 4);
    pub const OWNER: Self = Self(1 << 5);
    pub const BASE_LINES: Self = Self(1 << 6);
    pub const CHANGES_LINES: Self = Self(1 << 7);
    pub const REMOTE_LINES: Self = Self(1 << 8);
    pub const SIZE: Self = Self(1 << 9);
    pub const MTIME: Self = Self(1 << 10);
    pub const FORGE_REF: Self = Self(1 << 11);

    /// Fields whose values can change after a `git fetch`.
    pub const REMOTE_DERIVED: Self = Self(Self::REMOTE_AHEAD_BEHIND.0 | Self::REMOTE_LINES.0);

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

    /// Every bit set, including bits with no assigned field. Acts as the
    /// fully-received sentinel: a row whose seeded + patched bits reach
    /// `ALL` has nothing in flight (the renderer's inflight counter checks
    /// exactly this), and views with no streaming collector seed it
    /// outright (`repo remove`). The live list view requests a narrowed
    /// set instead (`collector_fields`) and seeds the complement.
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
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for FieldSet {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for FieldSet {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl Not for FieldSet {
    type Output = Self;
    fn not(self) -> Self {
        Self(!self.0)
    }
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
    fn complement_of_a_request_unions_back_to_all() {
        // The live table seeds rows with the complement of the collector
        // request and treats a row as settled once its bits reach `ALL`.
        // That contract holds only while `ALL` is the full `u32` and `Not`
        // is the bitwise complement.
        let requested = FieldSet::CHANGES | FieldSet::OWNER;
        assert_eq!(requested | !requested, FieldSet::ALL);
        assert_eq!(!FieldSet::EMPTY, FieldSet::ALL);
    }

    #[test]
    fn all_contains_every_known_member() {
        for member in [
            FieldSet::BASE_AHEAD_BEHIND,
            FieldSet::REMOTE_AHEAD_BEHIND,
            FieldSet::CHANGES,
            FieldSet::LAST_COMMIT,
            FieldSet::BRANCH_AGE,
            FieldSet::OWNER,
            FieldSet::BASE_LINES,
            FieldSet::CHANGES_LINES,
            FieldSet::REMOTE_LINES,
            FieldSet::SIZE,
            FieldSet::MTIME,
            FieldSet::FORGE_REF,
        ] {
            assert!(FieldSet::ALL.contains(member));
        }
    }
}
