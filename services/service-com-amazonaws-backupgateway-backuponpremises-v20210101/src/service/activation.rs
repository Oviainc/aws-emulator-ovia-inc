// SPDX-License-Identifier: CC0-1.0

//! The ACTIVATION channel: where an activation key comes from.
//!
//! `CreateGateway` takes an `ActivationKey` and no operation of this model issues one: a
//! key is minted when the backup gateway appliance a customer deployed makes its
//! activation handshake, which AWS documents as an HTTP request the appliance (or the
//! operator's browser) makes OUTSIDE the SDK — `activationRegion` names the region the key
//! is for, and the answer carries the key itself. Nothing about it is a modelled operation:
//! it carries no AWS credential and names no `X-Amz-Target`.
//!
//! This module is that handshake, served by this emulator because the emulator stands in
//! for the appliance as much as for the service (as `crate::state::synced_inventory` stands
//! in for a hypervisor's inventory). It is reached from two seams of
//! [`crate::service::EmulatedService`]:
//!
//! * [`Service::can_handle`](smithy_server_runtime::Service::can_handle) — the request
//!   carries no credential scope and no target header, so without [`claims_parts`] the
//!   front door could resolve no service for it and would answer `404 UnknownService`;
//! * [`Service::override_routing_error`](smithy_server_runtime::Service::override_routing_error)
//!   — it names no modelled operation, and that seam is reached BEFORE authentication,
//!   which is what lets an appliance that holds no AWS credential be answered at all.
//!
//! What it writes is the activation registry of this service's own store
//! ([`crate::state::ComAmazonawsBackupgatewayBackuponpremisesV20210101State::issue_activation_key`]):
//! one pending activation per handshake, which the create that activates the appliance
//! redeems. So the key a client presents to `CreateGateway` is one this service really
//! issued, and a key nothing issued is refused — which is the whole point of the
//! handshake.
//!
//! The key is answered as the response BODY for every form of the request, including the
//! one written without `no_redirect`, which AWS answers with a redirect carrying the key in
//! its location: that redirect form is deliberately NOT reproduced, because what AWS
//! redirects a browser to is a console this emulator does not host, so the redirect would
//! resolve nowhere.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::http;

/// The query parameter naming the region the key is issued for: the `activationRegion` of
/// the documented activation request.
const ACTIVATION_REGION_PARAM: &str = "activationRegion";

/// The query parameter naming the type of gateway being activated.
///
/// Carried by the documented activation request, and read here only to refuse a handshake
/// for a gateway type this service does not serve: the model declares exactly one
/// (`GatewayType`, one of [`crate::state::BACKUP_VM`]:
/// `smithy_query(query="shape",shape="GatewayType")`), so a handshake that names another
/// one is activating something this service has no gateway type for.
///
/// A handshake that OMITS it is served: with one gateway type in the model, an unnamed type
/// can only be that one, and the documented request is also written without it (the
/// `?activationRegion=…&no_redirect` form).
const GATEWAY_TYPE_PARAM: &str = "gatewayType";

/// The one path the handshake is served at: the root of the endpoint, as the documented
/// activation request addresses it.
const ACTIVATION_PATH: &str = "/";

/// The media type an issued key is served as: the bare key, which is what the documented
/// handshake answers with — not this service's JSON envelope, which belongs to its modelled
/// operations.
const ACTIVATION_CONTENT_TYPE: &str = "text/plain; charset=utf-8";

/// What a handshake this service cannot answer at all carries: its store is not reachable
/// on this engine, so no activation could be recorded and none is reported.
const UNPLACEABLE_MESSAGE: &str = "The activation could not be issued.";

/// The longest `activationRegion` a handshake may name.
///
/// The value is caller-supplied and is KEPT on the activation record, so its length is
/// bounded here rather than stored as sent: an unsigned request must not be able to grow
/// this service's memory by the size of its own query string. An EMULATOR bound, well past
/// the longest region name AWS publishes.
const MAX_ACTIVATION_REGION_LENGTH: usize = 64;

/// What a handshake naming a region this service cannot issue a key for carries, stating
/// what a region looks like so the caller can fix the request from the answer alone.
///
/// The value asked for is NOT echoed back: nothing is gained by reflecting a
/// caller-supplied value into a response body.
fn invalid_region_message() -> String {
    format!(
        "The activation region must name a region: at most \
         {MAX_ACTIVATION_REGION_LENGTH} characters of letters, digits and hyphens."
    )
}

/// What a handshake for another gateway type carries: the one type this service activates,
/// so the caller can fix the request from the answer alone.
///
/// The type the request asked for is NOT echoed back: nothing is gained by reflecting a
/// caller-supplied value into a response body, and the one supported value is what the
/// caller needs to know.
fn unsupported_gateway_type_message() -> String {
    format!(
        "The requested gateway type cannot be activated: the only gateway type is {}.",
        crate::state::BACKUP_VM
    )
}

/// The handshake a request makes, or `None` for a request that is not one.
///
/// A handshake is a `GET` of the endpoint root carrying a non-empty
/// [`ACTIVATION_REGION_PARAM`]: the method and the parameter together are what separate it
/// from every modelled request of this service, which are `POST`s carrying an
/// `X-Amz-Target` and no such parameter.
#[derive(Debug)]
struct Handshake {
    /// The region the key is asked for, as `activationRegion` named it.
    region: String,
    /// The gateway type the handshake named, when it named one.
    gateway_type: Option<String>,
}

impl Handshake {
    /// The handshake these request parts make — the reading
    /// [`Service::can_handle`](smithy_server_runtime::Service::can_handle) is handed.
    fn of_parts(parts: &http::request::Parts) -> Option<Self> {
        Self::of(
            parts.method.as_str(),
            parts.uri.path(),
            parts.uri.query().unwrap_or_default(),
        )
    }

