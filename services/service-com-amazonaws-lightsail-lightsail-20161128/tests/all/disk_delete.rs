// SPDX-License-Identifier: CC0-1.0

//! `DeleteDisk` — removing a Lightsail block storage disk from the account —
//! driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the sibling suites' own
//! ([`crate::instance_lifecycle::Emulator`]), and every disk a case deletes was
//! created with `CreateDisk` — no case reaches into the store.
//!
//! The rule under test: `DeleteDisk` removes the named disk from the account's
//! stored disks, and is accepted ONLY while that disk is attached to nothing
//! ("The disk must be in the available state (not attached to a Lightsail
//! instance)", the modelled `DeleteDisk` documentation), answering one terminal
//! operation-log entry located where the disk was and leaving its name free for
//! a new disk; a disk in any other `DiskState` is refused with the modelled
//! `OperationFailureException` (HTTP 400) and nothing about it changes; a name
//! this account holds no disk under — including one whose disk this call already
//! deleted — is refused with the service's `DoesNotExist` envelope. What the
//! disk's deletion does NOT remove is another disk, or a snapshot taken of this
//! one: a snapshot keeps the source name, ARN and size it recorded.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The blueprint and bundle every instance in this suite is created from: the
/// smallest published Linux plan (`src/state.rs`, `BUNDLE_SPECS`). Which plan it
/// is changes nothing about a disk deletion, so one is enough here.
const BLUEPRINT_ID: &str = "amazon_linux_2023";
/// See [`BLUEPRINT_ID`].
const BUNDLE_ID: &str = "nano_3_0";

/// The stored disk `name`, as `GetDisk` reports it.
async fn disk(emulator: &Emulator, name: &str) -> Value {
    emulator.ok("GetDisk", json!({"diskName": name})).await["disk"].clone()
}

/// The `message` of a failure body.
fn message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or("<no message>")
        .to_string()
}

