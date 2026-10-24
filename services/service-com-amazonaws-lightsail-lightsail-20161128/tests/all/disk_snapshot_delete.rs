// SPDX-License-Identifier: CC0-1.0

//! The Lightsail disk snapshot's deletion — `DeleteDiskSnapshot` — driven over
//! the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `DeleteDiskSnapshot` removes the ONE stored disk
//! snapshot the name it carries addresses — whatever that snapshot's source,
//! size, location or state — so the next read of that name is refused and the
//! name is free again, while every OTHER stored record is left exactly as it was
//! ("only the data not needed for any other snapshot is removed… regardless of
//! which prior snapshots have been deleted, all active snapshots will have
//! access to all the information needed to restore the disk"); it answers one
//! operation-log entry of the modelled `DeleteDiskSnapshot` type, located where
//! the deleted snapshot was and already terminal, because the record is gone as
//! the call returns. A name this account holds no disk snapshot under is refused
//! with the service's `DoesNotExist` envelope and deletes nothing.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The harness, the wire error code and the minted-id check this test target
// already exposes: a sibling suite drives the service through the same front
// door rather than standing up a second one.
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

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
async fn wait_for_state(emulator: &Emulator, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = snapshot(emulator, name).await;
        if snapshot["state"].as_str() == Some(want) {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for '{want}' on {name}: expected it within 5s, got {:?}",
            snapshot["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Assert that `name` names no stored disk snapshot: the service's
/// `DoesNotExist` envelope — HTTP 400, wire error `NotFoundException`, body
/// `code` `DoesNotExist` and the resource-type/name message.
async fn assert_absent(emulator: &Emulator, case: &str, name: &str) {
    let (status, body) = emulator
        .call("GetDiskSnapshot", json!({"diskSnapshotName": name}))
        .await;
    assert_eq!(
        status, 400,
        "case {case}: expected a 400 for the deleted {name}, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "NotFoundException",
        "case {case}: expected the modelled NotFoundException for {name}, got {body:?}"
    );
    assert_eq!(
        body["code"].as_str(),
        Some("DoesNotExist"),
        "case {case}: expected code DoesNotExist for {name}, got {body:?}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(format!("The DiskSnapshot does not exist: {name}").as_str()),
        "case {case}: expected the resource-type/name message for {name}, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// The deletion table
// ---------------------------------------------------------------------------

/// What one row snapshots: the two mutually exclusive sources the modelled
/// `CreateDiskSnapshot` input admits.
#[derive(Debug)]
enum Source {
    /// A block storage disk of this size in GB, created through `CreateDisk`.
    Disk { size_in_gb: i64 },
    /// An instance's system volume: the instance is created from this blueprint
    /// and bundle through `CreateInstances`.
    Instance {
        blueprint_id: &'static str,
        bundle_id: &'static str,
    },
}

/// One snapshot the service must delete.
///
/// The rows differ along everything the rule generalises over: the KIND of
/// source (a block storage disk, an instance's system volume), the source
/// volume's size — including the smallest disk Lightsail creates and a large one
/// — the Availability Zone (and so the region of the location the log entry
/// carries), the number of tags the deleted record held, the form of the
/// snapshot name (hyphens, underscores, digits), and the snapshot's own STATE
/// when the delete arrives (one the clock has settled into `completed`, one
/// still `pending`).
struct Row {
    case: &'static str,
    source_name: &'static str,
    snapshot_name: &'static str,
    availability_zone: &'static str,
    region: &'static str,
    source: Source,
    tags: &'static [(&'static str, &'static str)],
    /// Whether to let the clock settle the snapshot into `completed` before
    /// deleting it. A row that does not wait is deleted while the copy is still
    /// being taken.
    settled_before_delete: bool,
}

/// The table. The modelled documentation's own example — `my-disk-snapshot` of
/// the disk `Disk-Virginia-1` — is the first row.
const ROWS: &[Row] = &[
    Row {
        case: "the documented disk example, settled, no tags",
        source_name: "Disk-Virginia-1",
        snapshot_name: "my-disk-snapshot",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        source: Source::Disk { size_in_gb: 32 },
        tags: &[],
        settled_before_delete: true,
    },
    Row {
        case: "the smallest disk Lightsail creates, another region, one tag, still pending",
        source_name: "small-disk-8",
        snapshot_name: "small_disk_snapshot_1",
        availability_zone: "eu-west-1c",
        region: "eu-west-1",
        source: Source::Disk { size_in_gb: 8 },
        tags: &[("tier", "edge")],
        settled_before_delete: false,
    },
    Row {
        case: "a large disk, two tags, settled",
        source_name: "data-disk-4096",
        snapshot_name: "data-disk-4096-backup",
        availability_zone: "ap-southeast-2b",
        region: "ap-southeast-2",
        source: Source::Disk { size_in_gb: 4096 },
        tags: &[("stack", "prod"), ("owner", "storage-team")],
        settled_before_delete: true,
    },
    Row {
        case: "an instance's system volume, the smallest bundle, still pending",
        source_name: "nano-instance",
        snapshot_name: "nano-system-volume-snapshot",
        availability_zone: "us-west-2a",
        region: "us-west-2",
        source: Source::Instance {
            blueprint_id: "amazon_linux_2023",
            bundle_id: "nano_3_0",
        },
        tags: &[],
        settled_before_delete: false,
    },
    Row {
        case: "an instance's system volume, a larger bundle, one tag, settled",
        source_name: "medium_instance_2",
        snapshot_name: "medium-system-volume-snapshot",
        availability_zone: "ca-central-1a",
        region: "ca-central-1",
        source: Source::Instance {
            blueprint_id: "debian_12",
            bundle_id: "medium_3_0",
        },
        tags: &[("keep", "yes")],
        settled_before_delete: true,
    },
];

/// Create `row`'s source resource.
async fn create_source(emulator: &Emulator, row: &Row) {
    match row.source {
        Source::Disk { size_in_gb } => {
            emulator
                .ok(
                    "CreateDisk",
                    json!({
                        "diskName": row.source_name,
                        "availabilityZone": row.availability_zone,
                        "sizeInGb": size_in_gb,
                    }),
                )
                .await;
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
                        "availabilityZone": row.availability_zone,
                        "blueprintId": blueprint_id,
                        "bundleId": bundle_id,
                    }),
                )
                .await;
        }
    }
}

/// The `CreateDiskSnapshot` body for `row`.
fn create_body(row: &Row) -> Value {
    let mut body = json!({"diskSnapshotName": row.snapshot_name});
    match row.source {
        Source::Disk { .. } => body["diskName"] = json!(row.source_name),
        Source::Instance { .. } => body["instanceName"] = json!(row.source_name),
    }
    if !row.tags.is_empty() {
        body["tags"] = Value::Array(
            row.tags
                .iter()
                .map(|(key, value)| json!({"key": key, "value": value}))
                .collect(),
        );
    }
    body
}

/// Assert the source `row` was snapshotted from is still the account's: deleting
/// a point-in-time copy deletes the copy, never what it was taken of.
async fn assert_source_intact(emulator: &Emulator, row: &Row) {
    let case = row.case;
    match row.source {
        Source::Disk { .. } => {
            let disk = emulator
                .ok("GetDisk", json!({"diskName": row.source_name}))
                .await["disk"]
                .clone();
            assert_eq!(
                disk["name"].as_str(),
                Some(row.source_name),
                "case {case}: expected the source disk to survive its snapshot's deletion, got \
                 {disk:?}"
            );
        }
        Source::Instance { .. } => {
            let instance = emulator
                .ok("GetInstance", json!({"instanceName": row.source_name}))
                .await["instance"]
                .clone();
            assert_eq!(
                instance["name"].as_str(),
                Some(row.source_name),
                "case {case}: expected the source instance to survive its snapshot's deletion, \
                 got {instance:?}"
            );
        }
    }
}

/// RULE: `DeleteDiskSnapshot` removes the one stored snapshot its name addresses
/// and answers one terminal `DeleteDiskSnapshot` log entry located where that
/// snapshot was; afterwards the name reads back refused, the snapshot's source
/// and every other stored snapshot are untouched.
#[tokio::test]
async fn deleting_a_snapshot_removes_that_one_record() {
    let emulator = Emulator::new();

    // A bystander snapshot, created first and never deleted: every deletion
    // below has to leave it exactly as it is.
    emulator
        .ok(
            "CreateDisk",
            json!({"diskName": "bystander-disk", "availabilityZone": "us-east-2a", "sizeInGb": 16}),
        )
        .await;
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({
                "diskSnapshotName": "bystander_snapshot",
                "diskName": "bystander-disk",
                "tags": [{"key": "keep", "value": "me"}],
            }),
        )
        .await;
    let bystander = wait_for_state(&emulator, "bystander_snapshot", "completed").await;

    let mut operation_ids: BTreeSet<String> = BTreeSet::new();
    for row in ROWS {
        let case = row.case;
        let name = row.snapshot_name;
        create_source(&emulator, row).await;
        emulator.ok("CreateDiskSnapshot", create_body(row)).await;
        let before = if row.settled_before_delete {
            wait_for_state(&emulator, name, "completed").await
        } else {
            snapshot(&emulator, name).await
        };
        let state_before = before["state"].as_str().unwrap_or_default().to_string();
        assert!(
            state_before == "pending" || state_before == "completed",
            "case {case}: expected a modelled DiskSnapshotState before the delete, got \
             {state_before:?}"
        );

        let deleted = emulator
            .ok("DeleteDiskSnapshot", json!({"diskSnapshotName": name}))
            .await;
        let operations = deleted["operations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {deleted:?}"))
            .clone();
        assert_eq!(
            operations.len(),
            1,
            "case {case}: expected exactly one operation, got {operations:?}"
        );
        let operation = operations[0].clone();
        assert_eq!(
            operation["operationType"].as_str(),
            Some("DeleteDiskSnapshot"),
            "case {case}: expected the DeleteDiskSnapshot operation type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some("DiskSnapshot"),
            "case {case}: expected the DiskSnapshot resource type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(name),
            "case {case}: expected the operation to name the snapshot, got {operation:?}"
        );
        assert_eq!(
            operation["status"].as_str(),
            Some("Succeeded"),
            "case {case}: the record is gone as the call returns, so the operation is done, got \
             {operation:?}"
        );
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(true),
            "case {case}: expected a terminal operation, got {operation:?}"
        );
        assert_eq!(
            operation["location"]["availabilityZone"].as_str(),
            Some(row.availability_zone),
            "case {case}: expected the deleted snapshot's own zone, got {operation:?}"
        );
        assert_eq!(
            operation["location"]["regionName"].as_str(),
            Some(row.region),
            "case {case}: expected the deleted snapshot's own region, got {operation:?}"
        );
        let operation_id = operation["id"].as_str().unwrap_or_default().to_string();
        assert!(
            is_uuid_shaped(&operation_id),
            "case {case}: expected a uuid-shaped operation id, got {operation_id:?}"
        );
        assert!(
            operation_ids.insert(operation_id.clone()),
            "case {case}: expected a fresh operation id, got the already-used {operation_id:?}"
        );

        // The record is gone, and stays gone however long the clock runs: the
        // transition it was owed went with it.
        assert_absent(&emulator, case, name).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_absent(&emulator, case, name).await;

        assert_source_intact(&emulator, row).await;
        assert_eq!(
            snapshot(&emulator, "bystander_snapshot").await,
            bystander,
            "case {case}: expected the other stored snapshot to be untouched"
        );
    }
}

