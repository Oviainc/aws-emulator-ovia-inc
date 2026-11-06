// SPDX-License-Identifier: CC0-1.0

//! A stream's DATA-plane endpoints: what `GetDataEndpoint` resolves an `APIName` to.
//!
//! `GetDataEndpoint` exists because a stream's media APIs are NOT served at the
//! control-plane endpoint the call itself is made to — "Gets an endpoint for a
//! specified stream for either reading or writing … The returned endpoint does not
//! have the API name appended" — and because there is more than one such endpoint per
//! stream. Kinesis Video Media names the two: `InvalidEndpointException` is "Status
//! Code: 400, Caller used wrong endpoint to write data to a stream. On receiving such
//! an exception, the user must call `GetDataEndpoint` with `AccessMode` set to "READ"
//! and use the endpoint Kinesis Video returns in the next `GetMedia` call." Which
//! endpoint is REPORTED therefore follows from the access mode the requested API needs.
//!
//! So the value reported is a bare `https://<authority>` base URL — scheme and host
//! only, no path, because the client appends the API name itself — of the plane the
//! requested `APIName` needs. Every API of one access mode resolves to one endpoint,
//! the two modes resolve to different ones, and neither is the control-plane endpoint
//! the call arrived on.
//!
//! What this module decides is which endpoint each API is TOLD to use. SCOPING a
//! plane's address to that plane's own operations — the `InvalidEndpointException` a
//! wrong-plane write earns, and the refusal a control-plane call to a data address
//! earns — is a separate rule nothing here implements: every address of this emulator
//! is served by its one front door, which resolves the target service from the SigV4
//! signing name and the modelled path and not from the host. Each plane's address is
//! reachable and serves this emulator's whole surface.
//!
//! # The announced authority is one this emulator ANSWERS on
//!
//! A `DataEndpoint` is not a label: it is an address the client then DIALS ("To read
//! data from the stream or to write data to it, specify this endpoint in your
//! application"), with no host rewriting in between — a producer SDK, `kvssink` or a
//! client configured with `endpoint_url=DataEndpoint` resolves exactly the name handed
//! back. A public AWS hostname therefore resolves off this machine and the discovered
//! data plane cannot be reached at all, so each plane is published as an address this
//! emulator serves: one of the core's own listeners
//! ([`::emulator_server::Listeners::http`], bound on the emulator's interface —
//! loopback by default — with the core owning the socket and its TLS), opened once per
//! plane and reported as `https://host:port`.
//!
//! The two planes differ by PORT, which is what keeps them distinct from each other and
//! from the control-plane address while all three stay reachable; the particular port is
//! the operating system's and is per-environment, as a published data host's labels are
//! on AWS.
//!
//! What answers there is this emulator's OWN front door: the listener hands every
//! request it accepts to the router that is serving the call which announced the address
//! ([`serve_on_data_plane`] over [`::smithy_server_runtime::EmbeddedEmulator`], with the
//! same panic containment the control-plane front door applies), so the media and
//! archived-media routes this emulator registers — `/getMedia`, `/listFragments`,
//! `/getMediaForFragmentList`, `/getClip`, the HLS/DASH session URLs — are bound on the
//! announced authority, exactly as they are on the endpoint the control call arrived on.
//! Nothing is synthesised here: a request forwarded to the router is answered by
//! whichever service claims it, or by the router's own refusal when none does.
//!
//! Two consequences of holding that handle are worth stating. The plane is served by the
//! router that answered the call which opened the listener, so an engine mounted behind
//! two routers serves its data plane from the first of them. And the handle is the
//! handler's own callback, so a request dialled in from outside is accounted against the
//! callback chain of that one control call rather than starting a fresh chain — immaterial
//! at the core's chain limit, which is two orders of magnitude above any chain this
//! service makes.

use std::future::Future;
use std::pin::Pin;

use ::aws_smithy_runtime_api::client::http::{HttpConnector, SharedHttpConnector};
use ::aws_smithy_runtime_api::http::Request as RtRequest;
use ::smithy_server_runtime::EmbeddedEmulator;

use super::ComAmazonawsKinesisvideoKinesisvideo20170930State as ServiceState;

/// The access mode a stream's media API needs, and so the data endpoint that serves it.
///
/// The two modes Kinesis Video Media names ("the user must call `GetDataEndpoint` with
/// `AccessMode` set to \"READ\"", said of a write addressed to the wrong endpoint):
/// media is WRITTEN to a stream through one endpoint and READ back through another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DataPlane {
    /// The plane media is written to.
    Write,
    /// The plane a stream's media is read from.
    Read,
}

