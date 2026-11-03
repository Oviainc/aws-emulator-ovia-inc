// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-mwaa-amazonmwaa` service: how it claims requests and how it words the
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
//! - `override_operation` — EDITED HERE: not a re-route. The key and the handler stay the
//!   generated ones; the one operation that PROXIES the environment's own Apache Airflow
//!   REST API is served wrapped in that proxy ([`env_endpoint`]), which relays the refusal
//!   the endpoint it called answered.
//! - `override_refusal` — EDITED HERE: how this service words a request the framework
//!   refused before any handler ran. See the inline note on that method for the two
//!   rewordings it applies.
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

/// WHICH endpoint the model hosts each operation of this service on — the fact the
/// refusal wording of a name this service cannot resolve depends on.
pub mod endpoints;
/// The refusal contract of the ENVIRONMENT data endpoint: who words it, and the shape it
/// travels in on each of the operations hosted there.
pub mod env_endpoint;

/// The error code this service answers an input-constraint violation with: the modeled
/// `ValidationException`, which every operation that can be refused declares with
/// `@httpError 400`. It is also the framework's own default code for a refusal, restated
/// here because `override_refusal` replaces the code and the message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// One constraint violation, worded the way this service words it.
///
/// The framework already states every violation class in the AWS sentence
/// (`Value '<v>' at '<path>' failed to satisfy constraint: <requirement>`), so that
/// rendering is reused rather than re-spelled — and exactly one thing is changed: a
/// `@pattern` requirement quotes the expression WITHOUT the `^` / `$` the model spells it
/// with. AWS advertises the expression a value must satisfy, not the anchoring the model
/// uses to make the match total, and a client that reconstructs the advertised pattern
/// from the message must get one the service really emits.
///
/// The pattern is replaced at its LAST occurrence, which is the requirement clause: the
/// echoed value precedes it, so a caller who sends the pattern text as the value cannot
/// have its own value rewritten.
fn constraint_clause(violation: &Violation) -> String {
    let rendered = violation.message();
    match &violation.kind {
        ViolationKind::Pattern(pattern) => match rendered.rsplit_once(pattern.as_str()) {
            Some((head, tail)) => format!("{head}{}{tail}", unanchored(pattern)),
            None => rendered,
        },
        _ => rendered,
    }
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

/// The dotted path the validating walk names the tagging surface's ARN member with: the
/// lowerCamel spelling of `ResourceArn`, the member `TagResource`, `UntagResource` and
/// `ListTagsForResource` each bind to the `/tags/{ResourceArn}` URI label and the only
/// member of that name in this service's model.
const RESOURCE_ARN_PATH: &str = "resourceArn";

/// How the `/tags/{ResourceArn}` operations refuse a value their ARN member cannot be,
/// or `None` when this refusal is not that one.
///
/// A tagging request addresses its resource by an ARN, which that surface PARSES before
/// anything resolves it: a value whose SYNTAX is not that of one of this service's
/// resource ARNs is refused as a malformed resource ARN, with the wire error code that
/// surface publishes ([`crate::scaffolded::BAD_REQUEST_EXCEPTION`]) and its own one-line
/// reason ([`crate::state::INVALID_RESOURCE_ARN_MESSAGE`]) — never with the member path,
/// the submitted value or the model's expression, none of which that refusal states.
///
/// Keyed on the MEMBER, so all three operations that carry it are worded alike, and on the
/// `@pattern` violation alone, which is the parse this surface performs: a value that
/// satisfies the ARN grammar but breaks another of the member's bounds is a different
/// class of refusal and keeps the framework's wording, as does an absent member.
fn resource_arn_refusal(violations: &[Violation]) -> Option<Refused> {
    violations.iter().find(|violation| {
        violation.path == RESOURCE_ARN_PATH && matches!(violation.kind, ViolationKind::Pattern(_))
    })?;
    Some(Refused {
        code: crate::scaffolded::BAD_REQUEST_EXCEPTION.to_string(),
        message: crate::state::INVALID_RESOURCE_ARN_MESSAGE.to_string(),
    })
}

/// The `ValidationException` message for a refusal: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation, in the order the
/// framework accumulated them.
fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations.iter().map(constraint_clause).collect();
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The `com-amazonaws-mwaa-amazonmwaa` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The Apache Airflow proxy, which `override_operation` hands back for the one
    /// operation that relays another API's answer. Built once, because that seam returns a
    /// reference to it.
    airflow_proxy: env_endpoint::AirflowProxy,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            airflow_proxy: env_endpoint::AirflowProxy,
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
        // The operation that PROXIES the environment's own Apache Airflow REST API is
        // served through that proxy: the generated routing table still names the operation
        // and the framework still owns its whole walk, and the wrapper relays the one
        // refusal the endpoint it called answers — a name that front door cannot resolve —
        // the way that endpoint sent it (see [`env_endpoint`]). Every other request, the
        // token operations and the control-plane lookups included, resolves through the
        // generated routing table untouched and keeps the protocol's error document.
        env_endpoint::routes(&self.entry, req).then_some(&self.airflow_proxy as _)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // Two things are reworded here. A value whose syntax is not that of one of this
        // service's resource ARNs, on the `/tags/{ResourceArn}` surface, is refused the way
        // that surface refuses it — its own code and its own one-line reason, stating
        // nothing of the request (see [`resource_arn_refusal`]). Every other violation keeps
        // the framework's own rendering and the modeled `ValidationException` code, with one
        // change: a `@pattern` requirement quotes the expression without the model's `^` /
        // `$` anchors, which is how AWS advertises it. See [`constraint_clause`]. An
        // authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) => resource_arn_refusal(violations).or_else(|| {
                Some(Refused {
                    code: VALIDATION_EXCEPTION.to_string(),
                    message: constraint_envelope(violations)?,
                })
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

    /// One violation of `path`, outside `pattern`, holding `value`.
    fn pattern_violation(path: &str, pattern: &str, value: &str) -> Violation {
        Violation {
            path: path.to_string(),
            kind: ViolationKind::Pattern(pattern.to_string()),
            value: Some(value.to_string()),
            sensitive: false,
        }
    }

    /// A `@pattern` clause quotes the expression without the anchors the model spells it
    /// with, whichever anchors those are — including an expression anchored at ONE end
    /// only, and one whose trailing `$` is escaped and therefore part of the expression.
    #[test]
    fn a_pattern_clause_drops_only_the_anchors_the_model_carries() {
        for (case, modelled, advertised) in [
            ("both ends", r"^[a-z]+$", r"[a-z]+"),
            (
                "the start only",
                r"^arn:aws(-[a-z]+)?:airflow:[a-z0-9\-]+:\d{12}:environment/\w+",
                r"arn:aws(-[a-z]+)?:airflow:[a-z0-9\-]+:\d{12}:environment/\w+",
            ),
            ("the end only", r"[a-z]+$", r"[a-z]+"),
            ("neither end", r"(MON|TUE):(00|30)", r"(MON|TUE):(00|30)"),
            ("an escaped dollar", r"^[a-z]+\$", r"[a-z]+\$"),
        ] {
            let clause = constraint_clause(&pattern_violation("member", modelled, "rejected"));
            let expected = format!(
                "Value 'rejected' at 'member' failed to satisfy constraint: Member must \
                 satisfy regular expression pattern: {advertised}"
            );
            assert_eq!(
                clause, expected,
                "case {case:?}: expected {expected:?}, got {clause:?}"
            );
        }
    }

    /// The tagging surface's ARN member is refused by that surface when the value's SYNTAX
    /// is not an ARN's — and a violation of another member, of another class, or of an
    /// absent member is not.
    #[test]
    fn only_a_syntax_violation_of_the_tagging_arn_member_is_refused_by_that_surface() {
        let refused = resource_arn_refusal(&[pattern_violation(
            RESOURCE_ARN_PATH,
            r"^arn:",
            "not-an-mwaa-arn",
        )])
        .expect("the ARN member is refused by the tagging surface");
        assert_eq!(
            refused.code,
            crate::scaffolded::BAD_REQUEST_EXCEPTION,
            "expected the published code, got {:?}",
            refused.code
        );
        assert_eq!(
            refused.message,
            crate::state::INVALID_RESOURCE_ARN_MESSAGE,
            "expected the surface's own reason, got {:?}",
            refused.message
        );

        for (case, path) in [
            ("a name label", "name"),
            ("a body member", "sourceBucketArn"),
            ("a member whose name merely contains it", "myResourceArn"),
        ] {
            let answered = resource_arn_refusal(&[pattern_violation(path, r"^arn:", "rejected")]);
            assert!(
                answered.is_none(),
                "case {case:?}: must keep the framework's envelope, got {answered:?}"
            );
        }

        // Every other violation class of that same member is a different refusal: the
        // value satisfied the ARN grammar, so the surface read it and some OTHER bound
        // rejected it.
        for (case, kind) in [
            ("an absent member", ViolationKind::MissingRequiredMember),
            ("past the member's maximum", ViolationKind::MaxLength(1224)),
            ("short of its minimum", ViolationKind::MinLength(1)),
        ] {
            let answered = resource_arn_refusal(&[Violation {
                path: RESOURCE_ARN_PATH.to_string(),
                kind,
                value: Some("a value".to_string()),
                sensitive: false,
            }]);
            assert!(
                answered.is_none(),
                "case {case:?}: must keep the framework's envelope, got {answered:?}"
            );
        }
    }
}
