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
//! * [`ComAmazonawsCloudwatchGraniteserviceversion20100801State::log_alarms`] — the log
//!   alarms written by `PutLogAlarm` and read back by `DescribeAlarms`, each carrying the
//!   configuration of the CloudWatch Logs scheduled query it evaluates.
//! * [`ComAmazonawsCloudwatchGraniteserviceversion20100801State::metric_streams`] — the
//!   metric streams written by `PutMetricStream` and read back by `GetMetricStream`.
//! * [`ComAmazonawsCloudwatchGraniteserviceversion20100801State::insight_rules`] — the
//!   Contributor Insights rules written by `PutInsightRule`, read back by
//!   `DescribeInsightRules` and removed by `DeleteInsightRules`, each holding what the log
//!   events it has evaluated contributed ([`insight_rules`]), which is what
//!   `GetInsightRuleReport` reports from.
//! * [`ComAmazonawsCloudwatchGraniteserviceversion20100801State::anomaly_detectors`] — the
//!   anomaly detection models written by `PutAnomalyDetector`, read back by
//!   `DescribeAnomalyDetectors` and removed by `DeleteAnomalyDetector`, each reported
//!   `PENDING_TRAINING` until the training its write started comes due
//!   ([`ComAmazonawsCloudwatchGraniteserviceversion20100801State::settle_anomaly_detector_training`]).
//! * [`ComAmazonawsCloudwatchGraniteserviceversion20100801State::dashboards`] — the
//!   dashboards written by `PutDashboard` and read back by `GetDashboard` and
//!   `ListDashboards`, each holding the `DashboardBody` document verbatim.
//! * [`ComAmazonawsCloudwatchGraniteserviceversion20100801State::otel_enrichment`] — the
//!   account's vended-metric enrichment configuration, written by `StartOTelEnrichment`,
//!   `UpdateOTelEnrichment` and `StopOTelEnrichment` and read back by `GetOTelEnrichment`.
//! * [`ComAmazonawsCloudwatchGraniteserviceversion20100801State::dataset_kms_keys`] — the
//!   customer managed KMS key each Region's default dataset is encrypted with, read back by
//!   `GetDataset` and removed by `DisassociateDatasetKmsKey`.
//!
//! An alarm is stored as the modelled [`MetricAlarm`] shape itself rather than a parallel
//! record, so every member `PutMetricAlarm` accepts round-trips through `DescribeAlarms`
//! without a field-by-field copy that can silently drop one. A metric stream is stored the
//! same way, as the modelled [`GetMetricStreamOutput`] its reader returns.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use ::serde_json::{Map, Value};

use crate::ops::types::{
    ActionConfiguration, AlarmHistoryItem, AnomalyDetector, CompositeAlarm, Dimension,
    EvaluationCriteria, EvaluationWindow, GetMetricStreamOutput, InsightRule, LogAlarm,
    MetricAlarm, MetricDataQuery, MetricMathAnomalyDetector, OTelEnrichmentMetricSelector,
    PartialFailure, ResourceNotFoundException, ScheduledQueryConfiguration,
    SingleMetricAnomalyDetector, Tag, WarmUpConfiguration,
};

/// The action notifications an alarm state transition owes, and how they are delivered to
/// the ARNs the alarm names.
pub mod alarm_actions;

/// A metric alarm's `StateValue`, computed from the datapoints this store holds.
pub mod alarm_evaluation;

/// The Contributor Insights rule syntax a `RuleDefinition` carries, evaluated over the log
/// events ingested into the log groups a rule names, and the report `GetInsightRuleReport`
/// answers from what that evaluation collected.
pub mod insight_rules;

/// The metric math expression language a `MetricDataQuery$Expression` carries, parsed and
/// evaluated over the series this store holds.
pub mod metric_math;

/// A `MetricDataQuery` array — what `GetMetricData` answers from and what a metric-math
/// alarm's configuration is — resolved over the series this store holds.
pub mod metric_query;

/// The usage telemetry this service publishes about its own API: the `CallCount` series every
/// served call is counted into.
pub mod usage;

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

/// The `StandardUnit` value a datapoint carries when its publisher declared no unit of its
/// own: `None`, the value the enumerated type declares for a measurement that HAS no unit
/// (`smithy_query(query="shape",shape="StandardUnit")`).
///
/// So "no unit" is a unit like any other here: it is its own storage bucket, a read naming it
/// selects exactly the datapoints published without one, and a read naming none reports them
/// under it.
pub const UNIT_NONE: &str = "None";

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

    /// The unit this datapoint is stored under: the one its publisher declared, and
    /// [`UNIT_NONE`] when it declared none.
    ///
    /// A datapoint has a unit whether or not its publisher named one, because the enumerated
    /// type has a value for "no unit" ([`UNIT_NONE`]): so a read selecting that value finds
    /// the datapoints published without one, and a read selecting none reports them under it.
    /// The one definition of a datapoint's unit, shared by every reader that selects on one
    /// and by the alarm evaluation that grades one, so none of them can disagree about which
    /// bucket a datapoint is in.
    pub fn unit(&self) -> &str {
        self.unit.as_deref().unwrap_or(UNIT_NONE)
    }

    /// The instant this datapoint is AGGREGATED at: the start of the one-minute bucket its own
    /// time stamp falls in for a datapoint published at REGULAR resolution, and its own time
    /// stamp for a high-resolution one.
    ///
    /// "Setting this to 1 specifies this metric as a high-resolution metric, so that CloudWatch
    /// stores the metric with sub-minute resolution down to one second. Setting this to 60
    /// specifies this metric as regular resolution, which CloudWatch stores at 1-minute
    /// resolution."
    /// (`smithy_query(query="doc",shape="MetricDatum",member="StorageResolution")`) — a
    /// regular-resolution datapoint HAS no sub-minute detail stored, so a read that asks for a
    /// sub-minute period cannot see one: the datapoint is aggregated in the minute it was
    /// published in, which is why such a read answers with whole-minute instants however short
    /// its period is. Only a high-resolution datapoint is aggregated at a finer granularity.
    ///
    /// The granularity is the datapoint's own declared `StorageResolution`, so one definition
    /// serves every reader that lays period windows over a series and the alarm evaluation that
    /// grades them.
    pub fn aggregated_instant(&self) -> f64 {
        let resolution = f64::from(self.storage_resolution);
        if resolution <= 1.0 {
            return self.timestamp_secs;
        }
        (self.timestamp_secs / resolution).floor() * resolution
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

/// The sub-minute `Period` values a metric may be read at: "For metrics with regular
/// resolution, a period can be as short as one minute (60 seconds) and must be a multiple of
/// 60. For high-resolution metrics that are collected at intervals of less than one minute,
/// the period can be 1, 5, 10, 20, 30, 60, or any multiple of 60." (`MetricStat$Period`,
/// `GetMetricStatisticsInput$Period`) — every other legal period is a multiple of 60.
pub const SUB_MINUTE_PERIODS: [i32; 5] = [1, 5, 10, 20, 30];

/// How many dimensions one metric may carry, and so how long any `Dimensions` collection of a
/// request about a metric may be: "You can use up to 30 dimensions per metric to further
/// clarify what data the metric collects" (`PutMetricData`), which the modelled dimension
/// lists state as `length 0..30` (`GetMetricStatisticsInput$Dimensions`,
/// `MetricDatum$Dimensions`).
pub const MAX_METRIC_DIMENSIONS: usize = 30;

/// How CloudWatch states a value it cannot accept, naming the request member path the value
/// arrived on.
///
/// A client acts on the member path: it is what locates the member that has to change. This
/// is the family this service answers an unusable input value in, as its recorded refusals
/// carry it, and it is here rather than in one handler because the refusal of a value the
/// boundary validator catches (`Service::override_refusal`) and the refusal of one a handler
/// catches have to read alike.
pub fn value_is_invalid_message(value: &str, parameter: &str) -> String {
    format!("The value {value} for parameter {parameter} is invalid.")
}

/// How CloudWatch states a value outside the SET its member accepts: the member path and the
/// whole accepted set, and nothing of the submitted value.
///
/// The set is what a client repairs the request from — the message is only actionable when it
/// enumerates the legal values — so it is passed in from the declaration that owns it (an
/// enumerated member's own value set, as the model declares it) rather than written here. HOW
/// that set is spelled is the service's own ([`announced_set`]): a set this service is recorded
/// announcing in an order or a bracket form of its own is spelled that way, and every other one
/// is spelled in the order the declaration holds, bare.
pub fn value_in_set_message(parameter: &str, allowed: &[String]) -> String {
    format!(
        "The parameter {parameter} must be a value in the set {}.",
        announced_set(allowed)
    )
}

/// One set of accepted values AS THIS SERVICE ANNOUNCES IT: the values in the order it emits
/// them, and the bracket form it emits them in.
struct AnnouncedSet {
    /// The accepted values, in the order the service lists them.
    values: &'static [&'static str],
    /// Whether each bracket carries one space inside it.
    padded: bool,
}

impl AnnouncedSet {
    /// Whether this is the announcement of `values`: the same values, in whatever order the
    /// caller holds them — the announcement's own order is the point of it, so it cannot be the
    /// key as well.
    fn announces(&self, values: &[String]) -> bool {
        values.len() == self.values.len()
            && values
                .iter()
                .all(|value| self.values.contains(&value.as_str()))
    }

    /// The set as the service writes it: the values comma-and-space joined inside its brackets.
    fn rendered(&self) -> String {
        let listed = self.values.join(", ");
        if self.padded {
            format!("[ {listed} ]")
        } else {
            format!("[{listed}]")
        }
    }
}

/// `allowed` as this service announces it.
///
/// Neither the order a set is listed in nor the padding of its brackets is derivable from the
/// declaration that owns the set: both are properties of the service's own renderer, recorded
/// from what it answers. So a set with a recorded announcement is spelled from it, matched by
/// the set itself rather than by the member path, so that every member accepting the same
/// values announces them alike; any other set is spelled in the order its declaration holds,
/// inside bare brackets.
fn announced_set(allowed: &[String]) -> String {
    ANNOUNCED_SETS
        .iter()
        .find(|announced| announced.announces(allowed))
        .map_or_else(|| format!("[{}]", allowed.join(", ")), AnnouncedSet::rendered)
}

/// Every set this service is recorded announcing other than bare and in declaration order.
const ANNOUNCED_SETS: &[AnnouncedSet] = &[MEASUREMENT_UNITS, STORAGE_RESOLUTIONS];

/// How this service announces the unit a measurement is expressed in — the value set
/// `StandardUnit` declares (`smithy_query(query="shape",shape="StandardUnit")`), which every
/// `Unit` member targets — in the order it lists them, which the order the model declares them
/// in is not, inside padded brackets.
const MEASUREMENT_UNITS: AnnouncedSet = AnnouncedSet {
    values: &[
        "Megabits",
        "Terabits",
        "Gigabits",
        "Count",
        "Bytes",
        "Gigabytes",
        "Gigabytes/Second",
        "Kilobytes",
        "Kilobits/Second",
        "Terabytes",
        "Terabits/Second",
        "Bytes/Second",
        "Percent",
        "Megabytes",
        "Megabits/Second",
        "Milliseconds",
        "Microseconds",
        "Kilobytes/Second",
        "Gigabits/Second",
        "Megabytes/Second",
        "Bits",
        "Bits/Second",
        "Count/Second",
        "Seconds",
        "Kilobits",
        "Terabytes/Second",
        "None",
    ],
    padded: true,
};

/// How this service announces the resolutions a datum may be stored at ("Valid values are 1 and
/// 60", `MetricDatum$StorageResolution`): the two values, inside padded brackets.
const STORAGE_RESOLUTIONS: AnnouncedSet = AnnouncedSet {
    values: &["1", "60"],
    padded: true,
};

/// How CloudWatch states a value it understands but cannot serve on the member it arrived on —
/// a statistic name that is not of the kind its member computes, say: the value, because which
/// one of a collection was rejected is not derivable from the member path alone, and the path.
pub fn value_not_supported_message(value: &str, parameter: &str) -> String {
    format!("The value {value} for parameter {parameter} is not supported.")
}

/// The path a refusal of one ELEMENT of a request's list member names it by: the list, the
/// collection step, and the element's 1-based position inside it.
pub fn list_element_path(list: &str, position: usize) -> String {
    format!("{list}.{COLLECTION_STEP}.{position}")
}

/// The same path extended by the step CloudWatch's parameter walk names a list's own TARGET
/// shape with, which the refusals of a value inside a list carry after the position
/// (`Statistics.member.1.<list element>`).
pub fn list_target_path(element_path: &str) -> String {
    format!("{element_path}.{LIST_TARGET_STEP}")
}

/// That step, as CloudWatch spells it: a list's target shape has no member name of its own, so
/// the walk names it by what it IS.
const LIST_TARGET_STEP: &str = "<list element>";

/// How CloudWatch states a collection carrying more entries than its cap: the member and the
/// cap, and nothing of the entries themselves.
pub fn collection_too_large_message(collection: &str, cap: usize) -> String {
    format!("The collection {collection} must not have a size greater than {cap}.")
}

/// How CloudWatch states a request carrying two members that cannot be served together.
pub fn mutually_exclusive_message(first: &str, second: &str) -> String {
    format!(
        "The parameters {first} and {second} are mutually exclusive and you have specified both."
    )
}

/// How CloudWatch states a request carrying NEITHER of two members one of which it needs.
pub fn one_of_required_message(first: &str, second: &str) -> String {
    format!("At least one of the parameters {first} and {second} must be specified.")
}

/// How CloudWatch states a parameter it needs that the request did not supply, naming the
/// request member path it was looked for at.
///
/// Beside the other parameter sentences rather than in one handler, for the same reason they
/// are: a parameter the boundary validator finds unsupplied
/// (`Service::override_refusal`) and one a handler finds unsupplied have to read alike.
pub fn parameter_required_message(parameter: &str) -> String {
    format!("The parameter {parameter} is required.")
}

/// How CloudWatch states a value that does not match the grammar its member accepts: the
/// request member path and the expression the value had to match.
///
/// The expression is what a client re-validates against locally, which is why it is quoted
/// rather than described.
pub fn value_pattern_message(parameter: &str, pattern: &str) -> String {
    request_validation_message(&format!(
        "The value for parameter {parameter} is not matching the expected pattern {pattern}."
    ))
}

/// How CloudWatch states a collection whose entries repeat a value that has to identify each
/// of them: the member those values are of, and the collection they are in — the whole
/// sentence, since neither the repeated value nor the positions carrying it are named.
pub fn values_not_unique_message(member: &str, collection: &str) -> String {
    request_validation_message(&format!(
        "The values for parameter {member} in {collection} are not unique."
    ))
}

/// One sentence as the REQUEST-VALIDATION layer in front of the service writes it: the
/// sentence, then the terminator every message that layer renders ends with.
///
/// The layer that judges the SHAPE of a request — the grammar of a parameter, the uniqueness
/// of the values that identify a collection's entries, two members of one object that exclude
/// each other — ends its message with a newline, which a client reads back as part of the
/// message. A refusal the SERVICE itself makes, about a precondition between the resources it
/// holds, carries no terminator, so this is applied where that layer's own sentences are
/// rendered rather than to every refusal of the class.
pub fn request_validation_message(sentence: &str) -> String {
    format!("{sentence}{MESSAGE_TERMINATOR}")
}

/// What the request-validation layer ends each of its messages with.
const MESSAGE_TERMINATOR: &str = "\n";

/// The grammar the id of a query inside a `MetricDataQuery` array must match.
///
/// "The valid characters are letters, numbers, and underscore. The first character must be a
/// lowercase letter." (`MetricDataQuery$Id`) — the member carries no modelled `@pattern`, so
/// no boundary validator refuses an id outside the grammar before a handler runs, and the
/// expression is stated here once: it is both what [`is_metric_query_id`] judges and what the
/// refusal publishes to a client.
pub const METRIC_QUERY_ID_PATTERN: &str = "^[a-z][a-zA-Z0-9_]*$";

/// Whether `id` is a query id the grammar [`METRIC_QUERY_ID_PATTERN`] admits: a lowercase
/// letter, then letters, digits and underscores.
pub fn is_metric_query_id(id: &str) -> bool {
    let mut characters = id.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// The member of a query a refusal of its id names, as the sentence spells it.
pub const QUERY_ID_MEMBER: &str = "id";

/// The request member path a refusal NAMES, built from the path the validating walk spells.
///
/// The walk spells a member in lowerCamel and an element of a list as `<position>.member`
/// (`vendor/smithy-schema-server`'s `path` module); a query-protocol service names the same
/// position `Member.member.<position>`, in the member names its model declares. One
/// conversion, so a refusal from the boundary and one from a handler
/// ([`value_is_invalid_message`]) name the same member the same way.
pub fn query_parameter_path(path: &str) -> String {
    let mut segments: Vec<String> = Vec::new();
    let mut position: Option<&str> = None;
    for segment in path.split('.') {
        if !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_digit()) {
            position = Some(segment);
        } else if segment == COLLECTION_STEP {
            segments.push(COLLECTION_STEP.to_string());
            if let Some(index) = position.take() {
                segments.push(index.to_string());
            }
        } else {
            segments.push(upper_camel(segment));
        }
    }
    if let Some(index) = position {
        segments.push(index.to_string());
    }
    segments.join(".")
}

/// The segment both path spellings use for the step from a collection into one of its
/// elements.
const COLLECTION_STEP: &str = "member";

/// One path segment as a model names the member: the first character upper-cased, the rest as
/// it stands (a model's member names are already camel-cased).
fn upper_camel(segment: &str) -> String {
    let mut characters = segment.chars();
    match characters.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + characters.as_str(),
        None => String::new(),
    }
}

/// Why `period` is not a granularity a metric can be read at, or `None` when it is. Shared
/// by every operation that takes a `Period`, so one rule answers them all; each maps the
/// message onto the parameter error ITS model declares.
///
/// A period is a span of seconds, so it is at least one — the model says so (`Period` carries
/// `range min 1`) and so do the members above. Stated here as well, so the rule the windows
/// are built on does not depend on the boundary validator having run.
pub fn period_error(period: i32) -> Option<String> {
    let documented = period >= 1 && (SUB_MINUTE_PERIODS.contains(&period) || period % 60 == 0);
    (!documented).then(|| {
        // Stated the way CloudWatch states a value outside the set its member accepts
        // ([`value_in_set_message`]): the member and the granularities it names, and nothing
        // of the submitted value. The set the sentence enumerates is the sub-minute
        // granularities, the values a caller cannot reach by multiplying a minute.
        let allowed: Vec<String> = SUB_MINUTE_PERIODS.iter().map(i32::to_string).collect();
        value_in_set_message(PERIOD_PARAMETER, &allowed)
    })
}

/// The request member a period arrives on, as every operation that takes one spells it
/// (`GetMetricStatisticsInput$Period`, `MetricStat$Period`, `MetricDataQuery$Period`).
const PERIOD_PARAMETER: &str = "Period";

/// How long a datapoint stays readable at a SUB-MINUTE period: "Data points with a period of
/// less than 60 seconds are available for 3 hours." (`GetMetricStatistics`,
/// `GetMetricData`)
pub const SUB_MINUTE_RETENTION_SECS: f64 = 3.0 * 60.0 * 60.0;

/// How long a datapoint stays readable at a ONE-HOUR period: "Data points with a period of
/// 3600 seconds (1 hour) are available for 455 days (15 months)." — past it a datapoint is
/// readable at no granularity at all.
pub const ONE_HOUR_RETENTION_SECS: f64 = 455.0 * 24.0 * 60.0 * 60.0;

/// Whether a datapoint stamped `timestamp_secs` is still readable at `period` seconds of
/// granularity at the instant `now`.
///
/// CloudWatch keeps a series in coarser and coarser resolution as it ages, and a read sees an
/// aged datapoint only at a granularity whose own tier still holds it: "Data points with a
/// period of less than 60 seconds are available for 3 hours. […] Data points with a period of
/// 60 seconds (1-minute) are available for 15 days. Data points with a period of 300 seconds
/// (5-minute) are available for 63 days. Data points with a period of 3600 seconds (1 hour)
/// are available for 455 days (15 months). Data points that are initially published with a
/// shorter period are aggregated together for long-term storage." The same rule is stated
/// from the request's side on the readers' `Period` member — "Start time between 3 hours and
/// 15 days ago - Use a multiple of 60 seconds (1 minute). Start time between 15 and 63 days
/// ago - Use a multiple of 300 seconds (5 minutes). Start time greater than 63 days ago - Use
/// a multiple of 3600 seconds (1 hour)" — and judging each DATAPOINT by its own age is what
/// makes a range that crosses a tier boundary answer with the part that survives.
///
/// Shared by every read that lays period windows over a range, so one rule decides what any
/// of them can still see.
pub fn retained_at_period(timestamp_secs: f64, period: i32, now: f64) -> bool {
    let age = now - timestamp_secs;
    // A multiple of the tier's own granularity is what the aggregated tier can be read at;
    // the youngest tier is readable at every granularity a read may ask for.
    let multiple_of = |granularity: i32| period % granularity == 0;
    if age <= SUB_MINUTE_RETENTION_SECS {
        true
    } else if age <= FIFTEEN_DAYS_SECS {
        multiple_of(60)
    } else if age <= SIXTY_THREE_DAYS_SECS {
        multiple_of(300)
    } else if age <= ONE_HOUR_RETENTION_SECS {
        multiple_of(3_600)
    } else {
        false
    }
}

