// SPDX-License-Identifier: CC0-1.0

//! An alarm whose THRESHOLD is an anomaly detection model, evaluated end to end over the wire.
//!
//! One rule, asserted over a table whose rows differ along what it generalises over: **an alarm
//! that designates an `ANOMALY_DETECTION_BAND` query as its threshold ("In an alarm based on an
//! anomaly detection model, this is the ID of the ANOMALY_DETECTION_BAND function used as the
//! threshold for the alarm",
//! `smithy_query(query="doc",shape="PutMetricAlarmInput",member="ThresholdMetricId")`) is
//! decided by the same "M out of N" predicate as any metric alarm, with each graded window
//! compared against the BOUND of that band at the window's own period — the upper bound under
//! `GreaterThanUpperThreshold`, the lower bound under `LessThanLowerThreshold` and either of
//! them under `LessThanLowerOrGreaterThanUpperThreshold` — and its decision record reports that
//! bound on the datapoint it decided, which is the value `GetMetricData` answers the SAME band
//! query with at that instant.**
//!
//! Rows differ in the comparison operator, in the band's width (an explicit one, a wider one and
//! the default the call leaves out), in where the published value sits relative to the band
//! (far above it, far below it, inside it) and in the state the verdict therefore reaches.
//!
//! The second and third tests assert the two halves of the same rule that are about the MODEL
//! rather than one verdict: a model a live alarm's threshold is drawn from is re-created when it
//! is deleted under that alarm (while a model no alarm references stays deleted), and the same
//! alarm rides its band from `OK` into `ALARM` when the series leaves it, recording the
//! transition it made.
//!
//! The last test asserts what a decision record reports PER DATAPOINT under either kind of
//! threshold: an entry of `StateReasonData.evaluatedDatapoints` names what was measured over the
//! period it stands for — its instant, its observations and its value — plus the threshold it
//! was compared against where that threshold moves with the series, and a threshold the whole
//! evaluation shares is reported once for the evaluation instead.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    alarm_history_items, build_router, described_alarm, document_instant_secs,
    graded_window_stamp, ok, wait_for_evaluation, wait_for_period_room, AlarmKind,
};

/// The namespace every series of this suite is published under.
const NAMESPACE: &str = "MyApp/BandAlarms";

/// The metric every alarm of this suite watches.
const METRIC: &str = "Latency";

/// The width of the windows every row grades, in seconds.
const PERIOD: f64 = 60.0;

/// How much of the period a row needs left to publish its datapoints and read the verdict back
/// inside the window it placed them relative to.
const PERIOD_ROOM: f64 = 20.0;

/// The baseline history every row seeds its own series with, oldest last: the values the model
/// is trained on, spread around one level so a band of any width has room either side of it.
const BASELINE: &[f64] = &[100.0, 104.0, 96.0, 102.0, 98.0, 101.0, 99.0, 103.0, 97.0, 100.0];

/// A value inside any band those baseline values train: the level they are spread around.
const IN_BAND: f64 = 100.0;

/// A value far above that band.
const ABOVE_BAND: f64 = 1_000.0;

/// A value far below it.
const BELOW_BAND: f64 = -800.0;

/// How close the bound an alarm CITED has to be to the bound `GetMetricData` reports for the
/// same band at the same instant to count as the same number: both are computed from the same
/// model, so this leaves room for nothing but the JSON rendering of a float.
const RELATIVE_TOLERANCE: f64 = 1e-9;

/// Which bound of the band a row's comparison is decided by, and so which of the band's two
/// series the cited threshold must equal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bound {
    /// The band's "High" series.
    Upper,
    /// Its "Low" series.
    Lower,
}

impl Bound {
    /// The suffix the band labels this bound's series with, as `GetMetricData` reports it.
    const fn label_suffix(self) -> &'static str {
        match self {
            Self::Upper => " High",
            Self::Lower => " Low",
        }
    }

}

