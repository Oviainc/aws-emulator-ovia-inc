// SPDX-License-Identifier: CC0-1.0

//! The daily hour an automatic-snapshot add-on is scheduled at, driven over the
//! wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: the snapshot hour an automatic-snapshot add-on request
//! carries is accepted only on an HOURLY INCREMENT — the modelled
//! `AutoSnapshotAddOnRequest$snapshotTimeOfDay` documentation, "Must be in
//! `HH:00` format, and in an hourly increment" — so every operation that carries
//! an add-on request stores the hour the request named when its minutes are `00`
//! and refuses it with the modelled `InvalidInputException` when they are not,
//! writing nothing. That holds wherever such a request is accepted: the single
//! `addOnRequest` of `EnableAddOn` and the `addOns` list of `CreateInstances`,
//! `CreateDisk`, `CreateInstancesFromSnapshot` and `CreateDiskFromSnapshot`.

use serde_json::{json, Value};

// The harness and the wire error code this test target already exposes: this
// suite drives the service through the same front door rather than keeping a
// second copy of either.
use crate::instance_lifecycle::{error_code, Emulator};

/// The zone every resource in this suite is created in; a snapshot hour is not
/// regional, so one zone serves every case.
const ZONE: &str = "us-east-1a";

/// A Linux/Unix blueprint and the smallest plan: neither selects anything about
/// an add-on.
const BLUEPRINT: &str = "amazon_linux_2023";
/// See [`BLUEPRINT`].
const BUNDLE: &str = "nano_3_0";

/// The size of every block storage disk in this suite: the smallest Lightsail
/// publishes (8 GB), since the size selects nothing about an add-on.
const DISK_SIZE_IN_GB: i64 = 8;

/// Every snapshot hour the rule admits: `HH:00` for each hour of a UTC day,
/// which is every value on an hourly increment the modelled `TimeOfDay` pattern
/// `^(0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]$` admits — `00:00` and `23:00` included,
/// its two published bounds.
fn hourly_times() -> Vec<String> {
    (0..24).map(|hour| format!("{hour:02}:00")).collect()
}

/// Times of day the modelled `TimeOfDay` pattern admits that are NOT hourly
/// increments, spread across the day so each case is about the MINUTES and not
/// about one hour: one minute past the hour, each quarter of it, the half hour
/// and the last minute the pattern admits.
const NON_HOURLY_TIMES: &[&str] = &["00:01", "05:15", "10:30", "14:30", "18:45", "23:59"];

/// Times of day that are not `HH:00` at all, and that the modelled `TimeOfDay`
/// pattern does not admit either: a single-digit hour, an hour past the last of
/// the day, a signed hour, a bare hour with no minutes, and the empty string.
const MALFORMED_TIMES: &[&str] = &["9:00", "0:00", "24:00", "99:00", "+1:00", "07", ""];

/// A resource kind an add-on can be enabled on. The two the model gives an
/// `addOns` member, and the pair the modelled `AutoSnapshotAddOnRequest`
/// documentation names ("an Amazon Lightsail instance or disk").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Instance,
    Disk,
}

/// The `addOnRequest`/`addOns` entry asking for the automatic snapshot add-on at
/// `time_of_day`.
fn auto_snapshot_request(time_of_day: &str) -> Value {
    json!({
        "addOnType": "AutoSnapshot",
        "autoSnapshotAddOnRequest": {"snapshotTimeOfDay": time_of_day},
    })
}

/// Create the resource `name` of `kind`, carrying no add-ons.
async fn create(emulator: &Emulator, kind: Kind, name: &str) {
    let (operation, body) = match kind {
        Kind::Instance => (
            "CreateInstances",
            json!({
                "instanceNames": [name],
                "availabilityZone": ZONE,
                "blueprintId": BLUEPRINT,
                "bundleId": BUNDLE,
            }),
        ),
        Kind::Disk => (
            "CreateDisk",
            json!({
                "diskName": name,
                "availabilityZone": ZONE,
                "sizeInGb": DISK_SIZE_IN_GB,
            }),
        ),
    };
    emulator.ok(operation, body).await;
}

/// The `addOns` a read of the resource `name` of `kind` reports.
async fn add_ons(emulator: &Emulator, kind: Kind, name: &str) -> Vec<Value> {
    let resource = match kind {
        Kind::Instance => {
            emulator
                .ok("GetInstance", json!({"instanceName": name}))
                .await["instance"]
                .clone()
        }
        Kind::Disk => emulator.ok("GetDisk", json!({"diskName": name})).await["disk"].clone(),
    };
    resource["addOns"].as_array().cloned().unwrap_or_default()
}

/// Assert that `body` is this service's refusal of a snapshot hour that is not
/// an hourly increment: the modelled `InvalidInputException`, naming the member
/// the caller sent, the value that broke the rule and the constraint it broke.
fn assert_refused(case: &str, status: u16, body: &Value, member: &str, time_of_day: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the modelled InvalidInputException status, got {body:?}"
    );
    assert_eq!(
        error_code(body),
        "InvalidInputException",
        "case {case:?}: expected the modelled InvalidInputException, got {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(member)
            && message.contains(time_of_day)
            && message.contains("hourly increment"),
        "case {case:?}: expected the message to name {member}, {time_of_day} and the \
         constraint it broke, got {body:?}"
    );
}

