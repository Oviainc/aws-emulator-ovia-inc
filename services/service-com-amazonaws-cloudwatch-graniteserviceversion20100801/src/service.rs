// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-cloudwatch-graniteserviceversion20100801` service: how it claims requests and how it words the
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
//! - `override_operation` — EDITED HERE: not a re-route. The key and the handler stay the
//!   generated routing table's, but every operation is served through [`Metering`], so each
//!   call this service answers is counted into the usage telemetry it publishes about its own
//!   API; `PutMetricData` is served through [`ReportingDroppedEntities`], so an accepted call
//!   whose entity this service could not hold says so in its response header, and
//!   `GetMetricStatistics` through [`RefusingValuesOutsideAnEnum`], so a value outside an
//!   enumerated type is refused as one of the parameter faults ITS model declares, and
//!   `PutMetricStream` through [`RefusingAValueFirst`], so the two members CloudWatch judges
//!   as parameter VALUES are judged before the model's own shape checks.
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
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    OperationError, OperationRoutingTable, Refusal, Refused, ServerHttpProtocol, Service,
};
use std::sync::Arc;

/// Run `work` without occupying an async worker.
///
/// The alarm-evaluation pass ([`crate::state::alarm_actions::settle_alarms`]) is CPU-bound
/// and holds this service's store lock for its whole span, so it must not run on the thread
/// the runtime serves other requests with. `block_in_place` moves the current task off that
/// worker, which keeps the runtime serving while the pass runs; it is available only on a
/// multi-thread runtime — the one the emulator boots — so on a current-thread runtime (a
/// `#[tokio::test]`, or a caller driving the router in-process) the work runs inline, where
/// there is no other worker to protect.
pub(crate) fn off_the_async_worker<T>(work: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current().map(|handle| handle.runtime_flavor()) {
        Ok(tokio::runtime::RuntimeFlavor::MultiThread) => tokio::task::block_in_place(work),
        _ => work(),
    }
}

/// The `com-amazonaws-cloudwatch-graniteserviceversion20100801` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The operations whose ANSWER this file owns a part of — the one whose accepted response
    /// carries something its body cannot ([`ReportingDroppedEntities`]), and the one metric
    /// read whose model declares a parameter fault to refuse a value outside an enumerated
    /// type with ([`RefusingValuesOutsideAnEnum`]), and the metric-stream writer whose own
    /// parameter-value rules are judged ahead of the boundary walk ([`RefusingAValueFirst`]).
    /// Each is built from the SAME shared engine
    /// as the generated table's and keyed by the routing key the protocol resolves, so the
    /// operation served is the modelled one, and each is METERED ([`Metering`]) like every
    /// other operation of this service.
    overridden: OperationRoutingTable,
    /// Every OTHER modelled operation, served through the generated table and metered
    /// ([`Metering`] over [`TheModelledOperation`]): the call count this service publishes
    /// about its own API is owed by every operation, so the seam that counts a call is the one
    /// every request passes through rather than a line in each handler.
    metered: Metering<Eng, TheModelledOperation>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut overridden = OperationRoutingTable::new();
        overridden.add_operation(
            "PutMetricData",
            Box::new(Metering::new(
                Arc::clone(&engine),
                ReportingDroppedEntities::new(crate::ops::put_metric_data::PutMetricData::new(
                    Arc::clone(&engine),
                )),
            )) as Box<dyn ErasedOperation>,
        );
        // `GetMetricStatistics` validates the SAME enumerated types as the retrieving read and
        // answers a value outside one as one of the parameter faults its model declares, which
        // the refusal seam cannot do because it cannot see which operation was called
        // ([`RefusingValuesOutsideAnEnum`]); every other operation keeps this service's own
        // validation code ([`scalar_enum_refusal`]).
        overridden.add_operation(
            "GetMetricStatistics",
            Box::new(Metering::new(
                Arc::clone(&engine),
                RefusingValuesOutsideAnEnum::new(
                    crate::ops::get_metric_statistics::GetMetricStatistics::new(Arc::clone(
                        &engine,
                    )),
                    parameter_fault_for,
                ),
            )) as Box<dyn ErasedOperation>,
        );
        // `PutMetricStream` judges the two members CloudWatch judges as parameter VALUES —
        // the IAM role the stream delivers through and the stream's own name — BEFORE the
        // framework's constraint walk, so one of those refusals wins over a modelled
        // constraint the walk would otherwise answer first ([`RefusingAValueFirst`]).
        overridden.add_operation(
            "PutMetricStream",
            Box::new(Metering::new(
                Arc::clone(&engine),
                RefusingAValueFirst::new(
                    crate::ops::put_metric_stream::PutMetricStream::new(Arc::clone(&engine)),
                    crate::ops::put_metric_stream::parameter_value_refusal,
                ),
            )) as Box<dyn ErasedOperation>,
        );
        Self {
            metered: Metering::new(Arc::clone(&engine), TheModelledOperation),
            entry: crate::ServiceDefinition::new(engine),
            overridden,
        }
    }
}

