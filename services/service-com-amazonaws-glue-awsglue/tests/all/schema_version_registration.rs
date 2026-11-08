// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `RegisterSchemaVersion` and the two reads that observe what it
//! registers (`GetSchemaVersion`, `ListSchemaVersions`).
//!
//! The rules under test:
//!
//! 1. A registration resolves its `SchemaId` to a stored schema set and is decided by the
//!    `Compatibility` mode recorded on THAT set: `DISABLED` "restricts any additional schema
//!    versions from being added after the first schema version" (`smithy_query
//!    operation=CreateSchema`), so such a registration is refused with HTTP 400
//!    `InvalidInputException` and the set's versions are left exactly as they were; under
//!    every other mode the version is minted — a fresh `SchemaVersionId`, the set's next
//!    `VersionNumber`, status `AVAILABLE` — and is readable afterwards.
//! 2. "If the same schema definition is already stored in Schema Registry as a version, the
//!    schema ID of the existing schema is returned to the caller"
//!    (`smithy_query operation=RegisterSchemaVersion`): such a call registers nothing and
//!    reports the version already held.
//! 3. "This API will not create a new schema set": a `SchemaId` resolving to no schema set
//!    of this endpoint's estate is refused with `EntityNotFoundException`, and a `SchemaId`
//!    that addresses no schema set at all is a bad parameter (`InvalidInputException`).
//! 4. A version is addressed either by its own `SchemaVersionId` or by a `SchemaId` plus the
//!    `SchemaVersionNumber` saying which version is meant
//!    (`GetSchemaVersionInput$SchemaVersionId`), and the listing reports the versions a
//!    schema set holds in registration order, one `MaxResults`-sized page at a time.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! `Compatibility` mode the schema set was created with (every value the model admits),
//! which `DataFormat` its definitions are written in, the form the `SchemaId` addresses the
//! set by, how many versions the set already holds, and which member addresses a version on
//! a read. The recorded workflow's own case — an evolved Avro definition registered against
//! a `DISABLED` schema addressed by `SchemaArn` — is one row among them. Every call goes
//! over the wire through the shared [`crate::wire`] harness.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    DEFAULT_REGISTRY_NAME, FIRST_SCHEMA_VERSION, SCHEMA_VERSION_STATUS_AVAILABLE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID};

/// The Region every request from this harness is scoped to, which is the Region the ARNs a
/// response carries are minted for.
const REGION: &str = "us-east-1";

/// The `Compatibility` mode that admits no version beyond the first.
const DISABLED: &str = "DISABLED";

/// An Avro record schema, and the same record with an optional field added — the shape the
/// recorded workflow evolves.
const AVRO_BASE: &str =
    r#"{"type":"record","name":"User","fields":[{"name":"id","type":"string"}]}"#;
const AVRO_EVOLVED: &str = concat!(
    r#"{"type":"record","name":"User","fields":[{"name":"id","type":"string"},"#,
    r#"{"name":"email","type":["null","string"],"default":null}]}"#
);

/// A JSON Schema document and an evolution of it, for the `JSON` data format.
const JSON_BASE: &str =
    r#"{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}"#;
const JSON_EVOLVED: &str = concat!(
    r#"{"type":"object","properties":{"id":{"type":"string"},"#,
    r#""email":{"type":"string"}},"required":["id"]}"#
);

/// A protocol-buffer message and an evolution of it, for the `PROTOBUF` data format.
const PROTOBUF_BASE: &str = "syntax = \"proto3\"; message User { string id = 1; }";
const PROTOBUF_EVOLVED: &str =
    "syntax = \"proto3\"; message User { string id = 1; string email = 2; }";

/// Every `Compatibility` value the model admits, paired with the `DataFormat` the row's
/// schema set is created in.
///
/// Source: `smithy_query shape=Compatibility` (NONE | DISABLED | BACKWARD | BACKWARD_ALL |
/// FORWARD | FORWARD_ALL | FULL | FULL_ALL) and `shape=DataFormat` (AVRO | JSON | PROTOBUF).
const MODES: [(&str, &str); 8] = [
    ("NONE", "AVRO"),
    (DISABLED, "AVRO"),
    ("BACKWARD", "AVRO"),
    ("BACKWARD_ALL", "JSON"),
    ("FORWARD", "JSON"),
    ("FORWARD_ALL", "PROTOBUF"),
    ("FULL", "PROTOBUF"),
    ("FULL_ALL", "JSON"),
];

