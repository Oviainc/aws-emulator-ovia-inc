// SPDX-License-Identifier: CC0-1.0

//! The JOB CONFIG SCHEMA slice of this service's state: the schema a model-customization
//! job's `JobConfigDocument` has to conform to, the one `DescribeJobSchemaVersion` reports
//! and `CreateJob` admits a document against.
//!
//! `CreateJob`'s own doc: "Before creating a job, call `ListJobSchemaVersions` and
//! `DescribeJobSchemaVersion` to retrieve the configuration schema for your job category.
//! The `JobConfigDocument` must conform to the schema specified by
//! `JobConfigSchemaVersion`." So the schema is a resource of the service rather than of any
//! one operation: it is stated once here, the reader reports it and the create judges a
//! document against it, which is why neither of them can describe a different schema from
//! the other.
//!
//! # What the schema requires
//!
//! The service answers a non-conforming document with the schema layer's own diagnostic,
//! and that diagnostic is the evidence for what the schema declares: its required
//! properties, and that it allows no others
//! (`object instance has properties which are not allowed by the schema`). The recorded
//! refusal names [`REQUIRED_PROPERTIES`]; no recording covers a second shape, so the one
//! schema version the service publishes ([`SCHEMA_VERSION`]) is the schema of every
//! `JobCategory`. The INNER shape of each property is not evidenced by any recording and is
//! therefore not claimed: a document carrying the properties the schema names conforms.

/// The version the service publishes its job configuration schema under, in the
/// `JobSchemaVersion` form (`pattern ^\d+\.\d+\.\d+$`): the first, since this emulator has
/// published no revision of it.
pub const SCHEMA_VERSION: &str = "1.0.0";

/// The properties a job configuration document has to carry, in the order the schema
/// layer's diagnostic lists them (a total order on the names, so one document is refused
/// with one sentence however its own members happened to be ordered).
///
/// Read off the recorded schema diagnostic, which lists exactly these as the document's
/// required properties.
pub const REQUIRED_PROPERTIES: [&str; 5] = [
    "AgentConfig",
    "InputDataConfig",
    "ModelPackageConfig",
    "OutputDataConfig",
    "TrainingConfig",
];

/// The fixed text the refusal of a non-conforming document opens with, before the schema
/// layer's own findings.
const SCHEMA_VALIDATION_FAILED: &str = "Job config document failed schema validation: ";

/// How the schema layer separates the findings of one refusal.
const FINDING_SEPARATOR: &str = "; ";

/// How many properties the schema layer's unknown-property finding lists before it stops.
///
/// A configuration document is a caller-supplied value of up to `JobConfigDocument`'s
/// modelled 262144 bytes, so the properties it can carry are bounded only by its own size:
/// without this, one request could size the refusal it is answered with. The findings
/// themselves are read off the document's TOP LEVEL only — nothing here recurses into a
/// property's value, and the document is read by the maintained JSON parser, which bounds
/// its own nesting — so this is the only growth this check has.
const MAX_REPORTED_PROPERTIES: usize = 10;

/// The JSON Schema document `DescribeJobSchemaVersion` reports for a job category: the
/// object of [`REQUIRED_PROPERTIES`], every one of them required and no other property
/// allowed — which is what the refusal diagnostic says the schema declares.
#[must_use]
pub fn schema_document() -> String {
    let properties: Vec<String> = REQUIRED_PROPERTIES
        .iter()
        .map(|property| format!("\"{property}\":{{\"type\":\"object\"}}"))
        .collect();
    let required: Vec<String> = REQUIRED_PROPERTIES
        .iter()
        .map(|property| format!("\"{property}\""))
        .collect();
    format!(
        "{{\"$schema\":\"http://json-schema.org/draft-07/schema#\",\"type\":\"object\",\
         \"properties\":{{{}}},\"required\":[{}],\"additionalProperties\":false}}",
        properties.join(","),
        required.join(",")
    )
}

