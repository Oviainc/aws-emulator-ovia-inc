// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-lambda-awsgirapiservice` service: how it claims requests and how it words the
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

use service_common::shared_path::SharedPathArn;
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The order this service states one enum's value set in, one row per enum.
///
/// The value SET is the model's and the CHECK is the framework's; only the order the
/// sentence lists the set in is chosen here, because no model records it: AWS states a
/// value set in neither the model's declaration order nor ascending order, so each row
/// is RECORDED from an AWS answer rather than computed. Source for the one row below:
/// the answer the corpus unit `lambda.put-runtime-management-config-invalid-mode-rejected`
/// carries for an `UpdateRuntimeOn` outside its value set.
///
/// A row is matched by its value SET rather than by the member the clause names (see
/// [`reported_enum_order`]), because the order is a property of the enum SHAPE: every
/// member targeting that enum, in any operation, is stated alike. An enum with no row —
/// every other enum this model declares — keeps the model's declaration order, which is
/// the only order there is evidence for.
const REPORTED_ENUM_ORDERS: &[&[&str]] = &[
    // com.amazonaws.lambda#UpdateRuntimeOn (`Auto | Manual | FunctionUpdate` in the
    // model), reported by AWS as:
    &["FunctionUpdate", "Manual", "Auto"],
];

/// `allowed` in the order this service reports that enum in, or in the order the model
/// declares it (the order it arrived in) when there is no recorded order for it — the
/// only order there is evidence for.
fn reported_enum_order(allowed: &[String]) -> Vec<String> {
    match REPORTED_ENUM_ORDERS
        .iter()
        .find(|reported| is_same_set(reported, allowed))
    {
        Some(reported) => reported.iter().map(|value| (*value).to_string()).collect(),
        None => allowed.to_vec(),
    }
}

/// Whether two value sets hold the same values. An enum's values are unique by
/// definition, so equal lengths plus containment is set equality.
fn is_same_set(reported: &[&str], allowed: &[String]) -> bool {
    reported.len() == allowed.len()
        && reported
            .iter()
            .all(|value| allowed.iter().any(|candidate| candidate == value))
}

/// The literal first URI path segments that LEAD to the `{Resource}` ARN label of the
/// one path space this model and a peer signing as `lambda` bind IDENTICALLY:
/// `ListTags` (`GET`), `TagResource` (`POST`) and `UntagResource` (`DELETE`) are all
/// bound to `/2017-03-31/tags/{Resource}` here AND in the Lambda MicroVMs model, so the
/// model-derived path discriminator alone cannot separate the two there. Source:
/// `smithy_query(query="digest", …)` on both models.
const SHARED_TAGGING_PATH: &[&str] = &["2017-03-31", "tags"];

/// The `aws.api#service$arnNamespace` every service signing as `lambda` shares
/// (`smithy_query(query="service_info")`: `arnNamespace: lambda` for all four), and so
/// the only service field an ARN any of their tagging operations addresses can carry.
const SHARED_ARN_NAMESPACE: &str = "lambda";

/// The ARN resource types reachable on [`SHARED_TAGGING_PATH`] that belong to a PEER
/// service rather than to this one.
///
/// A resource belongs to the service whose own operations MINT it: `microvm-image` is
/// minted by `CreateMicrovmImage` in the Lambda MicroVMs service, whose model binds the
/// same tagging URIs and whose `TaggableResource` pattern accepts it (this model's does
/// not). Every other type that pattern accepts here — `function`, `layer`,
/// `code-signing-config`, `event-source-mapping`, `capacity-provider`,
/// `network-connector` — is minted by this
/// service or has no other claimant, so it stays with this endpoint, whose own identity
/// IS the shared signing name (`sdkId: Lambda`, `endpointPrefix: lambda`). Source:
/// `smithy_query(query="shape", shape="TaggableResource")` on both models.
const PEER_TAGGING_RESOURCE_TYPES: &[&str] = &["microvm-image"];

