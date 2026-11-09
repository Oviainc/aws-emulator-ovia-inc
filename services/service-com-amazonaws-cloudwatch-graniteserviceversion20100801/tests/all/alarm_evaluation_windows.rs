// SPDX-License-Identifier: CC0-1.0

//! WHICH range of metric data a metric alarm's evaluation selects, end to end over the wire:
//! the two evaluation windows an alarm chooses between.
//!
//! ONE rule, asserted over a table: **an alarm whose `EvaluationWindow` is a `WallClockWindow`
//! is evaluated over periods that begin at the clock boundaries its own `Period` repeats at in
//! the window's time zone, and only over periods that have CLOSED — so it reports no verdict
//! over the period the clock is in until that period's end has passed, and the instants its
//! decision record names are those boundaries** — while an alarm that names a `SlidingWindow`,
//! or no window at all, keeps the rolling range that advances each time it is evaluated.
//!
//! "The evaluation window that an alarm uses to select the range of metric data that it
//! evaluates each time it runs. This is a union type. Set exactly one of its members,
//! `SlidingWindow` or `WallClockWindow`. If you don't set `EvaluationWindow`, the alarm uses a
//! `SlidingWindow` by default." (`smithy_query(query="shape",shape="EvaluationWindow")`), where
//! a wall clock window "aligns the evaluated range to fixed clock boundaries that match the
//! alarm's period, such as the top of the hour, midnight, or the start of the calendar week,
//! optionally in a specific time zone" and a sliding one "advances each time the alarm is
//! evaluated, forming a rolling time window".
//!
//! The zone those boundaries are the clock's own in is the window's: "You can specify an IANA
//! time zone name (for example, `America/New_York`), a fixed UTC offset (for example,
//! `+05:30`), or an offset-prefixed identifier (for example, `UTC+05:30`) […] If you don't
//! specify a time zone, CloudWatch uses `UTC`. The time zone affects window alignment for all
//! periods, including periods of one hour or shorter."
//! (`smithy_query(query="shape",shape="WallClockWindow")`).
//!
//! The rows of the first table differ along what the rule generalises over: the width of the
//! period (every one an aligned window may carry — "the alarm's period must be 1 minute (60
//! seconds), 5 minutes (300 seconds), 1 hour (3,600 seconds), 1 day (86,400 seconds), or 1 week
//! (604,800 seconds)", the same catalog
//! `smithy_query(query="shape",shape="WallClockWindow")` publishes), the form the zone is
//! stated in (absent, an IANA name, a fixed offset ahead of UTC, a fixed offset behind it, and
//! an offset-prefixed identifier), whether that zone's offset shifts the boundaries off the ones
//! UTC would put them at, and how many periods the alarm evaluates. The second test is the
//! other half of the rule, which only the clock can show: a datapoint published into the period
//! the clock is IN decides an aligned alarm's state only once that period has closed, while the
//! sliding twin over the very same series has already decided on it. The third is the zone half:
//! a window naming a zone in one of the documented forms is created and aligned on that zone's
//! clock, and one naming a value in none of them — including an offset the documented
//! five-minute step rules out — is refused with HTTP 400 `ValidationError` and stores nothing,
//! rather than being evaluated on boundaries nothing resolved.

use std::time::Duration;

use ::chrono::{DateTime, Datelike, NaiveDateTime, Offset, TimeDelta, TimeZone, Timelike, Weekday};
use ::chrono_tz::Tz;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    alarm_stamp, build_router, call, document_instant_secs, error_code, error_message, now_secs,
    ok, same_json_value, wait_for_evaluation, wait_for_period_room, PAST_EVALUATION_DELAY,
};

/// The customer namespace every alarm of this suite watches.
const NAMESPACE: &str = "Custom/EvaluationWindows";

/// The metric they watch.
const METRIC: &str = "Load";

/// The value every row publishes, which crosses [`THRESHOLD`].
const BREACHING_VALUE: f64 = 100.0;

