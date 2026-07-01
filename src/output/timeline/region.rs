//! Live-region driver for the rail timeline.
//!
//! One `MultiProgress` per command. The composition invariant that makes the
//! hook-block weld work: **completed content is persisted eagerly**
//! (`mp.remove(bar)` + `mp.println(line)` — the atomic visual swap), so at any
//! moment the live bars are exactly `{active?, pending…, bottom spacer,
//! footer placeholder}`. Any `mp.println` — a warning, or the embedded hook
//! renderer's header/dumps/summary — therefore lands *between* the persisted
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
const TICK_CHARS: &str =
    "\u{2807}\u{2819}\u{2839}\u{2838}\u{283c}\u{2834}\u{2826}\u{2827}\u{2807}\u{280f}";

/// What an embedded hook renderer needs to draw inside the region.
pub struct HookEmbed {
    pub mp: MultiProgress,
    /// Insertion anchor: hook job bars go `insert_before(anchor)`. Always a
    /// live rail bar (first pending row, else the bottom spacer), which
    /// stays alive for the whole splice — `insert_before` panics on a
    /// removed anchor, so liveness is a hard invariant.
    pub anchor: ProgressBar,
}

enum Slot {
    Group {
        label: String,
        bar: Option<ProgressBar>,
    },
    Note {
        text: String,
        bar: Option<ProgressBar>,
    },
    Step {
        spec: StepSpec,
        bar: Option<ProgressBar>,
        state: StepState,
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
    /// Whether the most recently persisted line is a rail spacer (`│`) —
    /// used to avoid doubling spacers around embedded hook blocks.
    last_persisted_was_spacer: bool,
    /// A hook block is currently rendering in place of one of our rows.
    hook_block_open: bool,
}

impl TimelineCore {
    /// Materialize the region: header + top spacer persist immediately, every
    /// plan row becomes a live bar, then the bottom spacer and the footer
    /// placeholder.
    pub(super) fn new(header: String, plan: PlanCommit, verbose: bool, use_color: bool) -> Self {
        let mp = MultiProgress::new();

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
        let mut slots = Vec::with_capacity(plan.rows.len());
        for row in plan.rows {
            let slot = match row {
                Row::Group { label } => {
                    let bar = add_line_bar(&mp, &static_style, render::group(&label, use_color));
                    Slot::Group {
                        label,
                        bar: Some(bar),
                    }
                }
                Row::Note { text } => {
                    let bar = add_line_bar(&mp, &static_style, render::note(&text, use_color));
                    Slot::Note {
                        text,
                        bar: Some(bar),
                    }
                }
                Row::Step(spec) => {
                    if let Some(elapsed) = spec.pre_completed {
                        // Completed before the region existed (clone's bare
                        // phase) — persist directly, no bar.
                        mp.println(render::final_row(
                            &RowFace::Done {
                                duration: Some(elapsed),
                            },
                            &display_label(&spec, StepPhase::Done),
                            spec.annotation.as_deref(),
                            label_width,
                            use_color,
                        ))
                        .ok();
                        last_persisted_was_spacer = false;
                        Slot::Step {
                            spec,
                            bar: None,
                            state: StepState::Resolved,
                        }
                    } else {
                        let line = render::pending_row(
                            &display_label(&spec, StepPhase::Pending),
                            spec.annotation.as_deref(),
                            label_width,
                            use_color,
                        );
                        let bar = add_line_bar(&mp, &static_style, line);
                        Slot::Step {
                            spec,
                            bar: Some(bar),
                            state: StepState::Pending,
                        }
                    }
                }
            };
            slots.push(slot);
        }

        let bottom_spacer = add_line_bar(&mp, &static_style, render::spacer(use_color));
        let footer = add_line_bar(&mp, &static_style, render::footer("\u{2026}", use_color));

        Self {
            mp,
            use_color,
            verbose,
            label_width,
            slots,
            bottom_spacer,
            footer,
            detail_bar: None,
            last_persisted_was_spacer,
            hook_block_open: false,
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

    /// The step began: persist everything above it, swap its bar to the
    /// active spinner style.
    pub(super) fn activate(&mut self, key: &StepKey) {
        let Some(idx) = self.step_index(key) else {
            return;
        };
        self.persist_preceding(idx);
        let use_color = self.use_color;
        let label_width = self.label_width;
        if let Slot::Step { spec, bar, state } = &mut self.slots[idx]
            && let Some(bar) = bar.as_ref()
        {
            let msg = render::active_message(
                &display_label(spec, StepPhase::Active),
                spec.annotation.as_deref(),
                label_width,
            );
            bar.set_style(active_style(use_color));
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
    pub(super) fn resolve(&mut self, key: &StepKey, resolution: Resolution) {
        let Some(idx) = self.step_index(key) else {
            return;
        };
        self.persist_preceding(idx);
        self.clear_detail();
        let use_color = self.use_color;
        let label_width = self.label_width;
        let Slot::Step { spec, bar, state } = &mut self.slots[idx] else {
            return;
        };
        let started = match state {
            StepState::Active { started } => Some(*started),
            _ => None,
        };
        let Some(taken) = bar.take() else {
            *state = StepState::Resolved;
            return;
        };
        taken.disable_steady_tick();
        self.mp.remove(&taken);
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
                    RowFace::Failed => StepPhase::Pending,
                    RowFace::SkippedExpected => StepPhase::Skipped,
                    _ => StepPhase::Done,
                };
                let line = render::final_row(
                    &face,
                    &display_label(spec, phase),
                    spec.annotation.as_deref(),
                    label_width,
                    use_color,
                );
                self.mp.println(line).ok();
                self.last_persisted_was_spacer = false;
            }
        }
        *state = StepState::Resolved;
    }

    /// Patch a pending/active row's annotation in place.
    pub(super) fn set_annotation(&mut self, key: &StepKey, annotation: String) {
        let Some(idx) = self.step_index(key) else {
            return;
        };
        let use_color = self.use_color;
        let label_width = self.label_width;
        if let Slot::Step { spec, bar, state } = &mut self.slots[idx] {
            spec.annotation = Some(annotation);
            if let Some(bar) = bar.as_ref() {
                match state {
                    StepState::Pending => bar.set_message(render::pending_row(
                        &display_label(spec, StepPhase::Pending),
                        spec.annotation.as_deref(),
                        label_width,
                        use_color,
                    )),
                    StepState::Active { .. } => bar.set_message(render::active_message(
                        &display_label(spec, StepPhase::Active),
                        spec.annotation.as_deref(),
                        label_width,
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
        self.persist_preceding(idx);
        self.clear_detail();
        if let Slot::Step { bar, state, .. } = &mut self.slots[idx] {
            if let Some(taken) = bar.take() {
                taken.disable_steady_tick();
                self.mp.remove(&taken);
            }
            *state = StepState::Resolved;
        }
        if !self.last_persisted_was_spacer {
            self.mp.println(render::spacer(self.use_color)).ok();
            self.last_persisted_was_spacer = true;
        }
        self.hook_block_open = true;
        let anchor = self.first_live_bar_after(idx);
        Some(HookEmbed {
            mp: self.mp.clone(),
            anchor,
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
    /// everything already resolved); trailing groups/notes persist as-is;
    /// then the closing spacer + footer.
    pub(super) fn finish(mut self, footer_text: &str, unresolved: UnresolvedPolicy) {
        self.clear_detail();
        let use_color = self.use_color;
        let label_width = self.label_width;
        for slot in &mut self.slots {
            match slot {
                Slot::Group { label, bar } => {
                    if let Some(taken) = bar.take() {
                        self.mp.remove(&taken);
                        self.mp.println(render::group(label, use_color)).ok();
                    }
                }
                Slot::Note { text, bar } => {
                    if let Some(taken) = bar.take() {
                        self.mp.remove(&taken);
                        self.mp.println(render::note(text, use_color)).ok();
                    }
                }
                Slot::Step { spec, bar, state } => {
                    if let Some(taken) = bar.take() {
                        taken.disable_steady_tick();
                        self.mp.remove(&taken);
                        match unresolved {
                            UnresolvedPolicy::NotReached => {
                                self.mp
                                    .println(render::final_row(
                                        &RowFace::NotReached,
                                        &display_label(spec, StepPhase::Pending),
                                        None,
                                        label_width,
                                        use_color,
                                    ))
                                    .ok();
                            }
                            UnresolvedPolicy::Drop => {}
                        }
                        *state = StepState::Resolved;
                    }
                }
            }
        }
        self.mp.remove(&self.bottom_spacer);
        self.mp.remove(&self.footer);
        self.mp.println(render::spacer(use_color)).ok();
        self.mp.println(render::footer(footer_text, use_color)).ok();
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

    /// Persist every not-yet-persisted group/note row above `idx` so the
    /// row at `idx` is the topmost live rail bar.
    fn persist_preceding(&mut self, idx: usize) {
        let use_color = self.use_color;
        for slot in &mut self.slots[..idx] {
            match slot {
                Slot::Group { label, bar } => {
                    if let Some(taken) = bar.take() {
                        self.mp.remove(&taken);
                        self.mp.println(render::group(label, use_color)).ok();
                        self.last_persisted_was_spacer = false;
                    }
                }
                Slot::Note { text, bar } => {
                    if let Some(taken) = bar.take() {
                        self.mp.remove(&taken);
                        self.mp.println(render::note(text, use_color)).ok();
                        self.last_persisted_was_spacer = false;
                    }
                }
                Slot::Step { .. } => {}
            }
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
                Slot::Group { bar: Some(b), .. }
                | Slot::Note { bar: Some(b), .. }
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

fn active_style(use_color: bool) -> ProgressStyle {
    let template = if use_color {
        "{spinner:.cyan}  {msg}"
    } else {
        "{spinner}  {msg}"
    };
    ProgressStyle::with_template(template)
        .expect("active template is valid")
        .tick_chars(TICK_CHARS)
}

fn add_line_bar(mp: &MultiProgress, style: &ProgressStyle, line: String) -> ProgressBar {
    let bar = mp.add(ProgressBar::new_spinner());
    bar.set_style(style.clone());
    bar.set_message(line);
    bar
}
