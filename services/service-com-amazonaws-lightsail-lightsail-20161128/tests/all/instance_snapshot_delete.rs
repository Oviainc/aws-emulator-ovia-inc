// SPDX-License-Identifier: CC0-1.0

//! The Lightsail instance snapshot's deletion — `DeleteInstanceSnapshot` —
//! driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `DeleteInstanceSnapshot` removes the ONE stored instance
//! snapshot the name it carries addresses — whatever that snapshot's source,
//! size, location, attached disks or state — so the next read of that name is
//! refused and the name is free again, while every OTHER stored record (the
//! source instance, its block storage disks, the account's other snapshots) is
//! left exactly as it was; it answers one operation-log entry of the modelled
//! `DeleteInstanceSnapshot` type, located where the deleted snapshot was and
//! already terminal, because the record is gone as the call returns. A name this
//! account holds no instance snapshot under is refused with the service's
//! `DoesNotExist` envelope and deletes nothing.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The harness, the wire error code and the minted-id check this test target
// already exposes: a sibling suite drives the service through the same front
// door rather than standing up a second one.
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The stored instance snapshot `name`, as `GetInstanceSnapshot` reports it.
async fn snapshot(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok("GetInstanceSnapshot", json!({"instanceSnapshotName": name}))
        .await["instanceSnapshot"]
        .clone()
}

/// Poll `GetInstanceSnapshot` until it reports `want`, answering the snapshot.
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

/// Every instance snapshot name `GetInstanceSnapshots` reports, paged to the end.
async fn listed_names(emulator: &Emulator) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let body = match &token {
            Some(token) => {
                emulator
                    .ok("GetInstanceSnapshots", json!({"pageToken": token}))
                    .await
            }
            None => emulator.ok("GetInstanceSnapshots", json!({})).await,
        };
        names.extend(
            body["instanceSnapshots"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|entry| entry["name"].as_str().unwrap_or_default().to_string()),
        );
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    names
}

/// Assert that `name` names no stored instance snapshot: the service's
/// `DoesNotExist` envelope — HTTP 400, wire error `NotFoundException`, body
/// `code` `DoesNotExist` and the resource-type/name message.
async fn assert_absent(emulator: &Emulator, case: &str, name: &str) {
    let (status, body) = emulator
        .call("GetInstanceSnapshot", json!({"instanceSnapshotName": name}))
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
        Some(format!("The InstanceSnapshot does not exist: {name}").as_str()),
        "case {case}: expected the resource-type/name message for {name}, got {body:?}"
    );
}

/// The `tags` member a row's request carries, in the modelled `TagList` form.
fn tag_list(tags: &[(&str, &str)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({"key": key, "value": value}))
            .collect(),
    )
}

