// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what `ExportSchema` decides from: the schema LANGUAGE a
//! stored version carries and the language the request asks for — never who owns the
//! registry the schema lives in.
//!
//! Every case drives the real wire path through the harness in
//! `registry_schema_lifecycle.rs`: a schema is created over HTTP, exported over HTTP
//! and read back through `DescribeSchema`, so an export computed from anything but
//! the stored document would have to make a later read agree.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::ops::types::TYPE_VALUES;
use service_com_amazonaws_schemas_schemas::state::{
    aws_managed_schemas, AWS_MANAGED_REGISTRY, EVENT_ENVELOPE_COMPONENT, JSON_SCHEMA_DRAFT4_URI,
    OPENAPI_COMPONENT_REF,
};

use crate::registry_schema_lifecycle::{create_registry, export_uri, schema_uri, Emulator};

/// The schema language the export reads FROM. Source: `com.amazonaws.schemas#Type`,
/// re-exported as [`TYPE_VALUES`].
const OPEN_API_3: &str = "OpenApi3";

/// The schema language the export renders INTO. Source: as [`OPEN_API_3`].
const DRAFT4: &str = "JSONSchemaDraft4";

/// Where a draft-04 document keeps the schemas a `$ref` can name.
const DRAFT4_DEFINITION_REF: &str = "#/definitions/";

/// An OpenAPI 3 document with a single component and a title — the smallest document
/// the conversion has something to render.
const ONE_COMPONENT: &str = concat!(
    r#"{"openapi":"3.0.0","info":{"title":"Order Placed"},"#,
    r#""components":{"schemas":{"AWSEvent":{"type":"object"}}}}"#
);

/// An OpenAPI 3 document whose components reference each other, directly and through
/// an array, and which carries a nullable member.
const NESTED_REFS: &str = concat!(
    r#"{"openapi":"3.0.0","info":{"title":"Order Placed published by my.orders"},"#,
    r#""components":{"schemas":{"#,
    r#""AWSEvent":{"type":"object","properties":{"#,
    r##""detail":{"$ref":"#/components/schemas/Detail"},"##,
    r##""resources":{"type":"array","items":{"$ref":"#/components/schemas/Resource"}}}},"##,
    r#""Detail":{"type":"object","properties":{"id":{"type":"string"},"#,
    r#""extra":{"type":"object","nullable":true}}},"#,
    r#""Resource":{"type":"string"}}}}"#
);

/// An OpenAPI 3 document with four co-equal components, written out of name order and
/// carrying no title.
const MANY_UNTITLED: &str = concat!(
    r#"{"openapi":"3.0.0","components":{"schemas":{"#,
    r#""Zeta":{"type":"object"},"Alpha":{"type":"string"},"#,
    r##""Mu":{"type":"array","items":{"$ref":"#/components/schemas/Alpha"}},"##,
    r#""Beta":{"type":"integer"}}}}"#
);

/// An OpenAPI 3 document carrying the other members an OpenAPI document may hold
/// beside its components, and a nullable array member.
const PATHS_BESIDE_COMPONENTS: &str = concat!(
    r#"{"openapi":"3.0.0","info":{"title":"Shipment Delayed","version":"1.0.0"},"#,
    r#""paths":{},"servers":[{"url":"https://example.invalid/events"}],"#,
    r#""components":{"schemas":{"#,
    r#""Shipment":{"type":"object","properties":{"#,
    r#""tags":{"type":"array","nullable":true,"items":{"type":"string"}},"#,
    r##""carrier":{"$ref":"#/components/schemas/Carrier"}},"required":["carrier"]},"##,
    r#""Carrier":{"type":"string"}}}}"#
);

