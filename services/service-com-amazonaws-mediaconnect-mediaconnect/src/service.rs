// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-mediaconnect-mediaconnect` service: how it claims requests and how it words the
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
//!   correctly. This service names ONE class here: a request that addressed a route of
//!   this service and which the REST router matched to no operation only because a
//!   `@required` query parameter of that route was absent (see
//!   `REQUIRED_QUERY_OPERATION_INPUTS` and
//!   `EmulatedService::operation_addressed_without_a_required_parameter`). Every request
//!   the model's routing key does name resolves through the generated routing table.
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

/// The modelled member name a violation path ends in: `sources.1.member.protocol` ->
/// `Protocol`.
///
/// A violation path spells each member in lowerCamel — "a member names itself in
/// lowerCamel. That casing belongs to the path, not to the model"
/// (`aws_smithy_schema_server::constraints::path::member`) — so the modelled member name is
/// the last segment with its initial capitalised, the exact inverse of that spelling for
/// the names this model uses. `None` for a path that ends in no member name at all (an
/// empty path, or one ending in the collection step), where there is nothing to name.
///
/// (`service-com-amazonaws-servicequotas-servicequotasv20190624`'s `service.rs` recovers
/// the same thing for ITS service by walking that operation's schema members instead; the
/// two stay per-service until there is a shared home for one of them, because neither
/// crate may add a module to the other.)
///
/// TOTAL for every `path`: `rsplit` always yields a first item, the filter rejects the
/// cases with nothing to name, and the re-casing walks `chars()` rather than indexing or
/// slicing bytes — so no byte offset, `unwrap`/`expect`, parse or arithmetic is reachable,
/// whatever the path holds (a caller-chosen map key is one of its segments).
fn modelled_member_name(path: &str) -> Option<String> {
    let segment = path.rsplit('.').next().filter(|segment| {
        !segment.is_empty()
            && segment != &"member"
            && segment.starts_with(|c: char| c.is_ascii_alphabetic())
    })?;
    let mut name = String::with_capacity(segment.len());
    for (index, c) in segment.chars().enumerate() {
        if index == 0 {
            name.extend(c.to_uppercase());
        } else {
            name.push(c);
        }
    }
    Some(name)
}

/// How this service states ONE violation the framework's constraint walk found, as the
/// detail of its `Validation failed:` envelope.
///
/// A value outside a modelled value set is stated the way MediaConnect states it — `Invalid
/// <Member>`, naming the member the request carried the value in and nothing else, which is
/// what independent recordings of this service show for an enum-typed member. The member
/// name comes from the violation's own path, so it is derived from the request: a value
/// outside `Protocol`'s set reads `Invalid Protocol` wherever in the request it sat.
///
/// Every other violation kind keeps the framework's own sentence, which names the member,
/// the value and the rule it broke: MediaConnect's own wording for those is not known, and
/// a sentence that states the rule is more use to a caller than a guessed one.
///
/// TOTAL for every violation: `modelled_member_name` is total, `Violation::message` is the
/// framework's own rendering (which elides the echoed value), and the only other step is a
/// `format!`.
fn violation_detail(violation: &Violation) -> String {
    match &violation.kind {
        ViolationKind::EnumValue(_) => match modelled_member_name(&violation.path) {
            Some(member) => format!("Invalid {member}"),
            None => violation.message(),
        },
        _ => violation.message(),
    }
}

