// SPDX-License-Identifier: CC0-1.0

//! The `RecurrenceSettings` validity rule, asserted as ONE rule over both operations that
//! install a recurrence on a rotation.
//!
//! `CreateRotation` and `UpdateRotation` each take a whole `RecurrenceSettings` and store it,
//! so which recurrences they admit is a single rule:
//!
//! * every `DayOfWeek` the recurrence carries — a `WeeklySettings` day and a `ShiftCoverages`
//!   key, both of which reach a handler as plain strings — must be one of the values the
//!   model declares; and
//! * at least one of the daily, weekly and monthly settings lists must carry an entry,
//!   because a recurrence carrying none of them says when no shift ever starts.
//!
//! Each row below is driven through BOTH operations against the same store and the two
//! answers are asserted to AGREE — same HTTP status, and for a refusal the same error code
//! and the same message. That is what the sibling suites (`data_plane_precondition` for
//! `CreateRotation`, `rotation_updates` for `UpdateRotation`) cannot state on their own: they
//! pin each operation's answer separately, so the two could drift apart and still pass.
//!
//! The rows differ along what the rule generalises over: which of the three settings lists is
//! present, whether an absent list is absent or present-but-empty, how many entries a list
//! carries, whether shift coverages are present beside a settings list or alone, and which
//! `DayOfWeek` value a member names (the catalog test walks the whole modelled value set and
//! the forms just outside it).

use serde_json::{json, Value};

use service_com_amazonaws_ssmcontacts_ssmcontacts::ops::types::DAY_OF_WEEK_VALUES;

use crate::contact_lifecycle::{error_code, Emulator};

/// The `Alias` the one pool contact is created with, so both calls can name a stored contact.
const CONTACT_ALIAS: &str = "oncall-recurrence";

/// A recurrence every row's fixture rotation is created with: the simplest form the rule
/// admits, so a refused row is visibly "the store still holds this one".
fn baseline_recurrence() -> Value {
    json!({
        "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
        "NumberOfOnCalls": 1,
        // `RecurrenceMultiplier` is modelled `range 1..100`: the minimum.
        "RecurrenceMultiplier": 1,
    })
}

/// One recurrence under test and the verdict the rule gives it.
struct Row {
    case: &'static str,
    recurrence: Value,
    /// `true` when the rule admits this recurrence, `false` when it refuses it.
    admitted: bool,
}

/// A `ShiftCoverages` map keyed by `day`.
fn coverage_on(day: &str) -> Value {
    json!({
        day: [{
            "Start": {"HourOfDay": 9, "MinuteOfHour": 0},
            "End": {"HourOfDay": 17, "MinuteOfHour": 0},
        }],
    })
}

