// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-s3tables-s3tablebuckets` service: how it claims requests and how it words the
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
//! A sixth seam, `override_protocol`, is the one this service DOES take: every timestamp
//! member of this model declares `@timestampFormat("date-time")`
//! (`smithy_query(query="shape",
//! shape="TableSummary,TableBucketSummary,NamespaceSummary,GetTableResponse")` reports it on
//! each), so this service serves the model's `restJson1` codec with every timestamp in that
//! form — `service_common::date_time_json`, which is where the whole workspace's rendering
//! of such a member lives.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

use aws_smithy_schema_server::{Violation, ViolationKind};
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The one client-input error this service models: `BadRequestException`,
/// `errorKind` client, `httpError` 400, one `message` member
/// (`smithy_query(query="shape", shape="BadRequestException")`). It is a
/// service-common error, so EVERY operation declares it, and the model declares
/// no `ValidationException` anywhere — which is why a boundary refusal is
/// answered under this code rather than under the framework's generic one.
const BAD_REQUEST_CODE: &str = "BadRequestException";

/// `TableBucketName`'s character set and length
/// (`smithy_query(query="shape", shape="TableBucketName")`).
const TABLE_BUCKET_NAME_PATTERN: &str = "^[0-9a-z-]*$";
const TABLE_BUCKET_NAME_MIN: i64 = 3;
const TABLE_BUCKET_NAME_MAX: i64 = 63;

/// `TableName`'s character set and length
/// (`smithy_query(query="shape", shape="TableName")`). The two name shapes of
/// this model differ in BOTH, which is what tells a refused `name` member
/// apart: `CreateTableBucket` names a table bucket with it and the table
/// operations name a table.
const TABLE_NAME_PATTERN: &str = "^[0-9a-z_]*$";
const TABLE_NAME_MIN: i64 = 1;
const TABLE_NAME_MAX: i64 = 255;

/// The one part a namespace has: `CreateNamespaceRequest.namespace` is a
/// `NamespaceList` with `@length(min: 1, max: 1)`
/// (`smithy_query(query="operation", operation="CreateNamespace")`). No other
/// member of this model named `namespace` is bounded at one — the namespace
/// NAME shape is bounded at 255 — so a list longer than this is the only
/// refusal that reaches the multipart clause below.
const NAMESPACE_PARTS_MAX: i64 = 1;

/// The input member a violation path names: its FIRST segment, so a value
/// refused inside a collection (`namespace.1.member`) is named by the input
/// member that carried it rather than by the collection step.
fn violated_member(path: &str) -> &str {
    path.split('.').next().unwrap_or(path)
}

/// The fixed sentence this service states `violation` with, or `None` when the
/// member it refused is not one of the classes this service words itself.
///
/// The class is read off the MEMBER and the CONSTRAINT the value failed — the
/// two facts a violation carries — never off the operation or the rejected
/// value, so one arm covers every operation that takes the member.
fn fixed_sentence(violation: &Violation) -> Option<&'static str> {
    let member = violated_member(&violation.path);
    match (member, &violation.kind) {
        // `TableBucketARN` declares a pattern and no length, so its only
        // value-rejecting constraint is the pattern.
        ("tableBucketARN", ViolationKind::Pattern(_)) => {
            Some(crate::state::INVALID_TABLE_BUCKET_ARN_MESSAGE)
        }
        // `ResourceArn` declares both, and either way the value cannot be a
        // resource ARN at all.
        (
            "resourceArn",
            ViolationKind::Pattern(_) | ViolationKind::MinLength(_) | ViolationKind::MaxLength(_),
        ) => Some(crate::state::INVALID_RESOURCE_ARN_MESSAGE),
        // The namespace LIST's own bound: more parts than a namespace has.
        ("namespace", ViolationKind::MaxLength(NAMESPACE_PARTS_MAX)) => {
            Some(crate::state::MULTIPART_NAMESPACE_MESSAGE)
        }
        // A namespace NAME — the list's element, and the label the namespace and
        // table operations address one by — outside its character set or length.
        (
            "namespace" | "newNamespaceName",
            ViolationKind::Pattern(_) | ViolationKind::MinLength(_) | ViolationKind::MaxLength(_),
        ) => Some(crate::state::INVALID_NAMESPACE_NAME_MESSAGE),
        ("name", ViolationKind::Pattern(pattern)) if pattern == TABLE_BUCKET_NAME_PATTERN => {
            Some(crate::state::INVALID_TABLE_BUCKET_NAME_MESSAGE)
        }
        (
            "name",
            ViolationKind::MinLength(TABLE_BUCKET_NAME_MIN)
            | ViolationKind::MaxLength(TABLE_BUCKET_NAME_MAX),
        ) => Some(crate::state::INVALID_TABLE_BUCKET_NAME_MESSAGE),
        ("name" | "newName", ViolationKind::Pattern(pattern)) if pattern == TABLE_NAME_PATTERN => {
            Some(crate::state::INVALID_TABLE_NAME_MESSAGE)
        }
        (
            "name" | "newName",
            ViolationKind::MinLength(TABLE_NAME_MIN) | ViolationKind::MaxLength(TABLE_NAME_MAX),
        ) => Some(crate::state::INVALID_TABLE_NAME_MESSAGE),
        _ => None,
    }
}

/// How this service answers a request the boundary refused for breaking a
/// modelled constraint: always its own `BadRequestException`, worded with the
/// fixed sentence for the member class when it states one.
///
/// A member class this service has no sentence of its own for keeps the
/// framework's clause, which names the member and the requirement it broke —
/// only the error code changes there, and it changes because the code the
/// framework would otherwise name is a shape this service's model does not
/// declare.
///
/// `None` when there is nothing to state, which leaves the framework's wording.
fn constraint_refusal(violations: &[Violation]) -> Option<Refused> {
    let message = violations
        .iter()
        .find_map(fixed_sentence)
        .map(str::to_string)
        .or_else(|| aws_smithy_schema_server::envelope(violations))?;
    Some(Refused {
        code: BAD_REQUEST_CODE.to_string(),
        message,
    })
}

/// The `com-amazonaws-s3tables-s3tablebuckets` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// This service's wire protocol, built ONCE here because
    /// [`Service::override_protocol`] hands back a borrow of it.
    protocol: service_common::date_time_json::DateTimeRestJson,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            protocol: service_common::date_time_json::DateTimeRestJson::new(),
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
        // service reword it. A value this service's model rules out is its own
        // `BadRequestException` — the only client-input error it declares — and,
        // for a name, a namespace or an ARN, the fixed sentence it states that
        // class with. BOTH the code and the message are replaced together, which
        // is what [`constraint_refusal`] builds.
        //
        // An authentication refusal, and any refusal kind added later, keeps the
        // framework's wording.
        match refusal {
            Refusal::Invalid(violations) => constraint_refusal(violations),
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

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every timestamp member of this model declares
        // `@timestampFormat("date-time")` — a table's and a table bucket's
        // `createdAt`/`modifiedAt`, a namespace's `createdAt`, a maintenance or
        // expiration job's `lastRunTimestamp`, a replicated update's `timestamp` — so
        // every instant this service reports is an RFC 3339 string rather than the
        // protocol's default epoch-seconds number. The codec that applies it is
        // otherwise the model's own `restJson1`.
        Some(&self.protocol)
    }
}