/// One row of the table.
#[derive(Clone, Copy, Debug)]
struct Row {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The alarm's `ComparisonOperator`.
    comparison: &'static str,
    /// The band's width as the expression spells it, `None` leaving the call to the default
    /// ("If you omit this, the default of 2 is used").
    width: Option<f64>,
    /// The value published into the window the evaluation grades.
    published: f64,
    /// Whether that value is inside the band the baseline trains.
    inside: bool,
    /// The `StateValue` the verdict reaches.
    state: &'static str,
    /// The bound the record must cite for the graded datapoint.
    bound: Bound,
    /// What the `StateReason` of this comparison calls the thresholds it cites.
    thresholds_name: &'static str,
}

/// The table: one row per shape the rule generalises over.
const ROWS: &[Row] = &[
    Row {
        case: "a value far above the band, compared against its upper bound",
        comparison: "GreaterThanUpperThreshold",
        width: Some(2.0),
        published: ABOVE_BAND,
        inside: false,
        state: "ALARM",
        bound: Bound::Upper,
        thresholds_name: "the upper thresholds",
    },
    Row {
        case: "a value inside the band, compared against its upper bound",
        comparison: "GreaterThanUpperThreshold",
        width: Some(2.0),
        published: IN_BAND,
        inside: true,
        state: "OK",
        bound: Bound::Upper,
        thresholds_name: "the upper thresholds",
    },
    Row {
        case: "a value inside a band of the default width, compared against its upper bound",
        comparison: "GreaterThanUpperThreshold",
        width: None,
        published: IN_BAND,
        inside: true,
        state: "OK",
        bound: Bound::Upper,
        thresholds_name: "the upper thresholds",
    },
    Row {
        case: "a value far below a wider band, compared against its lower bound",
        comparison: "LessThanLowerThreshold",
        width: Some(3.0),
        published: BELOW_BAND,
        inside: false,
        state: "ALARM",
        bound: Bound::Lower,
        thresholds_name: "the lower thresholds",
    },
    Row {
        case: "a value inside the band, compared against its lower bound",
        comparison: "LessThanLowerThreshold",
        width: Some(2.0),
        published: IN_BAND,
        inside: true,
        state: "OK",
        bound: Bound::Lower,
        thresholds_name: "the lower thresholds",
    },
    Row {
        case: "a value far above the band, compared against both its bounds",
        comparison: "LessThanLowerOrGreaterThanUpperThreshold",
        width: Some(2.0),
        published: ABOVE_BAND,
        inside: false,
        state: "ALARM",
        bound: Bound::Upper,
        thresholds_name: "the band",
    },
    Row {
        case: "a value far below the band, compared against both its bounds",
        comparison: "LessThanLowerOrGreaterThanUpperThreshold",
        width: Some(2.0),
        published: BELOW_BAND,
        inside: false,
        state: "ALARM",
        bound: Bound::Lower,
        thresholds_name: "the band",
    },
    Row {
        case: "a value inside the band, compared against both its bounds",
        comparison: "LessThanLowerOrGreaterThanUpperThreshold",
        width: Some(2.0),
        published: IN_BAND,
        inside: true,
        state: "OK",
        bound: Bound::Upper,
        thresholds_name: "the band",
    },
];

/// The dimensions that tell one row's series — and so one row's detector and band — from
/// another's.
fn dimensions(index: usize) -> Value {
    json!([{"Name": "Series", "Value": format!("s{index}")}])
}

/// The band expression a row's threshold query carries.
fn band_expression(width: Option<f64>) -> String {
    match width {
        Some(width) => format!("ANOMALY_DETECTION_BAND(m1, {width})"),
        None => "ANOMALY_DETECTION_BAND(m1)".to_string(),
    }
}

/// The query array an anomaly alarm over the series `dimensions` is written with: the metric it
/// watches, and the band its threshold is drawn from.
fn metrics(dimensions: &Value, width: Option<f64>) -> Value {
    json!([
        {
            "Id": "m1",
            "ReturnData": true,
            "MetricStat": {
                "Metric": {
                    "Namespace": NAMESPACE,
                    "MetricName": METRIC,
                    "Dimensions": dimensions,
                },
                "Period": PERIOD,
                "Stat": "Average",
            },
        },
        {"Id": "ad1", "Expression": band_expression(width), "ReturnData": true},
    ])
}

