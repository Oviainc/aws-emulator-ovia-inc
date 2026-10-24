// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-forecast-amazonforecast service.
//!
//! The dataset / dataset-group control plane is stored here, plus the handful of
//! helpers every handler that touches it needs: ARN minting, the SigV4-scope region,
//! and the opaque `NextToken` cursor codec the `@paginated` List operations share.
//! Keeping them in one place is what makes two operations read ONE copy of a record.
//!
//! # The forecast chain, and where its data goes
//!
//! A forecast is the END of a chain this store holds every link of: a dataset carries
//! the history a `CreateDatasetImportJob` read out of object storage
//! ([`Dataset::series`]), a predictor carries the model fitted to that history
//! ([`Predictor::fitted`]), and a forecast is that model run forward over the
//! predictor's horizon. [`series`] is the one place that reading, bucketing, fitting
//! and stepping happen, so two operations cannot disagree about them.
//!
//! A forecast is ALSO a data-plane resource: `QueryForecast` lives in another service
//! (`forecastquery`, Amazon Forecast Runtime) reading another store on the SAME shared
//! engine. This control plane is the only thing that mints a `ForecastArn`, so it is
//! the only thing that can publish one: [`publish_forecast`] writes the computed
//! forecast into that store and [`withdraw_forecast`] takes it out again, which is
//! what makes a created forecast queryable and a deleted one stop being queryable.

use std::collections::BTreeMap;
use std::ops::Bound;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_forecastquery_amazonforecastruntime as forecastquery;

use crate::ops::types::{
    DataConfig, EncryptionConfig, Schema, Statistics, Tag, TimeAlignmentBoundary,
    TimeSeriesSelector,
};

pub mod series;

use self::series::{FittedSeries, Frequency, ImportedSeries};

/// The `Status` a dataset or dataset group carries. Forecast creates both
/// synchronously and `DescribeDataset`/`DescribeDatasetGroup` report them `ACTIVE`
/// from the first read ("The `Status` of a dataset must be `ACTIVE` before you can
/// import training data" — CreateDataset's own documentation), so there is no
/// clock-owed transition to materialise here.
pub const ACTIVE: &str = "ACTIVE";

/// This service's ARN namespace / SigV4 signing name
/// (`smithy_query(query="service_info")`: arnNamespace `forecast`).
pub const ARN_SERVICE: &str = "forecast";

/// The region an ARN is minted in when the request carries no SigV4 credential
/// scope to read one from (an anonymous call routed by endpoint).
pub const DEFAULT_REGION: &str = "us-east-1";

/// Page size for a List call that names no `MaxResults`: the modelled maximum.
/// `MaxResults` is `range 1..100` on both `ListDatasets` and `ListDatasetGroups`
/// (`smithy_query(query="operation",operation="ListDatasets")`), and the model
/// declares no default, so the largest page the member admits is used.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// The most datasets this emulator retains for one account.
///
/// Every `CreateDataset` is a growth site a caller drives — a fresh `DatasetName`
/// inserts a record holding that caller's `Schema`, `EncryptionConfig` and up to the
/// modelled 200 `Tags` — so the store carries an explicit ceiling: past it the create
/// is REFUSED with the modelled `LimitExceededException` ("The limit on the number of
/// resources per account has been exceeded", HTTP 409) rather than inserting, so
/// emulator memory cannot be grown without bound. The number is CHOSEN for the
/// emulator: Forecast publishes its own per-account quota, and the point of the
/// constant is that the growth site has a ceiling at all, answered with the error AWS
/// answers a quota with.
pub const MAX_DATASETS: usize = 500;

/// The most dataset groups this emulator retains for one account, for the reason
/// [`MAX_DATASETS`] gives and refused the same way.
pub const MAX_DATASET_GROUPS: usize = 200;

/// The most `DatasetArns` one dataset group's membership may name.
///
/// `ArnList` carries no modelled `@length`, so nothing upstream clamps the element
/// count of a caller-supplied membership before the handler walks it under the shared
/// state lock. This ceiling is checked BEFORE that walk, on both
/// `CreateDatasetGroup` and `UpdateDatasetGroup`. Also chosen for the emulator; it is
/// far above any real membership, which names at most one dataset per `DatasetType`.
pub const MAX_DATASET_ARNS_PER_GROUP: usize = 50;

/// Cursor kind tag for `ListDatasets`, so a dataset-group token cannot be replayed
/// against the dataset listing.
pub const DATASET_CURSOR: &str = "dataset";

