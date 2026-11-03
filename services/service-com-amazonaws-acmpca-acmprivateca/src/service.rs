// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-acmpca-acmprivateca` service: how it claims requests and how it words the
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
use std::collections::HashMap;
use std::sync::Arc;

/// The `com-amazonaws-acmpca-acmprivateca` emulated service: the generated [`ServiceDefinition`] plus
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
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. A value outside a constraint the MODEL declares is
        // refused here, ahead of any handler, with HTTP 400 and the error code
        // `ValidationException` — the same code the framework picks, kept
        // deliberately: the refusal is the request-validation layer's, not one of
        // this service's own modelled failures (`InvalidArgsException`,
        // `InvalidArnException`), which its handlers answer for a request whose
        // values are well formed but cannot be served. Only the MESSAGE is this
        // service's own ([`constraint_message`]).
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: "ValidationException".to_string(),
                message: constraint_message(violations),
            }),
            // Authentication is the emulator's to answer, not this service's.
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

/// Where a clause states the requirement a value failed, which is also the one place the
/// framework's own clause can be split at to read that requirement back off it.
const CONSTRAINT_PREAMBLE: &str = " failed to satisfy constraint: ";

/// The message a constraint refusal of this service carries: one clause per value that
/// failed, each naming the member path and the requirement it failed, under the count of
/// how many failed.
///
/// What the clauses say is not the value that failed: a refusal names the MEMBER and the
/// bound, never the submitted value, so nothing a caller sent is reflected back in the
/// diagnostic (a member whose value is sensitive is refused like any other).
///
/// The count prefix opens EVERY constraint refusal, whichever constraints the request
/// failed and however deep in it they sit: it states how many values failed, so a caller
/// reading the message learns that a second bad member is being reported even when it
/// parses only the first clause.
fn constraint_message(violations: &[Violation]) -> String {
    let stated = stated_order(violations);
    let clauses: Vec<String> = stated.iter().map(|violation| clause(violation)).collect();
    if clauses.is_empty() {
        return "invalid request".to_string();
    }
    let errors = if clauses.len() == 1 { "error" } else { "errors" };
    format!(
        "{} validation {errors} detected: {}",
        clauses.len(),
        clauses.join("; ")
    )
}

/// How wide the table an enumerated type's admissible values are held in is, and so the
/// modulus a value's bucket is taken at.
const VALUE_SET_BUCKETS: u32 = 32;

/// The order an enumerated type's admissible set is STATED in: the order the set's own
/// hash table is walked in, which is each value's bucket ascending and — where two values
/// share a bucket — the reverse of the order the model declares them in, the
/// later-declared value ahead of the earlier one.
///
/// The order belongs to the enumerated TYPE rather than to a member or an operation
/// carrying it, and it is computed from the values themselves: one type is stated the same
/// way under every member that targets it, at any depth of the request, and a type the
/// model gains is stated without anything being added here.
fn stated_value_set(allowed: &[String]) -> Vec<&str> {
    // Declaration order reversed FIRST, so the stable sort below leaves two values of one
    // bucket in that reverse order.
    let mut stated: Vec<&str> = allowed.iter().rev().map(String::as_str).collect();
    stated.sort_by_key(|value| value_bucket(value));
    stated
}

/// Which bucket of the table `value` sits in: its 31-based string hash — the hash so far
/// multiplied by 31 plus the next UTF-16 code unit, wrapping at 32 bits — reduced to the
/// table's width.
///
/// Two other crates in this tree model a hash-table walk of their own
/// (`services/service-com-amazonaws-schemas-schemas/src/ops/get_discovered_schema.rs`,
/// `services/service-com-amazonaws-cloudformation-cloudformation/src/state/template_validation.rs`),
/// each private to its own operation and each with the derivation ITS order needs: a hash
/// spread by its own high half, into a table whose width grows with the number of entries.
/// This service's walk is a different table — no spreading, a fixed width, and a bucket's
/// values stated in the reverse of the order they went in — and that derivation is what
/// reproduces the sets this service states, so it is taken here rather than borrowed from an
/// order that is not this one. The hash itself is the same four-line fold in all three: too
/// small to pull a crate in for, and not a digest, since it only decides a listing order
/// over model-declared values and reads nothing a caller sent.
fn value_bucket(value: &str) -> u32 {
    value.encode_utf16().fold(0u32, |hash, unit| {
        hash.wrapping_mul(31).wrapping_add(u32::from(unit))
    }) % VALUE_SET_BUCKETS
}

