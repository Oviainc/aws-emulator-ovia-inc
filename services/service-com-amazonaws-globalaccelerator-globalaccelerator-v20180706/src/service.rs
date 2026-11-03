// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-globalaccelerator-globalaccelerator-v20180706` service: how it claims requests and how it words the
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
//!
//! One of those seams is taken here: `override_operation` serves every routable request
//! through [`error_message_member::ErrorMessageMemberOperation`], so the text of a failure
//! the FRAMEWORK rendered is reported in the `Message` member every error shape in this
//! service's model declares — see [`error_message_member`] for the rule and why the
//! refusal seam below cannot state it.

pub mod error_message_member;

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation,
};
use std::sync::Arc;

/// The error code this service answers an input-constraint refusal with.
///
/// Global Accelerator models exactly one client-input error — `InvalidArgumentException`
/// (HTTP 400, "An argument that you specified is invalid.") — and declares it on every
/// operation that takes a constrained member. The framework's own synthetic
/// `ValidationException` is in NO Global Accelerator operation's error union, so a
/// generated SDK dispatching on the modeled error union cannot match it. Restated here
/// because [`Service::override_refusal`] replaces the code and the message together.
const INVALID_ARGUMENT_EXCEPTION: &str = "InvalidArgumentException";

/// The sentence this service states a TAG it rejects with.
///
/// Global Accelerator validates the tags a request carries in its own frontend rather
/// than reporting them through the per-member constraint report the rest of its input
/// goes through: recorded wire behaviour — a request carrying a tag whose key is the
/// empty string (outside the `@length` 1..128 `TagKey` declares) is answered `400`
/// `InvalidArgumentException` with this fixed sentence, which names no member and echoes
/// no value. It is the wording for the CLASS — a tag the request named is not a tag this
/// service accepts — so it is stated for either bound the model declares for a `Tag`,
/// rather than for the one bound a recording happened to break.
const INVALID_PARAMETER: &str = "Invalid parameter.";

/// The member that carries tags: the `Tags` list of
/// `Tag { Key (@length 1..128), Value (@length 0..256) }`
/// (`smithy_query(query="shape", shape="Tag,TagKey,TagValue")`), which is the member the
/// recorded refusal above is about and the one shape a tag is submitted as.
///
/// The framework's constraint walk reports a violation at the lowerCamel path of the
/// member it was found under (`tags.1.member.key`), so the member a clause is about is
/// read off the head of that path. `UntagResource`'s `TagKeys` — a list of bare key
/// strings, not of `Tag` — is deliberately NOT here: nothing records how this service
/// states a refusal of one, so it keeps the per-member report.
const TAG_PARAMETER_MEMBER: &str = "tags";

/// Whether every value `violations` reports is a tag the request carried — the condition
/// under which this service states [`INVALID_PARAMETER`] instead of the per-member
/// constraint report.
///
/// ALL of them, because a request that also breaks a bound on a member of its own is
/// stated as the report that names every clause: the tag sentence names nothing, so
/// answering it for such a request would drop what was said about the other member.
fn only_tag_parameters(violations: &[Violation]) -> bool {
    !violations.is_empty()
        && violations.iter().all(|violation| {
            violation.path.split('.').next().unwrap_or_default() == TAG_PARAMETER_MEMBER
        })
}

