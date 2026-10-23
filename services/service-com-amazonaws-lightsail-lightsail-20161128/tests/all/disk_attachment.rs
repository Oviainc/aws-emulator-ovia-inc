// SPDX-License-Identifier: CC0-1.0

//! Attaching a Lightsail block storage disk to an instance, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `AttachDisk` records on the ONE stored disk the request
//! names the instance it is attached to and the device path it is exposed at, so
//! every later read of that disk (`GetDisk`) and of that instance
//! (`GetInstance`, `GetInstances`) reports the attachment, and it answers the one
//! operation-log entry the attachment appended. An attach the two resources'
//! state does not allow — a disk that is already attached, a disk that is not
//! `available` yet, an instance that is neither running nor stopped, a disk and
//! an instance in different Availability Zones, or a device path the instance
//! already exposes — is refused with the modelled `OperationFailureException`
//! (HTTP 400) and attaches nothing; a disk or instance name the account does not
//! hold is refused with the service's `DoesNotExist` envelope.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The harness, the wire error code and the minted-id check this test target
// already exposes: a sibling suite drives the service through the same front door
// rather than standing up a second one.
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The device path an instance's own system disk occupies, as the service
/// reports it through `GetInstance`'s `hardware.disks` — so a path no block
/// storage disk can be attached at.
const SYSTEM_DISK_PATH: &str = "/dev/xvda";

/// The blueprint every instance in this suite is created from: one entry of the
/// published blueprint catalog `GetBlueprints` reports.
const BLUEPRINT_ID: &str = "amazon_linux_2023";

/// The bundle every instance in this suite is created on: one entry of the
/// published instance-plan catalog `GetBundles` reports.
const BUNDLE_ID: &str = "nano_3_0";

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

