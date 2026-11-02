// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-bedrockagentcore-amazonbedrockagentcore` service: how it claims requests and how it words the
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

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

use crate::ops::invoke_code_interpreter::InvokeCodeInterpreter;
use service_common::event_stream::EventStreamRoute;

/// The host-style endpoint this service is addressed by:
/// `bedrock-agentcore.<region>.amazonaws.com`.
///
/// Source: `smithy_query(query="service_info", model_path="services/\
/// service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/smithy-model/\
/// com-amazonaws-bedrockagentcore-amazonbedrockagentcore.json")` — `awsService.
/// endpointPrefix` is `bedrock-agentcore`, which here equals `sigv4Name`. Stated
/// anyway, and beside the claim it feeds, so the claim does not silently change meaning
/// if either name ever does.
const ENDPOINT_PREFIX: &str = "bedrock-agentcore";

/// The segments of `InvokeCodeInterpreter`'s modelled URI,
/// `/code-interpreters/{codeInterpreterIdentifier}/tools/invoke` (source:
/// `smithy_query(query="operation", operation="InvokeCodeInterpreter")`), stated beside
/// the one seam that reads them off a request line — the routing seam below, which runs
/// before any input is deserialized.
const CODE_INTERPRETERS_SEGMENT: &str = "code-interpreters";
/// See [`CODE_INTERPRETERS_SEGMENT`].
const TOOLS_SEGMENT: &str = "tools";
/// See [`CODE_INTERPRETERS_SEGMENT`].
const INVOKE_SEGMENT: &str = "invoke";

/// The `com-amazonaws-bedrockagentcore-amazonbedrockagentcore` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
///
/// Besides the generated entry it holds this service's one event-stream operation,
/// `InvokeCodeInterpreter`. It is NOT in the entry's routing table in a servable form:
/// the generated `ErasedOperation` shim of an operation whose output is a `@streaming`
/// union declines the request (it cannot drive `serialize_as_event_stream<P>` through
/// an object-safe `run`), so it is held here behind an [`EventStreamRoute`] and routed
/// by [`EmulatedService::override_operation`] — which the router consults FIRST, before
/// the generated table.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    invoke_code_interpreter: EventStreamRoute<InvokeCodeInterpreter<Eng>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            invoke_code_interpreter: EventStreamRoute(InvokeCodeInterpreter::new(engine)),
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
        // This service signs as `bedrock-agentcore` — a signing name it SHARES with the
        // AgentCore CONTROL plane
        // (`services/…-bedrockagentcorecontrol-amazonbedrockagentcorecontrol`), whose
        // endpoint is `bedrock-agentcore-control` while this one's is
        // `bedrock-agentcore`. The router resolves a request's routing key from its SigV4
        // credential scope first, so BOTH services see the key `bedrock-agentcore`:
        // claiming on the signing name alone makes each claim the other's traffic, and
        // the router — which refuses rather than guesses — answers `500 AmbiguousService`
        // for every AgentCore request, a code no AWS model carries.
        //
        // Nor can a path segment separate them: the two models publish the SAME first
        // segment for nine resource families (`/memories/…`, `/runtimes/…`,
        // `/browsers/…`, `/code-interpreters/…`, `/identities/…`,
        // `/capacity-providers/…`, `/harnesses/…`, `/payments/…`,
        // `/browser-profiles/…`), and they collide at deeper indexes too.
        //
        // What separates them is what AWS itself dispatches on: the endpoint host, then
        // the `restJson1` route. So this service claims a request whose routing key names
        // THIS endpoint and whose method + URI its OWN model binds a route for, by the
        // one rule `service-common` states for every service under a shared signing name.
        // The two models bind no identical method + URI pattern, so exactly one of them
        // claims every AgentCore request.
        service_common::claims_modeled_route(
            parts,
            &[self.entry.identity().signing_name(), ENDPOINT_PREFIX],
            self.definition(),
        )
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // The one event-stream operation the generated routing table cannot serve: its
        // output member targets a `@streaming` union, so the generated shim declines
        // the request and this seam \u2014 which the router consults FIRST \u2014 hands it to the
        // hand-authored operation instead.
        //
        // Routed by the method and the URI the model binds it to, `POST
        // /code-interpreters/{codeInterpreterIdentifier}/tools/invoke`: the identifier
        // is a label, so the match is on the method and on the two segments after it.
        // Every other request, including this service's other `@streaming` operations,
        // still resolves through the generated table and keeps answering the
        // framework's "modeled but not served" response rather than being half-served
        // here.
        if req.method() != http::Method::POST.as_str() {
            return None;
        }
        let path = service_common::request_path(req.uri());
        let addressed = service_common::nth_path_segment(path, 0) == Some(CODE_INTERPRETERS_SEGMENT)
            && service_common::nth_path_segment(path, 2) == Some(TOOLS_SEGMENT)
            && service_common::nth_path_segment(path, 3) == Some(INVOKE_SEGMENT)
            && service_common::nth_path_segment(path, 4).is_none();
        addressed.then_some(&self.invoke_code_interpreter as &dyn ErasedOperation)
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
