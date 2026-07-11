//! Live-region driver for the rail timeline.
//!
//! One `MultiProgress` per command. The composition invariant that makes the
//! hook-section embed work: **completed content is persisted eagerly**
//! (`mp.remove(bar)` + `mp.println(line)` — the atomic visual swap), so at any
//! moment the live bars are exactly `{active?, pending…, bottom spacer,
//! footer placeholder}`. Any `mp.println` — a warning, or the embedded hook
//! renderer's anchor/rows/log lines — therefore lands *between* the persisted
//! history above and the remaining plan below.
//!
//! indicatif discipline (in-tree lessons, see `hook_progress/interactive.rs`):
//! bars leave via `mp.remove`, never `finish_and_clear` (zombie-line
//! accounting); templates are never empty; rows are single-line (labels and
//! annotations are pre-composed, annotations truncate via `{wide_msg}`).

use super::render::{self, RowFace};
use crate::core::stage::{PlanCommit, Row, StepKey, StepSpec};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::time::{Duration, Instant};

/// House braille spinner frames (same set as the hook job spinners).
pub(super) const TICK_CHARS: &str =
    "\u{2807}\u{2819}\u{2839}\u{2838}\u{283c}\u{2834}\u{2826}\u{2827}\u{2807}\u{280f}";

/// What an embedded hook renderer needs to draw inside the region.
pub struct HookEmbed {
    pub mp: MultiProgress,
    /// Insertion anchor: hook job bars go `insert_before(anchor)`. Always a
    /// live rail bar (the section's own planned below-`│` when the plan laid
    /// one down, else the first pending row, else the bottom spacer), which
    /// stays alive for the whole splice — `insert_before` panics on a
    /// removed anchor, so liveness is a hard invariant.
    pub anchor: ProgressBar,
    /// Whether the region renders ANSI (NO_COLOR tracks the rail, not the
    /// renderer's own stderr probe).
    pub use_color: bool,
    /// The consumed row's plan label ("post-create hooks") when the step is
    /// a hook phase — the succinct renderer's section anchor. `None` for
    /// gate embeds (pre-push under an active Push/DeleteRemote row), whose
    /// section label derives from the phase name instead.
    pub section_label: Option<String>,
}

enum Slot {
    Group {
        label: String,
        bar: Option<ProgressBar>,
        /// The `│` spacer line above the anchor (the section dialect:
        /// `│` then `├─ <label>`). `None` for the plan's first slot — the
        /// header's own top spacer already provides that gap — and after
        /// the group persists or drops.
        spacer: Option<ProgressBar>,
    },
    /// Invisible group-span terminator — never renders, owns no bar.
    EndGroup,
    Note {
        text: String,
        bar: Option<ProgressBar>,
        /// Inside a group span: renders tucked into the rail gutter
        /// (`│  ○ …`) so the anchor's `├─` visibly carries it.
        in_group: bool,
    },
    Step {
        spec: StepSpec,
        bar: Option<ProgressBar>,
        state: StepState,
        /// Inside a group span: every face of this row (pending, active,
        /// final) renders in the rail gutter.
        in_group: bool,
        /// Pre-laid section spacers for top-level hook-phase rows: the `│`
        /// above/below where the section will open, materialized with the
        /// plan so the committed plan carries the receipt's rail rhythm and
        /// opening the section never shifts the rows below it (#651 field
        /// test: remove's pending half read crammed while the executed half
        /// had air). `None` where a neighbor already provides the gap, and
        /// after consumption.
        spacer_above: Option<ProgressBar>,
        spacer_below: Option<ProgressBar>,
    },
}

enum StepState {
    Pending,
    Active {
        started: Instant,
    },
    /// Persisted, silently removed, or replaced by an embedded hook block.
    Resolved,
}

pub(super) struct TimelineCore {
    mp: MultiProgress,
    use_color: bool,
    verbose: bool,
    label_width: usize,
    slots: Vec<Slot>,
    /// Dim `│` above the footer; lives until teardown. Doubles as the
    /// hook-embed anchor when no pending row remains.
    bottom_spacer: ProgressBar,
    /// `└  …` placeholder; lives until teardown.
    footer: ProgressBar,
    /// Dim free-text sub-line under the active step (`-v` only).
    detail_bar: Option<ProgressBar>,
    /// Suppresses the TTY's `^C` echo for the region's lifetime — the echo
    /// wraps the cursor and desyncs indicatif's line accounting, stranding
    /// a stale bar line on interrupt (see `output::term_guard`).
    _echo_guard: crate::output::term_guard::EchoCtlGuard,
    /// Whether the most recently persisted line is a rail spacer (`│`) —
    /// used to avoid doubling spacers around embedded hook blocks.
    last_persisted_was_spacer: bool,
    /// A hook block is currently rendering in place of one of our rows.
    hook_block_open: bool,
    /// The open section's planned below-`│`, taken from its slot at
    /// `begin_hook_embed` so the close path persists it exactly once (and
    /// job bars can anchor above it). `None` for gate embeds.
    open_hook_below: Option<ProgressBar>,
}

