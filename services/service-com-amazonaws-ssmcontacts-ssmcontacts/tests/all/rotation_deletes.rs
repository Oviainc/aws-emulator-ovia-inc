// SPDX-License-Identifier: CC0-1.0

//! `DeleteRotation`, end to end over the wire.
//!
//! Every case drives the crate's ONE integration-test harness (`contact_lifecycle`'s
//! `Emulator`) with a buffered awsJson1_1 request (`POST /`,
//! `X-Amz-Target: SSMContacts.<Operation>`), so what is asserted is what a signing AWS
//! client would see — routing, deserialization, constraint validation, the handler and
//! serialization included. Every rotation, contact and on-call schedule is CREATED through
//! the service's own creates and read back through `GetRotation`/`ListRotations`/
//! `GetContact`, so what is asserted is what the delete did to persisted state rather than
//! anything the test placed in the store.
//!
//! Three rules are under test, each as a TABLE rather than a single call:
//!
//! 1. THE DELETE. An admitted `DeleteRotation` removes exactly the rotation its
//!    `RotationId` names — that rotation stops being readable and stops being listed, every
//!    other rotation is untouched — and removes its ARN from the plan of every contact that
//!    referenced it ("if a rotation belongs to more than one on-call schedule, this
//!    operation deletes it from all of them"), leaving the references to the rotations that
//!    remain in place. The rows differ along what that generalises over: the recurrence
//!    form, the size of the shift team, how many rotations the store holds beside the one
//!    deleted, how many on-call schedules name the deleted rotation, and whether those
//!    schedules name the surviving rotations too.
//! 2. A ROTATION THE STORE DOES NOT HOLD is refused HTTP 404 `ResourceNotFoundException`
//!    and changes nothing, over every way an ARN can fail to name a stored rotation: one
//!    this service never minted, one that names a resource of another type (a contact), and
//!    one that was deleted by an earlier call.
//! 3. THE ACCOUNT GATE. The replication-set precondition is evaluated BEFORE `RotationId`
//!    is resolved, so an account with no ACTIVE replication set is refused HTTP 400
//!    `ValidationException` for a rotation the store holds and for one it does not alike —
//!    and the refused delete removes nothing.

use serde_json::{json, Value};

use service_com_amazonaws_ssmcontacts_ssmcontacts::state::{self, AccountReplicationSet};

use crate::contact_lifecycle::{error_code, parse, Emulator};
use crate::contact_updates::{active_replication_set, set_replication_set};

/// A rotation ARN this service never minted, for the "no such rotation" rows.
const ABSENT_ROTATION_ARN: &str =
    "arn:aws:ssm-contacts:us-east-1:000000000000:rotation/00000000-0000-0000-0000-000000000000";

// ---------------------------------------------------------------------------
// The recurrence catalog
// ---------------------------------------------------------------------------
//
// The three forms `RecurrenceSettings` admits — `DailySettings`, `WeeklySettings` and
// `MonthlySettings` (the members `smithy_query(query="shape",shape="RecurrenceSettings")`
// lists) — each with the two members the model makes required, `NumberOfOnCalls` and
// `RecurrenceMultiplier` (range 1..100).

/// A daily recurrence, on the lowest multiplier.
const DAILY: &str = r#"{
    "NumberOfOnCalls": 1,
    "RecurrenceMultiplier": 1,
    "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}]
}"#;

/// A weekly recurrence with two hand-offs.
const WEEKLY: &str = r#"{
    "NumberOfOnCalls": 1,
    "RecurrenceMultiplier": 2,
    "WeeklySettings": [
        {"DayOfWeek": "MON", "HandOffTime": {"HourOfDay": 8, "MinuteOfHour": 30}},
        {"DayOfWeek": "THU", "HandOffTime": {"HourOfDay": 20, "MinuteOfHour": 45}}
    ]
}"#;

