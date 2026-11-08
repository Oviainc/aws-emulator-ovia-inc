// SPDX-License-Identifier: CC0-1.0

//! The Event API's HTTP endpoint — `Api$dns`'s `HTTP` record — and the PUBLISH path behind
//! it, which the real-time endpoint shares.
//!
//! What it serves, over HTTP/1.1 on the API's own socket:
//!
//! > `POST /event` with `{"channel": …, "events": [ … ]}` publishes the events to one
//! > channel of one of the API's channel namespaces. The namespace is the channel's first
//! > segment; its `onPublish` handler ([`super::handlers`]) decides which of the events are
//! > published; each of those is delivered to every subscription whose channel covers the
//! > published one, as a `data` frame on that subscriber's socket; and the answer says
//! > which events the namespace accepted and which it could not.
//!
//! The transport-safety decisions the core makes for its own HTTP listeners are applied
//! here from the core's own predicates — the `Host` allowlist that guards against DNS
//! rebinding, the WebSocket refusal (the real-time endpoint is the other socket), and the
//! browser (`Origin` / `Sec-Fetch-Site`) check — and the request is read under one deadline
//! and one body ceiling, both the core's, so a slow or oversized publisher cannot hold a
//! connection permit open.

use ::std::future::Future;
use ::std::pin::Pin;
use ::std::sync::Arc;
use ::std::time::Duration;

use ::bytes::Bytes;
use ::emulator_server::{listener_caps, ListenerConn, NativeHandler, TlsUpgrader};
use ::http_body_util::{BodyExt as _, Full, Limited};
use ::serde_json::{json, Value as Json};
use ::tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::metrics::PublishCounts;
use super::{handlers, PlaneContext, PresentedAuth};
use crate::state::event_data_plane as plane;

/// The path the HTTP endpoint publishes at.
pub const PUBLISH_PATH: &str = "/event";

/// How many events one publish may carry.
///
/// AppSync's own limit for an Event API publish: a request carries up to five events, and
/// one past it is refused rather than partly accepted.
pub const MAX_EVENTS_PER_PUBLISH: usize = 5;

/// Serves one Event API's HTTP endpoint: the socket the API's `HTTP` DNS record publishes.
pub struct PublishSocket {
    /// What every request on this socket is answered from.
    context: PlaneContext,
    /// The core's TLS server, for a client that dialled `https://`.
    upgrader: Option<TlsUpgrader>,
    /// How many requests this endpoint reads at once, which is what bounds the memory the
    /// bodies in flight on it hold: the core's own HTTP in-flight cap, one permit per
    /// request, taken before a body is read and released when the answer is written.
    in_flight: Arc<Semaphore>,
}

impl PublishSocket {
    /// The endpoint of one API.
    pub fn new(context: PlaneContext, upgrader: Option<TlsUpgrader>) -> Self {
        Self {
            context,
            upgrader,
            in_flight: Arc::new(Semaphore::new(listener_caps::MAX_HTTP_INFLIGHT)),
        }
    }
}

impl NativeHandler for PublishSocket {
    /// One call per accepted connection. The handles are cloned into the connection's own
    /// future, which is what the core's `'static` handler contract asks for.
    fn serve(&self, conn: ListenerConn) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let context = self.context.clone();
        let upgrader = self.upgrader.clone();
        let in_flight = Arc::clone(&self.in_flight);
        Box::pin(async move {
            let Some(conn) = super::negotiated(conn, upgrader).await else {
                return;
            };
            serve_connection(context, in_flight, conn).await;
        })
    }
}

