// SPDX-License-Identifier: CC0-1.0

//! The FORM every instant inside one of this service's JSON documents is written in, end to end
//! over the wire.
//!
//! ONE rule, asserted over a table: **an alarm decision's machine-readable half —
//! `StateReasonData` ("An explanation for the alarm state, in JSON format.",
//! `smithy_query(query="shape",shape="MetricAlarm")`), the `HistoryData` that records it ("Data
//! about the alarm, in JSON format.", `smithy_query(query="shape",shape="AlarmHistoryItem")`)
//! and the notification an action carries — stamps EVERY instant inside it with a three-digit
//! millisecond field and a numeric UTC offset (`yyyy-MM-dd'T'HH:mm:ss.SSSZ`, e.g.
//! `2026-09-22T23:06:00.000+0000`), so one pattern parses every instant of the document; and
//! each such instant is the moment the evaluation really used — a graded datapoint's is the
//! start of its own `Period`-second bucket, and the query instant is the one the matching
//! modelled member reports.**
//!
//! The fixed part of the form is the `-`/`T`/`:` separators, the `.` before the milliseconds and
//! the trailing offset; the variable part is the instant, which comes from the alarm's own
//! evaluation window. A `Z`-suffixed instant, or one with no fractional field, is a different
//! form: a consumer parsing the document with that one pattern fails on it instead of recovering
//! when the evaluator graded a datapoint.
//!
//! The rows differ along what the rule generalises over: the length of the period (10, 20, 30
//! and 60 seconds — the high-resolution cadences and the regular one), the statistic that
//! aggregates a window (including the extended-statistic form), the comparison operator, the
//! state the evaluation computed, how many windows it graded and how many of them hold a real
//! datapoint, and the `TreatMissingData` arm — which decides whether the graded entries carry an
//! instant at all. The recorded case the issue was filed about is the first row.
//!
//! Every instant is found by WALKING the whole document rather than by naming the members the
//! issue cited, so a member this suite never names is held to the same form.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    build_router, document_instant_secs, now_secs, ok, wait_for_evaluation, AlarmKind,
};

/// The customer namespace every row of the table watches.
const NAMESPACE: &str = "Custom/DocumentInstants";

/// The metric every row watches.
const METRIC: &str = "Latency";

/// How far an instant a document names may sit from the test's own clock: every one of them is
/// the evaluation instant or the start of a window inside the evaluation range, so a document
/// can name nothing from before the suite started nor anything in the future.
const MAX_INSTANT_AGE_SECS: f64 = 3_600.0;

/// How close two renderings of ONE instant must be: a document holds an instant to the
/// millisecond, so the number a modelled member carries and the text a document writes agree to
/// within one.
const INSTANT_TOLERANCE_SECS: f64 = 0.0015;

/// One datapoint a row publishes: its value and WHICH period of the alarm it belongs to,
/// counting back from the period the publish falls in.
#[derive(Clone, Copy, Debug)]
struct Placed {
    /// The value published.
    value: f64,
    /// How many periods back from the current one the datapoint's time stamp falls in.
    periods_back: u32,
}

/// A datapoint of `value` stamped inside the `periods_back`th period back from the current one.
const fn placed(value: f64, periods_back: u32) -> Placed {
    Placed { value, periods_back }
}

/// One row of the table.
#[derive(Clone, Debug)]
struct Row {
    /// What this row is about, named by every assertion it makes.
    case: &'static str,
    /// The alarm's `Period`, in seconds.
    period: i32,
    /// The alarm's `Statistic`, for a row that names one.
    statistic: Option<&'static str>,
    /// The alarm's `ExtendedStatistic`, for a row that names one instead: "you must specify
    /// either Statistic or ExtendedStatistic but not both"
    /// (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="Statistic")`).
    extended_statistic: Option<&'static str>,
    /// The alarm's `ComparisonOperator`.
    comparison: &'static str,
    /// The alarm's `Threshold`.
    threshold: f64,
    /// The alarm's `EvaluationPeriods`.
    evaluation_periods: i32,
    /// The alarm's `DatapointsToAlarm`, when the row names one.
    datapoints_to_alarm: Option<i32>,
    /// The alarm's `TreatMissingData`, when the row names one.
    treat_missing_data: Option<&'static str>,
    /// The datapoints the row publishes, each placed in a period of its own choosing.
    data: &'static [Placed],
    /// The state the evaluation must put the alarm in.
    state: &'static str,
    /// How many entries the graded-datapoint list must carry an instant for: a window a
    /// missing-data arm substituted a datapoint for was never measured, so it names none.
    instant_bearing_datapoints: usize,
}

