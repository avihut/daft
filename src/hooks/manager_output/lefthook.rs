//! Lefthook piped-output recognizer.
//!
//! Grammar authority: `tests/fixtures/manager-output/lefthook/` — real piped
//! captures from **every released 2.x** (all 28 binaries, clustered into five
//! raw-variant groups; see the README there for the matrix and the capture
//! protocol). The structural anchors never changed across the line:
//!
//! ```text
//! ╭────────╮
//! │ [🥊 ]lefthook  vX.Y.Z   hook:  <hook> │      spacing varies by era → \s+
//! ╰────────╯
//! │  <job> (skip) <reason>                       skipped jobs, absent from summary
//! ┃  <job> ❯                                     block header (≤2.1.6: blank line follows)
//! <job stdout+stderr, verbatim>
//! exit status <N>                                only when the job failed
//!
//!   ─────────────
//! summary: (done in <T> seconds)
//! <glyph> <job> (<D> seconds)                    ✔️/✓ pass · 🥊/✗ fail
//! ```
//!
//! Era tolerances baked in: `NO_COLOR` ignored ≤2.1.2 (emoji glyphs stay);
//! CRLF job output ≤2.1.5; width-padded lines and a blank line after the
//! block header ≤2.1.6 — all absorbed by matching on an ANSI-stripped,
//! whitespace/`\r`-rstripped copy of each line while forwarding the raw one.
//!
//! The colour-mode fail glyph is the banner's own 🥊, so outcome lines are
//! only meaningful after `summary:` opens the summary section — matching them
//! earlier would collide with the banner.

use super::{ManagerEvent, ManagerRecognizer, Probe};
use crate::output::format::strip_ansi;
use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

/// Banner text line: `│ 🥊 lefthook  v2.1.10   hook:  pre-push │`. The emoji
/// is absent under `NO_COLOR` from 2.1.3, and inter-word spacing drifted at
/// 2.1.7 and 2.1.9 — hence optional emoji and `\s+` throughout.
static BANNER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^│\s*(?:🥊\s*)?lefthook\s+v(\S+)\s+hook:\s+(.+?)\s*│$").expect("valid regex")
});

/// Banner box frames. Corners are U+256D/U+256E (top) and U+2570/U+256F
/// (bottom); the run is U+2500. Distinct from the summary separator, which
/// has no corners.
static TOP_FRAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^╭─*╮$").expect("valid regex"));
static BOTTOM_FRAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^╰─*╯$").expect("valid regex"));

/// Skip notice: `│  <job> (skip) <reason>` — thin bar U+2502, not the thick
/// block-header bar U+2503. Skipped jobs never reach the summary.
static SKIP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^│\s+(.+?) \(skip\)\s*(.*)$").expect("valid regex"));

/// Job block header: `┃  <job> ❯` (thick bar U+2503, chevron U+276F).
/// Identical across every 2.x release.
static JOB_HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^┃\s+(.+?)\s+❯$").expect("valid regex"));

/// Summary separator: leading whitespace plus a bare U+2500 run.
static SEPARATOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*─+$").expect("valid regex"));

/// Per-job outcome line in the summary section:
/// `✔️ unit tests (related) (0.77 seconds)`. The greedy name backtracks off
/// the final duration parens, so parenthesized job names survive. Glyphs:
/// pass `✔`(+VS16)/`✓`, fail `🥊`/`✗` (`❌`/`✖` accepted defensively).
static OUTCOME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([✔✓🥊✗❌✖])\u{FE0F}?\s+(.+)\s+\((\d+(?:\.\d+)?)\s*seconds?\)$")
        .expect("valid regex")
});

/// Phase total inside the `summary:` line: `(done in 0.77 seconds)`.
static SUMMARY_TOTAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(done in\s+(\d+(?:\.\d+)?)\s*seconds?\)").expect("valid regex"));