impl TimelineCore {
    /// Materialize the region: header + top spacer persist immediately, every
    /// plan row becomes a live bar, then the bottom spacer and the footer
    /// placeholder. The caller provides the `MultiProgress` (production:
    /// `MultiProgress::new()`; tests: an `InMemoryTerm` draw target so the
    /// persisted line sequence is assertable).
    pub(super) fn new(
        mp: MultiProgress,
        header: String,
        plan: PlanCommit,
        verbose: bool,
        use_color: bool,
    ) -> Self {
        let label_width = plan
            .steps()
            .map(|s| display_label(s, StepPhase::Pending).chars().count())
            .max()
            .unwrap_or(0);

        mp.println(render::header(
            &header,
            plan.header_annotation.as_deref(),
            use_color,
        ))
        .ok();
        mp.println(render::spacer(use_color)).ok();

        let static_style = line_style();
        let mut last_persisted_was_spacer = true;
        let mut slots: Vec<Slot> = Vec::with_capacity(plan.rows.len());
        // Whether the row being materialized sits inside an open group span
        // (`Group` opens, `EndGroup` closes) — such rows render in the
        // gutter, tucked under their anchor.
        let mut in_group = false;
        let mut rows = plan.rows.into_iter().peekable();
        while let Some(row) = rows.next() {
            let slot = match row {
                Row::Group { label } => {
                    in_group = true;
                    let spacer = (!slots.is_empty())
                        .then(|| add_line_bar(&mp, &static_style, render::spacer(use_color)));
                    let bar =
                        add_line_bar(&mp, &static_style, render::group(&label, None, use_color));
                    Slot::Group {
                        label,
                        bar: Some(bar),
                        spacer,
                    }
                }
                Row::EndGroup => {
                    in_group = false;
                    Slot::EndGroup
                }
                Row::Note { text } => {
                    let line = in_span(render::note(&text, use_color), in_group, use_color);
                    let bar = add_line_bar(&mp, &static_style, line);
                    Slot::Note {
                        text,
                        bar: Some(bar),
                        in_group,
                    }
                }
                Row::Step(spec) => {
                    let inks = super::plan::subject_inks_for(spec.key.id);
                    if let Some(elapsed) = spec.pre_completed {
                        // Completed before the region existed (clone's bare
                        // phase) — persist directly, no bar.
                        let line = render::final_row(
                            &RowFace::Done {
                                duration: Some(elapsed),
                            },
                            &display_label(&spec, StepPhase::Done),
                            spec.annotation.as_deref(),
                            label_width,
                            inks,
                            use_color,
                        );
                        mp.println(in_span(line, in_group, use_color)).ok();
                        last_persisted_was_spacer = false;
                        Slot::Step {
                            spec,
                            bar: None,
                            state: StepState::Resolved,
                            in_group,
                            spacer_above: None,
                            spacer_below: None,
                        }
                    } else {
                        // A top-level hook phase opens as a `├─` section when
                        // it runs; its `│` spacers are part of the plan's
                        // shape, so they are laid down now. Skipped where a
                        // neighbor already provides the gap: the header or a
                        // preceding section above, the bottom spacer or a
                        // group's own spacer below.
                        let section = !in_group && spec.key.id.is_hook_phase();
                        let spacer_above = (section
                            && !slots.is_empty()
                            && !matches!(
                                slots.last(),
                                Some(Slot::Step { spec: s, in_group: false, .. })
                                    if s.key.id.is_hook_phase()
                            ))
                        .then(|| add_line_bar(&mp, &static_style, render::spacer(use_color)));
                        let line = render::pending_row(
                            &display_label(&spec, StepPhase::Pending),
                            spec.annotation.as_deref(),
                            label_width,
                            inks,
                            use_color,
                        );
                        let bar =
                            add_line_bar(&mp, &static_style, in_span(line, in_group, use_color));
                        let spacer_below = (section
                            && matches!(rows.peek(), Some(Row::Note { .. } | Row::Step(_))))
                        .then(|| add_line_bar(&mp, &static_style, render::spacer(use_color)));
                        Slot::Step {
                            spec,
                            bar: Some(bar),
                            state: StepState::Pending,
                            in_group,
                            spacer_above,
                            spacer_below,
                        }
                    }
                }
            };
            slots.push(slot);
        }

        let bottom_spacer = add_line_bar(&mp, &static_style, render::spacer(use_color));
        let footer = add_line_bar(&mp, &static_style, render::footer("\u{2026}", use_color));

        // Ctrl-C while the region is live: collapse it once (erase the live
        // bars; persisted history stays) and exit — printing nothing more,
        // per the stranded-frame lesson from the test runner's cancel path.
        // `suspend` is the atomic variant of clear-then-exit: it erases the
        // region and holds the draw lock while the process dies, so an 80ms
        // steady tick can never repaint a stranded frame in between. The
        // saved termios is restored by hand — process::exit skips drops.
        // Cleared again at teardown.
        let echo_guard = crate::output::term_guard::EchoCtlGuard::new();
        let saved_termios = echo_guard.saved();
        let mp_for_interrupt = mp.clone();
        crate::interrupt::set_behavior(move || {
            mp_for_interrupt.suspend(|| {
                crate::output::term_guard::restore_termios(&saved_termios);
                std::process::exit(130);
            });
        });

        Self {
            mp,
            use_color,
            verbose,
            label_width,
            slots,
            bottom_spacer,
            footer,
            detail_bar: None,
            _echo_guard: echo_guard,
            last_persisted_was_spacer,
            hook_block_open: false,
            open_hook_below: None,
        }
    }

