// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetSchemaByDefinition`: the read that addresses a version of a
//! schema set by the `SchemaDefinition` stored on it.
//!
//! The rules under test:
//!
//! 1. The definition is looked for "within the scope of the SchemaName or ARN (or the
//!    default registry, if none is supplied)" (`smithy_query
//!    operation=GetSchemaByDefinition`): the version reported is one of the schema set the
//!    `SchemaId` resolves to — whichever form addresses it — and the answer projects THAT
//!    version's id, status and creation instant beside the set's own ARN and `DataFormat`.
//! 2. "Otherwise, a 404 or NotFound error is returned": a schema set this endpoint holds
//!    none of and a definition no version of the resolved set carries are both refused with
//!    HTTP 400 `EntityNotFoundException`, while a `SchemaId` addressing no schema set at all
//!    is a bad parameter (`InvalidInputException`).
//! 3. "Schema versions in Deleted statuses will not be included in the results": a version
//!    a delete removed answers for its definition no longer.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the form
//! the `SchemaId` addresses the set by, the `DataFormat` the definitions are written in,
//! which version of the set the definition belongs to, and the `SchemaVersionStatus` that
//! version carries. Every call goes over the wire through the shared [`crate::wire`]
//! harness.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    DEFAULT_REGISTRY_NAME, SCHEMA_VERSION_STATUS_AVAILABLE, SCHEMA_VERSION_STATUS_FAILURE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID, REGION};

/// An Avro record, and two evolutions of it: the first adds a field carrying a default (a
/// backward-compatible change), the second one carrying none (a backward-incompatible one,
/// so the version registered for it is stored as `FAILURE`).
const BASE: &str = r#"{"type":"record","name":"User","fields":[{"name":"id","type":"string"}]}"#;
const COMPATIBLE: &str = concat!(
    r#"{"type":"record","name":"User","fields":[{"name":"id","type":"string"},"#,
    r#"{"name":"email","type":["null","string"],"default":null}]}"#
);
const INCOMPATIBLE: &str = concat!(
    r#"{"type":"record","name":"User","fields":[{"name":"id","type":"string"},"#,
    r#"{"name":"zip","type":"string"}]}"#
);

/// A JSON Schema document, for a row whose schema set is written in another `DataFormat`.
const JSON_DEFINITION: &str =
    r#"{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}"#;

/// The whole `Message` a schema-set miss is refused with.
const SCHEMA_NOT_FOUND: &str = "Schema is not found.";

/// The whole `Message` a definition no version of the resolved set carries is refused with.
const VERSION_NOT_FOUND: &str = "Schema version is not found.";

/// The ARN of schema `schema` of the default registry, as this endpoint mints it.
fn schema_arn(schema: &str) -> String {
    format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:schema/{DEFAULT_REGISTRY_NAME}/{schema}")
}

/// A string member of a response body.
fn text(case: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: {member} must be a string, got {body}"))
        .to_string()
}

/// Create the schema set `schema` in `data_format` with `definition` as its first version,
/// and report the `SchemaVersionId` of that version.
async fn create_schema(
    router: &MultiServiceRouter,
    case: &str,
    schema: &str,
    data_format: &str,
    definition: &str,
) -> String {
    let created = ok(
        router,
        "CreateSchema",
        &json!({
            "SchemaName": schema,
            "DataFormat": data_format,
            "Compatibility": "BACKWARD",
            "SchemaDefinition": definition,
        }),
    )
    .await;
    text(case, &created, "SchemaVersionId")
}

/// Register `definition` against `schema` and report the version's id and status.
async fn register(
    router: &MultiServiceRouter,
    case: &str,
    schema: &str,
    definition: &str,
) -> (String, String) {
    let registered = ok(
        router,
        "RegisterSchemaVersion",
        &json!({
            "SchemaId": { "SchemaName": schema },
            "SchemaDefinition": definition,
        }),
    )
    .await;
    (
        text(case, &registered, "SchemaVersionId"),
        text(case, &registered, "Status"),
    )
}

