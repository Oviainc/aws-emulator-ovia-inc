// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Bedrock Runtime emulator's first slice: synchronous
//! inference (`InvokeModel`, `Converse`, `CountTokens`) and the async-invoke
//! resource lifecycle (`StartAsyncInvoke`, `GetAsyncInvoke`,
//! `ListAsyncInvokes`).
//!
//! Every test drives real HTTP requests through the production router, so what
//! is asserted is the wire behaviour a client sees — status, headers, body —
//! never a handler called directly.
//!
//! Each assertion spans a SET of inputs: the tables below differ along what the
//! rule they check generalises over (model family, prompt size, message count,
//! token cap, page size, sort order, filter), so no row is the specification on
//! its own.

use std::sync::Arc;
use std::time::Duration;

use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::ops::types::{
    AsyncInvokeOutputDataConfig, AsyncInvokeS3OutputDataConfig,
};
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state;
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState as ServiceState;
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A host-style endpoint for the service, so the request routes even if the
/// SigV4 scope below ever changes.
const ENDPOINT: &str = "https://bedrock-runtime.us-east-1.amazonaws.com";

/// A SigV4 `Authorization` header scoped to this service's signing name
/// (`bedrock`) and to `us-east-1`, which is what `can_handle` routes on and what
/// the emulator reads the ARN region from. Request authentication is off in
/// these tests, so the signature is not checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEMULATOR000000000/20260101/us-east-1/bedrock/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// One recognised model per `InvokeModel` response family, with the family's own
/// request field for the prompt and the response fields the family answers in.
///
/// Model ids from "Amazon Bedrock base model IDs (on-demand throughput)" and the
/// request/response envelopes from "Inference request parameters and response
/// fields for foundation models", both in the Amazon Bedrock User Guide. Each
/// row names a DIFFERENT family, which is what `InvokeModel` generalises over.
const FAMILY_ROWS: &[FamilyRow] = &[
    FamilyRow {
        model_id: "anthropic.claude-3-haiku-20240307-v1:0",
        request: r#"{"anthropic_version":"bedrock-2023-05-31","max_tokens":256,"messages":[{"role":"user","content":[{"type":"text","text":"Summarize the Mars mission in one line."}]}]}"#,
        capped_request: r#"{"anthropic_version":"bedrock-2023-05-31","max_tokens":3,"messages":[{"role":"user","content":[{"type":"text","text":"Summarize the Mars mission in one line."}]}]}"#,
        text_pointer: "/content/0/text",
        input_token_pointer: "/usage/input_tokens",
        output_token_pointer: "/usage/output_tokens",
        stop_pointer: "/stop_reason",
        completed_stop: "end_turn",
        capped_stop: "max_tokens",
    },
    FamilyRow {
        model_id: "amazon.titan-text-express-v1",
        request: r#"{"inputText":"Write a haiku about persistent state.","textGenerationConfig":{"maxTokenCount":512}}"#,
        capped_request: r#"{"inputText":"Write a haiku about persistent state.","textGenerationConfig":{"maxTokenCount":3}}"#,
        text_pointer: "/results/0/outputText",
        input_token_pointer: "/inputTextTokenCount",
        output_token_pointer: "/results/0/tokenCount",
        stop_pointer: "/results/0/completionReason",
        completed_stop: "FINISH",
        capped_stop: "LENGTH",
    },
    FamilyRow {
        model_id: "amazon.nova-pro-v1:0",
        request: r#"{"system":[{"text":"You are terse."}],"messages":[{"role":"user","content":[{"text":"Name two planets."}]}]}"#,
        capped_request: r#"{"inferenceConfig":{"max_new_tokens":3},"system":[{"text":"You are terse."}],"messages":[{"role":"user","content":[{"text":"Name two planets."}]}]}"#,
        text_pointer: "/output/message/content/0/text",
        input_token_pointer: "/usage/inputTokens",
        output_token_pointer: "/usage/outputTokens",
        stop_pointer: "/stopReason",
        completed_stop: "end_turn",
        capped_stop: "max_tokens",
    },
    FamilyRow {
        model_id: "meta.llama3-8b-instruct-v1:0",
        request: r#"{"prompt":"Explain idempotency to a cat.","max_gen_len":128}"#,
        capped_request: r#"{"prompt":"Explain idempotency to a cat.","max_gen_len":3}"#,
        text_pointer: "/generation",
        input_token_pointer: "/prompt_token_count",
        output_token_pointer: "/generation_token_count",
        stop_pointer: "/stop_reason",
        completed_stop: "stop",
        capped_stop: "length",
    },
    FamilyRow {
        model_id: "mistral.mistral-7b-instruct-v0:2",
        request: r#"{"prompt":"<s>[INST] List three primes. [/INST]","max_tokens":64}"#,
        capped_request: r#"{"prompt":"<s>[INST] List three primes. [/INST]","max_tokens":3}"#,
        text_pointer: "/outputs/0/text",
        input_token_pointer: "",
        output_token_pointer: "",
        stop_pointer: "/outputs/0/stop_reason",
        completed_stop: "stop",
        capped_stop: "length",
    },
    FamilyRow {
        model_id: "cohere.command-text-v14",
        request: r#"{"prompt":"Describe a page token.","max_tokens":64}"#,
        capped_request: r#"{"prompt":"Describe a page token.","max_tokens":3}"#,
        text_pointer: "/generations/0/text",
        input_token_pointer: "",
        output_token_pointer: "",
        stop_pointer: "/generations/0/finish_reason",
        completed_stop: "COMPLETE",
        capped_stop: "MAX_TOKENS",
    },
];

/// A row of [`FAMILY_ROWS`].
struct FamilyRow {
    model_id: &'static str,
    /// A request whose generation cap is far above what the emulator returns.
    request: &'static str,
    /// The same request with the family's generation-cap field set to 3 tokens.
    capped_request: &'static str,
    /// JSON pointer to the generated text in the family's response envelope.
    text_pointer: &'static str,
    /// JSON pointer to the input-token count, empty when the family's envelope
    /// reports no token counts (Mistral and Cohere do not).
    input_token_pointer: &'static str,
    /// JSON pointer to the output-token count, empty as above.
    output_token_pointer: &'static str,
    /// JSON pointer to the family's stop/completion reason.
    stop_pointer: &'static str,
    /// The family's reason for a completion that ran to its end.
    completed_stop: &'static str,
    /// The family's reason for a completion cut off by the generation cap.
    capped_stop: &'static str,
}

/// A `Converse` conversation, spanning message count, text size, system blocks
/// and non-ASCII text.
struct ConversationRow {
    case: &'static str,
    system: serde_json::Value,
    messages: serde_json::Value,
}

