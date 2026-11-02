// SPDX-License-Identifier: CC0-1.0

//! The `AWS.AppConfig.FeatureFlags` content check.
//!
//! A configuration profile whose `Type` is `AWS.AppConfig.FeatureFlags` does not hold
//! arbitrary bytes: AppConfig runs the content of every
//! `CreateHostedConfigurationVersion` write against that profile through its own
//! feature-flags helper extension, and refuses a document the extension rejects with
//! the modelled `BadRequestException` rather than storing a version a deployment could
//! later serve.
//!
//! The document's shape is the AppConfig User Guide's "Type reference for
//! AWS.AppConfig.FeatureFlags" JSON schema. Only the parts of that schema that the
//! guide states are checked here, each one noted on the check that enforces it; a
//! document this module accepts is one whose structure the guide allows, not one that
//! has been round-tripped through AppConfig. The JSON itself is parsed by `serde_json`,
//! not by anything hand-written here.
//!
//! That envelope is not the whole check. A flag declares, in the same document, the
//! CONSTRAINTS its own values are authored under — an attribute's type, a number's
//! bounds, a string's allowed values or expression, an array's per-item constraints and
//! whether the attribute is required — and a document whose values violate what it
//! itself declares is refused rather than stored
//! ([`violates_declared_constraints`]).
//!
//! The helper does not only check the document: it REWRITES it before AppConfig stores
//! it, stamping every entry of `flags` and of `values` with the `_createdAt`/`_updatedAt`
//! instants the Type reference lists on a flag definition and on a flag's values — which
//! is why a version read back from a feature-flags profile carries metadata the caller
//! never wrote. [`stamped_feature_flags_content`] is that rewrite.
//!
//! The document the helper stores is the AUTHORING document, and it is not what a
//! configuration consumer receives: retrieving a feature-flag configuration returns the
//! flag VALUES rather than the flag definitions, and a disabled flag comes back carrying
//! nothing but its `enabled`. [`resolved_feature_flags_document`] is that projection,
//! computed from the stored document every time it is served, so the bytes
//! `GetHostedConfigurationVersion` reports and the bytes the data plane serves are two
//! views of ONE stored version rather than two stored copies.

use std::time::{Duration, SystemTime};

use serde_json::{Map, Value};

/// The only `version` the feature-flags schema accepts. The guide's schema declares
/// `version` as a required property with the single allowed value `"1"`.
pub const FEATURE_FLAGS_SCHEMA_VERSION: &str = "1";

/// The document's map of flag DEFINITIONS, keyed by flag key: the authoring half of the
/// document, which the schema makes required.
pub const FLAGS_PROPERTY: &str = "flags";

/// The document's map of flag VALUES, keyed by flag key: what each declared flag is
/// configured to, which the schema makes optional.
pub const VALUES_PROPERTY: &str = "values";

/// The document's schema version property; see [`FEATURE_FLAGS_SCHEMA_VERSION`].
pub const VERSION_PROPERTY: &str = "version";

/// One flag's own switch, inside its entry of [`VALUES_PROPERTY`]: a boolean the
/// schema defaults to `false`, so an entry that names no `enabled` configures a flag
/// that is off.
pub const ENABLED_KEY: &str = "enabled";

/// The top-level properties the schema declares, and the only ones it allows
/// (`additionalProperties: false`): `flags` and `version` are required, `values`
/// optional.
const TOP_LEVEL_PROPERTIES: &[&str] = &[FLAGS_PROPERTY, VALUES_PROPERTY, VERSION_PROPERTY];

/// The `type` values an attribute's `constraints` object accepts, from the guide's
/// attribute-constraints schema.
const ATTRIBUTE_CONSTRAINT_TYPES: &[&str] = &["string", "number", "boolean", "array"];

/// The longest flag or attribute key the schema allows: its key pattern is
/// `^[a-z][a-zA-Z\d-_]{0,63}$`, i.e. one leading letter plus at most 63 more
/// characters.
const MAX_KEY_LENGTH: usize = 64;

