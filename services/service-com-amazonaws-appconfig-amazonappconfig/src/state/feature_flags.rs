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

use serde_json::Value;

/// The only `version` the feature-flags schema accepts. The guide's schema declares
/// `version` as a required property with the single allowed value `"1"`.
pub const FEATURE_FLAGS_SCHEMA_VERSION: &str = "1";

/// The top-level properties the schema declares, and the only ones it allows
/// (`additionalProperties: false`): `flags` and `version` are required, `values`
/// optional.
const TOP_LEVEL_PROPERTIES: &[&str] = &["flags", "values", "version"];

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
    if document.get("version").and_then(Value::as_str) != Some(FEATURE_FLAGS_SCHEMA_VERSION) {
        return false;
    }

    // `flags` is required and is a map from flag key to flag definition.
    let Some(flags) = document.get("flags").and_then(Value::as_object) else {
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
    match document.get("values") {
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
    value.get("enabled").is_none_or(Value::is_boolean)
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