fn conversation_rows() -> Vec<ConversationRow> {
    vec![
        ConversationRow {
            case: "single short user turn",
            system: serde_json::json!([]),
            messages: serde_json::json!([{"role": "user", "content": [{"text": "hello"}]}]),
        },
        ConversationRow {
            case: "three turns",
            system: serde_json::json!([]),
            messages: serde_json::json!([
                {"role": "user", "content": [{"text": "What is a page token?"}]},
                {"role": "assistant", "content": [{"text": "An opaque cursor."}]},
                {"role": "user", "content": [{"text": "Why opaque?"}]},
            ]),
        },
        ConversationRow {
            case: "system blocks plus one turn",
            system: serde_json::json!([{"text": "You answer in one sentence."}, {"text": "Be literal."}]),
            messages: serde_json::json!([{"role": "user", "content": [{"text": "Define idempotency."}]}]),
        },
        ConversationRow {
            case: "long prompt",
            system: serde_json::json!([]),
            messages: serde_json::json!([
                {"role": "user", "content": [{"text": "word ".repeat(500)}]},
            ]),
        },
        ConversationRow {
            case: "non-ascii prompt",
            system: serde_json::json!([]),
            messages: serde_json::json!([
                {"role": "user", "content": [{"text": "日本語のトークンと émojis 🙂 を数える"}]},
            ]),
        },
        ConversationRow {
            case: "several content blocks in one turn",
            system: serde_json::json!([]),
            messages: serde_json::json!([
                {"role": "user", "content": [{"text": "first part,"}, {"text": "second part."}]},
            ]),
        },
    ]
}

/// A row of the `InvokeModel` refusal table: the case, the request headers and
/// the request body.
struct InvokeRefusalRow {
    case: &'static str,
    headers: Vec<(&'static str, &'static str)>,
    body: Vec<u8>,
}

/// The emulator under test: the production router with this one service mounted
/// on a shared engine, exactly as `core/router` assembles it. The engine is
/// returned too, so a test can read the service's own state the way the next
/// request would.
fn emulator_with_engine() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// The emulator under test, when a test does not read the state directly.
fn emulator() -> MultiServiceRouter {
    emulator_with_engine().1
}

