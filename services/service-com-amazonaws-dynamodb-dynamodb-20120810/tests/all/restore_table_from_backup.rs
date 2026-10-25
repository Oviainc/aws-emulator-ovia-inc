// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `RestoreTableFromBackup`.
//!
//! The rules pinned here:
//!
//! 1. a restore CREATES a table from the backup: the schema, the capacity mode, the
//!    secondary indexes and the ITEMS the backup recorded become a new table under the
//!    name the request names, with its own `TableId` — and the overrides the request
//!    carries (`BillingModeOverride`, `ProvisionedThroughputOverride` and the two
//!    secondary-index overrides) are applied to it;
//! 2. the restored table is `CREATING` until the clock makes it `ACTIVE`, and for that
//!    span its `RestoreSummary` reports the restore in progress, naming the backup it came
//!    from, the table that backup was taken of and the backup's own instant;
//! 3. what AWS says must be set up again on a restored table does NOT carry over —
//!    "Tags Stream settings Time to Live (TTL) settings" (the `RestoreTableFromBackup`
//!    doc) — and the SOURCE table and the backup are left exactly as they were;
//! 4. a backup a control-plane call still has is not one to restore from, and a name a
//!    table already holds is not one to restore onto: each refusal is the modelled error
//!    for that conflict;
//! 5. an override that contradicts the restored table's capacity mode, or names an index
//!    the backup never recorded, is a `ValidationException`.
//!
//! Every test drives the operations over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, and each rule is asserted over a
//! SET of inputs — the source table's capacity mode, key schema and indexes, the items it
//! held, and the overrides the restore carries — rather than one example.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, call_in, emulator, emulator_with_streams, error_code, message, ok};

/// Wait out one transient span, so whatever the clock owes a table or a backup is due.
async fn settle() {
    tokio::time::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(150),
    )
    .await;
}

/// Create one table and wait out its `CREATING` span: "You can perform read and write
/// operations only on an ACTIVE table", and a backup is taken of a settled table.
async fn active_table(router: &MultiServiceRouter, name: &str, create: &Value) {
    let mut body = create.clone();
    body["TableName"] = json!(name);
    ok(router, "CreateTable", body).await;
    settle().await;
}

/// Take one backup of `table` and wait out its `CREATING` span, so it is `AVAILABLE` to
/// restore from. Returns the `BackupDetails` the create reported.
async fn available_backup(router: &MultiServiceRouter, table: &str, name: &str) -> Value {
    let details = ok(
        router,
        "CreateBackup",
        json!({"TableName": table, "BackupName": name}),
    )
    .await["BackupDetails"]
        .clone();
    settle().await;
    details
}

/// The `BackupArn` of a `BackupDetails`.
fn backup_arn(details: &Value) -> String {
    details["BackupArn"].as_str().unwrap_or_default().to_string()
}

/// The stored table as `DescribeTable` reports it.
async fn described(router: &MultiServiceRouter, name: &str) -> Value {
    ok(router, "DescribeTable", json!({"TableName": name})).await["Table"].clone()
}

/// The primary key of `item`, read off the key schema the create declared — the same
/// projection a client makes to build a `GetItem` key.
fn key_of(create: &Value, item: &Value) -> Value {
    let mut key = Map::new();
    for element in create["KeySchema"].as_array().cloned().unwrap_or_default() {
        let name = element["AttributeName"].as_str().unwrap_or_default().to_string();
        key.insert(name.clone(), item[name.as_str()].clone());
    }
    Value::Object(key)
}

