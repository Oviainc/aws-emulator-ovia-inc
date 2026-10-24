// SPDX-License-Identifier: CC0-1.0

//! The `execute-api` WEBSOCKET data plane: how an RFC 6455 OPENING HANDSHAKE addressed to
//! a stage of a WebSocket api is recognised, and what it is answered with.
//!
//! A WebSocket api is reached at the URI its `ApiEndpoint` reports —
//! `wss://{api-id}.execute-api.{region}.amazonaws.com`, with "the stage name … appended to
//! this URI to form a complete path to a deployed API stage" (`Api$ApiEndpoint`) — so the
//! first request a client makes against a deployed WebSocket stage is not a modelled
//! operation at all: it is the opening handshake of RFC 6455 (a `GET` carrying
//! `Connection: Upgrade`, `Upgrade: websocket` and a `Sec-WebSocket-Key`). No service
//! claimed that request, so the emulator's front door could resolve no target service for
//! it and answered `404` `UnknownService` — an "this API does not exist" shape for an
//! endpoint this very service told the caller to use, and one no WebSocket client can act
//! on: it cannot tell a missing data plane from a stage that was never deployed.
//!
//! This module is therefore the second half of [`crate::service::EmulatedService`]: the
//! claim ([`claims_parts`], read by `Service::can_handle`) and the serving ([`serve`],
//! reached from `Service::override_routing_error` — a handshake names no modelled
//! operation, and it carries no credential whose verification could change the answer).
//! It is a pure READ of this service's own records: a handshake changes nothing here, and
//! every status it answers is computed from the api's and the stage's stored state.
//!
//! # What it resolves, in order
//!
//! 1. the request is an RFC 6455 opening handshake ([`Handshake::is_opening`]) and does not
//!    address the `@connections` space, which belongs to ApiGatewayManagementApi;
//! 2. it addresses an api THIS service holds whose `ProtocolType` is `WEBSOCKET`
//!    ([`resolve`]) — by the api id in an `execute-api` host, or, for a handshake whose
//!    host names no api (the emulator's own endpoint, or a custom domain fronting the
//!    api), by the stage segment of its path;
//! 3. that api holds a stage the path names, and that stage has a deployment released to
//!    it; a step that resolves nothing is `403` `{"message":"Forbidden"}`, which is what
//!    API Gateway answers for an api id it does not serve and for a stage that has no
//!    deployment (the `execute-api` status taxonomy the REST data plane already states,
//!    `service-com-amazonaws-apigateway-backplanecontrolservice/src/service/invoke.rs`).
//!
//! # What it does not do, and what it answers instead
//!
//! A DEPLOYED WebSocket stage is answered `501` `NotImplemented` with the sentence
//! [`NO_WEBSOCKET_TRANSPORT_MESSAGE`], and NOT with `101 Switching Protocols`.
//!
//! The emulator's front door serves one BUFFERED request per call — `emulator_server`'s
//! production listener hands each request to the router and writes the one response it
//! answers with, and the only seams a service is given (`Service::override_operation`,
//! `Service::override_routing_error`) take an already-buffered request and return a
//! buffered response. Nothing in that path can hand the connection's byte stream to a
//! service, so there is no socket on which to carry WebSocket frames after a handshake,
//! and nothing that could run `$connect`, the route-selection-expression route or
//! `$disconnect` over one. A `101` would therefore promise a session nothing can carry:
//! the client would see the upgrade succeed and then read EOF on its first frame, and a
//! connection id minted for it would be addressable at `@connections` while no socket
//! existed. That is a fabricated success, so it is refused instead — the same answer, for
//! the same reason, that the Transcribe Streaming WebSocket transport gives
//! (`service-com-amazonaws-transcribestreaming-transcribe/src/ops/types/event_stream.rs`,
//! which refuses the upgrade with its operation's own modelled error). `501` is this
//! emulator's "not implemented" channel (`crate::scaffolded`), and it is not a retryable
//! 5xx: a client learns the capability is absent rather than retrying for ever.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::http;

