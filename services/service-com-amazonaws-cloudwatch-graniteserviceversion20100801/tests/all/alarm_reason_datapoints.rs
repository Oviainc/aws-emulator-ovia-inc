// SPDX-License-Identifier: CC0-1.0

//! The DATAPOINTS an evaluated metric alarm's `StateReason` names, end to end over the wire.
//!
//! ONE rule, asserted over a table: the `StateReason` of a state an evaluation computed names
//! the datapoints it GRADED — each value the statistic took over a window, with the instant of
//! that window, in square brackets — beside the counts it made over them and the threshold it
//! compared them against. A reader of the reason can therefore tell WHICH values decided the
//! state, not only how many there were.
//!
//! The rows differ along what the rule generalises over: the graded value (whole, fractional,
//! negative), the number of graded datapoints, the comparison operator, the statistic, the
//! state the evaluation computed, and an alarm that evaluates more windows than the modelled
//! `StateReason` length (`@length 0..1023`,
//! `smithy_query(query="shape",shape="StateReason")`) can hold the datapoints of.
//!
//! What the suite does NOT pin is the prose around the list or the layout of the instant: the
//! rule is that each graded value is named with the instant it was measured over, so each entry
//! is checked to be a decimal value followed by a calendar instant — in whichever form the
//! renderer writes one — rather than compared against one rendering.

use serde_json::{json, Value};

use crate::alarm_evaluation::{datum, patched, publish, Datum};
use crate::harness::{build_router, ok, wait_for_evaluation};

/// One row of the table.
struct Row {
    /// What this row is about, named by every assertion it makes.
    case: &'static str,
    /// The members this row changes on the base alarm.
    patch: Value,
    /// The datapoints it publishes into its own series.
    data: Vec<Datum>,
    /// The state the evaluation must put the alarm in.
    state: &'static str,
    /// The values its reason must name as graded, newest first.
    named: Vec<&'static str>,
    /// The values its reason must NOT name as graded.
    not_named: Vec<&'static str>,
    /// The sentence fragments its reason must carry beside the list.
    fragments: Vec<&'static str>,
}

/// One row of the table, in the order the table below reads.
fn row(
    case: &'static str,
    patch: Value,
    data: Vec<Datum>,
    state: &'static str,
    named: Vec<&'static str>,
    not_named: Vec<&'static str>,
    fragments: Vec<&'static str>,
) -> Row {
    Row {
        case,
        patch,
        data,
        state,
        named,
        not_named,
        fragments,
    }
}

/// The modelled maximum length of `StateReason`
/// (`smithy_query(query="shape",shape="StateReason")`).
const MAX_STATE_REASON_LEN: usize = 1023;

/// The shortest `Period` an alarm may be evaluated on: "Valid values are 10, 20, 30, and any
/// multiple of 60." (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="Period")`).
const SHORTEST_PERIOD_SECS: i32 = 10;

/// How many windows the last row's alarm evaluates: enough graded datapoints that naming all
/// of them would take the reason past [`MAX_STATE_REASON_LEN`].
const CROWDED_PERIODS: i32 = 40;

/// The datapoints one reason names, as the text between its first pair of square brackets.
///
/// The graded list is the first bracketed part of a reason; a missing-data arm, which no row of
/// this table configures, is reported in brackets of its own further on.
fn graded_list(reason: &str) -> Option<&str> {
    let open = reason.find('[')? + 1;
    let close = reason[open..].find(']')? + open;
    Some(&reason[open..close])
}

/// Whether `entry` is a graded datapoint: the value the statistic took, then the instant of the
/// window it was measured over, in parentheses.
fn is_graded_datapoint(entry: &str) -> bool {
    let Some((value, instant)) = entry.split_once(" (") else {
        return false;
    };
    let Some(instant) = instant.strip_suffix(')') else {
        return false;
    };
    let numeric = value.contains('.')
        && value
            .chars()
            .all(|character| character.is_ascii_digit() || "+-.e".contains(character));
    numeric && is_calendar_instant(instant)
}

/// Whether `instant` is a calendar instant rather than a count or a placeholder: a date and a
/// time of day, made only of digits and the separators a calendar form uses.
///
/// Deliberately form-agnostic — the rule is that the datapoint carries the instant it was
/// measured over, not that the instant is written one particular way.
fn is_calendar_instant(instant: &str) -> bool {
    /// The shortest calendar instant any form writes (`1/1/70 0:00:00`).
    const MIN_INSTANT_LEN: usize = 14;

    instant.len() >= MIN_INSTANT_LEN
        && instant.contains(':')
        && instant.matches(|character: char| character.is_ascii_digit()).count() >= 10
        && instant
            .chars()
            .all(|character| character.is_ascii_digit() || "-:/.TZ+ ".contains(character))
        // A datapoint published by this suite was measured seconds ago, so an instant at the
        // start of the epoch is a value nothing measured.
        && !instant.starts_with("1970")
        && !instant.contains("/70 ")
}

