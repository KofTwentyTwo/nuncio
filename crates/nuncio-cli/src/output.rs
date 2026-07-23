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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn format_json_success_payload() {
        let payload = json!({"count": 42, "user": "alice"});
        let json_str = format_json(&payload);
        assert!(json_str.contains(r#""status":"ok""#));
        assert!(json_str.contains(r#""count":42"#));
    }

    #[test]
    fn format_json_error_payload() {
        let err_str = format_json_error("account not found");
        assert!(err_str.contains(r#""status":"error""#));
        assert!(err_str.contains(r#""error":"account not found""#));
    }
}
