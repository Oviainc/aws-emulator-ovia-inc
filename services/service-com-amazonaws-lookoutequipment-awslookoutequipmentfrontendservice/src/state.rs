// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lookoutequipment-awslookoutequipmentfrontendservice service.
//!
//! The service's primary resource is the **dataset**: a container for time-series data,
//! plus the data-ingestion jobs run against it. Every implemented operation computes its
//! answer from the records below — nothing is canned, and nothing is echoed straight back
//! out of a request without being stored first.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{DatasetSummary, IngestionInputConfiguration, Tag};

/// The signing name, endpoint prefix and ARN namespace of this service (all three are
/// `lookoutequipment`; OPERATIONS.md, "Wire protocol").
pub const ARN_SERVICE: &str = "lookoutequipment";

/// Region used when a request carries no SigV4 credential scope to read one from (an
/// unsigned request still routes by endpoint host).
pub const DEFAULT_REGION: &str = "us-east-1";

/// `com.amazonaws.lookoutequipment#DatasetStatus` values this emulator moves a dataset
/// through. The full enum (smithy_query: shape `DatasetStatus`) is CREATED,
/// INGESTION_IN_PROGRESS, ACTIVE, IMPORT_IN_PROGRESS; the last belongs to
/// `ImportDataset`, which is still a stub.
pub const DATASET_STATUS_CREATED: &str = "CREATED";
/// Transient status a dataset holds while a data-ingestion job runs against it.
pub const DATASET_STATUS_INGESTION_IN_PROGRESS: &str = "INGESTION_IN_PROGRESS";
/// The status a dataset rests in once data has been ingested into it.
pub const DATASET_STATUS_ACTIVE: &str = "ACTIVE";

/// `com.amazonaws.lookoutequipment#IngestionJobStatus` values this emulator uses. The
/// full enum (smithy_query: shape `IngestionJobStatus`) is IN_PROGRESS, SUCCESS, FAILED,
/// IMPORT_IN_PROGRESS.
pub const INGESTION_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// The terminal status an ingestion job this emulator accepted lands in.
pub const INGESTION_STATUS_SUCCESS: &str = "SUCCESS";

/// How long a resource stays in a transient status before the clock owes it the settled
/// one.
///
/// A resource the service reports in a non-terminal status (a dataset in
/// `INGESTION_IN_PROGRESS`, an ingestion job in `IN_PROGRESS`) has a transition the clock
/// owes. AWS documents no duration for data ingestion, so this span is the emulator's own
/// choice; it is deliberately non-zero, so a caller reading straight after
/// `StartDataIngestionJob` still observes the transient status — and the state errors that
/// go with it — as it does against the real service. The instant it comes due is stored on
/// the record ([`PendingTransition::due_at`]) and materialised by [`settle`]; nothing
/// sleeps.
///
/// [`settle`]: ComAmazonawsLookoutequipmentAwslookoutequipmentfrontendserviceState::settle
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// `ListDatasets`' `MaxResults` upper bound, from its `@range(1, 500)`. Also the page size
/// used when a caller names none.
pub const MAX_RESULTS_LIMIT: i32 = 500;

/// How many datasets this emulator will hold.
///
/// The real service caps datasets per account per Region with a service quota, and
/// `CreateDataset` models `ServiceQuotaExceededException` ("Resource limitations have been
/// exceeded") for a create past it. The number here is NOT a fidelity claim about the
/// published quota: it is the emulator's own ceiling, set generously so no realistic
/// workflow meets it, so that a caller looping on create cannot grow this process's memory
/// without bound.
pub const MAX_DATASETS: usize = 1_000;

