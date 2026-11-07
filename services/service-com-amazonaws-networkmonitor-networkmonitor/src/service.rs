// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-networkmonitor-networkmonitor` service: how it claims requests and how it words the
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
//! ## This service's edited seams: the front door, and how a refusal is stated
//!
//! ## Who may call
//!
//! Every operation is authorized against the CALLING identity's IAM policies before it
//! reads or writes anything ([`authorization::authorize`]), which is the step AWS takes at
//! the IAM layer ahead of the service: an explicit `Deny` refuses the call with this
//! service's `AccessDeniedException` and nothing is stored.
//!
//! A refusal the front door does NOT own — a value bound to the request URI —
//! keeps the modelled `ValidationException` and is stated through
//! [`Service::override_refusal`] in the words AWS uses, which
//! [`service_common::aws_validation_refusal`] owns for every service on a JSON
//! protocol.
//!
//! ## The front door
//!
//! Network Monitor rejects a request BODY that breaks a modelled constraint at
//! its front door, before the request reaches the application code that raises
//! the modelled `ValidationException`: the answer is HTTP 400 with the error
//! code `BadRequestException`, a code this service's model does not declare
//! (`smithy_query(query="operation", operation="CreateMonitor")` lists
//! `AccessDenied`/`Conflict`/`InternalServer`/`ServiceQuotaExceeded`/`Throttling`/`ValidationException`
//! and nothing else). A constraint on a value bound to the request URI — a
//! `@httpLabel` path segment, a `@httpQuery` parameter — is NOT a body rejection
//! and keeps the modelled `ValidationException`.
//!
//! The framework refuses every modelled constraint before a handler runs and
//! words it through [`Service::override_refusal`], which is handed the
//! violations alone — no operation, no request — so it cannot tell a body
//! violation from a URI one. [`FrontDoor`] is where that distinction lives:
//! each modelled operation is served wrapped in it, and the wrapper reads the
//! HTTP BINDING of the violated member off that operation's own input schema.

pub mod authorization;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{self, path};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    OperationRoutingTable, Refusal, Refused, ServerHttpProtocol, Service, Violation,
};
use std::sync::Arc;

/// The error code Network Monitor's front door names for a request body that
/// breaks a modelled constraint. NOT a shape in this service's model: the
/// rejection happens before the application code that owns the modelled errors,
/// so it is worded through the framework's refusal channel
/// ([`smithy_server_runtime::Refused`]) rather than through an error enum.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// What the front door says about a body it refused.
///
/// The front door validates the body against the shape and reports the class of
/// failure, not the member: a caller fixes the request from the constraint the
/// model publishes. (The member-by-member clause the framework writes is what
/// this service's modelled `ValidationException` carries, for the refusals the
/// application layer owns.)
const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// The `com-amazonaws-networkmonitor-networkmonitor` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`FrontDoor`] so a body that breaks a modelled constraint is refused with
    /// `BadRequestException` before the operation runs. Keyed by the routing key
    /// the protocol resolves — the key the generated table uses — and built from
    /// the SAME shared engine, so a wrapped operation reads and writes the one
    /// store.
    front_door: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each
/// wrapped in [`FrontDoor`] and keyed by its operation shape name — the routing
/// key `ServerProtocol::routing_key` resolves and the generated table is keyed
/// by.
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
        // EVERY modelled operation, so the front door's rule is applied by one
        // code path rather than per operation: an operation that binds nothing
        // to the body simply never has a body violation to find, which the
        // wrapper reads off its input schema.
        wrap_operations!(
            front_door,
            engine,
            create_monitor::CreateMonitor,
            create_probe::CreateProbe,
            delete_monitor::DeleteMonitor,
            delete_probe::DeleteProbe,
            get_monitor::GetMonitor,
            get_probe::GetProbe,
            list_monitors::ListMonitors,
            list_tags_for_resource::ListTagsForResource,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_monitor::UpdateMonitor,
            update_probe::UpdateProbe,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            front_door,
        }
    }
}

/// One modelled operation, served behind Network Monitor's front door.
///
/// WHY this is a seam and not a line in each handler: the framework validates
/// every modelled constraint BEFORE a handler runs, so a body that breaks one
/// never reaches `handle` — and the seam that words that refusal
/// ([`Service::override_refusal`]) is handed the violations alone, with no way
/// to tell which HTTP location the violated value came from. A wrapper is handed
/// the request and the operation, so it can: it keeps only the violations on
/// members the operation's input schema binds to the BODY (no `@httpLabel`, no
/// `@httpQuery`, no `@httpHeader`), and answers those with
/// [`BAD_REQUEST_EXCEPTION`].
///
/// Nothing else is re-implemented. A body this wrapper does not refuse is handed
/// to the generated `ErasedOperation::run`, which owns deserialize -> validate ->
/// handle -> serialize, telemetry and every modelled error — including the
/// modelled `ValidationException` for a URI-bound value outside its constraint,
/// and for the rules a handler enforces that the model states only in prose.
///
/// The violations come from the framework's own machinery (the protocol's
/// deserializer and `constraints::validate` over the built input), so this
/// cannot find a different set from the walk that follows it.
struct FrontDoor<O> {
    operation: O,
}

