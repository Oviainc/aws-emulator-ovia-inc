// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-forecastquery-amazonforecastruntime` service: how it claims requests and how it words the
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

use crate::state::ResourceKind;

// This service's RESOURCE IDENTIFIERS — the ARN each operation resolves a stored forecast
// by — are [`crate::state::RESOURCE_KINDS`]: `QueryForecast$ForecastArn` (target `Arn`,
// length 0..256) and `QueryWhatIfForecast$WhatIfForecastArn` (target `LongArn`, length
// 0..300), both carrying the pattern `^arn:([a-z\d-]+):forecast:.*:.*:.+$`. They are the
// only ARN-typed members the model declares, and a refusal names a member in lowerCamel
// (`ResourceKind::member_path`, which goes through
// `aws_smithy_schema_server::constraints::path::member`).
//
// Source: `smithy_query(query="closure",operation="QueryForecast,QueryWhatIfForecast",
// model_path="services/service-com-amazonaws-forecastquery-amazonforecastruntime/\
// smithy-model/com-amazonaws-forecastquery-amazonforecastruntime.json")`.

/// The service key `::emulator_server::service_from_target` reads out of the
/// `X-Amz-Target` header this service's awsJson1_1 operations carry: that reader takes
/// the segment before the `.`, strips any trailing `_<version>` suffix and lower-cases
/// the result, so `AmazonForecastRuntime.<OperationName>` (`OPERATIONS.md`, "Request
/// routing") normalises to this key.
///
/// The Amazon Forecast CONTROL plane holds the mirror image of this constant, in
/// `services/service-com-amazonaws-forecast-amazonforecast/src/service.rs`, where it is
/// `amazonforecast`. The two decide ONE split of the shared `forecast` signing name, so
/// change them together.
const TARGET_SERVICE_KEY: &str = "amazonforecastruntime";

/// The modelled error an ARN that is not a Forecast ARN is refused with:
/// `InvalidInputException` (HTTP 400), whose documented meaning is "The value is invalid
/// or is too long". It is a shape BOTH operations declare, so a client's
/// `except InvalidInputException` matches it.
const INVALID_INPUT_EXCEPTION: &str = "InvalidInputException";

/// The first violation, if any, that says a resource ARN is not an ARN of this service —
/// its modelled `@pattern`, or the modelled maximum length an ARN too long to be one
/// exceeds — together with the resource kind that member resolves.
///
/// ONE selection, used both to DECIDE that this is an ARN refusal and to WORD it, so the
/// code a caller branches on and the sentence it reads can never come from different
/// violations. The kind travels with it because the wording names the resource that was
/// asked for, not the constraint that failed.
///
/// ANY such violation is enough, and it wins over the rest: both operations resolve the
/// resource their ARN names before anything else about the request is judged, so an
/// unreadable ARN is what the caller is told about even when another member is also
/// wrong. Every refusal that does NOT concern the resource ARN (an absent required
/// member, a bound on `Filters` or `NextToken`) keeps the framework's generic
/// request-validation wording: those are request-SHAPE constraints, reported by the
/// validation layer in front of the service rather than by the service's own resolution
/// of the resource. A `Filters` KEY is such a constraint too, but one the boundary walk
/// does not reach, so each handler reports it itself
/// (`crate::state::shape_validation_message`) in the same shape-refusal envelope.
///
/// A violation with no value to echo cannot be worded this way and is left alone: this
/// message is built from the value the request supplied.
fn resource_arn_violation(violations: &[Violation]) -> Option<(&Violation, ResourceKind)> {
    violations.iter().find_map(|violation| {
        if violation.value.is_none()
            || !matches!(
                violation.kind,
                ViolationKind::Pattern(_) | ViolationKind::MaxLength(_)
            )
        {
            return None;
        }
        crate::state::RESOURCE_KINDS
            .into_iter()
            .find(|kind| kind.member_path() == violation.path)
            .map(|kind| (violation, kind))
    })
}

