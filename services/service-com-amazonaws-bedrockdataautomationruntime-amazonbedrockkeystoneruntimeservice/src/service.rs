// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-bedrockdataautomationruntime-amazonbedrockkeystoneruntimeservice` service: how it claims requests and how it words the
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

/// The error code this service names for a request its own rules refused — the one its
/// model declares for invalid input, and the code the handlers return for the rules the
/// framework's walk cannot express.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// One violation, worded the way this service words it.
///
/// Two differences from the framework's conventional wording, both shown by the recorded
/// wire for three separate operations (`inputConfiguration.s3Uri`, `invocationArn`,
/// `resourceARN`): the offending value is NOT echoed, and a `@pattern` is printed
/// without its anchors. Both live in `crate::state`'s wording helpers, which a handler
/// enforcing a rule the walk cannot express reaches too, so one logical failure cannot
/// read two ways.
///
/// The requirement half of each clause is the AWS envelope's own phrasing, restated here
/// for every violation kind so that a request with TWO violations cannot answer with two
/// different styles in one message. A kind this service does not restate (the enum is
/// `#[non_exhaustive]`), and an absent member — which has no value to omit, and which AWS
/// states as `Value null at '…'` — keep the framework's wording.
///
/// Restating each kind is what `services/common/src/validation.rs` stages a shared,
/// framework-derived requirement half to replace, so a restatement cannot drift from the
/// framework's wording. See the PENDING MIGRATION note on
/// `crate::state::pattern_requirement`: this change does not own that migration.
fn clause_for_violation(violation: &Violation) -> String {
    let requirement = match &violation.kind {
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        ViolationKind::Pattern(pattern) => crate::state::pattern_requirement(pattern),
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        // `MissingRequiredMember` and any kind added later: nothing to omit, so the
        // framework's own clause already reads the way this service states it.
        _ => return violation.message(),
    };
    crate::state::violation_clause(&violation.path, &requirement)
}

/// The `com-amazonaws-bedrockdataautomationruntime-amazonbedrockkeystoneruntimeservice` emulated service: the generated [`ServiceDefinition`] plus
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
        // This service signs as `bedrock`, the signing name every Bedrock-family service
        // shares while serving its own endpoint (`bedrock-data-automation-runtime` here).
        // Claiming on that name alone makes all six claimants for every Bedrock request,
        // which the router answers `500 AmbiguousService` — a code no AWS model carries —
        // rather than serving one of them.
        //
        // This is the one `awsJson1_1` model of the six and it publishes no
        // `smithy.api#http` bindings, so its generated `ROUTE_PATH_SEGMENTS` is EMPTY —
        // which on its own claims every path and swallows all five REST peers. An
        // `awsJson` request names its service out of band instead, by the LOCAL name of
        // the service shape (`X-Amz-Target: AmazonBedrockKeystoneRuntimeService.<Op>`),
        // so this service's own shape id — read off its model-derived identity, never
        // written out here — is passed as the RPC argument of the one rule
        // `service_common::bedrock` states for the family: it claims a root `POST` whose
        // target names IT, and nothing else, leaving each REST sibling's URI path space
        // to that sibling alone.
        service_common::bedrock::claims_request(
            parts,
            self.definition(),
            crate::ROUTE_PATH_SEGMENTS,
            Some(self.entry.identity().shape_id()),
        )
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
        // The framework refused the request before any handler ran. This service states
        // such a refusal as its model's own `ValidationException`, in the AWS
        // constraint-violation envelope (`N validation error(s) detected: <clause>; …`)
        // with THIS service's clause wording — see `clause_for_violation`. The code is
        // replaced along with the message, because a client branches on the code.
        //
        // Only `Refusal::Invalid` is reworded: an authentication refusal is the
        // framework's to word, and is not a `ValidationException`.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        let clauses: Vec<String> = violations.iter().map(clause_for_violation).collect();
        if clauses.is_empty() {
            return None;
        }
        Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message: crate::state::validation_envelope(&clauses),
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
