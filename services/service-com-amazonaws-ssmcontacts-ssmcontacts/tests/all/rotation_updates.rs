// SPDX-License-Identifier: CC0-1.0

//! `UpdateRotation`, end to end over the wire.
//!
//! Every case drives the crate's ONE integration-test harness (`contact_lifecycle`'s
//! `Emulator`) with a buffered awsJson1_1 request (`POST /`,
//! `X-Amz-Target: SSMContacts.<Operation>`), so what is asserted is what a signing AWS
//! client would see. Each rotation is CREATED through `CreateRotation` and read back through
//! `GetRotation`, so the update is asserted against what the service persisted rather than
//! against anything the test placed in the store.
//!
//! Four rules are under test, and each is a TABLE rather than a single call:
//!
//! 1. THE PATCH. An admitted update replaces every member the request names on the rotation
//!    the `RotationId` resolves to and leaves every member it omits exactly as it was; the
//!    rotation's identity — its ARN, its `Name` and its tags, none of which the input can
//!    carry — never moves. Each row's expectation is COMPUTED as "what the rotation read
//!    back as, overlaid with what the request named", so the assertion states that rule
//!    rather than a transcript. The rows differ along what the rule generalises over: the
//!    recurrence form (daily, weekly, monthly, shift coverages over the whole `DayOfWeek`
//!    value set), the multiplier at its published bounds, the shift team's size and ORDER,
//!    the time zone, whether a `StartTime` was ever named, and which subset of the optional
//!    members the request carries.
//! 2. THE ACCOUNT GATE. A replication-set-scoped write is admitted only while the account
//!    holds an ACTIVE replication set; otherwise it is refused HTTP 400 `ValidationException`
//!    BEFORE the rotation is looked up, and nothing is written.
//! 3. THE REFERENCES. For an onboarded account, a `RotationId` naming no stored rotation and
//!    a `ContactIds` entry naming no stored contact both answer HTTP 404
//!    `ResourceNotFoundException`, and a refused update writes nothing.
//! 4. THE RECURRENCE. `Recurrence` is modelled required, so every call replaces it, and a
//!    recurrence describing no shift at all — or naming a day outside the modelled
//!    `DayOfWeek` value set — is refused HTTP 400 with the stored rotation left as it was.

use serde_json::{json, Value};

use service_com_amazonaws_ssmcontacts_ssmcontacts::ops::types::DAY_OF_WEEK_VALUES;
use service_com_amazonaws_ssmcontacts_ssmcontacts::state::{service_state, AccountReplicationSet};

use crate::contact_lifecycle::{error_code, Emulator, ACCOUNT, REGION};
use crate::contact_updates::{active_replication_set, onboard, set_replication_set};

/// A rotation ARN of the form `CreateRotation` mints that this service never minted.
const ABSENT_ROTATION_ARN: &str =
    "arn:aws:ssm-contacts:us-east-1:000000000000:rotation/00000000-0000-0000-0000-000000000000";

/// How many contacts every case's emulator creates up front, so a patch can name a team that
/// is larger, smaller or merely reordered relative to the one the rotation was created with.
const POOL: usize = 6;

/// The `ContactArn` `CreateContact` mints for the pool contact aliased `oncall-<index>`.
fn contact_arn(index: usize) -> String {
    format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/oncall-{index}")
}

/// A shift team, in the order its indices are listed — which is the shift order.
fn team(indices: &[usize]) -> Vec<String> {
    indices.iter().copied().map(contact_arn).collect()
}

/// Create `count` pool contacts, aliased so each one's minted `ContactArn` is
/// [`contact_arn`] of its index.
///
/// `CreateRotation` and `UpdateRotation` both resolve every `ContactIds` entry against the
/// contact store, so the pool has to exist before any rotation that names it.
async fn create_pool(emulator: &Emulator, count: usize, case: &str) {
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
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateContact for oncall-{index} to answer 200, \
             got {status} {body:?}"
        );
    }
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

