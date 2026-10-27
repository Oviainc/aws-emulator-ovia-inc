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
//!
//! # Two wordings
//!
//! Most front doors answer the one fixed sentence above. Some report, in brackets, the
//! CLAUSE their body-schema validator states for each constraint class it found — the
//! regex a value does not match, the length bound a string broke. That is the only thing
//! a service chooses here: [`FrontDoor::worded`] takes a [`BodyWording`] built from the
//! sentences in [`crate::validation_message`], so the sentences themselves still cannot
//! drift per crate.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::path;
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, Refused, ServerHttpProtocol, Service,
    Violation,
};

use crate::validation::SubmittedEcho;

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

/// How a front door words a body it refused, for a service whose recordings show the
/// body-schema validator's CLAUSES rather than the fixed sentence alone.
///
/// It is handed every violation the request's BODY broke, each with the value the caller
/// submitted ([`SubmittedEcho`]) — the framework's own echo of a long value is elided, so
/// a clause that quotes one needs this — and answers the code and the message together,
/// or `None` to leave the request to the layers behind the door.
pub type BodyWording = fn(&[(Violation, SubmittedEcho)]) -> Option<Refused>;

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
    /// How this service's front door words a refused body, or `None` for the fixed
    /// sentence [`INVALID_REQUEST_BODY`].
    wording: Option<BodyWording>,
}

impl<O> FrontDoor<O> {
    /// Wrap one operation, already constructed from the shared engine. Its front door
    /// answers the fixed sentence.
    pub fn new(operation: O) -> Self {
        Self {
            operation,
            wording: None,
        }
    }

    /// Wrap one operation whose front door states `wording`'s clauses instead of the
    /// fixed sentence.
    pub fn worded(operation: O, wording: BodyWording) -> Self {
        Self {
            operation,
            wording: Some(wording),
        }
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
            // The door's own answer is computed in one call that reads, uses and drops
            // the protocol: `&dyn ErasedServiceDefinition` is not `Sync`, so holding it
            // across the await below would make this future non-`Send`.
            let refused = self.refusal(svc, req);
            if let Some(response) = refused {
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

impl<O: Operation> FrontDoor<O> {
    /// This door's answer to `req`, or `None` when its body breaks nothing the door
    /// refuses — in which case the operation behind it is served.
    ///
    /// The protocol is read, used and dropped inside this one function, so nothing
    /// borrowed from the service definition is held across the caller's await.
    fn refusal(&self, svc: &dyn Service, req: &RtRequest) -> Option<RtResponse> {
        let protocol = svc.definition().protocol();
        match self.wording {
            None => {
                let refused = body_violations::<O>(protocol, req);
                (!refused.is_empty()).then(|| rejection(svc, refused))
            }
            Some(wording) => {
                let refused = body_violations_with_echoes::<O>(protocol, req);
                let chosen = wording(&refused)?;
                let violations = refused.into_iter().map(|(violation, _)| violation).collect();
                Some(worded_rejection(svc, violations, chosen))
            }
        }
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
    crate::validation::violations_in::<O>(protocol, req, crate::validation::Echoes::Rendered)
        .violations
        .into_iter()
        .filter(|violation| binds_to_body(O::INPUT_SCHEMA, &violation.path))
        .collect()
}

/// As [`body_violations`], with each violation paired with how a clause can state the
/// value it is about ([`SubmittedEcho`]).
///
/// For a front door that QUOTES the submitted value: the framework's own echo elides a
/// long one, so a clause built from it would state text the caller never sent. The
/// violation set is the same one [`body_violations`] finds — the same collecting walk,
/// narrowed by the same binding rule.
pub fn body_violations_with_echoes<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Vec<(Violation, SubmittedEcho)> {
    crate::validation::refusing_violations_with_echoes::<O>(protocol, req)
        .into_iter()
        .filter(|(violation, _)| binds_to_body(O::INPUT_SCHEMA, &violation.path))
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
    worded_rejection(
        svc,
        refused,
        Refused {
            code: BAD_REQUEST_EXCEPTION.to_string(),
            message: INVALID_REQUEST_BODY.to_string(),
        },
    )
}

/// As [`rejection`], with the code and message a service's own [`BodyWording`] chose.
///
/// The envelope and the status stay the framework's — a constraint failure is a 400 in
/// every protocol — so a service's wording decides the words and nothing else.
pub fn worded_rejection(
    svc: &dyn Service,
    refused: Vec<Violation>,
    chosen: Refused,
) -> RtResponse {
    svc.definition()
        .refusal_response(ProtocolError::Invalid(refused), Some(chosen))
}

/// The routing table a service serves its operations behind its front door through:
/// every operation wrapped in [`FrontDoor`], keyed by the routing key the protocol
/// resolves.
///
/// A service builds one in its `EmulatedService::new` and returns it from
/// `override_operation`, which is the whole wiring: [`serve`](Self::serve) takes one
/// already-constructed operation, wraps it and keys it by the operation's own
/// [`Operation::NAME`] — the model-derived shape name the generated table is keyed by, so
/// a wrapped operation cannot be filed under a key the protocol never resolves. Every
/// service that had this wiring wrote the same wrapping loop by hand; this is that loop,
/// once.
pub struct FrontDoorTable {
    table: smithy_server_runtime::OperationRoutingTable,
    wording: Option<BodyWording>,
}

impl FrontDoorTable {
    /// An empty table whose doors answer the fixed sentence [`INVALID_REQUEST_BODY`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            table: smithy_server_runtime::OperationRoutingTable::new(),
            wording: None,
        }
    }

    /// An empty table whose doors state `wording`'s clauses instead.
    #[must_use]
    pub fn worded(wording: BodyWording) -> Self {
        Self {
            table: smithy_server_runtime::OperationRoutingTable::new(),
            wording: Some(wording),
        }
    }

    /// Serve `operation` behind this table's front door, under its modelled name.
    pub fn serve<O: Operation + ErasedOperation + 'static>(&mut self, operation: O) -> &mut Self {
        let door = match self.wording {
            Some(wording) => FrontDoor::worded(operation, wording),
            None => FrontDoor::new(operation),
        };
        self.table.add_operation(O::NAME, Box::new(door));
        self
    }

    /// The table, for the service to hold and answer `override_operation` from.
    #[must_use]
    pub fn finish(self) -> smithy_server_runtime::OperationRoutingTable {
        self.table
    }
}

impl Default for FrontDoorTable {
    fn default() -> Self {
        Self::new()
    }
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
