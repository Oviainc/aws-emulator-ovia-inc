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
//!   framework's pre-dispatch envelope — and a request whose BODY cannot be read
//!   into the modelled input shape is answered before any handler sees it.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. EDITED HERE: a value outside a declared constraint is
//!   `ValidationException`, worded from the violations the framework collected.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. EDITED HERE: the framework's `rest` default is
//!   `501 NotImplemented`, which this service never answers.
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
//! # This service's REQUEST LAYER
//!
//! Directory Service Data answers everything it rejects BEFORE an operation
//! handler runs from its own request layer, never from the framework's `rest`
//! default (`501 NotImplemented`). A request meets the layers in this order:
//!
//! 1. **The URI and the method must name an operation.** When they do not, the
//!    request is answered here and nothing reads its body. A request on a method
//!    this service serves operations on (`POST`) reached the layer that names an
//!    operation, so the answer is `200` with the unknown-operation envelope
//!    `{"Output":{"__type":"com.amazon.coral.service#UnknownOperationException"},"Version":"1.0"}`
//!    — the unresolved operation is reported INSIDE the envelope rather than as
//!    a transport failure. A request on any other method never reaches that
//!    layer and is `404` with an `UnknownOperationException` body key.
//! 2. **The body must read into the modelled input shape.** Bytes that are not
//!    JSON, or a value whose JSON type cannot be coerced into the modelled target
//!    type of the member it is bound to, are `400 SerializationException` —
//!    deserialization precedes constraint validation, so neither is a
//!    `ValidationException`. A value's TYPE is what this layer reads, and the
//!    CLASS of value is what its refusal names; what the value says is the
//!    validation layer's business.
//! 3. **A member bound to the request must be present.** `DirectoryId` rides in
//!    the query string (`@required` + `@httpQuery` on all 17 operations), so a
//!    request that omits it matches no route BY REQUEST SHAPE and the framework
//!    reports it as unrouted; it is still this service's `400
//!    ValidationException`, naming the absent member in the same envelope
//!    [`InModelledEnvelope`] answers a body-bound one with, and never a routing
//!    failure.
//!
//! The request layer's own error shapes are not this service's, so they name
//! themselves in `x-amzn-errortype` as `<ShapeName>:<the URI of the namespace
//! that declares them>`; a modelled error is serialized through its own schema,
//! which names it as every other modelled error of this service is named.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema::{Schema, ShapeType};
use aws_smithy_schema_server::constraints;
use aws_smithy_schema_server::constraints::path;
use aws_smithy_types::body::SdkBody;
use serde_json::Value;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, ServerHttpProtocol, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code this service names for a request it refused as invalid: the shape
/// `com.amazonaws.directoryservicedata#ValidationException`, which every operation
/// lists and whose only prose member the model calls `Message`
/// (`smithy_query(query="shape", shape="ValidationException")`).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The HTTP status that shape's `@httpError` declares.
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// The namespace URI of the REQUEST-LAYER error shapes, which this service's
/// model does not declare: they belong to `com.amazon.coral.service`, the
/// namespace the unknown-operation envelope names in its own `__type`
/// (`com.amazon.coral.service#UnknownOperationException`). `x-amzn-errortype`
/// names a shape as `<ShapeName>:<the URI of the namespace that declares it>`.
const REQUEST_LAYER_ERROR_NAMESPACE: &str =
    "http://internal.amazon.com/coral/com.amazon.coral.service/";

