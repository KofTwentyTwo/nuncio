#![allow(clippy::unwrap_used)]
#[test]
fn repair_exposes_explicit_scope_dry_run_and_wait_without_ambiguous_date_options() {
    let binary = env!("CARGO_BIN_EXE_nuncio-cli");
    let help = std::process::Command::new(binary)
        .args(["repair", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout).unwrap();
    for option in ["--scope", "--dry-run", "--wait", "--from", "--to"] {
        assert!(text.contains(option));
    }
    for args in [
        vec!["repair", "--account", "synthetic", "--scope", "unknown"],
        vec![
            "repair",
            "--account",
            "synthetic",
            "--scope",
            "mail",
            "--dry-run",
            "--wait",
        ],
        vec![
            "repair",
            "--account",
            "synthetic",
            "--scope",
            "calendar",
            "--from",
            "2026-10-01",
        ],
    ] {
        assert_eq!(
            std::process::Command::new(binary)
                .args(args)
                .output()
                .unwrap()
                .status
                .code(),
            Some(2)
        );
    }
}
