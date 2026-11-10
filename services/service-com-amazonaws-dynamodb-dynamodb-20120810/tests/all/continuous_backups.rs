// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the continuous-backups surface: `UpdateContinuousBackups` and
//! the reader it round-trips through, `DescribeContinuousBackups`.
//!
//! The rules pinned here:
//!
//! 1. a table's continuous backups are always on ("Continuous backups are ENABLED on all
//!    tables at table creation" — the `UpdateContinuousBackups` doc), and what the
//!    caller sets is its POINT-IN-TIME RECOVERY: the specification sent is stored on the
//!    table and every later read reports it, so the update and the describe agree;
//! 2. the recovery period is the one the request named, and 35 days when it named none
//!    ("This parameter is optional. If no value is provided, the value will default to
//!    35" — `PointInTimeRecoverySpecification$RecoveryPeriodInDays`);
//! 3. while recovery is on, the restorable window is DERIVED from the stored settings and
//!    the clock — its newest point about five minutes behind now
//!    ("`LatestRestorableDateTime` is typically 5 minutes before the current time") and
//!    its oldest no further back than the configured period, nor than the instant
//!    recovery started — and while recovery is off there is no window at all;
//! 4. a period outside the modelled 1..=35 range is refused as a `ValidationException`,
//!    and a table that does not exist as the `TableNotFoundException` both operations
//!    model.
//!
//! Every test drives the operations over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, and each rule is asserted over a
//! SET of inputs — the period asked for, the form the table is addressed in, the order
//! the toggles come in — rather than one example.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, ok};

/// One toggle a test applies: whether recovery is being turned on, and the period it
/// names (none at all for the documented default).
type Toggle = (bool, Option<i64>);

/// A day in seconds, the unit `RecoveryPeriodInDays` counts in.
const DAY_SECONDS: f64 = 24.0 * 60.0 * 60.0;


/// Wait out one transient span, so whatever the clock owes a table is due.
async fn settle() {
    tokio::time::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(150),
    )
    .await;
}

/// Create one on-demand table to carry the settings under test, and wait out the span
/// the clock owes it: continuous-backups settings are changed on a table whose creation
/// has FINISHED (`tests/all/backup_path_table_state.rs` pins what a table still being
/// created answers), so every row below starts from a settled table.
async fn create_table(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    settle().await;
}

/// The `ContinuousBackupsDescription` of a response under either operation's output
/// member, which is the same shape for both.
fn backups(body: &Value) -> &Value {
    &body["ContinuousBackupsDescription"]
}

/// The `PointInTimeRecoveryDescription` of a response.
fn recovery(body: &Value) -> &Value {
    &backups(body)["PointInTimeRecoveryDescription"]
}

/// Now, as the wire reports a timestamp: epoch seconds.
fn epoch_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// Send one `UpdateContinuousBackups` for `name`, with the period when one is named.
async fn set_recovery(
    router: &MultiServiceRouter,
    name: &str,
    enabled: bool,
    period: Option<i64>,
) -> Value {
    let mut specification = json!({"PointInTimeRecoveryEnabled": enabled});
    if let Some(days) = period {
        specification["RecoveryPeriodInDays"] = json!(days);
    }
    ok(
        router,
        "UpdateContinuousBackups",
        json!({"TableName": name, "PointInTimeRecoverySpecification": specification}),
    )
    .await
}

