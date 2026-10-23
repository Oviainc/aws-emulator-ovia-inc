// SPDX-License-Identifier: CC0-1.0

//! `GetRotation`, end to end over the wire.
//!
//! Every case drives the crate's ONE integration-test harness (`contact_lifecycle`'s
//! `Emulator`) with a buffered awsJson1_1 request (`POST /`,
//! `X-Amz-Target: SSMContacts.<Operation>`), so what is asserted is what a signing AWS
//! client would see — routing, deserialization, constraint validation, the handler and
//! serialization included. Each rotation is CREATED through `CreateRotation` and read back
//! through `GetRotation`, so the read is asserted against what the create persisted rather
//! than against anything the test placed in the store.
//!
//! Two rules are under test, and each is a TABLE rather than a single call:
//!
//! 1. THE ACCOUNT GATE. Incident Manager is fronted by a per-account replication set, and a
//!    replication-set-scoped read is admitted only while the account holds an ACTIVE one;
//!    otherwise it is refused HTTP 400 `ValidationException` BEFORE the rotation is looked
//!    up. The rows are every state the account's `replication_set` record can be in —
//!    absent, present but not active, present and active — and each is asked both for a
//!    rotation the store holds and for one it does not, which is what shows the gate is
//!    evaluated first.
//! 2. THE PROJECTION. An admitted read answers the created rotation's own members. The rows
//!    differ along what that generalises over: the recurrence form (daily, weekly, monthly
//!    with shift coverages, two hand-offs a day), the size of the shift team, the
//!    recurrence multiplier at its published bound, the time zone, and whether the create
//!    named a `StartTime` at all — the one member the response requires and the create does
//!    not.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use service_com_amazonaws_ssmcontacts_ssmcontacts::state::{
    self, AccountReplicationSet,
};

use crate::contact_lifecycle::{error_code, Emulator, ACCOUNT, REGION};
use crate::contact_updates::{active_replication_set, onboard, set_replication_set};

/// A rotation ARN this service never minted, for the "no such rotation" rows.
const ABSENT_ROTATION_ARN: &str =
    "arn:aws:ssm-contacts:us-east-1:000000000000:rotation/00000000-0000-0000-0000-000000000000";

/// A contact ARN of the form `CreateContact` mints, numbered.
fn contact_arn(index: usize) -> String {
    format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/oncall-{index}")
}

/// `count` contact ARNs, which is the rotation team in shift order.
fn contact_ids(count: usize) -> Vec<String> {
    (0..count).map(contact_arn).collect()
}

/// Create the `count` contacts a rotation team is made of, aliased `oncall-<index>` so each
/// one's minted `ContactArn` is [`contact_arn`] of that index.
///
/// `CreateRotation` resolves every `ContactIds` entry against the contact store, so the team
/// has to exist before the rotation that names it.
async fn create_team(emulator: &Emulator, count: usize, case: &str) {
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
            "case {case:?}: expected CreateContact for oncall-{index} to answer 200 (or 409 \
             if an earlier row created it), got {status} {body:?}"
        );
    }
}

