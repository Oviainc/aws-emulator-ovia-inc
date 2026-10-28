// SPDX-License-Identifier: CC0-1.0

//! Ingesting a log event publishes the CloudWatch metric data the log group's METRIC FILTERS
//! extract from it — in CloudWatch.
//!
//! The rule under test: **a metric filter is an ingest-time rule, so every log event
//! `PutLogEvents` stores into the log group is run against each stored filter's
//! `filterPattern`, and each `metricTransformations` entry of a filter that matches publishes
//! its `metricValue` into `metricNamespace`/`metricName` with the declared `unit`, under one
//! CloudWatch dimension per `dimensions` entry whose value is what that entry's selector
//! resolves to IN THAT EVENT, while an event the pattern does not match publishes the
//! transformation's `defaultValue` where it declares one and nothing where it does not.** Two
//! crates have to agree for that to hold (CloudWatch Logs ingests, CloudWatch answers for the
//! metric), so the suite lives here: a crate-local test can only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: the form of the filter pattern (the
//! empty pattern, a JSON pattern, a space-delimited one, an unstructured term), the form of
//! the metric value (a literal number, a JSON selector, a space-delimited field), how many
//! dimensions the transformation declares and from which selectors, whether a unit and a
//! default value are declared, how many events match, and whether a selector resolves in the
//! event at all. The recorded one-dimension `$.service` filter an issue happens to mention is
//! a single row.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// CloudWatch Logs' `X-Amz-Target` prefix and the signing name its credential scope carries.
const LOGS_TARGET_PREFIX: &str = "Logs_20140328";
const LOGS_SIGNING_NAME: &str = "logs";

/// CloudWatch's `X-Amz-Target` prefix and signing name (`sigv4Name: monitoring`).
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";
const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";

/// The `StandardUnit` value CloudWatch reports for a datapoint whose publisher declared no
/// unit of its own. Source: `MetricTransformation$unit` — "The unit to assign to the metric.
/// If you omit this, the unit is set as None" — and the model's `StandardUnit` enum, which
/// spells that value `None`.
const UNIT_NONE: &str = "None";

/// The whole message AWS refuses a transformation declaring both `dimensions` and a
/// `defaultValue` with: fixed text that interpolates nothing from the request.
const DIMENSIONS_WITH_DEFAULT_VALUE: &str =
    "Invalid metric transformation: dimensions and default value are mutually exclusive \
     properties";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// The SigV4 header a request carries, naming the signing name the router routes on.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
    )
}

/// Call one operation of either service over its awsJson wire.
async fn call(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    content_type: &str,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", content_type)
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Call one CloudWatch Logs operation, requiring it to succeed.
async fn logs(router: &MultiServiceRouter, case: &str, operation: &str, request: &Value) -> Value {
    let (status, parsed) = call(
        router,
        LOGS_SIGNING_NAME,
        LOGS_TARGET_PREFIX,
        "application/x-amz-json-1.1",
        operation,
        request,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from logs {operation}, got {status} with body {parsed}"
    );
    parsed
}

/// Call one CloudWatch Logs operation that must be refused, returning its wire error code and
/// message.
async fn logs_refusal(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
) -> (String, String) {
    let (status, parsed) = call(
        router,
        LOGS_SIGNING_NAME,
        LOGS_TARGET_PREFIX,
        "application/x-amz-json-1.1",
        operation,
        request,
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from logs {operation}, got {status} with body {parsed}"
    );
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a refusal carries __type, got {parsed}"));
    let message = parsed
        .get("message")
        .or_else(|| parsed.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a refusal carries a message, got {parsed}"));
    (
        code.rsplit(['#', '/']).next().unwrap_or(code).to_string(),
        message.to_string(),
    )
}

/// Call one CloudWatch operation, requiring it to succeed.
async fn cloudwatch(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
) -> Value {
    let (status, parsed) = call(
        router,
        CLOUDWATCH_SIGNING_NAME,
        CLOUDWATCH_TARGET_PREFIX,
        "application/x-amz-json-1.0",
        operation,
        request,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudWatch {operation}, got {status} with body \
         {parsed}"
    );
    parsed
}

/// Epoch milliseconds, the unit a CloudWatch Logs timestamp carries.
fn now_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the Unix epoch")
            .as_millis(),
    )
    .expect("the epoch milliseconds fit in an i64")
}

/// A log group and a stream inside it, both created.
async fn group_with_stream(router: &MultiServiceRouter, case: &str, group: &str, stream: &str) {
    logs(router, case, "CreateLogGroup", &json!({"logGroupName": group})).await;
    logs(
        router,
        case,
        "CreateLogStream",
        &json!({"logGroupName": group, "logStreamName": stream}),
    )
    .await;
}

