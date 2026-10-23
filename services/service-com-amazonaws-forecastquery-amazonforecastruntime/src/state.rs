// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-forecastquery-amazonforecastruntime service.
//!
//! # Where the state comes from
//!
//! `forecastquery` (Amazon Forecast Runtime) is a pure DATA-PLANE READ service: its two
//! operations retrieve predictions that the Amazon Forecast CONTROL plane produced.
//! Nothing in this crate creates a forecast — `CreateForecast` and
//! `CreateWhatIfForecast` live in `service-com-amazonaws-forecast-amazonforecast`, and the
//! ARN each one mints is exactly the ARN `QueryForecast` / `QueryWhatIfForecast` are
//! called with.
//!
//! So the maps below are written from OUTSIDE a query handler — by the control plane once
//! it is implemented (it reaches this store with a path dependency on this crate and the
//! same shared engine), or by a test that stands in for it. A query handler only reads
//! them: it never materialises a forecast for an ARN it has not been told about, because a
//! forecast invented on a read would be a fabricated prediction and would make the
//! documented `ResourceNotFoundException` unreachable.
//!
//! # The stored shape
//!
//! A forecast holds one [`StoredSeries`] per time series (one item, or one combination of
//! item and forecast dimensions), and each series holds the data points of each statistic
//! the forecast was created with. `Filters` selects the series; `StartDate`/`EndDate` clip
//! its points. [`StoredForecast::query`] is that whole projection, shared by both
//! operations so they cannot disagree about it.

use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

/// Per-service state for com-amazonaws-forecastquery-amazonforecastruntime. A handler reaches it through [`service_state`].
///
/// The two maps are deliberately separate: a what-if forecast ARN is not a forecast ARN,
/// so `QueryForecast` must not resolve one and `QueryWhatIfForecast` must not resolve the
/// other.
#[derive(Debug, Default)]
pub struct ComAmazonawsForecastqueryAmazonforecastruntimeState {
    /// Keyed by `ForecastArn`, as minted by the control plane's `CreateForecast`.
    pub forecasts: HashMap<String, StoredForecast>,
    /// Keyed by `WhatIfForecastArn`, as minted by the control plane's
    /// `CreateWhatIfForecast`. Each entry carries the base forecast it was derived from in
    /// [`StoredForecast::base_forecast_arn`].
    pub what_if_forecasts: HashMap<String, StoredForecast>,
}

impl ComAmazonawsForecastqueryAmazonforecastruntimeState {
    /// Record a forecast the control plane produced, keyed by its own ARN.
    pub fn put_forecast(&mut self, forecast: StoredForecast) {
        self.forecasts.insert(forecast.arn.clone(), forecast);
    }

    /// Record a what-if forecast the control plane produced, keyed by its own ARN.
    pub fn put_what_if_forecast(&mut self, forecast: StoredForecast) {
        self.what_if_forecasts.insert(forecast.arn.clone(), forecast);
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsForecastqueryAmazonforecastruntimeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsForecastqueryAmazonforecastruntimeState>)
}

/// The filter key that names the item itself, as the operation's doc has it: "the key is
/// either item_id (or the equivalent non-timestamp, non-target field) from the
/// TARGET_TIME_SERIES dataset, or one of the forecast dimensions". Always an acceptable
/// `Filters` key, whatever a particular forecast's series carry.
pub const ITEM_KEY: &str = "item_id";

/// The ARN namespace every Amazon Forecast resource lives in — the 3rd field of its ARN.
///
/// Both this runtime service and the Forecast control plane declare `arnNamespace:
/// forecast` (Smithy `service_info`), so it is the only namespace an ARN either query
/// operation can resolve, whatever the request's partition, region or account.
pub const ARN_NAMESPACE: &str = "forecast";

