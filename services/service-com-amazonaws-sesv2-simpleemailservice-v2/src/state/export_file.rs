// SPDX-License-Identifier: CC0-1.0

//! The export file an export job's run writes, and the statistics about that run.
//!
//! "SES writes the export file to a location that it manages. After the job completes,
//! call `GetExportJob` and use the `S3Url` that's returned to download the file" — so
//! the file is part of the job's record: it is written ONCE, when the run falls due
//! ([`super::ComAmazonawsSesv2SimpleemailserviceV2State::settle`]), from the records
//! the job's data source selects, and every later download of that job serves exactly
//! the bytes the run wrote and the statistics reported for it count.
//!
//! What a file holds is decided by the data source, which is the only thing the create
//! request says about the export:
//!
//! * a `MetricsDataSource` export holds one record per UTC day the account's records
//!   hold sending activity on, per combination of the dimension values the request
//!   names, carrying that day, those dimension values, and the value of each metric
//!   the request asks for ([`super::metric_day_counts`] answers each one, so the
//!   export and a metric query cannot disagree);
//! * a `MessageInsightsDataSource` export holds one record per recipient of each
//!   message the account's outbound log holds inside the requested interval, after the
//!   `Include` and `Exclude` filters, at most `MaxResults` of them.
//!
//! The record set is the same either way; only its rendering differs, and
//! `ExportDestination$DataFormat` decides that: `CSV` writes a header row and one row
//! per record, `JSON` writes one object per record.
//!
//! Split out of the state module root so this derived view lives in one place; every
//! item is re-exported from [`crate::state`], so a handler reaches it as
//! `crate::state::…`.

use std::collections::BTreeMap;
use std::time::SystemTime;

use super::{ComAmazonawsSesv2SimpleemailserviceV2State as Store, ExportJobRecord};

/// Records one export file may hold.
///
/// A CHOSEN bound, not an AWS one: the file lives in the job's record for as long as
/// the job does, so the account's export jobs cannot grow the store without end. It is
/// also the number of results a Message Insights export returns when the request names
/// no `MaxResults` ("If you don't specify MaxResults, the export returns a maximum of
/// 1,000 results"), which is the only count the model itself states for an export file.
pub const MAX_EXPORT_FILE_RECORDS: usize = 1_000;

/// `MessageInsightsDataSource$MaxResults` when the request names none: "If you don't
/// specify MaxResults, the export returns a maximum of 1,000 results."
pub const MESSAGE_INSIGHTS_DEFAULT_MAX_RESULTS: usize = 1_000;

/// Export files one call may write.
///
/// A CHOSEN bound, not an AWS one: a run writes its file as its completion falls due,
/// and every call that settles the store materialises the transitions the clock owes,
/// so without this one call could be made to write every pending run's file at once.
/// A run whose file this call did not get to is not complete yet — its completion stays
/// owed, and the next call writes it — so the work one caller pays for is bounded while
/// no run is reported finished without the file it is finished BY.
pub const MAX_EXPORT_FILES_PER_CALL: usize = 8;

/// Metrics one metrics export may report.
///
/// The model's own bound on `ExportMetrics` (`smithy_query(query="shape",shape="ExportMetrics")`:
/// length 1..10), applied here as well as at the request boundary, so the width of a
/// record is bounded by this file's own reading of the model and not only by the
/// validation that ran before it.
pub const MAX_EXPORT_METRICS: usize = 10;

/// Record visits one export file's run may make.
///
/// A CHOSEN bound, not an AWS one, and the budget that caps the WORK of a render rather
/// than its output: the records a metrics export reads are the account's accepted
/// messages (at most [`super::MAX_SENT_MESSAGES`]) times the metrics the request asks
/// for (at most ten, as the model bounds `ExportMetrics`), and each is placed into the
/// dimension combinations it belongs to. The budget is spent per record read and per
/// placement made, and a render that spends it stops reading, so no request can make
/// one file's run cost more than this however its data source is shaped.
pub const MAX_EXPORT_FILE_VISITS: usize = 100_000;

/// `DataFormat$JSON` — "a Json file".
pub const DATA_FORMAT_JSON: &str = "JSON";

/// How long the download URL `GetExportJob` hands out stays valid: "The URL expires
/// five minutes after `GetExportJob` returns it. Call `GetExportJob` again to get a
/// new URL."
pub const EXPORT_FILE_URL_EXPIRY: ::std::time::Duration = ::std::time::Duration::from_secs(5 * 60);

