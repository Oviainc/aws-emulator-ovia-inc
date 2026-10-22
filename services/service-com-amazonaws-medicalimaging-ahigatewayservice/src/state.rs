// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-medicalimaging-ahigatewayservice service.
//!
//! One data store map is the whole control plane: a data store owns its DICOM import
//! jobs and its image sets, so every handler reaches one record rather than joining
//! two parallel maps.
//!
//! # The clock, not the reader, moves a status
//!
//! AWS HealthImaging answers `CreateDatastore` with `CREATING` and `DeleteDatastore`
//! with `DELETING`, and `StartDICOMImportJob` with `SUBMITTED` — three non-terminal
//! statuses the service itself advances afterwards. This crate stores the instant each
//! transition falls DUE and materialises it in [`ComAmazonawsMedicalimagingAhigatewayserviceState::advance_clock`],
//! which every handler calls first. Nothing sleeps, and a reporting operation
//! (`GetDatastore`, `ListDatastores`, `GetDICOMImportJob`) only ever writes a change the
//! clock already owed at that instant — so a read never moves a status that was not
//! already due.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::ImportConfiguration;

/// How long after `CreateDatastore` the data store leaves `CREATING` for `ACTIVE`.
///
/// AWS publishes no duration for it, so this is a chosen span: long enough that a
/// caller polling straight after the create still observes `CREATING`, short enough
/// that a poll loop terminates promptly.
pub const DATASTORE_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// How long after `DeleteDatastore` the data store leaves `DELETING` and is gone.
/// A chosen span, for the same reason as [`DATASTORE_ACTIVATION_DELAY`].
pub const DATASTORE_DELETION_DELAY: Duration = Duration::from_millis(500);

/// How long after `StartDICOMImportJob` the job leaves `SUBMITTED` for `IN_PROGRESS`.
/// A chosen span, for the same reason as [`DATASTORE_ACTIVATION_DELAY`].
pub const IMPORT_JOB_IN_PROGRESS_DELAY: Duration = Duration::from_millis(300);

/// How long after `StartDICOMImportJob` the job reaches `COMPLETED` and gets `endedAt`.
/// A chosen span, for the same reason as [`DATASTORE_ACTIVATION_DELAY`].
pub const IMPORT_JOB_COMPLETION_DELAY: Duration = Duration::from_millis(600);

/// The smallest page `ListDatastores` will serve — the model's `maxResults` minimum
/// (`smithy_query(query="operation",operation="ListDatastores")` reports range 1..50).
pub const MIN_LIST_PAGE_SIZE: usize = 1;

/// The largest page `ListDatastores` will serve — the model's `maxResults` maximum, and
/// also the page size used when the caller names no `maxResults`. Every page is clamped
/// into `MIN_LIST_PAGE_SIZE..=MAX_LIST_PAGE_SIZE` in the handler rather than trusting
/// the boundary validator alone, so one request can never materialise an unbounded
/// response.
pub const MAX_LIST_PAGE_SIZE: usize = 50;

/// How many data stores this emulator will hold at once.
///
/// A memory bound, not an AWS quota: `CreateDatastore` persists caller-supplied data
/// (name, ARNs, the whole tag map) in process memory, so without a cap a caller could
/// grow it without limit. Past the cap the call is refused with the operation's own
/// `ServiceQuotaExceededException` (HTTP 402).
pub const MAX_DATASTORES: usize = 128;

/// How many import jobs one data store will hold at once — the same memory bound as
/// [`MAX_DATASTORES`], applied per data store so the product of the two is bounded too.
/// Past the cap `StartDICOMImportJob` is refused with `ServiceQuotaExceededException`.
pub const MAX_IMPORT_JOBS_PER_DATASTORE: usize = 128;

/// How many `@idempotencyToken` entries the ledger keeps.
///
/// The ledger outlives the resources it names (a token must stay honoured for a while
/// after its resource is gone), so it is bounded two ways: entries whose resource has
/// been retired are dropped in
/// [`ComAmazonawsMedicalimagingAhigatewayserviceState::advance_clock`], and the
/// remainder is capped here with FIFO eviction of the oldest token.
pub const MAX_CLIENT_TOKENS: usize = 1024;

/// The service's `arnNamespace` / sigv4 signing name, used to build data store ARNs.
pub const ARN_SERVICE: &str = "medical-imaging";

