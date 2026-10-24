// SPDX-License-Identifier: CC0-1.0

//! A rotation's effective start time, end to end over the wire.
//!
//! Every case drives the crate's ONE integration-test harness (`contact_lifecycle`'s
//! `Emulator`) with a buffered awsJson1_1 request (`POST /`,
//! `X-Amz-Target: SSMContacts.<Operation>`), so what is asserted is what a signing AWS
//! client would see. Each rotation is CREATED through `CreateRotation` and read back
//! through `GetRotation` and `ListRotations`, so what is asserted is what the service
//! persisted rather than anything the test placed in the store.
//!
//! ONE rule is under test, as a TABLE rather than a single call:
//!
//! > A rotation's start time is assigned ONCE, when it is created — the instant the
//! > request named, or the instant the create stored the rotation when the request named
//! > none — persisted, and reported unchanged by every operation that reports that
//! > rotation, until an update names another one.
//!
//! The rows differ along what the rule generalises over: whether the create names a
//! `StartTime` at all, and — when it does — which instant it names (the epoch itself, an
//! instant before the epoch, a sub-second instant, a far-future one), beside the
//! recurrence form and the size of the shift team, so the rule is asserted on rotations
//! that differ in everything else as well.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::contact_lifecycle::{Emulator, ACCOUNT, REGION};

/// The `ContactArn` `CreateContact` mints for the pool contact aliased `oncall-<index>`.
fn contact_arn(index: usize) -> String {
    format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/oncall-{index}")
}

/// `count` contact ARNs, which is the rotation team in shift order.
fn contact_ids(count: usize) -> Vec<String> {
    (0..count).map(contact_arn).collect()
}

/// Create `count` pool contacts, aliased so each one's minted `ContactArn` is
/// [`contact_arn`] of its index.
///
/// `CreateRotation` resolves every `ContactIds` entry against the contact store, so the
/// pool has to exist before any rotation that names it.
async fn create_pool(emulator: &Emulator, count: usize) {
    for index in 0..count {
        let (status, body) = emulator
            .call(
                "CreateContact",
                json!({
                    "Alias": format!("oncall-{index}"),
                    "Type": "PERSONAL",
                    "Plan": {"Stages": []},
                }),
            )
            .await;
        assert!(
            status == 200 || status == 409,
            "pool contact oncall-{index}: expected 200 (or 409 if an earlier row created \
             it), got {status} {body:?}"
        );
    }
}

/// Epoch seconds now, which brackets an instant the service assigns itself.
fn epoch_seconds_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// One row: a rotation to create, and the `StartTime` its create names (if any).
struct Row {
    case: &'static str,
    name: &'static str,
    /// How many pool contacts are on the team, in shift order.
    team: usize,
    time_zone_id: &'static str,
    recurrence: Value,
    /// The `StartTime` the create names, in epoch seconds as the wire carries a
    /// `DateTime`; `None` is a create that names none at all.
    start_time: Option<f64>,
}