/// The refusal classes CloudWatch answers as one of its own parameter faults, and the whole
/// rewording this service does.
///
/// A constraint the boundary validator catches is refused before any handler runs, so the
/// framework answers it under the protocol's generic `ValidationException`. CloudWatch's
/// metric operations model the parameter faults instead — `InvalidParameterValueException` is
/// the only client error `ListMetrics` declares, and `GetMetricStatistics` declares it beside
/// the combination and missing-parameter faults — so these two classes are stated under that
/// code, in this service's own parameter wording ([`crate::state`]'s shared sentences, the
/// same ones its handlers refuse with), and a client branching on the code reaches its
/// handler:
///
/// * a `Dimensions` collection longer than its cap, named with the cap it broke
///   ([`dimension_cap_refusal`]);
/// * a value inside a LIST that is outside the enumerated type the list's member names — a
///   percentile passed in `Statistics`, whose place is `ExtendedStatistics` — named as the
///   request member path it arrived on and the value set the type declares;
/// * a SCALAR member outside the enumerated type it is modelled as, which keeps the
///   framework's own clause and carries this service's validation code
///   ([`scalar_enum_refusal`]).
///
/// Every other violation keeps the framework's code and wording: a scalar outside its own
/// LENGTH or range bound is what this service is recorded answering `ValidationException`
/// for, and a code a client branches on is not changed without a recording that shows it.
///
/// This seam cannot see WHICH operation was called, and which validator answers a value
/// outside an enumerated type is an operation's property: the read whose model declares the
/// parameter faults answers one of THEM rather than the code above, so it is served through
/// [`RefusingValuesOutsideAnEnum`] and never reaches here with such a violation.
fn parameter_fault(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    violations
        .iter()
        .find_map(dimension_cap_refusal)
        .or_else(|| violations.iter().find_map(list_element_enum_refusal))
        .or_else(|| scalar_enum_refusal(violations))
}

/// The refusal of a SCALAR member carrying a value outside the enumerated type it is
/// modelled as, or `None` when no violation of the refusal is one.
///
/// CloudWatch judges a request against its model before any handler runs and answers what it
/// found under its OWN validation code ([`VALIDATION_ERROR`], the code
/// [`crate::scaffolded::Scaffolded::validation_error`] already answers this service's
/// handler-side validation refusals with), so a client branches on one code whichever layer
/// refused the request. Only the code is this service's: the message is the framework's own
/// counting envelope, naming the member path and the values the type admits, so the two
/// cannot drift.
fn scalar_enum_refusal(violations: &[aws_smithy_schema_server::Violation]) -> Option<Refused> {
    if !violations.iter().any(|violation| {
        matches!(
            violation.kind,
            aws_smithy_schema_server::ViolationKind::EnumValue(_)
        ) && !is_list_element(&violation.path)
    }) {
        return None;
    }
    Some(Refused {
        code: VALIDATION_ERROR.to_string(),
        message: aws_smithy_schema_server::envelope(violations)?,
    })
}

