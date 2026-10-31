// SPDX-License-Identifier: CC0-1.0

//! The generation stage's reach and the refusal wording of the Bedrock Runtime
//! inference surfaces, end to end over HTTP.
//!
//! Four rules are asserted here, each over a SET of inputs rather than over the
//! one request that showed it:
//!
//! * the text one conversation is served is the SAME on every text surface — the
//!   unary turn, the framed deltas and the invoked family's own envelope all come
//!   from the one generation stage, which says that no language model ran;
//! * an `InvokeModel` body whose inference parameter lies outside the range its
//!   provider documents is refused, with that provider's range in the message;
//! * a pagination token the service cannot place is refused with fixed text that
//!   quotes none of the request back;
//! * a `@range` refusal quotes a submitted floating-point value the way the
//!   service writes one.
//!
//! The harness is the one the other suites of this target drive, so what is
//! asserted is the wire behaviour a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state;

use crate::bedrock_runtime_inference::{emulator, get, post_json, send, Call};
use crate::bedrock_runtime_streaming::decode_frames;

/// A model the registry serves, the Converse API supports and the `InvokeModel`
/// surface renders a Nova-family envelope for, so one conversation can be sent to
/// all three text surfaces.
const NOVA: &str = "amazon.nova-lite-v1:0";

/// The text a `Converse` answer carries.
fn converse_text(case: &str, call: &Call) -> String {
    assert_eq!(
        call.status,
        200,
        "case {case:?}: expected 200, got {} / {}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );
    let body = call.json();
    body["output"]["message"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the turn answered no text: {body}"))
        .to_string()
}

/// One `Converse` turn for `messages`.
async fn converse(router: &smithy_server_runtime::MultiServiceRouter, messages: &Value) -> Call {
    post_json(
        router,
        &format!("/model/{NOVA}/converse"),
        &json!({ "messages": messages, "inferenceConfig": {"maxTokens": 512, "temperature": 0} }),
    )
    .await
}

/// The conversations the cross-surface rule is asserted over: the rows differ
/// along the number of turns, the number of content blocks in a turn, the size of
/// the text and its character set.
fn conversations() -> Vec<(&'static str, Value)> {
    vec![
        (
            "one short user turn",
            json!([{"role": "user", "content": [{"text": "Name one planet."}]}]),
        ),
        (
            "three turns",
            json!([
                {"role": "user", "content": [{"text": "What is a page token?"}]},
                {"role": "assistant", "content": [{"text": "An opaque cursor."}]},
                {"role": "user", "content": [{"text": "Why opaque?"}]},
            ]),
        ),
        (
            "several content blocks in one turn",
            json!([{
                "role": "user",
                "content": [{"text": "List three primes."}, {"text": "Then explain why."}],
            }]),
        ),
        (
            "characters outside the basic plane",
            json!([{
                "role": "user",
                "content": [{"text": "\u{65e5}\u{672c}\u{8a9e} caf\u{e9} \u{1f642}"}],
            }]),
        ),
        (
            "a long turn",
            json!([{"role": "user", "content": [{"text": "word ".repeat(200)}]}]),
        ),
    ]
}

/// RULE: every text surface of this service answers from ONE generation stage, so
/// what a conversation is served is that stage's notice that no language model
/// ran — never anything read out of the request — and the unary turn and the
/// reassembled `ConverseStream` deltas of one request are the identical text.
#[tokio::test]
async fn every_text_surface_serves_one_conversation_the_same_text() {
    let router = emulator();

    for (case, messages) in conversations() {
        let unary = converse_text(case, &converse(&router, &messages).await);

        let stream = post_json(
            &router,
            &format!("/model/{NOVA}/converse-stream"),
            &json!({ "messages": messages, "inferenceConfig": {"maxTokens": 512, "temperature": 0} }),
        )
        .await;
        assert_eq!(
            stream.status,
            200,
            "case {case:?}: expected ConverseStream to answer 200, got {} / {}",
            stream.status,
            String::from_utf8_lossy(&stream.body)
        );
        let streamed: String = decode_frames(case, &stream.body)
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
            streamed, unary,
            "case {case:?}: expected the deltas to reassemble to the text Converse serves \
             ({unary:?}), got {streamed:?}"
        );

        // The same conversation in the invoked family's own request body: the
        // envelope is that family's, and the text inside it is the same stage's.
        let native = send(
            &router,
            "POST",
            &format!("/model/{NOVA}/invoke"),
            &[
                ("content-type", "application/json"),
                ("accept", "application/json"),
            ],
            serde_json::to_vec(&json!({
                "messages": messages,
                "inferenceConfig": {"max_new_tokens": 512},
            }))
            .expect("a serializable body"),
        )
        .await;
        assert_eq!(
            native.status,
            200,
            "case {case:?}: expected InvokeModel to answer 200, got {} / {}",
            native.status,
            String::from_utf8_lossy(&native.body)
        );
        let served = native.json();
        let body_text = served
            .pointer("/output/message/content/0/text")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the native envelope carried no text: {served}")
            })
            .to_string();

        // Every surface serves the stage's notice, and none of them answers the
        // request's own text back as if a model had generated it.
        for (surface, text) in [
            ("Converse", &unary),
            ("ConverseStream", &streamed),
            ("InvokeModel", &body_text),
        ] {
            assert!(
                text.starts_with(state::EMULATED_COMPLETION_FLOOR),
                "case {case:?} on {surface}: expected the completion to open with the \
                 stage's notice, got {text:?}"
            );
            for block in messages.as_array().into_iter().flatten() {
                for content in block["content"].as_array().into_iter().flatten() {
                    let sent = content["text"].as_str().unwrap_or_default();
                    assert!(
                        !sent.is_empty() && !text.contains(sent),
                        "case {case:?} on {surface}: the completion must not answer the \
                         request's own text back, got {text:?}"
                    );
                }
            }
        }
    }
}

