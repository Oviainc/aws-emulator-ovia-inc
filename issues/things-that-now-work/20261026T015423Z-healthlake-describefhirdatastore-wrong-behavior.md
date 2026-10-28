Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6a69e82c081230a77b3ef80ecf5b0e8bc7e37fe8

# DescribeFHIRDatastore never reports EarliestRestorePoint/LatestRestorePoint, although the crate already derives that window for RestoreFHIRDatastore

_Filed: 2026-10-26T01:54:23Z_
_Operation: DescribeFHIRDatastore | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6a69e82c081230a77b3ef80ecf5b0e8bc7e37fe8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Datastore::properties()` is the ONE shape `DescribeFHIRDatastore` and `ListFHIRDatastores` both report, and it hard-codes `earliest_restore_point: None` / `latest_restore_point: None` inside `BackupStatusInfo`. So for a data store whose backup is ENABLED — a record that reports `BackupStatusInfo.Configuration` and `BackupEnabledAt` perfectly well — the two modelled restore-point timestamps are unreachable through every read this service has.

The window is not unknown to the crate: it is computed from the SAME record by `Datastore::restore_window(now)`, and `RestoreFHIRDatastore` both validates a requested `RestorePointTime` against it and renders BOTH instants into its refusal sentence. The result is an internal inconsistency: one operation in this crate tells the caller the earliest and latest restore point (but only inside a 400), while the only operations that report backup status refuse to report them at all, so the documented restore workflow (enable backup -> read the window -> restore at a point inside it) cannot be driven from the service's own reads.

The comment that justifies the two `None`s also contradicts the helper it declines to call: it claims the instants would have to be "derived from the clock on every read" and that reporting them would make "two identical reads of one data store differ", while `restore_window`'s own doc states that deriving it per call is exactly the right thing and is side-effect-free. Only `ScheduledPermanentDeletionTime` has a reason to stay unset (this slice retains no backup data of a deleted data store); the other two are computed state that is simply dropped on the floor.

## Evidence

Both restore-point members are hard-coded absent — services/service-com-amazonaws-healthlake-healthlake/src/state.rs:622-641 (inside `Datastore::properties()`, the shape `DescribeFHIRDatastore` returns at src/ops/describe_fhir_datastore.rs:170-172 `Ok(DescribeFHIRDatastoreOutput { datastore_properties: datastore.properties() })`):
```
            // Only the configuration half of the backup status is known here,
            // plus the instant that configuration became enabled. The
            // restore-point instants would each have to be derived from the
            // clock on every read — `RestoreFHIRDatastore` holds a requested
            // restore point against the window
            // [`Datastore::restore_window`] derives at the instant it runs —
            // ... so all three stay unset rather than being invented here and
            // making two identical reads of one data store differ.
            backup_status_info: self
                .backup_configuration
                .clone()
                .map(|configuration| DatastoreBackupStatus {
                    configuration: Some(configuration),
                    backup_enabled_at: self.backup_enabled_at,
                    earliest_restore_point: None,
                    latest_restore_point: None,
                    scheduled_permanent_deletion_time: None,
                }),
```

The other side — the crate DOES hold and derive exactly these two instants, state.rs:682-713:
```
    /// Derived on every call rather than stored: the latest restorable point
    /// advances with the clock, so there is nothing here to keep up to date and
    /// reading it writes nothing.
    pub fn restore_window(&self, now: SystemTime) -> Option<RestoreWindow> {
        let enabled_at = self.backup_enabled_at?;
        ...
        Some(RestoreWindow { earliest, latest })
    }
```
and a sibling handler calls it and states both instants back to the caller — src/ops/restore_fhir_datastore.rs:287 `let Some(window) = source.restore_window(now) else {` and src/ops/restore_fhir_datastore.rs:490-497:
```
fn restore_point_out_of_window_message(window: &crate::state::RestoreWindow) -> String {
    format!(
        "RestorePointTime must be at or after the earliest restore point ({}) and at or before \
         the latest restore point ({})",
        timestamp(&window.earliest),
        timestamp(&window.latest)
    )
}
```

Model text for the two members (`smithy_query(query="shape", shape="DatastoreBackupStatus")`): `EarliestRestorePoint` — "The earliest point in time the data store can be restored to."; `LatestRestorePoint` — "The latest point in time the data store can be restored to."

Test coverage: nothing pins these two members at absent. tests/all/update_fhir_datastore.rs:67-77 enumerates the reported paths and only maps `BackupConfiguration -> ["BackupStatusInfo", "Configuration"]`; tests/all/update_fhir_datastore.rs:289-322 asserts `BackupEnabledAt` absent-then-stamped; tests/all/restore_fhir_datastore.rs:881 reads `BackupStatusInfo.BackupEnabledAt` and :925 asserts only that the out-of-window refusal `.contains("RestorePointTime")`. A fix that fills the two members from `restore_window` leaves every one of those assertions unchanged.

## Steps to Reproduce

1. CreateFHIRDatastore (DatastoreTypeVersion R4) and wait past the 500 ms lifecycle transition so the store is ACTIVE
2. UpdateFHIRDatastore with BackupConfiguration {Status: ENABLED, BackupType: CONTINUOUS, RetentionPeriodInDays: 7}; wait past the transition again
3. DescribeFHIRDatastore for that DatastoreId: DatastoreProperties.BackupStatusInfo carries Configuration and BackupEnabledAt, but no EarliestRestorePoint and no LatestRestorePoint
4. Call RestoreFHIRDatastore on the same source with ContinuousBackupRestoreConfiguration.RestorePointTime set before BackupEnabledAt: the 400 ValidationException Message names both the earliest and the latest restore point — the two instants Describe refused to report
5. Read src/state.rs:632-641 (both members pinned to None) beside src/state.rs:685-713 (`restore_window` derives both) and src/ops/restore_fhir_datastore.rs:490-497 (both rendered into the refusal)
