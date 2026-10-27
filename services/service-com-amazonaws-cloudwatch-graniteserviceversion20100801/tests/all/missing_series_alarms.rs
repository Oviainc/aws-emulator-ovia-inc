// SPDX-License-Identifier: CC0-1.0

//! The TWO ARMS that tell MISSING DATA from a BREACH: a metric alarm over a series nothing has
//! published into, beside an identically shaped alarm over a series that is published into.
//!
//! One rule is asserted here, over a table whose rows differ along what the rule generalises
//! over rather than along a name:
//!
//! A metric alarm's state is computed from the datapoints stored for the ONE series its
//! `Namespace`, `MetricName` and `Dimensions` name
//! (`smithy_query(query="operation",operation="PutMetricAlarm")`), so an alarm whose dimension
//! values name a resource nothing publishes for finds no datapoint, substitutes none under the
//! documented default treatment (`missing`) and therefore settles in `INSUFFICIENT_DATA` and
//! never in `ALARM`, while the SAME alarm shape over a series fed breaching datapoints is
//! evaluated into `ALARM` with a `StateReason` that explains the crossing. The two arms
//! together are what shows a hole in a series is handled as a hole rather than as a breach.
//! A third alarm over the empty series, whose missing data is treated as breaching, is the
//! witness that the empty series IS evaluated: the insufficient data of the first arm is the
//! decision the default treatment took over it, not an alarm nothing ever looked at.
//!
//! The rows differ in the namespace, the metric, the shape of the dimension set that keys the
//! series (a cluster/service pair, one dimension, three dimensions, none at all), the
//! statistic, the comparison and the sign of the threshold, the period, how many periods are
//! evaluated and how many datapoints the control arm publishes, and whether the missing-data
//! treatment is left to its default or spelled out. The recorded case the issue was filed
//! about — a Container Insights metric keyed by a cluster/service pair that does not exist,
//! beside a control fed through `PutMetricData` — is the first row.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, merged, now_secs, ok, PAST_EVALUATION_DELAY};

/// One row of the table: the alarm shape EVERY arm of the row is configured with, and the
/// datapoints the control arm's series is fed.
struct Row {
    /// What this row is about, named by every assertion it makes.
    case: &'static str,
    /// The namespace every arm watches.
    namespace: &'static str,
    /// The metric every arm watches.
    metric: &'static str,
    /// The dimension NAMES that key the series an arm watches; each arm fills them with values
    /// of its own ([`series`]).
    dimension_names: &'static [&'static str],
    /// The members this row changes on the base alarm its arms are created from.
    patch: Value,
    /// The datapoints the CONTROL arm publishes into its own series: each a value and how many
    /// seconds before the publish its time stamp lies.
    data: &'static [(f64, f64)],
}

/// The series ONE arm of a row watches: the metric name and the dimension set it is stored
/// under.
///
/// Each arm has to watch its OWN series, or the control arm's datapoints would decide the
/// no-data arm too — so the arm's name goes into the members that identify the series: the
/// dimension values where the row keys its series by dimensions, and the metric name where it
/// keys it by none (an alarm with no dimensions watches the series published without any).
fn series(row: &Row, arm: &str) -> (String, Value) {
    if row.dimension_names.is_empty() {
        return (format!("{}-{arm}", row.metric), json!([]));
    }
    let dimensions: Vec<Value> = row
        .dimension_names
        .iter()
        .map(|name| json!({"Name": name, "Value": format!("{arm}-{name}")}))
        .collect();
    (row.metric.to_string(), Value::Array(dimensions))
}

/// The alarm `name` watching the series `arm` names: one 60-second period, one evaluation
/// period, a static threshold of 10 and a `GreaterThanThreshold` comparison, with the row's own
/// members merged in.
fn alarm_request(row: &Row, name: &str, arm: &str) -> Value {
    let (metric, dimensions) = series(row, arm);
    merged(
        json!({
            "AlarmName": name,
            "Namespace": row.namespace,
            "MetricName": metric,
            "Dimensions": dimensions,
            "Statistic": "Average",
            "Period": 60,
            "EvaluationPeriods": 1,
            "Threshold": 10.0,
            "ComparisonOperator": "GreaterThanThreshold",
        }),
        &row.patch,
    )
}

/// Publish this row's datapoints into the series the control arm named `arm` watches.
///
/// Stored at high resolution ("Setting this to 1 specifies this metric as a high-resolution
/// metric", `MetricDatum$StorageResolution`), so a row evaluating a sub-minute period reads the
/// windows its datapoints were really stamped in; a row on 60-second periods reads the same
/// windows either way.
async fn publish(router: &MultiServiceRouter, row: &Row, arm: &str) {
    let (metric, dimensions) = series(row, arm);
    let now = now_secs();
    let data: Vec<Value> = row
        .data
        .iter()
        .map(|(value, age_secs)| {
            json!({
                "MetricName": metric,
                "Dimensions": dimensions,
                "Value": value,
                "StorageResolution": 1,
                "Timestamp": now - age_secs,
            })
        })
        .collect();
    ok(
        router,
        "PutMetricData",
        &json!({"Namespace": row.namespace, "MetricData": data}),
    )
    .await;
}

