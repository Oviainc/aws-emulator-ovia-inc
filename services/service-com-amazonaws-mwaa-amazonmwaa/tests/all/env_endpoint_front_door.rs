// SPDX-License-Identifier: CC0-1.0

//! What the ENVIRONMENT data endpoint answers for an environment it cannot resolve, end to
//! end over HTTP through the router this service mounts in production.
//!
//! The rule: the operations the model hosts on that endpoint (`@endpoint` host prefix
//! `env.`) are addressed to a per-environment front door, which decides the SENTENCE of a
//! name it cannot resolve — its own fixed one, naming no environment — while WHO answers
//! the client decides the shape that sentence travels in. The operations this service
//! SERVES itself (the two that mint a token for the environment's web server) answer
//! through its own modelled error channel, so their 404 is the protocol's error DOCUMENT: a
//! JSON object carrying the sentence in `message`, which is where an SDK reads it. The
//! operation that PROXIES the environment's own Apache Airflow REST API relays what the
//! endpoint it called answered, and there is no Apache Airflow API to call for a name that
//! resolves to nothing — so what it relays is that front door's own refusal, whose body is
//! the sentence and nothing else. Both carry the modelled code in `x-amzn-errortype`, which
//! is where restJson1 resolves an error shape from, so a client branching on the code reads
//! `ResourceNotFoundException` either way.
//!
//! Everything the layers BEHIND the front door refuse arrives in the protocol's document —
//! a request outside a modelled constraint, an Apache Airflow call the environment's own API
//! refused, a control-plane lookup of an unknown name — which the second test asserts over
//! the same operations, so the one body outside the document is the relayed refusal and not
//! "every 4xx of this endpoint". That document across every other operation has its own
//! suite (`tests/all/env_endpoint_payload.rs`), and WHICH sentence each condition is worded
//! with a third (`tests/all/env_endpoint_not_found_wording.rs`).
//!
//! Each row of the first table varies what the rule generalises over: the operation (every
//! one the model hosts on that endpoint, on both sides of the split), the environment name
//! (every form `EnvironmentName`'s own constraints allow), and — for the proxied operation —
//! the Apache Airflow method, path and payload the call names.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mwaa_amazonmwaa::service::endpoints::{Endpoint, OPERATION_ENDPOINTS};
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

/// The sentence the environment data endpoint's front door words a name it cannot resolve
/// with, spelled out here rather than read from the service so this suite pins the text the
/// endpoint really answers with and not whatever the implementation currently holds.
const ENDPOINT_NOT_FOUND_SENTENCE: &str = "Environment not found";

/// The members a JSON error document can carry an error code in, in the order an SDK's
/// restJson1 parser consults them when the header is absent.
const DISCRIMINATOR_MEMBERS: [&str; 3] = ["__type", "code", "Code"];

/// The members it can carry the sentence in.
const MESSAGE_MEMBERS: [&str; 2] = ["message", "Message"];

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    error_type_header: Option<String>,
    body: Vec<u8>,
}

