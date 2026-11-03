// SPDX-License-Identifier: CC0-1.0

//! How this service states a request the framework refused before any handler ran, and
//! the gate that finds EVERY member such a request left out rather than only the first.
//!
//! The framework hands a refusal over structurally — one [`Violation`] per rule the
//! request broke, carrying the path it sits at, what it failed and what it held — and
//! leaves the words to the service, because no model records them. This module is that
//! wording for every constrained member of every operation of this service, so no
//! operation can state a refusal its own way, and nothing here is keyed to an operation
//! or to a submitted value.
//!
//! The dialect it implements, over the envelope AWS states every constraint refusal in
//! (`N validation error(s) detected: ` over one clause per violated constraint, joined
//! with `; `):
//!
//! * a clause names the offending MEMBER PATH and the CONSTRAINT it broke and states NO
//!   value — `Value at '<path>' failed to satisfy constraint: <requirement>` — neither
//!   the bytes a present member submitted nor the `null` an ABSENT member would otherwise
//!   be stated with: this service's refusals name the member and the requirement, which
//!   is what a caller acts on, and echo nothing back;
//! * a `@pattern` requirement is quoted WITHOUT the `^`/`$` a whole-value pattern is
//!   written with, so a caller that lifts the expression out of the message and
//!   pre-validates its own input compiles the expression the service advertises;
//! * EVERY violated member is stated, in member-path order, and the count the envelope
//!   opens with is the number of clauses that follow.
//!
//! Only the last of those needs more than the framework's own refusal: absence is found
//! while READING a request, and the generated deserializer stops at the first `@required`
//! member it cannot fill (`missing_member` in the generated `deserialize`), so a request
//! that left two members out reaches the refusal seam describing one. [`AbsentMemberGate`]
//! is what closes that: it reads the members the request's body carries BEFORE the
//! operation runs, states every required one of them that carries no value, and hands a
//! request that left none out straight to the generated handler.
//!
//! The envelope, the clause, the member paths and the `@pattern` rendering are
//! [`service_common::validation_message`]'s and the framework's, so nothing about them can
//! drift per crate; this module chooses only that every clause withholds its value half
//! and that the clauses are ordered by member path.

use std::marker::PhantomData;

use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::path;
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, Refusal, Refused, Service, Violation,
    ViolationKind,
};

/// The `@required` input members of each operation of this service, in the spelling a
/// validation path uses, from
/// `smithy_query(query="operation",operation="PutActionInteractions,PutActions,PutEvents,PutItems,PutUsers",model_path="services/service-com-amazonaws-personalizeevents-amazonpersonalizeevents/smithy-model/com-amazonaws-personalizeevents-amazonpersonalizeevents.json")`
/// — the same sets the crate root's generated `OPERATIONS.md` lists as `required`.
///
/// Stated here because `@required` has NO runtime surface to read it off: codegen consumes
/// the trait into the generated input struct's field (a required member is a bare field,
/// an optional one an `Option`), and the member schemas the framework carries hold the
/// CONSTRAINT traits only (`@length`, `@pattern`, …). So the one thing a body check needs
/// — which members a request must carry — is restated, and [`crate::service`]'s own
/// integration suite pins every row of it against the generated deserializer: a body
/// omitting just one listed member is refused naming exactly that member, and a body
/// carrying all of them is never refused for absence.
const REQUIRED_MEMBERS: &[(&str, &[&str])] = &[
    (
        "PutActionInteractions",
        &["actionInteractions", "trackingId"],
    ),
    ("PutActions", &["actions", "datasetArn"]),
    ("PutEvents", &["eventList", "sessionId", "trackingId"]),
    ("PutItems", &["datasetArn", "items"]),
    ("PutUsers", &["datasetArn", "users"]),
];

/// The required members of `operation`, or an empty slice for an operation this service
/// does not serve.
fn required_members(operation: &str) -> &'static [&'static str] {
    REQUIRED_MEMBERS
        .iter()
        .find(|(named, _)| *named == operation)
        .map_or(&[], |(_, members)| *members)
}

