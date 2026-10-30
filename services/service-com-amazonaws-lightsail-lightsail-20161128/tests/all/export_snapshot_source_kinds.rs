// SPDX-License-Identifier: CC0-1.0

//! What an export of a snapshot to Amazon EC2 says about the snapshot it
//! exported, by KIND of snapshot — `ExportSnapshot`, the record
//! `GetExportSnapshotRecords` reports and the exported snapshot's own operation
//! log — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: an export snapshot record describes its source snapshot
//! as the KIND of snapshot it is, out of the stored snapshot record. An INSTANCE
//! snapshot's record carries the blueprint and the bundle its source instance was
//! created from together with every disk the snapshot captured — the system
//! volume first, then each block storage disk the snapshot holds, each with its
//! own name, device path and size — and the export creates an Amazon Machine
//! Image. A DISK snapshot's record carries the snapshotted volume's size, and the
//! export creates an Amazon Elastic Block Store volume. Either kind names the
//! snapshot and the resource the snapshot was taken of, describes only its own
//! kind, is logged against the SNAPSHOT in that resource's own operation log, and
//! names the resource created at the destination only once the export the clock
//! owes has succeeded.
//!
//! The sibling suite `export_snapshot_records.rs` covers what every export has in
//! common — one record per call, the record's own identity, paging and the
//! refusal of a name that is no snapshot. This one covers what differs with the
//! kind of snapshot exported, over the number of disks a snapshot captured.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The harness and the helpers this test target already exposes: a sibling suite
// drives the service through the same front door, stands a source resource up
// and reads the operation log the same way rather than writing any of it a
// second time.
use crate::disk_attachment::{create_available_disk, wait_for_instance_state};
use crate::export_snapshot_records::is_ec2_resource_id;
use crate::instance_lifecycle::Emulator;
use crate::instance_snapshot_create::{single_operation, wait_for_terminal_operation};
use crate::operations_for_resource::{answered_entries, operation_types};
use crate::snapshot_copy::{wait_for_snapshot_state, Kind};

/// The device path an instance's system volume is exposed at
/// (`crate::state::SYSTEM_DISK_PATH`).
const SYSTEM_DISK_PATH: &str = "/dev/xvda";

/// The id prefix of the Amazon EC2 resource an exported INSTANCE snapshot
/// becomes: "Exported instance snapshots appear in Amazon EC2 as Amazon Machine
/// Images (AMIs)" (`ExportSnapshot`, `smithy_query(query="doc",
/// shape="ExportSnapshot")`).
const IMAGE_ID_PREFIX: &str = "ami-";

/// The id prefix of the Amazon EC2 resource an exported DISK snapshot becomes:
/// "Exported disk snapshots appear in Amazon EC2 as Amazon EBS volumes"
/// (same doc).
const VOLUME_ID_PREFIX: &str = "vol-";

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// The kind of snapshot one row exports — the two the modelled
/// `ExportSnapshotRecordSourceType` enum names.
#[derive(Debug)]
enum Snapshot {
    /// A snapshot of a whole instance, created from this blueprint and bundle,
    /// with these block storage disks (name, device path, size in GB) attached
    /// when the snapshot was taken.
    Instance {
        blueprint_id: &'static str,
        bundle_id: &'static str,
        disks: &'static [(&'static str, &'static str, i64)],
    },
    /// A snapshot of one block storage disk of this size in GB.
    Disk { size_in_gb: i64 },
}

/// One snapshot to export.
///
/// The rows differ along what the rule generalises over: the KIND of snapshot
/// exported, the NUMBER of block storage disks an instance snapshot captured
/// (none, one, three) with their differing sizes and device paths, the
/// snapshotted volume's size — the smallest disk Lightsail creates and a large
/// one — the blueprint and bundle the source instance was created from, the
/// Availability Zone, and the form of the snapshot name.
struct Row {
    case: &'static str,
    zone: &'static str,
    source_name: &'static str,
    snapshot_name: &'static str,
    snapshot: Snapshot,
}