use crate::state::{Api, ComAmazonawsApigatewayv2Apigatewayv2State, Stage};

/// The second label of every `execute-api` endpoint host: `{api-id}.execute-api.…`.
///
/// From `Api$ApiEndpoint`: "The URI of the API, of the form
/// `{api-id}.execute-api.{region}.amazonaws.com`".
pub const EXECUTE_API_LABEL: &str = "execute-api";

/// The path segment that belongs to ApiGatewayManagementApi rather than to a stage.
///
/// `@connections` is bound on the SAME host a WebSocket api is reached at — that service
/// binds `DELETE|GET|POST /@connections/{ConnectionId}` and its callers address it through
/// the api's invoke URL — so a request naming it is not an opening handshake against a
/// stage and is left to the service that models it. Claiming it here would make two
/// services claim one request, which the router answers `500 AmbiguousService`.
const CONNECTIONS_SEGMENT: &str = "@connections";

/// The `Connection` field token that asks for a protocol upgrade (RFC 6455 §4.1).
const UPGRADE_TOKEN: &str = "upgrade";

/// The `Upgrade` field value that names the WebSocket protocol (RFC 6455 §4.1).
const WEBSOCKET_PROTOCOL: &str = "websocket";

/// The header carrying the client's nonce, "a base64-encoded value that, when decoded, is
/// 16 bytes in length" (RFC 6455 §4.1). Its presence is what distinguishes an opening
/// handshake from any other upgrade request.
const SEC_WEBSOCKET_KEY: &str = "sec-websocket-key";

/// The media type the data plane answers in. The data plane is plain HTTPS rather than a
/// modelled operation, so it answers in the envelope API Gateway answers in — a JSON body
/// — rather than in this service's restJson1 error envelope.
const JSON_CONTENT_TYPE: &str = "application/json";

/// The member a data-plane body carries its reason under: API Gateway's own `execute-api`
/// error bodies are `{"message": …}`.
const MESSAGE_MEMBER: &str = "message";

/// The response header a restJson1 client resolves an error shape from, set so a caller
/// can branch on the refusal without parsing the body. The value is the one
/// [`crate::scaffolded`] renders for an unimplemented capability.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The error code the capability refusal carries; see [`ERROR_TYPE_HEADER`].
const NOT_IMPLEMENTED_CODE: &str = "NotImplemented";

/// The sentence API Gateway's `execute-api` endpoint answers for an api id it does not
/// serve and for a stage with no deployment: `403` `{"message":"Forbidden"}`.
const FORBIDDEN_MESSAGE: &str = "Forbidden";

/// The status that sentence is answered with.
const FORBIDDEN_STATUS: u16 = 403;

/// The reason a deployed WebSocket stage cannot complete the handshake. Stated to a
/// developer in one sentence, because the distinction they need first is "this emulator
/// has no WebSocket transport" rather than "the stage is misconfigured"; see the module
/// docs for why no `101` is answered.
pub const NO_WEBSOCKET_TRANSPORT_MESSAGE: &str =
    "NO_WEBSOCKET_TRANSPORT: this emulator's front door serves one buffered request per \
     call and cannot switch a connection to the WebSocket protocol, so the RFC 6455 \
     opening handshake against this deployed WEBSOCKET stage is refused instead of \
     answered 101 Switching Protocols; no connection id is minted and no $connect, route \
     or $disconnect integration runs";

/// The status the capability refusal carries: [`crate::scaffolded`]'s 501, which an SDK
/// retry classifier does not treat as retryable.
const NOT_IMPLEMENTED_STATUS: u16 = 501;

/// The status a request this service cannot place at all is answered with — a fault of the
/// emulator, never of the caller.
const UNPLACEABLE_STATUS: u16 = 500;

