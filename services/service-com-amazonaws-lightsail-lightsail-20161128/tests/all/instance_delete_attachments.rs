// SPDX-License-Identifier: CC0-1.0

//! What `DeleteInstance` does to the attachments other resources hold to the
//! instance it removes, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the sibling suites' own
//! ([`crate::instance_lifecycle::Emulator`]), and every attachment a case
//! releases was made with `AttachDisk` — no case reaches into the store.
//!
//! The rule under test: removing a resource releases every attachment another
//! stored resource holds to it, so no read reports a resource attached to one
//! the account no longer holds. An attachment names "the resources to which"
//! a resource "is attached" (the modelled `Disk$attachedTo`), and a removed
//! instance is none of them — so each disk the deleted instance held returns to
//! the state it has while it is attached to nothing: `available`, the modelled
//! `DiskState` member for a disk that exists and is attached to nothing, with no
//! `attachedTo` and no `path`, free to be attached again with no further call.
//! Each release is logged like any other mutation, as the held resource's own
//! `DetachDisk` entry beside the `DeleteInstance` one, and an attachment to any
//! OTHER instance is left exactly as it was.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The blueprint and bundle every instance in this suite is created from: the
/// smallest published Linux plan (`src/state.rs`, `BUNDLE_SPECS`). Which plan it
/// is changes nothing about what a delete releases, so one is enough here.
const BLUEPRINT_ID: &str = "amazon_linux_2023";
/// See [`BLUEPRINT_ID`].
const BUNDLE_ID: &str = "nano_3_0";

/// The wire error a Lightsail request for a resource the account does not hold
/// is refused with, and the `code` its body carries (pinned for every operation
/// in `instance_error_envelope.rs`).
const NOT_FOUND: &str = "NotFoundException";
/// See [`NOT_FOUND`].
const DOES_NOT_EXIST: &str = "DoesNotExist";

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

/// Every block storage disk name the instance `name` reports in its hardware —
/// its system disk excluded, since that one is not a `Disk` resource.
async fn attached_disk_names(emulator: &Emulator, name: &str) -> Vec<String> {
    instance(emulator, name).await["hardware"]["disks"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|disk| disk["isSystemDisk"].as_bool() != Some(true))
        .filter_map(|disk| disk["name"].as_str().map(str::to_string))
        .collect()
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
        let instance = instance(emulator, name).await;
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
/// make it `available` — the state a disk has to be in to be attached.
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

/// Attach `disk_name` to `instance_name` at `path`, over the wire.
async fn attach(emulator: &Emulator, disk_name: &str, instance_name: &str, path: &str) {
    emulator
        .ok(
            "AttachDisk",
            json!({
                "diskName": disk_name,
                "instanceName": instance_name,
                "diskPath": path,
            }),
        )
        .await;
}

/// Stop `name` and wait until it reports `stopped`.
async fn stop_instance(emulator: &Emulator, name: &str) {
    emulator
        .ok("StopInstance", json!({"instanceName": name}))
        .await;
    wait_for_instance_state(emulator, name, "stopped").await;
}

/// The `operations` array of a response, as a list of entries.
fn operations(case: &str, body: &Value) -> Vec<Value> {
    body["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {body:?}"))
        .clone()
}

/// Assert the disk `name` is attached to nothing: the state the rule says a
/// released disk is in, with every other member it holds untouched.
async fn assert_released(case: &str, emulator: &Emulator, name: &str, size_in_gb: i64, zone: &str) {
    let released = disk(emulator, name).await;
    assert_eq!(
        (
            released["state"].as_str(),
            released["isAttached"].as_bool(),
            released["attachedTo"].as_str(),
            released["path"].as_str(),
            released["attachmentState"].as_str(),
        ),
        (Some("available"), Some(false), None, None, Some("detached")),
        "case {case}: expected disk {name} attached to nothing, got {released:?}"
    );
    assert_eq!(
        (
            released["name"].as_str(),
            released["sizeInGb"].as_i64(),
            released["location"]["availabilityZone"].as_str(),
            released["isSystemDisk"].as_bool(),
        ),
        (Some(name), Some(size_in_gb), Some(zone), Some(false)),
        "case {case}: expected the release to change only the attachment, got {released:?}"
    );
}

