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

use std::time::SystemTime;

use serde_json::Value;

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
    match flag.get("attributes") {
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
    match attribute.get("constraints") {
        None => true,
        Some(constraints) => {
            let Some(constraints) = constraints.as_object() else {
                return false;
            };
            constraints
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| ATTRIBUTE_CONSTRAINT_TYPES.contains(&kind))
        }
    }
}

/// One flag's values: an object whose `enabled`, when present, is a boolean. The
/// remaining members are the flag's own attribute values, whose types the attribute
/// constraints govern rather than this schema.
fn is_valid_flag_value(value: &Value) -> bool {
    let Some(value) = value.as_object() else {
        return false;
    };
    value.get(ENABLED_KEY).is_none_or(Value::is_boolean)
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
