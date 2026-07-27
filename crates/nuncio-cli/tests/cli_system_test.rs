//! E2E System Test Suite for nuncio-cli.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_cli::{
    AccountSubcommand, CalSubcommand, Commands, FolderSubcommand, HeadlessRunner, MailSubcommand,
    PasswordArg, SystemSubcommand,
};
use nuncio_store::vault::SecretManager;
use serde_json::Value;
use std::sync::Arc;

#[tokio::test]
async fn system_test_cli_noun_verb_execution_matrix() {
    let runner = HeadlessRunner::ephemeral().await.expect("runner init");

    // 1. System status is a real gRPC client of the `nunciod` daemon, so it
    // is exercised separately below via `ephemeral_with` + `SecretManager::mock()`
    // rather than through this `ephemeral()`-constructed runner (which holds
    // the production `SecretManager` and must never touch the real OS
    // keyring in a test).

    // 1b. Banner output
    let out: String = runner.execute_command(&Commands::Banner, true).await;
    let json: Value = serde_json::from_str(&out).expect("valid json");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"]["name"], "Nuncio");

    // 1c. Licenses output
    let out: String = runner.execute_command(&Commands::Licenses, true).await;
    let json: Value = serde_json::from_str(&out).expect("valid json");
    assert_eq!(json["status"], "ok");

    // 2. Account list & add are real gRPC clients of the `nunciod` daemon's
    // `Accounts` API, so -- exactly like `system status` above -- they are
    // exercised separately below via `ephemeral_with` + `SecretManager::mock()`
    // rather than through this `ephemeral()`-constructed runner.

    // 3. Folder list & 4. Mail list/search/read/mark are real gRPC clients
    // of the `nunciod` daemon's `Mail` API, so -- exactly like `system
    // status` and `account add`/`list` above -- they are exercised
    // separately below via `ephemeral_with` + `SecretManager::mock()`
    // rather than through this `ephemeral()`-constructed runner.

    // 5. Calendar list & sync
    let out: String = runner
        .execute_command(
            &Commands::Cal {
                action: CalSubcommand::List,
            },
            true,
        )
        .await;
    let json: Value = serde_json::from_str(&out).expect("valid json");
    assert_eq!(json["status"], "ok");
}

/// `system status` is a real gRPC client of the `nunciod` daemon. With no
/// daemon reachable at an address nothing is listening on, it must report
/// a clear, honest error rather than fabricating a status.
#[tokio::test]
async fn system_status_reports_honest_error_when_daemon_unreachable() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral loopback port");
    let addr = listener.local_addr().expect("listener has local addr");
    drop(listener); // free the port; nothing is listening on it now

    let runner = HeadlessRunner::ephemeral_with(Arc::new(SecretManager::mock()), addr.to_string())
        .await
        .expect("runner init");

    let out: String = runner
        .execute_command(
            &Commands::System {
                action: SystemSubcommand::Status,
            },
            true,
        )
        .await;
    let json: Value = serde_json::from_str(&out).expect("valid json");
    assert_eq!(json["status"], "error");
    assert!(json["error"]
        .as_str()
        .expect("error message present")
        .contains("unreachable"));
}

/// `account add` / `account list` are real gRPC clients of the `nunciod`
/// daemon's `Accounts` API. With no daemon reachable, both must report a
/// clear, honest error rather than fabricating success or silently
/// falling back to any local state.
#[tokio::test]
async fn account_add_and_list_report_honest_errors_when_daemon_unreachable() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral loopback port");
    let addr = listener.local_addr().expect("listener has local addr");
    drop(listener); // free the port; nothing is listening on it now

    let runner = HeadlessRunner::ephemeral_with(Arc::new(SecretManager::mock()), addr.to_string())
        .await
        .expect("runner init");

    let list_out: String = runner
        .execute_command(
            &Commands::Account {
                action: AccountSubcommand::List,
            },
            true,
        )
        .await;
    let list_json: Value = serde_json::from_str(&list_out).expect("valid json");
    assert_eq!(list_json["status"], "error");
    assert!(list_json["error"]
        .as_str()
        .expect("error message present")
        .contains("unreachable"));

    let add_out: String = runner
        .execute_command(
            &Commands::Account {
                action: AccountSubcommand::Add {
                    email: "test@nuncio.mx".to_string(),
                    imap_host: "mail.nuncio.mx".to_string(),
                    imap_port: 993,
                    smtp_host: "mail.nuncio.mx".to_string(),
                    smtp_port: 465,
                    imap_mode: "implicit_tls".to_string(),
                    smtp_mode: "implicit_tls".to_string(),
                    password: PasswordArg("irrelevant-unreachable-daemon".to_string()),
                },
            },
            true,
        )
        .await;
    let add_json: Value = serde_json::from_str(&add_out).expect("valid json");
    assert_eq!(add_json["status"], "error");
    assert!(add_json["error"]
        .as_str()
        .expect("error message present")
        .contains("unreachable"));
    // The password must never leak into an error message either.
    assert!(!add_out.contains("irrelevant-unreachable-daemon"));
}

/// `folder list`, `mail list`, `mail read`, `mail search`, and `mail mark`
/// are real gRPC clients of the `nunciod` daemon's `Mail` API. With no
/// daemon reachable, all of them must report a clear, honest error rather
/// than fabricating an empty result.
#[tokio::test]
async fn mail_and_folder_report_honest_errors_when_daemon_unreachable() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral loopback port");
    let addr = listener.local_addr().expect("listener has local addr");
    drop(listener); // free the port; nothing is listening on it now

    let runner = HeadlessRunner::ephemeral_with(Arc::new(SecretManager::mock()), addr.to_string())
        .await
        .expect("runner init");

    let assert_honest_error = |out: String| {
        let json: Value = serde_json::from_str(&out).expect("valid json");
        assert_eq!(json["status"], "error");
        assert!(json["error"]
            .as_str()
            .expect("error message present")
            .contains("unreachable"));
    };

    assert_honest_error(
        runner
            .execute_command(
                &Commands::Folder {
                    action: FolderSubcommand::List,
                },
                true,
            )
            .await,
    );

    assert_honest_error(
        runner
            .execute_command(
                &Commands::Mail {
                    action: MailSubcommand::List {
                        folder: "INBOX".to_string(),
                    },
                },
                true,
            )
            .await,
    );

    assert_honest_error(
        runner
            .execute_command(
                &Commands::Mail {
                    action: MailSubcommand::Read {
                        id: "msg-1".to_string(),
                    },
                },
                true,
            )
            .await,
    );

    assert_honest_error(
        runner
            .execute_command(
                &Commands::Mail {
                    action: MailSubcommand::Search {
                        query: "Architecture".to_string(),
                    },
                },
                true,
            )
            .await,
    );

    assert_honest_error(
        runner
            .execute_command(
                &Commands::Mail {
                    action: MailSubcommand::Mark {
                        id: "msg-1".to_string(),
                        read: true,
                        unread: false,
                    },
                },
                true,
            )
            .await,
    );
}
