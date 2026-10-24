// SPDX-License-Identifier: CC0-1.0

//! The DATA behind a forecast: the import file a dataset import job reads, the
//! frequency it is bucketed to, the estimator a predictor is fitted with, and the
//! horizon a forecast steps forward.
//!
//! # What is real here, and what Amazon Forecast does instead
//!
//! Amazon Forecast trains learned models (CNN-QR, DeepAR+, Prophet, ARIMA, NPTS, ETS)
//! on the imported history and runs inference with the winning one. This emulator
//! cannot run those models, and it never invents prediction values: everything below
//! is COMPUTED from the rows a `CreateDatasetImportJob` actually read out of object
//! storage.
//!
//! The estimator it fits instead is stated once, here, so every operation reports the
//! same thing:
//!
//! * the history of one time series is bucketed to the predictor's
//!   `ForecastFrequency`, summing the target values that fall in a bucket — the
//!   aggregation Forecast documents as its default for a TARGET_TIME_SERIES ("the
//!   default is `sum`", `AttributeConfig$Transformations`);
//! * the fitted model is the EMPIRICAL DISTRIBUTION of those bucket values: a
//!   quantile forecast type (`0.1`, `0.5`, …) is that distribution's quantile by
//!   linear interpolation between order statistics, and `mean` is their arithmetic
//!   mean;
//! * a forecast holds the predictor's `ForecastHorizon` points per statistic, at the
//!   buckets that follow the last observed one, each carrying that statistic's fitted
//!   value. The baseline is stationary, so the horizon repeats the estimate rather
//!   than inventing a trend the data does not show.
//!
//! Deterministic, bounded and derived from persisted rows: two forecasts of the same
//! imported data agree, and no forecast exists for data nothing imported.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Days, Months, NaiveDate, NaiveDateTime, TimeDelta, Timelike, Utc};

use crate::ops::types::{
    Schema, Statistics, TimeSeriesCondition, TimeSeriesIdentifiers, TimeSeriesTransformation,
};

/// The most objects one import job reads, and the most bytes it reads from one object
/// and in total.
///
/// Amazon Forecast publishes its own per-dataset quotas; these are the EMULATOR's
/// bounds on the work one request does, so no submission can make this process read
/// an unbounded amount of object storage. An import that points past them is not
/// silently truncated: the job ends in the modelled `CREATE_FAILED` state naming the
/// bound it hit.
pub const MAX_IMPORT_OBJECTS: usize = 50;
/// The most bytes read from one import object (see [`MAX_IMPORT_OBJECTS`]).
pub const MAX_IMPORT_OBJECT_BYTES: usize = 1024 * 1024;
/// The most bytes read across one import (see [`MAX_IMPORT_OBJECTS`]).
pub const MAX_IMPORT_TOTAL_BYTES: usize = 8 * 1024 * 1024;
/// The most rows one import stores (see [`MAX_IMPORT_OBJECTS`]).
pub const MAX_IMPORT_ROWS: usize = 100_000;
/// The most distinct time series one dataset retains (see [`MAX_IMPORT_OBJECTS`]).
pub const MAX_SERIES_PER_DATASET: usize = 1_000;
/// The most observations one time series retains (see [`MAX_IMPORT_OBJECTS`]).
pub const MAX_POINTS_PER_SERIES: usize = 20_000;
/// The most buckets one series' history is aggregated into (see
/// [`MAX_IMPORT_OBJECTS`]). A bounded history can still span an unbounded number of
/// buckets — one row a century apart at a one-minute frequency — so the bucketing
/// walk is bounded too.
pub const MAX_BUCKETS_PER_SERIES: usize = 20_000;

/// The most bucket steps ONE predictor's whole fitting run may take, across every
/// time series it is fitted to.
///
/// [`MAX_BUCKETS_PER_SERIES`] bounds one series, but a predictor fits up to
/// [`MAX_SERIES_PER_DATASET`] of them in a single request, and that walk runs with the
/// store lock held, so the PRODUCT is bounded here as well: the fitting run is a few
/// hundred thousand cheap calendar steps at worst (milliseconds), not twenty million.
/// A predictor whose training data would take more is refused rather than fitted.
pub const MAX_BUCKETS_PER_FIT: usize = 200_000;

/// The most PREDICTION POINTS one forecast materialises, across every time series and
/// statistic it holds.
///
/// Each factor of that product is bounded on its own — the time series a dataset
/// retains ([`MAX_SERIES_PER_DATASET`]), the forecast types a forecast may name
/// ([`crate::state::MAX_FORECAST_TYPES`]) and the horizon a predictor may be created
/// with ([`crate::state::MAX_FORECAST_HORIZON`]) — but it is the PRODUCT that one
/// request allocates and the runtime store keeps, and 1,000 x 20 x 500 points is far
/// more than a developer host should hold from one small request. Bounded here, and
/// checked before a single point is built, so a create past it is refused with the
/// modelled `LimitExceededException` instead of allocating
/// ([`crate::state::forecast_points_rejection`]). CHOSEN for the emulator: it still
/// admits, for instance, 20 time series at 5 statistics over a 500-step horizon.
pub const MAX_FORECAST_POINTS: usize = 50_000;

/// The most prediction points this emulator retains across EVERY forecast and what-if
/// forecast of an account.
///
/// [`MAX_FORECAST_POINTS`] bounds one forecast and
/// [`crate::state::MAX_FORECASTS`] bounds how many there are, but their product is
/// again what the store would hold, so the retained total is bounded too and a create
/// past it is refused the same way.
pub const MAX_RETAINED_FORECAST_POINTS: usize = 500_000;