/// The threshold every row's alarm is compared against.
const THRESHOLD: f64 = 10.0;

/// The shortest period an aligned window may carry, one minute.
const MINUTE_PERIOD: i32 = 60;

/// How far from the next minute boundary a row's publish and its reading must both sit.
///
/// Every period an aligned window may carry is a whole number of minutes, and every offset a
/// zone may be stated in "must be aligned to a multiple of 5 minutes"
/// (`smithy_query(query="shape",shape="WallClockWindow")`), so every boundary either grid can
/// put a window at is a multiple of a minute: a publish and a read this far from the next minute
/// fall in the same window of every grid below.
const MINUTE_ROOM_SECS: f64 = 10.0;

/// How far apart two instants may read and still be the same instant: a document holds an
/// instant to the MILLISECOND ("2026-09-22T23:06:00.000+0000"), so one it writes for a boundary
/// is that boundary rounded to the millisecond.
const DOCUMENT_INSTANT_TOLERANCE_SECS: f64 = 0.001;

/// Where a row's time zone offset comes from.
#[derive(Clone, Copy, Debug)]
enum Shift {
    /// The offset the zone text itself states, in seconds ahead of UTC.
    Stated(f64),
    /// The offset the IANA database gives for a named zone at the instant the row runs, which
    /// the name does not state and a zone observing daylight saving does not keep all year.
    Database(Tz),
}

impl Shift {
    /// How far ahead of UTC the row's zone runs at `at` (epoch seconds).
    fn secs(self, at: f64) -> f64 {
        match self {
            Self::Stated(secs) => secs,
            Self::Database(zone) => {
                #[allow(clippy::cast_possible_truncation)]
                let at = DateTime::from_timestamp(at.floor() as i64, 0)
                    .expect("the test clock reads an instant");
                let offset = zone.offset_from_utc_datetime(&at.naive_utc());
                f64::from(offset.fix().local_minus_utc())
            }
        }
    }
}

/// One row of the table.
#[derive(Clone, Copy, Debug)]
struct Row {
    /// What this row is about, named by every assertion it makes.
    case: &'static str,
    /// The alarm's `Period`, in seconds.
    period: i32,
    /// The `Timezone` the row's wall clock window carries, where it carries one.
    timezone: Option<&'static str>,
    /// How far ahead of UTC that zone runs.
    shift: Shift,
    /// The alarm's `EvaluationPeriods`.
    evaluation_periods: i32,
    /// Which CLOSED aligned periods hold a breaching datapoint, counting back from the newest
    /// one that has closed.
    breaching_periods_back: &'static [u32],
}

