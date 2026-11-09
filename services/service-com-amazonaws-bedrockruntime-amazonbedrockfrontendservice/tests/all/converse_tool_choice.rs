// SPDX-License-Identifier: CC0-1.0

//! The tool-calling half of the Converse envelope, end to end over HTTP: which tool
//! a turn requests for a request that offers one, and how a later turn's `toolResult`
//! is addressed to the tool request it answers.
//!
//! Every case drives the real router through the harness the other suites of this
//! target use, so what is asserted is the wire behaviour a client sees.
//!
//! Each rule is asserted over a SET of inputs: the tables differ along what the
//! rule generalises over (the form of the choice, how many tools the request
//! offers, which of them is named, the kind of tool entry, the name's shape, the
//! way a supplied id differs from an issued one), so no row is the specification
//! on its own.

use serde_json::{json, Value};

use crate::bedrock_runtime_inference::{emulator, post_json, Call};
use crate::bedrock_runtime_streaming::decode_frames;

/// The modelled `ToolUseId` constraints: `@length` 1..64 and `@pattern`
/// `^[a-zA-Z0-9_.:-]+$` (smithy_query(query="shape", shape="ToolUseBlock")).
const TOOL_USE_ID_MAX: usize = 64;

/// One `toolConfig`, and the tool the turn it is sent with must request.
struct ChoiceRow {
    case: &'static str,
    /// A model the registry serves and the Converse API supports.
    model_id: &'static str,
    tool_config: Value,
    /// The tool the turn must request, or `None` where the request offers no tool
    /// the turn could ask for.
    requested: Option<&'static str>,
}

/// One tool definition, as `ToolSpecification` models it.
fn tool_spec(name: &str) -> Value {
    json!({"toolSpec": {
        "name": name,
        "description": format!("Look up {name}."),
        "inputSchema": {"json": {
            "type": "object",
            "properties": {"symbol": {"type": "string"}},
            "required": ["symbol"],
        }},
    }})
}

/// The choice forms a request can carry, against tool lists of different sizes,
/// entry kinds and name shapes.
///
/// Member names and the union's three members from `smithy_query(query="shape",
/// shape="ToolChoice,ToolConfiguration,Tool,SystemTool")` on this service's
/// model; the names exercise the whole `ToolName` pattern
/// (`^[a-zA-Z0-9_-]+$`).
fn choice_rows() -> Vec<ChoiceRow> {
    vec![
        ChoiceRow {
            case: "any over a single tool",
            model_id: "amazon.nova-lite-v1:0",
            tool_config: json!({
                "tools": [tool_spec("get_stock_price")],
                "toolChoice": {"any": {}},
            }),
            requested: Some("get_stock_price"),
        },
        ChoiceRow {
            case: "any over three tools requests the first one offered",
            model_id: "anthropic.claude-3-haiku-20240307-v1:0",
            tool_config: json!({
                "tools": [
                    tool_spec("get_stock_price"),
                    tool_spec("get_weather"),
                    tool_spec("get_time"),
                ],
                "toolChoice": {"any": {}},
            }),
            requested: Some("get_stock_price"),
        },
        ChoiceRow {
            case: "tool naming the only tool offered",
            model_id: "anthropic.claude-3-sonnet-20240229-v1:0",
            tool_config: json!({
                "tools": [tool_spec("get_weather")],
                "toolChoice": {"tool": {"name": "get_weather"}},
            }),
            requested: Some("get_weather"),
        },
        ChoiceRow {
            case: "tool naming the last of three",
            model_id: "amazon.nova-pro-v1:0",
            tool_config: json!({
                "tools": [
                    tool_spec("get_stock_price"),
                    tool_spec("get_weather"),
                    tool_spec("get_time"),
                ],
                "toolChoice": {"tool": {"name": "get_time"}},
            }),
            requested: Some("get_time"),
        },
        ChoiceRow {
            case: "any over a list whose first entry is a cache-point marker",
            model_id: "amazon.nova-lite-v1:0",
            tool_config: json!({
                "tools": [
                    {"cachePoint": {"type": "default"}},
                    tool_spec("get_weather"),
                ],
                "toolChoice": {"any": {}},
            }),
            requested: Some("get_weather"),
        },
        ChoiceRow {
            case: "tool naming a hyphenated and digit-bearing tool name",
            model_id: "mistral.mistral-7b-instruct-v0:2",
            tool_config: json!({
                "tools": [tool_spec("get-stock-price-v2"), tool_spec("other_tool")],
                "toolChoice": {"tool": {"name": "get-stock-price-v2"}},
            }),
            requested: Some("get-stock-price-v2"),
        },
        ChoiceRow {
            case: "any over a system-defined tool",
            model_id: "amazon.nova-lite-v1:0",
            tool_config: json!({
                "tools": [{"systemTool": {"name": "web_search"}}],
                "toolChoice": {"any": {}},
            }),
            requested: Some("web_search"),
        },
        ChoiceRow {
            case: "auto over a single tool requests it",
            model_id: "amazon.nova-lite-v1:0",
            tool_config: json!({
                "tools": [tool_spec("get_stock_price")],
                "toolChoice": {"auto": {}},
            }),
            requested: Some("get_stock_price"),
        },
        ChoiceRow {
            case: "a tool config with no choice at all requests the first tool offered",
            model_id: "meta.llama3-8b-instruct-v1:0",
            tool_config: json!({"tools": [tool_spec("get_stock_price"), tool_spec("get_time")]}),
            requested: Some("get_stock_price"),
        },
        ChoiceRow {
            case: "auto over a list that offers no requestable tool",
            model_id: "amazon.nova-lite-v1:0",
            tool_config: json!({
                "tools": [{"cachePoint": {"type": "default"}}],
                "toolChoice": {"auto": {}},
            }),
            requested: None,
        },
        ChoiceRow {
            case: "no choice at all over a list that offers no requestable tool",
            model_id: "meta.llama3-8b-instruct-v1:0",
            tool_config: json!({"tools": [{"cachePoint": {"type": "default"}}]}),
            requested: None,
        },
    ]
}