    /// Print a permanent line above the live bars (warnings, errors).
    pub(super) fn println_above(&mut self, line: &str) {
        self.mp.println(line).ok();
        self.last_persisted_was_spacer = false;
    }

    /// Clear the region, run `f` (e.g. a stdout write that must not land
    /// mid-region), redraw.
    pub(super) fn suspend<R>(&self, f: impl FnOnce() -> R) -> R {
        self.mp.suspend(f)
    }

    /// If an embedded hook block is still open, close it with a rail spacer
    /// so whatever persists next visually reconnects to the spine.
    fn reconnect_after_block(&mut self) {
        if self.hook_block_open {
            self.hook_block_open = false;
            // A section's planned below-`│` hands its line to the reconnect
            // (net zero); gate embeds, which planned none, insert one.
            if let Some(bar) = self.open_hook_below.take() {
                self.mp.remove(&bar);
            }
            self.mp.println(render::spacer(self.use_color)).ok();
            self.last_persisted_was_spacer = true;
        }
    }

    /// The step began: swap its bar to the active spinner style. Groups and
    /// notes above it stay live (their bars already render in plan order);
    /// they persist when visible content below them prints — see
    /// [`Self::flush_above`].
    pub(super) fn activate(&mut self, key: &StepKey) {
        let Some(idx) = self.step_index(key) else {
            return;
        };
        self.reconnect_after_block();
        let use_color = self.use_color;
        let label_width = self.label_width;
        if let Slot::Step {
            spec,
            bar,
            state,
            in_group,
            ..
        } = &mut self.slots[idx]
            && let Some(bar) = bar.as_ref()
        {
            let msg = render::active_message(
                &display_label(spec, StepPhase::Active),
                spec.annotation.as_deref(),
                label_width,
                super::plan::subject_inks_for(spec.key.id),
                use_color,
            );
            bar.set_style(active_style(use_color, *in_group));
            bar.set_message(msg);
            bar.enable_steady_tick(Duration::from_millis(80));
            bar.tick();
            *state = StepState::Active {
                started: Instant::now(),
            };
        }
    }

