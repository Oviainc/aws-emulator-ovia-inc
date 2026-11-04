// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-xray-awsxray` service: how it claims requests and how it words the
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
//!   correctly. Edited here: a request whose BODY cannot be read into the
//!   modelled input shape is answered by [`request_layer`]'s refusal, because
//!   that failure is the deserialization layer's and not a handler's.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran, for the refusals that do not reach
//!   [`request_layer`]'s pre-dispatch answer (an absent `@required` member is
//!   found while READING the body, so it arrives here). Edited here: a value that violates a MODELLED
//!   constraint (a `@length`, a `@range`, an absent `@required` member, a value
//!   outside an enum) is refused by the request layer, and AWS answers that
//!   class with the protocol-level `ValidationException` envelope — HTTP 400,
//!   code `ValidationException`, message "<N> validation error(s) detected:
//!   Value … at '<member>' failed to satisfy constraint: …" — never with an
//!   operation's own modelled error. The framework renders that envelope
//!   already; what this service delegates to `service-common` for is the one
//!   thing AWS words differently, how a `@pattern` requirement is quoted. A
//!   value the codec could not read AT ALL is a different class and is answered
//!   `SerializationException` through `override_operation` ([`request_layer`]).
//!   X-Ray's `InvalidRequestException` is what this SERVICE's handlers answer
//!   for a request they refuse themselves (a group name already in use, an
//!   unparseable filter expression, a group that does not exist), and they
//!   return it directly; mapping the boundary class onto it as well would hand a
//!   client one code for two different failures.
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

pub mod request_layer;

use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use smithy_server_runtime::{http, ErasedOperation, Refusal, Refused, Service};
use std::sync::Arc;

/// `refusal` with every enumerated value set stated in the order THIS service states
/// one, and everything else — the member paths, the values echoed, every other
/// requirement — untouched.
///
/// A refusal of a value outside an enumerated type names the values that type admits,
/// and the order it names them in is the SERVICE's rather than the model's: the order a
/// model happens to declare an enum's members in is an artefact of how the model was
/// written, and this service's own refusals state two of its types in orders the model
/// does not carry — the encryption type as `[KMS, NONE]` where the model declares
/// `NONE, KMS`, and the time-range type as `[TraceId, Service, Event]` where the model
/// declares `TraceId, Event, Service`. No one order over the declared values produces
/// both, so the sets this service is known to state in an order the sort does not give
/// are listed in [`STATED_VALUE_SET_ORDERS`], and every other set of this model takes
/// the sorted order — `service_common`'s one statement of that rule, applied to the
/// violations as they arrive, so it answers for every enum and every member targeting
/// one, in any operation.
///
/// Both of this service's refusal seams word their message through it — the framework's
/// own ([`EmulatedService::override_refusal`]) and the pre-dispatch one
/// ([`request_layer::ConstraintRefusal`]) — so the two cannot state one set two ways.
pub(crate) fn in_stated_value_order(refusal: &Refusal) -> Refusal {
    let Refusal::Invalid(violations) = refusal else {
        return refusal.clone();
    };
    Refusal::Invalid(violations.iter().map(in_this_services_order).collect())
}

/// The enumerated value sets this service states in an order the sort does not
/// give, each written in the order its own refusals state it.
///
/// Source: the refusals recorded for this service. The order belongs to the
/// TYPE — one set, one order, whatever member or operation carried the value —
/// so a set is recognised by the values it admits and by nothing about the
/// request.
const STATED_VALUE_SET_ORDERS: &[&[&str]] = &[
    // `com.amazonaws.xray#TimeRangeType`, which the model declares `TraceId,
    // Event, Service`.
    &["TraceId", "Service", "Event"],
];

/// One violation with its enumerated value set in the order this service states
/// it: the order recorded for that set when there is one, else the sorted order.
fn in_this_services_order(violation: &Violation) -> Violation {
    let sorted = service_common::validation_message::in_sorted_enum_order(violation);
    let ViolationKind::EnumValue(declared) = &sorted.kind else {
        return sorted;
    };
    match stated_order(declared) {
        Some(stated) => Violation {
            path: sorted.path.clone(),
            kind: ViolationKind::EnumValue(stated),
            value: sorted.value.clone(),
            sensitive: sorted.sensitive,
        },
        None => sorted,
    }
}