/// The timestamp format an import file carries when the request names none and the
/// data is collected daily or less often, from `CreateDatasetImportJob`'s
/// `TimestampFormat` documentation: "yyyy-MM-dd (Optional, if DataFrequency is Y, M,
/// W, or D)".
pub const DATE_FORMAT: &str = "yyyy-MM-dd";

/// The timestamp format an import file carries when the request names none and the
/// data is collected more often than daily, from the same documentation:
/// "yyyy-MM-dd HH:mm:ss (Optional, if DataFrequency is H, 30min, 15min, 10min, 5min,
/// or 1min; and optionally, for: Y, M, W, and D)".
pub const DATE_TIME_FORMAT: &str = "yyyy-MM-dd HH:mm:ss";

/// The format every date in a forecast's `Predictions` carries, as the runtime
/// service's `DataPoint$Timestamp` documents it: `yyyy-MM-dd'T'HH:mm:ss`.
const PREDICTION_TIMESTAMP: &str = "%Y-%m-%dT%H:%M:%S";

/// The `strftime` pattern one of the two documented `TimestampFormat` values is read
/// with, or `None` for a value Forecast does not document.
fn timestamp_pattern(format: &str) -> Option<&'static str> {
    match format {
        DATE_FORMAT => Some("%Y-%m-%d"),
        DATE_TIME_FORMAT => Some("%Y-%m-%d %H:%M:%S"),
        _ => None,
    }
}

/// The unit of a `Frequency` — the calendar step its count multiplies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrequencyUnit {
    Year,
    Month,
    Week,
    Day,
    Hour,
    Minute,
}

/// A data or forecast frequency: a count of a unit, as the modelled `Frequency`
/// pattern `^Y|M|W|D|H|30min|15min|10min|5min|1min$` and its documented multiples
/// spell it ("Valid intervals are an integer followed by Y (Year), M (Month), W
/// (Week), D (Day), H (Hour), and min (Minute) … Thus, if you want every other week
/// forecasts, specify 2W").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frequency {
    pub count: u32,
    pub unit: FrequencyUnit,
}

impl Frequency {
    /// Read a frequency, or `None` for a value that is not an optional count followed
    /// by one of the documented units.
    pub fn parse(value: &str) -> Option<Self> {
        let (digits, unit) = match value.strip_suffix("min") {
            Some(digits) => (digits, FrequencyUnit::Minute),
            None => {
                let mut characters = value.chars();
                let unit = match characters.next_back()? {
                    'Y' => FrequencyUnit::Year,
                    'M' => FrequencyUnit::Month,
                    'W' => FrequencyUnit::Week,
                    'D' => FrequencyUnit::Day,
                    'H' => FrequencyUnit::Hour,
                    _ => return None,
                };
                (characters.as_str(), unit)
            }
        };
        let count = if digits.is_empty() {
            1
        } else {
            digits.parse::<u32>().ok()?
        };
        (count > 0).then_some(Self { count, unit })
    }

    /// Whether this frequency is finer than a day.
    pub fn is_sub_daily(&self) -> bool {
        matches!(self.unit, FrequencyUnit::Hour | FrequencyUnit::Minute)
    }

    /// The start of the bucket `at` falls in when bucketing starts from it: the
    /// instant truncated to this frequency's UNIT, so buckets sit on calendar
    /// boundaries (a weekly bucket starts on a Monday, a monthly one on the 1st)
    /// rather than on the first row's wall clock.
    ///
    /// The truncation is to the unit, NOT to a grid of `count` units: a `2W` history
    /// starts on the Monday of its first observation's week and then steps two weeks at
    /// a time from there, rather than onto some absolute fortnight grid. Deterministic,
    /// and the same for every operation, because the bucketing walk in [`fit`] is the
    /// only caller.
    pub fn floor(&self, at: NaiveDateTime) -> Option<NaiveDateTime> {
        let date = at.date();
        let time = at.time();
        match self.unit {
            FrequencyUnit::Year => {
                NaiveDate::from_ymd_opt(date.year(), 1, 1)?.and_hms_opt(0, 0, 0)
            }
            FrequencyUnit::Month => {
                NaiveDate::from_ymd_opt(date.year(), date.month(), 1)?.and_hms_opt(0, 0, 0)
            }
            FrequencyUnit::Week => {
                let weekday = u64::from(date.weekday().num_days_from_monday());
                date.checked_sub_days(Days::new(weekday))?.and_hms_opt(0, 0, 0)
            }
            FrequencyUnit::Day => date.and_hms_opt(0, 0, 0),
            FrequencyUnit::Hour => date.and_hms_opt(time.hour(), 0, 0),
            FrequencyUnit::Minute => date.and_hms_opt(time.hour(), time.minute(), 0),
        }
    }

    /// One step forward from `at`, or `None` when the step leaves the representable
    /// calendar.
    pub fn advance(&self, at: NaiveDateTime) -> Option<NaiveDateTime> {
        let count = self.count;
        match self.unit {
            FrequencyUnit::Year => at.checked_add_months(Months::new(count.checked_mul(12)?)),
            FrequencyUnit::Month => at.checked_add_months(Months::new(count)),
            FrequencyUnit::Week => at.checked_add_days(Days::new(u64::from(count) * 7)),
            FrequencyUnit::Day => at.checked_add_days(Days::new(u64::from(count))),
            FrequencyUnit::Hour => at.checked_add_signed(TimeDelta::try_hours(i64::from(count))?),
            FrequencyUnit::Minute => {
                at.checked_add_signed(TimeDelta::try_minutes(i64::from(count))?)
            }
        }
    }
}

