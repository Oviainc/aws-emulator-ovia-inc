// SPDX-License-Identifier: CC0-1.0

//! `ListPreviewRotationShifts`, end to end over the wire.
//!
//! The operation answers the shifts a rotation WOULD have, computed from the rotation the
//! request describes. The rules under test, each asserted over a TABLE whose rows differ
//! along what the rule generalises over rather than over one transcript:
//!
//! 1. HAND-OFFS. `RecurrenceSettings` says when one shift ends and the next begins —
//!    `DailySettings` a time of day, `WeeklySettings` a day of the week, `MonthlySettings` a
//!    day of the month, `RecurrenceMultiplier` how many of those periods one turn spans — and
//!    the turns are counted from `RotationStartTime` in the rotation's IANA zone. The cadence
//!    table spans all three settings lists, several multipliers, more than one hand-off time,
//!    a day of the month not every month has, two zones, and both daylight-saving jumps,
//!    which is why its expectations are instants rather than a fixed number of seconds.
//! 2. WHO IS ON CALL. `NumberOfOnCalls` of the `Members`, taken in the order the request
//!    lists them and advancing by that many at every hand-off, walking the team round and
//!    round. The table spans team sizes 1..4 against every `NumberOfOnCalls` the team admits,
//!    with the expected contacts written out as positions in the team.
//! 3. `ShiftCoverages` narrows a shift to the parts of the week the rotation covers: a day of
//!    the week the map does not name is not covered at all. The table spans a window inside a
//!    day, two windows in one day, an open `Start`, an open `End`, a `CoverageTime` carrying
//!    neither, a window running past midnight, and a day of the week the range never reaches.
//! 4. An OVERRIDE replaces who is on call over a window: the pieces of a shift it covers are
//!    `OVERRIDDEN` and carry the contacts they displaced, and every other piece stays
//!    `REGULAR`. The table spans an override inside one shift, one exactly covering a shift,
//!    one spanning two shifts, one with no end, one with several new members, one with none,
//!    two at once, and one outside the range entirely.
//! 5. PAGING. `MaxResults` cuts the page and `NextToken` resumes it, so paging a range to
//!    exhaustion reports exactly what one unpaged call reports, in the same order.
//! 6. The REFUSALS, which are the modelled `ValidationException` (HTTP 400) and never a 501:
//!    the closed account gate, a recurrence that describes no schedule, a zone no zone
//!    database resolves, a `NumberOfOnCalls` the team cannot meet, an inverted range, an
//!    inverted override window and a `NextToken` this service did not issue.
//! 7. A preview WRITES NOTHING: no rotation, no contact and no tag any later caller sees.
//!
//! Every instant in this file is epoch seconds, and every expected one is derived from the
//! calendar rather than from a recorded response: the comments carry the civil reading.

use serde_json::{json, Value};

use crate::contact_lifecycle::{error_code, Emulator};
use crate::contact_updates::set_replication_set;

// ---------------------------------------------------------------------------
// Instants
// ---------------------------------------------------------------------------

/// One hour and one day, in seconds.
const HOUR: i64 = 3_600;
const DAY: i64 = 86_400;

/// 2025-01-01T00:00:00Z, a WEDNESDAY — the instant most rows start their rotation on, chosen
/// because a mid-week anchor is what shows a weekly recurrence being counted from the
/// rotation's own week.
const JAN_1: i64 = 1_735_689_600;

/// 2025-02-01T00:00:00Z, 31 days after [`JAN_1`].
const FEB_1: i64 = JAN_1 + 31 * DAY;

/// 2025-03-01T00:00:00Z, 28 days after [`FEB_1`] — February 2025 is not a leap February.
const MAR_1: i64 = FEB_1 + 28 * DAY;

/// 2025-04-01T00:00:00Z, 31 days after [`MAR_1`].
const APR_1: i64 = MAR_1 + 31 * DAY;

/// 2025-05-01T00:00:00Z, 30 days after [`APR_1`].
const MAY_1: i64 = APR_1 + 30 * DAY;

/// 2025-07-01T00:00:00Z: May has 31 days and June 30.
const JUL_1: i64 = MAY_1 + 31 * DAY + 30 * DAY;

/// 2025-03-07T09:00:00 America/New_York, which is 14:00Z because the zone is on standard time
/// (UTC-5) until the second Sunday of March.
const MAR_7_0900_NEW_YORK: i64 = MAR_1 + 6 * DAY + 14 * HOUR;

/// 2025-10-31T09:00:00 America/New_York, which is 13:00Z because the zone is on daylight time
/// (UTC-4) until the first Sunday of November.
const OCT_31_0900_NEW_YORK: i64 = JAN_1 + 303 * DAY + 13 * HOUR;

// ---------------------------------------------------------------------------
// The harness
// ---------------------------------------------------------------------------

/// One reported shift, in the terms the model gives `RotationShift`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    start: i64,
    end: i64,
    /// `ShiftType`, which the model admits as absent.
    kind: Option<String>,
    contacts: Vec<String>,
    /// `ShiftDetails$OverriddenContactIds`, present only where the shift is overridden.
    overridden: Option<Vec<String>>,
}

/// A modelled timestamp as epoch seconds.
fn secs(value: &Value, case: &str) -> i64 {
    let seconds = value
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a modelled timestamp, got {value:?}"));
    seconds as i64
}

/// The strings of a JSON array member, defaulting to empty when the member is absent.
fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Every shift a `ListPreviewRotationShifts` body reports.
fn reported(body: &Value, case: &str) -> Vec<Reported> {
    let shifts = body["RotationShifts"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: expected a served page carrying RotationShifts, got {body:?}")
    });
    shifts
        .iter()
        .map(|shift| Reported {
            start: secs(&shift["StartTime"], case),
            end: secs(&shift["EndTime"], case),
            kind: shift["Type"].as_str().map(str::to_string),
            contacts: strings(&shift["ContactIds"]),
            overridden: shift
                .get("ShiftDetails")
                .filter(|details| !details.is_null())
                .map(|details| strings(&details["OverriddenContactIds"])),
        })
        .collect()
}

/// Preview `request`, asserting it is served, and answer the shifts of the FIRST page.
async fn preview(emulator: &Emulator, request: Value, case: &str) -> Vec<Reported> {
    let (status, body) = emulator
        .call("ListPreviewRotationShifts", request.clone())
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListPreviewRotationShifts to answer 200 for {request}, got \
         {status} with {body:?}"
    );
    reported(&body, case)
}

/// Preview `request`, paging to exhaustion, and answer every shift reported in order.
async fn preview_to_exhaustion(
    emulator: &Emulator,
    request: Value,
    case: &str,
) -> Vec<Reported> {
    let mut request = request;
    let mut shifts: Vec<Reported> = Vec::new();
    for _ in 0..64 {
        let (status, body) = emulator
            .call("ListPreviewRotationShifts", request.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected every page to answer 200, got {status} with {body:?}"
        );
        shifts.extend(reported(&body, case));
        match body["NextToken"].as_str() {
            Some(token) => request["NextToken"] = json!(token),
            None => return shifts,
        }
    }
    panic!("case {case:?}: paging did not end after 64 pages, collected {} shifts", shifts.len())
}