/// The order the clauses are stated in: the members in the order the validating walk
/// found them, and — where ONE member failed more than one of its constraints — the
/// EXPRESSION it must match before the size it must fit, because the expression is the
/// shape of the value and the length is a bound on it.
///
/// Each violation's key is computed ONCE, in a single pass that remembers where each member
/// was first reported, rather than inside the comparator: a comparator that rescanned the
/// slice would compare paths once per pair it is asked about, where this compares each path
/// once. The validating walk also hands over at most its own ceiling of violations
/// (`vendor/smithy-schema-server/src/constraints.rs`), so neither the clause count nor the
/// message's length is something one request chooses.
fn stated_order(violations: &[Violation]) -> Vec<&Violation> {
    let mut first_reported_at: HashMap<&str, usize> = HashMap::with_capacity(violations.len());
    let mut keyed: Vec<((usize, bool), &Violation)> = violations
        .iter()
        .enumerate()
        .map(|(position, violation)| {
            let member = *first_reported_at
                .entry(violation.path.as_str())
                .or_insert(position);
            (
                (member, !matches!(violation.kind, ViolationKind::Pattern(_))),
                violation,
            )
        })
        .collect();
    // A stable sort, so two violations this key cannot separate stay in the order the
    // walk found them.
    keyed.sort_by_key(|(key, _)| *key);
    keyed.into_iter().map(|(_, violation)| violation).collect()
}

/// One failed value's clause: the member path and the requirement that value failed.
///
/// The requirement is the framework's own sentence for the constraint, read back off the
/// clause it writes, so a constraint cannot be worded one way here and another there.
/// Three things about the clause are this service's: the submitted value is not stated; a
/// `@pattern` is quoted as the EXPRESSION the value must match rather than as the model's
/// anchored literal — the anchors make it a whole-value match, which "must satisfy regular
/// expression pattern" already says; and an enumerated type's admissible set is listed in
/// the order this service states that type in ([`stated_value_set`]), not in the order the
/// model happens to declare its values.
fn clause(violation: &Violation) -> String {
    let stated = violation.message();
    let requirement = match stated.split_once(CONSTRAINT_PREAMBLE) {
        Some((_, requirement)) => requirement,
        // A clause in some other shape carries no requirement to restate; the framework's
        // own wording stands.
        None => return stated,
    };
    let requirement = match &violation.kind {
        ViolationKind::Pattern(pattern) => {
            let opened = pattern.strip_prefix('^').unwrap_or(pattern);
            let unanchored = opened.strip_suffix('$').unwrap_or(opened);
            requirement.replace(pattern.as_str(), unanchored)
        }
        // The set the framework listed in the model's declaration order, restated in the
        // order this service lists that enumerated type's values in. Both renderings are
        // built from the same values, so the two cannot drift.
        ViolationKind::EnumValue(allowed) => requirement.replace(
            &allowed.join(", "),
            &stated_value_set(allowed).join(", "),
        ),
        _ => requirement.to_string(),
    };
    // An absent member has no value to name, and the clause says so; a value that is
    // present — including one the walk withheld, which was still sent — is not named at
    // all.
    let held = if violation.value.is_some() || violation.sensitive {
        ""
    } else {
        " null"
    };
    format!(
        "Value{held} at '{}'{CONSTRAINT_PREAMBLE}{requirement}",
        violation.path
    )
}
