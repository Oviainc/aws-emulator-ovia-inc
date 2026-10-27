// SPDX-License-Identifier: CC0-1.0

//! What a client that polls for a new table observes, and the backup-and-restore
//! sequence that depends on it.
//!
//! `CreateTable`, `RestoreTableFromBackup` and `RestoreTableToPointInTime` all register a
//! table the clock still owes its `CREATING` -> `ACTIVE` transition, and a client confirms
//! the table is usable by polling `DescribeTable` — the operation the SDK's own
//! `table_exists` waiter polls, at an interval orders of magnitude longer than the span
//! this emulator compresses that transition into. The rules pinned here:
//!
//! 1. the creating call's own response reports the table `CREATING`, and the FIRST
//!    `DescribeTable` sent after it — with no wait before it, which is what a waiter's
//!    first probe is — reports the table `ACTIVE`, in no more than one transition span,
//!    for every one of the three calls that create a table and whatever shape the table
//!    has;
//! 2. a transition a LATER call asks for — a delete, an update — is reported as it
//!    stands, with no wait at all: the caller asked for that change and is watching it
//!    happen;
//! 3. so the documented backup-and-restore sequence converges in one probe per table: the
//!    backup restored to a free name carries the source items, and a restore onto the name
//!    of a settled table is refused HTTP 400 `TableAlreadyExistsException` with the
//!    sentence AWS states for it — `Table already exists: <TargetTableName>`, the only
//!    variable part being the name the REQUEST carried — leaving that table's identity,
//!    status and items exactly as they were.
//!
//! Every rule is asserted over a SET of inputs rather than one example, and every request
//! goes over the wire through the same `MultiServiceRouter` the emulator's front doors
//! use.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok};

use service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION;

/// How many probes a poll in this suite may take before it is a failure. A client's
/// waiter is allowed many more; one is what these rules require.
const MAX_PROBES: usize = 4;

/// Wait out one transient span, so whatever the clock owes a table or a backup is due.
/// This stands in for the interval a client's waiter sleeps between probes.
async fn settle() {
    tokio::time::sleep(STATUS_TRANSITION + Duration::from_millis(150)).await;
}

/// The stored table as `DescribeTable` reports it.
async fn described(router: &MultiServiceRouter, name: &str) -> Value {
    ok(router, "DescribeTable", json!({"TableName": name})).await["Table"].clone()
}

/// Create one table and wait out its transient span, so it is settled before it is read
/// or written: "You can perform read and write operations only on an ACTIVE table".
async fn active_table(router: &MultiServiceRouter, name: &str, create: &Value) {
    let mut body = create.clone();
    body["TableName"] = json!(name);
    ok(router, "CreateTable", body).await;
    settle().await;
}

/// The loop an SDK `table_exists` waiter runs: poll `DescribeTable` until it reports the
/// table `ACTIVE`, sleeping its interval between probes. Returns how many probes it took
/// and the description the successful one answered.
async fn table_exists_probes(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> (usize, Value) {
    let mut last = Value::Null;
    for probes in 1..=MAX_PROBES {
        last = described(router, name).await;
        if last["TableStatus"] == json!("ACTIVE") {
            return (probes, last);
        }
        settle().await;
    }
    panic!(
        "case {case}: {name} was not reported ACTIVE within {MAX_PROBES} probes, last \
         answer {last}"
    );
}

/// `DescribeTable` with the time it took to answer, which is what a poller spends on it.
async fn timed_describe(router: &MultiServiceRouter, name: &str) -> (Duration, Value) {
    let started = Instant::now();
    let table = described(router, name).await;
    (started.elapsed(), table)
}

/// Which asynchronous call registers the table under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Creator {
    CreateTable,
    RestoreTableFromBackup,
    RestoreTableToPointInTime,
}

/// One row of the table below: a way of creating a table, crossed with the shape of the
/// table created.
struct Row {
    case: &'static str,
    creator: Creator,
    create: Value,
    item: Value,
    key: Value,
}