    /// The handshake this runtime-API request makes, which carries its URI as a string.
    fn of_request(req: &RtRequest) -> Option<Self> {
        let (path, query) = split_uri(req.uri());
        Self::of(req.method(), &path, &query)
    }

    /// Both readings resolve through this one rule, so the request this service CLAIMS and
    /// the request it then serves cannot disagree about what a handshake is.
    ///
    /// A handshake carrying the parameter with an EMPTY value is still a handshake: it is
    /// claimed here and refused by [`Self::names_a_region`], so every value this service
    /// cannot issue a key for is answered the same way, rather than one of them falling
    /// through to the front door's `404 UnknownService`.
    fn of(method: &str, path: &str, query: &str) -> Option<Self> {
        if !method.eq_ignore_ascii_case(http::Method::GET.as_str()) || path != ACTIVATION_PATH {
            return None;
        }
        Some(Self {
            region: query_param(query, ACTIVATION_REGION_PARAM)?,
            gateway_type: query_param(query, GATEWAY_TYPE_PARAM),
        })
    }

    /// Whether the region this handshake asks for is one a key can be issued for: not
    /// empty, short enough to keep on the record, and spelled the way a region is — which
    /// is what `crate::state::arn_region` would otherwise silently substitute a fallback
    /// for, so a key is never issued under a region the caller did not ask for.
    fn names_a_region(&self) -> bool {
        !self.region.is_empty()
            && self.region.len() <= MAX_ACTIVATION_REGION_LENGTH
            && crate::state::arn_region(&self.region) == self.region
    }

    /// The type of gateway this handshake activates, or `None` when it names one this
    /// service has no gateway type for.
    fn serves_gateway_type(&self) -> bool {
        self.gateway_type
            .as_deref()
            .is_none_or(|requested| requested == crate::state::BACKUP_VM)
    }
}

/// The path and query of a URI carried as a string.
///
/// Parsed with `::http::Uri` — the same parser
/// [`Service::can_handle`](smithy_server_runtime::Service::can_handle) reads `parts.uri`
/// with — so neither half of the claim restates a URI grammar of its own.
fn split_uri(uri: &str) -> (String, String) {
    if let Ok(parsed) = uri.parse::<::http::Uri>() {
        return (
            parsed.path().to_string(),
            parsed.query().unwrap_or_default().to_string(),
        );
    }
    // A form the URI parser will not accept is still read the way it is written: the `?`
    // separates the two parts.
    match uri.split_once('?') {
        Some((path, query)) => (path.to_string(), query.to_string()),
        None => (uri.to_string(), String::new()),
    }
}

/// The value `name` carries in a URL query string, decoded, or `None` when the query does
/// not carry it.
///
/// The grammar is NOT restated here: the pairs are split and decoded by the framework's own
/// query decoder (`aws_smithy_schema_server::query::parse_form_urlencoded`), the same one a
/// `@httpQuery` binding is read with. First match wins, as a repeated key's first binding
/// does.
fn query_param(query: &str, name: &str) -> Option<String> {
    ::aws_smithy_schema_server::query::parse_form_urlencoded(query.as_bytes())
        .into_iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

/// Whether the request these parts came from is an activation handshake — the activation
/// half of `Service::can_handle`.
pub fn claims_parts(parts: &http::request::Parts) -> bool {
    Handshake::of_parts(parts).is_some()
}

/// [`claims_parts`] for the runtime-API request the serving seam is handed.
pub fn claims_request(req: &RtRequest) -> bool {
    Handshake::of_request(req).is_some()
}

/// Serve one activation handshake: issue a key for the region it names, record it pending
/// in this service's own store, and answer with the key.
///
/// Free-standing and reached with the ONE shared engine, the way a handler reaches it, so
/// the key this answers with is the key `CreateGateway` will find in the registry.
pub fn serve<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> RtResponse {
    let Some(handshake) = Handshake::of_request(req) else {
        // Unreachable: the seams above serve this only for a request `claims_request`
        // accepted, which is the same reading.
        return text_response(404, "Not Found".to_string());
    };
    if !handshake.serves_gateway_type() {
        tracing::info!(
            operation = "ActivationHandshake",
            outcome = "refused",
            reason = "gateway type",
            "refused an activation handshake"
        );
        return text_response(400, unsupported_gateway_type_message());
    }
    if !handshake.names_a_region() {
        tracing::info!(
            operation = "ActivationHandshake",
            outcome = "refused",
            reason = "activation region",
            "refused an activation handshake"
        );
        return text_response(400, invalid_region_message());
    }
    let Some(state) = crate::state::service_state(engine) else {
        return text_response(500, UNPLACEABLE_MESSAGE.to_string());
    };
    let key = state
        .lock()
        .issue_activation_key(&handshake.region, ::std::time::SystemTime::now());
    // The record of a state-changing call, which this unsigned seam serves without the
    // request walk every modelled operation goes through. The key itself is NEVER recorded:
    // it is a credential, and whoever holds it can activate a gateway.
    tracing::info!(
        operation = "ActivationHandshake",
        region = %handshake.region,
        outcome = "issued",
        "issued an activation key"
    );
    text_response(200, key)
}

/// One handshake answer: a document with its length, so a client can read it without the
/// connection closing.
fn text_response(status: u16, body: String) -> RtResponse {
    let bytes = body.into_bytes();
    let length = bytes.len();
    let mut response = RtResponse::new(
        StatusCode::try_from(status).expect("the statuses above are valid status codes"),
        SdkBody::from(bytes),
    );
    let headers = response.headers_mut();
    let _ = headers.try_insert("content-type", ACTIVATION_CONTENT_TYPE);
    let _ = headers.try_insert("content-length", length.to_string());
    response
}
