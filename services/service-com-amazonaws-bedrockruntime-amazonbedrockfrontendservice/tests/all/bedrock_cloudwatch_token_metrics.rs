// SPDX-License-Identifier: CC0-1.0

//! The `AWS/Bedrock` metric data a served inference publishes, asserted where a
//! client reads it: through CloudWatch's own `GetMetricStatistics` and
//! `ListMetrics`, with both services mounted on one shared engine exactly as
//! `core/router` assembles them.
//!
//! RULE under test: a Bedrock Runtime call that SERVES an inference accounts for
//! the token volume it reports inline as its own contribution to the
//! `AWS/Bedrock` series `InputTokenCount` and `OutputTokenCount`, dimensioned on
//! the invoked model id alone — so the counts a response describes are the ones
//! CloudWatch surfaces for that model, and a model nothing invoked has no series
//! at all.
//!
//! The table below spans what the rule generalises over: the invocation surface
//! (`Converse`, `ConverseStream`, `InvokeModel`,
//! `InvokeModelWithResponseStream`), the model and its response family, and the
//! token volume (prompt size and turn count). The recorded case — two `Converse`
//! calls on one model — is ONE row.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::EmulatedService;
// The namespace, dimension and metric names under test, read from this service's
// own publication rule so the suite and the handler cannot disagree about which
// series is meant.
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::invocation_metrics::{
    INPUT_TOKEN_COUNT, MODEL_ID_DIMENSION, NAMESPACE, OUTPUT_TOKEN_COUNT,
};
use smithy_server_runtime::MultiServiceRouter;

// The Bedrock Runtime request walk (`post_json`, its `Call`) and the
// event-stream decoders (`decode_frames`, `chunk_payloads`) are the ones the
// other suites of this target drive the service through, so this suite reaches
// it through the SAME definitions rather than a second copy of them.
use crate::bedrock_runtime_inference::post_json;
use crate::bedrock_runtime_streaming::{chunk_payloads, decode_frames};

/// The `X-Amz-Target` prefix the CloudWatch model declares.
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// How many seconds either side of the calls the metric window spans.
const WINDOW_SECS: f64 = 180.0;

/// The aggregation period the Bedrock runtime metrics are published at, which is
/// also the period every read below requests.
const PERIOD_SECS: i64 = 60;

/// The same period as a number of seconds, derived from the one constant above:
/// a read's windows are anchored at the requested start time rounded DOWN to the
/// minute, so a datapoint's own stamp — the start of the window it was folded out
/// of — may precede the requested `StartTime` by at most one period.
#[allow(clippy::cast_precision_loss)]
const PERIOD_SECS_F64: f64 = PERIOD_SECS as f64;

/// Which surface a row drives the inference through, with the request body it
/// sends and — for the unary `InvokeModel` surface, whose envelope is the model
/// family's own — the JSON pointers its token counts are reported at.
enum Surface {
    Converse {
        messages: Value,
    },
    ConverseStream {
        messages: Value,
    },
    InvokeModel {
        body: Value,
        input_pointer: &'static str,
        output_pointer: &'static str,
    },
    InvokeModelWithResponseStream {
        body: Value,
    },
}

/// One row of the table: a case name, the model to invoke and the surface to
/// invoke it through.
struct Row {
    case: &'static str,
    model_id: &'static str,
    surface: Surface,
}

