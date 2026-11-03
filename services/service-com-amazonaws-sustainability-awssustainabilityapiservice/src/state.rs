// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sustainability-awssustainabilityapiservice service.
//!
//! The Sustainability API is a read-only reporting surface: there is no create/delete
//! lifecycle, so the state is the environmental-impact dataset the four operations read
//! ([`ComAmazonawsSustainabilityAwssustainabilityapiserviceState::carbon`] and
//! [`ComAmazonawsSustainabilityAwssustainabilityapiserviceState::water`]) plus the
//! filter/bucket/aggregate machinery all four share.
//!
//! The dataset is SEEDED at first touch (`Default`) from a fixed grid of periods, regions
//! and services, and every value is derived from a SHA-256 of the row's identity, so the
//! same emulator process — and any later one — reports the same numbers for the same
//! request, while nothing about a response is canned: a response is always the sum of the
//! rows the request selects.
//!
//! A row's period is the smallest granularity the operations reading it support, which is
//! what makes each surface's documented selection rule fall out of one implementation: the
//! carbon dataset is MONTHLY ("the range must include the start date of a month"), the water
//! dataset is annual, because water allocation supports only `YEARLY_CALENDAR` ("the range
//! must include the start date of a year").
//!
//! Pagination carries its cursor in the token rather than in this state: these operations
//! are `@readonly`, so serving a page must not write anything a later caller could
//! observe. [`encode_page_token`] folds the operation name, a fingerprint of the request
//! and the row offset into one opaque string, and [`decode_page_token`] refuses a token
//! this service did not mint for this very request.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use aws_smithy_schema_server::constraints::path;
use aws_smithy_schema_server::{envelope, Violation, ViolationKind};
use sha2::{Digest, Sha256};

use crate::ops::types::{
    DimensionEntry, FilterExpression, TimePeriod, DIMENSION_VALUES, EMISSIONS_UNIT_VALUES,
    TIME_GRANULARITY_VALUES, WATER_ALLOCATION_TYPE_VALUES, WATER_ALLOCATION_UNIT_VALUES,
};

/// The version of the estimation model every emitted row is attributed to.
///
/// Matches the modelled `ModelVersion` pattern (`^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)…$`),
/// and is the same string on every row of every operation: one dataset, one model.
pub const MODEL_VERSION: &str = "v1.0.0";

/// The unit of every emissions value: the single member of the generated
/// `EMISSIONS_UNIT_VALUES` (the modelled `EmissionsUnit` enum), so it cannot drift from the
/// model.
pub const EMISSIONS_UNIT: &str = EMISSIONS_UNIT_VALUES[0];

/// The unit of every water-allocation value: the single member of the generated
/// `WATER_ALLOCATION_UNIT_VALUES` (the modelled `WaterAllocationUnit` enum).
pub const WATER_ALLOCATION_UNIT: &str = WATER_ALLOCATION_UNIT_VALUES[0];

/// The emissions types `GetEstimatedCarbonEmissions` reports when the request names none.
///
/// From the operation's own documentation of `EmissionsTypes`: "If absent, returns
/// TOTAL_LBM_CARBON_EMISSIONS and TOTAL_MBM_CARBON_EMISSIONS emissions types."
pub const DEFAULT_EMISSIONS_TYPES: &[&str] = &[
    "TOTAL_LBM_CARBON_EMISSIONS",
    "TOTAL_MBM_CARBON_EMISSIONS",
];

/// The allocation types `GetEstimatedWaterAllocation` reports when the request names none:
/// the whole modelled `WaterAllocationType` enum, which has exactly one member.
pub const DEFAULT_WATER_ALLOCATION_TYPES: &[&str] = WATER_ALLOCATION_TYPE_VALUES;

/// The granularities `GetEstimatedCarbonEmissions` supports: every modelled one, with
/// MONTHLY the smallest ("The smallest supported granularity for carbon emissions is
/// MONTHLY", the operation's documentation of `Granularity`).
pub const CARBON_SUPPORTED_GRANULARITIES: &[&str] = TIME_GRANULARITY_VALUES;

/// The granularities `GetEstimatedWaterAllocation` supports.
///
/// From that operation's own documentation of `Granularity`: "Only YEARLY_CALENDAR time
/// granularity is currently supported for water allocation." Any other modelled value is
/// refused with the modelled `ValidationException` rather than answered.
pub const WATER_SUPPORTED_GRANULARITIES: &[&str] = &["YEARLY_CALENDAR"];

/// The modelled default `Granularity` of `GetEstimatedCarbonEmissions`.
pub const DEFAULT_CARBON_GRANULARITY: &str = "MONTHLY";

/// The modelled default `Granularity` of `GetEstimatedWaterAllocation` — deliberately a
/// different default from carbon's, as the model declares.
pub const DEFAULT_WATER_GRANULARITY: &str = "YEARLY_CALENDAR";

/// The modelled default `MaxResults` (`@default: 1000`).
pub const DEFAULT_MAX_RESULTS: i32 = 1000;

