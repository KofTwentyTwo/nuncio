//! Universal Portable Data Export Engine for Nuncio.
//!
//! Provides high-speed streaming export of emails, accounts, and NSQL query result sets
//! into standard portable formats (MBOX, EML ZIP archive, and JSON/JSONL).

use crate::model::Email;
use serde::{Deserialize, Serialize};
use std::io::Write;
use thiserror::Error;
use zip::write::SimpleFileOptions;

/// Errors emitted during export operations.
#[derive(Error, Debug)]
pub enum ExportError {
    /// I/O error during file writing or directory creation.
    #[error("export I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Zip archive creation error.
    #[error("zip archive error: {0}")]
    Zip(#[from] zip::result::ZipError),
    /// Serialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    /// Unsupported format or invalid target.
    #[error("invalid export request: {0}")]
    InvalidRequest(String),
}

/// Portable export formats supported by Nuncio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ExportFormat {
    /// RFC 4155 MBOX format (.mbox), compatible with Thunderbird, Apple Mail, etc.
    #[default]
    Mbox,
    /// Directory of standard .eml MIME files inside a zip archive (.zip).
    EmlZip,
    /// Structured JSON array format (.json).
    Json,
    /// JSON Lines format (.jsonl) for stream processing.
    JsonLines,
}

impl std::str::FromStr for ExportFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "mbox" => Ok(ExportFormat::Mbox),
            "eml" | "eml.zip" | "zip" => Ok(ExportFormat::EmlZip),
            "json" => Ok(ExportFormat::Json),
            "jsonl" | "jsonlines" => Ok(ExportFormat::JsonLines),
            _ => Err(format!("Unknown export format '{s}'. Valid formats: mbox, eml, zip, json, jsonl")),
        }
    }
}

/// Summary statistics returned after completing an export operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExportSummary {
    /// Target file path written.
    pub output_path: String,
    /// Export format used.
    pub format: ExportFormat,
    /// Total messages exported.
    pub message_count: usize,
    /// Total bytes written to disk.
    pub bytes_written: u64,
}

/// Core Data Export Engine.
pub struct ExportEngine;

impl ExportEngine {
    /// Export a slice of [`Email`] messages to an MBOX formatted stream.
    pub fn export_mbox<W: Write>(messages: &[Email], mut writer: W) -> Result<u64, ExportError> {
        let mut total_bytes = 0u64;

        for email in messages {
            // MBOX message separator line: "From sender@domain.com Wed Jun 30 21:49:08 1993"
            let date_str = chrono::DateTime::from_timestamp(email.received_at, 0)
                .map(|dt| dt.format("%a %b %e %H:%M:%S %Y").to_string())
                .unwrap_or_else(|| "Thu Jan  1 00:00:00 1970".to_string());

            let from_line = format!("From {} {}\r\n", email.sender, date_str);
            writer.write_all(from_line.as_bytes())?;
            total_bytes += from_line.len() as u64;

            // Headers
            let headers = format!(
                "Message-ID: <{}>\r\nFrom: {}\r\nTo: {}\r\nSubject: {}\r\nDate: {}\r\nX-Nuncio-Account: {}\r\nX-Nuncio-Folder: {}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n",
                email.id, email.sender, email.recipient, email.subject, date_str, email.account_id, email.folder_id
            );
            writer.write_all(headers.as_bytes())?;
            total_bytes += headers.len() as u64;

            // Body
            let body = email.body_plain.as_deref().unwrap_or("");
            writer.write_all(body.as_bytes())?;
            total_bytes += body.len() as u64;

            let footer = "\r\n\r\n";
            writer.write_all(footer.as_bytes())?;
            total_bytes += footer.len() as u64;
        }

        writer.flush()?;
        Ok(total_bytes)
    }

