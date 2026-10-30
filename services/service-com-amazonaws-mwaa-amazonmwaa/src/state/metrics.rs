// SPDX-License-Identifier: CC0-1.0

//! Where the environment health metrics `PublishMetrics` accepts are PUBLISHED: the Amazon
//! CloudWatch store, which is the destination the operation documents for them ("Publishes
//! environment health metrics to Amazon CloudWatch").
//!
//! A datum this service accepts is appended to the CloudWatch service's OWN store in the
//! SAME shared engine, so what a publish stores is what `ListMetrics`,
//! `GetMetricStatistics` and `GetMetricData` report. Nothing is kept here beside it: a
//! second copy on this service's own record would be a store no reader reads, and one that
//! went when the environment's teardown retired its record — while a metric outlives the
//! resource it was published about, up to the destination's own retention.
//!
//! The datapoints are appended to that store directly rather than sent through
//! `PutMetricData`: CloudWatch reserves the `AWS/` namespaces for the services that own
//! them ("you should not specify a namespace that begins with `AWS/`",
//! `PutMetricData$Namespace`), so a service's own metric never travels over that API. It
//! goes into the very store those readers read, through CloudWatch's own bounded append, so
//! one metric and its reader cannot disagree.
//!
//! A series is identified by its namespace, its metric name and its dimension set together,
//! and the environment a publish names travels in the request's URI rather than in the
//! datum — so the dimension set a datum lands under is its own PLUS
//! [`ENVIRONMENT_DIMENSION`], the dimension the Amazon MWAA metrics in
//! [`METRIC_NAMESPACE`] are reported per. Without it the same metric published for two
//! environments would be one series, and no reader could tell which environment a
//! datapoint describes.
//!
//! What a series is NOT keyed by here is the Region: the destination store holds one series
//! per namespace, metric name and dimension set for the whole engine, so two environments of
//! one name in two Regions address one series and the per-environment ceiling counts it
//! once. That is the destination's own identity for a metric, not a claim about the modelled
//! one, and it is where a Region would have to enter for this to change.

use ::std::collections::BTreeMap;
use ::std::time::{SystemTime, UNIX_EPOCH};

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

use super::{MetricDatapoint, PublishedMetric, MAX_METRIC_SERIES};

/// The CloudWatch store this service's published metrics live in.
///
/// It is the CloudWatch service's OWN state struct, reached through the SAME shared engine
/// this service's store lives in, so what a publish appends is what CloudWatch's own
/// readers report.
pub type CloudWatchState =
    ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::ComAmazonawsCloudwatchGraniteserviceversion20100801State;

/// The CloudWatch store in the SAME shared engine. `None` means the engine carries no such
/// store — a deployment with no CloudWatch for a publish to reach.
pub fn cloudwatch_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<CloudWatchState>> {
    ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state::service_state(engine)
}

/// The namespace the environment health metrics of this service are published to, as the
/// Amazon MWAA metric reference the operation's own documentation points at names it
/// ("Amazon MWAA performance metrics in Amazon CloudWatch").
pub const METRIC_NAMESPACE: &str = "AWS/MWAA";

/// The dimension every one of those metrics is reported per: the environment the datum was
/// published for, which the request addresses in its URI rather than in the datum.
pub const ENVIRONMENT_DIMENSION: &str = "Environment";

/// The `StorageResolution` these datapoints are published at, in seconds: regular
/// resolution, "which CloudWatch stores at 1-minute resolution"
/// (`MetricDatum$StorageResolution`), so a reader laying one-minute periods over the series
/// sees the minute each datum was observed in.
pub const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// How many dimensions ONE metric may carry, as the destination itself declares it — the
/// number Amazon CloudWatch documents for a metric. Re-exported from there so this service's
/// own bound on a datum's dimensions ([`crate::state::MAX_DATUM_DIMENSIONS`]) cannot drift
/// from what the store those metrics live in holds.
pub const DESTINATION_MAX_METRIC_DIMENSIONS: usize = cloudwatch::MAX_METRIC_DIMENSIONS;

/// How many milliseconds one second holds — a published instant arrives in milliseconds
/// (`MetricDatapoint::timestamp_millis`) and the destination holds it in seconds.
const MILLIS_PER_SECOND: f64 = 1_000.0;

/// Why a batch a request carried was not published.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricRejection {
    /// The batch would take the environment past [`MAX_METRIC_SERIES`] series.
    SeriesLimitExceeded,
    /// The destination store holds no more series, so the datapoints have nowhere to go.
    DestinationAtCapacity,
}

/// The series one metric of `environment` lives in: the [`METRIC_NAMESPACE`] namespace, the
/// metric's name, and the datum's own dimensions together with [`ENVIRONMENT_DIMENSION`].
///
/// The one place a published series is addressed, so the write and every reader that looks
/// for what a publish stored name the same series.
#[must_use]
pub fn series_of(
    environment: &str,
    metric_name: &str,
    dimensions: &cloudwatch::DimensionSet,
) -> cloudwatch::MetricKey {
    let mut dimensions = dimensions.clone();
    dimensions.insert(ENVIRONMENT_DIMENSION.to_string(), environment.to_string());
    cloudwatch::MetricKey {
        namespace: METRIC_NAMESPACE.to_string(),
        metric_name: metric_name.to_string(),
        dimensions,
    }
}