/// Publish one datapoint of the series `dimensions`.
async fn publish(router: &MultiServiceRouter, dimensions: &Value, value: f64, at: f64) {
    ok(
        router,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [{
                "MetricName": METRIC,
                "Dimensions": dimensions,
                "Value": value,
                "Unit": "Milliseconds",
                "Timestamp": at,
            }],
        }),
    )
    .await;
}

/// Seed the history the model of the series `dimensions` is trained on: [`BASELINE`], one
/// datapoint per period, in the periods BEFORE the one the row's own datapoint goes into.
async fn seed_baseline(router: &MultiServiceRouter, dimensions: &Value, newest: f64) {
    for (periods_back, value) in BASELINE.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let offset = (periods_back + 1) as f64 * PERIOD;
        publish(router, dimensions, *value, newest - offset).await;
    }
}

/// Create the anomaly detector the band of the series `dimensions` is drawn from, and answer
/// its id.
async fn put_detector(router: &MultiServiceRouter, dimensions: &Value) -> String {
    let created = ok(
        router,
        "PutAnomalyDetector",
        &json!({
            "SingleMetricAnomalyDetector": {
                "Namespace": NAMESPACE,
                "MetricName": METRIC,
                "Dimensions": dimensions,
                "Stat": "Average",
            },
        }),
    )
    .await;
    created["AnomalyDetectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("PutAnomalyDetector must report an id, got {created}"))
        .to_string()
}

/// Delete that detector.
async fn delete_detector(router: &MultiServiceRouter, dimensions: &Value) {
    ok(
        router,
        "DeleteAnomalyDetector",
        &json!({
            "SingleMetricAnomalyDetector": {
                "Namespace": NAMESPACE,
                "MetricName": METRIC,
                "Dimensions": dimensions,
                "Stat": "Average",
            },
        }),
    )
    .await;
}

/// Every anomaly detector of this suite's metric, as `DescribeAnomalyDetectors` reports them,
/// by id.
async fn described_detectors(router: &MultiServiceRouter) -> Vec<Value> {
    let listed = ok(
        router,
        "DescribeAnomalyDetectors",
        &json!({"Namespace": NAMESPACE, "MetricName": METRIC}),
    )
    .await;
    listed["AnomalyDetectors"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// One anomaly alarm over the series `dimensions`, with the band as its threshold.
async fn put_band_alarm(
    router: &MultiServiceRouter,
    name: &str,
    dimensions: &Value,
    width: Option<f64>,
    comparison: &str,
) {
    ok(
        router,
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "Metrics": metrics(dimensions, width),
            "ThresholdMetricId": "ad1",
            "ComparisonOperator": comparison,
            "EvaluationPeriods": 1,
            "TreatMissingData": "missing",
            "ActionsEnabled": false,
        }),
    )
    .await;
}

/// What `GetMetricData` answers the SAME band query with: the `(instant, value)` pairs of the
/// bound `bound` names, over the windows around `newest`.
async fn read_band(
    router: &MultiServiceRouter,
    dimensions: &Value,
    width: Option<f64>,
    bound: Bound,
    newest: f64,
) -> Vec<(f64, f64)> {
    let window = (newest / PERIOD).floor() * PERIOD;
    let body = ok(
        router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": metrics(dimensions, width),
            "StartTime": window - 3.0 * PERIOD,
            "EndTime": window + PERIOD,
            "ScanBy": "TimestampAscending",
        }),
    )
    .await;
    let results = body["MetricDataResults"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let reported = results
        .iter()
        .find(|result| {
            result
                .get("Label")
                .and_then(Value::as_str)
                .is_some_and(|label| label.ends_with(bound.label_suffix()))
        })
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "GetMetricData must report the {} series of the band, got {body}",
                bound.label_suffix().trim()
            )
        });
    let timestamps: Vec<f64> = numbers(&reported, "Timestamps");
    let values: Vec<f64> = numbers(&reported, "Values");
    timestamps.into_iter().zip(values).collect()
}