/// RULE: a snapshot name addresses one snapshot, so a deletion frees the name —
/// a later `CreateDiskSnapshot` under it stores a NEW record, with its own ARN,
/// rather than reviving the deleted one.
#[tokio::test]
async fn deleting_a_snapshot_frees_its_name() {
    let emulator = Emulator::new();
    for (disk, size) in [("reused-first", 8_i64), ("reused-second", 64)] {
        emulator
            .ok(
                "CreateDisk",
                json!({
                    "diskName": disk,
                    "availabilityZone": "us-east-2a",
                    "sizeInGb": size,
                }),
            )
            .await;
    }

    let name = "reused-snapshot-name";
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({
                "diskSnapshotName": name,
                "diskName": "reused-first",
                "tags": [{"key": "generation", "value": "one"}],
            }),
        )
        .await;
    let first = wait_for_state(&emulator, name, "completed").await;

    emulator
        .ok("DeleteDiskSnapshot", json!({"diskSnapshotName": name}))
        .await;
    assert_absent(&emulator, "the name after its snapshot was deleted", name).await;

    // The same name, now of a snapshot of the OTHER disk: a fresh record, not
    // the deleted one.
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({"diskSnapshotName": name, "diskName": "reused-second"}),
        )
        .await;
    let second = wait_for_state(&emulator, name, "completed").await;
    assert_eq!(
        second["fromDiskName"].as_str(),
        Some("reused-second"),
        "case recreating a deleted name: expected the new source disk, got {second:?}"
    );
    assert_eq!(
        second["sizeInGb"].as_i64(),
        Some(64),
        "case recreating a deleted name: expected the new source's volume size, got {second:?}"
    );
    assert_ne!(
        second["arn"], first["arn"],
        "case recreating a deleted name: expected its own ARN, got {second:?}"
    );
    assert_eq!(
        second["tags"].as_array().map(Vec::len),
        Some(0),
        "case recreating a deleted name: expected none of the deleted record's tags, got {:?}",
        second["tags"]
    );
}

