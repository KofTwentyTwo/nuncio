//! Per-RPC tracing span and request-id correlation for the gRPC transport.
//!
//! This is a [`tower::Layer`] mounted on the tonic `Server` so that every
//! inbound RPC is wrapped in a single root span carrying:
//!
//! * a generated `request_id` -- a lightweight per-process identifier
//!   (process-start tag + monotonic counter) that lets an operator follow one
//!   request across every log line it produced, without pulling in a heavier
//!   UUID dependency; and
//! * the gRPC method path and the peer socket address.
//!
//! The inner service (routing + the per-service `BearerAuthInterceptor` + the
//! handler future) is entered inside that span, so *every* event emitted while
//! the request is handled -- including the auth accept/reject logs and any
//! event a handler emits -- inherits the `request_id` automatically. The span
//! opens with a DEBUG "received" event and closes, once the response body has
//! been fully drained, with an INFO event carrying the terminal gRPC status
//! code and the elapsed wall-clock time.
//!
//! ## Why the response body is wrapped
//!
//! A gRPC call's final status lives in the HTTP/2 *trailers* (`grpc-status`),
//! which are only produced once the response body finishes streaming (an
//! immediate error is instead a trailers-only response with `grpc-status` in
//! the leading headers). The layer therefore wraps the response body in
//! [`TracedBody`], which observes the terminal status as it flows past and
//! emits the completion event when the stream ends (or when the body is
//! dropped, e.g. on client disconnect), so a long-lived server-streaming RPC
//! is not logged as "complete" until it actually ends.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::task::{Context, Poll};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use http::{HeaderMap, Request, Response};
use http_body::{Body, Frame, SizeHint};
use tonic::body::Body as TonicBody;
use tonic::transport::server::TcpConnectInfo;
use tonic::Code;
use tower::{Layer, Service};
use tracing::{Instrument, Span};

/// HTTP/2 trailer (or leading header, for a trailers-only error response)
/// carrying the final gRPC status code.
const GRPC_STATUS_HEADER: &str = "grpc-status";

/// Generates a correlation id unique within this daemon process.
///
/// The id is `<process-tag>-<counter>`: the process tag is the daemon's start
/// time in unix nanoseconds (captured once), and the counter increments per
/// call. This is deliberately not a UUID -- it needs only to be unique within
/// a single running daemon so its logs can be correlated, which a per-process
/// tag plus a monotonic counter satisfies without a new dependency.
fn next_request_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    static PROCESS_TAG: OnceLock<u64> = OnceLock::new();

    let tag = *PROCESS_TAG.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    });
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{tag:016x}-{n:08x}")
}

/// Reads the `grpc-status` code from a header/trailer map, if present.
fn grpc_status_code(headers: &HeaderMap) -> Option<Code> {
    headers
        .get(GRPC_STATUS_HEADER)
        .map(|value| Code::from_bytes(value.as_bytes()))
}

/// [`tower::Layer`] that wraps each RPC in a correlation span. Mounted on the
/// tonic `Server` via `Server::builder().layer(RpcTraceLayer)`.
#[derive(Clone, Copy, Debug, Default)]
pub struct RpcTraceLayer;

impl<S> Layer<S> for RpcTraceLayer {
    type Service = RpcTrace<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RpcTrace { inner }
    }
}

/// The [`RpcTraceLayer`]'s service: opens the span, runs the inner service
/// (routing, auth interceptor, handler) inside it, and wraps the response body
/// so the terminal status can be observed at stream end.
#[derive(Clone, Debug)]
pub struct RpcTrace<S> {
    inner: S,
}

