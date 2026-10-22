// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-directoryservicedata-directoryservicedata` service: how it claims requests and how it words the
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
//! - `override_operation` — which operation serves a request. EDITED HERE: the key
//!   and the handler stay the generated table's, but the operation is served
//!   through [`InModelledEnvelope`], which refuses a request outside a modelled
//!   constraint with this service's own `ValidationException` instead of the
//!   framework's pre-dispatch envelope.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. EDITED HERE: a value outside a declared constraint is
//!   `ValidationException`, worded from the violations the framework collected.
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

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::SerdeError;
use aws_smithy_schema_server::constraints;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    OperationRoutingTable, Refusal, Refused, ServerHttpProtocol, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code this service names for a request it refused as invalid: the shape
/// `com.amazonaws.directoryservicedata#ValidationException`, which every operation
/// lists and whose only prose member the model calls `Message`
/// (`smithy_query(query="shape", shape="ValidationException")`).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The HTTP status that shape's `@httpError` declares.
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// The `com-amazonaws-directoryservicedata-directoryservicedata` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`InModelledEnvelope`] so a request it refuses before the handler runs is
    /// answered in this service's own `ValidationException`. Keyed by the routing key
    /// the protocol resolves — the key the generated table uses — and built from the
    /// SAME shared engine, so a wrapped operation reads and writes the one store.
    in_modelled_envelope: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`InModelledEnvelope`] and keyed by its operation shape name — the routing key
/// `ServerProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(InModelledEnvelope::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut in_modelled_envelope = OperationRoutingTable::new();
        wrap_operations!(
            in_modelled_envelope,
            engine,
            add_group_member::AddGroupMember,
            create_group::CreateGroup,
            create_user::CreateUser,
            delete_group::DeleteGroup,
            delete_user::DeleteUser,
            describe_group::DescribeGroup,
            describe_user::DescribeUser,
            disable_user::DisableUser,
            list_group_members::ListGroupMembers,
            list_groups::ListGroups,
            list_groups_for_member::ListGroupsForMember,
            list_users::ListUsers,
            remove_group_member::RemoveGroupMember,
            search_groups::SearchGroups,
            search_users::SearchUsers,
            update_group::UpdateGroup,
            update_user::UpdateUser,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            in_modelled_envelope,
        }
    }
}

/// One modelled operation, served with the refusals it owes stated in this service's
/// own `ValidationException`.
///
/// The framework's pre-dispatch renderer gets the status (400) and the error code
/// (`ValidationException`) right, but writes the prose into a framework-chosen
/// lowercase `message` key, the same for every restJson1 service. Directory Service
/// Data models the prose member of each of its error shapes as `Message`, so that
/// envelope carries no `Message` at all and an SDK surfaces a constraint refusal with
/// an empty reason.
///
/// So this wrapper collects the refusal itself — the very two classes the framework's
/// own walk collects, from the framework's own deserializer and constraint walk — and
/// renders it through the model's `ValidationException` schema. The envelope is built
/// from the structured [`Violation`]s, never from a rendered response. Nothing else is
/// re-implemented: a request this service does not refuse is handed to the generated
/// `ErasedOperation::run`, which owns deserialize -> validate -> handle -> serialize,
/// telemetry and every modelled error.
struct InModelledEnvelope<O> {
    operation: O,
}

impl<O> InModelledEnvelope<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for InModelledEnvelope<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // Read the protocol out before the await: `&dyn ErasedServiceDefinition`
            // is not `Sync`, and this future is. The protocol itself is.
            let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
            let violations = refused_by::<O>(protocol, req);
            if let Some(refusal) = validation_exception(protocol, &violations) {
                return refusal;
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// Every violation that refuses `req` before `O`'s handler runs: a `@required` member
/// absent, or a value outside a declared constraint. Empty when the request is valid,
/// and empty for a failure that is NOT a refusal — a body the codec cannot read is a
/// serialization failure, which the framework words and which this service hands on
/// unchanged.
///
/// Both classes come from the framework's own machinery (the protocol's deserializer
/// and `constraints::validate` over the built input), so this cannot find a different
/// set of violations from the walk that follows it.
fn refused_by<O: Operation>(protocol: &dyn ServerHttpProtocol, req: &RtRequest) -> Vec<Violation> {
    let Ok(mut deserializer) = protocol.deserialize_input(req, O::INPUT_SCHEMA) else {
        return Vec::new();
    };
    match <O::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
        Ok(input) => constraints::validate(&input, O::INPUT_SCHEMA),
        // Absence is found while READING the request and a bad value while walking
        // what was read; to a caller they are one refusal, so they are one here.
        Err(SerdeError::MissingMember { member_name }) => vec![Violation {
            path: member_name,
            kind: ViolationKind::MissingRequiredMember,
            value: None,
        }],
        Err(_) => Vec::new(),
    }
}

