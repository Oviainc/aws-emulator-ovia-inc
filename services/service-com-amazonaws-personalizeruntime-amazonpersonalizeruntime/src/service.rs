// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-personalizeruntime-amazonpersonalizeruntime` service: how it claims requests and how it words the
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

/// The error code the framework answers an input-constraint violation with, and the
/// one the recordings show this service answering: `ValidationException` (HTTP 400).
/// Restated here because `override_refusal` replaces the code and the message
/// together, and only the message changes below.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// A `@pattern` expression without the anchors the model spells it with. A `$` escaped
/// by a backslash is a literal dollar sign inside the expression, not the end anchor,
/// so it stays.
///
/// MIGRATION NOTE (the one `services/common/src/validation.rs` asks every new inliner
/// to leave): the unanchored-pattern rule and the value-free subject below are the same
/// in ~24 service crates (`grep -rln 'fn unanchored' services/*/src`). They are inlined
/// here rather than called because `Violation::requirement` is private to the vendored
/// framework and `vendor/` is read-only from inside the emulator, so there is no public
/// seam to call; the fix that owns the migration declares the shared helper and replaces
/// every copy, this one included, with a call to it.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// Whether `violation` is the class this service words itself: a violated `@pattern`.
fn is_pattern(violation: &Violation) -> bool {
    matches!(violation.kind, ViolationKind::Pattern(_))
}

/// One clause of this service's `ValidationException` message.
///
/// A violated `@pattern` is stated the way the Personalize data plane states it: the
/// member path and the expression a value has to satisfy, WITHOUT the `^`/`$` anchors
/// the model writes to make the match total, and WITHOUT echoing the rejected value —
/// every patterned member of this service is a resource ARN, which the service does not
/// quote back. Every other class keeps the framework's own clause, which already reads
/// the way AWS words it.
fn clause(violation: &Violation) -> String {
    match &violation.kind {
        ViolationKind::Pattern(pattern) => format!(
            "Value at '{}' failed to satisfy constraint: Member must satisfy regular expression \
             pattern: {}",
            violation.path,
            unanchored(pattern)
        ),
        _ => violation.message(),
    }
}

/// This service's `ValidationException` message: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation.
///
/// `None` declines the refusal — leaving the framework's wording untouched — for a
/// request with no `@pattern` violation in it, since that is the only class this file
/// words differently. A request that mixes one with another class IS reworded, so the
/// pattern clause loses its anchors and its value echo while every other clause reads
/// exactly as the framework would have stated it.
fn pattern_envelope(violations: &[Violation]) -> Option<String> {
    if !violations.iter().any(is_pattern) {
        return None;
    }
    let clauses: Vec<String> = violations.iter().map(clause).collect();
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The first path segment of a URI, or `None` for the root path.
fn first_path_segment(path: &str) -> Option<&str> {
    let segment = path.trim_start_matches('/').split('/').next()?;
    (!segment.is_empty()).then_some(segment)
}

/// Whether `parts` addresses a path inside `path_segments` — the model-derived first
/// URI path segments this service declares (`crate::ROUTE_PATH_SEGMENTS`).
///
/// This mirrors the framework's own discriminator,
/// `emulator_server::can_handle_based_on_route_key_and_path`, which the generated
/// `can_handle` entry point uses. The framework helper takes a whole buffered
/// `http::Request`, which the `Service::can_handle` seam below does not receive, so the
/// rule is applied here over `Parts` instead of called. It is kept identical, including
/// both edge cases: an EMPTY list means the model published no usable discriminator and
/// is "no constraint", and the framework's own `ROOT_PATH_SEGMENT` sentinel is how a
/// service that must answer `POST /` claims the root path without claiming every path.
/// Matching is EXACT segment equality, deliberately never a prefix test.
fn claims_modeled_path(parts: &http::request::Parts, path_segments: &[&str]) -> bool {
    if path_segments.is_empty() {
        return true;
    }
    match first_path_segment(parts.uri.path()) {
        Some(segment) => path_segments.contains(&segment),
        None => path_segments.contains(&emulator_server::ROOT_PATH_SEGMENT),
    }
}

/// The `com-amazonaws-personalizeruntime-amazonpersonalizeruntime` emulated service: the generated [`ServiceDefinition`] plus
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
        // THREE services sit behind ONE SigV4 signing name `personalize`: Amazon
        // Personalize (the control plane, endpoint `personalize`), Amazon Personalize
        // Events (endpoint `personalize-events`) and this one (endpoint
        // `personalize-runtime`). No Smithy trait expresses that, and the router's
        // routing key comes from the SigV4 credential scope first, so matching on the
        // signing name ALONE makes all three claim every Personalize-signed request —
        // which the router answers `500 AmbiguousService` for, including the three
        // inference calls this service exists to serve.
        //
        // So the claim is narrowed the way the framework narrows every other
        // signing-name collision: the signing name must match AND the request must
        // address this service's own modeled URI path space — `/recommendations`,
        // `/personalize-ranking`, `/action-recommendations` (see
        // `crate::ROUTE_PATH_SEGMENTS`, generated from the model). The control plane
        // tells its awsJson1_1 traffic apart by `X-Amz-Target` and Personalize Events
        // claims its own paths, so each family member claims only its own requests.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) if key == self.entry.identity().signing_name() => {
                claims_modeled_path(parts, crate::ROUTE_PATH_SEGMENTS)
            }
            _ => false,
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
        // service reword it.
        //
        // Amazon Personalize Runtime states a violated `@pattern` without the model's
        // anchors and without echoing the rejected value; the code stays the
        // `ValidationException` (HTTP 400) the framework would have answered, so only
        // the wording changes, and only for that class — every other refusal,
        // authentication included, keeps the framework's wording because
        // `pattern_envelope` declines it.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: pattern_envelope(violations)?,
            }),
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