/// The refusal of a `Dimensions` collection longer than the dimensions it may carry, or
/// `None` when `violation` is not one.
///
/// The cap CloudWatch enforces on a dimension collection is the per-metric dimension maximum
/// ([`crate::state::MAX_METRIC_DIMENSIONS`]), whatever the operation; `ListMetricsInput`'s own
/// filter list is modelled BELOW it (`length 0..10`), and a collection inside the per-metric
/// maximum but outside that member's own bound is refused naming THAT bound — the refusal
/// names the cap the collection actually broke, and this seam chooses the words of a refusal
/// rather than whether there is one.
fn dimension_cap_refusal(violation: &aws_smithy_schema_server::Violation) -> Option<Refused> {
    let aws_smithy_schema_server::ViolationKind::MaxLength(declared) = violation.kind else {
        return None;
    };
    if !is_dimension_collection(&violation.path) {
        return None;
    }
    let submitted =
        service_common::validation_message::list_item_count(violation.value.as_deref()?)?;
    let cap = if submitted > crate::state::MAX_METRIC_DIMENSIONS {
        crate::state::MAX_METRIC_DIMENSIONS
    } else {
        usize::try_from(declared).ok()?
    };
    Some(Refused {
        code: INVALID_PARAMETER_VALUE.to_string(),
        message: crate::state::collection_too_large_message(
            &crate::state::query_parameter_path(&violation.path),
            cap,
        ),
    })
}

/// The refusal of an element of a LIST holding a value outside the enumerated type the list's
/// member names, or `None` when `violation` is not one.
///
/// Stated the way CloudWatch states a value outside the set a member accepts
/// ([`crate::state::value_in_set_message`]): the request member path the element arrived on as
/// a query-protocol refusal names it, down to the list's own target shape
/// (`Statistics.member.1.<list element>`), and the whole value set the enumerated type
/// declares — which is what lets a client repair the request. The submitted value is NOT
/// echoed; the set it had to come from is what locates the repair.
fn list_element_enum_refusal(violation: &aws_smithy_schema_server::Violation) -> Option<Refused> {
    is_list_element(&violation.path)
        .then(|| enum_value_refusal(violation))
        .flatten()
}

/// The refusal of a value outside the enumerated type its member targets, as an operation that
/// MODELS this service's parameter faults states it, or `None` when `violation` is not one.
///
/// One sentence for every such value ([`crate::state::value_in_set_message`]): the request
/// member path it arrived on and the whole value set the type declares, and nothing of the
/// submitted value — the set is what lets a client repair the request. The path is the one the
/// validating walk found, spelled as this service's refusals spell a request member
/// ([`crate::state::query_parameter_path`]), with the step into a list's own target shape added
/// for an ELEMENT of a list, which has no member name of its own.
fn enum_value_refusal(violation: &aws_smithy_schema_server::Violation) -> Option<Refused> {
    let aws_smithy_schema_server::ViolationKind::EnumValue(ref allowed) = violation.kind else {
        return None;
    };
    let member = crate::state::query_parameter_path(&violation.path);
    let parameter = if is_list_element(&violation.path) {
        crate::state::list_target_path(&member)
    } else {
        member
    };
    Some(Refused {
        code: INVALID_PARAMETER_VALUE.to_string(),
        message: crate::state::value_in_set_message(&parameter, allowed),
    })
}

/// The wire error code of the modelled `InvalidParameterValueException` ("The value of an
/// input parameter is bad or out-of-range"): its `awsQueryError` code, which is what a client
/// of this service branches on.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The wire error code CloudWatch answers a request its model refuses with — the code this
/// service's own validation refusals carry (`crate::scaffolded`), so a boundary refusal and a
/// handler refusal of the same class read alike.
const VALIDATION_ERROR: &str = "ValidationError";

/// Whether `path` points at a request's `Dimensions` collection — of any operation, and at
/// any depth, since a metric's dimension set is one shape wherever it is submitted.
fn is_dimension_collection(path: &str) -> bool {
    path.split('.').next_back() == Some("dimensions")
}

/// Whether `path` points at an ELEMENT of a list rather than at a member of its own: the
/// validating walk ends such a path with the step from the collection into the element.
fn is_list_element(path: &str) -> bool {
    path.split('.').next_back() == Some("member")
}

