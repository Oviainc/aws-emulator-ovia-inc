// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice` service: how it claims requests and how it words the
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
//!
//! ## The front door
//!
//! This service is not reached directly: the AWS gateway in front of it judges the
//! request envelope BEFORE the service runs, and a request it will not forward is
//! refused with its own HTTP 403 `ForbiddenException` (`"Forbidden"`,
//! [`crate::scaffolded::FrontDoorRefusal::forbidden`]) — a shape the model does not
//! declare, carrying nothing of the request.
//!
//! What the gateway refuses here is an OVERSIZED `@sensitive` payload: a value past the
//! maximum length its modelled shape declares. Such a payload is dropped at the gateway
//! instead of being described back to the caller, so the answer is the opaque 403 rather
//! than the modelled 400 `ValidationException` the walk would otherwise state — which
//! for a `@sensitive` member could only name the member and the bound anyway (see
//! `constraint_clause` below). Every OTHER constraint violation, including one on the
//! same member for being too SHORT or for naming no supported value, stays the modelled
//! `ValidationException`.
//!
//! WHY THAT PREDICATE, and what is NOT claimed by it: what AWS recorded is one fact —
//! a secret past `SecretString`'s modelled maximum is answered 403 `ForbiddenException`
//! while an in-limit one reaches the service. AWS's own gateway rule behind that 403 is
//! not observable from here: a byte threshold on the whole body cannot be derived from
//! the recording either, because the in-limit request that DOES reach the service and
//! the over-limit one that does not differ by a single character, so no body size
//! separates them, and a threshold low enough to separate them would refuse requests the
//! model declares valid (a 15000-character secret, a full 50-entry tag map). So the
//! refusal is keyed to the one thing this emulator can observe and the model can state:
//! the modelled maximum of the payload the request would hand over, on the member whose
//! value may not be echoed. `@sensitive` is a STAND-IN for the unknown gateway rule — it
//! marks the payload a gateway treats as opaque — and the bound is read off the model,
//! never written here, so an oversized NON-sensitive member stays the modelled 400 until
//! a recording says otherwise.
//!
//! [`EmulatedService::front_door_refuses`] is that check, reached through
//! [`Service::override_operation`] because the refusal PRECEDES the modelled walk:
//! constraint validation runs before a handler, so no handler can answer ahead of it.
//! The request's auth schemes are unaffected — the router reads them off the MODELLED
//! operation the routing key names — so a front-door refusal is still only reached by a
//! request whose signature verified.

use aws_smithy_schema::serde::SerializableStruct;
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::validate;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, Refusal, Refused, Service,
    Violation, ViolationKind,
};
use std::sync::Arc;

use crate::ops::types;
use crate::scaffolded::FrontDoorRefusal;

/// The error code this service answers an input-constraint violation with: the
/// modelled `ValidationException`, which every operation of this service declares
/// with `@httpError 400` (`smithy_query(query="service_info")` -> `common_errors`).
/// It is the framework's own default code for a refusal too, restated here because
/// `override_refusal` replaces the code and the message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The members this service marks `@sensitive`, as a validation path spells them.
///
/// `smithy_query(query="shape",shape="SecretString")`: `SecretString` is the only
/// shape in this model carrying `sensitive: true`, and it is reached as
/// `DeploymentParameterInput$secretString`, so a violation on it is reported at
/// `deploymentParameter.secretString` — the LAST segment is the member whose value
/// would be echoed.
const SENSITIVE_MEMBERS: &[&str] = &["secretString"];

/// Whether `path` ends in a member this service marks `@sensitive`.
fn names_a_sensitive_member(path: &str) -> bool {
    let member = path.split('.').next_back().unwrap_or(path);
    SENSITIVE_MEMBERS.contains(&member)
}

/// Whether `violation` is the one the front door answers for: a `@sensitive` payload
/// past the MAXIMUM length its modelled shape declares.
///
/// The bound itself is never spelled here — it is read off the violation the model's own
/// validator reported — so this holds for every `@sensitive` length-bounded member of
/// this model, and moves with the model if a bound changes. A violation on the same
/// member for being too SHORT is not this refusal: it is the modelled
/// `ValidationException`, which states the member and the bound without echoing the
/// value. See the module docs for why the predicate is keyed this way and what it does
/// not claim.
fn is_oversized_sensitive_payload(violation: &Violation) -> bool {
    matches!(violation.kind, ViolationKind::MaxLength(_))
        && names_a_sensitive_member(&violation.path)
}

