// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-support-awssupport-20130415` service: how it claims requests and how it words the
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
//!   correctly. THIS SERVICE USES IT as the account-level Premium Support
//!   subscription gate: an account whose plan does not qualify is refused there,
//!   ahead of every operation's input validation and state access.
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

use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, OperationContext, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The wire error code every Support API operation answers when the calling account
/// holds no qualifying Premium Support plan.
///
/// The shape is NOT in this service's model (`smithy_query(shape=
/// "SubscriptionRequiredException")` finds nothing, and no operation lists it), so its
/// wire form is recorded behaviour: HTTP 400 with the awsJson1_1 body's `__type`
/// naming it. A client branches on that pair, so they are the requirement here.
const SUBSCRIPTION_REQUIRED_CODE: &str = "SubscriptionRequiredException";

/// The status that refusal carries. 400, not 403: it is the status the service is
/// recorded answering, and it is terminal, so an SDK does not retry a call that
/// cannot succeed until the account's plan changes.
const SUBSCRIPTION_REQUIRED_STATUS: u16 = 400;

/// The sentence that refusal carries. The service documentation states only that "the
/// SubscriptionRequiredException error message appears", so the words are Amazon Web
/// Services' own subscription-required text and are an EXAMPLE of the message rather
/// than part of the contract — the status and the code above are what a client reads.
const SUBSCRIPTION_REQUIRED_MESSAGE: &str =
    "Amazon Web Services Premium Support Subscription is required to use this service.";

/// The `message` member of the synthetic `SubscriptionRequiredException` error shape.
static SUBSCRIPTION_REQUIRED_MESSAGE_MEMBER: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#SubscriptionRequiredException$message",
        "smithy.framework",
        "message",
    ),
    ShapeType::String,
    "message",
    0,
);

/// Schema for the synthetic `SubscriptionRequiredException` error shape.
///
/// Deliberately in the `smithy.framework` namespace, for the same reason
/// `crate::scaffolded`'s synthetic shapes are: the shape is absent from this
/// service's model, so it must not collide with a modeled one. The wire `__type` is
/// [`SUBSCRIPTION_REQUIRED_CODE`], which the protocol writes from the error code it
/// is handed, not from this schema's id.
static SUBSCRIPTION_REQUIRED_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "smithy.framework#SubscriptionRequiredException",
        "smithy.framework",
        "SubscriptionRequiredException",
    ),
    ShapeType::Structure,
    &[&SUBSCRIPTION_REQUIRED_MESSAGE_MEMBER],
);

/// The payload of that refusal: the one `message` member the recorded body carries.
#[derive(Debug, Clone)]
struct SubscriptionRequiredException {
    message: &'static str,
}

impl SerializableStruct for SubscriptionRequiredException {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&SUBSCRIPTION_REQUIRED_MESSAGE_MEMBER, self.message)?;
        Ok(())
    }
}

/// The account-level Premium Support subscription check that fronts EVERY operation
/// of this API.
///
/// It is an operation rather than a line in each handler because the check precedes
/// the request entirely: the service documentation gates the API, not one call, so a
/// request from an account with no qualifying plan is refused before its input is
/// deserialized or validated and before any case is looked up or created. Served
/// through [`Service::override_operation`], the one dispatch-path seam that can answer
/// a request without running the operation the routing key names.
///
/// It declares no auth schemes of its own, so the modeled operation the request names
/// still decides how the request must be signed: an unsigned call is refused as
/// unauthenticated before this ever runs, exactly as the real service orders the two.
///
/// The subscription is a property of ONE account, and this emulator serves exactly one
/// — the canonical emulator account (`engine::emulator_account_id()`, the account every
/// case id and `submittedBy` is built from). So the plan is read from this service's
/// single store and the decision is the same for every caller: there is no second
/// account here to hold a different plan, and nothing in the request selects one.
struct SubscriptionGate;

impl ErasedOperation for SubscriptionGate {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        // Neither the request nor its context can change the answer: the refusal is
        // about the ACCOUNT, so every operation and every input gets the same one. The
        // request is handed to the error renderer below so the refusal closes the
        // envelope of the protocol this request is served in.
        let _ = ctx;
        Box::pin(async move {
            let payload = SubscriptionRequiredException {
                message: SUBSCRIPTION_REQUIRED_MESSAGE,
            };
            // Rendered through this service's own modeled protocol, so the envelope
            // (awsJson1_1 body with `__type`, and that protocol's content type) is
            // the one every other error of this service answers in.
            svc.definition()
                .protocol()
                .serialize_error(
                    &payload,
                    &SUBSCRIPTION_REQUIRED_SCHEMA,
                    SUBSCRIPTION_REQUIRED_CODE,
                    SUBSCRIPTION_REQUIRED_STATUS,
                )
                .unwrap_or_else(|error| svc.definition().framework_error_response(error, req))
        })
    }
}

/// The `com-amazonaws-support-awssupport-20130415` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same shared engine the generated entry handed to every operation, kept
    /// here too because the subscription gate below reads this service's store
    /// before any operation runs.
    engine: Arc<Eng>,
    /// Held in a field, not built per request: [`Service::override_operation`] hands
    /// back a borrow of `self`.
    subscription_gate: SubscriptionGate,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
            subscription_gate: SubscriptionGate,
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
        // THE SUBSCRIPTION GATE. Every operation of the Support API is served only
        // to an account whose Premium Support plan qualifies; the check is on the
        // account, so it belongs here — ahead of input validation and ahead of any
        // handler's resource lookup or state write — rather than in each handler.
        //
        // Only a request that names an operation this service BINDS is gated: one
        // that names none is answered by the routing-error path, as it is for a
        // subscribed account, so a routing mistake stays diagnosable instead of
        // being reported as a subscription problem.
        let bound = self
            .entry
            .protocol()
            .routing_key(req)
            .is_ok_and(|key| self.entry.routing_table().get(key).is_some());
        if !bound {
            return None;
        }
        // No store reachable: let the request through to its operation, which
        // answers that with its own modeled server error rather than reporting a
        // subscription problem the emulator cannot actually have checked.
        let state = crate::state::service_state(&*self.engine)?;
        let (allowed, plan) = {
            let store = state.lock();
            (store.support_api_allowed(), store.support_plan.clone())
        };
        if allowed {
            return None;
        }
        // Record the decision, so a refusal is attributable: an operator seeing every
        // call answered with `SubscriptionRequiredException` can tell this gate from a
        // routing or authentication refusal, and can see which plan was read.
        tracing::debug!(
            operation = self.entry.protocol().routing_key(req).unwrap_or("<unresolved>"),
            support_plan = plan.as_deref().unwrap_or("<none>"),
            "support: refusing the call; the account's plan does not qualify for the Support API"
        );
        Some(&self.subscription_gate as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording. To state a refusal the way the emulated service states it,
        // replace this body with a `match refusal { .. }` that returns
        // `Some(Refused { code, message })` for the cases you reword and `None`
        // for the rest. Replace BOTH the code and the message: a client branches
        // on the code, so a message without its matching code is worse than
        // leaving both alone.
        let _ = refusal;
        None
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
