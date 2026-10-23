// SPDX-License-Identifier: CC0-1.0

//! The Bedrock Runtime streaming surface, end to end over HTTP: `ConverseStream`
//! and `InvokeModelWithResponseStream`, plus the model-identifier rule every
//! operation that takes a `modelId` follows.
//!
//! Every case drives the real router, so what is asserted is the wire behaviour a
//! client sees — status, headers, and the `vnd.amazon.eventstream` frames — never a
//! handler called directly.
//!
//! The rule each test states is asserted over a SET of inputs: the tables differ
//! along what the rule generalises over (model family, message count, text size and
//! script, token cap, identifier form), so no row is the specification on its own.
//! The streaming surfaces are checked AGAINST their non-streaming siblings, which is
//! the invariant that matters: `ConverseStream` frames the turn `Converse` serves,
//! and `InvokeModelWithResponseStream` chunks the payload `InvokeModel` returns.

use std::sync::Arc;

use aws_smithy_eventstream::frame::read_message_from;
use aws_smithy_types::event_stream::HeaderValue;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A host-style endpoint for the service, so a request routes even if the SigV4
/// scope below ever changes.
const ENDPOINT: &str = "https://bedrock-runtime.us-east-1.amazonaws.com";

/// A SigV4 `Authorization` header scoped to this service's signing name
/// (`bedrock`). Request authentication is off in these tests, so the signature is
/// not checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEMULATOR000000000/20260101/us-east-1/bedrock/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The media type an event-stream body carries, in both directions.
const EVENT_STREAM: &str = "application/vnd.amazon.eventstream";

/// The frames a `ConverseStream` turn that runs to completion emits, in order —
/// `ConverseStreamOutput`'s own member names (smithy_query(query="shape",
/// shape="ConverseStreamOutput")).
const CONVERSE_STREAM_FRAMES: [&str; 5] = [
    "messageStart",
    "contentBlockDelta",
    "contentBlockStop",
    "messageStop",
    "metadata",
];

fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One HTTP exchange.
struct Call {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Call {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "response body is not JSON ({error}): {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The wire error code: the `x-amzn-errortype` header restJson1 sets, or the
    /// body's `__type` when a value could not be rendered as a header.
    fn error_code(&self) -> String {
        if let Some(value) = self.headers.get("x-amzn-errortype") {
            return value.to_str().unwrap_or_default().to_string();
        }
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    /// The essence of the `content-type` header, without any parameters.
    fn media_type(&self) -> String {
        self.header("content-type")
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
    }
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Call {
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!("{ENDPOINT}{path}"))
        .header("authorization", AUTHORIZATION)
        .header("host", "bedrock-runtime.us-east-1.amazonaws.com");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request
        .body(bytes::Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Call {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

async fn post_json(router: &MultiServiceRouter, path: &str, body: &Value) -> Call {
    send(
        router,
        "POST",
        path,
        &[("content-type", "application/json")],
        serde_json::to_vec(body).expect("a serializable test body"),
    )
    .await
}

/// Decode an event-stream response body, asserting the frame headers an SDK
/// decoder requires. Each entry is `(event type, payload)`.
fn decode_frames(case: &str, body: &[u8]) -> Vec<(String, Value)> {
    let mut buffer = bytes::Bytes::copy_from_slice(body);
    let mut frames = Vec::new();
    while !buffer.is_empty() {
        let message = read_message_from(&mut buffer).unwrap_or_else(|error| {
            panic!("case {case:?}: the body must decode as event-stream frames: {error}")
        });
        let header = |name: &str| {
            message
                .headers()
                .iter()
                .find(|header| header.name().as_str() == name)
                .map(|header| match header.value() {
                    HeaderValue::String(value) => value.as_str().to_string(),
                    other => format!("{other:?}"),
                })
        };
        assert_eq!(
            header(":message-type").as_deref(),
            Some("event"),
            "case {case:?}: a response frame must be an event, got {:?}",
            header(":message-type")
        );
        assert_eq!(
            header(":content-type").as_deref(),
            Some("application/json"),
            "case {case:?}: an event payload is JSON, got {:?}",
            header(":content-type")
        );
        let event_type = header(":event-type")
            .unwrap_or_else(|| panic!("case {case:?}: a response frame named no :event-type"));
        let payload: Value = serde_json::from_slice(message.payload()).unwrap_or_else(|error| {
            panic!("case {case:?}: the {event_type} payload must be JSON: {error}")
        });
        // Bedrock pads every event payload with the information-free `p` member;
        // a client's decoder sees it on every frame.
        assert!(
            payload.get("p").and_then(Value::as_str).is_some(),
            "case {case:?}: the {event_type} payload must carry Bedrock's `p` padding, got \
             {payload}"
        );
        frames.push((event_type, payload));
    }
    frames
}

/// One conversation, and the model it is sent to.
struct ConverseRow {
    case: &'static str,
    /// A model the registry serves and the Converse API supports — one row per
    /// response family, which is what the Converse surface generalises over.
    model_id: &'static str,
    system: Value,
    messages: Value,
    /// `inferenceConfig.maxTokens`, where the row is about the generation cap.
    max_tokens: Option<i64>,
}

fn converse_rows() -> Vec<ConverseRow> {
    vec![
        ConverseRow {
            case: "one short user turn",
            model_id: "anthropic.claude-3-haiku-20240307-v1:0",
            system: json!([]),
            messages: json!([{"role": "user", "content": [{"text": "Name one planet."}]}]),
            max_tokens: None,
        },
        ConverseRow {
            case: "three turns with a system prompt",
            model_id: "amazon.titan-text-premier-v1:0",
            system: json!([{"text": "You are terse."}]),
            messages: json!([
                {"role": "user", "content": [{"text": "Name two planets."}]},
                {"role": "assistant", "content": [{"text": "Mars and Venus."}]},
                {"role": "user", "content": [{"text": "And one moon?"}]},
            ]),
            max_tokens: None,
        },
        ConverseRow {
            case: "non-ascii content",
            model_id: "amazon.nova-pro-v1:0",
            system: json!([]),
            messages: json!([{
                "role": "user",
                "content": [{"text": "Übersetze: 木星 is the largest planet — richtig?"}],
            }]),
            max_tokens: None,
        },
        ConverseRow {
            case: "several content blocks in one turn",
            model_id: "mistral.mistral-7b-instruct-v0:2",
            system: json!([]),
            messages: json!([{
                "role": "user",
                "content": [
                    {"text": "List three primes."},
                    {"text": "Then explain why two of them are twins."},
                ],
            }]),
            max_tokens: None,
        },
        ConverseRow {
            case: "generation capped at three tokens",
            model_id: "meta.llama3-8b-instruct-v1:0",
            system: json!([]),
            messages: json!([{"role": "user", "content": [{"text": "Explain idempotency."}]}]),
            max_tokens: Some(3),
        },
        ConverseRow {
            case: "a long prompt",
            model_id: "cohere.command-r-plus-v1:0",
            system: json!([{"text": "Answer in one sentence."}]),
            messages: json!([{
                "role": "user",
                "content": [{"text": "word ".repeat(400)}],
            }]),
            max_tokens: None,
        },
    ]
}

impl ConverseRow {
    /// The request body both Converse surfaces are sent.
    fn request(&self) -> Value {
        let mut request = json!({ "messages": self.messages });
        if self.system.as_array().is_some_and(|blocks| !blocks.is_empty()) {
            request["system"] = self.system.clone();
        }
        if let Some(max_tokens) = self.max_tokens {
            request["inferenceConfig"] = json!({ "maxTokens": max_tokens });
        }
        request
    }
}

/// One `InvokeModel` request, per response family.
struct InvokeRow {
    case: &'static str,
    model_id: &'static str,
    body: &'static str,
}

/// Model ids from "Amazon Bedrock base model IDs (on-demand throughput)" and the
/// request envelopes from "Inference request parameters and response fields for
/// foundation models", both in the Amazon Bedrock User Guide. The last row is a
/// model the Converse API does NOT serve, so response streaming must still serve
/// it.
const INVOKE_ROWS: &[InvokeRow] = &[
    InvokeRow {
        case: "anthropic messages",
        model_id: "anthropic.claude-3-haiku-20240307-v1:0",
        body: r#"{"anthropic_version":"bedrock-2023-05-31","max_tokens":256,"messages":[{"role":"user","content":[{"type":"text","text":"Name one planet."}]}]}"#,
    },
    InvokeRow {
        case: "titan text",
        model_id: "amazon.titan-text-premier-v1:0",
        body: r#"{"inputText":"Write a haiku about persistent state.","textGenerationConfig":{"maxTokenCount":512}}"#,
    },
    InvokeRow {
        case: "nova",
        model_id: "amazon.nova-lite-v1:0",
        body: r#"{"system":[{"text":"You are terse."}],"messages":[{"role":"user","content":[{"text":"Name two planets."}]}]}"#,
    },
    InvokeRow {
        case: "llama, generation capped at three tokens",
        model_id: "meta.llama3-70b-instruct-v1:0",
        body: r#"{"prompt":"Explain idempotency to a cat.","max_gen_len":3}"#,
    },
    InvokeRow {
        case: "cohere command, which Converse does not serve",
        model_id: "cohere.command-text-v14",
        body: r#"{"prompt":"Describe a page token.","max_tokens":64}"#,
    },
];

// ---------------------------------------------------------------------------
// ConverseStream
// ---------------------------------------------------------------------------

/// RULE: `ConverseStream` serves the same turn `Converse` serves and frames it as
/// the `ConverseStreamOutput` events — `messageStart`, a `contentBlockDelta` and
/// `contentBlockStop` per content block, `messageStop`, `metadata` — in a
/// `application/vnd.amazon.eventstream` body, with the assistant text, the stop
/// reason and the token usage identical to what `Converse` reports for the same
/// request.
#[tokio::test]
async fn converse_stream_frames_the_turn_converse_serves() {
    let router = emulator();

    for row in converse_rows() {
        let case = row.case;
        let request = row.request();

        let converse = post_json(
            &router,
            &format!("/model/{}/converse", row.model_id),
            &request,
        )
        .await;
        assert_eq!(
            converse.status,
            200,
            "case {case:?}: expected Converse to answer 200, got {} / {}",
            converse.status,
            String::from_utf8_lossy(&converse.body)
        );
        let converse = converse.json();
        let expected_text = converse["output"]["message"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: Converse answered no text: {converse}"))
            .to_string();
        let expected_stop = converse["stopReason"].as_str().unwrap_or_default().to_string();
        let expected_usage = converse["usage"].clone();

        let stream = post_json(
            &router,
            &format!("/model/{}/converse-stream", row.model_id),
            &request,
        )
        .await;
        assert_eq!(
            stream.status,
            200,
            "case {case:?}: expected ConverseStream to answer 200, got {} / {}",
            stream.status,
            String::from_utf8_lossy(&stream.body)
        );
        assert_eq!(
            stream.media_type(),
            EVENT_STREAM,
            "case {case:?}: expected content-type {EVENT_STREAM}, got {:?}",
            stream.header("content-type")
        );

        let frames = decode_frames(case, &stream.body);
        let order: Vec<&str> = frames.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            order,
            CONVERSE_STREAM_FRAMES.to_vec(),
            "case {case:?}: expected the frame order {CONVERSE_STREAM_FRAMES:?}, got {order:?}"
        );

        let by_name = |name: &str| {
            frames
                .iter()
                .find(|(frame, _)| frame == name)
                .map(|(_, payload)| payload.clone())
                .unwrap_or_else(|| panic!("case {case:?}: no {name} frame in {order:?}"))
        };

        assert_eq!(
            by_name("messageStart")["role"].as_str(),
            Some("assistant"),
            "case {case:?}: messageStart must carry the assistant role, got {}",
            by_name("messageStart")
        );

        let delta = by_name("contentBlockDelta");
        let streamed: String = frames
            .iter()
            .filter(|(frame, _)| frame == "contentBlockDelta")
            .map(|(_, payload)| {
                payload["delta"]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            streamed, expected_text,
            "case {case:?}: the deltas must concatenate to the text Converse answers, \
             got {streamed:?}"
        );
        assert_eq!(
            delta["contentBlockIndex"].as_i64(),
            Some(0),
            "case {case:?}: the first delta is for content block 0, got {delta}"
        );
        assert_eq!(
            by_name("contentBlockStop")["contentBlockIndex"].as_i64(),
            Some(0),
            "case {case:?}: contentBlockStop must close block 0, got {}",
            by_name("contentBlockStop")
        );

        assert_eq!(
            by_name("messageStop")["stopReason"].as_str(),
            Some(expected_stop.as_str()),
            "case {case:?}: expected stopReason {expected_stop:?}, got {}",
            by_name("messageStop")
        );
        if row.max_tokens == Some(3) {
            assert_eq!(
                expected_stop, "max_tokens",
                "case {case:?}: a capped turn stops on max_tokens, got {expected_stop:?}"
            );
        }

        let metadata = by_name("metadata");
        assert_eq!(
            metadata["usage"], expected_usage,
            "case {case:?}: expected the usage Converse reports ({expected_usage}), got {}",
            metadata["usage"]
        );
        let latency = metadata["metrics"]["latencyMs"].as_i64();
        assert!(
            latency.is_some_and(|millis| millis >= 0),
            "case {case:?}: metadata must report the latency measured, got {metadata}"
        );
    }
}

/// RULE: a `ConverseStream` request that breaks a rule the service states is
/// refused with the modelled error for that rule, in the restJson1 JSON envelope —
/// never with an event stream and never with a 5xx.
#[tokio::test]
async fn converse_stream_refuses_a_bad_request_with_the_modelled_error() {
    let router = emulator();
    let claude = "anthropic.claude-3-haiku-20240307-v1:0";

    // (case, modelId, body, status, error code)
    let rows = vec![
        (
            "no messages at all",
            claude,
            json!({}),
            400,
            "ValidationException",
        ),
        (
            "an empty conversation",
            claude,
            json!({ "messages": [] }),
            400,
            "ValidationException",
        ),
        (
            "a conversation that starts with the assistant",
            claude,
            json!({ "messages": [{"role": "assistant", "content": [{"text": "hi"}]}] }),
            400,
            "ValidationException",
        ),
        (
            "two user turns in a row",
            claude,
            json!({ "messages": [
                {"role": "user", "content": [{"text": "one"}]},
                {"role": "user", "content": [{"text": "two"}]},
            ] }),
            400,
            "ValidationException",
        ),
        (
            "a model the Converse API does not serve",
            "cohere.command-text-v14",
            json!({ "messages": [{"role": "user", "content": [{"text": "hi"}]}] }),
            400,
            "ValidationException",
        ),
        (
            "an ARN that resolves to nothing",
            "arn%3Aaws%3Abedrock%3Aus-east-1%3A000000000000%3Afoundation-model%2Fnobody.nope-v1",
            json!({ "messages": [{"role": "user", "content": [{"text": "hi"}]}] }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a bare id naming no model",
            "nobody.such-model-v1",
            json!({ "messages": [{"role": "user", "content": [{"text": "hi"}]}] }),
            400,
            "ValidationException",
        ),
        (
            "a value that is not a model identifier",
            "not-a-model",
            json!({ "messages": [{"role": "user", "content": [{"text": "hi"}]}] }),
            400,
            "ValidationException",
        ),
    ];

    for (case, model_id, body, status, code) in rows {
        let call = post_json(&router, &format!("/model/{model_id}/converse-stream"), &body).await;
        assert_eq!(
            call.status,
            status,
            "case {case:?}: expected HTTP {status}, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            code,
            "case {case:?}: expected {code}, got {:?}",
            call.error_code()
        );
        assert_eq!(
            call.media_type(),
            "application/json",
            "case {case:?}: a restJson1 error is application/json, got {:?}",
            call.header("content-type")
        );
    }
}

// ---------------------------------------------------------------------------
// InvokeModelWithResponseStream
// ---------------------------------------------------------------------------

/// RULE: `InvokeModelWithResponseStream` serves the same inference `InvokeModel`
/// serves and returns the model's response payload as `ResponseStream` `chunk`
/// events in a `application/vnd.amazon.eventstream` body, reporting the payload's
/// MIME type in the required `X-Amzn-Bedrock-Content-Type` response header.
#[tokio::test]
async fn response_stream_chunks_the_payload_invoke_model_returns() {
    let router = emulator();

    for row in INVOKE_ROWS {
        let case = row.case;
        let invoke = send(
            &router,
            "POST",
            &format!("/model/{}/invoke", row.model_id),
            &[("content-type", "application/json")],
            row.body.as_bytes().to_vec(),
        )
        .await;
        assert_eq!(
            invoke.status,
            200,
            "case {case:?}: expected InvokeModel to answer 200, got {} / {}",
            invoke.status,
            String::from_utf8_lossy(&invoke.body)
        );
        let expected: Value = invoke.json();

        let stream = send(
            &router,
            "POST",
            &format!("/model/{}/invoke-with-response-stream", row.model_id),
            &[("content-type", "application/json")],
            row.body.as_bytes().to_vec(),
        )
        .await;
        assert_eq!(
            stream.status,
            200,
            "case {case:?}: expected InvokeModelWithResponseStream to answer 200, got {} / {}",
            stream.status,
            String::from_utf8_lossy(&stream.body)
        );
        assert_eq!(
            stream.media_type(),
            EVENT_STREAM,
            "case {case:?}: expected content-type {EVENT_STREAM}, got {:?}",
            stream.header("content-type")
        );
        assert_eq!(
            stream.header("x-amzn-bedrock-content-type"),
            "application/json",
            "case {case:?}: the modelled required header must report the payload MIME \
             type, got {:?}",
            stream.header("x-amzn-bedrock-content-type")
        );
        assert_eq!(
            stream.header("x-amzn-bedrock-performanceconfig-latency"),
            "standard",
            "case {case:?}: performanceConfigLatency defaults to standard, got {:?}",
            stream.header("x-amzn-bedrock-performanceconfig-latency")
        );

        let frames = decode_frames(case, &stream.body);
        let order: Vec<&str> = frames.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            order,
            vec!["chunk"],
            "case {case:?}: the response stream carries chunk events, got {order:?}"
        );

        // `PayloadPart$bytes` is a blob, so restJson1 renders it base64.
        let encoded = frames[0].1["bytes"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the chunk carried no bytes: {}", frames[0].1));
        let decoded = aws_smithy_types::base64::decode(encoded)
            .unwrap_or_else(|error| panic!("case {case:?}: chunk bytes must be base64: {error}"));
        let streamed: Value = serde_json::from_slice(&decoded).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the chunk payload must be the model's JSON response: {error} \
                 ({})",
                String::from_utf8_lossy(&decoded)
            )
        });
        assert_eq!(
            streamed, expected,
            "case {case:?}: the chunks must carry the payload InvokeModel returns ({expected}), \
             got {streamed}"
        );
    }
}

/// RULE: an `InvokeModelWithResponseStream` request that breaks a rule the service
/// states is refused with the modelled error for that rule, in the restJson1 JSON
/// envelope — the same refusals `InvokeModel` makes, since the two take the same
/// request.
#[tokio::test]
async fn response_stream_refuses_a_bad_request_with_the_modelled_error() {
    let router = emulator();
    let claude = "anthropic.claude-3-haiku-20240307-v1:0";
    let prompt = r#"{"anthropic_version":"bedrock-2023-05-31","messages":[]}"#;

    // (case, modelId, headers, body, status, error code)
    let rows = vec![
        (
            "an empty body",
            claude,
            vec![("content-type", "application/json")],
            "",
            400,
            "ValidationException",
        ),
        (
            "a body that is not JSON-typed",
            claude,
            vec![("content-type", "text/plain")],
            prompt,
            400,
            "ValidationException",
        ),
        (
            "an accept the model cannot answer in",
            claude,
            vec![
                ("content-type", "application/json"),
                ("x-amzn-bedrock-accept", "application/xml"),
            ],
            prompt,
            400,
            "ValidationException",
        ),
        (
            "a guardrail identifier with no version",
            claude,
            vec![
                ("content-type", "application/json"),
                ("x-amzn-bedrock-guardrailidentifier", "gr1234567890"),
            ],
            prompt,
            400,
            "ValidationException",
        ),
        (
            "an inline guardrail config with no identifier",
            claude,
            vec![("content-type", "application/json")],
            r#"{"amazon-bedrock-guardrailConfig":{"tagSuffix":"x"},"prompt":"hi"}"#,
            400,
            "ValidationException",
        ),
        (
            "an ARN that resolves to nothing",
            "arn%3Aaws%3Abedrock%3Aus-east-1%3A000000000000%3Afoundation-model%2Fnobody.nope-v1",
            vec![("content-type", "application/json")],
            prompt,
            404,
            "ResourceNotFoundException",
        ),
        (
            "a bare id naming no model",
            "nobody.such-model-v1",
            vec![("content-type", "application/json")],
            prompt,
            400,
            "ValidationException",
        ),
        (
            "a value that is not a model identifier",
            "not-a-model",
            vec![("content-type", "application/json")],
            prompt,
            400,
            "ValidationException",
        ),
        // A recognised model whose modality this emulator renders no response
        // envelope for: nothing ran, so the streaming surface owes the same
        // modelled model-processing error `InvokeModel` owes, never a chunk
        // frame carrying an invented payload. One row per non-text modality the
        // inventory carries.
        (
            "an embedding model, which produces no stream",
            "amazon.titan-embed-text-v2%3A0",
            vec![("content-type", "application/json")],
            r#"{"inputText":"a cat"}"#,
            424,
            "ModelErrorException",
        ),
        (
            "an image model, which produces no stream",
            "amazon.nova-canvas-v1%3A0",
            vec![("content-type", "application/json")],
            r#"{"inputText":"a cat"}"#,
            424,
            "ModelErrorException",
        ),
        (
            "a video model, which produces no stream",
            "amazon.nova-reel-v1%3A0",
            vec![("content-type", "application/json")],
            r#"{"inputText":"a cat"}"#,
            424,
            "ModelErrorException",
        ),
    ];

    for (case, model_id, headers, body, status, code) in rows {
        let call = send(
            &router,
            "POST",
            &format!("/model/{model_id}/invoke-with-response-stream"),
            &headers,
            body.as_bytes().to_vec(),
        )
        .await;
        assert_eq!(
            call.status,
            status,
            "case {case:?}: expected HTTP {status}, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            code,
            "case {case:?}: expected {code}, got {:?}",
            call.error_code()
        );
        assert_eq!(
            call.media_type(),
            "application/json",
            "case {case:?}: a restJson1 error is application/json, got {:?}",
            call.header("content-type")
        );
    }
}

// ---------------------------------------------------------------------------
// The model-identifier rule, across every operation that takes a modelId
// ---------------------------------------------------------------------------

/// RULE: a `modelId` that resolves to nothing is answered with the error for the
/// identifier space it is in: an ARN of one of the resource types the member names
/// is `ResourceNotFoundException` (404), which is what that error is documented for
/// ("The specified resource ARN was not found"); anything else — a bare model id,
/// a geography-prefixed profile id, a single word — is not a valid model identifier
/// and is `ValidationException` (400). Every operation that takes a `modelId`
/// follows it.
#[tokio::test]
async fn an_arn_that_resolves_to_nothing_is_not_found_and_anything_else_is_invalid() {
    let router = emulator();

    // (case, modelId, status, error code). The identifier forms come from the
    // `modelId` member's own `@pattern` (smithy_query(query="shape",
    // shape="ConverseStreamRequest")): the resource types an ARN may name, and the
    // bare / geography-prefixed base-model id.
    let rows: Vec<(&str, &str, u16, &str)> = vec![
        (
            "a foundation-model ARN",
            "arn:aws:bedrock:us-east-1:000000000000:foundation-model/nobody.nope-v1",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a provisioned-model ARN",
            "arn:aws:bedrock:us-east-1:000000000000:provisioned-model/abcdefghijkl",
            404,
            "ResourceNotFoundException",
        ),
        (
            "an inference-profile ARN",
            "arn:aws:bedrock:us-east-1:000000000000:inference-profile/eu.nobody.nope-v1",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a custom-model-deployment ARN",
            "arn:aws:bedrock:us-east-1:000000000000:custom-model-deployment/abcdefghijkl",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a SageMaker endpoint ARN",
            "arn:aws:sagemaker:us-east-1:000000000000:endpoint/my-endpoint",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a bare base-model id",
            "nobody.such-model-v1",
            400,
            "ValidationException",
        ),
        (
            "a base-model id with a version suffix",
            "nobody.such-model-v1:7",
            400,
            "ValidationException",
        ),
        (
            "a cross-region inference profile id",
            "eu.nobody.such-model-v1",
            400,
            "ValidationException",
        ),
        ("a single word", "not-a-model", 400, "ValidationException"),
        (
            "a word with an underscore",
            "model_one",
            400,
            "ValidationException",
        ),
        (
            // Every ARN is a reference to a resource, whatever its type: one this
            // emulator holds no record of names an absent resource rather than an
            // identifier the service cannot read.
            "an ARN of a resource type the member does not name",
            "arn:aws:bedrock:us-east-1:000000000000:guardrail/abc123",
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, model_id, status, code) in rows {
        let encoded = model_id.replace(':', "%3A").replace('/', "%2F");
        let conversation = json!({ "messages": [{"role": "user", "content": [{"text": "hi"}]}] });
        let calls = vec![
            (
                "Converse",
                post_json(&router, &format!("/model/{encoded}/converse"), &conversation).await,
            ),
            (
                "ConverseStream",
                post_json(
                    &router,
                    &format!("/model/{encoded}/converse-stream"),
                    &conversation,
                )
                .await,
            ),
            (
                "InvokeModel",
                send(
                    &router,
                    "POST",
                    &format!("/model/{encoded}/invoke"),
                    &[("content-type", "application/json")],
                    br#"{"prompt":"hi"}"#.to_vec(),
                )
                .await,
            ),
            (
                "InvokeModelWithResponseStream",
                send(
                    &router,
                    "POST",
                    &format!("/model/{encoded}/invoke-with-response-stream"),
                    &[("content-type", "application/json")],
                    br#"{"prompt":"hi"}"#.to_vec(),
                )
                .await,
            ),
            (
                "CountTokens",
                post_json(
                    &router,
                    &format!("/model/{encoded}/count-tokens"),
                    &json!({ "input": { "converse": conversation } }),
                )
                .await,
            ),
        ];
        for (operation, call) in calls {
            assert_eq!(
                call.status,
                status,
                "case {case:?} on {operation}: expected HTTP {status}, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            assert_eq!(
                call.error_code(),
                code,
                "case {case:?} on {operation}: expected {code}, got {:?}",
                call.error_code()
            );
        }
    }
}

/// RULE: a model version the Bedrock model lifecycle has retired is still a name
/// the service recognises: every operation that takes a `modelId` answers
/// `ResourceNotFoundException` (404) saying the version has reached the end of its
/// life, rather than serving it or treating the id as unknown. An active version of
/// the same family is served.
#[tokio::test]
async fn a_model_version_past_its_end_of_life_is_not_found_with_the_end_of_life_reason() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));

    // The rows come from the registry itself rather than a hand-written list, so
    // this spans every retired version the service seeds and cannot drift from it.
    use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::{
        service_state, ModelLifecycle, END_OF_LIFE_MESSAGE,
    };
    let state = service_state(&*engine).expect("the service state store");
    let (retired, active): (Vec<String>, Vec<String>) = {
        let guard = state.lock();
        (
            guard
                .models
                .values()
                .filter(|model| model.lifecycle == ModelLifecycle::EndOfLife)
                .map(|model| model.model_id.clone())
                .collect(),
            guard
                .models
                .values()
                .filter(|model| model.lifecycle == ModelLifecycle::Active)
                .map(|model| model.model_id.clone())
                .collect(),
        )
    };
    assert!(
        retired.len() >= 2 && !active.is_empty(),
        "the registry must seed both retired and active versions, got {} retired and {} \
         active",
        retired.len(),
        active.len()
    );

    for model_id in &retired {
        let encoded = model_id.replace(':', "%3A");
        let conversation = json!({ "messages": [{"role": "user", "content": [{"text": "hi"}]}] });
        let calls = vec![
            (
                "Converse",
                post_json(&router, &format!("/model/{encoded}/converse"), &conversation).await,
            ),
            (
                "ConverseStream",
                post_json(
                    &router,
                    &format!("/model/{encoded}/converse-stream"),
                    &conversation,
                )
                .await,
            ),
            (
                "InvokeModel",
                send(
                    &router,
                    "POST",
                    &format!("/model/{encoded}/invoke"),
                    &[("content-type", "application/json")],
                    br#"{"prompt":"hi"}"#.to_vec(),
                )
                .await,
            ),
            (
                "InvokeModelWithResponseStream",
                send(
                    &router,
                    "POST",
                    &format!("/model/{encoded}/invoke-with-response-stream"),
                    &[("content-type", "application/json")],
                    br#"{"prompt":"hi"}"#.to_vec(),
                )
                .await,
            ),
            (
                "CountTokens",
                post_json(
                    &router,
                    &format!("/model/{encoded}/count-tokens"),
                    &json!({ "input": { "converse": conversation } }),
                )
                .await,
            ),
        ];
        for (operation, call) in calls {
            assert_eq!(
                call.status,
                404,
                "case {model_id:?} on {operation}: a retired version is not found, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            assert_eq!(
                call.error_code(),
                "ResourceNotFoundException",
                "case {model_id:?} on {operation}: expected ResourceNotFoundException, got {:?}",
                call.error_code()
            );
            assert_eq!(
                call.json()["message"].as_str(),
                Some(END_OF_LIFE_MESSAGE),
                "case {model_id:?} on {operation}: the reason must be the end-of-life one, \
                 got {}",
                call.json()
            );
        }
    }

    // The counter-case: an active version is never answered as retired.
    for model_id in &active {
        let encoded = model_id.replace(':', "%3A");
        let call = post_json(
            &router,
            &format!("/model/{encoded}/converse"),
            &json!({ "messages": [{"role": "user", "content": [{"text": "hi"}]}] }),
        )
        .await;
        assert_ne!(
            call.status,
            404,
            "case {model_id:?}: an active version must not be answered as not found, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
    }
}

/// RULE: an invoke body is the prompt and inference parameters in the model
/// family's own JSON request format, so a body that does not parse as a JSON object
/// is refused as a malformed input request, and one that omits a member that
/// family's documented request schema requires is refused naming that member
/// (400 `ValidationException`) — on the streaming invoke path as on the
/// non-streaming one, which is why both read the rule from `crate::state`.
#[tokio::test]
async fn a_body_the_model_cannot_read_is_a_malformed_input_request() {
    let router = emulator();
    use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::MALFORMED_BODY_MESSAGE;

    /// The refusal for a body that omits a required member, as the service words
    /// a schema violation.
    fn missing_key(name: &str) -> String {
        format!(
            "Malformed input request: #: required key [{name}] not found, please reformat your \
             input and try again."
        )
    }

    // (case, modelId, body, expected refusal — None when the body is served).
    // Rows differ along the family whose request schema decides, and along the way
    // the body fails it: not JSON at all, a JSON scalar or array rather than an
    // object, another family's prompt member, and each required member of the
    // family's own envelope left out in turn. Required members per family from
    // "Inference request parameters and response fields for foundation models" in
    // the Amazon Bedrock User Guide.
    let rows: Vec<(&str, &str, &str, Option<String>)> = vec![
        (
            "anthropic: its own messages envelope",
            "anthropic.claude-3-haiku-20240307-v1:0",
            r#"{"anthropic_version":"bedrock-2023-05-31","max_tokens":64,"messages":[]}"#,
            None,
        ),
        (
            "anthropic: max_tokens left out",
            "anthropic.claude-3-haiku-20240307-v1:0",
            r#"{"anthropic_version":"bedrock-2023-05-31","messages":[]}"#,
            Some(missing_key("max_tokens")),
        ),
        (
            "anthropic: anthropic_version left out",
            "anthropic.claude-3-haiku-20240307-v1:0",
            r#"{"max_tokens":64,"messages":[]}"#,
            Some(missing_key("anthropic_version")),
        ),
        (
            "anthropic: the legacy text-completions prompt alone",
            "anthropic.claude-3-haiku-20240307-v1:0",
            r#"{"prompt":"\n\nHuman: hi\n\nAssistant:"}"#,
            Some(missing_key("messages")),
        ),
        (
            "anthropic: bytes that are not JSON",
            "anthropic.claude-3-haiku-20240307-v1:0",
            "not json at all",
            Some(MALFORMED_BODY_MESSAGE.to_string()),
        ),
        (
            "anthropic: a JSON array",
            "anthropic.claude-3-haiku-20240307-v1:0",
            r#"[{"role":"user"}]"#,
            Some(MALFORMED_BODY_MESSAGE.to_string()),
        ),
        (
            "anthropic: a JSON string",
            "anthropic.claude-3-haiku-20240307-v1:0",
            r#""hello""#,
            Some(MALFORMED_BODY_MESSAGE.to_string()),
        ),
        (
            "titan: its own envelope",
            "amazon.titan-text-premier-v1:0",
            r#"{"inputText":"hi"}"#,
            None,
        ),
        (
            "titan: another family's prompt member",
            "amazon.titan-text-premier-v1:0",
            r#"{"prompt":"hi"}"#,
            Some(missing_key("inputText")),
        ),
        (
            "nova: its own envelope",
            "amazon.nova-micro-v1:0",
            r#"{"messages":[{"role":"user","content":[{"text":"hi"}]}]}"#,
            None,
        ),
        (
            "nova: another family's prompt member",
            "amazon.nova-micro-v1:0",
            r#"{"inputText":"hi"}"#,
            Some(missing_key("messages")),
        ),
        (
            "llama: its own envelope",
            "meta.llama3-8b-instruct-v1:0",
            r#"{"prompt":"hi"}"#,
            None,
        ),
        (
            "llama: a chat transcript it does not take",
            "meta.llama3-8b-instruct-v1:0",
            r#"{"messages":[]}"#,
            Some(missing_key("prompt")),
        ),
        (
            "mistral: its own prompt envelope",
            "mistral.mixtral-8x7b-instruct-v0:1",
            r#"{"prompt":"hi"}"#,
            None,
        ),
        (
            "mistral: the chat transcript it also takes",
            "mistral.mixtral-8x7b-instruct-v0:1",
            r#"{"messages":[]}"#,
            None,
        ),
        (
            "cohere command: its own envelope",
            "cohere.command-text-v14",
            r#"{"prompt":"hi"}"#,
            None,
        ),
        (
            "cohere command r: the chat envelope",
            "cohere.command-r-plus-v1:0",
            r#"{"message":"hi"}"#,
            None,
        ),
        (
            "cohere: an empty object",
            "cohere.command-text-v14",
            "{}",
            Some(missing_key("prompt")),
        ),
    ];

    for (case, model_id, body, expected) in rows {
        for path in ["invoke", "invoke-with-response-stream"] {
            let call = send(
                &router,
                "POST",
                &format!("/model/{model_id}/{path}"),
                &[("content-type", "application/json")],
                body.as_bytes().to_vec(),
            )
            .await;
            match &expected {
                None => assert_eq!(
                    call.status,
                    200,
                    "case {case:?} on {path}: the family's own envelope is served, got {} / {}",
                    call.status,
                    String::from_utf8_lossy(&call.body)
                ),
                Some(message) => {
                    assert_eq!(
                        call.status,
                        400,
                        "case {case:?} on {path}: expected HTTP 400, got {} / {}",
                        call.status,
                        String::from_utf8_lossy(&call.body)
                    );
                    assert_eq!(
                        call.error_code(),
                        "ValidationException",
                        "case {case:?} on {path}: expected ValidationException, got {:?}",
                        call.error_code()
                    );
                    assert_eq!(
                        call.json()["message"].as_str(),
                        Some(message.as_str()),
                        "case {case:?} on {path}: expected the refusal {message:?}, got {}",
                        call.json()
                    );
                }
            }
        }
    }
}

/// RULE: a streamed inference is recorded in the service's inference log the way a
/// non-streamed one is, under the API that served it — so the log reports what the
/// service actually served rather than what any one surface returned.
#[tokio::test]
async fn a_streamed_inference_is_recorded_in_the_inference_log() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));

