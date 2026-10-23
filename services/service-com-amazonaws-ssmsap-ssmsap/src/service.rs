// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-ssmsap-ssmsap` service: how it claims requests and how it words the
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
//!   before any handler ran. THIS service uses it: it keeps the framework's
//!   `ValidationException` envelope, clause and echoed value, and reports a
//!   `@pattern` requirement as the expression without the model's outer `^`/`$`
//!   anchors (see the inline note on that method).
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

/// The error code this service answers an input-constraint violation with: the
/// modelled `ValidationException` (`com.amazonaws.ssmsap#ValidationException`,
/// `@httpError 400`), which every operation of this service declares and which is
/// also the code the framework's own refusal renderer picks. Restated here because
/// [`Service::override_refusal`] replaces the code and the message together, and a
/// message without its matching code is worse than leaving both alone.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// A `@pattern` expression without the anchors the model spells it with.
///
/// A model writes `^…$` to turn Smithy's unanchored find into a whole-string match;
/// that anchoring is how the rule is SPELLED, not part of the expression the service
/// reports, so a client that re-validates locally with the expression it read out of
/// the message applies the one AWS states. Only the OUTER anchors go: a `$` or `^`
/// inside an alternation belongs to the expression, and a `$` escaped by a backslash
/// is a literal dollar sign.
///
/// The same rule is inlined in ~24 other service crates
/// (`grep -rln 'fn unanchored' services/*/src`) rather than shared, because
/// `services/common/src/validation.rs` states ("Still to migrate here (staged, not
/// yet wired)") that moving it — together with the requirement and subject halves —
/// is one change of its own and not a side effect of a single service's issue. This
/// is another such copy, pinned by this crate's
/// `tests/all/validation_message_rendering.rs`; the migration that moves the rule
/// owns this copy too.
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

/// The requirement one violation states, as this service phrases it: the framework's
/// own sentence for every class, with a `@pattern` reported [`unanchored`].
///
/// Returns `None` for a violation class this service has no wording for, which lets
/// [`constraint_envelope`] decline the whole refusal and keep the framework's
/// wording rather than invent a sentence for it ([`ViolationKind`] is
/// `#[non_exhaustive]`, so a class can be added upstream).
fn requirement(kind: &ViolationKind) -> Option<String> {
    Some(match kind {
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
    })
}

/// One violation's clause: the submitted value, the input position that carries it
/// and the requirement it failed.
///
/// The value is echoed as the framework recorded it — this message is the only place
/// the server reports the offending input back, and a caller comparing the echo
/// against what it sent must see its own bytes. An ABSENT member has no submitted
/// value, so it is stated as the `null` it was.
fn constraint_clause(violation: &Violation) -> Option<String> {
    let requirement = requirement(&violation.kind)?;
    Some(match &violation.value {
        Some(submitted) => format!(
            "Value '{submitted}' at '{}' failed to satisfy constraint: {requirement}",
            violation.path
        ),
        None => format!(
            "Value null at '{}' failed to satisfy constraint: {requirement}",
            violation.path
        ),
    })
}

/// The `ValidationException` message for a refusal: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation, joined with
/// `; `.
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

