// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-cloudtrail-cloudtrail-20131101` service: how it claims requests and how it words the
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
//! - `can_handle` — which requests this service claims. EDITED HERE: the framework's
//!   default (the request's resolved route key equals this service's signing name),
//!   plus a request whose `X-Amz-Target` names this service, which the framework's
//!   routing-key fallback misreads for this service's dotted target prefix.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself. THIS service
//!   does: its `X-Amz-Target` prefix is the fully qualified Coral service name
//!   (`com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101`), so the operation is
//!   the segment after the LAST dot of the target rather than after the first.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. THIS service rewords ONE class: a tag key or value outside its modelled
//!   length, which CloudTrail answers as `InvalidTagParameterException` rather than as
//!   a generic `ValidationException`. See the inline note on that method.
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
    http, ErasedOperation, Refusal, Refused, Service, ViolationKind,
};
use std::sync::Arc;

/// The target rule in isolation, as THIS service's prefix forms exercise it: which
/// operation an `X-Amz-Target` names, over the prefix forms a client can write.
///
/// The rule itself lives in `service_common::rpc_target`, which states it once for every
/// service whose published prefix carries dots; this table pins the answers for this
/// service's own shape name, and the wire behaviour they produce is asserted end-to-end
/// in `tests/all/target_prefix_dispatch.rs`.
#[cfg(test)]
mod rpc_target;

/// The part of a tag a constraint-violation path names — `key` or `value` — or `None`
/// for a path that does not walk into a tag at all.
///
/// The validating walk spells a path in lowerCamel, with a `<1-based-index>.member`
/// step from a list into one of its elements, so the value of the second tag of a
/// tagging request's `TagsList` is `tagsList.2.member.value`, and a key in
/// `CreateChannel`'s `Tags` is `tags.1.member.key`. Those two member names are every
/// tag list this service's model declares.
fn tag_member(path: &str) -> Option<&'static str> {
    let collection = path.split('.').next()?;
    if !matches!(collection, "tagsList" | "tags") {
        return None;
    }
    match path.rsplit('.').next()? {
        "key" => Some("key"),
        "value" => Some("value"),
        _ => None,
    }
}

/// The `com-amazonaws-cloudtrail-cloudtrail-20131101` emulated service: the generated [`ServiceDefinition`] plus
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

    /// This service's model shape id, as the generated identity states it
    /// (`com.amazonaws.cloudtrail#CloudTrail_20131101`), whose LOCAL name is the last
    /// segment of every `X-Amz-Target` prefix a client writes for it. Read off the model
    /// rather than written out, so it cannot drift from it.
    fn shape_id(&self) -> &str {
        self.entry.identity().shape_id()
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
        if smithy_server_runtime::resolve_route_key(parts)
            .is_some_and(|key| key == self.entry.identity().signing_name())
        {
            return true;
        }
        // A request that carries no SigV4 credential scope is routed by its
        // `X-Amz-Target` before its `Host`, and the framework reads that signal as the
        // segment before the FIRST dot — which, for this service's fully qualified
        // target prefix, is a namespace label (`com`) that names no service. The target
        // still names THIS service, so read it by the same rule the operation is read
        // by: `service_common::rpc_target`, this emulator's one statement of the target
        // rule (the service is the last segment of the prefix, the operation the segment
        // after it).
        service_common::rpc_target::target_operation_of(&parts.headers, self.shape_id())
            .is_some()
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
        // This service's `X-Amz-Target` prefix is its fully qualified Coral service
        // name, so the prefix itself contains dots and the framework's first-dot split
        // leaves the whole `<namespace>.<Service>.<Operation>` tail as the routing key,
        // which matches no modelled operation and refuses every call with 501
        // `NotImplemented`. `service_common::rpc_target` states the rule once for every
        // service in this emulator whose prefix carries dots: the operation is the
        // segment after the LAST dot, which dispatches the target an SDK actually sends
        // and is the same answer for a single-identifier prefix. A target naming another
        // service, or an operation this model does not bind, still falls through to the
        // framework's own routing (and its routing error).
        let operation = service_common::rpc_target::requested_operation(req, self.shape_id())?;
        self.entry.routing_table().get(operation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // CloudTrail does not answer a tag outside the modelled `TagKey` / `TagValue`
        // length with the framework's generic `ValidationException`: a tag key or
        // value outside its bound is `InvalidTagParameterException`, the shape the
        // model declares for "the specified tag key or values are not valid", worded
        // from the bound the value broke. The recorded corpus pins it — `AddTags` with
        // an over-long tag value answers HTTP 400 `InvalidTagParameterException`
        // "Tag value length cannot be more than 256 characters." — and the rule holds
        // for every operation that takes a tag, not only the tagging pair.
        //
        // Every OTHER refusal keeps the framework's wording, which is what this
        // service was recorded answering elsewhere (a `resourceArn` outside its
        // pattern reads "1 validation error detected: Value at 'resourceArn' failed to
        // satisfy constraint: …").
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        violations.iter().find_map(|violation| {
            let ViolationKind::MaxLength(max) = violation.kind else {
                return None;
            };
            let part = tag_member(&violation.path)?;
            Some(Refused {
                code: "InvalidTagParameterException".to_string(),
                message: format!("Tag {part} length cannot be more than {max} characters."),
            })
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
