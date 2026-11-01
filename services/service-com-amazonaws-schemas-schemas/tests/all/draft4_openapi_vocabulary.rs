// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for OpenAPI 3.0's OWN vocabulary in the JSON Schema draft-4
//! document `ExportSchema` renders.
//!
//! RULE: the draft-04 export says in draft-04's terms what the stored OpenAPI 3 document
//! says, so a keyword OpenAPI 3.0 adds to the JSON Schema vocabulary does not travel:
//! `nullable` is re-stated as draft-04's type union (`"type":["<type>","null"]`) and
//! then goes, and the rest — `readOnly`, `writeOnly`, `deprecated`, `discriminator`,
//! `xml`, `example`, `externalDocs` — say nothing a draft-04 validator reads and go with
//! it. The conversion walks plain JSON objects, so it does not tell a schema keyword
//! apart from a member declared under the same name: a `properties` entry named for one
//! of these keywords goes too, which is how the published draft-04 export of a
//! CloudTrail-delivered event schema carries no `readOnly` although the record declares
//! a member of that name.
//!
//! The rows here are the documents the SERVICE wrote, because those are the documents an
//! export serves (a schema a caller authored has no export: see
//! `tests/all/export_reads_the_stored_schema.rs`). The rule is driven over a wider table
//! of documents, byte for byte, by this crate's own unit tests for the conversion in
//! `src/ops/export_schema.rs`.
//!
//! Every case reads the stored document over HTTP and exports it over HTTP, so an export
//! computed from anything but the stored document would have to make the read agree.

use serde_json::Value;
use service_com_amazonaws_schemas_schemas::state::{
    aws_managed_schemas, ManagedSchema, AWS_MANAGED_REGISTRY, JSON_SCHEMA_DRAFT4_URI,
};

use crate::registry_schema_lifecycle::{
    export_uri, schema_uri, Emulator, OPENAPI_ONLY_KEYWORDS,
};

/// The schema language the export renders into.
const DRAFT4: &str = "JSONSchemaDraft4";

/// A bounded, deterministic sample of the catalogue: every 149th entry, so the rows span
/// the publishing sources and the kinds of event the catalogue holds without replaying
/// thousands of HTTP calls — the CloudTrail-delivered entry whose record declares a
/// member named for an OpenAPI-only keyword among them.
fn sampled_catalogue() -> Vec<ManagedSchema> {
    let catalogue = aws_managed_schemas();
    let sampled: Vec<ManagedSchema> = catalogue
        .iter()
        .step_by(149)
        .chain(
            catalogue
                .iter()
                .filter(|entry| entry.name == "aws.events@AWSAPICallviaCloudTrail"),
        )
        .cloned()
        .collect();
    assert!(
        sampled.len() > 5,
        "the sample must span the catalogue, got {} of {}",
        sampled.len(),
        catalogue.len()
    );
    sampled
}

/// Every position `value` carries at any depth that declares a `type` and is marked
/// `nullable`, as `(path, declared type)`.
fn nullable_positions(value: &Value, path: &str, found: &mut Vec<(String, String)>) {
    match value {
        Value::Object(members) => {
            if members.get("nullable") == Some(&Value::Bool(true)) {
                if let Some(declared) = members.get("type").and_then(Value::as_str) {
                    found.push((path.to_string(), declared.to_string()));
                }
            }
            for (name, member) in members {
                nullable_positions(member, &format!("{path}/{name}"), found);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                nullable_positions(item, &format!("{path}/{index}"), found);
            }
        }
        _ => {}
    }
}

/// The value at a `/`-separated path of the exported document, where the path names the
/// same position the source document's one did under `components/schemas`.
fn at_path<'a>(document: &'a Value, path: &str) -> &'a Value {
    let mut cursor = document;
    for step in path.split('/').filter(|step| !step.is_empty()) {
        cursor = match step.parse::<usize>() {
            Ok(index) => &cursor[index],
            Err(_) => &cursor[step],
        };
    }
    cursor
}

/// RULE: no keyword of OpenAPI 3.0's own vocabulary survives the conversion, and what
/// `nullable` said is said by draft-04's type union instead — over the documents the
/// service itself wrote, which are the documents an export serves.
#[tokio::test]
async fn no_openapi_only_keyword_survives_the_draft_4_conversion() {
    let emulator = Emulator::new();
    let sampled = sampled_catalogue();

    // The sample is not vacuous: some stored document carries a member named for one of
    // the keywords, so a conversion that passed them through would fail a row.
    assert!(
        sampled.iter().any(|entry| OPENAPI_ONLY_KEYWORDS
            .iter()
            .any(|keyword| entry.content.contains(&format!("\"{keyword}\"")))),
        "a sampled document must declare something named for an OpenAPI-only keyword"
    );

    let mut widened = 0_usize;
    for entry in &sampled {
        let name = &entry.name;

        // The stored document, read back through the implemented reader: the export is
        // computed from these bytes.
        let described = emulator
            .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, name), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: expected 200 from DescribeSchema, got {} {:?}",
            described.status, described.body
        );
        let stored = described.str_at("Content");
        assert_eq!(
            stored, entry.content,
            "case {name:?}: the reader must serve the published document"
        );

        let exported = emulator
            .call("GET", &export_uri(AWS_MANAGED_REGISTRY, name, DRAFT4), None)
            .await;
        assert_eq!(
            exported.status, 200,
            "case {name:?}: expected 200, got {} {:?}",
            exported.status, exported.body
        );
        let content = exported.str_at("Content");
        for keyword in OPENAPI_ONLY_KEYWORDS {
            assert!(
                !content.contains(&format!("\"{keyword}\"")),
                "case {name:?}: draft-04 defines no {keyword:?}, got {content}"
            );
        }
        let converted: Value = serde_json::from_str(&content)
            .unwrap_or_else(|e| panic!("case {name:?}: the export must be JSON ({e}): {content}"));
        assert_eq!(
            converted["$schema"].as_str(),
            Some(JSON_SCHEMA_DRAFT4_URI),
            "case {name:?}: the export names draft-04, got {converted:?}"
        );

        // What the dropped `nullable` said is said by the type union instead, at every
        // position the stored document marked.
        let source: Value = serde_json::from_str(&stored)
            .unwrap_or_else(|e| panic!("case {name:?}: the source must be JSON ({e})"));
        let mut marked = Vec::new();
        nullable_positions(&source["components"]["schemas"], "", &mut marked);
        for (path, declared) in &marked {
            let rendered = at_path(&converted["definitions"], path);
            assert_eq!(
                rendered["type"],
                serde_json::json!([declared, "null"]),
                "case {name:?}: {path} must export as [{declared:?},\"null\"], got {rendered:?}"
            );
            widened += 1;
        }

        // Exporting it again renders the same bytes: the conversion is a function of the
        // stored document, and reading it changed nothing.
        let again = emulator
            .call("GET", &export_uri(AWS_MANAGED_REGISTRY, name, DRAFT4), None)
            .await;
        assert_eq!(
            again.str_at("Content"),
            content,
            "case {name:?}: two exports must render one document"
        );
    }

    assert!(
        widened > 0,
        "some sampled document must mark a position nullable, or the widening is untested"
    );
}
