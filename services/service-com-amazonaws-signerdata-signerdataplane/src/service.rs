// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-signerdata-signerdataplane` service: how it claims requests and how it words the
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
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

use aws_smithy_schema_server::constraints::{Violation, ViolationKind};

/// The first `/`-separated segment of `path`, or `None` for a path with no non-empty
/// first segment (`"/"`, `""`).
fn first_path_segment(path: &str) -> Option<&str> {
    let segment = path.trim_start_matches('/').split('/').next()?;
    (!segment.is_empty()).then_some(segment)
}

/// Whether `parts` addresses a path inside `path_segments` — the model-derived first URI
/// path segments this service declares (`crate::ROUTE_PATH_SEGMENTS`).
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

/// The `com-amazonaws-signerdata-signerdataplane` emulated service: the generated [`ServiceDefinition`] plus
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
        // Claim a request whose resolved routing key names THIS service's ENDPOINT,
        // which the model gives as `data-signer` while the SigV4 signing name is
        // `signer`. The router resolves the key from the credential scope first and
        // falls back to the `Host` label, so `data-signer` is the key of a request
        // addressed at this service's own endpoint host
        // (`data-signer.<region>.amazonaws.com`) that carries no credential scope —
        // exactly the traffic this endpoint answers alone.
        //
        // The signing name itself is SHARED with AWS Signer
        // (`services/service-com-amazonaws-signer-wallabyservice`, endpoint `signer`),
        // and it is NOT claimed here: a request signed for `signer` resolves to that
        // key whichever host it was sent to, and the control plane holds the
        // revocation records such a request is answered from
        // (`RevokeSigningProfile` / `RevokeSignature` write them), so it serves the
        // signing name's traffic whole. Claiming the shared name here as well would
        // give every `signer`-signed request two claimants and the router would answer
        // `500 AmbiguousService` for all of it.
        //
        // The claim is also narrowed to this service's own modeled URI path space
        // (`/revocations`, see `claims_modeled_path`), so a request addressed at this
        // endpoint on a path this model does not publish reaches the router's own
        // `404 UnknownService` rather than this service.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) if key == ENDPOINT_PREFIX => {
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
        // What is reworded here is ONE class, and only for a LIST member: a
        // required list the request supplied nothing for. The framework states
        // that as "Value null at '<member>' failed to satisfy constraint: Member
        // must not be null", which asserts the value was NULL — and for a list
        // that is a claim the wire cannot support: a query-bound list that is
        // absent and a query-bound EMPTY list are the same request, so "null"
        // names a constraint the request may not have violated. This service
        // therefore states the constraint such a member actually violated, naming
        // it ([`missing_list_member_constraint`]), and leaves every other refusal
        // — a single-value member that is absent, a value present but outside a
        // bound — to the framework's own wording, which is accurate for those.
        match refusal {
            Refusal::Invalid(violations) => missing_member_refusal(violations),
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation. This service models exactly ONE operation, and every one
        // of its five input members is `@required` + `@httpQuery`, so the REST
        // router's required-binding check rejects the only route it has whenever one
        // of those query parameters is absent — and the protocol default would then
        // answer 501 `NotImplemented`, claiming nobody implemented the operation when
        // what actually happened is that the request left a required parameter out.
        //
        // A `GET /revocations` that failed to route is therefore a GetRevocationStatus
        // missing required query parameters, and the service states it the way it
        // states any other refusal of the model's own rules: 400 `ValidationException`
        // naming each absent member. Anything else (another path, another method)
        // keeps the framework's protocol-correct default.
        let missing = missing_required_query_parameters(req)?;
        let violations: Vec<Violation> = missing
            .into_iter()
            .map(|member| Violation {
                path: member.to_string(),
                kind: ViolationKind::MissingRequiredMember,
                value: None,
                sensitive: false,
            })
            .collect();
        // Rendered through the generated entry, so this refusal lands in the same
        // envelope as one the boundary validator raises for a value that was
        // present but out of bounds — and through `refusal_response` with this
        // service's own wording (`override_refusal`), so a required member the
        // ROUTER found absent and one the VALIDATOR found absent are stated
        // identically.
        let chosen = self.override_refusal(&Refusal::Invalid(violations.clone()));
        Some(self.entry.refusal_response(
            aws_smithy_schema_server::ProtocolError::Invalid(violations),
            chosen,
            req,
        ))
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

/// This service's endpoint prefix, as the model gives it (`data-signer`) — distinct
/// from its SigV4 signing name (`signer`), which the generated identity carries.
/// `can_handle` claims a request that names either.
pub const ENDPOINT_PREFIX: &str = "data-signer";

/// The error code every refusal of a request's own values carries: the modelled
/// shape that is this service's 400
/// (`smithy_query(query="operation",operation="GetRevocationStatus")` lists
/// `ValidationException` @httpError 400).
const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// Whether the input member `member_name` names is a LIST, read off the generated
/// input schema so the model stays the one source of which members are lists.
fn is_list_member(member_name: &str) -> bool {
    crate::ops::types::GET_REVOCATION_STATUS_INPUT_SCHEMA
        .members()
        .iter()
        .any(|member| {
            member.http_query().map(|query| query.value()) == Some(member_name)
                && matches!(member.shape_type(), aws_smithy_schema::ShapeType::List)
        })
}

/// The constraint a required LIST member violated when the request supplied
/// nothing for it, as a sentence naming the member.
///
/// A list is the case the framework's generic wording gets wrong. On the wire a
/// query-bound list that is ABSENT and one that is EMPTY are the same request —
/// an SDK serializing an empty list emits no parameter at all — so the refusal
/// cannot claim the value was null; what it can state, and what the service
/// states, is that the member must carry at least one element.
pub fn missing_list_member_constraint(member_name: &str) -> String {
    format!("{member_name} must not be null or empty")
}

/// How this service states a refusal whose every violation is a required LIST
/// member the request supplied nothing for: one clause per member, in the order
/// the violations arrive (the model's member order), each naming the member and
/// the constraint it actually violated.
///
/// `None` for every other refusal, which keeps the framework's own wording: a
/// single-value member that is absent really was null, and a value present but
/// outside a declared bound is already described accurately, with the offending
/// value echoed.
fn missing_member_refusal(violations: &[Violation]) -> Option<Refused> {
    if violations.is_empty() {
        return None;
    }
    let mut clauses = Vec::with_capacity(violations.len());
    for violation in violations {
        if violation.kind != ViolationKind::MissingRequiredMember || !is_list_member(&violation.path)
        {
            return None;
        }
        clauses.push(missing_list_member_constraint(&violation.path));
    }
    Some(Refused {
        code: VALIDATION_EXCEPTION_CODE.to_string(),
        message: clauses.join("; "),
    })
}

/// The method of the one route this service's model binds (`GET /revocations`).
const REVOCATIONS_METHOD: &str = "GET";
/// The path of that route, as the single path segment it consists of. Compared the way
/// the router compares it: `route_lookup` trims the path's leading and trailing `/`
/// and splits the rest into segments, so `/revocations`, `/revocations/` and
/// `//revocations//` are all that one segment and anything else is another route.
const REVOCATIONS_PATH_SEGMENT: &str = "revocations";

/// The wire names of `GetRevocationStatus`'s `@httpQuery`-bound input members, read off
/// the generated input schema rather than written out here, so the model stays the one
/// source of the names.
///
/// Every one of them is also `@required`
/// (`smithy_query(query="operation",operation="GetRevocationStatus")` shows all five
/// input members as `required` + `httpQuery`), which is exactly the set the REST
/// router's generated required-binding table holds for this route — so a name here
/// missing from a request is a name the router refused to match the route for.
///
/// `@required` itself cannot be read back at runtime, which is why the filter is on the
/// query binding alone: `aws-smithy-schema` exposes no `required` accessor, and codegen
/// does not carry the trait on a member's generic `TraitMap` either — it CONSUMES
/// `@required` at generation time, to decide whether the Rust field is `Option<T>` and
/// to build `Route::with_required` (`vendor/smithy-macros/src/lib.rs`). The model fact
/// above is therefore enforced by test rather than by the type system: the
/// `a_missing_required_query_parameter_is_a_validation_error` suite asserts a 400 for
/// each of the five members omitted on its own, and a 200 for the complete request.
fn required_query_parameters() -> impl Iterator<Item = &'static str> {
    crate::ops::types::GET_REVOCATION_STATUS_INPUT_SCHEMA
        .members()
        .iter()
        .filter_map(|member| member.http_query().map(|query| query.value()))
}

