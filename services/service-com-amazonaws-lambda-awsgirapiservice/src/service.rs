// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-lambda-awsgirapiservice` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## The download plane
//!
//! `GetFunction` hands out `Code$Location`, "A presigned URL that you can use to download
//! the deployment package", and `GetLayerVersion` hands out `Content$Location`, "A link to
//! the layer archive in Amazon S3"; a URL this service announces is a promise that a
//! caller can follow it. Both are addressed to the per-region download hosts this service
//! publishes (`crate::state::is_download_host`), under
//! `crate::state::DOWNLOAD_PATH_PREFIX`, which name no modelled operation. Two seams
//! carry them, the same pair any emulated data plane uses:
//!
//! - [`Service::can_handle`] also claims a request addressed to those published hosts,
//!   whose first label names no service, so the router does not answer `UnknownService`
//!   for a URL this service handed out;
//! - [`Service::override_operation`] serves it, through [`Downloads`], which the router
//!   reaches BEFORE authentication — which is what lets a presigned URL be followed
//!   without a signature, as a presigned URL is.
//!
//! What it serves is the bytes the store HOLDS for the object the URL names: the
//! deployment package of the function revision the location was reported for (a published
//! version froze its own, so its location keeps serving those after the unpublished
//! version's code is replaced), or one layer version's archive. A download is a pure
//! READ: it answers the same bytes twice and changes nothing a later caller sees. A URL
//! whose object this emulator holds no bytes for — an S3-sourced package it does not
//! resolve — is a `404`, because nothing may be served that was not stored.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use service_common::event_stream::EventStreamRoute;
use service_common::shared_path::SharedPathArn;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The order this service states one enum's value set in, one row per enum.
///
/// The value SET is the model's and the CHECK is the framework's; only the order the
/// sentence lists the set in is chosen here, because no model records it: AWS states a
/// value set in neither the model's declaration order nor ascending order, so each row
/// is RECORDED from an AWS answer rather than computed. Source for the one row below:
/// the answer the corpus unit `lambda.put-runtime-management-config-invalid-mode-rejected`
/// carries for an `UpdateRuntimeOn` outside its value set.
///
/// A row is matched by its value SET rather than by the member the clause names (see
/// [`reported_enum_order`]), because the order is a property of the enum SHAPE: every
/// member targeting that enum, in any operation, is stated alike. An enum with no row —
/// every other enum this model declares — keeps the model's declaration order, which is
/// the only order there is evidence for.
const REPORTED_ENUM_ORDERS: &[&[&str]] = &[
    // com.amazonaws.lambda#UpdateRuntimeOn (`Auto | Manual | FunctionUpdate` in the
    // model), reported by AWS as:
    &["FunctionUpdate", "Manual", "Auto"],
];

/// `allowed` in the order this service reports that enum in, or in the order the model
/// declares it (the order it arrived in) when there is no recorded order for it — the
/// only order there is evidence for.
fn reported_enum_order(allowed: &[String]) -> Vec<String> {
    match REPORTED_ENUM_ORDERS
        .iter()
        .find(|reported| is_same_set(reported, allowed))
    {
        Some(reported) => reported.iter().map(|value| (*value).to_string()).collect(),
        None => allowed.to_vec(),
    }
}

/// Whether two value sets hold the same values. An enum's values are unique by
/// definition, so equal lengths plus containment is set equality.
fn is_same_set(reported: &[&str], allowed: &[String]) -> bool {
    reported.len() == allowed.len()
        && reported
            .iter()
            .all(|value| allowed.iter().any(|candidate| candidate == value))
}

/// The literal first URI path segments that LEAD to the `{Resource}` ARN label of the
/// one path space this model and a peer signing as `lambda` bind IDENTICALLY:
/// `ListTags` (`GET`), `TagResource` (`POST`) and `UntagResource` (`DELETE`) are all
/// bound to `/2017-03-31/tags/{Resource}` here AND in the Lambda MicroVMs model, so the
/// model-derived path discriminator alone cannot separate the two there. Source:
/// `smithy_query(query="digest", …)` on both models.
const SHARED_TAGGING_PATH: &[&str] = &["2017-03-31", "tags"];

/// The `aws.api#service$arnNamespace` every service signing as `lambda` shares
/// (`smithy_query(query="service_info")`: `arnNamespace: lambda` for all four), and so
/// the only service field an ARN any of their tagging operations addresses can carry.
const SHARED_ARN_NAMESPACE: &str = "lambda";

