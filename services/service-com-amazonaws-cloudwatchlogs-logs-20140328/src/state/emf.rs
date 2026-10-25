// SPDX-License-Identifier: CC0-1.0

//! Metric extraction from an INGESTED log event, the path that turns an Embedded Metric
//! Format log event into CloudWatch metric datapoints.
//!
//! A log event whose message is an Embedded Metric Format document declares the metrics it
//! carries in its own `_aws` member: a `CloudWatchMetrics` list of directives, each naming
//! the namespace to publish into, the metrics (by the name of the document member holding
//! each value) and the dimension sets to publish them under. Ingesting such an event
//! publishes those values as metric data, which is why a client sees a datapoint in that
//! namespace without ever calling `PutMetricData`.
//!
//! The datapoints are appended to the CloudWatch metric store — the SAME store
//! `PutMetricData` writes and `GetMetricStatistics` / `GetMetricData` read — reached through
//! the shared engine, so one metric has one home and the two writers cannot drift.
//!
//! What this module does NOT decide is whether extraction happens at all: an account-level
//! `METRIC_EXTRACTION_POLICY` can turn it off for the log groups its selection criteria
//! name, and that question belongs to the account policies in [`super`].

use std::collections::BTreeMap;

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

/// The member of a log event's message that makes it an Embedded Metric Format document,
/// and the members of it this extraction reads. The names are the ones the format defines,
/// which is also how the documents the format's own libraries emit spell them.
const AWS_MEMBER: &str = "_aws";
/// See [`AWS_MEMBER`]: the directives, each publishing into one namespace.
const CLOUDWATCH_METRICS_MEMBER: &str = "CloudWatchMetrics";
/// See [`AWS_MEMBER`]: the instant the document's values were observed, in milliseconds
/// since the epoch.
const TIMESTAMP_MEMBER: &str = "Timestamp";
/// See [`AWS_MEMBER`]: the namespace one directive publishes into.
const NAMESPACE_MEMBER: &str = "Namespace";
/// See [`AWS_MEMBER`]: the metrics one directive declares.
const METRICS_MEMBER: &str = "Metrics";
/// See [`AWS_MEMBER`]: the dimension sets one directive publishes its metrics under, each an
/// array of the document members whose values are the dimension values.
const DIMENSIONS_MEMBER: &str = "Dimensions";
/// See [`AWS_MEMBER`]: the name of the document member holding one metric's value, which is
/// also the metric's name.
const NAME_MEMBER: &str = "Name";
/// See [`AWS_MEMBER`]: the unit one metric's values are in.
const UNIT_MEMBER: &str = "Unit";
/// See [`AWS_MEMBER`]: the resolution one metric is stored at.
const STORAGE_RESOLUTION_MEMBER: &str = "StorageResolution";

/// The resolution a metric is stored at when the directive declares none: the default
/// `MetricDatum$StorageResolution` documents ("This field is optional, if you do not specify
/// it the default of 60 is used"), so an extracted metric is a regular-resolution metric
/// unless the document says otherwise.
const DEFAULT_STORAGE_RESOLUTION: i32 = 60;

/// How many datapoints one ingested event may publish, whatever its document declares: the
/// ceiling that keeps one `PutLogEvents` batch from writing an unbounded number of series.
/// THE EMULATOR'S OWN CEILING, not a documented limit.
const MAX_DATAPOINTS_PER_EVENT: usize = 100;

/// One datapoint an ingested Embedded Metric Format document publishes.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedDatapoint {
    /// The namespace the directive publishes into.
    pub namespace: String,
    /// The metric's name, which is the document member the value was read from.
    pub metric_name: String,
    /// The dimension set the value is published under, name to value.
    pub dimensions: BTreeMap<String, String>,
    /// The instant the value was observed, in milliseconds since the epoch.
    pub timestamp_ms: i64,
    /// The value itself.
    pub value: f64,
    /// The unit the directive declared, when it declared one.
    pub unit: Option<String>,
    /// The resolution the metric is stored at.
    pub storage_resolution: i32,
}

