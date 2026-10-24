// SPDX-License-Identifier: CC0-1.0

//! `UpdateTable`, driven end to end over the service's own `awsJson1_0` wire protocol
//! and believed only once `GetTable` reports the change back.
//!
//! The rules under test, each exercised over a SET of inputs rather than one example:
//!
//! 1. An `UpdateTable` applies the ONE setting the request names to the stored table,
//!    answers with that table's ARN, and leaves every other setting exactly as it was.
//!    The rows span the settings this slice serves — added columns, capacity mode,
//!    encryption, point-in-time recovery, ttl, default ttl, client-side timestamps —
//!    and, where a setting carries a value, the published bounds of that value.
//! 2. The update is asynchronous: the table is `UPDATING` from the moment the call is
//!    accepted until the clock owes it `ACTIVE` again, and a table that is not `ACTIVE`
//!    cannot be updated.
//! 3. `lastUpdateToPayPerRequestTimestamp` exists exactly on a table some update moved
//!    to `PAY_PER_REQUEST`.
//! 4. A request that breaks a rule the model cannot state is refused with the error the
//!    model declares for it, and the refusal leaves the table untouched — including the
//!    documented "you can only update one specific table setting per update operation".
//! 5. The ONE setting no report here can answer for — `cdcSpecification` — is refused with
//!    the scaffold's 501 rather than stored and forgotten; every other modelled setting is
//!    applied (see `tests/all/update_table_scaling_settings.rs` for auto scaling, the
//!    per-Region settings and warm throughput).
//! 6. A stored schema only grows, so growing one past the emulator's column cap is
//!    refused with the quota error the model declares — in one call or across several.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::ops::types::TABLE_STATUS_VALUES;
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use super::keyspace_table_lifecycle::{
    assert_failure, assert_ok, error_code, minimal_schema, settle_wait, table_arn, Emulator,
};

/// A KMS key ARN for the `CUSTOMER_MANAGED_KMS_KEY` rows — the form
/// `EncryptionSpecification$kmsKeyIdentifier` documents ("the KMS key in Amazon
/// Resource Name (ARN) format").
const KMS_KEY_ARN: &str = "arn:aws:kms:us-west-2:000000000000:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// The value at `path` (a JSON pointer) in `body`, or `Value::Null` when the body does
/// not carry it.
fn at(body: &Value, path: &str) -> Value {
    body.pointer(path).cloned().unwrap_or(Value::Null)
}

/// Assert a call was refused with the scaffold's 501 `NotImplemented`, which
/// [`assert_failure`] deliberately refuses to accept for a modelled failure.
fn assert_not_implemented(case: &str, got: (u16, Value)) {
    let (status, body) = got;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (501, "NotImplemented"),
        "case {case:?}: expected the honest 501 NotImplemented, got {status} {body}"
    );
}

/// Create `table` in `keyspace` from `request` (the names and a default schema are
/// filled in), then hand back the request as sent.
async fn create_table(emulator: &Emulator, keyspace: &str, table: &str, request: Value) -> Value {
    let mut request = request;
    request["keyspaceName"] = json!(keyspace);
    request["tableName"] = json!(table);
    if request.get("schemaDefinition").is_none() {
        request["schemaDefinition"] = minimal_schema("pk");
    }
    assert_ok(table, emulator.call("CreateTable", request.clone()).await);
    request
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

/// The epoch-seconds window a call ran in, as the JSON timestamps on this protocol are
/// compared against.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs_f64()
}

/// One row of RULE 1: a table to create, the single setting to update it with, the
/// `GetTable` members that may differ afterwards, and what they must then say.
struct Case {
    /// The case name, which is also the table name.
    name: &'static str,
    /// The `CreateTable` request's optional members.
    create: fn() -> Value,
    /// The `UpdateTable` request's single setting.
    update: fn() -> Value,
    /// The top-level `GetTable` members this update is allowed to change. Every other
    /// member must read back byte for byte as it did before the call.
    touched: &'static [&'static str],
    /// `(JSON pointer, expected value)` pairs the updated table must report.
    expect: fn() -> Vec<(&'static str, Value)>,
    /// Whether this update is a move to `PAY_PER_REQUEST`, which is the one thing that
    /// stamps `lastUpdateToPayPerRequestTimestamp`.
    stamps_pay_per_request: bool,
}

