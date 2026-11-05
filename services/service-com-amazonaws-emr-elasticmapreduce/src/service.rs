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
//! A request the framework refuses before any handler runs is refused by one of two
//! layers, and BOTH the error code and the message come from the layer that reached it.
//!
//! Amazon EMR checks the INPUT SHAPE of an operation itself, ahead of the generic
//! request validator, and answers its own modelled client-input error
//! [`INVALID_REQUEST_EXCEPTION`] (HTTP 400) — the identity an SDK client branching on
//! `emr.exceptions.InvalidRequestException` catches — for a value outside a modelled
//! enum and for an input member the request did not carry at all. A value carried
//! deeper, inside a nested request structure, is reached only by the generic validator,
//! which keeps its [`VALIDATION_EXCEPTION`]; `structure_steps` is that distinction, and
//! it is what the recorded `PutAutoScalingPolicy` refusal of a nested comparison
//! operator answers.
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
//! member are enumerated in the request's own order (`rejected_values_message`), an
//! absent member is stated by the sentence an unusable value of it gets
//! (`absent_member_refusal`), and an undeclared value of a CloudWatch metric alarm's
//! enum member is stated in EMR's prose for that member (`cloud_watch_alarm_message`).
//!
//! `Refused`, the framework's answer type, carries a code and a sentence. EMR's own
//! error carries its sentence in `Message` BESIDE the `ErrorCode` that tells one refusal
//! class from another, so a refusal reworded into that shape is answered by
//! [`OperationScopedRefusal`] from the shape itself, serialized through the model's own
//! schema.
//!
//! [`OperationScopedRefusal`] also restores the ORDER the checks run in. Amazon EMR
//! resolves the cluster a cluster-scoped request names before it checks the request's
//! other members, while the framework checks every member before the handler runs, so a
//! request carrying both a cluster id the service cannot use and another unusable member
//! is refused for the CLUSTER — which is what the recorded `ListSteps` carrying an
//! unresolvable cluster id beside an undeclared step state answers.

use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, OperationContext, Refusal, Refused, Service, Violation,
    ViolationKind,
};
use std::cell::RefCell;
use std::sync::Arc;

use crate::ops::types::InvalidRequestException;

pub mod request_layer;

/// The wire error code of `com.amazonaws.emr#InvalidRequestException`, this
/// service's modelled client-input error: "This exception occurs when there is
/// something wrong with user input" (HTTP 400). Used by
/// [`EmulatedService::override_refusal`] so a refusal the framework raises before
/// any handler runs carries an error identity this service's model declares.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";

/// The status both of this service's client-error envelopes carry: the model's
/// `httpError` for `com.amazonaws.emr#InvalidRequestException`
/// (`smithy_query(query="shape", shape="InvalidRequestException")`), and the request
/// validator's own — the status a handler raising either answers, so a refusal reworded
/// into one answers the same.
const REFUSED_STATUS: u16 = 400;

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

/// The operations that RESOLVE a cluster from a `ClusterId` of their own input shape and
/// can answer [`INVALID_REQUEST_EXCEPTION`] for one they cannot use.
///
/// Amazon EMR resolves that cluster BEFORE it checks the request's other members: a
/// recorded `ListSteps` carrying both a cluster id the service holds no cluster for and
/// an undeclared step state is refused for the CLUSTER. The framework validates the whole
/// input ahead of the handler, which would invert that order, so this table is what
/// [`OperationScopedRefusal`] restores it with — see `cluster_first_response`.
///
/// Read off this crate's handlers: these are the operations whose handler resolves
/// `input.cluster_id` against the cluster store. An operation still scaffolded resolves
/// nothing, and the rest of the legacy job-flow and policy family declares no client
/// error ([`NO_INVALID_REQUEST_EXCEPTION`]), so neither is here.
///
/// Sorted, which [`resolves_cluster_id`] relies on for `binary_search` and
/// `the_operations_that_resolve_a_cluster_id_are_sorted` pins.
const RESOLVES_CLUSTER_ID: &[&str] = &[
    "AddInstanceFleet",
    "CancelSteps",
    "DescribeCluster",
    "DescribeStep",
    "GetClusterSessionCredentials",
    "GetSession",
    "ListInstanceFleets",
    "ListInstanceGroups",
    "ListSteps",
    "ModifyCluster",
];