enum State {
    /// Undecided; only banner-prefix lines are tolerated.
    Probing,
    /// Engaged, before the summary: skip notices, block headers, job output.
    Body,
    /// After `summary:`: outcome lines; everything else (git's trailing push
    /// result on the merged stream) is noise.
    Summary,
}

/// Recognizer for lefthook 2.x piped output.
pub struct Lefthook {
    state: State,
    /// The job whose block is open; lines are attributed to it. In follow
    /// mode headers print at job start and outputs interleave — attributing
    /// to the most recent header matches what the raw stream shows a reader.
    current_job: Option<String>,
}

impl Lefthook {
    pub fn new() -> Self {
        Self {
            state: State::Probing,
            current_job: None,
        }
    }

    /// ANSI-stripped, right-trimmed view a line is matched on. `\r` covers
    /// the CRLF era (≤2.1.5); trailing spaces cover width-padding (≤2.1.6).
    fn matchable(raw_line: &str) -> String {
        let stripped = strip_ansi(raw_line);
        stripped.trim_end_matches([' ', '\t', '\r']).to_string()
    }
}

impl Default for Lefthook {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_seconds(text: &str) -> Option<Duration> {
    text.parse::<f64>().ok().map(Duration::from_secs_f64)
}

/// Config file names lefthook reads from a repo root, in preference order.
/// Best-effort: `extends:`, `lefthook-local.yml` overrides, and
/// `$LEFTHOOK_CONFIG` are not followed. Under-listing is harmless — a job they
/// add is revealed when its block flushes, exactly as without a seed.
const CONFIG_NAMES: &[&str] = &[
    "lefthook.yml",
    "lefthook.yaml",
    ".lefthook.yml",
    ".lefthook.yaml",
];

/// The jobs lefthook would run for `hook`, read from its config in `dir`.
///
/// This is the display-only roster seed (#753): lefthook's default (buffered)
/// output reveals a job only when it *completes*, so a long job is invisible
/// for its whole run. The routing wrapper shows these names as pending rows up
/// front and lets the stream resolve them. It never feeds verdict or exit
/// policy — a seeded job the run never resolves settles as skipped, and the
/// push verdict stays `PushIo`-derived. Empty on any missing, unreadable, or
/// unparseable config (the stream then reveals jobs as before).
pub fn roster(dir: &Path, hook: &str) -> Vec<String> {
    for name in CONFIG_NAMES {
        if let Ok(text) = std::fs::read_to_string(dir.join(name)) {
            return parse_roster(&text, hook);
        }
    }
    Vec::new()
}

/// Job names under `<hook>` in a lefthook config: `commands:` map keys and
/// `jobs:` list `name`s, in config order. Tolerant of either or both forms and
/// of unrelated keys; any parse error or missing hook yields an empty roster.
fn parse_roster(yaml: &str, hook: &str) -> Vec<String> {
    let Ok(root) = serde_yaml::from_str::<serde_yaml::Value>(yaml) else {
        return Vec::new();
    };
    let Some(section) = root.get(hook) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    // `commands:` — a mapping of job-name -> spec. serde_yaml's Mapping keeps
    // insertion (config) order.
    if let Some(commands) = section.get("commands").and_then(|v| v.as_mapping()) {
        names.extend(
            commands
                .keys()
                .filter_map(|k| k.as_str().map(str::to_string)),
        );
    }
    // `jobs:` — a sequence of specs, each carrying its own `name`.
    if let Some(jobs) = section.get("jobs").and_then(|v| v.as_sequence()) {
        names.extend(
            jobs.iter()
                .filter_map(|job| job.get("name").and_then(|v| v.as_str()).map(str::to_string)),
        );
    }
    names
}

impl ManagerRecognizer for Lefthook {
    fn manager(&self) -> &'static str {
        "lefthook"
    }

