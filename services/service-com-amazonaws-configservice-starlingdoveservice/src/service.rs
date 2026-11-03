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
//! Config's differs from the framework's conventional one in four ways, each a
//! property of the member or of the model rather than of one request:
//!
//! 1. a `@pattern` is advertised as the expression a value must satisfy, WITHOUT the
//!    whole-value `^`/`$` anchors the public model spells it with ([`unanchored`]);
//! 2. a member the input carries inside a WRAPPER structure is named by its own
//!    declared name, read off the model's schema ([`wrapped_member`]) — `QueryName`,
//!    not the path `storedQuery.queryName` — with the model's pattern verbatim;
//! 3. the value submitted at a member Config does not echo back
//!    ([`WITHHELD_MEMBERS`]) is withheld: the clause states the position and the
//!    constraint alone;
//! 4. a value is stated against EVERY constraint of its member it broke, one clause
//!    each, including the length an account id's twelve digits fix
//!    ([`implied_account_id_length`]) — so a member can account for more than one of
//!    the validation errors the envelope counts;
//! 5. a value outside the modelled `ResourceType` value set is refused with Config's
//!    own fixed sentence about a resource type it does not support
//!    ([`UNSUPPORTED_RESOURCE_TYPE_MESSAGE`]) instead of a clause enumerating that
//!    set, wherever a member targeting that shape carries it ([`resource_type_values`]).
//!
//! The status (400) and the error code ([`VALIDATION_EXCEPTION`]) are the framework's
//! own choice for a constraint failure and are unchanged; only the sentence differs.
//! Everything else about a clause — its frame, and the requirement each constraint
//! class states — is read back off the framework's own rendering ([`requirement`]),
//! so a constraint class cannot arrive here with wording of its own.

use aws_smithy_schema_server::constraints::{path, EnumTrait};
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
        sensitive: false,
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
            sensitive: violation.sensitive,
        }
        .message(),
    )
}

/// The `@pattern` Config's account-id shape declares
/// (`smithy_query(query="shape", shape="AccountId")`), as the model spells it.
const ACCOUNT_ID_PATTERN: &str = r"^\d{12}$";

/// How long a Config account id is. The pattern above admits exactly twelve digits and
/// every member targeting the shape documents it as "The 12-digit account ID", so the
/// length is a bound of the shape in its own right — one the public model leaves
/// implicit in the pattern and that Config's validator states as its own requirement:
/// a seventeen-character id is refused for the length AND for the pattern, two clauses
/// rather than one.
const ACCOUNT_ID_LENGTH: i64 = 12;

/// The length bound a value at an account-id member broke, as a violation of its own,
/// or `None` when the member is not an account id or its value is twelve characters
/// long (the only length the pattern admits, so the pattern is then the only clause).
///
/// Keyed on the `@pattern` the violation carries, which IS the account-id shape's own:
/// the bound belongs to the shape, so it holds wherever a member targeting it travels
/// rather than being a property of one operation's input.
///
/// A violation at a position INSIDE a collection is left alone: Config states a
/// collection's element constraints against the COLLECTION, as the whole set the
/// element shape declares, which is a rendering the framework's per-element violation
/// cannot be rebuilt into here (`Violation` carries the one element's value, not the
/// collection's). Stating a second per-element clause would move that rendering further
/// from Config's, so the element keeps the framework's single clause.
fn implied_account_id_length(violation: &Violation) -> Option<Violation> {
    let value = violation.value.as_deref()?;
    if violation.kind != ViolationKind::Pattern(ACCOUNT_ID_PATTERN.to_string())
        || violation
            .path
            .split('.')
            .any(|segment| segment == COLLECTION_STEP)
    {
        return None;
    }
    let length = i64::try_from(value.chars().count()).unwrap_or(i64::MAX);
    let kind = match length {
        length if length > ACCOUNT_ID_LENGTH => ViolationKind::MaxLength(ACCOUNT_ID_LENGTH),
        length if length < ACCOUNT_ID_LENGTH => ViolationKind::MinLength(ACCOUNT_ID_LENGTH),
        _ => return None,
    };
    Some(Violation {
        path: violation.path.clone(),
        kind,
        value: violation.value.clone(),
        sensitive: violation.sensitive,
    })
}

