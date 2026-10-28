// SPDX-License-Identifier: CC0-1.0

//! Metric extraction from an INGESTED log event through the log group's METRIC FILTERS, the
//! path that turns a matching log event into CloudWatch metric data.
//!
//! A metric filter is a configuration-time rule — "configure rules to extract metric data
//! from log events ingested through PutLogEvents" — so the work happens on INGEST: each
//! event of the batch is run against every filter of the log group it lands in, and each
//! transformation of a filter that matches publishes "the value to publish to the CloudWatch
//! metric when a filter pattern matches a log event" under its own namespace and metric
//! name. A transformation that declares `dimensions` publishes one dimension per entry,
//! whose value is what that entry's selector resolves to IN THAT EVENT, so every distinct
//! value found is its own series. A transformation that declares a `defaultValue` publishes
//! it for an event the pattern does NOT match ("the value to emit when a filter pattern does
//! not match a log event"), which is why an absent default leaves a non-matching event
//! publishing nothing.
//!
//! The datapoints go to the SAME CloudWatch metric store the Embedded Metric Format path
//! writes and `GetMetricStatistics` / `GetMetricData` read, through the one publisher in
//! [`super::emf`], so a metric has one home whichever ingest path produced it.

use std::collections::BTreeMap;

use super::emf::{ExtractedDatapoint, DEFAULT_STORAGE_RESOLUTION};
use super::filter_pattern::FilterPattern;
use super::{LogGroupRecord, MetricTransformationRecord};

/// The metric filters of ONE log group, with each filter pattern parsed into the form it is
/// RUN in, ready to be applied to the events of one batch.
///
/// The patterns are parsed ONCE per batch rather than once per event: a batch is up to
/// 10,000 events and a log group up to 100 filters, so the parse belongs outside both loops.
#[derive(Debug, Default)]
pub struct CompiledFilters {
    /// One entry per filter whose pattern this module could parse, in the group's own filter
    /// order, so the datapoints one event publishes are in the same order on every run.
    filters: Vec<CompiledFilter>,
}

/// One metric filter, ready to run.
#[derive(Debug)]
struct CompiledFilter {
    /// The filter's `filterPattern`, parsed.
    pattern: FilterPattern,
    /// The transformations it emits metric data through.
    transformations: Vec<MetricTransformationRecord>,
}

impl CompiledFilters {
    /// Every metric filter `group` holds, compiled.
    ///
    /// A pattern that does not parse is left out rather than failing the ingest: the batch
    /// was accepted, and a filter the emulator cannot run extracts nothing. Every pattern a
    /// `PutMetricFilter` stored has already been parsed once, when that put was accepted.
    pub fn of(group: &LogGroupRecord) -> Self {
        let filters = group
            .metric_filters
            .values()
            .filter_map(|filter| {
                FilterPattern::parse(&filter.filter_pattern)
                    .ok()
                    .map(|pattern| CompiledFilter {
                        pattern,
                        transformations: filter.transformations.clone(),
                    })
            })
            .collect();
        Self { filters }
    }

    /// The datapoints ONE ingested log event publishes through these filters.
    ///
    /// `event_timestamp_ms` is the event's own timestamp — the instant the values were
    /// observed — so a datapoint falls in the period the event belongs to rather than the
    /// one the batch happened to arrive in.
    ///
    /// `budget` is how many datapoints this event may still publish, so the per-event ceiling
    /// ([`super::emf::MAX_DATAPOINTS_PER_EVENT`]) bounds what ONE event publishes across every
    /// extraction path rather than once per path: the caller passes what the other paths left.
    pub fn datapoints(
        &self,
        message: &str,
        event_timestamp_ms: i64,
        budget: usize,
    ) -> Vec<ExtractedDatapoint> {
        if self.filters.is_empty() || budget == 0 {
            return Vec::new();
        }
        // The readings of the event a selector resolves against, each taken at most ONCE for
        // the whole event however many filters and selectors ask for it — and not at all when
        // none does.
        let event = Event::of(message);
        let mut datapoints: Vec<ExtractedDatapoint> = Vec::new();
        for filter in &self.filters {
            if datapoints.len() >= budget {
                return datapoints;
            }
            let matched = filter.pattern.match_message(message);
            for transformation in &filter.transformations {
                if datapoints.len() >= budget {
                    return datapoints;
                }
                let datapoint = match &matched {
                    Some(extracted) => {
                        matched_datapoint(transformation, extracted, &event, event_timestamp_ms)
                    }
                    None => default_datapoint(transformation, event_timestamp_ms),
                };
                if let Some(datapoint) = datapoint {
                    datapoints.push(datapoint);
                }
            }
        }
        datapoints
    }
}

/// ONE ingested event, as a `$…` selector reads it.
///
/// A selector names a field of the event rather than of the pattern, so these are the two
/// readings of the message the filter-pattern syntax selects from: the JSON document a
/// `$.path` selects a member of, and the space-delimited fields a `$N` selects by position.
/// Each is derived on first use and kept, so an event is read at most once per reading and a
/// selector no transformation states costs nothing.
#[derive(Debug)]
struct Event<'a> {
    /// The event's message, the one thing both readings are derived from.
    message: &'a str,
    /// The event parsed as a JSON document, `None` for a message that is not one.
    document: ::std::cell::OnceCell<Option<::serde_json::Value>>,
    /// The event's space-delimited fields, in the order the message carries them.
    fields: ::std::cell::OnceCell<Vec<String>>,
}

