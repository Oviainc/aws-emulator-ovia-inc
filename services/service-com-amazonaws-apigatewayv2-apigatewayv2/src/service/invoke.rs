// SPDX-License-Identifier: CC0-1.0

//! The `execute-api` HTTP data plane: how a request addressed to a deployed HTTP api
//! stage's invoke URL is recognised, authorized and answered.
//!
//! An HTTP api is reached at the URI its `ApiEndpoint` reports — "`{api-id}.execute-api.
//! {region}.amazonaws.com`. The stage name is typically appended to this URI to form a
//! complete path to a deployed API stage" (`Api$ApiEndpoint`) — so a client of a deployed
//! stage makes an ORDINARY HTTP request to a route of the api, which is no modelled
//! operation of this service at all: its host's first label is the api's own id, which
//! names no service, and its path is a route key's rather than a `/v2/…` route's. The
//! emulator's front door could resolve no target service for such a request and answered
//! `404` `UnknownService` — for the endpoint this very service told the caller to use. A
//! client could therefore not observe the thing an HTTP api exists to do: nothing was ever
//! served on the invoke URL, so a JWT authorizer in front of a route never evaluated a
//! token, and an emulator that never answers is indistinguishable from one that authorises
//! everything.
//!
//! This module is therefore the second data-plane half of
//! [`crate::service::EmulatedService`] (beside [`super::websocket`]'s): the claim
//! ([`claims_parts`], read by `Service::can_handle`) and the serving ([`serve`], reached
//! from the two seams the router offers before and after authentication). It is a READ of
//! this service's own records — a request to a stage changes nothing here — and every
//! status it answers is computed from them.
//!
//! A client on the machine this emulator runs on cannot reach that host at all: nothing
//! resolves `{api-id}.execute-api.{region}.amazonaws.com` there, so it points at the
//! emulator's own endpoint and carries in the PATH what it cannot carry in the host — the
//! api id ahead of the stage (plainly, or behind the [`LOCAL_ENDPOINT_PREFIX`] marker a
//! rewritten endpoint keeps it behind), or the stage alone. [`resolve`] reads every one of
//! those addressings, and each is checked against this service's own records, so one
//! request still has one claimant. The sibling REST data plane
//! (`service-com-amazonaws-apigateway-backplanecontrolservice/src/service/invoke.rs`)
//! resolves the same shapes for its own api ids, and the WEBSOCKET half of THIS service
//! ([`super::websocket`]) resolves only WEBSOCKET apis, so each claims only what it can
//! answer about.
//!
//! # What it resolves, in order
//!
//! 1. the request names an `Api` this service holds whose `ProtocolType` is `HTTP` — by the
//!    host's first label, by the first path segment, or, on a host that names no service,
//!    by the stage segment of its path ([`resolve`]) — whose `DisableExecuteApiEndpoint`
//!    must not have turned this endpoint off;
//! 2. the path within that api names a `Stage` of it: its first segment, or the
//!    [`crate::state::DEFAULT_STAGE_NAME`] stage, which is "the stage that serves an api's
//!    root" and so is reached with no stage segment in the path at all — that is the
//!    "typically" of `Api$ApiEndpoint`;
//! 3. that stage has a deployment released to it. A step that resolves nothing is `403`
//!    `{"message":"Forbidden"}`, the answer an `execute-api` endpoint gives for an api it
//!    does not serve and for a stage with no deployment;
//! 4. the method and the remaining path match a `Route` of the api, most specific match
//!    first ([`matching_route`]); nothing matching is `404` `{"message":"Not Found"}`;
//! 5. that route's `AuthorizationType` is applied BEFORE anything the route fronts
//!    ([`authorization_refusal`]) — which is what this module makes observable: a JWT
//!    authorizer's gate ([`super::jwt`]), the identity-source precondition of a `REQUEST`
//!    authorizer, and the credential an `AWS_IAM` route needs;
//! 6. the route's integration decides the response.
//!
//! # What it does not do, and what it answers instead
//!
//! Step 6 is the one step this emulator cannot perform as AWS does. Every integration type
//! an HTTP api admits names a backend outside this process: `AWS_PROXY` invokes a Lambda
//! function, and Lambda's own `Invoke` answers `501` for `RequestResponse` and `Event`
//! because there is no in-process runtime for a deployment package
//! (`services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs`), so no
//! function response exists to map; `HTTP_PROXY` addresses an endpoint over the network,
//! and this emulator makes no outbound connection. `MOCK` — the one type that answers
//! without a backend — "is not permitted for APIs with a protocol type of HTTP"
//! ([`crate::state::WEBSOCKET_ONLY_INTEGRATION_TYPES`]), so it cannot be configured here.
//!
//! Neither is faked: a body, a status or a header invented for a backend that never ran
//! would be indistinguishable from a real one. An `HTTP_PROXY` upstream is a URI the CALLER
//! can reach, so the caller is sent there — `307 Temporary Redirect` to the stored
//! `IntegrationUri`, the redirect that preserves the request's method and body — rather
//! than the gateway opening a connection of its own. An `AWS_PROXY` integration has no such
//! answer, because an invocation is not a URI a client can fetch: it is answered the way
//! API Gateway answers an integration that produced no response, `500`
//! `{"message":"Internal Server Error"}`, with the reason logged. WHICH status each kind of
//! backend FAULT maps to (a `503` on an integration timeout, a `500` on a handler
//! exception, a `500` rather than a `502` on a malformed proxy response) is the open issue
//! `20261023T151412Z-apigatewayv2-lambda-all-four-http-api-error-propagation.md`, and
//! becomes observable once a function can run at all
//! (`20261024T055421Z-lambda-opensearch-s3-lambda-invoke-cannot-execute-a-deployment-package-so.md`).
//!
//! Nor is a `REQUEST` authorizer's own ALLOW/DENY decision computed: that decision is the
//! answer of the Lambda function its `AuthorizerUri` names, and no deployment package runs
//! here, so a policy document invented in this module would be indistinguishable from the
//! function's. What step 5 applies for such an authorizer is the part of the authorization
//! that is the SERVICE's own and is computed from stored records — the identity-source
//! precondition the authorizer is only invoked past, and a route whose authorizer the api
//! does not hold. A JWT authorizer is different in kind, and is why this module exists: its
//! decision is a function of the token and of the api's own `JWTConfiguration`, both of
//! which are here, so it is computed in full ([`super::jwt`]).