/// The path segments the download URL of an export job's file addresses, before the
/// job's own id: the modelled route of the job itself
/// (`smithy_query(query="operation",operation="GetExportJob")`: `/v2/email/export-jobs/{JobId}`),
/// so the file a job holds is addressed under the job that holds it.
pub const EXPORT_FILE_PATH_SEGMENTS: [&str; 3] = ["v2", "email", "export-jobs"];

/// The last segment of that path: the file of the job the segments before it name.
pub const EXPORT_FILE_PATH_SUFFIX: &str = "file";

/// The path the download URL of `job_id`'s export file addresses, decoded, as the
/// signer takes it.
pub fn export_file_path(job_id: &str) -> String {
    format!(
        "/{}/{job_id}/{EXPORT_FILE_PATH_SUFFIX}",
        EXPORT_FILE_PATH_SEGMENTS.join("/")
    )
}

/// The job id the export-file path `segments` names, or `None` when the segments are
/// not such a path.
///
/// The counterpart of [`export_file_path`], so the route that serves a download and
/// the URL handed out for it cannot disagree about which job a path names.
pub fn export_file_path_job_id(segments: &[String]) -> Option<&str> {
    let (job_id, prefix) = match segments {
        [prefix @ .., job_id, suffix] if suffix == EXPORT_FILE_PATH_SUFFIX => (job_id, prefix),
        _ => return None,
    };
    let addresses_jobs = prefix.len() == EXPORT_FILE_PATH_SEGMENTS.len()
        && prefix
            .iter()
            .zip(EXPORT_FILE_PATH_SEGMENTS)
            .all(|(seen, expected)| seen == expected);
    (addresses_jobs && !job_id.is_empty()).then_some(job_id.as_str())
}

/// The media type a `CSV` export file is served as: "a comma-separated values file".
pub const CSV_MEDIA_TYPE: &str = "text/csv";

/// The media type a `JSON` export file is served as.
pub const JSON_MEDIA_TYPE: &str = "application/json";

/// The field a metrics export record carries the UTC day it covers in, as
/// `YYYY-MM-DD`: a metric data point covers one day
/// ([`super::DATA_POINT_SPAN_SECONDS`]), so the day is what tells two records of one
/// dimension combination apart.
const METRICS_DATE_FIELD: &str = "Date";

/// `MetricAggregation$VOLUME` — "the volume of events for this metric", and what a
/// metric the request names no aggregation for is counted as: a volume is the count
/// the records themselves hold, with nothing else to divide it by.
const AGGREGATION_VOLUME: &str = "VOLUME";

/// `MetricAggregation$RATE` — "the rate for this metric relative to the SEND metric
/// volume".
const AGGREGATION_RATE: &str = "RATE";

/// Fractional digits a `RATE` value is written with. A rate is a ratio of two counts,
/// so it is written to a fixed precision rather than with a count's exactness; six
/// digits distinguish one event in a million, finer than any volume the store holds.
const RATE_DIGITS: usize = 6;

/// The fields a Message Insights export record carries, in the order a `CSV` file
/// writes them.
///
/// The message's own identity and the instant SES accepted it, then exactly the
/// members `MessageInsightsFilters` filters on (`smithy_query(query="shape",
/// shape="MessageInsightsFilters")`), so a record carries every value a filter can
/// select it by.
const MESSAGE_INSIGHTS_FIELDS: [&str; 9] = [
    "MessageId",
    "Timestamp",
    "FromEmailAddress",
    "Subject",
    "Destination",
    "Isp",
    "TenantName",
    "LastDeliveryEvent",
    "LastEngagementEvent",
];

/// One export file: the bytes the run wrote, the media type they are served as, and
/// the statistics of the run that wrote them.
#[derive(Debug, Clone)]
pub struct ExportFile {
    /// The file itself, as the run rendered it.
    pub body: String,
    /// The media type the file is served as, from the data format it was written in.
    pub media_type: &'static str,
    /// `ExportStatistics$ProcessedRecordsCount`: "the number of records that were
    /// processed to generate the final export file" — the records of the account the
    /// run read, before the export's own filters and limits.
    pub processed_records: i32,
    /// `ExportStatistics$ExportedRecordsCount`: "the number of records that were
    /// exported to the final export file".
    pub exported_records: i32,
}

