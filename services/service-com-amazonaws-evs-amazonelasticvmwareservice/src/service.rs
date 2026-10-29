// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-evs-amazonelasticvmwareservice` service: how it claims requests and how it words the
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
//! Two of those seams are taken here, and they are two halves of one answer.
//! `override_refusal` states a framework-level refusal as this service's
//! `ValidationException`, and `override_operation` serves every routable request through
//! [`validation_envelope::ModelledRefusal`], which completes that body with the modelled
//! members a refusal rendering cannot carry — `reason`, which the model marks
//! `@required`, and the `fieldList` entries the same renderer built. Routing, status,
//! `__type` and `message` all stay with the framework.

pub mod validation_envelope;

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-evs-amazonelasticvmwareservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The wrapper every routable request is served through, which completes the
    /// modelled members of a refusal body. Held in a field, built once, because
    /// `override_operation` hands back a reference to it.
    modelled_refusal: validation_envelope::ModelledRefusal,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            modelled_refusal: validation_envelope::ModelledRefusal,
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
        // wrapper below, which delegates to that same table entry and then re-states a
        // `ValidationException` the framework rendered for it as the whole modelled shape
        // (see [`validation_envelope`]). The routing stays the model's: a request the
        // table cannot serve is declined here, so the framework answers it as the
        // unroutable request it is.
        validation_envelope::routes(&self.entry, req).then_some(&self.modelled_refusal as _)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // This service states such a refusal as the AWS validation envelope — the
        // clause naming the member path and the requirement it failed, without
        // quoting the rejected value back — which is a different rule from the
        // framework's default wording; `crate::state::constraint_validation_error`
        // is the one renderer behind every operation of this service and owns that
        // rule. The STATUS and the CODE are already the framework's and are what the
        // model declares (`ValidationException`, `@error(client)`, `@httpError(400)`),
        // so the code is restated unchanged alongside the reworded message: a message
        // without its matching code is worse than leaving both alone.
        //
        // That renderer states the WHOLE modelled shape, of which only the message fits
        // through this seam, so the refusal is recorded for the request being served and
        // [`validation_envelope::ModelledRefusal`] answers it with every member the model
        // declares: `reason` is `@required`, so a body without it is not the error the
        // model declares.
        //
        // An authentication refusal is left to the framework: it is not a
        // modelled-input failure and this service does not restate it.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        let stated = crate::state::constraint_validation_error(violations)?;
        validation_envelope::note_worded_refusal(&stated);
        Some(Refused {
            code: "ValidationException".to_string(),
            message: stated.message,
        })
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