    /// The step resolved: persist its final row (or nothing for a silent
    /// resolution) and clear any detail sub-line.
    ///
    /// A bar-less ACTIVE step still persists its final row: that is the
    /// gate-hook case, where `begin_hook_embed` consumed the active row's
    /// bar so the hook block (e.g. pre-push) could render in its place —
    /// the outcome line then lands below the block.
    pub(super) fn resolve(&mut self, key: &StepKey, resolution: Resolution) {
        let Some(idx) = self.step_index(key) else {
            return;
        };
        self.reconnect_after_block();
        let silent = matches!(resolution, Resolution::Silent);
        if !silent {
            // About to print a visible row: everything above it must be in
            // scrollback first. A silent resolution prints nothing, so it
            // must NOT flush — that is what lets a group whose rows all
            // vanish keep its anchor unprinted.
            self.flush_above(idx);
        }
        self.clear_detail();
        let use_color = self.use_color;
        let label_width = self.label_width;
        let Slot::Step {
            spec,
            bar,
            state,
            in_group,
            spacer_above,
            spacer_below,
        } = &mut self.slots[idx]
        else {
            return;
        };
        let in_group = *in_group;
        // A section-to-be resolving as a plain row (attention skip) keeps
        // its planned `│` frame — persisted around the row below; a silent
        // resolution takes the spacers with it.
        let above = spacer_above.take();
        let below = spacer_below.take();
        if let Some(b) = &above {
            self.mp.remove(b);
        }
        if let Some(b) = &below {
            self.mp.remove(b);
        }
        let started = match state {
            StepState::Active { started } => Some(*started),
            _ => None,
        };
        match bar.take() {
            Some(taken) => {
                taken.disable_steady_tick();
                self.mp.remove(&taken);
            }
            None => {
                if !matches!(state, StepState::Active { .. }) {
                    // Already fully resolved (e.g. replaced by a hook block,
                    // or a duplicate event) — idempotent no-op.
                    *state = StepState::Resolved;
                    return;
                }
            }
        }
        match resolution {
            Resolution::Silent => {}
            Resolution::Final { face, annotation } => {
                if let Some(a) = annotation {
                    spec.annotation = Some(a);
                }
                let face = match face {
                    FinalFace::Done => RowFace::Done {
                        duration: started.map(|s| s.elapsed()),
                    },
                    FinalFace::Failed => RowFace::Failed,
                    FinalFace::SkippedExpected => RowFace::SkippedExpected,
                    FinalFace::SkippedAttention => RowFace::SkippedAttention,
                };
                let phase = match face {
                    // The fact never happened — the label stays imperative
                    // (`↓ Fetch remote  failed — …`, never `↓ Fetched …`).
                    RowFace::Failed | RowFace::SkippedAttention => StepPhase::Pending,
                    RowFace::SkippedExpected => StepPhase::Skipped,
                    _ => StepPhase::Done,
                };
                let line = render::final_row(
                    &face,
                    &display_label(spec, phase),
                    spec.annotation.as_deref(),
                    label_width,
                    super::plan::subject_inks_for(spec.key.id),
                    use_color,
                );
                if above.is_some() && !self.last_persisted_was_spacer {
                    self.mp.println(render::spacer(use_color)).ok();
                }
                self.mp.println(in_span(line, in_group, use_color)).ok();
                self.last_persisted_was_spacer = false;
                if below.is_some() {
                    self.mp.println(render::spacer(use_color)).ok();
                    self.last_persisted_was_spacer = true;
                }
            }
        }
        *state = StepState::Resolved;
        if silent {
            // If this was the last unsettled row of a group whose rows all
            // vanished, the anchor would sit over nothing for the rest of
            // the run — drop it now rather than at teardown.
            self.drop_group_if_span_settled(idx);
        }
    }

    /// Patch a pending/active row's annotation in place.
    pub(super) fn set_annotation(&mut self, key: &StepKey, annotation: String) {
        let Some(idx) = self.step_index(key) else {
            return;
        };
        let use_color = self.use_color;
        let label_width = self.label_width;
        if let Slot::Step {
            spec,
            bar,
            state,
            in_group,
            ..
        } = &mut self.slots[idx]
        {
            spec.annotation = Some(annotation);
            let inks = super::plan::subject_inks_for(spec.key.id);
            if let Some(bar) = bar.as_ref() {
                match state {
                    StepState::Pending => bar.set_message(in_span(
                        render::pending_row(
                            &display_label(spec, StepPhase::Pending),
                            spec.annotation.as_deref(),
                            label_width,
                            inks,
                            use_color,
                        ),
                        *in_group,
                        use_color,
                    )),
                    StepState::Active { .. } => bar.set_message(render::active_message(
                        &display_label(spec, StepPhase::Active),
                        spec.annotation.as_deref(),
                        label_width,
                        inks,
                        use_color,
                    )),
                    StepState::Resolved => {}
                }
            }
        }
    }

