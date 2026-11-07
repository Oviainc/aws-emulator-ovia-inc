// SPDX-License-Identifier: CC0-1.0

//! The SOCKET a WEBSOCKET api is served on: the core-owned listener its `ApiEndpoint`
//! announces, the RFC 6455 opening handshake it completes, and the frames it then carries.
//!
//! A WebSocket api cannot be served through the emulator's front door: that door serves ONE
//! buffered request per call, and the seams a service is given (`Service::can_handle`,
//! `Service::override_operation`, `Service::override_routing_error`) take an already-buffered
//! request and return a buffered response, so nothing there can switch a connection to
//! another protocol and carry frames over it. What a WebSocket api needs is a socket, which
//! is exactly what the core lends a service for a resource with a native protocol:
//! `engine.listeners().tcp_plaintext(handler)` binds it on the operator-chosen interface
//! (loopback by default), assigns the port, caps the connections and severs them when the
//! handle is dropped, and hands this module each accepted connection's byte stream. The
//! address it reports is what the api announces as its `ApiEndpoint`, so the URI a caller
//! hands a WebSocket client library is one that client can reach
//! ([`crate::state::websocket_transport`]).
//!
//! # What this module does with a connection, in order
//!
//! 1. ENCRYPTION, negotiated in band. The socket is handed over plaintext and the first byte
//!    says what the client opened with: a TLS record (`0x16`) is handed to the core's own TLS
//!    server ([`::emulator_server::TlsUpgrader`]) and everything after it is read decrypted,
//!    while an HTTP method is read as it stands. So ONE announced address serves both the
//!    `wss://` a client of AWS dials and the `ws://` a client that does not verify a
//!    self-signed certificate dials, and the service never touches a key or a certificate.
//! 2. The HANDSHAKE, by the maintained RFC 6455 implementation (`tokio-tungstenite`): it
//!    reads the opening handshake, derives `Sec-WebSocket-Accept` and writes either
//!    `101 Switching Protocols` or the refusal this module decided ([`admission`]). Nothing
//!    about the protocol is hand-written here.
//! 3. The FRAMES, each dispatched to the route the api's `RouteSelectionExpression` selects
//!    and answered by that route's integration ([`super::dispatch`]), and each frame pushed
//!    TO the connection written back over the same socket.
//! 4. The CLOSE, which ends the connection's record: from then on its `ConnectionId`
//!    addresses a connection that no longer exists.
//!
//! Every answer is computed from this service's own records — the api, its stage, the
//! configuration that stage's deployment froze — read under the one shared store lock, and
//! the authorization the connection request's route carries is applied by the SAME rule the
//! service endpoint applies it by ([`super::connect_authorization_refusal`]).

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::SystemTime;

use ::emulator_server::{ListenerConn, NativeHandler, TlsUpgrader};
use ::futures_util::{SinkExt, StreamExt};
use ::service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi::state as connections;
use ::tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use ::tokio::sync::mpsc::{channel, Receiver, Sender};
use ::tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as HandshakeRequest, Response as HandshakeResponse,
};
use ::tokio_tungstenite::tungstenite::Message;

use super::dispatch;
use crate::state::ComAmazonawsApigatewayv2Apigatewayv2State as ServiceStore;

/// The first byte of a TLS record, which is what a client that dialled `wss://` opens with:
/// the `handshake` content type (22) of TLS 1.0 and later. An opening handshake made in the
/// clear begins with an HTTP method instead, so one byte tells the two apart.
const TLS_RECORD_FIRST_BYTE: u8 = 0x16;

/// The member a gateway frame carries its reason under, and the two members it names the
/// call by: API Gateway's own frames for a message it cannot route or serve are
/// `{"message": …, "connectionId": …, "requestId": …}`.
const FRAME_MESSAGE_MEMBER: &str = "message";
const FRAME_CONNECTION_ID_MEMBER: &str = "connectionId";
const FRAME_REQUEST_ID_MEMBER: &str = "requestId";