/// One observation of a time series: an instant on the UTC scale and the target value
/// recorded at it.
///
/// Forecast's predictions are in the dataset's own time zone (the operation's
/// documentation), so every timestamp in this crate is read and reported on one scale
/// and the stored seconds are directly comparable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observation {
    /// Seconds since the Unix epoch.
    pub at: i64,
    pub value: f64,
}

/// One imported time series: the dimension values that identify it (`item_id` plus any
/// forecast dimension) and its observations, ascending by instant.
#[derive(Debug, Clone, Default)]
pub struct ImportedSeries {
    pub dimensions: BTreeMap<String, String>,
    pub observations: Vec<Observation>,
}

/// The whole result of reading an import file: the series it holds, the per-field
/// statistics of the rows it carried, and how much was read.
#[derive(Debug, Clone, Default)]
pub struct ImportedData {
    pub series: Vec<ImportedSeries>,
    pub statistics: BTreeMap<String, Statistics>,
    pub rows: usize,
    pub bytes: usize,
}

/// Which CSV column carries what, read from the dataset's own `Schema`.
///
/// "The schema attributes and their order must match the fields in your data"
/// (`CreateDataset$Schema`), so the schema IS the column layout. Which column is which
/// is decided by the attribute's TYPE, not by its name: the name of a
/// TARGET_TIME_SERIES target field depends on the dataset's `Domain` (`demand` for
/// RETAIL, `target_value` for CUSTOM, `workforce_demand` for WORK_FORCE, …), while its
/// type is always numeric.
#[derive(Debug, Clone)]
pub struct ColumnLayout {
    /// Every attribute's name, in column order.
    pub names: Vec<String>,
    /// Every attribute's `AttributeType`, in column order.
    pub kinds: Vec<String>,
    /// The column of the single `timestamp` attribute.
    pub timestamp: usize,
    /// The first numeric column: the target value.
    pub target: usize,
    /// The `string`/`geolocation` columns: `item_id` and the forecast dimensions.
    pub dimensions: Vec<usize>,
}

/// The numeric `AttributeType`s a target value may be declared as
/// (`smithy_query(query="shape",shape="AttributeType")` lists float, geolocation,
/// integer, string, timestamp).
const NUMERIC_KINDS: [&str; 2] = ["float", "integer"];

/// The `AttributeType`s that identify a time series rather than measure it.
const DIMENSION_KINDS: [&str; 2] = ["string", "geolocation"];

impl ColumnLayout {
    /// The layout `schema` describes, or the reason it describes no time series this
    /// emulator can read rows of.
    pub fn read(schema: &Schema) -> Result<Self, String> {
        let attributes = schema.attributes.as_deref().unwrap_or_default();
        let mut names = Vec::with_capacity(attributes.len());
        let mut kinds = Vec::with_capacity(attributes.len());
        for attribute in attributes {
            names.push(attribute.attribute_name.clone().unwrap_or_default());
            kinds.push(attribute.attribute_type.clone().unwrap_or_default());
        }
        let timestamp = kinds
            .iter()
            .position(|kind| kind == "timestamp")
            .ok_or_else(|| {
                "The dataset's schema declares no timestamp attribute, so its import \
                 files hold no time series."
                    .to_string()
            })?;
        let target = kinds
            .iter()
            .position(|kind| NUMERIC_KINDS.contains(&kind.as_str()))
            .ok_or_else(|| {
                "The dataset's schema declares no float or integer attribute, so its \
                 import files carry no target values."
                    .to_string()
            })?;
        let dimensions: Vec<usize> = kinds
            .iter()
            .enumerate()
            .filter(|(_, kind)| DIMENSION_KINDS.contains(&kind.as_str()))
            .map(|(column, _)| column)
            .collect();
        if dimensions.is_empty() {
            return Err("The dataset's schema declares no string attribute, so its \
                        import files identify no item."
                .to_string());
        }
        Ok(Self {
            names,
            kinds,
            timestamp,
            target,
            dimensions,
        })
    }

    /// The dimension names a series of this layout is identified by, in column order.
    pub fn dimension_names(&self) -> Vec<String> {
        self.dimensions
            .iter()
            .filter_map(|column| self.names.get(*column).cloned())
            .collect()
    }
}

/// A field's statistics as they accumulate over the rows of an import.
#[derive(Debug, Default)]
struct FieldAccumulator {
    count: i64,
    nulls: i64,
    /// Values of a numeric field that are not numbers at all — counted, not assumed.
    not_numbers: i64,
    distinct: std::collections::BTreeSet<String>,
    numeric: bool,
    numbers: Vec<f64>,
    min_text: Option<String>,
    max_text: Option<String>,
}

impl FieldAccumulator {
    fn observe(&mut self, raw: &str) {
        if raw.is_empty() {
            self.nulls += 1;
            return;
        }
        self.count += 1;
        if self.distinct.len() < MAX_IMPORT_ROWS {
            self.distinct.insert(raw.to_string());
        }
        if self.numeric {
            match raw.parse::<f64>() {
                Ok(number) => {
                    if self.numbers.len() < MAX_IMPORT_ROWS {
                        self.numbers.push(number);
                    }
                }
                // A value of a numeric field that is not a number: what `CountNaN`
                // counts.
                Err(_) => self.not_numbers += 1,
            }
        }
        if self.min_text.as_deref().is_none_or(|current| raw < current) {
            self.min_text = Some(raw.to_string());
        }
        if self.max_text.as_deref().is_none_or(|current| raw > current) {
            self.max_text = Some(raw.to_string());
        }
    }