/// The modelled `MaxResults` range (`@range(min: 1, max: 5000)`).
pub const MAX_RESULTS_RANGE: std::ops::RangeInclusive<i32> = 1..=5000;

/// The modelled `GranularityConfiguration.FiscalYearStartMonth` range and default
/// (`@range(min: 1, max: 12)`, `@default: 1`).
pub const FISCAL_YEAR_START_MONTH_RANGE: std::ops::RangeInclusive<i32> = 1..=12;

/// The default `GranularityConfiguration.FiscalYearStartMonth`.
pub const DEFAULT_FISCAL_YEAR_START_MONTH: u8 = 1;

/// The first calendar year the seeded datasets cover.
pub const SEED_FIRST_YEAR: i32 = 2024;

/// The last calendar year the seeded datasets cover (inclusive).
pub const SEED_LAST_YEAR: i32 = 2026;

/// The regions the seeded datasets report usage in.
pub const SEED_REGIONS: &[&str] = &["eu-west-1", "us-east-1", "us-west-2"];

/// The services the seeded CARBON dataset reports usage for.
pub const SEED_CARBON_SERVICES: &[&str] = &["AWSLambda", "AmazonEC2", "AmazonRDS", "AmazonS3"];

/// The services the seeded WATER dataset reports usage for.
///
/// Deliberately a subset of [`SEED_CARBON_SERVICES`]: the two datasets are independent, so
/// `GetEstimatedWaterAllocationDimensionValues` reports the water dataset's services and
/// not the carbon dataset's.
pub const SEED_WATER_SERVICES: &[&str] = &["AmazonEC2", "AmazonRDS", "AmazonS3"];

/// How many months one seeded CARBON row covers: MONTHLY is the smallest granularity that
/// operation supports, so its dataset is monthly.
pub const CARBON_ROW_MONTHS: u8 = 1;

/// How many months one seeded WATER row covers: `YEARLY_CALENDAR` is the only granularity
/// that operation supports, so the water dataset is annual — which is also why its
/// `TimePeriod` doc reads "The range must include the start date of a year for that year's
/// data to be included in the response".
pub const WATER_ROW_MONTHS: u8 = 12;

/// The emissions types the seeded dataset measures DIRECTLY, one per GHG-Protocol scope and
/// (for scopes 2 and 3) one per accounting method.
///
/// `TOTAL_LBM_CARBON_EMISSIONS` and `TOTAL_MBM_CARBON_EMISSIONS` are not seeded: they are
/// the sums of [`LOCATION_BASED_COMPONENTS`] and [`MARKET_BASED_COMPONENTS`], which is the
/// GHG Protocol's own composition of a total from its scopes.
pub const SEEDED_EMISSIONS_COMPONENTS: &[&str] = &[
    "TOTAL_SCOPE_1_CARBON_EMISSIONS",
    "TOTAL_SCOPE_2_LBM_CARBON_EMISSIONS",
    "TOTAL_SCOPE_2_MBM_CARBON_EMISSIONS",
    "TOTAL_SCOPE_3_LBM_CARBON_EMISSIONS",
    "TOTAL_SCOPE_3_MBM_CARBON_EMISSIONS",
];

/// The components whose sum is `TOTAL_LBM_CARBON_EMISSIONS` (location-based method).
pub const LOCATION_BASED_COMPONENTS: &[&str] = &[
    "TOTAL_SCOPE_1_CARBON_EMISSIONS",
    "TOTAL_SCOPE_2_LBM_CARBON_EMISSIONS",
    "TOTAL_SCOPE_3_LBM_CARBON_EMISSIONS",
];

/// The components whose sum is `TOTAL_MBM_CARBON_EMISSIONS` (market-based method).
pub const MARKET_BASED_COMPONENTS: &[&str] = &[
    "TOTAL_SCOPE_1_CARBON_EMISSIONS",
    "TOTAL_SCOPE_2_MBM_CARBON_EMISSIONS",
    "TOTAL_SCOPE_3_MBM_CARBON_EMISSIONS",
];

/// The largest seeded emissions component, in thousandths of MTCO2e.
const CARBON_COMPONENT_SCALE: u32 = 25_000;

/// The largest seeded water withdrawal, in thousandths of a cubic metre.
const WATER_SCALE: u32 = 900_000;

/// One row of a seeded dataset: one period of one service's usage in one region, for one
/// account, with the impact values measured for it.
///
/// A row's period is the smallest granularity the operations reading that dataset support:
/// one month for carbon ([`CARBON_ROW_MONTHS`]), one calendar year for water
/// ([`WATER_ROW_MONTHS`]).
#[derive(Debug, Clone)]
pub struct ImpactRecord {
    /// The calendar year this row's period starts in.
    pub year: i32,
    /// The calendar month (1..=12) this row's period starts in.
    pub month: u8,
    /// How many months this row's period covers.
    pub span_months: u8,
    /// The `USAGE_ACCOUNT_ID` dimension value.
    pub usage_account_id: String,
    /// The `REGION` dimension value.
    pub region: String,
    /// The `SERVICE` dimension value.
    pub service: String,
    /// Impact values by emissions type (carbon) or water-allocation type (water).
    pub values: BTreeMap<String, f64>,
}