/// The table: one row per shape the rule generalises over.
const ROWS: &[Row] = &[
    Row {
        // The recorded case: a high-resolution `Maximum` over 10-second buckets, two of two
        // breaching, read back through `StateReasonData` and the history document.
        case: "a Maximum over two breaching 10-second buckets",
        period: 10,
        statistic: Some("Maximum"),
        extended_statistic: None,
        comparison: "GreaterThanOrEqualToThreshold",
        threshold: 50.0,
        evaluation_periods: 2,
        datapoints_to_alarm: Some(2),
        treat_missing_data: Some("missing"),
        data: &[placed(99.0, 1), placed(137.0, 0)],
        state: "ALARM",
        instant_bearing_datapoints: 2,
    },
    Row {
        case: "an Average over one 60-second bucket",
        period: 60,
        statistic: Some("Average"),
        extended_statistic: None,
        comparison: "GreaterThanThreshold",
        threshold: 50.0,
        evaluation_periods: 1,
        datapoints_to_alarm: None,
        treat_missing_data: None,
        data: &[placed(99.0, 0)],
        state: "ALARM",
        instant_bearing_datapoints: 1,
    },
    Row {
        // The OK direction: the state the evaluation computed is the one no datapoint breached,
        // and the entries it rests on carry their buckets' instants just the same.
        case: "a Minimum over a 30-second bucket that does not cross",
        period: 30,
        statistic: Some("Minimum"),
        extended_statistic: None,
        comparison: "LessThanThreshold",
        threshold: 10.0,
        evaluation_periods: 1,
        datapoints_to_alarm: None,
        treat_missing_data: None,
        data: &[placed(50.0, 0)],
        state: "OK",
        instant_bearing_datapoints: 1,
    },
    Row {
        // The extended-statistic form of the same document: a percentile is recorded under its
        // own member and its graded bucket is stamped the same way.
        case: "a p90 extended statistic over a 20-second bucket",
        period: 20,
        statistic: None,
        extended_statistic: Some("p90"),
        comparison: "GreaterThanThreshold",
        threshold: 50.0,
        evaluation_periods: 1,
        datapoints_to_alarm: None,
        treat_missing_data: None,
        data: &[placed(99.0, 0)],
        state: "ALARM",
        instant_bearing_datapoints: 1,
    },
    Row {
        // Gaps inside the window: three of five buckets hold a datapoint, so the graded entries
        // are two periods apart rather than consecutive.
        case: "three breaching 10-second buckets of five, with gaps",
        period: 10,
        statistic: Some("Maximum"),
        extended_statistic: None,
        comparison: "GreaterThanThreshold",
        threshold: 50.0,
        evaluation_periods: 5,
        datapoints_to_alarm: Some(3),
        treat_missing_data: Some("missing"),
        data: &[placed(99.0, 4), placed(99.0, 2), placed(99.0, 0)],
        state: "ALARM",
        instant_bearing_datapoints: 3,
    },
    Row {
        // A window NOTHING was published into, decided by the missing-data arm alone: the
        // entries stand for holes, so none of them names an instant and the document carries
        // only the instant of the query itself.
        case: "a breaching arm over a series with no datapoints at all",
        period: 60,
        statistic: Some("Maximum"),
        extended_statistic: None,
        comparison: "GreaterThanThreshold",
        threshold: 50.0,
        evaluation_periods: 2,
        datapoints_to_alarm: Some(2),
        treat_missing_data: Some("breaching"),
        data: &[],
        state: "ALARM",
        instant_bearing_datapoints: 0,
    },
];

/// The alarm request one row configures, watching its own dimension set so every row is its own
/// time series and all of them share one emulator.
fn alarm_of(row: &Row, name: &str) -> Value {
    let mut alarm = json!({
        "AlarmName": name,
        "Namespace": NAMESPACE,
        "MetricName": METRIC,
        "Dimensions": [{"Name": "Case", "Value": name}],
        "Period": row.period,
        "EvaluationPeriods": row.evaluation_periods,
        "Threshold": row.threshold,
        "ComparisonOperator": row.comparison,
    });
    let members = alarm.as_object_mut().expect("the request is an object");
    if let Some(statistic) = row.statistic {
        members.insert("Statistic".to_string(), json!(statistic));
    }
    if let Some(statistic) = row.extended_statistic {
        members.insert("ExtendedStatistic".to_string(), json!(statistic));
    }
    if let Some(count) = row.datapoints_to_alarm {
        members.insert("DatapointsToAlarm".to_string(), json!(count));
    }
    if let Some(arm) = row.treat_missing_data {
        members.insert("TreatMissingData".to_string(), json!(arm));
    }
    alarm
}

