use super::http::GoogleHttp;
use futures_util::StreamExt;
use serde_json::Value;

pub(crate) enum WriteResult {
    Acknowledged(Value),
    Rejected {
        code: &'static str,
        retry_at_ms: Option<i64>,
        authorization: bool,
    },
    Uncertain {
        code: &'static str,
        retry_at_ms: Option<i64>,
    },
}
impl WriteResult {
    pub fn retry_deadline(&self) -> Option<i64> {
        match self {
            Self::Acknowledged(_) => None,
            Self::Rejected { retry_at_ms, .. } | Self::Uncertain { retry_at_ms, .. } => {
                *retry_at_ms
            }
        }
    }
}
impl GoogleHttp {
    pub async fn write_gmail(
        &self,
        token: &str,
        path: &[&str],
        body: Vec<u8>,
        retry_at_ms: i64,
    ) -> WriteResult {
        let url = (|| {
            let mut url = url::Url::parse(&self.gmail_url).ok()?;
            for segment in path {
                if segment.is_empty()
                    || segment.len() > 2048
                    || matches!(*segment, "." | "..")
                    || segment.chars().any(char::is_control)
                {
                    return None;
                }
                url.path_segments_mut().ok()?.push(segment);
            }
            Some(url)
        })();
        let Some(url) = url else {
            return WriteResult::Rejected {
                code: "invalid_message_path",
                retry_at_ms: None,
                authorization: false,
            };
        };
        let response = match self
            .client
            .post(url)
            .bearer_auth(token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) if error.is_connect() => {
                return WriteResult::Rejected {
                    code: "connection_failed",
                    retry_at_ms: Some(retry_at_ms),
                    authorization: false,
                }
            }
            Err(_) => {
                return WriteResult::Uncertain {
                    code: "submission_response_lost",
                    retry_at_ms: None,
                }
            }
        };
        let status = response.status().as_u16();
        let retry = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|s| s.to_str().ok())
            .and_then(|s| super::http::retry_after(s, self.clock.now_ms()));
        // A server failure can follow acceptance. Backoff is still respected,
        // but a non-idempotent submission is never blindly repeated.
        if status >= 500 || status == 408 {
            return WriteResult::Uncertain {
                code: "submission_server_error",
                retry_at_ms: retry,
            };
        }
        if status == 401 {
            return WriteResult::Rejected {
                code: "authorization_required",
                retry_at_ms: Some(retry.unwrap_or(retry_at_ms)),
                authorization: true,
            };
        }
        if status == 429 {
            return WriteResult::Rejected {
                code: "rate_limited",
                retry_at_ms: Some(retry.unwrap_or(retry_at_ms)),
                authorization: false,
            };
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) if chunk.len() <= 262144_usize.saturating_sub(bytes.len()) => {
                    bytes.extend_from_slice(&chunk)
                }
                _ => {
                    return WriteResult::Uncertain {
                        code: "invalid_submission_response",
                        retry_at_ms: retry,
                    }
                }
            }
        }
        let value: Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => {
                return WriteResult::Uncertain {
                    code: "invalid_submission_response",
                    retry_at_ms: retry,
                }
            }
        };
        if status == 403 {
            let limited = value["error"]["errors"].as_array().is_some_and(|errors| {
                errors.iter().any(|e| {
                    matches!(
                        e["reason"].as_str(),
                        Some("rateLimitExceeded" | "userRateLimitExceeded" | "dailyLimitExceeded")
                    )
                })
            });
            return WriteResult::Rejected {
                code: if limited {
                    "rate_limited"
                } else {
                    "scope_denied"
                },
                retry_at_ms: Some(retry.unwrap_or(retry_at_ms)),
                authorization: !limited,
            };
        }
        if matches!(status, 400 | 404 | 413 | 422)
            && value["error"]["code"].as_u64() == Some(u64::from(status))
        {
            return WriteResult::Rejected {
                code: if status == 404 {
                    "resource_not_found"
                } else {
                    "submission_rejected"
                },
                retry_at_ms: None,
                authorization: false,
            };
        }
        if status == 200
            && value["id"].as_str().is_some_and(|id| {
                !id.is_empty() && id.len() <= 2048 && !id.chars().any(char::is_control)
            })
        {
            return WriteResult::Acknowledged(value);
        }
        WriteResult::Uncertain {
            code: "invalid_submission_response",
            retry_at_ms: retry,
        }
    }
}
