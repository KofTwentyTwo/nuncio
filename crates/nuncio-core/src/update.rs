//! Cross-Platform Auto-Update Engine for Nuncio Binaries.
//! Queries GitHub Releases API, verifies SHA-256 checksums, and atomically swaps running executables.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors emitted by the [`UpdateEngine`].
#[derive(Error, Debug)]
pub enum UpdateError {
    /// HTTP network request failure.
    #[error("network request failed: {0}")]
    Network(#[from] reqwest::Error),

    /// Release payload or JSON parsing failure.
    #[error("failed to parse release info: {0}")]
    Parse(String),

    /// Unsupported OS or CPU target triple.
    #[error("unsupported target architecture/OS platform: {0}")]
    UnsupportedPlatform(String),

    /// Missing release asset for target platform.
    #[error("release asset not found for target platform: {0}")]
    AssetNotFound(String),

    /// SHA-256 checksum mismatch verification error.
    #[error("checksum verification failed for {filename}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        /// Asset filename being verified.
        filename: String,
        /// Expected SHA-256 hex string from `SHA256SUMS.txt`.
        expected: String,
        /// Actual computed SHA-256 hex string.
        actual: String,
    },

    /// Error parsing or fetching checksum file.
    #[error("checksum file error: {0}")]
    ChecksumFileError(String),

    /// Archive unpacking failure (tar.gz or zip).
    #[error("archive extraction failed: {0}")]
    ArchiveError(String),

    /// Standard I/O file system failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Failed to resolve current running executable path.
    #[error("executable path resolution failed")]
    ExecutablePathUnknown,
}

/// GitHub release asset detail.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitHubReleaseAsset {
    /// Asset filename (e.g. `nuncio-x86_64-unknown-linux-gnu.tar.gz`).
    pub name: String,
    /// Direct browser download URL.
    pub browser_download_url: String,
}

/// Raw GitHub release object payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitHubRelease {
    /// Tag name string (e.g. `v0.2.0`).
    pub tag_name: String,
    /// Release notes body text.
    pub body: Option<String>,
    /// Published timestamp string.
    pub published_at: Option<String>,
    /// List of attached release build assets.
    pub assets: Vec<GitHubReleaseAsset>,
}

/// Parsed release metadata for auto-updater.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseInfo {
    /// Clean semver version string without leading 'v' (e.g. `"0.2.0"`).
    pub version: String,
    /// Original Git tag name (e.g. `"v0.2.0"`).
    pub tag_name: String,
    /// Markdown release notes description.
    pub release_notes: String,
    /// Direct download URL for local platform's binary archive.
    pub download_url: String,
    /// Direct download URL for `SHA256SUMS.txt`.
    pub checksum_url: Option<String>,
    /// Target platform archive filename (e.g. `nuncio-x86_64-pc-windows-msvc.zip`).
    pub archive_filename: String,
}

/// Detailed outcome of an update check operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateCheckResult {
    /// Currently installed binary version.
    pub current_version: String,
    /// Latest release version available on GitHub.
    pub latest_version: String,
    /// True if a newer version is available.
    pub update_available: bool,
    /// Detailed release info if an update is available.
    pub release_info: Option<ReleaseInfo>,
}

/// Cross-platform auto-update client.
pub struct UpdateEngine {
    client: reqwest::Client,
    base_url: Option<String>,
}

impl UpdateEngine {
    /// Default GitHub API endpoint for latest release.
    pub const DEFAULT_GITHUB_API_URL: &'static str =
        "https://api.github.com/repos/KofTwentyTwo/nuncio/releases/latest";

    /// Initialize a new `UpdateEngine` with standard User-Agent header.
    pub fn new() -> Result<Self, UpdateError> {
        let user_agent = format!("nuncio-updater/{}", env!("CARGO_PKG_VERSION"));
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .build()?;
        Ok(Self {
            client,
            base_url: None,
        })
    }

