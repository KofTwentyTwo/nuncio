//! Follows the daemon's rotating JSON log file and parses lines into records.
//!
//! `nunciod` writes daily-rotated log files named `nunciod.log.YYYY-MM-DD` in
//! its `logs/` directory (see `tracing_appender::rolling::daily`). JSON body
//! formatting is opt-in on the daemon side, so a plain-text file is an
//! expected, non-error state, not a parse failure. This module never depends
//! on the `nunciod` crate; the log directory is supplied by the caller.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// A single tailed log line, either successfully parsed as the daemon's JSON
/// log shape or preserved verbatim as an opaque `RAW` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord {
    pub timestamp: String,
    pub level: String,
    pub target: String,
    pub request_id: Option<String>,
    pub message: String,
    pub raw: String,
}

/// The shape `tracing_subscriber`'s JSON formatter emits per event. Fields we
/// don't recognize are ignored rather than rejected.
#[derive(Debug, Deserialize)]
struct JsonEvent {
    #[serde(default)]
    timestamp: String,
    #[serde(default)]
    level: String,
    #[serde(default)]
    target: String,
    #[serde(default)]
    fields: JsonEventFields,
}

#[derive(Debug, Default, Deserialize)]
struct JsonEventFields {
    #[serde(default)]
    message: String,
    #[serde(default)]
    request_id: Option<String>,
}

impl LogRecord {
    /// Parses one log line. A line that is not valid JSON, or is JSON that
    /// doesn't match the expected event shape, becomes an opaque `RAW`
    /// record rather than an error — a single malformed line must never
    /// abort the tail.
    pub fn parse(line: &str) -> Self {
        match serde_json::from_str::<JsonEvent>(line) {
            Ok(event) => LogRecord {
                timestamp: event.timestamp,
                level: event.level,
                target: event.target,
                request_id: event.fields.request_id,
                message: event.fields.message,
                raw: line.to_string(),
            },
            Err(_) => LogRecord {
                timestamp: String::new(),
                level: "RAW".to_string(),
                target: String::new(),
                request_id: None,
                message: String::new(),
                raw: line.to_string(),
            },
        }
    }
}

/// Whether the tailer currently has a usable, JSON-formatted log to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogAvailability {
    /// The log directory exists and lines parse as the expected JSON shape.
    Ok,
    /// The log directory does not exist. Per `docs/LOGGING.md`, the daemon
    /// degrades to stderr-only logging when it cannot create this
    /// directory, so this means "not writing logs", not "no logs yet".
    DirectoryMissing,
    /// The log directory exists but lines are not the expected JSON shape
    /// (the daemon was started without `NUNCIO_LOG_FORMAT=json`).
    NotJson,
}

/// Follows the lexicographically greatest `nunciod.log.*` file in a
/// directory, returning newly appended, parsed records on each poll.
pub struct LogTailer {
    dir: PathBuf,
    current_file_name: Option<String>,
    offset: u64,
    availability: LogAvailability,
    initialized: bool,
}

const FILE_PREFIX: &str = "nunciod.log.";

impl LogTailer {
    /// Creates a tailer over `dir`. No file is opened yet; the first
    /// `poll()` establishes the target file and seeks to its end so
    /// pre-existing history is not replayed.
    pub fn new(dir: PathBuf) -> Self {
        LogTailer {
            dir,
            current_file_name: None,
            offset: 0,
            availability: LogAvailability::DirectoryMissing,
            initialized: false,
        }
    }

    /// The tailer's current view of whether the daemon appears to be
    /// writing a readable JSON log.
    pub fn availability(&self) -> LogAvailability {
        self.availability
    }

    /// Reads and parses any lines appended since the last poll. Re-selects
    /// the target file on every call (the newest dated file wins), so a
    /// midnight rotation is followed rather than leaving the tailer pinned
    /// to yesterday's file.
    pub fn poll(&mut self) -> Vec<LogRecord> {
        let latest = match latest_log_file(&self.dir) {
            Some(name) => name,
            None => {
                self.availability = if self.dir.is_dir() {
                    LogAvailability::Ok
                } else {
                    LogAvailability::DirectoryMissing
                };
                self.current_file_name = None;
                self.offset = 0;
                self.initialized = false;
                return Vec::new();
            }
        };

        let rotated = self.current_file_name.as_deref() != Some(latest.as_str());

        let path = self.dir.join(&latest);
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(_) => {
                self.availability = LogAvailability::DirectoryMissing;
                return Vec::new();
            }
        };

