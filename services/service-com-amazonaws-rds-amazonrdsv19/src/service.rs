// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-rds-amazonrdsv19` service: how it claims requests and how it words the
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
//! Three of those seams are taken here.
//!
//! `override_refusal` names the error code this front door answers a value that broke a
//! constraint the model declares with — see [`invalid_parameter_value`]. The framework's
//! own code for that class is not one this service's Query API publishes, and the code is
//! what a client branches on.
//!
//! `override_operation` refuses a request whose Query envelope asks for an API version
//! this service does not serve — see [`api_version`]. The framework's `awsQuery` routing
//! key reads the `Action` field alone, so without it a request for `Version=2099-01-01`
//! would be served as though it had asked for `2014-10-31`. Every other routable request
//! is served through the envelope wrapper, and `definition` hands back the wrapped
//! definition, both for the one reason named in [`query_error_envelope`]: an awsQuery
//! error response carries a `<RequestId>` element in its `<ErrorResponse>` envelope,
//! which the framework's error renderers omit. Everything else about those paths —
//! status, `<Type>`, `<Code>`, `<Message>` — stays with the framework.
//!
//! That same wrapper is where this front door's AUDIT leg hangs: it serves each request
//! through the operation wrapped in the shared management-event recorder
//! ([`management_events`]), so every call this service serves is recorded in the account's
//! event history where CloudTrail's `LookupEvents` reports it — one code path for every
//! operation rather than a recording written into each handler.
//!
//! `can_handle` keeps the generated `rds` key. THREE mounted models resolve to it — this
//! one, the Amazon DocumentDB view and the Amazon Neptune view — because all three
//! publish one and the same wire identity (service shape `AmazonRDSv19`, version
//! `2014-10-31`, `awsQuery`, endpointPrefix `rds`, sigv4 name `rds`, xmlNamespace
//! `http://rds.amazonaws.com/doc/2014-10-31/`), differing only in `sdkId`; the shared key
//! is this service's own identity, and the two subset models (DocumentDB's 55 and
//! Neptune's 70 operations are each a strict subset of this model's 168) claim their own
//! routing keys instead. That is also what AWS does: one RDS Query front door serves all
//! three control planes, which is why their models share its endpoint prefix.

pub mod api_version;
pub mod management_events;
pub mod query_error_envelope;
pub mod unresolved_reference;

use api_version::UnsupportedApiVersionOperation;
use aws_smithy_schema_server::{envelope, Violation, ViolationKind};
use query_error_envelope::{RdsDefinition, RequestIdEnvelopeOperation};
use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The Query API error code this service answers a parameter value its member does not
/// accept with.
///
/// It is one of the common client errors the Query API publishes — `InvalidParameterValue`
/// for a value that is not valid for the parameter it was given for — and the code every
/// handler of this crate already refuses such a value under
/// (`crate::scaffolded::Scaffolded::invalid_parameter_value`). The framework picks a
/// generic validation code for the refusals it raises itself, which is no code this
/// service publishes, so a client branching on the documented one never fires.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// Whether `violation` reports a member that was ABSENT rather than a value that was
/// present and unacceptable.
///
/// The two are different refusals to this service: a value outside what its member accepts
/// is [`INVALID_PARAMETER_VALUE`], while a required parameter missing from the request is
/// the Query API's own missing-parameter error, with its own text — so absence is left as
/// the framework states it rather than restated under a code that does not fit it.
fn is_absent_member(violation: &Violation) -> bool {
    matches!(violation.kind, ViolationKind::MissingRequiredMember)
}

/// A refusal of a value that broke a constraint the MODEL declares, restated under the
/// error code this service's Query API publishes for it, or `None` when the refusal is not
/// of that class and the framework's own wording stands.
///
/// Only the CODE changes. The message is the framework's, built from the member path and
/// the requirement the violation carries (`1 validation error detected: Value '<value>' at
/// '<member>' failed to satisfy constraint: <requirement>`), which is the text AWS answers
/// a constraint violation with — so there is nothing to reword and the two cannot drift.
///
/// The class is every constraint a member can declare — a numeric range, a length, a
/// pattern, an enumerated value set, uniqueness — because one code covers them all: each
/// is a value its parameter does not accept. That makes one refusal of one class whichever
/// layer finds it: the operations whose page size is a modelled `@range` are now refused
/// exactly as the ones that carry the same bound in prose and check it in the handler.
///
/// A refusal carrying an absent required member ([`is_absent_member`]) is declined whole:
/// restating it here would answer a missing parameter under a code that reports a bad
/// value, and a bundle mixing the two cannot be answered under one code at all.
fn invalid_parameter_value(violations: &[Violation]) -> Option<Refused> {
    if violations.iter().any(is_absent_member) {
        return None;
    }
    Some(Refused {
        code: INVALID_PARAMETER_VALUE.to_string(),
        message: envelope(violations)?,
    })
}

