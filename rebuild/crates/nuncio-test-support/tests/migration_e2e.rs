#![allow(clippy::unwrap_used)]
#[allow(dead_code)]
#[path = "../../nuncio-engine/tests/support/historical_store.rs"]
mod historical;

use historical::{
    history, opened, rows, snapshot, ACCOUNT, DRAFT, IMAP, KEY, OPERATION, PDF, REQUEST, WIRE,
};
use nuncio_engine::secrets::{test_store::FileTestStore, SecretStore};
use nuncio_test_support::{
    google::Seed,
    process::{binary, isolated_command, E2eHarness},
    TestError,
};
use rusqlite::Connection;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};

fn catalogue(c: &Connection) -> Vec<(String, Option<String>)> {
    c.prepare("SELECT name,sql FROM sqlite_schema ORDER BY name")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn assert_generation(path: &Path, expected: &Connection, version: u32) {
    let c = opened(path);
    assert_eq!(
        c.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        version
    );
    assert_eq!(
        catalogue(&c),
        catalogue(expected),
        "partial migration DDL at version {version}"
    );
    for (name, table) in snapshot(expected) {
        assert_eq!(
            rows(&c, &name, &table.columns),
            table.rows,
            "partial migration rows in {name}"
        );
    }
    if version > 0 {
        assert_eq!(
            c.query_row("SELECT count(*) FROM schema_migrations", [], |r| r
                .get::<_, u32>(0))
                .unwrap(),
            version
        );
    }
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
}

async fn cli(h: &E2eHarness, args: &[&str]) -> Value {
    let output = h.cli(args).await.unwrap();
    assert_eq!(
        output.status,
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.json().unwrap()["result"].clone()
}

async fn check_cli(h: &E2eHarness, old: u32, restart: u32) -> Result<(), TestError> {
    let status = cli(h, &["--json", "system", "status"]).await;
    assert_eq!(status["storage"]["schema_version"], 23);
    let accounts = cli(h, &["--json", "account", "list"]).await;
    assert_eq!(
        accounts["accounts"].as_array().unwrap().len(),
        if old == 0 { 0 } else { 2 }
    );
    for account in if old == 0 {
        &[][..]
    } else {
        &[ACCOUNT, IMAP][..]
    } {
        if old >= 3 {
            let mail = cli(h, &["--json", "mail", "list", "--account", account]).await;
            assert_eq!(mail["items"].as_array().unwrap().len(), 1);
            for (kind, bytes) in [("raw", WIRE), ("attachment", PDF)] {
                let output = h.artifacts.join(format!("{account}-{restart}-{kind}.bin"));
                let mut args = vec![
                    "--json",
                    "mail",
                    kind,
                    "--account",
                    account,
                    "--message",
                    "message",
                    "--output",
                    output.to_str().unwrap(),
                ];
                if kind == "attachment" {
                    args.extend(["--attachment", "attachment"]);
                }
                cli(h, &args).await;
                assert_eq!(std::fs::read(output)?, bytes);
            }
        }
        if old >= 7 {
            let draft = cli(
                h,
                &[
                    "--json",
                    "mail",
                    "draft",
                    "show",
                    "--account",
                    account,
                    "--draft",
                    DRAFT,
                ],
            )
            .await;
            assert_eq!(draft["content"]["subject"], "Historical mail");
            assert_eq!(draft["attachments"].as_array().unwrap().len(), 1);
        }
        if old >= 10 {
            let op = cli(
                h,
                &[
                    "--json",
                    "operations",
                    "show",
                    "--account",
                    account,
                    "--operation",
                    OPERATION,
                ],
            )
            .await;
            assert_eq!(op["state"], "queued");
            assert_eq!(op["request_id"], REQUEST);
            let attempts = cli(
                h,
                &[
                    "--json",
                    "operations",
                    "attempts",
                    "--account",
                    account,
                    "--operation",
                    OPERATION,
                ],
            )
            .await;
            assert!(attempts["items"].as_array().unwrap().is_empty());
        }
    }
    Ok(())
}

#[tokio::test]
async fn sigkill_before_and_after_every_migration_commit_preserves_whole_generations_and_cli_data(
) -> Result<(), TestError> {
    for old in 0..history().last_schema {
        for after in [false, true] {
            let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
            h.shutdown().await?;
            let path = h.directory.join("store.db");
            std::fs::rename(&path, h.artifacts.join("initial-empty-store.db"))?;
            if old > 0 {
                historical::fixture(&path, old);
            }
            let c = opened(&path);
            let journal: String =
                c.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))?;
            assert_eq!(journal, "wal");
            c.close().unwrap();
            let baseline = std::fs::read(&path)?;
            let expected_path = h.artifacts.join("expected.db");
            std::fs::write(&expected_path, &baseline)?;
            let expected = opened(&expected_path);
            if after {
                expected.execute_batch(&history().migrations[old as usize].sql)?;
            }
            let expected_version = old + u32::from(after);
            let profile = std::fs::read(h.directory.join("profile.json"))?;
            let id: Value = serde_json::from_slice(&profile)?;
            FileTestStore::new(h.secrets_file.clone()).put(
                &format!("{}/profile/database", id["id"].as_str().unwrap()),
                &KEY,
            )?;
            let keys = zeroize::Zeroizing::new(std::fs::read(&h.secrets_file)?);
            let remote = serde_json::to_value(h.google.control().snapshot().await)?;
            let name = format!(
                "migration-{}-{}-commit",
                old + 1,
                if after { "after" } else { "before" }
            );
            let entered = h.artifacts.join("barriers").join(format!("{name}.entered"));
            std::fs::write(h.artifacts.join("barriers").join(format!("{name}.arm")), [])?;
            let ready = h.artifacts.join("migration.ready.json");
            let mut child =
                isolated_command(&binary("NUNCIO_E2E_DAEMON")?, &h.artifacts.join("tmp"))
                    .args(["--bind", "127.0.0.1:0", "--data-dir"])
                    .arg(&h.directory)
                    .arg("--test-secrets-file")
                    .arg(&h.secrets_file)
                    .arg("--test-config")
                    .arg(h.artifacts.join("daemon-1.test.json"))
                    .arg("--ready-file")
                    .arg(&ready)
                    .stdout(std::fs::File::create(
                        h.artifacts.join("migration.stdout.log"),
                    )?)
                    .stderr(std::fs::File::create(
                        h.artifacts.join("migration.stderr.log"),
                    )?)
                    .spawn()?;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            while !entered.exists() && tokio::time::Instant::now() < deadline {
                if child.try_wait()?.is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let reached = entered.exists();
            let premature_ready = ready.exists();
            if reached {
                assert_generation(&path, &expected, expected_version);
            }
            child.kill().await?;
            let killed = child.wait().await?;
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                assert_eq!(killed.signal(), Some(9));
            }
            assert!(reached, "actual daemon did not reach {name}");
            assert!(
                !premature_ready,
                "daemon advertised readiness during migration"
            );
            assert_generation(&path, &expected, expected_version);
            let crash_hash = hex::encode(Sha256::digest(std::fs::read(&path)?));
            if !after {
                assert!(
                    std::fs::read(&path)? == baseline,
                    "uncommitted migration changed original ciphertext"
                );
            }
            assert!(std::fs::read(&h.secrets_file)? == *keys);
            assert_eq!(std::fs::read(h.directory.join("profile.json"))?, profile);
            expected.close().unwrap();
            for restart in 1..=2 {
                h.restart().await?;
                check_cli(&h, old, restart).await?;
                h.shutdown().await?;
                assert_eq!(
                    serde_json::to_value(h.google.control().snapshot().await)?,
                    remote,
                    "migration/restart caused provider effects"
                );
                assert!(std::fs::read(&h.secrets_file)? == *keys);
                assert_eq!(std::fs::read(h.directory.join("profile.json"))?, profile);
            }
            if let Some(root) = std::env::var_os("NUNCIO_TEST_ARTIFACTS") {
                let case = h.artifacts.file_name().unwrap().to_str().unwrap();
                let evidence = serde_json::json!({
                    "case": name, "from_schema": old, "schema_after_kill": expected_version,
                    "final_schema": 23, "verified_restarts": 2, "signal": 9,
                    "frozen_migration_sha256": history().migrations[old as usize].sha256,
                    "original_ciphertext_sha256": hex::encode(Sha256::digest(&baseline)),
                    "post_crash_ciphertext_sha256": crash_hash,
                    "whole_catalog_and_rows_preserved": true, "cli_payloads_and_requests_preserved": true,
                    "profile_and_synthetic_keys_unchanged": true, "google_snapshot_unchanged": true
                });
                std::fs::write(
                    Path::new(&root).join(format!("{case}-migration.json")),
                    serde_json::to_vec_pretty(&evidence)?,
                )?;
            }
        }
    }
    Ok(())
}
