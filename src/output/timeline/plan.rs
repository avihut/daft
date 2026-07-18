//! Presentation vocabulary for plan steps.
//!
//! The tense table is the single source of truth for step microcopy:
//! imperative while pending (a promise), gerund while active, past tense once
//! done (a fact). Failed steps deliberately keep the imperative — the fact
//! never happened. Expected skips replace the label entirely (listr2-style
//! "reason replaces title"), e.g. `Delete remote branch` → `remote branch
//! kept`. Skips known at plan-commit time never plan a row in the first
//! place (#651) — the skipped labels serve runtime-discovered resolutions.
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
        // Rendered only as a fallback: the resolve row carries the PR/MR
        // identity ("PR #123") as a fixed label override in every phase.
        StageId::ResolveRef => StepLabels {
            pending: "Resolve reference",
            active: "Resolving reference",
            done: "Resolved reference",
            skipped: "not resolved",
        },
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
        // Rendered only as a fallback: shared-file rows carry the file path
        // as a fixed label override in every phase.
        StageId::SharedFile => StepLabels {
            pending: "Link shared file",
            active: "Linking shared file",
            done: "Linked shared file",
            skipped: "not linked",
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
        // Fallback only: exec rows always carry a fixed label override (the
        // worktree, or the command) — the face glyph alone carries state.
        StageId::ExecCommand => StepLabels {
            pending: "Run command",
            active: "Running command",
            done: "Ran command",
            skipped: "not run",
        },
        // Fallback only: task rows always carry the task name as a fixed
        // label override (noun label in every tense, like the hook phases).
        StageId::Task => StepLabels {
            pending: "task",
            active: "task",
            done: "task",
            skipped: "task not run",
        },
        StageId::ResolveWorktree => StepLabels {
            pending: "Resolve worktree",
            active: "Resolving worktree",
            done: "Resolved worktree",
            skipped: "no worktree",
        },
    }
}

/// Ink class of a row's subject parts (#651): what *kind* of thing the text
/// names. Identity inks are constant across lifecycle states — state stays
/// in the glyph; these mark whose thing the row touches.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SubjectInk {
    /// daft's own vocabulary or free text — carries no ink of its own.
    Plain,
    /// A remote name or ref (`origin`, `← origin/master`, `→ origin/x`) —
    /// ANSI cyan, the network.
    Remote,
    /// An ordinary filesystem path (worktree directory) — manila.
    Path,
    /// A shared file — violet, daft-managed and linked across worktrees.
    Shared,
}

/// Label + annotation inks for one stage.
#[derive(Clone, Copy)]
pub struct SubjectInks {
    pub label: SubjectInk,
    pub annotation: SubjectInk,
}

/// The subject table: which ink each stage's label / annotation wears.
/// Annotation inks apply to the stage's own subject (pending, active, done,
/// `Note` updates); failure details and skip reasons are composed text and
/// always render plain — the caller forces that at resolution time.
pub fn subject_inks_for(id: StageId) -> SubjectInks {
    let (label, annotation) = match id {
        // Remote subjects: the network is cyan.
        StageId::Fetch
        | StageId::Tracking
        | StageId::CreateBranch
        | StageId::CheckOut
        | StageId::Push
        | StageId::DeleteRemote
        | StageId::CloneBare => (SubjectInk::Plain, SubjectInk::Remote),
        // Path subjects: worktree directories are manila.
        StageId::CreateWorktree
        | StageId::CreateBaseWorktree
        | StageId::RemoveWorktree
        | StageId::ResolveWorktree => (SubjectInk::Plain, SubjectInk::Path),
        // Shared files: the row's label IS the path, violet.
        StageId::SharedFile => (SubjectInk::Shared, SubjectInk::Plain),
        // The resolve row's label IS the PR/MR identity ("PR #123"); its
        // annotation is the free-text title. Both plain.
        StageId::ResolveRef => (SubjectInk::Plain, SubjectInk::Plain),
        // Everything else speaks daft's own vocabulary. Exec rows carry a
        // fixed label (worktree or command) and a plain annotation
        // (`exit N`, `cancelled`, latest output line) — no identity ink.
        StageId::Carry
        | StageId::DeleteLocalBranch
        | StageId::Install
        | StageId::ExecCommand
        | StageId::Task
        | StageId::PreCreateHooks
        | StageId::PostCreateHooks
        | StageId::PreRemoveHooks
        | StageId::PostRemoveHooks
        | StageId::PostCloneHooks => (SubjectInk::Plain, SubjectInk::Plain),
    };
    SubjectInks { label, annotation }
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

    #[test]
    fn subject_table_types_the_identities() {
        assert_eq!(
            subject_inks_for(StageId::Push).annotation,
            SubjectInk::Remote
        );
        assert_eq!(
            subject_inks_for(StageId::CreateWorktree).annotation,
            SubjectInk::Path
        );
        assert_eq!(
            subject_inks_for(StageId::SharedFile).label,
            SubjectInk::Shared
        );
        // Free-text annotations (was merged into <default>) stay plain.
        assert_eq!(
            subject_inks_for(StageId::DeleteLocalBranch).annotation,
            SubjectInk::Plain
        );
    }

    #[test]
    fn resolve_worktree_speaks_path() {
        // `daft push` (#600): the resolved worktree is the row's whole story —
        // its annotation is a directory, so it wears the manila path ink.
        let l = labels_for(StageId::ResolveWorktree);
        assert_eq!(l.pending, "Resolve worktree");
        assert_eq!(l.active, "Resolving worktree");
        assert_eq!(l.done, "Resolved worktree");
        assert_eq!(l.skipped, "no worktree");
        assert_eq!(
            subject_inks_for(StageId::ResolveWorktree).annotation,
            SubjectInk::Path
        );
    }
}