/// Create the disk `name` of `size_in_gb` in `zone` and wait until it is
/// `available`, so it can be attached.
async fn create_available_disk(emulator: &Emulator, name: &str, zone: &str, size_in_gb: i64) {
    emulator
        .ok(
            "CreateDisk",
            json!({"diskName": name, "availabilityZone": zone, "sizeInGb": size_in_gb}),
        )
        .await;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let disk = emulator.ok("GetDisk", json!({"diskName": name})).await["disk"].clone();
        if disk["state"].as_str() == Some("available") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for disk {name} to be available: got {:?}",
            disk["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The deletion table
// ---------------------------------------------------------------------------

/// One snapshot the service must delete.
///
/// The rows differ along everything the rule generalises over: the source's
/// blueprint and bundle — and so the system volume size the deleted record
/// carried — the Availability Zone and so the region of the location the log
/// entry carries, the number of block storage disks attached to the source when
/// the snapshot was taken (none, one, two), the number of tags the deleted
/// record held, the form of the snapshot name (hyphens, underscores, digits,
/// mixed case, the shortest the modelled `ResourceName` pattern admits), and the
/// snapshot's own STATE when the delete arrives (one the clock has settled into
/// `available`, one still `pending`).
struct Row {
    case: &'static str,
    instance_name: &'static str,
    availability_zone: &'static str,
    region: &'static str,
    /// A blueprint id this service publishes (`GetBlueprints`).
    blueprint_id: &'static str,
    /// A bundle id this service serves (`GetBundles`).
    bundle_id: &'static str,
    snapshot_name: &'static str,
    /// The block storage disks attached to the source before the snapshot is
    /// taken: name, device path and size.
    disks: &'static [(&'static str, &'static str, i64)],
    tags: &'static [(&'static str, &'static str)],
    /// Whether to let the clock settle the snapshot into `available` before
    /// deleting it. A row that does not wait is deleted while the copy is still
    /// being taken.
    settled_before_delete: bool,
}

/// The table. Blueprint and bundle ids are entries of this service's own
/// published catalogs (`crate::state::BLUEPRINTS` and
/// `crate::state::BUNDLE_SPECS`).
const ROWS: &[Row] = &[
    Row {
        case: "amazon linux on the smallest published bundle, no attached disk, no tags, settled",
        instance_name: "amazon-nano",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        blueprint_id: "amazon_linux_2023",
        bundle_id: "nano_3_0",
        snapshot_name: "amazon-nano-snapshot",
        disks: &[],
        tags: &[],
        settled_before_delete: true,
    },
    Row {
        case: "an application blueprint, one attached disk, one tag, still pending",
        instance_name: "wordpress_app_1",
        availability_zone: "eu-west-1c",
        region: "eu-west-1",
        blueprint_id: "wordpress",
        bundle_id: "small_3_0",
        snapshot_name: "wordpress_snap_1",
        disks: &[("wordpress-data", "/dev/xvdf", 32)],
        tags: &[("env", "prod")],
        settled_before_delete: false,
    },
    Row {
        case: "windows on a bundle whose id carries the platform token, two attached disks, \
               three tags, settled",
        instance_name: "Win2019Build",
        availability_zone: "ap-southeast-2b",
        region: "ap-southeast-2",
        blueprint_id: "windows_server_2019",
        bundle_id: "medium_win_3_0",
        snapshot_name: "Win2019Build-backup-3",
        disks: &[
            ("win-build-one", "/dev/sdf", 256),
            ("win_build_two", "/dev/xvdp", 512),
        ],
        tags: &[("env", "staging"), ("owner", "builds"), ("tier", "silver")],
        settled_before_delete: true,
    },
    Row {
        case: "debian on a large bundle, shortest snapshot name the ResourceName pattern admits, \
               still pending",
        instance_name: "debian-xlarge",
        availability_zone: "ca-central-1a",
        region: "ca-central-1",
        blueprint_id: "debian_12",
        bundle_id: "xlarge_3_0",
        snapshot_name: "s7",
        disks: &[],
        tags: &[("app", "ci")],
        settled_before_delete: false,
    },
    Row {
        case: "ubuntu on a bundle generation the catalog no longer publishes, a fifth region, \
               one small disk, settled",
        instance_name: "ubuntu_legacy_gen",
        availability_zone: "eu-north-1c",
        region: "eu-north-1",
        blueprint_id: "ubuntu_24_04",
        bundle_id: "micro_2_0",
        snapshot_name: "Snapshot_Legacy_5",
        disks: &[("north-disk-8", "/dev/xvdg", 8)],
        tags: &[("env", "dev"), ("owner", "qa")],
        settled_before_delete: true,
    },
];

/// Stand up `row`'s source instance, attach every disk the row names, and take
/// the snapshot.
async fn create_row(emulator: &Emulator, row: &Row) {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [row.instance_name],
                "availabilityZone": row.availability_zone,
                "blueprintId": row.blueprint_id,
                "bundleId": row.bundle_id,
            }),
        )
        .await;
    for (disk_name, disk_path, size_in_gb) in row.disks {
        create_available_disk(emulator, disk_name, row.availability_zone, *size_in_gb).await;
        emulator
            .ok(
                "AttachDisk",
                json!({
                    "diskName": disk_name,
                    "instanceName": row.instance_name,
                    "diskPath": disk_path,
                }),
            )
            .await;
    }
    let mut body = json!({
        "instanceName": row.instance_name,
        "instanceSnapshotName": row.snapshot_name,
    });
    if !row.tags.is_empty() {
        body["tags"] = tag_list(row.tags);
    }
    emulator.ok("CreateInstanceSnapshot", body).await;
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: `DeleteInstanceSnapshot` removes the one stored snapshot its name
/// addresses and answers one terminal `DeleteInstanceSnapshot` log entry located
/// where that snapshot was; afterwards the name reads back refused and is gone
/// from the listing, while the snapshot's source, the disks attached to it and
/// every other stored snapshot are untouched.
#[tokio::test]
async fn deleting_a_snapshot_removes_that_one_record() {
    let emulator = Emulator::new();

    // A bystander snapshot, created first and never deleted: every deletion
    // below has to leave it exactly as it is.
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["bystander-instance"],
                "availabilityZone": "us-east-2a",
                "blueprintId": "amazon_linux_2",
                "bundleId": "nano_3_0",
            }),
        )
        .await;
    emulator
        .ok(
            "CreateInstanceSnapshot",
            json!({
                "instanceName": "bystander-instance",
                "instanceSnapshotName": "bystander_snapshot",
                "tags": [{"key": "keep", "value": "me"}],
            }),
        )
        .await;
    let bystander = wait_for_state(&emulator, "bystander_snapshot", "available").await;

    let mut operation_ids: BTreeSet<String> = BTreeSet::new();
    for row in ROWS {
        let case = row.case;
        let name = row.snapshot_name;
        create_row(&emulator, row).await;
        let before = if row.settled_before_delete {
            wait_for_state(&emulator, name, "available").await
        } else {
            snapshot(&emulator, name).await
        };
        let state_before = before["state"].as_str().unwrap_or_default().to_string();
        assert!(
            state_before == "pending" || state_before == "available",
            "case {case}: expected a modelled InstanceSnapshotState before the delete, got \
             {state_before:?}"
        );

        let deleted = emulator
            .ok(
                "DeleteInstanceSnapshot",
                json!({"instanceSnapshotName": name}),
            )
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
            Some("DeleteInstanceSnapshot"),
            "case {case}: expected the DeleteInstanceSnapshot operation type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some("InstanceSnapshot"),
            "case {case}: expected the InstanceSnapshot resource type, got {operation:?}"
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
        let names = listed_names(&emulator).await;
        assert!(
            !names.contains(&name.to_string()),
            "case {case}: expected the deleted snapshot to be gone from the listing, got {names:?}"
        );

        // Deleting a point-in-time copy deletes the copy, never what it was
        // taken of: the source instance and every disk attached to it stay.
        let source = emulator
            .ok("GetInstance", json!({"instanceName": row.instance_name}))
            .await["instance"]
            .clone();
        assert_eq!(
            source["name"].as_str(),
            Some(row.instance_name),
            "case {case}: expected the source instance to survive its snapshot's deletion, got \
             {source:?}"
        );
        for (disk_name, disk_path, size_in_gb) in row.disks {
            let disk = emulator
                .ok("GetDisk", json!({"diskName": disk_name}))
                .await["disk"]
                .clone();
            assert_eq!(
                disk["sizeInGb"].as_i64(),
                Some(*size_in_gb),
                "case {case}: expected {disk_name} to survive unchanged, got {disk:?}"
            );
            assert_eq!(
                disk["attachedTo"].as_str(),
                Some(row.instance_name),
                "case {case}: expected {disk_name} to stay attached, got {disk:?}"
            );
            assert_eq!(
                disk["path"].as_str(),
                Some(*disk_path),
                "case {case}: expected {disk_name} to keep its device path, got {disk:?}"
            );
        }
        assert_eq!(
            snapshot(&emulator, "bystander_snapshot").await,
            bystander,
            "case {case}: expected the other stored snapshot to be untouched"
        );
    }
}

