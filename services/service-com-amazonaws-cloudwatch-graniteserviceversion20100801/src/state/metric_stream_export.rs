// SPDX-License-Identifier: CC0-1.0

//! The EXPORT a running metric stream makes of the metric updates it streams.
//!
//! "Creates or updates a metric stream. Metric streams can automatically stream CloudWatch
//! metrics to Amazon Web Services destinations" … "When you use `PutMetricStream` to create a
//! new metric stream, the stream is created in the `running` state." (`PutMetricStream`) — so
//! the configuration a stream is written with is not the whole resource: a stream in that state
//! EXPORTS, without any further call, every metric update its filters select into the Amazon
//! Data Firehose delivery stream its `FirehoseArn` names. That export is this module.
//!
//! What one exported record is:
//!
//! * WHICH metrics — the stream's filters, as the operation states the three choices: "Stream
//!   metrics from all metric namespaces in the account", "… except for the namespaces that you
//!   list in `ExcludeFilters`", "… only the metric namespaces that you list in
//!   `IncludeFilters`", narrowed per namespace by "If you omit this parameter, all metrics in
//!   the namespace are included or excluded" (`MetricStreamFilter$MetricNames`)
//!   ([`streams_metric`]).
//! * WHICH statistics — "By default, a metric stream always sends the `MAX`, `MIN`, `SUM`, and
//!   `SAMPLECOUNT` statistics for each metric that is streamed. You can use the
//!   `StatisticsConfigurations` parameter to have the metric stream send additional statistics
//!   in the stream." (`PutMetricStream`), each additional statistic computed over the same
//!   distribution every other read of this store computes a statistic over
//!   ([`super::statistic_value`]).
//! * OVER WHICH SPAN — one record per metric per [`EXPORT_PERIOD_SECS`] period, the
//!   granularity a regular-resolution metric is aggregated at ("a period can be as short as one
//!   minute (60 seconds) and must be a multiple of 60", `MetricStat$Period`), stamped with the
//!   instant that period opens at.
//! * IN WHICH FORM — the `OutputFormat` the stream carries ([`JSON_FORMAT`]).
//!
//! The export happens where the update does: `PutMetricData` stores the datapoints, so the
//! records those datapoints make due are pushed from that call ([`export_published`]) and a
//! consumer downstream of the delivery stream sees them without anybody calling CloudWatch
//! again. The store is only READ here — the records are computed from the datapoints it already
//! holds — so an export changes nothing a later caller sees.
//!
//! The delivery stream is a resource of another service, so a record reaches it the way any
//! emulated service reaches another: an AWS SDK client over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), with `PutRecord` per record in
//! chronological order, so the records of one metric arrive in the order they were published.
//! The call is the ROLE's to make — "The ARN of an IAM role that this metric stream will use to
//! access Amazon Kinesis Data Firehose resources. This IAM role must already exist and must be
//! in the same account as the metric stream. This IAM role must include the following
//! permissions: firehose:PutRecord firehose:PutRecordBatch" (`PutMetricStreamInput$RoleArn`) —
//! so a role IAM answers withholds that write exports nothing.
//!
//! A stream configured for one of the OpenTelemetry output formats is not exported from here:
//! those formats frame their records as length-delimited protobuf, which this module does not
//! render, and a record in another format is not a record such a consumer could read. Nothing
//! is invented for them.

use ::serde_json::{Map, Value};

use crate::ops::types::{GetMetricStreamOutput, MetricStreamFilter};

/// The span one exported record covers, and so the bucket a published datapoint is exported
/// in: one minute, the granularity a regular-resolution metric is aggregated at ("For metrics
/// with regular resolution, a period can be as short as one minute (60 seconds) and must be a
/// multiple of 60", `MetricStat$Period`). A record is stamped with the instant its period opens
/// at, so two datapoints of one metric in one period are one record and two datapoints in
/// different periods are two.
const EXPORT_PERIOD_SECS: f64 = 60.0;

/// The `OutputFormat` whose records this module renders: "Valid values are `json`,
/// `opentelemetry1.0`, and `opentelemetry0.7`." (`PutMetricStreamInput$OutputFormat`).
const JSON_FORMAT: &str = "json";

/// The ARN namespace of Amazon Data Firehose, whose delivery streams are what a metric stream
/// exports into.
const FIREHOSE_ARN_NAMESPACE: &str = "firehose";

