Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 50acf5c50341569d447ddf04c6c32ab03f586e64

# The three hand-written copies of the restoreDate/useLatestRestorableAutoSnapshot rule disagree: two refuse a malformed date beside a MANUAL snapshot for the date's form, while the sibling that states the rule refuses it for the member combination

_Filed: 2026-10-29T03:54:12Z_
_Operation: CopySnapshot | Class: error-response_

> _Note: this issue was identified against the code base at commit `50acf5c50341569d447ddf04c6c32ab03f586e64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The same documented rule — `restoreDate` and `useLatestRestorableAutoSnapshot` "are mutually exclusive" and are to be defined "only when copying/creating … from an automatic snapshot", which the second source member names — is implemented THREE separate times in this crate, once per door, as a local `requested_source` function with a local `RequestedSource` enum: `CopySnapshot`, `CreateDiskFromSnapshot` and `CreateInstancesFromSnapshot`. The three copies have already drifted on the order in which the rule's two halves are judged, so one request shape earns a different refusal depending on which door it is sent to.

`CreateInstancesFromSnapshot` judges WHICH source the request names first and only then the date's form, and says in a comment why that order is the rule: "WHICH source the request names is judged first, so a request carrying the automatic-snapshot members with a MANUAL snapshot is told that, rather than being refused for the form of a date it should not have sent at all."

`CopySnapshot` and `CreateDiskFromSnapshot` do the opposite: both check `is_auto_snapshot_date` on `restoreDate` BEFORE looking at which source the request named. So a request that names a manual snapshot together with a malformed `restoreDate` — a date it should not have sent at all — is told its date is badly formatted instead of being told the member does not belong with a manual source, exactly the outcome the sibling's comment calls out as the wrong one. The same inversion also decides which of the two refusals a malformed date sent together with `useLatestRestorableAutoSnapshot=true` earns (the format one in `CopySnapshot`/`CreateDiskFromSnapshot`, the mutual-exclusion one in `CreateInstancesFromSnapshot`).

The crate's own pattern for a request rule shared by several operations is a single owner in `state.rs` — `crate::state::inverted_window_refusal` is "the one sentence this service answers that pair with wherever it takes one", and eight handlers call it rather than re-deriving it. This rule has no such owner, which is what let the copies diverge; the three copies are otherwise branch-for-branch and message-for-message identical apart from the member names, so one owner (parameterised by the member names and the operation's verb) can answer all three doors with each handler's present wording.

## Evidence

Copy 1, date form judged FIRST — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/copy_snapshot.rs:360-389:
```
fn requested_source(input: &CopySnapshotInput) -> Result<RequestedSource, CopySnapshotError> {
    let wants_latest = input.use_latest_restorable_auto_snapshot == Some(true);
    if let Some(restore_date) = &input.restore_date {
        if !crate::state::is_auto_snapshot_date(restore_date) {
            return Err(invalid(format!(
                "restoreDate '{restore_date}' must be specified in YYYY-MM-DD format"
            )));
        }
        if wants_latest {
            return Err(invalid(
                "restoreDate and useLatestRestorableAutoSnapshot are mutually exclusive: \
                 name the date of the automatic snapshot to copy, or ask for the latest \
                 restorable one, but not both"
                    .to_string(),
            ));
        }
    }
    match (&input.source_snapshot_name, &input.source_resource_name) {
        (Some(snapshot_name), None) => {
            if input.restore_date.is_some() || wants_latest {
                return Err(invalid(
                    "restoreDate and useLatestRestorableAutoSnapshot are to be defined only \
                     when copying an automatic snapshot as a manual snapshot, which \
                     sourceResourceName names the source of, not together with \
                     sourceSnapshotName"
                        .to_string(),
                ));
            }
```

Copy 2, the identical body one member-name set over — src/ops/create_disk_from_snapshot.rs:408-437:
```
fn requested_source(
    input: &CreateDiskFromSnapshotInput,
) -> Result<RequestedSource, CreateDiskFromSnapshotError> {
    let wants_latest = input.use_latest_restorable_auto_snapshot == Some(true);
    if let Some(restore_date) = &input.restore_date {
        if !crate::state::is_auto_snapshot_date(restore_date) {
            return Err(invalid(format!(
                "restoreDate '{restore_date}' must be specified in YYYY-MM-DD format"
            )));
        }
        if wants_latest {
            return Err(invalid(
                "restoreDate and useLatestRestorableAutoSnapshot are mutually exclusive: \
                 name the date of the automatic snapshot to restore, or ask for the latest \
                 restorable one, but not both"
                    .to_string(),
            ));
        }
    }
    match (&input.disk_snapshot_name, &input.source_disk_name) {
        (Some(snapshot_name), None) => {
            if input.restore_date.is_some() || wants_latest {
                return Err(invalid(
                    "restoreDate and useLatestRestorableAutoSnapshot are to be defined only \
                     when creating a new disk from an automatic snapshot, which \
                     sourceDiskName names the source of, not together with \
                     diskSnapshotName"
                        .to_string(),
                ));
            }
```

Copy 3, the drifted one, which STATES the ordering rule the other two break — src/ops/create_instances_from_snapshot.rs:698-729:
```
fn requested_source(
    input: &CreateInstancesFromSnapshotInput,
) -> Result<RequestedSource, CreateInstancesFromSnapshotError> {
    let wants_latest = input.use_latest_restorable_auto_snapshot == Some(true);
    // WHICH source the request names is judged first, so a request carrying the
    // automatic-snapshot members with a MANUAL snapshot is told that, rather
    // than being refused for the form of a date it should not have sent at all.
    match (&input.instance_snapshot_name, &input.source_instance_name) {
        (Some(snapshot_name), None) => {
            if input.restore_date.is_some() || wants_latest {
                return Err(invalid(
                    "restoreDate and useLatestRestorableAutoSnapshot are to be defined only \
                     when creating a new instance from an automatic snapshot, which \
                     sourceInstanceName names the source of, not together with \
                     instanceSnapshotName"
                        .to_string(),
                ));
            }
            Ok(RequestedSource::Snapshot(snapshot_name.clone()))
        }
        (None, Some(instance_name)) => match (&input.restore_date, wants_latest) {
            (Some(restore_date), false) => {
                if !crate::state::is_auto_snapshot_date(restore_date) {
                    return Err(invalid(format!(
                        "restoreDate '{restore_date}' must be specified in YYYY-MM-DD format"
                    )));
                }
```

The crate's own pattern for a rule several doors share, for contrast — src/state.rs:16321-16332:
```
/// The refusal a time window whose end does not follow its start earns, and
/// `None` while the pair is a window — the one sentence this service answers
/// that pair with wherever it takes one.
...
pub fn inverted_window_refusal(
```
(called from eight handlers: get_instance_metric_data.rs:226, get_bucket_metric_data.rs:281, get_distribution_metric_data.rs:273, get_container_service_metric_data.rs:215, get_load_balancer_metric_data.rs:230, get_relational_database_metric_data.rs:277, get_container_log.rs:216, get_relational_database_log_events.rs:203.)

Test coverage — nothing pins the diverging precedence, so a consolidation that keeps each door's present message passes every existing test:
* tests/all/snapshot_copy.rs:879-935 asserts the error TYPE only (`"InvalidInputException"`), and its two relevant rows are "a restore date beside a manual source" with a WELL-FORMED date (`"restoreDate": "2026-10-24"`, line 902) and "a restore date that is not a YYYY-MM-DD date" with an AUTOMATIC source (`"sourceResourceName": "refusal-disk"`, line 931) — never a malformed date beside a manual source.
* tests/all/disk_restore_from_snapshot.rs:747-794 (`the_automatic_snapshot_members_must_hold_together`) asserts only that the message mentions a member name, and its "a restore date together with a manual snapshot" row also uses a well-formed date (`"restoreDate": "2026-10-22"`, line 756).
* tests/all/instance_restore_from_snapshot.rs:886 states the same rule for the third door.

## Steps to Reproduce

1. Create an instance and an instance snapshot of it (CreateInstances, then CreateInstanceSnapshot with instanceSnapshotName="snap"), and a disk plus a disk snapshot "dsnap", all in us-east-1.
2. Call CopySnapshot with sourceRegion="us-east-1", sourceSnapshotName="dsnap", targetSnapshotName="copy" and restoreDate="24-10-2026" (a malformed date, sent beside a MANUAL source). Observe the refusal message: "restoreDate '24-10-2026' must be specified in YYYY-MM-DD format" (src/ops/copy_snapshot.rs:362-367).
3. Call CreateInstancesFromSnapshot with instanceNames=["i1"], bundleId, availabilityZone, instanceSnapshotName="snap" and the same restoreDate="24-10-2026". Observe the DIFFERENT refusal: "restoreDate and useLatestRestorableAutoSnapshot are to be defined only when creating a new instance from an automatic snapshot, which sourceInstanceName names the source of, not together with instanceSnapshotName" (src/ops/create_instances_from_snapshot.rs:707-714).
4. Call CreateDiskFromSnapshot with diskName="d2", availabilityZone="us-east-1a", sizeInGb=32, diskSnapshotName="dsnap" and restoreDate="24-10-2026": it answers the date-format refusal like CopySnapshot (src/ops/create_disk_from_snapshot.rs:412-417), not the member-combination one.
5. Read the comment at src/ops/create_instances_from_snapshot.rs:702-704 — "WHICH source the request names is judged first, so a request carrying the automatic-snapshot members with a MANUAL snapshot is told that, rather than being refused for the form of a date it should not have sent at all" — against src/ops/copy_snapshot.rs:361-376 and src/ops/create_disk_from_snapshot.rs:411-426, which judge the date's form first: three hand-written copies of one rule, two of which answer the case the third names as the one to get right.
