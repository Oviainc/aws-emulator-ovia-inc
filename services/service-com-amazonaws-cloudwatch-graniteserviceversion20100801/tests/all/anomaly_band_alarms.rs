// SPDX-License-Identifier: CC0-1.0

//! The MATH-FORM metric alarm, end to end over the wire: an alarm whose threshold is a query
//! of its own `Metrics` array rather than a static number.
//!
//! Two rules are asserted here, each over a table whose rows differ along what the rule
//! generalises over rather than along a name:
//!
//! 1. The "must return a single time series" requirement constrains the query whose result the
//!    alarm WATCHES. The query named by `ThresholdMetricId` is the one the watched result is
//!    compared AGAINST ("make this value match the ID of the ANOMALY_DETECTION_BAND function"
//!    — `PutMetricAlarmInput$ThresholdMetricId`), and a band is a pair of bounds, so that query
//!    may be a multi-series expression while any other data-returning one may not. Rows differ
//!    in the band's form, in whether the multi-series query is the designated one, and in which
//!    multi-series function it calls.
//! 2. A metric alarm's notification `Trigger` reports the form the alarm carries: a query-array
//!    alarm reports its `Metrics` (and its `ThresholdMetricId` when it has one) where a
//!    single-metric alarm reports the metric members, and a member the alarm does not carry is
//!    absent rather than synthesised — so an anomaly alarm's trigger carries no static
//!    `Threshold`. Rows differ in which form the alarm was written in.
//!
//! The round trip is asserted through `DescribeAlarms`, and the notification where a client
//! sees it: on the SQS queue subscribed to the alarm's SNS topic.

use serde_json::{json, Value};

use crate::harness::{
    element, emulator, given_queue, grant_sns_delivery_on, rpc, set_alarm_state, sns,
    subscribe_queue, Emulator, REGION, TARGET_PREFIX,
};

/// The namespace every metric this suite watches lives in.
const NAMESPACE: &str = "MyApp/Bands";

/// The metric every alarm in this suite watches.
const METRIC: &str = "AnomalyMetric";

/// How many times a reader retries while it waits for an asynchronous delivery.
const DELIVERY_READS: usize = 40;

/// Call one CloudWatch operation, requiring a 200.
async fn cw(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    rpc(emulator, "monitoring", TARGET_PREFIX, operation, body).await
}

