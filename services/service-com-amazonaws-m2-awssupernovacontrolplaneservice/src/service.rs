// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-m2-awssupernovacontrolplaneservice` service: how it claims requests and how it words the
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
//! ## This service's edited seams: how a request it will not serve is stated
//!
//! Three refusals are m2's own rather than the framework's default, and each is decided
//! here, ahead of every handler:
//!
//! 1. a caller the service will not authorize — the modelled `AccessDeniedException`
//!    (403), one of this service's common errors ([`ACCESS_DENIED_CODE`]);
//! 2. a caller-supplied `resourceArn` that is not an ARN at all — the ARN layer's bad
//!    request, which precedes the modelled constraint walk ([`resource_arn_refusal`]);
//! 3. every other broken constraint — the modelled `ValidationException` (400), whose
//!    `message` [`validation`] words.
//!
//! The third one needs something [`Service::override_refusal`] is never handed: the
//! REQUEST. The framework's constraint walk stores a clause's echoed value elided (a
//! bounded prefix plus a byte count, a marker AWS never emits) and has no value at all
//! for a LIST member, which it summarises as its element count — while m2 echoes the
//! value the caller submitted, a list as `[<element>, …]`. Both are recoverable only
//! from the built input, so every modelled operation is served wrapped in
//! [`StatedPerOperation`], which asks `service_common::validation` for the framework's
//! OWN violations against that operation's input schema with each echo restored, and
//! hands them to the same [`validation`] module `override_refusal` uses — so the wording
//! lives in ONE place whichever path a refusal arrives by.
//!
//! The refusals a HANDLER words — an application that is not there, a name already taken,
//! a definition this emulator cannot read — are not here; they are built where the
//! resource is, in [`crate::state`] and the operation's own file.

mod validation;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::constraints::Violation;
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, ServerHttpProtocol, Service,
};
use std::sync::Arc;

/// The error code m2 answers a caller it will not authorize with: the modelled
/// `com.amazonaws.m2#AccessDeniedException`, `@error("client")` with
/// `@httpError(403)` and a required `message`
/// (`smithy_query(query="shape",shape="AccessDeniedException")`). It is one of this
/// SERVICE's common errors — `smithy_query(query="service_info")` lists
/// `AccessDeniedException`/`InternalServerException`/`ThrottlingException` — so every
/// operation declares it, and it is the shape a client's catch-block names for a
/// request that fails authorization ahead of the handler.
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// What that denial says: the sentence the model documents for the shape ("The account
/// or role doesn't have the right permissions to make the request."). The refusal is an
/// authorization decision about the CALLER, not about any member of the request, so it
/// states the documented sentence rather than anything built from the request.
const ACCESS_DENIED_MESSAGE: &str =
    "The account or role doesn't have the right permissions to make the request.";

/// The error code m2's ARN layer refuses a caller-supplied `resourceArn` that is not
/// an ARN with.
///
/// `BadRequestException` is NOT a shape in this service's model: the ARN is parsed
/// ahead of the application code that owns the modelled errors, so the refusal is
/// worded through the framework's refusal channel ([`Refused`]) instead of an error
/// enum. Recorded behaviour (corpus `simparity-wire-aws-v1`, fixture
/// `m2-environment-not-found-errors`, step `task#4` `ListTagsForResource`), where
/// `GET /tags/not-a-valid-arn` is answered 400 with
/// `x-amzn-errortype: BadRequestException`.
const BAD_REQUEST_CODE: &str = "BadRequestException";

/// What that refusal says, recorded with the code above: the ARN layer reports the
/// class of failure (the value is not an ARN) and names neither the member nor the
/// value — the member-by-member clause is what the modelled validation error carries,
/// for the refusals the application layer owns.
const INVALID_RESOURCE_ARN: &str = "Invalid input resource arn: bad syntax";

