// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-eventbridge-awsevents` service: how it claims requests and how it words the
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
//!   before any handler ran. This service words one here: the constraint clauses
//!   the framework states, with a `@pattern` requirement reported without the
//!   anchors the model spells it with. See the inline note on that method.
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

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code this service answers an input-constraint violation with: the
/// `ValidationException` the framework itself would have chosen for this
/// `awsJson1_1` service — restated here because `override_refusal` replaces the
/// code and the message together, so rewording one without the other would
/// change which code a client branches on.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// Render one constraint violation the way this service states it: the value
/// that was rejected, the member path it sat at, and the requirement it failed.
///
/// Every requirement but one is worded exactly as the framework words it. The
/// exception is `@pattern`, reported as the expression a value must satisfy
/// WITHOUT the anchors the model spells it with
/// (`crate::state::pattern_requirement`): the anchoring is how the model makes
/// the match total, not part of the expression the service asks a value to
/// satisfy, and a client that lifts the expression out of the message to
/// re-validate its input cannot use one wrapped in anchors it never sent.
///
/// `None` for a violation class this service has no wording for, which lets
/// [`constraint_envelope`] decline the whole refusal and keep the framework's
/// wording rather than invent a sentence ([`ViolationKind`] is
/// `#[non_exhaustive]`, so a class can be added upstream).
fn constraint_clause(violation: &Violation) -> Option<String> {
    let requirement = match &violation.kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        ViolationKind::Pattern(pattern) => crate::state::pattern_requirement(pattern),
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    };
    // An absent member carries no value, and is reported as the null it was.
    let value = match &violation.kind {
        ViolationKind::MissingRequiredMember => None,
        _ => violation.value.as_deref(),
    };
    Some(crate::state::constraint_clause(
        &violation.path,
        value,
        &requirement,
    ))
}

/// The `ValidationException` message for a refusal: the count of violated
/// constraints over one clause per violation.
///
/// `None` declines the whole refusal — see [`constraint_clause`].
fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations
        .iter()
        .map(constraint_clause)
        .collect::<Option<Vec<_>>>()?;
    if clauses.is_empty() {
        return None;
    }
    Some(crate::state::validation_envelope(&clauses))
}

/// The first URI path segment EVERY request to this service carries: the ROOT path.
///
/// This model is `awsJson1_1` and binds no `smithy.api#http` trait to any operation
/// (`smithy_query(query="digest",
/// model_path="services/service-com-amazonaws-eventbridge-awsevents/smithy-model/\
/// com-amazonaws-eventbridge-awsevents.json")` reports protocol `awsJson1_1` and no
/// `http` binding), so every EventBridge request is `POST /` with
/// `X-Amz-Target: AWSEvents.<Operation>` — the crate's `OPERATIONS.md`, "Wire
/// protocol". `emulator_server::ROOT_PATH_SEGMENT` is the framework's sentinel for
/// exactly that path.
///
/// It is claimed EXPLICITLY, rather than through the generated `ROUTE_PATH_SEGMENTS`
/// (EMPTY for a service with no http bindings, which claims EVERY path under the
/// signing name): the `events` signing name is shared with the EventBridge V2
/// endpoint, whose `rpcv2Cbor` requests arrive at
/// `POST /service/AWSEventsV2/operation/<Operation>`. Claiming every path would make
/// both services claim that request and the router would answer `500
/// AmbiguousService` — a code that exists in no AWS model — instead of serving it.
const OWN_PATH_SEGMENTS: &[&str] = &[emulator_server::ROOT_PATH_SEGMENT];

/// Whether the request these `parts` came from matches this service's SigV4 signing
/// name AND the one URI path its wire protocol publishes ([`OWN_PATH_SEGMENTS`]).
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable
/// owner of that rule, and it is CALLED here rather than re-implemented: the request's
/// route key must match this service's signing name AND its first URI path segment must
/// be one of the segments passed, by exact segment equality.
///
/// The helper takes a whole `http::Request` while [`Service::can_handle`] is handed only
/// the request's parts, so the two things it reads — the URI and the headers — are
/// re-presented to it as a bodiless request. It reads neither the body nor the method, so
/// nothing it decides on is missing.
fn claims_route_key_and_root_path(parts: &http::request::Parts) -> bool {
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
        OWN_PATH_SEGMENTS,
    )
}

/// The `com-amazonaws-eventbridge-awsevents` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
        }
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
        // This service signs as `events` — a signing name it SHARES with the
        // EventBridge V2 endpoint (`services/service-com-amazonaws-eventbridgev2-\
        // awseventsv2`, `aws.auth#sigv4` name `events`, endpointPrefix `eventsv2`) and
        // with the LEGACY CloudWatch Events model of this very service. Claiming on the
        // signing name alone makes several services claim one request, which the router
        // answers `500 AmbiguousService` instead of serving any of them.
        //
        // So the claim is narrowed to this service's own request space: the route key
        // AND the single URI path its `awsJson1_1` wire protocol publishes (`POST /`),
        // leaving the V2 endpoint's `rpcv2Cbor` path space to that service. The legacy
        // CloudWatch Events crate — same shape, version, protocol, endpoint prefix and
        // signing name, a strict SUBSET of this model's operations — claims nothing, so
        // this service owns the `events` key deterministically.
        claims_route_key_and_root_path(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        let _ = req;
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // This service states a constraint violation through the one builder in
        // `crate::state`, which is also what a handler enforcing an unmodelled
        // constraint reaches, so a refusal found at the boundary and one found in a
        // handler read identically. The only difference from the framework's own
        // wording is that a `@pattern` requirement is reported unanchored — see
        // [`constraint_clause`].
        //
        // The code stays `ValidationException` (HTTP 400), which is what the
        // framework would have answered; `override_refusal` replaces the code and
        // the message together, so it is restated rather than changed.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: constraint_envelope(violations)?,
            }),
            _ => None,
        }
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
