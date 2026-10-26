// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-cloudwatch-graniteserviceversion20100801` service: how it claims requests and how it words the
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
//! - `override_operation` — EDITED HERE: not a re-route. The key and the handler stay the
//!   generated routing table's, but `PutMetricData` is served through
//!   [`ReportingDroppedEntities`], so an accepted call whose entity this service could not
//!   hold says so in its response header; every other key falls through to that table.
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
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, Service,
};
use std::sync::Arc;

/// Run `work` without occupying an async worker.
///
/// The alarm-evaluation pass ([`crate::state::alarm_actions::settle_alarms`]) is CPU-bound
/// and holds this service's store lock for its whole span, so it must not run on the thread
/// the runtime serves other requests with. `block_in_place` moves the current task off that
/// worker, which keeps the runtime serving while the pass runs; it is available only on a
/// multi-thread runtime — the one the emulator boots — so on a current-thread runtime (a
/// `#[tokio::test]`, or a caller driving the router in-process) the work runs inline, where
/// there is no other worker to protect.
pub(crate) fn off_the_async_worker<T>(work: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current().map(|handle| handle.runtime_flavor()) {
        Ok(tokio::runtime::RuntimeFlavor::MultiThread) => tokio::task::block_in_place(work),
        _ => work(),
    }
}

/// The `com-amazonaws-cloudwatch-graniteserviceversion20100801` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// `PutMetricData`, the one operation whose answer carries something the response body
    /// cannot: built from the SAME shared engine as the generated table's, and keyed by the
    /// routing key the protocol resolves, so the operation served is the modelled one.
    reporting_dropped_entities: OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut reporting_dropped_entities = OperationRoutingTable::new();
        reporting_dropped_entities.add_operation(
            "PutMetricData",
            Box::new(ReportingDroppedEntities::new(
                crate::ops::put_metric_data::PutMetricData::new(Arc::clone(&engine)),
            )) as Box<dyn ErasedOperation>,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            reporting_dropped_entities,
        }
    }
}

/// The response header an ACCEPTED `PutMetricData` reports a dropped entity in.
///
/// `PutMetricDataInput$StrictEntityValidation` documents that such an answer carries "an
/// additional response header [which] will contain information about the validation errors"
/// and enumerates the values that information is drawn from, but it does not name the header
/// itself: this name, and carrying the bare failure name as its value, are INFERRED from what
/// a lenient call is recorded answering, not quoted from the model.
const ENTITY_FAILURE_HEADER: &str = "x-amz-failure-message";

/// `PutMetricData`, served so that an accepted call which DROPPED an entity says so.
///
/// "In the case of an invalid entity, the operation will return a 200 status, but an
/// additional response header will contain information about the validation errors."
/// (`PutMetricDataInput$StrictEntityValidation`) — the accepted answer's shape is `Unit`, so
/// it binds no member the handler could put that on, and the header is added here, the one
/// place a response can still be touched. Nothing else is re-implemented: the request is
/// handed to the generated [`ErasedOperation::run`], which owns deserialize -> validate ->
/// handle -> serialize and every modelled error, and the failure the header names comes from
/// the operation's OWN entity validation
/// (`crate::ops::put_metric_data::dropped_entity_failure`), so the two cannot disagree.
struct ReportingDroppedEntities<O> {
    operation: O,
}

impl<O> ReportingDroppedEntities<O> {
    /// Wrap the operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for ReportingDroppedEntities<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // The protocol is read, used and dropped inside this one statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`. The request is read through the
            // service's OWN protocol into the operation's OWN input shape, and judged by the
            // operation's OWN entity validation, so what the header reports and what the
            // handler did cannot disagree. The cost is DELIBERATE: a request is read twice,
            // once here and once in the operation's own sequence, because the framework
            // hands no already-built input to an operation.
            let dropped = service_common::event_stream::deserialize_request::<
                crate::ops::types::PutMetricDataInput,
            >(
                svc.definition().protocol(),
                req,
                crate::ops::types::PUT_METRIC_DATA_INPUT_SCHEMA,
            )
            .ok()
            .and_then(|input| crate::ops::put_metric_data::dropped_entity_failure(&input));
            let mut response = self.operation.run(svc, req, ctx).await;
            // Only an ACCEPTED call drops an entity: a refused one ingested nothing, so it
            // has no accepted metric data to report an unassociated entity for.
            //
            // The failure is one of a FIXED set of names, never text from the request, so it
            // is always a representable header value; the fallible insert is used all the
            // same, so that a header which could not be built could only cost the answer its
            // header and never the answer itself.
            if response.status().is_success() {
                if let Some(failure) = dropped {
                    let _ = response
                        .headers_mut()
                        .try_insert(ENTITY_FAILURE_HEADER, failure);
                }
            }
            response
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
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
        // NOT a re-route: the key is the one the generated table's own protocol resolves, and
        // the operation it names is the one served — `PutMetricData` wrapped in
        // [`ReportingDroppedEntities`] so an accepted call reports the entity it dropped.
        // Every other key resolves to `None` here, exactly as it would in an unedited
        // service, and falls through to the generated table.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.reporting_dropped_entities.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording. To state a refusal the way the emulated service states it,
        // replace this body with a `match refusal { .. }` that returns
        // `Some(Refused { code, message })` for the cases you reword and `None`
        // for the rest. Replace BOTH the code and the message: a client branches
        // on the code, so a message without its matching code is worse than
        // leaving both alone.
        let _ = refusal;
        None
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