/// Cursor kind tag for `ListDatasetGroups`.
pub const DATASET_GROUP_CURSOR: &str = "dataset-group";

/// A dataset exactly as `CreateDataset` wrote it: every member
/// `DescribeDataset` / `ListDatasets` reports is read back from here.
#[derive(Debug, Clone)]
pub struct Dataset {
    pub arn: String,
    pub name: String,
    pub domain: String,
    pub dataset_type: String,
    pub data_frequency: Option<String>,
    pub schema: Schema,
    pub encryption_config: Option<EncryptionConfig>,
    pub status: String,
    pub creation_time: SystemTime,
    pub last_modification_time: SystemTime,
    /// The tags `CreateDataset` carried. Nothing reads them yet: `DescribeDataset` and
    /// `DatasetSummary` model no `Tags` member and `ListTagsForResource` is still a 501
    /// stub with its own gap issue, so they are stored for that operation to report.
    pub tags: Vec<Tag>,
    /// The history imported into this dataset: one entry per time series, as the rows
    /// of the import files a `CreateDatasetImportJob` read. EMPTY until an import job
    /// runs — nothing else writes it, because nothing else has data.
    pub series: Vec<ImportedSeries>,
    /// The import jobs whose data this dataset holds, in the order they ran. Reported
    /// by `DescribeAutoPredictor`'s `DatasetImportJobArns` for the predictor trained on
    /// them.
    pub import_job_arns: Vec<String>,
}

/// A dataset group. `dataset_arns` is the membership last written by
/// `CreateDatasetGroup` or `UpdateDatasetGroup`, and is what `DescribeDatasetGroup`
/// reports verbatim — including an ARN whose dataset has since been deleted, because
/// "Forecast does not automatically update any dataset groups that contain the deleted
/// dataset" (`DeleteDataset`'s modelled documentation).
#[derive(Debug, Clone)]
pub struct DatasetGroup {
    pub arn: String,
    pub name: String,
    pub domain: String,
    pub dataset_arns: Vec<String>,
    pub status: String,
    pub creation_time: SystemTime,
    pub last_modification_time: SystemTime,
    /// As on [`Dataset`]: stored for `ListTagsForResource`, read by nothing yet.
    pub tags: Vec<Tag>,
}

/// Per-service state for com-amazonaws-forecast-amazonforecast. A handler reaches it through [`service_state`].
///
/// Both maps are keyed by ARN and ordered, so the `@paginated` List operations walk a
/// stable sequence and a `NextToken` names a position in it rather than an index that
/// a concurrent create would shift. The ARN carries the resource's region and name, so
/// the key is also the uniqueness constraint a repeated Create trips over: Forecast
/// resources are regional, and the same name signed for another region is another
/// resource with another ARN.
///
/// Both maps are BOUNDED: a create past [`MAX_DATASETS`] / [`MAX_DATASET_GROUPS`] is
/// refused with the modelled `LimitExceededException` instead of inserting, and a
/// group's membership is refused past [`MAX_DATASET_ARNS_PER_GROUP`], so no sequence
/// of requests grows this state without limit.
#[derive(Debug, Default)]
pub struct ComAmazonawsForecastAmazonforecastState {
    /// Datasets by `DatasetArn`.
    pub datasets: BTreeMap<String, Dataset>,
    /// Dataset groups by `DatasetGroupArn`.
    pub dataset_groups: BTreeMap<String, DatasetGroup>,
    /// Dataset import jobs by `DatasetImportJobArn`.
    pub dataset_import_jobs: BTreeMap<String, DatasetImportJob>,
    /// Predictors by `PredictorArn`, each carrying the model fitted when it was
    /// created.
    pub predictors: BTreeMap<String, Predictor>,
    /// Forecasts by `ForecastArn`. The PREDICTIONS live in the runtime store this
    /// control plane publishes them into ([`publish_forecast`]); what is kept here is
    /// the control-plane record `DescribeForecast` reports.
    pub forecasts: BTreeMap<String, Forecast>,
    /// What-if analyses by `WhatIfAnalysisArn`.
    pub what_if_analyses: BTreeMap<String, WhatIfAnalysis>,
    /// What-if forecasts by `WhatIfForecastArn`, published into the runtime store's
    /// own what-if map ([`publish_what_if_forecast`]).
    pub what_if_forecasts: BTreeMap<String, WhatIfForecast>,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsForecastAmazonforecastState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsForecastAmazonforecastState>)
}