// ---------------------------------------------------------------------------
// What the read reports
// ---------------------------------------------------------------------------

/// Rule 1: every version a schema set holds answers for its own definition, with the status
/// that version carries — the failed one included — and the schema set's own ARN and
/// `DataFormat`.
#[tokio::test]
async fn each_definition_held_answers_with_its_own_version() {
    let router = emulator();
    let schema = "payments-by-definition";
    let first_id = create_schema(&router, "setup", schema, "AVRO", BASE).await;
    let (second_id, second_status) = register(&router, "setup", schema, COMPATIBLE).await;
    let (third_id, third_status) = register(&router, "setup", schema, INCOMPATIBLE).await;
    assert_eq!(
        (second_status.as_str(), third_status.as_str()),
        (
            SCHEMA_VERSION_STATUS_AVAILABLE,
            SCHEMA_VERSION_STATUS_FAILURE
        ),
        "case setup: expected the two registrations to differ in status, got \
         {second_status} and {third_status}"
    );

    let rows = [
        (
            "the definition the create registered",
            BASE,
            first_id.clone(),
            SCHEMA_VERSION_STATUS_AVAILABLE,
        ),
        (
            "a compatible definition registered after it",
            COMPATIBLE,
            second_id.clone(),
            SCHEMA_VERSION_STATUS_AVAILABLE,
        ),
        (
            "an incompatible definition, held as a failed version",
            INCOMPATIBLE,
            third_id.clone(),
            SCHEMA_VERSION_STATUS_FAILURE,
        ),
    ];
    for (case, definition, expected_id, expected_status) in rows {
        let read = ok(
            &router,
            "GetSchemaByDefinition",
            &json!({
                "SchemaId": { "SchemaName": schema },
                "SchemaDefinition": definition,
            }),
        )
        .await;
        assert_eq!(
            text(case, &read, "SchemaVersionId"),
            expected_id,
            "case {case}: expected the id of the version holding it, got {read}"
        );
        assert_eq!(
            text(case, &read, "Status"),
            expected_status,
            "case {case}: expected the version's own status {expected_status}, got {read}"
        );
        assert_eq!(
            text(case, &read, "DataFormat"),
            "AVRO",
            "case {case}: the format is the schema set's, got {read}"
        );
        assert_eq!(
            text(case, &read, "SchemaArn"),
            schema_arn(schema),
            "case {case}: the ARN is the schema set's, got {read}"
        );
        assert!(
            !text(case, &read, "CreatedTime").is_empty(),
            "case {case}: the version's creation instant is reported, got {read}"
        );
    }
}

