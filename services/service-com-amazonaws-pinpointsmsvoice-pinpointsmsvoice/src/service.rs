// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-pinpointsmsvoice-pinpointsmsvoice` service: how it claims requests and how it words the
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

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, Service,
};
use std::sync::Arc;

/// The body member this service reports an error's detail text in ON THE WIRE.
///
/// Every exception shape in this model declares `Message` and none carries a `@jsonName`
/// (`smithy_query(query="shape",shape="BadRequestException,NotFoundException,AlreadyExistsException,LimitExceededException,InternalServiceErrorException,TooManyRequestsException",model_path="services/service-com-amazonaws-pinpointsmsvoice-pinpointsmsvoice/smithy-model/com-amazonaws-pinpointsmsvoice-pinpointsmsvoice.json")`),
/// so the framework's restJson1 error renderer writes the text under
/// [`MODELLED_DETAIL_MEMBER`] — while the service itself answers it under this name, which
/// is also the member the framework's OWN pre-handler refusals and the scaffold's 501 use.
/// Renaming it keeps this service's two error paths reporting their text in ONE member,
/// whichever layer produced the failure.
const WIRE_DETAIL_MEMBER: &str = "message";

/// The member the framework's restJson1 renderer writes a MODELLED error's detail text in:
/// the member name the model declares, since a restJson1 member with no `@jsonName` goes on
/// the wire under its Smithy name.
const MODELLED_DETAIL_MEMBER: &str = "Message";

/// Every exception shape this service's model declares, by the wire code (`__type`) it
/// renders under — the same `smithy_query` call [`WIRE_DETAIL_MEMBER`] cites, which is
/// where this list comes from and the only thing that may extend it.
///
/// The rename is keyed on THIS set rather than on the status, so a failure whose class this
/// service does not model — the scaffold's own 501 `NotImplemented`, an authentication
/// refusal — keeps the member the framework wrote its text in.
const MODELLED_ERROR_SHAPES: [&str; 6] = [
    "AlreadyExistsException",
    "BadRequestException",
    "InternalServiceErrorException",
    "LimitExceededException",
    "NotFoundException",
    "TooManyRequestsException",
];

/// Serves one modelled operation and reports whatever it answers in this service's own error
/// body: the detail text in [`WIRE_DETAIL_MEMBER`], through the one renaming
/// `service-common` states for every service whose wire member differs from its model's.
///
/// It re-implements nothing else: the request is handed to the generated
/// [`ErasedOperation::run`], which owns deserialize -> validate -> handle -> serialize and
/// every status, code and message it answers with.
///
/// WHY a wrapper and not [`Service::override_refusal`]: that seam chooses the code and the
/// message of a FRAMEWORK refusal, and cannot touch the body member a MODELLED error a
/// handler returned was serialized into — which is the half that has to be renamed, since
/// the framework path already writes [`WIRE_DETAIL_MEMBER`].
struct OnTheWire<O> {
    operation: O,
}