/// The ARN of the dataset named `name` in `region`.
pub fn dataset_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("dataset/{name}"))
}

/// The ARN of the dataset group named `name` in `region`.
pub fn dataset_group_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("dataset-group/{name}"))
}

/// The region this request was signed for, from its SigV4 credential scope —
/// the same signal the router uses to pick the service — falling back to
/// [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The page size a List call serves: its `MaxResults` when it names one (the model
/// already refused anything outside 1..100), else [`DEFAULT_PAGE_SIZE`].
pub fn page_size(max_results: Option<i32>) -> usize {
    max_results
        .filter(|requested| *requested > 0)
        .map_or(DEFAULT_PAGE_SIZE, |requested| requested as usize)
}

/// Encode an opaque `NextToken` naming the last key served on this page.
///
/// Hex rather than the raw key so the token is opaque to a client — it carries a
/// version and the listing it belongs to, which is what lets [`decode_next_token`]
/// reject a token minted for the other listing.
pub fn encode_next_token(cursor: &str, last_key: &str) -> String {
    ::hex::encode(format!("forecast:v1:{cursor}:{last_key}"))
}

/// The key a `NextToken` resumes after, or `None` when the token was not minted by
/// [`encode_next_token`] for this listing — which the caller answers with
/// `InvalidNextTokenException`.
pub fn decode_next_token(cursor: &str, token: &str) -> Option<String> {
    let decoded = ::hex::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    text.strip_prefix(&format!("forecast:v1:{cursor}:"))
        .map(str::to_string)
}

/// One page of an ARN-ordered map: the entries after `after` (exclusive), at most
/// `limit` of them, plus the key to resume from when more entries remain.
///
/// The cursor is a KEY, not an offset, so a create or delete between two pages
/// neither skips nor repeats an entry the caller has not seen.
pub fn page<V>(
    items: &BTreeMap<String, V>,
    after: Option<String>,
    limit: usize,
) -> (Vec<&V>, Option<String>) {
    let lower = match after {
        Some(key) => Bound::Excluded(key),
        None => Bound::Unbounded,
    };
    let mut remaining = items.range((lower, Bound::Unbounded));
    let mut served = Vec::new();
    let mut last_key = None;
    for (key, value) in remaining.by_ref().take(limit) {
        served.push(value);
        last_key = Some(key.clone());
    }
    // A token is emitted only when there IS a further entry, so the last page omits
    // `NextToken` instead of handing back a cursor onto an empty page.
    let next = remaining.next().and(last_key);
    (served, next)
}

// ---------------------------------------------------------------------------
// The forecast chain: import jobs, predictors and forecasts
// ---------------------------------------------------------------------------

/// The `Status` Amazon Forecast answers a submitted import job, predictor or forecast
/// with: "CREATE_PENDING, CREATE_IN_PROGRESS, CREATE_FAILED" then `ACTIVE`
/// (`DescribeForecast$Status`).
pub const CREATE_PENDING: &str = "CREATE_PENDING";
/// The status a submission carries while its run is being processed (see
/// [`CREATE_PENDING`]).
pub const CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
/// The terminal status of a run that could not be carried out (see
/// [`CREATE_PENDING`]).
pub const CREATE_FAILED: &str = "CREATE_FAILED";

/// How long a submission stays `CREATE_PENDING` before the clock owes it
/// `CREATE_IN_PROGRESS`.
///
/// CHOSEN for the emulator: Forecast takes minutes to hours, and what matters is that
/// the transient state EXISTS, so a caller that queries immediately after creating
/// sees the status error the real service answers. Together with [`RUN_SPAN`] the whole
/// transition is owed 700 ms after the call — the long end of the band this emulator
/// uses for a transition the clock owes, so the transient state is still observable on
/// a loaded host.
pub const PENDING_SPAN: Duration = Duration::from_millis(300);

/// How long a submission stays `CREATE_IN_PROGRESS` before the clock owes it its
/// terminal status (see [`PENDING_SPAN`]).
pub const RUN_SPAN: Duration = Duration::from_millis(400);