    /// The modelled `Statistics` of this field: counts and extremes always, and the
    /// average and standard deviation only for a numeric field, which is the only one
    /// they are defined for.
    fn statistics(self) -> Statistics {
        let (min, max) = if self.numeric && !self.numbers.is_empty() {
            let min = self.numbers.iter().copied().fold(f64::INFINITY, f64::min);
            let max = self
                .numbers
                .iter()
                .copied()
                .fold(f64::NEG_INFINITY, f64::max);
            (Some(min.to_string()), Some(max.to_string()))
        } else {
            (self.min_text.clone(), self.max_text.clone())
        };
        let (avg, stddev) = if self.numeric && !self.numbers.is_empty() {
            let count = self.numbers.len() as f64;
            let mean = self.numbers.iter().sum::<f64>() / count;
            let variance = self
                .numbers
                .iter()
                .map(|value| (value - mean) * (value - mean))
                .sum::<f64>()
                / count;
            (Some(mean), Some(variance.sqrt()))
        } else {
            (None, None)
        };
        Statistics {
            count: i32::try_from(self.count).ok(),
            count_distinct: i32::try_from(self.distinct.len()).ok(),
            count_null: i32::try_from(self.nulls).ok(),
            // Only a numeric field can hold a value that is not a number; a text
            // field's "not a number" count is not defined, so it is not reported.
            count_nan: self.numeric.then(|| i32::try_from(self.not_numbers).ok()).flatten(),
            min,
            max,
            avg,
            stddev,
            count_long: Some(self.count),
            count_distinct_long: i64::try_from(self.distinct.len()).ok(),
            count_null_long: Some(self.nulls),
            count_nan_long: self.numeric.then_some(self.not_numbers),
        }
    }
}

/// Read every row of `objects` into the time series they describe, or the reason the
/// import could not be carried out.
///
/// Forecast's CSV import files carry NO header row ("Forecast … expects the data to
/// start on the first row"), so every row is data and the column layout comes from the
/// dataset's schema alone.
pub fn read_rows(
    objects: &[(String, String)],
    layout: &ColumnLayout,
    timestamp_format: &str,
) -> Result<ImportedData, String> {
    let pattern = timestamp_pattern(timestamp_format).ok_or_else(|| {
        format!(
            "The timestamp format {timestamp_format} is not one Amazon Forecast reads: \
             use {DATE_FORMAT} or {DATE_TIME_FORMAT}."
        )
    })?;
    let mut accumulators: Vec<FieldAccumulator> = layout
        .kinds
        .iter()
        .map(|kind| FieldAccumulator {
            numeric: NUMERIC_KINDS.contains(&kind.as_str()),
            ..FieldAccumulator::default()
        })
        .collect();
    let mut series: BTreeMap<Vec<String>, ImportedSeries> = BTreeMap::new();
    let mut rows = 0usize;
    let mut bytes = 0usize;

    for (key, body) in objects {
        bytes += body.len();
        let mut reader = ::csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_reader(body.as_bytes());
        for (index, record) in reader.records().enumerate() {
            let record = record.map_err(|error| {
                format!("Row {} of {key} could not be read: {error}", index + 1)
            })?;
            // A wholly empty line is a separator, not a row.
            if record.iter().all(|field| field.trim().is_empty()) {
                continue;
            }
            if record.len() != layout.names.len() {
                return Err(format!(
                    "Row {} of {key} has {} fields; the dataset's schema declares {}.",
                    index + 1,
                    record.len(),
                    layout.names.len(),
                ));
            }
            rows += 1;
            if rows > MAX_IMPORT_ROWS {
                return Err(format!(
                    "The import holds more than the {MAX_IMPORT_ROWS} rows this \
                     emulator stores for one dataset."
                ));
            }
            for (column, field) in record.iter().enumerate() {
                if let Some(accumulator) = accumulators.get_mut(column) {
                    accumulator.observe(field.trim());
                }
            }

            let raw_timestamp = record.get(layout.timestamp).unwrap_or_default().trim();
            let at = parse_timestamp(raw_timestamp, pattern).ok_or_else(|| {
                format!(
                    "Row {} of {key} carries the timestamp {raw_timestamp:?}, which is \
                     not in the format {timestamp_format}.",
                    index + 1,
                )
            })?;
            let raw_value = record.get(layout.target).unwrap_or_default().trim();
            // A row with no target value records no observation — Forecast fills a
            // missing target value rather than failing the import — but it is still a
            // row, and its null is counted in the field's statistics above.
            if raw_value.is_empty() {
                continue;
            }
            let value = raw_value.parse::<f64>().map_err(|_| {
                format!(
                    "Row {} of {key} carries the target value {raw_value:?}, which is \
                     not a number.",
                    index + 1,
                )
            })?;
            if !value.is_finite() {
                return Err(format!(
                    "Row {} of {key} carries the target value {raw_value:?}, which is \
                     not a finite number.",
                    index + 1,
                ));
            }

            let identity: Vec<String> = layout
                .dimensions
                .iter()
                .map(|column| record.get(*column).unwrap_or_default().trim().to_string())
                .collect();
            if !series.contains_key(&identity) && series.len() >= MAX_SERIES_PER_DATASET {
                return Err(format!(
                    "The import holds more than the {MAX_SERIES_PER_DATASET} time \
                     series this emulator stores for one dataset."
                ));
            }
            let entry = series.entry(identity.clone()).or_insert_with(|| {
                ImportedSeries {
                    dimensions: layout
                        .dimension_names()
                        .into_iter()
                        .zip(identity.iter().cloned())
                        .collect(),
                    observations: Vec::new(),
                }
            });
            if entry.observations.len() >= MAX_POINTS_PER_SERIES {
                return Err(format!(
                    "One time series of the import holds more than the \
                     {MAX_POINTS_PER_SERIES} observations this emulator stores."
                ));
            }
            entry.observations.push(Observation { at, value });
        }
    }

    let mut series: Vec<ImportedSeries> = series.into_values().collect();
    for one in &mut series {
        one.observations.sort_by_key(|observation| observation.at);
    }
    let statistics = layout
        .names
        .iter()
        .cloned()
        .zip(accumulators.into_iter().map(FieldAccumulator::statistics))
        .collect();
    Ok(ImportedData {
        series,
        statistics,
        rows,
        bytes,
    })
}

