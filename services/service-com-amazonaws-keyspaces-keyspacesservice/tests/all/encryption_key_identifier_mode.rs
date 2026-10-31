// SPDX-License-Identifier: CC0-1.0

//! The `kmsKeyIdentifier` of an encryption specification belongs to the customer managed
//! encryption type and to no other, driven end to end over the service's own `awsJson1_0`
//! wire protocol and believed only once `GetTable` reports the stored table back.
//!
//! The rule under test, exercised over a SET of inputs rather than one example:
//!
//! 1. An `encryptionSpecification` is refused BOTH when its type is
//!    `CUSTOMER_MANAGED_KMS_KEY` and no `kmsKeyIdentifier` comes with it — that type
//!    "requires the `kms_key_identifier` of the KMS key in Amazon Resource Name (ARN)
//!    format as input" (`EncryptionSpecification$type`) — and when a `kmsKeyIdentifier`
//!    comes with any other type, none of which takes one: the Amazon-owned key "is owned
//!    by Amazon Keyspaces", so there is no key of the caller's to name. The rule holds
//!    wherever a specification is input, so the rows run every operation that takes one:
//!    `CreateTable.encryptionSpecification`, `UpdateTable.encryptionSpecification` and
//!    `RestoreTable.encryptionSpecificationOverride`.
//! 2. Each refusal is the `ValidationException` the model declares for these operations,
//!    and stores nothing: the table keeps the encryption it had, and the create or
//!    restore makes no table.
//! 3. The accepted combinations round-trip through `GetTable`: the customer managed type
//!    reads back with its key identifier, and every other type reads back with no key
//!    identifier at all — including after an update that moves a table between them.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::ops::types::ENCRYPTION_TYPE_VALUES;
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use super::keyspace_table_lifecycle::{
    assert_failure, assert_ok, ensure_keyspace, minimal_schema, settle_wait, Emulator,
};

/// The keyspace every table of this suite lives in.
const KEYSPACE: &str = "encryption_rule_ks";

/// The table the update rows change and the restore rows read as their source.
const SUBJECT_TABLE: &str = "encryption_rule_tbl";

/// The table name a refused create or restore would have made, and must not.
const CANDIDATE_TABLE: &str = "encryption_rule_candidate";

/// A KMS key ARN for the `CUSTOMER_MANAGED_KMS_KEY` rows — the form
/// `EncryptionSpecification$kmsKeyIdentifier` documents ("the Amazon Resource Name (ARN)
/// of the customer managed KMS key").
const KMS_KEY_ARN: &str = "arn:aws:kms:us-west-2:000000000000:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// Every encryption type that takes NO key identifier: the values `EncryptionType` models
/// (`ops::types::ENCRYPTION_TYPE_VALUES`, generated from the model's enum) less the
/// customer managed one, so a type the model gains is covered here by construction.
fn types_taking_no_key() -> Vec<&'static str> {
    ENCRYPTION_TYPE_VALUES
        .iter()
        .copied()
        .filter(|value| *value != keyspaces::CUSTOMER_MANAGED_ENCRYPTION_TYPE)
        .collect()
}

/// The `kmsKeyIdentifier` values the refusal rows span, as `(form, value)`. The member is
/// a string of `length 1..5096` whose documented form is an ARN
/// (`EncryptionSpecification$kmsKeyIdentifier`), so the rows run from the shortest value
/// the model accepts, through the documented key ARN, an alias ARN and a bare key id, to
/// the longest: whether the member is refused is decided by its sibling type, never by
/// which of these it is.
fn key_identifiers() -> Vec<(&'static str, String)> {
    vec![
        ("the documented key ARN", KMS_KEY_ARN.to_string()),
        (
            "an alias ARN",
            "arn:aws:kms:us-west-2:000000000000:alias/app-key".to_string(),
        ),
        (
            "a bare key id",
            "bbbbbbbb-cccc-dddd-eeee-ffffffffffff".to_string(),
        ),
        ("the shortest value the model accepts", "k".to_string()),
        ("the longest value the model accepts", "a".repeat(5096)),
    ]
}