/// Open the socket `api_id`'s WebSocket data plane is served on and remember it, answering
/// the `host:port` it landed on — the authority that api's `ApiEndpoint` announces.
///
/// The core owns the socket, the interface, the port and the connection caps; this hands it
/// the handler and stores the returned handle in the service's own state, which is what keeps
/// the port open for as long as the api exists. `Err` when this service was mounted on an
/// engine with no listener surface, or when the core could not bind: the caller then answers
/// that rather than announcing an address nothing serves.
pub async fn open_for_api<Eng: Send + Sync + 'static>(
    engine: &Eng,
    api_id: &str,
) -> Result<String, String> {
    let shared = (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .ok_or_else(|| "this emulator's engine has no listener surface".to_string())?;
    let state = crate::state::service_state(engine)
        .ok_or_else(|| "this emulator's engine does not carry this service's store".to_string())?;
    let listeners = shared.listeners();
    let registry = connections::registry(engine).ok_or_else(|| {
        "this emulator's engine does not carry the WebSocket connection registry".to_string()
    })?;
    let handler = StageSocket {
        state: state.clone(),
        registry,
        api_id: api_id.to_string(),
        // The core's own TLS server, for a client that dialled `wss://`. A core that cannot
        // prepare it leaves this `None`, and such a client is then refused at the first byte
        // rather than being answered in the clear.
        upgrader: listeners.tls_upgrader().ok(),
    };
    let listener = listeners
        .tcp_plaintext(handler)
        .await
        .map_err(|error| format!("the api's WebSocket socket could not be opened: {error}"))?;
    let authority = state
        .lock()
        .websocket_transport
        .remember_listener(api_id, Arc::new(listener));
    tracing::info!(%api_id, %authority, "opened a WebSocket api's data-plane socket");
    Ok(authority)
}

/// The handler the core hands each connection accepted on ONE api's socket.
///
/// It carries the shared store (so every connection reads the same records the control plane
/// writes), the WebSocket CONNECTION REGISTRY the `@connections` management API serves (so a
/// connection this socket carries is the connection that API reports), and the api its
/// socket belongs to — the api is known from the socket itself, so no host or path has to
/// name it.
struct StageSocket {
    state: ::engine::ServiceState<ServiceStore>,
    registry: ::engine::ServiceState<connections::ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState>,
    api_id: String,
    upgrader: Option<TlsUpgrader>,
}

impl NativeHandler for StageSocket {
    fn serve(&self, conn: ListenerConn) -> Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        // Owned clones before the future: the core's handler contract hands back a `'static`
        // future, so nothing may borrow `&self` across it.
        let state = self.state.clone();
        let registry = self.registry.clone();
        let api_id = self.api_id.clone();
        let upgrader = self.upgrader.clone();
        Box::pin(async move {
            serve_connection(state, registry, api_id, upgrader, conn).await;
        })
    }
}

/// Serve one accepted connection: negotiate encryption, complete (or refuse) the handshake,
/// then carry frames until either side closes.
// The handshake callback's `Err` is the REFUSAL RESPONSE the WebSocket implementation writes
// (`http::Response<Option<String>>`), which is that dependency's own signature and is large
// by the `http` crate's measure. Boxing it is not available: the callback's type is the one
// `accept_hdr_async_with_config` takes.
#[allow(clippy::result_large_err)]
async fn serve_connection(
    state: ::engine::ServiceState<ServiceStore>,
    registry: ::engine::ServiceState<connections::ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState>,
    api_id: String,
    upgrader: Option<TlsUpgrader>,
    conn: ListenerConn,
) {
    let Some(conn) = negotiated(conn, upgrader).await else {
        return;
    };
    // Decided inside the handshake callback, which is where the request's path and headers
    // are, and read back here once the handshake has been written.
    let admitted: Arc<Mutex<Option<Admitted>>> = Arc::new(Mutex::new(None));
    let decide = {
        let state = state.clone();
        let api_id = api_id.clone();
        let admitted = Arc::clone(&admitted);
        move |request: &HandshakeRequest,
              response: HandshakeResponse|
              -> Result<HandshakeResponse, ErrorResponse> {
            match admission(&state, &api_id, request) {
                Ok(accepted) => {
                    *admitted.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
                        Some(accepted);
                    Ok(response)
                }
                Err(refusal) => Err(refusal.rendered()),
            }
        }
    };
    let socket = match ::tokio_tungstenite::accept_hdr_async_with_config(
        conn,
        decide,
        Some(socket_limits()),
    )
    .await
    {
        Ok(socket) => socket,
        Err(error) => {
            // Either the refusal above (already written to the client by the handshake) or a
            // client that did not complete the handshake at all.
            tracing::debug!(%api_id, %error, "a WebSocket opening handshake was not completed");
            return;
        }
    };
    let Some(accepted) = admitted
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
    else {
        return;
    };
    carry_frames(state, registry, api_id, accepted, socket).await;
}

