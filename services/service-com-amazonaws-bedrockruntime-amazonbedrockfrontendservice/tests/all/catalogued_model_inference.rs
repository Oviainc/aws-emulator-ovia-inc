// SPDX-License-Identifier: CC0-1.0

//! Three rules about what the inference surface serves, end to end over HTTP.
//!
//! 1. A `modelId` naming a base model the Amazon Bedrock catalogue publishes for on-demand
//!    inference is SERVED — whichever provider publishes it — and only an identifier naming
//!    no published model is answered as one the service cannot interpret. Identifier
//!    resolution is not where the emulator's envelope coverage is decided
//!    (`crate::state::model_catalog`).
//! 2. Every response names the request it answered, in the `x-amzn-requestid` header this
//!    protocol reports that id in — on a success exactly as on a failure — which is where
//!    an SDK's response metadata reads it from.
//! 3. A `Converse` request that OFFERS a tool starts the tool-use round trip: the first
//!    turn stops on `tool_use` with a `toolUse` block, and the turn handed the
//!    `toolResult` back answers in text that states the result the tool returned.
//!
//! Each rule is asserted over a SET of inputs: the tables differ along what the rule
//! generalises over (the provider and the id form, the operation and the outcome, the shape
//! of the tool's result), so no row is the specification on its own.

use serde_json::{json, Value};

use crate::bedrock_runtime_inference::{emulator, post_json, send, Call};

/// The header AWS reports a response's own request id in. HTTP header names are
/// case-insensitive and the wire carries this one lower-cased.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// One catalogued model and the body its provider's `InvokeModel` envelope takes.
struct CataloguedModel {
    case: &'static str,
    model_id: &'static str,
    /// A body that envelope accepts.
    body: &'static str,
    /// A JSON pointer at the generated text in that envelope's answer.
    text_pointer: &'static str,
}

/// Models from the published catalogue, one per PROVIDER this service's inventory carries a
/// response envelope for, none of them the one the issue cited.
///
/// Source: the model ids this emulator's Amazon Bedrock control plane publishes through
/// `ListFoundationModels` (its `state::foundation_models` catalogue), which is the set of
/// models an account can invoke here; the request and response envelopes are each
/// provider's own, as `crate::state::model_catalog` records them.
fn catalogued_models() -> Vec<CataloguedModel> {
    vec![
        CataloguedModel {
            case: "a provider served on the OpenAI-compatible schema (OpenAI)",
            model_id: "openai.gpt-oss-120b-1:0",
            body: r#"{"messages":[{"role":"user","content":"hi"}],"max_tokens":64}"#,
            text_pointer: "/choices/0/message/content",
        },
        CataloguedModel {
            case: "the same schema, another provider (Qwen)",
            model_id: "qwen.qwen3-32b-v1:0",
            body: r#"{"messages":[{"role":"user","content":"hi"}]}"#,
            text_pointer: "/choices/0/message/content",
        },
        CataloguedModel {
            case: "the same schema, in its completion form (DeepSeek)",
            model_id: "deepseek.r1-v1:0",
            body: r#"{"prompt":"hi","max_tokens":64}"#,
            text_pointer: "/choices/0/message/content",
        },
        CataloguedModel {
            case: "the same schema, another provider again (Z AI)",
            model_id: "zai.glm-5",
            body: r#"{"messages":[{"role":"user","content":"hi"}]}"#,
            text_pointer: "/choices/0/message/content",
        },
        CataloguedModel {
            case: "an AI21 Jamba model",
            model_id: "ai21.jamba-1-5-mini-v1:0",
            body: r#"{"messages":[{"role":"user","content":"hi"}]}"#,
            text_pointer: "/choices/0/message/content",
        },
        CataloguedModel {
            case: "a Claude line later than the dated Claude 3 ids",
            model_id: "anthropic.claude-sonnet-4-5-20250929-v1:0",
            body: r#"{"anthropic_version":"bedrock-2023-05-31","max_tokens":64,"messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}]}"#,
            text_pointer: "/content/0/text",
        },
        CataloguedModel {
            case: "a Llama 4 model",
            model_id: "meta.llama4-scout-17b-instruct-v1:0",
            body: r#"{"prompt":"hi"}"#,
            text_pointer: "/generation",
        },
        CataloguedModel {
            case: "a Mistral model published after the 2402 line",
            model_id: "mistral.ministral-3-8b-instruct",
            body: r#"{"prompt":"hi"}"#,
            text_pointer: "/outputs/0/text",
        },
        CataloguedModel {
            case: "an Amazon Nova 2 model",
            model_id: "amazon.nova-2-lite-v1:0",
            body: r#"{"messages":[{"role":"user","content":[{"text":"hi"}]}]}"#,
            text_pointer: "/output/message/content/0/text",
        },
        CataloguedModel {
            case: "a version-qualified Titan Text id the catalogue also publishes",
            model_id: "amazon.titan-text-lite-v1:0:4k",
            body: r#"{"inputText":"hi"}"#,
            text_pointer: "/results/0/outputText",
        },
    ]
}

