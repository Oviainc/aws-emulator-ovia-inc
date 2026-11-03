// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523` service: how it claims requests and how it words the
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

pub mod request_layer;

/// `refusal` with every enumerated value set stated in LEXICOGRAPHIC order, and everything
/// else — the member paths, the values echoed, every other requirement — untouched.
///
/// A refusal of a value outside an enumerated type names the values that type admits, and
/// the order it names them in is the service's rather than the model's: the order the model
/// happens to declare an enum's members in is an artefact of how the model was written
/// (`RuntimeEnvironment` declares its runtimes in release order), while this service states
/// such a set sorted. The re-ordering itself is `service_common`'s one statement of that
/// rule, and it is applied to the violations as they arrive, so it answers for every enum of
/// this model and for every member targeting one, in any operation, rather than for a table
/// of enums written out here.
fn in_stated_value_order(refusal: &Refusal) -> Refusal {
    let Refusal::Invalid(violations) = refusal else {
        return refusal.clone();
    };
    Refusal::Invalid(
        violations
            .iter()
            .map(service_common::validation_message::in_sorted_enum_order)
            .collect(),
    )
}

/// The `com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// What a request whose body carries a value no modelled timestamp member can be read
    /// from is served by, instead of the operation it named: see
    /// [`request_layer`]. Holds no state, so one value serves every such request.
    unreadable_body: request_layer::UnreadableBody,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            unreadable_body: request_layer::UnreadableBody,
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
        // This service is the 2018-05-23 API version of Kinesis Analytics, and AWS serves
        // it behind the SAME endpoint and the same SigV4 signing name as the 2015-08-14
        // version (`endpointPrefix`/`sigv4Name` `kinesisanalytics` in both models). The
        // router resolves a request's routing key from the credential scope, so both
        // versions see that one key: claiming on the signing name alone makes both of them
        // claim every request, which the router answers `500 AmbiguousService` — a code no
        // AWS model carries — instead of serving either.
        //
        // What names the version is the `awsJson1_1` wire form itself:
        // `X-Amz-Target: KinesisAnalytics_20180523.<Operation>` for this service and
        // `KinesisAnalytics_20150814.<Operation>` for its peer. So the claim is the one
        // rule `service-common` states for an endpoint serving several API versions — the
        // routing key AND a target naming THIS service shape, API-version suffix included,
        // since that suffix is the only difference between the two names.
        service_common::claims_versioned_rpc_request(
            parts,
            self.entry.identity().signing_name(),
            crate::ROUTE_PATH_SEGMENTS,
            self.entry.identity().shape_id(),
        )
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // An `awsJson` target names its operation in the segment after its LAST dot,
        // whatever prefix stands in front of it — and some SDKs write that prefix as the
        // fully qualified Coral name, which carries dots of its own
        // (`com.amazonaws.kinesisanalyticsv2.KinesisAnalytics_20180523.<Operation>`). The
        // framework resolves a routing key from everything after the FIRST dot, so a
        // dotted prefix resolves a key no modelled operation is wired under and the
        // request is refused before any handler runs, even though `can_handle` above
        // (which reads the same target by the same rule) claimed it.
        //
        // So the operation is resolved here through `service_common::rpc_target`, this
        // emulator's one statement of that rule. It is the same answer for the short
        // prefix an SDK normally sends, so one rule serves both forms; a target naming
        // another service, or an operation this model does not bind, resolves to `None`
        // and is left to the generated routing table and its protocol-correct refusal.
        let operation =
            service_common::rpc_target::requested_operation(req, self.entry.identity().shape_id())?;

        // Deserialization precedes every later layer, so a value the protocol cannot read
        // into the modelled member it is bound to is refused BEFORE the operation runs and
        // before anything is mutated: an `epoch-seconds` timestamp carrying a quoted number
        // is such a value, and `crate::service::request_layer` states that rule for every
        // timestamp member of this model.
        if request_layer::unreadable_timestamp(operation, request_layer::request_body(req))
            .is_some()
        {
            return Some(&self.unreadable_body);
        }

        self.entry.routing_table().get(operation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound or an enumerated set)
        // before any handler runs. This service is `awsJson1_1`, where AWS answers such a
        // refusal as `400 ValidationException` with the `N validation error(s) detected:
        // …` envelope — the envelope, the member paths and the requirement sentences the
        // framework already renders, which is why this delegates to the ONE shared
        // statement of them (`service_common::validation_message`) instead of writing a
        // second one here. The `@pattern` rendering passed to it is the identity, so a
        // pattern is quoted back as this model writes it, anchors included.
        //
        // What this service chooses is the ORDER an enumerated value set is stated in
        // ([`in_stated_value_order`]), which is not the order the model declares it in.
        service_common::validation_message::aws_validation_refusal_with(
            &in_stated_value_order(refusal),
            |pattern| pattern.to_string(),
        )
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