    /// `-v` free-text detail under the active step (dim, transient).
    pub(super) fn detail(&mut self, text: &str) {
        if !self.verbose {
            return;
        }
        let Some(active_bar) = self.active_bar() else {
            return;
        };
        let line = if self.use_color {
            format!(
                "{}   {text}{}",
                crate::output::palette::DARK_GREY,
                crate::styles::RESET
            )
        } else {
            format!("   {text}")
        };
        match &self.detail_bar {
            Some(bar) => bar.set_message(line),
            None => {
                let bar = self
                    .mp
                    .insert_after(&active_bar, ProgressBar::new_spinner());
                bar.set_style(line_style());
                bar.set_message(line);
                self.detail_bar = Some(bar);
            }
        }
    }

    /// The hook step is expanding into its block: remove its row (the block
    /// replaces it) and hand out the shared region + a live anchor. A rail
    /// spacer separates the block from the rows above it (unless one is
    /// already the last persisted line).
    pub(super) fn begin_hook_embed(&mut self, key: &StepKey) -> Option<HookEmbed> {
        let idx = self.step_index(key)?;
        self.reconnect_after_block();
        // The block is visible content: flush notes and this span's anchor.
        self.flush_above(idx);
        self.clear_detail();
        let mut section_label = None;
        if let Slot::Step {
            spec,
            bar,
            state,
            spacer_above,
            spacer_below,
            ..
        } = &mut self.slots[idx]
        {
            if let Some(taken) = bar.take() {
                taken.disable_steady_tick();
                self.mp.remove(&taken);
            }
            // The planned section spacers hand over to the block: the `│`
            // above persists via the print below (net zero — its bar leaves
            // here), the one below is stashed for the close path.
            if let Some(sp) = spacer_above.take() {
                self.mp.remove(&sp);
            }
            self.open_hook_below = spacer_below.take();
            // The consumed row's own label names the section for the
            // succinct renderer. Gate embeds (pre-push mid-Push) keep their
            // step label for the outcome row below — no section label.
            if spec.key.id.is_hook_phase() {
                section_label = Some(display_label(spec, StepPhase::Pending));
            }
            // A PENDING hook row is replaced by the block outright. An
            // ACTIVE row (a gate hook rendering mid-step, e.g. pre-push
            // during Push) stays Active so `resolve` prints its outcome row
            // below the block.
            if matches!(state, StepState::Pending) {
                *state = StepState::Resolved;
            }
        }
        if !self.last_persisted_was_spacer {
            self.mp.println(render::spacer(self.use_color)).ok();
            self.last_persisted_was_spacer = true;
        }
        self.hook_block_open = true;
        // Job bars land above the section's own below-`│` when the plan
        // laid one down; gate embeds fall back to the next live rail bar.
        let anchor = self
            .open_hook_below
            .clone()
            .unwrap_or_else(|| self.first_live_bar_after(idx));
        Some(HookEmbed {
            mp: self.mp.clone(),
            anchor,
            use_color: self.use_color,
            section_label,
        })
    }

    /// The hook block finished rendering. When plan rows remain below, a
    /// rail spacer reconnects them to the spine; when the block was the last
    /// step, `finish` provides the closing spacer instead.
    pub(super) fn close_hook_embed(&mut self) {
        if !self.hook_block_open {
            return;
        }
        self.hook_block_open = false;
        // The section's planned below-`│` persists as the reconnect (net
        // zero — the plan already showed it, and its presence encodes that
        // visible content follows, notes included).
        if let Some(bar) = self.open_hook_below.take() {
            self.mp.remove(&bar);
            self.mp.println(render::spacer(self.use_color)).ok();
            self.last_persisted_was_spacer = true;
            return;
        }
        // No planned spacer (a gate embed, or a section whose gap the next
        // group's own spacer provides): insert one only while plan rows
        // remain below.
        let steps_remain = self
            .slots
            .iter()
            .any(|s| matches!(s, Slot::Step { bar: Some(_), .. }));
        if steps_remain {
            self.mp.println(render::spacer(self.use_color)).ok();
            self.last_persisted_was_spacer = true;
        }
    }

