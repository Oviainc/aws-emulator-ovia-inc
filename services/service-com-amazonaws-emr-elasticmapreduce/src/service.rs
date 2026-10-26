// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-emr-elasticmapreduce` service: how it claims requests and how it words the
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
//! - `override_operation` — EDITED HERE. It still serves the operation the generated
//!   routing table names; what it adds is the OPERATION IDENTITY the refusal seam
//!   below needs — see "Which layer a refusal belongs to".
//! - `override_refusal` — EDITED HERE. A request refused for violating a modelled enum
//!   is answered by the layer that reached the offending value — see "Which layer a
//!   refusal belongs to".
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
//! # Which layer a refusal belongs to
//!
//! A value outside a modelled enum is refused by one of two layers, and BOTH the error
//! code and the message come from the layer that reached it.
//!
//! Amazon EMR checks the enum-valued members of an operation's own INPUT SHAPE itself,
//! ahead of the generic request validator, and answers its own modelled client-input
//! error [`INVALID_REQUEST_EXCEPTION`] (HTTP 400) — the identity an SDK client branching
//! on `emr.exceptions.InvalidRequestException` catches. A value carried deeper, inside a
//! nested request structure, is reached only by the generic validator, which keeps its
//! [`VALIDATION_EXCEPTION`]; `structure_steps` is that distinction, and it is what the
//! recorded `PutAutoScalingPolicy` refusal of a nested comparison operator answers.
//!
//! The code is also a per-OPERATION fact, because the error SET is: the legacy job-flow
//! and the policy put/get/remove families declare no client error at all
//! ([`NO_INVALID_REQUEST_EXCEPTION`]), so naming EMR's own error on one of them would
//! answer with a shape the operation cannot return. The framework refuses a request
//! BEFORE any handler runs and hands [`Service::override_refusal`] the refusal alone,
//! with no request and no operation; [`OperationScopedRefusal`] is how the operation
//! reaches it, running the real operation inside a task-local scope holding that fact.
//!
//! The MESSAGE is EMR's own where EMR has something of its own to say, and the
//! framework's envelope otherwise: the rejected values of ONE collection-valued filter
//! member are enumerated in the request's own order (`rejected_values_message`), and an
//! undeclared value of a CloudWatch metric alarm's enum member is stated in EMR's prose
//! for that member (`cloud_watch_alarm_message`).

use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, OperationContext, Refusal, Refused, Service, Violation,
    ViolationKind,
};
use std::sync::Arc;

/// The wire error code of `com.amazonaws.emr#InvalidRequestException`, this
/// service's modelled client-input error: "This exception occurs when there is
/// something wrong with user input" (HTTP 400). Used by
/// [`EmulatedService::override_refusal`] so a refusal the framework raises before
/// any handler runs carries an error identity this service's model declares.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";

/// The wire error code the generic request validator answers a refused request with
/// (HTTP 400). Named here because Amazon EMR keeps that identity for the refusals it
/// does not check itself, while still wording some of them in its own prose — the
/// code and the message are chosen separately.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The path segment the validating walk emits for the step from a collection into
/// one of its elements (`clusterStates.1.member`). A member path therefore spells a
/// collection step as the index-or-key segment followed by this one, which is what
/// [`structure_steps`] drops to leave only the STRUCTURE steps.
const COLLECTION_STEP: &str = "member";

/// The operations whose modelled error set declares NO [`INVALID_REQUEST_EXCEPTION`]:
/// the legacy job-flow family and the policy put/get/remove family, which declare only
/// `InternalServerError` (HTTP 500) or no error at all. Naming EMR's client-input error
/// on one of them would answer with an identity that operation cannot return, so the
/// generic request validator's [`VALIDATION_EXCEPTION`] stands there — which is what EMR
/// is recorded answering for `RunJobFlow` and for `PutAutoScalingPolicy`. Every other
/// operation of this service declares it.
///
/// Read off each operation's modelled error set, which the crate root's `OPERATIONS.md`
/// lists for every operation at once, generated from the same Smithy model as the
/// handlers (`smithy_query(query="digest")` lists the same sets).
///
/// Sorted, which [`models_invalid_request_exception`] relies on for `binary_search` and
/// `the_operations_without_the_client_error_are_sorted` pins.
const NO_INVALID_REQUEST_EXCEPTION: &[&str] = &[
    "AddInstanceGroups",
    "AddJobFlowSteps",
    "DescribeJobFlows",
    "GetAutoTerminationPolicy",
    "GetManagedScalingPolicy",
    "ModifyInstanceGroups",
    "PutAutoScalingPolicy",
    "PutAutoTerminationPolicy",
    "PutManagedScalingPolicy",
    "RemoveAutoScalingPolicy",
    "RemoveAutoTerminationPolicy",
    "RemoveManagedScalingPolicy",
    "RunJobFlow",
    "SetKeepJobFlowAliveWhenNoSteps",
    "SetTerminationProtection",
    "SetUnhealthyNodeReplacement",
    "SetVisibleToAllUsers",
    "TerminateJobFlows",
];

