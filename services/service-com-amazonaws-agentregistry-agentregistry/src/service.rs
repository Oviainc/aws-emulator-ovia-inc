// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-agentregistry-agentregistry` service: how it claims requests and how it words the
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

use aws_smithy_schema_server::ViolationKind;
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-agentregistry-agentregistry` emulated service: the generated [`ServiceDefinition`] plus
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
        // reword it. What this service states, and all it states, is in
        // [`stated_refusal`]: the envelope, the error count, the member paths and
        // every requirement sentence stay the framework's, and an authentication
        // refusal keeps its own code and wording.
        stated_refusal(refusal)
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

/// How this service states the framework's constraint refusal, or `None` to keep the
/// framework's own wording for anything that is not one.
///
/// Three rules, and nothing else:
///
/// * NO clause quotes a value. Every one names the member it was stated at and the
///   requirement that member's value had to satisfy, whatever the request carried —
///   so the sentence a caller reads back is the same for every value outside the same
///   bound, and nothing of a request is echoed into it.
/// * a constraint the ELEMENTS of a collection member declare is stated at that
///   MEMBER, with the element shape's whole declared set nested inside it
///   ([`element_set`]) rather than at the one element that offended.
/// * a `@pattern` is quoted as this service advertises the expression: without the
///   pair of whole-value anchors the model wraps it in, so a caller that re-applies
///   the expression it was handed back uses the same one. The CHECK still runs the
///   model's pattern as written; only the rendering drops the pair.
///
/// The envelope, its error count, the clause, the member paths, the grouping and every
/// requirement sentence belong to the shared home for this wording and to the framework
/// behind it — none of them is spelled here, so this service cannot drift from the
/// others that state the same sentences. An authentication refusal is a different
/// failure with its own code and status and nothing about it is this service's to word.
fn stated_refusal(refusal: &Refusal) -> Option<Refused> {
    service_common::validation_message::aws_validation_refusal_over_elements_stating_withholding(
        refusal,
        service_common::validation::unanchored,
        // Every clause, whatever member or class it is about.
        |_| true,
        element_set,
    )
}

/// Every shape of this service whose own members a refused COLLECTION is looked up in:
/// the three operation inputs, and the structures they nest.
///
/// A member schema carries its own traits and not the members of the shape it targets,
/// so a list declared inside a structure is reached by handing that structure over
/// itself. `RegistryRecordsEntry` declares `recordIds` and `RegistryRecordFilter`
/// declares `values`; they are the only structures this model's inputs nest
/// (`smithy_query(query="closure")`).
const COLLECTION_MEMBER_SHAPES: &[&::aws_smithy_schema::Schema] = &[
    crate::ops::types::BATCH_GET_DISCOVERABLE_REGISTRY_RECORD_INPUT_SCHEMA,
    crate::ops::types::LIST_DISCOVERABLE_REGISTRY_RECORDS_INPUT_SCHEMA,
    crate::ops::types::SEARCH_DISCOVERABLE_REGISTRY_RECORDS_INPUT_SCHEMA,
    crate::ops::types::REGISTRY_RECORDS_ENTRY_SCHEMA,
    crate::ops::types::REGISTRY_RECORD_FILTER_SCHEMA,
];

/// The requirement a clause states at the collection member `collection` for a
/// constraint one of its ELEMENTS broke — the element shape's whole declared set,
/// nested — or `None` when no shape of this model declares one for that member, which
/// leaves the framework's own per-element clause standing (a list of STRUCTURES, whose
/// elements declare no constraint of their own, is one such member).
///
/// The set is the element shape's WHOLE set and not just the constraint a value
/// happened to break, because it is a property of the model rather than of the request.
/// `collection` is the member path the clause is stated at, so a member declared inside
/// a nested structure is looked up under the last segment of that path.
fn element_set(collection: &str) -> Option<String> {
    use service_common::validation_message::{element_constraint_set, element_set_requirement};

    let member = collection
        .rsplit_once('.')
        .map_or(collection, |(_, member)| member);
    let declared = element_constraint_set(COLLECTION_MEMBER_SHAPES, member)?;
    let requirements: Vec<String> = declared.into_iter().map(stated_requirement).collect();
    Some(element_set_requirement(&requirements))
}

/// One requirement of an element shape as this service states it: the sentence is the
/// framework's own, with the one rendering this service chooses applied first, so a
/// requirement nested in a collection's clause reads exactly like the same requirement
/// stated at a member the walk reported.
fn stated_requirement(declared: ViolationKind) -> String {
    service_common::validation_message::requirement(match declared {
        // The CHECK always ran the model's pattern as written; only the rendering of
        // the expression drops the whole-value anchors.
        ViolationKind::Pattern(pattern) => {
            ViolationKind::Pattern(service_common::validation::unanchored(&pattern))
        }
        other => other,
    })
}