/// Rule 1 over the forms a `SchemaId` may take, and over the `DataFormat` the definitions
/// are written in: each form resolves the same schema set, so each answers with the same
/// version.
#[tokio::test]
async fn every_schema_id_form_resolves_the_same_schema_set() {
    let router = emulator();
    let rows = [
        ("AVRO", BASE),
        // A format whose definitions this read never parses: the definition is matched as
        // the artefact the registration stored, whatever format it is written in.
        ("JSON", JSON_DEFINITION),
    ];
    for (data_format, definition) in rows {
        let schema = format!("orders-{}", data_format.to_lowercase());
        let case = format!("{data_format} by every SchemaId form");
        let version_id = create_schema(&router, &case, &schema, data_format, definition).await;
        let arn = schema_arn(&schema);

        let forms = [
            ("SchemaName alone", json!({ "SchemaName": schema })),
            ("SchemaArn alone", json!({ "SchemaArn": arn })),
            (
                "SchemaName with its RegistryName",
                json!({ "SchemaName": schema, "RegistryName": DEFAULT_REGISTRY_NAME }),
            ),
        ];
        for (form, schema_id) in forms {
            let case = format!("{case}, {form}");
            let read = ok(
                &router,
                "GetSchemaByDefinition",
                &json!({ "SchemaId": schema_id, "SchemaDefinition": definition }),
            )
            .await;
            assert_eq!(
                text(&case, &read, "SchemaVersionId"),
                version_id,
                "case {case}: expected the version the set holds, got {read}"
            );
            assert_eq!(
                text(&case, &read, "DataFormat"),
                data_format,
                "case {case}: expected the set's own format, got {read}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What the read refuses
// ---------------------------------------------------------------------------

/// Rule 2: the two misses and the bad parameter, including a definition another schema set
/// holds a version of — the lookup is scoped to the set the `SchemaId` resolved to.
#[tokio::test]
async fn a_definition_outside_the_resolved_schema_set_is_refused() {
    let router = emulator();
    let schema = "payments-scoped";
    let elsewhere = "payments-elsewhere";
    create_schema(&router, "setup", schema, "AVRO", BASE).await;
    create_schema(&router, "setup", elsewhere, "AVRO", COMPATIBLE).await;

    let refusals = [
        (
            "a definition only another schema set holds",
            json!({ "SchemaName": schema }),
            COMPATIBLE,
            "EntityNotFoundException",
            VERSION_NOT_FOUND,
        ),
        (
            "a definition no version holds at all",
            json!({ "SchemaName": schema }),
            INCOMPATIBLE,
            "EntityNotFoundException",
            VERSION_NOT_FOUND,
        ),
        (
            "a SchemaName no schema set has",
            json!({ "SchemaName": "absent" }),
            BASE,
            "EntityNotFoundException",
            SCHEMA_NOT_FOUND,
        ),
        (
            "a SchemaArn of another account",
            json!({
                "SchemaArn":
                    format!("arn:aws:glue:{REGION}:210987654321:schema/{DEFAULT_REGISTRY_NAME}/{schema}"),
            }),
            BASE,
            "EntityNotFoundException",
            SCHEMA_NOT_FOUND,
        ),
        (
            "a SchemaId addressing no schema set",
            json!({ "RegistryName": DEFAULT_REGISTRY_NAME }),
            BASE,
            "InvalidInputException",
            "One of SchemaArn or SchemaName has to be provided.",
        ),
    ];
    for (case, schema_id, definition, expected_code, expected_message) in refusals {
        let (status, refused) = call(
            &router,
            "GetSchemaByDefinition",
            &json!({ "SchemaId": schema_id, "SchemaDefinition": definition }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {refused}"
        );
        assert!(
            error_code(&refused).contains(expected_code),
            "case {case}: expected {expected_code}, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            expected_message,
            "case {case}: expected the message {expected_message:?}, got {refused}"
        );
    }
}

/// Rule 3: a version a delete removed answers for its definition no longer, while the
/// versions left in place still do.
#[tokio::test]
async fn a_deleted_version_no_longer_answers_for_its_definition() {
    let router = emulator();
    let case = "a deleted version";
    let schema = "payments-deleted";
    create_schema(&router, case, schema, "AVRO", BASE).await;
    let (second_id, _) = register(&router, case, schema, COMPATIBLE).await;

    ok(
        &router,
        "DeleteSchemaVersions",
        &json!({ "SchemaId": { "SchemaName": schema }, "Versions": "2" }),
    )
    .await;

    let (status, refused) = call(
        &router,
        "GetSchemaByDefinition",
        &json!({ "SchemaId": { "SchemaName": schema }, "SchemaDefinition": COMPATIBLE }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case}: expected 400 once version {second_id} is gone, got {status} with \
         {refused}"
    );
    assert_eq!(
        error_message(&refused),
        VERSION_NOT_FOUND,
        "case {case}: expected the version miss, got {refused}"
    );

    let read = ok(
        &router,
        "GetSchemaByDefinition",
        &json!({ "SchemaId": { "SchemaName": schema }, "SchemaDefinition": BASE }),
    )
    .await;
    assert_eq!(
        text(case, &read, "Status"),
        SCHEMA_VERSION_STATUS_AVAILABLE,
        "case {case}: the version left in place still answers, got {read}"
    );
}
