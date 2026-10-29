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
//! `registry_schema_lifecycle.rs`: each document is stored over HTTP, exported over HTTP
//! and read back through `DescribeSchema`, so an export computed from anything but the
//! stored document would have to make a later read agree.

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::state::{
    aws_managed_schemas, ManagedSchema, AWS_MANAGED_REGISTRY, EVENT_ENVELOPE_COMPONENT,
    JSON_SCHEMA_DRAFT4_URI, OPENAPI_COMPONENT_REF,
};

use crate::registry_schema_lifecycle::{create_registry, export_uri, schema_uri, Emulator};

/// The schema language the export reads from.
const OPEN_API_3: &str = "OpenApi3";

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

/// One document the export is rendered from, and the document it must render.
struct Row {
    case: &'static str,
    schema: &'static str,
    /// The `OpenApi3` document stored over HTTP.
    document: &'static str,
    /// The `Content` the draft-4 export must answer, byte for byte: the rendered member
    /// ORDER is part of the document a client caches, diffs and hashes.
    exported: &'static str,
}

/// Store one schema over the wire, asserting it was stored.
async fn create_schema(emulator: &Emulator, registry: &str, schema: &str, content: &str) {
    let created = emulator
        .call(
            "POST",
            &schema_uri(registry, schema),
            Some(json!({"Content": content, "Type": OPEN_API_3})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case {registry}/{schema}: expected 201 from CreateSchema, got {} {:?}",
        created.status, created.body
    );
}

/// The keywords of `schema` that constrain an instance.
fn constraining_keywords(schema: &Value) -> Vec<&'static str> {
    DRAFT4_CONSTRAINING_KEYWORDS
        .iter()
        .filter(|keyword| !schema[**keyword].is_null())
        .copied()
        .collect()
}

