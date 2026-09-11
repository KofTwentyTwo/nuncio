use crate::accounts::{AccountError, Accounts, AuthSession, GoogleAuthRequest, SessionSlot};
use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tokio::{net::TcpListener, sync::watch};
use zeroize::Zeroizing;

struct Callback {
    accounts: Arc<Accounts>,
    request: GoogleAuthRequest,
    session: String,
    state: Zeroizing<String>,
    verifier: Zeroizing<String>,
    redirect: String,
    host: String,
    deadline: tokio::time::Instant,
    done: watch::Sender<bool>,
}
fn random() -> Zeroizing<String> {
    let mut bytes = Zeroizing::new([0_u8; 32]);
    rand::rngs::OsRng.fill_bytes(bytes.as_mut());
    Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes.as_slice()))
}
pub(crate) async fn begin(
    accounts: Arc<Accounts>,
    request: GoogleAuthRequest,
) -> Result<AuthSession, AccountError> {
    let mut sessions = accounts.sessions.lock().await;
    if *accounts.stop.borrow() {
        return Err(AccountError::Unavailable);
    }
    if sessions
        .values()
        .filter(|s| matches!(s.status.state.as_str(), "pending" | "completing"))
        .count()
        >= 16
    {
        return Err(AccountError::Limit);
    }
    if sessions.len() >= 64 {
        if let Some(id) = sessions
            .iter()
            .find(|(_, s)| !matches!(s.status.state.as_str(), "pending" | "completing"))
            .map(|(id, _)| id.clone())
        {
            sessions.remove(&id);
        }
    }
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| AccountError::Unavailable)?;
    let host = listener
        .local_addr()
        .map_err(|_| AccountError::Unavailable)?
        .to_string();
    let redirect = format!("http://{host}/oauth/callback");
    let state = random();
    let verifier = random();
    let mut url =
        url::Url::parse(&accounts.http.authorization_url).map_err(|_| AccountError::Invalid)?;
    url.query_pairs_mut().extend_pairs([
        ("client_id", request.registration.client_id.as_str()),
        ("redirect_uri", redirect.as_str()),
        ("response_type", "code"),
        ("state", state.as_str()),
        ("code_challenge_method", "S256"),
        (
            "code_challenge",
            URL_SAFE_NO_PAD
                .encode(Sha256::digest(verifier.as_bytes()))
                .as_str(),
        ),
        ("scope", super::http::SCOPES),
        ("access_type", "offline"),
        ("prompt", "consent"),
    ]);
    if let Some(hint) = &request.login_hint {
        url.query_pairs_mut().append_pair("login_hint", hint);
    }
    let session = AuthSession {
        session_id: uuid::Uuid::new_v4().to_string(),
        browser_url: url.into(),
        expires_at_ms: i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| AccountError::Unavailable)?
                .as_millis(),
        )
        .map_err(|_| AccountError::Unavailable)?
            + 300_000,
        state: "pending".into(),
        account_id: None,
        error_code: None,
        warning_code: None,
    };
    let (done, completed) = watch::channel(false);
    sessions.insert(
        session.session_id.clone(),
        SessionSlot {
            status: session.clone(),
            expected_account: request.account.clone(),
            cancel: done.clone(),
        },
    );
    drop(sessions);
    let stopped = accounts.stop.subscribe();
    let callback = Arc::new(Callback {
        accounts: accounts.clone(),
        request,
        session: session.session_id.clone(),
        state,
        verifier,
        redirect,
        host,
        deadline: tokio::time::Instant::now() + Duration::from_secs(300),
        done,
    });
    let task = tokio::spawn(serve_callback(listener, callback, completed, stopped));
    let mut tasks = accounts.tasks.lock().await;
    tasks.retain(|task| !task.is_finished());
    tasks.push(task);
    Ok(session)
}

async fn serve_callback(
    listener: TcpListener,
    callback: Arc<Callback>,
    mut completed: watch::Receiver<bool>,
    mut stopped: watch::Receiver<bool>,
) {
    let mut connections = tokio::task::JoinSet::new();
    loop {
        if *completed.borrow() || *stopped.borrow() {
            break;
        }
        tokio::select! {
            _=completed.changed()=>break,
            _=stopped.changed()=>break,
            _=tokio::time::sleep_until(callback.deadline)=>break,
            _=connections.join_next(), if !connections.is_empty()=>{},
            accepted=listener.accept(), if connections.len()<4=>{
                let Ok((stream,_))=accepted else {break};
                let callback=callback.clone();
                connections.spawn(async move {
                    let service=hyper::service::service_fn(move |request:hyper::Request<hyper::body::Incoming>| {
                        let callback=callback.clone();
                        async move {
                            Ok::<_,std::convert::Infallible>(handle(State(callback),request.map(axum::body::Body::new)).await)
                        }
                    });
                    let mut builder=hyper::server::conn::http1::Builder::new();
                    builder.timer(hyper_util::rt::TokioTimer::new()).header_read_timeout(Duration::from_secs(3))
                        .max_buf_size(16384).keep_alive(false);
                    let connection=builder.serve_connection(hyper_util::rt::TokioIo::new(stream),service);
                    let _=tokio::time::timeout(Duration::from_secs(65),connection).await;
                });
            }
        }
    }
    drop(listener);
    if let Some(slot) = callback
        .accounts
        .sessions
        .lock()
        .await
        .get_mut(&callback.session)
    {
        if matches!(slot.status.state.as_str(), "pending" | "completing") {
            slot.status.state = if *stopped.borrow() {
                "cancelled"
            } else {
                "expired"
            }
            .into();
            slot.status.browser_url.clear();
        }
    }
    // Finished callbacks get time to flush their response. Incomplete peers cannot
    // retain the daemon. Interrupted credential writes have durable cleanup intents.
    let drain = async { while connections.join_next().await.is_some() {} };
    let _ = tokio::time::timeout(Duration::from_secs(2), drain).await;
    connections.shutdown().await;
}