/// RULE: a snapshot name addresses one snapshot, so a deletion frees the name —
/// a later `CreateInstanceSnapshot` under it stores a NEW record, with its own
/// ARN and its own source, rather than reviving the deleted one.
#[tokio::test]
async fn deleting_a_snapshot_frees_its_name() {
    let emulator = Emulator::new();
    for (instance, bundle) in [("reused-first", "nano_3_0"), ("reused-second", "large_3_0")] {
        emulator
            .ok(
                "CreateInstances",
                json!({
                    "instanceNames": [instance],
                    "availabilityZone": "us-east-2a",
                    "blueprintId": "amazon_linux_2023",
                    "bundleId": bundle,
                }),
            )
            .await;
    }

    let name = "reused-snapshot-name";
    emulator
        .ok(
            "CreateInstanceSnapshot",
            json!({
                "instanceName": "reused-first",
                "instanceSnapshotName": name,
                "tags": [{"key": "generation", "value": "one"}],
            }),
        )
        .await;
    let first = wait_for_state(&emulator, name, "available").await;

    emulator
        .ok(
            "DeleteInstanceSnapshot",
            json!({"instanceSnapshotName": name}),
        )
        .await;
    assert_absent(&emulator, "the name after its snapshot was deleted", name).await;

    // The same name, now of a snapshot of the OTHER instance: a fresh record,
    // not the deleted one.
    emulator
        .ok(
            "CreateInstanceSnapshot",
            json!({"instanceName": "reused-second", "instanceSnapshotName": name}),
        )
        .await;
    let second = wait_for_state(&emulator, name, "available").await;
    assert_eq!(
        second["fromInstanceName"].as_str(),
        Some("reused-second"),
        "case recreating a deleted name: expected the new source instance, got {second:?}"
    );
    assert_ne!(
        second["sizeInGb"], first["sizeInGb"],
        "case recreating a deleted name: expected the new source's own system volume size, got \
         {second:?}"
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

/// RULE: snapshots of one instance are independent point-in-time copies, so
/// deleting one of them leaves the others readable AND still restorable.
#[tokio::test]
async fn deleting_one_snapshot_of_an_instance_leaves_the_others_restorable() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["chain-instance"],
                "availabilityZone": "us-east-2a",
                "blueprintId": "debian_12",
                "bundleId": "small_3_0",
            }),
        )
        .await;
    let chain = ["chain-snapshot-1", "chain_snapshot_2", "chain-snapshot-3"];
    for name in chain {
        emulator
            .ok(
                "CreateInstanceSnapshot",
                json!({"instanceName": "chain-instance", "instanceSnapshotName": name}),
            )
            .await;
        wait_for_state(&emulator, name, "available").await;
    }
    let first = snapshot(&emulator, chain[0]).await;
    let last = snapshot(&emulator, chain[2]).await;

    // The MIDDLE snapshot of the chain goes.
    emulator
        .ok(
            "DeleteInstanceSnapshot",
            json!({"instanceSnapshotName": chain[1]}),
        )
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
                "CreateInstancesFromSnapshot",
                json!({
                    "instanceNames": [restored.clone()],
                    "instanceSnapshotName": source,
                    "availabilityZone": "us-east-2a",
                    "bundleId": "small_3_0",
                }),
            )
            .await;
        let instance = emulator
            .ok("GetInstance", json!({"instanceName": restored}))
            .await["instance"]
            .clone();
        assert_eq!(
            instance["blueprintId"].as_str(),
            Some("debian_12"),
            "case {case}: expected the snapshotted blueprint on the restored instance, got \
             {instance:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// What a refusal row names.
struct Refusal {
    case: &'static str,
    /// The `instanceSnapshotName` the request carries.
    name: &'static str,
}

/// The names this account holds no INSTANCE SNAPSHOT under: one never created,
/// two near-misses of a stored name, one already deleted, the name of an
/// INSTANCE, the name of a block storage DISK, and the name of a DISK SNAPSHOT —
/// a different resource type, which this operation must not delete.
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
        case: "the name of an instance",
        name: "present-instance",
    },
    Refusal {
        case: "the name of a block storage disk",
        name: "present-disk",
    },
    Refusal {
        case: "the name of a disk snapshot",
        name: "present-disk-snapshot",
    },
];

