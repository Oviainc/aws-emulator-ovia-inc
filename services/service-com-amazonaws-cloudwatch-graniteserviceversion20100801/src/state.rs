// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudwatch-graniteserviceversion20100801 service.
//!
//! Three stores plus the helpers the metric, alarm and metric-stream handlers share:
//!
//! * [`ComAmazonawsCloudwatchGraniteserviceversion20100801State::metrics`] — the ingested
//!   time series, keyed by (namespace, metric name, normalised dimension set), exactly as
//!   `PutMetricData` creates them and `ListMetrics` / `GetMetricStatistics` read them back.
//! * [`ComAmazonawsCloudwatchGraniteserviceversion20100801State::alarms`] and
//!   [`ComAmazonawsCloudwatchGraniteserviceversion20100801State::alarm_history`] — the
//!   metric-alarm lifecycle written by `PutMetricAlarm` / `SetAlarmState` / `DeleteAlarms`
//!   and read by `DescribeAlarms` and `DescribeAlarmHistory`.
//! * [`ComAmazonawsCloudwatchGraniteserviceversion20100801State::metric_streams`] — the
//!   metric streams written by `PutMetricStream` and read back by `GetMetricStream`.
//!
//! An alarm is stored as the modelled [`MetricAlarm`] shape itself rather than a parallel
//! record, so every member `PutMetricAlarm` accepts round-trips through `DescribeAlarms`
//! without a field-by-field copy that can silently drop one. A metric stream is stored the
//! same way, as the modelled [`GetMetricStreamOutput`] its reader returns.

use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use ::serde_json::{Map, Value};

use crate::ops::types::{
    ActionConfiguration, AlarmHistoryItem, Dimension, EvaluationCriteria, EvaluationWindow,
    GetMetricStreamOutput, MetricAlarm, MetricDataQuery, Tag,
};

/// A dimension set, normalised: dimension name to value, ordered by name, so the same set
/// keys the same time series whatever order a request listed it in.
pub type DimensionSet = BTreeMap<String, String>;

/// The identity of one CloudWatch time series.
///
/// CloudWatch treats the dimension set as part of the metric's identity ("a metric is
/// uniquely defined by a name, a namespace, and zero or more dimensions"), which is why the
/// dimensions are in the key rather than on the datum.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MetricKey {
    /// The metric's namespace.
    pub namespace: String,
    /// The metric's name.
    pub metric_name: String,
    /// The metric's normalised dimension set.
    pub dimensions: DimensionSet,
}

/// One stored observation of a time series.
///
/// Every `PutMetricData` form folds into these four aggregate fields, so
/// `GetMetricStatistics` has a single aggregation path:
///
/// * a plain `Value` becomes `sample_count = 1`, `sum = minimum = maximum = value`;
/// * a `StatisticValues` statistic set is stored as given;
/// * each entry of `Values`/`Counts` becomes its own datum with `sample_count = count`,
///   `sum = value * count` and `minimum = maximum = value`, which keeps the individual
///   published values recoverable for percentile statistics.
#[derive(Debug, Clone)]
pub struct StoredDatum {
    /// The observation's time stamp, as epoch seconds.
    pub timestamp_secs: f64,
    /// How many observations this datum stands for.
    pub sample_count: f64,
    /// The sum of the observations this datum stands for.
    pub sum: f64,
    /// The smallest of the observations this datum stands for.
    pub minimum: f64,
    /// The largest of the observations this datum stands for.
    pub maximum: f64,
    /// The unit the publisher declared, when it declared one.
    pub unit: Option<String>,
    /// The publisher's `StorageResolution`, in seconds (1 or 60).
    pub storage_resolution: i32,
}

impl StoredDatum {
    /// The one value every observation in this datum took, when that value is known.
    ///
    /// `GetMetricStatistics` documents exactly when a stored aggregate still yields the
    /// individual values a percentile needs: "CloudWatch needs raw data points to calculate
    /// percentile statistics. If you publish data using a statistic set instead, you can
    /// only retrieve percentile statistics for this data if one of the following conditions
    /// is true: The SampleCount value of the statistic set is 1. The Min and the Max values
    /// of the statistic set are equal." `None` is an opaque aggregate whose individual
    /// values were never published, so no percentile over it can be computed rather than
    /// invented.
    #[allow(clippy::float_cmp)]
    pub fn single_value(&self) -> Option<f64> {
        if self.minimum == self.maximum {
            Some(self.minimum)
        } else if self.sample_count == 1.0 {
            Some(self.sum)
        } else {
            None
        }
    }
}

