// SPDX-License-Identifier: CC0-1.0

//! The Cedar policy language, as this service speaks it.
//!
//! Verified Permissions *is* Cedar: a policy's `effect` and scope, and every
//! authorization decision, are properties of the Cedar text the caller stored, so
//! they are read out of the real Cedar parser and evaluated by the real Cedar
//! authorizer (the `cedar-policy` crate) rather than being pattern-matched out of
//! the statement. A statement this emulator cannot parse is refused with the
//! parser's own diagnostics, never accepted and guessed at.
//!
//! ## How this in-process evaluator is bounded
//!
//! Cedar runs in the serving process, so every dimension of the work one request
//! can ask for is capped here, and each cap refuses with a modelled error rather
//! than with unbounded work:
//!
//! * **Termination** — Cedar has no loops, no recursion and no function calls, so
//!   evaluating one policy always terminates. There is nothing for a wall-clock
//!   timeout to interrupt; what needs bounding is how much input reaches it.
//! * **Size** — [`MAX_STATEMENT_BYTES`] caps one policy's Cedar text and
//!   [`crate::state::MAX_POLICY_BYTES_PER_STORE`] caps the total text one
//!   authorization request parses (the per-request step budget).
//!   [`MAX_CEDAR_JSON_BYTES`] caps the `cedarJson` text a request may carry.
//! * **Count** — the entity store is capped at the capacity the CALLING
//!   operation documents for its own `entities` member
//!   ([`MAX_ENTITIES_PER_REQUEST`] for a single-request call,
//!   [`MAX_ENTITIES_PER_BATCH`] for a batch one),
//!   [`MAX_COLLECTION_LEN`] caps one set/record/attribute map,
//!   [`crate::state::MAX_POLICIES_PER_STORE`] caps the policies walked, and
//!   [`MAX_BATCH_REQUESTS`] caps how many authorization requests one batch call
//!   evaluates against that one parsed policy set.
//! * **Depth** — [`MAX_VALUE_DEPTH`] caps the nesting of any context or entity
//!   value. Every conversion below carries a depth counter and refuses at the
//!   bound INSTEAD of recursing, so no caller-supplied nesting drives this
//!   module's own recursion past a fixed number of frames. Cedar's own parser
//!   grows its stack through `stacker` rather than overflowing the request
//!   thread's.
//! * **Output** — [`MAX_REPORTED_EVALUATION_ERRORS`] caps the `errors` list the
//!   response carries; `determiningPolicies` is bounded by the policy count.

use std::collections::{BTreeSet, HashMap};
use std::str::FromStr;

use crate::ops::types::{
    ActionIdentifier, AttributeValue, CedarTagValue, ContextDefinition, EntitiesDefinition,
    EntityIdentifier, EntityItem,
};
use crate::ops::{paths, InvalidInput};

use super::ScopeEntity;

/// Why a Cedar operation refused to answer.
///
/// The arms are different HTTP answers, and keeping them apart is what stops a
/// fault in SERVICE-side state being reported as the caller's mistake: a store
/// whose stored schema will not parse, or one holding more Cedar than a request
/// evaluates, is nothing the caller can fix from the request it sent.
#[derive(Debug, Clone)]
pub enum Refusal {
    /// A member of the request is invalid — the modelled `ValidationException`,
    /// with this member in its `fieldList`.
    InvalidInput(InvalidInput),
    /// The policy content the request carried was refused by the policy store's
    /// SCHEMA: one entry per reason the Cedar validator raised, in the order it
    /// raised them. It stays a list of reasons rather than an [`InvalidInput`]
    /// because the member they are reported under is the CALLER's: it is the
    /// request member whose content was validated (`/definition` for a policy,
    /// `/statement` for a policy template), which the operation knows and this
    /// module does not. [`crate::ops::schema_validation_exception`] turns the
    /// pair into the modelled error.
    SchemaValidation(Vec<String>),
    /// Service-side state cannot answer this request — the modelled
    /// `InternalServerException`, carrying this text.
    ServiceFault(String),
}

impl From<InvalidInput> for Refusal {
    fn from(invalid: InvalidInput) -> Self {
        Self::InvalidInput(invalid)
    }
}

/// How many entities ONE authorization request may carry — the capacity of the
/// single-request operations (`IsAuthorized`, `IsAuthorizedWithToken`), whose own
/// documentation states no figure for their `entities` member, so the published
/// quota *Entities per authorization request* is the bound.
///
/// It is NOT the bound for every operation carrying an `entities` member: that
/// capacity belongs to the OPERATION, which is why it is a parameter of
/// [`entities_to_cedar_json`] rather than one figure read inside it. See
/// [`MAX_ENTITIES_PER_BATCH`].
pub const MAX_ENTITIES_PER_REQUEST: usize = 100;

/// How many entities a BATCH authorization call's `entities` may carry, from
/// `BatchIsAuthorized`'s own model documentation: "The entities of a
/// BatchIsAuthorized API request can contain up to 100 principals and up to 100
/// resources."
///
/// The slice is a property of the BATCH and not of one of its requests — it is
/// built once and every request is evaluated against it — so its capacity is the
/// two documented figures together, and a batch that declares the principals and
/// the resources its requests range over is evaluated rather than refused.
///
/// The two figures are enforced as this one total and not per kind: whether an
/// entity is a principal or a resource is a property of the store's schema and of
/// the position a request puts it in, neither of which a slice alone carries (a
/// store may hold no schema at all), so there is nothing to split the count by
/// here. A reader should take 200 as the capacity of the whole slice, not as a
/// per-kind rule.
pub const MAX_ENTITIES_PER_BATCH: usize = 200;

/// How many entities a token-based BATCH authorization call's `entities` may
/// carry, from `BatchIsAuthorizedWithToken`'s own model documentation: "The
/// entities of a BatchIsAuthorizedWithToken API request can contain up to 100
/// resources and up to 99 user groups."
///
/// Only the FIRST of those two figures bounds the member, because only resources
/// (and the action entities named beside them) may travel in it: the same
/// documentation forbids the member any entity of a type the call's identity
/// source generates — a user or one of its groups — and
/// [`check_token_request_entities`] refuses one. The up-to-99 user groups of that
/// sentence are therefore the ones the call builds out of the token's own claims
/// and adds to the slice itself, not entities a caller may send.
pub const MAX_ENTITIES_PER_TOKEN_BATCH: usize = 100;

/// How many bytes of Cedar text one policy may carry. The published Verified
/// Permissions quota for the size of a policy is 10,000 bytes; the model puts no
/// upper `@length` on `PolicyStatement`, so the boundary validator cannot refuse
/// an over-long one and this is the place that does.
pub const MAX_STATEMENT_BYTES: usize = 10_000;

/// How many bytes of `cedarJson` text one request may carry for its `context` or
/// its `entities`. Both are parsed in the serving process, and the model puts no
/// `@length` on either, so the size needs a bound here. Sized to hold
/// [`MAX_ENTITIES_PER_BATCH`] entities with room to spare.
pub const MAX_CEDAR_JSON_BYTES: usize = 64 * 1024;

/// How deeply a context or entity value may nest. Each conversion below recurses
/// once per level, so this is the fixed bound on this module's own stack usage.
pub const MAX_VALUE_DEPTH: usize = 32;

/// How many elements one set, record or attribute map may carry.
pub const MAX_COLLECTION_LEN: usize = 1_000;

/// How many evaluation errors the response reports. Cedar can raise one per
/// policy, so the response needs its own output cap.
pub const MAX_REPORTED_EVALUATION_ERRORS: usize = 100;

/// Parse one Cedar POLICY TEMPLATE, refusing text the parser rejects. `Err`
/// names [`paths::TEMPLATE_STATEMENT`] and carries the parser's diagnostics, for
/// a `ValidationException`.
///
/// A template is Cedar's own notion, not a variant of a static policy:
/// `?principal` and `?resource` are slots, which [`parse_static_policy`]'s
/// `Policy::parse` refuses by design. So the template text goes to the real
/// template parser rather than being pattern-matched or parsed as a policy — and
/// that parser is also what refuses text with NO slot at all, which is a static
/// policy and not a template.
///
/// Refuses a statement past [`MAX_STATEMENT_BYTES`] BEFORE handing it to the
/// parser, so an over-long payload costs a length check rather than a parse.
pub fn parse_policy_template(statement: &str) -> Result<(), InvalidInput> {
    parse_template(None, statement)
        .map(|_| ())
        .map_err(|reason| InvalidInput::new(paths::TEMPLATE_STATEMENT, reason))
}

/// The parsed Cedar template, for the callers that need it: the parse check
/// above, the schema validation below and the template LINK that instantiates it
/// into a policy.
///
/// `id` is the Cedar policy id the template carries in a `PolicySet` — the
/// linking caller needs a known one to link against; `None` takes Cedar's own
/// generated id, which is what a diagnostic about the text then names.
///
/// A refusal carries the PARSER's own diagnostic and nothing else. Verified
/// Permissions IS Cedar, and what it reports for text Cedar will not parse is
/// Cedar's own sentence — the recorded refusal of a static policy submitted to
/// `CreatePolicyTemplate` is exactly the diagnostic this parser produces — so
/// wrapping it in prose of this emulator's own would report a message the service
/// does not word that way. The size bound above is this emulator's own check
/// rather than a parse, so it states itself.
fn parse_template(
    id: Option<cedar_policy::PolicyId>,
    statement: &str,
) -> Result<cedar_policy::Template, String> {
    if statement.len() > MAX_STATEMENT_BYTES {
        return Err(format!(
            "statement is {} bytes, more than the {MAX_STATEMENT_BYTES} bytes a policy \
             template may carry",
            statement.len()
        ));
    }
    cedar_policy::Template::parse(id, statement).map_err(|e| e.to_string())
}

