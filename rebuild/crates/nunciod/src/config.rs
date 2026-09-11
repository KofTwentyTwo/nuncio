use clap::Parser;
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser)]
#[command(version, about = "Nuncio local mail and calendar engine")]
pub struct Config {
    #[arg(long, default_value = "127.0.0.1:9421")]
    pub bind: SocketAddr,
    #[arg(long)]
    pub data_dir: Option<PathBuf>,
    #[arg(long, default_value = "default")]
    pub profile: String,
    #[arg(long)]
    pub ready_file: Option<PathBuf>,
    #[cfg(feature = "test-harness")]
    #[arg(long)]
    pub test_secrets_file: Option<PathBuf>,
    #[cfg(feature = "test-harness")]
    #[arg(long)]
    pub test_config: Option<PathBuf>,
}

impl Config {
    pub fn directory(&self) -> Result<PathBuf, &'static str> {
        if !self.bind.ip().is_loopback() {
            return Err("Bind address must be loopback");
        }
        if self.profile.is_empty()
            || self.profile.len() > 64
            || !self
                .profile
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err("Profile name must use 1–64 letters, digits, hyphens or underscores");
        }
        if let Some(path) = &self.data_dir {
            return Ok(path.clone());
        }
        let home =
            std::env::var_os("HOME").ok_or("Home directory is unavailable; specify --data-dir")?;
        Ok(PathBuf::from(home)
            .join(".nuncio-rebuild")
            .join(&self.profile))
    }
}
