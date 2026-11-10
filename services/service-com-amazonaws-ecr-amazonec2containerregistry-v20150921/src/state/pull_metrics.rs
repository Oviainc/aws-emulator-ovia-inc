// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch metric Amazon ECR publishes about the pulls its own data plane serves.
//!
//! A pull is served over the Docker Registry v2 endpoint with no API call of the client's
//! to report it (`crate::service`'s registry data plane), so the only way a client can see
//! that a repository was pulled from is the series ECR publishes about itself: the
//! [`REPOSITORY_PULL_COUNT_METRIC`] metric of the [`METRIC_NAMESPACE`] namespace,
//! dimensioned on the repository that was pulled from ([`REPOSITORY_NAME_DIMENSION`]) —
//! the Amazon ECR metrics the Amazon ECR User Guide publishes for a private registry.
//! One observation is published per pull, so `Sum` over any period is how many pulls that
//! period served.
//!
//! The datapoints are appended to the CloudWatch store directly rather than sent through
//! `PutMetricData`: CloudWatch reserves the `AWS/` namespaces for the services that own
//! them ("you should not specify a namespace that begins with `AWS/`",
//! `PutMetricData$Namespace`), so a service's own metric never travels over that API. It
//! goes into the very store `ListMetrics` and `GetMetricStatistics` read, through
//! CloudWatch's own bounded append, so one metric and its reader cannot disagree.
//!
//! A repository nothing has pulled from publishes NO datapoint: the series exists because
//! a pull happened, and reporting a zero for a repository nobody pulled would claim an
//! observation nothing made.

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

/// The namespace Amazon ECR publishes its own metrics to.
pub const METRIC_NAMESPACE: &str = "AWS/ECR";

/// The metric counting the pulls of one repository.
pub const REPOSITORY_PULL_COUNT_METRIC: &str = "RepositoryPullCount";

/// The dimension those pulls are reported per: the repository they were served from.
pub const REPOSITORY_NAME_DIMENSION: &str = "RepositoryName";

/// The unit one pull is counted in: `Count`, the `StandardUnit` member for a plain tally
/// of occurrences (`smithy_query(query="shape",shape="StandardUnit")`, read against the
/// CloudWatch model) — a pull count is a number of pulls and not a rate or a size.
pub const PULL_COUNT_UNIT: &str = "Count";

/// How much ONE observation of the pull count stands for: one pull.
const ONE_PULL: f64 = 1.0;

/// The `StorageResolution` the datapoint is published at, in seconds: regular resolution,
/// "which CloudWatch stores at 1-minute resolution"
/// (`MetricDatum$StorageResolution`) — the minute ECR publishes its registry metrics per.
const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// Count one pull of `repository_name`, served by this registry's data plane.
///
/// Best effort, the way every cross-service publication in this tree is: an emulator with
/// no CloudWatch mounted has nowhere to publish, and a CloudWatch store at capacity keeps
/// what it holds — the pull itself is served either way, and the failure is logged rather
/// than reported to the client, which asked for a layer and not for a metric.
pub fn count_repository_pull<Eng: Send + Sync + 'static>(engine: &Eng, repository_name: &str) {
    let Some(store) = cloudwatch::service_state(engine) else {
        return;
    };
    let key = cloudwatch::MetricKey {
        namespace: METRIC_NAMESPACE.to_string(),
        metric_name: REPOSITORY_PULL_COUNT_METRIC.to_string(),
        dimensions: [(
            REPOSITORY_NAME_DIMENSION.to_string(),
            repository_name.to_string(),
        )]
        .into_iter()
        .collect(),
    };
    let datum = cloudwatch::StoredDatum {
        timestamp_secs: cloudwatch::now_secs(),
        sample_count: ONE_PULL,
        sum: ONE_PULL,
        minimum: ONE_PULL,
        maximum: ONE_PULL,
        unit: Some(PULL_COUNT_UNIT.to_string()),
        storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
    };
    if store.lock().append_datapoints(key, vec![datum]).is_err() {
        ::tracing::warn!(
            repository = %repository_name,
            metric = REPOSITORY_PULL_COUNT_METRIC,
            "the CloudWatch store is at capacity; a repository pull could not be counted"
        );
    }
}