/// The basic statistics of a span of ONE series' datapoints.
///
/// One fold over the four aggregates every [`StoredDatum`] carries, so every reader that
/// aggregates a span of a series — `GetMetricStatistics`' per-period windows, and the
/// hourly load buckets Application Auto Scaling's predictive forecast is computed from —
/// computes a statistic the same way and the two cannot drift. The names are the model's
/// own `Statistic` enum values (`STATISTIC_VALUES`); the EXTENDED (percentile) statistics
/// are deliberately not part of this fold, because they need the individual published
/// values rather than these aggregates.
#[derive(Debug, Clone, Copy)]
pub struct BasicStatistics {
    /// How many observations the span stands for.
    pub sample_count: f64,
    /// The sum of those observations.
    pub sum: f64,
    /// The smallest of them, `f64::INFINITY` over no observation at all.
    pub minimum: f64,
    /// The largest of them, `f64::NEG_INFINITY` over no observation at all.
    pub maximum: f64,
}

/// The fold's identity: no observation, and the two infinities a min/max fold starts from.
impl Default for BasicStatistics {
    fn default() -> Self {
        Self {
            sample_count: 0.0,
            sum: 0.0,
            minimum: f64::INFINITY,
            maximum: f64::NEG_INFINITY,
        }
    }
}

impl BasicStatistics {
    /// Fold every datapoint of a span.
    pub fn of<'a>(data: impl IntoIterator<Item = &'a StoredDatum>) -> Self {
        let mut folded = Self::default();
        for datum in data {
            folded.add(datum);
        }
        folded
    }

    /// Fold one more datapoint into the span.
    pub fn add(&mut self, datum: &StoredDatum) {
        self.sample_count += datum.sample_count;
        self.sum += datum.sum;
        self.minimum = self.minimum.min(datum.minimum);
        self.maximum = self.maximum.max(datum.maximum);
    }

    /// The mean of the observations, when there is one: the sum over the sample count,
    /// which no observation at all does not have.
    pub fn average(&self) -> Option<f64> {
        let average = self.sum / self.sample_count;
        average.is_finite().then_some(average)
    }

    /// One statistic by the name the model gives it, or `None` for a name outside the basic
    /// five and for a statistic no observation in the span defines.
    pub fn named(&self, statistic: &str) -> Option<f64> {
        match statistic {
            "SampleCount" => Some(self.sample_count),
            "Sum" => Some(self.sum),
            "Average" => self.average(),
            "Minimum" => self.minimum.is_finite().then_some(self.minimum),
            "Maximum" => self.maximum.is_finite().then_some(self.maximum),
            _ => None,
        }
    }
}

/// A stored metric alarm: the modelled shape `DescribeAlarms` returns, plus the tags
/// `PutMetricAlarm` accepted (no `MetricAlarm` member carries them).
#[derive(Debug, Clone, Default)]
pub struct StoredAlarm {
    /// The alarm as `DescribeAlarms` reports it.
    pub alarm: MetricAlarm,
    /// The tags `PutMetricAlarm` was given for this alarm.
    pub tags: Vec<Tag>,
}

/// A stored alarm history item: the modelled shape `DescribeAlarmHistory` returns, plus the
/// store-wide sequence number it was recorded under.
///
/// The sequence number is the item's STABLE identity. It is what `DescribeAlarmHistory`
/// pages on: it never changes once assigned, so a cursor minted from it keeps naming the
/// same item even after retention has removed older items, which a position in the vector
/// would not. It also breaks a tie between two items recorded in the same instant, so the
/// reporting order is total.
#[derive(Debug, Clone, Default)]
pub struct StoredHistoryItem {
    /// The order this item was recorded in, across every alarm.
    pub sequence: u64,
    /// The item as `DescribeAlarmHistory` reports it.
    pub item: AlarmHistoryItem,
}