/// The most import jobs, predictors and forecasts this emulator retains for one
/// account, refused past the limit with the modelled `LimitExceededException` for the
/// reason [`MAX_DATASETS`] gives.
pub const MAX_DATASET_IMPORT_JOBS: usize = 200;
/// See [`MAX_DATASET_IMPORT_JOBS`].
pub const MAX_PREDICTORS: usize = 200;
/// See [`MAX_DATASET_IMPORT_JOBS`].
pub const MAX_FORECASTS: usize = 200;
/// See [`MAX_DATASET_IMPORT_JOBS`].
pub const MAX_WHAT_IF_ANALYSES: usize = 200;
/// See [`MAX_DATASET_IMPORT_JOBS`].
pub const MAX_WHAT_IF_FORECASTS: usize = 200;

/// The largest `ForecastHorizon` a predictor may be created with: "The maximum
/// forecast horizon is the lesser of 500 time-steps or 1/4 of the TARGET_TIME_SERIES
/// dataset length. If you are retraining an existing AutoPredictor, then the maximum
/// forecast horizon is the lesser of 500 time-steps or 1/3 of the TARGET_TIME_SERIES
/// dataset length" (`CreateAutoPredictor$ForecastHorizon`). The other half of that
/// rule is enforced against the fitted history, which is the dataset length this
/// emulator holds.
pub const MAX_FORECAST_HORIZON: i32 = 500;

/// A NEW predictor's horizon may be at most this fraction of its training history —
/// 1/4, from the sentence quoted on [`MAX_FORECAST_HORIZON`].
pub const NEW_PREDICTOR_HISTORY_RATIO: usize = 4;

/// A RETRAINED predictor's horizon may be at most this fraction of its training
/// history — 1/3, from the same sentence.
pub const RETRAINED_PREDICTOR_HISTORY_RATIO: usize = 3;

/// The forecast types a predictor is created with when the request names none: "If you
/// didn't specify quantiles, the default values are ["0.1", "0.5", "0.9"]"
/// (`CreateForecast$ForecastTypes`).
pub const DEFAULT_FORECAST_TYPES: [&str; 3] = ["0.1", "0.5", "0.9"];

/// The most forecast types one forecast may report: the modelled `@length` maximum of
/// `ForecastTypes` (1..20). Amazon Forecast's own guidance is narrower ("You can
/// currently specify up to 5 quantiles per forecast",
/// `CreateForecast$ForecastTypes`), but the model is what a request is judged against —
/// and this ceiling also bounds a list a forecast INHERITED from its predictor, which
/// no boundary walk sees.
pub const MAX_FORECAST_TYPES: usize = 20;

/// The `ImportMode` an import job runs in when the request names none: "If not
/// specified, this defaults to FULL" (`CreateDatasetImportJob$ImportMode`).
pub const DEFAULT_IMPORT_MODE: &str = "FULL";

/// The `Format` an import file is read as when the request names none: "The format of
/// the imported data, CSV or PARQUET. The default value is CSV"
/// (`CreateDatasetImportJob$Format`).
pub const DEFAULT_IMPORT_FORMAT: &str = "CSV";

/// The dataset type a predictor trains on: "Creates a forecast for each item in the
/// TARGET_TIME_SERIES dataset that was used to train the predictor"
/// (`CreateForecast`).
pub const TARGET_TIME_SERIES: &str = "TARGET_TIME_SERIES";

/// Where a submitted run has got to, as a function of the CLOCK alone.
///
/// Every status this service reports for an import job, a predictor or a forecast is
/// read from here, so no read has to write anything to move a resource on: the record
/// stores WHEN each transition becomes due and the status is computed from `now`. A
/// run's OUTCOME is decided by the work the submitting request actually did —
/// `failure` is the reason it could not be carried out — and is simply not revealed
/// until the clock owes it.
#[derive(Debug, Clone)]
pub struct Lifecycle {
    pub created_at: SystemTime,
    /// When `CREATE_IN_PROGRESS` becomes due.
    pub in_progress_at: SystemTime,
    /// When the terminal status becomes due.
    pub settled_at: SystemTime,
    /// Why the run could not be carried out, or `None` for one that was.
    pub failure: Option<String>,
}

impl Lifecycle {
    /// A run submitted at `now` that was carried out.
    pub fn succeeded(now: SystemTime) -> Self {
        Self {
            created_at: now,
            in_progress_at: now + PENDING_SPAN,
            settled_at: now + PENDING_SPAN + RUN_SPAN,
            failure: None,
        }
    }

    /// A run submitted at `now` that could not be carried out, and why.
    pub fn failed(now: SystemTime, reason: impl Into<String>) -> Self {
        Self {
            failure: Some(reason.into()),
            ..Self::succeeded(now)
        }
    }