use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::http;

use super::data_plane::{
    addresses_connections, api_id_of_host, authority_of, first_segment, forbidden, header_of,
    identity_source_value, json_response, path_of, path_segments, presents_identity_source,
    query_of, query_parameters, EXECUTE_API_LABEL, UNAUTHORIZED_MESSAGE, UNAUTHORIZED_STATUS,
    UNPLACEABLE_STATUS,
};
use super::jwt;
use crate::state::{Api, ComAmazonawsApigatewayv2Apigatewayv2State, Integration, Route, Stage};

/// The `RouteKey` that catches what no other route matches. AWS documents `$default` as
/// "the default route" of an HTTP api, the one reached when no route key matches the
/// request, and `Route$ApiGatewayManaged` names it ("the $default route is managed by API
/// Gateway") — a reserved key of the protocol rather than a value the model enumerates
/// (`RouteKey` targets the unconstrained `SelectionKey`).
const DEFAULT_ROUTE_KEY: &str = "$default";

/// The verb-independent catch-all a route key may name instead of a method: `ANY /{proxy+}`
/// is the key AWS's own quick-create and SAM templates write, and it matches every verb.
const ANY_METHOD: &str = "ANY";

/// The greedy suffix of a path-variable route-key segment: `{proxy+}` "matches the
/// remainder of the path", so such a segment stands for one or more request segments and
/// can only be the last one.
const GREEDY_VARIABLE_SUFFIX: char = '+';

/// The `AuthorizationType` that lets a request through with no authorization at all: "For
/// HTTP APIs, valid values are NONE for open access" (`Route$AuthorizationType`).
const AUTHORIZATION_NONE: &str = "NONE";

/// The `AuthorizationType` whose caller must sign the request: "AWS_IAM for using AWS IAM
/// permissions" (`Route$AuthorizationType`).
const AUTHORIZATION_AWS_IAM: &str = "AWS_IAM";

/// The `AuthorizationType` whose decision a JWT authorizer makes: "JWT for using JSON Web
/// Tokens" (`Route$AuthorizationType`).
const AUTHORIZATION_JWT: &str = "JWT";

/// The identity source a JWT authorizer extracts its token from when it records none.
///
/// `Authorizer$IdentitySource` for a JWT authorizer is "a single entry that specifies
/// where to extract the JSON Web Token (JWT) from inbound requests", and the example it
/// gives is this one; a configuration that names no entry still has to read the token
/// somewhere, and the `Authorization` header is where RFC 6750 puts a bearer token.
const DEFAULT_JWT_IDENTITY_SOURCE: &str = "$request.header.Authorization";

/// The status a request matching no route of the api is answered with, and its sentence —
/// the api serves the host, but nothing of it serves this method and path.
const NOT_FOUND_STATUS: u16 = 404;
const NOT_FOUND_MESSAGE: &str = "Not Found";

/// The first path segment that marks an endpoint carried IN THE PATH rather than in the
/// host: `/_aws/execute-api/{api-id}/{stage}/{route-path}`.
///
/// A client on the machine this emulator runs on cannot resolve
/// `{api-id}.execute-api.{region}.amazonaws.com`, so it reaches the emulator's own address
/// and keeps the endpoint it was given in the path behind this marker. The segment is
/// reserved for the emulator — it is not a path any AWS api serves — so reading it cannot
/// take a request away from a route of an api.
const LOCAL_ENDPOINT_PREFIX: &str = "_aws";

/// The sentence [`UNPLACEABLE_STATUS`] carries on the HTTP data plane: API Gateway's own
/// body for an HTTP api whose integration produced no response. Spelled as an HTTP api
/// spells it, which is not the REST data plane's `Internal server error`.
const INTERNAL_MESSAGE: &str = "Internal Server Error";

/// The status a caller is sent to an upstream with: the redirect that preserves the
/// request's method and body (RFC 7231 §6.4.7), so the client repeats the request the
/// gateway would have forwarded.
const TEMPORARY_REDIRECT_STATUS: u16 = 307;

/// The header that redirect carries the upstream in (RFC 7231 §7.1.2).
const LOCATION_HEADER: &str = "location";

/// The parts of a request this data plane reads: the method, and the host and path it is
/// addressed at.
///
/// One reader for BOTH request shapes the seams are handed — `http::request::Parts` in
/// `Service::can_handle` and the runtime-API request everywhere else — so the claim and
/// the serve cannot disagree about what a request addresses.
#[derive(Debug, Clone, Copy)]
struct Addressed<'r> {
    method: &'r str,
    /// `None` when the request carries no host at all, which names no api and no service.
    host: Option<&'r str>,
    path: &'r str,
}

impl<'r> Addressed<'r> {
    /// The reading of the request parts `Service::can_handle` is handed.
    fn of_parts(parts: &'r http::request::Parts) -> Self {
        Self {
            method: parts.method.as_str(),
            host: header_of(&parts.headers, "host")
                .or_else(|| parts.uri.authority().map(http::uri::Authority::as_str)),
            path: parts.uri.path(),
        }
    }