/// The rows: one per setting `UpdateTable` serves here, and more than one row for the
/// settings that carry a value, so the rule is asserted across the class and not on a
/// single example.
fn cases() -> Vec<Case> {
    vec![
        Case {
            // One added column, the smallest `addColumns` the model accepts (`len 1..`).
            name: "add_one_column",
            create: || json!({}),
            update: || json!({"addColumns": [{"name": "added_at", "type": "timestamp"}]}),
            touched: &["schemaDefinition"],
            expect: || {
                vec![(
                    "/schemaDefinition/allColumns",
                    json!([
                        {"name": "pk", "type": "text"},
                        {"name": "payload", "type": "int"},
                        {"name": "added_at", "type": "timestamp"},
                    ]),
                )]
            },
            stamps_pay_per_request: false,
        },
        Case {
            // Several added columns at once, spanning scalar, collection and frozen
            // Amazon Keyspaces data types.
            name: "add_several_columns",
            create: || json!({}),
            update: || {
                json!({"addColumns": [
                    {"name": "owner", "type": "uuid"},
                    {"name": "labels", "type": "map<text, text>"},
                    {"name": "history", "type": "frozen<list<int>>"},
                ]})
            },
            touched: &["schemaDefinition"],
            expect: || {
                vec![(
                    "/schemaDefinition/allColumns",
                    json!([
                        {"name": "pk", "type": "text"},
                        {"name": "payload", "type": "int"},
                        {"name": "owner", "type": "uuid"},
                        {"name": "labels", "type": "map<text, text>"},
                        {"name": "history", "type": "frozen<list<int>>"},
                    ]),
                )]
            },
            stamps_pay_per_request: false,
        },
        Case {
            // PAY_PER_REQUEST -> PROVISIONED, with the capacity units that mode
            // "requires ... as input" at the bottom of their modelled `1..` range.
            name: "capacity_to_provisioned_minimum_units",
            create: || json!({}),
            update: || {
                json!({"capacitySpecification": {
                    "throughputMode": "PROVISIONED",
                    "readCapacityUnits": 1,
                    "writeCapacityUnits": 1,
                }})
            },
            touched: &["capacitySpecification"],
            expect: || {
                vec![(
                    "/capacitySpecification",
                    json!({
                        "throughputMode": "PROVISIONED",
                        "readCapacityUnits": 1,
                        "writeCapacityUnits": 1,
                    }),
                )]
            },
            stamps_pay_per_request: false,
        },
        Case {
            // PROVISIONED -> PROVISIONED with different units: the mode did not move to
            // PAY_PER_REQUEST, so nothing is stamped.
            name: "capacity_units_changed_in_place",
            create: || {
                json!({"capacitySpecification": {
                    "throughputMode": "PROVISIONED",
                    "readCapacityUnits": 7,
                    "writeCapacityUnits": 3,
                }})
            },
            update: || {
                json!({"capacitySpecification": {
                    "throughputMode": "PROVISIONED",
                    "readCapacityUnits": 4000,
                    "writeCapacityUnits": 90,
                }})
            },
            touched: &["capacitySpecification"],
            expect: || {
                vec![(
                    "/capacitySpecification",
                    json!({
                        "throughputMode": "PROVISIONED",
                        "readCapacityUnits": 4000,
                        "writeCapacityUnits": 90,
                    }),
                )]
            },
            stamps_pay_per_request: false,
        },
        Case {
            // PROVISIONED -> PAY_PER_REQUEST: the capacity units belong to the mode that
            // was left, so they go with it, and the move is stamped.
            name: "capacity_to_pay_per_request",
            create: || {
                json!({"capacitySpecification": {
                    "throughputMode": "PROVISIONED",
                    "readCapacityUnits": 7,
                    "writeCapacityUnits": 3,
                }})
            },
            update: || json!({"capacitySpecification": {"throughputMode": "PAY_PER_REQUEST"}}),
            touched: &["capacitySpecification"],
            expect: || {
                vec![
                    (
                        "/capacitySpecification/throughputMode",
                        json!("PAY_PER_REQUEST"),
                    ),
                    ("/capacitySpecification/readCapacityUnits", Value::Null),
                    ("/capacitySpecification/writeCapacityUnits", Value::Null),
                ]
            },
            stamps_pay_per_request: true,
        },
        Case {
            // PAY_PER_REQUEST re-set on a table already in it: no throughput capacity
            // changed, so nothing is stamped.
            name: "capacity_mode_reset_in_place",
            create: || json!({}),
            update: || json!({"capacitySpecification": {"throughputMode": "PAY_PER_REQUEST"}}),
            touched: &["capacitySpecification"],
            expect: || {
                vec![(
                    "/capacitySpecification/throughputMode",
                    json!("PAY_PER_REQUEST"),
                )]
            },
            stamps_pay_per_request: false,
        },
        Case {
            // AWS_OWNED_KMS_KEY -> CUSTOMER_MANAGED_KMS_KEY, which takes the key ARN.
            name: "encryption_to_customer_managed",
            create: || json!({}),
            update: || {
                json!({"encryptionSpecification": {
                    "type": "CUSTOMER_MANAGED_KMS_KEY",
                    "kmsKeyIdentifier": KMS_KEY_ARN,
                }})
            },
            touched: &["encryptionSpecification"],
            expect: || {
                vec![(
                    "/encryptionSpecification",
                    json!({
                        "type": "CUSTOMER_MANAGED_KMS_KEY",
                        "kmsKeyIdentifier": KMS_KEY_ARN,
                    }),
                )]
            },
            stamps_pay_per_request: false,
        },
        Case {
            // CUSTOMER_MANAGED_KMS_KEY -> AWS_OWNED_KMS_KEY, the other direction: the
            // key identifier is input for the customer managed type only, so it goes.
            name: "encryption_to_aws_owned",
            create: || {
                json!({"encryptionSpecification": {
                    "type": "CUSTOMER_MANAGED_KMS_KEY",
                    "kmsKeyIdentifier": KMS_KEY_ARN,
                }})
            },
            update: || json!({"encryptionSpecification": {"type": "AWS_OWNED_KMS_KEY"}}),
            touched: &["encryptionSpecification"],
            expect: || vec![("/encryptionSpecification", json!({"type": "AWS_OWNED_KMS_KEY"}))],
            stamps_pay_per_request: false,
        },
        Case {
            // DISABLED -> ENABLED: continuous backups start now, so the table gains an
            // earliest restorable point.
            name: "point_in_time_recovery_enabled",
            create: || json!({}),
            update: || json!({"pointInTimeRecovery": {"status": "ENABLED"}}),
            touched: &["pointInTimeRecovery"],
            expect: || vec![("/pointInTimeRecovery/status", json!("ENABLED"))],
            stamps_pay_per_request: false,
        },
        Case {
            // ENABLED -> DISABLED, the other published value of the pair.
            name: "point_in_time_recovery_disabled",
            create: || json!({"pointInTimeRecovery": {"status": "ENABLED"}}),
            update: || json!({"pointInTimeRecovery": {"status": "DISABLED"}}),
            touched: &["pointInTimeRecovery"],
            expect: || {
                vec![
                    ("/pointInTimeRecovery/status", json!("DISABLED")),
                    (
                        "/pointInTimeRecovery/earliestRestorableTimestamp",
                        Value::Null,
                    ),
                ]
            },
            stamps_pay_per_request: false,
        },
        Case {
            // `ttl` is `status:enabled` and nothing else: `TimeToLiveStatus` models the
            // single value ENABLED, which is why this setting has one row.
            name: "ttl_enabled",
            create: || json!({}),
            update: || json!({"ttl": {"status": "ENABLED"}}),
            touched: &["ttl"],
            expect: || vec![("/ttl", json!({"status": "ENABLED"}))],
            stamps_pay_per_request: false,
        },
        Case {
            // `defaultTimeToLive` at the bottom of its modelled `0..630720000` range.
            name: "default_ttl_minimum",
            create: || json!({"defaultTimeToLive": 86400}),
            update: || json!({"defaultTimeToLive": 0}),
            touched: &["defaultTimeToLive"],
            expect: || vec![("/defaultTimeToLive", json!(0))],
            stamps_pay_per_request: false,
        },
        Case {
            // ... and at the top of it.
            name: "default_ttl_maximum",
            create: || json!({}),
            update: || json!({"defaultTimeToLive": 630_720_000}),
            touched: &["defaultTimeToLive"],
            expect: || vec![("/defaultTimeToLive", json!(630_720_000))],
            stamps_pay_per_request: false,
        },
        Case {
            // `ClientSideTimestampsStatus` models the single value ENABLED too.
            name: "client_side_timestamps_enabled",
            create: || json!({}),
            update: || json!({"clientSideTimestamps": {"status": "ENABLED"}}),
            touched: &["clientSideTimestamps"],
            expect: || vec![("/clientSideTimestamps", json!({"status": "ENABLED"}))],
            stamps_pay_per_request: false,
        },
    ]
}

