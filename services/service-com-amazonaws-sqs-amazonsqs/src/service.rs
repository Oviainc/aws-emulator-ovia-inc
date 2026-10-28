// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-sqs-amazonsqs` service: how it claims requests and how it words the
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
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself. Here it carries
//!   the transport's size guard: a request body above [`max_request_body_bytes`] is
//!   answered with a bare HTTP 413 instead of reaching a handler.
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
    http, BoxFuture, ErasedOperation, OperationContext, Refusal, Refused, Service, Violation,
    ViolationKind,
};
use std::sync::Arc;

/// The `com-amazonaws-sqs-amazonsqs` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The transport's answer to a request body above [`max_request_body_bytes`], held
    /// here because [`Service::override_operation`] hands back a reference borrowed from
    /// `&self`.
    oversized: RequestEntityTooLarge,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            oversized: RequestEntityTooLarge,
        }
    }
}

/// How much room beyond the payload the transport ceiling leaves for the request that
/// wraps it: the target and content-type framing, the queue url, the batch entry ids
/// (`DeleteMessageBatchRequestEntry$Id` is at most
/// [`crate::state::MAX_BATCH_ENTRY_ID_LEN`] characters, over at most
/// [`crate::state::MAX_BATCH_ENTRIES`] entries), the attribute names and types that
/// travel beside the bodies, and the protocol's own JSON punctuation and string escaping.
///
/// A fixed allowance rather than the sum of those modelled lengths: it only has to
/// comfortably exceed every member that can travel beside the payload, and a sum would
/// have to be re-derived whenever one more member is modelled. 64 KiB is the same figure
/// the only other per-service transport ceiling in this tree leaves for its own framing
/// (`service-com-amazonaws-codecommit-codecommit-20150413`'s `MAX_REQUEST_BODY_BYTES`).
const REQUEST_ENVELOPE_ALLOWANCE_BYTES: usize = 64 * 1024;

/// The largest HTTP request body this service reads at all.
///
/// The biggest payload any Amazon SQS request can carry is ONE maximum total payload
/// ([`crate::state::maximum_batch_payload_bytes`]) — "the maximum allowed individual
/// message size and the maximum total payload size (the sum of the individual lengths of
/// all of the batched messages)" are the same figure, so a single message and a whole
/// batch are bounded alike and a request carrying more than that figure of message bytes
/// is past what the service accepts however it is spelled. The ceiling is read from there
/// rather than written out a second time. Beyond it sits
/// [`REQUEST_ENVELOPE_ALLOWANCE_BYTES`] for the request wrapped around that payload, which
/// is what keeps the modelled `BatchRequestTooLong` reachable: a batch whose entries add
/// up to more than the payload limit is still read, and refused by the OPERATION with the
/// limit and the bytes it was sent.
///
/// A body larger than the sum cannot carry a payload this service would accept however it
/// is spelled, so it is refused by the TRANSPORT — a bare HTTP 413 with no error document
/// — before the body is deserialized and before any handler looks at a field. That is a
/// different refusal from a field-level one: a parameter refusal says the service read the
/// request and disliked a value, while this one says the request never fit on the wire.
///
/// This is not a second copy of the framework's own 413. The core's two body caps
/// (`vendor/server/src/lib.rs`'s `MAX_PRODUCTION_BODY_BYTES`, 16 MiB on the serve path,
/// and `vendor/server/src/listener_guard.rs`'s `caps::MAX_BYTES_PER_CONN`, 64 MiB per
/// resource listener) bound how much the PROCESS will buffer for any service at all: both
/// are `const` with no per-service setting, and both sit orders of magnitude above any
/// Amazon SQS payload, so neither can state what this service accepts. The ceiling an
/// individual service reads at all is the service's own, which is why it lives here —
/// the same place `service-com-amazonaws-codecommit-codecommit-20150413` keeps the one
/// for its git transport.
pub fn max_request_body_bytes() -> usize {
    usize::try_from(crate::state::maximum_batch_payload_bytes())
        .unwrap_or(usize::MAX)
        .saturating_add(REQUEST_ENVELOPE_ALLOWANCE_BYTES)
}