/// The resource type a Firehose delivery stream's ARN carries
/// (`arn:aws:firehose:<region>:<account>:deliverystream/<name>`).
const DELIVERY_STREAM_RESOURCE_TYPE: &str = "deliverystream";

/// The write a metric stream's role has to be allowed to make on the delivery stream, as
/// `PutMetricStreamInput$RoleArn` names it: "This IAM role must include the following
/// permissions: firehose:PutRecord firehose:PutRecordBatch".
const PUT_RECORD_ACTION: &str = "firehose:PutRecord";

/// How many records ONE request exports.
///
/// The request's OWN bound, not a limit AWS publishes: a batch names at most 1000 metrics
/// (`PutMetricData`) and every factor below that is bounded too, but their PRODUCT with the
/// number of running streams is not, and each record is an in-process `PutRecord`. It sits far
/// above the records any realistic publish owes (a handful of streams over the metrics of one
/// batch); what it does not reach is not exported, exactly as a metric update AWS drops when a
/// stream cannot keep up is not exported.
const MAX_RECORDS_PER_REQUEST: usize = 500;

/// The members of the `value` object every exported record carries, as the statistics a stream
/// always sends: "a metric stream always sends the `MAX`, `MIN`, `SUM`, and `SAMPLECOUNT`
/// statistics for each metric that is streamed" (`PutMetricStream`), each under the name the
/// json record format spells it with.
const MAX_MEMBER: &str = "max";
const MIN_MEMBER: &str = "min";
const SUM_MEMBER: &str = "sum";
const COUNT_MEMBER: &str = "count";

/// One series a request published datapoints into, and the periods those datapoints fall in.
///
/// What a publish owes an export is the (series, period) pairs it touched: the record for such a
/// pair is computed from everything the store holds for that period, so this carries the pairs
/// and not the values.
#[derive(Debug, Clone)]
pub struct PublishedSeries {
    /// The series the datapoints were published into.
    pub key: super::MetricKey,
    /// The instants the periods they fall in open at, ascending and without repetition.
    pub period_starts: Vec<f64>,
}

/// The (series, period) pairs `ingest` publishes into, as [`export_published`] takes them.
///
/// Built from the batch a `PutMetricData` call is about to store, so an export is owed for
/// exactly the periods that call wrote into — a datum carrying no observation at all creates the
/// series without publishing an update, and owes no record.
#[must_use]
pub fn published_series(
    ingest: &[(super::MetricKey, Vec<super::StoredDatum>)],
) -> Vec<PublishedSeries> {
    let mut published: Vec<PublishedSeries> = Vec::new();
    for (key, observations) in ingest {
        if observations.is_empty() {
            continue;
        }
        let starts = observations
            .iter()
            .map(|datum| period_start(datum.timestamp_secs));
        match published.iter_mut().find(|series| series.key == *key) {
            Some(series) => series.period_starts.extend(starts),
            None => published.push(PublishedSeries {
                key: key.clone(),
                period_starts: starts.collect(),
            }),
        }
    }
    for series in &mut published {
        series.period_starts.sort_unstable_by(f64::total_cmp);
        series.period_starts.dedup();
    }
    published
}

/// The instant the [`EXPORT_PERIOD_SECS`] period holding `timestamp_secs` opens at.
fn period_start(timestamp_secs: f64) -> f64 {
    (timestamp_secs / EXPORT_PERIOD_SECS).floor() * EXPORT_PERIOD_SECS
}

/// Export the records the datapoints a request just stored owe, to every running stream of
/// `region` whose filters select them.
///
/// The store is locked to COMPUTE the records and released before any of them is delivered, so
/// one publisher's fan-out cannot block another's read, and nothing in this path writes to the
/// store: a record is a reading of the datapoints it already holds.
pub async fn export_published<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    published: &[PublishedSeries],
) {
    if published.is_empty() {
        return;
    }
    let Some(state) = super::service_state(engine) else {
        // No store to read. The handler that called this answers the store's absence with its
        // own modelled server fault; there is nothing to report from here.
        return;
    };
    let exports = {
        let store = state.lock();
        planned(&store, region, published)
    };
    for export in exports {
        deliver(ctx, &export).await;
    }
}

