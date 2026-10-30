// SPDX-License-Identifier: CC0-1.0

//! Where a client READS what `PublishMetrics` accepted: Amazon CloudWatch, mounted beside
//! this service on ONE engine, asked through its own `ListMetrics`, `GetMetricStatistics`
//! and `GetMetricData`.
//!
//! The RULE every row asserts: a datum this service accepts is published to the destination
//! the operation documents for it — the `AWS/MWAA` namespace of Amazon CloudWatch, in the
//! series its metric name and its dimension set identify, that set being the datum's own
//! dimensions together with the one naming the environment the request addressed — as the
//! statistics of the observations the datum states and under the unit it declared. So every
//! CloudWatch reader of that series reports what was published, and a read that names a
//! different series (another environment's, or one without the environment's dimension at
//! all) reports nothing.
//!
//! The rows differ along what the rule generalises over: the form a datum states its
//! observations in (one `Value`, a `StatisticValues` set, and a mixture of both in one
//! series), how many datums one request carries, how many dimensions they carry (none, one,
//! two), and whether a unit travels with them.
//!
//! The namespace, the dimension name and the metric names are written here as the Amazon
//! MWAA metric reference names them rather than taken from this crate's own constants, so a
//! series published under a name no AWS reader would ask for fails these rows.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The namespace the environment health metrics of Amazon MWAA live in.
const NAMESPACE: &str = "AWS/MWAA";

/// The dimension those metrics are reported per: the environment they describe.
const ENVIRONMENT_DIMENSION: &str = "Environment";

/// The `Period` every read below asks at: the one-minute granularity these datapoints are
/// published at.
const PERIOD_SECS: i64 = 60;

/// Half the window each read spans, in seconds: wide enough to hold the minute the row
/// published into, narrow enough that one read asks for few windows.
const WINDOW_SECS: i64 = 300;

/// The `StandardUnit` value a datapoint published without one is reported under.
const NO_UNIT: &str = "None";

