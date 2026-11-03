// SPDX-License-Identifier: CC0-1.0

//! Which query of a `Metrics` array an alarm WATCHES, and what happens when the array does not
//! designate exactly one.
//!
//! The rule under test: **a `Metrics` array is accepted only when EXACTLY ONE of its queries is
//! the result the alarm grades** — "When used in `PutMetricAlarm`, specify true for the one
//! expression result to use as the alarm. For all other metrics and expressions in the same
//! `PutMetricAlarm` operation, specify `ReturnData` as False." … "If you omit this, the default
//! of true is used." (`smithy_query(query="doc",shape="MetricDataQuery",member="ReturnData")`),
//! and the query the alarm's THRESHOLD comes from is not that result ("If this is an alarm based
//! on an anomaly detection model, make this value match the ID of the ANOMALY_DETECTION_BAND
//! function.", `smithy_query(query="doc",shape="PutMetricAlarmInput",member="ThresholdMetricId")`).
//! An array designating none gives the alarm nothing to grade, and one designating several gives
//! it no single result to grade; both are refused with HTTP 400 and nothing is stored.
//!
//! So the rows below differ along the COUNT of designations and along how each designation is
//! stated — explicitly, by omission, or by the member the threshold comes from — and along the
//! size and the composition of the array (a retrieval alone, a retrieval feeding an expression,
//! an expression chain, a band beside the series it is drawn around). The name of the alarm and
//! the metric are incidental to the rule.
//!
//! The second test is the same refusal read as a query-compatible SDK reads it: this service
//! carries the `awsQueryCompatible` modifier, so a refusal names its class in the body's
//! qualified `__type` and in the `x-amzn-query-error` header.

use serde_json::{json, Value};

use crate::harness::{
    build_router, call, call_in_query_mode, call_with_headers, described_alarm, error_message,
    AlarmKind,
};

/// The namespace and metric every query below retrieves: one series, so no row's outcome depends
/// on what is published.
const NAMESPACE: &str = "Custom/Designation";
const METRIC: &str = "Latency";

/// The sentence CloudWatch refuses an array that designates other than one query with.
const DESIGNATION_REFUSAL: &str = "Exactly one element of the metrics list should return data.";

/// A `MetricStat` query under `id` retrieving one series, carrying `return_data` when the row
/// states one and leaving the member out when it does not.
fn stat_query(id: &str, return_data: Option<bool>) -> Value {
    let mut query = json!({
        "Id": id,
        "MetricStat": {
            "Metric": {"Namespace": NAMESPACE, "MetricName": METRIC},
            "Period": 60,
            "Stat": "Average",
        },
    });
    with_return_data(&mut query, return_data);
    query
}

/// An expression query under `id`, carrying `return_data` the same way.
fn expression_query(id: &str, expression: &str, return_data: Option<bool>) -> Value {
    let mut query = json!({"Id": id, "Expression": expression});
    with_return_data(&mut query, return_data);
    query
}

/// Set `ReturnData` on `query` when the row states it, and leave the member out otherwise — an
/// omitted member is a designation of its own, so a row that tests the default must not carry it.
fn with_return_data(query: &mut Value, return_data: Option<bool>) {
    if let (Some(return_data), Value::Object(members)) = (return_data, query) {
        members.insert("ReturnData".to_string(), Value::Bool(return_data));
    }
}

/// A `PutMetricAlarm` request watching `queries`, compared against a static threshold.
fn math_alarm(name: &str, queries: Value) -> Value {
    json!({
        "AlarmName": name,
        "Metrics": queries,
        "EvaluationPeriods": 1,
        "Threshold": 10.0,
        "ComparisonOperator": "GreaterThanThreshold",
    })
}

/// The same request comparing the watched result against the band `threshold_metric_id`
/// computes, which is what an anomaly-detection alarm carries instead of a static threshold.
fn band_alarm(name: &str, queries: Value, threshold_metric_id: &str) -> Value {
    json!({
        "AlarmName": name,
        "Metrics": queries,
        "EvaluationPeriods": 1,
        "ThresholdMetricId": threshold_metric_id,
        "ComparisonOperator": "LessThanLowerOrGreaterThanUpperThreshold",
    })
}

