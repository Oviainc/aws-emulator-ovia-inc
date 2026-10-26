// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch metric data a SERVED inference publishes.
//!
//! Amazon Bedrock reports an invocation's token volume twice. Once inline, in
//! the response the caller gets back (`TokenUsage$inputTokens`, "The number of
//! tokens sent in the request to the model", and `TokenUsage$outputTokens`,
//! "The number of tokens that the model generated for the request"), and once
//! as metric data in the `AWS/Bedrock` namespace, where "Amazon Bedrock
//! supports the following dimensions: ModelId - all metrics" ("Runtime metrics"
//! in the Amazon Bedrock monitoring documentation) lists `InputTokenCount` and
//! `OutputTokenCount` among the per-invocation metrics. Both readings are the
//! same accounting, so a call contributes exactly the counts it reported inline
//! and nothing else: a client that joins an invocation to its metered usage
//! after the fact finds the series the response described.
//!
//! The same reference lists the REQUEST metrics every served invocation
//! contributes beside its token volume: `Invocations`, in `Count`, is the
//! "number of successful requests to the Converse, ConverseStream, InvokeModel,
//! and InvokeModelWithResponseStream API operations", and `InvocationLatency`,
//! in `Milliseconds`, the latency of those requests. One call is one successful
//! request, so it contributes exactly one observation of each: the volume a
//! client counts and the duration it measures are the call that was actually
//! served, timed on the server side, which is the same measurement the
//! surfaces reporting a latency inline (`ConverseMetrics$latencyMs`, the
//! `invocationLatency` of an invoke stream's metrics event) hand back.
//!
//! A call that is REFUSED is not a successful request, so it publishes nothing
//! here: publication is reached only once an inference has been served, after
//! every refusal this service makes.
//!
//! The datapoints go into the CloudWatch emulator's own metric store — the SAME
//! store `PutMetricData` writes and `GetMetricStatistics` / `ListMetrics` read —
//! reached through the shared engine, so one metric has one home and the
//! publishers cannot drift.
//!
//! `StartAsyncInvoke` does NOT publish here: it accepts a job rather than
//! serving an inference, reports no `usage` and no latency of its own, and the
//! job it records settles with nothing generated, so there is no token volume
//! for that call to account for and no served request for it to count. The
//! request metrics name the four synchronous and streaming invocation
//! operations, and those are the ones that publish.

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

use super::InvocationLogEntry;

/// The namespace Amazon Bedrock publishes its runtime metrics into.
pub const NAMESPACE: &str = "AWS/Bedrock";

/// The dimension every `AWS/Bedrock` metric carries: the id of the model the
/// call ran inference against.
pub const MODEL_ID_DIMENSION: &str = "ModelId";

/// The metric carrying the tokens a call sent to the model.
pub const INPUT_TOKEN_COUNT: &str = "InputTokenCount";

/// The metric carrying the tokens the model generated for a call.
pub const OUTPUT_TOKEN_COUNT: &str = "OutputTokenCount";

/// The metric carrying the successful requests an invocation surface served.
pub const INVOCATIONS: &str = "Invocations";

/// The metric carrying how long serving one of those requests took.
pub const INVOCATION_LATENCY: &str = "InvocationLatency";

/// The unit a count is observed in: a token count and a request count are
/// counts, so neither is a time nor a size.
pub const COUNT_UNIT: &str = "Count";

/// The unit a latency is observed in: `InvocationLatency` is published in
/// milliseconds, the granularity this service measures and reports a served
/// invocation's duration in.
pub const MILLISECONDS_UNIT: &str = "Milliseconds";

/// The requests one served call is: a call serves one inference, so it
/// contributes exactly one successful request to the `Invocations` series.
const ONE_REQUEST: f64 = 1.0;

/// The resolution these metrics are stored at, in seconds — the regular
/// resolution a metric has unless its publisher asks for the high-resolution
/// one ("This field is optional, if you do not specify it the default of 60 is
/// used", `MetricDatum$StorageResolution`), which is the 1-minute aggregation
/// the Bedrock runtime metrics are published at.
pub const STORAGE_RESOLUTION_SECS: i32 = 60;

/// Publish what one served call contributes to the `AWS/Bedrock` namespace: the
/// token volume `entry` accounts for, the one successful request it is, and
/// `latency_ms`, the whole milliseconds it took to serve (as
/// [`super::latency_ms`] measures every latency this service reports).
///
/// One datapoint per metric per call, dimensioned on the invoked model id alone,
/// stamped with the instant the call was served: the dimension is part of a
/// metric's identity, so a model that was never invoked has no series at all
/// rather than a share of an aggregate.
///
/// Nothing is published when the CloudWatch store is not mounted beside this
/// service: a datapoint has one home and this path never makes a second one.
/// Call it with no Bedrock Runtime state guard held, so the two services' stores
/// are never locked at once. A store at its series capacity takes no new series;
/// the inference it came from is not failed for that, because the call itself
/// was served.
pub fn publish_served_invocation<Eng: Send + Sync + 'static>(
    engine: &Eng,
    entry: &InvocationLogEntry,
    latency_ms: i64,
) {
    let Some(state) = cloudwatch::state::service_state(engine) else {
        return;
    };
    // CloudWatch holds a datapoint's instant as epoch SECONDS, which is the unit
    // its own readers window by; an invocation is timed in milliseconds.
    let timestamp_secs = entry.at_millis as f64 / 1_000.0;
    let mut store = state.lock();
    for (metric_name, value, unit) in [
        (INPUT_TOKEN_COUNT, f64::from(entry.input_tokens), COUNT_UNIT),
        (
            OUTPUT_TOKEN_COUNT,
            f64::from(entry.output_tokens),
            COUNT_UNIT,
        ),
        (INVOCATIONS, ONE_REQUEST, COUNT_UNIT),
        (
            INVOCATION_LATENCY,
            latency_ms as f64,
            MILLISECONDS_UNIT,
        ),
    ] {
        let key = cloudwatch::state::MetricKey {
            namespace: NAMESPACE.to_string(),
            metric_name: metric_name.to_string(),
            dimensions: [(MODEL_ID_DIMENSION.to_string(), entry.model_id.clone())]
                .into_iter()
                .collect(),
        };
        let datum = cloudwatch::state::StoredDatum {
            timestamp_secs,
            // One observation: this call's own contribution to the series.
            sample_count: 1.0,
            sum: value,
            minimum: value,
            maximum: value,
            unit: Some(unit.to_string()),
            storage_resolution: STORAGE_RESOLUTION_SECS,
        };
        if store.append_datapoints(key, vec![datum]).is_err() {
            // The metric store holds no more series; the inference it came from
            // was already served, so the call is not failed for it. The dropped
            // observation is reported where a developer can see it.
            ::tracing::warn!(
                metric = metric_name,
                model_id = entry.model_id.as_str(),
                "the CloudWatch metric store is at capacity, so this observation was dropped"
            );
        }
    }
}
