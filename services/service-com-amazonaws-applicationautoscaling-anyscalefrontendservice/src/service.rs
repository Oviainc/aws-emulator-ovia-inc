// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-applicationautoscaling-anyscalefrontendservice` service: how it claims requests and how it words the
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
//! - `override_operation` — EDITED HERE: the key and the handler stay the generated
//!   table's, but each operation is served through [`WithSubmittedValues`], so a request
//!   refused before the handler runs echoes the value the CALLER submitted — including a
//!   list-valued member, which the framework's walk can only summarise as `<n> items` — and
//!   quotes a `@pattern` requirement as the expression this service publishes.
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

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::{constraints, ProtocolError};
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, Service,
};
use std::sync::Arc;

/// The error code a constraint failure is named by: the one the framework's own refusal
/// channel picks for this service's protocol, restated here because a chosen [`Refused`]
/// replaces the code and the message TOGETHER — a client branches on the code, so one
/// without the other is worse than neither.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// Whether IAM lets a request perform the action it names — the decision taken ahead of
/// every operation of this service.
mod authorization;

/// The `com-amazonaws-applicationautoscaling-anyscalefrontendservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`WithSubmittedValues`]. Keyed by the routing key the protocol resolves — the key
    /// the generated table uses — and built from the SAME shared engine, so a wrapped
    /// operation reads and writes the one store.
    with_submitted_values: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`WithSubmittedValues`] and keyed by its operation shape name — the routing key
/// `ServerHttpProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(WithSubmittedValues::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                    ::std::sync::Arc::clone(&$engine),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut with_submitted_values = OperationRoutingTable::new();
        wrap_operations!(
            with_submitted_values,
            engine,
            delete_scaling_policy::DeleteScalingPolicy,
            delete_scheduled_action::DeleteScheduledAction,
            deregister_scalable_target::DeregisterScalableTarget,
            describe_scalable_targets::DescribeScalableTargets,
            describe_scaling_activities::DescribeScalingActivities,
            describe_scaling_policies::DescribeScalingPolicies,
            describe_scheduled_actions::DescribeScheduledActions,
            get_predictive_scaling_forecast::GetPredictiveScalingForecast,
            list_tags_for_resource::ListTagsForResource,
            put_scaling_policy::PutScalingPolicy,
            put_scheduled_action::PutScheduledAction,
            register_scalable_target::RegisterScalableTarget,
            tag_resource::TagResource,
            untag_resource::UntagResource,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            with_submitted_values,
        }
    }
}

/// One modelled operation, served so that a refusal of a modelled CONSTRAINT echoes the
/// value the caller submitted.
///
/// An AWS constraint-validation message is built from the request — `1 validation error
/// detected: Value '<value>' at '<member path>' failed to satisfy constraint:
/// <requirement>` — so the value between the quotes is part of the rule rather than prose.
/// The framework's walk stores a RENDERING of that value on each violation, and for a
/// collection it has only the count (`<n> items`): a clause on `DescribeScalableTargets`'
/// `ResourceIds` (`@length` max 50) therefore reports `Value '51 items'` where AWS reports
/// the fifty-one resource ids. The framework's `override_refusal` seam cannot fix that —
/// it is handed the violations alone, never the request they came from — so the restoring
/// step happens here, per operation, where the input can be re-read.
///
/// The other difference this wrapper restores is in the REQUIREMENT half, and is about the
/// member's constraint rather than about the request: a `@pattern` is quoted as the service
/// publishes the expression ([`as_published`]).
///
/// Nothing else is re-implemented, and no check is repeated with different words: the
/// violations come from the framework's own machinery (the protocol's deserializer and
/// `constraints::validate` over the built input), the sentence from the framework's own
/// `constraints::envelope`, and the code and status from the framework's refusal channel.
/// A request this wrapper does not refuse is handed to the generated
/// [`ErasedOperation::run`], which owns deserialize -> validate -> handle -> serialize,
/// telemetry and every modelled error. The cost is DELIBERATE: a valid request's body is
/// read twice, once here and once in the operation's own sequence, because the framework
/// hands no already-built input to an operation.
struct WithSubmittedValues<O, Eng> {
    operation: O,
    /// The shared engine, so the authorization gate can read the IAM and STS stores the
    /// decision is made from, and this service's own store for the target a request
    /// addresses. The framework hands `run` the SERVICE, which holds the engine behind an
    /// engine-erased accessor, so the gate reaches it through here.
    engine: Arc<Eng>,
}

/// `violation` with a `@pattern` requirement quoting the expression this service PUBLISHES,
/// and every other violation unchanged.
///
/// A Smithy `@pattern` is a find, so a model that requires the whole value to match writes
/// the expression with `^` and `$` around it; those anchors are the model's wrapping and not
/// part of the grammar a value has to satisfy, so a refusal quotes the expression without
/// them ([`service_common::validation::unanchored`], which is where that one reading lives).
/// The CHECK is still against the pattern as the model declares it: only the rendering
/// changes, and it changes for EVERY `@pattern` member of this service rather than for one
/// operation's.
fn as_published(violation: &constraints::Violation) -> constraints::Violation {
    let constraints::ViolationKind::Pattern(declared) = &violation.kind else {
        return violation.clone();
    };
    constraints::Violation {
        path: violation.path.clone(),
        kind: constraints::ViolationKind::Pattern(service_common::validation::unanchored(declared)),
        value: violation.value.clone(),
        sensitive: violation.sensitive,
    }
}

