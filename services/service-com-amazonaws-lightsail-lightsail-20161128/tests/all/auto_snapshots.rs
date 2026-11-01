// SPDX-License-Identifier: CC0-1.0

//! The automatic snapshots the `AutoSnapshot` add-on takes of a Lightsail
//! instance or disk, read back through `GetAutoSnapshots`, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: the automatic snapshots an account holds of a resource
//! are the copies that resource's ENABLED automatic snapshot add-on has taken —
//! one per UTC date, each dated by the UTC date it was taken on, timestamped
//! with the instant the service started taking it, reporting the status that
//! copy is in, and (for an instance) the block storage disks attached to it when
//! it was taken. `GetAutoSnapshots` reports exactly those copies for the one
//! instance or disk the request names, together with that resource's own
//! `ResourceType`: an empty list for a resource no copy has been taken of, the
//! copies themselves for one that has them, nothing for a name that addresses no
//! copies of this account — a resource it no longer holds whose add-ons a delete
//! was asked to delete with it — and the service's `DoesNotExist` envelope for a
//! name that is neither an instance nor a disk. Only the CLOCK takes a copy: a
//! read reports what is held and never takes one.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

// The harness and the wire error code this test target already exposes: a
// sibling suite drives the service through the same front door rather than
// standing up a second one.
use crate::instance_lifecycle::{error_code, Emulator};

/// The zone every resource in this suite is created in: the automatic snapshot
/// add-on is not regional, so one zone serves every case.
const ZONE: &str = "us-east-1a";

/// A Linux/Unix blueprint and the smallest plan: neither selects anything about
/// the automatic snapshot add-on.
const BLUEPRINT: &str = "amazon_linux_2023";
/// See [`BLUEPRINT`].
const BUNDLE: &str = "nano_3_0";

/// How long a case waits for a copy the clock owes. Far longer than the span the
/// store takes to make the first copy due, so a loaded machine still finishes.
const WAIT: Duration = Duration::from_secs(5);

/// How long a case waits before concluding that NO copy is owed at all — more
/// than the store's own span, so a copy that was going to be taken has been.
pub(crate) const NO_COPY_WAIT: Duration = Duration::from_millis(1_500);

/// The modelled `AutoSnapshotStatus` values a copy of this store reports: the
/// one a copy the service is still taking is in, and the one it settles into.
const IN_PROGRESS: &str = "InProgress";
/// See [`IN_PROGRESS`].
const SUCCESS: &str = "Success";

/// A resource kind an automatic snapshot can be taken of: the pair the modelled
/// `AutoSnapshotAddOnRequest` documentation names ("an Amazon Lightsail instance
/// or disk"), which is also the pair `GetAutoSnapshots$resourceName` describes
/// ("the source instance or disk").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Instance,
    Disk,
}

impl Kind {
    /// The modelled `ResourceType` value a resource of this kind reports.
    pub(crate) fn resource_type(self) -> &'static str {
        match self {
            Self::Instance => "Instance",
            Self::Disk => "Disk",
        }
    }
}

/// One block storage disk a case attaches to its instance before the copy is
/// taken, so the copy captures it.
#[derive(Debug, Clone, Copy)]
struct Attached {
    name: &'static str,
    path: &'static str,
    size_in_gb: i64,
}

/// The `GetAutoSnapshots` answer for `name`, requiring success.
pub(crate) async fn report(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok("GetAutoSnapshots", json!({"resourceName": name}))
        .await
}

