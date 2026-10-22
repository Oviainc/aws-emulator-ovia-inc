// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-dynamodb-dynamodb-20120810` service: how it claims requests and how it words the
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

/// The error code this service answers an input-constraint violation with: the
/// `ValidationException` (HTTP 400) every DynamoDB operation refuses a malformed
/// request with. Restated here because `override_refusal` replaces the code and
/// the message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The path segment the framework contributes when it steps from a collection
/// into one of its elements (`RequestItems.<table>.member.Keys`). It is a fixed
/// marker rather than a model name, so it is never re-cased — and the segment
/// BEFORE it is the caller's own map key or the element's 1-based position, so
/// that one is never re-cased either.
const COLLECTION_STEP: &str = "member";

/// A member path as DynamoDB states it: the framework builds the path in
/// lowerCamel (its own convention, `path::member`), and this service names a
/// member the way its model spells it on the wire — every DynamoDB member name
/// is UpperCamel, so restoring the wire spelling is restoring the first
/// character of each MODEL segment. A segment the caller chose (a map key) and
/// the collection marker itself pass through untouched.
///
/// The path arrives FLATTENED (`RequestItems.<table>.member.Keys`), so a map key
/// that itself contains a `.` — a table name may — is indistinguishable from two
/// segments. The segment immediately before the marker is the one taken as the
/// caller's, which is exact for every key without a dot and leaves the earlier
/// part of a dotted key re-cased; the alternative (never re-casing a run) would
/// leave real model segments lowerCamel, which is the error a client reading the
/// path would actually trip over.
fn wire_member_path(path: &str) -> String {
    let segments: Vec<&str> = path.split('.').collect();
    segments
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            let is_collection_marker = *segment == COLLECTION_STEP;
            // A list index or a caller-chosen map key is the segment the
            // collection marker follows: `<key>.member`, `<position>.member`.
            let is_collection_selector =
                segments.get(index + 1).copied() == Some(COLLECTION_STEP);
            if is_collection_marker || is_collection_selector {
                (*segment).to_string()
            } else {
                upper_camel(segment)
            }
        })
        .collect::<Vec<String>>()
        .join(".")
}

