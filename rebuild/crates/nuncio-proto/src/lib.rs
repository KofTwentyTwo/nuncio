pub mod v2 {
    tonic::include_proto!("nuncio.v2");
}
pub const DESCRIPTOR: &[u8] = tonic::include_file_descriptor_set!("nuncio_v2");
pub mod client;

impl std::fmt::Debug for v2::RecoverySecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecoverySecret([redacted])")
    }
}

impl std::fmt::Debug for v2::BeginGoogleAuthRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BeginGoogleAuthRequest { [redacted] }")
    }
}

impl std::fmt::Debug for v2::ImapCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ImapCredentials([redacted])")
    }
}
impl std::fmt::Debug for v2::ConnectImapRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConnectImapRequest([redacted])")
    }
}