/// RULE: snapshots of one source are independent records — "regardless of which
/// prior snapshots have been deleted, all active snapshots will have access to
/// all the information needed to restore the disk" — so deleting one of them
/// leaves the others readable AND still restorable.
#[tokio::test]
async fn deleting_one_snapshot_of_a_disk_leaves_the_others_restorable() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateDisk",
            json!({"diskName": "chain-disk", "availabilityZone": "us-east-2a", "sizeInGb": 32}),
        )
        .await;
    let chain = ["chain-snapshot-1", "chain_snapshot_2", "chain-snapshot-3"];
    for name in chain {
        emulator
            .ok(
                "CreateDiskSnapshot",
                json!({"diskSnapshotName": name, "diskName": "chain-disk"}),
            )
            .await;
        wait_for_state(&emulator, name, "completed").await;
    }
    let first = snapshot(&emulator, chain[0]).await;
    let last = snapshot(&emulator, chain[2]).await;

    // The MIDDLE snapshot of the chain goes.
    emulator
        .ok("DeleteDiskSnapshot", json!({"diskSnapshotName": chain[1]}))
        .await;
    assert_absent(&emulator, "the deleted middle snapshot", chain[1]).await;

    assert_eq!(
        snapshot(&emulator, chain[0]).await,
        first,
        "case the snapshot taken before the deleted one: expected it untouched"
    );
    assert_eq!(
        snapshot(&emulator, chain[2]).await,
        last,
        "case the snapshot taken after the deleted one: expected it untouched"
    );

    for (case, source) in [
        ("restoring from the older surviving snapshot", chain[0]),
        ("restoring from the newer surviving snapshot", chain[2]),
    ] {
        let restored = format!("restored-from-{source}");
        emulator
            .ok(
                "CreateDiskFromSnapshot",
                json!({
                    "diskName": restored,
                    "diskSnapshotName": source,
                    "availabilityZone": "us-east-2a",
                    "sizeInGb": 32,
                }),
            )
            .await;
        let disk = emulator.ok("GetDisk", json!({"diskName": restored})).await["disk"].clone();
        assert_eq!(
            disk["sizeInGb"].as_i64(),
            Some(32),
            "case {case}: expected the snapshotted volume's size on the restored disk, got \
             {disk:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// What a refusal row names.
struct Refusal {
    case: &'static str,
    /// The `diskSnapshotName` the request carries.
    name: &'static str,
}

/// The names this account holds no DISK SNAPSHOT under: one never created, one
/// already deleted, the name of a block storage DISK, the name of an INSTANCE,
/// and the name of an INSTANCE SNAPSHOT — a different resource type, which this
/// operation must not delete.
const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "a name never created",
        name: "absent-snapshot",
    },
    Refusal {
        case: "a name close to one that exists",
        name: "present-snapshot-2",
    },
    Refusal {
        case: "a name in another form",
        name: "other_snapshot",
    },
    Refusal {
        case: "the already-deleted snapshot",
        name: "deleted-snapshot",
    },
    Refusal {
        case: "the name of a block storage disk",
        name: "present-disk",
    },
    Refusal {
        case: "the name of an instance",
        name: "present-instance",
    },
    Refusal {
        case: "the name of an instance snapshot",
        name: "present-instance-snapshot",
    },
];