/// One row: an array, and whether the alarm it configures is one CloudWatch creates.
struct Row {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The request, less its name — filled in per row below.
    request: Value,
    /// Whether `PutMetricAlarm` must accept it.
    accepted: bool,
}

/// `request` with `name` as the alarm's name.
fn named(request: &Value, name: &str) -> Value {
    let mut request = request.clone();
    if let Value::Object(members) = &mut request {
        members.insert("AlarmName".to_string(), Value::from(name));
    }
    request
}

#[tokio::test]
async fn an_alarm_is_created_only_when_one_query_of_its_array_returns_data() {
    let rows = [
        Row {
            case: "one retrieval, left to the default designation",
            request: math_alarm("", json!([stat_query("m1", None)])),
            accepted: true,
        },
        Row {
            case: "one retrieval, designated explicitly",
            request: math_alarm("", json!([stat_query("m1", Some(true))])),
            accepted: true,
        },
        Row {
            case: "one retrieval, designating nothing",
            request: math_alarm("", json!([stat_query("m1", Some(false))])),
            accepted: false,
        },
        Row {
            case: "a retrieval feeding the expression the alarm watches",
            request: math_alarm(
                "",
                json!([
                    stat_query("m1", Some(false)),
                    expression_query("e1", "m1 * 2", Some(true)),
                ]),
            ),
            accepted: true,
        },
        Row {
            case: "the watched expression left to the default designation",
            request: math_alarm(
                "",
                json!([
                    stat_query("m1", Some(false)),
                    expression_query("e1", "m1 * 2", None),
                ]),
            ),
            accepted: true,
        },
        Row {
            case: "two queries both designated explicitly",
            request: math_alarm(
                "",
                json!([
                    stat_query("m1", Some(true)),
                    expression_query("e1", "m1 * 2", Some(true)),
                ]),
            ),
            accepted: false,
        },
        Row {
            case: "two queries both left to the default designation",
            request: math_alarm(
                "",
                json!([stat_query("m1", None), expression_query("e1", "m1 * 2", None)]),
            ),
            accepted: false,
        },
        Row {
            case: "a chain of three queries designating its last",
            request: math_alarm(
                "",
                json!([
                    stat_query("m1", Some(false)),
                    expression_query("e1", "m1 * 2", Some(false)),
                    expression_query("e2", "e1 + 1", Some(true)),
                ]),
            ),
            accepted: true,
        },
        Row {
            case: "a chain of three queries designating two of them",
            request: math_alarm(
                "",
                json!([
                    stat_query("m1", Some(false)),
                    expression_query("e1", "m1 * 2", Some(true)),
                    expression_query("e2", "e1 + 1", Some(true)),
                ]),
            ),
            accepted: false,
        },
        Row {
            case: "three queries designating none of them",
            request: math_alarm(
                "",
                json!([
                    stat_query("m1", Some(false)),
                    expression_query("e1", "m1 * 2", Some(false)),
                    expression_query("e2", "e1 + 1", Some(false)),
                ]),
            ),
            accepted: false,
        },
        Row {
            case: "a band the threshold comes from beside the series it is drawn around",
            request: band_alarm(
                "",
                json!([
                    stat_query("m1", Some(true)),
                    expression_query("ad1", "ANOMALY_DETECTION_BAND(m1, 2)", Some(true)),
                ]),
                "ad1",
            ),
            accepted: true,
        },
        Row {
            case: "a band the threshold comes from, with no query left to watch",
            request: band_alarm(
                "",
                json!([
                    stat_query("m1", Some(false)),
                    expression_query("ad1", "ANOMALY_DETECTION_BAND(m1, 2)", Some(true)),
                ]),
                "ad1",
            ),
            accepted: false,
        },
        Row {
            case: "two watched queries beside the band the threshold comes from",
            request: band_alarm(
                "",
                json!([
                    stat_query("m1", Some(true)),
                    expression_query("e1", "m1 * 2", Some(true)),
                    expression_query("ad1", "ANOMALY_DETECTION_BAND(m1, 2)", Some(true)),
                ]),
                "ad1",
            ),
            accepted: false,
        },
    ];

    let router = build_router();
    for (index, row) in rows.iter().enumerate() {
        let name = format!("designation-{index}");
        let case = row.case;
        let (status, body) = call(&router, "PutMetricAlarm", &named(&row.request, &name)).await;
        let stored = described_alarm(&router, AlarmKind::Metric, &name).await;
        if row.accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the alarm to be created, got HTTP {status} with {body}"
            );
            // Round trip: an accepted array is the one the reader reports back.
            let reported: Vec<String> = stored
                .as_ref()
                .and_then(|alarm| alarm.get("Metrics"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|query| query.get("Id").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect();
            let asked: Vec<String> = row.request["Metrics"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|query| query.get("Id").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect();
            assert_eq!(
                reported, asked,
                "case {case:?}: expected the stored alarm to report the array it was given, \
                 got {stored:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the array to be refused with HTTP 400, got HTTP {status} \
             with {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, DESIGNATION_REFUSAL,
            "case {case:?}: expected the refusal to read {DESIGNATION_REFUSAL:?}, got \
             {message:?}"
        );
        assert_eq!(
            stored, None,
            "case {case:?}: a refused configuration must store nothing, got {stored:?}"
        );
    }
}

/// The header a query-compatible client reads the legacy error code out of.
const QUERY_ERROR_HEADER: &str = "x-amzn-query-error";

/// The shape id the body names the request-validation class under.
const VALIDATION_SHAPE_ID: &str = "com.amazon.coral.validate#ValidationException";

/// The legacy code and fault side that class is reported under.
const VALIDATION_REPORT: &str = "ValidationError;Sender";

#[tokio::test]
async fn the_designation_refusal_names_its_class_in_the_body_and_in_the_legacy_header() {
    // The rows are the two counts the rule refuses — none designated and several designated —
    // because the envelope belongs to the error CLASS and not to any one count.
    let rows: [(&str, Value); 2] = [
        (
            "an array designating no query",
            math_alarm(
                "envelope-none",
                json!([
                    stat_query("m1", Some(false)),
                    expression_query("e1", "m1 * 2", Some(false)),
                ]),
            ),
        ),
        (
            "an array designating two queries",
            math_alarm(
                "envelope-two",
                json!([
                    stat_query("m1", Some(true)),
                    expression_query("e1", "m1 * 2", Some(true)),
                ]),
            ),
        ),
    ];

    let router = build_router();
    for (case, request) in rows {
        // 1. A query-compatible SDK reads the class twice: qualified in the body, legacy in the
        //    header.
        let (status, headers, body) = call_in_query_mode(&router, "PutMetricAlarm", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        let shape_id = body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            shape_id, VALIDATION_SHAPE_ID,
            "case {case:?}: expected the body to name {VALIDATION_SHAPE_ID}, got {body}"
        );
        let reported = headers
            .get(QUERY_ERROR_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert_eq!(
            reported, VALIDATION_REPORT,
            "case {case:?}: expected {QUERY_ERROR_HEADER} to read {VALIDATION_REPORT:?}, got \
             {reported:?}"
        );

        // 2. A native JSON client gets the same body and no report: the legacy code is an
        //    addition for a client that asked for it.
        let (plain_status, plain_headers, plain_body) =
            call_with_headers(&router, "PutMetricAlarm", &request).await;
        assert_eq!(
            (
                plain_status,
                plain_body
                    .get("__type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            ),
            (400, VALIDATION_SHAPE_ID),
            "case {case:?}: a native JSON client must get the same answer, got {plain_status} \
             with {plain_body}"
        );
        assert!(
            plain_headers.get(QUERY_ERROR_HEADER).is_none(),
            "case {case:?}: a client that did not opt in must get no report, got {:?}",
            plain_headers.get(QUERY_ERROR_HEADER)
        );
    }
}
