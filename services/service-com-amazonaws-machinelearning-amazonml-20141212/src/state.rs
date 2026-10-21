// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-machinelearning-amazonml-20141212 service.
//!
//! The Amazon ML control plane this crate emulates is a set of user-identified
//! entities (`DataSource`, `MLModel`, `Evaluation`, `BatchPrediction`) keyed by the
//! caller-supplied id. Every record lives in one map on
//! [`ComAmazonawsMachinelearningAmazonml20141212State`] so two operations never read
//! two copies of one resource, and every response an operation returns is projected
//! from the record stored here.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::RealtimeEndpointInfo;

/// The `EntityStatus` values this slice stores, spelled exactly as the model's
/// `EntityStatus` enum spells them (see `ENTITY_STATUS_VALUES` in `ops::types`).
pub mod entity_status {
    /// The create call has been accepted and the asynchronous work has not finished.
    pub const PENDING: &str = "PENDING";
    /// The asynchronous work finished and the entity is usable.
    pub const COMPLETED: &str = "COMPLETED";
    /// The entity was deleted. Amazon ML soft-deletes: the record stays readable.
    pub const DELETED: &str = "DELETED";
}

/// The `RealtimeEndpointStatus` value a model with no realtime endpoint carries,
/// spelled as the model's `RealtimeEndpointStatus` enum spells it (`NONE`).
pub const REALTIME_ENDPOINT_STATUS_NONE: &str = "NONE";

/// The only `Algorithm` Amazon ML models are trained with; the model's `Algorithm`
/// enum declares exactly this one value (`ALGORITHM_VALUES == ["sgd"]`).
pub const ALGORITHM_SGD: &str = "sgd";

/// How long after a create call this service lets the asynchronous work it models
/// become due.
///
/// `CreateDataSourceFromS3` and `CreateMLModel` are documented as asynchronous:
/// each returns immediately with the entity `PENDING`, and Amazon ML moves it to
/// `COMPLETED` once the work finishes. The real duration depends on the data, so the
/// emulator picks one fixed, non-zero span: a caller that reads straight after the
/// create still observes `PENDING`, and the move to `COMPLETED` is materialised by
/// whichever later call first observes that the span has elapsed — the due instant is
/// stored on the record and nothing ever sleeps for it.
pub const SETTLE_AFTER: Duration = Duration::from_millis(500);

/// A stored `DataSource` record. Every `GetDataSource` / `DescribeDataSources` field
/// is projected from this.
#[derive(Debug, Clone)]
pub struct DataSource {
    /// The caller-supplied `DataSourceId`, and this record's map key.
    pub id: String,
    /// `DataSourceName` as supplied; reported as `Name`.
    pub name: Option<String>,
    /// `DataSpec.DataLocationS3`.
    pub data_location_s3: String,
    /// `DataSpec.DataRearrangement`.
    pub data_rearrangement: Option<String>,
    /// `DataSpec.DataSchema`; reported as `DataSourceSchema` on a verbose read.
    pub data_schema: Option<String>,
    /// `DataSpec.DataSchemaLocationS3`.
    pub data_schema_location_s3: Option<String>,
    /// `ComputeStatistics` as supplied.
    pub compute_statistics: bool,
    /// `RoleARN`, set only by the RDS/Redshift creates.
    pub role_arn: Option<String>,
    /// One of [`entity_status`].
    pub status: String,
    /// When the create call was accepted.
    pub created_at: SystemTime,
    /// When the record last changed.
    pub last_updated_at: SystemTime,
    /// When the asynchronous ingestion started: the create call itself.
    pub started_at: SystemTime,
    /// When it finished; `None` until the entity settles.
    pub finished_at: Option<SystemTime>,
    /// `ComputeTime`, the milliseconds spent between `started_at` and `finished_at`.
    pub compute_time_ms: Option<i64>,
    /// `DataSizeInBytes`. `None` here: nothing in this emulator reads the referenced
    /// Amazon S3 objects, so there is no observed size to report.
    pub data_size_in_bytes: Option<i64>,
    /// `NumberOfFiles`, `None` for the same reason as [`Self::data_size_in_bytes`].
    pub number_of_files: Option<i64>,
    /// `Message`, set only when the entity fails.
    pub message: Option<String>,
    /// `CreatedByIamUser`.
    pub created_by_iam_user: String,
    /// The instant from which the `PENDING` -> `COMPLETED` move is due.
    pub settles_at: SystemTime,
    /// Fingerprint of the create request, so a repeat of the same id can tell an
    /// identical retry from a conflicting one.
    pub request_fingerprint: String,
}