/// Assert `status`/`body` is the refusal of a request for a resource the account
/// does not hold.
fn assert_does_not_exist(case: &str, status: u16, body: &Value) {
    assert_eq!(
        (status, error_code(body).as_str(), body["code"].as_str()),
        (400, NOT_FOUND, Some(DOES_NOT_EXIST)),
        "case {case}: expected the DoesNotExist envelope, got {status} {body:?}"
    );
}

// ---------------------------------------------------------------------------
// The release
// ---------------------------------------------------------------------------

/// One disk a case attaches: its name, its size in GB and the device path it is
/// mounted at.
type DiskSpec = (&'static str, i64, &'static str);

/// One instance whose deletion must release what it holds. The rows differ along
/// everything the rule generalises over: how many attachments the removed
/// resource holds (none, one, several), the Availability Zone and region the
/// resources live in, the disks' sizes and device paths, the form of their names,
/// and the state the instance is in when it is deleted.
struct Row {
    case: &'static str,
    instance_name: &'static str,
    availability_zone: &'static str,
    disks: &'static [DiskSpec],
    /// Whether the instance is stopped before being deleted. A delete is
    /// accepted either way, so the release cannot depend on it.
    stop_first: bool,
}

/// The table for the release.
const ROWS: &[Row] = &[
    Row {
        case: "one 8 GB disk at /dev/xvdf, deleted while running",
        instance_name: "my-instance",
        availability_zone: "us-east-1a",
        disks: &[("my-disk", 8, "/dev/xvdf")],
        stop_first: false,
    },
    Row {
        case: "two disks at two paths in another region, deleted while stopped",
        instance_name: "data_host_1",
        availability_zone: "eu-west-1c",
        disks: &[
            ("data_disk_512", 512, "/dev/xvdg"),
            ("logs-disk-2", 8, "/dev/xvdh"),
        ],
        stop_first: true,
    },
    Row {
        case: "a 16384 GB disk, the largest published size",
        instance_name: "big_host_1",
        availability_zone: "ap-southeast-2b",
        disks: &[("big-disk-16tb", 16 * 1024, "/dev/xvdf")],
        stop_first: false,
    },
    Row {
        case: "an instance holding nothing at all",
        instance_name: "bare-host",
        availability_zone: "us-west-2a",
        disks: &[],
        stop_first: false,
    },
];