/// The `dimensions` map of a transformation, from `(name, selector)` pairs.
fn filter_dimensions(pairs: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (name, selector) in pairs {
        map.insert((*name).to_string(), json!(selector));
    }
    Value::Object(map)
}

/// The `Dimensions` list a CloudWatch read names, from `(name, value)` pairs.
fn read_dimensions(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(name, value)| json!({"Name": name, "Value": value}))
            .collect(),
    )
}

/// Ingest `messages` as one batch of log events, all stamped at `timestamp_ms`.
async fn ingest(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    stream: &str,
    timestamp_ms: i64,
    messages: &[&str],
) {
    let events: Vec<Value> = messages
        .iter()
        .map(|message| json!({"timestamp": timestamp_ms, "message": message}))
        .collect();
    let response = logs(
        router,
        case,
        "PutLogEvents",
        &json!({
            "logGroupName": group,
            "logStreamName": stream,
            "logEvents": events,
        }),
    )
    .await;
    assert!(
        response.get("rejectedLogEventsInfo").is_none(),
        "case {case:?}: the batch must be accepted whole, got {response}"
    );
}

/// The datapoints CloudWatch reports for one series over a window around `at_ms`.
async fn datapoints(
    router: &MultiServiceRouter,
    case: &str,
    namespace: &str,
    metric_name: &str,
    dimensions: &Value,
    at_ms: i64,
) -> Vec<Value> {
    let at_secs = at_ms as f64 / 1_000.0;
    let stats = cloudwatch(
        router,
        case,
        "GetMetricStatistics",
        &json!({
            "Namespace": namespace,
            "MetricName": metric_name,
            "Dimensions": dimensions,
            "StartTime": at_secs - 300.0,
            "EndTime": at_secs + 300.0,
            "Period": 600,
            "Statistics": ["Sum", "SampleCount"],
        }),
    )
    .await;
    stats["Datapoints"].as_array().cloned().unwrap_or_default()
}

/// The `Sum` CloudWatch reports for one series, or `None` when it holds no datapoint at all.
async fn sum(
    router: &MultiServiceRouter,
    case: &str,
    namespace: &str,
    metric_name: &str,
    dimensions: &Value,
    at_ms: i64,
) -> Option<f64> {
    let points = datapoints(router, case, namespace, metric_name, dimensions, at_ms).await;
    assert!(
        points.len() <= 1,
        "case {case:?}: one period window holds one datapoint, got {points:?}"
    );
    points
        .first()
        .map(|point| point["Sum"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: a datapoint carries a numeric Sum, got {point}")
        }))
}

/// The dimension set a read names, as a row states it: `(name, value)` pairs.
type Dimensions = &'static [(&'static str, &'static str)];

/// One read a row expects: the dimensions it names, and the `Sum` over the enclosing period —
/// `None` for a series that holds no datapoint.
type Read = (Dimensions, Option<f64>);

/// One row of the table: a filter to store, the events to ingest, and the series each must
/// make CloudWatch report.
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
    /// What a reader must find: the dimensions it names, and the `Sum` over the enclosing
    /// period — `None` for a series that holds no datapoint.
    reads: &'static [Read],
}

