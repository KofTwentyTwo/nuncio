use nuncio_test_support::{
    process::{binary, E2eHarness},
    TestError,
};
use serde_json::{json, Value};
use std::{path::Path, process::Stdio};
use tokio::io::AsyncWriteExt;

pub async fn run(
    h: &E2eHarness,
    args: &[&str],
    steps: Value,
    google: Option<&str>,
) -> Result<Value, TestError> {
    run_with_binary(h, args, steps, google, "NUNCIO_E2E_CLI").await
}

pub async fn run_with_binary(
    h: &E2eHarness,
    args: &[&str],
    steps: Value,
    google: Option<&str>,
    variable: &str,
) -> Result<Value, TestError> {
    let cli = binary(variable)?;
    let mut command = vec![
        cli.to_string_lossy().into_owned(),
        "--data-dir".into(),
        h.directory.to_string_lossy().into_owned(),
        "--test-secrets-file".into(),
        h.secrets_file.to_string_lossy().into_owned(),
        "--endpoint".into(),
        h.endpoint.clone(),
    ];
    command.extend(args.iter().map(|s| s.to_string()));
    let input = zeroize::Zeroizing::new(
        json!({"command":command,"steps":steps,"google":google}).to_string(),
    );
    let driver = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/terminal/driver.py");
    let mut child = tokio::process::Command::new("python3")
        .arg(driver)
        .env_clear()
        .env("LANG", "C")
        .env("TZ", "UTC")
        .env("TMPDIR", h.artifacts.join("tmp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("missing driver input")?;
    stdin.write_all(input.as_bytes()).await?;
    drop(stdin);
    let output = tokio::time::timeout(std::time::Duration::from_secs(45), child.wait_with_output())
        .await??;
    if !output.status.success() {
        return Err(
            std::io::Error::other(String::from_utf8_lossy(&output.stderr).into_owned()).into(),
        );
    }
    let result: Value = serde_json::from_slice(&output.stdout)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    std::fs::write(
        h.artifacts.join(format!("terminal-{stamp}.json")),
        serde_json::to_vec_pretty(&result)?,
    )?;
    Ok(result)
}