#[tokio::test]
async fn an_enabled_table_reports_the_period_it_was_given_and_both_operations_agree() {
    let router = emulator();

    // Rows differ in the period asked for: none at all (the documented default), each
    // published bound of the modelled 1..=35 range, and a value inside it. The second
    // column is the period the description then reports.
    let cases: [(&str, Option<i64>, i64); 4] = [
        ("no period named", None, 35),
        ("the smallest period", Some(1), 1),
        ("a period inside the range", Some(7), 7),
        ("the largest period", Some(35), 35),
    ];

    for (case, asked, expected) in cases {
        let name = format!("Recovered{}", case.replace(' ', ""));
        create_table(&router, &name).await;

        let before = epoch_now();
        let updated = set_recovery(&router, &name, true, asked).await;
        let after = epoch_now();
        assert_eq!(
            (
                backups(&updated)["ContinuousBackupsStatus"].clone(),
                recovery(&updated)["PointInTimeRecoveryStatus"].clone(),
                recovery(&updated)["RecoveryPeriodInDays"].clone(),
            ),
            (json!("ENABLED"), json!("ENABLED"), json!(expected)),
            "case {case}: the update reports the settings it stored, got {updated}"
        );

        // The window: newest point no later than now, oldest no earlier than the
        // configured period before now, and ordered. A window that began at this call
        // cannot reach back before it, so both ends sit inside the call's own span.
        let earliest = recovery(&updated)["EarliestRestorableDateTime"]
            .as_f64()
            .unwrap_or_default();
        let latest = recovery(&updated)["LatestRestorableDateTime"]
            .as_f64()
            .unwrap_or_default();
        assert!(
            earliest <= latest && latest <= after && earliest >= before - 1.0,
            "case {case}: the window is ordered and starts where recovery did, got \
             earliest={earliest} latest={latest} against the call's {before}..{after}"
        );

        // The reader reports what the update stored: the round trip.
        let described = ok(&router, "DescribeContinuousBackups", json!({"TableName": name})).await;
        assert_eq!(
            (
                backups(&described)["ContinuousBackupsStatus"].clone(),
                recovery(&described)["PointInTimeRecoveryStatus"].clone(),
                recovery(&described)["RecoveryPeriodInDays"].clone(),
            ),
            (json!("ENABLED"), json!("ENABLED"), json!(expected)),
            "case {case}: the describe reports the stored settings, got {described}"
        );

        // Reading again reports the same SETTINGS — a read stores nothing — and the
        // window still cannot reach back further than the configured period.
        let again = ok(&router, "DescribeContinuousBackups", json!({"TableName": name})).await;
        let read_earliest = recovery(&again)["EarliestRestorableDateTime"]
            .as_f64()
            .unwrap_or_default();
        assert_eq!(
            (
                recovery(&again)["PointInTimeRecoveryStatus"].clone(),
                recovery(&again)["RecoveryPeriodInDays"].clone(),
            ),
            (json!("ENABLED"), json!(expected)),
            "case {case}: a second read reports the same settings, got {again}"
        );
        let oldest_allowed = epoch_now() - (expected as f64) * DAY_SECONDS;
        assert!(
            read_earliest >= oldest_allowed && read_earliest >= before - 1.0,
            "case {case}: the oldest restorable point stays inside the {expected}-day \
             period, got {read_earliest} against {oldest_allowed}"
        );
    }
}

#[tokio::test]
async fn a_table_reports_recovery_disabled_until_it_is_turned_on_and_again_once_off() {
    let router = emulator();

    // Rows differ in what has happened to the table's recovery before it is read: never
    // touched, turned on and off again, and turned off without ever being on. Each one
    // reports continuous backups ENABLED with recovery DISABLED and NO window, because a
    // table with recovery off has no point in time to restore to.
    let cases: [(&str, &[Toggle]); 4] = [
        ("never set", &[]),
        ("turned off without being on", &[(false, None)]),
        ("turned on then off", &[(true, Some(12)), (false, None)]),
        (
            "turned on, re-stated, then off",
            &[(true, None), (true, Some(3)), (false, None)],
        ),
    ];

    for (case, toggles) in cases {
        let name = format!("Toggled{}", case.replace([' ', ','], ""));
        create_table(&router, &name).await;
        for (enabled, period) in toggles {
            set_recovery(&router, &name, *enabled, *period).await;
        }

        let described = ok(&router, "DescribeContinuousBackups", json!({"TableName": name})).await;
        assert_eq!(
            (
                backups(&described)["ContinuousBackupsStatus"].clone(),
                recovery(&described)["PointInTimeRecoveryStatus"].clone(),
                recovery(&described)["RecoveryPeriodInDays"].clone(),
                recovery(&described)["EarliestRestorableDateTime"].clone(),
                recovery(&described)["LatestRestorableDateTime"].clone(),
            ),
            (
                json!("ENABLED"),
                json!("DISABLED"),
                Value::Null,
                Value::Null,
                Value::Null,
            ),
            "case {case}: backups stay on and recovery is off with no window, got {described}"
        );

        // Turning it on again opens a window that starts NOW: the one the earlier
        // enable opened did not survive the disable.
        let reopened = epoch_now();
        let enabled = set_recovery(&router, &name, true, Some(20)).await;
        let earliest = recovery(&enabled)["EarliestRestorableDateTime"]
            .as_f64()
            .unwrap_or_default();
        assert!(
            recovery(&enabled)["PointInTimeRecoveryStatus"] == json!("ENABLED")
                && recovery(&enabled)["RecoveryPeriodInDays"] == json!(20)
                && earliest >= reopened - 1.0,
            "case {case}: re-enabling opens a window starting now, got {enabled}"
        );
    }
}

