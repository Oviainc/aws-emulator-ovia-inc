// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-gameliftstreams-gameliftstreams` service: how it claims requests and how it words the
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

use aws_smithy_schema_server::{envelope, Violation, ViolationKind};
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// Whether `violation` is a `StreamClass` value outside the model's enum — the
/// refusal whose enumerated set this service narrows.
fn is_stream_class_enum_violation(violation: &Violation) -> bool {
    violation.path == crate::state::STREAM_CLASS_PATH
        && matches!(violation.kind, ViolationKind::EnumValue(_))
}

/// The `com-amazonaws-gameliftstreams-gameliftstreams` emulated service: the generated [`ServiceDefinition`] plus
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
        // service reword it. Both rewordings below are recorded behaviour of
        // Amazon GameLift Streams that the model cannot express.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };

        // A `ResourceArn` that is not an ARN at all is answered by this API with
        // the error code `BadRequestException` (HTTP 400) — not the
        // `ValidationException` the rest of the API refuses a bad value with, and
        // not a code the model lists for the tagging operations at all. A client
        // branching on the code needs the code the service sends.
        //
        // A `resourceArn` pattern violation takes PRECEDENCE over any other
        // violation in the same request, deliberately: a request that is malformed
        // this way and in another way too is answered as a bad resource ARN,
        // because that is the refusal the caller has to fix first — the ARN names
        // what the rest of the request is about.
        if violations.iter().any(|violation| {
            violation.path == crate::state::RESOURCE_ARN_PATH
                && matches!(violation.kind, ViolationKind::Pattern(_))
        }) {
            return Some(Refused {
                code: "BadRequestException".to_string(),
                message: "Invalid input resource arn: bad syntax".to_string(),
            });
        }

        // The `StreamClass` set the service accepts is NARROWER than the model's
        // enum (see `crate::state::SERVICE_STREAM_CLASSES`), so the set this
        // refusal enumerates is narrowed to the one the service would enumerate.
        // The two model-only values are refused by the `CreateStreamGroup` handler,
        // which words its refusal through the very same envelope.
        if violations.iter().any(is_stream_class_enum_violation) {
            let narrowed: Vec<Violation> = violations
                .iter()
                .map(|violation| {
                    if is_stream_class_enum_violation(violation) {
                        // One source for the accepted set and its wording, shared
                        // with the `CreateStreamGroup` handler.
                        crate::state::stream_class_violation(violation.value.as_deref())
                    } else {
                        violation.clone()
                    }
                })
                .collect();
            // Falling back to the framework's own wording is NOT an option here:
            // that is the message carrying the wider model enum, which is the one
            // thing this branch exists to replace. The same plain sentence the
            // handler falls back to is used instead.
            let message = envelope(&narrowed).unwrap_or_else(crate::state::stream_class_fallback);
            return Some(Refused {
                code: "ValidationException".to_string(),
                message,
            });
        }
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
