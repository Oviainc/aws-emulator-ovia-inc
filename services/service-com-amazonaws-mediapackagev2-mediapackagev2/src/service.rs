// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-mediapackagev2-mediapackagev2` service: how it claims requests and how it words the
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
//!   correctly. THIS service uses it: it puts the input-validation gate described
//!   below in front of every modelled operation and lets the generated routing table
//!   serve the request from there.
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
//! MediaPackage's `ValidationException` is modelled with TWO members — `Message` and
//! `ValidationExceptionType` — and AWS sets both on every 400. The framework's
//! pre-dispatch envelope cannot: it renders a refusal as a generic
//! `{"__type":"ValidationException","message":...}` with no modelled member at all,
//! because at that point the refusal is a protocol-level fact rather than one of this
//! service's error SHAPES, and the only seam over it ([`Service::override_refusal`])
//! chooses a code and a sentence, not a body.
//!
//! So this service answers a constraint refusal from its own modelled shape, through
//! [`ModeledInputValidation`]: a gate [`Service::override_operation`] puts in front of
//! every modelled operation, which asks the framework's OWN constraint walk
//! (`aws_smithy_schema_server::constraints::validate`, the same call the centralized
//! sequence makes) what the request violates, renders the first violation through
//! [`crate::state::member_validation_failed`] (the one home of this service's refusal
//! bodies, beside the wording every other refusal uses), and otherwise hands the request
//! straight to the generated operation. The rules enforced are therefore exactly the
//! model's — nothing about a constraint is restated here — and only the BODY of the
//! refusal changes.
//!
//! Two things the gate deliberately does not touch: authentication, which the router
//! settles before any operation runs and reads off the MODELLED operation the routing
//! key names (so a gate cannot weaken an operation's auth schemes), and the
//! per-operation telemetry the centralized sequence records — a request the gate
//! refuses never enters that sequence, so it is counted nowhere, exactly as a request
//! refused before dispatch is.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::constraints::{validate, Violation};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext, Refusal,
    Refused, ServerHttpProtocol, Service,
};

/// `ValidationException`'s modelled status (`httpError` 400 on
/// `com.amazonaws.mediapackagev2#ValidationException`).
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// `ValidationException`'s wire error code — the shape name, which restJson1 puts in
/// `x-amzn-errortype`.
const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// The `com-amazonaws-mediapackagev2-mediapackagev2` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The input-validation gate for each modelled operation, keyed by the routing key
    /// the protocol resolves for it (the operation's own `NAME`). Built once, in `new`.
    input_validation: HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            input_validation: input_validation_gates::<Eng>(),
        }
    }
}

/// One [`ModeledInputValidation`] gate per modelled operation.
///
/// Each gate is zero-sized: it carries only the operation TYPE, which is what names the
/// input shape and its schema. The key is the operation's own `NAME`, so it is the
/// model's routing key rather than a literal written here.
fn input_validation_gates<Eng: Send + Sync + 'static>(
) -> HashMap<&'static str, Box<dyn ErasedOperation>> {
    let mut gates: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    macro_rules! gate {
        ($map:ident; $($module:ident :: $operation:ident),+ $(,)?) => {
            $(
                $map.insert(
                    <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                    Box::new(
                        ModeledInputValidation::<crate::ops::$module::$operation<Eng>>::new(),
                    ) as Box<dyn ErasedOperation>,
                );
            )+
        };
    }
    // Every operation the model binds. A constraint is a property of the INPUT SHAPE,
    // so the gate goes in front of all of them, implemented or still scaffolded: a
    // request that violates the model is refused the same way whoever serves it.
    gate!(
        gates;
        cancel_harvest_job::CancelHarvestJob,
        create_channel::CreateChannel,
        create_channel_group::CreateChannelGroup,
        create_harvest_job::CreateHarvestJob,
        create_origin_endpoint::CreateOriginEndpoint,
        delete_channel::DeleteChannel,
        delete_channel_group::DeleteChannelGroup,
        delete_channel_policy::DeleteChannelPolicy,
        delete_origin_endpoint::DeleteOriginEndpoint,
        delete_origin_endpoint_policy::DeleteOriginEndpointPolicy,
        get_channel::GetChannel,
        get_channel_group::GetChannelGroup,
        get_channel_policy::GetChannelPolicy,
        get_harvest_job::GetHarvestJob,
        get_origin_endpoint::GetOriginEndpoint,
        get_origin_endpoint_policy::GetOriginEndpointPolicy,
        list_channel_groups::ListChannelGroups,
        list_channels::ListChannels,
        list_harvest_jobs::ListHarvestJobs,
        list_origin_endpoints::ListOriginEndpoints,
        list_tags_for_resource::ListTagsForResource,
        put_channel_policy::PutChannelPolicy,
        put_origin_endpoint_policy::PutOriginEndpointPolicy,
        reset_channel_state::ResetChannelState,
        reset_origin_endpoint_state::ResetOriginEndpointState,
        tag_resource::TagResource,
        untag_resource::UntagResource,
        update_channel::UpdateChannel,
        update_channel_group::UpdateChannelGroup,
        update_origin_endpoint::UpdateOriginEndpoint,
    );
    gates
}