/// Speak HTTP/1.1 on one accepted connection until the client closes it.
async fn serve_connection(context: PlaneContext, in_flight: Arc<Semaphore>, conn: ListenerConn) {
    let api_id = context.api_id.clone();
    let io = ::hyper_util::rt::TokioIo::new(conn);
    let service = ::hyper::service::service_fn(move |request| {
        let context = context.clone();
        let in_flight = Arc::clone(&in_flight);
        async move {
            // Every request's whole answer is driven through the core's panic containment,
            // because this socket is served by this module rather than by the front door's
            // router: there is no middleware layer to carry it.
            let facts = ::emulator_server::RequestFacts::from_request(&request);
            Ok::<_, ::std::convert::Infallible>(
                contained(facts, answer(context, in_flight, request)).await,
            )
        }
    });
    if let Err(error) = ::hyper::server::conn::http1::Builder::new()
        .timer(::hyper_util::rt::TokioTimer::new())
        .header_read_timeout(request_deadline())
        .serve_connection(io, service)
        .await
    {
        // A connection the client dropped, or a request this endpoint could not frame,
        // ends here and takes nothing else with it.
        ::tracing::debug!(
            %api_id,
            %error,
            "a connection to an Event API's HTTP endpoint ended before its response"
        );
    }
}

/// Drive one request's whole answer under the core's own PANIC CONTAINMENT
/// (`emulator_server::contain_panics`), and word a contained panic in this endpoint's own
/// envelope.
///
/// A panic in a handler costs the caller its response; a panic that unwinds the task
/// serving the CONNECTION costs it the connection and leaves it reading a dead socket. The
/// front door layers that containment over its router; a plane served on a core-owned
/// socket has no such layer, so it drives each answer through the same function the core
/// exports for exactly this case. The panic is logged there with the request's facts and is
/// not swallowed: it stays a visible emulator defect.
///
/// A contained panic arrives back as the core's protocol-shaped `500`, which is re-worded
/// as the service's own `InternalFailureException` — "An internal AppSync error occurred.
/// Try your request again." — so a client of this endpoint reads one envelope whatever
/// failed. It is the ONLY `500` this endpoint can answer: every refusal it words itself
/// carries a `4xx` or the `503` of a shed request.
async fn contained<F>(
    facts: ::emulator_server::RequestFacts,
    answer: F,
) -> ::http::Response<Full<Bytes>>
where
    F: ::std::future::Future<Output = ::http::Response<Full<Bytes>>> + Send + 'static,
{
    let guarded: ::http::Response<Bytes> = ::emulator_server::contain_panics(facts, async move {
        let (parts, body) = answer.await.into_parts();
        let bytes = body
            .collect()
            .await
            .map_or_else(|_| Bytes::new(), |collected| collected.to_bytes());
        ::http::Response::from_parts(parts, bytes)
    })
    .await;
    let (parts, bytes) = guarded.into_parts();
    if parts.status == ::http::StatusCode::INTERNAL_SERVER_ERROR {
        return error_response(500, INTERNAL_FAILURE_ERROR_TYPE, INTERNAL_FAILURE_MESSAGE);
    }
    ::http::Response::from_parts(parts, Full::new(bytes))
}

/// Answer one request to the HTTP endpoint.
async fn answer(
    context: PlaneContext,
    in_flight: Arc<Semaphore>,
    request: ::http::Request<::hyper::body::Incoming>,
) -> ::http::Response<Full<Bytes>> {
    let (parts, body) = request.into_parts();
    if let Some(refused) = refused_before_dispatch(&parts) {
        return refused;
    }
    // One permit per request IN FLIGHT, taken before a byte of the body is read: a request
    // that finds none is shed, which is what the core's own HTTP listener answers such a
    // request with.
    let Ok(permit) = Arc::clone(&in_flight).try_acquire_owned() else {
        return error_response(503, "ServiceUnavailableException", SERVER_BUSY);
    };
    let limited = Limited::new(body, body_ceiling());
    let served = ::tokio::time::timeout(request_deadline(), async move {
        // Held for the whole read and the publish, and released with them.
        let _permit: OwnedSemaphorePermit = permit;
        let Ok(collected) = limited.collect().await else {
            return error_response(413, BAD_REQUEST_ERROR_TYPE, BODY_TOO_LARGE);
        };
        served_request(&context, &parts, &collected.to_bytes()).await
    })
    .await;
    served.unwrap_or_else(|_| error_response(408, BAD_REQUEST_ERROR_TYPE, REQUEST_TIMED_OUT))
}

