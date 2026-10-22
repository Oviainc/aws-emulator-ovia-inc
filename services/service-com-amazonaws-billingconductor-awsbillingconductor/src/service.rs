// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-billingconductor-awsbillingconductor` service: how it claims requests and how it words the
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
//!   generated routing table. Return `Some(op)` to route one yourself.
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
//!
//! ## Why this file validates input
//!
//! `com.amazonaws.billingconductor#ValidationException` (`@error("client")`,
//! `@httpError(400)`) is a COMMON error of every operation of this service, and its
//! `Message` member is `@required` — so the 400 a caller gets for a value that breaks a
//! rule the model declares always carries `$.Message`, and an SDK surfaces that text as
//! the exception's message.
//!
//! The framework's pre-dispatch envelope cannot answer with a modelled shape: at that
//! point the refusal is a protocol-level fact rather than one of this service's error
//! SHAPES, and the only seam over it ([`Service::override_refusal`]) chooses an error
//! code and a sentence, not a body — so the sentence goes out under the JSON
//! protocols' generic lowercase `message` key and the modelled, required `Message` is
//! absent from the body entirely. A client that logs `err.message` then gets an empty
//! string and cannot tell which constrained member it got wrong — a bad ARN from a bad
//! account id.
//!
//! [`ModelledValidation`] closes that gap: `override_operation` routes every modelled
//! operation through it, it asks the framework's OWN boundary walk — read the request
//! into the operation's input shape, then run the model's constraint traits over what
//! was read (`aws_smithy_schema_server::constraints::validate`, the same call the
//! centralized sequence makes) — what the request violates, and renders the violations
//! as the modelled `ValidationException`. The rules enforced are therefore exactly the
//! model's; only the WORDS and the envelope are this service's. A request the model
//! accepts is delegated, untouched, to the generated operation in the routing table, so
//! each handler, its state and its 501 are exactly as before.
//!
//! The message itself is built from the request, the way AWS builds it:
//! `<N> validation error(s) detected: ` over one clause per failing member, joined with
//! `; `. The clause rules, the clause ORDER and the envelope live in [`validation`], so
//! this file holds the seam and that module holds the words.

pub mod validation;

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use aws_smithy_schema::serde::SerdeError;
use aws_smithy_schema_server::constraints::{self, Violation, ViolationKind};
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext, Refusal,
    Refused, ServerHttpProtocol, Service,
};

use validation::{list_values, refusal_message};

/// The wire error code (`__type` / `x-amzn-errortype`) of this service's
/// input-validation refusal: the shape the model lists as a COMMON error of every
/// operation for "The input doesn't match with the constraints specified by Amazon Web
/// Services services", and the same name a handler-returned `ValidationException`
/// carries.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The status an input-validation refusal is served on:
/// `ValidationException`'s own `@httpError(400)`.
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// The `com-amazonaws-billingconductor-awsbillingconductor` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// One [`ModelledValidation`] per modelled operation, keyed by the routing key the
    /// model names that operation by ([`Operation::NAME`], the same key the generated
    /// routing table uses). Built once here, never per request; each entry is
    /// zero-sized, so this is a table of names, not a second set of operations.
    refusals: HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            refusals: modelled_validation_table::<Eng>(),
        }
    }
}

