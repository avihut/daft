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

use super::state::WorktreeRow;

#[derive(Clone)]
pub struct LiveTableConfig {
    pub stat: Stat,
    pub columns: Option<Vec<Column>>,
    pub columns_explicit: bool,
    pub sort_spec: Option<SortSpec>,
    /// `true` for prune/sync, `false` for `daft list`.
    pub pin_default_branch: bool,
    /// `true` for prune/sync, `false` for `daft list`.
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
    /// Per-row bitmask of "patches received".
    pub received_patches: Vec<FieldSet>,
    /// Index of the first row in the unowned section, or `None` if no
    /// partition. Recomputed when `partition_by_owner` is true.
    pub unowned_start_index: Option<usize>,
}

impl LiveTable {
    pub fn new(seed: Vec<WorktreeInfo>, cfg: LiveTableConfig) -> Self {
        let received_patches = vec![FieldSet::EMPTY; seed.len()];
        let rows: Vec<WorktreeRow> = seed.into_iter().map(WorktreeRow::idle).collect();
        let mut t = Self {
            rows,
            cfg,
            pending_resort: true,
            collection_complete: false,
            source_log: PatchSourceLog::default(),
            received_patches,
            unowned_start_index: None,
        };
        t.resort_and_repartition();
        t
    }

    pub fn apply_event(&mut self, event: &DagEvent) {
        match event {
            DagEvent::WorktreeInfoUpdated {
                branch_name,
                patch,
                source,
            } => {
                let touched = match self.find_row_idx(branch_name) {
                    Some(idx) => {
                        let claim = patch_field_claim(patch);
                        // PatchSource is Clone (not Copy) because it carries
                        // OperationPhase which contains a String-bearing variant.
                        if !self
                            .source_log
                            .try_admit(branch_name, claim, source.clone())
                        {
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
            _ => { /* phase/hook events handled by wrapper */ }
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
        let mut indexed: Vec<usize> = (0..self.rows.len()).collect();
        indexed.sort_by(|&a, &b| {
            let ra = &self.rows[a];
            let rb = &self.rows[b];
            if pin {
                let da = u8::from(!ra.info.is_default_branch);
                let db = u8::from(!rb.info.is_default_branch);
                let c = da.cmp(&db);
                if c != std::cmp::Ordering::Equal {
                    return c;
                }
            }
            let kind = |k: &EntryKind| match k {
                EntryKind::Worktree => 0,
                EntryKind::LocalBranch => 1,
                EntryKind::RemoteBranch => 2,
            };
            let c = kind(&ra.info.kind).cmp(&kind(&rb.info.kind));
            if c != std::cmp::Ordering::Equal {
                return c;
            }
            match &sort_spec {
                Some(spec) => spec.compare(&ra.info, &rb.info),
                None => ra
                    .info
                    .name
                    .to_lowercase()
                    .cmp(&rb.info.name.to_lowercase()),
            }
        });

        let mut new_rows: Vec<WorktreeRow> = Vec::with_capacity(self.rows.len());
        let mut new_recv: Vec<FieldSet> = Vec::with_capacity(self.received_patches.len());
        for &i in &indexed {
            new_rows.push(std::mem::replace(
                &mut self.rows[i],
                WorktreeRow::placeholder(),
            ));
            new_recv.push(self.received_patches[i]);
        }
        self.rows = new_rows;
        self.received_patches = new_recv;

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

    /// Append a new row, keeping `received_patches` in lockstep so
    /// `is_cell_loading` cannot index out of bounds. Used when a
    /// dynamically-discovered branch (e.g. a gone branch surfaced after
    /// fetch) gets a row.
    pub fn push_row(&mut self, info: WorktreeInfo) {
        self.rows.push(WorktreeRow::idle(info));
        self.received_patches.push(FieldSet::EMPTY);
    }
}

fn patch_field_claim(patch: &crate::core::worktree::sync_dag::WorktreeInfoPatch) -> FieldSet {
    use crate::core::worktree::sync_dag::WorktreeInfoPatch as P;
    match patch {
        P::BaseAheadBehind(_) => FieldSet::BASE_AHEAD_BEHIND,
        P::RemoteAheadBehind(_) => FieldSet::REMOTE_AHEAD_BEHIND,
        P::Changes { .. } => FieldSet::CHANGES,
        P::LastCommit { .. } => FieldSet::LAST_COMMIT,
        P::BranchAge(_) => FieldSet::BRANCH_AGE,
        P::Owner(_) => FieldSet::OWNER,
        P::BaseLines(_) => FieldSet::BASE_LINES,
        P::ChangesLines { .. } => FieldSet::CHANGES_LINES,
        P::RemoteLines(_) => FieldSet::REMOTE_LINES,
        P::Size(_) => FieldSet::SIZE,
        P::Mtime(_) => FieldSet::MTIME,
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
        t.apply_event(&DagEvent::WorktreeInfoUpdated {
            branch_name: "a".into(),
            patch: WorktreeInfoPatch::RemoteAheadBehind(Some((1, 1))),
            source: PatchSource::Collector,
        });
        assert_eq!(t.rows[0].info.remote_ahead, Some(5));
        assert_eq!(t.rows[0].info.remote_behind, Some(0));
    }
}