    /// The reading of the runtime-API request, which carries its URI as a string.
    fn of_request(req: &'r RtRequest) -> Self {
        Self {
            method: req.method(),
            host: req.headers().get("host").or_else(|| authority_of(req.uri())),
            path: path_of(req.uri()),
        }
    }
}

/// What a request addresses, as this service's records resolve it: the api, and the path
/// WITHIN that api — the part that names a stage and a route.
#[derive(Debug)]
struct Resolved<'s, 'r> {
    api: &'s Api,
    invoke_path: &'r str,
}

/// The HTTP api `addressed` addresses, or `None` when this data plane serves it not at all.
///
/// THREE addressings resolve, because a client reaches this emulator at a different address
/// than it reaches AWS at:
///
/// * HOST-STYLE, the addressing `Api$ApiEndpoint` documents: the host's first label names
///   the api and the whole path is the invoke path. A host that IS an `execute-api`
///   endpoint but whose api id this service does not hold resolves to nothing at all —
///   that host belongs to another api (an API Gateway REST api is reached on the same host
///   shape and its own service claims it), so reading its path as one of THIS service's
///   stages would make two services claim one request;
/// * PATH-STYLE: the api id is the FIRST PATH SEGMENT and the invoke path is what follows
///   it. A client on this machine resolves the host above nowhere, so it reaches the
///   emulator's own endpoint and carries in the path what it cannot carry in the host. It
///   is read only on a host that names NO SERVICE (the emulator's own endpoint) or names
///   THIS one, so a first segment that happens to equal an api id cannot take a request
///   away from the service whose endpoint the host names. Its [`LOCAL_ENDPOINT_PREFIX`]
///   spelling, which marks the rewritten host explicitly, is read on ANY host
///   ([`local_endpoint_path`]);
/// * STAGE-ONLY, for a client that carries the invoke path alone: a host that names NO
///   SERVICE at all is resolved by the STAGE segment of the path, the only part of such a
///   request that names a deployed stage — the same host-independent claim the WebSocket
///   half of this service makes over a stage's path. It is the LAST addressing tried,
///   because it is the only one that does not carry an api id: when two apis of this
///   service hold a stage of that name the request names no ONE api, so nothing is resolved
///   rather than one of them guessed at.
///
/// The limit of that last addressing, stated because the uniqueness guard above cannot
/// reach it: a stage name is unique within an api, not across SERVICES. The REST data
/// plane claims the same path space for its own api ids, so one stage name held by a REST
/// api AND by an HTTP api is claimed by both services and the router answers
/// `500 AmbiguousService` for it — which is why every addressing that names an api is
/// tried first, and why a client whose stage name may collide reaches the api through one
/// of them (the api's own host, or the api id in the path). Nothing here can decide that
/// collision alone: a claim narrowed to "no OTHER service holds this stage name" would
/// need another service's records, which this service does not read.
///
/// This service's own CONTROL-plane path space (`crate::ROUTE_PATH_SEGMENTS`, the single
/// segment `v2`) is never read as a stage: a request whose first segment is one of those is
/// a control-plane call on whatever host it arrives at, and answering it from the data
/// plane would take `/v2/apis` away from the operations that serve it. A stage named like
/// one of those segments is still reachable through the addressings above, where the api is
/// named before the stage is.
///
/// An api whose `ProtocolType` is not `HTTP` is never resolved here: a request to a
/// WEBSOCKET api's stage is its own data plane's ([`super::websocket`]).
fn resolve<'s, 'r>(
    store: &'s ComAmazonawsApigatewayv2Apigatewayv2State,
    addressed: &Addressed<'r>,
) -> Option<Resolved<'s, 'r>> {
    // The `@connections` path space belongs to ApiGatewayManagementApi, on either host.
    if addresses_connections(addressed.path) {
        return None;
    }
    // This service's own CONTROL-plane path space is never a data-plane path, on any host:
    // a request whose first segment is one of `crate::ROUTE_PATH_SEGMENTS` is a
    // control-plane call wherever it arrives, and claiming it here would take `/v2/apis`
    // away from the operations that serve it.
    if first_segment(addressed.path).is_some_and(|first| crate::ROUTE_PATH_SEGMENTS.contains(&first))
    {
        return None;
    }
    // LOCAL-ENDPOINT style first, because its marker is unambiguous on any host.
    if let Some((api_id, invoke_path)) = local_endpoint_path(addressed.path) {
        return servable(store.apis.get(api_id)).map(|api| Resolved { api, invoke_path });
    }
    if let Some(api_id) = addressed.host.and_then(api_id_of_host) {
        return servable(store.apis.get(api_id)).map(|api| Resolved {
            api,
            invoke_path: addressed.path,
        });
    }
    let (first, rest) = split_first_segment(addressed.path)?;
    // Which service the host names, by the front door's own rule: `None` is the emulator's
    // own endpoint, and this service's own key is its control-plane host.
    let host_service = addressed.host.and_then(service_named_by_host);
    if host_service
        .as_deref()
        .is_some_and(|service| service != crate::ROUTE_KEY)
    {
        // Another service's endpoint: whatever its path carries is that service's to
        // answer, so neither addressing below is read and one request keeps one claimant.
        return None;
    }
    if let Some(api) = servable(store.apis.get(first)) {
        return Some(Resolved {
            api,
            invoke_path: rest,
        });
    }
    if host_service.is_some() {
        // This service's own control-plane host: a path segment that is no api id of this
        // account is a control-plane call, not a stage.
        return None;
    }
    let mut holding = store
        .apis
        .values()
        .filter(|api| is_http(api) && !api.disable_execute_api_endpoint)
        .filter(|api| api.stages.contains_key(first));
    let api = holding.next()?;
    holding.next().is_none().then_some(Resolved {
        api,
        invoke_path: addressed.path,
    })
}