/// One `InvokeModel` body carrying an inference parameter, and what the provider's
/// documented range makes of it.
struct ParameterRow {
    case: &'static str,
    model_id: &'static str,
    /// The body, with the parameter bound where that family's request format
    /// documents it.
    body: Value,
    /// The refusal the body earns, or `None` for a value inside the range.
    refused: Option<&'static str>,
}

/// The inference-parameter table: one row per family, each parameter the family
/// bounds, values below the minimum, above the maximum, at both bounds and inside
/// the range, and the provider whose documented maximum is 5 rather than 1
/// (Cohere Command's `temperature`).
fn parameter_rows() -> Vec<ParameterRow> {
    let nova = |inference: Value| {
        json!({
            "messages": [{"role": "user", "content": [{"text": "Say hello."}]}],
            "inferenceConfig": inference,
        })
    };
    let temperature_0_1 = Some(
        "Malformed input request: the provided temperature must be between 0.0 and 1.0, \
         please reformat your input and try again.",
    );
    vec![
        ParameterRow {
            case: "nova temperature far above the maximum",
            model_id: NOVA,
            body: nova(json!({"temperature": 5.0, "maxTokens": 8})),
            refused: temperature_0_1,
        },
        ParameterRow {
            case: "nova temperature just above the maximum",
            model_id: NOVA,
            body: nova(json!({"temperature": 1.5, "maxTokens": 8})),
            refused: temperature_0_1,
        },
        ParameterRow {
            case: "nova temperature below the minimum",
            model_id: NOVA,
            body: nova(json!({"temperature": -0.25, "maxTokens": 8})),
            refused: temperature_0_1,
        },
        ParameterRow {
            case: "nova temperature at the maximum",
            model_id: NOVA,
            body: nova(json!({"temperature": 1.0, "maxTokens": 8})),
            refused: None,
        },
        ParameterRow {
            case: "nova temperature at the minimum",
            model_id: NOVA,
            body: nova(json!({"temperature": 0.0, "maxTokens": 8})),
            refused: None,
        },
        ParameterRow {
            case: "nova temperature inside the range",
            model_id: NOVA,
            body: nova(json!({"temperature": 0.5, "maxTokens": 8})),
            refused: None,
        },
        ParameterRow {
            case: "nova topP above the maximum",
            model_id: NOVA,
            body: nova(json!({"topP": 1.2, "maxTokens": 8})),
            refused: Some(
                "Malformed input request: the provided topP must be between 0.0 and 1.0, \
                 please reformat your input and try again.",
            ),
        },
        ParameterRow {
            case: "anthropic temperature above the maximum",
            model_id: "anthropic.claude-3-haiku-20240307-v1:0",
            body: json!({
                "anthropic_version": "bedrock-2023-05-31",
                "max_tokens": 8,
                "temperature": 1.2,
                "messages": [{"role": "user", "content": [{"type": "text", "text": "Say hello."}]}],
            }),
            refused: temperature_0_1,
        },
        ParameterRow {
            case: "anthropic top_p below the minimum",
            model_id: "anthropic.claude-3-haiku-20240307-v1:0",
            body: json!({
                "anthropic_version": "bedrock-2023-05-31",
                "max_tokens": 8,
                "top_p": -0.1,
                "messages": [{"role": "user", "content": [{"type": "text", "text": "Say hello."}]}],
            }),
            refused: Some(
                "Malformed input request: the provided top_p must be between 0.0 and 1.0, \
                 please reformat your input and try again.",
            ),
        },
        ParameterRow {
            case: "titan topP above the maximum, in the member titan binds it to",
            model_id: "amazon.titan-text-premier-v1:0",
            body: json!({
                "inputText": "Say hello.",
                "textGenerationConfig": {"maxTokenCount": 8, "topP": 3.0},
            }),
            refused: Some(
                "Malformed input request: the provided topP must be between 0.0 and 1.0, \
                 please reformat your input and try again.",
            ),
        },
        ParameterRow {
            case: "llama top_p above the maximum",
            model_id: "meta.llama3-8b-instruct-v1:0",
            body: json!({"prompt": "Say hello.", "max_gen_len": 8, "top_p": 1.01}),
            refused: Some(
                "Malformed input request: the provided top_p must be between 0.0 and 1.0, \
                 please reformat your input and try again.",
            ),
        },
        ParameterRow {
            case: "mistral temperature below the minimum",
            model_id: "mistral.mistral-7b-instruct-v0:2",
            body: json!({"prompt": "Say hello.", "max_tokens": 8, "temperature": -2.0}),
            refused: temperature_0_1,
        },
        ParameterRow {
            case: "cohere temperature inside its own wider documented range",
            model_id: "cohere.command-text-v14",
            body: json!({"prompt": "Say hello.", "max_tokens": 8, "temperature": 3.0}),
            refused: None,
        },
        ParameterRow {
            case: "cohere temperature above its own documented maximum",
            model_id: "cohere.command-text-v14",
            body: json!({"prompt": "Say hello.", "max_tokens": 8, "temperature": 5.5}),
            refused: Some(
                "Malformed input request: the provided temperature must be between 0.0 and \
                 5.0, please reformat your input and try again.",
            ),
        },
        ParameterRow {
            case: "cohere p above the maximum",
            model_id: "cohere.command-text-v14",
            body: json!({"prompt": "Say hello.", "max_tokens": 8, "p": 1.1}),
            refused: Some(
                "Malformed input request: the provided p must be between 0.0 and 1.0, please \
                 reformat your input and try again.",
            ),
        },
    ]
}