impl<'a> Event<'a> {
    /// The event carrying `message`, with neither reading taken yet.
    fn of(message: &'a str) -> Self {
        Self {
            message,
            document: ::std::cell::OnceCell::new(),
            fields: ::std::cell::OnceCell::new(),
        }
    }

    /// The event as a JSON document, for a message that is one.
    fn document(&self) -> Option<&::serde_json::Value> {
        self.document
            .get_or_init(|| ::serde_json::from_str::<::serde_json::Value>(self.message).ok())
            .as_ref()
    }

    /// The event's space-delimited fields.
    fn fields(&self) -> &[String] {
        self.fields
            .get_or_init(|| super::filter_pattern::space_delimited_fields(self.message))
    }
}

/// The datapoint one transformation publishes for an event its filter pattern MATCHED, or
/// `None` when the event supplies no value to publish.
///
/// A value the transformation reads from the event is only there when the event carries that
/// field, and a dimensioned metric needs a value for EVERY dimension it declares — a
/// dimension with no value is not a series — so an event that leaves either unresolved
/// publishes nothing rather than a datapoint standing on a value nothing supplied.
fn matched_datapoint(
    transformation: &MetricTransformationRecord,
    extracted: &BTreeMap<String, String>,
    event: &Event,
    event_timestamp_ms: i64,
) -> Option<ExtractedDatapoint> {
    let value = metric_value(transformation, extracted, event)?;
    let mut dimensions: BTreeMap<String, String> = BTreeMap::new();
    for (name, selector) in &transformation.dimensions {
        let resolved = selector_text(selector, extracted, event)?;
        dimensions.insert(name.clone(), resolved);
    }
    Some(ExtractedDatapoint {
        namespace: transformation.metric_namespace.clone(),
        metric_name: transformation.metric_name.clone(),
        dimensions,
        timestamp_ms: event_timestamp_ms,
        value,
        unit: transformation.unit.clone(),
        storage_resolution: DEFAULT_STORAGE_RESOLUTION,
    })
}

/// The datapoint one transformation publishes for an event its filter pattern did NOT match:
/// its `defaultValue`, and nothing at all when it declares none.
///
/// The default value is published under NO dimension, which is the only set it could be
/// published under: a transformation never declares both a default value and dimensions
/// ([`super::METRIC_FILTER_DEFAULT_VALUE_WITH_DIMENSIONS`]), and a non-matching event
/// resolves no dimension value in any case.
fn default_datapoint(
    transformation: &MetricTransformationRecord,
    event_timestamp_ms: i64,
) -> Option<ExtractedDatapoint> {
    let value = transformation.default_value?;
    Some(ExtractedDatapoint {
        namespace: transformation.metric_namespace.clone(),
        metric_name: transformation.metric_name.clone(),
        dimensions: BTreeMap::new(),
        timestamp_ms: event_timestamp_ms,
        value,
        unit: transformation.unit.clone(),
        storage_resolution: DEFAULT_STORAGE_RESOLUTION,
    })
}

/// The number one transformation publishes for a matching event: the field of the event its
/// `metricValue` selects, or the literal number it states.
///
/// `metricValue` is "the value to publish to the CloudWatch metric when a filter pattern
/// matches a log event", so it is either a selector naming where in the event the number is
/// — the same `$…` selectors a dimension value uses
/// ([`super::is_log_event_field_reference`]) — or the number itself, which is what a filter
/// counting its matching events states. A selected field that is not a number, and a literal
/// that is not one, measure nothing, so neither publishes a datapoint.
fn metric_value(
    transformation: &MetricTransformationRecord,
    extracted: &BTreeMap<String, String>,
    event: &Event,
) -> Option<f64> {
    let stated = transformation.metric_value.trim();
    let text = if super::is_log_event_field_reference(stated) {
        selector_text(stated, extracted, event)?
    } else {
        stated.to_string()
    };
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|value: &f64| value.is_finite())
}

/// What one `$…` selector resolves to in the event being ingested.
///
/// The values the matching pattern extracted answer first, which is how a space-delimited
/// field named by the pattern's own field list (`$field`) is reachable at all. Past them the
/// selector is resolved against the EVENT, in the reading its form names: `$.path` against
/// the JSON document, `$N` against the Nth space-delimited field of the message, 1-based.
/// That is what makes a selector readable where the pattern extracted nothing under it — an
/// unstructured pattern names no field of the event, a named slot is extracted under its name
/// rather than its position, and the extraction of a large document stops at its own bounds.
///
/// A positional selector therefore counts the fields of the MESSAGE. Those are the slots of
/// the pattern's field list wherever the list describes every field of the event, and differ
/// only for a list whose `...` stands for a run of them, where the slot a position names is
/// not fixed; the event's own fields are the reading that is defined for every pattern form,
/// including the ones that declare no field list at all.
fn selector_text(
    selector: &str,
    extracted: &BTreeMap<String, String>,
    event: &Event,
) -> Option<String> {
    if let Some(text) = extracted.get(selector) {
        return Some(text.clone());
    }
    let path = selector.strip_prefix('$')?;
    if path.starts_with('.') {
        return super::filter_pattern::json_selector_text(event.document()?, selector);
    }
    let position = path.parse::<usize>().ok()?.checked_sub(1)?;
    event.fields().get(position).cloned()
}
