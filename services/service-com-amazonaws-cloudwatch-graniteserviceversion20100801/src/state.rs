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
//! * [`ComAmazonawsCloudwatchGraniteserviceversion20100801State::composite_alarms`] — the
//!   composite alarms written by `PutCompositeAlarm`, whose state is EVALUATED from their
//!   `AlarmRule` over the alarms the rule references
//!   ([`ComAmazonawsCloudwatchGraniteserviceversion20100801State::evaluate_composite_alarms`]).
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
    ActionConfiguration, AlarmHistoryItem, CompositeAlarm, Dimension, EvaluationCriteria,
    EvaluationWindow, GetMetricStreamOutput, MetricAlarm, MetricDataQuery, Tag,
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

/// A stored composite alarm: the modelled shape `DescribeAlarms` returns, plus what the
/// store has to remember about it that no `CompositeAlarm` member carries — its `AlarmRule`
/// as the writer PARSED it, the tags `PutCompositeAlarm` accepted, when its next rule
/// evaluation is owed, and whether its state is currently one `SetAlarmState` forced.
#[derive(Debug, Clone)]
pub struct StoredCompositeAlarm {
    /// The alarm as `DescribeAlarms` reports it.
    pub alarm: CompositeAlarm,
    /// The alarm's `AlarmRule`, parsed once by the write that stored it.
    ///
    /// The rule TEXT is kept on the alarm too, because `DescribeAlarms` reports it, but
    /// nothing re-parses it: neither an evaluation nor a relationship read repeats the
    /// parse, and a stored rule is always one a writer already accepted inside the parser's
    /// bounds.
    pub rule: AlarmRule,
    /// The tags `PutCompositeAlarm` was given for this alarm.
    pub tags: Vec<Tag>,
    /// The instant, in epoch seconds, from which this alarm's rule evaluation is owed.
    ///
    /// "When this operation creates an alarm, the alarm state is immediately set to
    /// INSUFFICIENT_DATA. The alarm is then evaluated and its state is set appropriately."
    /// (`PutCompositeAlarm`) — so a write starts a span during which the alarm still reports
    /// the state the write left it in, and the evaluation becomes due at its end.
    pub evaluation_due_secs: f64,
    /// The states of the alarms this one's rule references, as they stood when
    /// `SetAlarmState` last forced this alarm's state; `None` when the alarm's state is its
    /// own evaluated one.
    ///
    /// "If you use SetAlarmState on a composite alarm, the composite alarm is not guaranteed
    /// to return to its actual state. It returns to its actual state only once any of its
    /// children alarms change state." (`SetAlarmState`) — comparing this snapshot against
    /// the children's current states is what detects that change.
    pub forced_child_states: Option<Vec<(String, Option<String>)>>,
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
    /// Composite alarms by alarm name.
    pub composite_alarms: HashMap<String, StoredCompositeAlarm>,
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

/// The most evaluation passes over the composite alarms ONE call may run.
///
/// A composite alarm's rule can reference another composite alarm, so a state change
/// propagates one level of such a chain per pass. This bounds the work one request can ask
/// for: at most this many passes over the stored composite alarms, whatever chain a caller
/// has built. A chain deeper than the bound is not answered wrongly, only settled later —
/// each following call carries it one more level, and the evaluation is idempotent.
const MAX_COMPOSITE_EVALUATION_PASSES: usize = 8;

/// `OK`, one of the three modelled `StateValue` values (`STATE_VALUE_VALUES`).
pub const ALARM_STATE_OK: &str = "OK";

/// `ALARM`, one of the three modelled `StateValue` values (`STATE_VALUE_VALUES`).
pub const ALARM_STATE_ALARM: &str = "ALARM";

/// `INSUFFICIENT_DATA`, one of the three modelled `StateValue` values
/// (`STATE_VALUE_VALUES`).
pub const ALARM_STATE_INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// `MetricAlarm`, one of the modelled `AlarmType` values (`ALARM_TYPE_VALUES`): the kind of
/// alarm `PutMetricAlarm` creates, which is also the kind `DescribeAlarms` and
/// `DescribeAlarmHistory` report when a request names no `AlarmTypes`.
pub const ALARM_TYPE_METRIC: &str = "MetricAlarm";

/// `CompositeAlarm`, one of the modelled `AlarmType` values (`ALARM_TYPE_VALUES`): the kind
/// of alarm `PutCompositeAlarm` creates.
pub const ALARM_TYPE_COMPOSITE: &str = "CompositeAlarm";

/// `Alarm`, one of the modelled `ActionsSuppressedBy` values
/// (`ACTIONS_SUPPRESSED_BY_VALUES`): "When the value is `ALARM`, it means that the actions
/// are suppressed because the suppressor alarm is in `ALARM`" (`CompositeAlarm$ActionsSuppressedBy`).
pub const ACTIONS_SUPPRESSED_BY_ALARM: &str = "Alarm";

/// How long after a write a composite alarm's rule evaluation is owed.
///
/// `PutCompositeAlarm` creates an alarm in `INSUFFICIENT_DATA` and documents that "the alarm
/// is then evaluated and its state is set appropriately", so the evaluated state is NOT due
/// at the instant of the write: a caller reading straight after the write still sees the
/// `INSUFFICIENT_DATA` the write left behind. CloudWatch publishes no duration for that
/// gap, so this span is the emulator's own choice inside the framework's 250-750 ms window
/// for a transition the clock owes, and it is never slept on \u2014 the due instant is stored and
/// the evaluation is materialised by the first call that happens at or after it.
pub const COMPOSITE_EVALUATION_DELAY_SECS: f64 = 0.5;

/// The modelled maximum length of `StateReasonData`: `CompositeAlarm$StateReasonData` and
/// `MetricAlarm$StateReasonData` both target `StateReasonData`, a string with
/// `@length 0..4000`.
pub const MAX_STATE_REASON_DATA_LEN: usize = 4000;

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

