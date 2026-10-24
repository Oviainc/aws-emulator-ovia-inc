// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-m2-awssupernovacontrolplaneservice` service: how it claims requests and how it words the
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

use aws_smithy_schema_server::constraints::Violation;
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The error code m2 answers a caller it will not authorize with: the modelled
/// `com.amazonaws.m2#AccessDeniedException`, `@error("client")` with
/// `@httpError(403)` and a required `message`
/// (`smithy_query(query="shape",shape="AccessDeniedException")`). It is one of this
/// SERVICE's common errors — `smithy_query(query="service_info")` lists
/// `AccessDeniedException`/`InternalServerException`/`ThrottlingException` — so every
/// operation declares it, and it is the shape a client's catch-block names for a
/// request that fails authorization ahead of the handler.
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// What that denial says: the sentence the model documents for the shape ("The account
/// or role doesn't have the right permissions to make the request."). The refusal is an
/// authorization decision about the CALLER, not about any member of the request, so it
/// states the documented sentence rather than anything built from the request.
const ACCESS_DENIED_MESSAGE: &str =
    "The account or role doesn't have the right permissions to make the request.";

/// The error code m2's ARN layer refuses a caller-supplied `resourceArn` that is not
/// an ARN with.
///
/// `BadRequestException` is NOT a shape in this service's model: the ARN is parsed
/// ahead of the application code that owns the modelled errors, so the refusal is
/// worded through the framework's refusal channel ([`Refused`]) instead of an error
/// enum. Recorded behaviour (corpus `simparity-wire-aws-v1`, fixture
/// `m2-environment-not-found-errors`, step `task#4` `ListTagsForResource`), where
/// `GET /tags/not-a-valid-arn` is answered 400 with
/// `x-amzn-errortype: BadRequestException`.
const BAD_REQUEST_CODE: &str = "BadRequestException";

/// What that refusal says, recorded with the code above: the ARN layer reports the
/// class of failure (the value is not an ARN) and names neither the member nor the
/// value — the member-by-member clause is what the modelled validation error carries,
/// for the refusals the application layer owns.
const INVALID_RESOURCE_ARN: &str = "Invalid input resource arn: bad syntax";

/// The member whose value goes through m2's ARN layer: the resource a tagging call
/// addresses, named by the violation path exactly as the constraint walk spells a
/// top-level member.
///
/// `smithy_query(query="operation",operation="ListTagsForResource,TagResource,
/// UntagResource")` shows the three tag operations binding `resourceArn` as an
/// `@httpLabel` targeting `Arn` on `/tags/{resourceArn}`, and no other operation of
/// this model binds a member of that name. Every OTHER `Arn`-typed input member (the
/// `roleArn` of `CreateApplication`, say) is an attribute of the resource being
/// written rather than the resource the call addresses, so it is not parsed here and
/// keeps the modelled validation refusal.
const RESOURCE_ARN_MEMBER: &str = "resourceArn";

/// m2's ARN-layer refusal for a `resourceArn` that is not syntactically an ARN, or
/// `None` when no violation is one.
///
/// The rule: a caller-supplied resource ARN is PARSED before the modelled constraint
/// walk reports on it, and a value that is not an ARN at all is a bad request
/// ([`BAD_REQUEST_CODE`] + [`INVALID_RESOURCE_ARN`]), not a pattern-constraint
/// violation. "Not an ARN at all" is [`is_resource_arn`]: six colon-separated fields,
/// the first literally `arn`, with a partition, a vendor and a resource. A value that IS
/// a well-formed ARN but falls outside the modelled `Arn` pattern (a short account id,
/// say) parsed fine, so it keeps the framework's modelled validation refusal, which
/// names the member and the constraint it failed.
///
/// PRECEDENCE: one bad-syntax `resourceArn` is the WHOLE answer, even when the request
/// breaks other constraints too (a `TagResource` with both a non-ARN label and an
/// out-of-bound `tags` map). That follows from where the refusal comes from — the ARN
/// layer runs ahead of the constraint walk, so the walk's other clauses were never
/// reported in the first place — and it is why this returns one `Refused` rather than
/// folding the ARN clause into the framework's aggregate.
fn resource_arn_refusal(violations: &[Violation]) -> Option<Refused> {
    let bad_syntax = violations.iter().any(|violation| {
        // The whole path, not its last segment: `resourceArn` is an `@httpLabel`, so
        // it is always a TOP-LEVEL member of the input, and nothing nested under
        // another member is this refusal's business.
        //
        // WHICH constraint the walk reported is not part of the test: what the ARN
        // layer decides is whether the value PARSES as an ARN, so any violation on
        // this member whose value does not is bad syntax. (Today the `Arn` shape
        // declares only a `@pattern`, so a pattern failure is the only kind this
        // member can produce; keying on the parse rather than on the kind is what
        // keeps the rule true if the model ever adds a bound to it.)
        violation.path == RESOURCE_ARN_MEMBER
            && violation
                .value
                .as_deref()
                .is_some_and(|value| !is_resource_arn(value))
    });
    bad_syntax.then(|| Refused {
        code: BAD_REQUEST_CODE.to_string(),
        message: INVALID_RESOURCE_ARN.to_string(),
    })
}

