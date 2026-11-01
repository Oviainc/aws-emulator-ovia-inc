// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the four rules this service owes beyond the plain
//! create/read lifecycle:
//!
//! 1. a registry is deleted only once nothing lives in it;
//! 2. the service-owned (`AWS`-scoped) schema catalogue every account sees, and the
//!    JSON Schema draft-4 export of it;
//! 3. the sentence a duplicate create is refused with;
//! 4. a schema version is minted per `UpdateSchema` that carries `Content`, uniquely,
//!    even when the calls race.
//!
//! Every case drives the real wire path, through the harness in
//! `registry_schema_lifecycle.rs` — nothing here touches the state struct directly.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::state::{
    aws_managed_schemas, ManagedSchema, AWS_MANAGED_REGISTRY, AWS_MANAGED_SCHEMA_TYPE,
    AWS_SERVICE_EVENT_TYPES, EVENT_ENVELOPE_COMPONENT,
};

use crate::registry_schema_lifecycle::{
    create_registry, export_uri, listed_versions, registry_names, registry_uri, schema_names,
    schema_uri, versions_uri, walk, Emulator,
};

/// A schema body `CreateSchema` accepts, distinct per `marker`.
fn schema_body(marker: &str) -> Value {
    json!({
        "Content": format!(r#"{{"openapi":"3.0.0","info":{{"title":"{marker}","version":"1"}}}}"#),
        "Type": "OpenApi3",
    })
}

/// RULE: a registry is a container, so deleting one is refused 409
/// `ConflictException` for as long as it still holds a schema; the refusal changes
/// nothing, and the identical call succeeds once the last schema is gone.
#[tokio::test]
async fn deleting_a_registry_that_still_holds_a_schema_is_refused() {
    // Rows: what the registry holds when the delete is attempted — one schema,
    // several, and one schema carrying several versions. The rule generalises over
    // the COUNT of schemas and versions, not over their names.
    let cases: Vec<(&str, Vec<&str>, usize)> = vec![
        ("one schema", vec!["only"], 1),
        ("several schemas", vec!["alpha", "beta", "gamma"], 1),
        ("one schema, several versions", vec!["versioned"], 3),
    ];

    for (case, schemas, versions) in cases {
        let emulator = Emulator::new();
        let registry = "holder";
        create_registry(&emulator, registry).await;
        for schema in &schemas {
            let created = emulator
                .call("POST", &schema_uri(registry, schema), Some(schema_body(schema)))
                .await;
            assert_eq!(
                created.status, 201,
                "case {case:?}: expected 201 from CreateSchema, got {} {:?}",
                created.status, created.body
            );
            for round in 1..versions {
                let updated = emulator
                    .call(
                        "PUT",
                        &schema_uri(registry, schema),
                        Some(schema_body(&format!("{schema}-{round}"))),
                    )
                    .await;
                assert_eq!(
                    updated.status, 200,
                    "case {case:?}: expected 200 from UpdateSchema, got {} {:?}",
                    updated.status, updated.body
                );
            }
        }

        let refused = emulator.call("DELETE", &registry_uri(registry), None).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected 409, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {:?}",
            refused.body
        );

        // The refusal changed nothing: the registry and every schema in it are still
        // there, with every version.
        let described = emulator.call("GET", &registry_uri(registry), None).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the registry must survive the refusal, got {} {:?}",
            described.status, described.body
        );
        let listed = emulator
            .call("GET", &format!("{}/schemas", registry_uri(registry)), None)
            .await;
        let mut expected: Vec<String> = schemas.iter().map(|s| (*s).to_string()).collect();
        expected.sort();
        assert_eq!(
            schema_names(&listed),
            expected,
            "case {case:?}: every schema must survive the refusal"
        );
        for schema in &schemas {
            let versions_listed = emulator
                .call("GET", &versions_uri(registry, schema), None)
                .await;
            assert_eq!(
                listed_versions(&versions_listed).len(),
                versions,
                "case {case:?}: schema {schema:?} must keep all {versions} versions, got {:?}",
                versions_listed.body
            );
        }

        // Emptying the registry makes the identical call succeed.
        for schema in &schemas {
            let deleted = emulator
                .call("DELETE", &schema_uri(registry, schema), None)
                .await;
            assert_eq!(
                deleted.status, 204,
                "case {case:?}: expected 204 from DeleteSchema, got {} {:?}",
                deleted.status, deleted.body
            );
        }
        let deleted = emulator.call("DELETE", &registry_uri(registry), None).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: an empty registry is deleted, got {} {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            emulator.call("GET", &registry_uri(registry), None).await.status,
            404,
            "case {case:?}: the deleted registry is gone from every read"
        );
    }
}