/// Whether a read of `period`-wide windows over a range beginning at `requested_start` asks for
/// a granularity that range can be read at.
///
/// The rule is stated from the request's side on both readers' own `Period` member: "If the
/// StartTime parameter specifies a time stamp that is greater than 3 hours ago, you must
/// specify the period as follows or no data points in that time range is returned: Start time
/// between 3 hours and 15 days ago - Use a multiple of 60 seconds (1 minute). Start time
/// between 15 and 63 days ago - Use a multiple of 300 seconds (5 minutes). Start time greater
/// than 63 days ago - Use a multiple of 3600 seconds (1 hour)."
/// (`GetMetricStatisticsInput$Period`, in the same words on `MetricStat$Period`) — a request at
/// a finer granularity than its start time's tier is stored at asks for a resolution nothing in
/// that range is kept at, so the read answers with no data point for the range.
///
/// The start time is CLAMPED to the retention horizon first: "Data points with a period of 3600
/// seconds (1 hour) are available for 455 days (15 months)" ([`ONE_HOUR_RETENTION_SECS`]) is as
/// far back as a series is held at all, so the documented bands cover every start time that
/// could select a tier and a range beginning before the horizon begins, for this rule, AT the
/// horizon. What decides the granularity is the part of the range that can hold a datapoint —
/// so a request for several years of history at an hourly granularity is answered with the
/// history there is, rather than with nothing because its start reaches past where any tier
/// ends.
///
/// Shared by both readers, so a range and a granularity mean the same thing to each of them.
pub fn granularity_available_from(requested_start: f64, period: i32, now: f64) -> bool {
    let horizon = now - ONE_HOUR_RETENTION_SECS;
    retained_at_period(requested_start.max(horizon), period, now)
}

/// Fold one series' datapoints into the `period`-wide windows of `[start, end)`, keyed by the
/// window's index from `start` and by the UNIT the datapoints in it were published with.
///
/// `start` is INCLUSIVE and `end` EXCLUSIVE, as both readers' members document. A window
/// holding no datapoint is never inserted, so what this allocates is bounded by the
/// datapoints the series holds in the range rather than by the range the request spans.
/// A datapoint the retention tier of `period` no longer holds is not in any window
/// ([`retained_at_period`]), and which window a datapoint falls in is decided by the instant
/// its own `StorageResolution` stores it at ([`StoredDatum::aggregated_instant`]).
///
/// The unit is part of the key because it is part of a datapoint's identity: "If you omit
/// Unit, all data that was collected with any unit is returned, along with the corresponding
/// units that were specified when the data was reported to CloudWatch."
/// (`GetMetricStatisticsInput$Unit`) — observations published at one instant under different
/// units are different series, so they fold into different windows and never into a
/// cross-unit aggregate.
///
/// `unit` is the caller's unit filter: "If you specify a unit, the operation returns only
/// data that was collected with that unit specified. […] CloudWatch does not perform unit
/// conversions." A request naming none takes every datapoint. A datapoint's own unit is the
/// one its publisher declared, or [`UNIT_NONE`] when it declared none
/// ([`StoredDatum::unit`]), so both halves of the selection read the same unit.
pub fn windows_by_unit<'a>(
    data: &'a [StoredDatum],
    start: f64,
    end: f64,
    period: i32,
    unit: Option<&str>,
    now: f64,
) -> BTreeMap<(i64, &'a str), Vec<&'a StoredDatum>> {
    let span = f64::from(period);
    let mut windows: BTreeMap<(i64, &str), Vec<&StoredDatum>> = BTreeMap::new();
    for datum in data {
        // The instant the datapoint is AGGREGATED at, which is the start of its own minute for
        // one published at regular resolution ([`StoredDatum::aggregated_instant`]): a read sees
        // a datapoint at the granularity it is stored at, not at a finer one it asks for.
        let instant = datum.aggregated_instant();
        if instant < start || instant >= end {
            continue;
        }
        if unit.is_some_and(|wanted| datum.unit() != wanted) {
            continue;
        }
        if !retained_at_period(instant, period, now) {
            continue;
        }
        #[allow(clippy::cast_possible_truncation)]
        let index = ((instant - start) / span).floor() as i64;
        windows.entry((index, datum.unit())).or_default().push(datum);
    }
    windows
}

/// Fold one series' datapoints into the `period`-wide windows of `[start, end)`, keyed by the
/// window's index from `start`, as ONE unit's series.
///
/// [`windows_by_unit`] for a reader that answers ONE series per query: a query naming a unit
/// reads that unit's datapoints, and a query naming NONE resolves to a single unit all the
/// same — the alphabetically first unit name the range holds — because a series is identified
/// by its unit and no read returns the sum across two of them. Every rule about what a window
/// holds — the range's ends, the caller's unit filter, the granularity the datapoint is stored
/// at and the retention tier of `period` — is [`windows_by_unit`]'s, so the two readers cannot
/// disagree about what is in range.
///
/// Which unit that is does not depend on the order the data was published in or on the window
/// it is read at, so two reads of one range resolve the same series.
pub fn windows_of<'a>(
    data: &'a [StoredDatum],
    start: f64,
    end: f64,
    period: i32,
    unit: Option<&str>,
    now: f64,
) -> BTreeMap<i64, Vec<&'a StoredDatum>> {
    let by_unit = windows_by_unit(data, start, end, period, unit, now);
    let resolved = unit.map(ToString::to_string).or_else(|| {
        by_unit
            .keys()
            .map(|(_index, unit)| (*unit).to_string())
            .min()
    });
    let mut windows: BTreeMap<i64, Vec<&StoredDatum>> = BTreeMap::new();
    for ((index, held), data) in by_unit {
        if resolved.as_deref() != Some(held) {
            continue;
        }
        windows.entry(index).or_default().extend(data);
    }
    windows
}

/// The instant the `index`th `period`-wide window from `start` begins, which is the time
/// stamp a datapoint folded out of that window carries.
pub fn window_start(start: f64, index: i64, period: f64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let offset = index as f64;
    start + offset * period
}

/// How long "start time less than 15 days ago" lasts, in seconds — the first of the three
/// spans a requested start time is rounded by.
const FIFTEEN_DAYS_SECS: f64 = 15.0 * 24.0 * 60.0 * 60.0;

/// How long "start time between 15 and 63 days ago" lasts, in seconds.
const SIXTY_THREE_DAYS_SECS: f64 = 63.0 * 24.0 * 60.0 * 60.0;

/// The sub-minute `Period` values a request that names one of them rounds its start time to:
/// "If you set Period to 5, 10, 20, or 30, the start time of your request is rounded down to
/// the nearest time that corresponds to even 5-, 10-, 20-, or 30-second divisions of a
/// minute." (`GetMetricDataInput$StartTime`) — four of the five sub-minute periods
/// [`SUB_MINUTE_PERIODS`] admits.
const START_TIME_ROUNDING_PERIODS: [i32; 4] = [5, 10, 20, 30];

/// The start of the first window a read searches: the requested start time rounded DOWN to the
/// boundary its age and its period put it on.
///
/// "CloudWatch rounds the specified time stamp as follows: Start time less than 15 days ago -
/// Round down to the nearest whole minute. […] Start time between 15 and 63 days ago - Round
/// down to the nearest 5-minute clock interval. […] Start time greater than 63 days ago -
/// Round down to the nearest 1-hour clock interval. […] If you set Period to 5, 10, 20, or 30,
/// the start time of your request is rounded down to the nearest time that corresponds to even
/// 5-, 10-, 20-, or 30-second divisions of a minute." (`GetMetricDataInput$StartTime`, in the
/// same words as `GetMetricStatisticsInput$StartTime`)
///
/// Shared by every read that lays period windows over a range, so the windows of one instant
/// are the same whichever operation reads them.
pub fn rounded_start(requested_start: f64, period: i32, now: f64) -> f64 {
    let interval = if START_TIME_ROUNDING_PERIODS.contains(&period) {
        f64::from(period)
    } else {
        let age = now - requested_start;
        if age < FIFTEEN_DAYS_SECS {
            60.0
        } else if age < SIXTY_THREE_DAYS_SECS {
            5.0 * 60.0
        } else {
            60.0 * 60.0
        }
    };
    (requested_start / interval).floor() * interval
}

/// The percentile a `pNN` / `pNN.NN` extended statistic names, in percent, or `None` for a
/// name that is not one. The `p` is matched without regard to ASCII case, and the number is
/// a plain decimal in `0..=100`, so `f64`'s sign and exponent forms (`p+9`, `p1e2`) cannot
/// slip through where a percentile is expected.
///
/// "The percentile statistics. Specify values between p0.0 and p100."
/// (`GetMetricStatisticsInput$ExtendedStatistics`)
pub fn parse_percentile(name: &str) -> Option<f64> {
    percentage(strip_prefix_ignore_ascii_case(name, "p")?)
}

/// The statistics CloudWatch names outright on its "CloudWatch statistics definitions" page:
/// the five basic statistics the store folds, and the interquartile mean.
const CLOUDWATCH_BASIC_STATISTICS: &[&str] =
    &["SampleCount", "Average", "Sum", "Minimum", "Maximum", "IQM"];

/// The statistic families from the same page that take a trimming span, either as a bare
/// percentile ("this includes tm98, wm90") or as a span: trimmed mean, trimmed count,
/// trimmed sum and Winsorized mean.
const TRIMMED_FAMILIES: &[&str] = &["tm", "tc", "ts", "wm"];

/// The five basic statistics this store folds out of the four aggregates every datapoint
/// carries ([`BasicStatistics::named`]); every other statistic is computed from the window's
/// own distribution instead ([`BoundedStatistic::of`], [`weighted_percentile`]).
const FOLDED_STATISTICS: &[&str] = &["SampleCount", "Average", "Sum", "Minimum", "Maximum"];

/// Whether `statistic` is one CloudWatch supports at all: a basic statistic, a percentile, or
/// a member of a bounded family. Shared by every operation that takes a statistic NAME and
/// admits the basic five among them — `MetricStat$Stat` is an unconstrained string in the
/// model, so the grammar is the only thing that tells a statistic from a typo. An operation
/// that admits only the statistics computed over the distribution asks
/// [`is_extended_statistic`] instead.
pub fn is_cloudwatch_statistic(statistic: &str) -> bool {
    CLOUDWATCH_BASIC_STATISTICS
        .iter()
        .any(|known| known.eq_ignore_ascii_case(statistic))
        || parse_percentile(statistic).is_some()
        || bounded_statistic(statistic).is_some()
}

/// The interquartile mean, which is the trimmed mean of the middle half of the distribution:
/// "Interquartile mean. The trimmed mean of the interquartile range" — the same statistic as
/// `TM(25%:75%)`.
const INTERQUARTILE_SPAN: (f64, f64) = (25.0, 75.0);

/// A statistic of one of the bounded families, in either documented form:
/// `<family><percentile>` (`tm98`) or `<family>(<low>:<high>)` (`TM(10%:90%)`, `PR(:300)`),
/// where one side of a span may be omitted but not both — and the interquartile mean, which
/// is one of them under its own name.
fn bounded_statistic(statistic: &str) -> Option<BoundedStatistic> {
    if statistic.eq_ignore_ascii_case("IQM") {
        return Some(BoundedStatistic {
            family: BoundedFamily::TrimmedMean,
            low: Some(Bound::Percentile(INTERQUARTILE_SPAN.0)),
            high: Some(Bound::Percentile(INTERQUARTILE_SPAN.1)),
        });
    }
    if let Some(open) = statistic.find('(') {
        let (family, span) = statistic.split_at(open);
        let family = BoundedFamily::named(family)?;
        let (low, high) = parse_span(span)?;
        return Some(BoundedStatistic { family, low, high });
    }
    // The bare form names a percentile only, and only for the families that take one: "this
    // includes tm98, wm90" — the lower bound of such a span is the bottom of the
    // distribution.
    for name in TRIMMED_FAMILIES {
        if let Some(percentile) = strip_prefix_ignore_ascii_case(statistic, name).and_then(percentage)
        {
            return Some(BoundedStatistic {
                family: BoundedFamily::named(name)?,
                low: Some(Bound::Percentile(0.0)),
                high: Some(Bound::Percentile(percentile)),
            });
        }
    }
    None
}

/// A `(<low>:<high>)` bound pair with at least one bound given; `None` for anything else.
fn parse_span(span: &str) -> Option<(Option<Bound>, Option<Bound>)> {
    let inner = span
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))?;
    let (low, high) = inner.split_once(':')?;
    if low.is_empty() && high.is_empty() {
        return None;
    }
    let bound = |text: &str| -> Option<Option<Bound>> {
        if text.is_empty() {
            return Some(None);
        }
        bound_value(text).map(Some)
    };
    Some((bound(low)?, bound(high)?))
}

/// One bound of a span: a percentage when it carries a `%`, otherwise a plain metric value.
fn bound_value(bound: &str) -> Option<Bound> {
    match bound.strip_suffix('%') {
        Some(percent) => percentage(percent).map(Bound::Percentile),
        None => decimal(bound).map(Bound::Value),
    }
}

/// One bound of a bounded statistic's span.
#[derive(Debug, Clone, Copy)]
enum Bound {
    /// A place in the window's own distribution, in percent.
    Percentile(f64),
    /// A metric value.
    Value(f64),
}

/// The statistic families CloudWatch defines over a span of the window's distribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundedFamily {
    /// "Trimmed mean. The mean of all values that are between two specified boundaries, but
    /// ignores outliers above and below these boundaries."
    TrimmedMean,
    /// "Trimmed count. The number of data points in the chosen range."
    TrimmedCount,
    /// "Trimmed sum. The sum of the values of data points in a chosen range."
    TrimmedSum,
    /// "Winsorized mean. ... values outside of the boundaries are replaced with the values of
    /// the boundaries, then the mean is computed."
    WinsorizedMean,
    /// "Percentile rank. The percentage of values that fall within the specified range."
    PercentileRank,
}

impl BoundedFamily {
    /// The family a statistic's prefix names.
    fn named(prefix: &str) -> Option<Self> {
        match () {
            () if prefix.eq_ignore_ascii_case("tm") => Some(Self::TrimmedMean),
            () if prefix.eq_ignore_ascii_case("tc") => Some(Self::TrimmedCount),
            () if prefix.eq_ignore_ascii_case("ts") => Some(Self::TrimmedSum),
            () if prefix.eq_ignore_ascii_case("wm") => Some(Self::WinsorizedMean),
            () if prefix.eq_ignore_ascii_case("pr") => Some(Self::PercentileRank),
            () => None,
        }
    }
}

/// A statistic over a span of one window's own distribution: which statistic, and the span.
#[derive(Debug, Clone, Copy)]
struct BoundedStatistic {
    /// Which of the families it is.
    family: BoundedFamily,
    /// The low end of the span, when the statistic names one.
    low: Option<Bound>,
    /// The high end of the span, when the statistic names one.
    high: Option<Bound>,
}

impl BoundedStatistic {
    /// This statistic over a window's distribution: the published values with their weights,
    /// ascending. `None` when the span is empty of values, so the statistic is undefined
    /// there.
    fn of(&self, values: &[(f64, f64)]) -> Option<f64> {
        let total: f64 = values.iter().map(|(_, weight)| weight).sum();
        if total <= 0.0 {
            return None;
        }
        // A percentile bound is a place in this window's distribution; a plain bound is a
        // metric value, and the same number whatever the window holds.
        let edge = |bound: Option<Bound>, fallback: f64| -> f64 {
            match bound {
                None => fallback,
                Some(Bound::Value(value)) => value,
                Some(Bound::Percentile(percentile)) => {
                    percentile_of(values, percentile, total).unwrap_or(fallback)
                }
            }
        };
        match self.family {
            // "The percentage of values that fall within the specified range": the weight
            // inside the span over the weight of the whole window. The span's own bounds are
            // inside it, so a rank and a trimmed count over the same bounds agree.
            BoundedFamily::PercentileRank => {
                let low = edge(self.low, f64::NEG_INFINITY);
                let high = edge(self.high, f64::INFINITY);
                let inside: f64 = values
                    .iter()
                    .filter(|(value, _)| *value >= low && *value <= high)
                    .map(|(_, weight)| weight)
                    .sum();
                Some(inside / total * 100.0)
            }
            // "Values outside of the boundaries are replaced with the values of the
            // boundaries": every observation still counts, held inside the span. The span's
            // own ends are ordered here rather than assumed, and the fold is `max`/`min`
            // rather than a clamp, which has no answer for ends that are not ordered.
            BoundedFamily::WinsorizedMean => {
                let (low, high) = {
                    let (first, second) = (edge(self.low, f64::NEG_INFINITY), edge(self.high, f64::INFINITY));
                    if first <= second {
                        (first, second)
                    } else {
                        (second, first)
                    }
                };
                let sum: f64 = values
                    .iter()
                    .map(|(value, weight)| value.max(low).min(high) * weight)
                    .sum();
                Some(sum / total)
            }
            family => {
                let kept = trim(values, self.low, self.high, total);
                let weight: f64 = kept.iter().map(|(_, weight)| weight).sum();
                let sum: f64 = kept.iter().map(|(value, weight)| value * weight).sum();
                match family {
                    BoundedFamily::TrimmedCount => Some(weight),
                    BoundedFamily::TrimmedSum => Some(sum),
                    _ => (weight > 0.0).then(|| sum / weight),
                }
            }
        }
    }
}

/// The observations of a window that fall inside a span, with their weights.
///
/// A percentile bound trims by WEIGHT — "`TM(10%:90%)` calculates the average after removing
/// the 10% lowest and 10% highest data points" — so an observation standing for several
/// samples may be trimmed in part; a plain value bound keeps the observations inside it.
fn trim(
    values: &[(f64, f64)],
    low: Option<Bound>,
    high: Option<Bound>,
    total: f64,
) -> Vec<(f64, f64)> {
    let mut trim_low = match low {
        Some(Bound::Percentile(percentile)) => total * percentile / 100.0,
        _ => 0.0,
    };
    let mut trim_high = match high {
        Some(Bound::Percentile(percentile)) => total * (100.0 - percentile) / 100.0,
        _ => 0.0,
    };
    let mut kept: Vec<(f64, f64)> = Vec::with_capacity(values.len());
    for (value, weight) in values {
        let mut weight = *weight;
        let trimmed = trim_low.min(weight);
        trim_low -= trimmed;
        weight -= trimmed;
        if weight > 0.0 {
            kept.push((*value, weight));
        }
    }
    for index in (0..kept.len()).rev() {
        if trim_high <= 0.0 {
            break;
        }
        let Some((_, weight)) = kept.get_mut(index) else {
            break;
        };
        let trimmed = trim_high.min(*weight);
        trim_high -= trimmed;
        *weight -= trimmed;
    }
    let low_value = match low {
        Some(Bound::Value(value)) => Some(value),
        _ => None,
    };
    let high_value = match high {
        Some(Bound::Value(value)) => Some(value),
        _ => None,
    };
    kept.retain(|(value, weight)| {
        *weight > 0.0
            && low_value.is_none_or(|low| *value >= low)
            && high_value.is_none_or(|high| *value <= high)
    });
    kept
}

/// A percentage: a decimal in `0..=100`.
fn percentage(text: &str) -> Option<f64> {
    decimal(text).filter(|value| (0.0..=100.0).contains(value))
}

/// A plain non-negative decimal number, so `f64`'s sign and exponent forms (`+5`, `1e3`,
/// `inf`) cannot slip through where a statistic's bound is expected.
fn decimal(text: &str) -> Option<f64> {
    if text.is_empty()
        || !text
            .chars()
            .all(|character| character.is_ascii_digit() || character == '.')
    {
        return None;
    }
    text.parse().ok()
}

/// `text` without `prefix`, comparing the prefix without regard to ASCII case.
fn strip_prefix_ignore_ascii_case<'text>(text: &'text str, prefix: &str) -> Option<&'text str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

/// The published values of a span with the number of observations each stands for, ascending
/// by value — the window's own distribution, which is what every statistic outside the
/// folded five is computed from.
///
/// `None` when the span holds a datapoint whose individual values were never published: an
/// opaque aggregate defines no distribution, so no statistic over one can be computed rather
/// than invented.
fn distribution(data: &[&StoredDatum]) -> Option<Vec<(f64, f64)>> {
    let mut values: Vec<(f64, f64)> = Vec::with_capacity(data.len());
    for datum in data {
        values.push((datum.single_value()?, datum.sample_count));
    }
    values.sort_by(|left, right| left.0.total_cmp(&right.0));
    Some(values)
}

/// The `percentile`-th value of a distribution whose weights sum to `total`, by nearest rank.
fn percentile_of(values: &[(f64, f64)], percentile: f64, total: f64) -> Option<f64> {
    let target = percentile / 100.0 * total;
    let mut seen = 0.0;
    for (value, weight) in values {
        seen += weight;
        if seen >= target {
            return Some(*value);
        }
    }
    values.last().map(|(value, _)| *value)
}

/// The `percentile`-th percentile of a span's published values, by nearest rank.
///
/// `None` when the span holds a datapoint whose individual values were never published, or
/// any negative value: "Percentile statistics are not available for metrics when any of the
/// metric values are negative numbers." (`GetMetricStatistics`)
pub fn weighted_percentile(data: &[&StoredDatum], percentile: f64) -> Option<f64> {
    let values = distribution(data)?;
    if values.iter().any(|(value, _)| *value < 0.0) {
        return None;
    }
    let total: f64 = values.iter().map(|(_, weight)| weight).sum();
    if total <= 0.0 {
        return None;
    }
    percentile_of(&values, percentile, total)
}

/// One statistic of a span of datapoints by the name a request gives it — "It can include any
/// CloudWatch statistic or extended statistic." (`MetricStat$Stat`): one of the basic five the
/// model names (matched without regard to ASCII case, as the statistic grammar matches them),
/// a `pNN` percentile, or a statistic over a span of the window's own distribution (the
/// interquartile mean and the trimmed / Winsorized / percentile-rank families).
///
/// `None` for a name outside the grammar, and for a statistic the span does not define.
pub fn statistic_value(data: &[&StoredDatum], statistic: &str) -> Option<f64> {
    if let Some(canonical) = FOLDED_STATISTICS
        .iter()
        .find(|known| known.eq_ignore_ascii_case(statistic))
    {
        return BasicStatistics::of(data.iter().copied()).named(canonical);
    }
    if let Some(percentile) = parse_percentile(statistic) {
        return weighted_percentile(data, percentile);
    }
    bounded_statistic(statistic)?.of(&distribution(data)?)
}

/// One EXTENDED statistic of a span by the name a request gives it: a `pNN` percentile or one
/// of the bounded families. `None` for a name that is not an extended statistic at all, which
/// is how a caller tells the two statistic members apart.
pub fn extended_statistic_value(data: &[&StoredDatum], statistic: &str) -> Option<f64> {
    is_extended_statistic(statistic)
        .then(|| statistic_value(data, statistic))
        .flatten()
}

