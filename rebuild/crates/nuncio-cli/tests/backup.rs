#![allow(clippy::unwrap_used)]
use std::process::Command;
#[test]
fn backup_commands_are_available_and_secrets_or_traversal_are_not_accepted_in_arguments() {
    for (command, flag) in [
        ("create", "--output"),
        ("inspect", "--file"),
        ("restore", "--new-profile"),
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_nuncio-cli"))
            .args(["backup", command, "--help"])
            .output()
            .unwrap();
        assert!(result.status.success(), "missing backup {command}");
        let text = String::from_utf8(result.stdout).unwrap();
        assert!(text.contains(flag));
        assert!(!text.contains("--passphrase"));
    }
    for args in [
        vec![
            "--json",
            "backup",
            "create",
            "--output",
            "unused.nuncio",
            "--passphrase",
            "never-print-secret-canary",
        ],
        vec![
            "--json",
            "backup",
            "restore",
            "--file",
            "unused.nuncio",
            "--new-profile",
            "../outside",
        ],
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_nuncio-cli"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(2));
        let stdout = String::from_utf8(result.stdout).unwrap();
        let stderr = String::from_utf8(result.stderr).unwrap();
        assert!(!stdout.contains("never-print-secret-canary"));
        assert!(!stderr.contains("never-print-secret-canary"));
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(value["error"]["code"], "invalid_input");
    }
}
