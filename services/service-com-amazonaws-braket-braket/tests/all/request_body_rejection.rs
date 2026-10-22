// SPDX-License-Identifier: CC0-1.0

//! How Braket answers a request it refuses BEFORE any handler runs, over HTTP.
//!
//! The rule under test: a request whose JSON BODY violates the operation's modelled
//! input is rejected by Braket's front door, ahead of the service — HTTP 400, error
//! type `BadRequestException`, and one fixed sentence about the body, with none of the
//! per-member constraint prose the service's own `ValidationException` carries. A
//! refusal that names only values bound OUTSIDE the body (a URI label, a query-string
//! parameter) is not body validation and keeps the modelled `ValidationException`.
//!
//! Where a value travels is decided per OPERATION, off that operation's own input
//! schema, so the one member name Braket binds both ways — `deviceArn`, a body member
//! of the creates and `GetDevice`'s URI label — is answered differently by each:
//! the two suites below carry that pair.
//!
//! Each table row differs along what the rule generalises over: the operation, the
//! member, how deeply it is nested, and which kind of modelled rule it breaks
//! (`@required`, `@length` min and max, `@pattern`, `@range`, an enum's value set).
//! The two requests the issue recorded are one row each.
//!
//! Every assertion goes through the router, so what is pinned is what a Braket client
//! sees. The two creates the issue cites are still scaffolded handlers; that is
//! irrelevant here, because this refusal happens before the handler is reached — which
//! the control suite at the end shows by sending bodies the model ACCEPTS.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The one percent-encoder in this test target: an `@httpLabel` ARN carries `:` and
// `/`, so an unencoded one would be read as several path segments.
use crate::quantum_task_loop::encode;

/// The sentence the body-rejection layer answers with. A fixed constant of that layer:
/// it names neither the member nor the value.
const INVALID_BODY: &str = "Invalid request body";

/// One response, reduced to what a client branches on.
struct Reply {
    status: u16,
    /// The wire error code (`x-amzn-errortype`, else the body's `__type`), if any.
    error: Option<String>,
    /// The error body's `message` member, if any.
    message: Option<String>,
    body: Value,
}

/// A Braket emulator behind the production router.
struct Braket {
    router: MultiServiceRouter,
}

impl Braket {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(service_com_amazonaws_braket_braket::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/eu-west-1/braket/aws4_request, \
                 SignedHeaders=host, Signature=00",
            )
            .body(payload)
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|v| v.to_str().ok())
            .map(ToString::to_string);
        let bytes = response.into_body();
        let body: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                panic!(
                    "response body is not JSON ({e}): {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        let error = header_error.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(|t| t.rsplit('#').next().unwrap_or(t).to_string())
        });
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        Reply {
            status,
            error,
            message,
            body,
        }
    }
}

/// A `CreateSpendingLimit` body every modelled rule accepts. Rows mutate one member of
/// it, so each row's only violation is the one it names.
fn spending_limit_body() -> Value {
    json!({
        "clientToken": "tok-spending-limit",
        "deviceArn": "arn:aws:braket:::device/qpu/ionq/Aria-1",
        "spendingLimit": "100.50",
    })
}

/// A `CreateJob` body every modelled rule accepts, built from the model's own patterns
/// (`smithy_query(query="operation",operation="CreateJob")` and the `ContainerImage`,
/// `JobOutputDataConfig`, `InstanceConfig` and `DeviceConfig` shapes).
fn job_body() -> Value {
    json!({
        "clientToken": "tok-create-job",
        "jobName": "valid-job-1",
        "roleArn": "arn:aws:iam::123456789012:role/BraketJobsRole",
        "algorithmSpecification": {
            "containerImage": {
                "uri": "123456789012.dkr.ecr.us-west-2.amazonaws.com/braket:latest"
            }
        },
        "outputDataConfig": { "s3Path": "s3://bucket/prefix" },
        "instanceConfig": { "instanceType": "ml.m5.large", "volumeSizeInGb": 30 },
        "deviceConfig": { "device": "arn:aws:braket:::device/qpu/ionq/Aria-1" },
    })
}