impl ExportFile {
    /// The modelled `ExportStatistics` view of the run that wrote this file.
    pub fn statistics(&self) -> crate::ops::types::ExportStatistics {
        crate::ops::types::ExportStatistics {
            processed_records_count: Some(self.processed_records),
            exported_records_count: Some(self.exported_records),
        }
    }
}

/// The file the run of `job` writes from `store`.
///
/// Called once, as the run completes, so the file a later download serves is the
/// records as they stood when the run fell due.
pub fn render(store: &Store, job: &ExportJobRecord) -> ExportFile {
    let records = if let Some(source) = job.data_source.metrics_data_source.as_ref() {
        metric_records(store, source)
    } else if let Some(source) = job.data_source.message_insights_data_source.as_ref() {
        message_insights_records(store, source)
    } else {
        // A stored job always carries exactly one data source, because that is what
        // `CreateExportJob` accepts: a record without one selects no records at all,
        // so its run writes an empty file rather than guessing a source.
        RecordSet::default()
    };
    let json = job.data_format == DATA_FORMAT_JSON;
    ExportFile {
        body: if json {
            render_json(&records)
        } else {
            render_csv(&records)
        },
        media_type: if json {
            JSON_MEDIA_TYPE
        } else {
            CSV_MEDIA_TYPE
        },
        processed_records: saturating_count(records.processed),
        exported_records: saturating_count(records.rows.len()),
    }
}

/// The records of one export file: the fields every record carries, in order, the
/// records themselves, and how many of the account's records the run read to produce
/// them.
#[derive(Debug, Default)]
struct RecordSet {
    /// The field names, in the order a file writes them.
    fields: Vec<String>,
    /// One row per record, each as long as [`Self::fields`].
    rows: Vec<Vec<Field>>,
    /// The records of the account the run read.
    processed: usize,
}

/// One field of one record: text, a count, or a rate.
#[derive(Debug, Clone)]
enum Field {
    /// A value the records carry as text, which an absent value renders as empty.
    Text(String),
    /// A count of events.
    Volume(i64),
    /// A ratio of two counts.
    Rate(f64),
}

impl Field {
    /// The value as one cell of a record: text as it stands, a count as its digits, a
    /// rate to a fixed precision. Nothing is quoted or escaped here — the `CSV` codec
    /// owns that ([`render_csv`]).
    fn cell(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Volume(count) => count.to_string(),
            Self::Rate(rate) => format!("{rate:.RATE_DIGITS$}"),
        }
    }

    /// The value as a JSON value.
    fn json_value(&self) -> serde_json::Value {
        match self {
            Self::Text(text) => serde_json::Value::String(text.clone()),
            Self::Volume(count) => serde_json::Value::from(*count),
            Self::Rate(rate) => serde_json::Number::from_f64(round_rate(*rate))
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
        }
    }
}

/// `rate` with [`RATE_DIGITS`] fractional digits kept, so the CSV and the JSON
/// renderings of one record carry the same value.
fn round_rate(rate: f64) -> f64 {
    let scale = 10_f64.powi(RATE_DIGITS as i32);
    (rate * scale).round() / scale
}

/// `count` as the modelled statistics report it, which is an integer: a count past
/// what one can hold is reported as the largest it can, never wrapped.
fn saturating_count(count: usize) -> i32 {
    i32::try_from(count).unwrap_or(i32::MAX)
}

/// The records a `CSV` file holds: the field names as its header row, then one row per
/// record. A file with no fields at all — a job that selects no records — is empty.
///
/// Comma-separated values as the `csv` crate writes them, so the quoting of a cell that
/// needs it — a subject or an address may carry a comma or a quote — is the codec's rule
/// and not re-derived here. Every row carries one cell per field and no byte the codec
/// can refuse, so the writer cannot fail; an unexpected failure answers no file rather
/// than a truncated one a client could not tell from a complete one, and is logged.
fn render_csv(records: &RecordSet) -> String {
    if records.fields.is_empty() {
        return String::new();
    }
    let mut writer = ::csv::Writer::from_writer(Vec::new());
    let mut write = || -> Result<(), ::csv::Error> {
        writer.write_record(&records.fields)?;
        for row in &records.rows {
            writer.write_record(row.iter().map(Field::cell))?;
        }
        writer.flush().map_err(::csv::Error::from)
    };
    if let Err(error) = write() {
        ::tracing::warn!(%error, "an export job's file could not be written");
        return String::new();
    }
    match writer.into_inner() {
        Ok(file) => String::from_utf8_lossy(&file).into_owned(),
        Err(error) => {
            ::tracing::warn!(%error, "an export job's file could not be written");
            String::new()
        }
    }
}