/// The status a refused request value carries: a value the model does not admit is the
/// caller's to repair, so every class below is a client error.
const REFUSAL_STATUS: u16 = 400;

/// How an operation whose model DECLARES this service's parameter faults states a boundary
/// refusal, or `None` when nothing in `violations` is one of the classes it words.
///
/// `GetMetricStatistics` declares `InvalidParameterValueException` ("The value of an input
/// parameter is bad or out-of-range", `awsQueryError` code [`INVALID_PARAMETER_VALUE`]) beside
/// the combination and missing-parameter faults, so a client of it branches on that code and
/// the sentence is this service's own: the dimension cap first and then a value outside an
/// enumerated type, in the SAME order as the service-wide seam ([`parameter_fault`]), so one
/// operation cannot answer a request that breaks both differently from every other. An
/// operation that declares none of those faults keeps the service's own validation code
/// instead ([`scalar_enum_refusal`]).
fn parameter_fault_for(violations: &[aws_smithy_schema_server::Violation]) -> Option<Refused> {
    violations
        .iter()
        .find_map(dimension_cap_refusal)
        .or_else(|| violations.iter().find_map(enum_value_refusal))
}

/// One modelled operation, served so that a request value outside the enumerated type its
/// member targets is refused by THIS operation's validator rather than by the framework's
/// generic one.
///
/// Which code such a refusal carries is a property of the OPERATION — an operation that models
/// this service's parameter faults answers one of them, where an operation that models none
/// keeps this service's own validation code ([`scalar_enum_refusal`]) — and
/// `Service::override_refusal` is handed the violations alone, with no way to tell which
/// operation was called. So the choice is made here, where the operation is known, by the
/// `refusal` policy this wrapper is mounted with ([`parameter_fault_for`]).
///
/// Nothing else is re-implemented: a request this policy does not refuse is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle ->
/// serialize and every modelled error, and the violations the policy reads come from the
/// framework's OWN deserializer and constraint walk over the same input schema, so this and
/// the walk behind it cannot find a different refusal.
struct RefusingValuesOutsideAnEnum<O> {
    operation: O,
    /// How this operation states the violations its validator found, or `None` to leave the
    /// request to the operation itself.
    refusal: fn(&[aws_smithy_schema_server::Violation]) -> Option<Refused>,
}

impl<O> RefusingValuesOutsideAnEnum<O> {
    /// Wrap the operation, already constructed from the shared engine, with the policy that
    /// words its boundary refusal.
    fn new(
        operation: O,
        refusal: fn(&[aws_smithy_schema_server::Violation]) -> Option<Refused>,
    ) -> Self {
        Self { operation, refusal }
    }
}

impl<O> ErasedOperation for RefusingValuesOutsideAnEnum<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // The protocol is read, used and dropped inside this one statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let refused = {
                let protocol = svc.definition().protocol();
                // The violations come from the framework's OWN deserializer and constraint
                // walk, collected by the one shared helper every service that words its own
                // pre-handler refusal uses, so this and the walk that follows it cannot find
                // a different set. A request the codec cannot read at all yields none, and so
                // falls through to the framework's own refusal.
                let violations =
                    service_common::validation::refusing_violations::<O>(protocol, req);
                (self.refusal)(&violations)
                    .and_then(|refused| refusal_response(protocol, &refused))
            };
            match refused {
                Some(response) => response,
                None => self.operation.run(svc, req, ctx).await,
            }
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// One modelled operation, served so that this service's own PARAMETER-VALUE rules are
/// judged BEFORE the framework's constraint walk.
///
/// The framework's per-request sequence is deserialize -> validate -> handle, so a handler
/// never sees a request that breaks a modelled constraint — and a request that breaks BOTH a
/// modelled constraint and one of the rules the service judges values by is answered for the
/// constraint, under the generic validation code, when CloudWatch answers the parameter-value
/// fault. A caller repairing what it was told about is then refused again for the fault it was
/// never shown. This wrapper takes the value decision on the way IN and answers it with the
/// operation's OWN modelled error.
///
/// Nothing else is re-implemented: a request the policy does not refuse is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle ->
/// serialize and every modelled error, and the refusal comes from the operation's own file
/// (`crate::ops::put_metric_stream::parameter_value_refusal`, which its handler calls too), so
/// the answer here and the answer behind it are one decision. The cost is DELIBERATE: such a
/// request is read twice, because the framework hands no already-built input to an operation.
struct RefusingAValueFirst<O: Operation, E> {
    operation: O,
    /// The modelled error this operation owes for the values the request carries, or `None` to
    /// leave the request to the operation itself.
    refusal: fn(&O::Input) -> Option<E>,
}