/// Whether `operation` resolves a cluster from a `ClusterId` of its own input shape.
fn resolves_cluster_id(operation: &str) -> bool {
    RESOLVES_CLUSTER_ID.binary_search(&operation).is_ok()
}

/// The `ClusterId` member as the wire spells it, which is how a request names it and how
/// a validator member path spells it once lower-camelled.
const CLUSTER_ID: &str = "ClusterId";

/// The operation that resolves an EC2 instance profile from its own input.
///
/// Amazon EMR resolves that profile BEFORE it checks the request's other members: a
/// recorded `RunJobFlow` carrying both a `JobFlowRole` naming no instance profile and no
/// `Instances` member at all is refused for the PROFILE. The framework validates the
/// whole input ahead of the handler, which would invert that order, so
/// [`OperationScopedRefusal`] restores it — see `job_flow_role_first_response`.
const RESOLVES_JOB_FLOW_ROLE: &str = "RunJobFlow";

/// The `JobFlowRole` member as the wire spells it.
const JOB_FLOW_ROLE: &str = "JobFlowRole";

/// The wire error code of the authorization failure Amazon EMR answers a cluster launch
/// whose instance profile it could not authorize with (HTTP 400).
///
/// RECORDED behavior: no operation of this service declares the shape, which is why it is
/// built from this crate's own synthetic one
/// ([`crate::scaffolded::AccessDeniedException`]).
const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";

/// The input member whose absence this service refuses as a cluster id it cannot
/// resolve, as a validator member path spells it.
const CLUSTER_ID_MEMBER: &str = "clusterId";

/// The input member whose absence this service refuses as a release label it cannot
/// resolve, as a validator member path spells it.
const RELEASE_LABEL_MEMBER: &str = "releaseLabel";

/// The legacy job-flow surface's refusal of a `ClusterId` it cannot use: that surface
/// states one fixed sentence, so the value is no part of it.
fn unusable_job_flow(_cluster_id: &str) -> InvalidRequestException {
    crate::state::unknown_job_flow()
}

/// `GetClusterSessionCredentials`'s refusal of a `ClusterId` it cannot use: the
/// cluster-id sentence with no id in it, so the value is no part of it either.
fn unusable_cluster_without_id(_cluster_id: &str) -> InvalidRequestException {
    crate::state::unknown_cluster_without_id()
}

/// How `operation` refuses a `ClusterId` it cannot use — one the request omitted, or one
/// it holds no cluster under: the refusal of the SURFACE it reaches a cluster through.
///
/// Every operation reports it as a cluster id, quoting the value
/// (`crate::state::unknown_cluster`), except the two that word that refusal otherwise and
/// say so at their own call site: the legacy job-flow surface, which calls a cluster a
/// job flow, and `GetClusterSessionCredentials`, which states the cluster-id sentence
/// with no id in it. The rest of that surface declares no client error at all
/// ([`NO_INVALID_REQUEST_EXCEPTION`]), so the generic validator answers there whatever
/// the violation.
fn cluster_id_refusal_of(operation: &str) -> fn(&str) -> InvalidRequestException {
    match operation {
        "AddInstanceFleet" => unusable_job_flow,
        "GetClusterSessionCredentials" => unusable_cluster_without_id,
        _ => crate::state::unknown_cluster,
    }
}

/// The per-operation facts [`EmulatedService::override_refusal`] needs, which the
/// framework hands it neither of: it is called with the refusal alone, with no request
/// and no operation.
#[derive(Debug, Clone, Copy)]
struct ServedOperation {
    /// Whether this operation's model declares [`INVALID_REQUEST_EXCEPTION`], and so
    /// whether that is an identity it can answer.
    models_client_error: bool,
    /// Whether it resolves a cluster from a `ClusterId` of its own input shape.
    resolves_cluster_id: bool,
    /// How it refuses a `ClusterId` it cannot use.
    cluster_id_refusal: fn(&str) -> InvalidRequestException,
}

impl ServedOperation {
    /// The facts of `operation`, read off the per-operation tables above.
    fn of(operation: &str) -> Self {
        Self {
            models_client_error: models_invalid_request_exception(operation),
            resolves_cluster_id: resolves_cluster_id(operation),
            cluster_id_refusal: cluster_id_refusal_of(operation),
        }
    }
}

