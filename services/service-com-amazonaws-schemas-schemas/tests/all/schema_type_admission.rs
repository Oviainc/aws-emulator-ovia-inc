// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the schema LANGUAGE a request names.
//!
//! RULE under test: a request's `Type` is admitted exactly when it names a value the
//! operation's own set admits — the modelled `Type` values for `GetDiscoveredSchema`,
//! which infers a document IN that language, and the narrower set of EXPORT TARGETS for
//! `ExportSchema`, which renders a stored document INTO one (that narrower set, and the
//! sentence a value outside it is refused with, are pinned in
//! `tests/all/export_reads_the_stored_schema.rs`). Where the handler is what refuses, the
//! refusal is 400 `BadRequestException` naming the admitted set, and it is decided on the
//! REQUEST — before any resource it names is resolved and before any event it carries is
//! read; where the member is modelled as the enum itself, the boundary validator refuses
//! the value first, against the same modelled set.
//!
//! Every case drives the real wire path through the harness in
//! `registry_schema_lifecycle.rs`: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it.

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::ops::types::TYPE_VALUES;
use service_com_amazonaws_schemas_schemas::state::{
    aws_managed_schemas, SchemaType, AWS_MANAGED_REGISTRY, JSON_SCHEMA_DRAFT4_URI,
};

use crate::registry_schema_lifecycle::{
    create_registry, export_uri, schema_uri, Answer, Emulator,
};

/// `GetDiscoveredSchema`'s URI. Source: the model's `@http` binding
/// (`smithy_query(query="operation",operation="GetDiscoveredSchema")`).
const DISCOVER_URI: &str = "/v1/discover";

/// The registry the exported schema lives in.
const REGISTRY: &str = "type-admission";

/// The schema the exported document is stored under.
const SCHEMA: &str = "order-placed";

/// The OpenAPI 3 document the export reads from: the only source language an export
/// can be rendered from, so every admitted `Type` has something to answer with.
const STORED: &str = concat!(
    r#"{"openapi":"3.0.0","info":{"title":"Order Placed"},"#,
    r#""components":{"schemas":{"AWSEvent":{"type":"object","properties":{"#,
    r#""id":{"type":"string"},"detail":{"type":"object","nullable":true}}}}}}"#
);

/// One event every discovery case below samples.
fn sampled_event() -> Value {
    json!({"id": "a1", "detail": {"total": 3}})
}

/// The refusal a `Type` outside the EXPORT TARGET set is answered with, built from that
/// set. Source: `crate::state::EXPORT_TARGETS`, the languages an export is rendered
/// into.
fn export_refusal(value: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at 'type' failed to satisfy \
         constraint: Member must satisfy enum value set: [{}]",
        SchemaType::JsonSchemaDraft4.as_wire()
    )
}

/// One `Type` a case sends, in both spellings a request needs: as a JSON body member
/// carries it and as a URI query string carries it.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// The value itself, as `GetDiscoveredSchema`'s body member carries it.
    value: &'static str,
    /// The same value percent-encoded, as `ExportSchema`'s `?type=` carries it.
    encoded: &'static str,
}

/// The values no modelled `Type` admits, each differing from an admitted one along
/// another dimension: a language this service does not store, letter case, a
/// neighbouring draft number, a truncation, surrounding whitespace, quoting, and two
/// admitted values sent at once.
const REFUSED: &[Row] = &[
    Row {
        case: "a schema language the service does not store",
        value: "Avro",
        encoded: "Avro",
    },
    Row {
        case: "an admitted value in another letter case",
        value: "openapi3",
        encoded: "openapi3",
    },
    Row {
        case: "another draft of the same language",
        value: "JSONSchemaDraft3",
        encoded: "JSONSchemaDraft3",
    },
    Row {
        case: "a prefix of an admitted value",
        value: "OpenApi",
        encoded: "OpenApi",
    },
    Row {
        case: "an admitted value with trailing whitespace",
        value: "OpenApi3 ",
        encoded: "OpenApi3%20",
    },
    Row {
        case: "an admitted value in quotes",
        value: "\"OpenApi3\"",
        encoded: "%22OpenApi3%22",
    },
    Row {
        case: "both admitted values at once",
        value: "JSONSchemaDraft4,OpenApi3",
        encoded: "JSONSchemaDraft4%2COpenApi3",
    },
];