/// A monthly recurrence on the published maximum multiplier (`RecurrenceMultiplier`,
/// range 1..100).
const MONTHLY: &str = r#"{
    "NumberOfOnCalls": 1,
    "RecurrenceMultiplier": 100,
    "MonthlySettings": [{"DayOfMonth": 15, "HandOffTime": {"HourOfDay": 7, "MinuteOfHour": 0}}]
}"#;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Create `count` contacts to be a rotation's shift team, and answer their `ContactArn`s in
/// the order they were created — which is the shift order a rotation stores them in.
async fn create_team(emulator: &Emulator, count: usize, case: &str) -> Vec<String> {
    let mut arns = Vec::new();
    for index in 0..count {
        let arn = emulator
            .create_contact(
                json!({
                    "Alias": format!("member-{index}"),
                    "Type": "PERSONAL",
                    "Plan": {"Stages": []},
                }),
                case,
            )
            .await;
        arns.push(arn);
    }
    arns
}

/// Create a rotation over `team`, asserting the call succeeded, and answer its
/// `RotationArn`.
async fn create_rotation(
    emulator: &Emulator,
    name: &str,
    team: &[String],
    recurrence: &str,
    case: &str,
) -> String {
    let (status, body) = emulator
        .call(
            "CreateRotation",
            json!({
                "Name": name,
                "ContactIds": team,
                "TimeZoneId": "UTC",
                "Recurrence": parse(recurrence),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateRotation {name:?} to answer 200, got {status} {body:?}"
    );
    body["RotationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a RotationArn, got {body:?}"))
        .to_string()
}

/// Create an ONCALL_SCHEDULE contact whose `Plan` names `rotations`, and answer its
/// `ContactArn`.
async fn create_schedule(
    emulator: &Emulator,
    alias: &str,
    rotations: &[String],
    case: &str,
) -> String {
    emulator
        .create_contact(
            json!({
                "Alias": alias,
                "Type": "ONCALL_SCHEDULE",
                "Plan": {"RotationIds": rotations},
            }),
            case,
        )
        .await
}

/// Every `RotationArn` `ListRotations` reports, paging to exhaustion.
async fn listed_rotation_arns(emulator: &Emulator, case: &str) -> Vec<String> {
    let mut request = json!({});
    let mut arns: Vec<String> = Vec::new();
    loop {
        let (status, body) = emulator.call("ListRotations", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListRotations to answer 200, got {status} {body:?}"
        );
        for rotation in body["Rotations"].as_array().into_iter().flatten() {
            arns.push(
                rotation["RotationArn"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a RotationArn in {rotation:?}")
                    })
                    .to_string(),
            );
        }
        match body["NextToken"].as_str() {
            Some(token) => request["NextToken"] = json!(token),
            None => return arns,
        }
    }
}

