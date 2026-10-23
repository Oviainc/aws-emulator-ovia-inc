// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions` service: how it claims requests and how it words the
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
    http, BoxFuture, ErasedOperation, OperationContext, Refusal, Refused, Service, ViolationKind,
};
use std::sync::Arc;

/// The URI `AssociateUser` is bound to (`@http(method: "POST", uri:
/// "/user/AssociateUser")`), matched by the authorization gate below before the framework
/// has resolved a routing key.
const ASSOCIATE_USER_URI: &str = "/user/AssociateUser";

/// The operation name a gated request is recorded under: the one the model names for that
/// route, so a refusal lands in the same telemetry series as the requests that reach the
/// handler.
const ASSOCIATE_USER_OPERATION: &str = "AssociateUser";

/// The member every tagging operation addresses its resource by, as the violation path the
/// boundary validator reports for it.
///
/// `ListTagsForResource`, `TagResource` and `UntagResource` each declare `ResourceArn`
/// with the service's `@pattern` for an ARN; a value outside it is this service's own
/// `BadRequestException`, not the generic `ValidationException` the validator would name.
/// A violation path spells a member in lowerCamel (`aws_smithy_schema_server::constraints`
/// `path::member`), which is the spelling matched here.
const RESOURCE_ARN_MEMBER: &str = "resourceArn";

