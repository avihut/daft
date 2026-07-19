//! The annotation column's sub-position layout, shared by both list renderers.
//!
//! The annotation column is a row of narrow slots carrying the *state* of a
//! row — what it is, not what it contains. Slots materialize per run: a slot
//! costs its width only when some row in this run actually uses it, so a
//! listing with no default branch and no operations spends nothing on either.
//!
//! Both the plain table (`commands::list`) and the live TUI
//! (`output::tui::render`) lay the column out from here. They differ only in
//! how they paint a glyph — ANSI escapes versus ratatui styles — so the two
//! surfaces cannot drift on *which* slots exist or what goes in them, which
//! is what happened before: the TUI had two hardcoded sub-positions while the
//! plain table computed three.

use crate::core::worktree::list::WorktreeInfo;
use crate::git::op_state::OpKind;
use crate::styles;

/// What occupies one annotation slot for one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationGlyph {
    /// The worktree the user is currently inside.
    Current,
    /// The repository's default branch.
    DefaultBranch,
    /// A detached checkout nothing explains.
    Sandbox,
    /// A paused git operation.
    Operation(OpKind),
}

impl AnnotationGlyph {
    /// The character to draw. Colour is the renderer's business.
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Current => styles::CURRENT_WORKTREE_SYMBOL,
            Self::DefaultBranch => styles::DEFAULT_BRANCH_SYMBOL,
            Self::Sandbox => styles::SANDBOX_SYMBOL,
            Self::Operation(op) => op.symbol(),
        }
    }
}

/// Which annotation slots this run needs, in display order.
///
/// Computed once from the full row set — never per row, and never per frame:
/// the live renderer recomputes column widths continuously, so a slot set that
/// tracked current row state would make the whole table reflow the moment an
/// operation appeared or ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AnnotationSlots {
    /// Slot 1 — the current-worktree marker.
    pub current: bool,
    /// Slot 2 — what this row *is*: default branch, or unexplained detachment.
    pub identity: bool,
    /// Slot 3 — what is *happening* to it: a paused operation.
    pub state: bool,
}

impl AnnotationSlots {
    /// The slots needed to annotate `infos`.
    pub fn for_rows(infos: &[WorktreeInfo]) -> Self {
        Self {
            current: infos.iter().any(|i| i.is_current),
            identity: infos.iter().any(|i| i.is_default_branch || i.is_sandbox),
            state: infos.iter().any(|i| i.op.is_some()),
        }
    }

    /// How many slots are in play.
    pub fn count(self) -> usize {
        usize::from(self.current) + usize::from(self.identity) + usize::from(self.state)
    }

    /// True when no row needs annotating at all — the column can be dropped.
    pub fn is_empty(self) -> bool {
        self.count() == 0
    }

    /// The rendered width of the column: one column per slot, plus a single
    /// space between adjacent slots.
    pub fn width(self) -> usize {
        match self.count() {
            0 => 0,
            n => n * 2 - 1,
        }
    }