impl<O, Eng> WithSubmittedValues<O, Eng> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O, engine: Arc<Eng>) -> Self {
        Self { operation, engine }
    }
}

impl<O, Eng> ErasedOperation for WithSubmittedValues<O, Eng>
where
    O: Operation + ErasedOperation,
    Eng: Send + Sync + 'static,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The protocol is read, used and dropped inside this one statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the
            // await below would make this future non-`Send`.
            let refused = service_common::validation::refusing_violations::<O>(
                svc.definition().protocol(),
                req,
            );
            let refused: Vec<constraints::Violation> =
                refused.iter().map(as_published).collect();
            if let Some(message) = constraints::envelope(&refused) {
                return svc.definition().refusal_response(
                    ProtocolError::Invalid(refused),
                    Some(Refused {
                        code: VALIDATION_EXCEPTION.to_string(),
                        message,
                    }),
                    req,
                );
            }
            // The authorization gate, ahead of the handler: whether the caller may perform
            // the action at all is a fact of the CREDENTIAL the request presented and the IAM
            // policies in force on the principal it speaks for, not of the store the handler
            // would read ([`authorization::authorize`]). It is asked AFTER the request has
            // been read, because the condition keys IAM decides a grant by are members OF
            // that request.
            if let Some(denied) = authorization_refusal(&*self.engine, svc, req, ctx, O::NAME) {
                return denied;
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// This service's `AccessDeniedException` response for a request whose caller the IAM
/// policies in force do not permit to perform the action it named, or `None` when they do.
///
/// `operation` is the operation's own name ([`Operation::NAME`]), which is the action the
/// request asked for; the request DOCUMENT is read for the members IAM's condition keys come
/// from — an awsJson request carries them in its body, and a body that is no object carries
/// none, which is decided as a request that names neither a namespace nor a dimension.
///
/// The code and the status are this service's own, held once for every refusal about the
/// caller's ACCESS ([`crate::state::wire_refusals::ACCESS_DENIED_CODE`] and its status), and
/// the sentence is IAM's ([`authorization::Denied::message`]). The envelope is the
/// protocol's, so this refusal is rendered exactly like every other pre-dispatch one: the
/// code and the message the renderer would otherwise pick are REPLACED, and it is rendered
/// from an empty violation list because an authorization refusal names no member of the
/// request.
fn authorization_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    svc: &dyn Service,
    req: &RtRequest,
    ctx: &OperationContext<'_>,
    operation: &str,
) -> Option<RtResponse> {
    let document = req
        .body()
        .bytes()
        .and_then(|body| serde_json::from_slice::<serde_json::Value>(body).ok())
        .unwrap_or(serde_json::Value::Null);
    let refusal = authorization::authorize(engine, ctx, operation, &document).err()?;
    Some(svc.definition().refusal_response(
        ProtocolError::Invalid(Vec::new()),
        Some(Refused {
            code: crate::state::wire_refusals::ACCESS_DENIED_CODE.to_string(),
            message: refusal.message(),
        }),
        req,
    ))
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
        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and the operation it names is the one served — wrapped in
        // [`WithSubmittedValues`] so a constraint refusal echoes the submitted value. A
        // key the model does not name resolves to `None` here exactly as it does in the
        // generated table, so the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.with_submitted_values.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording, which is what this service wants: the only thing it changes
        // about a constraint refusal is which value the clause echoes, and that
        // needs the request — which this seam is not handed, and
        // [`WithSubmittedValues`] answers before the walk reaches here.
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

#[cfg(test)]
mod tests {
    //! The one thing about the wiring above that a wire test cannot see: that the
    //! hand-maintained `wrap_operations!` list is the WHOLE surface.

    use super::*;

    /// This service's Smithy model, as it ships beside this crate. Read at test time
    /// rather than embedded: it is megabytes of JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-applicationautoscaling-anyscalefrontendservice.json"
    );

    /// RULE applied to the whole surface: every operation the model declares is served
    /// behind [`WithSubmittedValues`], under the same key the generated routing table is
    /// keyed by.
    ///
    /// The names come from the MODEL rather than from a second hand list, so an operation
    /// added to the model and not to `wrap_operations!` fails here instead of silently
    /// echoing a summarised value in a refusal — and a key spelled differently from the
    /// generated table's fails here instead of silently never being reached by
    /// `override_operation`.
    #[test]
    fn every_modelled_operation_is_served_with_its_submitted_values() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");
        let operations: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter_map(|(id, _)| id.rsplit('#').next())
            .collect();
        assert!(
            operations.len() > 10,
            "case the model's surface: expected this service's operations, got {operations:?}"
        );

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        for operation in operations {
            assert!(
                service.entry.routing_table().get(operation).is_some(),
                "case {operation}: the generated table keys an operation by its shape name; \
                 this one resolved to nothing, so the wrapper's key spelling is wrong"
            );
            assert!(
                service.with_submitted_values.get(operation).is_some(),
                "case {operation}: every modelled operation is served behind \
                 WithSubmittedValues; this one is missing from wrap_operations!, so a \
                 constraint refusal of it echoes the framework's summary of a value instead \
                 of the value the caller submitted"
            );
        }
    }
}