/// The records a `JSON` file holds: an array with one object per record, keyed by the
/// field names.
fn render_json(records: &RecordSet) -> String {
    let documents: Vec<serde_json::Value> = records
        .rows
        .iter()
        .map(|row| {
            let members: serde_json::Map<String, serde_json::Value> = records
                .fields
                .iter()
                .cloned()
                .zip(row.iter().map(Field::json_value))
                .collect();
            serde_json::Value::Object(members)
        })
        .collect();
    serde_json::Value::Array(documents).to_string()
}

// ---------------------------------------------------------------------------
// The metrics export
// ---------------------------------------------------------------------------

/// The records a metrics export holds: one per UTC day the account's records hold
/// sending activity on, per combination of the dimension values the request names.
///
/// The day set is the activity's, not the request's: a day of the interval the records
/// hold nothing for on a given combination is not a record, exactly as it is not a data
/// point of a metric query. Each record then carries every metric the request asks
/// for, which is zero on a day whose activity holds no event of it.
///
/// The work is one pass over the account's records PER METRIC, not per combination:
/// each record is placed into the combinations it belongs to
/// ([`Grid::place`]), so the cost is the records times the metrics times the dimension
/// values each record is tested against, and never the product with the combination
/// count.
fn metric_records(store: &Store, source: &crate::ops::types::MetricsDataSource) -> RecordSet {
    let start = source.start_date.as_secs_f64();
    let end = source.end_date.as_secs_f64();
    let grid = Grid::of(source);
    let metrics = requested_metrics(source);

    let mut fields: Vec<String> = Vec::with_capacity(1 + grid.dimensions.len() + metrics.len());
    fields.push(METRICS_DATE_FIELD.to_string());
    fields.extend(grid.dimensions.iter().map(|(name, _)| name.clone()));
    fields.extend(metrics.iter().map(|metric| metric.field.clone()));

    // The days each combination holds activity on, and the denominator of every rate
    // on each of them. The key order is (combination, day) ascending, which is the
    // order the records are written in.
    //
    // One budget for the whole render, spent across every tally below, so the work this
    // file costs is capped however the data source is shaped.
    let mut budget = Budget::new();
    let sends = grid.tally(store, super::METRIC_SEND, start, end, &mut budget);
    // Each requested metric's own tally, read from the same records, so the export
    // reports what a metric query reports for the same window and dimensions.
    let tallies: Vec<BTreeMap<(usize, i64), i64>> = metrics
        .iter()
        .map(|metric| grid.tally(store, &metric.name, start, end, &mut budget))
        .collect();

    let mut rows: Vec<Vec<Field>> = Vec::new();
    for ((combination, day), sent) in &sends {
        // Each written field is a visit too, so the rows a file materialises are
        // bounded by the same budget its reads are and not only by the record cap.
        if rows.len() >= MAX_EXPORT_FILE_RECORDS || !budget.spend() {
            break;
        }
        let mut row: Vec<Field> = Vec::with_capacity(fields.len());
        row.push(Field::Text(utc_day(*day)));
        row.extend(grid.values_of(*combination).map(Field::Text));
        for (metric, tally) in metrics.iter().zip(tallies.iter()) {
            if !budget.spend() {
                break;
            }
            let volume = tally.get(&(*combination, *day)).copied().unwrap_or_default();
            row.push(match metric.rate {
                // "The rate for this metric relative to the SEND metric volume":
                // a day whose activity sent nothing has no volume to be relative
                // to, so its rate is zero rather than undefined.
                true if *sent > 0 => Field::Rate(volume as f64 / *sent as f64),
                true => Field::Rate(0.0),
                false => Field::Volume(volume),
            });
        }
        rows.push(row);
    }

    RecordSet {
        fields,
        rows,
        // The records the run read: the account's accepted messages inside the
        // requested interval, whatever dimension combination each belongs to.
        processed: store
            .sent_messages
            .iter()
            .filter(|message| within(message.sent_at, start, end))
            .count(),
    }
}

