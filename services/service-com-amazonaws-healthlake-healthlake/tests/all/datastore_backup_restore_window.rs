// SPDX-License-Identifier: CC0-1.0

//! The restore window every read of a data store reports.
//!
//! The rule under test, over a SET of backup configurations rather than one
//! case: a data store whose backup is enabled reports, on EVERY read of its
//! properties, the two bounds of the window it can be restored to —
//! `EarliestRestorePoint` ("the earliest point in time the data store can be
//! restored to"), which for a continuous backup is the instant backup became
//! enabled unless the retention period the configuration names starts the window
//! later, and `LatestRestorePoint` ("the latest point in time the data store can
//! be restored to"), which is the instant of the read — and a data store with no
//! enabled backup reports neither, because it cannot be restored at all. The
//! window a read reports is the SAME window a restore holds a requested restore
//! point against.
//!
//! Member names, their documentation and every value set and bound used below
//! come from `smithy_query(query="shape",
//! shape="DatastoreBackupStatus,BackupConfiguration")`: `Status` is one of
//! ENABLED / DISABLED, `RetentionPeriodInDays` is modelled 1..30, and
//! `BackupTagsEnabled` is a boolean.
//!
//! Every call goes through [`MultiServiceRouter::dispatch`](smithy_server_runtime::MultiServiceRouter)
//! by way of the shared harness in `fhir_datastore_lifecycle`, so routing,
//! deserialization, the modelled constraints and the error envelope are all
//! exercised.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::fhir_datastore_lifecycle::{error_code, error_message, HealthLake, PAST_TRANSITION};

/// How far apart two renderings of ONE instant may be and still be the same
/// instant: ten microseconds, which absorbs the sub-microsecond digits the
/// date-time rendering drops and the ones epoch seconds in a double cannot hold,
/// while staying far below the milliseconds any genuine drift would cost.
const TIMESTAMP_TOLERANCE: f64 = 1e-5;

/// Whether two reported instants are the same instant.
fn same_instant(left: f64, right: f64) -> bool {
    (left - right).abs() < TIMESTAMP_TOLERANCE
}

/// Seconds since the epoch, the scale awsJson1_0 renders a modelled timestamp
/// (`HealthLakeTimestamp`) on.
fn epoch_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// One backup configuration the table spans, and whether a data store holding it
/// has a restore window at all.
struct Row {
    case: &'static str,
    /// The `BackupConfiguration` the create carries, or `Value::Null` for a
    /// create that carries none.
    configuration: Value,
    /// Whether this configuration leaves the data store restorable, which is
    /// what decides whether the two restore points are reported.
    restorable: bool,
}

/// The configurations under test. The rule generalises over whether backup is
/// enabled and over the retention period, so the rows differ along both: no
/// configuration at all, an explicitly disabled one, and an enabled one at each
/// published bound of the modelled 1..30 retention range, one value in between
/// that no recorded case names, and one with backup tags turned on.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no backup configuration at all",
            configuration: Value::Null,
            restorable: false,
        },
        Row {
            case: "backup explicitly DISABLED",
            configuration: json!({
                "Status": "DISABLED",
                "BackupType": "CONTINUOUS",
                "RetentionPeriodInDays": 7,
                "BackupTagsEnabled": false,
            }),
            restorable: false,
        },
        Row {
            case: "backup ENABLED, retention at the published minimum of 1 day",
            configuration: json!({
                "Status": "ENABLED",
                "BackupType": "CONTINUOUS",
                "RetentionPeriodInDays": 1,
                "BackupTagsEnabled": false,
            }),
            restorable: true,
        },
        Row {
            case: "backup ENABLED, retention of 23 days",
            configuration: json!({
                "Status": "ENABLED",
                "BackupType": "CONTINUOUS",
                "RetentionPeriodInDays": 23,
                "BackupTagsEnabled": false,
            }),
            restorable: true,
        },
        Row {
            case: "backup ENABLED, retention at the published maximum of 30 days",
            configuration: json!({
                "Status": "ENABLED",
                "BackupType": "CONTINUOUS",
                "RetentionPeriodInDays": 30,
                "BackupTagsEnabled": true,
            }),
            restorable: true,
        },
    ]
}

/// A create request carrying `configuration` as its `BackupConfiguration`, or
/// none when it is `Value::Null`.
fn create_request(configuration: &Value) -> Value {
    let mut request = json!({
        "DatastoreTypeVersion": "R4",
        "DatastoreName": "restore.window.store",
    });
    if !configuration.is_null() {
        request["BackupConfiguration"] = configuration.clone();
    }
    request
}

/// The two restore points a reported `DatastoreProperties` carries, each as the
/// epoch seconds the wire renders it on.
fn restore_points(properties: &Value) -> (Option<f64>, Option<f64>) {
    let status = &properties["BackupStatusInfo"];
    (
        status.get("EarliestRestorePoint").and_then(Value::as_f64),
        status.get("LatestRestorePoint").and_then(Value::as_f64),
    )
}

