// SPDX-License-Identifier: CC0-1.0

//! Which export snapshot record an AWS CloudFormation stack can be built from —
//! `CreateCloudFormationStack` against an export of each KIND of Lightsail
//! snapshot — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `instances[].sourceName` names an export snapshot record
//! that "contains the exported Lightsail instance snapshot that will be used as
//! the source of the new Amazon EC2 instance", so of the two kinds of snapshot an
//! export record can describe only an INSTANCE snapshot's export is a source a
//! stack can be built from — that export creates the machine image the new
//! instance is launched from, while a DISK snapshot's export creates a block
//! storage volume, which no instance can be launched from. An export of a disk
//! snapshot is refused with the modelled `InvalidInputException` at HTTP 400,
//! appends no stack record and leaves the export record it named exactly as it
//! was; an export of an instance snapshot of the very same account, zone and
//! instance is served.
//!
//! The sibling suite `cloud_formation_stack.rs` covers what every served call
//! does — one record per call, the record's own identity, the clock-owed settle,
//! paging and the service's other refusals. This one covers which kind of export
//! is a source at all.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The harness, the wire error code and the record walks this test target already
// exposes: the sibling suites drive the service through the same front door and
// read both families of record the same way rather than writing either a second
// time.
use crate::cloud_formation_stack::{all_export_records, all_records, export_record};
use crate::instance_lifecycle::{error_code, Emulator};
use crate::snapshot_copy::{wait_for_snapshot_state, Kind};

/// The modelled `ExportSnapshotRecordSourceType` value of an instance snapshot,
/// which is the one kind of export a stack can be built from
/// (`smithy_query(query="shape",shape="ExportSnapshotRecordSourceType")`).
const INSTANCE_SNAPSHOT: &str = "InstanceSnapshot";

/// The other member of that enum: the kind whose export a stack cannot be built
/// from.
const DISK_SNAPSHOT: &str = "DiskSnapshot";

/// The id prefix of the Amazon EC2 resource an exported INSTANCE snapshot
/// becomes: "Exported instance snapshots appear in Amazon EC2 as Amazon Machine
/// Images (AMIs)" (`smithy_query(query="doc",shape="ExportSnapshot")`).
const IMAGE_ID_PREFIX: &str = "ami-";

/// The id prefix of the Amazon EC2 resource an exported DISK snapshot becomes:
/// "Exported disk snapshots appear in Amazon EC2 as Amazon EBS volumes" (same
/// doc).
const VOLUME_ID_PREFIX: &str = "vol-";

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// What one row's DISK snapshot — the snapshot whose export is no source for a
/// stack — is taken of: the two sources `CreateDiskSnapshot` admits.
#[derive(Debug)]
enum Snapshotted {
    /// A block storage disk of this size in GB, created through `CreateDisk`.
    Disk {
        disk_name: &'static str,
        size_in_gb: i64,
    },
    /// The system volume of an instance — the same instance the row's instance
    /// snapshot is taken of, so the two exports differ in nothing but the KIND
    /// of snapshot they exported.
    SystemVolumeOfTheSameInstance,
}

/// One pair of exports to try as the source of a stack: a disk snapshot's and an
/// instance snapshot's, both of resources of the same Availability Zone.
///
/// The rows differ along everything the rule generalises over: what the disk
/// snapshot was taken of (a block storage disk, or an instance's system volume),
/// the size of that disk — the smallest Lightsail creates and a large one — the
/// Availability Zone (and so the region of every resource and record), the
/// blueprint and bundle the snapshotted instance was created from, the form of
/// each snapshot name (hyphens, underscores, digits) and the Amazon EC2 instance
/// type the entry asks for.
struct Row {
    case: &'static str,
    zone: &'static str,
    /// The instance the row's INSTANCE snapshot is taken of.
    instance_name: &'static str,
    blueprint_id: &'static str,
    bundle_id: &'static str,
    instance_snapshot_name: &'static str,
    /// What the row's DISK snapshot is taken of.
    snapshotted: Snapshotted,
    disk_snapshot_name: &'static str,
    instance_type: &'static str,
}

