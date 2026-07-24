//! Hook-manager output recognition (#753).
//!
//! Path A pre-push hooks are opaque to daft: git dispatches the hook and daft
//! sees one merged output stream. When that stream is a hook *manager*
//! (lefthook today; husky and friends later), it carries a stable line grammar
//! naming every job the manager ran — enough to render the manager's jobs as
//! first-class hook-job rows instead of one synthetic `pre-push` blob.
//!
//! This module is the manager-agnostic surface: [`ManagerEvent`] (what a
//! recognized stream reports), [`ManagerRecognizer`] (one manager's line
//! grammar), and [`Detector`] (probes every registered recognizer over the
//! first few lines and settles on engage or decline). Adding a manager is one
//! new file implementing [`ManagerRecognizer`] plus a line in
//! [`Detector::new`] — nothing above this module names a specific manager.
//!
//! Two invariants the whole design leans on:
//!
//! - **Decline is silent and total.** A stream no recognizer claims is
//!   replayed byte-for-byte and then passed through untouched — unrecognized
//!   hooks render exactly as they do today. The probe is bounded to the first
//!   few content lines so a slow plain hook's liveness is never held hostage.
//! - **Recognizers route, never rewrite.** Every event's payload is the raw
//!   line (or a name cut from it); matching happens on an ANSI-stripped,
//!   whitespace-rstripped copy, but what flows onward is what arrived.
//!   Verdicts and exit policy stay derived from the subprocess result
//!   (`push::collapse`), never from recognized events.
//!
//! The grammar authority for each recognizer is its captured-fixture tree
//! under `tests/fixtures/manager-output/` — real piped output, one directory
//! per raw-variant group. Trust the fixtures, not documentation or memory of
//! any one version's output.

pub mod lefthook;

use std::path::Path;
use std::time::Duration;

/// The jobs a recognized manager would run for `hook`, read from the manager's
/// own config under `dir`. Modular: a new manager adds one arm. This is the
/// display-only roster seed (#753) — see [`lefthook::roster`] for the contract.
/// Empty for an unknown manager or any unreadable config.
pub fn roster(manager: &str, dir: &Path, hook: &str) -> Vec<String> {
    match manager {
        "lefthook" => lefthook::roster(dir, hook),
        _ => Vec::new(),
    }
}

/// What a recognized manager stream reports, in stream order.
#[derive(Debug, Clone, PartialEq)]
pub enum ManagerEvent {
    /// The stream belongs to this manager. Always the first event, exactly
    /// once. `hook` is the hook name the manager printed (e.g. `pre-push`).
    Engaged {
        manager: &'static str,
        version: Option<String>,
        hook: Option<String>,
    },
    /// A job appeared. In buffered mode (lefthook's default) a job's block is
    /// flushed when the job *completes*, so "started" really means "finished
    /// running, output follows"; in follow mode headers print at true start.
    /// Either way the row's lifecycle is start → output → resolve.
    JobStarted { name: String },
    /// A line belonging to the named job's block, raw as it arrived.
    JobOutput { name: String, line: String },
    /// The manager skipped a job before running it (condition, glob, …).
    JobSkipped { name: String, reason: String },
    /// The manager's per-job verdict from its summary section. `duration` is
    /// the manager's own measurement when it printed one.
    JobResolved {
        name: String,
        ok: bool,
        duration: Option<Duration>,
    },
    /// The manager reported the phase finished. Jobs still unresolved after
    /// the stream ends (no summary: manager killed, output suppressed) are
    /// the consumer's to reconcile against the phase verdict.
    PhaseDone { total: Option<Duration> },
}

/// One probing step: what a still-undecided recognizer made of a line.
#[derive(Debug)]
pub enum Probe {
    /// Consistent with this manager's opening grammar, but not yet decisive.
    Pending,
    /// Decisively this manager's stream; the events cover every line consumed
    /// during the probe (the buffered prefix was the manager's banner).
    Engaged(Vec<ManagerEvent>),
    /// Decisively not this manager's stream.
    Declined,
}

/// One hook manager's line grammar.
///
/// Implementations are plain state machines: no IO, no locks, no clock. The
/// caller feeds raw lines; matching happens on an internally stripped copy.
pub trait ManagerRecognizer: Send {
    /// Manager name, e.g. `"lefthook"`.
    fn manager(&self) -> &'static str;