/// The region an ARN names when the request carries no SigV4 credential scope.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `@pattern` of `com.amazonaws.medicalimaging#Arn`
/// (`smithy_query(query="shape",shape="Arn")`), the shape every `resourceArn` path
/// label targets, copied verbatim.
///
/// It is the single source for two things: [`parse_resource_arn`] below, and the way
/// [`crate::service`] recognises a malformed RESOURCE ARN — a refused value is one
/// when the `@pattern` it failed is this one. No other member targets this shape:
/// `kmsKeyArn`, `lambdaAuthorizerArn` and `dataAccessRoleArn` each target their own
/// ARN shape with its own pattern, so they keep the generic constraint refusal.
pub const ARN_PATTERN: &str = "^arn:aws((-us-gov)|(-iso)|(-iso-b)|(-cn))?:medical-imaging:[a-z0-9-]+:[0-9]{12}:datastore/[0-9a-z]{32}(/imageset/[0-9a-z]{32})?$";

/// The ARN partitions [`ARN_PATTERN`] admits: `aws` plus the four suffixed ones
/// (`(-us-gov)|(-iso)|(-iso-b)|(-cn))?`).
pub const ARN_PARTITIONS: [&str; 5] = ["aws", "aws-us-gov", "aws-iso", "aws-iso-b", "aws-cn"];

/// How many tags one resource will hold at once.
///
/// A memory bound, not an AWS quota: the tagging operations persist caller-supplied
/// keys and values in process memory, so without a cap one caller could grow a
/// resource's tag map without limit. Past the cap `TagResource` is refused with its
/// own modelled `ValidationException` (HTTP 400). The value matches AWS's
/// general per-resource tag limit of 50.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// `DatastoreStatus`, exactly the values the model declares
/// (`smithy_query(query="shape",shape="DatastoreStatus")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatastoreStatus {
    Creating,
    CreateFailed,
    Active,
    Deleting,
    Deleted,
}

impl DatastoreStatus {
    /// The wire form of this status.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::CreateFailed => "CREATE_FAILED",
            Self::Active => "ACTIVE",
            Self::Deleting => "DELETING",
            Self::Deleted => "DELETED",
        }
    }

    /// Parse a wire status, or `None` when the string names no modelled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "CREATING" => Some(Self::Creating),
            "CREATE_FAILED" => Some(Self::CreateFailed),
            "ACTIVE" => Some(Self::Active),
            "DELETING" => Some(Self::Deleting),
            "DELETED" => Some(Self::Deleted),
            _ => None,
        }
    }
}

/// `JobStatus`, exactly the values the model declares
/// (`smithy_query(query="shape",shape="JobStatus")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Submitted,
    InProgress,
    Completed,
    Failed,
}

impl JobStatus {
    /// The wire form of this status.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Submitted => "SUBMITTED",
            Self::InProgress => "IN_PROGRESS",
            Self::Completed => "COMPLETED",
            Self::Failed => "FAILED",
        }
    }

    /// Parse a wire status, or `None` when the string names no modelled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "SUBMITTED" => Some(Self::Submitted),
            "IN_PROGRESS" => Some(Self::InProgress),
            "COMPLETED" => Some(Self::Completed),
            "FAILED" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// One persisted DICOM import job, owned by the data store it imports into.
#[derive(Debug, Clone)]
pub struct ImportJob {
    pub job_id: String,
    /// `StartDICOMImportJob` leaves `jobName` optional; `DICOMImportJobProperties`
    /// makes it required, so [`ImportJob::reported_name`] supplies the fallback.
    pub job_name: Option<String>,
    pub job_status: JobStatus,
    pub datastore_id: String,
    pub data_access_role_arn: String,
    pub input_s3_uri: String,
    pub output_s3_uri: String,
    pub input_owner_account_id: Option<String>,
    pub import_configuration: Option<ImportConfiguration>,
    pub submitted_at: SystemTime,
    pub ended_at: Option<SystemTime>,
    pub message: Option<String>,
    /// When the job is due to leave `SUBMITTED`.
    pub in_progress_due_at: SystemTime,
    /// When the job is due to reach `COMPLETED`.
    pub completed_due_at: SystemTime,
}

