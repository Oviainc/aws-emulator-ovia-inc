// SPDX-License-Identifier: CC0-1.0

//! The BODY SHAPE the refusals of this service that its own modelled layers answer arrive
//! in, end to end over HTTP through the router this service mounts in production.
//!
//! The rule: a refusal the service's own code answers is serialized in the protocol's error
//! envelope — a JSON OBJECT root carrying the modelled error code and the `message` — on
//! every operation, whichever endpoint the model hosts it on (`@endpoint` host prefix `api.`
//! or `env.`) and whichever status and code the condition answers with. That envelope is
//! what a restJson1 client's error parser reads a code and a message out of: it looks both
//! up as MEMBERS of the parsed body. The ONE refusal outside this rule is the one this
//! service does not answer itself: the proxied operation RELAYS what the endpoint it called
//! sent, which for a name the front door cannot resolve is the sentence as the whole body
//! — that split has its own suite (`tests/all/env_endpoint_front_door.rs`). WHICH sentence
//! a body carries is a separate rule with a third
//! (`tests/all/env_endpoint_not_found_wording.rs`).
//!
//! Each table row varies what the rule generalises over: the operation and the endpoint
//! hosting it, the status and the error code (404 `ResourceNotFoundException`, 400
//! `ValidationException`, 400 `RestApiClientException`, 404 `NotFoundException`), the
//! environment name (every form `EnvironmentName`'s own constraints allow), and the Apache
//! Airflow method, path and payload the proxied call names. A served call is the positive
//! control: its 200 body is the operation's own output object.

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

/// The region the `AUTHORIZATION` scope signs for.
const REGION: &str = "us-east-1";

/// The body members a restJson1 error parser looks the error code up in, in that order.
const CODE_MEMBERS: [&str; 3] = ["__type", "code", "Code"];

/// The body members it looks the message up in.
const MESSAGE_MEMBERS: [&str; 2] = ["message", "Message"];

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    /// The body as a JSON document, whatever its root type is.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|error| panic!("expected a JSON body, got {error}: {}", self.text()))
    }

    /// The error code and the message a restJson1 client's error parser reads out of this
    /// answer.
    ///
    /// It reads both as MEMBERS of the parsed body — the code from the discriminator member,
    /// the message from `message` — so an answer whose root is not an object resolves to
    /// `None`: no code, no message, and no error object for the caller to branch on.
    fn error_from_body(&self) -> Option<(String, String)> {
        let body = self.json();
        let object = body.as_object()?;
        let code = CODE_MEMBERS
            .iter()
            .find_map(|member| object.get(*member).and_then(Value::as_str))
            .map(shape_name)
            .unwrap_or_default();
        let message = MESSAGE_MEMBERS
            .iter()
            .find_map(|member| object.get(*member).and_then(Value::as_str))
            .unwrap_or_default();
        Some((code, message.to_string()))
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// The bare shape name a discriminator carries: a value may arrive namespaced
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
        Answer {
            status: response.status().as_u16(),
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
        self.send("POST", &format!("/restapi/{name}"), Some(body))
            .await
    }
}

/// What the `message` of a refusal must be.
enum Sentence {
    /// The exact sentence the condition is worded with.
    Exactly(String),
    /// Any non-empty sentence: the framework's own constraint envelope words these, and its
    /// wording is pinned by `tests/all/rejection_contract.rs` rather than here.
    NonEmpty,
    /// None at all: `RestApiClientException` / `RestApiServerException` model no `message`
    /// member, carrying the Apache Airflow status and document instead
    /// (`smithy_query(query="shape",shape="RestApiClientException")`), so the code is all a
    /// client reads out of the envelope.
    Unmodelled,
}

/// One refusal of some OTHER operation: `(label, method, uri, body, status, code, message)`.
type Refusal = (
    &'static str,
    &'static str,
    String,
    Option<Value>,
    u16,
    &'static str,
    Sentence,
);