impl<O: Operation, E> RefusingAValueFirst<O, E> {
    /// Wrap the operation, already constructed from the shared engine, with the policy that
    /// judges the values its input carries.
    fn new(operation: O, refusal: fn(&O::Input) -> Option<E>) -> Self {
        Self { operation, refusal }
    }
}

impl<O, E> ErasedOperation for RefusingAValueFirst<O, E>
where
    O: Operation + ErasedOperation,
    E: Into<O::Error> + Send + Sync + 'static,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // The protocol is read, used and dropped inside this one statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let refused = {
                let protocol = svc.definition().protocol();
                read_input::<O>(protocol, req)
                    .and_then(|input| (self.refusal)(&input))
                    .map(|refusal| modelled_error_response(protocol, req, &refusal.into()))
            };
            match refused {
                Some(response) => response,
                None => self.operation.run(svc, req, ctx).await,
            }
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// `req` read into `O`'s input shape through the service's own protocol, or `None` when it
/// does not READ as that input at all.
///
/// Deliberately NOT validated: the whole point of the wrapper above is to judge a value the
/// constraint walk would have refused the request for first. A body no codec can read, or one
/// missing a required member, yields `None` and is left to the framework's own refusal, which
/// is the only answer there is for a request that names no values to judge.
fn read_input<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &aws_smithy_runtime_api::http::Request,
) -> Option<O::Input> {
    let mut deserializer = protocol.deserialize_input(req, O::INPUT_SCHEMA).ok()?;
    let read = <O::Input as DeserializableStruct>::deserialize(&mut *deserializer);
    drop(deserializer);
    read.ok()
}

/// The header an `awsQueryCompatible` service reports the legacy query error code in, for a
/// client that asked for it.
const QUERY_ERROR_HEADER: &str = "x-amzn-query-error";

/// The header a query-compatible SDK opts into that report with.
const QUERY_MODE_HEADER: &str = "x-amzn-query-mode";

/// `error` rendered as this service answers a modelled failure: the error shape through the
/// service's own protocol, under its wire code and its modelled status
/// (`service_common::event_stream::error_response`, the shared renderer), plus the
/// `awsQueryCompatible` header when the client asked for the legacy code.
///
/// Rendered from the SAME facts the per-operation sequence renders a modelled error from
/// ([`OperationError`]), so a refusal answered ahead of that sequence and the same refusal
/// answered from the handler are one response. The header is added HERE because the framework
/// adds it inside the sequence this refusal precedes; `awsQueryCompatible` carries the legacy
/// code in it rather than in the body, and the body's own code is the shape name on this
/// service's protocol (`awsJson1_0`).
fn modelled_error_response<E: OperationError>(
    protocol: &dyn ServerHttpProtocol,
    req: &aws_smithy_runtime_api::http::Request,
    error: &E,
) -> aws_smithy_runtime_api::http::Response {
    let mut response = service_common::event_stream::error_response(protocol, error);
    // A query-compat SDK opts in with `x-amzn-query-mode: true`, and only then is the legacy
    // query error code and its fault reported in the header; a native awsJson client gets the
    // unadorned awsJson error.
    let query_mode = req
        .headers()
        .get(QUERY_MODE_HEADER)
        .is_some_and(|value| value.eq_ignore_ascii_case("true"));
    if query_mode {
        if let Some(code) = error.query_error_code() {
            let fault = if response.status().as_u16() >= 500 {
                "Receiver"
            } else {
                "Sender"
            };
            response
                .headers_mut()
                .insert(QUERY_ERROR_HEADER, format!("{code};{fault}"));
        }
    }
    response
}

