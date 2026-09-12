use super::{http::GoogleHttp, send::WriteResult};
use crate::store::{CalendarChangePayload, CalendarWriteKind};
use futures_util::StreamExt;
use serde_json::Value;

impl GoogleHttp {
    pub async fn write_calendar(
        &self,
        token: &str,
        intent: &CalendarChangePayload,
        retry_at_ms: i64,
    ) -> WriteResult {
        let preparation = (|| {
            let mut url = url::Url::parse(&self.calendar_url).ok()?;
            let mut segments = vec!["calendars", intent.provider_calendar_id.as_str(), "events"];
            if intent.kind != CalendarWriteKind::Create {
                segments.push(&intent.provider_event_id);
            }
            for segment in segments {
                if segment.is_empty()
                    || segment.len() > 2048
                    || matches!(segment, "." | "..")
                    || segment.chars().any(char::is_control)
                {
                    return None;
                }
                url.path_segments_mut().ok()?.push(segment);
            }
            if !matches!(
                intent.notifications.as_str(),
                "none" | "all" | "externalOnly"
            ) {
                return None;
            }
            url.query_pairs_mut()
                .append_pair("sendUpdates", &intent.notifications);
            let method = match intent.kind {
                CalendarWriteKind::Create => reqwest::Method::POST,
                CalendarWriteKind::Delete => reqwest::Method::DELETE,
                _ => reqwest::Method::PATCH,
            };
            let mut request = self.client.request(method, url).bearer_auth(token);
            if let Some(etag) = &intent.expected_etag {
                let value = reqwest::header::HeaderValue::from_str(etag).ok()?;
                request = request.header(reqwest::header::IF_MATCH, value);
            } else if intent.kind != CalendarWriteKind::Create {
                return None;
            }
            if intent.kind != CalendarWriteKind::Delete {
                let body = serde_json::to_vec(&intent.patch).ok()?;
                if body.len() > 1024 * 1024 {
                    return None;
                }
                request = request
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(body);
            }
            Some(request)
        })();
        let Some(request) = preparation else {
            return WriteResult::Rejected {
                code: "invalid_calendar_payload",
                retry_at_ms: None,
                authorization: false,
            };
        };
        let _request = match self.resources.request().await {
            Ok(permit) => permit,
            Err(_) => {
                return WriteResult::Rejected {
                    code: "provider_busy",
                    retry_at_ms: Some(retry_at_ms),
                    authorization: false,
                }
            }
        };
        let response = match request.send().await {
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
                    code: "calendar_response_lost",
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
        if status >= 500 || status == 408 {
            return WriteResult::Uncertain {
                code: "calendar_server_error",
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
        if status == 412 {
            return WriteResult::Rejected {
                code: "calendar_precondition_failed",
                retry_at_ms: None,
                authorization: false,
            };
        }
        if status == 409 {
            return WriteResult::Uncertain {
                code: "calendar_identity_exists",
                retry_at_ms: None,
            };
        }
        if status == 204 && intent.kind == CalendarWriteKind::Delete {
            return WriteResult::Acknowledged(Value::Null);
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            if let Ok(chunk) = &chunk {
                self.resources.received(chunk.len());
            }
            match chunk {
                Ok(chunk) if chunk.len() <= (1024 * 1024_usize).saturating_sub(bytes.len()) => {
                    bytes.extend_from_slice(&chunk)
                }
                _ => {
                    return WriteResult::Uncertain {
                        code: "invalid_calendar_response",
                        retry_at_ms: retry,
                    }
                }
            }
        }
        let value: Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => {
                return WriteResult::Uncertain {
                    code: "invalid_calendar_response",
                    retry_at_ms: retry,
                }
            }
        };
        if status == 403 {
            let reasons = value["error"]["errors"].as_array();
            let limited = reasons.is_some_and(|a| {
                a.iter().any(|e| {
                    matches!(
                        e["reason"].as_str(),
                        Some(
                            "rateLimitExceeded"
                                | "userRateLimitExceeded"
                                | "dailyLimitExceeded"
                                | "quotaExceeded"
                        )
                    )
                })
            });
            let authorization = reasons.is_some_and(|a| {
                a.iter().any(|e| {
                    matches!(
                        e["reason"].as_str(),
                        Some("insufficientPermissions" | "authError")
                    )
                })
            });
            return WriteResult::Rejected {
                code: if limited {
                    "rate_limited"
                } else if authorization {
                    "scope_denied"
                } else {
                    "calendar_permission_denied"
                },
                retry_at_ms: if limited || authorization {
                    Some(retry.unwrap_or(retry_at_ms))
                } else {
                    None
                },
                authorization,
            };
        }
        if matches!(status, 400 | 404 | 410 | 413 | 422)
            && value["error"]["code"].as_u64() == Some(u64::from(status))
        {
            return WriteResult::Rejected {
                code: if matches!(status, 404 | 410) {
                    "resource_not_found"
                } else {
                    "calendar_rejected"
                },
                retry_at_ms: None,
                authorization: false,
            };
        }
        if matches!(status, 200 | 201)
            && intent.kind != CalendarWriteKind::Delete
            && value["id"] == intent.provider_event_id
        {
            return WriteResult::Acknowledged(value);
        }
        WriteResult::Uncertain {
            code: "invalid_calendar_response",
            retry_at_ms: retry,
        }
    }
}
