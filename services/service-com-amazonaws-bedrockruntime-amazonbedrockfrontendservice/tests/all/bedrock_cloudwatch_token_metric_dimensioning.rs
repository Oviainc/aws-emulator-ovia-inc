// SPDX-License-Identifier: CC0-1.0

//! The DIMENSION side of the `AWS/Bedrock` token publication: one emulator,
//! several models invoked through several surfaces, read back through
//! CloudWatch's own `GetMetricStatistics` and `ListMetrics`.
//!
//! RULE under test: the invoked model id is part of a published `AWS/Bedrock`
//! series' identity rather than a label on one aggregate, so each model's
//! `InputTokenCount` / `OutputTokenCount` accounts for exactly the token volume
//! the calls on THAT model reported inline, a call that served no inference
//! contributes to no series, and `ListMetrics` surfaces one series per metric per
//! invoked model and none for a model nothing invoked.
//!
//! The suite that asserts the token volume itself
//! ([`crate::bedrock_cloudwatch_token_metrics`]) drives one model per emulator,
//! so it cannot see two series bleeding into each other; the table below spans
//! what the dimension rule generalises over: how many models are live at once,
//! which surface and response family each is invoked through, and how many calls
//! each gets. The recorded case — one model, two `Converse` calls — is ONE row of
//! it.

use std::collections::BTreeSet;

use serde_json::{json, Value};
// The namespace, dimension and metric names under test, read from this service's
// own publication rule so the suite and the handler cannot disagree about which
// series is meant.
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::invocation_metrics::{
    INPUT_TOKEN_COUNT, INVOCATIONS, INVOCATION_LATENCY, MODEL_ID_DIMENSION, NAMESPACE,
    OUTPUT_TOKEN_COUNT,
};
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState as ServiceState;

// The request walk, the emulator assembly and the inline-usage oracle are the
// ones the token-volume suite drives this service through, so this suite reaches
// them through the SAME definitions rather than a second copy of them.
use crate::bedrock_cloudwatch_token_metrics::{
    cloudwatch, emulator, now_secs, reported_usage, statistics_of, total, Row, Surface,
    WINDOW_SECS,
};
use crate::bedrock_runtime_inference::post_json;

/// How close two token totals have to be to count as the same volume: a count is
/// a whole number, so anything inside half a token is the same reading.
const SAME_COUNT: f64 = 0.5;

/// A well-formed model id naming no model at all, as a client inventing a
/// dimension value would send it.
const NEVER_INVOKED: &str = "example.no-such-model-v1:0";

/// The table: three models live in ONE emulator, each invoked through a
/// different surface and response family, a different number of times, with a
/// different prompt size — so the three token volumes are distinct and a series
/// that aggregated them could not match any of them.
///
/// The model ids are from "Amazon Bedrock base model IDs (on-demand
/// throughput)" and the `InvokeModel` request/response envelopes from "Inference
/// request parameters and response fields for foundation models", both in the
/// Amazon Bedrock User Guide.
fn invoked_rows() -> Vec<(Row, usize)> {
    vec![
        (
            Row {
                case: "nova-lite, two Converse calls",
                model_id: "amazon.nova-lite-v1:0",
                surface: Surface::Converse {
                    messages: json!([{"role": "user", "content": [{"text": "Name three colours, comma separated, nothing else."}]}]),
                },
            },
            2,
        ),
        (
            Row {
                case: "claude-3-haiku, one ConverseStream call with a long prompt",
                model_id: "anthropic.claude-3-haiku-20240307-v1:0",
                surface: Surface::ConverseStream {
                    messages: json!([{"role": "user", "content": [{"text": "word ".repeat(300)}]}]),
                },
            },
            1,
        ),
        (
            Row {
                case: "titan-text-premier, three InvokeModel calls",
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
            3,
        ),
    ]
}

/// Every `{MetricName, ModelId}` series the namespace surfaces, as a set: the
/// order `ListMetrics` pages series in is not part of the rule, but which series
/// exist is.
async fn listed_series(
    router: &smithy_server_runtime::MultiServiceRouter,
) -> BTreeSet<(String, String)> {
    let listed = cloudwatch(router, "ListMetrics", &json!({"Namespace": NAMESPACE})).await;
    let metrics = listed
        .get("Metrics")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("ListMetrics must answer a Metrics list, got {listed}"));
    metrics
        .iter()
        .map(|metric| {
            let name = metric
                .get("MetricName")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("a listed series carried no MetricName: {metric}"));
            // A published series carries exactly the one dimension the Bedrock
            // runtime metrics are dimensioned on.
            let dimensions = metric
                .get("Dimensions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                dimensions.len(),
                1,
                "a listed series must carry exactly the {MODEL_ID_DIMENSION} dimension, got \
                 {metric}"
            );
            assert_eq!(
                dimensions[0].get("Name").and_then(Value::as_str),
                Some(MODEL_ID_DIMENSION),
                "a listed series must carry exactly the {MODEL_ID_DIMENSION} dimension, got \
                 {metric}"
            );
            let model_id = dimensions[0]
                .get("Value")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("a listed dimension carried no Value: {metric}"));
            (name.to_string(), model_id.to_string())
        })
        .collect()
}