/// A stored metric stream: the modelled shape `GetMetricStream` returns, plus the tags
/// `PutMetricStream` accepted (no `GetMetricStreamOutput` member carries them).
#[derive(Debug, Clone, Default)]
pub struct StoredMetricStream {
    /// The stream as `GetMetricStream` reports it.
    pub stream: GetMetricStreamOutput,
    /// The tags `PutMetricStream` was given when this stream was created.
    pub tags: Vec<Tag>,
}

/// Per-service state for com-amazonaws-cloudwatch-graniteserviceversion20100801. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCloudwatchGraniteserviceversion20100801State {
    /// Every time series any `PutMetricData` call has created, with its datapoints in
    /// ingest order.
    pub metrics: HashMap<MetricKey, Vec<StoredDatum>>,
    /// Metric alarms by alarm name.
    pub alarms: HashMap<String, StoredAlarm>,
    /// Alarm history by alarm name, in append order.
    pub alarm_history: HashMap<String, Vec<StoredHistoryItem>>,
    /// The sequence number the next recorded history item takes.
    pub next_history_sequence: u64,
    /// Metric streams by stream name.
    pub metric_streams: HashMap<String, StoredMetricStream>,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCloudwatchGraniteserviceversion20100801State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCloudwatchGraniteserviceversion20100801State>)
}

/// Wall-clock now, as epoch seconds — the one clock the handlers read.
pub fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// Epoch seconds as the modelled timestamp type.
pub fn to_timestamp(secs: f64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs_f64(secs)
}

/// A modelled timestamp as epoch seconds.
pub fn from_timestamp(stamp: &::aws_smithy_types::DateTime) -> f64 {
    stamp.as_secs_f64()
}

/// Normalise a request's dimension list into the key form: later entries for one name win,
/// and the order the request used is discarded.
pub fn dimension_set(dimensions: Option<&[Dimension]>) -> DimensionSet {
    dimensions
        .into_iter()
        .flatten()
        .map(|dimension| (dimension.name.clone(), dimension.value.clone()))
        .collect()
}

/// Render a stored dimension set back as the modelled list, ordered by dimension name.
pub fn dimension_list(dimensions: &DimensionSet) -> Vec<Dimension> {
    dimensions
        .iter()
        .map(|(name, value)| Dimension {
            name: name.clone(),
            value: value.clone(),
        })
        .collect()
}

/// The region this request was signed for, defaulting to the emulator's own default region
/// when the request carries no SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// The region an unsigned request is answered in.
pub const DEFAULT_REGION: &str = "us-east-1";

/// A metric alarm's ARN: `arn:aws:cloudwatch:<region>:<account>:alarm:<AlarmName>`.
pub fn alarm_arn(region: &str, alarm_name: &str) -> String {
    ::engine::emulator_arn("cloudwatch", region, &format!("alarm:{alarm_name}"))
}

/// A metric stream's ARN: `arn:aws:cloudwatch:<region>:<account>:metric-stream/<Name>`.
pub fn metric_stream_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("cloudwatch", region, &format!("metric-stream/{name}"))
}

/// A running metric stream: "The state of the metric stream. The possible values are
/// `running` and `stopped`." (`GetMetricStreamOutput$State`) — `PutMetricStream` creates a
/// stream in this state, and only `StopMetricStreams` moves it out of it.
pub const METRIC_STREAM_RUNNING: &str = "running";

/// Encode a pagination cursor — the key the NEXT page starts at — as an opaque token.
///
/// Hex keeps the token opaque to the caller and, unlike the bare key, makes a token the
/// service did not mint detectable, which is what lets a paginating operation refuse a
/// malformed `NextToken` instead of silently treating it as a key.
pub fn encode_page_token(start_at: &str) -> String {
    ::hex::encode(start_at.as_bytes())
}

/// Decode a pagination cursor minted by [`encode_page_token`]; `None` for anything else.
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// The largest number of distinct time series this in-memory store holds.
///
/// A capacity bound, not an AWS quota: the store lives in this process's memory, so the
/// number of series a caller can create has to be finite. `PutMetricData` answers
/// `LimitExceededFault` rather than growing without limit once it is reached; appending to
/// a series that already exists is unaffected.
pub const MAX_SERIES: usize = 20_000;