/// The rows. `StartTime` is modelled a plain `DateTime` with no range, so the named
/// instants span the epoch itself, an instant before it, a sub-second instant and a
/// far-future one, beside a row that names none.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no StartTime at all, daily recurrence, one contact",
            name: "start-unnamed-daily",
            team: 1,
            time_zone_id: "UTC",
            recurrence: json!({
                "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
            }),
            start_time: None,
        },
        Row {
            case: "no StartTime at all, weekly recurrence with coverage, three contacts",
            name: "start-unnamed-weekly",
            team: 3,
            time_zone_id: "Asia/Seoul",
            recurrence: json!({
                "WeeklySettings": [
                    {"DayOfWeek": "WED", "HandOffTime": {"HourOfDay": 8, "MinuteOfHour": 0}},
                ],
                "ShiftCoverages": {
                    "WED": [{
                        "Start": {"HourOfDay": 8, "MinuteOfHour": 0},
                        "End": {"HourOfDay": 18, "MinuteOfHour": 0},
                    }],
                },
                "NumberOfOnCalls": 2,
                "RecurrenceMultiplier": 3,
            }),
            start_time: None,
        },
        Row {
            case: "the epoch itself as StartTime",
            name: "start-epoch",
            team: 2,
            time_zone_id: "America/Los_Angeles",
            recurrence: json!({
                "MonthlySettings": [
                    {"DayOfMonth": 1, "HandOffTime": {"HourOfDay": 0, "MinuteOfHour": 0}},
                ],
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
            }),
            start_time: Some(0.0),
        },
        Row {
            case: "an instant BEFORE the epoch as StartTime",
            name: "start-pre-epoch",
            team: 1,
            time_zone_id: "Etc/UTC",
            recurrence: json!({
                "DailySettings": [{"HourOfDay": 0, "MinuteOfHour": 0}],
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
            }),
            start_time: Some(-86_400.0),
        },
        Row {
            case: "a sub-second StartTime",
            name: "start-subsecond",
            team: 2,
            time_zone_id: "Europe/Berlin",
            recurrence: json!({
                "WeeklySettings": [
                    {"DayOfWeek": "FRI", "HandOffTime": {"HourOfDay": 17, "MinuteOfHour": 0}},
                ],
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 2,
            }),
            start_time: Some(1_700_000_000.5),
        },
        Row {
            case: "a far-future StartTime",
            name: "start-far-future",
            team: 4,
            time_zone_id: "Asia/Tokyo",
            recurrence: json!({
                "MonthlySettings": [
                    {"DayOfMonth": 28, "HandOffTime": {"HourOfDay": 23, "MinuteOfHour": 59}},
                ],
                "NumberOfOnCalls": 1,
                // `RecurrenceMultiplier` is modelled `range 1..100`: the maximum.
                "RecurrenceMultiplier": 100,
            }),
            start_time: Some(4_102_444_800.0),
        },
    ]
}

/// The `CreateRotation` body a row creates its rotation with.
fn create_request(row: &Row) -> Value {
    let mut request = json!({
        "Name": row.name,
        "ContactIds": contact_ids(row.team),
        "TimeZoneId": row.time_zone_id,
        "Recurrence": row.recurrence.clone(),
    });
    if let Some(start) = row.start_time {
        request["StartTime"] = json!(start);
    }
    request
}

/// Create a rotation, asserting the call succeeded, and answer its `RotationArn`.
async fn create_rotation(emulator: &Emulator, request: Value, case: &str) -> String {
    let (status, body) = emulator.call("CreateRotation", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateRotation to answer 200, got {status} {body:?}"
    );
    body["RotationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a RotationArn, got {body:?}"))
        .to_string()
}

/// The `StartTime` `GetRotation` reports for `arn`, asserting the member is present.
async fn get_rotation_start_time(emulator: &Emulator, arn: &str, case: &str) -> f64 {
    let (status, body) = emulator
        .call("GetRotation", json!({"RotationId": arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetRotation to answer 200, got {status} {body:?}"
    );
    body["StartTime"].as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected GetRotation to carry a StartTime, got {body:?}")
    })
}

/// The `StartTime` `ListRotations` reports in the summary of `arn`, asserting the prefix
/// selected exactly that rotation and the member is present.
async fn listed_start_time(emulator: &Emulator, name: &str, arn: &str, case: &str) -> f64 {
    let (status, body) = emulator
        .call("ListRotations", json!({"RotationNamePrefix": name}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListRotations to answer 200, got {status} {body:?}"
    );
    let summaries: Vec<Value> = body["Rotations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|summary| summary["RotationArn"].as_str() == Some(arn))
        .collect();
    assert_eq!(
        summaries.len(),
        1,
        "case {case:?}: expected the listing to report {arn} exactly once, got {body:?}"
    );
    summaries[0]["StartTime"].as_f64().unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected the summary to carry a StartTime, got {:?}",
            summaries[0]
        )
    })
}

/// A create assigns the rotation's start time, and `GetRotation` and `ListRotations`
/// report that ONE value for it — whether or not the create named an instant itself.
#[tokio::test]
async fn both_readers_report_the_start_time_the_create_assigned() {
    let emulator = Emulator::new();
    create_pool(&emulator, 4).await;

    for row in rows() {
        let case = row.case;
        let before = epoch_seconds_now();
        let arn = create_rotation(&emulator, create_request(&row), case).await;
        let after = epoch_seconds_now();

        let read = get_rotation_start_time(&emulator, &arn, case).await;
        let listed = listed_start_time(&emulator, row.name, &arn, case).await;
        assert!(
            (read - listed).abs() < f64::EPSILON,
            "case {case:?}: GetRotation and ListRotations must report one start time for \
             one rotation, got {read} and {listed}"
        );

        match row.start_time {
            // A named instant is the rotation's start time, kept as named.
            Some(named) => assert!(
                (read - named).abs() <= 1e-6,
                "case {case:?}: expected the created StartTime {named}, got {read}"
            ),
            // An unnamed one is assigned by the service, so it falls inside the window the
            // create ran in — never absent, and never the epoch default.
            None => assert!(
                read >= before && read <= after,
                "case {case:?}: expected a start time assigned inside [{before}, {after}], \
                 got {read}"
            ),
        }
    }
}

