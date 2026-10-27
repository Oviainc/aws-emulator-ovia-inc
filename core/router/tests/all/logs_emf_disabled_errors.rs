// SPDX-License-Identifier: CC0-1.0

//! What a log group whose Embedded Metric Format extraction is turned off publishes instead
//! of the metrics it was going to publish.
//!
//! The rule under test: **when metric extraction is disabled for the log group a document is
//! ingested into, that document publishes none of the metrics it declares and is COUNTED
//! once, as the log service's own `EMFDisabledErrors` metric in `AWS/Logs` dimensioned by the
//! log group's name — one count per skipped document, whatever number of metrics or dimension
//! sets it declared — while a document that declared nothing to skip, and every group
//! extraction is still enabled for, counts nothing.** CloudWatch Logs decides the count and
//! CloudWatch answers for it, so the suite lives here: neither crate alone can see both ends.
//!
//! The rows differ along what the rule generalises over: how a policy selects the group (by
//! exact name, by a prefix of it, not at all), how many metrics and dimension sets one skipped
//! document declares, how many documents one request carries, and whether the message declares
//! any metric at all. The recorded one-document case an issue happens to mention is one row.

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

/// The namespace, metric and dimension the skipped documents are counted under, which is what
/// this suite reads back through CloudWatch.
const LOGS_NAMESPACE: &str = "AWS/Logs";
const DISABLED_METRIC: &str = "EMFDisabledErrors";
const LOG_GROUP_DIMENSION: &str = "LogGroupName";

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
async fn group_with_stream(router: &MultiServiceRouter, case: &str, group: &str) {
    logs(router, case, "CreateLogGroup", &json!({"logGroupName": group})).await;
    logs(
        router,
        case,
        "CreateLogStream",
        &json!({"logGroupName": group, "logStreamName": "stream"}),
    )
    .await;
}