/// What the framework refused about this request, left for [`OperationScopedRefusal`] by
/// [`EmulatedService::override_refusal`] — which is handed the refusal alone, and can
/// answer with a code and a sentence and nothing else.
#[derive(Debug, Default)]
struct FrameworkRefusal {
    /// Whether the framework refused this request as invalid before any handler ran, so
    /// no handler has checked anything about it yet.
    refused: bool,
    /// Whether one of the violations is on the `ClusterId` member itself, in which case
    /// that member's own refusal is the one being answered.
    on_cluster_id: bool,
    /// The modelled client error chosen for it, when EMR's own refusal was chosen rather
    /// than the generic validator's: `InvalidRequestException` carries its sentence in
    /// `Message` BESIDE an `ErrorCode`, the discriminator a client branches on, so the
    /// shape itself travels here and is serialized through the model's own schema.
    chosen: Option<InvalidRequestException>,
}

tokio::task_local! {
    /// The operation the current request is being served as, set by
    /// [`OperationScopedRefusal::run`] around that operation's whole run and read back by
    /// [`EmulatedService::override_refusal`].
    ///
    /// Absent altogether means the refusal happened outside an operation's run (an
    /// authentication refusal, which the framework words), where no operation's facts
    /// apply.
    static SERVED_OPERATION: ServedOperation;

    /// What the framework refused about this request, for the scope of that same run.
    static FRAMEWORK_REFUSAL: RefCell<FrameworkRefusal>;
}

/// The facts of the operation this task is serving, if it is serving one.
fn served_operation() -> Option<ServedOperation> {
    SERVED_OPERATION.try_with(|served| *served).ok()
}

/// Leave what the framework refused for [`OperationScopedRefusal::run`] to answer with.
/// Dropped when the refusal happened outside an operation's run, where there is no
/// operation to answer for.
fn remember_framework_refusal(refusal: FrameworkRefusal) {
    let _ = FRAMEWORK_REFUSAL.try_with(|slot| *slot.borrow_mut() = refusal);
}

/// Serves a request as the operation the generated routing table names, with that
/// operation's facts ([`ServedOperation`]) in scope for
/// the whole run.
///
/// It adds nothing else to the SERVED request: the request, the service and the context
/// are handed to the modelled operation unchanged, so deserialization, validation, the
/// handler and the response are the framework's and the operation's own. The routing
/// table holds only generated operations, so the inner lookup can never resolve back to
/// this type.
///
/// A request the framework REFUSES before any handler ran is answered here, because the
/// refusal hook cannot answer it alone:
///
/// - `InvalidRequestException` carries its sentence in `Message` BESIDE an `ErrorCode`,
///   and [`Refused`] has no member for the second, so the chosen shape travels through
///   [`FRAMEWORK_REFUSAL`] and is serialized here through the model's own schema;
/// - Amazon EMR resolves the cluster a cluster-scoped request names BEFORE it checks the
///   request's other members, while the framework checks every member before the handler
///   runs — so the cluster-id refusal the handler would have answered takes precedence
///   here over the one the validator raised.
///
/// Both need the request and this service's store, which the refusal hook is handed
/// neither of, so the engine is held here.
struct OperationScopedRefusal<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> ErasedOperation for OperationScopedRefusal<Eng> {
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
            // Deserialization precedes every later layer: a value whose JSON type the
            // modelled member it is bound to cannot be read from never becomes a value
            // at all, so it is refused as the PROTOCOL's own failure before the
            // operation's own walk reaches its constraints or its handler. The framework
            // would otherwise render that codec failure in the constraint-violation
            // envelope, which is a different error class and a different remediation for
            // the caller ([`request_layer`]). The cost is deliberate: a request's body is
            // read twice, once here and once by the dispatch that follows.
            let unreadable = {
                let protocol = svc.definition().wire_protocol(req);
                request_layer::coercion_refusal::<Eng>(operation, protocol.protocol(), req)
            };
            if let Some(message) = unreadable {
                if let Some(response) = serialization_refusal_response(svc, req, message) {
                    return response;
                }
            }
            let served = ServedOperation::of(operation);
            let (response, refused) = SERVED_OPERATION
                .scope(
                    served,
                    FRAMEWORK_REFUSAL.scope(RefCell::new(FrameworkRefusal::default()), async {
                        let response = op.run(svc, req, ctx).await;
                        let refused = FRAMEWORK_REFUSAL.with(|slot| slot.take());
                        (response, refused)
                    }),
                )
                .await;
            if !refused.refused {
                // The operation itself answered, in its own order: nothing to reword.
                return response;
            }
            // The cluster the request names is resolved first — unless the refusal is
            // about the `ClusterId` member itself, which is then the member being
            // answered for.
            if !refused.on_cluster_id {
                if let Some(first) = self.cluster_first_response(served, svc, req) {
                    return first;
                }
            }
            // The EC2 instance profile a cluster launch names is resolved first too, for
            // the same reason and in the same way.
            if let Some(first) = job_flow_role_first_response(operation, svc, req, ctx).await {
                return first;
            }
            // The framework rendered the chosen code and sentence already; what it
            // cannot render is the modelled shape's OTHER member. Re-serializing is
            // therefore an improvement on an answer that is already correct in code,
            // status and prose, so a protocol that cannot carry the shape keeps it.
            match refused.chosen {
                Some(refusal) => modelled_refusal_response(svc, req, refusal).unwrap_or(response),
                None => response,
            }
        })
    }
}