/// One refusal the front door answers with, as a pre-dispatch operation.
///
/// WHY this is a seam and not a line in a handler: the refusal happens BEFORE the
/// modelled walk — the gateway judges the request envelope ahead of the service — and
/// the walk refuses a modelled constraint violation before any handler runs, so the
/// answer is not reachable from inside `handle`. [`Service::override_operation`] is the
/// one hook that sees the whole request ahead of that walk.
struct FrontDoorAnswer(FrontDoorRefusal);

impl ErasedOperation for FrontDoorAnswer {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        _req: &'r aws_smithy_runtime_api::http::Request,
        _ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // Rendered through this service's own protocol, so the refusal carries the
            // same envelope (and the same `x-amzn-errortype` header) as every other
            // error it answers. The status and the error code come from the refusal
            // itself: the shape is not in the model, so there is no modelled error to
            // read them off.
            svc.definition()
                .protocol()
                .serialize_error(
                    &self.0,
                    &crate::scaffolded::FRONT_DOOR_SCHEMA,
                    self.0.code,
                    self.0.status,
                )
                .unwrap_or_else(|e| svc.definition().framework_error_response(e))
        })
    }
}

/// Render one constraint violation the way this service states it.
///
/// The envelope and the clause skeleton are AWS's standard constraint report —
/// `Value '<value>' at '<member path>' failed to satisfy constraint:
/// <requirement>` — and two of its details are rules of how the report is BUILT
/// rather than of the model it is built from:
///
/// * a `@pattern` is reported as the expression a value must satisfy, WITHOUT the
///   `^` / `$` anchoring the model spells it with (see [`unanchored`]): Smithy's
///   `@pattern` is an unanchored find, so those characters are the model's way of
///   making the match total, not part of the character class a client may send;
/// * a `@sensitive` member's value is NOT echoed — the clause names the member and
///   the constraint and stops there, so a rejected secret never reaches a client's
///   logs through an error body.
///
/// Everything else — the member path, the bound a `@length` clause states, the
/// membership of an enumerated value set — is read off the violation the framework
/// reports.
///
/// Returns `None` for a violation class this service has no wording for, which lets
/// [`constraint_envelope`] decline the whole refusal and keep the framework's
/// wording rather than invent a sentence for it ([`ViolationKind`] is
/// `#[non_exhaustive]`, so a class can be added upstream).
fn constraint_clause(violation: &Violation) -> Option<String> {
    let requirement = match &violation.kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
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
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        ),
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    };
    let subject = match (&violation.kind, &violation.value) {
        // An absent member is reported as the null it was: a fact of the request's
        // shape, not a rendering of a value the caller sent.
        (ViolationKind::MissingRequiredMember, _) | (_, None) => "Value null".to_string(),
        // A `@sensitive` member names itself and nothing of what it held.
        (_, Some(_)) if names_a_sensitive_member(&violation.path) => "Value".to_string(),
        (_, Some(shown)) => format!("Value '{shown}'"),
    };
    Some(format!(
        "{subject} at '{}' failed to satisfy constraint: {requirement}",
        violation.path
    ))
}

