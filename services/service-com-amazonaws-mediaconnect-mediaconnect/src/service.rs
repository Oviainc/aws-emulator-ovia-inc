// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-mediaconnect-mediaconnect` service: how it claims requests and how it words the
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
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The modelled member name a violation path ends in: `sources.1.member.protocol` ->
/// `Protocol`.
///
/// A violation path spells each member in lowerCamel — "a member names itself in
/// lowerCamel. That casing belongs to the path, not to the model"
/// (`aws_smithy_schema_server::constraints::path::member`) — so the modelled member name is
/// the last segment with its initial capitalised, the exact inverse of that spelling for
/// the names this model uses. `None` for a path that ends in no member name at all (an
/// empty path, or one ending in the collection step), where there is nothing to name.
///
/// (`service-com-amazonaws-servicequotas-servicequotasv20190624`'s `service.rs` recovers
/// the same thing for ITS service by walking that operation's schema members instead; the
/// two stay per-service until there is a shared home for one of them, because neither
/// crate may add a module to the other.)
///
/// TOTAL for every `path`: `rsplit` always yields a first item, the filter rejects the
/// cases with nothing to name, and the re-casing walks `chars()` rather than indexing or
/// slicing bytes — so no byte offset, `unwrap`/`expect`, parse or arithmetic is reachable,
/// whatever the path holds (a caller-chosen map key is one of its segments).
fn modelled_member_name(path: &str) -> Option<String> {
    let segment = path.rsplit('.').next().filter(|segment| {
        !segment.is_empty()
            && segment != &"member"
            && segment.starts_with(|c: char| c.is_ascii_alphabetic())
    })?;
    let mut name = String::with_capacity(segment.len());
    for (index, c) in segment.chars().enumerate() {
        if index == 0 {
            name.extend(c.to_uppercase());
        } else {
            name.push(c);
        }
    }
    Some(name)
}

/// How this service states ONE violation the framework's constraint walk found, as the
/// detail of its `Validation failed:` envelope.
///
/// A value outside a modelled value set is stated the way MediaConnect states it — `Invalid
/// <Member>`, naming the member the request carried the value in and nothing else, which is
/// what independent recordings of this service show for an enum-typed member. The member
/// name comes from the violation's own path, so it is derived from the request: a value
/// outside `Protocol`'s set reads `Invalid Protocol` wherever in the request it sat.
///
/// Every other violation kind keeps the framework's own sentence, which names the member,
/// the value and the rule it broke: MediaConnect's own wording for those is not known, and
/// a sentence that states the rule is more use to a caller than a guessed one.
///
/// TOTAL for every violation: `modelled_member_name` is total, `Violation::message` is the
/// framework's own rendering (which elides the echoed value), and the only other step is a
/// `format!`.
fn violation_detail(violation: &Violation) -> String {
    match &violation.kind {
        ViolationKind::EnumValue(_) => match modelled_member_name(&violation.path) {
            Some(member) => format!("Invalid {member}"),
            None => violation.message(),
        },
        _ => violation.message(),
    }
}

/// The `com-amazonaws-mediaconnect-mediaconnect` emulated service: the generated [`ServiceDefinition`] plus
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
        match refusal {
            // MediaConnect declares no `ValidationException`. Its client-input error is
            // `BadRequestException` (HTTP 400) — "thrown if the request contains a
            // semantic error. The precise meaning depends on the API, and is documented
            // in the error message" — and every operation of this service lists it. So a
            // request the framework refuses before any handler runs (a required member
            // absent, a value outside a declared bound, an ARN failing its modelled
            // pattern) is worded with the code a MediaConnect client branches on, and the
            // message states each violation the framework found.
            //
            // The message goes through `crate::state::validation_failed_message`, the one
            // envelope this service words a refused request in: a boundary refusal and a
            // refusal a handler raises itself must not reach a client in two different
            // forms, because the `Validation failed:` prefix is what a client and a
            // log-based alert tell a validation refusal from a semantic one by. Each
            // violation inside that envelope is stated by `violation_detail`.
            Refusal::Invalid(violations) => {
                let detail = violations
                    .iter()
                    .map(violation_detail)
                    .collect::<Vec<String>>()
                    .join("; ");
                let detail = if detail.is_empty() {
                    "The request contains a semantic error.".to_string()
                } else {
                    detail
                };
                Some(Refused {
                    code: "BadRequestException".to_string(),
                    message: crate::state::validation_failed_message(&detail),
                })
            }
            // An unauthenticated caller keeps the framework's 403 wording.
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