/// A refusal of every operation this service answers one for through a layer that models
/// it, spanning both endpoints and every status and wire code it refuses with — so the
/// envelope is pinned as the rule it is rather than as one operation's quirk. The
/// `env.`-hosted TOKEN operations appear through their unresolvable-name path as well: that
/// refusal is worded by their endpoint's front door and answered by this service, so it
/// travels in this same envelope. The proxied operation appears through its SERVED path
/// only: the refusal it relays is the one body outside this rule, by
/// `tests/all/env_endpoint_front_door.rs`'s split.
fn other_refusals(unknown: &str, stored: &str) -> Vec<Refusal> {
    // `ResourceArn` is bound to a URI label, so its ARN's separators travel percent-encoded.
    let unresolvable_arn = format!("arn:aws:airflow:{REGION}:{ACCOUNT}:environment/NeverCreated")
        .replace(':', "%3A")
        .replace('/', "%2F");
    vec![
        (
            "GetEnvironment on an unknown name",
            "GET",
            format!("/environments/{unknown}"),
            None,
            404,
            "ResourceNotFoundException",
            Sentence::Exactly(state::environment_not_found_message(unknown)),
        ),
        (
            "UpdateEnvironment on an unknown name",
            "PATCH",
            format!("/environments/{unknown}"),
            Some(json!({"MaxWorkers": 2})),
            404,
            "ResourceNotFoundException",
            Sentence::Exactly(state::environment_not_found_message(unknown)),
        ),
        (
            "DeleteEnvironment on an unknown name",
            "DELETE",
            format!("/environments/{unknown}"),
            None,
            404,
            "ResourceNotFoundException",
            Sentence::Exactly(state::environment_not_found_message(unknown)),
        ),
        (
            "CreateCliToken on an unknown name",
            "POST",
            format!("/clitoken/{unknown}"),
            None,
            404,
            "ResourceNotFoundException",
            Sentence::Exactly(state::ENVIRONMENT_ENDPOINT_NOT_FOUND_MESSAGE.to_string()),
        ),
        (
            "CreateWebLoginToken on an unknown name",
            "POST",
            format!("/webtoken/{unknown}"),
            None,
            404,
            "ResourceNotFoundException",
            Sentence::Exactly(state::ENVIRONMENT_ENDPOINT_NOT_FOUND_MESSAGE.to_string()),
        ),
        (
            "ListTagsForResource on an ARN that resolves to nothing",
            "GET",
            format!("/tags/{unresolvable_arn}"),
            None,
            404,
            "NotFoundException",
            Sentence::Exactly(state::RESOURCE_NOT_FOUND_MESSAGE.to_string()),
        ),
        (
            "an Apache Airflow path the stored environment resolves to nothing",
            "POST",
            format!("/restapi/{stored}"),
            Some(json!({"Path": "/variables/absent", "Method": "GET"})),
            400,
            "RestApiClientException",
            Sentence::Unmodelled,
        ),
        (
            "a Path past @length(1, 64) on the stored environment",
            "POST",
            format!("/restapi/{stored}"),
            Some(json!({"Path": "/".repeat(65), "Method": "GET"})),
            400,
            "ValidationException",
            Sentence::NonEmpty,
        ),
        (
            "a Path past @length(1, 64) on an unknown name",
            "POST",
            format!("/restapi/{unknown}"),
            Some(json!({"Path": "/".repeat(65), "Method": "GET"})),
            400,
            "ValidationException",
            Sentence::NonEmpty,
        ),
        (
            "a CreateEnvironment whose worker range cannot be provisioned",
            "PUT",
            format!("/environments/{unknown}"),
            Some(json!({
                "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/mwaa-execution"),
                "SourceBucketArn": "arn:aws:s3:::my-airflow-bucket",
                "DagS3Path": "dags",
                "NetworkConfiguration": {
                    "SubnetIds": ["subnet-00000000000000001", "subnet-00000000000000002"],
                    "SecurityGroupIds": ["sg-00000000000000001"],
                },
                "MinWorkers": 5,
                "MaxWorkers": 1,
            })),
            400,
            "ValidationException",
            Sentence::Exactly("MinWorkers must be less than or equal to MaxWorkers.".to_string()),
        ),
    ]
}

#[tokio::test]
async fn every_modelled_refusal_of_this_service_carries_its_code_and_message_in_the_same_envelope()
{
    let harness = Harness::new();
    let stored = "TheOnlyEnvironment";
    assert_eq!(
        harness.create(stored).await.status,
        200,
        "the fixture create must succeed"
    );

    for (case, method, uri, body, status, code, message) in
        other_refusals("NoSuchEnvironment", stored)
    {
        let answer = harness.send(method, &uri, body).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} {}",
            answer.status,
            answer.text()
        );
        let (parsed_code, parsed_message) = answer.error_from_body().unwrap_or_else(|| {
            panic!(
                "case {case:?}: an SDK error parser read nothing out of {}",
                answer.text()
            )
        });
        assert_eq!(
            parsed_code, code,
            "case {case:?}: expected the body to carry the code {code}, got {parsed_code:?} in {}",
            answer.text()
        );
        match message {
            Sentence::Exactly(expected) => assert_eq!(
                parsed_message, expected,
                "case {case:?}: expected the message {expected:?}, got {parsed_message:?}"
            ),
            Sentence::NonEmpty => assert!(
                !parsed_message.is_empty(),
                "case {case:?}: expected a sentence beside the code, got {}",
                answer.text()
            ),
            Sentence::Unmodelled => assert!(
                answer.json().get("RestApiStatusCode").is_some(),
                "case {case:?}: an error shape with no message member must still carry its own \
                 modelled members, got {}",
                answer.text()
            ),
        }
    }
}

#[tokio::test]
async fn a_served_apache_airflow_call_still_answers_its_own_output_object() {
    let harness = Harness::new();
    let stored = "TheOnlyEnvironment";
    assert_eq!(
        harness.create(stored).await.status,
        200,
        "the fixture create must succeed"
    );

    // The positive control for the refusals above: a call the stored environment serves is
    // not an error at all, and its body is this operation's own output object.
    let served = harness
        .invoke(stored, json!({"Path": "/version", "Method": "GET"}))
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
            .json()
            .get("RestApiStatusCode")
            .and_then(Value::as_i64),
        Some(200),
        "the served body must carry RestApiStatusCode, got {}",
        served.text()
    );
    assert!(
        served.json().get("RestApiResponse").is_some(),
        "the served body must carry the Apache Airflow document, got {}",
        served.text()
    );
}
