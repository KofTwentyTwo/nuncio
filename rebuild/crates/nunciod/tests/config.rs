#![allow(clippy::unwrap_used)]
use std::process::Command;

#[test]
fn remote_bind_is_rejected_before_profile_creation_or_keyring_access() {
    let temporary = tempfile::tempdir().unwrap();
    let profile = temporary.path().join("must-not-create");
    let output = Command::new(env!("CARGO_BIN_EXE_nunciod"))
        .env_clear()
        .args(["--bind", "0.0.0.0:9421", "--data-dir"])
        .arg(&profile)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!profile.exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("loopback"));
}

#[test]
fn profile_name_cannot_escape_the_profile_root() {
    let temporary = tempfile::tempdir().unwrap();
    let profile = temporary.path().join("must-not-create");
    let output = Command::new(env!("CARGO_BIN_EXE_nunciod"))
        .env_clear()
        .args(["--profile", "../original", "--data-dir"])
        .arg(&profile)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!profile.exists());
}

#[cfg(not(feature = "test-harness"))]
#[test]
fn production_binary_rejects_test_secret_flags() {
    let output = Command::new(env!("CARGO_BIN_EXE_nunciod"))
        .env_clear()
        .args(["--test-secrets-file", "unused-synthetic-file"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}
