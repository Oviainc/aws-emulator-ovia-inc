// SPDX-License-Identifier: CC0-1.0

//! The STATISTIC grammar: what names a statistic, and what each one is over a span of stored
//! datapoints.
//!
//! One grammar for every operation of this service that takes a statistic NAME — a metric read,
//! a metric stream's additional statistics and an alarm's `Statistic` / `ExtendedStatistic` —
//! so no two of them can disagree about whether a name is a statistic, or compute a different
//! number for one span. A name is matched by its own spelling: the basic five as the `Statistic`
//! enumerated type declares them, the percentile form with its lower-case `p`, the interquartile
//! mean as the definitions page writes it, and the bounded families in either documented form.

use super::*;

/// The percentile a `pNN` / `pNN.NN` extended statistic names, in percent, or `None` for a
/// name that is not one. The `p` is matched EXACTLY: the percentile form is spelled with a
/// lower-case `p`, so an upper-case `P90` names no statistic this service supports. The number
/// is a plain decimal in `0..=100`, so `f64`'s sign and exponent forms (`p+9`, `p1e2`) cannot
/// slip through where a percentile is expected.
///
/// "The percentile statistics. Specify values between p0.0 and p100."
/// (`GetMetricStatisticsInput$ExtendedStatistics`)
pub fn parse_percentile(name: &str) -> Option<f64> {
    percentage(name.strip_prefix(PERCENTILE_PREFIX)?)
}

/// How a percentile statistic is spelled, exactly: CloudWatch documents the form as `p0.0` to
/// `p100` and supports no other casing of it.
pub(super) const PERCENTILE_PREFIX: &str = "p";

/// The statistics CloudWatch names outright on its "CloudWatch statistics definitions" page:
/// the five basic statistics the store folds, and the interquartile mean.
pub(super) const CLOUDWATCH_BASIC_STATISTICS: &[&str] =
    &["SampleCount", "Average", "Sum", "Minimum", "Maximum", "IQM"];

/// The statistic families from the same page that take a trimming span, either as a bare
/// percentile ("this includes tm98, wm90") or as a span: trimmed mean, trimmed count,
/// trimmed sum and Winsorized mean.
pub(super) const TRIMMED_FAMILIES: &[&str] = &["tm", "tc", "ts", "wm"];

/// The five basic statistics this store folds out of the four aggregates every datapoint
/// carries ([`BasicStatistics::named`]); every other statistic is computed from the window's
/// own distribution instead ([`BoundedStatistic::of`], [`weighted_percentile`]).
pub(super) const FOLDED_STATISTICS: &[&str] = &["SampleCount", "Average", "Sum", "Minimum", "Maximum"];

/// Whether `statistic` is one CloudWatch supports at all: a basic statistic, a percentile, or
/// a member of a bounded family. Shared by every operation that takes a statistic NAME and
/// admits the basic five among them — `MetricStat$Stat` is an unconstrained string in the
/// model, so the grammar is the only thing that tells a statistic from a typo. An operation
/// that admits only the statistics computed over the distribution asks
/// [`is_extended_statistic`] instead.
///
/// A statistic is named by its own spelling: the basic five are the names the `Statistic`
/// enumerated type declares (`smithy_query(query="shape",shape="Statistic")`) and the
/// percentile form is lower-case, so a name differing only in case is not that statistic.
pub fn is_cloudwatch_statistic(statistic: &str) -> bool {
    CLOUDWATCH_BASIC_STATISTICS.contains(&statistic)
        || parse_percentile(statistic).is_some()
        || bounded_statistic(statistic).is_some()
}

/// The interquartile mean, which is the trimmed mean of the middle half of the distribution:
/// "Interquartile mean. The trimmed mean of the interquartile range" — the same statistic as
/// `TM(25%:75%)`.
pub(super) const INTERQUARTILE_SPAN: (f64, f64) = (25.0, 75.0);

/// How the interquartile mean is spelled, exactly, on the definitions page that names it.
pub(super) const INTERQUARTILE_MEAN: &str = "IQM";

/// A statistic of one of the bounded families, in either documented form:
/// `<family><percentile>` (`tm98`) or `<family>(<low>:<high>)` (`TM(10%:90%)`, `PR(:300)`),
/// where one side of a span may be omitted but not both — and the interquartile mean, which
/// is one of them under its own name.
pub(super) fn bounded_statistic(statistic: &str) -> Option<BoundedStatistic> {
    if statistic == INTERQUARTILE_MEAN {
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
pub(super) fn parse_span(span: &str) -> Option<(Option<Bound>, Option<Bound>)> {
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
pub(super) fn bound_value(bound: &str) -> Option<Bound> {
    match bound.strip_suffix('%') {
        Some(percent) => percentage(percent).map(Bound::Percentile),
        None => decimal(bound).map(Bound::Value),
    }
}

/// One bound of a bounded statistic's span.
#[derive(Debug, Clone, Copy)]
pub(super) enum Bound {
    /// A place in the window's own distribution, in percent.
    Percentile(f64),
    /// A metric value.
    Value(f64),
}

/// The statistic families CloudWatch defines over a span of the window's distribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BoundedFamily {
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
    pub(super) fn named(prefix: &str) -> Option<Self> {
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
pub(super) struct BoundedStatistic {
    /// Which of the families it is.
    pub(super) family: BoundedFamily,
    /// The low end of the span, when the statistic names one.
    pub(super) low: Option<Bound>,
    /// The high end of the span, when the statistic names one.
    pub(super) high: Option<Bound>,
}

impl BoundedStatistic {
    /// This statistic over a window's distribution: the published values with their weights,
    /// ascending. `None` when the span is empty of values, so the statistic is undefined
    /// there.
    pub(super) fn of(&self, values: &[(f64, f64)]) -> Option<f64> {
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
pub(super) fn trim(
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
pub(super) fn percentage(text: &str) -> Option<f64> {
    decimal(text).filter(|value| (0.0..=100.0).contains(value))
}

/// A plain non-negative decimal number, so `f64`'s sign and exponent forms (`+5`, `1e3`,
/// `inf`) cannot slip through where a statistic's bound is expected.
pub(super) fn decimal(text: &str) -> Option<f64> {
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
pub(super) fn strip_prefix_ignore_ascii_case<'text>(text: &'text str, prefix: &str) -> Option<&'text str> {
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
pub(super) fn distribution(data: &[&StoredDatum]) -> Option<Vec<(f64, f64)>> {
    let mut values: Vec<(f64, f64)> = Vec::with_capacity(data.len());
    for datum in data {
        values.push((datum.single_value()?, datum.sample_count));
    }
    values.sort_by(|left, right| left.0.total_cmp(&right.0));
    Some(values)
}

/// The `percentile`-th value of a distribution whose weights sum to `total`, by nearest rank.
pub(super) fn percentile_of(values: &[(f64, f64)], percentile: f64, total: f64) -> Option<f64> {
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
/// model names (spelled as the `Statistic` enumerated type declares it), a `pNN` percentile, or
/// a statistic over a span of the window's own distribution (the interquartile mean and the
/// trimmed / Winsorized / percentile-rank families).
///
/// `None` for a name outside the grammar, and for a statistic the span does not define.
pub fn statistic_value(data: &[&StoredDatum], statistic: &str) -> Option<f64> {
    if let Some(folded) = FOLDED_STATISTICS.iter().find(|known| **known == statistic) {
        return BasicStatistics::of(data.iter().copied()).named(folded);
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
