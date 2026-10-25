// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-pipes-pipes` service: how it claims requests and how it words the
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

/// The error code the layer that fronts this service's tagging operations refuses a
/// broken tag rule with. No operation of this model declares it — the layer is not the
/// service — so a refusal worded with it cannot come from the modelled error enums; see
/// `crate::scaffolded::Scaffolded::BadRequest` for the handler-side channel.
const TAG_RULE_REFUSAL_CODE: &str = "BadRequestException";

/// The input members a TAG's own content arrives on, and the name request validation
/// reports a refused tag at for each.
///
/// `tagKeys` is bound ONLY by `UntagResource`
/// (`smithy_query(query="digest")`), so a violation on it is attributable and is reported
/// at the name that operation's validation uses. `tags` is bound by `TagResource` AND by
/// `CreatePipe`, and a [`Refusal`] carries no operation id, so a violation on it is
/// reported at the member as the MODEL names it (lowerCamel, which is what both
/// operations' paths spell) rather than at either operation's own validation name.
/// Narrowing that needs an operation-aware refusal seam, which this service cannot add.
const TAG_CONTENT_MEMBERS: &[&str] = &["tags", "tagKeys"];

/// The input member a violation is about: a `Violation::path` is DOTTED
/// (`tagKeys.1.member` for a list element, `tags.<key>` for a map value), so the member
/// is the path's first segment and not the whole path.
fn member_of(violation: &Violation) -> &str {
    violation.path.split('.').next().unwrap_or_default()
}

/// How a violation on one of [`TAG_CONTENT_MEMBERS`] is worded, or `None` when the
/// violation is not about ONE tag's content.
///
/// A violation at the member itself (`tags`, `tagKeys`) is about how MANY tags the
/// request carries — the collection's own `@length` — which is not a tag rule and keeps
/// the modelled wording. A violation one step inside it (a map VALUE, whose path segment
/// is the entry's key, or a list ELEMENT) is a tag whose content breaks the published
/// `TagKey`/`TagValue` bound, which is the layer's refusal to word.
fn tag_content_clause(violation: &Violation) -> Option<String> {
    let (member, inside) = violation.path.split_once('.')?;
    let value = violation.value.as_deref().unwrap_or_default();
    match member {
        // `tags.<key>`: the VALUE sent for that key is outside `TagValue`'s bound. The
        // key is the rest of the path, which it contributes whole (a key may carry a
        // dot of its own). A [`Refusal`] carries only the value that broke the rule, so
        // the entry it broke is the whole of the member's value this can echo.
        "tags" => Some(crate::state::tag_refusal(
            crate::state::CREATE_TAGS_MEMBER,
            crate::state::TagViolation::ValueLength,
            inside,
            &crate::state::echoed_tags(std::iter::once((inside, value))),
        )),
        // `tagKeys.<position>.member`: that key is outside `TagKey`'s bound.
        "tagKeys" => Some(crate::state::tag_key_refusal(
            crate::state::REMOVED_TAG_KEYS_MEMBER,
            value,
        )),
        _ => None,
    }
}

/// The `com-amazonaws-pipes-pipes` emulated service: the generated [`ServiceDefinition`] plus
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
        // Two layers answer a refused pipes request, and they do not answer alike.
        //
        // The resource-tagging layer in FRONT of this service owns the three
        // ARN-addressed tagging operations, and it refuses a tag whose content breaks a
        // published tag rule with its own code — `BadRequestException`, which no
        // operation of this model declares, because the layer is not the service. The
        // rule the tag broke is one the model DOES declare (`TagKey`'s and `TagValue`'s
        // `@length`), so the boundary validator is what finds it, and this is where that
        // refusal is worded. `crate::state::tag_refusal` states the same sentences for
        // the rules the handlers enforce themselves, so one tag rule is worded one way
        // whichever layer catches it.
        //
        // Everything else is the SERVICE's own refusal, and this service models
        // `ValidationException` for it — which is the code the default already names —
        // so it keeps the framework's wording.
        let violations = match refusal {
            Refusal::Invalid(violations) => violations,
            // An authentication refusal is the framework's 403 to word, not ours.
            _ => return None,
        };
        if !violations
            .iter()
            .all(|violation| TAG_CONTENT_MEMBERS.contains(&member_of(violation)))
        {
            return None;
        }
        let clauses: Vec<String> = violations.iter().filter_map(tag_content_clause).collect();
        // One refused tag at a time: the layer states the first tag rule the request
        // broke, as the handlers do, so the two cannot be told apart by how many
        // clauses they carry.
        let message = clauses.into_iter().next()?;
        Some(Refused {
            code: TAG_RULE_REFUSAL_CODE.to_string(),
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
