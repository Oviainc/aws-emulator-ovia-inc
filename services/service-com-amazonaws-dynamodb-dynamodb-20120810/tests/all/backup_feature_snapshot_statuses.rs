// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the STATUSES a backup's recorded source-table feature set
//! reports: `SourceTableFeatureDetails$TimeToLiveDescription$TimeToLiveStatus`, read back
//! through `DescribeBackup` and through the `DeleteBackup` response, beside the backup's
//! own `BackupStatus` as `DescribeBackup` and `ListBackups` report it.
//!
//! The rule pinned here: a status a setting reaches ASYNCHRONOUSLY is never frozen into a
//! stored record in its transient value. A backup records the source table's TTL SETTING,
//! so the status every backup reader projects from it is the one the clock holds when the
//! backup is READ — `ENABLING` only while the change is still owed, and `ENABLED` once it
//! has fallen due — exactly as the table's own `DescribeTimeToLive` reader projects it;
//! and one settled backup status is reported from the single-backup path and the listing
//! path alike.
//!
//! Each rule is asserted over a SET of rows — whether a TTL change had settled when the
//! backup was taken, whether TTL was ever set at all, and the attribute it names — rather
//! than one example, and every row is driven over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same router the
//! emulator's front doors use.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::ops::types::{
    BACKUP_STATUS_VALUES, TIME_TO_LIVE_STATUS_VALUES,
};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION;
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, ok};

/// The one key attribute every table in this suite is created with; no row varies it.
const PARTITION_KEY: &str = "Part";

/// How long a poll for a backup's `CREATING` -> `AVAILABLE` transition may take before
/// this suite calls it a hang: generous slack over the emulator's own transition span,
/// since the point of the bound is that the wait ENDS.
const AVAILABLE_DEADLINE: Duration = Duration::from_secs(10);

/// How long the poll below sleeps between `DescribeBackup` calls.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// One row: what it is called, the TTL attribute to enable (or `None` to leave TTL
/// untouched), whether the TTL change is waited out BEFORE the backup is taken, the TTL
/// status the backup reports while it is still being created, and the status and
/// attribute it reports once everything the clock owed has fallen due.
type Row<'a> = (&'a str, Option<&'a str>, bool, &'a str, &'a str, Value);

/// Wait out one transient span, so whatever the clock owes a table or its TTL setting is
/// due.
async fn settle() {
    tokio::time::sleep(STATUS_TRANSITION + Duration::from_millis(150)).await;
}

/// Create one on-demand table and poll it to `ACTIVE`: a backup is taken from a settled
/// table.
async fn create_active_table(router: &MultiServiceRouter, case: &str, name: &str) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": PARTITION_KEY, "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": PARTITION_KEY, "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(router, case, name).await;
}

/// The `TimeToLiveDescription` a `BackupDescription`-bearing response records for its
/// source table.
fn recorded_ttl(body: &Value) -> Value {
    body["BackupDescription"]["SourceTableFeatureDetails"]["TimeToLiveDescription"].clone()
}

/// The `(status, attribute)` pair one such record states.
fn ttl_pair(body: &Value) -> (Value, Value) {
    let recorded = recorded_ttl(body);
    (
        recorded["TimeToLiveStatus"].clone(),
        recorded["AttributeName"].clone(),
    )
}

/// The `BackupStatus` a `BackupDescription`-bearing response reports.
fn backup_status(body: &Value) -> Value {
    body["BackupDescription"]["BackupDetails"]["BackupStatus"].clone()
}

/// `DescribeBackup` for `arn`.
async fn describe_backup(router: &MultiServiceRouter, arn: &str) -> Value {
    ok(router, "DescribeBackup", json!({"BackupArn": arn})).await
}