    /// What each active slot holds for `info`, in display order. `None` is an
    /// active slot this row does not fill — it still occupies its width, so
    /// glyphs stay in vertical columns down the table.
    pub fn glyphs(self, info: &WorktreeInfo) -> Vec<Option<AnnotationGlyph>> {
        let mut out = Vec::with_capacity(self.count());
        if self.current {
            out.push(info.is_current.then_some(AnnotationGlyph::Current));
        }
        if self.identity {
            // The default-branch marker outranks the sandbox marker: a row can
            // only be one of them in practice, and this keeps the precedence
            // explicit rather than accidental.
            out.push(if info.is_default_branch {
                Some(AnnotationGlyph::DefaultBranch)
            } else if info.is_sandbox {
                Some(AnnotationGlyph::Sandbox)
            } else {
                None
            });
        }
        if self.state {
            out.push(info.op.map(AnnotationGlyph::Operation));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str) -> WorktreeInfo {
        WorktreeInfo::empty(name)
    }

    #[test]
    fn a_run_with_nothing_to_say_uses_no_slots() {
        let slots = AnnotationSlots::for_rows(&[row("a"), row("b")]);
        assert!(slots.is_empty());
        assert_eq!(slots.width(), 0);
        assert!(slots.glyphs(&row("a")).is_empty());
    }

    #[test]
    fn slots_materialize_only_for_states_present_in_this_run() {
        let mut current = row("a");
        current.is_current = true;
        let slots = AnnotationSlots::for_rows(&[current, row("b")]);

        assert_eq!(
            slots,
            AnnotationSlots {
                current: true,
                identity: false,
                state: false,
            }
        );
        assert_eq!(slots.count(), 1);
        assert_eq!(slots.width(), 1, "a lone slot needs no separator");
    }

    #[test]
    fn an_operation_opens_the_state_slot() {
        let mut rebasing = row("feat/x");
        rebasing.op = Some(OpKind::Rebase);
        let slots = AnnotationSlots::for_rows(&[rebasing.clone(), row("main")]);

        assert!(slots.state);
        assert!(
            !slots.identity,
            "no default branch and no sandbox in this run"
        );
        assert_eq!(
            slots.glyphs(&rebasing),
            vec![Some(AnnotationGlyph::Operation(OpKind::Rebase))]
        );
        // The row without an operation still occupies the slot, so glyphs
        // stay aligned down the column.
        assert_eq!(slots.glyphs(&row("main")), vec![None]);
    }

    #[test]
    fn all_three_slots_can_be_in_play_at_once() {
        let mut current = row("here");
        current.is_current = true;
        let mut default = row("main");
        default.is_default_branch = true;
        let mut rebasing = row("feat/x");
        rebasing.op = Some(OpKind::Rebase);

        let slots =
            AnnotationSlots::for_rows(&[current.clone(), default.clone(), rebasing.clone()]);
        assert_eq!(slots.count(), 3);
        assert_eq!(slots.width(), 5, "three glyphs plus two separators");

        assert_eq!(
            slots.glyphs(&current),
            vec![Some(AnnotationGlyph::Current), None, None]
        );
        assert_eq!(
            slots.glyphs(&default),
            vec![None, Some(AnnotationGlyph::DefaultBranch), None]
        );
        assert_eq!(
            slots.glyphs(&rebasing),
            vec![None, None, Some(AnnotationGlyph::Operation(OpKind::Rebase))]
        );
    }

    /// A sandbox and a rebasing worktree use *different* slots — the whole
    /// point of separating "what it is" from "what is happening to it".
    #[test]
    fn sandbox_and_operation_occupy_different_slots() {
        let mut sandbox = row("(detached)");
        sandbox.is_sandbox = true;
        let mut rebasing = row("feat/x");
        rebasing.op = Some(OpKind::Rebase);

        let slots = AnnotationSlots::for_rows(&[sandbox.clone(), rebasing.clone()]);
        assert_eq!(slots.count(), 2);
        assert_eq!(
            slots.glyphs(&sandbox),
            vec![Some(AnnotationGlyph::Sandbox), None]
        );
        assert_eq!(
            slots.glyphs(&rebasing),
            vec![None, Some(AnnotationGlyph::Operation(OpKind::Rebase))]
        );
    }

    /// A worktree can be mid-operation *and* be the default branch (a merge
    /// keeps HEAD attached), so both slots fill on the same row.
    #[test]
    fn a_merging_default_branch_fills_two_slots() {
        let mut merging = row("main");
        merging.is_default_branch = true;
        merging.op = Some(OpKind::Merge);

        let slots = AnnotationSlots::for_rows(&[merging.clone()]);
        assert_eq!(
            slots.glyphs(&merging),
            vec![
                Some(AnnotationGlyph::DefaultBranch),
                Some(AnnotationGlyph::Operation(OpKind::Merge)),
            ]
        );
    }

    #[test]
    fn every_operation_has_a_distinct_glyph() {
        let kinds = [
            OpKind::Rebase,
            OpKind::Am,
            OpKind::Merge,
            OpKind::CherryPick,
            OpKind::Revert,
            OpKind::Bisect,
        ];
        let symbols: Vec<&str> = kinds
            .iter()
            .map(|k| AnnotationGlyph::Operation(*k).symbol())
            .collect();
        let unique: std::collections::HashSet<&&str> = symbols.iter().collect();
        assert_eq!(unique.len(), kinds.len(), "glyphs collide: {symbols:?}");

        // And none of them collides with the identity markers sharing the row.
        for s in &symbols {
            assert_ne!(*s, styles::SANDBOX_SYMBOL);
            assert_ne!(*s, styles::DEFAULT_BRANCH_SYMBOL);
            assert_ne!(*s, styles::CURRENT_WORKTREE_SYMBOL);
        }
    }
}
