// SPDX-License-Identifier: CC0-1.0

//! `DetachDisk` — taking a Lightsail block storage disk off the instance that
//! holds it — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the sibling suites' own
//! ([`crate::instance_lifecycle::Emulator`]), and the attachment each case
//! detaches is made with `AttachDisk` — no case reaches into the store.
//!
//! The rule under test: `DetachDisk` releases the attachment the named disk
//! holds — it is accepted ONLY while the disk is attached and the instance
//! holding it is stopped ("Detaches a stopped block storage disk from a
//! Lightsail instance", the modelled `DetachDisk` documentation), answering one
//! non-terminal operation-log entry and leaving the disk attached and `in-use`
//! until the clock settles it into `available` with no `attachedTo` and no
//! `path` (and so out of the holding instance's reported hardware); a disk no
//! instance holds, and a disk whose instance is in any state but `stopped`, are
//! refused with the modelled `OperationFailureException` (HTTP 400) and nothing
//! about the disk changes; a name this account holds no disk under is refused
//! with the service's `DoesNotExist` envelope.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::instance_lifecycle::{error_code, Emulator};

/// The blueprint and bundle every instance in this suite is created from: the
/// smallest published Linux plan (`src/state.rs`, `BUNDLE_SPECS`). Which plan it
/// is changes nothing about a detach, so one is enough here.
const BLUEPRINT_ID: &str = "amazon_linux_2023";
/// See [`BLUEPRINT_ID`].
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

/// Stop `name` and wait until it reports `stopped` — the state the modelled
/// documentation requires of the instance a disk is detached from.
async fn stop_instance(emulator: &Emulator, name: &str) {
    emulator
        .ok("StopInstance", json!({"instanceName": name}))
        .await;
    wait_for_instance_state(emulator, name, "stopped").await;
}

// ---------------------------------------------------------------------------
// The accepted detach
// ---------------------------------------------------------------------------

/// One disk a detach must be accepted for. The rows differ along everything the
/// rule generalises over: the Availability Zone (and so the region the disk and
/// the instance live in), the disk's size, the device path it was mounted at,
/// and the form of the disk's own name (hyphens, underscores, digits).
struct Accepted {
    case: &'static str,
    disk_name: &'static str,
    instance_name: &'static str,
    availability_zone: &'static str,
    size_in_gb: i64,
    path: &'static str,
}

/// The table for the accepted detach.
const ACCEPTED: &[Accepted] = &[
    Accepted {
        case: "the documented 8 GB disk at /dev/xvdf",
        disk_name: "my-disk",
        instance_name: "my-instance",
        availability_zone: "us-east-1a",
        size_in_gb: 8,
        path: "/dev/xvdf",
    },
    Accepted {
        case: "a 512 GB disk in another region, at another path",
        disk_name: "data_disk_512",
        instance_name: "data-host",
        availability_zone: "eu-west-1c",
        size_in_gb: 512,
        path: "/dev/xvdg",
    },
    Accepted {
        case: "a 16384 GB disk, the largest published size",
        disk_name: "big-disk-16tb",
        instance_name: "big_host_1",
        availability_zone: "ap-southeast-2b",
        size_in_gb: 16 * 1024,
        path: "/dev/xvdh",
    },
];

