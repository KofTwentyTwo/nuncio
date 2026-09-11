#[test]
fn google_registration_request_debug_never_exposes_client_secrets() {
    let request = nuncio_proto::v2::BeginGoogleAuthRequest {
        client_id: "synthetic-client-id".into(),
        client_secret: Some("registration-secret-canary".into()),
        login_hint: Some("account-address-canary@example.test".into()),
        account_id: None,
    };
    let rendered = format!("{request:?}");
    assert_eq!(rendered, "BeginGoogleAuthRequest { [redacted] }");
}

#[test]
fn backup_requests_redact_the_recovery_passphrase_including_nested_stream_headers() {
    use nuncio_proto::v2;
    let secret = v2::RecoverySecret {
        passphrase: "recovery-passphrase-canary".into(),
    };
    assert_eq!(format!("{secret:?}"), "RecoverySecret([redacted])");
    let create = v2::CreateBackupRequest {
        secret: Some(secret.clone()),
    };
    let upload = v2::BackupUploadChunk {
        content: Some(v2::backup_upload_chunk::Content::Header(
            v2::BackupUploadHeader {
                secret: Some(secret),
                byte_length: 4096,
                sha256: "0".repeat(64),
                new_profile: Some("restored".into()),
            },
        )),
    };
    for debug in [format!("{create:?}"), format!("{upload:?}")] {
        assert!(!debug.contains("recovery-passphrase-canary"));
        assert!(debug.contains("[redacted]"));
    }
}
