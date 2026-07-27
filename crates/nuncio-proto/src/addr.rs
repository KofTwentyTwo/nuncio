//! Shared loopback address configuration for the `nuncio.v1` gRPC API.
//!
//! This is the single source of truth for the daemon's gRPC bind address
//! defaults, consumed by both the server (`nunciod`) and any thin
//! presentation-shell client (e.g. `nuncio-cli`) that needs to dial the same
//! endpoint without depending on the `nunciod` binary crate.

/// Default loopback bind address for the `nuncio.v1` gRPC server, used when
/// the [`GRPC_ADDR_ENV_VAR`] environment variable is unset. Deliberately
/// distinct from the legacy JSON-RPC IPC server's default (`127.0.0.1:9422`).
pub const DEFAULT_GRPC_ADDR: &str = "127.0.0.1:9420";

/// Environment variable overriding the gRPC bind/connect address.
pub const GRPC_ADDR_ENV_VAR: &str = "NUNCIO_GRPC_ADDR";

/// Resolves the gRPC address from [`GRPC_ADDR_ENV_VAR`], falling back to
/// [`DEFAULT_GRPC_ADDR`] when unset. Used both by the server to pick a bind
/// address and by clients to pick a connect address, so they agree by
/// default without any extra configuration.
pub fn grpc_addr_from_env() -> String {
    std::env::var(GRPC_ADDR_ENV_VAR).unwrap_or_else(|_| DEFAULT_GRPC_ADDR.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grpc_addr_from_env_defaults_when_unset() {
        // Serialize access to the shared process environment variable so
        // this test cannot interleave with any other test touching it.
        static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());

        std::env::remove_var(GRPC_ADDR_ENV_VAR);
        assert_eq!(grpc_addr_from_env(), DEFAULT_GRPC_ADDR);

        std::env::set_var(GRPC_ADDR_ENV_VAR, "127.0.0.1:12345");
        assert_eq!(grpc_addr_from_env(), "127.0.0.1:12345");
        std::env::remove_var(GRPC_ADDR_ENV_VAR);
    }
}