/// One numeric list member of a result or a document.
fn numbers(value: &Value, member: &str) -> Vec<f64> {
    value
        .get(member)
        .and_then(Value::as_array)
        .map(|listed| listed.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

/// The `StateReasonData` document of a described alarm, parsed.
fn reason_data(alarm: &Value, case: &str) -> Value {
    let text = alarm
        .get("StateReasonData")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a StateReasonData member, got {alarm}"));
    serde_json::from_str(text)
        .unwrap_or_else(|_| panic!("case {case:?}: StateReasonData must be JSON, got {text}"))
}

/// The one datapoint the verdict of a described alarm rests on.
fn verdict_datapoint(document: &Value, case: &str) -> Value {
    document
        .get("evaluatedDatapoints")
        .and_then(Value::as_array)
        .and_then(|listed| listed.first().cloned())
        .unwrap_or_else(|| {
            panic!("case {case:?}: the record must name the datapoint the verdict rests on, got {document}")
        })
}

/// Whether two numbers computed from the same model are the same number.
fn same_number(left: f64, right: f64) -> bool {
    (left - right).abs() <= RELATIVE_TOLERANCE * left.abs().max(right.abs()).max(1.0)
}

#[tokio::test]
async fn a_band_alarm_is_decided_by_the_bound_of_the_band_at_each_graded_period() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let dimensions = dimensions(index);
        // Both this row's publishes and the reads that follow them have to land in the window
        // it places its datapoint relative to, so it asks for that much of the period first.
        wait_for_period_room(PERIOD, PERIOD_ROOM).await;
        let stamp = graded_window_stamp(PERIOD);
        seed_baseline(&router, &dimensions, stamp).await;
        publish(&router, &dimensions, row.published, stamp).await;
        put_detector(&router, &dimensions).await;
        // The model is created pending training, so the band says nothing until the training
        // the write started comes due.
        wait_for_evaluation();

        let name = format!("band-alarm-{index}");
        put_band_alarm(&router, &name, &dimensions, row.width, row.comparison).await;
        wait_for_evaluation();

        let alarm = described_alarm(&router, AlarmKind::Metric, &name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: DescribeAlarms reports no alarm {name}"));
        assert_eq!(
            alarm.get("StateValue").and_then(Value::as_str),
            Some(row.state),
            "case {case:?}: expected the band verdict over {} to be {:?}, got {alarm}",
            row.published,
            row.state
        );

        // Where the published value sits relative to the band the SAME query answers a reader
        // with: a row that expects a verdict of `OK` published a value the band holds, and one
        // that expects `ALARM` published a value outside it.
        let lower = read_band(&router, &dimensions, row.width, Bound::Lower, stamp).await;
        let upper = read_band(&router, &dimensions, row.width, Bound::Upper, stamp).await;
        let newest_lower = lower
            .last()
            .copied()
            .unwrap_or_else(|| panic!("case {case:?}: the band's Low series must carry points"));
        let newest_upper = upper
            .last()
            .copied()
            .unwrap_or_else(|| panic!("case {case:?}: the band's High series must carry points"));
        let held = row.published >= newest_lower.1 && row.published <= newest_upper.1;
        assert_eq!(
            held, row.inside,
            "case {case:?}: expected the published {} to be {} the band [{}, {}], got {held}",
            row.published,
            if row.inside { "inside" } else { "outside" },
            newest_lower.1,
            newest_upper.1
        );

        // The record cites the bound the comparison was made against, on the datapoint it
        // decided: the value the band's own series carries at that datapoint's instant.
        let document = reason_data(&alarm, case);
        assert!(
            document.get("threshold").is_none(),
            "case {case:?}: an alarm whose threshold is a model reports no one threshold for the \
             whole evaluation, got {document}"
        );
        let decided = verdict_datapoint(&document, case);
        assert_eq!(
            decided.get("value").and_then(Value::as_f64),
            Some(row.published),
            "case {case:?}: the record must cite the value it graded, got {decided}"
        );
        assert!(
            decided.get("datapoint").is_none(),
            "case {case:?}: a datapoint the evaluation measured reports what was measured over \
             its period and no breaching flag, got {decided}"
        );
        let cited = decided
            .get("threshold")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the cited datapoint must carry its own threshold, got {decided}")
            });
        let at = decided
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(document_instant_secs)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the cited datapoint must name its instant, got {decided}")
            });
        let series = if row.bound == Bound::Upper { &upper } else { &lower };
        let reported = series
            .iter()
            .find(|(instant, _)| (instant - at).abs() < 1.0)
            .map(|(_, value)| *value)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: GetMetricData must report the band at the evaluated instant \
                     {at}, got {series:?}"
                )
            });
        assert!(
            same_number(cited, reported),
            "case {case:?}: expected the cited threshold to be the band's {} value {reported} at \
             the evaluated instant, got {cited}",
            row.bound.label_suffix().trim()
        );

        // The sentence states the comparison the evaluation made against that bound, and names
        // the bounds rather than a static threshold.
        let reason = alarm
            .get("StateReason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            reason.contains(&format!("{} [", row.thresholds_name)),
            "case {case:?}: expected the reason to name {:?}, got {reason:?}",
            row.thresholds_name
        );
        assert!(
            !reason.contains("the threshold ("),
            "case {case:?}: an anomaly alarm's reason names no static threshold, got {reason:?}"
        );
    }
}