/// How this service states `refusal`, or `None` to keep the framework's wording.
///
/// Only an input-constraint refusal is reworded: an authentication refusal is a different
/// failure with a different code and status, and nothing about it is this service's to
/// word.
///
/// The CODE travels with the message and is the shared renderer's: the recorded refusals of
/// this service carry the boundary's own `ValidationException` rather than a modelled error
/// shape, which is also what the framework already answered, so nothing a client branches on
/// changes here — only the words.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    // `withholds_value` is answered for EVERY member path: a clause of this service names
    // the member and the requirement and states no value at all, which is also how an
    // absent member is stated here — so the `null` the framework writes for one is dropped
    // with it.
    service_common::validation_message::aws_validation_refusal_withholding(
        &Refusal::Invalid(in_member_path_order(violations)),
        |_| true,
    )
}

/// `violations` ordered by the member path each is stated at.
///
/// A request can break several rules at once, and the order the clauses are stated in is
/// then part of the message. The framework's walk reports them in the order it reached
/// them — the input shape's member order for a value, and the deserializer's for an absent
/// member — which is a fact about the model rather than about the request; AWS states them
/// ordered by the member they name, so that is the order rendered here.
fn in_member_path_order(violations: &[Violation]) -> Vec<Violation> {
    let mut ordered = violations.to_vec();
    ordered.sort_by(|left, right| left.path.cmp(&right.path));
    ordered
}

/// Every `@required` member of `O` that `req`'s body carries no value for, as the
/// violations a refusal is stated over.
///
/// Empty for a request that carries all of them — including a request whose body is not a
/// JSON object at all, which is a failure to READ the body rather than a member left out,
/// and is the framework's to word.
///
/// A member the model binds to the URI or to a header is not carried in the body, so it is
/// not looked for there ([`carried_in_body`]); every input member of this service is a
/// plain body member, so today the check spans them all.
fn absent_required_members<O: Operation>(
    req: &aws_smithy_runtime_api::http::Request,
) -> Vec<Violation> {
    let required = required_members(O::NAME);
    if required.is_empty() {
        return Vec::new();
    }
    // The body the router buffered for this request, which is what the protocol itself
    // reads: a body it could not hold is one `deserialize_input` already refused.
    let body = req.body().bytes().unwrap_or_default();
    let Some(carried) = carried_members(body) else {
        return Vec::new();
    };
    required
        .iter()
        .filter(|member| !carries_value(&carried, O::INPUT_SCHEMA, member))
        .map(|member| Violation {
            path: path::member(member),
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        })
        .collect()
}

/// The top-level members `body` carries, or `None` when `body` is not an object of them.
///
/// A body of nothing but whitespace — the request a caller sends when it supplies no
/// members at all — is the object that carries none, rather than a body that could not be
/// read: every member is then absent, which is what AWS reports for it.
fn carried_members(body: &[u8]) -> Option<serde_json::Map<String, serde_json::Value>> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Some(serde_json::Map::new());
    }
    match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(serde_json::Value::Object(members)) => Some(members),
        _ => None,
    }
}

/// Whether `carried` holds a value for `member`.
///
/// The key a member is carried under is the model's: its `@jsonName` where one is
/// declared, else the member name, read off `input_schema` so nothing about the wire form
/// is spelled here. A member present as JSON `null` carries no value — the requirement it
/// breaks is that the member must not be null — so it is reported exactly as an omitted
/// one is.
fn carries_value(
    carried: &serde_json::Map<String, serde_json::Value>,
    input_schema: &'static Schema,
    member: &str,
) -> bool {
    let declared = input_schema
        .members()
        .iter()
        .copied()
        .find(|declared| declared.member_name() == Some(member));
    let Some(declared) = declared.filter(|declared| carried_in_body(declared)) else {
        // A member this operation's input does not declare as a body member is not one a
        // body check can answer for; treating it as carried leaves it to the framework.
        return true;
    };
    let key = declared.json_name().map_or(member, |json| json.value());
    carried.get(key).is_some_and(|value| !value.is_null())
}

/// Whether `member` is carried as a key of the request's JSON body object: a member the
/// model binds to the URI, to a header or to the whole payload is not.
fn carried_in_body(member: &Schema) -> bool {
    member.http_label().is_none()
        && member.http_query().is_none()
        && member.http_query_params().is_none()
        && member.http_header().is_none()
        && member.http_prefix_headers().is_none()
        && member.http_payload().is_none()
}