/// Whether this service claims a request on the `/2017-03-31/tags/{Resource}` path space
/// it shares with the Lambda MicroVMs service.
///
/// The ARN in the path names the resource, and a resource belongs to exactly one of the
/// two endpoints, so the resource type decides: this endpoint claims every tagging
/// request EXCEPT one whose ARN names a peer's resource. Everything else on the shared
/// API-date segment stays here — a label that names no recognisable `lambda` resource, and
/// a path under that segment neither model binds — because this is the service the shared
/// signing name belongs to, so exactly one service claims every request on it.
fn claims_shared_tagging_path(parts: &http::request::Parts) -> bool {
    match service_common::shared_path::shared_path_arn(
        parts.uri.path(),
        SHARED_TAGGING_PATH,
        SHARED_ARN_NAMESPACE,
    ) {
        SharedPathArn::NotShared | SharedPathArn::Unrecognised => true,
        SharedPathArn::Resource(kind) => !PEER_TAGGING_RESOURCE_TYPES.contains(&kind.as_str()),
    }
}

/// The `com-amazonaws-lambda-awsgirapiservice` emulated service: the generated [`ServiceDefinition`] plus
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
        // This service signs as `lambda` — a signing name it SHARES with every other
        // emulated service in the Lambda family (Lambda Core, Lambda MicroVMs and
        // Lambda Web; `smithy_query(query="service_info")` gives all four
        // `sigv4Name: lambda`). The router resolves a request's routing key from the
        // SigV4 credential scope first, so all four see the key `lambda`: claiming on
        // the signing name alone makes each one claim the others' traffic, which the
        // router answers `500 AmbiguousService` instead of serving any of them —
        // leaving every Lambda operation unreachable.
        //
        // So the claim is narrowed to this service's own request space, by the one rule
        // `service-common` states for a shared signing name: this model is `restJson1`
        // (a REST protocol, so the `rpc_service_shape_id` argument is `None`), and a
        // REST service's own surface is the modelled URI path space its generated
        // `ROUTE_PATH_SEGMENTS` publishes — here the API-date segments this model binds
        // (`2015-03-31`, `2018-10-31`, …), which are disjoint from the peers' own
        // (`2026-04-04`, `2025-09-09`, `2025-03-07`) except for the shared tagging path
        // `/2017-03-31/tags/{Resource}`, where the ARN decides (see
        // [`claims_shared_tagging_path`]).
        service_common::claims_request(
            parts,
            self.entry.identity().signing_name(),
            crate::ROUTE_PATH_SEGMENTS,
            None,
        ) && claims_shared_tagging_path(parts)
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
        // reword it. Everything about the envelope — the `N validation error(s)
        // detected:` prefix, the member paths, the echoed value and every requirement
        // sentence — is the framework's, so the two refusal paths cannot drift.
        //
        // TWO renderings are stated here, neither of which a model records:
        //
        // * the ORDER of an enum's value set (see [`reported_enum_order`]), which is
        //   this service's own and is applied below;
        // * how a `@pattern` requirement quotes its expression. AWS states it WITHOUT
        //   the `^`/`$` anchors a whole-value Smithy pattern wraps it in, so a
        //   `TaggableResource` the tagging URIs refuse is reported as
        //   `Member must satisfy regular expression pattern: arn:(aws[a-zA-Z-]*):lambda:…`
        //   rather than `^arn:…$`. That wording is shared by every service on a JSON
        //   protocol, so it lives in `service-common` and is delegated to rather than
        //   restated — the CHECK still uses the model's pattern as written, and only
        //   the rendering drops the pair.
        let Refusal::Invalid(violations) = refusal else {
            // An unauthenticated caller keeps the framework's 403 code and wording.
            return None;
        };
        let restated: Vec<Violation> = violations
            .iter()
            .map(|violation| match &violation.kind {
                ViolationKind::EnumValue(allowed) => Violation {
                    path: violation.path.clone(),
                    kind: ViolationKind::EnumValue(reported_enum_order(allowed)),
                    value: violation.value.clone(),
                },
                _ => violation.clone(),
            })
            .collect();
        // The code, the envelope and the `@pattern` rendering are `service-common`'s;
        // the value set order above is this service's.
        service_common::aws_validation_refusal(&Refusal::Invalid(restated))
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
