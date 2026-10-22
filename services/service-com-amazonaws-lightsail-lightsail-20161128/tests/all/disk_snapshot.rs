// SPDX-License-Identifier: CC0-1.0

//! The Lightsail disk snapshot — create and read — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `CreateDiskSnapshot` persists ONE disk snapshot per
//! `diskSnapshotName` the request names — a point-in-time copy of the ONE source
//! the request names, which is either a block storage disk (`diskName`) or an
//! instance's system volume (`instanceName`), the two being mutually exclusive —
//! taking the snapshotted volume's size and location from the source record and
//! the tags from the request verbatim, with every server-assigned member (ARN,
//! support code, creation time, resource type, state, progress) computed from
//! them plus the store's own allocators; so a later `GetDiskSnapshot` reports
//! exactly that record. The snapshot is created `pending` and the clock, not a
//! reader, settles it into `completed`; a request that names no source, both
//! sources, a source the account does not hold or a snapshot name already taken
//! is refused and stores nothing; and a snapshot name that was never created is
//! refused with the service's `DoesNotExist` envelope.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The harness, the wire error code and the minted-id check this test target already
// exposes: a sibling suite drives the service through the same front door rather than
// standing up a second one.
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and
/// support code carries.
const ACCOUNT: &str = "000000000000";

