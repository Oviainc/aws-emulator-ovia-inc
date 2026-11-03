// SPDX-License-Identifier: CC0-1.0

//! The input-constraint boundary: how a violation of a constraint the MODEL declares —
//! found by the walk in front of a handler rather than by the handler itself — is
//! answered with this service's own `ValidationException`.
//!
//! # The rule
//!
//! A refused input constraint is reported through the service's own modelled
//! `ValidationException`: HTTP 400, the `message` that names the member and the
//! requirement it failed, AND the `reason` member that classifies the refusal
//! (`smithy_query(query="shape",shape="ValidationException")` -> `message` + `reason`).
//! A constraint on a member's value is what `CONSTRAINT_VIOLATION` names among the
//! modelled reasons (`crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`), so that is
//! the value a violated `@required`, `@length`, `@range`, `@pattern` or enum is
//! classified with — whichever side of the request boundary caught it. The whole shape is
//! built by the one builder a handler enforcing a constraint itself reaches
//! ([`crate::state::constraint_violation`]), so neither side can word — or classify — one
//! refusal differently.
//!
//! # Why a gate rather than the service's refusal seam alone
//!
//! The framework's per-request sequence is deserialize -> validate -> handle, and a
//! refusal found in either of the first two steps is worded through
//! [`Service::override_refusal`], whose answer carries a code and a message and NOTHING
//! else ([`smithy_server_runtime::Refused`]) — so no modelled member can travel on it,
//! and that path can never answer more than two thirds of the shape. A wrapper, unlike
//! that seam, is handed the request AND the operation, so it can answer the refusal the
//! way the operation's own error channel does: the modelled shape, serialized by this
//! service's own protocol.
//!
//! Nothing else is re-implemented. The violations are collected by the framework's own
//! machinery — the protocol's own deserializer and `constraints::validate` over the input
//! that was read ([`constraint_violations`]) — so this cannot find a different set from
//! the walk that follows it, and a request it does not refuse is handed to the operation
//! it wraps, whose [`ErasedOperation::run`] owns deserialize -> validate -> handle ->
//! serialize, telemetry and every modelled error. The cost is that such a request is read
//! twice, once here and once in the operation's own sequence, because the framework hands
//! no already-built input to an operation.

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::SerdeError;
use aws_smithy_schema_server::constraints;
use smithy_server_runtime::{
    BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    ServerHttpProtocol, Service, Violation, ViolationKind,
};

use crate::ops::types::{ValidationException, SCHEMA_VALIDATION_EXCEPTION};

/// The HTTP status the modelled `ValidationException` is answered with
/// (`smithy_query(query="shape",shape="ValidationException")`: `@error client`,
/// `@httpError 400`), which is also the status the framework's own refusal channel
/// answers — restated here because serializing a modelled error takes its status as an
/// argument.
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// One operation, served so a violation of its modelled input constraints is answered
/// with the service's own `ValidationException` rather than with the code-and-message
/// envelope the framework's refusal channel can carry.
///
/// Generic over what it wraps, which is the operation as the service otherwise serves it:
/// `crate::service` wires it around the operation built from the shared engine, so the
/// store a request does reach is the one store.
pub struct ConstraintGate<Inner> {
    operation: Inner,
}

impl<Inner> ConstraintGate<Inner> {
    /// Gate `operation` — already constructed from the shared engine.
    pub fn over(operation: Inner) -> Self {
        Self { operation }
    }
}