impl DataSource {
    /// Materialise the `PENDING` -> `COMPLETED` move the clock already made due.
    ///
    /// Owed at `settles_at` whether or not anybody calls, so applying it on a read
    /// changes nothing a later caller would not have seen anyway.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == entity_status::PENDING && now >= self.settles_at {
            self.status = entity_status::COMPLETED.to_string();
            self.finished_at = Some(self.settles_at);
            self.compute_time_ms = Some(millis_between(self.started_at, self.settles_at));
            self.last_updated_at = self.settles_at;
        }
    }

    /// Whether this record may be used to train or evaluate a model: Amazon ML
    /// documents a `DataSource` in the `COMPLETED` or `PENDING` state as usable.
    pub fn is_usable(&self) -> bool {
        self.status == entity_status::COMPLETED || self.status == entity_status::PENDING
    }
}

/// A stored `MLModel` record. Every `GetMLModel` / `DescribeMLModels` field is
/// projected from this.
#[derive(Debug, Clone)]
pub struct MlModel {
    /// The caller-supplied `MLModelId`, and this record's map key.
    pub id: String,
    /// `MLModelName` as supplied; reported as `Name`.
    pub name: Option<String>,
    /// `MLModelType`: one of the model's `MLModelType` enum values.
    pub model_type: String,
    /// `TrainingDataSourceId`, the key of the `DataSource` trained on.
    pub training_data_source_id: String,
    /// `Parameters` as supplied; reported as `TrainingParameters`.
    pub training_parameters: BTreeMap<String, String>,
    /// `Recipe` as supplied; reported only on a verbose read.
    pub recipe: Option<String>,
    /// `RecipeUri` as supplied.
    pub recipe_uri: Option<String>,
    /// The data schema this model was trained against; reported only on a verbose
    /// read. Taken from the training `DataSource` at create time.
    pub schema: Option<String>,
    /// `ScoreThreshold`: the positive/negative boundary, which only binary models
    /// have.
    pub score_threshold: Option<f32>,
    /// When `ScoreThreshold` was last changed; `None` until `UpdateMLModel` moves it.
    pub score_threshold_last_updated_at: Option<SystemTime>,
    /// One of [`entity_status`].
    pub status: String,
    /// `SizeInBytes`. `None` here: no model artifact is produced, so there is no
    /// observed size to report.
    pub size_in_bytes: Option<i64>,
    /// When the create call was accepted.
    pub created_at: SystemTime,
    /// When the record last changed.
    pub last_updated_at: SystemTime,
    /// When training started: the create call itself.
    pub started_at: SystemTime,
    /// When training finished; `None` until the model settles.
    pub finished_at: Option<SystemTime>,
    /// `ComputeTime`, the milliseconds spent between `started_at` and `finished_at`.
    pub compute_time_ms: Option<i64>,
    /// `Message`, set only when the entity fails.
    pub message: Option<String>,
    /// The realtime endpoint, once `CreateRealtimeEndpoint` makes one.
    pub endpoint_info: Option<RealtimeEndpointInfo>,
    /// `CreatedByIamUser`.
    pub created_by_iam_user: String,
    /// The instant from which the `PENDING` -> `COMPLETED` move is due.
    pub settles_at: SystemTime,
    /// Fingerprint of the create request; see [`DataSource::request_fingerprint`].
    pub request_fingerprint: String,
}