impl<O> OnTheWire<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for OnTheWire<O>
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
            service_common::error_detail_member::with_renamed_detail_member(
                self.operation.run(svc, req, ctx).await,
                &MODELLED_ERROR_SHAPES,
                MODELLED_DETAIL_MEMBER,
                WIRE_DETAIL_MEMBER,
            )
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// Serve `operation` through [`OnTheWire`], filed under its own modelled
/// [`Operation::NAME`] — the routing key the protocol resolves and the generated table is
/// keyed by, so the key cannot be spelled by hand.
fn served<O: Operation + ErasedOperation + 'static>(
    table: &mut OperationRoutingTable,
    operation: O,
) {
    table.add_operation(O::NAME, Box::new(OnTheWire::new(operation)));
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table` through [`served`].
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            served(
                &mut $table,
                crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
            );
        )+
    };
}

/// The `com-amazonaws-pinpointsmsvoice-pinpointsmsvoice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each served through
    /// [`OnTheWire`] so every error reports its detail text in the member this service puts
    /// it in. Keyed by the routing key the protocol resolves — the key the generated table
    /// uses — and built from the SAME shared engine, so a wrapped operation reads and
    /// writes the one store.
    on_the_wire: OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut on_the_wire = OperationRoutingTable::new();
        // EVERY modelled operation, so the rule is applied by ONE code path rather than
        // per operation: an operation that answers no modelled error simply never has a
        // body to rename.
        wrap_operations!(
            on_the_wire,
            engine,
            create_configuration_set::CreateConfigurationSet,
            create_configuration_set_event_destination::CreateConfigurationSetEventDestination,
            delete_configuration_set::DeleteConfigurationSet,
            delete_configuration_set_event_destination::DeleteConfigurationSetEventDestination,
            get_configuration_set_event_destinations::GetConfigurationSetEventDestinations,
            list_configuration_sets::ListConfigurationSets,
            send_voice_message::SendVoiceMessage,
            update_configuration_set_event_destination::UpdateConfigurationSetEventDestination,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            on_the_wire,
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
        // This service signs as `sms-voice` — a signing name it SHARES with Pinpoint SMS
        // Voice V2, a wire service of its own (`sms-voice`, `awsJson1_0`) against this
        // one's `sms-voice.pinpoint`, `restJson1`. The router resolves the routing key
        // from the SigV4 credential scope first, and a host-addressed request resolves to
        // the first label of the endpoint host, which is `sms-voice` for both — so
        // claiming on the signing name ALONE makes each claim the other's traffic, and
        // the router, which refuses rather than guesses, answers `AmbiguousService` for
        // ALL of it.
        //
        // So the claim is narrowed to this service's own modelled wire surface, by the
        // one rule `service-common` states for every service under a shared signing
        // name. This model is `restJson1` and every operation binds a URI under
        // `/v1/...`, so its surface is that path space (the generated
        // `ROUTE_PATH_SEGMENTS`); the V2 peer's `awsJson1_0` traffic posts to the root
        // and is claimed by the service its `X-Amz-Target` names.
        service_common::claims_request(
            parts,
            self.entry.identity().signing_name(),
            crate::ROUTE_PATH_SEGMENTS,
            None,
        )
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and the operation it names is the one served — wrapped in [`OnTheWire`] so a
        // modelled error reports its detail text in the member this service puts it in,
        // the same member the framework's own refusals already use. A key the model does
        // not name resolves to `None` here exactly as it does in the generated table, so
        // the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.on_the_wire.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // Pinpoint SMS Voice states "the input you provided is invalid" as its own
        // modelled `BadRequestException` (HTTP 400) — the error every operation in this
        // service declares for a bad request. A refusal the framework found from the
        // model (an absent required member, a value outside a declared bound, an entry
        // outside an enum's value set) is exactly that failure, so it is worded with
        // this service's code rather than the framework's generic
        // `ValidationException`; the status stays 400 either way, and a client
        // branching on the code sees the one the real service sends.
        //
        // `Unauthenticated` keeps the framework's wording: it is not a modelled error
        // of this service at all.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: "BadRequestException".to_string(),
                message: aws_smithy_schema_server::envelope(violations)
                    .unwrap_or_else(|| "The input you provided is invalid.".to_string()),
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
    use super::*;

    /// This service's Smithy model, as it ships beside this crate. Read at test time rather
    /// than embedded: it is JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-pinpointsmsvoice-pinpointsmsvoice.json"
    );

    /// How many operations this service's model declares, as a second, independent
    /// statement of its surface: a model that grows or shrinks fails here rather than
    /// quietly leaving the new operation out of the wrapper.
    const MODELLED_OPERATIONS: usize = 8;

    /// RULE applied to the whole surface: every operation the model declares is served
    /// through [`OnTheWire`], under the same key the generated routing table is keyed by —
    /// so an error's detail text lands in the member this service reports it in for EVERY
    /// operation rather than for the ones someone remembered.
    ///
    /// The names come from the MODEL rather than from a second hand-written list, so an
    /// operation added to the model and not to `wrap_operations!` fails here instead of
    /// silently answering under the modelled member — and a key spelled differently from
    /// the generated table's fails here instead of silently never being reached by
    /// `override_operation`.
    #[test]
    fn every_modelled_operation_reports_its_errors_on_this_services_wire() {
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
            MODELLED_OPERATIONS,
            "case the model's surface: expected this service's {MODELLED_OPERATIONS} \
             operations, got {}",
            operations.len()
        );

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        for operation in operations {
            assert!(
                service.entry.routing_table().get(operation).is_some(),
                "case {operation}: the generated table keys an operation by its shape name; \
                 this one resolved to nothing, so the wrapper's key spelling is wrong"
            );
            assert!(
                service.on_the_wire.get(operation).is_some(),
                "case {operation}: every modelled operation is served through OnTheWire; \
                 this one is missing from wrap_operations!, so its detail text would land \
                 in the modelled member instead of the one this service reports it in"
            );
        }
    }

    /// RULE: the shapes the rename is keyed on are exactly the error shapes this model
    /// declares, read off the model rather than trusted to a hand-written list — a shape
    /// added to the model and not to `MODELLED_ERROR_SHAPES` would keep reporting its text
    /// in the modelled member.
    #[test]
    fn every_modelled_error_shape_is_keyed_for_the_rename() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");
        let mut declared: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| shape["traits"].get("smithy.api#error").is_some())
            .filter_map(|(id, _)| id.rsplit('#').next())
            .collect();
        declared.sort_unstable();
        let mut keyed: Vec<&str> = MODELLED_ERROR_SHAPES.to_vec();
        keyed.sort_unstable();
        assert_eq!(
            keyed, declared,
            "expected the rename to be keyed on exactly the model's error shapes, got \
             {keyed:?} against {declared:?}"
        );
    }
}
