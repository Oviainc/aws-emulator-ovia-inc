// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-ecr-amazonec2containerregistry-v20150921` service: how it claims requests and how it words the
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

/// The `com-amazonaws-ecr-amazonec2containerregistry-v20150921` emulated service: the generated [`ServiceDefinition`] plus
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

/// The clause ECR states for one refused input member: the member at fault and the
/// constraint it failed.
///
/// ECR words a parameter it will not accept as `Invalid parameter at '<member>' failed
/// to satisfy constraint: '<requirement>'`. Both halves are built from the request and
/// the model — the member the walk found at fault ([`member_at_fault`]) and the
/// constraint it fell outside of — so they are reproduced from the violation here rather
/// than written out per member. A violation kind this list does not know (the enum is
/// `#[non_exhaustive]`) keeps the framework's own clause, which states the same two facts
/// in its own words.
fn clause(violation: &Violation) -> String {
    let member = member_at_fault(&violation.path);
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
        // The one requirement NOT worded "Member must …": ECR states a pattern failure
        // by naming the expression a value has to satisfy — except for an ARN-valued
        // member, which it states in prose instead ([`arn_constraint`]). Deliberate — do
        // not "align" either with the arms around it.
        ViolationKind::Pattern(pattern) => {
            arn_constraint(member, pattern, violation.value.as_deref()).unwrap_or_else(|| {
                format!("must satisfy regular expression '{}'", as_stated(pattern))
            })
        }
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return violation.message(),
    };
    format!("Invalid parameter at '{member}' failed to satisfy constraint: '{requirement}'")
}

/// The member a refusal names: the member at fault ITSELF, not the path the validating
/// walk descended to reach it.
///
/// The framework builds a dotted path — `signingConfiguration.rules.1.member.signingProfileArn`
/// for the signing profile of a rule — because it has to say which element carried the
/// value. ECR names the member alone: `Invalid parameter at 'signingProfileArn' …`. So
/// the steps from a collection into an element are dropped off the end first, which leaves
/// the COLLECTION named when the refused value is itself an element (`repositoryNames`
/// rather than `repositoryNames.2.member`), and what remains is the last named member.
///
/// The element step is recognised through [`::service_common::validation::list_of_element`],
/// the one place in the tree that spells it, so this cannot disagree with the walk about
/// the shape of a path. The other step that spelling covers, a map entry, cannot arise
/// here: this service's model declares no map shape at all, and a map key is a value the
/// CALLER chose rather than a member name.
///
/// A path with no named member left in it keeps its own spelling: there is nothing in it
/// to prefer.
fn member_at_fault(path: &str) -> &str {
    let mut remaining = path;
    while let Some(collection) = ::service_common::validation::list_of_element(remaining) {
        remaining = collection;
    }
    remaining.rsplit('.').next().unwrap_or(remaining)
}

/// A `@pattern` expression as AWS STATES it, which differs from how the model spells it
/// in two ways — both of them about spelling the same expression, neither about what it
/// matches:
///
/// * the `^` / `$` the model anchors with are dropped ([`unanchored`]): AWS states the
///   expression a value must satisfy, not the anchoring that makes the match total;
/// * a `/` carries no backslash. A slash is not a regular-expression metacharacter, so
///   `\/` and `/` match the same character and AWS states the bare one. Every other
///   escape is kept — `\.`, `\w`, and `\\` included — because dropping one WOULD change
///   what the expression matches.
fn as_stated(pattern: &str) -> String {
    let body = unanchored(pattern);
    let mut stated = String::with_capacity(body.len());
    let mut characters = body.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            stated.push(character);
            continue;
        }
        match characters.next() {
            // The one escape that is not part of the expression AWS states.
            Some('/') => stated.push('/'),
            // Every other escape is: keep the pair exactly as the model spells it.
            Some(escaped) => {
                stated.push('\\');
                stated.push(escaped);
            }
            // A trailing backslash escapes nothing; it stands as it is.
            None => stated.push('\\'),
        }
    }
    stated
}

/// The suffix the name of a member holding an Amazon Resource Name ends in.
const ARN_MEMBER_SUFFIX: &str = "Arn";

/// The anchorless opening of a `@pattern` that describes an ARN.
const ARN_PREFIX: &str = "arn:";