/// The largest number of datapoints retained per series. Reaching it evicts the OLDEST
/// datapoints, the same direction CloudWatch's own retention ages data out in, so a
/// long-running emulator's memory stays bounded while recent reads stay exact.
pub const MAX_DATAPOINTS_PER_SERIES: usize = 50_000;

/// CloudWatch's published alarm quota: 5,000 alarms per Region per account — which is also
/// what `LimitExceededFault` ("The quota for alarms for this customer has already been
/// reached") reports on `PutMetricAlarm`.
pub const MAX_ALARMS: usize = 5_000;

/// The largest number of history items retained per alarm. Reaching it evicts the OLDEST,
/// so repeated configuration or state updates cannot grow memory without limit.
pub const MAX_HISTORY_ITEMS_PER_ALARM: usize = 1_000;

/// The largest number of ALARMS whose history this in-memory store holds.
///
/// A capacity bound, not an AWS quota, and a separate one from [`MAX_ALARMS`]: an alarm's
/// history outlives the alarm itself, so the number of histories is not bounded by how many
/// alarms exist and has to be bounded here — the records live in this process's memory, so
/// their number has to be finite. Reaching it evicts the history whose NEWEST item is the
/// oldest, the same direction retention ages history out in, so recent reads stay exact.
pub const MAX_ALARM_HISTORIES: usize = 10_000;

/// The largest number of metric streams this in-memory store holds.
///
/// A capacity bound, not an AWS quota: the stream records live in this process's memory, so
/// their number has to be finite. `PutMetricStream` answers its modelled server fault rather
/// than growing without limit once it is reached; updating a stream that already exists adds
/// none, so it is unaffected.
pub const MAX_METRIC_STREAMS: usize = 10_000;

/// How long an alarm history item is retained: CloudWatch keeps alarm history for 14 days.
pub const ALARM_HISTORY_RETENTION_SECS: f64 = 14.0 * 24.0 * 60.0 * 60.0;

impl ComAmazonawsCloudwatchGraniteserviceversion20100801State {
    /// Append `data` to the series `key` names, creating it when it does not exist.
    ///
    /// `Err(())` when the store already holds [`MAX_SERIES`] series and `key` names a new
    /// one; the caller turns that into its own modelled limit error. The series is trimmed
    /// to [`MAX_DATAPOINTS_PER_SERIES`], oldest first.
    pub fn append_datapoints(
        &mut self,
        key: MetricKey,
        data: Vec<StoredDatum>,
    ) -> Result<(), AtCapacity> {
        let at_capacity = self.metrics.len() >= MAX_SERIES;
        let series = match self.metrics.entry(key) {
            std::collections::hash_map::Entry::Occupied(series) => series.into_mut(),
            std::collections::hash_map::Entry::Vacant(slot) => {
                if at_capacity {
                    return Err(AtCapacity);
                }
                slot.insert(Vec::new())
            }
        };
        series.extend(data);
        if series.len() > MAX_DATAPOINTS_PER_SERIES {
            series.drain(..series.len() - MAX_DATAPOINTS_PER_SERIES);
        }
        Ok(())
    }

    /// Record an alarm history item, read back by `DescribeAlarmHistory`.
    ///
    /// The history is bounded on every append: THIS alarm's items past the retention window
    /// are dropped and what remains for it is trimmed to [`MAX_HISTORY_ITEMS_PER_ALARM`],
    /// oldest first, while the store-wide sweep stays on the read path
    /// ([`Self::expire_history`]) so one append never re-scans every retained item.
    pub fn record_history(&mut self, alarm_name: &str, now_secs: f64, item: AlarmHistoryItem) {
        // A history for an alarm this store holds none for yet needs a slot, so make one by
        // evicting the least recently written history when the store is already full.
        while !self.alarm_history.contains_key(alarm_name)
            && self.alarm_history.len() >= MAX_ALARM_HISTORIES
            && self.evict_oldest_history()
        {}
        let sequence = self.next_history_sequence;
        self.next_history_sequence = sequence.wrapping_add(1);
        let history = self
            .alarm_history
            .entry(alarm_name.to_string())
            .or_default();
        history.retain(|stored| within_retention(stored, now_secs));
        history.push(StoredHistoryItem { sequence, item });
        if history.len() > MAX_HISTORY_ITEMS_PER_ALARM {
            history.drain(..history.len() - MAX_HISTORY_ITEMS_PER_ALARM);
        }
    }

