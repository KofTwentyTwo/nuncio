//! Nuncio Monitor: a development instrument for watching a running `nunciod`.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

// Not yet wired into `main`; the status poller and UI tasks consume this.
#[allow(dead_code)]
mod engine;
// Not yet wired into `main`; the engine controller and UI tasks consume this.
#[allow(dead_code)]
mod log_tail;
// Not yet wired into `main`; the app state and UI tasks consume this.
#[allow(dead_code)]
mod status;

fn main() {
    println!("nuncio-monitor");
}