/// RULE: a refused duplicate create answers the sentence AWS builds from the name the
/// request sent — the registry's for a registry, the schema's ALONE for a schema — in
/// the service's conflict class (409), which is the class a resource that is already
/// there takes whichever create asked for it.
#[tokio::test]
async fn a_duplicate_create_answers_the_name_it_was_sent() {
    // Rows: names differing in shape, length and alphabet; the message template is
    // the rule, the name is whatever the request sent.
    for name in [
        "dup",
        "sim2real-schemas-reg-2a4bf69b3646",
        "order.placed_v1",
        "UPPER-and-lower-99",
        &"n".repeat(64),
    ] {
        let emulator = Emulator::new();
        create_registry(&emulator, name).await;

        let again = emulator
            .call("POST", &registry_uri(name), Some(json!({})))
            .await;
        assert_eq!(
            again.status, 409,
            "case {name:?}: expected 409, got {} {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "ConflictException",
            "case {name:?}: expected ConflictException, got {:?}",
            again.body
        );
        assert_eq!(
            again.str_at("Message"),
            format!("Registry with name {name} already exists."),
            "case {name:?}: wrong registry-conflict sentence"
        );

        let created = emulator
            .call("POST", &schema_uri(name, name), Some(schema_body(name)))
            .await;
        assert_eq!(
            created.status, 201,
            "case {name:?}: expected 201 from CreateSchema, got {} {:?}",
            created.status, created.body
        );
        let recreated = emulator
            .call("POST", &schema_uri(name, name), Some(schema_body("other")))
            .await;
        assert_eq!(
            recreated.status, 409,
            "case {name:?}: expected 409, got {} {:?}",
            recreated.status, recreated.body
        );
        assert_eq!(
            recreated.error_code(),
            "ConflictException",
            "case {name:?}: expected ConflictException, got {:?}",
            recreated.body
        );
        assert_eq!(
            recreated.str_at("Message"),
            format!("Schema with name {name} already exists."),
            "case {name:?}: the schema-conflict sentence names the schema alone"
        );
    }
}

/// RULE: every account sees the service-owned registries through
/// `ListRegistries(Scope='AWS')`, and the AWS event schemas they hold are readable
/// like any other schema.
#[tokio::test]
async fn the_aws_managed_catalogue_is_visible_to_every_account() {
    let emulator = Emulator::new();

    let listed = emulator
        .call("GET", "/v1/registries?scope=AWS", None)
        .await;
    assert_eq!(
        listed.status, 200,
        "expected 200, got {} {:?}",
        listed.status, listed.body
    );
    let visible = registry_names(&listed);
    assert!(
        visible.contains(&AWS_MANAGED_REGISTRY.to_string()),
        "an account must see the AWS-managed registry, got {visible:?}"
    );

    // Source: `crate::state::aws_managed_schemas`, the catalogue the store seeds, in
    // the codepoint order of the schema names a listing serves
    // (`crate::state::in_schema_name_order`).
    let catalogue = aws_managed_schemas();
    let mut expected: Vec<String> = catalogue.iter().map(|entry| entry.name.clone()).collect();
    expected.sort();
    let walked = walk(
        &emulator,
        &format!("{}/schemas", registry_uri(AWS_MANAGED_REGISTRY)),
        None,
        schema_names,
    )
    .await;
    assert_eq!(
        walked, expected,
        "ListSchemas must page through exactly the catalogue's schemas"
    );

    // Rows: one entry per service-specific event type, plus the CloudTrail event type
    // of a source that publishes nothing else — they differ in how many components
    // the document carries and how deeply those reference each other.
    for entry in sample_of(catalogue) {
        let schema = &entry.name;
        let described = emulator
            .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, schema), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {schema:?}: expected 200, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.str_at("Content"),
            entry.content,
            "case {schema:?}: DescribeSchema reports the catalogue document verbatim"
        );
        assert_eq!(
            described.str_at("Type"),
            AWS_MANAGED_SCHEMA_TYPE,
            "case {schema:?}: AWS stores its event schemas as {AWS_MANAGED_SCHEMA_TYPE}"
        );
    }
}