/// The required query parameters `req` leaves out, when `req` targets this service's
/// one route and leaves any out; `None` when it targets another route, or carries them
/// all (in which case it failed to route for some other reason and the framework's
/// default wording stands).
///
/// A parameter counts as present when its NAME is on the query string, whatever value
/// it carries, and the name is compared as the request spells it. That is precisely the
/// test the router's own required-binding check applies — it reads raw `key[=value]`
/// pairs (`parse_query_pairs`) and asks whether the name occurs
/// (`required_binding_score`, `vendor/smithy-schema-server/src/rest_xml.rs`), with no
/// percent-decoding of names on the routing path — so the two cannot disagree about
/// which parameters a request supplied. A present-but-invalid value is not this
/// function's business: the route then matches and the boundary validator refuses the
/// value itself.
fn missing_required_query_parameters(
    req: &aws_smithy_runtime_api::http::Request,
) -> Option<Vec<&'static str>> {
    if req.method() != REVOCATIONS_METHOD {
        return None;
    }
    // Parsed with `http::Uri` rather than by hand, so an absolute-form URI, an
    // origin-form one, and the userinfo/port/fragment variants all yield the same path
    // and query the router reads. A URI that does not parse is not this route's.
    let uri: http::Uri = req.uri().parse().ok()?;
    if uri.path().trim_matches('/') != REVOCATIONS_PATH_SEGMENT {
        return None;
    }

    let query = uri.query().unwrap_or_default();
    let supplied: Vec<&str> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| pair.split_once('=').map_or(pair, |(name, _)| name))
        .collect();
    let missing: Vec<&'static str> = required_query_parameters()
        .filter(|required| !supplied.contains(required))
        .collect();
    (!missing.is_empty()).then_some(missing)
}
