// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions` service: how it claims requests and how it words the
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

/// How this service words a request the framework refused before any handler ran: the
/// tag APIs' own error taxonomy, and the AWS constraint envelope for everything else.
pub mod validation;

/// The one authorization pre-condition the tag APIs apply to the `resourceArn` they are
/// addressed with, ahead of every input check and every lookup.
pub mod authorization;

/// The HTTP method of the one route of this service whose model requires a query
/// parameter: `UntagResource` is `DELETE /tags/{resourceArn}`
/// (`smithy_query(query="operation",operation="UntagResource")`).
const TAG_REMOVAL_METHOD: &str = "DELETE";

/// The first and only literal segment of the tag APIs' shared URI, `/tags/{resourceArn}`.
/// Compared the way the router compares it: `route_lookup` trims the path's leading and
/// trailing `/` and splits the rest into segments, so `/tags/<arn>` and `/tags/<arn>/`
/// are the same two segments and anything else is another route.
const TAG_PATH_SEGMENT: &str = "tags";

/// How many segments that URI has: the literal above plus the one `@httpLabel`.
const TAG_PATH_SEGMENTS: usize = 2;

/// The `@httpQuery`-bound input members of `UntagResource`, as `(member path, parameter
/// name)`, read off the generated input schema rather than written out here so the model
/// stays the one source of both names.
///
/// `tagKeys` is the only one, and it is `@required`
/// (`smithy_query(query="operation",operation="UntagResource")`), which is exactly the
/// set the REST router's generated required-binding table holds for this route — so a
/// name here absent from a request is a name the router refused to match the route for.
/// `@required` itself cannot be read back at runtime (codegen consumes the trait), so
/// that model fact is held by this crate's suites rather than by the type system.
///
/// `validation` reads the same mapping, so the sentence naming the parameters a request
/// has to add and the routing check that finds them absent cannot drift: both are this
/// one derivation from the model.
pub(crate) fn tag_request_parameters() -> impl Iterator<Item = (String, &'static str)> {
    crate::ops::types::UNTAG_RESOURCE_INPUT_SCHEMA
        .members()
        .iter()
        .filter_map(|member| {
            let parameter = member.http_query()?.value();
            let path = aws_smithy_schema_server::constraints::path::member(member.member_name()?);
            Some((path, parameter))
        })
}

/// The required query parameters of the tag-removal route that `req` leaves out, as the
/// violations the boundary validator would have raised for them — or `None` when `req`
/// addresses another route, or carries them all (in which case it failed to route for
/// some other reason and the framework's default stands).
///
/// A parameter counts as present when its NAME is on the query string, whatever value it
/// carries, which is precisely the test the router's own required-binding check applies
/// (`required_binding_score`, `vendor/smithy-schema-server/src/rest_xml.rs`): it reads
/// raw `key[=value]` pairs and asks whether the name occurs, with no percent-decoding of
/// names, so the two cannot disagree about what a request supplied. A parameter that IS
/// present but holds a value outside its constraints is not this function's business: the
/// route then matches and the boundary validator refuses the value itself.
fn absent_tag_removal_parameters(
    req: &aws_smithy_runtime_api::http::Request,
) -> Option<Vec<Violation>> {
    if req.method() != TAG_REMOVAL_METHOD {
        return None;
    }
    // Parsed with `http::Uri` rather than by hand, so an absolute-form URI, an
    // origin-form one, and the userinfo/port/fragment variants all yield the same path
    // and query the router reads. A URI that does not parse is not this route's.
    let uri: http::Uri = req.uri().parse().ok()?;
    // Segmented exactly as `route_lookup` segments it: the leading and trailing `/` are
    // trimmed and the rest is split, with an INTERIOR empty segment kept — so a path the
    // router matches to no route (`/tags//<arn>`, three segments to it) is not read here
    // as the two-segment tag route either.
    let trimmed = uri.path().trim_matches('/');
    let segments: Vec<&str> = if trimmed.is_empty() {
        Vec::new()
    } else {
        trimmed.split('/').collect()
    };
    if segments.len() != TAG_PATH_SEGMENTS || segments[0] != TAG_PATH_SEGMENT {
        return None;
    }

    let query = uri.query().unwrap_or_default();
    let supplied: Vec<&str> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| pair.split_once('=').map_or(pair, |(name, _)| name))
        .collect();
    let absent: Vec<Violation> = tag_request_parameters()
        .filter(|(_, parameter)| !supplied.contains(parameter))
        .map(|(path, _)| Violation {
            path,
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        })
        .collect();
    (!absent.is_empty()).then_some(absent)
}

/// The `com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions` emulated service: the generated [`ServiceDefinition`] plus
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
        // required member, or a value outside a declared bound) to let this service
        // reword it. This service does reword every input-constraint refusal: the
        // rules, and the one formatter that applies them, live in `validation`, so no
        // operation words a refusal its own way. Both the code and the message are
        // replaced, because a client branches on the code — the tagging surface's
        // refusals name `BadRequestException` rather than `ValidationException`.
        validation::refused(refusal)
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no modeled
        // operation. One class of request lands here that this service DOES serve: the
        // REST router discriminates the operations bound to one URI by the required
        // header/query parameters each model declares, so `DELETE /tags/{resourceArn}`
        // carrying no `tagKeys` parameter matches no route at all — and the protocol
        // default would answer 501 `NotImplemented`, claiming nobody implemented the
        // removal, when what happened is that the request left a required parameter out.
        // That discrimination exists to pick BETWEEN operations bound to one route; it
        // cannot un-name the only operation a route has.
        //
        // Such a request is therefore stated as the refusal it is, through the same
        // wording every other refusal of this surface goes through
        // (`override_refusal` -> `validation`): the tagging surface's own
        // `BadRequestException`, naming the parameters the caller has to add. Anything
        // else — another path, another method — keeps the framework's protocol-correct
        // default.
        let violations = absent_tag_removal_parameters(req)?;
        // Rendered through the generated entry, so this refusal lands in the same
        // envelope as one the boundary validator raises for a parameter that was present
        // but out of bounds — and with this service's own wording, so a required
        // parameter the ROUTER found absent and one the VALIDATOR found absent are
        // stated identically.
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
