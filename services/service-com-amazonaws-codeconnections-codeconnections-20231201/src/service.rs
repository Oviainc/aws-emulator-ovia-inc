// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-codeconnections-codeconnections-20231201` service: how it claims requests and how it words the
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
//! - `override_operation` — which operation serves a request. EDITED HERE: the
//!   operation is resolved from the LAST segment of `x-amz-target`, so a client whose
//!   target prefix carries dots of its own (this service's absolute shape id, which is
//!   what the AWS SDK model uses) reaches the same generated operation as one using the
//!   bare service shape name. See the method for why.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. EDITED HERE: every input-constraint refusal is worded by
//!   `validation`, this service's one statement of its validation-message dialect.
//!   See the inline note on that method.
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

pub(crate) mod validation;

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-codeconnections-codeconnections-20231201` emulated service: the generated [`ServiceDefinition`] plus
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

    /// This service's model shape id, as the generated identity states it
    /// (`com.amazonaws.codeconnections#CodeConnections_20231201`), whose LOCAL name is
    /// the last segment of every `X-Amz-Target` prefix a client writes for it. Read off
    /// the model rather than written out, so it cannot drift from it.
    fn shape_id(&self) -> &str {
        self.entry.identity().shape_id()
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
        // segment before the FIRST dot — which, for this service's dotted target prefix,
        // is a namespace label (`com`) that names no service, so such a request would
        // reach nobody and answer `404 UnknownService`. The target still names THIS
        // service, read by the same rule the operation below is read by:
        // `service_common::rpc_target`, this emulator's one statement of the target rule
        // (the service is the last segment of the prefix, the operation the segment
        // after it). A target naming another service is still not claimed, signed or not.
        service_common::rpc_target::target_operation_of(&parts.headers, self.shape_id())
            .is_some()
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // This service's awsJson1_0 RPC target prefix is the service shape's FULL
        // id, namespace included (`com.amazonaws.codeconnections` +
        // `CodeConnections_20231201`), so an SDK sends
        // `x-amz-target: com.amazonaws.codeconnections.CodeConnections_20231201.<Operation>`.
        // The framework resolves an awsJson routing key as everything after the FIRST
        // dot of that header, which for a DOTTED prefix is
        // `amazonaws.codeconnections.CodeConnections_20231201.<Operation>` — a key no
        // modeled operation is wired under, so every such request would be refused as
        // an unknown operation before any handler ran.
        //
        // An RPC target names its operation in the segment after the LAST dot, whatever
        // the prefix in front of it, so that is the segment resolved here; it is the
        // same answer for the short prefix (`CodeConnections_20231201.<Operation>`), so
        // one rule serves both forms. A target whose final segment names no modeled
        // operation, or whose prefix names another service, resolves to `None` and is
        // left to the generated routing table, which renders the protocol's own routing
        // failure.
        //
        // The rule is protocol-general rather than service-specific, so it is stated
        // ONCE, in `service_common::rpc_target`, and read from there by every service in
        // this emulator whose published prefix carries dots. It belongs further down
        // still — in the framework's own `routing_key_from_x_amz_target` — but `vendor/`
        // is read-only here, so this seam is where the emulator can hold it.
        let operation = service_common::rpc_target::requested_operation(req, self.shape_id())?;
        self.entry.routing_table().get(operation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. The wording is NOT written here: it is
        // `validation::refused`, the one formatter every operation's boundary
        // validation reaches, so no operation can state a refusal its own way and
        // a handler enforcing a constraint the model cannot express
        // (`validation::connection_arn_refusal`) states it in the same envelope.
        // Both the code and the message come from there, since a client branches
        // on the code and a message without its matching code is worse than
        // leaving both alone. A refusal class that module has no wording for keeps
        // the framework's own sentence.
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