/// The status a constraint refusal is answered with: the one the protocol gives a
/// pre-dispatch input refusal, and the one this model's own client-input error declares
/// (`InvalidInputException`, `@httpError 400`). Restated here only to classify the
/// telemetry row below, which is recorded beside the response rather than read off it.
const REFUSAL_STATUS: u16 = 400;

/// States every `@required` member of `O` the request left out, before `O`'s handler runs,
/// and hands a request that left none out to the generated handler untouched.
///
/// Why it exists: absence is found while READING the request, and the generated
/// deserializer returns at the FIRST required member it cannot fill, so the refusal seam
/// sees one absent member however many the request omitted — and a caller told about one
/// of two missing members fixes one and is refused again. The body is read here instead,
/// where every member the request carries is visible at once.
///
/// Nothing about the WORDS is decided here: the violations it states go through the same
/// [`refused`] this service words every boundary refusal with, and the response is
/// rendered by the service's own pre-dispatch envelope
/// ([`smithy_server_runtime::ErasedServiceDefinition::refusal_response`]), so a refusal
/// stated here and one the framework's own walk states are the same answer.
pub(crate) struct AbsentMemberGate<O> {
    /// The operation whose required members are checked; its `NAME` and `INPUT_SCHEMA` are
    /// all that is used, so no instance of it is held — the generated routing table owns
    /// the one instance built from the shared engine.
    operation: PhantomData<fn() -> O>,
}

impl<O> AbsentMemberGate<O> {
    pub(crate) const fn new() -> Self {
        Self {
            operation: PhantomData,
        }
    }
}

impl<O: Operation> ErasedOperation for AbsentMemberGate<O> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            let started = std::time::Instant::now();
            let absent = absent_required_members::<O>(req);
            if let Some(stated) = refused(&Refusal::Invalid(absent.clone())) {
                tracing::debug!(
                    operation = O::NAME,
                    violations = absent.len(),
                    "refused a request that left a required member out"
                );
                // The row the framework sequence would have recorded for this request, so
                // a refused one is not a hole in the telemetry series.
                record_refusal::<O>(ctx, started.elapsed(), &stated.code);
                return svc
                    .definition()
                    .refusal_response(ProtocolError::Invalid(absent), Some(stated), req);
            }
            // The request carries every required member: the generated handler serves it,
            // which runs the whole framework sequence (deserialize, validate, handle,
            // serialize) plus its telemetry over the same bytes — and words any refusal of
            // a VALUE through this service's own `override_refusal`. Reached through the
            // routing table rather than through `override_operation`, which would route
            // straight back here.
            match svc.definition().routing_table().get(O::NAME) {
                Some(operation) => operation.run(svc, req, ctx).await,
                None => svc.definition().framework_error_response(
                    ProtocolError::UnknownOperation(O::NAME.to_string()),
                    req,
                ),
            }
        })
    }
}

