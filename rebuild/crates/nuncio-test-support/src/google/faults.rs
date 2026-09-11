use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use super::{
    state::GoogleControl,
    wire::{Input, Reply},
};
use axum::{
    body::{Body, Bytes},
    response::{IntoResponse, Response},
};
use std::collections::BTreeMap;
use tokio::sync::watch;

#[derive(Clone, Serialize)]
pub struct RequestCount {
    pub account: Option<String>,
    pub method: String,
    pub path: String,
    pub count: u64,
}

pub(super) struct Barrier {
    pub entered: watch::Sender<bool>,
    pub released: watch::Sender<bool>,
}
impl Barrier {
    pub fn new() -> Self {
        Self {
            entered: watch::channel(false).0,
            released: watch::channel(false).0,
        }
    }
}
pub(super) type Counts = BTreeMap<(Option<String>, String, String), u64>;

pub(super) fn take(
    faults: &mut Vec<Fault>,
    input: &Input,
    account: &Option<String>,
    ordinal: u64,
    phase: Phase,
) -> Option<FaultAction> {
    let position = faults.iter().position(|fault| {
        fault.phase == phase
            && fault.method == input.method
            && fault.path == input.path
            && fault
                .account
                .as_ref()
                .is_none_or(|a| Some(a) == account.as_ref())
            && fault.call.is_none_or(|n| n == ordinal)
    })?;
    Some(faults.remove(position).action)
}

pub(super) async fn apply(
    control: &GoogleControl,
    action: Option<FaultAction>,
) -> Option<Response> {
    match action? {
        FaultAction::Status {
            code,
            retry_after_secs,
        } => {
            let mut response = Reply::error(code, "injectedFault").response();
            if let Some(seconds) = retry_after_secs {
                if let Ok(value) = seconds.to_string().parse() {
                    response.headers_mut().insert("retry-after", value);
                }
            }
            Some(response)
        }
        FaultAction::MalformedJson => {
            Some(([("content-type", "application/json")], "{invalid").into_response())
        }
        FaultAction::StatusWithRetryDate { code, retry_after } => {
            let mut response = Reply::error(code, "injectedFault").response();
            match retry_after.parse() {
                Ok(value) => {
                    response.headers_mut().insert("retry-after", value);
                    Some(response)
                }
                Err(_) => Some(Reply::error(500, "invalidFaultHeader").response()),
            }
        }
        FaultAction::TruncatedBody => {
            let chunks =
                futures_util::stream::once(async { Ok(Bytes::from_static(b"{\"partial\":")) })
                    .chain(futures_util::stream::once(async {
                        // Separate the body frames so Hyper flushes the prefix before the transport fails.
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                        Err(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "injected truncated response",
                        ))
                    }));
            Some(
                (
                    [("content-type", "application/json")],
                    Body::from_stream(chunks),
                )
                    .into_response(),
            )
        }
        FaultAction::Disconnect => Some(disconnected()),
        FaultAction::Delay { millis } => {
            let mut stop = control.1.subscribe();
            if *stop.borrow() {
                return Some(disconnected());
            }
            tokio::select! {
                () = tokio::time::sleep(std::time::Duration::from_millis(millis)) => None,
                _ = stop.changed() => Some(disconnected()),
            }
        }
        FaultAction::Withhold { barrier } => {
            let mut release = {
                let mut model = control.0.lock().await;
                let barrier = model.barriers.entry(barrier).or_insert_with(Barrier::new);
                barrier.entered.send_replace(true);
                barrier.released.subscribe()
            };
            let mut stop = control.1.subscribe();
            if *stop.borrow() {
                return Some(disconnected());
            }
            if *release.borrow() {
                return None;
            }
            tokio::select! {
                _ = release.changed() => None,
                _ = stop.changed() => Some(disconnected()),
            }
        }
    }
}

fn disconnected() -> Response {
    let stream = futures_util::stream::once(async {
        Err::<Bytes, _>(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "injected disconnect",
        ))
    });
    Body::from_stream(stream).into_response()
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Before,
    After,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FaultAction {
    StatusWithRetryDate {
        code: u16,
        retry_after: String,
    },
    Status {
        code: u16,
        retry_after_secs: Option<u64>,
    },
    MalformedJson,
    TruncatedBody,
    Disconnect,
    Delay {
        millis: u64,
    },
    Withhold {
        barrier: String,
    },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fault {
    pub method: String,
    pub path: String,
    pub account: Option<String>,
    pub call: Option<u64>,
    pub phase: Phase,
    pub action: FaultAction,
}