/// Four PERSONAL contacts, in a stable order, to be a previewed rotation's shift team.
///
/// The model's `Member` admits any non-blank string, and this operation resolves nothing
/// against the store (it declares no `ResourceNotFoundException`), but a rotation's team is
/// contacts, so the team these suites preview is a real one: `RotationShift$ContactIds` is
/// modelled `SsmContactsArn`.
async fn team(emulator: &Emulator) -> Vec<String> {
    let mut members = Vec::new();
    for alias in ["oncall-a", "oncall-b", "oncall-c", "oncall-d"] {
        members.push(
            emulator
                .create_contact(
                    json!({
                        "Alias": alias,
                        "Type": "PERSONAL",
                        "Plan": {"Stages": [{"DurationInMinutes": 5, "Targets": []}]},
                    }),
                    alias,
                )
                .await,
        );
    }
    members
}

/// A recurrence with one hand-off a day at `hour`:00 local, every turn.
fn daily_at(hour: i64, number_of_on_calls: i64) -> Value {
    json!({
        "NumberOfOnCalls": number_of_on_calls,
        "RecurrenceMultiplier": 1,
        "DailySettings": [{"HourOfDay": hour, "MinuteOfHour": 0}],
    })
}

// ---------------------------------------------------------------------------
// Rule 1: when the hand-offs fall
// ---------------------------------------------------------------------------

/// One cadence row: a zone, a rotation start and a recurrence, with the hand-off instants the
/// recurrence puts in the previewed range. Shift `k` is `[boundaries[k], boundaries[k + 1])`,
/// and the range ends on the last boundary, so the row expects exactly one shift fewer than it
/// lists boundaries.
struct Cadence {
    case: &'static str,
    zone: &'static str,
    rotation_start: i64,
    recurrence: Value,
    boundaries: Vec<i64>,
}

fn cadence_rows() -> Vec<Cadence> {
    vec![
        Cadence {
            case: "a daily hand-off at midnight",
            zone: "UTC",
            rotation_start: JAN_1,
            recurrence: daily_at(0, 1),
            boundaries: (0..5).map(|turn| JAN_1 + turn * DAY).collect(),
        },
        Cadence {
            case: "a daily hand-off whose time the rotation does not start on",
            zone: "UTC",
            rotation_start: JAN_1,
            recurrence: daily_at(9, 1),
            // Nothing hands off before the rotation starts, so the first shift begins at the
            // first 09:00 at or after it.
            boundaries: (0..4).map(|turn| JAN_1 + 9 * HOUR + turn * DAY).collect(),
        },
        Cadence {
            case: "a daily hand-off every second day",
            zone: "UTC",
            rotation_start: JAN_1,
            recurrence: json!({
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 2,
                "DailySettings": [{"HourOfDay": 0, "MinuteOfHour": 0}],
            }),
            boundaries: (0..4).map(|turn| JAN_1 + turn * 2 * DAY).collect(),
        },
        Cadence {
            case: "a daily hand-off every hundredth day, the multiplier's modelled maximum",
            zone: "UTC",
            rotation_start: JAN_1,
            recurrence: json!({
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 100,
                "DailySettings": [{"HourOfDay": 0, "MinuteOfHour": 0}],
            }),
            boundaries: (0..3).map(|turn| JAN_1 + turn * 100 * DAY).collect(),
        },
        Cadence {
            case: "two hand-off times a day, which make two shifts of a day",
            zone: "UTC",
            rotation_start: JAN_1,
            recurrence: json!({
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
                "DailySettings": [
                    {"HourOfDay": 0, "MinuteOfHour": 0},
                    {"HourOfDay": 6, "MinuteOfHour": 30},
                ],
            }),
            boundaries: vec![
                JAN_1,
                JAN_1 + 6 * HOUR + 1_800,
                JAN_1 + DAY,
                JAN_1 + DAY + 6 * HOUR + 1_800,
                JAN_1 + 2 * DAY,
            ],
        },
        Cadence {
            case: "a weekly hand-off on the rotation's own weekday",
            zone: "UTC",
            rotation_start: JAN_1,
            recurrence: json!({
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
                "WeeklySettings": [{"DayOfWeek": "WED", "HandOffTime": {"HourOfDay": 0, "MinuteOfHour": 0}}],
            }),
            boundaries: (0..4).map(|turn| JAN_1 + turn * 7 * DAY).collect(),
        },
        Cadence {
            case: "a weekly hand-off on a weekday earlier in the rotation's own week",
            zone: "UTC",
            rotation_start: JAN_1,
            recurrence: json!({
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
                "WeeklySettings": [{"DayOfWeek": "MON", "HandOffTime": {"HourOfDay": 0, "MinuteOfHour": 0}}],
            }),
            // The Monday of the rotation's week is 2024-12-30, before the rotation starts, so
            // the first hand-off is the Monday after it: 2025-01-06.
            boundaries: (0..3).map(|turn| JAN_1 + 5 * DAY + turn * 7 * DAY).collect(),
        },
        Cadence {
            case: "a weekly hand-off on two days of the week",
            zone: "UTC",
            rotation_start: JAN_1,
            recurrence: json!({
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
                "WeeklySettings": [
                    {"DayOfWeek": "WED", "HandOffTime": {"HourOfDay": 0, "MinuteOfHour": 0}},
                    {"DayOfWeek": "SAT", "HandOffTime": {"HourOfDay": 0, "MinuteOfHour": 0}},
                ],
            }),
            // Wednesday 2025-01-01, Saturday 2025-01-04, then the same two days a week later.
            boundaries: vec![JAN_1, JAN_1 + 3 * DAY, JAN_1 + 7 * DAY, JAN_1 + 10 * DAY],
        },
        Cadence {
            case: "a weekly hand-off every second week",
            zone: "UTC",
            rotation_start: JAN_1,
            recurrence: json!({
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 2,
                "WeeklySettings": [{"DayOfWeek": "WED", "HandOffTime": {"HourOfDay": 0, "MinuteOfHour": 0}}],
            }),
            boundaries: (0..3).map(|turn| JAN_1 + turn * 14 * DAY).collect(),
        },
        Cadence {
            case: "a monthly hand-off on the first, whose shifts are as long as the months",
            zone: "UTC",
            rotation_start: JAN_1,
            recurrence: json!({
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
                "MonthlySettings": [{"DayOfMonth": 1, "HandOffTime": {"HourOfDay": 0, "MinuteOfHour": 0}}],
            }),
            boundaries: vec![JAN_1, FEB_1, MAR_1, APR_1],
        },
        Cadence {
            case: "a monthly hand-off every second month",
            zone: "UTC",
            rotation_start: JAN_1,
            recurrence: json!({
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 2,
                "MonthlySettings": [{"DayOfMonth": 1, "HandOffTime": {"HourOfDay": 0, "MinuteOfHour": 0}}],
            }),
            boundaries: vec![JAN_1, MAR_1, MAY_1, JUL_1],
        },
        Cadence {
            case: "a monthly hand-off on the 31st, the day's modelled maximum, which not every \
                   month has",
            zone: "UTC",
            rotation_start: JAN_1,
            recurrence: json!({
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
                "MonthlySettings": [{"DayOfMonth": 31, "HandOffTime": {"HourOfDay": 0, "MinuteOfHour": 0}}],
            }),
            // February, April and June have no 31st, so those months hand off not at all:
            // 2025-01-31, 2025-03-31, 2025-05-31.
            boundaries: vec![FEB_1 - DAY, APR_1 - DAY, JUL_1 - 31 * DAY],
        },
        Cadence {
            case: "a daily hand-off read in a zone nine hours ahead of UTC",
            zone: "Asia/Seoul",
            rotation_start: JAN_1,
            // 09:00 in Seoul is 00:00Z, so the hand-offs fall on the UTC midnights.
            recurrence: daily_at(9, 1),
            boundaries: (0..4).map(|turn| JAN_1 + turn * DAY).collect(),
        },
        Cadence {
            case: "a daily hand-off across the spring daylight-saving jump, which makes one \
                   shift an hour short",
            zone: "America/New_York",
            rotation_start: MAR_7_0900_NEW_YORK,
            recurrence: daily_at(9, 1),
            // 2025-03-09 is the second Sunday of March, so 09:00 local that day is 13:00Z
            // rather than 14:00Z and the shift before it lasts 23 hours.
            boundaries: vec![
                MAR_7_0900_NEW_YORK,
                MAR_7_0900_NEW_YORK + DAY,
                MAR_7_0900_NEW_YORK + DAY + 23 * HOUR,
                MAR_7_0900_NEW_YORK + 2 * DAY + 23 * HOUR,
            ],
        },
        Cadence {
            case: "a daily hand-off across the autumn daylight-saving jump, which makes one \
                   shift an hour long",
            zone: "America/New_York",
            rotation_start: OCT_31_0900_NEW_YORK,
            recurrence: daily_at(9, 1),
            // 2025-11-02 is the first Sunday of November, so 09:00 local that day is 14:00Z
            // rather than 13:00Z and the shift before it lasts 25 hours.
            boundaries: vec![
                OCT_31_0900_NEW_YORK,
                OCT_31_0900_NEW_YORK + DAY,
                OCT_31_0900_NEW_YORK + DAY + 25 * HOUR,
                OCT_31_0900_NEW_YORK + 2 * DAY + 25 * HOUR,
            ],
        },
    ]
}

