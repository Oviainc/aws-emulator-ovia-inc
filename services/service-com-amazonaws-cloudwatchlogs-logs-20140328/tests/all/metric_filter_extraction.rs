// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ingest-time METRIC FILTER extraction: what `PutLogEvents`
//! publishes into the CloudWatch metric store for the filters a log group holds.
//!
//! The rule under test: **every log event this service stores is run against each stored
//! metric filter's `filterPattern`, and each `metricTransformations` entry of a filter that
//! matches publishes its `metricValue` into `metricNamespace`/`metricName` with the declared
//! `unit`, under one dimension per `dimensions` entry whose value is what that entry's
//! selector resolves to IN THAT EVENT, at the event's own timestamp — while an event the
//! pattern does not match publishes the transformation's `defaultValue` where it declares one
//! and nothing where it does not.**
//!
//! Every write goes over the wire — `POST /` with `X-Amz-Target: Logs_20140328.<Operation>`
//! through the router that mounts this service — so a datapoint that is not extracted from
//! the stored events cannot appear. What the datapoints are READ from is the CloudWatch
//! emulator's own metric store on the shared engine, which is where they have to land for
//! `GetMetricStatistics` to answer with them; the cross-service read path is asserted through
//! CloudWatch itself in `core/router/tests/all/logs_metric_filter_extraction.rs`.
//!
//! The rows differ along what the rule generalises over: the form of the filter pattern (the
//! empty pattern, a JSON pattern, a space-delimited one, an unstructured term), the form of
//! the metric value (a literal number, a JSON selector, a space-delimited field), how many
//! dimensions the transformation declares and from which selectors, whether a unit and a
//! default value are declared, and how many of the ingested events match. Any single filter
//! an issue happens to mention is one row.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

use crate::harness::{base_ms, group_with_stream, Logs};

/// The `StandardUnit` value a datapoint carries when its publisher declared none. Source:
/// `MetricTransformation$unit` — "The unit to assign to the metric. If you omit this, the
/// unit is set as `None`" — spelled as the model's `StandardUnit` enum spells it.
const UNIT_NONE: &str = "None";

/// The dimension set a series is published under, as a row states it: `(name, value)` pairs.
type Dimensions = &'static [(&'static str, &'static str)];

/// One series a row expects: the dimension set, and the values published under it.
type Series = (Dimensions, &'static [f64]);

/// One row: the filter to store, the events to ingest, and the series the ingest must publish.
struct Row {
    /// What the row varies, named in every assertion.
    case: &'static str,
    /// The filter's `filterPattern`.
    pattern: &'static str,
    /// The transformation's `metricValue`.
    metric_value: &'static str,
    /// The transformation's `dimensions`, as `(name, selector)` pairs.
    dimensions: Dimensions,
    /// The transformation's `unit`, when the row declares one.
    unit: Option<&'static str>,
    /// The transformation's `defaultValue`, when the row declares one.
    default_value: Option<f64>,
    /// The messages ingested, in one batch.
    messages: &'static [&'static str],
    /// The series the ingest must publish: the dimension set, and the values in it.
    expected: &'static [Series],
}

/// The `dimensions` map of a transformation, from `(name, selector)` pairs.
fn transformation_dimensions(pairs: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (name, selector) in pairs {
        map.insert((*name).to_string(), json!(selector));
    }
    Value::Object(map)
}

