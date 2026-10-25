// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-apigatewayv2-apigatewayv2` service: how it claims requests and how it words the
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
//! ## This service's other edited seams: the two `execute-api` data planes
//!
//! Beside its modelled `/v2/…` control plane, this service is addressed on the invoke
//! endpoint of every api it mints — `{api-id}.execute-api.{region}.amazonaws.com`, the URI
//! `Api$ApiEndpoint` reports — where a client's request is no modelled operation at all.
//! Two modules own that half, one per `ProtocolType`, over the readings they share
//! ([`data_plane`]):
//!
//! - [`websocket`] — the RFC 6455 opening handshake a WEBSOCKET api's stage is reached
//!   with: which handshakes this service claims ([`websocket::claims_parts`]) and what each
//!   is answered with ([`websocket::serve`], reached from [`Service::override_routing_error`]);
//! - [`invoke`] — an ordinary HTTP request to a route of a deployed HTTP api stage: the
//!   claim ([`invoke::claims_parts`]), and the serving ([`invoke::serve`]) from BOTH
//!   serving seams — [`Service::override_routing_error`] for the invoke that presents no
//!   credential, which the router reaches BEFORE authentication (so a route whose
//!   `AuthorizationType` is `NONE` or `JWT` is reachable by a client holding no AWS
//!   credential), and [`Service::override_operation`] for the invoke that DOES present a
//!   SigV4 credential, through [`CredentialedDataPlane`], so the emulator's own signature
//!   verification runs in front of an `AWS_IAM` route.

mod data_plane;
mod invoke;
mod jwt;
mod websocket;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::{Violation, ViolationKind};
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, OperationContext, Refusal, Refused, Service,
};
use std::sync::Arc;

/// Whether this service serves the request these `parts` came from.
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable owner
/// of this rule, and it is CALLED here rather than re-implemented: the request's route key
/// must match this service's signing name AND its first URI path segment must be one the
/// generated, model-derived `ROUTE_PATH_SEGMENTS` declares, by exact segment equality. It
/// is the same rule `service_common::claims_request` applies for a REST service; this
/// crate reaches the framework helper directly, as
/// `service-com-amazonaws-transcribe-transcribe`, `-appconfig-amazonappconfig` and
/// `-ivs-*` do.
///
/// The helper takes a whole `http::Request` while [`Service::can_handle`] is handed only
/// the request's parts, so the two things it reads — the URI and the headers — are
/// re-presented to it as a bodiless request. It reads neither the body nor the method, so
/// nothing it decides on is missing.
fn can_handle_parts(parts: &http::request::Parts) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    emulator_server::can_handle_based_on_route_key_and_path(
        &request,
        crate::ROUTE_KEY,
        crate::ROUTE_PATH_SEGMENTS,
    )
}

/// This service's own sentence for one violated member, or `None` for a member it
/// states no sentence of its own for.
///
/// API Gateway words the refusal of some members in the member's OWN terms rather than
/// in the generic "value at path failed to satisfy constraint" form: an api's protocol
/// is refused by listing the protocols there are, and an integration's timeout by naming
/// the window it has to fall in. Each sentence is built from what the MODEL declares for
/// that member — the enum's catalog, the range's bounds — so it cannot drift from the
/// constraint it is explaining.
fn stated_violation(violation: &Violation) -> Option<String> {
    // Only the FIRST violation of a refusal is offered here, so a request that breaks
    // several rules at once is stated in the framework's envelope, which lists them all.
    match (violation.path.as_str(), &violation.kind) {
        // `CreateApi$ProtocolType` / `ProtocolType`, the enum of the protocols this
        // control plane serves. The catalog is the generated `PROTOCOL_TYPE_VALUES`
        // rather than the violation's own list, because the sentence lists the protocols
        // in the catalog's order and the violation carries them in the model's
        // declaration order.
        ("protocolType", ViolationKind::EnumValue(_)) => Some(format!(
            "Invalid protocol specified. Must be one of [{}]",
            crate::ops::types::PROTOCOL_TYPE_VALUES.join(", ")
        )),
        // `CreateIntegration$TimeoutInMillis`, whose shape declares the window
        // (`IntegerWithLengthBetween50And30000`, range 50..30000).
        ("timeoutInMillis", ViolationKind::MinValue(_) | ViolationKind::MaxValue(_)) => {
            Some(format!(
                "Timeout should be between {} ms and {} ms",
                crate::state::MIN_INTEGRATION_TIMEOUT_MILLIS,
                crate::state::MAX_INTEGRATION_TIMEOUT_MILLIS
            ))
        }
        _ => None,
    }
}