/// Every alarm `DescribeAlarms` reports for `names`, by name.
async fn described(router: &MultiServiceRouter, names: &[String]) -> BTreeMap<String, Value> {
    let body = ok(router, "DescribeAlarms", &json!({"AlarmNames": names})).await;
    body.get("MetricAlarms")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|alarm| {
            alarm
                .get("AlarmName")
                .and_then(Value::as_str)
                .map(|name| (name.to_string(), alarm.clone()))
        })
        .collect()
}

/// The `StateValue` of the alarm `name`, as a client reads it off `DescribeAlarms`.
fn state_of(alarms: &BTreeMap<String, Value>, name: &str) -> String {
    alarms
        .get(name)
        .and_then(|alarm| alarm.get("StateValue"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `StateReason` of the alarm `name`.
fn reason_of(alarms: &BTreeMap<String, Value>, name: &str) -> String {
    alarms
        .get(name)
        .and_then(|alarm| alarm.get("StateReason"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The three alarms one row drives, each over the series its own arm names.
struct Arms<'row> {
    /// The row they were all configured from.
    row: &'row Row,
    /// The arm whose dimension values name a resource nothing publishes for.
    no_data: String,
    /// The arm whose series is fed breaching datapoints.
    control: String,
    /// The arm over the no-data arm's series whose missing data is treated as breaching.
    witness: String,
}

/// How many times the poll below reads the alarms before giving up on the control arms.
///
/// A client waiting for an evaluated state polls; the first evaluation is owed a span after the
/// write that configured the alarm
/// (`crate::state::alarm_evaluation::EVALUATION_DELAY_SECS`), so this many reads one third of
/// that span apart leave room for several of those spans without waiting on wall clock the
/// emulator does not need.
const POLL_READS: usize = 24;

/// The state an alarm with no datapoint of its own settles in, and the one it must never
/// report.
const INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// The state the control arm's breaching datapoints must put it in.
const ALARM: &str = "ALARM";

/// The opening of the `StateReason` an evaluated crossing explains itself with.
const CROSSED: &str = "Threshold Crossed";

#[tokio::test]
async fn a_series_nothing_publishes_holds_insufficient_data_while_its_control_arm_fires() {
    let rows: &[Row] = &[
        Row {
            case: "a Container Insights metric keyed by a cluster and a service",
            namespace: "ECS/ContainerInsights",
            metric: "CpuUtilized",
            dimension_names: &["ClusterName", "ServiceName"],
            patch: json!({}),
            data: &[(42.0, 0.0)],
        },
        Row {
            case: "the same namespace keyed by the cluster alone, at a threshold of 1",
            namespace: "ECS/ContainerInsights",
            metric: "RunningTaskCount",
            dimension_names: &["ClusterName"],
            patch: json!({
                "ComparisonOperator": "GreaterThanOrEqualToThreshold",
                "Threshold": 1.0,
            }),
            data: &[(3.0, 0.0)],
        },
        Row {
            case: "a series keyed by three dimensions, two periods of which both must breach",
            namespace: "ECS/ContainerInsights",
            metric: "MemoryUtilized",
            dimension_names: &["ClusterName", "ServiceName", "TaskDefinitionFamily"],
            patch: json!({"EvaluationPeriods": 2, "DatapointsToAlarm": 2}),
            data: &[(42.0, 0.0), (43.0, 70.0)],
        },
        Row {
            case: "a series with no dimensions at all",
            namespace: "ECS/ContainerInsights",
            metric: "ContainerInstanceCount",
            dimension_names: &[],
            patch: json!({}),
            data: &[(11.0, 0.0)],
        },
        Row {
            case: "the Sum of two datapoints of one period, in a namespace of the caller's own",
            namespace: "Custom/Container Insights",
            metric: "CpuReserved",
            dimension_names: &["ClusterName", "ServiceName"],
            patch: json!({"Statistic": "Sum"}),
            data: &[(4.0, 0.0), (8.0, 0.0)],
        },
        Row {
            case: "a LessThanThreshold comparison against a negative threshold",
            namespace: "ECS/ContainerInsights",
            metric: "StorageReadBytes",
            dimension_names: &["ClusterName", "ServiceName"],
            patch: json!({"ComparisonOperator": "LessThanThreshold", "Threshold": -1.0}),
            data: &[(-7.25, 0.0)],
        },
        Row {
            case: "the shortest period an alarm may be evaluated on",
            namespace: "ECS/ContainerInsights",
            metric: "NetworkRxBytes",
            dimension_names: &["ClusterName", "ServiceName"],
            // "Valid values are 10, 20, 30, and any multiple of 60."
            // (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="Period")`).
            patch: json!({"Period": 10}),
            data: &[(42.0, 0.0)],
        },
        Row {
            case: "the default missing-data treatment spelled out",
            namespace: "ECS/ContainerInsights",
            metric: "EphemeralStorageUtilized",
            dimension_names: &["ClusterName", "ServiceName"],
            // "If TreatMissingData is omitted, the default behavior of missing is used."
            // (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="TreatMissingData")`).
            patch: json!({"TreatMissingData": "missing"}),
            data: &[(42.0, 0.0)],
        },
    ];

    // Every row watches its own series, so all the rows are driven on ONE emulator and the
    // evaluation the clock owes is waited out once rather than per row.
    let router = build_router();
    let mut arms: Vec<Arms> = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let no_data = format!("no-data-{index}");
        let control = format!("control-{index}");
        let witness = format!("witness-{index}");
        ok(
            &router,
            "PutMetricAlarm",
            &alarm_request(row, &no_data, &no_data),
        )
        .await;
        ok(
            &router,
            "PutMetricAlarm",
            &alarm_request(row, &control, &control),
        )
        .await;
        // A third alarm over the SAME series the no-data arm watches, whose missing data is
        // treated as breaching: it is what shows the evaluation really reads that empty series
        // rather than leaving the alarm untouched, so the no-data arm's insufficient data is a
        // decision the default treatment took and not an alarm nothing evaluated.
        ok(
            &router,
            "PutMetricAlarm",
            &merged(
                alarm_request(row, &witness, &no_data),
                &json!({"TreatMissingData": "breaching"}),
            ),
        )
        .await;
        // Only the control arm's series is published into: the other arms' dimension values
        // name a resource nothing ever writes a datapoint for.
        publish(&router, row, &control).await;
        arms.push(Arms {
            row,
            no_data,
            control,
            witness,
        });
    }

    let names: Vec<String> = arms
        .iter()
        .flat_map(|arms| [arms.no_data.clone(), arms.control.clone(), arms.witness.clone()])
        .collect();

    // A client polls for the evaluated state. Every read checks the no-data arm — it must
    // report insufficient data on every one of them, since a hole may never be read as a
    // breach, whatever the other arms are doing — and the arms that do reach ALARM are waited
    // for.
    let mut last = BTreeMap::new();
    for read in 0..POLL_READS {
        std::thread::sleep(PAST_EVALUATION_DELAY / 3);
        last = described(&router, &names).await;
        for arms in &arms {
            let case = arms.row.case;
            let state = state_of(&last, &arms.no_data);
            assert_eq!(
                state, INSUFFICIENT_DATA,
                "case {case:?}: on read {read} the alarm over a series nothing publishes must \
                 report {INSUFFICIENT_DATA}, got {state:?}"
            );
        }
        if arms.iter().all(|arms| {
            state_of(&last, &arms.control) == ALARM && state_of(&last, &arms.witness) == ALARM
        }) {
            break;
        }
    }

    for arms in &arms {
        let case = arms.row.case;
        let control_state = state_of(&last, &arms.control);
        assert_eq!(
            control_state, ALARM,
            "case {case:?}: the control arm's breaching datapoints must put it in {ALARM}, got \
             {control_state:?} with {:?}",
            last.get(&arms.control)
        );
        let control_reason = reason_of(&last, &arms.control);
        assert!(
            control_reason.starts_with(CROSSED),
            "case {case:?}: expected the control arm's StateReason to open with {CROSSED:?}, \
             got {control_reason:?}"
        );
        let witness_state = state_of(&last, &arms.witness);
        assert_eq!(
            witness_state, ALARM,
            "case {case:?}: the alarm whose missing data is breaching must be evaluated into \
             {ALARM} over the same empty series, got {witness_state:?} with {:?}",
            last.get(&arms.witness)
        );
        let no_data_state = state_of(&last, &arms.no_data);
        assert_eq!(
            no_data_state, INSUFFICIENT_DATA,
            "case {case:?}: once the control arm has fired, the arm over a series nothing \
             publishes must still report {INSUFFICIENT_DATA}, got {no_data_state:?}"
        );
        // The no-data arm's explanation may not count datapoints it never had: an evaluation
        // with nothing to grade reports the hole, not a crossing.
        let no_data_reason = reason_of(&last, &arms.no_data);
        assert!(
            !no_data_reason.contains("out of the last"),
            "case {case:?}: expected the no-data arm's StateReason not to count graded \
             datapoints, got {no_data_reason:?}"
        );
    }
}
