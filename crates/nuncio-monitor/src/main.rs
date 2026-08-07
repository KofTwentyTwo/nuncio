//! Nuncio Monitor: a development instrument for watching a running `nunciod`.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

// Not yet wired into `main`; the engine controller and UI tasks consume this.
#[allow(dead_code)]
mod log_tail;

fn main() {
    println!("nuncio-monitor");
}