/// `api` when it is an HTTP api whose default endpoint still serves.
///
/// "Specifies whether clients can invoke your API by using the default execute-api
/// endpoint. By default, clients can invoke your API with the default
/// https://{api_id}.execute-api.{region}.amazonaws.com endpoint. To require that clients
/// use a custom domain name to invoke your API, disable the default endpoint"
/// (`Api$DisableExecuteApiEndpoint`): an api whose default endpoint is disabled serves
/// nothing on it, so this data plane resolves none of its addressings and the request is
/// left unclaimed rather than refused — the host it names is one this api no longer
/// answers on.
fn servable(api: Option<&Api>) -> Option<&Api> {
    api.filter(|api| is_http(api) && !api.disable_execute_api_endpoint)
}

/// The api id and invoke path of a request that names the api's endpoint INSIDE ITS PATH,
/// behind the [`LOCAL_ENDPOINT_PREFIX`] marker, or `None` for a path carrying no marker.
///
/// A client on the machine this emulator runs on resolves
/// `{api-id}.execute-api.{region}.amazonaws.com` nowhere, so it rewrites the endpoint it
/// was given into one it can reach: the emulator's own address, with the endpoint the
/// original URI named carried in the path ahead of the rest of it —
/// `/_aws/execute-api/{api-id}/{stage}/{route-path}`. The marker makes that rewriting
/// explicit, so it is read on ANY host and is never confused with a path of the api's own:
/// no route key of an api can begin with it, because the segment the api would have to
/// own is the emulator's.
///
/// Both spellings of the endpoint behind the marker resolve — the endpoint PREFIX followed
/// by the api id (`/_aws/execute-api/{api-id}/…`), and the endpoint HOST itself
/// (`/_aws/{api-id}.execute-api.{region}.amazonaws.com/…`) — because what a rewriter has
/// to carry across is the host, and those are the two ways to write it.
fn local_endpoint_path(path: &str) -> Option<(&str, &str)> {
    let (marker, rest) = split_first_segment(path)?;
    if marker != LOCAL_ENDPOINT_PREFIX {
        return None;
    }
    let (endpoint, rest) = split_first_segment(rest)?;
    if let Some(api_id) = api_id_of_host(endpoint) {
        return Some((api_id, rest));
    }
    // The endpoint PREFIX spelling: the api id is the segment after it.
    (endpoint == EXECUTE_API_LABEL)
        .then(|| split_first_segment(rest))
        .flatten()
}

/// Whether this api speaks the HTTP protocol: `ProtocolType` is an enum of exactly
/// `HTTP | WEBSOCKET` (`smithy_query(query="shape", shape="ProtocolType")`).
fn is_http(api: &Api) -> bool {
    api.protocol_type == crate::state::HTTP_PROTOCOL_TYPE
}

/// The first non-empty segment of `path` and the path that follows it (with its leading
/// `/`), or `None` for a path with no segment at all.
fn split_first_segment(path: &str) -> Option<(&str, &str)> {
    let trimmed = path.trim_start_matches('/');
    let (first, rest) = match trimmed.find('/') {
        Some(boundary) => (&trimmed[..boundary], &trimmed[boundary..]),
        None => (trimmed, "/"),
    };
    (!first.is_empty()).then_some((first, rest))
}

/// The service `host` names to the emulator's front door, or `None` when it names none.
///
/// The front door's OWN host reader decides it (`emulator_server::service_from_host`, the
/// last tier of the routing key every front door resolves), rather than a second rule
/// written here: the request the two path addressings are read for is exactly the one that
/// reader can place nowhere — a loopback address or `localhost`, which is the only endpoint
/// a client on this machine can reach.
fn service_named_by_host(host: &str) -> Option<String> {
    let mut headers = http::HeaderMap::new();
    let value = http::HeaderValue::from_str(host).ok()?;
    headers.insert(http::header::HOST, value);
    ::emulator_server::service_from_host(&headers)
}

/// Whether THIS data plane serves the request these parts came from — the HTTP half of
/// `Service::can_handle`.
///
/// Narrowed to an api this service HOLDS, read from the one shared store, so nothing is
/// claimed that this service cannot answer about and exactly one service claims a request.
pub fn claims_parts<Eng: Send + Sync + 'static>(
    engine: &Eng,
    parts: &http::request::Parts,
) -> bool {
    claims(engine, &Addressed::of_parts(parts))
}

/// [`claims_parts`] for the runtime-API request the serving seams are handed.
pub fn claims_request<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> bool {
    claims(engine, &Addressed::of_request(req))
}

/// Whether this data plane serves `addressed`.
fn claims<Eng: Send + Sync + 'static>(engine: &Eng, addressed: &Addressed<'_>) -> bool {
    crate::state::service_state(engine)
        .is_some_and(|state| resolve(&state.lock(), addressed).is_some())
}

/// Whether the request presents a SigV4 credential at all — in the `Authorization` header a
/// signed SDK request carries, or in the `X-Amz-Credential` query parameter of a presigned
/// URL.
///
/// Only PRESENCE is read. Whether the signature over that credential is correct is the
/// emulator's own check, never a service's — which is why this is the predicate BOTH the
/// routing seam and this module use: `Service::override_operation` names the data plane as
/// an operation exactly when this is true, so the router verifies the credential before
/// anything is served, and an `AWS_IAM` route is admitted below exactly when the same
/// predicate holds. Two different predicates would let a presigned invoke of an `AWS_IAM`
/// route be served from the pre-authentication seam.
pub fn carries_sigv4_credential(req: &RtRequest) -> bool {
    if req
        .headers()
        .get("authorization")
        .is_some_and(|value| value.contains("Credential="))
    {
        return true;
    }
    query_of(req.uri())
        .unwrap_or_default()
        .split('&')
        .any(|pair| pair.starts_with("X-Amz-Credential="))
}

