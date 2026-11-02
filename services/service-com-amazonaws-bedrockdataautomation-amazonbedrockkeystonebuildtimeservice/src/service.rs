// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice` service: how it claims requests and how it words the
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
//! A sixth seam, `override_protocol`, is the one this service DOES take: it serves the
//! model's own `restJson1` codec with every timestamp in the ISO-8601 `date-time` form
//! this model's single timestamp shape declares (see [`date_time_timestamps`]).
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::{Arc, LazyLock};

/// This service's Smithy model: the one source for which of its members are
/// `@sensitive`, and so for which values a boundary refusal states none.
const MODEL_JSON: &str = include_str!(
    "../smithy-model/com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice.json"
);

/// The members of this model whose value a refusal WITHHOLDS, read off [`MODEL_JSON`]
/// rather than listed here, so the decision cannot drift from the model as the model
/// grows a `@sensitive` member.
///
/// This model marks the two resource NAMES (`BlueprintName`,
/// `DataAutomationProjectName`) and the library name beside them, so a request refused
/// for the shape of a name it submitted is told which member and which requirement, and
/// never handed the name back.
static WITHHELD_MEMBERS: LazyLock<service_common::sensitive_members::WithheldMembers> =
    LazyLock::new(|| service_common::sensitive_members::WithheldMembers::from_model(MODEL_JSON));

pub mod date_time_timestamps;

/// The `com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// This service's wire protocol, built ONCE here because
    /// [`Service::override_protocol`] hands back a borrow of it.
    protocol: date_time_timestamps::DateTimeRestJson,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            protocol: date_time_timestamps::DateTimeRestJson::new(),
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
        // This service signs as `bedrock`, the signing name every Bedrock-family service
        // shares while serving its own endpoint (`bedrock-data-automation` here).
        // Claiming on that name alone makes all six claimants for every Bedrock request,
        // which the router answers `500 AmbiguousService` — a code no AWS model carries —
        // rather than serving one of them.
        //
        // So the claim is this model's own path space (`blueprints`,
        // `data-automation-projects`, `data-automation-libraries`,
        // `invokeBlueprintOptimizationAsync`, `getBlueprintOptimizationStatus`), through
        // the one rule `service_common::bedrock` states for the family (which narrows
        // `service_common::claims_request` with the family's shared-segment split). Its
        // three tagging paths (`/tagResource`, `/untagResource`, `/listTagsForResource`)
        // are bound IDENTICALLY by the Bedrock control plane, so nothing in such a request
        // tells the two apart — AWS tells them apart by endpoint host, which is not on
        // the wire here — and the family rule gives that segment to the control plane.
        // `None` for the RPC argument: every operation here is bound to a URI.
        service_common::bedrock::claims_request(
            parts,
            self.definition(),
            crate::ROUTE_PATH_SEGMENTS,
            None,
        )
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
        // required member, or a value outside a declared bound) and leaves the words
        // to the service. This service states them the shared AWS way: the framework's
        // envelope, clauses, member paths and requirements, with two renderings the
        // framework has no way to apply itself —
        //
        // * a `@pattern` requirement is quoted WITHOUT the `^`/`$` a whole-value
        //   pattern is written with, so a caller that lifts the expression out of the
        //   message compiles the expression this service advertises;
        // * a clause about a member the model marks `@sensitive` ([`WITHHELD_MEMBERS`])
        //   states NO value: a refusal does not hand back a value the model says not to
        //   disclose.
        //
        // The code stays `ValidationException`, which is both the modelled client-input
        // error of this service and what the framework already answered, so nothing a
        // client branches on changes — only the words.
        service_common::validation_message::aws_validation_refusal_withholding(refusal, |path| {
            WITHHELD_MEMBERS.withholds_value(path)
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

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model's own restJson1 codec, with the ONE difference the generic
        // renderer cannot express: a timestamp is written in the ISO-8601 date-time
        // form this model's only timestamp shape declares, rather than in the
        // protocol's default epoch-seconds form (see `date_time_timestamps`).
        // Borrowed from the field built in `new`, never constructed per request.
        Some(&self.protocol)
    }
}