    /// Tear the region down. Remaining unresolved steps persist as `face`
    /// (dim not-reached on failure paths, silent on clean finishes where
    /// everything already resolved); remaining notes persist as-is; a group
    /// anchor that never printed persists only if this teardown prints
    /// content into its span (an all-silent section leaves no trace); then
    /// the closing spacer + footer.
    pub(super) fn finish(mut self, footer_text: &str, unresolved: UnresolvedPolicy) {
        // The closing spacer below doubles as the block reconnect.
        self.hook_block_open = false;
        if let Some(bar) = self.open_hook_below.take() {
            self.mp.remove(&bar);
        }
        self.clear_detail();
        let use_color = self.use_color;
        let label_width = self.label_width;
        let mut pending_group: Option<usize> = None;
        for i in 0..self.slots.len() {
            match &self.slots[i] {
                Slot::Group { bar: Some(_), .. } => {
                    if let Some(old) = pending_group.replace(i) {
                        self.drop_group_bar_at(old); // span printed nothing
                    }
                }
                Slot::Group { bar: None, .. } | Slot::EndGroup => {
                    if let Some(old) = pending_group.take() {
                        self.drop_group_bar_at(old); // span printed nothing
                    }
                }
                Slot::Note { bar: Some(_), .. } => {
                    if let Some(g) = pending_group.take() {
                        self.print_group_at(g);
                    }
                    self.print_note_at(i);
                }
                Slot::Note { bar: None, .. } => {}
                Slot::Step { bar: Some(_), .. } => {
                    if matches!(unresolved, UnresolvedPolicy::NotReached)
                        && let Some(g) = pending_group.take()
                    {
                        self.print_group_at(g);
                    }
                    let Slot::Step {
                        spec,
                        bar,
                        state,
                        in_group,
                        spacer_above,
                        spacer_below,
                    } = &mut self.slots[i]
                    else {
                        unreachable!("matched Step above");
                    };
                    let taken = bar.take().expect("matched Some above");
                    taken.disable_steady_tick();
                    self.mp.remove(&taken);
                    // A never-reached section-to-be keeps its planned `│`
                    // frame in the receipt; a dropped one takes it along.
                    let above = spacer_above.take();
                    let below = spacer_below.take();
                    if let Some(b) = &above {
                        self.mp.remove(b);
                    }
                    if let Some(b) = &below {
                        self.mp.remove(b);
                    }
                    match unresolved {
                        UnresolvedPolicy::NotReached => {
                            let line = render::final_row(
                                &RowFace::NotReached,
                                &display_label(spec, StepPhase::Pending),
                                None,
                                label_width,
                                render::PLAIN_INKS,
                                use_color,
                            );
                            if above.is_some() && !self.last_persisted_was_spacer {
                                self.mp.println(render::spacer(use_color)).ok();
                            }
                            self.mp.println(in_span(line, *in_group, use_color)).ok();
                            self.last_persisted_was_spacer = false;
                            if below.is_some() {
                                self.mp.println(render::spacer(use_color)).ok();
                                self.last_persisted_was_spacer = true;
                            }
                        }
                        UnresolvedPolicy::Drop => {}
                    }
                    *state = StepState::Resolved;
                }
                Slot::Step { bar: None, .. } => {
                    // Insurance for the zero-live-bars teardown invariant:
                    // a resolved row's spacers are consumed with it, but a
                    // stray one must never outlive the region.
                    if let Slot::Step {
                        spacer_above,
                        spacer_below,
                        ..
                    } = &mut self.slots[i]
                    {
                        for b in spacer_above.take().into_iter().chain(spacer_below.take()) {
                            self.mp.remove(&b);
                        }
                    }
                }
            }
        }
        if let Some(g) = pending_group {
            self.drop_group_bar_at(g); // span printed nothing
        }
        self.mp.remove(&self.bottom_spacer);
        self.mp.remove(&self.footer);
        self.mp.println(render::spacer(use_color)).ok();
        self.mp.println(render::footer(footer_text, use_color)).ok();
        // The region is gone; Ctrl-C reverts to the default exit.
        crate::interrupt::clear_behavior();
        // `self.mp` drops here with zero live bars — nothing to strand.
    }

    /// Resolve a stage id (+ candidate scope) to the plan's actual key:
    /// prefer the scoped row (multi-branch plans), fall back to the unscoped
    /// one (single-target plans, where the hook context still carries a
    /// branch name that the plan never used as a scope).
    pub(super) fn resolve_key(
        &self,
        id: crate::core::stage::StageId,
        scope: Option<&str>,
    ) -> Option<StepKey> {
        if let Some(scope) = scope {
            let scoped = StepKey::scoped(id, scope);
            if self.step_index(&scoped).is_some() {
                return Some(scoped);
            }
        }
        let unscoped = StepKey::new(id);
        self.step_index(&unscoped).is_some().then_some(unscoped)
    }