/// The member whose value goes through m2's ARN layer: the resource a tagging call
/// addresses, named by the violation path exactly as the constraint walk spells a
/// top-level member.
///
/// `smithy_query(query="operation",operation="ListTagsForResource,TagResource,
/// UntagResource")` shows the three tag operations binding `resourceArn` as an
/// `@httpLabel` targeting `Arn` on `/tags/{resourceArn}`, and no other operation of
/// this model binds a member of that name. Every OTHER `Arn`-typed input member (the
/// `roleArn` of `CreateApplication`, say) is an attribute of the resource being
/// written rather than the resource the call addresses, so it is not parsed here and
/// keeps the modelled validation refusal.
const RESOURCE_ARN_MEMBER: &str = "resourceArn";

/// How m2 states a request it refuses before any handler runs, from the constraint
/// violations the framework's own walk found: the ARN layer's bad request when one of
/// them is a `resourceArn` that is not an ARN, else the modelled `ValidationException`
/// [`validation`] words.
///
/// The ONE decision both refusal paths go through — the per-operation wrapper below and
/// [`Service::override_refusal`] — so the two cannot answer the same request with
/// different codes or different prose.
fn pre_handler_refusal(violations: &[Violation]) -> Option<Refused> {
    resource_arn_refusal(violations).or_else(|| validation::validation_refusal(violations))
}

/// m2's ARN-layer refusal for a `resourceArn` that is not syntactically an ARN, or
/// `None` when no violation is one.
///
/// The rule: a caller-supplied resource ARN is PARSED before the modelled constraint
/// walk reports on it, and a value that is not an ARN at all is a bad request
/// ([`BAD_REQUEST_CODE`] + [`INVALID_RESOURCE_ARN`]), not a pattern-constraint
/// violation. "Not an ARN at all" is what `service_common::arn::parse` — the tree's one
/// resource-ARN parse — declines: six colon-separated fields, the first literally `arn`,
/// with a non-empty partition, vendor and resource. A value that IS a well-formed ARN
/// but falls outside the modelled `Arn` pattern (a short account id, say) parsed fine, so
/// it keeps the framework's modelled validation refusal, which names the member and the
/// constraint it failed.
///
/// PRECEDENCE: one bad-syntax `resourceArn` is the WHOLE answer, even when the request
/// breaks other constraints too (a `TagResource` with both a non-ARN label and an
/// out-of-bound `tags` map). That follows from where the refusal comes from — the ARN
/// layer runs ahead of the constraint walk, so the walk's other clauses were never
/// reported in the first place — and it is why this returns one `Refused` rather than
/// folding the ARN clause into the framework's aggregate.
fn resource_arn_refusal(violations: &[Violation]) -> Option<Refused> {
    let bad_syntax = violations.iter().any(|violation| {
        // The whole path, not its last segment: `resourceArn` is an `@httpLabel`, so
        // it is always a TOP-LEVEL member of the input, and nothing nested under
        // another member is this refusal's business.
        //
        // WHICH constraint the walk reported is not part of the test: what the ARN
        // layer decides is whether the value PARSES as an ARN, so any violation on
        // this member whose value does not is bad syntax. (Today the `Arn` shape
        // declares only a `@pattern`, so a pattern failure is the only kind this
        // member can produce; keying on the parse rather than on the kind is what
        // keeps the rule true if the model ever adds a bound to it.)
        violation.path == RESOURCE_ARN_MEMBER
            && violation
                .value
                .as_deref()
                .is_some_and(|value| ::service_common::arn::parse(value).is_none())
    });
    bad_syntax.then(|| Refused {
        code: BAD_REQUEST_CODE.to_string(),
        message: INVALID_RESOURCE_ARN.to_string(),
    })
}

/// The `com-amazonaws-m2-awssupernovacontrolplaneservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`StatedPerOperation`] so a refused request is worded the way this service words
    /// it. Keyed by the routing key the protocol resolves — the key the generated table
    /// uses — and built from the SAME shared engine, so a wrapped operation reads and
    /// writes the one store.
    stated: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`StatedPerOperation`] and keyed by its operation shape name — the routing key
