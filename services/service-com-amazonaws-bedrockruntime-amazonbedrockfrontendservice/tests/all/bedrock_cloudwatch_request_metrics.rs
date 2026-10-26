// SPDX-License-Identifier: CC0-1.0

//! The `AWS/Bedrock` REQUEST metrics a served invocation publishes, asserted
//! where a client reads them: through CloudWatch's own `GetMetricStatistics` and
//! `ListMetrics`, with both services mounted on one shared engine exactly as
//! `core/router` assembles them.
//!
//! RULE under test: a Bedrock Runtime call that SERVES an inference contributes
//! exactly one `Invocations` observation, in `Count`, and one
//! `InvocationLatency` observation, in `Milliseconds` of the duration measured
//! for serving it, to the `AWS/Bedrock` series dimensioned on the invoked model
//! id alone — so the request volume and the latency a client counts are the
//! calls that were actually served, a call that was REFUSED contributes
//! neither, and a model nothing invoked has no series at all.
//!
//! The table below spans what the rule generalises over: the invocation surface
//! (`Converse`, `ConverseStream`, `InvokeModel`,
//! `InvokeModelWithResponseStream`), the model and its response family, and how
//! many calls the client makes. The recorded case — three `Converse` calls on
//! one model — is ONE row.
//!
//! The namespace, the dimension, the metric names and the inference surfaces are
//! shared with the suite that asserts the TOKEN metrics of the same publication
//! ([`crate::bedrock_cloudwatch_token_metrics`]), so the two read one definition
//! of which series is meant.

use serde_json::{json, Value};
// The metric names, namespace and dimension under test, read from this service's
// own publication rule so the suite and the handler cannot disagree.
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::invocation_metrics::{
    COUNT_UNIT, INVOCATIONS, INVOCATION_LATENCY, MILLISECONDS_UNIT, MODEL_ID_DIMENSION, NAMESPACE,
};
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState as ServiceState;
use smithy_server_runtime::MultiServiceRouter;

// The `StartAsyncInvoke` request body of the async suite, and the Bedrock
// Runtime request walk of the inference suite: reached through the SAME
// definitions rather than a second copy of them.
use crate::async_invoke_model_contract::{start_request, valid_model_input};
use crate::bedrock_cloudwatch_token_metrics::{
    cloudwatch, emulator, now_secs, reported_usage, rows, statistics_of, total, Row, Surface,
    PERIOD_SECS_F64, WINDOW_SECS,
};
use crate::bedrock_runtime_inference::post_json;

/// How many calls a row's client makes, by the row's position in the table: one,
/// two and three calls, so the series is asserted as a count of the calls made
/// rather than as the presence of one datapoint.
const CALL_COUNTS: [usize; 3] = [1, 2, 3];

/// Milliseconds in a second, the conversion between the wall clock a run is
/// measured on and the unit `InvocationLatency` is published in.
const MILLIS_PER_SEC: f64 = 1_000.0;

/// One `Converse` request body, the shape every refusal row below sends.
fn converse_body() -> Value {
    json!({"messages": [{"role": "user", "content": [{"text": "Reply with PONG."}]}]})
}

/// The statistic `name` of every datapoint of a span, summed: `Sum` over a span
/// of one-observation datapoints is the volume, `SampleCount` the number of
/// observations.
fn folded(points: &[Value], name: &str) -> f64 {
    points
        .iter()
        .filter_map(|point| point.get(name).and_then(Value::as_f64))
        .sum()
}

/// The largest `name` any datapoint of the span reports, or `None` over no
/// datapoint at all.
fn largest(points: &[Value], name: &str) -> Option<f64> {
    points
        .iter()
        .filter_map(|point| point.get(name).and_then(Value::as_f64))
        .fold(None, |best: Option<f64>, value| {
            Some(best.map_or(value, |best| best.max(value)))
        })
}

/// The smallest `name` any datapoint of the span reports, or `None` over no
/// datapoint at all.
fn smallest(points: &[Value], name: &str) -> Option<f64> {
    points
        .iter()
        .filter_map(|point| point.get(name).and_then(Value::as_f64))
        .fold(None, |best: Option<f64>, value| {
            Some(best.map_or(value, |best| best.min(value)))
        })
}

/// Every datapoint of the span carries `unit`, which is the unit the metric is
/// published in.
fn assert_unit(case: &str, metric: &str, unit: &str, points: &[Value]) {
    for point in points {
        assert_eq!(
            point.get("Unit").and_then(Value::as_str),
            Some(unit),
            "case {case:?}: {metric} must be published in {unit}, got {point:?}"
        );
    }
}

/// Every datapoint of the span falls inside the window the read asked for.
///
/// A read's windows are anchored at the requested start time rounded DOWN to the
/// minute, so a datapoint's own stamp may precede `start` by at most one period.
fn assert_in_window(case: &str, metric: &str, points: &[Value], start: f64, end: f64) {
    for point in points {
        let stamp = point
            .get("Timestamp")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: a {metric} datapoint carried no Timestamp"));
        assert!(
            (start - PERIOD_SECS_F64..=end).contains(&stamp),
            "case {case:?}: {metric} datapoint {stamp} must fall inside the requested window \
             {start}..={end}"
        );
    }
}

