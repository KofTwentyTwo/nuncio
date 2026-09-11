//! Private-pipe control of independent Dovecot/Mailpit subprocess services.
use crate::TestError;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

const MAX_CONTROL: u64 = 96 * 1024 * 1024;
const CONTROL_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MailPorts {
    pub imap: u16,
    pub imaps: u16,
    pub smtp: u16,
    pub smtps: u16,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MailReady {
    pub schema_version: u32,
    pub project: String,
    pub ports: MailPorts,
    pub ca_file: PathBuf,
    pub credentials_file: PathBuf,
    pub hidden_capabilities: Vec<String>,
    pub seed: String,
    pub server_auto_sent: bool,
}

pub struct MockMailPlus {
    pub ready: MailReady,
    pub artifacts: PathBuf,
    process: MockProcess,
    sequence: u64,
    commands: File,
}

struct MockProcess {
    directory: PathBuf,
    child: Child,
    input: Option<ChildStdin>,
    output: Option<BufReader<ChildStdout>>,
    stopped: bool,
}

fn failure(message: &str) -> TestError {
    std::io::Error::other(message).into()
}

fn scripts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/imap")
}

fn private_file(path: &Path) -> Result<File, TestError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

fn artifacts(prefix: &str) -> Result<PathBuf, TestError> {
    let parent = std::env::var_os("NUNCIO_TEST_ARTIFACTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| scripts().join("../../test-results/imap-contract/runs"));
    std::fs::create_dir_all(&parent)?;
    // Preserve successful and failed protocol evidence, including the exact
    // composition identity needed for cleanup after forced process death.
    Ok(tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(parent)?
        .keep()
        .canonicalize()?)
}

impl MockProcess {
    async fn read(&mut self) -> Result<Value, TestError> {
        let output = self
            .output
            .as_mut()
            .ok_or_else(|| failure("mock output is closed"))?;
        let mut bytes = Vec::new();
        tokio::time::timeout(
            CONTROL_TIMEOUT,
            output.take(MAX_CONTROL + 1).read_until(b'\n', &mut bytes),
        )
        .await??;
        if bytes.len() as u64 > MAX_CONTROL || bytes.last() != Some(&b'\n') {
            return Err(failure(
                "missing or oversized mock response; inspect retained diagnostics",
            ));
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
}

impl MockMailPlus {
    pub async fn start(hidden_capabilities: &[&str]) -> Result<Self, TestError> {
        Self::start_with_sent_policy(hidden_capabilities, false).await
    }

    pub async fn start_with_sent_policy(
        hidden_capabilities: &[&str],
        server_auto_sent: bool,
    ) -> Result<Self, TestError> {
        if hidden_capabilities
            .iter()
            .any(|v| !["MOVE", "UIDPLUS", "CONDSTORE", "QRESYNC"].contains(v))
        {
            return Err(failure("invalid independent capability profile"));
        }
        let artifacts = artifacts("mock-mailplus-")?;
        let directory = artifacts.join("services");
        let mut command = Command::new("python3");
        command
            .arg(scripts().join("mock-mailplus.py"))
            .arg("--directory")
            .arg(&directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(private_file(&artifacts.join("mock.stderr"))?)
            .kill_on_drop(false);
        for capability in hidden_capabilities {
            command.arg("--hide-capability").arg(capability);
        }
        if server_auto_sent {
            command.arg("--server-auto-sent");
        }
        let child = command.spawn()?;
        let mut process = MockProcess {
            directory: directory.clone(),
            child,
            input: None,
            output: None,
            stopped: false,
        };
        process.input = process.child.stdin.take();
        process.output = process.child.stdout.take().map(BufReader::new);
        let announcement = process.read().await?;
        let ready_path = directory.join("mock-ready.json");
        if announcement != json!({"ready_file":ready_path}) {
            return Err(failure("invalid independent mock readiness announcement"));
        }
        let metadata = std::fs::symlink_metadata(&ready_path)?;
        if !metadata.is_file() || metadata.len() > 16384 {
            return Err(failure("invalid independent mock readiness file"));
        }
        let ready: MailReady = serde_json::from_slice(&std::fs::read(ready_path)?)?;
        if ready.schema_version != 1
            || ready.seed != "two_accounts"
            || ready.ca_file != directory.join("ca.crt")
            || ready.credentials_file != directory.join("credentials.json")
            || [
                ready.ports.imap,
                ready.ports.imaps,
                ready.ports.smtp,
                ready.ports.smtps,
            ]
            .contains(&0)
        {
            return Err(failure("unsupported independent mock readiness contract"));
        }
        let commands = private_file(&artifacts.join("controls.jsonl"))?;
        Ok(Self {
            ready,
            artifacts,
            process,
            sequence: 0,
            commands,
        })
    }

    pub async fn control(&mut self, control: Value) -> Result<Value, TestError> {
        self.sequence += 1;
        let identity = self.sequence.to_string();
        let mut request = serde_json::to_vec(&json!({"id":identity,"control":control}))?;
        request.push(b'\n');
        if request.len() as u64 > MAX_CONTROL {
            return Err(failure("mock request exceeds control bound"));
        }
        let input = self
            .process
            .input
            .as_mut()
            .ok_or_else(|| failure("mock input is closed"))?;
        tokio::time::timeout(CONTROL_TIMEOUT, input.write_all(&request)).await??;
        let reply = self.process.read().await?;
        let succeeded = reply["id"] == identity && reply["ok"] == true;
        writeln!(
            self.commands,
            "{}",
            json!({"id":identity,"command":control["command"],"ok":succeeded})
        )?;
        self.commands.flush()?;
        if !succeeded {
            return Err(failure(match reply["error"].as_str() {
                Some("invalid_control") => "independent mock rejected invalid control",
                Some("control_timeout") => "independent mock control timed out",
                _ => "independent mock control failed; inspect retained diagnostics",
            }));
        }
        reply
            .get("result")
            .cloned()
            .ok_or_else(|| failure("mock result is absent"))
    }

    /// Read only the private, freshly generated synthetic fixture credential.
    pub fn credential(&self, account: &str) -> Result<String, TestError> {
        let path = &self.ready.credentials_file;
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_file() || metadata.len() > 4096 {
            return Err(failure("invalid synthetic credential file"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o777 != 0o600 {
                return Err(failure("synthetic credential file must be private"));
            }
        }
        let accounts: std::collections::BTreeMap<String, String> =
            serde_json::from_slice(&std::fs::read(path)?)?;
        accounts
            .get(account)
            .cloned()
            .ok_or_else(|| failure("unknown synthetic mail account"))
    }

    pub async fn shutdown(&mut self) -> Result<(), TestError> {
        self.control(json!({"command":"shutdown"})).await?;
        self.process.input.take();
        let status = tokio::time::timeout(CONTROL_TIMEOUT, self.process.child.wait()).await??;
        if !status.success() {
            return Err(failure("independent mail service cleanup failed"));
        }
        self.process.stopped = true;
        Ok(())
    }
}

impl Drop for MockProcess {
    fn drop(&mut self) {
        if self.stopped {
            return;
        }
        self.input.take();
        self.output.take();
        let deadline = Instant::now() + CONTROL_TIMEOUT;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(status)) if status.success() => return,
                Ok(Some(_)) | Err(_) => break,
                Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            }
        }
        let _ = self.child.start_kill();
        if !self.directory.join("project.json").is_file() {
            return;
        }
        // SIGKILL cannot execute Python finally. The stop entry point validates
        // and removes only this profile's persisted composition identity.
        let cleanup = std::process::Command::new("python3")
            .arg(scripts().join("services.py"))
            .arg("stop")
            .arg("--directory")
            .arg(&self.directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !matches!(cleanup, Ok(status) if status.success()) {
            eprintln!("independent mail mock cleanup failed; retained runtime needs explicit stop");
        }
    }
}

pub async fn verify_independent_contracts() -> Result<(), TestError> {
    let directory = artifacts("mailplus-conformance-")?;
    let log = private_file(&directory.join("contracts.log"))?;
    let status = Command::new("python3")
        .arg(scripts().join("run-tests.py"))
        .arg("--output")
        .arg(directory.join("python"))
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .kill_on_drop(true)
        .status()
        .await?;
    std::fs::write(
        directory.join("status.json"),
        json!({"exit_status":status.code()}).to_string(),
    )?;
    if !status.success() {
        return Err(failure(
            "independent mail conformance failed; inspect retained contract logs",
        ));
    }
    Ok(())
}
