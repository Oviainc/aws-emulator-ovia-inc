// SPDX-License-Identifier: CC0-1.0

//! WHICH log groups a metric extraction policy turns Embedded Metric Format extraction off
//! for, and what happens to them once the policy is gone.
//!
//! The rule under test: **an account-level `METRIC_EXTRACTION_POLICY` suppresses Embedded
//! Metric Format metric creation for exactly the log groups its `selectionCriteria` select — a
//! `LogGroupName` or `LogGroupNamePrefix` filter written with `IN` selects the groups its
//! values name or prefix, and the same filter written with `NOT IN` selects every group they do
//! NOT — and deleting the policy returns the groups it selected to the documented default,
//! where their documents publish again.** CloudWatch Logs decides the suppression and
//! CloudWatch answers for the metric, so the suite lives here: neither crate alone can see both
//! ends.
//!
//! The rows differ along what the rule generalises over: which field the criteria filter on and
//! which operator they are written with. The four forms are the ones
//! `PutAccountPolicy$selectionCriteria` documents for this policy type ("The selection criteria
//! supports filtering by `LogGroupName` and `LogGroupNamePrefix` using the operators `IN` and
//! `NOT IN`"); the exact-name `IN` case a recording happens to carry is one row of them.
//!
//! The second suite pins which INSTANT an extracted datapoint lands at: the document's own
//! `_aws.Timestamp` when it declares one, and the instant of the log event it travelled in when
//! it does not.

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

/// The namespace, metric and dimension a skipped document is counted under.
const LOGS_NAMESPACE: &str = "AWS/Logs";
const DISABLED_METRIC: &str = "EMFDisabledErrors";
const LOG_GROUP_DIMENSION: &str = "LogGroupName";

/// The document member every directive below reads its dimension value from, so the two log
/// groups of one row are two series of the same metric.
const GROUP_DIMENSION: &str = "Group";

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

/// Ingest one message as a single log event stamped at `timestamp_ms`, requiring the batch to
/// be accepted whole: a rejected event is never extracted, so a row that silently lost its
/// event would be read as a suppressed one.
async fn ingest(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    timestamp_ms: i64,
    message: &str,
) {
    let response = logs(
        router,
        case,
        "PutLogEvents",
        &json!({
            "logGroupName": group,
            "logStreamName": "stream",
            "logEvents": [{"timestamp": timestamp_ms, "message": message}],
        }),
    )
    .await;
    assert!(
        response.get("rejectedLogEventsInfo").is_none(),
        "case {case:?}: the EMF batch for {group} must be accepted whole, got {response}"
    );
}

/// The datapoints CloudWatch reports for one series over a single period window around `at_ms`.
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

/// The `(Sum, SampleCount)` of the one period window a series reports, or `None` when it
/// reports no datapoint at all.
async fn series(
    router: &MultiServiceRouter,
    case: &str,
    namespace: &str,
    metric_name: &str,
    dimensions: &Value,
    at_ms: i64,
) -> Option<(f64, f64)> {
    let points = datapoints(router, case, namespace, metric_name, dimensions, at_ms).await;
    assert!(
        points.len() <= 1,
        "case {case:?}: one period window reports at most one datapoint, got {points:?}"
    );
    points.first().map(|point| {
        let statistic = |name: &str| {
            point.get(name).and_then(Value::as_f64).unwrap_or_else(|| {
                panic!("case {case:?}: a datapoint carries {name}, got {point}")
            })
        };
        (statistic("Sum"), statistic("SampleCount"))
    })
}

/// The count of documents skipped for one log group, as CloudWatch reports it.
async fn skipped_count(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    at_ms: i64,
) -> Option<f64> {
    series(
        router,
        case,
        LOGS_NAMESPACE,
        DISABLED_METRIC,
        &json!([{"Name": LOG_GROUP_DIMENSION, "Value": group}]),
        at_ms,
    )
    .await
    .map(|(sum, _)| sum)
}

/// An Embedded Metric Format document declaring one metric under the group dimension, carrying
/// its own `_aws.Timestamp` only when `document_timestamp_ms` is given.
fn emf_document(
    document_timestamp_ms: Option<i64>,
    namespace: &str,
    metric: &str,
    group: &str,
    value: f64,
) -> String {
    let mut metadata = json!({
        "CloudWatchMetrics": [{
            "Namespace": namespace,
            "Dimensions": [[GROUP_DIMENSION]],
            "Metrics": [{"Name": metric, "Unit": "Count"}],
        }],
    });
    if let Some(timestamp_ms) = document_timestamp_ms {
        metadata["Timestamp"] = json!(timestamp_ms);
    }
    json!({
        "_aws": metadata,
        GROUP_DIMENSION: group,
        metric: value,
    })
    .to_string()
}

