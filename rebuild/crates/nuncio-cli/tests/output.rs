#![allow(clippy::unwrap_used)]
use std::process::Command;

#[test]
fn unreachable_daemon_returns_json_error_and_nonzero_exit() {
    let output = Command::new(env!("CARGO_BIN_EXE_nuncio-cli"))
        .env_clear()
        .args([
            "--json",
            "--endpoint",
            "http://127.0.0.1:1",
            "system",
            "status",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "unavailable");
    assert!(value.get("result").is_none());
}

#[test]
fn invalid_input_is_an_error_and_help_succeeds() {
    let invalid = Command::new(env!("CARGO_BIN_EXE_nuncio-cli"))
        .env_clear()
        .args(["--json", "made-up-command"])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    let value: serde_json::Value = serde_json::from_slice(&invalid.stdout).unwrap();
    assert_eq!(value["error"]["code"], "invalid_input");
    let help = Command::new(env!("CARGO_BIN_EXE_nuncio-cli"))
        .env_clear()
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
}

#[test]
fn account_without_an_action_shows_help_before_connecting_or_creating_a_profile() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("unused-profile");
    for action in [vec!["account"], vec!["account", "--help"]] {
        let explicit_help = action.len() == 2;
        let output = Command::new(env!("CARGO_BIN_EXE_nuncio-cli"))
            .env_clear()
            .args(["--endpoint", "http://127.0.0.1:1", "--data-dir"])
            .arg(&data)
            .args(action)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(if explicit_help { 0 } else { 2 })
        );
        let help = if explicit_help {
            assert!(output.stderr.is_empty());
            String::from_utf8(output.stdout).unwrap()
        } else {
            assert!(output.stdout.is_empty());
            String::from_utf8(output.stderr).unwrap()
        };
        assert!(help.contains("Usage: nuncio-cli account"), "{help}");
        assert!(help.contains("Commands:"), "{help}");
        assert!(!data.exists());
    }
}

#[test]
fn account_without_an_action_preserves_json_errors_for_scripts() {
    for args in [["--json", "account"], ["account", "--json"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_nuncio-cli"))
            .env_clear()
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["code"], "invalid_input");
        assert!(value.get("result").is_none());
    }
}

#[test]
fn missing_mail_account_explains_selection_without_creating_a_profile() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("unused-profile");
    let output = Command::new(env!("CARGO_BIN_EXE_nuncio-cli"))
        .env_clear()
        .args(["--profile", "laptop-qa", "--data-dir"])
        .arg(&data)
        .args(["mail", "list"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("--account <ACCOUNT_ID>"), "{error}");
    assert!(error.contains("account list"), "{error}");
    assert!(error.contains("Usage: nuncio-cli mail list"), "{error}");
    assert!(!error.contains("schema_version"));
    assert!(!data.exists());
}

#[test]
fn normal_runtime_errors_are_actionable_text_and_json_remains_explicit() {
    let output = Command::new(env!("CARGO_BIN_EXE_nuncio-cli"))
        .env_clear()
        .args(["--endpoint", "http://127.0.0.1:1", "system", "status"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("nunciod"), "{error}");
    assert!(error.contains("--profile"), "{error}");
    assert!(!error.contains("schema_version"));
}

#[test]
fn helpful_parse_errors_never_echo_untrusted_argument_values() {
    let secret = "private-token-DO-NOT-ECHO\u{1b}[2J";
    for args in [
        vec![
            "mail",
            "list",
            "--account",
            "local-account",
            "--page-size",
            secret,
        ],
        vec!["mail", secret],
        vec!["mail", "list", secret],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_nuncio-cli"))
            .env_clear()
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("Usage:"), "{error}");
        assert!(!error.contains("private-token"));
        assert!(!error.contains('\u{1b}'));
    }
}

#[test]
fn write_request_ids_explain_the_required_uuid_before_connecting() {
    let output = Command::new(env!("CARGO_BIN_EXE_nuncio-cli"))
        .env_clear()
        .args([
            "mail",
            "send",
            "--account",
            "account",
            "--draft",
            "draft",
            "--request-id",
            "send-1",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("--request-id"), "{error}");
    assert!(error.contains("UUID"), "{error}");
}

#[test]
fn misspelled_options_suggest_declared_names_without_echoing_supplied_values() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("unused-profile");
    for option in ["--acount", "--acount=private-token-canary"] {
        let output = Command::new(env!("CARGO_BIN_EXE_nuncio-cli"))
            .env_clear()
            .args(["--data-dir"])
            .arg(&data)
            .args(["mail", "list", option, "private-value-canary"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("Did you mean '--account'?"), "{error}");
        assert!(!error.contains("private-token-canary"));
        assert!(!error.contains("private-value-canary"));
        assert!(!data.exists());
    }
    let output = Command::new(env!("CARGO_BIN_EXE_nuncio-cli"))
        .env_clear()
        .args(["--json", "mail", "list", "--acount", "private-value-canary"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "invalid_input");
    assert!(!String::from_utf8(output.stdout)
        .unwrap()
        .contains("private-value-canary"));
}