/// The table: every invocation surface, four response families, and token
/// volumes from a five-character prompt to a several-hundred-word one.
///
/// The model ids are from "Amazon Bedrock base model IDs (on-demand
/// throughput)" and the request/response envelopes from "Inference request
/// parameters and response fields for foundation models", both in the Amazon
/// Bedrock User Guide.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "Converse, one short turn",
            model_id: "amazon.nova-lite-v1:0",
            surface: Surface::Converse {
                messages: json!([{"role": "user", "content": [{"text": "Name three colours, comma separated, nothing else."}]}]),
            },
        },
        Row {
            case: "Converse, three turns and a long prompt",
            model_id: "anthropic.claude-3-haiku-20240307-v1:0",
            surface: Surface::Converse {
                messages: json!([
                    {"role": "user", "content": [{"text": "word ".repeat(400)}]},
                    {"role": "assistant", "content": [{"text": "An opaque cursor."}]},
                    {"role": "user", "content": [{"text": "Why opaque?"}]},
                ]),
            },
        },
        Row {
            case: "ConverseStream, one short turn",
            model_id: "amazon.nova-lite-v1:0",
            surface: Surface::ConverseStream {
                messages: json!([{"role": "user", "content": [{"text": "hello"}]}]),
            },
        },
        Row {
            case: "InvokeModel, Titan Text envelope",
            model_id: "amazon.titan-text-premier-v1:0",
            surface: Surface::InvokeModel {
                body: json!({
                    "inputText": "Write a haiku about persistent state.",
                    "textGenerationConfig": {"maxTokenCount": 512},
                }),
                input_pointer: "/inputTextTokenCount",
                output_pointer: "/results/0/tokenCount",
            },
        },
        Row {
            case: "InvokeModel, Llama envelope with a long prompt",
            model_id: "meta.llama3-8b-instruct-v1:0",
            surface: Surface::InvokeModel {
                body: json!({
                    "prompt": "Explain idempotency to a cat. ".repeat(60),
                    "max_gen_len": 128,
                }),
                input_pointer: "/prompt_token_count",
                output_pointer: "/generation_token_count",
            },
        },
        Row {
            case: "InvokeModelWithResponseStream, Anthropic envelope",
            model_id: "anthropic.claude-3-haiku-20240307-v1:0",
            surface: Surface::InvokeModelWithResponseStream {
                body: json!({
                    "anthropic_version": "bedrock-2023-05-31",
                    "max_tokens": 256,
                    "messages": [{"role": "user", "content": [{"type": "text", "text": "Summarize the Mars mission in one line."}]}],
                }),
            },
        },
    ]
}

/// The emulator under test: the production router with the Bedrock Runtime data
/// plane and CloudWatch mounted on ONE engine, which is what makes the metric
/// store the inference writes the same one CloudWatch reads.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// Wall-clock now as epoch seconds, the clock a metric window is measured
/// against.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// One `awsJson1_0` CloudWatch call, requiring a 200 — the protocol and target
/// prefix that service serves (its README / OPERATIONS.md "Wire protocol").
async fn cloudwatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "monitoring.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"),
        )
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEMULATOR000000000/20260101/us-east-1/monitoring/aws4_request, \
             SignedHeaders=host, Signature=0000",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("a serializable request"),
        ))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "{operation}: expected HTTP 200, got {status} with body {parsed}"
    );
    parsed
}

/// The number a JSON pointer addresses, as a count.
fn count_at(case: &str, document: &Value, pointer: &str) -> f64 {
    document
        .pointer(pointer)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| {
            panic!("case {case:?}: no token count at {pointer} in {document}")
        })
}

/// Serve one inference through the row's surface and return the token counts the
/// response reported INLINE — the oracle the published series is judged against.
async fn reported_usage(router: &MultiServiceRouter, row: &Row) -> (f64, f64) {
    let case = row.case;
    let model_id = row.model_id;
    match &row.surface {
        Surface::Converse { messages } => {
            let call = post_json(
                router,
                &format!("/model/{model_id}/converse"),
                &json!({"messages": messages}),
            )
            .await;
            assert_eq!(
                call.status,
                200,
                "case {case:?}: Converse must be served, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            let body = call.json();
            (
                count_at(case, &body, "/usage/inputTokens"),
                count_at(case, &body, "/usage/outputTokens"),
            )
        }
        Surface::ConverseStream { messages } => {
            let call = post_json(
                router,
                &format!("/model/{model_id}/converse-stream"),
                &json!({"messages": messages}),
            )
            .await;
            assert_eq!(
                call.status,
                200,
                "case {case:?}: ConverseStream must be served, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            let frames = decode_frames(case, &call.body);
            let metadata = frames
                .iter()
                .find(|(event_type, _)| event_type == "metadata")
                .map(|(_, payload)| payload.clone())
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the stream carried no metadata event: {frames:?}")
                });
            (
                count_at(case, &metadata, "/usage/inputTokens"),
                count_at(case, &metadata, "/usage/outputTokens"),
            )
        }
        Surface::InvokeModel {
            body,
            input_pointer,
            output_pointer,
        } => {
            let call = post_json(router, &format!("/model/{model_id}/invoke"), body).await;
            assert_eq!(
                call.status,
                200,
                "case {case:?}: InvokeModel must be served, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            let answered = call.json();
            (
                count_at(case, &answered, input_pointer),
                count_at(case, &answered, output_pointer),
            )
        }
        Surface::InvokeModelWithResponseStream { body } => {
            let call = post_json(
                router,
                &format!("/model/{model_id}/invoke-with-response-stream"),
                body,
            )
            .await;
            assert_eq!(
                call.status,
                200,
                "case {case:?}: InvokeModelWithResponseStream must be served, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            // The terminal chunk of a response stream carries the invocation's
            // own token accounting under `amazon-bedrock-invocationMetrics`.
            let metrics = chunk_payloads(case, &call.body)
                .into_iter()
                .filter_map(|chunk| chunk.get("amazon-bedrock-invocationMetrics").cloned())
                .next_back()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no chunk reported the invocation metrics")
                });
            (
                count_at(case, &metrics, "/inputTokenCount"),
                count_at(case, &metrics, "/outputTokenCount"),
            )
        }
    }
}

