// SPDX-License-Identifier: CC0-1.0

//! The CORS and security response headers this endpoint answers with, driven end-to-end
//! through the router the production entrypoint serves.
//!
//! RULE under test: a response whose shape binds a member to a response header carries that
//! header. This model binds the same seven — `Access-Control-Allow-Origin`,
//! `Access-Control-Expose-Headers`, `Cache-Control`, `Content-Security-Policy`,
//! `Strict-Transport-Security`, `X-Content-Type-Options` and `X-Frame-Options`
//! (`smithy_query(query="shape",shape="CreateIngestConfigurationResponse")`) — on four
//! output shapes and on every error shape it declares, and their values are the ENDPOINT's
//! answer policy rather than a fact about any resource, so every response carrying one
//! carries the rest with the same values.
//!
//! One of the seven is excepted, and this suite pins the exception: the cross-origin GRANT
//! (`Access-Control-Allow-Origin`) is which web origin may read an answer off this
//! listener, which belongs to whoever runs the emulator and is answered by the listener
//! itself, so no response of this service names an origin at all.
//!
//! Four readings of that rule:
//!
//! 1. every response class the service can answer carries the six restriction headers: a
//!    success whose output shape binds them beside a body, a success whose whole modelled
//!    output IS those members, a handler refusal, a boundary refusal, a not-found refusal
//!    and a scaffold's 501;
//! 2. no response of this service grants an origin;
//! 3. each value is one its own header admits, and the exposed-header list publishes every
//!    name the model's `@cors` trait exposes;
//! 4. the policy is a property of the endpoint: two different requests are answered with
//!    byte-identical values, nothing a request sent appears in one of them, and the body of
//!    an answer that carries them is unchanged — the created configuration still reads back
//!    through `GetIngestConfiguration`.
//!
//! The rows of each table differ along what the rule generalises over — the response class,
//! the operation, the status, and the shape of the request — never by renaming one case.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The account id every ARN this emulator mints carries, and so the account an ARN built by
/// hand here carries too.
const ACCOUNT: &str = "000000000000";

/// The region this suite signs for.
const REGION: &str = "us-east-1";

/// The six restriction headers and the value this endpoint answers each with, keyed by the
/// lowercase name an HTTP client reads them by.
///
/// The names are the `@httpHeader` traits of the four output shapes that bind them
/// (`smithy_query(query="shape",shape="CreateIngestConfigurationResponse,StartParticipantReplicationResponse")`).
const POLICY: [(&str, &str); 6] = [
    (
        "access-control-expose-headers",
        "x-amzn-errortype, x-amzn-requestid, x-amzn-errormessage, x-amzn-trace-id, \
         x-amz-apigw-id, date",
    ),
    ("cache-control", "no-store"),
    ("content-security-policy", "default-src 'none'"),
    ("strict-transport-security", "max-age=47304000; includeSubDomains"),
    ("x-content-type-options", "nosniff"),
    ("x-frame-options", "DENY"),
];

/// The one header of the seven this service never writes: the grant naming the web origins
/// allowed to read an answer off the listener.
const GRANT_HEADER: &str = "access-control-allow-origin";

/// Every header name the model's `@cors` trait publishes to a browser client, from
/// `smithy_query(query="service_info")`'s `cors.additionalExposedHeaders` (the trait repeats
/// `x-amzn-requestid`; it is one name).
const EXPOSED_HEADERS: [&str; 6] = [
    "x-amzn-errortype",
    "x-amzn-requestid",
    "x-amzn-errormessage",
    "x-amzn-trace-id",
    "x-amz-apigw-id",
    "date",
];

/// The values `X-Frame-Options` defines, per the MDN page the model's `xFrameOptions` doc
/// points at.
const FRAME_OPTIONS_VALUES: [&str; 2] = ["DENY", "SAMEORIGIN"];

/// One response, as a client sees it.
struct Answer {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Value,
}

impl Answer {
    /// A response header, or `"<absent>"` so a failing assertion names the missing header
    /// rather than panicking inside the reader.
    fn header(&self, name: &str) -> &str {
        self.headers.get(name).map_or("<absent>", String::as_str)
    }
}