/// RULES 1, 2 and 3: every row's setting lands on the stored table, nothing else moves,
/// the table is `UPDATING` until the clock owes it `ACTIVE`, and the move to
/// `PAY_PER_REQUEST` is the one update that stamps a timestamp.
#[tokio::test]
async fn update_table_applies_the_one_named_setting_and_leaves_the_rest() {
    let emulator = Emulator::start();
    let keyspace = "update_ks";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );

    let cases = cases();
    for case in &cases {
        create_table(&emulator, keyspace, case.name, (case.create)()).await;
    }
    // One wait for every table's CREATING span: a table is updatable only once ACTIVE.
    tokio::time::sleep(settle_wait()).await;

    // Phase one: the state each update is applied to, then the update itself.
    let mut before: Vec<Value> = Vec::with_capacity(cases.len());
    let update_floor = SystemTime::now();
    for case in &cases {
        let got = get_table(&emulator, keyspace, case.name).await;
        assert_eq!(
            got["status"],
            json!(keyspaces::TABLE_STATUS_ACTIVE),
            "case {:?}: the creation span has elapsed, so the table is ACTIVE: {got}",
            case.name
        );

        let mut request = (case.update)();
        request["keyspaceName"] = json!(keyspace);
        request["tableName"] = json!(case.name);
        let updated = assert_ok(case.name, emulator.call("UpdateTable", request).await);
        assert_eq!(
            updated["resourceArn"],
            json!(table_arn(keyspace, case.name)),
            "case {:?}: UpdateTable answers with the ARN of the table it modified, got {updated}",
            case.name
        );

        // RULE 2: the change is accepted, not yet complete, and the status says so by
        // the very next call.
        let updating = get_table(&emulator, keyspace, case.name).await;
        let status = updating["status"].as_str().unwrap_or_default().to_string();
        assert!(
            TABLE_STATUS_VALUES.contains(&status.as_str()),
            "case {:?}: status {status:?} is not a modelled TableStatus value",
            case.name
        );
        assert_eq!(
            status,
            keyspaces::TABLE_STATUS_UPDATING,
            "case {:?}: a table whose update was just accepted is UPDATING: {updating}",
            case.name
        );

        // RULE 2's other half: while the first change is still settling, a second one is
        // the modelled conflict.
        let mut again = (case.update)();
        again["keyspaceName"] = json!(keyspace);
        again["tableName"] = json!(case.name);
        assert_failure(
            case.name,
            emulator.call("UpdateTable", again).await,
            409,
            "ConflictException",
        );

        before.push(got);
    }
    let update_ceiling = SystemTime::now();

    // Phase two: the UPDATING span has elapsed, so every table is ACTIVE again and the
    // change it was given is what it reports.
    tokio::time::sleep(settle_wait()).await;
    for (case, was) in cases.iter().zip(&before) {
        let got = get_table(&emulator, keyspace, case.name).await;
        assert_eq!(
            got["status"],
            json!(keyspaces::TABLE_STATUS_ACTIVE),
            "case {:?}: the update span has elapsed, so the table is ACTIVE: {got}",
            case.name
        );

        for (path, expected) in (case.expect)() {
            assert_eq!(
                at(&got, path),
                expected,
                "case {:?}: expected {path} to be {expected}, got {got}",
                case.name
            );
        }

        // RULE 1: ONE setting per update, so every member the row does not name reads
        // back exactly as it did before the call.
        let members: Vec<String> = was
            .as_object()
            .expect("GetTable answers a JSON object")
            .keys()
            .chain(got.as_object().expect("GetTable answers a JSON object").keys())
            .cloned()
            .collect();
        for member in members {
            if case.touched.contains(&member.as_str()) || member == "status" {
                continue;
            }
            assert_eq!(
                got[&member], was[&member],
                "case {:?}: member {member} must not change on an update that does not \
                 name it: was {}, got {}",
                case.name, was[&member], got[&member]
            );
        }

        // RULE 3: the stamp exists exactly on a table an update moved to
        // PAY_PER_REQUEST, and it records WHEN that update ran.
        let stamp = at(&got, "/capacitySpecification/lastUpdateToPayPerRequestTimestamp");
        assert_eq!(
            !stamp.is_null(),
            case.stamps_pay_per_request,
            "case {:?}: lastUpdateToPayPerRequestTimestamp belongs to a move to \
             PAY_PER_REQUEST only, got {got}",
            case.name
        );
        if case.stamps_pay_per_request {
            let stamped = stamp.as_f64().unwrap_or_else(|| {
                panic!(
                    "case {:?}: lastUpdateToPayPerRequestTimestamp must be a timestamp: {got}",
                    case.name
                )
            });
            let floor = epoch_seconds(update_floor);
            let ceiling = epoch_seconds(update_ceiling);
            assert!(
                stamped >= floor - 1.0 && stamped <= ceiling + 1.0,
                "case {:?}: lastUpdateToPayPerRequestTimestamp {stamped} is outside the \
                 window [{floor}, {ceiling}] in which the update ran",
                case.name
            );
        }
    }
}

