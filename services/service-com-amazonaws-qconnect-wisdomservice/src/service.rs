// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-qconnect-wisdomservice` service: how it claims requests and how it words the
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

use smithy_server_runtime::{http, ErasedOperation, Refusal, Refused, Service, ViolationKind};
use std::sync::Arc;

/// The error code this endpoint answers a value outside a modelled enumeration's value
/// set with: `BadRequestException`, HTTP 400.
///
/// It is NOT a modelled error — no operation in this model (nor in the legacy Amazon
/// Connect Wisdom view of the same endpoint) declares it
/// (`smithy_query(query="operation", operation="CreateAssistant")`:
/// `AccessDeniedException`, `ConflictException`, `ServiceQuotaExceededException`,
/// `ValidationException`) — because the refusal happens where the request body is read
/// into the input shape, before the operation's own validation: a `type` outside
/// `AssistantType`'s value set, a `knowledgeBaseType` outside `KnowledgeBaseType`'s, and
/// so on are all answered with this one code, while a constraint this service validates
/// itself (an absent required member, a `@length` or `@pattern` violation) keeps the
/// modelled `ValidationException`.
///
/// The legacy Amazon Connect Wisdom view of this SAME endpoint
/// (`services/service-com-amazonaws-wisdom-wisdomservice/src/service.rs`, reachable under
/// its own registration key) words this class identically, from its own copy of this
/// constant and of `override_refusal`: the two are one endpoint, so a client must not get
/// a different code for the same input depending on which key it addressed. Change one and
/// change the other.
const ENUM_VALUE_REFUSAL_CODE: &str = "BadRequestException";

/// This model's OWN sdkId-derived key (`sdkId: QConnect`,
/// `smithy_query(query="service_info")`), beside the shared endpoint identity
/// [`crate::ROUTE_KEY`] names.
///
/// A client addressed at Amazon Q in Connect by its product name — a credential scope or
/// an endpoint host saying `qconnect` — reaches this service too, not only one signing
/// `wisdom`.
const OWN_SDK_ID_KEY: &str = "qconnect";

/// Whether the request these `parts` came from addresses THIS service: its route key is
/// the shared `wisdom` endpoint identity ([`crate::ROUTE_KEY`]) or this model's own
/// [`OWN_SDK_ID_KEY`], AND its first URI path segment is one this model publishes
/// (`crate::ROUTE_PATH_SEGMENTS`).
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable owner
/// of that rule, and it is CALLED here per key rather than re-implemented. It takes a
/// whole `http::Request` while [`Service::can_handle`] is handed only the request's
/// parts, so the two things it reads — the URI and the headers — are re-presented to it
/// as a bodiless request. It reads neither the body nor the method, so nothing it decides
/// on is missing. (`services/common`'s `claims_request` states the same rule over
/// `Parts`; it is not used here only because depending on it edits `Cargo.lock`, which
/// this repository's serialized squash merge refuses whenever another change lands
/// against it — migrating these two crates onto it is a one-line follow-up.)
fn claims_endpoint_traffic(parts: &http::request::Parts) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    [crate::ROUTE_KEY, OWN_SDK_ID_KEY].iter().any(|key| {
        emulator_server::can_handle_based_on_route_key_and_path(
            &request,
            key,
            crate::ROUTE_PATH_SEGMENTS,
        )
    })
}

/// The `com-amazonaws-qconnect-wisdomservice` emulated service: the generated [`ServiceDefinition`] plus
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
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This model is Amazon Q in Connect, and the endpoint identity it publishes is
        // NOT its own name: `smithy_query(query="service_info")` gives it sdkId
        // `QConnect` but sigv4Name/arnNamespace `wisdom` and the endpoint templates
        // `https://wisdom.{Region}.{dnsSuffix}` — AWS serves Q in Connect on Amazon
        // Connect Wisdom's endpoint and signing name. The repo carries that endpoint's
        // other model too, `services/service-com-amazonaws-wisdom-wisdomservice` (sdkId
        // `Wisdom`, the same service shape `WisdomService` 2020-10-19, the same
        // `restJson1` protocol, the same signing name), whose 41 operations are a strict
        // SUBSET of this model's 94 AT THE IDENTICAL METHOD+URI ROUTES. No signal in a
        // request can tell the two apart, and restJson1 carries no `X-Amz-Target`, so the
        // `wisdom` key has to be served by exactly ONE of them or the router refuses
        // every request to the endpoint as `500 AmbiguousService` — a code that exists in
        // no AWS model.
        //
        // It is served HERE: this model is the endpoint's current API and the superset,
        // so a client signing `wisdom` loses no operation (the legacy model's 41 routes
        // are all bound here), while serving it from the subset model would leave this
        // model's other 53 routes — every AI agent, AI prompt, AI guardrail, message
        // template, content association and session message operation — answerable by
        // nobody. The legacy crate answers under its own registration key instead and
        // contends for nothing. See `claims_endpoint_traffic`.
        claims_endpoint_traffic(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        let _ = req;
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it; `None` keeps the framework's default (400 `ValidationException`,
        // which is also what this service's model declares for a constraint it
        // validates itself, so every other class is left alone).
        //
        // The ONE class this endpoint states differently is a value outside a modelled
        // enumeration's value set: that is refused where the request body is read into
        // the operation's input shape, and this endpoint answers it 400
        // `BadRequestException` — a code neither this model nor the legacy Wisdom view
        // declares as an operation error, because it is the deserialization boundary's,
        // not a modelled failure. Only the CODE is chosen here; the message stays the
        // framework's own `Value '…' at '…' failed to satisfy constraint: Member must
        // satisfy enum value set: […]` sentence, built from the request. The legacy view
        // of this same endpoint words the same class the same way.
        //
        // `any`, not `all` or "the first": a request that ALSO breaks a constraint this
        // service validates itself could not be read into the input shape either, so the
        // boundary's refusal is the one the caller gets, and the message still states
        // every clause the walk found. That keeps the answer independent of the order the
        // members happen to be walked in.
        match refusal {
            Refusal::Invalid(violations)
                if violations
                    .iter()
                    .any(|violation| matches!(violation.kind, ViolationKind::EnumValue(_))) =>
            {
                Some(Refused {
                    code: ENUM_VALUE_REFUSAL_CODE.to_string(),
                    message: aws_smithy_schema_server::envelope(violations)
                        .unwrap_or_else(|| "invalid request".to_string()),
                })
            }
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