/// Whether `statistic` names an EXTENDED statistic: a percentile, or one of the statistics
/// computed over a span of the window's distribution. The basic five are not extended
/// statistics, which is what makes the two statistic members of a read not interchangeable.
pub fn is_extended_statistic(statistic: &str) -> bool {
    parse_percentile(statistic).is_some() || bounded_statistic(statistic).is_some()
}

/// One series ONE write stored datapoints into: its identity, and the unit each of those
/// datapoints carried.
///
/// What [`ComAmazonawsCloudwatchGraniteserviceversion20100801State::make_alarms_due`] needs to
/// decide whether a write stored anything a given alarm would GRADE — the series alone cannot
/// answer it, because an alarm's `Unit` selects which datapoints of that series it evaluates.
#[derive(Debug, Clone)]
pub struct WrittenSeries {
    /// The series the datapoints were stored into.
    pub key: MetricKey,
    /// The unit each stored datapoint carried, in the order the request listed them — each as
    /// the datapoint is STORED under ([`StoredDatum::unit`]), so a datum whose publisher
    /// declared none names [`UNIT_NONE`] here rather than nothing.
    pub units: Vec<String>,
}

/// A stored metric alarm: the modelled shape `DescribeAlarms` returns, plus the tags
/// `PutMetricAlarm` accepted (no `MetricAlarm` member carries them).
#[derive(Debug, Clone, Default)]
pub struct StoredAlarm {
    /// The alarm as `DescribeAlarms` reports it.
    pub alarm: MetricAlarm,
    /// The tags `PutMetricAlarm` was given for this alarm.
    pub tags: Vec<Tag>,
    /// The instant, in epoch seconds, from which this alarm's next evaluation against the
    /// stored metric data is owed
    /// ([`ComAmazonawsCloudwatchGraniteserviceversion20100801State::evaluate_metric_alarms`]).
    ///
    /// "When this operation creates an alarm, the alarm state is immediately set to
    /// INSUFFICIENT_DATA. The alarm is then evaluated and its state is set appropriately."
    /// (`PutMetricAlarm`) — so a write starts a span during which the alarm still reports the
    /// state the write left it in, and the evaluation becomes due at its end. `SetAlarmState`
    /// starts one of the alarm's own evaluation periods instead: the state it forces stands
    /// until the alarm's next evaluation is due
    /// ([`alarm_evaluation::forced_state_span_secs`]), and a datapoint of the series the alarm
    /// watches brings that evaluation forward ([`Self::make_alarms_due`]) — except while a
    /// forced state still stands ([`Self::forced_state_until_secs`]).
    pub evaluation_due_secs: f64,
    /// The instant, in epoch seconds, until which a state `SetAlarmState` forced on this alarm
    /// stands; `0.0` for an alarm whose state no caller has forced.
    ///
    /// A force is a state the caller asked to be visible for its span
    /// ([`alarm_evaluation::forced_state_span_secs`]), so nothing brings the evaluation that
    /// takes it back FORWARD of that instant — not an arriving datapoint of the series the
    /// alarm watches, which would otherwise leave the forced state visible to nobody at all.
    pub forced_state_until_secs: f64,
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
    /// The alarm NAMES this alarm's rule references, resolved once by the write that stored
    /// it, in the order the rule names them and without repeats.
    ///
    /// Derived from [`Self::rule`] and kept beside it for the same reason the parsed rule is
    /// kept: every evaluation pass and every relationship read needs this list, and walking
    /// the rule again for each of them would make one call's work grow with the rule's size
    /// times the alarms the store holds.
    pub child_names: Vec<String>,
    /// How many nodes this alarm's rule carries, counted once by the write that stored it.
    ///
    /// What one evaluation of this alarm COSTS, which is what the per-call evaluation budget
    /// ([`MAX_COMPOSITE_EVALUATION_NODES`]) is spent in.
    pub rule_nodes: usize,
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

/// A stored log alarm: the modelled shape `DescribeAlarms` returns, plus the tags
/// `PutLogAlarm` accepted (no `LogAlarm` member carries them).
#[derive(Debug, Clone, Default)]
pub struct StoredLogAlarm {
    /// The alarm as `DescribeAlarms` reports it.
    pub alarm: LogAlarm,
    /// The tags `PutLogAlarm` was given for this alarm.
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

/// A stored Contributor Insights rule: the modelled shape `DescribeInsightRules` reports,
/// plus the tags `PutInsightRule` accepted (no [`InsightRule`] member carries them).
#[derive(Debug, Clone, Default)]
pub struct StoredInsightRule {
    /// The rule as `DescribeInsightRules` reports it.
    pub rule: InsightRule,
    /// The rule's `Definition` as the evaluation READS it ([`insight_rules::LogRuleDefinition`]),
    /// or `None` for a definition that is not a log rule this emulator evaluates (a managed
    /// rule's, which names no log group and no contribution).
    ///
    /// Derived from `rule.definition` by the write that stored the rule, so an ingest decides
    /// which rules collect from a log group without re-reading the definition TEXT of every
    /// rule the store holds — the first factor of an ingest's cost
    /// ([`insight_rules::State::rules_collecting_from`]).
    pub collection: Option<insight_rules::LogRuleDefinition>,
    /// What the log events this rule has evaluated contributed — what `GetInsightRuleReport`
    /// reports from ([`insight_rules`]). A rule collects them while it is ENABLED, as the
    /// events arrive, so this is empty for a rule nothing has been ingested for yet.
    pub observations: Vec<insight_rules::ContributorObservation>,
    /// The tags the rule was created with.
    pub tags: Vec<Tag>,
    /// What this rule was enabled from, for a rule `PutManagedInsightRules` created;
    /// `None` for one a caller wrote with `PutInsightRule`.
    pub managed: Option<ManagedRuleOrigin>,
}

/// What a managed Contributor Insights rule was enabled from: the template
/// `PutManagedInsightRules` named, and the Amazon Web Services resource whose data the rule
/// collects.
///
/// `ListManagedInsightRules` reports the pair back as the `TemplateName` and `ResourceARN` of
/// its description ("The template name for the managed rule. Used to enable managed rules
/// using `PutManagedInsightRules`.", `ManagedRuleDescription$TemplateName`; "If a managed rule
/// is enabled, this is the ARN for the related Amazon Web Services resource.",
/// `ManagedRuleDescription$ResourceARN`), which is why it is kept beside the rule rather than
/// derived back out of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManagedRuleOrigin {
    /// The managed-rule template the rule was enabled from.
    pub template_name: String,
    /// The ARN of the resource the rule collects data for.
    pub resource_arn: String,
}

/// One managed Contributor Insights rule template this account can enable a rule from.
///
/// "The template name for the managed Contributor Insights rule, as returned by
/// `ListManagedInsightRules`." (`ManagedRule$TemplateName`) — the template a caller names is one
/// the SERVICE publishes for a kind of resource ("When you enable a managed rule, you create a
/// Contributor Insights rule that collects data from Amazon Web Services services.",
/// `PutManagedInsightRules`), so a template is held as the name it is published under and the
/// resources it collects data for, and nothing a caller sends adds one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedRuleTemplate {
    /// The name the template is published under, which a `ManagedRule` entry names it by.
    pub template_name: String,
    /// The ARN vendor whose resources this template collects data for — the third field of
    /// their ARNs (`::service_common::arn::Arn::vendor`).
    pub resource_vendor: String,
    /// The resource type inside that vendor the template applies to
    /// (`::service_common::arn::Arn::resource_type`), or `None` when it applies to every
    /// resource the vendor holds.
    pub resource_type: Option<String>,
}

impl ManagedRuleTemplate {
    /// The template published under `template_name` for every resource of the same vendor and
    /// type as `resource_arn`, or `None` when `resource_arn` is not an ARN.
    #[must_use]
    pub fn for_resource(template_name: &str, resource_arn: &str) -> Option<Self> {
        let arn = parse_resource_arn(resource_arn)?;
        Some(Self {
            template_name: template_name.to_string(),
            resource_vendor: arn.vendor.to_string(),
            resource_type: Some(arn.resource_type().to_string()),
        })
    }

    /// Whether this template is one of the managed rules available for `resource_arn`: the
    /// ARN's vendor is the one the template collects data for, and its resource type is the one
    /// the template applies to when the template names a type at all.
    #[must_use]
    pub fn available_for(&self, resource_arn: &str) -> bool {
        parse_resource_arn(resource_arn).is_some_and(|arn| {
            self.resource_vendor == arn.vendor
                && self
                    .resource_type
                    .as_deref()
                    .is_none_or(|resource_type| resource_type == arn.resource_type())
        })
    }
}

/// One managed rule `ListManagedInsightRules` reports for a resource: the template it is
/// available from, and the Contributor Insights rule enabled from that template when one is
/// enabled for the resource.
#[derive(Debug, Clone, Copy)]
pub struct AvailableManagedRule<'a> {
    /// The template the rule is (or would be) enabled from.
    pub template_name: &'a str,
    /// The rule enabled from the template for this resource — its name and record — or `None`
    /// when the template is available but nothing has enabled it: "If present, it contains
    /// information about the Contributor Insights rule that contains information about the
    /// related Amazon Web Services resource." (`ManagedRuleDescription$RuleState`)
    pub rule: Option<(&'a String, &'a StoredInsightRule)>,
}

/// A stored dashboard: the `DashboardBody` document `PutDashboard` was given, when that
/// write happened, and the tags the dashboard was created with.
///
/// The body is held verbatim, so `GetDashboard` returns "the data returned within
/// DashboardBody as the template for the new dashboard when you call PutDashboard"
/// (`GetDashboard`) — a document that round-trips character for character, which is what lets
/// a caller copy a dashboard by putting back what it read.
#[derive(Debug, Clone, Default)]
pub struct StoredDashboard {
    /// The dashboard document, exactly as the write that stored it was given.
    pub body: String,
    /// When this dashboard was last written, as epoch seconds: "The time stamp of when the
    /// dashboard was last modified, either by an API call or through the console."
    /// (`DashboardEntry$LastModified`)
    pub last_modified_secs: f64,
    /// The tags the dashboard was created with.
    pub tags: Vec<Tag>,
}

/// The account's vended-metric enrichment configuration, as the start/stop/update operations
/// last stored it and `GetOTelEnrichment` reports it.
///
/// One record per account rather than per Region: every operation describes enrichment as the
/// account's ("the current status of vended metric enrichment for the account",
/// `GetOTelEnrichment`), which is also how the metric store beside it is keyed — a series this
/// store holds belongs to the account, not to the Region whose endpoint published it.
///
/// A store with NO record is an account where enrichment has never been configured, which is
/// not the same as one that has been stopped: both report the stopped status, but only the
/// latter has a configuration that was ever "last stored" to report an `UpdatedAt` for.
#[derive(Debug, Clone, Default)]
pub struct StoredOTelEnrichment {
    /// Whether enrichment is enabled for the account: the `Running` / `Stopped` status
    /// `GetOTelEnrichment` reports.
    pub running: bool,
    /// The include filters stored for the account, empty when "every namespace that Amazon
    /// CloudWatch supports for enrichment is in scope" (`StartOTelEnrichmentInput$IncludeFilters`).
    pub include_filters: Vec<OTelEnrichmentMetricSelector>,
    /// The exclude filters stored for the account, empty when "nothing is excluded"
    /// (`StartOTelEnrichmentInput$ExcludeFilters`).
    pub exclude_filters: Vec<OTelEnrichmentMetricSelector>,
    /// When enrichment started for the account, as epoch seconds, or `None` while it is
    /// stopped: "The date and time that enrichment started for the account. This parameter is
    /// omitted when enrichment is stopped." (`GetOTelEnrichmentOutput$CreatedAt`)
    pub created_at_secs: Option<f64>,
    /// When this configuration was last stored, as epoch seconds: "The date and time that the
    /// enrichment configuration for the account was last stored."
    /// (`GetOTelEnrichmentOutput$UpdatedAt`)
    pub updated_at_secs: f64,
}

/// `Running`, one of the two modelled `OTelEnrichmentStatus` values
/// (`O_TEL_ENRICHMENT_STATUS_VALUES`): "enrichment is enabled"
/// (`GetOTelEnrichmentOutput$Status`).
pub const O_TEL_ENRICHMENT_RUNNING: &str = "Running";

/// `Stopped`, the other modelled `OTelEnrichmentStatus` value
/// (`O_TEL_ENRICHMENT_STATUS_VALUES`): "enrichment is disabled"
/// (`GetOTelEnrichmentOutput$Status`).
pub const O_TEL_ENRICHMENT_STOPPED: &str = "Stopped";

/// The most metric selectors one account's enrichment configuration holds: "A maximum of 100
/// filters is allowed across `IncludeFilters` and `ExcludeFilters` combined."
/// (`OTelEnrichmentMetricSelector`)
///
/// The model bounds each list at 100 on its own, so only the COMBINED bound is left for a
/// handler to enforce.
pub const MAX_O_TEL_ENRICHMENT_FILTERS: usize = 100;

/// Why this pair of enrichment filter lists cannot be stored, or `None` when it can.
///
/// Shared by the two operations that accept the pair (`StartOTelEnrichment` and
/// `UpdateOTelEnrichment`), so one rule is enforced in one place.
pub fn o_tel_enrichment_filters_error(
    include_filters: Option<&[OTelEnrichmentMetricSelector]>,
    exclude_filters: Option<&[OTelEnrichmentMetricSelector]>,
) -> Option<String> {
    let combined = include_filters.map_or(0, <[OTelEnrichmentMetricSelector]>::len)
        + exclude_filters.map_or(0, <[OTelEnrichmentMetricSelector]>::len);
    if combined > MAX_O_TEL_ENRICHMENT_FILTERS {
        return Some(format!(
            "A maximum of {MAX_O_TEL_ENRICHMENT_FILTERS} filters is allowed across \
             IncludeFilters and ExcludeFilters combined"
        ));
    }
    None
}

/// The stored filter list as the modelled member reports it: `None` for an empty list, which
/// every enrichment response documents as "omitted" rather than empty — an omitted
/// `IncludeFilters` means every supported namespace is in scope, and an omitted
/// `ExcludeFilters` means nothing is excluded.
pub fn o_tel_enrichment_filters(
    filters: &[OTelEnrichmentMetricSelector],
) -> Option<Vec<OTelEnrichmentMetricSelector>> {
    (!filters.is_empty()).then(|| filters.to_vec())
}

/// A stored anomaly detection model: the modelled shape `DescribeAnomalyDetectors` reports,
/// and when the training that creating it started comes due.
///
/// Held as the modelled [`AnomalyDetector`] itself rather than a parallel record, so every
/// member `PutAnomalyDetector` accepts round-trips through `DescribeAnomalyDetectors` without
/// a field-by-field copy that can silently drop one — the same way an alarm is held.
#[derive(Debug, Clone, Default)]
pub struct StoredAnomalyDetector {
    /// The detector as `DescribeAnomalyDetectors` reports it.
    pub detector: AnomalyDetector,
    /// When this detector's training is owed to have finished, as epoch seconds. Until that
    /// instant the model is still being trained, which is the `PENDING_TRAINING` its
    /// `StateValue` carries.
    pub trained_due_secs: f64,
}

/// `PENDING_TRAINING`, one of the three modelled `AnomalyDetectorStateValue` values
/// ([`crate::ops::types::ANOMALY_DETECTOR_STATE_VALUE_VALUES`]): the state a model is in while
/// it is still being trained, which is what a freshly written detector reports.
pub const ANOMALY_DETECTOR_PENDING_TRAINING: &str = "PENDING_TRAINING";

/// `TRAINED`: the model has been trained on the metric's data, so a band drawn from it says
/// what that metric's values are expected to be.
pub const ANOMALY_DETECTOR_TRAINED: &str = "TRAINED";

/// `TRAINED_INSUFFICIENT_DATA`: training has finished, but the metric the detector watches has
/// published nothing for the model to learn from.
pub const ANOMALY_DETECTOR_TRAINED_INSUFFICIENT_DATA: &str = "TRAINED_INSUFFICIENT_DATA";

/// `SINGLE_METRIC`, one of the two modelled `AnomalyDetectorType` values
/// ([`crate::ops::types::ANOMALY_DETECTOR_TYPE_VALUES`]): a detector over one metric and
/// statistic.
pub const ANOMALY_DETECTOR_TYPE_SINGLE_METRIC: &str = "SINGLE_METRIC";

/// `METRIC_MATH`, the other modelled `AnomalyDetectorType` value: a detector over the single
/// time series a metric math expression returns.
pub const ANOMALY_DETECTOR_TYPE_METRIC_MATH: &str = "METRIC_MATH";

/// How long after the write that stored an anomaly detector its training is owed to have
/// finished.
///
/// A model is created "pending training" and only then reports a trained state, so the
/// trained state is not due at the instant of the write: a caller reading straight after the
/// write still sees the training it started. CloudWatch publishes no duration for real
/// training (it reads hours of the metric's history), so this span is the emulator's own
/// choice inside the framework's 250-750 ms window for a transition the clock owes, and it is
/// never slept on — the due instant is stored and the transition is materialised by the first
/// call at or after it ([`ComAmazonawsCloudwatchGraniteserviceversion20100801State::settle_anomaly_detector_training`]).
pub const TRAINING_DELAY_SECS: f64 = 0.5;

/// How far back the history an anomaly detection model is trained on reaches.
///
/// A model is trained on the metric's own past — "You can use the model to display a band of
/// expected normal values when the metric is graphed." (`PutAnomalyDetector`) — and neither the
/// model nor the operation publishes how much of that past it reads, so the span is the
/// emulator's own bounded choice: the whole of the tier a one-minute read still reaches
/// ([`retained_at_period`]), so the history a model trains on is every datapoint the store can
/// still report at that granularity and the walk it costs stays bounded by the series itself.
pub const ANOMALY_TRAINING_SPAN_SECS: f64 = FIFTEEN_DAYS_SECS;

/// The granularity the history a model is trained on is folded at: one minute, the finest
/// granularity a datapoint stays readable at over the whole training span
/// ([`retained_at_period`]).
pub const ANOMALY_TRAINING_PERIOD_SECS: i32 = 60;

/// The largest number of anomaly detection models this in-memory store holds.
///
/// A capacity bound, not an AWS quota: the detector records live in this process's memory, so
/// their number has to be finite. `PutAnomalyDetector` answers its modelled
/// `LimitExceededException` ("The operation exceeded one or more limits.") rather than growing
/// without limit once it is reached; updating a detector that already exists adds none, so it
/// is unaffected.
pub const MAX_ANOMALY_DETECTORS: usize = 10_000;

/// The most excluded time ranges one model's training configuration carries: "You can specify
/// as many as 10 time ranges." (`PutAnomalyDetectorInput$Configuration`)
pub const MAX_EXCLUDED_TIME_RANGES: usize = 10;

/// The deprecated flat single-metric members a detector request may carry instead of its
/// `SingleMetricAnomalyDetector` structure ("Use SingleMetricAnomalyDetector.",
/// `PutAnomalyDetectorInput$Namespace`), so the two writers that accept both spellings read
/// one rule.
#[derive(Debug, Clone, Copy, Default)]
pub struct FlatSingleMetric<'a> {
    /// The request's `Namespace`.
    pub namespace: Option<&'a str>,
    /// The request's `MetricName`.
    pub metric_name: Option<&'a str>,
    /// The request's `Dimensions`.
    pub dimensions: Option<&'a [Dimension]>,
    /// The request's `Stat`.
    pub stat: Option<&'a str>,
}

impl FlatSingleMetric<'_> {
    /// The flat members this request carries, named as the model names them.
    fn named(&self) -> Vec<&'static str> {
        let mut named = Vec::new();
        if self.dimensions.is_some() {
            named.push("Dimensions");
        }
        if self.metric_name.is_some() {
            named.push("MetricName");
        }
        if self.namespace.is_some() {
            named.push("Namespace");
        }
        if self.stat.is_some() {
            named.push("Stat");
        }
        named
    }

    /// The flat members as the `SingleMetricAnomalyDetector` they are the deprecated spelling
    /// of, so one stored shape serves either spelling.
    fn as_structure(&self) -> SingleMetricAnomalyDetector {
        SingleMetricAnomalyDetector {
            account_id: None,
            namespace: self.namespace.map(str::to_string),
            metric_name: self.metric_name.map(str::to_string),
            dimensions: self.dimensions.map(<[Dimension]>::to_vec),
            stat: self.stat.map(str::to_string),
        }
    }
}

/// Which of the two forms of anomaly detector a request named, normalised: the deprecated flat
/// members and the `SingleMetricAnomalyDetector` structure are the same detector, so they
/// arrive here as one.
#[derive(Debug, Clone)]
pub enum AnomalyDetectorForm {
    /// A detector over one metric and statistic.
    SingleMetric(SingleMetricAnomalyDetector),
    /// A detector over the time series a metric math expression returns.
    MetricMath(MetricMathAnomalyDetector),
}