/// `ListMetrics` surfaces `metric` dimensioned on exactly the invoked model id.
async fn assert_listed_for_model(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    model_id: &str,
) {
    let listed = cloudwatch(
        router,
        "ListMetrics",
        &json!({"Namespace": NAMESPACE, "MetricName": metric}),
    )
    .await;
    assert_eq!(
        listed.get("Metrics"),
        Some(&json!([{
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": [{"Name": MODEL_ID_DIMENSION, "Value": model_id}],
        }])),
        "case {case:?}: ListMetrics must surface {metric} dimensioned on exactly the invoked \
         model, got {listed}"
    );
}

/// RULE: every served call contributes exactly one `Invocations` observation in
/// `Count` and one `InvocationLatency` observation in `Milliseconds`, so over a
/// run of N calls the `Invocations` series sums to N with no observation above
/// one, the latency series carries N positive observations bounded by the run's
/// own elapsed wall time, and both are surfaced under the invoked model id
/// alone.
#[tokio::test]
async fn every_served_invocation_publishes_one_count_and_one_latency_in_milliseconds() {
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let calls = CALL_COUNTS[index % CALL_COUNTS.len()];
        #[allow(clippy::cast_precision_loss)]
        let expected_calls = calls as f64;
        let router = emulator();
        let opened = now_secs();

        for _ in 0..calls {
            reported_usage(&router, &row).await;
        }

        let closed = now_secs();
        // The wall time the run itself took, in the unit the latency is
        // published in. Each observation is rounded UP to a whole millisecond
        // (the granularity this service reports a latency in), so a run of N
        // calls cannot account for more than its own elapsed wall time plus the
        // fraction of a millisecond each of those N roundings adds.
        let wall_millis = (closed - opened) * MILLIS_PER_SEC;
        let latency_ceiling = wall_millis + expected_calls;
        let start = opened - WINDOW_SECS;
        let end = closed + WINDOW_SECS;

        let counted = statistics_of(
            &router,
            INVOCATIONS,
            row.model_id,
            start,
            end,
            &["Sum", "SampleCount", "Maximum"],
        )
        .await;
        assert!(
            !counted.is_empty(),
            "case {case:?}: {calls} served call(s) must publish {INVOCATIONS} datapoints for {}, \
             got none",
            row.model_id
        );
        assert!(
            (folded(&counted, "Sum") - expected_calls).abs() < 0.5,
            "case {case:?}: {INVOCATIONS} must sum to the {calls} call(s) served, got {} from \
             {counted:?}",
            folded(&counted, "Sum")
        );
        assert!(
            (folded(&counted, "SampleCount") - expected_calls).abs() < 0.5,
            "case {case:?}: {INVOCATIONS} must carry one observation per call ({calls}), got {} \
             from {counted:?}",
            folded(&counted, "SampleCount")
        );
        assert!(
            largest(&counted, "Maximum").is_some_and(|max| (max - 1.0).abs() < 0.5),
            "case {case:?}: one call is one successful request, so no {INVOCATIONS} observation \
             may exceed 1, got {:?} from {counted:?}",
            largest(&counted, "Maximum")
        );
        assert_unit(case, INVOCATIONS, COUNT_UNIT, &counted);
        assert_in_window(case, INVOCATIONS, &counted, start, end);

        let timed = statistics_of(
            &router,
            INVOCATION_LATENCY,
            row.model_id,
            start,
            end,
            &["Sum", "SampleCount", "Maximum", "Minimum"],
        )
        .await;
        assert!(
            (folded(&timed, "SampleCount") - expected_calls).abs() < 0.5,
            "case {case:?}: {INVOCATION_LATENCY} must carry one observation per call ({calls}), \
             got {} from {timed:?}",
            folded(&timed, "SampleCount")
        );
        assert!(
            smallest(&timed, "Minimum").is_some_and(|min| min >= 1.0),
            "case {case:?}: serving a call takes time, so every {INVOCATION_LATENCY} observation \
             must be positive, got {:?} from {timed:?}",
            smallest(&timed, "Minimum")
        );
        assert!(
            folded(&timed, "Sum") <= latency_ceiling,
            "case {case:?}: {INVOCATION_LATENCY} must account for at most the {latency_ceiling} \
             ms the run took, got {} from {timed:?}",
            folded(&timed, "Sum")
        );
        assert_unit(case, INVOCATION_LATENCY, MILLISECONDS_UNIT, &timed);
        assert_in_window(case, INVOCATION_LATENCY, &timed, start, end);

        for metric in [INVOCATIONS, INVOCATION_LATENCY] {
            assert_listed_for_model(&router, case, metric, row.model_id).await;
        }
    }
}