impl ImportJob {
    /// The job name `DICOMImportJobProperties` reports. The properties shape makes
    /// `jobName` `@required` while the start request leaves it optional, so a job
    /// submitted without one reports its own identifier — which satisfies the member's
    /// `^[A-Za-z0-9._/#-]+$` pattern.
    pub fn reported_name(&self) -> String {
        self.job_name.clone().unwrap_or_else(|| self.job_id.clone())
    }

    /// Materialise every status change the clock owed this job by `now`.
    fn advance_to(&mut self, now: SystemTime) {
        if matches!(self.job_status, JobStatus::Completed | JobStatus::Failed) {
            return;
        }
        if now >= self.in_progress_due_at && self.job_status == JobStatus::Submitted {
            self.job_status = JobStatus::InProgress;
        }
        if now >= self.completed_due_at {
            self.job_status = JobStatus::Completed;
            // The due instant, not `now`: `endedAt` must not change between reads.
            self.ended_at = Some(self.completed_due_at);
        }
    }
}

/// One persisted image set. Import jobs populate this map in the image-set slice;
/// the bootstrap slice only needs the data store to own it.
#[derive(Debug, Clone)]
pub struct ImageSet {
    /// The tags `TagResource` / `ListTagsForResource` keep for this image set, which
    /// an ARN reaches through its optional `/imageset/<id>` suffix (see
    /// [`ARN_PATTERN`]). Empty until the image-set slice mints image sets.
    pub tags: HashMap<String, String>,
    pub image_set_id: String,
    pub image_set_arn: String,
    pub versions: Vec<String>,
    pub image_set_state: String,
    pub image_set_workflow_status: String,
    pub is_primary: bool,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub deleted_at: Option<SystemTime>,
}

/// One persisted data store, with the import jobs and image sets it owns.
#[derive(Debug, Clone)]
pub struct Datastore {
    pub datastore_id: String,
    /// `CreateDatastore` leaves `datastoreName` optional; `DatastoreProperties` and
    /// `DatastoreSummary` make it required, so [`Datastore::reported_name`] supplies
    /// the fallback.
    pub datastore_name: Option<String>,
    pub datastore_status: DatastoreStatus,
    pub datastore_arn: String,
    pub kms_key_arn: Option<String>,
    pub lambda_authorizer_arn: Option<String>,
    pub lossless_storage_format: Option<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// When the next status change this data store owes falls due, if any.
    pub status_due_at: Option<SystemTime>,
    pub import_jobs: HashMap<String, ImportJob>,
    pub image_sets: HashMap<String, ImageSet>,
}

impl Datastore {
    /// The data store name `DatastoreProperties` / `DatastoreSummary` report. Both
    /// shapes make `datastoreName` `@required` while the create request leaves it
    /// optional, so a store created without one reports its own identifier — which
    /// satisfies the member's `^[A-Za-z0-9._/#-]+$` pattern.
    pub fn reported_name(&self) -> String {
        self.datastore_name
            .clone()
            .unwrap_or_else(|| self.datastore_id.clone())
    }

    /// Materialise every change the clock owed this data store by `now`.
    ///
    /// Returns `false` when the deletion it owed has fallen due, i.e. the record must
    /// be dropped from the store.
    fn advance_to(&mut self, now: SystemTime) -> bool {
        if let Some(due) = self.status_due_at {
            if now >= due {
                match self.datastore_status {
                    DatastoreStatus::Creating => {
                        self.datastore_status = DatastoreStatus::Active;
                        // The due instant, not `now`: `updatedAt` must be stable
                        // across reads.
                        self.updated_at = due;
                        self.status_due_at = None;
                    }
                    DatastoreStatus::Deleting => {
                        self.datastore_status = DatastoreStatus::Deleted;
                        self.updated_at = due;
                        self.status_due_at = None;
                        return false;
                    }
                    _ => self.status_due_at = None,
                }
            }
        }
        for job in self.import_jobs.values_mut() {
            job.advance_to(now);
        }
        true
    }
}