/// The largest MESSAGE this data plane reads from a client, in bytes: the 128 KiB a frame of
/// an API Gateway WebSocket api may carry, which is also the modelled maximum of a payload
/// pushed the other way (`PostToConnection$Data`, `@length(max: 131072)`).
///
/// Stated HERE rather than left to the WebSocket implementation's own default, so the bound
/// a caller's bytes are buffered under is citable and survives a dependency bump: it is what
/// stands between one local caller and the host's memory, since the message is read into a
/// buffer sized by what that caller sent.
const MAX_MESSAGE_BYTES: usize = 131_072;

/// The largest FRAME this data plane reads, in bytes: the 32 KiB an API Gateway WebSocket
/// api fragments a message into, so a message at [`MAX_MESSAGE_BYTES`] arrives as several
/// frames and no single frame may be larger.
const MAX_FRAME_BYTES: usize = 32 * 1024;

/// How many bytes may wait on the WRITE side of one socket before the implementation stops
/// buffering: the depth of the pushes queued for a connection
/// ([`connections::CONNECTION_PUSH_DEPTH`]) times the largest payload one carries, so a
/// client that stops reading cannot pin more than the queue it is already bounded by.
const MAX_WRITE_BUFFER_BYTES: usize = connections::CONNECTION_PUSH_DEPTH * MAX_MESSAGE_BYTES;

/// The explicit size bounds every socket of this data plane is read and written under; see
/// the three constants above.
fn socket_limits() -> ::tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
    ::tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_FRAME_BYTES))
        .write_buffer_size(0)
        .max_write_buffer_size(MAX_WRITE_BUFFER_BYTES)
}

/// The connection with encryption negotiated: handed to the core's TLS server when the
/// client opened with a TLS record, and read as it stands otherwise. `None` for a client that
/// sent nothing, or whose TLS handshake this emulator could not complete.
async fn negotiated(mut conn: ListenerConn, upgrader: Option<TlsUpgrader>) -> Option<ListenerConn> {
    let mut first = [0u8; 1];
    match conn.read(&mut first).await {
        Ok(1) => {}
        // A client that closed before sending anything, or a read that failed: there is no
        // handshake to serve.
        _ => return None,
    }
    let prefixed: ListenerConn = Box::new(Prefixed {
        prefix: first.to_vec(),
        read: 0,
        inner: conn,
    });
    if first[0] != TLS_RECORD_FIRST_BYTE {
        return Some(prefixed);
    }
    let upgrader = upgrader?;
    // ALPN `None`: a WebSocket client offers `http/1.1` or nothing, and the handshake that
    // follows is HTTP/1.1 either way.
    match upgrader.accept(prefixed, None).await {
        Ok((upgraded, _negotiated)) => Some(upgraded),
        Err(error) => {
            tracing::debug!(%error, "a wss client's TLS handshake was not completed");
            None
        }
    }
}

/// A connection whose first bytes have already been read: they are served from the prefix
/// before the socket itself is read again, so a reader that has to see byte 0 — the core's
/// TLS server — reads the stream as the client sent it.
struct Prefixed<S> {
    prefix: Vec<u8>,
    read: usize,
    inner: S,
}

impl<S: AsyncRead + Unpin> AsyncRead for Prefixed<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.read < self.prefix.len() {
            let remaining = &self.prefix[self.read..];
            let taken = remaining.len().min(buffer.remaining());
            buffer.put_slice(&remaining[..taken]);
            self.read += taken;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(context, buffer)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Prefixed<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(context, buffer)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(context)
    }
}

/// What the handshake resolved: the stage of this api the connection reached, and what the
/// connection is attributed to.
#[derive(Debug, Clone)]
struct Admitted {
    stage_name: String,
    user_agent: Option<String>,
}

/// How a handshake this endpoint does not complete is answered: an HTTP status and the
/// `{"message": …}` body API Gateway's own endpoint answers it with.
#[derive(Debug, Clone)]
struct Refusal {
    status: u16,
    message: String,
}