/// The table: one row per shape the rule generalises over.
const ROWS: &[Row] = &[
    Row {
        // The recorded shape: five-minute periods aligned on UTC boundaries.
        case: "five-minute periods in UTC",
        period: 300,
        timezone: Some("UTC"),
        shift: Shift::Stated(0.0),
        evaluation_periods: 1,
        breaching_periods_back: &[0],
    },
    Row {
        // The shortest period an aligned window may carry, with the zone member left out: "If
        // you don't specify a time zone, CloudWatch uses UTC."
        case: "one-minute periods with no zone named",
        period: MINUTE_PERIOD,
        timezone: None,
        shift: Shift::Stated(0.0),
        evaluation_periods: 1,
        breaching_periods_back: &[0],
    },
    Row {
        // An IANA name whose zone runs half an hour off the hour, so the boundaries of an hour
        // period are NOT the ones UTC would put them at.
        case: "hour periods in an IANA zone half an hour off UTC",
        period: 3_600,
        timezone: Some("Asia/Kolkata"),
        shift: Shift::Database(Tz::Asia__Kolkata),
        evaluation_periods: 1,
        breaching_periods_back: &[0],
    },
    Row {
        // The same boundaries, stated as a fixed offset instead of a name.
        case: "hour periods in a fixed offset ahead of UTC",
        period: 3_600,
        timezone: Some("+05:30"),
        shift: Shift::Stated(19_800.0),
        evaluation_periods: 1,
        breaching_periods_back: &[0],
    },
    Row {
        // And as the offset-prefixed identifier of the same offset.
        case: "hour periods in an offset-prefixed identifier",
        period: 3_600,
        timezone: Some("UTC+05:30"),
        shift: Shift::Stated(19_800.0),
        evaluation_periods: 1,
        breaching_periods_back: &[0],
    },
    Row {
        // A zone BEHIND UTC, by a half hour too.
        case: "hour periods in a fixed offset behind UTC",
        period: 3_600,
        timezone: Some("-03:30"),
        shift: Shift::Stated(-12_600.0),
        evaluation_periods: 1,
        breaching_periods_back: &[0],
    },
    Row {
        // Midnight boundaries, over TWO closed periods rather than one: the oldest period the
        // verdict rests on is not the newest, so the span the record opens at is a different
        // boundary from the newest datapoint it cites.
        case: "day periods over two closed days",
        period: 86_400,
        timezone: Some("UTC"),
        shift: Shift::Stated(0.0),
        evaluation_periods: 2,
        breaching_periods_back: &[0, 1],
    },
    Row {
        // A named zone whose offset the IANA database decides, and whose day therefore starts at
        // a different instant from the UTC one.
        case: "day periods in a named zone behind UTC",
        period: 86_400,
        timezone: Some("America/New_York"),
        shift: Shift::Database(Tz::America__New_York),
        evaluation_periods: 1,
        breaching_periods_back: &[0],
    },
    Row {
        // The widest period an aligned window may carry.
        case: "week periods in UTC",
        period: 604_800,
        timezone: Some("UTC"),
        shift: Shift::Stated(0.0),
        evaluation_periods: 1,
        breaching_periods_back: &[0],
    },
];

/// The instant the `period`-wide window the clock is IN begins at, on the grid a zone `shift`
/// seconds ahead of UTC puts its boundaries on: the boundary the clock has last passed.
///
/// Derived from the CALENDAR fields of that zone's wall clock rather than from the arithmetic the
/// evaluator uses, so a row's expectation is not a second copy of the implementation: the top of
/// the minute, the five-minute mark, the top of the hour, midnight, and the Sunday a calendar
/// week begins at are what "fixed clock boundaries that match the alarm's period, such as the top
/// of the hour, midnight, or the start of the calendar week"
/// (`smithy_query(query="shape",shape="WallClockWindow")`) names.
fn open_window_start(at: f64, period: i32, shift: f64) -> f64 {
    let local = local_clock(at + shift);
    let truncated = local
        .with_second(0)
        .and_then(|local| local.with_nanosecond(0))
        .expect("the start of a minute is an instant");
    let start = match period {
        60 => truncated,
        300 => truncated
            .with_minute(truncated.minute() - truncated.minute() % 5)
            .expect("a five-minute mark is an instant"),
        3_600 => truncated
            .with_minute(0)
            .expect("the top of an hour is an instant"),
        86_400 => midnight_of(truncated),
        604_800 => midnight_of(
            truncated
                - TimeDelta::days(i64::from(truncated.weekday().num_days_from_sunday())),
        ),
        other => panic!("an aligned window carries no period of {other} seconds"),
    };
    epoch_secs(start) - shift
}

/// The local wall clock of a zone, as the naive calendar fields `secs` reads as there.
fn local_clock(secs: f64) -> NaiveDateTime {
    #[allow(clippy::cast_possible_truncation)]
    let secs = secs.floor() as i64;
    DateTime::from_timestamp(secs, 0)
        .expect("the test clock reads an instant")
        .naive_utc()
}

/// Midnight of the calendar day `local` falls on.
fn midnight_of(local: NaiveDateTime) -> NaiveDateTime {
    local
        .date()
        .and_hms_opt(0, 0, 0)
        .expect("midnight is an instant of every day")
}

