// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the on-demand backup surface: `CreateBackup` and the three
//! readers it round-trips through — `ListBackups`, `DescribeBackup` and `DeleteBackup`.
//!
//! The rules pinned here:
//!
//! 1. a backup is a RECORD of the table it was taken from, catalogued at the instant of
//!    the request ("a time marker of the request is cataloged" — the `CreateBackup` doc),
//!    so every member the three readers report is the one the create wrote: the ARN
//!    minted for it, its name, its type, its creation instant, and the source table's
//!    key schema, capacity mode and figures as they stood then;
//! 2. the backup is created asynchronously, so it is `CREATING` until the clock makes it
//!    `AVAILABLE`, and the span is non-zero: a caller reading straight after the create
//!    still sees the transient status;
//! 3. a list reports the records the request SELECTS — the table named, the
//!    `BackupTypeFilter` (`USER` when the request names none), and the half-open time
//!    range ("start time is inclusive, but end time is exclusive" — the `ListBackups`
//!    doc) — in ascending `BackupArn` order, `Limit` entries at a time, with
//!    `LastEvaluatedBackupArn` set exactly when more remain;
//! 4. `DeleteBackup` removes the record and reports it `DELETED`; nothing else can then
//!    find it, while deleting the source TABLE leaves its backups alone.
//!
//! Every test drives the operations over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, and each rule is asserted over a
//! SET of inputs — the source table, the filter, the page size, the ARN — rather than one
//! example.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_dynamodb_dynamodb_20120810::ops::types::{
    BACKUP_STATUS_VALUES, BACKUP_TYPE_VALUES,
};

use crate::emulator::{call, call_in, emulator, error_code, ok, REGION};

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
async fn active_table(router: &MultiServiceRouter, name: &str, mut body: Value) {
    body["TableName"] = json!(name);
    ok(router, "CreateTable", body).await;
    settle().await;
}

