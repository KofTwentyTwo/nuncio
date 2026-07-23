//! Standardized JSON machine-readable output formatters for Unix scripting.

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
}

impl<T: Serialize> JsonResponse<T> {
    /// Construct a successful JSON response wrapping payload data.
    pub fn success(data: T) -> Self {
        Self {
            status: "ok",
            data: Some(data),
            error: None,
        }
    }
}

impl JsonResponse<()> {
    /// Construct an error JSON response wrapping an error string.
    pub fn error(message: &str) -> Self {
        Self {
            status: "error",
            data: None,
            error: Some(message.to_string()),
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
