//! Four-phase move hook flow for worktree rename/layout-transform operations.
//!
//! A "move" is any operation that changes a worktree's path, branch, or both.
//! The four phases are:
//! 1. `worktree-pre-remove` — teardown preparation (old identity)
//! 2. `worktree-post-remove` — teardown completion (old identity)
//! 3. `worktree-pre-create` — setup preparation (new identity)
//! 4. `worktree-post-create` — setup completion (new identity)
//!
//! Only jobs whose `tracks` (or implicit template-variable tracking) intersects
//! with the changed attributes are executed in each phase.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::core::{HookRunner, ProgressSink};
use crate::hooks::environment::HookContext;
use crate::hooks::tracking::TrackedAttribute;
use crate::hooks::HookType;

/// Parameters describing a worktree move for hook purposes.
pub struct MoveHookParams {
    pub old_worktree_path: PathBuf,
    pub new_worktree_path: PathBuf,
    pub old_branch_name: String,
    pub new_branch_name: String,
    pub project_root: PathBuf,
    pub git_dir: PathBuf,
    pub remote: String,
    pub source_worktree: PathBuf,
    pub command: String,
    pub changed_attributes: HashSet<TrackedAttribute>,
}

/// Run teardown hooks (pre-remove + post-remove) for tracked jobs with old identity.
pub fn run_teardown_hooks(params: &MoveHookParams, sink: &mut (impl ProgressSink + HookRunner)) {
    for hook_type in [HookType::PreRemove, HookType::PostRemove] {
        let mut ctx = HookContext::new(
            hook_type,
            &params.command,
            params.project_root.clone(),
            params.git_dir.clone(),
            &params.remote,
            params.source_worktree.clone(),
            params.old_worktree_path.clone(),
            &params.old_branch_name,
        );
        ctx.is_move = true;
        ctx.old_worktree_path = Some(params.old_worktree_path.clone());
        ctx.old_branch_name = Some(params.old_branch_name.clone());
        ctx.changed_attributes = Some(params.changed_attributes.clone());

        if let Err(e) = sink.run_hook(&ctx) {
            sink.on_warning(&format!("Move {} hook failed: {e}", hook_type.yaml_name()));
        }
    }
}

/// Run setup hooks (pre-create + post-create) for tracked jobs with new identity.
pub fn run_setup_hooks(params: &MoveHookParams, sink: &mut (impl ProgressSink + HookRunner)) {
    for hook_type in [HookType::PreCreate, HookType::PostCreate] {
        let mut ctx = HookContext::new(
            hook_type,
            &params.command,
            params.project_root.clone(),
            params.git_dir.clone(),
            &params.remote,
            params.source_worktree.clone(),
            params.new_worktree_path.clone(),
            &params.new_branch_name,
        );
        ctx.is_move = true;
        ctx.old_worktree_path = Some(params.old_worktree_path.clone());
        ctx.old_branch_name = Some(params.old_branch_name.clone());
        ctx.changed_attributes = Some(params.changed_attributes.clone());

        if let Err(e) = sink.run_hook(&ctx) {
            sink.on_warning(&format!("Move {} hook failed: {e}", hook_type.yaml_name()));
        }
    }
}