/// The order [`STATED_VALUE_SET_ORDERS`] states the set `declared` in, or `None`
/// for a set it does not name — which is every set this service's refusals are
/// not recorded stating out of sorted order.
fn stated_order(declared: &[String]) -> Option<Vec<String>> {
    STATED_VALUE_SET_ORDERS
        .iter()
        .find(|stated| {
            stated.len() == declared.len()
                && stated
                    .iter()
                    .all(|value| declared.iter().any(|admitted| admitted == value))
        })
        .map(|stated| stated.iter().map(|value| (*value).to_string()).collect())
}

/// The `com-amazonaws-xray-awsxray` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// Answers a request whose body cannot be read into the modelled input
    /// shape, which is a DESERIALIZATION failure rather than a constraint
    /// violation (see [`request_layer`]). Held on the service because
    /// [`Service::override_operation`] hands back a borrow of one of its own
    /// operations.
    unreadable_body: request_layer::UnreadableBody<Eng>,
    /// Answers a request whose values the model's own constraints refuse, with
    /// each clause echoing the value the CALLER submitted — including a
    /// collection-valued member, which the framework's walk can only summarise
    /// as `<n> items` (see [`request_layer`]). Held on the service for the same
    /// reason.
    constraint_refusal: request_layer::ConstraintRefusal<Eng>,
    /// Answers a request carrying a time member this service read and could not
    /// convert to an instant, naming the class of value that was sent and the
    /// form an instant is read in (see [`request_layer`]). Held on the service
    /// for the same reason.
    unconvertible_time: request_layer::UnconvertibleTime<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            unreadable_body: request_layer::UnreadableBody::new(),
            constraint_refusal: request_layer::ConstraintRefusal::new(),
            unconvertible_time: request_layer::UnconvertibleTime::new(),
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
        // A body this service cannot read into the modelled input shape is
        // refused FIRST, because deserialization precedes every later layer: a
        // value whose JSON type the modelled member cannot be read from never
        // becomes a value at all. The framework would otherwise report that
        // codec failure as a constraint violation (`ValidationException`), which
        // is a different error class, a different remediation for the caller,
        // and a different SDK error type — see [`request_layer`] for the whole
        // argument and for the refusal this answers instead. The cost is
        // deliberate: every request's body is read twice (once here, once by the
        // dispatch that follows), and a REFUSED one a third time when the
        // refusal re-derives its message — an emulator pays that to answer the
        // right error class, and nothing but the refusal path pays the third.
        //
        // A request that READ and then broke a modelled constraint is the other
        // class, and it is answered here too — by `ConstraintRefusal`, in the
        // same `ValidationException` envelope the framework renders — because an
        // AWS constraint clause QUOTES the value the caller submitted, and the
        // `override_refusal` seam below is handed the violations alone, never
        // the request they came from: a collection-valued member therefore
        // reaches it as the walk's `<n> items` summary. This seam still has the
        // built input, so the echo is restored where it can be.
        let protocol = self.entry.protocol();
        if let Ok(key) = protocol.routing_key(req) {
            match request_layer::refusal_for::<Eng>(key, protocol, req) {
                Some(request_layer::PreDispatchRefusal::Unreadable(_)) => {
                    return Some(&self.unreadable_body);
                }
                Some(request_layer::PreDispatchRefusal::UnconvertibleTime(_)) => {
                    return Some(&self.unconvertible_time);
                }
                Some(request_layer::PreDispatchRefusal::Invalid(_)) => {
                    return Some(&self.constraint_refusal);
                }
                None => {}
            }
        }
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The framework calls this on a refusal it made BEFORE any handler ran
        // — a value outside a modelled constraint, an absent `@required`
        // member, a value outside an enum. AWS answers that class as the
        // `ValidationException` envelope the framework already renders, with
        // one difference: a `@pattern` requirement is quoted WITHOUT the
        // anchors a whole-value pattern wraps it in, so a client that re-applies
        // the reported expression gets the one AWS reports. That rendering is
        // shared by every service on a JSON protocol, so it lives in
        // `service-common` rather than here; the CHECK still uses the model's
        // pattern as written, and the code, the envelope, the member paths and
        // every other requirement stay the framework's.
        //
        // X-Ray's own `InvalidRequestException` is unaffected: it is what this
        // service's HANDLERS answer for a request they refuse themselves, and
        // they return it directly.
        //
        // What this service chooses beyond that is the ORDER an enumerated value
        // set is stated in ([`in_stated_value_order`]), which is not the order
        // the model declares it in.
        service_common::aws_validation_refusal(&in_stated_value_order(refusal))
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