impl<S> Service<Request<TonicBody>> for RpcTrace<S>
where
    S: Service<Request<TonicBody>, Response = Response<TonicBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = Response<TracedBody<TonicBody>>;
    type Error = S::Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<TonicBody>) -> Self::Future {
        // `poll_ready` was called on `self.inner`, so it -- not the fresh
        // clone -- is the readied service that must handle this request. Swap
        // the readied service out and leave the clone behind for the next
        // call (the standard tower clone-and-replace pattern that avoids
        // calling an un-readied clone).
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);

        let method = req.uri().path().to_string();
        let peer = req
            .extensions()
            .get::<TcpConnectInfo>()
            .and_then(TcpConnectInfo::remote_addr)
            .map(|addr| addr.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let request_id = next_request_id();

        let span = tracing::info_span!(
            "grpc.request",
            request_id = %request_id,
            rpc.method = %method,
            peer = %peer,
        );
        let start = Instant::now();

        Box::pin(async move {
            // Enter the span for the synchronous routing + interceptor call so
            // the auth accept/reject logs inherit the request_id, and emit the
            // open event under it.
            let call = span.in_scope(|| {
                tracing::debug!("gRPC request received");
                inner.call(req)
            });
            // Instrument the handler future so events emitted across its await
            // points also inherit the span.
            let response = call.instrument(span.clone()).await?;

            // A trailers-only error response carries `grpc-status` in the
            // leading headers; a normal response carries it in the trailers,
            // observed by `TracedBody` as the body drains.
            let initial_status = grpc_status_code(response.headers());
            let (parts, body) = response.into_parts();
            let traced = TracedBody::new(body, span, start, initial_status);
            Ok(Response::from_parts(parts, traced))
        })
    }
}

/// Response body wrapper that observes the terminal `grpc-status` and logs the
/// request's completion (status + elapsed) exactly once, when the body stream
/// ends or is dropped.
///
/// All fields are `Unpin`, so the pin projection in [`Body::poll_frame`] is a
/// safe `Pin::new` over the inner body rather than an `unsafe` projection.
#[derive(Debug)]
pub struct TracedBody<B> {
    inner: B,
    span: Span,
    start: Instant,
    status: Option<Code>,
    logged: bool,
}

impl<B> TracedBody<B> {
    fn new(inner: B, span: Span, start: Instant, initial_status: Option<Code>) -> Self {
        Self {
            inner,
            span,
            start,
            status: initial_status,
            logged: false,
        }
    }

    /// Emits the terminal completion event once. Idempotent: subsequent calls
    /// (e.g. a `Drop` after the stream already ended) are no-ops.
    fn log_completion(&mut self) {
        if self.logged {
            return;
        }
        self.logged = true;
        let elapsed_ms = self.start.elapsed().as_millis() as u64;
        let status = self.status.unwrap_or(Code::Unknown);
        self.span.in_scope(|| {
            tracing::info!(
                grpc.status = ?status,
                elapsed_ms,
                "gRPC request completed"
            );
        });
    }
}