/// The ARN resource types reachable on [`SHARED_TAGGING_PATH`] that belong to a PEER
/// service rather than to this one.
///
/// A resource belongs to the service whose own operations MINT it: `microvm-image` is
/// minted by `CreateMicrovmImage` in the Lambda MicroVMs service, whose model binds the
/// same tagging URIs and whose `TaggableResource` pattern accepts it (this model's does
/// not). Every other type that pattern accepts here — `function`, `layer`,
/// `code-signing-config`, `event-source-mapping`, `capacity-provider`,
/// `network-connector` — is minted by this
/// service or has no other claimant, so it stays with this endpoint, whose own identity
/// IS the shared signing name (`sdkId: Lambda`, `endpointPrefix: lambda`). Source:
/// `smithy_query(query="shape", shape="TaggableResource")` on both models.
const PEER_TAGGING_RESOURCE_TYPES: &[&str] = &["microvm-image"];

/// Whether this service claims a request on the `/2017-03-31/tags/{Resource}` path space
/// it shares with the Lambda MicroVMs service.
///
/// The ARN in the path names the resource, and a resource belongs to exactly one of the
/// two endpoints, so the resource type decides: this endpoint claims every tagging
/// request EXCEPT one whose ARN names a peer's resource. Everything else on the shared
/// API-date segment stays here — a label that names no recognisable `lambda` resource, and
/// a path under that segment neither model binds — because this is the service the shared
/// signing name belongs to, so exactly one service claims every request on it.
fn claims_shared_tagging_path(parts: &http::request::Parts) -> bool {
    match service_common::shared_path::shared_path_arn(
        parts.uri.path(),
        SHARED_TAGGING_PATH,
        SHARED_ARN_NAMESPACE,
    ) {
        SharedPathArn::NotShared | SharedPathArn::Unrecognised => true,
        SharedPathArn::Resource(kind) => !PEER_TAGGING_RESOURCE_TYPES.contains(&kind.as_str()),
    }
}

/// The `com-amazonaws-lambda-awsgirapiservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// Serves the download URLs `GetFunction` and `GetLayerVersion` announce, from the
    /// SAME shared engine the generated entry threads into every operation — not a
    /// second copy of the store.
    downloads: Downloads<Eng>,
    /// Serves `InvokeWithResponseStream`, whose `@streaming`-union output the generated
    /// routing-table entry declines; from the same shared engine, for the same reason.
    response_stream: EventStreamRoute<
        crate::ops::invoke_with_response_stream::ResponseStreamEntry<Eng>,
    >,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            response_stream: EventStreamRoute(
                crate::ops::invoke_with_response_stream::ResponseStreamEntry::new(Arc::clone(
                    &engine,
                )),
            ),
            downloads: Downloads { engine },
        }
    }
}

/// Serves a deployment package or a layer archive from the bytes [`crate::state`] holds.
///
/// This is the data plane behind the URLs `GetFunction` and `GetLayerVersion` announce.
/// It is NOT a modelled operation — AWS serves those objects from a presigned S3 URL, and
/// this emulator serves them from the host it published in that URL, because a URL it
/// announces has to resolve. It only READS.
struct Downloads<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> ErasedOperation for Downloads<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // An object is READ, so `GET` carries it and `HEAD` reports its metadata
            // without the bytes; anything else is refused as the method it is.
            if !matches!(req.method(), "GET" | "HEAD") {
                return download_error(405, "MethodNotAllowed", METHOD_NOT_ALLOWED_MESSAGE);
            }
            let Some(target) = request_target(req) else {
                return download_error(404, "NoSuchKey", NO_SUCH_KEY_MESSAGE);
            };
            let Some(state) = crate::state::service_state(&*self.engine) else {
                return download_error(
                    500,
                    "ServiceException",
                    "Lambda state is not reachable on this engine",
                );
            };
            let store = state.lock();
            let held = match &target {
                crate::state::DownloadTarget::FunctionPackage { name, revision_id } => store
                    .functions
                    .get(name)
                    .and_then(|function| function.package_at_revision(revision_id))
                    .and_then(crate::state::held_package_bytes),
                crate::state::DownloadTarget::LayerArchive { name, version } => store
                    .layers
                    .get(name)
                    .and_then(|layer| layer.versions.get(version))
                    .and_then(|held| held.archive.zip_bytes.as_deref()),
            };
            // A URL whose object this store no longer holds — a deleted function or layer
            // version, a revision that has been replaced — names no object, and so does
            // one whose bytes were never held here at all.
            let Some(bytes) = held else {
                return download_error(404, "NoSuchKey", NO_SUCH_KEY_MESSAGE);
            };
            download_response(bytes, req.method() == "GET")
        })
    }

    /// Served UNSIGNED. A download URL is handed to a caller to fetch directly, so the
    /// fetch carries no SigV4 `Authorization` header. The modelled operations are
    /// unaffected — the router reads their schemes from the model, which this answer
    /// cannot weaken.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(&[])
    }
}