/// The sentence that status carries.
const INTERNAL_MESSAGE: &str = "Internal server error";

/// The parts of a request this data plane reads: the method, the host and path it is
/// addressed at, and the three handshake fields of RFC 6455 §4.1.
///
/// One reader for BOTH request shapes the seams are handed — `http::request::Parts` in
/// `Service::can_handle` and the runtime-API request everywhere else — so the claim and the
/// serve cannot disagree about what a request addresses.
#[derive(Debug, Clone, Copy)]
pub struct Handshake<'r> {
    method: &'r str,
    host: &'r str,
    path: &'r str,
    connection: Option<&'r str>,
    upgrade: Option<&'r str>,
    key: Option<&'r str>,
}

impl<'r> Handshake<'r> {
    /// The reading of the request parts `Service::can_handle` is handed.
    pub fn of_parts(parts: &'r http::request::Parts) -> Self {
        Self {
            method: parts.method.as_str(),
            host: header_of(&parts.headers, "host")
                .or_else(|| parts.uri.authority().map(http::uri::Authority::as_str))
                .unwrap_or_default(),
            path: parts.uri.path(),
            connection: header_of(&parts.headers, "connection"),
            upgrade: header_of(&parts.headers, "upgrade"),
            key: header_of(&parts.headers, SEC_WEBSOCKET_KEY),
        }
    }

    /// The reading of the runtime-API request, which carries its URI as a string.
    pub fn of_request(req: &'r RtRequest) -> Self {
        Self {
            method: req.method(),
            host: req
                .headers()
                .get("host")
                .or_else(|| authority_of(req.uri()))
                .unwrap_or_default(),
            path: path_of(req.uri()),
            connection: req.headers().get("connection"),
            upgrade: req.headers().get("upgrade"),
            key: req.headers().get(SEC_WEBSOCKET_KEY),
        }
    }

    /// Whether this is an RFC 6455 opening handshake: a `GET` whose `Connection` field
    /// includes the `Upgrade` token, whose `Upgrade` field names `websocket`, and which
    /// carries a non-empty `Sec-WebSocket-Key`.
    ///
    /// Each field is a comma-separated token list compared case-insensitively, as RFC 6455
    /// §4.1 requires ("a |Connection| header field that includes the token "Upgrade"",
    /// "an |Upgrade| header field containing the value "websocket", treated as an
    /// ASCII case-insensitive value"), so `keep-alive, Upgrade` and `WebSocket` are the
    /// same handshake a client library sends.
    pub fn is_opening(&self) -> bool {
        self.method.eq_ignore_ascii_case("GET")
            && self
                .connection
                .is_some_and(|field| has_token(field, UPGRADE_TOKEN))
            && self
                .upgrade
                .is_some_and(|field| has_token(field, WEBSOCKET_PROTOCOL))
            && self.key.is_some_and(|key| !key.trim().is_empty())
    }

    /// Whether the path addresses the `@connections` space of ApiGatewayManagementApi, in
    /// either of the two forms that service's callers use: with the stage prefix its
    /// endpoint carries (`/{stage}/@connections/{id}`) and without it.
    fn addresses_connections(&self) -> bool {
        path_segments(self.path)
            .into_iter()
            .take(2)
            .any(|segment| segment == CONNECTIONS_SEGMENT)
    }
}

/// What a handshake addresses, as this service's records resolve it: the api, the stage
/// name the path carries, and that api's stage of that name when it holds one.
#[derive(Debug)]
struct Resolved<'s> {
    api: &'s Api,
    stage_name: String,
    stage: Option<&'s Stage>,
}