/// RULE 2 for a table that has never been `ACTIVE`: a table whose creation is still
/// settling cannot be updated, and the refusal does not disturb it.
#[tokio::test]
async fn updating_a_table_that_is_not_active_is_the_modelled_conflict() {
    let emulator = Emulator::start();
    let keyspace = "conflict_ks";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    create_table(&emulator, keyspace, "creating_tbl", json!({})).await;

    assert_failure(
        "update a table that is still CREATING",
        emulator
            .call(
                "UpdateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": "creating_tbl",
                    "defaultTimeToLive": 60,
                }),
            )
            .await,
        409,
        "ConflictException",
    );
    let refused = get_table(&emulator, keyspace, "creating_tbl").await;
    assert_eq!(
        refused["status"],
        json!(keyspaces::TABLE_STATUS_CREATING),
        "a refused update must not move the table's status: {refused}"
    );
    assert!(
        refused["defaultTimeToLive"].is_null(),
        "a refused update must not apply its setting: {refused}"
    );

    // Once the creation has settled the same call is accepted, so what was refused was
    // the status and nothing about the request itself.
    tokio::time::sleep(settle_wait()).await;
    assert_ok(
        "the same update on the now ACTIVE table",
        emulator
            .call(
                "UpdateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": "creating_tbl",
                    "defaultTimeToLive": 60,
                }),
            )
            .await,
    );

    // A table whose deletion is settling is not updatable either.
    tokio::time::sleep(settle_wait()).await;
    assert_ok(
        "delete the table",
        emulator
            .call(
                "DeleteTable",
                json!({"keyspaceName": keyspace, "tableName": "creating_tbl"}),
            )
            .await,
    );
    assert_failure(
        "update a table that is DELETING",
        emulator
            .call(
                "UpdateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": "creating_tbl",
                    "defaultTimeToLive": 120,
                }),
            )
            .await,
        409,
        "ConflictException",
    );
}

