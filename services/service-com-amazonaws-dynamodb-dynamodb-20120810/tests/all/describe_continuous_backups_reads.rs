// SPDX-License-Identifier: CC0-1.0

//! `DescribeContinuousBackups` as a REPORTING operation.
//!
//! `tests/all/continuous_backups.rs` pins what the two continuous-backups operations
//! agree about — the settings stored and the window derived from them. This suite pins
//! the properties of the READER itself, which no row there covers:
//!
//! > `DescribeContinuousBackups` computes its answer from the settings held on the table
//! > its own request's region keys, whatever status that table is in, resolving the table
//! > name the way every other operation of this service resolves it — and it stores
//! > nothing, so a read never changes what a later caller sees.
//!
//! Each rule is asserted over a SET of inputs: the states a table's recovery can be read
//! in, the regions the same table name can exist in side by side, the statuses a table
//! can be read in, and the forms a table name can be sent in.
//!
//! Every call goes over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call_in, emulator, error_code, message, REGION};

/// One `UpdateContinuousBackups` a case applies first: whether recovery is being turned
/// on, and the period it names (`None` for a request that names none).
type Toggle = (bool, Option<i64>);

/// Wait out one transient span, so whatever the clock owes a table is due.
async fn settle() {
    tokio::time::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(150),
    )
    .await;
}

