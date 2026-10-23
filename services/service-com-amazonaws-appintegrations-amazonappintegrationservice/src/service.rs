// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-appintegrations-amazonappintegrationservice` service: how it claims requests and how it words the
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
//! ## This service's one edited seam: the front door
//!
//! AppIntegrations validates a request BODY against the operation's modelled input at a
//! layer AHEAD of the application code that owns its modelled errors, and rejects it
//! there: HTTP 400 with the error code `BadRequestException` — a shape this service's
//! model does not declare anywhere (`smithy_query(query="operation",
//! operation="CreateApplication")` lists `AccessDeniedException`,
//! `DuplicateResourceException`, `InternalServiceError`, `InvalidRequestException`,
//! `ResourceQuotaExceededException`, `ThrottlingException` and
//! `UnsupportedOperationException`, and nothing else) — and the fixed sentence
//! `"Invalid request body"`, which names neither the member nor the value. A constraint
//! on a value bound to the request URI (a `@httpLabel` path segment such as
//! `GetEventIntegration`'s `Name`, a `@httpQuery` parameter such as
//! `ListEventIntegrations`' `maxResults`) is NOT a body rejection: the service itself
//! refuses those, as the modelled validation error, naming what failed.
//!
//! That distinction cannot be made in [`Service::override_refusal`], which is handed
//! the violations ALONE — no operation, no request — and this service binds the same
//! member names both ways (`Name` travels in `CreateEventIntegration`'s body and in
//! `GetEventIntegration`'s path). [`service_common::front_door::FrontDoor`] is where it
//! lives: every modelled operation is served wrapped in it (through the
//! `override_operation` seam below), and the wrapper reads the HTTP BINDING of each
//! violated member off THAT operation's own input schema.

use service_common::front_door::FrontDoor;
use smithy_server_runtime::{
    http, ErasedOperation, OperationRoutingTable, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-appintegrations-amazonappintegrationservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`FrontDoor`] so a request BODY that breaks a modelled constraint is refused
    /// with `BadRequestException` before the operation runs. Keyed by the routing key
    /// the protocol resolves — the key the generated table uses — and built from the
    /// SAME shared engine, so a wrapped operation reads and writes the one store.
    front_door: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`FrontDoor`] and keyed by its operation shape name — the routing key
/// `ServerProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(FrontDoor::new(
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
        let mut front_door = OperationRoutingTable::new();
        // EVERY modelled operation, so the front door's rule is applied by one code
        // path rather than per operation: an operation that binds nothing to the body
        // (`GetEventIntegration`, whose only member is its URI label) simply never has
        // a body violation to find, which the wrapper reads off its own input schema.
        wrap_operations!(
            front_door,
            engine,
            create_application::CreateApplication,
            create_data_integration::CreateDataIntegration,
            create_data_integration_association::CreateDataIntegrationAssociation,
            create_event_integration::CreateEventIntegration,
            delete_application::DeleteApplication,
            delete_data_integration::DeleteDataIntegration,
            delete_event_integration::DeleteEventIntegration,
            get_application::GetApplication,
            get_data_integration::GetDataIntegration,
            get_event_integration::GetEventIntegration,
            list_application_associations::ListApplicationAssociations,
            list_applications::ListApplications,
            list_data_integration_associations::ListDataIntegrationAssociations,
            list_data_integrations::ListDataIntegrations,
            list_event_integration_associations::ListEventIntegrationAssociations,
            list_event_integrations::ListEventIntegrations,
            list_tags_for_resource::ListTagsForResource,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_application::UpdateApplication,
            update_data_integration::UpdateDataIntegration,
            update_data_integration_association::UpdateDataIntegrationAssociation,
            update_event_integration::UpdateEventIntegration,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            front_door,
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
        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // `FrontDoor` so a request BODY that breaks a modelled constraint is refused
        // with `BadRequestException` before the operation runs, the way AppIntegrations'
        // front door refuses it. A key the model does not name resolves to `None` here
        // exactly as it does in the generated table, so the router's routing-error path
        // is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.front_door.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. Returning `None` keeps the framework's default wording, which for
        // this restJson1 service is 400 `ValidationException` — what AppIntegrations
        // answers for a value bound to the request URI (`GET /eventIntegrations/{Name}`
        // with a malformed name, `GET /dataIntegrations?maxResults=0` outside the
        // modelled range) and for a body the codec cannot read at all. The one refusal
        // this service words differently is a BODY that breaks a modelled constraint,
        // which `FrontDoor` answers as `BadRequestException` before this walk is
        // reached, so there is nothing left for this seam to reword.
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
    /// rather than embedded: it is JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-appintegrations-amazonappintegrationservice.json"
    );

    /// How many operations this service's model declares, as a second, independent
    /// statement of its surface: a model that grows or shrinks fails here rather than
    /// quietly leaving the new operation out of the front door.
    const MODELLED_OPERATIONS: usize = 23;

    /// RULE applied to the whole surface: every operation the model declares is served
    /// behind [`FrontDoor`], under the same key the generated routing table is keyed by.
    ///
    /// The names come from the MODEL rather than from a second hand list, so an
    /// operation added to the model and not to `wrap_operations!` fails here instead of
    /// silently answering a body-constraint refusal in the framework's wording — and a
    /// key spelled differently from the generated table's fails here instead of
    /// silently never being reached by `override_operation`.
    #[test]
    fn every_modelled_operation_is_served_behind_the_front_door() {
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
             operations, got {operations:?}"
        );

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        for operation in operations {
            assert!(
                service.entry.routing_table().get(operation).is_some(),
                "case {operation}: the generated table keys an operation by its shape \
                 name; this one resolved to nothing, so the wrapper's key spelling is wrong"
            );
            assert!(
                service.front_door.get(operation).is_some(),
                "case {operation}: every modelled operation is served behind FrontDoor; \
                 this one is missing from wrap_operations!, so a body that breaks a \
                 modelled constraint is refused in the framework's wording instead of \
                 this service's"
            );
        }
    }
}