impl<Eng: Send + Sync + 'static> OperationScopedRefusal<Eng> {
    /// The refusal the served operation would have answered for the `ClusterId` the
    /// request carries, when that id is one it cannot use. `None` when the operation
    /// resolves no cluster id, when the request carries none — the absent member is its
    /// own refusal — or when the id names a cluster this service holds.
    ///
    /// A value outside the class an identifier is drawn from is the request-validation
    /// envelope's, exactly as the handler answers it; an id of that class the store has
    /// no record under is the service's own.
    fn cluster_first_response(
        &self,
        served: ServedOperation,
        svc: &dyn Service,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        if !served.resolves_cluster_id {
            return None;
        }
        let cluster_id = request_member(req, CLUSTER_ID)?;
        if !crate::state::is_cluster_id_shaped(&cluster_id) {
            return validation_refusal_response(
                svc,
                req,
                crate::state::malformed_cluster_id_message(&cluster_id),
            );
        }
        let state = crate::state::service_state(&*self.engine)?;
        // A read of the key alone: resolving a cluster id is not a report of the cluster,
        // so nothing the clock owes is materialised here — the handler the request
        // reaches does that for the cluster it serves.
        if state.lock().clusters.contains_key(&cluster_id) {
            return None;
        }
        modelled_refusal_response(svc, req, (served.cluster_id_refusal)(&cluster_id))
    }
}

/// The string `member` of `req`'s body, as the request spelled it, or `None` when the
/// body carries no such member, carries it as something other than a string, or is not
/// readable as this service's one protocol
/// (`smithy_query(query="service_info")`: awsJson1_1).
fn request_member(req: &aws_smithy_runtime_api::http::Request, member: &str) -> Option<String> {
    let body = req.body().bytes()?;
    ::serde_json::from_slice::<::serde_json::Value>(body)
        .ok()?
        .get(member)?
        .as_str()
        .map(str::to_string)
}

/// `refusal` as this service answers it: the modelled client error, serialized through
/// the model's own schema in the envelope of the protocol `req` is served in — the same
/// path a handler-raised `InvalidRequestException` takes.
fn modelled_refusal_response(
    svc: &dyn Service,
    req: &aws_smithy_runtime_api::http::Request,
    refusal: InvalidRequestException,
) -> Option<aws_smithy_runtime_api::http::Response> {
    svc.definition()
        .wire_protocol(req)
        .protocol()
        .serialize_error(
            &refusal,
            &crate::ops::types::SCHEMA_INVALID_REQUEST_EXCEPTION,
            INVALID_REQUEST_EXCEPTION,
            REFUSED_STATUS,
        )
        .ok()
}

/// `message` as the request-validation envelope answers it, in the envelope of the
/// protocol `req` is served in — the same path a handler refusing a request through
/// [`crate::scaffolded::Scaffolded::validation`] takes.
fn validation_refusal_response(
    svc: &dyn Service,
    req: &aws_smithy_runtime_api::http::Request,
    message: String,
) -> Option<aws_smithy_runtime_api::http::Response> {
    svc.definition()
        .wire_protocol(req)
        .protocol()
        .serialize_error(
            &crate::scaffolded::ValidationException::new(message),
            &crate::scaffolded::VALIDATION_EXCEPTION_SCHEMA,
            VALIDATION_EXCEPTION,
            REFUSED_STATUS,
        )
        .ok()
}

