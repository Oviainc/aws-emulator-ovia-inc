// SPDX-License-Identifier: CC0-1.0

//! Copying a Lightsail snapshot — `CopySnapshot` — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `CopySnapshot` persists ONE new MANUAL snapshot under
//! `targetSnapshotName`, in the Region the request is made in, holding what the
//! snapshot it names holds — the same KIND of snapshot, the snapshotted volume's
//! size, and everything that snapshot says about the resource it was taken of —
//! where the snapshot it names is either a manual snapshot of `sourceRegion`
//! (`sourceSnapshotName`) or one of the copies the automatic snapshot add-on took
//! of a source instance or disk there (`sourceResourceName` with `restoreDate`,
//! or with `useLatestRestorableAutoSnapshot`). The copy is created with the state
//! a snapshot being taken has and the CLOCK, not a reader, settles it; the
//! snapshot it was copied from is left exactly as it was; and a request that
//! names no source, both sources, an automatic-snapshot member beside a manual
//! source, a source the Region does not hold, a source that is still being taken
//! or a target name already taken is refused and stores nothing.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The harness, the wire error code, the minted-id check and the settled-state
// wait this test target already exposes: a sibling suite drives the service
// through the same front door rather than standing up a second one.
use crate::auto_snapshots::{auto_snapshot_request, copies, wait_for_success};
use crate::instance_lifecycle::{error_code, is_uuid_shaped, wait_for_state, Emulator};

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and
/// support code carries.
const ACCOUNT: &str = "000000000000";

/// A Linux/Unix blueprint every instance in this suite is created from: nothing
/// about a copy turns on the image.
const BLUEPRINT: &str = "amazon_linux_2023";

/// The `OperationStatus` of an entry whose work is still running, and the state
/// a snapshot being taken is in.
const STARTED: &str = "Started";

/// How long a case waits for a transition the clock owes.
const WAIT: Duration = Duration::from_secs(5);

/// The kind of snapshot a copy is: the two the operation's own documentation
/// names ("a manual snapshot of an instance or disk"), which are the two kinds
/// this store holds.
///
/// `pub(crate)` so a sibling suite in this test target names the two kinds and
/// reads one back the same way rather than describing them a second time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    DiskSnapshot,
    InstanceSnapshot,
}

impl Kind {
    /// The modelled `ResourceType` value a snapshot of this kind reports.
    pub(crate) fn resource_type(self) -> &'static str {
        match self {
            Self::DiskSnapshot => "DiskSnapshot",
            Self::InstanceSnapshot => "InstanceSnapshot",
        }
    }

    /// The operation that reads one back, and the member naming it.
    pub(crate) fn reader(self) -> (&'static str, &'static str) {
        match self {
            Self::DiskSnapshot => ("GetDiskSnapshot", "diskSnapshotName"),
            Self::InstanceSnapshot => ("GetInstanceSnapshot", "instanceSnapshotName"),
        }
    }

    /// The member of that answer the snapshot is reported under.
    pub(crate) fn reported_as(self) -> &'static str {
        match self {
            Self::DiskSnapshot => "diskSnapshot",
            Self::InstanceSnapshot => "instanceSnapshot",
        }
    }

    /// The state a snapshot of this kind is in while it is being taken, from the
    /// modelled `DiskSnapshotState` and `InstanceSnapshotState` enums.
    ///
    /// `pub(crate)` so a sibling suite names the state a capture the service has
    /// not finished reports the same way rather than spelling it a second time.
    pub(crate) fn being_taken(self) -> &'static str {
        "pending"
    }

    /// The state it settles into: `completed` for a disk snapshot,
    /// `available` for an instance snapshot.
    pub(crate) fn settled(self) -> &'static str {
        match self {
            Self::DiskSnapshot => "completed",
            Self::InstanceSnapshot => "available",
        }
    }
}

/// The stored snapshot `name` of `kind`, as its reader reports it.
pub(crate) async fn snapshot(emulator: &Emulator, kind: Kind, name: &str) -> Value {
    let (operation, member) = kind.reader();
    emulator.ok(operation, json!({member: name})).await[kind.reported_as()].clone()
}