    /// The current `StateValue` of the alarm a rule reference names, whichever kind of alarm
    /// it is; `None` when the store holds no alarm of that name.
    ///
    /// "The alarms specified in a composite alarm's rule expression can include metric alarms
    /// and other composite alarms." (`PutCompositeAlarm`), so both stores are consulted.
    pub fn alarm_state_value(&self, reference: &str) -> Option<String> {
        let name = alarm_rule_reference_name(reference);
        self.alarms
            .get(name)
            .and_then(|stored| stored.alarm.state_value.clone())
            .or_else(|| {
                self.composite_alarms
                    .get(name)
                    .and_then(|stored| stored.alarm.state_value.clone())
            })
    }

    /// The names and current states of the alarms one composite-alarm rule references, in the
    /// order the rule names them.
    pub fn child_states(&self, rule: &AlarmRule) -> Vec<(String, Option<String>)> {
        alarm_rule_child_names(rule)
            .into_iter()
            .map(|name| {
                let state = self.alarm_state_value(&name);
                (name, state)
            })
            .collect()
    }

    /// The alarms one composite alarm's rule references: "These are the metric alarms and
    /// composite alarms referenced in the AlarmRule field of the composite alarm"
    /// (`DescribeAlarmsInput$ChildrenOfAlarmName`). Empty for an alarm this store does not
    /// hold as a composite alarm.
    pub fn composite_alarm_children(&self, name: &str) -> Vec<String> {
        self.composite_alarms
            .get(name)
            .map(|stored| alarm_rule_child_names(&stored.rule))
            .unwrap_or_default()
    }

    /// The composite alarms whose rule references `name`: "These are the composite alarms
    /// that have AlarmRule parameters that reference the alarm named in ParentsOfAlarmName"
    /// (`DescribeAlarmsInput$ParentsOfAlarmName`) — the alarm itself is never among them.
    pub fn composite_alarm_parents(&self, name: &str) -> Vec<String> {
        let mut parents: Vec<String> = self
            .composite_alarms
            .iter()
            .filter(|(parent, stored)| {
                parent.as_str() != name
                    && alarm_rule_child_names(&stored.rule)
                        .iter()
                        .any(|child| child == name)
            })
            .map(|(parent, _)| parent.clone())
            .collect();
        parents.sort();
        parents
    }