/// Answer one framed request: the publish route, and the refusals every other request to
/// this endpoint meets.
async fn served_request(
    context: &PlaneContext,
    parts: &::http::request::Parts,
    body: &[u8],
) -> ::http::Response<Full<Bytes>> {
    if parts.uri.path() != PUBLISH_PATH {
        return error_response(404, NOT_FOUND_ERROR_TYPE, NO_SUCH_ROUTE);
    }
    if parts.method != ::http::Method::POST {
        return error_response(405, BAD_REQUEST_ERROR_TYPE, PUBLISH_IS_A_POST);
    }
    let Ok(document) = ::serde_json::from_slice::<Json>(body) else {
        return error_response(400, BAD_REQUEST_ERROR_TYPE, PUBLISH_BODY_IS_A_DOCUMENT);
    };
    let Some(channel) = document.get("channel").and_then(Json::as_str) else {
        return error_response(400, BAD_REQUEST_ERROR_TYPE, PUBLISH_NAMES_A_CHANNEL);
    };
    let Some(events) = document.get("events").and_then(Json::as_array) else {
        return error_response(400, BAD_REQUEST_ERROR_TYPE, PUBLISH_CARRIES_EVENTS);
    };
    let auth = PresentedAuth::from_fields(&parts.headers);
    match publish(context, &auth, channel, events).await {
        Ok(outcome) => json_response(200, &outcome.answered()),
        Err(refusal) => error_response(
            refusal.status(),
            refusal.error_type(),
            &refusal.message(),
        ),
    }
}

/// What a publish came to: which events the namespace accepted, and which it could not.
///
/// An event the namespace's handler filtered out is in [`Self::successful`] all the same:
/// the publish of it succeeded, and what the handler decided is which subscribers it
/// reaches.
#[derive(Debug, Default, Clone)]
pub struct PublishOutcome {
    /// The accepted events, each as the answer reports one.
    pub successful: Vec<Json>,
    /// The events the namespace could not accept, each with why.
    pub failed: Vec<Json>,
}

impl PublishOutcome {
    /// This outcome as the body a publish is answered with.
    pub fn answered(&self) -> Json {
        json!({"failed": self.failed, "successful": self.successful})
    }
}

/// Why a publish was not served at all — as against one whose events were served and some
/// of them failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishRefusal {
    /// The material presented does not authorize a publish on this API.
    Unauthorized,
    /// The API itself is gone — its record was deleted while this socket was still
    /// draining.
    NoSuchApi,
    /// The channel names a channel namespace this API does not hold.
    NoSuchNamespace,
    /// The request is not a publish at all: no namespace in the channel, or more events
    /// than one publish carries.
    BadRequest(String),
}

impl PublishRefusal {
    /// The status this refusal is answered with.
    pub fn status(&self) -> u16 {
        match self {
            Self::Unauthorized => 401,
            Self::NoSuchApi | Self::NoSuchNamespace => 404,
            Self::BadRequest(_) => 400,
        }
    }

    /// The `errorType` it carries.
    pub fn error_type(&self) -> &'static str {
        match self {
            Self::Unauthorized => super::UNAUTHORIZED_ERROR_TYPE,
            Self::NoSuchApi | Self::NoSuchNamespace => NOT_FOUND_ERROR_TYPE,
            Self::BadRequest(_) => BAD_REQUEST_ERROR_TYPE,
        }
    }

    /// The sentence it states.
    pub fn message(&self) -> String {
        match self {
            Self::Unauthorized => super::UNAUTHORIZED_MESSAGE.to_string(),
            // The sentence the control plane answers for an API that is not there, so the
            // two planes word one absence one way.
            Self::NoSuchApi => crate::state::API_NOT_FOUND_MESSAGE.to_string(),
            // The same, for a channel namespace the addressed API does not hold.
            Self::NoSuchNamespace => crate::state::CHANNEL_NAMESPACE_NOT_FOUND_MESSAGE.to_string(),
            Self::BadRequest(message) => message.clone(),
        }
    }
}

