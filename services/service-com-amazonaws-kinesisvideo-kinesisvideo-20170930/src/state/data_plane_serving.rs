// SPDX-License-Identifier: CC0-1.0

//! What ANSWERS at an announced data endpoint: this emulator's own front door, on the
//! plane's own port, over the transport the client that just made the control call can
//! already dial.
//!
//! # Why the plane is served in the clear
//!
//! A `DataEndpoint` is an address a STOCK client dials with the configuration it already
//! has — "To read data from the stream or to write data to it, specify this endpoint in
//! your application" — and nothing hands it extra trust material in between. A client
//! that verifies TLS therefore accepts a plane's certificate only if it already trusts
//! the authority that issued it, and the only certificate such a client has been given is
//! the emulator's own front-door one: a plane presenting any other is refused at the
//! handshake, before one byte of the request is sent, so the whole discovered data plane
//! is undialable however well-formed the announced value looks.
//!
//! The core's own certificate is the core's, so a plane does not get to present it. What
//! the core does offer is a listener that hands the handler the connection's first byte
//! ([`::emulator_server::Listeners::tcp_plaintext`], the same socket, bind interface,
//! port range, connection caps, idle timeout and teardown as its TLS sibling), and on it
//! this module speaks HTTP/1.1 — the protocol the announced origin's scheme names. So the
//! announced origin is one the calling client dials as it stands, which is the whole
//! requirement on it; the planes stay distinct from each other and from the control-plane
//! address by PORT, exactly as before.
//!
//! The media a producer streams to such a port never leaves this machine: the interface
//! is the core's (`EMULATOR_BIND_ADDRESS`, loopback unless the operator says otherwise),
//! and the request is dispatched inside this process.
//!
//! # What is served there
//!
//! The transport is this module's; the ANSWER is not. Every request this listener accepts
//! is handed to the router serving the call that opened the listener
//! ([`super::serve_on_data_plane`]), so a plane answers whatever this emulator answers,
//! with the same panic containment. Before that, the transport-safety decisions the core
//! makes for its own HTTP listeners are applied here from the core's own predicates — the
//! `Host` allowlist that guards against DNS rebinding, the WebSocket refusal, and the
//! browser (`Origin` / `Sec-Fetch-Site`) check — and the request is read under one
//! deadline and one body ceiling, both the core's, so a slow or oversized producer cannot
//! hold a connection permit open.

use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use ::bytes::Bytes;
use ::emulator_server::{listener_caps, BrowserVerdict, ListenerConn, RemoteOrigins};
use ::http_body_util::{BodyExt as _, Full, Limited};
use ::smithy_server_runtime::EmbeddedEmulator;
use ::tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Serves one plane's announced address: this emulator's own front door, on a port of its
/// own.
pub struct DataPlaneFrontDoor {
    /// The emulator the control call that opened this listener was served by — the same
    /// router, the same engine, the same state. It is an IN-PROCESS handle: a request
    /// this listener accepts is dispatched inside this process and nothing here dials
    /// out, so the only address involved is the one the core bound.
    emulator: EmbeddedEmulator,
    /// How many requests this plane reads at once, which is what bounds the memory the
    /// bodies in flight on it hold: the core's own HTTP in-flight cap, one permit per
    /// request, taken before a body is read and released when the answer is written. A
    /// request that finds none is shed rather than buffered.
    in_flight: Arc<Semaphore>,
}

impl DataPlaneFrontDoor {
    /// The front door of one plane, over the emulator that announced it.
    pub fn new(emulator: EmbeddedEmulator) -> Self {
        Self {
            emulator,
            in_flight: Arc::new(Semaphore::new(listener_caps::MAX_HTTP_INFLIGHT)),
        }
    }
}

impl ::emulator_server::NativeHandler for DataPlaneFrontDoor {
    /// One call per accepted connection. The handles are cloned into the connection's own
    /// future, which is what the core's `'static` handler contract asks for.
    fn serve(&self, conn: ListenerConn) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let emulator = self.emulator.clone();
        let in_flight = Arc::clone(&self.in_flight);
        Box::pin(serve_connection(emulator, in_flight, conn))
    }
}

