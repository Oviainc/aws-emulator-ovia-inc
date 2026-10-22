// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the content `UpdateSchema` will store as a new version.
//!
//! The versioning and idempotency rules themselves are covered in
//! `aws_managed_and_versions.rs`; what this suite pins is which `Content` a version
//! may be minted FROM — a schema registry stores schema source, so content that is not
//! a JSON schema document at all is refused with the modelled `BadRequestException`
//! and stores nothing, while any JSON object document is stored and handed back
//! verbatim.
//!
//! Every case drives the real wire path through the harness in
//! `registry_schema_lifecycle.rs`; nothing here touches the state struct directly.

use serde_json::{json, Value};

use crate::registry_schema_lifecycle::{create_registry, schema_uri, Answer, Emulator};

/// The content every case starts the schema off with: a JSON object document of the
/// declared type.
fn stored_document(schema_type: &str, marker: &str) -> String {
    match schema_type {
        "OpenApi3" => json!({
            "openapi": "3.0.0",
            "info": {"title": marker, "version": "1.0.0"},
            "paths": {},
        })
        .to_string(),
        "JSONSchemaDraft4" => json!({
            "$schema": "http://json-schema.org/draft-04/schema#",
            "type": "object",
            "title": marker,
        })
        .to_string(),
        other => panic!("unknown schema type {other:?}"),
    }
}

/// Create a schema holding `content`, asserting the create succeeded.
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
        "case {schema:?}: expected 201 from CreateSchema, got {} {:?}",
        created.status, created.body
    );
}

/// `PUT` an update, returning the answer untouched so the case can assert on it.
async fn update_schema(emulator: &Emulator, registry: &str, schema: &str, body: Value) -> Answer {
    emulator
        .call("PUT", &schema_uri(registry, schema), Some(body))
        .await
}

/// Read one version back (`SchemaVersion` absent means "the latest").
async fn describe(
    emulator: &Emulator,
    registry: &str,
    schema: &str,
    version: Option<&str>,
) -> Answer {
    let uri = match version {
        None => schema_uri(registry, schema),
        Some(version) => format!("{}?schemaVersion={version}", schema_uri(registry, schema)),
    };
    emulator.call("GET", &uri, None).await
}

/// RULE: `Content` a version is minted from must be a JSON schema document — a JSON
/// body whose top level is an object. Content that is not is refused with HTTP 400
/// `BadRequestException` and stores nothing: the schema still reports the version and
/// the content it held, and it still takes the next valid update.
#[tokio::test]
async fn content_that_is_not_a_json_schema_document_is_refused_and_stores_nothing() {
    // Rows: the forms a body can take that are not a JSON object — not JSON at all,
    // truncated JSON, JSON with trailing garbage, and each other JSON type. Crossed
    // with both modelled `Type` values, because the requirement is the same for both.
    let shapes: [(&str, &str); 9] = [
        ("not-json", "this is not a schema"),
        ("truncated-json", "{\"type\":"),
        ("trailing-garbage", "{} }"),
        ("json-array", "[{\"type\":\"object\"}]"),
        ("json-number", "7"),
        ("json-string", "\"object\""),
        ("json-boolean", "true"),
        ("json-null", "null"),
        // One character, the lower bound of the modelled `len 1..100000`.
        ("one-character", "x"),
    ];

    for schema_type in ["OpenApi3", "JSONSchemaDraft4"] {
        for (shape, invalid) in shapes {
            let case = format!("{schema_type}/{shape}");
            let emulator = Emulator::new();
            let registry = "validated";
            let schema = "events";
            create_registry(&emulator, registry).await;
            let stored = stored_document(schema_type, "v1");
            create_schema(&emulator, registry, schema, schema_type, &stored).await;

            let refused = update_schema(
                &emulator,
                registry,
                schema,
                json!({"Content": invalid, "Type": schema_type}),
            )
            .await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: expected 400, got {} {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "BadRequestException",
                "case {case:?}: expected the wire code BadRequestException, got {:?}",
                refused.body
            );
            assert_eq!(
                refused.str_at("Code"),
                "MalformedRequest",
                "case {case:?}: the error's own Code member carries the service's error \
                 code for the condition, got {:?}",
                refused.body
            );
            assert!(
                refused.str_at("Message").contains("Content"),
                "case {case:?}: the refusal must name the member it is about, got {:?}",
                refused.body
            );

            // Nothing was stored: the schema is still at the version the create
            // stored, with the content it stored.
            let latest = describe(&emulator, registry, schema, None).await;
            assert_eq!(
                latest.str_at("SchemaVersion"),
                "1",
                "case {case:?}: a refused update must mint no version, got {:?}",
                latest.body
            );
            assert_eq!(
                latest.str_at("Content"),
                stored,
                "case {case:?}: a refused update must not replace the stored content"
            );
            let never_minted = describe(&emulator, registry, schema, Some("2")).await;
            assert_eq!(
                never_minted.status, 404,
                "case {case:?}: the refused update's version must not exist, got {} {:?}",
                never_minted.status, never_minted.body
            );

            // And the refusal left nothing wedged: the next valid update still mints.
            let accepted = stored_document(schema_type, "v2");
            let updated = update_schema(
                &emulator,
                registry,
                schema,
                json!({"Content": accepted, "Type": schema_type}),
            )
            .await;
            assert_eq!(
                updated.str_at("SchemaVersion"),
                "2",
                "case {case:?}: a valid update after a refused one mints version 2, got {:?}",
                updated.body
            );
        }
    }
}