#[tokio::test]
async fn the_recurrence_and_the_zone_decide_where_every_shift_begins_and_ends() {
    let emulator = Emulator::new();
    let members = team(&emulator).await;

    for row in cadence_rows() {
        let case = row.case;
        let last = *row
            .boundaries
            .last()
            .expect("every cadence row lists its hand-offs");
        let shifts = preview_to_exhaustion(
            &emulator,
            json!({
                "Members": [members[0]],
                "TimeZoneId": row.zone,
                "Recurrence": row.recurrence,
                "RotationStartTime": row.rotation_start,
                "EndTime": last,
            }),
            case,
        )
        .await;

        let spans: Vec<(i64, i64)> = shifts.iter().map(|shift| (shift.start, shift.end)).collect();
        let expected: Vec<(i64, i64)> = row
            .boundaries
            .windows(2)
            .map(|pair| (pair[0], pair[1]))
            .collect();
        assert_eq!(
            spans, expected,
            "case {case:?}: expected the shifts between consecutive hand-offs {:?}, got {spans:?}",
            row.boundaries
        );
        for shift in &shifts {
            assert_eq!(
                shift.kind.as_deref(),
                Some("REGULAR"),
                "case {case:?}: a shift no override touches is REGULAR, got {shift:?}"
            );
            assert_eq!(
                shift.contacts,
                vec![members[0].clone()],
                "case {case:?}: a one-contact team is on call for every shift, got {shift:?}"
            );
            assert!(
                shift.overridden.is_none(),
                "case {case:?}: a REGULAR shift carries no ShiftDetails, got {shift:?}"
            );
        }
    }
}