/// Why `document` does not conform to the job configuration schema, or `None` when it
/// does.
///
/// The sentence is the schema layer's: the fixed opening, then each finding in the order
/// that layer reports them — the properties the schema does not allow, then the required
/// properties the document is missing. A document that is not a JSON object carries none of
/// the schema's properties at all, so that is the one finding reported for it.
#[must_use]
pub fn document_refusal(document: &str) -> Option<String> {
    let findings = findings(document);
    (!findings.is_empty())
        .then(|| format!("{SCHEMA_VALIDATION_FAILED}{}", findings.join(FINDING_SEPARATOR)))
}

/// The schema layer's findings against `document`, in the order it reports them. Empty for
/// a document that conforms.
fn findings(document: &str) -> Vec<String> {
    let Ok(value) = ::serde_json::from_str::<::serde_json::Value>(document) else {
        return vec![type_finding("string")];
    };
    let Some(members) = value.as_object() else {
        return vec![type_finding(primitive_type(&value))];
    };
    let mut findings = Vec::new();
    let unknown: Vec<&String> = members
        .keys()
        .filter(|name| !REQUIRED_PROPERTIES.contains(&name.as_str()))
        .take(MAX_REPORTED_PROPERTIES)
        .collect();
    if !unknown.is_empty() {
        findings.push(format!(
            "object instance has properties which are not allowed by the schema: {}",
            quoted_list(unknown.into_iter().map(String::as_str))
        ));
    }
    let missing: Vec<&str> = REQUIRED_PROPERTIES
        .iter()
        .copied()
        .filter(|property| !members.contains_key(*property))
        .collect();
    if !missing.is_empty() {
        findings.push(format!(
            "object has missing required properties ({})",
            quoted_list(missing.into_iter())
        ));
    }
    findings
}

/// The finding the schema layer reports for a document of the wrong type: the type it
/// found, against the one the schema allows.
fn type_finding(found: &str) -> String {
    format!(
        "instance type ({found}) does not match any allowed primitive type (allowed: {})",
        quoted_list(::std::iter::once("object"))
    )
}

/// The JSON primitive type `value` is, as the schema layer names one.
fn primitive_type(value: &::serde_json::Value) -> &'static str {
    match value {
        ::serde_json::Value::Null => "null",
        ::serde_json::Value::Bool(_) => "boolean",
        ::serde_json::Value::Number(number) if number.is_f64() => "number",
        ::serde_json::Value::Number(_) => "integer",
        ::serde_json::Value::String(_) => "string",
        ::serde_json::Value::Array(_) => "array",
        ::serde_json::Value::Object(_) => "object",
    }
}

/// A list of names as the schema layer renders one: each quoted, comma-separated, in
/// brackets.
fn quoted_list<'a>(names: impl Iterator<Item = &'a str>) -> String {
    let quoted: Vec<String> = names.map(|name| format!("\"{name}\"")).collect();
    format!("[{}]", quoted.join(","))
}

#[cfg(test)]
mod tests {
    use super::{document_refusal, schema_document, REQUIRED_PROPERTIES};

    /// The schema the reader reports is the one the create judges against: every property
    /// the refusal names is in it, and it parses as JSON.
    #[test]
    fn the_published_schema_names_the_properties_the_refusal_does() {
        let document = schema_document();
        let parsed: ::serde_json::Value =
            ::serde_json::from_str(&document).expect("the published schema is a JSON document");
        for property in REQUIRED_PROPERTIES {
            assert!(
                parsed["properties"].get(property).is_some(),
                "expected the published schema to declare {property:?}, got {document}"
            );
        }
        // A document carrying every property the schema requires conforms, so the schema
        // and the admission check cannot disagree about what a valid document is.
        let conforming = format!(
            "{{{}}}",
            REQUIRED_PROPERTIES
                .iter()
                .map(|property| format!("\"{property}\":{{}}"))
                .collect::<Vec<String>>()
                .join(",")
        );
        let refusal = document_refusal(&conforming);
        assert!(
            refusal.is_none(),
            "expected a document of every required property to conform, got {refusal:?}"
        );
    }
}
