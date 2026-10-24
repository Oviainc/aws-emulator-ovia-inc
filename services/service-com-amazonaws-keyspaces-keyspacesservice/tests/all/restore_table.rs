// SPDX-License-Identifier: CC0-1.0

//! `RestoreTable`, driven end to end over the service's own `awsJson1_0` wire protocol
//! and believed only once `GetTable`, `GetTableAutoScalingSettings` and
//! `ListTagsForResource` report the restored table back.
//!
//! The rules under test, each exercised over a SET of inputs rather than one example:
//!
//! 1. A restore creates a NEW table under the target keyspace, answers with the ARN
//!    minted for it, and leaves the source table exactly as it was. The restored table's
//!    capacity mode, auto scaling, encryption, point-in-time recovery and tags are the
//!    source table's as of the current time, each replaced by the request's matching
//!    `*Override` member when it carries one.
//! 2. The selected timestamp decides the schema and the TTL settings: a restore
//!    reproduces the revision of them that was in effect at `restoreTimestamp`, and the
//!    current one when the request names no timestamp.
//! 3. The restore is asynchronous: the restored table is `CREATING` from the moment the
//!    call is accepted until the clock owes it `ACTIVE`.
//! 4. A request outside that contract is refused with the error the model declares for
//!    it, and the refusal creates no table — including a `restoreTimestamp` outside the
//!    source table's restorable window and a source table with no window at all.
//! 5. At most [`keyspaces::MAX_CONCURRENT_RESTORES`] restores run in the account at once;
//!    one more is the modelled quota failure until a restore in flight finishes.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use super::keyspace_table_lifecycle::{
    assert_failure, assert_ok, epoch_seconds, error_code, minimal_schema, settle_wait, table_arn,
    Emulator, REGION,
};

/// A KMS key ARN for the `CUSTOMER_MANAGED_KMS_KEY` rows — the form
/// `EncryptionSpecification$kmsKeyIdentifier` documents ("the KMS key in Amazon Resource
/// Name (ARN) format").
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-west-2:000000000000:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// The keyspace the source tables live in.
const SOURCE_KEYSPACE: &str = "restore_src_ks";

/// The keyspace the restored tables are created in.
const TARGET_KEYSPACE: &str = "restore_tgt_ks";

/// The value at `path` (a JSON pointer) in `body`, or `Value::Null` when the body does
/// not carry it.
fn at(body: &Value, path: &str) -> Value {
    body.pointer(path).cloned().unwrap_or(Value::Null)
}

/// Create a keyspace, ignoring a name already taken so a suite can call it per case.
async fn ensure_keyspace(emulator: &Emulator, name: &str) {
    let (status, body) = emulator
        .call("CreateKeyspace", json!({"keyspaceName": name}))
        .await;
    assert!(
        status == 200 || error_code(&body) == "ConflictException",
        "case {name:?}: expected the keyspace to be created or to already exist, got \
         {status} {body}"
    );
}

/// Create `table` in `keyspace` from `request` (the names and a default schema are
/// filled in) and wait for it to settle into `ACTIVE`, which is the only status a
/// restore may read it in.
async fn create_active_table(emulator: &Emulator, keyspace: &str, table: &str, request: Value) {
    let mut request = request;
    request["keyspaceName"] = json!(keyspace);
    request["tableName"] = json!(table);
    if request.get("schemaDefinition").is_none() {
        request["schemaDefinition"] = minimal_schema("pk");
    }
    assert_ok(table, emulator.call("CreateTable", request).await);
    tokio::time::sleep(settle_wait()).await;
}

/// `GetTable` for one table, as a 200 body.
async fn get_table(emulator: &Emulator, keyspace: &str, table: &str) -> Value {
    assert_ok(
        table,
        emulator
            .call(
                "GetTable",
                json!({"keyspaceName": keyspace, "tableName": table}),
            )
            .await,
    )
}