/// Validate one POLICY TEMPLATE's Cedar text against a policy store's schema, as
/// Cedar's own STRICT validation mode does — the template counterpart of
/// [`validate_static_policy`], which `ValidationSettings` documents together with
/// it ("Cedar performs validation on all submitted new or updated static policies
/// and policy templates").
///
/// `schema_json` is the text the store holds (already accepted by
/// [`parse_schema`]); `Err` carries [`Refusal::SchemaValidation`] with ONE reason
/// per diagnostic the validator raised ([`validation_reasons`]).
pub fn validate_policy_template(schema_json: &str, statement: &str) -> Result<(), Refusal> {
    let parsed = parse_schema(schema_json).map_err(|e| {
        // The store's own schema was accepted by `PutSchema`, so this is
        // service-side state the caller cannot fix.
        Refusal::ServiceFault(format!(
            "the policy store's schema could not be parsed, so a policy template cannot be \
             validated against it: {e}"
        ))
    })?;
    let template = parse_template(None, statement)
        .map_err(|reason| InvalidInput::new(paths::TEMPLATE_STATEMENT, reason))?;
    let mut set = cedar_policy::PolicySet::new();
    set.add_template(template).map_err(|e| {
        InvalidInput::new(
            paths::TEMPLATE_STATEMENT,
            format!("the policy template statement could not be loaded: {e}"),
        )
    })?;

    let result = cedar_policy::Validator::new(parsed.schema)
        .validate(&set, cedar_policy::ValidationMode::Strict);
    schema_validation(&result)
}

/// Which of the diagnostics the Cedar validator can raise as a WARNING are
/// validation reasons this service refuses a policy for.
///
/// `ValidationException`'s own documentation enumerates the reasons a policy is
/// refused for (`smithy_query(query="doc", shape="ValidationException")`), and
/// two of them — `InvalidActionApplication` ("the policy includes an action
/// that, according to the schema, doesn't support the specified principal and
/// resource") and `ImpossiblePolicy` ("Cedar has determined that a policy
/// condition always evaluates to false") — are diagnostics the `cedar-policy`
/// crate classifies as warnings rather than as errors. They are reasons here
/// because the service enumerates them as reasons; every other warning kind the
/// crate can raise is a style observation that enumeration does not name (a
/// confusable identifier, an action named in both the scope and a condition), so
/// it is neither reported nor a refusal.
///
/// A policy whose SCOPE admits no request at all carries both: Cedar reports the
/// inapplicable action AND, as a consequence of it, that the policy can never be
/// satisfied. The consequence states nothing the first reason does not, so the
/// impossible-policy reason is reported only when the scope is not itself what
/// was refused — which is also the case the enumeration describes, a policy
/// CONDITION that is always false.
fn reported_warnings(result: &cedar_policy::ValidationResult) -> Vec<String> {
    let inapplicable_action = result.validation_warnings().any(|warning| {
        matches!(
            warning,
            cedar_policy::ValidationWarning::InvalidActionApplication(_)
        )
    });
    result
        .validation_warnings()
        .filter(|warning| match warning {
            cedar_policy::ValidationWarning::InvalidActionApplication(_) => true,
            cedar_policy::ValidationWarning::ImpossiblePolicy(_) => !inapplicable_action,
            _ => false,
        })
        .map(ToString::to_string)
        .collect()
}

/// One entry per reason the Cedar validator raised about the policy, in the
/// order it raised them — every validation ERROR first, then every warning
/// [`reported_warnings`] keeps — and capped at
/// [`MAX_REPORTED_EVALUATION_ERRORS`], the output bound on a refusal that
/// reports one `fieldList` entry per reason.
///
/// EMPTY when the validator raised nothing to report, which is what
/// [`schema_validation`] reads as the policy validating.
fn validation_reasons(result: &cedar_policy::ValidationResult) -> Vec<String> {
    let mut reasons: Vec<String> = result
        .validation_errors()
        .map(ToString::to_string)
        .chain(reported_warnings(result))
        .take(MAX_REPORTED_EVALUATION_ERRORS)
        .collect();
    if reasons.is_empty() && !result.validation_passed() {
        // Defensive: the validator refused the policy, so a caller must still
        // get a reason rather than an empty list.
        reasons.push("it does not validate against the schema in this policy store".to_string());
    }
    reasons
}

/// What one run of the Cedar validator means for the request that submitted the
/// policy: `Ok` when it raised no reason at all, and otherwise the refusal
/// carrying ONE reason per diagnostic it raised.
///
/// The ONE place that decides this, so every operation validating against a
/// store's schema — a static policy, a policy template, a template-linked policy
/// — refuses on the same diagnostics and reports all of them.
fn schema_validation(result: &cedar_policy::ValidationResult) -> Result<(), Refusal> {
    let reasons = validation_reasons(result);
    if reasons.is_empty() {
        return Ok(());
    }
    Err(Refusal::SchemaValidation(reasons))
}

/// How many authorization requests one batch call may carry, from
/// `BatchIsAuthorized`'s own model documentation: "The requests of a
/// BatchIsAuthorized API request can contain up to 30 requests." The model's
/// `@length` on `BatchIsAuthorizedInputList` is `min: 1` only, so the boundary
/// validator cannot refuse an over-long list and this is the place that does. It
/// is also the batch's output bound: one result per request.
pub const MAX_BATCH_REQUESTS: usize = 30;

/// How many bytes of Cedar schema JSON one policy store may hold. The model puts
/// no upper `@length` on `SchemaJson` (`min 1` only), so the boundary validator
/// cannot refuse an over-long one: this is the emulator's bound on the text
/// `PutSchema` stores and that every later validation re-parses. CHOSEN, not an
/// AWS-published quota, and sized well above any hand-written schema.
pub const MAX_SCHEMA_BYTES: usize = 100_000;

/// Everything ONE scope position of a Cedar policy references, as the parser
/// reports its constraint: the entity type the position is restricted to and the
/// entity it names, each present only when the constraint carries it.
///
/// Both members are needed because a position can reference a type without
/// naming an entity (`principal is T`) and name an entity without restricting
/// the type (`principal == E`, `principal in E`), and `principal is T in E`
/// references both. Two positions are therefore the same reference only when
/// they agree on BOTH — which is what the update operations hold a replacement
/// to, since the entity type is part of the principal (or resource) a policy
/// references and not a detail below it.
///
/// What the API REPORTS of a position is narrower: only the entity
/// ([`ScopeReference::entity`]), because `GetPolicyOutput$principal` "isn't
/// included in the response when Principal isn't present in the policy content".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScopeReference {
    /// The entity type the position is restricted to — `T` of `is T` and of
    /// `is T in E`; `None` for a position that restricts no type (`principal`,
    /// `principal == E`, `principal in E`).
    pub entity_type: Option<String>,
    /// The entity the position names — `E` of `== E`, `in E` and `is T in E`;
    /// `None` for a position that names none (`principal`, `principal is T`).
    pub entity: Option<ScopeEntity>,
}

impl ScopeReference {
    /// The entity this position names, which is the part of the reference the
    /// API reports and persists — `None` for a position that names no entity.
    pub fn entity(&self) -> Option<&ScopeEntity> {
        self.entity.as_ref()
    }
}

/// What the Cedar parser says a static policy's scope is: the pieces
/// `CreatePolicy`, `GetPolicy` and `ListPolicies` report back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticScope {
    /// `Permit` or `Forbid`.
    pub effect: String,
    /// Everything the scope's principal position references.
    pub principal: ScopeReference,
    /// Same rule as `principal`, for the resource position.
    pub resource: ScopeReference,
    /// Every action the scope names; empty when the scope is a bare `action`.
    pub actions: Vec<ScopeEntity>,
}

/// Refuse a statement past [`MAX_STATEMENT_BYTES`] — the CHEAP check, so an
/// over-long payload costs a length comparison rather than a Cedar parse or a
/// schema validation. Called by every path that hands a caller's statement to
/// Cedar ([`parse_static_policy`], [`validate_static_policy`]) and, before any
/// of them, by the handler, so the bound dominates whichever runs first.
///
/// The model puts no upper `@length` on `PolicyStatement`, so the boundary
/// validator cannot refuse an over-long one and this is the place that does.
pub fn check_statement_size(statement: &str) -> Result<(), InvalidInput> {
    if statement.len() > MAX_STATEMENT_BYTES {
        return Err(InvalidInput::new(
            paths::STATIC_STATEMENT,
            format!(
                "the statement is {} bytes, more than the {MAX_STATEMENT_BYTES} bytes a \
                 policy may carry",
                statement.len()
            ),
        ));
    }
    Ok(())
}