/// `GetRotation` for `arn`, asserting it answered 200.
async fn get_rotation(emulator: &Emulator, arn: &str, case: &str) -> Value {
    let (status, body) = emulator
        .call("GetRotation", json!({"RotationId": arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetRotation to answer 200, got {status} {body:?}"
    );
    body
}

/// The tags `ListTagsForResource` reports for `arn`, as a sorted list of pairs.
async fn tags_of(emulator: &Emulator, arn: &str, case: &str) -> Vec<(String, String)> {
    let (status, body) = emulator
        .call("ListTagsForResource", json!({"ResourceARN": arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListTagsForResource to answer 200, got {status} {body:?}"
    );
    let mut tags: Vec<(String, String)> = body["Tags"]
        .as_array()
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    (
                        tag["Key"].as_str().unwrap_or_default().to_string(),
                        tag["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    tags.sort();
    tags
}

// ---------------------------------------------------------------------------
// The tables
// ---------------------------------------------------------------------------

/// A rotation to update, as `CreateRotation` created it.
struct BaseRotation {
    case: &'static str,
    /// Built per row, because each one names pool contacts by ARN.
    request: fn() -> Value,
}

/// The rotations the patch table is applied to. They differ in the recurrence form, the team
/// size, the time zone, whether the create named a `StartTime` and whether it carried tags —
/// everything the patch rule has to be independent of.
fn base_rotations() -> Vec<BaseRotation> {
    vec![
        BaseRotation {
            case: "a daily rotation of one contact with an explicit start",
            request: || {
                json!({
                    "Name": "daily-primary",
                    "ContactIds": team(&[0]),
                    "TimeZoneId": "America/Los_Angeles",
                    "StartTime": 1_735_689_600,
                    "Recurrence": {
                        "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
                        "NumberOfOnCalls": 1,
                        // `RecurrenceMultiplier` is modelled `range 1..100`: the minimum.
                        "RecurrenceMultiplier": 1,
                    },
                })
            },
        },
        BaseRotation {
            case: "a weekly rotation of three contacts with two hand-offs and tags",
            request: || {
                json!({
                    "Name": "weekly.rotation-team",
                    "ContactIds": team(&[0, 1, 2]),
                    "TimeZoneId": "UTC",
                    "StartTime": 1_700_000_000,
                    "Tags": [{"Key": "team", "Value": "sre"}],
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
        BaseRotation {
            case: "a monthly rotation with weekend coverage and NO start time",
            request: || {
                json!({
                    "Name": "monthly_weekend-cover",
                    "ContactIds": team(&[2, 1]),
                    "TimeZoneId": "Asia/Tokyo",
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
    ]
}

/// One update to apply: the members the `UpdateRotation` request names beside `RotationId`.
struct UpdateRow {
    case: &'static str,
    /// Built per row, because the `ContactIds` rows name pool contacts by ARN.
    patch: fn() -> Value,
}

/// A `ShiftCoverages` map covering EVERY value of the modelled `DayOfWeek` enum.
///
/// The value set is the crate's own `DAY_OF_WEEK_VALUES` (`src/ops/types/shared_types.rs`,
/// generated from the model's `com.amazonaws.ssmcontacts#DayOfWeek`), so this row exercises
/// days — `FRI`, `SUN`, `TUE`, `WED` among them — that no other row names.
fn every_day_coverage() -> Value {
    let mut coverage = serde_json::Map::new();
    for (index, day) in DAY_OF_WEEK_VALUES.iter().enumerate() {
        coverage.insert(
            (*day).to_string(),
            json!([{
                "Start": {"HourOfDay": index as i32, "MinuteOfHour": 0},
                "End": {"HourOfDay": index as i32 + 8, "MinuteOfHour": 30},
            }]),
        );
    }
    Value::Object(coverage)
}

/// The patch rows. `Recurrence` is modelled REQUIRED, so every row carries one; the rows
/// differ in which of the OPTIONAL members (`ContactIds`, `StartTime`, `TimeZoneId`) they
/// name as well, and in the value each named member carries.
fn update_rows() -> Vec<UpdateRow> {
    vec![
        UpdateRow {
            case: "only the required Recurrence, switched to a daily form",
            patch: || {
                json!({
                    "Recurrence": {
                        "DailySettings": [{"HourOfDay": 6, "MinuteOfHour": 15}],
                        "NumberOfOnCalls": 1,
                        "RecurrenceMultiplier": 1,
                    },
                })
            },
        },
        UpdateRow {
            case: "the same contacts in a new shift order",
            patch: || {
                json!({
                    "ContactIds": team(&[2, 1, 0]),
                    "Recurrence": {
                        "DailySettings": [{"HourOfDay": 0, "MinuteOfHour": 0}],
                        "NumberOfOnCalls": 1,
                        "RecurrenceMultiplier": 1,
                    },
                })
            },
        },
        UpdateRow {
            case: "a larger shift team and a weekly recurrence",
            patch: || {
                json!({
                    "ContactIds": team(&[0, 1, 2, 3, 4, 5]),
                    "Recurrence": {
                        "WeeklySettings": [
                            {"DayOfWeek": "FRI", "HandOffTime": {"HourOfDay": 17, "MinuteOfHour": 0}},
                        ],
                        "NumberOfOnCalls": 3,
                        "RecurrenceMultiplier": 4,
                    },
                })
            },
        },
        UpdateRow {
            case: "a shift team cut to one contact",
            patch: || {
                json!({
                    // `ContactIds` is modelled `length 1..30`: this row sits on the minimum.
                    "ContactIds": team(&[5]),
                    "Recurrence": {
                        "MonthlySettings": [
                            {"DayOfMonth": 1, "HandOffTime": {"HourOfDay": 12, "MinuteOfHour": 0}},
                        ],
                        "NumberOfOnCalls": 1,
                        "RecurrenceMultiplier": 1,
                    },
                })
            },
        },
        UpdateRow {
            case: "a new StartTime beside the recurrence",
            patch: || {
                json!({
                    "StartTime": 1_767_225_600,
                    "Recurrence": {
                        "DailySettings": [{"HourOfDay": 23, "MinuteOfHour": 59}],
                        "NumberOfOnCalls": 1,
                        "RecurrenceMultiplier": 7,
                    },
                })
            },
        },
        UpdateRow {
            case: "a new TimeZoneId beside the recurrence",
            patch: || {
                json!({
                    "TimeZoneId": "Europe/Berlin",
                    "Recurrence": {
                        "WeeklySettings": [
                            {"DayOfWeek": "SUN", "HandOffTime": {"HourOfDay": 10, "MinuteOfHour": 0}},
                        ],
                        "NumberOfOnCalls": 1,
                        "RecurrenceMultiplier": 1,
                    },
                })
            },
        },
        UpdateRow {
            case: "every optional member named at once, at the published multiplier maximum",
            patch: || {
                json!({
                    "ContactIds": team(&[4, 3]),
                    "StartTime": 1_600_000_000,
                    "TimeZoneId": "Etc/UTC",
                    "Recurrence": {
                        "MonthlySettings": [
                            {"DayOfMonth": 28, "HandOffTime": {"HourOfDay": 3, "MinuteOfHour": 45}},
                        ],
                        "ShiftCoverages": every_day_coverage(),
                        "NumberOfOnCalls": 2,
                        // `RecurrenceMultiplier` is modelled `range 1..100`: the maximum.
                        "RecurrenceMultiplier": 100,
                    },
                })
            },
        },
    ]
}

/// What `GetRotation` must answer after `patch` was applied to a rotation that read back as
/// `before`: `before` overlaid with the members the request named. This IS the rule.
fn expected_after(before: &Value, patch: &Value) -> Value {
    let mut expected = before.clone();
    for member in ["ContactIds", "StartTime", "TimeZoneId", "Recurrence"] {
        if let Some(named) = patch.get(member) {
            expected[member] = named.clone();
        }
    }
    expected
}

/// Assert `GetRotation`'s answer equals `expected`, comparing `StartTime` numerically (the
/// wire carries an epoch timestamp, which a request may have named as an integer).
fn assert_rotation_eq(actual: &Value, expected: &Value, case: &str) {
    for member in ["RotationArn", "Name", "ContactIds", "TimeZoneId", "Recurrence"] {
        assert_eq!(
            actual[member], expected[member],
            "case {case:?}: expected {member} {:?}, got {:?} (whole response {actual:?})",
            expected[member], actual[member]
        );
    }
    let actual_start = actual["StartTime"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a StartTime, got {actual:?}"));
    let expected_start = expected["StartTime"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a StartTime to compare against"));
    assert!(
        (actual_start - expected_start).abs() < f64::EPSILON,
        "case {case:?}: expected StartTime {expected_start}, got {actual_start}"
    );
}

// ---------------------------------------------------------------------------
// Rule 1: named members replace, omitted members are untouched
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_replaces_the_members_it_names_and_leaves_the_rest() {
    for base in base_rotations() {
        for update in update_rows() {
            let case = format!("{} updated by {}", base.case, update.case);
            let emulator = Emulator::new();
            onboard(&emulator);
            create_pool(&emulator, POOL, &case).await;
            let created = (base.request)();
            let arn = create_rotation(&emulator, created.clone(), &case).await;

            let before = get_rotation(&emulator, &arn, &case).await;
            let tags_before = tags_of(&emulator, &arn, &case).await;

            let patch = (update.patch)();
            let mut request = patch.clone();
            request["RotationId"] = json!(arn);
            let (status, body) = emulator.call("UpdateRotation", request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected UpdateRotation to answer 200, got {status} {body:?}"
            );
            assert!(
                body == json!({}) || body.is_null(),
                "case {case:?}: UpdateRotation models an empty output, got {body:?}"
            );

            let expected = expected_after(&before, &patch);
            let after = get_rotation(&emulator, &arn, &case).await;
            assert_rotation_eq(&after, &expected, &case);

            // The input carries neither a Name nor Tags, so neither can move — and the ARN
            // is the rotation's identity, so the update patched the rotation in place rather
            // than minting a second one.
            assert_eq!(
                after["Name"], created["Name"],
                "case {case:?}: an update must not change the Name, got {after:?}"
            );
            let tags_after = tags_of(&emulator, &arn, &case).await;
            assert_eq!(
                tags_after, tags_before,
                "case {case:?}: an update must not change the tags, expected \
                 {tags_before:?}, got {tags_after:?}"
            );
            let store =
                service_state(&*emulator.engine).expect("the engine holds this service's state");
            let rotations = store.lock().rotations.len();
            assert_eq!(
                rotations, 1,
                "case {case:?}: expected the update to patch the one rotation in place, \
                 got {rotations} rotation(s)"
            );
        }
    }
}

/// Updates accumulate — each one overlays the state the previous one left — and applying the
/// same update twice leaves the same rotation: what is written is determined by the request,
/// not by how many times it was sent.
#[tokio::test]
async fn updates_accumulate_and_the_same_update_twice_is_the_same_rotation() {
    let base = &base_rotations()[1];
    let case = "every update row applied in sequence to one rotation";
    let emulator = Emulator::new();
    onboard(&emulator);
    create_pool(&emulator, POOL, case).await;
    let arn = create_rotation(&emulator, (base.request)(), case).await;

    for update in update_rows() {
        let case = format!("{case}, at {}", update.case);
        let patch = (update.patch)();
        let before = get_rotation(&emulator, &arn, &case).await;
        let expected = expected_after(&before, &patch);

        for attempt in 1..=2 {
            let case = format!("{case}, attempt {attempt}");
            let mut request = patch.clone();
            request["RotationId"] = json!(arn);
            let (status, body) = emulator.call("UpdateRotation", request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {body:?}"
            );
            let after = get_rotation(&emulator, &arn, &case).await;
            assert_rotation_eq(&after, &expected, &case);
        }
    }
}

/// `ContactIds` is modelled `length 1..30`: a team at the published maximum is stored whole,
/// and one past it is refused before the handler runs.
#[tokio::test]
async fn a_shift_team_at_the_published_maximum_is_stored_whole() {
    let case = "a shift team at the published ContactIds maximum";
    let emulator = Emulator::new();
    onboard(&emulator);
    create_pool(&emulator, 31, case).await;
    let arn = create_rotation(
        &emulator,
        json!({
            "Name": "bounds",
            "ContactIds": team(&[0]),
            "TimeZoneId": "UTC",
            "Recurrence": {
                "DailySettings": [{"HourOfDay": 1, "MinuteOfHour": 0}],
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
            },
        }),
        case,
    )
    .await;

    let full: Vec<usize> = (0..30).collect();
    let (status, body) = emulator
        .call(
            "UpdateRotation",
            json!({
                "RotationId": arn,
                "ContactIds": team(&full),
                "Recurrence": {
                    "DailySettings": [{"HourOfDay": 2, "MinuteOfHour": 0}],
                    "NumberOfOnCalls": 30,
                    "RecurrenceMultiplier": 1,
                },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {body:?}"
    );
    let after = get_rotation(&emulator, &arn, case).await;
    assert_eq!(
        after["ContactIds"],
        json!(team(&full)),
        "case {case:?}: expected the whole 30-contact team in order, got {after:?}"
    );

    // One past the published maximum is outside the modelled constraint.
    let past: Vec<usize> = (0..31).collect();
    let (status, body) = emulator
        .call(
            "UpdateRotation",
            json!({
                "RotationId": arn,
                "ContactIds": team(&past),
                "Recurrence": {
                    "DailySettings": [{"HourOfDay": 3, "MinuteOfHour": 0}],
                    "NumberOfOnCalls": 1,
                    "RecurrenceMultiplier": 1,
                },
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"a shift team past the published ContactIds maximum\": expected 400, \
         got {status} {body:?}"
    );
    let unchanged = get_rotation(&emulator, &arn, case).await;
    assert_eq!(
        unchanged["ContactIds"],
        json!(team(&full)),
        "case \"a shift team past the published ContactIds maximum\": a refused update must \
         write nothing, got {unchanged:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: the account gate, evaluated before the rotation is looked up
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_is_refused_when_the_account_has_no_active_replication_set() {
    // Every state the account's `replication_set` record can be in that is not ACTIVE:
    // absent (an account that was never onboarded) and present but still replicating.
    let no_active_set: &[(&str, Option<AccountReplicationSet>)] = &[
        ("no replication set at all", None),
        (
            "a replication set that is not active yet",
            Some(AccountReplicationSet {
                active: false,
                ..active_replication_set()
            }),
        ),
    ];

    for (account_case, replication_set) in no_active_set {
        for update in update_rows() {
            let case = format!("{account_case}, updated by {}", update.case);
            let emulator = Emulator::new();
            // The rotation is created while the account IS onboarded, so the only thing the
            // update differs in is the precondition.
            onboard(&emulator);
            create_pool(&emulator, POOL, &case).await;
            let base = &base_rotations()[0];
            let live = create_rotation(&emulator, (base.request)(), &case).await;
            let before = get_rotation(&emulator, &live, &case).await;

            for (target_case, target) in [
                ("a stored rotation", live.as_str()),
                ("an absent rotation", ABSENT_ROTATION_ARN),
            ] {
                let case = format!("{case}, {target_case}");
                set_replication_set(&emulator, replication_set.clone());

                let mut request = (update.patch)();
                request["RotationId"] = json!(target);
                let (status, body) = emulator.call("UpdateRotation", request).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the account precondition to answer 400, \
                     got {status} {body:?}"
                );
                assert!(
                    error_code(&body).contains("ValidationException"),
                    "case {case:?}: expected a ValidationException — NOT a \
                     ResourceNotFoundException, even for an absent rotation — got {body:?}"
                );
                // The refusal carries the shape this family of refusals carries: a populated
                // Message and Reason, and Fields present and EMPTY — the failure is account
                // level, so no member of the request is at fault.
                assert!(
                    body["Message"].as_str().is_some_and(|text| !text.is_empty())
                        && body["Reason"].as_str().is_some_and(|text| !text.is_empty())
                        && body["Fields"] == json!([]),
                    "case {case:?}: expected a populated Message and Reason and an empty \
                     Fields list, got {body:?}"
                );

                // Nothing was written: with the account onboarded again, the rotation still
                // reads back exactly as it did before the refused update.
                onboard(&emulator);
                let after = get_rotation(&emulator, &live, &case).await;
                assert_rotation_eq(&after, &before, &case);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the references the request names must resolve
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_naming_an_unstored_rotation_or_contact_answers_404() {
    let case = "the references an update names";
    let emulator = Emulator::new();
    onboard(&emulator);
    create_pool(&emulator, POOL, case).await;
    let live = create_rotation(&emulator, (base_rotations()[1].request)(), case).await;
    let before = get_rotation(&emulator, &live, case).await;

    // A rotation that was created and then deleted, so "absent" cannot be confused with
    // "the store never held one".
    let doomed = create_rotation(
        &emulator,
        json!({
            "Name": "doomed",
            "ContactIds": team(&[0]),
            "TimeZoneId": "UTC",
            "Recurrence": {
                "DailySettings": [{"HourOfDay": 4, "MinuteOfHour": 0}],
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
            },
        }),
        case,
    )
    .await;
    let (status, body) = emulator
        .call("DeleteRotation", json!({"RotationId": doomed}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DeleteRotation to answer 200, got {status} {body:?}"
    );

    let recurrence = json!({
        "DailySettings": [{"HourOfDay": 5, "MinuteOfHour": 0}],
        "NumberOfOnCalls": 1,
        "RecurrenceMultiplier": 1,
    });

    // Every well-formed `SsmContactsArn` that names no STORED ROTATION answers the same way,
    // including one that names a resource of another kind.
    let absent_rotations: &[(&str, String)] = &[
        (
            "an ARN naming a rotation nothing created",
            ABSENT_ROTATION_ARN.to_string(),
        ),
        ("a deleted rotation's ARN", doomed),
        ("a contact's ARN", contact_arn(0)),
    ];
    for (target_case, target) in absent_rotations {
        let case = format!("{case}: {target_case}");
        let (status, body) = emulator
            .call(
                "UpdateRotation",
                json!({"RotationId": target, "Recurrence": recurrence.clone()}),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {body:?}"
        );
    }

    // A `ContactIds` entry that names no stored contact is the same refusal, wherever in the
    // list it sits and whatever else the list holds.
    let absent_contact = format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/never-created");
    let absent_teams: &[(&str, Vec<String>)] = &[
        ("the only contact in the list", vec![absent_contact.clone()]),
        (
            "the first of several",
            vec![absent_contact.clone(), contact_arn(0)],
        ),
        (
            "the last of several",
            vec![contact_arn(0), contact_arn(1), absent_contact.clone()],
        ),
        (
            "a rotation ARN where a contact ARN belongs",
            vec![live.clone()],
        ),
    ];
    for (target_case, contact_ids) in absent_teams {
        let case = format!("{case}: {target_case}");
        let (status, body) = emulator
            .call(
                "UpdateRotation",
                json!({
                    "RotationId": live,
                    "ContactIds": contact_ids,
                    "Recurrence": recurrence.clone(),
                }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {body:?}"
        );
    }

    // Every refusal above wrote nothing: the live rotation is exactly as it was created.
    let after = get_rotation(&emulator, &live, case).await;
    assert_rotation_eq(&after, &before, case);
}

// ---------------------------------------------------------------------------
// Rule 4: the recurrence the update installs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_recurrence_that_describes_no_shift_is_refused_and_writes_nothing() {
    let case = "the recurrence an update installs";
    let emulator = Emulator::new();
    onboard(&emulator);
    create_pool(&emulator, POOL, case).await;
    let live = create_rotation(&emulator, (base_rotations()[0].request)(), case).await;
    let before = get_rotation(&emulator, &live, case).await;

    // Each row is a `Recurrence` the service must refuse with HTTP 400: one carrying none of
    // the three settings lists (in each of the forms "absent" and "present but empty"), and
    // one naming a day outside the modelled `DayOfWeek` value set.
    let refused: &[(&str, Value)] = &[
        (
            "no settings list at all",
            json!({"NumberOfOnCalls": 1, "RecurrenceMultiplier": 1}),
        ),
        (
            "every settings list present but empty",
            json!({
                "DailySettings": [],
                "WeeklySettings": [],
                "MonthlySettings": [],
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
            }),
        ),
        (
            "only shift coverages, which describe no shift of their own",
            json!({
                "ShiftCoverages": {
                    "MON": [{
                        "Start": {"HourOfDay": 9, "MinuteOfHour": 0},
                        "End": {"HourOfDay": 17, "MinuteOfHour": 0},
                    }],
                },
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
            }),
        ),
        (
            "a weekly hand-off on a day outside the modelled DayOfWeek value set",
            json!({
                "WeeklySettings": [
                    {"DayOfWeek": "FUNDAY", "HandOffTime": {"HourOfDay": 9, "MinuteOfHour": 0}},
                ],
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
            }),
        ),
        (
            "shift coverage keyed by a day outside the modelled DayOfWeek value set",
            json!({
                "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
                "ShiftCoverages": {
                    "CATURDAY": [{
                        "Start": {"HourOfDay": 9, "MinuteOfHour": 0},
                        "End": {"HourOfDay": 17, "MinuteOfHour": 0},
                    }],
                },
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
            }),
        ),
    ];

    for (row_case, recurrence) in refused {
        let case = format!("{case}: {row_case}");
        let (status, body) = emulator
            .call(
                "UpdateRotation",
                json!({"RotationId": live, "Recurrence": recurrence}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
        let after = get_rotation(&emulator, &live, &case).await;
        assert_rotation_eq(&after, &before, &case);
    }

    // Every day the model DOES declare is accepted, which is what shows the refusals above
    // are about the value set and not about weekly settings as such.
    for day in DAY_OF_WEEK_VALUES {
        let case = format!("{case}: a weekly hand-off on {day}");
        let recurrence = json!({
            "WeeklySettings": [
                {"DayOfWeek": day, "HandOffTime": {"HourOfDay": 9, "MinuteOfHour": 0}},
            ],
            "NumberOfOnCalls": 1,
            "RecurrenceMultiplier": 1,
        });
        let (status, body) = emulator
            .call(
                "UpdateRotation",
                json!({"RotationId": live, "Recurrence": recurrence.clone()}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body:?}"
        );
        let after = get_rotation(&emulator, &live, &case).await;
        assert_eq!(
            after["Recurrence"], recurrence,
            "case {case:?}: expected the installed recurrence, got {after:?}"
        );
    }
}

/// A member outside its modelled constraint is refused with HTTP 400 before the handler runs
/// — asserted on each constraint `UpdateRotation`'s input declares.
#[tokio::test]
async fn members_outside_their_modelled_constraint_are_refused() {
    let case = "the modelled constraints of UpdateRotation's input";
    let emulator = Emulator::new();
    onboard(&emulator);
    create_pool(&emulator, POOL, case).await;
    let live = create_rotation(&emulator, (base_rotations()[0].request)(), case).await;
    let before = get_rotation(&emulator, &live, case).await;

    let recurrence = json!({
        "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
        "NumberOfOnCalls": 1,
        "RecurrenceMultiplier": 1,
    });

    let refused: Vec<(&str, Value)> = vec![
        // `RotationId` is required (SsmContactsArn, len 1..2048, pattern ^arn:...$).
        ("RotationId absent", json!({"Recurrence": recurrence})),
        (
            "RotationId outside its modelled pattern",
            json!({"RotationId": "not-an-arn", "Recurrence": recurrence}),
        ),
        (
            "RotationId shorter than its modelled minimum length",
            json!({"RotationId": "", "Recurrence": recurrence}),
        ),
        // `Recurrence` is required.
        ("Recurrence absent", json!({"RotationId": live})),
        // `ContactIds` is RotationContactsArnList: len 1..30.
        (
            "ContactIds shorter than its modelled minimum length",
            json!({"RotationId": live, "ContactIds": [], "Recurrence": recurrence}),
        ),
        // `TimeZoneId`: len 1..255, pattern ^[:a-zA-Z0-9_\-\s\.\\/]*$.
        (
            "TimeZoneId outside its modelled pattern",
            json!({"RotationId": live, "TimeZoneId": "Europe/Berlin!", "Recurrence": recurrence}),
        ),
        (
            "TimeZoneId longer than its modelled maximum length",
            json!({
                "RotationId": live,
                "TimeZoneId": "a".repeat(256),
                "Recurrence": recurrence,
            }),
        ),
        // `RecurrenceMultiplier` is modelled range 1..100.
        (
            "RecurrenceMultiplier below its published minimum",
            json!({
                "RotationId": live,
                "Recurrence": {
                    "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
                    "NumberOfOnCalls": 1,
                    "RecurrenceMultiplier": 0,
                },
            }),
        ),
        (
            "RecurrenceMultiplier past its published maximum",
            json!({
                "RotationId": live,
                "Recurrence": {
                    "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
                    "NumberOfOnCalls": 1,
                    "RecurrenceMultiplier": 101,
                },
            }),
        ),
    ];

    for (row_case, request) in &refused {
        let (status, body) = emulator.call("UpdateRotation", request.clone()).await;
        assert_eq!(
            status, 400,
            "case {row_case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {row_case:?}: expected a ValidationException, got {body:?}"
        );
    }

    // And the live rotation is untouched by every refused request.
    let after = get_rotation(&emulator, &live, case).await;
    assert_rotation_eq(&after, &before, case);
}
