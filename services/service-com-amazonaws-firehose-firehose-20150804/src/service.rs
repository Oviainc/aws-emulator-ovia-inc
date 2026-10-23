// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-firehose-firehose-20150804` service: how it claims requests and how it words the
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
//!   before any handler ran. Firehose words those refusals itself: the clause names the
//!   member path and the constraint and never the value that was rejected, and a
//!   `@pattern` is reported as the expression a value must satisfy rather than as the
//!   model's anchored string (see `constraint_clause` / `reported_pattern`).
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
//! This file also owns the sentences the HANDLERS state for the rules they enforce
//! themselves (`already_exists_message` and friends), so every operation words the same
//! refusal the same way instead of spelling its own.

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code this service answers an input-constraint violation with: the
/// `ValidationException` the framework's own renderer names for a request refused at the
/// model's boundary (HTTP 400). Restated here because `override_refusal` replaces the
/// code and the message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The error code Firehose answers a value that is not an IAM role ARN with.
///
/// NOT a modelled shape — the service model carries no `InvalidParameterValueException`,
/// and `CreateDeliveryStream`'s modelled error set is `InvalidArgumentException`,
/// `InvalidKMSResourceException`, `LimitExceededException`, `ResourceInUseException`
/// (`smithy_query(query="operation", operation="CreateDeliveryStream")`). Firehose
/// nevertheless answers a role ARN of the wrong SHAPE under this code rather than under
/// the generic constraint error: it validates the role itself, and it is the code a
/// client branches on, so it is what this service answers too.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// The `@pattern` a `RoleARN` member carries
/// (`smithy_query(query="shape", shape="RoleARN")`), which is what says a value is being
/// read as an IAM role ARN rather than as any other constrained string. Every `RoleARN`
/// member of every operation targets that one shape, so matching the expression — and not
/// a member path — is what makes the rule below hold wherever a role is taken.
const ROLE_ARN_PATTERN: &str = r"^arn:.*:iam::\d{12}:role/[a-zA-Z_0-9+=,.@\-_/]+$";

/// What Firehose states for a role ARN that does not have the shape of one: the fault is
/// the role, and the sentence names neither the value nor the member, because a role is
/// validated by the service and not by the generic constraint walk.
const INVALID_ROLE_ARN: &str = "Invalid role ARN.";

/// Render one constraint violation the way Firehose states it: the member path and the
/// violated constraint, and NOTHING of the value that was rejected.
///
/// Returns `None` for a violation class this service has no wording for, which lets
/// [`constraint_envelope`] decline the whole refusal and keep the framework's wording
/// rather than invent a sentence for it ([`ViolationKind`] is `#[non_exhaustive]`, so a
/// class can be added upstream).
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
            reported_pattern(pattern)
        ),
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    };
    // An absent member has no submitted value at all, so it is reported as the null it
    // was: that is a fact of the request's shape, not a rendering of a value the caller
    // sent. Every other violation names the member ONLY — the values this service
    // constrains include record payloads (`Data`, up to 1,024,000 bytes) and tag values,
    // and none of them belongs in a 400 body.
    let subject = match &violation.kind {
        ViolationKind::MissingRequiredMember => "Value null",
        _ => "Value",
    };
    Some(format!(
        "{subject} at '{}' failed to satisfy constraint: {requirement}",
        violation.path
    ))
}

/// A `@pattern` expression as a constraint message REPORTS it, which is not quite the
/// expression the model spells.
///
/// A model anchors a pattern so the match it describes is total (`^…$`); the reported
/// form states the expression a value must satisfy, with that anchoring normalised:
///
/// * the anchors are dropped, so `^[a-zA-Z0-9_.-]+$` is reported as `[a-zA-Z0-9_.-]+`;
/// * a TOP-LEVEL alternation cannot lose them branch by branch without changing the
///   language it accepts, so for one the anchoring is restated once around the whole
///   group: `^$|\.[0-9a-z]+$` is reported as `^(|\.[0-9a-z]+)$`.
///
/// Both forms describe exactly the values the constraint admits; neither is the raw
/// model string, which is what a client re-validating with the pattern it read out of a
/// message would otherwise get. An alternation the model did NOT anchor at both ends is
/// reported as the model spells it: anchoring it here would describe a narrower language
/// than the constraint enforces.
fn reported_pattern(pattern: &str) -> String {
    let body = unanchored(pattern);
    let branches = top_level_alternatives(body);
    match branches.as_slice() {
        [] | [_] => body.to_string(),
        // Anchored at one end only, or not at all: there is no total match to normalise,
        // and grouping it would state a narrower language than the constraint enforces.
        _ if !(pattern.starts_with('^') && without_end_anchor(pattern).is_some()) => {
            pattern.to_string()
        }
        branches => format!(
            "^({})$",
            branches
                .iter()
                .map(|branch| unanchored(branch))
                .collect::<Vec<&str>>()
                .join("|")
        ),
    }
}

/// A `@pattern` expression without the anchors a model may spell it with.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    without_end_anchor(body).unwrap_or(body)
}

/// `pattern` without its end anchor, or `None` when it has none. A `$` escaped by a
/// backslash is a literal dollar sign inside the expression, not the end anchor, so it
/// stays.
fn without_end_anchor(pattern: &str) -> Option<&str> {
    let trimmed = pattern.strip_suffix('$')?;
    let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
    (backslashes % 2 == 0).then_some(trimmed)
}