/// Call `PutMetricAlarm` and answer its HTTP status with the parsed body, so a row that
/// expects a REFUSAL can read what the service said.
async fn put_alarm(emulator: &Emulator, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("monitoring.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{TARGET_PREFIX}.PutMetricAlarm"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/monitoring/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The message a refusal carries, under either spelling the fault shapes of this service use.
fn fault_message(body: &Value) -> String {
    ["Message", "message"]
        .into_iter()
        .find_map(|member| body.get(member).and_then(Value::as_str))
        .unwrap_or_default()
        .to_string()
}

/// The query that RETRIEVES the series the band is drawn around, which every row below shares.
fn retrieval_query() -> Value {
    json!({
        "Id": "m1",
        "MetricStat": {
            "Metric": {"Namespace": NAMESPACE, "MetricName": METRIC},
            "Period": 60,
            "Stat": "Average",
        },
        "ReturnData": true,
    })
}

/// One query-array alarm: `queries` as its `Metrics`, and `threshold_metric_id` as the query
/// its watched result is compared against when the row designates one.
fn math_alarm(name: &str, queries: Value, threshold_metric_id: Option<&str>) -> Value {
    let mut request = json!({
        "AlarmName": name,
        "EvaluationPeriods": 1,
        "ComparisonOperator": "LessThanLowerOrGreaterThanUpperThreshold",
        "Metrics": queries,
    });
    if let (Some(id), Value::Object(members)) = (threshold_metric_id, &mut request) {
        members.insert("ThresholdMetricId".to_string(), Value::from(id));
    }
    request
}

/// One row of the single-series table.
struct SeriesRow {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The expression the second query of the array carries.
    expression: &'static str,
    /// Whether that query returns data (`None` leaves the member out, which defaults to true).
    return_data: Option<bool>,
    /// Whether the alarm designates that query as the one its threshold comes from.
    designated: bool,
    /// Whether `PutMetricAlarm` must accept the row.
    accepted: bool,
}

#[tokio::test]
async fn the_single_series_rule_spares_the_query_the_threshold_comes_from() {
    let emulator = emulator();
    // Rows differ in the FORM of the multi-series expression, in whether the alarm designates
    // it as its threshold query, and in which multi-series function it calls — not in a name.
    let rows = [
        SeriesRow {
            case: "a band with an explicit width, designated",
            expression: "ANOMALY_DETECTION_BAND(m1, 2)",
            return_data: Some(true),
            designated: true,
            accepted: true,
        },
        SeriesRow {
            case: "a band with a fractional width, designated",
            expression: "ANOMALY_DETECTION_BAND(m1, 0.5)",
            return_data: Some(true),
            designated: true,
            accepted: true,
        },
        SeriesRow {
            case: "a band taking the default width, designated",
            expression: "ANOMALY_DETECTION_BAND(m1)",
            return_data: Some(true),
            designated: true,
            accepted: true,
        },
        SeriesRow {
            case: "a designated band that does not return data",
            expression: "ANOMALY_DETECTION_BAND(m1, 3)",
            return_data: Some(false),
            designated: true,
            accepted: true,
        },
        SeriesRow {
            case: "a band left to the default ReturnData, designated",
            expression: "ANOMALY_DETECTION_BAND(m1, 2)",
            return_data: None,
            designated: true,
            accepted: true,
        },
        SeriesRow {
            case: "a band no ThresholdMetricId designates",
            expression: "ANOMALY_DETECTION_BAND(m1, 2)",
            return_data: Some(true),
            designated: false,
            accepted: false,
        },
        SeriesRow {
            case: "an undesignated multi-series expression over every metric of the array",
            expression: "METRICS()",
            return_data: Some(true),
            designated: false,
            accepted: false,
        },
        SeriesRow {
            case: "an undesignated expression folding the band to one series",
            expression: "MAX(ANOMALY_DETECTION_BAND(m1, 2))",
            return_data: Some(true),
            designated: false,
            accepted: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let name = format!("band-rule-{index}");
        let mut band = json!({"Id": "ad1", "Expression": row.expression});
        if let (Some(return_data), Value::Object(members)) = (row.return_data, &mut band) {
            members.insert("ReturnData".to_string(), Value::Bool(return_data));
        }
        // Exactly one query of the array is the result the alarm watches, and the query its
        // THRESHOLD comes from is not that result ("specify true for the one expression result
        // to use as the alarm", `MetricDataQuery$ReturnData`): so the retrieval query returns
        // data where the band is the designated threshold, and the band is the watched result
        // itself where nothing designates it.
        let mut retrieval = retrieval_query();
        retrieval["ReturnData"] = Value::Bool(row.designated);
        let request = math_alarm(
            &name,
            json!([retrieval, band]),
            row.designated.then_some("ad1"),
        );
        let (status, body) = put_alarm(&emulator, &request).await;
        let case = row.case;
        if row.accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected PutMetricAlarm to accept the alarm, got HTTP {status} \
                 with body {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected PutMetricAlarm to refuse the alarm with HTTP 400, got \
             HTTP {status} with body {body}"
        );
        let message = fault_message(&body);
        assert!(
            message.contains("must return a single time series"),
            "case {case:?}: expected the single-time-series refusal, got {message:?}"
        );
    }
}

#[tokio::test]
async fn an_anomaly_band_alarm_round_trips_in_its_math_form() {
    let emulator = emulator();
    // The detector the band is drawn from: the band is the DETECTOR's, so the loop the issue
    // describes starts by creating one and reading it back.
    let detector = cw(
        &emulator,
        "PutAnomalyDetector",
        &json!({
            "SingleMetricAnomalyDetector": {
                "Namespace": NAMESPACE,
                "MetricName": METRIC,
                "Stat": "Average",
            },
        }),
    )
    .await;
    let detector_id = detector["AnomalyDetectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("PutAnomalyDetector must report an id, got {detector}"))
        .to_string();
    let listed = cw(
        &emulator,
        "DescribeAnomalyDetectors",
        &json!({"Namespace": NAMESPACE, "MetricName": METRIC}),
    )
    .await;
    let detectors = listed["AnomalyDetectors"].as_array().cloned().unwrap_or_default();
    assert!(
        detectors.iter().any(|detector| {
            detector.get("AnomalyDetectorId").and_then(Value::as_str) == Some(detector_id.as_str())
        }),
        "the detector just created must be listed for its metric, got {listed}"
    );

    let name = "band-roundtrip";
    let band = json!({
        "Id": "ad1",
        "Expression": "ANOMALY_DETECTION_BAND(m1, 2)",
        "ReturnData": true,
    });
    let (status, body) = put_alarm(
        &emulator,
        &math_alarm(name, json!([retrieval_query(), band]), Some("ad1")),
    )
    .await;
    assert_eq!(
        status, 200,
        "the anomaly alarm must be accepted, got HTTP {status} with body {body}"
    );

    let described = cw(&emulator, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
    let alarm = described["MetricAlarms"][0].clone();
    assert_eq!(
        alarm.get("ThresholdMetricId").and_then(Value::as_str),
        Some("ad1"),
        "the alarm must round-trip the query its threshold comes from, got {alarm}"
    );
    assert!(
        alarm.get("Threshold").is_none(),
        "an anomaly alarm must report no static Threshold, got {alarm}"
    );
    let reported: Vec<String> = alarm["Metrics"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|query| query.get("Id").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        reported,
        vec!["m1".to_string(), "ad1".to_string()],
        "the alarm must round-trip its query array in order, got {alarm}"
    );
    let expression = alarm["Metrics"][1]
        .get("Expression")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        expression, "ANOMALY_DETECTION_BAND(m1, 2)",
        "the alarm must round-trip the band expression unchanged, got {expression:?}"
    );
}

/// One row of the trigger-form table: an alarm written in one form, and which trigger members
/// that form owes.
struct TriggerRow {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The alarm this row writes, less its name and its actions.
    request: Value,
    /// The trigger members this form must report.
    present: &'static [&'static str],
    /// The trigger members this form must NOT report.
    absent: &'static [&'static str],
}

#[tokio::test]
async fn a_notification_trigger_reports_the_form_the_alarm_was_written_in() {
    let emulator = emulator();
    let rows = [
        TriggerRow {
            case: "an anomaly-detection band alarm",
            request: math_alarm(
                "trigger-band",
                json!([
                    retrieval_query(),
                    {"Id": "ad1", "Expression": "ANOMALY_DETECTION_BAND(m1, 2)", "ReturnData": true},
                ]),
                Some("ad1"),
            ),
            present: &["Metrics", "ThresholdMetricId", "ComparisonOperator", "EvaluationPeriods"],
            absent: &["Threshold", "MetricName", "Namespace", "Statistic", "Period"],
        },
        TriggerRow {
            case: "a metric-math alarm on a static threshold",
            request: json!({
                "AlarmName": "trigger-math",
                "EvaluationPeriods": 1,
                "Threshold": 10.0,
                "ComparisonOperator": "GreaterThanThreshold",
                "Metrics": [
                    retrieval_query(),
                    {"Id": "e1", "Expression": "m1 * 2", "ReturnData": false},
                ],
            }),
            present: &["Metrics", "Threshold", "ComparisonOperator"],
            absent: &["ThresholdMetricId", "MetricName", "Namespace", "Statistic"],
        },
        TriggerRow {
            case: "a single-metric alarm on a static threshold",
            request: json!({
                "AlarmName": "trigger-static",
                "Namespace": NAMESPACE,
                "MetricName": METRIC,
                "Statistic": "Average",
                "Period": 60,
                "EvaluationPeriods": 1,
                "Threshold": 10.0,
                "ComparisonOperator": "GreaterThanThreshold",
            }),
            present: &["MetricName", "Namespace", "Statistic", "Period", "Threshold"],
            absent: &["Metrics", "ThresholdMetricId"],
        },
    ];

    for row in rows {
        let case = row.case;
        let name = row.request["AlarmName"]
            .as_str()
            .expect("every row names its alarm")
            .to_string();
        let (topic_arn, queue_url) = topic_with_subscribed_queue(&emulator, &name).await;
        let mut request = row.request.clone();
        if let Value::Object(members) = &mut request {
            members.insert("ActionsEnabled".to_string(), Value::Bool(true));
            members.insert("AlarmActions".to_string(), json!([topic_arn]));
        }
        let (status, body) = put_alarm(&emulator, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the alarm to be accepted, got HTTP {status} with body {body}"
        );

        set_alarm_state(&emulator, &name, "ALARM").await;
        let delivered = await_notifications(&emulator, &queue_url).await;
        let notification = delivered.first().unwrap_or_else(|| {
            panic!("case {case:?}: the transition must deliver a notification; none arrived")
        });
        let trigger = notification
            .get("Trigger")
            .unwrap_or_else(|| panic!("case {case:?}: the notification must carry a Trigger, got {notification}"));
        for member in row.present {
            assert!(
                trigger.get(member).is_some(),
                "case {case:?}: the Trigger must report {member}, got {trigger}"
            );
        }
        for member in row.absent {
            assert!(
                trigger.get(member).is_none(),
                "case {case:?}: the Trigger must not report {member}, got {trigger}"
            );
        }
    }
}

#[tokio::test]
async fn an_anomaly_alarms_trigger_carries_the_queries_the_alarm_was_written_with() {
    let emulator = emulator();
    let name = "trigger-queries";
    let (topic_arn, queue_url) = topic_with_subscribed_queue(&emulator, name).await;
    let mut request = math_alarm(
        name,
        json!([
            retrieval_query(),
            {
                "Id": "ad1",
                "Expression": "ANOMALY_DETECTION_BAND(m1, 2)",
                "Label": "AnomalyMetric (expected)",
                "ReturnData": true,
            },
        ]),
        Some("ad1"),
    );
    if let Value::Object(members) = &mut request {
        members.insert("ActionsEnabled".to_string(), Value::Bool(true));
        members.insert("AlarmActions".to_string(), json!([topic_arn]));
    }
    let (status, body) = put_alarm(&emulator, &request).await;
    assert_eq!(
        status, 200,
        "the anomaly alarm must be accepted, got HTTP {status} with body {body}"
    );

    set_alarm_state(&emulator, name, "ALARM").await;
    let delivered = await_notifications(&emulator, &queue_url).await;
    let notification = delivered
        .first()
        .expect("the transition must deliver a notification");
    let queries = notification["Trigger"]["Metrics"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let ids: Vec<&str> = queries
        .iter()
        .filter_map(|query| query.get("Id").and_then(Value::as_str))
        .collect();
    assert_eq!(
        ids,
        vec!["m1", "ad1"],
        "the Trigger must carry the alarm's queries in order, got {notification}"
    );
    assert_eq!(
        queries[1].get("Expression").and_then(Value::as_str),
        Some("ANOMALY_DETECTION_BAND(m1, 2)"),
        "the Trigger must carry the band expression the alarm watches against, got {notification}"
    );
    assert_eq!(
        queries[1].get("Label").and_then(Value::as_str),
        Some("AnomalyMetric (expected)"),
        "the Trigger must carry every member the query was written with, got {notification}"
    );
    let retrieved = &queries[0]["MetricStat"];
    assert_eq!(
        retrieved["Metric"].get("Namespace").and_then(Value::as_str),
        Some(NAMESPACE),
        "the Trigger's retrieval query must name the metric's namespace, got {notification}"
    );
    assert_eq!(
        retrieved["Metric"].get("MetricName").and_then(Value::as_str),
        Some(METRIC),
        "the Trigger's retrieval query must name the metric, got {notification}"
    );
    assert_eq!(
        retrieved.get("Stat").and_then(Value::as_str),
        Some("Average"),
        "the Trigger's retrieval query must name the statistic it folds by, got {notification}"
    );
    assert_eq!(
        retrieved.get("Period").and_then(Value::as_i64),
        Some(60),
        "the Trigger's retrieval query must name the period it retrieves at, got {notification}"
    );
    assert!(
        notification["Trigger"].get("Threshold").is_none(),
        "an anomaly alarm's Trigger must carry no static Threshold, got {notification}"
    );
}

/// A queue subscribed to a fresh topic named `name`: the path an alarm notification travels.
/// Answers the topic ARN and the queue URL.
async fn topic_with_subscribed_queue(emulator: &Emulator, name: &str) -> (String, String) {
    let (queue_url, queue_arn) = given_queue(emulator, name).await;
    grant_sns_delivery_on(emulator, &queue_url, &queue_arn).await;
    let topic_arn = element(
        &sns(emulator, "CreateTopic", &[("Name", name)]).await,
        "TopicArn",
    );
    subscribe_queue(emulator, &topic_arn, &queue_arn).await;
    (topic_arn, queue_url)
}

/// Read the queue until it has delivered a notification, and answer every one it delivered.
///
/// Delivery is asynchronous: the call that materialises a transition publishes what it owes,
/// so a reader waits for the notification rather than assuming it is already there.
async fn await_notifications(emulator: &Emulator, queue_url: &str) -> Vec<Value> {
    let mut delivered: Vec<Value> = Vec::new();
    for _ in 0..DELIVERY_READS {
        let received = rpc(
            emulator,
            "sqs",
            "AmazonSQS",
            "ReceiveMessage",
            &json!({"QueueUrl": queue_url, "MaxNumberOfMessages": 10}),
        )
        .await;
        delivered.extend(
            received["Messages"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|message| {
                    let envelope: Value = serde_json::from_str(
                        message.get("Body").and_then(Value::as_str).unwrap_or_default(),
                    )
                    .expect("the delivered body is the SNS notification envelope");
                    serde_json::from_str(envelope["Message"].as_str().unwrap_or_default())
                        .expect("the alarm notification is JSON")
                }),
        );
        if !delivered.is_empty() {
            break;
        }
        ::tokio::time::sleep(::std::time::Duration::from_millis(100)).await;
    }
    delivered
}