/// The operations of this service whose input binds a `@required` member to the query
/// string, each paired with the generated input schema that carries its `@httpQuery`
/// bindings.
///
/// Source: the five such operations this model declares — `BatchGetRouterInput`,
/// `BatchGetRouterNetworkInterface` and `BatchGetRouterOutput` (each binding `Arns` to
/// `arns`), and `UntagResource` and `UntagGlobalResource` (each binding `TagKeys` to
/// `tagKeys`), per
/// `smithy_query(query="operation",operation="BatchGetRouterInput,BatchGetRouterNetworkInterface,\
/// BatchGetRouterOutput,UntagResource,UntagGlobalResource",\
/// model_path="services/service-com-amazonaws-mediaconnect-mediaconnect/smithy-model/\
/// com-amazonaws-mediaconnect-mediaconnect.json")`. The model binds no member to a header
/// at all, so the query string is the whole of this class here.
///
/// Only the operation NAME — the key the generated routing table holds it under — is
/// written out: not a method, not a path, not one parameter name. Those come off that
/// operation's own input schema ([`aws_smithy_schema::Schema::members`]) and from the
/// router itself, so the model stays the one source of what a route is and which
/// parameters it binds. `@required` cannot be read back at runtime (codegen consumes the
/// trait), so WHICH members carry it is a model fact this crate's suite re-derives from
/// the model rather than one the type system holds — see
/// `tests/all/required_query_parameters.rs`, which walks the model and drives every
/// operation it finds.
const REQUIRED_QUERY_OPERATION_INPUTS: &[(&str, &::aws_smithy_schema::Schema)] = &[
    (
        "BatchGetRouterInput",
        crate::ops::types::BATCH_GET_ROUTER_INPUT_INPUT_SCHEMA,
    ),
    (
        "BatchGetRouterNetworkInterface",
        crate::ops::types::BATCH_GET_ROUTER_NETWORK_INTERFACE_INPUT_SCHEMA,
    ),
    (
        "BatchGetRouterOutput",
        crate::ops::types::BATCH_GET_ROUTER_OUTPUT_INPUT_SCHEMA,
    ),
    (
        "UntagGlobalResource",
        crate::ops::types::UNTAG_GLOBAL_RESOURCE_INPUT_SCHEMA,
    ),
    (
        "UntagResource",
        crate::ops::types::UNTAG_RESOURCE_INPUT_SCHEMA,
    ),
];

/// `req` with every `@httpQuery` parameter `input` binds that `req` does not already carry
/// ADDED to its query string — or `None` when it carries them all, so there is nothing to
/// add, and when its target cannot be read as a URI.
///
/// The added parameters carry no value, because the router's required-binding check asks
/// only whether the parameter's NAME occurs on the query string (`required_binding_score`,
/// `vendor/smithy-schema-server/src/rest_xml.rs`, which reads raw `key[=value]` pairs and
/// compares names): a bare name is exactly "supplied" to it. Everything else about the
/// request — its method, its path, its headers — is carried over unchanged, so the route
/// this answers for is the route the request addressed.
///
/// The result is a PROBE, asked of the router and then dropped: it is never deserialized
/// and never served, so the values the added names do not carry are never read. The
/// request the operation is then run against is the caller's own, which still leaves its
/// required parameter out — and is refused for exactly that.
///
/// Parsed with `http::Uri` rather than by hand, so an absolute-form target, an
/// origin-form one, and the userinfo/port/fragment variants all yield the path and query
/// the router itself reads.
fn with_query_parameters_supplied(
    req: &aws_smithy_runtime_api::http::Request,
    input: &::aws_smithy_schema::Schema,
) -> Option<aws_smithy_runtime_api::http::Request> {
    let uri = req.uri().parse::<http::Uri>().ok()?;
    let path_and_query = uri.path_and_query()?;
    let carried = path_and_query.query().unwrap_or_default();
    // Split the way the router splits it: raw pairs, the name being everything before the
    // first `=`, so the two cannot disagree about what a request supplied.
    let supplied: Vec<&str> = carried
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| pair.split_once('=').map_or(pair, |(name, _)| name))
        .collect();

    let mut query = carried.to_string();
    let mut added = false;
    for member in input.members() {
        let Some(parameter) = member.http_query().map(|binding| binding.value()) else {
            continue;
        };
        if supplied.contains(&parameter) {
            continue;
        }
        if !query.is_empty() {
            query.push('&');
        }
        query.push_str(parameter);
        added = true;
    }
    if !added {
        return None;
    }

    let mut probe =
        aws_smithy_runtime_api::http::Request::new(::aws_smithy_types::body::SdkBody::empty());
    probe
        .set_uri(format!("{}?{query}", path_and_query.path()).as_str())
        .ok()?;
    probe.set_method(req.method()).ok()?;
    for (name, value) in req.headers().iter() {
        // `append`, not `insert`: a repeated or multi-valued header keeps every value, so a
        // route discriminated by a header reads the same on the probe as on `req`. The
        // append is FALLIBLE: a header the HTTP layer cannot represent is dropped rather
        // than turned into a panic on a request path.
        let _ = probe
            .headers_mut()
            .try_append(name.to_string(), value.to_string());
    }
    Some(probe)
}

