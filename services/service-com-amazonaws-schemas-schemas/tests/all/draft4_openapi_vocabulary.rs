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
//! Every case stores its document over HTTP and exports it over HTTP, then reads the
//! stored document back — an export computed from anything but the stored document would
//! have to make that read agree.

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::state::JSON_SCHEMA_DRAFT4_URI;

use crate::registry_schema_lifecycle::{
    create_registry, export_uri, schema_uri, Emulator, OPENAPI_ONLY_KEYWORDS,
};

/// The schema language each document below is stored as.
const OPEN_API_3: &str = "OpenApi3";

/// The schema language the export renders into.
const DRAFT4: &str = "JSONSchemaDraft4";

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

/// RULE: none of OpenAPI 3.0's own keywords survives the conversion, wherever in the
/// document it sits and whether it is written as a schema keyword or as a member of a
/// `properties` map; `nullable` is re-stated as draft-04's type union first.
#[tokio::test]
async fn no_openapi_only_keyword_survives_the_draft_4_conversion() {
    let emulator = Emulator::new();
    create_registry(&emulator, "mine").await;

    // Rows differ along what the rule generalises over: WHICH keyword, WHERE it sits (a
    // component's own keywords, a member's, an array's items, a nested object, the
    // component the document roots at), whether it is a keyword or a member NAME, and
    // whether the position it marks declares a type for `nullable` to widen at all.
    let rows = [
        Row {
            case: "a keyword on a component and on one of its members",
            schema: "order.placed",
            document: concat!(
                r#"{"openapi":"3.0.0","info":{"title":"Order Placed published by my.orders"},"#,
                r#""components":{"schemas":{"Order":{"type":"object","readOnly":true,"#,
                r#""properties":{"id":{"type":"string","example":"abc"}}}}}}"#
            ),
            exported: concat!(
                r#"{"$schema":"http://json-schema.org/draft-04/schema#","#,
                r#""title":"Order Placed published by my.orders","#,
                r#""definitions":{"Order":{"properties":{"id":{"type":"string"}},"#,
                r#""type":"object"}}}"#
            ),
        },
        Row {
            case: "members named for the keywords, beside one that is not",
            schema: "audit.record",
            document: concat!(
                r#"{"openapi":"3.0.0","info":{"title":"Record published by my.audit"},"#,
                r#""components":{"schemas":{"Record":{"type":"object","properties":{"#,
                r#""readOnly":{"type":"boolean"},"writeOnly":{"type":"boolean"},"#,
                r#""when":{"type":"string","format":"date-time"}}}}}}"#
            ),
            exported: concat!(
                r#"{"$schema":"http://json-schema.org/draft-04/schema#","#,
                r#""title":"Record published by my.audit","#,
                r#""definitions":{"Record":{"properties":{"when":{"format":"date-time","#,
                r#""type":"string"}},"type":"object"}}}"#
            ),
        },
        Row {
            case: "nullable and a keyword on an array's items and on a nested member",
            schema: "job.batch",
            document: concat!(
                r#"{"openapi":"3.0.0","info":{"title":"Batch published by my.jobs"},"#,
                r#""components":{"schemas":{"Batch":{"type":"object","properties":{"#,
                r#""items":{"type":"array","items":{"type":"object","nullable":true,"#,
                r#""deprecated":true,"properties":{"note":{"type":"string","#,
                r#""nullable":true}}}}}}}}}"#
            ),
            exported: concat!(
                r#"{"$schema":"http://json-schema.org/draft-04/schema#","#,
                r#""title":"Batch published by my.jobs","#,
                r#""definitions":{"Batch":{"properties":{"items":{"items":{"#,
                r#""properties":{"note":{"type":["string","null"]}},"#,
                r#""type":["object","null"]},"type":"array"}},"type":"object"}}}"#
            ),
        },
        Row {
            case: "keywords on the component the document roots at, and a discriminator",
            schema: "shipment.created",
            document: concat!(
                r#"{"openapi":"3.0.0","info":{"title":"Shipment published by my.shipping"},"#,
                r##""components":{"schemas":{"AWSEvent":{"type":"object","deprecated":true,"##,
                r##""xml":{"name":"event"},"properties":{"detail":"##,
                r##"{"$ref":"#/components/schemas/Shipment"}}},"##,
                r#""Shipment":{"type":"object","discriminator":{"propertyName":"kind"},"#,
                r#""properties":{"kind":{"type":"string"},"example":{"type":"string"}}}}}}"#
            ),
            exported: concat!(
                r#"{"$schema":"http://json-schema.org/draft-04/schema#","#,
                r#""title":"Shipment published by my.shipping","#,
                r##""properties":{"detail":{"$ref":"#/definitions/Shipment"}},"##,
                r#""type":"object","definitions":{"Shipment":{"properties":{"#,
                r#""kind":{"type":"string"}},"type":"object"}}}"#
            ),
        },
        Row {
            case: "a nullable position that declares no type, beside externalDocs",
            schema: "loose.event",
            document: concat!(
                r#"{"openapi":"3.0.0","info":{"title":"Loose published by my.loose"},"#,
                r#""components":{"schemas":{"Loose":{"type":"object","#,
                r#""externalDocs":{"url":"https://example.invalid"},"properties":{"#,
                r##""any":{"nullable":true},"self":{"$ref":"#/components/schemas/Loose"}"##,
                r#"}}}}}"#
            ),
            exported: concat!(
                r#"{"$schema":"http://json-schema.org/draft-04/schema#","#,
                r#""title":"Loose published by my.loose","#,
                r##""definitions":{"Loose":{"properties":{"any":{},"##,
                r##""self":{"$ref":"#/definitions/Loose"}},"type":"object"}}}"##
            ),
        },
    ];

    // Every keyword the rule names is exercised by some row, so no row can be dropped
    // without losing one.
    for keyword in OPENAPI_ONLY_KEYWORDS {
        assert!(
            rows.iter()
                .any(|row| row.document.contains(&format!("\"{keyword}\""))),
            "case {keyword:?}: a row must store a document carrying it"
        );
    }

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
        let content = exported.str_at("Content");
        assert_eq!(
            content, row.exported,
            "case {case:?}: expected {}, got {content}",
            row.exported
        );
        for keyword in OPENAPI_ONLY_KEYWORDS {
            assert!(
                !content.contains(&format!("\"{keyword}\"")),
                "case {case:?}: draft-04 defines no {keyword:?}, got {content}"
            );
        }

        // What the dropped `nullable` said is said by the type union instead, and a
        // position that declared no type is left with nothing to widen.
        let converted: Value = serde_json::from_str(&content)
            .unwrap_or_else(|e| panic!("case {case:?}: the export must be JSON: {e}"));
        let source: Value = serde_json::from_str(row.document)
            .unwrap_or_else(|e| panic!("case {case:?}: the source must be JSON: {e}"));
        assert_eq!(
            converted["$schema"],
            json!(JSON_SCHEMA_DRAFT4_URI),
            "case {case:?}: the export names draft-04, got {converted:?}"
        );
        assert_eq!(
            converted["title"], source["info"]["title"],
            "case {case:?}: the export carries the source document's title, got {converted:?}"
        );

        // The stored document is unchanged by being exported: a read answers the bytes
        // the create stored, OpenAPI vocabulary and all.
        let described = emulator
            .call("GET", &schema_uri("mine", row.schema), None)
            .await;
        assert_eq!(
            described.str_at("Content"),
            row.document,
            "case {case:?}: expected the stored document, got {:?}",
            described.str_at("Content")
        );
        // And exporting it again renders the same bytes: the conversion is a function of
        // the stored document.
        let again = emulator
            .call("GET", &export_uri("mine", row.schema, DRAFT4), None)
            .await;
        assert_eq!(
            again.str_at("Content"),
            content,
            "case {case:?}: two exports must render one document"
        );
    }
}

