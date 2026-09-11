use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

#[derive(Debug, thiserror::Error)]
#[error("Invalid or stopped test configuration")]
pub struct TestControlError;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TestConfig {
    pub google_base_url: String,
    pub request_timeout_ms: u64,
    pub poll_interval_ms: u64,
    pub background_sync: bool,
    pub max_payload_bytes: u64,
    pub now_unix_ms: Option<i64>,
    pub barriers_directory: Option<PathBuf>,
}
impl Default for TestConfig {
    fn default() -> Self {
        Self {
            google_base_url: String::new(),
            request_timeout_ms: 30000,
            poll_interval_ms: 60000,
            background_sync: true,
            max_payload_bytes: 64 * 1024 * 1024,
            now_unix_ms: None,
            barriers_directory: None,
        }
    }
}
impl TestConfig {
    pub fn google(base: &str) -> Self {
        Self {
            google_base_url: base.into(),
            ..Self::default()
        }
    }
    pub fn validate(&self) -> Result<(), TestControlError> {
        let url = url::Url::parse(&self.google_base_url).map_err(|_| TestControlError)?;
        if url.scheme() != "http"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
            || url.port().is_none()
            || !matches!(url.host(),Some(url::Host::Ipv4(ip)) if ip.is_loopback())
                && !matches!(url.host(),Some(url::Host::Ipv6(ip)) if ip.is_loopback())
        {
            return Err(TestControlError);
        }
        if !(1..=30000).contains(&self.request_timeout_ms)
            || !(10..=86400000).contains(&self.poll_interval_ms)
            || !(1..=64 * 1024 * 1024).contains(&self.max_payload_bytes)
            || self
                .now_unix_ms
                .is_some_and(|n| !(0..=253402300799000).contains(&n))
        {
            return Err(TestControlError);
        }
        if let Some(path) = &self.barriers_directory {
            let metadata = std::fs::symlink_metadata(path).map_err(|_| TestControlError)?;
            if !path.is_absolute() || !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(TestControlError);
            }
        }
        Ok(())
    }
    pub fn now_ms(&self) -> Result<i64, TestControlError> {
        match self.now_unix_ms {
            Some(value) => Ok(value),
            None => i64::try_from(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| TestControlError)?
                    .as_millis(),
            )
            .map_err(|_| TestControlError),
        }
    }
    pub async fn clock_offset_ms(&self) -> Result<i64, TestControlError> {
        use tokio::io::AsyncReadExt;
        let Some(directory) = &self.barriers_directory else {
            return Ok(0);
        };
        let path = directory.join("clock-offset-ms");
        let metadata = match tokio::fs::symlink_metadata(&path).await {
            Ok(value) => value,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(_) => return Err(TestControlError),
        };
        if self.now_unix_ms.is_none()
            || !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > 20
        {
            return Err(TestControlError);
        }
        let mut bytes = Vec::new();
        tokio::fs::File::open(path)
            .await
            .map_err(|_| TestControlError)?
            .take(21)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| TestControlError)?;
        if bytes.len() > 20 {
            return Err(TestControlError);
        }
        let offset: i64 = std::str::from_utf8(&bytes)
            .map_err(|_| TestControlError)?
            .trim()
            .parse()
            .map_err(|_| TestControlError)?;
        if !(0..=31_622_400_000).contains(&offset) {
            return Err(TestControlError);
        }
        Ok(offset)
    }
    // Storage startup owns a blocking worker thread before the async store is ready.
    // Keep this finite and independent of the caller's Tokio runtime.
    pub(crate) fn blocking_checkpoint(&self, name: &str) -> Result<(), TestControlError> {
        if name.is_empty()
            || name.len() > 128
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        {
            return Err(TestControlError);
        }
        let Some(directory) = &self.barriers_directory else {
            return Ok(());
        };
        match std::fs::rename(
            directory.join(format!("{name}.arm")),
            directory.join(format!("{name}.entered")),
        ) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(TestControlError),
        }
        let release = directory.join(format!("{name}.release"));
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            if release.try_exists().map_err(|_| TestControlError)? {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(TestControlError);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    /// The harness arms a checkpoint after clearing its previous entered/release markers.
    pub async fn checkpoint(
        &self,
        name: &str,
        mut stop: watch::Receiver<bool>,
    ) -> Result<(), TestControlError> {
        if name.is_empty()
            || name.len() > 128
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        {
            return Err(TestControlError);
        }
        let Some(directory) = &self.barriers_directory else {
            return Ok(());
        };
        if *stop.borrow() {
            return Err(TestControlError);
        }
        let arm = directory.join(format!("{name}.arm"));
        let entered = directory.join(format!("{name}.entered"));
        match tokio::fs::rename(arm, entered).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(TestControlError),
        }
        let release = directory.join(format!("{name}.release"));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut poll = tokio::time::interval(Duration::from_millis(10));
        loop {
            if *stop.borrow() {
                return Err(TestControlError);
            }
            if tokio::fs::try_exists(&release)
                .await
                .map_err(|_| TestControlError)?
            {
                return Ok(());
            }
            tokio::select! {
                _=stop.changed()=>return Err(TestControlError),
                _=tokio::time::sleep_until(deadline)=>return Err(TestControlError),
                _=poll.tick()=>{},
            }
        }
    }
}