/// Parse one Cedar static policy and read its effect and scope off the parsed
/// form. `Err` names [`paths::STATIC_STATEMENT`] and carries the parser's own
/// diagnostic unwrapped, for a `ValidationException` — the same rule
/// [`parse_template`] states.
///
/// Refuses a statement past [`MAX_STATEMENT_BYTES`] BEFORE handing it to the
/// parser, so an over-long payload costs a length check rather than a parse.
pub fn parse_static_policy(statement: &str) -> Result<StaticScope, InvalidInput> {
    check_statement_size(statement)?;
    let policy = cedar_policy::Policy::parse(None, statement)
        .map_err(|e| InvalidInput::new(paths::STATIC_STATEMENT, e.to_string()))?;

    Ok(scope_of(&policy))
}

/// The effect and scope of ONE parsed Cedar policy, however it was obtained:
/// parsed from a static policy's text, or LINKED from a template
/// ([`link_template`]). Both report the same members, so they read them off the
/// parsed form through this one function.
///
/// Each scope position is reported in FULL ([`ScopeReference`]): the entity type
/// an `is T` constraint restricts the position to is part of what that position
/// references, so it is read off the constraint rather than dropped, and the
/// entity the position names is reported beside it.
fn scope_of(policy: &cedar_policy::Policy) -> StaticScope {
    let effect = effect_name(policy.effect());

    let principal = match policy.principal_constraint() {
        cedar_policy::PrincipalConstraint::Any => ScopeReference::default(),
        cedar_policy::PrincipalConstraint::Is(entity_type) => ScopeReference {
            entity_type: Some(entity_type.to_string()),
            entity: None,
        },
        cedar_policy::PrincipalConstraint::Eq(uid)
        | cedar_policy::PrincipalConstraint::In(uid) => ScopeReference {
            entity_type: None,
            entity: Some(scope_entity(&uid)),
        },
        cedar_policy::PrincipalConstraint::IsIn(entity_type, uid) => ScopeReference {
            entity_type: Some(entity_type.to_string()),
            entity: Some(scope_entity(&uid)),
        },
    };
    let resource = match policy.resource_constraint() {
        cedar_policy::ResourceConstraint::Any => ScopeReference::default(),
        cedar_policy::ResourceConstraint::Is(entity_type) => ScopeReference {
            entity_type: Some(entity_type.to_string()),
            entity: None,
        },
        cedar_policy::ResourceConstraint::Eq(uid)
        | cedar_policy::ResourceConstraint::In(uid) => ScopeReference {
            entity_type: None,
            entity: Some(scope_entity(&uid)),
        },
        cedar_policy::ResourceConstraint::IsIn(entity_type, uid) => ScopeReference {
            entity_type: Some(entity_type.to_string()),
            entity: Some(scope_entity(&uid)),
        },
    };
    let actions = actions_of(policy.action_constraint());

    StaticScope {
        effect,
        principal,
        resource,
        actions,
    }
}

/// `Permit` or `Forbid`, as the API words a policy's (or a policy template's)
/// effect. One function, so a static policy and a template cannot word the same
/// effect differently.
fn effect_name(effect: cedar_policy::Effect) -> String {
    match effect {
        cedar_policy::Effect::Permit => "Permit",
        cedar_policy::Effect::Forbid => "Forbid",
    }
    .to_string()
}

/// Every action a scope names, empty for a bare `action`. Cedar has no
/// placeholder in the action position, so a template's action constraint is read
/// exactly as a static policy's is.
fn actions_of(constraint: cedar_policy::ActionConstraint) -> Vec<ScopeEntity> {
    match constraint {
        cedar_policy::ActionConstraint::Any => Vec::new(),
        cedar_policy::ActionConstraint::Eq(uid) => vec![scope_entity(&uid)],
        cedar_policy::ActionConstraint::In(uids) => uids.iter().map(scope_entity).collect(),
    }
}

/// A parsed Cedar schema: the `cedar-policy` schema object every validation
/// needs, plus the namespaces the schema declares.
pub struct ParsedSchema {
    /// The schema as the Cedar validator and the request validator consume it.
    pub schema: cedar_policy::Schema,
    /// The namespaces the schema declares, which `PutSchema`/`GetSchema` report.
    pub namespaces: Vec<String>,
}

/// Parse one Cedar schema, written in Cedar schema JSON, and read the namespaces
/// it declares off the parsed form. `Err` carries the schema parser's own
/// diagnostics, for a `ValidationException`.
///
/// `text` must ALREADY be bounded by [`MAX_SCHEMA_BYTES`]: that limit has exactly
/// one owner, `PutSchema`, which is the only way schema text enters the service
/// and which refuses an over-long definition with the modelled quota error. Every
/// other caller here parses text this service already stored, so it is bounded by
/// construction — and that bound is the whole step budget for a parse, whose cost
/// is in the bytes it reads, with `serde_json`'s own recursion limit bounding how
/// deep the nesting inside those bytes can drive the parser's stack. No count or
/// depth cap is imposed on top, because a real schema's entity-type and action
/// counts are bounded only by its size.
///
/// The namespaces come from the schema JSON's top-level members, which is what
/// that format defines them to be: a Cedar JSON schema is an object keyed by
/// namespace name, each value holding that namespace's `entityTypes`, `actions`
/// and `commonTypes`. The unnamed (empty) namespace is the key `""`, and is
/// reported as such — `Namespace`'s pattern is `^.*$`, which permits it.
pub fn parse_schema(text: &str) -> Result<ParsedSchema, String> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| format!("the schema is not valid JSON: {e}"))?;
    let namespaces: Vec<String> = value
        .as_object()
        .map(|members| members.keys().cloned().collect())
        .unwrap_or_default();
    let schema = cedar_policy::Schema::from_json_value(value)
        .map_err(|e| format!("the schema is not a valid Cedar schema: {e}"))?;
    Ok(ParsedSchema { schema, namespaces })
}

/// Validate one static policy's Cedar text against a policy store's schema, as
/// Cedar's own STRICT validation mode does: every entity type, action and
/// attribute the policy names must be declared by the schema and used at the type
/// the schema gives it.
///
/// `schema_json` is the text the store holds (already accepted by
/// [`parse_schema`]); an `Err` carrying [`Refusal::SchemaValidation`] has ONE
/// reason per diagnostic the validator raised ([`validation_reasons`]),
/// [`Refusal::InvalidInput`] names the statement, and [`Refusal::ServiceFault`]
/// says the store's own schema is what could not be read.
pub fn validate_static_policy(schema_json: &str, statement: &str) -> Result<(), Refusal> {
    // The statement is bounded BEFORE the schema is parsed and before Cedar sees
    // the statement, so the cheap check dominates both parses.
    check_statement_size(statement)?;
    // The schema is text this service already accepted from `PutSchema`, so a
    // parse failure here is service-side state the caller cannot fix, not a bad
    // request — it is reported as a server fault rather than blamed on the
    // statement.
    let parsed = parse_schema(schema_json).map_err(|e| {
        Refusal::ServiceFault(format!(
            "the policy store's schema could not be parsed, so a static policy cannot be \
             validated against it: {e}"
        ))
    })?;
    let policy = cedar_policy::Policy::parse(None, statement)
        .map_err(|e| InvalidInput::new(paths::STATIC_STATEMENT, e.to_string()))?;
    let mut set = cedar_policy::PolicySet::new();
    set.add(policy).map_err(|e| {
        InvalidInput::new(
            paths::STATIC_STATEMENT,
            format!("the policy statement could not be loaded: {e}"),
        )
    })?;

    let result = cedar_policy::Validator::new(parsed.schema)
        .validate(&set, cedar_policy::ValidationMode::Strict);
    schema_validation(&result)
}

/// The Cedar policy id a template carries while it is linked, and the id its
/// link carries — both local to the one-template `PolicySet` [`link_template`]
/// builds, so neither reaches a response.
const LINK_TEMPLATE_ID: &str = "template";
const LINK_POLICY_ID: &str = "link";