/// The message AppConfig refuses an invalid feature-flag document with.
///
/// The refusal comes from the helper extension rather than from the operation, so the
/// text is the extension's fixed sentence — AppConfig names the extension it invoked
/// and the member whose data it rejected — and carries nothing from the request.
pub const INVALID_FEATURE_FLAGS_CONTENT_MESSAGE: &str =
    "Error invoking extension AppConfig Feature Flags Helper: Invalid 'Content' data";

/// Whether `content` is a feature-flag document the schema allows.
///
/// Answers `false` for anything the guide's schema rejects: content that is not a JSON
/// object, a missing or non-`"1"` `version`, a missing or non-object `flags`, a key
/// outside the schema's key pattern, a flag or value entry that is not an object, an
/// `enabled` that is not a boolean, an attribute constraint naming a `type` the schema
/// does not list, or a top-level property the schema does not declare.
///
/// This is the document's ENVELOPE. What a flag's values are held to beyond it — the
/// constraints the flag's own attributes declare — is [`violates_declared_constraints`],
/// which is a separate entry point because it is bounded and runs off the async worker.
pub fn is_valid_feature_flags_content(content: &[u8]) -> bool {
    let Ok(document) = serde_json::from_slice::<Value>(content) else {
        return false;
    };
    let Some(document) = document.as_object() else {
        return false;
    };

    // `additionalProperties: false` on the document: a property the schema does not
    // declare is not a flag document AppConfig would have accepted.
    if document
        .keys()
        .any(|property| !TOP_LEVEL_PROPERTIES.contains(&property.as_str()))
    {
        return false;
    }

    // `version` is required and its only allowed value is "1".
    if document.get(VERSION_PROPERTY).and_then(Value::as_str) != Some(FEATURE_FLAGS_SCHEMA_VERSION)
    {
        return false;
    }

    // `flags` is required and is a map from flag key to flag definition.
    let Some(flags) = document.get(FLAGS_PROPERTY).and_then(Value::as_object) else {
        return false;
    };
    for (key, flag) in flags {
        if !is_schema_key(key) {
            return false;
        }
        if !is_valid_flag(flag) {
            return false;
        }
    }

    // `values` is optional, and holds one entry per flag it configures.
    match document.get(VALUES_PROPERTY) {
        None => true,
        Some(values) => {
            let Some(values) = values.as_object() else {
                return false;
            };
            values.iter().all(|(key, value)| {
                // Every entry names a flag the document declares: a value for a flag
                // that is not in `flags` configures nothing.
                is_schema_key(key) && flags.contains_key(key) && is_valid_flag_value(value)
            })
        }
    }
}

/// One flag definition: an object whose `name` and `description`, when present, are
/// strings, and whose `attributes`, when present, is a map of attribute definitions.
fn is_valid_flag(flag: &Value) -> bool {
    let Some(flag) = flag.as_object() else {
        return false;
    };
    for member in ["name", "description"] {
        if flag.get(member).is_some_and(|value| !value.is_string()) {
            return false;
        }
    }
    match flag.get(ATTRIBUTES_PROPERTY) {
        None => true,
        Some(attributes) => {
            let Some(attributes) = attributes.as_object() else {
                return false;
            };
            attributes
                .iter()
                .all(|(key, attribute)| is_schema_key(key) && is_valid_attribute(attribute))
        }
    }
}

/// One attribute definition: an object whose `constraints`, when present, is an object
/// naming a `type` the schema lists.
fn is_valid_attribute(attribute: &Value) -> bool {
    let Some(attribute) = attribute.as_object() else {
        return false;
    };
    match attribute.get(CONSTRAINTS_PROPERTY) {
        None => true,
        Some(constraints) => {
            let Some(constraints) = constraints.as_object() else {
                return false;
            };
            constraints
                .get(TYPE_KEYWORD)
                .and_then(Value::as_str)
                .is_some_and(|kind| ATTRIBUTE_CONSTRAINT_TYPES.contains(&kind))
        }
    }
}

/// One flag's values: an object whose `enabled`, when present, is a boolean. The
/// remaining members are the flag's own attribute values, whose types the attribute
/// constraints govern rather than this schema — see [`violates_declared_constraints`].
fn is_valid_flag_value(value: &Value) -> bool {
    let Some(value) = value.as_object() else {
        return false;
    };
    value.get(ENABLED_KEY).is_none_or(Value::is_boolean)
}