/// The datapoints the log event `message` publishes, which is empty for every message that
/// is not an Embedded Metric Format document.
///
/// `event_timestamp_ms` is the log event's own timestamp, used for a document that declares
/// no `Timestamp` of its own: the value was observed when the event it travelled in was.
pub fn extracted_datapoints(message: &str, event_timestamp_ms: i64) -> Vec<ExtractedDatapoint> {
    let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(message) else {
        return Vec::new();
    };
    let Some(metadata) = document.get(AWS_MEMBER).and_then(|aws| aws.as_object()) else {
        return Vec::new();
    };
    let timestamp_ms = metadata
        .get(TIMESTAMP_MEMBER)
        .and_then(::serde_json::Value::as_i64)
        .unwrap_or(event_timestamp_ms);
    let Some(directives) = metadata
        .get(CLOUDWATCH_METRICS_MEMBER)
        .and_then(::serde_json::Value::as_array)
    else {
        return Vec::new();
    };

    let mut datapoints: Vec<ExtractedDatapoint> = Vec::new();
    for directive in directives {
        let Some(namespace) = directive
            .get(NAMESPACE_MEMBER)
            .and_then(::serde_json::Value::as_str)
        else {
            continue;
        };
        let Some(metrics) = directive
            .get(METRICS_MEMBER)
            .and_then(::serde_json::Value::as_array)
        else {
            continue;
        };
        // "Dimensions" is a list of dimension SETS: each set publishes the directive's
        // metrics under its own dimensions, so one metric of a directive declaring two sets
        // is two series. A directive declaring no set at all publishes the metrics with no
        // dimensions.
        let dimension_sets = dimension_sets(directive, &document);
        for metric in metrics {
            let Some(metric_name) = metric.get(NAME_MEMBER).and_then(::serde_json::Value::as_str)
            else {
                continue;
            };
            let unit = metric
                .get(UNIT_MEMBER)
                .and_then(::serde_json::Value::as_str)
                .map(ToString::to_string);
            let storage_resolution = metric
                .get(STORAGE_RESOLUTION_MEMBER)
                .and_then(::serde_json::Value::as_i64)
                .and_then(|resolution| i32::try_from(resolution).ok())
                .unwrap_or(DEFAULT_STORAGE_RESOLUTION);
            // The metric's values live in the document member the directive names, as one
            // number or as an array of numbers: a member the document does not carry, or
            // that is not numeric, declares a metric with nothing observed, so it publishes
            // no datapoint rather than a zero nothing measured.
            for value in values_of(document.get(metric_name)) {
                for dimensions in &dimension_sets {
                    if datapoints.len() >= MAX_DATAPOINTS_PER_EVENT {
                        return datapoints;
                    }
                    datapoints.push(ExtractedDatapoint {
                        namespace: namespace.to_string(),
                        metric_name: metric_name.to_string(),
                        dimensions: dimensions.clone(),
                        timestamp_ms,
                        value,
                        unit: unit.clone(),
                        storage_resolution,
                    });
                }
            }
        }
    }
    datapoints
}

/// The dimension sets one directive publishes under, each resolved from the document's own
/// members: a dimension's value is the value of the document member named by it.
///
/// A set naming a member the document does not carry, or whose value is not a string, names
/// no dimension value, so that set is left out rather than published with a value nothing
/// supplied.
fn dimension_sets(
    directive: &::serde_json::Value,
    document: &::serde_json::Value,
) -> Vec<BTreeMap<String, String>> {
    let Some(sets) = directive
        .get(DIMENSIONS_MEMBER)
        .and_then(::serde_json::Value::as_array)
    else {
        return vec![BTreeMap::new()];
    };
    if sets.is_empty() {
        return vec![BTreeMap::new()];
    }
    let mut resolved: Vec<BTreeMap<String, String>> = Vec::with_capacity(sets.len());
    for set in sets {
        let Some(names) = set.as_array() else {
            continue;
        };
        let mut dimensions: BTreeMap<String, String> = BTreeMap::new();
        let mut complete = true;
        for name in names {
            let value = name
                .as_str()
                .and_then(|name| document.get(name))
                .and_then(::serde_json::Value::as_str);
            match (name.as_str(), value) {
                (Some(name), Some(value)) => {
                    dimensions.insert(name.to_string(), value.to_string());
                }
                _ => complete = false,
            }
        }
        if complete {
            resolved.push(dimensions);
        }
    }
    resolved
}

/// The numeric observations a document member holds: one value, every value of an array of
/// them, or none at all for a member that is absent or not numeric.
fn values_of(member: Option<&::serde_json::Value>) -> Vec<f64> {
    match member {
        Some(::serde_json::Value::Array(values)) => values
            .iter()
            .filter_map(::serde_json::Value::as_f64)
            .collect(),
        Some(value) => value.as_f64().map(|value| vec![value]).unwrap_or_default(),
        None => Vec::new(),
    }
}

/// Publish `datapoints` into the CloudWatch metric store the shared engine holds.
///
/// Nothing is published when that store is not mounted: a datapoint has one home, and this
/// path never makes a second one. The call is made with no log-store guard held, so the two
/// services' stores are never locked at once.
pub fn publish<Eng: Send + Sync + 'static>(engine: &Eng, datapoints: &[ExtractedDatapoint]) {
    if datapoints.is_empty() {
        return;
    }
    let Some(state) = cloudwatch::state::service_state(engine) else {
        return;
    };
    let mut store = state.lock();
    for datapoint in datapoints {
        let key = cloudwatch::state::MetricKey {
            namespace: datapoint.namespace.clone(),
            metric_name: datapoint.metric_name.clone(),
            dimensions: datapoint.dimensions.clone(),
        };
        let stored = cloudwatch::state::StoredDatum {
            // CloudWatch holds a datapoint's instant in seconds, which is the unit its own
            // readers window by; the document carries milliseconds.
            timestamp_secs: datapoint.timestamp_ms as f64 / 1_000.0,
            sample_count: 1.0,
            sum: datapoint.value,
            minimum: datapoint.value,
            maximum: datapoint.value,
            unit: datapoint.unit.clone(),
            storage_resolution: datapoint.storage_resolution,
        };
        // A metric store at capacity takes no new series; the ingest it was extracted from
        // is not failed for it, because the log event itself was accepted.
        let _ = store.append_datapoints(key, vec![stored]);
    }
}