/// `refused` rendered in this service's own error envelope: its code and its one `message`,
/// with the status every refused request value carries ([`REFUSAL_STATUS`]).
///
/// The body is the single-`message` shape every refusal of this service that names no modelled
/// error renders through (`crate::scaffolded`), which is also all the refusal SEAM can answer
/// with — a code and a message and nothing else — so a refusal worded here and one the seam
/// words read alike; `None` when the envelope cannot be built, which leaves the request to the
/// operation and so to the framework's own refusal.
fn refusal_response(
    protocol: &dyn ServerHttpProtocol,
    refused: &Refused,
) -> Option<aws_smithy_runtime_api::http::Response> {
    protocol
        .serialize_error(
            &crate::scaffolded::ValidationException::new(refused.message.clone()),
            &crate::scaffolded::VALIDATION_EXCEPTION_SCHEMA,
            &refused.code,
            REFUSAL_STATUS,
        )
        .ok()
}

/// The response header an ACCEPTED `PutMetricData` reports a dropped entity in.
///
/// `PutMetricDataInput$StrictEntityValidation` documents that such an answer carries "an
/// additional response header [which] will contain information about the validation errors"
/// and enumerates the values that information is drawn from, but it does not name the header
/// itself: this name, and carrying the bare failure name as its value, are INFERRED from what
/// a lenient call is recorded answering, not quoted from the model.
const ENTITY_FAILURE_HEADER: &str = "x-amz-failure-message";

/// `PutMetricData`, served so that an accepted call which DROPPED an entity says so.
///
/// "In the case of an invalid entity, the operation will return a 200 status, but an
/// additional response header will contain information about the validation errors."
/// (`PutMetricDataInput$StrictEntityValidation`) — the accepted answer's shape is `Unit`, so
/// it binds no member the handler could put that on, and the header is added here, the one
/// place a response can still be touched. Nothing else is re-implemented: the request is
/// handed to the generated [`ErasedOperation::run`], which owns deserialize -> validate ->
/// handle -> serialize and every modelled error, and the failure the header names comes from
/// the operation's OWN entity validation
/// (`crate::ops::put_metric_data::dropped_entity_failure`), so the two cannot disagree.
struct ReportingDroppedEntities<O> {
    operation: O,
}

impl<O> ReportingDroppedEntities<O> {
    /// Wrap the operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for ReportingDroppedEntities<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // The protocol is read, used and dropped inside this one statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`. The request is read through the
            // service's OWN protocol into the operation's OWN input shape, and judged by the
            // operation's OWN entity validation, so what the header reports and what the
            // handler did cannot disagree. The cost is DELIBERATE: a request is read twice,
            // once here and once in the operation's own sequence, because the framework
            // hands no already-built input to an operation.
            let dropped = service_common::event_stream::deserialize_request::<
                crate::ops::types::PutMetricDataInput,
            >(
                svc.definition().protocol(),
                req,
                crate::ops::types::PUT_METRIC_DATA_INPUT_SCHEMA,
            )
            .ok()
            .and_then(|input| crate::ops::put_metric_data::dropped_entity_failure(&input));
            let mut response = self.operation.run(svc, req, ctx).await;
            // Only an ACCEPTED call drops an entity: a refused one ingested nothing, so it
            // has no accepted metric data to report an unassociated entity for.
            //
            // The failure is one of a FIXED set of names, never text from the request, so it
            // is always a representable header value; the fallible insert is used all the
            // same, so that a header which could not be built could only cost the answer its
            // header and never the answer itself.
            if response.status().is_success() {
                if let Some(failure) = dropped {
                    let _ = response
                        .headers_mut()
                        .try_insert(ENTITY_FAILURE_HEADER, failure);
                }
            }
            response
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// One operation, served so that the call this service answers is COUNTED into the usage
/// telemetry it publishes about its own API ([`crate::state::usage`]).
///
/// Every operation of this service is served through one of these, so a call is counted
/// wherever it was answered from — the handler, a modelled refusal, or the boundary refusal
/// one of the wrappers above words. The count is taken AFTER the answer is built and is
/// written off it: the response a caller sees is the wrapped operation's own, and a count that
/// could not be stored costs the telemetry a datapoint and never the answer.
///
/// The operation counted is named by the routing key the service's OWN protocol resolves —
/// the modelled operation's shape name, the same key the generated routing table is read with
/// — so the series a reader asks for by operation name is the series the call was counted in.
struct Metering<Eng, O> {
    /// The shared engine, so the count reaches the same store every handler of this service
    /// writes ([`crate::state::service_state`]).
    engine: Arc<Eng>,
    /// The operation whose answer this one is.
    operation: O,
}

impl<Eng, O> Metering<Eng, O> {
    /// Meter the operation, already constructed from the shared engine.
    fn new(engine: Arc<Eng>, operation: O) -> Self {
        Self { engine, operation }
    }
}

impl<Eng, O> ErasedOperation for Metering<Eng, O>
where
    Eng: Send + Sync + 'static,
    O: ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // The protocol is read, used and dropped inside this one statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let operation = svc
                .definition()
                .protocol()
                .routing_key(req)
                .ok()
                .map(ToString::to_string);
            let response = self.operation.run(svc, req, ctx).await;
            // A served call is counted whatever it answered: "every request" includes the
            // refused ones, which are requests the API answered.
            if let Some(operation) = operation {
                crate::state::usage::count_api_call(&*self.engine, &operation);
            }
            response
        })
    }

    /// The wrapped operation's modelled auth schemes: metering a call must not change which
    /// schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// The modelled operation this service's generated routing table holds for the request's own