/// The grid a metrics export reports over: the dimensions it names, each with the
/// values it names for that dimension.
///
/// A metrics export names dimension VALUES ("a mapping between a MetricDimensionName
/// and MetricDimensionValue to filter metrics by"), and a metric is reported per
/// combination of one value per dimension, because that is the activity one data point
/// counts. A combination is addressed by its INDEX — the mixed-radix number whose
/// digits are the value positions, most significant first — so a record is placed into
/// a combination without building one, and a request naming no dimension at all has
/// the single combination `0`, the account's whole activity.
struct Grid {
    /// The dimensions, ordered by name so two exports naming the same dimensions write
    /// the same fields in the same order whatever order the request's map arrived in,
    /// each with the values the request listed, in that order.
    dimensions: Vec<(String, Vec<String>)>,
    /// How many combinations one step of each dimension's digit is worth.
    strides: Vec<usize>,
    /// How many combinations the grid has, which is the product of the value counts.
    combinations: usize,
}

impl Grid {
    /// The grid `source` names.
    ///
    /// A dimension the request names no value for selects nothing, so it is left out
    /// rather than collapsing the grid to no combination at all; the model bounds the
    /// values at one to ten per dimension and the dimensions at three.
    fn of(source: &crate::ops::types::MetricsDataSource) -> Self {
        let dimensions: Vec<(String, Vec<String>)> = source
            .dimensions
            .iter()
            .filter(|(_, values)| !values.is_empty())
            .map(|(name, values)| (name.clone(), values.clone()))
            .collect::<BTreeMap<String, Vec<String>>>()
            .into_iter()
            .collect();
        // The last dimension's digit is the least significant, so its stride is one
        // and each earlier one multiplies by the counts to its right. Every step
        // saturates, so no product of a stored data source can wrap.
        let mut strides = vec![1_usize; dimensions.len()];
        let mut combinations = 1_usize;
        for (at, (_, values)) in dimensions.iter().enumerate().rev() {
            if let Some(stride) = strides.get_mut(at) {
                *stride = combinations;
            }
            combinations = combinations.saturating_mul(values.len());
        }
        Self {
            dimensions,
            strides,
            combinations,
        }
    }

    /// The values of the combination `combination`, in dimension order — the cells a
    /// record of that combination carries.
    fn values_of(&self, combination: usize) -> impl Iterator<Item = String> + '_ {
        self.dimensions
            .iter()
            .zip(self.strides.iter())
            .map(move |((_, values), stride)| {
                // The values are non-empty by construction and a stride is at least
                // one, so the remainder names one of them.
                let digit = combination
                    .checked_div(*stride)
                    .unwrap_or_default()
                    .checked_rem(values.len())
                    .unwrap_or_default();
                values.get(digit).cloned().unwrap_or_default()
            })
    }

    /// How many events of `metric` each combination of this grid holds on each UTC day
    /// of the window, keyed by `(combination, day)`.
    ///
    /// ONE pass over the records the metric counts: each record is tested against the
    /// values of each dimension and placed into the combinations those matches name, so
    /// no combination that holds nothing costs a pass of its own. A record matching
    /// several values of one dimension belongs to each of them, which is why the digits
    /// are enumerated rather than assumed unique.
    fn tally(
        &self,
        store: &Store,
        metric: &str,
        start: f64,
        end: f64,
        budget: &mut Budget,
    ) -> BTreeMap<(usize, i64), i64> {
        let mut counts: BTreeMap<(usize, i64), i64> = BTreeMap::new();
        for counted in super::metric_counted(store, metric, start, end) {
            if !budget.spend() {
                break;
            }
            for combination in self.place(store, &counted) {
                if !budget.spend() {
                    break;
                }
                let key = (combination, counted.day);
                // The file holds at most `MAX_EXPORT_FILE_RECORDS` records, so a tally
                // past that many positions can produce no further record: the counts
                // already keyed keep accumulating and no new position is opened.
                if counts.len() >= MAX_EXPORT_FILE_RECORDS && !counts.contains_key(&key) {
                    continue;
                }
                *counts.entry(key).or_default() += 1;
            }
        }
        counts
    }

    /// The combinations `counted` belongs to: those whose value for every dimension is
    /// one the record matches. Empty when any dimension matches none of its values.
    fn place(&self, store: &Store, counted: &super::CountedMessage<'_>) -> Vec<usize> {
        let mut combinations = vec![0_usize];
        for ((name, values), stride) in self.dimensions.iter().zip(self.strides.iter()) {
            let matched: Vec<usize> = values
                .iter()
                .enumerate()
                .filter(|(_, value)| super::dimension_keeps(store, name, value, counted))
                .map(|(at, _)| at)
                .collect();
            if matched.is_empty() {
                return Vec::new();
            }
            combinations = combinations
                .iter()
                .flat_map(|base| {
                    matched
                        .iter()
                        .map(move |at| base.saturating_add(at.saturating_mul(*stride)))
                })
                .filter(|combination| *combination < self.combinations)
                .collect();
        }
        combinations
    }
}