/// INSTANTIATE a policy template into a policy: bind the template's
/// `?principal` / `?resource` slots to the entities the request named and report
/// the effect and scope of the policy that results.
///
/// This is Cedar's own template linking (`PolicySet::link`), not a text
/// substitution: the scope comes back off the LINKED policy, so a slot the
/// template does not use keeps whatever the template's own scope says and the
/// effect and actions are the template's.
///
/// The slot rule is the template's: a slot the template USES must be bound by
/// the link, and one it does not use must not be — each refusal naming the
/// member that is missing or surplus. `template_statement` is the text the store
/// holds, so text it cannot parse is service-side state, not a bad request.
/// `schema_json`, when the store validates, is what the linked policy is
/// validated against, exactly as a static policy's text would be.
pub fn link_template(
    template_statement: &str,
    link: &super::TemplateLink,
    schema_json: Option<&str>,
) -> Result<StaticScope, Refusal> {
    let template_id = cedar_policy::PolicyId::new(LINK_TEMPLATE_ID);
    let link_id = cedar_policy::PolicyId::new(LINK_POLICY_ID);
    let template = parse_template(Some(template_id.clone()), template_statement)
        .map_err(|e| Refusal::ServiceFault(format!("the policy template cannot be linked: {e}")))?;

    // Which slots the template uses is the TEMPLATE's property, read off the
    // parsed form rather than searched for in the text.
    let slots: BTreeSet<String> = template.slots().map(ToString::to_string).collect();
    for (slot, bound, path) in [
        (
            cedar_policy::SlotId::principal(),
            link.principal.as_ref(),
            paths::TEMPLATE_LINKED_PRINCIPAL,
        ),
        (
            cedar_policy::SlotId::resource(),
            link.resource.as_ref(),
            paths::TEMPLATE_LINKED_RESOURCE,
        ),
    ] {
        match (slots.contains(&slot.to_string()), bound) {
            (true, None) => {
                return Err(InvalidInput::new(
                    path,
                    "the policy template's scope uses this placeholder, so the policy that \
                     instantiates it has to bind the placeholder to an entity",
                )
                .into());
            }
            (false, Some(_)) => {
                return Err(InvalidInput::new(
                    path,
                    "the policy template's scope does not use this placeholder, so the policy \
                     that instantiates it cannot bind it",
                )
                .into());
            }
            (true, Some(_)) | (false, None) => {}
        }
    }
    let values = slot_values(link)?;

    let (set, scope) = link_one(template, template_id, link_id, values).map_err(|failure| {
        match failure {
            LinkFailure::Load(e) => {
                Refusal::ServiceFault(format!("the policy template could not be loaded: {e}"))
            }
            // Linking fails on the VALUES the link bound (a slot Cedar will
            // not accept the entity for), so it is the caller's placeholder
            // binding that is named.
            LinkFailure::Link(e) => InvalidInput::new(
                paths::TEMPLATE_LINKED_DEFINITION,
                format!("the policy template could not be instantiated with these values: {e}"),
            )
            .into(),
            LinkFailure::Unreadable => Refusal::ServiceFault(
                "the policy template was linked but the resulting policy could not be read"
                    .to_string(),
            ),
        }
    })?;

    if let Some(schema_json) = schema_json {
        let parsed = parse_schema(schema_json).map_err(|e| {
            Refusal::ServiceFault(format!(
                "the policy store's schema could not be parsed, so a template-linked policy \
                 cannot be validated against it: {e}"
            ))
        })?;
        let result = cedar_policy::Validator::new(parsed.schema)
            .validate(&set, cedar_policy::ValidationMode::Strict);
        schema_validation(&result)?;
    }

    Ok(scope)
}

/// Which step of [`link_one`] failed, and the diagnostic Cedar gave for it. The
/// step is reported rather than a finished message, because the two callers
/// answer on different error channels and name different request members for the
/// same failure.
enum LinkFailure {
    /// The parsed template could not be added to the policy set.
    Load(String),
    /// Cedar refused the slot bindings.
    Link(String),
    /// The linked policy could not be read back out of the set.
    Unreadable,
}

/// Link ONE parsed template, in a policy set holding only it, and report the
/// scope of the policy that results — plus the set itself, which a caller that
/// validates against a schema needs.
///
/// The mechanics of instantiating a template (add the template, link it under
/// `link_id`, read the scope off the LINKED policy) kept apart from how a
/// failure is reported, because the step that failed is what the caller needs to
/// word its own refusal.
fn link_one(
    template: cedar_policy::Template,
    template_id: cedar_policy::PolicyId,
    link_id: cedar_policy::PolicyId,
    values: HashMap<cedar_policy::SlotId, cedar_policy::EntityUid>,
) -> Result<(cedar_policy::PolicySet, StaticScope), LinkFailure> {
    let mut set = cedar_policy::PolicySet::new();
    set.add_template(template)
        .map_err(|e| LinkFailure::Load(e.to_string()))?;
    set.link(template_id, link_id.clone(), values)
        .map_err(|e| LinkFailure::Link(e.to_string()))?;
    let scope = {
        let linked = set.policy(&link_id).ok_or(LinkFailure::Unreadable)?;
        scope_of(linked)
    };
    Ok((set, scope))
}

/// What ONE scope position of a policy TEMPLATE NAMES.
///
/// A template's position has one case a static policy's does not: it can hold
/// the `?principal` / `?resource` PLACEHOLDER the instantiating policy binds,
/// which is a case of its own beside naming an entity and naming none. Cedar's
/// template parser reports exactly that distinction — a constraint carrying no
/// entity is a slot — so it is carried here as a case of its own and never
/// stood in for by some entity: any entity chosen to stand in for a placeholder
/// is one a caller's own statement may name, and the two cases would then
/// compare equal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum TemplateScopeName {
    /// The position names nothing (`principal`, `principal is T`).
    #[default]
    Nothing,
    /// The position names this entity (`principal == E`, `principal in E`).
    Entity(ScopeEntity),
    /// The position holds the template's placeholder
    /// (`principal == ?principal`), which the policy that instantiates the
    /// template binds.
    Placeholder,
}

/// Everything ONE scope position of a policy TEMPLATE references: the entity
/// type the position is restricted to, and what it names.
///
/// The template counterpart of [`ScopeReference`], and the same rule — two
/// positions are the same reference only when they agree on BOTH members.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TemplateScopeReference {
    /// The entity type the position is restricted to — `T` of `is T` and of
    /// `is T in E`; `None` for a position that restricts no type.
    pub entity_type: Option<String>,
    /// What the position names.
    pub named: TemplateScopeName,
}

/// The effect and scope of one Cedar policy TEMPLATE, read off the parsed
/// template — the template counterpart of [`StaticScope`].
///
/// A template's scope can name an entity (`principal == User::"erin"`), hold a
/// placeholder for the instantiating policy (`principal == ?principal`) or name
/// nothing at all (`principal`), and those three are distinct references. That
/// is what lets `UpdatePolicyTemplate` hold a replacement to the elements its
/// own documentation makes immutable — "You can't change the following elements:
/// The effect (permit or forbid) of the policy template. The principal
/// referenced by the policy template. The resource referenced by the policy
/// template." — by comparing two scopes rather than two strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateScope {
    /// `Permit` or `Forbid`.
    pub effect: String,
    /// Everything the scope's principal position references.
    pub principal: TemplateScopeReference,
    /// Same rule as `principal`, for the resource position.
    pub resource: TemplateScopeReference,
    /// Every action the scope names; empty when the scope is a bare `action`.
    pub actions: Vec<ScopeEntity>,
}

/// The effect and scope of one Cedar policy TEMPLATE, read off the parsed form.
///
/// Each position comes from the TEMPLATE's own constraint, which Cedar reports
/// with the slot left as a slot, so a placeholder is never confused with an
/// entity some statement could name. `Err` names
/// [`paths::TEMPLATE_STATEMENT`] and carries the template parser's own
/// diagnostic, exactly as [`parse_policy_template`] does.
pub fn template_scope(statement: &str) -> Result<TemplateScope, InvalidInput> {
    let template = parse_template(None, statement)
        .map_err(|reason| InvalidInput::new(paths::TEMPLATE_STATEMENT, reason))?;

    let principal = match template.principal_constraint() {
        cedar_policy::TemplatePrincipalConstraint::Any => TemplateScopeReference::default(),
        cedar_policy::TemplatePrincipalConstraint::Is(entity_type) => TemplateScopeReference {
            entity_type: Some(entity_type.to_string()),
            named: TemplateScopeName::Nothing,
        },
        cedar_policy::TemplatePrincipalConstraint::Eq(uid)
        | cedar_policy::TemplatePrincipalConstraint::In(uid) => TemplateScopeReference {
            entity_type: None,
            named: scope_name(uid.as_ref()),
        },
        cedar_policy::TemplatePrincipalConstraint::IsIn(entity_type, uid) => {
            TemplateScopeReference {
                entity_type: Some(entity_type.to_string()),
                named: scope_name(uid.as_ref()),
            }
        }
    };
    let resource = match template.resource_constraint() {
        cedar_policy::TemplateResourceConstraint::Any => TemplateScopeReference::default(),
        cedar_policy::TemplateResourceConstraint::Is(entity_type) => TemplateScopeReference {
            entity_type: Some(entity_type.to_string()),
            named: TemplateScopeName::Nothing,
        },
        cedar_policy::TemplateResourceConstraint::Eq(uid)
        | cedar_policy::TemplateResourceConstraint::In(uid) => TemplateScopeReference {
            entity_type: None,
            named: scope_name(uid.as_ref()),
        },
        cedar_policy::TemplateResourceConstraint::IsIn(entity_type, uid) => {
            TemplateScopeReference {
                entity_type: Some(entity_type.to_string()),
                named: scope_name(uid.as_ref()),
            }
        }
    };

    Ok(TemplateScope {
        effect: effect_name(template.effect()),
        principal,
        resource,
        actions: actions_of(template.action_constraint()),
    })
}

/// What a template scope position that CARRIES an entity reference names: the
/// entity, or — when Cedar reports the reference without one — the template's
/// placeholder in that position.
fn scope_name(uid: Option<&cedar_policy::EntityUid>) -> TemplateScopeName {
    match uid {
        Some(uid) => TemplateScopeName::Entity(scope_entity(uid)),
        None => TemplateScopeName::Placeholder,
    }
}

/// The Cedar slot environment one template link binds: an entity per
/// placeholder the link carries.
///
/// A link's bindings ARE the slots its template uses — [`link_template`] refuses
/// a policy that binds a placeholder the template does not use, or leaves one it
/// does use unbound — so this is a translation of what the link holds, not a
/// decision about it. `Err` names the binding Cedar cannot read as an entity.
fn slot_values(
    link: &super::TemplateLink,
) -> Result<HashMap<cedar_policy::SlotId, cedar_policy::EntityUid>, InvalidInput> {
    let mut values: HashMap<cedar_policy::SlotId, cedar_policy::EntityUid> = HashMap::new();
    for (slot, bound, path) in [
        (
            cedar_policy::SlotId::principal(),
            link.principal.as_ref(),
            paths::TEMPLATE_LINKED_PRINCIPAL,
        ),
        (
            cedar_policy::SlotId::resource(),
            link.resource.as_ref(),
            paths::TEMPLATE_LINKED_RESOURCE,
        ),
    ] {
        if let Some(entity) = bound {
            values.insert(slot, entity_uid(entity, path)?);
        }
    }
    Ok(values)
}