impl ImpactRecord {
    /// The instant this row's period starts, as epoch seconds.
    pub fn period_start_secs(&self) -> i64 {
        month_start_secs(self.year, self.month)
    }

    /// The instant this row's period ends (exclusive), as epoch seconds.
    pub fn period_end_secs(&self) -> i64 {
        let (year, month) = add_months(self.year, self.month, i32::from(self.span_months));
        month_start_secs(year, month)
    }

    /// This row's value for one dimension, or `None` for a name that is not a dimension.
    pub fn dimension_value(&self, dimension: &str) -> Option<&str> {
        match dimension {
            "USAGE_ACCOUNT_ID" => Some(&self.usage_account_id),
            "REGION" => Some(&self.region),
            "SERVICE" => Some(&self.service),
            _ => None,
        }
    }
}

/// Per-service state for com-amazonaws-sustainability-awssustainabilityapiservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsSustainabilityAwssustainabilityapiserviceState {
    /// The model version every emitted row is attributed to.
    pub model_version: String,
    /// The seeded carbon-emissions dataset, read by `GetEstimatedCarbonEmissions` and
    /// `GetEstimatedCarbonEmissionsDimensionValues`.
    pub carbon: Vec<ImpactRecord>,
    /// The seeded water-allocation dataset, read by `GetEstimatedWaterAllocation` and
    /// `GetEstimatedWaterAllocationDimensionValues`.
    pub water: Vec<ImpactRecord>,
}

