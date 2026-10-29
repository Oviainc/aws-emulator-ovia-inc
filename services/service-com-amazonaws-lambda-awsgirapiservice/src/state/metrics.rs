// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch metrics Lambda publishes ABOUT ITS OWN work on a function.
//!
//! A batch an event source mapping reads and hands to a function is moved with no client
//! action at all ([`super::event_source_poller`]), so the only way a client can tell "the
//! poller read an aged backlog" from "nothing was read" is the series Lambda publishes
//! about itself. It goes to the [`METRIC_NAMESPACE`] namespace, dimensioned on the
//! function the batch was handed to ([`FUNCTION_NAME_DIMENSION`]).
//!
//! The datapoints are appended to the CloudWatch store directly rather than sent through
//! `PutMetricData`: CloudWatch reserves the `AWS/` namespaces for the services that own
//! them ("you should not specify a namespace that begins with `AWS/`",
//! `PutMetricData$Namespace`), so a service's own metric never travels over that API. It
//! goes into the very store `ListMetrics`, `GetMetricStatistics` and `GetMetricData` read,
//! through CloudWatch's own bounded append, so one metric and its reader cannot disagree.
//!
//! One observation is published per BATCH READ, carrying what that read measured, at the
//! regular storage resolution — so a reader laying one-minute periods over the series sees
//! the batches read in each minute, and `Maximum` over a minute is the oldest data any
//! batch of that minute carried.

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

/// The CloudWatch store Lambda's own metrics live in.
///
/// It is the CloudWatch service's OWN state struct, reached through the SAME shared engine
/// this service's store lives in, so what Lambda publishes is what `ListMetrics` and
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

/// The namespace Lambda publishes its own metrics to.
pub const METRIC_NAMESPACE: &str = "AWS/Lambda";

/// The dimension they are reported per: the function the work was done for.
pub const FUNCTION_NAME_DIMENSION: &str = "FunctionName";

/// How long the newest record of a batch had been sitting in the shard when the event
/// source mapping read that batch.
pub const ITERATOR_AGE_METRIC: &str = "IteratorAgeMilliseconds";

/// The unit that age carries: it is a span of milliseconds, which CloudWatch's
/// `StandardUnit` spells `Milliseconds`.
pub const ITERATOR_AGE_UNIT: &str = "Milliseconds";

/// The `StorageResolution` these datapoints are published at, in seconds: regular
/// resolution, "which CloudWatch stores at 1-minute resolution"
/// (`MetricDatum$StorageResolution`).
const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// The publisher of ONE function's metrics, held by the poller that moves batches to it.
///
/// It holds no counts of its own: every observation is a datapoint in the CloudWatch
/// store, never a second copy kept here.
pub struct FunctionMetrics {
    /// The store the datapoints are appended to, or `None` when the engine carries no
    /// CloudWatch.
    store: Option<::engine::ServiceState<CloudWatchState>>,
}

impl ::std::fmt::Debug for FunctionMetrics {
    /// Whether this publisher has a store to append to, which is all of it that is not
    /// that store (a store handle has no rendering of its own).
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("FunctionMetrics")
            .field("published", &self.store.is_some())
            .finish()
    }
}

impl FunctionMetrics {
    /// The publisher appending to `store`.
    #[must_use]
    pub fn new(store: Option<::engine::ServiceState<CloudWatchState>>) -> Self {
        Self { store }
    }

    /// Publish the iterator age ONE batch read for the function named `function_name`
    /// measured: `age_millis` is how long the newest record of that batch had been in the
    /// shard when it was read.
    pub fn publish_iterator_age(&self, function_name: &str, age_millis: f64) {
        self.publish(ITERATOR_AGE_METRIC, function_name, age_millis, ITERATOR_AGE_UNIT);
    }

    /// Append one observation of `metric` for one function.
    fn publish(&self, metric: &str, function_name: &str, value: f64, unit: &str) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let key = cloudwatch::MetricKey {
            namespace: METRIC_NAMESPACE.to_string(),
            metric_name: metric.to_string(),
            dimensions: [(
                FUNCTION_NAME_DIMENSION.to_string(),
                function_name.to_string(),
            )]
            .into_iter()
            .collect(),
        };
        let datum = cloudwatch::StoredDatum {
            timestamp_secs: cloudwatch::now_secs(),
            sample_count: 1.0,
            sum: value,
            minimum: value,
            maximum: value,
            unit: Some(unit.to_string()),
            storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
        };
        if store.lock().append_datapoints(key, vec![datum]).is_err() {
            ::tracing::warn!(
                function = %function_name,
                metric = %metric,
                "the CloudWatch store is at capacity; a Lambda metric could not be published"
            );
        }
    }
}
