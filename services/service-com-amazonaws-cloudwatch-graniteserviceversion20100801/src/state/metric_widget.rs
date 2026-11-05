// SPDX-License-Identifier: CC0-1.0

//! The metric widget definition `GetMetricWidgetImage` renders, read over this store.
//!
//! "A JSON string that defines the bitmap graph to be retrieved. The string includes the
//! metrics to include in the graph, statistics, annotations, title, axis limits, and so on."
//! (`GetMetricWidgetImageInput$MetricWidget`) — so a widget definition is a DOCUMENT, parsed
//! here with the maintained JSON parser, and what it names is resolved over the series this
//! store holds rather than drawn from anything in the request.
//!
//! The resolution is the SAME one `GetMetricData` answers from and a metric-math alarm is
//! graded by ([`super::metric_query`]): a widget's `metrics` array becomes the
//! [`MetricDataQuery`] array that resolution takes, so a graph and a read of the same metric
//! over the same window cannot disagree about what the series is. What is local to a widget is
//! the document's own vocabulary — the shorthand its `metrics` entries are written in, the
//! members that place the graph's window and axes, and the defaults it takes for the ones it
//! leaves out — and the bitmap the resolved series are drawn into ([`render`]).
//!
//! Every member this reading does not act on (`region`, `accountId`, `timezone`, `title`,
//! `legend`, `liveData`, `sparkline`, `labels`, `setPeriodToTimeRange`) is accepted and
//! ignored: the store holds one account's and Region's series, and the rest change how a
//! console lays a graph out rather than what the graph is of.

use ::serde_json::{Map, Value};

use super::metric_math::{MathSeries, MathValue};
use super::metric_query::{self, QueryRefusal, Resolution, StoreSeriesSource};
use super::ComAmazonawsCloudwatchGraniteserviceversion20100801State as State;
use crate::ops::types::{Dimension, Metric, MetricDataQuery, MetricStat};

/// The bitmap a resolved widget is drawn into: the graph the operation answers with.
pub mod render;

/// The request member a widget definition arrives on, as a refusal of something inside it
/// names it (`GetMetricWidgetImageInput$MetricWidget`).
pub const WIDGET_MEMBER: &str = "MetricWidget";

/// The member of the definition that names the metrics the graph is of.
const METRICS_MEMBER: &str = "metrics";

/// "Up to 100 KB uncompressed payload." (`GetMetricWidgetImage`) — the definition is the
/// payload of the call, so a document past that is refused before it is parsed.
pub const MAX_WIDGET_BYTES: usize = 100 * 1024;

/// "As many as 100 metrics in the graph." (`GetMetricWidgetImage`)
pub const MAX_WIDGET_METRICS: usize = 100;

/// The graph's width in pixels when the definition names none: "width — The width of the
/// graph in pixels. The default is 600." (the metric widget structure reference
/// `GetMetricWidgetImageInput$MetricWidget` points at).
pub const DEFAULT_WIDTH: u32 = 600;

/// The graph's height in pixels when the definition names none: "height — The height of the
/// graph in pixels. The default is 395." (the same reference).
pub const DEFAULT_HEIGHT: u32 = 395;

/// The widest and tallest graph this emulator renders.
///
/// Not a published AWS limit: it is the bound on the ALLOCATION a caller can ask for here, so
/// a definition naming a larger graph is refused rather than served from a bitmap sized by the
/// request. Comfortably above the largest graph a dashboard widget is rendered at.
pub const MAX_IMAGE_SIDE: u32 = 2_000;

/// The granularity the graph's windows take when the definition names none: "period — The
/// default is 300." (the metric widget structure reference).
pub const DEFAULT_PERIOD: i32 = 300;

/// The statistic a metric of the graph is folded by when neither it nor the definition names
/// one: "stat — The default is Average." (the same reference).
pub const DEFAULT_STAT: &str = "Average";

/// How far back the graph's window reaches when the definition names no `start`: three hours,
/// the span the relative default `-PT3H` names (the same reference).
pub const DEFAULT_RANGE_SECS: f64 = 3.0 * 60.0 * 60.0;

