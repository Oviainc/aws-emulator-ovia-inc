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
//!   correctly. EDITED HERE, for two reasons that compose: every request this service
//!   serves goes through [`AnnouncedMediaType`], which runs the operation unchanged and
//!   announces this service's media type on the response (see "The media type this
//!   service announces" below), and a `DELETE /tags/{ResourceArn}` that binds no
//!   `tagKeys` value — the empty key list `UntagResource` models — is served by
//!   `UntagResource`, which the framework's required-query-binding rule leaves unmatched
//!   (see [`UNTAG_RESOURCE_ROUTE`]). Which operation serves a request is still the
//!   model's routing table's answer in every other case.
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
//! ## The media type this service announces
//!
//! The model declares `restJson1`, whose serializer announces
//! [`crate::state::JSON_CONTENT_TYPE`] on every JSON body, but every recorded RUM
//! response — every operation, a 200 and a 404 alike — carries
//! [`crate::state::RESPONSE_CONTENT_TYPE`], and the announced media type is what a
//! client selecting a parser or validating the response's type reads. The value is a
//! property of the SERVICE, uniform across operations and across success and error
//! paths, so it is applied once here rather than in twenty handlers:
//! [`AnnouncedMediaType`] fronts every request this service serves, runs the operation
//! the model names unchanged, and re-announces that one header on the way out
//! ([`announced`]). A response whose body is not the JSON envelope keeps the type its own
//! target shape maps to.
//!
//! `override_protocol` is not the seam for it: the codec is right, and swapping in
//! another protocol would swap away the URI-based routing this REST service needs.
//!
//! Two things the gate deliberately does not touch: authentication, which the router
//! settles before any operation runs and reads off the MODELLED operation the routing
//! key names (so a gate cannot weaken an operation's auth schemes), and the refusals
//! that happen in FRONT of the service — a request naming no modelled operation, an
//! unverifiable signature, a malformed `aws-chunked` body. Those are not responses this
//! service produced, so they keep the framework's protocol-correct rendering.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, OperationContext, Refusal, Refused, Service,
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

/// The one route of this service whose operation the generated routing table cannot name
/// when a `@required` `@httpQuery` member is absent, as
/// `(method, collection segment, operation)`.
///
/// `UntagResource` is `DELETE /tags/{ResourceArn}` with `TagKeys` `@required` and bound to
/// the `tagKeys` query parameter (`smithy_query(query="operation",
/// operation="UntagResource")`). The framework's REST router rejects a candidate route
/// whose `@required` query bindings are not on the request — the rule that lets an
/// S3-class model tell several operations apart on ONE method + URI template by the SHAPE
/// of the request. This service overloads no route: `GET`, `POST` and `DELETE` on
/// `/tags/{ResourceArn}` name one operation each, so for `DELETE` the rule only misfires.
/// A call that binds no `tagKeys` value — which is how a query-bound list spells the empty
/// list `TagKeyList`'s `@length(min: 0)` admits — then matches NO route and is answered
/// "unknown operation" (HTTP 501 `NotImplemented`, a status and code the operation does
/// not model) instead of being served.
///
/// It is the only such route in this service: every other operation here either binds no
/// `@required` member to the query string at all, or (`ListTagsForResource`,
/// `TagResource`) binds none to it, so no other method+path pair can fail to resolve this
/// way.
const UNTAG_RESOURCE_ROUTE: (&str, &str, &str) = ("DELETE", "tags", "UntagResource");

/// Whether `req` is a request to [`UNTAG_RESOURCE_ROUTE`]: the method, and a path whose
/// segments are `tags` followed by ONE non-empty segment — the single `{ResourceArn}`
/// label the modelled URI pattern ends in, which an SDK percent-encodes, so the label
/// itself carries no `/`. Segments are read the way the framework's own matcher reads
/// them (leading and trailing `/` ignored), or the two would disagree about which
/// requests the generated table already resolves.
///
/// The URI this seam is handed is the whole one the SDK sent, so its path comes from the
/// shared reader (`service_common::uri_path`) and its segments from the shared
/// `nth_path_segment`, rather than from a private copy of either.
fn is_untag_resource_route(req: &aws_smithy_runtime_api::http::Request) -> bool {
    let (method, collection, _) = UNTAG_RESOURCE_ROUTE;
    if req.method() != method {
        return false;
    }
    let path = service_common::uri_path(req.uri());
    let path = path.trim_end_matches('/');
    service_common::nth_path_segment(path, 0) == Some(collection)
        && service_common::nth_path_segment(path, 1).is_some_and(|label| !label.is_empty())
        && service_common::nth_path_segment(path, 2).is_none()
}