/// One HTTP exchange.
struct Call {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Call {
    fn json(&self) -> serde_json::Value {
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
            .and_then(serde_json::Value::as_str)
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

async fn post_json(
    router: &MultiServiceRouter,
    path: &str,
    body: &serde_json::Value,
) -> Call {
    send(
        router,
        "POST",
        path,
        &[("content-type", "application/json")],
        serde_json::to_vec(body).expect("a serializable test body"),
    )
    .await
}

async fn get(router: &MultiServiceRouter, path: &str) -> Call {
    send(router, "GET", path, &[], Vec::new()).await
}

/// `CountTokens` over a `Converse`-shaped input.
async fn count_converse_tokens(
    router: &MultiServiceRouter,
    model_id: &str,
    system: &serde_json::Value,
    messages: &serde_json::Value,
) -> i64 {
    let mut converse = serde_json::json!({ "messages": messages });
    if system.as_array().is_some_and(|blocks| !blocks.is_empty()) {
        converse["system"] = system.clone();
    }
    let call = post_json(
        router,
        &format!("/model/{model_id}/count-tokens"),
        &serde_json::json!({ "input": { "converse": converse } }),
    )
    .await;
    assert_eq!(
        call.status,
        200,
        "CountTokens must succeed, got {} / {}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );
    call.json()["inputTokens"]
        .as_i64()
        .expect("inputTokens is a modelled integer")
}

// ---------------------------------------------------------------------------
// Synchronous inference
// ---------------------------------------------------------------------------

/// RULE: `CountTokens` and `Converse` count the tokens of one input with the
/// same tokenizer, so for identical input the count the two report is the same
/// one, and a `Converse` turn reports `usage.totalTokens` as the sum of the
/// input and output counts it actually served.
#[tokio::test]
async fn count_tokens_and_converse_report_the_same_input_token_count() {
    let router = emulator();
    let model_id = "anthropic.claude-3-sonnet-20240229-v1:0";

    for row in conversation_rows() {
        let case = row.case;
        let counted =
            count_converse_tokens(&router, model_id, &row.system, &row.messages).await;

        let mut request = serde_json::json!({ "messages": row.messages });
        if row.system.as_array().is_some_and(|blocks| !blocks.is_empty()) {
            request["system"] = row.system.clone();
        }
        let call = post_json(&router, &format!("/model/{model_id}/converse"), &request).await;
        assert_eq!(
            call.status,
            200,
            "case {case:?}: expected Converse to answer 200, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        let body = call.json();

        assert!(
            counted > 0,
            "case {case:?}: expected a positive token count, got {counted}"
        );
        let usage = &body["usage"];
        assert_eq!(
            usage["inputTokens"].as_i64(),
            Some(counted),
            "case {case:?}: expected Converse usage.inputTokens to equal the CountTokens \
             result {counted}, got {:?}",
            usage["inputTokens"]
        );
        let input = usage["inputTokens"].as_i64().unwrap_or_default();
        let output = usage["outputTokens"].as_i64().unwrap_or_default();
        assert_eq!(
            usage["totalTokens"].as_i64(),
            Some(input + output),
            "case {case:?}: expected totalTokens == inputTokens + outputTokens, got {:?}",
            usage["totalTokens"]
        );
        assert_eq!(
            body["output"]["message"]["role"].as_str(),
            Some("assistant"),
            "case {case:?}: expected an assistant message, got {:?}",
            body["output"]["message"]["role"]
        );
        let text = body["output"]["message"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default();
        assert!(
            !text.is_empty(),
            "case {case:?}: expected a non-empty content block, got {:?}",
            body["output"]["message"]["content"]
        );
        assert_eq!(
            state::count_text_tokens(text) as i64,
            output,
            "case {case:?}: expected outputTokens to be the tokens of the returned text, \
             got {output} for {text:?}"
        );
        assert_eq!(
            body["stopReason"].as_str(),
            Some("end_turn"),
            "case {case:?}: expected stopReason end_turn, got {:?}",
            body["stopReason"]
        );
        assert!(
            body["metrics"]["latencyMs"].as_i64().unwrap_or(-1) >= 0,
            "case {case:?}: expected a measured latency, got {:?}",
            body["metrics"]["latencyMs"]
        );
    }
}

/// RULE: a `Converse` turn stops on `max_tokens` when the generated text reaches
/// the cap `inferenceConfig.maxTokens` sets (bounded in turn by the model's own
/// ceiling), and on `end_turn` when it does not; the output count never exceeds
/// the cap.
#[tokio::test]
async fn converse_stops_on_max_tokens_exactly_when_the_cap_is_reached() {
    let router = emulator();
    let model_id = "amazon.nova-lite-v1:0";
    let messages = serde_json::json!([{"role": "user", "content": [{"text": "hello"}]}]);

    // One row per published bound of `maxTokens` (its modelled minimum, 1) and
    // around the length of the text this emulator returns, plus a cap far above
    // it.
    for cap in [1_i64, 2, 5, 17, 100, 4096] {
        let call = post_json(
            &router,
            &format!("/model/{model_id}/converse"),
            &serde_json::json!({
                "messages": messages,
                "inferenceConfig": { "maxTokens": cap },
            }),
        )
        .await;
        assert_eq!(
            call.status,
            200,
            "case maxTokens={cap}: expected 200, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        let body = call.json();
        let output = body["usage"]["outputTokens"].as_i64().unwrap_or_default();
        assert!(
            output <= cap,
            "case maxTokens={cap}: expected at most {cap} output tokens, got {output}"
        );
        let text = body["output"]["message"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default();
        let untruncated = state::count_text_tokens(&state::emulated_completion_text(
            model_id,
            1,
            body["usage"]["inputTokens"].as_i64().unwrap_or_default() as u32,
        )) as i64;
        let expected = if untruncated > cap {
            "max_tokens"
        } else {
            "end_turn"
        };
        assert_eq!(
            body["stopReason"].as_str(),
            Some(expected),
            "case maxTokens={cap}: expected stopReason {expected} for a {untruncated}-token \
             completion, got {:?} with text {text:?}",
            body["stopReason"]
        );
    }
}

/// RULE: `InvokeModel` answers in the response envelope of the invoked model's
/// own family, as a JSON body with a `Content-Type` of `application/json`, and
/// the token counts it reports are the ones the tokenizer computes for the
/// request body — the same count `CountTokens` reports for that body.
#[tokio::test]
async fn invoke_model_answers_in_each_family_response_envelope() {
    let router = emulator();

    for row in FAMILY_ROWS {
        let case = row.model_id;
        let call = send(
            &router,
            "POST",
            &format!("/model/{}/invoke", row.model_id),
            &[("content-type", "application/json"), ("accept", "application/json")],
            row.request.as_bytes().to_vec(),
        )
        .await;
        assert_eq!(
            call.status,
            200,
            "case {case:?}: expected 200, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.header("content-type"),
            "application/json",
            "case {case:?}: expected the modelled Content-Type response header, got {:?}",
            call.header("content-type")
        );
        assert_eq!(
            call.header("x-amzn-bedrock-performanceconfig-latency"),
            "standard",
            "case {case:?}: expected the defaulted performanceConfigLatency echoed back, got {:?}",
            call.header("x-amzn-bedrock-performanceconfig-latency")
        );

        let body = call.json();
        let text = body
            .pointer(row.text_pointer)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {case:?}: no text at {} in {body}", row.text_pointer)
            });
        assert!(
            !text.is_empty(),
            "case {case:?}: expected a non-empty completion at {}",
            row.text_pointer
        );

        let counted = post_json(
            &router,
            &format!("/model/{}/count-tokens", row.model_id),
            &serde_json::json!({
                "input": { "invokeModel": { "body": base64(row.request.as_bytes()) } },
            }),
        )
        .await;
        assert_eq!(
            counted.status,
            200,
            "case {case:?}: expected CountTokens to answer 200, got {} / {}",
            counted.status,
            String::from_utf8_lossy(&counted.body)
        );
        let counted = counted.json()["inputTokens"].as_i64().unwrap_or_default();
        assert!(
            counted > 0,
            "case {case:?}: expected a positive token count for the body, got {counted}"
        );
        if !row.input_token_pointer.is_empty() {
            assert_eq!(
                body.pointer(row.input_token_pointer)
                    .and_then(serde_json::Value::as_i64),
                Some(counted),
                "case {case:?}: expected {} to be the CountTokens result {counted} in {body}",
                row.input_token_pointer
            );
            assert_eq!(
                body.pointer(row.output_token_pointer)
                    .and_then(serde_json::Value::as_i64),
                Some(i64::from(state::count_text_tokens(text))),
                "case {case:?}: expected {} to be the tokens of the returned text in {body}",
                row.output_token_pointer
            );
        }
        assert_eq!(
            body.pointer(row.stop_pointer)
                .and_then(serde_json::Value::as_str),
            Some(row.completed_stop),
            "case {case:?}: expected the family's completed stop reason at {} in {body}",
            row.stop_pointer
        );
    }
}

/// RULE: `InvokeModel` honours the generation cap the request body carries for
/// the invoked model's family: the completion is cut to at most that many tokens
/// and the family's response reports its own length stop reason.
#[tokio::test]
async fn invoke_model_honours_the_generation_cap_the_body_carries() {
    let router = emulator();

    for row in FAMILY_ROWS {
        let case = row.model_id;
        let call = send(
            &router,
            "POST",
            &format!("/model/{}/invoke", row.model_id),
            &[("content-type", "application/json")],
            row.capped_request.as_bytes().to_vec(),
        )
        .await;
        assert_eq!(
            call.status,
            200,
            "case {case:?}: expected 200, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        let body = call.json();
        let text = body
            .pointer(row.text_pointer)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no text at {} in {body}", row.text_pointer));
        assert!(
            state::count_text_tokens(text) <= 3,
            "case {case:?}: expected at most the 3 tokens the body asked for, got {} for {text:?}",
            state::count_text_tokens(text)
        );
        assert_eq!(
            body.pointer(row.stop_pointer)
                .and_then(serde_json::Value::as_str),
            Some(row.capped_stop),
            "case {case:?}: expected the family's length stop reason at {} in {body}",
            row.stop_pointer
        );
        if !row.output_token_pointer.is_empty() {
            assert_eq!(
                body.pointer(row.output_token_pointer)
                    .and_then(serde_json::Value::as_i64),
                Some(i64::from(state::count_text_tokens(text))),
                "case {case:?}: expected {} to be the tokens of the cut text in {body}",
                row.output_token_pointer
            );
        }
    }
}

/// RULE: every served inference call is recorded in the service's own inference
/// log — which API served it, for which model, and the token counts it reported
/// — and the log is bounded, so it keeps at most its retention limit of entries.
#[tokio::test]
async fn every_served_inference_call_is_recorded_in_the_inference_log() {
    let (engine, router) = emulator_with_engine();
    let model_id = "amazon.nova-lite-v1:0";

    let invoked = send(
        &router,
        "POST",
        &format!("/model/{model_id}/invoke"),
        &[("content-type", "application/json")],
        br#"{"messages":[{"role":"user","content":[{"text":"count me"}]}]}"#.to_vec(),
    )
    .await;
    assert_eq!(invoked.status, 200, "InvokeModel must be served");
    let conversed = post_json(
        &router,
        &format!("/model/{model_id}/converse"),
        &serde_json::json!({"messages": [{"role": "user", "content": [{"text": "count me too"}]}]}),
    )
    .await;
    assert_eq!(conversed.status, 200, "Converse must be served");
    let started = post_json(
        &router,
        "/async-invoke",
        &serde_json::json!({
            "modelId": model_id,
            "modelInput": {"inputText": "and me"},
            "outputDataConfig": {"s3OutputDataConfig": {"s3Uri": "s3://bucket/out"}},
        }),
    )
    .await;
    assert_eq!(started.status, 200, "StartAsyncInvoke must be served");
    // A reporting operation must add nothing to the log.
    let _ = count_converse_tokens(
        &router,
        model_id,
        &serde_json::json!([]),
        &serde_json::json!([{"role": "user", "content": [{"text": "not an inference call"}]}]),
    )
    .await;

    let state_handle = engine.state::<ServiceState>();
    let logged = state_handle.lock().invocations.clone();
    assert_eq!(
        logged.len(),
        3,
        "expected one log entry per served inference call, got {logged:?}"
    );
    let kinds: Vec<state::InvokeKind> = logged.iter().map(|entry| entry.kind).collect();
    assert_eq!(
        kinds,
        vec![
            state::InvokeKind::Sync,
            state::InvokeKind::Converse,
            state::InvokeKind::Async
        ],
        "expected the log in call order with each API's own kind, got {kinds:?}"
    );
    for entry in &logged {
        assert_eq!(
            entry.model_id, model_id,
            "expected the resolved model id on every entry, got {entry:?}"
        );
        assert!(
            entry.input_tokens > 0,
            "expected the counted input tokens on every entry, got {entry:?}"
        );
    }
    assert!(
        logged.len() <= state::MAX_INVOCATION_LOG_ENTRIES,
        "expected a bounded log, got {} entries",
        logged.len()
    );
}

/// RULE: an operation that takes a `modelId` resolves it against the registry of
/// recognised models — accepting the bare id, a `foundation-model` ARN and a
/// cross-region inference profile id — and answers `ResourceNotFoundException`
/// (404) for a model that is not in it.
#[tokio::test]
async fn an_unrecognised_model_is_not_found_on_every_inference_operation() {
    let router = emulator();
    let invoke_body = br#"{"prompt":"hi"}"#.to_vec();

    // Spellings of ONE registered model the resolver must accept.
    let known = "meta.llama3-70b-instruct-v1:0";
    for spelling in [
        known.to_string(),
        format!("us.{known}"),
        format!("arn:aws:bedrock:us-east-1:000000000000:foundation-model/{known}"),
    ] {
        let encoded = spelling.replace(':', "%3A").replace('/', "%2F");
        let call = send(
            &router,
            "POST",
            &format!("/model/{encoded}/invoke"),
            &[("content-type", "application/json")],
            invoke_body.clone(),
        )
        .await;
        assert_eq!(
            call.status,
            200,
            "case {spelling:?}: expected a recognised model to be served, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
    }

    // Unrecognised spellings, across all three inference operations.
    for model_id in [
        "anthropic.claude-9-imaginary-v1%3A0",
        "nobody.such-model-v1",
        "arn%3Aaws%3Abedrock%3Aus-east-1%3A000000000000%3Afoundation-model%2Fnobody.nope-v1",
    ] {
        let calls = vec![
            (
                "InvokeModel",
                send(
                    &router,
                    "POST",
                    &format!("/model/{model_id}/invoke"),
                    &[("content-type", "application/json")],
                    invoke_body.clone(),
                )
                .await,
            ),
            (
                "Converse",
                post_json(
                    &router,
                    &format!("/model/{model_id}/converse"),
                    &serde_json::json!({
                        "messages": [{"role": "user", "content": [{"text": "hi"}]}],
                    }),
                )
                .await,
            ),
            (
                "CountTokens",
                post_json(
                    &router,
                    &format!("/model/{model_id}/count-tokens"),
                    &serde_json::json!({
                        "input": { "invokeModel": { "body": base64(b"{\"prompt\":\"hi\"}") } },
                    }),
                )
                .await,
            ),
        ];
        for (operation, call) in calls {
            assert_eq!(
                call.status,
                404,
                "case {operation} {model_id:?}: expected 404, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            assert_eq!(
                call.error_code(),
                "ResourceNotFoundException",
                "case {operation} {model_id:?}: expected ResourceNotFoundException, got {:?}",
                call.error_code()
            );
        }
    }
}

/// RULE: a request that breaks a rule the service states but the model cannot
/// express is refused with the modelled `ValidationException` (400) — for
/// `Converse`, a conversation that is not a non-empty user-first alternating
/// sequence or a model the Converse API does not serve; for `InvokeModel`, a
/// body that is not `application/json`, an empty body, and the guardrail
/// combinations the operation documents as errors.
#[tokio::test]
async fn a_request_that_breaks_a_stated_rule_is_refused_with_validation_exception() {
    let router = emulator();
    let claude = "anthropic.claude-3-haiku-20240307-v1:0";

    let converse_rows: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "no messages at all",
            claude,
            serde_json::json!({ "messages": [] }),
        ),
        (
            "leading assistant turn",
            claude,
            serde_json::json!({
                "messages": [{"role": "assistant", "content": [{"text": "hi"}]}],
            }),
        ),
        (
            "two user turns in a row",
            claude,
            serde_json::json!({
                "messages": [
                    {"role": "user", "content": [{"text": "one"}]},
                    {"role": "user", "content": [{"text": "two"}]},
                ],
            }),
        ),
        (
            "two assistant turns in a row",
            claude,
            serde_json::json!({
                "messages": [
                    {"role": "user", "content": [{"text": "one"}]},
                    {"role": "assistant", "content": [{"text": "two"}]},
                    {"role": "assistant", "content": [{"text": "three"}]},
                ],
            }),
        ),
        (
            "a model the Converse API does not serve",
            "cohere.command-text-v14",
            serde_json::json!({
                "messages": [{"role": "user", "content": [{"text": "hi"}]}],
            }),
        ),
    ];
    for (case, model_id, request) in converse_rows {
        let call = post_json(&router, &format!("/model/{model_id}/converse"), &request).await;
        assert_eq!(
            call.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            call.error_code()
        );
    }

    let invoke_rows = vec![
        InvokeRefusalRow {
            case: "a non-JSON content type",
            headers: vec![("content-type", "text/plain")],
            body: br#"{"prompt":"hi"}"#.to_vec(),
        },
        InvokeRefusalRow {
            case: "an unsupported accept type",
            headers: vec![
                ("content-type", "application/json"),
                ("accept", "image/png"),
            ],
            body: br#"{"prompt":"hi"}"#.to_vec(),
        },
        InvokeRefusalRow {
            case: "an empty body",
            headers: vec![("content-type", "application/json")],
            body: Vec::new(),
        },
        InvokeRefusalRow {
            case: "a guardrail identifier without a version",
            headers: vec![
                ("content-type", "application/json"),
                ("x-amzn-bedrock-guardrailidentifier", "abc123"),
            ],
            body: br#"{"prompt":"hi"}"#.to_vec(),
        },
        InvokeRefusalRow {
            case: "an inline guardrail config without an identifier",
            headers: vec![("content-type", "application/json")],
            body: br#"{"prompt":"hi","amazon-bedrock-guardrailConfig":{"tagSuffix":"x"}}"#
                .to_vec(),
        },
    ];
    for row in invoke_rows {
        let case = row.case;
        let call = send(
            &router,
            "POST",
            &format!("/model/{claude}/invoke"),
            &row.headers,
            row.body,
        )
        .await;
        assert_eq!(
            call.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            call.error_code()
        );
    }

    // The same request with the guardrail version supplied is served.
    let served = send(
        &router,
        "POST",
        &format!("/model/{claude}/invoke"),
        &[
            ("content-type", "application/json"),
            ("x-amzn-bedrock-guardrailidentifier", "abc123"),
            ("x-amzn-bedrock-guardrailversion", "1"),
        ],
        br#"{"prompt":"hi"}"#.to_vec(),
    )
    .await;
    assert_eq!(
        served.status,
        200,
        "case \"a complete guardrail configuration\": expected 200, got {} / {}",
        served.status,
        String::from_utf8_lossy(&served.body)
    );
}

// ---------------------------------------------------------------------------
// The async-invoke lifecycle
// ---------------------------------------------------------------------------

/// RULE: `StartAsyncInvoke` persists one invocation record and `GetAsyncInvoke`
/// reports exactly that record; the operation is idempotent, so the same
/// `clientRequestToken` with the same payload names the invocation already
/// minted and with a different payload is a `ConflictException`; and an ARN this
/// emulator never minted is refused, never answered with a record.
#[tokio::test]
async fn start_async_invoke_persists_a_record_that_get_async_invoke_reports_back() {
    let router = emulator();
    let model_id = "amazon.nova-pro-v1:0";

    // Rows differ in token (present/absent), tags and output configuration.
    let rows: Vec<(&str, serde_json::Value)> = vec![
        (
            "with a token and one tag",
            serde_json::json!({
                "modelId": model_id,
                "clientRequestToken": "tok-1",
                "modelInput": {"messages": [{"role": "user", "content": [{"text": "render"}]}]},
                "outputDataConfig": {"s3OutputDataConfig": {"s3Uri": "s3://bucket/prefix"}},
                "tags": [{"key": "team", "value": "platform"}],
            }),
        ),
        (
            "without a token",
            serde_json::json!({
                "modelId": model_id,
                "modelInput": {"inputText": "render something else"},
                "outputDataConfig": {"s3OutputDataConfig": {"s3Uri": "s3://other-bucket/out"}},
            }),
        ),
        (
            "with a bucket owner and a KMS key",
            serde_json::json!({
                "modelId": model_id,
                "clientRequestToken": "tok-3",
                "modelInput": {"inputText": "third"},
                "outputDataConfig": {"s3OutputDataConfig": {
                    "s3Uri": "s3://bucket/third",
                    "bucketOwner": "000000000000",
                }},
            }),
        ),
    ];

    for (case, request) in &rows {
        let started = post_json(&router, "/async-invoke", request).await;
        assert_eq!(
            started.status,
            200,
            "case {case:?}: expected StartAsyncInvoke to answer 200, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        let arn = started.json()["invocationArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_invocation_arn(&arn),
            "case {case:?}: expected an invocationArn matching the modelled pattern, got {arn:?}"
        );

        let fetched = get(&router, &format!("/async-invoke/{}", encode_path(&arn))).await;
        assert_eq!(
            fetched.status,
            200,
            "case {case:?}: expected GetAsyncInvoke to answer 200, got {} / {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        let record = fetched.json();
        assert_eq!(
            record["invocationArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the stored invocationArn, got {:?}",
            record["invocationArn"]
        );
        assert_eq!(
            record["modelArn"].as_str(),
            Some(
                format!("arn:aws:bedrock:us-east-1:000000000000:foundation-model/{model_id}")
                    .as_str()
            ),
            "case {case:?}: expected the modelArn resolved from the submitted modelId, got {:?}",
            record["modelArn"]
        );
        assert_eq!(
            record["status"].as_str(),
            Some("InProgress"),
            "case {case:?}: expected a freshly submitted invocation to be InProgress, got {:?}",
            record["status"]
        );
        assert_eq!(
            record["outputDataConfig"],
            request["outputDataConfig"],
            "case {case:?}: expected the stored outputDataConfig, got {:?}",
            record["outputDataConfig"]
        );
        assert_eq!(
            record["clientRequestToken"].as_str(),
            request["clientRequestToken"].as_str(),
            "case {case:?}: expected the stored clientRequestToken, got {:?}",
            record["clientRequestToken"]
        );
        let submitted = instant_of(&record["submitTime"]).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a submitTime, got {:?}",
                record["submitTime"]
            )
        });
        assert!(
            (submitted - now_seconds()).abs() < 60.0,
            "case {case:?}: expected submitTime to be the instant of the call, got {submitted}"
        );
        assert!(
            record["failureMessage"].is_null(),
            "case {case:?}: expected no failureMessage while InProgress, got {:?}",
            record["failureMessage"]
        );
    }

    // Idempotency: the identical request under a used token returns the same ARN.
    let first = post_json(&router, "/async-invoke", &rows[0].1).await;
    let repeat = post_json(&router, "/async-invoke", &rows[0].1).await;
    assert_eq!(
        repeat.status, 200,
        "case \"idempotent repeat\": expected 200, got {} / {}",
        repeat.status,
        String::from_utf8_lossy(&repeat.body)
    );
    assert_eq!(
        repeat.json()["invocationArn"],
        first.json()["invocationArn"],
        "case \"idempotent repeat\": expected the same invocationArn, got {:?} and {:?}",
        first.json()["invocationArn"],
        repeat.json()["invocationArn"]
    );

    // The same token with a different payload conflicts.
    for (case, change) in [
        ("a different modelInput", serde_json::json!({"inputText": "different"})),
        ("a different output location", serde_json::Value::Null),
    ] {
        let mut conflicting = rows[0].1.clone();
        if change.is_null() {
            conflicting["outputDataConfig"] =
                serde_json::json!({"s3OutputDataConfig": {"s3Uri": "s3://bucket/elsewhere"}});
        } else {
            conflicting["modelInput"] = change;
        }
        let call = post_json(&router, "/async-invoke", &conflicting).await;
        assert_eq!(
            call.status,
            400,
            "case {case:?}: expected a 400 ConflictException, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {:?}",
            call.error_code()
        );
    }

    // A well-formed ARN this emulator never minted resolves to nothing.
    for unminted in [
        "arn:aws:bedrock:us-east-1:000000000000:async-invoke/abcdef123456",
        "arn:aws:bedrock:eu-west-1:000000000000:async-invoke/000000000000",
    ] {
        let call = get(&router, &format!("/async-invoke/{}", encode_path(unminted))).await;
        assert_eq!(
            call.status,
            400,
            "case {unminted:?}: expected the modelled 400 refusal, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {unminted:?}: expected ValidationException, got {:?}",
            call.error_code()
        );
    }

    // An unrecognised model cannot be invoked asynchronously either.
    let unknown = post_json(
        &router,
        "/async-invoke",
        &serde_json::json!({
            "modelId": "nobody.such-model-v1",
            "modelInput": {"inputText": "x"},
            "outputDataConfig": {"s3OutputDataConfig": {"s3Uri": "s3://bucket/x"}},
        }),
    )
    .await;
    assert_eq!(
        unknown.status, 404,
        "case \"unrecognised model\": expected 404, got {} / {}",
        unknown.status,
        String::from_utf8_lossy(&unknown.body)
    );
}

/// RULE: a submitted invocation leaves `InProgress` only when the clock makes
/// the transition due, and — because no model runs in this emulator and nothing
/// is written to the caller's S3 location — it ends in the modelled `Failed`
/// state carrying the reason, with `endTime` and `lastModifiedTime` set.
#[tokio::test]
async fn an_in_progress_invocation_settles_into_the_modelled_failure_state() {
    let router = emulator();
    let started = post_json(
        &router,
        "/async-invoke",
        &serde_json::json!({
            "modelId": "amazon.nova-pro-v1:0",
            "modelInput": {"inputText": "render"},
            "outputDataConfig": {"s3OutputDataConfig": {"s3Uri": "s3://bucket/prefix"}},
        }),
    )
    .await;
    let arn = started.json()["invocationArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let immediate = get(&router, &format!("/async-invoke/{}", encode_path(&arn))).await;
    assert_eq!(
        immediate.json()["status"].as_str(),
        Some("InProgress"),
        "case \"read immediately\": expected InProgress, got {:?}",
        immediate.json()["status"]
    );

    // The transition is due a fixed span after the submitting call; nothing in
    // the emulator sleeps for it, so the test is what waits.
    std::thread::sleep(Duration::from_millis(
        u64::try_from(state::ASYNC_INVOKE_SETTLE_MILLIS).unwrap_or(500) + 150,
    ));

    let settled = get(&router, &format!("/async-invoke/{}", encode_path(&arn))).await;
    let record = settled.json();
    assert_eq!(
        record["status"].as_str(),
        Some("Failed"),
        "case \"read after the transition is due\": expected Failed, got {:?}",
        record["status"]
    );
    assert_eq!(
        record["failureMessage"].as_str(),
        Some(state::ASYNC_INVOKE_FAILURE_MESSAGE),
        "case \"read after the transition is due\": expected the recorded reason, got {:?}",
        record["failureMessage"]
    );
    let submitted = instant_of(&record["submitTime"]).unwrap_or_default();
    let ended = instant_of(&record["endTime"]).unwrap_or_else(|| {
        panic!(
            "case \"read after the transition is due\": expected an endTime, got {:?}",
            record["endTime"]
        )
    });
    assert!(
        ended >= submitted,
        "case \"read after the transition is due\": expected endTime {ended} at or after \
         submitTime {submitted}"
    );
    assert!(
        instant_of(&record["lastModifiedTime"]).is_some(),
        "case \"read after the transition is due\": expected a lastModifiedTime, got {:?}",
        record["lastModifiedTime"]
    );

    // Reading again reports the same terminal record: a read changes nothing a
    // later caller sees.
    let again = get(&router, &format!("/async-invoke/{}", encode_path(&arn))).await;
    assert_eq!(
        again.json(),
        record,
        "case \"read twice\": expected an identical record, got {:?}",
        again.json()
    );
}

/// RULE: `ListAsyncInvokes` reports the invocations `StartAsyncInvoke` created,
/// sorted by submit time in the requested order, cut into pages of at most
/// `maxResults` with a `nextToken` that resumes exactly after the last item of
/// the page it came with and is absent on the last page; every record appears
/// once across the pages.
#[tokio::test]
async fn list_async_invokes_pages_in_the_requested_order_without_gaps_or_duplicates() {
    let router = emulator();
    let mut created = Vec::new();
    for index in 0..5 {
        let started = post_json(
            &router,
            "/async-invoke",
            &serde_json::json!({
                "modelId": "amazon.nova-lite-v1:0",
                "clientRequestToken": format!("tok-{index}"),
                "modelInput": {"inputText": format!("job {index}")},
                "outputDataConfig": {"s3OutputDataConfig": {
                    "s3Uri": format!("s3://bucket/job-{index}"),
                }},
            }),
        )
        .await;
        assert_eq!(
            started.status,
            200,
            "case \"create job {index}\": expected 200, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        created.push(
            started.json()["invocationArn"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
        // Distinct submit times, so the ordering assertions below are about the
        // sort key rather than about the tie-break.
        std::thread::sleep(Duration::from_millis(5));
    }

    // Every page size, including one page per record and one page for all of
    // them, must enumerate the same records in the same order.
    let mut per_page_size = Vec::new();
    for page_size in [1_usize, 2, 3, 5, 1000] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let call = get(&router, &format!("/async-invoke{query}")).await;
            assert_eq!(
                call.status,
                200,
                "case page_size={page_size}: expected 200, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            let body = call.json();
            let summaries = body["asyncInvokeSummaries"]
                .as_array()
                .unwrap_or_else(|| panic!("case page_size={page_size}: no summaries in {body}"))
                .clone();
            assert!(
                summaries.len() <= page_size,
                "case page_size={page_size}: expected at most {page_size} summaries, got {}",
                summaries.len()
            );
            for summary in &summaries {
                seen.push(
                    summary["invocationArn"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            pages += 1;
            assert!(
                pages <= 10,
                "case page_size={page_size}: paging did not terminate after {pages} pages"
            );
            match body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen.len(),
            created.len(),
            "case page_size={page_size}: expected every record exactly once, got {seen:?}"
        );
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case page_size={page_size}: expected no duplicates across pages, got {seen:?}"
        );
        per_page_size.push(seen);
    }
    for (index, enumeration) in per_page_size.iter().enumerate() {
        assert_eq!(
            enumeration, &per_page_size[0],
            "case page_size row {index}: expected paging to enumerate the same order as a \
             single page, got {enumeration:?}"
        );
    }

    // Descending (the modelled default) is newest-first; Ascending is its
    // reverse.
    let descending = listed_arns(&router, "?sortOrder=Descending").await;
    let defaulted = listed_arns(&router, "").await;
    let ascending = listed_arns(&router, "?sortOrder=Ascending").await;
    assert_eq!(
        defaulted, descending,
        "case \"defaulted sortOrder\": expected the modelled default Descending, got {defaulted:?}"
    );
    assert_eq!(
        descending,
        created.iter().rev().cloned().collect::<Vec<_>>(),
        "case \"Descending\": expected newest-first, got {descending:?}"
    );
    assert_eq!(
        ascending, created,
        "case \"Ascending\": expected oldest-first, got {ascending:?}"
    );

    // An opaque token this emulator did not mint is refused.
    for bad_token in ["not-a-token", "6162"] {
        let call = get(&router, &format!("/async-invoke?nextToken={bad_token}")).await;
        assert_eq!(
            call.status,
            400,
            "case {bad_token:?}: expected 400, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {bad_token:?}: expected ValidationException, got {:?}",
            call.error_code()
        );
    }
}

/// RULE: `ListAsyncInvokes` applies its filters to the records it holds: a
/// status filter keeps the records in that status, and the submit-time bounds
/// keep the records submitted within them.
#[tokio::test]
async fn list_async_invokes_applies_its_status_and_submit_time_filters() {
    let router = emulator();
    let mut arns = Vec::new();
    let mut submitted = Vec::new();
    for index in 0..3 {
        let started = post_json(
            &router,
            "/async-invoke",
            &serde_json::json!({
                "modelId": "amazon.titan-text-lite-v1",
                "modelInput": {"inputText": format!("job {index}")},
                "outputDataConfig": {"s3OutputDataConfig": {"s3Uri": "s3://bucket/out"}},
            }),
        )
        .await;
        let arn = started.json()["invocationArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let fetched = get(&router, &format!("/async-invoke/{}", encode_path(&arn))).await;
        submitted.push(
            instant_of(&fetched.json()["submitTime"])
                .expect("GetAsyncInvoke reports a submitTime"),
        );
        arns.push(arn);
        std::thread::sleep(Duration::from_millis(5));
    }

    // While the transition is not yet due every record is InProgress, and no
    // record is in any other status.
    for (status, expected) in [("InProgress", 3_usize), ("Completed", 0), ("Failed", 0)] {
        let listed = listed_arns(&router, &format!("?statusEquals={status}")).await;
        assert_eq!(
            listed.len(),
            expected,
            "case statusEquals={status}: expected {expected} record(s), got {listed:?}"
        );
    }

    // The submit-time bounds keep the records submitted inside them. The bounds
    // are set between two records' submit instants, so what is asserted is the
    // filter and not the formatting precision of one boundary value.
    let between_first_two = rfc3339_query(midpoint(submitted[0], submitted[1]));
    let after = listed_arns(
        &router,
        &format!("?submitTimeAfter={between_first_two}&sortOrder=Ascending"),
    )
    .await;
    assert_eq!(
        after,
        arns[1..].to_vec(),
        "case submitTimeAfter=between the first two: expected the later records, got {after:?}"
    );
    let between_last_two = rfc3339_query(midpoint(submitted[1], submitted[2]));
    let before = listed_arns(
        &router,
        &format!("?submitTimeBefore={between_last_two}&sortOrder=Ascending"),
    )
    .await;
    assert_eq!(
        before,
        arns[..2].to_vec(),
        "case submitTimeBefore=between the last two: expected the earlier records, got \
         {before:?}"
    );
    let window = listed_arns(
        &router,
        &format!(
            "?submitTimeAfter={between_first_two}&submitTimeBefore={between_last_two}\
             &sortOrder=Ascending"
        ),
    )
    .await;
    assert_eq!(
        window,
        arns[1..2].to_vec(),
        "case \"both bounds\": expected only the middle record, got {window:?}"
    );

    // Once the clock has made the transition due, the status filter follows the
    // records into their terminal state.
    std::thread::sleep(Duration::from_millis(
        u64::try_from(state::ASYNC_INVOKE_SETTLE_MILLIS).unwrap_or(500) + 150,
    ));
    let failed = listed_arns(&router, "?statusEquals=Failed").await;
    assert_eq!(
        failed.len(),
        3,
        "case \"statusEquals=Failed after the transition is due\": expected 3 records, got {failed:?}"
    );
    let still_running = listed_arns(&router, "?statusEquals=InProgress").await;
    assert!(
        still_running.is_empty(),
        "case \"statusEquals=InProgress after the transition is due\": expected none, got \
         {still_running:?}"
    );
}

/// RULE: the tokenizer every operation counts with is a function of the text
/// alone: counting is monotonic in the text, cutting a text to N tokens yields
/// at most N tokens, and a text already shorter than the cut is returned whole.
#[tokio::test]
async fn the_tokenizer_counts_and_cuts_consistently() {
    let cases = [
        "",
        "a",
        "hello",
        "hello world",
        "tokens, punctuation; and symbols!",
        "s3://bucket/prefix",
        "日本語のトークン",
        "a very long sentence that keeps going and going and going for quite a while",
    ];
    for text in cases {
        let total = state::count_text_tokens(text);
        for cut in [0_u32, 1, 2, 3, 7, 50] {
            let truncated = state::truncate_to_tokens(text, cut);
            let truncated_tokens = state::count_text_tokens(&truncated);
            assert!(
                truncated_tokens <= cut.min(total),
                "case {text:?} cut to {cut}: expected at most {} tokens, got {truncated_tokens} \
                 for {truncated:?}",
                cut.min(total)
            );
            if cut >= total {
                assert_eq!(
                    truncated, text,
                    "case {text:?} cut to {cut}: expected the whole text back, got {truncated:?}"
                );
            }
            assert!(
                text.starts_with(truncated.trim_end()),
                "case {text:?} cut to {cut}: expected a prefix of the text, got {truncated:?}"
            );
        }
        let longer = format!("{text} plus more words here");
        assert!(
            state::count_text_tokens(&longer) >= total,
            "case {text:?}: expected appending text not to lower the count, got {} then {}",
            total,
            state::count_text_tokens(&longer)
        );
    }
}

/// RULE: the async-invoke store is bounded: past its retention limit the OLDEST
/// records are evicted, together with the idempotency-index entries that point
/// at them, so a caller cannot grow the store without limit.
#[tokio::test]
async fn the_async_invoke_store_evicts_its_oldest_records_past_the_retention_bound() {
    let mut store = ServiceState::default();
    let config = AsyncInvokeOutputDataConfig::s3OutputDataConfig(AsyncInvokeS3OutputDataConfig {
        s3_uri: "s3://bucket/out".to_string(),
        kms_key_id: None,
        bucket_owner: None,
    });
    let overflow = 5;
    let total = state::MAX_ASYNC_INVOKE_RECORDS + overflow;
    for index in 0..total {
        let invocation_arn =
            format!("arn:aws:bedrock:us-east-1:000000000000:async-invoke/{index:012}");
        store.insert_async_invoke(state::AsyncInvokeRecord {
            invocation_arn: invocation_arn.clone(),
            model_id: "amazon.nova-lite-v1:0".to_string(),
            model_arn: "arn:aws:bedrock:us-east-1:000000000000:foundation-model/amazon.nova-lite-v1:0"
                .to_string(),
            client_request_token: Some(format!("tok-{index}")),
            status: state::AsyncInvokeStatus::InProgress,
            submit_time: 1_000 + index as i64,
            last_modified_time: None,
            end_time: None,
            failure_message: None,
            output_data_config: config.clone(),
            model_input: aws_smithy_types::Document::Null,
            tags: Vec::new(),
            settles_at: 1_000 + index as i64 + state::ASYNC_INVOKE_SETTLE_MILLIS,
            request_fingerprint: format!("fingerprint-{index}"),
        });
        let kept = store.async_invokes.len();
        assert!(
            kept <= state::MAX_ASYNC_INVOKE_RECORDS,
            "case after {} insert(s): expected at most {} records, got {kept}",
            index + 1,
            state::MAX_ASYNC_INVOKE_RECORDS
        );
    }
    assert_eq!(
        store.async_invokes.len(),
        state::MAX_ASYNC_INVOKE_RECORDS,
        "case \"store filled past the bound\": expected the bound to hold, got {}",
        store.async_invokes.len()
    );
    assert_eq!(
        store.idempotency_index.len(),
        state::MAX_ASYNC_INVOKE_RECORDS,
        "case \"store filled past the bound\": expected the index to shrink with the store, got {}",
        store.idempotency_index.len()
    );
    for index in 0..overflow {
        let evicted = format!("arn:aws:bedrock:us-east-1:000000000000:async-invoke/{index:012}");
        assert!(
            !store.async_invokes.contains_key(&evicted),
            "case \"oldest record {index}\": expected it to be evicted, it is still stored"
        );
        assert!(
            !store.idempotency_index.contains_key(&format!("tok-{index}")),
            "case \"oldest record {index}\": expected its idempotency entry to be evicted too"
        );
    }
    let newest = format!(
        "arn:aws:bedrock:us-east-1:000000000000:async-invoke/{:012}",
        total - 1
    );
    assert!(
        store.async_invokes.contains_key(&newest),
        "case \"newest record\": expected eviction to keep it, got {:?}",
        store.async_invokes.keys().next_back()
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The `invocationArn` values a `ListAsyncInvokes` query reports, in order.
async fn listed_arns(router: &MultiServiceRouter, query: &str) -> Vec<String> {
    let call = get(router, &format!("/async-invoke{query}")).await;
    assert_eq!(
        call.status,
        200,
        "case {query:?}: expected ListAsyncInvokes to answer 200, got {} / {}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );
    call.json()["asyncInvokeSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            summary["invocationArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Whether `value` matches the modelled `InvocationArn` pattern
/// `^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:async-invoke/[a-z0-9]{12}$`,
/// checked field by field rather than with a regex dependency.
fn is_invocation_arn(value: &str) -> bool {
    let fields: Vec<&str> = value.split(':').collect();
    let [arn, partition, service, region, account, resource] = fields.as_slice() else {
        return false;
    };
    let Some(id) = resource.strip_prefix("async-invoke/") else {
        return false;
    };
    *arn == "arn"
        && (*partition == "aws" || partition.starts_with("aws-"))
        && *service == "bedrock"
        && !region.is_empty()
        && region.len() <= 20
        && region
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && account.len() == 12
        && account.chars().all(|c| c.is_ascii_digit())
        && id.len() == 12
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Percent-encode the characters an ARN carries that cannot sit in a path
/// segment unescaped. Only `:` and `/` occur in the ARNs these tests build; a
/// future ARN shape with other reserved characters would need more.
fn encode_path(value: &str) -> String {
    value.replace(':', "%3A").replace('/', "%2F")
}

/// A modelled timestamp as epoch seconds, however the protocol rendered it: the
/// restJson1 default (epoch seconds) or an RFC3339 string. `None` when the
/// member is absent.
fn instant_of(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(text) => aws_smithy_types::DateTime::from_str(
            text,
            aws_smithy_types::date_time::Format::DateTime,
        )
        .ok()
        .map(|parsed| parsed.as_secs_f64()),
        _ => None,
    }
}

/// Now, in epoch seconds.
fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// The instant halfway between two instants.
fn midpoint(left: f64, right: f64) -> f64 {
    (left + right) / 2.0
}

/// An instant as the `@httpQuery` timestamp value a date-time member takes,
/// percent-encoded for a query string.
fn rfc3339_query(seconds: f64) -> String {
    aws_smithy_types::DateTime::from_secs_f64(seconds)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("a formattable timestamp")
        .replace(':', "%3A")
}

/// Base64 of `bytes`, the JSON encoding of a Smithy blob member. The codec is
/// the one the wire protocol itself uses, from `aws-smithy-types`.
fn base64(bytes: &[u8]) -> String {
    aws_smithy_types::base64::encode(bytes)
}