/// The WEBSOCKET api and stage `handshake` addresses, or `None` when it addresses none
/// this service holds.
///
/// Two addressings resolve, because a client reaches this emulator at a different host
/// than it reaches AWS at:
///
/// * the host's first label names the api, which is the addressing `Api$ApiEndpoint`
///   documents. A host that IS an `execute-api` endpoint but whose api id this service
///   does not hold resolves to nothing at all — that host belongs to another api (an API
///   Gateway REST api is reached on the same host shape and its own service claims it), so
///   reading its path as one of THIS service's stages would make two services claim one
///   request;
/// * a host that names no api at all — the emulator's own endpoint, which a WebSocket
///   client points at because `{api-id}.execute-api.{region}.amazonaws.com` resolves
///   nowhere on the machine it runs on, or a custom domain fronting the api — is resolved
///   by the stage segment of its path, the only part of such a handshake that names a
///   deployed stage. This is the same host-independent claim ApiGatewayManagementApi makes
///   over its own `@connections` path space. When two WebSocket apis hold a stage of that
///   name the request names no ONE api, so nothing is resolved rather than one of them
///   guessed at.
///
/// An api whose `ProtocolType` is not `WEBSOCKET` is never resolved: an upgrade request to
/// an HTTP api's stage is an ordinary HTTP request to that api's routes, which is that
/// api's data plane and not this handshake.
fn resolve<'s>(
    store: &'s ComAmazonawsApigatewayv2Apigatewayv2State,
    handshake: &Handshake<'_>,
) -> Option<Resolved<'s>> {
    if handshake.addresses_connections() {
        return None;
    }
    let stage_name = first_segment(handshake.path);
    match api_id_of_host(handshake.host) {
        Some(api_id) => {
            let api = store.apis.get(api_id).filter(|api| is_websocket(api))?;
            // The host already names the api, so a path that names no stage of it is
            // still this api's to answer: an endpoint that serves nothing (`403`), not a
            // request this service cannot place.
            let stage_name = stage_name.unwrap_or_default();
            Some(Resolved {
                api,
                stage_name: stage_name.to_string(),
                stage: api.stages.get(stage_name),
            })
        }
        None => {
            let stage_name = stage_name?;
            let mut carrying = store
                .apis
                .values()
                .filter(|api| is_websocket(api) && api.stages.contains_key(stage_name));
            let api = carrying.next()?;
            if carrying.next().is_some() {
                return None;
            }
            Some(Resolved {
                api,
                stage_name: stage_name.to_string(),
                stage: api.stages.get(stage_name),
            })
        }
    }
}

/// Whether this api speaks the WebSocket protocol: `ProtocolType` is an enum of exactly
/// `HTTP | WEBSOCKET` (`smithy_query(query="shape", shape="ProtocolType")`).
fn is_websocket(api: &Api) -> bool {
    api.protocol_type == crate::state::WEBSOCKET_PROTOCOL_TYPE
}

/// Whether THIS service serves the request these parts came from as a WebSocket opening
/// handshake — the data-plane half of `Service::can_handle`.
///
/// Narrowed to a handshake against an api this service HOLDS, read from the one shared
/// store, so nothing is claimed that this service cannot answer about, and so the api ids
/// it minted are the only ones it claims a host for.
pub fn claims_parts<Eng: Send + Sync + 'static>(engine: &Eng, parts: &http::request::Parts) -> bool {
    claims(engine, &Handshake::of_parts(parts))
}

/// [`claims_parts`] for the runtime-API request the routing-error seam is handed.
pub fn claims_request<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> bool {
    claims(engine, &Handshake::of_request(req))
}

/// Whether this data plane serves `handshake`.
fn claims<Eng: Send + Sync + 'static>(engine: &Eng, handshake: &Handshake<'_>) -> bool {
    if !handshake.is_opening() {
        return false;
    }
    crate::state::service_state(engine)
        .is_some_and(|state| resolve(&state.lock(), handshake).is_some())
}

