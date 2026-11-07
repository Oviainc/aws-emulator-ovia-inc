// SPDX-License-Identifier: CC0-1.0

//! The FORM of the participant token this service issues: the serialization a client
//! parses it in before it ever presents it to a stage.
//!
//! Every case goes through the router's own `dispatch` with restJson1 requests, exactly as
//! signed `aws ivs-realtime` calls arrive, so what is asserted is the emulator's HTTP
//! behaviour and never a handler called directly.
//!
//! The RULE under test, exercised over a SET of inputs rather than on one request: a token
//! this service issues is a JSON Web Token in compact serialization (RFC 7515 §3.1,
//! RFC 7519 §3) — three `.`-separated segments, each a non-empty base64url encoding
//! (RFC 7515 §2: the URL-safe alphabet `A-Za-z0-9-_`, no padding), the first two decoding
//! to JSON objects (the JOSE header and the claim set) — and no two issuances carry the
//! same token. That is the serialization the clients of this API read a token in: they
//! split it on `.` and base64url-decode the segments, so a value carrying no separators is
//! rejected before any stage is reached, whatever its contents. What a token CONTAINS is
//! not asserted beyond the form the model's own doc leaves open: "Treat tokens as opaque;
//! i.e., do not build functionality based on token contents. The format of tokens could
//! change in the future."
//!
//! The rows differ in what the rule generalises over: WHICH operation issued the token
//! (`CreateParticipantToken`, and `CreateStage`'s own
//! `participantTokenConfigurations`), and what the issuing request asked for — no member
//! at all, an empty `capabilities` list, each capability, a `userId`, `attributes`, and the
//! duration at each modelled bound.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The region this suite signs for.
const REGION: &str = "us-east-1";

/// The modelled `@range` of `ParticipantTokenDurationMinutes`, from
/// `smithy_query(query="operation",operation="CreateParticipantToken")`.
const MIN_DURATION_MINUTES: i64 = 1;
const MAX_DURATION_MINUTES: i64 = 20160;

/// The values the modelled `ParticipantTokenCapability` carries, from
/// `smithy_query(query="shape",shape="ParticipantTokenCapability")`.
const CAPABILITIES: [&str; 2] = ["PUBLISH", "SUBSCRIBE"];

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches it.
struct RealTime {
    router: MultiServiceRouter,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router }
    }

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential scope
    /// naming `ivs` in [`REGION`], and report the status and body a client sees.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{REGION}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://ivsrealtime.{REGION}.amazonaws.com/{operation}"
            ))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, body)
    }
}

/// A string member, read as the empty string when absent.
fn member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The bytes one base64url segment carries, or `None` when the segment is not base64url
/// without padding (RFC 7515 §2): the alphabet is `A-Za-z0-9-_`, and the padding a
/// standard-alphabet decoder needs is restored here before the workspace's own codec is
/// handed the value.
fn decode_base64url(segment: &str) -> Option<Vec<u8>> {
    if segment.is_empty()
        || !segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return None;
    }
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    ::aws_smithy_types::base64::decode(standard).ok()
}

/// Assert `token` is a compact JWS carrying a JSON header and claim set, and that no token
/// asserted before it was the same value.
fn assert_token_form(case: &str, token: &str, seen: &mut HashSet<String>) {
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "case {case:?}: expected three '.'-separated segments, got {} ({token:?})",
        segments.len()
    );
    for (position, segment) in segments.iter().enumerate() {
        let decoded = decode_base64url(segment);
        assert!(
            decoded.is_some(),
            "case {case:?}: expected segment {position} to be non-empty base64url \
             (A-Za-z0-9-_, unpadded), got {segment:?}"
        );
        // The header and the claim set are JSON objects; the third segment is the
        // signature, which is octets rather than JSON.
        if position < 2 {
            let bytes = decoded.unwrap_or_default();
            let parsed: Option<Value> = serde_json::from_slice(&bytes).ok();
            assert!(
                parsed.as_ref().is_some_and(Value::is_object),
                "case {case:?}: expected segment {position} to decode to a JSON object, \
                 got {:?}",
                String::from_utf8_lossy(&bytes)
            );
        }
    }
    assert!(
        seen.insert(token.to_string()),
        "case {case:?}: expected a token minted for this issuance alone, got the \
         already-issued value"
    );
}

/// Every token `CreateParticipantToken` issues, over the inputs the operation admits, and
/// every token `CreateStage` issues with the stage, carries the compact-JWS form.
#[tokio::test]
async fn an_issued_participant_token_is_a_compact_jws() {
    let emulator = RealTime::new();
    let (status, created) = emulator
        .call("CreateStage", &json!({ "name": "token-form" }))
        .await;
    assert_eq!(
        status, 200,
        "the fixture stage must be created, got {status} {created}"
    );
    let stage_arn = member(&created["stage"], "arn");
    let mut seen = HashSet::new();

    // Rows differ in what the issuing request asks for: nothing beyond the stage, an empty
    // capability list (which the modelled `@length(0..2)` admits), each capability on its
    // own, both of them, a name, attributes, and the duration at each modelled bound.
    let rows: Vec<(&str, Value)> = vec![
        ("the smallest request the model allows", json!({})),
        ("an empty capabilities list", json!({ "capabilities": [] })),
        (
            "one capability",
            json!({ "capabilities": [CAPABILITIES[0]] }),
        ),
        (
            "the other capability",
            json!({ "capabilities": [CAPABILITIES[1]] }),
        ),
        ("both capabilities", json!({ "capabilities": CAPABILITIES })),
        ("a named participant", json!({ "userId": "alice" })),
        (
            "attributes to carry",
            json!({ "attributes": { "seat": "left", "row": "front" } }),
        ),
        (
            "the shortest duration the model admits",
            json!({ "duration": MIN_DURATION_MINUTES }),
        ),
        (
            "the longest duration the model admits",
            json!({ "duration": MAX_DURATION_MINUTES }),
        ),
    ];

    for (case, mut request) in rows {
        request["stageArn"] = json!(stage_arn);
        let (status, response) = emulator.call("CreateParticipantToken", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateParticipantToken {request} must answer HTTP 200, \
             got {status} {response}"
        );
        let token = member(&response["participantToken"], "token");
        assert_token_form(case, &token, &mut seen);
    }

    // The other issuer of a token: the tokens a stage is created with, which go through the
    // same minting, so their form cannot differ from the one above.
    let (status, response) = emulator
        .call(
            "CreateStage",
            &json!({
                "name": "issued-with-the-stage",
                "participantTokenConfigurations": [
                    {},
                    { "userId": "host", "capabilities": [CAPABILITIES[0]] },
                    { "duration": MAX_DURATION_MINUTES, "attributes": { "seat": "left" } },
                ],
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateStage with token configurations must answer HTTP 200, got {status} {response}"
    );
    let tokens = response["participantTokens"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        tokens.len(),
        3,
        "expected one token per configuration, got {response}"
    );
    for (position, token) in tokens.iter().enumerate() {
        let case = format!("a token issued with the stage, configuration {position}");
        assert_token_form(&case, &member(token, "token"), &mut seen);
    }
}