/// A Cedar `Type::"id"` as the API's `entityType`/`entityId` pair.
///
/// `EntityId::as_ref` gives the id's RAW contents. `escaped()` would give it
/// re-escaped for Cedar source syntax, which is wrong on the wire: an id
/// containing a quote would come back with a backslash the caller never sent.
fn scope_entity(uid: &cedar_policy::EntityUid) -> ScopeEntity {
    ScopeEntity {
        entity_type: uid.type_name().to_string(),
        entity_id: <cedar_policy::EntityId as AsRef<str>>::as_ref(uid.id()).to_string(),
    }
}

/// What the Cedar authorizer decided, in the shape `IsAuthorized` answers with.
#[derive(Debug, Clone)]
pub struct Verdict {
    /// `ALLOW` or `DENY`.
    pub decision: String,
    /// The ids of the policies that determined the decision, in the order Cedar
    /// reported them.
    pub determining_policies: Vec<String>,
    /// One description per error Cedar raised while evaluating, capped at
    /// [`MAX_REPORTED_EVALUATION_ERRORS`]; empty when every policy evaluated
    /// cleanly.
    pub errors: Vec<String>,
}

/// A store's static policies and its schema, parsed once for one API call, plus
/// the load diagnostics every request evaluated against them reports.
///
/// One API call parses the store's Cedar text ONCE and evaluates each of its
/// requests against the result, so a batch of [`MAX_BATCH_REQUESTS`] requests
/// costs one parse of at most [`crate::state::MAX_POLICY_BYTES_PER_STORE`] bytes
/// (plus one schema parse) rather than one per request.
#[derive(Debug)]
pub struct PreparedPolicies {
    /// Every policy that parsed, each carrying its `policyId` as its Cedar
    /// policy id so `determiningPolicies` names it.
    set: cedar_policy::PolicySet,
    /// The store's schema, when it holds one: what gives the request's entity
    /// types, action and context their types.
    schema: Option<cedar_policy::Schema>,
    /// One description per policy that could NOT be loaded. Reported on every
    /// request's `errors`, because every request was evaluated without it.
    load_errors: Vec<String>,
}

impl PreparedPolicies {
    /// The store's parsed schema, for the entity store every request of this call
    /// is evaluated against.
    pub fn schema(&self) -> Option<&cedar_policy::Schema> {
        self.schema.as_ref()
    }
}

/// Parse a store's policies, and its schema when it holds one, into what an
/// authorization request is evaluated against.
///
/// A STATIC policy contributes its own Cedar text. A TEMPLATE-LINKED one
/// contributes its template's text, LINKED to the entities the policy bound:
/// `CreatePolicyTemplate` documents that "if the template changes, then any
/// policies that are linked to that template are immediately updated as well",
/// so the template's text is re-read on every evaluation rather than snapshotted
/// into the policy.
///
/// A policy that will not parse or link is reported as an evaluation error rather
/// than silently skipped, so a store holding one is never reported as a clean
/// deny.
///
/// The store's schema, when it holds one, is what gives the request's entity
/// types, action and context their types, so when one is present the entity
/// store and the request are parsed AGAINST it where it types them
/// ([`build_entities`], [`evaluate`]) — and evaluated untyped where it does not,
/// because a value the schema cannot type is still evaluated and reported on
/// through the response's `errors` rather than refused.
///
/// The per-call step budget is enforced here, at the point of work: the total
/// Cedar text one call parses is capped at
/// [`crate::state::MAX_POLICY_BYTES_PER_STORE`], and the policy count at
/// [`crate::state::MAX_POLICIES_PER_STORE`].
pub fn load_policies(store: &super::PolicyStore) -> Result<PreparedPolicies, Refusal> {
    let mut set = cedar_policy::PolicySet::new();
    let mut errors: Vec<String> = Vec::new();
    let mut parsed_bytes = 0usize;
    let mut parsed_count = 0usize;
    // Each template is added to the set at most once, however many policies link
    // to it, so a store of links to one template costs one template parse.
    let mut linked_templates: BTreeSet<String> = BTreeSet::new();
    for (policy_id, policy) in &store.policies {
        let statement = match (policy.statement.as_deref(), policy.template_link.as_ref()) {
            (Some(statement), _) => statement,
            (None, Some(link)) => {
                match store.policy_templates.get(&link.policy_template_id) {
                    Some(template) => template.statement.as_str(),
                    None => {
                        errors.push(format!(
                            "policy {policy_id} is linked to policy template {}, which this \
                             policy store no longer holds",
                            link.policy_template_id
                        ));
                        continue;
                    }
                }
            }
            (None, None) => {
                errors.push(format!(
                    "policy {policy_id} holds neither a policy statement nor a policy \
                     template to link"
                ));
                continue;
            }
        };
        // The per-call step budget counts the Cedar text this call PARSES: a
        // static policy's own statement, and each distinct template ONCE —
        // however many policies link to it, it is parsed once, which is also how
        // [`crate::state::PolicyStore::cedar_bytes`] charges the store for it.
        // Charging every link for the whole template would refuse evaluation of
        // a store the create path legitimately accepted.
        let parses_new_text = match policy.template_link.as_ref() {
            None => true,
            Some(link) => linked_templates.insert(link.policy_template_id.clone()),
        };
        if parses_new_text {
            parsed_bytes += statement.len();
        }
        parsed_count += 1;
        if parsed_bytes > super::MAX_POLICY_BYTES_PER_STORE
            || parsed_count > super::MAX_POLICIES_PER_STORE
        {
            // Service-side state, not the request: the store holds more Cedar
            // than one request evaluates, so no request against it can be
            // answered and there is no member for the caller to fix.
            return Err(Refusal::ServiceFault(format!(
                "the policy store holds more Cedar text than one authorization request \
                 evaluates ({} bytes / {} policies)",
                super::MAX_POLICY_BYTES_PER_STORE,
                super::MAX_POLICIES_PER_STORE
            )));
        }
        match policy.template_link.as_ref() {
            // A static policy: its own Cedar text, under its own `policyId` as
            // the Cedar policy id, so `determiningPolicies` names it.
            None => match cedar_policy::Policy::parse(
                Some(cedar_policy::PolicyId::new(policy_id)),
                statement,
            ) {
                Ok(parsed) => {
                    if let Err(e) = set.add(parsed) {
                        errors.push(format!("policy {policy_id} could not be loaded: {e}"));
                    }
                }
                Err(e) => errors.push(format!("policy {policy_id} could not be parsed: {e}")),
            },
            // A template-linked policy: Cedar's own LINK of the template this
            // policy was instantiated from, bound to the entities the link
            // carries. The link takes the policy's own id, so a decision it
            // determines names the policy and not the template.
            Some(link) => {
                let template_id = link.policy_template_id.as_str();
                let template_pid = cedar_policy::PolicyId::new(template_id);
                if parses_new_text {
                    match parse_template(Some(template_pid.clone()), statement) {
                        Ok(template) => {
                            if let Err(e) = set.add_template(template) {
                                errors.push(format!(
                                    "policy template {template_id} could not be loaded: {e}"
                                ));
                                continue;
                            }
                        }
                        Err(e) => {
                            errors.push(format!(
                                "policy template {template_id} could not be parsed: {e}"
                            ));
                            continue;
                        }
                    }
                }
                let values = match slot_values(link) {
                    Ok(values) => values,
                    Err(reason) => {
                        errors.push(format!(
                            "policy {policy_id} could not be linked to policy template \
                             {template_id}: {reason}"
                        ));
                        continue;
                    }
                };
                let link_id = cedar_policy::PolicyId::new(policy_id);
                if let Err(e) = set.link(template_pid, link_id, values) {
                    errors.push(format!(
                        "policy {policy_id} could not be linked to policy template \
                         {template_id}: {e}"
                    ));
                }
            }
        }
    }
    let schema = match store.schema.as_ref() {
        Some(schema) => Some(
            parse_schema(&schema.cedar_json)
                .map_err(|e| {
                    // The store's own schema was accepted by `PutSchema`, so this
                    // is service-side state the caller cannot fix.
                    Refusal::ServiceFault(format!(
                        "the policy store's schema could not be parsed: {e}"
                    ))
                })?
                .schema,
        ),
        None => None,
    };
    Ok(PreparedPolicies {
        set,
        schema,
        load_errors: errors,
    })
}

