// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-kafka-kafka` service: how it claims requests and how it words the
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

use smithy_server_runtime::{http, ErasedOperation, Refusal, Refused, Service, Violation};
use std::sync::Arc;

/// The error code this service answers an input-constraint violation with.
///
/// MSK models NO `ValidationException` — the framework's default code for a
/// refusal — at all: `BadRequestException` (`@httpError 400`) is the client-input
/// error every operation of this service declares
/// (`smithy_query(query="operation",operation="ListConfigurations")` -> errors),
/// and the real service answers it for a value outside a modelled bound or an
/// absent required member. A client branches on the code, so the emulator must
/// name the one this service declares rather than a shape its model never
/// mentions.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// The message this service answers an input-constraint refusal with: Amazon
/// MSK's own fixed sentence for a parameter it will not accept
/// (`crate::state::BAD_REQUEST_MESSAGE`), which names neither the member nor the
/// bound it broke.
///
/// NOT the AWS constraint-validation envelope (`N validation error(s) detected:
/// Value '<v>' at '<member>' failed to satisfy constraint: …`): that envelope is
/// the wording of the input-validation layer in front of the service families
/// that use it, and MSK is not one of them — it validates a request itself and
/// answers one request-independent sentence for every parameter it refuses,
/// which is what the behavioural corpus records for a page size above the
/// modelled maximum. A client therefore sees the same text whatever member it
/// got wrong, and no part of its request comes back in the message; the member
/// at fault is reported in `BadRequestException$InvalidParameter` by the paths
/// that know it.
///
/// `violations` is taken but not read: the sentence is the same for every
/// violation class, and the parameter is still what the refusal is ABOUT, so a
/// refusal with no violations at all declines the override (`None`) and leaves
/// the framework's own wording, exactly as before.
fn refusal_message(violations: &[Violation]) -> Option<String> {
    (!violations.is_empty()).then(|| crate::state::BAD_REQUEST_MESSAGE.to_string())
}

/// The `com-amazonaws-kafka-kafka` emulated service: the generated [`ServiceDefinition`] plus
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
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        let _ = req;
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // This service answers such a refusal with the error code it models for a
        // bad request, [`BAD_REQUEST_EXCEPTION`], instead of the framework's
        // default `ValidationException`, which this model does not declare
        // anywhere, and with the one fixed sentence MSK words a refused
        // parameter in ([`refusal_message`]). The HTTP status stays with the
        // protocol (400), which is the status this error models.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure, and this service's 401/403 errors are raised
        // by the request-authentication path.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: BAD_REQUEST_EXCEPTION.to_string(),
                message: refusal_message(violations)?,
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