    /// Initialize a new `UpdateEngine` with custom API base URL (useful for testing).
    pub fn with_base_url(base_url: impl Into<String>) -> Result<Self, UpdateError> {
        let client = reqwest::Client::builder()
            .user_agent("nuncio-updater/test")
            .build()?;
        Ok(Self {
            client,
            base_url: Some(base_url.into()),
        })
    }

    /// Determine target platform triple for the running host.
    pub fn target_triple() -> Result<&'static str, UpdateError> {
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            Ok("x86_64-unknown-linux-gnu")
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            Ok("aarch64-apple-darwin")
        } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            Ok("x86_64-apple-darwin")
        } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            Ok("x86_64-pc-windows-msvc")
        } else {
            Err(UpdateError::UnsupportedPlatform(format!(
                "{}-{}",
                std::env::consts::OS,
                std::env::consts::ARCH
            )))
        }
    }

    /// Determine target archive filename for the running host.
    pub fn target_archive_filename() -> Result<String, UpdateError> {
        let triple = Self::target_triple()?;
        let ext = if cfg!(target_os = "windows") {
            "zip"
        } else {
            "tar.gz"
        };
        Ok(format!("nuncio-{triple}.{ext}"))
    }

    /// Query GitHub Releases API to check if a software update is available.
    pub async fn check_for_updates(&self) -> Result<UpdateCheckResult, UpdateError> {
        let url = self
            .base_url
            .as_deref()
            .unwrap_or(Self::DEFAULT_GITHUB_API_URL);

        let response = self.client.get(url).send().await?;
        if !response.status().is_success() {
            return Err(UpdateError::Parse(format!(
                "GitHub API returned HTTP status {}",
                response.status()
            )));
        }

        let release: GitHubRelease = response.json().await?;
        let tag_name = release.tag_name;
        let version = tag_name.trim_start_matches(['v', 'V']).to_string();
        let current_version = env!("CARGO_PKG_VERSION").to_string();

        let update_available = is_newer_version(&current_version, &version);

        let archive_filename = Self::target_archive_filename()?;
        let download_url = release
            .assets
            .iter()
            .find(|a| a.name == archive_filename)
            .map(|a| a.browser_download_url.clone());

        let checksum_url = release
            .assets
            .iter()
            .find(|a| a.name == "SHA256SUMS.txt")
            .map(|a| a.browser_download_url.clone());

        let release_info = match download_url {
            Some(dl_url) => Some(ReleaseInfo {
                version: version.clone(),
                tag_name,
                release_notes: release.body.unwrap_or_default(),
                download_url: dl_url,
                checksum_url,
                archive_filename,
            }),
            None => None,
        };

        Ok(UpdateCheckResult {
            current_version,
            latest_version: version,
            update_available,
            release_info,
        })
    }

    /// Download release package bytes and verify SHA-256 checksum against `SHA256SUMS.txt`.
    pub async fn download_and_verify(
        &self,
        release_info: &ReleaseInfo,
    ) -> Result<Vec<u8>, UpdateError> {
        // Download main binary package archive
        let archive_resp = self.client.get(&release_info.download_url).send().await?;
        if !archive_resp.status().is_success() {
            return Err(UpdateError::Parse(format!(
                "Failed to download update archive (HTTP status {})",
                archive_resp.status()
            )));
        }
        let archive_bytes = archive_resp.bytes().await?.to_vec();

        // Verify SHA-256 checksum if SHA256SUMS.txt asset is available
        if let Some(checksum_url) = &release_info.checksum_url {
            let sums_resp = self.client.get(checksum_url).send().await?;
            if sums_resp.status().is_success() {
                let sums_text = sums_resp.text().await?;
                if let Some(expected_hash) =
                    parse_sha256sums(&sums_text, &release_info.archive_filename)
                {
                    let actual_hash = compute_sha256(&archive_bytes);
                    if expected_hash.to_lowercase() != actual_hash.to_lowercase() {
                        return Err(UpdateError::ChecksumMismatch {
                            filename: release_info.archive_filename.clone(),
                            expected: expected_hash,
                            actual: actual_hash,
                        });
                    }
                } else {
                    return Err(UpdateError::ChecksumFileError(format!(
                        "Filename '{}' not found in SHA256SUMS.txt",
                        release_info.archive_filename
                    )));
                }
            }
        }

        Ok(archive_bytes)
    }

    /// Download, verify checksum, extract target binary, and atomically replace executable.
    pub async fn apply_update(&self, release_info: &ReleaseInfo) -> Result<String, UpdateError> {
        let current_exe =
            std::env::current_exe().map_err(|_| UpdateError::ExecutablePathUnknown)?;
        let archive_bytes = self.download_and_verify(release_info).await?;

        let exe_name = current_exe
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(UpdateError::ExecutablePathUnknown)?;

        let new_binary_bytes =
            extract_binary(&archive_bytes, exe_name, &release_info.archive_filename)?;

        replace_executable(&current_exe, &new_binary_bytes)?;

        // Best effort: update sibling Nuncio binaries if present in the same install directory
        if let Some(parent_dir) = current_exe.parent() {
            let sibling_names = ["nuncio-cli", "nuncio-tui", "nuncio-mcp", "nunciod"];
            for sib_base in sibling_names {
                let sib_filename = if cfg!(windows) {
                    format!("{sib_base}.exe")
                } else {
                    sib_base.to_string()
                };

                if sib_filename != exe_name {
                    let sib_path = parent_dir.join(&sib_filename);
                    if sib_path.exists() {
                        if let Ok(sib_bytes) = extract_binary(
                            &archive_bytes,
                            &sib_filename,
                            &release_info.archive_filename,
                        ) {
                            let _ = replace_executable(&sib_path, &sib_bytes);
                        }
                    }
                }
            }
        }

        Ok(format!(
            "Successfully updated binary '{}' to version v{}",
            exe_name, release_info.version
        ))
    }
}