impl AnomalyDetectorForm {
    /// This form's modelled `AnomalyDetectorType`, which is what
    /// `DescribeAnomalyDetectors$AnomalyDetectorTypes` filters on.
    pub fn detector_type(&self) -> &'static str {
        match self {
            Self::SingleMetric(_) => ANOMALY_DETECTOR_TYPE_SINGLE_METRIC,
            Self::MetricMath(_) => ANOMALY_DETECTOR_TYPE_METRIC_MATH,
        }
    }

    /// The unique identifier of the detector this form names.
    ///
    /// A detector is identified by WHAT it watches — the metric, its dimensions and the
    /// statistic for a single-metric detector, and the queries for a metric-math one — so a
    /// second write naming the same thing updates that model rather than creating a second
    /// one, which is what lets `PutAnomalyDetector` report "the anomaly detector that you
    /// created or updated" and lets a delete that names a metric find what a put created. The
    /// training configuration and the metric characteristics are NOT part of the identity:
    /// they are properties of the model rather than of the series it is trained on.
    ///
    /// The identifier is a digest of that identity in hex, which keeps it inside the modelled
    /// `AnomalyDetectorId` (1..128, `^[A-Za-z0-9_./:%()+-]+$`) whatever a metric, a dimension
    /// or an expression is spelled with.
    pub fn detector_id(&self) -> String {
        ::hex::encode(<::sha2::Sha256 as ::sha2::Digest>::digest(
            self.identity().as_bytes(),
        ))
    }

    /// What this form watches, rendered canonically: the dimension set is normalised, so the
    /// same metric named with its dimensions in another order is the same detector.
    fn identity(&self) -> String {
        let mut document = Map::new();
        document.insert(
            "type".to_string(),
            Value::from(self.detector_type().to_string()),
        );
        match self {
            Self::SingleMetric(single) => {
                insert_member(&mut document, "accountId", json_text(&single.account_id));
                insert_member(&mut document, "namespace", json_text(&single.namespace));
                insert_member(&mut document, "metricName", json_text(&single.metric_name));
                document.insert(
                    "dimensions".to_string(),
                    dimensions_document(&dimension_list(&dimension_set(
                        single.dimensions.as_deref(),
                    ))),
                );
                insert_member(&mut document, "stat", json_text(&single.stat));
            }
            Self::MetricMath(math) => {
                document.insert(
                    "metricDataQueries".to_string(),
                    Value::Array(
                        math.metric_data_queries
                            .iter()
                            .flatten()
                            .map(metric_query_document)
                            .collect(),
                    ),
                );
            }
        }
        Value::Object(document).to_string()
    }

    /// The metric and statistic a single-metric detector watches, or `None` for a metric-math
    /// one, which watches an expression instead.
    pub fn single_metric(&self) -> Option<&SingleMetricAnomalyDetector> {
        match self {
            Self::SingleMetric(single) => Some(single),
            Self::MetricMath(_) => None,
        }
    }

    /// The single-metric form of the metric a series was retrieved for, so the model created
    /// for that metric is found under the identity its write was keyed by.
    pub fn of_series_metric(metric: &metric_math::SeriesMetric) -> Self {
        Self::SingleMetric(SingleMetricAnomalyDetector {
            // A series this store holds belongs to the account, so it is never one a
            // cross-account detector named another account for.
            account_id: None,
            namespace: Some(metric.namespace.clone()),
            metric_name: Some(metric.metric_name.clone()),
            dimensions: Some(dimension_list(&metric.dimensions)),
            stat: Some(metric.stat.clone()),
        })
    }
}

/// Why a request does not name exactly one anomaly detector.
#[derive(Debug, Clone)]
pub enum AnomalyDetectorFormError {
    /// Parameters that cannot be used together, reported as the parameter-combination fault
    /// both detector writers model.
    Combination(String),
    /// A parameter the detector cannot be identified without is missing, reported as the
    /// missing-parameter fault both detector writers model.
    Missing(String),
}

/// Which detector a request names, or why it names none.
///
/// Shared by `PutAnomalyDetector` and `DeleteAnomalyDetector`: both accept the same three
/// spellings — the `SingleMetricAnomalyDetector` structure, the `MetricMathAnomalyDetector`
/// structure, and the deprecated flat metric members — under the same rule, so neither can
/// accept a combination the other refuses.
pub fn anomaly_detector_form(
    single: Option<&SingleMetricAnomalyDetector>,
    math: Option<&MetricMathAnomalyDetector>,
    flat: FlatSingleMetric<'_>,
) -> Result<AnomalyDetectorForm, AnomalyDetectorFormError> {
    let mut found: Vec<&str> = Vec::new();
    if !flat.named().is_empty() {
        found.push(FLAT_METRIC_FORM);
    }
    if single.is_some() {
        found.push(SINGLE_METRIC_FORM);
    }
    if math.is_some() {
        found.push(METRIC_MATH_FORM);
    }
    match (found.len(), single, math) {
        (0, _, _) => Err(AnomalyDetectorFormError::Missing(format!(
            "Either {FLAT_METRIC_FORM}, {SINGLE_METRIC_FORM} or {METRIC_MATH_FORM} must be set."
        ))),
        (1, Some(single), _) => Ok(AnomalyDetectorForm::SingleMetric(single.clone())),
        (1, _, Some(math)) => Ok(AnomalyDetectorForm::MetricMath(math.clone())),
        // The deprecated spelling of a single-metric detector: the flat members are the
        // structure's, so they are read as that structure from here on.
        (1, None, None) => Ok(AnomalyDetectorForm::SingleMetric(flat.as_structure())),
        _ => Err(AnomalyDetectorFormError::Combination(
            one_detector_form_message(&found),
        )),
    }
}

/// The deprecated flat metric members, as a refusal naming the forms a request mixed names
/// them.
pub const FLAT_METRIC_FORM: &str = "single metric attributes";

/// The `SingleMetricAnomalyDetector` structure, as such a refusal names it.
pub const SINGLE_METRIC_FORM: &str = "SingleMetricAnomalyDetector";

/// The `MetricMathAnomalyDetector` structure, as such a refusal names it.
pub const METRIC_MATH_FORM: &str = "MetricMathAnomalyDetector";

/// `AnomalyDetectorId`, as such a refusal names it.
pub const DETECTOR_ID_FORM: &str = "AnomalyDetectorId";

/// CloudWatch's wording for a request that names a detector more than one way: the three forms
/// one of which may be set, and then the ones the request was found carrying, in that order.
pub fn one_detector_form_message(found: &[&str]) -> String {
    format!(
        "Either {FLAT_METRIC_FORM}, {SINGLE_METRIC_FORM} or {METRIC_MATH_FORM} can be set, \
         found {}.",
        found.join(", ")
    )
}

/// Why the detector `form` names cannot be identified, or `None` when it can.
///
/// A single-metric detector is the metric AND the statistic — "The statistic to use for the
/// metric and the anomaly detection model." (`PutAnomalyDetectorInput$Stat`) — so a form
/// missing any of the three names no model to create or to find. A metric-math detector is
/// identified by its queries, which the writers check separately.
pub fn anomaly_detector_form_missing(form: &AnomalyDetectorForm) -> Option<String> {
    let single = form.single_metric()?;
    for (member, value) in [
        ("Namespace", &single.namespace),
        ("MetricName", &single.metric_name),
        ("Stat", &single.stat),
    ] {
        if value.as_deref().unwrap_or_default().is_empty() {
            return Some(format!("The parameter {member} must be specified."));
        }
    }
    None
}

/// What the model `stored` trained says the values of `points` are expected to be, or `None`
/// when it says nothing about them yet.
///
/// The band `ANOMALY_DETECTION_BAND` draws is the model's: "You can use the model to display a
/// band of expected normal values when the metric is graphed." (`PutAnomalyDetector`) — so what
/// is normal is what the series' OWN history has been, and the band's width is counted in the
/// deviation of that history, which is what makes a wider band tolerate more. The model is
/// therefore the mean of the values it was trained on and how far they spread from it, and the
/// band reaches the requested number of those spreads either side of the mean.
///
/// `history` is what the model was TRAINED on and `points` are the instants the band is
/// reported at: the two are separate because a model is trained on the metric's past — "the
/// model ... is based on the metric's historical data" — and not on the span a caller happens
/// to read, so the band a graph shows over one window and the band an alarm compares one period
/// against are the same numbers.
///
/// It says nothing in two cases: while the model is still being trained (the trained state is
/// owed at `now_secs`, so a caller reading before then sees the training its write started),
/// and when the training was left with no value at all — a model trained on nothing has no
/// expectation to report, which is the insufficient-data state it carries.
///
/// "An array of time ranges to exclude from use when the anomaly detection model is trained.
/// Use this to make sure that events that could cause unusual values for the metric ... are not
/// used when CloudWatch trains the model." (`AnomalyDetectorConfiguration$ExcludedTimeRanges`)
/// — so a point inside an excluded range is left out of the training while still being a point
/// the band is reported at.
pub fn anomaly_model_of(
    stored: &StoredAnomalyDetector,
    history: &[(f64, f64)],
    points: &[(f64, f64)],
    now_secs: f64,
) -> Option<metric_math::AnomalyModel> {
    if stored.trained_due_secs > now_secs {
        return None;
    }
    let excluded: Vec<(f64, f64)> = stored
        .detector
        .configuration
        .iter()
        .flat_map(|configuration| configuration.excluded_time_ranges.iter().flatten())
        .map(|range| {
            (
                from_timestamp(&range.start_time),
                from_timestamp(&range.end_time),
            )
        })
        .collect();
    let trained: Vec<f64> = history
        .iter()
        .filter(|(at, _)| {
            !excluded
                .iter()
                .any(|(start, end)| *at >= *start && *at < *end)
        })
        .map(|(_, value)| *value)
        .collect();
    if trained.is_empty() {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let count = trained.len() as f64;
    let expected = trained.iter().sum::<f64>() / count;
    let deviation = (trained
        .iter()
        .map(|value| (value - expected) * (value - expected))
        .sum::<f64>()
        / count)
        .sqrt();
    Some(metric_math::AnomalyModel {
        expected: points.iter().map(|(at, _)| (*at, expected)).collect(),
        deviation: vec![deviation; points.len()],
    })
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
    /// Log alarms by alarm name.
    pub log_alarms: HashMap<String, StoredLogAlarm>,
    /// Alarm history by alarm name, in append order.
    pub alarm_history: HashMap<String, Vec<StoredHistoryItem>>,
    /// The sequence number the next recorded history item takes.
    pub next_history_sequence: u64,
    /// Metric streams by the Region they live in and their name ([`MetricStreamKey`]).
    pub metric_streams: HashMap<MetricStreamKey, StoredMetricStream>,
    /// Contributor Insights rules by rule name.
    pub insight_rules: HashMap<String, StoredInsightRule>,
    /// The managed Contributor Insights rule templates this account can enable rules from, as
    /// the service publishes them.
    ///
    /// The catalog is the SERVICE's side of `ManagedRule$TemplateName` ("as returned by
    /// `ListManagedInsightRules`"): a caller names a template, it never adds one. It starts
    /// EMPTY, which is the state the recorded service answers from for every resource this
    /// emulator can hold — `ListManagedInsightRules` reports no available managed rule for a
    /// live DynamoDB table or a live CloudWatch alarm, and every template name tried for either
    /// is answered "could not find template" — so a `PutManagedInsightRules` entry is reported
    /// back among the rules that could not be enabled until a template of that name is
    /// published for the entry's resource through [`Self::publish_managed_rule_template`].
    ///
    /// An empty catalog therefore says that THIS emulator publishes no managed-rule template,
    /// not that no template exists anywhere: a template enters the catalog through that seam,
    /// for the vendor and resource type it collects data for, when a service this emulator
    /// serves publishes one.
    pub managed_rule_templates: Vec<ManagedRuleTemplate>,
    /// Anomaly detection models by their unique identifier
    /// ([`AnomalyDetectorForm::detector_id`]), which is what identifies the metric and
    /// statistic (or the queries) a model was created for, so a second write for the same
    /// thing updates that model rather than adding a second one.
    pub anomaly_detectors: HashMap<String, StoredAnomalyDetector>,
    /// Dashboards by dashboard name.
    ///
    /// Not keyed by Region: "All dashboards in your account are global, not region-specific."
    /// (`PutDashboard`), so a dashboard written through one Region's endpoint is the same
    /// dashboard every other Region's endpoint reads.
    pub dashboards: HashMap<String, StoredDashboard>,
    /// The action notifications the alarm state transitions this store has materialised owe
    /// but nothing has delivered yet, oldest first. Bounded by
    /// [`alarm_evaluation::MAX_PENDING_NOTIFICATIONS`]; drained by
    /// [`alarm_actions::settle_alarms`], which is what delivers them.
    pub pending_notifications: VecDeque<alarm_actions::AlarmNotification>,
    /// The in-process task that waits for this store's due alarm evaluations and delivers
    /// what the transitions they materialise owe ([`alarm_actions::drive_alarm_clock`]), when
    /// one is running.
    pub alarm_clock: Option<::tokio::task::JoinHandle<()>>,
    /// Whether an alarm clock is running for this store: the CLAIM, so at most one runs.
    pub alarm_clock_running: bool,
    /// Which claim on the alarm clock is current, so a clock that stands down releases only
    /// its own.
    pub alarm_clock_generation: u64,
    /// The account's vended-metric enrichment configuration, once any of the enrichment
    /// operations has stored one.
    pub otel_enrichment: Option<StoredOTelEnrichment>,
    /// The customer managed KMS key associated with a Region's default dataset, by Region.
    ///
    /// There is no dataset record to create: "The default dataset is implicit for every
    /// account in every Region" (`GetDataset`), so a Region's dataset exists whether or not
    /// this map holds an entry for it, and the only thing a dataset carries is the key
    /// association `AssociateDatasetKmsKey` writes and `DisassociateDatasetKmsKey` removes.
    /// An absent entry is therefore not an absent dataset: it is a dataset encrypted with an
    /// Amazon Web Services owned key, which is why `GetDataset` then omits `KmsKeyArn`.
    pub dataset_kms_keys: HashMap<String, String>,
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

/// Epoch seconds as the modelled timestamp type, held to the MILLISECOND.
///
/// One instant is reported two ways: as the modelled timestamp member (which this protocol
/// puts on the wire as epoch seconds) and, inside a notification document or an explanation, as
/// the text the timestamp type renders. The rendering carries a fixed number of fractional
/// digits while the number carries the whole fraction, so an instant with a finer fraction than
/// the rendering keeps would read as two different instants depending on which member a client
/// looked at. Holding the instant to the millisecond — finer than anything CloudWatch reports
/// about an alarm, and exactly representable in both forms — makes the two readings agree.
///
/// An instant too large for the millisecond form keeps the whole seconds it has rather than
/// being clamped: it is outside any instant this emulator mints.
pub fn to_timestamp(secs: f64) -> ::aws_smithy_types::DateTime {
    /// The largest number of milliseconds the modelled timestamp's integer form holds.
    const MAX_MILLIS: f64 = 9.0e15;

    let millis = (secs * 1_000.0).round();
    if millis.is_finite() && millis.abs() < MAX_MILLIS {
        #[allow(clippy::cast_possible_truncation)]
        return ::aws_smithy_types::DateTime::from_millis(millis as i64);
    }
    ::aws_smithy_types::DateTime::from_secs_f64(secs)
}

/// A modelled timestamp as epoch seconds.
pub fn from_timestamp(stamp: &::aws_smithy_types::DateTime) -> f64 {
    stamp.as_secs_f64()
}

/// `secs` as the instant a human-readable explanation names it, or `None` for an instant the
/// timestamp type cannot render.
///
/// The rendering is the modelled timestamp type's own
/// ([`::aws_smithy_types::date_time::Format::DateTime`], RFC 3339), so an instant a document
/// reports and one a sentence names are the same text, and no second date implementation
/// lives here.
pub fn rfc3339(secs: f64) -> Option<String> {
    to_timestamp(secs)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// `secs` as the instant one of this service's JSON DOCUMENTS writes it, or `None` for an
/// instant the timestamp type cannot render.
///
/// Every instant CloudWatch stamps INSIDE one of the JSON documents it reports — an alarm's
/// `StateReasonData` and the `HistoryData` that records it, and the notification document an
/// action carries — is written with a three-digit millisecond field and a NUMERIC UTC offset
/// (`2026-09-22T23:06:00.000+0000`), which is what lets a consumer parse it with one
/// `yyyy-MM-dd'T'HH:mm:ss.SSSZ` pattern. The fixed parts of the form are the `-`/`T`/`:`
/// separators, the `.` before the milliseconds and the trailing offset; the variable part is
/// the instant itself. An instant written with a `Z` suffix, or with no fractional field, is
/// NOT that form: a client parsing a document with that pattern fails on it instead of
/// recovering the moment the document is about.
///
/// The calendar fields come from the modelled timestamp type's own rendering ([`rfc3339`]) and
/// are only re-shaped here, so no second date implementation lives in this crate and an instant
/// a document names is the instant the modelled member beside it reports.
pub fn document_instant(secs: f64) -> Option<String> {
    /// How many fractional digits the form carries: milliseconds, written whether or not the
    /// instant has a fraction.
    const FRACTION_DIGITS: usize = 3;
    /// The numeric offset the form ends with — every instant this service reports is UTC.
    const UTC_OFFSET: &str = "+0000";

    let rendered = rfc3339(secs)?;
    // The modelled rendering writes the zone as `Z` and carries only as many fractional digits
    // as the instant needs, so the zone is replaced and the fraction padded (or cut) to the
    // fixed width of this form.
    let instant = rendered.strip_suffix('Z').unwrap_or(rendered.as_str());
    let (whole, fraction) = instant.split_once('.').unwrap_or((instant, ""));
    let millis: String = fraction
        .chars()
        .chain(::std::iter::repeat_n('0', FRACTION_DIGITS))
        .take(FRACTION_DIGITS)
        .collect();
    Some(format!("{whole}.{millis}{UTC_OFFSET}"))
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

/// The ARN of the CloudWatch Logs scheduled query a log alarm's configuration names:
/// `arn:aws:logs:<region>:<account>:scheduled-query:<AlarmName>`.
///
/// "When you create a log alarm, the operation creates a service-managed CloudWatch Logs
/// scheduled query" (`PutLogAlarm`), whose ARN `DescribeAlarms` reports back
/// (`ScheduledQueryConfiguration$QueryARN`). It is derived from the alarm's own name, so it
/// is stable across reads and names the same managed query for the life of the alarm.
pub fn scheduled_query_arn(region: &str, alarm_name: &str) -> String {
    ::engine::emulator_arn("logs", region, &format!("scheduled-query:{alarm_name}"))
}

/// A metric stream's ARN: `arn:aws:cloudwatch:<region>:<account>:metric-stream/<Name>`.
pub fn metric_stream_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("cloudwatch", region, &format!("metric-stream/{name}"))
}

/// A dashboard's ARN: `arn:aws:cloudwatch::<account>:dashboard/<DashboardName>`.
///
/// The Region segment is EMPTY because a dashboard is not a resource of one Region: "All
/// dashboards in your account are global, not region-specific." (`PutDashboard`), and the ARN
/// of a dashboard is documented as `arn:aws:cloudwatch::account-id:dashboard/dashboard-name`
/// (`TagResourceInput$ResourceARN`).
pub fn dashboard_arn(name: &str) -> String {
    ::engine::emulator_arn("cloudwatch", "", &format!("dashboard/{name}"))
}

/// How long a dashboard name may be: "The maximum length is 255"
/// (`PutDashboardInput$DashboardName`, `ListDashboardsInput$DashboardNamePrefix`).
pub const MAX_DASHBOARD_NAME_LEN: usize = 255;

/// How short a dashboard name may be: a dashboard is named, so a name of no characters names
/// no dashboard.
pub const MIN_DASHBOARD_NAME_LEN: usize = 1;

/// Why `value` is not a dashboard name, or `None` when it is one.
///
/// "The maximum length is 255, and valid characters are A-Z, a-z, 0-9, \"-\", and \"_\"."
/// (`PutDashboardInput$DashboardName`) — prose the model carries no `length` or `pattern` for,
/// so the boundary validator cannot refuse it and every operation that NAMES a dashboard
/// refuses it through this one rule. `field` is the member the value arrived in, so the
/// refusal names the field the caller got wrong.
pub fn dashboard_name_error(field: &str, value: &str) -> Option<String> {
    if value.chars().count() < MIN_DASHBOARD_NAME_LEN {
        return Some(format!(
            "The value for field {field} is too short. It must have length greater than or \
             equal to {MIN_DASHBOARD_NAME_LEN}.\n"
        ));
    }
    dashboard_name_characters_error(field, value, false)
}

/// Why `value` is not a dashboard name PREFIX, or `None` when it is one.
///
/// "The maximum length is 255, and valid characters are A-Z, a-z, 0-9, \".\", \"-\", and
/// \"_\"." (`ListDashboardsInput$DashboardNamePrefix`) — the same rule as a name's, widened by
/// the "." the prefix also allows, and satisfied by the empty prefix, which selects every
/// dashboard.
pub fn dashboard_name_prefix_error(field: &str, value: &str) -> Option<String> {
    dashboard_name_characters_error(field, value, true)
}

/// The length and character rule both dashboard-name members state, with the "." only the
/// prefix allows.
///
/// Both refusals are worded the way CloudWatch words a field whose value breaks its rule:
/// "The value for field `<field>` is too long. It must have length less than or equal to
/// `<max>`." and "The value for field `<field>` contains invalid characters. It can only
/// contain alphanumerics, dash (-) and underscore (_).", each closed by a newline, so every
/// field holding a dashboard name reports its rule the same way.
fn dashboard_name_characters_error(field: &str, value: &str, allow_dot: bool) -> Option<String> {
    if value.chars().count() > MAX_DASHBOARD_NAME_LEN {
        return Some(format!(
            "The value for field {field} is too long. It must have length less than or equal \
             to {MAX_DASHBOARD_NAME_LEN}.\n"
        ));
    }
    let valid = value.chars().all(|character| {
        character.is_ascii_alphanumeric()
            || character == '-'
            || character == '_'
            || (allow_dot && character == '.')
    });
    if valid {
        return None;
    }
    let characters = if allow_dot {
        "alphanumerics, dash (-), underscore (_) and period (.)"
    } else {
        "alphanumerics, dash (-) and underscore (_)"
    };
    Some(format!(
        "The value for field {field} contains invalid characters. It can only contain \
         {characters}.\n"
    ))
}

/// How many dashboards one `ListDashboards` page holds: "ListDashboards returns up to 1000
/// results on one page. If there are more than 1000 dashboards, you can call ListDashboards
/// again and include the value you received for NextToken in the first call, to receive the
/// next 1000 results." (`ListDashboards`)
pub const DASHBOARDS_PAGE_SIZE: usize = 1_000;

/// The largest number of dashboards this in-memory store holds.
///
/// A capacity bound, not an AWS quota: the dashboard documents live in this process's memory,
/// so their number has to be finite. `PutDashboard` answers `LimitExceededException` ("The
/// operation exceeded one or more limits.") rather than growing without limit once it is
/// reached; replacing a dashboard that already exists adds none, so it is unaffected.
pub const MAX_DASHBOARDS: usize = 10_000;

/// The largest dashboard document this in-memory store holds, in bytes.
///
/// A capacity bound, not an AWS quota, for the same reason as [`MAX_DASHBOARDS`]: a document
/// lives in this process's memory, so its size has to be finite, and a body above this is
/// refused with `LimitExceededException` ("The operation exceeded one or more limits.") before
/// it is parsed, checked or stored. Orders of magnitude above the dashboards AWS's own console
/// writes, so no document a caller authors meets it.
pub const MAX_DASHBOARD_BODY_BYTES: usize = 1_048_576;

/// The identifier of the one dataset this service has: "Only the default dataset is
/// supported." (`GetDataset`), and `GetDatasetOutput$DatasetId` is modelled `^default$`.
pub const DEFAULT_DATASET_ID: &str = "default";

/// A dataset's ARN: `arn:aws:cloudwatch:<region>:<account>:dataset/<dataset_id>`
/// (`GetDatasetOutput$Arn`).
pub fn dataset_arn(region: &str, dataset_id: &str) -> String {
    ::engine::emulator_arn("cloudwatch", region, &format!("dataset/{dataset_id}"))
}

/// Whether `identifier` names the default dataset of `region` in this account.
///
/// "you can specify either `default` or the full dataset Amazon Resource Name (ARN) in the
/// format `arn:aws:cloudwatch:Region:account-id:dataset/default`"
/// (`GetDatasetInput$DatasetIdentifier`): the bare identifier names the dataset of the Region
/// the request was signed for, and an ARN names it only when it is that dataset's own ARN. An
/// ARN carrying another partition, Region or account names a dataset in a scope this endpoint
/// does not serve, so it names no dataset the caller can reach here.
pub fn names_default_dataset(identifier: &str, region: &str) -> bool {
    identifier == DEFAULT_DATASET_ID || identifier == dataset_arn(region, DEFAULT_DATASET_ID)
}

/// What a dataset is called in the `ResourceType` member of a refusal that names one.
pub const DATASET_RESOURCE_TYPE: &str = "Dataset";

/// The ARN namespace of this service's own IAM actions, as an authorization failure names the
/// action it refused (`cloudwatch:GetDataset`).
const IAM_ACTION_NAMESPACE: &str = "cloudwatch";

/// How AWS states a request the caller is not allowed to make: who was refused, which action,
/// on which resource, and that nothing granted it.
///
/// The authorization layer in front of a service answers this for a resource the caller has no
/// grant for, which is a DIFFERENT answer from the resource not existing: a caller that may not
/// see a resource is never told whether it is there. The principal is the one this emulator's
/// credential speaks for (`service_common::caller_principal`), so the sentence names the
/// identity that really made the call, and the resource is the identifier the request carried.
///
/// Which POLICY the sentence says granted nothing follows from where the resource lives: a
/// resource in ANOTHER account can only be reached through a policy on the resource itself, so
/// such a request is refused as the RESOURCE policy allowing nothing — the sentence the shared
/// `service_common::iam_authorization::not_authorized_by_resource_policy` writes for every
/// service — while a resource of the caller's own account is reached through an identity-based
/// policy, which is the same sentence with that one clause replaced.
pub fn unauthorized_message(operation: &str, resource: &str) -> String {
    let principal = ::service_common::caller_principal::ambient_user_arn();
    let action = format!("{IAM_ACTION_NAMESPACE}:{operation}");
    let refused = ::service_common::iam_authorization::not_authorized_by_resource_policy(
        &principal, &action, resource,
    );
    if names_another_account(resource) {
        return refused;
    }
    // The one clause that differs, applied to the shared sentence rather than written again:
    // which kind of policy granted nothing.
    refused.replace("no resource-based policy", "no identity-based policy")
}

/// Whether `resource` is an ARN whose account field is not the account this emulator serves.
///
/// A value that is no ARN at all, or one carrying this account, names nothing in another
/// account. The account every ARN this emulator mints carries is the account contract's own
/// (`emulator_server::emulator_account_id`), so the comparison is against that rather than
/// against a literal.
fn names_another_account(resource: &str) -> bool {
    ::service_common::arn::parse(resource).is_some_and(|arn| {
        !arn.account.is_empty() && arn.account != ::emulator_server::emulator_account_id()
    })
}

/// How CloudWatch states a dataset that carries no customer managed KMS key, for the operation
/// that can only act on one that does.
///
/// A fixed sentence carrying the dataset the REQUEST named — `DisassociateDatasetKmsKey` is
/// documented to fail "if the dataset has no associated KMS key", and the identifier is the
/// only part of the sentence a caller can vary.
pub fn no_dataset_kms_key_message(identifier: &str) -> String {
    format!("No CMK association found for dataset {identifier}.")
}

/// Why `value` is not the ARN of an IAM role, or `None` when it is one.
///
/// A member that carries "the Amazon Resource Name (ARN) of an IAM role that CloudWatch
/// assumes" (`PutLogAlarmInput$ActionLogLineRoleArn`,
/// `ScheduledQueryConfiguration$ScheduledQueryRoleARN`) names an IAM role, so a value that is
/// not a role ARN at all could never be assumed by anything. Whether the role EXISTS, and
/// whether CloudWatch may assume it, is not decided here: that is settled when the query runs
/// and is reported on the alarm, not refused at configuration time.
pub fn role_arn_error(member: &str, value: &str) -> Option<String> {
    (!is_role_arn(value)).then(|| format!("{member} is not the ARN of an IAM role: {value}"))
}

/// Whether `value` is the ARN of an IAM role: `arn:<partition>:iam::<account>:role/<name>`.
///
/// The vendor-and-resource-type half is the shared requirement every member documented as the
/// ARN of an IAM role is judged by (`service_common::arn::IAM_ROLE`), so this crate and every
/// other service agree on what a role ARN is; the rest are the fields that requirement leaves
/// to the member — IAM is a global service, so a role's ARN carries no Region; it names an
/// account; and the role itself is named.
///
/// WHOSE account it names stays with the scope rule ([`foreign_arn_scope`]) — which is why
/// this is not `service_common::iam_role::role_name`, the reading that resolves a role of THIS
/// account: a role of another account IS a role ARN, and a member that requires its own
/// account says so in its own words rather than calling the value malformed.
pub fn is_role_arn(value: &str) -> bool {
    // ONE parse of the value: the shared requirement's own vendor-and-resource-type test is
    // re-stated against those fields rather than run as a second parse of the same string.
    ::service_common::arn::parse_of(value, ::service_common::arn::IAM_ROLE.vendor).is_some_and(
        |arn| {
            ::service_common::arn::IAM_ROLE
                .resource_type
                .is_none_or(|kind| arn.resource_type() == kind)
                && arn.region.is_empty()
                && !arn.account.is_empty()
                && arn
                    .resource
                    .strip_prefix("role/")
                    .is_some_and(|name| !name.is_empty())
        },
    )
}

/// The time units a `rate()` schedule expression accepts, each in its singular and plural
/// form: "Specify a `rate()` expression, for example `rate(5 minutes)`."
/// (`ScheduleConfiguration$ScheduleExpression`).
const RATE_UNITS: [&str; 6] = ["minute", "minutes", "hour", "hours", "day", "days"];

/// How many whitespace-separated fields a `cron()` schedule expression carries.
///
/// Five is the standard cron syntax the underlying scheduled query documents ("The expression
/// uses standard cron syntax and supports minute-level precision" — CloudWatch Logs
/// `CreateScheduledQuery$scheduleExpression`); six is that syntax with the year field AWS
/// schedule expressions add.
const CRON_FIELDS: [usize; 2] = [5, 6];

/// Why `expression` is not a schedule expression the underlying scheduled query can run on,
/// or `None` when it is one.
///
/// "The schedule expression that defines how often the underlying CloudWatch Logs scheduled
/// query runs. Specify a `rate()` expression, for example `rate(5 minutes)`."
/// (`ScheduleConfiguration$ScheduleExpression`), and the scheduled query itself takes a cron
/// expression (CloudWatch Logs `CreateScheduledQuery$scheduleExpression`) — so a schedule is
/// either a rate of a positive number of a documented time unit, or a cron expression.
pub fn schedule_expression_error(expression: &str) -> Option<String> {
    let trimmed = expression.trim();
    let inside = trimmed.strip_suffix(')');
    if let Some(rate) = inside.and_then(|head| head.strip_prefix("rate(")) {
        let fields: Vec<&str> = rate.split_whitespace().collect();
        let valid = matches!(fields.as_slice(), [value, unit]
            if value.parse::<u64>().is_ok_and(|value| value >= 1) && RATE_UNITS.contains(unit));
        return (!valid).then(|| {
            format!(
                "ScheduleExpression must be a rate of a positive number of {}: {expression}",
                RATE_UNITS.join(", ")
            )
        });
    }
    if let Some(cron) = inside.and_then(|head| head.strip_prefix("cron(")) {
        let fields = cron.split_whitespace().count();
        return (!CRON_FIELDS.contains(&fields)).then(|| {
            format!(
                "ScheduleExpression carries {fields} cron fields, and a cron expression has {} \
                 or {}: {expression}",
                CRON_FIELDS[0], CRON_FIELDS[1]
            )
        });
    }
    Some(format!(
        "ScheduleExpression is neither a rate() nor a cron() expression: {expression}"
    ))
}

/// A running metric stream: "The state of the metric stream. The possible values are
/// `running` and `stopped`." (`GetMetricStreamOutput$State`) — `PutMetricStream` creates a
/// stream in this state, and only `StopMetricStreams` moves it out of it.
pub const METRIC_STREAM_RUNNING: &str = "running";

/// The identity of one metric stream: the Region it lives in and its name.
///
/// A metric stream is a REGIONAL resource — its ARN carries the Region
/// (`arn:aws:cloudwatch:Region:account-id:metric-stream/metric-stream-name`,
/// `TagResourceInput$ResourceARN`) and its name "must be unique within the account and Region"
/// (`PutMetricStreamInput$Name`) — so one name in two Regions is two independent streams, with
/// their own filters, their own state and their own ARN. The Region is therefore part of the
/// key the store holds a stream under rather than merely part of what it reports, so a write
/// in one Region cannot reach another's stream.
///
/// Ordered by Region and then by name, which is the order a listing of ONE Region's streams
/// walks them in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MetricStreamKey {
    /// The Region the stream lives in, as the request that wrote it was signed for.
    pub region: String,
    /// The stream's name, unique within that Region.
    pub name: String,
}

/// The key the metric stream `name` of `region` is held under.
pub fn metric_stream_key(region: &str, name: &str) -> MetricStreamKey {
    MetricStreamKey {
        region: region.to_string(),
        name: name.to_string(),
    }
}

/// A stopped metric stream: the other of the two values
/// (`GetMetricStreamOutput$State`) — `StopMetricStreams` moves a stream into this state and
/// `StartMetricStreams` moves it back.
pub const METRIC_STREAM_STOPPED: &str = "stopped";

/// How CloudWatch states a metric-stream name that is not one a stream can carry.
///
/// A fixed sentence with no variable part: the member's own rule is published
/// ("Valid characters are A-Z, a-z, 0-9, \"-\" and \"_\".", `PutMetricStreamInput$Name`), so
/// the refusal names the fault rather than echoing the value. Every operation that acts on or
/// reports the stream a NAME identifies refuses one the same way, which is why the sentence
/// lives here.
pub const INVALID_METRIC_STREAM_NAME: &str = "Invalid Name";

/// How CloudWatch states a metric stream's `RoleArn` that is not the ARN of an IAM role —
/// the same fixed form as [`INVALID_METRIC_STREAM_NAME`], and for the same reason.
pub const INVALID_METRIC_STREAM_ROLE_ARN: &str = "Invalid Role ARN";

/// Whether `name` is a name a metric stream can carry: "Valid characters are A-Z, a-z, 0-9,
/// \"-\" and \"_\"." (`PutMetricStreamInput$Name`) — prose the model carries no `pattern`
/// for, so the boundary validator cannot judge it and every metric-stream operation judges
/// it here. The LENGTH is the model's own (`MetricStreamName`, 1..255) and is not re-checked.
pub fn is_metric_stream_name(name: &str) -> bool {
    name.chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '_')
}

/// [`INVALID_METRIC_STREAM_NAME`] when `name` is no metric-stream name, else `None`.
///
/// The one entry point every metric-stream operation that ACTS ON or REPORTS the stream a
/// name identifies refuses a bad NAME through, so none of them can answer one name two ways.
/// `DeleteMetricStream` is not one of them: it asks for the ABSENCE of a stream under the
/// name, which a name no stream can carry already satisfies, so it accepts one.
pub fn metric_stream_name_error(name: &str) -> Option<&'static str> {
    (!is_metric_stream_name(name)).then_some(INVALID_METRIC_STREAM_NAME)
}