#[tokio::test]
async fn an_evaluated_state_reason_names_the_datapoints_it_graded() {
    // The base alarm compares a 60-second `Average` against a threshold of 10 with
    // `GreaterThanThreshold` over one evaluation period.
    let mut crowded: Vec<Datum> = Vec::new();
    for window in 0..CROWDED_PERIODS {
        // One datapoint per evaluated window, each a different value, so the reason's list can
        // be read back to the window it came from: the newest is 100.0 and the oldest 139.0.
        let age = f64::from(window * SHORTEST_PERIOD_SECS) + 2.0;
        crowded.push(datum(100.0 + f64::from(window), age));
    }

    let rows: Vec<Row> = vec![
        row(
            "one whole-numbered datapoint past the threshold",
            json!({}),
            vec![datum(137.0, 0.0)],
            "ALARM",
            vec!["137.0"],
            Vec::new(),
            vec![
                "Threshold Crossed: 1 out of the last 1 datapoints [137.0 (",
                "with the threshold (10.0)",
            ],
        ),
        row(
            "a fractional datapoint at a fractional threshold",
            json!({"Threshold": 0.5, "ComparisonOperator": "GreaterThanOrEqualToThreshold"}),
            vec![datum(0.5, 0.0)],
            "ALARM",
            vec!["0.5"],
            Vec::new(),
            vec![
                "Threshold Crossed: 1 out of the last 1 datapoints [0.5 (",
                "GreaterThanOrEqualToThreshold",
                "with the threshold (0.5)",
            ],
        ),
        row(
            "a negative datapoint under a negative threshold",
            json!({"Threshold": -2.5, "ComparisonOperator": "LessThanThreshold"}),
            vec![datum(-7.25, 0.0)],
            "ALARM",
            vec!["-7.25"],
            Vec::new(),
            vec![
                "Threshold Crossed: 1 out of the last 1 datapoints [-7.25 (",
                "with the threshold (-2.5)",
            ],
        ),
        row(
            "three graded datapoints, newest first",
            json!({
                "Period": SHORTEST_PERIOD_SECS,
                "EvaluationPeriods": 3,
                "DatapointsToAlarm": 3,
            }),
            vec![datum(11.0, 2.0), datum(12.0, 12.0), datum(13.0, 22.0)],
            "ALARM",
            vec!["11.0", "12.0", "13.0"],
            Vec::new(),
            vec!["Threshold Crossed: 3 out of the last 3 datapoints [11.0 ("],
        ),
        row(
            "a datapoint the comparison did not hold for is named too",
            json!({}),
            vec![datum(1.0, 0.0)],
            "OK",
            vec!["1.0"],
            Vec::new(),
            vec![
                "Threshold Crossed: 0 out of the last 1 datapoints [1.0 (",
                "with the threshold (10.0)",
            ],
        ),
        row(
            "the value a percentile statistic computed is the value that is named",
            json!({"Statistic": Value::Null, "ExtendedStatistic": "p90"}),
            vec![datum(42.0, 0.0)],
            "ALARM",
            vec!["42.0"],
            Vec::new(),
            vec!["Threshold Crossed: 1 out of the last 1 datapoints [42.0 ("],
        ),
        row(
            "more graded datapoints than the modelled length holds",
            json!({
                "Period": SHORTEST_PERIOD_SECS,
                "EvaluationPeriods": CROWDED_PERIODS,
                "DatapointsToAlarm": 1,
            }),
            crowded,
            "ALARM",
            // The most recent datapoints are the ones the reason names; the oldest are left out
            // rather than taking the reason past its modelled length.
            vec!["100.0", "101.0"],
            vec!["139.0", "138.0"],
            vec!["Threshold Crossed: 40 out of the last 40 datapoints [100.0 ("],
        ),
    ];

    // Every row's alarm watches its own series (its `DomainName` dimension is its own name), so
    // all of them are configured on ONE emulator and the evaluation delay is waited out once.
    let router = build_router();
    let mut names: Vec<String> = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let name = format!("graded-{index}");
        ok(&router, "PutMetricAlarm", &patched(&name, &row.patch)).await;
        publish(&router, &name, &row.data).await;
        names.push(name);
    }
    wait_for_evaluation();

    for (row, name) in rows.iter().zip(&names) {
        let case = row.case;
        let state = row.state;
        let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
        let reported = &described["MetricAlarms"][0];
        assert_eq!(
            reported.get("StateValue").and_then(Value::as_str),
            Some(state),
            "case {case:?}: expected StateValue {state}, got {reported}"
        );
        let reason = reported
            .get("StateReason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            reason.len() <= MAX_STATE_REASON_LEN,
            "case {case:?}: StateReason must stay inside its modelled length, got {} \
             characters: {reason:?}",
            reason.len()
        );
        for fragment in &row.fragments {
            assert!(
                reason.contains(fragment),
                "case {case:?}: expected StateReason to report {fragment:?}, got {reason:?}"
            );
        }

        // The bracketed list: every entry is a value with the instant of the window it was
        // measured over, and that instant is a calendar instant rather than a filler.
        let listed = graded_list(&reason)
            .unwrap_or_else(|| panic!("case {case:?}: no bracketed datapoint list in {reason:?}"));
        let entries: Vec<&str> = listed.split(", ").collect();
        for entry in &entries {
            assert!(
                is_graded_datapoint(entry),
                "case {case:?}: {entry:?} is not a value with the instant of its window, in \
                 {reason:?}"
            );
        }
        assert!(
            entries.len() >= row.named.len(),
            "case {case:?}: expected at least {} graded datapoints, got {entries:?}",
            row.named.len()
        );

        // The values the evaluation graded, in the order the reason lists them: newest first.
        let mut previous = 0;
        for value in &row.named {
            let needle = format!("{value} (");
            let at = listed.find(needle.as_str()).unwrap_or_else(|| {
                panic!("case {case:?}: expected the reason to name {value:?}, got {reason:?}")
            });
            assert!(
                at >= previous,
                "case {case:?}: expected {value:?} to be listed after the newer datapoints, \
                 got {reason:?}"
            );
            previous = at;
        }
        for value in &row.not_named {
            let needle = format!("{value} (");
            assert!(
                !listed.contains(needle.as_str()),
                "case {case:?}: expected the reason not to name {value:?}, got {reason:?}"
            );
        }
    }
}
