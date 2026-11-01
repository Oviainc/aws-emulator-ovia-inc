// SPDX-License-Identifier: CC0-1.0

//! The SERIES an ingested Embedded Metric Format log event materialises, as CloudWatch's own
//! `ListMetrics` catalogues them.
//!
//! The rule under test: **ingesting an Embedded Metric Format log event materialises exactly
//! one metric series per (declared metric, declared dimension projection) pair of every
//! directive the document carries, where a projection is the declared dimension names the
//! document supplies a string value for — a name it supplies none for is not a dimension of
//! the series and is left out, so a projection of only such names is the series with no
//! dimensions — and no other series. The namespace's `ListMetrics` catalogue is therefore
//! exactly the set of (metric, dimension names) pairs the envelope declared.** Two crates have
//! to agree for that to hold (CloudWatch Logs ingests, CloudWatch catalogues), so the suite
//! lives here.
//!
//! The rows differ along what the rule generalises over: how many projections one directive
//! declares, whether the zero-dimension projection is the only one or sits beside dimensioned
//! ones, how it is spelled (an empty set inside the list, a list with no set at all, no
//! `Dimensions` member, a set of names the document supplies no value for), how many of a
//! projection's names the document supplies, how many directives one document carries and how
//! many metrics each declares, and whether a declared metric has a value to publish.

use std::collections::BTreeSet;
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

/// An Embedded Metric Format document: the `_aws` metadata the format defines, with the
/// directives `directives` and the document members the directives read values and dimension
/// values from.
fn emf_document(timestamp_ms: i64, directives: Value, members: Value) -> String {
    let mut document = json!({
        "_aws": {
            "Timestamp": timestamp_ms,
            "CloudWatchMetrics": directives,
        },
    });
    if let Some(members) = members.as_object() {
        for (member, value) in members {
            document[member] = value.clone();
        }
    }
    document.to_string()
}

/// The catalogue CloudWatch holds for one namespace: every (metric name, dimension names)
/// pair `ListMetrics` reports, which is the set form the exactness rule is stated over.
async fn catalogue(
    router: &MultiServiceRouter,
    case: &str,
    namespace: &str,
) -> BTreeSet<(String, Vec<String>)> {
    let mut pairs: BTreeSet<(String, Vec<String>)> = BTreeSet::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({"Namespace": namespace});
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let reply = cloudwatch(router, case, "ListMetrics", &request).await;
        for metric in reply["Metrics"].as_array().into_iter().flatten() {
            let name = metric["MetricName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a catalogued metric is named: {metric}"))
                .to_string();
            let mut names: Vec<String> = metric["Dimensions"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|dimension| {
                    dimension["Name"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: a catalogued dimension is named: {metric}")
                        })
                        .to_string()
                })
                .collect();
            names.sort();
            pairs.insert((name, names));
        }
        next_token = reply["NextToken"].as_str().map(ToString::to_string);
        if next_token.is_none() {
            return pairs;
        }
    }
}

/// The expected catalogue of one row, in the same set form.
fn expected_catalogue(pairs: &[(&str, &[&str])]) -> BTreeSet<(String, Vec<String>)> {
    pairs
        .iter()
        .map(|(metric, dimensions)| {
            let mut names: Vec<String> = dimensions.iter().map(ToString::to_string).collect();
            names.sort();
            (metric.to_string(), names)
        })
        .collect()
}

