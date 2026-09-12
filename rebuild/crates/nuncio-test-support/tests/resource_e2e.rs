#![allow(clippy::unwrap_used)]
use base64::{engine::general_purpose::STANDARD, Engine as _};
use nuncio_test_support::{
    google::Seed,
    process::{CliOutput, E2eHarness},
    TestError,
};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[track_caller]
fn result(output: CliOutput) -> Value {
    assert_eq!(output.status, 0, "CLI failed");
    output.json().unwrap()["result"].clone()
}
fn rss_kib(pid: u32) -> Result<u64, TestError> {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()?;
    assert!(
        output.status.success(),
        "owned daemon must remain alive while sampling"
    );
    Ok(std::str::from_utf8(&output.stdout)?.trim().parse()?)
}
fn thread_count(pid: u32) -> Result<usize, TestError> {
    let mode = if cfg!(target_os = "macos") {
        "-M"
    } else {
        "-L"
    };
    let output = std::process::Command::new("ps")
        .args([mode, "-p", &pid.to_string()])
        .output()?;
    assert!(
        output.status.success(),
        "owned daemon thread sampling failed"
    );
    Ok(std::str::from_utf8(&output.stdout)?
        .lines()
        .count()
        .saturating_sub(1))
}
async fn allocation_summaries(pid: u32, iteration: usize) -> Result<(), TestError> {
    if !cfg!(target_os = "macos") || std::env::var_os("REBUILD_ALLOCATION_PROFILES").is_none() {
        return Ok(());
    }
    let root = PathBuf::from(std::env::var_os("NUNCIO_TEST_ARTIFACTS").unwrap())
        .join(format!("allocations-{pid}"));
    std::fs::create_dir_all(&root)?;
    for (tool, args) in [
        ("heap", vec!["-s", "--noContent"]),
        ("vmmap", vec!["-summary"]),
    ] {
        let mut child = tokio::process::Command::new(format!("/usr/bin/{tool}"))
            .args(args)
            .arg(pid.to_string())
            .kill_on_drop(true)
            .stdout(std::fs::File::create(
                root.join(format!("{iteration}-{tool}.stdout.log")),
            )?)
            .stderr(std::fs::File::create(
                root.join(format!("{iteration}-{tool}.stderr.log")),
            )?)
            .spawn()?;
        let status = tokio::time::timeout(Duration::from_secs(30), child.wait()).await??;
        std::fs::write(
            root.join(format!("{iteration}-{tool}.status")),
            status.code().unwrap_or(-1).to_string(),
        )?;
    }
    Ok(())
}
struct Sampler {
    task: Option<tokio::task::JoinHandle<Result<Vec<u64>, TestError>>>,
    stop: tokio::sync::watch::Sender<bool>,
}
impl Sampler {
    fn start(pid: u32) -> Self {
        let (stop, mut stopped) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(async move {
            let mut samples = Vec::new();
            let mut tick = tokio::time::interval(Duration::from_millis(100));
            loop {
                tokio::select! {_=stopped.changed()=>return Ok(samples),_=tick.tick()=>samples.push(rss_kib(pid)?)};
            }
        });
        Self {
            task: Some(task),
            stop,
        }
    }
    async fn finish(mut self) -> Result<Vec<u64>, TestError> {
        self.stop.send_replace(true);
        self.task.take().unwrap().await?
    }
}
impl Drop for Sampler {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[tokio::test]
async fn actual_daemon_transfers_sixteen_mib_and_rss_stabilizes_after_repeated_fetches(
) -> Result<(), TestError> {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
    let account = h.connect_google("alpha@example.test").await?;
    let pid = h.daemon_pid().unwrap();
    let baseline = rss_kib(pid)?;
    let sampler = Sampler::start(pid);
    let started = Instant::now();
    let payload: Vec<u8> = (0..16 * 1024 * 1024).map(|n| (n % 251) as u8).collect();
    let mut raw=b"From: sender@example.test\r\nSubject: Large resource fixture\r\nMessage-ID: <large-resource@example.test>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=large-resource-boundary\r\n\r\n--large-resource-boundary\r\nContent-Type: text/plain\r\n\r\nLarge binary attachment follows.\r\n--large-resource-boundary\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=large.bin\r\nContent-Transfer-Encoding: base64\r\n\r\n".to_vec();
    for line in STANDARD.encode(&payload).as_bytes().chunks(76) {
        raw.extend_from_slice(line);
        raw.extend_from_slice(b"\r\n");
    }
    raw.extend_from_slice(b"--large-resource-boundary--\r\n");
    let raw_len = raw.len();
    h.google
        .control()
        .add_message(
            "alpha@example.test",
            "large-resource",
            "large-resource-thread",
            raw,
            ["INBOX".into()].into(),
        )
        .await?;
    result(
        h.cli(&["--json", "sync", "--account", &account, "--wait"])
            .await?,
    );
    let listing = result(
        h.cli(&["--json", "mail", "list", "--account", &account])
            .await?,
    );
    let id = listing["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["provider_id"] == "large-resource")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let metadata = result(
        h.cli(&[
            "--json",
            "mail",
            "read",
            "--account",
            &account,
            "--message",
            id,
        ])
        .await?,
    );
    assert_eq!(metadata["attachments"][0]["byte_length"], payload.len());
    let attachment = metadata["attachments"][0]["id"].as_str().unwrap();
    let file = h.artifacts.join("large-download.bin");
    let mut idle = Vec::new();
    let mut iteration_ms = Vec::new();
    let mut thread_counts = Vec::new();
    let mut storage_sizes = Vec::new();
    for iteration_index in 0..8 {
        let iteration = Instant::now();
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
                attachment,
                "--output",
                file.to_str().unwrap(),
            ])
            .await?,
        );
        assert_eq!(std::fs::read(&file)?, payload);
        std::fs::remove_file(&file)?;
        idle.push(rss_kib(pid)?);
        thread_counts.push(thread_count(pid)?);
        storage_sizes.push((
            std::fs::metadata(h.directory.join("store.db"))?.len(),
            std::fs::metadata(h.directory.join("store.db-wal"))?.len(),
        ));
        if matches!(iteration_index, 1 | 7) {
            allocation_summaries(pid, iteration_index + 1).await?;
        }
        iteration_ms.push(iteration.elapsed().as_millis());
    }
    let samples = sampler.finish().await?;
    assert!(samples.len() >= 2);
    if let Some(directory) = std::env::var_os("NUNCIO_TEST_ARTIFACTS") {
        let directory = PathBuf::from(directory);
        std::fs::create_dir_all(&directory)?;
        std::fs::write(
            directory.join("attachment-resources.json"),
            serde_json::to_vec_pretty(
                &json!({"pid":pid,"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"attachment_bytes":payload.len(),"raw_bytes":raw_len,"baseline_rss_kib":baseline,"peak_rss_kib":samples.iter().max(),"rss_samples_kib":samples,"idle_rss_kib":idle,"thread_counts":thread_counts,"storage_sizes_bytes":storage_sizes,"iteration_ms":iteration_ms,"total_ms":started.elapsed().as_millis(),"post_warmup_growth_limit_kib":2*64*1024}),
            )?,
        )?;
    }
    // Allow allocator/cache reuse of two maximum payload buffers after warm-up.
    // Repeated retained message buffers must not grow with every identical fetch.
    let steady = &idle[2..];
    assert!(
        steady.iter().max().unwrap() - steady.iter().min().unwrap() <= 2 * 64 * 1024,
        "RSS keeps growing after warm-up: {idle:?}"
    );
    let remote = h.google.control().snapshot().await;
    assert_eq!(
        remote.mail["alpha@example.test"].messages["large-resource"]
            .attachments
            .values()
            .next()
            .unwrap(),
        &payload
    );
    for mailbox in remote.mail.values() {
        assert!(mailbox.accepted_sends.is_empty());
        assert_eq!(mailbox.message_copies, 0);
    }
    for calendars in remote.calendars.values() {
        for calendar in calendars.values() {
            assert!(calendar.notifications.is_empty());
        }
    }
    h.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn actual_cli_refuses_attachment_above_sixty_four_mib_without_mutating_draft(
) -> Result<(), TestError> {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await?;
    let account = h.connect_google("alpha@example.test").await?;
    let input = h.artifacts.join("resource-draft.json");
    std::fs::write(
        &input,
        serde_json::to_vec(
            &json!({"subject":"Oversize refusal","text":"Retained draft","to":[{"address":"recipient@example.test"}]}),
        )?,
    )?;
    let original = result(
        h.cli(&[
            "--json",
            "mail",
            "draft",
            "save",
            "--account",
            &account,
            "--file",
            input.to_str().unwrap(),
        ])
        .await?,
    );
    let id = original["id"].as_str().unwrap();
    let remote = serde_json::to_value(h.google.control().snapshot().await)?;
    let file = h.artifacts.join("too-large.bin");
    std::fs::File::create(&file)?.set_len(64 * 1024 * 1024 + 1)?;
    let refused = h
        .cli(&[
            "--json",
            "mail",
            "draft",
            "attach",
            "--account",
            &account,
            "--draft",
            id,
            "--version",
            "1",
            "--file",
            file.to_str().unwrap(),
        ])
        .await?;
    assert_eq!(refused.status, 2);
    assert_eq!(
        result(
            h.cli(&[
                "--json",
                "mail",
                "draft",
                "show",
                "--account",
                &account,
                "--draft",
                id
            ])
            .await?
        ),
        original
    );
    h.shutdown().await?;
    h.restart().await?;
    assert_eq!(
        result(
            h.cli(&[
                "--json",
                "mail",
                "draft",
                "show",
                "--account",
                &account,
                "--draft",
                id
            ])
            .await?
        ),
        original
    );
    assert_eq!(
        serde_json::to_value(h.google.control().snapshot().await)?,
        remote
    );
    h.shutdown().await?;
    Ok(())
}
