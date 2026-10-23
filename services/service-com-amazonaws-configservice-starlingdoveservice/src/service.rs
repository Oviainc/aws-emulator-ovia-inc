// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-configservice-starlingdoveservice` service: how it claims requests and how it words the
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
//! ## This service's edited seam: how a broken constraint is stated
//!
//! A request-validation refusal is BUILT FROM THE REQUEST — it names the member that
//! failed and the constraint it failed — so its rendering is part of the contract, and
//! Config's differs from the framework's conventional one in three ways, each a
//! property of the member or of the model rather than of one request:
//!
//! 1. a `@pattern` is advertised as the expression a value must satisfy, WITHOUT the
//!    whole-value `^`/`$` anchors the public model spells it with ([`unanchored`]);
//! 2. a member the input carries inside a WRAPPER structure is named by its own
//!    declared name, read off the model's schema ([`wrapped_member`]) — `QueryName`,
//!    not the path `storedQuery.queryName` — with the model's pattern verbatim;
//! 3. the value submitted at a member Config does not echo back
//!    ([`WITHHELD_MEMBERS`]) is withheld: the clause states the position and the
//!    constraint alone.
//!
//! The status (400) and the error code ([`VALIDATION_EXCEPTION`]) are the framework's
//! own choice for a constraint failure and are unchanged; only the sentence differs.
//! Everything else about a clause — its frame, and the requirement each constraint
//! class states — is read back off the framework's own rendering ([`requirement`]),
//! so a constraint class cannot arrive here with wording of its own.

use aws_smithy_schema_server::constraints::path;
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code this service answers a request-validation failure with.
///
/// `ValidationException` is both the code the framework's own refusal renderer picks
/// for a constraint failure on this `awsJson1_1` service and the modelled
/// `com.amazonaws.configservice#ValidationException` (`errorKind` client,
/// `httpError` 400). It is restated here because a chosen [`Refused`] replaces the
/// code and the message TOGETHER, so a reworded message would otherwise drop the
/// code with it.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The marker that separates a clause's SUBJECT from the requirement it failed.
///
/// Reading the requirement back off the framework's own rendering is what keeps this
/// file from spelling a sentence per constraint class: see [`requirement`].
const CLAUSE_MARKER: &str = " failed to satisfy constraint: ";

/// How a clause names the subject when the submitted value is one Config does not
/// echo back: the input position that failed, and nothing of what was sent there.
const WITHHELD_SUBJECT: &str = "Value at";

/// The members whose submitted value Config does NOT echo into a validation clause,
/// by the lowerCamel name a clause names them with.
///
/// Keyed on the member NAME because that is the only identifier available here:
/// [`Service::override_refusal`] is handed the violations alone, so a top-level
/// member's target shape cannot be looked up (only a wrapped member's can, through
/// [`WRAPPED_SHAPES`]). In this model the name is a faithful key — every member
/// called `ServicePrincipal` targets `com.amazonaws.configservice#ServicePrincipal`
/// (`smithy_query(query="shape", shape="ServicePrincipal")`: a string with
/// `@length(1, 128)` and `@pattern("^[\\w+=,.@-]+$")`), the member
/// `PutServiceLinkedConfigurationRecorder`, `PutThirdPartyServiceLinked
/// ConfigurationRecorder` and `DeleteServiceLinkedConfigurationRecorder` take and the
/// one `ConfigurationRecorder` carries — and no other member of the model bears that
/// name. A refusal of any of them states the position and the constraint alone, which
/// is a property of the member rather than of one request, so it holds wherever that
/// member travels. Every other member keeps the framework's echo of what the caller
/// sent.
///
/// The model carries no `@sensitive` trait anywhere, so this property cannot be
/// derived from it today and the entry rests on the recorded refusal of that member.
/// A further entry is admitted only by the same kind of evidence — a recorded refusal
/// of that member showing the position without the value; where the model ever does
/// mark such members, prefer a trait-derived predicate over extending this list.
const WITHHELD_MEMBERS: [&str; 1] = ["servicePrincipal"];

