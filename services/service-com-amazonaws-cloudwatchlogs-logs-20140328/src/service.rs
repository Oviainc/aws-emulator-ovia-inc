// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-cloudwatchlogs-logs-20140328` service: how it claims requests and how it words the
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
//!   below needs — see "The code a refusal carries".
//! - `override_refusal` — EDITED HERE. A request refused for violating a modelled
//!   constraint is answered with the refusal error the OPERATION BEING SERVED
//!   declares, worded against the input position that failed — see "The code a refusal
//!   carries" and "How this service words a refusal".
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
//! # The code a refusal carries
//!
//! CloudWatch Logs does not answer a modelled-constraint violation with a generic
//! `ValidationException`. Its own error for "A parameter is specified incorrectly" is
//! [`INVALID_PARAMETER_EXCEPTION`] (HTTP 400), which all but a couple of this service's
//! operations declare, and which is the code the recordings show it raising for a value
//! outside a declared `@range`, `@pattern` or enum, and for an absent `@required`
//! member. The code is what an SDK's error mapper dispatches on, so answering
//! `ValidationException` where the operation does not even declare that shape leaves a
//! code-generated client with no branch to take.
//!
//! The code is a per-OPERATION fact, because the error SET is: the delivery,
//! scheduled-query and S3-table-source families declare `ValidationException` and no
//! `InvalidParameterException` ([`VALIDATION_EXCEPTION_OPERATIONS`]), and the two
//! `@deprecated` tag operations declare neither ([`NO_REFUSAL_ERROR_OPERATIONS`], where
//! this service has nothing of its own to say and the framework's wording stands). So
//! the refusal code is chosen by [`refusal_code`] from the operation's own modelled
//! error set — every set is listed in the crate root's `OPERATIONS.md`, which is
//! generated from the same Smithy model as the handlers.
//!
//! The framework refuses such a request BEFORE any handler runs — the centralized walk
//! deserializes, validates, and only then calls `handle` — so [`Service::override_refusal`]
//! is the only place this choice can be made, and that seam is handed the refusal alone,
//! with no request and no operation. [`OperationScopedRefusal`] is how the operation
//! reaches it: `override_operation` returns it for every request the routing table can
//! serve, and it runs the real operation inside a task-local scope holding that
//! operation's refusal code, which `override_refusal` reads back. The scope wraps the
//! whole of the operation's run, and a task-local travels with the task across every
//! await point, so the code read on the refusal path is always the one belonging to the
//! request being served.
//!
//! # How this service words a refusal
//!
//! The envelope is AWS's own — `N validation error(s) detected:` over one clause per
//! violation, joined by `"; "` — and each clause names the INPUT POSITION that failed and
//! the constraint it failed, and nothing of the value submitted there ([`SUBJECT`],
//! [`requirement`]). Two parts of that clause are rules this service owns, and both are
//! read off the recordings rather than copied from one of them:
//!
//! * the subject is `Value at '<member path>'` whether a value reached the member or not:
//!   the recorded refusal for an ABSENT `@required` member states
//!   `Value at 'storageTier' failed to satisfy constraint: Member must not be null`,
//!   with no `null` in the subject, exactly as a present-but-invalid value's clause reads;
//! * an enum's admissible set is listed in ASCENDING ORDER of its values, not in the
//!   model's declaration order: the recorded refusals list `StorageTier` as
//!   `[INTELLIGENT_TIERING, STANDARD]` where the model declares
//!   `[STANDARD, INTELLIGENT_TIERING]`, and `ListAggregateLogGroupSummariesGroupBy` as
//!   `[DATA_SOURCE_NAME_AND_TYPE, DATA_SOURCE_NAME_TYPE_AND_FORMAT]` where the model
//!   declares the format value first — two enums whose declaration order differs from the
//!   recorded one, and in both the recorded order is the sorted set.
//!
//! A `@pattern` is reported as the MODEL spells it, anchors included; the reasoning for
//! that one is on [`requirement`], which is also where the handler-side refusal in
//! `crate::state` reads its wording from ([`one_violation_message`]), so both refusal
//! paths read alike.
//!
//! The subject rule is the half `services/common/src/validation.rs` names as "still to
//! come", and ~26 service crates word it inline today. Migrating them onto one shared
//! helper is a change of its own rather than a side effect of this service's issues, so it
//! is worded here — as that module asks, and as it records that "which values an enum's
//! value set lists and in which order" stays with the service.