/// A `@pattern` expression without the anchors a model may spell it with. A `$`
/// escaped by a backslash is a literal dollar sign inside the expression, not the
/// end anchor, so it stays.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// The `ValidationException` message for a refusal: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation.
///
/// `None` declines the whole refusal — see [`constraint_clause`].
fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations
        .iter()
        .map(constraint_clause)
        .collect::<Option<Vec<_>>>()?;
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The `com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// Answers a request the front door will not forward: HTTP 403 `ForbiddenException`.
    forbidden: FrontDoorAnswer,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            forbidden: FrontDoorAnswer(FrontDoorRefusal::forbidden()),
        }
    }

    /// Whether the front door refuses this request outright — see the module docs.
    ///
    /// The input is read through the PROTOCOL's own binding against the operation's own
    /// model-derived input schema, so the value judged is the value dispatch would hand
    /// the handler (URI labels percent-decoded, body members parsed), and the bound
    /// checked is the model's, through the same validator the walk runs.
    /// Deserialization does not enforce constraints, which is what lets the gateway
    /// judge a value the modelled constraint would otherwise have refused.
    ///
    /// One arm per operation whose input carries a member the gateway judges — today
    /// `PutDeploymentParameter`, the only operation of this model carrying a
    /// `@sensitive` member. Every other operation's request has nothing for the front
    /// door to judge, so the modelled walk answers it.
    fn front_door_refuses(&self, req: &aws_smithy_runtime_api::http::Request) -> bool {
        let protocol = self.definition().protocol();
        let Ok(key) = protocol.routing_key(req) else {
            return false;
        };
        if key
            != <crate::ops::put_deployment_parameter::PutDeploymentParameter<Eng> as Operation>::NAME
        {
            return false;
        }
        oversized_sensitive_payload::<types::PutDeploymentParameterInput>(
            protocol,
            req,
            types::PUT_DEPLOYMENT_PARAMETER_INPUT_SCHEMA,
        )
    }
}

/// Whether the request carries a `@sensitive` payload past its modelled maximum length,
/// read through `protocol` against `schema`.
///
/// Generic over the input shape so a second operation carrying a `@sensitive` member
/// needs one call here rather than a second copy of the judgement. A request the protocol
/// cannot bind at all is NOT judged: the framework's own refusal answers it, and the
/// gateway's answer to a malformed envelope is not this one.
///
/// COST, knowingly paid: this binds and validates the request once here and dispatch
/// binds and validates it again, so a request carrying a large payload is parsed twice.
/// The framework offers no seam for handing the already-bound input on to dispatch (the
/// pre-dispatch hook answers with an operation, not with an input), and the alternative —
/// answering from inside the handler — cannot work at all, because the modelled walk
/// refuses the request before any handler runs. The work is bounded by the same payload
/// the request already carries.
fn oversized_sensitive_payload<I>(
    protocol: &dyn smithy_server_runtime::ServerHttpProtocol,
    req: &aws_smithy_runtime_api::http::Request,
    schema: &'static Schema,
) -> bool
where
    I: DeserializableStruct + SerializableStruct,
{
    let Ok(mut deserializer) = protocol.deserialize_input(req, schema) else {
        return false;
    };
    let Ok(input) = I::deserialize(&mut *deserializer) else {
        return false;
    };
    validate(&input, schema)
        .iter()
        .any(is_oversized_sensitive_payload)
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service signs as `aws-marketplace` — a signing name it SHARES with
        // every other AWS Marketplace-family service (Catalog, Reporting, Discovery,
        // Agreement, Entitlement, Metering), each serving its own endpoint
        // (`deployment-marketplace` here). The router's routing key comes from the
        // SigV4 credential scope first, so all of them resolve to `aws-marketplace`
        // and matching on the signing name ALONE makes every one of them claim every
        // request — which the router answers `500 AmbiguousService`.
        //
        // So the claim is narrowed to this service's own modelled wire surface, by the
        // one rule `service-common` states for every service under a shared signing
        // name. This model is `restJson1`: its surface is the URI path space its
        // operations bind (`/catalogs/…`, `/tags/…`), disjoint from every sibling's,
        // which is what `claims_request` matches against the generated
        // `ROUTE_PATH_SEGMENTS` when a service declares no RPC protocol.
        service_common::claims_request(
            parts,
            self.entry.identity().signing_name(),
            crate::ROUTE_PATH_SEGMENTS,
            None,
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
        //
        // This service is reached through a gateway that answers before it (see the
        // module docs): a request carrying a `@sensitive` payload past its modelled
        // maximum is refused there and never reaches the modelled walk at all. That
        // answer is pre-dispatch, which is what this hook is.
        self.front_door_refuses(req)
            .then_some(&self.forbidden as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // This service states a constraint violation in AWS's standard validation
        // report, which `constraint_envelope` builds: a modelled `@pattern` quoted
        // WITHOUT its `^` / `$` anchors, and a `@sensitive` member's value not
        // echoed at all. The code stays the modelled `ValidationException` (HTTP
        // 400), which is what the framework would have answered — only the wording
        // changes.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: constraint_envelope(violations)?,
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