/// Serve one request addressed to a deployed HTTP api stage, from this service's own
/// records.
///
/// Free-standing and reached with the ONE shared engine, the way the WebSocket data plane's
/// `serve` is, so the claim above and this answer read the same store.
pub fn serve<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> RtResponse {
    let addressed = Addressed::of_request(req);
    let Some(state) = crate::state::service_state(engine) else {
        return json_response(UNPLACEABLE_STATUS, INTERNAL_MESSAGE, None);
    };
    let store = state.lock();
    // Unreachable through the claim, which resolves the same api from the same store; the
    // api can still have been deleted between the claim and now, which is an api this
    // endpoint no longer serves.
    let Some(resolved) = resolve(&store, &addressed) else {
        return forbidden();
    };
    let api = resolved.api;
    // The stage: the path's first segment when it names one, and otherwise the stage that
    // serves the api's root. A path that names neither is an endpoint that serves nothing.
    let Some((stage, route_path)) = stage_of(api, resolved.invoke_path) else {
        tracing::debug!(
            api_id = %api.api_id,
            path = %resolved.invoke_path,
            "the request names no stage of this api"
        );
        return forbidden();
    };
    let released = stage
        .deployment_id
        .as_deref()
        .is_some_and(|deployment_id| api.deployments.contains_key(deployment_id));
    if !released {
        tracing::debug!(
            api_id = %api.api_id,
            stage = %stage.stage_name,
            "the request names a stage with no deployment released to it"
        );
        return forbidden();
    }
    let Some(route) = matching_route(api, addressed.method, &route_path) else {
        tracing::debug!(
            api_id = %api.api_id,
            stage = %stage.stage_name,
            method = %addressed.method,
            path = %route_path,
            "no route of this api matches the request"
        );
        return json_response(NOT_FOUND_STATUS, NOT_FOUND_MESSAGE, None);
    };
    // The route's authorization is applied BEFORE anything the route fronts: a request
    // refused here never reaches the integration below, which is what makes a JWT
    // authorizer's gate observable.
    if let Some(refusal) = authorization_refusal(api, stage, route, req) {
        return refusal;
    }
    serve_integration(api, stage, route)
}

/// The stage of `api` that `invoke_path` names, and the path WITHIN that stage.
///
/// Two spellings reach a stage, and `Api$ApiEndpoint` carries both in one word: "the stage
/// name is TYPICALLY appended to this URI". The first path segment names the stage when the
/// api holds one of that name; otherwise the stage is the one that serves the api's root,
/// [`crate::state::DEFAULT_STAGE_NAME`] — "the stage that serves an api's root" — and the
/// whole path is the route path. A named stage therefore wins over the root stage for a
/// path that begins with its name, which is the only reading under which both remain
/// reachable.
fn stage_of<'s>(api: &'s Api, invoke_path: &str) -> Option<(&'s Stage, String)> {
    if let Some(first) = first_segment(invoke_path) {
        if let Some(stage) = api.stages.get(first) {
            let rest = invoke_path
                .trim_start_matches('/')
                .strip_prefix(first)
                .unwrap_or_default();
            return Some((stage, normalised_path(rest)));
        }
    }
    api.stages
        .get(crate::state::DEFAULT_STAGE_NAME)
        .map(|stage| (stage, normalised_path(invoke_path)))
}

/// `path` with exactly one leading `/` and no trailing one (except for the root itself), so
/// that the same route path is compared however a client spelled it.
fn normalised_path(path: &str) -> String {
    let segments = path_segments(path);
    if segments.is_empty() {
        return "/".to_string();
    }
    format!("/{}", segments.join("/"))
}

/// The route of `api` that serves `method path`, or `None` when none of its routes does.
///
/// The rule is API Gateway's own: a request is served by the MOST SPECIFIC matching route
/// key, and `$default` catches what nothing else matches. Specificity is read off the key
/// itself, segment by segment — a literal segment is more specific than a `{variable}`,
/// which is more specific than the greedy `{proxy+}` that stands for the remainder of the
/// path — and compared from the left, so `GET /orders/{id}` beats `GET /{resource}/{id}`
/// for the same request, and a key naming the request's verb beats the `ANY` catch-all for
/// the same path.
///
/// The work is bounded: one scan of `api.routes`, capped at
/// [`crate::state::MAX_ROUTES_PER_API`] by the create that stored them, and each key's
/// segments are compared once against the request's.
fn matching_route<'s>(api: &'s Api, method: &str, path: &str) -> Option<&'s Route> {
    let requested = path_segments(path);
    let best = api
        .routes
        .values()
        .filter(|route| route.route_key != DEFAULT_ROUTE_KEY)
        .filter_map(|route| specificity(&route.route_key, method, &requested).map(|rank| (rank, route)))
        .max_by(|(left, _), (right, _)| left.cmp(right))
        .map(|(_, route)| route);
    best.or_else(|| {
        api.routes
            .values()
            .find(|route| route.route_key == DEFAULT_ROUTE_KEY)
    })
}