/// Send one operation in `region` that must succeed, and answer its parsed body.
async fn ok_in(router: &MultiServiceRouter, region: &str, operation: &str, body: Value) -> Value {
    let (status, parsed) = call_in(router, region, operation, body.clone()).await;
    assert_eq!(
        status, 200,
        "case {operation} in {region} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// Create one on-demand table in `region` to carry the settings under test.
async fn create_table(router: &MultiServiceRouter, region: &str, name: &str) {
    ok_in(
        router,
        region,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
}

/// The same, waiting out the span the clock owes the new table: a case that WRITES the
/// table's recovery settings starts from a table whose creation has finished
/// (`tests/all/backup_path_table_state.rs` pins what one still being created answers).
async fn active_table(router: &MultiServiceRouter, region: &str, name: &str) {
    create_table(router, region, name).await;
    settle().await;
}

/// Apply one `PointInTimeRecoverySpecification` to `name` in `region`.
async fn set_recovery(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    enabled: bool,
    period: Option<i64>,
) -> Value {
    let mut specification = json!({"PointInTimeRecoveryEnabled": enabled});
    if let Some(days) = period {
        specification["RecoveryPeriodInDays"] = json!(days);
    }
    ok_in(
        router,
        region,
        "UpdateContinuousBackups",
        json!({"TableName": name, "PointInTimeRecoverySpecification": specification}),
    )
    .await
}

/// Read `name` back in `region` through the operation under test.
async fn describe_backups(router: &MultiServiceRouter, region: &str, name: &str) -> Value {
    ok_in(
        router,
        region,
        "DescribeContinuousBackups",
        json!({"TableName": name}),
    )
    .await
}

/// The `PointInTimeRecoveryDescription` of a response.
fn recovery(body: &Value) -> &Value {
    &body["ContinuousBackupsDescription"]["PointInTimeRecoveryDescription"]
}

/// The three members a read must report the same way every time: the table's
/// continuous-backups status and its recovery status and period.
fn settings(body: &Value) -> (Value, Value, Value) {
    (
        body["ContinuousBackupsDescription"]["ContinuousBackupsStatus"].clone(),
        recovery(body)["PointInTimeRecoveryStatus"].clone(),
        recovery(body)["RecoveryPeriodInDays"].clone(),
    )
}

/// The two ends of the restorable window, as epoch seconds, or `None` unless BOTH are
/// reported — which is the answer for a table whose recovery is off, and a defect for
/// one whose recovery is on.
fn window(body: &Value) -> Option<(f64, f64)> {
    let earliest = recovery(body)["EarliestRestorableDateTime"].as_f64()?;
    let latest = recovery(body)["LatestRestorableDateTime"].as_f64()?;
    Some((earliest, latest))
}

/// The members of a `DescribeTable` that no read of any other operation may move: this
/// table's identity and the status it is in.
fn table_identity(described: &Value) -> Value {
    json!({
        "TableName": described["Table"]["TableName"].clone(),
        "TableArn": described["Table"]["TableArn"].clone(),
        "TableId": described["Table"]["TableId"].clone(),
        "TableStatus": described["Table"]["TableStatus"].clone(),
        "CreationDateTime": described["Table"]["CreationDateTime"].clone(),
        "ItemCount": described["Table"]["ItemCount"].clone(),
    })
}

/// The identity members of the table named, as a later caller's `DescribeTable` reports
/// them.
async fn table_snapshot(router: &MultiServiceRouter, name: &str) -> Value {
    table_identity(&ok_in(router, REGION, "DescribeTable", json!({"TableName": name})).await)
}

#[tokio::test]
async fn repeated_reads_report_the_stored_settings_and_store_nothing() {
    let router = emulator();

    // Rows differ in the state the table's recovery is read in — never touched, on with
    // the period the request named, on with the documented default, and turned off again
    // after being on — and in how many writes got it there. The last two columns are the
    // recovery status and period every read of that state must report.
    let cases: [(&str, &[Toggle], &str, Value); 5] = [
        ("never set", &[], "DISABLED", Value::Null),
        (
            "on with a named period",
            &[(true, Some(4))],
            "ENABLED",
            json!(4),
        ),
        (
            "on with the default period",
            &[(true, None)],
            "ENABLED",
            json!(35),
        ),
        (
            "on, then re-stated with another period",
            &[(true, Some(2)), (true, Some(31))],
            "ENABLED",
            json!(31),
        ),
        (
            "on, then off again",
            &[(true, Some(9)), (false, None)],
            "DISABLED",
            Value::Null,
        ),
    ];

    for (case, toggles, status, period) in cases {
        let name = format!("Read{}", case.replace([' ', ','], ""));
        active_table(&router, REGION, &name).await;
        for (enabled, days) in toggles {
            set_recovery(&router, REGION, &name, *enabled, *days).await;
        }

        // Let the clock finish what it owes the new table before the reads, so the
        // status this comparison carries is a terminal one and the only thing that
        // could move it is a write.
        settle().await;
        let before = table_snapshot(&router, &name).await;
        let first = describe_backups(&router, REGION, &name).await;
        assert_eq!(
            settings(&first),
            (json!("ENABLED"), json!(status), period.clone()),
            "case {case}: the read reports the settings the writes left, got {first}"
        );

        // Reading again, and again, reports the SAME settings: this operation derives
        // its answer and persists none of it, so no read can move the next one's.
        let first_latest = window(&first).map(|(_, latest)| latest);
        for repeat in 1..=3 {
            let again = describe_backups(&router, REGION, &name).await;
            assert_eq!(
                settings(&again),
                settings(&first),
                "case {case}: read {repeat} reports what read 1 did, got {again}"
            );
            if status == "ENABLED" {
                // Both ends are REPORTED while recovery is on, ordered, and no older
                // than the first read's: the window moves only with the clock.
                let Some((earliest, latest)) = window(&again) else {
                    panic!(
                        "case {case}: read {repeat} of a table with recovery on reports \
                         both ends of the window, got {again}"
                    )
                };
                let Some(first_latest) = first_latest else {
                    panic!(
                        "case {case}: the first read of a table with recovery on reports \
                         both ends of the window, got {first}"
                    )
                };
                assert!(
                    earliest <= latest && latest >= first_latest,
                    "case {case}: read {repeat}'s window is ordered and no older than the \
                     first read's, got earliest={earliest} latest={latest} against \
                     {first_latest}"
                );
            } else {
                // A table with recovery off has no point in time to restore to, so
                // neither end of the window is reported at all.
                assert_eq!(
                    (
                        recovery(&again)["EarliestRestorableDateTime"].clone(),
                        recovery(&again)["LatestRestorableDateTime"].clone(),
                    ),
                    (Value::Null, Value::Null),
                    "case {case}: read {repeat} of a table with recovery off reports no \
                     window, got {again}"
                );
            }
        }

        // ...and no other reader's answer moved either: a read is not a write.
        let after = table_snapshot(&router, &name).await;
        assert_eq!(
            after, before,
            "case {case}: the table a later caller describes is the one it was before the \
             reads, got {after} against {before}"
        );
    }
}

#[tokio::test]
async fn each_region_reports_the_settings_of_its_own_table() {
    let router = emulator();
    // One table NAME, held by several regions at once: "table names must be unique within
    // each Region" (the `CreateTable` doc), so the settings a read reports are the ones
    // its own request's region stored. Rows differ in the region and in the settings
    // written there, including a region where recovery was never turned on.
    let name = "SharedName";
    let rows: [(&str, Option<Toggle>, &str, Value); 4] = [
        (REGION, Some((true, Some(1))), "ENABLED", json!(1)),
        ("eu-west-1", Some((true, Some(35))), "ENABLED", json!(35)),
        ("ap-south-1", Some((true, None)), "ENABLED", json!(35)),
        ("sa-east-1", None, "DISABLED", Value::Null),
    ];

    for (region, toggle, _, _) in &rows {
        active_table(&router, region, name).await;
        if let Some((enabled, period)) = toggle {
            set_recovery(&router, region, name, *enabled, *period).await;
        }
    }

    for (region, _, status, period) in &rows {
        let described = describe_backups(&router, region, name).await;
        assert_eq!(
            settings(&described),
            (json!("ENABLED"), json!(*status), period.clone()),
            "case {region}: the region's own table's settings are reported, got {described}"
        );
    }

    // A region that holds no table by that name is refused, whatever other regions hold:
    // `TableNotFoundException` — "A source table with the name TableName does not
    // currently exist within the subscriber's account or the subscriber is operating in
    // the wrong Amazon Web Services Region".
    for elsewhere in ["eu-north-1", "us-west-2"] {
        let (status, body) = call_in(
            &router,
            elsewhere,
            "DescribeContinuousBackups",
            json!({"TableName": name}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "TableNotFoundException"),
            "case {elsewhere}: a region holding no such table refuses the read, got \
             {status} with {body}"
        );
        assert!(
            message(&body).contains(name),
            "case {elsewhere}: the refusal names the table asked for, got {}",
            message(&body)
        );
    }
}

#[tokio::test]
async fn a_table_is_served_whatever_status_it_is_read_in() {
    let router = emulator();
    // "Continuous backups are ENABLED on all tables at table creation" (the
    // `UpdateContinuousBackups` doc), so the reader answers for a table that is still
    // being created as well as for one that is active. Rows are the statuses a table
    // created and then left alone passes through, in order.
    let name = "StatusSpanned";
    create_table(&router, REGION, name).await;

    let creating = ok_in(&router, REGION, "DescribeTable", json!({"TableName": name})).await;
    let while_creating = describe_backups(&router, REGION, name).await;
    settle().await;
    let active = ok_in(&router, REGION, "DescribeTable", json!({"TableName": name})).await;
    let while_active = describe_backups(&router, REGION, name).await;

    for (case, table, backups) in [
        ("while creating", &creating, &while_creating),
        ("once active", &active, &while_active),
    ] {
        assert_eq!(
            settings(backups),
            (json!("ENABLED"), json!("DISABLED"), Value::Null),
            "case {case} (table status {}): continuous backups are on and recovery is \
             off, got {backups}",
            table["Table"]["TableStatus"]
        );
    }
    // The status the first pair was taken in is whatever the clock had reached by then —
    // `CREATING` while the span is still running, `ACTIVE` once it has elapsed — and the
    // settled pair is taken in the terminal one. Both are statuses this reader serves, so
    // the rows span them without the scheduler deciding the assertion.
    let first_status = creating["Table"]["TableStatus"].clone();
    assert!(
        [json!("CREATING"), json!("ACTIVE")].contains(&first_status),
        "case the first status: a table just created is reported in one of the statuses \
         its creation passes through, got {first_status}"
    );
    assert_eq!(
        active["Table"]["TableStatus"].clone(),
        json!("ACTIVE"),
        "case the settled status: the table the second pair was read in is active, got {}",
        active["Table"]["TableStatus"]
    );

    // Turning recovery on while the table is active is reported by the next read, so the
    // reader is answering from the table's own settings and not from its status.
    set_recovery(&router, REGION, name, true, Some(6)).await;
    let described = describe_backups(&router, REGION, name).await;
    assert_eq!(
        settings(&described),
        (json!("ENABLED"), json!("ENABLED"), json!(6)),
        "case after the enable: the read reports the new settings, got {described}"
    );
}

#[tokio::test]
async fn the_table_name_is_resolved_the_way_every_operation_resolves_it() {
    let router = emulator();
    let name = "Resolvable";
    active_table(&router, REGION, name).await;
    let arn = ok_in(&router, REGION, "DescribeTable", json!({"TableName": name}))
        .await["Table"]["TableArn"]
        .as_str()
        .expect("DescribeTable reports the table's ARN")
        .to_string();
    set_recovery(&router, REGION, name, true, Some(21)).await;

    // The member takes the bare name or the table's ARN ("You can also provide the Amazon
    // Resource Name (ARN) of the table in this parameter" —
    // `DescribeContinuousBackups$TableName`), and both name the one stored table.
    for (case, sent) in [
        ("by name", name.to_string()),
        ("by ARN", arn.clone()),
    ] {
        let described = describe_backups(&router, REGION, &sent).await;
        assert_eq!(
            settings(&described),
            (json!("ENABLED"), json!("ENABLED"), json!(21)),
            "case {case}: both forms address the same table, got {described}"
        );
    }

    // A name outside the documented 3..=255 `[a-zA-Z0-9_.-]+` form (the
    // `CreateTable$TableName` doc) is refused as a constraint violation, stated for this
    // operation exactly as this service states it for every other: one row per bound and
    // one for the pattern.
    let long_name = "a".repeat(256);
    let refusals: [(&str, String, String); 3] = [
        (
            "one character below the smallest name",
            "ab".to_string(),
            "1 validation error detected: Value 'ab' at 'tableName' failed to satisfy \
             constraint: Member must have length greater than or equal to 3"
                .to_string(),
        ),
        (
            "one character past the largest name",
            long_name.clone(),
            format!(
                "1 validation error detected: Value '{long_name}' at 'tableName' failed to \
                 satisfy constraint: Member must have length less than or equal to 255"
            ),
        ),
        (
            "a character the pattern does not allow",
            "bad name!".to_string(),
            "1 validation error detected: Value 'bad name!' at 'tableName' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+"
                .to_string(),
        ),
    ];
    for (case, sent, expected) in refusals {
        let (status, body) = call_in(
            &router,
            REGION,
            "DescribeContinuousBackups",
            json!({"TableName": sent}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str(), message(&body)),
            (400, "ValidationException", expected.clone()),
            "case {case} ({sent:?}): expected the stated constraint {expected:?}, got \
             {status} with {body}"
        );
    }

    // A name AT each published bound is a name, so it resolves to a table that is simply
    // not there rather than to a constraint violation: the refusals above are the bounds
    // and not an off-by-one.
    for (case, sent) in [
        ("the smallest name", "abc".to_string()),
        ("the largest name", "a".repeat(255)),
    ] {
        let (status, body) = call_in(
            &router,
            REGION,
            "DescribeContinuousBackups",
            json!({"TableName": sent}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "TableNotFoundException"),
            "case {case}: a name at the bound resolves to a missing table, got {status} \
             with {body}"
        );
    }
}