impl<O> FrontDoor<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for FrontDoor<O>
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
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across
            // the await below would make this future non-`Send`.
            let refused = body_violations::<O>(svc.definition().protocol(), req);
            if !refused.is_empty() {
                return front_door_rejection(svc, req, refused);
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must
    /// not change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// Every modelled constraint `req`'s BODY breaks, for the operation `O`.
///
/// Empty when the body satisfies the model, when the operation binds nothing to
/// the body, and when the request cannot be read into `O`'s input shape at all —
/// a codec failure is not a constraint violation, so it is left to the
/// framework's own walk, which refuses it in this service's modelled
/// `ValidationException`.
fn body_violations<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Vec<Violation> {
    let Ok(mut deserializer) = protocol.deserialize_input(req, O::INPUT_SCHEMA) else {
        return Vec::new();
    };
    let Ok(input) = <O::Input as DeserializableStruct>::deserialize(&mut *deserializer) else {
        return Vec::new();
    };
    drop(deserializer);
    constraints::validate(&input, O::INPUT_SCHEMA)
        .into_iter()
        .filter(|violation| binds_to_body(O::INPUT_SCHEMA, &violation.path))
        .collect()
}

/// Whether `violation_path` names a member `input_schema` carries in the request
/// BODY.
///
/// Read off the member's own HTTP binding traits, so the model decides: a member
/// with no `@httpLabel` / `@httpQuery` / `@httpQueryParams` / `@httpHeader` /
/// `@httpPrefixHeaders` binding travels in the body. A nested path
/// (`probe.destination`, `probes.1.member.packetSize`) is placed by its FIRST
/// segment, the top-level member the whole subtree hangs off.
fn binds_to_body(input_schema: &Schema, violation_path: &str) -> bool {
    let member_name = violation_path
        .split('.')
        .next()
        .unwrap_or(violation_path);
    input_schema.members().iter().any(|member| {
        member
            .member_name()
            .is_some_and(|name| path::member(name) == member_name)
            && member.http_label().is_none()
            && member.http_query().is_none()
            && member.http_query_params().is_none()
            && member.http_header().is_none()
            && member.http_prefix_headers().is_none()
    })
}

/// The front door's answer to a body it refused: HTTP 400 with the error code
/// [`BAD_REQUEST_EXCEPTION`], rendered in THIS service's protocol envelope.
///
/// The envelope and the status stay with the framework's refusal channel
/// (`refusal_response`, which the generated entry renders from the service's
/// model-derived protocol); only the code and the message are this service's.
fn front_door_rejection(svc: &dyn Service, req: &RtRequest, refused: Vec<Violation>) -> RtResponse {
    svc.definition().refusal_response(
        ProtocolError::Invalid(refused),
        Some(Refused {
            code: BAD_REQUEST_EXCEPTION.to_string(),
            message: INVALID_REQUEST_BODY.to_string(),
        }),
        req,
    )
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
        // `FrontDoor` so a request body that breaks a modelled constraint is
        // refused with `BadRequestException` before the operation runs, the way
        // this service's front door refuses it. A key the model does not name
        // resolves to `None` here exactly as it does in the generated table, so
        // the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.front_door.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service state it. What reaches here is a value bound to the request URI
        // (`GET /monitors/{monitorName}` with an over-long name, a `probeId` label
        // outside its pattern) — a body that breaks a modelled constraint is
        // answered by `FrontDoor` before the framework's walk is reached.
        //
        // The wording is AWS's own, which `service_common` owns for every
        // service on a JSON protocol: the same 400 `ValidationException` and the
        // same `N validation error(s) detected: Value '<value>' at '<path>'
        // failed to satisfy constraint: <requirement>` envelope the framework
        // renders, with the ONE difference AWS has — a `@pattern` requirement is
        // quoted as the expression a value must satisfy, without the `^`/`$` the
        // model wraps it in to make the match total. The check still uses the
        // model's pattern as written; only the rendering drops them, so a client
        // that lifts the expression out of the message gets the one AWS
        // advertises. `None` for anything that is not a constraint refusal keeps
        // the framework's wording.
        service_common::aws_validation_refusal(refusal)
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