impl Refusal {
    /// The handshake refusal as the RFC 6455 implementation writes it: the status, the JSON
    /// media type and the body. A status the `http` crate does not build is answered as the
    /// internal failure it is.
    fn rendered(&self) -> ErrorResponse {
        let body = ::serde_json::json!({ FRAME_MESSAGE_MEMBER: self.message }).to_string();
        ::http::Response::builder()
            .status(self.status)
            .header("content-type", super::super::data_plane::JSON_CONTENT_TYPE)
            .body(Some(body))
            .unwrap_or_else(|_| {
                ::http::Response::builder()
                    .status(super::super::data_plane::UNPLACEABLE_STATUS)
                    .body(None)
                    .expect("500 with no body builds")
            })
    }
}

/// Whether this opening handshake is completed, from the api's own records.
///
/// The steps, in the order API Gateway applies them, with the core's own browser check in
/// front of them:
///
/// 0. a handshake a BROWSER made from another site is refused before anything of the api is
///    read ([`browser_refusal`]) — the check the core applies to every request on one of its
///    own HTTP listeners, applied here because an RFC 6455 handshake is a request a page can
///    make to a loopback port without a CORS preflight;
/// 1. the path names a stage THIS api holds, and that stage has a deployment released to it;
///    a step that resolves nothing is `403` `{"message":"Forbidden"}`, which is what the
///    `execute-api` endpoint answers for a stage it does not serve;
/// 2. the AUTHORIZATION the `$connect` route carries is applied before anything that route
///    fronts, by the same rule the service endpoint applies it by
///    ([`super::connect_authorization_refusal`]);
/// 3. the CONNECTION REQUEST is served by the `$connect` route's integration, and the
///    handshake completes only on a successful reply ([`dispatch::connection_request`]): a
///    reply the configuration cannot produce is `500`, the answer a gateway gives for an
///    integration it cannot answer the connection request from.
fn admission(
    state: &::engine::ServiceState<ServiceStore>,
    api_id: &str,
    request: &HandshakeRequest,
) -> Result<Admitted, Refusal> {
    if let Some(refusal) = browser_refusal(request) {
        return Err(refusal);
    }
    let path = request.uri().path().to_string();
    let stage_name = super::super::data_plane::first_segment(&path)
        .unwrap_or_default()
        .to_string();
    let user_agent = request
        .headers()
        .get(::http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let store = state.lock();
    let Some(api) = store.apis.get(api_id) else {
        // The api was deleted between the handshake arriving and now; its socket is closing.
        return Err(forbidden());
    };
    let Some(stage) = api.stages.get(&stage_name) else {
        tracing::debug!(%api_id, stage = %stage_name, "a handshake named no stage of this api");
        return Err(forbidden());
    };
    let deployment = stage
        .deployment_id
        .as_deref()
        .and_then(|deployment_id| api.deployments.get(deployment_id));
    let Some(deployment) = deployment else {
        tracing::debug!(
            %api_id,
            stage = %stage_name,
            "a handshake named a stage with no deployment released to it"
        );
        return Err(forbidden());
    };
    if let Some((status, message)) = super::connect_authorization_refusal(
        api,
        stage,
        &deployment.configuration,
        |name: &str| {
            request
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        },
        &request
            .uri()
            .path_and_query()
            .map(|both| both.as_str().to_string())
            .unwrap_or_else(|| path.clone()),
    ) {
        return Err(Refusal {
            status,
            message: message.to_string(),
        });
    }
    match dispatch::connection_request(&deployment.configuration) {
        // No `$connect` route: there is nothing to refuse the connection.
        Ok(None) => Ok(Admitted {
            stage_name,
            user_agent,
        }),
        Ok(Some(reply)) if dispatch::is_success_status(reply.status_code.as_deref()) => {
            Ok(Admitted {
                stage_name,
                user_agent,
            })
        }
        Ok(Some(reply)) => {
            tracing::info!(
                %api_id,
                stage = %stage_name,
                status = ?reply.status_code,
                selected = %reply.selected_key,
                "refusing a WebSocket handshake: the connection request's integration did \
                 not reply successfully"
            );
            Err(internal_server_error())
        }
        Err(unserved) => {
            tracing::info!(
                %api_id,
                stage = %stage_name,
                ?unserved,
                "refusing a WebSocket handshake: the connection request's integration \
                 produced no reply"
            );
            Err(internal_server_error())
        }
    }
}

/// `403` `{"message":"Forbidden"}`, the answer an `execute-api` endpoint gives for a stage it
/// does not serve.
fn forbidden() -> Refusal {
    Refusal {
        status: super::super::data_plane::FORBIDDEN_STATUS,
        message: super::super::data_plane::FORBIDDEN_MESSAGE.to_string(),
    }
}

/// The refusal a handshake a BROWSER made from ANOTHER SITE owes, or `None` for every other
/// client.
///
/// An RFC 6455 handshake is one of the few requests a page can make to a port on the
/// developer's own machine with no CORS preflight to stop it, so the browser-set fields are
/// read by the CORE'S OWN reading of them (`emulator_server::classify_browser_request`,
/// which the core applies to every request on one of its HTTP listeners) rather than by a
/// second rule written here: a request carrying no `Origin` and no `Sec-Fetch-Site` is not a
/// browser's and is served, a same-origin one is served, and a cross-site one is refused
/// `403` before any record of the api is read. The policy is the default one — same-origin
/// only, and no remote origin reaches a data plane — because nothing here is an operator
/// knob.
fn browser_refusal(request: &HandshakeRequest) -> Option<Refusal> {
    let field = |name: ::http::HeaderName| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
    };
    let verdict = ::emulator_server::classify_browser_request(
        field(::http::header::ORIGIN),
        field(::http::HeaderName::from_static("sec-fetch-site")),
        &::emulator_server::RemoteOrigins::SameOriginOnly,
        false,
    );
    match verdict {
        ::emulator_server::BrowserVerdict::Allow => None,
        ::emulator_server::BrowserVerdict::Forbid(reason) => {
            tracing::info!(%reason, "refusing a WebSocket handshake a browser made cross-site");
            Some(forbidden())
        }
    }
}

