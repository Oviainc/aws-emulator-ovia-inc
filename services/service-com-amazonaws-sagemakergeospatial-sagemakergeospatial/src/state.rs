// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sagemakergeospatial-sagemakergeospatial service.
//!
//! Every operation computes its response from the records held here: a create is
//! observable from a later read, a stop from a later get, and a delete as a 404.
//! Nothing in this module is derived from one request's shape — the handlers write
//! records and read them back.

use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::*;

/// The region an ARN minted for a request carries when the request names none this
/// service can put in an ARN. Every emulated service defaults to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace (`sagemaker-geospatial`), as its Smithy model's
/// `aws.api#service` trait declares it.
pub const ARN_SERVICE: &str = "sagemaker-geospatial";

/// How long a job spends in each non-terminal status before the clock owes it the
/// next one.
///
/// CHOSEN, not documented by the service: AWS gives no published duration for an
/// Earth Observation job's phases. It is long enough that a caller acting straight
/// after `StartEarthObservationJob` still observes `INITIALIZING` (and the state
/// errors that go with a running job), and short enough that a poll loop
/// terminates promptly.
pub const JOB_PHASE: Duration = Duration::from_millis(500);

// The eight values `EarthObservationJobStatus` accepts on the wire, as
// `shared_types::EARTH_OBSERVATION_JOB_STATUS_VALUES` lists them.
/// `EarthObservationJobStatus` `INITIALIZING`: the status a started job opens in.
pub const STATUS_INITIALIZING: &str = "INITIALIZING";
/// `EarthObservationJobStatus` `IN_PROGRESS`.
pub const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// `EarthObservationJobStatus` `COMPLETED`.
pub const STATUS_COMPLETED: &str = "COMPLETED";
/// `EarthObservationJobStatus` `FAILED`.
pub const STATUS_FAILED: &str = "FAILED";
/// `EarthObservationJobStatus` `STOPPING`.
pub const STATUS_STOPPING: &str = "STOPPING";
/// `EarthObservationJobStatus` `STOPPED`.
pub const STATUS_STOPPED: &str = "STOPPED";
/// `EarthObservationJobStatus` `DELETING`.
pub const STATUS_DELETING: &str = "DELETING";
/// `EarthObservationJobStatus` `DELETED`.
pub const STATUS_DELETED: &str = "DELETED";

/// The most Earth Observation jobs this emulator holds at once.
///
/// A started job stays in the store until something deletes it, so an unbounded
/// store lets any caller that can reach the port grow this process's heap without
/// limit: the cap is the bound on that growth. It is a resource guard this
/// emulator owns, NOT an AWS-published quota — and the error a call past it
/// answers with is the one the service models for exactly this, a 402
/// `ServiceQuotaExceededException`.
///
/// The `idempotency` map needs no separate bound to stay bounded: an entry is
/// written only by the call that mints a job and is removed when that job is
/// deleted, so it never holds more entries than there are stored jobs. The cap is
/// checked against it too, so the bound does not rest on that argument alone.
pub const MAX_EARTH_OBSERVATION_JOBS: usize = 1000;

/// The most tags one resource carries.
///
/// AWS's tagging restrictions put the maximum number of tags per resource at 50,
/// and this model declares no constraint on the `Tags` map, so without the limit
/// `TagResource` would grow one record's tag map on every call. A request that
/// would take a resource past it is refused.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// `EarthObservationJobErrorType` `SERVER_ERROR`, as
/// `shared_types::EARTH_OBSERVATION_JOB_ERROR_TYPE_VALUES` lists it.
pub const ERROR_TYPE_SERVER_ERROR: &str = "SERVER_ERROR";

/// Why a started Earth Observation job ends in `FAILED` here.
///
/// Starting the job is control plane and is emulated for real (the record, its
/// ARN, its status timeline, its tags). The RUN is not: it reads satellite imagery
/// out of a raster data collection and writes processed rasters, and this emulator
/// has neither the imagery nor a processor for it. Reporting `COMPLETED` would
/// claim output bands and exportable results that nothing produced, so the job
/// ends in the status the service models for a run that did not succeed, carrying
/// the reason.
pub const RUN_UNSUPPORTED_MESSAGE: &str = "this emulator does not run Earth Observation \
processing: no raster imagery is available in-process, so the job produced no output bands";

/// Whether `status` is one a job never leaves on its own.
pub fn is_terminal_status(status: &str) -> bool {
    matches!(
        status,
        STATUS_COMPLETED | STATUS_FAILED | STATUS_STOPPED | STATUS_DELETED
    )
}