/// RULE: deleting an instance releases every attachment the store holds to it —
/// each disk it held reports `available` with no `attachedTo` and no `path` — and
/// the response logs the removal plus one entry per release.
#[tokio::test]
async fn deleting_an_instance_releases_every_disk_it_held() {
    let emulator = Emulator::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        create_running_instance(&emulator, row.instance_name, row.availability_zone).await;
        for (disk_name, size_in_gb, path) in row.disks {
            create_available_disk(&emulator, disk_name, row.availability_zone, *size_in_gb).await;
            attach(&emulator, disk_name, row.instance_name, path).await;
        }
        if row.stop_first {
            stop_instance(&emulator, row.instance_name).await;
        }

        // What a client sees before the delete: every disk is held by this
        // instance, and the instance reports them all.
        for (disk_name, _, path) in row.disks {
            let attached = disk(&emulator, disk_name).await;
            assert_eq!(
                (
                    attached["state"].as_str(),
                    attached["isAttached"].as_bool(),
                    attached["attachedTo"].as_str(),
                    attached["path"].as_str(),
                ),
                (
                    Some("in-use"),
                    Some(true),
                    Some(row.instance_name),
                    Some(*path)
                ),
                "case {case}: expected disk {disk_name} to start out attached, got {attached:?}"
            );
        }
        let before: BTreeSet<String> = attached_disk_names(&emulator, row.instance_name)
            .await
            .into_iter()
            .collect();
        let expected_names: BTreeSet<String> = row
            .disks
            .iter()
            .map(|(disk_name, _, _)| (*disk_name).to_string())
            .collect();
        assert_eq!(
            before, expected_names,
            "case {case}: expected the instance to report the disks it holds"
        );

        let deleted = emulator
            .ok("DeleteInstance", json!({"instanceName": row.instance_name}))
            .await;

        // The log: the removal itself, then one release per attachment — each
        // already done when the call returned, so each is terminal.
        let entries = operations(case, &deleted);
        assert_eq!(
            entries.len(),
            1 + row.disks.len(),
            "case {case}: expected one entry for the removal and one per release, got {entries:?}"
        );
        let removal = entries[0].clone();
        assert_eq!(
            (
                removal["operationType"].as_str(),
                removal["resourceType"].as_str(),
                removal["resourceName"].as_str(),
                removal["status"].as_str(),
                removal["isTerminal"].as_bool(),
                removal["location"]["availabilityZone"].as_str(),
            ),
            (
                Some("DeleteInstance"),
                Some("Instance"),
                Some(row.instance_name),
                Some("Succeeded"),
                Some(true),
                Some(row.availability_zone),
            ),
            "case {case}: expected a succeeded DeleteInstance entry, got {removal:?}"
        );
        let mut released_names: BTreeSet<String> = BTreeSet::new();
        for entry in &entries[1..] {
            assert_eq!(
                (
                    entry["operationType"].as_str(),
                    entry["resourceType"].as_str(),
                    entry["status"].as_str(),
                    entry["isTerminal"].as_bool(),
                    entry["location"]["availabilityZone"].as_str(),
                ),
                (
                    Some("DetachDisk"),
                    Some("Disk"),
                    Some("Succeeded"),
                    Some(true),
                    Some(row.availability_zone),
                ),
                "case {case}: expected a succeeded DetachDisk entry, got {entry:?}"
            );
            released_names.insert(
                entry["resourceName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        assert_eq!(
            released_names, expected_names,
            "case {case}: expected one release logged per disk the instance held"
        );
        for entry in &entries {
            let id = entry["id"].as_str().unwrap_or_default().to_string();
            assert!(
                is_uuid_shaped(&id),
                "case {case}: expected a uuid-shaped operation id, got {id:?}"
            );
            assert!(
                operation_ids.insert(id.clone()),
                "case {case}: expected a fresh operation id, got the already-used {id:?}"
            );
        }

        // The instance is gone from every read...
        let (status, body) = emulator
            .call("GetInstance", json!({"instanceName": row.instance_name}))
            .await;
        assert_does_not_exist(case, status, &body);

        // ...and so is every attachment that named it.
        for (disk_name, size_in_gb, _) in row.disks {
            assert_released(
                case,
                &emulator,
                disk_name,
                *size_in_gb,
                row.availability_zone,
            )
            .await;
        }
    }
}

/// RULE (the same one, read back through a write): a released disk is attached
/// to nothing, so it can be attached again straight away — no `DetachDisk` in
/// between.
#[tokio::test]
async fn a_disk_released_by_a_delete_attaches_again_with_no_further_call() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        let Some((disk_name, size_in_gb, path)) = row.disks.first() else {
            continue;
        };
        let holder = format!("{}-first", row.instance_name);
        let successor = format!("{}-second", row.instance_name);

        create_running_instance(&emulator, &holder, row.availability_zone).await;
        create_available_disk(&emulator, disk_name, row.availability_zone, *size_in_gb).await;
        attach(&emulator, disk_name, &holder, path).await;
        emulator
            .ok("DeleteInstance", json!({"instanceName": holder}))
            .await;

        create_running_instance(&emulator, &successor, row.availability_zone).await;
        attach(&emulator, disk_name, &successor, path).await;
        let reattached = disk(&emulator, disk_name).await;
        assert_eq!(
            (
                reattached["state"].as_str(),
                reattached["isAttached"].as_bool(),
                reattached["attachedTo"].as_str(),
                reattached["path"].as_str(),
            ),
            (
                Some("in-use"),
                Some(true),
                Some(successor.as_str()),
                Some(*path)
            ),
            "case {case}: expected the released disk to attach again, got {reattached:?}"
        );
        let reported = attached_disk_names(&emulator, &successor).await;
        assert_eq!(
            reported,
            vec![(*disk_name).to_string()],
            "case {case}: expected the new holder to report the disk, got {reported:?}"
        );
    }
}