    /// The `Status` this run reports at `now`.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.in_progress_at {
            CREATE_PENDING
        } else if now < self.settled_at {
            CREATE_IN_PROGRESS
        } else if self.failure.is_some() {
            CREATE_FAILED
        } else {
            ACTIVE
        }
    }

    /// The `Message` this run reports at `now`: the failure reason, and only once the
    /// clock owes the terminal status — a run still being processed has no outcome to
    /// report.
    pub fn message(&self, now: SystemTime) -> Option<String> {
        (now >= self.settled_at)
            .then(|| self.failure.clone())
            .flatten()
    }

    /// Whether this run has reached `ACTIVE` at `now`, which is what every operation
    /// that consumes the resource requires.
    pub fn is_active(&self, now: SystemTime) -> bool {
        now >= self.settled_at && self.failure.is_none()
    }

    /// Whether the clock owes this run its terminal status at `now`.
    pub fn is_settled(&self, now: SystemTime) -> bool {
        now >= self.settled_at
    }

    /// The instant of the last transition the clock has made due at `now`, which is
    /// what `LastModificationTime` reports ("The last time the resource was modified.
    /// The timestamp depends on the status of the job").
    pub fn last_modification_time(&self, now: SystemTime) -> SystemTime {
        if now >= self.settled_at {
            self.settled_at
        } else if now >= self.in_progress_at {
            self.in_progress_at
        } else {
            self.created_at
        }
    }

    /// `EstimatedTimeRemainingInMinutes` at `now`: the whole minutes left of the span
    /// still owed, which is zero for this emulator's sub-second spans, and nothing at
    /// all once the run has settled.
    pub fn estimated_minutes_remaining(&self, now: SystemTime) -> Option<i64> {
        let remaining = self.settled_at.duration_since(now).ok()?;
        i64::try_from(remaining.as_secs() / 60).ok()
    }
}

/// A dataset import job exactly as `CreateDatasetImportJob` wrote it. The DATA it read
/// lives on the dataset ([`Dataset::series`]); what is here is the submission and its
/// outcome.
#[derive(Debug, Clone)]
pub struct DatasetImportJob {
    pub arn: String,
    pub name: String,
    pub dataset_arn: String,
    pub data_source: crate::ops::types::DataSource,
    pub timestamp_format: String,
    pub time_zone: Option<String>,
    pub use_geolocation_for_time_zone: bool,
    pub geolocation_format: Option<String>,
    pub format: String,
    pub import_mode: String,
    /// The size of the data read, in gigabytes, as `DataSize` documents it ("The size
    /// of the dataset in gigabytes (GB) after the import job has finished").
    pub data_size: Option<f64>,
    /// The statistics of each field of the rows that were read — computed from them,
    /// not declared.
    pub field_statistics: BTreeMap<String, Statistics>,
    pub lifecycle: Lifecycle,
    pub tags: Vec<Tag>,
}

/// A predictor and the model fitted to the history it was trained on.
#[derive(Debug, Clone)]
pub struct Predictor {
    pub arn: String,
    pub name: String,
    pub dataset_group_arn: String,
    /// The import jobs whose data the training history came from, as
    /// `DescribeAutoPredictor$DatasetImportJobArns` reports them.
    pub dataset_import_job_arns: Vec<String>,
    pub forecast_horizon: i32,
    pub forecast_frequency: String,
    pub forecast_types: Vec<String>,
    pub forecast_dimensions: Vec<String>,
    pub data_config: DataConfig,
    pub encryption_config: Option<EncryptionConfig>,
    pub reference_predictor_arn: Option<String>,
    pub optimization_metric: Option<String>,
    pub time_alignment_boundary: Option<TimeAlignmentBoundary>,
    /// The fitted model: one entry per time series of the training data. This is what
    /// a forecast runs forward, so a predictor's forecasts report the data it was
    /// trained on and nothing later.
    pub fitted: Vec<FittedSeries>,
    /// The name of the TARGET attribute the fitted history came from, as the training
    /// dataset's own `Schema` spells it (`demand`, `target_value`, …). A what-if
    /// transformation changes this emulator's forecast only when it names this
    /// attribute, which is the only one the estimator reads.
    pub target_attribute: String,
    pub lifecycle: Lifecycle,
    pub tags: Vec<Tag>,
}

