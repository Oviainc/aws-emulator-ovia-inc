// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports` service: how it claims requests and how it words the
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
//!   resolves, wrapped in [`WordedRefusals`] so a constraint refusal can quote the value
//!   the caller submitted; see the note on that method for where this service authorizes
//!   its callers instead.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. THIS SERVICE words every boundary input-constraint refusal
//!   through [`validation`], the one renderer for its modelled `ValidationException`.
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
    Refused, Service, Violation,
};
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use service_common::validation::refusing_violations_with_echoes_over;

pub(crate) mod validation;

use validation::{envelope, MAX_VIOLATIONS, VALIDATION_EXCEPTION};

/// The member POSITIONS the restoring walk may visit when it rebuilds the value a refused
/// request carried — stated from THIS MODEL's largest collection, as
/// `service_common::validation::refusing_violations_with_echoes_over` documents.
///
/// `ResourceTagList` is `@length(0, 200)` over `ResourceTag`, a two-member structure
/// (`smithy_query(query="shape",shape="ResourceTagList,ResourceTag")`), and a refusal of
/// that bound is about a list one element PAST it: 201 elements contribute their two
/// members plus the element itself, and the list and the handful of other members of the
/// request sit above them. A composite renders only from children the walk reached, so a
/// ceiling below that count could not quote the list at all. It is a MODEL constant: no
/// request can raise it.
const MAX_QUOTED_POSITIONS: usize = (crate::state::MAX_RESOURCE_TAGS + 1) * 3 + 64;

/// The `com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`WordedRefusals`]. Keyed by the routing key the protocol resolves — the key the
    /// generated table uses — and built from the SAME shared engine, so a wrapped
    /// operation reads and writes the one store.
    worded_refusals: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`WordedRefusals`] and keyed by its operation shape name — the routing key
/// `ServerHttpProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(WordedRefusals::new(
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
        let mut worded_refusals = OperationRoutingTable::new();
        wrap_operations!(
            worded_refusals,
            engine,
            create_export::CreateExport,
            delete_export::DeleteExport,
            get_execution::GetExecution,
            get_export::GetExport,
            get_table::GetTable,
            list_executions::ListExecutions,
            list_exports::ListExports,
            list_tables::ListTables,
            list_tags_for_resource::ListTagsForResource,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_export::UpdateExport,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            worded_refusals,
        }
    }
}

/// One modelled operation, served so that a refusal of a modelled CONSTRAINT is worded the
/// way this service words one — quoting the value the caller SUBMITTED.
///
/// Nothing here re-implements a check, and no check is repeated with different words: the
/// violations come from the framework's own machinery (the protocol's deserializer and
/// `constraints::validate` over the built input) and every submitted value from
/// `service_common::validation`'s single walk over that input; the status and the error code
/// come from the framework's refusal channel, and only the SENTENCE is this service's (see
/// [`validation`]). A request this wrapper does not refuse is handed to the generated
/// [`ErasedOperation::run`], which owns deserialize -> validate -> handle -> serialize,
/// telemetry and every modelled error.
///
/// The wrapper exists because the clause needs the REQUEST: for a list-valued member the
/// framework's violation carries only `<n> items`, a summary AWS never puts on the wire,
/// and the `override_refusal` seam is handed the violations alone. The cost is DELIBERATE:
/// a valid request's body is read twice, once here and once in the operation's own
/// sequence, because the framework hands no already-built input to an operation.
///
/// The structurally right home for this is the runtime seam itself — `override_refusal`
/// taking the built input (or the request) beside the violations, so no wrapper and no
/// second read were needed. That signature lives in `vendor/`, which is read-only here.
struct WordedRefusals<O> {
    operation: O,
}

impl<O> WordedRefusals<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for WordedRefusals<O>
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
            // The protocol is read, used and dropped inside this one statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let mut refused = refusing_violations_with_echoes_over::<O>(
                svc.definition().protocol(),
                req,
                MAX_QUOTED_POSITIONS,
            );
            refused.truncate(MAX_VIOLATIONS);
            if let Some(message) = envelope(&refused) {
                let violations: Vec<Violation> =
                    refused.into_iter().map(|(violation, _)| violation).collect();
                return svc.definition().refusal_response(
                    ProtocolError::Invalid(violations),
                    Some(Refused {
                        code: VALIDATION_EXCEPTION.to_string(),
                        message,
                    }),
                    req,
                );
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
        // and the operation it names is the one served — wrapped in [`WordedRefusals`] so a
        // constraint refusal quotes the value the caller submitted. A key the model does
        // not name resolves to `None` here exactly as it does in the generated table, so
        // the router's routing-error path is unchanged.
        //
        // AUTHORIZATION IS NOT HERE. This service authorizes in the handlers whose model
        // declares `AccessDeniedException` (403) — `CreateExport`, `TagResource`,
        // `UntagResource`, `ListTagsForResource` — because the decision is per ACTION and
        // per RESOURCE, which only the operation knows: see
        // `crate::state::authorization`, which resolves the caller through STS's own store
        // and evaluates the IAM store's documents through
        // `service_common::iam_authorization`. An operation whose model declares no 403 is
        // not judged, since answering a caller with an error its operation does not model
        // would be a different divergence.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.worded_refusals.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound). This service words every
        // one of them through the ONE renderer in [`validation`], so the code
        // (`ValidationException`) and the message are replaced together and no operation
        // can word a refusal its own way. `None` from there keeps the framework's
        // wording, which is what an unworded violation class gets.
        //
        // [`WordedRefusals`] answers every constraint refusal of a MODELLED operation
        // before the walk reaches here, because it is the only layer that still has the
        // request and so can quote a composite member's submitted value; this is the
        // fallback for one that does not reach it, worded from the same clause builder.
        validation::refused(refusal)
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
    //! The one thing a wire test cannot see: that the hand-maintained `wrap_operations!`
    //! list is the WHOLE surface. How a clause is worded is [`validation`]'s own tests and
    //! `tests/all/validation_message_rendering.rs`.

    use super::*;

    /// This service's Smithy model, as it ships beside this crate. Read at test time
    /// rather than embedded: it is megabytes of JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports.json"
    );

    /// RULE applied to the whole surface: every operation the model declares is served
    /// behind [`WordedRefusals`], under the same key the generated routing table is keyed
    /// by.
    ///
    /// The names come from the MODEL rather than from a second hand list, so an operation
    /// added to the model and not to `wrap_operations!` fails here instead of silently
    /// answering a constraint refusal with the framework's summary of a composite value.
    #[test]
    fn every_modelled_operation_words_its_own_refusals() {
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
            operations.len() >= 12,
            "case the model's surface: expected this service's operations, got {operations:?}"
        );

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        for operation in operations {
            assert!(
                service.entry.routing_table().get(operation).is_some(),
                "case {operation}: the generated table keys an operation by its shape \
                 name; this one resolved to nothing, so the wrapper's key spelling is wrong"
            );
            assert!(
                service.worded_refusals.get(operation).is_some(),
                "case {operation}: every modelled operation is served behind \
                 WordedRefusals; this one is missing from wrap_operations!, so a \
                 constraint refusal of it is worded without the value the caller sent"
            );
        }
    }
}
