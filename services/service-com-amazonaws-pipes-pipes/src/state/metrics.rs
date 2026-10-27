// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch metrics a RUNNING pipe publishes about its OWN executions.
//!
//! A pipe's stages are run with no client action at all ([`super::data_plane`]), so the
//! only way a client can tell "the pipe ran and a stage refused it" from "the pipe never
//! ran" is the series the runtime publishes about itself. EventBridge Pipes publishes
//! them to the [`METRIC_NAMESPACE`] namespace, dimensioned on the pipe's name
//! ([`PIPE_NAME_DIMENSION`]), at one-minute resolution, with the counters carrying the
//! unit [`COUNTER_UNIT`] (EventBridge Pipes monitoring) — and each counter reads 0 for a
//! minute in which nothing of its kind happened rather than being absent, which is why
//! [`PipeMetrics::open_window`] publishes the whole counter set at 0 as it enters each
//! minute: a quiet pipe's series EXISTS and reads 0.
//!
//! The datapoints are appended to the CloudWatch store directly rather than sent through
//! `PutMetricData`: CloudWatch reserves the `AWS/` namespaces for the services that own
//! them ("you should not specify a namespace that begins with `AWS/`",
//! `PutMetricData$Namespace`), so a service's own metric never travels over that API. It
//! goes into the very store `ListMetrics` and `GetMetricStatistics` read, through
//! CloudWatch's own bounded append, so one metric and its reader cannot disagree.
//!
//! One observation is published per EXECUTION (per batch the poller moves), carrying what
//! that execution contributed, at the regular storage resolution — so the series a reader
//! lays one-minute periods over is the per-minute series the service publishes: `Sum` over
//! a minute is what happened in it, and a minute nothing happened in reads the 0 the
//! window opened with.

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

/// The CloudWatch store a pipe's own metrics live in.
///
/// It is the CloudWatch service's OWN state struct, reached through the SAME shared engine
/// this service's store lives in, so what a pipe publishes is what `ListMetrics` and
/// `GetMetricStatistics` report.
pub type CloudWatchState =
    ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::ComAmazonawsCloudwatchGraniteserviceversion20100801State;

/// The CloudWatch store in the SAME shared engine. `None` means the engine carries no
/// such store — a deployment with no CloudWatch to publish to.
pub fn cloudwatch_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<CloudWatchState>> {
    ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state::service_state(engine)
}

/// The namespace EventBridge Pipes publishes its own metrics to.
pub const METRIC_NAMESPACE: &str = "AWS/EventBridge/Pipes";

/// The dimension those metrics are reported per: the pipe they are about.
pub const PIPE_NAME_DIMENSION: &str = "PipeName";

/// The unit a pipe's COUNTERS carry. Not `Count`: EventBridge Pipes publishes its
/// counters with no unit of measure, which CloudWatch spells `None`
/// (`StandardUnit`'s `None` member).
pub const COUNTER_UNIT: &str = "None";

/// The events a pipe's executions took from their source.
pub const EVENT_COUNT_METRIC: &str = "EventCount";

/// The executions that failed, at whichever stage failed them.
pub const EXECUTION_FAILED_METRIC: &str = "ExecutionFailed";

/// The executions that failed AT THE TARGET STAGE: the pipe ran and the target did not
/// take what it was handed.
pub const TARGET_STAGE_FAILED_METRIC: &str = "TargetStageFailed";

/// The executions that failed AT THE ENRICHMENT STAGE, the stage counter's sibling for
/// the other stage a pipe hands its events to before the target.
pub const ENRICHMENT_STAGE_FAILED_METRIC: &str = "EnrichmentStageFailed";

/// Every counter a pipe publishes. A RUNNING pipe publishes all of them for every minute
/// it is running, so each exists and reads 0 for a minute in which nothing of its kind
/// happened.
pub const PIPE_COUNTERS: &[&str] = &[
    EVENT_COUNT_METRIC,
    EXECUTION_FAILED_METRIC,
    TARGET_STAGE_FAILED_METRIC,
    ENRICHMENT_STAGE_FAILED_METRIC,
];