/// RULE: each model's token series accounts for exactly the volume the calls on
/// THAT model reported inline, so three models live at once keep three separate
/// accounts rather than sharing one aggregate.
#[tokio::test]
async fn each_model_s_token_series_accounts_for_its_own_calls_alone() {
    let router = emulator();
    let opened = now_secs();

    // Serve every row's calls into the one emulator, summing what each call
    // reported inline — the oracle its own model's series is judged against.
    let mut expected: Vec<(&'static str, &'static str, f64, f64)> = Vec::new();
    for (row, calls) in invoked_rows() {
        let mut inputs = 0.0;
        let mut outputs = 0.0;
        for _ in 0..calls {
            let (input, output) = reported_usage(&router, &row).await;
            inputs += input;
            outputs += output;
        }
        assert!(
            inputs > 0.0 && outputs > 0.0,
            "case {:?}: the calls must report tokens to account for, got {inputs} and {outputs}",
            row.case
        );
        expected.push((row.case, row.model_id, inputs, outputs));
    }

    // The volumes have to be pairwise distinct, otherwise a series that summed
    // every model together could still match one of them.
    for (left, right) in expected
        .iter()
        .enumerate()
        .flat_map(|(index, left)| expected[index + 1..].iter().map(move |right| (left, right)))
    {
        assert!(
            (left.2 - right.2).abs() >= SAME_COUNT,
            "the table must give {:?} and {:?} distinct input volumes, got {} and {}",
            left.0,
            right.0,
            left.2,
            right.2
        );
    }

    let start = opened - WINDOW_SECS;
    let end = now_secs() + WINDOW_SECS;
    let whole_namespace: f64 = expected.iter().map(|row| row.2).sum();
    for (case, model_id, inputs, outputs) in &expected {
        for (metric, volume) in [(INPUT_TOKEN_COUNT, *inputs), (OUTPUT_TOKEN_COUNT, *outputs)] {
            let points = statistics_of(&router, metric, model_id, start, end, &["Sum"]).await;
            let actual = total(&points);
            assert!(
                (actual - volume).abs() < SAME_COUNT,
                "case {case:?}: {metric} at {MODEL_ID_DIMENSION}={model_id} must account for the \
                 {volume} tokens that model's own calls reported, got {actual} from {points:?}"
            );
        }
        let actual = total(&statistics_of(&router, INPUT_TOKEN_COUNT, model_id, start, end, &["Sum"]).await);
        assert!(
            (actual - whole_namespace).abs() >= SAME_COUNT,
            "case {case:?}: {INPUT_TOKEN_COUNT} at {MODEL_ID_DIMENSION}={model_id} must not carry \
             the whole namespace's {whole_namespace} tokens, got {actual}"
        );
    }
}