    /// Export a slice of [`Email`] messages to a ZIP archive containing `.eml` files.
    pub fn export_eml_zip<W: Write + std::io::Seek>(messages: &[Email], writer: W) -> Result<u64, ExportError> {
        let mut zip = zip::ZipWriter::new(writer);
        let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        for (idx, email) in messages.iter().enumerate() {
            let filename = format!("messages/{:05}_{}.eml", idx + 1, email.id);
            zip.start_file(filename, options)?;

            let date_str = chrono::DateTime::from_timestamp(email.received_at, 0)
                .map(|dt| dt.to_rfc2822())
                .unwrap_or_default();

            let eml_content = format!(
                "From: {}\r\nTo: {}\r\nSubject: {}\r\nDate: {}\r\nMessage-ID: <{}>\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{}",
                email.sender,
                email.recipient,
                email.subject,
                date_str,
                email.id,
                email.body_plain.as_deref().unwrap_or("")
            );

            zip.write_all(eml_content.as_bytes())?;
        }

        // Add manifest.json
        zip.start_file("manifest.json", options)?;
        let manifest = serde_json::json!({
            "exported_at": chrono::Utc::now().to_rfc3339(),
            "message_count": messages.len(),
            "generator": "Nuncio Sovereign Universal Export Engine v1.0"
        });
        serde_json::to_writer_pretty(&mut zip, &manifest)?;

        let mut finished = zip.finish()?;
        let total_bytes = finished.stream_position()?;
        Ok(total_bytes)
    }

    /// Export a slice of [`Email`] messages to a JSON file.
    pub fn export_json<W: Write>(messages: &[Email], mut writer: W) -> Result<u64, ExportError> {
        let json_bytes = serde_json::to_vec_pretty(messages)?;
        writer.write_all(&json_bytes)?;
        writer.flush()?;
        Ok(json_bytes.len() as u64)
    }

    /// Export a slice of [`Email`] messages to JSON Lines format (.jsonl).
    pub fn export_jsonl<W: Write>(messages: &[Email], mut writer: W) -> Result<u64, ExportError> {
        let mut total_bytes = 0u64;
        for email in messages {
            let line = serde_json::to_string(email)?;
            writer.write_all(line.as_bytes())?;
            writer.write_all(b"\n")?;
            total_bytes += line.len() as u64 + 1;
        }
        writer.flush()?;
        Ok(total_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_messages() -> Vec<Email> {
        vec![
            Email {
                id: "msg-100".to_string(),
                account_id: "acct-1".to_string(),
                folder_id: "INBOX".to_string(),
                subject: "Test Subject 1".to_string(),
                sender: "alice@nuncio.mx".to_string(),
                recipient: "bob@nuncio.mx".to_string(),
                received_at: 1700000000,
                read: false,
                body_plain: Some("Hello world body text".to_string()),
                body_html: None,
                attachments: Vec::new(),
            },
            Email {
                id: "msg-101".to_string(),
                account_id: "acct-1".to_string(),
                folder_id: "INBOX".to_string(),
                subject: "Test Subject 2".to_string(),
                sender: "charlie@nuncio.mx".to_string(),
                recipient: "bob@nuncio.mx".to_string(),
                received_at: 1700000100,
                read: true,
                body_plain: Some("Second body text".to_string()),
                body_html: None,
                attachments: Vec::new(),
            },
        ]
    }

    #[test]
    fn test_export_mbox_format() {
        let msgs = sample_messages();
        let mut buffer = Vec::new();
        let bytes = ExportEngine::export_mbox(&msgs, &mut buffer).unwrap();
        assert!(bytes > 0);

        let content = String::from_utf8(buffer).unwrap();
        assert!(content.contains("From alice@nuncio.mx"));
        assert!(content.contains("Subject: Test Subject 1"));
        assert!(content.contains("From charlie@nuncio.mx"));
    }

    #[test]
    fn test_export_json_and_jsonl_format() {
        let msgs = sample_messages();
        let mut json_buf = Vec::new();
        let json_bytes = ExportEngine::export_json(&msgs, &mut json_buf).unwrap();
        assert!(json_bytes > 0);

        let mut jsonl_buf = Vec::new();
        let jsonl_bytes = ExportEngine::export_jsonl(&msgs, &mut jsonl_buf).unwrap();
        assert!(jsonl_bytes > 0);

        let jsonl_str = String::from_utf8(jsonl_buf).unwrap();
        assert_eq!(jsonl_str.lines().count(), 2);
    }
}