/// `500` `{"message":"Internal server error"}`, the answer a gateway gives for a call its
/// api's own configuration cannot be served from.
fn internal_server_error() -> Refusal {
    Refusal {
        status: super::super::data_plane::UNPLACEABLE_STATUS,
        message: INTERNAL_MESSAGE.to_string(),
    }
}

/// The sentence a `500` carries, as API Gateway words it.
const INTERNAL_MESSAGE: &str = "Internal server error";

/// Carry one established connection: register it, dispatch every frame the client sends,
/// write every payload posted to it, and forget it when it closes.
async fn carry_frames<S>(
    state: ::engine::ServiceState<ServiceStore>,
    registry: ::engine::ServiceState<connections::ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState>,
    api_id: String,
    accepted: Admitted,
    socket: ::tokio_tungstenite::WebSocketStream<S>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (pushes, mut pushed): (Sender<Vec<u8>>, Receiver<Vec<u8>>) =
        channel(connections::CONNECTION_PUSH_DEPTH);
    let connection_id = state.lock().websocket_transport.mint_connection_id();
    // The connection EXISTS from here: it is registered in the registry the `@connections`
    // management API serves, with the channel that makes a `PostToConnection` to it a real
    // delivery over this socket. The registry holds the only sender, so a `DeleteConnection`
    // that removes the record also ends this loop and closes the socket.
    let registered = connections::open_connection(
        &registry,
        connection_id.clone(),
        connections::Identity {
            source_ip: LOOPBACK_SOURCE_IP.to_string(),
            user_agent: accepted.user_agent.clone().unwrap_or_default(),
        },
        SystemTime::now(),
        Some(pushes),
    );
    if !registered {
        tracing::warn!(%api_id, %connection_id, "a minted ConnectionId was already registered");
        return;
    }
    tracing::info!(
        %api_id,
        stage = %accepted.stage_name,
        %connection_id,
        "a WebSocket connection is open"
    );
    let (mut outgoing, mut incoming) = socket.split();
    loop {
        ::tokio::select! {
            push = pushed.recv() => {
                let Some(frame) = push else { break };
                if outgoing.send(frame_of(frame)).await.is_err() {
                    break;
                }
            }
            frame = incoming.next() => {
                let Some(Ok(frame)) = frame else { break };
                let body = match frame {
                    Message::Text(text) => text.as_str().to_string(),
                    // A payload that is not text selects no route key of any api, so it is
                    // served by `$default` if the api has one.
                    Message::Binary(bytes) => {
                        String::from_utf8(bytes.to_vec()).unwrap_or_default()
                    }
                    Message::Close(_) => break,
                    // Ping, Pong and Frame are the protocol's own: the implementation answers
                    // a ping itself and nothing of the api serves them.
                    _ => continue,
                };
                if let Some(reply) = served_frame(&state, &api_id, &accepted.stage_name, &connection_id, &body) {
                    if outgoing.send(Message::Text(reply.into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
    // The connection has ended: its record goes, so its id addresses a connection that no
    // longer exists — which is `GoneException` from each of the three `@connections`
    // operations. The id stays ISSUED, so it is never refused as one that never existed.
    connections::close_connection(&registry, &connection_id);
    tracing::info!(%api_id, %connection_id, "a WebSocket connection is closed");
}

/// The address a connection on a core-owned loopback listener comes from. The core does not
/// hand the handler the peer address, and every connection to a loopback listener comes from
/// loopback, so that is what a connection is attributed to.
const LOOPBACK_SOURCE_IP: &str = "127.0.0.1";

/// A pushed payload as a frame: text when the bytes are text, which is what a client reading
/// a JSON payload expects, and binary otherwise.
fn frame_of(frame: Vec<u8>) -> Message {
    match String::from_utf8(frame) {
        Ok(text) => Message::Text(text.into()),
        Err(not_text) => Message::Binary(not_text.into_bytes().into()),
    }
}

/// What one frame is answered with, or `None` when the api sends nothing back for it.
///
/// Three answers, all computed from the configuration the stage deployed
/// ([`dispatch::message`]):
///
/// * a message the api routes nowhere — no route's key is selected and there is no `$default`
///   route — is answered with the gateway's `Forbidden` frame;
/// * a message whose route's integration produced no reply is answered with the gateway's
///   `Internal server error` frame;
/// * a message served by a TWO-WAY route (one that declares a `RouteResponse`) is answered
///   with what the selected integration response rendered. A route that declares none is
///   one-way: its integration still runs, and nothing is sent back.
fn served_frame(
    state: &::engine::ServiceState<ServiceStore>,
    api_id: &str,
    stage_name: &str,
    connection_id: &str,
    body: &str,
) -> Option<String> {
    let store = state.lock();
    let api = store.apis.get(api_id)?;
    let stage = api.stages.get(stage_name)?;
    let deployment = api.deployments.get(stage.deployment_id.as_deref()?)?;
    match dispatch::message(api, &deployment.configuration, connection_id, body) {
        Ok(None) => {
            tracing::info!(%api_id, %connection_id, "a WebSocket message selected no route");
            Some(gateway_frame(
                super::super::data_plane::FORBIDDEN_MESSAGE,
                connection_id,
            ))
        }
        Ok(Some(served)) => {
            tracing::debug!(
                %api_id,
                %connection_id,
                route_key = %served.route_key,
                two_way = served.two_way,
                "a WebSocket message was served"
            );
            served
                .two_way
                .then_some(served.reply.payload)
                .flatten()
        }
        Err(unserved) => {
            tracing::info!(
                %api_id,
                %connection_id,
                ?unserved,
                "a WebSocket message's route produced no reply"
            );
            Some(gateway_frame(INTERNAL_MESSAGE, connection_id))
        }
    }
}

/// A frame the GATEWAY sends rather than a route: its reason, the connection it is about and
/// the id of the call, which is the shape API Gateway's own `Forbidden` and
/// `Internal server error` frames have.
fn gateway_frame(message: &str, connection_id: &str) -> String {
    let frame: BTreeMap<&str, String> = [
        (FRAME_MESSAGE_MEMBER, message.to_string()),
        (FRAME_CONNECTION_ID_MEMBER, connection_id.to_string()),
        (
            FRAME_REQUEST_ID_MEMBER,
            crate::service::invoke::proxy_event::mint_request_id(),
        ),
    ]
    .into_iter()
    .collect();
    ::serde_json::to_string(&frame).unwrap_or_else(|_| {
        ::serde_json::json!({ FRAME_MESSAGE_MEMBER: message }).to_string()
    })
}
