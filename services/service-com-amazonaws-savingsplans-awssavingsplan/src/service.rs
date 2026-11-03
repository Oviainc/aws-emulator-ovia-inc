// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-savingsplans-awssavingsplan` service: how it claims requests and how it words the
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
//!   table's, but every operation is served through [`WithSubmittedValues`], so a
//!   request refused before its handler ran echoes the value the CALLER submitted —
//!   including a list-valued member, which the framework's walk can only summarise as
//!   `<n> items`.
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
use aws_smithy_schema_server::{envelope, ProtocolError};
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, Service, Violation,
};
use std::sync::Arc;

/// The error code this service answers an input-constraint violation with: its
/// own modelled `ValidationException`, which the model declares with
/// `@httpError 400`
/// (`smithy_query(query="shape",shape="ValidationException")`). It is also the
/// code the framework picks for a boundary refusal on restJson1, restated here
/// because `override_refusal` replaces the code and the message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The object this service roots a refused member's path at.
///
/// A savingsplans constraint-violation clause names the member path from the
/// REQUEST OBJECT inwards — `originalRequest.<member>` — not from the member
/// alone, so a caller parsing the clause to locate the offending member reads
/// that prefix. Recorded behaviour of the live service, which no model records
/// (fixture `savingsplans-tagging-error-paths-unknown-resource`: a malformed
/// `resourceArn` is refused with `… at 'originalRequest.resourceArn' …`) — and
/// exactly the kind of per-service wording `override_refusal` exists for.
const REQUEST_ROOT: &str = "originalRequest";

/// `violations` with every member path rooted at [`REQUEST_ROOT`].
///
/// ONLY the path is restated. The envelope and the requirement sentence stay the
/// framework's own (`Violation::message` plus
/// `aws_smithy_schema_server::envelope`), and so does the echoed value — which
/// [`WithSubmittedValues`] has already restored to the bytes the caller sent by
/// the time a modelled operation's refusal reaches here — so this service cannot
/// drift from the others in anything but the one thing it words
/// differently. In particular a
/// `@pattern` requirement keeps the model's expression VERBATIM, anchors and
/// all, which is how savingsplans quotes it — so this deliberately does not
/// delegate to `services/common`'s `aws_validation_refusal`, which quotes a
/// wrapping `^…$` pair unanchored for the services that do that.
fn rooted_at_request(violations: &[Violation]) -> Vec<Violation> {
    violations
        .iter()
        .map(|violation| Violation {
            path: if violation.path.is_empty() {
                REQUEST_ROOT.to_owned()
            } else {
                format!("{REQUEST_ROOT}.{}", violation.path)
            },
            kind: violation.kind.clone(),
            value: violation.value.clone(),
            sensitive: violation.sensitive,
        })
        .collect()
}

/// The `com-amazonaws-savingsplans-awssavingsplan` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`WithSubmittedValues`]. Keyed by the routing key the protocol resolves — the
    /// key the generated table uses — and built from the SAME shared engine, so a
    /// wrapped operation reads and writes the one store.
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
            create_savings_plan::CreateSavingsPlan,
            delete_queued_savings_plan::DeleteQueuedSavingsPlan,
            describe_savings_plan_rates::DescribeSavingsPlanRates,
            describe_savings_plans::DescribeSavingsPlans,
            describe_savings_plans_offering_rates::DescribeSavingsPlansOfferingRates,
            describe_savings_plans_offerings::DescribeSavingsPlansOfferings,
            list_tags_for_resource::ListTagsForResource,
            return_savings_plan::ReturnSavingsPlan,
            tag_resource::TagResource,
            untag_resource::UntagResource,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            with_submitted_values,
        }
    }
}

/// One modelled operation, served so that a constraint refusal echoes the value the
/// CALLER submitted.
///
/// A savingsplans constraint violation is stated in the AWS envelope
/// (`N validation error(s) detected: Value '<value>' at '<member path>' failed to
/// satisfy constraint: <requirement>`), and the value between the quotes is BUILT FROM
/// THE REQUEST: it is the only place the refusal reports the offending input back, so a
/// caller that parses it to recover what it sent must see its own bytes. The framework's
/// walk stores a RENDERING of that value on each violation, and for a collection the only
/// rendering it has is the count (`<n> items`), so a clause on `savingsPlanArns`
/// (`@length` max 100) reports `Value '101 items'` where AWS reports the ARNs themselves;
/// a long string echo is likewise stored elided (`<prefix>... [<n> bytes]`), text AWS
/// never emits.
///
/// [`Service::override_refusal`] cannot restore either — it is handed the violations
/// alone, never the request they came from — so the restoring step happens here, per
/// operation, where the input can be re-read.
///
/// Nothing else is re-implemented and no check is repeated with different words: the
/// violations come from the framework's own machinery (the protocol's deserializer and
/// `constraints::validate` over the built input, through
/// [`service_common::validation::refusing_violations`]), the sentence from the
/// framework's own `envelope`, the member paths from this service's one rooting rule
/// ([`rooted_at_request`]), and the code and status from the framework's refusal channel.
/// A request this wrapper does not refuse is handed to the generated
/// [`ErasedOperation::run`], which owns deserialize -> validate -> handle -> serialize,
/// telemetry and every modelled error — including the modelled `ValidationException` for
/// the rules a handler enforces that the model states only in prose. The cost is
/// DELIBERATE: a valid request's body is read twice, once here and once in the
/// operation's own sequence, because the framework hands no already-built input to an
/// operation.
struct WithSubmittedValues<O> {
    operation: O,
}

impl<O> WithSubmittedValues<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for WithSubmittedValues<O>
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
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the
            // await below would make this future non-`Send`.
            let refused = service_common::validation::refusing_violations::<O>(
                svc.definition().protocol(),
                req,
            );
            let stated = rooted_at_request(&refused);
            if let Some(message) = envelope(&stated) {
                return svc.definition().refusal_response(
                    ProtocolError::Invalid(stated),
                    Some(Refused {
                        code: VALIDATION_EXCEPTION.to_owned(),
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
        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // [`WithSubmittedValues`] so a constraint refusal echoes the submitted value.
        // A key the model does not name resolves to `None` here exactly as it does in
        // the generated table, so the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.with_submitted_values.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // savingsplans states the member path rooted at the request object —
        // `originalRequest.<member>` — for EVERY constraint class and every
        // operation, because that root is a property of the service's validator
        // and not of one member. The code stays the modelled
        // `ValidationException` (HTTP 400), which is what the framework would
        // have answered, so only the path changes; a refusal that is not a
        // constraint violation (an unauthenticated caller) keeps the
        // framework's own code and wording.
        //
        // [`WithSubmittedValues`] answers a refusal of a MODELLED operation before the
        // walk reaches here, with the same code, the same envelope and the same rooted
        // paths — and with the submitted value restored, which this seam cannot do
        // because it is handed no request. This body therefore still words a refusal
        // raised outside that path, and the two cannot drift: both root through
        // [`rooted_at_request`] and both render through the framework's `envelope`.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_owned(),
                message: envelope(&rooted_at_request(violations))?,
            }),
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
    //! The one thing about the wiring above that a wire test cannot see: that the
    //! hand-maintained `wrap_operations!` list is the WHOLE surface.

    use super::*;

    /// This service's Smithy model, as it ships beside this crate. Read at test time
    /// rather than embedded: it is megabytes of JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-savingsplans-awssavingsplan.json"
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
        assert_eq!(
            operations.len(),
            10,
            "case the model's surface: expected this service's ten operations, got \
             {operations:?}"
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
