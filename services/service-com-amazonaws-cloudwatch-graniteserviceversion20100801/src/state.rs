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
//!   Contributor Insights rules written by `PutInsightRule` and read back by
//!   `DescribeInsightRules`.
//! * [`ComAmazonawsCloudwatchGraniteserviceversion20100801State::dashboards`] — the
//!   dashboards written by `PutDashboard` and read back by `GetDashboard` and
//!   `ListDashboards`, each holding the `DashboardBody` document verbatim.
//! * [`ComAmazonawsCloudwatchGraniteserviceversion20100801State::dataset_kms_keys`] — the
//!   customer managed KMS key each Region's default dataset is encrypted with, read back by
//!   `GetDataset`.
//!
//! An alarm is stored as the modelled [`MetricAlarm`] shape itself rather than a parallel
//! record, so every member `PutMetricAlarm` accepts round-trips through `DescribeAlarms`
//! without a field-by-field copy that can silently drop one. A metric stream is stored the
//! same way, as the modelled [`GetMetricStreamOutput`] its reader returns.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use ::serde_json::{Map, Value};

use crate::ops::types::{
    ActionConfiguration, AlarmHistoryItem, CompositeAlarm, Dimension, EvaluationCriteria,
    EvaluationWindow, GetMetricStreamOutput, InsightRule, LogAlarm, MetricAlarm, MetricDataQuery,
    ScheduledQueryConfiguration, Tag, WarmUpConfiguration,
};

/// The action notifications an alarm state transition owes, and how they are delivered to
/// the ARNs the alarm names.
pub mod alarm_actions;

/// A metric alarm's `StateValue`, computed from the datapoints this store holds.
pub mod alarm_evaluation;

/// The metric math expression language a `MetricDataQuery$Expression` carries, parsed and
/// evaluated over the series this store holds.
pub mod metric_math;

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

/// The sub-minute `Period` values a metric may be read at: "For metrics with regular
/// resolution, a period can be as short as one minute (60 seconds) and must be a multiple of
/// 60. For high-resolution metrics that are collected at intervals of less than one minute,
/// the period can be 1, 5, 10, 20, 30, 60, or any multiple of 60." (`MetricStat$Period`,
/// `GetMetricStatisticsInput$Period`) — every other legal period is a multiple of 60.
pub const SUB_MINUTE_PERIODS: [i32; 5] = [1, 5, 10, 20, 30];

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
        format!("Period must be a positive 1, 5, 10, 20, 30 or multiple of 60; got {period}")
    })
}