/// The `IndexName`s a description reports for one index member, in the order reported.
fn index_names(description: &Value, member: &str) -> Vec<String> {
    description[member]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|index| index["IndexName"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The reported `(ReadCapacityUnits, WriteCapacityUnits)` pair of a description.
fn reported_capacity(description: &Value) -> (i64, i64) {
    (
        description["ProvisionedThroughput"]["ReadCapacityUnits"]
            .as_i64()
            .unwrap_or(-1),
        description["ProvisionedThroughput"]["WriteCapacityUnits"]
            .as_i64()
            .unwrap_or(-1),
    )
}

/// A source table with two global secondary indexes, provisioned.
fn provisioned_with_indexes() -> Value {
    json!({
        "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
        "AttributeDefinitions": [
            {"AttributeName": "Part", "AttributeType": "S"},
            {"AttributeName": "Colour", "AttributeType": "S"},
            {"AttributeName": "Size", "AttributeType": "N"},
        ],
        "BillingMode": "PROVISIONED",
        "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 7},
        "GlobalSecondaryIndexes": [
            {
                "IndexName": "ByColour",
                "KeySchema": [{"AttributeName": "Colour", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
                "ProvisionedThroughput": {"ReadCapacityUnits": 2, "WriteCapacityUnits": 3},
            },
            {
                "IndexName": "BySize",
                "KeySchema": [{"AttributeName": "Size", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "KEYS_ONLY"},
                "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
            },
        ],
    })
}

/// A source table with a composite key and one local secondary index.
fn provisioned_with_local_index() -> Value {
    json!({
        "KeySchema": [
            {"AttributeName": "Part", "KeyType": "HASH"},
            {"AttributeName": "Sort", "KeyType": "RANGE"},
        ],
        "AttributeDefinitions": [
            {"AttributeName": "Part", "AttributeType": "S"},
            {"AttributeName": "Sort", "AttributeType": "N"},
            {"AttributeName": "Colour", "AttributeType": "S"},
        ],
        "BillingMode": "PROVISIONED",
        "ProvisionedThroughput": {"ReadCapacityUnits": 4, "WriteCapacityUnits": 4},
        "LocalSecondaryIndexes": [{
            "IndexName": "ByColour",
            "KeySchema": [
                {"AttributeName": "Part", "KeyType": "HASH"},
                {"AttributeName": "Colour", "KeyType": "RANGE"},
            ],
            "Projection": {"ProjectionType": "ALL"},
        }],
    })
}

/// An on-demand source table with a simple key.
fn on_demand_simple() -> Value {
    json!({
        "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
        "BillingMode": "PAY_PER_REQUEST",
    })
}

/// One row of the restore table: the case, the source table's `CreateTable` body, the
/// items it held, the overrides the restore carries, and what the restored table must
/// then report — its capacity mode, its reported capacity pair, its global and its local
/// secondary indexes.
type RestoreCase = (
    &'static str,
    Value,
    Vec<Value>,
    Value,
    &'static str,
    (i64, i64),
    Vec<&'static str>,
    Vec<&'static str>,
);

fn restore_cases() -> Vec<RestoreCase> {
    vec![
        (
            "on-demand simple key, no override",
            on_demand_simple(),
            vec![
                json!({"Part": {"S": "a"}, "Note": {"S": "first"}}),
                json!({"Part": {"S": "b"}, "Count": {"N": "17"}}),
            ],
            json!({}),
            "PAY_PER_REQUEST",
            (0, 0),
            vec![],
            vec![],
        ),
        (
            "empty on-demand table, no override",
            on_demand_simple(),
            vec![],
            json!({}),
            "PAY_PER_REQUEST",
            (0, 0),
            vec![],
            vec![],
        ),
        (
            "provisioned with two global indexes, no override",
            provisioned_with_indexes(),
            vec![
                json!({"Part": {"S": "a"}, "Colour": {"S": "red"}, "Size": {"N": "3"}}),
                json!({"Part": {"S": "b"}, "Colour": {"S": "blue"}, "Size": {"N": "4"}}),
                json!({"Part": {"S": "c"}}),
            ],
            json!({}),
            "PROVISIONED",
            (5, 7),
            vec!["ByColour", "BySize"],
            vec![],
        ),
        (
            "global index override selects one of the two recorded indexes",
            provisioned_with_indexes(),
            vec![json!({"Part": {"S": "a"}, "Colour": {"S": "red"}, "Size": {"N": "3"}})],
            json!({"GlobalSecondaryIndexOverride": [{
                "IndexName": "BySize",
                "KeySchema": [{"AttributeName": "Size", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "KEYS_ONLY"},
                "ProvisionedThroughput": {"ReadCapacityUnits": 6, "WriteCapacityUnits": 6},
            }]}),
            "PROVISIONED",
            (5, 7),
            vec!["BySize"],
            vec![],
        ),
        (
            "global index override excludes every recorded index",
            provisioned_with_indexes(),
            vec![json!({"Part": {"S": "a"}, "Colour": {"S": "red"}, "Size": {"N": "3"}})],
            json!({"GlobalSecondaryIndexOverride": []}),
            "PROVISIONED",
            (5, 7),
            vec![],
            vec![],
        ),
        (
            "composite key with a local index, restored on-demand",
            provisioned_with_local_index(),
            vec![
                json!({"Part": {"S": "a"}, "Sort": {"N": "1"}, "Colour": {"S": "red"}}),
                json!({"Part": {"S": "a"}, "Sort": {"N": "2"}, "Colour": {"S": "blue"}}),
            ],
            json!({"BillingModeOverride": "PAY_PER_REQUEST"}),
            "PAY_PER_REQUEST",
            (0, 0),
            vec![],
            vec!["ByColour"],
        ),
        (
            "local index override excludes the recorded local index",
            provisioned_with_local_index(),
            vec![json!({"Part": {"S": "a"}, "Sort": {"N": "1"}, "Colour": {"S": "red"}})],
            json!({"LocalSecondaryIndexOverride": []}),
            "PROVISIONED",
            (4, 4),
            vec![],
            vec![],
        ),
        (
            "on-demand backup restored provisioned with a throughput override",
            on_demand_simple(),
            vec![json!({"Part": {"S": "a"}, "Note": {"S": "kept"}})],
            json!({
                "BillingModeOverride": "PROVISIONED",
                "ProvisionedThroughputOverride": {
                    "ReadCapacityUnits": 3,
                    "WriteCapacityUnits": 11,
                },
            }),
            "PROVISIONED",
            (3, 11),
            vec![],
            vec![],
        ),
    ]
}

#[tokio::test]
async fn a_restore_creates_the_table_the_backup_recorded() {
    let router = emulator();

    for (row, (case, create, items, overrides, billing_mode, capacity, global, local)) in
        restore_cases().into_iter().enumerate()
    {
        let source = format!("Source-{row}");
        let target = format!("Restored-{row}");
        active_table(&router, &source, &create).await;
        for item in &items {
            ok(
                &router,
                "PutItem",
                json!({"TableName": source, "Item": item}),
            )
            .await;
        }
        let described_source = described(&router, &source).await;
        let details = available_backup(&router, &source, "nightly").await;
        let arn = backup_arn(&details);

        let mut request = overrides.clone();
        request["TargetTableName"] = json!(target);
        request["BackupArn"] = json!(arn);
        let restored = ok(&router, "RestoreTableFromBackup", request).await
            ["TableDescription"]
            .clone();

        // The restored table is the backup's schema under the requested name, with its
        // own identity.
        assert_eq!(
            restored["TableName"],
            json!(target),
            "case {case}: expected the restored table to be named {target}, got {restored}"
        );
        assert_eq!(
            restored["KeySchema"], described_source["KeySchema"],
            "case {case}: expected the recorded key schema {}, got {}",
            described_source["KeySchema"], restored["KeySchema"]
        );
        assert_eq!(
            restored["AttributeDefinitions"], described_source["AttributeDefinitions"],
            "case {case}: expected the recorded attribute definitions {}, got {}",
            described_source["AttributeDefinitions"], restored["AttributeDefinitions"]
        );
        assert_ne!(
            restored["TableId"], described_source["TableId"],
            "case {case}: expected a restored table to be minted its own TableId, got the \
             source table's {}",
            restored["TableId"]
        );
        assert_eq!(
            restored["BillingModeSummary"]["BillingMode"],
            json!(billing_mode),
            "case {case}: expected BillingMode {billing_mode}, got {}",
            restored["BillingModeSummary"]
        );
        assert_eq!(
            reported_capacity(&restored),
            capacity,
            "case {case}: expected the reported capacity {capacity:?}, got {:?}",
            reported_capacity(&restored)
        );
        assert_eq!(
            index_names(&restored, "GlobalSecondaryIndexes"),
            global,
            "case {case}: expected the global secondary indexes {global:?}, got {:?}",
            index_names(&restored, "GlobalSecondaryIndexes")
        );
        assert_eq!(
            index_names(&restored, "LocalSecondaryIndexes"),
            local,
            "case {case}: expected the local secondary indexes {local:?}, got {:?}",
            index_names(&restored, "LocalSecondaryIndexes")
        );

        // The restore is asynchronous, so the table is still being created and its
        // summary says so, naming what it was restored from.
        assert_eq!(
            restored["TableStatus"],
            json!("CREATING"),
            "case {case}: expected a restored table to be reported CREATING, got {}",
            restored["TableStatus"]
        );
        let summary = &restored["RestoreSummary"];
        assert_eq!(
            summary["SourceBackupArn"],
            json!(arn),
            "case {case}: expected SourceBackupArn {arn}, got {summary}"
        );
        assert_eq!(
            summary["SourceTableArn"], described_source["TableArn"],
            "case {case}: expected SourceTableArn {}, got {summary}",
            described_source["TableArn"]
        );
        assert_eq!(
            summary["RestoreDateTime"], details["BackupCreationDateTime"],
            "case {case}: expected RestoreDateTime to be the backup's own instant {}, got \
             {summary}",
            details["BackupCreationDateTime"]
        );
        assert_eq!(
            summary["RestoreInProgress"],
            json!(true),
            "case {case}: expected RestoreInProgress while the table is CREATING, got \
             {summary}"
        );

        // Once the clock has made the transition due the table is ACTIVE, the restore is
        // no longer in progress, and every item the backup recorded is readable from it.
        settle().await;
        let settled = described(&router, &target).await;
        assert_eq!(
            settled["TableStatus"],
            json!("ACTIVE"),
            "case {case}: expected the restored table to become ACTIVE, got {}",
            settled["TableStatus"]
        );
        assert_eq!(
            settled["RestoreSummary"]["RestoreInProgress"],
            json!(false),
            "case {case}: expected the restore to be finished, got {}",
            settled["RestoreSummary"]
        );
        assert_eq!(
            settled["ItemCount"],
            json!(items.len()),
            "case {case}: expected ItemCount {}, got {}",
            items.len(),
            settled["ItemCount"]
        );
        for item in &items {
            let key = key_of(&create, item);
            let read = ok(
                &router,
                "GetItem",
                json!({"TableName": target, "Key": key, "ConsistentRead": true}),
            )
            .await;
            assert_eq!(
                read["Item"], *item,
                "case {case}: expected the restored table to hold {item}, got {read}"
            );
        }
        let scanned = ok(&router, "Scan", json!({"TableName": target})).await;
        assert_eq!(
            scanned["Count"],
            json!(items.len()),
            "case {case}: expected the restored table to hold {} item(s), got {scanned}",
            items.len()
        );

        // The source table and the backup are untouched by the restore.
        let source_after = described(&router, &source).await;
        assert_eq!(
            source_after["TableId"], described_source["TableId"],
            "case {case}: expected the source table to be left as it was, got \
             {source_after}"
        );
        let backup_after = ok(&router, "DescribeBackup", json!({"BackupArn": arn})).await;
        assert_eq!(
            backup_after["BackupDescription"]["BackupDetails"]["BackupStatus"],
            json!("AVAILABLE"),
            "case {case}: expected the backup to still be AVAILABLE, got {backup_after}"
        );
    }
}

#[tokio::test]
async fn a_restored_table_carries_over_no_setting_a_caller_must_set_up_again() {
    // "You must manually set up the following on the restored table: Auto scaling
    // policies IAM policies Amazon CloudWatch metrics and alarms Tags Stream settings
    // Time to Live (TTL) settings" (the `RestoreTableFromBackup` doc). The source table
    // here carries every one of those this service stores, so each assertion below is
    // about a setting that WAS set on the source.
    let router = emulator_with_streams();
    let create = json!({
        "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
        "BillingMode": "PAY_PER_REQUEST",
        "StreamSpecification": {"StreamEnabled": true, "StreamViewType": "NEW_IMAGE"},
        "DeletionProtectionEnabled": true,
        "Tags": [{"Key": "owner", "Value": "platform"}],
    });
    active_table(&router, "Configured", &create).await;
    ok(
        &router,
        "UpdateContinuousBackups",
        json!({
            "TableName": "Configured",
            "PointInTimeRecoverySpecification": {"PointInTimeRecoveryEnabled": true},
        }),
    )
    .await;
    let details = available_backup(&router, "Configured", "configured").await;
    let arn = backup_arn(&details);
    ok(
        &router,
        "RestoreTableFromBackup",
        json!({"TargetTableName": "Plain", "BackupArn": arn}),
    )
    .await;
    settle().await;

    let source = described(&router, "Configured").await;
    let restored = described(&router, "Plain").await;
    for (case, member) in [
        ("the latest stream", "LatestStreamArn"),
        ("the stream specification", "StreamSpecification"),
    ] {
        assert!(
            source[member] != Value::Null && restored[member] == Value::Null,
            "case {case}: expected the source to report it and the restored table not to, \
             got {} and {}",
            source[member],
            restored[member]
        );
    }
    assert_eq!(
        restored["DeletionProtectionEnabled"],
        json!(false),
        "case deletion protection: expected a restored table not to carry it over, got {}",
        restored["DeletionProtectionEnabled"]
    );
    let tags = ok(
        &router,
        "ListTagsOfResource",
        json!({"ResourceArn": restored["TableArn"]}),
    )
    .await;
    assert_eq!(
        tags["Tags"].as_array().map_or(0, Vec::len),
        0,
        "case tags: expected a restored table to carry none of the source's tags, got {tags}"
    );
    let recovery = ok(
        &router,
        "DescribeContinuousBackups",
        json!({"TableName": "Plain"}),
    )
    .await;
    assert_eq!(
        recovery["ContinuousBackupsDescription"]["PointInTimeRecoveryDescription"]
            ["PointInTimeRecoveryStatus"],
        json!("DISABLED"),
        "case point-in-time recovery: expected a restored table to have it off, got {recovery}"
    );
}

#[tokio::test]
async fn a_restore_is_refused_while_the_backup_or_the_target_name_is_taken() {
    let router = emulator();
    active_table(&router, "Source", &on_demand_simple()).await;
    active_table(&router, "Taken", &on_demand_simple()).await;
    let details = available_backup(&router, "Source", "nightly").await;
    let arn = backup_arn(&details);
    // A well-formed ARN of this endpoint that names a backup nobody took, and a value
    // that is no backup ARN at all (long enough to satisfy the modelled length).
    let unknown = format!(
        "{}/backup/01234567890123-deadbeef",
        arn.split("/backup/").next().unwrap_or_default()
    );
    let malformed = format!("{}-not-a-backup-arn", arn.split("/backup/").next().unwrap_or_default());

    // Rows differ in WHICH conflict refuses the restore, and each is the modelled error
    // for that conflict.
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a backup nobody took",
            json!({"TargetTableName": "Fresh", "BackupArn": unknown}),
            400,
            "BackupNotFoundException",
        ),
        (
            "a value that is no backup ARN",
            json!({"TargetTableName": "Fresh", "BackupArn": malformed}),
            400,
            "ValidationException",
        ),
        (
            "a target name a settled table already holds",
            json!({"TargetTableName": "Taken", "BackupArn": arn}),
            400,
            "TableAlreadyExistsException",
        ),
    ];
    for (case, request, status, code) in cases {
        let (actual_status, body) = call(&router, "RestoreTableFromBackup", request).await;
        assert_eq!(
            actual_status, status,
            "case {case}: expected HTTP {status}, got {actual_status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case}: expected {code}, got {} with message {}",
            error_code(&body),
            message(&body)
        );
    }

    // A backup of ANOTHER region is not addressable from this endpoint at all.
    let (status, body) = call_in(
        &router,
        "us-west-2",
        "RestoreTableFromBackup",
        json!({"TargetTableName": "Fresh", "BackupArn": arn}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a backup of another region: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case a backup of another region: expected ValidationException, got {} with message {}",
        error_code(&body),
        message(&body)
    );

    // A target table that is still being created, and one already being deleted, are both
    // "either being created or deleted".
    for (case, target) in [("still creating", "Creating"), ("already deleting", "Taken")] {
        if case == "still creating" {
            ok(
                &router,
                "CreateTable",
                json!({
                    "TableName": target,
                    "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                    "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
                    "BillingMode": "PAY_PER_REQUEST",
                }),
            )
            .await;
        } else {
            ok(&router, "DeleteTable", json!({"TableName": target})).await;
        }
        let (status, body) = call(
            &router,
            "RestoreTableFromBackup",
            json!({"TargetTableName": target, "BackupArn": arn}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case a target table {case}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "TableInUseException",
            "case a target table {case}: expected TableInUseException, got {} with message {}",
            error_code(&body),
            message(&body)
        );
    }
}

#[tokio::test]
async fn a_backup_a_control_plane_call_still_has_is_not_one_to_restore_or_delete() {
    let router = emulator();
    active_table(&router, "Source", &on_demand_simple()).await;

    // A backup the clock has not finished creating is not one to restore from: the
    // create's own span is non-zero, so a caller that restores at once still sees it.
    let creating = ok(
        &router,
        "CreateBackup",
        json!({"TableName": "Source", "BackupName": "creating"}),
    )
    .await["BackupDetails"]
        .clone();
    let creating_arn = backup_arn(&creating);
    let (status, body) = call(
        &router,
        "RestoreTableFromBackup",
        json!({"TargetTableName": "FromCreating", "BackupArn": creating_arn}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a backup still being created: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "BackupInUseException",
        "case a backup still being created: expected BackupInUseException, got {} with \
         message {}",
        error_code(&body),
        message(&body)
    );
    assert_eq!(
        message(&body),
        format!("Backup is being created: {creating_arn}"),
        "case a backup still being created: expected the sentence AWS states for that \
         conflict, got {}",
        message(&body)
    );
    settle().await;

    // A backup a restore is still using is in use for every other backup control-plane
    // call, whichever one asks for it.
    let details = available_backup(&router, "Source", "nightly").await;
    let arn = backup_arn(&details);
    ok(
        &router,
        "RestoreTableFromBackup",
        json!({"TargetTableName": "FirstRestore", "BackupArn": arn}),
    )
    .await;
    for (case, operation, request) in [
        (
            "a second restore from the same backup",
            "RestoreTableFromBackup",
            json!({"TargetTableName": "SecondRestore", "BackupArn": arn}),
        ),
        (
            "a delete of the backup being restored",
            "DeleteBackup",
            json!({"BackupArn": arn}),
        ),
    ] {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "BackupInUseException",
            "case {case}: expected BackupInUseException, got {} with message {}",
            error_code(&body),
            message(&body)
        );
        // ONE sentence for that conflict, whichever call runs into it: the backup is
        // being used to restore a table.
        assert_eq!(
            message(&body),
            format!("Backup is being used to restore table: {arn}"),
            "case {case}: expected the sentence AWS states for that conflict, got {}",
            message(&body)
        );
    }

    // Once the restore has finished the backup is free again: the same delete now
    // removes it.
    settle().await;
    let removed = ok(&router, "DeleteBackup", json!({"BackupArn": arn})).await;
    assert_eq!(
        removed["BackupDescription"]["BackupDetails"]["BackupStatus"],
        json!("DELETED"),
        "case a delete once the restore has finished: expected the record DELETED, got \
         {removed}"
    );
}

#[tokio::test]
async fn an_override_that_cannot_hold_on_the_restored_table_is_refused() {
    let router = emulator();
    active_table(&router, "OnDemand", &on_demand_simple()).await;
    active_table(&router, "Provisioned", &provisioned_with_indexes()).await;
    let on_demand_arn = backup_arn(&available_backup(&router, "OnDemand", "nightly").await);
    let provisioned_arn =
        backup_arn(&available_backup(&router, "Provisioned", "nightly").await);

    // Rows differ in HOW the override and the restored table's capacity mode or the
    // backup's recorded indexes contradict each other. Each is a permanently invalid
    // request rather than a conflict to retry.
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a provisioned pair for an on-demand restore",
            json!({
                "TargetTableName": "Fresh",
                "BackupArn": on_demand_arn,
                "ProvisionedThroughputOverride": {
                    "ReadCapacityUnits": 1,
                    "WriteCapacityUnits": 1,
                },
            }),
            400,
            "ValidationException",
        ),
        (
            "a provisioned restore of an on-demand backup naming no pair",
            json!({
                "TargetTableName": "Fresh",
                "BackupArn": on_demand_arn,
                "BillingModeOverride": "PROVISIONED",
            }),
            400,
            "ValidationException",
        ),
        (
            "a provisioned pair below the documented minimum",
            json!({
                "TargetTableName": "Fresh",
                "BackupArn": on_demand_arn,
                "BillingModeOverride": "PROVISIONED",
                "ProvisionedThroughputOverride": {
                    "ReadCapacityUnits": 0,
                    "WriteCapacityUnits": 1,
                },
            }),
            400,
            "ValidationException",
        ),
        (
            "an index override naming an index the backup never recorded",
            json!({
                "TargetTableName": "Fresh",
                "BackupArn": provisioned_arn,
                "GlobalSecondaryIndexOverride": [{
                    "IndexName": "ByWeight",
                    "KeySchema": [{"AttributeName": "Weight", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                    "ProvisionedThroughput": {
                        "ReadCapacityUnits": 1,
                        "WriteCapacityUnits": 1,
                    },
                }],
            }),
            400,
            "ValidationException",
        ),
        (
            "a local index override naming an index the backup never recorded",
            json!({
                "TargetTableName": "Fresh",
                "BackupArn": provisioned_arn,
                "LocalSecondaryIndexOverride": [{
                    "IndexName": "ByWeight",
                    "KeySchema": [{"AttributeName": "Weight", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                }],
            }),
            400,
            "ValidationException",
        ),
        (
            "an index pair for an index of an on-demand restore",
            json!({
                "TargetTableName": "Fresh",
                "BackupArn": provisioned_arn,
                "BillingModeOverride": "PAY_PER_REQUEST",
                "GlobalSecondaryIndexOverride": [{
                    "IndexName": "ByColour",
                    "KeySchema": [{"AttributeName": "Colour", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                    "ProvisionedThroughput": {
                        "ReadCapacityUnits": 1,
                        "WriteCapacityUnits": 1,
                    },
                }],
            }),
            400,
            "ValidationException",
        ),
        (
            "an on-demand ceiling for a provisioned restore",
            json!({
                "TargetTableName": "Fresh",
                "BackupArn": provisioned_arn,
                "OnDemandThroughputOverride": {"MaxReadRequestUnits": 10},
            }),
            400,
            "ValidationException",
        ),
        // The two members no stored table carries yet are reported as unimplemented
        // rather than accepted and dropped — a 501 that says "nobody has written this
        // member", which no SDK branch mistakes for a modelled refusal.
        (
            "an on-demand ceiling for an on-demand restore",
            json!({
                "TargetTableName": "Fresh",
                "BackupArn": on_demand_arn,
                "OnDemandThroughputOverride": {"MaxReadRequestUnits": 10},
            }),
            501,
            "NotImplemented",
        ),
        (
            "server-side encryption settings",
            json!({
                "TargetTableName": "Fresh",
                "BackupArn": on_demand_arn,
                "SSESpecificationOverride": {"Enabled": true},
            }),
            501,
            "NotImplemented",
        ),
    ];
    for (case, request, status, code) in cases {
        let (actual_status, body) = call(&router, "RestoreTableFromBackup", request).await;
        assert_eq!(
            actual_status, status,
            "case {case}: expected HTTP {status}, got {actual_status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case}: expected {code}, got {} with message {}",
            error_code(&body),
            message(&body)
        );
        // A refused restore stores nothing: the target name is still free.
        let (described_status, described_body) =
            call(&router, "DescribeTable", json!({"TableName": "Fresh"})).await;
        assert_eq!(
            described_status, 400,
            "case {case}: expected a refused restore to store no table, got \
             {described_status} with {described_body}"
        );
    }
}
