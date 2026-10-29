Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 02c390dd8e861c3d98b16a550da31d4601fa546f

# The restorable window opens at the table's creation even when point-in-time recovery was enabled later, so a span with no continuous backups is reported and restored from

_Filed: 2026-10-29T14:04:28Z_
_Operation: GetTable | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `02c390dd8e861c3d98b16a550da31d4601fa546f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate stores no instant at which point-in-time recovery was turned ON for a table, so `Table::earliest_restorable_timestamp` derives the lower bound of the restorable window from `self.creation_timestamp` alone. Since `CreateTable`'s default PITR status is `DISABLED`, a table created without `pointInTimeRecovery` and later switched on by `UpdateTable` is reported — and treated — as restorable all the way back to its creation, i.e. across a span in which, by this crate's own rule, it had no restorable window at all.

The crate states the rule in two places and then breaks it in a third:

* `Table::earliest_restorable_timestamp` returns `None` "when the table has no restorable window at all, which is every table point-in-time recovery is not `ENABLED` on" — so while PITR is off, NO instant of the table is restorable, and `RestoreTable` refuses every restore of it.
* The same function's doc then asserts "Continuous backups begin when the table is created", which is only true for a table created WITH PITR enabled. For a table created with the modelled default (`DISABLED`) the two statements contradict each other, and the code follows the second one.

Three observable consequences, all from ordinary calls the model allows:

1. `GetTable` reports `pointInTimeRecovery.earliestRestorableTimestamp` equal to `creationTimestamp` for a table whose backups can only have started at the later `UpdateTable` — a restore point that does not exist. The member is documented as "the earliest possible restore point of the table".
2. `RestoreTable` checks `restoreTimestamp` against the same helper (`get_table.rs` says so explicitly: "the one `RestoreTable` checks `restoreTimestamp` against ... so one rule answers both operations"), so a restore aimed inside that phantom span is ACCEPTED with 200 instead of the `ValidationException` the handler raises for a timestamp "outside the restorable window". The restored table is then built out of `superseded`/current settings for an instant no backup covered.
3. Enable -> disable -> enable: during the disabled span `RestoreTable` refuses every restore (the window is `None`), yet after the second enable that same instant is inside the reported window again — one instant, two opposite answers, with nothing in the store recording the gap.

The missing state is visible in the `Table` record: it keeps the PITR setting but no instant for it, and `UpdateTable` writes the setting without recording when it changed (it also does not call `supersede_restorable_settings` for a PITR change, so no revision boundary marks it either).

## Evidence

The rule, stated by the helper itself — `services/service-com-amazonaws-keyspaces-keyspacesservice/src/state.rs:1149-1174`:
```
    /// The earliest instant a restore of this table may target —
    /// `PointInTimeRecoverySummary$earliestRestorableTimestamp` — or `None` when the table
    /// has no restorable window at all, which is every table point-in-time recovery is not
    /// `ENABLED` on and every table the service itself owns.
    ///
    /// Continuous backups begin when the table is created, reach back at most
    /// [`POINT_IN_TIME_RECOVERY_WINDOW`], and can only reproduce settings the store still
    /// holds a revision for ([`MAX_RETAINED_TABLE_REVISIONS`]), so the earliest restorable
    /// instant is the latest of those three.
    pub fn earliest_restorable_timestamp(&self, now: SystemTime) -> Option<SystemTime> {
        if self
            .point_in_time_recovery
            .as_ref()
            .is_none_or(|recovery| recovery.status != POINT_IN_TIME_RECOVERY_ENABLED)
        {
            return None;
        }
        let mut earliest = self.creation_timestamp?;
        if let Some(window_start) = now.checked_sub(POINT_IN_TIME_RECOVERY_WINDOW) {
            earliest = earliest.max(window_start);
        }
        if let Some(known_from) = self.earliest_known_settings {
            earliest = earliest.max(known_from);
        }
        Some(earliest)
    }
```
The lower bound is `creation_timestamp`; nothing in it (or anywhere else) knows when PITR was enabled.

That PITR starts OFF by default — `src/state.rs:198-199`:
```
/// Point-in-time recovery is off unless `CreateTable` asks for it.
pub const DEFAULT_POINT_IN_TIME_RECOVERY_STATUS: &str = "DISABLED";
```
applied by `src/ops/create_table.rs:300-302`:
```
        let point_in_time_recovery = input.point_in_time_recovery.unwrap_or(PointInTimeRecovery {
            status: crate::state::DEFAULT_POINT_IN_TIME_RECOVERY_STATUS.to_string(),
        });
```

The enable that records no instant — `src/ops/update_table.rs:732-734`:
```
        if let Some(point_in_time_recovery) = input.point_in_time_recovery {
            table.point_in_time_recovery = Some(point_in_time_recovery);
        }
```
and the stored field, which has room for the setting only — `src/state.rs:996-998`:
```
    /// The supplied point-in-time-recovery setting, or the modelled default; `None` for a
    /// service-managed table, which no backup setting applies to.
    pub point_in_time_recovery: Option<PointInTimeRecovery>,