/// What this service's download message says for an object it does not hold.
const NO_SUCH_KEY_MESSAGE: &str = "The specified key does not exist.";

/// What it says for a method a download does not answer.
const METHOD_NOT_ALLOWED_MESSAGE: &str = "The specified method is not allowed against this resource.";

/// The object a request to a download host names, or `None` when it names none.
///
/// The host comes from the request, because it is what says WHICH of the two published
/// stores the path addresses; both are read by `crate::state`, so the URL this service
/// mints and the request it serves cannot disagree about either. The path is taken by
/// `service_common`'s one reader of a request URI, so this plane and every other service's
/// claim read a URI the same way.
fn request_target(req: &RtRequest) -> Option<crate::state::DownloadTarget> {
    crate::state::parse_download_target(
        &request_authority(req)?,
        service_common::request_path(req.uri()),
    )
}

/// Whether the request is addressed to one of the download hosts this service publishes.
///
/// The HOST is what makes a request this plane's, rather than the path: a path that host
/// publishes no object at is answered by the plane as the object it does not hold, not by
/// the router as an unrouted request.
fn names_download_host(req: &RtRequest) -> bool {
    request_authority(req).is_some_and(|host| crate::state::is_download_host(&host))
}

/// The authority a request is addressed to: its `Host` header, or the URI authority when
/// the request carries one instead.
fn request_authority(req: &RtRequest) -> Option<String> {
    req.headers()
        .get("host")
        .map(str::to_string)
        .or_else(|| {
            req.uri()
                .parse::<::http::Uri>()
                .ok()
                .and_then(|uri| uri.authority().map(|authority| authority.as_str().to_string()))
        })
}

/// Serve one stored object: its own bytes, under its own length.
///
/// `with_body` is false for a `HEAD`, which reports the object's metadata without its
/// bytes.
fn download_response(bytes: &[u8], with_body: bool) -> RtResponse {
    let body = if with_body {
        SdkBody::from(bytes.to_vec())
    } else {
        SdkBody::empty()
    };
    let mut response = RtResponse::new(download_status(200), body);
    // A deployment package and a layer archive are both .zip archives, which is what the
    // store holds for them.
    response
        .headers_mut()
        .insert("content-type", "application/zip");
    response
        .headers_mut()
        .insert("content-length", bytes.len().to_string());
    response
}

/// One refusal of a download request, in this service's own JSON error envelope: the
/// path is not a modelled operation, so there is no modelled error to render.
fn download_error(code: u16, error_code: &str, message: &str) -> RtResponse {
    let body = ::serde_json::json!({ "__type": error_code, "message": message }).to_string();
    let mut response = RtResponse::new(download_status(code), SdkBody::from(body));
    response
        .headers_mut()
        .insert("content-type", "application/json");
    response
        .headers_mut()
        .insert("x-amzn-errortype", error_code.to_string());
    response
}

/// A status this plane answers with. Every call site passes a constant, so the conversion
/// cannot fail.
fn download_status(code: u16) -> StatusCode {
    StatusCode::try_from(code).expect("the download plane's statuses are valid constants")
}