/// The table. Blueprint and bundle ids are entries of this service's own
/// published catalogs (`GetBlueprints` and `GetBundles`).
const ROWS: &[Row] = &[
    Row {
        case: "a disk snapshot of a block storage disk",
        zone: "us-east-1a",
        instance_name: "al2023-source",
        blueprint_id: "amazon_linux_2023",
        bundle_id: "micro_3_0",
        instance_snapshot_name: "al2023-source-snapshot",
        snapshotted: Snapshotted::Disk {
            disk_name: "data-disk",
            size_in_gb: 32,
        },
        disk_snapshot_name: "data-disk-snapshot",
        instance_type: "t2.micro",
    },
    Row {
        case: "a disk snapshot of the system volume of the very same instance",
        zone: "eu-west-1c",
        instance_name: "ubuntu_source_1",
        blueprint_id: "ubuntu_22_04",
        bundle_id: "nano_3_0",
        instance_snapshot_name: "ubuntu_source_1_snap",
        snapshotted: Snapshotted::SystemVolumeOfTheSameInstance,
        disk_snapshot_name: "ubuntu-system-volume-snap",
        instance_type: "m5d.2xlarge",
    },
    Row {
        case: "a disk snapshot of the smallest disk Lightsail creates",
        zone: "ap-southeast-2b",
        instance_name: "WinSource",
        blueprint_id: "windows_server_2022",
        bundle_id: "large_3_0",
        instance_snapshot_name: "WinSource-2026-10-29",
        snapshotted: Snapshotted::Disk {
            disk_name: "small-disk",
            size_in_gb: 8,
        },
        disk_snapshot_name: "s1",
        instance_type: "mac2-m2pro.metal",
    },
    Row {
        case: "a disk snapshot of a large disk",
        zone: "ca-central-1a",
        instance_name: "debian-source-2",
        blueprint_id: "debian_12",
        bundle_id: "medium_3_0",
        instance_snapshot_name: "debian_source_2_snapshot",
        snapshotted: Snapshotted::Disk {
            disk_name: "archive_disk_2048",
            size_in_gb: 2048,
        },
        disk_snapshot_name: "archive_disk_2048_backup",
        instance_type: "m6i.32xlarge",
    },
];

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// Stand `row`'s resources up and ask for both of its snapshots, without waiting
/// for either: every row's captures are asked for first so the clock owes them
/// all at once.
async fn ask_for_snapshots(emulator: &Emulator, row: &Row) {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [row.instance_name],
                "availabilityZone": row.zone,
                "blueprintId": row.blueprint_id,
                "bundleId": row.bundle_id,
            }),
        )
        .await;
    let disk_snapshot = match row.snapshotted {
        Snapshotted::Disk {
            disk_name,
            size_in_gb,
        } => {
            emulator
                .ok(
                    "CreateDisk",
                    json!({
                        "diskName": disk_name,
                        "availabilityZone": row.zone,
                        "sizeInGb": size_in_gb,
                    }),
                )
                .await;
            json!({"diskName": disk_name, "diskSnapshotName": row.disk_snapshot_name})
        }
        Snapshotted::SystemVolumeOfTheSameInstance => {
            json!({
                "instanceName": row.instance_name,
                "diskSnapshotName": row.disk_snapshot_name,
            })
        }
    };
    emulator.ok("CreateDiskSnapshot", disk_snapshot).await;
    emulator
        .ok(
            "CreateInstanceSnapshot",
            json!({
                "instanceName": row.instance_name,
                "instanceSnapshotName": row.instance_snapshot_name,
            }),
        )
        .await;
}

/// Wait until the service has finished taking both of `row`'s snapshots, which is
/// when either can be exported.
async fn wait_for_snapshots(emulator: &Emulator, row: &Row) {
    wait_for_snapshot_state(
        emulator,
        Kind::DiskSnapshot,
        row.disk_snapshot_name,
        Kind::DiskSnapshot.settled(),
    )
    .await;
    wait_for_snapshot_state(
        emulator,
        Kind::InstanceSnapshot,
        row.instance_snapshot_name,
        Kind::InstanceSnapshot.settled(),
    )
    .await;
}