/// A forecast's control-plane record: everything `DescribeForecast` reports. Its
/// predictions live in the runtime store ([`publish_forecast`]).
#[derive(Debug, Clone)]
pub struct Forecast {
    pub arn: String,
    pub name: String,
    pub predictor_arn: String,
    pub dataset_group_arn: String,
    pub forecast_types: Vec<String>,
    pub time_series_selector: Option<TimeSeriesSelector>,
    /// The fitted series this forecast was computed from — the predictor's models for
    /// the series it covers. A what-if analysis of this forecast transforms exactly
    /// these, so the what-if covers the same time series the baseline does.
    pub fitted: Vec<FittedSeries>,
    /// How many prediction points this forecast published into the runtime store — what
    /// the store-wide bound in [`forecast_points_rejection`] is counted against.
    pub points: usize,
    pub lifecycle: Lifecycle,
    pub tags: Vec<Tag>,
}

/// A what-if analysis: a baseline forecast, and the time series the analysis covers.
///
/// "A what-if analysis is a scenario modelling tool … the analysis is created from a
/// baseline forecast" — so the analysis holds the `ForecastArn` it is of, and each
/// what-if forecast created in it is that baseline with its own transformations.
#[derive(Debug, Clone)]
pub struct WhatIfAnalysis {
    pub arn: String,
    pub name: String,
    pub forecast_arn: String,
    pub time_series_selector: Option<TimeSeriesSelector>,
    /// The identifiers the selector named, read when the analysis was created; `None`
    /// when it named no selector, which covers every series of the baseline.
    pub selected: Option<Vec<BTreeMap<String, String>>>,
    pub lifecycle: Lifecycle,
    pub tags: Vec<Tag>,
}

/// A what-if forecast's control-plane record. Its predictions live in the runtime
/// store's own what-if map ([`publish_what_if_forecast`]).
#[derive(Debug, Clone)]
pub struct WhatIfForecast {
    pub arn: String,
    pub name: String,
    pub what_if_analysis_arn: String,
    /// The baseline forecast the analysis is of, which this what-if is derived from.
    pub base_forecast_arn: String,
    pub forecast_types: Vec<String>,
    pub time_series_transformations: Vec<crate::ops::types::TimeSeriesTransformation>,
    /// How many prediction points this what-if forecast published into the runtime
    /// store (as on [`Forecast::points`]).
    pub points: usize,
    pub lifecycle: Lifecycle,
    pub tags: Vec<Tag>,
}

/// Apply an import's data to the dataset it was imported into.
///
/// `FULL` replaces: "A FULL dataset import replaces all of the existing data with the
/// newly imported data". `INCREMENTAL` appends: "An INCREMENTAL import appends the
/// imported data to the existing data" (`CreateDatasetImportJob$ImportMode`), merging
/// each imported series into the stored series with the same dimensions and keeping
/// the observations ascending so the fitting walk can read them in order.
///
/// `Err` is the reason the data could not be retained, which becomes the job's
/// `CREATE_FAILED` message — and then NOTHING of the import is applied.
pub fn apply_import(
    dataset: &mut Dataset,
    data: &series::ImportedData,
    import_mode: &str,
    job_arn: &str,
) -> Result<(), String> {
    let mut merged: Vec<ImportedSeries> = if import_mode == "INCREMENTAL" {
        dataset.series.clone()
    } else {
        Vec::new()
    };
    for imported in &data.series {
        match merged
            .iter_mut()
            .find(|stored| stored.dimensions == imported.dimensions)
        {
            Some(stored) => {
                if stored.observations.len() + imported.observations.len()
                    > series::MAX_POINTS_PER_SERIES
                {
                    return Err(format!(
                        "One time series would hold more than the {} observations this \
                         emulator stores.",
                        series::MAX_POINTS_PER_SERIES
                    ));
                }
                stored.observations.extend(imported.observations.iter().copied());
                stored
                    .observations
                    .sort_by_key(|observation| observation.at);
            }
            None => {
                if merged.len() >= series::MAX_SERIES_PER_DATASET {
                    return Err(format!(
                        "The dataset would hold more than the {} time series this \
                         emulator stores.",
                        series::MAX_SERIES_PER_DATASET
                    ));
                }
                merged.push(imported.clone());
            }
        }
    }
    dataset.series = merged;
    if import_mode == "INCREMENTAL" {
        dataset.import_job_arns.push(job_arn.to_string());
    } else {
        // A FULL import replaces the data, so the dataset's history is the work of
        // exactly this one job.
        dataset.import_job_arns = vec![job_arn.to_string()];
    }
    Ok(())
}

