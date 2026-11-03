// SPDX-License-Identifier: CC0-1.0

//! WHICH SENTENCE an environment-not-found refusal carries, end to end over HTTP through
//! the router this service mounts in production.
//!
//! The rule: WHO refuses decides the sentence, and the MODEL decides the shape it travels
//! in. An operation the model hosts on the ENVIRONMENT data endpoint (`@endpoint` host
//! prefix `env.`) is addressed to a per-environment front door, so a name that resolves to
//! nothing is refused by that front door with its own FIXED sentence, which names no
//! environment. On every OTHER endpoint — the control plane (`api.`) and the internal
//! operations endpoint (`ops.`) — the refusal is raised by the layer that owns this
//! service's modelled errors, which builds the sentence from the name the request asked
//! for, in the error shape the model declares for that operation: `ResourceNotFoundException`
//! at 404 where the operation models it, and for the `ops.`-hosted operation, which models
//! no 404, its one client error `ValidationException` at 400.
//!
//! Each row varies what the rule generalises over: the endpoint the operation is hosted on
//! (all three classes the model declares), the operation on each endpoint (every one
//! addressed by an environment name), the HTTP method and whether the request carries a
//! body, crossed with every form of environment name `EnvironmentName`'s own constraints
//! allow. Which endpoint hosts a row's operation is read from the crate's endpoint census
//! (`service::endpoints`, checked against the shipped model by
//! `tests/all/endpoint_host_prefix_census.rs`), so this suite cannot disagree with the model
//! about it, and the positive control proves the refusals are the name lookup failing rather
//! than the service answering nothing. WHICH SHAPE the sentence travels in is a separate
//! rule with its own suites (`tests/all/env_endpoint_front_door.rs` for the data endpoint,
//! `tests/all/env_endpoint_payload.rs` for every other refusal).

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mwaa_amazonmwaa::service::endpoints::{self, Endpoint};
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