/// Every clause ONE violation of the walk states: one per constraint of the member the
/// value broke, length before pattern — the order the validating walk itself checks
/// them in, so a member that declares both reads the same whether the walk found both
/// or this file supplied the implied one ([`implied_account_id_length`]).
fn constraint_clauses(violation: &Violation) -> Option<Vec<String>> {
    let mut clauses = Vec::new();
    if let Some(implied) = implied_account_id_length(violation) {
        clauses.push(constraint_clause(&implied)?);
    }
    clauses.push(constraint_clause(violation)?);
    Some(clauses)
}

/// How Config refuses a value that is no resource type it supports.
///
/// Config records the resource types its published catalogue covers and that catalogue
/// GROWS, so the service does not read its current value set back to the caller: the
/// sentence is FIXED and request-independent — it quotes neither the value sent nor the
/// types Config supports — and tells the caller to verify the type it named. Recorded on
/// the resource-type association path and stated for the shape rather than for one
/// operation, so it holds wherever a member targeting `ResourceType` carries a value
/// outside it.
const UNSUPPORTED_RESOURCE_TYPE_MESSAGE: &str =
    "The resource type you specified is not supported by AWS Config. Verify the resource \
     type, and try again.";

/// The members of this model that carry ONE resource type, as the model declares them:
/// where the `ResourceType` value set can be read at runtime.
///
/// Several of them, from unrelated operations, because this is how the set is reached at
/// all: a member targeting an enumerated shape carries that shape's value set as the
/// constraint the validating walk checks against, so the set is available through the
/// members that target it and not through the shape on its own. Listing more than one
/// means a member renamed or retargeted upstream cannot silently take the rule with it
/// — the others still name the set, and the unit test holds all of them to the same one.
static RESOURCE_TYPE_MEMBERS: &[(&::aws_smithy_schema::Schema, &str)] = &[
    (
        crate::ops::types::GET_RESOURCE_CONFIG_HISTORY_INPUT_SCHEMA,
        "resourceType",
    ),
    (
        crate::ops::types::LIST_DISCOVERED_RESOURCES_INPUT_SCHEMA,
        "resourceType",
    ),
    (
        crate::ops::types::LIST_AGGREGATE_DISCOVERED_RESOURCES_INPUT_SCHEMA,
        "ResourceType",
    ),
];

/// The value set the member `member_name` of `schema` admits, as the model's own schema
/// carries it, or `None` when the model declares no such member or attaches no
/// enumerated constraint to it.
fn enum_values(
    schema: &'static ::aws_smithy_schema::Schema,
    member_name: &str,
) -> Option<&'static [&'static str]> {
    let member = schema
        .members()
        .iter()
        .copied()
        .find(|member| member.member_name() == Some(member_name))?;
    let enumerated = member.traits()?.get(&EnumTrait::TRAIT_ID)?;
    Some(enumerated.as_any().downcast_ref::<EnumTrait>()?.values)
}

/// The value set the modelled `ResourceType` admits, read off the model's OWN schemas
/// rather than copied here. `None` when no listed member names it, which leaves the
/// framework's clause standing (the fail-safe direction).
fn resource_type_values() -> Option<&'static [&'static str]> {
    RESOURCE_TYPE_MEMBERS
        .iter()
        .find_map(|(schema, member_name)| enum_values(schema, member_name))
}

/// Whether `violation` is a value outside the modelled `ResourceType` value set.
///
/// Keyed on the SET the violation reports rather than on the member it sits at, because
/// that set identifies the shape the value failed: a resource type reaches an input
/// directly (`resourceType`) and as an element of a list (`resourceTypes.1.member`), and
/// both are the same shape with the same refusal.
fn is_unsupported_resource_type(violation: &Violation) -> bool {
    let ViolationKind::EnumValue(reported) = &violation.kind else {
        return false;
    };
    resource_type_values().is_some_and(|values| {
        reported
            .iter()
            .map(String::as_str)
            .eq(values.iter().copied())
    })
}