/// Per-service state for com-amazonaws-medicalimaging-ahigatewayservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMedicalimagingAhigatewayserviceState {
    /// Every live data store, keyed by its 32-character lowercase-alphanumeric id.
    pub datastores: HashMap<String, Datastore>,
    /// `@idempotencyToken` ledger: a scoped client token (see [`client_token_key`])
    /// mapped to the id it minted, so a repeat call returns the SAME resource. Bounded
    /// by [`MAX_CLIENT_TOKENS`]; write it only through
    /// [`ComAmazonawsMedicalimagingAhigatewayserviceState::remember_client_token`].
    pub client_tokens: HashMap<String, String>,
    /// The ledger's keys in insertion order, which is what makes the eviction in
    /// [`ComAmazonawsMedicalimagingAhigatewayserviceState::remember_client_token`] FIFO.
    pub client_token_order: VecDeque<String>,
    /// Monotonic counter behind [`ComAmazonawsMedicalimagingAhigatewayserviceState::mint_id`].
    pub next_id: u64,
}

impl ComAmazonawsMedicalimagingAhigatewayserviceState {
    /// Mint a fresh 32-character lowercase-hex id, which satisfies both
    /// `DatastoreId` (`^[0-9a-z]{32}$`) and `JobId` (`^[0-9a-z]+$`, 1..32).
    ///
    /// Deterministic: the n-th id of a fresh store is always the same, because the
    /// bits come from SplitMix64 over a counter rather than from the system RNG.
    /// SplitMix64 is a bijection on `u64`, so two counter values never collide.
    pub fn mint_id(&mut self) -> String {
        self.next_id = self.next_id.wrapping_add(1);
        let high = splitmix64(self.next_id);
        let low = splitmix64(self.next_id ^ 0x5DEE_CE66_D1B5_7A4F);
        format!("{high:016x}{low:016x}")
    }

    /// Record that `key` minted `minted_id`, evicting the oldest token once the ledger
    /// is at [`MAX_CLIENT_TOKENS`], so repeated creates cannot grow it without limit.
    pub fn remember_client_token(&mut self, key: String, minted_id: String) {
        if self.client_tokens.insert(key.clone(), minted_id).is_none() {
            self.client_token_order.push_back(key);
        }
        while self.client_token_order.len() > MAX_CLIENT_TOKENS {
            match self.client_token_order.pop_front() {
                Some(oldest) => {
                    self.client_tokens.remove(&oldest);
                }
                None => break,
            }
        }
    }

    /// The tag map of the resource `arn` names — a data store, or one image set it
    /// owns — or `None` when this emulator holds no such resource.
    ///
    /// One lookup for both resource kinds, so the three tagging operations agree on
    /// what exists: tags live ON the resource record rather than in a map beside it.
    /// The ARN must be the resource's OWN: a data store is reached by its id, and
    /// then only when the ARN it was named by is the one the data store reports.
    pub fn resource_tags(&self, arn: &ResourceArn) -> Option<&HashMap<String, String>> {
        let datastore = self.datastores.get(&arn.datastore_id)?;
        if datastore.datastore_arn != arn.datastore_arn {
            return None;
        }
        match &arn.image_set_id {
            None => Some(&datastore.tags),
            Some(image_set_id) => Some(&datastore.image_sets.get(image_set_id)?.tags),
        }
    }

    /// [`Self::resource_tags`], for a writer.
    pub fn resource_tags_mut(&mut self, arn: &ResourceArn) -> Option<&mut HashMap<String, String>> {
        let datastore = self.datastores.get_mut(&arn.datastore_id)?;
        if datastore.datastore_arn != arn.datastore_arn {
            return None;
        }
        match &arn.image_set_id {
            None => Some(&mut datastore.tags),
            Some(image_set_id) => Some(&mut datastore.image_sets.get_mut(image_set_id)?.tags),
        }
    }

    /// Materialise every status change the clock owed by `now`, dropping the data
    /// stores whose deletion has fallen due — and, with them, the ledger entries that
    /// named the data store or any of its import jobs, so a create/delete loop cannot
    /// leave entries behind forever. Every handler calls this first, so each request
    /// sees the state the clock already made true.
    pub fn advance_clock(&mut self, now: SystemTime) {
        let mut retired: HashSet<String> = HashSet::new();
        self.datastores.retain(|_, datastore| {
            if datastore.advance_to(now) {
                return true;
            }
            retired.insert(datastore.datastore_id.clone());
            retired.extend(datastore.import_jobs.keys().cloned());
            false
        });
        if retired.is_empty() {
            return;
        }
        self.client_tokens
            .retain(|_, minted_id| !retired.contains(minted_id));
        let live = &self.client_tokens;
        self.client_token_order.retain(|key| live.contains_key(key));
    }
}