/// The recurrences the rule is asserted over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a daily hand-off",
            recurrence: json!({
                "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
            }),
            admitted: true,
        },
        Row {
            case: "two daily hand-offs",
            recurrence: json!({
                "DailySettings": [
                    {"HourOfDay": 6, "MinuteOfHour": 30},
                    {"HourOfDay": 18, "MinuteOfHour": 45},
                ],
                "NumberOfOnCalls": 2,
                "RecurrenceMultiplier": 1,
            }),
            admitted: true,
        },
        Row {
            case: "a weekly hand-off only",
            recurrence: json!({
                "WeeklySettings": [
                    {"DayOfWeek": "WED", "HandOffTime": {"HourOfDay": 8, "MinuteOfHour": 0}},
                ],
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
            }),
            admitted: true,
        },
        Row {
            case: "a monthly hand-off only, with an empty weekly list beside it",
            recurrence: json!({
                "MonthlySettings": [
                    {"DayOfMonth": 28, "HandOffTime": {"HourOfDay": 7, "MinuteOfHour": 15}},
                ],
                "WeeklySettings": [],
                "NumberOfOnCalls": 1,
                // `RecurrenceMultiplier` is modelled `range 1..100`: the maximum.
                "RecurrenceMultiplier": 100,
            }),
            admitted: true,
        },
        Row {
            case: "all three settings lists at once",
            recurrence: json!({
                "DailySettings": [{"HourOfDay": 1, "MinuteOfHour": 0}],
                "WeeklySettings": [
                    {"DayOfWeek": "SUN", "HandOffTime": {"HourOfDay": 2, "MinuteOfHour": 0}},
                ],
                "MonthlySettings": [
                    {"DayOfMonth": 1, "HandOffTime": {"HourOfDay": 3, "MinuteOfHour": 0}},
                ],
                "NumberOfOnCalls": 3,
                "RecurrenceMultiplier": 7,
            }),
            admitted: true,
        },
        Row {
            case: "a daily hand-off with shift coverage over every modelled day",
            recurrence: json!({
                "DailySettings": [{"HourOfDay": 12, "MinuteOfHour": 0}],
                "ShiftCoverages": every_day_coverage(),
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
            }),
            admitted: true,
        },
        Row {
            case: "no settings list at all",
            recurrence: json!({"NumberOfOnCalls": 1, "RecurrenceMultiplier": 1}),
            admitted: false,
        },
        Row {
            case: "every settings list present but empty",
            recurrence: json!({
                "DailySettings": [],
                "WeeklySettings": [],
                "MonthlySettings": [],
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
            }),
            admitted: false,
        },
        Row {
            case: "shift coverage alone, which describes no shift of its own",
            recurrence: json!({
                "ShiftCoverages": coverage_on("MON"),
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
            }),
            admitted: false,
        },
        Row {
            case: "an empty monthly list beside shift coverage",
            recurrence: json!({
                "MonthlySettings": [],
                "ShiftCoverages": coverage_on("FRI"),
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
            }),
            admitted: false,
        },
        Row {
            case: "a weekly hand-off on a day outside the modelled value set",
            recurrence: json!({
                "WeeklySettings": [
                    {"DayOfWeek": "FUNDAY", "HandOffTime": {"HourOfDay": 9, "MinuteOfHour": 0}},
                ],
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
            }),
            admitted: false,
        },
        Row {
            case: "shift coverage keyed by a day outside the modelled value set",
            recurrence: json!({
                "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
                "ShiftCoverages": coverage_on("CATURDAY"),
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
            }),
            admitted: false,
        },
        Row {
            case: "a weekly day outside the value set beside a settings list that would admit",
            recurrence: json!({
                "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
                "WeeklySettings": [
                    {"DayOfWeek": "mon", "HandOffTime": {"HourOfDay": 9, "MinuteOfHour": 0}},
                ],
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
            }),
            admitted: false,
        },
    ]
}

/// A `ShiftCoverages` map covering EVERY value of the modelled `DayOfWeek` enum.
///
/// The value set is the crate's own `DAY_OF_WEEK_VALUES` (`src/ops/types/shared_types.rs`,
/// generated from the model's `com.amazonaws.ssmcontacts#DayOfWeek`), so this row names days
/// — `FRI`, `SAT`, `THU`, `TUE` among them — that no other row does.
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