/// The requirement one violation states, read off the framework's OWN rendering of
/// it rather than respelled here.
///
/// `Violation::message` is the single place a constraint class's sentence is written;
/// rendering the violation with NO value attached and no path, then splitting at the
/// LAST [`CLAUSE_MARKER`], recovers that sentence without this file knowing which
/// classes exist ([`ViolationKind`] is `#[non_exhaustive]`, so a class can be added
/// upstream). `rsplit_once` so a requirement that itself contained the marker could
/// not truncate it. This is the "requirement half" that
/// `services/common/src/validation.rs` lists as still to migrate into it; the
/// migration that moves it there owns this copy too.
fn requirement(kind: &ViolationKind) -> Option<String> {
    let rendered = Violation {
        path: String::new(),
        kind: kind.clone(),
        value: None,
    }
    .message();
    rendered
        .rsplit_once(CLAUSE_MARKER)
        .map(|(_, requirement)| requirement.to_string())
}

/// A `@pattern` expression without the whole-value anchors a model spells it with.
///
/// The model writes AWS's implicit whole-value match as an explicit `^…$` pair
/// (`smithy_query(query="shape", shape="AccountId")` gives `^\d{12}$`); the service
/// advertises the expression itself, `\d{12}`. The pair is dropped from the
/// RENDERING only — the check still runs the model's own pattern.
///
/// Only the PAIR is dropped. A pattern anchored at one end is a different expression
/// — `^arn:aws:` matches a prefix, `[0-9]+$` a suffix — and neither spells a
/// whole-value match, so both are emitted verbatim. A `$` escaped by a backslash is a
/// literal dollar inside the expression rather than the end anchor, so a pattern
/// ending in one keeps it.
///
/// MIGRATION NOTE, as `services/common/src/validation.rs` asks of anything that
/// inlines this rule again: its "Still to migrate here" section names the unanchored
/// `@pattern` rendering as one of three parts of a validation message that are the
/// same for EVERY service and belong there, carried inlined in the sibling service
/// crates today (`grep -rln 'fn unanchored' services/*/src`). This is one more such
/// copy, pinned by this service's own recordings, and is to be DELETED by the change
/// that owns that migration rather than copied again. The same holds for
/// [`requirement`] below.
fn unanchored(pattern: &str) -> &str {
    let Some(body) = pattern.strip_prefix('^') else {
        return pattern;
    };
    match body.strip_suffix('$') {
        Some(trimmed) if !ends_with_escape(trimmed) => trimmed,
        // A `^` with no closing `$`: an expression anchored at one end only, kept as
        // the model spells it.
        _ => pattern,
    }
}

/// Whether `text` ends in an escape — an odd number of trailing backslashes, so the
/// character that followed them was escaped.
fn ends_with_escape(text: &str) -> bool {
    text.chars().rev().take_while(|c| *c == '\\').count() % 2 == 1
}

/// The step the validating walk pushes for a collection's element
/// (`<member>.<index>.member`), which is not a member name.
const COLLECTION_STEP: &str = "member";

/// The structures an operation input of this service WRAPS: the shapes a validation
/// path can descend THROUGH, as the model's own schemas.
///
/// They are the model's schemas and not a hand-written list of names, so the declared
/// name of a member is read off the model rather than restored by guesswork — which
/// matters because this model declares its member names in BOTH casings
/// (`smithy_query(query="shape", shape="StoredQuery")` gives `QueryId`, `QueryArn`,
/// `QueryName`, `Description`, `Expression`, while
/// `smithy_query(query="shape", shape="ConfigurationRecorder")` gives `arn`, `name`,
/// `roleARN`, `servicePrincipal`, …). A wrapper this list does not hold yields no
/// declared name, so the clause keeps the framework's own path: the fail-safe
/// direction.
static WRAPPED_SHAPES: &[&::aws_smithy_schema::Schema] = &[
    &crate::ops::types::AccountAggregationSource::SCHEMA,
    &crate::ops::types::ConfigRule::SCHEMA,
    &crate::ops::types::ConfigurationRecorder::SCHEMA,
    &crate::ops::types::Connector::SCHEMA,
    &crate::ops::types::DeliveryChannel::SCHEMA,
    &crate::ops::types::ExternalEvaluation::SCHEMA,
    &crate::ops::types::OrganizationAggregationSource::SCHEMA,
    &crate::ops::types::RecordingGroup::SCHEMA,
    &crate::ops::types::RecordingMode::SCHEMA,
    &crate::ops::types::RemediationConfiguration::SCHEMA,
    &crate::ops::types::ResourceDetails::SCHEMA,
    &crate::ops::types::RetentionConfiguration::SCHEMA,
    &crate::ops::types::Scope::SCHEMA,
    &crate::ops::types::ScopeConfiguration::SCHEMA,
    &crate::ops::types::Source::SCHEMA,
    &crate::ops::types::StoredQuery::SCHEMA,
    &crate::ops::types::TemplateSSMDocumentDetails::SCHEMA,
];