/// Whether `operation`'s model declares [`INVALID_REQUEST_EXCEPTION`], and so whether
/// that is an identity it can answer at all.
fn models_invalid_request_exception(operation: &str) -> bool {
    NO_INVALID_REQUEST_EXCEPTION.binary_search(&operation).is_err()
}

tokio::task_local! {
    /// Whether the operation the current request is being served as declares
    /// [`INVALID_REQUEST_EXCEPTION`], set by [`OperationScopedRefusal::run`] around that
    /// operation's whole run and read back by [`EmulatedService::override_refusal`],
    /// which the framework hands no request and no operation.
    ///
    /// Absent altogether means the refusal happened outside an operation's run (an
    /// authentication refusal, which the framework words), which reads as `false`.
    static SERVED_MODELS_CLIENT_ERROR: bool;
}

/// Whether the operation this task is serving declares [`INVALID_REQUEST_EXCEPTION`].
fn served_models_client_error() -> bool {
    SERVED_MODELS_CLIENT_ERROR
        .try_with(|models| *models)
        .unwrap_or(false)
}

/// Serves a request as the operation the generated routing table names, with that
/// operation's modelled-error fact ([`models_invalid_request_exception`]) in scope for
/// the whole run.
///
/// It adds nothing else: the request, the service and the context are handed to the
/// modelled operation unchanged, so deserialization, validation, the handler and the
/// response are the framework's and the operation's own. The routing table holds only
/// generated operations, so the inner lookup can never resolve back to this type.
struct OperationScopedRefusal;

impl ErasedOperation for OperationScopedRefusal {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // `override_operation` returns this type only for a request the routing table
            // can serve, so the lookup below resolves; a request that cannot be routed
            // keeps the framework's routing-error answer either way.
            let routed = svc
                .definition()
                .protocol()
                .routing_key(req)
                .ok()
                .and_then(|key| Some((key, svc.definition().routing_table().get(key)?)));
            let Some((operation, op)) = routed else {
                return svc.definition().routing_error_response(req);
            };
            SERVED_MODELS_CLIENT_ERROR
                .scope(
                    models_invalid_request_exception(operation),
                    op.run(svc, req, ctx),
                )
                .await
        })
    }
}

/// The member of `com.amazonaws.emr#ScalingTrigger` that carries the CloudWatch
/// metric alarm of an automatic scaling rule. Amazon EMR validates that alarm's
/// enum-valued members itself and states each refusal in its own prose, so the step
/// is named here (`smithy_query(query="shape", shape="CloudWatchAlarmDefinition")`
/// lists those members: the comparison operator, the statistic and the unit).
const CLOUD_WATCH_ALARM: &str = "cloudWatchAlarmDefinition";

/// The structure members a validator member path descends through, with the
/// collection steps dropped.
///
/// One step means the offending value is carried by a member of the operation's own
/// input shape — directly (`status`) or as an element of one of its collections
/// (`clusterStates.1.member`). More steps mean the value sits inside a nested request
/// structure (`autoScalingPolicy.rules.1.member.trigger.cloudWatchAlarmDefinition`
/// `.comparisonOperator`), which Amazon EMR's own input check does not reach.
fn structure_steps(path: &str) -> Vec<&str> {
    let segments: Vec<&str> = path.split('.').collect();
    let mut steps = Vec::with_capacity(segments.len());
    let mut at = 0;
    while at < segments.len() {
        // The index-or-key segment plus the collection step name one element of a
        // collection, not a step into another structure.
        if segments.get(at + 1) == Some(&COLLECTION_STEP) {
            at += 2;
        } else {
            steps.push(segments[at]);
            at += 1;
        }
    }
    steps
}

/// The enum violations Amazon EMR checks itself on the request's own input shape.
/// Returned in the order the walk found them, which is the order the request spelled
/// the values in.
fn input_member_enum_violations(violations: &[Violation]) -> Vec<&Violation> {
    violations
        .iter()
        .filter(|violation| matches!(violation.kind, ViolationKind::EnumValue(_)))
        .filter(|violation| structure_steps(&violation.path).len() == 1)
        .collect()
}