#[tokio::test]
async fn re_stating_an_enabled_setting_keeps_the_window_it_already_had() {
    let router = emulator();
    create_table(&router, "Restated").await;

    // The first enable is what the window starts from; a later enable changes the
    // period without restarting the window, so the oldest restorable point does not
    // jump forward. Rows differ in the period each re-statement asks for.
    let opened = set_recovery(&router, "Restated", true, Some(5)).await;
    let first_earliest = recovery(&opened)["EarliestRestorableDateTime"]
        .as_f64()
        .unwrap_or_default();

    for (case, period, expected) in [
        ("the same period again", Some(5), 5),
        ("a longer period", Some(35), 35),
        ("a shorter period", Some(1), 1),
        ("no period, so the default", None, 35),
    ] {
        let updated = set_recovery(&router, "Restated", true, period).await;
        let earliest = recovery(&updated)["EarliestRestorableDateTime"]
            .as_f64()
            .unwrap_or_default();
        assert_eq!(
            recovery(&updated)["RecoveryPeriodInDays"].clone(),
            json!(expected),
            "case {case}: the re-stated period is the one reported, got {updated}"
        );
        assert!(
            (earliest - first_earliest).abs() < 1.0,
            "case {case}: the window keeps the start the first enable gave it, got \
             {earliest} against {first_earliest}"
        );
    }
}

#[tokio::test]
async fn the_table_can_be_named_by_name_or_by_arn() {
    let router = emulator();
    create_table(&router, "Addressed").await;

    // Rows differ in the form the table is addressed in: "You can also provide the
    // Amazon Resource Name (ARN) of the table in this parameter"
    // (`UpdateContinuousBackups$TableName`). Both name the one stored table, so the
    // second call reads back what the first stored.
    let described = ok(
        &router,
        "DescribeTable",
        json!({"TableName": "Addressed"}),
    )
    .await;
    let arn = described["Table"]["TableArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    for (case, written, read) in [
        ("written by name, read by ARN", "Addressed".to_string(), arn.clone()),
        ("written by ARN, read by name", arn.clone(), "Addressed".to_string()),
    ] {
        let updated = set_recovery(&router, &written, true, Some(9)).await;
        let read_back = ok(
            &router,
            "DescribeContinuousBackups",
            json!({"TableName": read}),
        )
        .await;
        assert_eq!(
            (
                recovery(&updated)["RecoveryPeriodInDays"].clone(),
                recovery(&read_back)["RecoveryPeriodInDays"].clone(),
                recovery(&read_back)["PointInTimeRecoveryStatus"].clone(),
            ),
            (json!(9), json!(9), json!("ENABLED")),
            "case {case}: both forms name the same table, got {updated} then {read_back}"
        );
        set_recovery(&router, &written, false, None).await;
    }
}

