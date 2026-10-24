// SPDX-License-Identifier: CC0-1.0

//! What the environment data endpoint answers for an environment it cannot resolve, end to
//! end over HTTP through the router this service mounts in production.
//!
//! The rule: the operation the model declares as a PROXY to the Apache Airflow REST API of
//! an environment's web server (`InvokeRestApi`, `@endpoint` host prefix `env.`) is refused
//! by the data endpoint's own front door, so its not-found answer is NOT a serialized
//! modelled exception: the body's ROOT is that endpoint's document — a JSON string — and it
//! carries no error-code member, while the status (404) and the `x-amzn-errortype` header a
//! restJson1 client resolves the error shape from stay the modelled ones. Every OTHER
//! answer of the service, including every control-plane operation's not-found refusal and
//! every other answer of the proxied operation itself, stays the serialized exception
//! object.
//!
//! Each table row varies what the rule generalises over — the environment name (every form
//! `EnvironmentName`'s own constraints allow), the Airflow method and path the call names,
//! and whether the request carries a body or query parameters — and the boundary cases vary
//! the operation and the status.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mwaa_amazonmwaa::state;

/// A signed-looking request header so the router's `can_handle` resolves this service by
/// its SigV4 credential scope, the way an SDK request does. Signature verification is off
/// in this harness, so the signature value is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/airflow/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The response header restJson1 resolves an error shape from.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    error_type_header: Option<String>,
    body: Vec<u8>,
}

impl Answer {
    /// The body as a JSON document, whatever its root type is.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!("expected a JSON body, got {error}: {}", self.text())
        })
    }

    /// The error code a JSON body carries in `__type`, or the empty string when the body
    /// names none — which is what a document payload does.
    fn body_error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// The emulator, assembled exactly as the router assembles it: one shared engine behind
/// one mounted service.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mwaa_amazonmwaa::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("a serialisable body"))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "api.airflow.us-east-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let error_type_header = response
            .headers()
            .get(ERROR_TYPE_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        Answer {
            status: response.status().as_u16(),
            error_type_header,
            body: response.into_body().to_vec(),
        }
    }

    /// Create one environment with every optional member absent.
    async fn create(&self, name: &str) -> Answer {
        self.send(
            "PUT",
            &format!("/environments/{name}"),
            Some(json!({
                "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/mwaa-execution"),
                "SourceBucketArn": "arn:aws:s3:::my-airflow-bucket",
                "DagS3Path": "dags",
                "NetworkConfiguration": {
                    "SubnetIds": ["subnet-00000000000000001", "subnet-00000000000000002"],
                    "SecurityGroupIds": ["sg-00000000000000001"],
                },
            })),
        )
        .await
    }

    /// One `InvokeRestApi` call.
    async fn invoke(&self, name: &str, body: Value) -> Answer {
        self.send("POST", &format!("/restapi/{name}"), Some(body)).await
    }
}

/// One row of the table: a label, an environment name no create ever stored, and the
/// `InvokeRestApi` body the call carries.
type Row = (&'static str, String, Value);

/// The rows the rule is asserted over. The names differ along what `EnvironmentName`'s own
/// `@length(1, 80)` and `^[a-zA-Z][0-9a-zA-Z-_]*$` allow (the one-character minimum, mixed
/// case, digits, both separators, the 80-character maximum); the bodies differ along
/// `RestApiMethod`'s own value list (GET | PUT | POST | PATCH | DELETE), the `RestApiPath`
/// `@length(1, 64)` bounds (one character, and 64), and whether `Body` / `QueryParameters`
/// are carried at all.
///
/// Source for both value lists: this service's Smithy model —
/// `smithy_query(query="shape",shape="RestApiMethod,RestApiPath",model_path="services/service-com-amazonaws-mwaa-amazonmwaa/smithy-model/com-amazonaws-mwaa-amazonmwaa.json")`.
fn unresolvable_calls() -> Vec<Row> {
    vec![
        (
            "the shortest name, a bare GET",
            "A".to_string(),
            json!({"Path": "/dags", "Method": "GET"}),
        ),
        (
            "a POST carrying a body",
            "NoSuchEnvironment".to_string(),
            json!({"Path": "/dags/x/dagRuns", "Method": "POST", "Body": {"conf": {}}}),
        ),
        (
            "a PATCH carrying query parameters",
            "missing-env_7".to_string(),
            json!({
                "Path": "/dags/x",
                "Method": "PATCH",
                "QueryParameters": {"update_mask": "is_paused"},
            }),
        ),
        (
            "a DELETE at the one-character path minimum",
            "MixedCase-Env_42".to_string(),
            json!({"Path": "/", "Method": "DELETE"}),
        ),
        (
            "a PUT at the 64-character path maximum",
            format!("L{}", "o".repeat(79)),
            json!({"Path": format!("/{}", "p".repeat(63)), "Method": "PUT"}),
        ),
    ]
}

#[tokio::test]
async fn the_proxied_operation_refuses_an_unknown_environment_with_the_endpoints_document_payload() {
    let harness = Harness::new();
    // One environment exists, so a refusal below is the lookup failing and not an empty
    // store answering.
    assert_eq!(
        harness.create("TheOnlyEnvironment").await.status,
        200,
        "the fixture create must succeed"
    );

    for (case, name, body) in unresolvable_calls() {
        let answer = harness.invoke(&name, body).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404 for a name that resolves to nothing, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.error_type_header.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: restJson1 resolves the shape from {ERROR_TYPE_HEADER}, got {:?}",
            answer.error_type_header
        );
        let document = answer.json();
        assert!(
            document.is_string(),
            "case {case:?}: the payload's root must be a JSON string, got {document:?}"
        );
        assert_eq!(
            answer.body_error_code(),
            "",
            "case {case:?}: the payload must carry no error-code member, got {}",
            answer.text()
        );
        assert_eq!(
            document.as_str(),
            Some(state::ENVIRONMENT_ENDPOINT_NOT_FOUND_MESSAGE),
            "case {case:?}: the payload is the endpoint's own sentence, got {document:?}"
        );
        // The endpoint's refusal is worded by the front door, not by the control plane's
        // request-derived builder, so it does not name the environment asked for.
        assert!(
            !answer.text().contains(name.as_str()),
            "case {case:?}: the payload must not echo the environment name, got {}",
            answer.text()
        );
    }
}