/// Serve one RFC 6455 opening handshake addressed to a WebSocket api's stage, from this
/// service's own records.
///
/// Free-standing and reached with the ONE shared engine, the way the REST data plane's
/// `serve` is, so the claim above and this answer read the same store.
pub fn serve<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> RtResponse {
    let handshake = Handshake::of_request(req);
    let Some(state) = crate::state::service_state(engine) else {
        return json_response(UNPLACEABLE_STATUS, INTERNAL_MESSAGE, None);
    };
    let store = state.lock();
    // Unreachable through the claim, which resolves the same api from the same store; the
    // api can still have been deleted between the claim and now, which is an api this
    // endpoint no longer serves.
    let Some(resolved) = resolve(&store, &handshake) else {
        return forbidden();
    };
    // A stage the api does not hold, and a stage with no deployment released to it, are
    // both an endpoint that serves nothing: `403` `{"message":"Forbidden"}`.
    let Some(stage) = resolved.stage else {
        tracing::debug!(
            api_id = %resolved.api.api_id,
            stage = %resolved.stage_name,
            "the WebSocket handshake names no stage of this api"
        );
        return forbidden();
    };
    let released = stage
        .deployment_id
        .as_deref()
        .is_some_and(|deployment_id| resolved.api.deployments.contains_key(deployment_id));
    if !released {
        tracing::debug!(
            api_id = %resolved.api.api_id,
            stage = %stage.stage_name,
            "the WebSocket handshake names a stage with no deployment released to it"
        );
        return forbidden();
    }
    // A deployed stage: the handshake is the capability this emulator does not have. See
    // the module docs for why this is not a `101`.
    tracing::info!(
        api_id = %resolved.api.api_id,
        stage = %stage.stage_name,
        "refusing a WebSocket opening handshake: this emulator has no WebSocket transport"
    );
    json_response(
        NOT_IMPLEMENTED_STATUS,
        NO_WEBSOCKET_TRANSPORT_MESSAGE,
        Some(NOT_IMPLEMENTED_CODE),
    )
}

/// `403` `{"message":"Forbidden"}` — the answer an `execute-api` endpoint gives for an api
/// or a stage it does not serve.
fn forbidden() -> RtResponse {
    json_response(FORBIDDEN_STATUS, FORBIDDEN_MESSAGE, None)
}

/// One data-plane response: a status, a JSON `{"message": …}` body, and, for a refusal a
/// client branches on, the error code in [`ERROR_TYPE_HEADER`].
fn json_response(status: u16, message: &str, error_code: Option<&str>) -> RtResponse {
    let body = ::serde_json::json!({ MESSAGE_MEMBER: message })
        .to_string()
        .into_bytes();
    let length = body.len();
    let mut response = RtResponse::new(
        StatusCode::try_from(status).unwrap_or(
            StatusCode::try_from(UNPLACEABLE_STATUS).expect("500 is a valid status code"),
        ),
        SdkBody::from(body),
    );
    let headers = response.headers_mut();
    let _ = headers.try_insert("content-type", JSON_CONTENT_TYPE);
    let _ = headers.try_insert("content-length", length.to_string());
    if let Some(error_code) = error_code {
        let _ = headers.try_insert(ERROR_TYPE_HEADER, error_code.to_string());
    }
    response
}

/// Whether a comma-separated HTTP field value carries `token`, compared
/// case-insensitively.
fn has_token(field: &str, token: &str) -> bool {
    field
        .split(',')
        .any(|candidate| candidate.trim().eq_ignore_ascii_case(token))
}