/// The `com-amazonaws-apigatewayv2-apigatewayv2` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The one shared engine, held here because the two data planes ([`websocket`],
    /// [`invoke`]) are decided and served from seams the generated entry does not reach: a
    /// claim, a routing-error answer and one operation of this file's own, all of which
    /// read this service's own records.
    engine: Arc<Eng>,
    /// The data-plane handler the router names as an OPERATION for an invoke that presents
    /// a SigV4 credential, so the emulator authenticates it before anything is served. See
    /// [`CredentialedDataPlane`].
    credentialed_data_plane: CredentialedDataPlane<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            credentialed_data_plane: CredentialedDataPlane {
                engine: Arc::clone(&engine),
            },
            engine,
        }
    }
}

/// The auth schemes a credentialed `execute-api` invoke is verified under.
///
/// SigV4 is the only scheme this service's model declares (`aws.auth#sigv4` name
/// `apigateway`), and it is the one a route whose `AuthorizationType` is `AWS_IAM` is
/// reached with, so an invoke arrives either without an AWS credential — which the
/// routing-error seam answers without the router authenticating anything — or with a SigV4
/// credential, which this names so the router's own authenticator verifies it.
const DATA_PLANE_AUTH_SCHEMES: &[&str] = &[::emulator_server::aws_credentials::SIGV4_SCHEME];

