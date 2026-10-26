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
//! The datapoints go into the CloudWatch emulator's own metric store — the SAME
//! store `PutMetricData` writes and `GetMetricStatistics` / `ListMetrics` read —
//! reached through the shared engine, so one metric has one home and the
//! publishers cannot drift.
//!
//! `StartAsyncInvoke` does NOT publish here: it accepts a job rather than
//! serving an inference, reports no `usage` of its own, and the job it records
//! settles with nothing generated, so there is no token volume for that call to
//! account for.

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

/// The unit a token count is observed in: the two metrics above count tokens,
/// so neither is a time nor a size.
pub const COUNT_UNIT: &str = "Count";

/// The resolution these metrics are stored at, in seconds — the regular
/// resolution a metric has unless its publisher asks for the high-resolution
/// one ("This field is optional, if you do not specify it the default of 60 is
/// used", `MetricDatum$StorageResolution`), which is the 1-minute aggregation
/// the Bedrock runtime metrics are published at.
pub const STORAGE_RESOLUTION_SECS: i32 = 60;

/// Publish the token volume `entry` accounts for as `AWS/Bedrock` metric data.
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
pub fn publish_token_usage<Eng: Send + Sync + 'static>(engine: &Eng, entry: &InvocationLogEntry) {
    let Some(state) = cloudwatch::state::service_state(engine) else {
        return;
    };
    // CloudWatch holds a datapoint's instant as epoch SECONDS, which is the unit
    // its own readers window by; an invocation is timed in milliseconds.
    let timestamp_secs = entry.at_millis as f64 / 1_000.0;
    let mut store = state.lock();
    for (metric_name, tokens) in [
        (INPUT_TOKEN_COUNT, entry.input_tokens),
        (OUTPUT_TOKEN_COUNT, entry.output_tokens),
    ] {
        let key = cloudwatch::state::MetricKey {
            namespace: NAMESPACE.to_string(),
            metric_name: metric_name.to_string(),
            dimensions: [(MODEL_ID_DIMENSION.to_string(), entry.model_id.clone())]
                .into_iter()
                .collect(),
        };
        let value = f64::from(tokens);
        let datum = cloudwatch::state::StoredDatum {
            timestamp_secs,
            // One observation: this call's own contribution to the series.
            sample_count: 1.0,
            sum: value,
            minimum: value,
            maximum: value,
            unit: Some(COUNT_UNIT.to_string()),
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