    let claude = "anthropic.claude-3-haiku-20240307-v1:0";
    let conversation = json!({ "messages": [{"role": "user", "content": [{"text": "hi"}]}] });
    post_json(&router, &format!("/model/{claude}/converse-stream"), &conversation).await;
    send(
        &router,
        "POST",
        &format!("/model/{claude}/invoke-with-response-stream"),
        &[("content-type", "application/json")],
        // The Anthropic Messages API's own required members, so the body is one
        // the model's documented request schema accepts.
        br#"{"anthropic_version":"bedrock-2023-05-31","max_tokens":64,"messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}]}"#.to_vec(),
    )
    .await;

    let state = service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::service_state(&*engine)
        .expect("the service state store");
    let logged: Vec<_> = state.lock().invocations.iter().cloned().collect();
    let kinds: Vec<_> = logged.iter().map(|entry| entry.kind).collect();
    use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::InvokeKind;
    assert_eq!(
        kinds,
        vec![InvokeKind::ConverseStream, InvokeKind::SyncStream],
        "the two streamed calls must be logged under the API that served each, got {kinds:?}"
    );
    for entry in &logged {
        assert_eq!(
            entry.model_id, claude,
            "every entry names the model served, got {entry:?}"
        );
        assert!(
            entry.input_tokens > 0 && entry.output_tokens > 0,
            "every entry carries the counts actually served, got {entry:?}"
        );
    }
}