#[tokio::test]
async fn a_live_band_alarm_re_creates_the_model_its_threshold_is_drawn_from() {
    let router = build_router();
    // Two identical detectors, on two series: one a live alarm's threshold is drawn from, and
    // one no alarm references at all.
    let watched = dimensions(0);
    let unwatched = dimensions(1);
    wait_for_period_room(PERIOD, PERIOD_ROOM).await;
    let stamp = graded_window_stamp(PERIOD);
    for dimensions in [&watched, &unwatched] {
        seed_baseline(&router, dimensions, stamp).await;
    }
    // The value the alarm grades goes one period further back than the value that will take it
    // out of the band later, so each phase has a window of its own.
    publish(&router, &watched, IN_BAND, stamp - PERIOD).await;
    let watched_id = put_detector(&router, &watched).await;
    let unwatched_id = put_detector(&router, &unwatched).await;
    wait_for_evaluation();

    let name = "band-alarm-rides-its-band";
    put_band_alarm(&router, name, &watched, Some(2.0), "GreaterThanUpperThreshold").await;
    wait_for_evaluation();
    let alarm = described_alarm(&router, AlarmKind::Metric, name)
        .await
        .unwrap_or_else(|| panic!("DescribeAlarms reports no alarm {name}"));
    assert_eq!(
        alarm.get("StateValue").and_then(Value::as_str),
        Some("OK"),
        "an in-band value must drive the band alarm to OK, got {alarm}"
    );

    // Both models are deleted. The one the live alarm's threshold is drawn from is created
    // again for it — by the evaluation the clock owes that alarm, so a span later — and the one
    // no alarm references stays gone.
    for dimensions in [&watched, &unwatched] {
        delete_detector(&router, dimensions).await;
    }
    wait_for_evaluation();
    let listed = described_detectors(&router).await;
    let reported = |id: &str| -> Option<Value> {
        listed
            .iter()
            .find(|detector| {
                detector.get("AnomalyDetectorId").and_then(Value::as_str) == Some(id)
            })
            .cloned()
    };
    let restored = reported(&watched_id).unwrap_or_else(|| {
        panic!("the model the live alarm's threshold is drawn from must be re-created, got {listed:?}")
    });
    let state = restored
        .get("StateValue")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        ["PENDING_TRAINING", "TRAINED", "TRAINED_INSUFFICIENT_DATA"].contains(&state.as_str()),
        "a re-created model reports a modelled AnomalyDetectorStateValue, got {state:?}"
    );
    assert!(
        reported(&unwatched_id).is_none(),
        "a model no alarm's threshold is drawn from must stay deleted, got {listed:?}"
    );
    let still_ok = described_alarm(&router, AlarmKind::Metric, name)
        .await
        .unwrap_or_else(|| panic!("DescribeAlarms reports no alarm {name}"));
    assert_eq!(
        still_ok.get("StateValue").and_then(Value::as_str),
        Some("OK"),
        "an alarm whose model is being re-trained keeps the state it has, got {still_ok}"
    );

    // The series then leaves the band, and the same alarm crosses — recording the transition it
    // made out of the state it was in.
    publish(&router, &watched, ABOVE_BAND, stamp).await;
    wait_for_evaluation();
    let crossed = described_alarm(&router, AlarmKind::Metric, name)
        .await
        .unwrap_or_else(|| panic!("DescribeAlarms reports no alarm {name}"));
    assert_eq!(
        crossed.get("StateValue").and_then(Value::as_str),
        Some("ALARM"),
        "a value far outside the band must drive the same alarm to ALARM, got {crossed}"
    );
    let items = alarm_history_items(&router, AlarmKind::Metric, name, "StateUpdate").await;
    let transitions: Vec<Value> = items
        .iter()
        .filter_map(|item| {
            item.get("HistoryData")
                .and_then(Value::as_str)
                .and_then(|text| serde_json::from_str::<Value>(text).ok())
        })
        .collect();
    assert!(
        transitions.iter().any(|data| {
            data.pointer("/oldState/stateValue").and_then(Value::as_str) == Some("OK")
                && data.pointer("/newState/stateValue").and_then(Value::as_str) == Some("ALARM")
        }),
        "the crossing must be recorded as a StateUpdate out of OK, got {transitions:?}"
    );
}

