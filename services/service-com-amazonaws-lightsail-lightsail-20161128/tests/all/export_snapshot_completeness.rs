// SPDX-License-Identifier: CC0-1.0

//! Exporting a Lightsail snapshot to Amazon EC2 whose capture is not finished
//! yet — `ExportSnapshot` — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a snapshot holds a complete copy of what it captured
//! only once the service has finished taking it, so an operation that CONSUMES a
//! snapshot — and an export to Amazon EC2 is one, since there is nothing
//! complete to turn into an Amazon EBS volume or an Amazon Machine Image until
//! then — refuses a source snapshot that has not reached its settled state with
//! the modelled `OperationFailureException`, naming the snapshot and the state
//! it is in, and stores nothing; the very same request is served once the clock
//! settles the snapshot. The sibling consumers — `CopySnapshot`,
//! `CreateDiskFromSnapshot` and `CreateInstancesFromSnapshot` — hold their own
//! sources to the same rule, pinned in `snapshot_copy.rs`,
//! `disk_restore_snapshot_completeness.rs` and
//! `instance_restore_from_snapshot.rs`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The harness and the helpers this test target already exposes: a sibling suite
// drives the service through the same front door, stands a source resource up,
// names the two kinds of snapshot, waits one into its settled state and checks a
// destination id the same way rather than writing any of it a second time.
use crate::disk_attachment::{create_available_disk, wait_for_instance_state};
use crate::export_snapshot_records::is_ec2_resource_id;
use crate::instance_lifecycle::{error_code, Emulator};
use crate::snapshot_copy::{snapshot, wait_for_snapshot_state, Kind};

/// The id prefix of the Amazon EC2 resource each kind of export creates:
/// "Exported instance snapshots appear in Amazon EC2 as Amazon Machine Images
/// (AMIs) ... Exported disk snapshots appear in Amazon EC2 as Amazon EBS
/// volumes" (`smithy_query(query="doc",shape="ExportSnapshot")`).
const VOLUME_ID_PREFIX: &str = "vol-";
/// See [`VOLUME_ID_PREFIX`].
const IMAGE_ID_PREFIX: &str = "ami-";

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// What one row's snapshot captures, which is also the KIND of snapshot it is.
#[derive(Debug)]
enum Snapshot {
    /// A disk snapshot of a block storage disk of this size in GB
    /// (`CreateDiskSnapshot` with `diskName`).
    Disk { size_in_gb: i64 },
    /// A disk snapshot of an instance's system volume (`CreateDiskSnapshot` with
    /// `instanceName`), taken of an instance created from this blueprint and
    /// bundle.
    SystemVolume {
        blueprint_id: &'static str,
        bundle_id: &'static str,
    },
    /// An instance snapshot (`CreateInstanceSnapshot`), of an instance created
    /// from this blueprint and bundle with these block storage disks attached at
    /// these paths.
    Instance {
        blueprint_id: &'static str,
        bundle_id: &'static str,
        disks: &'static [(&'static str, &'static str, i64)],
    },
}