/// The ARN of the import job named `job_name` of the dataset named `dataset_name`:
/// `dataset-import-job/<DatasetName>/<ImportJobName>`, the two-segment resource an
/// import job's ARN names.
pub fn dataset_import_job_arn(region: &str, dataset_name: &str, job_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("dataset-import-job/{dataset_name}/{job_name}"),
    )
}

/// The ARN of the predictor named `name` in `region`.
pub fn predictor_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("predictor/{name}"))
}

/// The ARN of the forecast named `name` in `region`.
pub fn forecast_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("forecast/{name}"))
}

/// The ARN of the what-if analysis named `name` in `region`.
pub fn what_if_analysis_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("what-if-analysis/{name}"))
}

/// The ARN of the what-if forecast named `forecast_name` in the analysis named
/// `analysis_name`: `what-if-forecast/<WhatIfAnalysisName>/<WhatIfForecastName>`,
/// because a what-if forecast's name is "unique within each what-if analysis"
/// (`CreateWhatIfForecast$WhatIfForecastName`) — which is also the two-segment resource
/// the runtime service resolves a `WhatIfForecastArn` as.
pub fn what_if_forecast_arn(region: &str, analysis_name: &str, forecast_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("what-if-forecast/{analysis_name}/{forecast_name}"),
    )
}

/// The Amazon Forecast RUNTIME store on the shared engine — the one
/// `QueryForecast`/`QueryWhatIfForecast` read.
///
/// `None` when this service is mounted on an engine that is not the shared
/// `InMemoryEngine`, in which case there is no runtime store to publish into and the
/// create cannot be carried out.
pub fn runtime_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<forecastquery::ComAmazonawsForecastqueryAmazonforecastruntimeState>,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                forecastquery::ComAmazonawsForecastqueryAmazonforecastruntimeState,
            >,
        )
}

/// How many prediction points a forecast of `series` time series and `forecast_types`
/// forecast types over `horizon` steps MATERIALISES.
///
/// The statistics are deduplicated, because `Predictions` is a map keyed by statistic:
/// two forecast types that name one statistic are one reported series.
pub fn forecast_point_count(series: usize, forecast_types: &[String], horizon: i32) -> usize {
    let mut statistics: Vec<String> = forecast_types
        .iter()
        .map(|forecast_type| {
            forecastquery::state::statistic_for_forecast_type(forecast_type)
        })
        .collect();
    statistics.sort();
    statistics.dedup();
    let horizon = usize::try_from(horizon.max(0)).unwrap_or_default();
    series
        .saturating_mul(statistics.len())
        .saturating_mul(horizon)
}

/// Why a forecast of `points` prediction points cannot be retained, or `None` when it
/// can.
///
/// The PRODUCT is what a create allocates and the runtime store keeps, and each of its
/// factors is bounded on its own, so the product and the store-wide total are bounded
/// here — checked BEFORE a single point is built, exactly as the import bounds are
/// checked before a single object is fetched.
pub fn forecast_points_rejection(
    store: &ComAmazonawsForecastAmazonforecastState,
    points: usize,
) -> Option<String> {
    if points > series::MAX_FORECAST_POINTS {
        return Some(format!(
            "The forecast would hold {points} prediction points, more than the {} this \
             emulator stores for one forecast. Create it over fewer time series, fewer \
             forecast types or a shorter horizon.",
            series::MAX_FORECAST_POINTS
        ));
    }
    let retained: usize = store
        .forecasts
        .values()
        .map(|forecast| forecast.points)
        .chain(
            store
                .what_if_forecasts
                .values()
                .map(|forecast| forecast.points),
        )
        .sum();
    if retained.saturating_add(points) > series::MAX_RETAINED_FORECAST_POINTS {
        return Some(format!(
            "The forecast would bring this account to {} retained prediction points, \
             more than the {} this emulator stores. Delete a forecast first.",
            retained.saturating_add(points),
            series::MAX_RETAINED_FORECAST_POINTS
        ));
    }
    None
}

