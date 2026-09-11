#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[path = "support/backup_system.rs"]
mod backup_system;
#[path = "support/imap_effects.rs"]
mod imap_effects;
mod imap_read_system;
mod imap_write_system;
#[path = "support/smtp_resolution_evidence.rs"]
mod smtp_resolution_evidence;
#[allow(dead_code)]
mod support;

use nuncio_proto::v2;
use nuncio_test_support::{google::Seed, imap::MockMailPlus, TestError};
use serde_json::json;
use support::system::SystemHarness;

async fn configuration(
    mock: &MockMailPlus,
    start_tls: bool,
    user: &str,
) -> Result<v2::ImapAccountConfig, TestError> {
    Ok(v2::ImapAccountConfig {
        address: user.into(),
        imap: Some(v2::MailEndpoint {
            host: "127.0.0.1".into(),
            port: u32::from(if start_tls {
                mock.ready.ports.imap
            } else {
                mock.ready.ports.imaps
            }),
            tls: if start_tls {
                v2::MailTls::StartTls
            } else {
                v2::MailTls::Implicit
            }
            .into(),
            username: user.into(),
        }),
        smtp: Some(v2::MailEndpoint {
            host: "127.0.0.1".into(),
            port: u32::from(if start_tls {
                mock.ready.ports.smtp
            } else {
                mock.ready.ports.smtps
            }),
            tls: if start_tls {
                v2::MailTls::StartTls
            } else {
                v2::MailTls::Implicit
            }
            .into(),
            username: user.into(),
        }),
        sent_policy: v2::SentPolicy::ClientAppend.into(),
        sent_folder: "Sent".into(),
        archive_folder: Some("Archive".into()),
        trash_folder: Some("Trash".into()),
        trusted_ca_pem: Some(tokio::fs::read_to_string(&mock.ready.ca_file).await?),
    })
}
async fn request(
    mock: &MockMailPlus,
    tls: bool,
    user: &str,
) -> Result<v2::ConnectImapRequest, TestError> {
    let password = mock.credential(user)?;
    Ok(v2::ConnectImapRequest {
        config: Some(configuration(mock, tls, user).await?),
        credentials: Some(v2::ImapCredentials {
            imap_password: password.clone(),
            smtp_password: password,
        }),
        account_id: None,
    })
}

