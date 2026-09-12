use crate::{
    google::{MockGoogle, Seed},
    TestError,
};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::process::{Child, Command};

pub struct CliOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}
impl CliOutput {
    pub fn json(&self) -> Result<Value, TestError> {
        Ok(serde_json::from_slice(&self.stdout)?)
    }
}
pub struct E2eHarness {
    redact_cli_logs: bool,
    clock_origin: std::time::Instant,
    poll_interval_ms: Option<u64>,
    request_timeout_ms: u64,
    pub google: MockGoogle,
    pub directory: PathBuf,
    pub secrets_file: PathBuf,
    pub endpoint: String,
    pub artifacts: PathBuf,
    temporary: Option<tempfile::TempDir>,
    daemon_path: PathBuf,
    cli_path: PathBuf,
    child: Option<Child>,
    generation: u64,
    invocation: AtomicU64,
    finished: bool,
}
fn error(message: impl Into<String>) -> TestError {
    std::io::Error::other(message.into()).into()
}

pub fn binary(variable: &str) -> Result<PathBuf, TestError> {
    let path=std::env::var_os(variable).map(PathBuf::from).ok_or_else(||error(format!("{variable} must name an absolute built binary; run python3 rebuild/scripts/verify.py --suite google_e2e")))?;
    if !path.is_absolute() || !path.is_file() {
        return Err(error(format!(
            "{variable} must name an existing absolute binary"
        )));
    }
    Ok(path)
}
pub fn isolated_command(binary: &Path, temporary: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .env_clear()
        .env("LANG", "C")
        .env("TZ", "UTC")
        .env("TMPDIR", temporary)
        .kill_on_drop(true)
        .stdin(Stdio::null());
    command
}
fn private_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}
fn bounded_read(path: &Path) -> Result<Vec<u8>, TestError> {
    read_limit(path, 64 * 1024 * 1024)
}
fn read_limit(path: &Path, limit: u64) -> Result<Vec<u8>, TestError> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(error("test process file exceeds its read limit"));
    }
    Ok(bytes)
}
fn redact(bytes: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes);
    let expression =
        r#"(?i)(bearer[ \t]+[^\s"\\]+|mock-(access|refresh|code)-[0-9]+|\b[a-f0-9]{64}\b)"#;
    match regex::Regex::new(expression) {
        Ok(pattern) => pattern
            .replace_all(&text, "[redacted]")
            .into_owned()
            .into_bytes(),
        Err(_) => b"[log redaction failed]".to_vec(),
    }
}
impl E2eHarness {
    pub async fn connect_google(&self, address: &str) -> Result<String, TestError> {
        let config = self.artifacts.join("mock-client-registration.json");
        if !config.exists() {
            use std::io::Write;
            private_file(&config)?
                .write_all(br#"{"installed":{"client_id":"nuncio-test-client"}}"#)?;
        }
        let path = config
            .to_str()
            .ok_or_else(|| error("test path is not UTF-8"))?;
        let started = self
            .cli(&[
                "--json",
                "account",
                "connect-google",
                "--client-config",
                path,
                "--login-hint",
                address,
                "--no-browser",
            ])
            .await?;
        if started.status != 0 {
            return Err(error("actual CLI failed to begin mock Google connection"));
        }
        let status = started.json()?;
        let session = status["result"]["session_id"]
            .as_str()
            .ok_or_else(|| error("missing auth session"))?;
        let url = status["result"]["browser_url"]
            .as_str()
            .ok_or_else(|| error("missing browser URL"))?;
        let url = url::Url::parse(url)?;
        if url.origin() != url::Url::parse(self.google.base_url())?.origin() {
            return Err(error("OAuth URL escaped the local mock provider"));
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(3))
            .build()?;
        let response = client.get(url).send().await?;
        if response.status() != 302 {
            return Err(error("mock consent did not redirect"));
        }
        let location = response
            .headers()
            .get("location")
            .ok_or_else(|| error("missing callback URL"))?
            .to_str()?;
        let callback = url::Url::parse(location)?;
        if callback.scheme() != "http"
            || !matches!(callback.host(),Some(url::Host::Ipv4(ip)) if ip.is_loopback())
            || !callback.username().is_empty()
            || callback.password().is_some()
        {
            return Err(error("callback URL escaped the local daemon"));
        }
        if client.get(callback).send().await?.status() != 200 {
            return Err(error("OAuth callback failed"));
        }
        let status = self
            .cli(&["--json", "account", "auth-status", "--session", session])
            .await?;
        let status = status.json()?;
        if status["result"]["state"] != "succeeded" {
            return Err(error("OAuth did not succeed"));
        }
        status["result"]["account_id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| error("missing connected account ID"))
    }
    pub async fn start(seed: Seed) -> Result<Self, TestError> {
        Self::start_with_polling(seed, None).await
    }
    pub fn daemon_pid(&self) -> Option<u32> {
        self.child.as_ref().and_then(Child::id)
    }
    pub async fn start_with_polling(
        seed: Seed,
        poll_interval_ms: Option<u64>,
    ) -> Result<Self, TestError> {
        Self::configured(seed, poll_interval_ms, true, 1000).await
    }
    pub async fn start_with_request_timeout(
        seed: Seed,
        request_timeout_ms: u64,
    ) -> Result<Self, TestError> {
        Self::configured(seed, None, true, request_timeout_ms).await
    }
    /// Security tests inspect original process output before Drop redacts it.
    pub async fn start_with_raw_cli_logs(seed: Seed) -> Result<Self, TestError> {
        Self::configured(seed, None, false, 1000).await
    }
    async fn configured(
        seed: Seed,
        poll_interval_ms: Option<u64>,
        redact_cli_logs: bool,
        request_timeout_ms: u64,
    ) -> Result<Self, TestError> {
        let daemon_path = binary("NUNCIO_E2E_DAEMON")?;
        let cli_path = binary("NUNCIO_E2E_CLI")?;
        let temporary = if let Some(path) = std::env::var_os("NUNCIO_TEST_ARTIFACTS") {
            std::fs::create_dir_all(&path)?;
            tempfile::Builder::new()
                .prefix("google-e2e-")
                .tempdir_in(path)?
        } else {
            tempfile::Builder::new()
                .prefix("nuncio-google-e2e-")
                .tempdir()?
        };
        let artifacts = temporary.path().to_path_buf();
        std::fs::create_dir(artifacts.join("tmp"))?;
        let google = MockGoogle::start(seed).await?;
        let mut harness = Self {
            redact_cli_logs,
            poll_interval_ms,
            request_timeout_ms,
            directory: artifacts.join("profile"),
            secrets_file: artifacts.join("synthetic-secrets.json"),
            endpoint: String::new(),
            artifacts,
            temporary: Some(temporary),
            daemon_path,
            cli_path,
            google,
            child: None,
            generation: 0,
            invocation: AtomicU64::new(0),
            finished: false,
            clock_origin: std::time::Instant::now(),
        };
        harness.restart().await?;
        Ok(harness)
    }
    pub async fn restart(&mut self) -> Result<(), TestError> {
        if self.child.is_some() {
            return Err(error("stop or force-kill the daemon before restarting"));
        }
        self.finished = false;
        self.generation += 1;
        let ready = self
            .artifacts
            .join(format!("daemon-{}.ready.json", self.generation));
        let test_config = self
            .artifacts
            .join(format!("daemon-{}.test.json", self.generation));
        let barriers = self.artifacts.join("barriers");
        std::fs::create_dir_all(&barriers)?;
        std::fs::write(
            &test_config,
            serde_json::to_vec(&serde_json::json!({
                "google_base_url":self.google.base_url(),"request_timeout_ms":self.request_timeout_ms,"poll_interval_ms":self.poll_interval_ms.unwrap_or(60000),"background_sync":self.poll_interval_ms.is_some(),"now_unix_ms":1772895600000_i64 + i64::try_from(self.clock_origin.elapsed().as_millis()).map_err(|_|error("test clock overflow"))?,"barriers_directory":barriers
            }))?,
        )?;
        let stdout = private_file(
            &self
                .artifacts
                .join(format!("daemon-{}.stdout.log", self.generation)),
        )?;
        let stderr = private_file(
            &self
                .artifacts
                .join(format!("daemon-{}.stderr.log", self.generation)),
        )?;
        let child = isolated_command(&self.daemon_path, &self.artifacts.join("tmp"))
            .args(["--bind", "127.0.0.1:0", "--data-dir"])
            .arg(&self.directory)
            .arg("--test-secrets-file")
            .arg(&self.secrets_file)
            .arg("--ready-file")
            .arg(&ready)
            .arg("--test-config")
            .arg(&test_config)
            .stdout(stdout)
            .stderr(stderr)
            .spawn()?;
        self.child = Some(child);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let mut poll = tokio::time::interval(Duration::from_millis(20));
        loop {
            if let Some(child) = self.child.as_mut() {
                if let Some(status) = child.try_wait()? {
                    return Err(error(format!(
                        "daemon exited before readiness ({status}); inspect {}",
                        self.artifacts.display()
                    )));
                }
            }
            if let Ok(bytes) = read_limit(&ready, 65536) {
                if bytes.len() > 65536 {
                    return Err(error("daemon readiness exceeds 64 KiB"));
                }
                if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                    let endpoint = value["endpoint"]
                        .as_str()
                        .ok_or_else(|| error("missing daemon endpoint"))?;
                    let url = url::Url::parse(endpoint)?;
                    if !matches!(url.host(),Some(url::Host::Ipv4(ip)) if ip.is_loopback())
                        && !matches!(url.host(),Some(url::Host::Ipv6(ip)) if ip.is_loopback())
                    {
                        return Err(error("daemon announced a non-loopback endpoint"));
                    }
                    if self.child.as_ref().and_then(Child::id).map(u64::from)
                        != value["pid"].as_u64()
                    {
                        return Err(error("readiness PID does not match the owned daemon"));
                    }
                    self.endpoint = endpoint.into();
                    let health = tokio::time::timeout_at(
                        deadline,
                        self.cli(&["--json", "system", "status"]),
                    )
                    .await??;
                    if health.status == 0
                        && health.json()?["result"]["profile_id"] == value["profile_id"]
                    {
                        return Ok(());
                    }
                    if ![0, 4].contains(&health.status) {
                        return Err(error("daemon authenticated health failed"));
                    }
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(error(format!(
                    "daemon readiness deadline exceeded; inspect {}",
                    self.artifacts.display()
                )));
            }
            poll.tick().await;
        }
    }
    pub async fn cli(&self, arguments: &[&str]) -> Result<CliOutput, TestError> {
        self.cli_input(arguments, None).await
    }
    pub async fn cli_with_stdin(
        &self,
        arguments: &[&str],
        input: &[u8],
    ) -> Result<CliOutput, TestError> {
        if input.len() > 16384 {
            return Err(error("CLI credential input exceeds test bound"));
        }
        self.cli_input(arguments, Some(input)).await
    }
    async fn cli_input(
        &self,
        arguments: &[&str],
        input: Option<&[u8]>,
    ) -> Result<CliOutput, TestError> {
        let index = self.invocation.fetch_add(1, Ordering::Relaxed);
        let stdout_path = self.artifacts.join(format!("cli-{index}.stdout.log"));
        let stderr_path = self.artifacts.join(format!("cli-{index}.stderr.log"));
        let mut child = isolated_command(&self.cli_path, &self.artifacts.join("tmp"))
            .arg("--data-dir")
            .arg(&self.directory)
            .arg("--test-secrets-file")
            .arg(&self.secrets_file)
            .args(["--endpoint", &self.endpoint])
            .args(arguments)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(private_file(&stdout_path)?)
            .stderr(private_file(&stderr_path)?)
            .spawn()?;
        if let Some(input) = input {
            use tokio::io::AsyncWriteExt;
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| error("CLI stdin pipe missing"))?;
            match tokio::time::timeout(Duration::from_secs(5), stdin.write_all(input)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
                _ => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    return Err(error("CLI stdin write failed or timed out"));
                }
            }
            drop(stdin);
        }
        let status = match tokio::time::timeout(Duration::from_secs(40), child.wait()).await {
            Ok(result) => result?,
            Err(_) => {
                child.kill().await?;
                let _ = child.wait().await;
                return Err(error(format!(
                    "CLI deadline exceeded at invocation {index}"
                )));
            }
        };
        let stdout = bounded_read(&stdout_path)?;
        let stderr = bounded_read(&stderr_path)?;
        if self.redact_cli_logs {
            std::fs::write(stdout_path, redact(&stdout))?;
            std::fs::write(stderr_path, redact(&stderr))?;
        }
        Ok(CliOutput {
            status: status.code().unwrap_or(-1),
            stdout,
            stderr,
        })
    }
    pub async fn force_kill(&mut self) -> Result<(), TestError> {
        if let Some(mut child) = self.child.take() {
            child.kill().await?;
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await??;
            return Ok(());
        }
        Err(error("daemon is not running"))
    }
    pub async fn shutdown(&mut self) -> Result<(), TestError> {
        if self.child.is_some() {
            let output = self.cli(&["--json", "system", "shutdown"]).await?;
            if output.status != 0 {
                return Err(error("daemon shutdown command failed"));
            }
            if let Some(mut child) = self.child.take() {
                match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
                    Ok(result) => {
                        if !result?.success() {
                            return Err(error("daemon exited unsuccessfully during shutdown"));
                        }
                    }
                    Err(_) => {
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                        return Err(error("daemon failed graceful shutdown"));
                    }
                }
            }
        }
        self.finished = true;
        Ok(())
    }
}
impl Drop for E2eHarness {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
        if let Ok(entries) = std::fs::read_dir(&self.artifacts) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "log") {
                    if let Ok(bytes) = bounded_read(&path) {
                        let _ = std::fs::write(path, redact(&bytes));
                    }
                }
            }
        }
        if !self.finished || std::thread::panicking() {
            if let Some(temporary) = self.temporary.take() {
                let path = temporary.keep();
                eprintln!("Test artifacts preserved at {}", path.display());
            }
        }
    }
}
