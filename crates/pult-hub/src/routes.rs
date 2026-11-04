//! The routes with data. The guard has run before any of them: each has a session.

use std::collections::HashMap;
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use appkit_hub::{Core, bearer, error_body, json_response, no_query};
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::http::header::{self, HeaderMap, HeaderValue};
use axum::response::Response;
use axum::routing::{get, post};
use futures_core::Stream;
use pult_core::protocol::Op;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::link::{Asked, PUBLISH_EVERY};
use crate::{IDENTITY, Status};

/// How long an operation waits for the owner of the mixer: it looks in a few times a second.
const ANSWER_WITHIN: Duration = Duration::from_secs(2);

/// The event streams each session holds open.
#[derive(Default)]
pub(crate) struct Streams(Arc<Mutex<HashMap<String, usize>>>);

/// One open stream; dropping it gives the place back.
struct StreamPlace {
    count: Arc<Mutex<HashMap<String, usize>>>,
    token: String,
}

impl Streams {
    fn acquire(&self, token: &str, max: usize) -> Option<StreamPlace> {
        let mut count = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let open = count.entry(token.to_owned()).or_default();
        if *open >= max {
            return None;
        }
        *open += 1;
        Some(StreamPlace {
            count: Arc::clone(&self.0),
            token: token.to_owned(),
        })
    }
}

impl Drop for StreamPlace {
    fn drop(&mut self) {
        let mut count = self.count.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(open) = count.get_mut(&self.token) {
            *open -= 1;
            if *open == 0 {
                count.remove(&self.token);
            }
        }
    }
}

/// The body of an event stream: what the task of the connection sends.
struct EventStream {
    rx: mpsc::Receiver<Bytes>,
}

impl Stream for EventStream {
    type Item = Result<Bytes, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx).map(|item| item.map(Ok))
    }
}

pub(crate) fn routes(_core: &Arc<Core<Status>>) -> Router<Arc<Core<Status>>> {
    Router::new()
        .route(&IDENTITY.route("state"), get(state))
        .route(&IDENTITY.route("stream"), get(stream))
        .route(&IDENTITY.route("ops"), post(ops))
}

fn not_ready() -> Response {
    error_body(
        StatusCode::SERVICE_UNAVAILABLE,
        "not_ready",
        "the mixer has not reported yet",
    )
}

/// `GET /api/pult/state`: the whole mixer, as the owner last published it.
async fn state(State(core): State<Arc<Core<Status>>>, RawQuery(query): RawQuery) -> Response {
    if let Some(response) = no_query(query.as_deref()) {
        return response;
    }
    let text = core.rt().link.state.borrow().clone();
    match text {
        Some(text) => json_response(StatusCode::OK, text.to_string(), None),
        None => not_ready(),
    }
}

/// `POST /api/pult/ops`: one operation. The route checks only its form; whether it fits is for
/// the mixer to say, and its refusal goes back unchanged (`SRV-18`).
async fn ops(
    State(core): State<Arc<Core<Status>>>,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Response {
    if let Some(response) = no_query(query.as_deref()) {
        return response;
    }
    // An object first: serde would also read an operation from an array of its fields.
    let op = serde_json::from_slice::<serde_json::Map<String, Value>>(&body)
        .and_then(|fields| serde_json::from_value::<Op>(Value::Object(fields)));
    let Ok(op) = op else {
        return error_body(
            StatusCode::BAD_REQUEST,
            "bad_op",
            "not an operation; see docs/CONTROL.md",
        );
    };
    let (answer, reply) = oneshot::channel();
    if core.rt().link.ops.try_send(Asked { op, answer }).is_err() {
        return error_body(
            StatusCode::SERVICE_UNAVAILABLE,
            "busy",
            "the mixer has too many operations waiting",
        );
    }
    match tokio::time::timeout(ANSWER_WITHIN, reply).await {
        Ok(Ok(Ok(lines))) => {
            json_response(StatusCode::OK, json!({ "lines": lines }).to_string(), None)
        }
        Ok(Ok(Err(why))) => error_body(StatusCode::UNPROCESSABLE_ENTITY, "refused", &why),
        Ok(Err(_)) | Err(_) => error_body(
            StatusCode::GATEWAY_TIMEOUT,
            "no_answer",
            "the mixer did not answer; the operation may still happen",
        ),
    }
}

/// `GET /api/pult/stream`: server-sent events, one `state` event each time the owner publishes,
/// at most ten a second. Every event is the whole state, so a slow client only misses states
/// and has nothing to catch up on.
async fn stream(
    State(core): State<Arc<Core<Status>>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = no_query(query.as_deref()) {
        return response;
    }
    // The guard has checked the session; the token is the key of the count.
    let token = bearer(&headers).unwrap_or_default();
    let rt = core.rt();
    let Some(place) = rt
        .streams
        .acquire(&token, core.limits.max_streams_per_session)
    else {
        return error_body(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_streams",
            "this session has too many open streams",
        );
    };
    let mut changes = rt.link.state.clone();
    let heartbeat = core.limits.heartbeat;
    let (tx, rx) = mpsc::channel::<Bytes>(4);
    let shared = Arc::clone(&core);
    tokio::spawn(async move {
        let _place = place;
        let mut ticker =
            tokio::time::interval_at(tokio::time::Instant::now() + heartbeat, heartbeat);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        changes.mark_changed();
        loop {
            let message = tokio::select! {
                outcome = changes.changed() => {
                    if outcome.is_err() {
                        return;
                    }
                    let text = changes.borrow_and_update().clone();
                    match text {
                        Some(text) => format!("event: state\ndata: {text}\n\n"),
                        None => continue,
                    }
                }
                _ = ticker.tick() => ": ping\n\n".to_owned(),
                () = tx.closed() => return,
            };
            // A session that was closed takes its streams with it.
            if !shared.sessions.check(&token) {
                return;
            }
            if tx.send(Bytes::from(message)).await.is_err() {
                return;
            }
            // The owner may publish faster than its promise; a client gets no more than this.
            tokio::time::sleep(PUBLISH_EVERY).await;
        }
    });
    let mut response = Response::new(Body::from_stream(EventStream { rx }));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    response
}