/// RULE 4: every request this slice refuses, with the status and code the model declares
/// for it — and the table is byte for byte what it was before each refusal.
#[tokio::test]
async fn a_refused_update_answers_the_modelled_error_and_changes_nothing() {
    let emulator = Emulator::start();
    let keyspace = "refusal_ks";
    let table = "refusal_tbl";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    create_table(
        &emulator,
        keyspace,
        table,
        json!({"capacitySpecification": {
            "throughputMode": "PROVISIONED",
            "readCapacityUnits": 7,
            "writeCapacityUnits": 3,
        }}),
    )
    .await;
    tokio::time::sleep(settle_wait()).await;
    let untouched = get_table(&emulator, keyspace, table).await;

    // Each row: why it is refused, the request's members beyond the two names, and the
    // status and code the model declares for that refusal.
    let refusals: Vec<(&str, Value, u16, &str)> = vec![
        // "Note that you can only update one specific table setting per update
        // operation" — so none is not an update, and two is not one setting.
        ("no setting at all", json!({}), 400, "ValidationException"),
        (
            "two settings: a column and a ttl",
            json!({
                "addColumns": [{"name": "extra", "type": "int"}],
                "ttl": {"status": "ENABLED"},
            }),
            400,
            "ValidationException",
        ),
        (
            "two settings: capacity and default ttl",
            json!({
                "capacitySpecification": {"throughputMode": "PAY_PER_REQUEST"},
                "defaultTimeToLive": 60,
            }),
            400,
            "ValidationException",
        ),
        (
            "two settings: encryption and point-in-time recovery",
            json!({
                "encryptionSpecification": {"type": "AWS_OWNED_KMS_KEY"},
                "pointInTimeRecovery": {"status": "ENABLED"},
            }),
            400,
            "ValidationException",
        ),
        // A column holds ONE name: `allColumns` declares each column of the table once.
        (
            "a column the table already declares",
            json!({"addColumns": [{"name": "payload", "type": "int"}]}),
            400,
            "ValidationException",
        ),
        (
            "a column the request itself repeats",
            json!({"addColumns": [
                {"name": "twice", "type": "int"},
                {"name": "twice", "type": "text"},
            ]}),
            400,
            "ValidationException",
        ),
        // "throughputMode:PROVISIONED - Provisioned capacity mode requires
        // readCapacityUnits and writeCapacityUnits as input", and no other mode takes
        // them.
        (
            "PROVISIONED without its capacity units",
            json!({"capacitySpecification": {"throughputMode": "PROVISIONED"}}),
            400,
            "ValidationException",
        ),
        (
            "PROVISIONED missing one capacity unit",
            json!({"capacitySpecification": {
                "throughputMode": "PROVISIONED",
                "readCapacityUnits": 5,
            }}),
            400,
            "ValidationException",
        ),
        (
            "PAY_PER_REQUEST carrying capacity units",
            json!({"capacitySpecification": {
                "throughputMode": "PAY_PER_REQUEST",
                "readCapacityUnits": 5,
                "writeCapacityUnits": 5,
            }}),
            400,
            "ValidationException",
        ),
        // "type:CUSTOMER_MANAGED_KMS_KEY ... requires the kms_key_identifier of the KMS
        // key in Amazon Resource Name (ARN) format as input".
        (
            "a customer managed key without its identifier",
            json!({"encryptionSpecification": {"type": "CUSTOMER_MANAGED_KMS_KEY"}}),
            400,
            "ValidationException",
        ),
        // Modelled constraints, refused at the boundary before the handler runs:
        // `defaultTimeToLive` is `range 0..630720000` and `addColumns` is `len 1..`.
        (
            "defaultTimeToLive one below its range",
            json!({"defaultTimeToLive": -1}),
            400,
            "ValidationException",
        ),
        (
            "defaultTimeToLive one above its range",
            json!({"defaultTimeToLive": 630_720_001}),
            400,
            "ValidationException",
        ),
        (
            "an empty addColumns list",
            json!({"addColumns": []}),
            400,
            "ValidationException",
        ),
        // `TimeToLiveStatus` and `ClientSideTimestampsStatus` each model exactly one
        // value, so the disable the documentation calls impossible is refused at the
        // boundary: "After ttl is enabled, you can't disable it for the table".
        (
            "ttl disabled, which the model has no value for",
            json!({"ttl": {"status": "DISABLED"}}),
            400,
            "ValidationException",
        ),
        (
            "client-side timestamps disabled",
            json!({"clientSideTimestamps": {"status": "DISABLED"}}),
            400,
            "ValidationException",
        ),
    ];

    for (case, members, status, code) in refusals {
        let mut request = members;
        request["keyspaceName"] = json!(keyspace);
        request["tableName"] = json!(table);
        assert_failure(case, emulator.call("UpdateTable", request).await, status, code);
        assert_eq!(
            get_table(&emulator, keyspace, table).await,
            untouched,
            "case {case:?}: a refused update must leave the table exactly as it was"
        );
    }

    // An absent keyspace, and an absent table in a keyspace that exists, are the
    // modelled not-found rather than a silent create.
    for (case, keyspace_name, table_name) in [
        ("an absent keyspace", "no_such_ks", table),
        ("an absent table", keyspace, "no_such_tbl"),
    ] {
        assert_failure(
            case,
            emulator
                .call(
                    "UpdateTable",
                    json!({
                        "keyspaceName": keyspace_name,
                        "tableName": table_name,
                        "defaultTimeToLive": 60,
                    }),
                )
                .await,
            404,
            "ResourceNotFoundException",
        );
    }
}