impl Answer {
    /// The body as a JSON document when it is one, and `Value::Null` when it is not.
    fn document(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The code and the sentence an SDK's restJson1 error parser reads out of this answer:
    /// the code from the header (falling back to a document's discriminator), the sentence
    /// from a document's `message` member, or from the body itself when the body is not a
    /// document.
    fn parsed_error(&self) -> (String, String) {
        let document = self.document();
        let object = document.as_object();
        let code = self
            .error_type_header
            .clone()
            .or_else(|| {
                object.and_then(|object| {
                    DISCRIMINATOR_MEMBERS
                        .iter()
                        .find_map(|member| object.get(*member).and_then(Value::as_str))
                        .map(str::to_string)
                })
            })
            .unwrap_or_default();
        let message = match object {
            Some(object) => object
                .get("message")
                .or_else(|| object.get("Message"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            None => self.text(),
        };
        (shape_name(&code), message)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// The bare shape name a code carries: it may arrive namespaced
/// (`com.amazonaws.mwaa#ResourceNotFoundException`) or with a trailing locator, and neither
/// is part of the code — the same normalisation an SDK applies.
fn shape_name(raw: &str) -> String {
    let after_namespace = raw.trim().rsplit(['#', '/']).next().unwrap_or(raw);
    after_namespace
        .split(':')
        .next()
        .unwrap_or(after_namespace)
        .to_string()
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
///
/// Source: this service's Smithy model —
/// `smithy_query(query="shape",shape="EnvironmentName",model_path="services/service-com-amazonaws-mwaa-amazonmwaa/smithy-model/com-amazonaws-mwaa-amazonmwaa.json")`.
fn unknown_names() -> Vec<String> {
    vec![
        "A".to_string(),
        "NoSuchEnvironment".to_string(),
        "missing-env_7".to_string(),
        "MixedCase-Env_42".to_string(),
        format!("L{}", "o".repeat(79)),
    ]
}

/// One call of an operation the environment data endpoint hosts:
/// `(label, method, uri template whose `{}` takes the name, body, whether the operation
/// RELAYS another API's answer rather than serving the caller itself)`.
type Call = (
    &'static str,
    &'static str,
    &'static str,
    Option<Value>,
    bool,
);

/// Every operation the model hosts on the environment data endpoint, each called the way
/// its HTTP binding says, with the side of the split it falls on. The proxied operation
/// appears several times, differing along `RestApiMethod`'s own value list (DELETE | GET |
/// PATCH | POST | PUT), the `RestApiPath` `@length(1, 64)` bounds (one character, and 64)
/// and whether `Body` / `QueryParameters` are carried at all.
///
/// Source for both value lists: this service's Smithy model —
/// `smithy_query(query="shape",shape="RestApiMethod,RestApiPath",model_path="services/service-com-amazonaws-mwaa-amazonmwaa/smithy-model/com-amazonaws-mwaa-amazonmwaa.json")`.
/// WHICH operations the data endpoint hosts is not restated here: it is read from the crate's
/// endpoint census (`service::endpoints`, checked against the shipped model by
/// `tests/all/endpoint_host_prefix_census.rs`) in the test below, which fails if this suite
/// leaves one of them out.
fn environment_data_calls() -> Vec<Call> {
    vec![
        ("CreateCliToken", "POST", "/clitoken/{}", None, false),
        ("CreateWebLoginToken", "POST", "/webtoken/{}", None, false),
        (
            "InvokeRestApi, a bare GET",
            "POST",
            "/restapi/{}",
            Some(json!({"Path": "/dags", "Method": "GET"})),
            true,
        ),
        (
            "InvokeRestApi, a POST carrying a body",
            "POST",
            "/restapi/{}",
            Some(json!({"Path": "/dags/x/dagRuns", "Method": "POST", "Body": {"conf": {}}})),
            true,
        ),
        (
            "InvokeRestApi, a PATCH carrying query parameters",
            "POST",
            "/restapi/{}",
            Some(json!({
                "Path": "/dags/x",
                "Method": "PATCH",
                "QueryParameters": {"update_mask": "is_paused"},
            })),
            true,
        ),
        (
            "InvokeRestApi, a DELETE at the one-character path minimum",
            "POST",
            "/restapi/{}",
            Some(json!({"Path": "/", "Method": "DELETE"})),
            true,
        ),
        (
            "InvokeRestApi, a PUT at the 64-character path maximum",
            "POST",
            "/restapi/{}",
            Some(json!({"Path": format!("/{}", "p".repeat(63)), "Method": "PUT"})),
            true,
        ),
    ]
}

#[tokio::test]
async fn the_refusal_of_an_unresolvable_name_arrives_in_the_shape_its_answerer_sends() {
    let harness = Harness::new();
    // One environment exists, so a refusal below is the name lookup failing and not an
    // empty store answering.
    assert_eq!(
        harness.create("TheOnlyEnvironment").await.status,
        200,
        "the fixture create must succeed"
    );

    // The class this suite is about, read from the crate's census rather than restated: every
    // operation the model hosts on the data endpoint must be exercised by a row below.
    for (operation, _) in OPERATION_ENDPOINTS
        .iter()
        .filter(|(_, endpoint)| *endpoint == Endpoint::EnvironmentData)
    {
        assert!(
            environment_data_calls()
                .iter()
                .any(|(label, ..)| label.starts_with(operation)),
            "the suite must exercise every operation the census hosts on the data endpoint, \
             and {operation:?} has no row"
        );
    }

    for name in unknown_names() {
        for (operation, method, uri, body, relays) in environment_data_calls() {
            let case = format!("{operation} / {name}");
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
                "case {case:?}: the code travels in {ERROR_TYPE_HEADER}, got {:?}",
                answer.error_type_header
            );
            if relays {
                // The operation relays what the endpoint it called answered, so the body is
                // that front door's own: the sentence, byte for byte.
                assert_eq!(
                    answer.text(),
                    ENDPOINT_NOT_FOUND_SENTENCE,
                    "case {case:?}: expected the relayed sentence as the whole body, got {:?}",
                    answer.text()
                );
                // Which it cannot be while also being an error document: nothing wraps it,
                // so there is nowhere in the body for a discriminator to sit.
                assert!(
                    !answer.document().is_object(),
                    "case {case:?}: the relayed body must not be a JSON document, got {:?}",
                    answer.text()
                );
                for member in DISCRIMINATOR_MEMBERS {
                    assert!(
                        !answer.text().contains(member),
                        "case {case:?}: the relayed body must carry no {member} discriminator, \
                         got {:?}",
                        answer.text()
                    );
                }
            } else {
                // The operation answers through this service's own modelled error channel,
                // so the body is the protocol's error document and the sentence is a MEMBER
                // of it — which is the only place an SDK looks.
                let document = answer.document();
                let object = document.as_object().unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected a JSON object error document, got {:?}",
                        answer.text()
                    )
                });
                let carried = MESSAGE_MEMBERS
                    .iter()
                    .find_map(|member| object.get(*member).and_then(Value::as_str));
                assert_eq!(
                    carried,
                    Some(ENDPOINT_NOT_FOUND_SENTENCE),
                    "case {case:?}: expected the sentence in the document's message member, \
                     got {:?}",
                    answer.text()
                );
            }
            // What an SDK's error parser hands its caller, on both sides of the split: the
            // modelled code from the header, and the sentence — out of the document where
            // there is one, out of the body it could not parse as a document where there is
            // not.
            let (code, message) = answer.parsed_error();
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: expected the modelled code, got {code:?}"
            );
            assert_eq!(
                message, ENDPOINT_NOT_FOUND_SENTENCE,
                "case {case:?}: expected the sentence, got {message:?}"
            );
            // The front door words the condition itself, so the name asked for appears
            // nowhere in what it answers.
            assert!(
                !answer.text().contains(name.as_str()),
                "case {case:?}: the refusal must not echo the environment name, got {:?}",
                answer.text()
            );
            assert_eq!(
                message, state::ENVIRONMENT_ENDPOINT_NOT_FOUND_MESSAGE,
                "case {case:?}: the sentence is the one this service words the condition \
                 with, got {message:?}"
            );
        }
    }
}

/// One refusal or answer the layers BEHIND the front door own:
/// `(label, method, uri, body, status, the code a client resolves)`.
type Behind = (
    &'static str,
    &'static str,
    String,
    Option<Value>,
    u16,
    &'static str,
);

/// What the same operations answer when the front door DID resolve the environment, plus a
/// control-plane lookup of a name that resolves to nothing: each is answered by a layer that
/// models its error, so each keeps the protocol's own error document.
fn answered_behind_the_front_door(stored: &str, unknown: &str) -> Vec<Behind> {
    vec![
        (
            "an Apache Airflow path the stored environment resolves to nothing",
            "POST",
            format!("/restapi/{stored}"),
            Some(json!({"Path": "/variables/absent", "Method": "GET"})),
            400,
            "RestApiClientException",
        ),
        (
            "a Path past @length(1, 64) on the stored environment",
            "POST",
            format!("/restapi/{stored}"),
            Some(json!({"Path": "/".repeat(65), "Method": "GET"})),
            400,
            "ValidationException",
        ),
        (
            "a Path past @length(1, 64) on a name that resolves to nothing",
            "POST",
            format!("/restapi/{unknown}"),
            Some(json!({"Path": "/".repeat(65), "Method": "GET"})),
            400,
            "ValidationException",
        ),
        (
            "a control-plane lookup of a name that resolves to nothing",
            "GET",
            format!("/environments/{unknown}"),
            None,
            404,
            "ResourceNotFoundException",
        ),
    ]
}

#[tokio::test]
async fn everything_behind_the_front_door_keeps_the_protocols_error_document() {
    let harness = Harness::new();
    let stored = "TheOnlyEnvironment";
    assert_eq!(
        harness.create(stored).await.status,
        200,
        "the fixture create must succeed"
    );

    for (case, method, uri, body, status, code) in
        answered_behind_the_front_door(stored, "NoSuchEnvironment")
    {
        let answer = harness.send(method, &uri, body).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.document().is_object(),
            "case {case:?}: expected the protocol's error document, got {:?}",
            answer.text()
        );
        let (parsed_code, _) = answer.parsed_error();
        assert_eq!(
            parsed_code, code,
            "case {case:?}: expected the code {code}, got {parsed_code:?} in {}",
            answer.text()
        );
    }

    // The positive controls, through the same front door: a call it resolves is served, and
    // its body is the operation's own output object rather than any sentence.
    for (case, uri, member) in [
        ("CreateCliToken", format!("/clitoken/{stored}"), "CliToken"),
        (
            "CreateWebLoginToken",
            format!("/webtoken/{stored}"),
            "WebToken",
        ),
    ] {
        let answer = harness.send("POST", &uri, None).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer
                .document()
                .get(member)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty()),
            "case {case:?}: expected a {member} in the output object, got {}",
            answer.text()
        );
    }

    let served = harness
        .send(
            "POST",
            &format!("/restapi/{stored}"),
            Some(json!({"Path": "/version", "Method": "GET"})),
        )
        .await;
    assert_eq!(
        served.status,
        200,
        "a served Apache Airflow call must answer 200, got {} {}",
        served.status,
        served.text()
    );
    assert_eq!(
        served
            .document()
            .get("RestApiStatusCode")
            .and_then(Value::as_i64),
        Some(200),
        "the served body must carry RestApiStatusCode, got {}",
        served.text()
    );
}