/// One metric a metrics export asks for: the metric the records are counted for, the
/// field the export writes it in, and whether it is a rate rather than a volume.
struct RequestedMetric {
    /// `ExportMetric$Name`, one of the modelled `Metric` values.
    name: String,
    /// The field this metric is written in: the metric's name and the aggregation
    /// applied to it, so a request asking for both aggregations of one metric gets a
    /// field for each.
    field: String,
    /// Whether `ExportMetric$Aggregation` is `RATE`.
    rate: bool,
}

/// The metrics `source` asks for, in the order it lists them, at most
/// [`MAX_EXPORT_METRICS`] of them.
///
/// `ExportMetric$Name` is optional in the model, so an entry that names no metric
/// selects nothing to count and is left out rather than written as an unnamed field.
fn requested_metrics(source: &crate::ops::types::MetricsDataSource) -> Vec<RequestedMetric> {
    source
        .metrics
        .iter()
        .take(MAX_EXPORT_METRICS)
        .filter_map(|metric| {
            let name = metric.name.clone()?;
            let aggregation = metric
                .aggregation
                .clone()
                .unwrap_or_else(|| AGGREGATION_VOLUME.to_string());
            let rate = aggregation == AGGREGATION_RATE;
            Some(RequestedMetric {
                field: format!("{name}_{aggregation}"),
                name,
                rate,
            })
        })
        .collect()
}

/// What one run of an export may still read, in record visits.
///
/// The bound is [`MAX_EXPORT_FILE_VISITS`]; a render that spends it stops reading, and
/// writes the records it got to.
struct Budget(usize);

impl Budget {
    /// A fresh budget for one file.
    fn new() -> Self {
        Self(MAX_EXPORT_FILE_VISITS)
    }

    /// Spend one visit, answering whether there was one left to spend.
    fn spend(&mut self) -> bool {
        match self.0.checked_sub(1) {
            Some(left) => {
                self.0 = left;
                true
            }
            None => false,
        }
    }
}