impl Default for ComAmazonawsSustainabilityAwssustainabilityapiserviceState {
    fn default() -> Self {
        Self {
            model_version: MODEL_VERSION.to_string(),
            carbon: seed_carbon(),
            water: seed_water(),
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSustainabilityAwssustainabilityapiserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSustainabilityAwssustainabilityapiserviceState>)
}

/// The account every seeded row is usage of: the account this emulator presents.
pub fn seed_account_id() -> String {
    ::engine::emulator_account_id().to_string()
}

/// Build the seeded carbon dataset: one MONTHLY row per (month, region, service) of the
/// grid, with the five directly measured components and the two totals composed from them.
fn seed_carbon() -> Vec<ImpactRecord> {
    seed_grid(SEED_CARBON_SERVICES, CARBON_ROW_MONTHS, |key| {
        let mut values: BTreeMap<String, f64> = SEEDED_EMISSIONS_COMPONENTS
            .iter()
            .map(|metric| {
                (
                    (*metric).to_string(),
                    seeded_value("carbon", key, metric, CARBON_COMPONENT_SCALE),
                )
            })
            .collect();
        let sum_of = |components: &[&str], values: &BTreeMap<String, f64>| {
            components
                .iter()
                .filter_map(|metric| values.get(*metric).copied())
                .sum::<f64>()
        };
        let location_based = sum_of(LOCATION_BASED_COMPONENTS, &values);
        let market_based = sum_of(MARKET_BASED_COMPONENTS, &values);
        values.insert(
            "TOTAL_LBM_CARBON_EMISSIONS".to_string(),
            location_based,
        );
        values.insert("TOTAL_MBM_CARBON_EMISSIONS".to_string(), market_based);
        values
    })
}

/// Build the seeded water dataset: one `YEARLY_CALENDAR` row per (year, region, service) of
/// the water grid, with the single modelled withdrawal metric.
fn seed_water() -> Vec<ImpactRecord> {
    seed_grid(SEED_WATER_SERVICES, WATER_ROW_MONTHS, |key| {
        DEFAULT_WATER_ALLOCATION_TYPES
            .iter()
            .map(|metric| {
                (
                    (*metric).to_string(),
                    seeded_value("water", key, metric, WATER_SCALE),
                )
            })
            .collect()
    })
}

/// The period × region × service grid a dataset is seeded over, with `span_months` the
/// length of one row's period and `values` deciding what is measured for each row.
fn seed_grid(
    services: &[&str],
    span_months: u8,
    values: impl Fn(&str) -> BTreeMap<String, f64>,
) -> Vec<ImpactRecord> {
    let account = seed_account_id();
    let mut records = Vec::new();
    for year in SEED_FIRST_YEAR..=SEED_LAST_YEAR {
        for month in (1..=12u8).step_by(usize::from(span_months.max(1))) {
            for region in SEED_REGIONS {
                for service in services {
                    let key = format!("{account}|{year}-{month:02}|{region}|{service}");
                    records.push(ImpactRecord {
                        year,
                        month,
                        span_months,
                        usage_account_id: account.clone(),
                        region: (*region).to_string(),
                        service: (*service).to_string(),
                        values: values(&key),
                    });
                }
            }
        }
    }
    records
}

/// One seeded measurement: a value in `0.001..=scale/1000`, derived from a SHA-256 of the
/// row's identity and the metric's name, so it is stable across processes and independent
/// per (row, metric) without any per-call randomness.
fn seeded_value(dataset: &str, record_key: &str, metric: &str, scale: u32) -> f64 {
    let digest = Sha256::digest(format!("{dataset}|{record_key}|{metric}").as_bytes());
    let bits = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    f64::from(bits % scale + 1) / 1000.0
}

/// The instant a calendar month starts, as epoch seconds (UTC midnight on day 1).
///
/// The civil-date to epoch-seconds conversion is `aws-smithy-types`' own RFC 3339 parser
/// (the same crate, and the same `DateTime`, the wire codec uses for every modelled
/// timestamp), so this crate carries no calendar arithmetic of its own; only the month
/// SHIFTING in [`add_months`] is arithmetic, and that is a count of months, not of days.
///
/// `month` is clamped into 1..=12 and `year` into [`MIN_YEAR`]..=[`MAX_YEAR`], so the
/// rendered civil date is always well formed and the conversion is TOTAL: it cannot fail or
/// panic for any input, which is what lets every caller treat a period boundary as an
/// ordinary integer. The `debug_assert!` makes a future caller that breaks that invariant
/// fail in tests instead of silently dating a row to the epoch.
pub fn month_start_secs(year: i32, month: u8) -> i64 {
    let month = month.clamp(1, 12);
    let year = year.clamp(MIN_YEAR, MAX_YEAR);
    let civil = format!("{year:04}-{month:02}-01T00:00:00Z");
    let instant = ::aws_smithy_types::DateTime::from_str(
        &civil,
        ::aws_smithy_types::date_time::Format::DateTime,
    );
    debug_assert!(
        instant.is_ok(),
        "a clamped civil date must parse as RFC 3339: {civil}"
    );
    instant.map_or(0, |instant| instant.secs())
}

/// The earliest year a period boundary may name. Well outside any seeded year; it exists so
/// [`month_start_secs`] is total rather than fallible.
pub const MIN_YEAR: i32 = 1;

/// The latest year a period boundary may name, for the same reason as [`MIN_YEAR`].
pub const MAX_YEAR: i32 = 9999;

/// Epoch seconds as the modelled timestamp type.
pub fn to_timestamp(secs: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs(secs)
}

/// Shift a (year, month) pair by `months` months.
fn add_months(year: i32, month: u8, months: i32) -> (i32, u8) {
    let zero_based = i32::from(month) - 1 + months;
    let year = year + zero_based.div_euclid(12);
    let month = u8::try_from(zero_based.rem_euclid(12) + 1).unwrap_or(1);
    (year, month)
}

/// The period one month falls into at `granularity`, as (start, end) epoch seconds with an
/// inclusive start and an exclusive end.
///
/// `MONTHLY` is the smallest supported granularity, so every period is a whole number of
/// months. The two fiscal granularities are the calendar ones shifted by
/// `fiscal_year_start_month`, which is why `FiscalYearStartMonth: 1` makes them identical
/// to their calendar counterparts. `None` for a granularity this service does not model.
pub fn period_of_month(
    year: i32,
    month: u8,
    granularity: &str,
    fiscal_year_start_month: u8,
) -> Option<(i64, i64)> {
    // A record's month is always 1..=12; clamping keeps the arithmetic below total rather
    // than relying on that invariant.
    let month = month.clamp(1, 12);
    let fiscal_start = i32::from(fiscal_year_start_month.clamp(1, 12));
    // Months elapsed since the start of the fiscal year this month belongs to.
    let into_fiscal_year = (i32::from(month) - fiscal_start).rem_euclid(12);
    let (start_year, start_month, length) = match granularity {
        "MONTHLY" => (year, month, 1),
        "QUARTERLY_CALENDAR" => {
            let quarter_start = (month - 1) / 3 * 3 + 1;
            (year, quarter_start, 3)
        }
        "YEARLY_CALENDAR" => (year, 1, 12),
        "QUARTERLY_FISCAL" => {
            let (start_year, start_month) =
                add_months(year, month, -(into_fiscal_year % 3));
            (start_year, start_month, 3)
        }
        "YEARLY_FISCAL" => {
            let (start_year, start_month) = add_months(year, month, -into_fiscal_year);
            (start_year, start_month, 12)
        }
        _ => return None,
    };
    let (end_year, end_month) = add_months(start_year, start_month, length);
    Some((
        month_start_secs(start_year, start_month),
        month_start_secs(end_year, end_month),
    ))
}

/// The selection, bucketing and grouping one data request asks for.
#[derive(Debug)]
pub struct Query<'a> {
    /// Inclusive start of the requested range, as epoch seconds.
    pub start_secs: i64,
    /// Exclusive end of the requested range, as epoch seconds.
    pub end_secs: i64,
    /// The requested `Granularity`.
    pub granularity: &'a str,
    /// The fiscal year's first month, for the fiscal granularities.
    pub fiscal_year_start_month: u8,
    /// The dimensions to group by, in request order; empty means one row per period.
    pub group_by: &'a [String],
    /// `FilterBy.Dimensions`, normalised: a row must match EVERY named dimension by having
    /// one of the listed values.
    pub filter: &'a BTreeMap<String, BTreeSet<String>>,
    /// The metrics to sum and report, in request order.
    pub metrics: &'a [String],
}