/// The disks `GetInstance` reports in the instance's hardware.
async fn instance_disks(emulator: &Emulator, name: &str) -> Vec<Value> {
    instance(emulator, name).await["hardware"]["disks"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Poll `GetDisk` until the disk reports `want`, answering the disk.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
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

/// Poll `GetInstance` until the instance reports `want`, answering the instance.
async fn wait_for_instance_state(emulator: &Emulator, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let instance = instance(emulator, name).await;
        if instance["state"]["name"].as_str() == Some(want) {
            return instance;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for instance '{want}' on {name}: expected it within 5s, got {:?}",
            instance["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Create the instance `name` in `zone` and wait until it is running.
async fn create_running_instance(emulator: &Emulator, name: &str, zone: &str) {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [name],
                "availabilityZone": zone,
                "blueprintId": BLUEPRINT_ID,
                "bundleId": BUNDLE_ID,
            }),
        )
        .await;
    wait_for_instance_state(emulator, name, "running").await;
}

/// Create the disk `name` of `size_in_gb` in `zone` and wait until it is
/// available.
async fn create_available_disk(emulator: &Emulator, name: &str, zone: &str, size_in_gb: i64) {
    emulator
        .ok(
            "CreateDisk",
            json!({
                "diskName": name,
                "availabilityZone": zone,
                "sizeInGb": size_in_gb,
            }),
        )
        .await;
    wait_for_disk_state(emulator, name, "available").await;
}

// ---------------------------------------------------------------------------
// The attach table
// ---------------------------------------------------------------------------

/// One `AttachDisk` request the service must serve.
///
/// The rows differ along everything the rule generalises over: the Availability
/// Zone (and so the region both resources live in), the size of the disk, the
/// device path it is exposed at, whether the instance is running or stopped when
/// the attach is made, the form of the two names (hyphens, underscores, digits),
/// and whether the request carries `autoMounting` at all.
struct Row {
    case: &'static str,
    instance_name: &'static str,
    disk_name: &'static str,
    availability_zone: &'static str,
    region: &'static str,
    size_in_gb: i64,
    disk_path: &'static str,
    /// Whether the instance is stopped (and waited for) before the attach: the
    /// operation's own documentation attaches "to a running or stopped Lightsail
    /// instance".
    stop_first: bool,
    /// The `autoMounting` member the request carries, when it carries one.
    auto_mounting: Option<bool>,
}

/// The table. The modelled documentation's own example — the disk `my-disk`
/// exposed at `/dev/xvdf` — is the first row.
const ROWS: &[Row] = &[
    Row {
        case: "the documented example: /dev/xvdf on a running instance",
        instance_name: "Instance-Ohio-1",
        disk_name: "my-disk",
        availability_zone: "us-east-2a",
        region: "us-east-2",
        size_in_gb: 32,
        disk_path: "/dev/xvdf",
        stop_first: false,
        auto_mounting: None,
    },
    Row {
        case: "the smallest disk, another region, a STOPPED instance, autoMounting false",
        instance_name: "stopped_instance_2",
        disk_name: "small-disk-8",
        availability_zone: "eu-west-1c",
        region: "eu-west-1",
        size_in_gb: 8,
        disk_path: "/dev/xvdg",
        stop_first: true,
        auto_mounting: Some(false),
    },
    Row {
        case: "a large disk at a /dev/sdf-style path, autoMounting true",
        instance_name: "data-instance-3",
        disk_name: "data_disk_512",
        availability_zone: "ap-southeast-2b",
        region: "ap-southeast-2",
        size_in_gb: 512,
        disk_path: "/dev/sdf",
        stop_first: false,
        auto_mounting: Some(true),
    },
    Row {
        case: "the last device letter Lightsail suggests, in a third region",
        instance_name: "WebServer4",
        disk_name: "mid-disk-2048",
        availability_zone: "us-west-2a",
        region: "us-west-2",
        size_in_gb: 2048,
        disk_path: "/dev/xvdp",
        stop_first: false,
        auto_mounting: None,
    },
];

/// The `AttachDisk` body for `row`.
fn attach_body(row: &Row) -> Value {
    let mut body = json!({
        "diskName": row.disk_name,
        "instanceName": row.instance_name,
        "diskPath": row.disk_path,
    });
    if let Some(auto_mounting) = row.auto_mounting {
        body["autoMounting"] = json!(auto_mounting);
    }
    body
}

/// RULE: `AttachDisk` records the attachment on the stored disk — the instance
/// it is attached to and the path it is exposed at — so the disk reads back
/// `in-use` and attached, the instance reports it in its hardware, and the call
/// answers the one operation-log entry the attachment appended.
#[tokio::test]
async fn attaching_a_disk_records_it_on_the_disk_and_on_the_instance() {
    let emulator = Emulator::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        create_running_instance(&emulator, row.instance_name, row.availability_zone).await;
        if row.stop_first {
            emulator
                .ok("StopInstance", json!({"instanceName": row.instance_name}))
                .await;
            wait_for_instance_state(&emulator, row.instance_name, "stopped").await;
        }
        create_available_disk(
            &emulator,
            row.disk_name,
            row.availability_zone,
            row.size_in_gb,
        )
        .await;

        // The attach answers ONE operation-log entry: it acts on exactly one
        // disk, and the entry is not terminal yet when the call returns.
        let attached = emulator.ok("AttachDisk", attach_body(row)).await;
        let operations = attached["operations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {attached:?}"))
            .clone();
        assert_eq!(
            operations.len(),
            1,
            "case {case}: expected exactly one operation, got {operations:?}"
        );
        let operation = operations[0].clone();
        assert_eq!(
            operation["operationType"].as_str(),
            Some("AttachDisk"),
            "case {case}: expected the AttachDisk operation type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some("Disk"),
            "case {case}: expected the Disk resource type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(row.disk_name),
            "case {case}: expected the operation to name the disk, got {operation:?}"
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
            "case {case}: expected the operation to be located where the disk is, got \
             {operation:?}"
        );
        assert_eq!(
            operation["location"]["regionName"].as_str(),
            Some(row.region),
            "case {case}: expected the disk's own region, got {operation:?}"
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

        // The attachment, as the DISK reports it.
        let disk = disk(&emulator, row.disk_name).await;
        assert_eq!(
            disk["isAttached"].as_bool(),
            Some(true),
            "case {case}: expected the disk to read back attached, got {disk:?}"
        );
        assert_eq!(
            disk["attachedTo"].as_str(),
            Some(row.instance_name),
            "case {case}: expected the disk to name the instance it was attached to, got \
             {disk:?}"
        );
        assert_eq!(
            disk["path"].as_str(),
            Some(row.disk_path),
            "case {case}: expected the device path the request carried, got {disk:?}"
        );
        assert_eq!(
            disk["state"].as_str(),
            Some("in-use"),
            "case {case}: expected an attached disk to be in-use, got {disk:?}"
        );
        assert_eq!(
            disk["attachmentState"].as_str(),
            Some("attached"),
            "case {case}: expected the discontinued attachmentState to mirror isAttached, got \
             {disk:?}"
        );
        // Nothing else about the disk moved: the attach records an attachment,
        // it does not re-create the disk.
        assert_eq!(
            disk["sizeInGb"].as_i64(),
            Some(row.size_in_gb),
            "case {case}: expected the size the disk was created with, got {disk:?}"
        );
        assert_eq!(
            disk["location"]["availabilityZone"].as_str(),
            Some(row.availability_zone),
            "case {case}: expected the disk to stay in its own zone, got {disk:?}"
        );
        assert_eq!(
            disk["isSystemDisk"].as_bool(),
            Some(false),
            "case {case}: an attached block storage disk is not a system disk, got {disk:?}"
        );
        // `autoMounting` applies only to Lightsail for Research resources, and
        // no instance this service holds is one, so no mount status is reported
        // however the request asked.
        assert!(
            disk["autoMountStatus"].is_null(),
            "case {case}: autoMountStatus applies only to Lightsail for Research, got {:?}",
            disk["autoMountStatus"]
        );

        // The same attachment, as the INSTANCE reports its hardware: the
        // bundle's system disk first, then the disk that was attached to it.
        let disks = instance_disks(&emulator, row.instance_name).await;
        assert_eq!(
            disks.len(),
            2,
            "case {case}: expected the system disk and the attached disk, got {disks:?}"
        );
        assert_eq!(
            disks[0]["isSystemDisk"].as_bool(),
            Some(true),
            "case {case}: expected the system disk first, got {disks:?}"
        );
        assert_eq!(
            disks[1]["name"].as_str(),
            Some(row.disk_name),
            "case {case}: expected the attached disk to be reported by name, got {disks:?}"
        );
        assert_eq!(
            disks[1]["path"].as_str(),
            Some(row.disk_path),
            "case {case}: expected the attached disk's own path, got {disks:?}"
        );
        assert_eq!(
            disks[1]["attachedTo"].as_str(),
            Some(row.instance_name),
            "case {case}: expected the attached disk to name this instance, got {disks:?}"
        );
        assert_eq!(
            disks[1]["sizeInGb"].as_i64(),
            Some(row.size_in_gb),
            "case {case}: expected the attached disk's own size, got {disks:?}"
        );
        assert_eq!(
            disks[1]["isSystemDisk"].as_bool(),
            Some(false),
            "case {case}: the attached disk is block storage, not a system disk, got {disks:?}"
        );
    }

    // The listing reports the same hardware as the single read: both render the
    // one stored record.
    let listed = emulator.ok("GetInstances", json!({})).await["instances"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for row in ROWS {
        let case = row.case;
        let listed = listed
            .iter()
            .find(|instance| instance["name"].as_str() == Some(row.instance_name))
            .unwrap_or_else(|| panic!("case {case}: expected {} to be listed", row.instance_name));
        let paths: Vec<String> = listed["hardware"]["disks"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|disk| disk["path"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            paths,
            vec![SYSTEM_DISK_PATH.to_string(), row.disk_path.to_string()],
            "case {case}: expected the listing to report the same two device paths, got {:?}",
            listed["hardware"]["disks"]
        );
    }
}

/// RULE: a disk is attached to at most one instance, so attaching one that is
/// already attached is refused — whatever instance or path the second request
/// names — and the first attachment stands.
#[tokio::test]
async fn an_already_attached_disk_cannot_be_attached_again() {
    let emulator = Emulator::new();
    let zone = "us-east-2a";
    create_running_instance(&emulator, "first-instance", zone).await;
    create_running_instance(&emulator, "second-instance", zone).await;
    create_available_disk(&emulator, "shared-disk", zone, 32).await;
    emulator
        .ok(
            "AttachDisk",
            json!({
                "diskName": "shared-disk",
                "instanceName": "first-instance",
                "diskPath": "/dev/xvdf",
            }),
        )
        .await;

    // Every way of asking again: the same instance at the same path, the same
    // instance at another path, and another instance.
    let retries: &[(&str, &str, &str)] = &[
        ("the same instance at the same path", "first-instance", "/dev/xvdf"),
        ("the same instance at another path", "first-instance", "/dev/xvdh"),
        ("another instance at the same path", "second-instance", "/dev/xvdf"),
        ("another instance at another path", "second-instance", "/dev/xvdh"),
    ];
    for (case, instance_name, disk_path) in retries {
        let (status, body) = emulator
            .call(
                "AttachDisk",
                json!({
                    "diskName": "shared-disk",
                    "instanceName": instance_name,
                    "diskPath": disk_path,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the attach to be refused, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "OperationFailureException",
            "case {case}: expected the modelled operation failure, got {body:?}"
        );

        // The first attachment is untouched: the refusal wrote nothing.
        let disk = disk(&emulator, "shared-disk").await;
        assert_eq!(
            disk["attachedTo"].as_str(),
            Some("first-instance"),
            "case {case}: expected the first attachment to stand, got {disk:?}"
        );
        assert_eq!(
            disk["path"].as_str(),
            Some("/dev/xvdf"),
            "case {case}: expected the first path to stand, got {disk:?}"
        );
        let other = instance_disks(&emulator, "second-instance").await;
        assert_eq!(
            other.len(),
            1,
            "case {case}: expected the other instance to hold only its system disk, got \
             {other:?}"
        );
    }
}

/// What one refused attach asks for, beyond the resources the case sets up.
struct Refusal {
    case: &'static str,
    disk_name: &'static str,
    instance_name: &'static str,
    disk_path: &'static str,
    /// The wire error code the refusal must carry.
    error_code: &'static str,
}

/// RULE: an attach the two resources' state does not allow is refused with the
/// modelled `OperationFailureException`, and one naming a resource the account
/// does not hold with the service's `DoesNotExist` envelope (`NotFoundException`
/// on the wire) — either way at HTTP 400, and either way attaching nothing.
#[tokio::test]
async fn an_attach_the_state_forbids_is_refused_and_attaches_nothing() {
    let emulator = Emulator::new();
    // One zone holds the instance and the disk the state cases use; a second
    // zone of the same region holds the disk the zone case uses.
    create_running_instance(&emulator, "host-instance", "us-east-2a").await;
    create_available_disk(&emulator, "free-disk", "us-east-2a", 32).await;
    create_available_disk(&emulator, "other-zone-disk", "us-east-2b", 32).await;
    // One disk already attached at /dev/xvdf, so a later attach at that path
    // finds it taken.
    create_available_disk(&emulator, "taken-path-disk", "us-east-2a", 8).await;
    emulator
        .ok(
            "AttachDisk",
            json!({
                "diskName": "taken-path-disk",
                "instanceName": "host-instance",
                "diskPath": "/dev/xvdf",
            }),
        )
        .await;

    let refusals: &[Refusal] = &[
        Refusal {
            case: "a disk name the account does not hold",
            disk_name: "no-such-disk",
            instance_name: "host-instance",
            disk_path: "/dev/xvdg",
            error_code: "NotFoundException",
        },
        Refusal {
            case: "an instance name the account does not hold",
            disk_name: "free-disk",
            instance_name: "no-such-instance",
            disk_path: "/dev/xvdg",
            error_code: "NotFoundException",
        },
        Refusal {
            case: "a disk in another Availability Zone",
            disk_name: "other-zone-disk",
            instance_name: "host-instance",
            disk_path: "/dev/xvdg",
            error_code: "OperationFailureException",
        },
        Refusal {
            case: "the path the instance's own system disk occupies",
            disk_name: "free-disk",
            instance_name: "host-instance",
            disk_path: SYSTEM_DISK_PATH,
            error_code: "OperationFailureException",
        },
        Refusal {
            case: "a path another attached disk already occupies",
            disk_name: "free-disk",
            instance_name: "host-instance",
            disk_path: "/dev/xvdf",
            error_code: "OperationFailureException",
        },
    ];

    for refusal in refusals {
        let case = refusal.case;
        let (status, body) = emulator
            .call(
                "AttachDisk",
                json!({
                    "diskName": refusal.disk_name,
                    "instanceName": refusal.instance_name,
                    "diskPath": refusal.disk_path,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the attach to be refused, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            refusal.error_code,
            "case {case}: expected {}, got {body:?}",
            refusal.error_code
        );
    }

    // Nothing above attached anything: the free disks are still free, and the
    // instance still exposes only its system disk and the one disk that was
    // attached in the setup.
    for name in ["free-disk", "other-zone-disk"] {
        let disk = disk(&emulator, name).await;
        assert_eq!(
            disk["isAttached"].as_bool(),
            Some(false),
            "case {name} after the refusals: expected it to be unattached, got {disk:?}"
        );
        assert!(
            disk["attachedTo"].is_null(),
            "case {name} after the refusals: expected no instance named, got {:?}",
            disk["attachedTo"]
        );
        assert!(
            disk["path"].is_null(),
            "case {name} after the refusals: expected no device path, got {:?}",
            disk["path"]
        );
        assert_eq!(
            disk["state"].as_str(),
            Some("available"),
            "case {name} after the refusals: expected it to stay available, got {disk:?}"
        );
    }
    let disks = instance_disks(&emulator, "host-instance").await;
    assert_eq!(
        disks.len(),
        2,
        "case the refused attaches: expected only the system disk and taken-path-disk, got \
         {disks:?}"
    );
}

/// RULE: only a disk that exists and is doing nothing can be attached, and only
/// to an instance that is running or stopped — so a resource the clock has not
/// settled yet is refused with the modelled `OperationFailureException`.
///
/// Both transient states are the ones the clock owes a freshly created resource,
/// which is why each case attaches BEFORE waiting for it to settle, and then
/// proves the same attach succeeds once the clock has run.
#[tokio::test]
async fn a_resource_the_clock_has_not_settled_cannot_be_attached() {
    let emulator = Emulator::new();
    let zone = "eu-west-1c";

    // A disk still being provisioned is `pending`: there is nothing to expose.
    create_running_instance(&emulator, "settled-instance", zone).await;
    emulator
        .ok(
            "CreateDisk",
            json!({"diskName": "fresh-disk", "availabilityZone": zone, "sizeInGb": 8}),
        )
        .await;
    let (status, body) = emulator
        .call(
            "AttachDisk",
            json!({
                "diskName": "fresh-disk",
                "instanceName": "settled-instance",
                "diskPath": "/dev/xvdf",
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case a pending disk: expected the attach to be refused, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "OperationFailureException",
        "case a pending disk: expected the modelled operation failure, got {body:?}"
    );

    // An instance still coming up is `pending`: it is neither running nor
    // stopped, so there is nothing to attach the disk to. The disk is settled
    // first, so the only state in the way is the instance's.
    wait_for_disk_state(&emulator, "fresh-disk", "available").await;
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["fresh-instance"],
                "availabilityZone": zone,
                "blueprintId": BLUEPRINT_ID,
                "bundleId": BUNDLE_ID,
            }),
        )
        .await;
    let (status, body) = emulator
        .call(
            "AttachDisk",
            json!({
                "diskName": "fresh-disk",
                "instanceName": "fresh-instance",
                "diskPath": "/dev/xvdf",
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case a pending instance: expected the attach to be refused, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "OperationFailureException",
        "case a pending instance: expected the modelled operation failure, got {body:?}"
    );

    // Once the clock has settled both, the very same attach is served.
    wait_for_instance_state(&emulator, "fresh-instance", "running").await;
    emulator
        .ok(
            "AttachDisk",
            json!({
                "diskName": "fresh-disk",
                "instanceName": "fresh-instance",
                "diskPath": "/dev/xvdf",
            }),
        )
        .await;
    let disk = disk(&emulator, "fresh-disk").await;
    assert_eq!(
        disk["attachedTo"].as_str(),
        Some("fresh-instance"),
        "case the settled attach: expected the attachment to be recorded, got {disk:?}"
    );
    assert_eq!(
        disk["state"].as_str(),
        Some("in-use"),
        "case the settled attach: expected the disk to be in-use, got {disk:?}"
    );
}

/// RULE: an instance exposes one device per path, so several disks attach to one
/// instance as long as each takes a path of its own — and each disk reports its
/// own attachment while the instance reports all of them.
#[tokio::test]
async fn several_disks_attach_to_one_instance_at_distinct_paths() {
    let emulator = Emulator::new();
    let zone = "ca-central-1a";
    create_running_instance(&emulator, "multi-disk-host", zone).await;

    // Three disks of different sizes at three different device paths. The disk
    // names are deliberately NOT in the order they are attached, so the order
    // the instance reports them in is the store's own.
    let attachments: &[(&str, i64, &str)] = &[
        ("volume-c", 8, "/dev/xvdh"),
        ("volume-a", 64, "/dev/xvdf"),
        ("volume-b", 1024, "/dev/sdg"),
    ];
    for (name, size_in_gb, path) in attachments {
        create_available_disk(&emulator, name, zone, *size_in_gb).await;
        emulator
            .ok(
                "AttachDisk",
                json!({
                    "diskName": name,
                    "instanceName": "multi-disk-host",
                    "diskPath": path,
                }),
            )
            .await;
        let disk = disk(&emulator, name).await;
        assert_eq!(
            disk["path"].as_str(),
            Some(*path),
            "case {name}: expected its own device path, got {disk:?}"
        );
        assert_eq!(
            disk["attachedTo"].as_str(),
            Some("multi-disk-host"),
            "case {name}: expected it to name the host instance, got {disk:?}"
        );
    }

    let disks = instance_disks(&emulator, "multi-disk-host").await;
    let reported: Vec<(Option<&str>, Option<i64>)> = disks
        .iter()
        .map(|disk| (disk["path"].as_str(), disk["sizeInGb"].as_i64()))
        .collect();
    assert_eq!(
        reported.len(),
        4,
        "case three attachments: expected the system disk and three attached disks, got \
         {reported:?}"
    );
    assert_eq!(
        reported[0].0,
        Some(SYSTEM_DISK_PATH),
        "case three attachments: expected the system disk first, got {reported:?}"
    );
    // The attached disks follow in the store's own total order — by disk name —
    // each carrying the path and size it was created and attached with.
    assert_eq!(
        reported[1..].to_vec(),
        vec![
            (Some("/dev/xvdf"), Some(64)),
            (Some("/dev/sdg"), Some(1024)),
            (Some("/dev/xvdh"), Some(8)),
        ],
        "case three attachments: expected volume-a, volume-b and volume-c in the store's \
         order, got {reported:?}"
    );
}
