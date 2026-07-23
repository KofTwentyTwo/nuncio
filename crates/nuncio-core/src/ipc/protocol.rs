//! JSON-RPC 2.0 data models and wire contracts for Nuncio IPC.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 Request Frame.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JsonRpcRequest {
    /// Protocol version string (must be `"2.0"`).
    pub jsonrpc: String,
    /// Numeric request identifier.
    pub id: u64,
    /// RPC method name (e.g. `system.ping`, `mail.sync_all`).
    pub method: String,
    /// Optional parameter payload.
    #[serde(default)]
    pub params: Value,
}

impl JsonRpcRequest {
    /// Construct a new JSON-RPC 2.0 request frame.
    pub fn new(id: u64, method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC 2.0 Response Frame.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JsonRpcResponse {
    /// Protocol version string (`"2.0"`).
    pub jsonrpc: String,
    /// Numeric request identifier matching corresponding request.
    pub id: u64,
    /// Result payload if successful.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error payload if execution failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    /// Construct a successful response frame.
    pub fn success(id: u64, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Construct an error response frame.
    pub fn error(id: u64, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

/// JSON-RPC 2.0 Error Detail.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JsonRpcError {
    /// Numeric error code (-32600 to -32603, or domain-specific integer).
    pub code: i32,
    /// Descriptive error message string.
    pub message: String,
    /// Optional additional error data context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// JSON-RPC 2.0 Unidirectional Notification Frame (Server Push).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JsonRpcNotification {
    /// Protocol version string (`"2.0"`).
    pub jsonrpc: String,
    /// Notification event topic (e.g. `events.notify`).
    pub method: String,
    /// Event payload parameters.
    pub params: Value,
}

impl JsonRpcNotification {
    /// Construct a new JSON-RPC 2.0 notification frame.
    pub fn new(method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_rpc_request_response_serde_roundtrip() {
        let req = JsonRpcRequest::new(42, "system.ping", json!({ "client": "tui" }));
        let serialized = serde_json::to_string(&req).expect("serialize request");
        let deserialized: JsonRpcRequest = serde_json::from_str(&serialized).expect("deserialize request");
        assert_eq!(req, deserialized);

        let resp = JsonRpcResponse::success(42, json!("pong"));
        let resp_serialized = serde_json::to_string(&resp).expect("serialize response");
        let resp_deserialized: JsonRpcResponse = serde_json::from_str(&resp_serialized).expect("deserialize response");
        assert_eq!(resp, resp_deserialized);
    }
}