/// The media type every response of this restJson1 service carries.
const JSON_CONTENT_TYPE: &str = "application/json";

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
    /// Answers a request whose body cannot be read into the modelled input shape.
    /// Held on the service because [`Service::override_operation`] hands back a
    /// borrow of one of its own operations.
    unreadable_body: UnreadableBody,
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
            unreadable_body: UnreadableBody,
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
/// serialization failure, which this service's request layer answers before this
/// wrapper is ever reached (see [`body_refusal`]).
///
/// Both classes come from the framework's own machinery (the protocol's deserializer
/// and `constraints::validate` over the built input), so this cannot find a different
/// set of violations from the walk that follows it.
///
/// Taken from the tree's shared collecting half, which is also where the value each
/// clause quotes is restored to the bytes the caller sent — including a value the
/// validating walk withheld as it recorded the violation, which this service's clause
/// states.
fn refused_by<O: Operation>(protocol: &dyn ServerHttpProtocol, req: &RtRequest) -> Vec<Violation> {
    ::service_common::validation::refusing_violations_restoring_withheld::<O>(protocol, req)
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
        // A body this service cannot read into the modelled input shape is refused
        // FIRST, because deserialization precedes every later layer: the framework
        // would otherwise report the codec failure as a constraint violation
        // (`ValidationException`), which is a different error class and a different
        // remediation for the caller.
        if let Some(route) = modeled_route(req.method(), request_path(req.uri())) {
            if body_refusal(route, request_body(req)).is_some() {
                return Some(&self.unreadable_body);
            }
        }

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
        // modeled operation. The framework's `rest` default is `501
        // NotImplemented`, which this service never answers: a 501 tells a
        // client the operation exists but is unimplemented, and it is commonly
        // treated as a non-retryable infrastructure failure. Directory Service
        // Data answers one of three things instead, keyed on what the request
        // did reach.
        let path = request_path(req.uri());

        // The URI AND the method name an operation, so what did not resolve is
        // the request's shape: a member the operation binds to the request
        // (`@httpQuery` / `@httpHeader`) is absent, which is what keeps the route
        // from matching. That is this service's own validation refusal, not a
        // routing failure — the caller omitted a parameter, it did not call
        // something that does not exist — so it is answered in the SAME envelope
        // `InModelledEnvelope` answers an absent body member with.
        if let Some(route) = modeled_route(req.method(), path) {
            let absent = absent_bound_members(route, req);
            if let Some(refusal) = validation_exception(self.entry.protocol(), &absent) {
                return Some(refusal);
            }
        }

        // The request uses a method this service serves operations on, so it
        // reached the layer that names one and that layer could not. Not an
        // error on the wire: the status is 200 and the unresolved operation is
        // reported inside the envelope, which is the marker a client probing the
        // surface reads.
        if serves_method(req.method()) {
            return Some(unknown_operation_envelope());
        }

        // Nothing this service serves answers this method, so the URI layer is
        // as far as the request got.
        Some(unknown_operation_not_found())
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

/// The operation this service serves for a request whose body it cannot read
/// into the modelled input shape.
///
/// It is an [`ErasedOperation`] rather than a response built in
/// [`Service::override_operation`] because that seam hands the router a BORROWED
/// operation; the answer is derived from the request the router passes back into
/// [`ErasedOperation::run`], so one value serves every such request.
struct UnreadableBody;

impl ErasedOperation for UnreadableBody {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        // The same decision `override_operation` made, re-derived from the same
        // request: this operation is reached only when it answered `Some`, so the
        // refusal is found again. The fallback keeps the error class right even
        // if it somehow is not.
        let refusal = modeled_route(req.method(), request_path(req.uri()))
            .and_then(|route| body_refusal(route, request_body(req)))
            .unwrap_or(BodyRefusal::NotJson);
        Box::pin(std::future::ready(refusal.response()))
    }
}

/// Why this service cannot read a request body into its modelled input shape.
#[derive(Debug, PartialEq, Eq)]
enum BodyRefusal {
    /// The bytes are not JSON at all, so no member can be named: the service
    /// answers the bare error, with an empty JSON body.
    NotJson,
    /// A value's JSON type cannot be coerced into the modelled target type of the
    /// member it is bound to. The message names the value and the type it could
    /// not become — both facts of the request and the model.
    Uncoercible(String),
}

impl BodyRefusal {
    /// This refusal on the wire: `400 SerializationException`, with the reason in
    /// `Message` when there is one to give.
    fn response(&self) -> RtResponse {
        let body = match self {
            Self::NotJson => "{}".to_string(),
            Self::Uncoercible(message) => message_body(message),
        };
        error_response(
            400,
            "SerializationException",
            REQUEST_LAYER_ERROR_NAMESPACE,
            body,
        )
    }
}