/// `message` as the PROTOCOL's own refusal of a body that could not be read answers it
/// ([`request_layer`]), in the envelope of the protocol `req` is served in.
///
/// The shape is synthetic because no model declares it: the request never became an
/// input, so no operation ran to answer with one of its own errors.
fn serialization_refusal_response(
    svc: &dyn Service,
    req: &aws_smithy_runtime_api::http::Request,
    message: String,
) -> Option<aws_smithy_runtime_api::http::Response> {
    svc.definition()
        .wire_protocol(req)
        .protocol()
        .serialize_error(
            &crate::scaffolded::SerializationException::new(message),
            &crate::scaffolded::SERIALIZATION_EXCEPTION_SCHEMA,
            request_layer::SERIALIZATION_EXCEPTION,
            REFUSED_STATUS,
        )
        .ok()
}

/// `message` as the authorization-failure envelope answers it, in the envelope of the
/// protocol `req` is served in — the same path a handler refusing a request through
/// [`crate::scaffolded::Scaffolded::access_denied`] takes.
fn access_denied_refusal_response(
    svc: &dyn Service,
    req: &aws_smithy_runtime_api::http::Request,
    message: String,
) -> Option<aws_smithy_runtime_api::http::Response> {
    svc.definition()
        .wire_protocol(req)
        .protocol()
        .serialize_error(
            &crate::scaffolded::AccessDeniedException::new(message),
            &crate::scaffolded::ACCESS_DENIED_EXCEPTION_SCHEMA,
            ACCESS_DENIED_EXCEPTION,
            REFUSED_STATUS,
        )
        .ok()
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

/// The prefix of the discriminator Amazon EMR answers rejected values of one input
/// member with: the code names that member, so a client branching on it learns WHICH
/// filter it has to fix. Recorded for the cluster-states filter.
const INVALID_VALUES_CODE: &str = "Invalid";

/// The first member of the operation's own input shape that the request did not carry,
/// if any. A member absent from a NESTED request structure is the generic validator's:
/// nothing has told EMR's own business logic about the request yet, exactly as for an
/// enum carried that deep.
fn absent_input_member(violations: &[Violation]) -> Option<&str> {
    violations.iter().find_map(|violation| {
        if !matches!(violation.kind, ViolationKind::MissingRequiredMember) {
            return None;
        }
        match structure_steps(&violation.path).as_slice() {
            [member] => Some(*member),
            _ => None,
        }
    })
}

/// The refusal for an input member the request did not carry: the refusal that member's
/// own value class carries on the surface the served operation reaches it through,
/// stated by the SAME sentence a value it cannot use gets, with
/// [`crate::state::ABSENT_VALUE`] in the value's place.
///
/// The members a `@required` member can be omitted on at all are the ones the model
/// marks `clientOptional` (`smithy_query(query="operation", operation="DescribeCluster")`
/// shows `ClusterId` as both), which is why the service — not the request validator —
/// answers for them. A member this service publishes no refusal class for is left to the
/// generic validator, whose envelope names it.
fn absent_member_refusal(
    served: ServedOperation,
    violations: &[Violation],
) -> Option<InvalidRequestException> {
    match absent_input_member(violations)? {
        CLUSTER_ID_MEMBER => Some((served.cluster_id_refusal)(crate::state::ABSENT_VALUE)),
        RELEASE_LABEL_MEMBER => Some(crate::state::invalid_release_label(
            crate::state::ABSENT_VALUE,
        )),
        _ => None,
    }
}

/// The refusal for values outside a modelled enum on the operation's own input shape:
/// the discriminator names the member whose values were rejected, in the casing the
/// model spells it ([`INVALID_VALUES_CODE`]), beside the sentence that enumerates them.
fn rejected_enum_values_refusal(violations: &[Violation]) -> Option<InvalidRequestException> {
    let on_input_member = input_member_enum_violations(violations);
    let first = on_input_member.first()?;
    let member = first.path.split('.').next()?;
    let message = match rejected_values_message(&on_input_member) {
        Some(sentence) => sentence,
        None => aws_smithy_schema_server::envelope(violations)?,
    };
    Some(InvalidRequestException {
        error_code: Some(format!("{INVALID_VALUES_CODE}{}", opening(member))),
        message: Some(message),
    })
}

/// The refusal the served operation would have answered for the `JobFlowRole` the request
/// carries, when that value is one it cannot launch a cluster with. `None` when the
/// operation resolves no instance profile, when the request carries no `JobFlowRole` — the
/// absent member is its own refusal, which the handler answers — or when the value names
/// a profile this account holds and Amazon EC2 may assume.
///
/// Amazon EMR RESOLVES that profile before it checks the request's other members, while
/// the framework checks every member before the handler runs — so this restores the order
/// for a request that gets both wrong, exactly as `cluster_first_response` does for a
/// cluster id. The classes and the sentences are the handler's own
/// ([`crate::state::instance_profile_violation`]), so the two orders cannot word one
/// refusal two ways; only the envelope is chosen here.
async fn job_flow_role_first_response(
    operation: &str,
    svc: &dyn Service,
    req: &aws_smithy_runtime_api::http::Request,
    ctx: &OperationContext<'_>,
) -> Option<aws_smithy_runtime_api::http::Response> {
    if operation != RESOLVES_JOB_FLOW_ROLE {
        return None;
    }
    let job_flow_role = request_member(req, JOB_FLOW_ROLE).filter(|role| !role.is_empty())?;
    let region = crate::state::request_region(ctx.parts());
    match crate::state::instance_profile_violation(ctx, &region, &job_flow_role).await? {
        crate::state::JobFlowRoleRefusal::Invalid(message) => {
            validation_refusal_response(svc, req, message)
        }
        crate::state::JobFlowRoleRefusal::Unauthorized(message) => {
            access_denied_refusal_response(svc, req, message)
        }
    }
}

/// The `com-amazonaws-emr-elasticmapreduce` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// Borrowed by `override_operation` so the operation being served is in scope on the
    /// refusal path, and so a refusal that depends on this service's own state can be
    /// answered there. It reaches the real operation through `svc`.
    refusal_scope: OperationScopedRefusal<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            refusal_scope: OperationScopedRefusal { engine },
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
        // that carries its own facts into `override_refusal`. The routing
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
        // Amazon EMR checks two classes of its own input itself, ahead of the generic
        // request validator, and refuses both with its own modelled client-input error
        // `InvalidRequestException` ("This exception occurs when there is something
        // wrong with user input", HTTP 400) — the identity an SDK client branching
        // on `emr.exceptions.InvalidRequestException` catches. The framework's
        // default code here, the synthetic `smithy.framework#ValidationException`,
        // is an identity no EMR operation declares, so such a client would see only
        // a generic `ClientError`:
        //
        // - a value outside the value set of an ENUM-valued member of the operation's
        //   own input shape, including the elements of its collections
        //   (`clusterStates.1.member`);
        // - an input member the request did not carry AT ALL, for a member whose
        //   refusal class EMR publishes a code for: the model marks those members
        //   `clientOptional` beside `@required`, so a client omits them and the
        //   SERVICE, not the request validator, answers.
        //
        // Both reach the members of the input shape, and only those. A value carried
        // deeper, inside a nested request structure, is reached by the GENERIC request
        // validator instead and keeps its `ValidationException`: nothing has told EMR's
        // own business logic about the request yet. `structure_steps` is that
        // distinction.
        //
        // And the identity is the SERVED OPERATION's to declare: the legacy job-flow
        // and policy families name no client error at all, so there the generic
        // identity stands too, whatever the violation. `ServedOperation`
        // reads that fact off the operation the request routed to.
        //
        // Every OTHER modelled constraint — a `@length`, a `@range` — is likewise left
        // to the generic validator's `ValidationException`, which is what EMR is
        // recorded answering for one (`RunJobFlow`'s `ManagedScalingPolicy` bounds, and
        // the refusals its own handler raises). A request violating both classes at
        // once is refused as EMR's own, because that is the check it runs first.
        //
        // The MESSAGE is chosen separately from the code, because EMR words these
        // refusals itself:
        //
        // - rejected values that are elements of ONE collection-valued filter member
        //   are enumerated, in the order the request spelled them: that sentence is
        //   the only part of the refusal telling a caller WHICH of several submitted
        //   values was rejected;
        // - an absent member is stated by the sentence an unusable value of it gets, in
        //   EITHER envelope — an operation that cannot answer EMR's own error still
        //   answers EMR's sentence, under the generic code;
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
        let Some(served) = served_operation() else {
            // No operation is being served, so no operation's facts apply.
            return Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: cloud_watch_alarm_message(violations)?,
            });
        };
        // The sentence for an input member the request did not carry is that member's
        // class's in EITHER envelope: an operation that declares no client error still
        // answers it, under the generic code — which is what EMR is recorded answering
        // for `GetManagedScalingPolicy` called with no `ClusterId`.
        let absent = absent_member_refusal(served, violations);
        let chosen = served
            .models_client_error
            .then(|| {
                absent
                    .clone()
                    .or_else(|| rejected_enum_values_refusal(violations))
            })
            .flatten();
        // What was refused travels to `OperationScopedRefusal::run`, which answers it:
        // this hook can name a code and a sentence and nothing else, while EMR's own
        // error carries an `ErrorCode` beside its `Message`, and the cluster a
        // cluster-scoped request names is resolved ahead of its other members.
        remember_framework_refusal(FrameworkRefusal {
            refused: true,
            on_cluster_id: violations
                .iter()
                .any(|violation| structure_steps(&violation.path) == [CLUSTER_ID_MEMBER]),
            chosen: chosen.clone(),
        });
        if let Some(chosen) = chosen {
            return Some(Refused {
                code: INVALID_REQUEST_EXCEPTION.to_string(),
                message: chosen.message.unwrap_or_default(),
            });
        }
        Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message: match absent {
                Some(absent) => absent.message.unwrap_or_default(),
                None => cloud_watch_alarm_message(violations)?,
            },
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
        member_words, models_invalid_request_exception, structure_steps, ServedOperation,
        NO_INVALID_REQUEST_EXCEPTION, RESOLVES_CLUSTER_ID,
    };

    /// Each operation words a `ClusterId` it cannot use through the surface it reaches a
    /// cluster by: the cluster surface quotes the id, the legacy job-flow surface names a
    /// job flow, and the session-credentials read states the cluster-id sentence with no
    /// id in it. Rows span all three.
    #[test]
    fn an_absent_cluster_id_is_refused_by_its_operations_surface() {
        let cases = [
            ("DescribeCluster", "MalformedClusterId", "Cluster id 'null' is not valid."),
            ("ListSteps", "MalformedClusterId", "Cluster id 'null' is not valid."),
            ("GetSession", "MalformedClusterId", "Cluster id 'null' is not valid."),
            (
                "AddInstanceFleet",
                "INVALID_JOB_ID_MALFORMED",
                "Specified job flow ID not valid.",
            ),
            (
                "GetClusterSessionCredentials",
                "MalformedClusterId",
                "Cluster id is not valid.",
            ),
        ];
        for (operation, code, sentence) in cases {
            let refusal = (ServedOperation::of(operation).cluster_id_refusal)(
                crate::state::ABSENT_VALUE,
            );
            assert_eq!(
                refusal.error_code.as_deref(),
                Some(code),
                "case {operation:?}: expected ErrorCode {code:?}, got {:?}",
                refusal.error_code
            );
            assert_eq!(
                refusal.message.as_deref(),
                Some(sentence),
                "case {operation:?}: expected {sentence:?}, got {:?}",
                refusal.message
            );
        }
    }

    /// The table is searched with `binary_search`, so it must be sorted and hold no
    /// duplicate.
    #[test]
    fn the_operations_that_resolve_a_cluster_id_are_sorted() {
        assert!(
            RESOLVES_CLUSTER_ID.windows(2).all(|pair| pair[0] < pair[1]),
            "case {RESOLVES_CLUSTER_ID:?}: the table must be sorted and hold no duplicate"
        );
    }

    /// Every operation that resolves a cluster id can answer EMR's own client error, so
    /// the refusal it would answer is one the operation declares.
    #[test]
    fn every_operation_that_resolves_a_cluster_id_models_the_client_error() {
        for operation in RESOLVES_CLUSTER_ID {
            assert!(
                models_invalid_request_exception(operation),
                "case {operation:?}: an operation that answers the cluster-id refusal must \
                 declare the client error it arrives in"
            );
        }
    }

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
