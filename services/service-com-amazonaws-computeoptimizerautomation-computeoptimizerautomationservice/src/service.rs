// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice` service: how it claims requests and how it words the
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
//!   generated routing table. Return `Some(op)` to route one yourself. THIS
//!   SERVICE uses it for authorization: a request whose resource member names no
//!   automation rule of this service is answered `AccessDeniedException` (403)
//!   there, because authorization is evaluated against the supplied resource
//!   string before the service parses it — see [`DenyUnauthorizedResource`].
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
    http, BoxFuture, ErasedOperation, OperationContext, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The error code and message a request for a resource the caller is not
/// authorized for is answered with.
///
/// `AccessDeniedException` is a service-common error of every operation of this
/// service, `@httpError 403`, documented "You do not have sufficient permissions
/// to perform this action."
/// (`smithy_query(query="shape",shape="AccessDeniedException")`). AWS's own text
/// embeds the caller's ARN and the action, which is account-specific; the
/// requirement is the 403 classification, so the modelled sentence is what this
/// states.
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";
const ACCESS_DENIED_MESSAGE: &str = "You do not have sufficient permissions to perform this action.";

/// The operations whose input names a resource by ARN, and the member each one
/// names it in.
///
/// These are exactly the `RuleArn`-typed input members of this service:
/// `ruleArn` on the three rule operations and `resourceArn` on the three tagging
/// ones (`smithy_query(query="operation",operation="...")`). The table is keyed
/// by OPERATION, not by member name alone, so a future member of another
/// operation that happens to be called `resourceArn` without being a `RuleArn`
/// is not judged by this gate — and an operation that takes no resource never
/// has its body read here.
const RESOURCE_MEMBERS: [(&str, &str); 6] = [
    ("GetAutomationRule", "ruleArn"),
    ("UpdateAutomationRule", "ruleArn"),
    ("DeleteAutomationRule", "ruleArn"),
    ("TagResource", "resourceArn"),
    ("UntagResource", "resourceArn"),
    ("ListTagsForResource", "resourceArn"),
];

/// Answers one request `AccessDeniedException` (HTTP 403).
///
/// Authorization is evaluated against the resource string a request supplies
/// BEFORE the service parses that string, so a string that identifies no
/// resource of this service is refused as unauthorized rather than as a bad
/// parameter: no permission a caller can hold names a resource the request does
/// not name. The framework's pre-handler validation would otherwise reject the
/// `RuleArn` `@pattern` with a 400, which classifies a permissions failure as a
/// request bug.
///
/// It is an [`ErasedOperation`] rather than a handler because that refusal has
/// to come BEFORE the modelled-constraint walk, and `override_operation` is the
/// seam that sees a whole request first. The router still authenticates the
/// request against the modelled operation's auth schemes before running this.
struct DenyUnauthorizedResource;

impl ErasedOperation for DenyUnauthorizedResource {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        let _ = (req, ctx);
        // Rendered through the service's own envelope (`__type` + `message` on
        // this service's awsJson protocol), with the 403 the modelled
        // `AccessDeniedException` carries.
        let response = svc.definition().framework_error_response(
            aws_smithy_schema_server::ProtocolError::Unauthenticated {
                code: ACCESS_DENIED_CODE,
                message: ACCESS_DENIED_MESSAGE,
            },
        );
        Box::pin(async move { response })
    }
}

/// The `com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The 403 answer for a request naming a resource the caller cannot hold a
    /// permission for. Held here so [`Service::override_operation`] can hand out
    /// a reference to it.
    deny_unauthorized_resource: DenyUnauthorizedResource,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            deny_unauthorized_resource: DenyUnauthorizedResource,
        }
    }

    /// Does `req` name a resource no permission of this service can cover?
    ///
    /// Resolves the operation the request names first, so only the operations
    /// that take a resource ARN ([`RESOURCE_MEMBERS`]) are judged at all, then
    /// reads that operation's resource member out of the body and asks
    /// [`crate::state::names_automation_rule`] whether it names an automation
    /// rule of this service. A request this cannot read — an unknown operation,
    /// a streamed body, a body that is not the protocol's JSON object — is left
    /// to the modelled walk, which refuses it on its own terms.
    fn names_unauthorized_resource(&self, req: &aws_smithy_runtime_api::http::Request) -> bool {
        let Ok(operation) = self.entry.protocol().routing_key(req) else {
            return false;
        };
        let Some((_, arn_member)) = RESOURCE_MEMBERS
            .iter()
            .find(|(candidate, _)| *candidate == operation)
        else {
            return false;
        };
        let Some(body) = req.body().bytes() else {
            return false;
        };
        let Ok(serde_json::Value::Object(members)) = serde_json::from_slice(body) else {
            return false;
        };
        members
            .get(*arn_member)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| !crate::state::names_automation_rule(value))
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
        // Authorization comes before parameter parsing: a request whose
        // resource member does not name an automation rule of this service at
        // all is refused `AccessDeniedException` (403) here, rather than falling
        // to the modelled `RuleArn` `@pattern` rejection (400) that the
        // framework's pre-handler walk would answer with. See
        // [`DenyUnauthorizedResource`]. Every other request resolves through the
        // generated routing table.
        self.names_unauthorized_resource(req)
            .then_some(&self.deny_unauthorized_resource as _)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound). This service's
        // model declares NO `ValidationException`: the one client-input error it
        // declares — on every operation, as a service-common error — is
        // `InvalidParameterValueException` (HTTP 400), "One or more parameter
        // values are not valid". So a value that failed a modeled rule is worded
        // with that code, which is what a client of this service branches on;
        // the framework's default `ValidationException` names a shape this
        // service never declares. The status stays 400, which the protocol owns.
        //
        // Both the code and the message are replaced together: the message is
        // this service's validation report (see
        // `crate::state::validation_report`) — how many members failed, then
        // one clause per member naming the member and the requirement it
        // failed, with no rejected value echoed back; except for a value
        // outside one of the service's enum PARAMETER TYPES, which that
        // reporter words as the type's own rejection (the type, the value as
        // sent, and the type's modelled values). Authentication refusals keep
        // the framework's wording (`None`).
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: "InvalidParameterValueException".to_string(),
                message: crate::state::validation_report(violations),
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