impl<B> Body for TracedBody<B>
where
    B: Body + Unpin,
{
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        // Safe because `Self: Unpin` (all fields are `Unpin`).
        let this = self.get_mut();
        let polled = Pin::new(&mut this.inner).poll_frame(cx);
        match &polled {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(trailers) = frame.trailers_ref() {
                    if let Some(code) = grpc_status_code(trailers) {
                        this.status = Some(code);
                    }
                }
            }
            Poll::Ready(Some(Err(_))) | Poll::Ready(None) => this.log_completion(),
            Poll::Pending => {}
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl<B> Drop for TracedBody<B> {
    fn drop(&mut self) {
        // Covers stream ends that do not surface a final frame here (client
        // disconnect, an outer layer dropping the body): the request is still
        // logged as completed exactly once.
        self.log_completion();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_tracing::with_recorder;
    use http_body_util::BodyExt;
    use std::collections::HashSet;
    use std::convert::Infallible;
    use tower::ServiceExt;
    use tracing::Level;

    /// Minimal inner service that returns a response with `grpc-status` set in
    /// the leading headers (a trailers-only response, tonic's shape for an
    /// immediate status), so the layer's terminal-status extraction can be
    /// exercised without a live server.
    #[derive(Clone)]
    struct StatusService {
        code: i32,
    }

    impl Service<Request<TonicBody>> for StatusService {
        type Response = Response<TonicBody>;
        type Error = Infallible;
        type Future =
            Pin<Box<dyn Future<Output = Result<Self::Response, Infallible>> + Send + 'static>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _req: Request<TonicBody>) -> Self::Future {
            let code = self.code;
            Box::pin(async move {
                // A handler-emitted event: it must inherit the span so it
                // carries the request_id set by the layer.
                tracing::info!("handler ran");
                let mut response = Response::new(TonicBody::empty());
                response
                    .headers_mut()
                    .insert(GRPC_STATUS_HEADER, code.to_string().parse().unwrap());
                Ok(response)
            })
        }
    }

    fn request() -> Request<TonicBody> {
        Request::builder()
            .uri("/nuncio.v1.Mail/Sync")
            .body(TonicBody::empty())
            .unwrap()
    }

    #[test]
    fn rpc_produces_span_with_request_id_and_logs_terminal_status() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        let (recorder, ()) = with_recorder(|| {
            runtime.block_on(async {
                let mut svc = RpcTraceLayer.layer(StatusService { code: 5 });
                let response = svc.ready().await.unwrap().call(request()).await.unwrap();
                // Drain the body so the completion event fires at stream end.
                response.into_body().collect().await.unwrap();
            });
        });

        let spans = recorder.spans();
        let rpc_span = spans
            .iter()
            .find(|s| s.name == "grpc.request")
            .expect("a grpc.request span must be created");
        let request_id = rpc_span
            .fields
            .get("request_id")
            .expect("the span must carry a request_id field");
        assert!(!request_id.is_empty(), "request_id must be populated");
        assert_eq!(
            rpc_span.fields.get("rpc.method").map(String::as_str),
            Some("/nuncio.v1.Mail/Sync"),
            "the span must carry the gRPC method path"
        );

        let events = recorder.events();
        let completion = events
            .iter()
            .find(|e| e.message() == "gRPC request completed")
            .expect("the terminal completion event must be logged");
        assert_eq!(
            completion.level,
            Level::INFO,
            "the completion event is logged at INFO"
        );
        assert_eq!(
            completion.fields.get("grpc.status").map(String::as_str),
            Some("NotFound"),
            "the completion event must carry the terminal gRPC status code"
        );
        assert!(
            completion.fields.contains_key("elapsed_ms"),
            "the completion event must carry the elapsed time"
        );

        // The handler ran inside the span, so its own event was captured too
        // (proving handler events inherit the correlation context).
        assert!(
            events.iter().any(|e| e.message() == "handler ran"),
            "a handler-emitted event must be observed under the request span"
        );
    }

    #[test]
    fn ok_status_is_recorded_at_completion() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let (recorder, ()) = with_recorder(|| {
            runtime.block_on(async {
                let mut svc = RpcTraceLayer.layer(StatusService { code: 0 });
                let response = svc.ready().await.unwrap().call(request()).await.unwrap();
                response.into_body().collect().await.unwrap();
            });
        });

        let events = recorder.events();
        let completion = events
            .iter()
            .find(|e| e.message() == "gRPC request completed")
            .expect("completion event");
        assert_eq!(
            completion.fields.get("grpc.status").map(String::as_str),
            Some("Ok")
        );
    }

    #[test]
    fn grpc_status_code_parses_known_codes_and_ignores_absent() {
        let mut headers = HeaderMap::new();
        assert!(grpc_status_code(&headers).is_none());
        headers.insert(GRPC_STATUS_HEADER, "3".parse().unwrap());
        assert_eq!(grpc_status_code(&headers), Some(Code::InvalidArgument));
    }

    #[test]
    fn request_ids_are_unique_within_the_process() {
        let ids: HashSet<String> = (0..1000).map(|_| next_request_id()).collect();
        assert_eq!(ids.len(), 1000, "request ids must not collide");
    }
}