/// What an ARN must NAME for one of this service's operations to resolve it.
///
/// An Amazon Forecast ARN is
/// `arn:<partition>:forecast:<region>:<account>:<type>/<name…>`, and the resource TYPE
/// plus the number of name segments after it is what tells one Forecast resource from
/// another — the model's `^arn:([a-z\d-]+):forecast:.*:.*:.+$` pattern admits them all
/// equally, so the boundary validator cannot:
///
/// * a forecast is `forecast/<forecastName>` — ONE name, which is why `ForecastArn`
///   targets the model's `Arn` (max 256);
/// * a what-if forecast is `what-if-forecast/<whatIfAnalysisName>/<whatIfForecastName>`
///   — TWO names, because a what-if forecast's name is "unique within each what-if
///   analysis" (the control plane's `CreateWhatIfForecast.WhatIfForecastName` doc), so
///   its ARN carries the analysis it belongs to as well. That nesting is why
///   `WhatIfForecastArn` is the one Forecast ARN that targets the longer `LongArn`
///   (max 300) while even the what-if ANALYSIS's own ARN is an `Arn` (max 256).
///
/// An ARN that does not name a resource of the kind an operation queries cannot identify
/// one, however the store is filled, so it is invalid input (400) rather than a resource
/// that happens to be absent (404). The store's answer — does a resource of that kind
/// exist under this ARN — is what separates the 404 from a 200.
#[derive(Debug, Clone, Copy)]
pub struct ResourceKind {
    /// The request member the ARN arrives in, as the model spells it. Lower-camelled by
    /// [`ResourceKind::member_path`] to match a boundary refusal's dotted path.
    pub member: &'static str,
    /// The ARN's resource type: the segment before the first `/` of its resource field.
    /// Also how the rejection message names the kind of resource that was asked for.
    pub resource_type: &'static str,
    /// How many non-empty `/`-separated name segments follow that type.
    pub name_segments: usize,
}

/// The resource `QueryForecast`'s `ForecastArn` must name:
/// `arn:<partition>:forecast:<region>:<account>:forecast/<forecastName>`.
pub const FORECAST: ResourceKind = ResourceKind {
    member: "ForecastArn",
    resource_type: "forecast",
    name_segments: 1,
};

/// The resource `QueryWhatIfForecast`'s `WhatIfForecastArn` must name:
/// `arn:<partition>:forecast:<region>:<account>:what-if-forecast/<whatIfAnalysisName>/<whatIfForecastName>`.
pub const WHAT_IF_FORECAST: ResourceKind = ResourceKind {
    member: "WhatIfForecastArn",
    resource_type: "what-if-forecast",
    name_segments: 2,
};

/// Every resource kind one of this service's operations resolves an ARN member by — the
/// table a refusal consults to word an ARN it could not read (`crate::service`).
pub const RESOURCE_KINDS: [ResourceKind; 2] = [FORECAST, WHAT_IF_FORECAST];

impl ResourceKind {
    /// Whether `arn` NAMES a resource of this kind: the Forecast ARN namespace, this
    /// resource type, and this type's name depth with no empty name.
    ///
    /// Says nothing about whether such a resource exists — that is the store's answer.
    /// Nothing here judges a partition, a region, an account or a name's spelling: those
    /// are values the control plane chose, and an ARN carrying ones this emulator never
    /// minted is simply a resource it does not hold.
    pub fn names(&self, arn: &str) -> bool {
        // `splitn(6, ..)` so a `:` inside the resource field stays part of it, exactly as
        // an ARN is read.
        let fields: Vec<&str> = arn.splitn(6, ':').collect();
        let [prefix, partition, namespace, _region, _account, resource] = fields.as_slice()
        else {
            return false;
        };
        if *prefix != "arn" || partition.is_empty() || *namespace != ARN_NAMESPACE {
            return false;
        }
        let mut segments = resource.split('/');
        if segments.next() != Some(self.resource_type) {
            return false;
        }
        let names: Vec<&str> = segments.collect();
        names.len() == self.name_segments && names.iter().all(|name| !name.is_empty())
    }