/// The instant `value` names in `pattern`, as seconds since the epoch.
///
/// A date-only pattern names midnight of that day, which is the instant Forecast
/// places a daily observation at.
fn parse_timestamp(value: &str, pattern: &str) -> Option<i64> {
    if pattern == "%Y-%m-%d" {
        let date = NaiveDate::parse_from_str(value, pattern).ok()?;
        return Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp());
    }
    NaiveDateTime::parse_from_str(value, pattern)
        .ok()
        .map(|at| at.and_utc().timestamp())
}

/// The naive UTC date and time `seconds` names.
fn date_time(seconds: i64) -> Option<NaiveDateTime> {
    DateTime::<Utc>::from_timestamp(seconds, 0).map(|at| at.naive_utc())
}

/// A predictor's fitted model for ONE time series: the empirical distribution of the
/// series' bucketed history, and where that history ends.
///
/// Fitted when the predictor is created, so a forecast reports the data the predictor
/// was trained on — a later import does not retroactively change an existing
/// predictor's forecasts, exactly as a trained model does not retrain itself.
#[derive(Debug, Clone)]
pub struct FittedSeries {
    /// The dimension values identifying the series (`item_id` and any forecast
    /// dimension).
    pub dimensions: BTreeMap<String, String>,
    /// The bucketed history's values, ascending: the empirical distribution.
    pub sorted_values: Vec<f64>,
    /// The arithmetic mean of that history.
    pub mean: f64,
    /// The start of the LAST bucket the history holds, as seconds since the epoch: the
    /// forecast begins at the bucket after it.
    pub last_bucket: i64,
    /// How many buckets the history held, which is what a forecast horizon is bounded
    /// against.
    pub buckets: usize,
}

/// Fit the baseline estimator to one imported series at `frequency`, spending from
/// `budget` — the bucket steps this fitting RUN may still take across every series
/// ([`MAX_BUCKETS_PER_FIT`]).
///
/// `Err` is the reason the series cannot be trained on, ready for the caller's own
/// modelled refusal or failure state; a `budget` left at zero is how the caller tells
/// "this series has no data at this frequency" from "the run has spent its budget".
pub fn fit(
    series: &ImportedSeries,
    frequency: &Frequency,
    budget: &mut usize,
) -> Result<FittedSeries, String> {
    let first = series
        .observations
        .first()
        .ok_or_else(|| "The time series holds no observations.".to_string())?;
    let mut bucket_start = date_time(first.at)
        .and_then(|at| frequency.floor(at))
        .ok_or_else(|| "The time series starts outside the representable calendar.".to_string())?;
    let mut bucket_end = frequency
        .advance(bucket_start)
        .ok_or_else(|| "The time series ends outside the representable calendar.".to_string())?;
    let mut values: Vec<f64> = Vec::new();
    // The bucket the walk is currently inside: its running sum, and whether any
    // observation has landed in it. A bucket with no observation is NOT reported as a
    // zero — nothing recorded a value for it — so it contributes no bucket value.
    let mut running = 0.0f64;
    let mut occupied = false;
    let mut advances = 0usize;
    let mut last_bucket = bucket_start.and_utc().timestamp();

    for observation in &series.observations {
        while observation.at >= bucket_end.and_utc().timestamp() {
            if occupied {
                values.push(running);
                running = 0.0;
                occupied = false;
            }
            bucket_start = bucket_end;
            bucket_end = frequency.advance(bucket_start).ok_or_else(|| {
                "The time series ends outside the representable calendar.".to_string()
            })?;
            advances += 1;
            if advances > MAX_BUCKETS_PER_SERIES {
                return Err(format!(
                    "The time series spans more than the {MAX_BUCKETS_PER_SERIES} \
                     buckets this emulator aggregates at that frequency."
                ));
            }
            match budget.checked_sub(1) {
                Some(left) => *budget = left,
                None => {
                    return Err(format!(
                        "Fitting this predictor would take more than the \
                         {MAX_BUCKETS_PER_FIT} bucket steps this emulator aggregates \
                         for one predictor."
                    ))
                }
            }
        }
        // Summing the observations of one bucket is Forecast's documented default
        // aggregation for a target value.
        running += observation.value;
        occupied = true;
        last_bucket = bucket_start.and_utc().timestamp();
    }
    if occupied {
        values.push(running);
    }

    if values.is_empty() {
        return Err("The time series holds no observations.".to_string());
    }
    let buckets = values.len();
    let total: f64 = values.iter().sum();
    let mean = total / buckets as f64;
    let mut sorted_values = values;
    sorted_values.sort_by(|left, right| left.total_cmp(right));
    Ok(FittedSeries {
        dimensions: series.dimensions.clone(),
        sorted_values,
        mean,
        last_bucket,
        buckets,
    })
}