/// Upper-case the first character of a member name, leaving the rest alone
/// (`requestItems` -> `RequestItems`, `sSESpecification` -> `SSESpecification`).
fn upper_camel(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The requirement a violation states, in AWS's fixed phrasing, for the clause
/// form the framework's own renderer cannot produce — the one that echoes NO
/// value. Every phrase comes from `crate::state`, which is where this service
/// spells each of them once (and says why they are not borrowed from the
/// framework).
///
/// `None` for a violation class this service has no wording for, which lets
/// [`validation_envelope`] decline the whole refusal and keep the framework's
/// wording rather than invent a sentence ([`ViolationKind`] is
/// `#[non_exhaustive]`, so a class can be added upstream).
fn requirement(kind: &ViolationKind) -> Option<String> {
    Some(match kind {
        ViolationKind::MinLength(min) => {
            crate::state::min_length_requirement(usize::try_from(*min).unwrap_or(0))
        }
        ViolationKind::MaxLength(max) => {
            crate::state::max_length_requirement(usize::try_from(*max).unwrap_or(0))
        }
        ViolationKind::MinValue(min) => crate::state::min_value_requirement(min),
        ViolationKind::MaxValue(max) => crate::state::max_value_requirement(max),
        ViolationKind::Pattern(pattern) => crate::state::pattern_requirement(pattern),
        ViolationKind::UniqueItems => crate::state::unique_items_requirement(),
        // Everything else has no no-echo phrase here: the enum and
        // absent-member classes render exactly as the framework renders them,
        // so `violation_clause` hands those to `Violation::message` instead,
        // and a class added upstream has no phrase at all, which declines the
        // refusal and keeps the framework's wording.
        _ => return None,
    })
}

/// One clause for one violated member.
///
/// Which member path is stated, and whether the rejected value is echoed,
/// follows the violation's CLASS, as AWS's own refusals do:
///
/// * a value outside an enumerated member's value set is the request's own
///   string, and AWS reports it the way its request validator does — the
///   member's lowerCamel path, with that value echoed, so a caller sees which
///   of its values is not a member of the set. That is exactly the framework's
///   own rendering, so the framework renders it (`Violation::message`) rather
///   than this service restating the sentence;
/// * an absent member states the `null` it was, which is a fact of the
///   request's shape — the framework's rendering again, over the member path
///   spelled as the model spells it;
/// * a bound on a member's SIZE or MAGNITUDE (`@length`, `@range`,
///   `@uniqueItems`, `@pattern`) is reported by the member's wire path with no
///   value echoed: the bound is a fact of the model, the member names itself as
///   the model spells it, and the framework's rendering of the offending value
///   (`'101 items'`, `'0'`) is a figure internal to the validating walk rather
///   than anything the caller sent. No renderer in the framework states a
///   clause without a value, so this one is built here.
fn violation_clause(violation: &Violation) -> Option<String> {
    match &violation.kind {
        ViolationKind::MissingRequiredMember | ViolationKind::EnumValue(_) => Some(
            Violation {
                path: match &violation.kind {
                    // An absent member names itself as the model spells it; an
                    // enum violation keeps the validator's own path, which is
                    // what the recorded refusals for this service state.
                    ViolationKind::MissingRequiredMember => wire_member_path(&violation.path),
                    _ => violation.path.clone(),
                },
                ..violation.clone()
            }
            .message(),
        ),
        _ => Some(crate::state::constraint_clause(
            &wire_member_path(&violation.path),
            None,
            &requirement(&violation.kind)?,
        )),
    }
}

/// The `ValidationException` message for a framework refusal: the same envelope
/// `crate::state::validation_message` builds for the bounds this service checks
/// itself, over one clause per violated member.
///
/// `None` declines the whole refusal — see [`requirement`].
fn validation_envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations
        .iter()
        .map(violation_clause)
        .collect::<Option<Vec<_>>>()?;
    crate::state::validation_message(clauses)
}

/// The service key `::emulator_server::service_from_target` reads out of a
/// DynamoDB STREAMS request's `X-Amz-Target` header. That service signs as
/// `dynamodb` too, so this key is what tells its traffic apart from ours: the
/// shared reader takes the segment before the `.`, strips the `_<version>`
/// suffix and lower-cases it, so `DynamoDBStreams_20120810.<Op>` normalises to
/// this key while `DynamoDB_20120810.<Op>` normalises to `dynamodb`. The streams
/// crate's own `can_handle` compares against the same key, so one discriminator
/// is read one way on both sides.
const STREAMS_TARGET_SERVICE_KEY: &str = "dynamodbstreams";

/// The `com-amazonaws-dynamodb-dynamodb-20120810` emulated service: the generated [`ServiceDefinition`] plus
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
        // DynamoDB and DynamoDB Streams are two services behind ONE SigV4 signing
        // name (`dynamodb`), which the model cannot express, so the default
        // signing-name match makes BOTH claim every DynamoDB-signed request and the
        // router answers `500 AmbiguousService` for all of it. On awsJson1_0 the
        // request names its service in `X-Amz-Target`, so a
        // `DynamoDBStreams_20120810.*` target belongs to the streams service and is
        // declined here.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) if key == self.entry.identity().signing_name() => {
                ::emulator_server::service_from_target(&parts.headers).as_deref()
                    != Some(STREAMS_TARGET_SERVICE_KEY)
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
        // DynamoDB words such a refusal as a `ValidationException` (HTTP 400 —
        // the status and code the framework would also have answered, so only
        // the wording changes) carrying the `<N> validation error(s) detected: `
        // envelope over one clause per violated member, each clause naming the
        // member by the path the MODEL spells on the wire and the constraint it
        // failed. An authentication refusal keeps the framework's wording: it is
        // not an input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: validation_envelope(violations)?,
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