/// The `X-Amz-Target` prefix of the CloudWatch service.
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// This service and CloudWatch on one engine: what a publish stores is CloudWatch's, so it is
/// read in-process through CloudWatch's own surface — the shape `core/router/src/lib.rs`
/// mounts them in.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_mwaa_amazonmwaa::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// One Amazon MWAA call, over its restJson1 surface.
async fn mwaa(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| value.to_string().into_bytes())
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("api.airflow.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/airflow/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One CloudWatch call, over its awsJson1.0 surface.
async fn cloudwatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("monitoring.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/monitoring/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// An environment called `name`, carrying the members a create requires and nothing else.
async fn given_environment(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, body) = mwaa(
        router,
        "PUT",
        &format!("/environments/{name}"),
        Some(json!({
            "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/{name}-execution"),
            "SourceBucketArn": "arn:aws:s3:::my-airflow-bucket-unique-name",
            "DagS3Path": "dags",
            "NetworkConfiguration": {
                "SubnetIds": ["subnet-aaa111", "subnet-bbb222"],
                "SecurityGroupIds": ["sg-111"],
            },
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture create of {name} must succeed, got {status} {body}"
    );
}

/// `PublishMetrics` for `name`, which must be served.
async fn publish(router: &MultiServiceRouter, case: &str, name: &str, datums: Vec<Value>) {
    let (status, body) = mwaa(
        router,
        "POST",
        &format!("/metrics/environments/{name}"),
        Some(json!({ "MetricData": datums })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PublishMetrics for {name} must be served, got {status} {body}"
    );
}

/// The start of the minute the clock is in, in epoch seconds: every datum of a row is
/// published inside it, so one period window holds the whole row.
fn minute_start() -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs() as i64;
    now - now % PERIOD_SECS
}

/// The dimension list of the published series: the datum's own dimensions and the
/// environment's, as CloudWatch takes them in a request.
fn series_dimensions(environment: &str, datum: &[(&str, &str)]) -> Value {
    let mut listed: Vec<Value> = datum
        .iter()
        .map(|(name, value)| json!({"Name": name, "Value": value}))
        .collect();
    listed.push(json!({"Name": ENVIRONMENT_DIMENSION, "Value": environment}));
    Value::Array(listed)
}

/// A dimension list as a comparable set of `name=value` pairs, so two lists that name the
/// same dimensions in different orders are one set.
fn dimension_set(listed: &Value) -> BTreeSet<String> {
    listed
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|dimension| {
            format!(
                "{}={}",
                dimension["Name"].as_str().unwrap_or_default(),
                dimension["Value"].as_str().unwrap_or_default()
            )
        })
        .collect()
}

/// The dimension sets `ListMetrics` reports for `metric` of the Amazon MWAA namespace.
async fn listed_dimension_sets(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
) -> Vec<BTreeSet<String>> {
    let (status, body) = cloudwatch(router, "ListMetrics", &json!({ "Namespace": NAMESPACE })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListMetrics answered {status} {body}"
    );
    body["Metrics"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|listed| listed["MetricName"] == json!(metric))
        .map(|listed| dimension_set(&listed["Dimensions"]))
        .collect()
}

/// The `Datapoints` `GetMetricStatistics` answers for one series over the window around the
/// minute `at`.
async fn statistics_datapoints(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    dimensions: Value,
    at: i64,
) -> Vec<Value> {
    let (status, body) = cloudwatch(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": dimensions,
            "StartTime": at - WINDOW_SECS,
            "EndTime": at + WINDOW_SECS,
            "Period": PERIOD_SECS,
            "Statistics": ["SampleCount", "Sum", "Minimum", "Maximum", "Average"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetMetricStatistics answered {status} {body}"
    );
    body["Datapoints"].as_array().cloned().unwrap_or_default()
}

/// The values `GetMetricData` answers for one series' `Sum` over the same window.
async fn metric_data_values(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    dimensions: Value,
    unit: &str,
    at: i64,
) -> Vec<f64> {
    let (status, body) = cloudwatch(
        router,
        "GetMetricData",
        &json!({
            "StartTime": at - WINDOW_SECS,
            "EndTime": at + WINDOW_SECS,
            "MetricDataQueries": [{
                "Id": "published",
                "MetricStat": {
                    "Metric": {
                        "Namespace": NAMESPACE,
                        "MetricName": metric,
                        "Dimensions": dimensions,
                    },
                    "Period": PERIOD_SECS,
                    "Stat": "Sum",
                    "Unit": unit,
                },
                "ReturnData": true,
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetMetricData answered {status} {body}"
    );
    body["MetricDataResults"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|result| result["Id"] == json!("published"))
        .flat_map(|result| {
            result["Values"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(Value::as_f64)
                .collect::<Vec<f64>>()
        })
        .collect()
}

/// What one datum of a row states: one observation, or a summarised set of them.
#[derive(Debug, Clone, Copy)]
enum Observation {
    /// A `Value`: one observation.
    Value(f64),
    /// A `StatisticValues` set: its count, sum, minimum and maximum.
    Set(i64, f64, f64, f64),
}

impl Observation {
    /// The datum as a `MetricData` member states it, at `at` (epoch seconds), under `unit`.
    fn datum(self, metric: &str, dimensions: &[(&str, &str)], unit: Option<&str>, at: i64) -> Value {
        let mut datum = json!({
            "MetricName": metric,
            "Timestamp": at,
        });
        let object = datum.as_object_mut().expect("an object datum");
        match self {
            Self::Value(value) => {
                object.insert("Value".to_string(), json!(value));
            }
            Self::Set(sample_count, sum, minimum, maximum) => {
                object.insert(
                    "StatisticValues".to_string(),
                    json!({
                        "SampleCount": sample_count,
                        "Sum": sum,
                        "Minimum": minimum,
                        "Maximum": maximum,
                    }),
                );
            }
        }
        if let Some(unit) = unit {
            object.insert("Unit".to_string(), json!(unit));
        }
        if !dimensions.is_empty() {
            object.insert(
                "Dimensions".to_string(),
                Value::Array(
                    dimensions
                        .iter()
                        .map(|(name, value)| json!({"Name": name, "Value": value}))
                        .collect(),
                ),
            );
        }
        datum
    }
}

/// One row: a series published for one environment, and the statistics its readers must
/// report for it.
struct Row {
    case: &'static str,
    metric: &'static str,
    /// The dimensions the DATUMS carry; the published series carries the environment's too.
    dimensions: &'static [(&'static str, &'static str)],
    /// The unit the datums declare, or `None` where they declare none.
    unit: Option<&'static str>,
    datums: &'static [Observation],
    sample_count: f64,
    sum: f64,
    minimum: f64,
    maximum: f64,
    average: f64,
}

/// Every series this suite asserts the rule over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one value datum, no dimensions and no unit",
            metric: "CpuUtilization",
            dimensions: &[],
            unit: None,
            datums: &[Observation::Value(42.5)],
            sample_count: 1.0,
            sum: 42.5,
            minimum: 42.5,
            maximum: 42.5,
            average: 42.5,
        },
        Row {
            case: "two value datums under two dimensions and a unit",
            metric: "QueuedTasks",
            dimensions: &[("Function", "Scheduler"), ("Queue", "default")],
            unit: Some("Count"),
            datums: &[Observation::Value(3.0), Observation::Value(5.0)],
            sample_count: 2.0,
            sum: 8.0,
            minimum: 3.0,
            maximum: 5.0,
            average: 4.0,
        },
        Row {
            case: "a statistic set under one dimension",
            metric: "TaskDuration",
            dimensions: &[("Function", "Worker")],
            unit: Some("Seconds"),
            datums: &[Observation::Set(4, 10.0, 1.0, 5.0)],
            sample_count: 4.0,
            sum: 10.0,
            minimum: 1.0,
            maximum: 5.0,
            average: 2.5,
        },
        Row {
            case: "a value and a statistic set of one series together",
            metric: "SchedulerHeartbeat",
            dimensions: &[],
            unit: Some("None"),
            datums: &[Observation::Value(3.0), Observation::Set(2, 9.0, 4.0, 5.0)],
            sample_count: 3.0,
            sum: 12.0,
            minimum: 3.0,
            maximum: 5.0,
            average: 4.0,
        },
    ]
}

/// Assert `datapoints` is the one aggregate `row` published, reported under `unit`.
fn assert_aggregate(row: &Row, datapoints: &[Value]) {
    let case = row.case;
    assert_eq!(
        datapoints.len(),
        1,
        "case {case:?}: expected the one period the row published into, got {datapoints:?}"
    );
    let datapoint = &datapoints[0];
    for (statistic, wanted) in [
        ("SampleCount", row.sample_count),
        ("Sum", row.sum),
        ("Minimum", row.minimum),
        ("Maximum", row.maximum),
        ("Average", row.average),
    ] {
        let actual = datapoint[statistic].as_f64();
        assert_eq!(
            actual,
            Some(wanted),
            "case {case:?}: expected {statistic} {wanted}, got {actual:?} in {datapoint}"
        );
    }
    let unit = datapoint["Unit"].as_str();
    assert_eq!(
        unit,
        Some(row.unit.unwrap_or(NO_UNIT)),
        "case {case:?}: expected the unit it was published with, got {unit:?}"
    );
}

#[tokio::test]
async fn every_published_datum_is_reported_by_the_cloudwatch_readers() {
    let router = emulator();
    let at = minute_start();
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let name = format!("DestinationRow{index}");
        given_environment(&router, case, &name).await;

        let datums: Vec<Value> = row
            .datums
            .iter()
            .enumerate()
            .map(|(position, observation)| {
                observation.datum(row.metric, row.dimensions, row.unit, at + position as i64)
            })
            .collect();
        publish(&router, case, &name, datums).await;

        // ListMetrics reports the series: the metric name, in the Amazon MWAA namespace,
        // under the datum's dimensions together with the environment's.
        let published = series_dimensions(&name, row.dimensions);
        let listed = listed_dimension_sets(&router, case, row.metric).await;
        assert!(
            listed.contains(&dimension_set(&published)),
            "case {case:?}: ListMetrics must report {} under {published}, got {listed:?}",
            row.metric
        );

        // GetMetricStatistics aggregates exactly what the row published.
        let datapoints =
            statistics_datapoints(&router, case, row.metric, published.clone(), at).await;
        assert_aggregate(row, &datapoints);

        // GetMetricData reads the same series, so its `Sum` over the window is the row's.
        let values = metric_data_values(
            &router,
            case,
            row.metric,
            published.clone(),
            row.unit.unwrap_or(NO_UNIT),
            at,
        )
        .await;
        assert_eq!(
            values,
            vec![row.sum],
            "case {case:?}: expected GetMetricData to sum to {}, got {values:?}",
            row.sum
        );

        // The environment is part of the series' identity: a read that omits its dimension
        // names another series, which nothing published to.
        let without_environment = Value::Array(
            row.dimensions
                .iter()
                .map(|(dimension, value)| json!({"Name": dimension, "Value": value}))
                .collect(),
        );
        let bare = statistics_datapoints(&router, case, row.metric, without_environment, at).await;
        assert!(
            bare.is_empty(),
            "case {case:?}: a read without the {ENVIRONMENT_DIMENSION} dimension must report \
             nothing, got {bare:?}"
        );
    }
}

#[tokio::test]
async fn one_metric_published_by_two_environments_is_two_series() {
    let router = emulator();
    let case = "two environments";
    let at = minute_start();
    let metric = "QueuedTasks";
    let dimensions: &[(&str, &str)] = &[("Function", "Scheduler")];

    // The same metric name, the same dimensions, two environments, two values.
    for (name, value) in [("NeighbourOne", 2.0), ("NeighbourTwo", 11.0)] {
        given_environment(&router, case, name).await;
        publish(
            &router,
            case,
            name,
            vec![Observation::Value(value).datum(metric, dimensions, Some("Count"), at)],
        )
        .await;
    }

    let listed = listed_dimension_sets(&router, case, metric).await;
    for name in ["NeighbourOne", "NeighbourTwo"] {
        let published = series_dimensions(name, dimensions);
        assert!(
            listed.contains(&dimension_set(&published)),
            "case {case:?}: ListMetrics must report the series of {name} ({published}), got \
             {listed:?}"
        );
    }

    // Each series reports its OWN environment's observation and nothing of the other's.
    for (name, value) in [("NeighbourOne", 2.0), ("NeighbourTwo", 11.0)] {
        let datapoints = statistics_datapoints(
            &router,
            case,
            metric,
            series_dimensions(name, dimensions),
            at,
        )
        .await;
        assert_eq!(
            datapoints.len(),
            1,
            "case {case:?}: expected one period for {name}, got {datapoints:?}"
        );
        for (statistic, wanted) in [("SampleCount", 1.0), ("Sum", value), ("Maximum", value)] {
            let actual = datapoints[0][statistic].as_f64();
            assert_eq!(
                actual,
                Some(wanted),
                "case {case:?}: expected {statistic} {wanted} for {name}, got {actual:?}"
            );
        }
    }
}