/// One row of the table: a document, and exactly the series it declares.
struct Row {
    /// What the row varies, named in every assertion.
    case: &'static str,
    /// The `_aws.CloudWatchMetrics` directives the document carries.
    directives: Value,
    /// The document's own members, holding the metric values and the dimension values.
    members: Value,
    /// Exactly the (metric, dimension names) pairs the namespace must catalogue.
    expected: Vec<(&'static str, &'static [&'static str])>,
}

/// The catalogue of an EMF namespace is exactly the set of projections the envelope declared.
#[tokio::test]
async fn an_emf_document_materialises_every_declared_projection_and_nothing_else() {
    let router = build_router();
    let at = now_ms() - 60_000;

    let rows: Vec<Row> = vec![
        Row {
            case: "the zero-dimension projection is the only one declared",
            directives: json!([{
                "Namespace": "__NAMESPACE__",
                "Dimensions": [[]],
                "Metrics": [{"Name": "Faults", "Unit": "Count"}],
            }]),
            members: json!({"Faults": 3}),
            expected: vec![("Faults", &[] as &[&str])],
        },
        Row {
            case: "no dimension set at all is declared",
            directives: json!([{
                "Namespace": "__NAMESPACE__",
                "Dimensions": [],
                "Metrics": [{"Name": "Faults", "Unit": "Count"}],
            }]),
            members: json!({"Faults": 3}),
            expected: vec![("Faults", &[] as &[&str])],
        },
        Row {
            case: "no Dimensions member at all",
            directives: json!([{
                "Namespace": "__NAMESPACE__",
                "Metrics": [{"Name": "Faults", "Unit": "Count"}],
            }]),
            members: json!({"Faults": 3}),
            expected: vec![("Faults", &[] as &[&str])],
        },
        Row {
            case: "the zero-dimension projection beside two dimensioned ones",
            directives: json!([{
                "Namespace": "__NAMESPACE__",
                "Dimensions": [["Service", "Operation"], ["Service"], []],
                "Metrics": [{"Name": "Calls", "Unit": "Count"}],
            }]),
            members: json!({"Service": "api", "Operation": "GetItem", "Calls": 2}),
            expected: vec![
                ("Calls", &["Operation", "Service"] as &[&str]),
                ("Calls", &["Service"] as &[&str]),
                ("Calls", &[] as &[&str]),
            ],
        },
        Row {
            case: "a dimensioned directive beside a zero-dimension directive",
            directives: json!([
                {
                    "Namespace": "__NAMESPACE__",
                    "Dimensions": [["Service", "Operation"], ["Service"]],
                    "Metrics": [
                        {"Name": "OrderCount", "Unit": "Count"},
                        {"Name": "OrderLatency", "Unit": "Milliseconds"},
                    ],
                },
                {
                    "Namespace": "__NAMESPACE__",
                    "Dimensions": [[]],
                    "Metrics": [{"Name": "Orphan", "Unit": "Count"}],
                },
            ]),
            members: json!({
                "Service": "api",
                "Operation": "PlaceOrder",
                "OrderCount": 2,
                "OrderLatency": 15.5,
                "Orphan": 1,
            }),
            expected: vec![
                ("OrderCount", &["Operation", "Service"] as &[&str]),
                ("OrderCount", &["Service"] as &[&str]),
                ("OrderLatency", &["Operation", "Service"] as &[&str]),
                ("OrderLatency", &["Service"] as &[&str]),
                ("Orphan", &[] as &[&str]),
            ],
        },
        Row {
            case: "a projection of one name the document supplies no value for",
            directives: json!([{
                "Namespace": "__NAMESPACE__",
                "Dimensions": [["NotAProperty"]],
                "Metrics": [{"Name": "Orphan", "Unit": "Count"}],
            }]),
            members: json!({"Service": "checkout", "Orphan": 5}),
            expected: vec![("Orphan", &[] as &[&str])],
        },
        Row {
            case: "a projection mixing a supplied name with one the document does not carry",
            directives: json!([{
                "Namespace": "__NAMESPACE__",
                "Dimensions": [["Service", "NotAProperty"]],
                "Metrics": [{"Name": "Calls", "Unit": "Count"}],
            }]),
            members: json!({"Service": "api", "Calls": 4}),
            expected: vec![("Calls", &["Service"] as &[&str])],
        },
        Row {
            case: "two projections the document resolves to the same dimensions",
            directives: json!([{
                "Namespace": "__NAMESPACE__",
                "Dimensions": [["Service", "NotAProperty"], ["Service"]],
                "Metrics": [{"Name": "Calls", "Unit": "Count"}],
            }]),
            members: json!({"Service": "api", "Calls": 4}),
            expected: vec![("Calls", &["Service"] as &[&str])],
        },
        Row {
            case: "an unsupplied projection beside the empty one is the same series",
            directives: json!([{
                "Namespace": "__NAMESPACE__",
                "Dimensions": [["Missing"], []],
                "Metrics": [{"Name": "Calls", "Unit": "Count"}],
            }]),
            members: json!({"Calls": 4}),
            expected: vec![("Calls", &[] as &[&str])],
        },
        Row {
            case: "a dimension name whose document value is not a string supplies none",
            directives: json!([{
                "Namespace": "__NAMESPACE__",
                "Dimensions": [["Service"]],
                "Metrics": [{"Name": "Calls", "Unit": "Count"}],
            }]),
            members: json!({"Service": 7, "Calls": 4}),
            expected: vec![("Calls", &[] as &[&str])],
        },
        Row {
            case: "a metric the document holds no value for declares no series",
            directives: json!([{
                "Namespace": "__NAMESPACE__",
                "Dimensions": [[]],
                "Metrics": [{"Name": "Present"}, {"Name": "Absent"}],
            }]),
            members: json!({"Present": 1}),
            expected: vec![("Present", &[] as &[&str])],
        },
        Row {
            case: "three metrics under three projections each",
            directives: json!([{
                "Namespace": "__NAMESPACE__",
                "Dimensions": [[], ["Service"], ["Service", "Operation"]],
                "Metrics": [
                    {"Name": "One"},
                    {"Name": "Two"},
                    {"Name": "Three"},
                ],
            }]),
            members: json!({
                "Service": "api",
                "Operation": "GetItem",
                "One": 1,
                "Two": [2, 3],
                "Three": 4.5,
            }),
            expected: vec![
                ("One", &[] as &[&str]),
                ("One", &["Service"] as &[&str]),
                ("One", &["Operation", "Service"] as &[&str]),
                ("Two", &[] as &[&str]),
                ("Two", &["Service"] as &[&str]),
                ("Two", &["Operation", "Service"] as &[&str]),
                ("Three", &[] as &[&str]),
                ("Three", &["Service"] as &[&str]),
                ("Three", &["Operation", "Service"] as &[&str]),
            ],
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let case = row.case;
        // A namespace and a log group of its own per row, so the catalogue a row reads is
        // exactly what that row's document declared.
        let namespace = format!("EmfProjections{index}");
        let group = format!("/emf/projections-{index}");
        group_with_stream(&router, case, &group, "stream").await;
        let directives = with_namespace(row.directives, &namespace);
        let document = emf_document(at, directives, row.members);
        ingest(&router, case, &group, "stream", at, &document).await;

        let actual = catalogue(&router, case, &namespace).await;
        let expected = expected_catalogue(&row.expected);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the namespace to catalogue exactly {expected:?}, got \
             {actual:?}"
        );
    }
}

/// A document declaring more distinct projections than the emulator resolves per directive
/// publishes a bounded number of them, and never one it did not declare.
///
/// The ceiling is the EMULATOR'S OWN (the crate's `MAX_PROJECTIONS_PER_DIRECTIVE`, 30, which a
/// test in another crate cannot name), so what is pinned here is the bound and the fidelity of
/// what survives it: one message cannot drive unbounded work, and every series it does
/// materialise is one its own envelope declared.
#[tokio::test]
async fn a_document_declaring_many_projections_publishes_a_bounded_subset_of_them() {
    const CASE: &str = "sixty-four distinct one-name projections";
    const CEILING: usize = 30;
    const NAMESPACE: &str = "EmfProjectionCeiling";
    const GROUP: &str = "/emf/projection-ceiling";
    let router = build_router();
    let at = now_ms() - 60_000;
    group_with_stream(&router, CASE, GROUP, "stream").await;

    // Sixty-four dimension names, each supplied with a value of its own, declared as
    // sixty-four one-name projections of one metric.
    let names: Vec<String> = (0..64).map(|index| format!("Dim{index}")).collect();
    let mut members = json!({"Calls": 1});
    for name in &names {
        members[name] = json!("value");
    }
    let directives = json!([{
        "Namespace": NAMESPACE,
        "Dimensions": names.iter().map(|name| json!([name])).collect::<Vec<Value>>(),
        "Metrics": [{"Name": "Calls", "Unit": "Count"}],
    }]);
    let document = emf_document(at, directives, members);
    ingest(&router, CASE, GROUP, "stream", at, &document).await;

    let actual = catalogue(&router, CASE, NAMESPACE).await;
    assert!(
        actual.len() <= CEILING,
        "case {CASE:?}: expected at most {CEILING} series from one directive, got {}: {actual:?}",
        actual.len()
    );
    assert!(
        !actual.is_empty(),
        "case {CASE:?}: the declared metric still publishes, got {actual:?}"
    );
    for (metric, dimensions) in &actual {
        assert_eq!(
            metric, "Calls",
            "case {CASE:?}: only the declared metric is published, got {metric:?}"
        );
        assert!(
            dimensions.len() == 1 && names.contains(&dimensions[0]),
            "case {CASE:?}: every published series is one the envelope declared, got \
             {dimensions:?}"
        );
    }
}

/// `directives` with each directive's namespace placeholder filled in with `namespace`, so a
/// row states its directives once and still publishes into a namespace of its own.
fn with_namespace(mut directives: Value, namespace: &str) -> Value {
    for directive in directives.as_array_mut().into_iter().flatten() {
        directive["Namespace"] = json!(namespace);
    }
    directives
}

/// The statistics CloudWatch reports for one series over a window around `at_ms`.
async fn statistics(
    router: &MultiServiceRouter,
    case: &str,
    namespace: &str,
    metric_name: &str,
    dimensions: &Value,
    at_ms: i64,
) -> Option<(f64, f64)> {
    let at_secs = at_ms as f64 / 1_000.0;
    let reply = cloudwatch(
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
    let points = reply["Datapoints"].as_array().cloned().unwrap_or_default();
    points.first().map(|point| {
        let statistic = |name: &str| {
            point.get(name).and_then(Value::as_f64).unwrap_or_else(|| {
                panic!("case {case:?}: a datapoint carries {name}, got {point}")
            })
        };
        (statistic("Sum"), statistic("SampleCount"))
    })
}

/// One row of the table below: a document, and the series its one value is counted in once.
struct ValueRow {
    /// What the row varies, named in every assertion.
    case: &'static str,
    /// The `_aws.CloudWatchMetrics` directives the document carries.
    directives: Value,
    /// The document's own members.
    members: Value,
    /// The metric the one declared value belongs to, and the value itself.
    metric: &'static str,
    /// The value the series must report, once.
    value: f64,
    /// The dimensions the series is read back under.
    dimensions: Value,
}

/// A projection the document resolves to the same dimensions as another is ONE series, and the
/// value a document declares once is counted once in it.
#[tokio::test]
async fn a_resolved_projection_carries_the_declared_value_counted_once() {
    let router = build_router();
    let at = now_ms() - 60_000;

    let rows: Vec<ValueRow> = vec![
        ValueRow {
            case: "the only declared name supplies no value",
            directives: json!([{
                "Namespace": "__NAMESPACE__",
                "Dimensions": [["NotAProperty"]],
                "Metrics": [{"Name": "Orphan", "Unit": "Count"}],
            }]),
            members: json!({"Service": "checkout", "Orphan": 5}),
            metric: "Orphan",
            value: 5.0,
            dimensions: json!([]),
        },
        ValueRow {
            case: "two declared projections resolve to the same one dimension",
            directives: json!([{
                "Namespace": "__NAMESPACE__",
                "Dimensions": [["Service", "NotAProperty"], ["Service"]],
                "Metrics": [{"Name": "Calls", "Unit": "Count"}],
            }]),
            members: json!({"Service": "api", "Calls": 7}),
            metric: "Calls",
            value: 7.0,
            dimensions: json!([{"Name": "Service", "Value": "api"}]),
        },
        ValueRow {
            case: "two declared projections resolve to the same empty one",
            directives: json!([{
                "Namespace": "__NAMESPACE__",
                "Dimensions": [["Missing"], []],
                "Metrics": [{"Name": "Calls", "Unit": "Count"}],
            }]),
            members: json!({"Calls": 9}),
            metric: "Calls",
            value: 9.0,
            dimensions: json!([]),
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let case = row.case;
        let namespace = format!("EmfProjectionValues{index}");
        let group = format!("/emf/projection-values-{index}");
        group_with_stream(&router, case, &group, "stream").await;
        let directives = with_namespace(row.directives, &namespace);
        let document = emf_document(at, directives, row.members);
        ingest(&router, case, &group, "stream", at, &document).await;

        let reported = statistics(&router, case, &namespace, row.metric, &row.dimensions, at).await;
        assert_eq!(
            reported,
            Some((row.value, 1.0)),
            "case {case:?}: expected the series to report the declared value {} once, got \
             {reported:?}",
            row.value
        );
    }
}