/// Every form of filter pattern, metric value and dimension set publishes exactly the
/// datapoints the stored filter declares.
#[tokio::test]
async fn an_ingested_event_publishes_the_metric_data_its_log_groups_filters_extract() {
    let router = build_router();
    let at = now_ms() - 60_000;

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
                r#"{"service":"checkout","status":"fail","n":3}"#,
                r#"{"service":"checkout","status":"ok","n":4}"#,
            ],
            reads: &[
                (&[("Service", "checkout")], Some(3.0)),
                // The dimensioned series is its own series: the undimensioned one holds
                // nothing, and so does one naming a value no event resolved.
                (&[], None),
                (&[("Service", "search")], None),
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
            reads: &[(&[], Some(350.5))],
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
                r#"{"service":"api","region":"eu-west-2","status":"fail"}"#,
                r#"{"service":"api","region":"us-east-1","status":"fail"}"#,
            ],
            reads: &[
                (
                    &[("Service", "api"), ("Region", "eu-west-2")],
                    Some(2.0),
                ),
                (&[("Service", "api"), ("Region", "us-east-1")], Some(1.0)),
                // A subset of a declared dimension set is a different series.
                (&[("Service", "api")], None),
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
                "10.0.0.4 404",
            ],
            reads: &[
                (&[("StatusCode", "200")], Some(768.0)),
                (&[("StatusCode", "404")], Some(64.0)),
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
                r#"{"service":"billing","msg":"ERROR again"}"#,
                r#"{"service":"billing","msg":"INFO fine"}"#,
            ],
            reads: &[(&[("Service", "billing")], Some(2.0))],
        },
        Row {
            case: "a dimension selector no event resolves publishes nothing",
            pattern: r#"{ $.status = "fail" }"#,
            metric_value: "1",
            dimensions: &[("Service", "$.absent")],
            unit: None,
            default_value: None,
            messages: &[r#"{"service":"checkout","status":"fail"}"#],
            reads: &[(&[], None), (&[("Service", "checkout")], None)],
        },
        Row {
            case: "a metric value no event resolves publishes nothing",
            pattern: r#"{ $.status = "fail" }"#,
            metric_value: "$.absent",
            dimensions: &[],
            unit: None,
            default_value: None,
            messages: &[r#"{"service":"checkout","status":"fail"}"#],
            reads: &[(&[], None)],
        },
        Row {
            case: "a default value is emitted for every non-matching event",
            pattern: r#"{ $.status = "fail" }"#,
            metric_value: "1",
            dimensions: &[],
            unit: None,
            default_value: Some(2.0),
            messages: &[
                r#"{"status":"fail"}"#,
                r#"{"status":"ok"}"#,
                "not a JSON document at all",
            ],
            reads: &[(&[], Some(5.0))],
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
            reads: &[(&[], Some(1.0))],
        },
        Row {
            case: "the empty pattern matches every ingested event",
            pattern: "",
            metric_value: "1",
            dimensions: &[],
            unit: None,
            default_value: None,
            messages: &["anything", "at all", "whatever its shape"],
            reads: &[(&[], Some(3.0))],
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let case = row.case;
        let group = format!("/metric-filters/forms-{index}");
        let namespace = format!("MetricFilterForms{index}");
        let metric = "Extracted";
        group_with_stream(&router, case, &group, "events").await;

        let mut transformation = json!({
            "metricName": metric,
            "metricNamespace": &namespace,
            "metricValue": row.metric_value,
        });
        if !row.dimensions.is_empty() {
            transformation["dimensions"] = filter_dimensions(row.dimensions);
        }
        if let Some(unit) = row.unit {
            transformation["unit"] = json!(unit);
        }
        if let Some(default_value) = row.default_value {
            transformation["defaultValue"] = json!(default_value);
        }
        logs(
            &router,
            case,
            "PutMetricFilter",
            &json!({
                "logGroupName": &group,
                "filterName": "extract",
                "filterPattern": row.pattern,
                "metricTransformations": [transformation],
            }),
        )
        .await;

        ingest(&router, case, &group, "events", at, row.messages).await;

        for (dimensions, expected) in row.reads {
            let read = read_dimensions(dimensions);
            let points = datapoints(&router, case, &namespace, metric, &read, at).await;
            match expected {
                None => assert!(
                    points.is_empty(),
                    "case {case:?}: {dimensions:?} must hold no datapoint, got {points:?}"
                ),
                Some(expected) => {
                    assert_eq!(
                        points.len(),
                        1,
                        "case {case:?}: {dimensions:?} must hold one period window, got \
                         {points:?}"
                    );
                    assert_eq!(
                        points[0]["Sum"].as_f64(),
                        Some(*expected),
                        "case {case:?}: {dimensions:?} must report Sum {expected}, got {:?}",
                        points[0]
                    );
                    assert_eq!(
                        points[0]["Unit"].as_str(),
                        Some(row.unit.unwrap_or(UNIT_NONE)),
                        "case {case:?}: {dimensions:?} must report the declared unit, got {:?}",
                        points[0]
                    );
                }
            }
        }
    }
}

/// A filter refused for declaring both `dimensions` and a `defaultValue` publishes nothing,
/// while its accepted sibling on the same log group publishes every matching event.
#[tokio::test]
async fn a_refused_transformation_publishes_nothing_and_its_accepted_sibling_publishes() {
    let router = build_router();
    let at = now_ms() - 60_000;
    const GROUP: &str = "/metric-filters/dimensions-and-default";
    const NAMESPACE: &str = "MetricFilterExclusion";
    let case = "dimensions and a default value are mutually exclusive";
    group_with_stream(&router, case, GROUP, "events").await;

    let (code, message) = logs_refusal(
        &router,
        case,
        "PutMetricFilter",
        &json!({
            "logGroupName": GROUP,
            "filterName": "dimdef-refused",
            "filterPattern": r#"{ $.status = "fail" }"#,
            "metricTransformations": [{
                "metricName": "Refused",
                "metricNamespace": NAMESPACE,
                "metricValue": "1",
                "defaultValue": 0.0,
                "dimensions": {"Service": "$.service"},
            }],
        }),
    )
    .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case {case:?}: expected InvalidParameterException, got {code}"
    );
    assert_eq!(
        message, DIMENSIONS_WITH_DEFAULT_VALUE,
        "case {case:?}: expected the fixed refusal text, got {message:?}"
    );

    logs(
        &router,
        case,
        "PutMetricFilter",
        &json!({
            "logGroupName": GROUP,
            "filterName": "dimdef-control",
            "filterPattern": r#"{ $.status = "fail" }"#,
            "metricTransformations": [{
                "metricName": "Control",
                "metricNamespace": NAMESPACE,
                "metricValue": "1",
                "dimensions": {"Service": "$.service"},
            }],
        }),
    )
    .await;
    // The accepted filter is the only one the group holds, which is what makes the refused
    // filter's silence an absence rather than lag.
    let described = logs(
        &router,
        case,
        "DescribeMetricFilters",
        &json!({"logGroupName": GROUP}),
    )
    .await;
    let names: Vec<&str> = described["metricFilters"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: DescribeMetricFilters lists filters, got {described}"))
        .iter()
        .filter_map(|filter| filter["filterName"].as_str())
        .collect();
    assert_eq!(
        names,
        vec!["dimdef-control"],
        "case {case:?}: only the accepted filter is stored, got {names:?}"
    );

    ingest(
        &router,
        case,
        GROUP,
        "events",
        at,
        &[
            r#"{"service":"checkout","status":"fail","n":1}"#,
            r#"{"service":"checkout","status":"fail","n":2}"#,
            r#"{"service":"checkout","status":"fail","n":3}"#,
        ],
    )
    .await;

    let dimensioned = read_dimensions(&[("Service", "checkout")]);
    assert_eq!(
        sum(&router, case, NAMESPACE, "Control", &dimensioned, at).await,
        Some(3.0),
        "case {case:?}: three matching events each publishing 1 sum to 3"
    );
    for (read_case, dimensions) in [
        ("under the dimension", dimensioned.clone()),
        ("with no dimension", read_dimensions(&[])),
    ] {
        assert_eq!(
            sum(&router, read_case, NAMESPACE, "Refused", &dimensions, at).await,
            None,
            "case {read_case:?}: the refused filter publishes nothing"
        );
    }
}

/// A filter extracts from the events ingested WHILE it is stored: the events that preceded it
/// publish nothing, and deleting it stops the extraction.
#[tokio::test]
async fn extraction_follows_the_filters_the_group_held_when_the_batch_arrived() {
    let router = build_router();
    let at = now_ms() - 60_000;
    const GROUP: &str = "/metric-filters/lifecycle";
    const NAMESPACE: &str = "MetricFilterLifecycle";
    const METRIC: &str = "Matches";
    let case = "a filter extracts from the events ingested while it is stored";
    group_with_stream(&router, case, GROUP, "events").await;

    let matching = r#"{"status":"fail"}"#;
    let put_filter = json!({
        "logGroupName": GROUP,
        "filterName": "count-failures",
        "filterPattern": r#"{ $.status = "fail" }"#,
        "metricTransformations": [{
            "metricName": METRIC,
            "metricNamespace": NAMESPACE,
            "metricValue": "1",
        }],
    });
    let no_dimensions = read_dimensions(&[]);

    // (case, what to do before the batch, the Sum the series must report afterwards)
    ingest(&router, "before any filter", GROUP, "events", at, &[matching]).await;
    assert_eq!(
        sum(
            &router,
            "before any filter",
            NAMESPACE,
            METRIC,
            &no_dimensions,
            at
        )
        .await,
        None,
        "case \"before any filter\": an event ingested before the filter existed publishes \
         nothing"
    );

    logs(&router, case, "PutMetricFilter", &put_filter).await;
    ingest(&router, case, GROUP, "events", at, &[matching, matching]).await;
    assert_eq!(
        sum(&router, case, NAMESPACE, METRIC, &no_dimensions, at).await,
        Some(2.0),
        "case {case:?}: the two events ingested under the filter each publish 1"
    );

    logs(
        &router,
        "after the filter is deleted",
        "DeleteMetricFilter",
        &json!({"logGroupName": GROUP, "filterName": "count-failures"}),
    )
    .await;
    ingest(
        &router,
        "after the filter is deleted",
        GROUP,
        "events",
        at,
        &[matching],
    )
    .await;
    assert_eq!(
        sum(
            &router,
            "after the filter is deleted",
            NAMESPACE,
            METRIC,
            &no_dimensions,
            at
        )
        .await,
        Some(2.0),
        "case \"after the filter is deleted\": the deleted filter extracts nothing more"
    );
}