/// One Earth Observation job, as this service stores it.
#[derive(Debug, Clone)]
pub struct EarthObservationJob {
    /// The job's ARN: the key it is stored under and the one `@httpLabel` callers
    /// address it by.
    pub arn: String,
    pub name: String,
    /// `CreationTime`, as reported on the wire.
    pub creation_time: ::aws_smithy_types::DateTime,
    /// The same instant on the monotonic clock, which is what the status timeline
    /// and `DurationInSeconds` are measured against.
    pub created_at: Instant,
    pub status: String,
    /// When the clock next owes this job a status transition, or `None` when it
    /// owes none (a terminal status).
    pub next_transition_at: Option<Instant>,
    /// When the job reached its terminal status, which is where
    /// `DurationInSeconds` stops counting.
    pub terminal_at: Option<Instant>,
    pub kms_key_id: Option<String>,
    pub input_config: InputConfigInput,
    pub job_config: JobConfigInput,
    pub output_bands: Vec<OutputBand>,
    pub execution_role_arn: String,
    pub error_details: Option<EarthObservationJobErrorDetails>,
    pub export_status: Option<String>,
    pub export_error_details: Option<ExportErrorDetails>,
    pub tags: HashMap<String, String>,
}

impl EarthObservationJob {
    /// Mint a job record for a `StartEarthObservationJob` call made at `now`.
    ///
    /// It opens in `INITIALIZING` with the first transition the clock owes already
    /// scheduled, so no caller has to ask for the timeline to move.
    pub fn new(arn: String, input: StartEarthObservationJobInput, now: Instant) -> Self {
        Self {
            arn,
            name: input.name,
            creation_time: ::aws_smithy_types::DateTime::from(SystemTime::now()),
            created_at: now,
            status: STATUS_INITIALIZING.to_string(),
            next_transition_at: Some(now + JOB_PHASE),
            terminal_at: None,
            kms_key_id: input.kms_key_id,
            input_config: input.input_config,
            job_config: input.job_config,
            output_bands: Vec::new(),
            execution_role_arn: input.execution_role_arn,
            error_details: None,
            export_status: None,
            export_error_details: None,
            tags: input.tags.unwrap_or_default(),
        }
    }

    /// Materialise every status transition the clock already owes this job by
    /// `now`.
    ///
    /// This is the only write a reporting operation makes: each step was due at a
    /// fixed instant whether or not anybody called. Transitions are scheduled from
    /// the instant they came due (`due + JOB_PHASE`), never from `now`, so a job
    /// first read long after it was started lands in the same status it would have
    /// had under continuous polling.
    pub fn materialize(&mut self, now: Instant) {
        while let Some(due) = self.next_transition_at {
            if now < due {
                return;
            }
            match self.status.as_str() {
                STATUS_INITIALIZING => {
                    self.status = STATUS_IN_PROGRESS.to_string();
                    self.next_transition_at = Some(due + JOB_PHASE);
                }
                STATUS_IN_PROGRESS => {
                    // The run itself cannot be performed here; see
                    // `RUN_UNSUPPORTED_MESSAGE`.
                    self.status = STATUS_FAILED.to_string();
                    self.error_details = Some(EarthObservationJobErrorDetails {
                        r#type: Some(ERROR_TYPE_SERVER_ERROR.to_string()),
                        message: Some(RUN_UNSUPPORTED_MESSAGE.to_string()),
                    });
                    self.next_transition_at = None;
                    self.terminal_at = Some(due);
                }
                STATUS_STOPPING => {
                    self.status = STATUS_STOPPED.to_string();
                    self.next_transition_at = None;
                    self.terminal_at = Some(due);
                }
                _ => {
                    self.next_transition_at = None;
                }
            }
        }
    }

    /// Begin stopping the job: `STOPPING` now, `STOPPED` once the clock owes it.
    pub fn begin_stopping(&mut self, now: Instant) {
        self.status = STATUS_STOPPING.to_string();
        self.next_transition_at = Some(now + JOB_PHASE);
        self.terminal_at = None;
    }

    /// `DurationInSeconds`: how long the job has been running, or how long it ran
    /// before it reached its terminal status.
    pub fn duration_in_seconds(&self, now: Instant) -> i32 {
        let end = self.terminal_at.unwrap_or(now);
        let secs = end.saturating_duration_since(self.created_at).as_secs();
        i32::try_from(secs).unwrap_or(i32::MAX)
    }