/// How many data-ingestion jobs this emulator will hold for ONE dataset, and how many in
/// total.
///
/// Same reasoning as [`MAX_DATASETS`], applied to the other store a request can grow: a
/// caller looping on `StartDataIngestionJob` must not be able to grow this process's
/// memory (the job map, and the parent dataset's own list of job ids) without bound.
/// Neither number is a fidelity claim about a published quota; both are the emulator's own
/// ceiling, set generously so no realistic workflow meets one, and a start past either is
/// refused with the modelled `ServiceQuotaExceededException` ("Resource limitations have
/// been exceeded").
pub const MAX_INGESTION_JOBS_PER_DATASET: usize = 100;
/// The whole-store ceiling on ingestion jobs. See [`MAX_INGESTION_JOBS_PER_DATASET`].
pub const MAX_INGESTION_JOBS: usize = 5_000;

/// How many `(operation, ClientToken)` records the idempotency map keeps.
///
/// A replay window need not be unbounded, and the token is a caller-chosen string, so the
/// map is capped and the OLDEST record is evicted once it is full: an idempotency replay
/// that far out of date is answered as a fresh request (for a create, that is the
/// `ConflictException` the existing name earns) rather than held in memory for the life of
/// the process.
pub const MAX_IDEMPOTENCY_RECORDS: usize = 10_000;

/// The idempotency-map operation key for `CreateDataset`.
pub const OP_CREATE_DATASET: &str = "CreateDataset";
/// The idempotency-map operation key for `StartDataIngestionJob`.
pub const OP_START_DATA_INGESTION_JOB: &str = "StartDataIngestionJob";

/// A status change the clock owes a record: where it lands, and when.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// The status the record settles into.
    pub to: String,
    /// The instant the transition comes due.
    pub due_at: SystemTime,
}

/// A stored dataset: everything `CreateDataset` was given, plus what the service minted
/// and what a later `StartDataIngestionJob` recorded against it.
#[derive(Debug, Clone)]
pub struct DatasetRecord {
    pub name: String,
    pub arn: String,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    /// A `DatasetStatus` value.
    pub status: String,
    /// The transition the clock owes, or `None` in a settled status.
    pub pending: Option<PendingTransition>,
    /// The inline JSON schema supplied as `DatasetSchema$InlineDataSchema` at create time.
    pub schema: Option<String>,
    /// The KMS key, resolved to the `KmsKeyArn` form `DescribeDataset` answers with.
    pub server_side_kms_key_id: Option<String>,
    pub ingestion_input_configuration: Option<IngestionInputConfiguration>,
    pub role_arn: Option<String>,
    pub data_start_time: Option<SystemTime>,
    pub data_end_time: Option<SystemTime>,
    pub source_dataset_arn: Option<String>,
    pub tags: Vec<Tag>,
    /// The `ClientToken` of the create that minted this dataset.
    pub client_token: String,
    /// The ingestion jobs run against this dataset, in start order.
    pub ingestion_job_ids: Vec<String>,
}

impl DatasetRecord {
    /// Materialise the transition the clock already owes this dataset. Answers whether the
    /// record is settled afterwards, so the caller can forget it.
    ///
    /// This is the ONLY write a reporting operation makes: the change was owed at this
    /// instant whether or not anybody called.
    fn settle(&mut self, now: SystemTime) -> bool {
        let Some(pending) = self.pending.clone() else {
            return true;
        };
        if now < pending.due_at {
            return false;
        }
        self.status = pending.to;
        // The change happened when the clock made it due, not when someone looked, so
        // two reads of a settled dataset answer the same `LastUpdatedAt`.
        self.last_updated_at = pending.due_at.max(self.last_updated_at);
        self.pending = None;
        true
    }

    /// True while a transition is still owed — the dataset is mid-ingestion.
    pub fn is_in_transition(&self) -> bool {
        self.pending.is_some()
    }

