// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301` service: how it claims requests and how it words the
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
//! ## How a refused request is stated
//!
//! This service models its own `ValidationException`, and that shape carries
//! `reason` (a `ValidationExceptionReason`) and `fields` (entries of
//! `{name, message}`) beside `message`. The framework validates every modelled
//! constraint — `DescribeAgreementInput$agreementId` has a pattern and a length,
//! `MaxResults` a range, `FilterList` a length, … — BEFORE any handler runs, and
//! renders a violation in the protocol's generic refusal body, which carries
//! `message` alone: `Service::override_refusal` chooses only a code and a
//! message, so the modelled members cannot be stated through it at all.
//!
//! [`ModelledValidation`] is therefore interposed in front of EVERY modelled
//! operation: it reads the violations the boundary walk would find and, when
//! there are any, answers them as this service's own `ValidationException`
//! through [`crate::state::validation_exception_for`] — the same rendering the
//! handlers' own refusals use, so one refused member reads alike wherever it was
//! found. A request with no violation is handed to the generated walk unchanged.

use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, Refusal, Refused, Service,
    Violation,
};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

/// The wire error code (`__type`) and HTTP status this service's modelled
/// `ValidationException` carries (`smithy_query(query="shape",
/// shape="ValidationException")`: `errorKind=client`, `httpError=400`).
const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";
/// See [`VALIDATION_EXCEPTION_CODE`].
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// Serves one modelled operation, stating a refused input in this service's own
/// modelled `ValidationException` instead of the framework's generic refusal
/// body.
///
/// WHY this is a seam and not a line in each handler: the boundary walk refuses a
/// request BEFORE `handle` runs, and the only seam it offers
/// (`Service::override_refusal` -> `Refused`) carries a code and a message —
/// there is nowhere in it to put the modelled `reason` and `fields`. So the
/// violations are read here, where they are still structured, and rendered
/// through [`crate::state::validation_exception_for`].
///
/// Generic over the OPERATION, so the same wrapper serves every one of them and
/// nothing about a particular operation's input is spelled out here: the schema
/// it validates against and the routing key it delegates under are read off the
/// operation's own `impl Operation`.
struct ModelledValidation<Op> {
    /// The operation this wrapper fronts. `fn() -> Op` rather than `Op`, so the
    /// wrapper is `Send + Sync` for the operation's sake alone and owns nothing.
    operation: PhantomData<fn() -> Op>,
}

impl<Op: Operation> ModelledValidation<Op> {
    const fn new() -> Self {
        Self {
            operation: PhantomData,
        }
    }

    /// The violations in `req`, read the way the framework's own walk reads them:
    /// deserialize the input against the operation's schema, then validate the
    /// built value against it. `None` means the request carries no structured
    /// refusal — either it is valid, or it failed in a way that has no
    /// member-level form (a body the codec cannot read at all), which the
    /// generated walk renders itself.
    fn violations(
        protocol: &dyn smithy_server_runtime::ServerHttpProtocol,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<Vec<Violation>> {
        let schema = Op::INPUT_SCHEMA;
        let error = match protocol.deserialize_input(req, schema) {
            Ok(mut deser) => match <Op::Input as DeserializableStruct>::deserialize(&mut *deser) {
                // A built input: every modelled constraint, in one walk.
                Ok(input) => {
                    let violations =
                        aws_smithy_schema_server::constraints::validate(&input, schema);
                    return (!violations.is_empty()).then_some(violations);
                }
                // Absence is found while READING the request, so it arrives as a
                // codec error; `refusal_of` gives it the same structured form a
                // constraint violation has.
                Err(e) => aws_smithy_schema_server::ProtocolError::Serde(e),
            },
            Err(e) => e,
        };
        match smithy_server_runtime::refusal_of(&error) {
            Some(Refusal::Invalid(violations)) if !violations.is_empty() => Some(violations),
            _ => None,
        }
    }
}

impl<Op: Operation> ErasedOperation for ModelledValidation<Op> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            let protocol = svc.definition().protocol();
            if let Some(violations) = Self::violations(protocol, req) {
                let refused = crate::state::validation_exception_for(&violations);
                return protocol
                    .serialize_error(
                        &refused,
                        &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
                        VALIDATION_EXCEPTION_CODE,
                        VALIDATION_EXCEPTION_STATUS,
                    )
                    .unwrap_or_else(|e| svc.definition().framework_error_response(e, req));
            }
            // Nothing to restate: run the generated deserialize -> validate ->
            // handle -> serialize walk. Reached through the routing table rather
            // than through `override_operation`, which would route straight back
            // here. The walk reads and validates the body again — the price of
            // answering a refusal in the modelled shape, paid once per request on
            // a body the transport has already bounded.
            match svc.definition().routing_table().get(Op::NAME) {
                Some(op) => op.run(svc, req, ctx).await,
                None => svc.definition().routing_error_response(req),
            }
        })
    }
}