    /// Materialise every composite-alarm rule evaluation the clock has already made due.
    ///
    /// A composite alarm's state is not a value a caller writes: it is "the rule that this
    /// alarm uses to evaluate its alarm state" (`CompositeAlarm$AlarmRule`) applied to the
    /// states of the alarms the rule references, so it is COMPUTED here from the alarms the
    /// store holds. The write is one the clock owed: each evaluation becomes due
    /// [`COMPOSITE_EVALUATION_DELAY_SECS`] after the call that configured the alarm, and
    /// from then on the outcome depends only on the stored alarm states — so whichever call
    /// materialises it, every later caller sees the same thing.
    pub fn evaluate_composite_alarms(&mut self, now_secs: f64) {
        // A composite alarm's rule can reference another composite alarm, so one alarm's new
        // state can change its parent's outcome too; one pass settles one level of that
        // chain. The number of passes is bounded by [`MAX_COMPOSITE_EVALUATION_PASSES`] as
        // well as by the number of composite alarms, so the work ONE call can do is bounded
        // by that constant times the alarms the store holds times the nodes a rule may
        // carry. The bound is also what stops a CYCLE of composite alarms: "the evaluation
        // of composite alarms stops if CloudWatch detects a cycle in the evaluation path."
        let passes = self
            .composite_alarms
            .len()
            .min(MAX_COMPOSITE_EVALUATION_PASSES);
        for _ in 0..passes {
            if !self.evaluate_composite_alarms_once(now_secs) {
                break;
            }
        }
    }

    /// One evaluation pass over every composite alarm, reporting whether any alarm's state
    /// value changed. Alarms are visited in name order, so the pass is deterministic.
    fn evaluate_composite_alarms_once(&mut self, now_secs: f64) -> bool {
        let mut names: Vec<String> = self.composite_alarms.keys().cloned().collect();
        names.sort();
        let mut changed = false;
        for name in names {
            changed |= self.evaluate_composite_alarm(&name, now_secs);
        }
        changed
    }

    /// Evaluate ONE composite alarm, reporting whether its state value changed.
    fn evaluate_composite_alarm(&mut self, name: &str, now_secs: f64) -> bool {
        // Everything this evaluation reads is read before anything is written.
        let Some(stored) = self.composite_alarms.get(name) else {
            return false;
        };
        if now_secs < stored.evaluation_due_secs {
            return false;
        }
        let rule = &stored.rule;
        let previous = AlarmStateSnapshot::of_composite_alarm(&stored.alarm);
        let forced = stored.forced_child_states.clone();
        let suppressor = stored.alarm.actions_suppressor.clone();
        let children = self.child_states(rule);
        // "It returns to its actual state only once any of its children alarms change state."
        // (`SetAlarmState`) — until then the state a caller forced stands.
        let forced_stands = forced.is_some_and(|snapshot| snapshot == children);
        let evaluated = rule.evaluate(&|reference| self.alarm_state_value(reference));
        // "Actions will be suppressed if the suppressor alarm is in the ALARM state."
        // (`CompositeAlarm$ActionsSuppressor`) — a reported fact about the suppressor's
        // current state, so it is derived here rather than stored by the writer.
        let suppressed = suppressor
            .as_deref()
            .filter(|suppressor| {
                self.alarm_state_value(suppressor).as_deref() == Some(ALARM_STATE_ALARM)
            })
            .map(|suppressor| {
                format!(
                    "Actions suppressed by alarm {}",
                    alarm_rule_reference_name(suppressor)
                )
            });

        let state = if evaluated {
            ALARM_STATE_ALARM
        } else {
            ALARM_STATE_OK
        };
        let summary = format!(
            "Alarm updated from {} to {state}",
            previous
                .state_value
                .as_deref()
                .unwrap_or(ALARM_STATE_INSUFFICIENT_DATA)
        );

        let Some(stored) = self.composite_alarms.get_mut(name) else {
            return false;
        };
        stored.alarm.actions_suppressed_by = suppressed
            .as_ref()
            .map(|_| ACTIONS_SUPPRESSED_BY_ALARM.to_string());
        stored.alarm.actions_suppressed_reason = suppressed;
        if forced_stands {
            return false;
        }
        // The evaluated state has taken over from any state a caller forced.
        stored.forced_child_states = None;
        if stored.alarm.state_value.as_deref() == Some(state) {
            return false;
        }
        stored.alarm.state_value = Some(state.to_string());
        stored.alarm.state_reason = Some(composite_state_reason(evaluated, state));
        stored.alarm.state_reason_data = Some(composite_state_reason_data(&children));
        stored.alarm.state_updated_timestamp = Some(to_timestamp(now_secs));
        // "The timestamp of the last change to the alarm's StateValue."
        // (`CompositeAlarm$StateTransitionedTimestamp`) — this call changed it.
        stored.alarm.state_transitioned_timestamp = Some(to_timestamp(now_secs));
        let updated = AlarmStateSnapshot::of_composite_alarm(&stored.alarm);
        let item = AlarmHistoryItem {
            alarm_name: Some(name.to_string()),
            alarm_contributor_id: None,
            alarm_type: Some(ALARM_TYPE_COMPOSITE.to_string()),
            timestamp: Some(to_timestamp(now_secs)),
            history_item_type: Some("StateUpdate".to_string()),
            history_summary: Some(summary),
            history_data: Some(state_transition_history_data(&previous, &updated)),
            alarm_contributor_attributes: None,
        };
        self.record_history(name, now_secs, item);
        true
    }
}