/// The stored disk snapshot `name`, as `GetDiskSnapshot` reports it.
async fn snapshot(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok("GetDiskSnapshot", json!({"diskSnapshotName": name}))
        .await["diskSnapshot"]
        .clone()
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

// ---------------------------------------------------------------------------
// The create/read table
// ---------------------------------------------------------------------------

/// What one row snapshots: the two mutually exclusive sources the modelled
/// `CreateDiskSnapshot` input admits.
#[derive(Debug)]
enum Source {
    /// A block storage disk of this size in GB, created through `CreateDisk`.
    Disk { size_in_gb: i64 },
    /// An instance's system volume: the instance is created from this blueprint
    /// and bundle, and the snapshot's size is the size of the system disk
    /// `GetInstance` reports for it.
    Instance {
        blueprint_id: &'static str,
        bundle_id: &'static str,
    },
}

/// One `CreateDiskSnapshot` request the service must serve.
///
/// The rows differ along everything the rule generalises over: the KIND of
/// source (a block storage disk, an instance's system volume), the source
/// volume's size — including the smallest disk Lightsail creates and a large one
/// — the Availability Zone (and so the region in the ARN and the location), the
/// number of tags, and the form of the snapshot name (hyphens, underscores,
/// digits).
struct Row {
    case: &'static str,
    source_name: &'static str,
    snapshot_name: &'static str,
    availability_zone: &'static str,
    region: &'static str,
    source: Source,
    tags: &'static [(&'static str, &'static str)],
}

/// The table. The modelled documentation's own example — `my-disk-snapshot` of
/// the disk `Disk-Virginia-1` — is the first row.
const ROWS: &[Row] = &[
    Row {
        case: "the documented disk example, no tags",
        source_name: "Disk-Virginia-1",
        snapshot_name: "my-disk-snapshot",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        source: Source::Disk { size_in_gb: 32 },
        tags: &[],
    },
    Row {
        case: "the smallest disk Lightsail creates, in another region, one tag",
        source_name: "small-disk-8",
        snapshot_name: "small_disk_snapshot_1",
        availability_zone: "eu-west-1c",
        region: "eu-west-1",
        source: Source::Disk { size_in_gb: 8 },
        tags: &[("tier", "edge")],
    },
    Row {
        case: "a large disk, two tags",
        source_name: "data-disk-4096",
        snapshot_name: "data-disk-4096-backup",
        availability_zone: "ap-southeast-2b",
        region: "ap-southeast-2",
        source: Source::Disk { size_in_gb: 4096 },
        tags: &[("stack", "prod"), ("owner", "storage-team")],
    },
    Row {
        case: "an instance's system volume, the smallest bundle",
        source_name: "nano-instance",
        snapshot_name: "nano-system-volume-snapshot",
        availability_zone: "us-west-2a",
        region: "us-west-2",
        source: Source::Instance {
            blueprint_id: "amazon_linux_2023",
            bundle_id: "nano_3_0",
        },
        tags: &[],
    },
    Row {
        case: "an instance's system volume, a larger bundle, one tag",
        source_name: "medium_instance_2",
        snapshot_name: "medium-system-volume-snapshot",
        availability_zone: "ca-central-1a",
        region: "ca-central-1",
        source: Source::Instance {
            blueprint_id: "debian_12",
            bundle_id: "medium_3_0",
        },
        tags: &[("keep", "yes")],
    },
];

/// Create `row`'s source resource, answering the size in GB of the volume a
/// snapshot of it must report.
///
/// For a disk that is the size the request carried; for an instance it is the
/// size of the system disk the service itself reports through
/// `GetInstance`'s `hardware.disks`, so the expectation is the service's own
/// account of the volume rather than a figure copied into this test.
async fn create_source(emulator: &Emulator, row: &Row) -> i64 {
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
            size_in_gb
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
            let instance = instance(emulator, row.source_name).await;
            let system_disk = instance["hardware"]["disks"][0].clone();
            assert_eq!(
                system_disk["isSystemDisk"].as_bool(),
                Some(true),
                "case {}: expected the instance's first disk to be its system disk, got \
                 {system_disk:?}",
                row.case
            );
            system_disk["sizeInGb"].as_i64().unwrap_or_default()
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

/// RULE: every member `GetDiskSnapshot` reports comes from the one record
/// `CreateDiskSnapshot` stored — the caller's snapshot name and tags verbatim,
/// the source's own name, ARN, size and location, and every server-assigned
/// member computed from them plus the store's allocators.
#[tokio::test]
async fn a_disk_snapshot_reads_back_as_it_was_created() {
    let emulator = Emulator::new();
    let mut arns: BTreeSet<String> = BTreeSet::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        let name = row.snapshot_name;
        let want_size = create_source(&emulator, row).await;
        assert!(
            want_size > 0,
            "case {case}: expected the source volume to have a size, got {want_size}"
        );

        // The create answers ONE operation-log entry — `CreateDiskSnapshot`
        // creates one snapshot — and it is not terminal yet, because the copy is
        // still being taken when the call returns.
        let created = emulator.ok("CreateDiskSnapshot", create_body(row)).await;
        let operations = created["operations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {created:?}"))
            .clone();
        assert_eq!(
            operations.len(),
            1,
            "case {case}: expected exactly one operation, got {operations:?}"
        );
        let operation = operations[0].clone();
        assert_eq!(
            operation["operationType"].as_str(),
            Some("CreateDiskSnapshot"),
            "case {case}: expected the CreateDiskSnapshot operation type, got {operation:?}"
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
            Some(row.availability_zone),
            "case {case}: expected the operation to be located where the source is, got \
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

        // The stored record, read back through the implemented reader.
        let snapshot = snapshot(&emulator, name).await;
        assert_eq!(
            snapshot["name"].as_str(),
            Some(name),
            "case {case}: expected the snapshot name the request carried, got {snapshot:?}"
        );
        assert_eq!(
            snapshot["resourceType"].as_str(),
            Some("DiskSnapshot"),
            "case {case}: expected the DiskSnapshot resource type, got {snapshot:?}"
        );
        assert_eq!(
            snapshot["sizeInGb"].as_i64(),
            Some(want_size),
            "case {case}: expected the snapshotted volume's size, got {snapshot:?}"
        );
        assert_eq!(
            snapshot["location"]["availabilityZone"].as_str(),
            Some(row.availability_zone),
            "case {case}: expected the source's own zone, got {:?}",
            snapshot["location"]
        );
        assert_eq!(
            snapshot["location"]["regionName"].as_str(),
            Some(row.region),
            "case {case}: expected the source's own region, got {:?}",
            snapshot["location"]
        );
        assert_eq!(
            snapshot["isFromAutoSnapshot"].as_bool(),
            Some(false),
            "case {case}: an explicitly requested snapshot is not an automatic one, got \
             {snapshot:?}"
        );

        // The source the request named, and ONLY that source: the other pair of
        // members describes a source this snapshot was not taken of.
        match row.source {
            Source::Disk { .. } => {
                let disk = disk(&emulator, row.source_name).await;
                assert_eq!(
                    snapshot["fromDiskName"].as_str(),
                    Some(row.source_name),
                    "case {case}: expected the source disk's name, got {snapshot:?}"
                );
                assert_eq!(
                    snapshot["fromDiskArn"], disk["arn"],
                    "case {case}: expected the source disk's own ARN, got {:?}",
                    snapshot["fromDiskArn"]
                );
                assert!(
                    snapshot["fromInstanceName"].is_null()
                        && snapshot["fromInstanceArn"].is_null(),
                    "case {case}: a snapshot of a disk names no source instance, got {snapshot:?}"
                );
            }
            Source::Instance { .. } => {
                let instance = instance(&emulator, row.source_name).await;
                assert_eq!(
                    snapshot["fromInstanceName"].as_str(),
                    Some(row.source_name),
                    "case {case}: expected the source instance's name, got {snapshot:?}"
                );
                assert_eq!(
                    snapshot["fromInstanceArn"], instance["arn"],
                    "case {case}: expected the source instance's own ARN, got {:?}",
                    snapshot["fromInstanceArn"]
                );
                assert!(
                    snapshot["fromDiskName"].is_null() && snapshot["fromDiskArn"].is_null(),
                    "case {case}: a snapshot of a system volume names no source disk, got \
                     {snapshot:?}"
                );
            }
        }

        // The tags the request carried, verbatim and in order.
        let reported_tags: Vec<(String, String)> = snapshot["tags"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a tags array, got {snapshot:?}"))
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
            snapshot["tags"]
        );

        // The ARN carries the source's region and the canonical emulator
        // account, and ends in a uuid no other snapshot is using; the support
        // code names the same id.
        let arn = snapshot["arn"].as_str().unwrap_or_default().to_string();
        let prefix = format!("arn:aws:lightsail:{}:{ACCOUNT}:DiskSnapshot/", row.region);
        let id = arn
            .strip_prefix(&prefix)
            .unwrap_or_else(|| panic!("case {case}: expected an ARN starting '{prefix}', got {arn:?}"));
        assert!(
            is_uuid_shaped(id),
            "case {case}: expected a uuid-shaped ARN id, got {id:?}"
        );
        assert!(
            arns.insert(arn.clone()),
            "case {case}: expected a fresh ARN, got the already-used {arn:?}"
        );
        assert_eq!(
            snapshot["supportCode"].as_str(),
            Some(format!("{ACCOUNT}/{id}").as_str()),
            "case {case}: expected the account/resource-id support code, got {:?}",
            snapshot["supportCode"]
        );
        assert!(
            snapshot["createdAt"].as_f64().unwrap_or_default() > 0.0,
            "case {case}: expected a creation timestamp, got {:?}",
            snapshot["createdAt"]
        );

        // The snapshot is one of the modelled `DiskSnapshotState` values
        // throughout, and its progress follows that state.
        let state = snapshot["state"].as_str().unwrap_or_default().to_string();
        assert!(
            state == "pending" || state == "completed",
            "case {case}: expected a pending or completed snapshot, got {state:?}"
        );
        let want_progress = if state == "completed" { "100%" } else { "0%" };
        assert_eq!(
            snapshot["progress"].as_str(),
            Some(want_progress),
            "case {case}: expected the progress of a {state} snapshot, got {:?}",
            snapshot["progress"]
        );
    }
}

/// RULE: a snapshot is a point-in-time COPY, so one source can be snapshotted
/// as often as the caller likes and every snapshot is its own record, readable
/// afterwards with its own name, source and size.
#[tokio::test]
async fn each_snapshot_is_stored_independently() {
    let emulator = Emulator::new();
    for row in ROWS {
        create_source(&emulator, row).await;
        emulator.ok("CreateDiskSnapshot", create_body(row)).await;
    }

    // A second snapshot of the first row's source, under its own name.
    let second = "my-disk-snapshot-2";
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({"diskSnapshotName": second, "diskName": ROWS[0].source_name}),
        )
        .await;

    for row in ROWS {
        let snapshot = snapshot(&emulator, row.snapshot_name).await;
        assert_eq!(
            snapshot["name"].as_str(),
            Some(row.snapshot_name),
            "case {}: expected '{}' to still read back, got {snapshot:?}",
            row.case,
            row.snapshot_name
        );
        assert_eq!(
            snapshot["tags"].as_array().map(Vec::len),
            Some(row.tags.len()),
            "case {}: expected its own tags to be untouched, got {:?}",
            row.case,
            snapshot["tags"]
        );
    }

    let first = snapshot(&emulator, ROWS[0].snapshot_name).await;
    let again = snapshot(&emulator, second).await;
    assert_eq!(
        again["fromDiskName"], first["fromDiskName"],
        "case a second snapshot of one disk: expected the same source disk, got {again:?}"
    );
    assert_eq!(
        again["sizeInGb"], first["sizeInGb"],
        "case a second snapshot of one disk: expected the same volume size, got {again:?}"
    );
    assert_ne!(
        again["arn"], first["arn"],
        "case a second snapshot of one disk: expected its own ARN, got {again:?}"
    );
}

