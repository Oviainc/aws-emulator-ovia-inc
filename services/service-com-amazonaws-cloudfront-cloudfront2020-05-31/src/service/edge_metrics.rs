// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch series the EDGE publishes about the viewer requests it serves.
//!
//! CloudFront reports what its edge did with viewer traffic as the per-distribution metrics
//! of the [`NAMESPACE`] namespace, dimensioned by the distribution the traffic was addressed
//! to ([`DISTRIBUTION_ID_DIMENSION`]) beside the Region the metrics are reported for, which
//! for a distribution is always [`GLOBAL_REGION`] — CloudFront is not a regional service, so
//! its per-distribution series carry that one value. Those are the series a client watches a
//! distribution with: an alarm over [`FOUR_XX_ERROR_RATE_METRIC`] is how a client is told its
//! viewers are getting errors, so an edge that publishes nothing leaves every such alarm
//! without data and an alarm that can never fire cannot be told from one correctly staying
//! `OK`.
//!
//! EVERY answer the edge serves for a distribution is counted — the ones an origin produced,
//! the ones the edge cache held and the ones CloudFront generated itself — because each of
//! them is a viewer request CloudFront received and answered. A request that resolves to no
//! distribution of this account is counted into nothing: these series are per-distribution,
//! and there is no distribution to charge it to.
//!
//! What each series carries follows the metric's own definition:
//!
//! * [`REQUESTS_METRIC`] — "the total number of viewer requests received by CloudFront, for
//!   all HTTP methods and for both HTTP and HTTPS requests": one observation of 1 per request,
//!   so the `Sum` over a period is how many requests that period had;
//! * [`BYTES_DOWNLOADED_METRIC`] — the bytes viewers downloaded, for the methods that read an
//!   object ([`DOWNLOADING_METHODS`]): the length of the body the edge served;
//! * [`BYTES_UPLOADED_METRIC`] — the bytes uploaded to the origin, for the methods that write
//!   one ([`UPLOADING_METHODS`]): the length of the body the viewer sent;
//! * [`FOUR_XX_ERROR_RATE_METRIC`], [`FIVE_XX_ERROR_RATE_METRIC`] and
//!   [`TOTAL_ERROR_RATE_METRIC`] — the PERCENTAGE of the period's viewer requests whose answer
//!   carried a status of that class. A percentage is published per request as
//!   [`WHOLE_PERCENT`] when that request's own status is of the class and 0 when it is not, so
//!   the `Average` over any period is exactly the share of that period's requests that
//!   answered in it — which is what the metric is, and is why it is published with
//!   [`PERCENT_UNIT`].
//!
//! The datapoints go into the CloudWatch emulator's OWN metric store — the same store
//! `ListMetrics`, `GetMetricStatistics` and the alarm evaluation read — reached through the
//! shared engine, so one metric has one home and the publisher and its readers cannot drift.
//! They never travel over `PutMetricData`, which refuses a publish into a namespace the
//! services own ("you should not specify a namespace that begins with `AWS/`",
//! `PutMetricDataInput$Namespace`).

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;
use std::time::{SystemTime, UNIX_EPOCH};

/// The namespace CloudFront reports its per-distribution metrics in.
pub const NAMESPACE: &str = "AWS/CloudFront";

/// The dimension naming the distribution a datapoint is about.
pub const DISTRIBUTION_ID_DIMENSION: &str = "DistributionId";

/// The dimension naming the Region a per-distribution series is reported for.
pub const REGION_DIMENSION: &str = "Region";

/// The one value [`REGION_DIMENSION`] takes for a distribution's own series: CloudFront
/// serves a distribution from every edge location rather than from a Region, so what its
/// per-distribution metrics are reported for is the whole of it.
pub const GLOBAL_REGION: &str = "Global";

/// How many viewer requests the edge received.
pub const REQUESTS_METRIC: &str = "Requests";

/// How many bytes viewers downloaded through the edge.
pub const BYTES_DOWNLOADED_METRIC: &str = "BytesDownloaded";

/// How many bytes viewers uploaded to the origin through the edge.
pub const BYTES_UPLOADED_METRIC: &str = "BytesUploaded";

/// The share of the period's viewer requests answered with a 4xx or a 5xx status.
pub const TOTAL_ERROR_RATE_METRIC: &str = "TotalErrorRate";

/// The share of the period's viewer requests answered with a 4xx status.
pub const FOUR_XX_ERROR_RATE_METRIC: &str = "4xxErrorRate";

/// The share of the period's viewer requests answered with a 5xx status.
pub const FIVE_XX_ERROR_RATE_METRIC: &str = "5xxErrorRate";

/// Every per-distribution series the edge publishes for a distribution that serves traffic.
///
/// All of them are published for every request, so a distribution with traffic HAS each of
/// these series and a reader of one is not told a period had no requests because that
/// period's requests happened to carry no bytes or no errors.
pub const DISTRIBUTION_METRICS: &[&str] = &[
    REQUESTS_METRIC,
    BYTES_DOWNLOADED_METRIC,
    BYTES_UPLOADED_METRIC,
    TOTAL_ERROR_RATE_METRIC,
    FOUR_XX_ERROR_RATE_METRIC,
    FIVE_XX_ERROR_RATE_METRIC,
];