/// The declared name of a member the input carries INSIDE a wrapper structure, as the
/// model's own schema spells it. `None` when the violation does not name such a
/// member, or when no listed shape can name it.
///
/// Config validates such an input itself rather than at the protocol boundary — the
/// model says so for the operation this is recorded on: `ValidationException`'s doc
/// enumerates `PutStoredQuery`'s input-validation failures ("There are missing
/// required fields", "The input value fails the validation"). Its clause names the
/// member the caller has to fix — `QueryName`, not the path `storedQuery.queryName`
/// through the wrapper the operation takes — spelled as the model declares it.
///
/// Only a plain nested MEMBER path qualifies. A path through a collection
/// (`tags.1.member.key`) names a position rather than a member of a wrapper
/// structure, so it keeps the framework's whole path.
fn wrapped_member(violation_path: &str) -> Option<&'static str> {
    let (wrapper, member) = violation_path.rsplit_once('.')?;
    if wrapper.is_empty()
        || member.is_empty()
        || violation_path
            .split('.')
            .any(|segment| segment == COLLECTION_STEP)
    {
        return None;
    }
    // The INNERMOST wrapper is the one that declares the member.
    let wrapper = wrapper.rsplit_once('.').map_or(wrapper, |(_, inner)| inner);
    let shape = WRAPPED_SHAPES
        .iter()
        .copied()
        .find(|shape| path::member(shape.shape_id().shape_name()) == wrapper)?;
    shape.members().iter().copied().find_map(|declared| {
        let name = declared.member_name()?;
        (path::member(name) == member).then_some(name)
    })
}

/// Whether the value submitted at a violation's member is one Config withholds from
/// a clause. Keyed on the member the path ENDS at, so it holds wherever that member
/// travels — directly on an input or inside a structure the input carries.
///
/// There has to BE a submitted value for withholding to mean anything: an ABSENT
/// member keeps `Value null at '<path>' …`, the cross-service rule
/// `services/common/src/validation.rs` states, because the `null` is a fact of the
/// request's SHAPE rather than a rendering of anything the caller sent.
fn is_withheld(violation: &Violation) -> bool {
    if violation.value.is_none() {
        return false;
    }
    let member = violation
        .path
        .rsplit_once('.')
        .map_or(violation.path.as_str(), |(_, member)| member);
    WITHHELD_MEMBERS.contains(&member)
}

/// One constraint violation as this service states it, or `None` for a class it has
/// no wording for (which lets [`constraint_envelope`] decline the whole refusal and
/// keep the framework's).
///
/// The two renderings are keyed on WHERE the member sits because that is what decides
/// which validator produced the refusal, and each half rests on its own recorded
/// evidence: `config.delete-pending-aggregation-request-malformed-identifiers-rejected`
/// (a top-level `RequesterAccountId`) shows the model's `^\d{12}$` advertised as
/// `\d{12}`, while `config.stored-query-invalid-name-rejected` (a `QueryName` inside
/// the `StoredQuery` the operation wraps) shows `^[a-zA-Z0-9-_]+$` printed verbatim
/// beside the member's own declared name — the operation whose input validation
/// `ValidationException`'s model doc states Config performs itself. Where a third
/// rendering appears for a wrapper this file cannot name, the framework's own clause
/// stands (see [`wrapped_member`]).
fn constraint_clause(violation: &Violation) -> Option<String> {
    let (path, kind) = match wrapped_member(&violation.path) {
        // A member inside a wrapper structure: its own declared name, and the
        // model's `@pattern` printed as the model declares it.
        Some(member) => (member.to_string(), violation.kind.clone()),
        // A member the input carries directly: the framework's own path spelling,
        // with a `@pattern` advertised without the model's whole-value anchors.
        None => (
            violation.path.clone(),
            match &violation.kind {
                ViolationKind::Pattern(pattern) => {
                    ViolationKind::Pattern(unanchored(pattern).to_string())
                }
                other => other.clone(),
            },
        ),
    };
    if is_withheld(violation) {
        let requirement = requirement(&kind)?;
        return Some(format!(
            "{WITHHELD_SUBJECT} '{path}'{CLAUSE_MARKER}{requirement}"
        ));
    }
    // A class the framework itself has no wording for has none here either: the probe
    // is what lets [`constraint_envelope`] decline the whole refusal.
    requirement(&kind)?;
    // Everything else — the subject, the quoting and the requirement — is the
    // framework's own clause, reached through the same `Violation` the walk
    // produced, so a constraint class cannot arrive here with wording of its own.
    Some(
        Violation {
            path,
            kind,
            value: violation.value.clone(),
        }
        .message(),
    )
}