/// RULE: the export is rooted at the component the source document roots at — the event
/// envelope — with the components left over as `definitions` and every reference
/// following the component it names; a document naming no root component keeps all of
/// them as definitions.
#[tokio::test]
async fn the_draft_4_export_is_rooted_at_the_component_the_document_roots_at() {
    let emulator = Emulator::new();
    create_registry(&emulator, "mine").await;

    // Rows differ along what the rule generalises over: whether the document defines the
    // component it roots at, how many components are left to define, whether a component
    // refers back to the promoted one, whether a reference leaves the document, whether
    // the document (or the promoted component) is titled, and which keywords the
    // promoted component carries.
    let rows = [
        Row {
            case: "rooted at the envelope, with a detail component and a nullable member",
            schema: "order-placed",
            document: concat!(
                r#"{"openapi":"3.0.0","info":{"title":"Order Placed published by my.orders"},"#,
                r#""paths":{},"components":{"schemas":{"#,
                r#""AWSEvent":{"type":"object","required":["detail","id"],"properties":{"#,
                r##""detail":{"$ref":"#/components/schemas/Detail"},"id":{"type":"string"}}},"##,
                r#""Detail":{"type":"object","properties":{"#,
                r#""extra":{"type":"object","nullable":true}}}}}}"#
            ),
            exported: concat!(
                r#"{"$schema":"http://json-schema.org/draft-04/schema#","#,
                r#""title":"Order Placed published by my.orders","#,
                r##""properties":{"detail":{"$ref":"#/definitions/Detail"},"##,
                r#""id":{"type":"string"}},"required":["detail","id"],"type":"object","#,
                r#""definitions":{"Detail":{"properties":{"#,
                r#""extra":{"type":["object","null"]}},"type":"object"}}}"#
            ),
        },
        Row {
            case: "the envelope alone, so no component is left to define",
            schema: "order-cancelled",
            document: concat!(
                r#"{"openapi":"3.0.0","info":{"title":"Order Cancelled published by my.orders"},"#,
                r#""components":{"schemas":{"AWSEvent":{"type":"object","#,
                r#""properties":{"id":{"type":"string"}}}}}}"#
            ),
            exported: concat!(
                r#"{"$schema":"http://json-schema.org/draft-04/schema#","#,
                r#""title":"Order Cancelled published by my.orders","#,
                r#""properties":{"id":{"type":"string"}},"type":"object"}"#
            ),
        },
        Row {
            case: "a definition referring back to the promoted component, and one leaving the document",
            schema: "order-amended",
            document: concat!(
                r#"{"openapi":"3.0.0","components":{"schemas":{"#,
                r##""AWSEvent":{"type":"object","properties":{"detail":{"$ref":"#/components/schemas/Detail"}}},"##,
                r#""Detail":{"type":"object","properties":{"#,
                r##""replaces":{"$ref":"#/components/schemas/AWSEvent"},"##,
                r##""elsewhere":{"$ref":"https://example.invalid/other#/Thing"}}}}}}"##
            ),
            exported: concat!(
                r#"{"$schema":"http://json-schema.org/draft-04/schema#","#,
                r##""properties":{"detail":{"$ref":"#/definitions/Detail"}},"type":"object","##,
                r#""definitions":{"Detail":{"properties":{"#,
                r##""elsewhere":{"$ref":"https://example.invalid/other#/Thing"},"##,
                r##""replaces":{"$ref":"#"}},"type":"object"}}}"##
            ),
        },
        Row {
            case: "co-equal components the document names no root among",
            schema: "catalog",
            document: concat!(
                r#"{"openapi":"3.0.0","info":{"title":"Catalog"},"components":{"schemas":{"#,
                r#""Zeta":{"type":"object"},"Alpha":{"type":"string"},"#,
                r##""Mu":{"type":"array","items":{"$ref":"#/components/schemas/Alpha"}}}}}"##
            ),
            exported: concat!(
                r#"{"$schema":"http://json-schema.org/draft-04/schema#","title":"Catalog","#,
                r##""definitions":{"Alpha":{"type":"string"},"##,
                r##""Mu":{"items":{"$ref":"#/definitions/Alpha"},"type":"array"},"##,
                r#""Zeta":{"type":"object"}}}"#
            ),
        },
        Row {
            case: "an envelope carrying an EventBridge extension and an array of references",
            schema: "shipment-delayed",
            document: concat!(
                r#"{"openapi":"3.0.0","#,
                r#""info":{"title":"Shipment Delayed published by my.shipping"},"#,
                r#""components":{"schemas":{"AWSEvent":{"type":"object","#,
                r#""x-amazon-events-source":"my.shipping","properties":{"#,
                r##""resources":{"type":"array","items":{"$ref":"#/components/schemas/Resource"}}}},"##,
                r#""Resource":{"type":"string"}}}}"#
            ),
            exported: concat!(
                r#"{"$schema":"http://json-schema.org/draft-04/schema#","#,
                r#""title":"Shipment Delayed published by my.shipping","properties":{"#,
                r##""resources":{"items":{"$ref":"#/definitions/Resource"},"type":"array"}},"##,
                r#""type":"object","x-amazon-events-source":"my.shipping","#,
                r#""definitions":{"Resource":{"type":"string"}}}"#
            ),
        },
        Row {
            case: "a promoted component carrying a title of its own",
            schema: "titled-twice",
            document: concat!(
                r#"{"openapi":"3.0.0","info":{"title":"Order Returned published by my.orders"},"#,
                r#""components":{"schemas":{"#,
                r#""AWSEvent":{"title":"The envelope","type":"object"}}}}"#
            ),
            // A JSON object carries one member per name: the document's own title is the
            // one the translation states.
            exported: concat!(
                r#"{"$schema":"http://json-schema.org/draft-04/schema#","#,
                r#""title":"Order Returned published by my.orders","type":"object"}"#
            ),
        },
    ];

    for row in &rows {
        let case = row.case;
        create_schema(&emulator, "mine", row.schema, row.document).await;

        let exported = emulator
            .call("GET", &export_uri("mine", row.schema, DRAFT4), None)
            .await;
        assert_eq!(
            exported.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            exported.status, exported.body
        );
        assert_eq!(
            exported.str_at("Content"),
            row.exported,
            "case {case:?}: expected {}, got {}",
            row.exported,
            exported.str_at("Content")
        );
        assert!(
            !exported.str_at("Content").contains(OPENAPI_COMPONENT_REF),
            "case {case:?}: no OpenAPI component ref may survive, got {}",
            exported.str_at("Content")
        );

        // The resource the export read is unchanged by the reading, and the export is
        // computed from it: the stored document still reads back as it was stored.
        let described = emulator
            .call("GET", &schema_uri("mine", row.schema), None)
            .await;
        assert_eq!(
            described.str_at("Content"),
            row.document,
            "case {case:?}: expected the stored document, got {:?}",
            described.str_at("Content")
        );

        // A document that roots at a component says something about the instance at its
        // root; one that names no root component says what it can — its definitions.
        let converted: Value = serde_json::from_str(&exported.str_at("Content"))
            .unwrap_or_else(|e| panic!("case {case:?}: the export must be JSON: {e}"));
        let source: Value = serde_json::from_str(row.document)
            .unwrap_or_else(|e| panic!("case {case:?}: the source must be JSON: {e}"));
        let roots_at_a_component =
            source["components"]["schemas"][EVENT_ENVELOPE_COMPONENT].is_object();
        let constraining = constraining_keywords(&converted);
        assert_eq!(
            !constraining.is_empty(),
            roots_at_a_component,
            "case {case:?}: a root constrains the instance only where the document names \
             the component to root at, got {constraining:?} in {converted:?}"
        );
        assert_eq!(
            converted["$schema"],
            json!(JSON_SCHEMA_DRAFT4_URI),
            "case {case:?}: expected the root to name draft-04, got {converted:?}"
        );
        assert!(
            converted["definitions"][EVENT_ENVELOPE_COMPONENT].is_null(),
            "case {case:?}: the promoted component is the root, not a definition too, \
             got {converted:?}"
        );
    }
}

