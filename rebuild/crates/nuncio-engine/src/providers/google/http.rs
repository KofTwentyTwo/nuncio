use crate::accounts::{AccountError, Credentials, GoogleRegistration};
use futures_util::StreamExt;
use serde::Deserialize;
use std::time::Duration;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

pub(crate) const SCOPES: &str = "openid email https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/calendar";
#[derive(Clone, Copy)]
pub(crate) enum GoogleApi {
    Gmail,
    Calendar,
}
impl GoogleApi {
    pub fn scope(self) -> &'static str {
        match self {
            Self::Gmail => "gmail",
            Self::Calendar => "calendar",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum GoogleRead<'a> {
    Get {
        path: &'a [&'a str],
        query: &'a [(&'a str, &'a str)],
        limit: usize,
    },
    FreeBusy(&'a serde_json::Value),
}

#[derive(Clone)]
pub(crate) struct GoogleHttp {
    pub resources: std::sync::Arc<crate::resources::Resources>,
    pub clock: std::sync::Arc<crate::clock::Clock>,
    pub(super) client: reqwest::Client,
    pub authorization_url: String,
    token_url: String,
    userinfo_url: String,
    pub(super) gmail_url: String,
    pub(super) calendar_url: String,
    pub max_payload_bytes: usize,
    #[cfg(feature = "test-harness")]
    pub test_config: Option<crate::test_controls::TestConfig>,
    pub synthetic: bool,
}

#[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
pub(crate) struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: u64,
    pub scope: Option<String>,
    token_type: String,
}
#[derive(Deserialize)]
pub(crate) struct UserInfo {
    pub sub: String,
    pub email: String,
    pub email_verified: bool,
}