/// Speak HTTP/1.1 on one accepted connection until the client closes it, answering each
/// request from this emulator's router.
///
/// HTTP/1.1 only, as the emulator's own front door offers: every request here is fully
/// buffered, so nothing relies on HTTP/2 streaming, and a client that expects an interim
/// `100 Continue` for a large body gets one. The header-read deadline is the core's, so a
/// client that opens a connection and dribbles a request line cannot hold the
/// connection permit the core issued for it.
async fn serve_connection(
    emulator: EmbeddedEmulator,
    in_flight: Arc<Semaphore>,
    conn: ListenerConn,
) {
    let io = ::hyper_util::rt::TokioIo::new(conn);
    let service = ::hyper::service::service_fn(move |request| {
        let emulator = emulator.clone();
        let in_flight = Arc::clone(&in_flight);
        async move {
            Ok::<_, ::std::convert::Infallible>(answer(emulator, in_flight, request).await)
        }
    });
    if let Err(error) = ::hyper::server::conn::http1::Builder::new()
        .timer(::hyper_util::rt::TokioTimer::new())
        .header_read_timeout(request_deadline())
        .serve_connection(io, service)
        .await
    {
        // A connection the client dropped, or a request this emulator could not frame,
        // ends here and takes nothing else with it: the next connection is served
        // normally.
        ::tracing::debug!(
            %error,
            "a connection to an announced data endpoint ended before its response"
        );
    }
}

/// How long one request on an announced plane may take, from its first header byte to the
/// answer: the core's own HTTP request deadline, so a plane bounds a request exactly as
/// the core's HTTP listeners do.
fn request_deadline() -> Duration {
    Duration::from_secs(listener_caps::HTTP_REQUEST_DEADLINE_SECS)
}

/// Answer one request to an announced plane.
async fn answer(
    emulator: EmbeddedEmulator,
    in_flight: Arc<Semaphore>,
    request: ::http::Request<::hyper::body::Incoming>,
) -> ::http::Response<Full<Bytes>> {
    let (parts, body) = request.into_parts();
    if let Some(refused) = refused_before_dispatch(&parts) {
        return refused;
    }
    // One permit per request IN FLIGHT, taken before a byte of the body is read: what the
    // bodies being read hold at once is then the in-flight cap times the body ceiling
    // below, rather than the connection cap times it. A request that finds no permit is
    // shed, which is what the core's own HTTP listener answers such a request.
    let Ok(permit) = Arc::clone(&in_flight).try_acquire_owned() else {
        return plain(503, SERVER_BUSY);
    };
    // The body ceiling and the deadline are the core's: a request is read only up to what
    // one connection may carry, and reading it and answering it together may not outlast
    // one request deadline.
    let limited = Limited::new(body, body_ceiling());
    let served = ::tokio::time::timeout(request_deadline(), async move {
        // Held for the whole read and dispatch, and released with it.
        let _permit: OwnedSemaphorePermit = permit;
        let Ok(collected) = limited.collect().await else {
            return plain(413, BODY_TOO_LARGE);
        };
        let forwarded = ::http::Request::from_parts(parts, collected.to_bytes());
        let answered = super::serve_on_data_plane(emulator.http_connector(), forwarded).await;
        let (parts, body) = answered.into_parts();
        ::http::Response::from_parts(parts, Full::new(Bytes::from(body)))
    })
    .await;
    served.unwrap_or_else(|_| plain(408, REQUEST_TIMED_OUT))
}

/// How many bytes of one request this plane reads: the core's per-connection ceiling.
fn body_ceiling() -> usize {
    usize::try_from(listener_caps::MAX_BYTES_PER_CONN).unwrap_or(usize::MAX)
}

