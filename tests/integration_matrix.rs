//! Full offline end-to-end integration test matrix for Nuncio core services.

use nuncio_cal::{CalendarError, MockCalendarBackend};
use nuncio_cli::{Commands, HeadlessRunner};
use nuncio_core::model::{CalendarEvent, Email, Folder};
use nuncio_core::{CoreCommand, EngineStatus, EventBus};
use nuncio_mail::{MailBackend, MockMailBackend};
use nuncio_store::{DatabaseEngine, PayloadCipher, SearchEngine, SecretManager};

#[tokio::test]
async fn full_offline_integration_test_matrix() {
    // 1. Initialize EventBus and state snapshot
    let bus = EventBus::new();
    let mut state_rx = bus.subscribe_state();
    let mut event_rx = bus.subscribe_events();

    assert_eq!(bus.current_state().status, EngineStatus::Idle);

    // 2. Initialize ephemeral SQLite database engine with WAL pragmas & FTS5
    let (db, _dir) = DatabaseEngine::connect_ephemeral()
        .await
        .expect("ephemeral DB created");
    let search = SearchEngine::new(&db);
    search.setup_fts_tables().await.expect("FTS tables created");

    // 3. Insert and search message via FTS5 trigram index
    sqlx::query(
        "INSERT INTO messages (id, account_id, folder_id, subject, sender, recipient, received_at, read_flag, body_plain)
         VALUES ('msg-matrix-1', 'acct-1', 'inbox', 'Q3 Architecture Roadmap', 'alice@nuncio.mx', 'bob@nuncio.mx', 1700000000, 0, 'Discuss JMAP and SQLite WAL')",
    )
    .execute(db.pool())
    .await
    .expect("message inserted");

    let hits = search
        .search_messages("Architecture")
        .await
        .expect("FTS search succeeds");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "msg-matrix-1");
    assert_eq!(hits[0].title, "Q3 Architecture Roadmap");

    // 4. Test MockMailBackend protocol operations
    let mail_mock = MockMailBackend::new();
    mail_mock.add_folder(Folder {
        id: "inbox".to_string(),
        name: "Inbox".to_string(),
        total_messages: 1,
        unread_messages: 1,
    });
    let folders = mail_mock.sync_folders().await.expect("sync folders");
    assert_eq!(folders.len(), 1);

    // 5. Test MockCalendarBackend operations
    let cal_mock = MockCalendarBackend::new();
    cal_mock.add_event(CalendarEvent {
        id: "evt-matrix-1".to_string(),
        account_id: "acct-1".to_string(),
        calendar_id: "cal-work".to_string(),
        summary: "Integration Review".to_string(),
        start_time: 1700000000,
        end_time: 1700003600,
        rrule: None,
        location: Some("Online".to_string()),
    });

    let evts = cal_mock
        .list_events("cal-work", 1699999000, 1700004000)
        .expect("list events succeeds");
    assert_eq!(evts.len(), 1);

    // 6. Test SecretManager vault storage
    let vault = SecretManager::mock();
    vault
        .set_secret("nuncio/acct-1", "passphrase_123")
        .expect("set secret");
    assert_eq!(
        vault.get_secret("nuncio/acct-1").expect("get secret"),
        "passphrase_123"
    );

    // 7. Test PayloadCipher encryption
    let key = [99u8; 32];
    let payload = b"Confidential integration data payload";
    let encrypted = PayloadCipher::encrypt_bytes(&key, payload).expect("encrypt");
    let decrypted = PayloadCipher::decrypt_bytes(&key, &encrypted).expect("decrypt");
    assert_eq!(decrypted, payload);

    // 8. Test HeadlessRunner CLI pipeline
    let runner = HeadlessRunner::ephemeral().await.expect("runner init");
    let output_json = runner.execute_command(&Commands::Sync, true).await;
    assert!(output_json.contains(r#""status":"ok""#));

    // 9. Dispatch EventBus command and verify state update
    bus.process_command(CoreCommand::SyncAll);
    assert_eq!(
        state_rx.borrow_and_update().status,
        EngineStatus::Syncing
    );
    assert_eq!(
        event_rx.recv().await.expect("event received"),
        nuncio_core::CoreEvent::SyncStarted { account_id: None }
    );
}
