// SPDX-License-Identifier: CC0-1.0

//! Ingesting an Embedded Metric Format log event publishes CloudWatch metric data — in
//! CloudWatch.
//!
//! The rule under test: **a log event whose message is an Embedded Metric Format document
//! publishes, on ingest, the metrics that document declares into the namespace, under the
//! dimension sets and with the units and values it declares, readable through CloudWatch's own
//! metric readers without any `PutMetricData` — unless an account-level
//! `METRIC_EXTRACTION_POLICY` selects the log group, in which case that group's documents
//! publish nothing while every group the policy does not select keeps publishing.** Two crates
//! have to agree for that to hold (CloudWatch Logs ingests, CloudWatch answers for the
//! metric), so the suite lives here: a crate-local test can only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: how many metrics one document
//! declares, whether a metric's value is a single number or an array of them, how many
//! dimension sets the directive declares (none, one, two) and which document members supply
//! their values, whether a unit and a storage resolution are declared, and whether the
//! message is an EMF document at all. The recorded one-metric document an issue happens to
//! mention is a single row.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// CloudWatch Logs' `X-Amz-Target` prefix and the signing name its credential scope carries.
const LOGS_TARGET_PREFIX: &str = "Logs_20140328";
const LOGS_SIGNING_NAME: &str = "logs";

/// CloudWatch's `X-Amz-Target` prefix and signing name (`sigv4Name: monitoring`).
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";
const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";

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

/// Epoch milliseconds, the unit a CloudWatch Logs timestamp and an EMF `Timestamp` carry.
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

/// Ingest one message as a single log event stamped at `timestamp_ms`.
async fn ingest(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    stream: &str,
    timestamp_ms: i64,
    message: &str,
) {
    logs(
        router,
        case,
        "PutLogEvents",
        &json!({
            "logGroupName": group,
            "logStreamName": stream,
            "logEvents": [{"timestamp": timestamp_ms, "message": message}],
        }),
    )
    .await;
}

/// The datapoints CloudWatch reports for one series over a window around `at_ms`.
async fn datapoints(
    router: &MultiServiceRouter,
    case: &str,
    namespace: &str,
    metric_name: &str,
    dimensions: &Value,
    at_ms: i64,
    statistics: &[&str],
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
            "Statistics": statistics,
        }),
    )
    .await;
    stats["Datapoints"].as_array().cloned().unwrap_or_default()
}

/// One statistic of a datapoint.
fn statistic(point: &Value, name: &str) -> Option<f64> {
    point.get(name).and_then(Value::as_f64)
}

/// An Embedded Metric Format document: the `_aws` metadata the format defines, plus the
/// members its directives read the values and the dimension values from.
fn emf_document(timestamp_ms: i64, directive: Value, members: Value) -> String {
    let mut document = json!({
        "_aws": {
            "Timestamp": timestamp_ms,
            "CloudWatchMetrics": [directive],
        },
    });
    if let Some(members) = members.as_object() {
        for (member, value) in members {
            document[member] = value.clone();
        }
    }
    document.to_string()
}