/// The rows: the three calls that create a table, crossed with the key schema, capacity
/// mode and secondary indexes of the table each one creates.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a created on-demand table with one partition key",
            creator: Creator::CreateTable,
            create: json!({
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
                "BillingMode": "PAY_PER_REQUEST",
            }),
            item: json!({"Part": {"S": "one"}, "Payload": {"N": "17"}}),
            key: json!({"Part": {"S": "one"}}),
        },
        Row {
            case: "a created provisioned table with a sort key and a global secondary index",
            creator: Creator::CreateTable,
            create: json!({
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
                "ProvisionedThroughput": {"ReadCapacityUnits": 2, "WriteCapacityUnits": 2},
                "GlobalSecondaryIndexes": [{
                    "IndexName": "ByColour",
                    "KeySchema": [{"AttributeName": "Colour", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                    "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
                }],
            }),
            item: json!({"Part": {"S": "two"}, "Sort": {"N": "5"}, "Colour": {"S": "red"}}),
            key: json!({"Part": {"S": "two"}, "Sort": {"N": "5"}}),
        },
        Row {
            case: "a table restored from a backup of an on-demand table",
            creator: Creator::RestoreTableFromBackup,
            create: json!({
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
                "BillingMode": "PAY_PER_REQUEST",
            }),
            item: json!({"Part": {"S": "three"}, "Payload": {"S": "restored"}}),
            key: json!({"Part": {"S": "three"}}),
        },
        Row {
            case: "a table restored from a backup of a provisioned table with an index",
            creator: Creator::RestoreTableFromBackup,
            create: json!({
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [
                    {"AttributeName": "Part", "AttributeType": "S"},
                    {"AttributeName": "Colour", "AttributeType": "S"},
                ],
                "BillingMode": "PROVISIONED",
                "ProvisionedThroughput": {"ReadCapacityUnits": 3, "WriteCapacityUnits": 3},
                "GlobalSecondaryIndexes": [{
                    "IndexName": "ByColour",
                    "KeySchema": [{"AttributeName": "Colour", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "KEYS_ONLY"},
                    "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
                }],
            }),
            item: json!({"Part": {"S": "four"}, "Colour": {"S": "blue"}}),
            key: json!({"Part": {"S": "four"}}),
        },
        Row {
            case: "a table restored to a point in time",
            creator: Creator::RestoreTableToPointInTime,
            create: json!({
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
                "BillingMode": "PAY_PER_REQUEST",
            }),
            item: json!({"Part": {"S": "five"}, "Payload": {"BOOL": true}}),
            key: json!({"Part": {"S": "five"}}),
        },
    ]
}

/// Run the asynchronous call this row creates `target` with, setting up whatever source
/// it restores from, and return the `TableDescription` that call answered with.
async fn start_creation(router: &MultiServiceRouter, row: &Row, target: &str) -> Value {
    let source = format!("{target}Source");
    match row.creator {
        Creator::CreateTable => {
            let mut body = row.create.clone();
            body["TableName"] = json!(target);
            ok(router, "CreateTable", body).await["TableDescription"].clone()
        }
        Creator::RestoreTableFromBackup => {
            active_table(router, &source, &row.create).await;
            let details = ok(
                router,
                "CreateBackup",
                json!({"TableName": &source, "BackupName": format!("{target}Backup")}),
            )
            .await["BackupDetails"]
                .clone();
            settle().await;
            ok(
                router,
                "RestoreTableFromBackup",
                json!({
                    "TargetTableName": target,
                    "BackupArn": details["BackupArn"].clone(),
                }),
            )
            .await["TableDescription"]
                .clone()
        }
        Creator::RestoreTableToPointInTime => {
            active_table(router, &source, &row.create).await;
            ok(
                router,
                "UpdateContinuousBackups",
                json!({
                    "TableName": &source,
                    "PointInTimeRecoverySpecification": {"PointInTimeRecoveryEnabled": true},
                }),
            )
            .await;
            settle().await;
            ok(
                router,
                "RestoreTableToPointInTime",
                json!({
                    "SourceTableName": &source,
                    "TargetTableName": target,
                    "UseLatestRestorableTime": true,
                }),
            )
            .await["TableDescription"]
                .clone()
        }
    }
}

#[tokio::test]
async fn the_first_poll_after_a_table_is_created_reports_it_settled() {
    for (position, row) in rows().into_iter().enumerate() {
        let router = emulator();
        let case = row.case;
        let target = format!("Polled{position}");

        // The asynchronous call answers with the transient status, exactly as AWS
        // documents: "DynamoDB immediately returns a response with a TableStatus of
        // CREATING".
        let started = start_creation(&router, &row, &target).await;
        assert_eq!(
            started["TableStatus"],
            json!("CREATING"),
            "case {case}: the creating call must report the table CREATING, got {started}"
        );

        // The first probe a client's waiter sends, with no wait before it.
        let (elapsed, first) = timed_describe(&router, &target).await;
        assert_eq!(
            first["TableStatus"],
            json!("ACTIVE"),
            "case {case}: the first poll must report the table settled, got \
             {first} after {elapsed:?}"
        );
        assert!(
            elapsed <= STATUS_TRANSITION * 3,
            "case {case}: the first poll must be answered within the transition span, took \
             {elapsed:?}"
        );

        // A table the clock owes nothing is reported with no wait at all.
        let (again, settled) = timed_describe(&router, &target).await;
        assert!(
            again < STATUS_TRANSITION && settled["TableStatus"] == json!("ACTIVE"),
            "case {case}: a settled table must be reported at once, took {again:?} for \
             {settled}"
        );

        // ...and the table the poll reported settled serves the data plane: the write
        // goes in and reads back through GetItem.
        ok(
            &router,
            "PutItem",
            json!({"TableName": &target, "Item": row.item.clone()}),
        )
        .await;
        let fetched = ok(
            &router,
            "GetItem",
            json!({"TableName": &target, "Key": row.key.clone(), "ConsistentRead": true}),
        )
        .await;
        assert_eq!(
            fetched["Item"], row.item,
            "case {case}: the settled table must serve the item written into it, got {fetched}"
        );
    }
}

#[tokio::test]
async fn a_transition_a_later_call_asked_for_is_reported_as_it_stands() {
    let router = emulator();
    let provisioned = json!({
        "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
        "BillingMode": "PROVISIONED",
        "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
    });

    // Rows: the two transitions a call made on a table that ALREADY serves requests, each
    // of which the reader reports in flight rather than waiting out.
    let rows: [(&str, &str, Value, &str); 2] = [
        (
            "a delete in flight",
            "Dropping",
            json!({"ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1}}),
            "DELETING",
        ),
        (
            "a capacity update in flight",
            "Moving",
            json!({"ProvisionedThroughput": {"ReadCapacityUnits": 9, "WriteCapacityUnits": 9}}),
            "UPDATING",
        ),
    ];
    for (case, name, change, expected) in rows {
        active_table(&router, name, &provisioned).await;
        let operation = if expected == "DELETING" {
            "DeleteTable"
        } else {
            "UpdateTable"
        };
        let mut body = change.clone();
        body["TableName"] = json!(name);
        if operation == "DeleteTable" {
            body = json!({"TableName": name});
        }
        ok(&router, operation, body).await;

        let (elapsed, reported) = timed_describe(&router, name).await;
        assert_eq!(
            reported["TableStatus"],
            json!(expected),
            "case {case}: the reader must report the transition in flight, got {reported}"
        );
        assert!(
            elapsed < STATUS_TRANSITION,
            "case {case}: reporting a transition in flight must not wait for it, took \
             {elapsed:?}"
        );
    }
}

#[tokio::test]
async fn the_backup_and_restore_sequence_converges_and_refuses_a_taken_target_name() {
    let router = emulator();
    let create = json!({
        "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
        "BillingMode": "PAY_PER_REQUEST",
    });
    let seed = json!({"pk": {"S": "seed"}, "v": {"S": "seed-data"}});
    let sentinel = json!({"pk": {"S": "sentinel"}, "v": {"S": "untouched"}});

    // Two tables, each created and waited for the way a client does it: CreateTable, then
    // poll DescribeTable until the table is ACTIVE. One probe each.
    for (name, item) in [("Seeded", &seed), ("Sentinel", &sentinel)] {
        let mut body = create.clone();
        body["TableName"] = json!(name);
        let created = ok(&router, "CreateTable", body).await["TableDescription"].clone();
        assert_eq!(
            created["TableStatus"],
            json!("CREATING"),
            "case {name}: the create must report the table CREATING, got {created}"
        );
        let (probes, _) = table_exists_probes(&router, name, name).await;
        assert_eq!(
            probes, 1,
            "case {name}: a client's waiter must see the new table ACTIVE on its first \
             probe, it took {probes}"
        );
        ok(
            &router,
            "PutItem",
            json!({"TableName": name, "Item": (*item).clone()}),
        )
        .await;
    }

    // "Continuous backups are ENABLED on all tables at table creation", so the poll that
    // waits for that is satisfied by its first probe too.
    let backups = ok(
        &router,
        "DescribeContinuousBackups",
        json!({"TableName": "Seeded"}),
    )
    .await;
    assert_eq!(
        backups["ContinuousBackupsDescription"]["ContinuousBackupsStatus"],
        json!("ENABLED"),
        "case continuous backups: every table reports them ENABLED, got {backups}"
    );

    // "the backup is created asynchronously", so poll DescribeBackup until it is
    // AVAILABLE — the second blocking wait of the sequence.
    let details = ok(
        &router,
        "CreateBackup",
        json!({"TableName": "Seeded", "BackupName": "flow"}),
    )
    .await["BackupDetails"]
        .clone();
    let arn = details["BackupArn"].as_str().unwrap_or_default().to_string();
    let mut status = details["BackupStatus"].clone();
    for _ in 0..MAX_PROBES {
        if status == json!("AVAILABLE") {
            break;
        }
        settle().await;
        status = ok(&router, "DescribeBackup", json!({"BackupArn": &arn})).await
            ["BackupDescription"]["BackupDetails"]["BackupStatus"]
            .clone();
    }
    assert_eq!(
        status,
        json!("AVAILABLE"),
        "case backup: the backup must become AVAILABLE within {MAX_PROBES} probes, got \
         {status}"
    );

    // The positive control: the same backup restored to a FREE name converges and carries
    // the source table's items.
    let restored = ok(
        &router,
        "RestoreTableFromBackup",
        json!({"TargetTableName": "RestoredFree", "BackupArn": &arn}),
    )
    .await["TableDescription"]
        .clone();
    assert_eq!(
        restored["TableStatus"],
        json!("CREATING"),
        "case restore to a free name: the restore must report the table CREATING, got \
         {restored}"
    );
    let (probes, settled) = table_exists_probes(&router, "restore to a free name", "RestoredFree")
        .await;
    assert_eq!(
        probes, 1,
        "case restore to a free name: the waiter must see the restored table ACTIVE on its \
         first probe, it took {probes}"
    );
    ok(
        &router,
        "TagResource",
        json!({
            "ResourceArn": settled["TableArn"].clone(),
            "Tags": [{"Key": "flow", "Value": "restored"}],
        }),
    )
    .await;
    let carried = ok(
        &router,
        "GetItem",
        json!({
            "TableName": "RestoredFree",
            "Key": {"pk": {"S": "seed"}},
            "ConsistentRead": true,
        }),
    )
    .await;
    assert_eq!(
        carried["Item"], seed,
        "case restore to a free name: the restored table must carry the source item, got \
         {carried}"
    );

    // And the collision. Rows: every settled table the sequence left behind, so the
    // refusal's variable part is read from the REQUEST rather than fixed, crossed with
    // whether that table's own data overlaps the backup's.
    let collisions: [(&str, &Value, bool); 2] = [
        ("Sentinel", &sentinel, false),
        ("Seeded", &seed, true),
    ];
    for (target, own, holds_seed) in collisions {
        let before = described(&router, target).await;
        let (status, body) = call(
            &router,
            "RestoreTableFromBackup",
            json!({"TargetTableName": target, "BackupArn": &arn}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "TableAlreadyExistsException"),
            "case a restore onto {target}: expected 400 TableAlreadyExistsException, got \
             {status} with {body}"
        );
        assert_eq!(
            message(&body),
            format!("Table already exists: {target}"),
            "case a restore onto {target}: expected the sentence AWS states for that \
             conflict, got {}",
            message(&body)
        );

        // The refused restore left the table it collided with untouched: same identity,
        // still settled, its own item intact.
        let after = described(&router, target).await;
        assert_eq!(
            (
                after["TableArn"].clone(),
                after["CreationDateTime"].clone(),
                after["TableStatus"].clone(),
                after["ItemCount"].clone(),
            ),
            (
                before["TableArn"].clone(),
                before["CreationDateTime"].clone(),
                json!("ACTIVE"),
                before["ItemCount"].clone(),
            ),
            "case a restore onto {target}: the colliding table must be unaffected, got \
             {after} against {before}"
        );
        let kept = ok(
            &router,
            "GetItem",
            json!({
                "TableName": target,
                "Key": {"pk": own["pk"].clone()},
                "ConsistentRead": true,
            }),
        )
        .await;
        assert_eq!(
            kept["Item"], *own,
            "case a restore onto {target}: the table's own item must be intact, got {kept}"
        );
        // ...and no item of the backup leaked into a table that did not already hold it.
        let leaked = ok(
            &router,
            "GetItem",
            json!({
                "TableName": target,
                "Key": {"pk": {"S": "seed"}},
                "ConsistentRead": true,
            }),
        )
        .await;
        assert_eq!(
            leaked.get("Item").is_some(),
            holds_seed,
            "case a restore onto {target}: the backup's item must be present only where it \
             already was, got {leaked}"
        );
    }
}