    /// Drop the history whose newest item is the oldest, reporting whether one was dropped.
    ///
    /// The capacity valve behind [`MAX_ALARM_HISTORIES`]: retention alone cannot bound the
    /// NUMBER of histories, because a history is retained for its full window even after its
    /// alarm is deleted.
    fn evict_oldest_history(&mut self) -> bool {
        let stalest = self
            .alarm_history
            .iter()
            .min_by(|left, right| newest_sequence(left.1).cmp(&newest_sequence(right.1)))
            .map(|(alarm_name, _)| alarm_name.clone());
        stalest.is_some_and(|alarm_name| self.alarm_history.remove(&alarm_name).is_some())
    }

    /// Drop every history item older than [`ALARM_HISTORY_RETENTION_SECS`], and every alarm
    /// whose history is empty once they are gone.
    ///
    /// This is an expiry the CLOCK already made due: an item 14 days old has aged out at
    /// this instant whether or not anybody called, which is why a reporting operation may
    /// materialise it. History outlives its alarm (`DescribeAlarmHistory`: "CloudWatch
    /// retains the history of an alarm even if you delete the alarm"), so retention is the
    /// only thing that ever removes it.
    pub fn expire_history(&mut self, now_secs: f64) {
        self.alarm_history.retain(|_, history| {
            history.retain(|stored| within_retention(stored, now_secs));
            !history.is_empty()
        });
    }
}

/// Whether one stored history item is still inside [`ALARM_HISTORY_RETENTION_SECS`] at
/// `now_secs`. An item carrying no time stamp has no age to age out of.
fn within_retention(stored: &StoredHistoryItem, now_secs: f64) -> bool {
    stored
        .item
        .timestamp
        .as_ref()
        .is_none_or(|stamp| now_secs - from_timestamp(stamp) <= ALARM_HISTORY_RETENTION_SECS)
}

/// The sequence number of the most recently recorded item in one alarm's history; `0` for an
/// empty history, which sorts it first for eviction.
fn newest_sequence(history: &[StoredHistoryItem]) -> u64 {
    history.last().map_or(0, |stored| stored.sequence)
}

/// The store cannot hold another resource of this kind; the caller answers with its own
/// modelled limit error.
#[derive(Debug, Clone, Copy)]
pub struct AtCapacity;

// ---------------------------------------------------------------------------
// Alarm history documents (`AlarmHistoryItem$HistoryData`)
// ---------------------------------------------------------------------------
//
// Every history item an alarm writer records carries `HistoryData`: "Data about the alarm, in
// JSON format." (`smithy_query(query="shape",shape="AlarmHistoryItem")`), a `HistoryData`
// string with `@length 1..4095`. It is the JSON record of the change the item reports, so it
// is built HERE, from the alarm as the store holds it before and after the write, and shared
// by every writer — one definition, so two writers cannot record two different shapes of
// document.

/// The modelled maximum length of one history document: `AlarmHistoryItem$HistoryData` targets
/// `HistoryData`, a string with `@length 1..4095`.
pub const MAX_HISTORY_DATA_LEN: usize = 4095;

/// The schema version every history document this service records declares, so a consumer can
/// tell which layout it is reading.
const HISTORY_DATA_VERSION: &str = "1.0";

/// A history document's `type` for an alarm the call CREATED.
const CONFIGURATION_CREATE: &str = "Create";

/// A history document's `type` for an alarm the call OVERWROTE.
const CONFIGURATION_UPDATE: &str = "Update";

/// The JSON record of a state transition: the alarm's state before and after the write.
///
/// A consumer of alarm history reads the state a transition moved the alarm to out of this
/// document (`HistoryData` → `newState` → `stateValue`), which is why both states are named
/// rather than only the reason text the summary already carries.
pub fn state_update_history_data(previous: &MetricAlarm, updated: &MetricAlarm) -> String {
    // `StateReasonData` is itself up to 4000 characters, so two of them need not fit the
    // modelled bound: the second candidate is the same document without them, which always
    // does (a state value plus two reasons of at most 1023 each).
    bounded_history_data(&[
        transition_document(previous, updated, true),
        transition_document(previous, updated, false),
    ])
}