/// [`INVALID_METRIC_STREAM_ROLE_ARN`] when `value` is not the ARN of an IAM role, else
/// `None`.
///
/// "The ARN of an IAM role that this metric stream will use to access Amazon Kinesis Data
/// Firehose resources." (`PutMetricStreamInput$RoleArn`) — a value that is no role ARN names
/// no role to assume, so the stream it configures could never deliver anything.
pub fn metric_stream_role_arn_error(value: &str) -> Option<&'static str> {
    (!is_role_arn(value)).then_some(INVALID_METRIC_STREAM_ROLE_ARN)
}

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

/// What a read that asks for more data points than one call returns is told.
///
/// "The maximum number of data points returned from a single call is 1,440. If you request
/// more than 1,440 data points, CloudWatch returns an error. To reduce the number of data
/// points, you can narrow the specified time range and make multiple requests across adjacent
/// time ranges, or you can increase the specified period." (`GetMetricStatistics`) — the
/// sentence states what the request asked for, the cap it broke, and the two remedies, so the
/// caller can turn it into a request that is served. Both numbers come from the request and
/// the cap, never from fixed text.
///
/// One wording for every read whose answer is one datapoint per period of a caller-chosen
/// window, since the condition and the two remedies are the same whichever of them asked:
/// `GetMetricStatistics` over a metric's series, and `GetInsightRuleReport` over a Contributor
/// Insights rule's.
pub fn datapoint_cap_exceeded(requested: f64, cap: f64) -> String {
    format!(
        "You have requested up to {requested:.0} datapoints, which exceeds the limit of \
         {cap:.0}. You may reduce the datapoints requested by increasing Period, or decreasing \
         the time range."
    )
}

/// CloudWatch's published alarm quota: 5,000 alarms per Region per account — which is also
/// what `LimitExceededFault` ("The quota for alarms for this customer has already been
/// reached") reports on `PutMetricAlarm`.
pub const MAX_ALARMS: usize = 5_000;

/// An alarm's action list as the stored alarm carries it, and so as every reader of an alarm
/// reports it: the actions the request configured, and the EMPTY list when it configured none.
///
/// `OKActions`, `AlarmActions` and `InsufficientDataActions` are `ResourceList`s of
/// `length 0..5` on every alarm writer, so "no action" is a list of none rather than no list:
/// a caller iterating a described alarm's actions, or reading one alarm to write it back, is
/// never left telling an empty list from an absent one. Shared by the alarm writers so one
/// rule answers them all.
pub fn configured_actions(actions: Option<&[String]>) -> Option<Vec<String>> {
    configured_list(actions)
}

/// One list member of a request as the stored resource carries it, and so as every reader of
/// that resource reports it: the entries the request named, and the EMPTY list when it named
/// none.
///
/// The protocols this service is served over cannot express the difference: on the query
/// protocol an omitted list and an empty one are the same bytes — neither carries a
/// `<member>.member.N` key — so a reader that reported them differently would show a caller a
/// change between two requests that were identical on the wire. A list member whose model
/// admits none of its entries (`length 0..n`) therefore reads back as the list it names, and
/// "none" is a list of none rather than no list.
pub fn configured_list<T: Clone>(entries: Option<&[T]>) -> Option<Vec<T>> {
    Some(entries.unwrap_or_default().to_vec())
}

/// The Region segment of `value` when it is an ARN that names a Region, else `None`.
///
/// Read through the ONE ARN parse (`service_common::arn`), so this service and every other
/// read the same form. A value that is not a well-formed ARN names no Region, and neither does
/// an ARN whose Region field is EMPTY: a resource that is not Region-scoped is named that way,
/// which is why the action values this operation documents include
/// `arn:aws:ssm-incidents::<account-id>:responseplan/<response-plan-name>`
/// (`PutMetricAlarmInput$AlarmActions`).
fn arn_region(value: &str) -> Option<&str> {
    let region = ::service_common::arn::parse(value)?.region;
    (!region.is_empty()).then_some(region)
}

/// Why one of the actions in `actions` names a Region other than `region`, or `None` when none
/// of them does.
///
/// CloudWatch executes an alarm's actions in the Region the alarm itself lives in, so an
/// action naming another Region is one the alarm could never carry out: the whole request is
/// refused rather than stored, and the refusal names the Region the request asked for and the
/// one this endpoint acts in. Shared by the alarm writers, whose `OKActions`, `AlarmActions`
/// and `InsufficientDataActions` carry the same action values.
pub fn foreign_action_region_error(
    actions: [Option<&[String]>; 3],
    region: &str,
) -> Option<String> {
    actions
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|action| arn_region(action))
        .find(|named| *named != region)
        .map(|named| format!("Invalid region {named} specified. Only {region} is supported."))
}

/// Which part of this endpoint's scope an ARN falls outside of ([`foreign_arn_scope`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForeignArnScope {
    /// The ARN names another Region, or another AWS partition: either way the resource it
    /// names is in another location than the one serving the request.
    Location,
    /// The ARN names another account.
    Account,
}

/// Which part of the scope of a resource this endpoint holds in `region` the ARN `value` falls
/// outside of, or `None` when it names a resource inside that scope.
///
/// A resource configured to use another AWS resource reaches it with this account's own
/// credentials, in the location the configuration itself lives in, so an ARN naming another
/// account — or another Region or partition, when the resource it names is Region-scoped —
/// names something the configuration could never reach. What it is judged against is what this
/// emulator itself hands out: the partition every ARN it mints carries ([`serving_partition`]),
/// the Region the request is served in, and this account's own id. An ARN naming no Region
/// (`arn:aws:iam::<account>:role/…`, a resource that is not Region-scoped) is judged on its
/// partition and account alone.
///
/// A value that is no well-formed ARN is not judged here: it names no account or Region to
/// compare, so its own member's form rule decides it. The WORDS of a refusal are the caller's,
/// because they name the resource being configured.
pub fn foreign_arn_scope(value: &str, region: &str) -> Option<ForeignArnScope> {
    let named = ::service_common::arn::parse(value)?;
    let serving = serving_partition();
    if (!serving.is_empty() && named.partition != serving)
        || (!named.region.is_empty() && named.region != region)
    {
        return Some(ForeignArnScope::Location);
    }
    (named.account != ::emulator_server::emulator_account_id())
        .then_some(ForeignArnScope::Account)
}

/// The partition every ARN this emulator mints carries.
///
/// Read ONCE off the ARN builder the whole emulator mints with (`engine`'s) and through the one
/// ARN parse (`service_common::arn`), so what a request is judged against is exactly what the
/// service hands out and nothing here reads the ARN form a second time. A partition is not
/// Region- or service-specific, so any ARN the builder renders carries the same one.
fn serving_partition() -> &'static str {
    static PARTITION: ::std::sync::OnceLock<String> = ::std::sync::OnceLock::new();
    PARTITION.get_or_init(|| {
        let minted = ::engine::emulator_arn("cloudwatch", "", "scope");
        ::service_common::arn::parse(&minted)
            .map(|arn| arn.partition.to_owned())
            // The builder renders every field that parse requires, so this cannot miss; an
            // unreadable reference leaves the partition UNJUDGED rather than making every ARN
            // foreign, so a change to the builder could never turn into a wall of refusals.
            .unwrap_or_default()
    })
}

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

/// The schema NAME every Contributor Insights rule a caller creates is written against: "For
/// rules that you create, this is always `{"Name": "CloudWatchLogRule", "Version": 1}`. For
/// managed rules, this is `{"Name": "ServiceLogRule", "Version": 1}`" (`InsightRule$Schema`).
pub const INSIGHT_RULE_SCHEMA_NAME: &str = "CloudWatchLogRule";

/// The version both schema names are at — the one the member's documented value states. See
/// [`INSIGHT_RULE_SCHEMA_NAME`].
pub const INSIGHT_RULE_SCHEMA_VERSION: u64 = 1;

/// A rule's schema as `InsightRule$Schema` carries it: the schema's name and version, joined
/// by `/`.
///
/// `InsightRuleSchema` is a plain string, and the member's documented value
/// (`{"Name": …, "Version": …}`) is the schema that string NAMES rather than the text of it —
/// so what goes on the wire is the compact pair, which is what lets a client tell a rule it
/// wrote from a rule the service manages by reading the name before the `/`.
#[must_use]
pub fn insight_rule_schema(schema_name: &str) -> String {
    format!("{schema_name}/{INSIGHT_RULE_SCHEMA_VERSION}")
}

/// The same schema as the JSON object a rule DEFINITION states it with
/// (`InsightRule$Definition`, "the definition of the rule, as a JSON object").
///
/// Built from the same name and version the wire form is ([`insight_rule_schema`]), so the
/// schema a definition declares and the one the rule reports cannot drift apart.
#[must_use]
pub fn insight_rule_schema_document(schema_name: &str) -> ::serde_json::Value {
    ::serde_json::json!({"Name": schema_name, "Version": INSIGHT_RULE_SCHEMA_VERSION})
}

/// A Contributor Insights rule that is evaluating log events: "The state of the rule. Valid
/// values are ENABLED and DISABLED." (`PutInsightRuleInput$RuleState`)
pub const INSIGHT_RULE_ENABLED: &str = "ENABLED";

/// A Contributor Insights rule that is not evaluating log events
/// (`PutInsightRuleInput$RuleState`).
pub const INSIGHT_RULE_DISABLED: &str = "DISABLED";

/// The `ExceptionType` a batch Contributor Insights rule operation reports a well-formed name
/// it holds no rule for under: "The type of error." (`PartialFailure$ExceptionType`) — the
/// name addresses a rule that is not there, which is the resource-not-found class and not a
/// fault in the value itself, reported per entry rather than as a refusal of the whole
/// request.
const INSIGHT_RULE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// The `FailureCode` beside it: "The code of the error." (`PartialFailure$FailureCode`) —
/// which member of the entry the failure is about, here the rule name the request carried.
const INSIGHT_RULE_NAME_FAILURE_CODE: &str = "RULE_NAME";