/// The constraint ECR states for an ARN-valued member refused against its `@pattern`:
/// prose naming the kind of ARN and the value that is not one, rather than the
/// expression.
///
/// ECR does not quote an ARN member's expression back at the caller — it says the ARN is
/// not well formed: `The signing profile ARN '<value>' must be well formed`. The kind of
/// ARN comes from the member's own name with its [`ARN_MEMBER_SUFFIX`] removed and its
/// lowerCamel words separated (`signingProfileArn` -> `signing profile`), so every
/// ARN-valued member this service models is worded by the one rule and none is written
/// out here.
///
/// `None` — so the expression itself is stated — when the member is not ARN-valued: its
/// name does not end in `Arn`, or the expression it failed does not describe an ARN. Also
/// `None` when the violation carries no value, which a `@pattern` failure always does
/// (only an absent member has none) but which is not this function's to assume.
fn arn_constraint(member: &str, pattern: &str, value: Option<&str>) -> Option<String> {
    let kind = member
        .strip_suffix(ARN_MEMBER_SUFFIX)
        .filter(|kind| !kind.is_empty())?;
    if !unanchored(pattern).starts_with(ARN_PREFIX) {
        return None;
    }
    let value = value?;
    Some(format!(
        "The {} ARN '{value}' must be well formed",
        in_words(kind)
    ))
}

/// A lowerCamel member name as the words it is made of: `signingProfile` -> `signing
/// profile`, `credential` -> `credential`.
///
/// Every member this service models is lowerCamel with no acronym RUN (`smithy_query`
/// digest), so each capital opens a word. A name carrying a run (`sourceKMSKey`) would
/// be worded letter by letter; it is spelled out here rather than guessed at, because a
/// run-aware split would have to decide where the run ends with nothing in the model to
/// decide it from.
fn in_words(name: &str) -> String {
    let mut words = String::with_capacity(name.len() + name.len() / 4);
    for character in name.chars() {
        if character.is_uppercase() && !words.is_empty() {
            words.push(' ');
        }
        words.extend(character.to_lowercase());
    }
    words
}

/// A `@pattern` expression without the anchors a model may spell it with: AWS states
/// the expression a value must satisfy, not the anchoring the model uses to make the
/// match total. A `$` escaped by a backslash is a literal dollar sign inside the
/// expression, not the end anchor, so it stays. (Same rule, and the same reasoning, as
/// `services/service-com-amazonaws-controlcatalog-controlcatalog/src/service.rs`'s own
/// `unanchored`; neither service's private helper is reachable from the other.)
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
        // The centralized walk calls this on a framework-level refusal — an absent
        // required member, or a value outside a `@length`, `@range`, `@pattern`,
        // `@enum` or `@uniqueItems` the model declares — before any handler runs.
        //
        // ECR states such an input as `InvalidParameterException` ("The specified
        // parameter is invalid. Review the available parameters for the API request."):
        // 57 of this model's 58 operations declare it, and it is the only client-input
        // error `CreateRepository`, `DescribeRepositories`, `ListImages`, `DescribeImages`
        // and `PutImage` declare at all. The framework's default code,
        // `ValidationException`, is not in those operations' error sets — the model binds
        // its own `ValidationException` ("There was an exception validating this request")
        // only to other operations — so a client branching on ECR's documented code sees
        // an error its SDK cannot even name. This seam is per SERVICE, not per operation,
        // so the code chosen here is the one nearly every operation models.
        //
        // The status stays with the protocol: awsJson1_1 renders a refusal as HTTP 400,
        // which is `InvalidParameterException`'s modelled status.
        match refusal {
            // Every operation but `DeleteSigningConfiguration` declares this error; that
            // one declares ECR's own `ValidationException` instead, and this seam cannot
            // name the operation, so it answers the code the other 57 model rather than
            // the framework's synthetic one, which none of them models.
            Refusal::Invalid(violations) => Some(Refused {
                code: "InvalidParameterException".to_string(),
                // Every member at fault, so a caller fixes the request from the message
                // alone; one refused member is the single-clause case of the same rule.
                // A refusal that named no violation still answers the modelled code,
                // with the error shape's own documented sentence: the alternative is
                // falling through to a code no operation here models.
                message: if violations.is_empty() {
                    "The specified parameter is invalid. Review the available parameters \
                     for the API request."
                        .to_string()
                } else {
                    violations
                        .iter()
                        .map(clause)
                        .collect::<Vec<String>>()
                        .join("; ")
                },
            }),
            // Authentication is not this service's concern, and a refusal kind added
            // later keeps the framework's wording until it is worded here.
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