/// One schema an export case reads.
struct Row {
    case: &'static str,
    registry: String,
    schema: String,
    /// The document stored as [`OPEN_API_3`], which the export must be computed from.
    document: String,
    /// `(component, member, declared type)` of every member the source marks nullable:
    /// each must come back as draft-04's union of that type with `"null"`.
    nullable_members: Vec<(&'static str, &'static str, &'static str)>,
}

/// Create one schema over the wire, asserting it was stored.
async fn create_schema(
    emulator: &Emulator,
    registry: &str,
    schema: &str,
    schema_type: &str,
    content: &str,
) {
    let created = emulator
        .call(
            "POST",
            &schema_uri(registry, schema),
            Some(json!({"Content": content, "Type": schema_type})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case {registry}/{schema}: expected 201 from CreateSchema, got {} {:?}",
        created.status, created.body
    );
}

/// Every `$ref` target `value` carries, at any depth.
fn ref_targets(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(members) => {
            for (key, member) in members {
                if key == "$ref" {
                    if let Some(target) = member.as_str() {
                        found.push(target.to_string());
                    }
                }
                ref_targets(member, found);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| ref_targets(item, found)),
        _ => {}
    }
}

/// Whether `value` carries `key` as an object member at any depth.
fn carries_key(value: &Value, key: &str) -> bool {
    match value {
        Value::Object(members) => members
            .iter()
            .any(|(name, member)| name == key || carries_key(member, key)),
        Value::Array(items) => items.iter().any(|item| carries_key(item, key)),
        _ => false,
    }
}

/// The names of an object's members, as a set.
fn member_names(value: &Value, case: &str, what: &str) -> BTreeSet<String> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected an object at {what}, got {value:?}"))
        .keys()
        .cloned()
        .collect()
}

/// RULE: `ExportSchema` renders the stored document of ANY schema the caller can
/// read — the registry's owner is never consulted — so an `OpenApi3` version converts
/// to one JSON Schema draft-4 document computed from it: draft-04 named at the root,
/// the source's title carried over, the component the document roots at promoted to the
/// root, every other component a definition, every component `$ref` pointing into those
/// definitions and every `nullable` member a draft-04 type union.
#[tokio::test]
async fn an_openapi_3_schema_converts_wherever_it_is_stored() {
    assert!(
        TYPE_VALUES.contains(&OPEN_API_3) && TYPE_VALUES.contains(&DRAFT4),
        "both languages must be modelled Type values, got {TYPE_VALUES:?}"
    );

    let emulator = Emulator::new();
    create_registry(&emulator, "mine").await;
    create_registry(&emulator, "another-registry").await;

    // Rows: documents differing in how many components they carry, whether those
    // reference each other, whether the document is titled, what it carries beside
    // its components — and in which registry the schema lives, including a registry
    // AWS owns, which is the dimension this rule must NOT depend on.
    let mut rows = vec![
        Row {
            case: "a caller's own single-component document",
            registry: "mine".to_string(),
            schema: "order-placed".to_string(),
            document: ONE_COMPONENT.to_string(),
            nullable_members: vec![],
        },
        Row {
            case: "a caller's own document whose components reference each other",
            registry: "mine".to_string(),
            schema: "order-placed-detailed".to_string(),
            document: NESTED_REFS.to_string(),
            nullable_members: vec![("Detail", "extra", "object")],
        },
        Row {
            case: "co-equal components, untitled, out of name order",
            registry: "mine".to_string(),
            schema: "co.equal_components-99".to_string(),
            document: MANY_UNTITLED.to_string(),
            nullable_members: vec![],
        },
        Row {
            case: "a second registry of the caller's, document with paths and servers",
            registry: "another-registry".to_string(),
            schema: "shipment-delayed".to_string(),
            document: PATHS_BESIDE_COMPONENTS.to_string(),
            nullable_members: vec![("Shipment", "tags", "array")],
        },
    ];
    for row in &rows {
        create_schema(
            &emulator,
            &row.registry,
            &row.schema,
            OPEN_API_3,
            &row.document,
        )
        .await;
    }

    // And one schema AWS publishes, stored by the seeded catalogue rather than by a
    // request: the same assertions must hold for it.
    let published = aws_managed_schemas()
        .first()
        .expect("the seeded catalogue holds schemas")
        .clone();
    rows.push(Row {
        case: "a schema AWS publishes",
        registry: AWS_MANAGED_REGISTRY.to_string(),
        schema: published.name.clone(),
        document: published.content.clone(),
        nullable_members: vec![],
    });

    for row in &rows {
        let case = row.case;
        let exported = emulator
            .call("GET", &export_uri(&row.registry, &row.schema, DRAFT4), None)
            .await;
        assert_eq!(
            exported.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            exported.status, exported.body
        );
        assert_eq!(
            exported.str_at("Type"),
            DRAFT4,
            "case {case:?}: expected the export to report {DRAFT4}, got {:?}",
            exported.body
        );
        assert_eq!(
            exported.str_at("SchemaName"),
            row.schema,
            "case {case:?}: expected the export to name {:?}, got {:?}",
            row.schema,
            exported.body
        );

        // The resource the export read, through the implemented reader: the document
        // it converted and the identity it reports are the stored ones.
        let described = emulator
            .call("GET", &schema_uri(&row.registry, &row.schema), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected 200 from DescribeSchema, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.str_at("Content"),
            row.document,
            "case {case:?}: expected the stored document, got {:?}",
            described.str_at("Content")
        );
        assert_eq!(
            exported.str_at("SchemaVersion"),
            described.str_at("SchemaVersion"),
            "case {case:?}: expected the stored version {:?}, got {:?}",
            described.str_at("SchemaVersion"),
            exported.str_at("SchemaVersion")
        );
        assert_eq!(
            exported.body.get("SchemaArn"),
            described.body.get("SchemaArn"),
            "case {case:?}: expected the ARN DescribeSchema reports ({:?}), got {:?}",
            described.body.get("SchemaArn"),
            exported.body.get("SchemaArn")
        );

        let source: Value = serde_json::from_str(&row.document)
            .unwrap_or_else(|e| panic!("case {case:?}: the source document must be JSON: {e}"));
        let converted: Value = serde_json::from_str(&exported.str_at("Content"))
            .unwrap_or_else(|e| panic!("case {case:?}: the export must be JSON: {e}"));

        assert_eq!(
            converted["$schema"],
            json!(JSON_SCHEMA_DRAFT4_URI),
            "case {case:?}: expected the root to name draft-04, got {:?}",
            converted["$schema"]
        );
        // A document that roots at the event envelope has that component's keywords at
        // the export's root and the components left over as definitions; one that names
        // no root among its co-equal components has all of them as definitions.
        let mut components =
            member_names(&source["components"]["schemas"], case, "components.schemas");
        let promoted = components.remove(EVENT_ENVELOPE_COMPONENT);
        if promoted {
            let envelope = &source["components"]["schemas"][EVENT_ENVELOPE_COMPONENT];
            for keyword in member_names(envelope, case, EVENT_ENVELOPE_COMPONENT) {
                assert!(
                    !converted[&keyword].is_null(),
                    "case {case:?}: expected the promoted component's {keyword:?} at the \
                     root, got {converted:?}"
                );
            }
            assert!(
                converted["definitions"][EVENT_ENVELOPE_COMPONENT].is_null(),
                "case {case:?}: expected the promoted component not to be a definition \
                 too, got {converted:?}"
            );
        } else {
            assert!(
                converted["type"].is_null() && converted["properties"].is_null(),
                "case {case:?}: expected no root schema where the document names no \
                 root component, got {converted:?}"
            );
        }
        let definitions = if components.is_empty() {
            assert!(
                converted["definitions"].is_null(),
                "case {case:?}: expected no definitions where no component is left to \
                 define, got {converted:?}"
            );
            BTreeSet::new()
        } else {
            member_names(&converted["definitions"], case, "definitions")
        };
        assert_eq!(
            definitions, components,
            "case {case:?}: expected a definition per component {components:?}, got \
             {definitions:?}"
        );
        match source["info"]["title"].as_str() {
            Some(title) => assert_eq!(
                converted["title"],
                json!(title),
                "case {case:?}: expected the source's title {title:?}, got {:?}",
                converted["title"]
            ),
            None => assert!(
                converted["title"].is_null(),
                "case {case:?}: expected no title on an untitled document, got {:?}",
                converted["title"]
            ),
        }

        let mut exported_refs = Vec::new();
        ref_targets(&converted, &mut exported_refs);
        let mut source_refs = Vec::new();
        ref_targets(&source["components"]["schemas"], &mut source_refs);
        assert_eq!(
            exported_refs.len(),
            source_refs.len(),
            "case {case:?}: expected the source's {} refs to travel, got {exported_refs:?}",
            source_refs.len()
        );
        for target in &exported_refs {
            let named = target.strip_prefix(DRAFT4_DEFINITION_REF);
            assert!(
                named.is_some_and(|name| definitions.contains(name)),
                "case {case:?}: expected every ref to name a definition, got {target:?}"
            );
        }
        assert!(
            !exported.str_at("Content").contains(OPENAPI_COMPONENT_REF),
            "case {case:?}: expected no OpenAPI component ref to survive, got {}",
            exported.str_at("Content")
        );
        assert!(
            !carries_key(&converted, "nullable"),
            "case {case:?}: expected no `nullable` keyword in a draft-04 document, got \
             {converted:?}"
        );
        for (component, member, declared) in &row.nullable_members {
            let rendered = &converted["definitions"][component]["properties"][member]["type"];
            assert_eq!(
                *rendered,
                json!([declared, "null"]),
                "case {case:?}: expected {component}.{member} to render as \
                 [{declared:?},\"null\"], got {rendered:?}"
            );
        }

        // Asking for the language the document is already stored in is the stored
        // document, in the caller's registry as in AWS's.
        let as_stored = emulator
            .call(
                "GET",
                &export_uri(&row.registry, &row.schema, OPEN_API_3),
                None,
            )
            .await;
        assert_eq!(
            as_stored.status, 200,
            "case {case:?}: expected 200 exporting the stored language, got {} {:?}",
            as_stored.status, as_stored.body
        );
        assert_eq!(
            as_stored.str_at("Content"),
            row.document,
            "case {case:?}: expected the stored document, got {:?}",
            as_stored.str_at("Content")
        );

        // Exporting reports; it changes nothing a later reader sees.
        let read_again = emulator
            .call("GET", &schema_uri(&row.registry, &row.schema), None)
            .await;
        assert_eq!(
            read_again.body, described.body,
            "case {case:?}: expected the schema to read back unchanged ({:?}), got {:?}",
            described.body, read_again.body
        );
    }
}

/// RULE: the export's refusal is a property of the schema LANGUAGE the resolved
/// version is stored in, not of the registry holding it: a version stored as JSON
/// Schema draft 4 is already the language the conversion produces and carries nothing
/// an OpenAPI document needs around it, so there is no rendering of it to serve and
/// every request for one is refused 403 `ForbiddenException` naming that stored type —
/// while an `OpenApi3` schema in the SAME registry exports.
#[tokio::test]
async fn the_refusal_names_the_stored_schema_language() {
    let emulator = Emulator::new();
    create_registry(&emulator, "mine").await;
    create_schema(&emulator, "mine", "exportable", OPEN_API_3, ONE_COMPONENT).await;

    // Rows: documents stored as draft 4, differing in what they carry — nothing, the
    // dialect alone, and a full draft-04 document with definitions of its own.
    let stored_as_draft4 = [
        ("an empty document", "{}".to_string()),
        (
            "a dialect-only root",
            format!(r#"{{"$schema":"{JSON_SCHEMA_DRAFT4_URI}"}}"#),
        ),
        (
            "a draft-04 document with definitions",
            format!(
                r#"{{"$schema":"{JSON_SCHEMA_DRAFT4_URI}","title":"Order Placed","definitions":{{"AWSEvent":{{"type":"object"}}}}}}"#
            ),
        ),
    ];

    for (index, (case, document)) in stored_as_draft4.iter().enumerate() {
        let schema = format!("stored-as-draft4-{index}");
        create_schema(&emulator, "mine", &schema, DRAFT4, document).await;

        // Every language the model names, asked for: the refusal is the stored one's
        // property, so it does not turn on what was requested.
        for requested in TYPE_VALUES {
            let refused = emulator
                .call("GET", &export_uri("mine", &schema, requested), None)
                .await;
            assert_eq!(
                refused.status, 403,
                "case {case:?} as {requested}: expected 403, got {} {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "ForbiddenException",
                "case {case:?} as {requested}: expected ForbiddenException, got {:?}",
                refused.body
            );
            assert_eq!(
                refused.str_at("Message"),
                format!("Exporting of a schema with type {DRAFT4} is not supported."),
                "case {case:?} as {requested}: the refusal must name the stored type, got {:?}",
                refused.body
            );
        }

        // The refusal changed nothing, and the schema is still readable.
        let described = emulator
            .call("GET", &schema_uri("mine", &schema), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected 200 from DescribeSchema, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.str_at("Content"),
            *document,
            "case {case:?}: expected the stored document, got {:?}",
            described.str_at("Content")
        );
    }

    // The same caller, the same registry, a document stored as OpenAPI 3: it exports.
    for requested in TYPE_VALUES {
        let exported = emulator
            .call("GET", &export_uri("mine", "exportable", requested), None)
            .await;
        assert_eq!(
            exported.status, 200,
            "case {requested:?}: a stored OpenAPI 3 document exports from the same \
             registry, got {} {:?}",
            exported.status, exported.body
        );
        assert_eq!(
            exported.str_at("Type"),
            *requested,
            "case {requested:?}: expected the export to report the language asked for, \
             got {:?}",
            exported.body
        );
    }
}

/// RULE: the export renders the document of the VERSION it resolves — the one the
/// request names, or the latest when it names none — for a caller's own schema as for
/// any other.
#[tokio::test]
async fn the_export_renders_the_version_it_resolves() {
    let emulator = Emulator::new();
    create_registry(&emulator, "versioned").await;

    let first = concat!(
        r#"{"openapi":"3.0.0","info":{"title":"Order Placed"},"#,
        r#""components":{"schemas":{"First":{"type":"object"}}}}"#
    );
    let second = concat!(
        r#"{"openapi":"3.0.0","info":{"title":"Order Placed"},"#,
        r#""components":{"schemas":{"Second":{"type":"string"}}}}"#
    );
    create_schema(&emulator, "versioned", "evolving", OPEN_API_3, first).await;
    let updated = emulator
        .call(
            "PUT",
            &schema_uri("versioned", "evolving"),
            Some(json!({"Content": second, "Type": OPEN_API_3})),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "expected 200 from UpdateSchema, got {} {:?}",
        updated.status, updated.body
    );

    // Rows: no version named (the latest answers), each stored version named.
    let cases = [
        ("the latest version", "", "2", "Second", "First"),
        ("the first version", "&schemaVersion=1", "1", "First", "Second"),
        ("the second version", "&schemaVersion=2", "2", "Second", "First"),
    ];

    for (case, query, version, present, absent) in cases {
        let uri = format!(
            "{}{query}",
            export_uri("versioned", "evolving", DRAFT4)
        );
        let exported = emulator.call("GET", &uri, None).await;
        assert_eq!(
            exported.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            exported.status, exported.body
        );
        assert_eq!(
            exported.str_at("SchemaVersion"),
            version,
            "case {case:?}: expected version {version}, got {:?}",
            exported.body
        );
        let converted: Value = serde_json::from_str(&exported.str_at("Content"))
            .unwrap_or_else(|e| panic!("case {case:?}: the export must be JSON: {e}"));
        assert!(
            converted["definitions"][present].is_object(),
            "case {case:?}: expected {present:?} among the definitions, got {converted:?}"
        );
        assert!(
            converted["definitions"][absent].is_null(),
            "case {case:?}: expected no {absent:?} among the definitions, got {converted:?}"
        );
    }
}