/// The catalogue entries the cases below drive: every service-specific event type,
/// plus the CloudTrail event type of two sources the issue did not cite.
///
/// The whole catalogue holds one schema per AWS event source, so a case that walks it
/// end to end walks a few hundred requests; these rows are what the rule generalises
/// over (the number of components a document carries, and whether the document was
/// rendered for a service-specific event or for the API-call event every source
/// publishes).
fn sample_of(catalogue: &[ManagedSchema]) -> Vec<ManagedSchema> {
    let sampled: Vec<String> = AWS_SERVICE_EVENT_TYPES
        .iter()
        .map(|event| event.schema_name())
        .chain([
            "aws.rds@AWSAPICallviaCloudTrail".to_string(),
            "aws.sqs@AWSAPICallviaCloudTrail".to_string(),
        ])
        .collect();
    let chosen: Vec<ManagedSchema> = catalogue
        .iter()
        .filter(|entry| sampled.contains(&entry.name))
        .cloned()
        .collect();
    assert_eq!(
        chosen.len(),
        sampled.len(),
        "every sampled name must be in the catalogue: wanted {sampled:?}, got {:?}",
        chosen.iter().map(|e| &e.name).collect::<Vec<_>>()
    );
    chosen
}

/// RULE: `ExportSchema(Type='JSONSchemaDraft4')` renders an AWS-managed OpenAPI 3
/// schema as one JSON Schema draft-4 document computed from the stored one — draft-04
/// named in `$schema`, the source document's title carried over, the components under
/// `definitions`, and every component `$ref` pointing into them.
#[tokio::test]
async fn exporting_an_aws_managed_schema_converts_it_to_draft_4() {
    let emulator = Emulator::new();

    // Rows: every service-specific schema the catalogue holds, plus two API-call
    // schemas — they differ in how many components they carry and how deeply those
    // reference each other.
    for entry in sample_of(aws_managed_schemas()) {
        let schema = &entry.name;
        let exported = emulator
            .call(
                "GET",
                &export_uri(AWS_MANAGED_REGISTRY, schema, "JSONSchemaDraft4"),
                None,
            )
            .await;
        assert_eq!(
            exported.status, 200,
            "case {schema:?}: expected 200, got {} {:?}",
            exported.status, exported.body
        );
        assert_eq!(
            exported.str_at("Type"),
            "JSONSchemaDraft4",
            "case {schema:?}: the export reports the type it produced"
        );
        assert_eq!(
            exported.str_at("SchemaVersion"),
            entry.version_count.to_string(),
            "case {schema:?}: the export answers the catalogue's newest version"
        );
        assert_eq!(
            exported.str_at("SchemaName"),
            *schema,
            "case {schema:?}: the export names the schema"
        );

        let content: Value = serde_json::from_str(&exported.str_at("Content"))
            .unwrap_or_else(|e| panic!("case {schema:?}: Content must be JSON: {e}"));
        let declared = content["$schema"].as_str().unwrap_or_default();
        assert!(
            declared.contains("draft-04"),
            "case {schema:?}: $schema must name draft-04, got {declared:?}"
        );
        assert!(
            !serde_json::to_string(&content)
                .unwrap_or_default()
                .contains("#/components/schemas/"),
            "case {schema:?}: no OpenAPI component ref may survive the conversion: {content:?}"
        );

        // EVERY component of the source document is a definition of the export, and the
        // refs between them follow. A published document declares no event envelope, so
        // it names no root among its co-equal components and the export promotes none.
        let source: Value = serde_json::from_str(&entry.content)
            .unwrap_or_else(|e| panic!("case {schema:?}: the source must be JSON: {e}"));
        let components = source["components"]["schemas"]
            .as_object()
            .unwrap_or_else(|| panic!("case {schema:?}: the source has components"));
        for name in components.keys() {
            assert!(
                content["definitions"][name].is_object(),
                "case {schema:?}: component {name:?} must be a definition, got {content:?}"
            );
        }
        assert!(
            source["components"]["schemas"][EVENT_ENVELOPE_COMPONENT].is_null(),
            "case {schema:?}: a published document declares no envelope, got {source:?}"
        );
        assert!(
            content["type"].is_null() && content["properties"].is_null(),
            "case {schema:?}: no component is promoted where the document names no root, \
             got {content:?}"
        );
        let event_name = schema
            .split_once('@')
            .map(|(_, event_name)| event_name)
            .unwrap_or_else(|| panic!("case {schema:?}: is <source>@<EventName>"));
        assert!(
            content["definitions"][event_name].is_object(),
            "case {schema:?}: the event's own component must be a definition, got {content:?}"
        );
        assert_eq!(
            content["title"], source["info"]["title"],
            "case {schema:?}: the export carries the source document's title"
        );
    }
}