/// One emulator holding one contact and one rotation created with [`baseline_recurrence`],
/// answering `(emulator, contact arn, rotation arn)`.
async fn fixture(case: &str) -> (Emulator, String, String) {
    let emulator = Emulator::new();
    let contact = emulator
        .create_contact(
            json!({"Alias": CONTACT_ALIAS, "Type": "PERSONAL", "Plan": {"Stages": []}}),
            case,
        )
        .await;
    let (status, body) = emulator
        .call(
            "CreateRotation",
            json!({
                "Name": "baseline",
                "ContactIds": [contact.clone()],
                "TimeZoneId": "UTC",
                "Recurrence": baseline_recurrence(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the fixture CreateRotation to answer 200, got {status} {body:?}"
    );
    let rotation = body["RotationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a RotationArn, got {body:?}"))
        .to_string();
    (emulator, contact, rotation)
}

/// The refusal message an error body carries.
///
/// A refusal raised by the shared constraint validator spells the member `message`, while a
/// handler's modelled `ValidationException` spells it `Message`, so both spellings are read:
/// what this suite asserts is that the two operations answer the SAME text, whichever of them
/// refused the recurrence and wherever the refusal came from.
fn error_message(body: &Value) -> &str {
    for member in ["Message", "message"] {
        if let Some(text) = body.get(member).and_then(Value::as_str) {
            return text;
        }
    }
    ""
}

/// The recurrence `GetRotation` reports for `arn`.
async fn stored_recurrence(emulator: &Emulator, arn: &str, case: &str) -> Value {
    let (status, body) = emulator
        .call("GetRotation", json!({"RotationId": arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetRotation to answer 200, got {status} {body:?}"
    );
    body["Recurrence"].clone()
}

/// Every `RotationArn` `ListRotations` reports, so a refused create can be shown to have
/// minted nothing.
async fn rotation_arns(emulator: &Emulator, case: &str) -> Vec<String> {
    let (status, body) = emulator.call("ListRotations", json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListRotations to answer 200, got {status} {body:?}"
    );
    body["Rotations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|rotation| {
            rotation["RotationArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected every rotation to carry an ARN, got {body:?}")
                })
                .to_string()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The rule: one verdict per recurrence, whichever operation installs it
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_and_update_judge_a_recurrence_identically() {
    for row in rows() {
        let case = format!("the recurrence rule applied to {}", row.case);
        let (emulator, contact, baseline) = fixture(&case).await;

        let (create_status, create_body) = emulator
            .call(
                "CreateRotation",
                json!({
                    "Name": "under-test",
                    "ContactIds": [contact],
                    "TimeZoneId": "UTC",
                    "Recurrence": row.recurrence.clone(),
                }),
            )
            .await;
        let (update_status, update_body) = emulator
            .call(
                "UpdateRotation",
                json!({"RotationId": baseline, "Recurrence": row.recurrence.clone()}),
            )
            .await;

        // The rule is ONE rule: the two operations agree on every recurrence.
        assert_eq!(
            create_status, update_status,
            "case {case:?}: CreateRotation answered {create_status} {create_body:?} while \
             UpdateRotation answered {update_status} {update_body:?}"
        );

        if row.admitted {
            assert_eq!(
                create_status, 200,
                "case {case:?}: expected HTTP 200, got {create_status} {create_body:?}"
            );
            // Admitted by both means stored by both: each operation's rotation reads back
            // carrying exactly the recurrence the request named.
            let created = create_body["RotationArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a RotationArn, got {create_body:?}"))
                .to_string();
            let after_create = stored_recurrence(&emulator, &created, &case).await;
            assert_eq!(
                after_create, row.recurrence,
                "case {case:?}: expected CreateRotation to store {:?}, got {after_create:?}",
                row.recurrence
            );
            let after_update = stored_recurrence(&emulator, &baseline, &case).await;
            assert_eq!(
                after_update, row.recurrence,
                "case {case:?}: expected UpdateRotation to store {:?}, got {after_update:?}",
                row.recurrence
            );
            continue;
        }

        assert_eq!(
            create_status, 400,
            "case {case:?}: expected HTTP 400, got {create_status} {create_body:?}"
        );
        assert!(
            error_code(&create_body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException from CreateRotation, got {create_body:?}"
        );
        assert_eq!(
            error_code(&create_body),
            error_code(&update_body),
            "case {case:?}: expected one error code, got {:?} from CreateRotation and {:?} from \
             UpdateRotation",
            error_code(&create_body),
            error_code(&update_body)
        );
        let create_message = error_message(&create_body);
        let update_message = error_message(&update_body);
        assert!(
            !create_message.is_empty(),
            "case {case:?}: expected a refusal message, got {create_body:?}"
        );
        assert_eq!(
            create_message, update_message,
            "case {case:?}: expected one refusal message, got {create_message:?} from \
             CreateRotation and {update_message:?} from UpdateRotation"
        );

        // A refusal writes nothing: no rotation was minted and the fixture's recurrence is
        // exactly as it was created.
        let arns = rotation_arns(&emulator, &case).await;
        assert_eq!(
            arns,
            vec![baseline.clone()],
            "case {case:?}: expected the store to hold only the fixture rotation, got {arns:?}"
        );
        let after = stored_recurrence(&emulator, &baseline, &case).await;
        assert_eq!(
            after,
            baseline_recurrence(),
            "case {case:?}: expected the stored recurrence untouched, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The value set the rule checks each `DayOfWeek` against
// ---------------------------------------------------------------------------

/// Every value the model declares for `DayOfWeek` is admitted in both places the rule reads
/// one — a `WeeklySettings` day and a `ShiftCoverages` key — and in both operations; a value
/// just outside the set (its lower-cased spelling, the empty-ish name, a plausible day name)
/// is refused by both with the same message.
#[tokio::test]
async fn both_operations_check_every_day_of_week_against_the_modelled_value_set() {
    // The catalog is the crate's `DAY_OF_WEEK_VALUES`
    // (`src/ops/types/shared_types.rs`, generated from `com.amazonaws.ssmcontacts#DayOfWeek`).
    for day in DAY_OF_WEEK_VALUES {
        for (place, recurrence) in [
            (
                "a weekly hand-off",
                json!({
                    "WeeklySettings": [
                        {"DayOfWeek": day, "HandOffTime": {"HourOfDay": 9, "MinuteOfHour": 0}},
                    ],
                    "NumberOfOnCalls": 1,
                    "RecurrenceMultiplier": 1,
                }),
            ),
            (
                "a shift coverage key",
                json!({
                    "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
                    "ShiftCoverages": coverage_on(day),
                    "NumberOfOnCalls": 1,
                    "RecurrenceMultiplier": 1,
                }),
            ),
        ] {
            let case = format!("{day} named by {place}");
            let (emulator, contact, baseline) = fixture(&case).await;

            let (create_status, create_body) = emulator
                .call(
                    "CreateRotation",
                    json!({
                        "Name": "under-test",
                        "ContactIds": [contact],
                        "TimeZoneId": "UTC",
                        "Recurrence": recurrence.clone(),
                    }),
                )
                .await;
            assert_eq!(
                create_status, 200,
                "case {case:?}: expected CreateRotation to admit a modelled day, \
                 got {create_status} {create_body:?}"
            );
            let (update_status, update_body) = emulator
                .call(
                    "UpdateRotation",
                    json!({"RotationId": baseline, "Recurrence": recurrence.clone()}),
                )
                .await;
            assert_eq!(
                update_status, 200,
                "case {case:?}: expected UpdateRotation to admit a modelled day, \
                 got {update_status} {update_body:?}"
            );
            let stored = stored_recurrence(&emulator, &baseline, &case).await;
            assert_eq!(
                stored, recurrence,
                "case {case:?}: expected the installed recurrence, got {stored:?}"
            );
        }
    }

    // One step outside the set, in each form a client plausibly sends.
    for outside in ["mon", "Mon", "MONDAY", "MO", "SUNDAY", "", "SUN "] {
        for (place, recurrence) in [
            (
                "a weekly hand-off",
                json!({
                    "WeeklySettings": [
                        {"DayOfWeek": outside, "HandOffTime": {"HourOfDay": 9, "MinuteOfHour": 0}},
                    ],
                    "NumberOfOnCalls": 1,
                    "RecurrenceMultiplier": 1,
                }),
            ),
            (
                "a shift coverage key",
                json!({
                    "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
                    "ShiftCoverages": coverage_on(outside),
                    "NumberOfOnCalls": 1,
                    "RecurrenceMultiplier": 1,
                }),
            ),
        ] {
            let case = format!("{outside:?} named by {place}");
            let (emulator, contact, baseline) = fixture(&case).await;

            let (create_status, create_body) = emulator
                .call(
                    "CreateRotation",
                    json!({
                        "Name": "under-test",
                        "ContactIds": [contact],
                        "TimeZoneId": "UTC",
                        "Recurrence": recurrence.clone(),
                    }),
                )
                .await;
            let (update_status, update_body) = emulator
                .call(
                    "UpdateRotation",
                    json!({"RotationId": baseline, "Recurrence": recurrence.clone()}),
                )
                .await;
            assert_eq!(
                create_status, 400,
                "case {case:?}: expected CreateRotation to refuse with HTTP 400, \
                 got {create_status} {create_body:?}"
            );
            assert_eq!(
                update_status, 400,
                "case {case:?}: expected UpdateRotation to refuse with HTTP 400, \
                 got {update_status} {update_body:?}"
            );
            assert_eq!(
                error_message(&create_body),
                error_message(&update_body),
                "case {case:?}: expected one refusal message, got {create_body:?} and \
                 {update_body:?}"
            );
            assert!(
                !error_message(&create_body).is_empty(),
                "case {case:?}: expected a refusal message, got {create_body:?}"
            );
            let stored = stored_recurrence(&emulator, &baseline, &case).await;
            assert_eq!(
                stored,
                baseline_recurrence(),
                "case {case:?}: expected the stored recurrence untouched, got {stored:?}"
            );
        }
    }
}