/// The modelled-`ValidationException` gate in front of one operation `Op`.
///
/// Zero-sized: `Op` is a type parameter only, naming the input shape and the schema the
/// constraint walk runs against. It holds no engine and no state, so the operation it
/// guards stays the single owner of the request's behaviour.
struct ModeledInputValidation<Op> {
    operation: PhantomData<fn() -> Op>,
}

impl<Op> ModeledInputValidation<Op> {
    const fn new() -> Self {
        Self {
            operation: PhantomData,
        }
    }
}

impl<Op: Operation + 'static> ErasedOperation for ModeledInputValidation<Op> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            if let Some(refused) = refuse_invalid_input::<Op>(svc, req) {
                return refused;
            }
            // Valid: the generated operation serves it, through the framework's own
            // deserialize -> validate -> handle -> serialize sequence (which finds
            // nothing left to refuse).
            match modeled_operation(svc, req) {
                Some(op) => op.run(svc, req, ctx).await,
                // Unreachable: this gate is only reached through `override_operation`,
                // which resolved its key from that same routing table.
                None => svc.definition().routing_error_response(req),
            }
        })
    }
}

/// The operation the model's routing table names for this request.
fn modeled_operation<'s>(svc: &'s dyn Service, req: &RtRequest) -> Option<&'s dyn ErasedOperation> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(key)
}

/// `Some(response)` when `Op`'s input violates a constraint the model declares: this
/// service's modelled `ValidationException`, 400, with both of its members set.
///
/// `None` when the request satisfies the model, or when it failed to READ for a reason
/// that is not a constraint violation — an unparseable body is a protocol failure, and
/// the framework words those per protocol.
fn refuse_invalid_input<Op: Operation>(svc: &dyn Service, req: &RtRequest) -> Option<RtResponse> {
    let protocol = svc.definition().protocol();
    let violation = first_violation::<Op>(protocol, req)?;
    let error = crate::state::member_validation_failed(&violation);
    protocol
        .serialize_error(
            &error,
            &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION_CODE,
            VALIDATION_EXCEPTION_STATUS,
        )
        // A modelled error that will not serialize leaves the request to the framework,
        // which refuses it in the generic envelope — the very divergence this gate
        // removes, so it is logged rather than passed over silently.
        .inspect_err(|error| {
            tracing::warn!(
                operation = Op::NAME,
                %error,
                "the modelled ValidationException would not serialize; this refusal falls \
                 back to the framework's generic envelope"
            );
        })
        .ok()
}

