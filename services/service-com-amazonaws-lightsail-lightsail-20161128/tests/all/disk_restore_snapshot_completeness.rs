// SPDX-License-Identifier: CC0-1.0

//! Restoring a Lightsail block storage disk from a disk snapshot whose capture
//! is not finished yet, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a snapshot holds a complete copy of the volume it
//! captured only once the service has finished taking it, so a restore naming a
//! source snapshot that has not reached its completed state is refused with the
//! modelled `OperationFailureException` and stores nothing, whatever the rest of
//! the request asks for; the very same request is served once the clock settles
//! the snapshot. The sibling restores — `CreateInstancesFromSnapshot` and
//! `CreateRelationalDatabaseFromSnapshot` — hold their own sources to the same
//! rule, pinned in `instance_restore_from_snapshot.rs` and
//! `relational_database_restore_from_snapshot.rs`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The harness and the wire error code this test target already exposes: a
// sibling suite drives the service through the same front door rather than
// standing up a second one.
use crate::instance_lifecycle::{error_code, Emulator};

/// The `DiskSnapshotState` value a snapshot the service has finished taking
/// reports — the one state a disk can be restored from. The modelled
/// `DiskSnapshotState` enum
/// (`smithy_query(query="shape",shape="DiskSnapshotState")`) carries `pending`,
/// `completed`, `error` and `unknown`; only `completed` says the capture is
/// done, and `pending` is the one this service reaches on its own, while the
/// snapshot is being taken.
const COMPLETED: &str = "completed";
/// See [`COMPLETED`].
const PENDING: &str = "pending";

/// What the snapshot a row restores was taken of: the two sources
/// `CreateDiskSnapshot` admits, so the refusal is exercised on a snapshot of a
/// block storage disk AND on one of an instance's system volume.
#[derive(Debug)]
enum Source {
    /// A block storage disk of this size in GB, created through `CreateDisk`.
    Disk { size_in_gb: i64 },
    /// An instance's system volume: the instance is created from this blueprint
    /// and bundle, and the snapshotted volume's size is the one the service
    /// itself reports for the snapshot.
    Instance {
        blueprint_id: &'static str,
        bundle_id: &'static str,
    },
}

/// One restore the service must refuse while the snapshot is still being taken,
/// and serve once it is taken.
///
/// The rows differ along everything beside the snapshot's state that the restore
/// otherwise turns on: the KIND of volume the snapshot captured (a block storage
/// disk, an instance's system volume) and its size, the Region and zone the
/// snapshot lives in, whether the disk is restored into the snapshot's own zone
/// or another zone of the same Region, how much bigger than the snapshotted
/// volume the new disk is, the number of tags, whether an add-on is asked for,
/// and the form of the new disk's name.
#[derive(Debug)]
struct Row {
    case: &'static str,
    source_name: &'static str,
    snapshot_name: &'static str,
    /// The zone the SOURCE is created in, which is where its snapshot lives.
    source_zone: &'static str,
    /// The zone the new disk is restored into — in the snapshot's own Region.
    restore_zone: &'static str,
    restored_name: &'static str,
    source: Source,
    /// GB on top of the snapshotted volume's own size: 0 restores at exactly the
    /// snapshot's size.
    extra_gb: i64,
    tags: &'static [(&'static str, &'static str)],
    /// `true` asks for the `AutoSnapshot` add-on, so the request carries every
    /// optional member a restore admits.
    with_add_on: bool,
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "the smallest block storage disk, restored at its own size in its own zone",
        source_name: "source-disk-8",
        snapshot_name: "source-disk-8-snapshot",
        source_zone: "us-east-1a",
        restore_zone: "us-east-1a",
        restored_name: "restored-disk-8",
        source: Source::Disk { size_in_gb: 8 },
        extra_gb: 0,
        tags: &[],
        with_add_on: false,
    },
    Row {
        case: "a 64 GB disk, restored larger into another zone of the Region, tagged, with an \
               add-on",
        source_name: "data_disk_64",
        snapshot_name: "data_disk_64_backup",
        source_zone: "eu-central-1b",
        restore_zone: "eu-central-1a",
        restored_name: "restored_disk_192",
        source: Source::Disk { size_in_gb: 64 },
        extra_gb: 128,
        tags: &[("stack", "prod"), ("owner", "storage-team")],
        with_add_on: true,
    },
    Row {
        case: "an instance's system volume, restored at the snapshotted size",
        source_name: "tokyo-instance",
        snapshot_name: "tokyo-system-volume-snapshot",
        source_zone: "ap-northeast-1a",
        restore_zone: "ap-northeast-1a",
        restored_name: "tokyo-system-restore",
        source: Source::Instance {
            blueprint_id: "amazon_linux_2023",
            bundle_id: "nano_3_0",
        },
        extra_gb: 0,
        tags: &[("keep", "yes")],
        with_add_on: false,
    },
];

/// The stored disk snapshot `name`, as `GetDiskSnapshot` reports it.
async fn snapshot(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok("GetDiskSnapshot", json!({"diskSnapshotName": name}))
        .await["diskSnapshot"]
        .clone()
}

