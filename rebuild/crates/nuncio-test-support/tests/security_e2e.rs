#![allow(clippy::unwrap_used)]
use nuncio_test_support::{
    google::Seed,
    process::{CliOutput, E2eHarness},
    TestError,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

const BODY: &str = "NuncioConfidentialBodyCanary476215";
const CLIENT_SECRET: &str = "synthetic-client-secret";
const PHRASE: &str = "SyntheticRecoveryPassphraseCanary819735";

#[track_caller]
fn result(output: CliOutput) -> Value {
    assert_eq!(output.status, 0, "CLI operation failed");
    output.json().unwrap()["result"].clone()
}

#[tokio::test]
async fn actual_cli_renders_hostile_mail_as_data_and_never_uses_received_attachment_paths(
) -> Result<(), TestError> {
    let mut h = E2eHarness::start_with_raw_cli_logs(Seed::TwoAccounts).await?;
    let account = h.connect_google("alpha@example.test").await?;
    let trap_path = "/gmail/v1/users/me/must-not-fetch";
    let response = reqwest::Client::new()
        .get(format!("{}{trap_path}", h.google.base_url()))
        .send()
        .await?;
    assert_eq!(response.status().as_u16(), 401);
    assert!(h
        .google
        .control()
        .snapshot()
        .await
        .requests
        .iter()
        .any(|r| r.path == trap_path && r.count == 1));
    let sentinel = h.artifacts.join("escape.txt");
    std::fs::write(&sentinel, b"untouched sentinel")?;
    let controls = "\u{1b}[2J\u{9b}2J\u{202e}concealed";
    let raw=format!("From: sender@example.test\r\nTo: alpha@example.test\r\nSubject: Hostile {controls}\r\nMessage-ID: <hostile@example.test>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=security-boundary\r\n\r\n--security-boundary\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nBody {controls}\r\n--security-boundary\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<script>fetch('{0}/gmail/v1/users/me/must-not-fetch')</script><img src='{0}/gmail/v1/users/me/must-not-fetch'>\r\n--security-boundary\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"../escape.txt\"\r\nContent-Transfer-Encoding: base64\r\n\r\neHl6\r\n--security-boundary--\r\n",h.google.base_url()).into_bytes();
    h.google
        .control()
        .add_message(
            "alpha@example.test",
            "hostile-mime",
            "hostile-thread",
            raw.clone(),
            ["INBOX".into()].into_iter().collect(),
        )
        .await?;
    result(
        h.cli(&["--json", "sync", "--account", &account, "--wait"])
            .await?,
    );
    let list = result(
        h.cli(&["--json", "mail", "list", "--account", &account])
            .await?,
    );
    let id = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["provider_id"] == "hostile-mime")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    result(
        h.cli(&[
            "--json",
            "mail",
            "fetch",
            "--account",
            &account,
            "--message",
            id,
            "--wait",
        ])
        .await?,
    );
    let before = serde_json::to_value(h.google.control().snapshot().await)?;
    let mut attachment = None;
    for json_mode in [false, true] {
        let mut args = vec!["mail", "read", "--account", &account, "--message", id];
        if json_mode {
            args.insert(0, "--json");
        }
        let output = h.cli(&args).await?;
        for bytes in [&output.stdout, &output.stderr] {
            let text = std::str::from_utf8(bytes)?;
            assert!(
                !text.contains(['\u{1b}', '\u{9b}', '\u{202e}']),
                "active terminal control escaped rendering"
            );
        }
        let read = result(output);
        assert!(
            read["text"].as_str().unwrap().contains(controls),
            "JSON consumers must retain original content"
        );
        assert!(
            read["html"].as_str().unwrap().contains("<script>fetch("),
            "HTML is retained as JSON data; the CLI must not evaluate it"
        );
        assert_eq!(read["attachments"][0]["filename"], "../escape.txt");
        attachment = Some(read["attachments"][0]["id"].as_str().unwrap().to_owned());
    }
    assert_eq!(std::fs::read(&sentinel)?, b"untouched sentinel");
    let output = h.artifacts.join("explicit-download.bin");
    result(
        h.cli(&[
            "--json",
            "mail",
            "attachment",
            "--account",
            &account,
            "--message",
            id,
            "--attachment",
            attachment.as_deref().unwrap(),
            "--output",
            output.to_str().unwrap(),
        ])
        .await?,
    );
    assert_eq!(std::fs::read(output)?, b"xyz");
    assert_eq!(std::fs::read(&sentinel)?, b"untouched sentinel");
    let exported = h.artifacts.join("explicit-original.eml");
    result(
        h.cli(&[
            "--json",
            "mail",
            "raw",
            "--account",
            &account,
            "--message",
            id,
            "--output",
            exported.to_str().unwrap(),
        ])
        .await?,
    );
    assert_eq!(std::fs::read(exported)?, raw);
    assert_eq!(
        serde_json::to_value(h.google.control().snapshot().await)?,
        before
    );
    h.shutdown().await?;
    Ok(())
}
fn no_canaries(bytes: &[u8], canaries: &[Vec<u8>], location: &Path) {
    for (index, canary) in canaries.iter().enumerate() {
        assert!(!canary.is_empty());
        assert!(
            !bytes.windows(canary.len()).any(|v| v == canary),
            "canary {index} leaked in {}",
            location.display()
        );
    }
}
fn scan_tree(
    path: &Path,
    canaries: &[Vec<u8>],
    scanned: &mut BTreeSet<PathBuf>,
) -> Result<(), TestError> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        assert!(
            !kind.is_symlink(),
            "test output contains an unexpected symlink"
        );
        if kind.is_dir() {
            scan_tree(&entry.path(), canaries, scanned)?;
        } else if kind.is_file() {
            no_canaries(&std::fs::read(entry.path())?, canaries, &entry.path());
            scanned.insert(entry.path());
        }
    }
    Ok(())
}
fn independent_sqlite_rejects(path: &Path) -> Result<(), TestError> {
    let script = r#"import sqlite3, sys, urllib.parse
uri='file:' + urllib.parse.quote(sys.argv[1], safe='/') + '?mode=ro&immutable=1'
try:
    c=sqlite3.connect(uri, uri=True)
    c.execute('SELECT count(*) FROM sqlite_master').fetchone()
except sqlite3.DatabaseError:
    sys.exit(0)
sys.exit(1)
"#;
    let output = std::process::Command::new("python3")
        .args(["-c", script])
        .arg(path)
        .output()?;
    assert!(
        output.status.success(),
        "ordinary Python SQLite must reject the encrypted file"
    );
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    Ok(())
}
fn wrong_sqlcipher_key_rejects(path: &Path) -> Result<(), TestError> {
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.pragma_update(None, "key", "wrong synthetic SQLCipher key")?;
    assert!(connection
        .query_row("SELECT count(*) FROM sqlite_master", [], |r| r
            .get::<_, i64>(0))
        .is_err());
    Ok(())
}

