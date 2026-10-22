// SPDX-License-Identifier: CC0-1.0

//! Restoring a Lightsail block storage disk from a disk snapshot, driven over
//! the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `CreateDiskFromSnapshot` persists ONE new block storage
//! disk per `diskName` the request names, as a copy of the snapshotted volume the
//! ONE source the request names captured — a disk snapshot this account holds
//! (`diskSnapshotName`) or an automatic snapshot of a disk (`sourceDiskName` with
//! `restoreDate` or `useLatestRestorableAutoSnapshot`), the two being mutually
//! exclusive — taking the new disk's zone, size, tags and add-ons from the
//! request verbatim and every server-assigned member (ARN, support code,
//! creation time, location, resource type, IOPS, state) from them plus the
//! store's own allocators, leaving the snapshot exactly as it was; so a later
//! `GetDisk` reports exactly that record. The disk is created `pending` and the
//! clock, not a reader, settles it into `available`; and a request outside what
//! Lightsail restores (no source or both sources, an automatic-snapshot member
//! without that source, a size below the snapshotted volume or outside the
//! published range, a snapshot of another region, a source the account does not
//! hold, a name the account already holds, an add-on a disk cannot carry) is
//! refused and stores nothing.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The harness, the wire error code and the minted-id check this test target
// already exposes: a sibling suite drives the service through the same front
// door rather than standing up a second one.
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and
/// support code carries.
const ACCOUNT: &str = "000000000000";

/// The published size range of a Lightsail block storage disk, in GB: 8 GB to
/// 16 TB. The modelled `CreateDiskFromSnapshot$sizeInGb` carries no `@range`, so
/// these are the bounds the service itself enforces.
const MIN_SIZE_IN_GB: i64 = 8;
/// See [`MIN_SIZE_IN_GB`].
const MAX_SIZE_IN_GB: i64 = 16 * 1024;

/// The IOPS baseline a Lightsail disk inherits from the EBS general purpose SSD
/// (`gp2`) volume behind it: 3 IOPS per GB, never below 100 and never above
/// 16,000.
fn want_iops(size_in_gb: i64) -> i64 {
    (size_in_gb * 3).clamp(100, 16_000)
}

/// The stored disk `name`, as `GetDisk` reports it.
async fn disk(emulator: &Emulator, name: &str) -> Value {
    emulator.ok("GetDisk", json!({"diskName": name})).await["disk"].clone()
}

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