// =============================================================================
// The helper's authoring check: a flag value against its own flag's constraints
// =============================================================================

/// A flag definition's map of attribute definitions, keyed by attribute key.
const ATTRIBUTES_PROPERTY: &str = "attributes";

/// One attribute definition's bound on what may be authored for that attribute.
const CONSTRAINTS_PROPERTY: &str = "constraints";

/// The constraint keyword naming the JSON type the attribute's value has to be. Spelled
/// as JSON Schema spells it, which is what lets the constraint be evaluated as one.
const TYPE_KEYWORD: &str = "type";

/// The constraint keyword stating that the flag's values have to CARRY the attribute.
const REQUIRED_KEYWORD: &str = "required";

/// The constraint keyword listing the values the attribute may hold.
const ENUM_KEYWORD: &str = "enum";

/// The constraint keyword an attribute's value has to match, which JSON Schema spells
/// the same way. It is the one constraint whose application COMPILES a program rather
/// than comparing a value, which is why [`collected_checks`] counts and measures every
/// one a document declares before any of them is handed to the evaluator.
const PATTERN_KEYWORD: &str = "pattern";

/// The constraint keywords bounding a number attribute's value, inclusive, each spelled
/// as JSON Schema spells the same assertion.
const VALUE_BOUND_KEYWORDS: &[&str] = &["minimum", "maximum"];

/// The constraint keyword holding an array attribute's per-ITEM constraints, which JSON
/// Schema states as [`ITEMS_KEYWORD`].
const ELEMENTS_KEYWORD: &str = "elements";

/// The schema keyword a derived per-item constraint is stated under.
const ITEMS_KEYWORD: &str = "items";

/// The schema keyword the per-attribute constraints of one entry are stated under.
const PROPERTIES_KEYWORD: &str = "properties";

/// How many levels of nested constraints are derived: an attribute's own, plus the
/// per-item constraints an array attribute's [`ELEMENTS_KEYWORD`] carries. The guide
/// declares no constraints inside an element's constraints, so the derivation stops
/// there and cannot follow a document's nesting.
const ELEMENT_NESTING: usize = 1;

/// What a flag's `values` entry carries besides attribute values: the flag's own switch
/// and the two stamps the helper writes ([`CREATED_AT_KEY`], [`UPDATED_AT_KEY`]).
const NON_ATTRIBUTE_VALUE_KEYS: &[&str] = &[ENABLED_KEY, CREATED_AT_KEY, UPDATED_AT_KEY];

/// The dialect the derived constraint schema is read under: the guide's Type reference
/// declares `draft-07`, and every keyword derived from a constraint means there what it
/// means in the constraint.
const CONSTRAINT_SCHEMA_DRAFT: ::boon::Draft = ::boon::Draft::V7;

/// The URL the derived schema is compiled as a resource under. It is a name, not an
/// address: the schema is built here and names no other resource, and nothing is fetched
/// from it (see the loader [`satisfies`] registers).
const ATTRIBUTE_CONSTRAINTS_SCHEMA_URL: &str =
    "https://appconfig.amazonaws.com/featureflags/attributeconstraints";

/// How many constrained attributes one document is checked for. The guide's own schema
/// caps a flag document at 100 flags of 25 attributes each, so every document it allows
/// is inside this; one that declares more constraint than this is not CHECKED, and
/// therefore not stored.
const MAX_CONSTRAINED_ATTRIBUTES: usize = 2_500;

/// How many caller-declared expressions one document is checked for.
///
/// An expression is the one constraint that is COMPILED rather than compared: its
/// automaton can be vastly larger than the text it was written as, which the evaluator's
/// engine bounds per expression by refusing to build a program past its own ceiling. What
/// that leaves unbounded is HOW MANY such programs one request builds, so the count is
/// capped here, far below [`MAX_CONSTRAINED_ATTRIBUTES`]: the cost of the check is the
/// PRODUCT of this count and that per-expression ceiling, not of either alone. A document
/// declaring more is not CHECKED, and therefore not stored. `pub` because it is a bound a
/// caller can reach, so a test spans it.
pub const MAX_PATTERN_CONSTRAINTS: usize = 8;