/// Assert the window `properties` reports for a RESTORABLE data store: it starts
/// at the instant backup became enabled (a retention period cannot start the
/// window of a data store younger than itself any later) and ends at the instant
/// the read ran, which `before` and `after` bracket.
fn assert_window(case: &str, properties: &Value, before: f64, after: f64) -> (f64, f64) {
    let (earliest, latest) = restore_points(properties);
    let enabled_at = properties["BackupStatusInfo"]["BackupEnabledAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: backup is enabled, got {properties}"));
    let earliest = earliest.unwrap_or_else(|| {
        panic!("case {case:?}: expected an EarliestRestorePoint, got {properties}")
    });
    let latest = latest.unwrap_or_else(|| {
        panic!("case {case:?}: expected a LatestRestorePoint, got {properties}")
    });
    assert!(
        same_instant(earliest, enabled_at),
        "case {case:?}: expected EarliestRestorePoint to be the instant backup became enabled \
         ({enabled_at}), got {earliest:?}"
    );
    assert!(
        latest >= earliest,
        "case {case:?}: expected LatestRestorePoint at or after the earliest ({earliest}), \
         got {latest:?}"
    );
    assert!(
        latest >= before && latest <= after,
        "case {case:?}: expected LatestRestorePoint inside the instants bracketing the read \
         ({before}, {after}), got {latest:?}"
    );
    (earliest, latest)
}

#[tokio::test]
async fn every_read_reports_the_restore_window_of_a_backup_enabled_datastore() {
    let service = HealthLake::new();

    // One data store per row, all created before the single wait, so the table
    // costs one lifecycle span rather than one per row.
    let mut created = Vec::new();
    for row in rows() {
        let (id, _) = service.create(&create_request(&row.configuration)).await;
        created.push((row, id));
    }
    tokio::time::sleep(PAST_TRANSITION).await;

    let mut described = Vec::new();
    for (row, id) in &created {
        let case = row.case;
        let before = epoch_secs();
        let properties = service.describe(id).await;
        let after = epoch_secs();

        if !row.restorable {
            let (earliest, latest) = restore_points(&properties);
            assert!(
                earliest.is_none() && latest.is_none(),
                "case {case:?}: a data store with no enabled backup cannot be restored, so \
                 expected neither restore point, got {earliest:?} / {latest:?}"
            );
            continue;
        }

        let (earliest, latest) = assert_window(case, &properties, before, after);

        // The window is DERIVED on every read, so a second read of a record
        // nothing changed keeps the same start and never moves the end
        // backwards.
        let again = service.describe(id).await;
        let (earliest_again, latest_again) = assert_window(case, &again, before, epoch_secs());
        assert!(
            same_instant(earliest_again, earliest),
            "case {case:?}: expected a second read to report the same EarliestRestorePoint \
             ({earliest}), got {earliest_again:?}"
        );
        assert!(
            latest_again >= latest,
            "case {case:?}: expected a second read's LatestRestorePoint at or after the first \
             ({latest}), got {latest_again:?}"
        );
        described.push((case, id.clone(), earliest));
    }

    // The window is reported by every read of the properties, not only by
    // `DescribeFHIRDatastore`: `ListFHIRDatastores` reports the SAME shape.
    let before = epoch_secs();
    let listed = service.list_all(&Value::Null).await;
    let after = epoch_secs();
    for (case, id, earliest) in described {
        let entry = listed
            .iter()
            .find(|entry| entry["DatastoreId"] == json!(id))
            .unwrap_or_else(|| panic!("case {case:?}: the listing must hold {id}"));
        let (listed_earliest, _) = assert_window(case, entry, before, after);
        assert!(
            same_instant(listed_earliest, earliest),
            "case {case:?}: expected ListFHIRDatastores to report the EarliestRestorePoint \
             DescribeFHIRDatastore reported ({earliest}), got {listed_earliest:?}"
        );
    }
}