    /// The modelled `DatasetSummary` for this record, as `ListDatasets` returns it.
    pub fn to_summary(&self) -> DatasetSummary {
        DatasetSummary {
            dataset_name: Some(self.name.clone()),
            dataset_arn: Some(self.arn.clone()),
            status: Some(self.status.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
        }
    }
}

/// A stored data-ingestion job, keyed in state by its minted `JobId`.
#[derive(Debug, Clone)]
pub struct IngestionJobRecord {
    /// 32 hex characters, the shape `IngestionJobId`'s `^[A-Fa-f0-9]{0,32}$` takes.
    pub job_id: String,
    pub dataset_name: String,
    pub dataset_arn: String,
    pub ingestion_input_configuration: IngestionInputConfiguration,
    pub role_arn: String,
    pub created_at: SystemTime,
    /// An `IngestionJobStatus` value.
    pub status: String,
    /// The transition the clock owes, or `None` in a terminal status.
    pub pending: Option<PendingTransition>,
    /// The `ClientToken` of the start call that minted this job.
    pub client_token: String,
}

impl IngestionJobRecord {
    /// See [`DatasetRecord::settle`].
    fn settle(&mut self, now: SystemTime) -> bool {
        let Some(pending) = self.pending.clone() else {
            return true;
        };
        if now < pending.due_at {
            return false;
        }
        self.status = pending.to;
        self.pending = None;
        true
    }
}

/// Per-service state for com-amazonaws-lookoutequipment-awslookoutequipmentfrontendservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLookoutequipmentAwslookoutequipmentfrontendserviceState {
    /// Every dataset, keyed by its name — unique per account per Region — and ordered by
    /// it, so `ListDatasets` paginates deterministically.
    pub datasets: BTreeMap<String, DatasetRecord>,
    /// Every data-ingestion job, keyed by its minted `JobId`. `DatasetRecord`'s
    /// `ingestion_job_ids` is the per-dataset index into this map.
    pub ingestion_jobs: BTreeMap<String, IngestionJobRecord>,
    /// `(operation, ClientToken) -> resource id`, so a replayed `@idempotencyToken`
    /// answers with the first result instead of creating a duplicate. Capped at
    /// [`MAX_IDEMPOTENCY_RECORDS`].
    pub idempotency: BTreeMap<(String, String), String>,
    /// The idempotency keys in insertion order, so the map can evict its oldest record
    /// once it is full. Never longer than [`MAX_IDEMPOTENCY_RECORDS`].
    idempotency_order: VecDeque<(String, String)>,
    /// The datasets that still owe the clock a transition — the ONLY records [`Self::settle`]
    /// has to walk, so a reporting call does work proportional to what is in flight rather
    /// than to the whole store.
    pending_datasets: BTreeSet<String>,
    /// The ingestion jobs that still owe the clock a transition. See [`Self::pending_datasets`].
    pending_jobs: BTreeSet<String>,
    /// Advanced once per minted id, so no id is ever a function of the request.
    id_seed: u64,
}

impl ComAmazonawsLookoutequipmentAwslookoutequipmentfrontendserviceState {
    /// Materialise every status transition the clock already owes. Called at the top of
    /// each handler, reporting ones included: they may write only a change that was
    /// already due at this instant.
    pub fn settle(&mut self, now: SystemTime) {
        for job_id in self.pending_jobs.iter().cloned().collect::<Vec<String>>() {
            let settled = self
                .ingestion_jobs
                .get_mut(&job_id)
                .is_none_or(|job| job.settle(now));
            if settled {
                self.pending_jobs.remove(&job_id);
            }
        }
        for name in self.pending_datasets.iter().cloned().collect::<Vec<String>>() {
            let settled = self
                .datasets
                .get_mut(&name)
                .is_none_or(|dataset| dataset.settle(now));
            if settled {
                self.pending_datasets.remove(&name);
            }
        }
    }

    /// Record that `name` / `job_id` owes the clock a transition, so [`Self::settle`] knows
    /// to look at it.
    pub fn note_pending_dataset(&mut self, name: &str) {
        self.pending_datasets.insert(name.to_string());
    }

    /// See [`Self::note_pending_dataset`].
    pub fn note_pending_job(&mut self, job_id: &str) {
        self.pending_jobs.insert(job_id.to_string());
    }

    /// The next value of the id counter: seeded from the clock on first use and advanced
    /// through a bijection, so a minted id is never a function of the request and two
    /// identical requests never mint the same one.
    fn next_u64(&mut self) -> u64 {
        if self.id_seed == 0 {
            self.id_seed = now_nanos();
        }
        self.id_seed = splitmix64(self.id_seed);
        self.id_seed
    }