/// One header's value as text, or `None` when it is absent or not text.
fn header_of<'h>(headers: &'h http::HeaderMap, name: &str) -> Option<&'h str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// The first label of `host` when `host` is an `execute-api` endpoint: the api id.
///
/// The shape is `{api-id}.execute-api.{region}.amazonaws.com` — a first label, then the
/// [`EXECUTE_API_LABEL`] label, then at least a region and a domain. Any `:port` and
/// userinfo is removed first, as the front door's own host reader does. The region and the
/// domain are not matched against a list: an emulator serves whatever region a caller
/// addresses, and a FIPS or a dualstack spelling of the same endpoint is the same endpoint.
fn api_id_of_host(host: &str) -> Option<&str> {
    let without_userinfo = host.rsplit('@').next().unwrap_or(host);
    let without_port = without_userinfo
        .split(':')
        .next()
        .unwrap_or(without_userinfo);
    let mut labels = without_port.split('.');
    let api_id = labels.next()?;
    let second = labels.next()?;
    // The label is `execute-api` itself, or one of its endpoint VARIANTS, which AWS spells
    // with a suffix after a hyphen (`execute-api-fips`). A label that merely STARTS with
    // those characters (`execute-apifoo`) is a different host and is not claimed.
    let execute_api = second == EXECUTE_API_LABEL
        || second
            .strip_prefix(EXECUTE_API_LABEL)
            .is_some_and(|variant| variant.starts_with('-') && variant.len() > 1);
    if api_id.is_empty() || !execute_api {
        return None;
    }
    // A region label and a domain: `{api-id}.execute-api.{region}.amazonaws.com` has four
    // labels after the id, and the shortest form this accepts still has two, so a bare
    // `execute-api.something` (no api id in front) is not read as one.
    (labels.count() >= 2).then_some(api_id)
}

/// The first non-empty path segment: the stage name of an invoke path.
fn first_segment(path: &str) -> Option<&str> {
    path_segments(path).into_iter().next()
}

/// The non-empty `/`-separated segments of a path.
fn path_segments(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .collect()
}

/// The authority of an absolute request URI, or `None` when the URI is origin-form.
fn authority_of(uri: &str) -> Option<&str> {
    let (_scheme, rest) = uri.split_once("://")?;
    Some(match rest.find('/') {
        Some(path_start) => &rest[..path_start],
        None => rest,
    })
}

/// The path component of a request URI, with any scheme, authority and query removed.
fn path_of(uri: &str) -> &str {
    // The query is dropped FIRST: a query VALUE may carry `://`, which a scheme scan over
    // the whole URI would mistake for the authority.
    let without_query = uri.split('?').next().unwrap_or(uri);
    match without_query.split_once("://") {
        Some((_scheme, authority_and_path)) => match authority_and_path.find('/') {
            Some(path_start) => &authority_and_path[path_start..],
            None => "/",
        },
        None => without_query,
    }
}

#[cfg(test)]
mod tests {
    //! The pure rules this module owns that a wire test cannot vary cheaply: which host is
    //! an `execute-api` endpoint, and which request is an RFC 6455 opening handshake. The
    //! served behaviour is pinned end to end in
    //! `tests/all/websocket_handshake_data_plane.rs`.

    use super::*;