/// The longest expression that is compiled, in characters.
const MAX_PATTERN_CHARS: usize = 256;

/// How long the whole authoring check of one document may take.
///
/// It runs on a blocking thread, so this is the longest a gated write waits on it. The
/// three bounds above are what bound the WORK; this bounds the WAIT, the same way
/// [`super::validators`] bounds a configuration profile's own schema check.
/// EMULATOR-chosen: AppConfig publishes no duration for it.
const CONSTRAINT_CHECK_BUDGET: Duration = Duration::from_secs(5);

/// Whether any value in `content` violates a constraint its own flag declares.
///
/// A feature flag's attribute constraints are what the AUTHORING of that flag is gated
/// on: a value outside a number attribute's range, outside a string attribute's list of
/// allowed values, failing the expression it declares, or of the wrong type for the
/// attribute it is written under is not a flag value AppConfig stores, and a `required`
/// attribute the flag's values do not carry leaves the flag unconfigured. The declaration
/// and the value live in the SAME document, so this is decided from that document alone.
///
/// The guide's constraint keywords are JSON Schema assertions under their own names —
/// `type`, `minimum`/`maximum`, `enum`, `pattern`, `required`, and an array's `elements` —
/// so the check is the schema those keywords already state, applied by the one evaluator
/// this crate decides schema compliance with ([`super::validators`]), rather than a second
/// comparison hand-written per keyword.
///
/// Answers `true` for a document this emulator does not CHECK as well as for one it finds
/// violating: a document declaring more constrained attributes than
/// [`MAX_CONSTRAINED_ATTRIBUTES`], more expressions than [`MAX_PATTERN_CONSTRAINTS`] or an
/// expression longer than [`MAX_PATTERN_CHARS`]; one carrying a constraint the evaluator
/// cannot compile at all, an expression that is no regular expression among them; and one
/// whose check does not answer inside [`CONSTRAINT_CHECK_BUDGET`]. Nothing about such a
/// document has been shown to satisfy what it declares, so the write it gates is refused
/// rather than stored unchecked.
///
/// Every bound is on the collecting walk, which happens before anything is compiled, and
/// the evaluation itself then runs on a blocking thread: the async worker that serves the
/// request is never occupied by it, and no store lock may be held across the await.
pub async fn violates_declared_constraints(content: &[u8]) -> bool {
    let Ok(document) = serde_json::from_slice::<Value>(content) else {
        // Not a document at all, so it declares nothing — and
        // [`is_valid_feature_flags_content`] has already refused it.
        return false;
    };
    let Some(checks) = collected_checks(&document) else {
        ::tracing::warn!(
            "a feature-flag document declares more constraint than this emulator checks, so \
             its values were not shown to satisfy it"
        );
        return true;
    };
    if checks.is_empty() {
        return false;
    }

    // The compile and the apply are synchronous and have no yield point, so they run on a
    // blocking thread: the request waits on them for at most the budget.
    let evaluated = ::tokio::time::timeout(
        CONSTRAINT_CHECK_BUDGET,
        ::tokio::task::spawn_blocking(move || checks.iter().any(FlagCheck::is_violated)),
    )
    .await;
    match evaluated {
        Ok(Ok(violated)) => violated,
        // The check did not finish inside the budget, or the thread it ran on did not come
        // back with an answer: the values were NOT shown to satisfy their constraints.
        Ok(Err(_)) | Err(_) => {
            ::tracing::warn!(
                "a feature-flag document's authoring check did not answer inside the budget \
                 this emulator allows"
            );
            true
        }
    }
}

/// What one flag's authored values are held to: the schema its attribute constraints
/// state, and the values themselves.
///
/// Owned, because the evaluation runs on a blocking thread and so borrows nothing from the
/// request.
#[derive(Debug)]
struct FlagCheck {
    /// The JSON Schema the flag's constraints state about its `values` entry.
    schema: Value,
    /// The values authored for the flag, without its switch and the helper's stamps.
    authored: Value,
}