/// RULE: a snapshot hour on an hourly increment is stored as the resource's
/// daily automatic-snapshot time, for every hour of the UTC day and on both
/// kinds of resource that can carry the add-on.
#[tokio::test]
async fn enable_add_on_stores_every_hourly_snapshot_time() {
    let emulator = Emulator::new();
    for (kind, name) in [
        (Kind::Instance, "hourly-instance"),
        (Kind::Disk, "hourly-disk"),
    ] {
        create(&emulator, kind, name).await;
        for time_of_day in hourly_times() {
            let case = format!("{kind:?} at {time_of_day}");
            emulator
                .ok(
                    "EnableAddOn",
                    json!({
                        "resourceName": name,
                        "addOnRequest": auto_snapshot_request(&time_of_day),
                    }),
                )
                .await;
            let reported = add_ons(&emulator, kind, name).await;
            assert_eq!(
                reported.len(),
                1,
                "case {case:?}: a resource carries one add-on per type, got {reported:?}"
            );
            assert_eq!(
                reported[0]["snapshotTimeOfDay"].as_str(),
                Some(time_of_day.as_str()),
                "case {case:?}: expected the hour the request named, got {reported:?}"
            );
        }
    }
}

/// RULE: a snapshot hour the modelled pattern admits as a wall-clock time but
/// that is not an hourly increment is refused, and the resource keeps the
/// schedule it already had.
#[tokio::test]
async fn enable_add_on_refuses_a_snapshot_time_that_is_not_hourly() {
    let emulator = Emulator::new();
    /// The hour each resource carries before the refused calls, so the "nothing
    /// was written" assertion has something to compare against.
    const HELD: &str = "03:00";

    for (kind, name) in [
        (Kind::Instance, "non-hourly-instance"),
        (Kind::Disk, "non-hourly-disk"),
    ] {
        create(&emulator, kind, name).await;
        emulator
            .ok(
                "EnableAddOn",
                json!({
                    "resourceName": name,
                    "addOnRequest": auto_snapshot_request(HELD),
                }),
            )
            .await;

        for time_of_day in NON_HOURLY_TIMES {
            let case = format!("{kind:?} at {time_of_day}");
            let (status, body) = emulator
                .call(
                    "EnableAddOn",
                    json!({
                        "resourceName": name,
                        "addOnRequest": auto_snapshot_request(time_of_day),
                    }),
                )
                .await;
            // The member path is this request's own single `addOnRequest`, never
            // the create calls' list.
            assert_refused(
                &case,
                status,
                &body,
                "addOnRequest.autoSnapshotAddOnRequest.snapshotTimeOfDay",
                time_of_day,
            );
            let message = body["message"].as_str().unwrap_or_default();
            assert!(
                !message.contains("addOns[]"),
                "case {case:?}: expected the member the caller sent, got {body:?}"
            );

            // A refused request writes nothing: the schedule is the one the
            // accepted call left.
            let reported = add_ons(&emulator, kind, name).await;
            assert_eq!(
                reported.len(),
                1,
                "case {case:?}: a refused request stores nothing, got {reported:?}"
            );
            assert_eq!(
                reported[0]["snapshotTimeOfDay"].as_str(),
                Some(HELD),
                "case {case:?}: expected the hour already enabled ({HELD}), got {reported:?}"
            );
        }

        // The rest of the class: a time of day that is not two digits, a colon
        // and two digits in range is not an `HH:00` hour either. Which layer
        // refuses it is the modelled `TimeOfDay` pattern's business — these are
        // values it does not admit — so the case asserts only what the rule
        // says: the request is refused and the schedule is untouched.
        for time_of_day in MALFORMED_TIMES {
            let case = format!("{kind:?} at {time_of_day:?}");
            let (status, body) = emulator
                .call(
                    "EnableAddOn",
                    json!({
                        "resourceName": name,
                        "addOnRequest": auto_snapshot_request(time_of_day),
                    }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected a refusal, got {body:?}"
            );
            let reported = add_ons(&emulator, kind, name).await;
            assert_eq!(
                reported.len(),
                1,
                "case {case:?}: a refused request stores nothing, got {reported:?}"
            );
            assert_eq!(
                reported[0]["snapshotTimeOfDay"].as_str(),
                Some(HELD),
                "case {case:?}: expected the hour already enabled ({HELD}), got {reported:?}"
            );
        }
    }
}

/// An operation that creates a resource carrying add-ons, and how the resource
/// it creates is read back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Creator {
    Instances,
    Disk,
    /// A restore, whose source this suite does not create: the add-on request is
    /// judged before any source is resolved, which is what lets the hourly case
    /// below tell the two refusals apart.
    InstancesFromSnapshot,
    DiskFromSnapshot,
}