/// The unit an error-rate series carries: it is a percentage of the period's requests, which
/// is the `StandardUnit` value `Percent`.
const PERCENT_UNIT: &str = "Percent";

/// The percentage ONE request contributes to the rate of the class its own status falls in:
/// that request was wholly of that class.
const WHOLE_PERCENT: f64 = 100.0;

/// The percentage one request contributes to the rate of a class its status is not in.
const NO_PERCENT: f64 = 0.0;

/// The `StorageResolution` these datapoints are published at, in seconds: regular resolution,
/// "which CloudWatch stores at 1-minute resolution" (`MetricDatum$StorageResolution`) — the
/// granularity CloudFront reports a distribution's traffic at.
const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// The methods whose answer's bytes are bytes a viewer DOWNLOADED, as
/// [`BYTES_DOWNLOADED_METRIC`] counts them ("the total number of bytes downloaded by viewers
/// for GET, HEAD, and OPTIONS requests").
const DOWNLOADING_METHODS: [&str; 3] = ["GET", "HEAD", "OPTIONS"];

/// The methods whose request body is bytes a viewer UPLOADED, as [`BYTES_UPLOADED_METRIC`]
/// counts them ("the total number of bytes that viewers uploaded to your origin […] using
/// POST and PUT requests").
const UPLOADING_METHODS: [&str; 2] = ["POST", "PUT"];

/// The lowest status of the 4xx class, and of the 5xx class, and the first status past both.
const CLIENT_ERROR_STATUS: u16 = 400;
const SERVER_ERROR_STATUS: u16 = 500;
const PAST_SERVER_ERROR_STATUS: u16 = 600;

/// ONE viewer request the edge answered, as these series read it.
#[derive(Debug)]
pub struct ServedRequest<'a> {
    /// The request's method, upper-cased, which decides which byte counter it feeds.
    pub method: &'a str,
    /// The status the edge answered with, whoever produced the answer.
    pub status: u16,
    /// How many bytes of body the edge served.
    pub response_bytes: usize,
    /// How many bytes of body the viewer sent.
    pub request_bytes: usize,
}

impl ServedRequest<'_> {
    /// What this one request contributes to each of the distribution's series: the metric, the
    /// value, and the unit the value is published under.
    fn observations(&self) -> [(&'static str, f64, Option<&'static str>); 6] {
        let four_xx = (CLIENT_ERROR_STATUS..SERVER_ERROR_STATUS).contains(&self.status);
        let five_xx = (SERVER_ERROR_STATUS..PAST_SERVER_ERROR_STATUS).contains(&self.status);
        [
            // A counter CloudFront publishes with no unit of measure, which CloudWatch
            // spells by leaving the unit out of the datum.
            (REQUESTS_METRIC, 1.0, None),
            (
                BYTES_DOWNLOADED_METRIC,
                bytes_of(DOWNLOADING_METHODS.contains(&self.method), self.response_bytes),
                None,
            ),
            (
                BYTES_UPLOADED_METRIC,
                bytes_of(UPLOADING_METHODS.contains(&self.method), self.request_bytes),
                None,
            ),
            (
                TOTAL_ERROR_RATE_METRIC,
                percent_of(four_xx || five_xx),
                Some(PERCENT_UNIT),
            ),
            (
                FOUR_XX_ERROR_RATE_METRIC,
                percent_of(four_xx),
                Some(PERCENT_UNIT),
            ),
            (
                FIVE_XX_ERROR_RATE_METRIC,
                percent_of(five_xx),
                Some(PERCENT_UNIT),
            ),
        ]
    }
}

/// The percentage one request contributes to a rate whose class its status either is or is
/// not in.
fn percent_of(in_class: bool) -> f64 {
    if in_class {
        WHOLE_PERCENT
    } else {
        NO_PERCENT
    }
}

/// `bytes` when this request's method is one the counter counts, and 0 when it is not: a
/// request of another method contributed nothing to it, which is what a counter's own 0 says.
fn bytes_of(counted: bool, bytes: usize) -> f64 {
    if counted {
        // A body longer than f64's exact integers cannot arise: the edge buffers both bodies
        // in memory.
        bytes as f64
    } else {
        0.0
    }
}