/// `ServerProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(StatedPerOperation::new(
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
        let mut stated = OperationRoutingTable::new();
        // EVERY modelled operation (37 of them, one per file in `src/ops/`), because a
        // constraint is a property of the INPUT SHAPE and `ValidationException` is a
        // common error of every operation of this service: an operation whose input
        // declares no constraint simply never has a violation to state, which the wrapper
        // reads off that operation's own input schema. A still-scaffolded operation is
        // wrapped too — the framework refuses a bad request before any handler runs, so
        // its refusals are this service's words even while its success path is a 501. An
        // operation added later is added here as well, or it answers a refusal in the
        // framework's wording rather than this service's; the test below fails when one
        // is missing.
        wrap_operations!(
            stated,
            engine,
            cancel_batch_job_execution::CancelBatchJobExecution,
            create_application::CreateApplication,
            create_data_set_export_task::CreateDataSetExportTask,
            create_data_set_import_task::CreateDataSetImportTask,
            create_deployment::CreateDeployment,
            create_environment::CreateEnvironment,
            delete_application::DeleteApplication,
            delete_application_from_environment::DeleteApplicationFromEnvironment,
            delete_environment::DeleteEnvironment,
            get_application::GetApplication,
            get_application_version::GetApplicationVersion,
            get_batch_job_execution::GetBatchJobExecution,
            get_data_set_details::GetDataSetDetails,
            get_data_set_export_task::GetDataSetExportTask,
            get_data_set_import_task::GetDataSetImportTask,
            get_deployment::GetDeployment,
            get_environment::GetEnvironment,
            get_signed_bluinsights_url::GetSignedBluinsightsUrl,
            list_application_versions::ListApplicationVersions,
            list_applications::ListApplications,
            list_batch_job_definitions::ListBatchJobDefinitions,
            list_batch_job_executions::ListBatchJobExecutions,
            list_batch_job_restart_points::ListBatchJobRestartPoints,
            list_data_set_export_history::ListDataSetExportHistory,
            list_data_set_import_history::ListDataSetImportHistory,
            list_data_sets::ListDataSets,
            list_deployments::ListDeployments,
            list_engine_versions::ListEngineVersions,
            list_environments::ListEnvironments,
            list_tags_for_resource::ListTagsForResource,
            start_application::StartApplication,
            start_batch_job::StartBatchJob,
            stop_application::StopApplication,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_application::UpdateApplication,
            update_environment::UpdateEnvironment,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            stated,
        }
    }
}

/// One modelled operation, served so that a request refused before its handler runs is
/// stated the way m2 states it: the value as the caller submitted it, a list as the list
/// it sent, and an enumerated type's value set in m2's own order.
///
/// Nothing else is re-implemented. A request this wrapper does not refuse is handed to
/// the generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle
/// -> serialize, telemetry and every modelled error. The violations come from the
/// framework's own machinery ([`stated_refusal`]), so this can neither refuse a request
/// the framework's walk would accept nor accept one it would refuse. The cost is that a
/// request it does not refuse is read twice, once here and once in the operation's own
/// sequence, because the framework hands no already-built input to an operation; the
/// structurally right place for the collecting half is the frozen crate, which
/// `services/common/src/validation.rs` records as the core seam this re-read exists only
/// until.
struct StatedPerOperation<O> {
    operation: O,
}

impl<O> StatedPerOperation<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

/// How m2 states every modelled constraint `req` breaks, for the operation `O`: the
/// violations the framework's own walk found, and the refusal [`pre_handler_refusal`]
/// gives them. `None` when the request breaks none — and for a failure that is NOT a
/// refusal (a body the codec cannot read at all, a member of the wrong type), which the
/// framework's own walk answers in its own envelope.
///
/// The collecting half is the tree's shared one,
/// `service_common::validation::refusing_violations`: one read of the request yields the
/// framework's own violations (the protocol's deserializer for an absent `@required`
/// member, `constraints::validate` over the built input) with each echoed value restored
/// to the bytes the caller sent — including, for a clause stated at a LIST member, the
/// list itself. Nothing is re-checked here and no second copy of that walk lives in this
/// crate: both halves are bounded where those bounds are stated.
fn stated_refusal<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<(Vec<Violation>, Refused)> {
    let violations = ::service_common::validation::refusing_violations::<O>(protocol, req);
    let stated = pre_handler_refusal(&violations)?;
    Some((violations, stated))
}