/// Poll `DescribeBackup` until the backup reports `AVAILABLE`, answering that
/// description: the loop a client runs before restoring from a backup it just asked for.
///
/// Every poll must answer 200 — a backup an accepted `CreateBackup` catalogued is never
/// missing — and a backup that never settles panics naming the last description it gave
/// rather than hanging.
async fn describe_backup_when_available(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> Value {
    let started = Instant::now();
    let mut last = Value::Null;
    while started.elapsed() < AVAILABLE_DEADLINE {
        let (status, body) = call(router, "DescribeBackup", json!({"BackupArn": arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeBackup must resolve a backup the create accepted, got \
             {status} with {body}"
        );
        if backup_status(&body) == json!("AVAILABLE") {
            return body;
        }
        last = body;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case:?}: the backup never reached AVAILABLE within {AVAILABLE_DEADLINE:?}, last \
         {last}"
    )
}

/// The `BackupStatus` `ListBackups` reports for `arn` among the backups of `table`, or
/// `Null` when the listing holds no summary for it.
async fn listed_status(router: &MultiServiceRouter, table: &str, arn: &str) -> Value {
    let listed = ok(router, "ListBackups", json!({"TableName": table})).await;
    listed["BackupSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|summary| summary["BackupArn"] == json!(arn))
        .map_or(Value::Null, |summary| summary["BackupStatus"].clone())
}

/// The rows this suite spans: each state a source table's TTL setting can be in when a
/// backup is taken from it, inside the one modification interval a table's setting takes
/// one change per (so a `DISABLING` capture is not reachable on a table of its own).
fn rows(longest_attribute: &str) -> [Row<'_>; 4] {
    [
        ("no TTL was ever set", None, false, "DISABLED", "DISABLED", Value::Null),
        (
            "TTL had settled on before the backup was taken",
            Some("expires_at"),
            true,
            "ENABLED",
            "ENABLED",
            json!("expires_at"),
        ),
        (
            "TTL was still enabling when the backup was taken",
            Some("ttl"),
            false,
            "ENABLING",
            "ENABLED",
            json!("ttl"),
        ),
        (
            // The longest attribute name the model admits
            // (`smithy_query(query="shape", shape="TimeToLiveAttributeName")`: len 1..255).
            "TTL was still enabling, over the longest attribute name",
            Some(longest_attribute),
            false,
            "ENABLING",
            "ENABLED",
            json!(longest_attribute),
        ),
    ]
}

#[tokio::test]
async fn a_backup_reports_the_ttl_status_the_clock_holds_when_it_is_read() {
    let router = emulator();
    let longest = "e".repeat(255);

    for (case, attribute, settled_first, creating_status, settled_status, settled_attribute) in
        rows(&longest)
    {
        let name = format!("Snapshot{}", case.replace([' ', ','], ""));
        create_active_table(&router, case, &name).await;
        if let Some(attribute) = attribute {
            ok(
                &router,
                "UpdateTimeToLive",
                json!({
                    "TableName": name,
                    "TimeToLiveSpecification": {"Enabled": true, "AttributeName": attribute},
                }),
            )
            .await;
            if settled_first {
                settle().await;
            }
        }

        let created = ok(
            &router,
            "CreateBackup",
            json!({"TableName": name, "BackupName": format!("{name}Backup")}),
        )
        .await;
        let arn = created["BackupDetails"]["BackupArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !arn.is_empty(),
            "case {case:?}: CreateBackup mints a BackupArn, got {created}"
        );

        // While the backup itself is still being taken, what it records is the status the
        // clock holds right then: a TTL change still in flight is reported in its
        // transient status rather than as settled.
        let creating = describe_backup(&router, &arn).await;
        assert_eq!(
            (backup_status(&creating), ttl_pair(&creating).0),
            (json!("CREATING"), json!(creating_status)),
            "case {case:?}: expected a CREATING backup recording TTL {creating_status}, got \
             {creating}"
        );

        // Once everything the clock owed has fallen due, the recorded setting reports its
        // SETTLED status — the one a client polling for TTL to be on waits for — and the
        // attribute it is in force for.
        let available = describe_backup_when_available(&router, case, &arn).await;
        assert_eq!(
            ttl_pair(&available),
            (json!(settled_status), settled_attribute.clone()),
            "case {case:?}: expected the settled TTL record {settled_status} on \
             {settled_attribute}, got {available}"
        );

        // The listing projects the same settled backup status as the single-backup
        // reader, so a client filtering for restorable backups finds this one.
        let listed = listed_status(&router, &name, &arn).await;
        assert_eq!(
            (listed.clone(), backup_status(&available)),
            (json!("AVAILABLE"), json!("AVAILABLE")),
            "case {case:?}: expected both paths to report AVAILABLE, got {listed} from the \
             listing and {available} from the reader"
        );

        // The delete response is the last record a client keeps of the backup, and it
        // repeats the same settled feature set.
        let deleted = ok(&router, "DeleteBackup", json!({"BackupArn": arn})).await;
        assert_eq!(
            (backup_status(&deleted), ttl_pair(&deleted).0, ttl_pair(&deleted).1),
            (
                json!("DELETED"),
                json!(settled_status),
                settled_attribute.clone()
            ),
            "case {case:?}: expected a DELETED backup still recording TTL {settled_status}, got \
             {deleted}"
        );
    }
}

#[tokio::test]
async fn every_status_a_backup_record_reports_is_one_the_model_declares() {
    let router = emulator();
    create_active_table(&router, "declared statuses", "Declared").await;
    ok(
        &router,
        "UpdateTimeToLive",
        json!({
            "TableName": "Declared",
            "TimeToLiveSpecification": {"Enabled": true, "AttributeName": "ttl"},
        }),
    )
    .await;
    let created = ok(
        &router,
        "CreateBackup",
        json!({"TableName": "Declared", "BackupName": "DeclaredBackup"}),
    )
    .await;
    let arn = created["BackupDetails"]["BackupArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The walk a backup is read through: while it is being taken, once it is available,
    // and in the response that removes it. Each status either reader states must come from
    // the model's own value table rather than a spelling this emulator invented.
    let creating = describe_backup(&router, &arn).await;
    let available = describe_backup_when_available(&router, "declared statuses", &arn).await;
    let deleted = ok(&router, "DeleteBackup", json!({"BackupArn": arn})).await;

    for (case, body) in [
        ("while creating", &creating),
        ("once available", &available),
        ("once deleted", &deleted),
    ] {
        let backup = backup_status(body);
        let backup = backup.as_str().unwrap_or("<no BackupStatus>");
        assert!(
            BACKUP_STATUS_VALUES.contains(&backup),
            "case {case}: expected a declared BackupStatus, got {backup:?} in {body}"
        );
        let (status, _) = ttl_pair(body);
        let status = status.as_str().unwrap_or("<no TimeToLiveStatus>");
        assert!(
            TIME_TO_LIVE_STATUS_VALUES.contains(&status),
            "case {case}: expected a declared TimeToLiveStatus, got {status:?} in {body}"
        );
    }

    // The two readings of the same record differ exactly in the half the clock settled:
    // the transient status is what the record reported while the change was still owed.
    assert_eq!(
        (ttl_pair(&creating).0, ttl_pair(&available).0),
        (json!("ENABLING"), json!("ENABLED")),
        "case the whole walk: expected ENABLING then ENABLED, got {creating} then {available}"
    );
}