/// One [`ModelledValidation`] per modelled operation, keyed by the routing key
/// the model names that operation by.
///
/// EVERY operation is wrapped: `ValidationException` is a service-common error
/// (`smithy_query(query="digest")` lists it under `common_errors`), so a refused
/// input is stated in the modelled envelope whichever operation the request
/// named — not only the ones whose refusals happen to have been observed.
macro_rules! modelled_validation_table {
    ($($module:ident :: $operation:ident),* $(,)?) => {
        fn modelled_validation_table<Eng: Send + Sync + 'static>()
            -> HashMap<&'static str, Box<dyn ErasedOperation>>
        {
            let mut table: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
            $(
                table.insert(
                    <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                    Box::new(ModelledValidation::<crate::ops::$module::$operation<Eng>>::new()),
                );
            )*
            table
        }
    };
}

modelled_validation_table!(
    accept_agreement_cancellation_request::AcceptAgreementCancellationRequest,
    accept_agreement_payment_request::AcceptAgreementPaymentRequest,
    accept_agreement_request::AcceptAgreementRequest,
    batch_create_billing_adjustment_request::BatchCreateBillingAdjustmentRequest,
    cancel_agreement::CancelAgreement,
    cancel_agreement_cancellation_request::CancelAgreementCancellationRequest,
    cancel_agreement_payment_request::CancelAgreementPaymentRequest,
    create_agreement_request::CreateAgreementRequest,
    describe_agreement::DescribeAgreement,
    get_agreement_cancellation_request::GetAgreementCancellationRequest,
    get_agreement_entitlements::GetAgreementEntitlements,
    get_agreement_payment_request::GetAgreementPaymentRequest,
    get_agreement_terms::GetAgreementTerms,
    get_billing_adjustment_request::GetBillingAdjustmentRequest,
    list_agreement_cancellation_requests::ListAgreementCancellationRequests,
    list_agreement_charges::ListAgreementCharges,
    list_agreement_invoice_line_items::ListAgreementInvoiceLineItems,
    list_agreement_payment_requests::ListAgreementPaymentRequests,
    list_billing_adjustment_requests::ListBillingAdjustmentRequests,
    reject_agreement_cancellation_request::RejectAgreementCancellationRequest,
    reject_agreement_payment_request::RejectAgreementPaymentRequest,
    search_agreements::SearchAgreements,
    send_agreement_cancellation_request::SendAgreementCancellationRequest,
    send_agreement_payment_request::SendAgreementPaymentRequest,
    update_purchase_orders::UpdatePurchaseOrders,
);

/// The `com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// States a refused input in this service's own modelled
    /// `ValidationException`, one entry per modelled operation; see
    /// [`ModelledValidation`].
    modelled_validation: HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            modelled_validation: modelled_validation_table::<Eng>(),
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
        // This service signs as `aws-marketplace` — a signing name it SHARES with
        // every other AWS Marketplace-family service (Catalog, Reporting, Deployment,
        // Discovery, Entitlement, Metering), each serving its own endpoint
        // (`agreement-marketplace` here). The router's routing key comes from the
        // SigV4 credential scope first, so all of them resolve to `aws-marketplace`
        // and matching on the signing name ALONE makes every one of them claim every
        // request — which the router answers `500 AmbiguousService`.
        //
        // So the claim is narrowed to this service's own modelled wire surface, by the
        // one rule `service-common` states for every service under a shared signing
        // name. This model is `awsJson1_0` + `rpcv2Cbor`, both RPC protocols: they post
        // every operation to one path (`/`, or
        // `/service/<Service>/operation/<Operation>`) and name the target service out
        // of band. Those paths are shared with its RPC peers (the Entitlement Service,
        // Metering) rather than owned, so `claims_request` claims a request only when
        // the service it NAMES is this one — `X-Amz-Target:
        // AWSMPCommerceService_v20200301.<OperationName>` on awsJson1_0
        // (`OPERATIONS.md`), or the `{Service}` label of an rpcv2Cbor path.
        service_common::claims_request(
            parts,
            self.entry.identity().signing_name(),
            crate::ROUTE_PATH_SEGMENTS,
            Some(self.entry.identity().shape_id()),
        )
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Every modelled operation is served through `ModelledValidation`, so a
        // refused input is stated in this service's own modelled
        // `ValidationException` — with the modelled `reason` and `fields` — rather
        // than in the framework's generic refusal body. See that type for why it
        // cannot be done from inside `handle`. The operation names come from the
        // model (each operation's own `Operation::NAME`), and a request naming no
        // modelled operation is left to the generated routing table, which refuses
        // it.
        let routing_key = self.definition().protocol().routing_key(req).ok()?;
        self.modelled_validation
            .get(routing_key)
            .map(|wrapped| wrapped.as_ref())
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // Nothing to reword here: a refusal of a modelled constraint never reaches
        // this seam, because `ModelledValidation` answers it BEFORE the framework
        // renders it. The words are not the problem — the generic refusal body
        // cannot carry the modelled `reason`/`fields` members at all, and
        // `Refused` is only a code and a message.
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
