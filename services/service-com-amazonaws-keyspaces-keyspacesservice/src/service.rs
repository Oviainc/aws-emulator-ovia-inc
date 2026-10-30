// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-keyspaces-keyspacesservice` service: how it claims requests and how it words the
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

use aws_smithy_schema_server::{Violation, ViolationKind};
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-keyspaces-keyspacesservice` emulated service: the generated [`ServiceDefinition`] plus
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
        // Two services sign as `cassandra`: this one (Amazon Keyspaces, whose requests
        // name `KeyspacesService.<Op>` and whose endpoint is `cassandra.<region>.…`)
        // and KeyspacesStreams (`KeyspacesStreams.<Op>`, endpoint
        // `cassandra-streams.<region>.…`). The SigV4 signing name alone therefore
        // cannot tell the two apart, and a request both services claim is answered by
        // neither — the router rejects an ambiguous claim. So decline a request that
        // names the streams service, and keep the signing-name rule for the rest.
        if names_keyspaces_streams(parts) {
            return false;
        }
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
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
        // required member, or a value outside a declared bound) to let this service
        // reword it. Amazon Keyspaces answers such a request as AWS's own boundary
        // validator does — `400 ValidationException` with the
        // `N validation error(s) detected: Value '<value>' at '<member>' failed to
        // satisfy constraint: <requirement>` envelope, and a `@pattern` requirement
        // quoted without the anchors a whole-value pattern wraps it in — which
        // `service_common::validation_message` states, so neither the envelope nor a
        // clause is written here.
        //
        // What is left to this service is the ORDER of the clauses about ONE member; see
        // [`stated_order`].
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        let reported = Refusal::Invalid(stated_order(violations));
        service_common::validation_message::aws_validation_refusal(&reported)
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

/// The violations of a boundary refusal, in the order this service states them as
/// clauses.
///
/// The MEMBERS keep the order the validating walk found them in — the walk visits a given
/// input in one order, so that is a total, deterministic order. What this chooses is the
/// order of the several clauses about ONE member, which is a property of that member: a
/// recorded refusal of a `keyspaceName` that breaks BOTH constraints `KeyspaceName`
/// declares states the `@pattern` clause before the `@length` one, while the recorded
/// refusal of a `tableName` that breaks the identical pair on `TableName` states them in
/// the walk's own order (length, then pattern) — two shapes with the same `@length` and
/// the same `@pattern`, so nothing in the model tells the two orders apart and the order
/// is read off the member rather than normalised across them. A member
/// [`PATTERN_FIRST_MEMBERS`] does not name keeps the walk's order.
///
/// `services/service-com-amazonaws-acmpca-acmprivateca/src/service.rs` orders the clauses
/// of one member the same way, with the `@pattern` clause first for EVERY member, which is
/// what that service's recordings show. The two differ only in that per-member choice, so
/// a third service that needs either would be the point at which the member-grouped
/// ordering moves into `service_common::validation_message` (which already owns the
/// envelope, the clauses and the `@pattern` rendering) with the rank as its caller's
/// argument.
fn stated_order(violations: &[Violation]) -> Vec<Violation> {
    // The paths in the order the walk reported them, each once: a member states its
    // clauses together, where that member's own clauses were first reported.
    let mut members: Vec<&str> = Vec::new();
    for violation in violations {
        if !members.contains(&violation.path.as_str()) {
            members.push(violation.path.as_str());
        }
    }
    let mut ordered = violations.to_vec();
    // Stable, so two clauses of equal rank about one member stay in the walk's order.
    ordered.sort_by_key(|violation| {
        let member = members
            .iter()
            .position(|path| *path == violation.path.as_str())
            .unwrap_or(0);
        (member, clause_rank(violation))
    });
    ordered
}

/// The members whose recorded refusal states the `@pattern` clause before the rest of
/// that member's clauses.
///
/// Every entry is read off a recorded refusal of this service and nothing else, so the
/// list grows only when another member's multi-violation refusal is recorded; a member
/// that is not recorded is not guessed at, and keeps the walk's order.
const PATTERN_FIRST_MEMBERS: [&str; 1] = ["keyspaceName"];

/// Where a clause sits among the clauses about its own member: the `@pattern` clause of a
/// [`PATTERN_FIRST_MEMBERS`] member goes first, and everything else keeps the walk's
/// order.
fn clause_rank(violation: &Violation) -> u8 {
    if !PATTERN_FIRST_MEMBERS.contains(&violation.path.as_str()) {
        return 0;
    }
    match matches!(violation.kind, ViolationKind::Pattern(_)) {
        true => 0,
        false => 1,
    }
}

/// Whether a request names KeyspacesStreams rather than Amazon Keyspaces.
///
/// Both services sign as `cassandra`, so the distinguishing signals are the ones that
/// name the service directly: the `X-Amz-Target` prefix an awsJson request carries
/// (`KeyspacesStreams.GetRecords` -> `keyspacesstreams`), then the endpoint host's
/// first label (`cassandra-streams.<region>.amazonaws.com` -> `cassandra-streams`).
///
/// The streams service holds the mirror image of this helper, in
/// `services/service-com-amazonaws-keyspacesstreams-keyspacesstreams/src/service.rs`,
/// where it CLAIMS what this one declines. The two decide one split, so change them
/// together.
fn names_keyspaces_streams(parts: &http::request::Parts) -> bool {
    /// The `X-Amz-Target` service prefix of KeyspacesStreams, normalized.
    const STREAMS_TARGET_PREFIX: &str = "keyspacesstreams";
    /// KeyspacesStreams's endpoint prefix (its model's `endpointPrefix`).
    const STREAMS_ENDPOINT_PREFIX: &str = "cassandra-streams";

    let named = emulator_server::service_from_target(&parts.headers)
        .or_else(|| emulator_server::service_from_host(&parts.headers))
        .or_else(|| emulator_server::service_from_authority(&parts.uri));
    named.is_some_and(|service| {
        service == STREAMS_TARGET_PREFIX || service == STREAMS_ENDPOINT_PREFIX
    })
}