/// The ledger key for one `@idempotencyToken`.
///
/// Scoped by operation (and, for an import job, by the data store it runs against), so
/// the same token reused on a different operation mints a different resource rather
/// than returning the first one.
pub fn client_token_key(scope: &str, client_token: &str) -> String {
    format!("{scope}\u{0}{client_token}")
}

/// SplitMix64's finalizer — a bijection on `u64`, which is what makes
/// [`ComAmazonawsMedicalimagingAhigatewayserviceState::mint_id`] collision-free.
fn splitmix64(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The region this request is signed for, used to build ARNs.
///
/// Read from the SigV4 credential scope (`<akid>/<date>/<region>/<name>/aws4_request`)
/// through the one parser the core vends; an unsigned request falls back to
/// [`DEFAULT_REGION`].
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a data store in `region`:
/// `arn:aws:medical-imaging:<region>:<account>:datastore/<datastoreId>`.
pub fn datastore_arn(region: &str, datastore_id: &str) -> String {
    ::emulator_server::emulator_arn(ARN_SERVICE, region, &format!("datastore/{datastore_id}"))
}

/// What a `resourceArn` label names: a data store, or ONE image set inside it —
/// the two resources [`ARN_PATTERN`] can spell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceArn {
    /// The ARN of the data store, i.e. the label minus any `/imageset/<id>` suffix.
    ///
    /// Carried, not just the id, because an ARN names a resource only when it is
    /// THAT resource's ARN: one differing in partition, region or account names a
    /// resource of another account or region, which this emulator does not hold.
    pub datastore_arn: String,
    pub datastore_id: String,
    /// `Some` only when the ARN carries the optional `/imageset/<id>` suffix.
    pub image_set_id: Option<String>,
}

/// Parse a medical-imaging resource ARN, or `None` when the string is not one.
///
/// Accepts exactly what [`ARN_PATTERN`] does — partition, this service's
/// `arnNamespace`, a region, a 12-digit account id, and a `datastore/<id>` resource
/// with an optional `/imageset/<id>` suffix — so a value the boundary validator let
/// through always parses, and a caller that reaches the handler by another route is
/// still refused rather than mis-read.
pub fn parse_resource_arn(arn: &str) -> Option<ResourceArn> {
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let partition = fields.next()?;
    if !ARN_PARTITIONS.contains(&partition) {
        return None;
    }
    if fields.next()? != ARN_SERVICE {
        return None;
    }
    // `[a-z0-9-]+`: the pattern constrains the region's alphabet, not its shape.
    let region = fields.next()?;
    if region.is_empty()
        || !region
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return None;
    }
    // `[0-9]{12}`: an account id.
    let account_id = fields.next()?;
    if account_id.len() != 12 || !account_id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut segments = fields.next()?.split('/');
    if segments.next()? != "datastore" {
        return None;
    }
    let datastore_id = segments.next().filter(|id| is_resource_id(id))?.to_string();
    let image_set_id = match segments.next() {
        None => None,
        Some("imageset") => Some(segments.next().filter(|id| is_resource_id(id))?.to_string()),
        Some(_) => return None,
    };
    if segments.next().is_some() {
        return None;
    }
    Some(ResourceArn {
        datastore_arn: format!(
            "arn:{partition}:{ARN_SERVICE}:{region}:{account_id}:datastore/{datastore_id}"
        ),
        datastore_id,
        image_set_id,
    })
}

/// `[0-9a-z]{32}`: the id form [`ARN_PATTERN`] gives both `datastore/` and
/// `/imageset/`, and the form [`ComAmazonawsMedicalimagingAhigatewayserviceState::mint_id`]
/// produces.
fn is_resource_id(value: &str) -> bool {
    value.len() == 32
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// A modelled timestamp for `instant`, kept to full nanosecond precision rather than
/// routed through an `f64` number of seconds.
pub fn to_date_time(instant: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX);
    ::aws_smithy_types::DateTime::from_secs_and_nanos(seconds, since_epoch.subsec_nanos())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMedicalimagingAhigatewayserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMedicalimagingAhigatewayserviceState>)
}