/// The `com-amazonaws-ssmsap-ssmsap` emulated service: the generated [`ServiceDefinition`] plus
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
        // The ONE difference from the framework's own wording is the requirement a
        // `@pattern` violation states: the expression without the model's outer
        // anchors (see [`unanchored`]). The count, the clause, the echoed value and
        // the envelope stay the framework's, and the code stays the modelled
        // `ValidationException` (HTTP 400) the framework would have answered — so
        // only the regex a caller reads out of the message changes.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `@pattern` this service's model declares, paired with the expression a
    /// requirement must name: the modelled spelling with its OUTER anchors removed.
    /// Source: `smithy_query(query="shape", shape="AppRegistryArn,ApplicationId,\
    /// ComponentId,DatabaseId,NextToken,OperationId,SAPInstanceNumber,SID,SsmSapArn,\
    /// TagKey", model_path="services/service-com-amazonaws-ssmsap-ssmsap/smithy-model/\
    /// com-amazonaws-ssmsap-ssmsap.json")`.
    const MODELLED_PATTERNS: &[(&str, &str, &str)] = &[
        ("ApplicationId", r"^[\w\d\.-]+$", r"[\w\d\.-]+"),
        ("ComponentId", r"^[\w\d-]+$", r"[\w\d-]+"),
        (
            "AppRegistryArn",
            r"^arn:aws:servicecatalog:[a-z0-9:\/-]+$",
            r"arn:aws:servicecatalog:[a-z0-9:\/-]+",
        ),
        ("NextToken", r"^.{16,2048}$", r".{16,2048}"),
        (
            "OperationId",
            r"^[{]?[0-9a-fA-F]{8}-([0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}[}]?$",
            r"[{]?[0-9a-fA-F]{8}-([0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}[}]?",
        ),
        ("SID", r"^[A-Z][A-Z0-9]{2}$", r"[A-Z][A-Z0-9]{2}"),
        ("SAPInstanceNumber", r"^[0-9]{2}$", r"[0-9]{2}"),
        (
            "TagKey",
            r"^(?!aws:)[a-zA-Z+-=._:/]+$",
            r"(?!aws:)[a-zA-Z+-=._:/]+",
        ),
        // `DatabaseId` spells only the end anchor, which is still an outer one.
        ("DatabaseId", r"[\w\d]+$", r"[\w\d]+"),
        // Only the OUTER anchors go: the `$` and `^` either side of `SsmSapArn`'s
        // alternation belong to its branches, so they are part of the expression.
        (
            "SsmSapArn",
            r"^arn:(.+:){2,4}.+$|^arn:(.+:){1,3}.+\/.+$",
            r"arn:(.+:){2,4}.+$|^arn:(.+:){1,3}.+\/.+",
        ),
    ];

    #[test]
    fn every_modelled_pattern_is_reported_without_its_outer_anchors() {
        for (shape, modelled, expected) in MODELLED_PATTERNS {
            let reported = unanchored(modelled);
            assert_eq!(
                reported, *expected,
                "case {shape:?}: expected the requirement to name {expected:?}, got \
                 {reported:?}"
            );
        }
    }

    #[test]
    fn an_escaped_trailing_dollar_is_a_literal_and_stays() {
        // A `$` preceded by an odd number of backslashes is a dollar sign inside the
        // expression rather than the end anchor; one past an even number is the anchor.
        let cases: &[(&str, &str)] = &[
            (r"^cost\$", r"cost\$"),
            (r"^cost\$$", r"cost\$"),
            (r"^cost\\$", r"cost\\"),
            ("^$", ""),
        ];
        for (modelled, expected) in cases {
            let reported = unanchored(modelled);
            assert_eq!(
                reported, *expected,
                "case {modelled:?}: expected {expected:?}, got {reported:?}"
            );
        }
    }

    #[test]
    fn only_the_pattern_requirement_differs_from_the_frameworks_own_wording() {
        let pattern = Violation {
            path: "applicationId".to_string(),
            kind: ViolationKind::Pattern(r"^[\w\d\.-]+$".to_string()),
            value: Some("bad app id!".to_string()),
        };
        let stated = constraint_envelope(std::slice::from_ref(&pattern))
            .expect("one clause is stated");
        assert_eq!(
            stated,
            "1 validation error detected: Value 'bad app id!' at 'applicationId' failed \
             to satisfy constraint: Member must satisfy regular expression pattern: \
             [\\w\\d\\.-]+",
            "case \"a pattern clause\": got {stated:?}"
        );

        // Every other class keeps the sentence the framework (and AWS) states, which
        // `Violation::message` renders.
        for kind in [
            ViolationKind::MissingRequiredMember,
            ViolationKind::MinLength(1),
            ViolationKind::MaxLength(60),
            ViolationKind::MinValue("1".to_string()),
            ViolationKind::MaxValue("50".to_string()),
            ViolationKind::UniqueItems,
        ] {
            let violation = Violation {
                path: "applicationId".to_string(),
                kind: kind.clone(),
                value: match kind {
                    ViolationKind::MissingRequiredMember => None,
                    _ => Some("x".to_string()),
                },
            };
            let stated = constraint_envelope(std::slice::from_ref(&violation))
                .expect("one clause is stated");
            let framework =
                aws_smithy_schema_server::constraints::envelope(std::slice::from_ref(
                    &violation,
                ))
                .expect("the framework states it");
            assert_eq!(
                stated, framework,
                "case {kind:?}: expected the framework's own wording {framework:?}, got \
                 {stated:?}"
            );
        }

        assert!(
            constraint_envelope(&[]).is_none(),
            "case \"no violations\": there is nothing to state"
        );
    }
}