#[tokio::test]
async fn a_band_alarm_whose_model_says_nothing_yet_keeps_the_state_it_has() {
    let router = build_router();
    // No detector at all: the designated query yields a band with no points, so there is no
    // threshold to compare the series against and the alarm reports the state its creation left
    // it in rather than one nothing computed.
    let dimensions = dimensions(0);
    wait_for_period_room(PERIOD, PERIOD_ROOM).await;
    let stamp = graded_window_stamp(PERIOD);
    seed_baseline(&router, &dimensions, stamp).await;
    publish(&router, &dimensions, IN_BAND, stamp).await;

    let name = "band-alarm-without-a-model";
    put_band_alarm(&router, name, &dimensions, Some(2.0), "GreaterThanUpperThreshold").await;
    wait_for_evaluation();
    let alarm = described_alarm(&router, AlarmKind::Metric, name)
        .await
        .unwrap_or_else(|| panic!("DescribeAlarms reports no alarm {name}"));
    assert_eq!(
        alarm.get("StateValue").and_then(Value::as_str),
        Some("INSUFFICIENT_DATA"),
        "an alarm whose model says nothing yet keeps the state it has, got {alarm}"
    );
    assert!(
        alarm.get("StateReasonData").is_none(),
        "an evaluation that decided nothing writes no decision record, got {alarm}"
    );

    // The model the alarm's threshold needs is one the store owes it, so the detector reader
    // reports it — and once its training is due the same alarm is decided by the band.
    let listed = described_detectors(&router).await;
    assert_eq!(
        listed.len(),
        1,
        "the model the live alarm's threshold is drawn from must be created for it, got {listed:?}"
    );
    wait_for_evaluation();
    let decided = described_alarm(&router, AlarmKind::Metric, name)
        .await
        .unwrap_or_else(|| panic!("DescribeAlarms reports no alarm {name}"));
    assert_eq!(
        decided.get("StateValue").and_then(Value::as_str),
        Some("OK"),
        "an in-band value must drive the alarm to OK once the model it needs is trained, got \
         {decided}"
    );
}