#[tokio::test]
async fn imap_account_authenticates_both_tls_servers_and_reopens_without_sending(
) -> Result<(), TestError> {
    let mut mock = MockMailPlus::start(&["MOVE", "CONDSTORE", "QRESYNC"]).await?;
    let mut harness = SystemHarness::start(Seed::TwoAccounts).await?;
    assert_eq!(
        harness
            .anonymous_accounts()
            .connect_imap(request(&mock, true, "alpha@example.test").await?)
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    let result = harness
        .accounts()
        .connect_imap(request(&mock, true, "alpha@example.test").await?)
        .await;
    if result.is_err() {
        eprintln!(
            "Provider observation: {}",
            mock.control(json!({"command":"snapshot"})).await?
        );
    }
    let result = result?.into_inner();
    let alpha = result.account.ok_or("missing account")?;
    assert_eq!(
        (alpha.provider.as_str(), alpha.state.as_str()),
        ("imap", "connected")
    );
    let caps = result.capabilities.ok_or("missing capabilities")?;
    assert!(!caps.move_messages && !caps.condstore && !caps.qresync);
    assert!(caps.uidplus && caps.idle);
    assert!(!result.credential_cleanup_pending);
    let beta = harness
        .accounts()
        .connect_imap(request(&mock, false, "beta@example.test").await?)
        .await?
        .into_inner()
        .account
        .ok_or("missing beta")?;
    assert_ne!(alpha.id, beta.id);
    for account in [&alpha, &beta] {
        assert_eq!(
            harness
                .accounts()
                .check_account(v2::AccountRequest {
                    account_id: account.id.clone()
                })
                .await?
                .into_inner()
                .state,
            "connected"
        );
    }
    let mut mismatch = request(&mock, true, "beta@example.test").await?;
    mismatch.account_id = Some(alpha.id.clone());
    assert_eq!(
        harness
            .accounts()
            .connect_imap(mismatch)
            .await
            .unwrap_err()
            .code(),
        tonic::Code::FailedPrecondition
    );
    harness.shutdown().await?;
    harness.restart().await?;
    assert_eq!(
        harness
            .accounts()
            .list_accounts(v2::ListAccountsRequest {})
            .await?
            .into_inner()
            .accounts
            .len(),
        2
    );
    assert_eq!(
        harness
            .accounts()
            .check_account(v2::AccountRequest {
                account_id: alpha.id.clone()
            })
            .await?
            .into_inner()
            .id,
        alpha.id
    );
    let local = harness
        .accounts()
        .get_imap_config(v2::AccountRequest {
            account_id: alpha.id.clone(),
        })
        .await?
        .into_inner();
    assert_eq!(local.config.unwrap().imap.unwrap().host, "127.0.0.1");
    for transport in ["starttls", "tls"] {
        assert_eq!(
            mock.control(json!({"command":"smtp","transport":transport}))
                .await?["total"],
            0
        );
    }
    let snapshot = mock.control(json!({"command":"snapshot"})).await?;
    assert_eq!(snapshot["smtp_deliveries"], json!([]));
    harness
        .accounts()
        .disconnect_account(v2::AccountRequest {
            account_id: alpha.id.clone(),
        })
        .await?;
    assert_eq!(
        harness
            .accounts()
            .check_account(v2::AccountRequest {
                account_id: alpha.id
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    harness.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn account_setup_rejects_bad_trust_and_credentials_without_persisting_or_sending(
) -> Result<(), TestError> {
    let mut mock = MockMailPlus::start(&[]).await?;
    let mut harness = SystemHarness::start(Seed::TwoAccounts).await?;
    for starttls in [false, true] {
        let mut untrusted = request(&mock, starttls, "alpha@example.test").await?;
        untrusted.config.as_mut().unwrap().trusted_ca_pem = None;
        assert_eq!(
            harness
                .accounts()
                .connect_imap(untrusted)
                .await
                .unwrap_err()
                .code(),
            tonic::Code::FailedPrecondition
        );
    }
    let snapshot = mock.control(json!({"command":"snapshot"})).await?;
    assert!(snapshot["requests"].get("imap AUTHENTICATE").is_none());
    for smtp in [false, true] {
        let mut bad = request(&mock, true, "alpha@example.test").await?;
        let value = bad.credentials.as_mut().unwrap();
        if smtp {
            value.smtp_password = "synthetic-wrong-canary".into();
        } else {
            value.imap_password = "synthetic-wrong-canary".into();
        }
        let error = harness.accounts().connect_imap(bad).await.unwrap_err();
        assert_eq!(error.code(), tonic::Code::Unauthenticated);
        assert!(!error.message().contains("canary"));
    }
    assert!(harness
        .accounts()
        .list_accounts(v2::ListAccountsRequest {})
        .await?
        .into_inner()
        .accounts
        .is_empty());
    harness
        .secrets
        .fail_put
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        harness
            .accounts()
            .connect_imap(request(&mock, true, "alpha@example.test").await?)
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Internal
    );
    assert!(harness
        .accounts()
        .list_accounts(v2::ListAccountsRequest {})
        .await?
        .into_inner()
        .accounts
        .is_empty());
    let connected = harness
        .accounts()
        .connect_imap(request(&mock, true, "alpha@example.test").await?)
        .await?
        .into_inner()
        .account
        .unwrap();
    for (protocol, command) in [("imap", "AUTHENTICATE"), ("smtp", "AUTH")] {
        mock.control(json!({"command":"inject","name":protocol,"protocol":protocol,"verb":command,"phase":"before","action":"reject"})).await?;
        let result = harness
            .accounts()
            .check_account(v2::AccountRequest {
                account_id: connected.id.clone(),
            })
            .await
            .unwrap_err();
        assert_eq!(
            result.code(),
            tonic::Code::Unavailable,
            "temporary rejection must preserve saved credentials"
        );
        assert_eq!(
            harness
                .accounts()
                .list_accounts(v2::ListAccountsRequest {})
                .await?
                .into_inner()
                .accounts[0]
                .state,
            "connected"
        );
        assert_eq!(
            harness
                .accounts()
                .check_account(v2::AccountRequest {
                    account_id: connected.id.clone()
                })
                .await?
                .into_inner()
                .id,
            connected.id
        );
    }
    assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 0);
    harness.shutdown().await?;
    harness.restart().await?;
    assert_eq!(
        harness
            .accounts()
            .check_account(v2::AccountRequest {
                account_id: connected.id
            })
            .await?
            .into_inner()
            .state,
        "connected"
    );
    harness.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}