/// Compare two semver strings (returns `true` if `latest` > `current`).
pub fn is_newer_version(current: &str, latest: &str) -> bool {
    let parse_v = |v: &str| -> Vec<u64> {
        v.trim_start_matches(['v', 'V'])
            .split('.')
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    };

    let c = parse_v(current);
    let l = parse_v(latest);

    for i in 0..c.len().max(l.len()) {
        let cv = c.get(i).copied().unwrap_or(0);
        let lv = l.get(i).copied().unwrap_or(0);
        if lv > cv {
            return true;
        } else if cv > lv {
            return false;
        }
    }
    false
}

/// Compute SHA-256 hex string of a byte slice.
pub fn compute_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Parse SHA256SUMS.txt content for target filename and return expected hash.
pub fn parse_sha256sums(content: &str, target_filename: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 2 {
            let hash = parts[0];
            let filename = parts[parts.len() - 1].trim_start_matches('*');
            if filename == target_filename || filename.ends_with(target_filename) {
                return Some(hash.to_lowercase());
            }
        }
    }
    None
}

/// Extract specific named binary file from a `.tar.gz` or `.zip` archive payload.
pub fn extract_binary(
    archive_bytes: &[u8],
    binary_name: &str,
    archive_filename: &str,
) -> Result<Vec<u8>, UpdateError> {
    if archive_filename.ends_with(".zip") {
        extract_binary_from_zip(archive_bytes, binary_name)
    } else if archive_filename.ends_with(".tar.gz") || archive_filename.ends_with(".tgz") {
        extract_binary_from_targz(archive_bytes, binary_name)
    } else {
        Err(UpdateError::ArchiveError(format!(
            "Unsupported archive format for file '{archive_filename}'"
        )))
    }
}

