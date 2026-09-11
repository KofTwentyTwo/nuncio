#![allow(clippy::unwrap_used)]
#[path = "support/historical_store.rs"]
mod historical;
use historical::*;
use nuncio_engine::store::{MailQuery, Store};
use zeroize::Zeroizing;

fn catalogue(c: &rusqlite::Connection) -> Vec<(String, Option<String>)> {
    c.prepare("SELECT name,sql FROM sqlite_schema ORDER BY name")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

#[tokio::test]
async fn failure_at_each_migration_ledger_write_rolls_back_schema_and_data_and_allows_retry() {
    for version in 1..history().last_schema {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.db");
        fixture(&path, version);
        let c = opened(&path);
        let journal: String = c
            .pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))
            .unwrap();
        assert_eq!(journal, "wal");
        if version == 4 {
            c.execute("UPDATE store_meta SET revision=20001", [])
                .unwrap();
            c.execute(
                "INSERT INTO change_log(revision,kind,account_id) VALUES(1,'old-change',?1)",
                [ACCOUNT],
            )
            .unwrap();
        }
        c.execute_batch(&format!("CREATE TRIGGER reject_migration BEFORE INSERT ON schema_migrations WHEN NEW.version={} BEGIN SELECT RAISE(ABORT,'synthetic migration ledger failure'); END;",version+1)).unwrap();
        let schema = catalogue(&c);
        let before = snapshot(&c);
        c.close().unwrap();
        let original = std::fs::read(&path).unwrap();
        assert!(matches!(
            Store::open(temp.path(), Zeroizing::new(KEY.to_vec())).await,
            Err(nuncio_engine::store::StoreError::Database(Some(1811)))
        ));
        assert!(
            std::fs::read(&path).unwrap() == original,
            "schema {version} failed migration rewrote original encrypted bytes"
        );
        let c = opened(&path);
        assert_eq!(catalogue(&c), schema, "schema {version} left partial DDL");
        for (name, table) in &before {
            assert!(
                rows(&c, name, &table.columns) == table.rows,
                "schema {version} failed migration altered {name}"
            );
        }
        assert_eq!(
            c.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                .unwrap(),
            version
        );
        assert_eq!(
            c.query_row("SELECT count(*) FROM schema_migrations", [], |r| r
                .get::<_, u32>(0))
                .unwrap(),
            version
        );
        c.execute_batch("DROP TRIGGER reject_migration").unwrap();
        c.close().unwrap();
        let store = Store::open(temp.path(), Zeroizing::new(KEY.to_vec()))
            .await
            .unwrap();
        assert_eq!(store.status().await.unwrap().schema_version, 22);
        if version >= 7 {
            assert_eq!(
                store
                    .get_draft(ACCOUNT.into(), DRAFT.into())
                    .await
                    .unwrap()
                    .attachments[0]
                    .blob_id,
                "pdf"
            );
        }
        store.close().await.unwrap();
    }
}