/// RULE: `ExportSchema` refuses what it cannot serve — a `Type` that is not the one
/// export target the service renders is refused 400 on the request, a schema the service
/// did not itself write is refused 403, and a resource nothing holds is refused 404.
/// (The exports that ARE served are the subject of
/// `tests/all/export_reads_the_stored_schema.rs`.)
#[tokio::test]
async fn exporting_what_cannot_be_exported_is_refused() {
    let emulator = Emulator::new();
    create_registry(&emulator, "mine").await;
    // A schema a caller wrote, in the language the conversion PRODUCES: the service did
    // not write it, so it has no export whichever language is asked for.
    let stored_as_draft4 = emulator
        .call(
            "POST",
            &schema_uri("mine", "already-draft4"),
            Some(json!({
                "Content": r#"{"$schema":"http://json-schema.org/draft-04/schema#","type":"object"}"#,
                "Type": "JSONSchemaDraft4",
            })),
        )
        .await;
    assert_eq!(
        stored_as_draft4.status, 201,
        "expected 201 from CreateSchema, got {} {:?}",
        stored_as_draft4.status, stored_as_draft4.body
    );
    let managed_registry = AWS_MANAGED_REGISTRY;
    // Any member of the catalogue serves: these refusals are about the request, not
    // about which schema it names.
    let catalogue = aws_managed_schemas();
    let managed_schema_name = catalogue
        .first()
        .expect("the catalogue holds schemas")
        .name
        .clone();
    let managed_schema = managed_schema_name.as_str();

    // Rows: a `Type` that is no export target (400 on the request, whichever schema is
    // named), a schema the service did not write (403), and a resource that does not
    // exist (404).
    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "a schema a caller wrote, asked for as the one export target",
            export_uri("mine", "already-draft4", "JSONSchemaDraft4"),
            403,
            "ForbiddenException",
        ),
        (
            "a schema a caller wrote, asked for as a language that is no target",
            export_uri("mine", "already-draft4", "OpenApi3"),
            400,
            "BadRequestException",
        ),
        (
            "the language the named schema is stored in, which is no export target",
            export_uri(managed_registry, managed_schema, "OpenApi3"),
            400,
            "BadRequestException",
        ),
        (
            "a type that is not a schema language",
            export_uri(managed_registry, managed_schema, "Avro"),
            400,
            "BadRequestException",
        ),
        (
            "an empty type",
            export_uri(managed_registry, managed_schema, ""),
            400,
            "BadRequestException",
        ),
        (
            "a type differing only in case",
            export_uri(managed_registry, managed_schema, "jsonschemadraft4"),
            400,
            "BadRequestException",
        ),
        (
            "a schema the registry does not hold",
            export_uri(managed_registry, "aws.nothing@Nothing", "JSONSchemaDraft4"),
            404,
            "NotFoundException",
        ),
        (
            "a version the schema does not hold",
            format!(
                "{}&schemaVersion=9",
                export_uri(managed_registry, managed_schema, "JSONSchemaDraft4")
            ),
            404,
            "NotFoundException",
        ),
    ];

    for (case, uri, status, code) in cases {
        let answer = emulator.call("GET", &uri, None).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected {status}, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: expected {code}, got {:?}",
            answer.body
        );
    }
}