/// Create a rotation and its team, asserting the call succeeded, and answer its
/// `RotationArn`.
async fn create_rotation(emulator: &Emulator, request: Value, case: &str) -> String {
    let team = request["ContactIds"]
        .as_array()
        .map_or(0, |contacts| contacts.len());
    create_team(emulator, team, case).await;
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

/// How many rotations the store holds right now.
fn rotation_count(emulator: &Emulator) -> usize {
    let store =
        state::service_state(&*emulator.engine).expect("the engine holds this service's state");
    let store = store.lock();
    store.rotations.len()
}

/// One projection row: the `CreateRotation` request, and what `GetRotation` must answer.
struct RotationRow {
    case: &'static str,
    /// The `CreateRotation` request body, built per row.
    request: fn() -> Value,
}

/// The rows. Each varies the recurrence form, the shift team's size, the multiplier, the
/// time zone and whether a `StartTime` is named — the things the projection rule
/// generalises over. `RecurrenceMultiplier` is modelled `range 1..100` and `ContactIds`
/// `length 1..30`, so the rows include both published bounds.
fn rotation_rows() -> Vec<RotationRow> {
    vec![
        RotationRow {
            case: "daily recurrence, one contact, the lowest multiplier, an explicit start",
            request: || {
                json!({
                    "Name": "daily-primary",
                    "ContactIds": contact_ids(1),
                    "TimeZoneId": "America/Los_Angeles",
                    "StartTime": 1_735_689_600,
                    "Recurrence": {
                        "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
                        "NumberOfOnCalls": 1,
                        "RecurrenceMultiplier": 1,
                    },
                })
            },
        },
        RotationRow {
            case: "weekly recurrence, three contacts, two on call at once",
            request: || {
                json!({
                    "Name": "weekly.rotation-team",
                    "ContactIds": contact_ids(3),
                    "TimeZoneId": "UTC",
                    "StartTime": 1_700_000_000,
                    "Recurrence": {
                        "WeeklySettings": [
                            {"DayOfWeek": "MON", "HandOffTime": {"HourOfDay": 8, "MinuteOfHour": 30}},
                            {"DayOfWeek": "THU", "HandOffTime": {"HourOfDay": 20, "MinuteOfHour": 45}},
                        ],
                        "NumberOfOnCalls": 2,
                        "RecurrenceMultiplier": 2,
                    },
                })
            },
        },
        RotationRow {
            case: "monthly recurrence with weekend shift coverage",
            request: || {
                json!({
                    "Name": "monthly_weekend-cover",
                    "ContactIds": contact_ids(2),
                    "TimeZoneId": "Asia/Tokyo",
                    "StartTime": 1_600_000_000,
                    "Recurrence": {
                        "MonthlySettings": [
                            {"DayOfMonth": 15, "HandOffTime": {"HourOfDay": 7, "MinuteOfHour": 0}},
                        ],
                        "ShiftCoverages": {
                            "SAT": [{
                                "Start": {"HourOfDay": 9, "MinuteOfHour": 0},
                                "End": {"HourOfDay": 17, "MinuteOfHour": 0},
                            }],
                        },
                        "NumberOfOnCalls": 1,
                        "RecurrenceMultiplier": 3,
                    },
                })
            },
        },
        RotationRow {
            case: "two daily hand-offs, the largest shift team, the highest multiplier, \
                   and NO start time",
            request: || {
                json!({
                    // `ContactIds` is modelled `length 1..30`: this row sits on the maximum.
                    "Name": "bounds",
                    "ContactIds": contact_ids(30),
                    "TimeZoneId": "Etc/UTC",
                    "Recurrence": {
                        "DailySettings": [
                            {"HourOfDay": 0, "MinuteOfHour": 0},
                            {"HourOfDay": 12, "MinuteOfHour": 30},
                        ],
                        "NumberOfOnCalls": 30,
                        // `RecurrenceMultiplier` is modelled `range 1..100`: the maximum.
                        "RecurrenceMultiplier": 100,
                    },
                })
            },
        },
    ]
}

/// Epoch seconds now, as the lower bound a server-chosen `StartTime` must not precede.
fn epoch_seconds_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// An admitted `GetRotation` answers the CREATED rotation's own members, whatever the
/// recurrence form, the team size, the time zone or whether a start time was named.
#[tokio::test]
async fn an_admitted_read_projects_the_created_rotation() {
    let emulator = Emulator::new();
    onboard(&emulator);

    for row in rotation_rows() {
        let case = row.case;
        let request = (row.request)();
        let before_create = epoch_seconds_now();
        let arn = create_rotation(&emulator, request.clone(), case).await;

        let (status, body) = emulator
            .call("GetRotation", json!({"RotationId": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetRotation to answer 200, got {status} {body:?}"
        );
        assert_eq!(
            body["RotationArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the ARN the create minted, got {body:?}"
        );
        assert_eq!(
            body["Name"], request["Name"],
            "case {case:?}: expected the created Name, got {body:?}"
        );
        assert_eq!(
            body["ContactIds"], request["ContactIds"],
            "case {case:?}: expected the created ContactIds in the created order, got {body:?}"
        );
        assert_eq!(
            body["TimeZoneId"], request["TimeZoneId"],
            "case {case:?}: expected the created TimeZoneId, got {body:?}"
        );
        assert_eq!(
            body["Recurrence"], request["Recurrence"],
            "case {case:?}: expected the created Recurrence, got {body:?}"
        );

        // `StartTime` is required on the response and optional on the create: a named one
        // is answered as created, and an unnamed one is answered as the instant the create
        // stored the rotation — never absent, and the same on every read.
        let start_time = body["StartTime"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: expected a StartTime, got {body:?}"));
        match request["StartTime"].as_f64() {
            Some(created_with) => assert!(
                (start_time - created_with).abs() < f64::EPSILON,
                "case {case:?}: expected the created StartTime {created_with}, got {start_time}"
            ),
            None => assert!(
                start_time >= before_create && start_time <= epoch_seconds_now(),
                "case {case:?}: expected the creation instant as StartTime, got {start_time}"
            ),
        }
        let (_, again) = emulator
            .call("GetRotation", json!({"RotationId": arn}))
            .await;
        assert_eq!(
            again["StartTime"], body["StartTime"],
            "case {case:?}: expected the same StartTime on a second read, got {again:?}"
        );
    }
}

/// The ACCOUNT GATE: a replication-set-scoped read is admitted only while the account holds
/// an ACTIVE replication set, and the gate is evaluated BEFORE the rotation is looked up —
/// so an un-onboarded account is refused 400 for a rotation that exists and for one that
/// does not alike, never told 404.
#[tokio::test]
async fn the_replication_set_gate_decides_before_the_rotation_is_looked_up() {
    let emulator = Emulator::new();
    onboard(&emulator);
    let row = &rotation_rows()[0];
    let live = create_rotation(&emulator, (row.request)(), row.case).await;

    // Every state the account's `replication_set` record can be in: absent (an account
    // that was never onboarded), present but not active (a set still replicating or being
    // deleted — only an ACTIVE set opens the data plane), and present and active.
    let not_active = AccountReplicationSet {
        active: false,
        ..active_replication_set()
    };
    let gate_cases = [
        ("no replication set at all", None, 400),
        (
            "a replication set that is not active",
            Some(not_active),
            400,
        ),
        (
            "an active replication set",
            Some(active_replication_set()),
            200,
        ),
    ];

    for (case, replication_set, expected_live_status) in gate_cases {
        set_replication_set(&emulator, replication_set);

        // The rotation the store HOLDS: admitted only once the account is onboarded.
        let (status, body) = emulator
            .call("GetRotation", json!({"RotationId": live}))
            .await;
        assert_eq!(
            status, expected_live_status,
            "case {case:?} on a stored rotation: expected HTTP {expected_live_status}, \
             got {status} {body:?}"
        );
        if expected_live_status == 400 {
            assert!(
                error_code(&body).contains("ValidationException"),
                "case {case:?} on a stored rotation: expected a ValidationException, got {body:?}"
            );
            // The refusal carries the SHAPE the recorded refusals of this family do: a
            // populated `Message`, a `Reason`, and `Fields` present and empty — no single
            // input member is at fault, so the list is empty rather than absent.
            assert!(
                body["Message"].as_str().is_some_and(|text| !text.is_empty())
                    && body["Reason"].as_str().is_some_and(|text| !text.is_empty())
                    && body["Fields"] == json!([]),
                "case {case:?} on a stored rotation: expected a populated Message and Reason \
                 and an empty Fields list, got {body:?}"
            );
        }

        // A rotation the store does NOT hold: 400 while the account is refused — the gate
        // decided first — and only 404 once it is admitted.
        let expected_absent_status = if expected_live_status == 200 {
            404
        } else {
            400
        };
        let (status, body) = emulator
            .call("GetRotation", json!({"RotationId": ABSENT_ROTATION_ARN}))
            .await;
        assert_eq!(
            status, expected_absent_status,
            "case {case:?} on an absent rotation: expected HTTP {expected_absent_status}, \
             got {status} {body:?}"
        );
        let expected_code = if expected_absent_status == 404 {
            "ResourceNotFoundException"
        } else {
            "ValidationException"
        };
        assert!(
            error_code(&body).contains(expected_code),
            "case {case:?} on an absent rotation: expected a {expected_code}, got {body:?}"
        );
    }
}

/// `GetRotation` REPORTS: it writes nothing, so two reads of an unchanged rotation answer
/// identically, the store keeps exactly what it held, and the account's onboarding is not
/// consumed by having been read.
#[tokio::test]
async fn a_read_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    onboard(&emulator);
    let row = &rotation_rows()[1];
    let arn = create_rotation(&emulator, (row.request)(), row.case).await;

    let (first_status, first) = emulator
        .call("GetRotation", json!({"RotationId": arn}))
        .await;
    // A refused read in between must not disturb anything either.
    let (_, _) = emulator
        .call("GetRotation", json!({"RotationId": ABSENT_ROTATION_ARN}))
        .await;
    let (second_status, second) = emulator
        .call("GetRotation", json!({"RotationId": arn}))
        .await;

    assert_eq!(
        (first_status, &first),
        (second_status, &second),
        "case \"two reads of an unchanged rotation\": expected identical answers, got \
         {first_status} {first:?} then {second_status} {second:?}"
    );
    assert_eq!(
        rotation_count(&emulator),
        1,
        "case \"two reads of an unchanged rotation\": expected the store untouched, got {} \
         rotation(s)",
        rotation_count(&emulator)
    );

    let store = state::service_state(&*emulator.engine)
        .expect("the engine holds this service's state");
    let still_onboarded = store.lock().active_replication_set().is_some();
    assert!(
        still_onboarded,
        "case \"two reads of an unchanged rotation\": expected the account still onboarded"
    );
}

/// Every recurrence form the rows cover is a distinct `Recurrence` shape, so the projection
/// table cannot pass by accident on one shape repeated.
#[tokio::test]
async fn the_projection_rows_cover_distinct_recurrence_shapes() {
    let mut seen: HashMap<String, &str> = HashMap::new();
    for row in rotation_rows() {
        let recurrence = (row.request)()["Recurrence"].to_string();
        if let Some(previous) = seen.insert(recurrence.clone(), row.case) {
            panic!(
                "case {:?}: its Recurrence is the same shape as case {previous:?} \
                 ({recurrence})",
                row.case
            );
        }
    }
}