    /// Feed one raw line while undecided. Once this returns
    /// [`Probe::Engaged`] the caller switches to [`Self::feed`]; once it
    /// returns [`Probe::Declined`] the recognizer is dropped.
    fn probe(&mut self, raw_line: &str) -> Probe;

    /// Feed one raw line after engagement. Unrecognized lines are the
    /// stream's noise (trailing git output, decorative frames) and yield no
    /// events by design — they must not be attributed to jobs.
    fn feed(&mut self, raw_line: &str) -> Vec<ManagerEvent>;

    /// The stream ended. A recognizer must not synthesize verdicts here —
    /// missing resolutions are signal for the consumer's reconciliation.
    fn finish(&mut self) -> Vec<ManagerEvent>;
}

/// Longest opening-line prefix a recognizer may hold as [`Probe::Pending`]
/// before the detector force-declines. Generous: the longest legitimate
/// prefix observed is frame + banner (+ a `sync hooks:` notice), well under
/// this. The bound exists so a pathological stream of frame-like lines can't
/// buffer unboundedly while the user sees nothing.
const PROBE_BUDGET: usize = 8;

/// What [`Detector::feed`] made of a line.
#[derive(Debug)]
pub enum DetectorStep {
    /// Undecided: the line was withheld (buffered for a possible replay).
    Buffering,
    /// Just declined: forward these withheld lines (the current line is the
    /// last of them), then treat the stream as plain passthrough.
    Declined { replay: Vec<String> },
    /// Declined earlier: the caller forwards the line itself.
    Passthrough,
    /// Engaged (now or earlier): structured events for this line. Empty when
    /// the line was manager noise.
    Events(Vec<ManagerEvent>),
}

/// How the stream ended, from [`Detector::finish`].
#[derive(Debug)]
pub enum DetectorEnd {
    /// Was engaged; any final events.
    Engaged(Vec<ManagerEvent>),
    /// Stream ended while still probing: forward the withheld lines.
    Undecided { replay: Vec<String> },
    /// Had already declined; everything already flowed through.
    Declined,
}

/// Feeds a stream's opening lines to every registered recognizer until one
/// engages or all decline, then routes the rest of the stream accordingly.
///
/// First engage wins; registry order is priority order. One detector serves
/// one stream — construct a fresh one per hook phase (a reused presenter
/// crossing phases resets by replacing its detector).
pub struct Detector {
    /// Recognizers still in the running while probing.
    candidates: Vec<Box<dyn ManagerRecognizer>>,
    /// The winner, once one engages.
    engaged: Option<Box<dyn ManagerRecognizer>>,
    /// All candidates declined (or the budget ran out).
    declined: bool,
    /// Withheld lines, replayed verbatim on decline.
    buffer: Vec<String>,
}

impl Detector {
    /// A detector over every supported manager. Registry order is priority
    /// order on the (unlikely) day two grammars overlap.
    pub fn new() -> Self {
        Self::with(vec![Box::new(lefthook::Lefthook::new())])
    }

    /// A detector over an explicit recognizer set (tests).
    pub fn with(candidates: Vec<Box<dyn ManagerRecognizer>>) -> Self {
        Self {
            candidates,
            engaged: None,
            declined: false,
            buffer: Vec::new(),
        }
    }