/// Every modeled operation's INPUT schema — the one place this file enumerates
/// the service's surface besides the wrapped operations above.
///
/// Nothing else about the model is written down here: each schema carries its
/// operation's `@http` route (method + URI) and, for every member, the request
/// binding (`@httpQuery` / `@httpHeader` / `@httpLabel`) and the modeled target
/// shape type, so the request layer reads the model off these rather than
/// holding a copy of it. A function rather than a `static`, because a `static`
/// initializer may not read other `static`s.
fn modeled_inputs() -> [&'static Schema; 17] {
    use crate::ops::types as t;
    [
        t::ADD_GROUP_MEMBER_INPUT_SCHEMA,
        t::CREATE_GROUP_INPUT_SCHEMA,
        t::CREATE_USER_INPUT_SCHEMA,
        t::DELETE_GROUP_INPUT_SCHEMA,
        t::DELETE_USER_INPUT_SCHEMA,
        t::DESCRIBE_GROUP_INPUT_SCHEMA,
        t::DESCRIBE_USER_INPUT_SCHEMA,
        t::DISABLE_USER_INPUT_SCHEMA,
        t::LIST_GROUP_MEMBERS_INPUT_SCHEMA,
        t::LIST_GROUPS_FOR_MEMBER_INPUT_SCHEMA,
        t::LIST_GROUPS_INPUT_SCHEMA,
        t::LIST_USERS_INPUT_SCHEMA,
        t::REMOVE_GROUP_MEMBER_INPUT_SCHEMA,
        t::SEARCH_GROUPS_INPUT_SCHEMA,
        t::SEARCH_USERS_INPUT_SCHEMA,
        t::UPDATE_GROUP_INPUT_SCHEMA,
        t::UPDATE_USER_INPUT_SCHEMA,
    ]
}

/// The input schema of the operation this service serves on `method` + `path`,
/// or `None` when the URI names none.
///
/// Matching is on the `@http` route alone — NOT on the request's shape — so a
/// request that names an operation and omits a member bound to the query string
/// still resolves here, which is what separates "this operation does not exist"
/// from "this request is missing a parameter".
fn modeled_route(method: &str, path: &str) -> Option<&'static Schema> {
    modeled_inputs().into_iter().find(|schema| {
        schema
            .http()
            .is_some_and(|route| route.method() == method && path_matches(route.uri(), path))
    })
}

/// Whether this service serves ANY operation on this HTTP method.
///
/// A request layer answers only the methods its operations are served on: one
/// that uses another never reaches the layer that names an operation, so it is
/// answered by the URI layer instead. Read off the model (`@http.method` on
/// every operation) rather than written down, so a service serving more than one
/// method needs no change here.
fn serves_method(method: &str) -> bool {
    modeled_inputs()
        .into_iter()
        .any(|schema| schema.http().is_some_and(|route| route.method() == method))
}

/// A URI's path segments, with the leading and trailing `/` dropped. An empty
/// path (the endpoint root) has none.
fn trimmed_segments(path: &str) -> Vec<&str> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return Vec::new();
    }
    trimmed.split('/').collect()
}

/// Whether a request path matches a modeled `@http` URI pattern: a literal
/// segment must be equal, a `{Label}` segment matches any one segment, and a
/// trailing `{Label+}` matches the rest of the path. A pattern's query literals
/// (everything after `?`) do not take part in path matching.
fn path_matches(pattern: &str, path: &str) -> bool {
    let pattern_path = pattern.split('?').next().unwrap_or(pattern);
    let pattern_segments = trimmed_segments(pattern_path);
    let path_segments = trimmed_segments(path);
    let greedy = pattern_segments
        .last()
        .is_some_and(|segment| segment.starts_with('{') && segment.ends_with("+}"));
    if greedy {
        if path_segments.len() < pattern_segments.len() {
            return false;
        }
    } else if path_segments.len() != pattern_segments.len() {
        return false;
    }
    pattern_segments
        .iter()
        .zip(path_segments.iter())
        .all(|(pattern, actual)| pattern.starts_with('{') || pattern == actual)
}