impl Creator {
    /// The operation name this creator is called by.
    fn operation(self) -> &'static str {
        match self {
            Self::Instances => "CreateInstances",
            Self::Disk => "CreateDisk",
            Self::InstancesFromSnapshot => "CreateInstancesFromSnapshot",
            Self::DiskFromSnapshot => "CreateDiskFromSnapshot",
        }
    }

    /// The kind of resource it creates, which says how a created one is read
    /// back.
    fn kind(self) -> Kind {
        match self {
            Self::Instances | Self::InstancesFromSnapshot => Kind::Instance,
            Self::Disk | Self::DiskFromSnapshot => Kind::Disk,
        }
    }

    /// Whether the resource it creates is restored from a source — and so
    /// whether a request this suite sends can succeed at all.
    fn restores(self) -> bool {
        matches!(self, Self::InstancesFromSnapshot | Self::DiskFromSnapshot)
    }

    /// The request body creating `name` with the automatic snapshot add-on at
    /// `time_of_day`.
    fn body(self, name: &str, time_of_day: &str) -> Value {
        let add_ons = json!([auto_snapshot_request(time_of_day)]);
        match self {
            Self::Instances => json!({
                "instanceNames": [name],
                "availabilityZone": ZONE,
                "blueprintId": BLUEPRINT,
                "bundleId": BUNDLE,
                "addOns": add_ons,
            }),
            Self::Disk => json!({
                "diskName": name,
                "availabilityZone": ZONE,
                "sizeInGb": DISK_SIZE_IN_GB,
                "addOns": add_ons,
            }),
            Self::InstancesFromSnapshot => json!({
                "instanceNames": [name],
                "availabilityZone": ZONE,
                "bundleId": BUNDLE,
                "instanceSnapshotName": "add-on-hour-no-such-snapshot",
                "addOns": add_ons,
            }),
            Self::DiskFromSnapshot => json!({
                "diskName": name,
                "availabilityZone": ZONE,
                "sizeInGb": DISK_SIZE_IN_GB,
                "diskSnapshotName": "add-on-hour-no-such-snapshot",
                "addOns": add_ons,
            }),
        }
    }
}

/// Every creator, so a new one is added to the table rather than left
/// untested.
const CREATORS: &[Creator] = &[
    Creator::Instances,
    Creator::Disk,
    Creator::InstancesFromSnapshot,
    Creator::DiskFromSnapshot,
];

/// RULE: the same hourly-increment constraint holds on the `addOns` list of
/// every operation that creates a resource carrying add-ons — a non-hourly hour
/// is refused and creates nothing, while an hourly one is carried past the
/// add-on conversion (a create that names an existing source stores it; a
/// restore whose source this account does not hold is then refused for the
/// SOURCE, not for the hour).
#[tokio::test]
async fn the_create_calls_refuse_a_snapshot_time_that_is_not_hourly() {
    let emulator = Emulator::new();

    for creator in CREATORS {
        for (index, time_of_day) in NON_HOURLY_TIMES.iter().enumerate() {
            let case = format!("{} at {time_of_day}", creator.operation());
            let name = format!("add-on-hour-refused-{}-{index}", creator.operation());
            let (status, body) = emulator
                .call(creator.operation(), creator.body(&name, time_of_day))
                .await;
            assert_refused(
                &case,
                status,
                &body,
                "addOns[].autoSnapshotAddOnRequest.snapshotTimeOfDay",
                time_of_day,
            );

            // A refused create creates nothing: the name it would have taken
            // holds no resource.
            let read = match creator.kind() {
                Kind::Instance => emulator
                    .call("GetInstance", json!({"instanceName": name}))
                    .await,
                Kind::Disk => emulator.call("GetDisk", json!({"diskName": name})).await,
            };
            assert_eq!(
                error_code(&read.1),
                "NotFoundException",
                "case {case:?}: a refused create stores nothing, got {:?}",
                read.1
            );
        }

        // The hourly control, in the SAME path: the add-on conversion accepts it,
        // so what happens next is about the request's source rather than its hour.
        let case = format!("{} at an hourly increment", creator.operation());
        let name = format!("add-on-hour-accepted-{}", creator.operation());
        let (status, body) = emulator
            .call(creator.operation(), creator.body(&name, "21:00"))
            .await;
        if creator.restores() {
            assert_eq!(
                status, 400,
                "case {case:?}: a restore from a source this account does not hold is \
                 refused, got {body:?}"
            );
            assert_eq!(
                body["code"].as_str(),
                Some("DoesNotExist"),
                "case {case:?}: expected the refusal to be about the source, got {body:?}"
            );
            let message = body["message"].as_str().unwrap_or_default();
            assert!(
                !message.contains("snapshotTimeOfDay"),
                "case {case:?}: an hourly increment is not what is refused, got {body:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: an hourly increment is accepted, got {body:?}"
            );
            let reported = add_ons(&emulator, creator.kind(), &name).await;
            assert_eq!(
                reported.len(),
                1,
                "case {case:?}: expected the created resource to carry the add-on, got \
                 {reported:?}"
            );
            assert_eq!(
                reported[0]["snapshotTimeOfDay"].as_str(),
                Some("21:00"),
                "case {case:?}: expected the hour the create named, got {reported:?}"
            );
        }
    }
}