/// Nothing is reported before the rotation begins, and nothing from outside the requested
/// range: the range is what filters, so a `StartTime` inside the schedule drops the shifts
/// that end before it and keeps the one it falls inside, with that shift's OWN times.
#[tokio::test]
async fn the_requested_range_filters_the_calculated_shifts_without_clipping_them() {
    let emulator = Emulator::new();
    let members = team(&emulator).await;
    let schedule = json!({
        "Members": [members[0]],
        "TimeZoneId": "UTC",
        "Recurrence": daily_at(0, 1),
        "RotationStartTime": JAN_1,
    });

    // The whole first week, for reference: seven daily shifts.
    let week = {
        let mut request = schedule.clone();
        request["EndTime"] = json!(JAN_1 + 7 * DAY);
        preview_to_exhaustion(&emulator, request, "the whole first week").await
    };
    assert_eq!(
        week.len(),
        7,
        "the first week of a daily rotation holds seven shifts, got {week:?}"
    );

    for (case, start, end, expected) in [
        (
            "a range beginning in the middle of the third shift",
            Some(JAN_1 + 2 * DAY + 12 * HOUR),
            JAN_1 + 7 * DAY,
            week[2..].to_vec(),
        ),
        (
            "a range ending in the middle of the third shift",
            None,
            JAN_1 + 2 * DAY + 12 * HOUR,
            week[..3].to_vec(),
        ),
        (
            "a range that is exactly one shift",
            Some(JAN_1 + 3 * DAY),
            JAN_1 + 4 * DAY,
            week[3..4].to_vec(),
        ),
        (
            "a range entirely before the rotation begins",
            Some(JAN_1 - 30 * DAY),
            JAN_1,
            Vec::new(),
        ),
        (
            "a range beginning before the rotation begins",
            Some(JAN_1 - 30 * DAY),
            JAN_1 + 2 * DAY,
            week[..2].to_vec(),
        ),
    ] {
        let mut request = schedule.clone();
        request["EndTime"] = json!(end);
        if let Some(start) = start {
            request["StartTime"] = json!(start);
        }
        let shifts = preview_to_exhaustion(&emulator, request, case).await;
        assert_eq!(
            shifts, expected,
            "case {case:?}: expected the shifts covering an instant of the range, reported with \
             their own times, got {shifts:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: who is on call
// ---------------------------------------------------------------------------

/// The team positions on call during each of the first shifts, for a team of
/// `team_size` handing off `number_of_on_calls` at a time.
///
/// Written out rather than computed, so the table states the rotation order instead of
/// repeating the implementation's arithmetic.
fn on_call_rows() -> Vec<(&'static str, usize, i64, Vec<Vec<usize>>)> {
    vec![
        (
            "a team of one, who is always on call",
            1,
            1,
            vec![vec![0], vec![0], vec![0], vec![0]],
        ),
        (
            "a team of three handing off one at a time",
            3,
            1,
            vec![vec![0], vec![1], vec![2], vec![0], vec![1], vec![2]],
        ),
        (
            "a team of four handing off two at a time",
            4,
            2,
            vec![vec![0, 1], vec![2, 3], vec![0, 1], vec![2, 3]],
        ),
        (
            "a team of three handing off two at a time, which wraps mid-team",
            3,
            2,
            vec![vec![0, 1], vec![2, 0], vec![1, 2], vec![0, 1]],
        ),
        (
            "a team of four handing off three at a time",
            4,
            3,
            vec![vec![0, 1, 2], vec![3, 0, 1], vec![2, 3, 0], vec![1, 2, 3]],
        ),
        (
            "a team of four all of whom are on call at once",
            4,
            4,
            vec![vec![0, 1, 2, 3], vec![0, 1, 2, 3], vec![0, 1, 2, 3]],
        ),
    ]
}

#[tokio::test]
async fn the_team_is_walked_in_its_own_order_advancing_at_every_hand_off() {
    let emulator = Emulator::new();
    let members = team(&emulator).await;

    for (case, team_size, number_of_on_calls, expected_positions) in on_call_rows() {
        let members_of_row: Vec<String> = members[..team_size].to_vec();
        let shifts = preview_to_exhaustion(
            &emulator,
            json!({
                "Members": members_of_row,
                "TimeZoneId": "UTC",
                "Recurrence": daily_at(0, number_of_on_calls),
                "RotationStartTime": JAN_1,
                "EndTime": JAN_1 + expected_positions.len() as i64 * DAY,
            }),
            case,
        )
        .await;

        let on_call: Vec<Vec<String>> = shifts.iter().map(|shift| shift.contacts.clone()).collect();
        let expected: Vec<Vec<String>> = expected_positions
            .iter()
            .map(|positions| {
                positions
                    .iter()
                    .map(|position| members_of_row[*position].clone())
                    .collect()
            })
            .collect();
        assert_eq!(
            on_call, expected,
            "case {case:?}: expected the team walked in order {expected_positions:?}, got \
             {on_call:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the coverage of the week
// ---------------------------------------------------------------------------

/// A covered piece a coverage row expects: its start, its end, and the position in the
/// previewed team of the contact on call during it.
type CoveredPiece = (i64, i64, usize);

/// One coverage row: a case, a `ShiftCoverages` map, and the covered pieces it leaves of a
/// daily rotation's shifts over a fortnight.
type CoverageRow = (&'static str, Value, Vec<CoveredPiece>);

/// The coverage rows.
///
/// The rotation hands off at midnight UTC on 2025-01-01, a Wednesday, so shift `k` runs over
/// the `k`th day and position `k % 3` is on call during it.
fn coverage_rows() -> Vec<CoverageRow> {
    vec![
        (
            "one window inside the covered day",
            json!({"WED": [{"Start": {"HourOfDay": 9, "MinuteOfHour": 0}, "End": {"HourOfDay": 17, "MinuteOfHour": 0}}]}),
            vec![
                (JAN_1 + 9 * HOUR, JAN_1 + 17 * HOUR, 0),
                (JAN_1 + 7 * DAY + 9 * HOUR, JAN_1 + 7 * DAY + 17 * HOUR, 1),
            ],
        ),
        (
            "two windows in one covered day",
            json!({"WED": [
                {"Start": {"HourOfDay": 9, "MinuteOfHour": 0}, "End": {"HourOfDay": 12, "MinuteOfHour": 0}},
                {"Start": {"HourOfDay": 14, "MinuteOfHour": 0}, "End": {"HourOfDay": 17, "MinuteOfHour": 0}},
            ]}),
            vec![
                (JAN_1 + 9 * HOUR, JAN_1 + 12 * HOUR, 0),
                (JAN_1 + 14 * HOUR, JAN_1 + 17 * HOUR, 0),
                (JAN_1 + 7 * DAY + 9 * HOUR, JAN_1 + 7 * DAY + 12 * HOUR, 1),
                (JAN_1 + 7 * DAY + 14 * HOUR, JAN_1 + 7 * DAY + 17 * HOUR, 1),
            ],
        ),
        (
            "a window with no end, which covers to the end of the day",
            json!({"WED": [{"Start": {"HourOfDay": 20, "MinuteOfHour": 0}}]}),
            vec![
                (JAN_1 + 20 * HOUR, JAN_1 + DAY, 0),
                (JAN_1 + 7 * DAY + 20 * HOUR, JAN_1 + 8 * DAY, 1),
            ],
        ),
        (
            "a window with no start, which covers from the beginning of the day",
            json!({"WED": [{"End": {"HourOfDay": 4, "MinuteOfHour": 0}}]}),
            vec![
                (JAN_1, JAN_1 + 4 * HOUR, 0),
                (JAN_1 + 7 * DAY, JAN_1 + 7 * DAY + 4 * HOUR, 1),
            ],
        ),
        (
            "a window with neither end, which covers the whole day",
            json!({"WED": [{}]}),
            vec![
                (JAN_1, JAN_1 + DAY, 0),
                (JAN_1 + 7 * DAY, JAN_1 + 8 * DAY, 1),
            ],
        ),
        (
            "a window running past midnight, which covers the end of one shift and the \
             beginning of the next",
            json!({"WED": [{"Start": {"HourOfDay": 22, "MinuteOfHour": 0}, "End": {"HourOfDay": 2, "MinuteOfHour": 0}}]}),
            vec![
                (JAN_1 + 22 * HOUR, JAN_1 + DAY, 0),
                (JAN_1 + DAY, JAN_1 + DAY + 2 * HOUR, 1),
                (JAN_1 + 7 * DAY + 22 * HOUR, JAN_1 + 8 * DAY, 1),
                (JAN_1 + 8 * DAY, JAN_1 + 8 * DAY + 2 * HOUR, 2),
            ],
        ),
        (
            "two covered days of the week",
            json!({
                "WED": [{"Start": {"HourOfDay": 8, "MinuteOfHour": 0}, "End": {"HourOfDay": 10, "MinuteOfHour": 0}}],
                "FRI": [{"Start": {"HourOfDay": 8, "MinuteOfHour": 0}, "End": {"HourOfDay": 10, "MinuteOfHour": 0}}],
            }),
            vec![
                (JAN_1 + 8 * HOUR, JAN_1 + 10 * HOUR, 0),
                (JAN_1 + 2 * DAY + 8 * HOUR, JAN_1 + 2 * DAY + 10 * HOUR, 2),
                (JAN_1 + 7 * DAY + 8 * HOUR, JAN_1 + 7 * DAY + 10 * HOUR, 1),
                (JAN_1 + 9 * DAY + 8 * HOUR, JAN_1 + 9 * DAY + 10 * HOUR, 0),
            ],
        ),
        (
            "every day of the week covered for the same hours",
            json!({
                "MON": [{"Start": {"HourOfDay": 6, "MinuteOfHour": 0}, "End": {"HourOfDay": 7, "MinuteOfHour": 0}}],
                "TUE": [{"Start": {"HourOfDay": 6, "MinuteOfHour": 0}, "End": {"HourOfDay": 7, "MinuteOfHour": 0}}],
                "WED": [{"Start": {"HourOfDay": 6, "MinuteOfHour": 0}, "End": {"HourOfDay": 7, "MinuteOfHour": 0}}],
                "THU": [{"Start": {"HourOfDay": 6, "MinuteOfHour": 0}, "End": {"HourOfDay": 7, "MinuteOfHour": 0}}],
                "FRI": [{"Start": {"HourOfDay": 6, "MinuteOfHour": 0}, "End": {"HourOfDay": 7, "MinuteOfHour": 0}}],
                "SAT": [{"Start": {"HourOfDay": 6, "MinuteOfHour": 0}, "End": {"HourOfDay": 7, "MinuteOfHour": 0}}],
                "SUN": [{"Start": {"HourOfDay": 6, "MinuteOfHour": 0}, "End": {"HourOfDay": 7, "MinuteOfHour": 0}}],
            }),
            (0..14)
                .map(|day| {
                    (
                        JAN_1 + day * DAY + 6 * HOUR,
                        JAN_1 + day * DAY + 7 * HOUR,
                        (day % 3) as usize,
                    )
                })
                .collect(),
        ),
    ]
}

#[tokio::test]
async fn shift_coverages_narrow_a_shift_to_the_parts_of_the_week_the_rotation_covers() {
    let emulator = Emulator::new();
    let members = team(&emulator).await;
    let members_of_row: Vec<String> = members[..3].to_vec();

    for (case, coverages, expected_pieces) in coverage_rows() {
        let mut recurrence = daily_at(0, 1);
        recurrence["ShiftCoverages"] = coverages;
        let shifts = preview_to_exhaustion(
            &emulator,
            json!({
                "Members": members_of_row,
                "TimeZoneId": "UTC",
                "Recurrence": recurrence,
                "RotationStartTime": JAN_1,
                "EndTime": JAN_1 + 14 * DAY,
            }),
            case,
        )
        .await;

        let pieces: Vec<(i64, i64, Vec<String>)> = shifts
            .iter()
            .map(|shift| (shift.start, shift.end, shift.contacts.clone()))
            .collect();
        let expected: Vec<(i64, i64, Vec<String>)> = expected_pieces
            .iter()
            .map(|(start, end, position)| {
                (*start, *end, vec![members_of_row[*position].clone()])
            })
            .collect();
        assert_eq!(
            pieces, expected,
            "case {case:?}: expected the covered pieces of the shifts, each still served by \
             the contact whose shift it is, got {pieces:?}"
        );
    }
}

/// A day of the week `ShiftCoverages` does not name is not covered at all, so a range that
/// reaches only such days reports NO shift — a served empty page, not a shift the rotation
/// does not cover.
#[tokio::test]
async fn a_day_of_the_week_the_coverage_does_not_name_is_not_covered() {
    let emulator = Emulator::new();
    let members = team(&emulator).await;
    let mut recurrence = daily_at(0, 1);
    recurrence["ShiftCoverages"] = json!({
        "FRI": [{"Start": {"HourOfDay": 9, "MinuteOfHour": 0}, "End": {"HourOfDay": 17, "MinuteOfHour": 0}}]
    });

    for (case, end, expected) in [
        ("a range of the Wednesday and Thursday only", JAN_1 + 2 * DAY, 0),
        ("a range reaching the Friday", JAN_1 + 3 * DAY, 1),
        ("a range of a fortnight, which holds two Fridays", JAN_1 + 14 * DAY, 2),
    ] {
        let shifts = preview_to_exhaustion(
            &emulator,
            json!({
                "Members": [members[0]],
                "TimeZoneId": "UTC",
                "Recurrence": recurrence.clone(),
                "RotationStartTime": JAN_1,
                "EndTime": end,
            }),
            case,
        )
        .await;
        assert_eq!(
            shifts.len(),
            expected,
            "case {case:?}: expected {expected} covered shift(s), got {shifts:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the overrides
// ---------------------------------------------------------------------------

/// What a row expects of one reported shift: its span, whether it is overridden, and the team
/// positions on call — from the previewed team for a regular shift, from the override's own
/// members for an overridden one, which also reports the positions it displaced.
struct Piece {
    start: i64,
    end: i64,
    overridden: bool,
    on_call: Vec<usize>,
    displaced: Vec<usize>,
}

/// A regular piece of the previewed rotation's own schedule.
fn regular(start: i64, end: i64, on_call: usize) -> Piece {
    Piece {
        start,
        end,
        overridden: false,
        on_call: vec![on_call],
        displaced: Vec::new(),
    }
}

/// An overridden piece: `on_call` are the override's new members and `displaced` the contacts
/// of the shift it took over.
fn overridden(start: i64, end: i64, on_call: Vec<usize>, displaced: usize) -> Piece {
    Piece {
        start,
        end,
        overridden: true,
        on_call,
        displaced: vec![displaced],
    }
}

/// One override as a row requests it: the two optional ends of its window, and the positions
/// in the contact catalog its `NewMembers` are drawn from.
type RequestedOverride = (Option<i64>, Option<i64>, Vec<usize>);

/// One override row: a case, the `Overrides` list, and the pieces the four shifts of
/// 2025-01-01..05 are reported as.
type OverrideRow = (&'static str, Vec<RequestedOverride>, Vec<Piece>);

/// The override rows. The previewed team is positions 0..3 of the catalog and the new members
/// are drawn from the same catalog, so an expectation names positions throughout.
fn override_rows() -> Vec<OverrideRow> {
    vec![
        (
            "no override at all",
            Vec::new(),
            vec![
                regular(JAN_1, JAN_1 + DAY, 0),
                regular(JAN_1 + DAY, JAN_1 + 2 * DAY, 1),
                regular(JAN_1 + 2 * DAY, JAN_1 + 3 * DAY, 2),
                regular(JAN_1 + 3 * DAY, JAN_1 + 4 * DAY, 0),
            ],
        ),
        (
            "an override inside one shift, which splits it in three",
            vec![(
                Some(JAN_1 + DAY + 6 * HOUR),
                Some(JAN_1 + DAY + 18 * HOUR),
                vec![3],
            )],
            vec![
                regular(JAN_1, JAN_1 + DAY, 0),
                regular(JAN_1 + DAY, JAN_1 + DAY + 6 * HOUR, 1),
                overridden(
                    JAN_1 + DAY + 6 * HOUR,
                    JAN_1 + DAY + 18 * HOUR,
                    vec![3],
                    1,
                ),
                regular(JAN_1 + DAY + 18 * HOUR, JAN_1 + 2 * DAY, 1),
                regular(JAN_1 + 2 * DAY, JAN_1 + 3 * DAY, 2),
                regular(JAN_1 + 3 * DAY, JAN_1 + 4 * DAY, 0),
            ],
        ),
        (
            "an override exactly covering one shift, which splits nothing",
            vec![(Some(JAN_1 + DAY), Some(JAN_1 + 2 * DAY), vec![3])],
            vec![
                regular(JAN_1, JAN_1 + DAY, 0),
                overridden(JAN_1 + DAY, JAN_1 + 2 * DAY, vec![3], 1),
                regular(JAN_1 + 2 * DAY, JAN_1 + 3 * DAY, 2),
                regular(JAN_1 + 3 * DAY, JAN_1 + 4 * DAY, 0),
            ],
        ),
        (
            "an override spanning two shifts, which displaces each shift's own contact",
            vec![(
                Some(JAN_1 + DAY + 12 * HOUR),
                Some(JAN_1 + 2 * DAY + 12 * HOUR),
                vec![3],
            )],
            vec![
                regular(JAN_1, JAN_1 + DAY, 0),
                regular(JAN_1 + DAY, JAN_1 + DAY + 12 * HOUR, 1),
                overridden(JAN_1 + DAY + 12 * HOUR, JAN_1 + 2 * DAY, vec![3], 1),
                overridden(JAN_1 + 2 * DAY, JAN_1 + 2 * DAY + 12 * HOUR, vec![3], 2),
                regular(JAN_1 + 2 * DAY + 12 * HOUR, JAN_1 + 3 * DAY, 2),
                regular(JAN_1 + 3 * DAY, JAN_1 + 4 * DAY, 0),
            ],
        ),
        (
            "an override naming several new members",
            vec![(Some(JAN_1), Some(JAN_1 + DAY), vec![2, 3])],
            vec![
                overridden(JAN_1, JAN_1 + DAY, vec![2, 3], 0),
                regular(JAN_1 + DAY, JAN_1 + 2 * DAY, 1),
                regular(JAN_1 + 2 * DAY, JAN_1 + 3 * DAY, 2),
                regular(JAN_1 + 3 * DAY, JAN_1 + 4 * DAY, 0),
            ],
        ),
        (
            "an override naming no new members, which the list's modelled length admits",
            vec![(Some(JAN_1), Some(JAN_1 + DAY), Vec::new())],
            vec![
                Piece {
                    start: JAN_1,
                    end: JAN_1 + DAY,
                    overridden: true,
                    on_call: Vec::new(),
                    displaced: vec![0],
                },
                regular(JAN_1 + DAY, JAN_1 + 2 * DAY, 1),
                regular(JAN_1 + 2 * DAY, JAN_1 + 3 * DAY, 2),
                regular(JAN_1 + 3 * DAY, JAN_1 + 4 * DAY, 0),
            ],
        ),
        (
            "an override with no end, which runs to the end of the range",
            vec![(Some(JAN_1 + 2 * DAY), None, vec![3])],
            vec![
                regular(JAN_1, JAN_1 + DAY, 0),
                regular(JAN_1 + DAY, JAN_1 + 2 * DAY, 1),
                overridden(JAN_1 + 2 * DAY, JAN_1 + 3 * DAY, vec![3], 2),
                overridden(JAN_1 + 3 * DAY, JAN_1 + 4 * DAY, vec![3], 0),
            ],
        ),
        (
            "two overrides, on two different shifts",
            vec![
                (Some(JAN_1), Some(JAN_1 + DAY), vec![3]),
                (Some(JAN_1 + 2 * DAY), Some(JAN_1 + 3 * DAY), vec![1]),
            ],
            vec![
                overridden(JAN_1, JAN_1 + DAY, vec![3], 0),
                regular(JAN_1 + DAY, JAN_1 + 2 * DAY, 1),
                overridden(JAN_1 + 2 * DAY, JAN_1 + 3 * DAY, vec![1], 2),
                regular(JAN_1 + 3 * DAY, JAN_1 + 4 * DAY, 0),
            ],
        ),
        (
            "an override entirely outside the previewed range, which changes nothing",
            vec![(
                Some(JAN_1 + 30 * DAY),
                Some(JAN_1 + 31 * DAY),
                vec![3],
            )],
            vec![
                regular(JAN_1, JAN_1 + DAY, 0),
                regular(JAN_1 + DAY, JAN_1 + 2 * DAY, 1),
                regular(JAN_1 + 2 * DAY, JAN_1 + 3 * DAY, 2),
                regular(JAN_1 + 3 * DAY, JAN_1 + 4 * DAY, 0),
            ],
        ),
    ]
}

#[tokio::test]
async fn an_override_replaces_who_is_on_call_over_its_window_and_reports_who_it_displaced() {
    let emulator = Emulator::new();
    let members = team(&emulator).await;
    // Positions 0..2 are the previewed team; the overrides draw their new members from the
    // same catalog, so an expectation can name a position for either.
    let members_of_rotation: Vec<String> = members[..3].to_vec();

    for (case, overrides, expected_pieces) in override_rows() {
        let requested: Vec<Value> = overrides
            .iter()
            .map(|(start, end, new_members)| {
                let mut entry = json!({
                    "NewMembers": new_members
                        .iter()
                        .map(|position| members[*position].clone())
                        .collect::<Vec<String>>(),
                });
                if let Some(start) = start {
                    entry["StartTime"] = json!(start);
                }
                if let Some(end) = end {
                    entry["EndTime"] = json!(end);
                }
                entry
            })
            .collect();

        let shifts = preview_to_exhaustion(
            &emulator,
            json!({
                "Members": members_of_rotation,
                "TimeZoneId": "UTC",
                "Recurrence": daily_at(0, 1),
                "RotationStartTime": JAN_1,
                "EndTime": JAN_1 + 4 * DAY,
                "Overrides": requested,
            }),
            case,
        )
        .await;

        let expected: Vec<Reported> = expected_pieces
            .iter()
            .map(|piece| Reported {
                start: piece.start,
                end: piece.end,
                kind: Some(
                    if piece.overridden { "OVERRIDDEN" } else { "REGULAR" }.to_string(),
                ),
                contacts: piece
                    .on_call
                    .iter()
                    .map(|position| members[*position].clone())
                    .collect(),
                overridden: piece.overridden.then(|| {
                    piece
                        .displaced
                        .iter()
                        .map(|position| members_of_rotation[*position].clone())
                        .collect()
                }),
            })
            .collect();
        assert_eq!(
            shifts, expected,
            "case {case:?}: expected the overridden pieces to carry the override's members and \
             the contacts they displaced, and every other piece to stay regular, got {shifts:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: paging
// ---------------------------------------------------------------------------

/// Paging a range reports exactly what one unpaged call reports, whatever the page size: the
/// token resumes where the page ended, and the last page carries none.
#[tokio::test]
async fn paging_a_previewed_range_reports_the_same_shifts_as_one_unpaged_call() {
    let emulator = Emulator::new();
    let members = team(&emulator).await;
    let schedule = json!({
        "Members": members[..3].to_vec(),
        "TimeZoneId": "UTC",
        "Recurrence": daily_at(0, 1),
        "RotationStartTime": JAN_1,
        "EndTime": JAN_1 + 10 * DAY,
    });

    let unpaged = preview(&emulator, schedule.clone(), "no MaxResults at all").await;
    assert_eq!(
        unpaged.len(),
        10,
        "ten daily shifts fit in the range, and the default page carries them all, got \
         {unpaged:?}"
    );

    // `MaxResults`' modelled range is 0..1024: both ends, the sizes that divide the range and
    // the ones that do not.
    for max_results in [0, 1, 2, 3, 4, 5, 9, 10, 11, 1024] {
        let case = format!("MaxResults {max_results}");
        let mut request = schedule.clone();
        request["MaxResults"] = json!(max_results);

        let first = {
            let (status, body) = emulator
                .call("ListPreviewRotationShifts", request.clone())
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the first page to answer 200, got {status} {body:?}"
            );
            let page = reported(&body, &case);
            // 0 names no page size, so it serves the default rather than an empty page for
            // ever.
            let expected_len = if max_results == 0 {
                unpaged.len()
            } else {
                (max_results as usize).min(unpaged.len())
            };
            assert_eq!(
                page.len(),
                expected_len,
                "case {case:?}: expected a page of {expected_len} shift(s), got {page:?}"
            );
            assert_eq!(
                body["NextToken"].is_string(),
                page.len() < unpaged.len(),
                "case {case:?}: a token is issued exactly when shifts remain, got {body:?}"
            );
            page
        };
        assert_eq!(
            first,
            unpaged[..first.len()].to_vec(),
            "case {case:?}: the first page is the beginning of the unpaged answer"
        );

        let walked = preview_to_exhaustion(&emulator, request, &case).await;
        assert_eq!(
            walked, unpaged,
            "case {case:?}: paging to exhaustion reports the unpaged answer in the same order"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 6: the refusals
// ---------------------------------------------------------------------------

/// A request the preview cannot compute from is refused with the modelled
/// `ValidationException` (HTTP 400) — never a 501, and never a served page.
#[tokio::test]
async fn a_request_the_preview_cannot_compute_from_is_refused_on_the_member() {
    let emulator = Emulator::new();
    let members = team(&emulator).await;
    let well_formed = json!({
        "Members": members[..3].to_vec(),
        "TimeZoneId": "UTC",
        "Recurrence": daily_at(0, 1),
        "RotationStartTime": JAN_1,
        "EndTime": JAN_1 + 4 * DAY,
    });

    let refused: Vec<(&str, Value)> = vec![
        (
            "a recurrence carrying none of the settings lists, which says when no shift starts",
            json!({"NumberOfOnCalls": 1, "RecurrenceMultiplier": 1}),
        ),
        (
            "a recurrence whose weekly day of the week is not a modelled DayOfWeek",
            json!({
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
                "WeeklySettings": [{"DayOfWeek": "Caturday", "HandOffTime": {"HourOfDay": 0, "MinuteOfHour": 0}}],
            }),
        ),
        (
            "a recurrence whose shift coverage is keyed by something that is not a DayOfWeek",
            json!({
                "NumberOfOnCalls": 1,
                "RecurrenceMultiplier": 1,
                "DailySettings": [{"HourOfDay": 0, "MinuteOfHour": 0}],
                "ShiftCoverages": {"Caturday": [{}]},
            }),
        ),
    ];
    for (case, recurrence) in refused {
        let mut request = well_formed.clone();
        request["Recurrence"] = recurrence;
        assert_refused(&emulator, request, case).await;
    }

    // A zone name no zone database resolves, inside the member's modelled pattern.
    let mut unresolvable_zone = well_formed.clone();
    unresolvable_zone["TimeZoneId"] = json!("Mars/Olympus_Mons");
    assert_refused(&emulator, unresolvable_zone, "a time zone no zone database resolves").await;

    // More contacts on call at once than the previewed team has.
    let mut too_many_on_call = well_formed.clone();
    too_many_on_call["Recurrence"] = daily_at(0, 4);
    assert_refused(
        &emulator,
        too_many_on_call,
        "a NumberOfOnCalls the previewed team cannot meet",
    )
    .await;

    // A range, and an override window, whose end precedes its start.
    let mut inverted_range = well_formed.clone();
    inverted_range["StartTime"] = json!(JAN_1 + 4 * DAY);
    inverted_range["EndTime"] = json!(JAN_1);
    assert_refused(&emulator, inverted_range, "a range whose end precedes its start").await;

    let mut inverted_override = well_formed.clone();
    inverted_override["Overrides"] = json!([{
        "NewMembers": [members[3]],
        "StartTime": JAN_1 + 2 * DAY,
        "EndTime": JAN_1 + DAY,
    }]);
    assert_refused(
        &emulator,
        inverted_override,
        "an override window whose end precedes its start",
    )
    .await;

    // A `NextToken` this service did not issue, inside the member's modelled pattern.
    for token in ["not-a-token", "00", "726f746174696f6e2d6f7665727269646573"] {
        let mut forged_token = well_formed.clone();
        forged_token["NextToken"] = json!(token);
        assert_refused(
            &emulator,
            forged_token,
            &format!("a NextToken this service did not issue ({token})"),
        )
        .await;
    }
}

/// Assert `request` is refused with the modelled `ValidationException` and no page.
async fn assert_refused(emulator: &Emulator, request: Value, case: &str) {
    let (status, body) = emulator
        .call("ListPreviewRotationShifts", request.clone())
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for {request}, got {status} with {body:?}"
    );
    assert!(
        error_code(&body).contains("ValidationException"),
        "case {case:?}: expected a ValidationException — not a NotImplemented — got {:?} in \
         {body:?}",
        error_code(&body)
    );
    assert!(
        body.get("RotationShifts").is_none(),
        "case {case:?}: a refusal may not also carry a page, got {body:?}"
    );
}

/// A recurrence settings list longer than its period has hand-off times is refused on the
/// member rather than read: the model gives those lists no `length`, while a `HandOffTime` is
/// an hour-of-day and minute-of-hour pair (`smithy_query(query="shape",shape="HandOffTime")`),
/// so a day holds 1440 distinct hand-off times and a list longer than that names no further
/// one. `WeeklySettings` and `MonthlySettings` carry the same rule at their own periods.
#[tokio::test]
async fn a_recurrence_list_longer_than_its_period_has_hand_off_times_is_refused() {
    let emulator = Emulator::new();
    let members = team(&emulator).await;
    let minutes_in_a_day: usize = 24 * 60;

    let hand_off_times: Vec<Value> = (0..minutes_in_a_day)
        .map(|minute| json!({"HourOfDay": minute / 60, "MinuteOfHour": minute % 60}))
        .collect();
    let coverage_windows: Vec<Value> = (0..minutes_in_a_day)
        .map(|minute| {
            json!({
                "Start": {"HourOfDay": minute / 60, "MinuteOfHour": minute % 60},
                "End": {"HourOfDay": 23, "MinuteOfHour": 59},
            })
        })
        .collect();

    // At the ceiling: a list of every distinct hand-off time a day holds is read, so the
    // refusals below are about the length rather than about the shape.
    let at_the_ceiling = json!({
        "Members": [members[0]],
        "TimeZoneId": "UTC",
        "Recurrence": {
            "NumberOfOnCalls": 1,
            "RecurrenceMultiplier": 1,
            "DailySettings": hand_off_times,
        },
        "RotationStartTime": JAN_1,
        "EndTime": JAN_1 + DAY,
    });
    let served =
        preview_to_exhaustion(&emulator, at_the_ceiling.clone(), "a hand-off every minute of the day")
            .await;
    assert_eq!(
        served.len(),
        minutes_in_a_day,
        "a hand-off every minute of the day makes a shift of every minute of the day, got {} \
         shifts",
        served.len()
    );

    // One entry past it, on each list that multiplies the walk.
    let mut too_many_hand_offs = at_the_ceiling.clone();
    too_many_hand_offs["Recurrence"]["DailySettings"]
        .as_array_mut()
        .expect("the row carries a DailySettings list")
        .push(json!({"HourOfDay": 0, "MinuteOfHour": 0}));
    assert_refused(
        &emulator,
        too_many_hand_offs,
        "one more daily hand-off time than a day has minutes",
    )
    .await;

    let mut too_many_windows = at_the_ceiling;
    too_many_windows["Recurrence"]["DailySettings"] = json!([{"HourOfDay": 0, "MinuteOfHour": 0}]);
    let mut windows = coverage_windows;
    windows.push(json!({"Start": {"HourOfDay": 0, "MinuteOfHour": 0}}));
    too_many_windows["Recurrence"]["ShiftCoverages"] = json!({"WED": windows});
    assert_refused(
        &emulator,
        too_many_windows,
        "one more coverage window on a day than that day has minutes",
    )
    .await;
}

/// A resume point no page of this operation can ever have issued is refused as the token it
/// is, rather than served: the position a token names is a position in a computation that
/// reports at most `MAX_SHIFTS` shifts, so one at or past that ceiling — the shape a caller
/// would forge to make the emulator compute without end — names nothing.
#[tokio::test]
async fn a_resume_point_past_the_computation_ceiling_is_refused_as_an_invalid_token() {
    use service_com_amazonaws_ssmcontacts_ssmcontacts::state::rotation_shifts::MAX_SHIFTS;
    use service_com_amazonaws_ssmcontacts_ssmcontacts::state::{
        encode_next_token, TOKEN_SCOPE_PREVIEW_ROTATION_SHIFTS,
    };

    let emulator = Emulator::new();
    let members = team(&emulator).await;
    let well_formed = json!({
        "Members": [members[0]],
        "TimeZoneId": "UTC",
        "Recurrence": daily_at(0, 1),
        "RotationStartTime": JAN_1,
        "EndTime": JAN_1 + 4 * DAY,
    });

    // Minted with the crate's own token encoder, so each of these IS a token of this list's
    // scope and only its payload is wrong.
    let forged = [
        (
            "a resume point exactly on the computation's ceiling",
            format!("{MAX_SHIFTS}:{JAN_1}"),
        ),
        (
            "a resume point far past the ceiling, which is what makes a walk unbounded",
            format!("{}:{JAN_1}", u64::from(u32::MAX)),
        ),
        ("a payload carrying no anchor at all", "3".to_string()),
        (
            "a payload whose anchor is not an instant",
            "3:tomorrow".to_string(),
        ),
    ];
    for (case, payload) in forged {
        let mut request = well_formed.clone();
        request["NextToken"] = json!(encode_next_token(
            TOKEN_SCOPE_PREVIEW_ROTATION_SHIFTS,
            &payload
        ));
        assert_refused(&emulator, request, case).await;
    }

    // And the complement: the resume point one page BELOW the ceiling is a well-formed token,
    // so the refusals above are about the position rather than about the shape.
    let mut resumable = well_formed;
    resumable["NextToken"] = json!(encode_next_token(
        TOKEN_SCOPE_PREVIEW_ROTATION_SHIFTS,
        &format!("{}:{JAN_1}", MAX_SHIFTS - 1)
    ));
    let (status, body) = emulator
        .call("ListPreviewRotationShifts", resumable.clone())
        .await;
    assert_eq!(
        status, 200,
        "a resume point inside the ceiling is served (its page is empty, the range holding \
         fewer shifts than that), got {status} with {body:?}"
    );
    assert_eq!(
        body["RotationShifts"].as_array().map(Vec::len),
        Some(0),
        "the range holds four shifts, so resuming past them reports none, got {body:?}"
    );
}

/// A request that names no `RotationStartTime` is anchored when it is served, and its
/// `NextToken` carries that anchor: a later page is therefore a page of the SAME computation,
/// so the pages of one walk are contiguous and strictly increasing rather than two slices of
/// two differently anchored schedules.
#[tokio::test]
async fn a_paged_walk_of_a_rotation_with_no_start_time_stays_one_computation() {
    let emulator = Emulator::new();
    let members = team(&emulator).await;
    // Anchored on the clock, so the range has to be read from the shifts themselves rather
    // than from a fixed instant: a century is far past any anchor the clock can give.
    let request = json!({
        "Members": members[..3].to_vec(),
        "TimeZoneId": "UTC",
        "Recurrence": daily_at(0, 1),
        "EndTime": JAN_1 + 36_525 * DAY,
        "MaxResults": 2,
    });

    let (status, first) = emulator
        .call("ListPreviewRotationShifts", request.clone())
        .await;
    assert_eq!(status, 200, "the first page is served, got {status} {first:?}");
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a century of daily shifts does not fit one page: {first:?}"))
        .to_string();
    let mut resumed_request = request;
    resumed_request["NextToken"] = json!(token);
    let (status, second) = emulator
        .call("ListPreviewRotationShifts", resumed_request)
        .await;
    assert_eq!(status, 200, "the resumed page is served, got {status} {second:?}");

    let mut walked = reported(&first, "the first page");
    walked.extend(reported(&second, "the resumed page"));
    assert_eq!(
        walked.len(),
        4,
        "two pages of two shifts each report four shifts, got {walked:?}"
    );
    for pair in walked.windows(2) {
        let [earlier, later] = pair else { continue };
        assert_eq!(
            earlier.end, later.start,
            "a daily rotation's shifts meet at every hand-off, so the pages of one walk are \
             contiguous: {earlier:?} then {later:?}"
        );
        assert!(
            earlier.start < later.start,
            "every page continues the walk rather than repeating it: {earlier:?} then {later:?}"
        );
    }
    let contacts: Vec<Vec<String>> = walked.iter().map(|shift| shift.contacts.clone()).collect();
    assert_eq!(
        contacts,
        vec![
            vec![members[0].clone()],
            vec![members[1].clone()],
            vec![members[2].clone()],
            vec![members[0].clone()],
        ],
        "the team is walked across the page boundary too, so the resumed page continues the \
         rotation order rather than restarting it, got {contacts:?}"
    );
}

/// The preview is an on-call rotation operation, so the account-level Incident Manager
/// precondition decides it before anything is computed: with no ACTIVE replication set the
/// call is refused about the ACCOUNT — the request names no ARN — and the same bytes are
/// served once the account holds one.
#[tokio::test]
async fn the_closed_account_gate_refuses_the_preview_before_it_is_computed() {
    use crate::account_gate_arn_wording::ACCOUNT_NOT_FOUND;
    use crate::contact_updates::active_replication_set;
    use service_com_amazonaws_ssmcontacts_ssmcontacts::state::AccountReplicationSet;

    let emulator = Emulator::new();
    let members = team(&emulator).await;
    let request = json!({
        "Members": [members[0]],
        "TimeZoneId": "UTC",
        "Recurrence": daily_at(0, 1),
        "RotationStartTime": JAN_1,
        "EndTime": JAN_1 + 4 * DAY,
    });

    for (case, replication_set) in [
        ("no replication set at all", None),
        (
            "a replication set that has not finished replicating",
            Some(AccountReplicationSet {
                active: false,
                ..active_replication_set()
            }),
        ),
    ] {
        set_replication_set(&emulator, replication_set);
        let (status, body) = emulator
            .call("ListPreviewRotationShifts", request.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the account precondition to answer 400, got {status} \
             {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {:?} in {body:?}",
            error_code(&body)
        );
        assert_eq!(
            body["Message"].as_str().unwrap_or_default(),
            ACCOUNT_NOT_FOUND,
            "case {case:?}: the preview names no ARN, so the closed gate reports the account, \
             got {body:?}"
        );
        assert_eq!(
            body["Fields"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: an account-level refusal blames no member, got {body:?}"
        );
    }

    set_replication_set(&emulator, Some(active_replication_set()));
    let shifts = preview(&emulator, request, "the same bytes on an onboarded account").await;
    assert_eq!(
        shifts.len(),
        4,
        "the account's record is what decided above, so the same bytes are served now, got \
         {shifts:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 7: a preview writes nothing
// ---------------------------------------------------------------------------

/// What a later caller sees of the account, read over the wire.
async fn visible(emulator: &Emulator) -> (Value, Value) {
    let (status, contacts) = emulator.call("ListContacts", json!({})).await;
    assert_eq!(status, 200, "ListContacts must answer 200, got {status} {contacts:?}");
    let (status, rotations) = emulator.call("ListRotations", json!({})).await;
    assert_eq!(status, 200, "ListRotations must answer 200, got {status} {rotations:?}");
    (contacts, rotations)
}

/// A preview creates nothing: no rotation, no contact, and no change to any the store holds.
/// It is a calculation over the schedule the request describes, so the same request answers
/// identically however often it is sent.
#[tokio::test]
async fn previewing_a_rotation_creates_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let members = team(&emulator).await;
    let contact = members[0].clone();
    let rotation = emulator
        .create_rotation(&contact, "stored-rotation", "the rotation a preview must not touch")
        .await;
    let before = visible(&emulator).await;

    let request = json!({
        "Members": members[..3].to_vec(),
        "TimeZoneId": "Europe/Berlin",
        "Recurrence": daily_at(9, 2),
        "RotationStartTime": JAN_1,
        "EndTime": JAN_1 + 7 * DAY,
        "Overrides": [{
            "NewMembers": [members[3]],
            "StartTime": JAN_1 + 2 * DAY,
            "EndTime": JAN_1 + 3 * DAY,
        }],
    });
    let first = preview(&emulator, request.clone(), "the first preview").await;
    let again = preview(&emulator, request, "the same preview again").await;
    assert_eq!(
        first, again,
        "a preview is a calculation, so sending it twice answers identically, got {again:?}"
    );
    assert!(
        !first.is_empty(),
        "the previewed week holds shifts, so the comparison above is not between two empty \
         pages, got {first:?}"
    );

    let after = visible(&emulator).await;
    assert_eq!(
        after, before,
        "a preview must leave what a later caller sees untouched, got {after:?}"
    );

    let state = service_com_amazonaws_ssmcontacts_ssmcontacts::state::service_state(
        &*emulator.engine,
    )
    .expect("the engine holds this service's state");
    let state = state.lock();
    assert_eq!(
        state.rotations.keys().cloned().collect::<Vec<String>>(),
        vec![rotation],
        "the store holds exactly the rotation this suite created, got {:?}",
        state.rotations.keys()
    );
}
