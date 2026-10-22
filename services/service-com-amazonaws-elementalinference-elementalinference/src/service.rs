// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-elementalinference-elementalinference` service: how it claims requests and how it words the
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

/// The member that carries a role this service is asked to pass on the caller's
/// behalf (`CreateFeedRequest$accessRoleArn`, `UpdateFeedRequest$accessRoleArn`:
/// "The ARN of an IAM role that Elemental Inference assumes to access resources in
/// your account on your behalf").
const PASS_ROLE_MEMBER: &str = "accessRoleArn";

/// Whether this refused value is a role this service may not pass, rather than a
/// member that merely failed its own syntax rules.
///
/// True for a violation of the pass-role member whose value names an account other
/// than the one this emulator serves: the service reaches the pass-role authorization
/// decision before the member's offline rules, so the account in the ARN decides the
/// answer even when the rest of the ARN would not satisfy the member's `@pattern` or
/// `@length`.
///
/// The framework pins a pre-handler refusal at HTTP 400, so this classification
/// changes the error code and the message; the same decision taken on a value that
/// satisfies the member's rules reaches the handler and is answered there with the
/// modelled `AccessDeniedException` and its own HTTP 403.
fn denies_a_cross_account_pass_role(violation: &Violation) -> bool {
    let names_pass_role_member = violation
        .path
        .split('.')
        .next_back()
        .is_some_and(|member| member == PASS_ROLE_MEMBER);
    names_pass_role_member
        && violation
            .value
            .as_deref()
            .is_some_and(crate::state::names_another_account)
}

/// The same violation with a `@pattern` stripped of its anchors.
///
/// A constraint-violation message names the member and the constraint it failed, and
/// this service renders a regular-expression constraint as the expression ALONE: the
/// leading `^` and trailing `$` of an anchored modelled pattern are not part of the
/// rendering. Every other violation kind is returned unchanged.
fn without_pattern_anchors(violation: &Violation) -> Violation {
    let ViolationKind::Pattern(pattern) = &violation.kind else {
        return violation.clone();
    };
    let mut stripped: &str = pattern.strip_prefix('^').unwrap_or(pattern);
    // A trailing `$` that is ESCAPED (`\$`) matches a literal dollar sign and is not
    // an anchor, so it stays.
    if let Some(body) = stripped.strip_suffix('$') {
        if !body.ends_with('\\') {
            stripped = body;
        }
    }
    Violation {
        kind: ViolationKind::Pattern(stripped.to_string()),
        ..violation.clone()
    }
}

/// The `com-amazonaws-elementalinference-elementalinference` emulated service: the generated [`ServiceDefinition`] plus
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
        // wording.
        //
        // Two things this service states differently from the default, both about
        // values the model constrains:
        //
        // 1. a role ARN naming another account is an AUTHORIZATION refusal, not a
        //    malformed request: Elemental Inference does not resolve a passed role
        //    offline, it evaluates the role by attempting to pass it, so that check
        //    is reached before the member's own syntax rules are. Both the code and
        //    the message change together, which is what a client branches on.
        // 2. a `@pattern` renders into a constraint-violation message WITHOUT its
        //    anchors, for every anchored pattern member of this service.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        if violations.iter().any(denies_a_cross_account_pass_role) {
            return Some(Refused {
                code: "AccessDeniedException".to_string(),
                message: crate::state::CROSS_ACCOUNT_PASS_ROLE_MESSAGE.to_string(),
            });
        }
        // Nothing else this service words differently, so a refusal with no pattern
        // violation keeps the framework's own rendering rather than being re-rendered
        // here: the blast radius stays on the one rule above.
        if !violations
            .iter()
            .any(|violation| matches!(violation.kind, ViolationKind::Pattern(_)))
        {
            return None;
        }
        let reworded: Vec<Violation> = violations.iter().map(without_pattern_anchors).collect();
        Some(Refused {
            code: "ValidationException".to_string(),
            // The same envelope the framework renders (`1 validation error detected:
            // …`), built from the reworded violations so the clause wording stays in
            // the one place that owns it.
            message: aws_smithy_schema_server::constraints::envelope(&reworded)?,
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
