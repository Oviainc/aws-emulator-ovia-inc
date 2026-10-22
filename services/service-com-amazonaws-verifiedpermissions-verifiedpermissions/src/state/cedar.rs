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
//! * **Count** — [`MAX_ENTITIES_PER_REQUEST`] caps the entity store,
//!   [`MAX_COLLECTION_LEN`] caps one set/record/attribute map, and
//!   [`crate::state::MAX_POLICIES_PER_STORE`] caps the policies walked.
//! * **Depth** — [`MAX_VALUE_DEPTH`] caps the nesting of any context or entity
//!   value. Every conversion below carries a depth counter and refuses at the
//!   bound INSTEAD of recursing, so no caller-supplied nesting drives this
//!   module's own recursion past a fixed number of frames. Cedar's own parser
//!   grows its stack through `stacker` rather than overflowing the request
//!   thread's.
//! * **Output** — [`MAX_REPORTED_EVALUATION_ERRORS`] caps the `errors` list the
//!   response carries; `determiningPolicies` is bounded by the policy count.

use std::collections::BTreeMap;
use std::str::FromStr;

use crate::ops::types::{
    AttributeValue, CedarTagValue, ContextDefinition, EntitiesDefinition, EntityIdentifier,
    EntityItem,
};

use super::{Policy, ScopeEntity};

/// How many entities one `IsAuthorized` request may carry. The published
/// Verified Permissions quota is *Entities per authorization request* — 100.
pub const MAX_ENTITIES_PER_REQUEST: usize = 100;

/// How many bytes of Cedar text one policy may carry. The published Verified
/// Permissions quota for the size of a policy is 10,000 bytes; the model puts no
/// upper `@length` on `PolicyStatement`, so the boundary validator cannot refuse
/// an over-long one and this is the place that does.
pub const MAX_STATEMENT_BYTES: usize = 10_000;

/// How many bytes of `cedarJson` text one request may carry for its `context` or
/// its `entities`. Both are parsed in the serving process, and the model puts no
/// `@length` on either, so the size needs a bound here. Sized to hold
/// [`MAX_ENTITIES_PER_REQUEST`] entities with room to spare.
pub const MAX_CEDAR_JSON_BYTES: usize = 64 * 1024;

/// How deeply a context or entity value may nest. Each conversion below recurses
/// once per level, so this is the fixed bound on this module's own stack usage.
pub const MAX_VALUE_DEPTH: usize = 32;

/// How many elements one set, record or attribute map may carry.
pub const MAX_COLLECTION_LEN: usize = 1_000;

/// How many evaluation errors the response reports. Cedar can raise one per
/// policy, so the response needs its own output cap.
pub const MAX_REPORTED_EVALUATION_ERRORS: usize = 100;

/// How many bytes of Cedar schema JSON one policy store may hold. The model puts
/// no upper `@length` on `SchemaJson` (`min 1` only), so the boundary validator
/// cannot refuse an over-long one: this is the emulator's bound on the text
/// `PutSchema` stores and that every later validation re-parses. CHOSEN, not an
/// AWS-published quota, and sized well above any hand-written schema.
pub const MAX_SCHEMA_BYTES: usize = 100_000;

/// What the Cedar parser says a static policy's scope is: the pieces
/// `CreatePolicy`, `GetPolicy` and `ListPolicies` report back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticScope {
    /// `Permit` or `Forbid`.
    pub effect: String,
    /// Present only when the scope names a principal entity (`principal ==`,
    /// `principal in`, `principal is T in`); absent for `principal` and
    /// `principal is T`, which name no entity.
    pub principal: Option<ScopeEntity>,
    /// Same rule as `principal`.
    pub resource: Option<ScopeEntity>,
    /// Every action the scope names; empty when the scope is a bare `action`.
    pub actions: Vec<ScopeEntity>,
}

