//! Worktree-rows widget shared by `daft list`, `daft prune`, and `daft sync`.
//!
//! Owns: row collection, sort, owner-partition, column selection, patch
//! application, loading-glyph state. Knows nothing about phases or hook
//! sub-rows — those live in the wrapping `OperationTable` / `TuiState`.

use crate::{
    core::{
        sort::SortSpec,
        worktree::{
            forge_ref::ForgePrLookup,
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
    // Unused by render after #494; pending removal in a follow-up.
    pub columns_explicit: bool,
    pub sort_spec: Option<SortSpec>,
    /// `true` for prune/sync, `false` for `daft list`.
    pub pin_default_branch: bool,
    /// `true` for prune/sync, `false` for `daft list`.
    pub partition_by_owner: bool,
    pub project_root: PathBuf,
    pub cwd: PathBuf,
    /// Fields whose authoritative value is determined before TUI start —
    /// either populated by the synchronous seed, or guaranteed never to
    /// arrive via the streaming collector. Pre-marking these in
    /// `received_patches` prevents the shimmer for cells the streaming
    /// collector won't emit a patch for, including the legitimate-empty
    /// case (e.g. the default branch's owner is `None` by design and
    /// must render as final, not loading). The render path keys shimmer
    /// off `vals.X.is_empty()` rather than the bit alone, so cells with
    /// the bit set but no seed value render as "final empty" rather than
    /// shimmering.
    pub seeded_fields: FieldSet,
    /// Annotation sub-positions this run renders, computed once from the
    /// seeded rows and then held fixed. Column widths are recomputed every
    /// frame, so deriving this from current row state would reflow the whole
    /// table the moment an operation appeared or ended.
    pub annotation_slots: crate::output::annotation::AnnotationSlots,
    /// Forge-PR cache decorations for the PR column (outbound PR numbers +
    /// CI states). Loaded once by `daft list` before the TUI starts and
    /// post-set after `TuiState::new` (like `unowned_start_index`); `None`
    /// for commands that don't decorate. While a refresh is in flight the
    /// seed is stripped to identity (`ForgePrLookup::identity_only`) —
    /// statuses only render once `ForgePrsRefreshed` delivers fresh data, and
    /// the identity breathes in the meantime (`forge_prs_stale`).
    pub forge_prs: Option<ForgePrLookup>,
    /// True while no PR snapshot has *ever* been taken and a refresh is in
    /// flight: PR cells without a value render the loading skeleton until
    /// `ForgePrsRefreshed` concludes the refresh (with or without data).
    /// Unlike per-cell patch state this survives collection completing —
    /// the refresh is out-of-band — but cancel clears it like any shimmer.
    pub forge_prs_loading: bool,
    /// How the PR column's *warm*-cache identities read right now — the PR
    /// analogue of a stale size cell. Mutually exclusive with
    /// `forge_prs_loading`: cold cache shimmers, warm cache breathes.
    pub forge_prs_stale: ForgePrStaleness,
}

/// How a warm PR cache's identity-only values should render while (and after)
/// a refresh runs. Three states rather than a bool because "the refresh ended"
/// and "the value was verified" are different facts, and a cached PR number
/// that nothing ever verified must not read like a fresh one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ForgePrStaleness {
    /// Nothing cached is awaiting verification: values render plain.
    #[default]
    Fresh,
    /// A warm cache stripped to identity (`ForgePrLookup::identity_only`) with
    /// a refresh in flight — the number is real, its fate is still loading.
    /// Breathes until `ForgePrsRefreshed` concludes.
    Refreshing,
    /// The refresh concluded without superseding the identities (it failed, or
    /// the 20s deadline passed while it was still running). The numbers are
    /// still cached-and-unverified, so they stay muted — but nothing is loading
    /// any more, so they hold still instead of breathing.
    Unrefreshed,
}

pub struct LiveTable {
    pub rows: Vec<WorktreeRow>,
    pub cfg: LiveTableConfig,
    pub pending_resort: bool,
    pub collection_complete: bool,
    /// Set when the user cancels (Ctrl-C). Cells that haven't received their
    /// patch should render a "data didn't load" marker rather than the
    /// loading shimmer. `mark_cancelled` also sets `collection_complete = true`
    /// so `is_cell_loading` naturally returns false post-cancel.
    pub cancelled: bool,
    /// Fields the user abandoned with `Esc` (#826) — table-wide, because a
    /// keypress drops a whole column class, never one row. Unlike `cancelled`
    /// this does **not** set `collection_complete`: the run keeps going and
    /// keeps drawing until the essential cells land. Its effect is confined to
    /// the three cell predicates below — the abandoned column stops
    /// shimmering, settles any cached figure it holds, and falls back to the
    /// "didn't load" marker only where there was no cached value to show.
    pub abandoned: FieldSet,
    /// Whether `Esc` means anything on this screen. False by default, which is
    /// what keeps `daft sync` / `prune` / `clone` safe by construction — they
    /// build their `TuiState` internally, and a default of true would bind a
    /// twitchy key to aborting a mid-flight rebase. The live list opts in.
    pub esc_abandons: bool,
    pub source_log: PatchSourceLog,
    /// Per-row bitmask of "patches received".
    pub received_patches: Vec<FieldSet>,
    /// Per-row bitmask of "cell holds a persisted (stale) value awaiting a
    /// fresh walk". Set at construction from seed values pre-populated by the
    /// caller (today only SIZE, seeded from the on-disk size cache). A stale
    /// bit is superseded implicitly the moment its patch lands —
    /// `is_cell_stale` ANDs against `!received_patches` rather than clearing
    /// the bit — so it needs no mutation on `apply_event`. Kept in lockstep
    /// with `received_patches` across resort/push.
    pub stale_fields: Vec<FieldSet>,
    /// Index of the first row in the unowned section, or `None` if no
    /// partition. Recomputed when `partition_by_owner` is true.
    pub unowned_start_index: Option<usize>,
}

impl LiveTable {
    pub fn new(seed: Vec<WorktreeInfo>, cfg: LiveTableConfig) -> Self {
        // Pre-seed `received_patches` with bits for fields not arriving via
        // the streaming collector. This stops the loading shimmer for cells
        // the collector won't emit a patch for (e.g. `info.owner = None` for
        // the default branch row in `daft prune` / `daft sync`). Synthesized
        // open-PR rows are seed-final by definition — everything they show
        // came from the forge cache and no collector targets them — so every
        // cell is marked received (blank means blank, not loading). A
        // branchless row (sandbox / foreign detached) additionally settles
        // its BRANCH_KEYED cells: the collector masks those out for it, so
        // without this they'd shimmer until the whole collection completes
        // and only then read as blank.
        let received_patches = seed
            .iter()
            .map(|info| Self::seed_received(info, cfg.seeded_fields))
            .collect();
        let rows: Vec<WorktreeRow> = seed.into_iter().map(WorktreeRow::idle).collect();
        // A cell is stale when the caller pre-populated its value (only SIZE
        // today, from the size cache) AND that field is still streamed by the
        // collector — i.e. not in `seeded_fields`, which marks fields the
        // collector won't emit a patch for. Guarding on `seeded_fields`
        // prevents a value that's authoritative-at-seed (e.g. size not
        // requested at all) from rendering as perpetually "refreshing".
        let seeded = cfg.seeded_fields;
        let stale_fields: Vec<FieldSet> = rows
            .iter()
            .map(|r| {
                if r.info.size_bytes.is_some() && !seeded.contains(FieldSet::SIZE) {
                    FieldSet::SIZE
                } else {
                    FieldSet::EMPTY
                }
            })
            .collect();
        let mut t = Self {
            rows,
            cfg,
            pending_resort: true,
            collection_complete: false,
            cancelled: false,
            abandoned: FieldSet::EMPTY,
            esc_abandons: false,
            source_log: PatchSourceLog::default(),
            received_patches,
            stale_fields,
            unowned_start_index: None,
        };
        t.resort_and_repartition();
        t
    }

    pub fn apply_event(&mut self, event: &DagEvent) {
        match event {
            DagEvent::ForgePrsRefreshed(outcome) => {
                // The next frame recomputes column values against the fresh
                // lookup — no per-row patching needed, the PR cell derives
                // from cfg at render time — and the row-set reconcile lands
                // in the same repaint: rows for PRs that closed drop, rows
                // for PRs the seed didn't know insert. A `None` outcome
                // (refresh failed or timed out) settles the loading skeleton
                // but keeps any identity-only seed statusless: the status
                // never loaded, so it must not appear.
                if let Some(refresh) = outcome {
                    self.cfg.forge_prs = Some(refresh.lookup.clone());
                    for name in &refresh.drop_rows {
                        self.remove_row(name);
                    }
                    for info in &refresh.add_rows {
                        if self.find_row_idx(&info.name).is_none() {
                            self.push_row(info.clone());
                        }
                    }
                    self.pending_resort = true;
                }
                self.cfg.forge_prs_loading = false;
                // The breath stops either way, but for different reasons — and
                // only one of them makes the value fresh. Success supersedes
                // the identities with verified data. Failure leaves them
                // cached and unverified, so they keep their muted ink and
                // merely stop moving; clearing to `Fresh` there would promote
                // an unverified number to full brightness, which is the exact
                // lie the stale breath exists to prevent (and what the Size
                // path refuses at collection end — see `is_cell_stale`).
                self.cfg.forge_prs_stale = if outcome.is_some() {
                    ForgePrStaleness::Fresh
                } else {
                    ForgePrStaleness::Unrefreshed
                };
            }
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
                if let Some(spec) = &self.cfg.sort_spec
                    && touched.intersects(spec.required_fields())
                {
                    self.pending_resort = true;
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

    /// Mark the live table as cancelled by user (Ctrl-C). Sets
    /// `collection_complete = true` so the loading shimmer stops and
    /// `pending_resort = true` so the next tick re-runs the sort/partition.
    /// Cells that haven't received their patch will render via
    /// `is_cell_unloaded` rather than the loading shimmer.
    pub fn mark_cancelled(&mut self) {
        self.cancelled = true;
        self.collection_complete = true;
        self.pending_resort = true;
    }

    /// Give up on `fields` without ending the run. Deliberately touches
    /// neither `collection_complete` (the renderer must keep drawing until the
    /// essential cells land) nor `pending_resort` (abandoning changes how a
    /// cell is drawn, never its value, so the ordering still holds).
    pub fn abandon(&mut self, fields: FieldSet) {
        self.abandoned |= fields;
    }

    /// Whether anything has been abandoned yet — the first `Esc` abandons,
    /// a second one exits.
    pub fn has_abandoned(&self) -> bool {
        !self.abandoned.is_empty()
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

    /// Remove a row by name, keeping `received_patches` and `stale_fields`
    /// in lockstep. Used by the forge reconcile to drop PR-sourced rows
    /// whose PR is no longer open.
    fn remove_row(&mut self, branch: &str) {
        if let Some(idx) = self.find_row_idx(branch) {
            self.rows.remove(idx);
            self.received_patches.remove(idx);
            self.stale_fields.remove(idx);
        }
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
            let c = ra
                .info
                .kind
                .section_order()
                .cmp(&rb.info.kind.section_order());
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
        let mut new_stale: Vec<FieldSet> = Vec::with_capacity(self.stale_fields.len());
        for &i in &indexed {
            new_rows.push(std::mem::replace(
                &mut self.rows[i],
                WorktreeRow::placeholder(),
            ));
            new_recv.push(self.received_patches[i]);
            new_stale.push(self.stale_fields[i]);
        }
        self.rows = new_rows;
        self.received_patches = new_recv;
        self.stale_fields = new_stale;

        self.unowned_start_index = if self.cfg.partition_by_owner {
            self.rows.iter().position(|r| r.info.owner.is_none())
        } else {
            None
        };
    }

    /// True when the cell for `field` on `row_idx` should render the
    /// loading glyph. Per-row patch state is only meaningful while
    /// !collection_complete; the repo-level PR first-load skeleton
    /// (`forge_prs_loading`) is out-of-band — the forge refresh outlives
    /// the collectors — and is cleared by its own conclusion event or by
    /// cancel.
    pub fn is_cell_loading(&self, row_idx: usize, field: FieldSet) -> bool {
        // An abandoned field is no longer in flight, whatever the collectors
        // are still doing — the shimmer would promise a value that is not
        // coming. Checked ahead of the out-of-band PR skeleton, which has its
        // own reason to be true and would otherwise outlive the abandon.
        if self.abandoned.intersects(field) {
            return false;
        }
        if field.contains(FieldSet::FORGE_REF) && self.cfg.forge_prs_loading && !self.cancelled {
            return true;
        }
        !self.collection_complete && !self.received_patches[row_idx].contains(field)
    }

    /// True when the cell for `field` on `row_idx` should render the
    /// "data didn't load" marker because the user cancelled before the
    /// patch arrived. Mutually exclusive with `is_cell_loading` after
    /// `mark_cancelled()` runs (which sets `collection_complete = true`).
    ///
    /// An abandoned field reaches the same marker, but only where there is
    /// nothing better to show: the render path checks this branch *after* the
    /// cell's value, so a row holding a cached figure settles that figure
    /// (`is_cell_stale_settled`) and never falls through to here.
    ///
    /// `FORGE_REF` is abandonable but never reaches the marker that way.
    /// Abandoning it means "stop waiting for the network refresh" — the local
    /// `branch.<name>.merge` read that fills the cell rides on the per-target
    /// workers, which `Esc` does not touch, so the value is still coming and
    /// an em-dash would be a lie the next patch immediately contradicts. A PR
    /// cell with nothing to show stays blank, exactly as it does at the end of
    /// an ordinary run.
    pub fn is_cell_unloaded(&self, row_idx: usize, field: FieldSet) -> bool {
        if self.received_patches[row_idx].contains(field) {
            return false;
        }
        self.cancelled || self.settled_fields().intersects(field)
    }

    /// Fields the run has genuinely stopped producing — abandoned, *and* with a
    /// producer that `Esc` actually halts. That is `SIZE` and only `SIZE`: the
    /// size coordinator is the single thread `Esc` stops. `FORGE_REF` earns its
    /// place in `DECORATIVE` for its other two effects (breaking the forge
    /// barrier, settling the stale PR cell) while the local
    /// `branch.<name>.merge` read that fills the cell keeps arriving on the
    /// per-target workers.
    ///
    /// Two callers need the distinction and would drift apart spelling it
    /// themselves: the em-dash marker (which would otherwise lie about a value
    /// still in flight) and the footer's inflight count (which would otherwise
    /// freeze at its pre-`Esc` value, since the rows it counts never receive
    /// the SIZE patch it is waiting for).
    pub fn settled_fields(&self) -> FieldSet {
        self.abandoned & !FieldSet::FORGE_REF
    }

    /// True when the cell for `field` on `row_idx` holds a persisted (stale)
    /// value that a fresh patch has not yet superseded — the render path
    /// breathes it to signal "last known, refreshing". Goes false the
    /// instant the matching patch lands. Deliberately not gated on
    /// `collection_complete`: a value the walk never refreshes stays honestly
    /// muted rather than promoting to "fresh" at collection end (the final
    /// frame settles its breath — see `render::stale_cell`).
    ///
    /// PR cells are stale table-wide rather than per-row: a warm cache is
    /// stripped to identity for every row at once (`forge_prs_stale`), and the
    /// refresh concludes for every row at once. Like `forge_prs_loading` this
    /// is out-of-band, so it deliberately outlives collection completing.
    /// Unlike the shimmer it is *not* cleared by cancel, nor by a refresh that
    /// failed — an unrefreshed value stays honestly stale either way.
    pub fn is_cell_stale(&self, row_idx: usize, field: FieldSet) -> bool {
        if field.contains(FieldSet::FORGE_REF)
            && self.cfg.forge_prs_stale != ForgePrStaleness::Fresh
        {
            return true;
        }
        self.stale_fields[row_idx].contains(field)
            && !self.received_patches[row_idx].contains(field)
    }

    /// True when a stale cell should hold still rather than breathe: its
    /// refresh has definitively concluded without superseding it, so the value
    /// stays muted (still unverified) but no longer signals activity that will
    /// never come.
    ///
    /// Two things can know this. `ForgePrsRefreshed(None)` is the PR column's
    /// explicit "nothing more is coming" verdict. `Esc` is the general one: an
    /// abandoned field's refresh has been called off by the user, which is the
    /// same conclusion arrived at deliberately — so a stale *size*, which has
    /// no per-row signal of its own and otherwise breathes until the final
    /// frame, settles the moment it is abandoned.
    pub fn is_cell_stale_settled(&self, field: FieldSet) -> bool {
        if self.abandoned.intersects(field) {
            return true;
        }
        field.contains(FieldSet::FORGE_REF)
            && self.cfg.forge_prs_stale == ForgePrStaleness::Unrefreshed
    }

    /// Append a new row, keeping `received_patches` in lockstep so
    /// `is_cell_loading` cannot index out of bounds. Initialized to
    /// `FieldSet::EMPTY`: dynamically-discovered branches have no
    /// upfront seed, so every cell starts as "loading" until a patch
    /// arrives. This is a conservative default, not provably-correct
    /// for all callers — cells that no patch ever lands on (e.g.
    /// gone branches surfaced after fetch in prune) will shimmer
    /// indefinitely. Callers that need rows treated as seed-final
    /// should extend this API rather than rely on the default.
    pub fn push_row(&mut self, info: WorktreeInfo) {
        // Synthesized open-PR rows are seed-final wherever they enter (see
        // `new`); other dynamic rows start all-loading as documented above,
        // minus the cells the collector could never patch for them.
        let received = Self::seed_received(&info, FieldSet::EMPTY);
        self.rows.push(WorktreeRow::idle(info));
        self.received_patches.push(received);
        // Dynamically-discovered rows carry no cache seed, so no field is
        // stale — keep the vector length in lockstep with `received_patches`.
        self.stale_fields.push(FieldSet::EMPTY);
    }

    /// The received-at-seed bits for a row: `base` (the fields this view's
    /// collector won't stream at all) plus the bits it will never patch for
    /// this particular row — everything for a seed-final ForgePr row, the
    /// `BRANCH_KEYED` cells for a branchless one (the collector masks them
    /// out per-target; see `list_stream::run_worker`).
    fn seed_received(info: &WorktreeInfo, base: FieldSet) -> FieldSet {
        if info.kind == EntryKind::ForgePr {
            return FieldSet::ALL;
        }
        if info.branchless {
            base | FieldSet::BRANCH_KEYED
        } else {
            base
        }
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
        P::ForgeRef(_) => FieldSet::FORGE_REF,
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
            seeded_fields: FieldSet::EMPTY,
            annotation_slots: Default::default(),
            forge_prs: None,
            forge_prs_loading: false,
            forge_prs_stale: ForgePrStaleness::Fresh,
        }
    }

    fn info(name: &str) -> WorktreeInfo {
        WorktreeInfo::empty(name)
    }

    fn info_with_size(name: &str, bytes: u64) -> WorktreeInfo {
        let mut info = WorktreeInfo::empty(name);
        info.size_bytes = Some(bytes);
        info
    }

    #[test]
    fn collection_done_sets_collection_complete() {
        let mut t = LiveTable::new(vec![info("a")], cfg());
        assert!(!t.collection_complete);
        t.apply_event(&DagEvent::WorktreeInfoCollectionDone);
        assert!(t.collection_complete);
    }

    /// A branchless row (sandbox / foreign detached) settles its
    /// branch-keyed cells at seed: the collector never streams them, so
    /// without the settle they'd shimmer until the whole collection
    /// completes and only then read as blank (#53).
    #[test]
    fn branchless_rows_settle_branch_keyed_cells_at_seed() {
        let mut sandbox = info("main-fork");
        sandbox.is_sandbox = true;
        sandbox.branchless = true;
        let t = LiveTable::new(vec![info("feat/x"), sandbox], cfg());
        let idx =
            |t: &LiveTable, name: &str| t.rows.iter().position(|r| r.info.name == name).unwrap();
        let b = idx(&t, "feat/x");
        let s = idx(&t, "main-fork");

        // Path-derived cells stream for both rows...
        assert!(t.is_cell_loading(b, FieldSet::CHANGES));
        assert!(t.is_cell_loading(s, FieldSet::CHANGES));
        assert!(t.is_cell_loading(s, FieldSet::SIZE));
        // ...branch-keyed cells load only where a branch exists.
        assert!(t.is_cell_loading(b, FieldSet::BASE_AHEAD_BEHIND));
        assert!(!t.is_cell_loading(s, FieldSet::BASE_AHEAD_BEHIND));
        assert!(!t.is_cell_loading(s, FieldSet::OWNER));
        assert!(!t.is_cell_loading(s, FieldSet::FORGE_REF));
    }

    /// Dynamically-pushed branchless rows get the same settle as seeded
    /// ones — `push_row` shares the seeding rule with `new`.
    #[test]
    fn push_row_settles_branch_keyed_cells_for_branchless_rows() {
        let mut t = LiveTable::new(vec![info("feat/x")], cfg());
        let mut sandbox = info("brave-otter");
        sandbox.branchless = true;
        t.push_row(sandbox);
        let s = t
            .rows
            .iter()
            .position(|r| r.info.name == "brave-otter")
            .unwrap();
        assert!(!t.is_cell_loading(s, FieldSet::BASE_AHEAD_BEHIND));
        assert!(t.is_cell_loading(s, FieldSet::CHANGES));
    }

    #[test]
    fn forge_refresh_event_swaps_the_pr_lookup_mid_run() {
        use crate::core::worktree::forge_ref::{
            ForgeBranchRef, ForgePrLookup, ForgeRefKind, PrDecoration, PrStatus,
        };
        use crate::core::worktree::pr_rows::ForgePrRowsRefresh;

        let mut t = LiveTable::new(vec![info("feat/x")], cfg());
        assert!(t.cfg.forge_prs.is_none(), "cold cache: no lookup at start");

        let mut fresh = ForgePrLookup::default();
        let r = ForgeBranchRef::new(ForgeRefKind::GithubPr, 7);
        fresh.by_branch.insert(
            "feat/x".into(),
            PrDecoration {
                r,
                status: Some(PrStatus::Merged),
                url: None,
                author: None,
            },
        );
        t.apply_event(&DagEvent::ForgePrsRefreshed(Some(ForgePrRowsRefresh {
            lookup: fresh.clone(),
            add_rows: vec![],
            drop_rows: vec![],
        })));

        assert_eq!(t.cfg.forge_prs, Some(fresh));
    }

    /// The refresh's row-set reconcile lands in the same repaint as the
    /// fresh statuses: rows for PRs the seed didn't know insert (seed-final,
    /// no shimmer), rows whose PR closed drop — with the bookkeeping vectors
    /// staying in lockstep.
    #[test]
    fn forge_refresh_reconciles_the_pr_row_set() {
        use crate::core::worktree::forge_ref::ForgePrLookup;
        use crate::core::worktree::list::EntryKind;
        use crate::core::worktree::pr_rows::ForgePrRowsRefresh;

        let mut stale_pr = info("alice:patch-1");
        stale_pr.kind = EntryKind::ForgePr;
        let mut t = LiveTable::new(vec![info("feat/x"), stale_pr], cfg());
        assert_eq!(t.rows.len(), 2);

        let mut fresh_row = info("bob:fix-panic");
        fresh_row.kind = EntryKind::ForgePr;
        t.apply_event(&DagEvent::ForgePrsRefreshed(Some(ForgePrRowsRefresh {
            lookup: ForgePrLookup::default(),
            add_rows: vec![fresh_row],
            drop_rows: vec!["alice:patch-1".into()],
        })));
        t.tick();

        let names: Vec<&str> = t.rows.iter().map(|r| r.info.name.as_str()).collect();
        assert_eq!(names, vec!["feat/x", "bob:fix-panic"]);
        assert_eq!(t.received_patches.len(), 2, "vectors stay in lockstep");
        assert_eq!(t.stale_fields.len(), 2);
        let idx = t.rows.iter().position(|r| r.info.name == "bob:fix-panic");
        assert!(
            !t.is_cell_loading(idx.unwrap(), FieldSet::SIZE),
            "an inserted PR row is seed-final — blank, never shimmer"
        );
    }

    /// First load in a repo that never had a snapshot: PR cells skeleton
    /// until the refresh concludes — with data (statuses land) or without
    /// (cells settle empty; a failed refresh must not strand the shimmer).
    #[test]
    fn forge_first_load_skeleton_settles_on_conclusion() {
        let mut t = LiveTable::new(vec![info("feat/x")], cfg());
        t.cfg.forge_prs_loading = true;
        // Out-of-band skeleton: survives per-row patches AND collection
        // completing (the refresh outlives the collectors).
        t.apply_event(&DagEvent::WorktreeInfoCollectionDone);
        assert!(t.is_cell_loading(0, FieldSet::FORGE_REF));
        assert!(
            !t.is_cell_loading(0, FieldSet::SIZE),
            "only the PR cell rides the repo-level flag"
        );

        t.apply_event(&DagEvent::ForgePrsRefreshed(None));
        assert!(
            !t.is_cell_loading(0, FieldSet::FORGE_REF),
            "a concluded-without-data refresh settles the skeleton"
        );
        assert!(t.cfg.forge_prs.is_none(), "no data means no decorations");
    }

    /// Warm cache + refresh in flight: the identity-only PR cells are stale,
    /// not loading — they breathe a real number rather than shimmering an
    /// empty one — until the refresh concludes.
    ///
    /// How it concludes decides what "concluded" means. Fresh data supersedes
    /// the identities, so they go plain. No data leaves them cached and
    /// unverified, so they must stay muted (and merely stop moving) — clearing
    /// to `Fresh` there would promote an unverified number to full brightness.
    #[test]
    fn forge_warm_cache_is_stale_until_the_refresh_concludes() {
        use crate::core::worktree::forge_ref::ForgePrLookup;
        use crate::core::worktree::pr_rows::ForgePrRowsRefresh;

        for outcome_has_data in [false, true] {
            let mut t = LiveTable::new(vec![info("feat/x")], cfg());
            t.cfg.forge_prs_stale = ForgePrStaleness::Refreshing;

            // Out-of-band like the skeleton: survives collection completing.
            t.apply_event(&DagEvent::WorktreeInfoCollectionDone);
            assert!(t.is_cell_stale(0, FieldSet::FORGE_REF));
            assert!(
                !t.is_cell_stale_settled(FieldSet::FORGE_REF),
                "a refresh in flight breathes; it must not hold still"
            );
            assert!(
                !t.is_cell_loading(0, FieldSet::FORGE_REF),
                "a warm cache breathes its value; it must not also shimmer"
            );
            assert!(
                !t.is_cell_stale(0, FieldSet::SIZE),
                "only the PR cell rides the repo-level flag"
            );

            let outcome = outcome_has_data.then(|| ForgePrRowsRefresh {
                lookup: ForgePrLookup::default(),
                add_rows: vec![],
                drop_rows: vec![],
            });
            t.apply_event(&DagEvent::ForgePrsRefreshed(outcome));
            assert!(
                !t.is_cell_loading(0, FieldSet::FORGE_REF),
                "the conclusion settles the skeleton either way"
            );
            assert_eq!(
                t.is_cell_stale(0, FieldSet::FORGE_REF),
                !outcome_has_data,
                "fresh data supersedes the identity; no data leaves it \
                 unverified and therefore still stale"
            );
            assert_eq!(
                t.is_cell_stale_settled(FieldSet::FORGE_REF),
                !outcome_has_data,
                "an unrefreshed identity holds still — muted, but no longer \
                 advertising a refresh that already gave up"
            );
        }
    }

    /// Unlike the shimmer, cancel does NOT clear the stale breath: the cached
    /// identity is still a real value that was never refreshed, so it stays
    /// honestly stale and the final frame settles it (see `render::stale_cell`).
    #[test]
    fn forge_warm_cache_stays_stale_across_cancel() {
        let mut t = LiveTable::new(vec![info("feat/x")], cfg());
        t.cfg.forge_prs_stale = ForgePrStaleness::Refreshing;
        t.mark_cancelled();
        assert!(
            t.is_cell_stale(0, FieldSet::FORGE_REF),
            "an unrefreshed cached value must not promote to fresh on cancel"
        );
    }

    /// Cancel must clear the PR skeleton like any other shimmer — the final
    /// frame renders blanks, not perpetual loading bars.
    #[test]
    fn forge_first_load_skeleton_clears_on_cancel() {
        let mut t = LiveTable::new(vec![info("feat/x")], cfg());
        t.cfg.forge_prs_loading = true;
        assert!(t.is_cell_loading(0, FieldSet::FORGE_REF));
        t.mark_cancelled();
        assert!(!t.is_cell_loading(0, FieldSet::FORGE_REF));
    }

    /// Abandoning stops the shimmer and settles the breath. What each cell
    /// *renders* is asserted at the render layer, where the predicates are
    /// consulted in order and a cell holding a value never reaches
    /// `is_cell_unloaded` — see
    /// `render::tests::abandoned_size_column_settles_the_cached_figure_and_marks_only_the_bare_cell`.
    #[test]
    fn abandon_stops_the_shimmer_and_settles_the_breath() {
        let mut t = LiveTable::new(
            vec![info_with_size("cached", 4096), info("uncached")],
            cfg(),
        );
        let cached = t.rows.iter().position(|r| r.info.name == "cached").unwrap();
        let bare = t
            .rows
            .iter()
            .position(|r| r.info.name == "uncached")
            .unwrap();

        // Before: the seeded row breathes its cached figure, the bare one
        // shimmers.
        assert!(t.is_cell_stale(cached, FieldSet::SIZE));
        assert!(!t.is_cell_stale_settled(FieldSet::SIZE));
        assert!(t.is_cell_loading(bare, FieldSet::SIZE));

        t.abandon(FieldSet::DECORATIVE);

        // Nothing shimmers any more — the walk is not coming back.
        assert!(!t.is_cell_loading(cached, FieldSet::SIZE));
        assert!(!t.is_cell_loading(bare, FieldSet::SIZE));
        // The cached figure stays, and stops moving.
        assert!(t.is_cell_stale(cached, FieldSet::SIZE));
        assert!(t.is_cell_stale_settled(FieldSet::SIZE));
        // A row with no value to show is what the "didn't load" marker is for.
        assert!(t.is_cell_unloaded(bare, FieldSet::SIZE));
    }

    /// Abandoning is not cancelling: the run keeps going so the essential
    /// cells can still land. Coupling these would make `Esc` exit instantly
    /// and silently drop the cells the user is still owed.
    #[test]
    fn abandon_leaves_the_run_live_and_the_essential_cells_alone() {
        let mut t = LiveTable::new(vec![info("a")], cfg());
        t.abandon(FieldSet::DECORATIVE);

        assert!(!t.collection_complete);
        assert!(!t.cancelled);
        assert!(t.has_abandoned());
        // An essential cell is untouched — still in flight, still not a marker.
        assert!(t.is_cell_loading(0, FieldSet::CHANGES));
        assert!(!t.is_cell_unloaded(0, FieldSet::CHANGES));
    }

    /// The PR skeleton is out-of-band (`forge_prs_loading` outlives the
    /// collectors), so it needs the abandon check ahead of it or it keeps
    /// shimmering after the user asked it to stop.
    #[test]
    fn abandon_clears_the_out_of_band_forge_skeleton() {
        let mut t = LiveTable::new(vec![info("feat/x")], cfg());
        t.cfg.forge_prs_loading = true;
        assert!(t.is_cell_loading(0, FieldSet::FORGE_REF));
        t.abandon(FieldSet::DECORATIVE);
        assert!(!t.is_cell_loading(0, FieldSet::FORGE_REF));
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

    #[test]
    fn mark_cancelled_sets_cancelled_and_collection_complete() {
        let mut t = LiveTable::new(vec![info("a")], cfg());
        assert!(!t.cancelled);
        assert!(!t.collection_complete);
        t.mark_cancelled();
        assert!(t.cancelled);
        assert!(t.collection_complete);
        assert!(t.pending_resort);
    }

    #[test]
    fn is_cell_unloaded_false_before_cancel() {
        let t = LiveTable::new(vec![info("a")], cfg());
        assert!(!t.is_cell_unloaded(0, FieldSet::SIZE));
    }

    #[test]
    fn is_cell_unloaded_true_when_cancelled_and_not_received() {
        let mut t = LiveTable::new(vec![info("a")], cfg());
        t.mark_cancelled();
        assert!(t.is_cell_unloaded(0, FieldSet::SIZE));
    }

    #[test]
    fn is_cell_unloaded_false_when_received_even_after_cancel() {
        let mut t = LiveTable::new(vec![info("a")], cfg());
        t.apply_event(&DagEvent::WorktreeInfoUpdated {
            branch_name: "a".into(),
            patch: WorktreeInfoPatch::Size(Some(123)),
            source: PatchSource::Collector,
        });
        t.mark_cancelled();
        assert!(!t.is_cell_unloaded(0, FieldSet::SIZE));
    }

    #[test]
    fn is_cell_loading_returns_false_after_mark_cancelled() {
        // Regression guard: mark_cancelled sets collection_complete = true,
        // which makes is_cell_loading naturally return false. We rely on this
        // so the render path doesn't need a second "and not cancelled" check
        // in the loading branch.
        let mut t = LiveTable::new(vec![info("a")], cfg());
        assert!(t.is_cell_loading(0, FieldSet::SIZE));
        t.mark_cancelled();
        assert!(!t.is_cell_loading(0, FieldSet::SIZE));
    }

    #[test]
    fn seeded_fields_marks_cells_received_at_construction() {
        // Regression guard for the prune/sync default-branch owner shimmer:
        // when the synchronous seed authoritatively populates a field
        // (including the empty/None case for the default branch's owner),
        // the cell must NOT render the loading shimmer just because the
        // streaming collector won't emit a patch for it.
        let mut cfg = cfg();
        cfg.seeded_fields = FieldSet::OWNER;
        let t = LiveTable::new(vec![info("main")], cfg);
        assert!(!t.is_cell_loading(0, FieldSet::OWNER));
    }

    #[test]
    fn seeded_size_value_marks_cell_stale_until_patch_supersedes() {
        // A row seeded with a cached size (SIZE not in seeded_fields, so it's
        // still streamed) renders stale until the fresh walk patch lands.
        let mut t = LiveTable::new(vec![info_with_size("a", 4096)], cfg());
        assert!(
            t.is_cell_stale(0, FieldSet::SIZE),
            "seeded cached size should read as stale before refresh"
        );
        // `is_cell_loading` is still technically true (no SIZE patch received
        // yet), but that is moot for rendering: the Size arm short-circuits on
        // a non-empty value and consults `is_cell_stale` before it would ever
        // reach the loading branch. So a stale cell shows its dimmed value,
        // never the shimmer.

        // Fresh walk result supersedes it.
        t.apply_event(&DagEvent::WorktreeInfoUpdated {
            branch_name: "a".into(),
            patch: WorktreeInfoPatch::Size(Some(8192)),
            source: PatchSource::Collector,
        });
        assert!(
            !t.is_cell_stale(0, FieldSet::SIZE),
            "landed patch must clear staleness"
        );
        assert_eq!(t.rows[0].info.size_bytes, Some(8192));
    }

    #[test]
    fn no_seed_value_is_not_stale() {
        // The default path (no cache hit) leaves size_bytes None → not stale,
        // just loading as before.
        let t = LiveTable::new(vec![info("a")], cfg());
        assert!(!t.is_cell_stale(0, FieldSet::SIZE));
        assert!(t.is_cell_loading(0, FieldSet::SIZE));
    }

    #[test]
    fn seeded_size_not_stale_when_size_is_authoritative() {
        // When SIZE is in seeded_fields (the collector won't stream it), a
        // pre-populated size is final, not "refreshing" — so never dim.
        let mut cfg = cfg();
        cfg.seeded_fields = FieldSet::SIZE;
        let t = LiveTable::new(vec![info_with_size("a", 4096)], cfg);
        assert!(!t.is_cell_stale(0, FieldSet::SIZE));
    }

    #[test]
    fn stale_fields_track_rows_across_resort() {
        // Two seeded-stale rows sorted by name: the stale bits must follow
        // their rows through the resort permutation, not stay by index.
        let mut c = cfg();
        c.pin_default_branch = false;
        let t = LiveTable::new(
            vec![info_with_size("zebra", 1), info_with_size("alpha", 2)],
            c,
        );
        // Sorted ascending: alpha (2) then zebra (1). Both remain stale, and
        // the values rode along with their rows.
        assert_eq!(t.rows[0].info.name, "alpha");
        assert_eq!(t.rows[0].info.size_bytes, Some(2));
        assert!(t.is_cell_stale(0, FieldSet::SIZE));
        assert!(t.is_cell_stale(1, FieldSet::SIZE));
    }

    #[test]
    fn seeded_fields_empty_preserves_existing_loading_behavior() {
        // Paired with `seeded_fields_marks_cells_received_at_construction`:
        // the default `seeded_fields = EMPTY` keeps the existing semantics
        // where every cell starts in the loading state until a patch lands.
        let cfg = cfg(); // seeded_fields: FieldSet::EMPTY
        let t = LiveTable::new(vec![info("main")], cfg);
        assert!(t.is_cell_loading(0, FieldSet::OWNER));
    }
}