/// RULE: AWS owns what is in a service-owned registry, so every caller write into one
/// is refused 403 `ForbiddenException` and the catalogue is unchanged.
#[tokio::test]
async fn a_caller_cannot_write_into_an_aws_managed_registry() {
    let emulator = Emulator::new();
    let registry = AWS_MANAGED_REGISTRY;
    let entry = aws_managed_schemas()
        .first()
        .expect("the catalogue holds schemas");
    let schema = entry.name.as_str();
    let content = entry.content.as_str();

    // Rows: every write this slice implements, against a service-owned registry.
    let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "CreateSchema",
            "POST",
            schema_uri(registry, "mine"),
            Some(schema_body("mine")),
        ),
        (
            "UpdateSchema",
            "PUT",
            schema_uri(registry, schema),
            Some(schema_body("rewritten")),
        ),
        ("DeleteSchema", "DELETE", schema_uri(registry, schema), None),
        ("DeleteRegistry", "DELETE", registry_uri(registry), None),
    ];

    for (case, method, uri, body) in cases {
        let answer = emulator.call(method, &uri, body).await;
        assert_eq!(
            answer.status, 403,
            "case {case:?}: expected 403, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ForbiddenException",
            "case {case:?}: expected ForbiddenException, got {:?}",
            answer.body
        );
    }

    // And nothing in the catalogue moved.
    let described = emulator.call("GET", &schema_uri(registry, schema), None).await;
    assert_eq!(
        described.str_at("Content"),
        content,
        "a refused write must leave the catalogue document as it was"
    );
    assert_eq!(
        described.str_at("SchemaVersion"),
        "1",
        "a refused write must not mint a version"
    );
    assert!(
        !schema_names(
            &emulator
                .call(
                    "GET",
                    &format!("{}/schemas?schemaNamePrefix=mine", registry_uri(registry)),
                    None
                )
                .await
        )
        .contains(&"mine".to_string()),
        "a refused create must have stored nothing"
    );
}

/// One `UpdateSchema` row.
#[derive(Debug)]
struct UpdateCase {
    case: &'static str,
    body: Value,
    /// Whether this call writes a new definition, and so mints a version.
    mints: bool,
    /// The content a read of the resulting latest version must report.
    expected_content: &'static str,
    expected_type: &'static str,
}

