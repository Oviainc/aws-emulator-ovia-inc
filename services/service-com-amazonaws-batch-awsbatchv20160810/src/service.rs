// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-batch-awsbatchv20160810` service: how it claims requests and how it words the
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

/// The error code AWS answers a refusal of this service's TAG MAP with.
///
/// Tagging is served by the Tagris frontend rather than by Batch's own request
/// validator, and its refusal of a tag map outside the bounds `TagrisTagsMap` declares
/// (`@length(min: 1, max: 50)`) is HTTP 400 `BadRequestException` — the code recorded for
/// a 51-entry `tags` map on a Batch resource.
const TAG_REFUSAL_CODE: &str = "BadRequestException";

/// The error code AWS answers every OTHER refusal of a Batch request with.
///
/// This service's model declares exactly two errors on every operation —
/// `ClientException` (HTTP 400) and `ServerException` (HTTP 500) — so a client-side
/// refusal is a `ClientException`, whoever found it. The framework's synthetic
/// `ValidationException` is not in Batch's error set at all, so an SDK branching on the
/// code cannot match it.
const REFUSAL_CODE: &str = "ClientException";

/// The input member Batch's tag map arrives on, on every operation that takes one
/// (`TagResource$tags`, `CreateJobQueue$tags`, `RegisterJobDefinition$tags`, …).
const TAGS_MEMBER: &str = "tags";

/// Whether `violation` is about the tag map (the member itself, or a key or value inside
/// it), which is what makes the refusal Tagris's rather than Batch's.
fn is_tag_violation(violation: &Violation) -> bool {
    violation.path == TAGS_MEMBER || violation.path.starts_with(&format!("{TAGS_MEMBER}."))
}

/// The `com-amazonaws-batch-awsbatchv20160810` emulated service: the generated [`ServiceDefinition`] plus
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
        // required member, or a value outside a declared bound) so this service can
        // answer with a code a Batch client actually branches on. The framework's
        // synthetic `ValidationException` is in no Batch operation's error set, so BOTH
        // the code and the message are replaced here:
        //
        // * a refusal of the tag map is Tagris's, HTTP 400 `BadRequestException` — the
        //   code recorded for a 51-entry `tags` map;
        // * every other refusal is the `ClientException` (HTTP 400) the model declares.
        //
        // The message stays the one the framework built from the request: it names the
        // member and the constraint it failed, which is what the caller has to fix.
        // Authentication refusals keep the framework's wording (403 is not a Batch
        // error shape at all).
        match refusal {
            Refusal::Invalid(violations) => {
                let code = if violations.iter().any(is_tag_violation) {
                    TAG_REFUSAL_CODE
                } else {
                    REFUSAL_CODE
                };
                Some(Refused {
                    code: code.to_string(),
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