/// `GetMetricStatistics` over one model's series of `metric`, as the whole span
/// around the calls: its datapoints, each `{Timestamp, Sum, Unit}`.
async fn datapoints(
    router: &MultiServiceRouter,
    metric: &str,
    model_id: &str,
    start: f64,
    end: f64,
) -> Vec<Value> {
    let answered = cloudwatch(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": [{"Name": MODEL_ID_DIMENSION, "Value": model_id}],
            "StartTime": start,
            "EndTime": end,
            "Period": PERIOD_SECS,
            "Statistics": ["Sum"],
        }),
    )
    .await;
    answered
        .get("Datapoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Sum` of every datapoint of a span, which is the volume the series
/// accounts for over it.
fn total(points: &[Value]) -> f64 {
    points
        .iter()
        .filter_map(|point| point.get("Sum").and_then(Value::as_f64))
        .sum()
}

/// RULE: a served inference accounts for the token counts it reported inline as
/// its own contribution to the `AWS/Bedrock` series of the invoked model, so the
/// `Sum` over the calls' window is exactly the counts the responses reported,
/// every datapoint falls inside that window, and the series is surfaced
/// dimensioned on the model id alone.
#[tokio::test]
async fn a_served_inference_publishes_the_token_counts_it_reported_inline() {
    for row in rows() {
        let case = row.case;
        let router = emulator();
        let opened = now_secs();

        // Two calls, as a client that invokes twice: the series accounts for
        // BOTH, which is what makes it a sum rather than a last value.
        let first = reported_usage(&router, &row).await;
        let second = reported_usage(&router, &row).await;
        let expected_input = first.0 + second.0;
        let expected_output = first.1 + second.1;
        assert!(
            expected_input > 0.0 && expected_output > 0.0,
            "case {case:?}: the two calls must report input and output tokens to account for, got \
             {expected_input} and {expected_output}"
        );

        let start = opened - WINDOW_SECS;
        let end = now_secs() + WINDOW_SECS;
        for (metric, expected) in [
            (INPUT_TOKEN_COUNT, expected_input),
            (OUTPUT_TOKEN_COUNT, expected_output),
        ] {
            let points = datapoints(&router, metric, row.model_id, start, end).await;
            let actual = total(&points);
            assert!(
                (actual - expected).abs() < 0.5,
                "case {case:?}: {metric} for {} must sum to the {expected} tokens the two calls \
                 reported, got {actual} from {points:?}",
                row.model_id
            );
            assert!(
                !points.is_empty(),
                "case {case:?}: {metric} for {} must have datapoints",
                row.model_id
            );
            for point in &points {
                let stamp = point
                    .get("Timestamp")
                    .and_then(Value::as_f64)
                    .unwrap_or_else(|| panic!("case {case:?}: a datapoint carried no Timestamp"));
                assert!(
                    (start - PERIOD_SECS_F64..=end).contains(&stamp),
                    "case {case:?}: {metric} datapoint {stamp} must fall inside the requested \
                     window {start}..={end}"
                );
                assert_eq!(
                    point.get("Unit").and_then(Value::as_str),
                    Some("Count"),
                    "case {case:?}: {metric} counts tokens, so its unit must be Count, got \
                     {point:?}"
                );
            }
        }

        // The dimension is part of the metric's identity, so the series is
        // surfaced under exactly the invoked model id and nothing else.
        for metric in [INPUT_TOKEN_COUNT, OUTPUT_TOKEN_COUNT] {
            let listed = cloudwatch(
                &router,
                "ListMetrics",
                &json!({"Namespace": NAMESPACE, "MetricName": metric}),
            )
            .await;
            assert_eq!(
                listed.get("Metrics"),
                Some(&json!([{
                    "Namespace": NAMESPACE,
                    "MetricName": metric,
                    "Dimensions": [{"Name": MODEL_ID_DIMENSION, "Value": row.model_id}],
                }])),
                "case {case:?}: ListMetrics must surface {metric} dimensioned on exactly the \
                 invoked model, got {listed}"
            );
        }
    }
}

/// RULE (the same rule read from the other side): the dimension is honoured
/// rather than aggregated away, so a well-formed model id that was never
/// invoked has no series and no datapoints at all.
#[tokio::test]
async fn a_model_that_was_never_invoked_has_no_series() {
    // Models the registry serves, so the only thing separating them here is
    // which one a call actually invoked.
    const INVOKED: &str = "amazon.nova-lite-v1:0";
    const NEVER_INVOKED: &[&str] = &[
        "amazon.titan-text-premier-v1:0",
        "meta.llama3-8b-instruct-v1:0",
        "anthropic.claude-3-haiku-20240307-v1:0",
    ];

    let router = emulator();
    let opened = now_secs();
    let row = Row {
        case: "the one invoked model",
        model_id: INVOKED,
        surface: Surface::Converse {
            messages: json!([{"role": "user", "content": [{"text": "Name three colours."}]}]),
        },
    };
    let (input_tokens, _) = reported_usage(&router, &row).await;
    let start = opened - WINDOW_SECS;
    let end = now_secs() + WINDOW_SECS;

    let invoked = datapoints(&router, INPUT_TOKEN_COUNT, INVOKED, start, end).await;
    assert!(
        (total(&invoked) - input_tokens).abs() < 0.5,
        "the invoked model's series must account for its {input_tokens} reported tokens, got \
         {invoked:?}"
    );

    for model_id in NEVER_INVOKED {
        for metric in [INPUT_TOKEN_COUNT, OUTPUT_TOKEN_COUNT] {
            let points = datapoints(&router, metric, model_id, start, end).await;
            assert!(
                points.is_empty(),
                "case {model_id:?}: a model nothing invoked must have no {metric} datapoints, got \
                 {points:?}"
            );
        }
        let listed = cloudwatch(
            &router,
            "ListMetrics",
            &json!({
                "Namespace": NAMESPACE,
                "Dimensions": [{"Name": MODEL_ID_DIMENSION, "Value": model_id}],
            }),
        )
        .await;
        assert_eq!(
            listed.get("Metrics"),
            Some(&json!([])),
            "case {model_id:?}: a model nothing invoked must surface no series, got {listed}"
        );
    }
}

/// RULE: reading the series does not change it — `GetMetricStatistics` is a
/// report, so the same window answers the same volume however often it is read,
/// and a second inference adds its own counts rather than replacing them.
#[tokio::test]
async fn reading_the_series_reports_it_without_changing_it() {
    let router = emulator();
    let opened = now_secs();
    let row = Row {
        case: "one Converse turn",
        model_id: "amazon.nova-lite-v1:0",
        surface: Surface::Converse {
            messages: json!([{"role": "user", "content": [{"text": "Define idempotency."}]}]),
        },
    };

    let (first_input, _) = reported_usage(&router, &row).await;
    let start = opened - WINDOW_SECS;
    let end = now_secs() + WINDOW_SECS;

    let once = total(&datapoints(&router, INPUT_TOKEN_COUNT, row.model_id, start, end).await);
    let twice = total(&datapoints(&router, INPUT_TOKEN_COUNT, row.model_id, start, end).await);
    assert!(
        (once - twice).abs() < 0.5 && (once - first_input).abs() < 0.5,
        "two reads of one window must report the same {first_input} tokens, got {once} then {twice}"
    );

    let (second_input, _) = reported_usage(&router, &row).await;
    let after = total(
        &datapoints(
            &router,
            INPUT_TOKEN_COUNT,
            row.model_id,
            start,
            now_secs() + WINDOW_SECS,
        )
        .await,
    );
    assert!(
        (after - (first_input + second_input)).abs() < 0.5,
        "a second inference must add its {second_input} tokens to the {first_input} already \
         accounted for, got {after}"
    );
}