/// A member name as words, all lower case (`comparisonOperator` -> `comparison
/// operator`).
fn member_words(member: &str) -> String {
    let mut words = String::with_capacity(member.len() + 2);
    for (at, character) in member.char_indices() {
        if character.is_uppercase() && at > 0 {
            words.push(' ');
        }
        words.extend(character.to_lowercase());
    }
    words
}

/// `words` as a sentence opens: the first letter capitalised.
fn opening(words: &str) -> String {
    let mut characters = words.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// The sentence Amazon EMR answers when the rejected values came from ONE
/// collection-valued filter member: the member's words, then the submitted values
/// that are outside the enum, in the request's own order.
///
/// `None` when the violations are not all elements of one such member — a
/// single-valued member carries one value the generic envelope already states, so
/// there is nothing to enumerate.
fn rejected_values_message(violations: &[&Violation]) -> Option<String> {
    let (first, rest) = violations.split_first()?;
    let member = first.path.split('.').next()?;
    let is_element = |violation: &&Violation| {
        let segments: Vec<&str> = violation.path.split('.').collect();
        segments.len() == 3
            && segments[0] == member
            && segments[2] == COLLECTION_STEP
            && violation.value.is_some()
    };
    if !is_element(first) || !rest.iter().all(is_element) {
        return None;
    }
    let rejected: Vec<&str> = violations
        .iter()
        .filter_map(|violation| violation.value.as_deref())
        .collect();
    Some(format!(
        "{} [{}] are not valid.",
        opening(&member_words(member)),
        rejected.join(", ")
    ))
}

/// Amazon EMR's own sentence for an undeclared value of one of a CloudWatch metric
/// alarm's enum members — the one statement it makes about that alarm, naming the
/// member in prose rather than by its wire path. `None` when no violation of this
/// refusal is on such a member.
///
/// The sentence's fixed text is EMR's, recorded for the alarm's comparison operator;
/// its one variable part is the MEMBER the value was rejected on, taken from the
/// violation's path, so the alarm's other enum members state themselves rather than
/// borrowing the comparison operator's words.
fn cloud_watch_alarm_message(violations: &[Violation]) -> Option<String> {
    violations
        .iter()
        .filter(|violation| matches!(violation.kind, ViolationKind::EnumValue(_)))
        .find_map(|violation| {
            let steps = structure_steps(&violation.path);
            let (member, enclosing) = (steps.last()?, steps.get(steps.len().checked_sub(2)?)?);
            (*enclosing == CLOUD_WATCH_ALARM).then(|| {
                format!(
                    "The supplied cloud watch {} is invalid.",
                    member_words(member)
                )
            })
        })
}

/// The `com-amazonaws-emr-elasticmapreduce` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// Borrowed by `override_operation` so the operation being served is in scope on the
    /// refusal path. Holds nothing: it reaches the real operation through `svc`.
    refusal_scope: OperationScopedRefusal,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            refusal_scope: OperationScopedRefusal,
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
        // Serve the operation the generated routing table names, through the scope
        // that carries its modelled-error fact into `override_refusal`. The routing
        // decision itself is NOT overridden: the key is resolved by the model's own
        // protocol, and a request the table cannot serve is declined here so the
        // framework's routing-error path stays exactly as it was.
        let operation = self.definition().protocol().routing_key(req).ok()?;
        self.definition().routing_table().get(operation)?;
        Some(&self.refusal_scope)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // Amazon EMR validates the ENUM-valued members of an operation's own input
        // shape itself, ahead of the generic request validator, and refuses a value
        // outside such an enum's value set with its own modelled client-input error
        // `InvalidRequestException` ("This exception occurs when there is something
        // wrong with user input", HTTP 400) — the identity an SDK client branching
        // on `emr.exceptions.InvalidRequestException` catches. The framework's
        // default code here, the synthetic `smithy.framework#ValidationException`,
        // is an identity no EMR operation declares, so such a client would see only
        // a generic `ClientError`.
        //
        // That check reaches the members of the input shape, including the elements
        // of its collections (`clusterStates.1.member`) — and only those. An enum
        // carried deeper, inside a nested request structure, is reached by the
        // GENERIC request validator instead and keeps its `ValidationException`:
        // nothing has told EMR's own business logic about the request yet.
        // `structure_steps` is that distinction.
        //
        // And the identity is the SERVED OPERATION's to declare: the legacy job-flow
        // and policy families name no client error at all, so there the generic
        // identity stands too, whatever the violation. `served_models_client_error`
        // reads that fact off the operation the request routed to.
        //
        // Every OTHER modelled constraint — a `@length`, a `@range`, a required
        // member absent — is likewise left to the generic validator's
        // `ValidationException`, which is what EMR is recorded answering for one
        // (`RunJobFlow`'s `ManagedScalingPolicy` bounds, and the refusals its own
        // handler raises). A request violating both classes at once is refused as
        // the enum class, because that is the check EMR runs first.
        //
        // The MESSAGE is chosen separately from the code, because EMR words some of
        // these refusals itself:
        //
        // - rejected values that are elements of ONE collection-valued filter member
        //   are enumerated, in the order the request spelled them: that sentence is
        //   the only part of the refusal telling a caller WHICH of several submitted
        //   values was rejected;
        // - an undeclared value of a CloudWatch metric alarm's enum member is stated
        //   in EMR's own prose for that member, under the generic code, because EMR
        //   validates that alarm itself;
        // - otherwise the message stays the framework's, which names the offending
        //   value, the member path the model spells and the constraint it failed.
        //
        // The status (400) is the framework's throughout, and an authentication
        // refusal keeps the framework's wording — it is not an input-constraint
        // failure.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        let on_input_member = input_member_enum_violations(violations);
        if on_input_member.is_empty() || !served_models_client_error() {
            return Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: cloud_watch_alarm_message(violations)?,
            });
        }
        let message = match rejected_values_message(&on_input_member) {
            Some(sentence) => sentence,
            None => aws_smithy_schema_server::envelope(violations)?,
        };
        Some(Refused {
            code: INVALID_REQUEST_EXCEPTION.to_string(),
            message,
        })
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

