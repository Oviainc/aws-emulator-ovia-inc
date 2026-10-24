// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-groundstation-groundstation` service: how it claims requests and how it words the
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
//! - `override_operation` — which operation serves a request. EDITED HERE: the key and
//!   the handler stay the generated table's, but `ListGroundStationReservations` — the one
//!   operation of this service that binds a timestamp to the query string — is served
//!   through [`EpochSecondsQueryTimestamps`], so the `epoch-seconds` form its members
//!   declare binds.
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
//!
//! # Timestamps in the query string
//!
//! `ListGroundStationReservations` binds its `startTime` and `endTime` to the query string
//! and declares the wire form on each member: `@timestampFormat("epoch-seconds")`
//! (`smithy_query(query="operation", operation="ListGroundStationReservations")` —
//! `{"target": "Timestamp", "timestampFormat": "epoch-seconds", "httpQuery":
//! "startTime", "required": true}`), so every SDK sends the number of seconds since the
//! epoch. The schema the framework builds from the model carries no member-level
//! `@timestampFormat`, so its query-value deserializer reads an `@httpQuery` timestamp in
//! the protocol's default RFC-3339 `date-time` form and refuses the declared one before
//! the operation's handler runs.
//!
//! [`EpochSecondsQueryTimestamps`] closes that gap for the operations of this service that
//! bind a timestamp to the query string: a query value that is a number is read as epoch
//! seconds, anything else is left exactly as the client sent it, so the declared form
//! binds and nothing else about the request changes. It is the SHARED wrapper in
//! `service_common::query_timestamps` — the gap is the generated schema's, not this
//! service's, so the rewrite is written once for every service whose model binds a
//! timestamp to the query string.

use service_common::query_timestamps::EpochSecondsQueryTimestamps;
use smithy_server_runtime::{http, ErasedOperation, OperationRoutingTable, Refusal, Refused, Service};
use std::sync::Arc;

/// The `com-amazonaws-groundstation-groundstation` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The operations that bind a timestamp to the query string, each the generated
    /// table's own operation wrapped in [`EpochSecondsQueryTimestamps`] and built from the
    /// SAME shared engine, so a wrapped operation reads and writes the one store. Keyed by
    /// the routing key the protocol resolves, which is the key the generated table uses.
    epoch_seconds_query_timestamps: OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        // Exactly the operations whose input binds a timestamp to the query string —
        // `ListGroundStationReservations`' `startTime` and `endTime`, the only two such
        // members this service's model declares (`OPERATIONS.md`: no other operation lists
        // a `(timestamp, @httpQuery(…))` input). Every other operation of this service
        // takes its timestamps in the body, where the protocol already reads the
        // epoch-seconds form, so wrapping them would change nothing.
        let mut epoch_seconds_query_timestamps = OperationRoutingTable::new();
        epoch_seconds_query_timestamps.add_operation(
            "ListGroundStationReservations",
            ::std::boxed::Box::new(EpochSecondsQueryTimestamps::new(
                crate::ops::list_ground_station_reservations::ListGroundStationReservations::new(
                    Arc::clone(&engine),
                ),
            )) as ::std::boxed::Box<dyn ErasedOperation>,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            epoch_seconds_query_timestamps,
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

    /// Serve the request through the operation the generated table's own protocol names,
    /// wrapped in [`EpochSecondsQueryTimestamps`] for the operations that bind a timestamp
    /// to the query string so the `epoch-seconds` form their members declare binds.
    ///
    /// This is NOT a re-route: the key is the one the generated table's own protocol
    /// resolves, and the operation it names is the one served. Every other key resolves to
    /// `None` here exactly as it does for an unedited service, leaving the generated table
    /// to serve it.
    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.epoch_seconds_query_timestamps.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // AWS Ground Station sits behind a request validator that refuses a body
        // failing the model's own rules — an absent required member, or a value
        // outside a declared bound — as HTTP 400 `BadRequestException` with the fixed
        // text below, rather than echoing the constraint that failed. The error CODE is
        // what an SDK resolves the error shape from and what a client branches on, and
        // `ValidationException` (the framework's default, and no error this service
        // models) is caught by nobody, so both the code and the message are replaced
        // here together. An authentication refusal is not this validator's, so it keeps
        // the framework's 403 wording.
        match refusal {
            Refusal::Invalid(_) => Some(Refused {
                code: crate::state::BAD_REQUEST_ERROR_CODE.to_string(),
                message: crate::state::INVALID_REQUEST_BODY.to_string(),
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