/// The `com-amazonaws-rum-rum` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The one gate every request this service serves is run through: it announces this
    /// service's media type on the response. Held on the service because
    /// [`Service::override_operation`] hands back a borrow of it.
    announced_media_type: AnnouncedMediaType<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            announced_media_type: AnnouncedMediaType::new(engine),
        }
    }
}

/// This service's response envelope in front of one request: the media type every RUM
/// response announces, and nothing else. See the module docs.
///
/// It owns no behaviour of its own: the operation that serves the request is the one the
/// model's routing table names, reached through its own `run`, so that operation stays
/// the single owner of the request — its telemetry included. The one exception is
/// [`UNTAG_RESOURCE_ROUTE`], the route the protocol's routing key cannot name, which the
/// gate serves from the `UntagResource` it holds.
struct AnnouncedMediaType<Eng> {
    /// `UntagResource`, built from the SAME shared engine the generated entry threads
    /// into every operation, so this gate and the rest of the service read one store.
    /// Held rather than looked up because [`ErasedOperation::auth_schemes`] is asked
    /// without a request in hand, and the schemes this gate reports for the route the
    /// routing key cannot name must be that operation's MODELLED ones.
    untag_resource: crate::ops::untag_resource::UntagResource<Eng>,
}

impl<Eng: Send + Sync + 'static> AnnouncedMediaType<Eng> {
    fn new(engine: Arc<Eng>) -> Self {
        Self {
            untag_resource: crate::ops::untag_resource::UntagResource::new(engine),
        }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for AnnouncedMediaType<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The generated operation serves the request through the framework's own
            // deserialize -> validate -> handle -> serialize sequence, so a modelled
            // success, a handler's modelled error and a boundary refusal all pass back
            // out through here. Reached through the routing table rather than through
            // `override_operation`, which would route straight back to this gate.
            let response = match modeled_operation(svc, req) {
                Some(op) => op.run(svc, req, ctx).await,
                // The one route the protocol's routing key cannot name, served by the
                // operation the model binds to it (see [`UNTAG_RESOURCE_ROUTE`]).
                None if is_untag_resource_route(req) => {
                    self.untag_resource.run(svc, req, ctx).await
                }
                // Unreachable: this gate is served only for a request that is one of the
                // two cases above.
                None => svc.definition().routing_error_response(req),
            };
            announced(response)
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // Consulted only when the modelled routing key names NO operation — the untag
        // route above — because the router prefers the modelled operation's schemes for
        // every request whose key it can resolve. So the schemes reported here are that
        // operation's own modelled ones, read off it rather than written out: a gate
        // must not be able to weaken what the model requires.
        self.untag_resource.auth_schemes()
    }
}

/// The response as this service announces it: one media type for every JSON body it
/// produces, successes and failures alike.
///
/// Only the `restJson1` JSON envelope is re-announced
/// ([`crate::state::JSON_CONTENT_TYPE`] -> [`crate::state::RESPONSE_CONTENT_TYPE`]); a
/// body that is not that envelope carries the type its own target shape maps to and is
/// left exactly as the serializer produced it.
fn announced(mut response: RtResponse) -> RtResponse {
    if response.headers().get("content-type") == Some(crate::state::JSON_CONTENT_TYPE) {
        response
            .headers_mut()
            .insert("content-type", crate::state::RESPONSE_CONTENT_TYPE);
    }
    response
}

/// The operation the model's routing table names for this request.
fn modeled_operation<'s>(svc: &'s dyn Service, req: &RtRequest) -> Option<&'s dyn ErasedOperation> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(key)
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

    /// Serve every request this service answers through [`AnnouncedMediaType`], the gate
    /// that announces this service's media type on the response (see the module docs),
    /// including the one request whose operation the protocol's routing key cannot name:
    /// a `DELETE /tags/{ResourceArn}` whose `@required` `tagKeys` query parameter is
    /// ABSENT — the empty key list `TagKeyList`'s `@length(min: 0)` admits.
    ///
    /// WHICH operation serves a request is still decided by the model: the gate resolves
    /// it from the generated routing table, exactly as the router would have, and the one
    /// route the table cannot name is matched by method and path alone (see
    /// [`UNTAG_RESOURCE_ROUTE`] for why the framework's required-binding rule leaves it
    /// unmatched, and `crate::ops::untag_resource`'s own input reader for how the absent
    /// member is then read as the empty list). A request that names no modelled operation
    /// and is not that route is left to the framework, which refuses it in front of this
    /// service in its own protocol-correct envelope.
    fn override_operation(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        let gate = &self.announced_media_type as &dyn ErasedOperation;
        if modeled_operation(self, req).is_some() || is_untag_resource_route(req) {
            return Some(gate);
        }
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