impl Query<'_> {
    /// Whether the requested range selects `record`.
    ///
    /// The rule is the one each operation's `TimePeriod` documents: the range must include
    /// the START of the period a row covers, with `Start` inclusive and `End` exclusive —
    /// "the start date of a month" for the monthly carbon dataset, "the start date of a
    /// year" for the annual water dataset, because a row's period IS the smallest
    /// granularity its operations support.
    fn selects(&self, record: &ImpactRecord) -> bool {
        let period_start = record.period_start_secs();
        period_start >= self.start_secs && period_start < self.end_secs
    }

    /// Whether `record` satisfies every dimension filter.
    fn matches_filter(&self, record: &ImpactRecord) -> bool {
        self.filter.iter().all(|(dimension, allowed)| {
            record
                .dimension_value(dimension)
                .is_some_and(|value| allowed.contains(value))
        })
    }
}

/// One aggregated row: a period, the group it describes, and the summed metrics.
#[derive(Debug, Clone)]
pub struct AggregateRow {
    /// Inclusive start of the row's period, as epoch seconds.
    pub period_start_secs: i64,
    /// Exclusive end of the row's period, as epoch seconds.
    pub period_end_secs: i64,
    /// The group's dimension values, by dimension; empty when the request grouped by none.
    pub dimensions: BTreeMap<String, String>,
    /// The summed value of each requested metric.
    pub values: BTreeMap<String, f64>,
}

impl AggregateRow {
    /// The row's period as the modelled shape.
    pub fn time_period(&self) -> TimePeriod {
        TimePeriod {
            start: to_timestamp(self.period_start_secs),
            end: to_timestamp(self.period_end_secs),
        }
    }
}