/// Poll the reader until the snapshot `name` of `kind` reports `want`, answering
/// it.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
pub(crate) async fn wait_for_snapshot_state(
    emulator: &Emulator,
    kind: Kind,
    name: &str,
    want: &str,
) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        let reported = snapshot(emulator, kind, name).await;
        if reported["state"].as_str() == Some(want) {
            return reported;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for '{want}' on {name}: expected it within {WAIT:?}, got {:?}",
            reported["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The stored disk `name`, as `GetDisk` reports it.
async fn disk(emulator: &Emulator, name: &str) -> Value {
    emulator.ok("GetDisk", json!({"diskName": name})).await["disk"].clone()
}

/// The stored instance `name`, as `GetInstance` reports it.
async fn instance(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok("GetInstance", json!({"instanceName": name}))
        .await["instance"]
        .clone()
}

/// Create a block storage disk, carrying `add_ons` when any are asked for, and
/// answer it as `GetDisk` reports it.
async fn create_disk(
    emulator: &Emulator,
    name: &str,
    zone: &str,
    size_in_gb: i64,
    add_ons: &[Value],
) -> Value {
    let mut body = json!({
        "diskName": name,
        "availabilityZone": zone,
        "sizeInGb": size_in_gb,
    });
    if !add_ons.is_empty() {
        body["addOns"] = json!(add_ons);
    }
    emulator.ok("CreateDisk", body).await;
    disk(emulator, name).await
}

/// Create an instance and answer it as `GetInstance` reports it.
async fn create_instance(emulator: &Emulator, name: &str, zone: &str, bundle_id: &str) -> Value {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [name],
                "availabilityZone": zone,
                "blueprintId": BLUEPRINT,
                "bundleId": bundle_id,
            }),
        )
        .await;
    instance(emulator, name).await
}

// ---------------------------------------------------------------------------
// The copy table
// ---------------------------------------------------------------------------