/// The members `route` binds to the request itself — the query string or a
/// header — that this request does not carry, as the violations a refusal states.
///
/// Every operation of this service binds exactly one member to the request,
/// `DirectoryId` (`@required` + `@httpQuery("DirectoryId")`, on all 17 — see
/// `smithy_query(query="operation")`), and binds nothing to a header, so for
/// this model the absent request-bound members ARE the absent required ones; a
/// model that bound an OPTIONAL member to the request would want the `@required`
/// trait consulted here, which a member `Schema` does not carry. A member bound
/// to the BODY is not read here: an absent one is collected by [`refused_by`],
/// from the framework's own deserializer.
fn absent_bound_members(route: &Schema, req: &RtRequest) -> Vec<Violation> {
    let query = query_pairs(request_query(req.uri()));
    let mut absent = Vec::new();
    for member in route.members() {
        let Some(name) = member.member_name() else {
            continue;
        };
        let carried = if let Some(bound) = member.http_query() {
            query.iter().any(|(key, _)| *key == bound.value())
        } else if let Some(bound) = member.http_header() {
            req.headers().get(bound.value()).is_some()
        } else {
            // Not bound to the request: the body carries it, and the framework
            // reports it absent.
            continue;
        };
        if !carried {
            absent.push(Violation {
                path: path::member(name),
                kind: ViolationKind::MissingRequiredMember,
                value: None,
                sensitive: false,
            });
        }
    }
    absent
}

/// Whether this service can read a body into `route`'s modelled input shape, and
/// why not when it cannot.
///
/// An absent or empty body is fine: a request may carry no body at all, and the
/// members a body would have held are then the validation layer's business. A
/// body that IS present must be a JSON object whose values have the JSON type
/// each member's modelled target can be read from.
fn body_refusal(route: &Schema, body: &[u8]) -> Option<BodyRefusal> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return None;
    }
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return Some(BodyRefusal::NotJson);
    };
    uncoercible_value(route, &value).map(BodyRefusal::Uncoercible)
}

/// The first value in `value` whose JSON type cannot be coerced into the modelled
/// target type of the member it is bound to, as the message naming it.
///
/// Reads the shape, not the request: a member's wire name and its modelled target
/// type both come off the schema. Two levels are reachable and both are checked
/// — the body document itself, which must be the input structure, and each of
/// the input's own members, in the order a JSON object reports them. A value
/// nested INSIDE a list or a map is left to the codec: a member `Schema` in this
/// framework carries its target's KIND but not its target's schema, so a
/// collection's element shape is not reachable from here (the generated codec
/// passes the collection member's own schema down to its elements).
fn uncoercible_value(schema: &Schema, value: &Value) -> Option<String> {
    if !json_type_fits(schema.shape_type(), value) {
        return Some(coercion_message(value, schema.shape_type()));
    }
    let Value::Object(members) = value else {
        return None;
    };
    members
        .iter()
        // A key naming no member is ignored, exactly as the codec ignores it,
        // and a member read from the URI carries no value in the body.
        .filter_map(|(key, held)| Some((member_by_wire_name(schema, key)?, held)))
        .filter(|(member, _)| !is_bound_to_request(member))
        .find_map(|(member, held)| {
            (!json_type_fits(member.shape_type(), held))
                .then(|| coercion_message(held, member.shape_type()))
        })
}

/// The member of `schema` a JSON key names: its `@jsonName` when the model gives
/// one, else its Smithy member name — the same lookup the JSON codec performs.
fn member_by_wire_name<'a>(schema: &'a Schema, key: &str) -> Option<&'a Schema> {
    schema.members().iter().copied().find(|member| {
        let wire = member
            .json_name()
            .map(|name| name.value())
            .or_else(|| member.member_name())
            .unwrap_or_default();
        wire == key
    })
}