async fn handle(State(callback): State<Arc<Callback>>, request: Request) -> Response {
    let query = parse(&callback, &request);
    drop(request);
    let result = match query {
        Ok(query) => accept(&callback, query).await,
        Err(error) => Err(error),
    };
    let (status, text) = match result {
        Ok(()) => (
            StatusCode::OK,
            "Authorization completed. Close this window and return to Nuncio.",
        ),
        Err(_) => (
            StatusCode::BAD_REQUEST,
            "Authorization was not completed. Return to Nuncio to inspect its status.",
        ),
    };
    (
        status,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
            (header::CONTENT_SECURITY_POLICY, "default-src 'none'"),
        ],
        text,
    )
        .into_response()
}
fn parse(
    callback: &Callback,
    request: &Request,
) -> Result<BTreeMap<String, Zeroizing<String>>, AccountError> {
    if request.method() != axum::http::Method::GET
        || request.uri().path() != "/oauth/callback"
        || request.uri().to_string().len() > 16384
        || tokio::time::Instant::now() >= callback.deadline
        || request
            .headers()
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            != Some(callback.host.as_str())
    {
        return Err(AccountError::Invalid);
    }
    let mut query = BTreeMap::new();
    for (name, value) in url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
    {
        if ![
            "state",
            "code",
            "error",
            "error_description",
            "scope",
            "authuser",
            "prompt",
            "hd",
            "iss",
        ]
        .contains(&name.as_ref())
            || query
                .insert(name.into_owned(), Zeroizing::new(value.into_owned()))
                .is_some()
        {
            return Err(AccountError::Invalid);
        }
    }
    let state = query.get("state").ok_or(AccountError::Invalid)?;
    if !bool::from(state.as_bytes().ct_eq(callback.state.as_bytes()))
        || query.contains_key("code") == query.contains_key("error")
        || query
            .get("code")
            .is_some_and(|s| s.is_empty() || s.len() > 8192 || s.chars().any(char::is_control))
    {
        return Err(AccountError::Invalid);
    }
    if query
        .get("iss")
        .is_some_and(|iss| iss.as_str() != "https://accounts.google.com")
    {
        return Err(AccountError::Invalid);
    }
    Ok(query)
}
async fn accept(
    callback: &Callback,
    query: BTreeMap<String, Zeroizing<String>>,
) -> Result<(), AccountError> {
    {
        let mut sessions = callback.accounts.sessions.lock().await;
        let slot = sessions
            .get_mut(&callback.session)
            .ok_or(AccountError::NotFound)?;
        if slot.status.state != "pending" {
            return Err(AccountError::Authorization);
        }
        slot.status.state = "completing".into();
        slot.status.browser_url.clear();
    }
    let result = if query.contains_key("error") {
        Err(AccountError::Authorization)
    } else {
        complete(callback, query.get("code").ok_or(AccountError::Invalid)?).await
    };
    {
        let mut sessions = callback.accounts.sessions.lock().await;
        if let Some(slot) = sessions.get_mut(&callback.session) {
            if slot.status.state != "cancelled" {
                match &result {
                    Ok((account, cleanup_pending)) => {
                        slot.status.state = "succeeded".into();
                        slot.status.account_id = Some(account.clone());
                        slot.status.warning_code =
                            cleanup_pending.then(|| "credential_cleanup_pending".into());
                    }
                    Err(error) => {
                        slot.status.state = if query.contains_key("error") {
                            "denied"
                        } else {
                            "failed"
                        }
                        .into();
                        slot.status.error_code = Some(error.code().into());
                    }
                }
            }
        }
    }
    callback.done.send_replace(true);
    result.map(|_| ())
}
async fn complete(callback: &Callback, code: &str) -> Result<(String, bool), AccountError> {
    let token = callback
        .accounts
        .http
        .exchange(
            &callback.request.registration,
            code,
            &callback.verifier,
            &callback.redirect,
        )
        .await?;
    let info = callback.accounts.http.userinfo(&token.access_token).await?;
    callback
        .accounts
        .finish(&callback.session, &callback.request, info, token)
        .await
}