/// The reason a composite alarm reports for the state its rule evaluation put it in.
fn composite_state_reason(evaluated: bool, state: &str) -> String {
    let outcome = if evaluated { "TRUE" } else { "FALSE" };
    format!("The composite alarm's rule evaluated to {outcome}, so the alarm is in {state} state")
}

/// The JSON `StateReasonData` of a composite alarm's evaluated state: the alarms its rule
/// references and the state each one was in when the rule was evaluated.
///
/// "An explanation for the alarm state, in JSON format."
/// (`CompositeAlarm$StateReasonData`), bounded by [`MAX_STATE_REASON_DATA_LEN`] — a document
/// is JSON, so it can never be truncated to fit: the children are dropped instead when a
/// rule references more of them than the bound holds.
fn composite_state_reason_data(children: &[(String, Option<String>)]) -> String {
    let mut document = Map::new();
    document.insert("version".to_string(), Value::from(HISTORY_DATA_VERSION));
    let evaluated: Vec<Value> = children
        .iter()
        .map(|(name, state)| {
            let mut child = Map::new();
            child.insert("alarmName".to_string(), Value::from(name.as_str()));
            insert_member(&mut child, "stateValue", json_text(state));
            Value::Object(child)
        })
        .collect();
    let mut full = document.clone();
    full.insert(
        "evaluatedChildren".to_string(),
        Value::Array(evaluated),
    );
    let rendered = Value::Object(full).to_string();
    if rendered.len() <= MAX_STATE_REASON_DATA_LEN {
        return rendered;
    }
    Value::Object(document).to_string()
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

/// One alarm's state, as a history document records it.
///
/// The three state members EVERY kind of alarm carries, so one definition of a transition
/// document serves the metric alarms `SetAlarmState` writes and the composite alarms whose
/// state the rule evaluation writes, and the two cannot record two different shapes of
/// document.
#[derive(Debug, Clone, Default)]
pub struct AlarmStateSnapshot {
    /// The alarm's `StateValue`.
    pub state_value: Option<String>,
    /// The alarm's `StateReason`.
    pub state_reason: Option<String>,
    /// The alarm's `StateReasonData`.
    pub state_reason_data: Option<String>,
}

impl AlarmStateSnapshot {
    /// One metric alarm's state.
    pub fn of_metric_alarm(alarm: &MetricAlarm) -> Self {
        Self {
            state_value: alarm.state_value.clone(),
            state_reason: alarm.state_reason.clone(),
            state_reason_data: alarm.state_reason_data.clone(),
        }
    }

    /// One composite alarm's state.
    pub fn of_composite_alarm(alarm: &CompositeAlarm) -> Self {
        Self {
            state_value: alarm.state_value.clone(),
            state_reason: alarm.state_reason.clone(),
            state_reason_data: alarm.state_reason_data.clone(),
        }
    }
}

/// The JSON record of a state transition: the alarm's state before and after the write.
///
/// A consumer of alarm history reads the state a transition moved the alarm to out of this
/// document (`HistoryData` → `newState` → `stateValue`), which is why both states are named
/// rather than only the reason text the summary already carries.
pub fn state_transition_history_data(
    previous: &AlarmStateSnapshot,
    updated: &AlarmStateSnapshot,
) -> String {
    // `StateReasonData` is itself up to 4000 characters, so two of them need not fit the
    // modelled bound: the second candidate is the same document without them, which always
    // does (a state value plus two reasons of at most 1023 each).
    bounded_history_data(&[
        transition_document(previous, updated, true),
        transition_document(previous, updated, false),
    ])
}

/// The JSON record of a metric alarm's state transition.
pub fn state_update_history_data(previous: &MetricAlarm, updated: &MetricAlarm) -> String {
    state_transition_history_data(
        &AlarmStateSnapshot::of_metric_alarm(previous),
        &AlarmStateSnapshot::of_metric_alarm(updated),
    )
}

/// The JSON record of a configuration write: the alarm the call stored, and the configuration
/// it replaced when the call overwrote an alarm that already existed.
///
/// Takes the configuration documents rather than an alarm, so the metric-alarm and
/// composite-alarm writers share one record layout and one length bound.
fn configuration_history_data(
    previous: Option<Value>,
    updated: Value,
    alarm_name: Option<&str>,
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
    // queries, each with an expression of up to 2048 characters, or a rule of up to 10240),
    // so the candidates drop fidelity in order: both configurations, then only the stored
    // one, then the name of the alarm the item already belongs to — which always fits.
    let mut full = heading(updated.clone());
    if let (Some(previous), Value::Object(members)) = (previous, &mut full) {
        members.insert("originalAlarm".to_string(), previous);
    }
    bounded_history_data(&[
        full,
        heading(updated),
        heading(Value::Object(
            [(
                "alarmName".to_string(),
                alarm_name.map_or(Value::Null, Value::from),
            )]
            .into_iter()
            .collect(),
        )),
    ])
}

/// The JSON record of a METRIC alarm's configuration write.
pub fn configuration_update_history_data(
    previous: Option<&MetricAlarm>,
    updated: &MetricAlarm,
) -> String {
    configuration_history_data(
        previous.map(alarm_configuration_document),
        alarm_configuration_document(updated),
        updated.alarm_name.as_deref(),
    )
}

/// The JSON record of a COMPOSITE alarm's configuration write.
pub fn composite_configuration_update_history_data(
    previous: Option<&CompositeAlarm>,
    updated: &CompositeAlarm,
) -> String {
    configuration_history_data(
        previous.map(composite_alarm_configuration_document),
        composite_alarm_configuration_document(updated),
        updated.alarm_name.as_deref(),
    )
}

/// A composite alarm's configuration as a history document records it: every configuration
/// member the alarm carries, under the model's member name in lower camel case, and only
/// those it carries. The state members are NOT part of it, for the same reason they are not
/// part of a metric alarm's: "When you update an existing alarm, its state is left
/// unchanged" (`PutCompositeAlarm`), so a configuration write records no state.
fn composite_alarm_configuration_document(alarm: &CompositeAlarm) -> Value {
    let mut config = Map::new();
    insert_member(&mut config, "alarmName", json_text(&alarm.alarm_name));
    insert_member(&mut config, "alarmArn", json_text(&alarm.alarm_arn));
    insert_member(
        &mut config,
        "alarmDescription",
        json_text(&alarm.alarm_description),
    );
    insert_member(&mut config, "alarmRule", json_text(&alarm.alarm_rule));
    insert_member(
        &mut config,
        "actionsEnabled",
        alarm.actions_enabled.map(Value::from),
    );
    insert_member(&mut config, "okActions", json_strings(&alarm.ok_actions));
    insert_member(&mut config, "alarmActions", json_strings(&alarm.alarm_actions));
    insert_member(
        &mut config,
        "insufficientDataActions",
        json_strings(&alarm.insufficient_data_actions),
    );
    insert_member(
        &mut config,
        "actionsSuppressor",
        json_text(&alarm.actions_suppressor),
    );
    insert_member(
        &mut config,
        "actionsSuppressorWaitPeriod",
        alarm.actions_suppressor_wait_period.map(Value::from),
    );
    insert_member(
        &mut config,
        "actionsSuppressorExtensionPeriod",
        alarm.actions_suppressor_extension_period.map(Value::from),
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

/// One state-transition document, with or without the (potentially long) reason data.
fn transition_document(
    previous: &AlarmStateSnapshot,
    updated: &AlarmStateSnapshot,
    reason_data: bool,
) -> Value {
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
fn alarm_state_document(alarm: &AlarmStateSnapshot, reason_data: bool) -> Value {
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

// ---------------------------------------------------------------------------
// Composite alarm rules (`CompositeAlarm$AlarmRule`)
// ---------------------------------------------------------------------------
//
// "An expression that specifies which other alarms are to be evaluated to determine this
// composite alarm's state. … You can use operators (AND, OR and NOT) to combine multiple
// functions in a single expression. You can use parenthesis to logically group the functions
// in your expression. You can use either alarm names or ARNs to reference the other alarms
// that are to be evaluated. Functions can include the following: ALARM("alarm-name or
// alarm-ARN") is TRUE if the named alarm is in ALARM state. OK(…) … INSUFFICIENT_DATA(…) …
// TRUE always evaluates to TRUE. FALSE always evaluates to FALSE. … Alarm names specified in
// AlarmRule can be surrounded with double-quotes ("), but do not have to be."
// (`smithy_query(query="doc",shape="PutCompositeAlarmInput",member="AlarmRule")`)
//
// That grammar is CloudWatch's own — no published parser crate reads it — so it is parsed
// here, ONCE, and shared by the writer that validates a rule and the evaluation that computes
// a composite alarm's state from it. Operator precedence is the conventional one the
// documented examples rely on: NOT binds tightest, then AND, then OR.
//
// A rule is CALLER-SUPPLIED text, and both the parse and every later walk of the parsed tree
// (evaluation, the reference walk, the tree's own drop) descend with the tree, so the parser
// is bounded in THREE ways before any of that recursion can grow with a request:
//
// * LENGTH — [`MAX_ALARM_RULE_LEN`], the modelled `@length` of `AlarmRule`, re-checked here
//   so the bound holds however the parser is reached.
// * NESTING — [`MAX_RULE_NESTING_DEPTH`], the depth the PARSE descends to, counted as it
//   descends: one level per `(` and one per `NOT`, the only two sites that recurse.
// * NODES — [`MAX_RULE_NODES`], the number of functions and operators one rule may carry,
//   counted as the tree is built. A chain of `OR`s parses iteratively but builds a tree as
//   deep as the chain is long, so this is what bounds the depth of every walk of the tree.
//
// Each bound is refused as the same 400 `ValidationError` a malformed rule is refused with,
// so no rule that could overflow a thread's stack is ever parsed, stored or evaluated.

/// The longest `AlarmRule` this service parses: the modelled `@length 1..10240` of
/// `PutCompositeAlarmInput$AlarmRule`
/// (`smithy_query(query="shape",shape="PutCompositeAlarmInput")`).
///
/// The boundary validator refuses a longer one before a handler runs; it is re-checked in
/// [`parse_alarm_rule`] so the bound holds for every caller of the parser, and because it is
/// what makes the two structural bounds below reachable in one pass over the text.
pub const MAX_ALARM_RULE_LEN: usize = 10_240;

/// The deepest NESTING one `AlarmRule` may carry: the levels the parse descends through,
/// one per `(` and one per `NOT`.
///
/// The value is the EMULATOR's choice: CloudWatch's own documented rule examples group a
/// handful of levels (`(ALARM("a") OR ALARM("b")) AND NOT OK("c")` is two), so 32 is an
/// order of magnitude above any rule a caller writes, and at roughly four stack frames per
/// level it is three orders of magnitude inside a worker thread's ordinary stack.
const MAX_RULE_NESTING_DEPTH: usize = 32;

/// The most functions and operators one `AlarmRule` may carry.
///
/// This is the general depth bound. `A OR B OR C …` parses without recursing, but builds a
/// tree as deep as the chain is long, and [`AlarmRule::evaluate`],
/// [`AlarmRule::references`] and the tree's own drop all descend with that depth — so the
/// node count is what bounds them, whatever shape the rule takes.
///
/// The value is the EMULATOR's choice, above the one quota CloudWatch publishes for a rule:
/// "The rule expression of a composite alarm can include as many as 100 underlying alarms",
/// which with its operators is a few hundred nodes.
const MAX_RULE_NODES: usize = 1_024;

/// The three state functions a rule can carry. Each one's name is also the `StateValue` it
/// tests, which is why the parsed node keeps the name as the state.
const RULE_STATE_FUNCTIONS: [&str; 3] = [
    ALARM_STATE_ALARM,
    ALARM_STATE_OK,
    ALARM_STATE_INSUFFICIENT_DATA,
];

/// One node of a parsed composite-alarm rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlarmRule {
    /// `TRUE` or `FALSE`.
    Literal(bool),
    /// `ALARM("x")`, `OK("x")` or `INSUFFICIENT_DATA("x")`: true when the alarm `reference`
    /// names is in `state`.
    InState {
        /// The alarm name or ARN the function names.
        reference: String,
        /// The `StateValue` the function tests for.
        state: String,
    },
    /// `NOT <rule>`.
    Not(Box<AlarmRule>),
    /// `<rule> AND <rule>`.
    And(Box<AlarmRule>, Box<AlarmRule>),
    /// `<rule> OR <rule>`.
    Or(Box<AlarmRule>, Box<AlarmRule>),
}

impl AlarmRule {
    /// Evaluate this rule, reading each referenced alarm's current `StateValue` through
    /// `state_of`.
    ///
    /// An alarm the store does not hold is in no state, so every function over it is false:
    /// the rule still has an outcome, which is what lets a composite alarm keep evaluating
    /// when a child has been deleted.
    pub fn evaluate(&self, state_of: &impl Fn(&str) -> Option<String>) -> bool {
        match self {
            Self::Literal(value) => *value,
            Self::InState { reference, state } => {
                state_of(reference).as_deref() == Some(state.as_str())
            }
            Self::Not(inner) => !inner.evaluate(state_of),
            Self::And(left, right) => left.evaluate(state_of) && right.evaluate(state_of),
            Self::Or(left, right) => left.evaluate(state_of) || right.evaluate(state_of),
        }
    }

    /// Every alarm reference this rule carries, in the order the rule names them, without
    /// repeats.
    pub fn references(&self) -> Vec<String> {
        let mut references = Vec::new();
        self.collect_references(&mut references);
        references
    }

    fn collect_references(&self, into: &mut Vec<String>) {
        match self {
            Self::Literal(_) => {}
            Self::InState { reference, .. } => {
                if !into.iter().any(|seen| seen == reference) {
                    into.push(reference.clone());
                }
            }
            Self::Not(inner) => inner.collect_references(into),
            Self::And(left, right) | Self::Or(left, right) => {
                left.collect_references(into);
                right.collect_references(into);
            }
        }
    }
}

/// The alarm NAME a rule reference names.
///
/// "You can use either alarm names or ARNs to reference the other alarms that are to be
/// evaluated." (`PutCompositeAlarmInput$AlarmRule`) — an alarm ARN's resource part is
/// `alarm:<name>` (see [`alarm_arn`]), so both forms resolve to the name the store keys on.
pub fn alarm_rule_reference_name(reference: &str) -> &str {
    if reference.starts_with("arn:") {
        if let Some((_, name)) = reference.rsplit_once(":alarm:") {
            return name;
        }
    }
    reference
}

/// The alarm names one rule references — its children — resolved from whatever form the rule
/// used, in the order the rule names them and without repeats.
pub fn alarm_rule_child_names(rule: &AlarmRule) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for reference in rule.references() {
        let name = alarm_rule_reference_name(&reference).to_string();
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// A rule this parser cannot read, and why. The caller answers it as a refusal naming the
/// member, so the text is built from the request rather than fixed.
#[derive(Debug, Clone)]
pub struct AlarmRuleError {
    /// What was expected, and where in the rule.
    pub message: String,
}

impl std::fmt::Display for AlarmRuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Parse a composite alarm's `AlarmRule`, within the three bounds this module states.
pub fn parse_alarm_rule(rule: &str) -> Result<AlarmRule, AlarmRuleError> {
    if rule.len() > MAX_ALARM_RULE_LEN {
        return Err(AlarmRuleError {
            message: format!(
                "AlarmRule must not be longer than {MAX_ALARM_RULE_LEN} characters"
            ),
        });
    }
    let mut parser = RuleParser {
        text: rule,
        at: 0,
        depth: 0,
        nodes: 0,
    };
    let parsed = parser.parse_or()?;
    parser.skip_spaces();
    if !parser.rest().is_empty() {
        return Err(parser.error("expected the end of the rule"));
    }
    Ok(parsed)
}

/// A recursive-descent parser over one rule's text, tracking the byte offset it has reached
/// so a refusal can say WHERE the rule stopped making sense.
struct RuleParser<'a> {
    text: &'a str,
    at: usize,
    /// How many levels the parse has descended at this point, bounded by
    /// [`MAX_RULE_NESTING_DEPTH`].
    depth: usize,
    /// How many nodes the parse has built, bounded by [`MAX_RULE_NODES`].
    nodes: usize,
}

impl<'a> RuleParser<'a> {
    /// The text this parser has not consumed yet.
    fn rest(&self) -> &'a str {
        self.text.get(self.at..).unwrap_or_default()
    }

    /// Advance past any whitespace.
    fn skip_spaces(&mut self) {
        let rest = self.rest();
        self.at += rest.len() - rest.trim_start().len();
    }

    /// Consume `ch` when it is the next non-whitespace character.
    fn take(&mut self, ch: char) -> bool {
        self.skip_spaces();
        if self.rest().starts_with(ch) {
            self.at += ch.len_utf8();
            true
        } else {
            false
        }
    }

    /// Consume `word` when the next non-whitespace text is that word — case-insensitively,
    /// and only as a whole word, so `OK` never matches the start of an alarm name.
    fn take_word(&mut self, word: &str) -> bool {
        self.skip_spaces();
        let rest = self.rest();
        let Some(head) = rest.get(..word.len()) else {
            return false;
        };
        if !head.eq_ignore_ascii_case(word) {
            return false;
        }
        if rest[word.len()..].starts_with(|ch: char| ch.is_alphanumeric() || ch == '_') {
            return false;
        }
        self.at += word.len();
        true
    }

    /// A refusal naming what was expected and where.
    fn error(&self, expected: &str) -> AlarmRuleError {
        AlarmRuleError {
            message: format!("AlarmRule: {expected} at offset {}", self.at),
        }
    }

    /// Enter one more level of nesting, refusing a rule that nests past
    /// [`MAX_RULE_NESTING_DEPTH`]. Called at the only two sites the parse recurses from, so
    /// the recursion can never grow with the request past the bound.
    fn descend(&mut self) -> Result<(), AlarmRuleError> {
        self.depth += 1;
        if self.depth > MAX_RULE_NESTING_DEPTH {
            return Err(self.error(&format!(
                "must not nest more than {MAX_RULE_NESTING_DEPTH} levels deep"
            )));
        }
        Ok(())
    }

    /// Leave one level of nesting.
    fn ascend(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// Account for one more node of the tree, refusing a rule carrying more than
    /// [`MAX_RULE_NODES`] of them — which is what bounds the DEPTH of every later walk of
    /// the tree.
    fn node(&mut self, node: AlarmRule) -> Result<AlarmRule, AlarmRuleError> {
        self.nodes += 1;
        if self.nodes > MAX_RULE_NODES {
            return Err(self.error(&format!(
                "must not carry more than {MAX_RULE_NODES} functions and operators"
            )));
        }
        Ok(node)
    }

    fn parse_or(&mut self) -> Result<AlarmRule, AlarmRuleError> {
        let mut left = self.parse_and()?;
        while self.take_word("OR") {
            let right = self.parse_and()?;
            left = self.node(AlarmRule::Or(Box::new(left), Box::new(right)))?;
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<AlarmRule, AlarmRuleError> {
        let mut left = self.parse_unary()?;
        while self.take_word("AND") {
            let right = self.parse_unary()?;
            left = self.node(AlarmRule::And(Box::new(left), Box::new(right)))?;
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<AlarmRule, AlarmRuleError> {
        if self.take_word("NOT") {
            self.descend()?;
            let inner = self.parse_unary()?;
            self.ascend();
            return self.node(AlarmRule::Not(Box::new(inner)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<AlarmRule, AlarmRuleError> {
        if self.take('(') {
            self.descend()?;
            let inner = self.parse_or()?;
            self.ascend();
            if !self.take(')') {
                return Err(self.error("expected `)`"));
            }
            return Ok(inner);
        }
        if self.take_word("TRUE") {
            return self.node(AlarmRule::Literal(true));
        }
        if self.take_word("FALSE") {
            return self.node(AlarmRule::Literal(false));
        }
        for state in RULE_STATE_FUNCTIONS {
            if self.take_word(state) {
                let reference = self.parse_reference()?;
                return self.node(AlarmRule::InState {
                    reference,
                    state: state.to_string(),
                });
            }
        }
        Err(self.error("expected ALARM, OK, INSUFFICIENT_DATA, TRUE, FALSE, NOT or `(`"))
    }

    /// The alarm a state function names: `("x")` or `(x)`, since a name "can be surrounded
    /// with double-quotes (\"), but do not have to be".
    fn parse_reference(&mut self) -> Result<String, AlarmRuleError> {
        if !self.take('(') {
            return Err(self.error("expected `(` after a rule function"));
        }
        self.skip_spaces();
        let reference = if self.take('"') {
            let rest = self.rest();
            let Some(end) = rest.find('"') else {
                return Err(self.error("expected a closing double-quote"));
            };
            let name = rest[..end].to_string();
            self.at += end + 1;
            name
        } else {
            let rest = self.rest();
            let Some(end) = rest.find(')') else {
                return Err(self.error("expected `)` after an alarm reference"));
            };
            let name = rest[..end].trim_end().to_string();
            self.at += end;
            name
        };
        if !self.take(')') {
            return Err(self.error("expected `)` after an alarm reference"));
        }
        if reference.is_empty() {
            return Err(self.error("a rule function must name an alarm"));
        }
        Ok(reference)
    }
}
