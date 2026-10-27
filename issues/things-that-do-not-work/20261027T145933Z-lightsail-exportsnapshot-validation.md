Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 83df1cdc32a5bdf510e34eb14380f1c3f1e573c9

# ExportSnapshot exports a snapshot the service has not finished taking, skipping the completeness check every other snapshot consumer in the crate applies

_Filed: 2026-10-27T14:59:33Z_
_Operation: ExportSnapshot | Class: validation_

> _Note: this issue was identified against the code base at commit `83df1cdc32a5bdf510e34eb14380f1c3f1e573c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ExportSnapshot` resolves the named snapshot, reads everything the export record reports from it and appends the record — without ever looking at the snapshot's `state`. Every other operation in this crate that CONSUMES a snapshot refuses one whose capture is unfinished, with the reason spelled out in `copy_snapshot.rs`: "A snapshot can be copied only once every block it captures is captured ... there is no complete copy to copy from yet." `CopySnapshot` (both kinds), `CreateDiskFromSnapshot` and `CreateInstancesFromSnapshot` each enforce it; `ExportSnapshot` is the one consumer that does not.

The consequence: a `CreateDiskSnapshot` immediately followed by `ExportSnapshot` answers 200 and stores an export snapshot record whose `sourceInfo` describes a `pending` snapshot and whose `destinationInfo` names an EC2 volume/image id minted from a capture that does not exist yet — and that record settles to `Succeeded`, after which `CreateCloudFormationStack` will build an instance from it. The source snapshot's own state is read nowhere in the handler (`export_source` reads `size_in_gb`, `source`, `location`, the ARNs — never `state`).

## Evidence

The handler, services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/export_snapshot.rs:187-237 — the only guards are the store's capacity and the name resolving:
```
        crate::state::with_state(&*self.engine, |store| {
            if store.export_snapshot_records.len() >= crate::state::MAX_EXPORT_SNAPSHOT_RECORDS {
...
            let exported = export_source(store, &source_snapshot_name)
                .ok_or_else(|| not_found(&source_snapshot_name))?;
```
and `export_source` (src/ops/export_snapshot.rs:279-334) reads `snapshot.size_in_gb`, `snapshot.source`, `snapshot.location`, `snapshot.arn` and the from-* members — there is no reference to `snapshot.state` anywhere in the file.

The rule every sibling applies — src/ops/copy_snapshot.rs:490-503:
```
        // A snapshot can be copied only once every block it captures is
        // captured: while Lightsail is still taking it the snapshot reports
        // `pending` (`DiskSnapshot$state`), and there is no complete copy to
        // copy from yet. That is a well-formed request the resources' state does
        // not allow, which is what the modelled `OperationFailureException` is
        // for.
        if snapshot.state != crate::state::DISK_SNAPSHOT_STATE_COMPLETED {
            return Err(operation_failure(format!(
                "The disk snapshot {} is {}: a snapshot can be copied only when it is {}.",
```
the same check for an instance snapshot at src/ops/copy_snapshot.rs:519-526, for a restore at src/ops/create_disk_from_snapshot.rs:326-331 and src/ops/create_instances_from_snapshot.rs:347-353.

The crate states the rule as a class rule in its tests — tests/all/disk_restore_snapshot_completeness.rs:12-20:
```
//! The rule under test: a snapshot holds a complete copy of the volume it
//! captured only once the service has finished taking it, so a restore naming a
//! source snapshot that has not reached its completed state is refused with the
//! modelled `OperationFailureException` and stores nothing, whatever the rest of
//! the request asks for; the very same request is served once the clock settles
//! the snapshot. The sibling restores — `CreateInstancesFromSnapshot` and
//! `CreateRelationalDatabaseFromSnapshot` — hold their own sources to the same
//! rule, ...
```
The export suite currently exercises the opposite, with no stated reason: tests/all/export_snapshot_records.rs:297-344 calls `CreateDiskSnapshot` and then `ExportSnapshot` with no wait, so every row exports a snapshot that is still `pending` (a snapshot settles only after `STATE_TRANSITION_MILLIS = 500`, src/state.rs:352). A fix has to make that suite settle its snapshot first, as disk_restore_snapshot_completeness.rs does.

## Steps to Reproduce

1. Call CreateDisk with diskName="d1", sizeInGb=32, availabilityZone="us-east-1a".
2. Call CreateDiskSnapshot with diskName="d1", diskSnapshotName="s1" and immediately call GetDiskSnapshot — state is "pending".
3. Within the same 500 ms, call ExportSnapshot with sourceSnapshotName="s1".
4. Observe HTTP 200 and an appended export snapshot record (GetExportSnapshotRecords) whose sourceInfo describes "s1" and whose destinationInfo names a vol-/ami- id, although the snapshot's capture was unfinished.
5. Compare with CopySnapshot (sourceSnapshotName="s1", sourceRegion="us-east-1", targetSnapshotName="s2") in the same window, which is refused with OperationFailureException "The disk snapshot s1 is pending: a snapshot can be copied only when it is completed."
