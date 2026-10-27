// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-neptunegraph-amazonneptunegraph` service: how it claims requests and how it words the
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
//!   correctly, or serve a modeled one through a wrapper. Every request this service
//!   claims resolves through the generated routing table except the three that ask it
//!   to assume one of the caller's IAM roles, which are served through
//!   [`pass_role::PassRoleGate`] so that authorization decision is reached ahead of the
//!   boundary's model-constraint validation.
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

use smithy_server_runtime::{
    http, ErasedOperation, Operation, OperationRoutingTable, Refusal, Refused, Service, Violation,
    ViolationKind,
};
use std::sync::Arc;

/// The authorization boundary in front of every operation that asks this service to
/// assume one of the caller's IAM roles. Public because each such operation implements
/// [`pass_role::PassesRole`] beside its own handler.
pub mod pass_role;

use pass_role::PassRoleGate;

/// The error code this service answers an input-constraint violation with: the modelled
/// `ValidationException` (`smithy_query(query="shape",shape="ValidationException")`:
/// `@error client`, `@httpError 400`), which is also the code the framework would have
/// chosen — restated here because `override_refusal` replaces the code and the message
/// together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// Render one constraint violation the way this service states it: the value that was
/// rejected, the member path it sat at, and the requirement it failed.
///
/// Every requirement but one is worded exactly as the framework words it. The exception
/// is `@pattern`, reported as the expression a value must satisfy WITHOUT the anchors the
/// model spells it with (`crate::state::pattern_requirement`): the anchoring is how the
/// model makes the match total, not part of the expression the service asks a value to
/// satisfy, and a client that lifts the expression out of the message to re-validate
/// input cannot use one wrapped in anchors it never sent.
///
/// `None` for a violation class this service has no wording for, which lets
/// [`constraint_envelope`] decline the whole refusal and keep the framework's wording
/// rather than invent a sentence ([`ViolationKind`] is `#[non_exhaustive]`, so a class can
/// be added upstream).
fn constraint_clause(violation: &Violation) -> Option<String> {
    let requirement = match &violation.kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => crate::state::min_value_requirement(min),
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

/// The `ValidationException` message for a refusal: the count of violated constraints
/// over one clause per violation.
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

/// The `com-amazonaws-neptunegraph-amazonneptunegraph` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The operations whose input carries a role this service has to assume, each the
    /// SAME operation the generated table holds, wrapped in [`PassRoleGate`] so the
    /// pass-role authorization decision is reached ahead of the model-constraint
    /// validation. Keyed by the routing key the protocol resolves — the key the
    /// generated table uses — and built from the SAME shared engine, so a wrapped
    /// operation reads and writes the one store.
    pass_role_gated: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`PassRoleGate`] and keyed by its operation shape name — the routing key
/// `ServerProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! gate_pass_role_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                ::std::boxed::Box::new(PassRoleGate::over(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        // EVERY operation whose input carries a `roleArn` this service must assume
        // (`smithy_query(query="operation",operation="StartImportTask,CreateGraphUsingImportTask,StartExportTask")`),
        // because each of them takes the same authorization decision about the caller
        // and that one member, and each takes it ahead of its own validation. An
        // operation that merely REPORTS the role a stored task was started with takes
        // no such decision and is served through the generated table.
        let mut pass_role_gated = OperationRoutingTable::new();
        gate_pass_role_operations!(
            pass_role_gated,
            engine,
            create_graph_using_import_task::CreateGraphUsingImportTask,
            start_export_task::StartExportTask,
            start_import_task::StartImportTask,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            pass_role_gated,
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
        // resolves, and the operation it names is the one served — wrapped in
        // `PassRoleGate` for the three operations that ask this service to assume one
        // of the caller's roles, so that authorization decision is reached ahead of the
        // model-constraint validation, which no other seam is handed the request to do.
        // Every other key resolves to `None` here and is served by the generated table
        // unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.pass_role_gated.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // This service states a constraint violation through the one builder in
        // `crate::state`, which is also what a handler enforcing a constraint itself
        // reaches, so a refusal found at the boundary and one found in a handler read
        // identically. The only difference from the framework's own wording is that a
        // `@pattern` requirement is reported unanchored — see [`constraint_clause`].
        //
        // The code stays the modelled `ValidationException` (HTTP 400), which is what
        // the framework would have answered; `override_refusal` replaces the code and
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
