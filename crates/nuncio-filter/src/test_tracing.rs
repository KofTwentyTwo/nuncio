//! Shared in-test `tracing` capture helper.
//!
//! Records one formatted line per event (level plus `Debug`-rendered fields,
//! including the message) into an in-memory buffer, so tests can assert that
//! a level tag and specific fields appear on the SAME logged event. This
//! crate has no dependency on `tracing-subscriber`, so the capture is a
//! minimal, hand-rolled `tracing::Subscriber` rather than a `Layer` -- that
//! keeps this helper free of a new dependency. Fully offline: no I/O.
//!
//! The subscriber is installed **once, globally**, and events are routed to a
//! per-thread line buffer. That split is deliberate and load-bearing: a
//! thread-local subscriber cannot work here, because `tracing` caches each
//! callsite's `Interest` process-wide on first use. See
//! [`install_global_capture`].

#![cfg(test)]

use std::cell::RefCell;
use std::sync::{Arc, Mutex, OnceLock};

use tracing::field::{Field, Visit};
use tracing::span;

thread_local! {
    /// The line buffer collecting telemetry for the test running on THIS
    /// thread, or `None` on any thread not currently inside [`capture_logs`].
    ///
    /// Routing per thread is what keeps the single global subscriber from
    /// letting concurrent tests read each other's telemetry.
    static ACTIVE_LINES: RefCell<Option<Arc<Mutex<Vec<String>>>>> = const { RefCell::new(None) };
}

struct LineVisitor<'a>(&'a mut String);

impl Visit for LineVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!(" {}={:?}", field.name(), value));
    }
}

/// Minimal `tracing::Subscriber` that records a formatted line per event
/// (level plus fields) without pulling in `tracing-subscriber`'s registry
/// machinery. Spans are not tracked (`new_span` returns a constant id) since
/// no test in this crate asserts on span structure, only on event lines.
struct CaptureSubscriber;

impl tracing::Subscriber for CaptureSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(1)
    }

    fn record(&self, _span: &span::Id, _values: &span::Record<'_>) {}

    fn record_follows_from(&self, _span: &span::Id, _follows: &span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = format!("{}", event.metadata().level());
        let mut visitor = LineVisitor(&mut line);
        event.record(&mut visitor);
        ACTIVE_LINES.with(|slot| {
            if let Some(lines) = slot.borrow().as_ref() {
                lines.lock().unwrap_or_else(|e| e.into_inner()).push(line);
            }
        });
    }

    fn enter(&self, _span: &span::Id) {}

    fn exit(&self, _span: &span::Id) {}
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
/// (`ACTIVE_LINES`) decides what is actually recorded.
fn install_global_capture() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        // A global default may already be set by another component of the same
        // test binary; capturing still works through the thread-local routing
        // above, so a lost race here is not fatal.
        let _ = tracing::subscriber::set_global_default(CaptureSubscriber);
        // Callsites executed before this point resolved their interest against
        // the no-op dispatcher and cached "never". Recompute so they can be
        // captured from here on. Done once, at install -- not per test.
        tracing::callsite::rebuild_interest_cache();
    });
}

/// Resets this thread's line buffer slot on drop, so a panicking test cannot
/// leak its buffer into whatever the harness runs next on the same thread.
struct LinesGuard;

impl Drop for LinesGuard {
    fn drop(&mut self) {
        ACTIVE_LINES.with(|slot| *slot.borrow_mut() = None);
    }
}

/// A capture in progress on the current thread. Held across the code under
/// test -- including any `.await` points, as long as the test stays on a
/// single OS thread (e.g. the default current-thread `#[tokio::test]`
/// runtime) -- then queried for the lines captured so far.
pub(crate) struct CapturedLogs {
    lines: Arc<Mutex<Vec<String>>>,
    _guard: LinesGuard,
}

impl CapturedLogs {
    /// A snapshot of every formatted event line captured on this thread so
    /// far: level plus `Debug`-rendered fields (including `message`).
    pub fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Start capturing this thread's `tracing` events as formatted lines.
///
/// Only this thread's events are captured, so any async work driven while the
/// returned [`CapturedLogs`] is alive must run on the calling thread (e.g. a
/// current-thread runtime's `block_on`) to be captured -- the subscriber
/// itself is global, but the destination buffer is per-thread; see
/// [`install_global_capture`] for why that split is load-bearing.
pub(crate) fn capture_logs() -> CapturedLogs {
    install_global_capture();
    let lines = Arc::new(Mutex::new(Vec::new()));
    ACTIVE_LINES.with(|slot| *slot.borrow_mut() = Some(lines.clone()));
    CapturedLogs {
        lines,
        _guard: LinesGuard,
    }
}