/// The three requests one encryption `specification` reaches the service's shared
/// resolution through: a create's member, an update's member, and a restore's override.
fn requests_carrying(specification: &Value) -> Vec<(&'static str, Value)> {
    vec![
        (
            "CreateTable",
            json!({
                "keyspaceName": KEYSPACE,
                "tableName": CANDIDATE_TABLE,
                "schemaDefinition": minimal_schema("pk"),
                "encryptionSpecification": specification,
            }),
        ),
        (
            "UpdateTable",
            json!({
                "keyspaceName": KEYSPACE,
                "tableName": SUBJECT_TABLE,
                "encryptionSpecification": specification,
            }),
        ),
        (
            "RestoreTable",
            json!({
                "sourceKeyspaceName": KEYSPACE,
                "sourceTableName": SUBJECT_TABLE,
                "targetKeyspaceName": KEYSPACE,
                "targetTableName": CANDIDATE_TABLE,
                "encryptionSpecificationOverride": specification,
            }),
        ),
    ]
}

/// Create `table` in [`KEYSPACE`] from `extra` (the names and a default schema are filled
/// in) and wait for it to settle into `ACTIVE`, the status an update and a restore source
/// have to be in.
async fn create_active_table(emulator: &Emulator, table: &str, extra: Value) {
    let mut request = extra;
    request["keyspaceName"] = json!(KEYSPACE);
    request["tableName"] = json!(table);
    request["schemaDefinition"] = minimal_schema("pk");
    assert_ok(table, emulator.call("CreateTable", request).await);
    tokio::time::sleep(settle_wait()).await;
}

/// `GetTable` for one table of [`KEYSPACE`], as a 200 body.
async fn get_table(emulator: &Emulator, table: &str) -> Value {
    assert_ok(
        table,
        emulator
            .call(
                "GetTable",
                json!({"keyspaceName": KEYSPACE, "tableName": table}),
            )
            .await,
    )
}

/// The `encryptionSpecification` a table reports, or `Value::Null` when it reports none.
async fn reported_encryption(emulator: &Emulator, table: &str) -> Value {
    get_table(emulator, table)
        .await
        .get("encryptionSpecification")
        .cloned()
        .unwrap_or(Value::Null)
}

/// Assert no table was made under [`CANDIDATE_TABLE`]: a refused create or restore stores
/// nothing, so reading it back is the modelled not-found.
async fn assert_no_candidate_table(case: &str, emulator: &Emulator) {
    assert_failure(
        case,
        emulator
            .call(
                "GetTable",
                json!({"keyspaceName": KEYSPACE, "tableName": CANDIDATE_TABLE}),
            )
            .await,
        400,
        "ResourceNotFoundException",
    );
}

/// RULE 1, the "does not apply" half, and RULE 2: a key identifier supplied under a type
/// that takes none is refused by every caller, whatever the identifier looks like, and
/// the refusal leaves the store exactly as it was.
#[tokio::test]
async fn a_key_identifier_outside_the_customer_managed_type_is_refused_by_every_caller() {
    let emulator = Emulator::start();
    ensure_keyspace(&emulator, KEYSPACE).await;
    // Point-in-time recovery gives the table the restorable window a restore needs, so
    // the restore rows are refused for their encryption override and nothing else.
    create_active_table(
        &emulator,
        SUBJECT_TABLE,
        json!({"pointInTimeRecovery": {"status": "ENABLED"}}),
    )
    .await;
    let untouched = get_table(&emulator, SUBJECT_TABLE).await;

    for type_name in types_taking_no_key() {
        for (form, identifier) in key_identifiers() {
            let specification = json!({"type": type_name, "kmsKeyIdentifier": identifier});
            for (operation, request) in requests_carrying(&specification) {
                let case = format!("{operation} with type {type_name:?} and {form}");
                assert_failure(
                    &case,
                    emulator.call(operation, request).await,
                    400,
                    "ValidationException",
                );
                let after = get_table(&emulator, SUBJECT_TABLE).await;
                assert_eq!(
                    after, untouched,
                    "case {case:?}: a refused request must leave the table exactly as it \
                     was, got {after}"
                );
                assert_no_candidate_table(&case, &emulator).await;
            }
        }
    }
}

