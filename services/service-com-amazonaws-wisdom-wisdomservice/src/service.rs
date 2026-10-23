// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-wisdom-wisdomservice` service: how it claims requests and how it words the
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
/// It is NOT a modelled error — no operation in this model (nor in the Q Connect view of
/// the same endpoint) declares it (`smithy_query(query="operation",
/// operation="CreateAssistant")`: `AccessDeniedException`, `ConflictException`,
/// `ServiceQuotaExceededException`, `ValidationException`) — because the refusal happens
/// where the request body is read into the input shape, before the operation's own
/// validation: a `type` outside `AssistantType`'s value set, a `knowledgeBaseType` outside
/// `KnowledgeBaseType`'s, and so on are all answered with this one code, while a
/// constraint this service validates itself (an absent required member, a `@length` or
/// `@pattern` violation) keeps the modelled `ValidationException`.
///
/// The Q Connect view of this SAME endpoint — which is the view the shared `wisdom` key
/// is served from (`services/service-com-amazonaws-qconnect-wisdomservice/src/service.rs`)
/// — words this class identically, from its own copy of this constant and of
/// `override_refusal`: the two are one endpoint, so a client must not get a different code
/// for the same input depending on which key it addressed. Change one and change the other.
const ENUM_VALUE_REFUSAL_CODE: &str = "BadRequestException";

/// The key THIS service answers under, which is NOT the `wisdom` key its model's
/// `aws.auth#sigv4` name resolves to.
///
/// This model is Amazon Connect Wisdom: `smithy_query(query="service_info")` gives it
/// sdkId `Wisdom`, sigv4Name `wisdom`, arnNamespace `wisdom`, service shape
/// `WisdomService` version 2020-10-19, protocol `restJson1`, 41 operations. The repo
/// carries a SECOND model of that same endpoint,
/// `services/service-com-amazonaws-qconnect-wisdomservice` (sdkId `QConnect` — Amazon Q
/// in Connect, the endpoint's current API), whose `service_info` differs ONLY in sdkId:
/// the same service shape and version, the same protocol, the same signing name, the same
/// endpoint templates `https://wisdom.{Region}.{dnsSuffix}`, and 94 operations that
/// INCLUDE all 41 of this model's at the identical method+URI routes. restJson1 carries
/// no `X-Amz-Target`, so a request is dispatched by method+URI — and those routes are the
/// same here as there, which means no signal in a request can tell the two models apart.
/// The `wisdom` key therefore has to be served by exactly ONE of them, or the router
/// (which requires exactly one claimant) answers `500 AmbiguousService` for every request
/// to the endpoint, a code that exists in no AWS model.
///
/// The QConnect crate serves it, because it is the endpoint's current API and the
/// superset: a client signing `wisdom` reaches every operation EITHER model declares,
/// which is also what AWS does — one service answers both APIs on `wisdom.{region}`.
/// Claiming `wisdom` here as well would reinstate the ambiguity.
///
/// This service therefore answers under its own REGISTRATION identity: this model's shape
/// id `com.amazonaws.wisdom#WisdomService` in the key form the project names its service
/// crates by (`service-com-amazonaws-wisdom-wisdomservice`). It is unique per registered
/// service by construction and is no AWS signing name, endpoint prefix or sdkId — which
/// is exactly why it cannot collide with the shared endpoint identity. The legacy model
/// stays mounted, reachable and exercised by its own suite without ever contending for
/// `wisdom`.
///
/// Read this key as ADMINISTRATIVE, not as a wire identity: no AWS SDK or CLI ever signs
/// or addresses `com-amazonaws-wisdom-wisdomservice`, because AWS publishes exactly one
/// identity for this endpoint and the QConnect model above serves it. Only a caller that
/// names this registered model deliberately — this crate's own suite, or an operator
/// probing one mounted service — reaches these handlers, and nothing is lost by that: all
/// 41 routes of this model are bound in the QConnect crate, which answers the endpoint's
/// real traffic.
const OWN_REGISTRATION_KEY: &str = "com-amazonaws-wisdom-wisdomservice";

/// Whether the request these `parts` came from resolves to [`OWN_REGISTRATION_KEY`] AND a
/// first URI path segment this model publishes (`crate::ROUTE_PATH_SEGMENTS`).
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable owner
/// of that rule, and it is CALLED here rather than re-implemented, with this service's
/// own key in place of the signing name. It takes a whole `http::Request` while
/// [`Service::can_handle`] is handed only the request's parts, so the two things it reads
/// — the URI and the headers — are re-presented to it as a bodiless request. It reads
/// neither the body nor the method, so nothing it decides on is missing.
/// (`services/common`'s `claims_request` states the same rule over `Parts`; it is not
/// used here only because depending on it edits `Cargo.lock`, which this repository's
/// serialized squash merge refuses whenever another change lands against it — migrating
/// these two crates onto it is a one-line follow-up.)
fn claims_own_registration_key(parts: &http::request::Parts) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    emulator_server::can_handle_based_on_route_key_and_path(
        &request,
        OWN_REGISTRATION_KEY,
        crate::ROUTE_PATH_SEGMENTS,
    )
}

/// The `com-amazonaws-wisdom-wisdomservice` emulated service: the generated [`ServiceDefinition`] plus
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
        // Delegated to the framework's own predicate — see
        // [`claims_own_registration_key`]: this model is the Amazon Connect Wisdom view of
        // the endpoint Amazon Q in Connect now serves, publishing the very same wire
        // identity (signing name, endpoint, service shape, version, protocol) and a
        // strict subset of its routes, so the generated `wisdom` route key cannot tell
        // the two apart. It is claimed by the QConnect crate — the endpoint's current API
        // and the superset — and this service answers under its own registration key
        // instead, so exactly one service claims every request.
        claims_own_registration_key(parts)
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
        // The ONE class this service states differently is a value outside a modelled
        // enumeration's value set: that is refused where the request body is read into
        // the operation's input shape, and this endpoint answers it 400
        // `BadRequestException` — a code neither this model nor the Q Connect view
        // declares as an operation error, because it is the deserialization boundary's,
        // not a modelled failure. Only the CODE is chosen here; the message stays the
        // framework's own `Value '…' at '…' failed to satisfy constraint: Member must
        // satisfy enum value set: […]` sentence, built from the request.
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
