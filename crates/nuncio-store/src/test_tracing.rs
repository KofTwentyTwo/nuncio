//! Shared in-test `tracing` capture helper.
//!
//! Captures the process's formatted `tracing` output (the same text a real
//! log line would carry, level tag and all) into an in-memory buffer, so
//! tests can assert on substrings such as a table/column name or an `"INFO"`/
//! `"WARN"` level tag. Fully offline: no I/O beyond the in-memory buffer.
//!
//! The subscriber is installed **once, globally**, and each thread's output is
//! routed to that thread's own buffer. That split is deliberate and
//! load-bearing: a thread-local subscriber cannot work here, because
//! `tracing` caches each callsite's `Interest` process-wide on first use. See
//! [`install_global_capture`].

#![cfg(test)]

use std::cell::RefCell;
use std::io;
use std::sync::{Arc, Mutex, OnceLock};

use tracing_subscriber::fmt::MakeWriter;

thread_local! {
    /// The buffer collecting formatted log output for the test running on
    /// THIS thread, or `None` on any thread not currently inside
    /// [`capture_logs`].
    ///
    /// Routing per thread is what keeps the single global subscriber from
    /// letting concurrent tests read each other's telemetry.
    static ACTIVE_BUFFER: RefCell<Option<Arc<Mutex<Vec<u8>>>>> = const { RefCell::new(None) };
}

/// A `Write`/`MakeWriter` that appends into whichever buffer is active for
/// the calling thread, discarding the write if no test on this thread is
/// currently capturing.
#[derive(Clone, Default)]
struct ThreadRoutedWriter;

impl io::Write for ThreadRoutedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        ACTIVE_BUFFER.with(|slot| {
            if let Some(buffer) = slot.borrow().as_ref() {
                buffer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend_from_slice(buf);
            }
        });
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for ThreadRoutedWriter {
    type Writer = ThreadRoutedWriter;
    fn make_writer(&'a self) -> Self::Writer {
        ThreadRoutedWriter
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
/// (`ACTIVE_BUFFER`) decides what is actually recorded.
fn install_global_capture() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let subscriber = tracing_subscriber::fmt()
            .with_writer(ThreadRoutedWriter)
            .with_ansi(false)
            .finish();
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

/// Resets this thread's buffer slot on drop, so a panicking test cannot leak
/// its buffer into whatever the harness runs next on the same thread.
struct BufferGuard;

impl Drop for BufferGuard {
    fn drop(&mut self) {
        ACTIVE_BUFFER.with(|slot| *slot.borrow_mut() = None);
    }
}

/// A capture in progress on the current thread. Held across the code under
/// test -- including any `.await` points, as long as the test stays on a
/// single OS thread (e.g. the default current-thread `#[tokio::test]`
/// runtime) -- then queried for the formatted text captured so far.
pub(crate) struct CapturedLogs {
    buffer: Arc<Mutex<Vec<u8>>>,
    _guard: BufferGuard,
}

impl CapturedLogs {
    /// A snapshot of everything captured on this thread so far, as the
    /// formatted text a real log line would carry (level tag included).
    pub fn text(&self) -> String {
        let bytes = self
            .buffer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        String::from_utf8(bytes)
            .unwrap_or_else(|e| String::from_utf8_lossy(&e.into_bytes()).into_owned())
    }
}

/// Start capturing this thread's formatted `tracing` output.
///
/// Only this thread's output is captured, so any async work driven while the
/// returned [`CapturedLogs`] is alive must run on the calling thread (e.g. a
/// current-thread runtime's `block_on`) to be captured -- the subscriber
/// itself is global, but the destination buffer is per-thread; see
/// [`install_global_capture`] for why that split is load-bearing.
pub(crate) fn capture_logs() -> CapturedLogs {
    install_global_capture();
    let buffer = Arc::new(Mutex::new(Vec::new()));
    ACTIVE_BUFFER.with(|slot| *slot.borrow_mut() = Some(buffer.clone()));
    CapturedLogs {
        buffer,
        _guard: BufferGuard,
    }
}
