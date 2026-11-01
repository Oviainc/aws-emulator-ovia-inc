// SPDX-License-Identifier: CC0-1.0

//! `ListRotationShifts` end to end over the wire.
//!
//! Every case drives the crate's ONE integration-test harness (`contact_lifecycle`'s
//! `Emulator`) with a buffered awsJson1_1 request (`POST /`,
//! `X-Amz-Target: SSMContacts.ListRotationShifts`), so what is asserted is what a signing AWS
//! client would see — routing, deserialization, constraint validation, the handler and
//! serialization included. Every rotation and override under test is created through
//! `CreateRotation`/`CreateRotationOverride`; nothing is placed in the store by hand except
//! the account's replication set, which no modelled operation of this service can mint.
//!
//! The rules under test, each as a TABLE rather than a single call. The computation itself is
//! `crate::state::rotation_shifts`, the one this service answers both shift listings from
//! (`tests/all/rotation_shift_preview.rs` drives it through the preview); what these cases
//! assert is that a STORED rotation's own fields and overrides are what it is handed, and the
//! range, page and refusals this operation adds:
//!
//! 1. THE SCHEDULE. The listing reports the shifts the stored rotation's own schedule
//!    generates in the requested range, read in the rotation's time zone: a hand-off happens
//!    at each time of day the recurrence names, on the day of the week or of the month it
//!    names, once every `RecurrenceMultiplier` periods counted from the rotation's start time;
//!    a shift runs from one hand-off to the next; and the contacts on call are the next
//!    `NumberOfOnCalls` members of the team in its stored shift order, advancing by that many
//!    at every hand-off. The rows differ along what that generalises over: the recurrence form
//!    (daily, two hand-offs a day, a multiplier above one, weekly, monthly), a day of the
//!    month a month has no such day for, the size of the team and how many of it are on call
//!    at once, whether `ShiftCoverages` narrows the shifts (including a window that crosses
//!    midnight), and the zone — a fixed one and one that moves its clocks, over the day it
//!    moves them.
//! 2. THE RANGE. A shift is reported when its own window covers an instant of the requested
//!    range, so the shift a range begins inside is reported whole, and a request naming no
//!    `StartTime` is answered from where the rotation itself goes into effect.
//! 3. THE OVERRIDES. The overrides STORED against the rotation are what narrow it: the piece of
//!    a shift an override covers is reported as an overridden shift carrying the override's
//!    contacts and the contacts it displaced, the rest of the shift stays regular, and where
//!    two of them cover one piece the first that covers it whole holds it.
//! 4. THE PAGINATION. `MaxResults` bounds a page and the opaque `NextToken` resumes after it,
//!    so walking every page yields each shift exactly once in the same order one unpaginated
//!    call reports, and the last page carries no token. A token this list did not issue is
//!    refused.
//! 5. THE READ. The listing computes its answer from the stored rotation, so two identical
//!    calls answer identically and neither changes what a later caller sees.
//! 6. THE REFUSALS. A rotation the store does not hold answers 404, a range whose end precedes
//!    its start answers 400, and an account with no ACTIVE replication set is refused 400
//!    before the rotation is resolved.

use serde_json::{json, Value};

use crate::contact_lifecycle::{error_code, Emulator, ACCOUNT, REGION};
use crate::contact_updates::set_replication_set;
use crate::rotation_overrides::ensure_contacts;

/// 2025-01-01T00:00:00Z, a Wednesday: the instant every rotation under test starts at, so a
/// row's expected hand-offs are offsets from one named instant.
const JAN_1: i64 = 1_735_689_600;

const HOUR: i64 = 3_600;
const DAY: i64 = 24 * HOUR;

/// 2025-01-31T09:00:00Z, the first hand-off of a rotation that hands off on the 31st.
const JAN_31_0900: i64 = JAN_1 + 30 * DAY + 9 * HOUR;

/// 2025-03-31T09:00:00Z, the next 31st there is: February has none, so it hands off not at all.
const MAR_31_0900: i64 = JAN_1 + 89 * DAY + 9 * HOUR;

/// 2025-05-31T09:00:00Z, the 31st after March's, April having none either.
const MAY_31_0900: i64 = JAN_1 + 150 * DAY + 9 * HOUR;

/// 2025-03-07T10:30:00Z — 02:30 in `America/Los_Angeles`, two days before that zone advances
/// its clocks.
const LA_HAND_OFF_MAR_7: i64 = 1_741_343_400;

/// 2025-03-08T10:30:00Z — 02:30 local, the last hand-off before the clocks move.
const LA_HAND_OFF_MAR_8: i64 = 1_741_429_800;

/// 2025-03-09T10:30:00Z — on the day the zone advances its clocks the local clock SKIPS 02:30
/// altogether, so the hand-off is read an hour on, at 03:30 local, which is the first instant
/// the zone reaches after the jump.
const LA_HAND_OFF_MAR_9: i64 = 1_741_516_200;

/// 2025-03-10T09:30:00Z — 02:30 local again, now an hour earlier in UTC than before the move.
const LA_HAND_OFF_MAR_10: i64 = 1_741_599_000;

/// 2025-03-11T09:30:00Z, which ends the shift that began the day before.
const LA_HAND_OFF_MAR_11: i64 = 1_741_685_400;

/// One shift the listing must report: its window, and which members of the team — by their
/// index in the rotation's shift order — are on call for it.
struct Shift {
    start: i64,
    end: i64,
    on_call: &'static [usize],
}

/// One schedule row: the rotation to create, the range to ask for, and the shifts that answers.
struct Row {
    case: &'static str,
    /// How many contacts the rotation's team holds, in shift order.
    team: usize,
    /// The recurrence's `NumberOfOnCalls`.
    on_calls: i64,
    /// The rotation's `TimeZoneId`.
    zone: &'static str,
    /// The recurrence's settings, beside `NumberOfOnCalls` and `RecurrenceMultiplier`.
    settings: Value,
    /// The recurrence's `RecurrenceMultiplier`.
    multiplier: i64,
    /// The range asked for. `None` as a start names no `StartTime` at all.
    from: Option<i64>,
    to: i64,
    expected: Vec<Shift>,
}

fn shift(start: i64, end: i64, on_call: &'static [usize]) -> Shift {
    Shift {
        start,
        end,
        on_call,
    }
}