/// Record a refusal answered above as the operation row the framework sequence would have
/// recorded, so a refused request is not a hole in the telemetry series: the same
/// classification (from this response's status and the error shape it names), the same
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

impl<O> ErasedOperation for StatedPerOperation<O>
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
            // The definition is read, used and dropped inside each statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the
            // await below would make this future non-`Send`.
            let refused = stated_refusal::<O>(svc.definition().protocol(), req);
            if let Some((violations, stated)) = refused {
                let code = stated.code.clone();
                // The SAME renderer the centralized walk would have reached — the
                // service's own pre-dispatch envelope and the status the protocol gives
                // a refusal — with this service's code and message in place of the
                // framework's.
                let response = svc
                    .definition()
                    .refusal_response(ProtocolError::Invalid(violations), Some(stated), req);
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

    fn override_operation(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // `StatedPerOperation` so a refused request is worded the way this service words
        // it. A key the model does not name resolves to `None` here exactly as it does
        // in the generated table, so the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.stated.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal, before any
        // handler runs. Two of this service's refusals are not what the framework
        // words by default, and each replaces BOTH the code and the message — a client
        // branches on the code.
        //
        // 1. A caller the service will not authorize: the framework names its own
        //    `AccessDenied`, a code no m2 client branches on, but m2 answers the
        //    modelled `AccessDeniedException` (403, required `message`), which is one
        //    of the service's common errors and so the shape every operation declares
        //    for this refusal. The status belongs to the protocol (403) and is the same
        //    either way. `Refusal::Unauthenticated` carries no code of its own, so this
        //    is every pre-handler credential refusal — the framework does not
        //    distinguish a caller it cannot authenticate from one it will not
        //    authorize, and this service's model declares a shape only for the latter.
        // 2. A broken modelled constraint: the ARN layer's bad request, or the modelled
        //    `ValidationException` — see [`pre_handler_refusal`].
        //
        // A refusal of a request that named a modelled operation is answered by
        // `StatedPerOperation` before this walk is reached, with the REQUEST in hand.
        // What still arrives here is a refusal with no modelled operation to read the
        // request against; it is stated by the same function, so the two paths cannot
        // word one request differently — this one just has only the violations to state
        // from, so a clause's echoed value is the framework's elided rendering.
        match refusal {
            Refusal::Unauthenticated => Some(Refused {
                code: ACCESS_DENIED_CODE.to_string(),
                message: ACCESS_DENIED_MESSAGE.to_string(),
            }),
            Refusal::Invalid(violations) => pre_handler_refusal(violations),
            _ => None,
        }
    }

    fn override_routing_error(&self, req: &RtRequest) -> Option<RtResponse> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(&self, error: &ProtocolError) -> Option<RtResponse> {
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
        "/smithy-model/com-amazonaws-m2-awssupernovacontrolplaneservice.json"
    );

    /// RULE applied to the whole surface: every operation the model declares is served
    /// behind [`StatedPerOperation`], under the same key the generated routing table is
    /// keyed by.
    ///
    /// The names come from the MODEL rather than from a second hand list, so an operation
    /// added to the model and not to `wrap_operations!` fails here instead of silently
    /// answering a refusal in the framework's wording — and a key spelled differently
    /// from the generated table's fails here instead of silently never being reached by
    /// `override_operation`.
    #[test]
    fn every_modelled_operation_is_served_with_this_services_refusal_wording() {
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
        assert_eq!(
            operations.len(),
            37,
            "case the model's surface: expected this service's 37 operations, got \
             {operations:?}"
        );

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        for operation in operations {
            assert!(
                service.entry.routing_table().get(operation).is_some(),
                "case {operation}: the generated table keys an operation by its shape \
                 name; this one resolved to nothing, so the wrapper's key spelling is \
                 wrong"
            );
            assert!(
                service.stated.get(operation).is_some(),
                "case {operation}: every modelled operation is served behind \
                 StatedPerOperation; this one is missing from wrap_operations!, so it \
                 answers a refusal in the framework's wording instead of this service's"
            );
        }
    }
}