/// Those calendar fields back as the epoch seconds a row measures its boundaries in.
#[allow(clippy::cast_precision_loss)]
fn epoch_secs(local: NaiveDateTime) -> f64 {
    local.and_utc().timestamp() as f64
}

/// Assert that `instant` is a boundary a `period`-wide window may begin at on the wall clock of a
/// zone `shift` seconds ahead of UTC: the calendar fields such a boundary carries there.
fn assert_clock_boundary(case: &str, instant: f64, period: i32, shift: f64) {
    let local = local_clock(instant + shift);
    assert_eq!(
        (local.second(), local.nanosecond()),
        (0, 0),
        "case {case:?}: a clock boundary begins on a whole minute, got {local} in the zone \
         {shift} seconds ahead of UTC"
    );
    match period {
        60 => {}
        300 => assert_eq!(
            local.minute() % 5,
            0,
            "case {case:?}: a five-minute window begins at a five-minute mark, got {local}"
        ),
        3_600 => assert_eq!(
            local.minute(),
            0,
            "case {case:?}: an hour window begins at the top of the hour, got {local}"
        ),
        86_400 => assert_eq!(
            (local.hour(), local.minute()),
            (0, 0),
            "case {case:?}: a day window begins at midnight, got {local}"
        ),
        604_800 => assert_eq!(
            (local.weekday(), local.hour(), local.minute()),
            (Weekday::Sun, 0, 0),
            "case {case:?}: a week window begins where a calendar week does, at midnight on \
             Sunday, got {local}"
        ),
        other => panic!("case {case:?}: an aligned window carries no period of {other} seconds"),
    }
}

/// The wall clock window a row names, in the shape the request carries it.
fn wall_clock_window(row: &Row) -> Value {
    match row.timezone {
        Some(timezone) => json!({"WallClockWindow": {"Timezone": timezone}}),
        None => json!({"WallClockWindow": {}}),
    }
}

/// The alarm request one row configures, watching a series of its own so every row of the table
/// shares one emulator.
fn alarm_of(row: &Row, name: &str, window: &Value) -> Value {
    json!({
        "AlarmName": name,
        "Namespace": NAMESPACE,
        "MetricName": METRIC,
        "Dimensions": [{"Name": "Case", "Value": name}],
        "Statistic": "Maximum",
        "Period": row.period,
        "EvaluationPeriods": row.evaluation_periods,
        "DatapointsToAlarm": row.breaching_periods_back.len(),
        "Threshold": THRESHOLD,
        "ComparisonOperator": "GreaterThanThreshold",
        "TreatMissingData": "missing",
        "EvaluationWindow": window,
        "ActionsEnabled": false,
    })
}

/// Publish one breaching datapoint into the MIDDLE of each period `boundaries` names.
async fn publish_breaching(
    router: &MultiServiceRouter,
    name: &str,
    period: f64,
    boundaries: &[f64],
) {
    let data: Vec<Value> = boundaries
        .iter()
        .map(|start| {
            json!({
                "MetricName": METRIC,
                "Dimensions": [{"Name": "Case", "Value": name}],
                "Value": BREACHING_VALUE,
                "Timestamp": start + period / 2.0,
            })
        })
        .collect();
    ok(
        router,
        "PutMetricData",
        &json!({"Namespace": NAMESPACE, "MetricData": data}),
    )
    .await;
}

/// The `StateReasonData` of a described alarm, parsed: it is "an explanation for the alarm
/// state, in JSON format", so a consumer reads it as a document.
fn reason_data(case: &str, alarm: &Value) -> Value {
    let text = alarm
        .get("StateReasonData")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected StateReasonData, got {alarm}"));
    serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("case {case:?}: StateReasonData is not JSON: {error}"))
}

/// One instant a decision record carries, as epoch seconds.
fn instant_of(case: &str, record: &Value, member: &str) -> f64 {
    record
        .get(member)
        .and_then(Value::as_str)
        .and_then(document_instant_secs)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} instant, got {record}"))
}

