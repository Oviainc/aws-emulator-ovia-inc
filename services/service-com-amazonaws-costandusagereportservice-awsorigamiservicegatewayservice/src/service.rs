// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-costandusagereportservice-awsorigamiservicegatewayservice` service: how it claims requests and how it words the
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
//!   correctly. THIS SERVICE answers it with the same operation the generated table
//!   resolves, wrapped in [`SubmittedEchoes`] so a constraint refusal quotes the value
//!   the caller submitted rather than the framework's elision of it.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. THIS SERVICE words every boundary input-constraint refusal
//!   through the shared renderer; see the note on that method.
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
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, Service, ServerHttpProtocol, Violation,
};
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;

/// The `com-amazonaws-costandusagereportservice-awsorigamiservicegatewayservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`SubmittedEchoes`]. Keyed by the routing key the protocol resolves — the key the
    /// generated table uses — and built from the SAME shared engine, so a wrapped
    /// operation reads and writes the one store.
    submitted_echoes: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`SubmittedEchoes`] and keyed by its operation shape name — the routing key
/// `ServerHttpProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(SubmittedEchoes::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
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
        let mut submitted_echoes = OperationRoutingTable::new();
        wrap_operations!(
            submitted_echoes,
            engine,
            delete_report_definition::DeleteReportDefinition,
            describe_report_definitions::DescribeReportDefinitions,
            list_tags_for_resource::ListTagsForResource,
            modify_report_definition::ModifyReportDefinition,
            put_report_definition::PutReportDefinition,
            tag_resource::TagResource,
            untag_resource::UntagResource,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            submitted_echoes,
        }
    }
}

/// How this service states every modelled constraint `req` breaks, for the operation `O`:
/// the violations the framework's own walk found, and the refusal the shared renderer
/// words them as. `None` when the request breaks none — and for a failure that is NOT a
/// constraint refusal (a body the codec cannot read at all, a member of the wrong type),
/// which the framework's own walk answers in its own envelope.
///
/// The collecting half is the tree's shared one,
/// `service_common::validation::refusing_violations`: ONE read of the request yields the
/// framework's own violations (the protocol's deserializer for an absent `@required`
/// member, the constraint walk over the built input) with each echoed value restored to
/// the bytes the caller SENT. That restoration is why this layer exists at all: a clause
/// echoes the offending value, the walk stores a byte-bounded prefix of it followed by a
/// count marker, and a member of this model may carry a value longer than that bound — so
/// a refusal of such a value would quote text the caller never submitted. Nothing is
/// re-checked here and no constraint is enforced twice: the same walk decides, so this can
/// neither refuse a request the framework would accept nor accept one it would refuse.
fn stated_refusal<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<(Vec<Violation>, Refused)> {
    let violations = ::service_common::validation::refusing_violations::<O>(protocol, req);
    let stated = ::service_common::validation_message::aws_validation_refusal(
        &Refusal::Invalid(violations.clone()),
    )?;
    Some((violations, stated))
}

/// Record a refusal answered above as the operation row the framework sequence would have
/// recorded, so a refused request is not a hole in the telemetry series: the same
/// classification (from this response's status and the error code it names), the same
/// folded handle (this service's signing name and the operation's own name), and the same
/// already sanitized request metadata. Everything is gated on a client collecting, exactly
/// as the framework gates it.
fn record_refusal(
    ctx: &OperationContext<'_>,
    operation: &'static str,
    error_code: &str,
    response: &RtResponse,
    latency: std::time::Duration,
) {
    if !::emulator_telemetry::recording_active() {
        return;
    }
    let handle = ::emulator_telemetry::OperationHandle::folded(
        ctx.identity().signing_name_static(),
        operation,
    );
    ::emulator_telemetry::record_generated_operation_with_handle(
        &handle,
        latency,
        Some(::emulator_telemetry::OperationFailure::classify(
            response.status().as_u16(),
            error_code,
        )),
        smithy_server_runtime::RequestTelemetry::from_headers(&ctx.parts().headers)
            .into_metadata(),
    );
}

/// One modelled operation, served so that a refusal of a modelled constraint quotes the
/// value the caller SUBMITTED.
///
/// A request this wrapper does not refuse is handed to the generated
/// [`ErasedOperation::run`], which owns deserialize -> validate -> handle -> serialize,
/// telemetry and every modelled error. The status and the error code come from the
/// framework's own refusal channel, and the sentence from the shared renderer, so wrapping
/// changes nothing but the value a clause quotes.
///
/// The wrapper exists because the restored echo needs the REQUEST, and the
/// `override_refusal` seam is handed the violations alone. The cost is DELIBERATE: a valid
/// request's body is read twice, once here and once in the operation's own sequence,
/// because the framework hands no already-built input to an operation. The structurally
/// right home for it is the runtime seam itself — a violation carrying the unelided value —
/// which lives in `vendor/` and is read-only here.
struct SubmittedEchoes<O> {
    operation: O,
}

impl<O> SubmittedEchoes<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for SubmittedEchoes<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let started = std::time::Instant::now();
            // The definition is read, used and dropped inside this one statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let refused = stated_refusal::<O>(svc.definition().protocol(), req);
            if let Some((violations, stated)) = refused {
                let code = stated.code.clone();
                // The SAME renderer the centralized walk would have reached — this
                // service's envelope and the status the protocol gives a refusal — with
                // the clause quoting what the caller sent.
                let response = svc.definition().refusal_response(
                    ProtocolError::Invalid(violations),
                    Some(stated),
                    req,
                );
                record_refusal(ctx, O::NAME, &code, &response, started.elapsed());
                return response;
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
        // and the operation it names is the one served — wrapped in [`SubmittedEchoes`] so
        // a constraint refusal quotes the value the caller submitted. A key the model does
        // not name resolves to `None` here exactly as it does in the generated table, so
        // the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.submitted_echoes.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) so this service can
        // state it the way the emulated service does.
        //
        // This service is on awsJson1_1 (`smithy_query(query="digest")`), where a
        // constraint refusal is `400 ValidationException` carrying the counted envelope
        // over one clause per offending member — which is the framework's own wording
        // except for how a `@pattern` requirement QUOTES the expression: the requirement
        // stated is the expression a value has to satisfy, without the `^` / `$` the
        // model wraps it in to make that match total, so a client that lifts the quoted
        // expression out of the message and re-validates locally gets the expression the
        // service advertises. The CHECK still runs the model's pattern as written.
        //
        // Nothing here is keyed to an operation, a member or a value: the rule holds for
        // every `@pattern`-constrained member of every operation of this service, and the
        // envelope, the member paths, the error code and every other requirement stay the
        // shared ones, so no member of this model can be worded its own way.
        //
        // [`SubmittedEchoes`] answers every constraint refusal of a modelled operation
        // before the centralized walk reaches here, because it is the only layer that
        // still has the request and so can quote the value the caller submitted whole;
        // this stays the fallback for a refusal that does not reach that wrapper, worded
        // by the same renderer.
        ::service_common::validation_message::aws_validation_refusal(refusal)
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