/// Poll `GetDisk` until it reports `want`, answering the disk.
async fn wait_for_disk_state(emulator: &Emulator, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let disk = disk(emulator, name).await;
        if disk["state"].as_str() == Some(want) {
            return disk;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for disk '{want}' on {name}: expected it within 5s, got {:?}",
            disk["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The restore table
// ---------------------------------------------------------------------------

/// What the snapshot a row restores was taken of: the two sources
/// `CreateDiskSnapshot` admits, so the restore is exercised on a snapshot of a
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

/// One `CreateDiskFromSnapshot` request the service must serve.
///
/// The rows differ along everything the rule generalises over: the KIND of
/// volume the snapshot captured (a block storage disk, an instance's system
/// volume) and its size, the zone the disk is restored into (the snapshot's own
/// zone and another zone of the same region, and so the region in the ARN and
/// the location), how much bigger than the snapshotted volume the new disk is
/// (the same size, and larger), the number of tags, whether an add-on is asked
/// for, and the form of the new disk's name (hyphens, underscores, digits).
struct Row {
    case: &'static str,
    source_name: &'static str,
    snapshot_name: &'static str,
    /// The zone the SOURCE is created in, which is where its snapshot lives.
    source_zone: &'static str,
    /// The zone the new disk is restored into — in the snapshot's own region.
    restore_zone: &'static str,
    region: &'static str,
    restored_name: &'static str,
    source: Source,
    /// GB on top of the snapshotted volume's own size: 0 restores at exactly
    /// the snapshot's size.
    extra_gb: i64,
    tags: &'static [(&'static str, &'static str)],
    /// `Some(time)` asks for the `AutoSnapshot` add-on with that
    /// `snapshotTimeOfDay`; `Some("")` asks for it without one, so the service
    /// picks the hour; `None` asks for no add-on at all.
    auto_snapshot_at: Option<&'static str>,
}

/// The table. The modelled documentation's own example — a disk restored from
/// `my-snapshot` at 32 GB — is the first row.
const ROWS: &[Row] = &[
    Row {
        case: "the documented example: a 32 GB disk snapshot restored at its own size",
        source_name: "source-disk-32",
        snapshot_name: "my-snapshot",
        source_zone: "us-east-2a",
        restore_zone: "us-east-2a",
        region: "us-east-2",
        restored_name: "my-disk",
        source: Source::Disk { size_in_gb: 32 },
        extra_gb: 0,
        tags: &[],
        auto_snapshot_at: None,
    },
    Row {
        case: "the smallest disk, restored larger into another zone of the same region",
        source_name: "small-disk-8",
        snapshot_name: "small_disk_snapshot_1",
        source_zone: "eu-west-1c",
        restore_zone: "eu-west-1a",
        region: "eu-west-1",
        restored_name: "restored_disk_1",
        source: Source::Disk {
            size_in_gb: MIN_SIZE_IN_GB,
        },
        extra_gb: 56,
        tags: &[("tier", "edge")],
        auto_snapshot_at: Some(""),
    },
    Row {
        case: "a large disk snapshot, doubled, two tags and an add-on with its own hour",
        source_name: "data-disk-4096",
        snapshot_name: "data-disk-4096-backup",
        source_zone: "ap-southeast-2b",
        restore_zone: "ap-southeast-2b",
        region: "ap-southeast-2",
        restored_name: "data-disk-8192",
        source: Source::Disk { size_in_gb: 4096 },
        extra_gb: 4096,
        tags: &[("stack", "prod"), ("owner", "storage-team")],
        auto_snapshot_at: Some("06:00"),
    },
    Row {
        case: "an instance's system volume, restored at the snapshotted size",
        source_name: "nano-instance",
        snapshot_name: "nano-system-volume-snapshot",
        source_zone: "us-west-2a",
        restore_zone: "us-west-2b",
        region: "us-west-2",
        restored_name: "nano-system-restore",
        source: Source::Instance {
            blueprint_id: "amazon_linux_2023",
            bundle_id: "nano_3_0",
        },
        extra_gb: 0,
        tags: &[],
        auto_snapshot_at: None,
    },
    Row {
        case: "an instance's system volume from a larger bundle, restored larger",
        source_name: "medium_instance_2",
        snapshot_name: "medium-system-volume-snapshot",
        source_zone: "ca-central-1a",
        restore_zone: "ca-central-1a",
        region: "ca-central-1",
        restored_name: "medium_restore_2",
        source: Source::Instance {
            blueprint_id: "debian_12",
            bundle_id: "medium_3_0",
        },
        extra_gb: 128,
        tags: &[("keep", "yes")],
        auto_snapshot_at: None,
    },
];

/// Create `row`'s source resource and snapshot it, answering the size in GB the
/// snapshot reports for the volume it captured.
///
/// That size is the SERVICE's own account of the snapshotted volume, read back
/// through `GetDiskSnapshot`, rather than a figure copied into this test.
async fn create_snapshot(emulator: &Emulator, row: &Row) -> i64 {
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
    let snapshot = wait_for_snapshot_state(emulator, row.snapshot_name, "completed").await;
    let size = snapshot["sizeInGb"].as_i64().unwrap_or_default();
    assert!(
        size > 0,
        "case {}: expected the snapshot to report the captured volume's size, got {snapshot:?}",
        row.case
    );
    size
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
    match row.auto_snapshot_at {
        None => {}
        Some("") => body["addOns"] = json!([{"addOnType": "AutoSnapshot"}]),
        Some(at) => {
            body["addOns"] = json!([{
                "addOnType": "AutoSnapshot",
                "autoSnapshotAddOnRequest": {"snapshotTimeOfDay": at},
            }]);
        }
    }
    body
}

/// RULE: every member `GetDisk` reports of a restored disk comes from the ONE
/// record `CreateDiskFromSnapshot` stored — the caller's name, zone, size, tags
/// and add-ons verbatim, every server-assigned member computed from them plus
/// the store's allocators — and the snapshot it was restored from is left
/// exactly as it was.
#[tokio::test]
async fn a_restored_disk_reads_back_as_it_was_requested() {
    let emulator = Emulator::new();
    let mut arns: BTreeSet<String> = BTreeSet::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        let name = row.restored_name;
        let snapshot_size = create_snapshot(&emulator, row).await;
        let want_size = snapshot_size + row.extra_gb;
        let snapshot_before = snapshot(&emulator, row.snapshot_name).await;

        // The restore answers ONE operation-log entry — it creates one disk —
        // and it is not terminal yet, because the volume is still being restored
        // when the call returns.
        let restored = emulator
            .ok("CreateDiskFromSnapshot", restore_body(row, want_size))
            .await;
        let operations = restored["operations"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case}: expected an operations array, got {restored:?}")
            })
            .clone();
        assert_eq!(
            operations.len(),
            1,
            "case {case}: expected exactly one operation, got {operations:?}"
        );
        let operation = operations[0].clone();
        assert_eq!(
            operation["operationType"].as_str(),
            Some("CreateDiskFromSnapshot"),
            "case {case}: expected the CreateDiskFromSnapshot operation type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some("Disk"),
            "case {case}: expected the Disk resource type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(name),
            "case {case}: expected the operation to name the new disk, got {operation:?}"
        );
        assert_eq!(
            operation["status"].as_str(),
            Some("Started"),
            "case {case}: expected a Started operation, got {operation:?}"
        );
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(false),
            "case {case}: expected a non-terminal operation, got {operation:?}"
        );
        assert_eq!(
            operation["location"]["availabilityZone"].as_str(),
            Some(row.restore_zone),
            "case {case}: expected the operation to be located where the disk was restored, \
             got {operation:?}"
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

        // The stored record, read back through the implemented reader.
        let restored_disk = disk(&emulator, name).await;
        assert_eq!(
            restored_disk["name"].as_str(),
            Some(name),
            "case {case}: expected the disk name the request carried, got {restored_disk:?}"
        );
        assert_eq!(
            restored_disk["resourceType"].as_str(),
            Some("Disk"),
            "case {case}: expected the Disk resource type, got {restored_disk:?}"
        );
        assert_eq!(
            restored_disk["sizeInGb"].as_i64(),
            Some(want_size),
            "case {case}: expected the size the request asked for, got {restored_disk:?}"
        );
        assert_eq!(
            restored_disk["iops"].as_i64(),
            Some(want_iops(want_size)),
            "case {case}: expected the IOPS of a {want_size} GB disk, got {:?}",
            restored_disk["iops"]
        );
        assert_eq!(
            restored_disk["location"]["availabilityZone"].as_str(),
            Some(row.restore_zone),
            "case {case}: expected the zone the request named, got {:?}",
            restored_disk["location"]
        );
        assert_eq!(
            restored_disk["location"]["regionName"].as_str(),
            Some(row.region),
            "case {case}: expected the region of that zone, got {:?}",
            restored_disk["location"]
        );
        assert_eq!(
            restored_disk["isSystemDisk"].as_bool(),
            Some(false),
            "case {case}: a restored disk is additional block storage, got {restored_disk:?}"
        );
        assert_eq!(
            restored_disk["isAttached"].as_bool(),
            Some(false),
            "case {case}: a restored disk is attached to nothing yet, got {restored_disk:?}"
        );
        assert_eq!(
            restored_disk["attachmentState"].as_str(),
            Some("detached"),
            "case {case}: a restored disk is attached to nothing yet, got {restored_disk:?}"
        );
        assert!(
            restored_disk["attachedTo"].is_null() && restored_disk["path"].is_null(),
            "case {case}: a restored disk names no instance and no path, got {restored_disk:?}"
        );

        // The tags the request carried, verbatim and in order.
        let reported_tags: Vec<(String, String)> = restored_disk["tags"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a tags array, got {restored_disk:?}"))
            .iter()
            .map(|tag| {
                (
                    tag["key"].as_str().unwrap_or_default().to_string(),
                    tag["value"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let want_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported_tags, want_tags,
            "case {case}: expected the tags the request carried, got {:?}",
            restored_disk["tags"]
        );

        // The add-ons the request asked for, enabled, with the hour it named or
        // the one the service picked.
        let add_ons = restored_disk["addOns"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an addOns array, got {restored_disk:?}"))
            .clone();
        match row.auto_snapshot_at {
            None => assert!(
                add_ons.is_empty(),
                "case {case}: expected no add-ons, got {add_ons:?}"
            ),
            Some(at) => {
                assert_eq!(
                    add_ons.len(),
                    1,
                    "case {case}: expected the one add-on the request asked for, got {add_ons:?}"
                );
                assert_eq!(
                    add_ons[0]["name"].as_str(),
                    Some("AutoSnapshot"),
                    "case {case}: expected the AutoSnapshot add-on, got {add_ons:?}"
                );
                assert_eq!(
                    add_ons[0]["status"].as_str(),
                    Some("Enabled"),
                    "case {case}: expected the add-on enabled, got {add_ons:?}"
                );
                let hour = add_ons[0]["snapshotTimeOfDay"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                if at.is_empty() {
                    // The modelled `TimeOfDay` pattern
                    // `^(0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]$`.
                    let (hh, mm) = hour.split_once(':').unwrap_or(("", ""));
                    assert!(
                        hh.len() == 2
                            && mm.len() == 2
                            && hh.parse::<u32>().is_ok_and(|hh| hh <= 23)
                            && mm.parse::<u32>().is_ok_and(|mm| mm <= 59),
                        "case {case}: expected a modelled time of day, got {hour:?}"
                    );
                } else {
                    assert_eq!(
                        hour, at,
                        "case {case}: expected the hour the request named, got {hour:?}"
                    );
                }
            }
        }

        // The ARN carries the restore zone's region and the canonical emulator
        // account, and ends in a uuid no other resource is using; the support
        // code names the same id.
        let arn = restored_disk["arn"].as_str().unwrap_or_default().to_string();
        let prefix = format!("arn:aws:lightsail:{}:{ACCOUNT}:Disk/", row.region);
        let id = arn.strip_prefix(&prefix).unwrap_or_else(|| {
            panic!("case {case}: expected an ARN starting '{prefix}', got {arn:?}")
        });
        assert!(
            is_uuid_shaped(id),
            "case {case}: expected a uuid-shaped ARN id, got {id:?}"
        );
        assert!(
            arns.insert(arn.clone()),
            "case {case}: expected a fresh ARN, got the already-used {arn:?}"
        );
        assert_eq!(
            restored_disk["supportCode"].as_str(),
            Some(format!("{ACCOUNT}/{id}").as_str()),
            "case {case}: expected the account/resource-id support code, got {:?}",
            restored_disk["supportCode"]
        );
        assert!(
            restored_disk["createdAt"].as_f64().unwrap_or_default() > 0.0,
            "case {case}: expected a creation timestamp, got {:?}",
            restored_disk["createdAt"]
        );

        // A restore READS the snapshot: the stored snapshot is untouched.
        let snapshot_after = snapshot(&emulator, row.snapshot_name).await;
        assert_eq!(
            snapshot_after, snapshot_before,
            "case {case}: expected the snapshot to be untouched by the restore, got \
             {snapshot_after:?}"
        );
    }
}

/// RULE: a restored disk is `pending` when the call returns and the CLOCK
/// settles it into `available`; a read reports the state, never moves it.
#[tokio::test]
async fn the_clock_settles_a_restored_disk_into_available() {
    let emulator = Emulator::new();
    let row = &ROWS[0];
    let snapshot_size = create_snapshot(&emulator, row).await;

    let restored = emulator
        .ok("CreateDiskFromSnapshot", restore_body(row, snapshot_size))
        .await;
    assert_eq!(
        restored["operations"][0]["status"].as_str(),
        Some("Started"),
        "case straight after the restore: expected a Started operation, got {restored:?}"
    );

    let fresh = disk(&emulator, row.restored_name).await;
    assert_eq!(
        fresh["state"].as_str(),
        Some("pending"),
        "case straight after the restore: expected pending, got {:?}",
        fresh["state"]
    );

    let settled = wait_for_disk_state(&emulator, row.restored_name, "available").await;
    let again = disk(&emulator, row.restored_name).await;
    assert_eq!(
        again, settled,
        "case reading twice after it settled: expected the same record, got {again:?}"
    );
}

/// RULE: a snapshot is a point-in-time copy, so it restores into as many
/// independent disks as the caller asks for — each with its own name, size and
/// ARN, each at least as large as the snapshotted volume.
#[tokio::test]
async fn one_snapshot_restores_into_independent_disks() {
    let emulator = Emulator::new();
    let row = &ROWS[0];
    let snapshot_size = create_snapshot(&emulator, row).await;

    let cases: &[(&str, i64)] = &[
        ("at the snapshotted size", snapshot_size),
        ("one GB larger", snapshot_size + 1),
        ("far larger", snapshot_size * 4),
    ];
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (index, (case, size)) in cases.iter().enumerate() {
        let name = format!("restore-{index}");
        emulator
            .ok(
                "CreateDiskFromSnapshot",
                json!({
                    "diskName": name,
                    "diskSnapshotName": row.snapshot_name,
                    "availabilityZone": row.restore_zone,
                    "sizeInGb": size,
                }),
            )
            .await;
        let restored = disk(&emulator, &name).await;
        assert_eq!(
            restored["sizeInGb"].as_i64(),
            Some(*size),
            "case {case}: expected the requested size, got {restored:?}"
        );
        let arn = restored["arn"].as_str().unwrap_or_default().to_string();
        assert!(
            seen.insert(arn.clone()),
            "case {case}: expected its own ARN, got the already-used {arn:?}"
        );
    }

    // Every restore is still readable with its own size, so no restore replaced
    // another's record.
    for (index, (case, size)) in cases.iter().enumerate() {
        let restored = disk(&emulator, &format!("restore-{index}")).await;
        assert_eq!(
            restored["sizeInGb"].as_i64(),
            Some(*size),
            "case {case}: expected its own record to be untouched, got {restored:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// Create one disk, one instance and one snapshot of that disk in `us-east-2a`,
/// answering the emulator they live in — the fixture every refusal case below
/// refuses against.
async fn with_one_snapshot() -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateDisk",
            json!({"diskName": "present-disk", "availabilityZone": "us-east-2a", "sizeInGb": 32}),
        )
        .await;
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({"diskSnapshotName": "present-snapshot", "diskName": "present-disk"}),
        )
        .await;
    wait_for_snapshot_state(&emulator, "present-snapshot", "completed").await;
    emulator
}

/// Assert that `body` is refused with HTTP 400 and the modelled
/// `InvalidInputException`, whose message names `member`, and that the disk
/// `disk_name` it asked for was not stored.
async fn refused_as_invalid(emulator: &Emulator, case: &str, body: Value, member: &str) {
    let disk_name = body["diskName"].as_str().unwrap_or_default().to_string();
    let (status, answered) = emulator.call("CreateDiskFromSnapshot", body).await;
    assert_eq!(
        status, 400,
        "case {case}: expected a 400 refusal, got {status} {answered:?}"
    );
    assert_eq!(
        error_code(&answered),
        "InvalidInputException",
        "case {case}: expected the modelled InvalidInputException, got {answered:?}"
    );
    let message = answered["message"].as_str().unwrap_or_default().to_string();
    assert!(
        message.contains(member),
        "case {case}: expected the refusal to name {member}, got {message:?}"
    );
    let (read_status, read_body) = emulator
        .call("GetDisk", json!({"diskName": disk_name}))
        .await;
    assert_eq!(
        read_status, 400,
        "case {case}: a refused restore stores nothing, so the read must be refused too, got \
         {read_status} {read_body:?}"
    );
}

/// RULE: a disk is restored from exactly ONE source, so a request naming both
/// `diskSnapshotName` and `sourceDiskName` — or neither — is refused with the
/// modelled `InvalidInputException` (400) and stores nothing.
#[tokio::test]
async fn a_request_that_does_not_name_exactly_one_source_is_refused() {
    let emulator = with_one_snapshot().await;
    let cases: &[(&str, Value, &str)] = &[
        (
            "both a disk snapshot and a source disk",
            json!({
                "diskName": "two-sources",
                "diskSnapshotName": "present-snapshot",
                "sourceDiskName": "present-disk",
                "useLatestRestorableAutoSnapshot": true,
                "availabilityZone": "us-east-2a",
                "sizeInGb": 32,
            }),
            "sourceDiskName",
        ),
        (
            "no source at all",
            json!({
                "diskName": "no-source",
                "availabilityZone": "us-east-2a",
                "sizeInGb": 32,
            }),
            "diskSnapshotName",
        ),
        (
            "no source, but tags",
            json!({
                "diskName": "no-source-tagged",
                "availabilityZone": "us-east-2a",
                "sizeInGb": 32,
                "tags": [{"key": "stack", "value": "prod"}],
            }),
            "sourceDiskName",
        ),
    ];
    for (case, body, member) in cases {
        refused_as_invalid(&emulator, case, body.clone(), member).await;
    }
}

/// RULE: `restoreDate` and `useLatestRestorableAutoSnapshot` address an
/// AUTOMATIC snapshot, so they hold together only with `sourceDiskName`, never
/// with each other, and a restore date is a `YYYY-MM-DD` date; anything else is
/// refused with the modelled `InvalidInputException` and stores nothing.
#[tokio::test]
async fn the_automatic_snapshot_members_must_hold_together() {
    let emulator = with_one_snapshot().await;
    let cases: &[(&str, Value, &str)] = &[
        (
            "a restore date together with a manual snapshot",
            json!({
                "diskName": "dated-manual",
                "diskSnapshotName": "present-snapshot",
                "restoreDate": "2026-10-22",
                "availabilityZone": "us-east-2a",
                "sizeInGb": 32,
            }),
            "restoreDate",
        ),
        (
            "the latest restorable automatic snapshot, with a manual snapshot",
            json!({
                "diskName": "latest-manual",
                "diskSnapshotName": "present-snapshot",
                "useLatestRestorableAutoSnapshot": true,
                "availabilityZone": "us-east-2a",
                "sizeInGb": 32,
            }),
            "useLatestRestorableAutoSnapshot",
        ),
        (
            "both a restore date and the latest restorable automatic snapshot",
            json!({
                "diskName": "date-and-latest",
                "sourceDiskName": "present-disk",
                "restoreDate": "2026-10-22",
                "useLatestRestorableAutoSnapshot": true,
                "availabilityZone": "us-east-2a",
                "sizeInGb": 32,
            }),
            "restoreDate",
        ),
        (
            "a source disk with neither of them",
            json!({
                "diskName": "source-only",
                "sourceDiskName": "present-disk",
                "availabilityZone": "us-east-2a",
                "sizeInGb": 32,
            }),
            "restoreDate",
        ),
        (
            "a source disk with useLatestRestorableAutoSnapshot explicitly false",
            json!({
                "diskName": "source-not-latest",
                "sourceDiskName": "present-disk",
                "useLatestRestorableAutoSnapshot": false,
                "availabilityZone": "us-east-2a",
                "sizeInGb": 32,
            }),
            "restoreDate",
        ),
        (
            "a restore date that is not a date",
            json!({
                "diskName": "bad-date",
                "sourceDiskName": "present-disk",
                "restoreDate": "22-10-2026",
                "availabilityZone": "us-east-2a",
                "sizeInGb": 32,
            }),
            "restoreDate",
        ),
        (
            "a restore date with a month that is not two digits",
            json!({
                "diskName": "bad-date-2",
                "sourceDiskName": "present-disk",
                "restoreDate": "2026-1-22",
                "availabilityZone": "us-east-2a",
                "sizeInGb": 32,
            }),
            "restoreDate",
        ),
    ];
    for (case, body, member) in cases {
        refused_as_invalid(&emulator, case, body.clone(), member).await;
    }
}

/// RULE: an automatic snapshot is one the `AutoSnapshot` add-on took, and this
/// account has taken none, so a restore that addresses one is refused with the
/// service's `DoesNotExist` envelope — and a `sourceDiskName` that names no disk
/// at all is refused the same way, naming the Disk.
#[tokio::test]
async fn an_automatic_snapshot_this_account_has_not_taken_cannot_be_restored() {
    let emulator = with_one_snapshot().await;
    // A disk with the add-on enabled: the add-on records the hour it would run
    // at, and still nothing has taken an automatic copy.
    emulator
        .ok(
            "CreateDisk",
            json!({
                "diskName": "scheduled-disk",
                "availabilityZone": "us-east-2a",
                "sizeInGb": 32,
                "addOns": [{
                    "addOnType": "AutoSnapshot",
                    "autoSnapshotAddOnRequest": {"snapshotTimeOfDay": "06:00"},
                }],
            }),
        )
        .await;

    let cases: &[(&str, &str, Value, Option<&str>)] = &[
        (
            "the latest restorable automatic snapshot of a plain disk",
            "latest-of-present",
            json!({"sourceDiskName": "present-disk", "useLatestRestorableAutoSnapshot": true}),
            None,
        ),
        (
            "a dated automatic snapshot of a disk with the add-on enabled",
            "dated-of-scheduled",
            json!({"sourceDiskName": "scheduled-disk", "restoreDate": "2026-10-22"}),
            None,
        ),
        (
            "an automatic snapshot of a disk the account does not hold",
            "latest-of-absent",
            json!({"sourceDiskName": "absent-disk", "useLatestRestorableAutoSnapshot": true}),
            Some("The Disk does not exist: absent-disk"),
        ),
    ];

    for (case, disk_name, source, want_message) in cases {
        let mut body = json!({
            "diskName": disk_name,
            "availabilityZone": "us-east-2a",
            "sizeInGb": 32,
        });
        for (member, value) in source.as_object().expect("the source names members") {
            body[member] = value.clone();
        }
        let (status, answered) = emulator.call("CreateDiskFromSnapshot", body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 refusal, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "NotFoundException",
            "case {case}: expected the modelled NotFoundException, got {answered:?}"
        );
        assert_eq!(
            answered["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected code DoesNotExist, got {answered:?}"
        );
        if let Some(message) = want_message {
            assert_eq!(
                answered["message"].as_str(),
                Some(*message),
                "case {case}: expected the resource-type/name message, got {answered:?}"
            );
        }
        let (read_status, read_body) = emulator
            .call("GetDisk", json!({"diskName": disk_name}))
            .await;
        assert_eq!(
            read_status, 400,
            "case {case}: a refused restore stores nothing, got {read_status} {read_body:?}"
        );
    }
}

/// RULE: the snapshot must be one this account holds, so a well-formed name that
/// names none is refused with the service's `DoesNotExist` envelope — naming the
/// resource TYPE the request pointed at — and stores nothing.
#[tokio::test]
async fn a_snapshot_the_account_does_not_hold_is_refused() {
    let emulator = with_one_snapshot().await;
    // The last case points at the DISK's name: a disk is not a disk snapshot, so
    // `diskSnapshotName` finds nothing under it.
    for snapshot_name in ["absent-snapshot", "present-snapshot-2", "present-disk"] {
        let disk_name = format!("restore-of-{snapshot_name}");
        let (status, answered) = emulator
            .call(
                "CreateDiskFromSnapshot",
                json!({
                    "diskName": disk_name,
                    "diskSnapshotName": snapshot_name,
                    "availabilityZone": "us-east-2a",
                    "sizeInGb": 32,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case restoring {snapshot_name}: expected a 400 refusal, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "NotFoundException",
            "case restoring {snapshot_name}: expected the modelled NotFoundException, got \
             {answered:?}"
        );
        assert_eq!(
            answered["code"].as_str(),
            Some("DoesNotExist"),
            "case restoring {snapshot_name}: expected code DoesNotExist, got {answered:?}"
        );
        assert_eq!(
            answered["message"].as_str(),
            Some(format!("The DiskSnapshot does not exist: {snapshot_name}").as_str()),
            "case restoring {snapshot_name}: expected the resource-type/name message, got \
             {answered:?}"
        );
        let (read_status, read_body) = emulator
            .call("GetDisk", json!({"diskName": disk_name}))
            .await;
        assert_eq!(
            read_status, 400,
            "case restoring {snapshot_name}: a refused restore stores nothing, got \
             {read_status} {read_body:?}"
        );
    }
}

/// RULE: a restored disk holds every block the snapshot captured and is a disk
/// Lightsail creates, so its size is at least the snapshotted volume's and
/// within the published 8 GB — 16 TB range; a size outside that is refused and
/// stores nothing, each published bound is accepted and each step past it is
/// not.
#[tokio::test]
async fn a_restored_disk_is_sized_between_the_snapshot_and_the_published_maximum() {
    let emulator = Emulator::new();
    // The snapshot is of the smallest disk Lightsail creates, so the whole
    // published range is on one side of it.
    emulator
        .ok(
            "CreateDisk",
            json!({
                "diskName": "floor-disk",
                "availabilityZone": "us-east-2a",
                "sizeInGb": MIN_SIZE_IN_GB,
            }),
        )
        .await;
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({"diskSnapshotName": "floor-snapshot", "diskName": "floor-disk"}),
        )
        .await;
    wait_for_snapshot_state(&emulator, "floor-snapshot", "completed").await;
    // A second snapshot, of a bigger volume, so the floor under a restore is the
    // SNAPSHOT's size and not the published minimum.
    emulator
        .ok(
            "CreateDisk",
            json!({"diskName": "mid-disk", "availabilityZone": "us-east-2a", "sizeInGb": 64}),
        )
        .await;
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({"diskSnapshotName": "mid-snapshot", "diskName": "mid-disk"}),
        )
        .await;
    wait_for_snapshot_state(&emulator, "mid-snapshot", "completed").await;

    // (case, snapshot, size, accepted)
    let cases: &[(&str, &str, i64, bool)] = &[
        (
            "the published minimum, which is the snapshot's size",
            "floor-snapshot",
            MIN_SIZE_IN_GB,
            true,
        ),
        (
            "one GB below the published minimum",
            "floor-snapshot",
            MIN_SIZE_IN_GB - 1,
            false,
        ),
        (
            "the published maximum",
            "floor-snapshot",
            MAX_SIZE_IN_GB,
            true,
        ),
        (
            "one GB past the published maximum",
            "floor-snapshot",
            MAX_SIZE_IN_GB + 1,
            false,
        ),
        ("the snapshotted volume's own size", "mid-snapshot", 64, true),
        (
            "one GB below the snapshotted volume",
            "mid-snapshot",
            63,
            false,
        ),
        (
            "well below the snapshotted volume, but a size Lightsail creates",
            "mid-snapshot",
            MIN_SIZE_IN_GB,
            false,
        ),
        ("one GB above the snapshotted volume", "mid-snapshot", 65, true),
    ];

    for (index, (case, snapshot_name, size, accepted)) in cases.iter().enumerate() {
        let disk_name = format!("sized-{index}");
        let body = json!({
            "diskName": disk_name,
            "diskSnapshotName": snapshot_name,
            "availabilityZone": "us-east-2a",
            "sizeInGb": size,
        });
        if *accepted {
            emulator.ok("CreateDiskFromSnapshot", body).await;
            let restored = disk(&emulator, &disk_name).await;
            assert_eq!(
                restored["sizeInGb"].as_i64(),
                Some(*size),
                "case {case}: expected the accepted size to be stored, got {restored:?}"
            );
        } else {
            refused_as_invalid(&emulator, case, body, "sizeInGb").await;
        }
    }
}

/// RULE: a snapshot belongs to the region it was taken in, so it restores only
/// into a zone of that region; a zone of another region — or no Lightsail region
/// at all — is refused and stores nothing.
#[tokio::test]
async fn a_snapshot_restores_only_into_its_own_region() {
    let emulator = with_one_snapshot().await;
    let cases: &[(&str, &str, &str)] = &[
        (
            "another region's zone",
            "cross-region",
            "eu-west-1a",
        ),
        (
            "a region on another continent",
            "cross-continent",
            "ap-southeast-2b",
        ),
        (
            "a zone of no Lightsail region",
            "no-such-region",
            "xx-middle-9z",
        ),
    ];
    for (case, disk_name, zone) in cases {
        refused_as_invalid(
            &emulator,
            case,
            json!({
                "diskName": disk_name,
                "diskSnapshotName": "present-snapshot",
                "availabilityZone": zone,
                "sizeInGb": 32,
            }),
            "availabilityZone",
        )
        .await;
    }
}

/// RULE: a disk name addresses ONE disk, so restoring onto a name the account
/// already holds is refused and leaves the stored disk exactly as it was; and of
/// the modelled `AddOnType` members only `AutoSnapshot` describes a disk, so
/// asking a restored disk for the idle-stop add-on is refused too.
#[tokio::test]
async fn a_restore_that_cannot_stand_leaves_the_store_as_it_was() {
    let emulator = with_one_snapshot().await;
    let before = wait_for_disk_state(&emulator, "present-disk", "available").await;

    let (status, body) = emulator
        .call(
            "CreateDiskFromSnapshot",
            json!({
                "diskName": "present-disk",
                "diskSnapshotName": "present-snapshot",
                "availabilityZone": "us-east-2a",
                "sizeInGb": 64,
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case restoring onto an existing disk: expected a 400 refusal, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case restoring onto an existing disk: expected the modelled InvalidInputException, got \
         {body:?}"
    );
    let after = disk(&emulator, "present-disk").await;
    assert_eq!(
        after, before,
        "case restoring onto an existing disk: expected the stored disk to be untouched, got \
         {after:?}"
    );

    refused_as_invalid(
        &emulator,
        "an instance add-on on a restored disk",
        json!({
            "diskName": "idle-stop-restore",
            "diskSnapshotName": "present-snapshot",
            "availabilityZone": "us-east-2a",
            "sizeInGb": 32,
            "addOns": [{
                "addOnType": "StopInstanceOnIdle",
                "stopInstanceOnIdleRequest": {"threshold": "5", "duration": "20"},
            }],
        }),
        "addOns",
    )
    .await;
}