    /// `OperationType`, which the service documents only as "the operation type
    /// for an Earth Observation job": the name of the `JobConfigInput` union
    /// member the job was created with, read back off the stored config.
    pub fn operation_type(&self) -> &'static str {
        match self.job_config {
            JobConfigInput::BandMathConfig(_) => "BandMathConfig",
            JobConfigInput::ResamplingConfig(_) => "ResamplingConfig",
            JobConfigInput::TemporalStatisticsConfig(_) => "TemporalStatisticsConfig",
            JobConfigInput::CloudRemovalConfig(_) => "CloudRemovalConfig",
            JobConfigInput::ZonalStatisticsConfig(_) => "ZonalStatisticsConfig",
            JobConfigInput::GeoMosaicConfig(_) => "GeoMosaicConfig",
            JobConfigInput::StackConfig(_) => "StackConfig",
            JobConfigInput::CloudMaskingConfig(_) => "CloudMaskingConfig",
            JobConfigInput::LandCoverSegmentationConfig(_) => "LandCoverSegmentationConfig",
        }
    }

    /// The job's tags, or `None` when it carries none — the shape of an optional
    /// `Tags` member on a job response.
    pub fn tags_for_output(&self) -> Option<HashMap<String, String>> {
        (!self.tags.is_empty()).then(|| self.tags.clone())
    }

    /// The job's `OutputBands`, or `None` when it produced none.
    pub fn output_bands_for_output(&self) -> Option<Vec<OutputBand>> {
        (!self.output_bands.is_empty()).then(|| self.output_bands.clone())
    }
}

/// One Vector Enrichment job, as this service stores it.
///
/// Declared with the store so the two job families share one home; the
/// `StartVectorEnrichmentJob` family is its own gap issue and still answers 501,
/// so nothing writes here yet. Tag lookup already reads it, so a tag call on a
/// vector job resolves as soon as that slice lands.
#[derive(Debug, Clone)]
pub struct VectorEnrichmentJob {
    pub arn: String,
    pub name: String,
    pub r#type: String,
    pub creation_time: ::aws_smithy_types::DateTime,
    pub created_at: Instant,
    pub status: String,
    pub next_transition_at: Option<Instant>,
    pub terminal_at: Option<Instant>,
    pub kms_key_id: Option<String>,
    pub input_config: VectorEnrichmentJobInputConfig,
    pub job_config: VectorEnrichmentJobConfig,
    pub execution_role_arn: String,
    pub tags: HashMap<String, String>,
}

/// One raster data collection in this emulator's catalog.
///
/// The catalog is seeded by the `ListRasterDataCollections` /
/// `GetRasterDataCollection` slice (their own gap issues), so it is empty here;
/// [`ComAmazonawsSagemakergeospatialSagemakergeospatialState::raster_data_collection_name`]
/// reads a collection's name out of it when the collection is held.
#[derive(Debug, Clone, Default)]
pub struct RasterDataCollection {
    pub arn: String,
    pub name: String,
    pub r#type: String,
    pub description: String,
    pub description_page_url: String,
    pub supported_filters: Vec<Filter>,
    pub image_source_bands: Vec<String>,
    pub tags: HashMap<String, String>,
}

/// Per-service state for com-amazonaws-sagemakergeospatial-sagemakergeospatial. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSagemakergeospatialSagemakergeospatialState {
    /// Earth Observation jobs by ARN. `BTreeMap`, so every listing walks the
    /// records in one fixed order before the requested sort is applied.
    pub earth_observation_jobs: BTreeMap<String, EarthObservationJob>,
    /// Vector Enrichment jobs by ARN (populated by that slice).
    pub vector_enrichment_jobs: BTreeMap<String, VectorEnrichmentJob>,
    /// The raster data collection catalog by ARN (seeded by that slice).
    pub raster_data_collections: BTreeMap<String, RasterDataCollection>,
    /// `ClientToken` -> the ARN the first call carrying it minted, so a repeated
    /// `StartEarthObservationJob` returns that job instead of creating a second.
    pub idempotency: HashMap<String, String>,
    /// Mixed into every minted resource id, so two jobs minted in the same
    /// nanosecond still get different ids.
    mint_counter: u64,
}

impl ComAmazonawsSagemakergeospatialSagemakergeospatialState {
    /// Whether this store is full, so `StartEarthObservationJob` must refuse
    /// rather than add another record. See [`MAX_EARTH_OBSERVATION_JOBS`].
    ///
    /// Both caller-driven maps are checked, even though `idempotency` cannot
    /// outgrow `earth_observation_jobs` by construction, so the bound holds by
    /// the check and not only by that argument.
    pub fn is_full(&self) -> bool {
        self.earth_observation_jobs.len() >= MAX_EARTH_OBSERVATION_JOBS
            || self.idempotency.len() >= MAX_EARTH_OBSERVATION_JOBS
    }

    /// Materialise the status transitions the clock owes the job `arn` names,
    /// and report whether this service holds such a job.
    ///
    /// The per-record path every handler that addresses ONE job takes, so its
    /// work does not grow with the number of stored records.
    pub fn materialize_job(&mut self, arn: &str, now: Instant) -> bool {
        match self.earth_observation_jobs.get_mut(arn) {
            Some(job) => {
                job.materialize(now);
                true
            }
            None => false,
        }
    }