/// This service's answer to `violations`: HTTP 400 `ValidationException` with the
/// prose in the `Message` member the model declares, serialized through that shape's
/// own schema. `None` when there is nothing to refuse — and also when the error
/// cannot be serialized, which leaves the framework's walk to refuse the request in
/// its own envelope rather than answering a 400 with no reason at all.
fn validation_exception(
    protocol: &dyn ServerHttpProtocol,
    violations: &[Violation],
) -> Option<RtResponse> {
    let stated: Vec<Violation> = violations.iter().map(as_stated).collect();
    let refused = crate::ops::types::shared_types::ValidationException {
        message: Some(constraints::envelope(&stated)?),
        // `Reason` names a rule beyond the modelled constraints (an unknown
        // `NextToken`, an attribute this directory does not hold); a member outside
        // its declared constraint has no such reason, and the handlers that do know
        // one set it themselves.
        reason: None,
    };
    protocol
        .serialize_error(
            &refused,
            &crate::ops::types::shared_types::SCHEMA_VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION_STATUS,
        )
        .ok()
}

/// A violation as this service states it: the framework's clause, with a `@pattern`
/// named without the anchors that bracket it.
///
/// Directory Service Data states the pattern of a refused member as the expression
/// alone — `d-[0-9a-f]{10}` for `DirectoryId`'s `^d-[0-9a-f]{10}$` — because the
/// clause already says the whole member must satisfy it, so the anchors would be
/// restating the sentence. Every other clause is the framework's unchanged.
fn as_stated(violation: &Violation) -> Violation {
    match &violation.kind {
        ViolationKind::Pattern(regex) => Violation {
            kind: ViolationKind::Pattern(unanchored(regex).to_string()),
            ..violation.clone()
        },
        _ => violation.clone(),
    }
}

/// `regex` without a leading `^` or a trailing `$`. A trailing `$` escaped by a
/// backslash is a literal dollar in the expression, not an anchor, so it stays.
fn unanchored(regex: &str) -> &str {
    let inner = regex.strip_prefix('^').unwrap_or(regex);
    match inner.strip_suffix('$') {
        Some(stripped) if !stripped.ends_with('\\') => stripped,
        _ => inner,
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
        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // `InModelledEnvelope` so a request it refuses before the handler runs is
        // refused in this service's `ValidationException` rather than in the
        // framework's pre-dispatch envelope, which names no `Message`. A key the
        // model does not name resolves to `None` here exactly as it does in the
        // generated table, so the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.in_modelled_envelope.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // `InModelledEnvelope` refuses a constraint violation itself, in the modelled
        // envelope, so this seam is what words any refusal the FRAMEWORK still
        // renders — a protocol whose body this service does not re-render, and the
        // fallback in `validation_exception`. Wording it here from the same
        // violations keeps the two paths from disagreeing: a value outside a declared
        // bound is this service's `ValidationException` either way. The clauses come
        // from the framework's own renderer so a new constraint class cannot arrive
        // with its own sentence; `as_stated` only names a `@pattern` the way this
        // service names it.
        match refusal {
            Refusal::Invalid(violations) => {
                let stated: Vec<Violation> = violations.iter().map(as_stated).collect();
                Some(Refused {
                    code: VALIDATION_EXCEPTION.to_string(),
                    message: constraints::envelope(&stated)?,
                })
            }
            // Authentication is the framework's to word: this service models
            // `AccessDeniedException` for a caller it refuses, and the request never
            // reached an operation, so there is nothing of this service's own to say.
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
