// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a SageMaker resource reference it cannot read as an ARN.
//!
//! A value outside the modelled `SageMakerResourceArn` `@pattern` fails the WHOLE call
//! with `400 ValidationException` and ONE constant sentence — no member path, no
//! collection position, no submitted value and no expression quoted back — whatever the
//! offending value is, wherever in `MetricQueries` it sits and however many queries the
//! batch carries.
//!
//! Every other boundary refusal keeps the framework's per-member envelope, and a
//! reference the pattern ACCEPTS but that no resource answers stays a per-query
//! `Status=ValidationError` inside a 200, so the two channels remain distinguishable.
//!
//! Every test drives the real wire path (`MultiServiceRouter::dispatch` with a restJson1
//! request), so the framework's constraint walk and this service's refusal seam are both
//! exercised.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sagemakermetrics_sagemakermetricsservice::ops::batch_get_metrics::{
    INVALID_RESOURCE_ARN_CODE, INVALID_RESOURCE_ARN_MESSAGE, SAGEMAKER_RESOURCE_ARN_PATTERN,
};
use service_com_amazonaws_sagemakermetrics_sagemakermetricsservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// An epoch second the metric points in these tests are written at.
const T0: i64 = 1_700_000_000;

/// How the framework opens its own per-member constraint clause: the envelope every
/// refusal this service does NOT word itself keeps.
const FRAMEWORK_ENVELOPE_OPENING: &str = "validation error";

/// One emulator with this service mounted on a single shared engine, exactly as the
/// router mounts it in production.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are the framework's business and are not what these tests pin.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What one call answered: the status, the error code (the restJson1 `__type` and the
/// `x-amzn-errortype` header, which must agree) and the parsed body.
struct Answer {
    status: u16,
    error_type: Option<String>,
    header_error_type: Option<String>,
    body: Value,
}

impl Answer {
    /// The `message` member of an error body, or the empty string for a success body.
    fn message(&self) -> &str {
        self.body.get("message").and_then(Value::as_str).unwrap_or("")
    }
}

/// Dispatch one restJson1 request.
async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: &Value) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://metrics.sagemaker.us-east-1.amazonaws.com{path}"))
        .header("host", "metrics.sagemaker.us-east-1.amazonaws.com")
        .header("content-type", "application/json")
        // The SigV4 credential scope is how the router picks the service; the signature
        // itself is not verified here (authentication is off above).
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20261029/us-east-1/sagemaker/aws4_request, \
             SignedHeaders=host, Signature=not-verified",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|e| {
            panic!(
                "{method} {path}: response body is not JSON ({e}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    Answer {
        status,
        error_type: parsed
            .get("__type")
            .and_then(Value::as_str)
            .map(str::to_string),
        header_error_type,
        body: parsed,
    }
}

/// One `MetricQuery` over `resource_arn`, every other member well inside its constraints.
fn query(resource_arn: &str) -> Value {
    json!({
        "MetricName": "loss",
        "ResourceArn": resource_arn,
        "MetricStat": "Last",
        "Period": "IterationNumber",
        "XAxisType": "IterationNumber",
    })
}

/// `POST /BatchGetMetrics` over `queries`.
async fn get_metrics(router: &MultiServiceRouter, queries: Vec<Value>) -> Answer {
    call(
        router,
        "POST",
        "/BatchGetMetrics",
        &json!({ "MetricQueries": queries }),
    )
    .await
}