/// One row of the table below: the document to ingest and the series it must publish.
struct Row {
    /// What the row varies, named in every assertion.
    case: &'static str,
    /// The namespace the directive publishes into.
    namespace: &'static str,
    /// The `_aws.CloudWatchMetrics` directive the document carries.
    directive: Value,
    /// The document's own members, which the directive reads the values and the dimension
    /// values from.
    members: Value,
    /// The metric the series is read back under.
    metric: &'static str,
    /// The dimensions it is read back under.
    dimensions: Value,
    /// The statistic each published value must come back as.
    expected: Vec<(&'static str, f64)>,
}

/// Every form a document's metrics take publishes exactly the datapoints it declares.
#[tokio::test]
async fn an_ingested_emf_document_publishes_the_metrics_it_declares() {
    let router = build_router();
    let at = now_ms() - 60_000;

    // The rows differ along how a metric's value is carried, how many dimension sets the
    // directive declares, and whether a unit or a storage resolution is declared.
    let cases: Vec<Row> = vec![
        Row {
            case: "one metric with no dimensions",
            namespace: "EmfOne",
            directive: json!({
                "Namespace": "EmfOne",
                "Dimensions": [[]],
                "Metrics": [{"Name": "Faults", "Unit": "Count"}],
            }),
            members: json!({"Faults": 3}),
            metric: "Faults",
            dimensions: json!([]),
            expected: vec![("Maximum", 3.0), ("Sum", 3.0), ("SampleCount", 1.0)],
        },
        Row {
            case: "one metric under one dimension set",
            namespace: "EmfTwo",
            directive: json!({
                "Namespace": "EmfTwo",
                "Dimensions": [["Service"]],
                "Metrics": [{"Name": "Latency", "Unit": "Milliseconds"}],
            }),
            members: json!({"Service": "api", "Latency": 42.5}),
            metric: "Latency",
            dimensions: json!([{"Name": "Service", "Value": "api"}]),
            expected: vec![("Maximum", 42.5), ("Sum", 42.5), ("SampleCount", 1.0)],
        },
        Row {
            case: "an array of values for one metric",
            namespace: "EmfThree",
            directive: json!({
                "Namespace": "EmfThree",
                "Dimensions": [["Service"]],
                "Metrics": [{"Name": "Latency"}],
            }),
            members: json!({"Service": "api", "Latency": [10.0, 20.0, 30.0]}),
            metric: "Latency",
            dimensions: json!([{"Name": "Service", "Value": "api"}]),
            expected: vec![
                ("Maximum", 30.0),
                ("Minimum", 10.0),
                ("Sum", 60.0),
                ("SampleCount", 3.0),
                ("Average", 20.0),
            ],
        },
        Row {
            case: "a high-resolution metric",
            namespace: "EmfFour",
            directive: json!({
                "Namespace": "EmfFour",
                "Dimensions": [["Service"]],
                "Metrics": [{"Name": "Ticks", "StorageResolution": 1}],
            }),
            members: json!({"Service": "api", "Ticks": 7}),
            metric: "Ticks",
            dimensions: json!([{"Name": "Service", "Value": "api"}]),
            expected: vec![("Maximum", 7.0)],
        },
    ];
    for (index, row) in cases.into_iter().enumerate() {
        let case = row.case;
        let group = format!("/emf/forms-{index}");
        group_with_stream(&router, case, &group, "stream").await;
        let document = emf_document(at, row.directive, row.members);
        ingest(&router, case, &group, "stream", at, &document).await;

        let statistics: Vec<&str> = row.expected.iter().map(|(name, _)| *name).collect();
        let points = datapoints(
            &router,
            case,
            row.namespace,
            row.metric,
            &row.dimensions,
            at,
            &statistics,
        )
        .await;
        assert_eq!(
            points.len(),
            1,
            "case {case:?}: expected one period window of datapoints, got {points:?}"
        );
        for (name, value) in row.expected {
            assert_eq!(
                statistic(&points[0], name),
                Some(value),
                "case {case:?}: expected {name} {value}, got {:?}",
                points[0]
            );
        }
    }
}

/// A document declaring several metrics and several dimension sets publishes each series, and
/// a message that declares nothing publishes nothing.
#[tokio::test]
async fn several_dimension_sets_are_several_series_and_a_plain_message_publishes_nothing() {
    let router = build_router();
    let at = now_ms() - 60_000;
    const GROUP: &str = "/emf/sets";
    const NAMESPACE: &str = "EmfSets";
    group_with_stream(&router, "several sets", GROUP, "stream").await;

    let document = emf_document(
        at,
        json!({
            "Namespace": NAMESPACE,
            // Two dimension sets: the metrics are published once under each.
            "Dimensions": [["Service", "Operation"], ["Service"]],
            "Metrics": [
                {"Name": "Calls", "Unit": "Count"},
                {"Name": "Latency", "Unit": "Milliseconds"},
            ],
        }),
        json!({"Service": "api", "Operation": "GetItem", "Calls": 2, "Latency": 15.0}),
    );
    ingest(&router, "several sets", GROUP, "stream", at, &document).await;

    // (case, the metric, the dimensions, the value the series must report)
    let series: Vec<(&str, &str, Value, f64)> = vec![
        (
            "Calls under both dimensions",
            "Calls",
            json!([
                {"Name": "Service", "Value": "api"},
                {"Name": "Operation", "Value": "GetItem"},
            ]),
            2.0,
        ),
        (
            "Calls under the one-dimension set",
            "Calls",
            json!([{"Name": "Service", "Value": "api"}]),
            2.0,
        ),
        (
            "Latency under both dimensions",
            "Latency",
            json!([
                {"Name": "Service", "Value": "api"},
                {"Name": "Operation", "Value": "GetItem"},
            ]),
            15.0,
        ),
        (
            "Latency under the one-dimension set",
            "Latency",
            json!([{"Name": "Service", "Value": "api"}]),
            15.0,
        ),
    ];
    for (case, metric, dimensions, expected) in series {
        let points = datapoints(
            &router,
            case,
            NAMESPACE,
            metric,
            &dimensions,
            at,
            &["Maximum"],
        )
        .await;
        assert_eq!(
            points.len(),
            1,
            "case {case:?}: expected one datapoint, got {points:?}"
        );
        assert_eq!(
            statistic(&points[0], "Maximum"),
            Some(expected),
            "case {case:?}: expected Maximum {expected}, got {:?}",
            points[0]
        );
    }

    // (case, the message, the metric a reader would look for)
    let publishes_nothing: Vec<(&str, String, &str)> = vec![
        (
            "a plain text message",
            "ERROR something went wrong".to_string(),
            "Calls",
        ),
        (
            "a JSON message with no _aws metadata",
            json!({"Calls": 99}).to_string(),
            "Calls",
        ),
        (
            "a directive naming a member the document does not carry",
            emf_document(
                at,
                json!({
                    "Namespace": "EmfNothing",
                    "Dimensions": [[]],
                    "Metrics": [{"Name": "Absent"}],
                }),
                json!({}),
            ),
            "Absent",
        ),
    ];
    for (case, message, metric) in publishes_nothing {
        ingest(&router, case, GROUP, "stream", at, &message).await;
        let points = datapoints(
            &router,
            case,
            "EmfNothing",
            metric,
            &json!([]),
            at,
            &["Maximum"],
        )
        .await;
        assert!(
            points.is_empty(),
            "case {case:?}: a message that declares no metric publishes none, got {points:?}"
        );
    }

    // A declared dimension name the document supplies no value for is not a dimension of the
    // published series: it is left out of the projection it was declared in, so this
    // projection is the one with no dimensions — the series the metric is read back under.
    // The sibling `logs_emf_declared_projections` suite is where that rule is tabulated.
    const UNSUPPLIED: &str = "a dimension set naming a member the document does not carry";
    ingest(
        &router,
        UNSUPPLIED,
        GROUP,
        "stream",
        at,
        &emf_document(
            at,
            json!({
                "Namespace": "EmfUnsupplied",
                "Dimensions": [["Missing"]],
                "Metrics": [{"Name": "Present"}],
            }),
            json!({"Present": 1}),
        ),
    )
    .await;
    let points = datapoints(
        &router,
        UNSUPPLIED,
        "EmfUnsupplied",
        "Present",
        &json!([]),
        at,
        &["Maximum"],
    )
    .await;
    assert_eq!(
        points.len(),
        1,
        "case {UNSUPPLIED:?}: the metric publishes under the projection with no dimensions, got \
         {points:?}"
    );
    assert_eq!(
        statistic(&points[0], "Maximum"),
        Some(1.0),
        "case {UNSUPPLIED:?}: the published value is the one the document declared, got {:?}",
        points[0]
    );
}

/// A `METRIC_EXTRACTION_POLICY` suppresses extraction for exactly the log groups its
/// selection criteria select.
#[tokio::test]
async fn a_metric_extraction_policy_suppresses_only_the_log_groups_it_selects() {
    let router = build_router();
    let at = now_ms() - 60_000;
    const SELECTED: &str = "/emf/selected";
    const OTHER: &str = "/emf/other";
    const NAMESPACE: &str = "EmfPolicy";
    group_with_stream(&router, "policy", SELECTED, "stream").await;
    group_with_stream(&router, "policy", OTHER, "stream").await;

    // The policy a metric extraction policy document may carry: the real service accepts only
    // the status that turns the format off.
    logs(
        &router,
        "policy",
        "PutAccountPolicy",
        &json!({
            "policyName": "no-emf-for-selected",
            "policyType": "METRIC_EXTRACTION_POLICY",
            "policyDocument": json!({"EmbeddedMetricFormat": {"Status": "Disabled"}}).to_string(),
            "selectionCriteria": format!(r#"LogGroupName IN ["{SELECTED}"]"#),
        }),
    )
    .await;

    // (case, the log group the document is ingested into, the metric it declares, whether a
    // datapoint must appear)
    let cases: Vec<(&str, &str, &str, bool)> = vec![
        (
            "the log group the policy selects",
            SELECTED,
            "Suppressed",
            false,
        ),
        (
            "a log group the policy does not select",
            OTHER,
            "Published",
            true,
        ),
    ];
    for (case, group, metric, expected) in cases {
        let document = emf_document(
            at,
            json!({
                "Namespace": NAMESPACE,
                "Dimensions": [[]],
                "Metrics": [{"Name": metric, "Unit": "Count"}],
            }),
            json!({metric: 3}),
        );
        ingest(&router, case, group, "stream", at, &document).await;
        let points = datapoints(
            &router,
            case,
            NAMESPACE,
            metric,
            &json!([]),
            at,
            &["Maximum"],
        )
        .await;
        assert_eq!(
            !points.is_empty(),
            expected,
            "case {case:?}: expected a published datapoint: {expected}, got {points:?}"
        );
        if expected {
            assert_eq!(
                statistic(&points[0], "Maximum"),
                Some(3.0),
                "case {case:?}: the published value is the one the document declared, got {:?}",
                points[0]
            );
        }
    }

    // Removing the policy puts the selected group back to the default, which is extraction
    // enabled: "By default, EMF metric creation is enabled for all log groups."
    logs(
        &router,
        "policy removed",
        "DeleteAccountPolicy",
        &json!({
            "policyName": "no-emf-for-selected",
            "policyType": "METRIC_EXTRACTION_POLICY",
        }),
    )
    .await;
    let document = emf_document(
        at,
        json!({
            "Namespace": NAMESPACE,
            "Dimensions": [[]],
            "Metrics": [{"Name": "Suppressed", "Unit": "Count"}],
        }),
        json!({"Suppressed": 5}),
    );
    ingest(&router, "policy removed", SELECTED, "stream", at, &document).await;
    let points = datapoints(
        &router,
        "policy removed",
        NAMESPACE,
        "Suppressed",
        &json!([]),
        at,
        &["Maximum"],
    )
    .await;
    assert_eq!(
        points.len(),
        1,
        "case policy removed: the group publishes again once no policy selects it, got \
         {points:?}"
    );
    assert_eq!(
        statistic(&points[0], "Maximum"),
        Some(5.0),
        "case policy removed: the published value is the one the document declared, got {:?}",
        points[0]
    );

    // An account-wide policy — one with no selection criteria — selects every log group, so
    // neither group publishes under it.
    logs(
        &router,
        "account-wide policy",
        "PutAccountPolicy",
        &json!({
            "policyName": "no-emf-at-all",
            "policyType": "METRIC_EXTRACTION_POLICY",
            "policyDocument": json!({"EmbeddedMetricFormat": {"Status": "Disabled"}}).to_string(),
        }),
    )
    .await;
    for group in [SELECTED, OTHER] {
        let document = emf_document(
            at,
            json!({
                "Namespace": NAMESPACE,
                "Dimensions": [[]],
                "Metrics": [{"Name": "AccountWide", "Unit": "Count"}],
            }),
            json!({"AccountWide": 9}),
        );
        ingest(&router, "account-wide policy", group, "stream", at, &document).await;
    }
    let points = datapoints(
        &router,
        "account-wide policy",
        NAMESPACE,
        "AccountWide",
        &json!([]),
        at,
        &["Maximum"],
    )
    .await;
    assert!(
        points.is_empty(),
        "case account-wide policy: a policy selecting every group suppresses every group, got \
         {points:?}"
    );
}