/// The `ValidationException` message for a refusal: the AWS envelope over one clause
/// per violated constraint, in the order the validating walk found them — except for a
/// resource type Config does not support, whose own fixed sentence replaces the whole
/// envelope.
fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    if violations.iter().any(is_unsupported_resource_type) {
        return Some(UNSUPPORTED_RESOURCE_TYPE_MESSAGE.to_string());
    }
    let clauses: Vec<String> = violations
        .iter()
        .map(constraint_clauses)
        .collect::<Option<Vec<_>>>()?
        .concat();
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
        // pattern verbatim, a member whose value Config does not echo back
        // ([`WITHHELD_MEMBERS`]) states the position and the constraint alone, and a
        // value that broke several of its member's constraints states one clause for
        // each of them. A value outside the modelled `ResourceType` set is the one
        // refusal Config does not state as a clause at all: it answers its own fixed
        // sentence ([`UNSUPPORTED_RESOURCE_TYPE_MESSAGE`]). The code stays
        // `ValidationException` (HTTP 400) — what the framework would have answered —
        // so only the wording changes.
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
                    sensitive: false,
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
                    sensitive: false,
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
                    sensitive: false,
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
                    sensitive: false,
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
                    sensitive: false,
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
                    sensitive: false,
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
                    sensitive: false,
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
                sensitive: false,
            },
            Violation {
                path: "requesterAccountId".to_string(),
                kind: ViolationKind::Pattern(r"^\d{12}$".to_string()),
                value: Some("not-an-acct1".to_string()),
                sensitive: false,
            },
        ];
        let stated = constraint_envelope(&found).expect("two violations word a refusal");
        let expected = "2 validation errors detected: Value '' at 'QueryName' failed to \
                        satisfy constraint: Member must have length greater than or equal \
                        to 1; Value 'not-an-acct1' at 'requesterAccountId' failed to \
                        satisfy constraint: Member must satisfy regular expression \
                        pattern: \\d{12}";
        assert_eq!(
            stated, expected,
            "case two violations: expected {expected:?}, got {stated:?}"
        );
    }

    /// An account id of any length but twelve broke the length its twelve digits fix as
    /// well as the pattern, and the refusal states a clause for each.
    #[test]
    fn an_account_ids_broken_length_is_stated_beside_its_pattern() {
        // (case, the violation the walk found, the message the service answers). The
        // rows differ along the submitted value's LENGTH — past twelve, under twelve,
        // exactly twelve — and along the position the member sits at.
        let account_id_pattern = ViolationKind::Pattern(ACCOUNT_ID_PATTERN.to_string());
        let rows: [(&str, Violation, &str); 5] = [
            (
                "longer than an account id",
                Violation {
                    path: "authorizedAccountId".to_string(),
                    kind: account_id_pattern.clone(),
                    value: Some("not-an-account-id".to_string()),
                    sensitive: false,
                },
                "2 validation errors detected: Value 'not-an-account-id' at \
                 'authorizedAccountId' failed to satisfy constraint: Member must have \
                 length less than or equal to 12; Value 'not-an-account-id' at \
                 'authorizedAccountId' failed to satisfy constraint: Member must satisfy \
                 regular expression pattern: \\d{12}",
            ),
            (
                "shorter than an account id",
                Violation {
                    path: "requesterAccountId".to_string(),
                    kind: account_id_pattern.clone(),
                    value: Some("12345".to_string()),
                    sensitive: false,
                },
                "2 validation errors detected: Value '12345' at 'requesterAccountId' \
                 failed to satisfy constraint: Member must have length greater than or \
                 equal to 12; Value '12345' at 'requesterAccountId' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: \\d{12}",
            ),
            (
                "the empty value",
                Violation {
                    path: "accountId".to_string(),
                    kind: account_id_pattern.clone(),
                    value: Some(String::new()),
                    sensitive: false,
                },
                "2 validation errors detected: Value '' at 'accountId' failed to satisfy \
                 constraint: Member must have length greater than or equal to 12; Value \
                 '' at 'accountId' failed to satisfy constraint: Member must satisfy \
                 regular expression pattern: \\d{12}",
            ),
            (
                // Twelve characters is the only length the pattern admits, so a value
                // of that length broke the pattern alone.
                "an account id's length, but not its digits",
                Violation {
                    path: "authorizedAccountId".to_string(),
                    kind: account_id_pattern.clone(),
                    value: Some("not-an-acct1".to_string()),
                    sensitive: false,
                },
                "1 validation error detected: Value 'not-an-acct1' at \
                 'authorizedAccountId' failed to satisfy constraint: Member must satisfy \
                 regular expression pattern: \\d{12}",
            ),
            (
                // An element of a list keeps the framework's single clause: Config
                // states a collection's element constraints against the collection.
                "an account id inside a list",
                Violation {
                    path: "accountAggregationSources.1.member.accountIds.1.member"
                        .to_string(),
                    kind: account_id_pattern,
                    value: Some("12345".to_string()),
                    sensitive: false,
                },
                "1 validation error detected: Value '12345' at \
                 'accountAggregationSources.1.member.accountIds.1.member' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: \
                 \\d{12}",
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

    /// A member whose `@pattern` is not the account-id shape's keeps its single clause,
    /// whatever its value's length: the implied bound belongs to that shape alone.
    #[test]
    fn another_shapes_pattern_states_one_clause_whatever_the_length() {
        // The patterns are this model's own, from `smithy_query(query="shape",
        // shape="ServicePrincipal"|"QueryName"|"QueryId")`.
        for (case, pattern, value) in [
            ("a long value", r"^[\w+=,.@-]+$", "not a valid principal!!"),
            ("a short value", "^[a-zA-Z0-9-_]+$", "a b"),
            ("a twelve-character value", r"^\S+$", "a b c d e f "),
        ] {
            let found = Violation {
                path: "queryId".to_string(),
                kind: ViolationKind::Pattern(pattern.to_string()),
                value: Some(value.to_string()),
                sensitive: false,
            };
            let stated = constraint_envelope(std::slice::from_ref(&found))
                .unwrap_or_else(|| panic!("case {case:?}: expected a reworded refusal"));
            assert!(
                stated.starts_with("1 validation error detected: "),
                "case {case:?}: expected one clause, got {stated:?}"
            );
        }
    }

    /// The pattern this file keys the implied account-id length on is the one the MODEL
    /// declares for the members that carry an account id, read back off the generated
    /// schemas rather than trusted as a copy of it.
    #[test]
    fn the_account_id_pattern_is_the_one_the_model_declares() {
        use aws_smithy_schema_server::constraints::PatternTrait;

        // (case, the operation's input schema, the member that carries an account id).
        let rows: [(&str, &::aws_smithy_schema::Schema, &str); 3] = [
            (
                "PutAggregationAuthorization",
                crate::ops::types::PUT_AGGREGATION_AUTHORIZATION_INPUT_SCHEMA,
                "AuthorizedAccountId",
            ),
            (
                "DeleteAggregationAuthorization",
                crate::ops::types::DELETE_AGGREGATION_AUTHORIZATION_INPUT_SCHEMA,
                "AuthorizedAccountId",
            ),
            (
                "DeletePendingAggregationRequest",
                crate::ops::types::DELETE_PENDING_AGGREGATION_REQUEST_INPUT_SCHEMA,
                "RequesterAccountId",
            ),
        ];

        for (case, schema, member_name) in rows {
            let member = schema
                .members()
                .iter()
                .copied()
                .find(|member| member.member_name() == Some(member_name))
                .unwrap_or_else(|| panic!("case {case:?}: the model declares {member_name}"));
            let declared = member
                .traits()
                .and_then(|traits| traits.get(&PatternTrait::TRAIT_ID))
                .and_then(|declared| declared.as_any().downcast_ref::<PatternTrait>())
                .unwrap_or_else(|| {
                    panic!("case {case:?}: {member_name} declares a @pattern")
                });
            assert_eq!(
                declared.regex, ACCOUNT_ID_PATTERN,
                "case {case:?}: expected the account-id pattern {ACCOUNT_ID_PATTERN:?}, \
                 got {:?}",
                declared.regex
            );
        }
    }

    /// A value outside the modelled `ResourceType` set is refused with Config's own
    /// fixed sentence wherever that member sits, and another shape's value set keeps
    /// the framework's clause.
    #[test]
    fn an_unsupported_resource_type_is_refused_with_configs_own_sentence() {
        let resource_types: Vec<String> = resource_type_values()
            .expect("the model declares the ResourceType value set")
            .iter()
            .map(|value| (*value).to_string())
            .collect();
        assert!(
            resource_types.iter().any(|value| value == "AWS::S3::Bucket"),
            "the set read off the model carries Config's published types, got {} entries",
            resource_types.len()
        );

        // Every member the rule is reached through names the SAME set, so a member
        // renamed or retargeted upstream shows up here rather than silently taking the
        // rule with it.
        for (schema, member_name) in RESOURCE_TYPE_MEMBERS {
            let values = enum_values(schema, member_name).unwrap_or_else(|| {
                panic!(
                    "case {member_name:?} of {:?}: the model declares the member and its \
                     value set",
                    schema.shape_id().shape_name()
                )
            });
            assert!(
                values.iter().copied().eq(resource_types.iter().map(String::as_str)),
                "case {member_name:?} of {:?}: expected the same ResourceType set, got {} \
                 entries against {}",
                schema.shape_id().shape_name(),
                values.len(),
                resource_types.len()
            );
        }

        // Rows differ along WHERE the value sits and along the value sent: a member an
        // input carries directly, two positions of a resource-type list, and a
        // position inside a wrapper the input carries.
        let rows: [(&str, &str, &str); 4] = [
            (
                "a single resource-type member",
                "resourceType",
                "AWS::NotAReal::ResourceType",
            ),
            (
                "the first element of a list",
                "resourceTypes.1.member",
                "AWS::EC2::NotAThing",
            ),
            (
                "a later element of a list",
                "resourceTypes.3.member",
                "not-a-resource-type",
            ),
            (
                "a position inside a wrapper",
                "recordingGroup.resourceTypes.2.member",
                "",
            ),
        ];

        for (case, violation_path, value) in rows {
            let found = Violation {
                path: violation_path.to_string(),
                kind: ViolationKind::EnumValue(resource_types.clone()),
                value: Some(value.to_string()),
                sensitive: false,
            };
            let stated = constraint_envelope(std::slice::from_ref(&found))
                .unwrap_or_else(|| panic!("case {case:?}: expected a reworded refusal"));
            assert_eq!(
                stated, UNSUPPORTED_RESOURCE_TYPE_MESSAGE,
                "case {case:?}: expected {UNSUPPORTED_RESOURCE_TYPE_MESSAGE:?}, got \
                 {stated:?}"
            );
        }

        // Another shape's value set is still stated as a clause enumerating it: the
        // sentence above belongs to the resource-type shape alone.
        let other = Violation {
            path: "chronologicalOrder".to_string(),
            kind: ViolationKind::EnumValue(vec!["Forward".to_string(), "Reverse".to_string()]),
            value: Some("Sideways".to_string()),
            sensitive: false,
        };
        let stated =
            constraint_envelope(std::slice::from_ref(&other)).expect("an enum states a clause");
        let expected = "1 validation error detected: Value 'Sideways' at \
                        'chronologicalOrder' failed to satisfy constraint: Member must \
                        satisfy enum value set: [Forward, Reverse]";
        assert_eq!(
            stated, expected,
            "case \"another shape's value set\": expected {expected:?}, got {stated:?}"
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