/// The `view` values the metric widget structure documents ("view — The default is
/// timeSeries. Valid values: timeSeries | singleValue | gauge | bar | pie"), which is the set
/// a definition naming something else is refused against.
const VIEWS: [&str; 5] = ["timeSeries", "singleValue", "gauge", "bar", "pie"];

/// Which plot the resolved series are drawn as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// The value over time, as a line through the series' points — the default view, and the
    /// plot the views that are not a column chart are drawn as: a graph of the same resolved
    /// series is what every one of them reports.
    TimeSeries,
    /// The value over time, as one column per window (`view: bar`).
    Bar,
}

/// One widget definition, resolved against the clock: its window, its axes, and the queries
/// its `metrics` array names.
#[derive(Debug, Clone)]
pub struct MetricWidget {
    /// The graph's width in pixels.
    pub width: u32,
    /// The graph's height in pixels.
    pub height: u32,
    /// Which plot the series are drawn as.
    pub view: View,
    /// Whether the graph is drawn on the dark theme (`theme: dark`).
    pub dark: bool,
    /// The first instant of the graph's window, in epoch seconds.
    pub start: f64,
    /// The first instant after the graph's window, in epoch seconds.
    pub end: f64,
    /// The granularity the window is folded into.
    pub period: i32,
    /// The value the left axis starts at, when `yAxis.left.min` fixes it.
    pub y_min: Option<f64>,
    /// The value the left axis ends at, when `yAxis.left.max` fixes it.
    pub y_max: Option<f64>,
    /// The values the graph draws a horizontal annotation line at
    /// (`annotations.horizontal[].value`).
    pub annotations: Vec<f64>,
    /// The queries the `metrics` array names, in the order it lists them — the array the
    /// shared resolution takes.
    pub queries: Vec<MetricDataQuery>,
    /// How each of those queries is drawn, by position in [`MetricWidget::queries`].
    pub plots: Vec<Plot>,
}

/// How one query of the definition is drawn.
#[derive(Debug, Clone)]
pub struct Plot {
    /// The `Id` the query resolves under.
    pub id: String,
    /// The colour the series is drawn in: the one the entry's `color` option names, else the
    /// one the graph palette assigns it.
    pub color: [u8; 3],
    /// Whether the series is drawn at all ("visible — Set this to false to hide the metric
    /// from the graph", the metric widget structure reference), which is the same thing the
    /// resolution calls returning the query's data.
    pub visible: bool,
}

/// One series the graph draws: its label, its colour, and its points oldest first.
#[derive(Debug, Clone)]
pub struct PlottedSeries {
    /// The label the graph legends the series under.
    pub label: String,
    /// The colour it is drawn in.
    pub color: [u8; 3],
    /// Its `(epoch seconds, value)` points, oldest first.
    pub points: Vec<(f64, f64)>,
}

/// The colours a graph assigns its series when their entries name none.
///
/// "color — The six-digit HTML hex color code to be used for this metric. … If you omit this
/// field, a color is assigned automatically." (the metric widget structure reference) — the
/// assignment cycles this palette in the order the `metrics` array lists its entries, so the
/// same definition always draws the same series in the same colour.
const PALETTE: [[u8; 3]; 10] = [
    [0x1f, 0x77, 0xb4],
    [0xff, 0x7f, 0x0e],
    [0x2c, 0xa0, 0x2c],
    [0xd6, 0x27, 0x28],
    [0x94, 0x67, 0xbd],
    [0x8c, 0x56, 0x4b],
    [0xe3, 0x77, 0xc2],
    [0x7f, 0x7f, 0x7f],
    [0xbc, 0xbd, 0x22],
    [0x17, 0xbe, 0xcf],
];