/// Publish `events` to `channel` of the API `context` serves — the ONE publish path, which
/// the HTTP endpoint and a `publish` message on the real-time endpoint both take.
///
/// In order: resolve the channel's namespace and authorize the call against the API's own
/// records; read each event, which is "a JSON string"; run the namespace's `onPublish`
/// handler over them; deliver what it returned to every subscription the channel covers;
/// publish what the call did to `AWS/AppSync`; and answer which events were accepted.
pub async fn publish(
    context: &PlaneContext,
    auth: &PresentedAuth,
    channel: &str,
    events: &[Json],
) -> Result<PublishOutcome, PublishRefusal> {
    let path = plane::normalised_channel(channel);
    let Some(namespace_name) = plane::channel_namespace_name(channel).map(str::to_string) else {
        return Err(PublishRefusal::BadRequest(
            PUBLISH_NAMES_A_CHANNEL.to_string(),
        ));
    };
    if events.len() > MAX_EVENTS_PER_PUBLISH {
        return Err(PublishRefusal::BadRequest(format!(
            "A publish carries at most {MAX_EVENTS_PER_PUBLISH} events; this one carries {}.",
            events.len()
        )));
    }
    // One pass of the store: the authorization decision, the namespace's handler code and
    // an identifier per event, all read together so a publish cannot be served from a
    // namespace that was deleted between two reads. The guard is released before the
    // handler runs, because nothing of this service's state may be held across an await.
    let (code_handlers, identifiers) = {
        let mut store = context.state.lock();
        if !store.event_apis.contains_key(&context.api_id) {
            return Err(PublishRefusal::NoSuchApi);
        }
        if !super::authorized(
            &store,
            &context.api_id,
            super::PlaneCall::Publish,
            Some(&namespace_name),
            auth,
        ) {
            return Err(PublishRefusal::Unauthorized);
        }
        let Some(namespace) = store
            .event_apis
            .get(&context.api_id)
            .and_then(|api| api.channel_namespaces.get(&namespace_name))
        else {
            return Err(PublishRefusal::NoSuchNamespace);
        };
        let code_handlers = namespace.code_handlers.clone();
        let identifiers: Vec<String> = (0..events.len())
            .map(|_| store.event_data_plane.mint_event_identifier())
            .collect();
        (code_handlers, identifiers)
    };

    // Each event is "a list of up to 5 events as JSON strings": a value that is not a
    // string, or a string that is not a document, is no event and is reported as the one
    // that failed rather than failing the whole publish. The identifiers are ZIPPED onto
    // the events — one was minted per event above — so no request-derived index addresses
    // anything.
    let mut accepted: Vec<(usize, String, Json)> = Vec::new();
    let mut failed: Vec<Json> = Vec::new();
    for (index, (event, identifier)) in events.iter().zip(identifiers).enumerate() {
        match read_event(event) {
            Some(payload) => accepted.push((index, identifier, payload)),
            None => failed.push(failed_event(&identifier, index, EVENT_IS_A_JSON_STRING)),
        }
    }

    let run = match code_handlers.as_deref() {
        Some(code) => {
            let context_document = handler_context(&path, &namespace_name, &accepted);
            handlers::evaluate(code.to_string(), handlers::ON_PUBLISH, context_document).await
        }
        None => handlers::HandlerRun::NotDeclared,
    };
    let invocations = u32::from(!matches!(run, handlers::HandlerRun::NotDeclared));
    let delivered = match &run {
        // A namespace with no `onPublish` handler publishes what it was given.
        handlers::HandlerRun::NotDeclared => accepted.clone(),
        handlers::HandlerRun::Returned(returned) => returned_events(returned, &accepted),
        handlers::HandlerRun::Failed(reason) => {
            ::tracing::info!(
                api_id = %context.api_id,
                namespace = %namespace_name,
                %reason,
                "a channel namespace's onPublish handler did not return"
            );
            Vec::new()
        }
    };
    // A handler that did not return accepts nothing: every event it was given failed,
    // with what the handler failed with.
    if let handlers::HandlerRun::Failed(reason) = &run {
        for (index, identifier, _) in &accepted {
            failed.push(failed_event(identifier, *index, reason));
        }
        accepted.clear();
    }

    let dropped = accepted.len().saturating_sub(delivered.len());
    deliver(context, &path, &delivered).await;
    // Every count is bounded by [`MAX_EVENTS_PER_PUBLISH`], so none of them loses
    // anything as a metric value.
    let counted = |count: usize| u32::try_from(count).unwrap_or(u32::MAX);
    context.metrics.published(
        &namespace_name,
        PublishCounts {
            handler_invocations: invocations,
            successful: counted(accepted.len()),
            dropped: counted(dropped),
            failed: counted(failed.len()),
        },
    );
    Ok(PublishOutcome {
        successful: accepted
            .iter()
            .map(|(index, identifier, _)| json!({"identifier": identifier, "index": index}))
            .collect(),
        failed,
    })
}