    /// The `Message` an ARN this service cannot read as one of this kind is refused with:
    /// `Invalid ARN for <resource type>: <the value supplied>`.
    ///
    /// ONE rule for every way the value fails — outside the member's modelled `@pattern`
    /// or `@length`, naming another kind of resource, or carrying the wrong number of name
    /// segments — because nothing in the answer distinguishes them: Amazon Forecast
    /// reports an ARN it cannot resolve to a resource of the queried kind by quoting the
    /// value back after the kind that was expected, and says nothing about the ARN's
    /// internal form. It is composed from the request, so a caller sees exactly what it
    /// sent.
    pub fn invalid_arn_message(&self, arn: &str) -> String {
        format!("Invalid ARN for {}: {}", self.resource_type, arn)
    }

    /// The dotted member path a boundary refusal names this ARN member under.
    ///
    /// Spelled by the framework's OWN path writer (lowerCamel), so this service's table
    /// and the validating walk that produced the violation cannot drift into two
    /// spellings of one position.
    pub fn member_path(&self) -> String {
        aws_smithy_schema_server::constraints::path::member(self.member)
    }
}

/// One `{Timestamp, Value}` of a stored time series.
///
/// `timestamp` uses the format the model documents for every date in this service:
/// `yyyy-MM-dd'T'HH:mm:ss`. A stored point whose timestamp does not parse in that format
/// is not a data point this service can place on a timeline, so the projection skips it
/// rather than returning an uncomparable value.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredPoint {
    pub timestamp: String,
    pub value: f64,
}

impl StoredPoint {
    pub fn new(timestamp: impl Into<String>, value: f64) -> Self {
        Self {
            timestamp: timestamp.into(),
            value,
        }
    }
}

/// One time series of a forecast: the dimension values that identify it, and its points
/// per statistic.
#[derive(Debug, Clone, Default)]
pub struct StoredSeries {
    /// `item_id` plus any forecast dimension — exactly what `Filters` matches on.
    pub dimensions: BTreeMap<String, String>,
    /// Statistic (`p10`, `p50`, `mean`, …) -> that statistic's points, ascending.
    pub points: BTreeMap<String, Vec<StoredPoint>>,
}

impl StoredSeries {
    /// A series identified by `dimensions` (key/value pairs, e.g. `item_id=client_21`).
    pub fn new<K: Into<String>, V: Into<String>>(
        dimensions: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        Self {
            dimensions: dimensions
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
            points: BTreeMap::new(),
        }
    }

    /// Attach `points` for one statistic, replacing any already stored for it.
    #[must_use]
    pub fn with_statistic(
        mut self,
        statistic: impl Into<String>,
        points: impl IntoIterator<Item = StoredPoint>,
    ) -> Self {
        self.points
            .insert(statistic.into(), points.into_iter().collect());
        self
    }

    /// Does this series carry every key/value pair in `filters`?
    ///
    /// A superset match: a forecast dimension the caller did not filter on does not
    /// disqualify the series.
    fn matches(&self, filters: &HashMap<String, String>) -> bool {
        filters
            .iter()
            .all(|(key, value)| self.dimensions.get(key).is_some_and(|stored| stored == value))
    }
}

/// A forecast (or what-if forecast) as the control plane produced it.
#[derive(Debug, Clone)]
pub struct StoredForecast {
    /// The ARN the control plane minted, and the ARN a query names.
    pub arn: String,
    /// The forecast's own name, as `CreateForecast` was given it. Part of the resource's
    /// identity rather than of a query's answer: neither query operation reports it (the
    /// modelled `Forecast` carries only `Predictions`), so it is stored for the control
    /// plane's readers, not read here.
    pub name: String,
    /// The forecast types the forecast was CREATED with, in the order they were given:
    /// quantiles (`"0.1"`) and/or `"mean"`. Projected to response statistics by
    /// [`statistic_for_forecast_type`].
    pub forecast_types: Vec<String>,
    /// The data frequency the predictor used (`"D"`, `"H"`, …) — reported by the control
    /// plane, kept here because it is part of the forecast's identity; like `name` and
    /// `created_at` it is not a member of the modelled query response.
    pub frequency: String,
    /// The time series this forecast holds — the one thing a query projects.
    pub series: Vec<StoredSeries>,
    /// When the control plane created the forecast. Stored for the control plane's own
    /// readers (`DescribeForecast`), not reported by a query.
    pub created_at: SystemTime,
    /// For a what-if forecast, the forecast it was derived from; `None` for a base
    /// forecast. The what-if's own series is what a query answers with, so this is the
    /// provenance link the control plane records, not an instruction to read the base.
    pub base_forecast_arn: Option<String>,
}