/// What ONE stream is owed: where it delivers, under which role, and the records themselves in
/// the order they are to arrive.
#[derive(Debug, Clone)]
struct StreamExport {
    /// The delivery stream the records are pushed into.
    firehose_arn: String,
    /// The role the push is made as, when the stream carries one.
    role_arn: Option<String>,
    /// The records, oldest period first.
    records: Vec<String>,
}

/// The exports every running stream of `region` is owed for `published`.
fn planned(
    store: &super::ComAmazonawsCloudwatchGraniteserviceversion20100801State,
    region: &str,
    published: &[PublishedSeries],
) -> Vec<StreamExport> {
    let mut exports: Vec<StreamExport> = Vec::new();
    let mut budget = MAX_RECORDS_PER_REQUEST;
    for (key, stored) in &store.metric_streams {
        if key.region != region {
            continue;
        }
        if budget == 0 {
            break;
        }
        let stream = &stored.stream;
        // "The state of the metric stream. The possible values are `running` and `stopped`."
        // (`GetMetricStreamOutput$State`) — a stopped stream is one `StopMetricStreams` has
        // taken out of service, so it exports nothing until it is started again.
        if stream.state.as_deref() != Some(super::METRIC_STREAM_RUNNING) {
            continue;
        }
        if stream.output_format.as_deref() != Some(JSON_FORMAT) {
            continue;
        }
        let Some(firehose_arn) = stream.firehose_arn.clone() else {
            continue;
        };
        let name = stream.name.clone().unwrap_or_else(|| key.name.clone());
        // Oldest period first across every metric of the batch, so the records of one metric
        // arrive in the order its datapoints were published.
        let mut records: Vec<(f64, String)> = Vec::new();
        for series in published {
            if !streams_metric(stream, &series.key) {
                continue;
            }
            let Some(data) = store.metrics.get(&series.key) else {
                continue;
            };
            let additional = additional_statistics(stream, &series.key);
            for start in &series.period_starts {
                for document in period_records(&name, region, series, *start, data, &additional) {
                    records.push((*start, document));
                }
            }
        }
        records.sort_by(|(left, _), (right, _)| left.total_cmp(right));
        records.truncate(budget);
        if records.is_empty() {
            continue;
        }
        budget -= records.len();
        exports.push(StreamExport {
            firehose_arn,
            role_arn: stream.role_arn.clone(),
            records: records.into_iter().map(|(_, document)| document).collect(),
        });
    }
    exports
}

/// Whether `stream` streams the metric `key` names.
///
/// The three choices `PutMetricStream` documents: "Stream metrics from all metric namespaces in
/// the account", "Stream metrics from all metric namespaces in the account, except for the
/// namespaces that you list in `ExcludeFilters`", "Stream metrics from only the metric
/// namespaces that you list in `IncludeFilters`" — a stream carries at most one of the two
/// lists ("You cannot include ExcludeFilters and IncludeFilters in the same operation.",
/// `PutMetricStreamInput$ExcludeFilters`), so the lists are read in that order.
fn streams_metric(stream: &GetMetricStreamOutput, key: &super::MetricKey) -> bool {
    if let Some(include) = stream.include_filters.as_deref() {
        return include.iter().any(|filter| selects(filter, key));
    }
    if let Some(exclude) = stream.exclude_filters.as_deref() {
        return !exclude.iter().any(|filter| selects(filter, key));
    }
    true
}

/// Whether one filter names the metric `key` names: its namespace, and either the whole
/// namespace or a metric name it lists — "If you omit this parameter, all metrics in the
/// namespace are included or excluded" (`MetricStreamFilter$MetricNames`).
fn selects(filter: &MetricStreamFilter, key: &super::MetricKey) -> bool {
    filter.namespace.as_deref() == Some(key.namespace.as_str())
        && filter
            .metric_names
            .as_deref()
            .is_none_or(|names| names.contains(&key.metric_name))
}

/// The additional statistics `stream` sends for the metric `key` names: "For each entry in this
/// array, you specify one or more metrics and the list of additional statistics to stream for
/// those metrics." (`PutMetricStreamInput$StatisticsConfigurations`) — so the configurations
/// naming this metric contribute their statistics, and a metric no configuration names gets the
/// four every streamed metric gets and nothing else.
fn additional_statistics(stream: &GetMetricStreamOutput, key: &super::MetricKey) -> Vec<String> {
    let mut statistics: Vec<String> = Vec::new();
    for configuration in stream.statistics_configurations.iter().flatten() {
        let names_metric = configuration.include_metrics.iter().any(|metric| {
            metric.namespace == key.namespace && metric.metric_name == key.metric_name
        });
        if !names_metric {
            continue;
        }
        for statistic in &configuration.additional_statistics {
            if !statistics.iter().any(|held| held == statistic) {
                statistics.push(statistic.clone());
            }
        }
    }
    statistics
}

