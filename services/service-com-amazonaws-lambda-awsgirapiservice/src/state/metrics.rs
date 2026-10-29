// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch metrics Lambda publishes ABOUT ITS OWN work on a function.
//!
//! A batch an event source mapping reads and hands to a function is moved with no client
//! action at all ([`super::event_source_poller`]), so the only way a client can tell "the
//! poller read an aged backlog" from "nothing was read" is the series Lambda publishes
//! about itself. It goes to the [`METRIC_NAMESPACE`] namespace, dimensioned on the
//! function the batch was handed to ([`FUNCTION_NAME_DIMENSION`]).
//!
//! A mapping's own work is reported per MAPPING rather than per function: how many events
//! its poller took out of the source, how many its filter criteria rejected, how many
//! reached the function and how many it deleted from the source are quantities of that one
//! mapping ([`EVENT_SOURCE_MAPPING_UUID_DIMENSION`]), and a client asks for them with the
//! mapping's own `MetricsConfig` ([`publishes_event_counts`]).
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
//!
//! What a series is identified by and what one observation of it is are this module's
//! ([`series`], [`observation`], [`append`]), so every metric this service publishes —
//! the per-function ones here and the account-level concurrency metering in
//! [`super::concurrency_metering`] — reaches the store the same way and carries the same
//! resolution. An account-level series carries NO dimension: "a metric is uniquely defined
//! by a name, a namespace, and zero or more dimensions", and the quantity such a series
//! measures belongs to the Region rather than to one function.

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

/// The dimension a series about ONE EVENT SOURCE MAPPING's own work is reported per: that
/// mapping's `UUID`, which is "The identifier of the event source mapping". The work such a
/// series measures belongs to the mapping rather than to the function it invokes — several
/// mappings can invoke one function — so the mapping is what identifies it.
pub const EVENT_SOURCE_MAPPING_UUID_DIMENSION: &str = "EventSourceMappingUUID";

/// The `EventSourceMappingMetric` a mapping's `MetricsConfig` turns the event-count family
/// on with: "`EventCount` to receive metrics related to the number of events processed by
/// your event source mapping" (`EventSourceMappingMetricsConfig$Metrics`).
pub const EVENT_COUNT_METRICS_SELECTOR: &str = "EventCount";

/// How many events a mapping's poller took out of its event source.
pub const POLLED_EVENT_COUNT_METRIC: &str = "PolledEventCount";

/// How many of the polled events the mapping's `FilterCriteria` rejected — "the filter
/// criteria that determine whether Lambda should process an event".
pub const FILTERED_OUT_EVENT_COUNT_METRIC: &str = "FilteredOutEventCount";

/// How many of the polled events were handed to the function in an invocation it accepted.
pub const INVOKED_EVENT_COUNT_METRIC: &str = "InvokedEventCount";

/// How many events the mapping's poller deleted from its event source.
pub const DELETED_EVENT_COUNT_METRIC: &str = "DeletedEventCount";

/// How long the newest record of a batch had been sitting in the shard when the event
/// source mapping read that batch.
///
/// The name is the one Lambda's own series carries in [`METRIC_NAMESPACE`]: `IteratorAge`,
/// "the age of the last record in the event" for an event source mapping that reads from a
/// stream. It is NOT the name the stream service uses for the same quantity in its own
/// namespace — a series is identified by its namespace, its name and its dimensions
/// together, so a client reading Lambda's backlog age asks for Lambda's name and finds
/// nothing under any other.
pub const ITERATOR_AGE_METRIC: &str = "IteratorAge";

/// The unit that age carries: it is a span of milliseconds, which CloudWatch's
/// `StandardUnit` spells `Milliseconds`.
pub const ITERATOR_AGE_UNIT: &str = "Milliseconds";

/// The `StorageResolution` these datapoints are published at, in seconds: regular
/// resolution, "which CloudWatch stores at 1-minute resolution"
/// (`MetricDatum$StorageResolution`).
pub const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// The `StandardUnit` a bare count of things carries: `Count`.
pub const COUNT_UNIT: &str = "Count";

/// The series one of Lambda's own metrics lives in: the [`METRIC_NAMESPACE`] namespace, the
/// metric's name, and the dimension set that identifies it — empty for a series measuring
/// something that belongs to the Region rather than to one function.
#[must_use]
pub fn series(metric: &str, dimensions: cloudwatch::DimensionSet) -> cloudwatch::MetricKey {
    cloudwatch::MetricKey {
        namespace: METRIC_NAMESPACE.to_string(),
        metric_name: metric.to_string(),
        dimensions,
    }
}

/// The dimension set of a series reported per function: the function the work was done for.
#[must_use]
pub fn function_dimensions(function_name: &str) -> cloudwatch::DimensionSet {
    [(
        FUNCTION_NAME_DIMENSION.to_string(),
        function_name.to_string(),
    )]
    .into_iter()
    .collect()
}

/// The dimension set of a series reported per event source mapping: the mapping whose
/// poller did the work.
#[must_use]
pub fn mapping_dimensions(uuid: &str) -> cloudwatch::DimensionSet {
    [(
        EVENT_SOURCE_MAPPING_UUID_DIMENSION.to_string(),
        uuid.to_string(),
    )]
    .into_iter()
    .collect()
}