/// One event's payload: an event is published as "a JSON string", so a string is read as
/// the document it holds. `None` for a value that is no event at all.
fn read_event(event: &Json) -> Option<Json> {
    let text = event.as_str()?;
    ::serde_json::from_str(text).ok()
}

/// The `ctx` a channel namespace's handler is called with.
///
/// What AppSync's own event handlers read off it: `ctx.events`, each event with the `id`
/// this publish minted for it and the `payload` it carries, and `ctx.info`, which names the
/// channel the publish addressed, its segments, the channel namespace it belongs to and the
/// operation being handled.
fn handler_context(path: &str, namespace: &str, accepted: &[(usize, String, Json)]) -> Json {
    let events: Vec<Json> = accepted
        .iter()
        .map(|(_, identifier, payload)| json!({"id": identifier, "payload": payload}))
        .collect();
    json!({
        "events": events,
        "info": {
            "channel": {
                "path": path,
                "segments": plane::channel_segments(path),
            },
            "channelNamespace": {"name": namespace},
            "operation": "PUBLISH",
        },
        "stash": {},
    })
}

/// The events a handler's answer says to publish, matched back onto the events it was
/// given.
///
/// A handler "must return the events that should be published", each carrying the `id` it
/// was given, so the answer is read as a list of events: an entry whose `id` names one of
/// the given events publishes that event with the entry's own `payload`, and a given event
/// no entry names is one the handler filtered out. A handler that returned nothing
/// published nothing, and an answer that is not a list of events publishes nothing either —
/// there is no list of events in it to publish.
fn returned_events(
    returned: &Json,
    accepted: &[(usize, String, Json)],
) -> Vec<(usize, String, Json)> {
    let Some(entries) = returned.as_array() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let id = entry.get("id").and_then(Json::as_str)?;
            let (index, identifier, given) = accepted
                .iter()
                .find(|(_, identifier, _)| identifier == id)?;
            let payload = entry.get("payload").cloned().unwrap_or_else(|| given.clone());
            Some((*index, identifier.clone(), payload))
        })
        .collect()
}

/// Deliver each published event to every subscription whose channel covers `path`, as the
/// `data` frame a subscriber reads it off its own socket as.
///
/// The queues are taken out of the store and written to with no lock held. A subscriber
/// whose queue is full is not reading its socket, so the frame is dropped rather than
/// buffered without limit — the queue is that subscriber's whole allowance, and an event's
/// own bytes are bounded by [`super::MAX_MESSAGE_BYTES`], so what one publish can put in
/// flight is bounded in bytes and not only in frames.
///
/// The fan-out YIELDS between subscribers: the number of them is bounded
/// (`crate::state::event_data_plane`'s connections per API times its subscriptions per
/// connection), and yielding keeps the executor serving other connections and leaves a
/// cancellation somewhere to land while a wide fan-out is written.
async fn deliver(context: &PlaneContext, path: &str, published: &[(usize, String, Json)]) {
    if published.is_empty() {
        return;
    }
    let deliveries = context
        .state
        .lock()
        .event_data_plane
        .deliveries(&context.api_id, path);
    for (subscription_id, frames) in deliveries {
        for (_, _, payload) in published {
            let frame = data_frame(&subscription_id, payload);
            if frames.try_send(frame).is_err() {
                ::tracing::warn!(
                    api_id = %context.api_id,
                    %subscription_id,
                    "a subscriber is not reading its socket; an event was not delivered to it"
                );
                break;
            }
        }
        ::tokio::task::yield_now().await;
    }
}