/// The `autoSnapshots` of that answer, in the order it reports them.
pub(crate) async fn copies(emulator: &Emulator, name: &str) -> Vec<Value> {
    report(emulator, name).await["autoSnapshots"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Assert that the answer for `name` names the resource and its kind — true of
/// every answer, whether or not any copy has been taken.
fn assert_names_the_resource(case: &str, answered: &Value, kind: Kind, name: &str) {
    assert_eq!(
        answered["resourceName"].as_str(),
        Some(name),
        "case {case:?}: expected the answer to name {name}, got {answered:?}"
    );
    assert_eq!(
        answered["resourceType"].as_str(),
        Some(kind.resource_type()),
        "case {case:?}: expected the resource's own type, got {answered:?}"
    );
}

/// Poll until `name` reports at least one automatic snapshot, answering the
/// copies.
///
/// The copy is owed by the clock, not by the poll: it arrives without anyone
/// asking more than once.
pub(crate) async fn wait_for_a_copy(emulator: &Emulator, case: &str, name: &str) -> Vec<Value> {
    let deadline = Instant::now() + WAIT;
    loop {
        let reported = copies(emulator, name).await;
        if !reported.is_empty() {
            return reported;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the add-on's copy of {name} within {WAIT:?}, got \
             {reported:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Poll until the one copy of `name` reports `Success`, answering it.
pub(crate) async fn wait_for_success(emulator: &Emulator, case: &str, name: &str) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        let reported = copies(emulator, name).await;
        if reported.len() == 1 && reported[0]["status"].as_str() == Some(SUCCESS) {
            return reported[0].clone();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the copy of {name} to reach {SUCCESS} within {WAIT:?}, \
             got {reported:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Now, in epoch seconds — what a reported `createdAt` is measured against.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// The UTC calendar date of the epoch-second instant `secs`, in `YYYY-MM-DD`
/// form.
///
/// Derived here from the timestamp the answer itself carries, through the same
/// library rendering (`DateTime::fmt(Format::DateTime)` is RFC 3339 in UTC), so
/// the case asserts the RELATION the rule states — a copy is dated by the UTC
/// date of the instant it was taken — rather than re-deriving a calendar.
pub(crate) fn utc_date_of(secs: f64) -> String {
    ::aws_smithy_types::DateTime::from_secs_f64(secs)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
        .and_then(|rendered| rendered.split_once('T').map(|(date, _)| date.to_string()))
        .unwrap_or_default()
}

/// The `addOnRequest`/`addOns` entry for the automatic snapshot add-on, carrying
/// `hour` when one is named.
pub(crate) fn auto_snapshot_request(hour: Option<&str>) -> Value {
    let mut request = json!({"addOnType": "AutoSnapshot"});
    if let Some(hour) = hour {
        request["autoSnapshotAddOnRequest"] = json!({"snapshotTimeOfDay": hour});
    }
    request
}

/// Create the resource `name` of `kind`, carrying `add_ons` at creation time.
pub(crate) async fn create(
    emulator: &Emulator,
    kind: Kind,
    name: &str,
    add_ons: &[Value],
    size_in_gb: i64,
) {
    let mut body = match kind {
        Kind::Instance => json!({
            "instanceNames": [name],
            "availabilityZone": ZONE,
            "blueprintId": BLUEPRINT,
            "bundleId": BUNDLE,
        }),
        Kind::Disk => json!({
            "diskName": name,
            "availabilityZone": ZONE,
            "sizeInGb": size_in_gb,
        }),
    };
    if !add_ons.is_empty() {
        body["addOns"] = json!(add_ons);
    }
    let operation = match kind {
        Kind::Instance => "CreateInstances",
        Kind::Disk => "CreateDisk",
    };
    emulator.ok(operation, body).await;
}

/// Attach `disk` to the instance `instance_name`, once the disk is available.
async fn attach(emulator: &Emulator, instance_name: &str, disk: Attached) {
    create(emulator, Kind::Disk, disk.name, &[], disk.size_in_gb).await;
    let deadline = Instant::now() + WAIT;
    loop {
        let state = emulator
            .ok("GetDisk", json!({"diskName": disk.name}))
            .await["disk"]["state"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if state == "available" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case attaching {}: expected it available within {WAIT:?}, got {state}",
            disk.name
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    emulator
        .ok(
            "AttachDisk",
            json!({
                "diskName": disk.name,
                "instanceName": instance_name,
                "diskPath": disk.path,
            }),
        )
        .await;
}

// ---------------------------------------------------------------------------
// The copy table
// ---------------------------------------------------------------------------

/// Where the resource's automatic snapshot add-on came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnabledBy {
    /// The create call carried it, with the hour it names.
    Create(Option<&'static str>),
    /// A later `EnableAddOn` call enabled it, with the hour it names.
    EnableAddOn(Option<&'static str>),
}

/// One resource whose add-on must take a copy, and what that copy must report.
///
/// The rows differ along everything the rule generalises over: the KIND of
/// resource the copy is taken of, which call enabled the add-on, whether that
/// call named the daily hour at all (and, when it did, each published bound of
/// the modelled `TimeOfDay`), and how many block storage disks were attached to
/// the instance when the copy was taken.
struct Row {
    case: &'static str,
    kind: Kind,
    /// Distinct per row, so no row can see another's resource.
    name: &'static str,
    size_in_gb: i64,
    enabled_by: EnabledBy,
    /// The disks attached before the add-on is enabled, which the copy must
    /// capture. Instances only: a disk is one volume and has nothing attached
    /// to it.
    attached: &'static [Attached],
}

/// The table. `00:00` and `23:00` are the first and last hour the modelled
/// `TimeOfDay` pattern `^(0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]$` admits on the
/// hourly increment `AutoSnapshotAddOnRequest$snapshotTimeOfDay` requires.
const ROWS: &[Row] = &[
    Row {
        case: "an instance whose create enabled the add-on at a named hour",
        kind: Kind::Instance,
        name: "auto-instance-created-with-add-on",
        size_in_gb: 0,
        enabled_by: EnabledBy::Create(Some("06:00")),
        attached: &[],
    },
    Row {
        case: "an instance whose create enabled the add-on with no hour named",
        kind: Kind::Instance,
        name: "auto_instance_service_chosen_hour",
        size_in_gb: 0,
        enabled_by: EnabledBy::Create(None),
        attached: &[],
    },
    Row {
        case: "an instance with two attached disks, add-on enabled afterwards at the last hour",
        kind: Kind::Instance,
        name: "auto-instance-two-disks",
        size_in_gb: 0,
        enabled_by: EnabledBy::EnableAddOn(Some("23:00")),
        attached: &[
            Attached {
                name: "auto-attached-disk-a",
                path: "/dev/xvdf",
                size_in_gb: 8,
            },
            Attached {
                name: "auto-attached-disk-b",
                path: "/dev/xvdg",
                size_in_gb: 40,
            },
        ],
    },
    Row {
        case: "an instance with one attached disk, add-on enabled afterwards with no hour named",
        kind: Kind::Instance,
        name: "auto-instance-one-disk",
        size_in_gb: 0,
        enabled_by: EnabledBy::EnableAddOn(None),
        attached: &[Attached {
            name: "auto-attached-disk-c",
            path: "/dev/xvdh",
            size_in_gb: 16,
        }],
    },
    Row {
        case: "a disk whose create enabled the add-on at the first hour of the day",
        kind: Kind::Disk,
        name: "auto-disk-created-with-add-on",
        size_in_gb: 8,
        enabled_by: EnabledBy::Create(Some("00:00")),
        attached: &[],
    },
    Row {
        case: "a large disk whose add-on was enabled afterwards at a named hour",
        kind: Kind::Disk,
        name: "auto-disk-enabled-afterwards",
        size_in_gb: 256,
        enabled_by: EnabledBy::EnableAddOn(Some("13:00")),
        attached: &[],
    },
];

/// Every row of the table: the add-on's schedule takes one copy of the resource,
/// the copy carries the date and the instant it was taken and the disks it
/// captured, and the clock settles it into `Success`.
#[tokio::test]
async fn the_add_on_schedule_takes_the_copy_get_auto_snapshots_reports() {
    let emulator = Emulator::new();
    for row in ROWS {
        let case = row.case;
        let enabled_at = match row.enabled_by {
            EnabledBy::Create(hour) => {
                let before = now_secs();
                create(
                    &emulator,
                    row.kind,
                    row.name,
                    &[auto_snapshot_request(hour)],
                    row.size_in_gb,
                )
                .await;
                before
            }
            EnabledBy::EnableAddOn(hour) => {
                create(&emulator, row.kind, row.name, &[], row.size_in_gb).await;
                for disk in row.attached {
                    attach(&emulator, row.name, *disk).await;
                }
                // Before the add-on is enabled the resource exists and nothing
                // has been copied of it, which is an empty list and not a
                // refusal.
                let answered = report(&emulator, row.name).await;
                assert_names_the_resource(case, &answered, row.kind, row.name);
                assert_eq!(
                    answered["autoSnapshots"].as_array().map(Vec::len),
                    Some(0),
                    "case {case:?}: no add-on, so no copy has been taken, got {answered:?}"
                );
                let before = now_secs();
                emulator
                    .ok(
                        "EnableAddOn",
                        json!({
                            "resourceName": row.name,
                            "addOnRequest": auto_snapshot_request(hour),
                        }),
                    )
                    .await;
                before
            }
        };

        // The copy is the CLOCK's: it is not there the instant the add-on is
        // enabled, and it arrives without anyone asking more than once.
        let first = copies(&emulator, row.name).await;
        assert!(
            first.is_empty(),
            "case {case:?}: the first copy is not taken yet when the add-on is enabled, got \
             {first:?}"
        );

        let taken = wait_for_a_copy(&emulator, case, row.name).await;
        assert_eq!(
            taken.len(),
            1,
            "case {case:?}: the add-on takes one copy a day, got {taken:?}"
        );
        let copy = &taken[0];

        // The copy was taken between the call that enabled the add-on and now,
        // and it is dated by the UTC date of that instant.
        let created_at = copy["createdAt"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: expected a createdAt timestamp, got {copy:?}")
        });
        let now = now_secs();
        assert!(
            created_at >= enabled_at && created_at <= now,
            "case {case:?}: expected the copy taken between {enabled_at} and {now}, got \
             {created_at}"
        );
        assert_eq!(
            copy["date"].as_str(),
            Some(utc_date_of(created_at).as_str()),
            "case {case:?}: expected the UTC date of {created_at}, got {copy:?}"
        );

        // A copy the service is still taking reports `InProgress`; one it has
        // taken reports `Success`.
        let status = copy["status"].as_str().unwrap_or_default();
        assert!(
            status == IN_PROGRESS || status == SUCCESS,
            "case {case:?}: expected {IN_PROGRESS} or {SUCCESS}, got {copy:?}"
        );

        // What the copy captured: for an instance, exactly the disks attached to
        // it, by the device path each is mounted at and its size; for a disk,
        // nothing — a disk is one volume with nothing attached to it.
        match row.kind {
            Kind::Instance => {
                let want: Vec<Value> = row
                    .attached
                    .iter()
                    .map(|disk| json!({"path": disk.path, "sizeInGb": disk.size_in_gb}))
                    .collect();
                assert_eq!(
                    copy["fromAttachedDisks"],
                    json!(want),
                    "case {case:?}: expected the disks attached when it was taken, got {copy:?}"
                );
            }
            Kind::Disk => assert!(
                copy["fromAttachedDisks"].is_null(),
                "case {case:?}: a copy of a disk captures no attached disks, got {copy:?}"
            ),
        }

        // The clock settles the copy, and settling changes nothing else about
        // it: same date, same instant, same capture.
        let settled = wait_for_success(&emulator, case, row.name).await;
        for member in ["date", "createdAt", "fromAttachedDisks"] {
            assert_eq!(
                settled[member], copy[member],
                "case {case:?}: expected {member} to survive the settle as {:?}, got {:?}",
                copy[member], settled[member]
            );
        }

        // Reading is not copying: two more reads report exactly the settled
        // copy, and no second one.
        let answered = report(&emulator, row.name).await;
        assert_names_the_resource(case, &answered, row.kind, row.name);
        let again = copies(&emulator, row.name).await;
        assert_eq!(
            again,
            vec![settled.clone()],
            "case {case:?}: expected every read to report the one settled copy {settled:?}, \
             got {again:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// No add-on, no copy
// ---------------------------------------------------------------------------

/// A resource whose automatic snapshot add-on is not enabled: the answer names
/// the resource and reports no copy, however long the clock runs.
#[tokio::test]
async fn a_resource_without_the_add_on_has_no_automatic_snapshot() {
    let emulator = Emulator::new();

    // (case, kind, name, the add-ons the create carried, size)
    let rows: Vec<(&str, Kind, &str, Vec<Value>, i64)> = vec![
        (
            "an instance created with no add-ons",
            Kind::Instance,
            "auto-plain-instance",
            Vec::new(),
            0,
        ),
        (
            "a disk created with no add-ons",
            Kind::Disk,
            "auto-plain-disk",
            Vec::new(),
            32,
        ),
        (
            "an instance carrying only the idle-stop add-on",
            Kind::Instance,
            "auto-idle-stop-instance",
            vec![json!({
                "addOnType": "StopInstanceOnIdle",
                "stopInstanceOnIdleRequest": {"threshold": "20", "duration": "30"},
            })],
            0,
        ),
    ];

    for (_, kind, name, add_ons, size_in_gb) in &rows {
        create(&emulator, *kind, name, add_ons, *size_in_gb).await;
    }
    // Long enough that a copy the store was going to take has been taken.
    tokio::time::sleep(NO_COPY_WAIT).await;
    for (case, kind, name, _, _) in &rows {
        let answered = report(&emulator, name).await;
        assert_names_the_resource(case, &answered, *kind, name);
        assert_eq!(
            answered["autoSnapshots"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: nothing takes a copy without the add-on, got {answered:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The add-on's own lifecycle
// ---------------------------------------------------------------------------

/// Disabling the add-on stops the schedule and KEEPS what it already took: the
/// copies are the account's until something deletes them. Enabling it again
/// takes no second copy for a date that already has one — an automatic snapshot
/// is addressed by its date, and the add-on takes one a day.
#[tokio::test]
async fn disabling_and_re_enabling_the_add_on_keeps_the_one_copy_of_the_day() {
    let emulator = Emulator::new();
    let case = "an instance whose add-on is disabled and enabled again";
    let name = "auto-instance-add-on-cycle";
    create(
        &emulator,
        Kind::Instance,
        name,
        &[auto_snapshot_request(Some("04:00"))],
        0,
    )
    .await;
    let settled = wait_for_success(&emulator, case, name).await;

    emulator
        .ok(
            "DisableAddOn",
            json!({"resourceName": name, "addOnType": "AutoSnapshot"}),
        )
        .await;
    let kept = copies(&emulator, name).await;
    assert_eq!(
        kept,
        vec![settled.clone()],
        "case {case:?}: expected the copy already taken to be kept, got {kept:?}"
    );

    emulator
        .ok(
            "EnableAddOn",
            json!({
                "resourceName": name,
                "addOnRequest": auto_snapshot_request(Some("05:00")),
            }),
        )
        .await;
    tokio::time::sleep(NO_COPY_WAIT).await;
    let after = copies(&emulator, name).await;
    assert_eq!(
        after,
        vec![settled.clone()],
        "case {case:?}: expected one copy for the day, still {settled:?}, got {after:?}"
    );
}

/// A delete ASKED to delete the resource's add-ons (`forceDeleteAddOns`) takes
/// the copies the add-on took with it: the name answers nothing afterwards, and
/// a resource created under the same name starts with none. What a delete that
/// was NOT asked does instead is pinned by
/// [`crate::auto_snapshot_add_on_delete`].
#[tokio::test]
async fn a_deleted_resource_takes_its_automatic_snapshots_with_it() {
    let emulator = Emulator::new();

    // (case, kind, name, the delete operation and its member, size)
    let rows: &[(&str, Kind, &str, &str, &str, i64)] = &[
        (
            "an instance deleted and created again",
            Kind::Instance,
            "auto-recreated-instance",
            "DeleteInstance",
            "instanceName",
            0,
        ),
        (
            "a disk deleted and created again",
            Kind::Disk,
            "auto-recreated-disk",
            "DeleteDisk",
            "diskName",
            16,
        ),
    ];

    for (case, kind, name, delete, member, size_in_gb) in rows {
        create(
            &emulator,
            *kind,
            name,
            &[auto_snapshot_request(Some("07:00"))],
            *size_in_gb,
        )
        .await;
        let taken = wait_for_a_copy(&emulator, case, name).await;
        assert_eq!(
            taken.len(),
            1,
            "case {case:?}: expected the add-on's copy before the delete, got {taken:?}"
        );

        emulator
            .ok(delete, json!({*member: name, "forceDeleteAddOns": true}))
            .await;
        let (status, answered) = emulator
            .call("GetAutoSnapshots", json!({"resourceName": name}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a deleted resource holds no automatic snapshot, got {answered:?}"
        );

        // The same name, created again with no add-on: the copies of the
        // resource that is gone did not survive it.
        create(&emulator, *kind, name, &[], *size_in_gb).await;
        tokio::time::sleep(NO_COPY_WAIT).await;
        let reported = copies(&emulator, name).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: expected the new resource to hold no copy, got {reported:?}"
        );
    }
}

/// A Lightsail name is unique per resource TYPE, so one name can be both an
/// instance's and a disk's. The answer is the INSTANCE's — the kind that can
/// carry every modelled add-on, and the one the add-on resolution of this
/// service answers first.
#[tokio::test]
async fn one_name_held_by_both_an_instance_and_a_disk_answers_the_instance() {
    let emulator = Emulator::new();
    let case = "one name held by both an instance and a disk";
    let name = "auto-name-collision";
    create(
        &emulator,
        Kind::Instance,
        name,
        &[auto_snapshot_request(Some("09:00"))],
        0,
    )
    .await;
    create(&emulator, Kind::Disk, name, &[], 8).await;

    let settled = wait_for_success(&emulator, case, name).await;
    let answered = report(&emulator, name).await;
    assert_names_the_resource(case, &answered, Kind::Instance, name);
    assert_eq!(
        answered["autoSnapshots"],
        json!([settled]),
        "case {case:?}: expected the instance's own copy, got {answered:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A `resourceName` that is not an instance or a disk of this account is refused
/// with the service's `DoesNotExist` envelope: a name nothing was created under,
/// a resource of a kind no automatic snapshot is taken of, and a name whose
/// resource has since been deleted.
#[tokio::test]
async fn a_name_that_is_not_an_instance_or_disk_is_refused() {
    let emulator = Emulator::new();

    // A resource of a kind that carries no automatic snapshot add-on at all.
    emulator
        .ok(
            "AllocateStaticIp",
            json!({"staticIpName": "auto-static-ip"}),
        )
        .await;
    // A snapshot is a resource of its own, and not one copies are taken OF.
    create(&emulator, Kind::Disk, "auto-snapshot-source", &[], 8).await;
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({
                "diskSnapshotName": "auto-manual-snapshot",
                "diskName": "auto-snapshot-source",
            }),
        )
        .await;
    // A resource that existed and does not any more.
    create(&emulator, Kind::Instance, "auto-gone-instance", &[], 0).await;
    emulator
        .ok("DeleteInstance", json!({"instanceName": "auto-gone-instance"}))
        .await;

    for (case, name) in [
        ("a name nothing was created under", "auto-no-such-resource"),
        (
            "a static IP, which no copy is taken of",
            "auto-static-ip",
        ),
        (
            "a disk snapshot, which is a resource of its own",
            "auto-manual-snapshot",
        ),
        ("an instance that has been deleted", "auto-gone-instance"),
    ] {
        let (status, answered) = emulator
            .call("GetAutoSnapshots", json!({"resourceName": name}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the DoesNotExist status, got {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "NotFoundException",
            "case {case:?}: expected the modelled NotFoundException, got {answered:?}"
        );
        assert_eq!(
            answered["code"].as_str(),
            Some("DoesNotExist"),
            "case {case:?}: expected the DoesNotExist code, got {answered:?}"
        );
    }
}