/// The table. Blueprint and bundle ids are entries of this service's own
/// published catalogs (`GetBlueprints` and `GetBundles`).
const ROWS: &[Row] = &[
    Row {
        case: "an instance snapshot of an instance with no attached disk",
        zone: "us-east-1a",
        source_name: "al2023-plain",
        snapshot_name: "al2023-plain-snapshot",
        snapshot: Snapshot::Instance {
            blueprint_id: "amazon_linux_2023",
            bundle_id: "micro_3_0",
            disks: &[],
        },
    },
    Row {
        case: "an instance snapshot of an instance with one attached disk",
        zone: "eu-west-1c",
        source_name: "ubuntu_one_disk",
        snapshot_name: "ubuntu_one_disk_snap",
        snapshot: Snapshot::Instance {
            blueprint_id: "ubuntu_22_04",
            bundle_id: "nano_3_0",
            disks: &[("ubuntu-data", "/dev/xvdf", 64)],
        },
    },
    Row {
        case: "an instance snapshot of an instance with three attached disks of differing sizes",
        zone: "ap-southeast-2b",
        source_name: "WinThreeDisks",
        snapshot_name: "WinThreeDisks-2026-10-24",
        snapshot: Snapshot::Instance {
            blueprint_id: "windows_server_2022",
            bundle_id: "large_3_0",
            disks: &[
                ("win-data-one", "/dev/sdf", 8),
                ("win_data_two", "/dev/xvdp", 256),
                ("win-data-3", "/dev/xvdg", 2048),
            ],
        },
    },
    Row {
        case: "a disk snapshot of the smallest disk Lightsail creates",
        zone: "us-west-2a",
        source_name: "small-disk",
        snapshot_name: "s1",
        snapshot: Snapshot::Disk { size_in_gb: 8 },
    },
    Row {
        case: "a disk snapshot of a large disk",
        zone: "ca-central-1a",
        source_name: "archive_disk_2048",
        snapshot_name: "archive_disk_2048_backup",
        snapshot: Snapshot::Disk { size_in_gb: 2048 },
    },
];

/// The modelled `ExportSnapshotRecordSourceType` of `row`'s snapshot, which is
/// also the `Operation$resourceType` the export is logged against.
fn source_type(row: &Row) -> &'static str {
    match row.snapshot {
        Snapshot::Instance { .. } => "InstanceSnapshot",
        Snapshot::Disk { .. } => "DiskSnapshot",
    }
}

/// The id prefix of the Amazon EC2 resource `row`'s export creates.
fn destination_prefix(row: &Row) -> &'static str {
    match row.snapshot {
        Snapshot::Instance { .. } => IMAGE_ID_PREFIX,
        Snapshot::Disk { .. } => VOLUME_ID_PREFIX,
    }
}

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// Stand `row`'s snapshot up, answering the snapshot as the service reports it
/// and the name and ARN of the resource it was taken of.
///
/// Only a snapshot the service has finished taking can be exported, so the
/// snapshot is handed back once the capture the clock owes it has arrived; the
/// refusal of an export before that is pinned in
/// `export_snapshot_completeness.rs`.
async fn create_snapshot(emulator: &Emulator, row: &Row) -> (Value, String, String) {
    match &row.snapshot {
        Snapshot::Instance {
            blueprint_id,
            bundle_id,
            disks,
        } => {
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
            for (disk_name, disk_path, size_in_gb) in *disks {
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
            let instance = emulator
                .ok("GetInstance", json!({"instanceName": row.source_name}))
                .await["instance"]
                .clone();
            emulator
                .ok(
                    "CreateInstanceSnapshot",
                    json!({
                        "instanceName": row.source_name,
                        "instanceSnapshotName": row.snapshot_name,
                    }),
                )
                .await;
            let snapshot = wait_for_snapshot_state(
                emulator,
                Kind::InstanceSnapshot,
                row.snapshot_name,
                Kind::InstanceSnapshot.settled(),
            )
            .await;
            (
                snapshot,
                row.source_name.to_string(),
                instance["arn"].as_str().unwrap_or_default().to_string(),
            )
        }
        Snapshot::Disk { size_in_gb } => {
            create_available_disk(emulator, row.source_name, row.zone, *size_in_gb).await;
            let disk = emulator
                .ok("GetDisk", json!({"diskName": row.source_name}))
                .await["disk"]
                .clone();
            emulator
                .ok(
                    "CreateDiskSnapshot",
                    json!({
                        "diskName": row.source_name,
                        "diskSnapshotName": row.snapshot_name,
                    }),
                )
                .await;
            let snapshot = wait_for_snapshot_state(
                emulator,
                Kind::DiskSnapshot,
                row.snapshot_name,
                Kind::DiskSnapshot.settled(),
            )
            .await;
            (
                snapshot,
                row.source_name.to_string(),
                disk["arn"].as_str().unwrap_or_default().to_string(),
            )
        }
    }
}

/// The ONE export snapshot record this account holds, which is the one the
/// export under test appended.
async fn only_record(emulator: &Emulator, case: &str) -> Value {
    let body = emulator.ok("GetExportSnapshotRecords", json!({})).await;
    let records = body["exportSnapshotRecords"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case}: expected an exportSnapshotRecords array, got {body:?}")
        })
        .clone();
    assert_eq!(
        records.len(),
        1,
        "case {case}: expected the one record this export appended, got {records:?}"
    );
    records[0].clone()
}