/// What ONE pass of a mapping's poller did with the events it took out of the event
/// source — each count a tally of that pass's own work, never a running total kept here:
/// the totals a client reads are the SUM CloudWatch computes over the datapoints.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EventCounts {
    /// How many events the pass took out of the event source.
    pub polled: u32,
    /// How many of them the mapping's `FilterCriteria` rejected.
    pub filtered_out: u32,
    /// How many of them an invocation of the mapping's function accepted.
    pub invoked: u32,
    /// How many of them the pass deleted from the event source.
    pub deleted: u32,
}

/// Whether `metrics_config` selects the event-count family, which is the documented switch
/// for it: "the metrics you want your event source mapping to produce". A mapping carrying
/// no `MetricsConfig`, or one whose `Metrics` list does not hold
/// [`EVENT_COUNT_METRICS_SELECTOR`] — an emptied list included — produces none of them.
#[must_use]
pub fn publishes_event_counts(
    metrics_config: Option<&crate::ops::types::EventSourceMappingMetricsConfig>,
) -> bool {
    metrics_config
        .and_then(|config| config.metrics.as_ref())
        .is_some_and(|metrics| {
            metrics
                .iter()
                .any(|metric| metric == EVENT_COUNT_METRICS_SELECTOR)
        })
}

/// ONE measurement of a series, taken at `at_secs` (epoch seconds), published at the
/// regular storage resolution so it is aggregated in the minute it was taken in.
#[must_use]
pub fn observation(value: f64, unit: &str, at_secs: f64) -> cloudwatch::StoredDatum {
    cloudwatch::StoredDatum {
        timestamp_secs: at_secs,
        sample_count: 1.0,
        sum: value,
        minimum: value,
        maximum: value,
        unit: Some(unit.to_string()),
        storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
    }
}

/// Store one observation of the series `key` names, and make every alarm watching that
/// series due for evaluation — exactly as an ingested `PutMetricData` datapoint does, so an
/// alarm cannot tell a service's own metric from a client's.
///
/// Answers whether it was stored: a store already holding its full complement of series
/// drops the observation rather than failing the work it is about, because a metric is
/// published off the response path.
pub fn append(
    store: &::engine::ServiceState<CloudWatchState>,
    key: cloudwatch::MetricKey,
    datum: cloudwatch::StoredDatum,
) -> bool {
    let watched = cloudwatch::WrittenSeries {
        key: key.clone(),
        units: vec![datum.unit().to_string()],
    };
    let at_secs = datum.timestamp_secs;
    let mut store = store.lock();
    if store.append_datapoints(key, vec![datum]).is_err() {
        return false;
    }
    store.make_alarms_due(&[watched], at_secs);
    true
}

/// The publisher of the metrics Lambda publishes about its own work — on the function a
/// batch is handed to and on the event source mapping that polled it — held by the poller
/// doing that work.
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

    /// Publish what ONE pass of the event source mapping `uuid` names did with the events
    /// it polled, at `at_secs` (epoch seconds): the whole event-count family, one
    /// observation of each, so a reader summing a period over any one of them gets the
    /// events of that period.
    ///
    /// Every count is published, a zero included: "the number of events" a pass filtered
    /// out, invoked the function with or deleted is as much a measurement when it is none
    /// as when it is many, and a series that skipped its zeros would leave a client unable
    /// to tell a poller that did nothing from one that is not publishing at all.
    pub fn publish_event_counts(&self, uuid: &str, counts: &EventCounts, at_secs: f64) {
        for (metric, value) in [
            (POLLED_EVENT_COUNT_METRIC, counts.polled),
            (FILTERED_OUT_EVENT_COUNT_METRIC, counts.filtered_out),
            (INVOKED_EVENT_COUNT_METRIC, counts.invoked),
            (DELETED_EVENT_COUNT_METRIC, counts.deleted),
        ] {
            self.publish_series(
                metric,
                mapping_dimensions(uuid),
                f64::from(value),
                COUNT_UNIT,
                at_secs,
                uuid,
            );
        }
    }

    /// Publish that the mapping `uuid` names polled NOTHING: one 0-valued
    /// [`POLLED_EVENT_COUNT_METRIC`] observation at `at_secs` (epoch seconds).
    ///
    /// This is what makes an empty window mean "nothing was there to poll" rather than
    /// "the poller is not publishing": a client reading a zero knows the mapping polled and
    /// found its source empty.
    pub fn publish_idle_poll(&self, uuid: &str, at_secs: f64) {
        self.publish_series(
            POLLED_EVENT_COUNT_METRIC,
            mapping_dimensions(uuid),
            0.0,
            COUNT_UNIT,
            at_secs,
            uuid,
        );
    }

    /// Append one observation of `metric` for one function.
    fn publish(&self, metric: &str, function_name: &str, value: f64, unit: &str) {
        self.publish_series(
            metric,
            function_dimensions(function_name),
            value,
            unit,
            cloudwatch::now_secs(),
            function_name,
        );
    }

    /// Append one observation of the series `metric` and `dimensions` identify, taken at
    /// `at_secs`. `about` is the resource the observation is of, named in the one log line
    /// a store at capacity produces.
    fn publish_series(
        &self,
        metric: &str,
        dimensions: cloudwatch::DimensionSet,
        value: f64,
        unit: &str,
        at_secs: f64,
        about: &str,
    ) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        if !append(store, series(metric, dimensions), observation(value, unit, at_secs)) {
            ::tracing::warn!(
                resource = %about,
                metric = %metric,
                "the CloudWatch store is at capacity; a Lambda metric could not be published"
            );
        }
    }
}
