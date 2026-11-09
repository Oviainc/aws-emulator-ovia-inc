// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-rbin-amazonrecyclebin` service: how it claims requests and how it words the
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
//!   correctly. THIS service uses it to put the input-validation gate described below
//!   in front of every modelled operation; the generated routing table still serves
//!   every request the model accepts.
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
//! Recycle Bin's `ValidationException` is modelled with TWO members — `Message` and
//! `Reason` (`ValidationExceptionReason`) — and AWS sets both on every 400 it refuses a
//! parameter value with. The framework's pre-dispatch envelope cannot: it renders a
//! refusal that happens before any handler runs as a generic
//! `{"__type":"ValidationException","message":…}`, with neither modelled member in the
//! body, because at that point the refusal is a protocol-level fact rather than one of
//! this service's error SHAPES — and the only seam over it
//! ([`Service::override_refusal`]) chooses a code and a sentence, not a body. A client
//! then sees a 400 whose `Message` is absent and whose `Reason` enum it cannot branch
//! on at all.
//!
//! So this service answers a constraint refusal from its own modelled shape, through
//! [`ModelledInputValidation`]: a gate [`Service::override_operation`] puts in front of
//! every modelled operation, which asks the framework's OWN constraint walk
//! (`aws_smithy_schema_server::constraints::validate`, the same call the centralized
//! sequence makes) what the request violates and renders the answer through
//! [`crate::state::constraint_violations`] — the one home of this service's refusal
//! bodies, beside the wording every handler-raised refusal uses. Every request the
//! model accepts is handed straight to the generated operation. The rules enforced are
//! therefore exactly the model's: nothing about a constraint is restated here, and only
//! the BODY of the refusal changes.
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
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{path, validate, Violation};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext, Refusal,
    Refused, ServerHttpProtocol, Service,
};

/// `ValidationException`'s modelled status.
/// Source: smithy_query(query="shape", shape="ValidationException") — `httpError` 400.
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// `ValidationException`'s wire error code — the shape name, which restJson1 puts in
/// `x-amzn-errortype` and in the body's `__type`.
const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// The `com-amazonaws-rbin-amazonrecyclebin` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The input-validation gate for each modelled operation, keyed by the routing key
    /// the protocol resolves for it (the operation's own `NAME`). Built once, in `new`.
    input_validation: HashMap<&'static str, Box<dyn ErasedOperation>>,
    /// Every member path this service binds into a request URI PATH, over all of its
    /// operations — what [`Service::override_refusal`] renders a clause's `@pattern`
    /// requirement by, since that seam is handed a refusal without the operation it
    /// belongs to. Built once, in `new`, from the same operation list and the same
    /// model-reading as the per-operation gates ([`uri_path_label_paths`]).
    uri_path_labels: Vec<String>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let (input_validation, uri_path_labels) = modelled_operation_gates::<Eng>();
        Self {
            entry: crate::ServiceDefinition::new(engine),
            input_validation,
            uri_path_labels,
        }
    }
}

/// The lower-camel member paths `input_schema` binds into the request URI PATH — its
/// `@httpLabel` members, as a validation clause spells a member path.
///
/// Read off the member's own binding trait, so the MODEL decides: a member with no
/// `@httpLabel` travels in the body or the query string. The casing is the framework's
/// own path spelling ([`path::member`]), which is how the constraint walk spells the path
/// a violation carries, so the two cannot drift.
fn uri_path_label_paths(input_schema: &Schema) -> Vec<String> {
    input_schema
        .members()
        .iter()
        .filter(|member| member.http_label().is_some())
        .filter_map(|member| member.member_name())
        .map(path::member)
        .collect()
}

/// One [`ModelledInputValidation`] gate per modelled operation, and the URI-path member
/// paths of every one of them.
///
/// Each gate is zero-sized: it carries only the operation TYPE, which is what names the
/// input shape and its schema. The key is the operation's own `NAME`, so it is the
/// model's routing key rather than a literal written here. The second half is the union
/// of each input shape's `@httpLabel` member paths, for the one refusal path that is
/// reached without an operation.
fn modelled_operation_gates<Eng: Send + Sync + 'static>(
) -> (HashMap<&'static str, Box<dyn ErasedOperation>>, Vec<String>) {
    let mut gates: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    let mut uri_path_labels: Vec<String> = Vec::new();
    macro_rules! gate {
        ($map:ident, $labels:ident; $($module:ident :: $operation:ident),+ $(,)?) => {
            $(
                $map.insert(
                    <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                    Box::new(
                        ModelledInputValidation::<crate::ops::$module::$operation<Eng>>::new(),
                    ) as Box<dyn ErasedOperation>,
                );
                $labels.extend(uri_path_label_paths(
                    <crate::ops::$module::$operation<Eng> as Operation>::INPUT_SCHEMA,
                ));
            )+
        };
    }
    // Every operation the model binds. A constraint is a property of the INPUT SHAPE, so
    // the gate goes in front of all of them: a request that violates the model is
    // refused the same way whichever operation it named.
    gate!(
        gates, uri_path_labels;
        create_rule::CreateRule,
        delete_rule::DeleteRule,
        get_rule::GetRule,
        list_rules::ListRules,
        list_tags_for_resource::ListTagsForResource,
        lock_rule::LockRule,
        tag_resource::TagResource,
        unlock_rule::UnlockRule,
        untag_resource::UntagResource,
        update_rule::UpdateRule,
    );
    uri_path_labels.sort_unstable();
    uri_path_labels.dedup();
    (gates, uri_path_labels)
}