/// The `com-amazonaws-rds-amazonrdsv19` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: RdsDefinition<Eng>,
    /// The refusal served for a Query envelope naming another API version. Held in a
    /// field, built once, because `override_operation` hands back a reference to it.
    unsupported_api_version: UnsupportedApiVersionOperation,
    /// The envelope wrapper every other routable request is served through. Held in a
    /// field for the same reason.
    envelope: RequestIdEnvelopeOperation<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            // Every modelled operation, each constructed from this same shared engine and
            // served behind the audit recorder, so each call this front door serves is
            // recorded as a management event of `rds.amazonaws.com` (see
            // [`management_events`]). The envelope wrapper routes to it first and to the
            // generated table for anything it does not hold, which is what keeps the
            // `<RequestId>` element on every error response either way.
            envelope: RequestIdEnvelopeOperation::new(
                std::sync::Arc::clone(&engine),
                management_events::audited_table(&engine),
            ),
            entry: RdsDefinition::new(engine),
            unsupported_api_version: UnsupportedApiVersionOperation,
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        //
        // It is handed back wrapped in `RdsDefinition`, which adds the `<RequestId>`
        // an awsQuery `<ErrorResponse>` carries to the framework-level error renderers
        // (a routing failure, a transport failure, a pre-dispatch refusal) without
        // restating anything the model owns.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing name,
        // `rds`.
        //
        // That key is SHARED: Amazon DocumentDB
        // (`services/service-com-amazonaws-docdb-amazonrdsv19`) and Amazon Neptune
        // (`services/service-com-amazonaws-neptune-amazonrdsv19`) are served by this
        // same RDS front door and publish the identical wire identity — service shape
        // `AmazonRDSv19`, version `2014-10-31`, `awsQuery`, `endpointPrefix` /
        // `arnNamespace` / `aws.auth#sigv4` name `rds` — so no signal in a request can
        // tell the three apart and the key must resolve to exactly ONE of them. THIS
        // service owns it: its 168 operations are a superset of DocumentDB's 55 and
        // Neptune's 70 (every one of their operation names is in this model), so a
        // client signing for `rds` loses no operation. Those two crates claim their own
        // sdkId-derived keys (`docdb`, `neptune`) instead of `rds`, which is what keeps
        // this claim unambiguous; before that, all three claimed `rds` and the router
        // answered `500 AmbiguousService` — a code that exists in no AWS model, in a
        // JSON body an awsQuery client cannot parse — for every RDS request. The claim
        // table is `core/router/tests/all/rds_docdb_neptune_signing_name_routing.rs`.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // The ONE routing decision the model's key cannot make: an `awsQuery` envelope
        // names an action AND the API version it is asked for, and the framework's
        // routing key reads only the action. A request naming a version this service
        // does not serve is refused here rather than served as if it had named this
        // one.
        if !api_version::serves_requested_version(req) {
            return Some(&self.unsupported_api_version);
        }
        // Every request the generated routing table can serve is served through the
        // envelope wrapper, which delegates to the audited operation for that same key
        // (and to the generated table entry for a key the audit table does not hold) and
        // then adds the `<RequestId>` the awsQuery error envelope carries (see
        // [`query_error_envelope`]). The routing stays the model's: a request whose
        // `Action` names no modelled operation is declined here, so the framework answers
        // it as the unroutable request it is — through the wrapped definition, which adds
        // the same element to that refusal.
        self.entry.routes(req).then_some(&self.envelope as _)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service state
        // it the way this service states it. The one class reworded here is a value that
        // broke a constraint the model declares, which carries this service's own
        // `InvalidParameterValue` code and the framework's message unchanged (see
        // [`invalid_parameter_value`]); every other refusal keeps the framework's wording.
        match refusal {
            Refusal::Invalid(violations) => invalid_parameter_value(violations),
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