/// The user turn every row below is sent with.
fn user_turn() -> Value {
    json!([{"role": "user", "content": [{"text": "What is the price of AMZN today?"}]}])
}

/// The one content block a message carries, panicking with the case when the
/// message does not carry exactly one.
fn only_block(case: &str, message: &Value) -> Value {
    let content = message["content"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the message carries no content: {message}"));
    assert_eq!(
        content.len(),
        1,
        "case {case:?}: expected exactly one content block, got {content:?}"
    );
    content[0].clone()
}

/// Assert `id` satisfies the modelled `ToolUseId` constraints.
fn assert_tool_use_id(case: &str, id: &str) {
    assert!(
        !id.is_empty() && id.len() <= TOOL_USE_ID_MAX,
        "case {case:?}: expected a toolUseId of 1..{TOOL_USE_ID_MAX} characters, got {id:?}"
    );
    assert!(
        id.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-')),
        "case {case:?}: expected a toolUseId matching the modelled pattern, got {id:?}"
    );
}

/// RULE: a turn whose request OFFERS a tool it can ask for ends by requesting one:
/// the response's `stopReason` is `tool_use` and the assistant message is exactly one
/// `toolUse` block carrying a server-minted `toolUseId`, the name of the tool the
/// choice settles on and an `input` member — no text. A mandatory `toolChoice` (`any`,
/// or `tool` naming one of the tools the request offers) settles which tool that is;
/// `auto`, and a request stating no choice at all, leave it to the model, which asks
/// for the first tool offered. A request whose tool list offers no requestable tool
/// has nothing to ask for, so its turn generates text.
#[tokio::test]
async fn a_turn_requests_a_tool_the_request_offers_and_generates_text_when_it_offers_none() {
    let router = emulator();
    let mut minted: Vec<String> = Vec::new();

    for row in choice_rows() {
        let case = row.case;
        let request = json!({
            "messages": user_turn(),
            "toolConfig": row.tool_config,
        });
        let call = post_json(
            &router,
            &format!("/model/{}/converse", row.model_id),
            &request,
        )
        .await;
        assert_eq!(
            call.status,
            200,
            "case {case:?}: expected Converse to answer 200, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        let body = call.json();
        let message = &body["output"]["message"];
        let block = only_block(case, message);
        let stop = body["stopReason"].as_str().unwrap_or_default();

        match row.requested {
            Some(expected) => {
                assert_eq!(
                    stop, "tool_use",
                    "case {case:?}: a turn offered a tool it can request must stop to call \
                     it, got {stop:?} with content {block}"
                );
                let tool_use = &block["toolUse"];
                assert!(
                    tool_use.is_object(),
                    "case {case:?}: expected one toolUse block, got {block}"
                );
                assert!(
                    block.get("text").is_none(),
                    "case {case:?}: a turn that requests a tool generates no text, got {block}"
                );
                assert_eq!(
                    tool_use["name"].as_str(),
                    Some(expected),
                    "case {case:?}: expected the toolUse block to request {expected:?}, got {}",
                    tool_use["name"]
                );
                assert!(
                    tool_use["input"].is_object(),
                    "case {case:?}: expected the modelled input member, got {tool_use}"
                );
                let id = tool_use["toolUseId"].as_str().unwrap_or_else(|| {
                    panic!("case {case:?}: the toolUse block carries no toolUseId: {tool_use}")
                });
                assert_tool_use_id(case, id);
                assert!(
                    !minted.iter().any(|seen| seen == id),
                    "case {case:?}: every tool request is issued under its own id, but \
                     {id:?} was issued before (already seen: {minted:?})"
                );
                minted.push(id.to_string());
            }
            None => {
                assert_eq!(
                    stop, "end_turn",
                    "case {case:?}: a request offering no requestable tool leaves the turn \
                     generating text, got {stop:?} with content {block}"
                );
                assert!(
                    block["text"].is_string(),
                    "case {case:?}: expected a text block, got {block}"
                );
                assert!(
                    block.get("toolUse").is_none(),
                    "case {case:?}: no tool was requested, got {block}"
                );
            }
        }
    }

    assert!(
        minted.len() >= 9,
        "the table must cover every choice form a turn can request a tool under, got {} \
         rows: {minted:?}",
        minted.len()
    );
}

/// RULE: a `toolChoice` the request's own tools cannot satisfy — `tool` naming a
/// tool the request does not offer, or a mandatory choice over a tool list that
/// offers no requestable tool — is refused with `ValidationException` (400).
#[tokio::test]
async fn a_tool_choice_the_request_cannot_satisfy_is_refused() {
    let router = emulator();
    let model_id = "amazon.nova-lite-v1:0";
    let rows: Vec<(&str, Value)> = vec![
        (
            "tool naming a tool the request does not offer",
            json!({
                "tools": [tool_spec("get_weather")],
                "toolChoice": {"tool": {"name": "get_stock_price"}},
            }),
        ),
        (
            "tool naming a tool that differs only in case",
            json!({
                "tools": [tool_spec("get_weather")],
                "toolChoice": {"tool": {"name": "Get_Weather"}},
            }),
        ),
        (
            "any over a list whose only entry is a cache-point marker",
            json!({
                "tools": [{"cachePoint": {"type": "default"}}],
                "toolChoice": {"any": {}},
            }),
        ),
    ];

    for (case, tool_config) in rows {
        let call = post_json(
            &router,
            &format!("/model/{model_id}/converse"),
            &json!({"messages": user_turn(), "toolConfig": tool_config}),
        )
        .await;
        assert_eq!(
            call.status,
            400,
            "case {case:?}: expected a 400 refusal, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert!(
            call.error_code().contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            call.error_code()
        );
    }
}

/// How a supplied `toolUseId` can differ from the one that was issued.
struct AddressRow {
    case: &'static str,
    /// The id to send, derived from the issued one.
    supplied: fn(&str) -> String,
}

/// Mint a tool request on `model_id` and answer the turn, returning the whole
/// exchange: the assistant message and the id it was issued under.
async fn mint_tool_request(
    router: &smithy_server_runtime::MultiServiceRouter,
    model_id: &str,
    tool_config: &Value,
) -> (Value, String) {
    let call = post_json(
        router,
        &format!("/model/{model_id}/converse"),
        &json!({"messages": user_turn(), "toolConfig": tool_config}),
    )
    .await;
    assert_eq!(
        call.status,
        200,
        "the forced turn must be served, got {} / {}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );
    let body = call.json();
    let message = body["output"]["message"].clone();
    let id = message["content"][0]["toolUse"]["toolUseId"]
        .as_str()
        .unwrap_or_else(|| panic!("the forced turn minted no toolUseId: {message}"))
        .to_string();
    (message, id)
}

/// The follow-up conversation: the user turn, the assistant's tool request, and
/// a user turn answering it with `supplied` as the addressed id.
fn follow_up(assistant: &Value, supplied: &str) -> Value {
    json!([
        user_turn()[0],
        assistant,
        {"role": "user", "content": [{"toolResult": {
            "toolUseId": supplied,
            "content": [{"json": {"price": 231.4}}],
            "status": "success",
        }}]},
    ])
}

/// RULE: a `toolResult` answers a tool request the conversation carries and is
/// addressed by the `toolUseId` that request was issued under, so a conversation
/// whose `toolResult` names an id no `toolUse` block in it was issued with is
/// refused with `ValidationException` (400), and the refusal names both the id
/// the result could have answered and the id it supplied.
#[tokio::test]
async fn a_tool_result_must_address_a_tool_request_the_conversation_carries() {
    let router = emulator();
    let model_id = "amazon.nova-lite-v1:0";
    let tool_config = json!({
        "tools": [tool_spec("get_stock_price")],
        "toolChoice": {"any": {}},
    });
    let (assistant, issued) = mint_tool_request(&router, model_id, &tool_config).await;
    // The same tools, with the choice dropped: the follow-up turn is the one
    // that reports the tool's result.
    let follow_up_tools = json!({"tools": [tool_spec("get_stock_price")]});

    // The issued id addresses the request, so the follow-up turn is served.
    let served = post_json(
        &router,
        &format!("/model/{model_id}/converse"),
        &json!({
            "messages": follow_up(&assistant, &issued),
            "toolConfig": follow_up_tools,
        }),
    )
    .await;
    assert_eq!(
        served.status,
        200,
        "case \"the issued id\": expected 200, got {} / {}",
        served.status,
        String::from_utf8_lossy(&served.body)
    );

    let rows = [
        AddressRow {
            case: "the issued id with its last character changed",
            supplied: |issued| {
                let mut id = issued[..issued.len() - 1].to_string();
                let last = issued.chars().next_back().unwrap_or('0');
                id.push(if last == '0' { '1' } else { '0' });
                id
            },
        },
        AddressRow {
            case: "the issued id with its first character changed",
            supplied: |issued| {
                let mut id = String::from("X");
                id.push_str(&issued[1..]);
                id
            },
        },
        AddressRow {
            case: "the issued id truncated",
            supplied: |issued| issued[..issued.len() - 4].to_string(),
        },
        AddressRow {
            case: "the issued id with a suffix appended",
            supplied: |issued| format!("{issued}-2"),
        },
        AddressRow {
            case: "an id of the shape a client invents",
            supplied: |_| "tooluse_client_invented_1".to_string(),
        },
        AddressRow {
            case: "a one-character id",
            supplied: |_| "a".to_string(),
        },
    ];

    for row in rows {
        let case = row.case;
        let supplied = (row.supplied)(&issued);
        let call = post_json(
            &router,
            &format!("/model/{model_id}/converse"),
            &json!({
                "messages": follow_up(&assistant, &supplied),
                "toolConfig": follow_up_tools,
            }),
        )
        .await;
        assert_eq!(
            call.status,
            400,
            "case {case:?}: a toolResult addressed to {supplied:?}, which was never issued, \
             must be refused, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert!(
            call.error_code().contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            call.error_code()
        );
        let message = call.json()["message"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains(&supplied) && message.contains(&issued),
            "case {case:?}: the refusal must name the id supplied ({supplied}) and the one \
             issued ({issued}), got {message:?}"
        );
    }

    // An id minted in ANOTHER conversation was never issued in THIS history.
    let (_, elsewhere) = mint_tool_request(&router, model_id, &tool_config).await;
    let call = post_json(
        &router,
        &format!("/model/{model_id}/converse"),
        &json!({
            "messages": follow_up(&assistant, &elsewhere),
            "toolConfig": follow_up_tools,
        }),
    )
    .await;
    assert_eq!(
        call.status,
        400,
        "case \"an id issued in another conversation\": expected a 400 refusal, got {} / {}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );

    // A conversation whose first turn already answers a tool request carries no
    // tool request at all.
    let unaddressed = post_json(
        &router,
        &format!("/model/{model_id}/converse"),
        &json!({
            "messages": [{"role": "user", "content": [{"toolResult": {
                "toolUseId": issued,
                "content": [{"text": "231.4"}],
            }}]}],
            "toolConfig": follow_up_tools,
        }),
    )
    .await;
    assert_eq!(
        unaddressed.status,
        400,
        "case \"a toolResult with no tool request in the conversation\": expected a 400 \
         refusal, got {} / {}",
        unaddressed.status,
        String::from_utf8_lossy(&unaddressed.body)
    );
    assert!(
        unaddressed.error_code().contains("ValidationException"),
        "case \"a toolResult with no tool request in the conversation\": expected \
         ValidationException, got {:?}",
        unaddressed.error_code()
    );
}

/// The payload of every frame of `name`, in order.
fn frames_named(frames: &[(String, Value)], name: &str) -> Vec<Value> {
    frames
        .iter()
        .filter(|(frame, _)| frame == name)
        .map(|(_, payload)| payload.clone())
        .collect()
}

/// RULE: `ConverseStream` frames the same turn `Converse` serves, so a turn that
/// requests a tool makes the stream open the content block with a
/// `contentBlockStart` naming the minted `toolUseId` and the tool requested,
/// deliver that request's input in the block's `contentBlockDelta` frames, and
/// report `tool_use` on `messageStop`.
#[tokio::test]
async fn converse_stream_frames_the_tool_request_the_turn_makes() {
    let router = emulator();

    for row in choice_rows() {
        let case = row.case;
        let request = json!({
            "messages": user_turn(),
            "toolConfig": row.tool_config,
        });
        let stream: Call = post_json(
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
        let frames = decode_frames(case, &stream.body);
        let order: Vec<&str> = frames.iter().map(|(name, _)| name.as_str()).collect();
        let stops: Vec<String> = frames_named(&frames, "messageStop")
            .iter()
            .map(|payload| payload["stopReason"].as_str().unwrap_or_default().to_string())
            .collect();

        match row.requested {
            Some(expected) => {
                assert_eq!(
                    stops,
                    vec!["tool_use".to_string()],
                    "case {case:?}: a turn offered a tool it can request must stop the \
                     stream on tool_use, got {stops:?} over frames {order:?}"
                );
                let starts = frames_named(&frames, "contentBlockStart");
                assert_eq!(
                    starts.len(),
                    1,
                    "case {case:?}: expected one contentBlockStart, got {starts:?}"
                );
                let start = &starts[0]["start"]["toolUse"];
                assert_eq!(
                    start["name"].as_str(),
                    Some(expected),
                    "case {case:?}: expected the block to start the request for \
                     {expected:?}, got {}",
                    starts[0]
                );
                let id = start["toolUseId"].as_str().unwrap_or_else(|| {
                    panic!("case {case:?}: contentBlockStart carried no toolUseId: {}", starts[0])
                });
                assert_tool_use_id(case, id);
                let input: String = frames_named(&frames, "contentBlockDelta")
                    .iter()
                    .map(|payload| {
                        payload["delta"]["toolUse"]["input"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect();
                assert!(
                    serde_json::from_str::<Value>(&input).is_ok_and(|value| value.is_object()),
                    "case {case:?}: the deltas must concatenate to the tool request's JSON \
                     input, got {input:?}"
                );
                for payload in frames_named(&frames, "contentBlockDelta") {
                    assert!(
                        payload["delta"].get("text").is_none(),
                        "case {case:?}: a turn that requests a tool streams no text, got \
                         {payload}"
                    );
                }
            }
            None => {
                assert_eq!(
                    stops,
                    vec!["end_turn".to_string()],
                    "case {case:?}: a request offering no requestable tool leaves the turn \
                     generating text, got {stops:?} over frames {order:?}"
                );
                assert!(
                    frames_named(&frames, "contentBlockStart").is_empty(),
                    "case {case:?}: a text block opens no contentBlockStart, got {order:?}"
                );
                let streamed: String = frames_named(&frames, "contentBlockDelta")
                    .iter()
                    .map(|payload| {
                        payload["delta"]["text"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect();
                assert!(
                    !streamed.is_empty(),
                    "case {case:?}: expected streamed text, got frames {order:?}"
                );
            }
        }
    }
}