impl GoogleHttp {
    #[cfg(feature = "test-harness")]
    pub async fn refresh_test_clock(&self) -> Result<(), AccountError> {
        if let Some(config) = &self.test_config {
            self.clock.set_test_offset(
                config
                    .clock_offset_ms()
                    .await
                    .map_err(|_| AccountError::Invalid)?,
            );
        }
        Ok(())
    }
    pub fn sync_policy(&self) -> (u64, bool) {
        #[cfg(feature = "test-harness")]
        if let Some(config) = &self.test_config {
            return (config.poll_interval_ms, config.background_sync);
        }
        (60_000, true)
    }
    pub fn production() -> Result<Self, AccountError> {
        Ok(Self {
            resources: crate::resources::Resources::new(),
            clock: std::sync::Arc::new(crate::clock::Clock::new(None)),
            client: Self::client(30_000)?,
            authorization_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token_url: "https://oauth2.googleapis.com/token".into(),
            userinfo_url: "https://openidconnect.googleapis.com/v1/userinfo".into(),
            gmail_url: "https://gmail.googleapis.com/gmail/v1/users/me".into(),
            calendar_url: "https://www.googleapis.com/calendar/v3".into(),
            max_payload_bytes: crate::domain::mail::MAX_PAYLOAD_BYTES,
            #[cfg(feature = "test-harness")]
            test_config: None,
            synthetic: false,
        })
    }
    #[cfg(feature = "test-harness")]
    pub fn for_test(config: &crate::test_controls::TestConfig) -> Result<Self, AccountError> {
        config.validate().map_err(|_| AccountError::Invalid)?;
        let base = config.google_base_url.trim_end_matches('/');
        Ok(Self {
            resources: crate::resources::Resources::new(),
            clock: std::sync::Arc::new(crate::clock::Clock::new(config.now_unix_ms)),
            client: Self::client(config.request_timeout_ms)?,
            authorization_url: format!("{base}/o/oauth2/v2/auth"),
            token_url: format!("{base}/token"),
            userinfo_url: format!("{base}/v1/userinfo"),
            gmail_url: format!("{base}/gmail/v1/users/me"),
            calendar_url: format!("{base}/calendar/v3"),
            max_payload_bytes: usize::try_from(config.max_payload_bytes)
                .map_err(|_| AccountError::Invalid)?,
            test_config: Some(config.clone()),
            synthetic: true,
        })
    }
    fn client(timeout_ms: u64) -> Result<reqwest::Client, AccountError> {
        reqwest::Client::builder()
            .no_proxy()
            .retry(reqwest::retry::never())
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .map_err(|_| AccountError::Unavailable)
    }
    pub async fn read(
        &self,
        api: GoogleApi,
        token: &str,
        input: GoogleRead<'_>,
    ) -> Result<serde_json::Value, crate::mail::MailError> {
        use crate::mail::MailError;
        let (path, query, limit) = match input {
            GoogleRead::Get { path, query, limit } => (path, query, limit),
            GoogleRead::FreeBusy(_) if matches!(api, GoogleApi::Calendar) => {
                (&["freeBusy"][..], &[][..], 4 * 1024 * 1024)
            }
            _ => return Err(MailError::Provider),
        };
        let base = match api {
            GoogleApi::Gmail => &self.gmail_url,
            GoogleApi::Calendar => &self.calendar_url,
        };
        let mut url = url::Url::parse(base).map_err(|_| MailError::Provider)?;
        for segment in path {
            if segment.is_empty() || segment.len() > 2048 || matches!(*segment, "." | "..") {
                return Err(MailError::Provider);
            }
            url.path_segments_mut()
                .map_err(|_| MailError::Provider)?
                .push(segment);
        }
        let request = match input {
            GoogleRead::Get { .. } => self.client.get(url).query(query),
            GoogleRead::FreeBusy(body) => self.client.post(url).json(body),
        };
        let _request = self.resources.request().await?;
        let response = request
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| MailError::Unavailable)?;
        let status = response.status().as_u16();
        let retry = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| retry_after(v, self.clock.now_ms()));
        if status == 401 {
            return Err(AccountError::Authorization.into());
        }
        if status == 404 {
            return Err(MailError::NotFound);
        }
        if status == 410 && matches!(api, GoogleApi::Calendar) {
            return Err(MailError::CalendarExpired);
        }
        if status == 429 || status >= 500 {
            return Err(retry
                .map(MailError::RetryAfter)
                .unwrap_or(MailError::Unavailable));
        }
        let limit = if status == 200 { limit } else { 65536 };
        if response.content_length().is_some_and(|n| n > limit as u64) {
            return Err(MailError::TooLarge);
        }
        // Reuse predictable allocation sizes instead of growing from whichever
        // network chunk happens to arrive first. The advertised size is bounded
        // above before allocation; streamed bodies remain checked per chunk.
        let capacity = response
            .content_length()
            .map(|n| n as usize)
            .unwrap_or(limit.min(64 * 1024));
        let mut stream = response.bytes_stream();
        let mut bytes = Zeroizing::new(Vec::with_capacity(capacity));
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| MailError::Unavailable)?;
            self.resources.received(chunk.len());
            if chunk.len() > limit.saturating_sub(bytes.len()) {
                return Err(MailError::TooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: serde_json::Value =
            tokio::task::spawn_blocking(move || serde_json::from_slice(&bytes))
                .await
                .map_err(|_| MailError::Provider)?
                .map_err(|_| MailError::Provider)?;
        if status == 403 {
            let reasons = value["error"]["errors"].as_array();
            if reasons.is_some_and(|errors| {
                errors.iter().any(|e| {
                    matches!(
                        e["reason"].as_str(),
                        Some("rateLimitExceeded" | "userRateLimitExceeded" | "dailyLimitExceeded")
                    )
                })
            }) {
                return Err(retry
                    .map(MailError::RetryAfter)
                    .unwrap_or(MailError::Unavailable));
            }
            return Err(AccountError::ScopeDenied.into());
        }
        if status != 200 {
            return Err(MailError::Provider);
        }
        Ok(value)
    }
    pub async fn exchange(
        &self,
        registration: &GoogleRegistration,
        code: &str,
        verifier: &str,
        redirect: &str,
    ) -> Result<TokenResponse, AccountError> {
        let mut fields = vec![
            ("client_id", registration.client_id.as_str()),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", redirect),
        ];
        if let Some(secret) = &registration.client_secret {
            fields.push(("client_secret", secret));
        }
        self.token(&fields, true).await
    }
    pub async fn refresh(&self, credentials: &Credentials) -> Result<TokenResponse, AccountError> {
        let mut fields = vec![
            ("client_id", credentials.client_id.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", credentials.refresh_token.as_str()),
        ];
        if let Some(secret) = &credentials.client_secret {
            fields.push(("client_secret", secret));
        }
        self.token(&fields, false).await
    }
    async fn token(
        &self,
        fields: &[(&str, &str)],
        initial: bool,
    ) -> Result<TokenResponse, AccountError> {
        let _request = self
            .resources
            .request()
            .await
            .map_err(|_| AccountError::Unavailable)?;
        let response = self
            .client
            .post(&self.token_url)
            .form(fields)
            .send()
            .await
            .map_err(|_| AccountError::Unavailable)?;
        let status = response.status();
        if status.as_u16() == 429 || status.is_server_error() {
            return Err(self.account_unavailable(&response));
        }
        let bytes = body(response, 65536, &self.resources).await?;
        if !status.is_success() {
            #[derive(Deserialize)]
            struct TokenError {
                error: String,
            }
            return Err(
                match serde_json::from_slice::<TokenError>(&bytes)
                    .ok()
                    .map(|v| v.error)
                    .as_deref()
                {
                    Some("invalid_grant" | "invalid_client" | "unauthorized_client") => {
                        AccountError::Authorization
                    }
                    _ if status.as_u16() == 429 || status.is_server_error() => {
                        AccountError::Unavailable
                    }
                    _ => AccountError::Provider,
                },
            );
        }
        let token: TokenResponse =
            serde_json::from_slice(&bytes).map_err(|_| AccountError::Provider)?;
        if !valid_token(&token.access_token)
            || token.token_type != "Bearer"
            || token.expires_in == 0
            || token.expires_in > 86400
            || token
                .refresh_token
                .as_ref()
                .is_some_and(|t| !valid_token(t))
            || initial && token.refresh_token.is_none()
        {
            return Err(AccountError::Provider);
        }
        if let Some(scopes) = &token.scope {
            // Full engine capability requires every requested permission.
            if SCOPES.split_whitespace().any(|s| {
                !scopes.split_whitespace().any(|granted| {
                    granted == s
                        || s == "email"
                            && granted == "https://www.googleapis.com/auth/userinfo.email"
                })
            }) {
                return Err(AccountError::ScopeDenied);
            }
        } else if initial {
            return Err(AccountError::ScopeDenied);
        }
        Ok(token)
    }
    pub async fn userinfo(&self, token: &str) -> Result<UserInfo, AccountError> {
        let _request = self
            .resources
            .request()
            .await
            .map_err(|_| AccountError::Unavailable)?;
        let response = self
            .client
            .get(&self.userinfo_url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| AccountError::Unavailable)?;
        match response.status().as_u16() {
            401 => return Err(AccountError::Authorization),
            403 => return Err(AccountError::ScopeDenied),
            429 | 500..=599 => return Err(self.account_unavailable(&response)),
            200 => {}
            _ => return Err(AccountError::Provider),
        }
        let info: UserInfo = serde_json::from_slice(&body(response, 65536, &self.resources).await?)
            .map_err(|_| AccountError::Provider)?;
        if info.sub.is_empty()
            || info.sub.len() > 255
            || !info.sub.bytes().all(|b| b.is_ascii_graphic())
            || !info.email_verified
            || info.email.is_empty()
            || info.email.len() > 320
            || info.email.chars().any(char::is_control)
            || !info.email.contains('@')
        {
            return Err(AccountError::Provider);
        }
        Ok(info)
    }
    fn account_unavailable(&self, response: &reqwest::Response) -> AccountError {
        response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| retry_after(v, self.clock.now_ms()))
            .map(AccountError::RetryAfter)
            .unwrap_or(AccountError::Unavailable)
    }
}
pub(super) fn retry_after(value: &str, now_ms: i64) -> Option<i64> {
    let value = value.trim();
    if value.is_empty() || value.len() > 80 {
        return None;
    }
    if value.bytes().all(|b| b.is_ascii_digit()) {
        // Saturate valid but unrepresentable delays rather than retrying early.
        let seconds = value.parse::<i64>().unwrap_or(i64::MAX);
        return Some(now_ms.saturating_add(seconds.saturating_mul(1000)));
    }
    let date = httpdate::parse_http_date(value).ok()?;
    let duration = date
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    Some(
        i64::try_from(duration.as_millis())
            .unwrap_or(i64::MAX)
            .max(now_ms),
    )
}

