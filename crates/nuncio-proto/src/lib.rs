//! `nuncio-proto`: versioned gRPC/protobuf API definitions for the Nuncio
//! daemon (`nunciod`).
//!
//! This crate holds the `.proto` sources under `proto/nuncio/v1/` and the
//! `tonic`-generated client + server stubs for the `nuncio.v1` package. It is
//! a compiling skeleton with a single minimal `System.GetStatus` RPC. Real
//! server wiring, auth, and business logic are intentionally out of scope
//! here and are added incrementally as the daemon grows.
//!
//! Consumers (e.g. `nunciod`, `nuncio-cli`, and future client repos) should
//! depend on this crate and use the generated client/server types under
//! [`v1`] rather than re-generating protobuf code themselves.
//!
//! This crate is also the single source of truth for the gRPC loopback
//! address defaults ([`addr`]) and the shared authenticated client
//! connection helper ([`client::connect_system`]), so that thin
//! presentation-shell clients (e.g. `nuncio-cli`) can talk to the `nunciod`
//! daemon's `nuncio.v1.System` API without depending on the `nunciod`
//! binary crate itself.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod addr;
pub mod client;
pub mod errors;

pub use addr::{grpc_addr_from_env, DEFAULT_GRPC_ADDR, GRPC_ADDR_ENV_VAR};

/// Generated code for the `nuncio.v1` protobuf package.
///
/// The generated `prost`/`tonic` code does not conform to this workspace's
/// lint policy (see root `Cargo.toml` `[workspace.lints]`), so this module
/// is exempted from clippy's pedantic/all groups and other lints that only
/// make sense for hand-written code. Hand-written code elsewhere in this
/// crate remains subject to the full workspace lint set via
/// `[lints] workspace = true` in `Cargo.toml`.
#[allow(
    clippy::all,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    unused_qualifications
)]
pub mod v1 {
    tonic::include_proto!("nuncio.v1");
}

#[cfg(test)]
mod tests {
    use super::v1::{GetStatusRequest, GetStatusResponse};

    #[test]
    fn get_status_response_constructs_and_reads_back_fields() {
        let response = GetStatusResponse {
            engine_status: "ok".to_string(),
            version: "0.1.0".to_string(),
        };

        assert_eq!(response.engine_status, "ok");
        assert_eq!(response.version, "0.1.0");
    }

    #[test]
    fn get_status_request_is_constructible() {
        // GetStatusRequest currently has no fields; this proves the
        // generated type exists and is usable (e.g. as a tonic::Request
        // payload) even though it carries no data yet.
        let _request = GetStatusRequest {};
    }
}

/// Contract-stability check for the published `nuncio.v1` wire format.
///
/// `build.rs` emits a `FileDescriptorSet` for `proto/nuncio/v1/nuncio.proto`
/// to `$OUT_DIR` on every build (with `SourceCodeInfo` stripped, so pure doc-
/// comment edits don't perturb it). This module compares that freshly
/// compiled descriptor, byte for byte, against the one committed at
/// `proto/nuncio/v1/descriptor.bin` -- the crate's published contract, which
/// future native client repos (Swift, C#, TypeScript) codegen from.
///
/// A mismatch means the `.proto` sources changed the wire contract: an added,
/// removed, or renumbered field; a changed field type; a changed RPC
/// signature; etc. See the assertion message below for how to intentionally
/// update the golden once such a change is reviewed and accepted.
#[cfg(test)]
mod contract_stability {
    /// The published, committed `nuncio.v1` `FileDescriptorSet`.
    const GOLDEN_DESCRIPTOR: &[u8] = include_bytes!("../proto/nuncio/v1/descriptor.bin");

    /// The `FileDescriptorSet` compiled from the current `.proto` sources by
    /// this very build, written by `build.rs` via
    /// `file_descriptor_set_path`.
    const FRESH_DESCRIPTOR: &[u8] =
        include_bytes!(concat!(env!("OUT_DIR"), "/nuncio_v1_descriptor.bin"));

    #[test]
    fn descriptor_matches_committed_golden() {
        assert_eq!(
            FRESH_DESCRIPTOR,
            GOLDEN_DESCRIPTOR,
            "the `nuncio.v1` FileDescriptorSet compiled from \
             `proto/nuncio/v1/nuncio.proto` no longer matches the committed \
             golden at `crates/nuncio-proto/proto/nuncio/v1/descriptor.bin`. \
             This means the wire contract changed (an added/removed/\
             renumbered field, a changed field type, or a changed RPC \
             signature). If this change is intentional -- either an \
             additive, backward-compatible change within `nuncio.v1`, or a \
             deliberate breaking change that has ALSO been moved to a new \
             `nuncio.v2` package -- update the golden: rebuild with \
             `cargo build -p nuncio-proto`, then overwrite the committed \
             `descriptor.bin` with the freshly generated descriptor at:\n  \
             {}\n\nThen re-run `cargo test -p nuncio-proto` and commit the \
             updated `descriptor.bin` alongside the `.proto` change.",
            concat!(env!("OUT_DIR"), "/nuncio_v1_descriptor.bin")
        );
    }
}
