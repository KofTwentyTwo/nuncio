use axum::{
    body::{to_bytes, Body},
    extract::Request,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(super) type Result<T> = std::result::Result<T, Reply>;

#[derive(Clone, Default)]
pub(super) struct Query(pub BTreeMap<String, Vec<String>>);
impl Query {
    pub fn page_size(&self, default: usize, maximum: usize) -> Result<usize> {
        let size = self
            .get("maxResults")
            .map(str::parse::<usize>)
            .transpose()
            .map_err(|_| Reply::error(400, "invalidArgument"))?
            .unwrap_or(default);
        if size == 0 || size > maximum {
            return Err(Reply::error(400, "invalidArgument"));
        }
        Ok(size)
    }
    pub fn boolean(&self, key: &str, default: bool) -> Result<bool> {
        match self.get(key) {
            None => Ok(default),
            Some("true") => Ok(true),
            Some("false") => Ok(false),
            _ => Err(Reply::error(400, "invalidArgument")),
        }
    }
    pub fn parse(bytes: &[u8]) -> Self {
        let mut pairs = BTreeMap::<String, Vec<String>>::new();
        for (key, value) in url::form_urlencoded::parse(bytes) {
            pairs
                .entry(key.into_owned())
                .or_default()
                .push(value.into_owned());
        }
        Self(pairs)
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key)?.first().map(String::as_str)
    }
    pub fn required(&self, key: &str) -> Result<&str> {
        self.get(key)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| Reply::error(400, "invalidArgument"))
    }
    pub fn validate(&self, allowed: &[&str], repeated: &[&str]) -> Result<()> {
        if self.0.iter().any(|(key, values)| {
            !allowed.contains(&key.as_str())
                || (values.len() != 1 && !repeated.contains(&key.as_str()))
        }) {
            return Err(Reply::error(400, "invalidArgument"));
        }
        Ok(())
    }
}

pub(super) struct Input {
    pub method: String,
    pub path: String,
    pub query: Query,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}
impl Input {
    pub fn segments(&self) -> Result<Vec<String>> {
        self.path
            .trim_start_matches('/')
            .split('/')
            .map(|segment| {
                percent_encoding::percent_decode_str(segment)
                    .decode_utf8()
                    .map(|s| s.into_owned())
                    .map_err(|_| Reply::error(400, "invalidArgument"))
            })
            .collect()
    }
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        if !self
            .headers
            .get("content-type")
            .and_then(|s| s.to_str().ok())
            .is_some_and(|s| s.starts_with("application/json"))
        {
            return Err(Reply::error(400, "invalidArgument"));
        }
        serde_json::from_slice(&self.body).map_err(|_| Reply::error(400, "invalidArgument"))
    }
    pub async fn read(request: Request) -> Result<Self> {
        let (parts, body) = request.into_parts();
        let body = to_bytes(body, 90 * 1024 * 1024)
            .await
            .map_err(|_| Reply::error(413, "payloadTooLarge"))?
            .to_vec();
        if parts.method == "GET" && !body.is_empty() {
            return Err(Reply::error(400, "invalidArgument"));
        }
        Ok(Self {
            method: parts.method.to_string(),
            path: parts.uri.path().into(),
            query: Query::parse(parts.uri.query().unwrap_or("").as_bytes()),
            headers: parts.headers,
            body,
        })
    }
}

#[derive(Debug)]
pub(super) struct Reply {
    pub status: u16,
    pub body: Value,
    pub location: Option<String>,
}
impl std::fmt::Display for Reply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "mock wire error {}", self.status)
    }
}
impl std::error::Error for Reply {}
impl Reply {
    pub fn json(body: Value) -> Self {
        Self {
            status: 200,
            body,
            location: None,
        }
    }
    pub fn error(status: u16, reason: &str) -> Self {
        Self {
            status,
            body: json!({"error":{"code":status,"message":reason,"errors":[{"domain":"global","reason":reason}]}}),
            location: None,
        }
    }
    pub fn response(self) -> Response {
        if self.status == 204 {
            return StatusCode::NO_CONTENT.into_response();
        }
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        if let Some(location) = self.location {
            return (status, [("location", location)], Body::empty()).into_response();
        }
        (status, Json(self.body)).into_response()
    }
}
