// SPDX-License-Identifier: CC0-1.0

//! The daily time an enabled automatic-snapshot add-on reports its NEXT copy
//! for, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: an add-on reports, as `nextSnapshotTimeOfDay`, the UTC
//! hour (`HH:00`) the next automatic copy the service owes the resource falls in
//! — the modelled member is "the next daily time an automatic snapshot will be
//! created" — and reports none when the service owes that resource no copy. So
//! an enabled `AutoSnapshot` add-on whose schedule has come round onto its own
//! daily hour names that hour, a freshly enabled one names the hour its first
//! copy lands in, one whose add-on has just been modified still names the copy
//! already owed rather than the hour the request named, and an add-on no copy is
//! owed for (the idle-stop add-on, or an automatic-snapshot add-on that has been
//! disabled) names nothing. It holds for every resource an add-on can be enabled
//! on and through every reader of one — `GetInstance`, `GetInstances`, `GetDisk`,
//! `GetDisks` and the disks an instance reports in its hardware — and a read
//! never changes it.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

// The harness, the modelled `TimeOfDay` check, the resource kind, the create and
// add-on readers and the copy wait this test target already exposes: this suite
// drives the service through the same front door and reads the same values
// rather than keeping second copies of any of them.
use crate::auto_snapshots::{auto_snapshot_request, create, wait_for_a_copy, Kind};
use crate::disk_block_storage::is_modelled_time_of_day;
use crate::instance_lifecycle::Emulator;
use crate::resource_add_ons::add_ons;

/// The size of every block storage disk in this suite: the smallest Lightsail
/// publishes (8 GB), since the size selects nothing about an add-on.
const DISK_SIZE_IN_GB: i64 = 8;

/// How long a case waits for the clock to take a copy or settle a disk.
const WAIT: Duration = Duration::from_secs(5);

/// The modelled `AddOnType` members: the add-on that takes the daily copy, and
/// the one that stops an idle virtual computer and takes no copy at all.
const AUTO_SNAPSHOT: &str = "AutoSnapshot";
const STOP_INSTANCE_ON_IDLE: &str = "StopInstanceOnIdle";

/// The current instant in epoch seconds, so a case can say which UTC hour its
/// own call was made in.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// The UTC hour of the epoch-second instant `secs`, in the `HH:00` form the
/// modelled automatic-snapshot members report ("in `HH:00` format, and in
/// Coordinated Universal Time (UTC)").
///
/// Derived through the same library rendering the service's own timestamps use
/// (`DateTime::fmt(Format::DateTime)` is RFC 3339 in UTC), so the case asserts
/// the RELATION the rule states rather than re-deriving a calendar.
fn utc_hour_of(secs: f64) -> String {
    let rendered = ::aws_smithy_types::DateTime::from_secs_f64(secs)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    let time = rendered.split_once('T').map(|(_, time)| time).unwrap_or("");
    let hour = time.split_once(':').map(|(hour, _)| hour).unwrap_or("");
    format!("{hour}:00")
}

/// The add-on of type `add_on_type` among `reported`, or `None` when the
/// resource carries none of that type.
fn add_on_of_type(reported: &[Value], add_on_type: &str) -> Option<Value> {
    reported
        .iter()
        .find(|add_on| add_on["name"].as_str() == Some(add_on_type))
        .cloned()
}

/// The `nextSnapshotTimeOfDay` the automatic-snapshot add-on of the resource
/// `name` of `kind` reports, requiring that the resource carries that add-on.
async fn next_time(emulator: &Emulator, case: &str, kind: Kind, name: &str) -> Option<String> {
    let reported = add_ons(emulator, kind, name).await;
    let add_on = add_on_of_type(&reported, AUTO_SNAPSHOT).unwrap_or_else(|| {
        panic!("case {case:?}: expected the resource to carry the {AUTO_SNAPSHOT} add-on, got {reported:?}")
    });
    add_on["nextSnapshotTimeOfDay"]
        .as_str()
        .map(str::to_string)
}

/// Enable `request` on the resource `name`, requiring success.
async fn enable(emulator: &Emulator, name: &str, request: Value) {
    emulator
        .ok(
            "EnableAddOn",
            json!({"resourceName": name, "addOnRequest": request}),
        )
        .await;
}