#[tokio::test]
async fn a_decision_record_reports_a_moving_threshold_per_datapoint_and_a_fixed_one_once() {
    let router = build_router();
    // Two alarms over the same published datapoint, differing in the KIND of threshold they are
    // compared against: a number, and the band a model draws.
    let dimensions = dimensions(0);
    wait_for_period_room(PERIOD, PERIOD_ROOM).await;
    let stamp = graded_window_stamp(PERIOD);
    seed_baseline(&router, &dimensions, stamp).await;
    publish(&router, &dimensions, ABOVE_BAND, stamp).await;
    put_detector(&router, &dimensions).await;
    wait_for_evaluation();

    /// The threshold a static alarm compares the published value against, which it is above.
    const STATIC_THRESHOLD: f64 = 50.0;

    let band = "record-shape-band";
    put_band_alarm(&router, band, &dimensions, Some(2.0), "GreaterThanUpperThreshold").await;
    let fixed = "record-shape-static";
    ok(
        &router,
        "PutMetricAlarm",
        &json!({
            "AlarmName": fixed,
            "Namespace": NAMESPACE,
            "MetricName": METRIC,
            "Dimensions": dimensions,
            "Statistic": "Average",
            "Period": PERIOD,
            "EvaluationPeriods": 1,
            "Threshold": STATIC_THRESHOLD,
            "ComparisonOperator": "GreaterThanThreshold",
            "TreatMissingData": "missing",
            "ActionsEnabled": false,
        }),
    )
    .await;
    wait_for_evaluation();

    // (the alarm, what this case varies, whether its threshold moves with the series, whether
    // its graded value is one metric's own observations)
    for (name, case, moving, measured) in [
        (band, "a threshold the model moves with the series", true, false),
        (fixed, "a threshold the alarm is configured with", false, true),
    ] {
        let alarm = described_alarm(&router, AlarmKind::Metric, name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: DescribeAlarms reports no alarm {name}"));
        assert_eq!(
            alarm.get("StateValue").and_then(Value::as_str),
            Some("ALARM"),
            "case {case:?}: the published value crosses either threshold, got {alarm}"
        );
        let document = reason_data(&alarm, case);
        let decided = verdict_datapoint(&document, case);
        // What was MEASURED over the period the datapoint stands for, whichever threshold
        // decided it: the instant of its own window and the value the statistic took over it,
        // plus the observations it aggregated where the graded series is one metric's own — and
        // no flag saying how it read, which the state the record explains already says.
        assert!(
            decided.get("timestamp").is_some()
                && decided.get("value").and_then(Value::as_f64) == Some(ABOVE_BAND),
            "case {case:?}: expected the measured period to be reported, got {decided}"
        );
        assert_eq!(
            decided.get("sampleCount").and_then(Value::as_f64),
            measured.then_some(1.0),
            "case {case:?}: expected the observations of the period to be reported only where \
             the graded value is one metric's own, got {decided}"
        );
        assert!(
            decided.get("datapoint").is_none(),
            "case {case:?}: a measured datapoint carries no breaching flag, got {decided}"
        );
        if moving {
            assert!(
                decided.get("threshold").and_then(Value::as_f64).is_some(),
                "case {case:?}: a moving threshold is reported on the datapoint it decided, got \
                 {decided}"
            );
            assert!(
                document.get("threshold").is_none(),
                "case {case:?}: an evaluation with no one threshold reports none for itself, got \
                 {document}"
            );
            continue;
        }
        assert_eq!(
            document.get("threshold").and_then(Value::as_f64),
            Some(STATIC_THRESHOLD),
            "case {case:?}: a threshold the whole evaluation shares is reported once, got \
             {document}"
        );
        assert!(
            decided.get("threshold").is_none(),
            "case {case:?}: a threshold the whole evaluation shares is not repeated on each \
             datapoint, got {decided}"
        );
    }
}