/// RULE (the invariant behind the rows above, over a position of every JSON type):
/// `nullable: true` widens the declared type to the union of it with `"null"`, and no
/// other member of the position changes.
#[tokio::test]
async fn nullable_widens_the_declared_type_whatever_the_type_is() {
    let emulator = Emulator::new();
    create_registry(&emulator, "widened").await;

    // Rows: every type name JSON Schema draft-04 defines for an instance (section 5.5.2),
    // so the widening is asserted over the whole set rather than the one the cited
    // document happened to carry.
    let types = [
        "array", "boolean", "integer", "number", "object", "string",
    ];
    for declared in types {
        let schema = format!("nullable.{declared}");
        let document = format!(
            concat!(
                r#"{{"openapi":"3.0.0","info":{{"title":"Widened published by my.types"}},"#,
                r#""components":{{"schemas":{{"Widened":{{"type":"object","properties":{{"#,
                r#""marked":{{"type":"{declared}","nullable":true}},"#,
                r#""plain":{{"type":"{declared}"}}}}}}}}}}}}"#
            ),
            declared = declared
        );
        create_schema(&emulator, "widened", &schema, &document).await;

        let exported = emulator
            .call("GET", &export_uri("widened", &schema, DRAFT4), None)
            .await;
        assert_eq!(
            exported.status, 200,
            "case {declared:?}: expected 200, got {} {:?}",
            exported.status, exported.body
        );
        let converted: Value = serde_json::from_str(&exported.str_at("Content"))
            .unwrap_or_else(|e| panic!("case {declared:?}: the export must be JSON: {e}"));
        let properties = &converted["definitions"]["Widened"]["properties"];
        assert_eq!(
            properties["marked"]["type"],
            json!([declared, "null"]),
            "case {declared:?}: a nullable position widens to the union, got {properties:?}"
        );
        assert_eq!(
            properties["plain"]["type"],
            json!(declared),
            "case {declared:?}: a position the document did not mark keeps its type, \
             got {properties:?}"
        );
    }
}