/// Assert that `next` is a time the modelled `TimeOfDay` pattern admits on the
/// hourly increment the automatic-snapshot members are reported on.
fn assert_hourly(case: &str, next: &str, reported: &[Value]) {
    assert!(
        is_modelled_time_of_day(next) && next.ends_with(":00"),
        "case {case:?}: expected an hourly HH:00 next snapshot time, got {next:?} in \
         {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// The schedule the add-on is on
// ---------------------------------------------------------------------------

/// Which call enabled the add-on, so the rule is asserted on both paths that
/// start a schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnabledBy {
    /// The create call carried the add-on, with the hour it names.
    Create(Option<&'static str>),
    /// A later `EnableAddOn` call enabled it, with the hour it names.
    EnableAddOn(Option<&'static str>),
}

/// One resource whose enabled add-on must report the next daily copy it is owed.
///
/// The rows differ along everything the rule generalises over: the KIND of
/// resource the add-on is enabled on, which call enabled it, whether that call
/// named the daily hour at all, and — when it did — the hour itself, including
/// each published bound of the modelled `TimeOfDay` pattern
/// `^(0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]$` on an hourly increment and hours
/// between them that the issue did not cite.
struct Row {
    case: &'static str,
    kind: Kind,
    /// Distinct per row, so no row can see another's resource.
    name: &'static str,
    enabled_by: EnabledBy,
}

/// The table. `00:00` and `23:00` are the first and last hour the modelled
/// pattern admits on an hourly increment; a row that names no hour takes the one
/// the service picks for itself, which the rule must follow just the same.
const ROWS: &[Row] = &[
    Row {
        case: "an instance whose create enabled the add-on at the first hour of the day",
        kind: Kind::Instance,
        name: "next-time-instance-first-hour",
        enabled_by: EnabledBy::Create(Some("00:00")),
    },
    Row {
        case: "an instance whose create enabled the add-on at the last hour of the day",
        kind: Kind::Instance,
        name: "next-time-instance-last-hour",
        enabled_by: EnabledBy::Create(Some("23:00")),
    },
    Row {
        case: "an instance whose create named no hour, so the service picked one",
        kind: Kind::Instance,
        name: "next-time-instance-service-chosen",
        enabled_by: EnabledBy::Create(None),
    },
    Row {
        case: "an instance whose add-on a later EnableAddOn enabled at a midday hour",
        kind: Kind::Instance,
        name: "next-time-instance-enabled-afterwards",
        enabled_by: EnabledBy::EnableAddOn(Some("13:00")),
    },
    Row {
        case: "a disk whose create enabled the add-on at an early hour",
        kind: Kind::Disk,
        name: "next-time-disk-created-with-add-on",
        enabled_by: EnabledBy::Create(Some("04:00")),
    },
    Row {
        case: "a disk whose add-on a later EnableAddOn enabled at a late hour",
        kind: Kind::Disk,
        name: "next-time-disk-enabled-afterwards",
        enabled_by: EnabledBy::EnableAddOn(Some("19:00")),
    },
    Row {
        case: "a disk whose EnableAddOn named no hour, so the service picked one",
        kind: Kind::Disk,
        name: "next-time-disk-service-chosen",
        enabled_by: EnabledBy::EnableAddOn(None),
    },
];

/// Every row of the table: once the add-on's schedule has come round onto its own
/// daily hour — which it does as soon as the clock has taken a copy — the next
/// daily time the add-on reports IS that hour, whichever hour it is, on both
/// kinds of resource and whichever call enabled the add-on.
#[tokio::test]
async fn an_enabled_add_on_reports_the_daily_hour_its_next_copy_is_due_at() {
    let emulator = Emulator::new();
    for row in ROWS {
        let case = row.case;
        match row.enabled_by {
            EnabledBy::Create(hour) => {
                create(
                    &emulator,
                    row.kind,
                    row.name,
                    &[auto_snapshot_request(hour)],
                    DISK_SIZE_IN_GB,
                )
                .await;
            }
            EnabledBy::EnableAddOn(hour) => {
                create(&emulator, row.kind, row.name, &[], DISK_SIZE_IN_GB).await;
                enable(&emulator, row.name, auto_snapshot_request(hour)).await;
            }
        }

        // The copy the add-on owes the resource first is what brings its
        // schedule onto the daily hour, and the CLOCK takes it.
        wait_for_a_copy(&emulator, case, row.name).await;

        let reported = add_ons(&emulator, row.kind, row.name).await;
        let add_on = add_on_of_type(&reported, AUTO_SNAPSHOT).unwrap_or_else(|| {
            panic!("case {case:?}: expected the {AUTO_SNAPSHOT} add-on, got {reported:?}")
        });
        let scheduled = add_on["snapshotTimeOfDay"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let next = add_on["nextSnapshotTimeOfDay"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_hourly(case, &next, &reported);
        assert_eq!(
            next, scheduled,
            "case {case:?}: a daily copy is next due at the hour the add-on is scheduled \
             at ({scheduled}), got {reported:?}"
        );

        // Reading is not scheduling: a second read reports exactly the same
        // add-on, next daily time included.
        let again = add_ons(&emulator, row.kind, row.name).await;
        assert_eq!(
            again, reported,
            "case {case:?}: expected every read to report {reported:?}, got {again:?}"
        );
    }
}

/// A freshly enabled add-on is owed its FIRST copy a short span after the call
/// that enabled it, so the next daily time it reports names the hour that copy
/// lands in — the hour of the enabling call, or the hour it rolls into — and once
/// the clock has taken that copy the schedule has come round onto the add-on's
/// own hour. The member is never absent while the add-on is enabled.
#[tokio::test]
async fn a_freshly_enabled_add_on_names_the_hour_its_first_copy_lands_in() {
    let emulator = Emulator::new();
    /// A daily hour no row's enabling call is made in unless the suite happens
    /// to run in it — the point of the case is that the FIRST copy does not wait
    /// for the daily hour.
    const HOUR: &str = "07:00";

    for (kind, name) in [
        (Kind::Instance, "next-time-fresh-instance"),
        (Kind::Disk, "next-time-fresh-disk"),
    ] {
        let case = format!("{kind:?} whose add-on was just enabled");
        let before = now_secs();
        create(
            &emulator,
            kind,
            name,
            &[auto_snapshot_request(Some(HOUR))],
            DISK_SIZE_IN_GB,
        )
        .await;
        let reported = add_ons(&emulator, kind, name).await;
        let after = now_secs();

        let add_on = add_on_of_type(&reported, AUTO_SNAPSHOT).unwrap_or_else(|| {
            panic!("case {case:?}: expected the {AUTO_SNAPSHOT} add-on, got {reported:?}")
        });
        let next = add_on["nextSnapshotTimeOfDay"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_hourly(&case, &next, &reported);
        // The hour the call was made in, the hour the read happened in (the two
        // differ only when the read crossed an hour boundary), and the add-on's
        // own hour — which is what the schedule has come round onto if the clock
        // already took the first copy.
        let admitted = [utc_hour_of(before), utc_hour_of(after), HOUR.to_string()];
        assert!(
            admitted.contains(&next),
            "case {case:?}: expected the hour the owed copy lands in, one of {admitted:?}, \
             got {reported:?}"
        );
    }
}

/// Modifying the add-on does not restart the wait, so the next daily time keeps
/// naming the copy the service ALREADY owes the resource rather than the hour the
/// new request named: the member reports the schedule the service is on, and the
/// two members can therefore differ.
#[tokio::test]
async fn modifying_the_add_on_keeps_the_copy_already_owed() {
    let emulator = Emulator::new();
    /// The hour the schedule is on, and the hour it is moved to.
    const SCHEDULED: &str = "05:00";
    const MOVED_TO: &str = "17:00";

    for (kind, name) in [
        (Kind::Instance, "next-time-modified-instance"),
        (Kind::Disk, "next-time-modified-disk"),
    ] {
        let case = format!("{kind:?} whose add-on hour was changed");
        create(
            &emulator,
            kind,
            name,
            &[auto_snapshot_request(Some(SCHEDULED))],
            DISK_SIZE_IN_GB,
        )
        .await;
        wait_for_a_copy(&emulator, &case, name).await;
        assert_eq!(
            next_time(&emulator, &case, kind, name).await.as_deref(),
            Some(SCHEDULED),
            "case {case:?}: expected the scheduled hour before the change"
        );

        enable(&emulator, name, auto_snapshot_request(Some(MOVED_TO))).await;
        let reported = add_ons(&emulator, kind, name).await;
        let add_on = add_on_of_type(&reported, AUTO_SNAPSHOT).unwrap_or_else(|| {
            panic!("case {case:?}: expected the {AUTO_SNAPSHOT} add-on, got {reported:?}")
        });
        assert_eq!(
            add_on["snapshotTimeOfDay"].as_str(),
            Some(MOVED_TO),
            "case {case:?}: the daily hour is the one the request named, got {reported:?}"
        );
        assert_eq!(
            add_on["nextSnapshotTimeOfDay"].as_str(),
            Some(SCHEDULED),
            "case {case:?}: the next copy is the one already owed, at {SCHEDULED}, got \
             {reported:?}"
        );
    }
}

/// An add-on enabled again after it was disabled is owed its next copy at its own
/// daily hour — the copy for the day it already holds one for is not taken twice
/// — so the member names that hour and is never absent on an enabled add-on.
#[tokio::test]
async fn a_re_enabled_add_on_is_owed_its_next_daily_copy() {
    let emulator = Emulator::new();
    /// The hour the add-on is enabled at, both times.
    const HOUR: &str = "11:00";

    for (kind, name) in [
        (Kind::Instance, "next-time-recycled-instance"),
        (Kind::Disk, "next-time-recycled-disk"),
    ] {
        let case = format!("{kind:?} whose add-on was disabled and enabled again");
        create(
            &emulator,
            kind,
            name,
            &[auto_snapshot_request(Some(HOUR))],
            DISK_SIZE_IN_GB,
        )
        .await;
        wait_for_a_copy(&emulator, &case, name).await;

        emulator
            .ok(
                "DisableAddOn",
                json!({"resourceName": name, "addOnType": AUTO_SNAPSHOT}),
            )
            .await;
        let disabled = add_ons(&emulator, kind, name).await;
        assert!(
            add_on_of_type(&disabled, AUTO_SNAPSHOT).is_none(),
            "case {case:?}: a disabled add-on is not reported at all, got {disabled:?}"
        );

        enable(&emulator, name, auto_snapshot_request(Some(HOUR))).await;
        assert_eq!(
            next_time(&emulator, &case, kind, name).await.as_deref(),
            Some(HOUR),
            "case {case:?}: expected the add-on's own daily hour once the day's copy is held"
        );
    }
}

/// An add-on that takes no automatic snapshot is owed no copy, so it reports no
/// next daily time — and enabling it beside the automatic-snapshot add-on leaves
/// that one's time alone.
#[tokio::test]
async fn an_add_on_that_takes_no_snapshot_reports_no_next_daily_time() {
    let emulator = Emulator::new();
    let case = "an instance carrying both modelled add-ons";
    let name = "next-time-both-add-ons";
    /// The hour the automatic-snapshot add-on is enabled at.
    const HOUR: &str = "08:00";

    create(&emulator, Kind::Instance, name, &[], DISK_SIZE_IN_GB).await;
    enable(
        &emulator,
        name,
        json!({
            "addOnType": STOP_INSTANCE_ON_IDLE,
            "stopInstanceOnIdleRequest": {"threshold": "20", "duration": "30"},
        }),
    )
    .await;

    let idle_only = add_ons(&emulator, Kind::Instance, name).await;
    let idle = add_on_of_type(&idle_only, STOP_INSTANCE_ON_IDLE)
        .unwrap_or_else(|| panic!("case {case:?}: expected the idle-stop add-on, got {idle_only:?}"));
    assert!(
        idle["nextSnapshotTimeOfDay"].is_null(),
        "case {case:?}: an add-on that takes no snapshot has no next snapshot time, got \
         {idle:?}"
    );

    enable(&emulator, name, auto_snapshot_request(Some(HOUR))).await;
    wait_for_a_copy(&emulator, case, name).await;
    let both = add_ons(&emulator, Kind::Instance, name).await;
    assert_eq!(
        both.len(),
        2,
        "case {case:?}: expected the resource to carry both add-ons, got {both:?}"
    );
    let idle = add_on_of_type(&both, STOP_INSTANCE_ON_IDLE)
        .unwrap_or_else(|| panic!("case {case:?}: expected the idle-stop add-on, got {both:?}"));
    assert!(
        idle["nextSnapshotTimeOfDay"].is_null(),
        "case {case:?}: only the add-on that takes copies reports a next one, got {idle:?}"
    );
    assert_eq!(
        next_time(&emulator, case, Kind::Instance, name).await.as_deref(),
        Some(HOUR),
        "case {case:?}: expected the automatic-snapshot add-on's own daily hour"
    );
}

/// Every reader of a resource reports the same next daily time: the single reads,
/// the listings, and the disks an instance reports in its hardware, where a disk
/// carries the schedule of its OWN add-on.
#[tokio::test]
async fn every_reader_reports_the_same_next_daily_time() {
    let emulator = Emulator::new();
    let case = "an instance with an attached disk, each carrying its own add-on";
    let instance = "next-time-readers-instance";
    let disk = "next-time-readers-disk";
    /// The hour each resource's add-on is scheduled at; the two differ, so a
    /// reader reporting the wrong resource's schedule is visible.
    const INSTANCE_HOUR: &str = "02:00";
    const DISK_HOUR: &str = "21:00";

    create(
        &emulator,
        Kind::Instance,
        instance,
        &[auto_snapshot_request(Some(INSTANCE_HOUR))],
        DISK_SIZE_IN_GB,
    )
    .await;
    create(
        &emulator,
        Kind::Disk,
        disk,
        &[auto_snapshot_request(Some(DISK_HOUR))],
        DISK_SIZE_IN_GB,
    )
    .await;

    // A disk is attachable once the clock has settled it into `available`.
    let deadline = Instant::now() + WAIT;
    loop {
        let state = emulator.ok("GetDisk", json!({"diskName": disk})).await["disk"]["state"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if state == "available" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {disk} available within {WAIT:?}, got {state:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    emulator
        .ok(
            "AttachDisk",
            json!({"diskName": disk, "instanceName": instance, "diskPath": "/dev/xvdf"}),
        )
        .await;

    wait_for_a_copy(&emulator, case, instance).await;
    wait_for_a_copy(&emulator, case, disk).await;

    // The single reads.
    assert_eq!(
        next_time(&emulator, case, Kind::Instance, instance)
            .await
            .as_deref(),
        Some(INSTANCE_HOUR),
        "case {case:?}: expected GetInstance to report the instance's own schedule"
    );
    assert_eq!(
        next_time(&emulator, case, Kind::Disk, disk).await.as_deref(),
        Some(DISK_HOUR),
        "case {case:?}: expected GetDisk to report the disk's own schedule"
    );

    // The listings, which render the same stored records.
    let listed_instance = named(
        &emulator.ok("GetInstances", json!({})).await["instances"],
        instance,
    )
    .unwrap_or_else(|| panic!("case {case:?}: expected GetInstances to report {instance}"));
    let listed_disk = named(&emulator.ok("GetDisks", json!({})).await["disks"], disk)
        .unwrap_or_else(|| panic!("case {case:?}: expected GetDisks to report {disk}"));
    assert_eq!(
        next_of(&listed_instance, AUTO_SNAPSHOT).as_deref(),
        Some(INSTANCE_HOUR),
        "case {case:?}: expected GetInstances to report the same schedule, got \
         {listed_instance:?}"
    );
    assert_eq!(
        next_of(&listed_disk, AUTO_SNAPSHOT).as_deref(),
        Some(DISK_HOUR),
        "case {case:?}: expected GetDisks to report the same schedule, got {listed_disk:?}"
    );

    // The attached disk inside the instance's hardware carries the DISK's own
    // schedule, not the instance's.
    let read = emulator
        .ok("GetInstance", json!({"instanceName": instance}))
        .await["instance"]
        .clone();
    let attached = named(&read["hardware"]["disks"], disk).unwrap_or_else(|| {
        panic!("case {case:?}: expected the instance's hardware to report {disk}, got {read:?}")
    });
    assert_eq!(
        next_of(&attached, AUTO_SNAPSHOT).as_deref(),
        Some(DISK_HOUR),
        "case {case:?}: expected the attached disk's own schedule, got {attached:?}"
    );
}

/// The entry of the reported list `list` whose `name` is `name`.
fn named(list: &Value, name: &str) -> Option<Value> {
    list.as_array()?
        .iter()
        .find(|entry| entry["name"].as_str() == Some(name))
        .cloned()
}

/// The `nextSnapshotTimeOfDay` the add-on of type `add_on_type` of the reported
/// `resource` carries.
fn next_of(resource: &Value, add_on_type: &str) -> Option<String> {
    let reported = resource["addOns"].as_array().cloned().unwrap_or_default();
    add_on_of_type(&reported, add_on_type)?["nextSnapshotTimeOfDay"]
        .as_str()
        .map(str::to_string)
}