    fn probe(&mut self, raw_line: &str) -> Probe {
        debug_assert!(
            matches!(self.state, State::Probing),
            "probe() after engagement"
        );
        let line = Self::matchable(raw_line);

        if let Some(captures) = BANNER.captures(&line) {
            self.state = State::Body;
            return Probe::Engaged(vec![ManagerEvent::Engaged {
                manager: self.manager(),
                version: Some(captures[1].to_string()),
                hook: Some(captures[2].to_string()),
            }]);
        }
        // The only things lefthook prints before its banner: the box's top
        // frame and (on config drift) a `sync hooks:` notice. A blank is
        // tolerated as prefix slack. Anything else is not lefthook.
        if line.is_empty() || TOP_FRAME.is_match(&line) || line.starts_with("sync hooks:") {
            return Probe::Pending;
        }
        Probe::Declined
    }

    fn feed(&mut self, raw_line: &str) -> Vec<ManagerEvent> {
        let line = Self::matchable(raw_line);
        match self.state {
            State::Probing => {
                debug_assert!(false, "feed() before engagement");
                Vec::new()
            }
            State::Body => {
                if line.starts_with("summary:") {
                    self.state = State::Summary;
                    self.current_job = None;
                    let total = SUMMARY_TOTAL
                        .captures(&line)
                        .and_then(|c| parse_seconds(&c[1]));
                    return vec![ManagerEvent::PhaseDone { total }];
                }
                if let Some(captures) = JOB_HEADER.captures(&line) {
                    let name = captures[1].to_string();
                    self.current_job = Some(name.clone());
                    return vec![ManagerEvent::JobStarted { name }];
                }
                if let Some(captures) = SKIP.captures(&line) {
                    return vec![ManagerEvent::JobSkipped {
                        name: captures[1].to_string(),
                        reason: captures[2].to_string(),
                    }];
                }
                if SEPARATOR.is_match(&line) {
                    // The summary separator closes the last block; blanks
                    // after it must not be attributed to that job.
                    self.current_job = None;
                    return Vec::new();
                }
                if BOTTOM_FRAME.is_match(&line) || TOP_FRAME.is_match(&line) {
                    return Vec::new();
                }
                if self.current_job.is_none() && line.starts_with("sync hooks:") {
                    return Vec::new();
                }
                match &self.current_job {
                    // Inside a block every line is that job's output — blanks
                    // included (they are part of the block's shape; failure
                    // dumps trim their own edges).
                    Some(name) => vec![ManagerEvent::JobOutput {
                        name: name.clone(),
                        line: raw_line.to_string(),
                    }],
                    // Between the banner and the first block: noise.
                    None => Vec::new(),
                }
            }
            State::Summary => {
                if let Some(captures) = OUTCOME.captures(&line) {
                    let ok = matches!(&captures[1], "✔" | "✓");
                    return vec![ManagerEvent::JobResolved {
                        name: captures[2].to_string(),
                        ok,
                        duration: parse_seconds(&captures[3]),
                    }];
                }
                // Git's own push-result lines follow the hook on the merged
                // stream; the push verdict is derived from PushIo, never from
                // here — drop them.
                Vec::new()
            }
        }
    }