/// The branches of `expression`'s top-level alternation — the `|`s that are not inside a
/// group, a character class or an escape. A single-branch answer means the expression is
/// not an alternation at all.
fn top_level_alternatives(expression: &str) -> Vec<&str> {
    let mut branches = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    let mut in_class = false;
    let mut escaped = false;
    for (index, character) in expression.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,
            '(' if !in_class => depth += 1,
            ')' if !in_class => depth = depth.saturating_sub(1),
            '|' if !in_class && depth == 0 => {
                branches.push(&expression[start..index]);
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    branches.push(&expression[start..]);
    branches
}

/// The message a refusal states: the AWS envelope (`N validation error(s) detected: `)
/// over one clause per violation.
///
/// `None` declines the whole refusal — see [`constraint_clause`].
fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations
        .iter()
        .map(constraint_clause)
        .collect::<Option<Vec<String>>>()?;
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        count => Some(format!(
            "{count} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

// ---------------------------------------------------------------------------------
// Refusal wording for the rules the HANDLERS enforce
//
// Firehose scopes these sentences to the ACCOUNT whose namespace the stream names live
// in: a stream name is unique per account per Region, so the account is what tells two
// collisions apart, and a lookup that found nothing reports only the account it was
// scoped to — never the name that was probed, which would answer "does this stream
// exist?" for a caller enumerating names. The account is the emulator's canonical one
// (`engine::emulator_account_id`), never a literal.
//
// They live beside the refusal rendering above so every operation words the same
// refusal the same way; a handler calls them rather than spelling a sentence itself.
// ---------------------------------------------------------------------------------

/// A create whose name is already taken: the requested name and the account whose
/// namespace it is taken in.
pub fn already_exists_message(name: &str) -> String {
    format!(
        "Firehose {name} under accountId {} already exists",
        ::engine::emulator_account_id()
    )
}

/// A control-plane read or delete that resolved no stream: the account the lookup was
/// scoped to, and not the name it was given.
pub fn not_found_message() -> String {
    format!(
        "Firehose under account {} not found.",
        ::engine::emulator_account_id()
    )
}

/// A put that resolved no stream. Same rule as [`not_found_message`] — the account, not
/// the probed name — in the wording the data plane uses.
pub fn put_not_found_message() -> String {
    format!(
        "Firehose not found under account {}.",
        ::engine::emulator_account_id()
    )
}

/// A put refused because the stream takes data from CloudWatch Logs only: the account the
/// put was scoped to and the stream it named.
///
/// This one DOES name the stream, unlike [`put_not_found_message`]: the refusal is a
/// property of a stream the caller already named successfully, so the sentence answers
/// "does this stream exist?" for nobody who did not know already.
pub fn invalid_source_message(name: &str) -> String {
    format!(
        "Put to Firehose failed for AccountId: {}, FirehoseName: {name} \
         because the request is not originating from CloudWatch.",
        ::engine::emulator_account_id()
    )
}

/// A delete the stream's current state blocks: the name, the account and the state that
/// blocks it, stated for whatever that state is rather than asserting a precondition of
/// its own.
pub fn blocked_delete_message(name: &str, state: &str) -> String {
    format!(
        "Firehose {name} under account {} cannot be deleted in {state} state",
        ::engine::emulator_account_id()
    )
}

/// Two or more members that exclude each other were supplied: the members, in the order
/// the model declares them.
///
/// The model expresses no union over them, so the exclusion is a rule this service
/// enforces and states as one sentence over the alternatives it saw.
pub fn exactly_one_of_message(members: &[&str]) -> String {
    let alternatives = match members {
        [] => String::new(),
        [only] => (*only).to_string(),
        [first @ .., last] => format!("{} or {last}", first.join(", ")),
    };
    format!("Exactly one of {alternatives} must be specified.")
}

/// Whether a boundary refusal is ABOUT a role ARN: one of its violations is a `@pattern`
/// violation of the `RoleARN` expression.
///
/// Keyed to the expression the model constrains every `RoleARN` member with, so the rule
/// holds for whichever member of whichever operation carried the value — the destination
/// role of a create, the source role, a VPC or secret role, the role of an update — and
/// never to a path spelled out here. A refusal that ALSO carries other violations is
/// still answered as the role fault: Firehose validates the role itself, so that is the
/// fault it reports.
fn refused_role_arn(violations: &[Violation]) -> bool {
    violations.iter().any(|violation| {
        matches!(&violation.kind, ViolationKind::Pattern(pattern) if pattern == ROLE_ARN_PATTERN)
    })
}

/// A role Firehose could not assume: the ARN the request named, and what to do about it.
///
/// The role ARN is the caller's own value and the sentence is built from it, which is why
/// it is stated — unlike a stream name, a role ARN is not a name in this account's
/// namespace that a refusal could disclose by echoing.
pub fn unassumable_role_message(role_arn: &str) -> String {
    format!("Firehose is unable to assume role {role_arn}. Please check the role provided.")
}

/// The `com-amazonaws-firehose-firehose-20150804` emulated service: the generated [`ServiceDefinition`] plus
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
        // required member, or a value outside a declared bound) to let this service
        // reword it.
        //
        // Firehose states a constraint violation WITHOUT the value that violated it: the
        // message names the member path and the constraint only, and reports a `@pattern`
        // as the expression a value must satisfy rather than as the model's anchored
        // string. The code stays the `ValidationException` (HTTP 400) the framework would
        // have answered; only the wording changes.
        //
        // A role ARN is the exception: Firehose validates a `RoleARN` member itself, so a
        // value that is not shaped like an IAM role ARN is answered under the service's
        // own parameter-value code and sentence, and the generic constraint walk is not
        // the terminal responder for it ([`refused_role_arn`]).
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) if refused_role_arn(violations) => Some(Refused {
                code: INVALID_PARAMETER_VALUE.to_string(),
                message: INVALID_ROLE_ARN.to_string(),
            }),
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