/// RULE (the same rule, over the documents AWS publishes rather than a caller's): the
/// draft-04 export of a published event schema is rooted at the envelope the published
/// document roots at — so its root constrains the event, its `detail` names the
/// event-specific component among the `definitions`, and the envelope is not a
/// definition of its own export.
#[tokio::test]
async fn a_published_event_schema_exports_rooted_at_its_envelope() {
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
        let envelope = &source["components"]["schemas"][EVENT_ENVELOPE_COMPONENT];

        let constraining = constraining_keywords(&converted);
        assert!(
            !constraining.is_empty(),
            "case {name:?}: the export's root must constrain the event, got {converted:?}"
        );
        assert_eq!(
            converted["type"], envelope["type"],
            "case {name:?}: the root is the envelope's schema, got {converted:?}"
        );
        assert_eq!(
            converted["required"], envelope["required"],
            "case {name:?}: the root requires what the envelope requires, got {converted:?}"
        );
        assert!(
            converted["definitions"][EVENT_ENVELOPE_COMPONENT].is_null(),
            "case {name:?}: the promoted envelope is no definition of its own export, \
             got {converted:?}"
        );

        // The envelope's `detail` named a component of the source document; it names
        // where the conversion put that component.
        let event_component = envelope["properties"]["detail"]["$ref"]
            .as_str()
            .and_then(|target| target.strip_prefix(OPENAPI_COMPONENT_REF))
            .unwrap_or_else(|| panic!("case {name:?}: the envelope's detail names a component"));
        assert_eq!(
            converted["properties"]["detail"]["$ref"],
            json!(format!("#/definitions/{event_component}")),
            "case {name:?}: the root's detail must name the event's definition, got {converted:?}"
        );
        assert!(
            converted["definitions"][event_component].is_object(),
            "case {name:?}: the event's component must be a definition, got {converted:?}"
        );
    }
}
