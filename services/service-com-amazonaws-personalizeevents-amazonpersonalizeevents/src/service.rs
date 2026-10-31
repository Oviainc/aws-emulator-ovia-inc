// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-personalizeevents-amazonpersonalizeevents` service: how it claims requests and how it words the
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
//!   correctly. This service uses it: every request is checked for the `@required`
//!   members its body left out BEFORE the operation runs, because the generated
//!   deserializer stops at the first one and AWS states them all (see
//!   [`validation::AbsentMemberGate`]). A request that left none out reaches the
//!   generated handler untouched.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran; delegated to [`validation::refused`], which states the
//!   refusal in this service's own dialect of the AWS validation envelope.
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

mod validation;

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The first path segment of a URI, or `None` for the root path.
fn first_path_segment(path: &str) -> Option<&str> {
    let segment = path.trim_start_matches('/').split('/').next()?;
    (!segment.is_empty()).then_some(segment)
}

/// Whether `parts` addresses a path inside `path_segments` — the model-derived first
/// URI path segments this service declares (`crate::ROUTE_PATH_SEGMENTS`).
///
/// This mirrors the framework's own discriminator,
/// `emulator_server::can_handle_based_on_route_key_and_path`, which the generated
/// `can_handle` entry point uses. The framework helper takes a whole buffered
/// `http::Request`, which the `Service::can_handle` seam below does not receive, so the
/// rule is applied here over `Parts` instead of called. It is kept identical, including
/// both edge cases: an EMPTY list means the model published no usable discriminator and
/// is "no constraint", and the framework's own `ROOT_PATH_SEGMENT` sentinel is how a
/// service that must answer `POST /` claims the root path without claiming every path.
/// Matching is EXACT segment equality, deliberately never a prefix test.
fn claims_modeled_path(parts: &http::request::Parts, path_segments: &[&str]) -> bool {
    if path_segments.is_empty() {
        return true;
    }
    match first_path_segment(parts.uri.path()) {
        Some(segment) => path_segments.contains(&segment),
        None => path_segments.contains(&emulator_server::ROOT_PATH_SEGMENT),
    }
}

/// The `com-amazonaws-personalizeevents-amazonpersonalizeevents` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// States every `@required` member a `PutActionInteractions` request left out, before
    /// the handler runs; see [`validation::AbsentMemberGate`]. Held on the service because
    /// [`Service::override_operation`] hands back a borrow of `&self`.
    put_action_interactions_gate: validation::AbsentMemberGate<
        crate::ops::put_action_interactions::PutActionInteractions<Eng>,
    >,
    /// The same gate for `PutActions`.
    put_actions_gate: validation::AbsentMemberGate<crate::ops::put_actions::PutActions<Eng>>,
    /// The same gate for `PutEvents`.
    put_events_gate: validation::AbsentMemberGate<crate::ops::put_events::PutEvents<Eng>>,
    /// The same gate for `PutItems`.
    put_items_gate: validation::AbsentMemberGate<crate::ops::put_items::PutItems<Eng>>,
    /// The same gate for `PutUsers`.
    put_users_gate: validation::AbsentMemberGate<crate::ops::put_users::PutUsers<Eng>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            put_action_interactions_gate: validation::AbsentMemberGate::new(),
            put_actions_gate: validation::AbsentMemberGate::new(),
            put_events_gate: validation::AbsentMemberGate::new(),
            put_items_gate: validation::AbsentMemberGate::new(),
            put_users_gate: validation::AbsentMemberGate::new(),
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
        // THREE services sit behind ONE SigV4 signing name `personalize`: Amazon
        // Personalize (the control plane, endpoint `personalize`), Amazon Personalize
        // Runtime (endpoint `personalize-runtime`) and this one (endpoint
        // `personalize-events`). No Smithy trait expresses that, and the router's
        // routing key comes from the SigV4 credential scope first, so matching on the
        // signing name ALONE makes all three claim every Personalize-signed request —
        // which the router answers `500 AmbiguousService` for.
        //
        // So the claim is narrowed the way the framework narrows every other
        // signing-name collision: the signing name must match AND the request must
        // address this service's own modeled URI path space — `/events`, `/items`,
        // `/users`, `/actions`, `/action-interactions` (see
        // `crate::ROUTE_PATH_SEGMENTS`, generated from the model).
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) if key == self.entry.identity().signing_name() => {
                claims_modeled_path(parts, crate::ROUTE_PATH_SEGMENTS)
            }
            _ => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Every request this service routes is checked for the `@required` members its
        // body left out before the operation runs: absence is found while READING a
        // request and the generated deserializer stops at the first member it cannot
        // fill, so the refusal seam would see one absent member however many were
        // omitted. The gate states them all and hands a request that left none out to
        // the generated handler. Which operation a request names is the model's own
        // routing key, so no route table is restated here.
        match self.definition().protocol().routing_key(req).ok()? {
            "PutActionInteractions" => Some(&self.put_action_interactions_gate),
            "PutActions" => Some(&self.put_actions_gate),
            "PutEvents" => Some(&self.put_events_gate),
            "PutItems" => Some(&self.put_items_gate),
            "PutUsers" => Some(&self.put_users_gate),
            _ => None,
        }
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. This service states one dialect of the AWS validation envelope for
        // every constrained member of every operation — see [`validation::refused`],
        // which the pre-handler gate above answers with too, so one request cannot be
        // refused in two wordings.
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