/// Count one answer the edge served into the series of the distribution it served it for.
///
/// Nothing is published when the CloudWatch store is not mounted beside this service: a
/// datapoint has one home, and this path never makes a second one.
pub fn count_viewer_request<Eng: Send + Sync + 'static>(
    engine: &Eng,
    distribution_id: &str,
    at: SystemTime,
    served: &ServedRequest<'_>,
) {
    let Some(state) = cloudwatch::state::service_state(engine) else {
        return;
    };
    let dimensions: cloudwatch::state::DimensionSet = [
        (
            DISTRIBUTION_ID_DIMENSION.to_string(),
            distribution_id.to_string(),
        ),
        (REGION_DIMENSION.to_string(), GLOBAL_REGION.to_string()),
    ]
    .into_iter()
    .collect();
    let timestamp_secs = epoch_secs(at);
    let mut store = state.lock();
    for (metric_name, value, unit) in served.observations() {
        let key = cloudwatch::state::MetricKey {
            namespace: NAMESPACE.to_string(),
            metric_name: metric_name.to_string(),
            dimensions: dimensions.clone(),
        };
        let datum = cloudwatch::state::StoredDatum {
            // CloudWatch holds a datapoint's instant as epoch SECONDS, which is the unit its
            // own readers window by.
            timestamp_secs,
            // One observation: this request's own contribution to the series.
            sample_count: 1.0,
            sum: value,
            minimum: value,
            maximum: value,
            unit: unit.map(str::to_string),
            storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
        };
        if store.append_datapoints(key, vec![datum]).is_err() {
            // The metric store holds no more series. The request itself was already served,
            // so nothing is failed for it; the dropped observation is reported where a
            // developer can see it.
            ::tracing::warn!(
                distribution = %distribution_id,
                metric = %metric_name,
                "the CloudWatch metric store is at capacity, so this viewer request was not \
                 counted"
            );
        }
    }
}

/// `at` as the epoch seconds a stored datapoint carries.
fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

#[cfg(test)]
mod tests {
    //! What one served answer contributes to each series, which is the rule a reader folding
    //! a period of them depends on. The published series themselves are pinned end to end
    //! over the wire in `tests/all/edge_request_metrics.rs`.

    use super::*;

    /// The value `metric` takes in `served`'s observations.
    fn value_of(served: &ServedRequest<'_>, metric: &str) -> f64 {
        served
            .observations()
            .into_iter()
            .find(|(name, _, _)| *name == metric)
            .map(|(_, value, _)| value)
            .unwrap_or_else(|| panic!("{metric} is one of the series the edge publishes"))
    }

    #[test]
    fn an_error_rate_is_the_whole_of_a_request_of_its_own_class() {
        // Rows span the status classes the rates split viewer requests into, including the
        // boundaries of each class.
        let cases: [(u16, f64, f64, f64); 8] = [
            (200, 0.0, 0.0, 0.0),
            (304, 0.0, 0.0, 0.0),
            (399, 0.0, 0.0, 0.0),
            (400, 100.0, 0.0, 100.0),
            (404, 100.0, 0.0, 100.0),
            (499, 100.0, 0.0, 100.0),
            (500, 0.0, 100.0, 100.0),
            (502, 0.0, 100.0, 100.0),
        ];
        for (status, four_xx, five_xx, total) in cases {
            let served = ServedRequest {
                method: "GET",
                status,
                response_bytes: 0,
                request_bytes: 0,
            };
            assert_eq!(
                value_of(&served, FOUR_XX_ERROR_RATE_METRIC),
                four_xx,
                "case {status}: expected a 4xx rate of {four_xx}"
            );
            assert_eq!(
                value_of(&served, FIVE_XX_ERROR_RATE_METRIC),
                five_xx,
                "case {status}: expected a 5xx rate of {five_xx}"
            );
            assert_eq!(
                value_of(&served, TOTAL_ERROR_RATE_METRIC),
                total,
                "case {status}: expected a total error rate of {total}"
            );
            assert_eq!(
                value_of(&served, REQUESTS_METRIC),
                1.0,
                "case {status}: every answer is one viewer request"
            );
        }
    }

    #[test]
    fn the_byte_counters_count_the_methods_their_own_definitions_name() {
        // Rows span the methods a cache behaviour can allow, each one carrying bytes in both
        // directions so what is counted is the method's own counter and not the bytes.
        let cases: [(&str, f64, f64); 7] = [
            ("GET", 11.0, 0.0),
            ("HEAD", 11.0, 0.0),
            ("OPTIONS", 11.0, 0.0),
            ("POST", 0.0, 7.0),
            ("PUT", 0.0, 7.0),
            ("PATCH", 0.0, 0.0),
            ("DELETE", 0.0, 0.0),
        ];
        for (method, downloaded, uploaded) in cases {
            let served = ServedRequest {
                method,
                status: 200,
                response_bytes: 11,
                request_bytes: 7,
            };
            assert_eq!(
                value_of(&served, BYTES_DOWNLOADED_METRIC),
                downloaded,
                "case {method}: expected {downloaded} bytes downloaded"
            );
            assert_eq!(
                value_of(&served, BYTES_UPLOADED_METRIC),
                uploaded,
                "case {method}: expected {uploaded} bytes uploaded"
            );
        }
    }

    #[test]
    fn every_published_series_is_observed_by_every_request() {
        let served = ServedRequest {
            method: "GET",
            status: 404,
            response_bytes: 3,
            request_bytes: 0,
        };
        let observed: Vec<&str> = served
            .observations()
            .into_iter()
            .map(|(name, _, _)| name)
            .collect();
        assert_eq!(
            observed, DISTRIBUTION_METRICS,
            "every series the edge publishes is observed by every request it serves"
        );
    }
}
