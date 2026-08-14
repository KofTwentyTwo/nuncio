//! Shared in-test `tracing` capture helper.
//!
//! Records every span (with its fields) and every event (with its level,
//! fields, and message) into an in-memory buffer, so tests can assert on what
//! the code logged -- including the negative assertion that a secret never
//! appears in any recorded field. Fully offline: no I/O.
//!
//! The subscriber is installed **once, globally**, and events are routed to a
//! per-thread recorder. That split is deliberate and load-bearing: a
//! thread-local subscriber cannot work here, because `tracing` caches each
//! callsite's `Interest` process-wide on first use. See
//! [`install_global_capture`].

#![cfg(test)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use tracing::field::{Field, Visit};
use tracing::span::Attributes;
use tracing::{Event, Id, Level, Subscriber};
use tracing_subscriber::layer::Context as LayerContext;
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;

/// A captured span: its name and the string-rendered fields it opened with.
#[derive(Debug, Clone)]
pub(crate) struct RecordedSpan {
    #[allow(dead_code)]
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

thread_local! {
    /// The recorder collecting telemetry for the test running on THIS thread,
    /// or `None` on any thread not currently inside [`with_recorder`].
    ///
    /// Routing per thread is what keeps the single global subscriber from
    /// letting concurrent tests read each other's telemetry.
    static ACTIVE_RECORDER: RefCell<Option<Arc<Recorder>>> = const { RefCell::new(None) };
}

/// Run `f` against the recorder installed for this thread, if any.
fn with_active<R>(f: impl FnOnce(&Recorder) -> R) -> Option<R> {
    ACTIVE_RECORDER.with(|slot| slot.borrow().as_ref().map(|rec| f(rec)))
}

struct CaptureLayer;

impl<S> tracing_subscriber::Layer<S> for CaptureLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, _id: &Id, _ctx: LayerContext<'_, S>) {
        with_active(|rec| {
            let mut visitor = FieldVisitor(HashMap::new());
            attrs.record(&mut visitor);
            rec.spans
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(RecordedSpan {
                    name: attrs.metadata().name().to_string(),
                    fields: visitor.0,
                });
        });
    }

    fn on_event(&self, event: &Event<'_>, _ctx: LayerContext<'_, S>) {
        with_active(|rec| {
            let mut visitor = FieldVisitor(HashMap::new());
            event.record(&mut visitor);
            rec.events
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(RecordedEvent {
                    level: *event.metadata().level(),
                    fields: visitor.0,
                });
        });
    }
}

/// Install the capturing subscriber as the process-wide default, exactly once.
///
/// It MUST be global rather than thread-local, and this is the whole reason the
/// helper is shaped this way. `tracing` resolves each callsite's `Interest`
/// **once for the process** and caches it; a callsite first executed on a thread
/// with no subscriber caches `Interest::never()` and is then skipped forever --
/// before any later thread-local subscriber is ever consulted. Under a parallel
/// test harness, whichever thread reaches a callsite first therefore decides
/// whether any other test can ever capture it, which made every log-capture
/// assertion in this crate a coin flip.
///
/// A single global subscriber removes the race: every callsite registers against
/// a real subscriber, so interest is always enabled, and per-thread routing
/// (`ACTIVE_RECORDER`) decides what is actually recorded.
fn install_global_capture() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let subscriber = tracing_subscriber::registry().with(CaptureLayer);
        // A global default may already be set by another component of the same
        // test binary; capturing still works through the thread-local routing
        // above, so a lost race here is not fatal.
        let _ = tracing::subscriber::set_global_default(subscriber);
        // Callsites executed before this point resolved their interest against
        // the no-op dispatcher and cached "never". Recompute so they can be
        // captured from here on. Done once, at install -- not per test.
        tracing::callsite::rebuild_interest_cache();
    });
}

/// Resets this thread's recorder slot on drop, so a panicking test cannot leak
/// its recorder into whatever the harness runs next on the same thread.
struct RecorderGuard;

impl Drop for RecorderGuard {
    fn drop(&mut self) {
        ACTIVE_RECORDER.with(|slot| *slot.borrow_mut() = None);
    }
}

/// Runs `f` with telemetry from THIS thread captured, returning the recorder
/// alongside `f`'s result.
///
/// Only this thread's telemetry is recorded, so any async work driven inside
/// `f` must run on the calling thread (e.g. a current-thread runtime's
/// `block_on`) to be captured -- unchanged from the previous thread-local
/// implementation. What changed is that the subscriber itself is now global;
/// see [`install_global_capture`] for why that is load-bearing.
pub(crate) fn with_recorder<T>(f: impl FnOnce() -> T) -> (Arc<Recorder>, T) {
    install_global_capture();
    let recorder = Arc::new(Recorder::default());
    ACTIVE_RECORDER.with(|slot| *slot.borrow_mut() = Some(recorder.clone()));
    let _guard = RecorderGuard;
    let value = f();
    (recorder, value)
}
