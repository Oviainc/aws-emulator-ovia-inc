// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-s3vectors-s3vectors` service: how it claims requests and how it words the
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
//!   correctly, or serve a modelled operation WRAPPED. This service wraps every one of
//!   them in [`ModelledValidation`], so a request refused before its handler runs is
//!   refused in this service's own modelled `ValidationException` rather than in the
//!   framework's pre-dispatch envelope (see the section below).
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
//! # How this service refuses a request it never ran
//!
//! `ValidationException` is one of the errors this model binds to EVERY operation, and
//! it carries `fieldList` beside the `@required` `message` — the machine-readable half
//! of the body, naming each input that failed. A request refused for a modelled
//! constraint is therefore answered with that whole body, exactly as this service's
//! handler-side refusals are. The framework's pre-dispatch renderer cannot: its only
//! seam, `override_refusal`, carries a code and a sentence, so the body it writes is
//! `message` alone and `fieldList` is missing from it.
//!
//! Every operation is therefore served through [`ModelledValidation`], which states
//! that refusal itself — from the framework's own deserializer and constraint walk —
//! before the handler runs. [`validation`] owns the wording and the modelled body, and
//! `override_refusal` words any refusal the framework still renders from the same
//! helpers so the two paths cannot disagree.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, Refusal, Refused,
    ServerHttpProtocol, Service,
};

/// The refusal layer: how this service words a request it never ran, and the modelled
/// `ValidationException` body it answers with. Internal to this crate — what a client
/// sees of it is the error on the wire, which the crate's tests assert through the
/// router.
mod validation;

/// One modelled operation, served so that a request refused BEFORE its handler runs is
/// answered in the modelled `ValidationException` — with the `fieldList` naming each
/// input that failed — rather than in the framework's generic refusal body, which
/// carries `message` alone.
///
/// Zero-sized: the wrapper holds no engine and no operation instance. It carries the
/// operation TYPE, which is all it needs — the input shape to read the request into and
/// the input `Schema` to validate it against — and hands a request the model accepts to
/// the one operation instance the generated routing table built from the shared engine.
/// There is therefore no second instance of any operation and no second store.
///
/// Nothing else is re-implemented: a request this wrapper does not refuse is served by
/// the generated operation, which owns deserialize -> validate -> handle -> serialize,
/// its telemetry and every modelled error. The cost is that such a request is read
/// twice, once here and once in the operation's own sequence, because the framework
/// hands no already-built input to an operation.
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
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let started = std::time::Instant::now();
            // Scoped so the borrowed `&dyn ErasedServiceDefinition` — which is not
            // `Sync`, while this future is — is dropped before the await below. The
            // protocol itself is.
            let refused = {
                let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
                validation::modelled_refusal::<Op>(protocol, req)
            };
            if let Some(response) = refused {
                // The row the framework sequence would have recorded for this request,
                // so a refused one is not a hole in the telemetry series.
                record_refusal::<Op>(ctx, started.elapsed());
                return response;
            }
            // Served by the generated operation, reached through the routing table
            // rather than through `override_operation`, which would route straight back
            // here.
            match svc.definition().routing_table().get(Op::NAME) {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable in practice: this wrapper is only reachable under the key
                // the generated table registered. Answer the framework's own
                // no-such-operation response rather than inventing one.
                None => svc.definition().routing_error_response(req),
            }
        })
    }
}

/// Record the refusal answered above as the operation row the framework sequence would
/// have recorded, so a request refused by the wrapper is not a hole in the telemetry
/// series: the same classification (from the refusal's status and error shape), the same
/// folded handle (this service's signing name and the operation's own name), and the
/// same already-sanitized request metadata. Everything is gated on a client collecting,
/// exactly as the framework gates it.
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
            400,
            validation::VALIDATION_EXCEPTION_CODE,
        )),
        smithy_server_runtime::RequestTelemetry::from_headers(&ctx.parts().headers).into_metadata(),
    );
}

/// The modelled-refusal wrapper for EVERY operation this service serves.
///
/// A constraint is a property of the INPUT SHAPE and this service's model declares
/// `ValidationException` as a common error on every operation
/// (`smithy_query(query="digest")`), so the wrapper goes in front of all of them: the
/// constraint refusal each one owes is the same refusal, so none of them may answer it
/// in a different body. The list is hand-maintained — `OperationRoutingTable` exposes
/// only `add_operation` / `get`, so this crate cannot walk the generated table to wrap
/// whatever it holds — and an operation added to the model and not added here would
/// fall back to the framework's `message`-only envelope.
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
        create_index::CreateIndex,
        create_vector_bucket::CreateVectorBucket,
        delete_index::DeleteIndex,
        delete_vector_bucket::DeleteVectorBucket,
        delete_vector_bucket_policy::DeleteVectorBucketPolicy,
        delete_vectors::DeleteVectors,
        get_index::GetIndex,
        get_vector_bucket::GetVectorBucket,
        get_vector_bucket_policy::GetVectorBucketPolicy,
        get_vectors::GetVectors,
        list_indexes::ListIndexes,
        list_tags_for_resource::ListTagsForResource,
        list_vector_buckets::ListVectorBuckets,
        list_vectors::ListVectors,
        put_vector_bucket_default_index_mode::PutVectorBucketDefaultIndexMode,
        put_vector_bucket_policy::PutVectorBucketPolicy,
        put_vectors::PutVectors,
        query_vectors::QueryVectors,
        tag_resource::TagResource,
        untag_resource::UntagResource,
        update_index_mode::UpdateIndexMode,
    );
    refusals
}

/// The `com-amazonaws-s3vectors-s3vectors` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The modelled-refusal wrapper every operation is served behind, keyed by the
    /// routing key the model names that operation by; see [`ModelledValidation`]. Built
    /// once here because [`Service::override_operation`] hands back a borrow of it.
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
        // NOT a re-route: serve every modelled operation through its
        // [`ModelledValidation`] wrapper, so a request refused before the handler runs
        // is stated in the MODELLED `ValidationException` — with the `fieldList` a
        // client reads the failing input from — instead of the framework's generic
        // refusal body, which can carry only a code and a message. See
        // [`ModelledValidation`] for why that cannot be done from `override_refusal` or
        // from inside `handle`.
        //
        // The key comes from the protocol's own routing resolution and the wrappers are
        // registered under the model's own operation names, so a request naming no
        // modelled operation resolves to nothing here and is left to the generated
        // routing table (and then to the framework's routing error).
        let routing_key = self.entry.protocol().routing_key(req).ok()?;
        self.refusals
            .get(routing_key)
            .map(|wrapper| wrapper.as_ref())
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // state it the way S3 Vectors states it. [`ModelledValidation`] answers a
        // refusal it can state in the modelled `ValidationException` BODY, so what
        // reaches here is what the framework still renders: a constraint class the
        // service has no wording for, and the fallback when the modelled error cannot
        // be serialized. Worded from the same helpers, so the two paths cannot
        // disagree, and both halves are replaced together because a client branches on
        // the code.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) => validation::refused_for(violations),
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
