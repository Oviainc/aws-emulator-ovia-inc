// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-mediaconvert-mediaconvert` service: how it claims requests and how it words the
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
//! Two of those seams are taken here. `override_refusal` states a client-side refusal as
//! one of this service's own modelled errors (below), and `override_operation` serves
//! every routable request through [`error_envelope::ErrorEnvelopeOperation`], which
//! completes MediaConvert's error envelope — `errorType`, `httpStatus`, `accountId`,
//! `requestId`, `operation`, `settingsValidationErrorsJsonBlob` — on the error body the
//! framework rendered, for the one reason named in [`error_envelope`]: those members
//! belong to the envelope rather than to any modelled error shape, and one of them is the
//! operation being served, which only this layer knows. Routing, status, `__type` and
//! `message` all stay with the framework.

pub mod error_envelope;

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-mediaconvert-mediaconvert` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The envelope wrapper every routable request is served through. Held in a field,
    /// built once, because `override_operation` hands back a reference to it.
    envelope: error_envelope::ErrorEnvelopeOperation,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            envelope: error_envelope::ErrorEnvelopeOperation,
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
        // Every request the generated routing table can serve is served through the
        // envelope wrapper, which delegates to that same table entry and then completes
        // the error envelope of whatever it answered (see [`error_envelope`]). The
        // routing stays the model's: a request the table cannot serve is declined here,
        // so the framework answers it as the unroutable request it is.
        error_envelope::routes(&self.entry, req).then_some(&self.envelope as _)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // MediaConvert states every client-side refusal through one of its own modelled
        // errors, so the code a client branches on is a code the model declares — not
        // the framework's generic `ValidationException`, which this service never
        // models. Both the code and the message are replaced together.
        //
        // * `BadRequestException` (400): "The service can't process your request because
        //   of a problem in the request. Please check your request form and syntax."
        // * `ForbiddenException` (403): "You don't have permissions for this action with
        //   the credentials you sent."
        //
        // The status stays with the protocol, which already answers 400 for an invalid
        // request and 403 for an unauthenticated one.
        match refusal {
            Refusal::Invalid(violations) => {
                let detail = violations
                    .iter()
                    .map(smithy_server_runtime::Violation::message)
                    .collect::<Vec<_>>()
                    .join("; ");
                Some(Refused {
                    code: "BadRequestException".to_string(),
                    message: if detail.is_empty() {
                        "The service can't process your request because of a problem in \
                         the request. Please check your request form and syntax."
                            .to_string()
                    } else {
                        detail
                    },
                })
            }
            Refusal::Unauthenticated => Some(Refused {
                code: "ForbiddenException".to_string(),
                message: "You don't have permissions for this action with the credentials \
                          you sent."
                    .to_string(),
            }),
            // `Refusal` is `#[non_exhaustive]`: a kind added later keeps the framework's
            // own wording until this service decides how to state it.
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