/// Assert `answer` is the refusal a malformed SageMaker resource reference gets: the
/// whole call refused with the one constant sentence and nothing of the request in it.
fn assert_refused_as_invalid_arn(case: &str, answer: &Answer, submitted: &[&str]) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected status 400, got {} ({:?})",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_type.as_deref(),
        Some(INVALID_RESOURCE_ARN_CODE),
        "case {case:?}: expected __type {INVALID_RESOURCE_ARN_CODE}, got {:?}",
        answer.error_type
    );
    assert_eq!(
        answer.header_error_type.as_deref(),
        Some(INVALID_RESOURCE_ARN_CODE),
        "case {case:?}: expected x-amzn-errortype {INVALID_RESOURCE_ARN_CODE}, got {:?}",
        answer.header_error_type
    );
    assert_eq!(
        answer.message(),
        INVALID_RESOURCE_ARN_MESSAGE,
        "case {case:?}: expected message {INVALID_RESOURCE_ARN_MESSAGE:?}, got {:?}",
        answer.message()
    );
    // The sentence is ALL fixed text: nothing of the request, of the member path or of
    // the modelled expression is interpolated into it.
    for value in submitted.iter().filter(|value| !value.is_empty()) {
        assert!(
            !answer.message().contains(value),
            "case {case:?}: the message must not quote the submitted value {value:?}, \
             got {:?}",
            answer.message()
        );
    }
    for leak in [
        "metricQueries",
        "resourceArn",
        FRAMEWORK_ENVELOPE_OPENING,
        SAGEMAKER_RESOURCE_ARN_PATTERN,
    ] {
        assert!(
            !answer.message().contains(leak),
            "case {case:?}: the message must not carry {leak:?}, got {:?}",
            answer.message()
        );
    }
}