/// RULE: only the removed instance's attachments are released — an attachment
/// another instance holds is left exactly as it was.
#[tokio::test]
async fn deleting_one_instance_leaves_another_holders_attachment_alone() {
    let emulator = Emulator::new();
    // Both instances live in one zone, so nothing but the attachment itself
    // tells the two disks apart.
    let zone = "us-east-1a";
    let cases: &[(&str, &str, &str)] = &[
        ("the deleted holder", "doomed-host", "doomed-disk"),
        ("the surviving holder", "kept_host_1", "kept-disk-2"),
    ];

    for (_, instance_name, disk_name) in cases {
        create_running_instance(&emulator, instance_name, zone).await;
        create_available_disk(&emulator, disk_name, zone, 8).await;
        attach(&emulator, disk_name, instance_name, "/dev/xvdf").await;
    }

    let (_, doomed_instance, doomed_disk) = cases[0];
    let (_, kept_instance, kept_disk) = cases[1];
    emulator
        .ok("DeleteInstance", json!({"instanceName": doomed_instance}))
        .await;

    assert_released("the deleted holder", &emulator, doomed_disk, 8, zone).await;

    let kept = disk(&emulator, kept_disk).await;
    assert_eq!(
        (
            kept["state"].as_str(),
            kept["isAttached"].as_bool(),
            kept["attachedTo"].as_str(),
            kept["path"].as_str(),
        ),
        (
            Some("in-use"),
            Some(true),
            Some(kept_instance),
            Some("/dev/xvdf")
        ),
        "case the surviving holder: expected its attachment untouched, got {kept:?}"
    );
    let reported = attached_disk_names(&emulator, kept_instance).await;
    assert_eq!(
        reported,
        vec![kept_disk.to_string()],
        "case the surviving holder: expected it to still report its disk, got {reported:?}"
    );
}

/// RULE: the release is complete when the delete returns, so a detachment the
/// clock still owed the same disk has nothing left to do — the disk stays
/// `available` and attached to nothing once that instant has passed.
#[tokio::test]
async fn a_delete_during_a_detach_in_flight_settles_the_disk_once() {
    let emulator = Emulator::new();
    let case = "a delete while a detach is in flight";
    let zone = "us-east-1a";
    create_running_instance(&emulator, "racing-host", zone).await;
    create_available_disk(&emulator, "racing-disk", zone, 32).await;
    attach(&emulator, "racing-disk", "racing-host", "/dev/xvdf").await;
    stop_instance(&emulator, "racing-host").await;

    // A detach the clock owes, started but not yet due.
    emulator
        .ok("DetachDisk", json!({"diskName": "racing-disk"}))
        .await;
    emulator
        .ok("DeleteInstance", json!({"instanceName": "racing-host"}))
        .await;

    // The delete released the attachment there and then, before the detach the
    // clock owed was due.
    assert_released(case, &emulator, "racing-disk", 32, zone).await;

    // And once that instant has passed, the disk is still exactly where the
    // release left it: the owed settle cannot undo or repeat it.
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_released(case, &emulator, "racing-disk", 32, zone).await;
}
