// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `RestoreTableToPointInTime`.
//!
//! The rules pinned here:
//!
//! 1. an operation that PRODUCES a table registers it in state under the name it was
//!    given, so every later call addresses it: the restore reports the new table and a
//!    `DescribeTable` sent straight afterwards reports the same one, `CREATING` until the
//!    clock makes it `ACTIVE` — and then the data plane serves it (`PutItem`, `GetItem`,
//!    `Query`) and `DeleteTable` tears it down;
//! 2. what the restored table carries is the SOURCE table's current settings and data —
//!    "Global secondary indexes (GSIs) Local secondary indexes (LSIs) Provisioned read
//!    and write capacity ... All these settings come from the current settings of the
//!    source table at the time of restore" (the `RestoreTableToPointInTime` doc) — with
//!    its own `TableId` and ARN, and a `RestoreSummary` naming the table it came from;
//! 3. what AWS says must be set up again does not carry over: "Tags Stream settings Time
//!    to Live (TTL) settings Point in time recovery settings", so the restored table's own
//!    recovery is off;
//! 4. the restore is served against the source table's continuous-backups settings: a
//!    table with recovery off is refused
//!    (`PointInTimeRecoveryUnavailableException`), a point outside the restorable window
//!    is `InvalidRestoreTimeException`, a source table that is not there is
//!    `TableNotFoundException`, and a target name a table already holds is
//!    `TableAlreadyExistsException`/`TableInUseException`;
//! 5. a request that names no point in time, two contradicting points, or an override
//!    that no index or capacity mode of the source table can satisfy, is a
//!    `ValidationException`.
//!
//! Every test drives the operations over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, and rules 1-3 are asserted over a
//! TABLE of source tables — their key schema, capacity mode, secondary indexes and items
//! — crossed with the way each restore names its source and its point in time.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok};

/// Wait out one transient span, so whatever the clock owes a table is due.
async fn settle() {
    tokio::time::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(150),
    )
    .await;
}

/// Create one table, wait out its `CREATING` span and write `items` into it: "You can
/// perform read and write operations only on an ACTIVE table".
async fn active_table(router: &MultiServiceRouter, name: &str, create: &Value, items: &[Value]) {
    let mut body = create.clone();
    body["TableName"] = json!(name);
    ok(router, "CreateTable", body).await;
    settle().await;
    for item in items {
        ok(
            router,
            "PutItem",
            json!({"TableName": name, "Item": item.clone()}),
        )
        .await;
    }
}

/// Turn point-in-time recovery on for `table` and return the restorable window it
/// reports, as the wire carries it.
async fn enable_recovery(router: &MultiServiceRouter, table: &str) -> (Value, Value) {
    let updated = ok(
        router,
        "UpdateContinuousBackups",
        json!({
            "TableName": table,
            "PointInTimeRecoverySpecification": {"PointInTimeRecoveryEnabled": true},
        }),
    )
    .await;
    let recovery =
        updated["ContinuousBackupsDescription"]["PointInTimeRecoveryDescription"].clone();
    (
        recovery["EarliestRestorableDateTime"].clone(),
        recovery["LatestRestorableDateTime"].clone(),
    )
}

/// The stored table as `DescribeTable` reports it.
async fn described(router: &MultiServiceRouter, name: &str) -> Value {
    ok(router, "DescribeTable", json!({"TableName": name})).await["Table"].clone()
}

/// The `PointInTimeRecoveryStatus` a table reports.
async fn recovery_status(router: &MultiServiceRouter, name: &str) -> Value {
    ok(
        router,
        "DescribeContinuousBackups",
        json!({"TableName": name}),
    )
    .await["ContinuousBackupsDescription"]["PointInTimeRecoveryDescription"]
        ["PointInTimeRecoveryStatus"]
        .clone()
}