impl StoredForecast {
    /// A forecast with no series yet. `forecast_types` is what `CreateForecast` was given.
    pub fn new<T: Into<String>>(
        arn: impl Into<String>,
        name: impl Into<String>,
        frequency: impl Into<String>,
        forecast_types: impl IntoIterator<Item = T>,
    ) -> Self {
        Self {
            arn: arn.into(),
            name: name.into(),
            forecast_types: forecast_types.into_iter().map(Into::into).collect(),
            frequency: frequency.into(),
            series: Vec::new(),
            created_at: SystemTime::now(),
            base_forecast_arn: None,
        }
    }

    /// Add one time series.
    #[must_use]
    pub fn with_series(mut self, series: StoredSeries) -> Self {
        self.series.push(series);
        self
    }

    /// Mark this forecast as derived from `base`, as a what-if forecast is.
    #[must_use]
    pub fn derived_from(mut self, base: impl Into<String>) -> Self {
        self.base_forecast_arn = Some(base.into());
        self
    }

    /// The statistics this forecast reports, in creation order: one per forecast type.
    ///
    /// Deduplicated, because `Predictions` is a map KEYED by statistic: two forecast types
    /// that name the same statistic are one reported series, never two entries one of which
    /// silently overwrites the other.
    pub fn statistics(&self) -> Vec<String> {
        let mut statistics: Vec<String> = Vec::with_capacity(self.forecast_types.len());
        for forecast_type in &self.forecast_types {
            let statistic = statistic_for_forecast_type(forecast_type);
            if !statistics.contains(&statistic) {
                statistics.push(statistic);
            }
        }
        statistics
    }

    /// Every dimension name any of this forecast's series carries, sorted, plus the item
    /// key itself.
    ///
    /// These are the only keys `Filters` may name: the model documents a filter key as
    /// "either item_id … or one of the forecast dimensions". `item_id` is therefore always
    /// a valid KEY — a value of it that matches nothing is "we can't find that resource",
    /// not an invalid filter.
    pub fn dimension_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .series
            .iter()
            .flat_map(|series| series.dimensions.keys().cloned())
            .chain(std::iter::once(ITEM_KEY.to_string()))
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Project this forecast into the `Predictions` a query answers with: the statistic of
    /// each forecast type, mapped to its stored points clipped to `[start, end]`
    /// inclusive and ordered ascending by timestamp.
    ///
    /// Pure: it reads the stored forecast and writes nothing, so two identical queries
    /// answer identically and a narrower window answers a subset of a wider one.
    pub fn query(
        &self,
        filters: &HashMap<String, String>,
        start: Option<&str>,
        end: Option<&str>,
    ) -> Result<Vec<(String, Vec<StoredPoint>)>, QueryRejection> {
        let window = DateWindow::parse(start, end)?;

        // `Filters` is required with 1..50 entries, so the boundary validator refuses an
        // empty map before a handler runs. Guarded here as well because an empty filter
        // set would otherwise match EVERY series vacuously, which is the one input for
        // which "retrieves a forecast for a single item" has no answer.
        if filters.is_empty() {
            return Err(QueryRejection::NoFilters);
        }

        let known = self.dimension_names();
        if let Some(unknown) = filters
            .keys()
            .find(|key| !known.iter().any(|name| name == *key))
        {
            return Err(QueryRejection::UnknownDimension {
                key: unknown.clone(),
                known,
            });
        }

        let matched: Vec<&StoredSeries> = self
            .series
            .iter()
            .filter(|series| series.matches(filters))
            .collect();
        let series = match matched.as_slice() {
            [one] => *one,
            [] => return Err(QueryRejection::NoMatchingSeries),
            many => {
                return Err(QueryRejection::AmbiguousFilters {
                    matched: many.len(),
                })
            }
        };

        Ok(self
            .statistics()
            .into_iter()
            .map(|statistic| {
                let points = series
                    .points
                    .get(&statistic)
                    .map(|points| window.clip(points))
                    .unwrap_or_default();
                (statistic, points)
            })
            .collect())
    }
}

