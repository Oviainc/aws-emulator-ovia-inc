// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-entityresolution-awsveniceservice` service: how it claims requests and how it words the
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
//!   correctly. Edited here: a request that carries no value for a `@required`
//!   `@httpHeader` member matches no route at all, because the REST router reads
//!   such a member as a route discriminator, so it is matched against the route
//!   the operation's own `@http` binding gives it and that operation serves it.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. Edited here: an enumerated type's value set is stated
//!   in the order THIS service states it in, which is not the order the set reaches
//!   the seam in; every other refusal keeps the framework's own wording.
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
    http, ErasedOperation, ErasedServiceDefinition, Operation, Refusal, Refused, Service,
    Violation, ViolationKind,
};
use std::sync::Arc;

/// The operation the model's own routing table names for `req`.
fn modeled_operation<'d>(
    definition: &'d dyn ErasedServiceDefinition,
    req: &aws_smithy_runtime_api::http::Request,
) -> Option<&'d dyn ErasedOperation> {
    let key = definition.protocol().routing_key(req).ok()?;
    definition.routing_table().get(key)
}

/// The same violation with its enum value set in the order this service STATES that
/// enum's values in.
///
/// A refusal of a value outside an enumerated type names the values that type admits,
/// and the order it names them in is the SERVICE's: the order a value set reaches this
/// seam in is the order the model happens to declare that enum's members in, while the
/// service has its own published order for the sets it publishes one for
/// ([`crate::state::published_enum_value_set`]). The enum is recognised by its value
/// SET, so one enum is stated the same way for every member targeting it, in any
/// operation and at any depth; a set this service publishes no order for, and every
/// other violation kind, is returned unchanged.
fn in_stated_enum_order(violation: &Violation) -> Violation {
    let ViolationKind::EnumValue(values) = &violation.kind else {
        return violation.clone();
    };
    let Some(stated) = crate::state::published_enum_value_set(values) else {
        return violation.clone();
    };
    Violation {
        kind: ViolationKind::EnumValue(stated.iter().map(|value| (*value).to_string()).collect()),
        ..violation.clone()
    }
}

/// The `com-amazonaws-entityresolution-awsveniceservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// `BatchDeleteUniqueId`, built from the SAME shared engine the generated entry
    /// threads into every operation, so this instance and the rest of the service read
    /// one store. Held because [`Service::override_operation`] hands back a BORROW of an
    /// operation, and this is the one operation of this service whose route the REST
    /// router keys on a `@required` `@httpHeader` member (see
    /// [`EmulatedService::header_keyed_operation`]).
    batch_delete_unique_id: crate::ops::batch_delete_unique_id::BatchDeleteUniqueId<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            batch_delete_unique_id:
                crate::ops::batch_delete_unique_id::BatchDeleteUniqueId::new(engine),
        }
    }

    /// The operation of this service whose own `@http` route `req` ADDRESSES, among the
    /// ones whose route the REST router keys on a `@required` `@httpHeader` member, or
    /// `None` when `req` addresses none of them.
    ///
    /// The framework's REST router treats such a member as a route DISCRIMINATOR: it
    /// rejects a candidate route whose `@required` header bindings are absent from the
    /// request, which is how an S3-class model tells several operations apart on ONE
    /// method + URI template by the shape of the request. This service overloads no
    /// route — every method + URI template it publishes names exactly one operation — so
    /// for a request that binds no value for such a member the rule can only misfire:
    /// the request matches NO route and is answered "unknown operation" (HTTP 501, a
    /// status no operation here models) instead of reaching the operation the model
    /// binds to its method and path, which models `ValidationException` for exactly this
    /// mistake.
    ///
    /// `BatchDeleteUniqueId$uniqueIds` is the one such member this model declares
    /// (`@required` + `@httpHeader "uniqueIds"`,
    /// `smithy_query(query="operation",operation="BatchDeleteUniqueId")`); every other
    /// operation binds its `@required` members to the path or the body, so its route
    /// always resolves and it is never reached this way. And a LIST bound to a header is
    /// exactly where the misfire is reachable through a legitimate call: a list of no
    /// elements has no header form at all, so a caller that submits one sends no header.
    /// No method or URI template is written out here: the candidate is matched through
    /// the shared reader (`service_common::addresses_modeled_http_route`) against the
    /// route its OWN input schema's `@http` binding gives it, so this seam cannot drift
    /// from the model and the operation can only ever serve the route the model binds to
    /// it.
    fn header_keyed_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        let schema = <crate::ops::batch_delete_unique_id::BatchDeleteUniqueId<Eng> as Operation>::INPUT_SCHEMA;
        service_common::addresses_modeled_http_route(req, schema)
            .then_some(&self.batch_delete_unique_id as &dyn ErasedOperation)
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
        // The model's own routing table is asked FIRST and is left to resolve every
        // request it can name. What it cannot name is a request that binds no value for
        // a `@required` `@httpHeader` member of the operation the model binds to its
        // method and path: the REST router reads such a member as a route discriminator
        // and matches no route at all, so the request would be answered "unknown
        // operation" (HTTP 501) although the operation is implemented and models a
        // client-input error for exactly this mistake. Such a request is therefore
        // matched against the route the candidate operation's own `@http` binding gives
        // it ([`EmulatedService::header_keyed_operation`]), and that operation then
        // answers it from the request's OWN values — refusing the absent member as an
        // absent `@required` member of the same operation bound to the path or the body
        // already is.
        if modeled_operation(self.definition(), req).is_some() {
            return None;
        }
        self.header_keyed_operation(req)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording.
        //
        // ONE thing this service states differently: an enumerated type's value
        // set is listed in the order the service publishes for that enum
        // ([`in_stated_enum_order`]) rather than the order the set reaches this
        // seam in. The code, the envelope, the member paths, the echoed value and
        // every other requirement stay the framework's, so nothing else can drift.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        let restated: Vec<Violation> = violations.iter().map(in_stated_enum_order).collect();
        // A refusal this service states exactly as the framework already does keeps
        // the framework's own rendering rather than being re-rendered here, so the
        // blast radius stays on the rule above.
        if restated == *violations {
            return None;
        }
        Some(Refused {
            // The code a constraint refusal carries on this service's restJson1
            // envelope — the same one the framework's default picks, supplied here
            // because a chosen message must travel with its matching code.
            code: "ValidationException".to_string(),
            // The same envelope the framework renders (`1 validation error
            // detected: …`), built from the restated violations so the clause
            // wording stays in the one place that owns it.
            message: aws_smithy_schema_server::constraints::envelope(&restated)?,
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
}