/// Fold one series' datapoints into the `period`-wide windows of `[start, end)`, keyed by the
/// window's index from `start`.
///
/// `start` is INCLUSIVE and `end` EXCLUSIVE, as both readers' members document. A window
/// holding no datapoint is never inserted, so what this allocates is bounded by the
/// datapoints the series holds in the range rather than by the range the request spans.
///
/// `unit` is the caller's unit filter: "If you specify a unit, the operation returns only
/// data that was collected with that unit specified. […] CloudWatch does not perform unit
/// conversions." A request naming none takes every datapoint.
pub fn windows_of<'a>(
    data: &'a [StoredDatum],
    start: f64,
    end: f64,
    period: f64,
    unit: Option<&str>,
) -> BTreeMap<i64, Vec<&'a StoredDatum>> {
    let mut windows: BTreeMap<i64, Vec<&StoredDatum>> = BTreeMap::new();
    for datum in data {
        if datum.timestamp_secs < start || datum.timestamp_secs >= end {
            continue;
        }
        if unit.is_some_and(|unit| datum.unit.as_deref() != Some(unit)) {
            continue;
        }
        #[allow(clippy::cast_possible_truncation)]
        let index = ((datum.timestamp_secs - start) / period).floor() as i64;
        windows.entry(index).or_default().push(datum);
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
/// a member of a bounded family. Shared by every operation that takes a statistic NAME —
/// `MetricStat$Stat` is an unconstrained string in the model, so the grammar is the only thing
/// that tells a statistic from a typo.
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
    /// starts one of the same length: the state it forces stands until that evaluation is due
    /// ([`alarm_evaluation::forced_state_span_secs`]).
    pub evaluation_due_secs: f64,
    /// The instant, in epoch seconds, until which a state `SetAlarmState` forced on this alarm
    /// stands; `None` when the alarm reports its own evaluated state.
    ///
    /// "Temporarily sets the state of an alarm for testing purposes." (`SetAlarmState`) — a
    /// forced state is meant to stand for the span the force started, so an arriving datapoint
    /// does not cut it short:
    /// [`ComAmazonawsCloudwatchGraniteserviceversion20100801State::make_alarms_due`] leaves an
    /// alarm holding one alone until that span is out.
    pub forced_state_until_secs: Option<f64>,
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
    /// Metric streams by stream name.
    pub metric_streams: HashMap<String, StoredMetricStream>,
    /// Contributor Insights rules by rule name.
    pub insight_rules: HashMap<String, StoredInsightRule>,
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

/// Why `value` is not the ARN of an IAM role, or `None` when it is one.
///
/// A member that carries "the Amazon Resource Name (ARN) of an IAM role that CloudWatch
/// assumes" (`PutLogAlarmInput$ActionLogLineRoleArn`,
/// `ScheduledQueryConfiguration$ScheduledQueryRoleARN`) names an IAM role, so a value that is
/// not a role ARN at all could never be assumed by anything. Whether the role EXISTS, and
/// whether CloudWatch may assume it, is not decided here: that is settled when the query runs
/// and is reported on the alarm, not refused at configuration time.
pub fn role_arn_error(member: &str, value: &str) -> Option<String> {
    let fields: Vec<&str> = value.splitn(6, ':').collect();
    let is_role_arn = matches!(fields.as_slice(), ["arn", partition, "iam", "", account, resource]
        if !partition.is_empty()
            && !account.is_empty()
            && resource.strip_prefix("role/").is_some_and(|name| !name.is_empty()));
    (!is_role_arn).then(|| format!("{member} is not the ARN of an IAM role: {value}"))
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

/// An alarm's action list as the stored alarm carries it, and so as every reader of an alarm
/// reports it: the actions the request configured, and the EMPTY list when it configured none.
///
/// `OKActions`, `AlarmActions` and `InsufficientDataActions` are `ResourceList`s of
/// `length 0..5` on every alarm writer, so "no action" is a list of none rather than no list:
/// a caller iterating a described alarm's actions, or reading one alarm to write it back, is
/// never left telling an empty list from an absent one. Shared by the alarm writers so one
/// rule answers them all.
pub fn configured_actions(actions: Option<&[String]>) -> Option<Vec<String>> {
    Some(actions.unwrap_or_default().to_vec())
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

/// The `Schema` every Contributor Insights rule a caller creates carries: "For rules that
/// you create, this is always `{"Name": "CloudWatchLogRule", "Version": 1}`. For managed
/// rules, this is `{"Name": "ServiceLogRule", "Version": 1}`" (`InsightRule$Schema`).
pub const INSIGHT_RULE_SCHEMA: &str = "{\"Name\": \"CloudWatchLogRule\", \"Version\": 1}";

/// A Contributor Insights rule that is evaluating log events: "The state of the rule. Valid
/// values are ENABLED and DISABLED." (`PutInsightRuleInput$RuleState`)
pub const INSIGHT_RULE_ENABLED: &str = "ENABLED";

/// A Contributor Insights rule that is not evaluating log events
/// (`PutInsightRuleInput$RuleState`).
pub const INSIGHT_RULE_DISABLED: &str = "DISABLED";

/// The `Schema` a managed Contributor Insights rule carries: "For managed rules, this is
/// `{"Name": "ServiceLogRule", "Version": 1}`" (`InsightRule$Schema`) — the schema of a rule
/// `PutManagedInsightRules` enabled, which is why `PutInsightRule` refuses a definition
/// carrying it ("You cannot edit these rules with PutInsightRule",
/// `PutManagedInsightRules`).
pub const MANAGED_INSIGHT_RULE_SCHEMA: &str = "{\"Name\": \"ServiceLogRule\", \"Version\": 1}";

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

/// The taggable resource one ARN names: its kind, and the name the store holding that kind
/// keys on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaggableResource<'a> {
    /// Which of this service's resources the ARN names.
    pub kind: TaggableKind,
    /// The resource's name, as the ARN's resource part carries it.
    pub name: &'a str,
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
/// The resource is resolved from its KIND and NAME, which is what every store here keys on;
/// the partition, region and account fields are not part of that key.
pub fn parse_taggable_arn(arn: &str) -> Option<TaggableResource<'_>> {
    let resource = ::service_common::arn::parse_of(arn, ARN_NAMESPACE)?.resource;
    TAGGABLE_ARN_PREFIXES.iter().find_map(|(prefix, kind)| {
        resource
            .strip_prefix(prefix)
            .filter(|name| !name.is_empty())
            .map(|name| TaggableResource { kind: *kind, name })
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

/// Why a resource this store does not hold cannot carry tags. The caller reports it through
/// its modelled `ResourceNotFoundException` ("The named resource does not exist").
pub fn unknown_resource_message(arn: &str) -> String {
    format!("the CloudWatch resource {arn} does not exist")
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
                .get(resource.name)
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
                .get_mut(resource.name)
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
    /// carries, and an alarm holding a state a caller forced keeps it until that force's own
    /// span is out.
    ///
    /// An alarm is matched on the series it names — namespace, metric name and dimension set —
    /// which is the identity its own evaluation reads, so a datapoint of any other series makes
    /// no alarm due.
    pub fn make_alarms_due(&mut self, keys: &[MetricKey], now_secs: f64) {
        if keys.is_empty() {
            return;
        }
        let due = now_secs + alarm_evaluation::EVALUATION_DELAY_SECS;
        for stored in self.alarms.values_mut() {
            if stored
                .forced_state_until_secs
                .is_some_and(|until| now_secs < until)
            {
                continue;
            }
            let watched = MetricKey {
                namespace: stored.alarm.namespace.clone().unwrap_or_default(),
                metric_name: stored.alarm.metric_name.clone().unwrap_or_default(),
                dimensions: dimension_set(stored.alarm.dimensions.as_deref()),
            };
            if keys.contains(&watched) {
                stored.evaluation_due_secs = stored.evaluation_due_secs.min(due);
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
    /// `AlarmConfigurationUpdatedTimestamp` and records the `ConfigurationUpdate` history item
    /// every other configuration write records. Asking for the setting an alarm already has
    /// changes nothing, so it records nothing either.
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
                let data = configuration_update_history_data(Some(&previous), &stored.alarm);
                records.push((name.to_string(), ALARM_TYPE_METRIC, data));
            } else if let Some(stored) = self.composite_alarms.get_mut(name) {
                if stored.alarm.actions_enabled == Some(enabled) {
                    continue;
                }
                let previous = stored.alarm.clone();
                stored.alarm.actions_enabled = Some(enabled);
                stored.alarm.alarm_configuration_updated_timestamp = Some(to_timestamp(now_secs));
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
    /// value changed. Alarms are visited in name order, so the pass is deterministic, and the
    /// pass stops as soon as `nodes` or the request's own `budget` is spent.
    fn evaluate_composite_alarms_once(
        &mut self,
        now_secs: f64,
        nodes: &mut usize,
        budget: &mut alarm_evaluation::EvaluationBudget,
    ) -> bool {
        let mut names: Vec<String> = self.composite_alarms.keys().cloned().collect();
        names.sort();
        let mut changed = false;
        for name in names {
            if *nodes == 0 || budget.spent() {
                break;
            }
            budget.charge_alarm();
            changed |= self.evaluate_composite_alarm(&name, now_secs, nodes);
        }
        changed
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
                child.transitioned_secs.and_then(rfc3339).map(Value::from),
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