/// The UTC day the instant `epoch_seconds` falls in, as `YYYY-MM-DD`.
///
/// The calendar conversion is the maintained library's; only the slice is taken here.
fn utc_day(epoch_seconds: i64) -> String {
    const DAY_LENGTH: usize = "YYYY-MM-DD".len();
    ::aws_smithy_types::DateTime::from_secs(epoch_seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .map(|rendered| rendered.chars().take(DAY_LENGTH).collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The Message Insights export
// ---------------------------------------------------------------------------

/// The records a Message Insights export holds: one per recipient of each message the
/// account's outbound log holds inside the requested interval — the same per-recipient
/// view `GetMessageInsights` reports for one message — kept or dropped by the
/// request's own filters, oldest first, at most `MaxResults` of them.
fn message_insights_records(
    store: &Store,
    source: &crate::ops::types::MessageInsightsDataSource,
) -> RecordSet {
    let start = source.start_date.as_secs_f64();
    let end = source.end_date.as_secs_f64();
    let limit = source
        .max_results
        .and_then(|max| usize::try_from(max).ok())
        .unwrap_or(MESSAGE_INSIGHTS_DEFAULT_MAX_RESULTS)
        .min(MAX_EXPORT_FILE_RECORDS);

    let mut processed = 0_usize;
    let mut rows: Vec<Vec<Field>> = Vec::new();
    // One budget for the whole render, so the records this file reads are capped
    // however many recipients the logged messages were addressed to.
    let mut budget = Budget::new();
    for message in &store.sent_messages {
        // "The start date is inclusive … the end date is inclusive": a message
        // accepted outside the interval is not part of this export at all.
        if !within(message.sent_at, start, end) {
            continue;
        }
        for recipient in message.recipients() {
            if !budget.spend() {
                break;
            }
            processed += 1;
            let record = InsightRecord::of(message, recipient);
            // "Filters for results to be included in the export file" and "…to be
            // excluded": a record is kept when it matches everything `Include` names
            // and nothing `Exclude` does.
            if !record.matches(source.include.as_ref())
                || record.excluded_by(source.exclude.as_ref())
            {
                continue;
            }
            if rows.len() >= limit {
                continue;
            }
            rows.push(record.row());
        }
    }

    RecordSet {
        fields: MESSAGE_INSIGHTS_FIELDS
            .iter()
            .map(|field| (*field).to_string())
            .collect(),
        rows,
        processed,
    }
}

/// One Message Insights record: one recipient of one accepted message, with every
/// value a filter selects on read off the log.
struct InsightRecord<'a> {
    /// The message this record is one recipient of.
    message: &'a super::SentMessage,
    /// The recipient, as the send request spelled it.
    recipient: &'a str,
}

impl<'a> InsightRecord<'a> {
    /// The record for `recipient` of `message`.
    fn of(message: &'a super::SentMessage, recipient: &'a str) -> Self {
        Self { message, recipient }
    }

    /// The `From` address the message was accepted for.
    fn sender_address(&self) -> &str {
        self.message.from.as_deref().unwrap_or_default()
    }

    /// The subject the message was accepted with, which a raw MIME document carries
    /// inside itself rather than in the log.
    fn subject(&self) -> &str {
        self.message.subject.as_deref().unwrap_or_default()
    }

    /// The provider that hosts the recipient's mailbox.
    ///
    /// Absent from every record, as the per-recipient insights view of the same message
    /// reports it ([`super::SentMessage::insights`]): the provider is not observable
    /// without a delivery attempt, and nothing in this slice makes one. The `Isp`
    /// FILTER still selects on the recipient's own address, the way the `ISP` metric
    /// dimension names a provider — what a filter may select by is not what the records
    /// observed.
    fn isp(&self) -> &'static str {
        ""
    }

    /// The last delivery-related event of the message, "where the ordering is as
    /// follows: SEND < BOUNCE < DELIVERY < COMPLAINT".
    ///
    /// Nothing in this slice delivers a message, bounces one or receives a complaint
    /// about one, so the acceptance is the last delivery event every record carries —
    /// the one event `GetMessageInsights` reports for the same message.
    fn last_delivery_event(&self) -> &'static str {
        super::EVENT_TYPE_SEND
    }

    /// The last engagement-related event of the message, `OPEN` or `CLICK`.
    ///
    /// Nothing here observes a recipient reading a message, so no record carries one.
    fn last_engagement_event(&self) -> &'static str {
        ""
    }

    /// The tenant the message was sent through, which no message in the log was: the
    /// send operations answer a request carrying a `TenantName` as unwritten.
    fn tenant_name(&self) -> &'static str {
        ""
    }

    /// This record as the export writes it, in [`MESSAGE_INSIGHTS_FIELDS`] order.
    fn row(&self) -> Vec<Field> {
        vec![
            Field::Text(self.message.message_id.clone()),
            Field::Text(instant(self.message.sent_at)),
            Field::Text(self.sender_address().to_string()),
            Field::Text(self.subject().to_string()),
            Field::Text(self.recipient.to_string()),
            Field::Text(self.isp().to_string()),
            Field::Text(self.tenant_name().to_string()),
            Field::Text(self.last_delivery_event().to_string()),
            Field::Text(self.last_engagement_event().to_string()),
        ]
    }

    /// Whether this record matches every filter `filters` names.
    ///
    /// "If you specify multiple filters, the filters are joined by AND. If you specify
    /// multiple values for a filter, the values are joined by OR." A filter list that
    /// is absent or empty names nothing, so it constrains nothing.
    fn matches(&self, filters: Option<&crate::ops::types::MessageInsightsFilters>) -> bool {
        self.filter_matches(filters, true)
    }

    /// Whether this record matches every filter `filters` names, which is what
    /// `Exclude` drops it for: the members it names are joined by AND, as `Include`'s
    /// are. A filter list naming nothing excludes nothing.
    fn excluded_by(&self, filters: Option<&crate::ops::types::MessageInsightsFilters>) -> bool {
        self.filter_matches(filters, false)
    }

    /// Whether this record satisfies `filters`, where `unfiltered` is the answer for a
    /// filter list that names nothing at all.
    fn filter_matches(
        &self,
        filters: Option<&crate::ops::types::MessageInsightsFilters>,
        unfiltered: bool,
    ) -> bool {
        let Some(filters) = filters else {
            return unfiltered;
        };
        // Each entry is (the values the filter names, the value of this record it
        // selects on, how the two are compared).
        let named: [(Option<&Vec<String>>, String, Match); 7] = [
            (
                filters.from_email_address.as_ref(),
                self.sender_address().to_string(),
                Match::Partial,
            ),
            (
                filters.destination.as_ref(),
                self.recipient.to_string(),
                Match::Partial,
            ),
            (
                filters.subject.as_ref(),
                self.subject().to_string(),
                Match::Partial,
            ),
            (filters.isp.as_ref(), self.recipient.to_string(), Match::Isp),
            (
                filters.tenant_name.as_ref(),
                self.tenant_name().to_string(),
                Match::Exact,
            ),
            (
                filters.last_delivery_event.as_ref(),
                self.last_delivery_event().to_string(),
                Match::Exact,
            ),
            (
                filters.last_engagement_event.as_ref(),
                self.last_engagement_event().to_string(),
                Match::Exact,
            ),
        ];
        let mut constrained = false;
        for (values, value, compared) in named {
            let values = values.map(Vec::as_slice).unwrap_or_default();
            if values.is_empty() {
                continue;
            }
            constrained = true;
            let matched = values
                .iter()
                .any(|named| compared.matches(named, &value));
            if !matched {
                return false;
            }
        }
        if constrained {
            true
        } else {
            unfiltered
        }
    }
}