/// Select, bucket, group and sum `records` as `query` asks.
///
/// A row's reported period is the UNION of the periods of the rows it sums, so it always
/// states exactly what the value covers:
///
/// * carbon, whose rows are monthly, reports a partly covered bucket narrowed to the months
///   the range selected — "requesting 2025-04-01T00:00:00Z to 2026-04-01T00:00:00Z with
///   YEARLY_CALENDAR granularity will return the last 9 months for 2025 and the first 3
///   months of 2026" (`GetEstimatedCarbonEmissions`'s documentation of `Granularity`);
/// * water, whose rows are annual and whose only supported granularity is
///   `YEARLY_CALENDAR`, reports the whole selected year — the same request "will return all
///   the data for 2026 only" (`GetEstimatedWaterAllocation`'s documentation of
///   `Granularity`), since 2026 is the only year whose start the range includes.
///
/// Rows are grouped by their natural period, so the grouping does not depend on which
/// months a range happens to clip. The result is ordered by period, then by the group's
/// dimension values, so two calls with the same request return the same rows in the same
/// order and a page token names a stable position.
pub fn aggregate(records: &[ImpactRecord], query: &Query<'_>) -> Vec<AggregateRow> {
    /// The union of the selected rows' periods, with the metrics summed over them.
    type Group = (i64, i64, BTreeMap<String, f64>);

    let mut groups: BTreeMap<(i64, i64, Vec<String>), Group> = BTreeMap::new();
    for record in records {
        if !query.selects(record) || !query.matches_filter(record) {
            continue;
        }
        let Some((bucket_start, bucket_end)) = period_of_month(
            record.year,
            record.month,
            query.granularity,
            query.fiscal_year_start_month,
        ) else {
            continue;
        };
        let group = query
            .group_by
            .iter()
            .map(|dimension| {
                record
                    .dimension_value(dimension)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect::<Vec<String>>();
        let (period_start, period_end) = (record.period_start_secs(), record.period_end_secs());
        let entry = groups
            .entry((bucket_start, bucket_end, group))
            .or_insert((period_start, period_end, BTreeMap::new()));
        entry.0 = entry.0.min(period_start);
        entry.1 = entry.1.max(period_end);
        for metric in query.metrics {
            *entry.2.entry(metric.clone()).or_insert(0.0) +=
                record.values.get(metric).copied().unwrap_or_default();
        }
    }
    groups
        .into_iter()
        .map(
            |((_, _, group), (period_start_secs, period_end_secs, values))| AggregateRow {
                period_start_secs,
                period_end_secs,
                dimensions: query
                    .group_by
                    .iter()
                    .cloned()
                    .zip(group)
                    .collect::<BTreeMap<String, String>>(),
                values,
            },
        )
        .collect()
}

/// The distinct values each requested dimension takes over the rows the range selects, as
/// the modelled entries.
///
/// A row is selected by the start of the period it covers, exactly as the data operations
/// select it (the rule on [`Query::selects`]), so the values reported here are exactly the
/// ones that can appear in the matching data operation's `DimensionsValues` for the same
/// range. Ordered by the request's dimension order and then by value, so the entries page
/// stably.
pub fn dimension_values(
    records: &[ImpactRecord],
    start_secs: i64,
    end_secs: i64,
    dimensions: &[String],
) -> Vec<DimensionEntry> {
    let in_range = |record: &&ImpactRecord| {
        let period_start = record.period_start_secs();
        period_start >= start_secs && period_start < end_secs
    };
    dimensions
        .iter()
        .flat_map(|dimension| {
            records
                .iter()
                .filter(in_range)
                .filter_map(|record| record.dimension_value(dimension))
                .collect::<BTreeSet<&str>>()
                .into_iter()
                .map(|value| DimensionEntry {
                    dimension: dimension.clone(),
                    value: value.to_string(),
                })
                .collect::<Vec<DimensionEntry>>()
        })
        .collect()
}

/// The page of `rows` a cursor at `offset` with page size `page_size` covers, and the
/// offset the next page starts at when one exists.
pub fn page<T: Clone>(rows: &[T], offset: usize, page_size: usize) -> (Vec<T>, Option<usize>) {
    let start = offset.min(rows.len());
    let end = start.saturating_add(page_size).min(rows.len());
    let next = (end < rows.len()).then_some(end);
    (rows[start..end].to_vec(), next)
}

/// The version marker every page token carries, so a token minted by an older encoding is
/// refused rather than misread.
const TOKEN_VERSION: &str = "v1";

/// An opaque page token naming a position in ONE operation's answer to ONE request.
///
/// Hex-encoding keeps the cursor opaque to the caller; carrying the operation name and the
/// request fingerprint is what lets [`decode_page_token`] refuse a token minted by another
/// operation, or for a different request, instead of silently paging the wrong rows.
pub fn encode_page_token(operation: &str, fingerprint: &str, offset: usize) -> String {
    ::hex::encode(format!("{TOKEN_VERSION}|{operation}|{fingerprint}|{offset}"))
}

/// The offset a token names, or `None` for a token this service did not mint for this
/// operation and this request.
pub fn decode_page_token(token: &str, operation: &str, fingerprint: &str) -> Option<usize> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let mut fields = decoded.split('|');
    let version = fields.next()?;
    let token_operation = fields.next()?;
    let token_fingerprint = fields.next()?;
    let offset = fields.next()?;
    if fields.next().is_some()
        || version != TOKEN_VERSION
        || token_operation != operation
        || token_fingerprint != fingerprint
    {
        return None;
    }
    offset.parse::<usize>().ok()
}

/// A fingerprint of everything about a request that decides its rows, so resuming with a
/// token minted for different parameters is refused.
pub fn request_fingerprint(parts: &[String]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0u8]);
    }
    ::hex::encode(&hasher.finalize()[..8])
}

/// The canonical form of a normalised dimension filter, for a request fingerprint.
pub fn canonical_filter(filter: &BTreeMap<String, BTreeSet<String>>) -> String {
    filter
        .iter()
        .map(|(dimension, values)| {
            format!(
                "{dimension}={}",
                values.iter().cloned().collect::<Vec<String>>().join(",")
            )
        })
        .collect::<Vec<String>>()
        .join(";")
}

/// The shape name of the modelled 403 error every operation of this service declares:
/// `AccessDeniedException` (`@error("client")`, `@httpError(403)`, required `Message` with
/// `jsonName: "message"`).
///
/// A refusal taken before any handler ran is rendered by the framework, which names its
/// own `AccessDenied`. `restJson1` resolves the error SHAPE from that name, so this
/// service answers a pre-dispatch 403 with the shape its model declares instead — see
/// `EmulatedService::override_refusal`.
pub const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";

/// The message of a pre-dispatch 403: the documented text of `AccessDeniedException`
/// ("You do not have sufficient access to perform this action." —
/// `smithy_query(query="doc", shape="AccessDeniedException")`) followed by what the caller
/// must present, which the framework's own wording carried and the shape's documentation
/// does not.
///
/// [`Refusal::Unauthenticated`] carries no detail — an absent signature, a scope naming
/// another service and a signature that does not verify all arrive the same way — so the
/// message states the requirement only and diagnoses nothing it cannot observe.
pub const ACCESS_DENIED_MESSAGE: &str = "You do not have sufficient access to perform this \
     action. Sign the request with the emulator credentials, using SigV4 \
     (AWS4-HMAC-SHA256).";

/// How long a rejected value may be when a message echoes it back.
///
/// The framework caps its own echoed values at 256 bytes, on a char boundary, because a
/// caller controls the length and a message interpolates it as-is: an uncapped echo turns
/// one request into an arbitrarily long response. The cap is not exported (its `echoed`
/// helper is private to `aws_smithy_schema_server::constraints`), so it is restated here
/// with the same bound and the same rendering.
const MAX_ECHOED_VALUE: usize = 256;