/// RULE: a `modelId` naming a base model the catalogue publishes for on-demand inference is
/// served — by `InvokeModel` in that provider's own response envelope and by the Converse
/// API, whose request and response shapes are the service's own — and the identifier is
/// never refused as one the service cannot interpret just because of which provider
/// publishes the model.
#[tokio::test]
async fn every_catalogued_text_model_is_served_by_invoke_and_converse() {
    let router = emulator();

    for row in catalogued_models() {
        let case = row.case;
        let encoded = row.model_id.replace(':', "%3A");
        let invoked = send(
            &router,
            "POST",
            &format!("/model/{encoded}/invoke"),
            &[("content-type", "application/json")],
            row.body.as_bytes().to_vec(),
        )
        .await;
        assert_eq!(
            invoked.status,
            200,
            "case {case:?} ({}): expected the catalogued model to be invocable, got {} / {}",
            row.model_id,
            invoked.status,
            String::from_utf8_lossy(&invoked.body)
        );
        let body = invoked.json();
        assert!(
            body.pointer(row.text_pointer)
                .and_then(Value::as_str)
                .is_some_and(|text| !text.is_empty()),
            "case {case:?}: expected generated text at {} in {body}",
            row.text_pointer
        );

        let conversed = post_json(
            &router,
            &format!("/model/{encoded}/converse"),
            &json!({"messages": [{"role": "user", "content": [{"text": "hi"}]}]}),
        )
        .await;
        assert_eq!(
            conversed.status,
            200,
            "case {case:?} ({}): expected the Converse API to serve it, got {} / {}",
            row.model_id,
            conversed.status,
            String::from_utf8_lossy(&conversed.body)
        );
        assert_eq!(
            conversed.json()["stopReason"].as_str(),
            Some("end_turn"),
            "case {case:?}: expected a completed turn, got {}",
            conversed.json()
        );
    }
}

