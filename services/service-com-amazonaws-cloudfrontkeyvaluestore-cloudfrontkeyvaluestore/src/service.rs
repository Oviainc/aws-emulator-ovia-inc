// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-cloudfrontkeyvaluestore-cloudfrontkeyvaluestore` service: how it claims requests and how it words the
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

/// The `com-amazonaws-cloudfrontkeyvaluestore-cloudfrontkeyvaluestore` emulated service: the generated [`ServiceDefinition`] plus
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
        // modeled operation. One such request is NOT an unknown operation: every
        // mutating operation here binds a `@required` `If-Match` header, and the
        // REST router treats a `@required` header/query binding as a routing
        // discriminator — so a request to a mutating route that omits `If-Match`
        // matches no route at all and would otherwise be answered as if the
        // operation did not exist. An absent required member is a client error, so
        // state it as the 400 `ValidationException` the framework answers for every
        // other absent required member instead of a routing failure.
        //
        // The underlying rule is router-wide: EVERY service with a `@required` header
        // binding needs this. The right home is `route_lookup` in
        // `vendor/smithy-schema-server/src/rest_xml.rs`, which should route a request
        // whose only unsatisfied discriminator is a required binding and let constraint
        // validation answer the 400. That tree is read-only from here, so this stays a
        // narrow, documented stopgap for this service's three mutating routes.
        if missing_if_match_on_mutating_route(req) {
            return Some(smithy_server_runtime::protocol_error_response(
                smithy_server_runtime::PreDispatchProtocol::Json {
                    content_type: "application/json",
                    errortype_header: true,
                },
                aws_smithy_schema_server::ProtocolError::Invalid(vec![
                    smithy_server_runtime::Violation {
                        path: "ifMatch".to_string(),
                        kind: smithy_server_runtime::ViolationKind::MissingRequiredMember,
                        value: None,
                    },
                ]),
            ));
        }
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

/// The literal first path segment every route of this service shares.
const KEY_VALUE_STORES_SEGMENT: &str = "key-value-stores";
/// The literal `keys` collection segment the key routes share.
const KEYS_SEGMENT: &str = "keys";

/// Whether `req` addresses one of this service's mutating routes — `PutKey`
/// (`PUT /key-value-stores/{KvsARN}/keys/{Key}`), `DeleteKey` (the same path under
/// `DELETE`) or `UpdateKeys` (`POST /key-value-stores/{KvsARN}/keys`) — while omitting
/// the `If-Match` header all three declare `@required`.
///
/// Anchored on the leading `key-value-stores` segment and the trailing `keys` /
/// `keys/{Key}` shape rather than on an exact segment count, so a hand-built client
/// that sends the ARN label with a literal `/` (an SDK percent-encodes it) still
/// resolves to the same route and gets the same 400.
fn missing_if_match_on_mutating_route(req: &aws_smithy_runtime_api::http::Request) -> bool {
    if req.headers().get("if-match").is_some() {
        return false;
    }
    // Parsed with `http::Uri` rather than a hand-written path splitter: it handles both
    // the origin-form target an SDK sends and the absolute URI a hand-built client may,
    // and strips the query string. (The vendored router has its own `request_path`
    // helper, but it is `pub(crate)` in a tree this crate may not edit.)
    let Ok(uri) = req.uri().parse::<::http::Uri>() else {
        return false;
    };
    let segments: Vec<&str> = uri.path().trim_matches('/').split('/').collect();
    if segments.first() != Some(&KEY_VALUE_STORES_SEGMENT) {
        return false;
    }
    match req.method() {
        // `/key-value-stores/{KvsARN}/keys/{Key}` — `keys` is the second-to-last segment.
        "PUT" | "DELETE" => {
            segments.len() >= 4 && segments[segments.len() - 2] == KEYS_SEGMENT
        }
        // `/key-value-stores/{KvsARN}/keys` — `keys` is the last segment.
        "POST" => segments.len() >= 3 && segments[segments.len() - 1] == KEYS_SEGMENT,
        _ => false,
    }
}