/// How specifically `route_key` matches `method` and the request's path `segments`, or
/// `None` when it does not match at all.
///
/// The rank compares as a tuple: the per-segment scores first (compared from the left, a
/// literal above a variable above the greedy remainder), then whether the key named the
/// request's verb rather than `ANY`. A longer run of literal matches therefore outranks a
/// shorter one, and the greedy key ranks below every key that matched segment for segment.
fn specificity(route_key: &str, method: &str, segments: &[&str]) -> Option<(Vec<u8>, u8)> {
    let (key_method, key_path) = route_key.split_once(' ')?;
    let exact_method = if key_method.eq_ignore_ascii_case(method) {
        1
    } else if key_method.eq_ignore_ascii_case(ANY_METHOD) {
        0
    } else {
        return None;
    };
    let mut scores = Vec::new();
    let pattern = path_segments(key_path);
    let mut requested = segments.iter();
    for (position, expected) in pattern.iter().enumerate() {
        match path_variable(expected) {
            // The greedy remainder: it stands for one or more segments, so it matches only
            // when at least one is left, and it ends the comparison.
            Some(name) if name.ends_with(GREEDY_VARIABLE_SUFFIX) => {
                if position + 1 != pattern.len() || requested.next().is_none() {
                    return None;
                }
                scores.push(0);
                return Some((scores, exact_method));
            }
            // A single-segment variable matches one non-empty segment, whatever it says.
            Some(_) => {
                requested.next()?;
                scores.push(1);
            }
            None => {
                let actual = requested.next()?;
                if actual != expected {
                    return None;
                }
                scores.push(2);
            }
        }
    }
    // Every pattern segment is spoken for; a request carrying MORE segments than the key
    // is not this key's to serve.
    requested.next().is_none().then_some((scores, exact_method))
}

/// The name a route-key path segment declares when it is a path variable (`{id}` names
/// `id`, the greedy `{proxy+}` names `proxy+`), or `None` for a literal segment.
fn path_variable(segment: &str) -> Option<&str> {
    segment.strip_prefix('{')?.strip_suffix('}')
}

/// The refusal the route's `AuthorizationType` owes this request, or `None` when the
/// request may proceed to the integration.
///
/// `NONE` admits every request — "NONE for open access" (`Route$AuthorizationType`).
///
/// `AWS_IAM` needs a SigV4 credential, whose SIGNATURE the emulator itself verifies (a
/// service never checks signatures), so only its presence is read here; a request carrying
/// none is `403` `{"message":"Forbidden"}`. What this does NOT do, stated so a passing test
/// is not read as more than it is: no IAM authorization DECISION is made. AWS evaluates
/// `execute-api:Invoke` for the resolved principal, and this emulator models no caller
/// identity and no policy evaluation, so the refusal an authenticated but unprivileged
/// principal earns is not one it can produce.
///
/// `JWT` is decided in full from the api's own records: the token the authorizer's identity
/// source names is evaluated against that authorizer's `JWTConfiguration` and the route's
/// `AuthorizationScopes` ([`super::jwt::evaluate`]). A token the gate cannot accept is
/// `401` `{"message":"Unauthorized"}` and the integration is NOT reached; a token it
/// accepts whose scopes do not meet the route's is `403` — "otherwise, the invocation is
/// not authorized" (`Route$AuthorizationScopes`).
///
/// `CUSTOM` names a Lambda authorizer, whose decision is that function's answer. The part
/// that is settled BEFORE the function is called is applied: every identity source must be
/// "present in the request, not null, and non-empty", and "only when this is true does the
/// authorizer invoke the authorizer Lambda function. Otherwise, it returns a 401
/// Unauthorized response without calling the Lambda function"
/// (`Authorizer$IdentitySource`). Past that precondition the decision needs the function,
/// which cannot run here, so the request is refused as a configuration this endpoint could
/// not evaluate (`500`) rather than allowed — an allow would be a decision no authorizer
/// made.
///
/// A route whose authorizer-backed type names no authorizer of this api cannot be
/// authorized at all, which is a fault of the api's configuration and not of the caller:
/// `500`. The writers of a route refuse that association
/// ([`crate::state::UNRESOLVED_AUTHORIZER_MESSAGE`]), so this is a store that lost the
/// authorizer some other way and not a route a caller can create.
fn authorization_refusal(
    api: &Api,
    stage: &Stage,
    route: &Route,
    req: &RtRequest,
) -> Option<RtResponse> {
    let authorization_type = route.authorization_type.as_str();
    if authorization_type == AUTHORIZATION_NONE {
        return None;
    }
    if authorization_type == AUTHORIZATION_AWS_IAM {
        if carries_sigv4_credential(req) {
            return None;
        }
        tracing::info!(
            api_id = %api.api_id,
            stage = %stage.stage_name,
            route_key = %route.route_key,
            "refusing an invoke of an AWS_IAM route that presented no credential"
        );
        return Some(forbidden());
    }
    let Some(authorizer) = route
        .authorizer_id
        .as_deref()
        .and_then(|authorizer_id| api.authorizers.get(authorizer_id))
    else {
        tracing::warn!(
            api_id = %api.api_id,
            route_key = %route.route_key,
            authorization_type = %authorization_type,
            authorizer_id = ?route.authorizer_id,
            "the matched route is authorized by an authorizer this api does not hold"
        );
        return Some(json_response(UNPLACEABLE_STATUS, INTERNAL_MESSAGE, None));
    };
    let query_parameters = query_parameters(req.uri());
    let header = |name: &str| req.headers().get(name).map(str::to_string);
    if authorization_type != AUTHORIZATION_JWT {
        // A `CUSTOM` (Lambda) authorizer: the identity-source precondition, then the
        // decision this emulator cannot obtain. See this function's docs.
        let absent: Vec<&str> = authorizer
            .identity_source
            .iter()
            .filter(|identity_source| {
                !presents_identity_source(
                    identity_source,
                    header,
                    &query_parameters,
                    &stage.stage_variables,
                )
            })
            .map(String::as_str)
            .collect();
        if !absent.is_empty() {
            tracing::info!(
                api_id = %api.api_id,
                stage = %stage.stage_name,
                authorizer_id = %authorizer.authorizer_id,
                absent = ?absent,
                "refusing an invoke: an identity source of the route's authorizer is \
                 absent, so the authorizer is not invoked"
            );
            return Some(json_response(
                UNAUTHORIZED_STATUS,
                UNAUTHORIZED_MESSAGE,
                None,
            ));
        }
        tracing::warn!(
            api_id = %api.api_id,
            stage = %stage.stage_name,
            authorizer_id = %authorizer.authorizer_id,
            authorizer_uri = ?authorizer.authorizer_uri,
            "refusing an invoke: deciding this route's Lambda authorizer would require \
             invoking its function, which this emulator cannot run"
        );
        return Some(json_response(UNPLACEABLE_STATUS, INTERNAL_MESSAGE, None));
    }
    let Some(configuration) = authorizer.jwt_configuration.as_ref() else {
        tracing::warn!(
            api_id = %api.api_id,
            route_key = %route.route_key,
            authorizer_id = %authorizer.authorizer_id,
            "the route's JWT authorizer holds no JwtConfiguration to evaluate a token against"
        );
        return Some(json_response(UNPLACEABLE_STATUS, INTERNAL_MESSAGE, None));
    };
    let audience = configuration.audience.as_deref().unwrap_or_default();
    let identity_source = authorizer
        .identity_source
        .iter()
        .map(String::as_str)
        .find(|source| !source.trim().is_empty())
        .unwrap_or(DEFAULT_JWT_IDENTITY_SOURCE);
    let presented = identity_source_value(identity_source, header, &query_parameters);
    let decision = jwt::evaluate(
        presented.as_deref(),
        jwt::Expectations {
            issuer: configuration.issuer.as_deref().unwrap_or_default(),
            audience,
            scopes: &route.authorization_scopes,
        },
        now_unix_seconds(),
    );
    match decision {
        jwt::Decision::Allow => None,
        jwt::Decision::Unauthorized(reason) => {
            tracing::info!(
                api_id = %api.api_id,
                stage = %stage.stage_name,
                route_key = %route.route_key,
                authorizer_id = %authorizer.authorizer_id,
                reason = ?reason,
                presented = ?jwt::described(presented.as_deref()),
                "refusing an invoke: the JWT authorizer did not accept the presented token"
            );
            Some(json_response(
                UNAUTHORIZED_STATUS,
                UNAUTHORIZED_MESSAGE,
                None,
            ))
        }
        jwt::Decision::InsufficientScope => {
            tracing::info!(
                api_id = %api.api_id,
                stage = %stage.stage_name,
                route_key = %route.route_key,
                authorizer_id = %authorizer.authorizer_id,
                scopes = ?route.authorization_scopes,
                "refusing an invoke: the presented token claims none of the route's \
                 authorization scopes"
            );
            Some(forbidden())
        }
    }
}