/// The `StorageResolution` these datapoints are published at, in seconds: regular
/// resolution, "which CloudWatch stores at 1-minute resolution"
/// (`MetricDatum$StorageResolution`) — the minute a pipe's metrics are published per.
const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// How long the window one set of counters is published for lasts, in seconds: the minute
/// EventBridge Pipes publishes per.
const PUBLICATION_WINDOW_SECONDS: f64 = 60.0;

/// The publisher of ONE pipe's metrics, held by that pipe's poller for as long as it
/// runs.
///
/// It remembers the minute whose counters it has already opened, which is all the state a
/// per-minute series needs: the counts themselves are datapoints in the CloudWatch store,
/// never a second copy held here.
pub struct PipeMetrics {
    /// The pipe every datapoint is dimensioned on.
    pipe: String,
    /// The store the datapoints are appended to, or `None` when the engine carries no
    /// CloudWatch.
    store: Option<::engine::ServiceState<CloudWatchState>>,
    /// The minute (as epoch seconds divided by the window) whose counter set has already
    /// been opened at 0.
    opened_window: Option<i64>,
}

impl ::std::fmt::Debug for PipeMetrics {
    /// The pipe and the window, which is all of this publisher that is not the store it
    /// appends to (a store handle has no rendering of its own).
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("PipeMetrics")
            .field("pipe", &self.pipe)
            .field("published", &self.store.is_some())
            .field("opened_window", &self.opened_window)
            .finish()
    }
}

impl PipeMetrics {
    /// The publisher for the pipe named `pipe`, appending to `store`.
    pub fn new(pipe: &str, store: Option<::engine::ServiceState<CloudWatchState>>) -> Self {
        Self {
            pipe: pipe.to_string(),
            store,
            opened_window: None,
        }
    }

    /// Open the minute the clock is now in, if it is not already open: publish every
    /// counter at 0, so the series exists and reads 0 for a minute in which nothing of
    /// its kind happens.
    ///
    /// Called by the poller on each pass it is RUNNING for, so a pipe publishes for as
    /// long as it runs and stops publishing when it stops running.
    pub fn open_window(&mut self) {
        let window = (cloudwatch::now_secs() / PUBLICATION_WINDOW_SECONDS).floor() as i64;
        if self.opened_window == Some(window) {
            return;
        }
        self.opened_window = Some(window);
        for metric in PIPE_COUNTERS {
            self.publish(metric, 0.0);
        }
    }

    /// Count what ONE execution contributed to `metric`: an observation of `observations`
    /// at the instant the execution came to it.
    ///
    /// Nothing is published for an execution that contributed nothing — the minute's own
    /// 0 is what such a minute reads.
    pub fn count(&self, metric: &str, observations: usize) {
        if observations == 0 {
            return;
        }
        // A count past f64's exact integers cannot arise: one batch holds at most the
        // records the data plane bounds it to.
        self.publish(metric, observations as f64);
    }

    /// Append one observation of `metric`.
    fn publish(&self, metric: &str, value: f64) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let key = cloudwatch::MetricKey {
            namespace: METRIC_NAMESPACE.to_string(),
            metric_name: metric.to_string(),
            dimensions: [(PIPE_NAME_DIMENSION.to_string(), self.pipe.clone())]
                .into_iter()
                .collect(),
        };
        let datum = cloudwatch::StoredDatum {
            timestamp_secs: cloudwatch::now_secs(),
            sample_count: 1.0,
            sum: value,
            minimum: value,
            maximum: value,
            unit: Some(COUNTER_UNIT.to_string()),
            storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
        };
        if store.lock().append_datapoints(key, vec![datum]).is_err() {
            ::tracing::warn!(
                pipe = %self.pipe,
                metric = %metric,
                "the CloudWatch store is at capacity; a pipe's own metric could not be published"
            );
        }
    }
}