/// Poll `GetExportSnapshotRecords` until the one record it reports has
/// succeeded, answering it.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_succeeded(emulator: &Emulator, case: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let record = only_record(emulator, case).await;
        if record["state"].as_str() == Some("Succeeded") {
            return record;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: expected the export to succeed within 5s, got {:?}",
            record["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The `DiskInfo` the record must carry for `snapshot`, as the stored snapshot
/// itself reports its disks: "the disks that were attached to the source
/// instance" (`InstanceSnapshotInfo$fromDiskInfo`), the system volume — the one
/// disk every instance has, which is not a `Disk` resource of its own and so has
/// no name — first.
fn want_disk_info(snapshot: &Value) -> Vec<Value> {
    let system = json!({
        "path": SYSTEM_DISK_PATH,
        "sizeInGb": snapshot["sizeInGb"].clone(),
        "isSystemDisk": true,
    });
    std::iter::once(system)
        .chain(
            snapshot["fromAttachedDisks"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|disk| {
                    json!({
                        "name": disk["name"].clone(),
                        "path": disk["path"].clone(),
                        "sizeInGb": disk["sizeInGb"].clone(),
                        "isSystemDisk": false,
                    })
                }),
        )
        .collect()
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: the record describes its source snapshot as the kind of snapshot it is
/// — an instance snapshot by the blueprint and bundle its source instance was
/// created from plus every disk it captured, a disk snapshot by the snapshotted
/// volume's size — and nothing of the other kind.
#[tokio::test]
async fn an_export_describes_its_snapshot_as_the_kind_of_snapshot_it_is() {
    for row in ROWS {
        let case = row.case;
        // One account per row, so the record under test is the only one the
        // store holds and no earlier row can stand in for it.
        let emulator = Emulator::new();
        let (snapshot, from_resource_name, from_resource_arn) =
            create_snapshot(&emulator, row).await;

        let exported = emulator
            .ok(
                "ExportSnapshot",
                json!({"sourceSnapshotName": row.snapshot_name}),
            )
            .await;
        let operation = single_operation(case, &exported);
        assert_eq!(
            operation["operationType"].as_str(),
            Some("ExportSnapshot"),
            "case {case}: expected the ExportSnapshot operation type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some(source_type(row)),
            "case {case}: expected the exported snapshot's resource type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(row.snapshot_name),
            "case {case}: expected the operation to name the exported snapshot, got {operation:?}"
        );

        // What the record says about the snapshot it exported: the stored
        // snapshot's own members, and the resource THAT snapshot was taken of.
        let record = only_record(&emulator, case).await;
        let source_info = record["sourceInfo"].clone();
        assert_eq!(
            source_info["resourceType"].as_str(),
            Some(source_type(row)),
            "case {case}: expected the snapshot's own source type, got {source_info:?}"
        );
        assert_eq!(
            source_info["name"].as_str(),
            Some(row.snapshot_name),
            "case {case}: expected the exported snapshot's name, got {source_info:?}"
        );
        assert_eq!(
            source_info["arn"], snapshot["arn"],
            "case {case}: expected the exported snapshot's own ARN, got {source_info:?}"
        );
        assert_eq!(
            source_info["createdAt"], snapshot["createdAt"],
            "case {case}: expected the exported snapshot's own creation time, got {source_info:?}"
        );
        assert_eq!(
            source_info["fromResourceName"].as_str(),
            Some(from_resource_name.as_str()),
            "case {case}: expected the snapshot's source resource, got {source_info:?}"
        );
        assert_eq!(
            source_info["fromResourceArn"].as_str(),
            Some(from_resource_arn.as_str()),
            "case {case}: expected the snapshot's source resource ARN, got {source_info:?}"
        );

        match &row.snapshot {
            Snapshot::Instance {
                blueprint_id,
                bundle_id,
                disks,
            } => {
                assert!(
                    source_info["diskSnapshotInfo"].is_null(),
                    "case {case}: the export of an instance snapshot describes no disk snapshot, \
                     got {source_info:?}"
                );
                let info = source_info["instanceSnapshotInfo"].clone();
                assert_eq!(
                    info["fromBlueprintId"].as_str(),
                    Some(*blueprint_id),
                    "case {case}: expected the source instance's blueprint, got {info:?}"
                );
                assert_eq!(
                    info["fromBundleId"].as_str(),
                    Some(*bundle_id),
                    "case {case}: expected the source instance's bundle, got {info:?}"
                );

                // The snapshot really captured the disks the row attached, so
                // the list below is the snapshot's own and not an empty one.
                let captured: BTreeSet<String> = snapshot["fromAttachedDisks"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|disk| disk["name"].as_str().unwrap_or_default().to_string())
                    .collect();
                let attached: BTreeSet<String> = disks
                    .iter()
                    .map(|(name, _, _)| (*name).to_string())
                    .collect();
                assert_eq!(
                    captured, attached,
                    "case {case}: expected the snapshot to have captured every attached disk, got \
                     {captured:?}"
                );

                // The system volume first, then each captured disk: name, path
                // and size as the snapshot holds them.
                let reported = info["fromDiskInfo"].as_array().cloned().unwrap_or_default();
                let want = want_disk_info(&snapshot);
                assert_eq!(
                    reported.len(),
                    want.len(),
                    "case {case}: expected the system volume and every captured disk, got \
                     {reported:?}"
                );
                for (at, (got, want_disk)) in reported.iter().zip(want.iter()).enumerate() {
                    assert_eq!(
                        got["isSystemDisk"], want_disk["isSystemDisk"],
                        "case {case} disk {at}: expected isSystemDisk {:?}, got {got:?}",
                        want_disk["isSystemDisk"]
                    );
                    assert_eq!(
                        got["path"], want_disk["path"],
                        "case {case} disk {at}: expected the path {:?}, got {got:?}",
                        want_disk["path"]
                    );
                    assert_eq!(
                        got["sizeInGb"], want_disk["sizeInGb"],
                        "case {case} disk {at}: expected the size {:?}, got {got:?}",
                        want_disk["sizeInGb"]
                    );
                    // The system volume is not a `Disk` resource, so it has no
                    // name to report; every other entry names its disk.
                    assert_eq!(
                        got["name"], want_disk["name"],
                        "case {case} disk {at}: expected the name {:?}, got {got:?}",
                        want_disk["name"]
                    );
                }

                // The expectations above come out of the stored snapshot; these
                // come out of the ROW, so each captured disk is described with
                // the device path and the size the request that created and
                // attached it named.
                for (at, (disk_name, disk_path, size_in_gb)) in disks.iter().enumerate() {
                    let got = reported
                        .iter()
                        .find(|entry| entry["name"].as_str() == Some(*disk_name))
                        .unwrap_or_else(|| {
                            panic!(
                                "case {case} disk {at}: expected {disk_name} to be described, got \
                                 {reported:?}"
                            )
                        });
                    assert_eq!(
                        got["path"].as_str(),
                        Some(*disk_path),
                        "case {case} disk {at}: expected the device path the attach named, got \
                         {got:?}"
                    );
                    assert_eq!(
                        got["sizeInGb"].as_i64(),
                        Some(*size_in_gb),
                        "case {case} disk {at}: expected the size the disk was created with, got \
                         {got:?}"
                    );
                    assert_eq!(
                        got["isSystemDisk"].as_bool(),
                        Some(false),
                        "case {case} disk {at}: a block storage disk is not a system disk, got \
                         {got:?}"
                    );
                }
            }
            Snapshot::Disk { size_in_gb } => {
                assert!(
                    source_info["instanceSnapshotInfo"].is_null(),
                    "case {case}: the export of a disk snapshot describes no instance snapshot, \
                     got {source_info:?}"
                );
                assert_eq!(
                    source_info["diskSnapshotInfo"]["sizeInGb"].as_i64(),
                    Some(*size_in_gb),
                    "case {case}: expected the snapshotted volume's size, got {source_info:?}"
                );
                assert_eq!(
                    snapshot["sizeInGb"].as_i64(),
                    Some(*size_in_gb),
                    "case {case}: expected the stored snapshot's own size, got {snapshot:?}"
                );
            }
        }

        // The export is logged against the SNAPSHOT, so the snapshot's own
        // operation log carries this export beside the create that took it.
        let logged = emulator
            .ok(
                "GetOperationsForResource",
                json!({"resourceName": row.snapshot_name}),
            )
            .await;
        let entries = answered_entries(case, &logged);
        let types = operation_types(&entries);
        assert!(
            types.iter().any(|kind| kind == "ExportSnapshot"),
            "case {case}: expected the export in the snapshot's own operation log, got {types:?}"
        );
        let operation_id = operation["id"].as_str().unwrap_or_default().to_string();
        assert!(
            entries
                .iter()
                .any(|entry| entry["id"].as_str() == Some(operation_id.as_str())),
            "case {case}: expected the export's own entry {operation_id} in the snapshot's log, \
             got {entries:?}"
        );

        // Only once the export the clock owes has succeeded does the record name
        // the Amazon EC2 resource it created — an image for an instance
        // snapshot, a volume for a disk snapshot.
        let settled = wait_for_succeeded(&emulator, case).await;
        let destination = settled["destinationInfo"].clone();
        assert_eq!(
            destination["service"].as_str(),
            Some("ec2"),
            "case {case}: expected Amazon EC2 as the destination service, got {destination:?}"
        );
        let destination_id = destination["id"].as_str().unwrap_or_default();
        assert!(
            is_ec2_resource_id(destination_id, destination_prefix(row)),
            "case {case}: expected a '{}' Amazon EC2 resource id, got {destination_id:?}",
            destination_prefix(row)
        );
        assert_eq!(
            settled["sourceInfo"], source_info,
            "case {case}: expected the exported snapshot's description to be unchanged by the \
             export succeeding, got {:?}",
            settled["sourceInfo"]
        );
        let settled_operation = wait_for_terminal_operation(&emulator, &operation_id).await;
        assert_eq!(
            settled_operation["status"].as_str(),
            Some("Succeeded"),
            "case {case}: expected the export's log entry to have succeeded, got \
             {settled_operation:?}"
        );
    }
}

/// RULE: a snapshot exports whatever it captured, so a second export of the same
/// snapshot describes it exactly as the first did — the description comes from
/// the stored snapshot, not from the export — while each export creates a
/// resource of its own at the destination.
#[tokio::test]
async fn re_exporting_a_snapshot_describes_it_the_same_way() {
    let emulator = Emulator::new();
    let row = &ROWS[1];
    let case = row.case;
    create_snapshot(&emulator, row).await;

    let mut descriptions: Vec<Value> = Vec::new();
    let mut destination_ids: BTreeSet<String> = BTreeSet::new();
    for export in 1..=2 {
        let answered = emulator
            .ok(
                "ExportSnapshot",
                json!({"sourceSnapshotName": row.snapshot_name}),
            )
            .await;
        let operation = single_operation(case, &answered);
        let id = operation["id"].as_str().unwrap_or_default().to_string();
        wait_for_terminal_operation(&emulator, &id).await;

        let body = emulator.ok("GetExportSnapshotRecords", json!({})).await;
        let records = body["exportSnapshotRecords"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            records.len(),
            export,
            "case {case} export {export}: expected one record per export, got {records:?}"
        );
        for record in &records {
            if record["state"].as_str() != Some("Succeeded") {
                continue;
            }
            let id = record["destinationInfo"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                is_ec2_resource_id(&id, destination_prefix(row)),
                "case {case} export {export}: expected an Amazon EC2 resource id, got {id:?}"
            );
            destination_ids.insert(id);
        }
        descriptions.push(records[export - 1]["sourceInfo"].clone());
    }

    assert_eq!(
        descriptions[0], descriptions[1],
        "case {case}: expected both exports to describe the one stored snapshot identically, got \
         {:?}",
        descriptions[1]
    );
    assert_eq!(
        destination_ids.len(),
        2,
        "case {case}: expected each export to create a resource of its own, got \
         {destination_ids:?}"
    );
}