/// Whether `value` is a well-formed resource ARN:
/// `arn:<partition>:<vendor>:<region>:<account>:<resource>`, six colon-separated fields
/// whose first is literally `arn`, with a non-empty partition, vendor and resource. The
/// resource field keeps every colon it carries, so `…:type:id` reads exactly like
/// `…:type/id`; `region` and `account` may be EMPTY, because AWS writes a
/// partition-global or account-less resource that way (`arn:aws:s3:::bucket`) and those
/// ARNs are well formed.
///
/// That is the grammar `services/common/src/arn.rs` parses, read here rather than called
/// through: this crate cannot take that path dependency, because the dependency edge it
/// adds to the workspace `Cargo.lock` cannot be landed — mainline rewrites that file
/// every few minutes and the squash merge refuses any overlap in it, so a fix carrying a
/// `Cargo.lock` delta never merges. Nothing else in this crate reads an ARN, so this is
/// the one reader; if the crate ever gains `service-common` for another reason, delete
/// this and call `service_common::arn::parse`.
fn is_resource_arn(value: &str) -> bool {
    // `splitn(6, …)` leaves the whole resource — colons included — as the last field.
    let fields: Vec<&str> = value.splitn(6, ':').collect();
    let [prefix, partition, vendor, _region, _account, resource] = fields.as_slice() else {
        return false;
    };
    *prefix == "arn" && !partition.is_empty() && !vendor.is_empty() && !resource.is_empty()
}

/// The `com-amazonaws-m2-awssupernovacontrolplaneservice` emulated service: the generated [`ServiceDefinition`] plus
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
        // The centralized walk calls this on a framework-level refusal, before any
        // handler runs. Two of this service's refusals are not what the framework
        // words by default, and each replaces BOTH the code and the message — a client
        // branches on the code.
        //
        // 1. A caller the service will not authorize: the framework names its own
        //    `AccessDenied`, a code no m2 client branches on, but m2 answers the
        //    modelled `AccessDeniedException` (403, required `message`), which is one
        //    of the service's common errors and so the shape every operation declares
        //    for this refusal. The status belongs to the protocol (403) and is the same
        //    either way. `Refusal::Unauthenticated` carries no code of its own, so this
        //    is every pre-handler credential refusal — the framework does not
        //    distinguish a caller it cannot authenticate from one it will not
        //    authorize, and this service's model declares a shape only for the latter.
        // 2. A `resourceArn` that is not an ARN: refused by the ARN layer as a bad
        //    request rather than reported as a pattern-constraint violation; see
        //    [`resource_arn_refusal`].
        //
        // Every other constraint refusal keeps the framework's wording, which names
        // the member and the constraint it failed.
        match refusal {
            Refusal::Unauthenticated => Some(Refused {
                code: ACCESS_DENIED_CODE.to_string(),
                message: ACCESS_DENIED_MESSAGE.to_string(),
            }),
            Refusal::Invalid(violations) => resource_arn_refusal(violations),
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