/// The store's column dimension is bounded: a stored schema only ever grows, so a
/// request that would take it past [`keyspaces::MAX_COLUMNS_PER_TABLE`] is refused with
/// the quota error this operation models, whether it does so in one call or across
/// several.
#[tokio::test]
async fn growing_a_schema_past_the_column_cap_is_the_modelled_quota_error() {
    /// `count` columns named apart from the default schema's, from `first` onwards.
    fn columns(first: usize, count: usize) -> Value {
        Value::Array(
            (first..first + count)
                .map(|index| json!({"name": format!("c{index}"), "type": "int"}))
                .collect(),
        )
    }

    let emulator = Emulator::start();
    let keyspace = "quota_ks";
    let table = "quota_tbl";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    create_table(&emulator, keyspace, table, json!({})).await;
    tokio::time::sleep(settle_wait()).await;

    // The default schema already holds two columns, so this is the room left in it.
    let room = keyspaces::MAX_COLUMNS_PER_TABLE - 2;

    // One request past the cap is refused, and refused BEFORE anything is stored.
    assert_failure(
        "one update past the column cap",
        emulator
            .call(
                "UpdateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "addColumns": columns(0, room + 1),
                }),
            )
            .await,
        402,
        "ServiceQuotaExceededException",
    );
    let refused = get_table(&emulator, keyspace, table).await;
    assert_eq!(
        refused["schemaDefinition"]["allColumns"]
            .as_array()
            .map(Vec::len),
        Some(2),
        "a refused update must not append any of its columns: {refused}"
    );

    // Filling the schema exactly to the cap is accepted.
    assert_ok(
        "fill the schema to the cap",
        emulator
            .call(
                "UpdateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "addColumns": columns(0, room),
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;
    let full = get_table(&emulator, keyspace, table).await;
    let full_columns = full["schemaDefinition"]["allColumns"]
        .as_array()
        .map(Vec::len);
    assert_eq!(
        full_columns,
        Some(keyspaces::MAX_COLUMNS_PER_TABLE),
        "the schema must hold the cap's worth of columns, got {full_columns:?}"
    );

    // ... and the next column, however it is named, is refused: the cap is cumulative,
    // not per request.
    assert_failure(
        "one column past a full schema",
        emulator
            .call(
                "UpdateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "addColumns": columns(room, 1),
                }),
            )
            .await,
        402,
        "ServiceQuotaExceededException",
    );

    // The same cap bounds the other end of the dimension: a create cannot start a table
    // past it either.
    let mut all_columns = columns(0, keyspaces::MAX_COLUMNS_PER_TABLE + 1);
    all_columns[0] = json!({"name": "pk", "type": "text"});
    assert_failure(
        "create a table past the column cap",
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": "too_wide_tbl",
                    "schemaDefinition": {
                        "allColumns": all_columns,
                        "partitionKeys": [{"name": "pk"}],
                    },
                }),
            )
            .await,
        402,
        "ServiceQuotaExceededException",
    );
}

/// RULE 5: the one setting no operation in this slice can report is refused with the honest
/// 501, never stored, and never answered as a success.
#[tokio::test]
async fn a_setting_this_slice_cannot_report_is_refused_with_the_scaffold_501() {
    let emulator = Emulator::start();
    let keyspace = "unimplemented_ks";
    let table = "unimplemented_tbl";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    create_table(&emulator, keyspace, table, json!({})).await;
    tokio::time::sleep(settle_wait()).await;
    let untouched = get_table(&emulator, keyspace, table).await;

    // `cdcSpecification` alone: `GetTable` has no CDC summary to report, so storing it
    // would mean answering for a setting nothing here can read back.
    let unimplemented: Vec<(&str, Value)> = vec![(
        "cdcSpecification",
        json!({"cdcSpecification": {"status": "ENABLED"}}),
    )];

    for (case, members) in unimplemented {
        let mut request = members;
        request["keyspaceName"] = json!(keyspace);
        request["tableName"] = json!(table);
        assert_not_implemented(case, emulator.call("UpdateTable", request).await);
        assert_eq!(
            get_table(&emulator, keyspace, table).await,
            untouched,
            "case {case:?}: a 501 must leave the table exactly as it was"
        );
    }
}
