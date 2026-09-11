mod calendar;
mod calendar_recurrence;
mod calendar_state;
mod gmail;
pub use calendar_state::{CalendarSnapshot, NotificationEffect};
mod faults;
pub use faults::{Fault, FaultAction, Phase, RequestCount};
mod mail_state;
mod oauth;
mod paging;
mod state;
mod wire;

use crate::TestError;
use axum::{
    extract::{Request, State},
    response::Response,
    Router,
};
pub use mail_state::{AcceptedSend, MailboxSnapshot, RemoteMessage};
pub use state::{CursorScope, GoogleControl, Seed, Snapshot};
use std::sync::Arc;
use tokio::{
    sync::{oneshot, Mutex},
    task::JoinHandle,
};

pub struct MockGoogle {
    base: String,
    control: GoogleControl,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
}

impl MockGoogle {
    pub async fn start(seed: Seed) -> Result<Self, TestError> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        let control = GoogleControl(
            Arc::new(Mutex::new(state::Model::new(seed)?)),
            tokio::sync::watch::channel(false).0,
        );
        let app = Router::new().fallback(handle).with_state(control.clone());
        let (shutdown, receiver) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = receiver.await;
                })
                .await
        });
        Ok(Self {
            base,
            control,
            shutdown: Some(shutdown),
            task: Some(task),
        })
    }
    pub fn base_url(&self) -> &str {
        &self.base
    }
    pub fn control(&self) -> GoogleControl {
        self.control.clone()
    }
    pub async fn shutdown(mut self) -> Result<(), TestError> {
        self.stop().await
    }
    /// Close the actual listener while retaining independent observations.
    pub async fn stop(&mut self) -> Result<(), TestError> {
        self.control.1.send_replace(true);
        if let Some(sender) = self.shutdown.take() {
            let _ = sender.send(());
        }
        if let Some(mut task) = self.task.take() {
            match tokio::time::timeout(std::time::Duration::from_secs(2), &mut task).await {
                Ok(result) => {
                    result??;
                }
                Err(error) => {
                    task.abort();
                    let _ = task.await;
                    return Err(error.into());
                }
            }
        }
        Ok(())
    }
}

impl Drop for MockGoogle {
    fn drop(&mut self) {
        self.control.1.send_replace(true);
        if let Some(sender) = self.shutdown.take() {
            let _ = sender.send(());
        }
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

async fn handle(State(control): State<GoogleControl>, request: Request) -> Response {
    let input = match wire::Input::read(request).await {
        Ok(input) => input,
        Err(error) => return error.response(),
    };
    let mut model = control.0.lock().await;
    if !input.path.starts_with("/gmail/")
        && !input.path.starts_with("/calendar/")
        && input.path != "/o/oauth2/v2/auth"
        && input.path != "/token"
        && input.path != "/v1/userinfo"
    {
        return wire::Reply::error(404, "notFound").response();
    }
    let account = model.observed_account(&input);
    let count = model
        .counts
        .entry((account.clone(), input.method.clone(), input.path.clone()))
        .or_default();
    *count += 1;
    let ordinal = *count;
    let before = faults::take(&mut model.faults, &input, &account, ordinal, Phase::Before);
    drop(model);
    if let Some(response) = faults::apply(&control, before).await {
        return response;
    }
    let mut model = control.0.lock().await;
    let reply = if input.path == "/o/oauth2/v2/auth" || input.path == "/token" {
        oauth::route(&mut model, &input)
    } else {
        model.authorize(&input).and_then(|account| {
            if input.path == "/v1/userinfo" {
                oauth::userinfo(&input, &account)
            } else if input.path.starts_with("/gmail/") {
                gmail::route(&mut model, &input, &account)
            } else if input.path.starts_with("/calendar/") {
                calendar::route(&mut model, &input, &account)
            } else {
                Err(wire::Reply::error(404, "notFound"))
            }
        })
    };
    let reply = reply.unwrap_or_else(|error| error);
    let after = if reply.status < 300 {
        faults::take(&mut model.faults, &input, &account, ordinal, Phase::After)
    } else {
        None
    };
    drop(model);
    faults::apply(&control, after)
        .await
        .unwrap_or_else(|| reply.response())
}