/// RULE (the discoverability side): `ListMetrics` surfaces one series per metric
/// per INVOKED model and nothing else, so an operator finds exactly the models
/// that ran inference.
#[tokio::test]
async fn list_metrics_surfaces_one_series_per_metric_per_invoked_model() {
    let router = emulator();
    let mut invoked: Vec<&'static str> = Vec::new();
    for (row, calls) in invoked_rows() {
        for _ in 0..calls {
            reported_usage(&router, &row).await;
        }
        invoked.push(row.model_id);
    }

    // The four per-invocation series the Bedrock runtime metrics documentation
    // lists, as this service's publication rule names them.
    let wanted: BTreeSet<(String, String)> = [
        INPUT_TOKEN_COUNT,
        OUTPUT_TOKEN_COUNT,
        INVOCATIONS,
        INVOCATION_LATENCY,
    ]
    .into_iter()
    .flat_map(|metric| {
        invoked
            .iter()
            .map(move |model_id| (metric.to_string(), (*model_id).to_string()))
    })
    .collect();

    let surfaced = listed_series(&router).await;
    assert_eq!(
        surfaced, wanted,
        "ListMetrics must surface exactly one series per metric per invoked model, got \
         {surfaced:?} instead of {wanted:?}"
    );
    assert!(
        surfaced.iter().all(|(_, model_id)| model_id != NEVER_INVOKED),
        "a model nothing invoked must have no series, got {surfaced:?}"
    );
}

/// RULE (the refusal side): a call that served no inference has no token volume
/// to account for, so the model it named gets no token series — asserted over
/// every model of the seeded inventory the `Converse` API does not serve, plus a
/// well-formed id naming no model at all.
#[tokio::test]
async fn a_call_that_served_no_inference_publishes_no_token_series() {
    let router = emulator();
    let opened = now_secs();

    // One served call, so the namespace exists and an absent series below is the
    // refusal's doing rather than an empty store.
    let served = Row {
        case: "the one served call",
        model_id: "amazon.nova-lite-v1:0",
        surface: Surface::Converse {
            messages: json!([{"role": "user", "content": [{"text": "Define idempotency."}]}]),
        },
    };
    let (served_input, _) = reported_usage(&router, &served).await;

    let body = json!({"messages": [{"role": "user", "content": [{"text": "Reply with PONG."}]}]});
    let mut refused: Vec<String> = vec![NEVER_INVOKED.to_string()];
    for (model_id, entry) in &ServiceState::default().models {
        if entry.supports_converse {
            continue;
        }
        refused.push(model_id.clone());
    }
    assert!(
        refused.len() > 2,
        "the inventory must offer more than one model the Converse API does not serve, got \
         {refused:?}"
    );
    for model_id in &refused {
        let call = post_json(&router, &format!("/model/{model_id}/converse"), &body).await;
        assert_ne!(
            call.status,
            200,
            "case {model_id:?}: the Converse API does not serve this model, so the call must be \
             refused, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
    }

    let start = opened - WINDOW_SECS;
    let end = now_secs() + WINDOW_SECS;
    for model_id in &refused {
        for metric in [INPUT_TOKEN_COUNT, OUTPUT_TOKEN_COUNT] {
            let points = statistics_of(&router, metric, model_id, start, end, &["Sum"]).await;
            assert!(
                points.is_empty(),
                "case {model_id:?}: a call that served no inference must publish no {metric} \
                 datapoints, got {points:?}"
            );
        }
    }

    // The refusals changed nothing about the one series that does exist.
    let actual = total(
        &statistics_of(
            &router,
            INPUT_TOKEN_COUNT,
            served.model_id,
            start,
            end,
            &["Sum"],
        )
        .await,
    );
    assert!(
        (actual - served_input).abs() < SAME_COUNT,
        "the served call's series must still account for its {served_input} tokens, got {actual}"
    );
}