/// The frame one published event is delivered to a subscription as: the subscription's own
/// id and the event, which travels as the JSON string it was published as.
pub fn data_frame(subscription_id: &str, payload: &Json) -> String {
    json!({
        "type": "data",
        "id": subscription_id,
        "event": payload.to_string(),
    })
    .to_string()
}

/// One entry of a publish's `failed` list: which event it was and why it was not accepted.
fn failed_event(identifier: &str, index: usize, reason: &str) -> Json {
    json!({
        "identifier": identifier,
        "index": index,
        "errorCode": 400,
        "errorMessage": reason,
    })
}

/// The `errorType` a malformed request to this endpoint carries, as AppSync's own
/// `BadRequestException` names it.
const BAD_REQUEST_ERROR_TYPE: &str = "BadRequestException";

/// The `errorType` a request for something this endpoint does not hold carries.
const NOT_FOUND_ERROR_TYPE: &str = "NotFoundException";

/// The `errorType` and the sentence a CONTAINED PANIC is answered with: AppSync's own
/// server error, whose documentation words it "An internal AppSync error occurred. Try your
/// request again."
pub const INTERNAL_FAILURE_ERROR_TYPE: &str = "InternalFailureException";
pub const INTERNAL_FAILURE_MESSAGE: &str =
    "An internal AppSync error occurred. Try your request again.";

/// The sentences this endpoint's own refusals state.
const NO_SUCH_ROUTE: &str = "This endpoint serves POST /event.";
const PUBLISH_IS_A_POST: &str = "An event is published with POST.";
const PUBLISH_BODY_IS_A_DOCUMENT: &str = "The request body must be a JSON document.";
const PUBLISH_NAMES_A_CHANNEL: &str =
    "The request must name a channel of a channel namespace of this API.";
const PUBLISH_CARRIES_EVENTS: &str = "The request must carry a list of events.";
const EVENT_IS_A_JSON_STRING: &str = "An event must be a JSON string.";
const BODY_TOO_LARGE: &str = "This request is larger than this endpoint accepts.";
const REQUEST_TIMED_OUT: &str = "This request took longer than this endpoint allows.";
const SERVER_BUSY: &str = "This endpoint is serving as many requests as it can. Try again.";
const NO_WEBSOCKETS: &str = "A WebSocket connection is served at the API's real-time endpoint.";

/// How long one request on this endpoint may take, from its first header byte to the
/// answer: the core's own HTTP request deadline.
fn request_deadline() -> Duration {
    Duration::from_secs(listener_caps::HTTP_REQUEST_DEADLINE_SECS)
}

/// How many bytes of one request this endpoint reads: the bound this plane states for a
/// message on either endpoint ([`super::MAX_MESSAGE_BYTES`]), which is well under the
/// core's own per-connection ceiling — a published event is COPIED once per matching
/// subscription, so what one publish may carry has to be bounded in bytes and not only in
/// events.
fn body_ceiling() -> usize {
    super::MAX_MESSAGE_BYTES
}

/// The transport-safety refusals the core applies to its own HTTP listeners, applied here
/// from the core's own predicates so this endpoint and a core listener decide them alike.
fn refused_before_dispatch(
    parts: &::http::request::Parts,
) -> Option<::http::Response<Full<Bytes>>> {
    let header = |name: &str| parts.headers.get(name).and_then(|value| value.to_str().ok());
    // HTTP/1.1 requires a `Host`, and without an authority the rebinding guard cannot be
    // run at all, so a request carrying none is refused rather than served: this fails
    // CLOSED, exactly as the core's own HTTP listener does.
    let Some(host) = header("host").or_else(|| parts.uri.host()) else {
        return Some(error_response(421, BAD_REQUEST_ERROR_TYPE, super::HOST_REQUIRED));
    };
    if !super::host_allowed(host) {
        return Some(error_response(
            421,
            BAD_REQUEST_ERROR_TYPE,
            super::HOST_NOT_ALLOWED,
        ));
    }
    if ::emulator_server::is_websocket_upgrade(
        header("connection"),
        header("upgrade"),
        header(":protocol"),
    ) {
        return Some(error_response(400, BAD_REQUEST_ERROR_TYPE, NO_WEBSOCKETS));
    }
    if let Some(reason) = super::cross_site_browser(header("origin"), header("sec-fetch-site")) {
        return Some(error_response(403, super::UNAUTHORIZED_ERROR_TYPE, &reason));
    }
    None
}