/// The records one (series, period) pair owes: one per UNIT the period's datapoints carry,
/// since a record reports the measurement's unit and two datapoints published under different
/// units are not one measurement.
fn period_records(
    stream_name: &str,
    region: &str,
    series: &PublishedSeries,
    start: f64,
    data: &[super::StoredDatum],
    additional: &[String],
) -> Vec<String> {
    let mut by_unit: ::std::collections::BTreeMap<&str, Vec<&super::StoredDatum>> =
        ::std::collections::BTreeMap::new();
    for datum in data {
        if datum.timestamp_secs >= start && datum.timestamp_secs < start + EXPORT_PERIOD_SECS {
            by_unit.entry(datum.unit()).or_default().push(datum);
        }
    }
    by_unit
        .into_iter()
        .map(|(unit, group)| {
            record(
                stream_name,
                region,
                &series.key,
                start,
                unit,
                &group,
                additional,
            )
        })
        .collect()
}

/// ONE exported record, in the json output format: which stream and account it came from, the
/// metric it reports, the period it covers and the statistics of that period.
fn record(
    stream_name: &str,
    region: &str,
    key: &super::MetricKey,
    start: f64,
    unit: &str,
    group: &[&super::StoredDatum],
    additional: &[String],
) -> String {
    let folded = super::BasicStatistics::of(group.iter().copied());
    let mut value = Map::new();
    for (member, statistic) in [
        (MAX_MEMBER, folded.maximum),
        (MIN_MEMBER, folded.minimum),
        (SUM_MEMBER, folded.sum),
        (COUNT_MEMBER, folded.sample_count),
    ] {
        insert_number(&mut value, member, statistic);
    }
    // An additional statistic the period does not define is not reported as anything: a
    // percentile of a datapoint whose individual values were never published, say, is a value
    // this store cannot compute rather than one it may invent.
    for statistic in additional {
        if let Some(computed) = super::statistic_value(group, statistic) {
            insert_number(&mut value, statistic, computed);
        }
    }
    let dimensions: Map<String, Value> = key
        .dimensions
        .iter()
        .map(|(name, value)| (name.clone(), Value::String(value.clone())))
        .collect();
    let mut document = Map::new();
    document.insert(
        "metric_stream_name".to_string(),
        Value::String(stream_name.to_string()),
    );
    document.insert(
        "account_id".to_string(),
        Value::String(::emulator_server::emulator_account_id().to_string()),
    );
    document.insert("region".to_string(), Value::String(region.to_string()));
    document.insert(
        "namespace".to_string(),
        Value::String(key.namespace.clone()),
    );
    document.insert(
        "metric_name".to_string(),
        Value::String(key.metric_name.clone()),
    );
    document.insert("dimensions".to_string(), Value::Object(dimensions));
    // The instant the period opens at, in the milliseconds the json record format stamps a
    // record with.
    document.insert(
        "timestamp".to_string(),
        Value::from((start * 1_000.0) as i64),
    );
    document.insert("value".to_string(), Value::Object(value));
    document.insert("unit".to_string(), Value::String(unit.to_string()));
    // One record per line, so a consumer reading the delivered bytes reads one JSON document
    // per record however many of them a buffer was flushed with.
    format!("{}\n", Value::Object(document))
}

/// Report one statistic in the `value` object, where it is a number JSON can carry: a
/// non-finite fold is no measurement at all, and is left out rather than reported as null.
fn insert_number(value: &mut Map<String, Value>, member: &str, statistic: f64) {
    if let Some(number) = ::serde_json::Number::from_f64(statistic) {
        value.insert(member.to_string(), Value::Number(number));
    }
}