/// One export the service must refuse while the snapshot is still being taken,
/// and serve once it is taken.
///
/// The rows differ along everything beside the snapshot's state that the export
/// turns on: the KIND of snapshot exported (a disk snapshot, which becomes an
/// Amazon EBS volume, and an instance snapshot, which becomes an Amazon Machine
/// Image) and so the state name its capture settles into, what the snapshot was
/// taken of (a block storage disk, an instance's system volume, a whole
/// instance), the snapshotted volume's size — including the smallest disk
/// Lightsail creates and a large one — the number of disks the snapshot
/// captured, the Region and zone the snapshot lives in, and the form of both
/// names.
#[derive(Debug)]
struct Row {
    case: &'static str,
    source_name: &'static str,
    snapshot_name: &'static str,
    zone: &'static str,
    snapshot: Snapshot,
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "a snapshot of the smallest block storage disk Lightsail creates",
        source_name: "small-source-disk",
        snapshot_name: "small-source-disk-snapshot",
        zone: "us-east-1a",
        snapshot: Snapshot::Disk { size_in_gb: 8 },
    },
    Row {
        case: "a snapshot of a large block storage disk, in another Region",
        source_name: "big_source_disk_2048",
        snapshot_name: "big_source_disk_backup",
        zone: "eu-west-1c",
        snapshot: Snapshot::Disk { size_in_gb: 2048 },
    },
    Row {
        case: "a disk snapshot of an instance's system volume",
        source_name: "system-volume-source",
        snapshot_name: "system-volume-source-snapshot",
        zone: "ap-northeast-1a",
        snapshot: Snapshot::SystemVolume {
            blueprint_id: "amazon_linux_2023",
            bundle_id: "nano_3_0",
        },
    },
    Row {
        case: "an instance snapshot with no attached disk",
        source_name: "plain-instance",
        snapshot_name: "plain-instance-snapshot",
        zone: "us-west-2a",
        snapshot: Snapshot::Instance {
            blueprint_id: "debian_12",
            bundle_id: "micro_3_0",
            disks: &[],
        },
    },
    Row {
        case: "an instance snapshot with two attached disks, in another Region",
        source_name: "attached_instance_2",
        snapshot_name: "attached_instance_2_snap",
        zone: "ca-central-1a",
        snapshot: Snapshot::Instance {
            blueprint_id: "amazon_linux_2023",
            bundle_id: "medium_3_0",
            disks: &[("export-data-1", "/dev/xvdf", 16), ("export_data_2", "/dev/xvdg", 64)],
        },
    },
];

/// The KIND of snapshot `row` exports, which is the shared vocabulary for its
/// reader, the state it settles into and the `ResourceType` it reports
/// ([`Kind`]).
fn kind(row: &Row) -> Kind {
    match row.snapshot {
        Snapshot::Instance { .. } => Kind::InstanceSnapshot,
        Snapshot::Disk { .. } | Snapshot::SystemVolume { .. } => Kind::DiskSnapshot,
    }
}

/// The id prefix of the Amazon EC2 resource `row`'s export creates.
fn destination_prefix(row: &Row) -> &'static str {
    match row.snapshot {
        Snapshot::Instance { .. } => IMAGE_ID_PREFIX,
        Snapshot::Disk { .. } | Snapshot::SystemVolume { .. } => VOLUME_ID_PREFIX,
    }
}

/// Create `row`'s source resource and start the snapshot of it this suite
/// exports, WITHOUT waiting for the capture the clock owes.
async fn start_snapshot(emulator: &Emulator, row: &Row) {
    match &row.snapshot {
        Snapshot::Disk { size_in_gb } => {
            create_available_disk(emulator, row.source_name, row.zone, *size_in_gb).await;
            emulator
                .ok(
                    "CreateDiskSnapshot",
                    json!({
                        "diskName": row.source_name,
                        "diskSnapshotName": row.snapshot_name,
                    }),
                )
                .await;
        }
        Snapshot::SystemVolume {
            blueprint_id,
            bundle_id,
        } => {
            create_instance(emulator, row, blueprint_id, bundle_id, &[]).await;
            emulator
                .ok(
                    "CreateDiskSnapshot",
                    json!({
                        "instanceName": row.source_name,
                        "diskSnapshotName": row.snapshot_name,
                    }),
                )
                .await;
        }
        Snapshot::Instance {
            blueprint_id,
            bundle_id,
            disks,
        } => {
            create_instance(emulator, row, blueprint_id, bundle_id, disks).await;
            emulator
                .ok(
                    "CreateInstanceSnapshot",
                    json!({
                        "instanceName": row.source_name,
                        "instanceSnapshotName": row.snapshot_name,
                    }),
                )
                .await;
        }
    }
}

