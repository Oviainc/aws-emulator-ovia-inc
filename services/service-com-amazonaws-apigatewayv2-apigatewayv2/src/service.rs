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
//! - `override_protocol` — the protocol every modelled response is serialized by. EDITED
//!   here: the model's own protocol, with the framing correction the section "How an answer
//!   that carries no content is framed" below states.
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
//!   claim ([`invoke::claims_parts`]), and the serving ([`invoke::serve`]) from the ONE
//!   serving seam — [`Service::override_routing_error`], which the router reaches BEFORE
//!   authentication, for every invoke whatever credential it carries.
//!
//! An invoke is not named as an OPERATION, which is how a service asks the router to verify
//! a credential before anything is served, because the router verifies a named operation's
//! SigV4 credential against the signing name of the service that CLAIMED the request, and
//! this service's is its control plane's (`apigateway`). An invoke of a deployed stage is
//! signed under the data plane's own signing name instead — the `execute-api` service AWS
//! publishes the invoke endpoint and the `execute-api:Invoke` action under — so a correctly
//! signed invoke never matches that name and is refused before the stage is reached,
//! whatever the api's records say. What a route's `AuthorizationType` requires of the
//! request is unchanged: `AWS_IAM` still admits only a request that CARRIES a SigV4
//! credential ([`invoke::carries_sigv4_credential`]), and refuses one that carries none.
//!
//! Beside those two there is a THIRD data plane, addressed on the domain every portal this
//! service mints reports (`EndpointConfigurationResponse$PortalDefaultDomainName`):
//!
//! - [`portal`] — an ordinary HTTP request to the website a PUBLISHED portal serves: the
//!   claim ([`portal::claims_parts`]) and the serving ([`portal::serve`], reached from
//!   [`Service::override_routing_error`], since a consumer of a developer portal presents
//!   no AWS credential at all).
//!
//! ## How an answer that carries no content is framed
//!
//! Every operation of this model whose success is `204 No Content` answers an empty output
//! (`smithy.api#Unit`): the deletes, `DisablePortal`, `ResetAuthorizersCache`,
//! `UntagResource`. The recorded AWS answers for those STATE the length such a message
//! has — `content-length: 0` — rather than leaving the framing header off, so a client that
//! closes the message out on an explicit zero length (or a proxy that re-frames it) reads
//! the same message end the origin wrote.
//!
//! [`Service::override_protocol`] below therefore serves a protocol that is the model's own
//! in every respect except one: a `204` this service answers carries no body, whatever a
//! serializer rendered for it, and states `content-length: 0`. A response that carries
//! content is left exactly as the model's protocol rendered it. It is done there, at the
//! one place every modelled response of this service passes through, rather than per
//! operation. The three data planes above frame their own answers, which carry real
//! content, and are not reached through this seam.
//!
//! CAVEAT: a client reached over HTTP/1.1 does not see the header, because the HTTP writer
//! the vendored transport uses removes `content-length` from a `204` (RFC 9110 §15.3.5
//! forbids a body there) — that writer is below anything this crate decides. What this
//! service states is what the in-process front door (the inter-service callback path and
//! the SDK connector, both of which serve `dispatch`'s response unchanged) delivers, and is
//! pinned by `tests/all/empty_answer_framing.rs`.

mod data_plane;
mod invoke;
mod jwt;
mod portal;
mod websocket;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{ProtocolError, ServerProtocol, Violation, ViolationKind};
use smithy_server_runtime::{http, Refusal, Refused, Service};
use std::sync::Arc;

/// The status whose answer carries NO content, whatever a serializer rendered for it, and
/// the length such an answer states. See the module docs.
const NO_CONTENT_STATUS: u16 = 204;
const NO_CONTENT_LENGTH: &str = "0";

/// How an answer that carries no content is framed: with no body at all, and stating the
/// length it does have. See the module docs.
///
/// A response that carries content is untouched — it keeps the body the protocol rendered
/// and the framing that body already has — so this cannot reframe a modelled output or an
/// error envelope. The insert is FALLIBLE and ignored on failure: a decimal length is
/// always a valid header value, so it cannot fail today, and if it ever did the answer
/// would go out unframed rather than as a transport failure.
fn frame_empty_answer(response: &mut RtResponse) {
    if response.status().as_u16() != NO_CONTENT_STATUS {
        return;
    }
    *response.body_mut() = aws_smithy_types::body::SdkBody::empty();
    let _ = response
        .headers_mut()
        .try_insert("content-length", NO_CONTENT_LENGTH);
}

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
        // A routing rule's `Priority`, whose shape declares the order a rule may be
        // evaluated in (`RoutingRulePriority`, range 1..1000000). Stated as the whole
        // range the member has to fall in, however far outside it the value sat.
        ("priority", ViolationKind::MinValue(_) | ViolationKind::MaxValue(_)) => {
            Some(crate::state::routing_rule_priority_range_message())
        }
        // `CreateIntegration$IntegrationType`, the `@required` member that targets the
        // `IntegrationType` enum. A value OUTSIDE that enum names no integration type
        // this service can read, exactly as an absent member does, so both are refused
        // with the one sentence that names the member and what it is required for
        // ([`crate::state::INTEGRATION_TYPE_REQUIRED`]) rather than with the generic
        // constraint clause.
        (
            "integrationType",
            ViolationKind::MissingRequiredMember | ViolationKind::EnumValue(_),
        ) => Some(crate::state::INTEGRATION_TYPE_REQUIRED.to_string()),
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
    /// claim and a routing-error answer, both of which read this service's own records.
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
        }
    }
}

