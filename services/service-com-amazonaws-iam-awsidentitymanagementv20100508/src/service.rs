// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-iam-awsidentitymanagementv20100508` service: how it claims requests and how it words the
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

/// The first `/`-separated segment of `path`, or `None` for a path with no non-empty
/// first segment (`"/"`, `""`).
fn first_path_segment(path: &str) -> Option<&str> {
    let segment = path.trim_start_matches('/').split('/').next()?;
    (!segment.is_empty()).then_some(segment)
}

/// Whether `parts` addresses a path inside `path_segments` — the first URI path segments
/// this service's wire protocol puts its operations under.
///
/// This mirrors the framework's own discriminator,
/// `emulator_server::can_handle_based_on_route_key_and_path`, which the generated
/// `can_handle` entry point uses. The framework helper takes a whole buffered
/// `http::Request`, which the `Service::can_handle` seam below does not receive, so the
/// rule is applied here over `Parts` instead of called. It is kept identical, including
/// both edge cases: an EMPTY list means no usable discriminator and is "no constraint",
/// and the framework's own `ROOT_PATH_SEGMENT` sentinel is how a service that answers
/// `POST /` claims the root path without claiming every path. Matching is EXACT segment
/// equality, deliberately never a prefix test.
fn claims_modeled_path(parts: &http::request::Parts, path_segments: &[&str]) -> bool {
    if path_segments.is_empty() {
        return true;
    }
    match first_path_segment(parts.uri.path()) {
        Some(segment) => path_segments.contains(&segment),
        None => path_segments.contains(&emulator_server::ROOT_PATH_SEGMENT),
    }
}

/// The URI path space IAM's wire protocol uses: the ROOT path, and nothing else.
///
/// IAM speaks `awsQuery` (`smithy_query(query="service_info")`: protocols `["awsQuery"]`)
/// and no operation in its model carries a `smithy.api#http` binding, so every one of its
/// 180 operations arrives as `POST /` with the operation named by the `Action` form field.
/// The generated `crate::ROUTE_PATH_SEGMENTS` is EMPTY for exactly that case — the
/// generator keeps it empty for an RPC service with no bindings so as not to narrow
/// services that already work — and an empty list means "claims every path", which is
/// what made IAM collide with its signing-name peer. The root claim is therefore stated
/// here, using the framework's own sentinel for it. It is deliberately NOT called
/// `ROUTE_PATH_SEGMENTS`: that name belongs to the generated crate-level constant, which
/// is empty.
const WIRE_PATH_SEGMENTS: &[&str] = &[emulator_server::ROOT_PATH_SEGMENT];

/// The error code the query protocols name a refusal of a value outside a DECLARED BOUND
/// with (`vendor/smithy-server-runtime/src/pre_dispatch.rs`), and the code the framework
/// itself would have answered this refusal with. It is restated here because [`Refused`]
/// replaces the code and the message TOGETHER and only the message changes below: a
/// client branches on the code, so it must stay the one it would have seen.
const BOUND_REFUSAL_CODE: &str = "ValidationError";

/// Whether `violation` is a value outside a BOUND the model declares for its member — a
/// number outside its `@range`, or a string or collection outside its `@length`.
///
/// The other classes the framework reports are not bounds: a string outside its
/// `@pattern`, a value outside an enumerated set, a repeated element, an absent required
/// member.
fn is_declared_bound(violation: &Violation) -> bool {
    matches!(
        violation.kind,
        ViolationKind::MinValue(_)
            | ViolationKind::MaxValue(_)
            | ViolationKind::MinLength(_)
            | ViolationKind::MaxLength(_)
    )
}

/// A boundary refusal restated so a value outside a DECLARED BOUND names the member and
/// the bound it broke and nothing of the value itself, or `None` when the request broke no
/// such bound and the framework's own wording stands.
///
/// This service states that class of refusal without echoing the rejected value — the
/// clause reads `Value at '<member>' failed to satisfy constraint: Member must have
/// {value,length} {greater,less} than or equal to <bound>` — while the other classes (a
/// string outside its `@pattern`, a value outside an enumerated set, an absent required
/// member) keep the framework's sentence, value and all. Only the `Value '<the value>' at`
/// opening is lifted out: the envelope, the member path and the requirement are the
/// framework's, built from the same two fields the renderer interpolated, so the two
/// cannot drift and the replacement matches the echo whatever the value is. The ONE thing
/// it couples to is the renderer's quoting of that opening
/// (`vendor/smithy-schema-server/src/constraints.rs::Violation::message`, `Value
/// '<shown>' at '<path>'`); the suite below asserts the whole message and that the value
/// appears nowhere in it, so a renderer that quoted it differently would fail there rather
/// than turn this into a silent no-op.
///
/// The rule covers every member with a bound, not one member: the bound and the member
/// path both come from the model, so a page size, a session duration, a description's
/// length and a tag list's size are one class of refusal and are worded alike
/// (`tests/all/range_refusal_wording.rs` reads them back, over both bound directions and
/// over the scalar and collection forms of a length). Two of those rows go beyond what
/// this service's recorded answers show — a role name over its `@length` maximum and a tag
/// list over its own — and follow the class rather than an observation of their own; a
/// later recording that echoes the value for one of them would narrow the class here, not
/// contradict the seam.
///
/// A refusal carrying an ABSENT required member is declined whole: that one renders under
/// a different code than a failed bound, and restating it under [`BOUND_REFUSAL_CODE`]
/// would change what a client branches on.
fn without_bound_value_echo(violations: &[Violation]) -> Option<Refused> {
    let absent_member = violations
        .iter()
        .any(|violation| matches!(violation.kind, ViolationKind::MissingRequiredMember));
    if absent_member || !violations.iter().any(is_declared_bound) {
        return None;
    }
    let mut message = envelope(violations)?;
    for violation in violations.iter().filter(|violation| is_declared_bound(violation)) {
        let Some(value) = violation.value.as_deref() else {
            continue;
        };
        message = message.replace(
            &format!("Value '{value}' at '{}'", violation.path),
            &format!("Value at '{}'", violation.path),
        );
    }
    Some(Refused {
        code: BOUND_REFUSAL_CODE.to_string(),
        message,
    })
}

/// The `com-amazonaws-iam-awsidentitymanagementv20100508` emulated service: the generated [`ServiceDefinition`] plus
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
        // This service signs as `iam` — a signing name it SHARES with IAM Toolbox
        // (`services/service-com-amazonaws-iamtoolbox-authrequestservice`, sigv4Name
        // `iam`, endpointPrefix `iam-toolbox`). The router's routing key comes from the
        // SigV4 credential scope first, so both services resolve to `iam` and matching on
        // the signing name ALONE makes both claim every request — which the router
        // answers `500 AmbiguousService` for, in a JSON body an awsQuery client cannot
        // even parse.
        //
        // So the claim is narrowed the way the framework narrows the other signing-name
        // collisions: the signing name must match AND the request must address this
        // service's own URI path space, which for an awsQuery service is the root path
        // (see `WIRE_PATH_SEGMENTS`). IAM Toolbox's `/authorization-details/…` is then
        // claimed by it alone.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) if key == self.entry.identity().signing_name() => {
                claims_modeled_path(parts, WIRE_PATH_SEGMENTS)
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
        // service reword it. Returning `None` keeps the framework's default
        // wording; the one class this service words differently is a value outside
        // a bound its member declares, which it states without echoing the value
        // (see [`without_bound_value_echo`], which answers with the code the
        // framework would have chosen).
        match refusal {
            Refusal::Invalid(violations) => without_bound_value_echo(violations),
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
