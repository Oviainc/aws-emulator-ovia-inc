// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol` service: how it claims requests and how it words the
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
//!   before any handler ran. This service DOES reword one thing: a `@pattern` is
//!   quoted without the `^` / `$` anchors the model spells it with. See the inline
//!   note on that method.
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

use aws_smithy_schema_server::constraints::{self, Violation, ViolationKind};
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The wire error code for a request this service refuses as invalid: the shape
/// `com.amazonaws.bedrockagentcorecontrol#ValidationException`, which every operation of
/// this service declares and whose `@httpError` is 400.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// A `@pattern` expression without the anchors a model may spell it with.
///
/// Only a `^…$` PAIR bracketing the whole expression is dropped: `^` alone, or `$` alone,
/// is an expression anchored at one end — a different language, not an anchored spelling
/// of this one — and an anchor left INSIDE after the pair is removed means the pair
/// bracketed one branch of an alternation (`^a$|^b$`) rather than the whole expression, so
/// such a pattern is reported exactly as the model spells it. A `$` escaped by a backslash
/// is a literal dollar sign, not the end anchor, so it stays.
///
/// Inlined here rather than shared: every service that states this rule carries its own
/// copy, pinned by its own recordings (`grep -rln 'fn unanchored' services/*/src`), and
/// `services/common/src/validation.rs` keeps the envelope-and-clause half with the service
/// deliberately, because services do not agree on it.
fn unanchored(pattern: &str) -> &str {
    let Some(body) = pattern.strip_prefix('^') else {
        return pattern;
    };
    let Some(body) = without_end_anchor(body) else {
        return pattern;
    };
    if has_interior_anchor(body) {
        return pattern;
    }
    body
}

/// `pattern` without its end anchor, or `None` when it carries none. A trailing `$`
/// preceded by an ODD number of backslashes is escaped — a literal dollar sign inside the
/// expression — so it is not an anchor.
fn without_end_anchor(pattern: &str) -> Option<&str> {
    let body = pattern.strip_suffix('$')?;
    let escapes = body.len() - body.trim_end_matches('\\').len();
    (escapes % 2 == 0).then_some(body)
}

/// Whether `expression` still carries an anchor of its own — a `^` or `$` that is neither
/// escaped nor inside a character class — after the outer pair was stripped.
///
/// A character class is skipped because `^` is a NEGATION there (`[^:]`, which half of
/// this model's ARN patterns are built from) and `$` an ordinary character, so neither
/// anchors anything and an expression built from one must still have its outer anchors
/// removed.
fn has_interior_anchor(expression: &str) -> bool {
    let mut escaped = false;
    let mut in_class = false;
    for ch in expression.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,
            '^' | '$' if !in_class => return true,
            _ => {}
        }
    }
    false
}

/// The same violation with a `@pattern` reported unanchored; every other class is
/// untouched, so the requirement each one words stays the framework's.
fn as_stated(violation: &Violation) -> Violation {
    match &violation.kind {
        ViolationKind::Pattern(pattern) => Violation {
            path: violation.path.clone(),
            kind: ViolationKind::Pattern(unanchored(pattern).to_string()),
            value: violation.value.clone(),
        },
        _ => violation.clone(),
    }
}

/// The host-style endpoint this service is addressed by:
/// `bedrock-agentcore-control.<region>.amazonaws.com`.
///
/// Source: `smithy_query(query="service_info", model_path="services/\
/// service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/\
/// smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json")`
/// — `awsService.endpointPrefix` is `bedrock-agentcore-control`, while `sigv4Name` is the
/// shorter `bedrock-agentcore`. The two differ, so the endpoint name is NOT derivable
/// from the generated identity and is stated here.
const ENDPOINT_PREFIX: &str = "bedrock-agentcore-control";