/// The dimensions one row's series is read back under.
fn group_dimensions(group: &str) -> Value {
    json!([{"Name": GROUP_DIMENSION, "Value": group}])
}

/// Which `selectionCriteria` filter one row's policy is written with. The four forms are the
/// ones `PutAccountPolicy$selectionCriteria` documents for a metric extraction policy: the
/// fields `LogGroupName` and `LogGroupNamePrefix`, each with the operator `IN` or `NOT IN`.
#[derive(Debug, Clone, Copy)]
enum Form {
    /// `LogGroupName IN [...]` naming the group extraction is off for.
    NameSelects,
    /// `LogGroupName NOT IN [...]` naming the one group extraction stays on for.
    NameExcludes,
    /// `LogGroupNamePrefix IN [...]` naming the prefix extraction is off under.
    PrefixSelects,
    /// `LogGroupNamePrefix NOT IN [...]` naming the one prefix extraction stays on under.
    PrefixExcludes,
}

/// One row: a criteria form, and the two log groups it must sort into a suppressed one and one
/// that keeps extracting.
struct Row {
    /// What the row varies, named in every assertion.
    case: &'static str,
    /// The filter the row's policy is written with.
    form: Form,
}

/// Each documented criteria form suppresses exactly the groups it selects, and deleting the
/// policy puts them back to extracting.
#[tokio::test]
async fn each_selection_criteria_form_suppresses_the_groups_it_selects_until_the_policy_is_gone() {
    let router = build_router();
    let at = now_ms() - 60_000;

    let rows: Vec<Row> = vec![
        Row {
            case: "LogGroupName IN naming the suppressed group",
            form: Form::NameSelects,
        },
        Row {
            case: "LogGroupName NOT IN naming the extracting group",
            form: Form::NameExcludes,
        },
        Row {
            case: "LogGroupNamePrefix IN naming the suppressed prefix",
            form: Form::PrefixSelects,
        },
        Row {
            case: "LogGroupNamePrefix NOT IN naming the extracting prefix",
            form: Form::PrefixExcludes,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let case = row.case;
        // Fresh groups per row, under two disjoint prefixes, so the row's series and its
        // skipped-document count are its own.
        let off_prefix = format!("/emf/scope-{index}/off/");
        let on_prefix = format!("/emf/scope-{index}/on/");
        let suppressed = format!("{off_prefix}group");
        let extracting = format!("{on_prefix}group");
        let namespace = format!("EmfScope{index}");
        let metric = "Probe";
        // Values the row publishes: one per phase, differing per row so no assertion can pass
        // on another row's datapoint.
        let before = 4.0 + index as f64;
        let after = 100.0 + index as f64;

        group_with_stream(&router, case, &suppressed).await;
        group_with_stream(&router, case, &extracting).await;

        let criteria = match row.form {
            Form::NameSelects => format!(r#"LogGroupName IN ["{suppressed}"]"#),
            Form::NameExcludes => format!(r#"LogGroupName NOT IN ["{extracting}"]"#),
            Form::PrefixSelects => format!(r#"LogGroupNamePrefix IN ["{off_prefix}"]"#),
            Form::PrefixExcludes => format!(r#"LogGroupNamePrefix NOT IN ["{on_prefix}"]"#),
        };
        // A metric extraction policy document says only that the format is off, which is the
        // one thing such a policy does. One policy is held at a time: two of this type may not
        // select overlapping sets of log groups.
        let policy_name = format!("no-emf-{index}");
        logs(
            &router,
            case,
            "PutAccountPolicy",
            &json!({
                "policyName": policy_name,
                "policyType": "METRIC_EXTRACTION_POLICY",
                "policyDocument":
                    json!({"EmbeddedMetricFormat": {"Status": "Disabled"}}).to_string(),
                "selectionCriteria": criteria,
            }),
        )
        .await;

        // While the policy is held: the selected group publishes nothing and counts one skipped
        // document, and the group it does not select publishes exactly what its document
        // declared and counts nothing.
        for group in [&suppressed, &extracting] {
            let document = emf_document(Some(at), &namespace, metric, group, before);
            ingest(&router, case, group, at, &document).await;
        }
        let during = series(
            &router,
            case,
            &namespace,
            metric,
            &group_dimensions(&suppressed),
            at,
        )
        .await;
        assert_eq!(
            during, None,
            "case {case:?}: criteria {criteria} select {suppressed}, so it publishes nothing, \
             got {during:?}"
        );
        let unselected = series(
            &router,
            case,
            &namespace,
            metric,
            &group_dimensions(&extracting),
            at,
        )
        .await;
        assert_eq!(
            unselected,
            Some((before, 1.0)),
            "case {case:?}: criteria {criteria} do not select {extracting}, so it publishes \
             ({before}, 1.0), got {unselected:?}"
        );
        let counted = skipped_count(&router, case, &suppressed, at).await;
        assert_eq!(
            counted,
            Some(1.0),
            "case {case:?}: {suppressed} skipped one document, got {counted:?}"
        );
        let uncounted = skipped_count(&router, case, &extracting, at).await;
        assert_eq!(
            uncounted, None,
            "case {case:?}: {extracting} skipped no document, got {uncounted:?}"
        );

        // Deleting the policy returns the groups it selected to the documented default, where
        // EMF metric creation is enabled for every log group.
        logs(
            &router,
            case,
            "DeleteAccountPolicy",
            &json!({
                "policyName": policy_name,
                "policyType": "METRIC_EXTRACTION_POLICY",
            }),
        )
        .await;
        let document = emf_document(Some(at), &namespace, metric, &suppressed, after);
        ingest(&router, case, &suppressed, at, &document).await;
        let resumed = series(
            &router,
            case,
            &namespace,
            metric,
            &group_dimensions(&suppressed),
            at,
        )
        .await;
        // The window holds BOTH ingests, so a Sum of exactly the second value is also the
        // evidence that the first one published nothing while the policy was held.
        assert_eq!(
            resumed,
            Some((after, 1.0)),
            "case {case:?}: with the policy gone {suppressed} publishes ({after}, 1.0), got \
             {resumed:?}"
        );
        let still_counted = skipped_count(&router, case, &suppressed, at).await;
        assert_eq!(
            still_counted,
            Some(1.0),
            "case {case:?}: with the policy gone {suppressed} skips nothing further, so its \
             count stays at 1.0, got {still_counted:?}"
        );
    }
}

/// Which instant an extracted datapoint lands at.
#[tokio::test]
async fn an_extracted_datapoint_lands_at_the_instant_its_document_was_observed() {
    let router = build_router();
    let now = now_ms();

    // (case, the log event's own instant, the `_aws.Timestamp` the document declares, the
    // instant the datapoint must land at, the OTHER instant it could have been stamped with —
    // the one a reader must find nothing at, so the row discriminates between them)
    let rows: Vec<(&str, i64, Option<i64>, i64, i64)> = vec![
        (
            // The document's instant is the one that counts, not the log event's.
            "a document declaring its own Timestamp",
            now - 60_000,
            Some(now - 7_200_000),
            now - 7_200_000,
            now - 60_000,
        ),
        (
            // With no instant of its own, the value was observed when the event it travelled
            // in was — not when the ingest that carried it ran.
            "a document declaring no Timestamp",
            now - 3_600_000,
            None,
            now - 3_600_000,
            now,
        ),
    ];

    for (index, (case, event_at, document_at, expected_at, elsewhere_at)) in
        rows.into_iter().enumerate()
    {
        let group = format!("/emf/instant-{index}");
        let namespace = format!("EmfInstant{index}");
        let metric = "Probe";
        let value = 7.0 + index as f64;
        group_with_stream(&router, case, &group).await;
        let document = emf_document(document_at, &namespace, metric, &group, value);
        ingest(&router, case, &group, event_at, &document).await;

        let landed = series(
            &router,
            case,
            &namespace,
            metric,
            &group_dimensions(&group),
            expected_at,
        )
        .await;
        assert_eq!(
            landed,
            Some((value, 1.0)),
            "case {case:?}: the datapoint falls in the window around {expected_at}, got \
             {landed:?}"
        );

        // The window around the OTHER instant the datapoint could have carried reports
        // nothing, which is what makes the assertion above about the instant rather than about
        // the value.
        let elsewhere = series(
            &router,
            case,
            &namespace,
            metric,
            &group_dimensions(&group),
            elsewhere_at,
        )
        .await;
        assert_eq!(
            elsewhere, None,
            "case {case:?}: no datapoint falls in the window around {elsewhere_at}, got \
             {elsewhere:?}"
        );
    }
}