```

The report built from it — `src/ops/get_table.rs:296-307`:
```
            point_in_time_recovery: table.point_in_time_recovery.as_ref().map(|recovery| {
                PointInTimeRecoverySummary {
                    status: recovery.status.clone(),
                    // The earliest point a restore can target, which is the one
                    // `RestoreTable` checks `restoreTimestamp` against: see
                    // `crate::state::Table::earliest_restorable_timestamp`, so one rule
                    // answers both operations.
                    earliest_restorable_timestamp: table
                        .earliest_restorable_timestamp(now)
                        .map(::aws_smithy_types::DateTime::from),
                }
            }),
```
against the modelled member (`smithy_query(query="shape",shape="PointInTimeRecoverySummary")`): `earliestRestorableTimestamp` — "Specifies the earliest possible restore point of the table in ISO 8601 format."

The same helper gating `RestoreTable` — `src/ops/restore_table.rs:370-393`:
```
        // "Restores the table to the specified point in time within the
        // earliest_restorable_timestamp and the current time." The window is the one
        // `GetTable` reports for the source table, so a table point-in-time recovery is not
        // enabled on has no window and no restorable point at all.
        let Some(earliest) = source.earliest_restorable_timestamp(now) else {
            return Err(invalid(format!(
                "table '{}.{}' has no restorable point in time: point-in-time recovery is \
                 not {} on it",
...
        if restore_point < earliest || restore_point > now {
            return Err(invalid(format!(
                "restoreTimestamp is outside the restorable window of table '{}.{}', which \
                 runs from its earliestRestorableTimestamp ({} seconds since the epoch) to \
                 the current time ({} seconds since the epoch)",
```

The crate's own tests state the rule the code breaks. `tests/all/update_table.rs:277-285` — the row for exactly this transition says the backups start at the ENABLE, and asserts only the status, never the timestamp, so it passes today:
```
        Case {
            // DISABLED -> ENABLED: continuous backups start now, so the table gains an
            // earliest restorable point.
            name: "point_in_time_recovery_enabled",
            create: || json!({}),
            update: || json!({"pointInTimeRecovery": {"status": "ENABLED"}}),
            touched: &["pointInTimeRecovery"],
            expect: || vec![("/pointInTimeRecovery/status", json!("ENABLED"))],
```
and `tests/all/restore_table.rs:788-795` states the no-PITR half:
```
        Row {
            // No point-in-time recovery means no restorable window, so there is no
            // instant this table can be restored to.
            name: "source_without_point_in_time_recovery",
            request: restore_request("no_backups", "refused_4", json!({})),
            status: 400,
            code: "ValidationException",
        },
```

Test coverage that constrains a fix: `tests/all/restore_table.rs:588-595` pins the window opening at creation for a table created WITH PITR enabled, which stays correct under the fix:
```
    // The window the restores below target opens at the source table's creation, which is
    // what `GetTable` reports for a table with point-in-time recovery enabled.
    let source = get_table(&emulator, SOURCE_KEYSPACE, source_table).await;
    assert_eq!(
        at(&source, "/pointInTimeRecovery/earliestRestorableTimestamp"),
        at(&source, "/creationTimestamp"),
```
`tests/all/update_table.rs:287-303` pins that a DISABLE drops `earliestRestorableTimestamp` to null — also unchanged. No test pins the timestamp after a later ENABLE, so nothing else constrains the fix.

## Steps to Reproduce

1. Start the Keyspaces emulator
2. CreateKeyspace {"keyspaceName":"app"} and wait out the transient CREATING span
3. CreateTable {"keyspaceName":"app","tableName":"t","schemaDefinition":{"allColumns":[{"name":"id","type":"text"}],"partitionKeys":[{"name":"id"}]}} — no pointInTimeRecovery, so the table is stored with the modelled default DISABLED. Note its creationTimestamp from GetTable
4. GetTable {"keyspaceName":"app","tableName":"t"} — pointInTimeRecovery.status is DISABLED and earliestRestorableTimestamp is absent; a RestoreTable of it now is refused 400 ValidationException ("has no restorable point in time")
5. Wait a few seconds, then UpdateTable {"keyspaceName":"app","tableName":"t","pointInTimeRecovery":{"status":"ENABLED"}} and wait out the UPDATING span
6. GetTable again — actual: earliestRestorableTimestamp equals the table's creationTimestamp, i.e. seconds before continuous backups could have started; expected: the instant the UpdateTable turned PITR on
7. RestoreTable {"sourceKeyspaceName":"app","sourceTableName":"t","targetKeyspaceName":"app","targetTableName":"t_restored","restoreTimestamp":<creationTimestamp + 1 second>} — actual: 200 with restoredTableARN, a table restored to an instant the service held no backup for; expected: the 400 ValidationException the handler gives for a restoreTimestamp outside the restorable window
8. Third shape: with PITR ENABLED, UpdateTable it to DISABLED, observe that every RestoreTable is now refused, then UpdateTable it back to ENABLED — the instants inside the disabled span are reported as restorable again and accepted by RestoreTable