    /// The engaged manager's name, if any.
    pub fn engaged_manager(&self) -> Option<&'static str> {
        self.engaged.as_ref().map(|r| r.manager())
    }

    /// Feed one raw line.
    pub fn feed(&mut self, raw_line: &str) -> DetectorStep {
        if self.declined {
            return DetectorStep::Passthrough;
        }
        if let Some(recognizer) = self.engaged.as_mut() {
            return DetectorStep::Events(recognizer.feed(raw_line));
        }

        self.buffer.push(raw_line.to_string());
        let mut still_pending = Vec::new();
        let mut winner: Option<(Box<dyn ManagerRecognizer>, Vec<ManagerEvent>)> = None;
        for mut candidate in self.candidates.drain(..) {
            if winner.is_some() {
                continue; // First engage wins; drop the rest.
            }
            match candidate.probe(raw_line) {
                Probe::Pending => still_pending.push(candidate),
                Probe::Engaged(events) => winner = Some((candidate, events)),
                Probe::Declined => {}
            }
        }

        if let Some((recognizer, events)) = winner {
            self.engaged = Some(recognizer);
            self.buffer.clear();
            return DetectorStep::Events(events);
        }

        self.candidates = still_pending;
        if self.candidates.is_empty() || self.buffer.len() >= PROBE_BUDGET {
            self.declined = true;
            self.candidates.clear();
            return DetectorStep::Declined {
                replay: std::mem::take(&mut self.buffer),
            };
        }
        DetectorStep::Buffering
    }

    /// The stream ended.
    pub fn finish(&mut self) -> DetectorEnd {
        if let Some(recognizer) = self.engaged.as_mut() {
            return DetectorEnd::Engaged(recognizer.finish());
        }
        if self.declined {
            return DetectorEnd::Declined;
        }
        self.declined = true;
        DetectorEnd::Undecided {
            replay: std::mem::take(&mut self.buffer),
        }
    }
}

impl Default for Detector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(detector: &mut Detector, lines: &[&str]) -> Vec<DetectorStep> {
        lines.iter().map(|l| detector.feed(l)).collect()
    }

    #[test]
    fn a_plain_stream_declines_on_its_first_line_and_replays_it() {
        let mut detector = Detector::new();
        match detector.feed("Running pre-push checks...") {
            DetectorStep::Declined { replay } => {
                assert_eq!(replay, vec!["Running pre-push checks...".to_string()]);
            }
            other => panic!("expected immediate decline, got {other:?}"),
        }
        // Everything after a decline is the caller's to forward.
        assert!(matches!(detector.feed("more"), DetectorStep::Passthrough));
        assert!(matches!(detector.finish(), DetectorEnd::Declined));
    }

    #[test]
    fn a_custom_box_banner_holds_one_line_then_declines_with_both() {
        // The tricky decline: a hook drawing its own ╭──╮ box. The frame is a
        // legitimate lefthook prefix, so it buffers — the non-banner text on
        // the next line must decline and replay BOTH lines in arrival order.
        let mut detector = Detector::new();
        let steps = feed_all(&mut detector, &["╭───╮", "🔧 my custom pre-push"]);
        assert!(matches!(steps[0], DetectorStep::Buffering));
        match &steps[1] {
            DetectorStep::Declined { replay } => {
                assert_eq!(
                    replay,
                    &["╭───╮".to_string(), "🔧 my custom pre-push".to_string()]
                );
            }
            other => panic!("expected decline with replay, got {other:?}"),
        }
    }

    #[test]
    fn an_endless_frame_stream_declines_at_the_probe_budget() {
        // Pathological guard: a stream of frame-like lines must not buffer
        // forever while the user sees nothing.
        let mut detector = Detector::new();
        let mut declined_at = None;
        for i in 0..PROBE_BUDGET + 2 {
            match detector.feed("╭───╮") {
                DetectorStep::Buffering => {}
                DetectorStep::Declined { replay } => {
                    assert_eq!(replay.len(), i + 1, "replay must cover every withheld line");
                    declined_at = Some(i);
                    break;
                }
                other => panic!("unexpected step {other:?}"),
            }
        }
        assert_eq!(declined_at, Some(PROBE_BUDGET - 1));
    }

    #[test]
    fn a_stream_that_ends_mid_probe_replays_from_finish() {
        // A hook that prints one frame line and dies: the withheld line must
        // come back out at end-of-stream, not vanish.
        let mut detector = Detector::new();
        assert!(matches!(detector.feed("╭───╮"), DetectorStep::Buffering));
        match detector.finish() {
            DetectorEnd::Undecided { replay } => {
                assert_eq!(replay, vec!["╭───╮".to_string()]);
            }
            other => panic!("expected undecided replay, got {other:?}"),
        }
    }

    #[test]
    fn an_engaged_stream_reports_its_manager() {
        let mut detector = Detector::new();
        assert_eq!(detector.engaged_manager(), None);
        detector.feed("╭──────╮");
        detector.feed("│ 🥊 lefthook  v2.1.10   hook:  pre-push │");
        assert_eq!(detector.engaged_manager(), Some("lefthook"));
    }
}