#[cfg(test)]
mod tests {
    use super::{
        member_words, models_invalid_request_exception, structure_steps,
        NO_INVALID_REQUEST_EXCEPTION,
    };

    /// The table is searched with `binary_search`, so it must be sorted and hold no
    /// duplicate.
    #[test]
    fn the_operations_without_the_client_error_are_sorted() {
        assert!(
            NO_INVALID_REQUEST_EXCEPTION
                .windows(2)
                .all(|pair| pair[0] < pair[1]),
            "case {NO_INVALID_REQUEST_EXCEPTION:?}: the table must be sorted and hold no \
             duplicate"
        );
    }

    /// Whether an operation can answer EMR's client-input error is read off the table,
    /// over operations from each family: the legacy job-flow and policy ones cannot, the
    /// rest can.
    #[test]
    fn the_client_error_is_only_claimed_where_the_model_declares_it() {
        let cases = [
            ("ListClusters", true),
            ("ListSteps", true),
            ("ListInstances", true),
            ("CreatePersistentAppUI", true),
            ("AddInstanceFleet", true),
            ("RunJobFlow", false),
            ("PutAutoScalingPolicy", false),
            ("AddJobFlowSteps", false),
            ("TerminateJobFlows", false),
            ("GetManagedScalingPolicy", false),
        ];
        for (operation, expected) in cases {
            assert_eq!(
                models_invalid_request_exception(operation),
                expected,
                "case {operation:?}: expected {expected}, got {}",
                models_invalid_request_exception(operation)
            );
        }
    }

    /// A member path's structure steps, with the index-or-key plus collection-step pairs
    /// dropped: one step for a member of the operation's own input shape, more for a
    /// value nested inside a request structure.
    #[test]
    fn structure_steps_drop_the_collection_steps() {
        let cases: [(&str, &[&str]); 5] = [
            ("status", &["status"]),
            ("clusterStates.1.member", &["clusterStates"]),
            ("instanceFleet.instanceFleetType", &["instanceFleet", "instanceFleetType"]),
            ("steps.1.member.actionOnFailure", &["steps", "actionOnFailure"]),
            (
                "autoScalingPolicy.rules.2.member.trigger.cloudWatchAlarmDefinition.\
                 comparisonOperator",
                &[
                    "autoScalingPolicy",
                    "rules",
                    "trigger",
                    "cloudWatchAlarmDefinition",
                    "comparisonOperator",
                ],
            ),
        ];
        for (path, expected) in cases {
            let steps = structure_steps(path);
            assert_eq!(
                steps, expected,
                "case {path:?}: expected {expected:?}, got {steps:?}"
            );
        }
    }

    /// A member name becomes the words a sentence states it with.
    #[test]
    fn a_member_name_becomes_its_words() {
        let cases = [
            ("status", "status"),
            ("clusterStates", "cluster states"),
            ("instanceGroupTypes", "instance group types"),
            ("comparisonOperator", "comparison operator"),
            ("", ""),
        ];
        for (member, expected) in cases {
            let words = member_words(member);
            assert_eq!(
                words, expected,
                "case {member:?}: expected {expected:?}, got {words:?}"
            );
        }
    }
}