/// A `CreateQuantumTask` body every modelled rule accepts.
fn quantum_task_body() -> Value {
    json!({
        "clientToken": "tok-quantum-task",
        "deviceArn": "arn:aws:braket:::device/quantum-simulator/amazon/sv1",
        "shots": 1,
        "outputS3Bucket": "amazon-braket-results",
        "outputS3KeyPrefix": "tasks",
        "action": "{\"braketSchemaHeader\":{\"name\":\"a\",\"version\":\"1\"}}",
    })
}

/// A body member outside its modelled rules is rejected as the body itself: 400,
/// `BadRequestException`, the fixed sentence — for every operation that takes a body,
/// every kind of rule, and a nested member as well as a top-level one.
#[tokio::test]
async fn a_body_outside_the_modelled_input_is_rejected_as_a_bad_request() {
    let braket = Braket::new();

    // (case, method, path, body) — one row per (member, rule kind) pair the class
    // spans. The first row of each create is the request the issue recorded.
    let mut rows: Vec<(&str, &str, &str, Value)> = Vec::new();

    let mut body = spending_limit_body();
    body["spendingLimit"] = json!("not-a-number");
    rows.push((
        "CreateSpendingLimit spendingLimit @pattern",
        "POST",
        "/spending-limit",
        body,
    ));

    let mut body = spending_limit_body();
    body["spendingLimit"] = json!("");
    rows.push((
        "CreateSpendingLimit spendingLimit @length min 1",
        "POST",
        "/spending-limit",
        body,
    ));

    let mut body = spending_limit_body();
    body.as_object_mut().expect("an object").remove("spendingLimit");
    rows.push((
        "CreateSpendingLimit spendingLimit @required absent",
        "POST",
        "/spending-limit",
        body,
    ));

    let mut body = spending_limit_body();
    body["clientToken"] = json!("t".repeat(65));
    rows.push((
        "CreateSpendingLimit clientToken @length max 64",
        "POST",
        "/spending-limit",
        body,
    ));

    let mut body = spending_limit_body();
    body["deviceArn"] = json!("");
    rows.push((
        "CreateSpendingLimit deviceArn @length min 1",
        "POST",
        "/spending-limit",
        body,
    ));

    let mut body = job_body();
    body["jobName"] = json!("invalid_job_name_!!");
    rows.push(("CreateJob jobName @pattern", "POST", "/job", body));

    let mut body = job_body();
    body["jobName"] = json!("j".repeat(51));
    rows.push(("CreateJob jobName @length max 50", "POST", "/job", body));

    let mut body = job_body();
    body["roleArn"] = json!("not-a-role-arn");
    rows.push(("CreateJob roleArn @pattern", "POST", "/job", body));

    let mut body = job_body();
    body["outputDataConfig"] = json!({ "s3Path": "ftp://bucket/prefix" });
    rows.push((
        "CreateJob outputDataConfig.s3Path @pattern (nested)",
        "POST",
        "/job",
        body,
    ));

    let mut body = job_body();
    body["instanceConfig"] = json!({ "instanceType": "ml.m5.large", "volumeSizeInGb": 0 });
    rows.push((
        "CreateJob instanceConfig.volumeSizeInGb @range min 1 (nested)",
        "POST",
        "/job",
        body,
    ));

    let mut body = job_body();
    body.as_object_mut().expect("an object").remove("deviceConfig");
    rows.push((
        "CreateJob deviceConfig @required absent",
        "POST",
        "/job",
        body,
    ));

    let mut body = quantum_task_body();
    body["shots"] = json!(-1);
    rows.push((
        "CreateQuantumTask shots @range min 0",
        "POST",
        "/quantum-task",
        body,
    ));

    let mut body = quantum_task_body();
    body["outputS3Bucket"] = json!("ab");
    rows.push((
        "CreateQuantumTask outputS3Bucket @length min 3",
        "POST",
        "/quantum-task",
        body,
    ));

    rows.push((
        "SearchDevices maxResults @range min 1",
        "POST",
        "/devices",
        json!({ "filters": [], "maxResults": 0 }),
    ));
    rows.push((
        "SearchDevices maxResults @range max 100",
        "POST",
        "/devices",
        json!({ "filters": [], "maxResults": 101 }),
    ));
    rows.push((
        "SearchDevices filters @required absent",
        "POST",
        "/devices",
        json!({}),
    ));
    rows.push((
        "SearchJobs filters @required absent",
        "POST",
        "/jobs",
        json!({ "maxResults": 10 }),
    ));
    rows.push((
        "TagResource tags @required absent",
        "POST",
        "/tags/arn%3Aaws%3Abraket%3Aeu-west-1%3A000000000000%3Aquantum-task%2Ft1",
        json!({}),
    ));

    for (case, method, path, body) in rows {
        let reply = braket.call(method, path, Some(body)).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {:?} in {}",
            reply.error,
            reply.body
        );
        assert_eq!(
            reply.message.as_deref(),
            Some(INVALID_BODY),
            "case {case:?}: expected the fixed body-rejection sentence, got {:?}",
            reply.message
        );
        assert!(
            !reply.body.to_string().contains("failed to satisfy constraint"),
            "case {case:?}: the body rejection must not carry per-member constraint \
             prose, got {}",
            reply.body
        );
    }
}

