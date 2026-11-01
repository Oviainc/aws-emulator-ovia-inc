// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHERE `ExportSchema` roots the JSON Schema draft-4
//! document it renders.
//!
//! RULE: a draft-04 document IS a schema — its keywords constrain the instance, while
//! `definitions` is a location for re-usable schemas that "plays no role in validation
//! per se" (JSON Schema draft-04 validation, section 5.5.7) — so the translation of an
//! OpenAPI 3 document is rooted at the component that document roots at: the event
//! envelope's keywords become the exported document's own, the components left over
//! become its `definitions`, and every component reference follows what the conversion
//! did with the component it names (`#/definitions/<name>`, or the root reference `#`
//! for the promoted one). A document that defines no envelope component names no root
//! among its co-equal components, so none is promoted and all of them stay definitions.
//!
//! Every case drives the real wire path through the harness in
//! `registry_schema_lifecycle.rs`: each document is exported over HTTP and compared with
//! the document the store holds, so an export computed from anything but the stored
//! document would have to make that read agree. The documents here are the ones the
//! SERVICE wrote, because those are the documents an export serves (a schema a caller
//! authored has no export: see `tests/all/export_reads_the_stored_schema.rs`), and every
//! one of them names no envelope component. The other half of the rule — a document that
//! DOES root at a component — is driven over a table of such documents, byte for byte, by
//! this crate's own unit tests for the conversion in `src/ops/export_schema.rs`.

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::state::{
    aws_managed_schemas, ManagedSchema, AWS_MANAGED_REGISTRY, EVENT_ENVELOPE_COMPONENT,
    JSON_SCHEMA_DRAFT4_URI, OPENAPI_COMPONENT_REF,
};

use crate::registry_schema_lifecycle::{export_uri, Emulator};

/// The schema language the export renders into.
const DRAFT4: &str = "JSONSchemaDraft4";

/// The keywords a draft-04 schema can constrain an object instance with — a root
/// carrying none of them admits every instance, which is no translation of the document
/// it was rendered from.
///
/// Source: JSON Schema draft-04 validation, sections 5.4 (`properties`, `required`),
/// 5.5 (`type`, `enum`, `allOf`, `anyOf`, `oneOf`) and 7 (`$ref`).
const DRAFT4_CONSTRAINING_KEYWORDS: &[&str] = &[
    "$ref",
    "allOf",
    "anyOf",
    "enum",
    "oneOf",
    "properties",
    "required",
    "type",
];

/// The keywords of `schema` that constrain an instance.
fn constraining_keywords(schema: &Value) -> Vec<&'static str> {
    DRAFT4_CONSTRAINING_KEYWORDS
        .iter()
        .filter(|keyword| !schema[**keyword].is_null())
        .copied()
        .collect()
}

/// RULE (over the documents AWS publishes): a published event document declares no
/// envelope component, so it names no root among its co-equal components — its export
/// promotes none of them, carries draft-04 and the document's title at its root, and
/// renders every component the document defines as a definition.
#[tokio::test]
async fn a_published_event_schema_exports_with_its_components_as_definitions() {
    let emulator = Emulator::new();

    // A bounded, deterministic sample of the catalogue: every 149th entry, so the rows
    // span the publishing sources and the kinds of event the catalogue holds without
    // replaying thousands of HTTP calls — the entry the issue cited among them.
    let catalogue = aws_managed_schemas();
    let sampled: Vec<&ManagedSchema> = catalogue
        .iter()
        .step_by(149)
        .chain(
            catalogue
                .iter()
                .filter(|entry| entry.name == "aws.acm@ACMCertificateAvailable"),
        )
        .collect();
    assert!(
        sampled.len() > 5,
        "the sample must span the catalogue, got {} of {}",
        sampled.len(),
        catalogue.len()
    );
    assert!(
        sampled
            .iter()
            .any(|entry| entry.name == "aws.acm@ACMCertificateAvailable"),
        "the cited entry must be among the rows, got {:?}",
        sampled.iter().map(|entry| &entry.name).collect::<Vec<_>>()
    );

    for entry in sampled {
        let name = &entry.name;
        let exported = emulator
            .call("GET", &export_uri(AWS_MANAGED_REGISTRY, name, DRAFT4), None)
            .await;
        assert_eq!(
            exported.status, 200,
            "case {name:?}: expected 200, got {} {:?}",
            exported.status, exported.body
        );
        let converted: Value = serde_json::from_str(&exported.str_at("Content"))
            .unwrap_or_else(|e| panic!("case {name:?}: the export must be JSON: {e}"));
        let source: Value = serde_json::from_str(&entry.content)
            .unwrap_or_else(|e| panic!("case {name:?}: the source must be JSON: {e}"));
        assert!(
            source["components"]["schemas"][EVENT_ENVELOPE_COMPONENT].is_null(),
            "case {name:?}: a published document declares no envelope, got {source:?}"
        );

        let constraining = constraining_keywords(&converted);
        assert!(
            constraining.is_empty(),
            "case {name:?}: a document that names no root to promote exports none, \
             got {constraining:?} in {converted:?}"
        );
        assert_eq!(
            converted["$schema"],
            json!(JSON_SCHEMA_DRAFT4_URI),
            "case {name:?}: expected the root to name draft-04, got {converted:?}"
        );
        assert_eq!(
            converted["title"], source["info"]["title"],
            "case {name:?}: the export carries the document's title, got {converted:?}"
        );

        // Every component the published document defines is a definition of the export,
        // and no `$ref` into the source's components survives.
        let components = source["components"]["schemas"]
            .as_object()
            .unwrap_or_else(|| panic!("case {name:?}: the document has components"));
        for component in components.keys() {
            assert!(
                converted["definitions"][component].is_object(),
                "case {name:?}: component {component:?} must be a definition, got {converted:?}"
            );
        }
        assert!(
            !exported.str_at("Content").contains(OPENAPI_COMPONENT_REF),
            "case {name:?}: no OpenAPI component ref may survive, got {}",
            exported.str_at("Content")
        );
    }
}