/// Record the refusal above as the operation row the framework sequence would have
/// recorded: the same classification (from the refusal's status and error code), the same
/// folded handle (this service's signing name and the operation's own name), and the same
/// already-sanitized request metadata. Gated on a client collecting, exactly as the
/// framework gates it.
fn record_refusal<O: Operation>(
    ctx: &OperationContext<'_>,
    latency: std::time::Duration,
    code: &str,
) {
    if !emulator_telemetry::recording_active() {
        return;
    }
    let handle =
        emulator_telemetry::OperationHandle::folded(ctx.identity().signing_name_static(), O::NAME);
    emulator_telemetry::record_generated_operation_with_handle(
        &handle,
        latency,
        Some(emulator_telemetry::OperationFailure::classify(
            REFUSAL_STATUS,
            code,
        )),
        smithy_server_runtime::RequestTelemetry::from_headers(&ctx.parts().headers).into_metadata(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: every clause states the member and the requirement and no value, the envelope
    /// counts the clauses, and the clauses are ordered by member path.
    ///
    /// The rows differ along what the rule generalises over: the constraint class (absent
    /// member, pattern, both length bounds), whether the member was absent or carried a
    /// value, and how many members one request broke.
    #[test]
    fn a_clause_names_the_member_and_the_requirement_and_states_no_value() {
        let absent = |member: &str| Violation {
            path: member.to_string(),
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        };
        let outside = |member: &str, kind: ViolationKind, value: &str| Violation {
            path: member.to_string(),
            kind,
            value: Some(value.to_string()),
            sensitive: false,
        };

        let rows: Vec<(&str, Vec<Violation>, &str)> = vec![
            (
                "one absent member",
                vec![absent("trackingId")],
                "1 validation error detected: Value at 'trackingId' failed to satisfy \
                 constraint: Member must not be null",
            ),
            (
                "two absent members, stated in member-path order",
                vec![absent("trackingId"), absent("actionInteractions")],
                "2 validation errors detected: Value at 'actionInteractions' failed to satisfy \
                 constraint: Member must not be null; Value at 'trackingId' failed to satisfy \
                 constraint: Member must not be null",
            ),
            (
                "a value outside its pattern, quoted without the whole-value anchors",
                vec![outside(
                    "datasetArn",
                    ViolationKind::Pattern(
                        r"^arn:([a-z\d-]+):personalize:.*:.*:.+$".to_string(),
                    ),
                    "nope",
                )],
                r"1 validation error detected: Value at 'datasetArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:([a-z\d-]+):personalize:.*:.*:.+",
            ),
            (
                "a value under its length minimum",
                vec![outside("trackingId", ViolationKind::MinLength(1), "")],
                "1 validation error detected: Value at 'trackingId' failed to satisfy \
                 constraint: Member must have length greater than or equal to 1",
            ),
            (
                "a collection over its length maximum",
                vec![outside("items", ViolationKind::MaxLength(10), "11 items")],
                "1 validation error detected: Value at 'items' failed to satisfy constraint: \
                 Member must have length less than or equal to 10",
            ),
            (
                "one absent member and one value out of bounds",
                vec![
                    outside("trackingId", ViolationKind::MinLength(1), ""),
                    absent("actionInteractions"),
                ],
                "2 validation errors detected: Value at 'actionInteractions' failed to satisfy \
                 constraint: Member must not be null; Value at 'trackingId' failed to satisfy \
                 constraint: Member must have length greater than or equal to 1",
            ),
        ];

        for (case, violations, expected) in rows {
            let stated = refused(&Refusal::Invalid(violations)).expect("a refusal is stated");
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }
    }

    /// RULE: only a constraint refusal is reworded, and a refusal with nothing to state
    /// keeps the framework's wording.
    #[test]
    fn nothing_but_a_constraint_refusal_is_reworded() {
        assert!(
            refused(&Refusal::Unauthenticated).is_none(),
            "expected an unauthenticated caller to keep the framework's wording"
        );
        assert!(
            refused(&Refusal::Invalid(Vec::new())).is_none(),
            "expected no violations to state nothing"
        );
    }

    /// RULE: the required members this module states are the ones the model declares, in
    /// the spelling a validation path uses, and every one of them is declared by the input
    /// shape of the operation it is listed under.
    #[test]
    fn every_required_member_is_a_member_of_that_operations_input() {
        // The input schema of each operation, paired here so the catalog above can be
        // checked against the model's own member list.
        let schemas: [(&str, &'static Schema); 5] = [
            (
                "PutActionInteractions",
                crate::ops::types::PUT_ACTION_INTERACTIONS_INPUT_SCHEMA,
            ),
            ("PutActions", crate::ops::types::PUT_ACTIONS_INPUT_SCHEMA),
            ("PutEvents", crate::ops::types::PUT_EVENTS_INPUT_SCHEMA),
            ("PutItems", crate::ops::types::PUT_ITEMS_INPUT_SCHEMA),
            ("PutUsers", crate::ops::types::PUT_USERS_INPUT_SCHEMA),
        ];
        for (operation, schema) in schemas {
            let members = required_members(operation);
            assert!(
                !members.is_empty(),
                "case {operation:?}: expected the catalog to list this operation's required \
                 members, got none"
            );
            for member in members {
                let declared = schema
                    .members()
                    .iter()
                    .any(|declared| declared.member_name() == Some(*member));
                assert!(
                    declared,
                    "case {operation:?}: expected {member:?} to be a member of the input shape, \
                     whose members are {:?}",
                    schema
                        .members()
                        .iter()
                        .map(|declared| declared.member_name())
                        .collect::<Vec<_>>()
                );
                assert_eq!(
                    path::member(member),
                    *member,
                    "case {operation:?}: expected {member:?} to be spelled as a validation path \
                     spells it"
                );
            }
        }
    }
}