/// Ingest `messages` as one chronologically ordered batch of log events, each a millisecond
/// after the last, and require the batch to be accepted whole.
async fn ingest(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    at_ms: i64,
    messages: &[String],
) {
    let events: Vec<Value> = messages
        .iter()
        .enumerate()
        .map(|(index, message)| json!({"timestamp": at_ms + index as i64, "message": message}))
        .collect();
    let response = logs(
        router,
        case,
        "PutLogEvents",
        &json!({
            "logGroupName": group,
            "logStreamName": "stream",
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

/// The `Sum` of the one period window a series reports, or `None` when it reports none.
async fn series_sum(
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
        "case {case:?}: one period window reports at most one datapoint, got {points:?}"
    );
    points
        .first()
        .map(|point| {
            point
                .get("Sum")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("case {case:?}: a datapoint carries Sum, got {point}"))
        })
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

/// The criteria that select a log group for a metric extraction policy, or `None` for a row
/// no policy selects.
#[derive(Debug, Clone, Copy)]
enum Selection {
    /// Selected by the policy whose criteria name the group's exact name.
    ByName,
    /// Selected by the policy whose criteria name a prefix of the group's name.
    ByPrefix,
    /// Not selected by any policy, so extraction is still on for the group.
    NotSelected,
}

/// One row of the table: a log group, the documents written into it, and what the two series
/// must report afterwards.
struct Row {
    /// What the row varies, named in every assertion.
    case: &'static str,
    /// Which policy, if any, selects the row's log group.
    selected: Selection,
    /// The namespace the row's documents declare, read back to see whether they published.
    namespace: &'static str,
    /// The metric those documents declare in that namespace.
    metric: &'static str,
    /// The `Sum` the document's OWN namespace must report, or `None` when it must report
    /// nothing because extraction was skipped.
    expected_own_sum: Option<f64>,
    /// The `Sum` of `AWS/Logs` `EMFDisabledErrors` for the row's group, or `None` when the row
    /// skipped no document and so counts nothing.
    expected_disabled_sum: Option<f64>,
}

/// The exact-name prefix the `ByName` row's group lives under, kept disjoint from the prefix
/// the `ByPrefix` policy selects: two metric extraction policies of one account may not select
/// overlapping sets of log groups.
const BY_NAME_GROUP: &str = "/emf/named-group";
/// The prefix the `ByPrefix` policy selects every group under.
const BY_PREFIX: &str = "/emf/prefixed/";

/// A skipped document counts exactly one `EMFDisabledErrors` for its log group, and publishes
/// none of its own metrics; a document with nothing to skip counts nothing.
#[tokio::test]
async fn a_suppressed_emf_document_counts_one_emf_disabled_error_for_its_log_group() {
    let router = build_router();
    let at = now_ms() - 60_000;

    // The two policies the rows are selected by. A metric extraction policy document says only
    // that the format is off, which is the one thing such a policy does.
    let disabled_document = json!({"EmbeddedMetricFormat": {"Status": "Disabled"}}).to_string();
    for (name, criteria) in [
        ("no-emf-by-name", format!(r#"LogGroupName IN ["{BY_NAME_GROUP}"]"#)),
        ("no-emf-by-prefix", format!(r#"LogGroupNamePrefix IN ["{BY_PREFIX}"]"#)),
    ] {
        logs(
            &router,
            name,
            "PutAccountPolicy",
            &json!({
                "policyName": name,
                "policyType": "METRIC_EXTRACTION_POLICY",
                "policyDocument": disabled_document,
                "selectionCriteria": criteria,
            }),
        )
        .await;
    }

    let rows: Vec<(Row, Vec<String>)> = vec![
        (
            Row {
                case: "one metric, the group named by the policy",
                selected: Selection::ByName,
                namespace: "EmfDisabledOne",
                metric: "Probe",
                expected_own_sum: None,
                expected_disabled_sum: Some(1.0),
            },
            vec![emf_document(
                at,
                json!({
                    "Namespace": "EmfDisabledOne",
                    "Dimensions": [["Svc"]],
                    "Metrics": [{"Name": "Probe", "Unit": "Count"}],
                }),
                json!({"Svc": "obs", "Probe": 11}),
            )],
        ),
        (
            Row {
                // Two metrics under two dimension sets is four series the document would have
                // published; it is still ONE skipped document.
                case: "two metrics and two dimension sets, a group under the selected prefix",
                selected: Selection::ByPrefix,
                namespace: "EmfDisabledMany",
                metric: "Calls",
                expected_own_sum: None,
                expected_disabled_sum: Some(1.0),
            },
            vec![emf_document(
                at,
                json!({
                    "Namespace": "EmfDisabledMany",
                    "Dimensions": [["Svc", "Op"], ["Svc"]],
                    "Metrics": [
                        {"Name": "Calls", "Unit": "Count"},
                        {"Name": "Latency", "Unit": "Milliseconds"},
                    ],
                }),
                json!({"Svc": "obs", "Op": "Get", "Calls": 2, "Latency": 15.0}),
            )],
        ),
        (
            Row {
                case: "three documents in one request, a group under the selected prefix",
                selected: Selection::ByPrefix,
                namespace: "EmfDisabledBatch",
                metric: "Probe",
                expected_own_sum: None,
                expected_disabled_sum: Some(3.0),
            },
            (0..3)
                .map(|index| {
                    emf_document(
                        at,
                        json!({
                            "Namespace": "EmfDisabledBatch",
                            "Dimensions": [["Svc"]],
                            "Metrics": [{"Name": "Probe", "Unit": "Count"}],
                        }),
                        json!({"Svc": "obs", "Probe": 1 + index}),
                    )
                })
                .collect(),
        ),
        (
            Row {
                case: "a plain message in a suppressed group",
                selected: Selection::ByPrefix,
                namespace: "EmfDisabledPlain",
                metric: "Probe",
                expected_own_sum: None,
                expected_disabled_sum: None,
            },
            vec!["ERROR something went wrong".to_string()],
        ),
        (
            Row {
                // The directive names a member the document does not carry, so there was no
                // observation to skip.
                case: "a document declaring a metric the document does not carry",
                selected: Selection::ByPrefix,
                namespace: "EmfDisabledAbsent",
                metric: "Probe",
                expected_own_sum: None,
                expected_disabled_sum: None,
            },
            vec![emf_document(
                at,
                json!({
                    "Namespace": "EmfDisabledAbsent",
                    "Dimensions": [[]],
                    "Metrics": [{"Name": "Probe", "Unit": "Count"}],
                }),
                json!({"Svc": "obs"}),
            )],
        ),
        (
            Row {
                case: "a group no policy selects keeps extracting",
                selected: Selection::NotSelected,
                namespace: "EmfEnabled",
                metric: "Probe",
                expected_own_sum: Some(11.0),
                expected_disabled_sum: None,
            },
            vec![emf_document(
                at,
                json!({
                    "Namespace": "EmfEnabled",
                    "Dimensions": [["Svc"]],
                    "Metrics": [{"Name": "Probe", "Unit": "Count"}],
                }),
                json!({"Svc": "obs", "Probe": 11}),
            )],
        ),
    ];

    for (index, (row, messages)) in rows.into_iter().enumerate() {
        let case = row.case;
        let group = match row.selected {
            Selection::ByName => BY_NAME_GROUP.to_string(),
            Selection::ByPrefix => format!("{BY_PREFIX}group-{index}"),
            Selection::NotSelected => format!("/emf/open/group-{index}"),
        };
        group_with_stream(&router, case, &group).await;
        ingest(&router, case, &group, at, &messages).await;

        // The document's OWN namespace: a suppressed group publishes nothing into it, and a
        // group still extracting publishes exactly the value the document declared.
        let own = series_sum(
            &router,
            case,
            row.namespace,
            row.metric,
            &json!([{"Name": "Svc", "Value": "obs"}]),
            at,
        )
        .await;
        assert_eq!(
            own, row.expected_own_sum,
            "case {case:?}: expected the document's own namespace to report Sum \
             {:?}, got {own:?}",
            row.expected_own_sum
        );

        // The count of skipped documents, dimensioned by the log group they were skipped for.
        let disabled = series_sum(
            &router,
            case,
            LOGS_NAMESPACE,
            DISABLED_METRIC,
            &json!([{"Name": LOG_GROUP_DIMENSION, "Value": group}]),
            at,
        )
        .await;
        assert_eq!(
            disabled, row.expected_disabled_sum,
            "case {case:?}: expected {LOGS_NAMESPACE} {DISABLED_METRIC} for {group} to report \
             Sum {:?}, got {disabled:?}",
            row.expected_disabled_sum
        );
    }

    // The count is dimensioned by log group, so it is not also published undimensioned: a
    // reader asking for the series with no dimensions finds nothing.
    let undimensioned = series_sum(
        &router,
        "undimensioned",
        LOGS_NAMESPACE,
        DISABLED_METRIC,
        &json!([]),
        at,
    )
    .await;
    assert_eq!(
        undimensioned, None,
        "case undimensioned: the count is published only under {LOG_GROUP_DIMENSION}, got \
         {undimensioned:?}"
    );
}

/// A metric extraction policy document the service refuses changes nothing: the group it would
/// have named keeps extracting and counts no skipped document.
#[tokio::test]
async fn a_refused_metric_extraction_policy_leaves_extraction_running() {
    let router = build_router();
    let at = now_ms() - 60_000;
    const GROUP: &str = "/emf/refused";
    const NAMESPACE: &str = "EmfRefused";
    group_with_stream(&router, "refused", GROUP).await;

    // Every status other than the one the service accepts, including the one that spells
    // "enabled" and the ones that differ from `Disabled` only in case.
    for status in ["Enabled", "ENABLED", "DISABLED", "disabled", "bogus"] {
        let (code, body) = call(
            &router,
            LOGS_SIGNING_NAME,
            LOGS_TARGET_PREFIX,
            "application/x-amz-json-1.1",
            "PutAccountPolicy",
            &json!({
                "policyName": "refused-policy",
                "policyType": "METRIC_EXTRACTION_POLICY",
                "policyDocument":
                    json!({"EmbeddedMetricFormat": {"Status": status}}).to_string(),
                "scope": "ALL",
                "selectionCriteria": format!(r#"LogGroupName IN ["{GROUP}"]"#),
            }),
        )
        .await;
        assert_eq!(
            code, 400,
            "case {status:?}: expected HTTP 400, got {code} with body {body}"
        );
        let wire_code = body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {status:?}: an error body carries __type, got {body}"));
        assert!(
            wire_code.ends_with("InvalidParameterException"),
            "case {status:?}: expected InvalidParameterException, got {wire_code}"
        );
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some("Invalid json policy document"),
            "case {status:?}: expected the fixed refusal message, got {body}"
        );

        // A refused put stores nothing, so no policy of that name exists to read back.
        let described = logs(
            &router,
            status,
            "DescribeAccountPolicies",
            &json!({
                "policyType": "METRIC_EXTRACTION_POLICY",
                "policyName": "refused-policy",
            }),
        )
        .await;
        let policies = described["accountPolicies"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            policies.is_empty(),
            "case {status:?}: a refused put stores nothing, got {policies:?}"
        );
    }

    // Nothing selects the group, so its documents still publish and count no skipped document.
    let document = emf_document(
        at,
        json!({
            "Namespace": NAMESPACE,
            "Dimensions": [["Svc"]],
            "Metrics": [{"Name": "Probe", "Unit": "Count"}],
        }),
        json!({"Svc": "obs", "Probe": 11}),
    );
    ingest(&router, "refused", GROUP, at, &[document]).await;

    let points = datapoints(
        &router,
        "refused",
        NAMESPACE,
        "Probe",
        &json!([{"Name": "Svc", "Value": "obs"}]),
        at,
    )
    .await;
    assert_eq!(
        points.len(),
        1,
        "case refused: the group keeps extracting, got {points:?}"
    );
    assert_eq!(
        points[0].get("Sum").and_then(Value::as_f64),
        Some(11.0),
        "case refused: expected Sum 11, got {:?}",
        points[0]
    );
    assert_eq!(
        points[0].get("SampleCount").and_then(Value::as_f64),
        Some(1.0),
        "case refused: expected SampleCount 1, got {:?}",
        points[0]
    );

    let disabled = series_sum(
        &router,
        "refused",
        LOGS_NAMESPACE,
        DISABLED_METRIC,
        &json!([{"Name": LOG_GROUP_DIMENSION, "Value": GROUP}]),
        at,
    )
    .await;
    assert_eq!(
        disabled, None,
        "case refused: a group no accepted policy selects counts no skipped document, got \
         {disabled:?}"
    );
}