fn valid_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= 16384 && token.bytes().all(|b| b.is_ascii_graphic())
}
async fn body(
    response: reqwest::Response,
    limit: usize,
    resources: &crate::resources::Resources,
) -> Result<Zeroizing<Vec<u8>>, AccountError> {
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err(AccountError::Provider);
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Zeroizing::new(Vec::new());
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| AccountError::Unavailable)?;
        resources.received(chunk.len());
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(AccountError::Provider);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::retry_after;
    #[test]
    fn retry_after_accepts_http_dates_and_decimal_seconds_without_overflow() {
        let now = 1772895600000;
        assert_eq!(retry_after(" 6 ", now), Some(now + 6000));
        assert_eq!(
            retry_after("Sat, 07 Mar 2026 15:00:06 GMT", now),
            Some(now + 6000)
        );
        assert_eq!(
            retry_after("Saturday, 07-Mar-26 15:00:06 GMT", now),
            Some(now + 6000)
        );
        assert_eq!(retry_after("Sun, 06 Nov 1994 08:49:37 GMT", now), Some(now));
        assert_eq!(
            retry_after("999999999999999999999999999", now),
            Some(i64::MAX)
        );
        for invalid in ["", "-1", "+1", "1.2", "1, 2", "tomorrow"] {
            assert_eq!(retry_after(invalid, now), None);
        }
    }
}