/// Publish a row's datapoints, each stamped INSIDE the period it names.
///
/// The time stamps are placed on the period grid, in the MIDDLE of the period they name, so the
/// period a datapoint belongs to is the one the row asked for whatever phase of a period the
/// publish falls in. A period boundary crossed between the publish and the reading shifts every
/// one of a row's periods back together, which changes neither the state nor the form of the
/// instants the document names.
async fn publish_placed(router: &MultiServiceRouter, name: &str, row: &Row) {
    if row.data.is_empty() {
        return;
    }
    let period = f64::from(row.period);
    let current_period_start = (now_secs() / period).floor() * period;
    let data: Vec<Value> = row
        .data
        .iter()
        .map(|datum| {
            let start = current_period_start - f64::from(datum.periods_back) * period;
            json!({
                "MetricName": METRIC,
                "Dimensions": [{"Name": "Case", "Value": name}],
                "Value": datum.value,
                // A high-resolution series: a row whose period is sub-minute reads only data
                // stored at that resolution, because regular resolution stores a datapoint at
                // 1-minute granularity.
                "StorageResolution": 1,
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

/// Whether `text` is written as a calendar instant at all: four digits of year, then the date
/// and time separators of every form a consumer might meet.
///
/// This is what makes the walk below find the instants of a document WITHOUT naming its members:
/// anything written as an instant is held to the one form, including a member this suite does
/// not know about.
fn looks_like_an_instant(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 19
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
}

/// Every instant one document writes, each with the member path it was found at — including the
/// instants of a document EMBEDDED in it, which is how a history item carries the state reason
/// data of the transition it records.
fn instants_of(path: &str, document: &Value, found: &mut Vec<(String, String)>) {
    match document {
        Value::Object(members) => {
            for (member, value) in members {
                instants_of(&format!("{path}.{member}"), value, found);
            }
        }
        Value::Array(entries) => {
            for (index, value) in entries.iter().enumerate() {
                instants_of(&format!("{path}[{index}]"), value, found);
            }
        }
        Value::String(text) => {
            if looks_like_an_instant(text) {
                found.push((path.to_string(), text.clone()));
            } else if let Ok(embedded) = serde_json::from_str::<Value>(text) {
                // A document embedded as a quoted string is still a document, and its instants
                // are held to the same form.
                if embedded.is_object() || embedded.is_array() {
                    instants_of(path, &embedded, found);
                }
            }
        }
        _ => {}
    }
}

/// Assert that every instant `document` writes is in the one form a consumer parses a document
/// with, and that none of them is outside the span the evaluation could have read, returning the
/// instants it found as epoch seconds.
fn graded_instants(case: &str, what: &str, document: &Value, now: f64) -> Vec<(String, f64)> {
    let mut found: Vec<(String, String)> = Vec::new();
    instants_of("", document, &mut found);
    found
        .into_iter()
        .map(|(path, text)| {
            let secs = document_instant_secs(&text).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {what}{path} must be written as \
                     yyyy-MM-dd'T'HH:mm:ss.SSSZ (a 3-digit millisecond field and the offset \
                     +0000), got {text:?}"
                )
            });
            assert!(
                secs <= now + INSTANT_TOLERANCE_SECS && secs >= now - MAX_INSTANT_AGE_SECS,
                "case {case:?}: {what}{path} must name an instant inside the evaluation range, \
                 got {text:?} against a clock reading {now}"
            );
            (path, secs)
        })
        .collect()
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

#[tokio::test]
async fn every_instant_an_alarm_document_writes_is_in_the_document_form() {
    // Every row watches its own series, so all of them are configured on ONE emulator and the
    // evaluation delay is waited out once rather than per row.
    let router = build_router();
    let mut names: Vec<String> = Vec::with_capacity(ROWS.len());
    for (index, row) in ROWS.iter().enumerate() {
        let name = format!("document-instants-{index}");
        ok(&router, "PutMetricAlarm", &alarm_of(row, &name)).await;
        publish_placed(&router, &name, row).await;
        names.push(name);
    }
    wait_for_evaluation();

    for (row, name) in ROWS.iter().zip(&names) {
        let case = row.case;
        let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
        let reported = &described["MetricAlarms"][0];
        assert_eq!(
            reported.get("StateValue").and_then(Value::as_str),
            Some(row.state),
            "case {case:?}: expected StateValue {}, got {reported}",
            row.state
        );
        let now = now_secs();
        let record = reason_data(case, reported);
        let instants = graded_instants(case, "StateReasonData", &record, now);
        assert!(
            !instants.is_empty(),
            "case {case:?}: the decision record must name the instant it queried the series at, \
             got {record}"
        );

        // The instant the record says it queried the series at is the instant the modelled
        // member beside it reports: one moment, two renderings.
        let query_date = record
            .get("queryDate")
            .and_then(Value::as_str)
            .and_then(document_instant_secs)
            .unwrap_or_else(|| panic!("case {case:?}: expected a queryDate, got {record}"));
        let updated = reported
            .get("StateUpdatedTimestamp")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a StateUpdatedTimestamp, got {reported}")
            });
        assert!(
            (query_date - updated).abs() < INSTANT_TOLERANCE_SECS,
            "case {case:?}: the record's queryDate must name the instant DescribeAlarms reports \
             as StateUpdatedTimestamp ({updated}), got {query_date}"
        );

        // A graded datapoint's instant is the START of its own `Period`-second bucket, which is
        // what lets a client join it back onto a metric read, and the record's startDate is the
        // oldest bucket the evaluation graded.
        let graded: Vec<(String, f64)> = instants
            .iter()
            .filter(|(path, _)| path.starts_with(".evaluatedDatapoints["))
            .cloned()
            .collect();
        assert_eq!(
            graded.len(),
            row.instant_bearing_datapoints,
            "case {case:?}: expected {} graded datapoints to name an instant, got {record}",
            row.instant_bearing_datapoints
        );
        let period = f64::from(row.period);
        for (path, secs) in &graded {
            assert!(
                secs.rem_euclid(period) == 0.0,
                "case {case:?}: StateReasonData{path} must be the start of a {period}-second \
                 bucket, got {secs}"
            );
            assert!(
                *secs <= query_date,
                "case {case:?}: StateReasonData{path} must name a bucket the query had already \
                 reached ({query_date}), got {secs}"
            );
        }
        if let Some(start_date) = record
            .get("startDate")
            .and_then(Value::as_str)
            .and_then(document_instant_secs)
        {
            assert!(
                start_date.rem_euclid(period) == 0.0 && start_date <= query_date,
                "case {case:?}: the record's startDate must be the start of a {period}-second \
                 bucket no later than the query, got {record}"
            );
            for (path, secs) in &graded {
                assert!(
                    *secs >= start_date,
                    "case {case:?}: StateReasonData{path} must sit inside the span the record's \
                     startDate opens ({start_date}), got {secs}"
                );
            }
        }

        // The HISTORY document of the same transition is the same decision record embedded in
        // the item that reports it, so its instants are held to the same form — this is the
        // document a client reads the graded datapoints out of after the fact.
        let history = ok(
            &router,
            "DescribeAlarmHistory",
            &json!({"AlarmName": name, "HistoryItemType": "StateUpdate"}),
        )
        .await;
        let items = history["AlarmHistoryItems"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            !items.is_empty(),
            "case {case:?}: expected a StateUpdate history item for the transition, got {history}"
        );
        for item in &items {
            let data: Value = item
                .get("HistoryData")
                .and_then(Value::as_str)
                .map(|text| {
                    serde_json::from_str(text).unwrap_or_else(|error| {
                        panic!("case {case:?}: HistoryData is not JSON: {error}")
                    })
                })
                .unwrap_or_else(|| panic!("case {case:?}: expected HistoryData, got {item}"));
            let recorded = graded_instants(case, "HistoryData", &data, now_secs());
            assert!(
                !recorded.is_empty(),
                "case {case:?}: the history document must carry the decision record's instants, \
                 got {data}"
            );
        }
    }
}

#[tokio::test]
async fn a_composite_alarm_document_stamps_its_children_the_same_way() {
    // The same rule over the OTHER writer of a decision record: a composite alarm's
    // `StateReasonData` names the child alarm whose transition its rule reacted to and when that
    // child moved, so that instant is written the way a document writes one too.
    let router = build_router();
    let parent = "document-instants-composite";
    AlarmKind::Composite.create(&router, parent, None).await;
    let child = format!("{parent}-child");
    ok(
        &router,
        "SetAlarmState",
        &json!({
            "AlarmName": child,
            "StateValue": "ALARM",
            "StateReason": "driven by the test",
        }),
    )
    .await;
    wait_for_evaluation();

    let described = ok(
        &router,
        "DescribeAlarms",
        &json!({"AlarmNames": [parent], "AlarmTypes": ["CompositeAlarm"]}),
    )
    .await;
    let reported = &described["CompositeAlarms"][0];
    let case = "a composite alarm's triggering child";
    let record = reason_data(case, reported);
    let instants = graded_instants(case, "StateReasonData", &record, now_secs());
    assert_eq!(
        instants.len(),
        1,
        "case {case:?}: expected the record to stamp the one child it reports, got {record}"
    );

    // That instant is the moment the child really moved, as the child's own modelled member
    // reports it.
    let child_alarm = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [child]})).await;
    let transitioned = child_alarm["MetricAlarms"][0]
        .get("StateTransitionedTimestamp")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected the child's transition instant"));
    let (path, secs) = &instants[0];
    assert!(
        (secs - transitioned).abs() < INSTANT_TOLERANCE_SECS,
        "case {case:?}: StateReasonData{path} must name the instant the child transitioned \
         ({transitioned}), got {secs}"
    );
}