/// Build the Cedar entity store one API call evaluates against, from the Cedar
/// JSON [`entities_to_cedar_json`] already bounded, typed by the store's schema
/// when the schema types it. The refusal names the `entities` member.
///
/// A schema is what gives a schema-declared action its entity and an extension
/// attribute its type, so the slice is parsed AGAINST the schema first. But
/// conformance to the store's schema is NOT part of this request's input
/// contract: the schema constrains the policies a store accepts (see
/// [`validate_static_policy`], applied per the store's validation mode), not the
/// values an authorization request carries. So a slice the schema cannot type is
/// still parsed, untyped, and evaluated — whatever the evaluation then raises
/// about it is reported in-band in the response's `errors`. Only a payload that
/// is no Cedar entity store at all (an unparseable entity type, a uid repeated in
/// a `cedarJson` store) is refused as a bad request; a repeated identifier in the
/// modelled `entityList` is not, because [`entities_to_cedar_json`] has already
/// collapsed it onto the entity given last.
pub fn build_entities(
    entities: serde_json::Value,
    schema: Option<&cedar_policy::Schema>,
) -> Result<cedar_policy::Entities, InvalidInput> {
    if let Some(schema) = schema {
        if let Ok(typed) =
            cedar_policy::Entities::from_json_value(entities.clone(), Some(schema))
        {
            return Ok(typed);
        }
    }
    cedar_policy::Entities::from_json_value(entities, None).map_err(|e| {
        InvalidInput::new(
            paths::ENTITIES,
            format!("the entities are not a valid Cedar entity store: {e}"),
        )
    })
}

/// One call's `context`: the Cedar JSON an evaluation reads, and the member of
/// the request it came from — which is the member a refusal about it names.
///
/// The two travel together because `ContextDefinition` is a union: the JSON
/// alone no longer says which arm the client sent.
pub struct RequestContext<'a> {
    /// The context as Cedar JSON, from [`context_to_cedar_json`].
    pub json: serde_json::Value,
    /// Where it came from, from [`context_path`].
    pub path: &'a str,
}

/// Evaluate ONE authorization request against an already-parsed policy set.
///
/// `principal`, `action` and `resource` are the request's triple and `context` is
/// its Cedar JSON context, already bounded by [`context_to_cedar_json`]. The
/// request is built against the store's schema when the schema types it, and
/// evaluated UNTYPED when it does not: schema conformance is not part of this
/// request's input contract, so a context value whose type the schema does not
/// sanction is still evaluated, and the type error a policy reading it raises is
/// reported in the verdict's `errors` with that policy excluded from the
/// determining ones — the decision fails closed rather than the call being
/// refused.
///
/// `context_path` is where the call's `context` came from ([`context_path`]), so
/// a refusal about it names the union arm the client actually sent.
pub fn evaluate(
    prepared: &PreparedPolicies,
    principal: &ScopeEntity,
    action: &ScopeEntity,
    resource: &ScopeEntity,
    context: RequestContext<'_>,
    entities: &cedar_policy::Entities,
) -> Result<Verdict, InvalidInput> {
    let principal_uid = entity_uid(principal, paths::PRINCIPAL)?;
    let action_uid = entity_uid(action, paths::ACTION)?;
    let resource_uid = entity_uid(resource, paths::RESOURCE)?;

    let context_path = context.path;
    let context_json = context.json;
    // The context is parsed as Cedar values; a payload that is no Cedar context
    // at all is the one thing refused about it, and that IS part of the input
    // contract.
    let cedar_context = |json: serde_json::Value| {
        cedar_policy::Context::from_json_value(json, None).map_err(|e| {
            InvalidInput::new(
                context_path,
                format!("the context is not a valid Cedar context: {e}"),
            )
        })
    };
    // Building the request against the schema is what lets the evaluation use
    // the types the schema declares. A request the schema does not sanction —
    // a context attribute at another type, an absent or undeclared attribute, an
    // action whose `appliesTo` does not cover the triple — is NOT refused: it is
    // built untyped and evaluated, and the evaluation reports whatever it raises
    // in `errors`.
    let request = match prepared.schema() {
        Some(schema) => cedar_policy::Request::new(
            principal_uid.clone(),
            action_uid.clone(),
            resource_uid.clone(),
            cedar_context(context_json.clone())?,
            Some(schema),
        )
        .ok(),
        None => None,
    };
    let request = match request {
        Some(request) => request,
        None => cedar_policy::Request::new(
            principal_uid,
            action_uid,
            resource_uid,
            cedar_context(context_json)?,
            None,
        )
        .map_err(|e| {
            // Unreachable with no schema to validate against, but a caller still
            // gets the member rather than a panic.
            InvalidInput::new(
                paths::ACTION,
                format!("the authorization request could not be built: {e}"),
            )
        })?,
    };

    let response = cedar_policy::Authorizer::new().is_authorized(&request, &prepared.set, entities);

    let decision = match response.decision() {
        cedar_policy::Decision::Allow => "ALLOW",
        cedar_policy::Decision::Deny => "DENY",
    }
    .to_string();
    let determining_policies = response
        .diagnostics()
        .reason()
        .map(ToString::to_string)
        .collect();
    let mut errors = prepared.load_errors.clone();
    errors.extend(response.diagnostics().errors().map(ToString::to_string));
    // `EvaluationErrorList` is `@uniqueItems`, so the same description must not
    // appear twice; the first occurrence keeps its position.
    let mut seen = std::collections::BTreeSet::new();
    errors.retain(|error| seen.insert(error.clone()));
    errors.truncate(MAX_REPORTED_EVALUATION_ERRORS);

    Ok(Verdict {
        decision,
        determining_policies,
        errors,
    })
}

/// Evaluate one authorization request against a policy STORE:
/// [`load_policies`] followed by [`build_entities`] and [`evaluate`], which is
/// what a single-request operation needs.
///
/// `context` and `entities` are Cedar JSON already built and already bounded by
/// [`context_to_cedar_json`] / [`entities_to_cedar_json`]; the store's policies,
/// its policy templates and its schema all come from the one `store`, so an
/// evaluation can never see one of them without the others.
pub fn authorize(
    store: &super::PolicyStore,
    principal: &ScopeEntity,
    action: &ScopeEntity,
    resource: &ScopeEntity,
    context: RequestContext<'_>,
    entities: serde_json::Value,
) -> Result<Verdict, Refusal> {
    let prepared = load_policies(store)?;
    let entities = build_entities(entities, prepared.schema())?;
    Ok(evaluate(
        &prepared, principal, action, resource, context, &entities,
    )?)
}

/// An `EntityIdentifier` request member as a state scope entity. `None` for an
/// absent member: an authorization request is evaluated as a complete Cedar
/// triple, so every caller refuses an absent one by naming the member rather
/// than answering with a deny.
pub fn request_entity(identifier: Option<&EntityIdentifier>) -> Option<ScopeEntity> {
    identifier.map(|e| ScopeEntity {
        entity_type: e.entity_type.clone(),
        entity_id: e.entity_id.clone(),
    })
}

/// An `ActionIdentifier` request member as a state scope entity: an action is a
/// Cedar entity too, whose type and id arrive as `actionType`/`actionId`.
pub fn request_action(action: Option<&ActionIdentifier>) -> Option<ScopeEntity> {
    action.map(|a| ScopeEntity {
        entity_type: a.action_type.clone(),
        entity_id: a.action_id.clone(),
    })
}

/// Build a Cedar entity uid from an `entityType`/`entityId` pair. `path` is the
/// JSON pointer of the member that carried the pair, and it is the member the
/// refusal names: the pair arrives as one member value, so that is the location
/// a client is pointed at.
pub fn entity_uid(
    entity: &ScopeEntity,
    path: &str,
) -> Result<cedar_policy::EntityUid, InvalidInput> {
    let type_name = cedar_policy::EntityTypeName::from_str(&entity.entity_type).map_err(|e| {
        InvalidInput::new(
            path,
            format!("its type is not a valid Cedar entity type: {e}"),
        )
    })?;
    Ok(cedar_policy::EntityUid::from_type_name_and_id(
        type_name,
        cedar_policy::EntityId::new(&entity.entity_id),
    ))
}

/// Refuse a value that nests past [`MAX_VALUE_DEPTH`], so the conversions below
/// never recurse further than that many frames.
fn check_depth(depth: usize, path: &str) -> Result<(), InvalidInput> {
    if depth > MAX_VALUE_DEPTH {
        return Err(InvalidInput::new(
            path,
            format!("the value nests more than {MAX_VALUE_DEPTH} levels deep"),
        ));
    }
    Ok(())
}

/// Refuse a collection longer than [`MAX_COLLECTION_LEN`].
fn check_len(len: usize, path: &str) -> Result<(), InvalidInput> {
    if len > MAX_COLLECTION_LEN {
        return Err(InvalidInput::new(
            path,
            format!("it carries {len} elements, more than the {MAX_COLLECTION_LEN} allowed"),
        ));
    }
    Ok(())
}

/// One `AttributeValue` as Cedar's JSON value form.
///
/// The scalar arms map straight onto JSON; an entity reference and each extension
/// value (`ipaddr`, `decimal`, `datetime`, `duration`) use Cedar's `__entity` /
/// `__extn` escapes, so Cedar parses them as the value they denote rather than as
/// a record that happens to look like one.
///
/// `depth` is this value's nesting level and `path` is the JSON pointer of the
/// member that carries it, which a refusal names; a set or record recurses one
/// level deeper and is refused at [`MAX_VALUE_DEPTH`].
pub fn attribute_value_to_cedar_json(
    value: &AttributeValue,
    depth: usize,
    path: &str,
) -> Result<serde_json::Value, InvalidInput> {
    check_depth(depth, path)?;
    Ok(match value {
        AttributeValue::Boolean(b) => serde_json::Value::Bool(*b),
        AttributeValue::Long(n) => serde_json::Value::from(*n),
        AttributeValue::String(s) => serde_json::Value::String(s.clone()),
        AttributeValue::Set(items) => {
            check_len(items.len(), path)?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(attribute_value_to_cedar_json(item, depth + 1, path)?);
            }
            serde_json::Value::Array(out)
        }
        AttributeValue::Record(members) => {
            check_len(members.len(), path)?;
            let mut out = serde_json::Map::new();
            for (key, value) in members {
                out.insert(
                    key.clone(),
                    attribute_value_to_cedar_json(value, depth + 1, path)?,
                );
            }
            serde_json::Value::Object(out)
        }
        AttributeValue::entityIdentifier(entity) => entity_ref_json(entity),
        AttributeValue::Ipaddr(v) => extn_json("ip", v),
        AttributeValue::Decimal(v) => extn_json("decimal", v),
        AttributeValue::Datetime(v) => extn_json("datetime", v),
        AttributeValue::Duration(v) => extn_json("duration", v),
    })
}