/// The `com-amazonaws-globalaccelerator-globalaccelerator-v20180706` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The wrapper `override_operation` hands back. Stateless, and held here rather than
    /// built per request because that seam returns a BORROW.
    error_message_member: error_message_member::ErrorMessageMemberOperation,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            error_message_member: error_message_member::ErrorMessageMemberOperation,
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
        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and the operation it names is the one served — wrapped so an error the FRAMEWORK
        // rendered (the pre-handler refusal of a modelled constraint, the scaffold's 501)
        // reports its text in the `Message` member every error shape in this service's
        // model declares, rather than in the synthetic shapes' own `message`, which no
        // client of this awsJson1_1 service reads (see [`error_message_member`]). A key
        // the model does not name resolves to `None` here exactly as it does in the
        // generated table, so the router's routing-error path is unchanged.
        error_message_member::routes(&self.entry, req)
            .then_some(&self.error_message_member as &dyn ErasedOperation)    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal — a member
        // the model requires that the request omitted, or a value outside a
        // declared `@range`/`@length`/`@pattern`/enum bound — before any handler
        // runs.
        //
        // Global Accelerator states EVERY such refusal as its own modeled
        // `InvalidArgumentException` (HTTP 400 — the status the framework answers
        // too, so only the code and the wording change), carrying the
        // `<N> validation error(s) detected: ` envelope over one clause per
        // violated member: the rejected value, the member's path and the bound it
        // failed, all taken from the request. That envelope is exactly what the
        // framework renders, so the message is borrowed rather than restated —
        // only the code, which a client branches on, is this service's own.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure, and the service models `AccessDeniedException`
        // for it on the operations that can answer it.
        //
        // This seam chooses the code and the words only; the body MEMBER they are
        // serialized into is not reachable from here — see [`error_message_member`].
        //
        // The ONE class it states in its own words is a tag (see [`INVALID_PARAMETER`]),
        // which this service validates itself and reports without naming a member.
        match refusal {
            Refusal::Invalid(violations) if only_tag_parameters(violations) => Some(Refused {
                code: INVALID_ARGUMENT_EXCEPTION.to_string(),
                message: INVALID_PARAMETER.to_string(),
            }),
            Refusal::Invalid(violations) => Some(Refused {
                code: INVALID_ARGUMENT_EXCEPTION.to_string(),
                message: ::aws_smithy_schema_server::envelope(violations)?,
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

#[cfg(test)]
mod tests {
    use super::only_tag_parameters;
    use smithy_server_runtime::{Violation, ViolationKind};

    /// One violation at `path`, of the shape the constraint walk reports for a `@length`
    /// maximum.
    fn at(path: &str) -> Violation {
        Violation {
            path: path.to_owned(),
            kind: ViolationKind::MaxLength(128),
            value: Some("x".to_owned()),
            sensitive: false,
        }
    }

    /// RULE: a refusal is about tags only when EVERY clause names a member of a tag the
    /// request carried, at whatever position in the list and whichever of the two members
    /// it broke; anything else is the per-member constraint report.
    #[test]
    fn only_a_refusal_about_tags_alone_is_a_tag_refusal() {
        // (case, the paths the walk reported, whether this is a tag refusal)
        let rows: [(&str, &[&str], bool); 9] = [
            ("one tag key", &["tags.1.member.key"], true),
            ("one tag value", &["tags.1.member.value"], true),
            ("a tag beyond the first", &["tags.7.member.key"], true),
            (
                "both members of one tag",
                &["tags.1.member.key", "tags.1.member.value"],
                true,
            ),
            ("the tags member itself", &["tags"], true),
            ("a member of the request", &["trafficDialPercentage"], false),
            (
                "a tag and a member of the request",
                &["tags.1.member.key", "name"],
                false,
            ),
            // A list of bare tag KEYS is a different shape, and nothing records how this
            // service states a refusal of one, so it keeps the per-member report.
            ("a tag key on the untagging surface", &["tagKeys.1.member"], false),
            // A member whose name merely STARTS with the tags member's name is a member of
            // the request, not a tag: the head of the path is compared whole.
            ("a member named like the tags member", &["tagsOfSomething"], false),
        ];

        for (case, paths, expected) in rows {
            let violations: Vec<Violation> = paths.iter().map(|path| at(path)).collect();
            let answered = only_tag_parameters(&violations);
            assert_eq!(
                answered, expected,
                "case {case:?} over {paths:?}: expected {expected}, got {answered}"
            );
        }
    }

    /// Nothing to state: a refusal carrying no clause at all is not a tag refusal, so the
    /// framework's own wording stands rather than a sentence about tags nobody mentioned.
    #[test]
    fn a_refusal_with_no_clause_is_not_a_tag_refusal() {
        assert!(
            !only_tag_parameters(&[]),
            "expected no clause to state nothing about tags"
        );
    }
}