/// The first constraint `Op`'s input violates, as the framework's own walk reports it.
///
/// Both validation classes are classified by the framework's own
/// [`smithy_server_runtime::refusal_of`], so this carries no second copy of "what counts
/// as a constraint refusal": a value outside a declared bound is found by walking what
/// was read, and a required member absent from the request is found while READING it, and
/// `refusal_of` already states that they are one refusal. Anything it does not recognise
/// (an unreadable body, a missing header) is not a constraint refusal and is left to the
/// framework, which words it per protocol.
///
/// AWS names ONE member per `ValidationException`, and the walk visits members in
/// schema order, so "the first" is a property of the input shape rather than of the
/// request.
fn first_violation<Op: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<Violation> {
    let error = match protocol.deserialize_input(req, Op::INPUT_SCHEMA) {
        Err(unreadable) => unreadable,
        Ok(mut deserializer) => {
            match <Op::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
                Err(absent_or_malformed) => ProtocolError::Serde(absent_or_malformed),
                // The request was read: the model's own constraint traits decide. This
                // is the same call the centralized sequence makes, so no rule is
                // restated here.
                Ok(input) => {
                    let violations = validate(&input, Op::INPUT_SCHEMA);
                    if violations.is_empty() {
                        return None;
                    }
                    ProtocolError::Invalid(violations)
                }
            }
        }
    };
    match smithy_server_runtime::refusal_of(&error)? {
        Refusal::Invalid(violations) => violations.into_iter().next(),
        // Authentication is settled by the router before any operation runs, so an
        // `Unauthenticated` refusal cannot arrive here — and it is not a constraint
        // violation in any case.
        _ => None,
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
        // Put this service's input-validation gate in front of the modelled operation
        // the request names, so a value outside a constraint the model declares is
        // refused with the MODELLED `ValidationException` (both members set) instead of
        // the framework's generic 400 envelope. The gate then hands every request the
        // model accepts to the generated operation, which the routing table still owns;
        // a request that names no modelled operation resolves as it always did.
        let key = self.entry.protocol().routing_key(req).ok()?;
        let gate = self.input_validation.get(key)?;
        Some(gate.as_ref())
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // mediapackagev2 does NOT word a refused request the way the framework does.
        // Every input failure this service reports — whether the request boundary found
        // it from the model's own `@range`/`@length`/`@pattern`/enum constraints, or a
        // handler found a cross-member rule the model cannot express — is a 400
        // `ValidationException` whose message reads
        // `Validation error encountered at request.<member>: <the violated rule>`.
        // Leaving the framework's `N validation errors detected: Value '..' at '..'
        // failed to satisfy constraint: ..` sentence here would give one service two
        // unrelated validation-text families, so a client matching this service's
        // documented wording would not recognise half of its rejections.
        //
        // The code stays `ValidationException` (what the framework already chose and
        // what the model declares), so code and message move together.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: "ValidationException".to_string(),
                message: crate::state::refusal_message(violations),
            }),
            // Authentication is the emulator's, not this service's: leave its wording.
            Refusal::Unauthenticated => None,
            // `Refusal` is `#[non_exhaustive]`: a refusal class added later keeps the
            // framework's wording until this service has a form for it.
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
    /// Every operation the model declares has an input-validation gate.
    ///
    /// The gate set is written out by name in `input_validation_gates` — the operation
    /// TYPE is what names the input shape and its schema, and the erased routing table
    /// cannot hand that back — so this is what keeps the two in step: an operation added
    /// to the model (and so to `src/ops/`) without a gate would quietly go back to
    /// answering a constraint refusal in the framework's generic envelope, and this test
    /// fails naming it.
    #[test]
    fn every_modeled_operation_has_an_input_validation_gate() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/smithy-model/com-amazonaws-mediapackagev2-mediapackagev2.json"
        );
        let model = std::fs::read_to_string(path).expect("this crate ships its Smithy model");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the Smithy model is JSON");
        let shapes = model
            .get("shapes")
            .and_then(serde_json::Value::as_object)
            .expect("a Smithy model has shapes");
        let operations: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| shape.get("type").is_some_and(|kind| kind == "operation"))
            .filter_map(|(id, _)| id.rsplit('#').next())
            .collect();
        assert!(
            !operations.is_empty(),
            "the model declares no operations, so this test proves nothing"
        );
        let gates = super::input_validation_gates::<engine::InMemoryEngine>();
        for operation in &operations {
            assert!(
                gates.contains_key(*operation),
                "operation {operation} has no input-validation gate, so a request that \
                 violates its model would be refused without the modelled members"
            );
        }
        assert_eq!(
            gates.len(),
            operations.len(),
            "the gate set and the model's operations must match exactly, got {} gates for \
             {} operations",
            gates.len(),
            operations.len()
        );
    }
}
