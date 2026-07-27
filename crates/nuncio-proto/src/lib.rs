//! `nuncio-proto`: versioned gRPC/protobuf API definitions for the Nuncio
//! daemon (`nunciod`).
//!
//! This crate holds the `.proto` sources under `proto/nuncio/v1/` and the
//! `tonic`-generated client + server stubs for the `nuncio.v1` package. It is
//! the foundation laid by backlog story 1.A.1 (GH-148): a compiling skeleton
//! with a single minimal `System.GetStatus` RPC. Real server wiring, auth,
//! and business logic are intentionally out of scope here and land in later
//! stories (1.A.2 / 1.A.3).
//!
//! Consumers (e.g. `nunciod`, `nuncio-cli`, and future client repos) should
//! depend on this crate and use the generated client/server types under
//! [`v1`] rather than re-generating protobuf code themselves.

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
