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
//! - `override_operation` — EDITED HERE. The key and the handler stay the generated
//!   table's; the operation is served through [`PreHandlerChecks`], which states a
//!   request refused for violating a modelled constraint in THIS service's own
//!   `ValidationException`. See "Why this file validates input" below.
//! - `override_refusal` — EDITED HERE. A refusal the framework still renders (one with
//!   no modelled operation to read the request against) is worded from the same
//!   violations through the same [`refusal_message`], so the two refusal paths cannot
//!   disagree.
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
//! [`PreHandlerChecks`] closes that gap: `override_operation` routes every modelled
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
//!
//! ## Why this file authorizes the caller
//!
//! Billing Conductor is a PAYER-ACCOUNT-ONLY API: its control plane is authorized for
//! the payer (management) account of the consolidated billing family alone, and a call
//! from any other account is refused HTTP 403 `AccessDeniedException` — the shape the
//! model lists as a COMMON error of every operation of this service, with a `@required
//! Message` built from the request (the calling account and the operation name, see
//! [`crate::state::not_payer_account`]).
//!
//! The decision is reached from the CALLER alone, so it precedes everything the request
//! SAYS: the operation's own semantic validation (a grouping it will not accept), its
//! resource lookups (a plan or a group that exists nowhere) and its dispatch — an
//! operation nobody has implemented yet is denied rather than answered with the
//! scaffold's 501. Because the gate sits in front of the handler, it also precedes the
//! two stages a handler reaches before it answers: an `@idempotencyToken` lookup (a
//! `ClientToken` a denied caller reuses cannot launder a cached success out of
//! `CreatePricingRule`) and any per-resource result assembly (the batch association
//! operations deny the whole call rather than report it in `FailedAssociatedResources` /
//! `FailedDisassociatedResources`). Reading the caller and the store is all it does, so
//! it is the same answer for every racer of a concurrent set — no racer wins and no
//! state is mutated.
//!
//! One refusal still comes BEFORE it: the boundary's modelled-constraint refusal above.
//! That is the recorded order, not a convenience — the same recorded callers that are
//! denied 403 on a conforming request are answered 400 `ValidationException` when a
//! member breaks its modelled `@pattern` (`UpdateBillingGroup`, `DeleteBillingGroup`,
//! `AssociateAccounts`, `DisassociateAccounts`, `GetBillingGroupCostReport` and the
//! batch association operations are all recorded both ways), because a constraint the
//! model declares is refused by the front door, ahead of the service code that reads who
//! is calling. So this file runs the two checks in that order, in ONE place every
//! operation shares rather than in 34 handlers: constraint refusal (400), then caller
//! authorization (403), then the operation.
//!
//! The decision itself lives in state beside the resources, as
//! [`crate::state::ComAmazonawsBillingconductorAwsbillingconductorState::payer_account_id`]:
//! this emulator records no organization, so no account is the payer and the control
//! plane is reachable by nobody — which is exactly what AWS answers the non-payer
//! callers every recorded response was taken from.
//!
//! The TAGGING operations are not on that gate (`PAYER_UNGATED_OPERATIONS`): a
//! recorded `TagResource` call from a non-payer account against a billing-group ARN
//! that names no resource is answered by resource resolution (HTTP 404), not by the
//! payer gate, so this service's tag operations resolve their `ResourceArn` and answer
//! from the store like any other read or write.

pub mod validation;

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Response as RtResponse, StatusCode};
use aws_smithy_schema::serde::SerdeError;
use aws_smithy_schema_server::constraints::{self, Violation, ViolationKind};
use aws_smithy_types::body::SdkBody;
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

/// The wire error code (`__type` / `x-amzn-errortype`) of this service's
/// caller-authorization refusal: the shape the model lists as a COMMON error of every
/// operation for "You do not have sufficient access to perform this action", and the
/// same name a handler-returned `AccessDeniedException` carries.
const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";

/// The status an authorization refusal is served on: `AccessDeniedException`'s own
/// `@httpError(403)`.
const ACCESS_DENIED_EXCEPTION_STATUS: u16 = 403;

/// The operations of this service that are NOT gated on the payer account.
///
/// The tag operations address their target by ARN and are answered by resolving it:
/// a recorded `TagResource` from a non-payer account naming a billing-group ARN that
/// exists nowhere is answered 404 "Resource was not found", which a payer gate ahead of
/// resolution could never produce. Everything else — every create, update, delete,
/// association and list of the control plane — is gated.
pub const PAYER_UNGATED_OPERATIONS: &[&str] =
    &["ListTagsForResource", "TagResource", "UntagResource"];

/// Whether AWS gates `operation` on the caller being the payer account. See
/// [`PAYER_UNGATED_OPERATIONS`].
fn is_payer_gated(operation: &str) -> bool {
    !PAYER_UNGATED_OPERATIONS.contains(&operation)
}

/// The `com-amazonaws-billingconductor-awsbillingconductor` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// One [`PreHandlerChecks`] per modelled operation, keyed by the routing key the
    /// model names that operation by ([`Operation::NAME`], the same key the generated
    /// routing table uses). Built once here, never per request; each entry holds the
    /// shared engine and the operation's TYPE, so this is a table of names, not a second
    /// set of operations and not a second store.
    refusals: HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            refusals: pre_handler_check_table::<Eng>(&engine),
            entry: crate::ServiceDefinition::new(engine),
        }
    }
}

