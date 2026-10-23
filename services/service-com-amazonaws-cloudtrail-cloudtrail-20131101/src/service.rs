// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-cloudtrail-cloudtrail-20131101` service: how it claims requests and how it words the
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
//! - `can_handle` — which requests this service claims. EDITED HERE: the framework's
//!   default (the request's resolved route key equals this service's signing name),
//!   plus a request whose `X-Amz-Target` names this service, which the framework's
//!   routing-key fallback misreads for this service's dotted target prefix.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself. THIS service
//!   does: its `X-Amz-Target` prefix is the fully qualified Coral service name
//!   (`com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101`), so the operation is
//!   the segment after the LAST dot of the target rather than after the first.
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

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The target rule in isolation: which operation an `X-Amz-Target` names, over the
/// prefix forms a client can write. Lives beside this file rather than inline so the
/// table can grow without pushing the seams it tests off the screen; the wire behaviour
/// it produces is asserted end-to-end in `tests/all/target_prefix_dispatch.rs`.
#[cfg(test)]
mod rpc_target;

/// The header an `awsJson` client names the operation in.
const TARGET_HEADER: &str = "x-amz-target";

/// The operation the `X-Amz-Target` value `target` names, for the service whose shape's
/// local name is `service_shape_name`; `None` when the target names another service, or
/// names no operation at all.
///
/// An `awsJson` target is `<targetPrefix>.<OperationName>`, and a target prefix MAY
/// itself contain dots: this service's is the fully qualified Coral service name
/// (`com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101`), which is what every AWS
/// SDK sends. So the operation is the segment after the LAST dot, and the service it
/// belongs to is the last dotted segment of what precedes it — the service shape's local
/// name, with or without a leading namespace. The framework's own reader
/// (`aws_smithy_schema_server::rpc::routing_key_from_x_amz_target`) splits at the FIRST
/// dot, which is right only for a single-segment prefix, so this service resolves its own
/// targets through the `override_operation` seam.
fn operation_from_target<'target>(
    target: &'target str,
    service_shape_name: &str,
) -> Option<&'target str> {
    let (prefix, operation) = target.trim().rsplit_once('.')?;
    let named_service = prefix.split('.').next_back()?;
    (named_service == service_shape_name && !operation.is_empty()).then_some(operation)
}

/// The `com-amazonaws-cloudtrail-cloudtrail-20131101` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
        }
    }

    /// This service's shape name — the local name of the model-derived shape id
    /// (`com.amazonaws.cloudtrail#CloudTrail_20131101` -> `CloudTrail_20131101`), which
    /// is the last segment of every `X-Amz-Target` prefix a client writes for it.
    /// Read off the generated identity rather than written out, so it cannot drift from
    /// the model.
    fn shape_name(&self) -> &str {
        let shape_id = self.entry.identity().shape_id();
        shape_id.split('#').next_back().unwrap_or(shape_id)
    }

    /// The operation an `X-Amz-Target` names for THIS service, by the rule in
    /// [`operation_from_target`].
    fn target_operation<'target>(&self, target: &'target str) -> Option<&'target str> {
        operation_from_target(target, self.shape_name())
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
        if smithy_server_runtime::resolve_route_key(parts)
            .is_some_and(|key| key == self.entry.identity().signing_name())
        {
            return true;
        }
        // A request that carries no SigV4 credential scope is routed by its
        // `X-Amz-Target` before its `Host`, and the framework reads that signal as the
        // segment before the FIRST dot — which, for this service's fully qualified
        // target prefix, is a namespace label (`com`) that names no service. The target
        // still names THIS service, so read it by the same rule the operation is read
        // by rather than leaving such a request unclaimed.
        parts
            .headers
            .get(TARGET_HEADER)
            .and_then(|target| target.to_str().ok())
            .and_then(|target| self.target_operation(target))
            .is_some()
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        //
        // This service's `X-Amz-Target` prefix is its fully qualified Coral service
        // name, so the prefix itself contains dots and the framework's first-dot split
        // leaves the whole `<namespace>.<Service>.<Operation>` tail as the routing key,
        // which matches no modelled operation and refuses every call with 501
        // `NotImplemented`. Resolving the operation as the segment after the LAST dot
        // dispatches the target an SDK actually sends; a target naming another service,
        // or an operation this model does not bind, still falls through to the
        // framework's own routing (and its routing error).
        let target = req.headers().get(TARGET_HEADER)?;
        let operation = self.target_operation(target)?;
        self.entry.routing_table().get(operation)
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
