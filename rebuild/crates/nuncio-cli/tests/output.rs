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