use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, OperationContext, Refusal, Refused, Service, Violation,
    ViolationKind,
};
use std::sync::Arc;

/// CloudWatch Logs' error for a parameter it will not accept: "A parameter is specified
/// incorrectly" (HTTP 400). Declared by all but the operations named in
/// [`VALIDATION_EXCEPTION_OPERATIONS`] and [`NO_REFUSAL_ERROR_OPERATIONS`], and the code
/// this service answers for a modelled-constraint violation wherever it is declared.
const INVALID_PARAMETER_EXCEPTION: &str = "InvalidParameterException";

/// The refusal error of the operations that declare no `InvalidParameterException`: the
/// modeled `ValidationException` (`com.amazonaws.cloudwatchlogs#ValidationException`,
/// `@error("client")`, HTTP 400), "One of the parameters for the request is not valid".
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The operations whose modelled error set declares [`VALIDATION_EXCEPTION`] and NOT
/// [`INVALID_PARAMETER_EXCEPTION`] — the delivery, scheduled-query and S3-table-source
/// families, which were added to this service with the newer error vocabulary. Each
/// one's error set is listed in the crate root's `OPERATIONS.md`.
///
/// Sorted, which [`refusal_code`] relies on for `binary_search` and
/// `operation_tables_are_sorted_and_disjoint` pins.
const VALIDATION_EXCEPTION_OPERATIONS: &[&str] = &[
    "AssociateSourceToS3TableIntegration",
    "CreateDelivery",
    "CreateScheduledQuery",
    "DeleteDelivery",
    "DeleteDeliveryDestination",
    "DeleteDeliveryDestinationPolicy",
    "DeleteDeliverySource",
    "DeleteScheduledQuery",
    "DescribeConfigurationTemplates",
    "DescribeDeliveries",
    "DescribeDeliveryDestinations",
    "DescribeDeliverySources",
    "DisassociateSourceFromS3TableIntegration",
    "GetDelivery",
    "GetDeliveryDestination",
    "GetDeliveryDestinationPolicy",
    "GetDeliverySource",
    "GetScheduledQuery",
    "GetScheduledQueryHistory",
    "ListScheduledQueries",
    "ListSourcesForS3TableIntegration",
    "PutDeliveryDestination",
    "PutDeliveryDestinationPolicy",
    "PutDeliverySource",
    "UpdateDeliveryConfiguration",
    "UpdateScheduledQuery",
];

/// The operations that declare NEITHER refusal error: the two `@deprecated` log-group
/// tag operations, whose modelled errors are `ResourceNotFoundException` (and, for
/// `ListTagsLogGroup`, `ServiceUnavailableException`). Naming either refusal shape on
/// them would answer with an error the operation cannot return, so this service declines
/// to choose and the framework's own wording stands.
///
/// Sorted, as [`VALIDATION_EXCEPTION_OPERATIONS`] is.
const NO_REFUSAL_ERROR_OPERATIONS: &[&str] = &["ListTagsLogGroup", "UntagLogGroup"];

/// The error code `operation` answers for a value outside a modelled constraint, or
/// `None` when it declares no refusal error and the framework's wording stands.
fn refusal_code(operation: &str) -> Option<&'static str> {
    if NO_REFUSAL_ERROR_OPERATIONS.binary_search(&operation).is_ok() {
        return None;
    }
    if VALIDATION_EXCEPTION_OPERATIONS
        .binary_search(&operation)
        .is_ok()
    {
        return Some(VALIDATION_EXCEPTION);
    }
    Some(INVALID_PARAMETER_EXCEPTION)
}

tokio::task_local! {
    /// The refusal code of the operation the current request is being served as, set by
    /// [`OperationScopedRefusal::run`] around that operation's whole run and read back by
    /// [`EmulatedService::override_refusal`], which the framework hands no request.
    ///
    /// `None` inside the scope means the operation declares no refusal error; absent
    /// altogether means the refusal happened outside an operation's run (an
    /// authentication refusal, which the framework words), and both leave the framework's
    /// wording in place.
    static SERVED_REFUSAL_CODE: Option<&'static str>;
}