/// Assert `answer` is the service's refusal of a `Type` that names no export target.
fn assert_refused(case: &str, value: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected 400, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "BadRequestException",
        "case {case:?}: expected a BadRequestException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("Code"),
        "MalformedRequest",
        "case {case:?}: expected the malformed-request code, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("Message"),
        export_refusal(value),
        "case {case:?}: the refusal must name the value and the admitted set, got {:?}",
        answer.body
    );
}

/// Assert `answer` is the boundary validator's refusal of a `Type` modelled as the
/// `Type` enum: the handler never runs, and the set the value failed against is the
/// modelled one.
fn assert_validation_refusal(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected 400, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected a ValidationException, got {:?}",
        answer.body
    );
    let message = answer
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    for modelled in TYPE_VALUES {
        assert!(
            message.contains(modelled),
            "case {case:?}: the refusal must name the modelled value {modelled:?}, \
             got {message:?}"
        );
    }
}

/// The `Content` of an answer, parsed. `Content` is schema SOURCE — a string on the
/// wire — so a client parses it exactly like this.
fn content(case: &str, answer: &Answer) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    let content = answer.str_at("Content");
    serde_json::from_str(&content)
        .unwrap_or_else(|error| panic!("case {case:?}: Content must be JSON ({error}): {content}"))
}

/// RULE: the set of `Type` values the service admits IS the modelled set — every
/// modelled value names a schema language this service knows, and nothing else does.
#[tokio::test]
async fn the_admitted_set_is_the_modelled_set() {
    let admitted: Vec<&str> = SchemaType::ALL
        .iter()
        .map(|language| language.as_wire())
        .collect();
    assert_eq!(
        admitted,
        TYPE_VALUES.to_vec(),
        "the languages the service admits must be the modelled Type values, got {admitted:?}"
    );
    for modelled in TYPE_VALUES {
        let language = SchemaType::from_modeled(modelled).unwrap_or_else(|refused| {
            panic!("case {modelled:?}: a modelled Type must be admitted, got {refused:?}")
        });
        assert_eq!(
            language.as_wire(),
            *modelled,
            "case {modelled:?}: the admitted language must be the one asked for, got {language:?}"
        );
    }
    for row in REFUSED {
        let refused = SchemaType::from_modeled(row.value);
        assert!(
            refused.is_err(),
            "case {:?}: {:?} is no modelled Type, got {refused:?}",
            row.case,
            row.value
        );
    }
}