/// The JSON record of a configuration write: the alarm the call stored, and the configuration
/// it replaced when the call overwrote an alarm that already existed.
pub fn configuration_update_history_data(
    previous: Option<&MetricAlarm>,
    updated: &MetricAlarm,
) -> String {
    let (kind, stored_member) = match previous {
        Some(_) => (CONFIGURATION_UPDATE, "updatedAlarm"),
        None => (CONFIGURATION_CREATE, "createdAlarm"),
    };
    let heading = |document: Value| -> Value {
        Value::Object(
            [
                ("version".to_string(), Value::from(HISTORY_DATA_VERSION)),
                ("type".to_string(), Value::from(kind)),
                (stored_member.to_string(), document),
            ]
            .into_iter()
            .collect(),
        )
    };
    // An alarm's configuration is itself unbounded in the modelled sense (up to 20 metric
    // queries, each with an expression of up to 2048 characters), so the candidates drop
    // fidelity in order: both configurations, then only the stored one, then the name of the
    // alarm the item already belongs to — which always fits.
    let mut full = heading(alarm_configuration_document(updated));
    if let (Some(previous), Value::Object(members)) = (previous, &mut full) {
        members.insert(
            "originalAlarm".to_string(),
            alarm_configuration_document(previous),
        );
    }
    bounded_history_data(&[
        full,
        heading(alarm_configuration_document(updated)),
        heading(Value::Object(
            [(
                "alarmName".to_string(),
                Value::from(updated.alarm_name.clone()),
            )]
            .into_iter()
            .collect(),
        )),
    ])
}

/// One state-transition document, with or without the (potentially long) reason data.
fn transition_document(previous: &MetricAlarm, updated: &MetricAlarm, reason_data: bool) -> Value {
    Value::Object(
        [
            ("version".to_string(), Value::from(HISTORY_DATA_VERSION)),
            (
                "oldState".to_string(),
                alarm_state_document(previous, reason_data),
            ),
            (
                "newState".to_string(),
                alarm_state_document(updated, reason_data),
            ),
        ]
        .into_iter()
        .collect(),
    )
}

/// One alarm state as a transition document records it.
///
/// `stateReasonData` is itself JSON ("The reason that this alarm is set to this specific state,
/// in JSON format." — `MetricAlarm$StateReasonData`), so it is embedded as JSON rather than as
/// a quoted string whenever it parses as JSON.
fn alarm_state_document(alarm: &MetricAlarm, reason_data: bool) -> Value {
    let mut state = Map::new();
    insert_member(&mut state, "stateValue", json_text(&alarm.state_value));
    insert_member(&mut state, "stateReason", json_text(&alarm.state_reason));
    if reason_data {
        insert_member(
            &mut state,
            "stateReasonData",
            alarm.state_reason_data.as_deref().map(embedded_json),
        );
    }
    Value::Object(state)
}