/// The answer the route's integration gives, for a request its authorization admitted.
///
/// Two answers, by what the integration's backend IS:
///
/// * an `HTTP_PROXY` upstream is a URI the CALLER can reach, and this emulator opens no
///   outbound connection of its own, so the caller is sent there: `307 Temporary Redirect`
///   with the stored `IntegrationUri` in `Location`. A `307` is the redirect that preserves
///   the request's method and body (RFC 7231 §6.4.7), so the client repeats exactly the
///   request the gateway would have forwarded. Nothing about the upstream is invented —
///   the caller is told where the backend is instead of being handed a body no upstream
///   produced;
/// * an `AWS_PROXY` integration invokes a Lambda function, which cannot run here (Lambda's
///   own `Invoke` has no in-process runtime for a deployment package), and no redirect can
///   stand in for it: an invocation is not a URI a client can fetch. It is answered the way
///   API Gateway answers an integration that produced no response, `500`
///   `{"message":"Internal Server Error"}`, and the reason is logged so a developer reading
///   the emulator's output learns which backend was not called. See the module docs for
///   which open issue owns the fault-to-status mapping.
///
/// A route that targets no integration, or one whose target names none this api holds, is
/// the same `500` for a different reason: a route with nothing behind it cannot answer at
/// all.
fn serve_integration(api: &Api, stage: &Stage, route: &Route) -> RtResponse {
    let integration: Option<&Integration> = route
        .target
        .as_deref()
        .and_then(crate::state::target_integration_id)
        .and_then(|integration_id| api.integrations.get(integration_id));
    let Some(integration) = integration else {
        tracing::warn!(
            api_id = %api.api_id,
            stage = %stage.stage_name,
            route_key = %route.route_key,
            target = ?route.target,
            "the matched route targets no integration of this api, so it has nothing to call"
        );
        return json_response(UNPLACEABLE_STATUS, INTERNAL_MESSAGE, None);
    };
    if integration.integration_type == crate::state::HTTP_PROXY_INTEGRATION_TYPE {
        if let Some(upstream) = integration
            .integration_uri
            .as_deref()
            .map(str::trim)
            .filter(|upstream| !upstream.is_empty())
        {
            tracing::info!(
                api_id = %api.api_id,
                stage = %stage.stage_name,
                route_key = %route.route_key,
                "redirecting the caller to this route's HTTP_PROXY upstream rather than \
                 connecting to it"
            );
            return redirect(upstream);
        }
    }
    tracing::warn!(
        api_id = %api.api_id,
        stage = %stage.stage_name,
        route_key = %route.route_key,
        integration_type = %integration.integration_type,
        integration_uri = ?integration.integration_uri,
        "the route's integration names a backend this emulator cannot call, so the \
         request is answered as an integration that produced no response"
    );
    json_response(UNPLACEABLE_STATUS, INTERNAL_MESSAGE, None)
}

