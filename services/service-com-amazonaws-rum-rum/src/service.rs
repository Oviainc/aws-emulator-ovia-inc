// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-rum-rum` service: how it claims requests and how it words the
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

use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The error code RUM words a refused TAG call with: `BadRequestException`.
///
/// The ARN-addressed tagging operations are served by a different frontend from this
/// service's app-monitor operations, with its own error vocabulary: the recorded corpus
/// unit `rum.untag-resource-unknown-resource-rejected` sends a `ResourceArn` that is not
/// an ARN and records `err["Code"] == "BadRequestException"`, where an app-monitor call
/// refused on a modelled constraint answers `ValidationException`. The STATUS is the same
/// 400 either way; the code a client branches on is not.
const TAGGING_BAD_REQUEST_CODE: &str = "BadRequestException";

/// The whole message that frontend reports a `ResourceArn` which is not an ARN with.
///
/// It carries nothing from the request — not the value, not the member, not the pattern —
/// so it is a constant rather than a format string, and it is the same sentence for every
/// value that fails `Arn`'s syntax. Recorded in the same corpus unit, which sends a
/// `ResourceArn` of `not-an-arn-at-all` and pins this message. A refusal of any OTHER tag
/// member keeps the framework's request-derived sentence: the frontend's wording for those
/// is not known, and naming the member and the constraint it failed is what a caller needs
/// to fix the request.
const INVALID_RESOURCE_ARN_MESSAGE: &str = "Invalid input resource arn: bad syntax";

/// The dotted lowerCamel path a violation on the tagging operations' `ResourceArn`
/// member reports.
const RESOURCE_ARN_MEMBER_PATH: &str = "resourceArn";

/// The input members only RUM's tagging operations declare: `ResourceArn` (`TagResource`,
/// `UntagResource`, `ListTagsForResource`) and `TagKeys` (`UntagResource`), as the dotted
/// lowerCamel paths a violation reports.
///
/// Source: `smithy_query(query="operation",
/// operation="TagResource,UntagResource,ListTagsForResource")` against this service's
/// model, and the service digest for the rest — every other operation here names its
/// resource by `Name` or by `Id`. `Tags` is deliberately NOT in the list: `TagResource`
/// and `CreateAppMonitor` both declare it, so a violation about it does not say which
/// frontend refused the call.
const TAGGING_MEMBER_PATHS: [&str; 2] = [RESOURCE_ARN_MEMBER_PATH, "tagKeys"];

/// Whether these violations are about a request to one of the tagging operations.
///
/// [`Refusal`] describes WHAT was refused, not which operation was called, so the member
/// is the discriminator: a violation on one of [`TAGGING_MEMBER_PATHS`] can only have come
/// from a tag call. A path is dotted, so a violation inside a member starts with that
/// member's name followed by a `.`.
fn is_tagging_refusal(violations: &[Violation]) -> bool {
    violations.iter().any(|violation| {
        TAGGING_MEMBER_PATHS.iter().any(|member| {
            violation.path == *member || violation.path.starts_with(&format!("{member}."))
        })
    })
}

/// Whether one of these violations says the `ResourceArn` is not an ARN: a `@pattern`
/// violation on that member, which is the one refusal the tagging frontend has its own
/// sentence for ([`INVALID_RESOURCE_ARN_MESSAGE`]).
fn is_resource_arn_syntax_violation(violations: &[Violation]) -> bool {
    violations.iter().any(|violation| {
        violation.path == RESOURCE_ARN_MEMBER_PATH
            && matches!(violation.kind, ViolationKind::Pattern(_))
    })
}

/// The `com-amazonaws-rum-rum` emulated service: the generated [`ServiceDefinition`] plus
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
        // service reword it. Returning `None` keeps the framework's default
        // wording, which is what every operation but the tagging ones uses: an
        // app-monitor call refused on a modelled constraint answers
        // `ValidationException`, the code this service's model declares.
        //
        // A refused TAG call is the one exception (see [`TAGGING_BAD_REQUEST_CODE`]):
        // its code is `BadRequestException`, and a `ResourceArn` that is not an ARN gets
        // that frontend's own sentence for it. Any other tag refusal keeps the
        // framework's request-derived envelope, which names the member and the constraint
        // it failed, under the tag frontend's code.
        match refusal {
            Refusal::Invalid(violations) if is_tagging_refusal(violations) => Some(Refused {
                code: TAGGING_BAD_REQUEST_CODE.to_string(),
                message: if is_resource_arn_syntax_violation(violations) {
                    INVALID_RESOURCE_ARN_MESSAGE.to_string()
                } else {
                    aws_smithy_schema_server::constraints::envelope(violations)?
                },
            }),
            // Every other refusal, including an unauthenticated caller, keeps the
            // framework's protocol-correct default.
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