/// The instants the record reports for the datapoints the verdict rests on, oldest first.
fn graded_instants(record: &Value) -> Vec<f64> {
    let mut instants: Vec<f64> = record
        .get("evaluatedDatapoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| {
            entry
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(document_instant_secs)
        })
        .collect();
    instants.sort_by(f64::total_cmp);
    instants
}

/// Read one alarm back, by the name it was created under.
async fn described(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let answered = ok(router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
    answered
        .pointer("/MetricAlarms/0")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected the alarm {name:?}, got {answered}"))
}

/// The state value a described alarm reports.
fn state_of(alarm: &Value) -> Option<&str> {
    alarm.get("StateValue").and_then(Value::as_str)
}

#[tokio::test]
async fn an_aligned_alarm_grades_the_closed_periods_of_its_own_clock_boundaries() {
    let router = build_router();
    // Every boundary either grid lays a window at is a multiple of a minute, so a publish and a
    // reading this far from the next minute fall in the same windows.
    wait_for_period_room(f64::from(MINUTE_PERIOD), MINUTE_ROOM_SECS).await;
    let at = now_secs();

    // Each row's aligned alarm, beside a SLIDING twin over the SAME series and configuration:
    // the control that keeps the rolling range the default window has always had.
    let mut configured: Vec<(Row, String, String, Vec<f64>)> = Vec::with_capacity(ROWS.len());
    for (index, row) in ROWS.iter().enumerate() {
        let period = f64::from(row.period);
        let open_start = open_window_start(at, row.period, row.shift.secs(at));
        // The CLOSED periods the row names, newest first: the window before the one the clock is
        // in, and the windows before that.
        let boundaries: Vec<f64> = row
            .breaching_periods_back
            .iter()
            .map(|back| open_start - period * f64::from(*back + 1))
            .collect();

        let name = format!("aligned-{index}");
        ok(
            &router,
            "PutMetricAlarm",
            &alarm_of(row, &name, &wall_clock_window(row)),
        )
        .await;
        publish_breaching(&router, &name, period, &boundaries).await;

        let twin = format!("sliding-{index}");
        ok(
            &router,
            "PutMetricAlarm",
            &alarm_of(row, &twin, &json!({"SlidingWindow": {}})),
        )
        .await;
        // The twin watches a series of its own, published exactly like the aligned alarm's, so
        // the two differ only in the window they select their range with.
        publish_breaching(&router, &twin, period, &boundaries).await;
        configured.push((*row, name, twin, boundaries));
    }
    wait_for_evaluation();

    for (row, name, twin, boundaries) in &configured {
        let case = row.case;
        let period = f64::from(row.period);
        let shift = row.shift.secs(at);
        let reported = described(&router, case, name).await;
        assert_eq!(
            state_of(&reported),
            Some("ALARM"),
            "case {case:?}: a breaching datapoint of a CLOSED aligned period decides the alarm, \
             got {reported}"
        );
        // The window the alarm was created with reads back as it was asked for, and an alarm
        // selecting its range with one reports no evaluation state of its own.
        let asked = wall_clock_window(row);
        let read_back = reported
            .get("EvaluationWindow")
            .cloned()
            .unwrap_or(Value::Null);
        assert!(
            same_json_value(&asked, &read_back),
            "case {case:?}: expected EvaluationWindow {asked}, got {read_back}"
        );
        assert!(
            reported.get("EvaluationState").is_none(),
            "case {case:?}: an alarm selecting its range with an evaluation window reports no \
             EvaluationState, got {reported}"
        );

        let record = reason_data(case, &reported);
        let graded = graded_instants(&record);
        let mut expected = boundaries.clone();
        expected.sort_by(f64::total_cmp);
        assert_eq!(
            graded, expected,
            "case {case:?}: the datapoints the verdict rests on are the ones aggregated out of \
             the aligned periods ({expected:?}), got {graded:?} in {record}"
        );
        for instant in &graded {
            assert_clock_boundary(case, *instant, row.period, shift);
        }
        let start_date = instant_of(case, &record, "startDate");
        let oldest = expected
            .first()
            .copied()
            .unwrap_or_else(|| panic!("case {case:?}: the row publishes at least one datapoint"));
        assert!(
            (start_date - oldest).abs() < DOCUMENT_INSTANT_TOLERANCE_SECS,
            "case {case:?}: the record opens its span at the boundary of the oldest aligned \
             period it graded ({oldest}), got {start_date} in {record}"
        );
        // The verdict was reached only after every period it names had CLOSED: both the instant
        // the evaluation queried the series at and the instant it wrote the state are at or
        // after the end of the newest period it graded.
        let newest_close = graded
            .iter()
            .copied()
            .reduce(f64::max)
            .map_or(0.0, |newest| newest + period);
        let query_date = instant_of(case, &record, "queryDate");
        assert!(
            query_date >= newest_close,
            "case {case:?}: an aligned window grades no period before it closes, so the query \
             instant ({query_date}) must be at or after {newest_close}, got {record}"
        );
        let updated = alarm_stamp(&reported, "StateUpdatedTimestamp").unwrap_or_else(|| {
            panic!("case {case:?}: expected a StateUpdatedTimestamp, got {reported}")
        });
        assert!(
            updated >= newest_close,
            "case {case:?}: the state was written after the period it was decided over closed, \
             so {updated} must be at or after {newest_close}, got {reported}"
        );

        // The sliding twin reaches the same verdict on data that is already in the past — the
        // default window is unchanged — and its range is the rolling one, so a zone that shifts
        // the aligned boundaries off the ones a rolling range falls on makes the two records
        // name DIFFERENT instants.
        let twin_reported = described(&router, case, twin).await;
        assert_eq!(
            state_of(&twin_reported),
            Some("ALARM"),
            "case {case:?}: the sliding twin grades the same past datapoint, got {twin_reported}"
        );
        let twin_record = reason_data(case, &twin_reported);
        let twin_graded = graded_instants(&twin_record);
        if shift.rem_euclid(period) != 0.0 {
            assert!(
                twin_graded != graded,
                "case {case:?}: a zone that shifts the aligned boundaries off the rolling grid \
                 must make the two windows grade different instants, got {graded:?} for the \
                 aligned alarm and {twin_graded:?} for the twin ({twin_record})"
            );
        }
    }
}

/// How far into the minute the second test publishes at the earliest, and at the latest.
///
/// The sliding twin's range ends one evaluation cadence behind the clock, so the period the
/// clock is in is on its rolling range once that period has been open that long: the publish
/// waits until then, which is what makes the twin decide on the datapoint while the aligned
/// window over the same series is still open. The upper bound leaves enough of the minute for
/// the reading that observes the difference.
const PHASE_FROM_SECS: f64 = 15.0;

/// The late end of that phase.
const PHASE_UNTIL_SECS: f64 = 40.0;

/// Wait until the clock is between [`PHASE_FROM_SECS`] and [`PHASE_UNTIL_SECS`] into the minute.
async fn wait_for_minute_phase() {
    let minute = f64::from(MINUTE_PERIOD);
    while !(PHASE_FROM_SECS..=PHASE_UNTIL_SECS).contains(&now_secs().rem_euclid(minute)) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The row the second test configures its two alarms from: one breaching datapoint of its own
/// period is enough to decide it, and a period with no datapoint at all leaves it undecided.
const OPEN_PERIOD_ROW: Row = Row {
    case: "a datapoint published into the period the clock is in",
    period: MINUTE_PERIOD,
    timezone: Some("UTC"),
    shift: Shift::Stated(0.0),
    evaluation_periods: 1,
    breaching_periods_back: &[0],
};

#[tokio::test]
async fn an_aligned_alarm_holds_its_state_until_the_period_the_clock_is_in_closes() {
    let router = build_router();
    let period = f64::from(MINUTE_PERIOD);
    let aligned = "aligned-open-period";
    let twin = "sliding-open-period";
    for (name, window) in [
        (aligned, wall_clock_window(&OPEN_PERIOD_ROW)),
        (twin, json!({"SlidingWindow": {}})),
    ] {
        ok(
            &router,
            "PutMetricAlarm",
            &alarm_of(&OPEN_PERIOD_ROW, name, &window),
        )
        .await;
    }

    // A breaching datapoint stamped inside the period the clock is IN, published far enough into
    // that period for the sliding twin's rolling range to have reached it and far enough from
    // its end for the reading below to happen while the aligned window is still open.
    wait_for_minute_phase().await;
    let published_at = now_secs();
    let open_start = open_window_start(published_at, MINUTE_PERIOD, 0.0);
    let closes_at = open_start + period;
    for name in [aligned, twin] {
        ok(
            &router,
            "PutMetricData",
            &json!({
                "Namespace": NAMESPACE,
                "MetricData": [{
                    "MetricName": METRIC,
                    "Dimensions": [{"Name": "Case", "Value": name}],
                    "Value": BREACHING_VALUE,
                    "Timestamp": published_at,
                }],
            }),
        )
        .await;
    }
    wait_for_evaluation();

    // The sliding twin has decided on the datapoint: its range is the rolling one, which has
    // already reached the period the clock is in.
    let case = "the sliding twin, while the aligned period is open";
    let twin_reported = described(&router, case, twin).await;
    assert_eq!(
        state_of(&twin_reported),
        Some("ALARM"),
        "case {case:?}: a rolling range reaches the period the clock is in, got {twin_reported}"
    );
    // And it decided BEFORE the aligned period closed, which is the difference the two windows
    // are for.
    let twin_record = reason_data(case, &twin_reported);
    let twin_query = instant_of(case, &twin_record, "queryDate");
    assert!(
        twin_query < closes_at,
        "case {case:?}: the sliding twin must decide while the aligned period ({open_start} .. \
         {closes_at}) is still open, got a query instant of {twin_query} in {twin_record}"
    );

    // The aligned alarm has not: the period its range would start at has not closed, so it is
    // still the state its creation left it in.
    let case = "the aligned alarm, while its period is open";
    let reported = described(&router, case, aligned).await;
    assert_eq!(
        state_of(&reported),
        Some("INSUFFICIENT_DATA"),
        "case {case:?}: an aligned window reports no verdict over the period the clock is in \
         ({open_start} .. {closes_at}), got {reported}"
    );

    // Once that period has closed, the same datapoint decides the alarm — and the record names
    // the boundary the period began at, not the instant the datapoint was published at.
    std::thread::sleep(
        Duration::from_secs_f64((closes_at - now_secs()).max(0.0)) + PAST_EVALUATION_DELAY,
    );
    let case = "the aligned alarm, once its period has closed";
    let reported = described(&router, case, aligned).await;
    assert_eq!(
        state_of(&reported),
        Some("ALARM"),
        "case {case:?}: the datapoint is graded by the evaluation that follows the close of its \
         aligned period, got {reported}"
    );
    let record = reason_data(case, &reported);
    assert_eq!(
        graded_instants(&record),
        vec![open_start],
        "case {case:?}: the verdict rests on the datapoint of the aligned period that closed \
         ({open_start}), got {record}"
    );
    let start_date = instant_of(case, &record, "startDate");
    assert!(
        (start_date - open_start).abs() < DOCUMENT_INSTANT_TOLERANCE_SECS,
        "case {case:?}: the record opens its span at the clock boundary the aligned period began \
         at ({open_start}), got {start_date} in {record}"
    );
    let query_date = instant_of(case, &record, "queryDate");
    assert!(
        query_date >= closes_at,
        "case {case:?}: the verdict was reached only after the period closed, so the query \
         instant ({query_date}) must be at or after {closes_at}, got {record}"
    );
    let updated = alarm_stamp(&reported, "StateUpdatedTimestamp").unwrap_or_else(|| {
        panic!("case {case:?}: expected a StateUpdatedTimestamp, got {reported}")
    });
    assert!(
        updated >= closes_at,
        "case {case:?}: the state was written after the aligned period closed, so {updated} must \
         be at or after {closes_at}, got {reported}"
    );
}

/// The error class CloudWatch refuses a configuration it would not create with, as the body of
/// the answer names it: the request-validation class, whose legacy query code
/// (`ValidationError`) travels in the `x-amzn-query-error` header instead.
const REFUSAL_CODE: &str = "ValidationException";

#[tokio::test]
async fn an_aligned_window_is_created_only_on_a_zone_whose_clock_has_boundaries() {
    // (case, the Timezone the wall clock window carries, whether the alarm is created)
    //
    // "You can specify an IANA time zone name (for example, America/New_York), a fixed UTC
    // offset (for example, +05:30), or an offset-prefixed identifier (for example, UTC+05:30).
    // The offset must be aligned to a multiple of 5 minutes. If you don't specify a time zone,
    // CloudWatch uses UTC." (`smithy_query(query="shape",shape="WallClockWindow")`) — each
    // documented form, and values in none of them.
    let rows: [(&str, Option<&str>, bool); 12] = [
        ("no zone named at all", None, true),
        ("the zone CloudWatch uses by default", Some("UTC"), true),
        ("an IANA name", Some("America/New_York"), true),
        (
            "an IANA name the documentation does not cite",
            Some("Asia/Kolkata"),
            true,
        ),
        ("a fixed offset ahead of UTC", Some("+05:30"), true),
        ("a fixed offset behind UTC", Some("-03:30"), true),
        ("an offset-prefixed identifier", Some("UTC+05:30"), true),
        (
            "an offset-prefixed identifier behind UTC",
            Some("UTC-08:00"),
            true,
        ),
        ("a name no zone answers to", Some("Mars/Phobos"), false),
        (
            "a zone abbreviation, which is not an IANA name",
            Some("Pacific Standard Time"),
            false,
        ),
        (
            "an offset off the published five-minute step",
            Some("+05:07"),
            false,
        ),
        ("an offset with no sign at all", Some("05:30"), false),
    ];

    let router = build_router();
    for (index, (case, timezone, accepted)) in rows.into_iter().enumerate() {
        let name = format!("zone-{index}");
        let row = Row {
            case,
            period: 300,
            timezone,
            shift: Shift::Stated(0.0),
            evaluation_periods: 1,
            breaching_periods_back: &[0],
        };
        let request = alarm_of(&row, &name, &wall_clock_window(&row));
        let (status, body) = call(&router, "PutMetricAlarm", &request).await;
        let stored = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [&name]})).await;
        let held = stored.pointer("/MetricAlarms/0").cloned();
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a zone stated in a documented form is accepted with HTTP 200, \
                 got {status} with body {body}"
            );
            let held = held.unwrap_or_else(|| {
                panic!("case {case:?}: an accepted window must be stored, got {stored}")
            });
            let read_back = held
                .get("EvaluationWindow")
                .cloned()
                .unwrap_or(Value::Null);
            let asked = wall_clock_window(&row);
            assert!(
                same_json_value(&asked, &read_back),
                "case {case:?}: expected EvaluationWindow {asked}, got {read_back}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: a value no zone answers to is refused with HTTP 400, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            REFUSAL_CODE,
            "case {case:?}: expected the error code {REFUSAL_CODE}, got {body}"
        );
        // The refusal names the member path the value arrived on and the value itself, the one
        // sentence this service states an unusable parameter value in.
        let value = timezone.unwrap_or_default();
        let expected =
            format!("The value {value} for parameter EvaluationWindow.WallClockWindow.Timezone \
                     is invalid.");
        assert_eq!(
            error_message(&body),
            expected,
            "case {case:?}: expected the message {expected:?}, got {body}"
        );
        assert_eq!(
            held, None,
            "case {case:?}: a refused configuration must store nothing, got {held:?}"
        );
    }
}