/// The tags of one resource, read back through `ListTagsForResource`. Every resource
/// here holds far fewer than one page, so one call reports them all.
async fn tags_of(case: &str, emulator: &Emulator, resource_arn: &str) -> Vec<(String, String)> {
    let body = assert_ok(
        case,
        emulator
            .call("ListTagsForResource", json!({"resourceArn": resource_arn}))
            .await,
    );
    body.get("tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    (
                        tag["key"].as_str().unwrap_or_default().to_string(),
                        tag["value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A `RestoreTable` request for one source/target pair, plus whatever `extra` members
/// the case supplies.
fn restore_request(source: &str, target: &str, extra: Value) -> Value {
    let mut request = json!({
        "sourceKeyspaceName": SOURCE_KEYSPACE,
        "sourceTableName": source,
        "targetKeyspaceName": TARGET_KEYSPACE,
        "targetTableName": target,
    });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            request[member] = value.clone();
        }
    }
    request
}

/// A valid `autoScalingSpecification` for a provisioned table: the published shape, with
/// `targetValue` inside its documented 20..=90 range.
fn auto_scaling(minimum: i64, maximum: i64, target: f64) -> Value {
    let settings = json!({
        "autoScalingDisabled": false,
        "minimumUnits": minimum,
        "maximumUnits": maximum,
        "scalingPolicy": {
            "targetTrackingScalingPolicyConfiguration": {"targetValue": target},
        },
    });
    json!({
        "readCapacityAutoScaling": settings,
        "writeCapacityAutoScaling": settings,
    })
}

/// One row of RULE 1: a source table to create, the restore request's optional members,
/// and what the restored table must then report.
struct Case {
    /// The case name, which is also the source and target table names.
    name: &'static str,
    /// The `CreateTable` request's optional members for the source table. Every row
    /// enables point-in-time recovery, because a table without it has no restorable
    /// window (RULE 4 covers that refusal).
    source: fn() -> Value,
    /// The `RestoreTable` request's optional members.
    restore: fn() -> Value,
    /// `(JSON pointer, expected value)` pairs the restored table's `GetTable` must
    /// report.
    expect: fn() -> Vec<(&'static str, Value)>,
    /// The tags the restored table must hold, as `ListTagsForResource` reports them.
    expect_tags: fn() -> Vec<(String, String)>,
    /// `(JSON pointer under `autoScalingSpecification`, expected value)` pairs
    /// `GetTableAutoScalingSettings` must report for the restored table, or `None` when
    /// it must have no auto scaling settings at all.
    expect_auto_scaling: fn() -> Option<Vec<(&'static str, Value)>>,
}

/// The rows: one per setting a restore carries over or an override replaces, so the rule
/// is asserted across the class and not on a single example.
fn cases() -> Vec<Case> {
    vec![
        Case {
            // Nothing but the schema and PITR: the restored table takes the modelled
            // defaults the source table itself took.
            name: "defaults_carried_over",
            source: || json!({"pointInTimeRecovery": {"status": "ENABLED"}}),
            restore: || json!({}),
            expect: || {
                vec![
                    ("/capacitySpecification/throughputMode", json!("PAY_PER_REQUEST")),
                    ("/encryptionSpecification/type", json!("AWS_OWNED_KMS_KEY")),
                    ("/pointInTimeRecovery/status", json!("ENABLED")),
                    ("/schemaDefinition/allColumns/0/name", json!("pk")),
                    ("/schemaDefinition/allColumns/1/name", json!("payload")),
                ]
            },
            expect_tags: Vec::new,
            expect_auto_scaling: || None,
        },
        Case {
            // A provisioned source with auto scaling: both are "always restored based on
            // the table's settings as of the current time".
            name: "provisioned_with_auto_scaling",
            source: || {
                json!({
                    "pointInTimeRecovery": {"status": "ENABLED"},
                    "capacitySpecification": {
                        "throughputMode": "PROVISIONED",
                        "readCapacityUnits": 5,
                        "writeCapacityUnits": 7,
                    },
                    "autoScalingSpecification": auto_scaling(5, 50, 70.0),
                })
            },
            restore: || json!({}),
            expect: || {
                vec![
                    ("/capacitySpecification/throughputMode", json!("PROVISIONED")),
                    ("/capacitySpecification/readCapacityUnits", json!(5)),
                    ("/capacitySpecification/writeCapacityUnits", json!(7)),
                    // A restored table is a new table: no update has moved it to
                    // PAY_PER_REQUEST, so it carries no such stamp.
                    (
                        "/capacitySpecification/lastUpdateToPayPerRequestTimestamp",
                        Value::Null,
                    ),
                ]
            },
            expect_tags: Vec::new,
            expect_auto_scaling: || {
                Some(vec![
                    ("/readCapacityAutoScaling/minimumUnits", json!(5)),
                    ("/readCapacityAutoScaling/maximumUnits", json!(50)),
                    (
                        "/readCapacityAutoScaling/scalingPolicy/targetTrackingScalingPolicyConfiguration/targetValue",
                        json!(70.0),
                    ),
                    ("/writeCapacityAutoScaling/minimumUnits", json!(5)),
                    ("/writeCapacityAutoScaling/autoScalingDisabled", json!(false)),
                ])
            },
        },
        Case {
            // `capacitySpecificationOverride` replaces the mode, and the source's auto
            // scaling does not apply to the mode the restored table lands in.
            name: "capacity_override_to_on_demand",
            source: || {
                json!({
                    "pointInTimeRecovery": {"status": "ENABLED"},
                    "capacitySpecification": {
                        "throughputMode": "PROVISIONED",
                        "readCapacityUnits": 9,
                        "writeCapacityUnits": 9,
                    },
                    "autoScalingSpecification": auto_scaling(9, 90, 20.0),
                })
            },
            restore: || json!({"capacitySpecificationOverride": {"throughputMode": "PAY_PER_REQUEST"}}),
            expect: || {
                vec![
                    ("/capacitySpecification/throughputMode", json!("PAY_PER_REQUEST")),
                    ("/capacitySpecification/readCapacityUnits", Value::Null),
                    ("/capacitySpecification/writeCapacityUnits", Value::Null),
                ]
            },
            expect_tags: Vec::new,
            expect_auto_scaling: || None,
        },
        Case {
            // The other direction, with the auto scaling settings supplied by the
            // restore itself.
            name: "capacity_override_to_provisioned",
            source: || json!({"pointInTimeRecovery": {"status": "ENABLED"}}),
            restore: || {
                json!({
                    "capacitySpecificationOverride": {
                        "throughputMode": "PROVISIONED",
                        "readCapacityUnits": 11,
                        "writeCapacityUnits": 23,
                    },
                    "autoScalingSpecification": auto_scaling(1, 100, 90.0),
                })
            },
            expect: || {
                vec![
                    ("/capacitySpecification/throughputMode", json!("PROVISIONED")),
                    ("/capacitySpecification/readCapacityUnits", json!(11)),
                    ("/capacitySpecification/writeCapacityUnits", json!(23)),
                ]
            },
            expect_tags: Vec::new,
            expect_auto_scaling: || {
                Some(vec![
                    ("/readCapacityAutoScaling/minimumUnits", json!(1)),
                    ("/readCapacityAutoScaling/maximumUnits", json!(100)),
                    (
                        "/writeCapacityAutoScaling/scalingPolicy/targetTrackingScalingPolicyConfiguration/targetValue",
                        json!(90.0),
                    ),
                ])
            },
        },
        Case {
            // `encryptionSpecificationOverride`, on the one type that takes a key.
            name: "encryption_override",
            source: || json!({"pointInTimeRecovery": {"status": "ENABLED"}}),
            restore: || {
                json!({
                    "encryptionSpecificationOverride": {
                        "type": "CUSTOMER_MANAGED_KMS_KEY",
                        "kmsKeyIdentifier": KMS_KEY_ARN,
                    },
                })
            },
            expect: || {
                vec![
                    ("/encryptionSpecification/type", json!("CUSTOMER_MANAGED_KMS_KEY")),
                    ("/encryptionSpecification/kmsKeyIdentifier", json!(KMS_KEY_ARN)),
                ]
            },
            expect_tags: Vec::new,
            expect_auto_scaling: || None,
        },
        Case {
            // `pointInTimeRecoveryOverride`: the restored table need not keep the
            // source's backup setting, and a disabled one reports no restorable point.
            name: "point_in_time_recovery_override",
            source: || json!({"pointInTimeRecovery": {"status": "ENABLED"}}),
            restore: || json!({"pointInTimeRecoveryOverride": {"status": "DISABLED"}}),
            expect: || {
                vec![
                    ("/pointInTimeRecovery/status", json!("DISABLED")),
                    ("/pointInTimeRecovery/earliestRestorableTimestamp", Value::Null),
                ]
            },
            expect_tags: Vec::new,
            expect_auto_scaling: || None,
        },
        Case {
            // The TTL settings and the table's comment come from the source table.
            name: "ttl_and_comment_carried_over",
            source: || {
                json!({
                    "pointInTimeRecovery": {"status": "ENABLED"},
                    "ttl": {"status": "ENABLED"},
                    "defaultTimeToLive": 3_600,
                    "comment": {"message": "orders by customer"},
                    "clientSideTimestamps": {"status": "ENABLED"},
                })
            },
            restore: || json!({}),
            expect: || {
                vec![
                    ("/ttl/status", json!("ENABLED")),
                    ("/defaultTimeToLive", json!(3_600)),
                    ("/comment/message", json!("orders by customer")),
                    ("/clientSideTimestamps/status", json!("ENABLED")),
                ]
            },
            expect_tags: Vec::new,
            expect_auto_scaling: || None,
        },
        Case {
            // Tags are among the settings a restore takes from the source table.
            name: "tags_carried_over",
            source: || {
                json!({
                    "pointInTimeRecovery": {"status": "ENABLED"},
                    "tags": [{"key": "env", "value": "prod"}, {"key": "team", "value": "orders"}],
                })
            },
            restore: || json!({}),
            expect: Vec::new,
            expect_tags: || {
                vec![
                    ("env".to_string(), "prod".to_string()),
                    ("team".to_string(), "orders".to_string()),
                ]
            },
            expect_auto_scaling: || None,
        },
        Case {
            // ... and `tagsOverride` replaces that whole set rather than adding to it.
            name: "tags_override",
            source: || {
                json!({
                    "pointInTimeRecovery": {"status": "ENABLED"},
                    "tags": [{"key": "env", "value": "prod"}],
                })
            },
            restore: || json!({"tagsOverride": [{"key": "restored", "value": "yes"}]}),
            expect: Vec::new,
            expect_tags: || vec![("restored".to_string(), "yes".to_string())],
            expect_auto_scaling: || None,
        },
        Case {
            // A schema with every role filled, to show the restored schema is the
            // source's verbatim and not a reduction of it.
            name: "full_schema_carried_over",
            source: || {
                json!({
                    "pointInTimeRecovery": {"status": "ENABLED"},
                    "schemaDefinition": {
                        "allColumns": [
                            {"name": "pk", "type": "text"},
                            {"name": "ck", "type": "timestamp"},
                            {"name": "shared", "type": "int"},
                            {"name": "payload", "type": "blob"},
                        ],
                        "partitionKeys": [{"name": "pk"}],
                        "clusteringKeys": [{"name": "ck", "orderBy": "DESC"}],
                        "staticColumns": [{"name": "shared"}],
                    },
                })
            },
            restore: || json!({}),
            expect: || {
                vec![
                    ("/schemaDefinition/allColumns/1/name", json!("ck")),
                    ("/schemaDefinition/allColumns/3/type", json!("blob")),
                    ("/schemaDefinition/clusteringKeys/0/orderBy", json!("DESC")),
                    ("/schemaDefinition/staticColumns/0/name", json!("shared")),
                ]
            },
            expect_tags: Vec::new,
            expect_auto_scaling: || None,
        },
    ]
}

/// RULE 1 and RULE 3: a restore creates the target table out of the source table's
/// settings and the request's overrides, answers with that table's ARN, leaves the source
/// table untouched, and the created table is `CREATING` until the clock settles it.
#[tokio::test]
async fn restore_creates_the_target_table_from_the_source_settings_and_the_overrides() {
    let emulator = Emulator::start();
    ensure_keyspace(&emulator, SOURCE_KEYSPACE).await;
    ensure_keyspace(&emulator, TARGET_KEYSPACE).await;

    for case in cases() {
        let source_table = format!("src_{}", case.name);
        let target_table = format!("tgt_{}", case.name);
        create_active_table(&emulator, SOURCE_KEYSPACE, &source_table, (case.source)()).await;
        let source_before = get_table(&emulator, SOURCE_KEYSPACE, &source_table).await;

        let restored = assert_ok(
            case.name,
            emulator
                .call(
                    "RestoreTable",
                    restore_request(&source_table, &target_table, (case.restore)()),
                )
                .await,
        );
        assert_eq!(
            restored["restoredTableARN"],
            json!(table_arn(TARGET_KEYSPACE, &target_table)),
            "case {:?}: expected the published ARN of the restored table, got {restored}",
            case.name
        );

        // RULE 3: the restore is asynchronous, so the table it created is in the
        // transient status first and `ACTIVE` once the clock owes it.
        let creating = get_table(&emulator, TARGET_KEYSPACE, &target_table).await;
        assert_eq!(
            creating["status"],
            json!(keyspaces::TABLE_STATUS_CREATING),
            "case {:?}: expected the restored table to be {}, got {creating}",
            case.name,
            keyspaces::TABLE_STATUS_CREATING
        );

        // RULE 1: the source table is a source, not a resource the restore changed.
        let source_after = get_table(&emulator, SOURCE_KEYSPACE, &source_table).await;
        assert_eq!(
            source_after, source_before,
            "case {:?}: expected the source table to read back unchanged, got \
             {source_after} after {source_before}",
            case.name
        );

        tokio::time::sleep(settle_wait()).await;
        let got = get_table(&emulator, TARGET_KEYSPACE, &target_table).await;
        assert_eq!(
            got["status"],
            json!(keyspaces::TABLE_STATUS_ACTIVE),
            "case {:?}: expected the restored table to settle into {}, got {got}",
            case.name,
            keyspaces::TABLE_STATUS_ACTIVE
        );
        assert_eq!(
            got["keyspaceName"],
            json!(TARGET_KEYSPACE),
            "case {:?}: expected the restored table in the target keyspace, got {got}",
            case.name
        );
        assert_eq!(
            got["tableName"],
            json!(target_table),
            "case {:?}: expected the restored table under the target name, got {got}",
            case.name
        );
        for (pointer, expected) in (case.expect)() {
            assert_eq!(
                at(&got, pointer),
                expected,
                "case {:?}: expected {pointer} to be {expected}, got {}",
                case.name,
                at(&got, pointer)
            );
        }

        // The schema the restore reproduced is the source's, member for member.
        assert_eq!(
            at(&got, "/schemaDefinition"),
            at(&source_before, "/schemaDefinition"),
            "case {:?}: expected the source table's schema verbatim, got {}",
            case.name,
            at(&got, "/schemaDefinition")
        );

        let expected_tags = (case.expect_tags)();
        let got_tags = tags_of(
            case.name,
            &emulator,
            &table_arn(TARGET_KEYSPACE, &target_table),
        )
        .await;
        assert_eq!(
            got_tags, expected_tags,
            "case {:?}: expected the restored table to hold {expected_tags:?}, got \
             {got_tags:?}",
            case.name
        );

        let auto_scaling_answer = emulator
            .call(
                "GetTableAutoScalingSettings",
                json!({"keyspaceName": TARGET_KEYSPACE, "tableName": target_table}),
            )
            .await;
        match (case.expect_auto_scaling)() {
            Some(expected) => {
                let body = assert_ok(case.name, auto_scaling_answer);
                for (pointer, value) in expected {
                    let reported = at(&body, &format!("/autoScalingSpecification{pointer}"));
                    assert_eq!(
                        reported, value,
                        "case {:?}: expected autoScalingSpecification{pointer} to be {value}, \
                         got {reported}",
                        case.name
                    );
                }
            }
            // A table with no scalable target is a table this operation "tried to access
            // a ... resource that doesn't exist" for.
            None => assert_failure(case.name, auto_scaling_answer, 404, "ResourceNotFoundException"),
        }
    }
}

/// RULE 2: `restoreTimestamp` picks the revision of the schema and the TTL settings that
/// was in effect at that instant — and a request naming no timestamp restores the current
/// one.
///
/// The source table is changed TWICE, so the rows span a timestamp before both changes,
/// one between them, one after both, and an absent one.
#[tokio::test]
async fn restore_timestamp_selects_the_schema_and_ttl_revision_in_effect_then() {
    let emulator = Emulator::start();
    ensure_keyspace(&emulator, SOURCE_KEYSPACE).await;
    ensure_keyspace(&emulator, TARGET_KEYSPACE).await;
    let source_table = "revisions";
    create_active_table(
        &emulator,
        SOURCE_KEYSPACE,
        source_table,
        json!({
            "pointInTimeRecovery": {"status": "ENABLED"},
            "defaultTimeToLive": 100,
        }),
    )
    .await;
    // The window the restores below target opens at the source table's creation, which is
    // what `GetTable` reports for a table with point-in-time recovery enabled.
    let source = get_table(&emulator, SOURCE_KEYSPACE, source_table).await;
    assert_eq!(
        at(&source, "/pointInTimeRecovery/earliestRestorableTimestamp"),
        at(&source, "/creationTimestamp"),
        "expected the restorable window to open at the table's creation, got {source}"
    );

    // Revision 1 is in effect now: the created schema, defaultTimeToLive 100.
    let before_any_change = SystemTime::now();
    assert_ok(
        "add_column",
        emulator
            .call(
                "UpdateTable",
                json!({
                    "keyspaceName": SOURCE_KEYSPACE,
                    "tableName": source_table,
                    "addColumns": [{"name": "added_later", "type": "text"}],
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;
    // Revision 2: the wider schema, defaultTimeToLive still 100.
    let after_the_column = SystemTime::now();
    assert_ok(
        "change_default_ttl",
        emulator
            .call(
                "UpdateTable",
                json!({
                    "keyspaceName": SOURCE_KEYSPACE,
                    "tableName": source_table,
                    "defaultTimeToLive": 200,
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;
    // The current revision: the wider schema, defaultTimeToLive 200.
    let after_both_changes = SystemTime::now();

    /// One row: the `restoreTimestamp` to restore at, and the revision it must reproduce.
    struct Row {
        name: &'static str,
        restore_timestamp: Option<SystemTime>,
        columns: usize,
        has_added_column: bool,
        default_time_to_live: i64,
    }

    let rows = [
        Row {
            name: "before_any_change",
            restore_timestamp: Some(before_any_change),
            columns: 2,
            has_added_column: false,
            default_time_to_live: 100,
        },
        Row {
            name: "between_the_changes",
            restore_timestamp: Some(after_the_column),
            columns: 3,
            has_added_column: true,
            default_time_to_live: 100,
        },
        Row {
            name: "after_both_changes",
            restore_timestamp: Some(after_both_changes),
            columns: 3,
            has_added_column: true,
            default_time_to_live: 200,
        },
        Row {
            // No timestamp at all asks for the most recent restorable point.
            name: "no_timestamp",
            restore_timestamp: None,
            columns: 3,
            has_added_column: true,
            default_time_to_live: 200,
        },
    ];

    for row in rows {
        let target_table = format!("tgt_{}", row.name);
        let extra = match row.restore_timestamp {
            None => json!({}),
            Some(at) => json!({"restoreTimestamp": epoch_seconds(at)}),
        };
        assert_ok(
            row.name,
            emulator
                .call(
                    "RestoreTable",
                    restore_request(source_table, &target_table, extra),
                )
                .await,
        );
        let got = get_table(&emulator, TARGET_KEYSPACE, &target_table).await;
        let columns = at(&got, "/schemaDefinition/allColumns");
        let reported = columns.as_array().map(Vec::len).unwrap_or_default();
        assert_eq!(
            reported, row.columns,
            "case {:?}: expected the schema in effect then to hold {} column(s), got {columns}",
            row.name, row.columns
        );
        let has_added = columns
            .as_array()
            .is_some_and(|all| all.iter().any(|column| column["name"] == json!("added_later")));
        assert_eq!(
            has_added, row.has_added_column,
            "case {:?}: expected the added column present={}, got {columns}",
            row.name, row.has_added_column
        );
        assert_eq!(
            at(&got, "/defaultTimeToLive"),
            json!(row.default_time_to_live),
            "case {:?}: expected defaultTimeToLive {}, got {}",
            row.name,
            row.default_time_to_live,
            at(&got, "/defaultTimeToLive")
        );
    }
}

/// RULE 4: every request outside the contract is refused with the modelled error, and
/// nothing is created for it.
#[tokio::test]
async fn restore_refuses_what_the_contract_does_not_allow() {
    let emulator = Emulator::start();
    ensure_keyspace(&emulator, SOURCE_KEYSPACE).await;
    ensure_keyspace(&emulator, TARGET_KEYSPACE).await;
    // One restorable source table, one with no restorable window at all, and one table
    // already sitting under a target name.
    create_active_table(
        &emulator,
        SOURCE_KEYSPACE,
        "restorable",
        json!({"pointInTimeRecovery": {"status": "ENABLED"}}),
    )
    .await;
    create_active_table(&emulator, SOURCE_KEYSPACE, "no_backups", json!({})).await;
    create_active_table(
        &emulator,
        TARGET_KEYSPACE,
        "already_there",
        json!({"pointInTimeRecovery": {"status": "ENABLED"}}),
    )
    .await;
    let restorable = get_table(&emulator, SOURCE_KEYSPACE, "restorable").await;
    let created_at = at(&restorable, "/creationTimestamp")
        .as_f64()
        .expect("GetTable reports the source table's creation timestamp");
    let now = epoch_seconds(SystemTime::now());

    /// One refusal row: the request, and the status and code the model declares for it.
    struct Row {
        name: &'static str,
        request: Value,
        status: u16,
        code: &'static str,
    }

    let rows = vec![
        Row {
            name: "source_keyspace_missing",
            request: json!({
                "sourceKeyspaceName": "no_such_keyspace",
                "sourceTableName": "restorable",
                "targetKeyspaceName": TARGET_KEYSPACE,
                "targetTableName": "refused_1",
            }),
            status: 404,
            code: "ResourceNotFoundException",
        },
        Row {
            name: "source_table_missing",
            request: restore_request("no_such_table", "refused_2", json!({})),
            status: 404,
            code: "ResourceNotFoundException",
        },
        Row {
            name: "target_keyspace_missing",
            request: json!({
                "sourceKeyspaceName": SOURCE_KEYSPACE,
                "sourceTableName": "restorable",
                "targetKeyspaceName": "no_such_keyspace",
                "targetTableName": "refused_3",
            }),
            status: 404,
            code: "ResourceNotFoundException",
        },
        Row {
            name: "target_table_already_exists",
            request: restore_request("restorable", "already_there", json!({})),
            status: 409,
            code: "ConflictException",
        },
        Row {
            // No point-in-time recovery means no restorable window, so there is no
            // instant this table can be restored to.
            name: "source_without_point_in_time_recovery",
            request: restore_request("no_backups", "refused_4", json!({})),
            status: 400,
            code: "ValidationException",
        },
        Row {
            // "within the earliest_restorable_timestamp and the current time": one
            // minute before the window opened is outside it.
            name: "restore_timestamp_before_the_window",
            request: restore_request(
                "restorable",
                "refused_5",
                json!({"restoreTimestamp": created_at - 60.0}),
            ),
            status: 400,
            code: "ValidationException",
        },
        Row {
            // ... and so is an hour into the future.
            name: "restore_timestamp_after_the_current_time",
            request: restore_request(
                "restorable",
                "refused_6",
                json!({"restoreTimestamp": now + 3_600.0}),
            ),
            status: 400,
            code: "ValidationException",
        },
        Row {
            // A system keyspace is read-only to a client, on either side of a restore.
            name: "service_managed_source",
            request: json!({
                "sourceKeyspaceName": "system",
                "sourceTableName": "local",
                "targetKeyspaceName": TARGET_KEYSPACE,
                "targetTableName": "refused_7",
            }),
            status: 400,
            code: "ValidationException",
        },
        Row {
            name: "service_managed_target",
            request: json!({
                "sourceKeyspaceName": SOURCE_KEYSPACE,
                "sourceTableName": "restorable",
                "targetKeyspaceName": "system_schema",
                "targetTableName": "refused_8",
            }),
            status: 400,
            code: "ValidationException",
        },
        Row {
            // The capacity units are input for PROVISIONED only, and that mode requires
            // both of them.
            name: "capacity_override_missing_units",
            request: restore_request(
                "restorable",
                "refused_9",
                json!({"capacitySpecificationOverride": {"throughputMode": "PROVISIONED"}}),
            ),
            status: 400,
            code: "ValidationException",
        },
        Row {
            name: "capacity_override_units_on_on_demand",
            request: restore_request(
                "restorable",
                "refused_10",
                json!({
                    "capacitySpecificationOverride": {
                        "throughputMode": "PAY_PER_REQUEST",
                        "readCapacityUnits": 5,
                    },
                }),
            ),
            status: 400,
            code: "ValidationException",
        },
        Row {
            // A customer-managed key "requires the kms_key_identifier ... as input".
            name: "encryption_override_without_key",
            request: restore_request(
                "restorable",
                "refused_11",
                json!({"encryptionSpecificationOverride": {"type": "CUSTOMER_MANAGED_KMS_KEY"}}),
            ),
            status: 400,
            code: "ValidationException",
        },
        Row {
            // Auto scaling is for "the restored table in provisioned capacity mode", and
            // this restore leaves the table on demand.
            name: "auto_scaling_on_an_on_demand_restore",
            request: restore_request(
                "restorable",
                "refused_12",
                json!({"autoScalingSpecification": auto_scaling(5, 50, 50.0)}),
            ),
            status: 400,
            code: "ValidationException",
        },
        Row {
            // The `aws:` tag key prefix "cannot be assigned" by a caller.
            name: "tags_override_with_a_reserved_key",
            request: restore_request(
                "restorable",
                "refused_13",
                json!({"tagsOverride": [{"key": "aws:owner", "value": "mine"}]}),
            ),
            status: 400,
            code: "ValidationException",
        },
        Row {
            // Modelled: `targetTableName` is `len 1..48` with a pattern, so the boundary
            // validator refuses this one before the handler runs.
            name: "target_table_name_outside_the_modelled_pattern",
            request: restore_request("restorable", "bad name!", json!({})),
            status: 400,
            code: "ValidationException",
        },
    ];

    for row in rows {
        let target_table = row.request["targetTableName"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_failure(
            row.name,
            emulator.call("RestoreTable", row.request).await,
            row.status,
            row.code,
        );
        // A refused restore created nothing: the only target name that resolves is the
        // table that was already there, and a name outside the modelled pattern is one
        // `GetTable`'s own boundary validator refuses rather than looks up.
        if target_table != "already_there" && !target_table.contains(' ') {
            assert_failure(
                row.name,
                emulator
                    .call(
                        "GetTable",
                        json!({"keyspaceName": TARGET_KEYSPACE, "tableName": target_table}),
                    )
                    .await,
                404,
                "ResourceNotFoundException",
            );
        }
    }

    // A source that is itself mid-change is the modelled conflict: this one is still
    // `CREATING`, because nothing waited for it.
    assert_ok(
        "creating_source",
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": SOURCE_KEYSPACE,
                    "tableName": "still_creating",
                    "schemaDefinition": minimal_schema("pk"),
                    "pointInTimeRecovery": {"status": "ENABLED"},
                }),
            )
            .await,
    );
    assert_failure(
        "creating_source",
        emulator
            .call(
                "RestoreTable",
                restore_request("still_creating", "refused_14", json!({})),
            )
            .await,
        409,
        "ConflictException",
    );

    // A per-Region setting is "the Region specific settings of a multi-Regional table", so
    // a restore into a single-Region keyspace has no Region to put one in.
    assert_failure(
        "replica_specifications_in_a_single_region_keyspace",
        emulator
            .call(
                "RestoreTable",
                restore_request(
                    "restorable",
                    "refused_15",
                    json!({"replicaSpecifications": [{"region": "us-east-1"}]}),
                ),
            )
            .await,
        400,
        "ValidationException",
    );
}

/// RULE 1, for the per-Region settings of a multi-Region target: a restore resolves
/// `replicaSpecifications` against the TARGET keyspace's replication group and reports the
/// result through `GetTable`'s replica summaries, and carries the SOURCE table's
/// per-Region settings over for the Regions that keyspace replicates to when the request
/// names none.
#[tokio::test]
async fn restore_carries_the_per_region_settings_of_a_multi_region_target() {
    let emulator = Emulator::start();
    ensure_keyspace(&emulator, SOURCE_KEYSPACE).await;
    // "regionList requires the current Region and at least one additional Amazon Web
    // Services Region", so the group is the signing Region plus one.
    let other_region = "us-east-1";
    let multi_region_keyspace = "restore_mr_ks";
    assert_ok(
        multi_region_keyspace,
        emulator
            .call(
                "CreateKeyspace",
                json!({
                    "keyspaceName": multi_region_keyspace,
                    "replicationSpecification": {
                        "replicationStrategy": "MULTI_REGION",
                        "regionList": [REGION, other_region],
                    },
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;
    // A provisioned source table, because a per-Region read capacity override belongs to
    // `throughputMode:PROVISIONED`.
    create_active_table(
        &emulator,
        SOURCE_KEYSPACE,
        "mr_source",
        json!({
            "pointInTimeRecovery": {"status": "ENABLED"},
            "capacitySpecification": {
                "throughputMode": "PROVISIONED",
                "readCapacityUnits": 6,
                "writeCapacityUnits": 6,
            },
        }),
    )
    .await;

    /// One row: the restore request's per-Region members, and the read capacity each
    /// Region of the restored table must then report.
    struct Row {
        name: &'static str,
        extra: Value,
        in_signing_region: Value,
        in_other_region: Value,
    }

    let rows = vec![
        Row {
            // No per-Region member: every Region uses the table's own read capacity.
            name: "no_per_region_member",
            extra: json!({}),
            in_signing_region: json!(6),
            in_other_region: json!(6),
        },
        Row {
            // An override for the additional Region only: that Region reports it, and the
            // table's own Region keeps the table's.
            name: "override_the_additional_region",
            extra: json!({
                "replicaSpecifications": [{"region": "us-east-1", "readCapacityUnits": 15}],
            }),
            in_signing_region: json!(6),
            in_other_region: json!(15),
        },
        Row {
            // Both Regions named, each with its own read capacity.
            name: "override_both_regions",
            extra: json!({
                "replicaSpecifications": [
                    {"region": "us-west-2", "readCapacityUnits": 9},
                    {"region": "us-east-1", "readCapacityUnits": 21},
                ],
            }),
            in_signing_region: json!(9),
            in_other_region: json!(21),
        },
    ];

    for row in rows {
        let target_table = format!("mr_{}", row.name);
        let mut request = restore_request("mr_source", &target_table, row.extra);
        request["targetKeyspaceName"] = json!(multi_region_keyspace);
        assert_ok(row.name, emulator.call("RestoreTable", request).await);

        let got = get_table(&emulator, multi_region_keyspace, &target_table).await;
        let summaries = at(&got, "/replicaSpecifications");
        let reported: Vec<(String, Value)> = summaries
            .as_array()
            .map(|all| {
                all.iter()
                    .map(|summary| {
                        (
                            summary["region"].as_str().unwrap_or_default().to_string(),
                            at(summary, "/capacitySpecification/readCapacityUnits"),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        // One summary per Region the target keyspace replicates to, in the one total order
        // the keyspace stores them in.
        let expected: Vec<(String, Value)> = vec![
            (REGION.to_string(), row.in_signing_region.clone()),
            (other_region.to_string(), row.in_other_region.clone()),
        ];
        assert_eq!(
            reported, expected,
            "case {:?}: expected the replica summaries to report {expected:?}, got {summaries}",
            row.name
        );
    }

    // A Region the target keyspace does not replicate to is not a Region the restored
    // table can carry a setting for.
    let mut outside = restore_request(
        "mr_source",
        "mr_outside",
        json!({"replicaSpecifications": [{"region": "eu-west-1", "readCapacityUnits": 5}]}),
    );
    outside["targetKeyspaceName"] = json!(multi_region_keyspace);
    assert_failure(
        "region_outside_the_replication_group",
        emulator.call("RestoreTable", outside).await,
        400,
        "ValidationException",
    );
}

/// RULE 5: the account runs at most [`keyspaces::MAX_CONCURRENT_RESTORES`] restores at
/// once, and one more is the modelled quota failure until a restore in flight finishes.
#[tokio::test]
async fn restores_past_the_concurrent_limit_are_the_modelled_quota_failure() {
    let emulator = Emulator::start();
    ensure_keyspace(&emulator, SOURCE_KEYSPACE).await;
    ensure_keyspace(&emulator, TARGET_KEYSPACE).await;
    create_active_table(
        &emulator,
        SOURCE_KEYSPACE,
        "busy",
        json!({"pointInTimeRecovery": {"status": "ENABLED"}}),
    )
    .await;

    // Back to back, so every one of them is still `CREATING` when the next arrives.
    for index in 0..keyspaces::MAX_CONCURRENT_RESTORES {
        let case = format!("concurrent_{index}");
        assert_ok(
            &case,
            emulator
                .call(
                    "RestoreTable",
                    restore_request("busy", &format!("tgt_{index}"), json!({})),
                )
                .await,
        );
    }
    assert_failure(
        "one_past_the_limit",
        emulator
            .call(
                "RestoreTable",
                restore_request("busy", "tgt_over_limit", json!({})),
            )
            .await,
        402,
        "ServiceQuotaExceededException",
    );

    // Once the restores in flight have settled, the account is under the limit again and
    // the same request is accepted.
    tokio::time::sleep(settle_wait()).await;
    assert_ok(
        "after_the_restores_settled",
        emulator
            .call(
                "RestoreTable",
                restore_request("busy", "tgt_over_limit", json!({})),
            )
            .await,
    );
    let got = get_table(&emulator, TARGET_KEYSPACE, "tgt_over_limit").await;
    assert_eq!(
        got["resourceArn"],
        json!(table_arn(TARGET_KEYSPACE, "tgt_over_limit")),
        "expected the restored table to read back under its own ARN, got {got}"
    );
}