impl MetricWidget {
    /// The widget definition `document` names, resolved against the instant `now`, or the
    /// sentence refusing it.
    ///
    /// Every refusal names the member path inside the definition that has to change, in the
    /// sentences this service refuses a value in ([`super::value_is_invalid_message`] and the
    /// parameter sentences beside it), because that path is what locates the node a caller
    /// repairs.
    pub fn parse(document: &str, now: f64) -> Result<Self, String> {
        if document.len() > MAX_WIDGET_BYTES {
            return Err(format!(
                "The field {WIDGET_MEMBER} must not be larger than {MAX_WIDGET_BYTES} bytes"
            ));
        }
        let widget = ::serde_json::from_str::<Value>(document)
            .ok()
            .and_then(|parsed| match parsed {
                Value::Object(members) => Some(members),
                _ => None,
            })
            .ok_or_else(|| format!("The field {WIDGET_MEMBER} must be a valid JSON object"))?;

        let width = pixels(&widget, "width", DEFAULT_WIDTH)?;
        let height = pixels(&widget, "height", DEFAULT_HEIGHT)?;
        let view = view_of(&widget)?;
        // "theme — A value of light or dark" (the metric widget structure reference); light is
        // the theme a graph is drawn on when the definition names none.
        let dark = string_member(&widget, WIDGET_MEMBER, "theme")? == Some("dark");

        let period = period_of(&widget, WIDGET_MEMBER)?;
        let (start, end) = window_of(&widget, period, now)?;

        let stat = string_member(&widget, WIDGET_MEMBER, "stat")?
            .unwrap_or(DEFAULT_STAT)
            .to_string();
        let (y_min, y_max) = left_axis_of(&widget)?;
        let annotations = annotations_of(&widget)?;

        let entries = match widget.get(METRICS_MEMBER).filter(|value| !value.is_null()) {
            None => return Err(super::parameter_required_message(&path(METRICS_MEMBER))),
            Some(value) => value.as_array().ok_or_else(|| {
                super::value_is_invalid_message(&value.to_string(), &path(METRICS_MEMBER))
            })?,
        };
        if entries.is_empty() {
            return Err(super::parameter_required_message(&path(METRICS_MEMBER)));
        }
        if entries.len() > MAX_WIDGET_METRICS {
            return Err(super::collection_too_large_message(
                &path(METRICS_MEMBER),
                MAX_WIDGET_METRICS,
            ));
        }

        let mut queries: Vec<MetricDataQuery> = Vec::with_capacity(entries.len());
        let mut plots: Vec<Plot> = Vec::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            let (query, plot) = read_entry(entry, index, period, &stat)?;
            queries.push(query);
            plots.push(plot);
        }
        // The ids of the array are judged by the one rule every `MetricDataQuery` array is
        // judged by ([`super::metric_query::query_id_refusal`]): an expression refers to the
        // other entries BY id, so an array using one id twice names a value nothing can
        // resolve. The generated ids are unique by construction, so only ids the definition
        // supplies itself can collide.
        if let Some(message) = metric_query::query_id_refusal(&queries, &path(METRICS_MEMBER)) {
            return Err(message);
        }

        Ok(Self {
            width,
            height,
            view,
            dark,
            start,
            end,
            period,
            y_min,
            y_max,
            annotations,
            queries,
            plots,
        })
    }
}

/// The path a refusal names a member of the definition by: the request member, then the
/// member inside the document.
fn path(member: &str) -> String {
    format!("{WIDGET_MEMBER}.{member}")
}

/// The string member `member` of the object at path `at` carries, `None` when it is left out,
/// or the refusal for a member that is not a string.
fn string_member<'a>(
    members: &'a Map<String, Value>,
    at: &str,
    member: &str,
) -> Result<Option<&'a str>, String> {
    match members.get(member) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.as_str())),
        Some(other) => Err(super::value_is_invalid_message(
            &other.to_string(),
            &format!("{at}.{member}"),
        )),
    }
}