#[tokio::test]
async fn the_reported_window_is_the_one_a_restore_is_held_against() {
    let service = HealthLake::new();
    let (id, _) = service
        .create(&create_request(&json!({
            "Status": "ENABLED",
            "BackupType": "CONTINUOUS",
            "RetentionPeriodInDays": 7,
            "BackupTagsEnabled": false,
        })))
        .await;
    tokio::time::sleep(PAST_TRANSITION).await;

    let before = epoch_secs();
    let properties = service.describe(&id).await;
    let (earliest, latest) = assert_window("the reported window", &properties, before, epoch_secs());

    // A point the read reported as inside the window is a restore point this
    // service accepts: the window a caller READS is the window a restore is held
    // against, which is what makes the documented workflow — enable backup, read
    // the window, restore at a point inside it — drivable.
    for (case, point) in [
        ("a point inside the reported window", (earliest + latest) / 2.0),
        ("the reported latest restore point", latest),
    ] {
        let restored = service
            .ok(
                "RestoreFHIRDatastore",
                &json!({
                    "SourceDatastoreId": &id,
                    "RestoreConfiguration": {
                        "ContinuousBackupRestoreConfiguration": { "RestorePointTime": point },
                    },
                }),
            )
            .await;
        assert!(
            restored["DatastoreId"].is_string(),
            "case {case:?}: restoring at {point} must mint a data store, got {restored}"
        );
    }

    // And a point one second before the reported earliest is outside it — with
    // the refusal naming the SAME start the read reported, so the two cannot
    // drift apart. (Its end is derived at the instant the refusal runs, which is
    // later than the read, so only the start is comparable.)
    let (status, payload) = service
        .call(
            "RestoreFHIRDatastore",
            &json!({
                "SourceDatastoreId": &id,
                "RestoreConfiguration": {
                    "ContinuousBackupRestoreConfiguration": {
                        "RestorePointTime": earliest - 1.0,
                    },
                },
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "a restore point before the reported earliest must be refused, got {status}: {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ValidationException",
        "a restore point before the reported earliest must answer ValidationException, \
         got {payload}"
    );
    let refused = error_message(&payload);
    let named = instants_in(&refused);
    assert!(
        named
            .first()
            .is_some_and(|start| same_instant(*start, earliest)),
        "the refusal must name the window start the read reported ({earliest}), got {named:?} \
         in {refused:?}"
    );
}

/// Every date-time instant a refusal sentence names, in the order it names them,
/// as epoch seconds.
///
/// The sentence renders each one in the date-time form
/// (`aws_smithy_types::date_time::Format::DateTime`), so they are read back the
/// same way rather than by matching on the sentence's wording.
fn instants_in(message: &str) -> Vec<f64> {
    message
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == ':' || c == '-' || c == '.'))
        .filter_map(|word| {
            ::aws_smithy_types::DateTime::from_str(
                word,
                ::aws_smithy_types::date_time::Format::DateTime,
            )
            .ok()
        })
        .map(|instant| instant.as_secs_f64())
        .collect()
}

#[tokio::test]
async fn the_window_appears_and_disappears_with_the_backup_configuration() {
    let service = HealthLake::new();
    let (id, _) = service.create(&create_request(&Value::Null)).await;
    tokio::time::sleep(PAST_TRANSITION).await;

    // Nothing is backed up yet, so there is no window — and no backup status at
    // all, since no configuration has ever been recorded.
    let properties = service.describe(&id).await;
    assert!(
        properties.get("BackupStatusInfo").is_none(),
        "a data store created with no backup configuration reports no backup status, \
         got {properties}"
    );

    // Enabling backup starts the window, and the answer the WRITE gives reports
    // it as well as the later read does.
    let before = epoch_secs();
    let enabled = service
        .ok(
            "UpdateFHIRDatastore",
            &json!({
                "DatastoreId": &id,
                "BackupConfiguration": {
                    "Status": "ENABLED",
                    "BackupType": "CONTINUOUS",
                    "RetentionPeriodInDays": 7,
                    "BackupTagsEnabled": false,
                },
            }),
        )
        .await;
    let (earliest, _) = assert_window(
        "the update that enables backup",
        &enabled["DatastoreProperties"],
        before,
        epoch_secs(),
    );

    tokio::time::sleep(PAST_TRANSITION).await;
    let read_back = service.describe(&id).await;
    let (earliest_read, _) = assert_window(
        "a read after backup was enabled",
        &read_back,
        before,
        epoch_secs(),
    );
    assert!(
        same_instant(earliest_read, earliest),
        "expected the window to keep the start the enabling update reported ({earliest}), \
         got {earliest_read:?}"
    );

    // Disabling it ends the window: there is nothing to restore to any more,
    // though the configuration itself is still reported.
    let disabled = service
        .ok(
            "UpdateFHIRDatastore",
            &json!({
                "DatastoreId": &id,
                "BackupConfiguration": {
                    "Status": "DISABLED",
                    "BackupType": "CONTINUOUS",
                    "RetentionPeriodInDays": 7,
                    "BackupTagsEnabled": false,
                },
            }),
        )
        .await;
    let properties = &disabled["DatastoreProperties"];
    let (earliest, latest) = restore_points(properties);
    assert!(
        earliest.is_none() && latest.is_none(),
        "disabling backup leaves nothing to restore to, so expected neither restore point, \
         got {earliest:?} / {latest:?}"
    );
    assert_eq!(
        properties["BackupStatusInfo"]["Configuration"]["Status"], "DISABLED",
        "the disabled configuration is still reported, got {properties}"
    );
}
