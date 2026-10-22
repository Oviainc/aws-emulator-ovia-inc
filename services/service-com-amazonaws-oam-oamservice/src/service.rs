// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-oam-oamservice` service: how it claims requests and how it words the
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

/// The validation paths bound ONLY by the three tagging operations, whose modelled
/// client-input error is `ValidationException` rather than the two parameter errors
/// every other operation in this service declares.
///
/// `ResourceArn` is an `@httpLabel` on all three and `TagKeys` an `@httpQuery` on
/// `UntagResource`; no other operation binds either
/// (`smithy_query(query="digest")`). The paths are lowerCamel because that is the
/// casing the validation envelope uses.
///
/// `Tags` is deliberately NOT here: `TagResource` binds it, but so do `CreateSink`
/// and `CreateLink`, where the parameter errors ARE the modelled ones — and a
/// [`Refusal`] carries no operation id, so a violation on `tags` alone cannot be
/// attributed to one of them. Such a refusal therefore keeps the parameter wording,
/// which is right for the two creates and is the one case where a tagging operation
/// is worded with a code it does not declare. Narrowing it further needs an
/// operation-aware refusal seam in the framework, which this service cannot add.
const TAGGING_ONLY_MEMBERS: &[&str] = &["resourceArn", "tagKeys"];

/// The `com-amazonaws-oam-oamservice` emulated service: the generated [`ServiceDefinition`] plus
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
        // OAM models its own two client-input errors and uses them where the
        // framework's default wording would say `ValidationException`: a member the
        // request left out is `MissingRequiredParameterException` ("A required
        // parameter is missing from the request") and a value outside a declared
        // bound, pattern or enum is `InvalidParameterException` ("A parameter is
        // specified incorrectly"). Both are HTTP 400, which is the status this
        // refusal already carries, so only the code and the message change.
        //
        // The three tagging operations are the exception: `ListTagsForResource`,
        // `TagResource` and `UntagResource` model `ValidationException` and neither
        // of the two codes above, and they are the only operations that bind
        // `ResourceArn` and `TagKeys` — so a violation on one of those members keeps
        // the `ValidationException` the default already names.
        let violations = match refusal {
            Refusal::Invalid(violations) => violations,
            // An authentication refusal is the framework's 403 to word, not ours.
            _ => return None,
        };
        if violations.is_empty() {
            return None;
        }
        if violations.iter().any(|violation| {
            // `Violation::path` is DOTTED (`tagKeys.1.member` for a list element),
            // so the member is the path's first segment, not the whole path.
            let member = violation.path.split('.').next().unwrap_or_default();
            TAGGING_ONLY_MEMBERS.contains(&member)
        }) {
            return None;
        }
        // One absent member makes the whole request a missing-parameter refusal; the
        // caller has to supply it before any other complaint matters.
        let code = if violations
            .iter()
            .any(|violation| violation.kind == ViolationKind::MissingRequiredMember)
        {
            "MissingRequiredParameterException"
        } else {
            "InvalidParameterException"
        };
        let message = violations
            .iter()
            .map(Violation::message)
            .collect::<Vec<String>>()
            .join("; ");
        Some(Refused {
            code: code.to_string(),
            message,
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
