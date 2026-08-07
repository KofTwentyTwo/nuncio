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

impl LogRecord {
    /// Parses one log line. A line that is not valid JSON, is JSON that
    /// isn't an object, or is a JSON object that carries none of the
    /// recognized event fields (`level`, `timestamp`, `fields.message`)
    /// becomes an opaque `RAW` record with the original text preserved in
    /// `raw`, rather than an error or a silently blank record — a single
    /// malformed or unrecognized line must never abort the tail, and it
    /// must never be mistaken for "nothing happened".
    pub fn parse(line: &str) -> Self {
        let raw = || LogRecord {
            timestamp: String::new(),
            level: "RAW".to_string(),
            target: String::new(),
            request_id: None,
            message: String::new(),
            raw: line.to_string(),
        };

        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            return raw();
        };
        let Some(obj) = value.as_object() else {
            return raw();
        };

        let fields = obj.get("fields").and_then(|v| v.as_object());
        let recognized = obj.contains_key("level")
            || obj.contains_key("timestamp")
            || fields.is_some_and(|f| f.contains_key("message"));
        if !recognized {
            return raw();
        }

        let as_str = |v: Option<&serde_json::Value>| {
            v.and_then(|v| v.as_str()).unwrap_or_default().to_string()
        };

        LogRecord {
            timestamp: as_str(obj.get("timestamp")),
            level: as_str(obj.get("level")),
            target: as_str(obj.get("target")),
            request_id: fields
                .and_then(|f| f.get("request_id"))
                .and_then(|v| v.as_str())
                .map(str::to_string),
            message: fields.map(|f| as_str(f.get("message"))).unwrap_or_default(),
            raw: line.to_string(),
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
        self.poll_with(scan_log_dir)
    }

    /// `poll()`'s implementation, parameterized over how the directory is
    /// scanned so tests can inject a transient scan failure deterministically
    /// instead of relying on OS-specific permission errors.
    fn poll_with(&mut self, scan: impl Fn(&Path) -> DirScan) -> Vec<LogRecord> {
        let latest = match scan(&self.dir) {
            DirScan::Missing => {
                // The directory itself does not exist: per `docs/LOGGING.md`
                // the daemon degrades to stderr-only logging when it can't
                // create this directory, so this is a reportable "not
                // writing logs" state, not "no logs yet".
                self.availability = LogAvailability::DirectoryMissing;
                self.current_file_name = None;
                self.offset = 0;
                self.initialized = false;
                return Vec::new();
            }
            DirScan::ReadError => {
                // A transient failure to read an existing directory (a
                // permission blip, a race with the daemon renaming a
                // rotated file, ...). Hold position and retry on the next
                // poll rather than treating this as "start over" — resetting
                // here would make a reappearing file look brand new and
                // silently discard whatever was written during the outage.
                return Vec::new();
            }
            DirScan::Found(None) => {
                // Directory exists but no log file has been written yet.
                self.availability = LogAvailability::Ok;
                self.current_file_name = None;
                self.offset = 0;
                self.initialized = false;
                return Vec::new();
            }
            DirScan::Found(Some(name)) => name,
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

/// The outcome of trying to list a log directory, distinguishing "the
/// directory genuinely doesn't exist" from "reading it failed transiently" —
/// the two must not be handled the same way, since only the former is safe
/// to treat as a reset to the tailer's initial state.
enum DirScan {
    /// The directory does not exist (`io::ErrorKind::NotFound`).
    Missing,
    /// The directory could not be read for some other reason (permissions,
    /// a race with a concurrent rename, ...). Transient by assumption.
    ReadError,
    /// The directory was read successfully; carries the lexicographically
    /// greatest `nunciod.log.*` file name found, or `None` if there wasn't
    /// one yet.
    Found(Option<String>),
}

/// Lists `dir` and selects the lexicographically greatest `nunciod.log.*`
/// entry. ISO-8601 date suffixes sort lexicographically in chronological
/// order, so this is also the newest file.
fn scan_log_dir(dir: &Path) -> DirScan {
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            let latest = entries
                .filter_map(|entry| entry.ok())
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| name.starts_with(FILE_PREFIX))
                .max();
            DirScan::Found(latest)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => DirScan::Missing,
        Err(_) => DirScan::ReadError,
    }
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
    fn a_well_formed_but_unrelated_json_object_becomes_raw_not_blank() {
        // Legitimate JSON, but not the daemon's event shape: no `level`,
        // `timestamp`, or `fields.message`. This must read as visibly
        // opaque (RAW), never as a blank record that looks like "nothing
        // happened".
        let rec = LogRecord::parse(r#"{"foo":"bar"}"#);
        assert_eq!(rec.level, "RAW");
        assert_eq!(rec.raw, r#"{"foo":"bar"}"#);
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

    #[test]
    fn a_transient_directory_read_failure_does_not_discard_data() {
        use std::io::Write;

        let dir = tempfile::tempdir().expect("tempdir");
        let log_file = dir.path().join("nunciod.log.2026-08-07");
        let mut f = std::fs::File::create(&log_file).expect("create log file");
        writeln!(f, r#"{{"level":"INFO","fields":{{"message":"first"}}}}"#).expect("write");
        f.flush().expect("flush");

        let mut tailer = LogTailer::new(dir.path().to_path_buf());

        // First poll (real scan): establishes position at end-of-file.
        assert_eq!(tailer.poll(), Vec::new());

        // Content arrives while the directory is (simulated) transiently
        // unreadable — e.g. a permission blip or a race with the daemon
        // renaming a rotated file in. A poll during the outage must not
        // reset the tailer's position.
        writeln!(f, r#"{{"level":"WARN","fields":{{"message":"second"}}}}"#).expect("write");
        f.flush().expect("flush");
        writeln!(f, r#"{{"level":"ERROR","fields":{{"message":"third"}}}}"#).expect("write");
        f.flush().expect("flush");

        let during_outage = tailer.poll_with(|_dir| DirScan::ReadError);
        assert_eq!(during_outage, Vec::new());

        // Recovery: the next real poll must see everything written during
        // the outage, not just what arrives after recovery.
        let recovered = tailer.poll();
        let messages: Vec<&str> = recovered.iter().map(|r| r.message.as_str()).collect();
        assert_eq!(messages, vec!["second", "third"]);
    }
}