/// The transport's refusal of a request whose body is above [`max_request_body_bytes`]:
/// HTTP 413 (Request Entity Too Large) and nothing else.
///
/// It is shaped as an operation only because the router names one before anything can be
/// answered; nothing of this service's model reaches it. It deserializes no input, takes
/// no store and writes nothing, so the oversized body is never read and no message it
/// carried is enqueued. The response carries NO body, so a client sees the bare transport
/// status rather than a service error code — which is the whole difference this refusal
/// states.
///
/// [`ErasedOperation::auth_schemes`] is deliberately left at its default `None`: an
/// author-written operation that answers `None` defers to the schemes the model names for
/// the operation the request's routing key resolves, so routing a request here never
/// weakens what must sign it.
#[derive(Debug)]
struct RequestEntityTooLarge;

/// The status the transport refuses an over-sized body with.
const REQUEST_ENTITY_TOO_LARGE: u16 = 413;

impl ErasedOperation for RequestEntityTooLarge {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        _req: &'r aws_smithy_runtime_api::http::Request,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            aws_smithy_runtime_api::http::Response::new(
                aws_smithy_runtime_api::http::StatusCode::try_from(REQUEST_ENTITY_TOO_LARGE)
                    .expect("413 is a status code"),
                aws_smithy_types::body::SdkBody::empty(),
            )
        })
    }
}

/// The error Amazon SQS names for a value the boundary validator refused, or `None` to
/// leave the framework's wording in place.
///
/// One case, and it is model-driven rather than keyed to a member name: a value
/// outside `QueueAttributeName`'s value set is an attribute name that does not exist,
/// which this service's model has an error shape for — `InvalidAttributeName`, "The
/// specified attribute doesn't exist." (HTTP 400, the same status the refusal already
/// carries). Every operation that takes attribute names reaches it through the same
/// enum, so matching on the refused value SET covers all of them without naming one.
///
/// An attribute name supplied as a MAP KEY (`CreateQueue`/`SetQueueAttributes`
/// `Attributes`) never arrives here: the constraint walk captures a map key rather than
/// validating it, so those handlers refuse an unknown name themselves
/// (`crate::state::validate_attributes`). This covers the LIST form
/// (`GetQueueAttributes` `AttributeNames`), which the walk does validate.
fn refusal_for(violation: &Violation) -> Option<Refused> {
    let ViolationKind::EnumValue(allowed) = &violation.kind else {
        return None;
    };
    if allowed.len() != crate::ops::types::QUEUE_ATTRIBUTE_NAME_VALUES.len()
        || !allowed
            .iter()
            .all(|value| crate::ops::types::QUEUE_ATTRIBUTE_NAME_VALUES.contains(&value.as_str()))
    {
        return None;
    }
    let name = violation.value.as_deref().unwrap_or(&violation.path);
    Some(Refused {
        code: "InvalidAttributeName".to_string(),
        message: format!("Unknown Attribute {name}."),
    })
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
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        //
        // BOUNDED BEFORE ANYTHING IS READ. The body reaching here is the one dispatch
        // will deserialize (the router already undid any `aws-chunked` framing), so this
        // is the first point at which its size is known and the last before a handler
        // reads a field off it. A body above the transport ceiling is answered by
        // [`RequestEntityTooLarge`] — the bare 413 — whatever operation it names, so no
        // modeled operation is handed a request the transport could not accept and
        // nothing it carried is stored. A body the ceiling admits routes exactly as it
        // did before.
        match req.body().bytes() {
            Some(body) if body.len() > max_request_body_bytes() => Some(&self.oversized),
            Some(_) => None,
            // A body that is not in memory has no length to measure, so the ceiling
            // cannot be applied to it and this arm must not guess: it routes on. The
            // router buffers every request body before it looks an operation up, so the
            // arm is unreachable through dispatch, and that is what the suite in
            // `tests/all/request_size_transport_bound.rs` pins — a runtime that stopped
            // buffering would silently disable the bound, and those rows would fail.
            None => None,
        }
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Both the code and the message are replaced, because a
        // client branches on the code. Everything [`refusal_for`] does not recognise
        // keeps the framework's own wording.
        match refusal {
            Refusal::Invalid(violations) => violations.iter().find_map(refusal_for),
            // Authentication and every refusal added later keep the framework's
            // wording.
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