/// Which plane serves one `APIName`, or `None` for a value the model does not publish.
///
/// The catalog below is the `APIName` enum
/// (`smithy_query(query="shape", shape="APIName")`, mirrored in
/// [`crate::ops::types::A_P_I_NAME_VALUES`]), each value paired with the access mode the
/// API it names needs: `PUT_MEDIA` writes media onto the stream (Kinesis Video Media
/// `PutMedia`), and every other published value reads the stream — its media
/// (`GetMedia`) or its archive (`ListFragments`, `GetMediaForFragmentList`, `GetClip`,
/// `GetHLSStreamingSessionURL`, `GetDASHStreamingSessionURL`, `GetImages`, all of
/// Kinesis Video Archived Media).
///
/// `None` is what refuses an `APIName` outside that published set, so a value this
/// service cannot place on a plane is answered as a bad argument rather than handed an
/// endpoint chosen for it.
pub fn data_plane_of(api_name: &str) -> Option<DataPlane> {
    const PLANES: &[(&str, DataPlane)] = &[
        ("PUT_MEDIA", DataPlane::Write),
        ("GET_MEDIA", DataPlane::Read),
        ("GET_MEDIA_FOR_FRAGMENT_LIST", DataPlane::Read),
        ("LIST_FRAGMENTS", DataPlane::Read),
        ("GET_CLIP", DataPlane::Read),
        ("GET_HLS_STREAMING_SESSION_URL", DataPlane::Read),
        ("GET_DASH_STREAMING_SESSION_URL", DataPlane::Read),
        ("GET_IMAGES", DataPlane::Read),
    ];
    PLANES
        .iter()
        .find(|(name, _)| *name == api_name)
        .map(|(_, plane)| *plane)
}

/// This service's regional host: its endpoint prefix, the request's region and the AWS
/// DNS suffix — the name AWS publishes this service at, used where a request carries no
/// address of its own to report back ([`super::request_authority`]).
pub fn service_host(region: &str) -> String {
    format!("kinesisvideo.{region}.amazonaws.com")
}

/// One plane's data endpoint: the address it is published at, and the listener that
/// answers there.
///
/// The state OWNS the listener: dropping it closes the port, so it must outlive every
/// endpoint handed out, which is the whole life of the store.
pub struct DataPlaneEndpoint {
    /// `https://host:port`, the whole value `GetDataEndpoint` reports for this plane.
    base_url: String,
    /// Held only to keep the port open; the core owns the socket and its TLS.
    _listener: ::emulator_server::ResourceListener,
}

/// Written by hand rather than derived: [`DataPlaneEndpoint`] holds a core
/// `ResourceListener`, which is not `Debug`. The listener is reported by the address it
/// announced, which is what a reader needs.
impl std::fmt::Debug for DataPlaneEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataPlaneEndpoint")
            .field("base_url", &self.base_url)
            .finish()
    }
}

/// The base URL of `plane` for this store, opening that plane's listener on first use.
///
/// Scheme and host only: "The returned endpoint does not have the API name appended.
/// The client needs to add the API name to the returned endpoint", so a path here would
/// be appended to rather than replaced.
///
/// Opening a listener changes no stream and is idempotent: the first listener of a plane
/// is kept, so the plane resolves to the same address for every stream and for as long
/// as the store lives — a client can key a session on it. `None` when the address could
/// not be opened at all, which leaves the caller to answer with a modelled error rather
/// than to announce an endpoint nothing serves.
pub async fn ensure_data_endpoint<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<ServiceState>,
    plane: DataPlane,
    emulator: &EmbeddedEmulator,
) -> Option<String> {
    // The guard is taken and dropped before the await below: a lock held across an
    // await would make this future non-`Send`.
    let existing = {
        let store = state.lock();
        store
            .data_endpoints
            .get(&plane)
            .map(|endpoint| endpoint.base_url.clone())
    };
    if let Some(base_url) = existing {
        return Some(base_url);
    }

    let listener = super::shared_engine(engine)?
        .listeners()
        .http(
            DataPlaneFrontDoor {
                emulator: emulator.clone(),
            },
            ::emulator_server::HttpPolicy::default(),
        )
        .await
        .ok()?;
    let base_url = format!(
        "https://{}",
        ::service_common::data_plane_addressing::listener_authority(&listener)
    );

    let mut store = state.lock();
    // Another request may have opened one while this one was binding; keep the first,
    // so an announced endpoint never changes under a client. Dropping the loser closes
    // its port.
    if let Some(endpoint) = store.data_endpoints.get(&plane) {
        return Some(endpoint.base_url.clone());
    }
    store.data_endpoints.insert(
        plane,
        DataPlaneEndpoint {
            base_url: base_url.clone(),
            _listener: listener,
        },
    );
    Some(base_url)
}