#[tokio::test]
async fn every_other_answer_of_this_service_stays_a_serialized_exception_object() {
    let harness = Harness::new();
    assert_eq!(
        harness.create("TheOnlyEnvironment").await.status,
        200,
        "the fixture create must succeed"
    );

    // The operations addressed by the same `Name` whose refusal is the modelled exception
    // OBJECT: the control-plane lookups, which word it from the request, and the two
    // `env.`-hosted token operations, which carry the endpoint's own fixed sentence in that
    // object's `message` (the wording rule itself is pinned by
    // `tests/all/env_endpoint_not_found_wording.rs`).
    let name = "NoSuchEnvironment";
    for (operation, method, uri, body, message) in [
        (
            "GetEnvironment",
            "GET",
            format!("/environments/{name}"),
            None,
            state::environment_not_found_message(name),
        ),
        (
            "UpdateEnvironment",
            "PATCH",
            format!("/environments/{name}"),
            Some(json!({"MaxWorkers": 2})),
            state::environment_not_found_message(name),
        ),
        (
            "DeleteEnvironment",
            "DELETE",
            format!("/environments/{name}"),
            None,
            state::environment_not_found_message(name),
        ),
        (
            "CreateCliToken",
            "POST",
            format!("/clitoken/{name}"),
            None,
            state::ENVIRONMENT_ENDPOINT_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "CreateWebLoginToken",
            "POST",
            format!("/webtoken/{name}"),
            None,
            state::ENVIRONMENT_ENDPOINT_NOT_FOUND_MESSAGE.to_string(),
        ),
    ] {
        let answer = harness.send(method, &uri, body).await;
        assert_eq!(
            answer.status, 404,
            "case {operation:?}: expected HTTP 404, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.body_error_code(),
            "ResourceNotFoundException",
            "case {operation:?}: the body must name the modelled code, got {}",
            answer.text()
        );
        assert_eq!(
            answer.json().get("message").and_then(Value::as_str),
            Some(message.as_str()),
            "case {operation:?}: expected the message {message:?}, got {}",
            answer.text()
        );
    }

    // And the proxied operation's OTHER answers: the scaffold's 501 for an environment that
    // exists (no Apache Airflow runs in this process) and the framework's 400 for an input
    // outside its modelled constraints both stay exception objects.
    for (case, name, body, status, code) in [
        (
            "an environment that exists",
            "TheOnlyEnvironment",
            json!({"Path": "/dags", "Method": "GET"}),
            501u16,
            "NotImplemented",
        ),
        (
            "a Path past @length(1, 64)",
            "TheOnlyEnvironment",
            json!({"Path": "/".repeat(65), "Method": "GET"}),
            400,
            "ValidationException",
        ),
        (
            "a Path past @length(1, 64) on an unknown environment",
            "NoSuchEnvironment",
            json!({"Path": "/".repeat(65), "Method": "GET"}),
            400,
            "ValidationException",
        ),
    ] {
        let answer = harness.invoke(name, body).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.body_error_code(),
            code,
            "case {case:?}: expected the body to name {code}, got {}",
            answer.text()
        );
    }
}