/// The empirical quantile of `sorted` at `quantile` — linear interpolation between the
/// order statistics it falls between (the standard "linear"/R-7 estimator).
pub fn empirical_quantile(sorted: &[f64], quantile: f64) -> Option<f64> {
    let last = sorted.len().checked_sub(1)?;
    let position = quantile.clamp(0.0, 1.0) * last as f64;
    let below = position.floor();
    let above = position.ceil();
    let low = *sorted.get(below as usize)?;
    let high = *sorted.get(above as usize)?;
    if (above - below).abs() < f64::EPSILON {
        return Some(low);
    }
    Some(low + (high - low) * (position - below))
}

/// The fitted value one forecast type reports for this series.
///
/// `mean` is the history's arithmetic mean; a quantile forecast type is the history's
/// empirical quantile. A forecast type that is neither is one `CreateForecast` would
/// not have accepted, so it has no value here.
pub fn fitted_value(fitted: &FittedSeries, forecast_type: &str) -> Option<f64> {
    if forecast_type.trim() == "mean" {
        return Some(fitted.mean);
    }
    let quantile = stored_quantile(forecast_type)?;
    empirical_quantile(&fitted.sorted_values, quantile)
}

/// The quantile a forecast type names ON INPUT, or `None` when it names none.
///
/// Amazon Forecast accepts "0.01 to 0.99 (increments of .01 only) and mean"
/// (`CreateForecast$ForecastTypes`) — a decimal, NOT the `p<percent>` spelling its
/// answers report a quantile as, which is why that spelling is refused here and read
/// only by [`stored_quantile`] on the projection side.
pub fn parse_quantile(forecast_type: &str) -> Option<f64> {
    let quantile: f64 = forecast_type.trim().parse().ok()?;
    if !(quantile > 0.0 && quantile < 1.0) {
        return None;
    }
    // "increments of .01 only": the quantile must be a whole number of percent, which
    // is also why the runtime service reports it as `p<percent>` and bounds `Statistic`
    // at four characters. The comparison is against the ROUNDED percent rather than a
    // remainder, because 0.07 is 0.0700000000000000066… in binary and a remainder test
    // would refuse it.
    let percent = quantile * 100.0;
    ((percent - percent.round()).abs() < 1e-9).then_some(percent.round() / 100.0)
}

/// The quantile a STORED forecast type names: the decimal a create accepted, or the
/// `p<percent>` spelling the runtime reports a quantile as — so a forecast recorded
/// either way projects to the same statistic's value.
fn stored_quantile(forecast_type: &str) -> Option<f64> {
    let value = forecast_type.trim();
    if let Some(percent) = value.strip_prefix('p') {
        let percent: u32 = percent.parse().ok()?;
        return (1..=99)
            .contains(&percent)
            .then(|| f64::from(percent) / 100.0);
    }
    parse_quantile(value)
}

/// Whether `forecast_type` is one Amazon Forecast accepts: `mean`, or a quantile
/// between 0.01 and 0.99 in increments of .01.
pub fn is_forecast_type(forecast_type: &str) -> bool {
    forecast_type.trim() == "mean" || parse_quantile(forecast_type).is_some()
}

/// The timestamps of a forecast's horizon: the `horizon` buckets that follow the last
/// observed one, each rendered in the format a `DataPoint` carries.
pub fn horizon_timestamps(
    fitted: &FittedSeries,
    frequency: &Frequency,
    horizon: usize,
) -> Result<Vec<String>, String> {
    let mut at = date_time(fitted.last_bucket).ok_or_else(|| {
        "The time series ends outside the representable calendar.".to_string()
    })?;
    let mut timestamps = Vec::with_capacity(horizon);
    for _ in 0..horizon {
        at = frequency.advance(at).ok_or_else(|| {
            "The forecast horizon reaches outside the representable calendar.".to_string()
        })?;
        timestamps.push(at.format(PREDICTION_TIMESTAMP).to_string());
    }
    Ok(timestamps)
}

// --- The import file's object storage -------------------------------------------
// An import reads the objects its `DataSource` names out of the emulator's own S3,
// through the framework's in-process callback: the same transport a client's own S3
// request takes, so an import reads exactly what was uploaded.

/// An `s3://bucket/key` path split into its parts.
#[derive(Debug, Clone)]
pub struct S3Location {
    pub bucket: String,
    pub key: String,
}

/// Split an `S3Config$Path` into bucket and key; `None` for a value that names no
/// bucket. The member's modelled pattern (`^s3://[a-z0-9].+$`) is enforced before a
/// handler runs.
pub fn parse_s3_path(path: &str) -> Option<S3Location> {
    let rest = path.strip_prefix("s3://")?;
    let (bucket, key) = match rest.split_once('/') {
        Some((bucket, key)) => (bucket, key),
        None => (rest, ""),
    };
    (!bucket.is_empty()).then(|| S3Location {
        bucket: bucket.to_string(),
        key: key.to_string(),
    })
}

/// An Amazon S3 client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated
/// service: it routes in process, so no network client is configured here. `region` is
/// the request's own signing region, so an import reads where the caller's own S3
/// requests go.
pub fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a
        // virtual-host bucket name is not a host this emulator resolves, so every
        // request addresses its bucket in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: there is nothing to retry, time out or watch
        // for a stalled stream, and each of those would need a sleep implementation
        // this client is not given.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// Read every object under `location` as UTF-8 text, keyed by its own key, bounded by