/// routing key: what the request would have been served by had this file claimed nothing.
///
/// It re-implements NOTHING — it resolves the same key the router would have resolved and runs
/// the generated table's own operation — and exists only so that the metering above can wrap
/// every operation of this service without this file naming all 55 of them.
struct TheModelledOperation;

impl ErasedOperation for TheModelledOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // The definition is read and dropped inside this one statement, as above: the
            // operation it hands back is `Send`, the definition reference is not.
            let modelled = {
                let definition = svc.definition();
                definition
                    .protocol()
                    .routing_key(req)
                    .ok()
                    .and_then(|key| definition.routing_table().get(key))
            };
            match modelled {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable through `override_operation`, which hands a request here only
                // once the generated table has answered for its key; a request whose key the
                // table does not name is the framework's own routing failure, worded by this
                // service's entry exactly as it would be had nothing claimed the key.
                None => svc.definition().routing_error_response(req),
            }
        })
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
        // NOT a re-route: the key is the one the generated table's own protocol resolves, and
        // the operation it names is the one served — `PutMetricData` wrapped in
        // [`ReportingDroppedEntities`] so an accepted call reports the entity it dropped, and
        // `GetMetricStatistics` wrapped in [`RefusingValuesOutsideAnEnum`] so a value outside
        // an enumerated type is refused as one of the parameter faults its model declares —
        // where a refusal of such a value by any OTHER operation keeps this service's own
        // validation code ([`scalar_enum_refusal`]), and `PutMetricStream` wrapped in
        // [`RefusingAValueFirst`] so the values its model does not constrain to the form the
        // service requires are judged before the boundary walk.
        //
        // Every modelled key resolves here, because every call this service answers is counted
        // into the usage telemetry it publishes about its own API ([`Metering`]): a key the two
        // wrappers above do not claim is served by the generated table's own operation
        // ([`TheModelledOperation`]), metered. A key the table does not name at all resolves to
        // `None`, exactly as it would in an unedited service, and falls through to the
        // framework's routing failure.
        let key = self.entry.protocol().routing_key(req).ok()?;
        if let Some(operation) = self.overridden.get(key) {
            return Some(operation);
        }
        self.entry
            .routing_table()
            .get(key)
            .map(|_| &self.metered as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent required
        // member, or a value outside a declared bound) to let this service reword it. Only
        // the classes CloudWatch answers as one of its own PARAMETER faults are restated
        // ([`parameter_fault`]); everything else keeps the framework's code and wording.
        parameter_fault(refusal)
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