/// Push one stream's records into the delivery stream its `FirehoseArn` names, oldest first.
///
/// Every failure is LOGGED and changes nothing a caller is told: an export is asynchronous and
/// is not part of any operation's answer, exactly as an alarm action's delivery is not
/// ([`super::alarm_actions`]).
async fn deliver(ctx: &smithy_server_runtime::OperationContext<'_>, export: &StreamExport) {
    let Some(stream_name) = ::service_common::arn::resource_id_of_type(
        &export.firehose_arn,
        DELIVERY_STREAM_RESOURCE_TYPE,
    ) else {
        ::tracing::warn!(
            firehose_arn = %export.firehose_arn,
            "this metric stream's destination names no Firehose delivery stream, so nothing \
             was exported"
        );
        return;
    };
    // The Region the destination is addressed in comes from its own ARN, which a request
    // carried, so it is held to the grammar a Region is written in before it reaches an
    // endpoint.
    let Some(region) = ::service_common::arn::region_of(&export.firehose_arn, FIREHOSE_ARN_NAMESPACE)
        .and_then(::service_common::arn::region_label)
    else {
        ::tracing::warn!(
            firehose_arn = %export.firehose_arn,
            "this metric stream's destination names no Region this emulator can reach, so \
             nothing was exported"
        );
        return;
    };
    // The export is the ROLE's write to make, so a role IAM answers does not allow it exports
    // nothing: a record the destination's service would have refused the call to is never
    // pushed.
    if let Some(role_arn) = export.role_arn.as_deref() {
        if ::service_common::iam_role::role_withholds_action(
            ctx,
            region,
            role_arn,
            PUT_RECORD_ACTION,
            &export.firehose_arn,
        )
        .await
        {
            ::tracing::warn!(
                role_arn = %role_arn,
                firehose_arn = %export.firehose_arn,
                "this metric stream's role is not allowed to put records into its destination, \
                 so nothing was exported"
            );
            return;
        }
    }
    let client = firehose_client(ctx.callback(), region);
    // One `PutRecord` per record, awaited in turn: the destination takes the records in the
    // order they are put, which is what makes the records of one metric arrive in the order its
    // datapoints were published.
    for document in &export.records {
        let built = ::aws_sdk_firehose::types::Record::builder()
            .data(::aws_sdk_firehose::primitives::Blob::new(
                document.as_bytes().to_vec(),
            ))
            .build();
        let failure = match built {
            Ok(record) => client
                .put_record()
                .delivery_stream_name(stream_name)
                .record(record)
                .send()
                .await
                .err()
                .map(|error| why(&error)),
            Err(error) => Some(why(&error)),
        };
        if let Some(failure) = failure {
            ::tracing::warn!(
                firehose_arn = %export.firehose_arn,
                error = %failure,
                "a metric stream record could not be exported"
            );
            // The destination answered this put a failure, so the ones after it would be
            // answered the same way: the rest of this stream's records are left for the next
            // update rather than re-asked for here.
            return;
        }
    }
}

/// How a failed export is worded in the warning: the SDK's own rendering of the answer the
/// destination service gave.
fn why(error: &impl ::std::error::Error) -> String {
    ::aws_smithy_types::error::display::DisplayErrorContext(error).to_string()
}