/// The modelled-refusal wrapper for EVERY operation this service serves.
///
/// A constraint is a property of the INPUT SHAPE and `ValidationException` is a common
/// error of every operation of this service, so the wrapper goes in front of all of
/// them — implemented or still scaffolded. A stub answers 501 for a request it could
/// serve and the modelled 400 for one no operation could, which is the order the real
/// service evaluates them in.
///
/// An operation left out of this list keeps the framework's generic envelope — the defect
/// this wrapper removes — so the list is not left to review: the
/// `every_modelled_operation_is_validated_in_the_modelled_shape` test in
/// `tests/all/validation_messages.rs` drives every operation the model declares and fails
/// when one of them is not served through the wrapper.
fn modelled_validation_table<Eng: Send + Sync + 'static>(
) -> HashMap<&'static str, Box<dyn ErasedOperation>> {
    let mut refusals: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    macro_rules! register {
        ($($module:ident :: $operation:ident),+ $(,)?) => {
            $(
                refusals.insert(
                    <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                    Box::new(ModelledValidation::<crate::ops::$module::$operation<Eng>>::new())
                        as Box<dyn ErasedOperation>,
                );
            )+
        };
    }
    register!(
        associate_accounts::AssociateAccounts,
        associate_pricing_rules::AssociatePricingRules,
        batch_associate_resources_to_custom_line_item::BatchAssociateResourcesToCustomLineItem,
        batch_disassociate_resources_from_custom_line_item::BatchDisassociateResourcesFromCustomLineItem,
        create_billing_group::CreateBillingGroup,
        create_custom_line_item::CreateCustomLineItem,
        create_pricing_plan::CreatePricingPlan,
        create_pricing_rule::CreatePricingRule,
        delete_billing_group::DeleteBillingGroup,
        delete_custom_line_item::DeleteCustomLineItem,
        delete_pricing_plan::DeletePricingPlan,
        delete_pricing_rule::DeletePricingRule,
        disassociate_accounts::DisassociateAccounts,
        disassociate_pricing_rules::DisassociatePricingRules,
        get_billing_group_cost_report::GetBillingGroupCostReport,
        get_billing_transfer_preference::GetBillingTransferPreference,
        list_account_associations::ListAccountAssociations,
        list_billing_group_cost_reports::ListBillingGroupCostReports,
        list_billing_groups::ListBillingGroups,
        list_custom_line_item_versions::ListCustomLineItemVersions,
        list_custom_line_items::ListCustomLineItems,
        list_pricing_plans::ListPricingPlans,
        list_pricing_plans_associated_with_pricing_rule::ListPricingPlansAssociatedWithPricingRule,
        list_pricing_rules::ListPricingRules,
        list_pricing_rules_associated_to_pricing_plan::ListPricingRulesAssociatedToPricingPlan,
        list_resources_associated_to_custom_line_item::ListResourcesAssociatedToCustomLineItem,
        list_tags_for_resource::ListTagsForResource,
        tag_resource::TagResource,
        untag_resource::UntagResource,
        update_billing_group::UpdateBillingGroup,
        update_billing_transfer_preference::UpdateBillingTransferPreference,
        update_custom_line_item::UpdateCustomLineItem,
        update_pricing_plan::UpdatePricingPlan,
        update_pricing_rule::UpdatePricingRule,
    );
    refusals
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
        // Serve every modelled operation through its [`ModelledValidation`] wrapper, so
        // a request refused before the handler runs is stated in this service's OWN
        // modelled `ValidationException` — whose `@required Message` member is what puts
        // `$.Message` in the body — instead of the framework's generic envelope. The
        // wrapper delegates a request the model accepts to the generated operation in
        // the routing table, which stays the only thing that serves it.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.refusals.get(key).map(|wrapper| &**wrapper)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The FALLBACK path: a refusal of a request that named a modelled operation is
        // answered by [`ModelledValidation`] above, in the modelled shape. What still
        // arrives here is a refusal with no modelled operation to read the request
        // against — a body the codec could not read at all, say. It is still this
        // service's `ValidationException` (HTTP 400) and still carries the clause that
        // names the member and the rule it broke; only the generic envelope's lowercase
        // `message` key is left, which is all this seam can choose.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: refusal_message(violations, &HashMap::new())?,
            }),
            // `Refusal` is `#[non_exhaustive]`: a refusal this service has no name of
            // its own for keeps the framework's wording.
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

/// One modelled operation, wrapped so a request this service refuses BEFORE the handler
/// runs is answered in its own modelled `ValidationException` rather than the
/// framework's generic envelope.
///
/// Zero-sized: the wrapper holds no engine and no operation. It carries the operation
/// TYPE, which is all it needs — the input shape to read the request into and the input
/// `Schema` to validate it against — and delegates a request the model accepts to the
/// one operation instance the generated routing table built from the shared engine.
/// There is therefore no second instance of any operation and no second store.
struct ModelledValidation<Op> {
    operation: PhantomData<fn() -> Op>,
}

impl<Op> ModelledValidation<Op> {
    const fn new() -> Self {
        Self {
            operation: PhantomData,
        }
    }
}