/// How CloudWatch states that one of the names it was given is not a rule this store holds:
/// fixed text around the name the request carried, ended as a sentence.
///
/// One wording for every operation that answers for a name no rule answers to — the batch
/// operations report it as a `Failures` entry's description and `GetInsightRuleReport` as its
/// `ResourceNotFoundException` message, so the two cannot drift apart.
pub fn unknown_insight_rule(rule_name: &str) -> String {
    format!("Rule {rule_name} does not exist.")
}

/// The one `Failures` entry a batch Contributor Insights rule operation reports for a name
/// this store holds no rule for: "An array listing the rules that could not be deleted."
/// (`DeleteInsightRulesOutput$Failures`), worded and coded identically whichever of those
/// operations was called, so the entry is built here once.
fn unknown_insight_rule_failure(rule_name: &str) -> PartialFailure {
    PartialFailure {
        failure_resource: Some(rule_name.to_string()),
        exception_type: Some(INSIGHT_RULE_NOT_FOUND_EXCEPTION.to_string()),
        failure_code: Some(INSIGHT_RULE_NAME_FAILURE_CODE.to_string()),
        failure_description: Some(unknown_insight_rule(rule_name)),
    }
}

/// How CloudWatch states that a request named no Contributor Insights rule at all.
///
/// "An array of the rule names to delete." (`DeleteInsightRulesInput$RuleNames`, required) — an
/// array carrying no entry names nothing to act on, which this service refuses as a bad value
/// for `RuleNames` rather than as a change it applied to nothing.
pub const EMPTY_RULE_NAMES: &str = "EMPTY_RULE_NAMES: RuleNames cannot be an empty array.";

/// The granularity a Contributor Insights rule's contributor data is aggregated at, and so the
/// step every `Period` a report may be asked at is a multiple of: one minute.
///
/// A rule folds the log events its groups receive into per-minute contributor data, so the
/// `Period` a report's datapoints are aggregated over ("The period, in seconds, to use for the
/// statistics in the InsightRuleMetricDatapoint results.",
/// `GetInsightRuleReportInput$Period`) is a whole number of minutes: there is no finer fold to
/// report from.
pub const INSIGHT_RULE_PERIOD_STEP_SECS: i32 = 60;

/// The widest `Period` a Contributor Insights report may be asked at: one day, the span
/// Contributor Insights keeps a rule's contributor data over. See
/// [`INSIGHT_RULE_PERIOD_STEP_SECS`].
pub const MAX_INSIGHT_RULE_PERIOD_SECS: i32 = 86_400;

/// Why `period` is not a granularity a Contributor Insights report can be aggregated at, or
/// `None` when it is.
///
/// The `Period` member is modelled only as `range 1..`, so the granularities Contributor
/// Insights actually folds its contributor data at are held here: a multiple of
/// [`INSIGHT_RULE_PERIOD_STEP_SECS`], from that step up to
/// [`MAX_INSIGHT_RULE_PERIOD_SECS`]. The refusal names the condition by its CODE and states
/// the whole accepted set, the way this service's other Contributor Insights refusals do
/// ([`EMPTY_RULE_NAMES`]), and carries nothing of the value that was asked for.
#[must_use]
pub fn insight_rule_period_error(period: i32) -> Option<String> {
    let aggregatable = (INSIGHT_RULE_PERIOD_STEP_SECS..=MAX_INSIGHT_RULE_PERIOD_SECS)
        .contains(&period)
        && period % INSIGHT_RULE_PERIOD_STEP_SECS == 0;
    (!aggregatable).then(|| {
        format!(
            "INVALID_PERIOD: A Period must be a multiple of {INSIGHT_RULE_PERIOD_STEP_SECS} \
             seconds, from {INSIGHT_RULE_PERIOD_STEP_SECS} to {MAX_INSIGHT_RULE_PERIOD_SECS}."
        )
    })
}

/// How CloudWatch states the same thing for the operations that MOVE a rule's state
/// (`EnableInsightRules`, `DisableInsightRules`).
///
/// Those operations read their `RuleNames` as the set of rule states to write — "The state of
/// the rule. Valid values are ENABLED and DISABLED." (`PutInsightRuleInput$RuleState`) — so an
/// empty array is refused as the empty set of states it asks for, under the same
/// `InvalidParameterValueException`. Shared so neither of them can word it differently from
/// the other.
pub const EMPTY_RULE_STATES: &str = "EMPTY_RULE_STATES: RuleStates cannot be an empty array.";

/// The schema NAME a managed Contributor Insights rule is written against: "For managed rules,
/// this is `{"Name": "ServiceLogRule", "Version": 1}`" (`InsightRule$Schema`) — the schema of a
/// rule `PutManagedInsightRules` enabled, which is why `PutInsightRule` refuses a definition
/// carrying it ("You cannot edit these rules with PutInsightRule",
/// `PutManagedInsightRules`).
pub const MANAGED_INSIGHT_RULE_SCHEMA_NAME: &str = "ServiceLogRule";

/// The name the managed Contributor Insights rule enabled from `template_name` for
/// `resource_arn` takes.
///
/// Derived from the pair rather than minted per call, so enabling the same template for the
/// same resource twice names the SAME rule — which is what lets a second call re-enable the
/// rule the first one created: "If a previously created managed rule is currently disabled, a
/// subsequent call to this API will re-enable it." (`PutManagedInsightRules`).
///
/// The resource's own field of the ARN identifies it within the account and Region the request
/// is answered in, so that field is what the name carries beside the template. The result is
/// held inside `InsightRuleName`'s modelled 1..128 length and `^[\x20-\x7E]+$` pattern: a
/// character the pattern does not allow stands in as `-`, and an over-long name is cut to the
/// maximum.
pub fn managed_insight_rule_name(template_name: &str, resource_arn: &str) -> String {
    let resource = parse_resource_arn(resource_arn).map_or(resource_arn, |arn| arn.resource);
    let name: String = format!("{template_name}-{resource}")
        .chars()
        .map(|c| if c.is_ascii_graphic() || c == ' ' { c } else { '-' })
        .take(MAX_INSIGHT_RULE_NAME_LEN)
        .collect();
    name
}

/// `InsightRuleName`'s modelled maximum length, which a derived rule name is held inside.
const MAX_INSIGHT_RULE_NAME_LEN: usize = 128;

/// The managed-rule template `stored` was enabled from, or the empty string for a rule a
/// caller created with `PutInsightRule` — which orders the unmanaged rules before every
/// managed one, and is never reported.
pub fn template_name_of(stored: &StoredInsightRule) -> &str {
    stored
        .managed
        .as_ref()
        .map_or("", |origin| origin.template_name.as_str())
}

/// `value`'s ARN fields, or `None` when `value` is not a well-formed Amazon Resource Name.
///
/// A member documented as "The ARN of an Amazon Web Services resource"
/// (`ManagedRule$ResourceARN`, `ListManagedInsightRulesInput$ResourceARN`) is modelled only as
/// a 1..1024 string, so what makes it an ARN is read here — through the one shared parse, the
/// same one the alarm actions are read with.
pub fn parse_resource_arn(value: &str) -> Option<::service_common::arn::Arn<'_>> {
    ::service_common::arn::parse(value)
}

/// How CloudWatch states that a `ResourceARN` is not the ARN of an Amazon Web Services
/// resource, or `None` when it is one.
///
/// The sentence is the service's own for this error class and names no value: the caller
/// renders it through whichever parameter fault its own model declares.
pub fn resource_arn_error(value: &str) -> Option<String> {
    parse_resource_arn(value)
        .is_none()
        .then(|| INVALID_RESOURCE_ARN.to_string())
}

/// CloudWatch's wording for a `ResourceARN` that is not an ARN.
pub const INVALID_RESOURCE_ARN: &str = "INVALID_RESOURCE_ARN: Invalid ARN.";

/// The resource types this service's OWN ARN namespace addresses, each as the token an ARN of
/// such a resource carries before the resource's own name
/// (`::service_common::arn::Arn::resource_type`).
///
/// "The ARN format of an alarm is arn:aws:cloudwatch:Region:account-id:alarm:alarm-name The
/// ARN format of a Contributor Insights rule is
/// arn:aws:cloudwatch:Region:account-id:insight-rule/insight-rule-name The ARN format of a
/// dashboard is arn:aws:cloudwatch::account-id:dashboard/dashboard-name The ARN format of a
/// metric stream is arn:aws:cloudwatch:Region:account-id:metric-stream/metric-stream-name"
/// (`TagResourceInput$ResourceARN`); a dataset is addressed "in the format
/// arn:aws:cloudwatch:Region:account-id:dataset/dataset-id" (`GetDatasetInput$DatasetIdentifier`)
/// and an alarm mute rule as `arn:aws:cloudwatch:[REGION]:123456789012:alarm-mute-rule:*`
/// (`PutAlarmMuteRule`).
const ARN_RESOURCE_TYPES: [&str; 6] = [
    "alarm",
    "alarm-mute-rule",
    "dashboard",
    "dataset",
    "insight-rule",
    "metric-stream",
];

/// Whether `value` addresses an Amazon Web Services resource, as against being well formed as
/// an ARN while addressing nothing.
///
/// An ARN's resource field opens with the resource TYPE, and only the service that owns the
/// ARN's vendor field defines which types it has — so a value in THIS service's namespace is
/// held to the resource types this service addresses ([`ARN_RESOURCE_TYPES`]), each carrying a
/// non-empty id of a resource of that type (the same thing [`parse_taggable_arn`] requires of
/// the ARNs it reads), while a value in another vendor's namespace is that vendor's to judge:
/// this service cannot enumerate another's resources, and treating them as unaddressed would
/// reject the resources of the very services whose data a managed rule collects.
pub fn addresses_resource(value: &str) -> bool {
    parse_resource_arn(value).is_some_and(|arn| {
        arn.vendor != ARN_NAMESPACE
            || ARN_RESOURCE_TYPES.iter().any(|resource_type| {
                ::service_common::arn::resource_id_of_type(value, resource_type)
                    .is_some_and(|id| !id.is_empty())
            })
    })
}

/// The largest number of Contributor Insights rules this in-memory store holds.
///
/// A capacity bound, not an AWS quota: the rule records live in this process's memory, so
/// their number has to be finite. `PutInsightRule` answers its modelled `LimitExceededException`
/// ("The operation exceeded one or more limits.") rather than growing without limit once it is
/// reached; updating a rule that already exists adds none, so it is unaffected.
pub const MAX_INSIGHT_RULES: usize = 10_000;

/// How many tags one CloudWatch resource may carry: "You can associate as many as 50 tags
/// with a rule." (`PutInsightRuleInput$Tags`) — the same 50 every taggable resource of this
/// service takes ("You can associate as many as 50 tags with a metric stream.",
/// `PutMetricStreamInput$Tags`), which is why the bound lives here rather than per operation.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// Why `tags` is more tags than one `resource` may carry, or `None` when it is not. The
/// caller renders the sentence through whichever parameter fault its own model declares.
pub fn tag_count_error(resource: &str, tags: Option<&[Tag]>) -> Option<String> {
    let count = tags.map_or(0, <[Tag]>::len);
    (count > MAX_TAGS_PER_RESOURCE).then(|| {
        format!(
            "a {resource} can be associated with at most {MAX_TAGS_PER_RESOURCE} tags; \
             got {count}"
        )
    })
}

/// The kind of CloudWatch resource an `AmazonResourceName` names, for the operations that
/// read and write a resource's tags: "Currently, the only CloudWatch resources that can be
/// tagged are alarms, dashboards, metric streams and Contributor Insights rules."
/// (`TagResource`)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggableKind {
    /// An alarm — metric, composite or log — whose ARN's resource part is `alarm:<name>`.
    Alarm,
    /// A dashboard, whose ARN's resource part is `dashboard/<name>`.
    Dashboard,
    /// A metric stream, whose ARN's resource part is `metric-stream/<name>`.
    MetricStream,
    /// A Contributor Insights rule, whose ARN's resource part is `insight-rule/<name>`.
    InsightRule,
}

impl TaggableKind {
    /// The `ResourceType` a `ResourceNotFoundException` about a resource of this kind
    /// reports.
    pub fn resource_type(self) -> &'static str {
        match self {
            Self::Alarm => "Alarm",
            Self::Dashboard => "Dashboard",
            Self::MetricStream => "MetricStream",
            Self::InsightRule => "InsightRule",
        }
    }
}

/// The taggable resource one ARN names: its kind, the name the store holding that kind
/// keys on, and the Region the ARN scopes it to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaggableResource<'a> {
    /// Which of this service's resources the ARN names.
    pub kind: TaggableKind,
    /// The resource's name, as the ARN's resource part carries it.
    pub name: &'a str,
    /// The Region field of the ARN, EMPTY for a resource whose ARN carries none (a dashboard
    /// is global). It is part of the identity only of the kinds this store keys by Region
    /// ([`MetricStreamKey`]).
    pub region: &'a str,
}

/// The resource-part prefix each taggable kind's ARN carries: "The ARN format of an alarm is
/// arn:aws:cloudwatch:Region:account-id:alarm:alarm-name The ARN format of a Contributor
/// Insights rule is arn:aws:cloudwatch:Region:account-id:insight-rule/insight-rule-name The
/// ARN format of a dashboard is arn:aws:cloudwatch::account-id:dashboard/dashboard-name The
/// ARN format of a metric stream is
/// arn:aws:cloudwatch:Region:account-id:metric-stream/metric-stream-name"
/// (`TagResourceInput$ResourceARN`).
const TAGGABLE_ARN_PREFIXES: [(&str, TaggableKind); 4] = [
    ("alarm:", TaggableKind::Alarm),
    ("dashboard/", TaggableKind::Dashboard),
    ("insight-rule/", TaggableKind::InsightRule),
    ("metric-stream/", TaggableKind::MetricStream),
];

/// The ARN namespace of this service's resources, the vendor field every ARN it tags
/// carries (`aws.api#service$arnNamespace`).
const ARN_NAMESPACE: &str = "cloudwatch";

/// The CloudWatch resource `arn` names, or `None` when `arn` is not the ARN of a resource
/// this service can tag.
///
/// The ARN itself is read by the one shared parse, so this service cannot disagree with
/// another about what an ARN is; only the resource-part prefixes above are this service's.
/// The resource is resolved from its KIND, its NAME and — for the kinds this store keys by
/// Region — the ARN's own Region field; the partition and account fields are not part of that
/// key.
pub fn parse_taggable_arn(arn: &str) -> Option<TaggableResource<'_>> {
    let parsed = ::service_common::arn::parse_of(arn, ARN_NAMESPACE)?;
    let resource = parsed.resource;
    TAGGABLE_ARN_PREFIXES.iter().find_map(|(prefix, kind)| {
        resource
            .strip_prefix(prefix)
            .filter(|name| !name.is_empty())
            .map(|name| TaggableResource {
                kind: *kind,
                name,
                region: parsed.region,
            })
    })
}

/// The CloudWatch resource `arn` names, or the sentence saying why it names none. Every
/// operation that takes a `ResourceARN` resolves it through this one rule and renders the
/// sentence through whichever parameter fault its own model declares.
pub fn taggable_resource(arn: &str) -> Result<TaggableResource<'_>, String> {
    parse_taggable_arn(arn).ok_or_else(|| {
        format!(
            "The value {arn} for parameter ResourceARN is not valid: it is not the ARN of a \
             CloudWatch alarm, dashboard, metric stream or Contributor Insights rule"
        )
    })
}

/// The `ResourceNotFoundException` body ("The named resource does not exist") an operation
/// taking a `ResourceARN` refuses an ARN this store holds no resource for with.
///
/// The resource is identified by the error's own `ResourceType` and `ResourceId` members, and
/// the refusal's `Message` is EMPTY: this error class is answered by its code alone, so there
/// is no text to reproduce and inventing a sentence would put text on the wire the service
/// never sends. The member is present and empty, as the recorded refusal of this class
/// carries it — `FaultDescription` bounds no length, so an empty value is one it can hold.
/// Every operation that resolves a `ResourceARN` builds the body here, so they cannot
/// disagree about it.
pub fn unknown_taggable_resource(
    arn: &str,
    resource: TaggableResource<'_>,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        resource_type: Some(resource.kind.resource_type().to_string()),
        resource_id: Some(arn.to_string()),
        message: Some(String::new()),
    }
}

/// Apply one request's `tags` to a resource's tag set.
///
/// "If you specify a new tag key for the alarm, this tag is appended to the list of tags
/// associated with the alarm. If you specify a tag key that is already associated with the
/// alarm, the new tag value that you specify replaces the previous value for that tag."
/// (`TagResource`) — so a key already held keeps its position and takes the new value, and
/// a key not held is appended. A request naming one key twice leaves its last value, by the
/// same rule applied in order.
pub fn merge_tags(held: &mut Vec<Tag>, tags: &[Tag]) {
    for tag in tags {
        match held.iter_mut().find(|existing| existing.key == tag.key) {
            Some(existing) => existing.value = tag.value.clone(),
            None => held.push(tag.clone()),
        }
    }
}

/// Remove from a resource's tag set every tag whose key `keys` names, leaving the rest in
/// the order they were added. A key the resource does not carry removes nothing.
pub fn remove_tags(held: &mut Vec<Tag>, keys: &[String]) {
    held.retain(|tag| !keys.iter().any(|key| key == &tag.key));
}

/// The largest number of metric streams this in-memory store holds.
///
/// A capacity bound, not an AWS quota: the stream records live in this process's memory, so
/// their number has to be finite. `PutMetricStream` answers its modelled server fault rather
/// than growing without limit once it is reached; updating a stream that already exists adds
/// none, so it is unaffected.
pub const MAX_METRIC_STREAMS: usize = 10_000;

/// The largest number of dataset KMS-key associations this in-memory store holds.
///
/// A capacity bound, not an AWS quota: the default dataset is "implicit for every account in
/// every Region", so this map holds one entry per REGION a caller has associated a key in, and
/// the Region a request names is the caller's own. The records live in this process's memory,
/// so their number has to be finite: `AssociateDatasetKmsKey` answers its server fault rather
/// than growing without limit once the bound is reached, and replacing the key of a Region that
/// already carries one adds no entry.
pub const MAX_DATASET_KMS_KEYS: usize = 1_000;

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

/// The most rule NODES one call may evaluate, across every composite alarm and every pass.
///
/// The pass bound alone does not bound the work: without this, one call's evaluation grows
/// with the composite alarms the store holds ([`MAX_ALARMS`]) times the nodes each rule may
/// carry, which a caller controls. This is the ceiling on that product, chosen three orders
/// of magnitude above what a realistic store needs (a thousand composite alarms whose rules
/// name a handful of children each settle in a few thousand nodes per pass). An evaluation
/// the budget stops stays DUE, so the next call carries it and the outcome is the same — only
/// later.
const MAX_COMPOSITE_EVALUATION_NODES: usize = 1_000_000;

/// `OK`, one of the three modelled `StateValue` values (`STATE_VALUE_VALUES`).
pub const ALARM_STATE_OK: &str = "OK";

/// `ALARM`, one of the three modelled `StateValue` values (`STATE_VALUE_VALUES`).
pub const ALARM_STATE_ALARM: &str = "ALARM";

/// `INSUFFICIENT_DATA`, one of the three modelled `StateValue` values
/// (`STATE_VALUE_VALUES`).
pub const ALARM_STATE_INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// The `StateReason` an alarm NOTHING has evaluated yet reports: "When this operation creates
/// an alarm, the alarm state is immediately set to INSUFFICIENT_DATA. The alarm is then
/// evaluated and its state is set appropriately." (`PutMetricAlarm`) — the service's own
/// constant for that un-evaluated seed state, recorded verbatim, so every alarm writer of
/// every kind seeds the state the same way and no evaluation has to word it.
pub const INITIAL_STATE_REASON: &str = "Unchecked: Initial alarm creation";

/// `MetricAlarm`, one of the modelled `AlarmType` values (`ALARM_TYPE_VALUES`): the kind of
/// alarm `PutMetricAlarm` creates, which is also the kind `DescribeAlarms` and
/// `DescribeAlarmHistory` report when a request names no `AlarmTypes`.
pub const ALARM_TYPE_METRIC: &str = "MetricAlarm";

/// `CompositeAlarm`, one of the modelled `AlarmType` values (`ALARM_TYPE_VALUES`): the kind
/// of alarm `PutCompositeAlarm` creates.
pub const ALARM_TYPE_COMPOSITE: &str = "CompositeAlarm";

/// `LogAlarm`, one of the modelled `AlarmType` values (`ALARM_TYPE_VALUES`): the kind of
/// alarm `PutLogAlarm` creates.
pub const ALARM_TYPE_LOG: &str = "LogAlarm";

