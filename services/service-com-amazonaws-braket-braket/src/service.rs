// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-braket-braket` service: how it claims requests and how it words the
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
//! Braket rejects a request BODY that breaks a modelled constraint at its front door,
//! before the request reaches the application code that raises the modelled
//! `ValidationException`: the answer is HTTP 400 with the error code
//! `BadRequestException`, a code this service's model does not declare
//! (`smithy_query(query="operation", operation="CreateSpendingLimit")` lists
//! `AccessDenied`/`DeviceRetired`/`InternalService`/`Throttling`/`ValidationException`
//! and nothing else). A constraint on a value bound to the request URI — a
//! `@httpLabel` path segment, a `@httpQuery` parameter — is NOT a body rejection and
//! keeps the modelled `ValidationException`, which is what the recordings show for a
//! malformed `spendingLimitArn` or `jobArn` label.
//!
//! The framework refuses every modelled constraint before a handler runs and words it
//! through [`Service::override_refusal`], which is handed the violations alone — no
//! operation, no request — so it cannot tell a body violation from a URI one (and
//! Braket binds `deviceArn` BOTH ways: in the creates' bodies and as `GetDevice`'s
//! label). [`service_common::front_door::FrontDoor`] is where that distinction lives:
//! each modelled operation is served wrapped in it, and the wrapper reads the HTTP
//! BINDING of the violated member off THAT operation's own input schema.

use service_common::front_door::FrontDoor;
use smithy_server_runtime::{
    http, ErasedOperation, OperationRoutingTable, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-braket-braket` emulated service: the generated [`ServiceDefinition`] plus
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
        // (`GetDevice`, whose only member is its URI label) simply never has a body
        // violation to find, which the wrapper reads off its own input schema.
        wrap_operations!(
            front_door,
            engine,
            cancel_job::CancelJob,
            cancel_quantum_task::CancelQuantumTask,
            create_job::CreateJob,
            create_quantum_task::CreateQuantumTask,
            create_spending_limit::CreateSpendingLimit,
            delete_spending_limit::DeleteSpendingLimit,
            get_device::GetDevice,
            get_job::GetJob,
            get_quantum_task::GetQuantumTask,
            list_tags_for_resource::ListTagsForResource,
            search_devices::SearchDevices,
            search_jobs::SearchJobs,
            search_quantum_tasks::SearchQuantumTasks,
            search_spending_limits::SearchSpendingLimits,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_spending_limit::UpdateSpendingLimit,
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
        // with `BadRequestException` before the operation runs, the way Braket's front
        // door refuses it. A key the model does not name resolves to `None` here
        // exactly as it does in the generated table, so the router's routing-error
        // path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.front_door.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. Returning `None` keeps the framework's default wording, which for
        // this restJson1 service is 400 `ValidationException` — the error EVERY Braket
        // operation models for input that fails its constraints, and what the service
        // answers for a value bound to the request URI (`GET /job/{jobArn}` with a
        // malformed ARN, `DELETE /tags/{resourceArn}` with no `tagKeys`) and for a
        // body the codec cannot read at all. The recordings confirm it: a malformed
        // `spendingLimitArn` or `jobArn` URI LABEL is answered `ValidationException`.
        // The one refusal this service words differently is a BODY that breaks a
        // modelled constraint, which `FrontDoor` answers before this walk is reached.
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
