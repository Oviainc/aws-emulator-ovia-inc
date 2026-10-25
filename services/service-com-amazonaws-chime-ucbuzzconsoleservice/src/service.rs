// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-chime-ucbuzzconsoleservice` service: how it claims requests and how it words the
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

/// This service's own `aws.api#service$endpointPrefix`, the name that identifies it
/// among the six services that sign as `chime`.
///
/// It is the SHARED signing name itself: this is the original Amazon Chime endpoint, out
/// of which the five `*-chime` SDK endpoints were carved. That is why a `chime`-signed
/// request that names no endpoint at all is THIS service's — see `can_handle`.
///
/// Source: `smithy_query(query="service_info",
/// model_path="services/service-com-amazonaws-chime-ucbuzzconsoleservice/smithy-model/\
/// com-amazonaws-chime-ucbuzzconsoleservice.json")` — `awsService.endpointPrefix`.
const OWN_ENDPOINT_PREFIX: &str = "chime";

/// The `com-amazonaws-chime-ucbuzzconsoleservice` emulated service: the generated [`ServiceDefinition`] plus
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
        // This service signs as `chime` — a signing name it SHARES with the five Amazon
        // Chime SDK services in this workspace, each of which serves its OWN endpoint
        // (`identity-chime`, `media-pipelines-chime`, `meetings-chime`,
        // `messaging-chime`, `voice-chime`). The router resolves a request's routing key
        // from the SigV4 credential scope first, so all six see the key `chime`: claiming
        // on the signing name alone makes every one of them a claimant for every one of
        // the others' requests, and the router — which refuses rather than guesses —
        // answers `500 AmbiguousService` for ALL of it, including this service's own
        // `/accounts` traffic.
        //
        // So the claim is keyed on what identifies ONE service: the endpoint the request
        // addresses, and failing that this model's own URI path space. That rule, and the
        // readers for the path spaces the family shares, live once in `service-common`.
        //
        // Two shared spaces are this service's. The telephony space
        // (`/phone-numbers`, `/phone-number-orders`, `/phone-number-countries`,
        // `/settings`, `/search`) is bound IDENTICALLY by the Chime SDK Voice model, so
        // no path tells the two apart — but this endpoint's `endpointPrefix` IS the
        // shared signing name, so a request that names no endpoint names this one, and
        // the Voice endpoint claims that space only when the request addresses
        // `voice-chime`. Under `/meetings`, which the Chime SDK Meetings endpoint owns,
        // this model binds only `/meetings/{MeetingId}/dial-outs`.
        service_common::chime_family::claims_chime_family_request(
            parts,
            OWN_ENDPOINT_PREFIX,
            crate::ROUTE_PATH_SEGMENTS,
            |parts| {
                let path = parts.uri.path();
                service_common::chime_family::addresses_shared_telephony_space(path)
                    || service_common::chime_family::addresses_meeting_dial_outs(path)
            },
        )
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
        // Amazon Chime states a request that breaks a rule its model declares as
        // `BadRequestException` — "The input parameters don't match the service's
        // restrictions" — the HTTP 400 every operation in this service declares. The
        // framework's generic `ValidationException` is not a code a Chime client can
        // branch on, so both the code and the message are replaced here; the status
        // (400 for a refused value, 403 for an unauthenticated caller) stays with the
        // protocol, which owns it.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: "BadRequestException".to_string(),
                message: aws_smithy_schema_server::constraints::envelope(violations)
                    .unwrap_or_else(|| {
                        "The input parameters don't match the service's restrictions."
                            .to_string()
                    }),
            }),
            // 403 in this service's model is `ForbiddenException`.
            Refusal::Unauthenticated => Some(Refused {
                code: "ForbiddenException".to_string(),
                message: "The client is permanently forbidden from making the request."
                    .to_string(),
            }),
            // `Refusal` is `#[non_exhaustive]`: a refusal this service has no modelled
            // answer for keeps the framework's own wording.
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