/// RULE: a detach of a disk held by a stopped instance is accepted, answers one
/// non-terminal operation-log entry, and releases the attachment when the clock
/// settles the disk into `available` — nothing else about the disk changes, and
/// the instance stops reporting it.
#[tokio::test]
async fn detaching_a_disk_from_a_stopped_instance_releases_it() {
    let emulator = Emulator::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();

    for row in ACCEPTED {
        let case = row.case;
        create_running_instance(&emulator, row.instance_name, row.availability_zone).await;
        create_available_disk(
            &emulator,
            row.disk_name,
            row.availability_zone,
            row.size_in_gb,
        )
        .await;
        attach(&emulator, row.disk_name, row.instance_name, row.path).await;
        stop_instance(&emulator, row.instance_name).await;

        // The attachment `AttachDisk` made is what a client sees before the
        // detach.
        let attached = disk(&emulator, row.disk_name).await;
        assert_eq!(
            (
                attached["state"].as_str(),
                attached["isAttached"].as_bool(),
                attached["attachedTo"].as_str(),
                attached["path"].as_str()
            ),
            (
                Some("in-use"),
                Some(true),
                Some(row.instance_name),
                Some(row.path)
            ),
            "case {case}: expected the disk to start out attached, got {attached:?}"
        );

        let detached = emulator
            .ok("DetachDisk", json!({"diskName": row.disk_name}))
            .await;
        let operations = detached["operations"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case}: expected an operations array, got {detached:?}")
            })
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
                Some("DetachDisk"),
                Some("Disk"),
                Some(row.disk_name),
                Some("Started"),
                Some(false),
            ),
            "case {case}: expected a started DetachDisk entry for this disk, got {operation:?}"
        );
        assert_eq!(
            operation["location"]["availabilityZone"].as_str(),
            Some(row.availability_zone),
            "case {case}: expected the entry located where the disk is, got {operation:?}"
        );
        let operation_id = operation["id"].as_str().unwrap_or_default().to_string();
        assert!(
            operation_ids.insert(operation_id.clone()),
            "case {case}: expected a fresh operation id, got the already-used {operation_id:?}"
        );

        // And once the clock has settled it, the attachment is gone — and only
        // the attachment: the disk's own members are untouched.
        let released = wait_for_disk_state(&emulator, row.disk_name, "available").await;
        assert_eq!(
            (
                released["isAttached"].as_bool(),
                released["attachedTo"].as_str(),
                released["path"].as_str(),
                released["attachmentState"].as_str(),
            ),
            (Some(false), None, None, Some("detached")),
            "case {case}: expected a released disk, got {released:?}"
        );
        assert_eq!(
            (
                released["name"].as_str(),
                released["sizeInGb"].as_i64(),
                released["location"]["availabilityZone"].as_str(),
                released["isSystemDisk"].as_bool(),
            ),
            (
                Some(row.disk_name),
                Some(row.size_in_gb),
                Some(row.availability_zone),
                Some(false),
            ),
            "case {case}: expected the detach to change only the attachment, got {released:?}"
        );

        // The instance no longer reports the disk in its hardware, because that
        // projection is built from the attachment this detach released.
        let reported = attached_disk_names(&emulator, row.instance_name).await;
        assert!(
            reported.is_empty(),
            "case {case}: expected the instance to report no block storage disk, got {reported:?}"
        );
    }
}

/// RULE (the span is never zero): the release is work the clock owes, so a disk
/// read straight after an accepted detach is still attached and `in-use`, and the
/// instance still reports it — the transient state AWS shows too.
#[tokio::test]
async fn a_detach_in_flight_still_reports_the_disk_as_attached() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, "inflight-host", "us-east-2a").await;
    create_available_disk(&emulator, "inflight-disk", "us-east-2a", 32).await;
    attach(&emulator, "inflight-disk", "inflight-host", "/dev/xvdf").await;
    stop_instance(&emulator, "inflight-host").await;

    emulator
        .ok("DetachDisk", json!({"diskName": "inflight-disk"}))
        .await;

    let detaching = disk(&emulator, "inflight-disk").await;
    assert_eq!(
        (
            detaching["state"].as_str(),
            detaching["isAttached"].as_bool(),
            detaching["attachedTo"].as_str(),
            detaching["path"].as_str(),
        ),
        (
            Some("in-use"),
            Some(true),
            Some("inflight-host"),
            Some("/dev/xvdf")
        ),
        "case detach in flight: expected the disk still attached, got {detaching:?}"
    );
    let reported = attached_disk_names(&emulator, "inflight-host").await;
    assert_eq!(
        reported,
        vec!["inflight-disk".to_string()],
        "case detach in flight: expected the instance still reporting the disk, got {reported:?}"
    );

    // The clock, not a reader, is what finishes it.
    let released = wait_for_disk_state(&emulator, "inflight-disk", "available").await;
    assert_eq!(
        released["isAttached"].as_bool(),
        Some(false),
        "case detach in flight: expected the clock to release the disk, got {released:?}"
    );
}

/// RULE (idempotence): a detach of a disk whose detach is already in flight is
/// accepted again and still ends with the disk released exactly once; once it is
/// released there is no attachment left, so a later detach is refused.
#[tokio::test]
async fn repeating_a_detach_in_flight_is_accepted_and_releases_the_disk_once() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, "repeat-host", "us-east-2a").await;
    create_available_disk(&emulator, "repeat-disk", "us-east-2a", 16).await;
    attach(&emulator, "repeat-disk", "repeat-host", "/dev/xvdf").await;
    stop_instance(&emulator, "repeat-host").await;

    let mut ids: BTreeSet<String> = BTreeSet::new();
    for attempt in 1..=2 {
        let answered = emulator
            .ok("DetachDisk", json!({"diskName": "repeat-disk"}))
            .await;
        let operation = answered["operations"][0].clone();
        assert_eq!(
            operation["operationType"].as_str(),
            Some("DetachDisk"),
            "case detach attempt {attempt}: expected a DetachDisk entry, got {operation:?}"
        );
        assert!(
            ids.insert(operation["id"].as_str().unwrap_or_default().to_string()),
            "case detach attempt {attempt}: expected its own operation id, got {operation:?}"
        );
    }

    let released = wait_for_disk_state(&emulator, "repeat-disk", "available").await;
    assert_eq!(
        (
            released["isAttached"].as_bool(),
            released["attachedTo"].as_str()
        ),
        (Some(false), None),
        "case repeated detach: expected the disk released once, got {released:?}"
    );

    let (status, body) = emulator
        .call("DetachDisk", json!({"diskName": "repeat-disk"}))
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "OperationFailureException"),
        "case detach after release: expected the operation-failure rejection, got {body:?}"
    );
}

