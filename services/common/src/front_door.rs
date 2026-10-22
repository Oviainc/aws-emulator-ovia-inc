// SPDX-License-Identifier: CC0-1.0

//! The FRONT DOOR an AWS REST service puts in front of its application code, as a
//! wrapper any service can serve its operations behind.
//!
//! # What a front door is
//!
//! Several AWS REST services validate a request's JSON BODY against the operation's
//! modelled input at a layer AHEAD of the code that owns the service's modelled
//! errors, and reject it there: HTTP 400 with the error code
//! [`BAD_REQUEST_EXCEPTION`] — a shape those services' models do not declare — and the
//! fixed sentence [`INVALID_REQUEST_BODY`], which names neither the member nor the
//! value. A constraint on a value bound to the request URI (`@httpLabel`,
//! `@httpQuery`) is NOT a body rejection: the service itself refuses those, as the
//! modelled validation error, naming what failed.
//!
//! # Why this cannot live in `Service::override_refusal`
//!
//! The framework refuses every modelled constraint before a handler runs and words
//! that refusal through [`smithy_server_runtime::Service::override_refusal`], which is
//! handed the violations ALONE — no operation, no request — so it cannot tell a body
//! violation from a URI one. Two operations of one service can even bind the same
//! member name both ways (a create carrying an ARN in the body, a read carrying it in
//! the path), so no rule over violation paths alone can place them.
//!
//! [`FrontDoor`] is where that distinction lives: a service serves each modelled
//! operation wrapped in it (through its `override_operation` seam), and the wrapper
//! reads the HTTP BINDING of each violated member off THAT operation's own input
//! schema.
//!
//! # Services still carrying their own copy
//!
//! This rule predates this module and is inlined in the service crates that needed it
//! before there was anywhere shared to put it —
//! `grep -rln 'fn binds_to_body\|fn body_bound\|fn is_bound_to_request' services/*/src/service.rs`
//! lists them (today: `networkmonitor`, `artifact`, `uxc`, `directoryservicedata`).
//! Those copies are to be MIGRATED onto this module; they are left in place only
//! because this change does not own those services' issues, and each is covered by its
//! own suite. Nothing new should inline the rule again — call this module.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::path;
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, Refused, ServerHttpProtocol, Service,
    Violation,
};

/// The error code a front door names for a request body that breaks a modelled
/// constraint. NOT a shape in the serving service's model: the rejection happens ahead
/// of the application code that owns the modelled errors, so it is worded through the
/// framework's refusal channel ([`Refused`]) rather than through an error enum.
pub const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// What a front door says about a body it refused.
///
/// It validates the body against the shape and reports the class of failure, not the
/// member: a caller fixes the request from the constraint the model publishes. (The
/// member-by-member clause the framework writes is what the service's own modelled
/// validation error carries, for the refusals the application layer owns.)
pub const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// One modelled operation, served behind its service's front door.
///
/// Wrap every modelled operation of a service that has one, so the rule is applied by
/// ONE code path rather than per operation: an operation that binds nothing to the body
/// simply never has a body violation to find, which the wrapper reads off its input
/// schema.
///
/// Nothing else is re-implemented. A body this wrapper does not refuse is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle ->
/// serialize, telemetry and every modelled error — including the modelled validation
/// error for a URI-bound value outside its constraint, and for the rules a handler
/// enforces that the model states only in prose.
///
/// The violations come from the framework's own machinery (the protocol's deserializer
/// and `constraints::validate` over the built input), so this cannot find a different
/// set from the walk that follows it. The cost of that is DELIBERATE: a request this
/// wrapper does not refuse is read and walked twice, once here and once in the
/// operation's own sequence, because the framework hands no already-built input to an
/// operation. Two parses of the same bytes, bounded by the body limits the framework
/// already enforces, buy the per-operation placement no other seam can make.
pub struct FrontDoor<O> {
    operation: O,
}