/// [`MAX_IMPORT_OBJECTS`] and the byte bounds beside it.
///
/// `Err` is the reason the import could not be carried out, ready for the job's
/// failure state.
pub async fn read_objects(
    client: &::aws_sdk_s3::Client,
    location: &S3Location,
    path: &str,
) -> Result<Vec<(String, String)>, String> {
    let mut keys: Vec<(String, usize)> = Vec::new();
    let mut continuation: Option<String> = None;
    loop {
        let page = client
            .list_objects_v2()
            .bucket(&location.bucket)
            .prefix(&location.key)
            .set_continuation_token(continuation.clone())
            .send()
            .await
            .map_err(|error| {
                format!(
                    "Could not list the import files at {path}: {}",
                    ::aws_smithy_types::error::display::DisplayErrorContext(&error),
                )
            })?;
        for object in page.contents() {
            let Some(key) = object.key() else { continue };
            // A key ending in the delimiter is a folder marker, not an import file.
            if key.ends_with('/') {
                continue;
            }
            keys.push((
                key.to_string(),
                usize::try_from(object.size().unwrap_or_default().max(0)).unwrap_or(usize::MAX),
            ));
            if keys.len() > MAX_IMPORT_OBJECTS {
                return Err(format!(
                    "The import at {path} holds more than the {MAX_IMPORT_OBJECTS} \
                     files this emulator reads in one job."
                ));
            }
        }
        match page.next_continuation_token() {
            Some(token) => continuation = Some(token.to_string()),
            None => break,
        }
    }
    if keys.is_empty() {
        return Err(format!("No import files were found at {path}."));
    }
    // The bounds are decided from the LISTED sizes, before a single object is
    // fetched, so an import past them costs no buffering at all.
    if let Some((key, size)) = keys.iter().find(|(_, size)| *size > MAX_IMPORT_OBJECT_BYTES) {
        return Err(format!(
            "The import file s3://{}/{key} is {size} bytes, more than the \
             {MAX_IMPORT_OBJECT_BYTES} bytes this emulator reads from one file.",
            location.bucket,
        ));
    }
    let listed: usize = keys.iter().map(|(_, size)| *size).sum();
    if listed > MAX_IMPORT_TOTAL_BYTES {
        return Err(format!(
            "The import at {path} is {listed} bytes, more than the \
             {MAX_IMPORT_TOTAL_BYTES} bytes this emulator reads in one job."
        ));
    }

    let mut objects = Vec::with_capacity(keys.len());
    let mut remaining = MAX_IMPORT_TOTAL_BYTES;
    for (key, _) in keys {
        let object = client
            .get_object()
            .bucket(&location.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|error| {
                format!(
                    "Could not read the import file s3://{}/{key}: {}",
                    location.bucket,
                    ::aws_smithy_types::error::display::DisplayErrorContext(&error),
                )
            })?;
        let body = object
            .body
            .collect()
            .await
            .map_err(|error| {
                format!(
                    "Could not read the import file s3://{}/{key}: {error}",
                    location.bucket,
                )
            })?
            .into_bytes();
        if body.len() > remaining {
            return Err(format!(
                "The import at {path} is larger than the {MAX_IMPORT_TOTAL_BYTES} \
                 bytes this emulator reads in one job."
            ));
        }
        remaining -= body.len();
        let text = String::from_utf8(body.to_vec()).map_err(|_| {
            format!(
                "The import file s3://{}/{key} is not UTF-8 text; this emulator reads \
                 CSV import files only.",
                location.bucket,
            )
        })?;
        objects.push((format!("s3://{}/{key}", location.bucket), text));
    }
    Ok(objects)
}

/// The time series a `TimeSeriesIdentifiers` names, read from the object storage it
/// points at.
///
/// The identifiers file is a CSV of the dimension columns its own `Schema` declares
/// ("The TimeSeriesIdentifiers object needs the following information: DataSource
/// Format Schema"), so it is read by the same reader an import file is — one place that
/// knows how a Forecast CSV is laid out. Shared by `CreateForecast`'s and
/// `CreateWhatIfAnalysis`'s selectors, so they cannot narrow a forecast differently.
pub async fn read_identifiers(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    identifiers: &TimeSeriesIdentifiers,
) -> Result<Vec<BTreeMap<String, String>>, String> {
    let Some(source) = identifiers.data_source.as_ref() else {
        return Err(
            "TimeSeriesIdentifiers names no DataSource to read the time series \
             identifiers from."
                .to_string(),
        );
    };
    if let Some(format) = identifiers.format.as_deref() {
        if format != "CSV" {
            return Err(format!(
                "The {format} format is not one this emulator reads; use CSV."
            ));
        }
    }
    let Some(schema) = identifiers.schema.as_ref() else {
        return Err("TimeSeriesIdentifiers names no Schema for its identifiers.".to_string());
    };
    let names: Vec<String> = schema
        .attributes
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|attribute| attribute.attribute_name.clone().unwrap_or_default())
        .collect();
    if names.is_empty() {
        return Err("The TimeSeriesIdentifiers schema declares no attribute.".to_string());
    }

    let path = source.s3_config.path.clone();
    let location = parse_s3_path(&path)
        .ok_or_else(|| format!("The data source {path} names no Amazon S3 bucket."))?;
    let client = s3_client(ctx, region);
    let objects = read_objects(&client, &location, &path).await?;

    let mut selected: Vec<BTreeMap<String, String>> = Vec::new();
    for (key, body) in &objects {
        let mut reader = ::csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_reader(body.as_bytes());
        for (index, record) in reader.records().enumerate() {
            let record = record
                .map_err(|error| format!("Row {} of {key} could not be read: {error}", index + 1))?;
            if record.iter().all(|field| field.trim().is_empty()) {
                continue;
            }
            if record.len() != names.len() {
                return Err(format!(
                    "Row {} of {key} has {} fields; the TimeSeriesIdentifiers schema \
                     declares {}.",
                    index + 1,
                    record.len(),
                    names.len(),
                ));
            }
            if selected.len() >= MAX_SERIES_PER_DATASET {
                return Err(format!(
                    "The TimeSeriesIdentifiers at {path} name more than the \
                     {MAX_SERIES_PER_DATASET} time series this emulator reads."
                ));
            }
            selected.push(
                names
                    .iter()
                    .cloned()
                    .zip(record.iter().map(|field| field.trim().to_string()))
                    .collect(),
            );
        }
    }
    Ok(selected)
}