/// Whether a member is read from the request rather than from the body, so the
/// body carries no value for it.
fn is_bound_to_request(member: &Schema) -> bool {
    member.http_query().is_some()
        || member.http_header().is_some()
        || member.http_label().is_some()
        || member.http_query_params().is_some()
        || member.http_prefix_headers().is_some()
}

/// Whether a JSON value's type is one the modelled `target` type can be read
/// from.
///
/// The classes are the Smithy ones: a number reads into a numeric shape, a
/// string into a string-shaped one (an enum IS a string shape here, and a
/// `blob`'s base64 text is a string on the wire), an array into a list, an
/// object into a map or a structure, and anything at all into a `document`. A
/// timestamp reads from either a number or a string, because `@timestampFormat`
/// decides which. An integral shape reads only a whole number its width holds: a
/// fraction, or a value past that width, is no more readable than a string.
/// JSON `null` is absence, not a type error — an absent required member is the
/// validation layer's refusal, not the codec's.
fn json_type_fits(target: ShapeType, value: &Value) -> bool {
    if matches!(target, ShapeType::Document) {
        return true;
    }
    match value {
        Value::Null => true,
        Value::Bool(_) => matches!(target, ShapeType::Boolean),
        Value::Number(number) => match target {
            ShapeType::Byte => fits_integral(number, i8::MIN.into(), i8::MAX.into()),
            ShapeType::Short => fits_integral(number, i16::MIN.into(), i16::MAX.into()),
            ShapeType::Integer => fits_integral(number, i32::MIN.into(), i32::MAX.into()),
            ShapeType::Long | ShapeType::BigInteger => {
                fits_integral(number, i64::MIN, i64::MAX)
            }
            ShapeType::Float | ShapeType::Double | ShapeType::BigDecimal | ShapeType::Timestamp => {
                true
            }
            _ => false,
        },
        Value::String(_) => matches!(
            target,
            ShapeType::String
                | ShapeType::Blob
                | ShapeType::Timestamp
                | ShapeType::BigInteger
                | ShapeType::BigDecimal
        ),
        Value::Array(_) => matches!(target, ShapeType::List),
        Value::Object(_) => matches!(
            target,
            ShapeType::Map | ShapeType::Structure | ShapeType::Union
        ),
    }
}

/// Whether `number` is a whole number the inclusive range `min..=max` holds — the
/// range of the integral shape it is being read into.
fn fits_integral(number: &serde_json::Number, min: i64, max: i64) -> bool {
    number.as_i64().is_some_and(|held| (min..=max).contains(&held))
}

/// The message a failed coercion states: the CLASS of value the request sent
/// and the modelled type it could not be read as, which are the two facts a caller
/// has to reconcile. The type is named by its Smithy shape type (`Integer`,
/// `Long`, `Boolean`, …); the article is grammatical.
///
/// CROSS-REFERENCE:
/// `services/service-com-amazonaws-xray-awsxray/src/service/request_layer.rs`
/// states the same sentence for its own request layer, reached a different way
/// (it runs the framework's deserializer rather than walking the body). If a
/// third service needs it, hoist this function and [`json_value_class`] into
/// `services/common` and have all three read it instead of copying it again.
fn coercion_message(value: &Value, target: ShapeType) -> String {
    let named = format!("{target:?}");
    let article = if named.starts_with(['A', 'E', 'I', 'O', 'U']) {
        "an"
    } else {
        "a"
    };
    format!(
        "{} can not be converted to {article} {named}",
        json_value_class(value)
    )
}

/// The class of JSON value this is, as a coercion failure names it:
/// `<JSON type>_VALUE`.
///
/// The refusal names the CLASS, never the value: the recorded refusal of a
/// string bound to an integer member is "STRING_VALUE can not be converted to an
/// Integer" whatever string was sent, which is both what the service says and
/// what keeps a caller from amplifying a long value into a long response. That
/// one recorded name fixes the form; the rest follow it.
fn json_value_class(value: &Value) -> &'static str {
    match value {
        Value::Null => "NULL_VALUE",
        Value::Bool(_) => "BOOLEAN_VALUE",
        Value::Number(_) => "NUMBER_VALUE",
        Value::String(_) => "STRING_VALUE",
        Value::Array(_) => "ARRAY_VALUE",
        Value::Object(_) => "OBJECT_VALUE",
    }
}