    /// Mint the trailing segment of a dataset ARN: a version-4-shaped UUID, which is what
    /// `DatasetArn`'s `^arn:…:dataset\/[0-9a-zA-Z_-]{1,200}\/.+$` tail carries.
    pub fn mint_dataset_uuid(&mut self) -> String {
        let high = self.next_u64();
        let low = self.next_u64();
        let mut hex = format!("{high:016x}{low:016x}");
        // Version nibble 4 and a variant nibble in 8..=b, as a random UUID carries.
        hex.replace_range(12..13, "4");
        let variant = char::from(b"89ab"[(low & 0x3) as usize]);
        hex.replace_range(16..17, &variant.to_string());
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// Mint an ingestion `JobId`: 32 hex characters, per `IngestionJobId`'s
    /// `^[A-Fa-f0-9]{0,32}$` and `@length(0, 32)`.
    pub fn mint_job_id(&mut self) -> String {
        let high = self.next_u64();
        let low = self.next_u64();
        format!("{high:016x}{low:016x}")
    }

    /// The resource id a previous call recorded for `(operation, client_token)`.
    pub fn replayed(&self, operation: &str, client_token: &str) -> Option<&str> {
        self.idempotency
            .get(&(operation.to_string(), client_token.to_string()))
            .map(String::as_str)
    }

    /// Record `(operation, client_token) -> resource_id` so a replay answers with it,
    /// evicting the oldest record once the map holds [`MAX_IDEMPOTENCY_RECORDS`].
    pub fn remember(&mut self, operation: &str, client_token: &str, resource_id: &str) {
        let key = (operation.to_string(), client_token.to_string());
        self.idempotency.insert(key.clone(), resource_id.to_string());
        self.idempotency_order.push_back(key);
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_RECORDS {
            // A key already dropped by `remove_dataset` is simply absent; popping it still
            // shortens the queue, so neither the queue nor the map can exceed the cap.
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency.remove(&oldest);
            }
        }
    }