    // ── internals ────────────────────────────────────────────────────────

    fn step_index(&self, key: &StepKey) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| matches!(s, Slot::Step { spec, .. } if spec.key == *key))
    }

    /// Persist everything above `idx` that must land in scrollback before
    /// the visible content about to print at `idx`: every unprinted note,
    /// and — lazily — group anchors. An anchor prints only once content in
    /// its span (the rows between it and the next group) prints, so a group
    /// whose rows all resolve silently never shows its anchor (#651 shared
    /// files). A group superseded before printing stays live and is dropped
    /// when its span settles or at teardown.
    fn flush_above(&mut self, idx: usize) {
        let mut pending_group: Option<usize> = None;
        for i in 0..idx {
            match &self.slots[i] {
                Slot::Group { bar: Some(_), .. } => pending_group = Some(i),
                // The span closed without showing content: the row at `idx`
                // is ungrouped, so this anchor must not print for it.
                Slot::EndGroup => pending_group = None,
                Slot::Note { bar: Some(_), .. } => {
                    if let Some(g) = pending_group.take() {
                        self.print_group_at(g);
                    }
                    self.print_note_at(i);
                }
                _ => {}
            }
        }
        // The nearest group above `idx` anchors the content about to print.
        if let Some(g) = pending_group {
            self.print_group_at(g);
        }
    }

    /// Persist the group anchor at slot `i` (no-op if already gone): a `│`
    /// spacer first — skipped when the previously persisted line is already
    /// one (e.g. a hook block's reconnect spacer) — then the `├─` anchor.
    fn print_group_at(&mut self, i: usize) {
        let use_color = self.use_color;
        if let Slot::Group { label, bar, spacer } = &mut self.slots[i]
            && let Some(taken) = bar.take()
        {
            if let Some(sp) = spacer.take() {
                self.mp.remove(&sp);
            }
            self.mp.remove(&taken);
            if !self.last_persisted_was_spacer {
                self.mp.println(render::spacer(use_color)).ok();
            }
            self.mp.println(render::group(label, None, use_color)).ok();
            self.last_persisted_was_spacer = false;
        }
    }

    /// Persist the note at slot `i` (no-op if already gone).
    fn print_note_at(&mut self, i: usize) {
        let use_color = self.use_color;
        if let Slot::Note {
            text,
            bar,
            in_group,
        } = &mut self.slots[i]
            && let Some(taken) = bar.take()
        {
            let line = in_span(render::note(text, use_color), *in_group, use_color);
            self.mp.remove(&taken);
            self.mp.println(line).ok();
            self.last_persisted_was_spacer = false;
        }
    }

    /// Remove the group anchor (and its spacer) at slot `i` without
    /// printing either.
    fn drop_group_bar_at(&mut self, i: usize) {
        if let Slot::Group { bar, spacer, .. } = &mut self.slots[i] {
            if let Some(taken) = spacer.take() {
                self.mp.remove(&taken);
            }
            if let Some(taken) = bar.take() {
                self.mp.remove(&taken);
            }
        }
    }

    /// After a silent resolution at `idx`: if the group span containing
    /// `idx` is now fully settled without ever printing content, drop its
    /// unprinted anchor so it doesn't hang over nothing for the rest of the
    /// run.
    fn drop_group_if_span_settled(&mut self, idx: usize) {
        // Walk up to the group owning `idx`'s span; an EndGroup on the way
        // means `idx` is ungrouped and settles nothing.
        let Some(g) = self.slots[..=idx]
            .iter()
            .rposition(|s| matches!(s, Slot::Group { .. }) || matches!(s, Slot::EndGroup))
        else {
            return;
        };
        if !matches!(&self.slots[g], Slot::Group { bar: Some(_), .. }) {
            return; // ungrouped, already printed, or already dropped
        }
        let span_end = self.slots[g + 1..]
            .iter()
            .position(|s| matches!(s, Slot::Group { .. } | Slot::EndGroup))
            .map_or(self.slots.len(), |p| g + 1 + p);
        let settled = self.slots[g + 1..span_end].iter().all(|s| match s {
            Slot::Step { state, .. } => matches!(state, StepState::Resolved),
            Slot::Note { bar, .. } => bar.is_none(),
            // Unreachable: the span ends at the next group/terminator.
            Slot::Group { .. } | Slot::EndGroup => true,
        });
        if settled {
            self.drop_group_bar_at(g);
        }
    }

    fn active_bar(&self) -> Option<ProgressBar> {
        self.slots.iter().find_map(|s| match s {
            Slot::Step {
                bar: Some(bar),
                state: StepState::Active { .. },
                ..
            } => Some(bar.clone()),
            _ => None,
        })
    }

    fn first_live_bar_after(&self, idx: usize) -> ProgressBar {
        self.slots[idx + 1..]
            .iter()
            .find_map(|s| match s {
                // A group's (or section-to-be's) topmost live bar is its
                // spacer: content inserted `insert_before` this anchor must
                // land above the blank line, not between it and the row —
                // below the blank it would read as part of the next section.
                Slot::Group {
                    spacer: Some(b), ..
                }
                | Slot::Group { bar: Some(b), .. }
                | Slot::Note { bar: Some(b), .. }
                | Slot::Step {
                    spacer_above: Some(b),
                    ..
                }
                | Slot::Step { bar: Some(b), .. } => Some(b.clone()),
                _ => None,
            })
            .unwrap_or_else(|| self.bottom_spacer.clone())
    }

    fn clear_detail(&mut self) {
        if let Some(bar) = self.detail_bar.take() {
            self.mp.remove(&bar);
        }
    }
}