/// Project a computed forecast into the runtime store's shape: one series per fitted
/// time series, each carrying every statistic the forecast was created with over the
/// predictor's horizon.
///
/// Shared by a baseline forecast and a what-if forecast — they are the same predictor
/// run over histories that differ — so the two cannot report a horizon differently.
/// `active_at` is when the creation run becomes `ACTIVE`, which is the instant the
/// runtime service may answer the forecast from: "The Status of the forecast must be
/// ACTIVE before you can query or export the forecast" (`CreateForecast`).
///
/// The caller must already have accepted the point count
/// ([`forecast_points_rejection`]): this function allocates it.
pub fn stored_predictions(
    arn: &str,
    name: &str,
    forecast_types: &[String],
    predictor: &Predictor,
    selected: &[FittedSeries],
    active_at: SystemTime,
) -> Result<forecastquery::state::StoredForecast, String> {
    let frequency = Frequency::parse(&predictor.forecast_frequency).ok_or_else(|| {
        format!(
            "The predictor's forecast frequency {} is not an interval Amazon Forecast \
             reads.",
            predictor.forecast_frequency
        )
    })?;
    let horizon = usize::try_from(predictor.forecast_horizon.max(0)).unwrap_or_default();
    let mut stored = forecastquery::state::StoredForecast::new(
        arn,
        name,
        &predictor.forecast_frequency,
        forecast_types.to_vec(),
    )
    .active_from(active_at);
    for fitted in selected {
        let timestamps = series::horizon_timestamps(fitted, &frequency, horizon)?;
        let mut stored_series =
            forecastquery::state::StoredSeries::new(fitted.dimensions.clone());
        for forecast_type in forecast_types {
            let value = series::fitted_value(fitted, forecast_type).ok_or_else(|| {
                format!("The forecast type {forecast_type} has no value for this series.")
            })?;
            // The baseline is stationary: every point of the horizon carries the
            // statistic fitted to the series' own history, so the forecast states
            // exactly what the data supports and no trend it does not.
            let points = timestamps
                .iter()
                .map(|timestamp| forecastquery::state::StoredPoint::new(timestamp, value))
                .collect::<Vec<_>>();
            stored_series = stored_series.with_statistic(
                forecastquery::state::statistic_for_forecast_type(forecast_type),
                points,
            );
        }
        stored = stored.with_series(stored_series);
    }
    Ok(stored)
}

/// Publish a computed forecast into the runtime store, so `QueryForecast` can answer
/// it. Returns `false` when there is no runtime store on this engine.
pub fn publish_forecast<Eng: Send + Sync + 'static>(
    engine: &Eng,
    stored: forecastquery::state::StoredForecast,
) -> bool {
    match runtime_state(engine) {
        Some(runtime) => {
            runtime.lock().put_forecast(stored);
            true
        }
        None => false,
    }
}

/// Withdraw a forecast from the runtime store, so the ARN a `DeleteForecast` removed
/// stops being queryable. Returns `false` when there is no runtime store on this
/// engine.
pub fn withdraw_forecast<Eng: Send + Sync + 'static>(engine: &Eng, arn: &str) -> bool {
    match runtime_state(engine) {
        Some(runtime) => {
            runtime.lock().forecasts.remove(arn);
            true
        }
        None => false,
    }
}

/// Project a computed what-if forecast into the runtime store's shape, the same way
/// [`stored_predictions`] does for a baseline — plus the baseline it is derived from,
/// which is the provenance the runtime store records.
pub fn stored_what_if_forecast(
    what_if: &WhatIfForecast,
    predictor: &Predictor,
    transformed: &[FittedSeries],
    active_at: SystemTime,
) -> Result<forecastquery::state::StoredForecast, String> {
    Ok(stored_predictions(
        &what_if.arn,
        &what_if.name,
        &what_if.forecast_types,
        predictor,
        transformed,
        active_at,
    )?
    .derived_from(&what_if.base_forecast_arn))
}

/// Publish a computed what-if forecast into the runtime store, so
/// `QueryWhatIfForecast` can answer it. Returns `false` when there is no runtime store
/// on this engine.
pub fn publish_what_if_forecast<Eng: Send + Sync + 'static>(
    engine: &Eng,
    stored: forecastquery::state::StoredForecast,
) -> bool {
    match runtime_state(engine) {
        Some(runtime) => {
            runtime.lock().put_what_if_forecast(stored);
            true
        }
        None => false,
    }
}

/// Withdraw a what-if forecast from the runtime store, so the ARN a
/// `DeleteWhatIfForecast` removed stops being queryable.
pub fn withdraw_what_if_forecast<Eng: Send + Sync + 'static>(engine: &Eng, arn: &str) -> bool {
    match runtime_state(engine) {
        Some(runtime) => {
            runtime.lock().what_if_forecasts.remove(arn);
            true
        }
        None => false,
    }
}
