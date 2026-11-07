// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-marketplacediscovery-awsmarketplacediscovery` service: how it claims requests and how it words the
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

/// The error code this service answers an input-constraint violation with: the modeled
/// `ValidationException`, which every operation of this service declares with
/// `@httpError 400` (the framework's own default code for a refusal, restated here
/// because `override_refusal` replaces code and message together).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// Render one constraint violation the way this service states it.
///
/// The envelope and the clause skeleton are AWS's standard constraint report — `Value
/// '<value>' at '<member path>' failed to satisfy constraint: <requirement>` — and two
/// of its details are rules of how the report is BUILT rather than of the model it is
/// built from:
///
/// * a `@pattern` is reported as the expression a value must satisfy, WITHOUT the `^` /
///   `$` anchoring a model spells it with (see [`unanchored`]);
/// * the echoed value is entity-escaped TWICE (see [`entity_escaped_twice`]).
///
/// * an enumerated value set is listed in the order THIS SERVICE emits it in, which is
///   not the order the model declares its values in (see [`listed_value_set`]).
///
/// Everything else — the member path, the bound a `@length` / `@range` clause states, and
/// the membership of an enumerated value set — is read off the violation the framework
/// reports.
///
/// Returns `None` for a violation class this service has no wording for, which lets
/// [`constraint_envelope`] decline the whole refusal and keep the framework's wording
/// rather than invent a sentence for it ([`ViolationKind`] is `#[non_exhaustive]`, so a
/// class can be added upstream).
pub(crate) fn constraint_clause(violation: &Violation) -> Option<String> {
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
            format!(
                "Member must satisfy enum value set: [{}]",
                listed_value_set(allowed).join(", ")
            )
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    };
    let path = &violation.path;
    Some(match &violation.value {
        Some(shown) => format!(
            "Value '{}' at '{path}' failed to satisfy constraint: {requirement}",
            entity_escaped_twice(shown)
        ),
        // An absent member has no value to echo: that is a fact of the request's shape,
        // not a value the caller sent, so there is nothing to escape.
        None => format!("Value null at '{path}' failed to satisfy constraint: {requirement}"),
    })
}

/// The values of `SearchFilterType`, in the order this service LISTS them in an enum
/// clause.
///
/// Recorded behaviour establishes the order: a refusal of a `filterType` outside the enum
/// reports the set this way, which is NOT the order the model declares the values in
/// (`smithy_query(query="shape",shape="SearchFilterType")` declares
/// `MIN_AVERAGE_CUSTOMER_RATING` first and `NUMBER_OF_PRODUCTS` last). The list is fixed
/// text of the refusal — a constant of the service, carrying nothing from the request —
/// so the same order is reported for every member and every operation whose value set is
/// this enum's.
const SEARCH_FILTER_TYPE_LISTED_ORDER: &[&str] = &[
    "MAX_AVERAGE_CUSTOMER_RATING",
    "FULFILLMENT_OPTION_TYPE",
    "MIN_AVERAGE_CUSTOMER_RATING",
    "PRICING_MODEL",
    "NUMBER_OF_PRODUCTS",
    "DEPLOYED_ON_AWS",
    "PUBLISHER",
    "PRICING_UNIT",
    "CATEGORY",
];

/// Every value set whose listed order recorded behaviour establishes, one row per enum.
const LISTED_VALUE_SET_ORDERS: &[&[&str]] = &[SEARCH_FILTER_TYPE_LISTED_ORDER];

/// The admissible values as this service LISTS them in an enum clause.
///
/// The MEMBERSHIP comes off the violation the framework reports — the enum shape's own
/// values, so it cannot drift from the model. The ORDER does not: this service lists a
/// value set in an order of its own ([`LISTED_VALUE_SET_ORDERS`]), the same on every run
/// and for every member the set constrains, rather than in the order the model happens to
/// declare the values in. A row is used only when the reported membership is exactly that
/// row's, so an enum this service has no recorded order for — and a model change that adds
/// or removes a value — keeps declaration order instead of silently dropping a value from
/// the clause or inventing one.
fn listed_value_set(allowed: &[String]) -> Vec<&str> {
    for order in LISTED_VALUE_SET_ORDERS {
        if order.len() == allowed.len()
            && order
                .iter()
                .all(|value| allowed.iter().any(|reported| reported == value))
        {
            return order.to_vec();
        }
    }
    allowed.iter().map(String::as_str).collect()
}

/// A `@pattern` expression without the anchors a model may spell it with. A `$` escaped
/// by a backslash is a literal dollar sign inside the expression, not the end anchor, so
/// it stays.
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

/// The rejected value as the report echoes it: entity-escaped, then entity-escaped
/// AGAIN, so a caller's `&` reaches the client as `&amp;amp;`.
///
/// Recorded AWS behaviour for this service: a value carrying a markup-significant
/// character comes back doubly escaped, the validation frontend escaping it once while
/// building the report and the response serializer escaping the result again. The rule
/// is the double escape of the request-derived value; the clause around it is unchanged.
/// Only the three characters that are markup-significant in text (`&`, `<`, `>`) are
/// escaped — quotes are left alone, since the clause quotes the value with `'` of its
/// own and no recorded response shows a quote escaped.
fn entity_escaped_twice(value: &str) -> String {
    entity_escaped(&entity_escaped(value))
}

/// One entity-escaping pass over the characters that are markup-significant in text.
/// `&` is handled first by construction: each character is mapped independently, so an
/// entity this pass introduces is never rescanned.
///
/// Deliberately NOT the framework's `xml_escape`: that one also escapes `"` and `'` and
/// emits numeric references for control characters, which would alter values no recorded
/// response attests, and it is an XML-envelope concern while this is a step in building
/// the report's text.
fn entity_escaped(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
    out
}

/// The `ValidationException` message for a refusal: the AWS envelope (`N validation
/// error(s) detected: `) over one clause per violation.
///
/// `None` declines the whole refusal — see [`constraint_clause`].
pub(crate) fn constraint_envelope(violations: &[Violation]) -> Option<String> {
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

/// The `com-amazonaws-marketplacediscovery-awsmarketplacediscovery` emulated service: the generated [`ServiceDefinition`] plus
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
        // This service signs as `aws-marketplace` — a signing name it SHARES with
        // every other AWS Marketplace-family service (Catalog, Reporting, Deployment,
        // Agreement, Entitlement, Metering), each serving its own endpoint
        // (`discovery-marketplace` here). The router's routing key comes from the
        // SigV4 credential scope first, so all of them resolve to `aws-marketplace`
        // and matching on the signing name ALONE makes every one of them claim every
        // request — which the router answers `500 AmbiguousService`.
        //
        // So the claim is narrowed to this service's own modelled wire surface, by the
        // one rule `service-common` states for every service under a shared signing
        // name. This model is `restJson1`: its surface is the URI path space its
        // operations bind — every operation here is `POST /2026-02-05/<operation>`
        // (`OPERATIONS.md`) — which is what `claims_request` matches against the
        // generated `ROUTE_PATH_SEGMENTS` when a service declares no RPC protocol.
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
        let _ = req;
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // This service states a constraint violation in AWS's standard validation
        // report, which `constraint_envelope` builds: the anchors stripped from a
        // modeled `@pattern`, the echoed value entity-escaped twice, and an enumerated
        // value set listed in this service's own order. The code stays the modeled
        // `ValidationException` (HTTP 400), which is what the framework would have
        // answered — only the wording changes.
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
