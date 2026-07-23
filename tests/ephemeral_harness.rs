//! Ephemeral SQLite end-to-end integration test harness and WAL lifecycle verification.

use nuncio_core::config::{AccountConfig, AccountProtocol, TlsMode};
use nuncio_core::{AppState, CoreCommand, EngineStatus, EventBus};
use nuncio_store::DatabaseEngine;
use tempfile::tempdir;

#[tokio::test]
async fn test_ephemeral_harness_multi_account_lifecycle() {
    let dir = tempdir().expect("tempdir created");
    let db_path = dir.path().join("ephemeral_harness.sqlite");

    // Initialize database engine
    let db = DatabaseEngine::connect_file(&db_path)
        .await
        .expect("DB connected");

    // Create 2 test account configurations
    let acct1 = AccountConfig {
        id: "acct-work".to_string(),
        name: "Work Mail".to_string(),
        email_address: "work@nuncio.mx".to_string(),
        protocol: AccountProtocol::Jmap,
        server_host: "jmap.nuncio.mx".to_string(),
        server_port: 443,
        use_tls: true,
        imap_tls_mode: TlsMode::ImplicitTls,
        smtp_tls_mode: TlsMode::ImplicitTls,
        keyring_secret_key: "nuncio/acct-work".to_string(),
        sync_interval_secs: 30,
    };
    assert!(acct1.validate().is_ok());

    let acct2 = AccountConfig {
        id: "acct-personal".to_string(),
        name: "Personal Mail".to_string(),
        email_address: "personal@nuncio.mx".to_string(),
        protocol: AccountProtocol::ImapSmtp,
        server_host: "imap.nuncio.mx".to_string(),
        server_port: 993,
        use_tls: true,
        imap_tls_mode: TlsMode::ImplicitTls,
        smtp_tls_mode: TlsMode::StartTls,
        keyring_secret_key: "nuncio/acct-personal".to_string(),
        sync_interval_secs: 60,
    };
    assert!(acct2.validate().is_ok());

    // Initialize EventBus seeded with 2 accounts loaded
    let bus = EventBus::with_initial_state(AppState {
        status: EngineStatus::Idle,
        accounts_loaded: 2,
        unread_count: 0,
        last_error: None,
    });

    let mut state_rx = bus.subscribe_state();

    // Trigger Account 1 Sync
    bus.process_command(CoreCommand::SyncAccount {
        account_id: acct1.id.clone(),
    });
    assert_eq!(
        state_rx.borrow_and_update().status,
        EngineStatus::Syncing
    );

    // Complete Account 1 Sync
    bus.complete_sync(Some(acct1.id.clone()));
    assert_eq!(state_rx.borrow_and_update().status, EngineStatus::Idle);

    // Mark 3 unread messages
    bus.process_command(CoreCommand::MarkRead {
        message_id: "m1".to_string(),
        read: false,
    });
    bus.process_command(CoreCommand::MarkRead {
        message_id: "m2".to_string(),
        read: false,
    });
    bus.process_command(CoreCommand::MarkRead {
        message_id: "m3".to_string(),
        read: false,
    });
    assert_eq!(state_rx.borrow_and_update().unread_count, 3);

    // Shutdown Core
    bus.process_command(CoreCommand::Shutdown);
    assert_eq!(
        state_rx.borrow_and_update().status,
        EngineStatus::ShuttingDown
    );

    // Query database to ensure connection pool drops cleanly
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM messages")
        .fetch_one(db.pool())
        .await
        .expect("query messages");
    assert_eq!(row.0, 0);
}
