// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-databrew-awsgluedatabrew` service: how it claims requests and how it words the
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

/// The `com-amazonaws-databrew-awsgluedatabrew` emulated service: the generated [`ServiceDefinition`] plus
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
        // required member, or a value outside a declared bound) to let this
        // service reword it. ONE thing is reworded here: the order an enumerated
        // type's admissible set is stated in ([`stated_value_set`]). The envelope,
        // the clause count, the echoed value, the member path and every
        // requirement's wording stay the framework's — they are rendered by the
        // same function the default wording uses — and the code stays the one the
        // framework picks for a constraint refusal, so the chosen message travels
        // with its matching code.
        match refusal {
            Refusal::Invalid(violations) => {
                let stated: Vec<Violation> =
                    violations.iter().map(in_stated_value_set_order).collect();
                Some(Refused {
                    code: VALIDATION_EXCEPTION.to_string(),
                    message: aws_smithy_schema_server::envelope(&stated)?,
                })
            }
            // Authentication is the emulator's to answer, not this service's.
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

/// The error code a constraint refusal carries on this service's wire (restJson1), which
/// is also the code the framework's own default picks for one — kept so the message
/// chosen above always travels with the code a client branches on.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// How wide the table an enumerated type's admissible values are held in is, and so the
/// modulus a value's bucket is taken at.
const VALUE_SET_BUCKETS: u32 = 32;

/// `violation` with an enumerated type's admissible set stated in the order this service
/// states that set in, and every other violation returned unchanged.
///
/// Only the ORDER of the values changes: the clause itself, the member path, the echoed
/// value and the requirement's wording keep coming from the one place that writes them.
fn in_stated_value_set_order(violation: &Violation) -> Violation {
    let ViolationKind::EnumValue(declared) = &violation.kind else {
        return violation.clone();
    };
    let stated = stated_value_set(declared)
        .into_iter()
        .map(str::to_string)
        .collect();
    Violation {
        path: violation.path.clone(),
        kind: ViolationKind::EnumValue(stated),
        value: violation.value.clone(),
        sensitive: violation.sensitive,
    }
}

/// The order an enumerated type's admissible set is STATED in: the order the set's own
/// hash table is walked in, which is each value's bucket ascending and — where two values
/// share a bucket — the reverse of the order the model declares them in, the
/// later-declared value ahead of the earlier one.
///
/// The order belongs to the enumerated TYPE rather than to a member, an operation or a
/// request carrying it, and it is computed from the values themselves: one type is stated
/// the same way under every member that targets it, at any depth of the request, and a
/// type the model gains is stated without anything being added here. It is NOT the order
/// the model declares a type's values in, and not their sorted order either.
fn stated_value_set(declared: &[String]) -> Vec<&str> {
    // Declaration order reversed FIRST, so the stable sort below leaves two values of one
    // bucket in that reverse order.
    let mut stated: Vec<&str> = declared.iter().rev().map(String::as_str).collect();
    stated.sort_by_key(|value| value_bucket(value));
    stated
}

/// Which bucket of the table `value` sits in: its 31-based string hash — the hash so far
/// multiplied by 31 plus the next UTF-16 code unit, wrapping at 32 bits — reduced to the
/// table's width.
///
/// `services/service-com-amazonaws-acmpca-acmprivateca/src/service.rs` derives the same
/// walk for its own value sets; the fold is four lines and decides only a listing order
/// over model-declared values (it reads nothing a caller sent and is not a digest), so it
/// is taken here rather than shared, as that crate's own note on the derivation explains.
fn value_bucket(value: &str) -> u32 {
    value.encode_utf16().fold(0u32, |hash, unit| {
        hash.wrapping_mul(31).wrapping_add(u32::from(unit))
    }) % VALUE_SET_BUCKETS
}