/// One answer carrying a document.
fn json_response(status: u16, document: &Json) -> ::http::Response<Full<Bytes>> {
    let body = Bytes::from(document.to_string());
    ::http::Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .header("content-length", body.len().to_string())
        .body(Full::new(body.clone()))
        // Every argument is this function's own, so the builder cannot fail; answer a bare
        // status rather than panicking if it ever does.
        .unwrap_or_else(|_| {
            let mut response = ::http::Response::new(Full::new(body));
            *response.status_mut() = ::http::StatusCode::INTERNAL_SERVER_ERROR;
            response
        })
}

/// One refusal, in the envelope AppSync answers a data-plane error in: a list of errors,
/// each naming its type and stating its sentence.
fn error_response(
    status: u16,
    error_type: &str,
    message: &str,
) -> ::http::Response<Full<Bytes>> {
    json_response(status, &errors_document(error_type, message))
}

/// The `{"errors": [{"errorType": …, "message": …}]}` document a refusal carries, which is
/// also what a `subscribe_error` frame carries on the real-time endpoint.
pub fn errors_document(error_type: &str, message: &str) -> Json {
    json!({"errors": [{"errorType": error_type, "message": message}]})
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The facts a request to this endpoint is contained under.
    fn facts() -> ::emulator_server::RequestFacts {
        ::emulator_server::RequestFacts::from_request(
            &::http::Request::builder()
                .method("POST")
                .uri(PUBLISH_PATH)
                .header("content-type", "application/json")
                .body(())
                .expect("a well-formed request"),
        )
    }

    /// The status and the parsed body of one answer.
    async fn answered(response: ::http::Response<Full<Bytes>>) -> (u16, Json) {
        let (parts, body) = response.into_parts();
        let bytes = body
            .collect()
            .await
            .map_or_else(|_| Bytes::new(), |collected| collected.to_bytes());
        (
            parts.status.as_u16(),
            ::serde_json::from_slice(&bytes).unwrap_or(Json::Null),
        )
    }

    #[tokio::test]
    async fn a_panic_serving_one_request_is_answered_rather_than_taking_the_connection() {
        // The rows span what the containment generalises over: an answer this endpoint
        // produced normally, a panic raised before the first await, and one raised after an
        // await (the case a plain `catch_unwind` around the call would miss).
        let healthy = contained(facts(), async {
            json_response(200, &json!({"failed": [], "successful": []}))
        })
        .await;
        assert_eq!(
            answered(healthy).await,
            (200, json!({"failed": [], "successful": []})),
            "case \"a healthy answer\": the containment must pass it through untouched"
        );

        for case in ["a panic before any await", "a panic after an await"] {
            let awaited = case.contains("after");
            let (status, body) = answered(
                contained(facts(), async move {
                    if awaited {
                        ::tokio::task::yield_now().await;
                    }
                    panic!("an emulator defect on the publish route")
                })
                .await,
            )
            .await;
            assert_eq!(
                (
                    status,
                    body.pointer("/errors/0/errorType").and_then(Json::as_str),
                    body.pointer("/errors/0/message").and_then(Json::as_str),
                ),
                (
                    500,
                    Some(INTERNAL_FAILURE_ERROR_TYPE),
                    Some(INTERNAL_FAILURE_MESSAGE),
                ),
                "case {case:?}: expected the service's own internal failure, got {status} {body}"
            );
        }
    }
}