impl MlModel {
    /// Materialise the `PENDING` -> `COMPLETED` move the clock already made due.
    /// See [`DataSource::settle`].
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == entity_status::PENDING && now >= self.settles_at {
            self.status = entity_status::COMPLETED.to_string();
            self.finished_at = Some(self.settles_at);
            self.compute_time_ms = Some(millis_between(self.started_at, self.settles_at));
            self.last_updated_at = self.settles_at;
        }
    }

    /// The model's realtime endpoint as the API reports it: the stored endpoint when
    /// one exists, otherwise the `NONE` endpoint a model without one carries.
    pub fn realtime_endpoint(&self) -> RealtimeEndpointInfo {
        self.endpoint_info
            .clone()
            .unwrap_or_else(|| RealtimeEndpointInfo {
                peak_requests_per_second: 0,
                created_at: None,
                endpoint_url: None,
                endpoint_status: Some(REALTIME_ENDPOINT_STATUS_NONE.to_string()),
            })
    }

    /// The `RealtimeEndpointStatus` this model reports.
    pub fn realtime_endpoint_status(&self) -> String {
        self.endpoint_info
            .as_ref()
            .and_then(|info| info.endpoint_status.clone())
            .unwrap_or_else(|| REALTIME_ENDPOINT_STATUS_NONE.to_string())
    }
}

/// Per-service state for com-amazonaws-machinelearning-amazonml-20141212. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMachinelearningAmazonml20141212State {
    /// `DataSource` records by `DataSourceId`. `BTreeMap` so iteration order is
    /// stable, which is what makes a continuation token resume deterministically.
    pub data_sources: BTreeMap<String, DataSource>,
    /// `MLModel` records by `MLModelId`.
    pub ml_models: BTreeMap<String, MlModel>,
}

impl ComAmazonawsMachinelearningAmazonml20141212State {
    /// Apply every `PENDING` -> `COMPLETED` move the clock has already made due.
    pub fn settle_all(&mut self, now: SystemTime) {
        for data_source in self.data_sources.values_mut() {
            data_source.settle(now);
        }
        for model in self.ml_models.values_mut() {
            model.settle(now);
        }
    }

    /// `InputDataLocationS3` for a model: the `DataLocationS3` of the `DataSource` it
    /// was trained on, resolved through the stored record rather than copied at
    /// create time.
    pub fn input_data_location_s3(&self, model: &MlModel) -> Option<String> {
        self.data_sources
            .get(&model.training_data_source_id)
            .map(|data_source| data_source.data_location_s3.clone())
    }