/// `307 Temporary Redirect` to `location`, with no body of its own: the answer for an
/// upstream the CALLER can reach and this emulator does not connect to.
fn redirect(location: &str) -> RtResponse {
    let mut response = RtResponse::new(
        StatusCode::try_from(TEMPORARY_REDIRECT_STATUS).unwrap_or(
            StatusCode::try_from(UNPLACEABLE_STATUS).expect("500 is a valid status code"),
        ),
        SdkBody::empty(),
    );
    let headers = response.headers_mut();
    if headers.try_insert(LOCATION_HEADER, location.to_string()).is_err() {
        // A stored `IntegrationUri` that cannot be a header value names no reachable
        // upstream, so there is nowhere to send the caller.
        return json_response(UNPLACEABLE_STATUS, INTERNAL_MESSAGE, None);
    }
    let _ = headers.try_insert("content-length", "0");
    response
}

/// Now, as whole seconds since the epoch — the `NumericDate` a token's `exp` and `nbf` are
/// compared against (RFC 7519 §2). A clock before the epoch reads as the epoch, so the
/// comparison is total.
fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    //! The pure rules this module owns that a wire test cannot vary cheaply: which route
    //! key matches a request, and how an invoke path is split into a stage and a route
    //! path. The served behaviour is pinned end to end in
    //! `tests/all/http_api_jwt_data_plane.rs`.

    use super::*;

    /// The rank of one key against one request, as [`specificity`] reads it.
    fn rank(route_key: &str, method: &str, path: &str) -> Option<(Vec<u8>, u8)> {
        let segments = path_segments(path);
        specificity(route_key, method, &segments)
    }

    #[test]
    fn a_route_key_matches_only_the_requests_it_names() {
        let cases: &[(&str, &str, &str, &str, bool)] = &[
            ("an exact method and path", "GET /orders", "GET", "/orders", true),
            ("a trailing slash on the request", "GET /orders", "GET", "/orders/", true),
            ("another method", "GET /orders", "POST", "/orders", false),
            ("another path", "GET /orders", "GET", "/invoices", false),
            ("a deeper path", "GET /orders", "GET", "/orders/1", false),
            ("the ANY catch-all", "ANY /orders", "DELETE", "/orders", true),
            ("a single-segment variable", "GET /orders/{id}", "GET", "/orders/7", true),
            (
                "a single-segment variable against two segments",
                "GET /orders/{id}",
                "GET",
                "/orders/7/items",
                false,
            ),
            (
                "a single-segment variable against none",
                "GET /orders/{id}",
                "GET",
                "/orders",
                false,
            ),
            ("the greedy remainder", "ANY /{proxy+}", "GET", "/orders/7/items", true),
            ("the greedy remainder over one segment", "ANY /{proxy+}", "GET", "/x", true),
            (
                // Greedy stands for one or more, so the api root is not it.
                "the greedy remainder over no segment",
                "ANY /{proxy+}",
                "GET",
                "/",
                false,
            ),
            ("a literal under a greedy prefix", "GET /files/{path+}", "GET", "/files/a/b", true),
            ("a key naming no path at all", "$connect", "GET", "/", false),
        ];
        for (case, route_key, method, path, matches) in cases {
            let actual = rank(route_key, method, path).is_some();
            assert_eq!(
                actual, *matches,
                "case {case:?}: expected {route_key:?} to match {method} {path}: {matches}"
            );
        }
    }

    #[test]
    fn the_most_specific_matching_key_outranks_the_others() {
        // Each pair: the key that must win, and the key it must beat, for one request.
        let cases: &[(&str, &str, &str, &str, &str)] = &[
            (
                "a literal beats a variable at the same position",
                "GET /orders/new",
                "GET /orders/{id}",
                "GET",
                "/orders/new",
            ),
            (
                "a leftmost literal decides",
                "GET /orders/{id}",
                "GET /{resource}/{id}",
                "GET",
                "/orders/7",
            ),
            (
                "a segment-for-segment match beats the greedy remainder",
                "GET /orders/{id}",
                "ANY /{proxy+}",
                "GET",
                "/orders/7",
            ),
            (
                "the request's own verb beats the ANY catch-all",
                "GET /orders",
                "ANY /orders",
                "GET",
                "/orders",
            ),
        ];
        for (case, winner, loser, method, path) in cases {
            let winning = rank(winner, method, path)
                .unwrap_or_else(|| panic!("case {case:?}: {winner:?} must match {method} {path}"));
            let losing = rank(loser, method, path)
                .unwrap_or_else(|| panic!("case {case:?}: {loser:?} must match {method} {path}"));
            assert!(
                winning > losing,
                "case {case:?}: expected {winner:?} ({winning:?}) to outrank {loser:?} ({losing:?})"
            );
        }
    }

    #[test]
    fn an_invoke_path_keeps_one_leading_slash_and_no_trailing_one() {
        assert_eq!(normalised_path(""), "/");
        assert_eq!(normalised_path("/"), "/");
        assert_eq!(normalised_path("orders"), "/orders");
        assert_eq!(normalised_path("/orders/"), "/orders");
        assert_eq!(normalised_path("//orders//7/"), "/orders/7");
    }

    #[test]
    fn a_path_is_split_into_its_first_segment_and_the_rest() {
        assert_eq!(split_first_segment("/abc/prod/x"), Some(("abc", "/prod/x")));
        assert_eq!(split_first_segment("/abc"), Some(("abc", "/")));
        assert_eq!(split_first_segment("abc/x"), Some(("abc", "/x")));
        assert_eq!(split_first_segment("/"), None);
        assert_eq!(split_first_segment(""), None);
    }
}