/// Unpack binary from `.tar.gz` payload.
pub fn extract_binary_from_targz(
    archive_bytes: &[u8],
    binary_name: &str,
) -> Result<Vec<u8>, UpdateError> {
    use flate2::read::GzDecoder;
    use std::io::Read;
    use tar::Archive;

    let gz = GzDecoder::new(archive_bytes);
    let mut archive = Archive::new(gz);

    let entries = archive
        .entries()
        .map_err(|e| UpdateError::ArchiveError(e.to_string()))?;

    for entry_res in entries {
        let mut entry = entry_res.map_err(|e| UpdateError::ArchiveError(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| UpdateError::ArchiveError(e.to_string()))?;
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if name == binary_name {
            let mut contents = Vec::new();
            entry
                .read_to_end(&mut contents)
                .map_err(|e| UpdateError::ArchiveError(e.to_string()))?;
            return Ok(contents);
        }
    }

    Err(UpdateError::AssetNotFound(format!(
        "Binary '{binary_name}' not found in .tar.gz archive"
    )))
}

/// Unpack binary from `.zip` payload.
pub fn extract_binary_from_zip(
    archive_bytes: &[u8],
    binary_name: &str,
) -> Result<Vec<u8>, UpdateError> {
    use std::io::Read;
    use zip::ZipArchive;

    let cursor = std::io::Cursor::new(archive_bytes);
    let mut archive =
        ZipArchive::new(cursor).map_err(|e| UpdateError::ArchiveError(e.to_string()))?;

    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| UpdateError::ArchiveError(e.to_string()))?;
        let name = Path::new(file.name())
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        if name == binary_name {
            let mut contents = Vec::new();
            file.read_to_end(&mut contents)
                .map_err(|e| UpdateError::ArchiveError(e.to_string()))?;
            return Ok(contents);
        }
    }

    Err(UpdateError::AssetNotFound(format!(
        "Binary '{binary_name}' not found in .zip archive"
    )))
}