/// A rejected value as it appears in a MESSAGE: itself when short, a char-boundary-safe
/// prefix with an elision when not. RENDERING ONLY — every check runs on the original.
fn echoed(value: &str) -> String {
    if value.len() <= MAX_ECHOED_VALUE {
        return value.to_string();
    }
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &value[..end], value.len())
}

/// The modelled `ValidationException` message for one constraint a request failed, in the
/// AWS validation envelope.
///
/// A constraint the MODEL declares is checked by the framework's validating walk before a
/// handler runs, and these operations check the rest (an effective value set documented
/// per operation, a bound a handler owns). Both must read alike, so this builds the
/// message the same way: `aws_smithy_schema_server`'s one sentence builder
/// ([`Violation::message`] via [`envelope`]) — `1 validation error detected: Value
/// '<value>' at '<member>' failed to satisfy constraint: <requirement>`. A caller
/// therefore cannot tell which layer refused a value, and the offending member and the
/// constraint it failed are always named.
pub fn constraint_message(member: &str, value: Option<&str>, kind: ViolationKind) -> String {
    let violation = Violation {
        path: member.to_string(),
        kind,
        value: value.map(echoed),
        sensitive: false,
    };
    envelope(std::slice::from_ref(&violation)).unwrap_or_else(|| violation.message())
}

/// The enum constraint of a member whose value set is `allowed`.
fn enum_constraint(allowed: &[&str]) -> ViolationKind {
    ViolationKind::EnumValue(allowed.iter().map(|value| (*value).to_string()).collect())
}

/// The wire path of a Smithy member, as the AWS validation envelope spells it: every
/// segment of a dotted member path in lowerCamel, joined by the framework's own path
/// helpers — so `AllocationTypes` is reported at `allocationTypes` and
/// `GranularityConfiguration.FiscalYearStartMonth` at
/// `granularityConfiguration.fiscalYearStartMonth`.
pub fn member_path(member: &str) -> String {
    let segments: Vec<String> = member.split('.').map(path::member).collect();
    path::join(&segments)
}

/// The wire path of the `position`-th element (1-based) of a list member, as the framework
/// spells a collection step: `allocationTypes.1.member`.
pub fn element_path(member: &str, position: usize) -> String {
    path::join(&[member_path(member), path::element(position)])
}

/// `TimePeriod` as (start, end) epoch seconds.
///
/// `Start` is inclusive and `End` exclusive, so an empty or inverted range selects nothing
/// and is refused rather than answered with an empty page.
pub fn validate_time_period(time_period: &TimePeriod) -> Result<(i64, i64), String> {
    let start = time_period.start.secs();
    let end = time_period.end.secs();
    if end <= start {
        return Err(format!(
            "TimePeriod.End must be later than TimePeriod.Start, because Start is inclusive \
             and End is exclusive; got Start {start} and End {end} as epoch seconds"
        ));
    }
    Ok((start, end))
}

/// The values of one enum-valued list member, de-duplicated and in request order.
///
/// `member` is the Smithy member path of the LIST (`AllocationTypes`, `GroupBy`,
/// `Dimensions`, `EmissionsTypes`). A value outside `allowed` is refused with the enum
/// constraint of the list's member, reported at the element's own path — the spelling the
/// framework's validating walk uses for the same class (`allocationTypes.1.member`), so a
/// caller cannot tell which layer refused the value.
pub fn validate_enum_list(
    member: &str,
    values: &[String],
    allowed: &[&str],
) -> Result<Vec<String>, String> {
    let mut accepted: Vec<String> = Vec::new();
    for (position, value) in values.iter().enumerate() {
        if !allowed.contains(&value.as_str()) {
            return Err(constraint_message(
                &element_path(member, position + 1),
                Some(value),
                enum_constraint(allowed),
            ));
        }
        if !accepted.contains(value) {
            accepted.push(value.clone());
        }
    }
    Ok(accepted)
}

/// The requested `Granularity`.
///
/// A request that omits it arrives carrying that operation's modelled `@default`
/// ([`DEFAULT_CARBON_GRANULARITY`] / [`DEFAULT_WATER_GRANULARITY`]), so every value reaching
/// here was really asked for and no value is normalised: the only outcomes are the value
/// itself or a refusal.
///
/// `supported` is the set THIS operation supports, which is not always every modelled
/// value: a modelled granularity the operation does not support is refused with the
/// modelled `ValidationException` rather than answered with a plausible row. The refusal
/// names the set this operation accepts — the effective value set of the member here —
/// so one check covers both an unmodelled value and a modelled one this operation does
/// not serve.
pub fn validate_granularity(granularity: &str, supported: &[&str]) -> Result<String, String> {
    if !supported.contains(&granularity) {
        return Err(constraint_message(
            &member_path("Granularity"),
            Some(granularity),
            enum_constraint(supported),
        ));
    }
    Ok(granularity.to_string())
}