#[tokio::test]
async fn every_historical_backup_restores_into_a_new_encrypted_store_with_pending_sends_held() {
    const PASSPHRASE: &str = "Historical synthetic backup recovery phrase";
    let decoded = nuncio_engine::domain::mail::decode_mime(WIRE, 64 * 1024 * 1024).unwrap();
    assert_eq!(decoded.attachments.len(), 1);
    assert_eq!(decoded.attachments[0].part_index, 2);
    assert_eq!(decoded.attachments[0].bytes, PDF);
    for version in 1..=history().last_schema {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("original.nuncio");
        let before = backup_fixture(&source, version, PASSPHRASE);
        assert_eq!(before.contains_key("drafts"), version >= 7);
        assert_eq!(before.contains_key("operations"), version >= 10);
        let inspection =
            nuncio_engine::store::inspect_backup(&source, Zeroizing::new(PASSPHRASE.into()))
                .unwrap();
        assert_eq!(inspection.accounts, 2);
        assert_eq!(inspection.drafts, if version >= 7 { 2 } else { 0 });
        assert_eq!(inspection.operations, if version >= 10 { 2 } else { 0 });
        let source_bytes = std::fs::read(&source).unwrap();
        let staged = nuncio_engine::store::stage_restore(
            &source,
            Zeroizing::new(PASSPHRASE.into()),
            Zeroizing::new(KEY.to_vec()),
            temp.path(),
            2000,
        )
        .unwrap();
        let target = temp.path().join("restored");
        let report = staged.activate(&target).unwrap();
        assert_eq!(report.schema_version, 22);
        assert_eq!(report.backup.schema_version, version);
        assert_eq!(report.held_operations, if version >= 10 { 2 } else { 0 });
        assert!(
            std::fs::read(&source).unwrap() == source_bytes,
            "schema {version} restore modified its original backup"
        );
        let store = Store::open(&target, Zeroizing::new(KEY.to_vec()))
            .await
            .unwrap();
        for account in [ACCOUNT, IMAP] {
            let row = store.account(account.into()).await.unwrap().unwrap();
            assert_eq!(row.state, "disconnected");
            assert!(row.credential_ref.is_none());
            if version >= 3 {
                assert_eq!(
                    store
                        .blob_chunk(account.into(), "pdf".into(), 0)
                        .await
                        .unwrap(),
                    PDF
                );
            }
            if version >= 7 {
                assert_eq!(
                    store
                        .get_draft(account.into(), DRAFT.into())
                        .await
                        .unwrap()
                        .attachments[0]
                        .blob_id,
                    "pdf"
                );
            }
            if version >= 10 {
                let operation = store
                    .find_send_request(account.into(), REQUEST.into(), DRAFT.into(), None)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(operation.id, OPERATION);
                assert_eq!(operation.state, "uncertain");
                assert_eq!(
                    operation.error_code.as_deref(),
                    Some("restored_snapshot_unreconciled")
                );
                assert_eq!(operation.version, 2);
                assert!(!operation.needs_reconciliation);
                let wire = store
                    .send_payload(account.into(), OPERATION.into())
                    .await
                    .unwrap()
                    .wire;
                assert_eq!(
                    store.blob_chunk(account.into(), wire.id, 0).await.unwrap(),
                    WIRE
                );
            }
        }
        assert!(store.credential_cleanup().await.unwrap().is_empty());
        store.close().await.unwrap();
        let c = opened(&target.join("store.db"));
        for (name, table) in &before {
            match name.as_str() {
                "store_meta"
                | "change_log"
                | "nuncio_backup_metadata"
                | "credential_cleanup"
                | "restored_operations" => continue,
                _ => {}
            }
            let mutable: &[&str] = match name.as_str() {
                "accounts" => &["state", "credential_ref"],
                "operations" => &[
                    "state",
                    "version",
                    "updated_at_ms",
                    "next_attempt_at_ms",
                    "needs_reconciliation",
                    "error_code",
                ],
                _ => &[],
            };
            let indexes = table
                .columns
                .iter()
                .enumerate()
                .filter(|(_, column)| !mutable.contains(&column.as_str()))
                .map(|(i, _)| i)
                .collect::<Vec<_>>();
            let columns = indexes
                .iter()
                .map(|&i| table.columns[i].clone())
                .collect::<Vec<_>>();
            let expected = table
                .rows
                .iter()
                .map(|row| indexes.iter().map(|&i| row[i].clone()).collect::<Vec<_>>())
                .collect::<Vec<_>>();
            assert!(
                rows(&c, name, &columns) == expected,
                "schema {version} restore changed immutable fields in {name}"
            );
        }
        assert_eq!(
            c.query_row("SELECT revision FROM store_meta", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            42
        );
        assert_eq!(
            c.query_row("SELECT kind FROM change_log WHERE revision=41", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
            "legacy-fixture"
        );
        assert_eq!(
            c.query_row("SELECT kind FROM change_log WHERE revision=42", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
            "profile_restored"
        );
        assert_eq!(c.query_row("SELECT count(*) FROM restored_operations WHERE source_state='queued' AND source_version=1 AND restored_at_ms=2000 AND backup_sha256=?1",[&report.backup.sha256],|r|r.get::<_,i64>(0)).unwrap(),if version>=10 {2}else{0});
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name='nuncio_backup_metadata'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        c.close().unwrap();
    }
}

#[test]
fn backup_counts_do_not_hide_missing_tables_in_schemas_that_require_them() {
    const PASSPHRASE: &str = "Synthetic incomplete historical schema phrase";
    for (version, table) in [(7, "drafts"), (10, "operations")] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("damaged.nuncio");
        backup_fixture(&path, version, PASSPHRASE);
        let c = rusqlite::Connection::open(&path).unwrap();
        c.pragma_update(None, "key", PASSPHRASE).unwrap();
        c.pragma_update(None, "foreign_keys", false).unwrap();
        // Remove dependents as well, so this reaches the required-table query.
        if table == "drafts" {
            c.execute_batch("DROP TABLE draft_attachments").unwrap();
        } else {
            c.execute_batch("DROP TABLE send_payloads; DROP TABLE operation_receipts; DROP TABLE operation_resolutions; DROP TABLE operation_attempts").unwrap();
        }
        c.execute_batch(&format!("DROP TABLE {table}")).unwrap();
        c.close().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            nuncio_engine::store::inspect_backup(&path, Zeroizing::new(PASSPHRASE.into())).is_err()
        );
        assert!(std::fs::read(&path).unwrap() == bytes);
    }
}

#[tokio::test]
async fn every_frozen_schema_migrates_without_losing_existing_columns_or_durable_payloads() {
    for version in 1..=history().last_schema {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.db");
        fixture(&path, version);
        let c = opened(&path);
        let before = snapshot(&c);
        c.close().unwrap();
        let store = Store::open(temp.path(), Zeroizing::new(KEY.to_vec()))
            .await
            .unwrap();
        assert_eq!(store.status().await.unwrap().schema_version, 22);
        assert_eq!(store.accounts().await.unwrap().len(), 2);
        for account in [ACCOUNT, IMAP] {
            assert_eq!(
                store.account(account.into()).await.unwrap().unwrap().state,
                "disconnected"
            );
            if version >= 3 {
                let result = store
                    .query_mail(MailQuery {
                        account_id: account.into(),
                        query: Some("historical".into()),
                        collection_id: None,
                        page_size: 10,
                        page_token: None,
                    })
                    .await
                    .unwrap();
                assert_eq!(result.items.len(), 1);
                assert_eq!(
                    store
                        .blob_chunk(account.into(), "pdf".into(), 0)
                        .await
                        .unwrap(),
                    PDF
                );
                assert_eq!(
                    store
                        .blob_chunk(account.into(), "wire".into(), 0)
                        .await
                        .unwrap(),
                    WIRE
                );
            }
            if version >= 7 {
                let draft = store.get_draft(account.into(), DRAFT.into()).await.unwrap();
                assert_eq!(draft.content.subject, "Historical mail");
                assert_eq!(draft.attachments.len(), 1);
                assert_eq!(draft.attachments[0].blob_id, "pdf");
            }
            if version >= 10 {
                let operation = store
                    .find_send_request(account.into(), REQUEST.into(), DRAFT.into(), None)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(operation.id, OPERATION);
                assert_eq!(operation.state, "queued");
                assert_eq!(
                    store
                        .send_payload(account.into(), OPERATION.into())
                        .await
                        .unwrap()
                        .message_id,
                    "<legacy@example.test>"
                );
            }
        }
        if version >= 13 {
            assert!(store.imap_config(IMAP.into()).await.unwrap().1.uidplus);
        }
        if version >= 17 {
            let intent = store
                .smtp_intent(IMAP.into(), OPERATION.into())
                .await
                .unwrap();
            assert!(intent.sent_fingerprint.is_none());
            let progress = store
                .smtp_progress(IMAP.into(), OPERATION.into())
                .await
                .unwrap();
            assert!(
                progress.sent_floor.is_none(),
                "migration must not invent a pre-DATA observation"
            );
            assert_eq!(progress.step, nuncio_engine::store::SmtpStep::Prepared);
        }
        store.close().await.unwrap();
        let c = opened(&path);
        for (name, table) in &before {
            assert!(
                rows(&c, name, &table.columns) == table.rows,
                "schema {version} changed existing data in {name}"
            );
        }
        assert_eq!(
            c.query_row("SELECT count(*) FROM schema_migrations", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            22
        );
        assert_eq!(
            c.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert!(c
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .query([])
            .unwrap()
            .next()
            .unwrap()
            .is_none());
        c.close().unwrap();
        let original = std::fs::read(&path).unwrap();
        assert!(!original
            .windows(b"Original historical body".len())
            .any(|v| v == b"Original historical body"));
        Store::open(temp.path(), Zeroizing::new(KEY.to_vec()))
            .await
            .unwrap()
            .close()
            .await
            .unwrap();
    }
}