/// RULE: an `InvokeModel` body is validated against the inference-parameter
/// ranges the invoked model's provider documents, so a value outside one is
/// refused `400 ValidationException` naming that parameter and its two bounds,
/// and a value inside it — the bounds themselves included — is served.
#[tokio::test]
async fn invoke_model_refuses_an_inference_parameter_outside_the_providers_range() {
    let router = emulator();

    for row in parameter_rows() {
        let case = row.case;
        let call = send(
            &router,
            "POST",
            &format!("/model/{}/invoke", row.model_id),
            &[
                ("content-type", "application/json"),
                ("accept", "application/json"),
            ],
            serde_json::to_vec(&row.body).expect("a serializable body"),
        )
        .await;
        match row.refused {
            Some(message) => {
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
                let stated = call.json()["message"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert_eq!(
                    stated, message,
                    "case {case:?}: expected the message {message:?}, got {stated:?}"
                );
            }
            None => assert_eq!(
                call.status,
                200,
                "case {case:?}: expected the in-range value to be served, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            ),
        }
    }
}

/// RULE: a pagination token the service cannot place is refused
/// `400 ValidationException` with the FIXED sentence for that refusal, which
/// carries no value from the request — while the `maxResults` range refusals,
/// whose template does name the submitted value and the bound, keep stating both.
#[tokio::test]
async fn list_async_invokes_refuses_a_pagination_token_with_fixed_text() {
    let router = emulator();

    // Rows along what the rule generalises over: a token that is not the encoding
    // at all, one that decodes to something else, one that is well-formed but
    // names no record, a long one, and ones carrying characters a client might
    // expect to be echoed (submitted percent-encoded, so the service reads the
    // decoded form).
    let long = "a".repeat(2048);
    let tokens: [(&str, &str); 6] = [
        ("not-a-valid-token!!", "not-a-valid-token!!"),
        ("6162", "6162"),
        ("zz", "zz"),
        (long.as_str(), long.as_str()),
        ("%3Cscript%3E", "<script>"),
        ("%7Bevil%7D", "{evil}"),
    ];
    for (token, decoded) in tokens {
        let call = get(&router, &format!("/async-invoke?nextToken={token}")).await;
        assert_eq!(
            call.status,
            400,
            "case {token:?}: expected 400, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {token:?}: expected ValidationException, got {:?}",
            call.error_code()
        );
        let stated = call.json()["message"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            stated, "Invalid pagination token was passed",
            "case {token:?}: expected the fixed refusal sentence, got {stated:?}"
        );
        assert!(
            !stated.contains(decoded),
            "case {token:?}: the refusal must not quote the submitted token back, got \
             {stated:?}"
        );
    }

    // The other refusals of the same operation are a different template, whose
    // variable parts ARE the submitted value and the bound: the published bounds
    // of `maxResults` (`@range` min 1, max 1000, from
    // `smithy_query(query="shape", shape="ListAsyncInvokesRequest")`).
    for (value, requirement) in [
        ("0", "Member must have value greater than or equal to 1"),
        ("1001", "Member must have value less than or equal to 1000"),
    ] {
        let call = get(&router, &format!("/async-invoke?maxResults={value}")).await;
        let stated = call.json()["message"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let expected = format!(
            "1 validation error detected: Value '{value}' at 'maxResults' failed to satisfy \
             constraint: {requirement}"
        );
        assert_eq!(
            stated, expected,
            "case maxResults={value}: expected {expected:?}, got {stated:?}"
        );
    }
}

/// One `@range` refusal: the value a request submits for a floating-point member,
/// and how the clause quotes it back.
struct FloatRow {
    case: &'static str,
    /// The `inferenceConfig` the request carries.
    inference: Value,
    /// The member path the clause names, and the requirement it states.
    path: &'static str,
    requirement: &'static str,
    /// The text the clause quotes the submitted value with.
    rendered: &'static str,
}

/// The float-rendering table: the rows differ along the MAGNITUDE of the
/// submitted value (far below the plain-decimal band, just below it, inside it,
/// at and past its upper end), its sign, whether it is whole or fractional, and
/// which of the two floating-point members of `InferenceConfiguration`
/// (`temperature` and `topP`, both `Float` with `@range` {min 0, max 1} from
/// `smithy_query(query="shape", shape="InferenceConfiguration")`) carries it.
fn float_rows() -> Vec<FloatRow> {
    let min_0 = "Member must have value greater than or equal to 0";
    let max_1 = "Member must have value less than or equal to 1";
    vec![
        FloatRow {
            case: "a small negative magnitude below the minimum",
            inference: json!({"temperature": 0.0, "topP": -1e-6, "maxTokens": 10}),
            path: "inferenceConfig.topP",
            requirement: min_0,
            rendered: "-1.0E-6",
        },
        FloatRow {
            case: "a smaller negative magnitude with more digits",
            inference: json!({"topP": -1.5e-7, "maxTokens": 10}),
            path: "inferenceConfig.topP",
            requirement: min_0,
            rendered: "-1.5E-7",
        },
        FloatRow {
            case: "just below the plain-decimal band",
            inference: json!({"temperature": -0.0009, "maxTokens": 10}),
            path: "inferenceConfig.temperature",
            requirement: min_0,
            rendered: "-9.0E-4",
        },
        FloatRow {
            case: "inside the plain-decimal band, just over the maximum",
            inference: json!({"temperature": 1.000001, "maxTokens": 10}),
            path: "inferenceConfig.temperature",
            requirement: max_1,
            rendered: "1.000001",
        },
        FloatRow {
            case: "inside the plain-decimal band, below the minimum",
            inference: json!({"temperature": -0.5, "maxTokens": 10}),
            path: "inferenceConfig.temperature",
            requirement: min_0,
            rendered: "-0.5",
        },
        FloatRow {
            case: "a whole value, which still carries a fractional part",
            inference: json!({"topP": 2.0, "maxTokens": 10}),
            path: "inferenceConfig.topP",
            requirement: max_1,
            rendered: "2.0",
        },
        FloatRow {
            case: "at the upper end of the plain-decimal band",
            inference: json!({"temperature": 1e7, "maxTokens": 10}),
            path: "inferenceConfig.temperature",
            requirement: max_1,
            rendered: "1.0E7",
        },
        FloatRow {
            case: "past the upper end of the band, with several digits",
            inference: json!({"topP": 12345680.0, "maxTokens": 10}),
            path: "inferenceConfig.topP",
            requirement: max_1,
            rendered: "1.234568E7",
        },
    ]
}

/// RULE: a constraint-violation clause quotes the submitted value the way the
/// service WRITES that member's type — a floating-point value always carrying a
/// fractional part or an exponent, in scientific notation outside the
/// plain-decimal band — while the member path, the bound and the envelope stay the
/// ones the model and the framework give it. A value inside the declared range is
/// not refused at all.
#[tokio::test]
async fn a_range_refusal_quotes_a_submitted_float_the_way_the_service_writes_one() {
    let router = emulator();

    for row in float_rows() {
        let case = row.case;
        let call = post_json(
            &router,
            &format!("/model/{NOVA}/converse"),
            &json!({
                "messages": [{"role": "user", "content": [{"text": "Say hello."}]}],
                "inferenceConfig": row.inference,
            }),
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
        let stated = call.json()["message"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let expected = format!(
            "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: {}",
            row.rendered, row.path, row.requirement
        );
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }

    // The boundary values the range admits are served, so the rendering rule is
    // about a refusal and not about the range being read differently.
    for inference in [
        json!({"temperature": 0.0, "topP": 0.0, "maxTokens": 10}),
        json!({"temperature": 1.0, "topP": 1.0, "maxTokens": 10}),
        json!({"temperature": 0.000001, "topP": 0.999999, "maxTokens": 10}),
    ] {
        let call = post_json(
            &router,
            &format!("/model/{NOVA}/converse"),
            &json!({
                "messages": [{"role": "user", "content": [{"text": "Say hello."}]}],
                "inferenceConfig": inference,
            }),
        )
        .await;
        assert_eq!(
            call.status,
            200,
            "case {inference}: expected the in-range configuration to be served, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
    }
}