/// Poll `GetDiskSnapshot` until it reports `want`, answering the snapshot.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_snapshot_state(emulator: &Emulator, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = snapshot(emulator, name).await;
        if snapshot["state"].as_str() == Some(want) {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for snapshot '{want}' on {name}: expected it within 5s, got {:?}",
            snapshot["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Create `row`'s source resource and start a snapshot of it, WITHOUT waiting
/// for the capture the clock owes.
async fn start_snapshot(emulator: &Emulator, row: &Row) {
    let mut body = json!({"diskSnapshotName": row.snapshot_name});
    match row.source {
        Source::Disk { size_in_gb } => {
            emulator
                .ok(
                    "CreateDisk",
                    json!({
                        "diskName": row.source_name,
                        "availabilityZone": row.source_zone,
                        "sizeInGb": size_in_gb,
                    }),
                )
                .await;
            body["diskName"] = json!(row.source_name);
        }
        Source::Instance {
            blueprint_id,
            bundle_id,
        } => {
            emulator
                .ok(
                    "CreateInstances",
                    json!({
                        "instanceNames": [row.source_name],
                        "availabilityZone": row.source_zone,
                        "blueprintId": blueprint_id,
                        "bundleId": bundle_id,
                    }),
                )
                .await;
            body["instanceName"] = json!(row.source_name);
        }
    }
    emulator.ok("CreateDiskSnapshot", body).await;
}

/// The `CreateDiskFromSnapshot` body for `row`, restoring at `size_in_gb`.
fn restore_body(row: &Row, size_in_gb: i64) -> Value {
    let mut body = json!({
        "diskName": row.restored_name,
        "diskSnapshotName": row.snapshot_name,
        "availabilityZone": row.restore_zone,
        "sizeInGb": size_in_gb,
    });
    if !row.tags.is_empty() {
        body["tags"] = Value::Array(
            row.tags
                .iter()
                .map(|(key, value)| json!({"key": key, "value": value}))
                .collect(),
        );
    }
    if row.with_add_on {
        body["addOns"] = json!([{"addOnType": "AutoSnapshot"}]);
    }
    body
}

/// RULE: a disk is restored from the complete copy a snapshot holds, so a
/// restore whose source snapshot has not reached its completed state is refused
/// with the modelled `OperationFailureException` and stores no disk; once the
/// clock settles the snapshot, the very same request is served and the restored
/// disk reads back as a copy of the snapshotted volume.
#[tokio::test]
async fn a_disk_snapshot_still_being_taken_cannot_be_restored() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        start_snapshot(&emulator, row).await;

        // The precondition this case is about, read from the service's own
        // record rather than assumed: the capture has not finished.
        let taking = snapshot(&emulator, row.snapshot_name).await;
        let state = taking["state"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            state, PENDING,
            "case {case}: expected a snapshot still being taken, got {taking:?}"
        );
        // The snapshotted volume's size is the SERVICE's own account of it,
        // rather than a figure copied into this test.
        let snapshot_size = taking["sizeInGb"].as_i64().unwrap_or_default();
        assert!(
            snapshot_size > 0,
            "case {case}: expected the snapshot to report the captured volume's size, got \
             {taking:?}"
        );

        let body = restore_body(row, snapshot_size + row.extra_gb);
        let (status, answered) = emulator
            .call("CreateDiskFromSnapshot", body.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 refusal of a restore from an unfinished snapshot, got \
             {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "OperationFailureException",
            "case {case}: expected the modelled OperationFailureException, got {answered:?}"
        );
        let message = answered["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(row.snapshot_name) && message.contains(&state),
            "case {case}: expected the refusal to name the snapshot and the state it is in, \
             got {answered:?}"
        );

        // A refused restore stores nothing: the name it asked for is still free,
        // which the `DoesNotExist` read proves.
        let (read_status, read) = emulator
            .call("GetDisk", json!({"diskName": row.restored_name}))
            .await;
        assert_eq!(
            read_status, 400,
            "case {case}: a refused restore stores no disk, got {read_status} {read:?}"
        );

        // Only the snapshot's state stood in the way: the clock settles it, and
        // then the very same request is served.
        let settled = wait_for_snapshot_state(&emulator, row.snapshot_name, COMPLETED).await;
        assert_eq!(
            settled["sizeInGb"].as_i64(),
            Some(snapshot_size),
            "case {case}: a refused restore leaves the snapshot as it was, got {settled:?}"
        );
        emulator.ok("CreateDiskFromSnapshot", body).await;
        let restored = emulator
            .ok("GetDisk", json!({"diskName": row.restored_name}))
            .await["disk"]
            .clone();
        assert_eq!(
            restored["sizeInGb"].as_i64(),
            Some(snapshot_size + row.extra_gb),
            "case {case}: expected the restored disk at the size the request asked for, got \
             {restored:?}"
        );
        assert_eq!(
            restored["location"]["availabilityZone"].as_str(),
            Some(row.restore_zone),
            "case {case}: expected the restored disk in the zone the request named, got \
             {restored:?}"
        );
    }
}
