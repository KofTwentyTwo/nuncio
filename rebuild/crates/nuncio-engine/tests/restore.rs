#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    domain::{
        drafts::{DraftContent, Recipient},
        prepare::PreparedAttachment,
        submission::{freeze, SubmissionTransport},
    },
    store::{
        stage_restore, AttemptKind, AttemptOutcome, ConnectedAccount, DraftUploadInput,
        EnqueueSend, OperationReceipt, SaveDraft, Store, StoreError,
    },
};
use rusqlite::{types::Value, Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::path::Path;
use zeroize::Zeroizing;

fn phrase() -> Zeroizing<String> {
    Zeroizing::new("synthetic independent restore recovery phrase".into())
}
fn opened(path: &Path, key: &str) -> Connection {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    c.pragma_update(None, "key", key).unwrap();
    c
}
fn rows(c: &Connection, table: &str) -> Vec<Vec<Value>> {
    let mut statement = c
        .prepare(&format!("SELECT * FROM {table} ORDER BY 1,2"))
        .unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |r| (0..columns).map(|i| r.get(i)).collect())
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}
async fn connect(store: &Store, account: &str, reference: &str) {
    store.prepare_credential(reference.into()).await.unwrap();
    store
        .connect_google(ConnectedAccount {
            id: account.into(),
            subject: "synthetic-alpha".into(),
            address: "alpha@example.test".into(),
            credential_ref: reference.into(),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn restore_rekeys_preserves_durable_payloads_and_holds_all_pending_work_after_reconnect() {
    let temp = tempfile::tempdir().unwrap();
    let original_dir = temp.path().join("original");
    let store = Store::open(&original_dir, Zeroizing::new(vec![0x41; 32]))
        .await
        .unwrap();
    let account = uuid::Uuid::new_v4().to_string();
    connect(&store, &account, "old-profile-active-credential").await;
    store
        .prepare_credential("old-profile-pending-cleanup".into())
        .await
        .unwrap();
    let content = DraftContent {
        to: vec![Recipient {
            address: "beta@example.test".into(),
            name: None,
        }],
        subject: "restore durable canary".into(),
        text: Some("Keep this draft and frozen body".into()),
        ..Default::default()
    };
    let draft = store
        .save_draft(
            SaveDraft {
                account_id: account.clone(),
                id: None,
                expected_version: None,
                content: content.clone(),
            },
            1000,
        )
        .await
        .unwrap();
    let pdf = include_bytes!("../../../tests/fixtures/recovery/queued-draft.pdf");
    let upload = store
        .begin_draft_upload(
            DraftUploadInput {
                account_id: account.clone(),
                draft_id: draft.id.clone(),
                expected_version: 1,
                filename: Some("queued-draft.pdf".into()),
                mime_type: "application/pdf".into(),
                parameters: Default::default(),
                content_id: None,
                disposition: "attachment".into(),
                byte_length: pdf.len() as u64,
                sha256: format!("{:x}", Sha256::digest(pdf)),
            },
            1000,
        )
        .await
        .unwrap();
    store
        .append_draft_upload(&upload, 0, pdf.to_vec())
        .await
        .unwrap();
    let draft = store.finish_draft_upload(upload, 1000).await.unwrap();
    let attachments = [PreparedAttachment {
        filename: Some("queued-draft.pdf".into()),
        mime_type: "application/pdf".into(),
        parameters: Default::default(),
        content_id: None,
        disposition: "attachment".into(),
        bytes: pdf.to_vec(),
    }];
    let mut ids = Vec::new();
    for (i, state) in ["applied", "queued", "uncertain", "running"]
        .into_iter()
        .enumerate()
    {
        let op = store
            .enqueue_send(
                EnqueueSend {
                    account_id: account.clone(),
                    request_id: uuid::Uuid::new_v4().to_string(),
                    draft_id: draft.id.clone(),
                    snapshot_version: 2,
                    expected_version: Some(2),
                    sender: "alpha@example.test".into(),
                    frozen: freeze(
                        "alpha@example.test",
                        &content,
                        None,
                        &attachments,
                        &format!("restore-{i}@nuncio.invalid"),
                        1001,
                        SubmissionTransport::Gmail,
                    )
                    .unwrap(),
                },
                1001,
            )
            .await
            .unwrap();
        if state != "queued" {
            store
                .begin_operation_attempt(
                    account.clone(),
                    op.id.clone(),
                    AttemptKind::Dispatch,
                    1002,
                )
                .await
                .unwrap();
        }
        if state == "applied" {
            store
                .finish_operation_attempt(
                    account.clone(),
                    op.id.clone(),
                    1,
                    AttemptOutcome::Applied(OperationReceipt {
                        kind: "google_send".into(),
                        source: "acknowledgement".into(),
                        provider_id: Some("remote-positive-id".into()),
                        etag: None,
                    }),
                    1003,
                )
                .await
                .unwrap();
        } else if state == "uncertain" {
            store
                .finish_operation_attempt(
                    account.clone(),
                    op.id.clone(),
                    1,
                    AttemptOutcome::Uncertain {
                        code: "lost_acknowledgement".into(),
                    },
                    1003,
                )
                .await
                .unwrap();
        }
        ids.push(op.id);
    }
    let backup = store.create_backup(phrase(), 2000).await.unwrap();
    let original_bytes = std::fs::read(backup.path()).unwrap();
    let source = opened(backup.path(), &phrase());
    let source_operations = rows(&source, "operations");
    let source_attempts = rows(&source, "operation_attempts");
    let staged = stage_restore(
        backup.path(),
        phrase(),
        Zeroizing::new(vec![0x52; 32]),
        temp.path(),
        900,
    )
    .unwrap();
    let stage_path = staged.directory().to_owned();
    assert!(stage_path.join("store.db").is_file());
    let target = temp.path().join("restored");
    let report = staged.activate(&target).unwrap();
    assert_eq!(report.backup, *backup.inspection());
    assert_eq!(report.schema_version, 22);
    assert_eq!(report.held_operations, 3);
    assert!(report.revision > report.backup.revision);
    assert!(!stage_path.exists());
    let restored_db = target.join("store.db");
    let restored = opened(&restored_db, &format!("x'{}'", hex::encode([0x52; 32])));
    for table in [
        "drafts",
        "draft_attachments",
        "send_payloads",
        "blobs",
        "blob_chunks",
        "operation_receipts",
        "operation_resolutions",
    ] {
        assert_eq!(
            rows(&source, table),
            rows(&restored, table),
            "changed durable {table}"
        );
    }
    assert_eq!(
        restored
            .pragma_query_value(None, "application_id", |r| r.get::<_, u32>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        restored
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name='nuncio_backup_metadata'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        restored
            .query_row(
                "SELECT count(*) FROM restored_operations WHERE backup_sha256=?1",
                [&report.backup.sha256],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        3
    );
    let restored_store = Store::open(&target, Zeroizing::new(vec![0x52; 32]))
        .await
        .unwrap();
    let restored_account = restored_store
        .account(account.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored_account.state, "disconnected");
    assert!(restored_account.credential_ref.is_none());
    assert!(restored_store
        .credential_cleanup()
        .await
        .unwrap()
        .is_empty());
    assert_eq!(restored_store.recover_operations(4000).await.unwrap(), 0);
    connect(&restored_store, &account, "new-profile-credential").await;
    assert!(restored_store
        .ready_operations(5000)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        restored_store
            .get_operation(account.clone(), ids[0].clone())
            .await
            .unwrap()
            .state,
        "applied"
    );
    for id in &ids[1..] {
        let op = restored_store
            .get_operation(account.clone(), id.clone())
            .await
            .unwrap();
        let original = store
            .get_operation(account.clone(), id.clone())
            .await
            .unwrap();
        assert_eq!(op.request_id, original.request_id);
        assert_eq!(op.fingerprint, original.fingerprint);
        assert_eq!(op.resource_id, original.resource_id);
        assert_eq!(op.version, original.version + 1);
        assert_eq!(op.state, "uncertain");
        assert!(!op.needs_reconciliation);
        assert!(op.next_attempt_at_ms.is_none());
        assert_eq!(
            op.error_code.as_deref(),
            Some("restored_snapshot_unreconciled")
        );
        for kind in [AttemptKind::Dispatch, AttemptKind::Reconcile] {
            assert!(matches!(
                restored_store
                    .begin_operation_attempt(account.clone(), id.clone(), kind, 5001)
                    .await,
                Err(StoreError::VersionConflict)
            ));
        }
    }
    let attempts = restored_store
        .operation_attempts(account.clone(), ids[3].clone())
        .await
        .unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].finished_at_ms, Some(1002));
    assert_eq!(attempts[0].outcome.as_deref(), Some("uncertain"));
    assert_eq!(
        attempts[0].error_code.as_deref(),
        Some("restored_snapshot_interrupted")
    );
    assert!(attempts[0].receipts.is_empty());
    restored_store.close().await.unwrap();
    drop(restored);
    for wrong_key in [
        phrase().to_string(),
        format!("x'{}'", hex::encode([0x41; 32])),
    ] {
        assert!(opened(&restored_db, &wrong_key)
            .query_row("SELECT count(*) FROM sqlite_schema", [], |r| r
                .get::<_, i64>(0))
            .is_err());
    }
    assert_eq!(rows(&source, "operations"), source_operations);
    assert_eq!(rows(&source, "operation_attempts"), source_attempts);
    assert_eq!(std::fs::read(backup.path()).unwrap(), original_bytes);
    assert_eq!(
        store
            .account(account.clone())
            .await
            .unwrap()
            .unwrap()
            .credential_ref
            .as_deref(),
        Some("old-profile-active-credential")
    );
    assert_eq!(
        store.credential_cleanup().await.unwrap(),
        ["old-profile-pending-cleanup"]
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn restore_never_overwrites_existing_targets_and_cleans_only_its_private_stage() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("source"), Zeroizing::new(vec![0x61; 32]))
        .await
        .unwrap();
    let backup = store.create_backup(phrase(), 1).await.unwrap();
    for kind in ["empty", "nonempty", "file", "symlink"] {
        let target = temp.path().join(kind);
        match kind {
            "empty" => std::fs::create_dir(&target).unwrap(),
            "nonempty" => {
                std::fs::create_dir(&target).unwrap();
                std::fs::write(target.join("original"), b"original").unwrap();
            }
            "file" => std::fs::write(&target, b"original").unwrap(),
            _ => {
                #[cfg(unix)]
                std::os::unix::fs::symlink(temp.path().join("nonempty"), &target).unwrap();
            }
        }
        let staged = stage_restore(
            backup.path(),
            phrase(),
            Zeroizing::new(vec![0x62; 32]),
            temp.path(),
            2,
        )
        .unwrap();
        let private = staged.directory().to_owned();
        assert!(staged.activate(&target).is_err(), "overwrote {kind}");
        assert!(!private.exists());
        match kind {
            "empty" => assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0),
            "file" => assert_eq!(std::fs::read(&target).unwrap(), b"original"),
            _ => assert_eq!(std::fs::read(target.join("original")).unwrap(), b"original"),
        }
    }
    let staged = stage_restore(
        backup.path(),
        phrase(),
        Zeroizing::new(vec![0x62; 32]),
        temp.path(),
        2,
    )
    .unwrap();
    let private = staged.directory().to_owned();
    drop(staged);
    assert!(!private.exists());
    store.close().await.unwrap();
}

#[tokio::test]
async fn restore_rejects_bad_inputs_and_failed_migration_without_altering_original_files() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("profile"), Zeroizing::new(vec![0x31; 32]))
        .await
        .unwrap();
    let backup = store.create_backup(phrase(), 1).await.unwrap();
    let original = std::fs::read(backup.path()).unwrap();
    let target = temp.path().join("original-target");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("canary"), b"keep original target").unwrap();
    for case in [
        "wrong-key",
        "corrupt",
        "truncated",
        "future",
        "failed-migration",
        "schema18",
    ] {
        let input = temp.path().join(case);
        std::fs::write(&input, &original).unwrap();
        match case {
            "corrupt" => {
                let mut bytes = original.clone();
                let n = bytes.len();
                bytes[n - 37] ^= 1;
                std::fs::write(&input, bytes).unwrap();
            }
            "truncated" => std::fs::OpenOptions::new()
                .write(true)
                .open(&input)
                .unwrap()
                .set_len(128)
                .unwrap(),
            "future" | "failed-migration" | "schema18" => {
                let c = Connection::open(&input).unwrap();
                c.pragma_update(None, "key", phrase().as_str()).unwrap();
                let version = if case == "future" { 999 } else { 18 };
                c.pragma_update(None, "user_version", version).unwrap();
                c.execute(
                    "UPDATE nuncio_backup_metadata SET schema_version=?1",
                    [version],
                )
                .unwrap();
                if version == 18 {
                    c.execute_batch("DROP TABLE staged_calendar_repair_events; DROP TABLE staged_calendar_repair_ids; DROP TABLE operation_reconciliation_requests; DROP TABLE restore_cleanup_jobs;").unwrap();
                    c.execute("DELETE FROM schema_migrations WHERE version>18", [])
                        .unwrap();
                    if case == "schema18" {
                        c.execute("DROP TABLE restored_operations", []).unwrap();
                    }
                }
            }
            _ => {}
        }
        let before = std::fs::read(&input).unwrap();
        let passphrase = if case == "wrong-key" {
            Zeroizing::new("incorrect synthetic recovery phrase".into())
        } else {
            phrase()
        };
        let result = stage_restore(
            &input,
            passphrase,
            Zeroizing::new(vec![0x32; 32]),
            temp.path(),
            2,
        );
        if case == "schema18" {
            let staged = result.unwrap();
            let report = staged.activate(&temp.path().join("upgraded")).unwrap();
            assert_eq!(report.backup.schema_version, 18);
            assert_eq!(report.schema_version, 22);
            let c = opened(
                &temp.path().join("upgraded/store.db"),
                &format!("x'{}'", hex::encode([0x32; 32])),
            );
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM schema_migrations WHERE version=19",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
        } else {
            assert!(result.is_err(), "accepted {case}");
            if case == "future" {
                assert!(matches!(result, Err(StoreError::FutureSchema)));
            }
        }
        assert_eq!(std::fs::read(&input).unwrap(), before);
        assert_eq!(std::fs::read(backup.path()).unwrap(), original);
        assert_eq!(
            std::fs::read(target.join("canary")).unwrap(),
            b"keep original target"
        );
        assert!(!std::fs::read_dir(temp.path()).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".restore-")));
    }
    for len in [0, 31, 33] {
        assert!(matches!(
            stage_restore(
                backup.path(),
                phrase(),
                Zeroizing::new(vec![0x32; len]),
                temp.path(),
                2
            ),
            Err(StoreError::InvalidKey)
        ));
    }
    assert!(matches!(
        stage_restore(
            backup.path(),
            phrase(),
            Zeroizing::new(vec![0x32; 32]),
            temp.path(),
            -1
        ),
        Err(StoreError::InvalidInput)
    ));
    for suffix in ["-wal", "-shm", "-journal"] {
        let input = temp.path().join(format!("sidecar{suffix}"));
        std::fs::write(&input, &original).unwrap();
        let mut sidecar = input.as_os_str().to_os_string();
        sidecar.push(suffix);
        std::fs::write(&sidecar, b"untrusted companion").unwrap();
        assert!(matches!(
            stage_restore(
                &input,
                phrase(),
                Zeroizing::new(vec![0x32; 32]),
                temp.path(),
                2
            ),
            Err(StoreError::InvalidInput)
        ));
        assert_eq!(std::fs::read(&sidecar).unwrap(), b"untrusted companion");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let link = temp.path().join("linked-source");
        symlink(backup.path(), &link).unwrap();
        assert!(stage_restore(
            &link,
            phrase(),
            Zeroizing::new(vec![0x32; 32]),
            temp.path(),
            2
        )
        .is_err());
        let parent = temp.path().join("parent");
        std::fs::create_dir(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
        let staged = stage_restore(
            backup.path(),
            phrase(),
            Zeroizing::new(vec![0x32; 32]),
            &parent,
            2,
        )
        .unwrap();
        assert_eq!(
            std::fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            std::fs::metadata(staged.directory())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(staged.directory().join("store.db"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let private = staged.directory().to_owned();
        assert!(matches!(
            staged.activate(&temp.path().join("different-parent")),
            Err(StoreError::InvalidPath)
        ));
        assert!(!private.exists());
    }
    store.close().await.unwrap();
}