/// RULE 1, the "requires" half, and RULE 2: the customer managed type without its key
/// identifier is refused by every caller, and stores nothing.
#[tokio::test]
async fn the_customer_managed_type_without_its_key_identifier_is_refused_by_every_caller() {
    let emulator = Emulator::start();
    ensure_keyspace(&emulator, KEYSPACE).await;
    create_active_table(
        &emulator,
        SUBJECT_TABLE,
        json!({"pointInTimeRecovery": {"status": "ENABLED"}}),
    )
    .await;
    let untouched = get_table(&emulator, SUBJECT_TABLE).await;

    let specification = json!({"type": keyspaces::CUSTOMER_MANAGED_ENCRYPTION_TYPE});
    for (operation, request) in requests_carrying(&specification) {
        let case = format!("{operation} with the customer managed type and no key identifier");
        assert_failure(
            &case,
            emulator.call(operation, request).await,
            400,
            "ValidationException",
        );
        let after = get_table(&emulator, SUBJECT_TABLE).await;
        assert_eq!(
            after, untouched,
            "case {case:?}: a refused request must leave the table exactly as it was, got \
             {after}"
        );
        assert_no_candidate_table(&case, &emulator).await;
    }
}

/// RULE 3: the combinations the rule allows are accepted by every caller and read back as
/// supplied — the customer managed type with its key identifier, and every other type
/// with none.
#[tokio::test]
async fn the_allowed_combinations_round_trip_through_get_table() {
    let emulator = Emulator::start();
    ensure_keyspace(&emulator, KEYSPACE).await;

    // Every allowed specification, as `(case name, the specification a request supplies —
    // which is also what a read of the table must then report)`.
    let mut allowed: Vec<(String, Value)> = vec![(
        keyspaces::CUSTOMER_MANAGED_ENCRYPTION_TYPE.to_string(),
        json!({
            "type": keyspaces::CUSTOMER_MANAGED_ENCRYPTION_TYPE,
            "kmsKeyIdentifier": KMS_KEY_ARN,
        }),
    )];
    allowed.extend(
        types_taking_no_key()
            .into_iter()
            .map(|type_name| (type_name.to_string(), json!({"type": type_name}))),
    );

    // A create stores the specification it is given, and a table reads back exactly it.
    for (index, (case, specification)) in allowed.iter().enumerate() {
        let table = format!("created_{index}");
        create_active_table(
            &emulator,
            &table,
            json!({"encryptionSpecification": specification}),
        )
        .await;
        let reported = reported_encryption(&emulator, &table).await;
        assert_eq!(
            &reported, specification,
            "case {case:?}: CreateTable must report back the encryption it stored, got \
             {reported:?}"
        );
    }

    // An update moves a table from one allowed specification to the next, and the stored
    // one is the request's whole specification: moving off the customer managed type
    // therefore drops the key identifier that belonged to it.
    create_active_table(
        &emulator,
        SUBJECT_TABLE,
        json!({"pointInTimeRecovery": {"status": "ENABLED"}}),
    )
    .await;
    for (case, specification) in &allowed {
        assert_ok(
            case,
            emulator
                .call(
                    "UpdateTable",
                    json!({
                        "keyspaceName": KEYSPACE,
                        "tableName": SUBJECT_TABLE,
                        "encryptionSpecification": specification,
                    }),
                )
                .await,
        );
        tokio::time::sleep(settle_wait()).await;
        let reported = reported_encryption(&emulator, SUBJECT_TABLE).await;
        assert_eq!(
            &reported, specification,
            "case {case:?}: UpdateTable must report back the encryption it stored, got \
             {reported:?}"
        );
    }

    // A restore's override replaces the source table's encryption on the table it
    // creates, under the same rule.
    for (index, (case, specification)) in allowed.iter().enumerate() {
        let table = format!("restored_{index}");
        assert_ok(
            case,
            emulator
                .call(
                    "RestoreTable",
                    json!({
                        "sourceKeyspaceName": KEYSPACE,
                        "sourceTableName": SUBJECT_TABLE,
                        "targetKeyspaceName": KEYSPACE,
                        "targetTableName": table,
                        "encryptionSpecificationOverride": specification,
                    }),
                )
                .await,
        );
        tokio::time::sleep(settle_wait()).await;
        let reported = reported_encryption(&emulator, &table).await;
        assert_eq!(
            &reported, specification,
            "case {case:?}: RestoreTable must report back the encryption its override \
             asked for, got {reported:?}"
        );
    }
}