/// Stand `row`'s source instance up with `disks` attached to it.
async fn create_instance(
    emulator: &Emulator,
    row: &Row,
    blueprint_id: &str,
    bundle_id: &str,
    disks: &[(&str, &str, i64)],
) {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [row.source_name],
                "availabilityZone": row.zone,
                "blueprintId": blueprint_id,
                "bundleId": bundle_id,
            }),
        )
        .await;
    wait_for_instance_state(emulator, row.source_name, "running").await;
    for (disk_name, disk_path, size_in_gb) in disks {
        create_available_disk(emulator, disk_name, row.zone, *size_in_gb).await;
        emulator
            .ok(
                "AttachDisk",
                json!({
                    "diskName": disk_name,
                    "instanceName": row.source_name,
                    "diskPath": disk_path,
                }),
            )
            .await;
    }
}

/// Wait until `row`'s snapshot is one the service has finished taking.
async fn wait_for_settled_snapshot(emulator: &Emulator, row: &Row) -> Value {
    let kind = kind(row);
    wait_for_snapshot_state(emulator, kind, row.snapshot_name, kind.settled()).await
}

/// Every export snapshot record this account holds, walking every page.
async fn all_records(emulator: &Emulator) -> Vec<Value> {
    let mut records: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..100 {
        let request = match &token {
            Some(token) => json!({"pageToken": token}),
            None => json!({}),
        };
        let body = emulator.ok("GetExportSnapshotRecords", request).await;
        let page = body["exportSnapshotRecords"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case listing export snapshot records: expected an array, got {body:?}")
            })
            .clone();
        records.extend(page);
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return records,
        }
    }
    panic!("case listing export snapshot records: the page walk did not terminate")
}