    /// A handshake addressed at `host` with `path`, carrying the three fields of RFC 6455
    /// §4.1 as `connection` / `upgrade` / `key` spell them.
    fn handshake<'r>(
        method: &'r str,
        host: &'r str,
        path: &'r str,
        connection: Option<&'r str>,
        upgrade: Option<&'r str>,
        key: Option<&'r str>,
    ) -> Handshake<'r> {
        Handshake {
            method,
            host,
            path,
            connection,
            upgrade,
            key,
        }
    }

    #[test]
    fn execute_api_hosts_are_read_and_others_are_not() {
        let cases: &[(&str, Option<&str>)] = &[
            ("abc1230000.execute-api.us-east-1.amazonaws.com", Some("abc1230000")),
            ("0123456789.execute-api.eu-west-3.amazonaws.com", Some("0123456789")),
            ("a.execute-api.ap-southeast-2.amazonaws.com:443", Some("a")),
            // An endpoint variant: the same endpoint under a FIPS spelling.
            (
                "abc1230000.execute-api-fips.us-gov-west-1.amazonaws.com",
                Some("abc1230000"),
            ),
            ("abc1230000.execute-api.cn-north-1.amazonaws.com.cn", Some("abc1230000")),
            // Not an `execute-api` endpoint at all.
            ("abc1230000.execute-apifoo.us-east-1.amazonaws.com", None),
            ("apigateway.us-east-1.amazonaws.com", None),
            ("execute-api.us-east-1.amazonaws.com", None),
            ("127.0.0.1:9297", None),
            ("localhost", None),
            ("sockets.example.com", None),
            ("abc1230000.execute-api", None),
        ];
        for (host, expected) in cases {
            assert_eq!(
                api_id_of_host(host),
                *expected,
                "case {host:?}: expected the api id {expected:?}"
            );
        }
    }

    #[test]
    fn only_an_rfc_6455_opening_handshake_is_one() {
        let key = Some("dGhlIHNhbXBsZSBub25jZQ==");
        let cases: &[(&str, bool, Handshake<'_>)] = &[
            (
                "the three fields a client library sends",
                true,
                handshake("GET", "host", "/prod", Some("Upgrade"), Some("websocket"), key),
            ),
            (
                "a Connection field carrying more than one token",
                true,
                handshake(
                    "GET",
                    "host",
                    "/prod",
                    Some("keep-alive, Upgrade"),
                    Some("WebSocket"),
                    key,
                ),
            ),
            (
                "the fields in lower case, as HTTP/1.1 allows",
                true,
                handshake("get", "host", "/prod", Some("upgrade"), Some("websocket"), key),
            ),
            (
                "no upgrade asked for: an ordinary GET on the stage path",
                false,
                handshake("GET", "host", "/prod", Some("keep-alive"), None, None),
            ),
            (
                "an upgrade to another protocol",
                false,
                handshake("GET", "host", "/prod", Some("Upgrade"), Some("h2c"), None),
            ),
            (
                "the handshake fields without a key",
                false,
                handshake("GET", "host", "/prod", Some("Upgrade"), Some("websocket"), None),
            ),
            (
                "an empty key",
                false,
                handshake("GET", "host", "/prod", Some("Upgrade"), Some("websocket"), Some("  ")),
            ),
            (
                "a POST carrying the handshake fields",
                false,
                handshake("POST", "host", "/prod", Some("Upgrade"), Some("websocket"), key),
            ),
        ];
        for (case, expected, request) in cases {
            assert_eq!(
                request.is_opening(),
                *expected,
                "case {case:?}: expected is_opening() to be {expected}, got {:?}",
                request
            );
        }
    }

    #[test]
    fn the_connections_path_space_is_not_a_stage_handshake() {
        let key = Some("dGhlIHNhbXBsZSBub25jZQ==");
        for path in [
            "/@connections/abc",
            "/prod/@connections/abc",
            "/@connections",
        ] {
            let request = handshake("GET", "host", path, Some("Upgrade"), Some("websocket"), key);
            assert!(
                request.addresses_connections(),
                "case {path:?}: the @connections space belongs to ApiGatewayManagementApi"
            );
        }
        for path in ["/prod", "/prod/connections/abc", "/@connectionsx/abc"] {
            let request = handshake("GET", "host", path, Some("Upgrade"), Some("websocket"), key);
            assert!(
                !request.addresses_connections(),
                "case {path:?}: expected a stage path, not the @connections space"
            );
        }
    }

    #[test]
    fn a_uri_is_split_into_its_authority_and_path() {
        assert_eq!(authority_of("https://host:9297/prod"), Some("host:9297"));
        assert_eq!(authority_of("/prod"), None);
        assert_eq!(path_of("https://host:9297/prod"), "/prod");
        assert_eq!(path_of("https://host:9297/prod?a=b://c"), "/prod");
        assert_eq!(path_of("https://host:9297"), "/");
        assert_eq!(path_of("/prod/sub"), "/prod/sub");
        assert_eq!(first_segment("/prod/sub"), Some("prod"));
        assert_eq!(first_segment("/"), None);
    }
}