/// One `CedarTagValue` as Cedar's JSON value form. Same value grammar, and the
/// same depth and length bounds, as [`attribute_value_to_cedar_json`].
pub fn tag_value_to_cedar_json(
    value: &CedarTagValue,
    depth: usize,
    path: &str,
) -> Result<serde_json::Value, InvalidInput> {
    check_depth(depth, path)?;
    Ok(match value {
        CedarTagValue::Boolean(b) => serde_json::Value::Bool(*b),
        CedarTagValue::Long(n) => serde_json::Value::from(*n),
        CedarTagValue::String(s) => serde_json::Value::String(s.clone()),
        CedarTagValue::Set(items) => {
            check_len(items.len(), path)?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(tag_value_to_cedar_json(item, depth + 1, path)?);
            }
            serde_json::Value::Array(out)
        }
        CedarTagValue::Record(members) => {
            check_len(members.len(), path)?;
            let mut out = serde_json::Map::new();
            for (key, value) in members {
                out.insert(key.clone(), tag_value_to_cedar_json(value, depth + 1, path)?);
            }
            serde_json::Value::Object(out)
        }
        CedarTagValue::entityIdentifier(entity) => entity_ref_json(entity),
        CedarTagValue::Ipaddr(v) => extn_json("ip", v),
        CedarTagValue::Decimal(v) => extn_json("decimal", v),
        CedarTagValue::Datetime(v) => extn_json("datetime", v),
        CedarTagValue::Duration(v) => extn_json("duration", v),
    })
}

/// `{"__entity": {"type": T, "id": I}}` — Cedar's escape for an entity reference.
fn entity_ref_json(entity: &EntityIdentifier) -> serde_json::Value {
    serde_json::json!({
        "__entity": { "type": entity.entity_type, "id": entity.entity_id }
    })
}

/// `{"__extn": {"fn": F, "arg": A}}` — Cedar's escape for an extension value.
fn extn_json(function: &str, argument: &str) -> serde_json::Value {
    serde_json::json!({ "__extn": { "fn": function, "arg": argument } })
}

/// Parse a caller-supplied `cedarJson` payload, bounded by size first and by
/// nesting depth after: `serde_json` stops at its own recursion limit, and this
/// pins the depth to the same [`MAX_VALUE_DEPTH`] the modelled arms obey.
fn parse_cedar_json(text: &str, path: &str) -> Result<serde_json::Value, InvalidInput> {
    if text.len() > MAX_CEDAR_JSON_BYTES {
        return Err(InvalidInput::new(
            path,
            format!(
                "it is {} bytes, more than the {MAX_CEDAR_JSON_BYTES} bytes a request may carry",
                text.len()
            ),
        ));
    }
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| InvalidInput::new(path, format!("it is not valid JSON: {e}")))?;
    check_json_depth(&value, 0, path)?;
    Ok(value)
}