/// What one row copies.
#[derive(Debug)]
enum Source {
    /// A manual disk snapshot of a block storage disk of this size in GB.
    DiskSnapshotOfDisk { size_in_gb: i64 },
    /// A manual disk snapshot of the system volume of an instance on this plan.
    DiskSnapshotOfSystemVolume { bundle_id: &'static str },
    /// A manual instance snapshot of an instance on this plan.
    InstanceSnapshotOfInstance { bundle_id: &'static str },
    /// One of the automatic snapshots of a block storage disk of this size,
    /// addressed by the date it was taken on.
    AutomaticSnapshotOfDiskByDate { size_in_gb: i64 },
    /// The latest restorable automatic snapshot of an instance on this plan,
    /// which had a block storage disk of this size attached at this device path
    /// when the copy was taken.
    LatestAutomaticSnapshotOfInstance {
        bundle_id: &'static str,
        attached_gb: i64,
        attached_path: &'static str,
    },
}

/// One `CopySnapshot` request the service must serve.
///
/// The rows differ along everything the rule generalises over: the KIND of
/// snapshot copied (a disk snapshot, an instance snapshot), what that snapshot
/// was taken of (a block storage disk, an instance's system volume, a whole
/// instance, an instance with a block storage disk attached), whether it is a
/// manual snapshot or one the automatic snapshot add-on took, how an automatic
/// one is addressed (by its restore date, by asking for the latest restorable
/// one), the snapshotted volume's size, whether the copy is made within the
/// snapshot's own Region or into another one, and the form of the target name
/// (hyphens, underscores, digits).
struct Row {
    case: &'static str,
    /// The instance or disk the copied snapshot was taken of.
    source_resource: &'static str,
    /// The manual snapshot a manual row copies; `None` for an automatic row,
    /// whose source snapshot has no name of its own.
    source_snapshot: Option<&'static str>,
    source_zone: &'static str,
    source_region: &'static str,
    /// The Region the request is made in, which is the Region the copy is
    /// created in.
    target_region: &'static str,
    target_name: &'static str,
    source: Source,
    kind: Kind,
    /// Whether the copy reports `isFromAutoSnapshot`: true exactly for a copy of
    /// an automatic snapshot.
    from_auto_snapshot: bool,
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "a manual disk snapshot of a disk, copied within its own Region",
        source_resource: "Disk-Virginia-1",
        source_snapshot: Some("my-disk-snapshot"),
        source_zone: "us-east-1a",
        source_region: "us-east-1",
        target_region: "us-east-1",
        target_name: "my-disk-snapshot-copy",
        source: Source::DiskSnapshotOfDisk { size_in_gb: 8 },
        kind: Kind::DiskSnapshot,
        from_auto_snapshot: false,
    },
    Row {
        case: "a manual disk snapshot of a system volume, copied into another Region",
        source_resource: "ireland_instance_1",
        source_snapshot: Some("ireland_system_volume_snapshot"),
        source_zone: "eu-west-1c",
        source_region: "eu-west-1",
        target_region: "us-east-2",
        target_name: "ireland_system_volume_copy_1",
        source: Source::DiskSnapshotOfSystemVolume {
            bundle_id: "nano_3_0",
        },
        kind: Kind::DiskSnapshot,
        from_auto_snapshot: false,
    },
    Row {
        case: "a manual instance snapshot, copied into another Region",
        source_resource: "tokyo-instance",
        source_snapshot: Some("tokyo-instance-snapshot"),
        source_zone: "ap-northeast-1a",
        source_region: "ap-northeast-1",
        target_region: "eu-central-1",
        target_name: "tokyo-instance-snapshot-copy",
        source: Source::InstanceSnapshotOfInstance {
            bundle_id: "medium_3_0",
        },
        kind: Kind::InstanceSnapshot,
        from_auto_snapshot: false,
    },
    Row {
        case: "an automatic snapshot of a disk, copied by its restore date within the Region",
        source_resource: "canada_data_disk_64",
        source_snapshot: None,
        source_zone: "ca-central-1a",
        source_region: "ca-central-1",
        target_region: "ca-central-1",
        target_name: "canada_data_disk_64_manual_copy",
        source: Source::AutomaticSnapshotOfDiskByDate { size_in_gb: 64 },
        kind: Kind::DiskSnapshot,
        from_auto_snapshot: true,
    },
    Row {
        case: "the latest restorable automatic snapshot of an instance with an attached disk, \
               copied into another Region",
        source_resource: "oregon-instance",
        source_snapshot: None,
        source_zone: "us-west-2a",
        source_region: "us-west-2",
        target_region: "sa-east-1",
        target_name: "oregon-instance-auto-copy",
        source: Source::LatestAutomaticSnapshotOfInstance {
            bundle_id: "small_3_0",
            attached_gb: 16,
            attached_path: "/dev/xvdf",
        },
        kind: Kind::InstanceSnapshot,
        from_auto_snapshot: true,
    },
];

/// What the copy of `row`'s source must report, read from the service's own
/// records rather than copied into this test, together with the request that
/// makes the copy.
struct Expected {
    /// The size of the snapshotted volume.
    size_in_gb: i64,
    /// The `fromDiskName`/`fromDiskArn` pair, when the snapshot was taken of a
    /// block storage disk.
    from_disk: Option<(String, String)>,
    /// The `fromInstanceName`/`fromInstanceArn` pair, when it was taken of an
    /// instance.
    from_instance: Option<(String, String)>,
    /// The `fromBlueprintId`/`fromBundleId` pair an instance snapshot carries.
    from_image: Option<(String, String)>,
    /// The device path and size of each block storage disk an instance snapshot
    /// captured.
    from_attached_disks: Vec<(String, i64)>,
    /// The `CopySnapshot` body that copies the source.
    copy_body: Value,
}

/// Create `row`'s source resource and the snapshot the row copies, waiting until
/// that snapshot holds a complete copy, and answer what the copy of it must say.
async fn prepare(emulator: &Emulator, row: &Row) -> Expected {
    let case = row.case;
    let mut copy_body = json!({
        "sourceRegion": row.source_region,
        "targetSnapshotName": row.target_name,
    });
    if let Some(source_snapshot) = row.source_snapshot {
        copy_body["sourceSnapshotName"] = json!(source_snapshot);
    }
    match row.source {
        Source::DiskSnapshotOfDisk { size_in_gb } => {
            let source = create_disk(emulator, row.source_resource, row.source_zone, size_in_gb, &[])
                .await;
            let snapshot_name = row.source_snapshot.expect("a manual row names its snapshot");
            emulator
                .ok(
                    "CreateDiskSnapshot",
                    json!({
                        "diskSnapshotName": snapshot_name,
                        "diskName": row.source_resource,
                    }),
                )
                .await;
            let taken =
                wait_for_snapshot_state(emulator, row.kind, snapshot_name, row.kind.settled())
                    .await;
            Expected {
                size_in_gb: reported_size(case, &taken),
                from_disk: Some(named(row.source_resource, &source)),
                from_instance: None,
                from_image: None,
                from_attached_disks: Vec::new(),
                copy_body,
            }
        }
        Source::DiskSnapshotOfSystemVolume { bundle_id } => {
            let source =
                create_instance(emulator, row.source_resource, row.source_zone, bundle_id).await;
            let snapshot_name = row.source_snapshot.expect("a manual row names its snapshot");
            emulator
                .ok(
                    "CreateDiskSnapshot",
                    json!({
                        "diskSnapshotName": snapshot_name,
                        "instanceName": row.source_resource,
                    }),
                )
                .await;
            let taken =
                wait_for_snapshot_state(emulator, row.kind, snapshot_name, row.kind.settled())
                    .await;
            Expected {
                size_in_gb: reported_size(case, &taken),
                from_disk: None,
                from_instance: Some(named(row.source_resource, &source)),
                from_image: None,
                from_attached_disks: Vec::new(),
                copy_body,
            }
        }
        Source::InstanceSnapshotOfInstance { bundle_id } => {
            let source =
                create_instance(emulator, row.source_resource, row.source_zone, bundle_id).await;
            let snapshot_name = row.source_snapshot.expect("a manual row names its snapshot");
            emulator
                .ok(
                    "CreateInstanceSnapshot",
                    json!({
                        "instanceSnapshotName": snapshot_name,
                        "instanceName": row.source_resource,
                    }),
                )
                .await;
            let taken =
                wait_for_snapshot_state(emulator, row.kind, snapshot_name, row.kind.settled())
                    .await;
            Expected {
                size_in_gb: reported_size(case, &taken),
                from_disk: None,
                from_instance: Some(named(row.source_resource, &source)),
                from_image: Some(image_of(case, &taken)),
                from_attached_disks: Vec::new(),
                copy_body,
            }
        }
        Source::AutomaticSnapshotOfDiskByDate { size_in_gb } => {
            // The add-on is enabled as the disk is created, so the schedule owes
            // the disk its first copy and the CLOCK takes it.
            let source = create_disk(
                emulator,
                row.source_resource,
                row.source_zone,
                size_in_gb,
                &[auto_snapshot_request(None)],
            )
            .await;
            let taken = wait_for_success(emulator, case, row.source_resource).await;
            let date = taken["date"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected the copy's date, got {taken:?}"))
                .to_string();
            copy_body["sourceResourceName"] = json!(row.source_resource);
            copy_body["restoreDate"] = json!(date);
            Expected {
                size_in_gb: source["sizeInGb"].as_i64().unwrap_or_default(),
                from_disk: Some(named(row.source_resource, &source)),
                from_instance: None,
                from_image: None,
                from_attached_disks: Vec::new(),
                copy_body,
            }
        }
        Source::LatestAutomaticSnapshotOfInstance {
            bundle_id,
            attached_gb,
            attached_path,
        } => {
            let source =
                create_instance(emulator, row.source_resource, row.source_zone, bundle_id).await;
            // The disk is attached BEFORE the add-on is enabled, so the copy the
            // schedule then owes captures it.
            wait_for_state(emulator, row.source_resource, "running").await;
            let attached_name = format!("{}-attached", row.source_resource);
            create_disk(
                emulator,
                &attached_name,
                row.source_zone,
                attached_gb,
                &[],
            )
            .await;
            wait_for_disk_state(emulator, case, &attached_name, "available").await;
            emulator
                .ok(
                    "AttachDisk",
                    json!({
                        "diskName": attached_name,
                        "instanceName": row.source_resource,
                        "diskPath": attached_path,
                    }),
                )
                .await;
            emulator
                .ok(
                    "EnableAddOn",
                    json!({
                        "resourceName": row.source_resource,
                        "addOnRequest": auto_snapshot_request(None),
                    }),
                )
                .await;
            wait_for_success(emulator, case, row.source_resource).await;
            copy_body["sourceResourceName"] = json!(row.source_resource);
            copy_body["useLatestRestorableAutoSnapshot"] = json!(true);
            // The system volume is the plan's own SSD — the disk `GetInstance`
            // reports as the instance's system disk — so the expectation is the
            // service's own account of it.
            let system_disk = source["hardware"]["disks"][0].clone();
            assert_eq!(
                system_disk["isSystemDisk"].as_bool(),
                Some(true),
                "case {case}: expected the instance's first disk to be its system disk, got \
                 {system_disk:?}"
            );
            Expected {
                size_in_gb: system_disk["sizeInGb"].as_i64().unwrap_or_default(),
                from_disk: None,
                from_instance: Some(named(row.source_resource, &source)),
                from_image: Some((
                    source["blueprintId"].as_str().unwrap_or_default().to_string(),
                    source["bundleId"].as_str().unwrap_or_default().to_string(),
                )),
                from_attached_disks: vec![(attached_path.to_string(), attached_gb)],
                copy_body,
            }
        }
    }
}

/// The snapshotted volume's size, as the snapshot being copied reports it.
fn reported_size(case: &str, taken: &Value) -> i64 {
    let size = taken["sizeInGb"].as_i64().unwrap_or_default();
    assert!(
        size > 0,
        "case {case}: expected the snapshot to report the captured volume's size, got {taken:?}"
    );
    size
}

/// The blueprint and plan an instance snapshot reports.
fn image_of(case: &str, taken: &Value) -> (String, String) {
    let blueprint = taken["fromBlueprintId"].as_str().unwrap_or_default();
    let bundle = taken["fromBundleId"].as_str().unwrap_or_default();
    assert!(
        !blueprint.is_empty() && !bundle.is_empty(),
        "case {case}: expected the snapshot to report the source's image and plan, got {taken:?}"
    );
    (blueprint.to_string(), bundle.to_string())
}

/// The name/ARN pair of a stored resource, for the source members the copy must
/// carry.
fn named(name: &str, record: &Value) -> (String, String) {
    (
        name.to_string(),
        record["arn"].as_str().unwrap_or_default().to_string(),
    )
}

/// Poll `GetDisk` until the disk `name` reports `want`.
async fn wait_for_disk_state(emulator: &Emulator, case: &str, name: &str, want: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        let reported = disk(emulator, name).await;
        if reported["state"].as_str() == Some(want) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: expected {name} to be {want} within {WAIT:?}, got {:?}",
            reported["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// RULE: a copy is a new manual snapshot of the Region the request was made in
/// that holds what the snapshot it names holds — its kind, the snapshotted
/// volume's size and everything it says about the resource it was taken of —
/// with every server-assigned member (ARN, support code, creation time, state)
/// computed from the store's own allocators and the Region it was copied into.
#[tokio::test]
async fn a_copied_snapshot_holds_what_the_snapshot_it_was_copied_from_holds() {
    let emulator = Emulator::new();
    let mut arns: BTreeSet<String> = BTreeSet::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        let expected = prepare(&emulator, row).await;

        let answered = emulator
            .ok_at(
                None,
                Some(row.target_region),
                "CopySnapshot",
                expected.copy_body.clone(),
            )
            .await;

        // The copy answers ONE operation-log entry — it creates one snapshot —
        // against the copy it created, in the Region it was created in, and it
        // is not terminal yet because the copy is still being taken when the
        // call returns. It names the call that wrote it, as every entry of this
        // log does (the whole log's rule is pinned in
        // `operation_log_naming.rs`).
        let operations = answered["operations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {answered:?}"))
            .clone();
        assert_eq!(
            operations.len(),
            1,
            "case {case}: expected exactly one operation, got {operations:?}"
        );
        let operation = operations[0].clone();
        assert_eq!(
            operation["operationType"].as_str(),
            Some("CopySnapshot"),
            "case {case}: expected the entry to name the call that wrote it, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some(row.kind.resource_type()),
            "case {case}: expected the copied snapshot's own resource type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(row.target_name),
            "case {case}: expected the entry to name the copy, got {operation:?}"
        );
        assert_eq!(
            operation["status"].as_str(),
            Some(STARTED),
            "case {case}: expected a Started operation, got {operation:?}"
        );
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(false),
            "case {case}: expected a non-terminal operation, got {operation:?}"
        );
        assert_eq!(
            operation["location"]["regionName"].as_str(),
            Some(row.target_region),
            "case {case}: expected the entry in the Region the copy was made in, got \
             {operation:?}"
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

        // The stored copy, read back through the implemented reader.
        let copy = snapshot(&emulator, row.kind, row.target_name).await;
        assert_eq!(
            copy["name"].as_str(),
            Some(row.target_name),
            "case {case}: expected the target name the request carried, got {copy:?}"
        );
        assert_eq!(
            copy["resourceType"].as_str(),
            Some(row.kind.resource_type()),
            "case {case}: expected the copied snapshot's own resource type, got {copy:?}"
        );
        assert_eq!(
            copy["sizeInGb"].as_i64(),
            Some(expected.size_in_gb),
            "case {case}: expected the snapshotted volume's size, got {copy:?}"
        );
        assert_eq!(
            copy["location"]["regionName"].as_str(),
            Some(row.target_region),
            "case {case}: expected the copy in the Region the request was made in, got {:?}",
            copy["location"]
        );
        // Within one Region the copy holds the same volume in the same zone; a
        // copy into another Region names no zone of the Region it came from.
        if row.source_region == row.target_region {
            assert_eq!(
                copy["location"]["availabilityZone"].as_str(),
                Some(row.source_zone),
                "case {case}: expected the source's own zone, got {:?}",
                copy["location"]
            );
        } else {
            assert!(
                copy["location"]["availabilityZone"].is_null(),
                "case {case}: a copy into another Region names no zone of the source's, got \
                 {:?}",
                copy["location"]
            );
        }
        assert_eq!(
            copy["isFromAutoSnapshot"].as_bool(),
            Some(row.from_auto_snapshot),
            "case {case}: expected isFromAutoSnapshot to say whether an automatic snapshot was \
             copied, got {copy:?}"
        );
        // This input models no tags: a copy carries none until `TagResource`
        // gives it some.
        assert_eq!(
            copy["tags"].as_array().map(Vec::len),
            Some(0),
            "case {case}: expected a copy with no tags, got {:?}",
            copy["tags"]
        );

        // What the copy says about the resource the snapshot was taken of: the
        // source the snapshot itself described, and nothing about the other kind
        // of source it was not taken of.
        match &expected.from_disk {
            Some((name, arn)) => {
                assert_eq!(
                    copy["fromDiskName"].as_str(),
                    Some(name.as_str()),
                    "case {case}: expected the source disk's name, got {copy:?}"
                );
                assert_eq!(
                    copy["fromDiskArn"].as_str(),
                    Some(arn.as_str()),
                    "case {case}: expected the source disk's own ARN, got {copy:?}"
                );
            }
            None => assert!(
                copy["fromDiskName"].is_null() && copy["fromDiskArn"].is_null(),
                "case {case}: a copy of a snapshot of no disk names none, got {copy:?}"
            ),
        }
        match &expected.from_instance {
            Some((name, arn)) => {
                assert_eq!(
                    copy["fromInstanceName"].as_str(),
                    Some(name.as_str()),
                    "case {case}: expected the source instance's name, got {copy:?}"
                );
                assert_eq!(
                    copy["fromInstanceArn"].as_str(),
                    Some(arn.as_str()),
                    "case {case}: expected the source instance's own ARN, got {copy:?}"
                );
            }
            None => assert!(
                copy["fromInstanceName"].is_null() && copy["fromInstanceArn"].is_null(),
                "case {case}: a copy of a snapshot of no instance names none, got {copy:?}"
            ),
        }
        if let Some((blueprint, bundle)) = &expected.from_image {
            assert_eq!(
                copy["fromBlueprintId"].as_str(),
                Some(blueprint.as_str()),
                "case {case}: expected the snapshotted instance's image, got {copy:?}"
            );
            assert_eq!(
                copy["fromBundleId"].as_str(),
                Some(bundle.as_str()),
                "case {case}: expected the snapshotted instance's plan, got {copy:?}"
            );
        }
        if row.kind == Kind::InstanceSnapshot {
            let captured: Vec<(String, i64)> = copy["fromAttachedDisks"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|captured| {
                    (
                        captured["path"].as_str().unwrap_or_default().to_string(),
                        captured["sizeInGb"].as_i64().unwrap_or_default(),
                    )
                })
                .collect();
            assert_eq!(
                captured, expected.from_attached_disks,
                "case {case}: expected the block storage disks the snapshot captured, got {:?}",
                copy["fromAttachedDisks"]
            );
        }

        // The ARN carries the Region the copy was made in and the canonical
        // emulator account, and ends in a uuid no other snapshot is using; the
        // support code names the same id.
        let arn = copy["arn"].as_str().unwrap_or_default().to_string();
        let prefix = format!(
            "arn:aws:lightsail:{}:{ACCOUNT}:{}/",
            row.target_region,
            row.kind.resource_type()
        );
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
            copy["supportCode"].as_str(),
            Some(format!("{ACCOUNT}/{id}").as_str()),
            "case {case}: expected the account/resource-id support code, got {:?}",
            copy["supportCode"]
        );
        assert!(
            copy["createdAt"].as_f64().unwrap_or_default() > 0.0,
            "case {case}: expected a creation timestamp, got {:?}",
            copy["createdAt"]
        );

        // The copy is not restorable the instant it is asked for: the CLOCK
        // settles it, without anyone asking more than once.
        let state = copy["state"].as_str().unwrap_or_default().to_string();
        assert!(
            state == row.kind.being_taken() || state == row.kind.settled(),
            "case {case}: expected a snapshot being taken or settled, got {state:?}"
        );
        let settled =
            wait_for_snapshot_state(&emulator, row.kind, row.target_name, row.kind.settled())
                .await;
        assert_eq!(
            settled["sizeInGb"].as_i64(),
            Some(expected.size_in_gb),
            "case {case}: a settled copy holds the same volume, got {settled:?}"
        );

        // Copying takes nothing from the snapshot it copied: a manual source is
        // still the snapshot it was, in its own Region, and an automatic one is
        // still held by the resource it was taken of.
        match row.source_snapshot {
            Some(source_snapshot) => {
                let source = snapshot(&emulator, row.kind, source_snapshot).await;
                assert_eq!(
                    source["location"]["regionName"].as_str(),
                    Some(row.source_region),
                    "case {case}: a copy leaves the source snapshot in its own Region, got \
                     {source:?}"
                );
                assert_eq!(
                    source["sizeInGb"].as_i64(),
                    Some(expected.size_in_gb),
                    "case {case}: a copy leaves the source snapshot as it was, got {source:?}"
                );
                assert_eq!(
                    source["isFromAutoSnapshot"].as_bool(),
                    Some(false),
                    "case {case}: a manual source snapshot stays a manual one, got {source:?}"
                );
            }
            None => {
                let held = copies(&emulator, row.source_resource).await;
                assert_eq!(
                    held.len(),
                    1,
                    "case {case}: a copy consumes no automatic snapshot, got {held:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The refusals
// ---------------------------------------------------------------------------

/// The Region every source in the refusal cases lives in.
const REFUSAL_REGION: &str = "us-east-1";
/// See [`REFUSAL_REGION`].
const REFUSAL_ZONE: &str = "us-east-1a";

/// RULE: a request that does not name exactly one copyable source, or names a
/// source the `sourceRegion` does not hold, or a target name already taken, is
/// refused with a modelled error and stores nothing.
#[tokio::test]
async fn a_copy_the_request_does_not_describe_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();

    // One completed manual disk snapshot, one instance that holds no automatic
    // snapshot, and the disk the snapshot was taken of.
    create_disk(&emulator, "refusal-disk", REFUSAL_ZONE, 8, &[]).await;
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({"diskSnapshotName": "refusal-snapshot", "diskName": "refusal-disk"}),
        )
        .await;
    wait_for_snapshot_state(&emulator, Kind::DiskSnapshot, "refusal-snapshot", "completed").await;
    create_instance(&emulator, "refusal-instance", REFUSAL_ZONE, "nano_3_0").await;

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "no source at all",
            json!({"targetSnapshotName": "copy-1", "sourceRegion": REFUSAL_REGION}),
            "InvalidInputException",
        ),
        (
            "both a manual and an automatic source",
            json!({
                "targetSnapshotName": "copy-2",
                "sourceRegion": REFUSAL_REGION,
                "sourceSnapshotName": "refusal-snapshot",
                "sourceResourceName": "refusal-disk",
                "useLatestRestorableAutoSnapshot": true,
            }),
            "InvalidInputException",
        ),
        (
            "a restore date beside a manual source",
            json!({
                "targetSnapshotName": "copy-3",
                "sourceRegion": REFUSAL_REGION,
                "sourceSnapshotName": "refusal-snapshot",
                "restoreDate": "2026-10-24",
            }),
            "InvalidInputException",
        ),
        (
            "a restore date and the latest restorable copy at once",
            json!({
                "targetSnapshotName": "copy-4",
                "sourceRegion": REFUSAL_REGION,
                "sourceResourceName": "refusal-disk",
                "restoreDate": "2026-10-24",
                "useLatestRestorableAutoSnapshot": true,
            }),
            "InvalidInputException",
        ),
        (
            "an automatic source naming neither the date nor the latest copy",
            json!({
                "targetSnapshotName": "copy-5",
                "sourceRegion": REFUSAL_REGION,
                "sourceResourceName": "refusal-disk",
            }),
            "InvalidInputException",
        ),
        (
            "a restore date that is not a YYYY-MM-DD date",
            json!({
                "targetSnapshotName": "copy-6",
                "sourceRegion": REFUSAL_REGION,
                "sourceResourceName": "refusal-disk",
                "restoreDate": "24-10-2026",
            }),
            "InvalidInputException",
        ),
        (
            "a manual snapshot the account does not hold",
            json!({
                "targetSnapshotName": "copy-7",
                "sourceRegion": REFUSAL_REGION,
                "sourceSnapshotName": "no-such-snapshot",
            }),
            "NotFoundException",
        ),
        (
            "a manual snapshot of another Region",
            json!({
                "targetSnapshotName": "copy-8",
                "sourceRegion": "eu-west-1",
                "sourceSnapshotName": "refusal-snapshot",
            }),
            "NotFoundException",
        ),
        (
            "a source resource the account does not hold",
            json!({
                "targetSnapshotName": "copy-9",
                "sourceRegion": REFUSAL_REGION,
                "sourceResourceName": "no-such-resource",
                "useLatestRestorableAutoSnapshot": true,
            }),
            "NotFoundException",
        ),
        (
            "a resource that holds no automatic snapshot",
            json!({
                "targetSnapshotName": "copy-10",
                "sourceRegion": REFUSAL_REGION,
                "sourceResourceName": "refusal-instance",
                "useLatestRestorableAutoSnapshot": true,
            }),
            "NotFoundException",
        ),
        (
            "a date the resource holds no automatic snapshot for",
            json!({
                "targetSnapshotName": "copy-11",
                "sourceRegion": REFUSAL_REGION,
                "sourceResourceName": "refusal-disk",
                "restoreDate": "2020-01-01",
            }),
            "NotFoundException",
        ),
        (
            "a target name a snapshot already has",
            json!({
                "targetSnapshotName": "refusal-snapshot",
                "sourceRegion": REFUSAL_REGION,
                "sourceSnapshotName": "refusal-snapshot",
            }),
            "InvalidInputException",
        ),
        (
            "a sourceRegion outside the Regions a stored resource can live in",
            json!({
                "targetSnapshotName": "copy-12",
                "sourceRegion": "mars-central-1",
                "sourceSnapshotName": "refusal-snapshot",
            }),
            // `sourceRegion` targets the modelled `RegionName` enum, so the
            // centralized constraint walk refuses a value outside it before the
            // handler runs.
            "ValidationException",
        ),
    ];

    for (case, body, want_code) in cases {
        let target = body["targetSnapshotName"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (status, answered) = emulator.call("CopySnapshot", body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            want_code,
            "case {case:?}: expected the modelled {want_code}, got {answered:?}"
        );
        assert!(
            answered["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: expected a message saying what was wrong, got {answered:?}"
        );
        // A refused copy stores nothing: the name it asked for holds no snapshot
        // of either kind — except where the case is that the name was already
        // taken, where what was there is untouched.
        if target != "refusal-snapshot" {
            for kind in [Kind::DiskSnapshot, Kind::InstanceSnapshot] {
                let (operation, member) = kind.reader();
                let (read_status, read) = emulator.call(operation, json!({member: target})).await;
                assert_eq!(
                    read_status, 400,
                    "case {case:?}: a refused copy stores no {}, got {read_status} {read:?}",
                    kind.resource_type()
                );
            }
        }
    }

    let untouched = snapshot(&emulator, Kind::DiskSnapshot, "refusal-snapshot").await;
    assert_eq!(
        untouched["fromDiskName"].as_str(),
        Some("refusal-disk"),
        "case refused copies: the source snapshot is left as it was, got {untouched:?}"
    );
}

/// RULE: a snapshot holds a complete copy only once the service has finished
/// taking it, so a copy whose source has not reached its settled state is
/// refused with the modelled `OperationFailureException` and stores nothing; the
/// very same request is served once the clock settles the source.
#[tokio::test]
async fn a_snapshot_still_being_taken_cannot_be_copied() {
    let emulator = Emulator::new();
    let case = "a disk snapshot still being taken";

    create_disk(&emulator, "pending-source-disk", REFUSAL_ZONE, 8, &[]).await;
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({
                "diskSnapshotName": "pending-source-snapshot",
                "diskName": "pending-source-disk",
            }),
        )
        .await;

    // The precondition this case is about, read from the service's own record
    // rather than assumed: the capture has not finished.
    let taking = snapshot(&emulator, Kind::DiskSnapshot, "pending-source-snapshot").await;
    assert_eq!(
        taking["state"].as_str(),
        Some(Kind::DiskSnapshot.being_taken()),
        "case {case}: expected a snapshot still being taken, got {taking:?}"
    );

    let body = json!({
        "sourceRegion": REFUSAL_REGION,
        "sourceSnapshotName": "pending-source-snapshot",
        "targetSnapshotName": "pending-source-snapshot-copy",
    });
    let (status, answered) = emulator.call("CopySnapshot", body.clone()).await;
    assert_eq!(
        status, 400,
        "case {case}: expected a 400 refusal of a copy of an unfinished snapshot, got {status} \
         {answered:?}"
    );
    assert_eq!(
        error_code(&answered),
        "OperationFailureException",
        "case {case}: expected the modelled OperationFailureException, got {answered:?}"
    );
    let message = answered["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("pending-source-snapshot")
            && message.contains(Kind::DiskSnapshot.being_taken()),
        "case {case}: expected the refusal to name the snapshot and the state it is in, got \
         {answered:?}"
    );
    let (read_status, read) = emulator
        .call(
            "GetDiskSnapshot",
            json!({"diskSnapshotName": "pending-source-snapshot-copy"}),
        )
        .await;
    assert_eq!(
        read_status, 400,
        "case {case}: a refused copy stores no snapshot, got {read_status} {read:?}"
    );

    // Only the source's state stood in the way: the clock settles it, and then
    // the very same request is served.
    wait_for_snapshot_state(
        &emulator,
        Kind::DiskSnapshot,
        "pending-source-snapshot",
        Kind::DiskSnapshot.settled(),
    )
    .await;
    emulator.ok("CopySnapshot", body).await;
    let copy = snapshot(&emulator, Kind::DiskSnapshot, "pending-source-snapshot-copy").await;
    assert_eq!(
        copy["fromDiskName"].as_str(),
        Some("pending-source-disk"),
        "case {case}: expected the copy to describe the volume the snapshot was taken of, got \
         {copy:?}"
    );
}
