//! Presentation vocabulary for plan steps.
//!
//! The tense table is the single source of truth for step microcopy:
//! imperative while pending (a promise), gerund while active, past tense once
//! done (a fact). Failed steps deliberately keep the imperative — the fact
//! never happened. Expected skips replace the label entirely (listr2-style
//! "reason replaces title"), e.g. `Push` → `not pushed`.
//!
//! Hook phases are noun labels in every tense; when they actually run, the
//! row is replaced by the hook renderer's own block, so only the pending /
//! skipped forms ever render.

use crate::core::stage::StageId;

/// Label set for one stage, by lifecycle.
pub struct StepLabels {
    /// Imperative — pending rows, and failed rows (the fact never happened).
    pub pending: &'static str,
    /// Gerund — the active row.
    pub active: &'static str,
    /// Past tense — the persisted done row.
    pub done: &'static str,
    /// Replacement label for an expected (quiet, dim) skip.
    pub skipped: &'static str,
}

/// The tense table.
pub fn labels_for(id: StageId) -> StepLabels {
    match id {
        StageId::Fetch => StepLabels {
            pending: "Fetch remote",
            active: "Fetching remote",
            done: "Fetched remote",
            skipped: "not fetched",
        },
        StageId::Carry => StepLabels {
            pending: "Carry changes",
            active: "Carrying changes",
            done: "Carried changes",
            skipped: "nothing to carry",
        },
        StageId::PreCreateHooks => StepLabels {
            pending: "pre-create hooks",
            active: "pre-create hooks",
            done: "pre-create hooks",
            skipped: "pre-create hooks",
        },
        StageId::CreateBranch => StepLabels {
            pending: "Create branch",
            active: "Creating branch",
            done: "Created branch",
            skipped: "branch not created",
        },
        StageId::CheckOut => StepLabels {
            pending: "Check out branch",
            active: "Checking out branch",
            done: "Checked out branch",
            skipped: "not checked out",
        },
        StageId::CreateWorktree | StageId::CreateBaseWorktree => StepLabels {
            pending: "Create worktree",
            active: "Creating worktree",
            done: "Created worktree",
            skipped: "worktree not created",
        },
        StageId::Push => StepLabels {
            pending: "Push",
            active: "Pushing",
            done: "Pushed",
            skipped: "not pushed",
        },
        StageId::PostCreateHooks => StepLabels {
            pending: "post-create hooks",
            active: "post-create hooks",
            done: "post-create hooks",
            skipped: "post-create hooks",
        },
        StageId::PreRemoveHooks => StepLabels {
            pending: "pre-remove hooks",
            active: "pre-remove hooks",
            done: "pre-remove hooks",
            skipped: "pre-remove hooks",
        },
        StageId::DeleteRemote => StepLabels {
            pending: "Delete remote branch",
            active: "Deleting remote branch",
            done: "Deleted remote branch",
            skipped: "remote branch kept",
        },
        StageId::RemoveWorktree => StepLabels {
            pending: "Remove worktree",
            active: "Removing worktree",
            done: "Removed worktree",
            skipped: "worktree kept",
        },
        StageId::DeleteLocalBranch => StepLabels {
            pending: "Delete branch",
            active: "Deleting branch",
            done: "Deleted branch",
            skipped: "branch kept",
        },
        StageId::PostRemoveHooks => StepLabels {
            pending: "post-remove hooks",
            active: "post-remove hooks",
            done: "post-remove hooks",
            skipped: "post-remove hooks",
        },
        StageId::CloneBare => StepLabels {
            pending: "Clone repository",
            active: "Cloning repository",
            done: "Cloned repository",
            skipped: "not cloned",
        },
        StageId::Tracking => StepLabels {
            pending: "Set up tracking",
            active: "Setting up tracking",
            done: "Set up tracking",
            skipped: "no tracking",
        },
        StageId::PostCloneHooks => StepLabels {
            pending: "post-clone hooks",
            active: "post-clone hooks",
            done: "post-clone hooks",
            skipped: "post-clone hooks",
        },
        StageId::Install => StepLabels {
            pending: "Install daft",
            active: "Installing daft",
            done: "Installed daft",
            skipped: "not installed",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenses_flip_promise_to_fact() {
        let l = labels_for(StageId::CreateWorktree);
        assert_eq!(l.pending, "Create worktree");
        assert_eq!(l.active, "Creating worktree");
        assert_eq!(l.done, "Created worktree");
    }

    #[test]
    fn hook_phases_are_nouns_in_every_tense() {
        let l = labels_for(StageId::PostCreateHooks);
        assert_eq!(l.pending, l.done);
        assert_eq!(l.pending, "post-create hooks");
    }

    #[test]
    fn expected_skip_replaces_the_label() {
        assert_eq!(labels_for(StageId::Push).skipped, "not pushed");
        assert_eq!(
            labels_for(StageId::DeleteRemote).skipped,
            "remote branch kept"
        );
    }
}