/// The base definition and an evolution of it, in `data_format`.
fn definitions(data_format: &str) -> (&'static str, &'static str) {
    match data_format {
        "AVRO" => (AVRO_BASE, AVRO_EVOLVED),
        "JSON" => (JSON_BASE, JSON_EVOLVED),
        "PROTOBUF" => (PROTOBUF_BASE, PROTOBUF_EVOLVED),
        other => panic!("case {other}: the model admits AVRO, JSON and PROTOBUF only"),
    }
}

/// The ARN of schema `schema` of the default registry, as this endpoint mints it.
fn schema_arn(schema: &str) -> String {
    format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:schema/{DEFAULT_REGISTRY_NAME}/{schema}")
}

/// Whether `value` has the shape the modelled `SchemaVersionIdString` pins: 36 characters,
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`.
///
/// Source: `smithy_query shape=SchemaVersionIdString` — length 36..36 and that pattern.
fn has_version_id_shape(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(groups.iter())
            .all(|(width, group)| {
                group.len() == *width
                    && group
                        .chars()
                        .all(|character| character.is_ascii_digit() || ('a'..='f').contains(&character))
            })
}

/// The string member `member` of `body`.
fn text(case: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: {member} must be a string, got {body}"))
        .to_string()
}

/// The integer member `member` of `body`.
fn number(case: &str, body: &Value, member: &str) -> i64 {
    body.get(member)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case}: {member} must be a number, got {body}"))
}

/// Create the schema set `schema` in the default registry, with `Compatibility` `mode` and
/// the base definition of `data_format` as its first version. Returns the
/// `SchemaVersionId` of that first version.
async fn create_schema(
    router: &MultiServiceRouter,
    case: &str,
    schema: &str,
    data_format: &str,
    mode: &str,
) -> String {
    let (base, _) = definitions(data_format);
    let created = ok(
        router,
        "CreateSchema",
        &json!({
            "SchemaName": schema,
            "DataFormat": data_format,
            "Compatibility": mode,
            "SchemaDefinition": base,
        }),
    )
    .await;
    text(case, &created, "SchemaVersionId")
}

/// The `Schemas` list `ListSchemaVersions` reports for `schema_id`.
async fn listed_versions(router: &MultiServiceRouter, schema_id: &Value) -> Vec<Value> {
    let body = ok(router, "ListSchemaVersions", &json!({ "SchemaId": schema_id })).await;
    body.get("Schemas")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("ListSchemaVersions must report a Schemas list, got {body}"))
}

/// The `VersionNumber` of every entry of a `Schemas` list, in the order it was reported.
fn listed_numbers(case: &str, listed: &[Value]) -> Vec<i64> {
    listed
        .iter()
        .map(|item| number(case, item, "VersionNumber"))
        .collect()
}

// ---------------------------------------------------------------------------
// Compatibility decides whether a version is registered at all
// ---------------------------------------------------------------------------

/// Rule 1: under every mode but `DISABLED` the evolved definition is registered and
/// observable; under `DISABLED` it is refused with `InvalidInputException` and the schema
/// set still holds exactly the version its create registered.
#[tokio::test]
async fn compatibility_decides_whether_a_version_is_registered() {
    let router = emulator();

    for (mode, data_format) in MODES {
        let case = format!("{mode}/{data_format}");
        let schema = format!("payments-{}", mode.to_lowercase());
        let (_, evolved) = definitions(data_format);
        let first_version_id = create_schema(&router, &case, &schema, data_format, mode).await;
        let arn = schema_arn(&schema);
        let schema_id = json!({ "SchemaArn": arn });

        let (status, body) = call(
            &router,
            "RegisterSchemaVersion",
            &json!({ "SchemaId": schema_id, "SchemaDefinition": evolved }),
        )
        .await;

        if mode == DISABLED {
            assert_eq!(
                status, 400,
                "case {case}: a registration under {mode} must be refused with 400, got \
                 {status} with {body}"
            );
            assert!(
                error_code(&body).contains("InvalidInputException"),
                "case {case}: expected InvalidInputException, got {body}"
            );
            assert_eq!(
                error_message(&body),
                format!(
                    "Compatibility {mode} does not allow versioning. SchemaId: \
                     SchemaId(schemaArn={arn}, schemaName=null, registryName=null)"
                ),
                "case {case}: the refusal states the mode and the SchemaId the request \
                 addressed, got {body}"
            );

            let listed = listed_versions(&router, &schema_id).await;
            assert_eq!(
                listed_numbers(&case, &listed),
                vec![FIRST_SCHEMA_VERSION],
                "case {case}: a refused registration adds no version, got {listed:?}"
            );
            assert_eq!(
                text(&case, &listed[0], "SchemaVersionId"),
                first_version_id,
                "case {case}: the version held is still the one the create registered, got \
                 {listed:?}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case}: a registration under {mode} must succeed, got {status} with {body}"
        );
        let version_id = text(&case, &body, "SchemaVersionId");
        assert!(
            has_version_id_shape(&version_id),
            "case {case}: expected a uuid SchemaVersionId, got {version_id}"
        );
        assert_ne!(
            version_id, first_version_id,
            "case {case}: a registered version carries an id of its own, got {version_id}"
        );
        assert_eq!(
            number(&case, &body, "VersionNumber"),
            FIRST_SCHEMA_VERSION + 1,
            "case {case}: expected the set's next version number, got {body}"
        );
        assert_eq!(
            text(&case, &body, "Status"),
            SCHEMA_VERSION_STATUS_AVAILABLE,
            "case {case}: expected a registered version to be available, got {body}"
        );

        // The version is READ BACK through the id the registration minted: what was
        // registered is what the store serves.
        let read = ok(
            &router,
            "GetSchemaVersion",
            &json!({ "SchemaVersionId": version_id }),
        )
        .await;
        assert_eq!(
            text(&case, &read, "SchemaDefinition"),
            evolved,
            "case {case}: the definition registered is the one read back, got {read}"
        );
        assert_eq!(
            number(&case, &read, "VersionNumber"),
            FIRST_SCHEMA_VERSION + 1,
            "case {case}: expected version {} on the read, got {read}",
            FIRST_SCHEMA_VERSION + 1
        );
        assert_eq!(
            text(&case, &read, "DataFormat"),
            data_format,
            "case {case}: a version reports the format of its schema set, got {read}"
        );
        assert_eq!(
            text(&case, &read, "SchemaArn"),
            arn,
            "case {case}: a version reports its schema set's ARN, got {read}"
        );
        assert_eq!(
            text(&case, &read, "Status"),
            SCHEMA_VERSION_STATUS_AVAILABLE,
            "case {case}: expected the stored status on the read, got {read}"
        );
        assert!(
            !text(&case, &read, "CreatedTime").is_empty(),
            "case {case}: a version reports when it was registered, got {read}"
        );

        let listed = listed_versions(&router, &schema_id).await;
        assert_eq!(
            listed_numbers(&case, &listed),
            vec![FIRST_SCHEMA_VERSION, FIRST_SCHEMA_VERSION + 1],
            "case {case}: the listing reports both versions in registration order, got \
             {listed:?}"
        );

        // The schema set's own counters moved with the registration, and its checkpoint —
        // "the version number of the checkpoint (the last time the compatibility mode was
        // changed)" — did not.
        let schema_read = ok(
            &router,
            "GetSchema",
            &json!({ "SchemaId": { "SchemaArn": arn } }),
        )
        .await;
        assert_eq!(
            number(&case, &schema_read, "LatestSchemaVersion"),
            FIRST_SCHEMA_VERSION + 1,
            "case {case}: expected the latest version to be the registered one, got \
             {schema_read}"
        );
        assert_eq!(
            number(&case, &schema_read, "NextSchemaVersion"),
            FIRST_SCHEMA_VERSION + 2,
            "case {case}: expected the next version to follow the registered one, got \
             {schema_read}"
        );
        assert_eq!(
            number(&case, &schema_read, "SchemaCheckpoint"),
            FIRST_SCHEMA_VERSION,
            "case {case}: a registration does not move the checkpoint, got {schema_read}"
        );
    }
}

/// Rule 1, at the boundary the mode is worded over: `DISABLED` restricts versions "after
/// the first schema version", so a schema set created WITHOUT a definition still takes its
/// first version — and refuses the one after it.
#[tokio::test]
async fn a_disabled_schema_set_holding_no_version_takes_its_first() {
    let router = emulator();
    let case = "DISABLED, no first version";
    let schema = "events-disabled-empty";
    ok(
        &router,
        "CreateSchema",
        &json!({
            "SchemaName": schema,
            "DataFormat": "AVRO",
            "Compatibility": DISABLED,
        }),
    )
    .await;
    let schema_id = json!({ "SchemaName": schema });

    let first = ok(
        &router,
        "RegisterSchemaVersion",
        &json!({ "SchemaId": schema_id, "SchemaDefinition": AVRO_BASE }),
    )
    .await;
    assert_eq!(
        number(case, &first, "VersionNumber"),
        FIRST_SCHEMA_VERSION,
        "case {case}: the first version of a set holding none is version \
         {FIRST_SCHEMA_VERSION}, got {first}"
    );

    let (status, refused) = call(
        &router,
        "RegisterSchemaVersion",
        &json!({ "SchemaId": schema_id, "SchemaDefinition": AVRO_EVOLVED }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case}: the version after the first is refused, got {status} with {refused}"
    );
    assert!(
        error_code(&refused).contains("InvalidInputException"),
        "case {case}: expected InvalidInputException, got {refused}"
    );
    assert_eq!(
        error_message(&refused),
        format!(
            "Compatibility {DISABLED} does not allow versioning. SchemaId: \
             SchemaId(schemaArn=null, schemaName={schema}, registryName=null)"
        ),
        "case {case}: the refusal renders the SchemaId the request sent, got {refused}"
    );
    assert_eq!(
        listed_numbers(case, &listed_versions(&router, &schema_id).await),
        vec![FIRST_SCHEMA_VERSION],
        "case {case}: the refused registration added nothing"
    );
}

// ---------------------------------------------------------------------------
// How the registration addresses its schema set
// ---------------------------------------------------------------------------

/// Rule 1 again, over the forms a `SchemaId` may take: each form resolves to the SAME
/// schema set, so four registrations against it number 2, 3, 4 and 5 and the listing
/// reports them in registration order.
#[tokio::test]
async fn every_schema_id_form_registers_against_the_same_schema_set() {
    let router = emulator();
    let schema = "orders-addressing";
    create_schema(&router, "setup", schema, "AVRO", "BACKWARD").await;
    let arn = schema_arn(schema);

    let rows = [
        ("SchemaArn alone", json!({ "SchemaArn": arn })),
        ("SchemaName alone", json!({ "SchemaName": schema })),
        (
            "SchemaName with RegistryName",
            json!({ "SchemaName": schema, "RegistryName": DEFAULT_REGISTRY_NAME }),
        ),
        (
            "both members, naming the same schema",
            json!({ "SchemaArn": arn, "SchemaName": schema }),
        ),
    ];

    let mut expected_number = FIRST_SCHEMA_VERSION;
    let mut expected_numbers = vec![FIRST_SCHEMA_VERSION];
    for (case, schema_id) in rows {
        expected_number += 1;
        // One definition per row, so no row is answered from another's version.
        let definition =
            format!(r#"{{"type":"record","name":"Order","fields":[{{"name":"f{expected_number}","type":"string"}}]}}"#);
        let body = ok(
            &router,
            "RegisterSchemaVersion",
            &json!({ "SchemaId": schema_id, "SchemaDefinition": definition }),
        )
        .await;
        assert_eq!(
            number(case, &body, "VersionNumber"),
            expected_number,
            "case {case}: expected version {expected_number}, got {body}"
        );
        expected_numbers.push(expected_number);

        let read = ok(
            &router,
            "GetSchemaVersion",
            &json!({
                "SchemaId": { "SchemaName": schema },
                "SchemaVersionNumber": { "VersionNumber": expected_number },
            }),
        )
        .await;
        assert_eq!(
            text(case, &read, "SchemaDefinition"),
            definition,
            "case {case}: version {expected_number} serves the definition this row \
             registered, got {read}"
        );
    }

    let listed = listed_versions(&router, &json!({ "SchemaArn": arn })).await;
    assert_eq!(
        listed_numbers("every form", &listed),
        expected_numbers,
        "case every form: the set holds one version per registration, in order, got \
         {listed:?}"
    );
}

/// Rule 2: a definition the schema set already holds a version of registers nothing and is
/// answered with THAT version, whichever version it is.
#[tokio::test]
async fn registering_a_held_definition_reports_the_version_already_held() {
    let router = emulator();
    let schema = "payments-idempotent";
    let first_version_id = create_schema(&router, "setup", schema, "AVRO", "BACKWARD").await;
    let schema_id = json!({ "SchemaName": schema });

    let registered = ok(
        &router,
        "RegisterSchemaVersion",
        &json!({ "SchemaId": schema_id, "SchemaDefinition": AVRO_EVOLVED }),
    )
    .await;
    let second_version_id = text("setup", &registered, "SchemaVersionId");

    let rows = [
        (
            "the definition the create registered",
            AVRO_BASE,
            first_version_id.clone(),
            FIRST_SCHEMA_VERSION,
        ),
        (
            "the definition a registration added",
            AVRO_EVOLVED,
            second_version_id.clone(),
            FIRST_SCHEMA_VERSION + 1,
        ),
        (
            "the same definition a second time over",
            AVRO_EVOLVED,
            second_version_id.clone(),
            FIRST_SCHEMA_VERSION + 1,
        ),
    ];
    for (case, definition, expected_id, expected_number) in rows {
        let body = ok(
            &router,
            "RegisterSchemaVersion",
            &json!({ "SchemaId": schema_id, "SchemaDefinition": definition }),
        )
        .await;
        assert_eq!(
            text(case, &body, "SchemaVersionId"),
            expected_id,
            "case {case}: expected the id of the version already held, got {body}"
        );
        assert_eq!(
            number(case, &body, "VersionNumber"),
            expected_number,
            "case {case}: expected version {expected_number}, got {body}"
        );
        assert_eq!(
            listed_numbers(case, &listed_versions(&router, &schema_id).await),
            vec![FIRST_SCHEMA_VERSION, FIRST_SCHEMA_VERSION + 1],
            "case {case}: nothing is added for a definition already held"
        );
    }
}

/// Rule 3: a `SchemaId` resolving to no schema set of this estate is a MISS, and one that
/// addresses no schema set at all is a bad parameter — and neither writes anything.
#[tokio::test]
async fn a_registration_against_no_schema_set_is_refused() {
    let router = emulator();
    let schema = "payments-present";
    create_schema(&router, "setup", schema, "AVRO", "BACKWARD").await;
    let present = json!({ "SchemaName": schema });

    let misses = [
        ("a SchemaName no schema set has", json!({ "SchemaName": "absent" })),
        (
            "a SchemaArn no schema set has",
            json!({ "SchemaArn": schema_arn("absent") }),
        ),
        (
            "a SchemaName in a registry that does not exist",
            json!({ "SchemaName": schema, "RegistryName": "no-such-registry" }),
        ),
        (
            "a SchemaArn of another Region",
            json!({
                "SchemaArn":
                    format!("arn:aws:glue:us-west-2:{ACCOUNT_ID}:schema/{DEFAULT_REGISTRY_NAME}/{schema}"),
            }),
        ),
        (
            "a SchemaArn of another account",
            json!({
                "SchemaArn":
                    format!("arn:aws:glue:{REGION}:210987654321:schema/{DEFAULT_REGISTRY_NAME}/{schema}"),
            }),
        ),
    ];
    for (case, schema_id) in misses {
        let (status, refused) = call(
            &router,
            "RegisterSchemaVersion",
            &json!({ "SchemaId": schema_id, "SchemaDefinition": AVRO_EVOLVED }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {refused}"
        );
        assert!(
            error_code(&refused).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            "Schema is not found.",
            "case {case}: expected the schema-registry miss wording, got {refused}"
        );
    }

    let bad_parameters = [
        (
            "a SchemaId carrying neither member",
            json!({}),
            "One of SchemaArn or SchemaName has to be provided.",
        ),
        (
            "a SchemaArn that is not a schema's",
            json!({ "SchemaArn": format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:registry/{DEFAULT_REGISTRY_NAME}") }),
            "SchemaArn \"arn:aws:glue:us-east-1:000000000000:registry/default-registry\" is \
             not the ARN of a Glue schema",
        ),
    ];
    for (case, schema_id, expected) in bad_parameters {
        let (status, refused) = call(
            &router,
            "RegisterSchemaVersion",
            &json!({ "SchemaId": schema_id, "SchemaDefinition": AVRO_EVOLVED }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {refused}"
        );
        assert!(
            error_code(&refused).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            expected,
            "case {case}: expected the refusal to state the requirement, got {refused}"
        );
    }

    assert_eq!(
        listed_numbers("refused registrations", &listed_versions(&router, &present).await),
        vec![FIRST_SCHEMA_VERSION],
        "case refused registrations: none of them added a version to the schema set that \
         does exist"
    );
}

// ---------------------------------------------------------------------------
// Reading the versions back
// ---------------------------------------------------------------------------

/// Rule 4: each addressing form of `GetSchemaVersion` resolves the version it names, and a
/// request addressing none is refused.
#[tokio::test]
async fn a_version_is_read_by_id_or_by_number() {
    let router = emulator();
    let schema = "payments-reads";
    create_schema(&router, "setup", schema, "AVRO", "FULL").await;
    let arn = schema_arn(schema);
    let registered = ok(
        &router,
        "RegisterSchemaVersion",
        &json!({ "SchemaId": { "SchemaName": schema }, "SchemaDefinition": AVRO_EVOLVED }),
    )
    .await;
    let second_version_id = text("setup", &registered, "SchemaVersionId");

    let rows = [
        (
            "by SchemaVersionId",
            json!({ "SchemaVersionId": second_version_id }),
            FIRST_SCHEMA_VERSION + 1,
            AVRO_EVOLVED,
        ),
        (
            "by SchemaName and VersionNumber",
            json!({
                "SchemaId": { "SchemaName": schema },
                "SchemaVersionNumber": { "VersionNumber": FIRST_SCHEMA_VERSION },
            }),
            FIRST_SCHEMA_VERSION,
            AVRO_BASE,
        ),
        (
            "by SchemaArn and LatestVersion",
            json!({
                "SchemaId": { "SchemaArn": arn },
                "SchemaVersionNumber": { "LatestVersion": true },
            }),
            FIRST_SCHEMA_VERSION + 1,
            AVRO_EVOLVED,
        ),
    ];
    for (case, body, expected_number, expected_definition) in rows {
        let read = ok(&router, "GetSchemaVersion", &body).await;
        assert_eq!(
            number(case, &read, "VersionNumber"),
            expected_number,
            "case {case}: expected version {expected_number}, got {read}"
        );
        assert_eq!(
            text(case, &read, "SchemaDefinition"),
            expected_definition,
            "case {case}: expected that version's definition, got {read}"
        );
    }

    let refusals = [
        (
            "no version addressed at all",
            json!({}),
            "InvalidInputException",
            "One of SchemaVersionId or SchemaId and SchemaVersionNumber has to be provided.",
        ),
        (
            "a SchemaId with no SchemaVersionNumber",
            json!({ "SchemaId": { "SchemaName": schema } }),
            "InvalidInputException",
            "One of SchemaVersionId or SchemaId and SchemaVersionNumber has to be provided.",
        ),
        (
            "a SchemaVersionNumber naming neither member",
            json!({ "SchemaId": { "SchemaName": schema }, "SchemaVersionNumber": {} }),
            "InvalidInputException",
            "One of LatestVersion or VersionNumber has to be provided.",
        ),
        (
            "a SchemaVersionNumber naming both members",
            json!({
                "SchemaId": { "SchemaName": schema },
                "SchemaVersionNumber": { "LatestVersion": true, "VersionNumber": 1 },
            }),
            "InvalidInputException",
            "One of LatestVersion or VersionNumber has to be provided.",
        ),
        (
            "a version number the set does not hold",
            json!({
                "SchemaId": { "SchemaName": schema },
                "SchemaVersionNumber": { "VersionNumber": 99 },
            }),
            "EntityNotFoundException",
            "Schema version is not found.",
        ),
        (
            "a SchemaVersionId no version carries",
            json!({ "SchemaVersionId": "00000000-0000-0000-0000-000000000000" }),
            "EntityNotFoundException",
            "Schema version is not found.",
        ),
        (
            "a SchemaId naming no schema set",
            json!({
                "SchemaId": { "SchemaName": "absent" },
                "SchemaVersionNumber": { "LatestVersion": true },
            }),
            "EntityNotFoundException",
            "Schema is not found.",
        ),
    ];
    for (case, body, expected_code, expected_message) in refusals {
        let (status, refused) = call(&router, "GetSchemaVersion", &body).await;
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
            "case {case}: expected the refusal to state why, got {refused}"
        );
    }
}

/// Rule 4's listing half: the versions a schema set holds are reported in registration
/// order, one `MaxResults`-sized page at a time, a set holding none answers an empty list,
/// and a `SchemaId` naming no set is refused.
#[tokio::test]
async fn versions_are_listed_in_registration_order_one_page_at_a_time() {
    let router = emulator();
    let schema = "payments-listing";
    create_schema(&router, "setup", schema, "AVRO", "NONE").await;
    let schema_id = json!({ "SchemaName": schema });
    // Four more versions, so the pages below are not all the same size.
    for counter in 2..=5 {
        ok(
            &router,
            "RegisterSchemaVersion",
            &json!({
                "SchemaId": schema_id,
                "SchemaDefinition":
                    format!(r#"{{"type":"record","name":"Payment","fields":[{{"name":"f{counter}","type":"string"}}]}}"#),
            }),
        )
        .await;
    }

    // One page size per row: smaller than the set, exactly the set, larger than it, and
    // none at all (the member's documented default of 25, which is larger still).
    for (case, page_size, expected_pages) in [
        ("MaxResults 2", Some(2), vec![vec![1, 2], vec![3, 4], vec![5]]),
        ("MaxResults 5", Some(5), vec![vec![1, 2, 3, 4, 5]]),
        ("MaxResults 100", Some(100), vec![vec![1, 2, 3, 4, 5]]),
        ("no MaxResults", None, vec![vec![1, 2, 3, 4, 5]]),
    ] {
        let mut pages: Vec<Vec<i64>> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut body = json!({ "SchemaId": schema_id });
            if let Some(page_size) = page_size {
                body["MaxResults"] = json!(page_size);
            }
            if let Some(token) = next_token.as_deref() {
                body["NextToken"] = json!(token);
            }
            let page = ok(&router, "ListSchemaVersions", &body).await;
            let listed = page
                .get("Schemas")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case}: expected a Schemas list, got {page}"));
            pages.push(listed_numbers(case, listed));
            next_token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            pages, expected_pages,
            "case {case}: expected the pages {expected_pages:?}, got {pages:?}"
        );
    }

    let empty = "a schema set holding no version";
    ok(
        &router,
        "CreateSchema",
        &json!({ "SchemaName": "empty-listing", "DataFormat": "AVRO" }),
    )
    .await;
    assert_eq!(
        listed_versions(&router, &json!({ "SchemaName": "empty-listing" })).await,
        Vec::<Value>::new(),
        "case {empty}: expected an empty Schemas list"
    );

    let (status, refused) = call(
        &router,
        "ListSchemaVersions",
        &json!({ "SchemaId": { "SchemaName": "absent" } }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a SchemaId naming no schema set: expected 400, got {status} with {refused}"
    );
    assert!(
        error_code(&refused).contains("EntityNotFoundException"),
        "case a SchemaId naming no schema set: expected EntityNotFoundException, got {refused}"
    );
}