/// One on-demand table with a simple key, for a test that is not about the source table.
async fn simple_table(router: &MultiServiceRouter, name: &str) {
    active_table(
        router,
        name,
        json!({
            "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
}

/// Take one backup of `table` and return its `BackupDetails`.
async fn take_backup(router: &MultiServiceRouter, table: &str, name: &str) -> Value {
    ok(
        router,
        "CreateBackup",
        json!({"TableName": table, "BackupName": name}),
    )
    .await["BackupDetails"]
        .clone()
}

/// The `BackupSummaries` of a list response, as a list.
fn summaries(body: &Value) -> Vec<Value> {
    body["BackupSummaries"].as_array().cloned().unwrap_or_default()
}

/// The `BackupArn`s a list response reports, in the order it reported them.
fn listed_arns(body: &Value) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| summary["BackupArn"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// One row of the source-table table: the case, the `CreateTable` body, the items put
/// into the table, and the capacity mode and provisioned pair a backup of it records.
type SourceCase = (&'static str, Value, Vec<Value>, &'static str, (i64, i64));

/// The `TableArn` and `TableId` a table reports for itself, which a backup of it records.
async fn table_identity(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let described = ok(router, "DescribeTable", json!({"TableName": name})).await;
    (
        described["Table"]["TableArn"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        described["Table"]["TableId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

#[tokio::test]
async fn a_backup_records_the_source_table_it_was_taken_from() {
    let router = emulator();

    // Rows differ in the SOURCE TABLE a backup is taken from: its capacity mode, its key
    // schema, the secondary index it carries and the items it holds. The last two columns
    // are the capacity mode and the provisioned pair the backup then records — an
    // on-demand table "does not use provisioned throughput", so a backup of one records
    // zeroes, exactly as the table's own description reports them.
    let cases: [SourceCase; 4] = [
        (
            "a provisioned table with a simple key and no items",
            json!({
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
                "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 7},
            }),
            vec![],
            "PROVISIONED",
            (5, 7),
        ),
        (
            "an on-demand table with a composite key and two items",
            json!({
                "KeySchema": [
                    {"AttributeName": "Part", "KeyType": "HASH"},
                    {"AttributeName": "Sort", "KeyType": "RANGE"},
                ],
                "AttributeDefinitions": [
                    {"AttributeName": "Part", "AttributeType": "S"},
                    {"AttributeName": "Sort", "AttributeType": "N"},
                ],
                "BillingMode": "PAY_PER_REQUEST",
            }),
            vec![
                json!({"Part": {"S": "a"}, "Sort": {"N": "1"}}),
                json!({"Part": {"S": "a"}, "Sort": {"N": "2"}}),
            ],
            "PAY_PER_REQUEST",
            (0, 0),
        ),
        (
            "a provisioned table with a global secondary index and one item",
            json!({
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [
                    {"AttributeName": "Part", "AttributeType": "S"},
                    {"AttributeName": "Other", "AttributeType": "S"},
                ],
                "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
                "GlobalSecondaryIndexes": [{
                    "IndexName": "ByOther",
                    "KeySchema": [{"AttributeName": "Other", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                    "ProvisionedThroughput": {"ReadCapacityUnits": 2, "WriteCapacityUnits": 3},
                }],
            }),
            vec![json!({"Part": {"S": "a"}, "Other": {"S": "b"}})],
            "PROVISIONED",
            (1, 1),
        ),
        (
            "a provisioned table with a local secondary index and three items",
            json!({
                "KeySchema": [
                    {"AttributeName": "Part", "KeyType": "HASH"},
                    {"AttributeName": "Sort", "KeyType": "RANGE"},
                ],
                "AttributeDefinitions": [
                    {"AttributeName": "Part", "AttributeType": "S"},
                    {"AttributeName": "Sort", "AttributeType": "N"},
                    {"AttributeName": "Local", "AttributeType": "S"},
                ],
                "ProvisionedThroughput": {"ReadCapacityUnits": 9, "WriteCapacityUnits": 9},
                "LocalSecondaryIndexes": [{
                    "IndexName": "ByLocal",
                    "KeySchema": [
                        {"AttributeName": "Part", "KeyType": "HASH"},
                        {"AttributeName": "Local", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "KEYS_ONLY"},
                }],
            }),
            vec![
                json!({"Part": {"S": "a"}, "Sort": {"N": "1"}, "Local": {"S": "x"}}),
                json!({"Part": {"S": "a"}, "Sort": {"N": "2"}, "Local": {"S": "y"}}),
                json!({"Part": {"S": "b"}, "Sort": {"N": "3"}, "Local": {"S": "z"}}),
            ],
            "PROVISIONED",
            (9, 9),
        ),
    ];

    for (index, (case, create, items, billing_mode, capacity)) in cases.into_iter().enumerate() {
        let name = format!("Source{index}");
        let key_schema = create["KeySchema"].clone();
        active_table(&router, &name, create).await;
        for item in &items {
            ok(
                &router,
                "PutItem",
                json!({"TableName": name, "Item": item}),
            )
            .await;
        }
        let (table_arn, table_id) = table_identity(&router, &name).await;

        let details = take_backup(&router, &name, &format!("Backup{index}")).await;
        let arn = details["BackupArn"].as_str().unwrap_or_default().to_string();
        let status = details["BackupStatus"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let backup_type = details["BackupType"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            arn.starts_with(&format!("{table_arn}/backup/"))
                && details["BackupName"] == json!(format!("Backup{index}"))
                && status == "CREATING"
                && BACKUP_STATUS_VALUES.contains(&status.as_str())
                && backup_type == "USER"
                && BACKUP_TYPE_VALUES.contains(&backup_type.as_str()),
            "case {case}: the create reports the record it wrote, with the transient \
             status a backup starts in, got {details}"
        );

        // Once the clock has made the transition due, both readers report the same
        // record — that is the round trip — now `AVAILABLE`.
        settle().await;
        let listed = ok(&router, "ListBackups", json!({"TableName": name})).await;
        let only = summaries(&listed);
        assert_eq!(
            only.len(),
            1,
            "case {case}: the table's one backup is listed, got {listed}"
        );
        assert_eq!(
            (
                only[0]["BackupArn"].clone(),
                only[0]["BackupName"].clone(),
                only[0]["BackupStatus"].clone(),
                only[0]["BackupType"].clone(),
                only[0]["TableName"].clone(),
                only[0]["TableArn"].clone(),
                only[0]["TableId"].clone(),
                only[0]["BackupCreationDateTime"].clone(),
                only[0]["BackupSizeBytes"].clone(),
            ),
            (
                json!(arn),
                json!(format!("Backup{index}")),
                json!("AVAILABLE"),
                json!("USER"),
                json!(name),
                json!(table_arn),
                json!(table_id),
                details["BackupCreationDateTime"].clone(),
                details["BackupSizeBytes"].clone(),
            ),
            "case {case}: the list reports the record the create wrote, got {listed}"
        );

        let described = ok(&router, "DescribeBackup", json!({"BackupArn": arn})).await;
        let source = described["BackupDescription"]["SourceTableDetails"].clone();
        assert_eq!(
            (
                described["BackupDescription"]["BackupDetails"]["BackupArn"].clone(),
                described["BackupDescription"]["BackupDetails"]["BackupStatus"].clone(),
                described["BackupDescription"]["BackupDetails"]["BackupSizeBytes"].clone(),
                source["TableName"].clone(),
                source["TableArn"].clone(),
                source["TableId"].clone(),
                source["KeySchema"].clone(),
                source["BillingMode"].clone(),
                source["ProvisionedThroughput"]["ReadCapacityUnits"].clone(),
                source["ProvisionedThroughput"]["WriteCapacityUnits"].clone(),
                source["ItemCount"].clone(),
            ),
            (
                json!(arn),
                json!("AVAILABLE"),
                details["BackupSizeBytes"].clone(),
                json!(name),
                json!(table_arn),
                json!(table_id),
                key_schema,
                json!(billing_mode),
                json!(capacity.0),
                json!(capacity.1),
                json!(items.len()),
            ),
            "case {case}: the describe reports the source table the backup recorded, \
             got {described}"
        );

        // The bytes the backup holds are the bytes its source table held: a table with
        // items backs up more than an empty one, and the two figures agree.
        let backup_bytes = described["BackupDescription"]["BackupDetails"]["BackupSizeBytes"]
            .as_i64()
            .unwrap_or(-1);
        assert_eq!(
            (backup_bytes >= 0, source["TableSizeBytes"].as_i64()),
            (true, Some(backup_bytes)),
            "case {case}: the backup's size is the source table's, got {described}"
        );
        if !items.is_empty() {
            assert!(
                backup_bytes > 0,
                "case {case}: a table holding {} item(s) backs up more than nothing, got \
                 {backup_bytes}",
                items.len()
            );
        }
    }
}

#[tokio::test]
async fn the_index_details_of_the_source_table_are_recorded_too() {
    let router = emulator();

    // Rows differ in which secondary indexes the source table carries, which is what the
    // feature details of a backup of it report: a table with none reports no list at all.
    let cases: [(&str, Value, Value, Value); 3] = [
        (
            "no secondary index",
            json!({
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
                "BillingMode": "PAY_PER_REQUEST",
            }),
            Value::Null,
            Value::Null,
        ),
        (
            "one global secondary index",
            json!({
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [
                    {"AttributeName": "Part", "AttributeType": "S"},
                    {"AttributeName": "Other", "AttributeType": "S"},
                ],
                "BillingMode": "PAY_PER_REQUEST",
                "GlobalSecondaryIndexes": [{
                    "IndexName": "ByOther",
                    "KeySchema": [{"AttributeName": "Other", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                }],
            }),
            json!(["ByOther"]),
            Value::Null,
        ),
        (
            "one local secondary index",
            json!({
                "KeySchema": [
                    {"AttributeName": "Part", "KeyType": "HASH"},
                    {"AttributeName": "Sort", "KeyType": "RANGE"},
                ],
                "AttributeDefinitions": [
                    {"AttributeName": "Part", "AttributeType": "S"},
                    {"AttributeName": "Sort", "AttributeType": "N"},
                    {"AttributeName": "Local", "AttributeType": "S"},
                ],
                "BillingMode": "PAY_PER_REQUEST",
                "LocalSecondaryIndexes": [{
                    "IndexName": "ByLocal",
                    "KeySchema": [
                        {"AttributeName": "Part", "KeyType": "HASH"},
                        {"AttributeName": "Local", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "KEYS_ONLY"},
                }],
            }),
            Value::Null,
            json!(["ByLocal"]),
        ),
    ];

    for (index, (case, create, global, local)) in cases.into_iter().enumerate() {
        let name = format!("Indexed{index}");
        active_table(&router, &name, create).await;
        let details = take_backup(&router, &name, &format!("IndexedBackup{index}")).await;
        let arn = details["BackupArn"].as_str().unwrap_or_default().to_string();

        let described = ok(&router, "DescribeBackup", json!({"BackupArn": arn})).await;
        let features = described["BackupDescription"]["SourceTableFeatureDetails"].clone();
        let names = |list: &Value| -> Value {
            match list.as_array() {
                Some(entries) => json!(entries
                    .iter()
                    .map(|entry| entry["IndexName"].clone())
                    .collect::<Vec<Value>>()),
                None => Value::Null,
            }
        };
        assert_eq!(
            (
                names(&features["GlobalSecondaryIndexes"]),
                names(&features["LocalSecondaryIndexes"]),
            ),
            (global, local),
            "case {case}: the backup records the indexes the table carried, got {described}"
        );
    }
}

#[tokio::test]
async fn a_backup_is_creating_until_the_clock_makes_it_available() {
    let router = emulator();
    simple_table(&router, "Transient").await;

    // "the backup is created asynchronously" (the `CreateBackup` doc), so the span is
    // non-zero: a caller listing straight after the create still sees `CREATING`, and the
    // same record reads `AVAILABLE` once the span has elapsed. Nothing else about it
    // changes.
    let details = take_backup(&router, "Transient", "Settling").await;
    let arn = details["BackupArn"].as_str().unwrap_or_default().to_string();

    let at_once = ok(&router, "ListBackups", json!({"TableName": "Transient"})).await;
    let described_at_once = ok(&router, "DescribeBackup", json!({"BackupArn": arn})).await;
    assert_eq!(
        (
            summaries(&at_once).first().map(|s| s["BackupStatus"].clone()),
            described_at_once["BackupDescription"]["BackupDetails"]["BackupStatus"].clone(),
        ),
        (Some(json!("CREATING")), json!("CREATING")),
        "case straight after the create: the transient status is observable, got \
         {at_once} and {described_at_once}"
    );

    settle().await;
    let later = ok(&router, "ListBackups", json!({"TableName": "Transient"})).await;
    let described_later = ok(&router, "DescribeBackup", json!({"BackupArn": arn})).await;
    assert_eq!(
        (
            summaries(&later).first().map(|s| s["BackupStatus"].clone()),
            summaries(&later)
                .first()
                .map(|s| s["BackupCreationDateTime"].clone()),
            described_later["BackupDescription"]["BackupDetails"]["BackupStatus"].clone(),
        ),
        (
            Some(json!("AVAILABLE")),
            Some(details["BackupCreationDateTime"].clone()),
            json!("AVAILABLE"),
        ),
        "case once the span has elapsed: the same record is available, got {later} and \
         {described_later}"
    );
}

#[tokio::test]
async fn a_list_reports_only_the_backups_the_request_selects() {
    let router = emulator();
    simple_table(&router, "Selected").await;
    simple_table(&router, "Other").await;
    let selected = take_backup(&router, "Selected", "OfSelected").await;
    let other = take_backup(&router, "Other", "OfOther").await;
    settle().await;
    let selected_arn = selected["BackupArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let other_arn = other["BackupArn"].as_str().unwrap_or_default().to_string();
    let (selected_table_arn, _) = table_identity(&router, "Selected").await;

    // Rows differ in the selector: the table named (by name and, since "You can also
    // provide the Amazon Resource Name (ARN) of the table in this parameter", by ARN),
    // and the `BackupTypeFilter`. `USER` is "the default setting if no other backup types
    // are specified"; `ALL` spans "all types of on-demand backups (USER and SYSTEM)"; a
    // filter naming a type nothing here takes selects nothing.
    let cases: [(&str, Value, Vec<String>); 8] = [
        (
            "no selector at all",
            json!({}),
            vec![other_arn.clone(), selected_arn.clone()],
        ),
        (
            "the table named by name",
            json!({"TableName": "Selected"}),
            vec![selected_arn.clone()],
        ),
        (
            "the table named by ARN",
            json!({"TableName": selected_table_arn}),
            vec![selected_arn.clone()],
        ),
        (
            "a table that holds no backup",
            json!({"TableName": "Absent"}),
            vec![],
        ),
        (
            "the default type filter, stated",
            json!({"BackupType": "USER"}),
            vec![other_arn.clone(), selected_arn.clone()],
        ),
        (
            "every on-demand type",
            json!({"BackupType": "ALL"}),
            vec![other_arn.clone(), selected_arn.clone()],
        ),
        ("the system type", json!({"BackupType": "SYSTEM"}), vec![]),
        (
            "the Amazon Web Services Backup type",
            json!({"BackupType": "AWS_BACKUP"}),
            vec![],
        ),
    ];

    for (case, request, expected) in cases {
        let listed = ok(&router, "ListBackups", request.clone()).await;
        // The reported order is ascending `BackupArn`, which is what the cursor pages
        // through, so the expectation is sorted the same way.
        let mut expected = expected;
        expected.sort();
        assert_eq!(
            listed_arns(&listed),
            expected,
            "case {case}: the list reports what {request} selects, got {listed}"
        );
    }

    // A backup belongs to the region its table lives in, so a list in another region
    // reports none of them.
    let (_, elsewhere) = call_in(&router, "us-west-2", "ListBackups", json!({})).await;
    assert_eq!(
        listed_arns(&elsewhere),
        Vec::<String>::new(),
        "case another region: the catalogue is per region, got {elsewhere}"
    );
}

#[tokio::test]
async fn the_time_range_is_inclusive_below_and_exclusive_above() {
    let router = emulator();
    simple_table(&router, "Ranged").await;
    let details = take_backup(&router, "Ranged", "InRange").await;
    settle().await;
    let arn = details["BackupArn"].as_str().unwrap_or_default().to_string();
    let created = details["BackupCreationDateTime"].as_f64().unwrap_or_default();

    // "In the request, start time is inclusive, but end time is exclusive. Note that
    // these boundaries are for the time at which the original backup was requested" (the
    // `ListBackups` doc). Rows put each bound ON the creation instant and a second either
    // side of it.
    let cases: [(&str, Option<f64>, Option<f64>, bool); 6] = [
        ("the lower bound at the creation instant", Some(created), None, true),
        ("the lower bound a second later", Some(created + 1.0), None, false),
        ("the upper bound at the creation instant", None, Some(created), false),
        ("the upper bound a second later", None, Some(created + 1.0), true),
        (
            "a window around the creation instant",
            Some(created - 1.0),
            Some(created + 1.0),
            true,
        ),
        (
            "a window that ends before it",
            Some(created - 2.0),
            Some(created - 1.0),
            false,
        ),
    ];

    for (case, lower, upper, listed) in cases {
        let mut request = json!({"TableName": "Ranged"});
        if let Some(bound) = lower {
            request["TimeRangeLowerBound"] = json!(bound);
        }
        if let Some(bound) = upper {
            request["TimeRangeUpperBound"] = json!(bound);
        }
        let body = ok(&router, "ListBackups", request.clone()).await;
        let expected = if listed {
            vec![arn.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            listed_arns(&body),
            expected,
            "case {case}: {request} selects the backup only inside the half-open range, \
             got {body}"
        );
    }
}

#[tokio::test]
async fn a_page_carries_at_most_the_limit_and_the_cursor_resumes_after_it() {
    let router = emulator();
    simple_table(&router, "Paged").await;
    let mut all: Vec<String> = Vec::new();
    for index in 0..5 {
        let details = take_backup(&router, "Paged", &format!("Page{index}")).await;
        all.push(details["BackupArn"].as_str().unwrap_or_default().to_string());
    }
    settle().await;
    all.sort();

    // Rows differ in the page size asked for: each published bound of the modelled
    // 1..=100 range, and sizes either side of the five records held. Paging to the end
    // reports every record exactly once, in ascending ARN order, and
    // `LastEvaluatedBackupArn` is set exactly while more remain.
    for (case, limit) in [
        ("the smallest page", 1_i64),
        ("a page smaller than the catalogue", 2),
        ("a page exactly the size of the catalogue", 5),
        ("a page larger than the catalogue", 6),
        ("the largest page", 100),
    ] {
        let mut walked: Vec<String> = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut request = json!({"TableName": "Paged", "Limit": limit});
            if let Some(start) = &cursor {
                request["ExclusiveStartBackupArn"] = json!(start);
            }
            let page = ok(&router, "ListBackups", request).await;
            let arns = listed_arns(&page);
            let expected_last = usize::try_from(limit).unwrap_or(0) < all.len() - walked.len();
            assert_eq!(
                (
                    arns.len() <= usize::try_from(limit).unwrap_or(0),
                    page["LastEvaluatedBackupArn"].is_string(),
                ),
                (true, expected_last),
                "case {case}: a page carries at most {limit} entries and reports a cursor \
                 only while more remain, got {page}"
            );
            walked.extend(arns);
            cursor = page["LastEvaluatedBackupArn"]
                .as_str()
                .map(ToString::to_string);
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, all,
            "case {case}: paging to the end reports every backup once, in ARN order"
        );
    }

    // The modelled range for `Limit` is 1..=100
    // (`smithy_query(query="shape", shape="BackupsInputLimit")`), so a value just past a
    // published bound is refused rather than clamped.
    for (case, limit) in [("one below the smallest page", 0_i64), ("one above the largest page", 101)] {
        let (status, body) = call(
            &router,
            "ListBackups",
            json!({"TableName": "Paged", "Limit": limit}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case}: {limit} is outside the declared range, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn deleting_a_backup_removes_it_and_reports_it_deleted() {
    let router = emulator();
    simple_table(&router, "Deleted").await;
    let kept = take_backup(&router, "Deleted", "Kept").await;
    let doomed = take_backup(&router, "Deleted", "Doomed").await;
    settle().await;
    let kept_arn = kept["BackupArn"].as_str().unwrap_or_default().to_string();
    let doomed_arn = doomed["BackupArn"].as_str().unwrap_or_default().to_string();

    let removed = ok(&router, "DeleteBackup", json!({"BackupArn": doomed_arn})).await;
    assert_eq!(
        (
            removed["BackupDescription"]["BackupDetails"]["BackupArn"].clone(),
            removed["BackupDescription"]["BackupDetails"]["BackupStatus"].clone(),
            removed["BackupDescription"]["BackupDetails"]["BackupName"].clone(),
            removed["BackupDescription"]["SourceTableDetails"]["TableName"].clone(),
        ),
        (
            json!(doomed_arn),
            json!("DELETED"),
            json!("Doomed"),
            json!("Deleted"),
        ),
        "case the removed backup: the delete reports the record it removed, got {removed}"
    );

    // The record is gone for every reader, and only that one: the other backup of the
    // same table is untouched.
    let listed = ok(&router, "ListBackups", json!({"TableName": "Deleted"})).await;
    assert_eq!(
        listed_arns(&listed),
        vec![kept_arn.clone()],
        "case after the delete: the list reports the backups still held, got {listed}"
    );
    for (case, operation) in [
        ("describing it again", "DescribeBackup"),
        ("deleting it again", "DeleteBackup"),
    ] {
        let (status, body) = call(&router, operation, json!({"BackupArn": doomed_arn})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "BackupNotFoundException"),
            "case {case}: a removed backup is found by nothing, got {status} with {body}"
        );
    }

    // Deleting the SOURCE TABLE leaves its backups alone: a backup is a resource of its
    // own, which is the point of taking one.
    ok(&router, "DeleteTable", json!({"TableName": "Deleted"})).await;
    settle().await;
    let orphaned = ok(&router, "ListBackups", json!({})).await;
    let described = ok(&router, "DescribeBackup", json!({"BackupArn": kept_arn})).await;
    assert_eq!(
        (
            listed_arns(&orphaned),
            described["BackupDescription"]["SourceTableDetails"]["TableName"].clone(),
        ),
        (vec![kept_arn.clone()], json!("Deleted")),
        "case after the source table is deleted: its backup is still held, got \
         {orphaned} and {described}"
    );
}

#[tokio::test]
async fn a_backup_still_being_created_is_not_one_to_delete() {
    let router = emulator();
    simple_table(&router, "Racing").await;

    // "The backup is either being created, deleted or restored to a table"
    // (`BackupInUseException`): while the clock still owes the backup its
    // `CREATING` -> `AVAILABLE` transition, a delete is refused — and once the span has
    // elapsed the same call is served.
    let details = take_backup(&router, "Racing", "Racer").await;
    let arn = details["BackupArn"].as_str().unwrap_or_default().to_string();
    let (status, body) = call(&router, "DeleteBackup", json!({"BackupArn": arn})).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "BackupInUseException"),
        "case while it is still being created: the delete is refused, got {status} with {body}"
    );

    settle().await;
    let removed = ok(&router, "DeleteBackup", json!({"BackupArn": arn})).await;
    assert_eq!(
        removed["BackupDescription"]["BackupDetails"]["BackupStatus"].clone(),
        json!("DELETED"),
        "case once it is available: the delete is served, got {removed}"
    );
}

#[tokio::test]
async fn an_arn_that_names_no_backup_of_this_endpoint_is_refused() {
    let router = emulator();
    simple_table(&router, "Addressed").await;
    let details = take_backup(&router, "Addressed", "Addressable").await;
    settle().await;
    let arn = details["BackupArn"].as_str().unwrap_or_default().to_string();
    let (table_arn, _) = table_identity(&router, "Addressed").await;

    // Rows differ in HOW the value fails to name a backup of this endpoint. A value that
    // is not such an ARN at all names no resource whose existence could be looked up, so
    // it is a `ValidationException`; a well-formed, in-scope ARN the catalogue does not
    // hold is the `BackupNotFoundException` both operations model.
    let cases: [(&str, String, &str); 6] = [
        (
            "the table's own ARN, with no backup segment",
            table_arn.clone(),
            "ValidationException",
        ),
        (
            "an ARN of another service",
            arn.replace(":dynamodb:", ":dynamodb-else:"),
            "ValidationException",
        ),
        (
            "an ARN of another region",
            arn.replace(&format!(":{REGION}:"), ":eu-west-1:"),
            "ValidationException",
        ),
        (
            "a value that is no ARN at all",
            "not-an-arn-but-long-enough-to-be-modelled".to_string(),
            "ValidationException",
        ),
        (
            "a backup segment that is empty",
            format!("{table_arn}/backup/"),
            "ValidationException",
        ),
        (
            "an ARN this endpoint could hold but does not",
            format!("{table_arn}/backup/01700000000000-abcdef12"),
            "BackupNotFoundException",
        ),
    ];

    for (case, addressed, expected) in cases {
        for operation in ["DescribeBackup", "DeleteBackup"] {
            let (status, body) =
                call(&router, operation, json!({"BackupArn": addressed})).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, expected),
                "case {case} on {operation}: got {status} with {body}"
            );
        }
    }

    // The backup that IS addressable is still served, so the refusals above are about
    // the values sent and not about the operations.
    let served = ok(&router, "DescribeBackup", json!({"BackupArn": arn})).await;
    assert_eq!(
        served["BackupDescription"]["BackupDetails"]["BackupArn"].clone(),
        json!(arn),
        "case the addressable backup: it is served, got {served}"
    );
}

#[tokio::test]
async fn a_table_that_cannot_be_backed_up_is_refused() {
    let router = emulator();

    // Rows differ in why the table is not one to catalogue: it was never created, it was
    // created and deleted, and it is still being created. The backup path acts on a table
    // whose creation has FINISHED and whose deletion has not started, so each of the three
    // is the `TableNotFoundException` this operation models — "A source table with the
    // name TableName does not currently exist" — and none is a conflict a caller would
    // retry in place.
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "Removed",
            "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    settle().await;
    ok(&router, "DeleteTable", json!({"TableName": "Removed"})).await;
    settle().await;
    // Not settled: this one is still inside its `CREATING` span.
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "Creating",
            "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;

    for (case, table, expected) in [
        ("a table that was never created", "Absent", "TableNotFoundException"),
        ("a table that was deleted", "Removed", "TableNotFoundException"),
        ("a table that is still being created", "Creating", "TableNotFoundException"),
    ] {
        let (status, body) = call(
            &router,
            "CreateBackup",
            json!({"TableName": table, "BackupName": "Attempted"}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, expected),
            "case {case}: got {status} with {body}"
        );
    }

    // None of the refused requests catalogued anything.
    let listed = ok(&router, "ListBackups", json!({})).await;
    assert_eq!(
        listed_arns(&listed),
        Vec::<String>::new(),
        "case after the refusals: nothing was catalogued, got {listed}"
    );

    // The table that settles IS backed up, so the refusals are about the tables named.
    settle().await;
    let details = take_backup(&router, "Creating", "Settled").await;
    assert_eq!(
        details["BackupName"].clone(),
        json!("Settled"),
        "case the settled table: a backup of it is catalogued, got {details}"
    );
}