/// RULE: `UpdateSchema` mints one new, immutable version per call that carries
/// `Content` — numbered one past the highest ever minted — and mints none for a call
/// that only changes metadata; every earlier version keeps the content it was written
/// with.
#[tokio::test]
async fn updating_a_schema_mints_one_version_per_new_definition() {
    let emulator = Emulator::new();
    let registry = "versioned";
    let schema = "evolving";
    create_registry(&emulator, registry).await;
    let created = emulator
        .call("POST", &schema_uri(registry, schema), Some(schema_body("v1")))
        .await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateSchema, got {} {:?}",
        created.status, created.body
    );
    // `CreateSchemaOutput` carries no `Content`, so version 1's content is the one
    // the create sent — which `DescribeSchema` is asserted to report below.
    let first_content = schema_body("v1")["Content"]
        .as_str()
        .expect("the create body carries Content")
        .to_string();

    // Rows: what the update sends — content alone, content with a description,
    // content with another modelled `Type`, a description alone (no new definition),
    // and an empty request (nothing to apply).
    let cases = vec![
        UpdateCase {
            case: "content only",
            body: json!({"Content": r#"{"openapi":"3.0.1"}"#}),
            mints: true,
            expected_content: r#"{"openapi":"3.0.1"}"#,
            expected_type: "OpenApi3",
        },
        UpdateCase {
            case: "content and description",
            body: json!({"Content": r#"{"openapi":"3.0.2"}"#, "Description": "the third cut"}),
            mints: true,
            expected_content: r#"{"openapi":"3.0.2"}"#,
            expected_type: "OpenApi3",
        },
        UpdateCase {
            case: "content with another modelled type",
            body: json!({
                "Content": r#"{"$schema":"http://json-schema.org/draft-04/schema#"}"#,
                "Type": "JSONSchemaDraft4",
            }),
            mints: true,
            expected_content: r#"{"$schema":"http://json-schema.org/draft-04/schema#"}"#,
            expected_type: "JSONSchemaDraft4",
        },
        UpdateCase {
            case: "description only",
            body: json!({"Description": "renamed, not rewritten"}),
            mints: false,
            expected_content: r#"{"$schema":"http://json-schema.org/draft-04/schema#"}"#,
            expected_type: "JSONSchemaDraft4",
        },
        UpdateCase {
            case: "nothing to apply",
            body: json!({}),
            mints: false,
            expected_content: r#"{"$schema":"http://json-schema.org/draft-04/schema#"}"#,
            expected_type: "JSONSchemaDraft4",
        },
    ];

    let mut latest: u64 = 1;
    // What each minted version must still report at the end: version -> content.
    let mut written: Vec<(u64, String)> = vec![(1, first_content)];

    for case in cases {
        let updated = emulator
            .call("PUT", &schema_uri(registry, schema), Some(case.body.clone()))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.case, updated.status, updated.body
        );
        if case.mints {
            latest += 1;
            written.push((latest, case.expected_content.to_string()));
        }
        assert_eq!(
            updated.str_at("SchemaVersion"),
            latest.to_string(),
            "case {:?}: wrong version minted, got {:?}",
            case.case,
            updated.body["SchemaVersion"]
        );
        assert_eq!(
            updated.str_at("Type"),
            case.expected_type,
            "case {:?}: the version's type is the type of its content",
            case.case
        );

        // The read-back: the latest version is what the update wrote.
        let described = emulator.call("GET", &schema_uri(registry, schema), None).await;
        assert_eq!(
            described.str_at("Content"),
            case.expected_content,
            "case {:?}: DescribeSchema must report the newest definition",
            case.case
        );
        assert_eq!(
            described.str_at("SchemaVersion"),
            latest.to_string(),
            "case {:?}: the latest version is the one just minted",
            case.case
        );
        assert_eq!(
            described.str_at("SchemaArn"),
            created.str_at("SchemaArn"),
            "case {:?}: an update does not re-mint the schema's ARN",
            case.case
        );
    }

    // Every version ever minted is still readable, with the content it was written
    // with: a version is immutable.
    for (version, content) in &written {
        let pinned = emulator
            .call(
                "GET",
                &format!("{}?schemaVersion={version}", schema_uri(registry, schema)),
                None,
            )
            .await;
        assert_eq!(
            pinned.status, 200,
            "case version={version}: expected 200, got {} {:?}",
            pinned.status, pinned.body
        );
        assert_eq!(
            &pinned.str_at("Content"),
            content,
            "case version={version}: an earlier version keeps its own content"
        );
    }

    // And the listing reports exactly those versions.
    let listed = emulator
        .call("GET", &versions_uri(registry, schema), None)
        .await;
    assert_eq!(
        listed.status, 200,
        "expected 200 from ListSchemaVersions, got {} {:?}",
        listed.status, listed.body
    );
    let expected: BTreeSet<String> = written
        .iter()
        .map(|(version, _)| version.to_string())
        .collect();
    assert_eq!(
        listed_versions(&listed),
        expected,
        "ListSchemaVersions must report exactly the minted versions, got {:?}",
        listed.body
    );
}

/// RULE (an invariant): racing `UpdateSchema` calls on one schema are serialised — at
/// least one wins, no two winners share a version, and afterwards
/// `ListSchemaVersions` holds version 1 plus exactly one entry per winner, each
/// readable with the content its own call sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_updates_mint_unique_versions() {
    // Rows: how many writers race. The invariant is about concurrency, so it is
    // asserted at several widths, including one (no race at all).
    for racers in [1_usize, 2, 5, 8] {
        let emulator = Arc::new(Emulator::new());
        let registry = "raced";
        let schema = "contended";
        create_registry(&emulator, registry).await;
        let created = emulator
            .call("POST", &schema_uri(registry, schema), Some(schema_body("v1")))
            .await;
        assert_eq!(
            created.status, 201,
            "case racers={racers}: expected 201, got {} {:?}",
            created.status, created.body
        );

        let mut racing = Vec::new();
        for racer in 0..racers {
            let emulator = Arc::clone(&emulator);
            let uri = schema_uri(registry, schema);
            let content = format!(r#"{{"openapi":"3.0.0","x-racer":{racer}}}"#);
            racing.push(tokio::spawn(async move {
                let answer = emulator
                    .call("PUT", &uri, Some(json!({"Content": content.clone()})))
                    .await;
                (content, answer)
            }));
        }

        let mut winners: Vec<(String, String)> = Vec::new();
        for racing in racing {
            let (content, answer) = racing.await.expect("a racing update ran to completion");
            if answer.status == 200 {
                winners.push((answer.str_at("SchemaVersion"), content));
                continue;
            }
            // A loser is refused as a conflict, never with a 5xx or a 501.
            assert_eq!(
                answer.status, 409,
                "case racers={racers}: a losing update must be refused 409, got {} {:?}",
                answer.status, answer.body
            );
        }

        assert!(
            !winners.is_empty(),
            "case racers={racers}: at least one concurrent update must win"
        );
        let distinct: BTreeSet<&String> = winners.iter().map(|(version, _)| version).collect();
        assert_eq!(
            distinct.len(),
            winners.len(),
            "case racers={racers}: no two winners may share a version, got {winners:?}"
        );

        // The store agrees: version 1 plus one per winner, and each winner's content
        // is readable at the version it was told.
        let listed = emulator
            .call("GET", &versions_uri(registry, schema), None)
            .await;
        let mut expected: BTreeSet<String> = distinct.into_iter().cloned().collect();
        expected.insert("1".to_string());
        assert_eq!(
            listed_versions(&listed),
            expected,
            "case racers={racers}: ListSchemaVersions must hold version 1 plus one per winner, \
             got {:?}",
            listed.body
        );
        for (version, content) in &winners {
            let pinned = emulator
                .call(
                    "GET",
                    &format!("{}?schemaVersion={version}", schema_uri(registry, schema)),
                    None,
                )
                .await;
            assert_eq!(
                &pinned.str_at("Content"),
                content,
                "case racers={racers}: version {version} must hold the content its own call sent"
            );
        }
    }
}

/// RULE: a description is the schema's own metadata, so changing it alone mints no
/// version, is reported by the next read, and leaves every version's CONTENT as it
/// was written.
#[tokio::test]
async fn changing_only_the_description_is_visible_and_mints_nothing() {
    let emulator = Emulator::new();
    let registry = "described";
    let schema = "doc";
    create_registry(&emulator, registry).await;
    let created = emulator
        .call(
            "POST",
            &schema_uri(registry, schema),
            Some(json!({"Content": r#"{"openapi":"3.0.0"}"#, "Type": "OpenApi3", "Description": "as created"})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "expected 201, got {} {:?}",
        created.status, created.body
    );

    // Rows: descriptions differing in content, emptiness and alphabet; each is read
    // back by the next DescribeSchema, and none mints a version.
    for description in ["rewritten", "", "caf\u{e9} \u{2014} \u{30b9}\u{30ad}\u{30fc}\u{30de}", &"d".repeat(256)] {
        let updated = emulator
            .call(
                "PUT",
                &schema_uri(registry, schema),
                Some(json!({"Description": description})),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {description:?}: expected 200, got {} {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.str_at("SchemaVersion"),
            "1",
            "case {description:?}: a metadata change mints no version"
        );

        let described = emulator.call("GET", &schema_uri(registry, schema), None).await;
        assert_eq!(
            described.str_at("Description"),
            description,
            "case {description:?}: the next read must report the new description"
        );
        assert_eq!(
            described.str_at("Content"),
            r#"{"openapi":"3.0.0"}"#,
            "case {description:?}: a metadata change must not touch the stored content"
        );
        assert_eq!(
            listed_versions(
                &emulator
                    .call("GET", &versions_uri(registry, schema), None)
                    .await
            ),
            BTreeSet::from(["1".to_string()]),
            "case {description:?}: the version list is unchanged"
        );
    }
}

/// RULE: `ClientTokenId` is an idempotency token, so a retry carrying the token of a
/// call that already minted a version answers that same version and writes nothing;
/// a call with a new token mints a new one.
#[tokio::test]
async fn a_retried_update_carrying_the_same_client_token_mints_once() {
    let emulator = Emulator::new();
    let registry = "idempotent";
    let schema = "once";
    create_registry(&emulator, registry).await;
    let created = emulator
        .call("POST", &schema_uri(registry, schema), Some(schema_body("v1")))
        .await;
    assert_eq!(
        created.status, 201,
        "expected 201, got {} {:?}",
        created.status, created.body
    );

    // Rows: token shapes a client sends — a plain word and the UUID an SDK generates;
    // each is a DIFFERENT token, so each mints exactly once however often it is sent.
    let mut minted: BTreeSet<String> = BTreeSet::from(["1".to_string()]);
    for (round, token) in ["retry-token", "11111111-2222-3333-4444-555555555555"]
        .into_iter()
        .enumerate()
    {
        let content = format!(r#"{{"openapi":"3.0.0","x-round":{round}}}"#);
        let first = emulator
            .call(
                "PUT",
                &schema_uri(registry, schema),
                Some(json!({"Content": content.clone(), "ClientTokenId": token})),
            )
            .await;
        assert_eq!(
            first.status, 200,
            "case {token:?}: expected 200, got {} {:?}",
            first.status, first.body
        );
        let version = first.str_at("SchemaVersion");
        assert!(
            minted.insert(version.clone()),
            "case {token:?}: a new token must mint a new version, got {version:?} among {minted:?}"
        );

        // The retry: same token, DIFFERENT content. It answers the version the first
        // attempt minted, and the store still holds that attempt's content.
        let retried = emulator
            .call(
                "PUT",
                &schema_uri(registry, schema),
                Some(json!({
                    "Content": r#"{"openapi":"3.0.0","x-retry":true}"#,
                    "ClientTokenId": token,
                })),
            )
            .await;
        assert_eq!(
            retried.status, 200,
            "case {token:?}: expected 200 from the retry, got {} {:?}",
            retried.status, retried.body
        );
        assert_eq!(
            retried.str_at("SchemaVersion"),
            version,
            "case {token:?}: a retry must answer the version its first attempt minted"
        );
        let pinned = emulator
            .call(
                "GET",
                &format!("{}?schemaVersion={version}", schema_uri(registry, schema)),
                None,
            )
            .await;
        assert_eq!(
            pinned.str_at("Content"),
            content,
            "case {token:?}: a retry must not rewrite the version it de-duplicated"
        );
        let listed = emulator
            .call("GET", &versions_uri(registry, schema), None)
            .await;
        assert_eq!(
            listed_versions(&listed),
            minted,
            "case {token:?}: a retry must not add a version, got {:?}",
            listed.body
        );
    }
}

/// RULE: `Type` names the type of the content, so it cannot be changed by a request
/// that sends no content.
#[tokio::test]
async fn retyping_a_schema_without_content_is_refused() {
    let emulator = Emulator::new();
    create_registry(&emulator, "typed").await;
    let created = emulator
        .call("POST", &schema_uri("typed", "doc"), Some(schema_body("v1")))
        .await;
    assert_eq!(
        created.status, 201,
        "expected 201, got {} {:?}",
        created.status, created.body
    );

    let refused = emulator
        .call(
            "PUT",
            &schema_uri("typed", "doc"),
            Some(json!({"Type": "JSONSchemaDraft4"})),
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "expected 400, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "BadRequestException",
        "expected BadRequestException, got {:?}",
        refused.body
    );

    // Re-sending the type the schema already has changes nothing and is not refused.
    let accepted = emulator
        .call(
            "PUT",
            &schema_uri("typed", "doc"),
            Some(json!({"Type": "OpenApi3"})),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "the type it already has is not a change, got {} {:?}",
        accepted.status, accepted.body
    );
    assert_eq!(
        accepted.str_at("SchemaVersion"),
        "1",
        "a request that applies nothing mints no version"
    );
}

/// RULE: an update naming a resource the store does not hold answers 404.
#[tokio::test]
async fn updating_an_absent_schema_is_a_404() {
    let emulator = Emulator::new();
    create_registry(&emulator, "present").await;

    for (case, uri) in [
        ("absent registry", schema_uri("ghost", "any")),
        ("absent schema", schema_uri("present", "ghost")),
    ] {
        for (reader, method) in [("UpdateSchema", "PUT"), ("ListSchemaVersions", "GET")] {
            let uri = if method == "GET" {
                format!("{uri}/versions")
            } else {
                uri.clone()
            };
            let body = (method == "PUT").then(|| schema_body("x"));
            let answer = emulator.call(method, &uri, body).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?} / {reader}: expected 404, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "NotFoundException",
                "case {case:?} / {reader}: expected NotFoundException, got {:?}",
                answer.body
            );
        }
    }
}