    fn finish(&mut self) -> Vec<ManagerEvent> {
        // No summary means the manager died mid-run (or suppressed output).
        // Missing resolutions are the reconciliation signal — synthesizing
        // verdicts here would erase it.
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Detector, DetectorEnd, DetectorStep};
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/manager-output")
    }

    /// The outcome of running one captured stream through a fresh detector.
    struct Run {
        events: Vec<ManagerEvent>,
        /// Set when the stream declined (at a line or at finish).
        replay: Option<Vec<String>>,
    }

    fn run_stream(text: &str) -> Run {
        let mut detector = Detector::new();
        let mut events = Vec::new();
        let mut replay = None;
        let mut lines: Vec<&str> = text.split('\n').collect();
        // A trailing newline yields one empty tail segment no reader would
        // deliver as a line.
        if lines.last() == Some(&"") {
            lines.pop();
        }
        for line in lines {
            match detector.feed(line) {
                DetectorStep::Events(mut batch) => events.append(&mut batch),
                DetectorStep::Declined { replay: lines } => replay = Some(lines),
                DetectorStep::Buffering | DetectorStep::Passthrough => {}
            }
        }
        match detector.finish() {
            DetectorEnd::Engaged(mut batch) => events.append(&mut batch),
            DetectorEnd::Undecided { replay: lines } => replay = Some(lines),
            DetectorEnd::Declined => {}
        }
        Run { events, replay }
    }

    fn read_fixture(path: &Path) -> String {
        String::from_utf8_lossy(&fs::read(path).expect("fixture readable")).into_owned()
    }

    impl Run {
        fn engaged(&self) -> Option<(&str, Option<&str>, Option<&str>)> {
            self.events.iter().find_map(|e| match e {
                ManagerEvent::Engaged {
                    manager,
                    version,
                    hook,
                } => Some((*manager, version.as_deref(), hook.as_deref())),
                _ => None,
            })
        }
        fn started(&self) -> Vec<&str> {
            self.events
                .iter()
                .filter_map(|e| match e {
                    ManagerEvent::JobStarted { name } => Some(name.as_str()),
                    _ => None,
                })
                .collect()
        }
        fn resolved(&self) -> BTreeMap<&str, bool> {
            self.events
                .iter()
                .filter_map(|e| match e {
                    ManagerEvent::JobResolved { name, ok, .. } => Some((name.as_str(), *ok)),
                    _ => None,
                })
                .collect()
        }
        fn skipped(&self) -> BTreeMap<&str, &str> {
            self.events
                .iter()
                .filter_map(|e| match e {
                    ManagerEvent::JobSkipped { name, reason } => {
                        Some((name.as_str(), reason.as_str()))
                    }
                    _ => None,
                })
                .collect()
        }
        fn outputs_for(&self, job: &str) -> Vec<&str> {
            self.events
                .iter()
                .filter_map(|e| match e {
                    ManagerEvent::JobOutput { name, line } if name == job => Some(line.as_str()),
                    _ => None,
                })
                .collect()
        }
        fn output_count(&self) -> usize {
            self.events
                .iter()
                .filter(|e| matches!(e, ManagerEvent::JobOutput { .. }))
                .count()
        }
        fn phase_done_count(&self) -> usize {
            self.events
                .iter()
                .filter(|e| matches!(e, ManagerEvent::PhaseDone { .. }))
                .count()
        }
    }

    fn assert_all_ok(run: &Run, jobs: &[&str], context: &str) {
        let resolved = run.resolved();
        for job in jobs {
            assert_eq!(
                resolved.get(job),
                Some(&true),
                "{context}: '{job}' must resolve ok (resolved: {resolved:?})"
            );
        }
        let started = run.started();
        for job in jobs {
            assert!(
                started.contains(job),
                "{context}: '{job}' must have a started event (started: {started:?})"
            );
        }
    }

    /// Every captured fixture in every version directory parses, with
    /// scenario-keyed expectations. An unknown scenario name fails loudly so
    /// a future fixture can't land unasserted.
    #[test]
    fn every_captured_fixture_across_the_2x_line_parses() {
        let root = fixture_root().join("lefthook");
        let mut checked = 0usize;
        for dir_entry in fs::read_dir(&root).expect("fixture root readable") {
            let dir = dir_entry.expect("readable entry").path();
            if !dir.is_dir() {
                continue;
            }
            let dir_name = dir.file_name().unwrap().to_string_lossy().into_owned();
            let version = dir_name.strip_prefix('v').expect("version dir");
            for file_entry in fs::read_dir(&dir).expect("version dir readable") {
                let file = file_entry.expect("readable entry").path();
                if file.extension().and_then(|e| e.to_str()) != Some("txt") {
                    continue;
                }
                let stem = file.file_stem().unwrap().to_string_lossy().into_owned();
                let run = run_stream(&read_fixture(&file));
                let context = format!("{dir_name}/{stem}");
                check_scenario(&stem, version, &run, &context);
                checked += 1;
            }
        }
        // 5 matrix files x 5 version dirs, plus the v2.1.10 extras.
        assert!(
            checked >= 27,
            "expected the full fixture tree, got {checked}"
        );
    }

    fn check_scenario(stem: &str, version: &str, run: &Run, context: &str) {
        // Everything except the suppressed-output capture engages as
        // lefthook with the capturing binary's version.
        if stem != "output-summary-pass" {
            let (manager, banner_version, hook) = run
                .engaged()
                .unwrap_or_else(|| panic!("{context}: must engage"));
            assert_eq!(manager, "lefthook", "{context}");
            assert_eq!(banner_version, Some(version), "{context}: banner version");
            assert_eq!(hook, Some("pre-push"), "{context}: hook name");
        }

        match stem {
            "parallel-pass" | "no-color-pass" => {
                assert_all_ok(
                    run,
                    &["fmt", "clippy", "unit tests (related)", "typecheck"],
                    context,
                );
                assert_eq!(run.phase_done_count(), 1, "{context}");
            }
            "parallel-fail" | "no-color-fail" => {
                assert_all_ok(run, &["fmt", "typecheck"], context);
                assert_eq!(
                    run.resolved().get("unit tests (related)"),
                    Some(&false),
                    "{context}: the failing job must resolve failed \
                     (era glyph 🥊 or ✗; resolved: {:?})",
                    run.resolved()
                );
                assert!(
                    run.outputs_for("unit tests (related)")
                        .iter()
                        .any(|l| l.contains("exit status 1")),
                    "{context}: the failing block carries its exit-status line"
                );
            }
            "serial-pass" => {
                assert_all_ok(run, &["first", "second"], context);
            }
            "with-skips-pass" => {
                assert_all_ok(run, &["runs"], context);
                let skipped = run.skipped();
                assert_eq!(
                    skipped.get("skipped-by-flag"),
                    Some(&"by condition"),
                    "{context}"
                );
                assert_eq!(
                    skipped.get("skipped-by-glob"),
                    Some(&"no matching push files"),
                    "{context}"
                );
            }
            "no-summary" => {
                // Manager killed mid-run: jobs appeared, nothing resolved —
                // that absence is the reconciliation signal.
                assert_eq!(run.started().len(), 4, "{context}");
                assert!(run.resolved().is_empty(), "{context}");
                assert_eq!(run.phase_done_count(), 0, "{context}");
            }
            "remote-rejected" => {
                // Hook passed; the push was rejected AFTER it. Git's trailing
                // lines must be swallowed, and no job may look failed (#752).
                assert_all_ok(run, &["quick", "slow"], context);
                assert!(
                    run.resolved().values().all(|ok| *ok),
                    "{context}: no job may be flipped to failed by a push rejection"
                );
                assert!(
                    !run.events.iter().any(|e| matches!(
                        e,
                        ManagerEvent::JobOutput { line, .. } if line.contains("[rejected]")
                    )),
                    "{context}: git's push-result lines are not job output"
                );
            }
            "follow-pass" => {
                // follow: true — headers at job start, outputs interleaved.
                // Attribution follows the most recent header (the stream's
                // visual truth); resolution still comes from the summary.
                assert_all_ok(run, &["talker1", "talker2"], context);
                assert_eq!(run.output_count(), 6, "{context}: all six lines attributed");
            }
            "output-summary-pass" => {
                // output: [summary] suppresses the banner — no anchor, no
                // engagement: this stream stays passthrough, by design.
                assert!(run.engaged().is_none(), "{context}: must not engage");
                assert_eq!(
                    run.replay.as_deref().map(<[String]>::len),
                    Some(1),
                    "{context}: declined on its first line"
                );
            }
            other => panic!("unhandled fixture scenario '{other}' — add assertions for it"),
        }
    }

    #[test]
    fn decline_fixtures_replay_verbatim_and_never_engage() {
        let root = fixture_root().join("decline");
        let mut checked = 0usize;
        for entry in fs::read_dir(&root).expect("decline dir readable") {
            let file = entry.expect("readable entry").path();
            if file.extension().and_then(|e| e.to_str()) != Some("txt") {
                continue;
            }
            let text = read_fixture(&file);
            let run = run_stream(&text);
            let name = file.file_name().unwrap().to_string_lossy().into_owned();
            assert!(run.engaged().is_none(), "{name}: must not engage");
            let replay = run
                .replay
                .as_ref()
                .unwrap_or_else(|| panic!("{name}: must decline with a replay"));
            // The replay is exactly the stream's withheld prefix, in order —
            // byte-identical passthrough is the whole decline contract.
            let expected: Vec<&str> = text.split('\n').take(replay.len()).collect();
            assert_eq!(
                replay.iter().map(String::as_str).collect::<Vec<_>>(),
                expected,
                "{name}: replay must be the verbatim prefix"
            );
            checked += 1;
        }
        assert!(checked >= 2, "expected the decline fixtures, got {checked}");
    }

    #[test]
    fn banner_spacing_of_every_era_engages() {
        // The four spacing eras (2.0.0 baseline, 2.1.7, 2.1.9) plus the
        // NO_COLOR emoji-less form (2.1.3+). One regex covers all of them.
        for banner in [
            "│ 🥊 lefthook v2.0.0  hook: pre-push │",
            "│ 🥊 lefthook  v2.1.7  hook:  pre-push │",
            "│ 🥊 lefthook  v2.1.9   hook:  pre-push │",
            "│ lefthook v2.1.3  hook: pre-push │",
        ] {
            let mut recognizer = Lefthook::new();
            match recognizer.probe(banner) {
                Probe::Engaged(events) => {
                    assert!(
                        matches!(
                            &events[0],
                            ManagerEvent::Engaged { hook: Some(h), .. } if h == "pre-push"
                        ),
                        "banner {banner:?} must capture the hook name"
                    );
                }
                other => panic!("banner {banner:?} must engage, got {other:?}"),
            }
        }
    }

    #[test]
    fn ansi_styled_lines_match_like_their_plain_forms() {
        // Lefthook forces colour even when piped; the banner text arrives
        // wrapped in truecolor SGR. Matching must see through it.
        let styled = "\u{1b}[38;2;0;0;0m│\u{1b}[m 🥊 lefthook  v2.1.10   hook:  \
                      \u{1b}[1mpre-push\u{1b}[m \u{1b}[38;2;52;52;52m│\u{1b}[m";
        let mut recognizer = Lefthook::new();
        assert!(
            matches!(recognizer.probe(styled), Probe::Engaged(_)),
            "styled banner must engage"
        );
    }

    #[test]
    fn crlf_era_output_lines_stay_raw_but_match_stripped() {
        // ≤2.1.5 job output arrives CRLF; matching rstrips but the forwarded
        // payload keeps the bytes that arrived.
        let mut recognizer = Lefthook::new();
        recognizer.probe("│ 🥊 lefthook v2.0.0  hook: pre-push │");
        recognizer.feed("┃  fmt ❯ ");
        let events = recognizer.feed("fmt output line\r");
        assert_eq!(
            events,
            vec![ManagerEvent::JobOutput {
                name: "fmt".to_string(),
                line: "fmt output line\r".to_string(),
            }]
        );
    }

    #[test]
    fn outcome_glyphs_are_gated_to_the_summary_section() {
        // The colour-mode fail glyph is the banner's own 🥊 — an outcome-
        // shaped line before `summary:` must NOT resolve anything.
        let mut recognizer = Lefthook::new();
        recognizer.probe("│ 🥊 lefthook v2.0.0  hook: pre-push │");
        recognizer.feed("┃  fmt ❯ ");
        let premature = recognizer.feed("✔️ fmt (0.36 seconds)");
        assert_eq!(
            premature,
            vec![ManagerEvent::JobOutput {
                name: "fmt".to_string(),
                line: "✔️ fmt (0.36 seconds)".to_string(),
            }],
            "before the summary an outcome-shaped line is just block output"
        );
        recognizer.feed("summary: (done in 1.0 seconds)");
        let resolved = recognizer.feed("✔️ fmt (0.36 seconds)");
        assert_eq!(
            resolved,
            vec![ManagerEvent::JobResolved {
                name: "fmt".to_string(),
                ok: true,
                duration: Some(Duration::from_secs_f64(0.36)),
            }]
        );
    }

    #[test]
    fn a_parenthesized_job_name_survives_the_outcome_parse() {
        let mut recognizer = Lefthook::new();
        recognizer.probe("│ 🥊 lefthook v2.0.0  hook: pre-push │");
        recognizer.feed("summary: (done in 1.0 seconds)");
        let events = recognizer.feed("🥊 unit tests (related) (0.45 seconds)");
        assert_eq!(
            events,
            vec![ManagerEvent::JobResolved {
                name: "unit tests (related)".to_string(),
                ok: false,
                duration: Some(Duration::from_secs_f64(0.45)),
            }]
        );
    }

    #[test]
    fn a_summary_without_a_parsable_total_still_opens_the_summary() {
        let mut recognizer = Lefthook::new();
        recognizer.probe("│ 🥊 lefthook v2.0.0  hook: pre-push │");
        let events = recognizer.feed("summary: (skipped)");
        assert_eq!(events, vec![ManagerEvent::PhaseDone { total: None }]);
    }

    // ── roster seed (#753) ────────────────────────────────────────────────

    #[test]
    fn roster_reads_commands_in_config_order() {
        // The `commands:` map form (daft's own lefthook.yml). Order matters:
        // the seeded rows should mirror the config, not sort.
        let yaml = "\
pre-push:
  parallel: true
  commands:
    build-check:
      run: cargo check
    unit-tests:
      glob: \"*.rs\"
      run: mise run test:unit
";
        assert_eq!(
            parse_roster(yaml, "pre-push"),
            vec!["build-check", "unit-tests"]
        );
    }

    #[test]
    fn roster_reads_the_jobs_list_form() {
        // The newer `jobs:` sequence form, each entry naming itself.
        let yaml = "\
pre-push:
  jobs:
    - name: fmt
      run: cargo fmt --check
    - name: clippy
      run: cargo clippy
";
        assert_eq!(parse_roster(yaml, "pre-push"), vec!["fmt", "clippy"]);
    }

    #[test]
    fn roster_merges_commands_and_jobs_and_ignores_nameless_and_other_hooks() {
        let yaml = "\
pre-commit:
  commands:
    other-hook-job:
      run: true
pre-push:
  commands:
    a:
      run: true
  jobs:
    - name: b
      run: true
    - run: nameless-is-dropped
";
        assert_eq!(parse_roster(yaml, "pre-push"), vec!["a", "b"]);
    }

    #[test]
    fn roster_is_empty_for_a_missing_hook_or_malformed_config() {
        let yaml = "pre-commit:\n  commands:\n    x:\n      run: true\n";
        assert!(parse_roster(yaml, "pre-push").is_empty(), "hook absent");
        assert!(
            parse_roster("this: [is: not: valid", "pre-push").is_empty(),
            "malformed YAML yields no roster, never a panic"
        );
        assert!(
            parse_roster("pre-push: just-a-scalar", "pre-push").is_empty(),
            "a hook without commands/jobs yields no roster"
        );
    }

    #[test]
    fn roster_reads_the_first_config_file_present_and_is_empty_without_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(
            roster(dir.path(), "pre-push").is_empty(),
            "no config file → empty roster (stream reveals jobs as before)"
        );
        fs::write(
            dir.path().join("lefthook.yml"),
            "pre-push:\n  commands:\n    build-check:\n      run: cargo check\n",
        )
        .expect("write config");
        assert_eq!(roster(dir.path(), "pre-push"), vec!["build-check"]);
    }
}