impl<Inner: Operation + ErasedOperation> ErasedOperation for ConstraintGate<Inner> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The service definition is read, used and dropped inside each of the two
            // statements below: `&dyn ErasedServiceDefinition` is not `Sync`, so holding
            // it across the await would make this future non-`Send`.
            let refused = constraint_violations::<Inner>(svc.definition().protocol(), req);
            if let Some(response) = constraint_refusal::<Inner>(svc, req, &refused) {
                return response;
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// Every modelled constraint `req` breaks, for the operation `O`.
///
/// Both classes the framework refuses before a handler runs are collected: a value
/// outside a declared `@pattern`, `@length`, `@range` or enum, found by walking the input
/// that was read, and a `@required` member ABSENT, found while READING it. To a caller
/// they are one refusal — the framework's own `refusal_of` folds the absent member into
/// exactly the violation built below — so they are one here. Any OTHER codec failure (a
/// body the codec cannot read at all, a member of the wrong type) is not a constraint
/// violation and is left to the framework's own walk, which refuses it in its own
/// envelope.
///
/// The walk itself is the framework's (`constraints::validate`), so this cannot find a
/// different set from the one the operation's own sequence finds.
///
/// Written here rather than taken from `service_common::validation`, as `workspaces`,
/// `networkmonitor` and `uxc` write it in their own `service.rs` too: these are a handful
/// of framework calls, and that module's collecting walk also RESTORES each clause's value
/// echo to the caller's unelided bytes — a different rule from this one, and one this
/// service's refusals do not follow on either side of the boundary, so taking it would
/// change what a clause quotes rather than only which shape carries it.
///
/// Empty when the request satisfies the model.
pub fn constraint_violations<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Vec<Violation> {
    let Ok(mut deserializer) = protocol.deserialize_input(req, O::INPUT_SCHEMA) else {
        return Vec::new();
    };
    let read = <O::Input as DeserializableStruct>::deserialize(&mut *deserializer);
    drop(deserializer);
    match read {
        Ok(input) => constraints::validate(&input, O::INPUT_SCHEMA),
        // `member_name` is already a violation path in the spelling the constraint walk
        // uses, so it places identically to a value violation.
        Err(SerdeError::MissingMember { member_name }) => vec![Violation {
            path: member_name,
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        }],
        Err(_) => Vec::new(),
    }
}

/// The answer this service gives a request that broke the constraints `refused` names:
/// the modelled `ValidationException` (HTTP 400) carrying one clause per violation and
/// the `reason` that classifies them, rendered in this service's own protocol envelope.
///
/// Rendered from the SAME four facts the framework's own error tail renders a modelled
/// failure from — the error shape, its schema, its wire code and its modelled HTTP status
/// — so this is the answer the handler would have given had the violated constraint been
/// one the model cannot express.
///
/// `None` when there is nothing to refuse, and `None` for a violation class this service
/// has no wording for, which leaves the whole refusal to the framework's own walk rather
/// than inventing a sentence for it (see [`super::constraint_clause`]).
///
/// `req` is the request being refused: the framework's own renderer, the fallback when the
/// modelled shape cannot be serialized, reads the protocol it is served in off it.
pub fn constraint_refusal<O: Operation>(
    svc: &dyn Service,
    req: &RtRequest,
    refused: &[Violation],
) -> Option<RtResponse> {
    let clauses = super::constraint_clauses(refused)?;
    let refusal = crate::state::constraint_violation(&clauses);
    // The record the per-operation sequence would have made: this answers ahead of
    // `ErasedOperation::run`, which is where the framework classifies an operation's
    // outcome, so the refusal is noted here instead. Nothing the caller sent is logged —
    // the outcome and how many constraints it broke are what a reader needs.
    ::tracing::info!(
        operation = O::NAME,
        outcome = ValidationException::shape_name(),
        status = VALIDATION_EXCEPTION_STATUS,
        violations = clauses.len(),
        "refused a request that broke a modelled input constraint"
    );
    let protocol = svc.definition().protocol();
    match protocol.serialize_error(
        &refusal,
        &SCHEMA_VALIDATION_EXCEPTION,
        ValidationException::shape_name(),
        VALIDATION_EXCEPTION_STATUS,
    ) {
        Ok(response) => Some(response),
        Err(unserializable) => Some(
            svc.definition()
                .framework_error_response(unserializable, req),
        ),
    }
}