/// One emulator holding only this service, driven through the router that serves production
/// traffic.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential scope
    /// naming `ivs` in [`REGION`].
    async fn post(&self, operation: &str, body: &Value) -> Answer {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{REGION}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://ivsrealtime.{REGION}.amazonaws.com/{operation}"))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let headers: BTreeMap<String, String> = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(value.as_bytes()).to_string(),
                )
            })
            .collect();
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Answer {
            status,
            headers,
            body,
        }
    }

    /// A stage, created the way a client creates one, reported by its ARN.
    async fn stage(&self, case: &str, name: &str) -> String {
        let answer = self.post("CreateStage", &json!({ "name": name })).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: CreateStage {name:?} must answer HTTP 200, got {} {}",
            answer.status, answer.body
        );
        answer.body["stage"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// A publisher of `stage_arn`, from a token issued for it: the identity the model says a
    /// `participantId` comes from.
    async fn publisher(&self, case: &str, stage_arn: &str) -> String {
        let answer = self
            .post("CreateParticipantToken", &json!({ "stageArn": stage_arn }))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: CreateParticipantToken must answer HTTP 200, got {} {}",
            answer.status, answer.body
        );
        answer.body["participantToken"]["participantId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// Assert ONE response carries each restriction header with the endpoint's value for it,
/// and grants no origin.
fn assert_header_policy(case: &str, answer: &Answer, expected_status: u16) {
    assert_eq!(
        answer.status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {} {}",
        answer.status, answer.body
    );
    for (name, expected) in POLICY {
        assert_eq!(
            answer.header(name),
            expected,
            "case {case:?}: expected {name} to carry {expected:?}, got {:?} in {:?}",
            answer.header(name),
            answer.headers
        );
    }
    assert!(
        !answer.headers.contains_key(GRANT_HEADER),
        "case {case:?}: expected no {GRANT_HEADER} grant from this service, got {:?} in {:?}",
        answer.header(GRANT_HEADER),
        answer.headers
    );
}

/// A well-formed stage ARN naming a stage that was never created.
fn absent_stage_arn(id: &str) -> String {
    format!("arn:aws:ivs:{REGION}:{ACCOUNT}:stage/{id}")
}

/// RULE, first and second readings: every response class this service answers carries the
/// six restriction headers, and none of them grants an origin.
#[tokio::test]
async fn every_response_class_carries_the_endpoints_header_policy() {
    let emulator = Emulator::new();
    let source = emulator.stage("replication", "policy-source").await;
    let destination = emulator.stage("replication", "policy-destination").await;
    let publisher = emulator.publisher("replication", &source).await;
    let absent = absent_stage_arn("neverCreated");

    // Rows span the response CLASSES: a success whose output shape binds the members beside
    // a body (three shapes of request, so the policy is not a function of the input), a
    // success whose whole modelled output IS those members, a handler refusal, a boundary
    // refusal, a not-found refusal, and a scaffold's 501.
    let cases: Vec<(&str, &str, Value, u16)> = vec![
        (
            "a created ingest configuration over RTMPS",
            "CreateIngestConfiguration",
            json!({ "ingestProtocol": "RTMPS" }),
            200,
        ),
        (
            "a created ingest configuration over cleartext RTMP",
            "CreateIngestConfiguration",
            json!({ "ingestProtocol": "RTMP", "insecureIngest": true }),
            200,
        ),
        (
            "a created ingest configuration naming a stage, a user and tags",
            "CreateIngestConfiguration",
            json!({
                "ingestProtocol": "RTMPS",
                "name": "policy-config",
                "stageArn": source,
                "userId": "policy-user",
                "redundantIngest": true,
                "tags": { "owner": "headers" },
            }),
            200,
        ),
        (
            "a started replication, whose whole modelled output is those headers",
            "StartParticipantReplication",
            json!({
                "sourceStageArn": source,
                "destinationStageArn": destination,
                "participantId": publisher,
            }),
            200,
        ),
        (
            "a handler refusal: cleartext ingest without insecure ingest",
            "CreateIngestConfiguration",
            json!({ "ingestProtocol": "RTMP" }),
            400,
        ),
        (
            "a boundary refusal: an ingest protocol the model does not declare",
            "CreateIngestConfiguration",
            json!({ "ingestProtocol": "SRT" }),
            400,
        ),
        (
            "a boundary refusal: a required member the request omits",
            "CreateIngestConfiguration",
            json!({}),
            400,
        ),
        (
            "a not-found refusal",
            "GetStage",
            json!({ "arn": absent }),
            404,
        ),
        (
            "a scaffolded operation",
            "StopParticipantReplication",
            json!({
                "sourceStageArn": source,
                "destinationStageArn": destination,
                "participantId": publisher,
            }),
            501,
        ),
    ];

    for (case, operation, request, status) in cases {
        let answer = emulator.post(operation, &request).await;
        assert_header_policy(case, &answer, status);
    }
}

/// RULE, third reading: each value is one its own header admits, and the exposed-header
/// list publishes every name the model's `@cors` trait exposes.
#[tokio::test]
async fn each_header_carries_a_value_its_own_definition_admits() {
    let emulator = Emulator::new();
    let answer = emulator
        .post(
            "CreateIngestConfiguration",
            &json!({ "ingestProtocol": "RTMPS" }),
        )
        .await;
    assert_header_policy("a created ingest configuration", &answer, 200);

    let case = "the endpoint's policy";

    // Every name the model's `@cors` trait exposes — including the three the issue did not
    // cite (`x-amzn-trace-id`, `x-amz-apigw-id`, `date`) — is published, and each is
    // published once, as its own entry in the comma-separated list.
    let exposed = answer.header("access-control-expose-headers");
    let published: Vec<&str> = exposed.split(',').map(str::trim).collect();
    for name in EXPOSED_HEADERS {
        assert_eq!(
            published.iter().filter(|entry| **entry == name).count(),
            1,
            "case {case:?}: expected the exposed-header list to publish {name} exactly once, \
             got {published:?}"
        );
    }

    // Cache-Control states a directive the header defines, and this endpoint's answer is
    // never storable.
    let cache = answer.header("cache-control");
    assert!(
        cache.split(',').any(|directive| directive.trim() == "no-store"),
        "case {case:?}: expected Cache-Control to forbid storing the answer, got {cache:?}"
    );

    // Content-Security-Policy states at least one directive, and the fallback source list
    // for a JSON answer loads nothing.
    let csp = answer.header("content-security-policy");
    assert!(
        csp.split(';')
            .any(|directive| directive.trim() == "default-src 'none'"),
        "case {case:?}: expected the content policy to load nothing, got {csp:?}"
    );

    // Strict-Transport-Security carries a max-age in whole seconds, and applies to the
    // endpoint's subdomains.
    let hsts = answer.header("strict-transport-security");
    let max_age = hsts
        .split(';')
        .map(str::trim)
        .find_map(|directive| directive.strip_prefix("max-age="));
    assert!(
        max_age.is_some_and(|seconds| {
            !seconds.is_empty() && seconds.bytes().all(|byte| byte.is_ascii_digit())
        }),
        "case {case:?}: expected a max-age in whole seconds, got {hsts:?}"
    );
    assert!(
        hsts.split(';')
            .any(|directive| directive.trim() == "includeSubDomains"),
        "case {case:?}: expected the transport policy to cover subdomains, got {hsts:?}"
    );

    // X-Content-Type-Options defines exactly one value; X-Frame-Options defines two.
    assert_eq!(
        answer.header("x-content-type-options"),
        "nosniff",
        "case {case:?}: expected the one value this header defines, got {:?}",
        answer.header("x-content-type-options")
    );
    let frame = answer.header("x-frame-options");
    assert!(
        FRAME_OPTIONS_VALUES.contains(&frame),
        "case {case:?}: expected one of {FRAME_OPTIONS_VALUES:?}, got {frame:?}"
    );
}

/// RULE, fourth reading: the policy belongs to the endpoint, not to the request or the
/// resource — the same values on every answer, nothing of the request in them, and the
/// answer's own body unchanged.
#[tokio::test]
async fn the_policy_is_the_endpoints_and_leaves_the_answer_unchanged() {
    let emulator = Emulator::new();
    let stage = emulator.stage("policy", "policy-stage").await;

    // Two requests differing in protocol, name, user, stage, redundancy and tags: the
    // headers they are answered with must not differ at all.
    let requests = vec![
        (
            "a bare configuration",
            json!({ "ingestProtocol": "RTMPS" }),
        ),
        (
            "a configuration naming everything it can",
            json!({
                "ingestProtocol": "RTMP",
                "insecureIngest": true,
                "name": "mirror-config",
                "stageArn": stage,
                "userId": "mirror-user",
                "redundantIngest": true,
                "tags": { "owner": "mirror" },
            }),
        ),
    ];

    let mut answers = Vec::new();
    for (case, request) in &requests {
        let answer = emulator.post("CreateIngestConfiguration", request).await;
        assert_header_policy(case, &answer, 200);
        answers.push((case, request, answer));
    }

    // The same values, byte for byte, and nothing the request sent inside one of them.
    let sent = ["mirror-config", "mirror-user", "mirror", stage.as_str()];
    for (name, _) in POLICY {
        let first = answers[0].2.header(name).to_string();
        for (case, _, answer) in &answers {
            assert_eq!(
                answer.header(name),
                first,
                "case {case:?}: expected {name} to carry the endpoint's own value {first:?}, \
                 got {:?}",
                answer.header(name)
            );
        }
        for value in sent {
            assert!(
                !first.contains(value),
                "case \"the endpoint's policy\": expected {name} to carry nothing the request \
                 sent, found {value:?} in {first:?}"
            );
        }
    }

    // The body of an answer that carries the headers is untouched: the configuration the
    // second request created reads back through `GetIngestConfiguration` exactly as the
    // create reported it.
    let created = answers[1].2.body["ingestConfiguration"].clone();
    let arn = created["arn"].as_str().unwrap_or_default().to_string();
    assert!(
        !arn.is_empty(),
        "case \"a configuration naming everything it can\": expected a minted ARN, got {created}"
    );
    let read = emulator
        .post("GetIngestConfiguration", &json!({ "arn": arn }))
        .await;
    assert_eq!(
        read.status, 200,
        "case \"a configuration naming everything it can\": GetIngestConfiguration must \
         answer HTTP 200, got {} {}",
        read.status, read.body
    );
    assert_eq!(
        read.body["ingestConfiguration"], created,
        "case \"a configuration naming everything it can\": expected the created \
         configuration to read back unchanged, got {}",
        read.body
    );
}