/// An alarm's configuration as a history document records it: every configuration member the
/// alarm carries, under the model's member name in lower camel case, and only those it carries
/// — a member the alarm does not have is absent from the document, exactly as it is absent
/// from the alarm. The state members are NOT part of it: "When you update an existing alarm,
/// its state is left unchanged" (`PutMetricAlarm`), so a configuration write records no state.
fn alarm_configuration_document(alarm: &MetricAlarm) -> Value {
    let mut config = Map::new();
    insert_member(&mut config, "alarmName", json_text(&alarm.alarm_name));
    insert_member(&mut config, "alarmArn", json_text(&alarm.alarm_arn));
    insert_member(
        &mut config,
        "alarmDescription",
        json_text(&alarm.alarm_description),
    );
    insert_member(
        &mut config,
        "actionsEnabled",
        alarm.actions_enabled.map(Value::from),
    );
    insert_member(&mut config, "okActions", json_strings(&alarm.ok_actions));
    insert_member(
        &mut config,
        "alarmActions",
        json_strings(&alarm.alarm_actions),
    );
    insert_member(
        &mut config,
        "insufficientDataActions",
        json_strings(&alarm.insufficient_data_actions),
    );
    insert_member(&mut config, "namespace", json_text(&alarm.namespace));
    insert_member(&mut config, "metricName", json_text(&alarm.metric_name));
    insert_member(&mut config, "statistic", json_text(&alarm.statistic));
    insert_member(
        &mut config,
        "extendedStatistic",
        json_text(&alarm.extended_statistic),
    );
    insert_member(
        &mut config,
        "dimensions",
        alarm.dimensions.as_deref().map(dimensions_document),
    );
    insert_member(&mut config, "period", alarm.period.map(Value::from));
    insert_member(&mut config, "unit", json_text(&alarm.unit));
    insert_member(
        &mut config,
        "evaluationPeriods",
        alarm.evaluation_periods.map(Value::from),
    );
    insert_member(
        &mut config,
        "datapointsToAlarm",
        alarm.datapoints_to_alarm.map(Value::from),
    );
    insert_member(&mut config, "threshold", alarm.threshold.map(Value::from));
    insert_member(
        &mut config,
        "comparisonOperator",
        json_text(&alarm.comparison_operator),
    );
    insert_member(
        &mut config,
        "treatMissingData",
        json_text(&alarm.treat_missing_data),
    );
    insert_member(
        &mut config,
        "evaluateLowSampleCountPercentile",
        json_text(&alarm.evaluate_low_sample_count_percentile),
    );
    insert_member(
        &mut config,
        "thresholdMetricId",
        json_text(&alarm.threshold_metric_id),
    );
    insert_member(
        &mut config,
        "metrics",
        alarm.metrics.as_deref().map(|queries| {
            Value::Array(queries.iter().map(metric_query_document).collect())
        }),
    );
    insert_member(
        &mut config,
        "evaluationInterval",
        alarm.evaluation_interval.map(Value::from),
    );
    insert_member(
        &mut config,
        "evaluationWindow",
        alarm.evaluation_window.as_ref().map(evaluation_window_document),
    );
    insert_member(
        &mut config,
        "warmUpConfiguration",
        alarm.warm_up_configuration.as_ref().map(|warm_up| {
            let mut document = Map::new();
            document.insert(
                "warmUpPeriodDurationInMinutes".to_string(),
                Value::from(warm_up.warm_up_period_duration_in_minutes),
            );
            insert_member(
                &mut document,
                "onlyStartEvaluatingAfterWarmUpPeriodEnds",
                warm_up
                    .only_start_evaluating_after_warm_up_period_ends
                    .map(Value::from),
            );
            Value::Object(document)
        }),
    );
    insert_member(
        &mut config,
        "evaluationCriteria",
        alarm
            .evaluation_criteria
            .as_ref()
            .map(evaluation_criteria_document),
    );
    insert_member(
        &mut config,
        "actionConfiguration",
        alarm
            .action_configuration
            .as_ref()
            .map(action_configuration_document),
    );
    Value::Object(config)
}

/// A dimension list as a history document records it.
fn dimensions_document(dimensions: &[Dimension]) -> Value {
    Value::Array(
        dimensions
            .iter()
            .map(|dimension| {
                Value::Object(
                    [
                        ("name".to_string(), Value::from(dimension.name.clone())),
                        ("value".to_string(), Value::from(dimension.value.clone())),
                    ]
                    .into_iter()
                    .collect(),
                )
            })
            .collect(),
    )
}

/// One metric-math query of a `Metrics` alarm as a history document records it.
fn metric_query_document(query: &MetricDataQuery) -> Value {
    let mut document = Map::new();
    document.insert("id".to_string(), Value::from(query.id.clone()));
    insert_member(&mut document, "expression", json_text(&query.expression));
    insert_member(&mut document, "label", json_text(&query.label));
    insert_member(
        &mut document,
        "returnData",
        query.return_data.map(Value::from),
    );
    insert_member(&mut document, "period", query.period.map(Value::from));
    insert_member(&mut document, "accountId", json_text(&query.account_id));
    insert_member(
        &mut document,
        "metricStat",
        query.metric_stat.as_ref().map(|stat| {
            let mut metric = Map::new();
            insert_member(&mut metric, "namespace", json_text(&stat.metric.namespace));
            insert_member(
                &mut metric,
                "metricName",
                json_text(&stat.metric.metric_name),
            );
            insert_member(
                &mut metric,
                "dimensions",
                stat.metric.dimensions.as_deref().map(dimensions_document),
            );
            let mut document = Map::new();
            document.insert("metric".to_string(), Value::Object(metric));
            document.insert("period".to_string(), Value::from(stat.period));
            document.insert("stat".to_string(), Value::from(stat.stat.clone()));
            insert_member(&mut document, "unit", json_text(&stat.unit));
            Value::Object(document)
        }),
    );
    Value::Object(document)
}