/// The primary key of `item`, read off the key schema the create declared — the same
/// projection a client makes to build a `GetItem` key.
fn key_of(create: &Value, item: &Value) -> Value {
    let mut key = serde_json::Map::new();
    for element in create["KeySchema"].as_array().cloned().unwrap_or_default() {
        let name = element["AttributeName"]
            .as_str()
            .unwrap_or_default()
            .to_string();
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
        .map(|index| {
            index["IndexName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// How each source table under test is created, and the items it is given.
struct Source {
    case: &'static str,
    create: Value,
    items: Vec<Value>,
}

/// The source tables rules 1-3 are asserted over: they differ in key schema (one
/// attribute and two), capacity mode, which secondary indexes they carry (none, one
/// global, one global and one local, two globals) and how many items they hold.
fn sources() -> Vec<Source> {
    vec![
        Source {
            case: "a provisioned table with a global and a local secondary index",
            create: json!({
                "KeySchema": [
                    {"AttributeName": "Part", "KeyType": "HASH"},
                    {"AttributeName": "Sort", "KeyType": "RANGE"},
                ],
                "AttributeDefinitions": [
                    {"AttributeName": "Part", "AttributeType": "S"},
                    {"AttributeName": "Sort", "AttributeType": "S"},
                    {"AttributeName": "Colour", "AttributeType": "S"},
                    {"AttributeName": "Size", "AttributeType": "N"},
                ],
                "BillingMode": "PROVISIONED",
                "ProvisionedThroughput": {"ReadCapacityUnits": 4, "WriteCapacityUnits": 2},
                "GlobalSecondaryIndexes": [{
                    "IndexName": "ByColour",
                    "KeySchema": [{"AttributeName": "Colour", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "INCLUDE", "NonKeyAttributes": ["Size"]},
                    "ProvisionedThroughput": {"ReadCapacityUnits": 3, "WriteCapacityUnits": 1},
                }],
                "LocalSecondaryIndexes": [{
                    "IndexName": "BySize",
                    "KeySchema": [
                        {"AttributeName": "Part", "KeyType": "HASH"},
                        {"AttributeName": "Size", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "ALL"},
                }],
            }),
            items: vec![
                json!({"Part": {"S": "p1"}, "Sort": {"S": "a"},
                       "Colour": {"S": "red"}, "Size": {"N": "3"}}),
                json!({"Part": {"S": "p1"}, "Sort": {"S": "b"},
                       "Colour": {"S": "blue"}, "Size": {"N": "7"}}),
            ],
        },
        Source {
            case: "an on-demand table with one global secondary index",
            create: json!({
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [
                    {"AttributeName": "Part", "AttributeType": "S"},
                    {"AttributeName": "Colour", "AttributeType": "S"},
                ],
                "BillingMode": "PAY_PER_REQUEST",
                "GlobalSecondaryIndexes": [{
                    "IndexName": "ByColour",
                    "KeySchema": [{"AttributeName": "Colour", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "KEYS_ONLY"},
                }],
            }),
            items: vec![json!({"Part": {"S": "only"}, "Colour": {"S": "green"}})],
        },
        Source {
            case: "a provisioned table with no secondary index and no items",
            create: json!({
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
                "BillingMode": "PROVISIONED",
                "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
            }),
            items: Vec::new(),
        },
        Source {
            case: "a provisioned table with two global secondary indexes",
            create: json!({
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [
                    {"AttributeName": "Part", "AttributeType": "S"},
                    {"AttributeName": "Colour", "AttributeType": "S"},
                    {"AttributeName": "Size", "AttributeType": "N"},
                ],
                "BillingMode": "PROVISIONED",
                "ProvisionedThroughput": {"ReadCapacityUnits": 2, "WriteCapacityUnits": 2},
                "GlobalSecondaryIndexes": [
                    {
                        "IndexName": "ByColour",
                        "KeySchema": [{"AttributeName": "Colour", "KeyType": "HASH"}],
                        "Projection": {"ProjectionType": "ALL"},
                        "ProvisionedThroughput": {
                            "ReadCapacityUnits": 2, "WriteCapacityUnits": 2,
                        },
                    },
                    {
                        "IndexName": "BySize",
                        "KeySchema": [{"AttributeName": "Size", "KeyType": "HASH"}],
                        "Projection": {"ProjectionType": "KEYS_ONLY"},
                        "ProvisionedThroughput": {
                            "ReadCapacityUnits": 1, "WriteCapacityUnits": 1,
                        },
                    },
                ],
            }),
            items: vec![
                json!({"Part": {"S": "p1"}, "Colour": {"S": "red"}, "Size": {"N": "1"}}),
                json!({"Part": {"S": "p2"}, "Colour": {"S": "red"}, "Size": {"N": "2"}}),
                json!({"Part": {"S": "p3"}, "Colour": {"S": "blue"}, "Size": {"N": "3"}}),
            ],
        },
    ]
}

#[tokio::test]
async fn a_restore_registers_the_table_it_creates_and_every_later_call_addresses_it() {
    let router = emulator();

    // The rows are the source tables above, crossed with the way the restore names its
    // source (by name, by ARN) and its point in time (the latest restorable one, and the
    // instant the window itself reports).
    for (position, source) in sources().into_iter().enumerate() {
        let case = source.case;
        let name = format!("PitrSource{position}");
        let target = format!("PitrRestored{position}");
        active_table(&router, &name, &source.create, &source.items).await;
        let before = described(&router, &name).await;
        let (_, latest) = enable_recovery(&router, &name).await;

        let mut request = json!({"TargetTableName": &target});
        if position % 2 == 0 {
            request["SourceTableName"] = json!(&name);
        } else {
            request["SourceTableArn"] = before["TableArn"].clone();
        }
        if position % 2 == 0 {
            request["UseLatestRestorableTime"] = json!(true);
        } else {
            // The newest restorable point as the window itself reported it: a client
            // sends back exactly the instant it was given.
            request["RestoreDateTime"] = latest.clone();
        }

        let restored = ok(&router, "RestoreTableToPointInTime", request.clone()).await
            ["TableDescription"]
            .clone();
        assert_eq!(
            (
                restored["TableName"].clone(),
                restored["TableStatus"].clone(),
                restored["RestoreSummary"]["SourceTableArn"].clone(),
                restored["RestoreSummary"]["RestoreInProgress"].clone(),
            ),
            (
                json!(&target),
                json!("CREATING"),
                before["TableArn"].clone(),
                json!(true),
            ),
            "case {case:?} ({request}): the restore reports the table it created, got \
             {restored}"
        );
        assert!(
            restored["TableId"] != before["TableId"]
                && restored["TableArn"]
                    .as_str()
                    .unwrap_or_default()
                    .ends_with(&format!("table/{target}")),
            "case {case:?}: the restored table is a NEW table with its own id and an ARN \
             naming it, got {restored} against {before}"
        );

        // The table is registered from the instant the restore answered: a call sent
        // straight afterwards addresses it rather than answering as if it were absent.
        let creating = described(&router, &target).await;
        assert_eq!(
            (
                creating["TableName"].clone(),
                creating["TableStatus"].clone(),
                creating["KeySchema"].clone(),
            ),
            (
                json!(&target),
                json!("CREATING"),
                before["KeySchema"].clone(),
            ),
            "case {case:?}: the table is registered while it is still CREATING, got {creating}"
        );

        // Once the clock has made the transition due, the table and every index it
        // carries are ACTIVE, and the configuration is the source table's.
        settle().await;
        let active = described(&router, &target).await;
        let index_statuses: Vec<Value> = active["GlobalSecondaryIndexes"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|index| index["IndexStatus"].clone())
            .collect();
        assert_eq!(
            (
                active["TableStatus"].clone(),
                index_names(&active, "GlobalSecondaryIndexes"),
                index_names(&active, "LocalSecondaryIndexes"),
                index_statuses.clone(),
                active["BillingModeSummary"]["BillingMode"].clone(),
            ),
            (
                json!("ACTIVE"),
                index_names(&before, "GlobalSecondaryIndexes"),
                index_names(&before, "LocalSecondaryIndexes"),
                index_names(&before, "GlobalSecondaryIndexes")
                    .iter()
                    .map(|_| json!("ACTIVE"))
                    .collect::<Vec<Value>>(),
                before["BillingModeSummary"]["BillingMode"].clone(),
            ),
            "case {case:?}: the restored table carries the source's indexes and capacity \
             mode and reaches ACTIVE, got {active}"
        );
        for member in ["GlobalSecondaryIndexes", "LocalSecondaryIndexes"] {
            let projections: Vec<Value> = active[member]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|index| index["Projection"].clone())
                .collect();
            let expected: Vec<Value> = before[member]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|index| index["Projection"].clone())
                .collect();
            assert_eq!(
                projections, expected,
                "case {case:?}: {member} keep the projections the source declared, got {active}"
            );
        }
        assert_eq!(
            active["ProvisionedThroughput"]["ReadCapacityUnits"].clone(),
            before["ProvisionedThroughput"]["ReadCapacityUnits"].clone(),
            "case {case:?}: the restored table keeps the source's read capacity, got {active}"
        );

        // The DATA the source table held is in the restored table, and the restored
        // table serves reads and writes of its own.
        for item in &source.items {
            let key = key_of(&source.create, item);
            let read = ok(
                &router,
                "GetItem",
                json!({"TableName": &target, "Key": key.clone()}),
            )
            .await;
            assert_eq!(
                read["Item"].clone(),
                item.clone(),
                "case {case:?}: the restored table holds the source's item {key}, got {read}"
            );
        }
        let mut written = source
            .items
            .first()
            .cloned()
            .unwrap_or_else(|| json!({"Part": {"S": "fresh"}}));
        written["Part"] = json!({"S": "written-after-the-restore"});
        ok(
            &router,
            "PutItem",
            json!({"TableName": &target, "Item": written.clone()}),
        )
        .await;
        let queried = ok(
            &router,
            "Query",
            json!({
                "TableName": &target,
                "KeyConditionExpression": "Part = :p",
                "ExpressionAttributeValues": {":p": {"S": "written-after-the-restore"}},
            }),
        )
        .await;
        assert_eq!(
            queried["Count"].clone(),
            json!(1),
            "case {case:?}: the restored table serves the item just written to it, got {queried}"
        );

        // "You must manually set up the following on the restored table: ... Point in
        // time recovery settings": the restored table's own recovery is off, while the
        // source table is left exactly as it was.
        assert_eq!(
            (
                recovery_status(&router, &target).await,
                recovery_status(&router, &name).await,
                described(&router, &name).await["TableStatus"].clone(),
            ),
            (json!("DISABLED"), json!("ENABLED"), json!("ACTIVE")),
            "case {case:?}: recovery does not carry over and the source is untouched"
        );

        // And the table a restore created is torn down like any other.
        let deleted = ok(&router, "DeleteTable", json!({"TableName": &target})).await;
        assert_eq!(
            deleted["TableDescription"]["TableStatus"].clone(),
            json!("DELETING"),
            "case {case:?}: the restored table is deletable, got {deleted}"
        );
    }
}

#[tokio::test]
async fn an_index_override_selects_the_source_tables_indexes() {
    let router = emulator();
    let source = &sources()[3];
    active_table(&router, "PitrOverride", &source.create, &source.items).await;
    enable_recovery(&router, "PitrOverride").await;

    // Rows differ in which of the source table's indexes the override selects — "You can
    // choose to exclude some or all of the indexes at the time of restore"
    // (`RestoreTableToPointInTime$GlobalSecondaryIndexOverride`) — and in whether it
    // restates an index's capacity.
    let cases: [(&str, Value, Vec<&str>); 3] = [
        ("no override, so every index", Value::Null, vec!["ByColour", "BySize"]),
        (
            "one of the two indexes",
            json!([{
                "IndexName": "BySize",
                "KeySchema": [{"AttributeName": "Size", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "KEYS_ONLY"},
                "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 6},
            }]),
            vec!["BySize"],
        ),
        ("an empty override, so no index at all", json!([]), vec![]),
    ];

    for (position, (case, override_value, expected)) in cases.into_iter().enumerate() {
        let target = format!("PitrOverridden{position}");
        let mut request = json!({
            "SourceTableName": "PitrOverride",
            "TargetTableName": &target,
            "UseLatestRestorableTime": true,
        });
        if !override_value.is_null() {
            request["GlobalSecondaryIndexOverride"] = override_value.clone();
        }
        ok(&router, "RestoreTableToPointInTime", request.clone()).await;
        settle().await;
        let active = described(&router, &target).await;
        assert_eq!(
            index_names(&active, "GlobalSecondaryIndexes"),
            expected
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<String>>(),
            "case {case:?} ({request}): the override selects the indexes restored, got {active}"
        );
    }

    // The restated capacity is the one the restored index reports.
    let restated = described(&router, "PitrOverridden1").await["GlobalSecondaryIndexes"][0]
        ["ProvisionedThroughput"]
        .clone();
    assert_eq!(
        (
            restated["ReadCapacityUnits"].clone(),
            restated["WriteCapacityUnits"].clone(),
        ),
        (json!(5), json!(6)),
        "case the restated index capacity: the override is applied, got {restated}"
    );
}

#[tokio::test]
async fn a_restore_the_source_table_cannot_serve_is_refused_as_the_modelled_error() {
    let router = emulator();
    let source = &sources()[1];
    active_table(&router, "PitrServed", &source.create, &source.items).await;
    active_table(&router, "PitrNoRecovery", &source.create, &source.items).await;
    let (earliest, _) = enable_recovery(&router, "PitrServed").await;
    // One settled table to collide with, and one still being created.
    ok(
        &router,
        "RestoreTableToPointInTime",
        json!({
            "SourceTableName": "PitrServed",
            "TargetTableName": "PitrTaken",
            "UseLatestRestorableTime": true,
        }),
    )
    .await;
    settle().await;

    let a_day = 24.0 * 60.0 * 60.0;
    let far_past = json!(earliest.as_f64().unwrap_or_default() - 40.0 * a_day);
    let future = json!(earliest.as_f64().unwrap_or_default() + a_day);

    // Rows differ in what the restore asks for that cannot be served: each is the error
    // the model declares for that conflict, never a 501 and never a resource miss where
    // the input is what is wrong.
    let cases: [(&str, Value, &str, &str); 8] = [
        (
            "a source table that was never created",
            json!({"SourceTableName": "PitrAbsent", "TargetTableName": "PitrOut1",
                   "UseLatestRestorableTime": true}),
            "TableNotFoundException",
            "PitrAbsent",
        ),
        (
            "a source table whose recovery was never turned on",
            json!({"SourceTableName": "PitrNoRecovery", "TargetTableName": "PitrOut2",
                   "UseLatestRestorableTime": true}),
            "PointInTimeRecoveryUnavailableException",
            "PitrNoRecovery",
        ),
        (
            "a point in time before the window opened",
            json!({"SourceTableName": "PitrServed", "TargetTableName": "PitrOut3",
                   "RestoreDateTime": far_past}),
            "InvalidRestoreTimeException",
            "RestoreDateTime must be between",
        ),
        (
            "a point in time after the window's newest point",
            json!({"SourceTableName": "PitrServed", "TargetTableName": "PitrOut4",
                   "RestoreDateTime": future}),
            "InvalidRestoreTimeException",
            "RestoreDateTime must be between",
        ),
        (
            "no point in time at all",
            json!({"SourceTableName": "PitrServed", "TargetTableName": "PitrOut5"}),
            "ValidationException",
            "UseLatestRestorableTime",
        ),
        (
            "two contradicting points in time",
            json!({"SourceTableName": "PitrServed", "TargetTableName": "PitrOut6",
                   "UseLatestRestorableTime": true, "RestoreDateTime": earliest.clone()}),
            "ValidationException",
            "RestoreDateTime",
        ),
        (
            "no source table named at all",
            json!({"TargetTableName": "PitrOut7", "UseLatestRestorableTime": true}),
            "ValidationException",
            "SourceTableArn",
        ),
        (
            "a target name a table already holds",
            json!({"SourceTableName": "PitrServed", "TargetTableName": "PitrTaken",
                   "UseLatestRestorableTime": true}),
            "TableAlreadyExistsException",
            "PitrTaken",
        ),
    ];

    for (case, request, code, detail) in cases {
        let (status, body) = call(&router, "RestoreTableToPointInTime", request.clone()).await;
        let stated = message(&body);
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, code),
            "case {case:?} ({request}): expected 400 {code}, got {status} with {body}"
        );
        assert!(
            stated.contains(detail),
            "case {case:?}: the refusal states {detail:?}, got {stated:?}"
        );
    }

    // An override that names an index the source table does not carry, and one that
    // contradicts the restored table's capacity mode, are bad requests too.
    for (case, request, detail) in [
        (
            "an override naming an index the source table does not carry",
            json!({"SourceTableName": "PitrServed", "TargetTableName": "PitrOut8",
                   "UseLatestRestorableTime": true,
                   "GlobalSecondaryIndexOverride": [{
                       "IndexName": "ByNothing",
                       "KeySchema": [{"AttributeName": "Colour", "KeyType": "HASH"}],
                       "Projection": {"ProjectionType": "ALL"}}]}),
            "ByNothing",
        ),
        (
            "a provisioned throughput override on an on-demand restore",
            json!({"SourceTableName": "PitrServed", "TargetTableName": "PitrOut9",
                   "UseLatestRestorableTime": true,
                   "BillingModeOverride": "PAY_PER_REQUEST",
                   "ProvisionedThroughputOverride": {
                       "ReadCapacityUnits": 1, "WriteCapacityUnits": 1}}),
            "PAY_PER_REQUEST",
        ),
        (
            "a WarmThroughput an index override may not carry here",
            json!({"SourceTableName": "PitrServed", "TargetTableName": "PitrOut10",
                   "UseLatestRestorableTime": true,
                   "GlobalSecondaryIndexOverride": [{
                       "IndexName": "ByColour",
                       "KeySchema": [{"AttributeName": "Colour", "KeyType": "HASH"}],
                       "Projection": {"ProjectionType": "KEYS_ONLY"},
                       "WarmThroughput": {"ReadUnitsPerSecond": 12000}}]}),
            "WarmThroughput",
        ),
    ] {
        let (status, body) = call(&router, "RestoreTableToPointInTime", request.clone()).await;
        let stated = message(&body);
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?} ({request}): expected 400 ValidationException, got {status} with \
             {body}"
        );
        assert!(
            stated.contains(detail),
            "case {case:?}: the refusal states {detail:?}, got {stated:?}"
        );
    }

    // None of the refusals above created a table: the names they named are still free.
    for name in [
        "PitrOut1", "PitrOut2", "PitrOut3", "PitrOut4", "PitrOut5", "PitrOut6", "PitrOut7",
        "PitrOut8", "PitrOut9", "PitrOut10",
    ] {
        let (status, body) = call(&router, "DescribeTable", json!({"TableName": name})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {name}: a refused restore stored nothing, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn a_target_table_that_is_still_being_created_is_in_use() {
    let router = emulator();
    let source = &sources()[2];
    active_table(&router, "PitrInUseSource", &source.create, &source.items).await;
    enable_recovery(&router, "PitrInUseSource").await;

    // The first restore leaves the target `CREATING` for the transient span, which is
    // "a target table with the specified name [that] is either being created or deleted"
    // (`TableInUseException`) — a different conflict from a settled table under that name.
    ok(
        &router,
        "RestoreTableToPointInTime",
        json!({
            "SourceTableName": "PitrInUseSource",
            "TargetTableName": "PitrInUseTarget",
            "UseLatestRestorableTime": true,
        }),
    )
    .await;
    let (status, body) = call(
        &router,
        "RestoreTableToPointInTime",
        json!({
            "SourceTableName": "PitrInUseSource",
            "TargetTableName": "PitrInUseTarget",
            "UseLatestRestorableTime": true,
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "TableInUseException"),
        "case a target still being created: expected 400 TableInUseException, got {status} with \
         {body}"
    );

    settle().await;
    let (status, body) = call(
        &router,
        "RestoreTableToPointInTime",
        json!({
            "SourceTableName": "PitrInUseSource",
            "TargetTableName": "PitrInUseTarget",
            "UseLatestRestorableTime": true,
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "TableAlreadyExistsException"),
        "case a settled target: expected 400 TableAlreadyExistsException, got {status} with \
         {body}"
    );
}