/// The `ValidationException` message for a refusal: the AWS envelope over one clause
/// per violation, in the order the validating walk found them.
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

/// The `com-amazonaws-configservice-starlingdoveservice` emulated service: the generated [`ServiceDefinition`] plus
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
        // Config states such a refusal from the member the request named and the
        // constraint it failed: a member the input carries DIRECTLY keeps the
        // framework's lowerCamel path and advertises a `@pattern` without the
        // model's whole-value `^`/`$` anchors, a member the input carries inside a
        // WRAPPER structure is named by its own declared name with the model's
        // pattern verbatim, and a member whose value Config does not echo back
        // ([`WITHHELD_MEMBERS`]) states the position and the constraint alone. The
        // code stays `ValidationException` (HTTP 400) — what the framework would
        // have answered — so only the wording changes.
        //
        // An authentication refusal is not an input-constraint failure and keeps
        // the framework's wording.
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
    //! The three renderings this file owns, each over a table spanning the class
    //! rather than the one recorded case. The end-to-end wording is pinned on the
    //! wire in `tests/all/error_message_rules.rs`.

    use super::*;

    /// A `@pattern` loses the whole-value anchor PAIR and nothing else.
    #[test]
    fn a_pattern_is_advertised_without_its_whole_value_anchors() {
        // (case, the pattern the model declares, the expression the service states).
        // The first three are this model's own, from
        // `smithy_query(query="shape", shape="ServicePrincipal"|"AccountId"|"QueryName")`.
        let rows: [(&str, &str, &str); 9] = [
            ("the modelled service principal", r"^[\w+=,.@-]+$", r"[\w+=,.@-]+"),
            ("the modelled account id", r"^\d{12}$", r"\d{12}"),
            ("the modelled query name", "^[a-zA-Z0-9-_]+$", "[a-zA-Z0-9-_]+"),
            // `DeleteConfigRule$ConfigRuleName` declares a pattern with no anchor at
            // all, which is emitted as the model spells it.
            ("no anchor at all", r"\S", r"\S"),
            ("anchored at the start only", "^arn:aws:", "^arn:aws:"),
            ("anchored at the end only", "[0-9]+$", "[0-9]+$"),
            ("an escaped trailing dollar", r"^[a-z]+\$$", r"[a-z]+\$"),
            ("a negated character class", "^[^/]+$", "[^/]+"),
            ("an empty expression between anchors", "^$", ""),
        ];

        for (case, declared, expected) in rows {
            let actual = unanchored(declared);
            assert_eq!(
                actual, expected,
                "case {case:?} ({declared}): expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A member inside a wrapper structure is named by the name the MODEL declares it
    /// with — whichever casing that is — and a path that names no such member is left
    /// to the framework.
    #[test]
    fn a_wrapped_member_is_named_as_the_model_declares_it() {
        // (case, the path the walk built, the name the service states or None to keep
        // the framework's path). Sources: `smithy_query(query="shape",
        // shape="StoredQuery"|"ConfigurationRecorder"|"DeliveryChannel"|"ConfigRule")`.
        let rows: [(&str, &str, Option<&str>); 10] = [
            ("StoredQuery's UpperCamel member", "storedQuery.queryName", Some("QueryName")),
            ("another StoredQuery member", "storedQuery.queryId", Some("QueryId")),
            ("a StoredQuery member with no acronym", "storedQuery.expression", Some("Expression")),
            ("ConfigurationRecorder's lowerCamel member", "configurationRecorder.name", Some("name")),
            ("a member declared with an acronym", "configurationRecorder.roleARN", Some("roleARN")),
            ("a DeliveryChannel member", "deliveryChannel.s3BucketName", Some("s3BucketName")),
            ("a ConfigRule member", "configRule.configRuleName", Some("ConfigRuleName")),
            ("a member the input carries directly", "servicePrincipal", None),
            ("a path through a collection", "tags.1.member.key", None),
            ("a wrapper no listed shape names", "notAShape.someMember", None),
        ];

        for (case, violation_path, expected) in rows {
            let actual = wrapped_member(violation_path);
            assert_eq!(
                actual, expected,
                "case {case:?} ({violation_path}): expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// Every constraint class the framework can report is stated with this service's
    /// subject, path and requirement.
    #[test]
    fn a_violation_is_stated_the_way_config_states_it() {
        // (case, the violation the walk found, the message the service answers).
        let rows: [(&str, Violation, &str); 7] = [
            (
                "a withheld member's pattern",
                Violation {
                    path: "servicePrincipal".to_string(),
                    kind: ViolationKind::Pattern(r"^[\w+=,.@-]+$".to_string()),
                    value: Some("not a valid principal!!".to_string()),
                },
                "1 validation error detected: Value at 'servicePrincipal' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: \
                 [\\w+=,.@-]+",
            ),
            (
                "a top-level member's pattern",
                Violation {
                    path: "requesterAccountId".to_string(),
                    kind: ViolationKind::Pattern(r"^\d{12}$".to_string()),
                    value: Some("not-an-acct1".to_string()),
                },
                "1 validation error detected: Value 'not-an-acct1' at \
                 'requesterAccountId' failed to satisfy constraint: Member must satisfy \
                 regular expression pattern: \\d{12}",
            ),
            (
                "a wrapped member's pattern",
                Violation {
                    path: "storedQuery.queryName".to_string(),
                    kind: ViolationKind::Pattern("^[a-zA-Z0-9-_]+$".to_string()),
                    value: Some("invalid query name".to_string()),
                },
                "1 validation error detected: Value 'invalid query name' at \
                 'QueryName' failed to satisfy constraint: Member must satisfy regular \
                 expression pattern: ^[a-zA-Z0-9-_]+$",
            ),
            (
                "an absent required member",
                Violation {
                    path: "requesterAwsRegion".to_string(),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                },
                "1 validation error detected: Value null at 'requesterAwsRegion' failed \
                 to satisfy constraint: Member must not be null",
            ),
            (
                "an absent withheld member",
                Violation {
                    path: "servicePrincipal".to_string(),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                },
                "1 validation error detected: Value null at 'servicePrincipal' failed to \
                 satisfy constraint: Member must not be null",
            ),
            (
                "a length bound on a wrapped member",
                Violation {
                    path: "storedQuery.queryName".to_string(),
                    kind: ViolationKind::MaxLength(64),
                    value: Some("q".to_string()),
                },
                "1 validation error detected: Value 'q' at 'QueryName' failed to satisfy \
                 constraint: Member must have length less than or equal to 64",
            ),
            (
                "a length bound on a collection",
                Violation {
                    path: "tags".to_string(),
                    kind: ViolationKind::MaxLength(50),
                    value: Some("51 items".to_string()),
                },
                "1 validation error detected: Value '51 items' at 'tags' failed to \
                 satisfy constraint: Member must have length less than or equal to 50",
            ),
        ];

        for (case, found, expected) in rows {
            let stated = constraint_envelope(std::slice::from_ref(&found))
                .unwrap_or_else(|| panic!("case {case:?}: expected a reworded refusal"));
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    /// Several violations are joined in the plural envelope, in the order the walk
    /// found them.
    #[test]
    fn several_violations_are_joined_in_the_plural_envelope() {
        let found = [
            Violation {
                path: "storedQuery.queryName".to_string(),
                kind: ViolationKind::MinLength(1),
                value: Some(String::new()),
            },
            Violation {
                path: "requesterAccountId".to_string(),
                kind: ViolationKind::Pattern(r"^\d{12}$".to_string()),
                value: Some("1".to_string()),
            },
        ];
        let stated = constraint_envelope(&found).expect("two violations word a refusal");
        let expected = "2 validation errors detected: Value '' at 'QueryName' failed to \
                        satisfy constraint: Member must have length greater than or equal \
                        to 1; Value '1' at 'requesterAccountId' failed to satisfy \
                        constraint: Member must satisfy regular expression pattern: \
                        \\d{12}";
        assert_eq!(
            stated, expected,
            "case two violations: expected {expected:?}, got {stated:?}"
        );
    }

    /// Nothing to word is no refusal: a request that breaks no constraint is not
    /// answered here.
    #[test]
    fn no_violations_is_no_refusal() {
        let stated = constraint_envelope(&[]);
        assert!(
            stated.is_none(),
            "case no violations: expected None, got {stated:?}"
        );
    }
}