impl<Op: Operation> ErasedOperation for ModelledValidation<Op> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            let started = std::time::Instant::now();
            let protocol = svc.definition().protocol();
            if let Some(exception) = modelled_refusal::<Op>(protocol, req) {
                // Serialized through the service's own protocol, exactly as a
                // handler-returned `ValidationException` is: the same body member name
                // (`Message`), the same `__type`, and the status the model declares for
                // it.
                match protocol.serialize_error(
                    &exception,
                    &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
                    VALIDATION_EXCEPTION,
                    VALIDATION_EXCEPTION_STATUS,
                ) {
                    Ok(response) => {
                        record_refusal::<Op>(ctx, started.elapsed());
                        return response;
                    }
                    // A modelled error that will not serialize leaves the request to
                    // the framework, which refuses it in the generic envelope — the
                    // very divergence this wrapper removes, so it is logged rather
                    // than passed over silently.
                    Err(error) => tracing::warn!(
                        operation = Op::NAME,
                        %error,
                        "the modelled ValidationException would not serialize; this \
                         refusal falls back to the framework's generic envelope"
                    ),
                }
            }
            // Nothing to reword: the request is served by the generated operation,
            // which runs the whole framework sequence (deserialize, validate, handle,
            // serialize) over the same bytes.
            match svc.definition().routing_table().get(Op::NAME) {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable in practice: this wrapper is only reachable under the key
                // the generated table registered. Answer the framework's own
                // no-such-operation response rather than inventing one.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The wrapper is not an operation of its own: the request is signed (or not) as
        // the MODEL says for the operation it names, so authentication is decided by the
        // modelled operation this wrapper stands in front of.
        None
    }
}

/// Record the refusal answered above as the operation row the framework sequence would
/// have recorded, so a refused request is not a hole in the telemetry series: the same
/// classification (from this response's status and error shape), the same folded handle
/// (this service's signing name and the operation's own name), and the same already
/// sanitized request metadata. Everything is gated on a client collecting, exactly as
/// the framework gates it.
fn record_refusal<Op: Operation>(ctx: &OperationContext<'_>, latency: std::time::Duration) {
    if !emulator_telemetry::recording_active() {
        return;
    }
    let handle =
        emulator_telemetry::OperationHandle::folded(ctx.identity().signing_name_static(), Op::NAME);
    emulator_telemetry::record_generated_operation_with_handle(
        &handle,
        latency,
        Some(emulator_telemetry::OperationFailure::classify(
            VALIDATION_EXCEPTION_STATUS,
            VALIDATION_EXCEPTION,
        )),
        smithy_server_runtime::RequestTelemetry::from_headers(&ctx.parts().headers).into_metadata(),
    );
}

/// How this service states the request `Op` cannot be served from, or `None` when it has
/// nothing of its own to say — in which case the request goes on to the generated
/// operation and the framework answers as it always did.
///
/// This is the boundary walk the framework runs, in the framework's order and with the
/// framework's own code: read the request into the operation's input shape, then run
/// constraint-trait validation over what was read. Only the WORDS and the envelope are
/// this service's.
fn modelled_refusal<Op: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &aws_smithy_runtime_api::http::Request,
) -> Option<crate::ops::types::ValidationException> {
    let input = {
        let mut deserializer = protocol.deserialize_input(req, Op::INPUT_SCHEMA).ok()?;
        match <Op::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
            Ok(input) => input,
            // A member the shape requires is absent. It is found while READING the
            // request rather than by walking what was read, but to a caller it is the
            // same refusal — so it is stated the same way, naming the member and the
            // rule it broke.
            Err(SerdeError::MissingMember { member_name }) => {
                let absent = Violation {
                    // The validating walk spells a member's path in lowerCamel; the
                    // codec hands over the Smithy name, so it is cased here to the one
                    // spelling the clause builder uses.
                    path: constraints::path::member(&member_name),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                };
                // Nothing was read, so there is no list value to render.
                return refusal_message(&[absent], &HashMap::new()).map(validation_exception);
            }
            // Any other codec failure is not a refusal this service rewords.
            Err(_) => return None,
        }
    };
    let violations = constraints::validate(&input, Op::INPUT_SCHEMA);
    if violations.is_empty() {
        return None;
    }
    refusal_message(&violations, &list_values(&input, Op::INPUT_SCHEMA)).map(validation_exception)
}

/// This service's modelled refusal carrying `message` — the one place the boundary's
/// `ValidationException` is built from a sentence.
///
/// `Reason` and `Fields` are left absent: the model marks only `Message` required, and a
/// refusal the MODEL's constraints produced has no `ValidationExceptionReason` to name —
/// the enumerated reasons are the service's own semantic refusals, which the handlers
/// state through [`crate::state::validation_exception`].
fn validation_exception(message: String) -> crate::ops::types::ValidationException {
    crate::ops::types::ValidationException {
        message,
        reason: None,
        fields: None,
    }
}