/// The operation types the log of the resource `name` carries.
async fn logged_types(emulator: &Emulator, name: &str) -> Vec<String> {
    let body = emulator
        .ok("GetOperationsForResource", json!({"resourceName": name}))
        .await;
    body["operations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            entry["operationType"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: a snapshot is exported to Amazon EC2 out of the complete copy it holds,
/// so an export whose source snapshot has not reached its settled state is
/// refused with the modelled `OperationFailureException` and appends neither an
/// export snapshot record nor an operation-log entry; once the clock settles the
/// snapshot, the very same request is served and the export describes it.
#[tokio::test]
async fn a_snapshot_still_being_taken_cannot_be_exported() {
    let emulator = Emulator::new();
    let mut records_held = 0usize;

    for row in ROWS {
        let case = row.case;
        start_snapshot(&emulator, row).await;

        // The precondition this case is about, read from the service's own
        // record rather than assumed: the capture has not finished.
        let taking = snapshot(&emulator, kind(row), row.snapshot_name).await;
        let state = taking["state"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            state,
            kind(row).being_taken(),
            "case {case}: expected a snapshot still being taken, got {taking:?}"
        );

        let request = json!({"sourceSnapshotName": row.snapshot_name});
        let (status, answered) = emulator.call("ExportSnapshot", request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 refusal of an export of an unfinished snapshot, got \
             {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "OperationFailureException",
            "case {case}: expected the modelled OperationFailureException, got {answered:?}"
        );
        let message = answered["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(row.snapshot_name)
                && message.contains(&state)
                && message.contains(kind(row).settled()),
            "case {case}: expected the refusal to name the snapshot, the state it is in and the \
             state it has to reach, got {answered:?}"
        );

        // A refused export stores nothing: no record of its own, and nothing in
        // the snapshot's own operation log.
        let records = all_records(&emulator).await;
        assert_eq!(
            records.len(),
            records_held,
            "case {case}: expected a refused export to append no record, got {records:?}"
        );
        let types = logged_types(&emulator, row.snapshot_name).await;
        assert!(
            !types.iter().any(|kind| kind == "ExportSnapshot"),
            "case {case}: expected a refused export to log nothing against the snapshot, got \
             {types:?}"
        );

        // Only the snapshot's state stood in the way: the clock settles it, and
        // then the very same request is served.
        let settled = wait_for_settled_snapshot(&emulator, row).await;
        assert_eq!(
            settled["arn"], taking["arn"],
            "case {case}: a refused export leaves the snapshot as it was, got {settled:?}"
        );
        let served = emulator.ok("ExportSnapshot", request).await;
        let operations = served["operations"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            operations.len(),
            1,
            "case {case}: expected the one operation an export answers, got {served:?}"
        );
        assert_eq!(
            operations[0]["operationType"].as_str(),
            Some("ExportSnapshot"),
            "case {case}: expected the ExportSnapshot operation type, got {:?}",
            operations[0]
        );

        records_held += 1;
        let records = all_records(&emulator).await;
        assert_eq!(
            records.len(),
            records_held,
            "case {case}: expected the one record this export appended, got {records:?}"
        );
        let record = records
            .iter()
            .find(|record| record["sourceInfo"]["name"].as_str() == Some(row.snapshot_name))
            .unwrap_or_else(|| {
                panic!("case {case}: expected a record describing the exported snapshot")
            })
            .clone();
        assert_eq!(
            record["sourceInfo"]["resourceType"].as_str(),
            Some(kind(row).resource_type()),
            "case {case}: expected the exported snapshot's own source type, got {record:?}"
        );
        assert_eq!(
            record["sourceInfo"]["arn"], settled["arn"],
            "case {case}: expected the exported snapshot's own ARN, got {record:?}"
        );
    }

    // Every export of the table settles into a destination resource of the kind
    // the exported snapshot becomes in Amazon EC2.
    for row in ROWS {
        let case = row.case;
        let record = wait_for_succeeded_record(&emulator, row).await;
        let destination_id = record["destinationInfo"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_ec2_resource_id(&destination_id, destination_prefix(row)),
            "case {case}: expected a {} destination resource, got {destination_id:?}",
            destination_prefix(row)
        );
    }
}

/// Poll `GetExportSnapshotRecords` until the record of `row`'s export has
/// succeeded, answering it.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_succeeded_record(emulator: &Emulator, row: &Row) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let records = all_records(emulator).await;
        let record = records
            .iter()
            .find(|record| record["sourceInfo"]["name"].as_str() == Some(row.snapshot_name))
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {}: expected the record of this export to be reported",
                    row.case
                )
            });
        if record["state"].as_str() == Some("Succeeded") {
            return record;
        }
        assert!(
            Instant::now() < deadline,
            "case {}: expected the export to succeed within 5s, got {:?}",
            row.case,
            record["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// RULE: the refusal is about the snapshot's state and nothing else — a name
/// that is no snapshot of this account is still the `DoesNotExist` refusal, and
/// a settled snapshot of either kind still exports, however many times.
#[tokio::test]
async fn only_the_unfinished_capture_is_refused() {
    let emulator = Emulator::new();
    let row = &ROWS[0];
    start_snapshot(&emulator, row).await;

    // A name that is no snapshot at all is refused for not existing, not for
    // the state of a snapshot: the two refusals are distinct.
    let (status, answered) = emulator
        .call(
            "ExportSnapshot",
            json!({"sourceSnapshotName": "no-such-snapshot"}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case a name that is no snapshot: expected a 400, got {status} {answered:?}"
    );
    assert_eq!(
        error_code(&answered),
        "NotFoundException",
        "case a name that is no snapshot: expected the NotFoundException wire error, got \
         {answered:?}"
    );

    // The DISK the snapshot was taken of is a resource this account holds and
    // is still no snapshot, so it earns the same `DoesNotExist` refusal.
    let (status, answered) = emulator
        .call(
            "ExportSnapshot",
            json!({"sourceSnapshotName": row.source_name}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case the disk the snapshot was taken of: expected a 400, got {status} {answered:?}"
    );
    assert_eq!(
        answered["code"].as_str(),
        Some("DoesNotExist"),
        "case the disk the snapshot was taken of: expected the DoesNotExist body code, got \
         {answered:?}"
    );

    // Once the capture is finished the snapshot exports as often as it is
    // asked for, each export a record of its own.
    wait_for_settled_snapshot(&emulator, row).await;
    for export in 1..=3usize {
        emulator
            .ok(
                "ExportSnapshot",
                json!({"sourceSnapshotName": row.snapshot_name}),
            )
            .await;
        let records = all_records(&emulator).await;
        assert_eq!(
            records.len(),
            export,
            "case export {export} of a settled snapshot: expected one record per export, got \
             {records:?}"
        );
    }
}