/// The `Host` header of a buffered request, or the URI authority when the request is
/// HTTP/2 (which carries `:authority` on the URI instead of a `Host` header).
fn request_host(parts: &http::request::Parts) -> Option<&str> {
    match parts.headers.get("host").and_then(|host| host.to_str().ok()) {
        Some(host) => Some(host),
        None => parts.uri.authority().map(http::uri::Authority::as_str),
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service signs as `lambda` — a signing name it SHARES with every other
        // emulated service in the Lambda family (Lambda Core, Lambda MicroVMs and
        // Lambda Web; `smithy_query(query="service_info")` gives all four
        // `sigv4Name: lambda`). The router resolves a request's routing key from the
        // SigV4 credential scope first, so all four see the key `lambda`: claiming on
        // the signing name alone makes each one claim the others' traffic, which the
        // router answers `500 AmbiguousService` instead of serving any of them —
        // leaving every Lambda operation unreachable.
        //
        // So the claim is narrowed to this service's own request space, by the one rule
        // `service-common` states for a shared signing name: this model is `restJson1`
        // (a REST protocol, so the `rpc_service_shape_id` argument is `None`), and a
        // REST service's own surface is the modelled URI path space its generated
        // `ROUTE_PATH_SEGMENTS` publishes — here the API-date segments this model binds
        // (`2015-03-31`, `2018-10-31`, …), which are disjoint from the peers' own
        // (`2026-04-04`, `2025-09-09`, `2025-03-07`) except for the shared tagging path
        // `/2017-03-31/tags/{Resource}`, where the ARN decides (see
        // [`claims_shared_tagging_path`]).
        service_common::claims_request(
            parts,
            self.entry.identity().signing_name(),
            crate::ROUTE_PATH_SEGMENTS,
            None,
        ) && claims_shared_tagging_path(parts)
            // And a request addressed to one of the DOWNLOAD hosts this service publishes
            // its presigned URLs under, whose first label names no service at all:
            // without this the router answers `UnknownService` for a URL this service
            // handed out. See the module docs.
            || request_host(parts).is_some_and(crate::state::is_download_host)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        //
        // The download plane is the one such request: a fetch of a URL this service
        // announced names no modelled operation at all, so there is no routing key to
        // resolve for it (see the module docs). Every method addressed to a published
        // download host reaches the plane, which decides the method itself: a read is
        // served and anything else is refused as a method, rather than falling through to
        // the router's answer for an unrouted request.
        if names_download_host(req) {
            return Some(&self.downloads as &dyn ErasedOperation);
        }
        // And `InvokeWithResponseStream`: its output binds a `@streaming` union, which
        // the generated routing-table entry declines (that entry is still what the router
        // reads the model's auth schemes off, so the operation stays signed exactly as the
        // model declares). The match is the operation's own route, applied by the function
        // that lives beside the handler, so routing and handling cannot disagree about
        // which requests it claims.
        if crate::ops::invoke_with_response_stream::addresses_response_streaming_invocations(req) {
            return Some(&self.response_stream as &dyn ErasedOperation);
        }
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. Everything about the envelope — the `N validation error(s)
        // detected:` prefix, the member paths, the echoed value and every requirement
        // sentence — is the framework's, so the two refusal paths cannot drift.
        //
        // TWO renderings are stated here, neither of which a model records:
        //
        // * the ORDER of an enum's value set (see [`reported_enum_order`]), which is
        //   this service's own and is applied below;
        // * how a `@pattern` requirement quotes its expression. AWS states it WITHOUT
        //   the `^`/`$` anchors a whole-value Smithy pattern wraps it in, so a
        //   `TaggableResource` the tagging URIs refuse is reported as
        //   `Member must satisfy regular expression pattern: arn:(aws[a-zA-Z-]*):lambda:…`
        //   rather than `^arn:…$`. That wording is shared by every service on a JSON
        //   protocol, so it lives in `service-common` and is delegated to rather than
        //   restated — the CHECK still uses the model's pattern as written, and only
        //   the rendering drops the pair.
        let Refusal::Invalid(violations) = refusal else {
            // An unauthenticated caller keeps the framework's 403 code and wording.
            return None;
        };
        let restated: Vec<Violation> = violations
            .iter()
            .map(|violation| match &violation.kind {
                ViolationKind::EnumValue(allowed) => Violation {
                    path: violation.path.clone(),
                    kind: ViolationKind::EnumValue(reported_enum_order(allowed)),
                    value: violation.value.clone(),
                },
                _ => violation.clone(),
            })
            .collect();
        // The code, the envelope and the `@pattern` rendering are `service-common`'s;
        // the value set order above is this service's.
        service_common::aws_validation_refusal(&Refusal::Invalid(restated))
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}