/// A refusal that names only a value bound OUTSIDE the body is not body validation: it
/// keeps the service's own modelled `ValidationException` and its constraint prose.
#[tokio::test]
async fn a_refusal_outside_the_body_keeps_the_modelled_validation_exception() {
    let braket = Braket::new();

    // (case, method, path, body) — one row per operation whose only violable member is
    // bound to the URI path or the query string. The `GetDevice` row is the pair of
    // the `CreateSpendingLimit deviceArn` row in the suite above: ONE member name,
    // answered by the binding the OPERATION declares for it — body in the create, URI
    // label in the read.
    let long_arn = "a".repeat(300);
    let rows: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "GetDevice deviceArn @length max 256 (URI label)",
            "GET",
            format!("/device/{long_arn}"),
            None,
        ),
        (
            "GetJob jobArn @pattern (URI label)",
            "GET",
            "/job/not-an-arn".to_string(),
            None,
        ),
        (
            "CancelJob jobArn @pattern (URI label)",
            "PUT",
            "/job/not-an-arn/cancel".to_string(),
            None,
        ),
        (
            "DeleteSpendingLimit spendingLimitArn @pattern (URI label)",
            "DELETE",
            "/spending-limit/not-an-arn/delete".to_string(),
            None,
        ),
        (
            "GetQuantumTask additionalAttributeNames outside its enum (query string)",
            "GET",
            "/quantum-task/arn%3Aaws%3Abraket%3Aeu-west-1%3A000000000000%3Aquantum-task%2Ft1\
             ?additionalAttributeNames=Bogus"
                .to_string(),
            None,
        ),
    ];

    for (case, method, path, body) in rows {
        let reply = braket.call(method, &path, body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} in {}",
            reply.error,
            reply.body
        );
        assert_ne!(
            reply.message.as_deref(),
            Some(INVALID_BODY),
            "case {case:?}: a refusal outside the body must not be worded as a body \
             rejection, got {:?}",
            reply.message
        );
    }
}