/// The stored points of one statistic, as the modelled `TimeSeries` of `DataPoint`s.
///
/// Lives beside the stored shape rather than in either handler so both operations project
/// a point the same way.
pub fn data_points(points: &[StoredPoint]) -> Vec<crate::ops::types::DataPoint> {
    points
        .iter()
        .map(|point| crate::ops::types::DataPoint {
            timestamp: Some(point.timestamp.clone()),
            value: Some(point.value),
        })
        .collect()
}

/// The statistic a forecast type is reported as in `Forecast.Predictions`.
///
/// Amazon Forecast accepts a forecast type as a quantile between `0.01` and `0.99` in
/// increments of `.01`, or `mean` (CreateForecast's `ForecastTypes` doc), and reports a
/// quantile as `p` followed by that quantile in percent — `0.1` -> `p10`, `0.5` -> `p50`,
/// `0.01` -> `p1` — which is also why the model bounds `Statistic` at 4 characters.
/// A value that already names a statistic (`p10`, `mean`) is reported unchanged, and so is
/// anything else: `CreateForecast` is what validates `ForecastTypes`, so a forecast stored
/// here was created with a type that service accepted, and this projection does not get to
/// re-judge it.
pub fn statistic_for_forecast_type(forecast_type: &str) -> String {
    match forecast_type.trim().parse::<f64>() {
        // A quantile in the documented open range; percent is an integer because the
        // increment is .01. `round` rather than a cast, so 0.07 (0.0700000000000000066…
        // in binary) is 7 and not 6.
        Ok(quantile) if quantile > 0.0 && quantile < 1.0 => {
            format!("p{}", (quantile * 100.0).round() as u32)
        }
        // "mean", "p10", or anything else the forecast was created with: already a
        // statistic name.
        _ => forecast_type.to_string(),
    }
}