/// RULE (round trip): a released disk can be attached again, and detached again
/// — the attachment is state, not a one-way door.
#[tokio::test]
async fn a_released_disk_can_be_attached_and_detached_again() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, "cycle-host", "us-west-2a").await;
    create_available_disk(&emulator, "cycle-disk", "us-west-2a", 8).await;

    for (round, path) in [(1, "/dev/xvdf"), (2, "/dev/xvdg")] {
        // Round 1 attaches to a running instance and stops it; round 2 attaches
        // to the instance it left stopped — "a running or stopped Lightsail
        // instance" is what `AttachDisk` accepts, and a detach needs it stopped
        // either way.
        attach(&emulator, "cycle-disk", "cycle-host", path).await;
        let attached = disk(&emulator, "cycle-disk").await;
        assert_eq!(
            (attached["state"].as_str(), attached["path"].as_str()),
            (Some("in-use"), Some(path)),
            "case round {round}: expected the disk attached at {path}, got {attached:?}"
        );

        stop_instance(&emulator, "cycle-host").await;
        emulator
            .ok("DetachDisk", json!({"diskName": "cycle-disk"}))
            .await;
        let released = wait_for_disk_state(&emulator, "cycle-disk", "available").await;
        assert_eq!(
            (
                released["isAttached"].as_bool(),
                released["path"].as_str(),
                released["attachedTo"].as_str()
            ),
            (Some(false), None, None),
            "case round {round}: expected the disk released again, got {released:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The refusals
// ---------------------------------------------------------------------------

/// One disk no instance holds, and the `DiskState` value it carries when the
/// detach is attempted. The rows differ along the disk's state (a disk the clock
/// has not settled yet, and a settled one), its size, its zone and the form of
/// its name.
struct Unattached {
    case: &'static str,
    disk_name: &'static str,
    availability_zone: &'static str,
    size_in_gb: i64,
    /// Whether to wait for the clock before detaching — `false` catches the disk
    /// while it is still `pending`.
    wait_for_available: bool,
    state: &'static str,
}

/// The table for a detach of a disk nothing holds. The issue's own case — a disk
/// in state `available` — is one row of it.
const UNATTACHED: &[Unattached] = &[
    Unattached {
        case: "a disk the clock has settled, the recorded case",
        disk_name: "unattached-available",
        availability_zone: "us-west-2a",
        size_in_gb: 8,
        wait_for_available: true,
        state: "available",
    },
    Unattached {
        case: "a disk still being provisioned",
        disk_name: "unattached_pending",
        availability_zone: "eu-central-1b",
        size_in_gb: 128,
        wait_for_available: false,
        state: "pending",
    },
    Unattached {
        case: "a settled disk of another size in another region",
        disk_name: "unattached-big-1",
        availability_zone: "ap-northeast-1c",
        size_in_gb: 2048,
        wait_for_available: true,
        state: "available",
    },
];

/// RULE: a disk no instance holds has no attachment for a detach to release, so
/// the call fails as an operation — HTTP 400 `OperationFailureException`, whose
/// message names the disk's own state — and the disk is left exactly as it was.
#[tokio::test]
async fn detaching_a_disk_nothing_holds_is_refused_as_an_operation_failure() {
    let emulator = Emulator::new();

    for row in UNATTACHED {
        let case = row.case;
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
        if row.wait_for_available {
            wait_for_disk_state(&emulator, row.disk_name, "available").await;
        }

        let (status, body) = emulator
            .call("DetachDisk", json!({"diskName": row.disk_name}))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "OperationFailureException",
            "case {case}: expected the operation-failure code, got {body:?}"
        );
        let answered = message(&body);
        assert!(
            answered.contains(&format!("The state of this disk is: {}.", row.state)),
            "case {case}: expected a message naming the disk's state '{}', got {answered:?}",
            row.state
        );

        // The refusal changed nothing.
        let disk = disk(&emulator, row.disk_name).await;
        assert_eq!(
            (
                disk["isAttached"].as_bool(),
                disk["attachedTo"].as_str(),
                disk["sizeInGb"].as_i64()
            ),
            (Some(false), None, Some(row.size_in_gb)),
            "case {case}: expected the refused detach to leave the disk alone, got {disk:?}"
        );
        // A disk caught while `pending` still settles, exactly as it would have
        // without the refused call.
        let settled = wait_for_disk_state(&emulator, row.disk_name, "available").await;
        assert_eq!(
            settled["state"].as_str(),
            Some("available"),
            "case {case}: expected the disk to settle regardless, got {settled:?}"
        );
    }
}

/// One instance state a detach must be refused for, and how to put the instance
/// in it.
struct NotStopped {
    case: &'static str,
    disk_name: &'static str,
    instance_name: &'static str,
    availability_zone: &'static str,
    /// The `InstanceState` name the instance reports when the detach is made.
    state: &'static str,
    /// Whether to ask the instance to stop first — a `stopping` instance is on
    /// its way down and is not stopped yet.
    stop_first: bool,
}

/// The table for a detach from an instance that is not stopped. `AttachDisk`
/// attaches "to a running or stopped Lightsail instance", so the two states an
/// attached disk can be found on while its instance is not stopped are `running`
/// and `stopping`.
const NOT_STOPPED: &[NotStopped] = &[
    NotStopped {
        case: "a serving instance",
        disk_name: "running_host_disk",
        instance_name: "running-host",
        availability_zone: "us-west-1b",
        state: "running",
        stop_first: false,
    },
    NotStopped {
        case: "an instance on its way down",
        disk_name: "stopping-host-disk-1",
        instance_name: "stopping_host",
        availability_zone: "eu-west-2a",
        state: "stopping",
        stop_first: true,
    },
];

/// RULE: a disk comes off an instance only while that instance is stopped
/// ("Detaches a stopped block storage disk from a Lightsail instance"), so a
/// detach from an instance in any other state is refused as an operation failure
/// and the disk stays attached.
#[tokio::test]
async fn detaching_a_disk_from_an_instance_that_is_not_stopped_is_refused() {
    let emulator = Emulator::new();

    for row in NOT_STOPPED {
        let case = row.case;
        create_running_instance(&emulator, row.instance_name, row.availability_zone).await;
        create_available_disk(&emulator, row.disk_name, row.availability_zone, 8).await;
        attach(&emulator, row.disk_name, row.instance_name, "/dev/xvdf").await;
        if row.stop_first {
            emulator
                .ok("StopInstance", json!({"instanceName": row.instance_name}))
                .await;
        }
        // The instance really is in the state this row is about.
        let holder = instance(&emulator, row.instance_name).await;
        assert_eq!(
            holder["state"]["name"].as_str(),
            Some(row.state),
            "case {case}: expected the holding instance {} to be {}, got {holder:?}",
            row.instance_name,
            row.state
        );

        let (status, body) = emulator
            .call("DetachDisk", json!({"diskName": row.disk_name}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "OperationFailureException"),
            "case {case}: expected the operation-failure rejection, got {body:?}"
        );
        let answered = message(&body);
        assert!(
            answered.contains(&format!(
                "the state of instance {} is: {}.",
                row.instance_name, row.state
            )),
            "case {case}: expected a message naming the instance and its state '{}', got \
             {answered:?}",
            row.state
        );

        // The disk is still held by the instance, at the path it was attached at.
        let disk = disk(&emulator, row.disk_name).await;
        assert_eq!(
            (
                disk["state"].as_str(),
                disk["isAttached"].as_bool(),
                disk["attachedTo"].as_str(),
                disk["path"].as_str()
            ),
            (
                Some("in-use"),
                Some(true),
                Some(row.instance_name),
                Some("/dev/xvdf")
            ),
            "case {case}: expected the refused detach to leave the disk attached, got {disk:?}"
        );

        // And once that instance IS stopped, the same call is accepted — the
        // refusal was about the state, not about the request.
        stop_instance(&emulator, row.instance_name).await;
        emulator
            .ok("DetachDisk", json!({"diskName": row.disk_name}))
            .await;
        let released = wait_for_disk_state(&emulator, row.disk_name, "available").await;
        assert_eq!(
            released["isAttached"].as_bool(),
            Some(false),
            "case {case}: expected the detach accepted once stopped, got {released:?}"
        );
    }
}

/// RULE: a name this account holds no disk under is refused with the service's
/// `DoesNotExist` envelope — HTTP 400, wire error `NotFoundException`, body
/// `code` `DoesNotExist` — whatever else that name may address.
#[tokio::test]
async fn detaching_a_disk_this_account_does_not_hold_is_refused() {
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
            .call("DetachDisk", json!({"diskName": name}))
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

    // The disk that does exist is untouched by any of those refusals.
    let disk = disk(&emulator, "a-real-disk").await;
    assert_eq!(
        disk["state"].as_str(),
        Some("available"),
        "case refused names: expected the stored disk untouched, got {disk:?}"
    );
}
