// SPDX-License-Identifier: CC0-1.0

//! Where a table's restorable window OPENS, driven end to end over the service's own
//! `awsJson1_0` wire protocol and believed only once `GetTable` reports it back and
//! `RestoreTable` answers accordingly.
//!
//! The rule under test, exercised over a SET of point-in-time-recovery histories rather
//! than one example: a table's restorable window opens when its continuous backups begin
//! — the instant point-in-time recovery was last turned ON, which is the table's creation
//! only for a table created with it already enabled — and turning the setting off ends
//! the window, so the span it stayed off for is restorable from nothing and a later
//! enable opens a NEW window. A request that re-sets the status the table already holds
//! starts and ends nothing, so it moves the window's opening instant nowhere.
//!
//! `GetTable$pointInTimeRecovery.earliestRestorableTimestamp` ("the earliest possible
//! restore point of the table") and the window `RestoreTable` checks `restoreTimestamp`
//! against are the same window, so every row asserts both: the reported instant, a
//! restore one second before it, and a restore at the instant itself.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use super::keyspace_table_lifecycle::{
    assert_failure, assert_ok, ensure_keyspace, epoch_seconds, minimal_schema, settle_wait,
    table_arn, Emulator,
};

/// How far outside the instants either side of a call the timestamp it stamped may be
/// reported: the wire carries a timestamp as fractional epoch seconds, so the only
/// difference between the stamped instant and the reported one is that encoding.
const TIMESTAMP_TOLERANCE: f64 = 0.01;

/// The keyspace the source tables of every row live in.
const SOURCE_KEYSPACE: &str = "pitr_window_src_ks";

/// The keyspace the restores of every row target.
const TARGET_KEYSPACE: &str = "pitr_window_tgt_ks";

/// The value at `path` (a JSON pointer) in `body`, or `Value::Null` when the body does
/// not carry it.
fn at(body: &Value, path: &str) -> Value {
    body.pointer(path).cloned().unwrap_or(Value::Null)
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

/// Where a row's restorable window must open once its whole history has been applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opens {
    /// At the table's own `creationTimestamp`: the create already carried the setting
    /// `ENABLED`, so the backups are as old as the table.
    AtCreation,
    /// At the update with this index in the row's history — the one that turned the
    /// setting on.
    AtUpdate(usize),
    /// Nowhere: the setting is off, so the table has no restorable point at all.
    Nowhere,
}

/// One row: how the table was created, the point-in-time-recovery statuses the updates
/// after it set in order, and where the window must then open.
struct Row {
    /// The row name, which is also the source table's name.
    name: &'static str,
    /// The `status` the create's `pointInTimeRecovery` carries, or `None` for a create
    /// that names the member at all — which takes the modelled default,
    /// [`keyspaces::DEFAULT_POINT_IN_TIME_RECOVERY_STATUS`].
    created_with: Option<&'static str>,
    /// One `UpdateTable` per entry, each carrying that `pointInTimeRecovery.status`.
    updates: &'static [&'static str],
    /// Where the window opens afterwards.
    opens: Opens,
}

