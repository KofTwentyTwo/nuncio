//! Shared in-test `tracing` capture helper.
//!
//! Installs a thread-local subscriber that records every span (with its
//! fields) and every event (with its level, fields, and message) into an
//! in-memory buffer, so tests can assert on what the code logged -- including
//! the negative assertion that a secret never appears in any recorded field.
//! Deterministic and fully offline: no global subscriber, no I/O.

#![cfg(test)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::span::Attributes;
use tracing::{Event, Id, Level, Subscriber};
use tracing_subscriber::layer::Context as LayerContext;
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;

/// A captured span: its name and the string-rendered fields it opened with.
#[derive(Debug, Clone)]
pub(crate) struct RecordedSpan {
    pub name: String,
    pub fields: HashMap<String, String>,
}

/// A captured event: its level and its string-rendered fields (including the
/// `message`).
#[derive(Debug, Clone)]
pub(crate) struct RecordedEvent {
    pub level: Level,
    pub fields: HashMap<String, String>,
}

impl RecordedEvent {
    /// The event's `message` field, or the empty string if it had none.
    pub fn message(&self) -> &str {
        self.fields.get("message").map(String::as_str).unwrap_or("")
    }
}

/// In-memory sink of everything the subscriber captured during a test.
#[derive(Debug, Default)]
pub(crate) struct Recorder {
    spans: Mutex<Vec<RecordedSpan>>,
    events: Mutex<Vec<RecordedEvent>>,
}

impl Recorder {
    /// A snapshot of every captured span.
    pub fn spans(&self) -> Vec<RecordedSpan> {
        self.spans.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// A snapshot of every captured event.
    pub fn events(&self) -> Vec<RecordedEvent> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Every field value recorded across all spans and events, so a test can
    /// assert that a secret never appears anywhere in the telemetry.
    pub fn all_field_values(&self) -> Vec<String> {
        let mut values = Vec::new();
        for span in self.spans().iter() {
            values.extend(span.fields.values().cloned());
        }
        for event in self.events().iter() {
            values.extend(event.fields.values().cloned());
        }
        values
    }
}

struct FieldVisitor(HashMap<String, String>);

impl Visit for FieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .entry(field.name().to_string())
            .or_insert_with(|| format!("{value:?}"));
    }
}

struct CaptureLayer(Arc<Recorder>);

impl<S> tracing_subscriber::Layer<S> for CaptureLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, _id: &Id, _ctx: LayerContext<'_, S>) {
        let mut visitor = FieldVisitor(HashMap::new());
        attrs.record(&mut visitor);
        self.0
            .spans
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(RecordedSpan {
                name: attrs.metadata().name().to_string(),
                fields: visitor.0,
            });
    }

    fn on_event(&self, event: &Event<'_>, _ctx: LayerContext<'_, S>) {
        let mut visitor = FieldVisitor(HashMap::new());
        event.record(&mut visitor);
        self.0
            .events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(RecordedEvent {
                level: *event.metadata().level(),
                fields: visitor.0,
            });
    }
}

/// Runs `f` with a thread-local capturing subscriber installed and returns the
/// recorder alongside `f`'s result. Because the subscriber is thread-local,
/// any async work driven inside `f` must run on the calling thread (e.g. a
/// current-thread runtime's `block_on`) for its telemetry to be captured.
pub(crate) fn with_recorder<T>(f: impl FnOnce() -> T) -> (Arc<Recorder>, T) {
    let recorder = Arc::new(Recorder::default());
    let subscriber = tracing_subscriber::registry().with(CaptureLayer(recorder.clone()));
    let value = tracing::subscriber::with_default(subscriber, f);
    (recorder, value)
}