/// The PRE-HANDLER checks of this service, keyed by the routing key the model names each
/// operation by: one wrapper per operation, each running the modelled-constraint refusal
/// and then the payer-account authorization gate ahead of the operation itself.
///
/// A constraint is a property of the INPUT SHAPE and `ValidationException` is a common
/// error of every operation of this service, so the wrapper goes in front of all of
/// them — implemented or still scaffolded. A stub answers 501 for a request it could
/// serve and the modelled 400 for one no operation could, which is the order the real
/// service evaluates them in.
///
/// `AccessDeniedException` is a common error of every operation too, and the gate that
/// raises it is decided from the CALLER, so it is wired from the same one list of
/// operations ([`is_payer_gated`] names the exceptions) rather than from a second list
/// that could drift from it.
///
/// An operation left out of this list keeps the framework's generic envelope and no gate
/// at all — the defects these checks remove — so the list is not left to review:
/// `every_modelled_operation_is_validated_in_the_modelled_shape` in
/// `tests/all/validation_messages.rs` drives every operation the model declares and fails
/// when one of them is not served through the wrapper, and
/// `every_payer_gated_operation_denies_a_caller_that_is_not_the_payer_account` in
/// `tests/all/payer_account_gate.rs` fails when one of them is not gated, while
/// `tests/all/payer_gate_precedes_idempotency_existence_and_assembly.rs` pins the STAGES
/// the gate comes before (the idempotency-token lookup, resource existence resolution,
/// per-resource result assembly and state mutation, under concurrency).
fn pre_handler_check_table<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
) -> HashMap<&'static str, Box<dyn ErasedOperation>> {
    let mut refusals: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    macro_rules! register {
        ($($module:ident :: $operation:ident),+ $(,)?) => {
            $({
                let name = <crate::ops::$module::$operation<Eng> as Operation>::NAME;
                refusals.insert(
                    name,
                    Box::new(PreHandlerChecks::<crate::ops::$module::$operation<Eng>, Eng>::new(
                        Arc::clone(engine),
                        is_payer_gated(name),
                    )) as Box<dyn ErasedOperation>,
                );
            })+
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
        // Serve every modelled operation through its [`PreHandlerChecks`] wrapper, so a
        // request refused before the handler runs is stated in this service's OWN
        // modelled shapes — the `ValidationException` whose `@required Message` member
        // puts `$.Message` in the body, and the `AccessDeniedException` the
        // payer-account gate raises — instead of the framework's generic envelope or no
        // refusal at all. The wrapper delegates a request that passes both checks to the
        // generated operation in the routing table, which stays the only thing that
        // serves it.
        //
        // A request that names no modelled operation has nothing to validate and nothing
        // to authorize: the routing failure is the answer it gets.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.refusals.get(key).map(|wrapper| &**wrapper)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The FALLBACK path: a refusal of a request that named a modelled operation is
        // answered by [`PreHandlerChecks`] above, in the modelled shape. What still
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
                // No modelled operation means no input shape to read a violation's member
                // path against, so the clause states its constraint without a value (see
                // `validation::is_sensitive`).
                message: refusal_message(violations, &HashMap::new(), None)?,
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

/// One modelled operation, wrapped so the two checks that precede its handler are run
/// where every operation shares them: the modelled-constraint refusal, answered in this
/// service's own `ValidationException` rather than the framework's generic envelope, and
/// then the payer-account authorization gate, answered in its modelled
/// `AccessDeniedException`.
///
/// Holds no operation: it carries the operation TYPE, which is all the first check needs
/// (the input shape to read the request into and the input `Schema` to validate it
/// against), and delegates a request that passes both to the one operation instance the
/// generated routing table built from the shared engine. There is therefore no second
/// instance of any operation. The `engine` field is the SAME `Arc` the generated entry
/// holds — never a second store — and is what lets the gate read the payer account out
/// of this service's state before the handler runs.
struct PreHandlerChecks<Op, Eng> {
    engine: Arc<Eng>,
    /// Whether AWS gates this operation on the caller being the payer account
    /// ([`is_payer_gated`]), decided once at construction from the operation's name.
    payer_gated: bool,
    operation: PhantomData<fn() -> Op>,
}

impl<Op, Eng> PreHandlerChecks<Op, Eng> {
    const fn new(engine: Arc<Eng>, payer_gated: bool) -> Self {
        Self {
            engine,
            payer_gated,
            operation: PhantomData,
        }
    }
}

impl<Op: Operation, Eng: Send + Sync + 'static> ErasedOperation for PreHandlerChecks<Op, Eng> {
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
                        record_refusal(
                            ctx,
                            Op::NAME,
                            VALIDATION_EXCEPTION,
                            VALIDATION_EXCEPTION_STATUS,
                            started.elapsed(),
                        );
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
            // THE AUTHORIZATION GATE, after the boundary's constraint refusal and ahead
            // of everything the operation does (see the module docs): Billing
            // Conductor's control plane is the payer account's, so a conforming request
            // from any other caller is denied 403 `AccessDeniedException` before the
            // operation's own semantic checks, its resource lookups, and its dispatch —
            // including the scaffold's 501.
            //
            // The request's auth schemes are unaffected: the router reads them off the
            // MODELLED operation the routing key names, so a denial here is still only
            // reached by a request whose signature verified.
            if self.payer_gated && !crate::state::caller_is_payer_account(&*self.engine) {
                return payer_account_denial(protocol, ctx, Op::NAME, started);
            }
            // Nothing to reword and nobody to deny: the request is served by the
            // generated operation, which runs the whole framework sequence (deserialize,
            // validate, handle, serialize) over the same bytes.
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

/// The payer-account denial of one operation: HTTP 403 `AccessDeniedException` whose
/// `Message` is built from the calling account and `operation`.
///
/// Reached only for a request already judged unauthorized by
/// [`crate::state::caller_is_payer_account`].
fn payer_account_denial(
    protocol: &dyn ServerHttpProtocol,
    ctx: &OperationContext<'_>,
    operation: &'static str,
    started: std::time::Instant,
) -> aws_smithy_runtime_api::http::Response {
    let exception = crate::state::not_payer_account(operation);
    // Serialized through the service's own protocol, exactly as a handler-returned
    // `AccessDeniedException` is: the same body member name (`Message`), the same
    // `__type`, the same `x-amzn-errortype` header restJson1 resolves an error shape
    // from, and the status the model declares for it.
    match protocol.serialize_error(
        &exception,
        &crate::ops::types::SCHEMA_ACCESS_DENIED_EXCEPTION,
        ACCESS_DENIED_EXCEPTION,
        ACCESS_DENIED_EXCEPTION_STATUS,
    ) {
        Ok(response) => {
            record_refusal(
                ctx,
                operation,
                ACCESS_DENIED_EXCEPTION,
                ACCESS_DENIED_EXCEPTION_STATUS,
                started.elapsed(),
            );
            response
        }
        // An authorization decision never fails OPEN: a modelled error that will not
        // serialize is still a denial, answered in this protocol's own envelope
        // (`restJson1`: the 403, the error-type header and a JSON body) rather than by
        // letting the request through to the operation.
        Err(error) => {
            tracing::warn!(
                operation,
                %error,
                "the modelled AccessDeniedException would not serialize; this denial \
                 falls back to a hand-built 403"
            );
            unserialized_denial(&exception.message)
        }
    }
}

/// The 403 a denial falls back to when the modelled shape will not serialize: the same
/// status, the same wire error code and the same `Message`, in `restJson1`'s own
/// envelope.
fn unserialized_denial(message: &str) -> aws_smithy_runtime_api::http::Response {
    let body = ::serde_json::json!({
        "__type": ACCESS_DENIED_EXCEPTION,
        "Message": message,
    })
    .to_string();
    let status = StatusCode::try_from(ACCESS_DENIED_EXCEPTION_STATUS)
        .expect("the modelled 403 is a valid HTTP status");
    let mut response = RtResponse::new(status, SdkBody::from(body));
    let headers = response.headers_mut();
    headers.insert("content-type", "application/json");
    headers.insert("x-amzn-errortype", ACCESS_DENIED_EXCEPTION);
    response
}

/// Record a refusal answered above as the operation row the framework sequence would
/// have recorded, so a refused request is not a hole in the telemetry series: the same
/// classification (from this response's status and error shape), the same folded handle
/// (this service's signing name and the operation's own name), and the same already
/// sanitized request metadata. Everything is gated on a client collecting, exactly as
/// the framework gates it.
fn record_refusal(
    ctx: &OperationContext<'_>,
    operation: &'static str,
    error_code: &'static str,
    status: u16,
    latency: std::time::Duration,
) {
    if !emulator_telemetry::recording_active() {
        return;
    }
    let handle = emulator_telemetry::OperationHandle::folded(
        ctx.identity().signing_name_static(),
        operation,
    );
    emulator_telemetry::record_generated_operation_with_handle(
        &handle,
        latency,
        Some(emulator_telemetry::OperationFailure::classify(
            status, error_code,
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
    // The shape the request is read into, named as the model names it: it is what a
    // violation's member path is read as a position in the model, which is how a clause
    // knows whether the value it is about is one the model marks `@sensitive`.
    let input_shape = Op::INPUT_SCHEMA.shape_id().to_string();
    let input = Some(input_shape.as_str());
    let input_value = {
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
                    sensitive: false,
                };
                // Nothing was read, so there is no list value to render.
                return refusal_message(&[absent], &HashMap::new(), input)
                    .map(validation_exception);
            }
            // Any other codec failure is not a refusal this service rewords.
            Err(_) => return None,
        }
    };
    let violations = constraints::validate(&input_value, Op::INPUT_SCHEMA);
    if violations.is_empty() {
        return None;
    }
    refusal_message(
        &violations,
        &list_values(&input_value, Op::INPUT_SCHEMA),
        input,
    )
    .map(validation_exception)
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

