//! On-disk schema for structured per-job logs.
//!
//! Replaces the legacy single-file `output.log` (intermixed stdout+stderr,
//! raw text) with `output.jsonl`: one [`LogRecord`] per line.
//!
//! Each record carries a per-job sequence number (`seq`) and unix-ms
//! timestamp (`ts`). `seq` advances even when records are dropped by
//! sampling, so consumers can detect gaps. Lifecycle events (`Status`) are
//! never sampled.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::Write;

/// Stream tag carried through the in-process output channel between
/// `executor::command` (which spawns hook job processes) and the
/// `LogSink` consumers. Mapped to [`LogRecordKind::Stdout`] /
/// [`LogRecordKind::Stderr`] at sink time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogRecord {
    pub seq: u64,
    /// Unix milliseconds since epoch.
    pub ts: i64,
    #[serde(flatten)]
    pub kind: LogRecordKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "data", rename_all = "lowercase")]
pub enum LogRecordKind {
    Stdout(String),
    Stderr(String),
    Status(StatusEvent),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum StatusEvent {
    Started { pid: u32 },
    Finished { exit_code: Option<i32> },
    Signaled { signal: i32 },
    Crashed { message: String },
}

impl LogRecord {
    /// Construct a record with `ts = now()` and the given kind. Caller
    /// supplies `seq` so the sink owns the per-job sequence counter.
    pub fn new(seq: u64, kind: LogRecordKind) -> Self {
        Self {
            seq,
            ts: chrono::Utc::now().timestamp_millis(),
            kind,
        }
    }

    pub fn stdout(seq: u64, line: impl Into<String>) -> Self {
        Self::new(seq, LogRecordKind::Stdout(line.into()))
    }

    pub fn stderr(seq: u64, line: impl Into<String>) -> Self {
        Self::new(seq, LogRecordKind::Stderr(line.into()))
    }

    pub fn status(seq: u64, event: StatusEvent) -> Self {
        Self::new(seq, LogRecordKind::Status(event))
    }

    /// `true` for `Stdout`/`Stderr` (subject to sampling); `false` for `Status`
    /// (never sampled).
    pub fn is_output(&self) -> bool {
        matches!(
            self.kind,
            LogRecordKind::Stdout(_) | LogRecordKind::Stderr(_)
        )
    }
}

/// Build a record from an `(OutputKind, line)` pair. Shared by foreground
/// and background sinks so encoding stays consistent.
pub fn record_from(seq: u64, kind: OutputKind, line: impl Into<String>) -> LogRecord {
    match kind {
        OutputKind::Stdout => LogRecord::stdout(seq, line),
        OutputKind::Stderr => LogRecord::stderr(seq, line),
    }
}

/// Serialize a record to JSONL: compact JSON + trailing `\n`.
pub fn write_log_record<W: Write>(w: &mut W, record: &LogRecord) -> Result<()> {
    serde_json::to_writer(&mut *w, record)?;
    w.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdout_record_serializes_to_expected_jsonl_shape() {
        let r = LogRecord {
            seq: 7,
            ts: 1_700_000_000_000,
            kind: LogRecordKind::Stdout("hello world".into()),
        };
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(
            s,
            r#"{"seq":7,"ts":1700000000000,"kind":"stdout","data":"hello world"}"#
        );
    }

    #[test]
    fn stderr_record_serializes_with_kind_stderr() {
        let r = LogRecord {
            seq: 1,
            ts: 0,
            kind: LogRecordKind::Stderr("boom".into()),
        };
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(s, r#"{"seq":1,"ts":0,"kind":"stderr","data":"boom"}"#);
    }

    #[test]
    fn status_started_round_trips() {
        let r = LogRecord {
            seq: 0,
            ts: 0,
            kind: LogRecordKind::Status(StatusEvent::Started { pid: 4242 }),
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: LogRecord = serde_json::from_str(&s).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn status_finished_with_exit_code_round_trips() {
        let r = LogRecord {
            seq: 9,
            ts: 100,
            kind: LogRecordKind::Status(StatusEvent::Finished { exit_code: Some(0) }),
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: LogRecord = serde_json::from_str(&s).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn status_finished_without_exit_code_round_trips() {
        let r = LogRecord {
            seq: 10,
            ts: 200,
            kind: LogRecordKind::Status(StatusEvent::Finished { exit_code: None }),
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: LogRecord = serde_json::from_str(&s).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn is_output_distinguishes_stdout_stderr_from_status() {
        assert!(LogRecord::stdout(0, "x").is_output());
        assert!(LogRecord::stderr(1, "y").is_output());
        assert!(!LogRecord::status(2, StatusEvent::Finished { exit_code: Some(0) }).is_output());
    }

    #[test]
    fn write_log_record_appends_newline() {
        let r = LogRecord::stdout(0, "hi");
        let mut buf = Vec::new();
        write_log_record(&mut buf, &r).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.ends_with('\n'));
        assert!(s.starts_with(r#"{"seq":0"#));
        let parsed: LogRecord = serde_json::from_str(s.trim_end()).unwrap();
        assert_eq!(parsed, r);
    }

    #[test]
    fn record_from_maps_output_kind_to_record_kind() {
        let a = record_from(1, OutputKind::Stdout, "x");
        let b = record_from(2, OutputKind::Stderr, "y");
        assert!(matches!(a.kind, LogRecordKind::Stdout(ref s) if s == "x"));
        assert!(matches!(b.kind, LogRecordKind::Stderr(ref s) if s == "y"));
    }

    #[test]
    fn embedded_newlines_in_data_survive_jsonl_round_trip() {
        let r = LogRecord::stdout(0, "line one\nline two");
        let mut buf = Vec::new();
        write_log_record(&mut buf, &r).unwrap();
        // The JSON serialization should escape the inner newline as \n,
        // leaving exactly one literal \n at the very end as the JSONL
        // terminator.
        let s = String::from_utf8(buf).unwrap();
        let newline_count = s.bytes().filter(|&b| b == b'\n').count();
        assert_eq!(newline_count, 1, "raw JSONL: {s:?}");
        let parsed: LogRecord = serde_json::from_str(s.trim_end()).unwrap();
        assert_eq!(parsed, r);
    }
}