impl<O> FrontDoor<O> {
    /// Wrap one operation, already constructed from the shared engine.
    pub fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for FrontDoor<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The protocol is read, used and dropped inside this one statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the
            // await below would make this future non-`Send`.
            let refused = body_violations::<O>(svc.definition().protocol(), req);
            if !refused.is_empty() {
                return rejection(svc, refused);
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

/// Every modelled constraint `req`'s BODY breaks, for the operation `O`.
///
/// Absence counts: a required BODY member the request omits is found while READING the
/// request rather than while walking the value it built, but it is the same refusal to
/// a caller — the framework's own `refusal_of` folds it into exactly the violation this
/// returns — so a front door rejects it as a body too. Any OTHER codec failure (a body
/// that is not parseable at all, a member of the wrong type) is not a constraint
/// violation, so it is left to the framework's own walk, which refuses it in the
/// service's modelled validation error. That is the conservative reading of the
/// evidence rather than a settled rule: the recordings behind this module cover a body
/// member outside its declared constraints, not a body a codec cannot read at all, so
/// a recording of THAT is what should decide whether a front door words it the same
/// way.
///
/// Empty when the body satisfies the model and when the operation binds nothing to the
/// body.
pub fn body_violations<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Vec<Violation> {
    // The collecting walk is the shared one (`crate::validation`), so a front door cannot
    // find a different violation set from the framework's own; the echo is left as the
    // framework rendered it, because a front door's answer quotes no value. The narrowing
    // below is this module's own rule.
    crate::validation::violations_in::<O>(protocol, req, crate::validation::Echoes::AsRendered)
        .into_iter()
        .filter(|violation| binds_to_body(O::INPUT_SCHEMA, &violation.path))
        .collect()
}

/// Whether `violation_path` names a member `input_schema` carries in the request BODY.
///
/// Read off the member's own HTTP binding traits, so the model decides: a member with
/// no `@httpLabel` / `@httpQuery` / `@httpQueryParams` / `@httpHeader` /
/// `@httpPrefixHeaders` binding travels in the body. (`@httpPayload` is not one of
/// those: a payload member IS the body.) A nested path (`probe.destination`,
/// `probes.1.member.packetSize`, `tags.someKey.member`) is placed by its FIRST segment,
/// the top-level member the whole subtree hangs off.
pub fn binds_to_body(input_schema: &Schema, violation_path: &str) -> bool {
    // `split` always yields at least one item, so this is the whole path when it has
    // no `.` in it.
    let member_name = violation_path.split('.').next().unwrap_or_default();
    input_schema.members().iter().any(|member| {
        member
            .member_name()
            .is_some_and(|name| path::member(name) == member_name)
            && member.http_label().is_none()
            && member.http_query().is_none()
            && member.http_query_params().is_none()
            && member.http_header().is_none()
            && member.http_prefix_headers().is_none()
    })
}

/// A front door's answer to a body it refused: the service's protocol envelope and the
/// status the framework's refusal channel chooses for a constraint failure (400), with
/// [`BAD_REQUEST_EXCEPTION`] and [`INVALID_REQUEST_BODY`] replacing the code and the
/// message together — a client branches on the code, so one without the other is worse
/// than neither.
pub fn rejection(svc: &dyn Service, refused: Vec<Violation>) -> RtResponse {
    svc.definition().refusal_response(
        ProtocolError::Invalid(refused),
        Some(Refused {
            code: BAD_REQUEST_EXCEPTION.to_string(),
            message: INVALID_REQUEST_BODY.to_string(),
        }),
    )
}

#[cfg(test)]
mod tests {
    //! Where a member travels, read off one input schema that binds members every
    //! way a REST protocol can.

    use super::*;
    use aws_smithy_schema::{ShapeId, ShapeType};

    static LABEL: Schema = Schema::new_member(
        ShapeId::from_static("test#Input$resourceArn", "test#Input", "resourceArn"),
        ShapeType::String,
        "resourceArn",
        0,
    )
    .with_http_label();
    static QUERY: Schema = Schema::new_member(
        ShapeId::from_static("test#Input$tagKeys", "test#Input", "tagKeys"),
        ShapeType::List,
        "tagKeys",
        1,
    )
    .with_http_query("tagKeys");
    static HEADER: Schema = Schema::new_member(
        ShapeId::from_static("test#Input$clientToken", "test#Input", "clientToken"),
        ShapeType::String,
        "clientToken",
        2,
    )
    .with_http_header("x-client-token");
    static BODY_SCALAR: Schema = Schema::new_member(
        ShapeId::from_static("test#Input$spendingLimit", "test#Input", "spendingLimit"),
        ShapeType::String,
        "spendingLimit",
        3,
    );
    static BODY_STRUCT: Schema = Schema::new_member(
        ShapeId::from_static("test#Input$timePeriod", "test#Input", "timePeriod"),
        ShapeType::Structure,
        "timePeriod",
        4,
    );
    static MEMBERS: &[&Schema] = &[&LABEL, &QUERY, &HEADER, &BODY_SCALAR, &BODY_STRUCT];
    static INPUT: Schema = Schema::new_struct(
        ShapeId::from_static("test#Input", "test", "Input"),
        ShapeType::Structure,
        MEMBERS,
    );

    /// A violation path is placed by the HTTP binding of the top-level member it hangs
    /// off — for every location a REST protocol binds, at every depth.
    #[test]
    fn a_violation_is_placed_by_its_top_level_members_binding() {
        // (case, violation path, carried in the body)
        let rows: [(&str, &str, bool); 8] = [
            ("a URI label", "resourceArn", false),
            ("a query parameter", "tagKeys", false),
            ("an element of a query list", "tagKeys.1.member", false),
            ("a header", "clientToken", false),
            ("a body member", "spendingLimit", true),
            ("a nested body member", "timePeriod.startAt", true),
            ("a body member's map entry", "timePeriod.someKey.member", true),
            ("a member this shape does not declare", "nobody", false),
        ];

        for (case, violation_path, expected) in rows {
            let actual = binds_to_body(&INPUT, violation_path);
            assert_eq!(
                actual, expected,
                "case {case:?} ({violation_path}): expected binds_to_body == {expected}, \
                 got {actual}"
            );
        }
    }
}