/// The rows: the point-in-time-recovery histories a client can drive a table through with
/// the two statuses `PointInTimeRecoveryStatus` publishes
/// (`keyspaces::POINT_IN_TIME_RECOVERY_ENABLED` and
/// `keyspaces::DEFAULT_POINT_IN_TIME_RECOVERY_STATUS`, the pair listed by
/// `PointInTimeRecovery$status`: "the options are status=ENABLED, status=DISABLED") —
/// spanning both ways of starting off, an enable, a re-set of a status already held, a
/// disable, and an enable after a disable.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // On from the create: the backups are as old as the table.
            name: "created_enabled",
            created_with: Some(keyspaces::POINT_IN_TIME_RECOVERY_ENABLED),
            updates: &[],
            opens: Opens::AtCreation,
        },
        Row {
            // The modelled default leaves it off, so nothing is backed up and nothing is
            // restorable.
            name: "created_without_the_member",
            created_with: None,
            updates: &[],
            opens: Opens::Nowhere,
        },
        Row {
            // Off by default, then switched on: the backups — and the window — start at
            // the update, not at the create.
            name: "default_then_enabled",
            created_with: None,
            updates: &[keyspaces::POINT_IN_TIME_RECOVERY_ENABLED],
            opens: Opens::AtUpdate(0),
        },
        Row {
            // The same history with the create naming the default explicitly.
            name: "disabled_then_enabled",
            created_with: Some(keyspaces::DEFAULT_POINT_IN_TIME_RECOVERY_STATUS),
            updates: &[keyspaces::POINT_IN_TIME_RECOVERY_ENABLED],
            opens: Opens::AtUpdate(0),
        },
        Row {
            // Re-setting ENABLED on a table that already holds it starts no new span of
            // backups, so the window still opens at the create.
            name: "enabled_then_enabled_again",
            created_with: Some(keyspaces::POINT_IN_TIME_RECOVERY_ENABLED),
            updates: &[keyspaces::POINT_IN_TIME_RECOVERY_ENABLED],
            opens: Opens::AtCreation,
        },
        Row {
            // ... and the same re-set after an enable keeps THAT enable's instant.
            name: "enabled_by_update_then_enabled_again",
            created_with: None,
            updates: &[
                keyspaces::POINT_IN_TIME_RECOVERY_ENABLED,
                keyspaces::POINT_IN_TIME_RECOVERY_ENABLED,
            ],
            opens: Opens::AtUpdate(0),
        },
        Row {
            // Turned off: the backups stop, so there is no restorable point.
            name: "enabled_then_disabled",
            created_with: Some(keyspaces::POINT_IN_TIME_RECOVERY_ENABLED),
            updates: &[keyspaces::DEFAULT_POINT_IN_TIME_RECOVERY_STATUS],
            opens: Opens::Nowhere,
        },
        Row {
            // Off while it was off, and the re-enable opens a NEW window: the instants of
            // the disabled span are restorable from nothing, including the ones the first
            // enabled span covered.
            name: "enabled_disabled_enabled",
            created_with: Some(keyspaces::POINT_IN_TIME_RECOVERY_ENABLED),
            updates: &[
                keyspaces::DEFAULT_POINT_IN_TIME_RECOVERY_STATUS,
                keyspaces::POINT_IN_TIME_RECOVERY_ENABLED,
            ],
            opens: Opens::AtUpdate(1),
        },
        Row {
            // Off, on, off again: the last word is what the table holds.
            name: "disabled_enabled_disabled",
            created_with: Some(keyspaces::DEFAULT_POINT_IN_TIME_RECOVERY_STATUS),
            updates: &[
                keyspaces::POINT_IN_TIME_RECOVERY_ENABLED,
                keyspaces::DEFAULT_POINT_IN_TIME_RECOVERY_STATUS,
            ],
            opens: Opens::Nowhere,
        },
    ]
}

/// A `RestoreTable` of `source` into `target`, with `restore_timestamp` when the row
/// names one.
async fn restore(
    emulator: &Emulator,
    source: &str,
    target: &str,
    restore_timestamp: Option<f64>,
) -> (u16, Value) {
    let mut request = json!({
        "sourceKeyspaceName": SOURCE_KEYSPACE,
        "sourceTableName": source,
        "targetKeyspaceName": TARGET_KEYSPACE,
        "targetTableName": target,
    });
    if let Some(timestamp) = restore_timestamp {
        request["restoreTimestamp"] = json!(timestamp);
    }
    emulator.call("RestoreTable", request).await
}