/// RULE: a created snapshot is `pending` when the call returns and the CLOCK
/// settles it into `completed`; a read reports the state, never moves it.
#[tokio::test]
async fn the_clock_settles_a_new_snapshot_into_completed() {
    let emulator = Emulator::new();
    let source = "awaited-disk";
    let name = "awaited-snapshot";
    emulator
        .ok(
            "CreateDisk",
            json!({"diskName": source, "availabilityZone": "us-east-2a", "sizeInGb": 32}),
        )
        .await;
    let created = emulator
        .ok(
            "CreateDiskSnapshot",
            json!({"diskSnapshotName": name, "diskName": source}),
        )
        .await;
    assert_eq!(
        created["operations"][0]["status"].as_str(),
        Some("Started"),
        "case straight after the create: expected a Started operation, got {created:?}"
    );

    let fresh = snapshot(&emulator, name).await;
    assert_eq!(
        fresh["state"].as_str(),
        Some("pending"),
        "case straight after create: expected pending, got {:?}",
        fresh["state"]
    );
    assert_eq!(
        fresh["progress"].as_str(),
        Some("0%"),
        "case straight after create: expected no progress yet, got {:?}",
        fresh["progress"]
    );

    let settled = wait_for_state(&emulator, name, "completed").await;
    assert_eq!(
        settled["progress"].as_str(),
        Some("100%"),
        "case after the clock settled it: expected a completed snapshot's progress, got \
         {settled:?}"
    );

    // Reading changes nothing a later caller sees: the same record, twice.
    let again = snapshot(&emulator, name).await;
    assert_eq!(
        again, settled,
        "case reading twice after it settled: expected the same record, got {again:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a snapshot is taken of exactly ONE source, so a request naming both
/// `diskName` and `instanceName` — or neither — is refused with the modelled
/// `InvalidInputException` (400) and stores nothing.
#[tokio::test]
async fn a_request_that_does_not_name_exactly_one_source_is_refused() {
    let cases: &[(&str, Value)] = &[
        (
            "both a disk and an instance",
            json!({
                "diskSnapshotName": "two-sources",
                "diskName": "a-disk",
                "instanceName": "an-instance",
            }),
        ),
        (
            "no source at all",
            json!({"diskSnapshotName": "two-sources"}),
        ),
        (
            "no source, but tags",
            json!({
                "diskSnapshotName": "two-sources",
                "tags": [{"key": "stack", "value": "prod"}],
            }),
        ),
    ];

    for (case, body) in cases {
        let emulator = Emulator::new();
        emulator
            .ok(
                "CreateDisk",
                json!({"diskName": "a-disk", "availabilityZone": "us-east-2a", "sizeInGb": 8}),
            )
            .await;
        emulator
            .ok(
                "CreateInstances",
                json!({
                    "instanceNames": ["an-instance"],
                    "availabilityZone": "us-east-2a",
                    "blueprintId": "amazon_linux_2023",
                    "bundleId": "nano_3_0",
                }),
            )
            .await;

        let (status, answered) = emulator.call("CreateDiskSnapshot", body.clone()).await;
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
            message.contains("diskName") && message.contains("instanceName"),
            "case {case}: expected the refusal to name both members, got {message:?}"
        );

        let (read_status, read_body) = emulator
            .call("GetDiskSnapshot", json!({"diskSnapshotName": "two-sources"}))
            .await;
        assert_eq!(
            read_status, 400,
            "case {case}: a refused create stores nothing, so the read must be refused too, \
             got {read_status} {read_body:?}"
        );
        assert_eq!(
            read_body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: a refused create stores nothing, so the read must answer code \
             DoesNotExist, got {read_body:?}"
        );
    }
}

/// RULE: the source must be a resource this account holds, so a well-formed
/// name that names none is refused with the service's `DoesNotExist` envelope —
/// naming the resource TYPE the request pointed at — and stores nothing.
#[tokio::test]
async fn a_source_the_account_does_not_hold_is_refused() {
    let cases: &[(&str, &str, &str, &str)] = &[
        ("an unknown disk", "diskName", "absent-disk", "Disk"),
        ("an unknown instance", "instanceName", "absent-instance", "Instance"),
        (
            "a disk name that is the instance's",
            "diskName",
            "present-instance",
            "Disk",
        ),
        (
            "an instance name that is the disk's",
            "instanceName",
            "present-disk",
            "Instance",
        ),
    ];

    for (case, member, source_name, resource_type) in cases {
        let emulator = Emulator::new();
        emulator
            .ok(
                "CreateDisk",
                json!({
                    "diskName": "present-disk",
                    "availabilityZone": "us-east-2a",
                    "sizeInGb": 8,
                }),
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

        let snapshot_name = "snapshot-of-nothing";
        let mut body = json!({"diskSnapshotName": snapshot_name});
        body[*member] = json!(source_name);
        let (status, answered) = emulator.call("CreateDiskSnapshot", body).await;
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
        assert_eq!(
            answered["message"].as_str(),
            Some(format!("The {resource_type} does not exist: {source_name}").as_str()),
            "case {case}: expected the resource-type/name message, got {answered:?}"
        );

        let (read_status, read_body) = emulator
            .call(
                "GetDiskSnapshot",
                json!({"diskSnapshotName": snapshot_name}),
            )
            .await;
        assert_eq!(
            read_status, 400,
            "case {case}: a refused create stores nothing, got {read_status} {read_body:?}"
        );
    }
}

/// RULE: a snapshot name addresses ONE snapshot, so creating one the account
/// already holds is refused and leaves the stored snapshot exactly as it was.
#[tokio::test]
async fn an_existing_snapshot_cannot_be_created_again() {
    let emulator = Emulator::new();
    let name = "only-once";
    for (disk, size) in [("first-disk", 8), ("second-disk", 64)] {
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
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({
                "diskSnapshotName": name,
                "diskName": "first-disk",
                "tags": [{"key": "keep", "value": "me"}],
            }),
        )
        .await;
    let before = wait_for_state(&emulator, name, "completed").await;

    let (status, body) = emulator
        .call(
            "CreateDiskSnapshot",
            json!({"diskSnapshotName": name, "diskName": "second-disk"}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case creating an existing snapshot: expected a 400 refusal, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case creating an existing snapshot: expected the modelled InvalidInputException, got \
         {body:?}"
    );

    let after = snapshot(&emulator, name).await;
    assert_eq!(
        after, before,
        "case creating an existing snapshot: expected the stored snapshot to be untouched, got \
         {after:?}"
    );
}

/// RULE: a well-formed name that names no stored snapshot is refused with the
/// service's `DoesNotExist` envelope — HTTP 400, `NotFoundException`, body code
/// `DoesNotExist` and a message naming the resource type and the requested name
/// — never a 501 and never a hollow 200.
#[tokio::test]
async fn an_unknown_snapshot_does_not_exist() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateDisk",
            json!({"diskName": "present-disk", "availabilityZone": "us-east-2a", "sizeInGb": 8}),
        )
        .await;
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({"diskSnapshotName": "present-snapshot", "diskName": "present-disk"}),
        )
        .await;

    for name in ["absent-snapshot", "present-snapshot-2", "other_snapshot"] {
        let (status, body) = emulator
            .call("GetDiskSnapshot", json!({"diskSnapshotName": name}))
            .await;
        assert_eq!(
            status, 400,
            "case reading {name}: expected a 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case reading {name}: expected the modelled NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case reading {name}: expected code DoesNotExist, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The DiskSnapshot does not exist: {name}").as_str()),
            "case reading {name}: expected the resource-type/name message, got {body:?}"
        );
    }
}