/// Depth-check an already-parsed JSON value. Recursion is bounded by
/// [`MAX_VALUE_DEPTH`] because it returns at the bound instead of descending.
fn check_json_depth(
    value: &serde_json::Value,
    depth: usize,
    path: &str,
) -> Result<(), InvalidInput> {
    check_depth(depth, path)?;
    match value {
        serde_json::Value::Array(items) => {
            check_len(items.len(), path)?;
            for item in items {
                check_json_depth(item, depth + 1, path)?;
            }
        }
        serde_json::Value::Object(members) => {
            check_len(members.len(), path)?;
            for item in members.values() {
                check_json_depth(item, depth + 1, path)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// The request's `context` as Cedar JSON. An absent context is the empty record.
pub fn context_to_cedar_json(
    context: Option<&ContextDefinition>,
) -> Result<serde_json::Value, InvalidInput> {
    match context {
        None => Ok(serde_json::json!({})),
        Some(ContextDefinition::contextMap(map)) => {
            let path = paths::CONTEXT_MAP;
            check_len(map.len(), path)?;
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                out.insert(key.clone(), attribute_value_to_cedar_json(value, 1, path)?);
            }
            Ok(serde_json::Value::Object(out))
        }
        Some(ContextDefinition::cedarJson(text)) => {
            parse_cedar_json(text, paths::CONTEXT_CEDAR_JSON)
        }
    }
}

/// Where the `context` a call carried lives in the request, which is the member
/// a refusal about it names.
///
/// `ContextDefinition` is a union of two arms, so a client is pointed at the one
/// it actually sent. An absent context evaluates as the empty record, which is
/// the modelled map arm.
pub fn context_path(context: Option<&ContextDefinition>) -> &'static str {
    match context {
        Some(ContextDefinition::cedarJson(_)) => paths::CONTEXT_CEDAR_JSON,
        Some(ContextDefinition::contextMap(_)) | None => paths::CONTEXT_MAP,
    }
}

/// The request's `entities` as Cedar JSON (an array of entity objects). An absent
/// entity set is the empty store.
///
/// The modelled `entityList` arm is keyed by `identifier`, so a list that names
/// one identifier more than once is accepted and collapsed onto the entity given
/// for that identifier LAST; the earlier ones take no part in the decision. The
/// `cedarJson` arm is a Cedar entity store as written, so what is a valid store
/// there is Cedar's own question, answered by [`build_entities`].
///
/// Refuses a set past `capacity`: an authorization call is evaluated in-process,
/// so the entity store it carries is capped — at the capacity the CALLING
/// operation's own documentation states for its `entities` member
/// ([`MAX_ENTITIES_PER_REQUEST`] for a single-request call,
/// [`MAX_ENTITIES_PER_BATCH`] for a batch one), because that capacity is a
/// property of the operation and not of this conversion.
pub fn entities_to_cedar_json(
    entities: Option<&EntitiesDefinition>,
    capacity: usize,
) -> Result<serde_json::Value, InvalidInput> {
    let too_many = |count: usize| {
        InvalidInput::new(
            paths::ENTITIES,
            format!(
                "it carries {count} entities, more than the {capacity} an \
                 authorization request may carry"
            ),
        )
    };
    let value = match entities {
        None => serde_json::Value::Array(Vec::new()),
        Some(EntitiesDefinition::entityList(items)) => {
            let mut out: Vec<serde_json::Value> =
                Vec::with_capacity(items.len().min(capacity + 1));
            // The list is keyed by `identifier`: repeating one identifier is a
            // SUCCESS path on which only the LAST entity given for it is processed
            // (`smithy_query(query="shape", shape="EntitiesDefinition")`, member
            // `entityList`). So the list is collapsed onto its identifiers here,
            // each identifier keeping the entity the request named for it last —
            // which is also what makes it a Cedar entity store, where one uid
            // carries one entity.
            let mut at: HashMap<(&str, &str), usize> = HashMap::new();
            for (index, item) in items.iter().take(capacity + 1).enumerate() {
                let entity = entity_item_to_cedar_json(item, index)?;
                let identifier = (
                    item.identifier.entity_type.as_str(),
                    item.identifier.entity_id.as_str(),
                );
                match at.get(&identifier) {
                    Some(&seen) => out[seen] = entity,
                    None => {
                        at.insert(identifier, out.len());
                        out.push(entity);
                    }
                }
            }
            if items.len() > capacity {
                return Err(too_many(items.len()));
            }
            serde_json::Value::Array(out)
        }
        Some(EntitiesDefinition::cedarJson(text)) => {
            parse_cedar_json(text, paths::ENTITIES_CEDAR_JSON)?
        }
    };
    let count = value.as_array().map_or(0, Vec::len);
    if count > capacity {
        return Err(too_many(count));
    }
    Ok(value)
}

/// One `EntityItem` as Cedar's entity JSON form.
///
/// `index` is the item's own 0-based position in the `entityList` the request
/// sent, and a refusal about one of the item's members is re-rooted at that
/// position, so the member it names is the one the request really carries and the
/// pointer resolves against the request body — the spelling a refusal of a
/// modelled constraint on the same member already has.
fn entity_item_to_cedar_json(
    item: &EntityItem,
    index: usize,
) -> Result<serde_json::Value, InvalidInput> {
    entity_item_members_to_cedar_json(item)
        .map_err(|invalid| invalid.within_list_element(paths::ENTITY_LIST, index))
}

/// The conversion [`entity_item_to_cedar_json`] re-roots: one entity item's
/// members, each refusal naming the member RELATIVE to the item that carries it,
/// so the checks are written once and the walk over the list supplies the
/// position.
fn entity_item_members_to_cedar_json(
    item: &EntityItem,
) -> Result<serde_json::Value, InvalidInput> {
    let mut entity = serde_json::Map::new();
    entity.insert(
        "uid".to_string(),
        serde_json::json!({
            "type": item.identifier.entity_type,
            "id": item.identifier.entity_id,
        }),
    );

    let mut attrs = serde_json::Map::new();
    if let Some(given) = item.attributes.as_ref() {
        let path = paths::ENTITY_ITEM_ATTRIBUTES;
        check_len(given.len(), path)?;
        for (key, value) in given {
            attrs.insert(key.clone(), attribute_value_to_cedar_json(value, 1, path)?);
        }
    }
    entity.insert("attrs".to_string(), serde_json::Value::Object(attrs));

    let mut parents = Vec::new();
    if let Some(given) = item.parents.as_ref() {
        check_len(given.len(), paths::ENTITY_ITEM_PARENTS)?;
        for parent in given {
            parents.push(serde_json::json!({
                "type": parent.entity_type,
                "id": parent.entity_id,
            }));
        }
    }
    entity.insert("parents".to_string(), serde_json::Value::Array(parents));

    if let Some(given) = item.tags.as_ref() {
        let path = paths::ENTITY_ITEM_TAGS;
        check_len(given.len(), path)?;
        let mut tags = serde_json::Map::new();
        for (key, value) in given {
            tags.insert(key.clone(), tag_value_to_cedar_json(value, 1, path)?);
        }
        entity.insert("tags".to_string(), serde_json::Value::Object(tags));
    }

    Ok(serde_json::Value::Object(entity))
}

/// Whether `entity_type` names an ACTION, rather than a principal or a resource.
///
/// The form is the model's own: `ActionType`'s pattern is `^Action$|^.+::Action$`
/// (`smithy_query(query="shape", shape="ActionIdentifier")`), so an action's entity type
/// is `Action`, alone or qualified by a namespace, and every other entity type names a
/// principal or a resource. The form is read here rather than re-derived at each caller,
/// so one answer decides what an action entity is wherever that matters.
pub fn is_action_entity_type(entity_type: &str) -> bool {
    match entity_type.strip_suffix("Action") {
        Some("") => true,
        Some(qualifier) => qualifier
            .strip_suffix("::")
            .is_some_and(|namespace| !namespace.is_empty()),
        None => false,
    }
}

/// Check the `entities` slice of an authorization request that names its ACTION itself,
/// over the Cedar JSON [`entities_to_cedar_json`] built.
///
/// Such a call's `entities` member carries principal and resource entities only — "You can
/// include only principal and resource entities in this parameter; you can't include
/// actions. You must specify actions in the schema."
/// (`smithy_query(query="doc", shape="IsAuthorizedInput", member="entities")`, worded
/// identically on `BatchIsAuthorizedInput$entities`) — so an action entity
/// ([`is_action_entity_type`]) is refused rather than loaded into the entity store: the
/// actions one evaluation knows, and the hierarchy they sit in, are the ones the policy
/// store's schema declares and not ones a request describes. Both arms of
/// `EntitiesDefinition` are covered, because this reads the Cedar JSON they are both
/// converted into.
///
/// A payload that is no entity LIST at all carries no entity to inspect; it is refused by
/// [`build_entities`], where what counts as a Cedar entity store is decided.
pub fn check_request_entities(entities: &serde_json::Value) -> Result<(), InvalidInput> {
    let Some(supplied) = entities.as_array() else {
        return Ok(());
    };
    for entity in supplied {
        let Some(entity_type) = entity_uid_type(entity) else {
            continue;
        };
        if is_action_entity_type(entity_type) {
            return Err(InvalidInput::new(
                paths::ENTITIES,
                format!(
                    "it carries an entity of type {entity_type}, which is an action entity \
                     type, and an authorization request carries only principal and resource \
                     entities: the actions are the ones the policy store's schema specifies"
                ),
            ));
        }
    }
    Ok(())
}

/// One entity type a policy store knows as a PRINCIPAL type, and where it says so — which
/// is what a refusal naming the type reports.
#[derive(Debug, Clone)]
pub struct PrincipalEntityType {
    /// The entity type itself.
    pub entity_type: String,
    /// Where the store says this type names principals.
    pub origin: PrincipalEntityTypeOrigin,
}

impl PrincipalEntityType {
    /// A type an identity source generates principals (or their groups) as.
    pub fn from_identity_source(entity_type: impl Into<String>) -> Self {
        Self {
            entity_type: entity_type.into(),
            origin: PrincipalEntityTypeOrigin::IdentitySource,
        }
    }
}

/// The two places a policy store says an entity type names principals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrincipalEntityTypeOrigin {
    /// An identity source of the store generates principals of this type, or maps their
    /// groups to it.
    IdentitySource,
    /// The store's schema declares this type as the principal an action applies to.
    Schema,
}

/// Every entity type a policy store's SCHEMA declares as a principal and not as a
/// resource.
///
/// A Cedar schema is where a store declares which entity types name principals: each
/// action it declares names the principal types and the resource types it applies to. A
/// type the schema names on BOTH sides is a legitimate resource of some action, so only
/// the types it knows exclusively as principals are reported — the kinds are read off the
/// PARSED schema rather than searched for in its text.
///
/// `schema_json` is the text the store holds (already accepted by [`parse_schema`]), so
/// text that will not parse is service-side state the caller cannot fix; a store holding
/// no schema declares no principal type.
pub fn schema_principal_entity_types(
    schema_json: Option<&str>,
) -> Result<Vec<PrincipalEntityType>, Refusal> {
    let Some(schema_json) = schema_json else {
        return Ok(Vec::new());
    };
    let parsed = parse_schema(schema_json).map_err(|e| {
        Refusal::ServiceFault(format!(
            "the policy store's schema could not be parsed, so the entities an authorization \
             request carries cannot be checked against it: {e}"
        ))
    })?;
    let resources: BTreeSet<String> = parsed
        .schema
        .resources()
        .map(ToString::to_string)
        .collect();
    let principals: BTreeSet<String> = parsed
        .schema
        .principals()
        .map(ToString::to_string)
        .filter(|entity_type| !resources.contains(entity_type))
        .collect();
    Ok(principals
        .into_iter()
        .map(|entity_type| PrincipalEntityType {
            entity_type,
            origin: PrincipalEntityTypeOrigin::Schema,
        })
        .collect())
}

/// Check the `entities` slice of an authorization request whose principal comes from a
/// TOKEN, over the Cedar JSON [`entities_to_cedar_json`] built.
///
/// Such a call takes its principal from only the token it presented, so the slice carries
/// only resource and action entities:
///
/// * an entity whose type the store knows as a principal type —
///   [`PrincipalEntityType`], whether an identity source generates it or the store's
///   schema declares it — is refused, because the principal of this call is the token's to
///   build and no caller-supplied principal takes part in the decision;
/// * an action entity ([`is_action_entity_type`]) is NAMED and not described, so one
///   supplying attributes, parent entities or tags is refused; an action entity carrying
///   its identifier and type alone is accepted, as a resource entity is.
pub fn check_token_request_entities(
    supplied: &[serde_json::Value],
    principal_entity_types: &[PrincipalEntityType],
) -> Result<(), InvalidInput> {
    for entity in supplied {
        let Some(entity_type) = entity_uid_type(entity) else {
            continue;
        };
        if is_action_entity_type(entity_type) {
            if let Some(surplus) = described_beyond_its_identity(entity) {
                return Err(InvalidInput::new(
                    paths::ENTITIES,
                    format!(
                        "it carries an action entity of type {entity_type} supplying \
                         {surplus}, and an action entity can carry only its identifier and \
                         its entity type"
                    ),
                ));
            }
            continue;
        }
        if let Some(principal) = principal_entity_types
            .iter()
            .find(|principal| principal.entity_type == entity_type)
        {
            return Err(InvalidInput::new(
                paths::ENTITIES,
                match principal.origin {
                    PrincipalEntityTypeOrigin::IdentitySource => format!(
                        "it carries an entity of type {entity_type}, which is an entity type \
                         the identity source that issued the token generates"
                    ),
                    PrincipalEntityTypeOrigin::Schema => format!(
                        "it carries an entity of type {entity_type}, which the policy store's \
                         schema declares as a principal type, and an authorization request \
                         whose principal comes from a token carries only resource and action \
                         entities"
                    ),
                },
            ));
        }
    }
    Ok(())
}

/// The uid TYPE of one entity in a Cedar entity store. Cedar's entity JSON carries a uid
/// either plainly or behind its `__entity` escape, and both name the same entity.
fn entity_uid_type(entity: &serde_json::Value) -> Option<&str> {
    entity
        .pointer("/uid/type")
        .or_else(|| entity.pointer("/uid/__entity/type"))
        .and_then(serde_json::Value::as_str)
}

/// What one entity describes BEYOND its identity, named as the request describes it —
/// `None` when it carries nothing but its uid. An absent member and an empty one are the
/// same thing: [`entity_item_to_cedar_json`] always writes the attribute map and the
/// parent list, empty where the request named none.
fn described_beyond_its_identity(entity: &serde_json::Value) -> Option<&'static str> {
    [
        ("attrs", "attributes"),
        ("parents", "parent entities"),
        ("tags", "tags"),
    ]
    .into_iter()
    .find(|(key, _)| {
        entity.get(key).is_some_and(|value| match value {
            serde_json::Value::Object(members) => !members.is_empty(),
            serde_json::Value::Array(items) => !items.is_empty(),
            serde_json::Value::Null => false,
            _ => true,
        })
    })
    .map(|(_, described)| described)
}
