// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-redshiftdata-redshiftdata` service: how it claims requests and how it words the
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
//! - `override_operation` — EDITED HERE: the key and the handler stay the
//!   generated table's, but each modelled operation is served behind this
//!   service's front door, which is where a boundary refusal is worded — see
//!   "How a boundary refusal of a Data API request is worded".
//! - `override_refusal` — EDITED HERE: the fall-back for a boundary refusal the
//!   front door could not re-read the request for. See the inline note on that
//!   method.
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
//! # How a boundary refusal of a Data API request is worded
//!
//! A value outside a modelled constraint is refused before any handler runs, in
//! the `ValidationException` envelope AWS states such a refusal with — and that
//! envelope QUOTES the value it rejected, between `Value ` and
//! ` at '<member path>'`. The value is part of the message rather than prose: it
//! is the only place the service reports the offending input back, so it has to
//! be the bytes the caller sent, rendered the way a value of that member's type
//! renders (a string as its own text, a number as its digits, a list as its
//! bracketed elements).
//!
//! Neither half of that is reachable from the `override_refusal` seam, which is
//! handed the violations ALONE: the framework's own violation carries an ELIDED
//! rendering of a long value and only a `<n> items` summary of a collection, so
//! a clause built from it states text no caller submitted. Both are reachable
//! from the request, which is why every modelled operation is served behind
//! [`service_common::front_door::FrontDoorTable`]: the wrapper re-reads the
//! request for the operation it is wrapping and words the refusal from the value
//! the caller really sent. Nothing is re-implemented and no check is repeated in
//! other words — the violations come from the framework's own deserializer and
//! `constraints::validate` through `service_common::validation`, which also owns
//! every bound on what one clause may quote, and the envelope, the clauses, the
//! member paths and the status stay the framework's. Only the VALUE half, and the
//! `@pattern` rendering, are chosen here.

use service_common::validation::{unanchored, SubmittedEcho};
use smithy_server_runtime::{
    http, ErasedOperation, OperationRoutingTable, Refusal, Refused, Service, Violation,
    ViolationKind,
};
use std::sync::Arc;

/// The error code every refusal this file words carries: the
/// `ValidationException` each operation of this model declares for invalid input,
/// which is the code the framework's own refusal channel names too — so only the
/// BODY of the message differs from the default.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// `violation` as a clause of this service's refusal states it: the value the
/// CALLER submitted in the value half, and a `@pattern` quoted without the
/// anchors a model wraps the whole value in.
///
/// An ABSENT member has no value to quote, so its clause keeps the `Value null`
/// the framework writes for one; a value that WAS submitted and has no text
/// rendering keeps the framework's own summary of it, which is the fail-safe
/// direction — a clause states a value the walk could not render rather than one
/// missing part of itself.
fn quoting_the_submitted_value(violation: &Violation, echo: &SubmittedEcho) -> Violation {
    Violation {
        path: violation.path.clone(),
        kind: match &violation.kind {
            ViolationKind::Pattern(pattern) => ViolationKind::Pattern(unanchored(pattern)),
            other => other.clone(),
        },
        value: match echo {
            SubmittedEcho::Rendered(value) => Some(value.clone()),
            SubmittedEcho::Absent => None,
            SubmittedEcho::Unrendered => violation.value.clone(),
        },
        sensitive: violation.sensitive,
    }
}

/// How this service words a request the boundary validator refused: the
/// framework's own envelope over the framework's own clauses, each quoting the
/// value the caller submitted ([`quoting_the_submitted_value`]).
///
/// `None` only when there is no clause to state at all, which leaves the request
/// to the layers behind the door.
fn body_wording(refused: &[(Violation, SubmittedEcho)]) -> Option<Refused> {
    let quoted: Vec<Violation> = refused
        .iter()
        .map(|(violation, echo)| quoting_the_submitted_value(violation, echo))
        .collect();
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_owned(),
        message: ::aws_smithy_schema_server::envelope(&quoted)?,
    })
}

/// The `com-amazonaws-redshiftdata-redshiftdata` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each served behind
    /// this service's front door so a boundary refusal quotes the value the
    /// caller submitted (see "How a boundary refusal of a Data API request is
    /// worded"). Keyed by the routing key the protocol resolves — the key the
    /// generated table uses — and built from the SAME shared engine, so a wrapped
    /// operation reads and writes the one store.
    front_door: OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut front_door =
            ::service_common::front_door::FrontDoorTable::worded(body_wording);
        // EVERY modelled operation, so the rule is applied by ONE code path
        // rather than per operation. Each is keyed by its own modelled name,
        // which `FrontDoorTable` reads off the operation.
        front_door
            .serve(crate::ops::batch_execute_statement::BatchExecuteStatement::new(Arc::clone(
                &engine,
            )))
            .serve(crate::ops::cancel_statement::CancelStatement::new(Arc::clone(&engine)))
            .serve(crate::ops::describe_statement::DescribeStatement::new(Arc::clone(&engine)))
            .serve(crate::ops::describe_table::DescribeTable::new(Arc::clone(&engine)))
            .serve(crate::ops::execute_statement::ExecuteStatement::new(Arc::clone(&engine)))
            .serve(crate::ops::get_statement_result::GetStatementResult::new(Arc::clone(&engine)))
            .serve(crate::ops::get_statement_result_v2::GetStatementResultV2::new(Arc::clone(
                &engine,
            )))
            .serve(crate::ops::list_databases::ListDatabases::new(Arc::clone(&engine)))
            .serve(crate::ops::list_schemas::ListSchemas::new(Arc::clone(&engine)))
            .serve(crate::ops::list_sessions::ListSessions::new(Arc::clone(&engine)))
            .serve(crate::ops::list_statements::ListStatements::new(Arc::clone(&engine)))
            .serve(crate::ops::list_tables::ListTables::new(Arc::clone(&engine)));
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            front_door: front_door.finish(),
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
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in the
        // front door so a boundary refusal quotes the value the caller submitted.
        // A key the model does not name resolves to `None` here exactly as it
        // does in the generated table, so the router's routing-error path is
        // unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.front_door.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The front door answers every boundary refusal of a request it could
        // re-read, so this seam is the fall-back for the ones it could not — a
        // body the codec itself rejected, which reports no constraint violation
        // to re-word. AWS states it as the `ValidationException` envelope the
        // framework already renders, except that it quotes a `@pattern`
        // requirement WITHOUT the `^`/`$` anchors the Smithy pattern wraps the
        // whole value in. That wording is shared by every service on a JSON
        // protocol, so it lives in `service-common`; the check itself still uses
        // the model's pattern as written.
        ::service_common::validation_message::aws_validation_refusal(refusal)
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
