// SPDX-License-Identifier: CC0-1.0

//! WHICH SENTENCE an environment-not-found refusal carries, end to end over HTTP through
//! the router this service mounts in production.
//!
//! The rule: an operation the model hosts on the ENVIRONMENT data endpoint (`@endpoint`
//! host prefix `env.`) is addressed to a per-environment front door, so a name that
//! resolves to nothing is refused by that front door with its own FIXED sentence, which
//! names no environment; only an operation hosted on the control-plane endpoint (host
//! prefix `api.`) builds the sentence from the request's `Name`. The status (404) and the
//! wire code (`ResourceNotFoundException`) are the same either way.
//!
//! Each row varies what the rule generalises over: the endpoint the operation is hosted on,
//! the operation on each endpoint (every one addressed by `Name`), the HTTP method and
//! whether the request carries a body, crossed with every form of environment name
//! `EnvironmentName`'s own constraints allow. The host prefix of each row is quoted from
//! the model, and the positive control proves the refusals are the name lookup failing
//! rather than the service answering nothing.

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

/// The two `@endpoint` host prefixes this service's operations are hosted on.
///
/// Source: this service's Smithy model —
/// `smithy_query(query="operation",operation="CreateCliToken,CreateWebLoginToken,InvokeRestApi,GetEnvironment",model_path="services/service-com-amazonaws-mwaa-amazonmwaa/smithy-model/com-amazonaws-mwaa-amazonmwaa.json")`
/// reports `endpoint.hostPrefix` `env.` for the first three and `api.` for the control-plane
/// lookups.
#[derive(Clone, Copy, Debug)]
enum HostPrefix {
    /// `api.` — the control plane, which words the refusal from the request.
    ControlPlane,
    /// `env.` — the per-environment data endpoint, which words it itself.
    EnvironmentData,
}

impl HostPrefix {
    /// The sentence a not-found refusal served by this endpoint carries for `name`.
    fn not_found_message(self, name: &str) -> String {
        match self {
            Self::ControlPlane => state::environment_not_found_message(name),
            Self::EnvironmentData => state::ENVIRONMENT_ENDPOINT_NOT_FOUND_MESSAGE.to_string(),
        }
    }
}

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

    /// The sentence the refusal carries, wherever the endpoint puts it: the `message` of a
    /// serialized exception object, or the root of the data endpoint's document payload.
    fn refusal_message(&self) -> String {
        let body = self.json();
        if let Some(bare) = body.as_str() {
            return bare.to_string();
        }
        body.get("message")
            .or_else(|| body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn member(&self, name: &str) -> String {
        self.json()
            .get(name)
            .and_then(Value::as_str)
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
}

/// Environment names no create in these tests ever stores. They differ along what
/// `EnvironmentName`'s own `@length(1, 80)` and `^[a-zA-Z][0-9a-zA-Z-_]*$` allow: the
/// one-character minimum, mixed case, digits, both separator characters, and the
/// 80-character maximum.
fn unknown_names() -> Vec<String> {
    vec![
        "A".to_string(),
        "NoSuchEnvironment".to_string(),
        "missing-env_7".to_string(),
        "MixedCase-Env_42".to_string(),
        format!("L{}", "o".repeat(79)),
    ]
}

/// One row: the operation, the endpoint the model hosts it on, and how it is called —
/// `(label, host prefix, method, uri template whose `{}` takes the name, body)`.
type Row = (
    &'static str,
    HostPrefix,
    &'static str,
    &'static str,
    Option<Value>,
);

/// Every operation this service addresses by `Name`, on both endpoints.
fn name_addressed_calls() -> Vec<Row> {
    vec![
        (
            "GetEnvironment",
            HostPrefix::ControlPlane,
            "GET",
            "/environments/{}",
            None,
        ),
        (
            "UpdateEnvironment",
            HostPrefix::ControlPlane,
            "PATCH",
            "/environments/{}",
            Some(json!({"MaxWorkers": 2})),
        ),
        (
            "DeleteEnvironment",
            HostPrefix::ControlPlane,
            "DELETE",
            "/environments/{}",
            None,
        ),
        (
            "CreateCliToken",
            HostPrefix::EnvironmentData,
            "POST",
            "/clitoken/{}",
            None,
        ),
        (
            "CreateWebLoginToken",
            HostPrefix::EnvironmentData,
            "POST",
            "/webtoken/{}",
            None,
        ),
        (
            "InvokeRestApi",
            HostPrefix::EnvironmentData,
            "POST",
            "/restapi/{}",
            Some(json!({"Path": "/dags", "Method": "GET"})),
        ),
    ]
}

#[tokio::test]
async fn the_endpoint_hosting_the_operation_decides_the_not_found_wording() {
    let harness = Harness::new();
    // One environment exists, so a refusal below is the name lookup failing and not an
    // empty store answering.
    assert_eq!(
        harness.create("TheOnlyEnvironment").await.status,
        200,
        "the fixture create must succeed"
    );

    for name in unknown_names() {
        for (operation, host_prefix, method, uri, body) in name_addressed_calls() {
            let case = format!("{operation} ({host_prefix:?}) / {name}");
            let answer = harness
                .send(method, &uri.replace("{}", &name), body.clone())
                .await;
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
            let expected = host_prefix.not_found_message(&name);
            assert_eq!(
                answer.refusal_message(),
                expected,
                "case {case:?}: expected the sentence {expected:?}, got {:?}",
                answer.refusal_message()
            );
            // The distinction that is not a matter of example text: an `env.`-hosted
            // refusal is worded by a front door that knows no environment, so the
            // requested name cannot appear anywhere in it, while a control-plane one is
            // built from that very name.
            match host_prefix {
                HostPrefix::EnvironmentData => assert!(
                    !answer.text().contains(name.as_str()),
                    "case {case:?}: the data endpoint's refusal must not echo the name, got {}",
                    answer.text()
                ),
                HostPrefix::ControlPlane => assert!(
                    answer.refusal_message().contains(name.as_str()),
                    "case {case:?}: the control-plane refusal must name the environment, got {:?}",
                    answer.refusal_message()
                ),
            }
        }
    }
}

#[tokio::test]
async fn the_token_operations_still_answer_an_environment_that_exists() {
    let harness = Harness::new();

    // The positive control for the refusals above, over several stored environments: the
    // same operations answer 200 and a token for a name that DOES resolve, so the 404s are
    // the lookup failing.
    for name in ["A", "Stored-Env_1", &format!("S{}", "t".repeat(79))] {
        assert_eq!(
            harness.create(name).await.status,
            200,
            "case {name:?}: the fixture create must succeed"
        );
        for (operation, uri, token_member) in [
            ("CreateCliToken", format!("/clitoken/{name}"), "CliToken"),
            ("CreateWebLoginToken", format!("/webtoken/{name}"), "WebToken"),
        ] {
            let answer = harness.send("POST", &uri, None).await;
            assert_eq!(
                answer.status, 200,
                "case {operation:?}/{name:?}: expected HTTP 200, got {} {}",
                answer.status,
                answer.text()
            );
            assert!(
                !answer.member(token_member).is_empty(),
                "case {operation:?}/{name:?}: expected a {token_member}, got {}",
                answer.text()
            );
            assert!(
                !answer.member("WebServerHostname").is_empty(),
                "case {operation:?}/{name:?}: expected a WebServerHostname, got {}",
                answer.text()
            );
        }
    }
}