/// The `com-amazonaws-forecastquery-amazonforecastruntime` emulated service: the generated [`ServiceDefinition`] plus
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
        // TWO services sit behind the ONE SigV4 signing name `forecast`: the Amazon
        // Forecast control plane (sdkId `forecast`, endpoint `forecast`, target prefix
        // `AmazonForecast`) and this query/runtime data plane (sdkId `forecastquery`,
        // endpoint `forecastquery`, target prefix `AmazonForecastRuntime`). The model
        // says so itself — `aws.auth#sigv4` names `forecast` here while
        // `aws.api#service` gives `endpointPrefix: forecastquery` — and no Smithy trait
        // expresses the sharing. The router's routing key comes from the SigV4
        // credential scope first, so both services resolve to `forecast`: matching on
        // the signing name ALONE makes each one claim the other's traffic, the router
        // finds two claimants and refuses every Forecast-signed request with
        // `500 AmbiguousService` — so NEITHER service is reachable.
        //
        // So the claim is narrowed to this service's own modelled wire surface, by the
        // rule every service under a shared signing name applies: the signing name must
        // match AND the request must address THIS service. This model is `awsJson1_1`,
        // an RPC protocol: every operation posts to `/` — the path its control-plane
        // peer posts to as well, so a path test cannot separate them — and names its
        // target service out of band, in `X-Amz-Target:
        // AmazonForecastRuntime.<OperationName>`. That target is therefore the
        // discriminator, read through the framework's OWN reader so the parsing rule
        // (strip the operation, strip a trailing `_<version>`, lower-case) stays in one
        // place — the same rule DynamoDB uses against DynamoDB Streams and Amazon
        // Personalize against its two data planes, and exactly how real AWS tells
        // `forecastquery` traffic from `forecast` traffic.
        //
        // A request carrying no such target names no operation of this awsJson1_1
        // service, so it is not this service's traffic.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) if key == self.entry.identity().signing_name() => {
                ::emulator_server::service_from_target(&parts.headers).as_deref()
                    == Some(TARGET_SERVICE_KEY)
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
        // required member, or a value outside a declared bound) to let this service
        // reword it.
        //
        // ONE class is reworded, and it replaces BOTH the code and the message: a
        // resource ARN that is not a Forecast ARN. Each operation reads the ARN it is
        // called with before it resolves (or fails to resolve) the forecast it names, so
        // an ARN the service cannot read is answered with the service's OWN modelled
        // input error — `InvalidInputException`, HTTP 400 — and not with a generic code
        // this model does not declare. That is the code a client branches on
        // (`except fq.exceptions.InvalidInputException`), and the one that keeps the
        // documented precedence observable: ARN syntax is refused BEFORE resource
        // resolution, which answers `ResourceNotFoundException` for an ARN that is
        // well-formed but names nothing.
        //
        // The MESSAGE is the service's own rule for an ARN it cannot read as one of the
        // kind the operation queries — `Invalid ARN for <resource type>: <the value
        // supplied>` ([`crate::state::ResourceKind::invalid_arn_message`]) — the same
        // sentence the handler emits for an ARN that satisfies the member's constraints
        // yet names no resource of that kind. The two paths differ only in WHERE the
        // value is read, never in what the caller is told, so a client cannot tell a
        // boundary refusal from a handler rejection.
        //
        // Every other refusal keeps the framework's wording: an absent required member
        // and a bound on another member are request-SHAPE constraints, reported by the
        // validation layer in front of the service.
        match refusal {
            Refusal::Invalid(violations) => {
                resource_arn_violation(violations).map(|(violation, kind)| Refused {
                    code: INVALID_INPUT_EXCEPTION.to_string(),
                    // `Some`, because `resource_arn_violation` selects only a violation
                    // that carries the value it refused.
                    message: kind.invalid_arn_message(violation.value.as_deref().unwrap_or_default()),
                })
            }
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
