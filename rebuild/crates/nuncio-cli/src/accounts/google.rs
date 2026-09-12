use super::{open_browser, registration, rpc_error, session, v2, AccountClient, AppError};
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};

pub(super) async fn connect(
    client: &mut AccountClient,
    config: PathBuf,
    login_hint: Option<String>,
    account_id: Option<String>,
    no_browser: bool,
    wait_for_consent: bool,
) -> Result<Value, AppError> {
    let mut registration = registration(&config)?;
    let result = client
        .begin_google_auth(v2::BeginGoogleAuthRequest {
            client_id: std::mem::take(&mut registration.client_id),
            client_secret: registration.client_secret.take(),
            login_hint,
            account_id,
        })
        .await
        .map_err(rpc_error)?
        .into_inner();
    let opened = !no_browser && open_browser(&result.browser_url).await;
    if wait_for_consent {
        eprintln!(
            "Complete Google consent in your browser: {}",
            result.browser_url
        );
        return wait(client, result.session_id, 300).await;
    }
    let mut value = session(result);
    value["browser_opened"] = json!(opened);
    Ok(value)
}

pub(super) async fn wait(
    client: &mut AccountClient,
    id: String,
    seconds: u64,
) -> Result<Value, AppError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    loop {
        let status = tokio::time::timeout_at(
            deadline,
            client.get_auth_status(v2::GetAuthStatusRequest {
                session_id: id.clone(),
            }),
        )
        .await
        .map_err(|_| AppError {
            code: "auth_timeout",
            message: "Authorization status timed out; run account auth-status for this session",
            exit: 5,
            recovery: Some(json!({"session_id":id})),
            operation: None,
            sync_run: None,
        })?
        .map_err(rpc_error)?
        .into_inner();
        match status.state.as_str() {
            "succeeded" => return Ok(session(status)),
            "pending" | "completing" => {}
            _ => {
                return Err(auth_error(
                    "auth_failed",
                    "Google authorization did not complete; inspect the session status",
                    status,
                ))
            }
        }
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => return Err(auth_error("auth_timeout", "Google consent is still pending; complete it and run account auth-wait again", status)),
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}
            result = tokio::signal::ctrl_c() => {
                result.map_err(|_| AppError::unavailable())?;
                let status = client.cancel_google_auth(v2::GetAuthStatusRequest { session_id: id }).await.map_err(rpc_error)?.into_inner();
                if status.state == "succeeded" { return Ok(session(status)); }
                return Err(auth_error("auth_cancelled", "Google authorization was cancelled", status));
            }
        }
    }
}

fn auth_error(code: &'static str, message: &'static str, status: v2::AuthSession) -> AppError {
    AppError {
        code,
        message,
        exit: 5,
        recovery: Some(session(status)),
        operation: None,
        sync_run: None,
    }
}