/// How one filter's values are compared with the value of the record they select on.
#[derive(Debug, Clone, Copy)]
enum Match {
    /// The members the model documents the `*` wildcard for — `FromEmailAddress`,
    /// `Destination` and `Subject`.
    Partial,
    /// Every other member, which compares exactly.
    Exact,
    /// `Isp`, which names a mailbox provider rather than a value the log stores, so
    /// it is compared against the recipient's own address the way the `ISP` metric
    /// dimension names a provider: by the mail domain it belongs to, with or without
    /// that domain's public suffix.
    Isp,
}

impl Match {
    /// Whether the filter value `named` selects the record value `value`.
    ///
    /// "Filter values are case-sensitive", so every comparison but the provider's —
    /// which is a DNS name, compared the way DNS compares one — is case-sensitive.
    fn matches(self, named: &str, value: &str) -> bool {
        match self {
            Self::Partial => matches_filter_value(named, value),
            Self::Exact => named == value,
            // `value` is the recipient's address here, which is what names the
            // provider that hosts its mailbox.
            Self::Isp => super::serves_recipient(named, value),
        }
    }
}

/// Whether `value` satisfies the filter value `pattern`.
///
/// "A partial match is performed by using the `*` wildcard character placed at the
/// beginning (suffix match), the end (prefix match) or both ends of the string
/// (contains match). In order to match the literal characters `*` or `\`, they must be
/// escaped using the `\` character. If no wildcard character is present, an exact
/// match is performed."
fn matches_filter_value(pattern: &str, value: &str) -> bool {
    // Both ends are stripped by the pattern's own prefix and suffix, so no index into
    // caller-supplied text is taken anywhere here.
    let (leading, body) = match pattern.strip_prefix('*') {
        Some(rest) => (true, rest),
        None => (false, pattern),
    };
    let (trailing, body) = match strip_wildcard_suffix(body) {
        Some(rest) => (true, rest),
        None => (false, body),
    };
    let wanted = unescape(body);
    match (leading, trailing) {
        (true, true) => value.contains(&wanted),
        (true, false) => value.ends_with(&wanted),
        (false, true) => value.starts_with(&wanted),
        (false, false) => value == wanted,
    }
}

/// `pattern` without the trailing `*` that stands for the wildcard, or `None` when it
/// ends in no such `*`.
///
/// A `*` stands for the wildcard rather than for the literal character when an EVEN
/// number of backslashes precedes it, because an escaping backslash is itself escaped
/// by one.
fn strip_wildcard_suffix(pattern: &str) -> Option<&str> {
    let body = pattern.strip_suffix('*')?;
    let escapes = body.chars().rev().take_while(|c| *c == '\\').count();
    (escapes % 2 == 0).then_some(body)
}

/// `pattern` with its escapes resolved: `\x` stands for `x`, and a trailing backslash
/// stands for itself.
fn unescape(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(escaped) => out.push(escaped),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether `at` falls inside the interval `start..=end` (seconds since the epoch),
/// both bounds inclusive.
fn within(at: SystemTime, start: f64, end: f64) -> bool {
    let Ok(since_epoch) = at.duration_since(SystemTime::UNIX_EPOCH) else {
        return false;
    };
    let seconds = since_epoch.as_secs_f64();
    seconds >= start && seconds <= end
}

/// `at` as an export record carries an instant: the RFC 3339 form every timestamp
/// this service reports to a client is rendered in.
fn instant(at: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}