    /// The value `DescribeMLModels` filters on for one `MLModelFilterVariable`, or
    /// `None` when the model has no value for it.
    ///
    /// Timestamps project to their ISO-8601 UTC rendering, whose lexicographic order
    /// is their chronological order, so one comparator implementation serves every
    /// filter variable.
    pub fn ml_model_filter_value(&self, model: &MlModel, variable: &str) -> Option<String> {
        match variable {
            "CreatedAt" => format_timestamp(model.created_at),
            "LastUpdatedAt" => format_timestamp(model.last_updated_at),
            "Status" => Some(model.status.clone()),
            "Name" => model.name.clone(),
            "IAMUser" => Some(model.created_by_iam_user.clone()),
            "TrainingDataSourceId" => Some(model.training_data_source_id.clone()),
            "RealtimeEndpointStatus" => Some(model.realtime_endpoint_status()),
            "MLModelType" => Some(model.model_type.clone()),
            "Algorithm" => Some(ALGORITHM_SGD.to_string()),
            "TrainingDataURI" => self.input_data_location_s3(model),
            _ => None,
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMachinelearningAmazonml20141212State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMachinelearningAmazonml20141212State>)
}

/// The `CreatedByIamUser` every record this emulator creates carries: the canonical
/// emulator account's root user, built through `engine::emulator_arn` so the account
/// segment has one source of truth.
pub fn created_by_iam_user() -> String {
    ::engine::emulator_arn("iam", "", "root")
}

/// The ceiling on how many `DataSource` records this store holds, and the matching
/// ceiling for `MLModel` records: the two caps that bound caller-driven growth.
///
/// A generated emulator keeps its resources in memory for the life of the process and a
/// create is the only thing that adds one, so each store needs a maximum entry count —
/// neither a TTL nor an eviction policy is available to it, because `GetDataSource` and
/// `GetMLModel` must still answer what the creates created, and Amazon ML soft-deletes,
/// so even a delete must keep its record readable. The two operations that grow the
/// store refuse past these caps with their own modelled `InternalServerException`
/// (HTTP 500): the capacity is the server's, not anything wrong with the request, and
/// Amazon ML models no quota error on these operations. The numbers are the emulator's
/// own bound, not a published AWS default, and are set high enough that no ordinary
/// workflow meets one.
pub const MAX_DATA_SOURCES: usize = 5_000;

/// See [`MAX_DATA_SOURCES`].
pub const MAX_ML_MODELS: usize = 5_000;

/// Record one state-changing call in the audit stream.
///
/// A control-plane mutation (a create, an update, a delete) leaves a structured record
/// naming the operation, the resource it changed, the outcome, and the access key id
/// the caller PRESENTED — never a secret, and never this emulator's own credentials.
/// It is emitted on the `audit` target so a collector can filter it apart from ordinary
/// tracing, and the subscriber stamps the timestamp and the request's span context.
pub fn audit_mutation(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource_type: &str,
    resource_id: &str,
    outcome: &str,
) {
    ::tracing::info!(
        target: "audit",
        operation,
        resource_type,
        resource_id,
        outcome,
        principal = %::engine::extract_akid(&ctx.parts().headers),
        "state-changing Amazon ML operation"
    );
}

/// A wall-clock instant as the modelled `EpochTime` timestamp type.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// `at` as a fixed-width ISO-8601 UTC rendering with millisecond precision
/// (`YYYY-MM-DDTHH:MM:SS.mmmZ`), or `None` when it cannot be represented.
///
/// Fixed width is what makes the lexicographic order of two renderings their
/// chronological order, so `DescribeMLModels` can compare and sort a timestamp filter
/// variable with the same string comparator every other filter variable uses. The
/// model's own `Format::DateTime` rendering omits a zero fraction and varies the
/// number of fractional digits, which would order `…:43Z` after `…:43.5Z`.
pub fn format_timestamp(at: SystemTime) -> Option<String> {
    let at = to_date_time(at);
    let whole_seconds = ::aws_smithy_types::DateTime::from_secs(at.secs())
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()?;
    let millis = at.subsec_nanos() / 1_000_000;
    Some(format!(
        "{}.{millis:03}Z",
        whole_seconds.trim_end_matches('Z')
    ))
}

/// Whole milliseconds from `from` to `to`, saturating at 0 when `to` precedes `from`.
pub fn millis_between(from: SystemTime, to: SystemTime) -> i64 {
    to.duration_since(from)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// A collision-resistant fingerprint of a create request's parameters.
///
/// Every field is length-prefixed and absence is encoded distinctly, so no two
/// different parameter sets can produce the same input to the hash. Used to tell an
/// identical retry of a create (which is idempotent) from a second, conflicting use
/// of the same id (which is `IdempotentParameterMismatchException`).
pub fn fingerprint<'a>(fields: impl IntoIterator<Item = (&'a str, Option<&'a str>)>) -> String {
    use sha2::Digest;

    let mut hasher = sha2::Sha256::new();
    for (name, value) in fields {
        hasher.update((name.len() as u64).to_be_bytes());
        hasher.update(name.as_bytes());
        match value {
            Some(value) => {
                hasher.update([1u8]);
                hasher.update((value.len() as u64).to_be_bytes());
                hasher.update(value.as_bytes());
            }
            None => hasher.update([0u8]),
        }
    }
    hex::encode(hasher.finalize())
}