/// The values `TreatMissingData` accepts: "Valid values are breaching, notBreaching, ignore,
/// and missing." (`smithy_query(query="doc",shape="PutLogAlarmInput",member="TreatMissingData")`)
/// — the member is a plain string in the model, so the value list is enforced here.
pub const TREAT_MISSING_DATA_VALUES: [&str; 4] = ["breaching", "notBreaching", "ignore", "missing"];

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
    /// The tags the resource `resource` names carries, or `None` when this store holds no
    /// such resource.
    ///
    /// There is ONE tag set per resource: the tags live on the stored resource its writer
    /// created (`PutMetricAlarm`, `PutCompositeAlarm`, `PutLogAlarm`, `PutMetricStream`,
    /// `PutInsightRule`, `PutDashboard`), so a tag written at create time and one written
    /// afterwards are the same set, and deleting the resource takes its tags with it.
    pub fn resource_tags(&self, resource: &TaggableResource<'_>) -> Option<&[Tag]> {
        match resource.kind {
            // An alarm ARN does not say which KIND of alarm it names, and one name belongs
            // to at most one of the three stores, so all three answer for it.
            TaggableKind::Alarm => self
                .alarms
                .get(resource.name)
                .map(|stored| stored.tags.as_slice())
                .or_else(|| {
                    self.composite_alarms
                        .get(resource.name)
                        .map(|stored| stored.tags.as_slice())
                })
                .or_else(|| {
                    self.log_alarms
                        .get(resource.name)
                        .map(|stored| stored.tags.as_slice())
                }),
            TaggableKind::MetricStream => self
                .metric_streams
                .get(&metric_stream_key(resource.region, resource.name))
                .map(|stored| stored.tags.as_slice()),
            TaggableKind::InsightRule => self
                .insight_rules
                .get(resource.name)
                .map(|stored| stored.tags.as_slice()),
            // A dashboard ARN carries no Region (dashboards are global), which the shared
            // ARN parse and this store both already treat as part of the ARN rather than of
            // the key: the name is what `PutDashboard` keyed the dashboard on.
            TaggableKind::Dashboard => self
                .dashboards
                .get(resource.name)
                .map(|stored| stored.tags.as_slice()),
        }
    }

    /// The tag set of the resource `resource` names, to write through, or `None` when this
    /// store holds no such resource. The mutable half of [`Self::resource_tags`].
    pub fn resource_tags_mut(&mut self, resource: &TaggableResource<'_>) -> Option<&mut Vec<Tag>> {
        match resource.kind {
            TaggableKind::Alarm => {
                if let Some(stored) = self.alarms.get_mut(resource.name) {
                    return Some(&mut stored.tags);
                }
                if let Some(stored) = self.composite_alarms.get_mut(resource.name) {
                    return Some(&mut stored.tags);
                }
                self.log_alarms
                    .get_mut(resource.name)
                    .map(|stored| &mut stored.tags)
            }
            TaggableKind::MetricStream => self
                .metric_streams
                .get_mut(&metric_stream_key(resource.region, resource.name))
                .map(|stored| &mut stored.tags),
            TaggableKind::InsightRule => self
                .insight_rules
                .get_mut(resource.name)
                .map(|stored| &mut stored.tags),
            TaggableKind::Dashboard => self
                .dashboards
                .get_mut(resource.name)
                .map(|stored| &mut stored.tags),
        }
    }

    /// Move the metric stream `name` of `region` into `state`, or do nothing when this store
    /// holds no such stream.
    ///
    /// "Starts the streaming of metrics for one or more of your metric streams."
    /// (`StartMetricStreams`) / "Stops the streaming of metrics for one or more of your metric
    /// streams." (`StopMetricStreams`) — a transition is a change to the stream, so it stamps
    /// `LastUpdateDate` ("The date that the configuration of this metric stream was most
    /// recently updated.", `GetMetricStreamOutput$LastUpdateDate`): an audit trail reading that
    /// member sees when the stream was last started or stopped. Asking for the state a stream
    /// is already in changes nothing, so it stamps nothing either.
    ///
    /// Both writers share it, so one rule decides what a transition leaves behind.
    pub fn set_metric_stream_state(
        &mut self,
        region: &str,
        name: &str,
        state: &str,
        now_secs: f64,
    ) {
        let Some(stored) = self.metric_streams.get_mut(&metric_stream_key(region, name)) else {
            return;
        };
        if stored.stream.state.as_deref() == Some(state) {
            return;
        }
        stored.stream.state = Some(state.to_string());
        stored.stream.last_update_date = Some(to_timestamp(now_secs));
    }

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

    /// Make every metric alarm watching one of `keys` DUE for evaluation at `now_secs`.
    ///
    /// An alarm's evaluation is owed from the arrival of the data it grades: the datapoints are
    /// what the alarm is evaluated FROM, so the write that stores a datapoint of a series an
    /// alarm watches is what makes that alarm's evaluation due — a span later
    /// ([`alarm_evaluation::EVALUATION_DELAY_SECS`], as for every transition the clock owes),
    /// so a caller reading straight after the write still sees the state the alarm had, and
    /// the clock the write starts materialises the transition and delivers the actions it fires
    /// without waiting for whatever call happens to read the alarm next. Nothing else is
    /// pulled forward: an alarm whose series this write did not touch keeps the instant it
    /// carries.
    ///
    /// A state `SetAlarmState` forced is NOT cut short by it: a force stands for its own span
    /// ([`alarm_evaluation::forced_state_span_secs`]) whatever the series holds, so the
    /// evaluation that takes it back is never brought forward of the instant the force stands
    /// until ([`StoredAlarm::forced_state_until_secs`]) — a forced state an arriving datapoint
    /// could reclaim at once would be a state nothing could read, neither the caller that
    /// forced it nor the composite alarm whose rule is evaluated over it.
    ///
    /// An alarm is matched on the datapoints it would GRADE: the series it names — namespace,
    /// metric name and dimension set, the identity its own evaluation reads, or the identities
    /// its `Metrics` queries name — and the unit each of them selects ("If you specify a unit,
    /// the operation returns only data that was collected with that unit specified"), so a
    /// datapoint of another series, or of a unit this alarm does not grade, makes it no more
    /// due than one nobody published.
    pub fn make_alarms_due(&mut self, written: &[WrittenSeries], now_secs: f64) {
        if written.is_empty() {
            return;
        }
        let due = now_secs + alarm_evaluation::EVALUATION_DELAY_SECS;
        for stored in self.alarms.values_mut() {
            let watched = alarm_evaluation::watched_series_set(&stored.alarm);
            let grades = written.iter().any(|series| {
                watched.iter().any(|(key, selector)| {
                    series.key == *key
                        && series.units.iter().any(|unit| {
                            alarm_evaluation::grades_unit(selector.as_deref(), unit)
                        })
                })
            });
            if grades {
                stored.evaluation_due_secs = stored
                    .evaluation_due_secs
                    .min(due)
                    .max(stored.forced_state_until_secs);
            }
        }
    }

    /// Make every COMPOSITE alarm whose rule reads the alarm `name` DUE for evaluation at
    /// `now_secs`.
    ///
    /// A composite alarm's state is its rule over the states of the alarms the rule names, so
    /// its evaluation is owed from the moment one of those states changes — a span later
    /// ([`COMPOSITE_EVALUATION_DELAY_SECS`], the same span its own write owes it), the same
    /// principle as [`Self::make_alarms_due`] for the datapoints a metric alarm reads. The
    /// clock the call that materialised the child's transition started is then what materialises
    /// the parent's and delivers the actions the parent's transition fires, rather than
    /// whatever call happens to read the parent next. A composite alarm no rule input touched
    /// keeps the instant it carries.
    pub fn make_parents_due(&mut self, name: &str, now_secs: f64) {
        let due = now_secs + COMPOSITE_EVALUATION_DELAY_SECS;
        for stored in self.composite_alarms.values_mut() {
            if stored.child_names.iter().any(|child| child == name) {
                stored.evaluation_due_secs = stored.evaluation_due_secs.min(due);
            }
        }
    }

    /// The kind of alarm `name` already names, as one of the modelled `AlarmType` values;
    /// `None` when no alarm of that name exists.
    ///
    /// "This name must be unique within the Amazon Web Services account and Region."
    /// (`PutLogAlarmInput$AlarmName`, and the same sentence on the other alarm writers) \u2014 one
    /// name names ONE alarm whatever kind it is, so every writer asks this before it stores an
    /// alarm and refuses a name another kind already holds.
    pub fn alarm_kind(&self, name: &str) -> Option<&'static str> {
        if self.alarms.contains_key(name) {
            Some(ALARM_TYPE_METRIC)
        } else if self.composite_alarms.contains_key(name) {
            Some(ALARM_TYPE_COMPOSITE)
        } else if self.log_alarms.contains_key(name) {
            Some(ALARM_TYPE_LOG)
        } else {
            None
        }
    }

    /// Turn the ACTIONS of every alarm `names` names on or off, whatever kind of alarm each
    /// one is, reporting back the names this store holds no alarm for.
    ///
    /// "Indicates whether actions should be executed during any changes to the alarm state"
    /// (`MetricAlarm$ActionsEnabled`) — the one member `EnableAlarmActions` ("Enables the
    /// actions for the specified alarms.") and `DisableAlarmActions` ("Disables the actions
    /// for the specified alarms. When an alarm's actions are disabled, the alarm actions do
    /// not execute when the alarm state changes.") write, so both operations share this one
    /// writer and neither can drift from the other. Every kind of alarm carries the member,
    /// so a name of any kind is written.
    ///
    /// A write that CHANGES the member is a configuration update: it stamps
    /// `AlarmConfigurationUpdatedTimestamp`, records the `ConfigurationUpdate` history item
    /// every other configuration write records, and — like every other configuration write —
    /// hands the alarm's state back to its own evaluation, which becomes due a span later ("the
    /// alarm is then evaluated and its state is set appropriately", `PutMetricAlarm`; "It is
    /// also reevaluated if you update its configuration", `SetAlarmState` on a composite
    /// alarm). So a state a caller forced does not outlive a reconfiguration of the alarm it
    /// was forced on. Asking for the setting an alarm already has changes nothing, so it
    /// records nothing and re-evaluates nothing either.
    pub fn set_actions_enabled(
        &mut self,
        names: &[String],
        enabled: bool,
        now_secs: f64,
    ) -> Vec<String> {
        // A name the request repeats names ONE alarm, so the deduplicated list is walked: the
        // second mention would otherwise find the member already written and report the alarm
        // as unchanged twice.
        let mut named: Vec<&str> = names.iter().map(String::as_str).collect();
        named.sort_unstable();
        named.dedup();

        let mut unknown: Vec<String> = Vec::new();
        // When a configuration write re-evaluates the alarm it touched: the same span after the
        // write as every other transition this store's clock owes, so a caller reading straight
        // back still sees the state the write found.
        let due = now_secs + alarm_evaluation::EVALUATION_DELAY_SECS;
        // The history items the writes owe, recorded once the store is no longer borrowed by
        // the alarm each one belongs to.
        let mut records: Vec<(String, &'static str, String)> = Vec::new();
        for name in named {
            if let Some(stored) = self.alarms.get_mut(name) {
                if stored.alarm.actions_enabled == Some(enabled) {
                    continue;
                }
                let previous = stored.alarm.clone();
                stored.alarm.actions_enabled = Some(enabled);
                stored.alarm.alarm_configuration_updated_timestamp = Some(to_timestamp(now_secs));
                stored.evaluation_due_secs = stored.evaluation_due_secs.min(due);
                let data = configuration_update_history_data(Some(&previous), &stored.alarm);
                records.push((name.to_string(), ALARM_TYPE_METRIC, data));
            } else if let Some(stored) = self.composite_alarms.get_mut(name) {
                if stored.alarm.actions_enabled == Some(enabled) {
                    continue;
                }
                let previous = stored.alarm.clone();
                stored.alarm.actions_enabled = Some(enabled);
                stored.alarm.alarm_configuration_updated_timestamp = Some(to_timestamp(now_secs));
                stored.evaluation_due_secs = stored.evaluation_due_secs.min(due);
                // "It is also reevaluated if you update its configuration" (`SetAlarmState`) —
                // so the rule, not the force, owns this alarm's state from that evaluation on.
                stored.forced_child_states = None;
                let data =
                    composite_configuration_update_history_data(Some(&previous), &stored.alarm);
                records.push((name.to_string(), ALARM_TYPE_COMPOSITE, data));
            } else if let Some(stored) = self.log_alarms.get_mut(name) {
                if stored.alarm.actions_enabled == Some(enabled) {
                    continue;
                }
                let previous = stored.alarm.clone();
                stored.alarm.actions_enabled = Some(enabled);
                stored.alarm.alarm_configuration_updated_timestamp = Some(to_timestamp(now_secs));
                let data = log_configuration_update_history_data(Some(&previous), &stored.alarm);
                records.push((name.to_string(), ALARM_TYPE_LOG, data));
            } else {
                unknown.push(name.to_string());
            }
        }
        for (name, kind, data) in records {
            self.record_history(
                &name,
                now_secs,
                AlarmHistoryItem {
                    alarm_name: Some(name.clone()),
                    alarm_contributor_id: None,
                    alarm_type: Some(kind.to_string()),
                    timestamp: Some(to_timestamp(now_secs)),
                    history_item_type: Some("ConfigurationUpdate".to_string()),
                    history_summary: Some("Alarm updated from configuration".to_string()),
                    history_data: Some(data),
                    alarm_contributor_attributes: None,
                },
            );
        }
        unknown
    }

    /// How many alarms this store holds, counting every kind.
    ///
    /// "The quota for alarms for this customer has already been reached."
    /// (`LimitExceededFault`) \u2014 CloudWatch's alarm quota is per Region per account over every
    /// kind of alarm, so the count every writer checks against [`MAX_ALARMS`] is this one.
    pub fn alarm_count(&self) -> usize {
        self.alarms.len() + self.composite_alarms.len() + self.log_alarms.len()
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

    /// Each alarm one composite-alarm rule references, in the order the rule names them: which
    /// alarm it is, the state it is in and when it moved there — everything the parent's
    /// explanation of its own state, and the notification its transition publishes, report
    /// about its children.
    ///
    /// Takes the resolved child names ([`StoredCompositeAlarm::child_names`]) rather than the
    /// rule, so a caller on a hot path never re-walks the rule to obtain them.
    ///
    /// The stores consulted are the ones the rule itself reads
    /// ([`Self::alarm_state_value`]), so a child reported here is always an alarm the rule
    /// evaluated over. A name the rule references and no alarm answers to is reported with
    /// nothing filled in: the rule read no state for it.
    pub fn child_alarm_states(&self, names: &[String]) -> Vec<ChildAlarmState> {
        names
            .iter()
            .map(|reference| {
                let name = alarm_rule_reference_name(reference);
                if let Some(stored) = self.alarms.get(name) {
                    return ChildAlarmState::of(
                        name,
                        stored.alarm.alarm_arn.as_deref(),
                        stored.alarm.state_value.as_deref(),
                        stored.alarm.state_transitioned_timestamp.as_ref(),
                    );
                }
                match self.composite_alarms.get(name) {
                    Some(stored) => ChildAlarmState::of(
                        name,
                        stored.alarm.alarm_arn.as_deref(),
                        stored.alarm.state_value.as_deref(),
                        stored.alarm.state_transitioned_timestamp.as_ref(),
                    ),
                    None => ChildAlarmState::of(name, None, None, None),
                }
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
            .map(|stored| stored.child_names.clone())
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
                    && stored.child_names.iter().any(|child| child == name)
            })
            .map(|(parent, _)| parent.clone())
            .collect();
        parents.sort();
        parents
    }

    /// The composite alarms whose CONFIGURATION references `name`: the ones whose `AlarmRule`
    /// names it ([`Self::composite_alarm_parents`]) and the ones that use it as their
    /// `ActionsSuppressor` ("The actions suppressor alarm is specified by its ARN or name" —
    /// `CompositeAlarm$ActionsSuppressor`, so either form resolves to the name the store keys
    /// on). The alarm itself is never among them, and the list is ordered and without repeats.
    ///
    /// This is every reference that would dangle if the alarm named went away.
    pub fn composite_alarm_referrers(&self, name: &str) -> Vec<String> {
        let mut referrers = self.composite_alarm_parents(name);
        referrers.extend(
            self.composite_alarms
                .iter()
                .filter(|(parent, stored)| {
                    parent.as_str() != name
                        && stored.alarm.actions_suppressor.as_deref().is_some_and(
                            |suppressor| alarm_rule_reference_name(suppressor) == name,
                        )
                })
                .map(|(parent, _)| parent.clone()),
        );
        referrers.sort();
        referrers.dedup();
        referrers
    }

    /// The managed Contributor Insights rules this store holds for `resource_arn`, as
    /// (rule name, rule) pairs ordered by template name and then rule name.
    ///
    /// "The managed rules that are available for the specified Amazon Web Services resource."
    /// (`ListManagedInsightRulesOutput$ManagedRules`) — a managed rule belongs to the resource
    /// its `PutManagedInsightRules` call named, so the resource is what selects them, and the
    /// order is total so one page of a listing follows the previous one.
    pub fn managed_insight_rules_for(
        &self,
        resource_arn: &str,
    ) -> Vec<(&String, &StoredInsightRule)> {
        let mut rules: Vec<(&String, &StoredInsightRule)> = self
            .insight_rules
            .iter()
            .filter(|(_, stored)| {
                stored
                    .managed
                    .as_ref()
                    .is_some_and(|origin| origin.resource_arn == resource_arn)
            })
            .collect();
        rules.sort_unstable_by(|(left_name, left), (right_name, right)| {
            template_name_of(left)
                .cmp(template_name_of(right))
                .then_with(|| left_name.cmp(right_name))
        });
        rules
    }

    /// Publish `template` into this account's managed-rule catalog, replacing a template
    /// already published under the same name for the same resources.
    ///
    /// The seam the SERVICE side of the catalog is written through: "The template name for the
    /// managed Contributor Insights rule, as returned by `ListManagedInsightRules`."
    /// (`ManagedRule$TemplateName`) — a caller names a published template, so nothing a
    /// `PutManagedInsightRules` request carries reaches this.
    pub fn publish_managed_rule_template(&mut self, template: ManagedRuleTemplate) {
        match self.managed_rule_templates.iter_mut().find(|held| {
            held.template_name == template.template_name
                && held.resource_vendor == template.resource_vendor
                && held.resource_type == template.resource_type
        }) {
            Some(held) => *held = template,
            None => self.managed_rule_templates.push(template),
        }
    }

    /// The published template `template_name` names for `resource_arn`, or `None` when the
    /// catalog publishes no template of that name for that resource.
    ///
    /// Exact match on the name: a template is named as it is published, so a name that differs
    /// in case or punctuation names a different template — and, when none is published under
    /// it, no template at all.
    pub fn managed_rule_template(
        &self,
        template_name: &str,
        resource_arn: &str,
    ) -> Option<&ManagedRuleTemplate> {
        self.managed_rule_templates.iter().find(|template| {
            template.template_name == template_name && template.available_for(resource_arn)
        })
    }

    /// Every managed rule available for `resource_arn`: ONE entry per template the catalog
    /// publishes for it or a rule is enabled from, carrying that rule when one is enabled,
    /// ordered by template name.
    ///
    /// "The managed rules that are available for the specified Amazon Web Services resource."
    /// (`ListManagedInsightRulesOutput$ManagedRules`) — available is what the catalog publishes
    /// for the resource, and whether a rule has been enabled from a template is what its
    /// `RuleState` says ("If present, it contains information about the Contributor Insights
    /// rule…", `ManagedRuleDescription$RuleState`). A rule whose template the catalog no longer
    /// publishes is still reported: the rule exists.
    ///
    /// One entry per template is what makes the template name identify the entry — which is
    /// what a listing's page token is, so it has to hold however the store got into its current
    /// shape. A resource holds at most one rule per template by construction (the rule's name
    /// is derived from the (template, resource) pair, so enabling the same template again names
    /// the same rule), and a second rule that reached the same template anyway is reported
    /// once, under the first of them in rule-name order.
    pub fn managed_rules_available_for(&self, resource_arn: &str) -> Vec<AvailableManagedRule<'_>> {
        let enabled = self.managed_insight_rules_for(resource_arn);
        let mut available: Vec<AvailableManagedRule<'_>> = Vec::new();
        for (name, stored) in &enabled {
            let template_name = template_name_of(stored);
            if available
                .iter()
                .any(|reported| reported.template_name == template_name)
            {
                continue;
            }
            available.push(AvailableManagedRule {
                template_name,
                rule: Some((*name, *stored)),
            });
        }
        for template in &self.managed_rule_templates {
            let name = template.template_name.as_str();
            if template.available_for(resource_arn)
                && !available
                    .iter()
                    .any(|reported| reported.template_name == name)
            {
                available.push(AvailableManagedRule {
                    template_name: name,
                    rule: None,
                });
            }
        }
        available.sort_by(|left, right| left.template_name.cmp(right.template_name));
        available
    }

    /// Move every Contributor Insights rule `rule_names` names to `rule_state`, reporting back
    /// one entry per name this store holds no rule for.
    ///
    /// "An array listing the rules that could not be disabled."
    /// (`DisableInsightRulesOutput$Failures`) — a name no rule answers to is reported as an
    /// entry of that array and the rest of the request still takes effect, rather than the
    /// whole call failing. "This array is empty if the API operation was successful for all
    /// the rules specified in the request." (`PartialFailure`)
    ///
    /// The state a rule moves to is the only thing that differs between the operations that
    /// enable and disable rules, so the walk lives here and neither can drift from the other.
    /// Naming a rule that already holds the state asks for no change and is not a failure: the
    /// operations say what a rule's state BECOMES.
    pub fn set_insight_rule_state(
        &mut self,
        rule_names: &[String],
        rule_state: &str,
    ) -> Vec<PartialFailure> {
        let mut failures: Vec<PartialFailure> = Vec::new();
        for rule_name in rule_names {
            match self.insight_rules.get_mut(rule_name) {
                Some(stored) => stored.rule.state = rule_state.to_string(),
                None => failures.push(unknown_insight_rule_failure(rule_name)),
            }
        }
        failures
    }

    /// Remove every Contributor Insights rule `rule_names` names, reporting back one entry per
    /// name this store holds no rule for.
    ///
    /// "Permanently deletes the specified Contributor Insights rules."
    /// (`DeleteInsightRules`) — the rule and everything stored with it (its definition, its
    /// state and its tags) go away, so a later read reports no such rule. "An array listing the
    /// rules that could not be deleted." (`DeleteInsightRulesOutput$Failures`) is reported the
    /// same way the state operations report theirs: per name, with the rest of the request
    /// still taking effect.
    ///
    /// The names a request carries are the SET of rules it deletes, so a name it repeats is
    /// deleted once and is not a failure the second time — the same way naming a rule twice
    /// asks the state operations for one state. A rule `PutManagedInsightRules` enabled is
    /// removed by the same walk: "The rules can be enabled, disabled, and deleted using
    /// EnableInsightRules, DisableInsightRules, and DeleteInsightRules."
    /// (`PutManagedInsightRules`).
    pub fn delete_insight_rules(&mut self, rule_names: &[String]) -> Vec<PartialFailure> {
        let mut deleted: HashSet<&String> = HashSet::new();
        let mut failures: Vec<PartialFailure> = Vec::new();
        for rule_name in rule_names {
            if self.insight_rules.remove(rule_name).is_some() {
                deleted.insert(rule_name);
            } else if !deleted.contains(rule_name) {
                failures.push(unknown_insight_rule_failure(rule_name));
            }
        }
        failures
    }

    /// Materialise the anomaly-detector training the clock has already made due.
    ///
    /// A detector's `StateValue` is not a value a caller writes: a model is created pending
    /// training and reports a trained state once its training is done, so the transition is
    /// the CLOCK's. It becomes due [`TRAINING_DELAY_SECS`] after the write that stored the
    /// model, and from then on the state depends only on what the store holds — so whichever
    /// call materialises it, every later caller sees the same thing.
    pub fn settle_anomaly_detector_training(&mut self, now_secs: f64) {
        let settled: Vec<(String, String)> = self
            .anomaly_detectors
            .iter()
            .filter(|(_, stored)| {
                stored.detector.state_value.as_deref() == Some(ANOMALY_DETECTOR_PENDING_TRAINING)
                    && stored.trained_due_secs <= now_secs
            })
            .map(|(id, stored)| (id.clone(), self.trained_state_of(&stored.detector)))
            .collect();
        for (id, state_value) in settled {
            if let Some(stored) = self.anomaly_detectors.get_mut(&id) {
                stored.detector.state_value = Some(state_value);
            }
        }
    }

    /// What a model reports once its training is done: `TRAINED` when the series it watches
    /// has published data for the model to learn from, and `TRAINED_INSUFFICIENT_DATA` when it
    /// has not — a model trained on no datapoint has nothing to expect a value by.
    fn trained_state_of(&self, detector: &AnomalyDetector) -> String {
        let trained_on_data = match (
            detector.single_metric_anomaly_detector.as_ref(),
            detector.metric_math_anomaly_detector.as_ref(),
        ) {
            (Some(single), _) => self.has_datapoints(
                single.namespace.as_deref().unwrap_or_default(),
                single.metric_name.as_deref().unwrap_or_default(),
                single.dimensions.as_deref(),
            ),
            // A metric-math model is trained on the series its expression returns, which is
            // computed from the metrics its queries retrieve: data in any of them is data the
            // expression has to train on.
            (None, Some(math)) => math
                .metric_data_queries
                .iter()
                .flatten()
                .filter_map(|query| query.metric_stat.as_ref())
                .any(|stat| {
                    self.has_datapoints(
                        stat.metric.namespace.as_deref().unwrap_or_default(),
                        stat.metric.metric_name.as_deref().unwrap_or_default(),
                        stat.metric.dimensions.as_deref(),
                    )
                }),
            (None, None) => false,
        };
        if trained_on_data {
            ANOMALY_DETECTOR_TRAINED.to_string()
        } else {
            ANOMALY_DETECTOR_TRAINED_INSUFFICIENT_DATA.to_string()
        }
    }

    /// Whether this store holds any datapoint for one metric, named the way a request names
    /// it: the dimension set is normalised, because the dimensions are part of the series'
    /// identity ([`MetricKey`]).
    fn has_datapoints(
        &self,
        namespace: &str,
        metric_name: &str,
        dimensions: Option<&[Dimension]>,
    ) -> bool {
        let key = MetricKey {
            namespace: namespace.to_string(),
            metric_name: metric_name.to_string(),
            dimensions: dimension_set(dimensions),
        };
        self.metrics
            .get(&key)
            .is_some_and(|data| !data.is_empty())
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
    pub fn evaluate_composite_alarms(
        &mut self,
        now_secs: f64,
        budget: &mut alarm_evaluation::EvaluationBudget,
    ) {
        // A composite alarm's rule can reference another composite alarm, so one alarm's new
        // state can change its parent's outcome too; one pass settles one level of that
        // chain. The number of passes is bounded by [`MAX_COMPOSITE_EVALUATION_PASSES`] as
        // well as by the number of composite alarms, and the NODES one call may evaluate are
        // bounded by [`MAX_COMPOSITE_EVALUATION_NODES`] — so the work ONE call can do is
        // bounded whatever the store holds, rather than growing with the alarms times the size
        // of their rules. An evaluation the budget stops is not answered wrongly, only
        // settled later: it stays due, and the next call carries it (the evaluation is
        // idempotent). The pass bound is also what stops a CYCLE of composite alarms: "the
        // evaluation of composite alarms stops if CloudWatch detects a cycle in the
        // evaluation path."
        // The composite alarms are a PASS of their own, owed its first evaluation whatever the
        // metric-alarm pass before it has already spent
        // ([`alarm_evaluation::EvaluationBudget::start_pass`]), so a store whose metric alarms
        // keep one request busy cannot leave its composite alarms in the state their last
        // write left behind.
        budget.start_pass();
        let passes = self
            .composite_alarms
            .len()
            .min(MAX_COMPOSITE_EVALUATION_PASSES);
        let mut nodes = MAX_COMPOSITE_EVALUATION_NODES;
        for _ in 0..passes {
            if budget.spent()
                || !self.evaluate_composite_alarms_once(now_secs, &mut nodes, budget)
                || nodes == 0
            {
                break;
            }
        }
    }

    /// One evaluation pass over every composite alarm, reporting whether any alarm's state
    /// value changed. Alarms are visited in the order their evaluations came due
    /// ([`alarm_evaluation::due_order`]), and the pass stops as soon as `nodes` or the
    /// request's own `budget` is spent.
    ///
    /// Only an alarm whose evaluation the clock has already made DUE is charged, so the one
    /// evaluation a pass is owed is spent on an alarm that has one to settle rather than on
    /// whichever name happens to come first.
    fn evaluate_composite_alarms_once(
        &mut self,
        now_secs: f64,
        nodes: &mut usize,
        budget: &mut alarm_evaluation::EvaluationBudget,
    ) -> bool {
        let ordered = alarm_evaluation::due_order(
            self.composite_alarms
                .iter()
                .map(|(name, stored)| (name.as_str(), stored.evaluation_due_secs)),
        );
        let mut changed = false;
        for name in ordered {
            if *nodes == 0 || budget.spent() {
                break;
            }
            if !self.composite_alarm_evaluation_due(&name, now_secs) {
                continue;
            }
            budget.charge_alarm();
            changed |= self.evaluate_composite_alarm(&name, now_secs, nodes);
            budget.settled_one();
        }
        changed
    }

    /// Whether the composite alarm `name` has an evaluation the clock has already made due.
    fn composite_alarm_evaluation_due(&self, name: &str, now_secs: f64) -> bool {
        self.composite_alarms
            .get(name)
            .is_some_and(|stored| now_secs >= stored.evaluation_due_secs)
    }

    /// Evaluate ONE composite alarm, reporting whether its state value changed.
    ///
    /// The alarm's rule nodes are charged to `budget`, and an alarm whose evaluation the
    /// remaining budget cannot pay for is left for a later call.
    fn evaluate_composite_alarm(
        &mut self,
        name: &str,
        now_secs: f64,
        budget: &mut usize,
    ) -> bool {
        // Everything this evaluation reads is read before anything is written.
        let Some(stored) = self.composite_alarms.get(name) else {
            return false;
        };
        if now_secs < stored.evaluation_due_secs {
            return false;
        }
        // A rule carries at least one node, so every evaluation costs something and the
        // budget always advances.
        let cost = stored.rule_nodes.max(1);
        if cost > *budget {
            return false;
        }
        *budget -= cost;
        let rule = &stored.rule;
        let previous = AlarmStateSnapshot::of_composite_alarm(&stored.alarm);
        let forced = stored.forced_child_states.clone();
        let suppressor = stored.alarm.actions_suppressor.clone();
        let child_states = self.child_alarm_states(&stored.child_names);
        // The pairs the forced-state snapshot is compared against: a state a caller forced
        // stands until a CHILD's state changes, which is what this comparison detects.
        let children: Vec<(String, Option<String>)> = child_states
            .iter()
            .map(|child| (child.name.clone(), child.state_value.clone()))
            .collect();
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
        stored.alarm.state_reason = Some(composite_state_reason(
            latest_child_transition(&child_states),
            evaluated,
            state,
        ));
        stored.alarm.state_reason_data = Some(composite_state_reason_data(&child_states));
        stored.alarm.state_updated_timestamp = Some(to_timestamp(now_secs));
        // "The timestamp of the last change to the alarm's StateValue."
        // (`CompositeAlarm$StateTransitionedTimestamp`) — this call changed it.
        stored.alarm.state_transitioned_timestamp = Some(to_timestamp(now_secs));
        let updated = AlarmStateSnapshot::of_composite_alarm(&stored.alarm);
        // The actions this transition fires, queued for the call that materialised it to
        // deliver: "the actions to execute when this alarm transitions to the ALARM state"
        // holds for a composite alarm exactly as it does for a metric alarm.
        let notification = alarm_actions::AlarmNotification::of_composite_transition(
            &stored.alarm,
            &child_states,
            previous.state_value.as_deref(),
            now_secs,
        );
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
        // This alarm's new state is an input to every composite alarm whose rule reads it, so
        // their evaluations are owed from this instant too.
        self.make_parents_due(name, now_secs);
        if let Some(notification) = notification {
            self.enqueue_notification(notification);
        }
        true
    }
}

/// One child of a composite alarm's rule, as the parent's explanation of its own state and
/// the notification its transition publishes report it.
#[derive(Debug, Clone)]
pub struct ChildAlarmState {
    /// The child's name, as the rule resolved it.
    pub name: String,
    /// The ARN `DescribeAlarms` reports for the child, when the store holds the child at all.
    pub arn: Option<String>,
    /// The state the child is in, when the rule read one for it.
    pub state_value: Option<String>,
    /// When the child moved to that state, in epoch seconds, when it has ever moved.
    pub transitioned_secs: Option<f64>,
}

impl ChildAlarmState {
    /// One child as the stores hold it.
    fn of(
        name: &str,
        arn: Option<&str>,
        state_value: Option<&str>,
        transitioned: Option<&::aws_smithy_types::DateTime>,
    ) -> Self {
        Self {
            name: name.to_string(),
            arn: arn.map(ToString::to_string),
            state_value: state_value.map(ToString::to_string),
            transitioned_secs: transitioned.map(from_timestamp),
        }
    }

    /// How an explanation names this child: the ARN `DescribeAlarms` reports for it, or its
    /// name when the store holds no ARN for it.
    #[must_use]
    pub fn identity(&self) -> &str {
        self.arn.as_deref().unwrap_or(&self.name)
    }
}

/// The child of a composite alarm's rule whose own transition is the MOST RECENT one: the
/// child a change in the parent's state is attributed to.
///
/// A composite alarm's state changes because one of the alarms its rule reads changed state,
/// so the explanation of that change names THAT alarm, the state it moved to and when it moved
/// there. The most recent transition among the rule's inputs is the one that made the rule's
/// value what it now is: every earlier one was already accounted for by the state the parent
/// held before. Two children that transitioned in the very same instant are
/// indistinguishable, and the last in rule order is named.
///
/// `None` when no child of the rule still exists, or none of them has ever transitioned.
fn latest_child_transition(children: &[ChildAlarmState]) -> Option<&ChildAlarmState> {
    children
        .iter()
        .filter(|child| child.state_value.is_some())
        .max_by(|left, right| {
            left.transitioned_secs
                .unwrap_or(f64::NEG_INFINITY)
                .total_cmp(&right.transitioned_secs.unwrap_or(f64::NEG_INFINITY))
        })
        .filter(|child| child.transitioned_secs.is_some())
}

/// The reason a composite alarm reports for the state its rule evaluation put it in: the child
/// transition the change is attributed to ([`latest_child_transition`]), and what the rule made
/// of it.
///
/// A rule whose children have all been deleted still has an outcome, and the reason then
/// reports the outcome alone: there is no transition to attribute it to.
///
/// Inside the modelled `@length 0..1023` of `StateReason`: the only variable part is the
/// child's identity, and an alarm ARN this service minted is bounded by the alarm name's own
/// `@length 1..255`.
fn composite_state_reason(
    trigger: Option<&ChildAlarmState>,
    evaluated: bool,
    state: &str,
) -> String {
    let outcome = if evaluated { "TRUE" } else { "FALSE" };
    let Some(trigger) = trigger else {
        return format!(
            "The composite alarm's rule evaluated to {outcome}, so the alarm is in {state} state"
        );
    };
    let at = trigger
        .transitioned_secs
        .and_then(rfc3339)
        .map_or_else(String::new, |at| format!(" at {at}"));
    format!(
        "{} transitioned to {}{at} and the composite alarm's rule evaluated to {outcome}, so \
         the alarm is in {state} state",
        trigger.identity(),
        trigger.state_value.as_deref().unwrap_or_default()
    )
}

/// The JSON `StateReasonData` of a composite alarm's evaluated state: the alarms its rule
/// references and the state each one was in when the rule was evaluated.
///
/// "An explanation for the alarm state, in JSON format."
/// (`CompositeAlarm$StateReasonData`) — the alarms whose states the rule reacted to, each
/// named by its ARN and carrying the state it is in and when it moved there, so a consumer can
/// see WHICH child put the parent where it is rather than only that the rule was true. A child
/// the rule read no state for is left out: it triggered nothing.
///
/// Bounded by [`MAX_STATE_REASON_DATA_LEN`] — a document is JSON, so it can never be truncated
/// to fit: the children are dropped instead when a rule references more of them than the bound
/// holds.
fn composite_state_reason_data(children: &[ChildAlarmState]) -> String {
    let mut document = Map::new();
    document.insert("version".to_string(), Value::from(HISTORY_DATA_VERSION));
    let triggering: Vec<Value> = children
        .iter()
        .filter(|child| child.state_value.is_some())
        .map(|child| {
            let mut state = Map::new();
            insert_member(&mut state, "value", json_text(&child.state_value));
            insert_member(
                &mut state,
                "timestamp",
                child
                    .transitioned_secs
                    .and_then(document_instant)
                    .map(Value::from),
            );
            let mut entry = Map::new();
            entry.insert("arn".to_string(), Value::from(child.identity()));
            entry.insert("state".to_string(), Value::Object(state));
            Value::Object(entry)
        })
        .collect();
    let mut full = document.clone();
    full.insert("triggeringAlarms".to_string(), Value::Array(triggering));
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

/// The JSON record of a LOG alarm's configuration write.
pub fn log_configuration_update_history_data(
    previous: Option<&LogAlarm>,
    updated: &LogAlarm,
) -> String {
    configuration_history_data(
        previous.map(log_alarm_configuration_document),
        log_alarm_configuration_document(updated),
        updated.alarm_name.as_deref(),
    )
}

/// A log alarm's configuration as a history document records it: every configuration member
/// the alarm carries, under the model's member name in lower camel case, and only those it
/// carries. The state members are NOT part of it, for the same reason they are not part of a
/// metric alarm's: "If you call this operation with the name of an existing log alarm, the
/// operation replaces the previous configuration of that alarm." (`PutLogAlarm`), so a
/// configuration write records configuration only.
fn log_alarm_configuration_document(alarm: &LogAlarm) -> Value {
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
    insert_member(
        &mut config,
        "scheduledQueryConfiguration",
        alarm
            .scheduled_query_configuration
            .as_ref()
            .map(scheduled_query_configuration_document),
    );
    insert_member(
        &mut config,
        "queryResultsToEvaluate",
        alarm.query_results_to_evaluate.map(Value::from),
    );
    insert_member(
        &mut config,
        "queryResultsToAlarm",
        alarm.query_results_to_alarm.map(Value::from),
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
        "actionLogLineCount",
        alarm.action_log_line_count.map(Value::from),
    );
    insert_member(
        &mut config,
        "actionLogLineRoleArn",
        json_text(&alarm.action_log_line_role_arn),
    );
    insert_member(
        &mut config,
        "warmUpConfiguration",
        alarm
            .warm_up_configuration
            .as_ref()
            .map(warm_up_configuration_document),
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

/// A log alarm's `ScheduledQueryConfiguration` as a history document records it.
fn scheduled_query_configuration_document(configuration: &ScheduledQueryConfiguration) -> Value {
    let mut document = Map::new();
    document.insert(
        "queryString".to_string(),
        Value::from(configuration.query_string.clone()),
    );
    insert_member(
        &mut document,
        "logGroupIdentifiers",
        json_strings(&configuration.log_group_identifiers),
    );
    insert_member(&mut document, "queryARN", json_text(&configuration.query_arn));
    document.insert(
        "scheduledQueryRoleARN".to_string(),
        Value::from(configuration.scheduled_query_role_arn.clone()),
    );
    let schedule = &configuration.schedule_configuration;
    let mut schedule_document = Map::new();
    schedule_document.insert(
        "scheduleExpression".to_string(),
        Value::from(schedule.schedule_expression.clone()),
    );
    schedule_document.insert(
        "startTimeOffset".to_string(),
        Value::from(schedule.start_time_offset),
    );
    insert_member(
        &mut schedule_document,
        "endTimeOffset",
        schedule.end_time_offset.map(Value::from),
    );
    document.insert(
        "scheduleConfiguration".to_string(),
        Value::Object(schedule_document),
    );
    document.insert(
        "aggregationExpression".to_string(),
        Value::from(configuration.aggregation_expression.clone()),
    );
    Value::Object(document)
}

/// A `WarmUpConfiguration` as a history document records it.
fn warm_up_configuration_document(warm_up: &WarmUpConfiguration) -> Value {
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
        alarm
            .warm_up_configuration
            .as_ref()
            .map(warm_up_configuration_document),
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
        let mut seen = HashSet::new();
        self.collect_references(&mut references, &mut seen);
        references
    }

    /// How many nodes this rule carries — what evaluating it once costs.
    ///
    /// Bounded by the node count the parser accepted, which is what bounds this walk as it
    /// bounds [`Self::evaluate`] and [`Self::references`].
    pub fn node_count(&self) -> usize {
        match self {
            Self::Literal(_) | Self::InState { .. } => 1,
            Self::Not(inner) => 1 + inner.node_count(),
            Self::And(left, right) | Self::Or(left, right) => {
                1 + left.node_count() + right.node_count()
            }
        }
    }

    /// Collect this rule's references in order, using `seen` to skip repeats — a SET, so the
    /// walk costs one step per node rather than one scan of everything collected so far.
    fn collect_references(&self, into: &mut Vec<String>, seen: &mut HashSet<String>) {
        match self {
            Self::Literal(_) => {}
            Self::InState { reference, .. } => {
                if seen.insert(reference.clone()) {
                    into.push(reference.clone());
                }
            }
            Self::Not(inner) => inner.collect_references(into, seen),
            Self::And(left, right) | Self::Or(left, right) => {
                left.collect_references(into, seen);
                right.collect_references(into, seen);
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
    let mut seen: HashSet<String> = HashSet::new();
    for reference in rule.references() {
        let name = alarm_rule_reference_name(&reference).to_string();
        if seen.insert(name.clone()) {
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

/// The tokens a rule may carry where a TERM is expected, as CloudWatch lists them in the
/// diagnosis of a rule it could not parse: the open parenthesis and `NOT` that start a
/// grouped or negated term, the `AT_LEAST` quantifier, the two boolean literals, and the
/// three state functions. The list is the service's own, reproduced as it words it.
const RULE_TERM_TOKENS: &str =
    "'(', 'NOT', AT_LEAST, TRUE or FALSE, ALARM, OK, or INSUFFICIENT_DATA";

/// What this parser states it expected where a rule ends before the term it owes: the refusal
/// class the diagnosis above has no recorded wording for.
const TERM_EXPECTED: &str = "expected ALARM, OK, INSUFFICIENT_DATA, TRUE, FALSE, NOT or `(`";

/// One diagnosis in the envelope CloudWatch carries an `AlarmRule` refusal in, so a client
/// that classifies a refusal by its message can tell a rule it could not parse from the two
/// other refusals the same operation answers with the same error code.
fn alarm_rule_diagnosis(diagnosis: &str) -> String {
    format!("Error in AlarmRule [{diagnosis}]")
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

    /// The refusal for a token sitting where the rule can only carry a TERM, or `None` where
    /// the rule carries no token at that position at all — it simply STOPS before the term it
    /// owes, which is a different class, and one no recording words for this service.
    ///
    /// CloudWatch diagnoses a rule that carries such a token by naming the OFFENDING TOKEN and
    /// WHERE in the rule it sits, together with the tokens that position admits
    /// ([`RULE_TERM_TOKENS`]) — which is what tells a client it mistyped the expression rather
    /// than referenced an alarm that does not exist. Both variable parts are read off the rule
    /// the caller submitted: the token as it appears in it ([`Self::token_here`]) and its offset
    /// in CHARACTERS from the start of the rule, which is the position a client counts to in the
    /// string it sent.
    fn unsupported_token(&self) -> Option<AlarmRuleError> {
        let token = self.token_here();
        if token.is_empty() {
            return None;
        }
        let at_char = self
            .text
            .get(..self.at)
            .unwrap_or_default()
            .chars()
            .count();
        Some(AlarmRuleError {
            message: alarm_rule_diagnosis(&format!(
                "Unsupported token '{token}' at char {at_char}, must be: {RULE_TERM_TOKENS}"
            )),
        })
    }

    /// The token the rule carries at the position this parse reached: the whole WORD where the
    /// text continues with one (`AND`, a misspelled function), otherwise the single character
    /// there (a stray operator or bracket), and nothing at all at the end of the rule — where
    /// the rule simply stops before the term it owes.
    fn token_here(&self) -> &'a str {
        let rest = self.rest();
        let Some(first) = rest.chars().next() else {
            return "";
        };
        if first.is_alphanumeric() || first == '_' {
            let end = rest
                .find(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
                .unwrap_or(rest.len());
            &rest[..end]
        } else {
            &rest[..first.len_utf8()]
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
        // Nothing here starts a term. A rule that carries a TOKEN at this position is refused
        // with CloudWatch's own diagnosis of it — the one parse failure a recording words. A
        // rule that simply ENDS here carries no token to name, and nothing records how the
        // service words that, so it keeps this parser's own wording rather than the recorded
        // template with an empty slot in it.
        Err(self
            .unsupported_token()
            .unwrap_or_else(|| self.error(TERM_EXPECTED)))
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