/// An `EvaluationWindow` union as a history document records it: the one member it carries.
fn evaluation_window_document(window: &EvaluationWindow) -> Value {
    let (member, document) = match window {
        EvaluationWindow::WallClockWindow(wall_clock) => {
            let mut inner = Map::new();
            insert_member(&mut inner, "timezone", json_text(&wall_clock.timezone));
            ("wallClockWindow", Value::Object(inner))
        }
        EvaluationWindow::SlidingWindow(_) => ("slidingWindow", Value::Object(Map::new())),
    };
    Value::Object([(member.to_string(), document)].into_iter().collect())
}

/// An `EvaluationCriteria` union as a history document records it.
fn evaluation_criteria_document(criteria: &EvaluationCriteria) -> Value {
    let EvaluationCriteria::PromQLCriteria(promql) = criteria;
    let mut inner = Map::new();
    inner.insert("query".to_string(), Value::from(promql.query.clone()));
    insert_member(
        &mut inner,
        "pendingPeriod",
        promql.pending_period.map(Value::from),
    );
    insert_member(
        &mut inner,
        "recoveryPeriod",
        promql.recovery_period.map(Value::from),
    );
    Value::Object(
        [("promQLCriteria".to_string(), Value::Object(inner))]
            .into_iter()
            .collect(),
    )
}

/// An `ActionConfiguration` as a history document records it.
fn action_configuration_document(configuration: &ActionConfiguration) -> Value {
    let mut document = Map::new();
    insert_member(
        &mut document,
        "alarmTagsPropagation",
        configuration
            .alarm_tags_propagation
            .as_ref()
            .map(|propagation| {
                let mut inner = Map::new();
                inner.insert("enabled".to_string(), Value::from(propagation.enabled));
                insert_member(
                    &mut inner,
                    "includedTagKeys",
                    json_strings(&propagation.included_tag_keys),
                );
                insert_member(
                    &mut inner,
                    "excludedTagKeys",
                    json_strings(&propagation.excluded_tag_keys),
                );
                Value::Object(inner)
            }),
    );
    Value::Object(document)
}

/// The first candidate whose rendering fits the modelled `HistoryData` length.
///
/// A document is JSON, so it can never be truncated to fit: a reader parses it. Dropping the
/// longest part and re-rendering is what keeps it both parseable and inside
/// [`MAX_HISTORY_DATA_LEN`]; the last candidate a caller passes is one that always fits, and
/// the bare version object is the floor if even that one does not.
fn bounded_history_data(candidates: &[Value]) -> String {
    for candidate in candidates {
        let rendered = candidate.to_string();
        if (1..=MAX_HISTORY_DATA_LEN).contains(&rendered.len()) {
            return rendered;
        }
    }
    Value::Object(
        [("version".to_string(), Value::from(HISTORY_DATA_VERSION))]
            .into_iter()
            .collect(),
    )
    .to_string()
}

/// Add `value` to a document under `member`, and nothing at all when there is no value.
fn insert_member(document: &mut Map<String, Value>, member: &str, value: Option<Value>) {
    if let Some(value) = value {
        document.insert(member.to_string(), value);
    }
}

/// An optional string member as a JSON string.
fn json_text(value: &Option<String>) -> Option<Value> {
    value.as_deref().map(Value::from)
}

/// An optional string list as a JSON array of strings.
fn json_strings(values: &Option<Vec<String>>) -> Option<Value> {
    values
        .as_deref()
        .map(|values| Value::Array(values.iter().map(|value| Value::from(value.as_str())).collect()))
}

/// A member whose stored value is itself JSON, embedded as JSON when it parses as JSON and as
/// the string the writer stored when it does not — a caller may store anything in
/// `StateReasonData`, and a history document has to stay parseable either way.
fn embedded_json(raw: &str) -> Value {
    ::serde_json::from_str(raw).unwrap_or_else(|_| Value::from(raw))
}