/// Whether `dimensions` identifies a series one of `identifiers` names: a superset
/// match, so an identifier that names only `item_id` selects every series of that item.
pub fn is_selected(
    dimensions: &BTreeMap<String, String>,
    identifiers: &[BTreeMap<String, String>],
) -> bool {
    identifiers.iter().any(|wanted| {
        wanted
            .iter()
            .all(|(key, value)| dimensions.get(key) == Some(value))
    })
}

// --- What-if transformations ------------------------------------------------------
//
// A what-if forecast is the base forecast's predictor run again over a history its
// `TimeSeriesTransformations` changed: "a what-if forecast is a forecast that is
// created from a modified version of the baseline forecast". Only a transformation of
// the TARGET attribute changes what this emulator's baseline estimator reads — the
// baseline is fitted to the target history alone — so a transformation of a related
// attribute (a price, a promotion flag) leaves the series as the base forecast has it,
// and nothing is invented for a regressor the estimator does not use.

/// Whether one `TimeSeriesCondition` holds for the series identified by `dimensions`.
///
/// `EQUALS`/`NOT_EQUALS` compare the dimension's value as text; `GREATER_THAN`/
/// `LESS_THAN` compare it as a number, and hold for neither when either side is not
/// one. A series that carries no such dimension satisfies only `NOT_EQUALS`.
fn condition_holds(dimensions: &BTreeMap<String, String>, condition: &TimeSeriesCondition) -> bool {
    let stored = dimensions.get(&condition.attribute_name);
    match condition.condition.as_str() {
        "EQUALS" => stored == Some(&condition.attribute_value),
        "NOT_EQUALS" => stored != Some(&condition.attribute_value),
        "GREATER_THAN" | "LESS_THAN" => {
            let Some(stored) = stored.and_then(|value| value.parse::<f64>().ok()) else {
                return false;
            };
            let Ok(wanted) = condition.attribute_value.parse::<f64>() else {
                return false;
            };
            if condition.condition == "GREATER_THAN" {
                stored > wanted
            } else {
                stored < wanted
            }
        }
        // A condition this service does not know cannot be judged, so it selects
        // nothing rather than everything.
        _ => false,
    }
}

/// Apply `operation` to `value` by `amount`, as the modelled `Operation` enum names it
/// (ADD, SUBTRACT, MULTIPLY, DIVIDE).
fn apply_operation(value: f64, operation: &str, amount: f64) -> Result<f64, String> {
    match operation {
        "ADD" => Ok(value + amount),
        "SUBTRACT" => Ok(value - amount),
        "MULTIPLY" => Ok(value * amount),
        "DIVIDE" => {
            if amount == 0.0 {
                return Err("A DIVIDE transformation cannot divide by zero.".to_string());
            }
            Ok(value / amount)
        }
        other => Err(format!(
            "The {other} operation is not one Amazon Forecast transforms a time series \
             with."
        )),
    }
}

/// The fitted model a what-if forecast runs: `fitted` with every transformation that
/// applies to this series and names `target_attribute` applied to its history, then
/// re-fitted (re-sorted, mean recomputed) so its quantiles are the transformed history's
/// own.
pub fn transform(
    fitted: &FittedSeries,
    transformations: &[TimeSeriesTransformation],
    target_attribute: &str,
) -> Result<FittedSeries, String> {
    let mut values = fitted.sorted_values.clone();
    for transformation in transformations {
        let Some(action) = transformation.action.as_ref() else {
            continue;
        };
        let conditions = transformation
            .time_series_conditions
            .as_deref()
            .unwrap_or_default();
        // The conditions are a conjunction: a transformation applies to a series only
        // when every condition it names holds for it.
        if !conditions
            .iter()
            .all(|condition| condition_holds(&fitted.dimensions, condition))
        {
            continue;
        }
        if action.attribute_name != target_attribute {
            // A related attribute this emulator's estimator does not read: the series
            // is unchanged, and no effect is invented for it.
            continue;
        }
        for value in &mut values {
            *value = apply_operation(*value, &action.operation, action.value)?;
        }
    }
    values.sort_by(|left, right| left.total_cmp(right));
    let mean = if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    };
    Ok(FittedSeries {
        dimensions: fitted.dimensions.clone(),
        sorted_values: values,
        mean,
        last_bucket: fitted.last_bucket,
        buckets: fitted.buckets,
    })
}
