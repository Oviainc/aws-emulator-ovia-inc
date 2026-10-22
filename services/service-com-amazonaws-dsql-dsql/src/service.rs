// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-dsql-dsql` service: how it claims requests and how it words the
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

/// `TagMap`'s modelled maximum entry count (`smithy_query(query="shape",
/// shape="TagMap")`: `@length(min: 0, max: 200)`). A request carrying more tags
/// than this is a bad request BODY, not a member-level constraint failure — see
/// [`refused_for`].
const TAG_MAP_MAX_ENTRIES: i64 = 200;

/// The member every `TagMap` in this service is bound to (`tags` on
/// `CreateCluster`, `CreateStream` and `TagResource`:
/// `smithy_query(query="operation", operation="CreateCluster,CreateStream,TagResource")`).
const TAG_MAP_MEMBER: &str = "tags";

/// The error code Aurora DSQL refuses a malformed request body with, and the
/// sentence it carries. Not a modelled shape: the service's REST frontend
/// answers it before any operation's own errors are reachable.
const BAD_REQUEST_CODE: &str = "BadRequestException";
/// See [`BAD_REQUEST_CODE`].
const BAD_REQUEST_MESSAGE: &str = "Invalid request body";

/// How this service states a set of constraint violations.
///
/// Two refusals, told apart by WHAT failed:
///
/// * a tag map over [`TAG_MAP_MAX_ENTRIES`] is a bad request body, so the answer
///   is [`BAD_REQUEST_CODE`] with [`BAD_REQUEST_MESSAGE`] — the whole batch is
///   refused and none of it is applied;
/// * everything else is a `ValidationException` whose message is the AWS
///   envelope built from the member and its modelled constraint. DSQL's
///   construction states the requirement ONLY: it does not echo the rejected
///   value back to the caller, and it quotes a `@pattern` as modelled minus the
///   `^`/`$` anchors.
fn refused_for(violations: &[Violation]) -> Refused {
    if violations.iter().any(is_tag_map_overflow) {
        return Refused {
            code: BAD_REQUEST_CODE.to_string(),
            message: BAD_REQUEST_MESSAGE.to_string(),
        };
    }
    Refused {
        code: "ValidationException".to_string(),
        message: validation_envelope(violations),
    }
}

/// Whether `violation` is a tag map carrying more entries than `TagMap` allows.
fn is_tag_map_overflow(violation: &Violation) -> bool {
    violation.path.rsplit('.').next() == Some(TAG_MAP_MEMBER)
        && matches!(violation.kind, ViolationKind::MaxLength(max) if max == TAG_MAP_MAX_ENTRIES)
}

/// `1 validation error detected: <clause>` / `N validation errors detected:
/// <clause>; <clause>`, each clause as [`clause_for`] words it.
fn validation_envelope(violations: &[Violation]) -> String {
    let clauses: Vec<String> = violations.iter().map(clause_for).collect();
    match clauses.len() {
        // The framework only calls this with at least one violation; an empty
        // set still has to read as a refusal rather than as an empty sentence.
        0 => "1 validation error detected".to_string(),
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
    }
}

/// One violation as DSQL states it: the member and the requirement it failed,
/// with no echo of the value the caller sent.
fn clause_for(violation: &Violation) -> String {
    let path = &violation.path;
    let requirement = match &violation.kind {
        // An absent member has no value to withhold, so the clause names the
        // absence itself.
        ViolationKind::MissingRequiredMember => {
            return format!("Value null at '{path}' failed to satisfy constraint: Member must not be null")
        }
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        ),
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        // `ViolationKind` is `#[non_exhaustive]`: a class this service has no
        // wording for keeps the framework's own clause rather than being dropped.
        _ => return violation.message(),
    };
    format!("Value at '{path}' failed to satisfy constraint: {requirement}")
}

/// A `@pattern` as DSQL quotes it: the modelled regex without its leading `^`
/// and trailing `$`, which are the anchors Smithy's find-semantics make
/// redundant and which AWS does not print.
fn unanchored(pattern: &str) -> &str {
    let without_caret = pattern.strip_prefix('^').unwrap_or(pattern);
    without_caret.strip_suffix('$').unwrap_or(without_caret)
}

/// The `com-amazonaws-dsql-dsql` emulated service: the generated [`ServiceDefinition`] plus
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
        // service state it the way Aurora DSQL states it. Both halves below
        // replace the code AND the message together, because a client branches on
        // the code.
        match refusal {
            Refusal::Invalid(violations) => Some(refused_for(violations)),
            // Authentication is the framework's answer to give; DSQL words it no
            // differently.
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