/// The requested page size, refusing a value outside the modelled range.
///
/// A request that omits `MaxResults` arrives carrying the modelled `@default` of
/// [`DEFAULT_MAX_RESULTS`], so every value reaching here was really asked for.
pub fn validate_max_results(max_results: i32) -> Result<usize, String> {
    let member = member_path("MaxResults");
    let shown = max_results.to_string();
    if max_results < *MAX_RESULTS_RANGE.start() {
        return Err(constraint_message(
            &member,
            Some(&shown),
            ViolationKind::MinValue(MAX_RESULTS_RANGE.start().to_string()),
        ));
    }
    if max_results > *MAX_RESULTS_RANGE.end() {
        return Err(constraint_message(
            &member,
            Some(&shown),
            ViolationKind::MaxValue(MAX_RESULTS_RANGE.end().to_string()),
        ));
    }
    // The two checks above leave the value inside `MAX_RESULTS_RANGE`, so the conversion
    // cannot fail; the assertion makes a bound that ever moved below zero fail loudly in a
    // debug build (every test runs one) instead of quietly serving another page size.
    debug_assert!(
        MAX_RESULTS_RANGE.contains(&max_results),
        "MaxResults {max_results} passed both bound checks but is outside the modelled range"
    );
    Ok(usize::try_from(max_results).unwrap_or(1))
}

/// The fiscal year's first month, refusing a value outside the modelled range.
///
/// As with [`validate_granularity`] and [`validate_max_results`], an omitted member arrives
/// carrying the modelled `@default` of [`DEFAULT_FISCAL_YEAR_START_MONTH`], so a value
/// outside the range is one a caller really sent and is refused rather than normalised.
pub fn validate_fiscal_year_start_month(month: i32) -> Result<u8, String> {
    let member = member_path("GranularityConfiguration.FiscalYearStartMonth");
    let shown = month.to_string();
    if month < *FISCAL_YEAR_START_MONTH_RANGE.start() {
        return Err(constraint_message(
            &member,
            Some(&shown),
            ViolationKind::MinValue(FISCAL_YEAR_START_MONTH_RANGE.start().to_string()),
        ));
    }
    if month > *FISCAL_YEAR_START_MONTH_RANGE.end() {
        return Err(constraint_message(
            &member,
            Some(&shown),
            ViolationKind::MaxValue(FISCAL_YEAR_START_MONTH_RANGE.end().to_string()),
        ));
    }
    // As with `validate_max_results`: in 1..=12 after the two checks above, asserted in a
    // debug build so a moved bound fails loudly rather than substituting a month.
    debug_assert!(
        FISCAL_YEAR_START_MONTH_RANGE.contains(&month),
        "FiscalYearStartMonth {month} passed both bound checks but is outside the modelled range"
    );
    Ok(u8::try_from(month).unwrap_or(DEFAULT_FISCAL_YEAR_START_MONTH))
}

/// `FilterBy.Dimensions` normalised to dimension -> set of accepted values.
///
/// A dimension the service does not model, or one listed with no values at all (a filter no
/// row could satisfy and that no caller can have meant), is refused rather than silently
/// dropped or silently answered with an empty page.
///
/// The entries are walked in sorted key order, not in the order a `HashMap` happens to
/// yield them, so a request naming two offending dimensions is always refused for the same
/// one: the refusal is a function of the request, as every other answer here is.
pub fn validate_filter(
    filter: Option<&FilterExpression>,
) -> Result<BTreeMap<String, BTreeSet<String>>, String> {
    let mut normalised: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let dimensions: Option<&HashMap<String, Vec<String>>> =
        filter.and_then(|filter| filter.dimensions.as_ref());
    let mut entries: Vec<(&String, &Vec<String>)> = dimensions.into_iter().flatten().collect();
    entries.sort_by_key(|(dimension, _)| *dimension);
    for (dimension, values) in entries {
        if !DIMENSION_VALUES.contains(&dimension.as_str()) {
            // The map's KEY is the modelled `Dimension` enum (`DimensionListMap`), so a
            // key outside that value set fails the same enum constraint a `GroupBy`
            // element does. It is stated at the MAP member's own path, with the rejected
            // key as the value: the framework's `path::entry` spelling names the step into
            // an entry's VALUE, which is not what failed here.
            return Err(constraint_message(
                &member_path("FilterBy.Dimensions"),
                Some(dimension),
                enum_constraint(DIMENSION_VALUES),
            ));
        }
        if values.is_empty() {
            // Not a declared constraint — the model puts no `@length` on
            // `DimensionValueList` — but a dimension listed with no values is a filter no
            // row can satisfy and no caller can have meant, so it is refused rather than
            // silently dropped or answered with an empty page.
            return Err(format!(
                "FilterBy.Dimensions['{}'] must list at least one value",
                echoed(dimension)
            ));
        }
        normalised.insert(dimension.clone(), values.iter().cloned().collect());
    }
    Ok(normalised)
}