/// A Firehose client over the in-process callback into this same emulator.
fn firehose_client(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_firehose::Client {
    let config = ::aws_sdk_firehose::Config::builder()
        .behavior_version(::aws_sdk_firehose::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_firehose::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://firehose.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client is
        // not given. The same goes for caching the identity: the credentials are handed over
        // directly.
        .retry_config(::aws_sdk_firehose::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_firehose::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_firehose::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_firehose::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_firehose::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::types::{MetricStreamStatisticsConfiguration, MetricStreamStatisticsMetric};

    /// A series key, for the filter rows below.
    fn key(namespace: &str, metric_name: &str) -> super::super::MetricKey {
        super::super::MetricKey {
            namespace: namespace.to_string(),
            metric_name: metric_name.to_string(),
            dimensions: super::super::DimensionSet::new(),
        }
    }

    /// A filter naming `namespace`, and the metric names when the row lists any.
    fn filter(namespace: &str, metric_names: Option<&[&str]>) -> MetricStreamFilter {
        MetricStreamFilter {
            namespace: Some(namespace.to_string()),
            metric_names: metric_names
                .map(|names| names.iter().map(|name| (*name).to_string()).collect()),
        }
    }

    /// RULE: a stream with no filter streams every namespace; one with `IncludeFilters` streams
    /// only what they name; one with `ExcludeFilters` streams everything else.
    #[test]
    fn the_filters_decide_which_metrics_are_streamed() {
        let cases: [(&str, GetMetricStreamOutput, &str, &str, bool); 6] = [
            (
                "no filter at all",
                GetMetricStreamOutput::default(),
                "Custom/One",
                "Latency",
                true,
            ),
            (
                "an include filter naming the namespace",
                GetMetricStreamOutput {
                    include_filters: Some(vec![filter("Custom/One", None)]),
                    ..GetMetricStreamOutput::default()
                },
                "Custom/One",
                "Latency",
                true,
            ),
            (
                "an include filter naming another namespace",
                GetMetricStreamOutput {
                    include_filters: Some(vec![filter("Custom/One", None)]),
                    ..GetMetricStreamOutput::default()
                },
                "Custom/Two",
                "Latency",
                false,
            ),
            (
                "an include filter naming one metric of the namespace",
                GetMetricStreamOutput {
                    include_filters: Some(vec![filter("Custom/One", Some(&["Latency"]))]),
                    ..GetMetricStreamOutput::default()
                },
                "Custom/One",
                "Errors",
                false,
            ),
            (
                "an exclude filter naming the namespace",
                GetMetricStreamOutput {
                    exclude_filters: Some(vec![filter("Custom/One", None)]),
                    ..GetMetricStreamOutput::default()
                },
                "Custom/One",
                "Latency",
                false,
            ),
            (
                "an exclude filter naming another namespace",
                GetMetricStreamOutput {
                    exclude_filters: Some(vec![filter("Custom/One", None)]),
                    ..GetMetricStreamOutput::default()
                },
                "Custom/Two",
                "Latency",
                true,
            ),
        ];
        for (case, stream, namespace, metric_name, expected) in cases {
            let streamed = streams_metric(&stream, &key(namespace, metric_name));
            assert_eq!(
                streamed, expected,
                "case {case:?}: expected streamed={expected}, got {streamed}"
            );
        }
    }

    /// RULE: the additional statistics of a metric are the ones every configuration naming THAT
    /// metric asks for, each once.
    #[test]
    fn additional_statistics_are_collected_per_metric() {
        let stream = GetMetricStreamOutput {
            statistics_configurations: Some(vec![
                MetricStreamStatisticsConfiguration {
                    include_metrics: vec![MetricStreamStatisticsMetric {
                        namespace: "Custom/One".to_string(),
                        metric_name: "Latency".to_string(),
                    }],
                    additional_statistics: vec!["p99".to_string(), "p90".to_string()],
                },
                MetricStreamStatisticsConfiguration {
                    include_metrics: vec![MetricStreamStatisticsMetric {
                        namespace: "Custom/One".to_string(),
                        metric_name: "Latency".to_string(),
                    }],
                    additional_statistics: vec!["p99".to_string(), "IQM".to_string()],
                },
            ]),
            ..GetMetricStreamOutput::default()
        };
        let asked = additional_statistics(&stream, &key("Custom/One", "Latency"));
        assert_eq!(
            asked,
            vec!["p99".to_string(), "p90".to_string(), "IQM".to_string()],
            "the statistics of the named metric must be each configuration's, each once, got \
             {asked:?}"
        );
        let other = additional_statistics(&stream, &key("Custom/One", "Errors"));
        assert!(
            other.is_empty(),
            "a metric no configuration names must get no additional statistic, got {other:?}"
        );
    }

    /// RULE: a datapoint is exported in the one-minute period it falls in, and two datapoints of
    /// one period owe one record.
    #[test]
    fn published_periods_are_the_minute_buckets_the_datapoints_fall_in() {
        let datum = |timestamp_secs: f64| super::super::StoredDatum {
            timestamp_secs,
            sample_count: 1.0,
            sum: 1.0,
            minimum: 1.0,
            maximum: 1.0,
            unit: None,
            storage_resolution: 60,
        };
        let series = key("Custom/One", "Latency");
        let published = published_series(&[(
            series.clone(),
            vec![datum(120.0), datum(179.0), datum(60.5), datum(0.0)],
        )]);
        assert_eq!(
            published.len(),
            1,
            "one series must owe one entry, got {published:?}"
        );
        assert_eq!(
            published[0].period_starts,
            vec![0.0, 60.0, 120.0],
            "the periods must be the minute buckets, ascending and without repetition, got {:?}",
            published[0].period_starts
        );
        let none = published_series(&[(series, Vec::new())]);
        assert!(
            none.is_empty(),
            "a datum carrying no observation owes no record, got {none:?}"
        );
    }
}