/// The assigned start time is PERSISTED, not derived per read: reading a rotation again —
/// through either reader, and after other rotations have been created — reports the same
/// value.
#[tokio::test]
async fn the_assigned_start_time_does_not_move_on_a_later_read() {
    let emulator = Emulator::new();
    create_pool(&emulator, 4).await;

    let mut first: Vec<(&'static str, &'static str, String, f64)> = Vec::new();
    for row in rows() {
        let case = row.case;
        let arn = create_rotation(&emulator, create_request(&row), case).await;
        let read = get_rotation_start_time(&emulator, &arn, case).await;
        first.push((case, row.name, arn, read));
    }

    for (case, name, arn, expected) in &first {
        let read = get_rotation_start_time(&emulator, arn, case).await;
        let listed = listed_start_time(&emulator, name, arn, case).await;
        assert!(
            (read - *expected).abs() < f64::EPSILON,
            "case {case:?}: expected GetRotation to keep reporting {expected}, got {read}"
        );
        assert!(
            (listed - *expected).abs() < f64::EPSILON,
            "case {case:?}: expected ListRotations to keep reporting {expected}, \
             got {listed}"
        );
    }
}

/// An update that names no `StartTime` leaves the assigned one alone, and one that names an
/// instant moves BOTH readers to it: the start time stays a single stored value.
#[tokio::test]
async fn an_update_moves_both_readers_together_or_neither() {
    let emulator = Emulator::new();
    create_pool(&emulator, 4).await;

    // The recurrence `UpdateRotation` requires on every call, whatever else it names.
    let recurrence = json!({
        "DailySettings": [{"HourOfDay": 12, "MinuteOfHour": 30}],
        "NumberOfOnCalls": 1,
        "RecurrenceMultiplier": 1,
    });
    // The patches differ in whether they name a `StartTime`, and in which instant: the
    // omitted case, the epoch, a pre-epoch instant and a far-future one.
    let patches: &[(&str, Option<f64>)] = &[
        ("an update naming no StartTime", None),
        ("an update naming the epoch", Some(0.0)),
        ("an update naming a pre-epoch instant", Some(-1_000.0)),
        ("an update naming a far-future instant", Some(4_102_444_800.0)),
    ];

    for row in rows() {
        for (index, (patch_case, patched_start)) in patches.iter().enumerate() {
            let case = format!("{} / {patch_case}", row.case);
            let case = case.as_str();
            let mut request = create_request(&row);
            // Each (row, patch) pair gets its own rotation, so one pair's update cannot be
            // read as another's.
            let name = format!("{}-{index}", row.name);
            request["Name"] = json!(name);
            let arn = create_rotation(&emulator, request, case).await;
            let assigned = get_rotation_start_time(&emulator, &arn, case).await;

            let mut patch = json!({"RotationId": arn.clone(), "Recurrence": recurrence.clone()});
            if let Some(start) = patched_start {
                patch["StartTime"] = json!(start);
            }
            let (status, body) = emulator.call("UpdateRotation", patch).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected UpdateRotation to answer 200, got {status} {body:?}"
            );

            let expected = patched_start.unwrap_or(assigned);
            let read = get_rotation_start_time(&emulator, &arn, case).await;
            let listed = listed_start_time(&emulator, &name, &arn, case).await;
            assert!(
                (read - expected).abs() <= 1e-6,
                "case {case:?}: expected GetRotation to report {expected}, got {read}"
            );
            assert!(
                (listed - read).abs() < f64::EPSILON,
                "case {case:?}: GetRotation and ListRotations must agree after the update, \
                 got {read} and {listed}"
            );
        }
    }
}