    /// Drop the dataset named `name`, its ingestion jobs, and every idempotency record
    /// that pointed at any of them — so the names and tokens it held are free again.
    pub fn remove_dataset(&mut self, name: &str) -> Option<DatasetRecord> {
        let dataset = self.datasets.remove(name)?;
        self.pending_datasets.remove(name);
        for job_id in &dataset.ingestion_job_ids {
            self.ingestion_jobs.remove(job_id);
            self.pending_jobs.remove(job_id);
        }
        let gone: Vec<(String, String)> = self
            .idempotency
            .iter()
            .filter(|((operation, _), resource_id)| {
                (operation == OP_CREATE_DATASET && resource_id.as_str() == name)
                    || (operation == OP_START_DATA_INGESTION_JOB
                        && dataset.ingestion_job_ids.contains(resource_id))
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in gone {
            self.idempotency.remove(&key);
        }
        Some(dataset)
    }
}

/// The region this request was signed for, read from its SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a dataset: `arn:aws:lookoutequipment:<region>:<account>:dataset/<name>/<id>`,
/// the shape `DatasetArn` requires with its
/// `^arn:aws(-[^:]+)?:lookoutequipment:[a-zA-Z0-9\-]*:[0-9]{12}:dataset\/[0-9a-zA-Z_-]{1,200}\/.+$`.
pub fn dataset_arn(region: &str, dataset_name: &str, dataset_uuid: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("dataset/{dataset_name}/{dataset_uuid}"),
    )
}

/// The KMS key id, alias or ARN a create supplied, as the key ARN a read answers with.
///
/// `CreateDatasetRequest$ServerSideKmsKeyId` is a `NameOrArn` (a key id, an alias or an
/// ARN), while `DescribeDatasetResponse$ServerSideKmsKeyId` is a `KmsKeyArn`
/// (`^arn:aws[a-z\-]*:kms:[a-z0-9\-]*:\d{12}:[\w\-\/]+$`) — so the stored value is the
/// resolved ARN, exactly as the real service reports the key it encrypted with.
pub fn kms_key_arn(region: &str, name_or_arn: &str) -> String {
    if name_or_arn.starts_with("arn:") {
        return name_or_arn.to_string();
    }
    let resource = if name_or_arn.starts_with("alias/") {
        name_or_arn.to_string()
    } else {
        format!("key/{name_or_arn}")
    };
    ::engine::emulator_arn("kms", region, &resource)
}

/// One page of a `@paginated` listing: the items to return, plus the cursor that resumes
/// exactly where the page ended (`None` when the page is the last one).
#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// A `NextToken` no listing of this service ever minted.
#[derive(Debug, Clone, Copy)]
pub struct MalformedCursor;

/// Prefix the cursor plaintext carries, so a token this service never minted cannot decode
/// into a usable position.
const CURSOR_PREFIX: &str = "lookoutequipment-after-dataset:";

/// Encode the resume cursor that follows the dataset named `dataset_name`.
///
/// Opaque to the caller (hex of the internal cursor text) and well within `NextToken`'s
/// `@length(0, 8192)` and `^\p{ASCII}{0,8192}$`. The cursor names a POSITION in the name
/// ordering rather than an offset, so resuming never repeats or skips a dataset when one is
/// created or deleted between pages.
///
/// It is self-describing, so `ListDatasets` validates a token by decoding it and never has
/// to record the tokens it issued — which matters because `ListDatasets` is a reporting
/// operation: remembering an issued token would make a read mutate state.
pub fn encode_cursor(dataset_name: &str) -> String {
    ::hex::encode(format!("{CURSOR_PREFIX}{dataset_name}"))
}

/// Decode a cursor minted by [`encode_cursor`]; `None` starts at the beginning.
pub fn decode_cursor(next_token: Option<&str>) -> Result<Option<String>, MalformedCursor> {
    let Some(token) = next_token else {
        return Ok(None);
    };
    if token.is_empty() {
        // `NextToken` is `@length(0, 8192)`, so the empty string is a legal value; it
        // names no position, which is the start.
        return Ok(None);
    }
    let bytes = ::hex::decode(token).map_err(|_| MalformedCursor)?;
    let text = String::from_utf8(bytes).map_err(|_| MalformedCursor)?;
    text.strip_prefix(CURSOR_PREFIX)
        .map(|name| Some(name.to_string()))
        .ok_or(MalformedCursor)
}

/// Take the page of `ordered` (sorted by dataset name) that follows `after`, at most
/// `max_results` long.
///
/// `max_results` is `@range(1, 500)`, so the boundary validator has already refused
/// anything outside it; an absent one takes a full page.
pub fn page_of<T>(
    ordered: impl IntoIterator<Item = (String, T)>,
    after: Option<&str>,
    max_results: Option<i32>,
) -> Page<T> {
    let limit = max_results
        .unwrap_or(MAX_RESULTS_LIMIT)
        .clamp(1, MAX_RESULTS_LIMIT) as usize;
    let mut items = Vec::new();
    let mut last_name = String::new();
    let mut more_follow = false;
    for (name, item) in ordered
        .into_iter()
        .filter(|(name, _)| after.is_none_or(|after| name.as_str() > after))
    {
        if items.len() == limit {
            // One item beyond the page proves a next page exists; the cursor names where
            // this page ended, so resuming never repeats or skips an item.
            more_follow = true;
            break;
        }
        last_name = name;
        items.push(item);
    }
    Page {
        items,
        next_token: more_follow.then(|| encode_cursor(&last_name)),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLookoutequipmentAwslookoutequipmentfrontendserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLookoutequipmentAwslookoutequipmentfrontendserviceState>)
}

/// Wall-clock nanoseconds since the epoch, used only to seed the id counter.
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |elapsed| elapsed.as_nanos() as u64)
        | 1
}

/// SplitMix64: a bijection on `u64`, so advancing the id counter through it never revisits
/// a value before the counter wraps.
fn splitmix64(state: u64) -> u64 {
    let mut z = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