/// Parse one Cedar static policy and read its effect and scope off the parsed
/// form. `Err` carries the parser's diagnostics, for a `ValidationException`.
///
/// Refuses a statement past [`MAX_STATEMENT_BYTES`] BEFORE handing it to the
/// parser, so an over-long payload costs a length check rather than a parse.
pub fn parse_static_policy(statement: &str) -> Result<StaticScope, String> {
    if statement.len() > MAX_STATEMENT_BYTES {
        return Err(format!(
            "definition.static.statement is {} bytes, more than the {MAX_STATEMENT_BYTES} \
             bytes a policy may carry",
            statement.len()
        ));
    }
    let policy = cedar_policy::Policy::parse(None, statement)
        .map_err(|e| format!("the policy statement is not valid Cedar: {e}"))?;

    let effect = match policy.effect() {
        cedar_policy::Effect::Permit => "Permit",
        cedar_policy::Effect::Forbid => "Forbid",
    }
    .to_string();

    let principal = match policy.principal_constraint() {
        cedar_policy::PrincipalConstraint::Any | cedar_policy::PrincipalConstraint::Is(_) => None,
        cedar_policy::PrincipalConstraint::Eq(uid)
        | cedar_policy::PrincipalConstraint::In(uid)
        | cedar_policy::PrincipalConstraint::IsIn(_, uid) => Some(scope_entity(&uid)),
    };
    let resource = match policy.resource_constraint() {
        cedar_policy::ResourceConstraint::Any | cedar_policy::ResourceConstraint::Is(_) => None,
        cedar_policy::ResourceConstraint::Eq(uid)
        | cedar_policy::ResourceConstraint::In(uid)
        | cedar_policy::ResourceConstraint::IsIn(_, uid) => Some(scope_entity(&uid)),
    };
    let actions = match policy.action_constraint() {
        cedar_policy::ActionConstraint::Any => Vec::new(),
        cedar_policy::ActionConstraint::Eq(uid) => vec![scope_entity(&uid)],
        cedar_policy::ActionConstraint::In(uids) => uids.iter().map(scope_entity).collect(),
    };

    Ok(StaticScope {
        effect,
        principal,
        resource,
        actions,
    })
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
/// [`parse_schema`]); `Err` carries the validator's diagnostics, for a
/// `ValidationException`.
pub fn validate_static_policy(schema_json: &str, statement: &str) -> Result<(), String> {
    let parsed = parse_schema(schema_json)?;
    if statement.len() > MAX_STATEMENT_BYTES {
        return Err(format!(
            "definition.static.statement is {} bytes, more than the {MAX_STATEMENT_BYTES} \
             bytes a policy may carry",
            statement.len()
        ));
    }
    let policy = cedar_policy::Policy::parse(None, statement)
        .map_err(|e| format!("the policy statement is not valid Cedar: {e}"))?;
    let mut set = cedar_policy::PolicySet::new();
    set.add(policy)
        .map_err(|e| format!("the policy statement could not be loaded: {e}"))?;

    let result = cedar_policy::Validator::new(parsed.schema)
        .validate(&set, cedar_policy::ValidationMode::Strict);
    if result.validation_passed() {
        return Ok(());
    }
    let mut diagnostics: Vec<String> = result
        .validation_errors()
        .take(MAX_REPORTED_EVALUATION_ERRORS)
        .map(ToString::to_string)
        .collect();
    if diagnostics.is_empty() {
        // Defensive: `validation_passed()` is false only when there is at least
        // one error, but a caller must still get a reason rather than an empty
        // message.
        diagnostics.push("the policy does not validate against the schema".to_string());
    }
    Err(format!(
        "the policy does not validate against the schema in this policy store: {}",
        diagnostics.join("; ")
    ))
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

/// Evaluate `policies` for one authorization request.
///
/// `principal`, `action` and `resource` are the request's triple; `context` and
/// `entities` are Cedar JSON already built and already bounded by
/// [`context_to_cedar_json`] / [`entities_to_cedar_json`]. A policy that will not
/// parse is reported as an evaluation error rather than silently skipped, so a
/// store holding one is never reported as a clean deny.
///
/// `schema_json` is the store's schema, when it holds one. A schema is what gives
/// the request's entity types, action and context their types, so when one is
/// present the entity store and the request are parsed AGAINST it: an entity or a
/// context attribute that does not type-check is refused rather than evaluated,
/// which is the only way a `context` whose attribute has the wrong type can be
/// told apart from a deny.
///
/// The per-request step budget is enforced here, at the point of work: the total
/// Cedar text this call parses is capped at
/// [`crate::state::MAX_POLICY_BYTES_PER_STORE`], and the policy count at
/// [`crate::state::MAX_POLICIES_PER_STORE`].
pub fn authorize(
    policies: &BTreeMap<String, Policy>,
    principal: &ScopeEntity,
    action: &ScopeEntity,
    resource: &ScopeEntity,
    context: serde_json::Value,
    entities: serde_json::Value,
    schema_json: Option<&str>,
) -> Result<Verdict, String> {
    let mut set = cedar_policy::PolicySet::new();
    let mut errors: Vec<String> = Vec::new();
    let mut parsed_bytes = 0usize;
    let mut parsed_count = 0usize;
    for (policy_id, policy) in policies {
        if policy.policy_type != "STATIC" {
            // Template-linked policies are not part of this slice: the templates
            // they instantiate cannot be created yet, so a store never holds one.
            continue;
        }
        parsed_bytes += policy.statement.len();
        parsed_count += 1;
        if parsed_bytes > super::MAX_POLICY_BYTES_PER_STORE
            || parsed_count > super::MAX_POLICIES_PER_STORE
        {
            return Err(format!(
                "policy store {} holds more Cedar text than one authorization request \
                 evaluates ({} bytes / {} policies)",
                policy.policy_store_id,
                super::MAX_POLICY_BYTES_PER_STORE,
                super::MAX_POLICIES_PER_STORE
            ));
        }
        match cedar_policy::Policy::parse(
            Some(cedar_policy::PolicyId::new(policy_id)),
            &policy.statement,
        ) {
            Ok(parsed) => {
                if let Err(e) = set.add(parsed) {
                    errors.push(format!("policy {policy_id} could not be loaded: {e}"));
                }
            }
            Err(e) => errors.push(format!("policy {policy_id} could not be parsed: {e}")),
        }
    }

    let principal_uid = entity_uid(principal, "principal")?;
    let action_uid = entity_uid(action, "action")?;
    let resource_uid = entity_uid(resource, "resource")?;

    let schema = match schema_json {
        Some(text) => Some(parse_schema(text)?.schema),
        None => None,
    };

    let context = cedar_policy::Context::from_json_value(context, None)
        .map_err(|e| format!("context is not a valid Cedar context: {e}"))?;
    let entities = cedar_policy::Entities::from_json_value(entities, schema.as_ref())
        .map_err(|e| format!("entities are not a valid Cedar entity store: {e}"))?;

    let request = cedar_policy::Request::new(
        principal_uid,
        action_uid,
        resource_uid,
        context,
        schema.as_ref(),
    )
    .map_err(|e| format!("the authorization request is not valid: {e}"))?;

    let response = cedar_policy::Authorizer::new().is_authorized(&request, &set, &entities);

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
    errors.extend(response.diagnostics().errors().map(ToString::to_string));
    errors.truncate(MAX_REPORTED_EVALUATION_ERRORS);

    Ok(Verdict {
        decision,
        determining_policies,
        errors,
    })
}

/// Build a Cedar entity uid from an `entityType`/`entityId` pair.
pub fn entity_uid(entity: &ScopeEntity, member: &str) -> Result<cedar_policy::EntityUid, String> {
    let type_name = cedar_policy::EntityTypeName::from_str(&entity.entity_type)
        .map_err(|e| format!("{member}.entityType is not a valid Cedar entity type: {e}"))?;
    Ok(cedar_policy::EntityUid::from_type_name_and_id(
        type_name,
        cedar_policy::EntityId::new(&entity.entity_id),
    ))
}

/// Refuse a value that nests past [`MAX_VALUE_DEPTH`], so the conversions below
/// never recurse further than that many frames.
fn check_depth(depth: usize, member: &str) -> Result<(), String> {
    if depth > MAX_VALUE_DEPTH {
        return Err(format!(
            "{member} nests more than {MAX_VALUE_DEPTH} levels deep"
        ));
    }
    Ok(())
}

/// Refuse a collection longer than [`MAX_COLLECTION_LEN`].
fn check_len(len: usize, member: &str) -> Result<(), String> {
    if len > MAX_COLLECTION_LEN {
        return Err(format!(
            "{member} carries {len} elements, more than the {MAX_COLLECTION_LEN} allowed"
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
/// `depth` is this value's nesting level and `member` names it for the refusal
/// message; a set or record recurses one level deeper and is refused at
/// [`MAX_VALUE_DEPTH`].
pub fn attribute_value_to_cedar_json(
    value: &AttributeValue,
    depth: usize,
    member: &str,
) -> Result<serde_json::Value, String> {
    check_depth(depth, member)?;
    Ok(match value {
        AttributeValue::Boolean(b) => serde_json::Value::Bool(*b),
        AttributeValue::Long(n) => serde_json::Value::from(*n),
        AttributeValue::String(s) => serde_json::Value::String(s.clone()),
        AttributeValue::Set(items) => {
            check_len(items.len(), member)?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(attribute_value_to_cedar_json(item, depth + 1, member)?);
            }
            serde_json::Value::Array(out)
        }
        AttributeValue::Record(members) => {
            check_len(members.len(), member)?;
            let mut out = serde_json::Map::new();
            for (key, value) in members {
                out.insert(
                    key.clone(),
                    attribute_value_to_cedar_json(value, depth + 1, member)?,
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
    member: &str,
) -> Result<serde_json::Value, String> {
    check_depth(depth, member)?;
    Ok(match value {
        CedarTagValue::Boolean(b) => serde_json::Value::Bool(*b),
        CedarTagValue::Long(n) => serde_json::Value::from(*n),
        CedarTagValue::String(s) => serde_json::Value::String(s.clone()),
        CedarTagValue::Set(items) => {
            check_len(items.len(), member)?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(tag_value_to_cedar_json(item, depth + 1, member)?);
            }
            serde_json::Value::Array(out)
        }
        CedarTagValue::Record(members) => {
            check_len(members.len(), member)?;
            let mut out = serde_json::Map::new();
            for (key, value) in members {
                out.insert(
                    key.clone(),
                    tag_value_to_cedar_json(value, depth + 1, member)?,
                );
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
fn parse_cedar_json(text: &str, member: &str) -> Result<serde_json::Value, String> {
    if text.len() > MAX_CEDAR_JSON_BYTES {
        return Err(format!(
            "{member} is {} bytes, more than the {MAX_CEDAR_JSON_BYTES} bytes a request \
             may carry",
            text.len()
        ));
    }
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| format!("{member} is not valid JSON: {e}"))?;
    check_json_depth(&value, 0, member)?;
    Ok(value)
}

/// Depth-check an already-parsed JSON value. Recursion is bounded by
/// [`MAX_VALUE_DEPTH`] because it returns at the bound instead of descending.
fn check_json_depth(value: &serde_json::Value, depth: usize, member: &str) -> Result<(), String> {
    check_depth(depth, member)?;
    match value {
        serde_json::Value::Array(items) => {
            check_len(items.len(), member)?;
            for item in items {
                check_json_depth(item, depth + 1, member)?;
            }
        }
        serde_json::Value::Object(members) => {
            check_len(members.len(), member)?;
            for item in members.values() {
                check_json_depth(item, depth + 1, member)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// The request's `context` as Cedar JSON. An absent context is the empty record.
pub fn context_to_cedar_json(
    context: Option<&ContextDefinition>,
) -> Result<serde_json::Value, String> {
    match context {
        None => Ok(serde_json::json!({})),
        Some(ContextDefinition::contextMap(map)) => {
            check_len(map.len(), "context.contextMap")?;
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                out.insert(
                    key.clone(),
                    attribute_value_to_cedar_json(value, 1, "context.contextMap")?,
                );
            }
            Ok(serde_json::Value::Object(out))
        }
        Some(ContextDefinition::cedarJson(text)) => parse_cedar_json(text, "context.cedarJson"),
    }
}

/// The request's `entities` as Cedar JSON (an array of entity objects). An absent
/// entity set is the empty store.
///
/// Refuses a set past [`MAX_ENTITIES_PER_REQUEST`]: an authorization request is
/// evaluated in-process, so the entity store it carries is capped at the
/// published quota.
pub fn entities_to_cedar_json(
    entities: Option<&EntitiesDefinition>,
) -> Result<serde_json::Value, String> {
    let value = match entities {
        None => serde_json::Value::Array(Vec::new()),
        Some(EntitiesDefinition::entityList(items)) => {
            let mut out = Vec::with_capacity(items.len().min(MAX_ENTITIES_PER_REQUEST + 1));
            for item in items.iter().take(MAX_ENTITIES_PER_REQUEST + 1) {
                out.push(entity_item_to_cedar_json(item)?);
            }
            if items.len() > MAX_ENTITIES_PER_REQUEST {
                return Err(format!(
                    "entities carries {} entities, more than the {MAX_ENTITIES_PER_REQUEST} \
                     an authorization request may carry",
                    items.len()
                ));
            }
            serde_json::Value::Array(out)
        }
        Some(EntitiesDefinition::cedarJson(text)) => parse_cedar_json(text, "entities.cedarJson")?,
    };
    let count = value.as_array().map_or(0, Vec::len);
    if count > MAX_ENTITIES_PER_REQUEST {
        return Err(format!(
            "entities carries {count} entities, more than the {MAX_ENTITIES_PER_REQUEST} \
             an authorization request may carry"
        ));
    }
    Ok(value)
}

/// One `EntityItem` as Cedar's entity JSON form.
fn entity_item_to_cedar_json(item: &EntityItem) -> Result<serde_json::Value, String> {
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
        check_len(given.len(), "entities.entityList.attributes")?;
        for (key, value) in given {
            attrs.insert(
                key.clone(),
                attribute_value_to_cedar_json(value, 1, "entities.entityList.attributes")?,
            );
        }
    }
    entity.insert("attrs".to_string(), serde_json::Value::Object(attrs));

    let mut parents = Vec::new();
    if let Some(given) = item.parents.as_ref() {
        check_len(given.len(), "entities.entityList.parents")?;
        for parent in given {
            parents.push(serde_json::json!({
                "type": parent.entity_type,
                "id": parent.entity_id,
            }));
        }
    }
    entity.insert("parents".to_string(), serde_json::Value::Array(parents));

    if let Some(given) = item.tags.as_ref() {
        check_len(given.len(), "entities.entityList.tags")?;
        let mut tags = serde_json::Map::new();
        for (key, value) in given {
            tags.insert(
                key.clone(),
                tag_value_to_cedar_json(value, 1, "entities.entityList.tags")?,
            );
        }
        entity.insert("tags".to_string(), serde_json::Value::Object(tags));
    }

    Ok(serde_json::Value::Object(entity))
}