#[tokio::test]
async fn a_period_outside_the_modelled_range_is_invalid() {
    let router = emulator();
    create_table(&router, "Bounded").await;

    // The range `RecoveryPeriodInDays` declares is 1..=35
    // (`smithy_query(query="shape", shape="RecoveryPeriodInDays")`), so each value just
    // past a published bound is refused while the bounds themselves are served.
    for (case, days) in [
        ("one below the smallest period", 0_i64),
        ("one above the largest period", 36),
        ("far below the smallest period", -7),
        ("far above the largest period", 365),
    ] {
        let (status, body) = call(
            &router,
            "UpdateContinuousBackups",
            json!({
                "TableName": "Bounded",
                "PointInTimeRecoverySpecification": {
                    "PointInTimeRecoveryEnabled": true,
                    "RecoveryPeriodInDays": days,
                },
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case}: {days} days is outside the declared range, got {status} with {body}"
        );
    }

    // The refused requests stored nothing: the table's recovery is still off.
    let described = ok(
        &router,
        "DescribeContinuousBackups",
        json!({"TableName": "Bounded"}),
    )
    .await;
    assert_eq!(
        recovery(&described)["PointInTimeRecoveryStatus"].clone(),
        json!("DISABLED"),
        "case after the refusals: nothing was stored, got {described}"
    );

    for (case, days) in [("the smallest period", 1_i64), ("the largest period", 35)] {
        let updated = set_recovery(&router, "Bounded", true, Some(days)).await;
        assert_eq!(
            recovery(&updated)["RecoveryPeriodInDays"].clone(),
            json!(days),
            "case {case}: a published bound is served, got {updated}"
        );
    }
}

#[tokio::test]
async fn every_status_reported_is_one_the_model_declares() {
    // The two status members are enums, so what a description reports must be a value
    // from the model's own table — the generated
    // `CONTINUOUS_BACKUPS_STATUS_VALUES` / `POINT_IN_TIME_RECOVERY_STATUS_VALUES` — and
    // not a spelling this emulator invented. Rows span the states a table can be read
    // in: recovery never set, on with the default period, on with a named period, and
    // off again.
    use service_com_amazonaws_dynamodb_dynamodb_20120810::ops::types::{
        CONTINUOUS_BACKUPS_STATUS_VALUES, POINT_IN_TIME_RECOVERY_STATUS_VALUES,
    };

    let router = emulator();
    create_table(&router, "Declared").await;

    let cases: [(&str, Option<Toggle>); 4] = [
        ("never set", None),
        ("on with the default period", Some((true, None))),
        ("on with a named period", Some((true, Some(14)))),
        ("off again", Some((false, None))),
    ];
    for (case, toggle) in cases {
        if let Some((enabled, period)) = toggle {
            set_recovery(&router, "Declared", enabled, period).await;
        }
        let described = ok(
            &router,
            "DescribeContinuousBackups",
            json!({"TableName": "Declared"}),
        )
        .await;
        let backups_status = backups(&described)["ContinuousBackupsStatus"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let recovery_status = recovery(&described)["PointInTimeRecoveryStatus"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            CONTINUOUS_BACKUPS_STATUS_VALUES.contains(&backups_status.as_str())
                && POINT_IN_TIME_RECOVERY_STATUS_VALUES.contains(&recovery_status.as_str()),
            "case {case}: both statuses are values the model declares, got \
             {backups_status:?} and {recovery_status:?} in {described}"
        );
    }
}

#[tokio::test]
async fn a_table_that_does_not_exist_is_refused_by_both_operations() {
    let router = emulator();
    create_table(&router, "Present").await;

    // Rows differ in why the table is not there: never created, and created then
    // deleted. Both operations model `TableNotFoundException` — "A source table with the
    // name TableName does not currently exist" — rather than the control plane's
    // `ResourceNotFoundException`.
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

    for case in ["Absent", "Removed"] {
        for operation in ["DescribeContinuousBackups", "UpdateContinuousBackups"] {
            let mut request = json!({"TableName": case});
            if operation == "UpdateContinuousBackups" {
                request["PointInTimeRecoverySpecification"] =
                    json!({"PointInTimeRecoveryEnabled": true});
            }
            let (status, body) = call(&router, operation, request).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "TableNotFoundException"),
                "case {case} on {operation}: a table that is not there is refused, got {status} \
                 with {body}"
            );
        }
    }

    // The table that IS there is still served, so the refusals above are about the
    // table named and not about the operations.
    let served = set_recovery(&router, "Present", true, None).await;
    assert_eq!(
        recovery(&served)["PointInTimeRecoveryStatus"].clone(),
        json!("ENABLED"),
        "case the present table: it is served, got {served}"
    );
}