/// How `finish` treats steps that never resolved.
#[derive(Clone, Copy)]
pub(super) enum UnresolvedPolicy {
    /// Persist them as dim `○ … (not run)` rows (failure teardown).
    NotReached,
    /// Drop them silently (clean finishes have none by construction).
    Drop,
}

/// Final face requested by the caller (duration is computed internally).
pub(super) enum FinalFace {
    Done,
    Failed,
    SkippedExpected,
    SkippedAttention,
}

pub(super) enum Resolution {
    /// Persist a final row.
    Final {
        face: FinalFace,
        annotation: Option<String>,
    },
    /// Remove the row without a trace (benign hook skip).
    Silent,
}

/// Which tense/label variant to render.
#[derive(Clone, Copy)]
enum StepPhase {
    Pending,
    Active,
    Done,
    Skipped,
}

fn display_label(spec: &StepSpec, phase: StepPhase) -> String {
    // A fixed label (a shared file's path) wins in every phase — the face
    // glyph alone carries the row's state.
    if let Some(label) = &spec.label {
        return label.clone();
    }
    let labels = super::plan::labels_for(spec.key.id);
    let base = match phase {
        StepPhase::Pending => labels.pending,
        StepPhase::Active => labels.active,
        StepPhase::Done => labels.done,
        StepPhase::Skipped => labels.skipped,
    };
    base.to_string()
}

/// Static single-line bar: template is just the message (never empty — every
/// rendered row has at least a glyph).
fn line_style() -> ProgressStyle {
    ProgressStyle::with_template("{msg}").expect("static template is valid")
}

/// Wrap a rendered row in the rail gutter when it belongs to a group span.
fn in_span(line: String, in_group: bool, use_color: bool) -> String {
    if in_group {
        render::gutter(&line, use_color)
    } else {
        line
    }
}

fn active_style(use_color: bool, in_group: bool) -> ProgressStyle {
    let base = if use_color {
        "{spinner:.cyan}  {msg}"
    } else {
        "{spinner}  {msg}"
    };
    // In-span rows carry the gutter in the template — the spinner glyph
    // lives there, so the message alone can't provide the prefix.
    let template = in_span(base.to_string(), in_group, use_color);
    ProgressStyle::with_template(&template)
        .expect("active template is valid")
        .tick_chars(TICK_CHARS)
}

fn add_line_bar(mp: &MultiProgress, style: &ProgressStyle, line: String) -> ProgressBar {
    let bar = mp.add(ProgressBar::new_spinner());
    bar.set_style(style.clone());
    bar.set_message(line);
    bar
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stage::StageId;

    #[test]
    fn label_override_wins_in_every_phase() {
        let spec = StepSpec::new(StepKey::scoped(StageId::SharedFile, ".env")).with_label(".env");
        for phase in [
            StepPhase::Pending,
            StepPhase::Active,
            StepPhase::Done,
            StepPhase::Skipped,
        ] {
            assert_eq!(display_label(&spec, phase), ".env");
        }
        let plain = StepSpec::new(StepKey::new(StageId::Push));
        assert_eq!(display_label(&plain, StepPhase::Done), "Pushed");
    }
}