    /// Materialise the status transitions the clock owes every stored job.
    ///
    /// For the listing, which reports every record and so has to see every
    /// record's current status. Bounded work: the store holds at most
    /// [`MAX_EARTH_OBSERVATION_JOBS`] records.
    pub fn materialize_all(&mut self, now: Instant) {
        for job in self.earth_observation_jobs.values_mut() {
            job.materialize(now);
        }
    }

    /// Mint an unused Earth Observation job ARN in `region`.
    ///
    /// The resource id is 12 lowercase alphanumeric characters, so the ARN
    /// satisfies the model's `EarthObservationJobArn` pattern
    /// (`…:earth-observation-job/[a-z0-9]{12,}$`) and round-trips through the
    /// `@httpLabel` on `GetEarthObservationJob`.
    pub fn mint_earth_observation_job_arn(&mut self, region: &str) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let id = mint_resource_id(self.mint_counter);
            let arn = ::engine::emulator_arn(
                ARN_SERVICE,
                region,
                &format!("earth-observation-job/{id}"),
            );
            if !self.earth_observation_jobs.contains_key(&arn) {
                return arn;
            }
        }
    }

    /// A raster data collection's name as this emulator knows it: the catalog
    /// entry's own `Name` when the collection is held here, otherwise the
    /// collection identifier its ARN carries — the only name available for a
    /// collection this emulator holds no record of.
    pub fn raster_data_collection_name(&self, collection_arn: &str) -> String {
        if let Some(collection) = self.raster_data_collections.get(collection_arn) {
            return collection.name.clone();
        }
        collection_arn
            .rsplit_once('/')
            .map_or(collection_arn, |(_, id)| id)
            .to_string()
    }

    /// `InputConfigOutput` for a stored job: its `InputConfigInput` as submitted,
    /// plus the collection name this service resolves for the query's collection.
    pub fn input_config_output(&self, job: &EarthObservationJob) -> InputConfigOutput {
        let query = job.input_config.raster_data_collection_query.as_ref().map(|q| {
            RasterDataCollectionQueryOutput {
                raster_data_collection_arn: q.raster_data_collection_arn.clone(),
                raster_data_collection_name: self
                    .raster_data_collection_name(&q.raster_data_collection_arn),
                time_range_filter: TimeRangeFilterOutput {
                    start_time: q.time_range_filter.start_time,
                    end_time: q.time_range_filter.end_time,
                },
                area_of_interest: q.area_of_interest.clone(),
                property_filters: q.property_filters.clone(),
            }
        });
        InputConfigOutput {
            previous_earth_observation_job_arn: job
                .input_config
                .previous_earth_observation_job_arn
                .clone(),
            raster_data_collection_query: query,
        }
    }

    /// The tag map of the resource `resource_arn` names, whichever family it
    /// belongs to, or `None` when this service holds no such resource.
    pub fn tags_of(&self, resource_arn: &str) -> Option<&HashMap<String, String>> {
        if let Some(job) = self.earth_observation_jobs.get(resource_arn) {
            return Some(&job.tags);
        }
        if let Some(job) = self.vector_enrichment_jobs.get(resource_arn) {
            return Some(&job.tags);
        }
        self.raster_data_collections
            .get(resource_arn)
            .map(|collection| &collection.tags)
    }

    /// The same lookup as [`tags_of`](Self::tags_of), for a caller that writes.
    pub fn tags_of_mut(&mut self, resource_arn: &str) -> Option<&mut HashMap<String, String>> {
        if let Some(job) = self.earth_observation_jobs.get_mut(resource_arn) {
            return Some(&mut job.tags);
        }
        if let Some(job) = self.vector_enrichment_jobs.get_mut(resource_arn) {
            return Some(&mut job.tags);
        }
        self.raster_data_collections
            .get_mut(resource_arn)
            .map(|collection| &mut collection.tags)
    }
}

/// 12 lowercase alphanumeric characters for a resource id, mixed from `counter`
/// and the wall clock.
fn mint_resource_id(counter: u64) -> String {
    const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut hasher = std::hash::DefaultHasher::new();
    counter.hash(&mut hasher);
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    let mut value = hasher.finish();
    let mut id = String::with_capacity(12);
    for _ in 0..12 {
        id.push(char::from(ALPHABET[(value % 36) as usize]));
        value /= 36;
    }
    id
}

/// The region an ARN minted for this request must carry: the one the request's
/// SigV4 credential scope names, or [`DEFAULT_REGION`] when it names none this
/// service can put in an ARN.
///
/// A region is usable only if it fits the `[a-z0-9-]{1,25}` segment every ARN
/// pattern in this model declares; anything else would mint an ARN its own
/// `@httpLabel` pattern rejects.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| {
            (1..=25).contains(&region.len())
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSagemakergeospatialSagemakergeospatialState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSagemakergeospatialSagemakergeospatialState>)
}
