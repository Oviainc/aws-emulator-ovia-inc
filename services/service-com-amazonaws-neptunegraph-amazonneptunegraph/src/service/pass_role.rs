// SPDX-License-Identifier: CC0-1.0

//! The pass-role authorization boundary: how an operation that asks this service to
//! ASSUME one of the caller's IAM roles settles whether the caller may hand it over,
//! ahead of everything else the request is checked for.
//!
//! # The rule
//!
//! Passing a role is a decision about the caller and that ONE member, so it is taken
//! first: a value the caller may not pass is refused with the service's denial shape
//! `AccessDeniedException` (HTTP 403, `smithy_query(query="shape",
//! shape="AccessDeniedException")`) whatever else the request says, and no record of
//! the call reaches the store. The decision itself — which roles are passable, and the
//! sentence each refusal carries — lives in [`crate::state::pass_role_denial`], so the
//! gate here and the handlers behind it cannot answer one role two ways.
//!
//! # Why a gate rather than the handler alone
//!
//! The framework's per-request sequence is deserialize -> validate -> handle, so a
//! handler never sees a request that breaks a modelled constraint: the `@pattern` on a
//! pass-role member would answer a value that names no role 400 `ValidationException`
//! — "fix your ARN" — where the authorization boundary says 403, and a client that
//! re-prompts for credentials on a 403 and treats a 400 as a permanent bad-input bug
//! diagnoses an authorization failure as a malformed request. [`PassRoleGate`] takes
//! the decision on the way IN, before that walk, and answers it.
//!
//! Nothing else is re-implemented. A request it does not refuse is handed on: first to
//! the constraint refusal every other operation of this service is served behind
//! ([`super::constraints`]), which is the step the decision above comes before, and then
//! to the operation it wraps, whose [`ErasedOperation::run`] owns deserialize ->
//! validate -> handle -> serialize, telemetry and every modelled error. The cost is that
//! such a request is read twice, once here and once in the operation's own sequence,
//! because the framework hands no already-built input to an operation.

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{
    BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext, OperationError,
    Service,
};

use crate::ops::types::AccessDeniedException;

/// One operation whose input names an IAM role this service is asked to pass.
///
/// The two things the gate cannot know generically: WHERE the role sits in that
/// operation's input, and how that operation's error channel carries the service's
/// denial shape. Implemented beside each such handler, so the operation's own file
/// stays the one place its input and its errors are named.
pub trait PassesRole: Operation {
    /// The role this request asks the service to pass, exactly as it was sent — the
    /// gate runs before the boundary's constraint walk, so the value has no form yet.
    fn passed_role(input: &Self::Input) -> &str;

    /// `denial` lifted into this operation's error channel, which is what fixes the
    /// wire code and the modelled 403 status the gate answers with.
    fn pass_role_denied(denial: AccessDeniedException) -> Self::Error;
}

/// One pass-role operation, served so the authorization decision on its role is
/// reached before the request is checked for anything else.
///
/// Generic over what it wraps, which is the operation as the service otherwise serves
/// it: `crate::service` wires it around the operation built from the shared engine, so
/// the store a request does reach is the one store.
pub struct PassRoleGate<Inner> {
    operation: Inner,
}

impl<Inner> PassRoleGate<Inner> {
    /// Gate `operation` — already constructed from the shared engine, and behind
    /// whatever else the service puts in front of its handler.
    pub fn over(operation: Inner) -> Self {
        Self { operation }
    }
}

impl<Inner: PassesRole + ErasedOperation> ErasedOperation for PassRoleGate<Inner> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The service definition is read, used and dropped inside this one
            // statement: `&dyn ErasedServiceDefinition` is not `Sync`, so holding it
            // across the await below would make this future non-`Send`.
            if let Some(denied) = denial_response::<Inner>(svc, req, ctx) {
                return denied;
            }
            // The refusal `super::constraints::ConstraintGate` answers for every other
            // operation of this service, answered here for the same reason that gate
            // exists — the modelled `ValidationException` carries a `reason` the
            // framework's refusal channel cannot — and reached only AFTER the
            // authorization decision above, which is the one thing that comes first.
            let refused =
                super::constraints::constraint_violations::<Inner>(svc.definition().protocol(), req);
            if let Some(response) =
                super::constraints::constraint_refusal::<Inner>(svc, req, &refused)
            {
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

/// The 403 this service answers a request naming a role the caller may not pass, or
/// `None` when there is no such decision to take.
///
/// Rendered from the SAME four facts the framework's own error tail renders a modelled
/// failure from — the error shape, its schema, its wire code and its modelled HTTP
/// status — so this is the answer the handler would have given had the constraint walk
/// not refused the request first.
///
/// A request that does not READ as this operation's input names no role: a body no
/// codec can read, or one missing a required member, is left to the framework's own
/// refusal. Once a role IS read the decision is taken and ANSWERED — a denial that
/// cannot be serialized renders through the service's own framework-error envelope
/// rather than falling through to the walk whose 400 this exists to reclassify.
fn denial_response<O: PassesRole>(
    svc: &dyn Service,
    req: &RtRequest,
    ctx: &OperationContext<'_>,
) -> Option<RtResponse> {
    let protocol = svc.definition().protocol();
    let mut deserializer = protocol.deserialize_input(req, O::INPUT_SCHEMA).ok()?;
    let read = <O::Input as DeserializableStruct>::deserialize(&mut *deserializer);
    drop(deserializer);
    let input = read.ok()?;
    let denial = crate::state::pass_role_denial(ctx, O::passed_role(&input))?;
    let error = O::pass_role_denied(denial);
    // The record the per-operation sequence would have made: the gate answers ahead of
    // `ErasedOperation::run`, which is where the framework classifies an operation's
    // outcome, so the denial is noted here instead. Nothing the caller sent is logged —
    // the outcome is what a reader needs.
    ::tracing::info!(
        operation = O::NAME,
        outcome = error.shape_name(),
        status = error.http_status(),
        "refused a call naming a role the caller may not pass"
    );
    match protocol.serialize_error(
        error.as_serializable(),
        error.variant_schema(),
        error.shape_name(),
        error.http_status(),
    ) {
        Ok(response) => Some(response),
        Err(unserializable) => Some(
            svc.definition()
                .framework_error_response(unserializable, req),
        ),
    }
}