/// The `RotationIds` the stored plan of `contact_arn` carries, read back through
/// `GetContact`.
async fn plan_rotation_ids(emulator: &Emulator, contact_arn: &str, case: &str) -> Vec<String> {
    let (status, body) = emulator
        .call("GetContact", json!({"ContactId": contact_arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetContact to answer 200, got {status} {body:?}"
    );
    body["Plan"]["RotationIds"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|arn| {
            arn.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an ARN, got {body:?}"))
                .to_string()
        })
        .collect()
}

/// How many rotations the store holds right now.
fn rotation_count(emulator: &Emulator) -> usize {
    let store =
        state::service_state(&*emulator.engine).expect("the engine holds this service's state");
    let store = store.lock();
    store.rotations.len()
}

// ---------------------------------------------------------------------------
// 1. The delete
// ---------------------------------------------------------------------------

/// One delete row: the rotation to delete, what else the store holds, and who references it.
struct DeleteRow {
    case: &'static str,
    /// The recurrence every rotation in this row is created with.
    recurrence: &'static str,
    /// The size of the shift team (`ContactIds`, modelled `length` 1..30).
    team: usize,
    /// How many rotations the store holds BESIDE the one deleted.
    siblings: usize,
    /// How many ONCALL_SCHEDULE contacts name the deleted rotation in their plan.
    schedules: usize,
    /// Whether those schedules name the surviving rotations too, so the delete is shown to
    /// drop only the reference to the rotation it removed.
    schedules_name_siblings: bool,
}

/// The rows. Each varies the recurrence form, the team size, how many other rotations the
/// store holds, how many on-call schedules reference the deleted rotation and whether those
/// schedules also reference the survivors — the things the delete rule generalises over.
const DELETE_ROWS: &[DeleteRow] = &[
    DeleteRow {
        case: "the only rotation in the store, named by no schedule",
        recurrence: DAILY,
        team: 1,
        siblings: 0,
        schedules: 0,
        schedules_name_siblings: false,
    },
    DeleteRow {
        case: "one of three rotations, named by one on-call schedule",
        recurrence: WEEKLY,
        team: 3,
        siblings: 2,
        schedules: 1,
        schedules_name_siblings: false,
    },
    DeleteRow {
        case: "a rotation named by three on-call schedules, each naming the survivors too",
        recurrence: MONTHLY,
        team: 2,
        siblings: 2,
        schedules: 3,
        schedules_name_siblings: true,
    },
    DeleteRow {
        case: "the largest shift team, one sibling, two schedules naming only the deleted one",
        recurrence: DAILY,
        // `ContactIds` is modelled `length` 1..30: this row sits on the maximum.
        team: 30,
        siblings: 1,
        schedules: 2,
        schedules_name_siblings: false,
    },
];

/// An admitted `DeleteRotation` removes exactly the rotation it names — it stops being
/// readable and stops being listed — leaves every other rotation in place, and drops its ARN
/// from the plan of every on-call schedule that named it while leaving the references to the
/// surviving rotations alone.
#[tokio::test]
async fn a_delete_removes_the_rotation_and_every_plan_reference_to_it() {
    for row in DELETE_ROWS {
        let case = row.case;
        let emulator = Emulator::new();

        let team = create_team(&emulator, row.team, case).await;
        let target = create_rotation(&emulator, "target", &team, row.recurrence, case).await;
        let mut survivors = Vec::new();
        for index in 0..row.siblings {
            survivors.push(
                create_rotation(
                    &emulator,
                    &format!("sibling-{index}"),
                    &team,
                    row.recurrence,
                    case,
                )
                .await,
            );
        }

        let mut planned: Vec<String> = vec![target.clone()];
        if row.schedules_name_siblings {
            planned.extend(survivors.iter().cloned());
        }
        let mut schedules = Vec::new();
        for index in 0..row.schedules {
            schedules
                .push(create_schedule(&emulator, &format!("schedule-{index}"), &planned, case).await);
        }

        // Before: the rotation is readable, and every schedule's plan names it.
        let (status, body) = emulator
            .call("GetRotation", json!({"RotationId": target}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the rotation readable before the delete, got {status} \
             {body:?}"
        );
        for schedule in &schedules {
            let ids = plan_rotation_ids(&emulator, schedule, case).await;
            assert!(
                ids.contains(&target),
                "case {case:?}: expected {schedule} to name the rotation before the delete, \
                 got {ids:?}"
            );
        }

        let (status, body) = emulator
            .call("DeleteRotation", json!({"RotationId": target}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteRotation to answer 200, got {status} {body:?}"
        );
        assert!(
            error_code(&body).is_empty(),
            "case {case:?}: expected a success body carrying no error code, got {body:?}"
        );

        // The rotation is gone: not readable, not listed, not in the store.
        let (status, body) = emulator
            .call("GetRotation", json!({"RotationId": target}))
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected GetRotation on the deleted rotation to answer 404, got \
             {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {body:?}"
        );

        let listed = listed_rotation_arns(&emulator, case).await;
        assert!(
            !listed.contains(&target),
            "case {case:?}: expected the deleted rotation to stop being listed, got {listed:?}"
        );
        for survivor in &survivors {
            assert!(
                listed.contains(survivor),
                "case {case:?}: expected the untouched rotation {survivor} still listed, got \
                 {listed:?}"
            );
        }
        assert_eq!(
            rotation_count(&emulator),
            row.siblings,
            "case {case:?}: expected exactly the {} untouched rotation(s) left in the store, \
             got {}",
            row.siblings,
            rotation_count(&emulator)
        );

        // Every plan that named it lost the reference — and kept the others.
        for schedule in &schedules {
            let ids = plan_rotation_ids(&emulator, schedule, case).await;
            assert!(
                !ids.contains(&target),
                "case {case:?}: expected the plan of {schedule} to drop the deleted rotation, \
                 got {ids:?}"
            );
            let expected_rest: Vec<String> = if row.schedules_name_siblings {
                survivors.clone()
            } else {
                Vec::new()
            };
            assert_eq!(
                ids, expected_rest,
                "case {case:?}: expected the plan of {schedule} to keep exactly the surviving \
                 rotations {expected_rest:?}, got {ids:?}"
            );
        }

        // The team contacts are the rotation's members, not part of it: a rotation delete
        // removes none of them.
        for member in &team {
            let (status, body) = emulator
                .call("GetContact", json!({"ContactId": member}))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the team contact {member} to survive the rotation \
                 delete, got {status} {body:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 2. A rotation the store does not hold
// ---------------------------------------------------------------------------

/// Which ARN a "no such rotation" row names, resolved against the fixture the test built.
#[derive(Clone, Copy)]
enum AbsentArn {
    /// An ARN of a rotation this service never minted.
    NeverMinted,
    /// A well-formed SSM Contacts ARN that names a CONTACT, not a rotation.
    AnotherResourceType,
    /// The ARN of a rotation that an earlier `DeleteRotation` already removed.
    AlreadyDeleted,
}

/// A `RotationId` that names no stored rotation is refused HTTP 404
/// `ResourceNotFoundException`, and the refusal changes nothing: the surviving rotation
/// stays readable, and a contact whose ARN was passed as a `RotationId` is not deleted.
#[tokio::test]
async fn a_rotation_the_store_does_not_hold_is_refused_and_changes_nothing() {
    let rows = [
        ("a rotation ARN this service never minted", AbsentArn::NeverMinted),
        ("an ARN naming a contact rather than a rotation", AbsentArn::AnotherResourceType),
        ("a rotation an earlier delete already removed", AbsentArn::AlreadyDeleted),
    ];

    for (case, which) in rows {
        let emulator = Emulator::new();
        let team = create_team(&emulator, 1, case).await;
        let keep = create_rotation(&emulator, "keep", &team, WEEKLY, case).await;
        let doomed = create_rotation(&emulator, "doomed", &team, DAILY, case).await;

        let (status, body) = emulator
            .call("DeleteRotation", json!({"RotationId": doomed}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first DeleteRotation to answer 200, got {status} \
             {body:?}"
        );

        let rotation_id = match which {
            AbsentArn::NeverMinted => ABSENT_ROTATION_ARN.to_string(),
            AbsentArn::AnotherResourceType => team[0].clone(),
            AbsentArn::AlreadyDeleted => doomed.clone(),
        };

        let (status, body) = emulator
            .call("DeleteRotation", json!({"RotationId": rotation_id}))
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 for an ARN naming no stored rotation, got \
             {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {body:?}"
        );

        // Nothing else moved: the other rotation is still there, and a contact whose ARN was
        // sent as a `RotationId` is still a contact.
        let (status, body) = emulator
            .call("GetRotation", json!({"RotationId": keep}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the untouched rotation still readable, got {status} \
             {body:?}"
        );
        let (status, body) = emulator
            .call("GetContact", json!({"ContactId": team[0]}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the contact untouched by a refused rotation delete, got \
             {status} {body:?}"
        );
        assert_eq!(
            rotation_count(&emulator),
            1,
            "case {case:?}: expected exactly the one surviving rotation in the store, got {}",
            rotation_count(&emulator)
        );
    }
}

// ---------------------------------------------------------------------------
// 3. The account gate
// ---------------------------------------------------------------------------

/// The ACCOUNT GATE: `DeleteRotation` is admitted only while the account holds an ACTIVE
/// replication set, the gate is evaluated BEFORE `RotationId` is resolved — so a refused
/// account is answered 400 for a rotation that exists and for one that does not alike — and
/// a refused delete removes nothing.
#[tokio::test]
async fn the_replication_set_gate_decides_before_the_rotation_is_removed() {
    let emulator = Emulator::new();
    let team = create_team(&emulator, 1, "the gate suite's team").await;
    let live = create_rotation(&emulator, "gated", &team, DAILY, "the gate suite's rotation").await;

    // Every state the account's `replication_set` record can be in: absent (never
    // onboarded), present but not active (a set still replicating — only an ACTIVE set
    // opens the data plane), and present and active.
    let not_active = AccountReplicationSet {
        active: false,
        ..active_replication_set()
    };
    let gate_cases: [(&str, Option<AccountReplicationSet>, u16); 3] = [
        ("no replication set at all", None, 400),
        ("a replication set that is not active", Some(not_active), 400),
        ("an active replication set", Some(active_replication_set()), 200),
    ];

    for (case, replication_set, expected_live_status) in gate_cases {
        set_replication_set(&emulator, replication_set);

        // A rotation the store does NOT hold: 400 while the account is refused — the gate
        // decided first — and 404 only once it is admitted.
        let expected_absent_status = if expected_live_status == 200 { 404 } else { 400 };
        let (status, body) = emulator
            .call("DeleteRotation", json!({"RotationId": ABSENT_ROTATION_ARN}))
            .await;
        assert_eq!(
            status, expected_absent_status,
            "case {case:?} on an absent rotation: expected HTTP {expected_absent_status}, got \
             {status} {body:?}"
        );

        // The rotation the store HOLDS: removed only once the account is onboarded.
        let (status, body) = emulator
            .call("DeleteRotation", json!({"RotationId": live}))
            .await;
        assert_eq!(
            status, expected_live_status,
            "case {case:?} on a stored rotation: expected HTTP {expected_live_status}, got \
             {status} {body:?}"
        );
        if expected_live_status == 400 {
            assert!(
                error_code(&body).contains("ValidationException"),
                "case {case:?} on a stored rotation: expected a ValidationException, got {body:?}"
            );
            // The refusal carries the shape this family of refusals does: a populated
            // `Message`, a `Reason`, and `Fields` present and empty — no single input member
            // is at fault, so the list is empty rather than absent.
            assert!(
                body["Message"].as_str().is_some_and(|text| !text.is_empty())
                    && body["Reason"].as_str().is_some_and(|text| !text.is_empty())
                    && body["Fields"] == json!([]),
                "case {case:?} on a stored rotation: expected a populated Message and Reason \
                 and an empty Fields list, got {body:?}"
            );
            // A refused delete is not a partial one: the rotation is still in the store.
            assert_eq!(
                rotation_count(&emulator),
                1,
                "case {case:?}: expected the refused delete to remove nothing, got {} \
                 rotation(s)",
                rotation_count(&emulator)
            );
        } else {
            assert_eq!(
                rotation_count(&emulator),
                0,
                "case {case:?}: expected the admitted delete to remove the rotation, got {} \
                 rotation(s)",
                rotation_count(&emulator)
            );
        }
    }
}

/// The delete rows really do differ in the recurrence form they create, so the table cannot
/// pass by accident on one shape repeated.
#[tokio::test]
async fn the_delete_rows_cover_every_recurrence_form() {
    let forms: Vec<Value> = [DAILY, WEEKLY, MONTHLY].iter().map(|row| parse(row)).collect();
    for form in &forms {
        assert!(
            DELETE_ROWS
                .iter()
                .any(|row| &parse(row.recurrence) == form),
            "case \"recurrence coverage\": expected a delete row created with {form:?}"
        );
    }
}