/// RULE: whether `Content` is a schema document is a property of the REQUEST, so the
/// refusal does not depend on the schema's idempotency-token history: a call whose
/// `ClientTokenId` has already minted a version is still refused when it sends content
/// that is not a document, rather than being answered as a retry.
#[tokio::test]
async fn content_is_refused_whatever_the_token_history() {
    let emulator = Emulator::new();
    let registry = "validated";
    let schema = "events";
    create_registry(&emulator, registry).await;
    create_schema(
        &emulator,
        registry,
        schema,
        "OpenApi3",
        &stored_document("OpenApi3", "v1"),
    )
    .await;

    let served = stored_document("OpenApi3", "v2");
    let minted = update_schema(
        &emulator,
        registry,
        schema,
        json!({"Content": served, "Type": "OpenApi3", "ClientTokenId": "tok"}),
    )
    .await;
    assert_eq!(
        minted.str_at("SchemaVersion"),
        "2",
        "case served: the first call mints version 2, got {:?}",
        minted.body
    );

    // Rows: the token the first call used, and a token no call has used. Both carry
    // content that is not a document, so both must be refused.
    for (case, token) in [("already-minted-token", "tok"), ("fresh-token", "other")] {
        let refused = update_schema(
            &emulator,
            registry,
            schema,
            json!({"Content": "not a schema", "Type": "OpenApi3", "ClientTokenId": token}),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            refused.body
        );
    }

    let latest = describe(&emulator, registry, schema, None).await;
    assert_eq!(
        latest.str_at("SchemaVersion"),
        "2",
        "the refusals must leave the stored version alone, got {:?}",
        latest.body
    );
    assert_eq!(
        latest.str_at("Content"),
        served,
        "and the stored content alone"
    );
}

/// RULE: any JSON object document is content a version may be minted from — the
/// refusal is scoped to content that is not a schema document at all, and a stored
/// version hands its content back byte for byte.
#[tokio::test]
async fn every_json_object_document_is_stored_and_read_back_verbatim() {
    // Rows: documents differing along what the rule generalises over — how complete
    // the document is, its type, its alphabet, its nesting and its size. The empty
    // object is the smallest JSON object there is; the last row sits just under the
    // modelled `len 1..100000` ceiling.
    let documents: [(&str, &str, String); 7] = [
        (
            "full-openapi-document",
            "OpenApi3",
            stored_document("OpenApi3", "complete"),
        ),
        (
            "openapi-version-only",
            "OpenApi3",
            r#"{"openapi":"3.0.1"}"#.to_string(),
        ),
        ("empty-object", "OpenApi3", "{}".to_string()),
        (
            "draft4-document",
            "JSONSchemaDraft4",
            stored_document("JSONSchemaDraft4", "complete"),
        ),
        (
            "nested-and-non-ascii",
            "JSONSchemaDraft4",
            json!({
                "type": "object",
                "properties": {"caf\u{e9}": {"type": "string", "description": "\u{30b9}\u{30ad}\u{30fc}\u{30de}"}},
                "deep": {"a": {"b": {"c": [1, 2, {"d": null}]}}},
            })
            .to_string(),
        ),
        (
            "whitespace-and-newlines",
            "OpenApi3",
            "{\n  \"openapi\" : \"3.0.0\" ,\n  \"info\" : {\"title\": \"spaced\", \"version\": \"1\"}\n}"
                .to_string(),
        ),
        (
            "near-the-modelled-upper-length-bound",
            "JSONSchemaDraft4",
            format!(r#"{{"type":"object","description":"{}"}}"#, "x".repeat(99_900)),
        ),
    ];

    let emulator = Emulator::new();
    let registry = "accepted";
    let schema = "events";
    create_registry(&emulator, registry).await;
    create_schema(
        &emulator,
        registry,
        schema,
        "OpenApi3",
        &stored_document("OpenApi3", "v1"),
    )
    .await;

    // Each accepted document becomes the next version, in order; version 1 is the
    // create's.
    let mut expected_version = 1_u64;
    for (case, schema_type, content) in &documents {
        expected_version += 1;
        let updated = update_schema(
            &emulator,
            registry,
            schema,
            json!({"Content": content, "Type": schema_type}),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200 from UpdateSchema, got {} {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.str_at("SchemaVersion"),
            expected_version.to_string(),
            "case {case:?}: expected version {expected_version}, got {:?}",
            updated.body
        );
        assert_eq!(
            &updated.str_at("Type"),
            schema_type,
            "case {case:?}: the version carries the type it was stored under"
        );
    }

    // Every stored version still reports exactly the bytes it was minted from.
    let mut version = 1_u64;
    for (case, schema_type, content) in &documents {
        version += 1;
        let described = describe(&emulator, registry, schema, Some(&version.to_string())).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: version {version} must be readable, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            &described.str_at("Content"),
            content,
            "case {case:?}: version {version} must report its content verbatim"
        );
        assert_eq!(
            &described.str_at("Type"),
            schema_type,
            "case {case:?}: version {version} must report the type it was stored under"
        );
    }
}
