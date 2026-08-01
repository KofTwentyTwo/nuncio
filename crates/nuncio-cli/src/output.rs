//! Standardized JSON machine-readable output formatters for Unix scripting.

use std::collections::BTreeMap;

use serde::Serialize;

/// Standardized JSON response envelope for CLI machine output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JsonResponse<T: Serialize> {
    /// Result status string ("ok" or "error").
    pub status: &'static str,
    /// Result payload data (present if status is "ok").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    /// Error message string (present if status is "error").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The gRPC status code name (e.g. `"NotFound"`), when the error came
    /// from a daemon RPC.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// The typed `nuncio.v1.ErrorReason` name (e.g. `"ACCOUNT_NOT_FOUND"`),
    /// decoded from the daemon's `ErrorInfo` status details, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Structured error context (e.g. `{"account_id": "acct-1"}`) carried in
    /// the daemon's `ErrorInfo`, when present and non-empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, String>>,
}

impl<T: Serialize> JsonResponse<T> {
    /// Construct a successful JSON response wrapping payload data.
    pub fn success(data: T) -> Self {
        Self {
            status: "ok",
            data: Some(data),
            error: None,
            code: None,
            reason: None,
            metadata: None,
        }
    }
}

impl JsonResponse<()> {
    /// Construct an error JSON response wrapping an error string, with no
    /// typed gRPC code/reason/metadata (e.g. a purely client-side failure
    /// such as an unreachable daemon).
    pub fn error(message: &str) -> Self {
        Self {
            status: "error",
            data: None,
            error: Some(message.to_string()),
            code: None,
            reason: None,
            metadata: None,
        }
    }

    /// Construct an error JSON response carrying the gRPC code and, when the
    /// daemon attached a WS-A1 `ErrorInfo`, the typed reason and metadata.
    pub fn error_with_info(
        message: &str,
        code: Option<String>,
        reason: Option<String>,
        metadata: BTreeMap<String, String>,
    ) -> Self {
        Self {
            status: "error",
            data: None,
            error: Some(message.to_string()),
            code,
            reason,
            metadata: if metadata.is_empty() {
                None
            } else {
                Some(metadata)
            },
        }
    }
}

/// Serialize payload to a formatted JSON string.
pub fn format_json<T: Serialize>(data: &T) -> String {
    let response = JsonResponse::success(data);
    serde_json::to_string(&response)
        .unwrap_or_else(|_| r#"{"status":"error","error":"JSON serialization failed"}"#.to_string())
}

/// Serialize error message to a formatted JSON string.
pub fn format_json_error(message: &str) -> String {
    let response = JsonResponse::error(message);
    serde_json::to_string(&response)
        .unwrap_or_else(|_| r#"{"status":"error","error":"JSON serialization failed"}"#.to_string())
}

/// Serialize a typed daemon RPC error (gRPC code, optional `ErrorReason`
/// name, and optional metadata) to a formatted JSON string. See
/// [`JsonResponse::error_with_info`].
pub fn format_json_error_with_info(
    message: &str,
    code: Option<String>,
    reason: Option<String>,
    metadata: BTreeMap<String, String>,
) -> String {
    let response = JsonResponse::error_with_info(message, code, reason, metadata);
    serde_json::to_string(&response)
        .unwrap_or_else(|_| r#"{"status":"error","error":"JSON serialization failed"}"#.to_string())
}

/// Print ANSI color ASCII art splash banner for CLI startup.
pub fn print_splash_banner() {
    let banner = get_splash_banner_text();
    println!("{banner}");
}

/// Retrieve exact formatted splash banner string with aligned vertical borders.
pub fn get_splash_banner_text() -> &'static str {
    r#"
 ╔═════════════════════════════════════════════════════════════════════╗
 ║  ███╗   ██╗██╗   ██╗███╗   ██╗██████╗██╗ ██████╗                    ║
 ║  ████╗  ██║██║   ██║████╗  ██║██╔════╝██║██╔═══██╗                  ║
 ║  ██╔██╗ ██║██║   ██║██╔██╗ ██║██║     ██║██║   ██║                  ║
 ║  ██║╚██╗██║██║   ██║██║╚██╗██║██║     ██║██║   ██║                  ║
 ║  ██║ ╚████║╚██████╔╝██║ ╚████║╚██████╗██║╚██████╔╝                  ║
 ║  ╚═╝  ╚═══╝ ╚═════╝ ╚═╝  ╚═══╝ ╚═════╝╚═╝ ╚═════╝                   ║
 ║                                                                     ║
 ║        Nuncio Mail & Calendar Suite — https://nuncio.mx             ║
 ║   Latin: nūntiō ("I announce, I declare, I deliver a message")      ║
 ╠═════════════════════════════════════════════════════════════════════╣
 ║  4 Presentation Shells: POSIX CLI │ Ratatui TUI │ GUI │ MCP AI Stdio║
 ║  Engine: SQLite WAL FTS5 Trigram │ AES-256-GCM │ age Stream Cipher  ║
 ╚═════════════════════════════════════════════════════════════════════╝"#
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn format_json_success_payload() {
        let payload = json!({ "key": "value" });
        let out = format_json(&payload);
        assert!(out.contains(r#""status":"ok""#));
        assert!(out.contains(r#""key":"value""#));
    }

    #[test]
    fn format_json_error_payload() {
        let err_str = format_json_error("account not found");
        assert!(err_str.contains(r#""status":"error""#));
        assert!(err_str.contains(r#""error":"account not found""#));
        // A plain error carries no typed code/reason/metadata.
        assert!(!err_str.contains("\"code\""));
        assert!(!err_str.contains("\"reason\""));
        assert!(!err_str.contains("\"metadata\""));
    }

    #[test]
    fn format_json_error_with_info_carries_code_reason_and_metadata() {
        let mut metadata = BTreeMap::new();
        metadata.insert("account_id".to_string(), "acct-1".to_string());

        let out = format_json_error_with_info(
            "nunciod daemon rejected account_show: account 'acct-1' not found",
            Some("NotFound".to_string()),
            Some("ACCOUNT_NOT_FOUND".to_string()),
            metadata,
        );
        assert!(out.contains(r#""status":"error""#));
        assert!(out.contains(r#""code":"NotFound""#));
        assert!(out.contains(r#""reason":"ACCOUNT_NOT_FOUND""#));
        assert!(out.contains(r#""account_id":"acct-1""#));
    }

    #[test]
    fn format_json_error_with_info_omits_reason_and_metadata_when_absent() {
        // Falls back cleanly when the status carried no typed `ErrorInfo`:
        // still structured (code present), but no fabricated reason/metadata.
        let out = format_json_error_with_info(
            "daemon unreachable",
            Some("Unavailable".to_string()),
            None,
            BTreeMap::new(),
        );
        assert!(out.contains(r#""code":"Unavailable""#));
        assert!(!out.contains("\"reason\""));
        assert!(!out.contains("\"metadata\""));
    }

    #[test]
    fn splash_banner_vertical_borders_perfectly_aligned() {
        let banner = get_splash_banner_text();
        let lines: Vec<&str> = banner.lines().filter(|l| !l.trim().is_empty()).collect();
        assert!(!lines.is_empty());

        let expected_len = lines[0].chars().count();
        for (idx, line) in lines.iter().enumerate() {
            assert_eq!(
                line.chars().count(),
                expected_len,
                "Banner line {} width mismatch! Expected {}, got {}. Line: '{}'",
                idx + 1,
                expected_len,
                line.chars().count(),
                line
            );
        }
    }
}