/// RULE (the dimension side of the same rule): the model id is part of the
/// metric's identity rather than aggregated away, so a well-formed model id
/// nothing invoked has neither datapoints nor a listed series.
#[tokio::test]
async fn a_model_that_was_never_invoked_has_no_request_series() {
    // Models the registry serves, so the only thing separating them here is
    // which one a call actually invoked.
    const INVOKED: &str = "amazon.nova-lite-v1:0";
    const NEVER_INVOKED: &[&str] = &[
        "amazon.titan-text-premier-v1:0",
        "meta.llama3-8b-instruct-v1:0",
        "anthropic.claude-3-haiku-20240307-v1:0",
        // A well-formed id naming no model at all, as a client inventing a
        // dimension value would send.
        "example.no-such-model-v1:0",
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
    reported_usage(&router, &row).await;
    let start = opened - WINDOW_SECS;
    let end = now_secs() + WINDOW_SECS;

    let invoked = statistics_of(&router, INVOCATIONS, INVOKED, start, end, &["Sum"]).await;
    assert!(
        (total(&invoked) - 1.0).abs() < 0.5,
        "the invoked model's {INVOCATIONS} series must account for its one call, got {invoked:?}"
    );

    for model_id in NEVER_INVOKED {
        for metric in [INVOCATIONS, INVOCATION_LATENCY] {
            let points = statistics_of(&router, metric, model_id, start, end, &["Sum"]).await;
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

/// RULE: `Invocations` counts the requests that SUCCEEDED, so a call this
/// service refuses contributes no observation of either request metric.
///
/// The refused rows are every model of the seeded inventory the Converse API
/// does not serve, so the rule is asserted over the whole set rather than over
/// one hand-picked id, and `StartAsyncInvoke` is a row too: it accepts a job
/// instead of serving an inference, and the request metrics are published for
/// the synchronous and streaming invocation operations.
#[tokio::test]
async fn a_call_that_was_not_served_publishes_no_request_metrics() {
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
    reported_usage(&router, &served).await;

    let mut refused: Vec<String> = Vec::new();
    for (model_id, entry) in &ServiceState::default().models {
        if entry.supports_converse {
            continue;
        }
        let call = post_json(
            &router,
            &format!("/model/{model_id}/converse"),
            &converse_body(),
        )
        .await;
        assert_ne!(
            call.status,
            200,
            "case {model_id:?}: the Converse API does not serve this model, so the call must be \
             refused, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        refused.push(model_id.clone());
    }
    assert!(
        refused.len() > 1,
        "the inventory must offer more than one model the Converse API does not serve, got \
         {refused:?}"
    );

    // A job submission: accepted, and not a served inference either.
    let async_model = "amazon.nova-reel-v1:0";
    let model_input = valid_model_input(async_model)
        .unwrap_or_else(|| panic!("case {async_model:?}: no published model input"));
    let submitted = post_json(
        &router,
        "/async-invoke",
        &start_request(async_model, model_input),
    )
    .await;
    assert_eq!(
        submitted.status,
        200,
        "case {async_model:?}: StartAsyncInvoke must accept the job, got {} / {}",
        submitted.status,
        String::from_utf8_lossy(&submitted.body)
    );
    refused.push(async_model.to_string());

    let start = opened - WINDOW_SECS;
    let end = now_secs() + WINDOW_SECS;
    let counted = statistics_of(
        &router,
        INVOCATIONS,
        served.model_id,
        start,
        end,
        &["Sum", "SampleCount"],
    )
    .await;
    assert!(
        (folded(&counted, "SampleCount") - 1.0).abs() < 0.5,
        "only the one served call may be counted as an invocation, got {counted:?}"
    );

    for model_id in &refused {
        for metric in [INVOCATIONS, INVOCATION_LATENCY] {
            let points = statistics_of(&router, metric, model_id, start, end, &["Sum"]).await;
            assert!(
                points.is_empty(),
                "case {model_id:?}: a call that served no inference must publish no {metric} \
                 datapoints, got {points:?}"
            );
        }
    }
}

/// RULE: reading the series reports it without changing it — a read is not an
/// invocation, so the same window answers the same volume however often it is
/// read, and the next served call adds its own observation.
#[tokio::test]
async fn reading_the_request_series_reports_it_without_changing_it() {
    let router = emulator();
    let opened = now_secs();
    let row = Row {
        case: "one Converse turn",
        model_id: "amazon.nova-lite-v1:0",
        surface: Surface::Converse {
            messages: json!([{"role": "user", "content": [{"text": "Define idempotency."}]}]),
        },
    };

    reported_usage(&router, &row).await;
    let start = opened - WINDOW_SECS;
    let end = now_secs() + WINDOW_SECS;

    let once = total(&statistics_of(&router, INVOCATIONS, row.model_id, start, end, &["Sum"]).await);
    let twice =
        total(&statistics_of(&router, INVOCATIONS, row.model_id, start, end, &["Sum"]).await);
    assert!(
        (once - 1.0).abs() < 0.5 && (twice - 1.0).abs() < 0.5,
        "two reads of one window must both report the one call served, got {once} then {twice}"
    );

    reported_usage(&router, &row).await;
    let after = total(
        &statistics_of(
            &router,
            INVOCATIONS,
            row.model_id,
            start,
            now_secs() + WINDOW_SECS,
            &["Sum"],
        )
        .await,
    );
    assert!(
        (after - 2.0).abs() < 0.5,
        "a second served call must add its own invocation to the one already counted, got {after}"
    );
}