/// Poll `GetDisk` until it reports `want`, answering the disk.
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
            "case waiting for disk state '{want}' on {name}: expected it within 5s, got {:?}",
            disk["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Poll `GetInstance` until it reports `want`.
async fn wait_for_instance_state(emulator: &Emulator, name: &str, want: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let instance = emulator
            .ok("GetInstance", json!({"instanceName": name}))
            .await["instance"]
            .clone();
        if instance["state"]["name"].as_str() == Some(want) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for instance state '{want}' on {name}: expected it within 5s, got {:?}",
            instance["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Create the disk `name` in `zone` at `size_in_gb` GB and wait for the clock to
/// make it `available` — the state the modelled documentation requires of a disk
/// that is to be deleted.
async fn create_available_disk(emulator: &Emulator, name: &str, zone: &str, size_in_gb: i64) {
    emulator
        .ok(
            "CreateDisk",
            json!({"diskName": name, "availabilityZone": zone, "sizeInGb": size_in_gb}),
        )
        .await;
    wait_for_disk_state(emulator, name, "available").await;
}

/// Create the instance `name` in `zone` and wait until it is serving.
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

/// The one operation entry a `DeleteDisk` answer carries, asserted to be a
/// terminal `DeleteDisk` entry for `disk_name` in `zone`, and answered so the
/// caller can check its id.
fn single_delete_operation(case: &str, answered: &Value, disk_name: &str, zone: &str) -> Value {
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
        (
            operation["operationType"].as_str(),
            operation["resourceType"].as_str(),
            operation["resourceName"].as_str(),
            operation["status"].as_str(),
            operation["isTerminal"].as_bool(),
        ),
        (
            Some("DeleteDisk"),
            Some("Disk"),
            Some(disk_name),
            Some("Succeeded"),
            Some(true),
        ),
        "case {case}: expected a finished DeleteDisk entry for this disk, got {operation:?}"
    );
    assert_eq!(
        operation["location"]["availabilityZone"].as_str(),
        Some(zone),
        "case {case}: expected the entry located where the disk was, got {operation:?}"
    );
    operation
}

// ---------------------------------------------------------------------------
// The accepted delete
// ---------------------------------------------------------------------------

/// One disk a delete must be accepted for. The rows differ along everything the
/// rule generalises over: the Availability Zone (and so the region the disk
/// lives in), the disk's size, the form of its name (hyphens, underscores,
/// digits), whether it carries tags and the automatic-snapshot add-on, and the
/// value of `forceDeleteAddOns` — absent, `false` and `true`.
struct Accepted {
    case: &'static str,
    disk_name: &'static str,
    availability_zone: &'static str,
    size_in_gb: i64,
    /// Whether the created disk carries a tag and the one add-on a disk can have
    /// (`src/state.rs`, `ADD_ON_TYPE_AUTO_SNAPSHOT`).
    with_add_on: bool,
    force_delete_add_ons: Option<bool>,
}

/// The table for the accepted delete.
const ACCEPTED: &[Accepted] = &[
    Accepted {
        case: "the documented 8 GB disk, no flag",
        disk_name: "my-disk",
        availability_zone: "us-east-1a",
        size_in_gb: 8,
        with_add_on: false,
        force_delete_add_ons: None,
    },
    Accepted {
        case: "a 512 GB disk in another region, forceDeleteAddOns false",
        disk_name: "data_disk_512",
        availability_zone: "eu-west-1c",
        size_in_gb: 512,
        with_add_on: false,
        force_delete_add_ons: Some(false),
    },
    Accepted {
        case: "a tagged disk carrying the automatic-snapshot add-on, forced",
        disk_name: "add-on-disk-1",
        availability_zone: "ap-southeast-2b",
        size_in_gb: 128,
        with_add_on: true,
        force_delete_add_ons: Some(true),
    },
    Accepted {
        case: "a 16384 GB disk, the largest published size",
        disk_name: "big_disk_16tb",
        availability_zone: "us-west-2a",
        size_in_gb: 16 * 1024,
        with_add_on: false,
        force_delete_add_ons: Some(true),
    },
];

/// RULE: a delete of a disk nothing holds is accepted, answers one terminal
/// operation-log entry located where the disk was, and the disk is gone from the
/// account's stored disks as the call returns — so the name is free for a new
/// disk.
#[tokio::test]
async fn deleting_an_available_disk_removes_it_and_frees_its_name() {
    let emulator = Emulator::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();

    for row in ACCEPTED {
        let case = row.case;
        let mut request = json!({
            "diskName": row.disk_name,
            "availabilityZone": row.availability_zone,
            "sizeInGb": row.size_in_gb,
        });
        if row.with_add_on {
            request["tags"] = json!([{"key": "stack", "value": "test"}]);
            request["addOns"] = json!([{
                "addOnType": "AutoSnapshot",
                "autoSnapshotAddOnRequest": {"snapshotTimeOfDay": "06:00"},
            }]);
        }
        emulator.ok("CreateDisk", request).await;
        let stored = wait_for_disk_state(&emulator, row.disk_name, "available").await;
        assert_eq!(
            stored["sizeInGb"].as_i64(),
            Some(row.size_in_gb),
            "case {case}: expected the disk stored at the requested size, got {stored:?}"
        );

        let mut delete = json!({"diskName": row.disk_name});
        if let Some(force) = row.force_delete_add_ons {
            delete["forceDeleteAddOns"] = json!(force);
        }
        let answered = emulator.ok("DeleteDisk", delete).await;
        let operation = single_delete_operation(
            case,
            &answered,
            row.disk_name,
            row.availability_zone,
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

        // The record is gone as the call returns: there is no deletion the clock
        // still owes, because the modelled `DiskState` has no value for a disk
        // being deleted.
        let (status, body) = emulator
            .call("GetDisk", json!({"diskName": row.disk_name}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str(), body["code"].as_str()),
            (400, "NotFoundException", Some("DoesNotExist")),
            "case {case}: expected the deleted disk to be gone, got {body:?}"
        );

        // ROUND TRIP: the name is free again, and what a new disk under it
        // reports is the new request's own, not the deleted disk's.
        let reused_size = row.size_in_gb.min(64) + 1;
        create_available_disk(
            &emulator,
            row.disk_name,
            row.availability_zone,
            reused_size,
        )
        .await;
        let recreated = disk(&emulator, row.disk_name).await;
        assert_eq!(
            (
                recreated["name"].as_str(),
                recreated["sizeInGb"].as_i64(),
                recreated["isAttached"].as_bool(),
            ),
            (Some(row.disk_name), Some(reused_size), Some(false)),
            "case {case}: expected a fresh disk under the freed name, got {recreated:?}"
        );
        assert_ne!(
            recreated["arn"].as_str(),
            stored["arn"].as_str(),
            "case {case}: expected the new disk its own ARN, got {recreated:?}"
        );
    }
}

/// RULE (idempotence has a limit): a delete removes the record, so repeating it
/// is refused with the `DoesNotExist` envelope rather than reporting a deletion
/// that did not happen.
#[tokio::test]
async fn deleting_the_same_disk_twice_is_refused_the_second_time() {
    let emulator = Emulator::new();
    create_available_disk(&emulator, "twice-disk", "us-east-2a", 32).await;

    emulator
        .ok("DeleteDisk", json!({"diskName": "twice-disk"}))
        .await;

    let (status, body) = emulator
        .call("DeleteDisk", json!({"diskName": "twice-disk"}))
        .await;
    assert_eq!(
        (status, error_code(&body).as_str(), body["code"].as_str()),
        (400, "NotFoundException", Some("DoesNotExist")),
        "case delete twice: expected the second call refused, got {body:?}"
    );
    assert_eq!(
        message(&body),
        "The Disk does not exist: twice-disk",
        "case delete twice: expected a message naming the resource type and the name, got \
         {body:?}"
    );
}

/// RULE (what the delete does NOT remove): another disk of the account, and a
/// snapshot taken of the deleted disk, are resources of their own — the snapshot
/// keeps the source name, ARN and size it recorded when it was taken.
#[tokio::test]
async fn deleting_a_disk_leaves_other_disks_and_its_snapshots_alone() {
    let emulator = Emulator::new();
    create_available_disk(&emulator, "source-disk", "us-east-1a", 64).await;
    create_available_disk(&emulator, "bystander-disk", "us-east-1a", 8).await;
    let source_arn = disk(&emulator, "source-disk").await["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({"diskName": "source-disk", "diskSnapshotName": "source-disk-snap"}),
        )
        .await;

    emulator
        .ok("DeleteDisk", json!({"diskName": "source-disk"}))
        .await;

    let snapshot = emulator
        .ok(
            "GetDiskSnapshot",
            json!({"diskSnapshotName": "source-disk-snap"}),
        )
        .await["diskSnapshot"]
        .clone();
    assert_eq!(
        (
            snapshot["fromDiskName"].as_str(),
            snapshot["fromDiskArn"].as_str(),
            snapshot["sizeInGb"].as_i64(),
        ),
        (Some("source-disk"), Some(source_arn.as_str()), Some(64)),
        "case snapshot of a deleted disk: expected it to outlive the disk, got {snapshot:?}"
    );

    let bystander = disk(&emulator, "bystander-disk").await;
    assert_eq!(
        (
            bystander["name"].as_str(),
            bystander["sizeInGb"].as_i64(),
            bystander["state"].as_str(),
        ),
        (Some("bystander-disk"), Some(8), Some("available")),
        "case another disk: expected it untouched by the delete, got {bystander:?}"
    );
}

// ---------------------------------------------------------------------------
// The refusals
// ---------------------------------------------------------------------------

/// One disk a delete must be refused for, and the `DiskState` value it carries
/// when the delete is attempted. The rows differ along the disk's state (a disk
/// the clock has not settled yet, and one an instance holds), its size, its zone
/// and the form of its name.
struct NotAvailable {
    case: &'static str,
    disk_name: &'static str,
    availability_zone: &'static str,
    size_in_gb: i64,
    /// The instance to attach the disk to before deleting it, when the row is
    /// about a disk in use.
    attach_to: Option<&'static str>,
    state: &'static str,
}

/// The table for a delete of a disk that is not `available`: the two
/// non-`available` `DiskState` values a stored disk of this emulator can be
/// found in (`src/state.rs`, `DISK_STATE_PENDING` and `DISK_STATE_IN_USE`).
const NOT_AVAILABLE: &[NotAvailable] = &[
    NotAvailable {
        case: "a disk still being provisioned",
        disk_name: "pending_disk_1",
        availability_zone: "eu-central-1b",
        size_in_gb: 128,
        attach_to: None,
        state: "pending",
    },
    NotAvailable {
        case: "a disk an instance has in use",
        disk_name: "in-use-disk",
        availability_zone: "us-west-2b",
        size_in_gb: 8,
        attach_to: Some("disk-holder"),
        state: "in-use",
    },
    NotAvailable {
        case: "a larger disk in another region, in use",
        disk_name: "in_use_disk_2048",
        availability_zone: "ap-northeast-1c",
        size_in_gb: 2048,
        attach_to: Some("disk_holder_2"),
        state: "in-use",
    },
];

/// RULE: a disk is deletable only while it is attached to nothing, so a delete of
/// a disk in any other state fails as an operation — HTTP 400
/// `OperationFailureException`, whose message names the instance still holding
/// the disk, or the disk's own state when nothing holds it — and the disk is left
/// exactly as it was. Once that disk IS `available`, the same call is accepted.
#[tokio::test]
async fn deleting_a_disk_that_is_not_available_is_refused_as_an_operation_failure() {
    let emulator = Emulator::new();

    for row in NOT_AVAILABLE {
        let case = row.case;
        if let Some(instance_name) = row.attach_to {
            create_running_instance(&emulator, instance_name, row.availability_zone).await;
            create_available_disk(
                &emulator,
                row.disk_name,
                row.availability_zone,
                row.size_in_gb,
            )
            .await;
            emulator
                .ok(
                    "AttachDisk",
                    json!({
                        "diskName": row.disk_name,
                        "instanceName": instance_name,
                        "diskPath": "/dev/xvdf",
                    }),
                )
                .await;
        } else {
            // Not waited on, so the disk is caught while the clock still owes it
            // `available`.
            emulator
                .ok(
                    "CreateDisk",
                    json!({
                        "diskName": row.disk_name,
                        "availabilityZone": row.availability_zone,
                        "sizeInGb": row.size_in_gb,
                    }),
                )
                .await;
        }
        // The disk really is in the state this row is about.
        let before = disk(&emulator, row.disk_name).await;
        assert_eq!(
            before["state"].as_str(),
            Some(row.state),
            "case {case}: expected the disk to be {}, got {before:?}",
            row.state
        );

        let (status, body) = emulator
            .call("DeleteDisk", json!({"diskName": row.disk_name}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "OperationFailureException"),
            "case {case}: expected the operation-failure rejection, got {body:?}"
        );
        let answered = message(&body);
        let expected = match row.attach_to {
            // A disk an instance holds is blocked by the ATTACHMENT, and the
            // refusal names the instance to detach it from.
            Some(instance_name) => format!(
                "You can't perform this operation because the disk is still attached to a \
                 Lightsail instance: {instance_name}."
            ),
            // A disk nothing holds is blocked by its state, and the refusal
            // reports that state.
            None => format!(
                "You can't delete this disk right now. The state of this disk is: {}.",
                row.state
            ),
        };
        assert_eq!(
            answered, expected,
            "case {case}: expected the refusal {expected:?}, got {answered:?}"
        );

        // The refusal removed nothing and changed nothing.
        let after = disk(&emulator, row.disk_name).await;
        assert_eq!(
            (
                after["name"].as_str(),
                after["sizeInGb"].as_i64(),
                after["attachedTo"].as_str(),
                after["location"]["availabilityZone"].as_str(),
            ),
            (
                Some(row.disk_name),
                Some(row.size_in_gb),
                before["attachedTo"].as_str(),
                Some(row.availability_zone),
            ),
            "case {case}: expected the refused delete to leave the disk alone, got {after:?}"
        );

        // And once the disk is attached to nothing, the same call is accepted —
        // the refusal was about the state, not about the request.
        if let Some(instance_name) = row.attach_to {
            emulator
                .ok("StopInstance", json!({"instanceName": instance_name}))
                .await;
            wait_for_instance_state(&emulator, instance_name, "stopped").await;
            emulator
                .ok("DetachDisk", json!({"diskName": row.disk_name}))
                .await;
        }
        wait_for_disk_state(&emulator, row.disk_name, "available").await;
        let answered = emulator
            .ok("DeleteDisk", json!({"diskName": row.disk_name}))
            .await;
        single_delete_operation(case, &answered, row.disk_name, row.availability_zone);
        let (status, body) = emulator
            .call("GetDisk", json!({"diskName": row.disk_name}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "NotFoundException"),
            "case {case}: expected the disk gone once it was deletable, got {body:?}"
        );
    }
}

/// RULE: a name this account holds no disk under is refused with the service's
/// `DoesNotExist` envelope — HTTP 400, wire error `NotFoundException`, body
/// `code` `DoesNotExist` — whatever else that name may address.
#[tokio::test]
async fn deleting_a_disk_this_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    // A name that addresses an INSTANCE, so the lookup is shown to be per
    // resource type rather than per name.
    create_running_instance(&emulator, "not-a-disk", "us-east-1a").await;
    create_available_disk(&emulator, "a-real-disk", "us-east-1a", 8).await;

    let cases: &[(&str, &str)] = &[
        ("a name never created", "no-such-disk"),
        ("an underscored name never created", "no_such_disk_2"),
        ("the name of an instance, not a disk", "not-a-disk"),
        ("a real disk's name with a suffix", "a-real-disk-2"),
    ];
    for (case, name) in cases {
        let (status, body) = emulator
            .call("DeleteDisk", json!({"diskName": name}))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case}: expected the not-found code, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected the DoesNotExist envelope, got {body:?}"
        );
        assert_eq!(
            message(&body),
            format!("The Disk does not exist: {name}"),
            "case {case}: expected a message naming the resource type and the name, got {body:?}"
        );
    }

    // The disk and the instance that do exist are untouched by any of those
    // refusals.
    let stored = disk(&emulator, "a-real-disk").await;
    assert_eq!(
        stored["state"].as_str(),
        Some("available"),
        "case refused names: expected the stored disk untouched, got {stored:?}"
    );
    let instance = emulator
        .ok("GetInstance", json!({"instanceName": "not-a-disk"}))
        .await["instance"]
        .clone();
    assert_eq!(
        instance["name"].as_str(),
        Some("not-a-disk"),
        "case refused names: expected the instance untouched, got {instance:?}"
    );
}