#[tokio::test]
async fn actual_daemon_cli_keeps_content_and_secrets_out_of_storage_temp_and_unredacted_logs(
) -> Result<(), TestError> {
    let mut h = E2eHarness::start_with_raw_cli_logs(Seed::TwoAccounts).await?;
    std::fs::write(
        h.artifacts.join("mock-client-registration.json"),
        serde_json::to_vec(
            &json!({"installed":{"client_id":"nuncio-test-client","client_secret":CLIENT_SECRET}}),
        )?,
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            h.artifacts.join("mock-client-registration.json"),
            std::fs::Permissions::from_mode(0o600),
        )?;
    }
    let account = h.connect_google("alpha@example.test").await?;
    let raw = format!("From: sender@example.test\r\nTo: alpha@example.test\r\nSubject: Confidential fixture\r\nMessage-ID: <confidential@example.test>\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{BODY}\r\n").into_bytes();
    h.google
        .control()
        .add_message(
            "alpha@example.test",
            "security-body",
            "security-thread",
            raw,
            ["INBOX".into()].into_iter().collect(),
        )
        .await?;
    result(
        h.cli(&["--json", "sync", "--account", &account, "--wait"])
            .await?,
    );
    let listed = result(
        h.cli(&["--json", "mail", "list", "--account", &account])
            .await?,
    );
    let id = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["provider_id"] == "security-body")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    result(
        h.cli(&[
            "--json",
            "mail",
            "fetch",
            "--account",
            &account,
            "--message",
            id,
            "--wait",
        ])
        .await?,
    );
    let found = result(
        h.cli(&[
            "--json",
            "mail",
            "search",
            "--account",
            &account,
            "--query",
            BODY,
        ])
        .await?,
    );
    assert_eq!(
        found["items"].as_array().unwrap().len(),
        1,
        "private body must actually reach encrypted FTS"
    );
    assert_eq!(found["items"][0]["id"], id);
    assert_eq!(found["items"][0]["body_availability"], "available");
    let mut canaries = vec![
        BODY.as_bytes().to_vec(),
        CLIENT_SECRET.as_bytes().to_vec(),
        PHRASE.as_bytes().to_vec(),
        b"mock-access-".to_vec(),
        b"mock-refresh-".to_vec(),
        b"mock-code-".to_vec(),
    ];
    let keys: Value = serde_json::from_slice(&std::fs::read(&h.secrets_file)?)?;
    for (name, value) in keys.as_object().unwrap() {
        if name.contains("/profile/") {
            let encoded = value.as_str().unwrap();
            canaries.push(encoded.as_bytes().to_vec());
            canaries.push(hex::decode(encoded)?);
        }
    }
    let mut scanned = BTreeSet::new();
    let wal = h.directory.join("store.db-wal");
    assert!(
        std::fs::metadata(&wal)?.len() > 32,
        "scan a populated live WAL"
    );
    scan_tree(&h.directory, &canaries, &mut scanned)?;
    scan_tree(&h.artifacts.join("tmp"), &canaries, &mut scanned)?;
    let backup = h.artifacts.join("security-backup.nuncio");
    let input = serde_json::to_vec(&json!({"passphrase":PHRASE}))?;
    result(
        h.cli_with_stdin(
            &[
                "--json",
                "backup",
                "create",
                "--output",
                backup.to_str().unwrap(),
            ],
            &input,
        )
        .await?,
    );
    no_canaries(&std::fs::read(&backup)?, &canaries, &backup);
    scanned.insert(backup.clone());
    let wrong = h
        .cli_with_stdin(
            &[
                "--json",
                "backup",
                "inspect",
                "--file",
                backup.to_str().unwrap(),
            ],
            br#"{"passphrase":"another synthetic incorrect phrase"}"#,
        )
        .await?;
    assert_eq!(wrong.status, 2);
    let originals = [h.directory.join("store.db"), backup.clone()].map(|p| {
        let bytes = std::fs::read(&p).unwrap();
        (p, bytes)
    });
    for (path, _) in &originals {
        independent_sqlite_rejects(path)?;
        wrong_sqlcipher_key_rejects(path)?;
    }
    for (path, bytes) in originals {
        assert_eq!(std::fs::read(path)?, bytes);
    }
    h.shutdown().await?;
    h.restart().await?;
    let found = result(
        h.cli(&[
            "--json",
            "mail",
            "search",
            "--account",
            &account,
            "--query",
            BODY,
        ])
        .await?,
    );
    assert_eq!(found["items"][0]["id"], id);
    h.shutdown().await?;
    scan_tree(&h.directory, &canaries, &mut scanned)?;
    scan_tree(&h.artifacts.join("tmp"), &canaries, &mut scanned)?;
    let mut log_count = 0;
    for entry in std::fs::read_dir(&h.artifacts)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "log") {
            no_canaries(&std::fs::read(&path)?, &canaries, &path);
            scanned.insert(path);
            log_count += 1;
        }
    }
    assert!(
        log_count >= 20,
        "audit original daemon and CLI stdout/stderr, before redaction"
    );
    let remote = h.google.control().snapshot().await;
    for mailbox in remote.mail.values() {
        assert!(mailbox.accepted_sends.is_empty());
        assert_eq!(mailbox.message_copies, 0);
    }
    for calendars in remote.calendars.values() {
        for calendar in calendars.values() {
            assert!(calendar.notifications.is_empty());
        }
    }
    if let Some(path) = std::env::var_os("NUNCIO_TEST_ARTIFACTS") {
        let files: Vec<_> = scanned
            .iter()
            .map(|p| {
                p.strip_prefix(&h.artifacts)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        std::fs::write(
            PathBuf::from(path).join("confidentiality-audit.json"),
            serde_json::to_vec_pretty(&json!({
                "files":files,"unredacted_log_files":log_count,"canaries":canaries.len(),
                "populated_wal_scanned":true,"ordinary_sqlite_rejections":2,"wrong_sqlcipher_key_rejections":2,
                "excluded_inputs":["synthetic-secrets.json (test keystore)","mock-client-registration.json (test OAuth input)"],
                "profile_and_temp_scanned_recursively":true
            }))?,
        )?;
    }
    Ok(())
}