/// The `com-amazonaws-mediaconnect-mediaconnect` emulated service: the generated [`ServiceDefinition`] plus
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

    /// The operation a request that named NO modelled operation nevertheless ADDRESSED,
    /// when the only thing between it and that operation is a `@required` query parameter
    /// it left out — or `None` when it addressed no route of this service.
    ///
    /// The REST router matches a route only for a request carrying every `@required`
    /// header/query binding the operation declares, because that is how REST services tell
    /// apart operations overloaded on ONE method + URI template. That discrimination picks
    /// BETWEEN operations; it cannot un-name the only operation a route has. So a request
    /// that leaves such a parameter out resolves to NO operation, and the protocol default
    /// for that is 501 `NotImplemented` — which says nobody implemented the operation,
    /// when what actually happened is that the request left a required parameter out.
    ///
    /// Naming the operation here hands such a request to the operation it addressed, so
    /// the centralized walk's own boundary validator refuses it with this service's
    /// `BadRequestException` (400) naming the absent member — the same answer, through the
    /// same envelope (`override_refusal`), that a required member absent from a BODY
    /// already gets.
    ///
    /// Which route a request addressed is answered by the ROUTER rather than by matching
    /// paths here: each candidate is asked of `routing_key` with that candidate's own
    /// query parameters supplied ([`with_query_parameters_supplied`]), and a candidate
    /// counts only when the router then resolves the candidate itself — so the method, the
    /// path labels and the route precedence are all the router's own, and a request
    /// addressing nothing this service routes keeps the framework's answer.
    fn operation_addressed_without_a_required_parameter(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&'static str> {
        REQUIRED_QUERY_OPERATION_INPUTS
            .iter()
            .find_map(|(operation, input)| {
                let probe = with_query_parameters_supplied(req, input)?;
                let resolved = self.entry.protocol().routing_key(&probe).ok()?;
                (resolved == *operation).then_some(*operation)
            })
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
        //
        // ONE class of request is named here: the generated table resolved nothing for
        // it, yet it ADDRESSED a route of this service and the only thing in the way was
        // a `@required` query parameter it left out (see
        // [`EmulatedService::operation_addressed_without_a_required_parameter`]). Handing
        // it to the operation it addressed is what lets the centralized walk refuse it as
        // the client error it is — this service's `BadRequestException` (400) naming the
        // absent member — instead of the protocol's routing default claiming, with 501,
        // that the operation is not implemented.
        //
        // Every request the model's own routing key DOES name is left to the generated
        // table, so nothing that routes today routes differently.
        match self.entry.protocol().routing_key(req) {
            Ok(_) => None,
            Err(_) => {
                let operation = self.operation_addressed_without_a_required_parameter(req)?;
                self.entry.routing_table().get(operation)
            }
        }
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        match refusal {
            // MediaConnect declares no `ValidationException`. Its client-input error is
            // `BadRequestException` (HTTP 400) — "thrown if the request contains a
            // semantic error. The precise meaning depends on the API, and is documented
            // in the error message" — and every operation of this service lists it. So a
            // request the framework refuses before any handler runs (a required member
            // absent, a value outside a declared bound, an ARN failing its modelled
            // pattern) is worded with the code a MediaConnect client branches on, and the
            // message states each violation the framework found.
            //
            // The message goes through `crate::state::validation_failed_message`, the one
            // envelope this service words a refused request in: a boundary refusal and a
            // refusal a handler raises itself must not reach a client in two different
            // forms, because the `Validation failed:` prefix is what a client and a
            // log-based alert tell a validation refusal from a semantic one by. Each
            // violation inside that envelope is stated by `violation_detail`.
            Refusal::Invalid(violations) => {
                let detail = violations
                    .iter()
                    .map(violation_detail)
                    .collect::<Vec<String>>()
                    .join("; ");
                let detail = if detail.is_empty() {
                    "The request contains a semantic error.".to_string()
                } else {
                    detail
                };
                Some(Refused {
                    code: "BadRequestException".to_string(),
                    message: crate::state::validation_failed_message(&detail),
                })
            }
            // An unauthenticated caller keeps the framework's 403 wording.
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