/// Serves one plane's announced address: this emulator's own front door, on a port of
/// its own. The core runs the browser checks and owns CORS; this only hands the request
/// on to the router.
struct DataPlaneFrontDoor {
    /// The emulator the control call that opened this listener was served by — the same
    /// router, the same engine, the same state. It is an IN-PROCESS handle: a request
    /// this listener accepts is dispatched inside this process and nothing here dials
    /// out, so the only address involved is the one the core bound.
    emulator: EmbeddedEmulator,
}

impl ::emulator_server::HttpHandler for DataPlaneFrontDoor {
    fn handle(
        &self,
        req: ::http::Request<Vec<u8>>,
    ) -> Pin<Box<dyn Future<Output = ::http::Response<Vec<u8>>> + Send>> {
        Box::pin(serve_on_data_plane(self.emulator.http_connector(), req))
    }
}

/// What a request to an announced data endpoint is answered with: whatever this
/// emulator's router answers it, as it arrived.
///
/// A panic anywhere behind this address — in the dispatch machinery or in the handler of
/// whichever service claims the request — is CONTAINED here
/// ([`::emulator_server::contain_panics`]) and answered as the protocol-shaped internal
/// failure, exactly as the endpoint the control call arrived on does for the same
/// request: one caller cannot take the listener, the connection or the process down with
/// it, and the next request is served normally.
pub async fn serve_on_data_plane(
    transport: SharedHttpConnector,
    req: ::http::Request<Vec<u8>>,
) -> ::http::Response<Vec<u8>> {
    let facts = ::emulator_server::RequestFacts::from_request(&req);
    ::emulator_server::contain_panics(facts, forward_to_router(transport, req)).await
}

/// Hand one request to this emulator's own router and answer what it answers.
///
/// Method, path, headers and body all travel unchanged — the `Host` header included, so
/// the signature the client computed over them still verifies, and so the service the
/// router resolves from the SigV4 signing name and the modelled path is the one the API
/// belongs to.
async fn forward_to_router(
    transport: SharedHttpConnector,
    req: ::http::Request<Vec<u8>>,
) -> ::http::Response<Vec<u8>> {
    let (parts, body) = req.into_parts();
    let forwarded = ::http::Request::from_parts(
        parts,
        ::aws_smithy_types::body::SdkBody::from(::bytes::Bytes::from(body)),
    );
    let Ok(forwarded) = RtRequest::try_from(forwarded) else {
        return data_plane_response(400, MALFORMED_DATA_PLANE_REQUEST);
    };
    let answered = match transport.call(forwarded).await {
        Ok(answered) => answered,
        Err(error) => {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a request to an announced data endpoint reached no service of this emulator"
            );
            return data_plane_response(500, DATA_PLANE_NOT_ANSWERED);
        }
    };
    let status = answered.status().as_u16();
    let headers: Vec<(String, String)> = answered
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    let body = answered
        .into_body()
        .bytes()
        .map(<[u8]>::to_vec)
        .unwrap_or_default();
    let mut builder = ::http::Response::builder().status(status);
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    builder
        .body(body)
        .unwrap_or_else(|_| data_plane_response(500, DATA_PLANE_NOT_ANSWERED))
}

/// What an announced data endpoint answers a request that is not an HTTP request the
/// router can be handed at all. One sentence carrying nothing from the request.
const MALFORMED_DATA_PLANE_REQUEST: &str = "This request is not one this endpoint can serve.\n";

/// What it answers when the emulator's own front door did not answer the forwarded
/// request — an in-process fault rather than anything the caller sent.
const DATA_PLANE_NOT_ANSWERED: &str = "This endpoint could not be served. Try again later.\n";

/// One answer of this module's own, for a request that never reached the router. Plain
/// text rather than a service envelope: which protocol an answer belongs in is the
/// claiming service's to decide, and no service claimed this one.
fn data_plane_response(status: u16, message: &'static str) -> ::http::Response<Vec<u8>> {
    ::http::Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .header("content-length", message.len().to_string())
        .body(message.as_bytes().to_vec())
        // Every argument is this function's own constant, so the builder cannot fail;
        // answer a bare 500 rather than panicking if it ever does.
        .unwrap_or_else(|_| {
            let mut response = ::http::Response::new(Vec::new());
            *response.status_mut() = ::http::StatusCode::INTERNAL_SERVER_ERROR;
            response
        })
}
