// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-chimesdkvoice-chimesdktelephonyservice` service: how it claims requests and how it words the
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
    http, ErasedOperation, Refusal, Refused, Service, Violation,
};
use std::sync::Arc;

/// This service's own `aws.api#service$endpointPrefix`, the name that identifies it
/// among the six services that sign as `chime`.
///
/// Source: `smithy_query(query="service_info",
/// model_path="services/service-com-amazonaws-chimesdkvoice-chimesdktelephonyservice/\
/// smithy-model/com-amazonaws-chimesdkvoice-chimesdktelephonyservice.json")` —
/// `awsService.endpointPrefix`.
const OWN_ENDPOINT_PREFIX: &str = "voice-chime";

/// The `com-amazonaws-chimesdkvoice-chimesdktelephonyservice` emulated service: the generated [`ServiceDefinition`] plus
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
        // This service signs as `chime` — a signing name it SHARES with the five other
        // Amazon Chime services in this workspace, each of which serves its OWN endpoint
        // (`chime`, `identity-chime`, `media-pipelines-chime`, `meetings-chime`,
        // `messaging-chime`). The router resolves a request's routing key from the SigV4
        // credential scope first, so all six see the key `chime`: claiming on the signing
        // name alone makes every one of them a claimant for every one of the others'
        // requests, and the router — which refuses rather than guesses — answers
        // `500 AmbiguousService` for ALL of it, including this service's own
        // `/voice-connectors` traffic.
        //
        // So the claim is keyed on what identifies ONE service: the endpoint the request
        // addresses, and failing that this model's own URI path space. That rule, and the
        // readers for the path spaces the family shares, live once in `service-common`.
        //
        // This model claims NONE of the shared spaces on a request that names no
        // endpoint: its telephony URIs (`/phone-numbers`, `/phone-number-orders`,
        // `/phone-number-countries`, `/settings`, `/search`) are the legacy Amazon Chime
        // endpoint's too, operation for operation, and `chime` — the key such a request
        // resolves to — is that endpoint's `endpointPrefix`. A caller reaches THIS
        // endpoint's half of that space by addressing `voice-chime`, which the first
        // conjunct answers. (`/tags` is shared by five of the six and is resolved by
        // endpoint prefix only — see `chime_family`.)
        service_common::chime_family::claims_chime_family_request(
            parts,
            OWN_ENDPOINT_PREFIX,
            crate::ROUTE_PATH_SEGMENTS,
            |_| false,
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
        // Amazon Chime SDK Voice states a request that breaks a declared input rule
        // as `BadRequestException` ("The input parameters don't match the service's
        // restrictions.", HTTP 400) — the error EVERY operation in this service's
        // model declares for exactly that case. The framework's default code here is
        // the generic `ValidationException`, which no operation of this service
        // declares, so a client branching on the modelled code would never match it.
        // The message keeps the framework's per-violation clause, which names the
        // member and the constraint it failed, so the caller can fix the request.
        //
        // The status stays with the protocol (400 for a declared-rule violation),
        // which is `BadRequestException`'s own `httpError`, so code and status agree.
        match refusal {
            Refusal::Invalid(violations) => {
                let message = if violations.is_empty() {
                    "The input parameters don't match the service's restrictions.".to_string()
                } else {
                    violations
                        .iter()
                        .map(Violation::message)
                        .collect::<Vec<String>>()
                        .join("; ")
                };
                Some(Refused {
                    code: "BadRequestException".to_string(),
                    message,
                })
            }
            // An authentication refusal is a 403 the framework already words; this
            // service's modelled `UnauthorizedClientException` is a 401, so renaming
            // it here would answer a 401 code with a 403 status.
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