/// A dimension set as the metric store keys one.
fn dimension_set(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// One published datapoint, as the CloudWatch metric store holds it.
#[derive(Debug, Clone, PartialEq)]
struct Published {
    /// The dimension set the value was published under.
    dimensions: BTreeMap<String, String>,
    /// The value itself.
    value: f64,
    /// The unit it was published with, or [`UNIT_NONE`] where none was declared.
    unit: String,
    /// The instant it was published at, in epoch seconds.
    timestamp_secs: f64,
}

/// Every datapoint the CloudWatch metric store holds for one metric, in dimension-set order.
///
/// This reads the SAME store `PutMetricData` writes and `GetMetricStatistics` reads, through
/// the shared engine both services are mounted on.
fn published(logs: &Logs, namespace: &str, metric_name: &str) -> Vec<Published> {
    let state = cloudwatch::state::service_state(&**logs.engine())
        .expect("the CloudWatch metric store is mounted on the shared engine");
    let store = state.lock();
    let mut datapoints: Vec<Published> = store
        .metrics
        .iter()
        .filter(|(key, _)| key.namespace == namespace && key.metric_name == metric_name)
        .flat_map(|(key, data)| {
            data.iter().map(|datum| Published {
                dimensions: key.dimensions.clone(),
                value: datum.sum,
                unit: datum.unit().to_string(),
                timestamp_secs: datum.timestamp_secs,
            })
        })
        .collect();
    datapoints.sort_by(|left, right| {
        left.dimensions
            .cmp(&right.dimensions)
            .then(left.value.total_cmp(&right.value))
    });
    datapoints
}

/// Ingest one message as a single log event stamped at `timestamp_ms`.
async fn ingest_one(logs: &Logs, group: &str, timestamp_ms: i64, message: &str) {
    logs.ok(
        "PutLogEvents",
        &json!({
            "logGroupName": group,
            "logStreamName": "events",
            "logEvents": [{"timestamp": timestamp_ms, "message": message}],
        }),
    )
    .await;
}

/// Every form of filter pattern, metric value and dimension set publishes exactly the
/// datapoints the stored filter declares, at the ingested event's own instant.
#[tokio::test]
async fn an_ingested_event_publishes_the_metric_data_the_groups_filters_extract() {
    let logs = Logs::new();
    let at = base_ms();

    let cases: Vec<Row> = vec![
        Row {
            case: "a JSON pattern counting matching events under one dimension",
            pattern: r#"{ $.status = "fail" }"#,
            metric_value: "1",
            dimensions: &[("Service", "$.service")],
            unit: None,
            default_value: None,
            messages: &[
                r#"{"service":"checkout","status":"fail","n":1}"#,
                r#"{"service":"checkout","status":"fail","n":2}"#,
                r#"{"service":"search","status":"fail","n":3}"#,
                r#"{"service":"checkout","status":"ok","n":4}"#,
            ],
            // Each distinct value the selector resolves to is its own series.
            expected: &[
                (&[("Service", "checkout")], &[1.0, 1.0]),
                (&[("Service", "search")], &[1.0]),
            ],
        },
        Row {
            case: "a JSON selector as the metric value, with a unit",
            pattern: r#"{ $.level = "error" }"#,
            metric_value: "$.bytes",
            dimensions: &[],
            unit: Some("Bytes"),
            default_value: None,
            messages: &[
                r#"{"level":"error","bytes":100}"#,
                r#"{"level":"error","bytes":250.5}"#,
                r#"{"level":"info","bytes":9000}"#,
            ],
            expected: &[(&[], &[100.0, 250.5])],
        },
        Row {
            case: "two dimensions resolved from the same event",
            pattern: r#"{ $.status = "fail" }"#,
            metric_value: "1",
            dimensions: &[("Service", "$.service"), ("Region", "$.region")],
            unit: None,
            default_value: None,
            messages: &[
                r#"{"service":"api","region":"eu-west-2","status":"fail"}"#,
                r#"{"service":"api","region":"us-east-1","status":"fail"}"#,
            ],
            expected: &[
                (&[("Region", "eu-west-2"), ("Service", "api")], &[1.0]),
                (&[("Region", "us-east-1"), ("Service", "api")], &[1.0]),
            ],
        },
        Row {
            case: "a space-delimited pattern, field value and field dimension",
            pattern: "[ip, status_code, size]",
            metric_value: "$size",
            dimensions: &[("StatusCode", "$status_code")],
            unit: Some("Bytes"),
            default_value: None,
            messages: &[
                "10.0.0.1 200 512",
                "10.0.0.2 200 256",
                "10.0.0.3 404 64",
                // Three fields are declared, so a two-field event matches no row of the list.
                "10.0.0.4 404",
            ],
            expected: &[
                (&[("StatusCode", "200")], &[256.0, 512.0]),
                (&[("StatusCode", "404")], &[64.0]),
            ],
        },
        Row {
            case: "an unstructured term pattern with a JSON dimension selector",
            pattern: "ERROR",
            metric_value: "1",
            dimensions: &[("Service", "$.service")],
            unit: Some("Count"),
            default_value: None,
            messages: &[
                r#"{"service":"billing","msg":"ERROR boom"}"#,
                r#"{"service":"billing","msg":"INFO fine"}"#,
            ],
            expected: &[(&[("Service", "billing")], &[1.0])],
        },
        Row {
            case: "a positional field of a space-delimited event as the dimension",
            pattern: "[, status_code, ]",
            metric_value: "1",
            dimensions: &[("StatusCode", "$2")],
            unit: None,
            default_value: None,
            messages: &["10.0.0.1 503 12"],
            expected: &[(&[("StatusCode", "503")], &[1.0])],
        },
        Row {
            case: "a space-delimited field list whose '...' stands for a run of fields",
            pattern: "[ip, ..., size]",
            metric_value: "$size",
            dimensions: &[("Ip", "$ip")],
            unit: Some("Bytes"),
            default_value: None,
            messages: &[
                "10.0.0.1 GET /orders 200 512",
                "10.0.0.2 404",
                // Two slots are named, so a one-field event matches no row of the list.
                "10.0.0.3",
            ],
            // The slots before the wildcard count from the start of the event and those
            // after it from its end, so `$size` is the last field however many lie between —
            // including none, which is what the second event has.
            expected: &[
                (&[("Ip", "10.0.0.1")], &[512.0]),
                (&[("Ip", "10.0.0.2")], &[404.0]),
            ],
        },
        Row {
            case: "a dimension selector no event resolves publishes nothing",
            pattern: r#"{ $.status = "fail" }"#,
            metric_value: "1",
            dimensions: &[("Service", "$.absent")],
            unit: None,
            default_value: None,
            messages: &[r#"{"service":"checkout","status":"fail"}"#],
            expected: &[],
        },
        Row {
            case: "a metric value no event resolves publishes nothing",
            pattern: r#"{ $.status = "fail" }"#,
            metric_value: "$.absent",
            dimensions: &[],
            unit: None,
            default_value: None,
            messages: &[r#"{"service":"checkout","status":"fail"}"#],
            expected: &[],
        },
        Row {
            case: "a default value is emitted for every non-matching event",
            pattern: r#"{ $.status = "fail" }"#,
            metric_value: "1",
            dimensions: &[],
            unit: Some("Count"),
            default_value: Some(2.0),
            messages: &[
                r#"{"status":"fail"}"#,
                r#"{"status":"ok"}"#,
                "not a JSON document at all",
            ],
            expected: &[(&[], &[1.0, 2.0, 2.0])],
        },
        Row {
            case: "an absent default value emits nothing for a non-matching event",
            pattern: r#"{ $.status = "fail" }"#,
            metric_value: "1",
            dimensions: &[],
            unit: None,
            default_value: None,
            messages: &[
                r#"{"status":"fail"}"#,
                r#"{"status":"ok"}"#,
                "not a JSON document at all",
            ],
            expected: &[(&[], &[1.0])],
        },
        Row {
            case: "the empty pattern matches every ingested event",
            pattern: "",
            metric_value: "1",
            dimensions: &[],
            unit: None,
            default_value: None,
            messages: &["anything", "at all", "whatever its shape"],
            expected: &[(&[], &[1.0, 1.0, 1.0])],
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let case = row.case;
        let group = format!("/metric-filters/extraction-{index}");
        let namespace = format!("MetricFilterExtraction{index}");
        let metric = "Extracted";
        group_with_stream(&logs, &group, "events", None).await;

        let mut transformation = json!({
            "metricName": metric,
            "metricNamespace": &namespace,
            "metricValue": row.metric_value,
        });
        if !row.dimensions.is_empty() {
            transformation["dimensions"] = transformation_dimensions(row.dimensions);
        }
        if let Some(unit) = row.unit {
            transformation["unit"] = json!(unit);
        }
        if let Some(default_value) = row.default_value {
            transformation["defaultValue"] = json!(default_value);
        }
        logs.ok(
            "PutMetricFilter",
            &json!({
                "logGroupName": &group,
                "filterName": "extract",
                "filterPattern": row.pattern,
                "metricTransformations": [transformation],
            }),
        )
        .await;

        let events: Vec<Value> = row
            .messages
            .iter()
            .map(|message| json!({"timestamp": at, "message": message}))
            .collect();
        let response = logs
            .ok(
                "PutLogEvents",
                &json!({
                    "logGroupName": &group,
                    "logStreamName": "events",
                    "logEvents": events,
                }),
            )
            .await;
        assert!(
            response.get("rejectedLogEventsInfo").is_none(),
            "case {case:?}: the batch must be accepted whole, got {response}"
        );

        let mut expected: Vec<Published> = row
            .expected
            .iter()
            .flat_map(|(dimensions, values)| {
                values.iter().map(move |value| Published {
                    dimensions: dimension_set(dimensions),
                    value: *value,
                    unit: row.unit.unwrap_or(UNIT_NONE).to_string(),
                    // A datapoint carries the instant of the EVENT it was extracted from,
                    // which CloudWatch holds in seconds.
                    timestamp_secs: at as f64 / 1_000.0,
                })
            })
            .collect();
        expected.sort_by(|left, right| {
            left.dimensions
                .cmp(&right.dimensions)
                .then(left.value.total_cmp(&right.value))
        });
        let actual = published(&logs, &namespace, metric);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the published datapoints {expected:?}, got {actual:?}"
        );
    }
}

/// A metric filter extracts from the events ingested WHILE it is stored, and a second filter
/// on the same log group extracts independently.
#[tokio::test]
async fn extraction_follows_the_filters_the_group_holds_when_the_batch_arrives() {
    let logs = Logs::new();
    let at = base_ms();
    const GROUP: &str = "/metric-filters/lifecycle";
    const NAMESPACE: &str = "MetricFilterLifecycle";
    const FAILURES: &str = "Failures";
    const EVERYTHING: &str = "Everything";
    let matching = r#"{"status":"fail"}"#;
    group_with_stream(&logs, GROUP, "events", None).await;

    // An event ingested before any filter was stored publishes nothing: extraction happens
    // on ingest, so there is no later call that could make the metric appear.
    ingest_one(&logs, GROUP, at, matching).await;
    assert!(
        published(&logs, NAMESPACE, FAILURES).is_empty(),
        "case \"before any filter\": an event ingested before the filter existed publishes \
         nothing, got {:?}",
        published(&logs, NAMESPACE, FAILURES)
    );

    for (filter, metric, pattern) in [
        ("count-failures", FAILURES, r#"{ $.status = "fail" }"#),
        ("count-everything", EVERYTHING, ""),
    ] {
        logs.ok(
            "PutMetricFilter",
            &json!({
                "logGroupName": GROUP,
                "filterName": filter,
                "filterPattern": pattern,
                "metricTransformations": [{
                    "metricName": metric,
                    "metricNamespace": NAMESPACE,
                    "metricValue": "1",
                }],
            }),
        )
        .await;
    }

    // One event, two filters: each publishes its own metric, independently of the other.
    ingest_one(&logs, GROUP, at, matching).await;
    for metric in [FAILURES, EVERYTHING] {
        let actual = published(&logs, NAMESPACE, metric);
        assert_eq!(
            actual.len(),
            1,
            "case {metric:?}: each stored filter extracts from the same event once, got \
             {actual:?}"
        );
    }

    // A filter the group no longer holds extracts nothing from a later batch, while the one
    // it still holds keeps extracting.
    logs.ok(
        "DeleteMetricFilter",
        &json!({"logGroupName": GROUP, "filterName": "count-failures"}),
    )
    .await;
    ingest_one(&logs, GROUP, at, matching).await;
    for (metric, expected) in [(FAILURES, 1usize), (EVERYTHING, 2usize)] {
        let actual = published(&logs, NAMESPACE, metric);
        assert_eq!(
            actual.len(),
            expected,
            "case \"after count-failures is deleted\": {metric} must hold {expected} \
             datapoints, got {actual:?}"
        );
    }
}