/// The pixel size member `member` carries, else `default`.
///
/// A size is a whole number of pixels inside [`MAX_IMAGE_SIDE`]; anything else — a negative
/// number, a fraction, text, or a graph larger than this emulator renders — is refused rather
/// than clamped, so the caller is told which member to change.
fn pixels(widget: &Map<String, Value>, member: &str, default: u32) -> Result<u32, String> {
    let Some(value) = widget.get(member).filter(|value| !value.is_null()) else {
        return Ok(default);
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let pixels = value
        .as_f64()
        .filter(|pixels| pixels.is_finite() && pixels.fract() == 0.0)
        .filter(|pixels| (1.0..=f64::from(MAX_IMAGE_SIDE)).contains(pixels))
        .map(|pixels| pixels as u32);
    pixels.ok_or_else(|| super::value_is_invalid_message(&value.to_string(), &path(member)))
}

/// Which plot the definition's `view` asks for.
///
/// The two plots this renderer draws are the line through the series' points and the column
/// per window; the other documented views report a graph of the same resolved series, so they
/// are drawn as that line. A value outside the documented set is refused against the set
/// itself ([`VIEWS`]), which is what a caller repairs the definition from.
fn view_of(widget: &Map<String, Value>) -> Result<View, String> {
    match string_member(widget, WIDGET_MEMBER, "view")? {
        None | Some("timeSeries") => Ok(View::TimeSeries),
        Some("bar") => Ok(View::Bar),
        Some(named) if VIEWS.contains(&named) => Ok(View::TimeSeries),
        Some(_) => Err(super::value_in_set_message(
            &path("view"),
            &VIEWS.map(String::from),
        )),
    }
}

/// The granularity the `period` of `members` names, else [`DEFAULT_PERIOD`].
///
/// `at` is the path of the object the period is a member of — the definition itself, or one
/// `metrics` entry's options — so the refusal names the member that has to change. The value
/// is judged by the one rule every period of this service is judged by
/// ([`super::period_error`]).
fn period_of(members: &Map<String, Value>, at: &str) -> Result<i32, String> {
    let Some(value) = members.get("period").filter(|value| !value.is_null()) else {
        return Ok(DEFAULT_PERIOD);
    };
    let period = value
        .as_i64()
        .and_then(|period| i32::try_from(period).ok())
        .ok_or_else(|| {
            super::value_is_invalid_message(&value.to_string(), &format!("{at}.period"))
        })?;
    match super::period_error(period) {
        Some(message) => Err(message),
        None => Ok(period),
    }
}

/// The graph's window: the `[start, end)` the definition names, aligned to the period's
/// boundaries.
///
/// "start — The start of the time range to use for each widget on the dashboard. … You can
/// specify start without specifying end to specify a relative time range that ends with the
/// current time" (the metric widget structure reference), so each of the two is either an
/// absolute ISO 8601 instant or a SIGNED ISO 8601 duration offset from now, and a definition
/// naming neither is answered for the last [`DEFAULT_RANGE_SECS`].
///
/// The window is then widened to the period's own boundaries, the same boundaries every read
/// of a series folds into ([`super::rounded_start`]): a graph is of whole periods, which is
/// also what makes two calls inside one period draw the same graph of the same store.
fn window_of(widget: &Map<String, Value>, period: i32, now: f64) -> Result<(f64, f64), String> {
    let end = match string_member(widget, WIDGET_MEMBER, "end")? {
        None => now,
        Some(value) => instant_named_by(value, now)
            .ok_or_else(|| super::value_is_invalid_message(value, &path("end")))?,
    };
    let start = match string_member(widget, WIDGET_MEMBER, "start")? {
        None => end - DEFAULT_RANGE_SECS,
        Some(value) => instant_named_by(value, now)
            .ok_or_else(|| super::value_is_invalid_message(value, &path("start")))?,
    };
    if end <= start {
        return Err(format!(
            "The parameter {} must name an instant later than {}.",
            path("end"),
            path("start")
        ));
    }
    let span = f64::from(period);
    Ok(((start / span).floor() * span, (end / span).ceil() * span))
}

/// The instant `value` names: an absolute ISO 8601 instant, or a signed ISO 8601 duration as
/// an offset from `now`. `None` for text naming neither.
///
/// Both forms are read through the ONE reading this workspace has of each — the maintained
/// duration grammar (`service_common::iso8601_span`) and the shared zone reader
/// (`service_common::rfc3339::split_utc_offset`) with the modelled timestamp type's own parse
/// — so no second date implementation lives here.
fn instant_named_by(value: &str, now: f64) -> Option<f64> {
    let (sign, unsigned) = match value.strip_prefix('-') {
        Some(unsigned) => (-1.0, unsigned),
        None => (1.0, value.strip_prefix('+').unwrap_or(value)),
    };
    if let Some(span) = ::service_common::iso8601_span::iso8601_span(unsigned) {
        return Some(now + sign * span.as_secs_f64());
    }
    let (civil, offset_seconds) = ::service_common::rfc3339::split_utc_offset(value)?;
    let civil = ::aws_smithy_types::DateTime::from_str(
        &format!("{civil}Z"),
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    // The instant is the civil reading shifted BACK by the offset the text declares:
    // 10:00+02:00 is 08:00 UTC.
    #[allow(clippy::cast_precision_loss)]
    Some(civil.as_secs_f64() - offset_seconds as f64)
}

/// The bounds `yAxis.left` fixes, each `None` when the definition leaves it to the data.
///
/// "yAxis — … left and right each support min and max" (the metric widget structure
/// reference): an axis bound is a number, so anything else is refused for the member it
/// arrived on.
fn left_axis_of(widget: &Map<String, Value>) -> Result<(Option<f64>, Option<f64>), String> {
    let Some(axis) = widget.get("yAxis").filter(|value| !value.is_null()) else {
        return Ok((None, None));
    };
    let axis = axis
        .as_object()
        .ok_or_else(|| super::value_is_invalid_message(&axis.to_string(), &path("yAxis")))?;
    let Some(left) = axis.get("left").filter(|value| !value.is_null()) else {
        return Ok((None, None));
    };
    let left = left
        .as_object()
        .ok_or_else(|| super::value_is_invalid_message(&left.to_string(), &path("yAxis.left")))?;
    let bound = |member: &str| -> Result<Option<f64>, String> {
        match left.get(member).filter(|value| !value.is_null()) {
            None => Ok(None),
            Some(value) => value
                .as_f64()
                .filter(|number| number.is_finite())
                .map(Some)
                .ok_or_else(|| {
                    super::value_is_invalid_message(
                        &value.to_string(),
                        &path(&format!("yAxis.left.{member}")),
                    )
                }),
        }
    };
    Ok((bound("min")?, bound("max")?))
}

/// The values the definition's horizontal annotations are drawn at, in the order it lists
/// them.
///
/// "annotations — … horizontal is an array of horizontal annotations, each with a value" (the
/// metric widget structure reference). A vertical annotation marks an INSTANT rather than a
/// value, so it is not a line of the value axis and is ignored here.
fn annotations_of(widget: &Map<String, Value>) -> Result<Vec<f64>, String> {
    let Some(annotations) = widget.get("annotations").filter(|value| !value.is_null()) else {
        return Ok(Vec::new());
    };
    let annotations = annotations.as_object().ok_or_else(|| {
        super::value_is_invalid_message(&annotations.to_string(), &path("annotations"))
    })?;
    let Some(horizontal) = annotations
        .get("horizontal")
        .filter(|value| !value.is_null())
    else {
        return Ok(Vec::new());
    };
    let horizontal = horizontal.as_array().ok_or_else(|| {
        super::value_is_invalid_message(&horizontal.to_string(), &path("annotations.horizontal"))
    })?;
    let mut values = Vec::with_capacity(horizontal.len());
    for (index, annotation) in horizontal.iter().enumerate() {
        let at = super::list_element_path(&path("annotations.horizontal"), index + 1);
        let value = annotation
            .get("value")
            .and_then(Value::as_f64)
            .filter(|number| number.is_finite())
            .ok_or_else(|| super::parameter_required_message(&format!("{at}.value")))?;
        values.push(value);
    }
    Ok(values)
}

/// One `metrics` entry read as the query it names and the way it is drawn.
///
/// "metrics — … Each item in the array is itself an array that defines one metric or
/// expression" (the metric widget structure reference): an entry is the SHORTHAND for a
/// `MetricDataQuery`, written as the namespace, the metric name and the dimension name/value
/// pairs, optionally followed by an options object — or, for an expression, as that options
/// object alone. So this is the one place the shorthand becomes the modelled query the shared
/// resolution takes.
fn read_entry(
    entry: &Value,
    index: usize,
    widget_period: i32,
    widget_stat: &str,
) -> Result<(MetricDataQuery, Plot), String> {
    let at = super::list_element_path(&path(METRICS_MEMBER), index + 1);
    let elements = entry
        .as_array()
        .ok_or_else(|| super::value_is_invalid_message(&entry.to_string(), &at))?;
    // The options object is the entry's LAST element when it has one; everything before it
    // names the metric.
    let (named, options) = match elements.split_last() {
        Some((Value::Object(options), named)) => (named, options.clone()),
        _ => (&elements[..], Map::new()),
    };
    let mut names: Vec<&str> = Vec::with_capacity(named.len());
    for element in named {
        let name = element
            .as_str()
            .ok_or_else(|| super::value_is_invalid_message(&element.to_string(), &at))?;
        names.push(name);
    }

    let expression = string_member(&options, &at, "expression")?.map(String::from);
    // "visible — Set this to false to hide the metric from the graph", which is the same thing
    // the resolution calls returning a query's data.
    let visible = match options.get("visible").filter(|value| !value.is_null()) {
        None => true,
        Some(Value::Bool(visible)) => *visible,
        Some(other) => {
            return Err(super::value_is_invalid_message(
                &other.to_string(),
                &format!("{at}.visible"),
            ))
        }
    };
    // An entry naming no period of its own is folded at the definition's.
    let period = match options.get("period").filter(|value| !value.is_null()) {
        None => widget_period,
        Some(_) => period_of(&options, &at)?,
    };
    let label = string_member(&options, &at, "label")?.map(String::from);
    let color = match string_member(&options, &at, "color")? {
        None => PALETTE[index % PALETTE.len()],
        Some(text) => {
            rgb(text).ok_or_else(|| super::value_is_invalid_message(text, &format!("{at}.color")))?
        }
    };

    let id = match string_member(&options, &at, "id")? {
        Some(id) if super::is_metric_query_id(id) => id.to_string(),
        Some(_) => {
            return Err(super::value_pattern_message(
                &format!("{at}.id"),
                super::METRIC_QUERY_ID_PATTERN,
            ))
        }
        // The id a definition leaves out is generated from the entry's own position, which is
        // unique inside the array by construction and is the id an expression refers to the
        // entry by: `m` for a metric, `e` for an expression, as the console names them.
        None if expression.is_some() => format!("e{}", index + 1),
        None => format!("m{}", index + 1),
    };

    let metric_stat = if names.is_empty() {
        if expression.is_none() {
            return Err(super::parameter_required_message(&format!(
                "{at}.expression"
            )));
        }
        None
    } else {
        Some(metric_stat_of(&names, &at, period, &options, widget_stat)?)
    };

    Ok((
        MetricDataQuery {
            id: id.clone(),
            metric_stat,
            expression,
            label,
            return_data: Some(visible),
            // The granularity is on the query itself as well as on its `MetricStat`, because
            // an expression's constructed series is spaced at the query's own period.
            period: Some(period),
            account_id: None,
        },
        Plot {
            id,
            color,
            visible,
        },
    ))
}

/// The `MetricStat` the entry's names and options define.
///
/// "a metric is uniquely defined by a name, a namespace, and zero or more dimensions", so the
/// shorthand names the namespace, the metric name and then the dimensions as name/value pairs
/// — an entry naming half of one names no series at all.
fn metric_stat_of(
    names: &[&str],
    at: &str,
    period: i32,
    options: &Map<String, Value>,
    widget_stat: &str,
) -> Result<MetricStat, String> {
    let pairs = names.get(2..).unwrap_or_default();
    let (Some(namespace), Some(metric_name)) = (names.first(), names.get(1)) else {
        return Err(super::value_is_invalid_message(
            &Value::from(names.to_vec()).to_string(),
            at,
        ));
    };
    if !pairs.len().is_multiple_of(2) {
        return Err(super::value_is_invalid_message(
            &Value::from(names.to_vec()).to_string(),
            at,
        ));
    }
    let dimensions: Vec<Dimension> = pairs
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| Dimension {
            name: pair[0].to_string(),
            value: pair[1].to_string(),
        })
        .collect();
    if dimensions.len() > super::MAX_METRIC_DIMENSIONS {
        return Err(super::collection_too_large_message(
            at,
            super::MAX_METRIC_DIMENSIONS,
        ));
    }
    Ok(MetricStat {
        metric: Metric {
            namespace: Some((*namespace).to_string()),
            metric_name: Some((*metric_name).to_string()),
            dimensions: Some(dimensions),
        },
        period,
        // "stat — The statistic to use for this metric", else the definition's own `stat`,
        // else the default every graph of a series takes.
        stat: string_member(options, at, "stat")?
            .unwrap_or(widget_stat)
            .to_string(),
        unit: string_member(options, at, "unit")?.map(String::from),
    })
}

/// The colour an entry's `color` option names: "The six-digit HTML hex color code to be used
/// for this metric" (the metric widget structure reference), with or without the leading `#`.
///
/// The six digits are read by the maintained hex codec this crate already decodes its opaque
/// tokens with, so no second hex reading lives here.
fn rgb(text: &str) -> Option<[u8; 3]> {
    let digits = text.strip_prefix('#').unwrap_or(text);
    let decoded = ::hex::decode(digits).ok()?;
    <[u8; 3]>::try_from(decoded.as_slice()).ok()
}

/// Every series the widget draws, resolved over `store` at the instant `now`.
///
/// The resolution is the shared one ([`super::metric_query`]): each entry naming a metric is
/// folded out of the store by its statistic into the graph's period windows, and each entry
/// carrying an expression is evaluated over the values of the entries it refers to. So the
/// graph is drawn from the datapoints `PutMetricData` stored and from nothing else — a store
/// holding no datapoint in the window draws an empty graph rather than an invented one.
///
/// An entry the definition hides contributes no series to the graph while still resolving, so
/// an expression can read it.
pub fn plotted_series(
    store: &State,
    widget: &MetricWidget,
    now: f64,
) -> Result<Vec<PlottedSeries>, QueryRefusal> {
    if let Some(refused) =
        metric_query::resolution_refusal(&widget.queries, widget.start, widget.end)
    {
        return Err(refused);
    }

    let mut resolution = Resolution::default();
    for query in &widget.queries {
        let Some(stat) = query.metric_stat.as_ref() else {
            continue;
        };
        let series =
            metric_query::retrieved_series(store, query, stat, widget.start, widget.end, now)?;
        resolution.retrieve(query, series)?;
    }

    let parsed = metric_query::parse_queries(&widget.queries)?;
    let order = metric_query::evaluation_order(&widget.queries, &parsed)?;
    for id in order {
        let Some(tree) = parsed.get(id) else {
            continue;
        };
        let Some(query) = widget.queries.iter().find(|query| query.id == *id) else {
            continue;
        };
        let evaluated = resolution.evaluate(
            id,
            tree,
            &StoreSeriesSource {
                store,
                requested_start: widget.start,
                end: widget.end,
                now,
            },
            (widget.start, widget.end),
            metric_query::query_period(query),
        )?;
        resolution.record(id, evaluated);
    }

    let mut plotted: Vec<PlottedSeries> = Vec::with_capacity(widget.plots.len());
    for (plot, query) in widget.plots.iter().zip(&widget.queries) {
        if !plot.visible {
            continue;
        }
        let Some(value) = resolution.value(&plot.id) else {
            continue;
        };
        match value {
            MathValue::Series(series) => {
                plotted.push(drawn(plot, query.label.as_deref(), series, 0));
            }
            // A search answers with every series it matched, each drawn in its own colour.
            MathValue::Array(list) => {
                for (position, series) in list.iter().enumerate() {
                    plotted.push(drawn(plot, query.label.as_deref(), series, position));
                }
            }
            // A scalar holds across the whole window, which is how a graph draws one: a level
            // at that value from the start of the window to its end.
            MathValue::Scalar(number) => plotted.push(PlottedSeries {
                label: query.label.clone().unwrap_or_else(|| plot.id.clone()),
                color: plot.color,
                points: vec![(widget.start, *number), (widget.end, *number)],
            }),
        }
    }
    Ok(plotted)
}

/// One resolved series as the graph draws it: the label the entry asks for, else the one the
/// resolution reports, and the entry's colour — cycled from the palette for the series after
/// the first of one entry, so the series of a search are told apart.
fn drawn(
    plot: &Plot,
    label: Option<&str>,
    series: &MathSeries,
    position: usize,
) -> PlottedSeries {
    PlottedSeries {
        label: label.unwrap_or(series.label.as_str()).to_string(),
        color: if position == 0 {
            plot.color
        } else {
            PALETTE[position % PALETTE.len()]
        },
        points: series.points.clone(),
    }
}