/// ONE observation of a series, as the destination holds it: the statistics the datum
/// states, at the instant it states, under the unit it declared (`None` where it declared
/// none, which the destination reports as the unit the enumerated type has for a
/// measurement without one).
#[must_use]
pub fn observation(datapoint: &MetricDatapoint) -> cloudwatch::StoredDatum {
    cloudwatch::StoredDatum {
        timestamp_secs: datapoint.timestamp_millis as f64 / MILLIS_PER_SECOND,
        sample_count: datapoint.statistics.sample_count as f64,
        sum: datapoint.statistics.sum,
        minimum: datapoint.statistics.minimum,
        maximum: datapoint.statistics.maximum,
        unit: datapoint.unit.clone(),
        storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
    }
}

/// `at` as the epoch seconds the destination stamps a write with — the instant an alarm
/// over the written series becomes due to be evaluated from. An instant before the epoch
/// (which no clock reports) reads as the epoch itself rather than failing the publish.
#[must_use]
pub fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// Whether `key` is a series this service published for `environment`.
fn published_for(key: &cloudwatch::MetricKey, environment: &str) -> bool {
    key.namespace == METRIC_NAMESPACE
        && key
            .dimensions
            .get(ENVIRONMENT_DIMENSION)
            .is_some_and(|named| named == environment)
}

/// How many distinct series this service has published for `environment` — what the
/// per-environment ceiling [`MAX_METRIC_SERIES`] is judged against, counted in the
/// destination rather than from a tally kept here, so it cannot drift from what is stored.
#[must_use]
pub fn environment_series_count(store: &CloudWatchState, environment: &str) -> usize {
    store
        .metrics
        .keys()
        .filter(|key| published_for(key, environment))
        .count()
}

/// Publish a whole batch for `environment`, or none of it, at the instant `now_secs`
/// (epoch seconds).
///
/// Both fallible steps are taken FIRST, against the series the batch would OPEN counted
/// together: the environment's own ceiling, and the destination having room for them. Past
/// those checks an append cannot refuse a series, so no datapoint of a refused batch can
/// already have been written.
///
/// The work this does while the destination is locked is sized by the batch, which the
/// caller bounds before it builds one ([`crate::state::MAX_PUBLISHED_DATUMS`]): the series
/// are deduplicated here, so what the alarm fan-out is judged against is the series written
/// rather than the datapoints carried.
///
/// Datapoints of one series keep the order the request listed them in; which minute each is
/// aggregated in is its own instant's, which is what the destination's readers window by.
pub fn record_all(
    state: &::engine::ServiceState<CloudWatchState>,
    environment: &str,
    batch: &[PublishedMetric],
    now_secs: f64,
) -> Result<(), MetricRejection> {
    let mut grouped: BTreeMap<cloudwatch::MetricKey, Vec<cloudwatch::StoredDatum>> =
        BTreeMap::new();
    for metric in batch {
        grouped
            .entry(series_of(
                environment,
                &metric.metric_name,
                &metric.dimensions,
            ))
            .or_default()
            .push(observation(&metric.datapoint));
    }

    let mut store = state.lock();
    let opening = grouped
        .keys()
        .filter(|key| !store.metrics.contains_key(*key))
        .count();
    if environment_series_count(&store, environment) + opening > MAX_METRIC_SERIES {
        return Err(MetricRejection::SeriesLimitExceeded);
    }
    if store.metrics.len() + opening > cloudwatch::MAX_SERIES {
        return Err(MetricRejection::DestinationAtCapacity);
    }

    let mut written: Vec<cloudwatch::WrittenSeries> = Vec::with_capacity(grouped.len());
    for (key, data) in grouped {
        // The units these datapoints carry, each as the destination holds it — which is
        // what decides whether an alarm over this series would grade any of them. DISTINCT
        // units, because the question is which units the series was written under: a batch
        // of many datapoints publishes a handful of them at most.
        let units: Vec<String> = data
            .iter()
            .map(|datum| datum.unit().to_string())
            .collect::<::std::collections::BTreeSet<String>>()
            .into_iter()
            .collect();
        let watched = key.clone();
        store
            .append_datapoints(key, data)
            .map_err(|_| MetricRejection::DestinationAtCapacity)?;
        written.push(cloudwatch::WrittenSeries {
            key: watched,
            units,
        });
    }
    // An alarm is evaluated FROM these datapoints, so storing a series it watches is what
    // makes its evaluation due — an alarm cannot tell a service's own metric from one a
    // client published.
    store.make_alarms_due(&written, now_secs);
    Ok(())
}