/// The protocol this service answers modelled requests in: the model's own, with the one
/// framing correction the module docs state.
///
/// Implemented on the service itself because the model-derived protocol it delegates to is
/// owned by the generated entry and reached by reference (`self.entry.protocol()`), and
/// [`Service::override_protocol`] hands back a borrow of `&self` — so there is one protocol
/// instance, the generated one, and no second routing table to drift from the model.
impl<Eng: Send + Sync + 'static> ServerProtocol for EmulatedService<Eng> {
    type Request = RtRequest;
    type Response = RtResponse;

    fn protocol_id(&self) -> &aws_smithy_schema::ShapeId {
        self.entry.protocol().protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a RtRequest) -> Result<&'a str, ProtocolError> {
        self.entry.protocol().routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a RtRequest,
        input_schema: &Schema,
    ) -> Result<Box<dyn aws_smithy_schema::serde::ShapeDeserializer + 'a>, ProtocolError> {
        self.entry
            .protocol()
            .deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn aws_smithy_schema::serde::SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        let mut response = self.entry.protocol().serialize_output(
            output,
            output_schema,
            operation_name,
            success_status,
        )?;
        // The status and every other header are the protocol's: only an answer that
        // carries no content is emptied of whatever was rendered for it and framed.
        frame_empty_answer(&mut response);
        Ok(response)
    }

    fn serialize_error(
        &self,
        error: &dyn aws_smithy_schema::serde::SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        let mut response = self
            .entry
            .protocol()
            .serialize_error(error, error_schema, error_code, http_status)?;
        // Every modelled failure of this service carries a JSON error envelope at a 4xx, so
        // nothing here is emptied or reframed; the same call is made on both paths rather
        // than leaving one of them to answer differently.
        frame_empty_answer(&mut response);
        Ok(response)
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn aws_smithy_schema::serde::SerializableStruct,
        event_schema: &Schema,
    ) -> Result<RtResponse, ProtocolError> {
        self.entry.protocol().serialize_event_stream_message(
            event_type,
            is_exception,
            event,
            event_schema,
        )
    }
}

/// `ServerProtocol` requires `Debug`; the generated entry is not `Debug`, so this names the
/// protocol being served rather than the service's internals.
impl<Eng: Send + Sync + 'static> std::fmt::Debug for EmulatedService<Eng> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmulatedService")
            .field("protocol", &self.entry.protocol().protocol_id())
            .finish()
    }
}

// `Service::override_operation` is deliberately NOT overridden here: this service names no
// operation of its own, so the generated routing table resolves every request it serves,
// and an invoke of a deployed stage is left unnamed. Naming an operation is how a service
// asks the router to verify a credential, and the router verifies it against THIS service's
// own signing name, which is its control plane's, while an invoke is signed under the data
// plane's own — so such an invoke is answered by `override_routing_error` below, which the
// router reaches before authentication. The module docs state the whole rule.
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
        websocket::claims_parts(&*self.engine, parts)
            || invoke::claims_parts(&*self.engine, parts)
            || portal::claims_parts(&*self.engine, parts)
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

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every response a modelled operation of this service produces is serialized by the
        // protocol returned here. See the module docs: it is the model's own protocol, with
        // one correction — an answer that carries no content carries no body and states
        // `content-length: 0`, rather than leaving the framing header off.
        Some(self)
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
        // So is an invoke of an HTTP api's stage, whatever credential it carries: reaching
        // it BEFORE authentication is what lets a client holding only a bearer token, or
        // one signing for the data plane's own signing name, be answered at all.
        if websocket::claims_request(&*self.engine, req) {
            return Some(websocket::serve(&*self.engine, req));
        }
        if invoke::claims_request(&*self.engine, req) {
            return Some(invoke::serve(&*self.engine, req));
        }
        // And a visit to the website a published portal serves on its own domain, which
        // likewise names no modelled operation and carries no AWS credential.
        if portal::claims_request(&*self.engine, req) {
            return Some(portal::serve(&*self.engine, req));
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