impl FlagCheck {
    /// Whether the authored values break what the flag declares.
    fn is_violated(&self) -> bool {
        !satisfies(&self.schema, &self.authored)
    }
}

/// One [`FlagCheck`] per flag whose declared constraints bound something its `values`
/// entry carries, or `None` when the document declares more than this emulator checks.
///
/// This walk is where every bound is applied: it visits each declared attribute once and
/// stops as soon as a count passes its cap, so nothing is compiled for a document that is
/// beyond them.
fn collected_checks(document: &Value) -> Option<Vec<FlagCheck>> {
    let flags = document.get(FLAGS_PROPERTY).and_then(Value::as_object);
    let values = document.get(VALUES_PROPERTY).and_then(Value::as_object);
    // A document that declares no flags, or configures none, authors no value to check.
    let (Some(flags), Some(values)) = (flags, values) else {
        return Some(Vec::new());
    };

    let mut checks: Vec<FlagCheck> = Vec::new();
    let mut constrained = 0usize;
    let mut expressions = 0usize;
    // One entry per flag, so one check per flag: `values` is a map keyed by flag key.
    for (key, entry) in values {
        // A value for a flag the document does not declare, or an entry that is not an
        // object, is already refused by [`is_valid_feature_flags_content`].
        let (Some(flag), Some(entry)) = (flags.get(key), entry.as_object()) else {
            continue;
        };
        // A flag that declares no attributes constrains none of its values.
        let Some(attributes) = flag.get(ATTRIBUTES_PROPERTY).and_then(Value::as_object) else {
            continue;
        };

        let mut properties = Map::new();
        let mut required: Vec<Value> = Vec::new();
        for (attribute, definition) in attributes {
            let Some(constraints) = definition
                .get(CONSTRAINTS_PROPERTY)
                .and_then(Value::as_object)
            else {
                // An attribute with no constraints bounds nothing, so a value written
                // under it is whatever the caller authored.
                continue;
            };
            constrained += 1;
            if constrained > MAX_CONSTRAINED_ATTRIBUTES {
                return None;
            }
            // Each expression the attribute declares \u2014 its own, and the one its elements
            // declare \u2014 becomes a compiled program when the schema is applied, so each is
            // counted and measured HERE, before anything is handed to the evaluator.
            for expression in declared_expressions(constraints) {
                expressions += 1;
                if expressions > MAX_PATTERN_CONSTRAINTS
                    || expression.chars().count() > MAX_PATTERN_CHARS
                {
                    return None;
                }
            }
            // `required` is an assertion about the ENTRY — that it carries the attribute
            // at all — so it is collected for the entry's own schema, not the value's.
            if constraints.get(REQUIRED_KEYWORD).and_then(Value::as_bool) == Some(true) {
                required.push(Value::from(attribute.clone()));
            }
            properties.insert(
                attribute.clone(),
                constraint_schema(constraints, ELEMENT_NESTING),
            );
        }
        if properties.is_empty() && required.is_empty() {
            // Nothing the flag declares bounds what was authored for it.
            continue;
        }

        // The entry is an object of attribute values, each held to its own attribute's
        // constraints, and it has to carry the attributes declared `required`.
        let mut schema = Map::new();
        schema.insert(TYPE_KEYWORD.to_string(), Value::from("object"));
        schema.insert(PROPERTIES_KEYWORD.to_string(), Value::Object(properties));
        if !required.is_empty() {
            schema.insert(REQUIRED_KEYWORD.to_string(), Value::Array(required));
        }
        // What was AUTHORED for the flag: the entry without its switch and without the
        // helper's own stamps, neither of which is an attribute value.
        let authored: Map<String, Value> = entry
            .iter()
            .filter(|(key, _)| !NON_ATTRIBUTE_VALUE_KEYS.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        checks.push(FlagCheck {
            schema: Value::Object(schema),
            authored: Value::Object(authored),
        });
    }
    Some(checks)
}

/// The JSON Schema one attribute's `constraints` object states about that attribute's
/// value, keyword for keyword.
///
/// `depth` is how many further levels of nested constraints are derived; see
/// [`ELEMENT_NESTING`].
fn constraint_schema(constraints: &Map<String, Value>, depth: usize) -> Value {
    let mut schema = Map::new();
    match constraints.get(ENUM_KEYWORD).and_then(Value::as_array) {
        // An `enum` lists the values the attribute may hold, which is the narrower
        // statement of the same constraint, so it REPLACES the type assertion rather than
        // joining it: the guide allows a string attribute's enum to list integers, and a
        // `type` beside it would refuse an entry the list allows.
        Some(allowed) => {
            schema.insert(ENUM_KEYWORD.to_string(), Value::Array(allowed.clone()));
        }
        None => {
            if let Some(declared) = constraints.get(TYPE_KEYWORD).and_then(Value::as_str) {
                schema.insert(TYPE_KEYWORD.to_string(), Value::from(declared));
            }
        }
    }
    for bound in VALUE_BOUND_KEYWORDS {
        if let Some(limit) = constraints.get(*bound).filter(|limit| limit.is_number()) {
            schema.insert((*bound).to_string(), limit.clone());
        }
    }
    if let Some(expression) = constraints
        .get(PATTERN_KEYWORD)
        .filter(|expression| expression.is_string())
    {
        schema.insert(PATTERN_KEYWORD.to_string(), expression.clone());
    }
    if depth > 0 {
        if let Some(elements) = constraints.get(ELEMENTS_KEYWORD).and_then(Value::as_object) {
            schema.insert(
                ITEMS_KEYWORD.to_string(),
                constraint_schema(elements, depth - 1),
            );
        }
    }
    Value::Object(schema)
}

/// The expressions one attribute's `constraints` object declares: its own, and the one its
/// [`ELEMENTS_KEYWORD`] declares about each item of an array value.
///
/// These are what [`collected_checks`] counts and measures, because an expression is the
/// one constraint whose application COMPILES a program rather than comparing a value.
fn declared_expressions(constraints: &Map<String, Value>) -> Vec<&str> {
    let nested = constraints
        .get(ELEMENTS_KEYWORD)
        .and_then(Value::as_object)
        .and_then(|elements| elements.get(PATTERN_KEYWORD));
    [constraints.get(PATTERN_KEYWORD), nested]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

/// Whether `instance` satisfies `schema`.
///
/// Both are derived here from a document already capped where it is stored
/// (`super::MAX_HOSTED_CONFIGURATION_BYTES`), and the schema is one property per declared
/// attribute and carries no reference and no combinator keyword — so no subschema is
/// applied to an instance node more than once and the whole application costs no more than
/// a walk of those two documents. The count of properties it can hold is itself capped by
/// [`MAX_CONSTRAINED_ATTRIBUTES`], and the expressions it can hold by
/// [`MAX_PATTERN_CONSTRAINTS`] and [`MAX_PATTERN_CHARS`], all applied before this runs.
/// The evaluator matches an expression in time linear in the value — there is no
/// backtracking for one to drive exponential — and refuses to build a program past its
/// own ceiling, which answers `false` here rather than passing.
///
/// `false` when the schema does not compile at all: a constraint whose declaration this
/// emulator cannot read as an assertion has not been satisfied, and the authoring check is
/// what a flag document is stored on, so an unusable declaration refuses the write rather
/// than passing it.
fn satisfies(schema: &Value, instance: &Value) -> bool {
    let mut compiler = ::boon::Compiler::new();
    // The schema is built here, keyword by keyword, so it names no other resource; the
    // loader that resolves nothing is registered all the same, in place of the one the
    // compiler comes with, so compiling it reaches neither the filesystem nor a socket.
    compiler.use_loader(Box::new(super::validators::NoSchemaResources));
    compiler.set_default_draft(CONSTRAINT_SCHEMA_DRAFT);
    if compiler
        .add_resource(ATTRIBUTE_CONSTRAINTS_SCHEMA_URL, schema.clone())
        .is_err()
    {
        return false;
    }
    let mut schemas = ::boon::Schemas::new();
    let Ok(compiled) = compiler.compile(ATTRIBUTE_CONSTRAINTS_SCHEMA_URL, &mut schemas) else {
        return false;
    };
    schemas.validate(instance, compiled).is_ok()
}

// =============================================================================
// The helper's rewrite: `_createdAt` / `_updatedAt` on every stamped entry
// =============================================================================

/// The metadata key naming when an entry first appeared in the profile.
pub const CREATED_AT_KEY: &str = "_createdAt";

/// The metadata key naming when an entry's content last changed.
pub const UPDATED_AT_KEY: &str = "_updatedAt";

/// The document's maps whose every entry the helper stamps: one entry per flag in
/// `flags` (the flag's definition) and one per flag in `values` (that flag's values).
/// Nothing else in the document is stamped — `version` is a scalar, and an attribute
/// lives inside a flag definition rather than as an entry of a stamped map.
pub const STAMPED_MAPS: &[&str] = &[FLAGS_PROPERTY, VALUES_PROPERTY];

/// `content` as the feature-flags helper would store it: the same document with
/// `_createdAt` and `_updatedAt` on every entry of each map in [`STAMPED_MAPS`].
///
/// `previous` is the content of the profile's latest existing hosted version, when it has
/// one, because the two instants are properties of the ENTRY's history in the profile and
/// not of this call:
///
/// * `_createdAt` is when the entry's key first appeared — carried over from `previous`
///   when that version already held the key, otherwise `now`.
/// * `_updatedAt` is when the entry's content last changed — `now` when the entry's body
///   differs from the one `previous` held (or the key is new), otherwise the instant
///   carried over from `previous`.
///
/// Metadata the CALLER supplied is dropped first: the stamps are the helper's, so a
/// request cannot dictate them, and a document re-submitted unchanged keeps the instants
/// the profile already recorded rather than whatever a client echoed back.
///
/// `None` when `content` is not a JSON object or the instant is not representable, which
/// is the caller's signal to refuse rather than store an unstamped document;
/// [`is_valid_feature_flags_content`] has already rejected the former by the time this
/// runs on a real request.
pub fn stamped_feature_flags_content(
    content: &[u8],
    previous: Option<&[u8]>,
    now: SystemTime,
) -> Option<Vec<u8>> {
    let mut document = serde_json::from_slice::<Value>(content).ok()?;
    let previous = previous.and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok());
    let now = format_instant(now)?;
    let document_object = document.as_object_mut()?;

    for map_name in STAMPED_MAPS {
        let Some(entries) = document_object
            .get_mut(*map_name)
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        for (key, entry) in entries.iter_mut() {
            let Some(entry) = entry.as_object_mut() else {
                continue;
            };
            // What the previous version recorded for this same key, if anything.
            let prior = previous
                .as_ref()
                .and_then(|document| document.get(*map_name))
                .and_then(|map| map.get(key.as_str()))
                .and_then(Value::as_object)
                .cloned();

            entry.remove(CREATED_AT_KEY);
            entry.remove(UPDATED_AT_KEY);

            let prior_created = prior
                .as_ref()
                .and_then(|prior| prior.get(CREATED_AT_KEY))
                .and_then(Value::as_str);
            let prior_updated = prior
                .as_ref()
                .and_then(|prior| prior.get(UPDATED_AT_KEY))
                .and_then(Value::as_str);
            // The previous body, compared with the metadata keys stripped from both
            // sides, so carrying the stamps forward is not itself read as a change.
            let unchanged = prior.as_ref().is_some_and(|prior| {
                let mut body = prior.clone();
                body.remove(CREATED_AT_KEY);
                body.remove(UPDATED_AT_KEY);
                body == *entry
            });

            let created = prior_created.unwrap_or(now.as_str()).to_string();
            let updated = if unchanged {
                prior_updated.unwrap_or(now.as_str()).to_string()
            } else {
                now.clone()
            };
            entry.insert(CREATED_AT_KEY.to_string(), Value::from(created));
            entry.insert(UPDATED_AT_KEY.to_string(), Value::from(updated));
        }
    }

    serde_json::to_vec(&document).ok()
}

// =============================================================================
// The helper's projection: the flag document a configuration consumer receives
// =============================================================================

/// The flag document AppConfig serves for `content`, the stored authoring document.
///
/// Retrieving a feature-flag configuration does not hand back the authoring document:
/// what a consumer receives is the flag VALUES, keyed by flag key at the TOP level, so a
/// feature-flag client reads `document[flagKey].enabled` and the flag's attribute values
/// straight off it. Everything that belongs to authoring rather than to the served
/// configuration is therefore absent: the [`FLAGS_PROPERTY`] definitions (the flag's
/// name, description and attribute constraints), the [`VERSION_PROPERTY`] of the
/// authoring schema, and the helper's own [`CREATED_AT_KEY`]/[`UPDATED_AT_KEY`]
/// bookkeeping. A flag that is OFF is served as nothing but its `enabled`: its attribute
/// values configure behaviour a disabled flag does not have, so resolution drops them.
///
/// Derived from the stored bytes on every serve rather than stored beside them, so the
/// document a consumer receives follows the version currently deployed and the authoring
/// document a `GetHostedConfigurationVersion` reports is left exactly as it is.
///
/// `None` when `content` is not a JSON object, which is the caller's signal to serve the
/// stored bytes unchanged; [`is_valid_feature_flags_content`] has already refused such a
/// document at write time, so a stored version never reaches this.
pub fn resolved_feature_flags_document(content: &[u8]) -> Option<Vec<u8>> {
    let document = serde_json::from_slice::<Value>(content).ok()?;
    let document = document.as_object()?;

    let mut resolved = serde_json::Map::new();
    // The values half is what is served; it is the schema's optional property, so a
    // document that configures no flag serves an empty document rather than failing.
    let values = document
        .get(VALUES_PROPERTY)
        .and_then(Value::as_object)
        .into_iter()
        .flatten();
    for (key, entry) in values {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        // `enabled` is a boolean the schema defaults to `false`, so an entry that names
        // none configures a flag that is off.
        let enabled = entry
            .get(ENABLED_KEY)
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut served = serde_json::Map::new();
        served.insert(ENABLED_KEY.to_string(), Value::Bool(enabled));
        if enabled {
            // The flag's own attribute values, whatever they are and however deeply
            // nested: everything in the entry except the switch itself and the helper's
            // stamps is configuration the flag carries.
            for (attribute, value) in entry {
                if [ENABLED_KEY, CREATED_AT_KEY, UPDATED_AT_KEY].contains(&attribute.as_str()) {
                    continue;
                }
                served.insert(attribute.clone(), value.clone());
            }
        }
        resolved.insert(key.clone(), Value::Object(served));
    }

    serde_json::to_vec(&Value::Object(resolved)).ok()
}

/// `at` as the instant the stamps carry: an RFC 3339 UTC instant with a fixed
/// MILLISECOND fraction, the resolution AppConfig's feature-flags helper writes these
/// stamps at (the recorded documents carry three fractional digits, e.g.
/// `2026-09-07T05:48:55.763Z`).
///
/// The calendar part comes from the same `aws-smithy-types` date type the wire serializer
/// uses rather than from a second date implementation here; only the fraction is written
/// out to a fixed width, because that type trims a trailing zero and a stamp's width must
/// not depend on the value of its milliseconds.
fn format_instant(at: SystemTime) -> Option<String> {
    let millis = i64::try_from(at.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_millis()).ok()?;
    let whole = ::aws_smithy_types::DateTime::from_secs(millis.div_euclid(1_000))
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()?;
    let civil = whole.strip_suffix('Z')?;
    Some(format!("{civil}.{:03}Z", millis.rem_euclid(1_000)))
}

/// Whether `key` matches the schema's flag/attribute key pattern,
/// `^[a-z][a-zA-Z\d-_]{0,63}$`: a leading lowercase letter followed by at most 63 more
/// ASCII letters, digits, hyphens or underscores.
fn is_schema_key(key: &str) -> bool {
    let mut characters = key.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && key.len() <= MAX_KEY_LENGTH
        && characters.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}
