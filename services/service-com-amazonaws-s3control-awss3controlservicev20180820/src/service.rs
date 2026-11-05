// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-s3control-awss3controlservicev20180820` service: how it claims requests and how it words the
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
//! Three modules beside this file carry what those seams are used FOR here, so
//! the rules live in one place each rather than inside the seam bodies:
//!
//! - [`routes`] — the routes whose URI template puts literal segments after a
//!   greedy `@httpLabel`, which the model-derived routing table cannot match,
//!   and the rule that resolves them;
//! - [`authorization`] — the account a request names, and whether the caller is
//!   authorized for it, which is decided ahead of the operation it names;
//! - [`envelope`] — the error envelope every refusal this service answers is
//!   finished in, and the one dispatch seam `override_operation` hands back.

pub mod authorization;
pub mod envelope;
pub mod routes;

use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-s3control-awss3controlservicev20180820` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: envelope::S3ControlDefinition<Eng>,
    /// Resolves the operation a request names — the sub-resource routes the
    /// generated table cannot name included — and finishes the error envelope
    /// of every response this service answers. Held here because
    /// [`Service::override_operation`] hands back a borrow of it.
    dispatch: envelope::Dispatch,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: envelope::S3ControlDefinition::new(engine),
            dispatch: envelope::Dispatch,
        }
    }

    /// Whether `path` is in the URI path space this service's model declares.
    ///
    /// This mirrors the framework's own
    /// [`::emulator_server::can_handle_based_on_route_key_and_path`] rule — the
    /// empty-list fallback, EXACT first-segment equality (never a prefix test,
    /// so a peer's segment that merely starts with one of ours is not claimed)
    /// and the [`::emulator_server::ROOT_PATH_SEGMENT`] sentinel. It is mirrored
    /// rather than called because that helper takes a whole buffered
    /// `http::Request` while [`Service::can_handle`] is handed only the request
    /// parts, and the framework crate is vendored (not editable from here).
    fn claims_path(&self, path: &str) -> bool {
        if crate::ROUTE_PATH_SEGMENTS.is_empty() {
            return true;
        }
        match path.trim_start_matches('/').split('/').next() {
            Some(segment) if !segment.is_empty() => {
                crate::ROUTE_PATH_SEGMENTS.contains(&segment)
            }
            // The root path (`POST /`), which an RPC service claims through the
            // framework's sentinel segment.
            _ => crate::ROUTE_PATH_SEGMENTS.contains(&::emulator_server::ROOT_PATH_SEGMENT),
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
        // S3 Control SHARES its SigV4 signing name (`s3`) with Amazon S3, so the
        // signing name alone does not name this service and the router would see
        // two claimants. The model's own path space is the discriminator: every
        // S3 Control operation is under `/v20180820`, which is
        // `crate::ROUTE_PATH_SEGMENTS` (derived from the model at build time),
        // while S3's operations are under `/{Bucket}/{Key}`.
        //
        // An EMPTY segment list means the model published no usable path
        // discriminator, and this degrades to matching on the signing name alone
        // — the same rule the framework's own
        // `can_handle_based_on_route_key_and_path` applies.
        match smithy_server_runtime::resolve_route_key(parts) {
            // Case-insensitively, as the framework's own
            // `can_handle_based_on_route_key` compares it.
            Some(key) => {
                key.eq_ignore_ascii_case(self.entry.identity().signing_name())
                    && self.claims_path(parts.uri.path())
            }
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Every request this service serves is served through the one seam in
        // `envelope`: it runs the same routing-table entry the router would have
        // run — plus the Multi-Region Access Point sub-resource routes that table
        // cannot name, whose templates put a literal segment after a greedy label
        // (see `routes`) — and then finishes the response's error envelope. A
        // request the seam resolves to nothing is left alone, so naming no
        // modelled operation is still refused in front of the service, in the
        // entry's own envelope, which `envelope::S3ControlDefinition` finishes the
        // same way.
        self.entry
            .routes(req)
            .then_some(&self.dispatch as &dyn ErasedOperation)
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