/// The `com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol` emulated service: the generated [`ServiceDefinition`] plus
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
        // This service signs as `bedrock-agentcore` — a signing name it SHARES with the
        // AgentCore DATA plane
        // (`services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore`),
        // whose endpoint is `bedrock-agentcore` while this one's is
        // `bedrock-agentcore-control`. The router resolves a request's routing key from
        // its SigV4 credential scope first, so BOTH services see the key
        // `bedrock-agentcore`: claiming on the signing name alone makes each claim the
        // other's traffic, and the router — which refuses rather than guesses — answers
        // `500 AmbiguousService` for every AgentCore request, a code no AWS model
        // carries.
        //
        // Nor can a path segment separate them: the two models publish the SAME first
        // segment for nine resource families (`/memories/…`, `/runtimes/…`,
        // `/browsers/…`, `/code-interpreters/…`, `/identities/…`,
        // `/capacity-providers/…`, `/harnesses/…`, `/payments/…`,
        // `/browser-profiles/…`), and they collide at deeper indexes too.
        //
        // What separates them is what AWS itself dispatches on: the endpoint host, then
        // the `restJson1` route. So this service claims a request whose routing key names
        // THIS endpoint — its signing name, or the `endpointPrefix` a host-addressed,
        // unsigned request resolves to — and whose method + URI its OWN model binds a
        // route for, by the one rule `service-common` states for every service under a
        // shared signing name. The two models bind no identical method + URI pattern, so
        // exactly one of them claims every AgentCore request.
        service_common::claims_modeled_route(
            parts,
            &[self.entry.identity().signing_name(), ENDPOINT_PREFIX],
            self.definition(),
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
        // required member, or a value outside a declared bound) to let this service
        // reword it. This service states such a refusal in the framework's own envelope
        // and the framework's own clause — the count, `Value '<what was sent>' at
        // '<member>' failed to satisfy constraint: <requirement>`, the clauses joined by
        // `"; "` — with ONE difference, which is RECORDED: the `@pattern` a clause quotes
        // is the expression a value has to satisfy, NOT the `^` / `$` anchoring the model
        // writes to make that match total. The anchoring is how Smithy turns a find into a
        // whole-string match, not part of the character grammar the caller is told about,
        // and a client that lifts the advertised expression into its own pre-flight check
        // still has to anchor the match itself, exactly as it would against AWS.
        //
        // The code stays `ValidationException`, which is both what the framework would
        // answer and the shape this service's model declares (`@httpError` 400), because a
        // client branches on the code and `Refused` replaces the code and the message
        // together.
        match refusal {
            Refusal::Invalid(violations) => {
                let stated: Vec<Violation> = violations.iter().map(as_stated).collect();
                Some(Refused {
                    code: VALIDATION_EXCEPTION.to_string(),
                    message: constraints::envelope(&stated)?,
                })
            }
            // Authentication is the framework's to word: the request never reached a
            // modelled constraint, so there is no clause of this service's to state.
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
    use super::unanchored;

    /// The anchoring a model spells a `@pattern` with is dropped only when the `^…$` pair
    /// brackets the WHOLE expression; anything else is reported as the model writes it.
    ///
    /// The rows span this model's own pattern shapes (a character class, a class whose `^`
    /// is a NEGATION, an alternation of groups) and the shapes the rule must leave alone
    /// (one-end anchors, an escaped trailing dollar, individually anchored branches).
    #[test]
    fn only_a_whole_expression_anchor_pair_is_dropped() {
        // (case, the model's expression, what a clause quotes)
        let rows: [(&str, &str, &str); 9] = [
            (
                "HarnessName's character class",
                "^[a-zA-Z][a-zA-Z0-9_]{0,39}$",
                "[a-zA-Z][a-zA-Z0-9_]{0,39}",
            ),
            (
                "RoleArn, whose class negation is not an anchor",
                "^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$",
                "arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+",
            ),
            (
                "HarnessVersion's anchored group",
                "^([1-9][0-9]{0,4})$",
                "([1-9][0-9]{0,4})",
            ),
            ("anchored at the start only", "^arn:aws:", "^arn:aws:"),
            ("anchored at the end only", "[0-9]{12}$", "[0-9]{12}$"),
            ("no anchor at all", r"\d{3}", r"\d{3}"),
            ("a fully anchored alternation", "^a$|^b$", "^a$|^b$"),
            ("an escaped dollar before the anchor", r"^costs\$$", r"costs\$"),
            ("an escaped backslash before the anchor", r"^path\\$", r"path\\"),
        ];

        for (case, modelled, expected) in rows {
            let actual = unanchored(modelled);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?} from {modelled:?}, got {actual:?}"
            );
        }
    }
}