/// Values the modelled `SageMakerResourceArn` `@pattern`
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:[a-z\-].*/`, from
/// `smithy_query(query="shape",shape="SageMakerResourceArn")`) REJECTS, one per way of
/// being malformed. Every one is inside the member's `@length 0..2048`, so the pattern is
/// the only constraint any of them breaks.
fn malformed_arns() -> Vec<(&'static str, String)> {
    vec![
        ("not-an-arn-at-all", "not-an-arn".to_string()),
        // @length allows the empty string; the pattern does not.
        ("empty", String::new()),
        (
            "no-resource-separator",
            "arn:aws:sagemaker:us-east-1:123456789012:experiment-trial-component".to_string(),
        ),
        (
            "another-services-arn",
            "arn:aws:s3:::my-bucket/metrics".to_string(),
        ),
        (
            "account-not-twelve-digits",
            "arn:aws:sagemaker:us-east-1:12345:experiment-trial-component/tc".to_string(),
        ),
        (
            "resource-type-not-lowercase",
            "arn:aws:sagemaker:us-east-1:123456789012:Experiment-Trial-Component/tc".to_string(),
        ),
        (
            "uppercase-arn-scheme",
            "ARN:AWS:SAGEMAKER:us-east-1:123456789012:experiment-trial-component/tc".to_string(),
        ),
        (
            "no-resource-type",
            "arn:aws:sagemaker:us-east-1:123456789012:/tc".to_string(),
        ),
        (
            "partition-separator-missing",
            "arn-aws-sagemaker-us-east-1-123456789012-experiment-trial-component/tc".to_string(),
        ),
        // Long, but well inside @length max 2048: length is not what refuses it.
        ("long-but-not-an-arn", "x".repeat(2_000)),
    ]
}

/// Values the same expression ACCEPTS: the refusal keys on the expression, so these are
/// served rather than refused — across partitions, regions, accounts and resource types.
fn well_formed_arns() -> Vec<(&'static str, String)> {
    vec![
        (
            "trial-component",
            "arn:aws:sagemaker:us-east-1:123456789012:experiment-trial-component/tc-arn-rules"
                .to_string(),
        ),
        (
            "china-partition-training-job",
            "arn:aws-cn:sagemaker:cn-north-1:210987654321:training-job/job-7".to_string(),
        ),
        (
            "govcloud-partition-processing-job",
            "arn:aws-us-gov:sagemaker:us-gov-west-1:111122223333:processing-job/proc".to_string(),
        ),
        (
            // `[a-z0-9\-]*` admits an empty region, as a global resource has.
            "empty-region",
            "arn:aws:sagemaker::123456789012:experiment-trial-component/global".to_string(),
        ),
        (
            // `.*` admits further path segments after the resource type.
            "nested-resource-path",
            "arn:aws:sagemaker:eu-west-1:123456789012:pipeline/my-pipeline/execution/abc"
                .to_string(),
        ),
    ]
}

/// Every way of being malformed, at every position of batches of several sizes: one
/// constant sentence each.
#[tokio::test]
async fn a_malformed_resource_arn_refuses_the_whole_call_with_one_constant_sentence() {
    let router = emulator();
    let good = well_formed_arns()[0].1.clone();

    for (shape, malformed) in malformed_arns() {
        // The same value alone, first, last and in the middle of a longer batch, and
        // twice in one batch: the answer does not vary with position or count.
        let layouts: Vec<(&str, Vec<Value>)> = vec![
            ("alone", vec![query(&malformed)]),
            ("first-of-two", vec![query(&malformed), query(&good)]),
            ("last-of-two", vec![query(&good), query(&malformed)]),
            (
                "middle-of-three",
                vec![query(&good), query(&malformed), query(&good)],
            ),
            (
                "twice-in-four",
                vec![
                    query(&good),
                    query(&malformed),
                    query(&good),
                    query(&malformed),
                ],
            ),
        ];
        for (layout, queries) in layouts {
            let case = format!("{shape}/{layout}");
            let answer = get_metrics(&router, queries).await;
            assert_refused_as_invalid_arn(&case, &answer, &[&malformed, &good]);
        }
    }
}

/// A reference the expression accepts is SERVED: the refusal is about the expression, not
/// about the one recorded resource.
#[tokio::test]
async fn a_well_formed_resource_arn_is_served() {
    let router = emulator();
    for (case, arn) in well_formed_arns() {
        let answer = get_metrics(&router, vec![query(&arn)]).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a well-formed reference is served, got {} ({:?})",
            answer.status, answer.body
        );
        let results = answer.body["MetricQueryResults"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected MetricQueryResults, got {:?}", answer.body));
        assert_eq!(
            results.len(),
            1,
            "case {case:?}: expected one result per query, got {results:?}"
        );
        assert_eq!(
            results[0]["Status"].as_str(),
            Some("Complete"),
            "case {case:?}: nothing was written for this resource, which is Complete and \
             empty, got {:?}",
            results[0]
        );
    }
}

/// The refusal is about the SageMaker-ARN expression alone: every other constraint class
/// — including another member's own `@pattern` — keeps the framework's per-member
/// envelope.
#[tokio::test]
async fn other_constraint_refusals_keep_the_framework_envelope() {
    let router = emulator();
    let good = well_formed_arns()[0].1.clone();

    // case, method, path, body — one per constraint class the model declares elsewhere.
    let cases: Vec<(&str, &str, &str, Value)> = vec![
        (
            // MetricQueryList is modelled @length 1..100.
            "empty-metric-queries",
            "POST",
            "/BatchGetMetrics",
            json!({ "MetricQueries": [] }),
        ),
        (
            // MetricStatistic enumerates no `Median`.
            "metric-stat-outside-the-enum",
            "POST",
            "/BatchGetMetrics",
            json!({ "MetricQueries": [{
                "MetricName": "loss",
                "ResourceArn": good,
                "MetricStat": "Median",
                "Period": "IterationNumber",
                "XAxisType": "IterationNumber",
            }] }),
        ),
        (
            // MetricName is modelled @length 1..255 @pattern '^.+$': another member's
            // pattern, which this service does NOT word itself.
            "metric-name-empty",
            "POST",
            "/BatchGetMetrics",
            json!({ "MetricQueries": [{
                "MetricName": "",
                "ResourceArn": good,
                "MetricStat": "Last",
                "Period": "IterationNumber",
                "XAxisType": "IterationNumber",
            }] }),
        ),
        (
            // ExperimentEntityName is modelled @pattern '^[a-z0-9](-*[a-z0-9]){0,119}$'.
            "trial-component-name-uppercase",
            "PUT",
            "/BatchPutMetrics",
            json!({
                "TrialComponentName": "TC-Arn-Rules",
                "MetricData": [{
                    "MetricName": "loss",
                    "Timestamp": T0,
                    "Step": 0,
                    "Value": 1.0,
                }],
            }),
        ),
        (
            // Step is modelled @range min 0.
            "negative-step",
            "PUT",
            "/BatchPutMetrics",
            json!({
                "TrialComponentName": "tc-arn-rules",
                "MetricData": [{
                    "MetricName": "loss",
                    "Timestamp": T0,
                    "Step": -1,
                    "Value": 1.0,
                }],
            }),
        ),
    ];

    for (case, method, path, body) in cases {
        let answer = call(&router, method, path, &body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a modelled-constraint violation is refused with 400, got {} ({:?})",
            answer.status, answer.body
        );
        assert_ne!(
            answer.message(),
            INVALID_RESOURCE_ARN_MESSAGE,
            "case {case:?}: this is not a resource-reference fault, got {:?}",
            answer.message()
        );
        assert!(
            answer.message().contains(FRAMEWORK_ENVELOPE_OPENING),
            "case {case:?}: expected the framework's per-member envelope, got {:?}",
            answer.message()
        );
    }
}

/// A request that breaks the ARN expression AND another constraint is answered as the
/// resource-reference fault: the reference is resolved before anything else is read.
#[tokio::test]
async fn a_malformed_arn_beside_another_violation_is_still_the_arn_fault() {
    let router = emulator();
    let answer = get_metrics(
        &router,
        vec![json!({
            "MetricName": "",
            "ResourceArn": "not-an-arn",
            "MetricStat": "Median",
            "Period": "IterationNumber",
            "XAxisType": "IterationNumber",
        })],
    )
    .await;
    assert_refused_as_invalid_arn("arn-and-enum-and-name", &answer, &["not-an-arn", "Median"]);
}

/// The two channels stay distinct: a reference the expression ACCEPTS but that names no
/// resource is reported in-band, per query, inside a 200 — and a sibling query is still
/// answered from what `BatchPutMetrics` stored.
#[tokio::test]
async fn a_semantic_reference_failure_stays_an_in_band_per_query_status() {
    let router = emulator();
    let component = "tc-arn-channels";
    let put = call(
        &router,
        "PUT",
        "/BatchPutMetrics",
        &json!({
            "TrialComponentName": component,
            "MetricData": [{
                "MetricName": "loss",
                "Timestamp": T0,
                "Step": 0,
                "Value": 4.0,
            }],
        }),
    )
    .await;
    assert_eq!(
        put.status, 200,
        "BatchPutMetrics must answer 200, got {} ({:?})",
        put.status, put.body
    );
    assert!(
        put.body
            .get("Errors")
            .is_none_or(|errors| errors.as_array().is_none_or(Vec::is_empty)),
        "the batch must be accepted, got {:?}",
        put.body
    );

    let stored = format!("arn:aws:sagemaker:us-east-1:123456789012:experiment-trial-component/{component}");
    // The expression only requires the resource part to CONTAIN a `/`, so this passes it
    // and still names no resource.
    let no_name_segment = "arn:aws:sagemaker:us-east-1:123456789012:experiment-trial-component/";
    let answer = get_metrics(&router, vec![query(no_name_segment), query(&stored)]).await;
    assert_eq!(
        answer.status, 200,
        "a semantic failure is reported inside a 200, got {} ({:?})",
        answer.status, answer.body
    );
    let results = answer.body["MetricQueryResults"]
        .as_array()
        .unwrap_or_else(|| panic!("expected MetricQueryResults, got {:?}", answer.body))
        .clone();
    assert_eq!(
        results.len(),
        2,
        "one result per query, positionally aligned, got {results:?}"
    );
    assert_eq!(
        results[0]["Status"].as_str(),
        Some("ValidationError"),
        "case {no_name_segment:?}: expected a per-query ValidationError, got {:?}",
        results[0]
    );
    assert_eq!(
        results[1]["Status"].as_str(),
        Some("Complete"),
        "case {stored:?}: the sibling query is answered from state, got {:?}",
        results[1]
    );
    assert_eq!(
        results[1]["MetricValues"].as_array().map(Vec::len),
        Some(1),
        "case {stored:?}: expected the one stored point, got {:?}",
        results[1]
    );
    let value = results[1]["MetricValues"][0].as_f64();
    assert_eq!(
        value,
        Some(4.0),
        "case {stored:?}: expected the stored value 4.0, got {value:?}"
    );
}