/// The schedule rows.
///
/// The hand-off times, days of the week and days of the month are the members
/// `RecurrenceSettings` carries (`smithy_query(query="shape",shape="RecurrenceSettings")`), and
/// the zone names are IANA ones, which is the format `TimeZoneId` documents.
fn schedule_rows() -> Vec<Row> {
    vec![
        Row {
            case: "one daily hand-off, a team of three, one on call",
            team: 3,
            on_calls: 1,
            zone: "UTC",
            settings: json!({"DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}]}),
            multiplier: 1,
            from: Some(JAN_1),
            to: JAN_1 + 4 * DAY,
            expected: vec![
                shift(JAN_1 + 9 * HOUR, JAN_1 + DAY + 9 * HOUR, &[0]),
                shift(JAN_1 + DAY + 9 * HOUR, JAN_1 + 2 * DAY + 9 * HOUR, &[1]),
                shift(JAN_1 + 2 * DAY + 9 * HOUR, JAN_1 + 3 * DAY + 9 * HOUR, &[2]),
                shift(JAN_1 + 3 * DAY + 9 * HOUR, JAN_1 + 4 * DAY + 9 * HOUR, &[0]),
            ],
        },
        Row {
            case: "a daily rotation that lasts three days",
            team: 2,
            on_calls: 1,
            zone: "UTC",
            settings: json!({"DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}]}),
            multiplier: 3,
            from: Some(JAN_1),
            to: JAN_1 + 10 * DAY,
            expected: vec![
                shift(JAN_1 + 9 * HOUR, JAN_1 + 3 * DAY + 9 * HOUR, &[0]),
                shift(JAN_1 + 3 * DAY + 9 * HOUR, JAN_1 + 6 * DAY + 9 * HOUR, &[1]),
                shift(JAN_1 + 6 * DAY + 9 * HOUR, JAN_1 + 9 * DAY + 9 * HOUR, &[0]),
                shift(JAN_1 + 9 * DAY + 9 * HOUR, JAN_1 + 12 * DAY + 9 * HOUR, &[1]),
            ],
        },
        Row {
            case: "two hand-offs a day, so two shifts a day",
            team: 2,
            on_calls: 1,
            zone: "UTC",
            settings: json!({
                "DailySettings": [
                    {"HourOfDay": 9, "MinuteOfHour": 0},
                    {"HourOfDay": 17, "MinuteOfHour": 0},
                ],
            }),
            multiplier: 1,
            from: Some(JAN_1),
            to: JAN_1 + 2 * DAY,
            expected: vec![
                shift(JAN_1 + 9 * HOUR, JAN_1 + 17 * HOUR, &[0]),
                shift(JAN_1 + 17 * HOUR, JAN_1 + DAY + 9 * HOUR, &[1]),
                shift(JAN_1 + DAY + 9 * HOUR, JAN_1 + DAY + 17 * HOUR, &[0]),
                shift(JAN_1 + DAY + 17 * HOUR, JAN_1 + 2 * DAY + 9 * HOUR, &[1]),
            ],
        },
        Row {
            case: "a weekly hand-off, on the first such day after the rotation started",
            team: 2,
            on_calls: 1,
            zone: "UTC",
            settings: json!({
                "WeeklySettings": [
                    {"DayOfWeek": "MON", "HandOffTime": {"HourOfDay": 9, "MinuteOfHour": 0}},
                ],
            }),
            multiplier: 1,
            from: Some(JAN_1),
            to: JAN_1 + 21 * DAY,
            expected: vec![
                shift(JAN_1 + 5 * DAY + 9 * HOUR, JAN_1 + 12 * DAY + 9 * HOUR, &[0]),
                shift(JAN_1 + 12 * DAY + 9 * HOUR, JAN_1 + 19 * DAY + 9 * HOUR, &[1]),
                shift(JAN_1 + 19 * DAY + 9 * HOUR, JAN_1 + 26 * DAY + 9 * HOUR, &[0]),
            ],
        },
        Row {
            case: "a monthly hand-off on the 31st, which a shorter month has no day for",
            team: 1,
            on_calls: 1,
            zone: "UTC",
            settings: json!({
                "MonthlySettings": [
                    {"DayOfMonth": 31, "HandOffTime": {"HourOfDay": 9, "MinuteOfHour": 0}},
                ],
            }),
            multiplier: 1,
            from: Some(JAN_1),
            to: JAN_1 + 90 * DAY,
            // February has no 31st, so February hands off not at all and January's shift runs
            // to March's hand-off.
            expected: vec![
                shift(JAN_31_0900, MAR_31_0900, &[0]),
                shift(MAR_31_0900, MAY_31_0900, &[0]),
            ],
        },
        Row {
            case: "two of four contacts on call at once",
            team: 4,
            on_calls: 2,
            zone: "UTC",
            settings: json!({"DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}]}),
            multiplier: 1,
            from: Some(JAN_1),
            to: JAN_1 + 3 * DAY,
            expected: vec![
                shift(JAN_1 + 9 * HOUR, JAN_1 + DAY + 9 * HOUR, &[0, 1]),
                shift(JAN_1 + DAY + 9 * HOUR, JAN_1 + 2 * DAY + 9 * HOUR, &[2, 3]),
                shift(JAN_1 + 2 * DAY + 9 * HOUR, JAN_1 + 3 * DAY + 9 * HOUR, &[0, 1]),
            ],
        },
        Row {
            case: "two of three contacts on call, so the team wraps mid-shift",
            team: 3,
            on_calls: 2,
            zone: "UTC",
            settings: json!({"DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}]}),
            multiplier: 1,
            from: Some(JAN_1),
            to: JAN_1 + 3 * DAY,
            expected: vec![
                shift(JAN_1 + 9 * HOUR, JAN_1 + DAY + 9 * HOUR, &[0, 1]),
                shift(JAN_1 + DAY + 9 * HOUR, JAN_1 + 2 * DAY + 9 * HOUR, &[2, 0]),
                shift(JAN_1 + 2 * DAY + 9 * HOUR, JAN_1 + 3 * DAY + 9 * HOUR, &[1, 2]),
            ],
        },
        Row {
            case: "shift coverage on the working days only",
            team: 3,
            on_calls: 1,
            zone: "UTC",
            settings: json!({
                "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
                "ShiftCoverages": {
                    "MON": [working_day()],
                    "TUE": [working_day()],
                    "WED": [working_day()],
                    "THU": [working_day()],
                    "FRI": [working_day()],
                },
            }),
            multiplier: 1,
            from: Some(JAN_1),
            to: JAN_1 + 6 * DAY,
            expected: vec![
                shift(JAN_1 + 9 * HOUR, JAN_1 + 17 * HOUR, &[0]),
                shift(JAN_1 + DAY + 9 * HOUR, JAN_1 + DAY + 17 * HOUR, &[1]),
                shift(JAN_1 + 2 * DAY + 9 * HOUR, JAN_1 + 2 * DAY + 17 * HOUR, &[2]),
                shift(JAN_1 + 5 * DAY + 9 * HOUR, JAN_1 + 5 * DAY + 17 * HOUR, &[2]),
            ],
        },
        Row {
            case: "shift coverage that crosses midnight",
            team: 1,
            on_calls: 1,
            zone: "UTC",
            settings: json!({
                "DailySettings": [{"HourOfDay": 12, "MinuteOfHour": 0}],
                "ShiftCoverages": {
                    "WED": [{
                        "Start": {"HourOfDay": 22, "MinuteOfHour": 0},
                        "End": {"HourOfDay": 6, "MinuteOfHour": 0},
                    }],
                },
            }),
            multiplier: 1,
            from: Some(JAN_1),
            to: JAN_1 + DAY,
            expected: vec![shift(JAN_1 + 22 * HOUR, JAN_1 + DAY + 6 * HOUR, &[0])],
        },
        Row {
            case: "a zone that moves its clocks, over the day it moves them",
            team: 1,
            on_calls: 1,
            zone: "America/Los_Angeles",
            settings: json!({"DailySettings": [{"HourOfDay": 2, "MinuteOfHour": 30}]}),
            multiplier: 1,
            from: Some(LA_HAND_OFF_MAR_8 - 18 * HOUR),
            to: LA_HAND_OFF_MAR_10 + 14 * HOUR,
            expected: vec![
                shift(LA_HAND_OFF_MAR_7, LA_HAND_OFF_MAR_8, &[0]),
                shift(LA_HAND_OFF_MAR_8, LA_HAND_OFF_MAR_9, &[0]),
                shift(LA_HAND_OFF_MAR_9, LA_HAND_OFF_MAR_10, &[0]),
                shift(LA_HAND_OFF_MAR_10, LA_HAND_OFF_MAR_11, &[0]),
            ],
        },
        Row {
            case: "a range that names no start, so it is answered from the rotation's own",
            team: 2,
            on_calls: 1,
            zone: "UTC",
            settings: json!({"DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}]}),
            multiplier: 1,
            from: None,
            to: JAN_1 + 3 * DAY,
            expected: vec![
                shift(JAN_1 + 9 * HOUR, JAN_1 + DAY + 9 * HOUR, &[0]),
                shift(JAN_1 + DAY + 9 * HOUR, JAN_1 + 2 * DAY + 9 * HOUR, &[1]),
                shift(JAN_1 + 2 * DAY + 9 * HOUR, JAN_1 + 3 * DAY + 9 * HOUR, &[0]),
            ],
        },
    ]
}

/// A coverage window from 09:00 to 17:00 — one working day.
fn working_day() -> Value {
    json!({
        "Start": {"HourOfDay": 9, "MinuteOfHour": 0},
        "End": {"HourOfDay": 17, "MinuteOfHour": 0},
    })
}

/// The rotation a case creates: what its schedule is made of, which is what every case varies.
struct Schedule {
    /// How many contacts the rotation's team holds, in shift order.
    team: usize,
    /// The recurrence's `NumberOfOnCalls`.
    on_calls: i64,
    /// The rotation's `TimeZoneId`.
    zone: &'static str,
    /// The recurrence's `RecurrenceMultiplier`.
    multiplier: i64,
    /// The recurrence's settings, beside the two members above.
    settings: Value,
}

/// The schedule most cases use: one daily hand-off at 09:00 UTC, with a team of `team` and one
/// of them on call.
fn daily_hand_off(team: usize) -> Schedule {
    Schedule {
        team,
        on_calls: 1,
        zone: "UTC",
        multiplier: 1,
        settings: json!({"DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}]}),
    }
}

/// Create a rotation whose team is the first `schedule.team` contacts, and answer its
/// `RotationArn` beside the team's ARNs in shift order.
async fn create_rotation(
    emulator: &Emulator,
    name: &str,
    schedule: &Schedule,
    case: &str,
) -> (String, Vec<String>) {
    let contacts = ensure_contacts(emulator, schedule.team.max(1), case).await;
    let team: Vec<String> = contacts.into_iter().take(schedule.team).collect();
    let mut recurrence = schedule.settings.clone();
    recurrence["NumberOfOnCalls"] = json!(schedule.on_calls);
    recurrence["RecurrenceMultiplier"] = json!(schedule.multiplier);
    let (status, body) = emulator
        .call(
            "CreateRotation",
            json!({
                "Name": name,
                "ContactIds": team,
                "TimeZoneId": schedule.zone,
                "StartTime": JAN_1,
                "Recurrence": recurrence,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateRotation to answer 200, got {status} {body:?}"
    );
    let arn = body["RotationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a RotationArn, got {body:?}"))
        .to_string();
    (arn, team)
}

/// Every shift `ListRotationShifts` reports for `request`, paging to exhaustion, asserting no
/// page is larger than the `MaxResults` the request names.
async fn all_shifts(emulator: &Emulator, request: Value, case: &str) -> Vec<Value> {
    let mut request = request;
    let limit = request["MaxResults"].as_i64();
    let mut shifts: Vec<Value> = Vec::new();
    // A page that reports nothing can still carry a token — the walk may stop on its work
    // ceiling before it reaches a shift — so the loop is bounded by the number of PAGES it
    // asks for as well as by the shifts it collects, and a listing that does not end fails
    // here instead of hanging.
    for page_number in 0..512 {
        let (status, body) = emulator
            .call("ListRotationShifts", request.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListRotationShifts to answer 200, got {status} {body:?}"
        );
        let page = body["RotationShifts"].as_array().cloned().unwrap_or_default();
        if let Some(limit) = limit.filter(|limit| *limit > 0) {
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= limit,
                "case {case:?}: a page held {} shifts for MaxResults={limit}",
                page.len()
            );
        }
        shifts.extend(page);
        match body["NextToken"].as_str() {
            Some(token) => {
                assert!(
                    shifts.len() <= 256,
                    "case {case:?}: paging did not terminate, collected {} shifts over \
                     {page_number} page(s)",
                    shifts.len()
                );
                request["NextToken"] = json!(token);
            }
            None => return shifts,
        }
    }
    panic!("case {case:?}: paging did not terminate within 512 pages, collected {shifts:?}")
}

/// The `ContactIds` a reported shift carries.
fn reported_contacts(shift: &Value) -> Vec<String> {
    shift["ContactIds"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|arn| arn.as_str().map(str::to_string))
        .collect()
}

/// The contacts a row expects on call, as ARNs.
fn expected_contacts(team: &[String], on_call: &[usize]) -> Vec<String> {
    on_call.iter().map(|index| team[*index].clone()).collect()
}

/// Assert one reported shift against what the row expects of it.
fn assert_shift(reported: &Value, expected: &Shift, team: &[String], case: &str) {
    assert_eq!(
        reported["StartTime"].as_f64(),
        Some(expected.start as f64),
        "case {case:?}: expected a shift starting at {}, got {reported:?}",
        expected.start
    );
    assert_eq!(
        reported["EndTime"].as_f64(),
        Some(expected.end as f64),
        "case {case:?}: expected the shift at {} to end at {}, got {reported:?}",
        expected.start,
        expected.end
    );
    assert_eq!(
        reported["Type"].as_str(),
        Some("REGULAR"),
        "case {case:?}: expected the shift at {} to be a REGULAR one, got {reported:?}",
        expected.start
    );
    assert!(
        reported.get("ShiftDetails").is_none(),
        "case {case:?}: a shift no override touched carries no ShiftDetails, got {reported:?}"
    );
    assert_eq!(
        reported_contacts(reported),
        expected_contacts(team, expected.on_call),
        "case {case:?}: wrong team on call for the shift at {}, got {reported:?}",
        expected.start
    );
}

// ---------------------------------------------------------------------------
// Rules 1 and 2: the schedule, and the range it is reported over
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_listing_reports_the_shifts_the_stored_schedule_generates() {
    for row in schedule_rows() {
        let emulator = Emulator::new();
        let (rotation, team) = create_rotation(
            &emulator,
            "shift-schedule",
            &Schedule {
                team: row.team,
                on_calls: row.on_calls,
                zone: row.zone,
                multiplier: row.multiplier,
                settings: row.settings.clone(),
            },
            row.case,
        )
        .await;

        let mut request = json!({"RotationId": rotation, "EndTime": row.to});
        if let Some(from) = row.from {
            request["StartTime"] = json!(from);
        }
        let (status, body) = emulator.call("ListRotationShifts", request).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected ListRotationShifts to answer 200, got {status} {body:?}",
            row.case
        );
        let reported = body["RotationShifts"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            reported.len(),
            row.expected.len(),
            "case {:?}: expected {} shifts, got {reported:?}",
            row.case,
            row.expected.len()
        );
        for (answered, expected) in reported.iter().zip(&row.expected) {
            assert_shift(answered, expected, &team, row.case);
        }

        // Whatever the row's form, every reported shift covers an instant of the range asked
        // for and none of them overlap.
        let from = row.from.unwrap_or(JAN_1);
        let mut previous_end: Option<f64> = None;
        for answered in &reported {
            let start = answered["StartTime"].as_f64().unwrap_or_default();
            let end = answered["EndTime"].as_f64().unwrap_or_default();
            assert!(
                start < end,
                "case {:?}: a shift must cover a span of time, got {answered:?}",
                row.case
            );
            assert!(
                end > from as f64 && start <= row.to as f64,
                "case {:?}: a reported shift must cover an instant of [{from}, {}], \
                 got {answered:?}",
                row.case,
                row.to
            );
            if let Some(previous_end) = previous_end {
                assert!(
                    start >= previous_end,
                    "case {:?}: shifts must not overlap, got {answered:?} after a shift \
                     ending at {previous_end}",
                    row.case
                );
            }
            previous_end = Some(end);
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a rotation override replaces the team for its own window
// ---------------------------------------------------------------------------

/// One override to create: its window, and which contact replaces the team for it.
struct Override {
    start: i64,
    end: i64,
    /// The index, in the contact pool, of the contact that takes over.
    replacement: usize,
}

/// What a row expects of one reported shift: the window, and either the team (regular) or the
/// replacement that holds it (overridden).
struct Segment {
    start: i64,
    end: i64,
    /// `None` for a regular shift, else the pool index of the contact on call instead.
    replacement: Option<usize>,
}

fn regular(start: i64, end: i64) -> Segment {
    Segment {
        start,
        end,
        replacement: None,
    }
}

fn overridden(start: i64, end: i64, replacement: usize) -> Segment {
    Segment {
        start,
        end,
        replacement: Some(replacement),
    }
}

struct OverrideRow {
    case: &'static str,
    /// The overrides to create, in this order — which is the order that decides which of two
    /// overlapping ones holds.
    overrides: Vec<Override>,
    expected: Vec<Segment>,
}

/// The override rows. The rotation is the same in each: a daily hand-off at 09:00 UTC with a
/// team of one, so what varies is only where the override's window sits relative to a shift.
fn override_rows() -> Vec<OverrideRow> {
    vec![
        OverrideRow {
            case: "an override inside one shift",
            overrides: vec![Override {
                start: JAN_1 + DAY + 12 * HOUR,
                end: JAN_1 + DAY + 15 * HOUR,
                replacement: 1,
            }],
            expected: vec![
                regular(JAN_1 + 9 * HOUR, JAN_1 + DAY + 9 * HOUR),
                regular(JAN_1 + DAY + 9 * HOUR, JAN_1 + DAY + 12 * HOUR),
                overridden(JAN_1 + DAY + 12 * HOUR, JAN_1 + DAY + 15 * HOUR, 1),
                regular(JAN_1 + DAY + 15 * HOUR, JAN_1 + 2 * DAY + 9 * HOUR),
                regular(JAN_1 + 2 * DAY + 9 * HOUR, JAN_1 + 3 * DAY + 9 * HOUR),
            ],
        },
        OverrideRow {
            case: "an override covering a whole shift",
            overrides: vec![Override {
                start: JAN_1 + DAY + 9 * HOUR,
                end: JAN_1 + 2 * DAY + 9 * HOUR,
                replacement: 1,
            }],
            expected: vec![
                regular(JAN_1 + 9 * HOUR, JAN_1 + DAY + 9 * HOUR),
                overridden(JAN_1 + DAY + 9 * HOUR, JAN_1 + 2 * DAY + 9 * HOUR, 1),
                regular(JAN_1 + 2 * DAY + 9 * HOUR, JAN_1 + 3 * DAY + 9 * HOUR),
            ],
        },
        OverrideRow {
            case: "an override straddling a hand-off, which touches both shifts",
            overrides: vec![Override {
                start: JAN_1 + DAY + 6 * HOUR,
                end: JAN_1 + DAY + 12 * HOUR,
                replacement: 1,
            }],
            expected: vec![
                regular(JAN_1 + 9 * HOUR, JAN_1 + DAY + 6 * HOUR),
                overridden(JAN_1 + DAY + 6 * HOUR, JAN_1 + DAY + 9 * HOUR, 1),
                overridden(JAN_1 + DAY + 9 * HOUR, JAN_1 + DAY + 12 * HOUR, 1),
                regular(JAN_1 + DAY + 12 * HOUR, JAN_1 + 2 * DAY + 9 * HOUR),
                regular(JAN_1 + 2 * DAY + 9 * HOUR, JAN_1 + 3 * DAY + 9 * HOUR),
            ],
        },
        OverrideRow {
            case: "two overlapping overrides, the first that covers a piece holding it",
            overrides: vec![
                Override {
                    start: JAN_1 + DAY + 10 * HOUR,
                    end: JAN_1 + DAY + 14 * HOUR,
                    replacement: 1,
                },
                Override {
                    start: JAN_1 + DAY + 12 * HOUR,
                    end: JAN_1 + DAY + 16 * HOUR,
                    replacement: 2,
                },
            ],
            expected: vec![
                regular(JAN_1 + 9 * HOUR, JAN_1 + DAY + 9 * HOUR),
                regular(JAN_1 + DAY + 9 * HOUR, JAN_1 + DAY + 10 * HOUR),
                overridden(JAN_1 + DAY + 10 * HOUR, JAN_1 + DAY + 12 * HOUR, 1),
                // Both cover this piece whole; the first of them that does holds it.
                overridden(JAN_1 + DAY + 12 * HOUR, JAN_1 + DAY + 14 * HOUR, 1),
                overridden(JAN_1 + DAY + 14 * HOUR, JAN_1 + DAY + 16 * HOUR, 2),
                regular(JAN_1 + DAY + 16 * HOUR, JAN_1 + 2 * DAY + 9 * HOUR),
                regular(JAN_1 + 2 * DAY + 9 * HOUR, JAN_1 + 3 * DAY + 9 * HOUR),
            ],
        },
        OverrideRow {
            case: "an override of a window the range does not reach",
            overrides: vec![Override {
                start: JAN_1 + 30 * DAY,
                end: JAN_1 + 31 * DAY,
                replacement: 1,
            }],
            expected: vec![
                regular(JAN_1 + 9 * HOUR, JAN_1 + DAY + 9 * HOUR),
                regular(JAN_1 + DAY + 9 * HOUR, JAN_1 + 2 * DAY + 9 * HOUR),
                regular(JAN_1 + 2 * DAY + 9 * HOUR, JAN_1 + 3 * DAY + 9 * HOUR),
            ],
        },
    ]
}

#[tokio::test]
async fn an_override_replaces_the_team_for_exactly_its_own_window() {
    for row in override_rows() {
        let emulator = Emulator::new();
        // Three contacts: the rotation's own team of one, and the two replacements the rows
        // hand a shift to.
        let pool = ensure_contacts(&emulator, 3, row.case).await;
        let (rotation, team) = create_rotation(
            &emulator,
            "override-schedule",
            &daily_hand_off(1),
            row.case,
        )
        .await;

        for window in &row.overrides {
            let (status, body) = emulator
                .call(
                    "CreateRotationOverride",
                    json!({
                        "RotationId": rotation,
                        "NewContactIds": [pool[window.replacement].clone()],
                        "StartTime": window.start,
                        "EndTime": window.end,
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected CreateRotationOverride to answer 200, got {status} {body:?}",
                row.case
            );
        }

        let (status, body) = emulator
            .call(
                "ListRotationShifts",
                json!({
                    "RotationId": rotation,
                    "StartTime": JAN_1,
                    "EndTime": JAN_1 + 3 * DAY,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected ListRotationShifts to answer 200, got {status} {body:?}",
            row.case
        );
        let reported = body["RotationShifts"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            reported.len(),
            row.expected.len(),
            "case {:?}: expected {} shifts, got {reported:?}",
            row.case,
            row.expected.len()
        );
        for (answered, expected) in reported.iter().zip(&row.expected) {
            assert_eq!(
                answered["StartTime"].as_f64(),
                Some(expected.start as f64),
                "case {:?}: expected a shift starting at {}, got {answered:?}",
                row.case,
                expected.start
            );
            assert_eq!(
                answered["EndTime"].as_f64(),
                Some(expected.end as f64),
                "case {:?}: expected the shift at {} to end at {}, got {answered:?}",
                row.case,
                expected.start,
                expected.end
            );
            match expected.replacement {
                Some(replacement) => {
                    assert_eq!(
                        answered["Type"].as_str(),
                        Some("OVERRIDDEN"),
                        "case {:?}: the shift at {} is covered by an override, got {answered:?}",
                        row.case,
                        expected.start
                    );
                    assert_eq!(
                        reported_contacts(answered),
                        vec![pool[replacement].clone()],
                        "case {:?}: expected the override's contact on call at {}, \
                         got {answered:?}",
                        row.case,
                        expected.start
                    );
                    assert_eq!(
                        answered["ShiftDetails"]["OverriddenContactIds"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default()
                            .iter()
                            .filter_map(|arn| arn.as_str().map(str::to_string))
                            .collect::<Vec<String>>(),
                        team,
                        "case {:?}: an overridden shift reports the contacts it replaced, \
                         got {answered:?}",
                        row.case
                    );
                }
                None => {
                    assert_eq!(
                        answered["Type"].as_str(),
                        Some("REGULAR"),
                        "case {:?}: the shift at {} is covered by no override, got {answered:?}",
                        row.case,
                        expected.start
                    );
                    assert_eq!(
                        reported_contacts(answered),
                        team,
                        "case {:?}: expected the rotation's own team on call at {}, \
                         got {answered:?}",
                        row.case,
                        expected.start
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: pagination
// ---------------------------------------------------------------------------

/// One paging row: the rotation to page the shifts of, and the fewest shifts a week of it
/// holds, so a page size of one really pages.
///
/// The rows put a page boundary in every place one can fall: between two shifts, inside a
/// shift its coverage narrows into two reported parts, and inside a shift a rotation override
/// splits into three.
struct PagingCase {
    case: &'static str,
    schedule: Schedule,
    overrides: Vec<Override>,
    least: usize,
}

fn paging_cases() -> Vec<PagingCase> {
    vec![
        PagingCase {
            case: "two hand-offs a day",
            schedule: Schedule {
                team: 3,
                on_calls: 1,
                zone: "UTC",
                multiplier: 1,
                settings: json!({
                    "DailySettings": [
                        {"HourOfDay": 9, "MinuteOfHour": 0},
                        {"HourOfDay": 21, "MinuteOfHour": 0},
                    ],
                }),
            },
            overrides: Vec::new(),
            least: 14,
        },
        PagingCase {
            case: "one hand-off a day, narrowed into two parts a day by its coverage",
            schedule: Schedule {
                team: 2,
                on_calls: 1,
                zone: "UTC",
                multiplier: 1,
                settings: json!({
                    "DailySettings": [{"HourOfDay": 0, "MinuteOfHour": 0}],
                    "ShiftCoverages": {
                        "MON": [morning(), afternoon()],
                        "TUE": [morning(), afternoon()],
                        "WED": [morning(), afternoon()],
                        "THU": [morning(), afternoon()],
                        "FRI": [morning(), afternoon()],
                        "SAT": [morning(), afternoon()],
                        "SUN": [morning(), afternoon()],
                    },
                }),
            },
            overrides: Vec::new(),
            least: 12,
        },
        PagingCase {
            case: "one hand-off a day, with an override splitting one shift into three",
            schedule: daily_hand_off(2),
            overrides: vec![Override {
                start: JAN_1 + DAY + 12 * HOUR,
                end: JAN_1 + DAY + 15 * HOUR,
                replacement: 2,
            }],
            least: 9,
        },
        PagingCase {
            case: "one hand-off a day, with an override straddling two hand-offs",
            schedule: daily_hand_off(2),
            overrides: vec![Override {
                start: JAN_1 + 2 * DAY + 12 * HOUR,
                end: JAN_1 + 4 * DAY + 3 * HOUR,
                replacement: 2,
            }],
            least: 9,
        },
        PagingCase {
            case: "coverage that runs contiguously from one day into the next",
            schedule: Schedule {
                team: 2,
                on_calls: 1,
                zone: "UTC",
                multiplier: 1,
                settings: json!({
                    "DailySettings": [{"HourOfDay": 6, "MinuteOfHour": 0}],
                    // Every other day's coverage ends at midnight and the day after it begins
                    // there, so the two are one unbroken part of a shift rather than two.
                    "ShiftCoverages": {
                        "MON": [until_midnight()],
                        "TUE": [from_midnight()],
                        "WED": [until_midnight()],
                        "THU": [from_midnight()],
                        "FRI": [until_midnight()],
                        "SAT": [from_midnight()],
                        "SUN": [until_midnight()],
                    },
                }),
            },
            overrides: Vec::new(),
            least: 5,
        },
    ]
}

/// A coverage window from the morning to midnight — its end does not follow its start, so it
/// runs into the next day.
fn until_midnight() -> Value {
    json!({
        "Start": {"HourOfDay": 8, "MinuteOfHour": 0},
        "End": {"HourOfDay": 0, "MinuteOfHour": 0},
    })
}

/// A coverage window from midnight to midday, which begins exactly where the previous day's
/// window ended.
fn from_midnight() -> Value {
    json!({
        "Start": {"HourOfDay": 0, "MinuteOfHour": 0},
        "End": {"HourOfDay": 12, "MinuteOfHour": 0},
    })
}

/// A coverage window over the morning, and one over the afternoon: two windows of the same day,
/// so one shift is reported as two parts.
fn morning() -> Value {
    json!({
        "Start": {"HourOfDay": 8, "MinuteOfHour": 0},
        "End": {"HourOfDay": 12, "MinuteOfHour": 0},
    })
}

/// A `ShiftCoverages` map naming `windows` on every day of the week
/// ([`crate::rotation_shifts`]'s rows name single days; this covers the lot).
fn coverage_every_day(windows: &[Value]) -> Value {
    let mut map = serde_json::Map::new();
    // The modelled `DayOfWeek` value set
    // (`smithy_query(query="shape",shape="DayOfWeek")`).
    for day in ["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"] {
        map.insert(day.to_string(), json!(windows));
    }
    Value::Object(map)
}

/// Coverage windows that say nothing another window of the same day does not — the same span
/// named again and again, and spans contained in a longer one — narrow a shift exactly as the
/// window they are redundant with, so the listing is the one that coverage names once.
#[tokio::test]
async fn redundant_coverage_windows_report_the_same_shifts_as_one() {
    let case = "coverage naming the same span many times, and spans inside it";
    let emulator = Emulator::new();

    // The same working day named 200 times, plus every hour inside it as a window of its own.
    let mut crowd: Vec<Value> = (0..200).map(|_| working_day()).collect();
    for hour in 10..16 {
        crowd.push(json!({
            "Start": {"HourOfDay": hour, "MinuteOfHour": 0},
            "End": {"HourOfDay": hour + 1, "MinuteOfHour": 0},
        }));
    }

    let mut listings: Vec<Vec<Value>> = Vec::new();
    for (name, windows) in [("coverage-once", vec![working_day()]), ("coverage-crowd", crowd)] {
        let (rotation, _) = create_rotation(
            &emulator,
            name,
            &Schedule {
                team: 2,
                on_calls: 1,
                zone: "UTC",
                multiplier: 1,
                settings: json!({
                    "DailySettings": [{"HourOfDay": 0, "MinuteOfHour": 0}],
                    "ShiftCoverages": coverage_every_day(&windows),
                }),
            },
            case,
        )
        .await;
        listings.push(
            all_shifts(
                &emulator,
                json!({
                    "RotationId": rotation,
                    "StartTime": JAN_1,
                    "EndTime": JAN_1 + 7 * DAY,
                    "MaxResults": 3,
                }),
                case,
            )
            .await,
        );
    }

    assert!(
        listings[0].len() >= 7,
        "case {case:?}: a week of working days holds a shift a day, got {:?}",
        listings[0]
    );
    assert_eq!(
        listings[1], listings[0],
        "case {case:?}: redundant coverage windows must report the shifts one window reports"
    );
}

/// A coverage window from midday to the evening, beside [`morning`].
fn afternoon() -> Value {
    json!({
        "Start": {"HourOfDay": 13, "MinuteOfHour": 0},
        "End": {"HourOfDay": 18, "MinuteOfHour": 0},
    })
}

#[tokio::test]
async fn paging_yields_every_shift_exactly_once() {
    for row in paging_cases() {
        let case = row.case;
        let emulator = Emulator::new();
        // Three contacts: the rotation's own team, and the replacement an override hands a
        // shift to.
        let pool = ensure_contacts(&emulator, 3, case).await;
        let (rotation, _) = create_rotation(&emulator, "shift-paging", &row.schedule, case).await;
        for window in &row.overrides {
            let (status, body) = emulator
                .call(
                    "CreateRotationOverride",
                    json!({
                        "RotationId": rotation,
                        "NewContactIds": [pool[window.replacement].clone()],
                        "StartTime": window.start,
                        "EndTime": window.end,
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateRotationOverride to answer 200, got {status} \
                 {body:?}"
            );
        }
        let range = json!({
            "RotationId": rotation,
            "StartTime": JAN_1,
            "EndTime": JAN_1 + 7 * DAY,
        });

        let whole = all_shifts(&emulator, range.clone(), case).await;
        assert!(
            whole.len() >= row.least,
            "case {case:?}: a week of this schedule holds at least {} shifts, got {}",
            row.least,
            whole.len()
        );

        // Every page size from one shift a page to more than the whole listing holds.
        for size in [1, 2, 3, 5, whole.len(), whole.len() + 1] {
            let case = format!("{case} at MaxResults={size}");
            let mut request = range.clone();
            request["MaxResults"] = json!(size);
            let paged = all_shifts(&emulator, request, &case).await;
            assert_eq!(
                paged, whole,
                "case {case:?}: paging must yield the same shifts, in the same order, as one \
                 unpaginated call"
            );
        }
    }
}

#[tokio::test]
async fn a_next_token_this_listing_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    let (rotation, _) =
        create_rotation(&emulator, "shift-token", &daily_hand_off(1), "a bad token").await;
    let range = json!({
        "RotationId": rotation,
        "StartTime": JAN_1,
        "EndTime": JAN_1 + 7 * DAY,
    });

    for token in [
        "not-a-token",
        "6e6f742d612d746f6b656e",
        // A token of this list's own scope whose payload is no position this list can have
        // issued: hex of "rotation-shifts\u{1f}999999999".
        "726f746174696f6e2d7368696674731f393939393939393939",
    ] {
        let case = format!("a NextToken this listing did not issue: {token}");
        let mut request = range.clone();
        request["NextToken"] = json!(token);
        let (status, body) = emulator.call("ListRotationShifts", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
    }
}

/// The largest `DailySettings` list a rotation write admits still pages: every entry names the
/// same hand-off time, so the schedule hands off once a day and what grows is only how much
/// reading one turn of the recurrence costs.
#[tokio::test]
async fn a_recurrence_at_the_largest_settings_list_still_pages() {
    let case = "a daily settings list at the largest a rotation write admits";
    let emulator = Emulator::new();
    // A rotation write admits a settings list no longer than the minutes of the period it
    // names (`crate::state::check_recurrence`), and this list is exactly that long.
    let hand_offs: Vec<Value> = (0..1_440)
        .map(|_| json!({"HourOfDay": 9, "MinuteOfHour": 0}))
        .collect();
    let (rotation, _) = create_rotation(
        &emulator,
        "shift-dense-recurrence",
        &Schedule {
            team: 2,
            on_calls: 1,
            zone: "UTC",
            multiplier: 1,
            settings: json!({"DailySettings": hand_offs}),
        },
        case,
    )
    .await;

    let shifts = all_shifts(
        &emulator,
        json!({
            "RotationId": rotation,
            "StartTime": JAN_1,
            "EndTime": JAN_1 + 5 * DAY,
            "MaxResults": 2,
        }),
        case,
    )
    .await;

    let starts: Vec<f64> = shifts
        .iter()
        .map(|shift| shift["StartTime"].as_f64().unwrap_or_default())
        .collect();
    let expected: Vec<f64> = (0..5)
        .map(|day| (JAN_1 + day * DAY + 9 * HOUR) as f64)
        .collect();
    assert_eq!(
        starts, expected,
        "case {case:?}: the same hand-off time named many times hands off once a day, and \
         paging must reach every one of those shifts, got {shifts:?}"
    );
}

/// One shift can span a hundred months — `RecurrenceMultiplier`'s maximum on a monthly
/// recurrence — and a range inside it is answered with the shift it sits in, whose own times
/// are reported rather than the range's.
#[tokio::test]
async fn a_shift_spanning_years_is_answered_from_inside_it() {
    let case = "a one-hour range two years into a shift of a hundred months";
    let emulator = Emulator::new();
    let (rotation, _) = create_rotation(
        &emulator,
        "shift-long",
        &Schedule {
            team: 1,
            on_calls: 1,
            zone: "UTC",
            multiplier: 100,
            settings: json!({
                "MonthlySettings": [
                    {"DayOfMonth": 1, "HandOffTime": {"HourOfDay": 0, "MinuteOfHour": 0}},
                ],
            }),
        },
        case,
    )
    .await;

    // The rotation starts on the first of a month, so its first hand-off is its own start and
    // the next is a hundred months later.
    let from = JAN_1 + 730 * DAY;
    let (status, body) = emulator
        .call(
            "ListRotationShifts",
            json!({
                "RotationId": rotation,
                "StartTime": from,
                "EndTime": from + HOUR,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {body:?}"
    );
    let shifts = body["RotationShifts"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        shifts.len(),
        1,
        "case {case:?}: the range sits inside one shift, got {shifts:?}"
    );
    assert_eq!(
        shifts[0]["StartTime"].as_f64(),
        Some(JAN_1 as f64),
        "case {case:?}: the shift's own start is reported, not the range's, got {:?}",
        shifts[0]
    );
    assert!(
        shifts[0]["EndTime"].as_f64().unwrap_or_default() > (from + HOUR) as f64,
        "case {case:?}: the shift runs past the range it was asked about, got {:?}",
        shifts[0]
    );
}

// ---------------------------------------------------------------------------
// Rule 5: the listing is a read
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_listing_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let (rotation, _) = create_rotation(&emulator, "shift-read", &daily_hand_off(2), "a read").await;
    let request = json!({
        "RotationId": rotation,
        "StartTime": JAN_1,
        "EndTime": JAN_1 + 5 * DAY,
    });

    let (status, first) = emulator.call("ListRotationShifts", request.clone()).await;
    assert_eq!(status, 200, "expected 200, got {status} {first:?}");
    let (status, again) = emulator.call("ListRotationShifts", request.clone()).await;
    assert_eq!(status, 200, "expected 200, got {status} {again:?}");
    assert_eq!(
        first, again,
        "case \"two identical calls\": a read of an unchanged rotation answers the same \
         shifts, got {first:?} then {again:?}"
    );

    // The rotation itself is untouched, so its own read answers exactly what it did before.
    let (status, read) = emulator
        .call("GetRotation", json!({"RotationId": rotation}))
        .await;
    assert_eq!(status, 200, "expected 200, got {status} {read:?}");
    assert_eq!(
        read["StartTime"].as_f64(),
        Some(JAN_1 as f64),
        "case \"the rotation after the listing\": the listing must not move the rotation's \
         start time, got {read:?}"
    );
    let store = service_com_amazonaws_ssmcontacts_ssmcontacts::state::service_state(
        &*emulator.engine,
    )
    .expect("the engine holds this service's state");
    let store = store.lock();
    assert!(
        store.rotation_overrides.is_empty(),
        "case \"the store after the listing\": a listing of shifts writes nothing, got \
         {} overrides",
        store.rotation_overrides.len()
    );
}

// ---------------------------------------------------------------------------
// Rule 6: the refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_rotation_the_store_does_not_hold_answers_404() {
    let emulator = Emulator::new();
    let absent = format!(
        "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:rotation/00000000-0000-0000-0000-000000000000"
    );
    let (status, body) = emulator
        .call(
            "ListRotationShifts",
            json!({"RotationId": absent, "StartTime": JAN_1, "EndTime": JAN_1 + DAY}),
        )
        .await;
    assert_eq!(
        status, 404,
        "case \"a rotation the store does not hold\": expected 404, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case \"a rotation the store does not hold\": expected a ResourceNotFoundException, \
         got {body:?}"
    );
}

#[tokio::test]
async fn a_range_whose_end_precedes_its_start_is_refused() {
    let emulator = Emulator::new();
    let (rotation, _) = create_rotation(
        &emulator,
        "shift-range",
        &daily_hand_off(1),
        "an inverted range",
    )
    .await;
    for (case, from, to) in [
        ("an end a day before the start", JAN_1 + DAY, JAN_1),
        ("an end a second before the start", JAN_1 + 1, JAN_1),
    ] {
        let (status, body) = emulator
            .call(
                "ListRotationShifts",
                json!({"RotationId": rotation, "StartTime": from, "EndTime": to}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
    }

    // A range that is one instant long is a range, and is served.
    let (status, body) = emulator
        .call(
            "ListRotationShifts",
            json!({"RotationId": rotation, "StartTime": JAN_1, "EndTime": JAN_1}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"a range one instant long\": expected 200, got {status} {body:?}"
    );

    // A request that names no `StartTime` named no inverted range either, whatever the
    // rotation's own start time is: an `EndTime` before the schedule begins selects no shift,
    // which is an empty page rather than a refusal.
    for (case, to) in [
        ("an EndTime a day before the rotation started", JAN_1 - DAY),
        ("an EndTime long before the rotation started", JAN_1 - 400 * DAY),
    ] {
        let (status, body) = emulator
            .call(
                "ListRotationShifts",
                json!({"RotationId": rotation, "EndTime": to}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a request naming no StartTime is not an inverted range, \
             got {status} {body:?}"
        );
        assert_eq!(
            body["RotationShifts"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: no shift precedes the rotation's start time, got {body:?}"
        );
        assert!(
            body["NextToken"].as_str().is_none(),
            "case {case:?}: a listing with nothing left to walk carries no token, got {body:?}"
        );
    }
}

/// A range no caller could page to the end of in one call is still walked a page at a time: the
/// pages that come back carry shifts in strictly increasing order, none repeated, and each one
/// says there is more to come.
#[tokio::test]
async fn a_range_far_larger_than_a_page_is_walked_a_page_at_a_time() {
    let case = "a hundred years of shifts, ten at a time";
    let emulator = Emulator::new();
    let (rotation, _) = create_rotation(
        &emulator,
        "shift-long-range",
        &Schedule {
            team: 3,
            on_calls: 1,
            zone: "Asia/Seoul",
            multiplier: 1,
            settings: json!({
                "DailySettings": [{"HourOfDay": 7, "MinuteOfHour": 30}],
                // Coverage on one day of the week only, so most hand-offs of the range report
                // no shift at all and the walk has to keep going to fill a page.
                "ShiftCoverages": {"WED": [morning()]},
            }),
        },
        case,
    )
    .await;

    let mut request = json!({
        "RotationId": rotation,
        "StartTime": JAN_1,
        // A hundred years past the rotation's start.
        "EndTime": JAN_1 + 36_525 * DAY,
        "MaxResults": 10,
    });
    let mut seen: Vec<f64> = Vec::new();
    for page in 0..5 {
        let (status, body) = emulator
            .call("ListRotationShifts", request.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {page} expected 200, got {status} {body:?}"
        );
        let shifts = body["RotationShifts"].as_array().cloned().unwrap_or_default();
        assert!(
            !shifts.is_empty(),
            "case {case:?}: page {page} must report shifts of a range that holds thousands, \
             got {body:?}"
        );
        for shift in &shifts {
            let start = shift["StartTime"].as_f64().unwrap_or_default();
            assert!(
                seen.last().is_none_or(|previous| start > *previous),
                "case {case:?}: page {page} reported a shift at {start} after one at {:?}",
                seen.last()
            );
            seen.push(start);
        }
        let token = body["NextToken"].as_str().unwrap_or_else(|| {
            panic!(
                "case {case:?}: page {page} of a hundred-year range must say there is more to \
                 walk, got {body:?}"
            )
        });
        request["NextToken"] = json!(token);
    }
}

#[tokio::test]
async fn an_account_with_no_active_replication_set_is_refused_before_the_rotation_is_resolved() {
    let emulator = Emulator::new();
    let (rotation, _) = create_rotation(
        &emulator,
        "shift-gate",
        &daily_hand_off(1),
        "the account gate",
    )
    .await;
    let absent = format!(
        "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:rotation/00000000-0000-0000-0000-000000000000"
    );

    // The gate is closed: the account holds no replication set at all.
    set_replication_set(&emulator, None);
    for (case, rotation_id) in [
        ("a rotation the store holds", rotation.as_str()),
        ("a rotation ARN the store never minted", absent.as_str()),
    ] {
        let (status, body) = emulator
            .call(
                "ListRotationShifts",
                json!({
                    "RotationId": rotation_id,
                    "StartTime": JAN_1,
                    "EndTime": JAN_1 + DAY,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a closed account gate is refused 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
    }
}