/// RULE: every modelled `Type` is answered by the operation whose set is the modelled
/// one — discovery infers the document that language defines — while the export answers
/// only the language it renders into, refusing the other modelled value on the request.
#[tokio::test]
async fn every_modelled_type_is_answered_by_the_operation_whose_set_admits_it() {
    let emulator = Emulator::new();
    create_registry(&emulator, REGISTRY).await;
    let created = emulator
        .call(
            "POST",
            &schema_uri(REGISTRY, SCHEMA),
            Some(json!({"Content": STORED, "Type": SchemaType::OpenApi3.as_wire()})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateSchema, got {} {:?}",
        created.status, created.body
    );
    // A schema the SERVICE wrote, which is what an export serves; read from the seeded
    // catalogue rather than written out here.
    let published = aws_managed_schemas()
        .first()
        .expect("the catalogue publishes schemas")
        .name
        .clone();

    // Rows: every value the model's `Type` enum holds, asked of both operations.
    for modelled in TYPE_VALUES {
        let language = SchemaType::from_modeled(modelled).unwrap_or_else(|refused| {
            panic!("case {modelled:?}: a modelled Type must be admitted, got {refused:?}")
        });

        let exported = emulator
            .call(
                "GET",
                &export_uri(AWS_MANAGED_REGISTRY, &published, modelled),
                None,
            )
            .await;
        let case = format!("ExportSchema as {modelled}");

        let discovered = emulator
            .call(
                "POST",
                DISCOVER_URI,
                Some(json!({"Events": [sampled_event().to_string()], "Type": modelled})),
            )
            .await;
        let discovered_case = format!("GetDiscoveredSchema as {modelled}");
        let inferred = content(&discovered_case, &discovered);

        // Each language declares itself its own way: a draft-04 document names the
        // draft it is written in at its root, an OpenAPI document names its version.
        match language {
            SchemaType::JsonSchemaDraft4 => {
                // The one export target: the export is served and renders that language.
                let document = content(&case, &exported);
                for (case, document) in [(&case, &document), (&discovered_case, &inferred)] {
                    assert_eq!(
                        document["$schema"], JSON_SCHEMA_DRAFT4_URI,
                        "case {case:?}: a draft-04 document names the draft at its root, \
                         got {document:?}"
                    );
                }
            }
            SchemaType::OpenApi3 => {
                // A language a schema is STORED as, and no export target.
                assert_refused(&case, modelled, &exported);
                assert!(
                    inferred["openapi"].is_string(),
                    "case {discovered_case:?}: an OpenAPI document names its version, \
                     got {inferred:?}"
                );
            }
        }
    }
}

/// RULE: a `Type` outside the set the operation admits is answered by neither operation.
/// Where the handler is what admits the value (`ExportSchema`, whose `Type` is a
/// query-string `__string`), it is refused on the REQUEST — before the resource the
/// request names is resolved and before the events it carries are read — so the refusal
/// does not turn on what else the request happens to be wrong about; where the member is
/// modelled as the `Type` enum (`GetDiscoveredSchema`), the boundary validator refuses it
/// against the modelled set before the handler runs at all.
#[tokio::test]
async fn a_type_outside_the_admitted_set_is_answered_by_neither_operation() {
    let emulator = Emulator::new();
    create_registry(&emulator, REGISTRY).await;
    let created = emulator
        .call(
            "POST",
            &schema_uri(REGISTRY, SCHEMA),
            Some(json!({"Content": STORED, "Type": SchemaType::OpenApi3.as_wire()})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateSchema, got {} {:?}",
        created.status, created.body
    );
    let published = aws_managed_schemas()
        .first()
        .expect("the catalogue publishes schemas")
        .name
        .clone();

    for row in REFUSED {
        // The export of a schema that EXISTS, and of one in a registry that was never
        // created: the value is refused on the request either way.
        let exported = emulator
            .call(
                "GET",
                &export_uri(AWS_MANAGED_REGISTRY, &published, row.encoded),
                None,
            )
            .await;
        assert_refused(&format!("{}/ExportSchema", row.case), row.value, &exported);

        let missing = emulator
            .call(
                "GET",
                &export_uri("no-such-registry", "no-such-schema", row.encoded),
                None,
            )
            .await;
        assert_refused(
            &format!("{}/ExportSchema of a schema that does not exist", row.case),
            row.value,
            &missing,
        );

        // Discovery over an event that parses, and over one that does not: the value
        // is refused before any event is read either way — here by the boundary
        // validator, because this operation's `Type` IS the modelled enum.
        for (what, events) in [
            ("GetDiscoveredSchema", vec![sampled_event().to_string()]),
            (
                "GetDiscoveredSchema over an unreadable event",
                vec!["not json at all".to_string()],
            ),
        ] {
            let answer = emulator
                .call(
                    "POST",
                    DISCOVER_URI,
                    Some(json!({"Events": events, "Type": row.value})),
                )
                .await;
            assert_validation_refusal(&format!("{}/{what}", row.case), &answer);
        }
    }

    // The refusals changed nothing: the published schema still exports to the one target,
    // and the schema the caller stored still reads back.
    let exported = emulator
        .call(
            "GET",
            &export_uri(
                AWS_MANAGED_REGISTRY,
                &published,
                SchemaType::JsonSchemaDraft4.as_wire(),
            ),
            None,
        )
        .await;
    assert_eq!(
        exported.status, 200,
        "the published schema must still export, got {} {:?}",
        exported.status, exported.body
    );
    let described = emulator
        .call("GET", &schema_uri(REGISTRY, SCHEMA), None)
        .await;
    assert_eq!(
        described.str_at("Content"),
        STORED,
        "the stored document must still read back, got {:?}",
        described.str_at("Content")
    );
}