/// The path component of a request URI: no scheme, no authority, no query.
///
/// DUPLICATION, deliberate: the framework splits a URI the same way in
/// `vendor/smithy-schema-server/src/rest_xml.rs` (`request_path` /
/// `request_query`), but both are crate-private and the vendored tree is
/// read-only from a service crate, so a service that must tell an absent
/// request-bound member from an unserved URI has to split the URI itself — as
/// `services/service-com-amazonaws-sso-swbportalservice/src/service.rs` already
/// does for the same reason. The reconciliation is to make ONE of them public
/// and have every caller read it.
fn request_path(uri: &str) -> &str {
    // The query is dropped FIRST: a query VALUE may carry `://`, which a scheme
    // scan over the whole URI would mistake for the authority.
    let without_query = uri.split('?').next().unwrap_or(uri);
    match without_query.split_once("://") {
        Some((_scheme, authority_and_path)) => match authority_and_path.find('/') {
            Some(path_start) => &authority_and_path[path_start..],
            None => "/",
        },
        None => without_query,
    }
}

/// The query component of a request URI: everything after the first `?`. See the
/// duplication note on [`request_path`].
fn request_query(uri: &str) -> &str {
    match uri.find('?') {
        Some(mark) => &uri[mark + 1..],
        None => "",
    }
}

/// A query string as `(key, value)` pairs; a bare key carries `None`. The keys
/// are compared exactly as sent, which is how the framework's own route matcher
/// reads a required query binding — so this layer and routing cannot disagree
/// about whether a parameter is there.
fn query_pairs(query: &str) -> Vec<(&str, Option<&str>)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.find('=') {
            Some(mark) => (&pair[..mark], Some(&pair[mark + 1..])),
            None => (pair, None),
        })
        .collect()
}

/// The request's body bytes, or empty when the body is not buffered.
fn request_body(req: &RtRequest) -> &[u8] {
    req.body().bytes().unwrap_or_default()
}

/// A body carrying just the reason, in the `Message` member this service's error
/// shapes name it by. Built through the JSON writer so any text escapes.
fn message_body(message: &str) -> String {
    Value::Object(
        [("Message".to_string(), Value::String(message.to_string()))]
            .into_iter()
            .collect(),
    )
    .to_string()
}

/// One request-layer error response: the status, the shape named in
/// `x-amzn-errortype` as `<ShapeName>:<namespace URI>`, and a JSON body.
fn error_response(status: u16, shape: &str, namespace: &str, body: String) -> RtResponse {
    let mut resp = json_response(status, body);
    resp.headers_mut()
        .insert("x-amzn-errortype", format!("{shape}:{namespace}"));
    resp
}

/// A JSON response with this status and body.
fn json_response(status: u16, body: String) -> RtResponse {
    let mut resp = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::from(body),
    );
    resp.headers_mut().insert("content-type", JSON_CONTENT_TYPE);
    resp
}

/// The envelope for a request that reached the layer naming an operation and
/// resolved none.
///
/// Not an error on the wire: the status is 200 and the unresolved operation is
/// reported inside the envelope, as the marker a client probing the surface
/// reads. The `__type` names the request layer's own shape, which is why the
/// namespace of every other request-layer error is that shape's namespace.
fn unknown_operation_envelope() -> RtResponse {
    json_response(
        200,
        r#"{"Output":{"__type":"com.amazon.coral.service#UnknownOperationException"},"Version":"1.0"}"#
            .to_string(),
    )
}

/// The answer for a request that never reached the layer naming an operation:
/// `404`, with the exception as the body's one key. No operation was named, so
/// there is no member to report a reason for.
fn unknown_operation_not_found() -> RtResponse {
    json_response(404, r#"{"UnknownOperationException":""}"#.to_string())
}