/// The names of every export snapshot record this account holds.
async fn export_names(emulator: &Emulator) -> BTreeSet<String> {
    all_export_records(emulator)
        .await
        .into_iter()
        .map(|record| record["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Export `snapshot_name` and answer the name of the one export snapshot record
/// that export appended, WITHOUT waiting for it: the waits of every row overlap.
async fn ask_for_export(emulator: &Emulator, snapshot_name: &str) -> String {
    let before = export_names(emulator).await;
    emulator
        .ok("ExportSnapshot", json!({"sourceSnapshotName": snapshot_name}))
        .await;
    let new: Vec<String> = export_names(emulator)
        .await
        .difference(&before)
        .cloned()
        .collect();
    assert_eq!(
        new.len(),
        1,
        "case exporting {snapshot_name}: expected exactly one new export record, got {new:?}"
    );
    new[0].clone()
}

/// Poll `GetExportSnapshotRecords` until the export snapshot record `name` names
/// has succeeded, answering it — which is the state a stack can be created from.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_succeeded_export(emulator: &Emulator, name: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let record = export_record(emulator, name).await;
        if record["state"].as_str() == Some("Succeeded") {
            return record;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for the export {name} to succeed: expected it within 5s, got {:?}",
            record["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The `instances` entry `row` asks for, against the export snapshot record
/// `source_name`.
fn entry(row: &Row, source_name: &str) -> Value {
    json!({"instances": [{
        "sourceName": source_name,
        "instanceType": row.instance_type,
        "portInfoSource": "DEFAULT",
        "availabilityZone": row.zone,
    }]})
}

// ---------------------------------------------------------------------------
// The case
// ---------------------------------------------------------------------------

/// RULE: a stack is built only from an export snapshot record of an INSTANCE
/// snapshot; an export of a disk snapshot is refused with the modelled
/// `InvalidInputException` at HTTP 400, appends no stack record and leaves the
/// export record it named alone, while the instance snapshot's export of the same
/// instance, zone and account is served.
#[tokio::test]
async fn only_an_exported_instance_snapshot_is_a_source_for_a_stack() {
    let emulator = Emulator::new();

    // Every capture is asked for before any is waited on, and the same for every
    // export, so the clock owes them all at once.
    for row in ROWS {
        ask_for_snapshots(&emulator, row).await;
    }
    for row in ROWS {
        wait_for_snapshots(&emulator, row).await;
    }
    let mut exports: Vec<(String, String)> = Vec::new();
    for row in ROWS {
        let disk = ask_for_export(&emulator, row.disk_snapshot_name).await;
        let instance = ask_for_export(&emulator, row.instance_snapshot_name).await;
        exports.push((disk, instance));
    }

    let mut served = 0usize;
    for (row, (disk_export_name, instance_export_name)) in ROWS.iter().zip(&exports) {
        let case = row.case;

        // The export of the disk snapshot has succeeded, and what it created at
        // the destination is a block storage volume — so there is no machine
        // image for a stack to launch an instance from.
        let disk_export = wait_for_succeeded_export(&emulator, disk_export_name).await;
        assert_eq!(
            disk_export["sourceInfo"]["resourceType"].as_str(),
            Some(DISK_SNAPSHOT),
            "case {case}: expected the export to report the kind of snapshot it exported, got \
             {disk_export:?}"
        );
        let destination = disk_export["destinationInfo"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            destination.starts_with(VOLUME_ID_PREFIX),
            "case {case}: expected the export's destination to be a volume, got {destination:?}"
        );

        let (status, body) = emulator
            .call("CreateCloudFormationStack", entry(row, disk_export_name))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the invalid-input status for a disk snapshot's export, got \
             {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case}: expected the InvalidInputException wire error, got {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default().to_string();
        assert!(
            message.contains(disk_export_name) && message.contains(DISK_SNAPSHOT),
            "case {case}: expected the refusal to name the record and the kind it exported, got \
             {message:?}"
        );
        let (records, _) = all_records(&emulator).await;
        assert_eq!(
            records.len(),
            served,
            "case {case}: expected a refused call to append no stack record, got {records:?}"
        );

        // The refused call left the export record it named exactly as it was.
        assert_eq!(
            export_record(&emulator, disk_export_name).await,
            disk_export,
            "case {case}: expected the refused call to change nothing about the export record"
        );

        // The INSTANCE snapshot's export of the same instance, in the same zone,
        // is a source: its destination is a machine image, and the stack is
        // created from it.
        let instance_export = wait_for_succeeded_export(&emulator, instance_export_name).await;
        assert_eq!(
            instance_export["sourceInfo"]["resourceType"].as_str(),
            Some(INSTANCE_SNAPSHOT),
            "case {case}: expected the export to report the kind of snapshot it exported, got \
             {instance_export:?}"
        );
        let destination = instance_export["destinationInfo"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            destination.starts_with(IMAGE_ID_PREFIX),
            "case {case}: expected the export's destination to be a machine image, got \
             {destination:?}"
        );
        let created = emulator
            .ok("CreateCloudFormationStack", entry(row, instance_export_name))
            .await;
        let stack_record = created["operations"][0]["resourceName"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        served += 1;
        let (records, _) = all_records(&emulator).await;
        assert_eq!(
            records.len(),
            served,
            "case {case}: expected the one stack record that call appended, got {records:?}"
        );

        // And the record that call left behind names THAT export, read back
        // through `GetCloudFormationStackRecords`.
        let record = records
            .iter()
            .find(|record| record["name"].as_str() == Some(stack_record.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case}: expected the record {stack_record} to be reported")
            });
        assert_eq!(
            record["sourceInfo"][0]["name"].as_str(),
            Some(instance_export_name.as_str()),
            "case {case}: expected the stack record to name the instance snapshot's export, got \
             {record:?}"
        );
        assert_eq!(
            record["sourceInfo"][0]["arn"], instance_export["arn"],
            "case {case}: expected the stack record to carry that export's own ARN, got {record:?}"
        );
    }
}
