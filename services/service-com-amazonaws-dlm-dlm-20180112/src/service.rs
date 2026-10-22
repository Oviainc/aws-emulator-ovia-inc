// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-dlm-dlm-20180112` service: how it claims requests and how it words the
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
//! - `override_operation` — which operation serves a request. EDITED HERE: the key
//!   and the handler stay the generated table's, but an operation that takes an
//!   `ExecutionRoleArn` is served through [`PassRoleAuthorized`], which authorizes
//!   `iam:PassRole` for that ARN ahead of the operation — the order the real
//!   front door works in, and the only place a refusal can precede the member's
//!   own `@pattern`.
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
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    OperationRoutingTable, Refusal, Refused, ServerHttpProtocol, Service,
};
use std::sync::Arc;

/// The `message` member of the `AccessDeniedException` this service answers a
/// denied `iam:PassRole` with.
static ACCESS_DENIED_MESSAGE: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#AccessDeniedException$message",
        "smithy.framework",
        "message",
    ),
    ShapeType::String,
    "message",
    0,
)
.with_xml_name("Message");

/// Schema for the `AccessDeniedException` shape.
///
/// Deliberately in the `smithy.framework` namespace, as the scaffold's own
/// synthetic shapes are: the DLM model declares no `AccessDeniedException`
/// (`iam:PassRole` is authorized in the front door, ahead of the operation), so
/// this shape must not collide with a modelled one. The wire `__type` and the
/// `x-amzn-errortype` header carry [`crate::state::ACCESS_DENIED_EXCEPTION`],
/// which `serialize_error` is given explicitly.
static ACCESS_DENIED_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "smithy.framework#AccessDeniedException",
        "smithy.framework",
        "AccessDeniedException",
    ),
    ShapeType::Structure,
    &[&ACCESS_DENIED_MESSAGE],
);

/// The payload of an `AccessDeniedException`: why the caller may not pass the
/// role it named.
#[derive(Debug, Clone)]
struct AccessDeniedException {
    message: String,
}

impl SerializableStruct for AccessDeniedException {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&ACCESS_DENIED_MESSAGE, self.message.as_str())?;
        Ok(())
    }
}

/// An operation input that names an `ExecutionRoleArn` for Amazon Data Lifecycle
/// Manager to assume: the create states it `@required`, the update states it
/// optionally, and both must have `iam:PassRole` for it before they run.
trait NamesExecutionRole {
    /// The `ExecutionRoleArn` this request names, when it names one.
    fn execution_role_arn(&self) -> Option<&str>;
}

impl NamesExecutionRole for crate::ops::types::CreateLifecyclePolicyInput {
    fn execution_role_arn(&self) -> Option<&str> {
        Some(self.execution_role_arn.as_str())
    }
}

impl NamesExecutionRole for crate::ops::types::UpdateLifecyclePolicyInput {
    fn execution_role_arn(&self) -> Option<&str> {
        self.execution_role_arn.as_deref()
    }
}

/// The `com-amazonaws-dlm-dlm-20180112` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The operations that take an `ExecutionRoleArn`, each the generated table's
    /// own operation wrapped in [`PassRoleAuthorized`] and built from the SAME
    /// shared engine, so a wrapped operation reads and writes the one store.
    /// Keyed by the routing key the protocol resolves, which is the key the
    /// generated table uses.
    pass_role_authorized: OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut pass_role_authorized = OperationRoutingTable::new();
        pass_role_authorized.add_operation(
            "CreateLifecyclePolicy",
            Box::new(PassRoleAuthorized::new(
                crate::ops::create_lifecycle_policy::CreateLifecyclePolicy::new(Arc::clone(
                    &engine,
                )),
            )) as Box<dyn ErasedOperation>,
        );
        pass_role_authorized.add_operation(
            "UpdateLifecyclePolicy",
            Box::new(PassRoleAuthorized::new(
                crate::ops::update_lifecycle_policy::UpdateLifecyclePolicy::new(Arc::clone(
                    &engine,
                )),
            )) as Box<dyn ErasedOperation>,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            pass_role_authorized,
        }
    }
}

/// One modelled operation, served with the `iam:PassRole` authorization of its
/// `ExecutionRoleArn` done first.
///
/// Amazon Data Lifecycle Manager assumes the role the request names, so the front
/// door authorizes `iam:PassRole` for that ARN before the operation runs at all,
/// and answers `AccessDeniedException` when it is denied. That is ahead of the
/// member-constraint walk, so the denial reaches the caller even for an ARN the
/// `ExecutionRoleArn` `@pattern` would otherwise refuse as malformed — and,
/// because nothing has run yet, a denied create leaves no policy behind and a
/// denied update leaves the stored policy as it was.
///
/// Nothing else is re-implemented: a request this wrapper does not deny is handed
/// to the generated [`ErasedOperation::run`], which owns deserialize -> validate
/// -> handle -> serialize, telemetry and every modelled error.
struct PassRoleAuthorized<O> {
    operation: O,
}

impl<O> PassRoleAuthorized<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for PassRoleAuthorized<O>
where
    O: Operation + ErasedOperation,
    O::Input: NamesExecutionRole,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // Read the protocol out before the await: `&dyn ErasedServiceDefinition`
            // is not `Sync`, and this future is. The protocol itself is.
            let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
            if let Some(denied) = pass_role_denied::<O>(protocol, req) {
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

/// The `AccessDeniedException` response for `req`, when the `ExecutionRoleArn` it
/// names is one the caller may not pass. `None` when it may be passed, when the
/// request names none, and when the request cannot be read at all — a body the
/// codec rejects is not an authorization answer, and the framework's own walk
/// words it.
///
/// The request is read with the framework's own deserializer (the same one the
/// walk uses) and WITHOUT the constraint walk, because `iam:PassRole` is
/// authorized ahead of the member constraints: an ARN that does not denote a
/// role the caller may pass is denied rather than refused as malformed.
fn pass_role_denied<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<RtResponse>
where
    O::Input: NamesExecutionRole,
{
    let mut deserializer = protocol.deserialize_input(req, O::INPUT_SCHEMA).ok()?;
    let input = <O::Input as DeserializableStruct>::deserialize(&mut *deserializer).ok()?;
    let message = crate::state::pass_role_denial(input.execution_role_arn()?)?;
    protocol
        .serialize_error(
            &AccessDeniedException { message },
            &ACCESS_DENIED_SCHEMA,
            crate::state::ACCESS_DENIED_EXCEPTION,
            crate::state::ACCESS_DENIED_STATUS,
        )
        .ok()
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
        // `PassRoleAuthorized` for the two operations that take an
        // `ExecutionRoleArn`, so that ARN's `iam:PassRole` is authorized ahead of
        // the operation (and so ahead of the member-constraint walk), as the real
        // front door does it. Every other key resolves to `None` here exactly as
        // it does for an unedited service, leaving the generated table to serve it.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.pass_role_authorized.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording. To state a refusal the way the emulated service states it,
        // replace this body with a `match refusal { .. }` that returns
        // `Some(Refused { code, message })` for the cases you reword and `None`
        // for the rest. Replace BOTH the code and the message: a client branches
        // on the code, so a message without its matching code is worse than
        // leaving both alone.
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