/// The control: a body the model ACCEPTS is never answered as an invalid body — the
/// request reaches its operation, which answers for itself (a served operation with its
/// own response, a scaffolded one with 501).
#[tokio::test]
async fn a_body_the_model_accepts_is_never_rejected_as_a_bad_request() {
    let braket = Braket::new();

    // (case, method, path, body)
    let rows: Vec<(&str, &str, &str, Value)> = vec![
        (
            "SearchDevices",
            "POST",
            "/devices",
            json!({ "filters": [], "maxResults": 10 }),
        ),
        (
            "CreateQuantumTask",
            "POST",
            "/quantum-task",
            quantum_task_body(),
        ),
        (
            "CreateSpendingLimit",
            "POST",
            "/spending-limit",
            spending_limit_body(),
        ),
        ("CreateJob", "POST", "/job", job_body()),
    ];

    for (case, method, path, body) in rows {
        let reply = braket.call(method, path, Some(body)).await;
        assert_ne!(
            reply.error.as_deref(),
            Some("BadRequestException"),
            "case {case:?}: a body the model accepts must reach its operation, got {} {}",
            reply.status,
            reply.body
        );
        assert_ne!(
            reply.message.as_deref(),
            Some(INVALID_BODY),
            "case {case:?}: a body the model accepts must not be worded as an invalid \
             body, got {} {}",
            reply.status,
            reply.body
        );
    }
}

/// An ARN-shaped PARAMETER whose value is not a valid ARN is refused by naming THAT
/// PARAMETER: `"<parameter> parameter is not valid."`, as the modelled
/// `ValidationException`.
///
/// The class is every member bound outside the body whose target carries an ARN
/// `@pattern` — `jobArn` on `GetJob` and `CancelJob`, `spendingLimitArn` on
/// `DeleteSpendingLimit` and `UpdateSpendingLimit`
/// (`smithy_query(query="operation",operation="GetJob,CancelJob,DeleteSpendingLimit,UpdateSpendingLimit")`).
/// Rows differ in the operation, the method, the parameter and the way the value misses
/// the pattern.
#[tokio::test]
async fn an_invalid_arn_parameter_is_refused_by_naming_the_parameter() {
    let braket = Braket::new();

    // (case, parameter, method, path, body)
    let rows: Vec<(&str, &str, &str, String, Option<Value>)> = vec![
        (
            "GetJob, a value that is not an ARN at all",
            "jobArn",
            "GET",
            "/job/not-an-arn".to_string(),
            None,
        ),
        (
            "GetJob, an ARN naming another resource type",
            "jobArn",
            "GET",
            format!(
                "/job/{}",
                encode("arn:aws:braket:us-west-2:000000000000:not-a-job-resource")
            ),
            None,
        ),
        (
            "GetJob, an ARN of another service",
            "jobArn",
            "GET",
            format!(
                "/job/{}",
                encode("arn:aws:sagemaker:us-west-2:000000000000:job/j1")
            ),
            None,
        ),
        (
            "CancelJob, the same parameter on another operation",
            "jobArn",
            "PUT",
            "/job/not-an-arn/cancel".to_string(),
            None,
        ),
        (
            "DeleteSpendingLimit, another ARN parameter",
            "spendingLimitArn",
            "DELETE",
            "/spending-limit/not-an-arn/delete".to_string(),
            None,
        ),
        (
            "UpdateSpendingLimit, another ARN parameter with a body the model accepts",
            "spendingLimitArn",
            "PATCH",
            "/spending-limit/not-an-arn/update".to_string(),
            Some(json!({ "clientToken": "tok-update" })),
        ),
    ];

    for (case, parameter, method, path, body) in rows {
        let reply = braket.call(method, &path, body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected the modelled ValidationException, got {:?} in {}",
            reply.error,
            reply.body
        );
        assert_eq!(
            reply.message.as_deref(),
            Some(format!("{parameter} parameter is not valid.").as_str()),
            "case {case:?}: expected the refusal to name the parameter, got {:?}",
            reply.message
        );
    }

    // The control: a parameter that breaks some OTHER constraint failed something else,
    // and keeps the framework's wording, which states the constraint it broke.
    let long = "a".repeat(300);
    let reply = braket.call("GET", &format!("/device/{long}"), None).await;
    assert_eq!(
        reply.error.as_deref(),
        Some("ValidationException"),
        "case a label past its @length bound: expected ValidationException, got {:?} in {}",
        reply.error,
        reply.body
    );
    let message = reply.message.unwrap_or_default();
    assert!(
        message.contains("failed to satisfy constraint") && message.contains("deviceArn"),
        "case a label past its @length bound: expected the constraint wording naming the \
         member, got {message:?}"
    );
}