/// The transport-safety refusals the core applies to its own HTTP listeners, applied here
/// from the core's own predicates so a plane and a core listener decide them alike:
/// the `Host` allowlist (the DNS-rebinding guard a browser needs), the WebSocket refusal,
/// and the browser check. `None` lets the request through to the router.
fn refused_before_dispatch(
    parts: &::http::request::Parts,
) -> Option<::http::Response<Full<Bytes>>> {
    let header = |name: &str| parts.headers.get(name).and_then(|value| value.to_str().ok());

    // HTTP/1.1 requires a `Host`, and without an authority the rebinding guard cannot be
    // run at all, so a request carrying none is refused rather than served: this fails
    // CLOSED, exactly as the core's own HTTP listener does.
    let Some(host) = header("host").or_else(|| parts.uri.host()) else {
        return Some(plain(421, HOST_REQUIRED));
    };
    if !host_allowed(::service_common::data_plane_addressing::host_only(host)) {
        return Some(plain(421, HOST_NOT_ALLOWED));
    }
    if ::emulator_server::is_websocket_upgrade(
        header("connection"),
        header("upgrade"),
        header(":protocol"),
    ) {
        return Some(plain(501, NO_WEBSOCKETS));
    }
    if let BrowserVerdict::Forbid(reason) = ::emulator_server::classify_browser_request(
        header("origin"),
        header("sec-fetch-site"),
        // The plane's address is this emulator's, reached from the machine it runs on:
        // the same default a core HTTP listener carries, which admits no remote origin.
        &RemoteOrigins::SameOriginOnly,
        false,
    ) {
        return Some(plain(403, reason));
    }
    None
}

/// Whether a request's `Host` names the interface this plane was bound on, by the rule a
/// core HTTP listener applies: a loopback `Host` on the default loopback bind (which is
/// what guards a browser against DNS rebinding), the operator's own address when they
/// bound a routable one, and any `Host` on a wildcard bind — there the operator exposed
/// the port deliberately and no single address names it.
fn host_allowed(host: &str) -> bool {
    if ::emulator_server::is_loopback_host(host) {
        return true;
    }
    let Ok(bound) = ::emulator_server::bind_address_from_env() else {
        return false;
    };
    bound.is_unspecified()
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|named| named == bound)
}

/// What a plane answers a request whose `Host` is not the address it serves.
const HOST_NOT_ALLOWED: &str = "This endpoint does not serve that host.\n";

/// What it answers a request that names no host at all, which no HTTP/1.1 request may.
const HOST_REQUIRED: &str = "This endpoint requires the host it is addressed by.\n";

/// What it answers a WebSocket upgrade, which no endpoint of this emulator serves.
const NO_WEBSOCKETS: &str = "This endpoint does not serve WebSocket connections.\n";

/// What it answers a request carrying more than one connection may.
const BODY_TOO_LARGE: &str = "This request is larger than this endpoint accepts.\n";

/// What it answers a request that did not finish within one request deadline.
const REQUEST_TIMED_OUT: &str = "This request took longer than this endpoint allows.\n";

/// What it answers a request that arrived while it was already reading as many as it reads
/// at once.
const SERVER_BUSY: &str = "This endpoint is serving as many requests as it can. Try again.\n";

/// One answer of this module's own, for a request that never reached the router. Plain
/// text rather than a service envelope: which protocol an answer belongs in is the
/// claiming service's to decide, and no service was asked about this one.
fn plain(status: u16, message: &str) -> ::http::Response<Full<Bytes>> {
    let body = Bytes::from(message.to_string());
    ::http::Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .header("content-length", body.len().to_string())
        .body(Full::new(body.clone()))
        // Every argument is this function's own, so the builder cannot fail; answer a
        // bare status rather than panicking if it ever does.
        .unwrap_or_else(|_| {
            let mut response = ::http::Response::new(Full::new(body));
            *response.status_mut() = ::http::StatusCode::INTERNAL_SERVER_ERROR;
            response
        })
}