/// The sentence a not-found refusal carries for `name` when the model hosts the operation on
/// `endpoint`: the layer that refuses is what decides it.
fn not_found_message(endpoint: Endpoint, name: &str) -> String {
    match endpoint {
        // Refused by the per-environment front door, ahead of this service's own code: its
        // sentence is fixed and names no environment.
        Endpoint::EnvironmentData => state::ENVIRONMENT_ENDPOINT_NOT_FOUND_MESSAGE.to_string(),
        // Refused by the layer that owns the modelled errors, which names the environment the
        // request asked for.
        Endpoint::ControlPlane | Endpoint::Operations => format!("Environment {name} not found."),
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

    /// The sentence the refusal carries, the way an SDK's JSON error parser reads it: the
    /// `message` member of a document body, and the body itself when the body is not a
    /// document — which is how the proxied operation relays the data endpoint's own refusal
    /// (the shape each refusal travels in is pinned by
    /// `tests/all/env_endpoint_front_door.rs`).
    fn refusal_message(&self) -> String {
        match serde_json::from_slice(&self.body).unwrap_or(Value::Null) {
            Value::Object(object) => object
                .get("message")
                .or_else(|| object.get("Message"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            _ => self.text(),
        }
    }

    /// The wire error code a restJson1 client resolves: the `x-amzn-errortype` header first,
    /// then the body's own discriminator.
    fn error_code(&self) -> String {
        let named = self.error_type_header.clone().unwrap_or_else(|| {
            self.json()
                .get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        });
        named
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .split(':')
            .next()
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

/// A `MetricData` batch every member of which satisfies what `PublishMetrics` publishes, so
/// a refusal of such a request is the environment lookup and nothing else.
fn metric_batch() -> Value {
    json!({"MetricData": [{"MetricName": "CpuUtilization", "Timestamp": 1_700_000_000, "Value": 1.0}]})
}

/// One row: the operation, the endpoint the model hosts it on, how it is called, and the
/// status and wire code the model gives its refusal — `(label, endpoint, method, uri template
/// whose `{}` takes the name, body, status, code)`.
type Row = (
    &'static str,
    Endpoint,
    &'static str,
    &'static str,
    Option<Value>,
    u16,
    &'static str,
);

/// Every operation this service addresses by an environment name, across all three endpoint
/// classes.
///
/// The status and code of each row are the ones the model declares for that operation:
/// `ResourceNotFoundException` (`@httpError` 404) wherever the operation models it, and for
/// `PublishMetrics` — which models only `InternalServerException` and `ValidationException` —
/// the latter, its one client error, at 400.
fn name_addressed_calls() -> Vec<Row> {
    vec![
        (
            "GetEnvironment",
            Endpoint::ControlPlane,
            "GET",
            "/environments/{}",
            None,
            404,
            "ResourceNotFoundException",
        ),
        (
            "UpdateEnvironment",
            Endpoint::ControlPlane,
            "PATCH",
            "/environments/{}",
            Some(json!({"MaxWorkers": 2})),
            404,
            "ResourceNotFoundException",
        ),
        (
            "DeleteEnvironment",
            Endpoint::ControlPlane,
            "DELETE",
            "/environments/{}",
            None,
            404,
            "ResourceNotFoundException",
        ),
        (
            "CreateCliToken",
            Endpoint::EnvironmentData,
            "POST",
            "/clitoken/{}",
            None,
            404,
            "ResourceNotFoundException",
        ),
        (
            "CreateWebLoginToken",
            Endpoint::EnvironmentData,
            "POST",
            "/webtoken/{}",
            None,
            404,
            "ResourceNotFoundException",
        ),
        (
            "InvokeRestApi",
            Endpoint::EnvironmentData,
            "POST",
            "/restapi/{}",
            Some(json!({"Path": "/dags", "Method": "GET"})),
            404,
            "ResourceNotFoundException",
        ),
        (
            "PublishMetrics",
            Endpoint::Operations,
            "POST",
            "/metrics/environments/{}",
            Some(metric_batch()),
            400,
            "ValidationException",
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

    // Every class the census records must be exercised, so no class is silently missing.
    let mut classes_seen: BTreeSet<&str> = BTreeSet::new();

    for name in unknown_names() {
        for (operation, endpoint, method, uri, body, status, code) in name_addressed_calls() {
            classes_seen.insert(endpoint.host_prefix());
            let case = format!("{operation} ({}) / {name}", endpoint.host_prefix());
            assert_eq!(
                endpoints::endpoint_of(operation),
                Some(endpoint),
                "case {case:?}: the row's endpoint must be the one the census records, got {:?}",
                endpoints::endpoint_of(operation)
            );
            let answer = harness
                .send(method, &uri.replace("{}", &name), body.clone())
                .await;
            assert_eq!(
                answer.status, status,
                "case {case:?}: expected the modelled status {status} for a name that resolves \
                 to nothing, got {} {}",
                answer.status,
                answer.text()
            );
            assert_eq!(
                answer.error_code(),
                code,
                "case {case:?}: restJson1 resolves the shape from {ERROR_TYPE_HEADER}, expected \
                 {code:?}, got {:?}",
                answer.error_code()
            );
            let expected = not_found_message(endpoint, &name);
            assert_eq!(
                answer.refusal_message(),
                expected,
                "case {case:?}: expected the sentence {expected:?}, got {:?}",
                answer.refusal_message()
            );
            // The distinction that is not a matter of example text: an `env.`-hosted
            // refusal is worded by a front door that knows no environment, so the
            // requested name cannot appear anywhere in it, while a refusal this service's
            // own layer raises is built from that very name.
            match endpoint {
                Endpoint::EnvironmentData => assert!(
                    !answer.text().contains(name.as_str()),
                    "case {case:?}: the data endpoint's refusal must not echo the name, got {}",
                    answer.text()
                ),
                Endpoint::ControlPlane | Endpoint::Operations => assert!(
                    answer.refusal_message().contains(name.as_str()),
                    "case {case:?}: a refusal this service's own layer raises must name the \
                     environment, got {:?}",
                    answer.refusal_message()
                ),
            }
        }
    }

    let recorded_classes: BTreeSet<&str> = endpoints::OPERATION_ENDPOINTS
        .iter()
        .map(|(_, endpoint)| endpoint.host_prefix())
        .collect();
    assert_eq!(
        classes_seen, recorded_classes,
        "every endpoint class the census records must be exercised above, covered \
         {classes_seen:?}"
    );
}

#[tokio::test]
async fn an_environment_that_exists_is_still_answered_on_every_endpoint() {
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

        // The same control for the `ops.`-hosted operation, whose refusal above is the one
        // client error its model declares rather than a 404: it is SERVED for a name that
        // resolves.
        let published = harness
            .send(
                "POST",
                &format!("/metrics/environments/{name}"),
                Some(metric_batch()),
            )
            .await;
        assert_eq!(
            published.status, 200,
            "case \"PublishMetrics\"/{name:?}: expected HTTP 200 for a stored environment, got {} {}",
            published.status,
            published.text()
        );
    }
}