/// The `com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same shared engine the entry constructed each operation from, so the
    /// authorization gate in [`Self::override_operation`] can read this service's store
    /// before any handler runs. The gate must decide BEFORE the framework deserializes and
    /// constraint-checks the request, which is the one decision that cannot be made from
    /// inside a handler.
    engine: Arc<Eng>,
    /// The refusal the authorization gate serves, held here because
    /// [`Service::override_operation`] hands back a borrowed operation.
    fail_closed: FailClosed,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
            fail_closed: FailClosed,
        }
    }

    /// Whether this service will authorize an `AssociateUser` request, read off the RAW
    /// request body.
    ///
    /// The predicate is the handler's own
    /// ([`crate::state::ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::authorizes_instance`]):
    /// the caller's ownership of the EC2 instance the request names. Asking the same
    /// question in the same place is what keeps the two gates from ever disagreeing — this
    /// one only answers it EARLIER, before the boundary validator runs.
    ///
    /// `None` means "the gate does not apply": the body is not readable as JSON, or it
    /// names no `InstanceId` at all, so there is nothing to authorize and the framework's
    /// own refusal stands. `Some(false)` is a request this service refuses before it looks
    /// at the request's shape.
    fn authorizes_user_association(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<bool> {
        let body: serde_json::Value = serde_json::from_slice(req.body().bytes()?).ok()?;
        let instance_id = body.get("InstanceId")?.as_str()?;
        let state = crate::state::service_state(&*self.engine)?;
        let authorized = state.lock().authorizes_instance(instance_id);
        Some(authorized)
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
        // THE AUTHORIZATION GATE, and the only seam it can run in.
        //
        // `AssociateUser` is authorized before its request shape is validated: the same
        // call against an instance this service will not authorize is refused identically
        // whether its `Tags` map is within the modelled `@length` or past it. The
        // framework's centralized walk is deserialize -> validate -> handle, so a handler
        // runs only AFTER modelled-constraint enforcement; this hook is consulted before
        // any of it, which is why the decision lives here and not in
        // `crate::ops::associate_user`.
        //
        // Only `AssociateUser` is gated. `DisassociateUser` answers the scaffold's 501 for
        // every request, so there is nothing yet for a gate in front of it to be
        // consistent with; its own issue owns it.
        if req.method() == "POST" && request_path(req.uri()) == ASSOCIATE_USER_URI {
            if self.authorizes_user_association(req) == Some(false) {
                return Some(&self.fail_closed);
            }
            return None;
        }
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // A `ResourceArn` outside this service's ARN `@pattern` is refused by the boundary
        // validator, which names every such refusal `ValidationException`. This service
        // names it `BadRequestException` instead — the code its tagging operations really
        // send for an ARN that is not usable, and the code a tag-management client branches
        // on. The handlers answer that same code for an ARN that is well formed and names
        // no resource (`crate::scaffolded::Scaffolded::invalid_input_resource_arn`), so one
        // refusal does not arrive under two codes depending on which layer caught it; only
        // the sentence differs, because only one of the two is about syntax.
        //
        // Only a PATTERN violation of that member is reworded: the sentence says the ARN's
        // syntax is wrong, which is true of nothing else. It is reworded whenever the set
        // CONTAINS one, not only when it is the whole set — an ARN this service cannot use
        // is answered the same way whatever else the request also got wrong, which is the
        // rule a tag-management client branches on. A refusal with no such violation keeps
        // the framework's wording, which is what returning `None` leaves in place.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        let bad_arn_syntax = |violation: &smithy_server_runtime::Violation| {
            violation.path == RESOURCE_ARN_MEMBER
                && matches!(violation.kind, ViolationKind::Pattern(_))
        };
        if violations.iter().any(bad_arn_syntax) {
            return Some(Refused {
                code: crate::scaffolded::BAD_REQUEST_EXCEPTION_CODE.to_string(),
                message: crate::scaffolded::INVALID_INPUT_RESOURCE_ARN_SYNTAX_MESSAGE.to_string(),
            });
        }
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

/// The path portion of a request URI, which is what an operation's `@http` `uri` pattern
/// is written against. The runtime-API request carries the URI as a string, so the query
/// and any scheme/authority prefix are cut here rather than matched into the pattern.
///
/// A local copy, as the sibling services that need it carry one: the framework's own
/// `request_path` (`vendor/smithy-schema-server/src/rest_xml.rs`) is `pub(crate)` and that
/// tree is read-only here, and `service-common`'s `pub` one would make this crate depend
/// on that crate for three lines. Migrating these copies onto one shared reader is the
/// follow-up this duplication is waiting on.
fn request_path(uri: &str) -> &str {
    let without_query = uri.split(['?', '#']).next().unwrap_or(uri);
    match without_query.find("://") {
        Some(scheme_end) => match without_query[scheme_end + 3..].find('/') {
            Some(authority_end) => &without_query[scheme_end + 3 + authority_end..],
            None => "/",
        },
        None => without_query,
    }
}

/// The refusal this service fails closed with, as an operation the dispatch path can run.
///
/// It answers one thing: the non-modelled `UnauthorizedException` and its fixed sentence,
/// with the SAME code, sentence and status the handler's own gate answers
/// ([`crate::scaffolded::Scaffolded::unauthorized`]), so a caller cannot tell which gate
/// refused the request — only that it was refused. It is rendered through the service's
/// own protocol envelope (`refusal_response`), so a fail-closed refusal is shaped exactly
/// like every other refusal this service sends, and it reads neither the request nor any
/// state: the decision was already made by
/// [`EmulatedService::authorizes_user_association`]; this only states it.
///
/// The refusal travels as an `Invalid` protocol error with no violations, which is what
/// gives it the 400 this service reports a refusal with
/// ([`crate::scaffolded::SERVICE_REFUSAL_STATUS`]); the chosen code and message replace
/// the wording that error would otherwise carry.
struct FailClosed;

impl ErasedOperation for FailClosed {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        let _ = req;
        let started = std::time::Instant::now();
        Box::pin(async move {
            let response = svc.definition().refusal_response(
                aws_smithy_schema_server::ProtocolError::Invalid(Vec::new()),
                Some(Refused {
                    code: crate::scaffolded::UNAUTHORIZED_EXCEPTION_CODE.to_string(),
                    message: crate::scaffolded::UNAUTHORIZED_EXCEPTION_MESSAGE.to_string(),
                }),
            );
            record_refusal(ctx, response.status().as_u16(), started.elapsed());
            response
        })
    }

    // `None`, NOT an empty list: the request is still signed as the operation the model
    // names. Answering `Some(&[])` here would serve a gated request unsigned.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        None
    }
}

/// Record the refusal above as the operation row the framework's dispatch sequence would
/// have recorded, so a gated request is not a hole in the per-operation telemetry series:
/// answering from this seam skips that sequence, and the identical refusal the handler's
/// own gate answers IS recorded by it. Same classification (from the response's status and
/// the error code), same folded handle (this service's signing name and the operation the
/// request named), same already-sanitized request metadata, and gated on a client
/// collecting exactly as the framework gates it.
fn record_refusal(ctx: &OperationContext<'_>, status: u16, latency: std::time::Duration) {
    if !emulator_telemetry::recording_active() {
        return;
    }
    let handle = emulator_telemetry::OperationHandle::folded(
        ctx.identity().signing_name_static(),
        ASSOCIATE_USER_OPERATION,
    );
    emulator_telemetry::record_generated_operation_with_handle(
        &handle,
        latency,
        Some(emulator_telemetry::OperationFailure::classify(
            status,
            crate::scaffolded::UNAUTHORIZED_EXCEPTION_CODE,
        )),
        smithy_server_runtime::RequestTelemetry::from_headers(&ctx.parts().headers).into_metadata(),
    );
}