/// The rule: the window opens where the backups began, `GetTable` reports exactly that
/// instant, and `RestoreTable` accepts it while refusing the instant before it.
#[tokio::test]
async fn the_restorable_window_opens_when_continuous_backups_begin() {
    let emulator = Emulator::start();
    ensure_keyspace(&emulator, SOURCE_KEYSPACE).await;
    ensure_keyspace(&emulator, TARGET_KEYSPACE).await;

    for row in rows() {
        // The create, with the row's point-in-time-recovery setting and nothing else
        // optional.
        let mut request = json!({
            "keyspaceName": SOURCE_KEYSPACE,
            "tableName": row.name,
            "schemaDefinition": minimal_schema("pk"),
        });
        if let Some(status) = row.created_with {
            request["pointInTimeRecovery"] = json!({"status": status});
        }
        assert_ok(row.name, emulator.call("CreateTable", request).await);
        // A table is updatable, and restorable, only once its CREATING span has settled.
        tokio::time::sleep(settle_wait()).await;

        // Every update of the row's history, each bracketed by the instants either side
        // of the call: the window may only open inside the bracket of the update that
        // turned the setting on.
        let mut brackets: Vec<(f64, f64)> = Vec::with_capacity(row.updates.len());
        for status in row.updates {
            let before = epoch_seconds(SystemTime::now());
            assert_ok(
                row.name,
                emulator
                    .call(
                        "UpdateTable",
                        json!({
                            "keyspaceName": SOURCE_KEYSPACE,
                            "tableName": row.name,
                            "pointInTimeRecovery": {"status": status},
                        }),
                    )
                    .await,
            );
            let after = epoch_seconds(SystemTime::now());
            brackets.push((before, after));
            // The update is asynchronous, so wait out the UPDATING span before the next
            // call to this table.
            tokio::time::sleep(settle_wait()).await;
        }

        let got = get_table(&emulator, SOURCE_KEYSPACE, row.name).await;
        let expected_status = row
            .updates
            .last()
            .copied()
            .or(row.created_with)
            .unwrap_or(keyspaces::DEFAULT_POINT_IN_TIME_RECOVERY_STATUS);
        assert_eq!(
            at(&got, "/pointInTimeRecovery/status"),
            json!(expected_status),
            "case {:?}: expected status {expected_status:?}, got {}",
            row.name,
            at(&got, "/pointInTimeRecovery/status")
        );
        let created_at = at(&got, "/creationTimestamp")
            .as_f64()
            .unwrap_or_else(|| panic!("case {:?}: GetTable reports creationTimestamp", row.name));
        let reported = at(&got, "/pointInTimeRecovery/earliestRestorableTimestamp");

        let earliest = match row.opens {
            Opens::Nowhere => {
                assert_eq!(
                    reported,
                    Value::Null,
                    "case {:?}: point-in-time recovery is off, so the table has no \
                     earliestRestorableTimestamp, got {reported:?}",
                    row.name
                );
                // RULE, the other half: a table with no window has no restorable instant,
                // so every restore of it is refused.
                assert_failure(
                    row.name,
                    restore(
                        &emulator,
                        row.name,
                        &format!("{}_refused", row.name),
                        None,
                    )
                    .await,
                    400,
                    "ValidationException",
                );
                continue;
            }
            Opens::AtCreation => {
                assert_eq!(
                    reported,
                    json!(created_at),
                    "case {:?}: the create enabled point-in-time recovery, so the window \
                     opens at creationTimestamp {created_at}, got {reported:?}",
                    row.name
                );
                created_at
            }
            Opens::AtUpdate(index) => {
                let (before, after) = brackets[index];
                let earliest = reported.as_f64().unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected an earliestRestorableTimestamp inside \
                         [{before}, {after}], got {reported:?}",
                        row.name
                    )
                });
                assert!(
                    (before - TIMESTAMP_TOLERANCE..=after + TIMESTAMP_TOLERANCE)
                        .contains(&earliest),
                    "case {:?}: the window opens at the update that turned the setting \
                     on, so expected an earliestRestorableTimestamp inside [{before}, \
                     {after}], got {earliest}",
                    row.name
                );
                assert!(
                    earliest > created_at,
                    "case {:?}: the backups started after the table was created, so \
                     expected an earliestRestorableTimestamp later than \
                     creationTimestamp {created_at}, got {earliest}",
                    row.name
                );
                // The span before the backups started is covered by nothing, so the
                // table's own creation instant is NOT a restorable point of it — not even
                // when an earlier enabled span once covered it.
                assert_failure(
                    row.name,
                    restore(
                        &emulator,
                        row.name,
                        &format!("{}_before_backups", row.name),
                        Some(created_at),
                    )
                    .await,
                    400,
                    "ValidationException",
                );
                earliest
            }
        };

        // "Restores the table to the specified point in time within the
        // earliest_restorable_timestamp and the current time": one second before the
        // window opens is outside it, and an instant just inside it is restorable. The
        // boundary is bracketed rather than hit exactly, because a wire timestamp carries
        // fractional seconds and the stored instant carries nanoseconds, so the opening
        // instant itself is not addressable from a client.
        assert_failure(
            row.name,
            restore(
                &emulator,
                row.name,
                &format!("{}_too_early", row.name),
                Some(earliest - 1.0),
            )
            .await,
            400,
            "ValidationException",
        );
        let target = format!("{}_restored", row.name);
        let restored = assert_ok(
            row.name,
            restore(
                &emulator,
                row.name,
                &target,
                Some(earliest + TIMESTAMP_TOLERANCE),
            )
            .await,
        );
        assert_eq!(
            at(&restored, "/restoredTableARN"),
            json!(table_arn(TARGET_KEYSPACE, &target)),
            "case {:?}: a restore just inside the window is accepted and answers with the \
             restored table's ARN, got {restored}",
            row.name
        );
        // The restored table is RESTORING until the clock owes it ACTIVE; wait it out so
        // the next row's restores are not counted against the account's concurrency.
        tokio::time::sleep(settle_wait()).await;
        // Round trip: the restored table reads back, and its OWN backups begin with the
        // restore rather than with the source table's window.
        let restored_table = get_table(&emulator, TARGET_KEYSPACE, &target).await;
        let restored_created_at = at(&restored_table, "/creationTimestamp")
            .as_f64()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: GetTable reports the restored table's creationTimestamp, \
                     got {restored_table}",
                    row.name
                )
            });
        assert_eq!(
            at(&restored_table, "/pointInTimeRecovery/earliestRestorableTimestamp"),
            json!(restored_created_at),
            "case {:?}: the restored table inherits the setting ENABLED, so its own \
             backups begin at its creation {restored_created_at}, got {restored_table}",
            row.name
        );
    }
}