/// The refusal code of the operation this task is serving, if any.
fn served_refusal_code() -> Option<&'static str> {
    SERVED_REFUSAL_CODE.try_with(|code| *code).ok().flatten()
}

/// Serves a request as the operation the generated routing table names, with that
/// operation's refusal code ([`refusal_code`]) in scope for the whole run.
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
            // can serve, so the two lookups below resolve; a request that cannot be routed
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
            SERVED_REFUSAL_CODE
                .scope(refusal_code(operation), op.run(svc, req, ctx))
                .await
        })
    }
}

/// How CloudWatch Logs names the subject of a constraint clause: the POSITION in the input
/// that failed, and never the value sitting there — nor, for a member the request did not
/// carry at all, the framework's `null` (see "How this service words a refusal").
///
/// A caller's own data — a delivery id, a delivery-source name, an integration-source
/// identifier, a page size — is not reflected back into an error body a client may log or
/// match on.
const SUBJECT: &str = "Value at";

/// The requirement one violation states: the modeled constraint the value failed.
///
/// Read off the violation's structural [`ViolationKind`] — the bound, the pattern, the
/// value set the MODEL declares — so a clause states the constraint rather than anything
/// about the request beyond which member it names.
///
/// THE PATTERN RENDERING RULE, stated once for the whole service (the handler-side
/// refusal in `crate::state` words its clause through [`one_violation_message`], so both
/// refusal paths read alike): a `@pattern` is reported as the MODEL spells it, anchors
/// included. The model is this emulator's only source for that expression, and the
/// recorded refusals of this service disagree with each other about the anchors — the
/// three recorded `DeliveryId` refusals print the model's `^[0-9A-Za-z]+$` anchored,
/// while the recorded `DeliverySourceName` one prints the model's `^[\w-]*$` as
/// `[\w-]*` — so no rule over the model can produce both and the expression is rendered
/// rather than rewritten.
///
/// THE ENUM ORDERING RULE: an enum's admissible set is listed in ascending order of its
/// VALUES rather than in the model's declaration order, which is what the recordings show
/// for the two enums whose orders differ (see "How this service words a refusal"). The
/// order comes from the values themselves, so it holds for every enum this service
/// declares rather than for the two the recordings happen to cover.
///
/// Returns `None` for a violation class this service has no wording for, which lets
/// [`constraint_envelope`] decline the whole refusal and keep the framework's wording
/// rather than invent a sentence for it ([`ViolationKind`] is `#[non_exhaustive]`, so a
/// class can be added upstream).
fn requirement(kind: &ViolationKind) -> Option<String> {
    Some(match kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        ViolationKind::Pattern(pattern) => {
            format!("Member must satisfy regular expression pattern: {pattern}")
        }
        // A "value set" is just that: the requirement is WHICH values the model allows,
        // listed in ascending order of the values (the enum ordering rule above).
        ViolationKind::EnumValue(allowed) => {
            let mut values: Vec<&str> = allowed.iter().map(String::as_str).collect();
            values.sort_unstable();
            format!("Member must satisfy enum value set: [{}]", values.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    })
}

/// Render one constraint violation the way this service states it: the input position and
/// the constraint it failed, and NOTHING of the value that was rejected.
fn constraint_clause(violation: &Violation) -> Option<String> {
    let requirement = requirement(&violation.kind)?;
    Some(clause_at(&violation.path, &requirement))
}

/// The clause a member at `path` is refused with: the position and the requirement, with
/// no value — the one subject form this service states, for a value it rejected and for a
/// member the request never carried alike.
fn clause_at(path: &str, requirement: &str) -> String {
    format!("{SUBJECT} '{path}' failed to satisfy constraint: {requirement}")
}

/// The envelope one clause is wrapped in.
fn one_error_envelope(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The whole message for ONE refused member, for a violation a HANDLER found itself at a
/// member the framework's validating walk no longer sees (a member carrying
/// `skip_validation`, which `crate::state` judges).
///
/// Built from the same subject, the same envelope and the same pattern rendering as a
/// refusal the framework raised, so one violation reads as one message whichever of the
/// two refused the request, and the submitted value is withheld either way.
pub(crate) fn one_violation_message(member: &str, requirement: &str) -> String {
    one_error_envelope(&clause_at(member, requirement))
}

/// The refusal message for a framework-level refusal: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation, in the order the
/// validating walk found them.
///
/// `None` declines the whole refusal — see [`requirement`].
fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations
        .iter()
        .map(constraint_clause)
        .collect::<Option<Vec<_>>>()?;
    match clauses.len() {
        0 => None,
        1 => Some(one_error_envelope(&clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The `com-amazonaws-cloudwatchlogs-logs-20140328` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The dispatch `override_operation` hands back, which carries the served
    /// operation's refusal code into [`Service::override_refusal`]. Held on the service
    /// because that seam returns a borrow of `self`.
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
        // Serve the operation the generated routing table names, through the scope that
        // carries its refusal code into `override_refusal` (see "The code a refusal
        // carries"). The routing decision itself is NOT overridden: the key is resolved
        // by the model's own protocol, and a request the table cannot serve is declined
        // here so the framework's routing-error path stays exactly as it was.
        let operation = self.definition().protocol().routing_key(req).ok()?;
        self.definition().routing_table().get(operation)?;
        Some(&self.refusal_scope)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it.
        //
        // THE CODE is the refusal error the SERVED OPERATION declares —
        // `InvalidParameterException` on all but the delivery, scheduled-query and
        // S3-table-source families — which is what an SDK's error mapper binds for this
        // failure; see "The code a refusal carries".
        //
        // THE MESSAGE states a member-constraint failure against the INPUT POSITION that
        // failed and the constraint it failed — never the value that was submitted there,
        // so a delivery id, a source name or a page size a caller sent is not reflected
        // back into a 400 body. A `@pattern` keeps the anchors the model spells it with
        // and an enum's value set is listed in ascending order — both rules stated once
        // on `requirement` above, and shared with the handler-side refusal through
        // `one_violation_message`.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure, and it is raised before any operation is served.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: served_refusal_code()?.to_string(),
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

#[cfg(test)]
mod tests {
    use super::{
        constraint_clause, constraint_envelope, refusal_code, served_refusal_code,
        INVALID_PARAMETER_EXCEPTION, NO_REFUSAL_ERROR_OPERATIONS, SERVED_REFUSAL_CODE,
        VALIDATION_EXCEPTION, VALIDATION_EXCEPTION_OPERATIONS,
    };
    use smithy_server_runtime::{Violation, ViolationKind};

    /// Both tables are searched with `binary_search`, so both must be sorted; and an
    /// operation cannot be in both, or its refusal code would depend on which table was
    /// consulted first.
    #[test]
    fn operation_tables_are_sorted_and_disjoint() {
        for table in [VALIDATION_EXCEPTION_OPERATIONS, NO_REFUSAL_ERROR_OPERATIONS] {
            assert!(
                table.windows(2).all(|pair| pair[0] < pair[1]),
                "case {table:?}: the table must be sorted and hold no duplicate"
            );
        }
        for operation in NO_REFUSAL_ERROR_OPERATIONS {
            assert!(
                !VALIDATION_EXCEPTION_OPERATIONS.contains(operation),
                "case {operation}: an operation belongs to at most one table"
            );
        }
    }

    /// The code is the operation's own: `InvalidParameterException` by default, the
    /// newer families' `ValidationException` where that is all they declare, and nothing
    /// at all for an operation that declares neither.
    #[test]
    fn the_refusal_code_is_the_served_operations_own() {
        let cases = [
            ("GetLogEvents", Some(INVALID_PARAMETER_EXCEPTION)),
            ("DescribeLogStreams", Some(INVALID_PARAMETER_EXCEPTION)),
            ("PutStorageTierPolicy", Some(INVALID_PARAMETER_EXCEPTION)),
            ("CreateLookupTable", Some(INVALID_PARAMETER_EXCEPTION)),
            ("PutIntegration", Some(INVALID_PARAMETER_EXCEPTION)),
            (
                "ListAggregateLogGroupSummaries",
                Some(INVALID_PARAMETER_EXCEPTION),
            ),
            ("GetDelivery", Some(VALIDATION_EXCEPTION)),
            ("UpdateScheduledQuery", Some(VALIDATION_EXCEPTION)),
            (
                "AssociateSourceToS3TableIntegration",
                Some(VALIDATION_EXCEPTION),
            ),
            ("ListTagsLogGroup", None),
            ("UntagLogGroup", None),
        ];
        for (operation, expected) in cases {
            let actual = refusal_code(operation);
            assert_eq!(
                actual, expected,
                "case {operation}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// Outside an operation's run there is no served operation, so nothing is reworded;
    /// inside one, the scoped code is what the refusal seam reads.
    #[tokio::test]
    async fn the_served_code_is_read_back_only_inside_the_scope() {
        assert_eq!(
            served_refusal_code(),
            None,
            "case outside a scope: no operation is being served"
        );
        let inside = SERVED_REFUSAL_CODE
            .scope(refusal_code("GetLogEvents"), async {
                // An await point inside the scope: a task-local survives it.
                tokio::task::yield_now().await;
                served_refusal_code()
            })
            .await;
        assert_eq!(
            inside,
            Some(INVALID_PARAMETER_EXCEPTION),
            "case inside GetLogEvents' scope: the served operation's code is read back"
        );
        let declined = SERVED_REFUSAL_CODE
            .scope(refusal_code("UntagLogGroup"), async {
                served_refusal_code()
            })
            .await;
        assert_eq!(
            declined, None,
            "case inside UntagLogGroup's scope: the operation declares no refusal error"
        );
    }

    /// Every constraint class states `Value at '<member>'` and no value — an enum with its
    /// admissible set sorted, a `@pattern` with the anchors the model writes, a bound, and
    /// an absent member with no `null` in the subject — and several violations take the
    /// plural envelope, counted and joined the way AWS counts and joins them.
    #[test]
    fn a_clause_states_the_member_and_the_requirement_without_a_value() {
        let outside_enum = Violation {
            path: "storageTier".to_string(),
            // The model's declaration order, which is not the order a refusal lists.
            kind: ViolationKind::EnumValue(vec![
                "STANDARD".to_string(),
                "INTELLIGENT_TIERING".to_string(),
            ]),
            value: Some("GLACIER".to_string()),
        };
        let outside_pattern = Violation {
            path: "lookupTableName".to_string(),
            kind: ViolationKind::Pattern("^[a-zA-Z0-9_]+$".to_string()),
            value: Some("bad-name-with-hyphen".to_string()),
        };
        let above_bound = Violation {
            path: "limit".to_string(),
            kind: ViolationKind::MaxValue("50".to_string()),
            value: Some("51".to_string()),
        };
        let absent = Violation {
            path: "storageTier".to_string(),
            kind: ViolationKind::MissingRequiredMember,
            value: None,
        };
        let cases = [
            (
                "a value outside an enum",
                &outside_enum,
                "Value at 'storageTier' failed to satisfy constraint: Member must satisfy \
                 enum value set: [INTELLIGENT_TIERING, STANDARD]",
            ),
            (
                "a value outside a pattern",
                &outside_pattern,
                "Value at 'lookupTableName' failed to satisfy constraint: Member must \
                 satisfy regular expression pattern: ^[a-zA-Z0-9_]+$",
            ),
            (
                "a value above a bound",
                &above_bound,
                "Value at 'limit' failed to satisfy constraint: Member must have value \
                 less than or equal to 50",
            ),
            (
                "an absent member",
                &absent,
                "Value at 'storageTier' failed to satisfy constraint: Member must not be \
                 null",
            ),
        ];
        for (case, violation, expected) in cases {
            let actual = constraint_clause(violation);
            assert_eq!(
                actual.as_deref(),
                Some(expected),
                "case {case}: expected {expected:?}, got {actual:?}"
            );
        }
        assert_eq!(constraint_envelope(&[]), None, "nothing to state");
        assert_eq!(
            constraint_envelope(std::slice::from_ref(&above_bound)).as_deref(),
            Some(
                "1 validation error detected: Value at 'limit' failed to satisfy \
                 constraint: Member must have value less than or equal to 50"
            ),
            "one violation takes the singular envelope"
        );
        assert_eq!(
            constraint_envelope(&[above_bound, absent]).as_deref(),
            Some(
                "2 validation errors detected: Value at 'limit' failed to satisfy \
                 constraint: Member must have value less than or equal to 50; Value at \
                 'storageTier' failed to satisfy constraint: Member must not be null"
            ),
            "several violations are counted and joined"
        );
    }
}