/// The modelled-`ValidationException` gate in front of one operation `Op`.
///
/// `Op` is a type parameter only, naming the input shape and the schema the constraint
/// walk runs against. The gate holds no engine and no request state, so the operation it
/// guards stays the single owner of the request's behaviour — only the member paths that
/// input shape binds into the URI path, read off the model once at construction because
/// they are a fact of the shape and not of a request.
struct ModelledInputValidation<Op> {
    operation: PhantomData<fn() -> Op>,
    uri_path_labels: Vec<String>,
}

impl<Op: Operation> ModelledInputValidation<Op> {
    fn new() -> Self {
        Self {
            operation: PhantomData,
            uri_path_labels: uri_path_label_paths(Op::INPUT_SCHEMA),
        }
    }
}

impl<Op: Operation + 'static> ErasedOperation for ModelledInputValidation<Op> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            if let Some(refused) = refuse_invalid_input::<Op>(svc, req, &self.uri_path_labels) {
                return refused;
            }
            // Valid: the generated operation serves it, through the framework's own
            // deserialize -> validate -> handle -> serialize sequence (which finds
            // nothing left to refuse).
            match modelled_operation(svc, req) {
                Some(op) => op.run(svc, req, ctx).await,
                // Unreachable: this gate is only reached through `override_operation`,
                // which resolved its key from that same routing table.
                None => svc.definition().routing_error_response(req),
            }
        })
    }
}

/// The operation the model's routing table names for this request.
fn modelled_operation<'s>(
    svc: &'s dyn Service,
    req: &RtRequest,
) -> Option<&'s dyn ErasedOperation> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(key)
}

/// `Some(response)` when `Op`'s input violates a constraint the model declares: this
/// service's modelled `ValidationException`, 400, with both of its members set.
///
/// `None` when the request satisfies the model, or when it failed to READ for a reason
/// that is not a constraint violation — an unparseable body is a protocol failure, and
/// the framework words those per protocol.
fn refuse_invalid_input<Op: Operation>(
    svc: &dyn Service,
    req: &RtRequest,
    uri_path_labels: &[String],
) -> Option<RtResponse> {
    let protocol = svc.definition().protocol();
    let violations = violations_of::<Op>(protocol, req);
    // A clause quotes a `@pattern` as the service publishes that member's requirement,
    // which turns on whether the member is bound into the URI path, so the refusal is
    // rendered with the member paths THIS operation's input shape binds there.
    let error = crate::state::constraint_violations(&violations, uri_path_labels)?;
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

/// Every constraint `Op`'s input violates, as the framework's own walk reports them.
///
/// Both validation classes are classified by the framework's own
/// [`smithy_server_runtime::refusal_of`], so this carries no second copy of "what counts
/// as a constraint refusal": a value outside a declared bound is found by walking what
/// was read, and a required member absent from the request is found while READING it,
/// and `refusal_of` already states that they are one refusal. Anything it does not
/// recognise (an unreadable body, a missing header) is not a constraint refusal and is
/// left to the framework, which words it per protocol.
fn violations_of<Op: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Vec<Violation> {
    let error = match protocol.deserialize_input(req, Op::INPUT_SCHEMA) {
        Err(unreadable) => unreadable,
        Ok(mut deserializer) => {
            match <Op::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
                Err(absent_or_malformed) => ProtocolError::Serde(absent_or_malformed),
                // The request was read: the model's own constraint traits decide. This
                // is the same call the centralized sequence makes, so no rule is
                // restated here.
                Ok(input) => return validate(&input, Op::INPUT_SCHEMA),
            }
        }
    };
    match smithy_server_runtime::refusal_of(&error) {
        Some(Refusal::Invalid(violations)) => violations,
        // Authentication is settled by the router before any operation runs, so an
        // `Unauthenticated` refusal cannot arrive here — and it is not a constraint
        // violation in any case. `Refusal` is `#[non_exhaustive]`: a class added later
        // keeps the framework's own rendering until this service has a form for it.
        _ => Vec::new(),
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
        // a request that names no modelled operation resolves exactly as it did before.
        let key = self.entry.protocol().routing_key(req).ok()?;
        let gate = self.input_validation.get(key)?;
        Some(gate.as_ref())
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it.
        //
        // The gate above answers every such refusal of a MODELLED operation from the
        // modelled shape, so what reaches here is a refusal the gate could not render
        // (a modelled error that would not serialize). It is still stated in this
        // service's one validation envelope — `crate::state::validation_envelope`, the
        // same text the modelled body carries — rather than in the framework's, so one
        // refusal cannot read two ways depending on which layer rendered it. The code
        // stays `ValidationException`, which is what the framework already chose and
        // what the model declares, so code and message move together.
        match refusal {
            Refusal::Invalid(violations) => {
                crate::state::validation_envelope(violations, &self.uri_path_labels).map(
                    |message| Refused {
                        code: VALIDATION_EXCEPTION_CODE.to_string(),
                        message,
                    },
                )
            }
            // Authentication is the emulator's, not this service's: leave its wording.
            // `Refusal` is `#[non_exhaustive]`, so a class added later keeps the
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