/// The instant a `yyyy-MM-dd'T'HH:mm:ss` date names, as seconds on one scale, or `None`
/// when `value` is not in that format.
///
/// That format is the one the model documents for `StartDate`, `EndDate` and a
/// `DataPoint`'s `Timestamp`. It is checked in two parts, neither of them hand-rolled
/// arithmetic:
///
/// 1. the LAYOUT must be exactly the documented one, so a value carrying a zone or offset,
///    a space instead of `T`, fractional seconds, or a missing field is refused;
/// 2. the CALENDAR is then checked by the AWS date type this crate already depends on
///    (`aws_smithy_types::DateTime`): RFC 3339 is this very layout plus a zone designator,
///    so appending the UTC one hands that parser exactly what it expects, and it is what
///    rejects a 30th of February or an hour past 23.
///
/// Amazon Forecast's predictions are all in the dataset's own timezone (the operation's
/// doc), so every date in a request and in the stored series is on one scale and comparing
/// the parsed seconds is what a clip needs.
pub fn parse_date_time(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.len() != 19 {
        return None;
    }
    for (index, byte) in bytes.iter().enumerate() {
        let separator = match index {
            4 | 7 => Some(b'-'),
            10 => Some(b'T'),
            13 | 16 => Some(b':'),
            _ => None,
        };
        match separator {
            Some(expected) if *byte != expected => return None,
            None if !byte.is_ascii_digit() => return None,
            _ => {}
        }
    }

    aws_smithy_types::DateTime::from_str(
        &format!("{value}Z"),
        aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()
    .map(|parsed| parsed.secs())
}

/// The inclusive `[StartDate, EndDate]` window a query asked for; either bound may be
/// absent, and with both absent the complete date range is returned (the model's
/// documented default).
#[derive(Debug, Clone, Copy, Default)]
struct DateWindow {
    start: Option<i64>,
    end: Option<i64>,
}

impl DateWindow {
    fn parse(start: Option<&str>, end: Option<&str>) -> Result<Self, QueryRejection> {
        let parse_member = |member: &'static str, value: Option<&str>| match value {
            None => Ok(None),
            Some(raw) => match parse_date_time(raw) {
                Some(parsed) => Ok(Some(parsed)),
                None => Err(QueryRejection::MalformedDate {
                    member,
                    value: raw.to_string(),
                }),
            },
        };

        let window = Self {
            start: parse_member("StartDate", start)?,
            end: parse_member("EndDate", end)?,
        };
        if let (Some(from), Some(to)) = (window.start, window.end) {
            if from > to {
                return Err(QueryRejection::InvertedRange);
            }
        }
        Ok(window)
    }

    /// `points` restricted to this window, ascending by timestamp.
    fn clip(&self, points: &[StoredPoint]) -> Vec<StoredPoint> {
        let mut kept: Vec<(i64, StoredPoint)> = points
            .iter()
            .filter_map(|point| {
                let at = parse_date_time(&point.timestamp)?;
                let inside = self.start.is_none_or(|from| at >= from)
                    && self.end.is_none_or(|to| at <= to);
                inside.then(|| (at, point.clone()))
            })
            .collect();
        kept.sort_by_key(|(at, _)| *at);
        kept.into_iter().map(|(_, point)| point).collect()
    }
}

/// Why a query cannot be answered from a stored forecast.
///
/// The handlers map each case onto the operation's own modelled error, so the mapping is
/// stated once here and rendered per operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryRejection {
    /// A `Filters` key that is not `item_id` and not one of the forecast's dimensions.
    UnknownDimension { key: String, known: Vec<String> },
    /// `Filters` carried no criteria at all, so it names no time series.
    NoFilters,
    /// `Filters` matched none of the forecast's time series.
    NoMatchingSeries,
    /// `Filters` matched more than one series, so it does not name the single item a
    /// query retrieves.
    AmbiguousFilters { matched: usize },
    /// A date member that is not `yyyy-MM-dd'T'HH:mm:ss`.
    MalformedDate {
        member: &'static str,
        value: String,
    },
    /// `StartDate` is after `EndDate`, so the window is empty by construction.
    InvertedRange,
}

impl QueryRejection {
    /// The `Message` to report, built from the request so a caller can fix it.
    pub fn message(&self) -> String {
        match self {
            Self::UnknownDimension { key, known } => format!(
                "Filters key '{key}' is not a dimension of this forecast; \
                 valid keys are [{}]",
                known.join(", ")
            ),
            Self::NoFilters => {
                "Filters must carry at least one criterion naming the time series to \
                 retrieve, for example {\"item_id\": \"<item>\"}"
                    .to_string()
            }
            Self::NoMatchingSeries => {
                "We can't find that resource. No time series of this forecast matches the \
                 supplied Filters."
                    .to_string()
            }
            Self::AmbiguousFilters { matched } => format!(
                "Filters match {matched} time series; a forecast is retrieved for a single \
                 item, so supply a filter for every forecast dimension"
            ),
            Self::MalformedDate { member, value } => format!(
                "{member} '{value}' is not a valid date; specify the date using this format: \
                 yyyy-MM-dd'T'HH:mm:ss (ISO 8601 format)"
            ),
            Self::InvertedRange => {
                "StartDate must not be later than EndDate".to_string()
            }
        }
    }

    /// Is this a "the thing you asked for is not there" rejection (404) rather than a
    /// "your request is wrong" one (400)?
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NoMatchingSeries)
    }
}