/// RULE: a well-formed name this account holds no disk snapshot under is refused
/// with the service's `DoesNotExist` envelope — HTTP 400, `NotFoundException`,
/// body `code` `DoesNotExist` and the resource-type/name message — and deletes
/// nothing, so a second delete of one snapshot fails rather than reporting a
/// deletion that did not happen.
#[tokio::test]
async fn a_name_the_account_holds_no_disk_snapshot_under_is_refused() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateDisk",
            json!({"diskName": "present-disk", "availabilityZone": "us-east-2a", "sizeInGb": 8}),
        )
        .await;
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["present-instance"],
                "availabilityZone": "us-east-2a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "nano_3_0",
            }),
        )
        .await;
    emulator
        .ok(
            "CreateInstanceSnapshot",
            json!({
                "instanceSnapshotName": "present-instance-snapshot",
                "instanceName": "present-instance",
            }),
        )
        .await;
    for name in ["present-snapshot", "deleted-snapshot"] {
        emulator
            .ok(
                "CreateDiskSnapshot",
                json!({"diskSnapshotName": name, "diskName": "present-disk"}),
            )
            .await;
    }
    let kept = wait_for_state(&emulator, "present-snapshot", "completed").await;
    emulator
        .ok(
            "DeleteDiskSnapshot",
            json!({"diskSnapshotName": "deleted-snapshot"}),
        )
        .await;

    for refusal in REFUSALS {
        let case = refusal.case;
        let (status, body) = emulator
            .call(
                "DeleteDiskSnapshot",
                json!({"diskSnapshotName": refusal.name}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 refusal, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case}: expected the modelled NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected code DoesNotExist, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The DiskSnapshot does not exist: {}", refusal.name).as_str()),
            "case {case}: expected the resource-type/name message, got {body:?}"
        );
    }

    // Nothing the refusals named was removed, and the one snapshot that was
    // never deleted is exactly as it was.
    assert_eq!(
        snapshot(&emulator, "present-snapshot").await,
        kept,
        "case after every refusal: expected the stored snapshot to be untouched"
    );
    let disk = emulator
        .ok("GetDisk", json!({"diskName": "present-disk"}))
        .await["disk"]
        .clone();
    assert_eq!(
        disk["name"].as_str(),
        Some("present-disk"),
        "case after every refusal: expected the disk to be untouched, got {disk:?}"
    );
    let instance_snapshots = emulator.ok("GetInstanceSnapshots", json!({})).await;
    let names: Vec<String> = instance_snapshots["instanceSnapshots"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["present-instance-snapshot".to_string()],
        "case after every refusal: expected the instance snapshot to be untouched, got {names:?}"
    );
}