/// RULE: a well-formed name this account holds no instance snapshot under is
/// refused with the service's `DoesNotExist` envelope — HTTP 400,
/// `NotFoundException`, body `code` `DoesNotExist` and the resource-type/name
/// message — and deletes nothing, so a second delete of one snapshot fails
/// rather than reporting a deletion that did not happen.
#[tokio::test]
async fn a_name_the_account_holds_no_instance_snapshot_under_is_refused() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["present-instance"],
                "availabilityZone": "us-east-2a",
                "blueprintId": "amazon_linux_2",
                "bundleId": "nano_3_0",
            }),
        )
        .await;
    create_available_disk(&emulator, "present-disk", "us-east-2a", 8).await;
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({
                "diskSnapshotName": "present-disk-snapshot",
                "diskName": "present-disk",
            }),
        )
        .await;
    for name in ["present-snapshot", "deleted-snapshot"] {
        emulator
            .ok(
                "CreateInstanceSnapshot",
                json!({"instanceName": "present-instance", "instanceSnapshotName": name}),
            )
            .await;
    }
    let kept = wait_for_state(&emulator, "present-snapshot", "available").await;
    emulator
        .ok(
            "DeleteInstanceSnapshot",
            json!({"instanceSnapshotName": "deleted-snapshot"}),
        )
        .await;

    for refusal in REFUSALS {
        let case = refusal.case;
        let (status, body) = emulator
            .call(
                "DeleteInstanceSnapshot",
                json!({"instanceSnapshotName": refusal.name}),
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
            Some(format!("The InstanceSnapshot does not exist: {}", refusal.name).as_str()),
            "case {case}: expected the resource-type/name message, got {body:?}"
        );
    }

    // Nothing the refusals named was removed: the one snapshot that was never
    // deleted is exactly as it was, and so are the records of the other types
    // the refusals addressed.
    assert_eq!(
        snapshot(&emulator, "present-snapshot").await,
        kept,
        "case after every refusal: expected the stored snapshot to be untouched"
    );
    assert_eq!(
        listed_names(&emulator).await,
        vec!["present-snapshot".to_string()],
        "case after every refusal: expected exactly the one surviving snapshot in the listing"
    );
    let instance = emulator
        .ok("GetInstance", json!({"instanceName": "present-instance"}))
        .await["instance"]
        .clone();
    assert_eq!(
        instance["name"].as_str(),
        Some("present-instance"),
        "case after every refusal: expected the instance to be untouched, got {instance:?}"
    );
    let disk_snapshot = emulator
        .ok(
            "GetDiskSnapshot",
            json!({"diskSnapshotName": "present-disk-snapshot"}),
        )
        .await["diskSnapshot"]
        .clone();
    assert_eq!(
        disk_snapshot["name"].as_str(),
        Some("present-disk-snapshot"),
        "case after every refusal: expected the disk snapshot to be untouched, got \
         {disk_snapshot:?}"
    );
}