        if rotated {
            self.current_file_name = Some(latest.clone());
            if !self.initialized {
                // Seek to end on the very first open: the tailer follows
                // new output, it does not replay the day's history.
                self.offset = file.metadata().map(|m| m.len()).unwrap_or(0);
                self.initialized = true;
                self.availability = LogAvailability::Ok;
                return Vec::new();
            }
            // Rotation to a newer dated file after the tailer is already
            // running: read it from the start, since none of it has been
            // seen yet.
            self.offset = 0;
        }

        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }

        let mut buf = Vec::new();
        if file.read_to_end(&mut buf).is_err() {
            return Vec::new();
        }
        self.offset += buf.len() as u64;

        let mut saw_non_json = false;
        let records: Vec<LogRecord> = BufReader::new(buf.as_slice())
            .lines()
            .map_while(Result::ok)
            .filter(|line| !line.is_empty())
            .map(|line| {
                let rec = LogRecord::parse(&line);
                if rec.level == "RAW" {
                    saw_non_json = true;
                }
                rec
            })
            .collect();

        self.availability = if saw_non_json {
            LogAvailability::NotJson
        } else {
            LogAvailability::Ok
        };

        records
    }
}

/// Returns the file name (not full path) of the lexicographically greatest
/// `nunciod.log.*` entry in `dir`, or `None` if the directory is missing or
/// has no matching files. ISO-8601 date suffixes sort lexicographically in
/// chronological order.
fn latest_log_file(dir: &Path) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with(FILE_PREFIX))
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_json_line_into_a_record() {
        let line = r#"{"timestamp":"2026-08-07T12:00:00Z","level":"INFO","target":"nunciod","fields":{"message":"started","request_id":"abc"}}"#;
        let rec = LogRecord::parse(line);
        assert_eq!(rec.level, "INFO");
        assert_eq!(rec.request_id.as_deref(), Some("abc"));
        assert_eq!(rec.message, "started");
    }

    #[test]
    fn a_non_json_line_becomes_an_opaque_record_not_an_error() {
        let rec = LogRecord::parse("2026-08-07 plain text log line");
        assert_eq!(rec.level, "RAW");
        assert_eq!(rec.raw, "2026-08-07 plain text log line");
    }

    #[test]
    fn a_malformed_json_line_mid_stream_does_not_abort_the_tail() {
        let lines = ["{\"level\":\"INFO\"}", "{not json", "{\"level\":\"WARN\"}"];
        let recs: Vec<_> = lines.iter().map(|l| LogRecord::parse(l)).collect();
        assert_eq!(recs.len(), 3);
        assert_eq!(recs[1].level, "RAW");
    }

    #[test]
    fn rotating_to_a_new_dated_file_continues_the_tail() {
        use std::io::Write;

        let dir = tempfile::tempdir().expect("tempdir");

        let day_one = dir.path().join("nunciod.log.2026-08-07");
        let mut f1 = std::fs::File::create(&day_one).expect("create day one");
        writeln!(f1, r#"{{"level":"INFO","fields":{{"message":"first"}}}}"#).expect("write");
        f1.flush().expect("flush");

        let mut tailer = LogTailer::new(dir.path().to_path_buf());

        // First poll establishes the position; a tailer seeks to end on open, so
        // pre-existing history is deliberately NOT replayed.
        let _ = tailer.poll();

        writeln!(f1, r#"{{"level":"WARN","fields":{{"message":"second"}}}}"#).expect("write");
        f1.flush().expect("flush");
        let batch = tailer.poll();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].message, "second");

        // Rotation: a newer dated file appears. The tailer must switch to it and
        // read from its start, not stay pinned to the previous day.
        let day_two = dir.path().join("nunciod.log.2026-08-08");
        let mut f2 = std::fs::File::create(&day_two).expect("create day two");
        writeln!(f2, r#"{{"level":"ERROR","fields":{{"message":"third"}}}}"#).expect("write");
        f2.flush().expect("flush");

        let batch = tailer.poll();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].message, "third");
        assert_eq!(batch[0].level, "ERROR");
    }

    #[test]
    fn a_missing_log_directory_is_reported_not_silently_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tailer = LogTailer::new(dir.path().join("does-not-exist"));
        assert_eq!(tailer.poll(), Vec::new());
        assert_eq!(tailer.availability(), LogAvailability::DirectoryMissing);
    }
}
