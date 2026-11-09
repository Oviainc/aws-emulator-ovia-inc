// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-computeoptimizer-computeoptimizerservice` service: how it claims requests and how it words the
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
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The error code this service answers an input-constraint violation with.
///
/// Compute Optimizer models `InvalidParameterValueException` (HTTP 400, "The value
/// supplied for the input parameter is out of range or not valid") as the ONE
/// bad-input client error of every operation that takes an input, and models no
/// `ValidationException` shape at all — so that is the only code a client written
/// against this API can catch for a value its model's `@range`, `@length`, `@pattern`
/// or `@enum` rules refuse, or for a required member it left out. Restated here
/// because `override_refusal` replaces the code and the message together.
const INVALID_PARAMETER_VALUE_EXCEPTION: &str = "InvalidParameterValueException";

/// The segment a constraint walk spells where it steps from a collection into one of its
/// elements; the segment before it is the element's position or key, not a member name.
const COLLECTION_STEP: &str = "member";

/// The input parameter a refused value sat at, as this service NAMES a parameter.
///
/// A constraint walk reports a POSITION rather than a parameter: `maxResults` for a scalar,
/// `filters.1.member.name` for a member of the first element of a list. The parameter is the
/// deepest member that position names, so the collection steps are dropped (the `member`
/// segment and the index or key before it), and the member's lowerCamel name is written as
/// the words it is made of, each capitalised — `maxResults` is the `Max Results` of the
/// request.
fn parameter_at(path: &str) -> String {
    let mut named: Vec<&str> = Vec::new();
    for segment in path.split('.') {
        if segment == COLLECTION_STEP {
            named.pop();
            continue;
        }
        named.push(segment);
    }
    in_words(named.last().copied().unwrap_or(path))
}

/// A lowerCamel member name as the words it is made of, each capitalised: `maxResults`
/// becomes `Max Results`. A new word starts at each uppercase letter, which is where a
/// lowerCamel name marks one — so a name carrying an acronym is worded with the acronym
/// capitalised rather than upper-cased (`instanceArn` becomes `Instance Arn`), which is the
/// one general derivation a member name supports without a per-acronym table.
fn in_words(member: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    for character in member.chars() {
        if words.is_empty() || character.is_uppercase() {
            words.push(String::new());
        }
        let word = words.last_mut().expect("a word has been started");
        if word.is_empty() {
            word.extend(character.to_uppercase());
        } else {
            word.push(character);
        }
    }
    words.join(" ")
}

/// How this service states a refusal of a value its model does not accept: it NAMES the
/// input parameter at fault — `Invalid Max Results.` for a paging size outside its range —
/// rather than restating the constraint the value failed. A refusal carrying more than one
/// violation names the first, which is the one the walk reached first.
fn invalid_parameter_message(violations: &[::aws_smithy_schema_server::constraints::Violation]) -> Option<String> {
    let first = violations.first()?;
    Some(format!("Invalid {}.", parameter_at(&first.path)))
}

/// The `com-amazonaws-computeoptimizer-computeoptimizerservice` emulated service: the generated [`ServiceDefinition`] plus
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
        // service reword it.
        //
        // Every input-constraint violation this API refuses is an
        // `InvalidParameterValueException` (HTTP 400 — the status the framework
        // would also have answered, so only the code and the wording change):
        // that shape is the one bad-input error its operations model, and the
        // model declares no `ValidationException` at all, so answering the
        // framework's generic code hands a client an error code its own model
        // cannot name and no typed handler can catch. The MESSAGE is this
        // service's own: it names the input parameter at fault
        // ([`invalid_parameter_message`]) rather than restating the bound, and a
        // violation the walk cannot attribute to a parameter keeps the
        // framework's sentence.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure, and this service models its own 403s for it.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: INVALID_PARAMETER_VALUE_EXCEPTION.to_string(),
                message: invalid_parameter_message(violations)?,
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

#[cfg(test)]
mod tests {
    use super::*;
    use ::aws_smithy_schema_server::constraints::{Violation, ViolationKind};

    /// The parameter a refusal names is the deepest MEMBER of the position the walk
    /// reported, written as the words the member name is made of — whatever collection
    /// steps the position carries on the way there.
    #[test]
    fn a_refusal_names_the_parameter_the_refused_value_sat_at() {
        let cases: [(&str, &str); 7] = [
            ("maxResults", "Max Results"),
            ("status", "Status"),
            ("resourceType", "Resource Type"),
            ("orderBy.order", "Order"),
            ("filters.1.member.name", "Name"),
            ("scope.name", "Name"),
            ("recommendationPreferenceNames.2.member", "Recommendation Preference Names"),
        ];
        for (path, expected) in cases {
            let named = parameter_at(path);
            assert_eq!(
                named, expected,
                "case {path:?}: expected {expected:?}, got {named:?}"
            );
        }
    }

    /// Every violation kind is stated the same way — by naming the parameter — and a
    /// refusal carrying none has nothing to name, so the framework's wording is kept.
    #[test]
    fn every_violation_kind_is_stated_by_naming_its_parameter() {
        let kinds = [
            ViolationKind::MissingRequiredMember,
            ViolationKind::MaxValue("100".to_string()),
            ViolationKind::MinValue("0".to_string()),
            ViolationKind::MaxLength(1000),
            ViolationKind::EnumValue(vec!["Active".to_string()]),
        ];
        for kind in kinds {
            let violations = vec![Violation {
                path: "maxResults".to_string(),
                kind: kind.clone(),
                value: Some("101".to_string()),
                sensitive: false,
            }];
            let message = invalid_parameter_message(&violations);
            assert_eq!(
                message.as_deref(),
                Some("Invalid Max Results."),
                "case {kind:?}: expected the parameter to be named, got {message:?}"
            );
        }
        assert_eq!(
            invalid_parameter_message(&[]),
            None,
            "a refusal carrying no violation names no parameter"
        );
    }
}