/// Atomically replace target executable file with new binary data.
pub fn replace_executable(target_path: &Path, new_binary_bytes: &[u8]) -> Result<(), UpdateError> {
    let parent_dir = target_path
        .parent()
        .map(PathBuf::from)
        .ok_or_else(|| {
            UpdateError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Target path has no parent directory",
            ))
        })?;

    let temp_name = format!(
        ".tmp_update_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let temp_file_path = parent_dir.join(temp_name);

    std::fs::write(&temp_file_path, new_binary_bytes)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(0o755);
        let _ = std::fs::set_permissions(&temp_file_path, permissions);
    }

    #[cfg(not(windows))]
    {
        if let Err(e) = std::fs::rename(&temp_file_path, target_path) {
            let _ = std::fs::remove_file(&temp_file_path);
            return Err(UpdateError::Io(e));
        }
    }

    #[cfg(windows)]
    {
        let old_path = target_path.with_extension("exe.old");
        if old_path.exists() {
            let _ = std::fs::remove_file(&old_path);
        }

        if let Err(e) = std::fs::rename(target_path, &old_path) {
            let _ = std::fs::remove_file(&temp_file_path);
            return Err(UpdateError::Io(e));
        }

        if let Err(e) = std::fs::rename(&temp_file_path, target_path) {
            let _ = std::fs::rename(&old_path, target_path);
            let _ = std::fs::remove_file(&temp_file_path);
            return Err(UpdateError::Io(e));
        }

        let _ = std::fs::remove_file(&old_path);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn version_comparison_logic() {
        assert!(is_newer_version("0.1.0", "0.2.0"));
        assert!(is_newer_version("0.1.0", "v0.1.1"));
        assert!(is_newer_version("0.1.0", "1.0.0"));
        assert!(!is_newer_version("0.2.0", "0.1.0"));
        assert!(!is_newer_version("0.1.0", "0.1.0"));
        assert!(!is_newer_version("0.1.0", "v0.1.0"));
    }

    #[test]
    fn parse_sha256sums_extracts_correct_hash() {
        let content = r#"
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  nuncio-x86_64-unknown-linux-gnu.tar.gz
11223344556677889900aabbccddeeff11223344556677889900aabbccddeeff *nuncio-x86_64-pc-windows-msvc.zip
        "#;

        assert_eq!(
            parse_sha256sums(content, "nuncio-x86_64-unknown-linux-gnu.tar.gz"),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string())
        );
        assert_eq!(
            parse_sha256sums(content, "nuncio-x86_64-pc-windows-msvc.zip"),
            Some("11223344556677889900aabbccddeeff11223344556677889900aabbccddeeff".to_string())
        );
        assert_eq!(parse_sha256sums(content, "nonexistent.zip"), None);
    }

    #[test]
    fn compute_sha256_matches_known_string() {
        let bytes = b"hello nuncio auto update";
        let hash = compute_sha256(bytes);
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn extract_binary_from_tar_gz() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use tar::Builder;

        let mut tar_bytes = Vec::new();
        {
            let gz = GzEncoder::new(&mut tar_bytes, Compression::default());
            let mut builder = Builder::new(gz);

            let mut header = tar::Header::new_gnu();
            let data = b"binary content payload";
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();

            builder.append_data(&mut header, "nuncio-cli", &data[..]).unwrap();
            builder.finish().unwrap();
        }

        let extracted = extract_binary(&tar_bytes, "nuncio-cli", "nuncio-x86_64-unknown-linux-gnu.tar.gz")
            .expect("extraction succeeds");
        assert_eq!(extracted, b"binary content payload");
    }

    #[test]
    fn extract_binary_from_zip_archive() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;
        use zip::ZipWriter;

        let mut zip_bytes = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut zip_bytes);
            let mut zip = ZipWriter::new(cursor);
            zip.start_file("nuncio-cli.exe", SimpleFileOptions::default()).unwrap();
            zip.write_all(b"windows binary payload").unwrap();
            zip.finish().unwrap();
        }

        let extracted = extract_binary(&zip_bytes, "nuncio-cli.exe", "nuncio-x86_64-pc-windows-msvc.zip")
            .expect("zip extraction succeeds");
        assert_eq!(extracted, b"windows binary payload");
    }

    #[test]
    fn atomic_replacement_swaps_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let target_file = temp_dir.path().join("nuncio_test_bin");

        std::fs::write(&target_file, b"old version 1").unwrap();
        replace_executable(&target_file, b"new version 2").unwrap();

        let updated_content = std::fs::read(&target_file).unwrap();
        assert_eq!(updated_content, b"new version 2");
    }

    #[tokio::test]
    async fn check_for_updates_with_wiremock() {
        let mock_server = MockServer::start().await;
        let archive_filename = UpdateEngine::target_archive_filename().unwrap();

        let body = serde_json::json!({
            "tag_name": "v99.0.0",
            "body": "Major security release",
            "published_at": "2026-07-23T12:00:00Z",
            "assets": [
                {
                    "name": archive_filename,
                    "browser_download_url": format!("{}/download/{}", mock_server.uri(), archive_filename)
                },
                {
                    "name": "SHA256SUMS.txt",
                    "browser_download_url": format!("{}/download/SHA256SUMS.txt", mock_server.uri())
                }
            ]
        });

        Mock::given(method("GET"))
            .and(path("/releases/latest"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&body))
            .mount(&mock_server)
            .await;

        let updater = UpdateEngine::with_base_url(format!("{}/releases/latest", mock_server.uri())).unwrap();
        let result = updater.check_for_updates().await.expect("check updates succeeds");

        assert!(result.update_available);
        assert_eq!(result.latest_version, "99.0.0");
        let info = result.release_info.expect("release info present");
        assert_eq!(info.release_notes, "Major security release");
        assert!(info.download_url.contains(&archive_filename));
    }
}
