// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-codedeploy-codedeploy-20141006` service: how it claims requests and how it words the
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
    http, ErasedOperation, Refusal, Refused, Service, ViolationKind,
};
use std::sync::Arc;

/// Which modelled error CodeDeploy answers for a refused input member: the code
/// for an ABSENT (or empty) value, and the code for a value that is present but
/// not one the service accepts. `None` in either position means this service
/// models no such code for that member, so the framework's default wording
/// stands for that case.
///
/// Every code here is a shape this service's model declares for the operations
/// carrying that member — see the crate root's `OPERATIONS.md`, which lists each
/// operation's errors.
const REFUSAL_CODES: &[(&str, Option<&str>, Option<&str>)] = &[
    (
        "applicationName",
        Some("ApplicationNameRequiredException"),
        Some("InvalidApplicationNameException"),
    ),
    (
        "applicationNames",
        Some("ApplicationNameRequiredException"),
        Some("InvalidApplicationNameException"),
    ),
    (
        "deploymentGroupName",
        Some("DeploymentGroupNameRequiredException"),
        Some("InvalidDeploymentGroupNameException"),
    ),
    (
        "deploymentGroupNames",
        Some("DeploymentGroupNameRequiredException"),
        Some("InvalidDeploymentGroupNameException"),
    ),
    (
        "deploymentConfigName",
        Some("DeploymentConfigNameRequiredException"),
        Some("InvalidDeploymentConfigNameException"),
    ),
    (
        "deploymentId",
        Some("DeploymentIdRequiredException"),
        Some("InvalidDeploymentIdException"),
    ),
    (
        "deploymentIds",
        Some("DeploymentIdRequiredException"),
        Some("InvalidDeploymentIdException"),
    ),
    (
        "instanceName",
        Some("InstanceNameRequiredException"),
        Some("InvalidInstanceNameException"),
    ),
    (
        "instanceNames",
        Some("InstanceNameRequiredException"),
        Some("InvalidInstanceNameException"),
    ),
    ("instanceId", Some("InstanceIdRequiredException"), None),
    ("instanceIds", Some("InstanceIdRequiredException"), None),
    (
        "targetId",
        Some("DeploymentTargetIdRequiredException"),
        Some("InvalidDeploymentTargetIdException"),
    ),
    (
        "targetIds",
        Some("DeploymentTargetIdRequiredException"),
        Some("InvalidDeploymentTargetIdException"),
    ),
    (
        "serviceRoleArn",
        Some("RoleRequiredException"),
        Some("InvalidRoleException"),
    ),
    (
        "resourceArn",
        Some("ResourceArnRequiredException"),
        Some("InvalidArnException"),
    ),
    (
        "revision",
        Some("RevisionRequiredException"),
        Some("InvalidRevisionException"),
    ),
    (
        "revisions",
        Some("RevisionRequiredException"),
        Some("InvalidRevisionException"),
    ),
    // Value-set members: a refusal can only ever be "not one of the values".
    ("computePlatform", None, Some("InvalidComputePlatformException")),
    (
        "fileExistsBehavior",
        None,
        Some("InvalidFileExistsBehaviorException"),
    ),
    (
        "includeOnlyStatuses",
        None,
        Some("InvalidDeploymentStatusException"),
    ),
    (
        "registrationStatus",
        None,
        Some("InvalidRegistrationStatusException"),
    ),
    ("tagFilters", None, Some("InvalidTagFilterException")),
    (
        "deploymentWaitType",
        None,
        Some("InvalidDeploymentWaitTypeException"),
    ),
    (
        "status",
        None,
        Some("InvalidLifecycleEventHookExecutionStatusException"),
    ),
    ("sortBy", None, Some("InvalidSortByException")),
    ("sortOrder", None, Some("InvalidSortOrderException")),
    ("deployed", None, Some("InvalidDeployedStateFilterException")),
    (
        "instanceStatusFilter",
        None,
        Some("InvalidInstanceStatusException"),
    ),
    (
        "instanceTypeFilter",
        None,
        Some("InvalidInstanceTypeException"),
    ),
    // `targetFilters` is a map whose KEY is the enumerated filter name, and the
    // framework captures no key constraints, so such a value is not refused at the
    // boundary at all and there is nothing here to word.
];

/// The code [`EmulatedService::override_refusal`] answers for `member` failing
/// `kind`.
///
/// A member absent from the request and one sent empty are the SAME refusal to
/// CodeDeploy — both mean "you did not give me this name" — so a `@length`
/// minimum failure maps to the member's `…RequiredException` alongside an absent
/// one; every other constraint failure means the value itself is not one the
/// service accepts.
fn refusal_code(member: &str, kind: &ViolationKind) -> Option<&'static str> {
    let absent = matches!(
        kind,
        ViolationKind::MissingRequiredMember | ViolationKind::MinLength(_)
    );
    let (_, for_absent, for_invalid) = REFUSAL_CODES
        .iter()
        .find(|(name, _, _)| *name == member)?;
    if absent {
        *for_absent
    } else {
        *for_invalid
    }
}

/// The `com-amazonaws-codedeploy-codedeploy-20141006` emulated service: the generated [`ServiceDefinition`] plus
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
        // CodeDeploy does NOT answer a generic `ValidationException` for an input it
        // will not accept: every name and id member has its own pair of modelled
        // error shapes — one for "you did not give me this", one for "what you gave
        // me is not a valid one" — and a client branches on those codes. The
        // framework refuses a modelled constraint before any handler runs, so this
        // seam is the only place those codes can be chosen. A member the table does
        // not name keeps the framework's default wording.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        // The first refused member, which is the one the service names.
        let violation = violations.first()?;
        // The path is rooted at the top-level input member and descends from there
        // (`includeOnlyStatuses.1.member`), so the root segment is the member the
        // caller sent.
        let member = violation.path.split('.').next()?;
        let code = refusal_code(member, &violation.kind)?;
        Some(Refused {
            code: code.to_string(),
            // The clause the framework built from the request: it names the member
            // and the constraint it failed, which is what a caller has to fix.
            message: violation.message(),
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