/// RULE (the other side of the same one): an identifier naming NO published model is still
/// answered as one the service cannot interpret — a 400 `ValidationException` — so widening
/// the inventory to the catalogue did not make every string a model.
#[tokio::test]
async fn an_identifier_naming_no_catalogued_model_is_still_refused() {
    let router = emulator();
    // (case, modelId) — a provider nothing publishes, a published provider at a version the
    // catalogue does not carry, and a published id with a typo in it.
    let rows = [
        ("a provider the catalogue does not publish", "nobody.such-model-v1"),
        ("a published provider at an unpublished version", "qwen.qwen9-1b-v9:0"),
        ("a published id with a typo", "openai.gpt-oss-121b-1:0"),
    ];

    for (case, model_id) in rows {
        let call = post_json(
            &router,
            &format!("/model/{}/converse", model_id.replace(':', "%3A")),
            &json!({"messages": [{"role": "user", "content": [{"text": "hi"}]}]}),
        )
        .await;
        assert_eq!(
            call.status,
            400,
            "case {case:?} ({model_id}): expected a 400, got {} / {}",
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
}

/// Whether `id` has the 8-4-4-4-12 lower-case-hex form every AWS request id carries.
fn is_request_id(id: &str) -> bool {
    let mut groups = id.split('-');
    for width in [8usize, 4, 4, 4, 12] {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != width
            || !group
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return false;
        }
    }
    groups.next().is_none()
}

/// RULE: every response names the request it answered in `x-amzn-requestid`, with a FRESH
/// id per response — a success as much as a refusal, and whichever layer rendered it.
///
/// The rows span the operations and the layers: a served `Converse`, a served
/// `InvokeModel`, a refusal a handler returned, a refusal the framework's constraint walk
/// raised before any handler ran, and a request that names no modelled operation at all.
#[tokio::test]
async fn every_response_names_the_request_it_answered() {
    let router = emulator();
    let prompt = json!({"messages": [{"role": "user", "content": [{"text": "hi"}]}]});
    // (case, expected status, the call)
    let calls: Vec<(&str, u16, Call)> = vec![
        (
            "a served Converse",
            200,
            post_json(&router, "/model/amazon.nova-lite-v1%3A0/converse", &prompt).await,
        ),
        (
            "a served InvokeModel",
            200,
            send(
                &router,
                "POST",
                "/model/amazon.nova-lite-v1%3A0/invoke",
                &[("content-type", "application/json")],
                br#"{"messages":[{"role":"user","content":[{"text":"hi"}]}]}"#.to_vec(),
            )
            .await,
        ),
        (
            "a refusal the handler returned",
            400,
            post_json(&router, "/model/nobody.no-such-model/converse", &prompt).await,
        ),
        (
            "a refusal raised before the handler ran",
            400,
            post_json(
                &router,
                "/model/amazon.nova-lite-v1%3A0/converse",
                &json!({"messages": []}),
            )
            .await,
        ),
        (
            "a request naming no modelled operation",
            501,
            post_json(&router, "/model/amazon.nova-lite-v1%3A0/nothing-here", &prompt).await,
        ),
    ];

    let mut reported: Vec<String> = Vec::new();
    for (case, expected, call) in calls {
        assert_eq!(
            call.status, expected,
            "case {case:?}: expected {expected}, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        let id = call.header(REQUEST_ID_HEADER);
        assert!(
            is_request_id(&id),
            "case {case:?}: expected the response to name the request it answered in \
             {REQUEST_ID_HEADER}, got {id:?}"
        );
        assert!(
            !reported.contains(&id),
            "case {case:?}: every response names its OWN request, but {id:?} was reported \
             before (already seen: {reported:?})"
        );
        reported.push(id);
    }
}

/// One tool result a client hands back, and the text the final answer has to state.
struct ResultRow {
    case: &'static str,
    /// The `toolResult` content blocks the follow-up turn carries.
    content: Value,
    /// What the final answer must state about the result.
    stated: &'static str,
}

/// The result shapes a client answers a tool request with: the JSON document a tool
/// returns, a plain text result, and several blocks in one result.
fn result_rows() -> Vec<ResultRow> {
    vec![
        ResultRow {
            case: "a JSON document",
            content: json!([{"json": {"price": 231.4}}]),
            stated: r#"{"price":231.4}"#,
        },
        ResultRow {
            case: "a text result",
            content: json!([{"text": "231.4 USD"}]),
            stated: "231.4 USD",
        },
        ResultRow {
            case: "two blocks in one result",
            content: json!([{"text": "AMZN"}, {"json": {"price": 231.4}}]),
            stated: r#"AMZN {"price":231.4}"#,
        },
    ]
}

/// The `toolConfig` offering one tool, with no `toolChoice`: the decision is the model's.
fn offered_tool() -> Value {
    json!({"tools": [{"toolSpec": {
        "name": "get_stock_price",
        "description": "Look up a stock price.",
        "inputSchema": {"json": {
            "type": "object",
            "properties": {"symbol": {"type": "string"}},
            "required": ["symbol"],
        }},
    }}]})
}

/// RULE: a `Converse` request that offers a tool starts the round trip — the first turn
/// stops on `tool_use` with a `toolUse` block naming the offered tool — and the turn handed
/// the `toolResult` for that request answers in text that STATES the result the tool
/// returned, which is what makes the exchange usable by an agent client.
#[tokio::test]
async fn a_tool_use_round_trip_ends_in_an_answer_that_states_the_tool_result() {
    let router = emulator();
    let model_id = "amazon.nova-pro-v1%3A0";
    let user_turn = json!({"role": "user", "content": [{"text": "What is AMZN trading at?"}]});

    for row in result_rows() {
        let case = row.case;
        let first = post_json(
            &router,
            &format!("/model/{model_id}/converse"),
            &json!({"messages": [user_turn], "toolConfig": offered_tool()}),
        )
        .await;
        assert_eq!(
            first.status,
            200,
            "case {case:?}: expected the first turn to be served, got {} / {}",
            first.status,
            String::from_utf8_lossy(&first.body)
        );
        let first_body = first.json();
        assert_eq!(
            first_body["stopReason"].as_str(),
            Some("tool_use"),
            "case {case:?}: the model must stop to call the tool, got {}",
            first_body
        );
        let assistant = first_body["output"]["message"].clone();
        let tool_use = assistant["content"][0]["toolUse"].clone();
        assert_eq!(
            tool_use["name"].as_str(),
            Some("get_stock_price"),
            "case {case:?}: expected the offered tool to be requested, got {assistant}"
        );
        let tool_use_id = tool_use["toolUseId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no toolUseId in {assistant}"))
            .to_string();

        let second = post_json(
            &router,
            &format!("/model/{model_id}/converse"),
            &json!({
                "messages": [
                    user_turn,
                    assistant,
                    {"role": "user", "content": [{"toolResult": {
                        "toolUseId": tool_use_id,
                        "content": row.content,
                        "status": "success",
                    }}]},
                ],
                "toolConfig": offered_tool(),
            }),
        )
        .await;
        assert_eq!(
            second.status,
            200,
            "case {case:?}: expected the turn carrying the tool result to be served, got {} / {}",
            second.status,
            String::from_utf8_lossy(&second.body)
        );
        let second_body = second.json();
        assert_eq!(
            second_body["stopReason"].as_str(),
            Some("end_turn"),
            "case {case:?}: the turn handed the tool's result answers in text, got {}",
            second_body
        );
        let answer = second_body["output"]["message"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            answer.contains("get_stock_price") && answer.contains(row.stated),
            "case {case:?}: expected the final answer to state what the tool returned \
             ({:?}), got {answer:?}",
            row.stated
        );
    }
}

/// RULE (the bound on the same stage): what a turn states about the tool results it was
/// handed is bounded by the EMULATOR's own limits, so neither the number of `toolResult`
/// blocks a request carries nor the size of one decides how much text the answer carries.
#[tokio::test]
async fn the_answer_states_tool_results_under_the_emulators_own_bound() {
    let router = emulator();
    let model_id = "amazon.nova-pro-v1%3A0";
    let user_turn = json!({"role": "user", "content": [{"text": "What is AMZN trading at?"}]});

    // A conversation whose assistant turn requested the tool many times over and whose
    // latest turn answers every one of those requests with a large result.
    let requests: Vec<Value> = (0..64)
        .map(|index| {
            json!({"toolUse": {
                "toolUseId": format!("tooluse_pinned{index:04}"),
                "name": "get_stock_price",
                "input": {"symbol": "AMZN"},
            }})
        })
        .collect();
    let results: Vec<Value> = (0..64)
        .map(|index| {
            json!({"toolResult": {
                "toolUseId": format!("tooluse_pinned{index:04}"),
                "content": [{"text": "x".repeat(4096)}],
                "status": "success",
            }})
        })
        .collect();

    let call = post_json(
        &router,
        &format!("/model/{model_id}/converse"),
        &json!({
            "messages": [
                user_turn,
                {"role": "assistant", "content": requests},
                {"role": "user", "content": results},
            ],
            "toolConfig": offered_tool(),
        }),
    )
    .await;
    assert_eq!(
        call.status,
        200,
        "expected the turn to be served, got {} / {}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );
    let answer = call.json()["output"]["message"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        answer.contains("get_stock_price"),
        "expected the answer to state the tool it was answered for, got {answer:?}"
    );
    // The request carries 64 results of 4096 characters each; the answer states a bounded
    // number of them, each quoted up to a bounded length, so it cannot grow with the
    // request.
    let stated = answer.matches("The tool get_stock_price returned").count();
    assert!(
        stated <= 8,
        "expected at most the bounded number of result clauses, got {stated} in {answer:?}"
    );
    assert!(
        answer.chars().count() < 8_192,
        "expected the answer to stay bounded, got {} characters",
        answer.chars().count()
    );
}