/// The HTTP data plane for a request that PRESENTS a SigV4 credential, as an
/// [`ErasedOperation`] so the router authenticates it.
///
/// It holds the SAME shared engine the generated entry threads into every operation, and
/// serves the request through exactly the function the routing-error seam uses
/// ([`invoke::serve`]), so the credentialed and the credential-less path cannot diverge in
/// what they serve — only in whether a signature was verified on the way in.
struct CredentialedDataPlane<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> ErasedOperation for CredentialedDataPlane<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            if invoke::claims_request(&*self.engine, req) {
                return invoke::serve(&*self.engine, req);
            }
            // Unreachable: `override_operation` names this only for a request this data
            // plane claims.
            svc.definition().routing_error_response(req)
        })
    }

    /// This is NOT a modelled operation, so there is no modelled operation for the router
    /// to read schemes off: without this the router would find no scheme for an invoke and
    /// could not verify the credential it carries.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(DATA_PLANE_AUTH_SCHEMES)
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
        // This service signs as `apigateway` — a signing name it SHARES with the Amazon
        // API Gateway control plane
        // (`services/service-com-amazonaws-apigateway-backplanecontrolservice`, `sdkId`
        // `API Gateway`, the same `apigateway` endpoint prefix and signing name), a
        // DIFFERENT service that AWS separates by URI path space: every operation of THIS
        // model is bound under `/v2/…` (`/v2/apis`, `/v2/domainnames`, `/v2/vpclinks`,
        // `/v2/tags`, …), while the control plane's are bound under `/restapis`,
        // `/apikeys`, `/account`, … The router resolves a request's routing key from the
        // SigV4 credential scope, so both services resolve to `apigateway`; claiming on
        // the signing name ALONE makes both claim every such request, which the router
        // answers `500 AmbiguousService` — a code no AWS model carries — leaving BOTH
        // services unreachable.
        //
        // So the claim adds the framework's path conjunct over this service's own
        // model-derived path space — see [`can_handle_parts`], which for this model is the
        // single segment `v2`. None of the control plane's segments is `v2`, so it alone
        // claims its traffic and each service answers only its own.
        if can_handle_parts(parts) {
            return true;
        }
        // And the two DATA planes: a request addressed to a stage of an api THIS service
        // holds. Such a request carries no credential scope naming this service (or
        // carries one while naming no `/v2/…` route), and its path is no modelled
        // operation's, so the front door could resolve no target service for it and
        // answered `404 UnknownService` — for the endpoint this service itself reported in
        // `ApiEndpoint`. Each claim is narrowed to an api this service HOLDS, read from the
        // one shared store, and to that api's own `ProtocolType`, so exactly one service
        // and exactly one of these modules claims the request; see [`websocket`] and
        // [`invoke`] for the whole rule and for what each request is answered with.
        websocket::claims_parts(&*self.engine, parts) || invoke::claims_parts(&*self.engine, parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // An invoke of an HTTP api's stage that PRESENTS a SigV4 credential is named as an
        // operation, so the router verifies that credential's signature before anything is
        // served — the router authenticates only what it names an operation for. An invoke
        // carrying no credential is left unnamed on purpose: it is answered by
        // `override_routing_error` below, which the router reaches before authentication,
        // so a route whose `AuthorizationType` is `NONE` — or `JWT`, whose caller presents
        // a bearer token and no AWS credential — is reachable, as it is on AWS.
        //
        // The predicate is [`invoke::carries_sigv4_credential`], the SAME one that admits
        // an `AWS_IAM` route: a credential this names is verified by the router before the
        // route is served, so the two cannot disagree about what counts as a credential (a
        // presigned URL carries its scope in the query string, not in a header).
        if invoke::claims_request(&*self.engine, req) && invoke::carries_sigv4_credential(req) {
            return Some(&self.credentialed_data_plane);
        }
        // Otherwise the generated routing table resolves the request, as it does for every
        // modelled operation of this service.
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) BEFORE any handler
        // runs, so it is where this service names the error shape such a refusal
        // carries. This model declares exactly one error for a request it cannot
        // accept — `BadRequestException`, "The request is not valid, for example, the
        // input is incomplete or incorrect", bound to HTTP 400 on every operation that
        // writes — and declares no `ValidationException` anywhere, so the framework's
        // default code names a shape no client of this service can resolve.
        //
        // The message is this service's own sentence for the member and the constraint
        // that failed where the service has one ([`stated_violation`]), and otherwise
        // the framework's, which already names both.
        //
        // A refusal of the caller's IDENTITY is not an invalid request and keeps the
        // framework's 403 wording.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        let message = violations
            .first()
            .and_then(stated_violation)
            .or_else(|| aws_smithy_schema_server::envelope(violations))
            .unwrap_or_else(|| "The request is not valid.".to_string());
        Some(Refused {
            code: "BadRequestException".to_string(),
            message,
        })
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
        //
        // A WebSocket opening handshake IS such a request: it names no modelled operation
        // (its path is a stage's, not a `/v2/…` route's), so this pre-authentication seam
        // is where this service's WebSocket data plane answers it. It is served here
        // rather than named as an operation because a handshake carries no credential
        // whose verification could change the answer — the refusals below are computed
        // from the api's and the stage's stored state alone.
        //
        // So is an invoke of an HTTP api's stage that carries no SigV4 credential, which is
        // every invoke of a `NONE` or `JWT` route: reaching it BEFORE authentication is
        // what lets a client holding only a bearer token be answered at all.
        if websocket::claims_request(&*self.engine, req) {
            return Some(websocket::serve(&*self.engine, req));
        }
        if invoke::claims_request(&*self.engine, req) {
            return Some(invoke::serve(&*self.engine, req));
        }
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
