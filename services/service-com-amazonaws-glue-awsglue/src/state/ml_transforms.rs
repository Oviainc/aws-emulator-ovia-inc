// SPDX-License-Identifier: CC0-1.0

//! The stored machine-learning transform records, the task runs started of them, and the
//! capacity, ordering and instant helpers their listings page with.

use super::*;

/// How many machine learning transform ids this process has minted. A nonce source for
/// [`new_ml_transform_id`], exactly as [`MINTED_DATABASE_IDS`] is for a database id.
pub(super) static MINTED_ML_TRANSFORM_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `TransformId`: the id the service assigns a machine learning transform when
/// `CreateMLTransform` stores it.
///
/// 32 lowercase hexadecimal characters, i.e. 128 bits, which is inside `HashString`'s
/// `length 1..255` — the same shape and the same width as the other `HashString` id this
/// store mints ([`new_data_quality_rule_recommendation_run_id`]), because the service
/// publishes no spelling for this one. The bits come from [`mint_hex_id`], so two creates
/// landing in the same nanosecond cannot be given the same id, and the id is stored on the
/// record, so "the ID is guaranteed to be unique and does not change"
/// (`MLTransform$TransformId`) holds across every later read.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_ml_transform_id() -> String {
    mint_hex_id(&MINTED_ML_TRANSFORM_IDS, ML_TRANSFORM_ID_WORDS)
}

/// How many 64-bit words a machine learning transform id carries: two, i.e. 128 bits
/// rendered as 32 lowercase hexadecimal characters.
pub(super) const ML_TRANSFORM_ID_WORDS: usize = 2;

/// The `TransformStatusType` a machine learning transform is stored in by a create: of the
/// modelled enum (`NOT_READY`, `READY`, `DELETING`), the one that says the transform
/// cannot be used yet.
///
/// A create registers the transform and nothing more — "Call this operation as the first
/// step in the process of using a machine learning transform" (`CreateMLTransform`) — so
/// it holds no labels and has been taught nothing. Moving on from here takes a TASK RUN
/// that teaches it, not the passage of time, so no later status is owed by the clock.
pub const TRANSFORM_STATUS_NOT_READY: &str = "NOT_READY";

/// The `SortDirectionType` that reverses a listing's order: one of the modelled values
/// (`ASCENDING`, `DESCENDING`), the one a sort criteria names to walk the order backwards.
pub const SORT_DIRECTION_DESCENDING: &str = "DESCENDING";

/// The `TransformSortColumnType` that orders transforms by their creation instant. One of
/// the modelled values the generated [`crate::ops::types::TRANSFORM_SORT_COLUMN_TYPE_VALUES`]
/// carries (`CREATED`, `LAST_MODIFIED`, `NAME`, `STATUS`, `TRANSFORM_TYPE`), which the
/// request layer validates a sort criteria against.
pub const TRANSFORM_SORT_COLUMN_CREATED: &str = "CREATED";
/// See [`TRANSFORM_SORT_COLUMN_CREATED`]: orders by the instant last modified.
pub const TRANSFORM_SORT_COLUMN_LAST_MODIFIED: &str = "LAST_MODIFIED";
/// See [`TRANSFORM_SORT_COLUMN_CREATED`]: orders by the transform's name.
pub const TRANSFORM_SORT_COLUMN_NAME: &str = "NAME";
/// See [`TRANSFORM_SORT_COLUMN_CREATED`]: orders by the transform's status.
pub const TRANSFORM_SORT_COLUMN_STATUS: &str = "STATUS";
/// See [`TRANSFORM_SORT_COLUMN_CREATED`]: orders by the transform's algorithm type.
pub const TRANSFORM_SORT_COLUMN_TRANSFORM_TYPE: &str = "TRANSFORM_TYPE";

/// `MLTransform$GlueVersion` when the create named none: "If the value is not set, the
/// Glue compatibility defaults to Glue 0.9" (`CreateMLTransform$GlueVersion`).
pub const DEFAULT_ML_TRANSFORM_GLUE_VERSION: &str = "0.9";

/// `MLTransform$MaxCapacity` when the create named no capacity at all: "You can allocate
/// from 2 to 100 DPUs; the default is 10" (`CreateMLTransform$MaxCapacity`).
pub const DEFAULT_ML_TRANSFORM_DPUS: f64 = 10.0;

/// `MLTransform$Timeout` when the create named none: "The default is 2,880 minutes (48
/// hours)" (`CreateMLTransform$Timeout`).
pub const DEFAULT_ML_TRANSFORM_TIMEOUT_MINUTES: i32 = 2_880;

/// How many columns a transform's schema holds at most: the modelled length of
/// `MLTransform$Schema` (`TransformSchema`, length 0..100) — "Has an upper bound of 100
/// columns".
pub const MAX_TRANSFORM_SCHEMA_COLUMNS: usize = 100;

/// The separator between the parts of a sorted listing's ordering key (and so of its
/// `NextToken`): the sorted value and then the record's own id.
///
/// ONE copy, shared by the listings that page over a value a `Sort` names (the machine
/// learning transforms and the task runs of one transform), so the token format cannot
/// drift between them. `U+0001` cannot appear in a name, a status, a type or a minted id:
/// every such value's modelled pattern admits `\u0020` and up, so the separator can never
/// be confused with a character inside one of the parts it joins — and a key whose first
/// part is a prefix of another's therefore still orders before it.
pub(super) const LISTING_KEY_SEPARATOR: char = '\u{1}';

/// `at` rendered so that lexicographic order over the rendering is the order of the
/// instants themselves: nanoseconds since the epoch, zero-padded, so no comparison of
/// ordering keys has to parse a number back out. Twenty digits hold any nanosecond count
/// this clock can report.
pub(super) fn ordered_instant(at: SystemTime) -> String {
    let nanos = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{nanos:020}")
}

/// `at` as seconds since the epoch, the one scale a stored instant is compared against a
/// client-supplied timestamp on.
pub(super) fn secs_since_epoch(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Whether `at` sits inside the window a filter's `…Before`/`…After` pair describes.
///
/// One reading, shared by every filtered listing in this service: a bound the criteria
/// leave out narrows nothing, and a bound they name admits only an instant STRICTLY inside
/// it — "the time and date before which the … were created" is a window the instant is
/// before, not one it may sit on the edge of.
pub(super) fn instant_window_admits(
    at: SystemTime,
    before: Option<&::aws_smithy_types::DateTime>,
    after: Option<&::aws_smithy_types::DateTime>,
) -> bool {
    let instant = secs_since_epoch(at);
    before.is_none_or(|bound| instant < bound.as_secs_f64())
        && after.is_none_or(|bound| instant > bound.as_secs_f64())
}

/// A stored machine learning transform: the definition `CreateMLTransform` registered,
/// plus the members the SERVER fills in.
///
/// Every member of the modelled `MLTransform` shape is held here, plus the create's
/// `Tags`, which that shape does not carry — so a later read reports what the create
/// stored and nothing invented on the way out. The server-set members are the minted
/// `TransformId`, the two instants, the status every create starts in
/// ([`TRANSFORM_STATUS_NOT_READY`]), the label count and the SCHEMA, which is taken from
/// the catalog tables the create named rather than from the request.
///
/// `EvaluationMetrics` is NOT held: metrics "provide an estimate of the quality of your
/// machine learning transform" (`MLTransform$EvaluationMetrics`), and nothing has trained
/// or evaluated this transform, so there are none to report.
#[derive(Debug, Clone)]
pub struct MlTransformEntry {
    /// `MLTransform$TransformId`, minted by [`new_ml_transform_id`], which is also this
    /// record's key in [`ComAmazonawsGlueAwsglueState::ml_transforms`].
    pub transform_id: String,
    /// "A user-defined name for the machine learning transform", stored exactly as sent.
    /// NOT unique over the namespace: "Names are not guaranteed unique and can be changed
    /// at any time" (`MLTransform$Name`).
    pub name: String,
    /// "A user-defined, long-form description text", or `None` when the create named none.
    pub description: Option<String>,
    /// One of the modelled `TransformStatusType` values; see
    /// [`TRANSFORM_STATUS_NOT_READY`].
    pub status: String,
    /// "The time and date that this machine learning transform was created."
    pub created_on: SystemTime,
    /// "The last point in time when this machine learning transform was modified", which
    /// is the create's own instant until a write moves it.
    pub last_modified_on: SystemTime,
    /// "A list of Glue table definitions used by the transform", as the create addressed
    /// them.
    pub input_record_tables: Vec<GlueTable>,
    /// "The algorithm-specific parameters that are associated with the machine learning
    /// transform", stored exactly as sent.
    pub parameters: TransformParameters,
    /// "A count identifier for the labeling files generated by Glue for this transform"
    /// (`MLTransform$LabelCount`): zero on a create, which generates none, and the count a
    /// labelling write would move.
    pub label_count: i32,
    /// "The columns and data types that this transform can run against"
    /// (`MLTransform$Schema`), taken from the catalog columns of
    /// [`Self::input_record_tables`] when the transform was stored — the data the transform
    /// reads is what decides them, so they are read out of the catalog rather than taken
    /// from the request, which carries no schema at all.
    pub schema: Vec<SchemaColumn>,
    /// "The name or Amazon Resource Name (ARN) of the IAM role with the required
    /// permissions", stored exactly as sent.
    pub role: String,
    /// "This value determines which version of Glue this machine learning transform is
    /// compatible with", defaulting to [`DEFAULT_ML_TRANSFORM_GLUE_VERSION`].
    pub glue_version: String,
    /// "The number of Glue data processing units (DPUs) that are allocated to task runs for
    /// this transform", as [`ml_transform_capacity`] resolved it.
    pub max_capacity: Option<f64>,
    /// "The type of predefined worker that is allocated when a task of this transform
    /// runs", or `None` when the create named none.
    pub worker_type: Option<String>,
    /// "The number of workers of a defined `workerType` that are allocated when a task of
    /// the transform runs", or `None` when the create named none.
    pub number_of_workers: Option<i32>,
    /// "The timeout in minutes of the machine learning transform", defaulting to
    /// [`DEFAULT_ML_TRANSFORM_TIMEOUT_MINUTES`].
    pub timeout: i32,
    /// "The maximum number of times to retry after an `MLTaskRun` of the machine learning
    /// transform fails", or `None` when the create named none: the model documents no
    /// default for it, so nothing is reported where the client asked for nothing.
    pub max_retries: Option<i32>,
    /// "The encryption-at-rest settings of the transform that apply to accessing user
    /// data", stored exactly as sent.
    pub transform_encryption: Option<TransformEncryption>,
    /// The create's `Tags`, which are not part of the `MLTransform` shape.
    ///
    /// Kept so the create does not discard what the client sent, and NOT reported by a
    /// read: the shape carries no tag member, so the tags are reachable only through the
    /// tagging operations.
    pub tags: HashMap<String, String>,
    /// The task runs started of this transform, oldest first.
    ///
    /// Held ON the transform, the way a job definition holds its own runs
    /// ([`JobEntry::runs`]): every operation that addresses a task run
    /// (`GetMLTaskRun`, `CancelMLTaskRun`) names the parent `TransformId` and then the
    /// `TaskRunId` the start minted, so a run of one transform is unreachable through
    /// another. Kept in start order, so a listing that names no sort criteria still has
    /// one deterministic order to page over, and bounded by
    /// [`MAX_TASK_RUNS_PER_TRANSFORM`].
    pub task_runs: Vec<MlTaskRunEntry>,
}

impl MlTransformEntry {
    /// Build a stored transform from a `CreateMLTransform` request served at `now`.
    ///
    /// The definition comes from the request; `transform_id` is the minted id and `schema`
    /// the columns the catalog holds for the request's input tables
    /// ([`ComAmazonawsGlueAwsglueState::ml_transform_schema`]).
    pub fn from_create_input(
        input: &CreateMLTransformInput,
        transform_id: String,
        schema: Vec<SchemaColumn>,
        now: SystemTime,
    ) -> Self {
        let (worker_type, number_of_workers, max_capacity) = ml_transform_capacity(
            input.worker_type.as_deref(),
            input.number_of_workers,
            input.max_capacity,
        );
        Self {
            transform_id,
            name: input.name.clone(),
            description: input.description.clone(),
            status: TRANSFORM_STATUS_NOT_READY.to_string(),
            created_on: now,
            last_modified_on: now,
            input_record_tables: input.input_record_tables.clone(),
            parameters: input.parameters.clone(),
            // A create generates no labelling file.
            label_count: 0,
            schema,
            role: input.role.clone(),
            glue_version: input
                .glue_version
                .clone()
                .unwrap_or_else(|| DEFAULT_ML_TRANSFORM_GLUE_VERSION.to_string()),
            max_capacity,
            worker_type,
            number_of_workers,
            timeout: input
                .timeout
                .unwrap_or(DEFAULT_ML_TRANSFORM_TIMEOUT_MINUTES),
            max_retries: input.max_retries,
            transform_encryption: input.transform_encryption.clone(),
            tags: input.tags.clone().unwrap_or_default(),
            // A create starts no task run: "Call this operation as the first step in the
            // process of using a machine learning transform" (`CreateMLTransform`).
            task_runs: Vec::new(),
        }
    }

    /// Apply an `UpdateMLTransform` request to this stored transform, member by member,
    /// as served at `now`.
    ///
    /// Every member the request CARRIES replaces the stored one and every member it OMITS
    /// is preserved: the request is a patch over the definition the create registered, not
    /// a replacement for it, which is why "Call this operation to tune the algorithm
    /// parameters" (`UpdateMLTransform`) names no required member beyond the `TransformId`
    /// that addresses the transform. The three capacity members are resolved TOGETHER,
    /// through the one rule the create uses ([`ml_transform_capacity`]), from the ones the
    /// request names falling back on the ones already stored — so a request that moves the
    /// worker configuration moves the DPU figure with it, and one that names no capacity
    /// member at all leaves the stored figure where it was.
    ///
    /// The members `UpdateMLTransformRequest` has NO member for are not this request's to
    /// change: the minted `TransformId`, the creation instant, the status, the label count,
    /// the input tables and the schema they decide, the encryption settings and the tags
    /// all stay as they are. `LastModifiedOn` becomes `now`, since it is "the last point in
    /// time when this machine learning transform was modified" (`MLTransform`).
    pub fn apply_update(&mut self, input: &UpdateMLTransformInput, now: SystemTime) {
        if let Some(name) = input.name.as_ref() {
            self.name = name.clone();
        }
        if let Some(description) = input.description.as_ref() {
            self.description = Some(description.clone());
        }
        if let Some(parameters) = input.parameters.as_ref() {
            self.parameters = parameters.clone();
        }
        if let Some(role) = input.role.as_ref() {
            self.role = role.clone();
        }
        if let Some(glue_version) = input.glue_version.as_ref() {
            self.glue_version = glue_version.clone();
        }
        if let Some(timeout) = input.timeout {
            self.timeout = timeout;
        }
        if let Some(max_retries) = input.max_retries {
            self.max_retries = Some(max_retries);
        }
        let (worker_type, number_of_workers, max_capacity) = ml_transform_capacity(
            input
                .worker_type
                .as_deref()
                .or(self.worker_type.as_deref()),
            input.number_of_workers.or(self.number_of_workers),
            input.max_capacity.or(self.max_capacity),
        );
        self.worker_type = worker_type;
        self.number_of_workers = number_of_workers;
        self.max_capacity = max_capacity;
        self.last_modified_on = now;
    }

    /// Rebuild the modelled `MLTransform` shape from what is stored, so a listing reports
    /// the same transform a single read does. The stored `Tags` are not part of that shape,
    /// so they stay out of it.
    pub fn to_ml_transform(&self) -> MLTransform {
        MLTransform {
            transform_id: Some(self.transform_id.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            status: Some(self.status.clone()),
            created_on: Some(to_date_time(self.created_on)),
            last_modified_on: Some(to_date_time(self.last_modified_on)),
            input_record_tables: Some(self.input_record_tables.clone()),
            parameters: Some(self.parameters.clone()),
            // Nothing has trained or evaluated this transform, so it has no metrics.
            evaluation_metrics: None,
            label_count: self.label_count,
            schema: Some(self.schema.clone()),
            role: Some(self.role.clone()),
            glue_version: Some(self.glue_version.clone()),
            max_capacity: self.max_capacity,
            worker_type: self.worker_type.clone(),
            number_of_workers: self.number_of_workers,
            timeout: Some(self.timeout),
            max_retries: self.max_retries,
            transform_encryption: self.transform_encryption.clone(),
        }
    }

    /// Whether this transform satisfies every criterion `filter` names.
    ///
    /// A `TransformFilterCriteria` is a conjunction: each member it carries narrows the
    /// listing, and a member it leaves out narrows nothing. The three enum members and the
    /// name are matched exactly, the two instant pairs bound the window the criteria
    /// describe ("the time and date before which the transforms were created", so an
    /// instant strictly inside the bound), and `Schema` selects "datasets with a specific
    /// schema", i.e. the transforms whose own schema carries every column it names.
    pub fn matches(&self, filter: &TransformFilterCriteria) -> bool {
        if filter
            .name
            .as_deref()
            .is_some_and(|name| name != self.name.as_str())
        {
            return false;
        }
        if filter
            .transform_type
            .as_deref()
            .is_some_and(|kind| kind != self.parameters.transform_type.as_str())
        {
            return false;
        }
        if filter
            .status
            .as_deref()
            .is_some_and(|status| status != self.status.as_str())
        {
            return false;
        }
        if filter
            .glue_version
            .as_deref()
            .is_some_and(|version| version != self.glue_version.as_str())
        {
            return false;
        }
        if !instant_window_admits(
            self.created_on,
            filter.created_before.as_ref(),
            filter.created_after.as_ref(),
        ) || !instant_window_admits(
            self.last_modified_on,
            filter.last_modified_before.as_ref(),
            filter.last_modified_after.as_ref(),
        ) {
            return false;
        }
        filter.schema.as_deref().is_none_or(|wanted| {
            wanted.iter().all(|column| {
                self.schema.iter().any(|held| {
                    held.name == column.name
                        && (column.data_type.is_none() || held.data_type == column.data_type)
                })
            })
        })
    }

    /// This transform's place in a listing ordered by `column`, as the continuation token
    /// renders it: the sorted value, then the transform's own id so that two transforms
    /// sharing that value still have one total order to page over.
    ///
    /// `None` is the order a request that names no sort criteria gets, which is the same
    /// key [`TRANSFORM_SORT_COLUMN_NAME`] names: the model documents no default order, so
    /// the listing falls back on a deterministic one rather than on however the store
    /// happens to hold its keys. A column outside the modelled list cannot reach here —
    /// the request layer validates `Sort$Column` against it — and is ordered the same way.
    pub fn ordering_key(&self, column: Option<&str>) -> String {
        let sorted = match column {
            Some(TRANSFORM_SORT_COLUMN_TRANSFORM_TYPE) => self.parameters.transform_type.clone(),
            Some(TRANSFORM_SORT_COLUMN_STATUS) => self.status.clone(),
            Some(TRANSFORM_SORT_COLUMN_CREATED) => ordered_instant(self.created_on),
            Some(TRANSFORM_SORT_COLUMN_LAST_MODIFIED) => ordered_instant(self.last_modified_on),
            Some(TRANSFORM_SORT_COLUMN_NAME) | Some(_) | None => self.name.clone(),
        };
        format!(
            "{sorted}{LISTING_KEY_SEPARATOR}{}",
            self.transform_id
        )
    }

    /// Give every task run of this transform the status the CLOCK already owed it at
    /// `now`.
    ///
    /// The one write a reporting operation performs, exactly as [`JobEntry::settle_due_runs`]
    /// is for a job run: a run's next status was due at [`MlTaskRunEntry::settles_at`]
    /// whether or not anybody called, so materialising it on read reports the status the
    /// clock had already reached. It never moves a run because something was read.
    pub fn settle_due_task_runs(&mut self, now: SystemTime) {
        for run in &mut self.task_runs {
            run.settle_if_due(now);
        }
    }

    /// Append a task run of this transform, keeping at most
    /// [`MAX_TASK_RUNS_PER_TRANSFORM`] of them.
    pub fn record_task_run(&mut self, run: MlTaskRunEntry) {
        self.task_runs.push(run);
        if self.task_runs.len() > MAX_TASK_RUNS_PER_TRANSFORM {
            let overflow = self.task_runs.len() - MAX_TASK_RUNS_PER_TRANSFORM;
            self.task_runs.drain(..overflow);
        }
    }

    /// The ONE task run of this transform the id `run_id` addresses, if it holds one.
    ///
    /// A run's address is its parent `TransformId` and then the `TaskRunId` the start
    /// minted, so the search is over THIS transform's runs alone and a run of another
    /// transform is unreachable through it. The id is matched exactly, as the store's own
    /// key: the service mints it ([`new_ml_task_run_id`]) rather than taking it from a
    /// request, so no other spelling of it names a run.
    ///
    /// Call [`Self::settle_due_task_runs`] first, or the run is reported in a status the
    /// clock has already moved it out of.
    pub fn task_run(&self, run_id: &str) -> Option<&MlTaskRunEntry> {
        self.task_runs.iter().find(|run| run.id == run_id)
    }

    /// Every task run of this transform `filter` admits, in the order `sort` names, each
    /// paired with its ordering key — which is what the listing's continuation token
    /// carries ([`MlTaskRunEntry::ordering_key`]).
    ///
    /// Call [`Self::settle_due_task_runs`] first, or a run the clock has already finished
    /// is reported, and filtered on, in a status it has already left. Bounded by
    /// [`MAX_TASK_RUNS_PER_TRANSFORM`].
    pub fn task_runs_in_order(
        &self,
        filter: Option<&TaskRunFilterCriteria>,
        sort: Option<&TaskRunSortCriteria>,
    ) -> Vec<(String, &MlTaskRunEntry)> {
        let column = sort.map(|criteria| criteria.column.as_str());
        let mut ordered: Vec<(String, &MlTaskRunEntry)> = self
            .task_runs
            .iter()
            .filter(|run| filter.is_none_or(|criteria| run.matches(criteria)))
            .map(|run| (run.ordering_key(column), run))
            .collect();
        ordered.sort_by(|left, right| left.0.cmp(&right.0));
        if sort.is_some_and(|criteria| criteria.sort_direction == SORT_DIRECTION_DESCENDING) {
            // The ordering key is unique per run, so reversing the ascending order is
            // exactly the descending one.
            ordered.reverse();
        }
        ordered
    }
}

/// The capacity a machine learning transform runs its task runs with, as `(worker_type,
/// number_of_workers, max_capacity)`, resolved from the three capacity members a request
/// asks for.
///
/// `MaxCapacity` and the worker configuration are two spellings of one quantity, and the
/// worker configuration wins where the request names one: "When the `WorkerType` field is
/// set to a value other than `Standard`, the `MaxCapacity` field is set automatically and
/// becomes read-only" (`CreateMLTransform$WorkerType`), so the figure reported is the DPUs
/// those workers map to ([`dpu_per_worker`]). That sentence excepts `Standard`, which the
/// model gives no per-worker figure at all, so a worker configuration this emulator cannot
/// convert leaves the figure the REQUEST named — nothing the client sent is discarded, and
/// nothing is derived where the service documents no conversion. Where the request names
/// no capacity at all the documented default applies ([`DEFAULT_ML_TRANSFORM_DPUS`]).
///
/// Taken as three values rather than as one request so that the create, which resolves
/// them from the request alone, and an UPDATE, which resolves them from the members the
/// request names falling back on the ones the transform already carries
/// ([`MlTransformEntry::apply_update`]), cannot disagree about one capacity. Resolving a
/// resolved triple again answers it unchanged, so a write that names no capacity member
/// leaves the stored one where it was.
pub fn ml_transform_capacity(
    worker_type: Option<&str>,
    number_of_workers: Option<i32>,
    max_capacity: Option<f64>,
) -> (Option<String>, Option<i32>, Option<f64>) {
    match (worker_type, number_of_workers) {
        (Some(worker_type), workers) => (
            Some(worker_type.to_string()),
            workers,
            dpu_per_worker(worker_type)
                .zip(workers)
                .map(|(per_worker, workers)| per_worker * f64::from(workers))
                .or(max_capacity),
        ),
        // A worker count with no worker type names no DPU figure this emulator could
        // derive, so only what the request itself named is reported.
        (None, Some(workers)) => (None, Some(workers), max_capacity),
        (None, None) => (
            None,
            None,
            Some(max_capacity.unwrap_or(DEFAULT_ML_TRANSFORM_DPUS)),
        ),
    }
}

/// Whether an `UpdateMLTransform` request names a member to change at all.
///
/// Every member of that request other than the `TransformId` addressing the transform is
/// optional, so a request carrying none of them asks for no change: nothing is written, so
/// the transform's `LastModifiedOn` stays where the last write that did change something
/// put it — the same way an `UpdateCrawler` naming no member leaves its crawler's
/// `LastUpdated` alone ([`crawler_update_names_a_change`]).
pub fn ml_transform_update_names_a_change(input: &UpdateMLTransformInput) -> bool {
    input.name.is_some()
        || input.description.is_some()
        || input.parameters.is_some()
        || input.role.is_some()
        || input.glue_version.is_some()
        || input.max_capacity.is_some()
        || input.worker_type.is_some()
        || input.number_of_workers.is_some()
        || input.timeout.is_some()
        || input.max_retries.is_some()
}

/// How many task runs of ONE machine learning transform this store keeps.
///
/// CHOSEN: Glue publishes no retention for a transform's task run history, so this is the
/// emulator's own window, for the reason [`MAX_RUNS_PER_JOB`] gives: it bounds
/// [`MlTransformEntry::task_runs`] and every walk over it. The OLDEST run is dropped
/// first, so the runs a caller is most likely to read are kept.
pub const MAX_TASK_RUNS_PER_TRANSFORM: usize = 100;

/// The `TaskType` of a run `StartMLLabelingSetGenerationTaskRun` starts: of the modelled
/// enum (`EVALUATION | LABELING_SET_GENERATION | IMPORT_LABELS | EXPORT_LABELS |
/// FIND_MATCHES`), the one that names generating a labelling set, which is what that
/// operation starts ("Starts the active learning workflow for your machine learning
/// transform to improve the transform's quality by generating label sets").
pub const ML_TASK_TYPE_LABELING_SET_GENERATION: &str = "LABELING_SET_GENERATION";

/// The `TaskStatusType` a task run is recorded in by the call that starts it: of the
/// modelled enum (`STARTING | RUNNING | STOPPING | STOPPED | SUCCEEDED | FAILED |
/// TIMEOUT`), the one that says the run has been accepted and no task has begun yet.
pub const ML_TASK_RUN_STATUS_STARTING: &str = "STARTING";

/// See [`ML_TASK_RUN_STATUS_STARTING`]: the status a run that did not run comes to rest
/// in.
pub const ML_TASK_RUN_STATUS_FAILED: &str = "FAILED";

/// How long a task run stays in the status the call that started it put it in before the
/// clock owes it the next one.
///
/// CHOSEN, not documented: Glue publishes no duration for starting a machine learning
/// task run, so this is the emulator's own span, the same one a job run's transitions take
/// ([`JOB_RUN_SETTLES_AFTER`]) — deliberately not zero, so a caller that lists the runs
/// straight after a start still observes the run in flight. The due instant is stored on
/// the run and materialised by whichever later call reads it; nothing sleeps.
pub const ML_TASK_RUN_SETTLES_AFTER: Duration = JOB_RUN_SETTLES_AFTER;

/// `TaskRun$ErrorString` for the terminal status the clock owes every machine learning
/// task run.
///
/// This emulator is the Glue control plane: it records a task run faithfully, but it runs
/// no machine learning task over the transform's data, so a run can never reach
/// `SUCCEEDED` — it comes to rest in the modelled failure status carrying a reason that
/// says exactly that, rather than reporting a labelling set or a metric nothing produced.
pub const ML_TASK_RUN_NOT_EXECUTED_MESSAGE: &str =
    "This emulator records machine learning task runs but runs no machine learning task, \
     so the task run ended without running.";

/// The `TaskRunSortColumnType` that orders a transform's task runs by their task type. One
/// of the modelled values the generated
/// [`crate::ops::types::TASK_RUN_SORT_COLUMN_TYPE_VALUES`] carries (`TASK_RUN_TYPE`,
/// `STATUS`, `STARTED`), which the request layer validates a sort criteria against.
pub const ML_TASK_RUN_SORT_COLUMN_TASK_RUN_TYPE: &str = "TASK_RUN_TYPE";
/// See [`ML_TASK_RUN_SORT_COLUMN_TASK_RUN_TYPE`]: orders by the status each run stands in.
pub const ML_TASK_RUN_SORT_COLUMN_STATUS: &str = "STATUS";
/// See [`ML_TASK_RUN_SORT_COLUMN_TASK_RUN_TYPE`]: orders by the instant each run started.
pub const ML_TASK_RUN_SORT_COLUMN_STARTED: &str = "STARTED";

/// How many machine learning task run ids this process has minted. A nonce source for
/// [`new_ml_task_run_id`], exactly as [`MINTED_JOB_RUN_IDS`] is for a job run id.
pub(super) static MINTED_ML_TASK_RUN_IDS: AtomicU64 = AtomicU64::new(0);

/// How many 64-bit words a machine learning task run id carries: two, i.e. 128 bits
/// rendered as 32 lowercase hexadecimal characters.
pub(super) const ML_TASK_RUN_ID_WORDS: usize = 2;

/// A fresh `TaskRunId`: the id the service assigns a machine learning task run when a
/// start records it.
///
/// 32 lowercase hexadecimal characters, i.e. 128 bits, which is inside `HashString`'s
/// `length 1..255` — the same shape and width as the transform id this store mints
/// ([`new_ml_transform_id`]), because the service publishes no spelling for either. The
/// bits come from [`mint_hex_id`], so two starts landing in the same nanosecond cannot be
/// given the same id, and the id is stored on the run, so every later read of that run
/// reports the same one.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_ml_task_run_id() -> String {
    mint_hex_id(&MINTED_ML_TASK_RUN_IDS, ML_TASK_RUN_ID_WORDS)
}

/// One machine learning task run, as the call that started it recorded it.
///
/// A run carries the transform it was started of, the configuration properties of the task
/// that start asked for, and the members only a run has: the minted id, the status it
/// stands in and the instants it moved between statuses. The properties are the modelled
/// `TaskRunProperties` shape, which is where the TYPE of the task lives
/// ([`Self::task_type`]), so each start records the properties of its own task kind and a
/// listing reports them unchanged.
#[derive(Debug, Clone)]
pub struct MlTaskRunEntry {
    /// `TaskRun$TaskRunId`, minted by [`new_ml_task_run_id`].
    pub id: String,
    /// The `TransformId` of the transform this run was started of, which is the first half
    /// of a run's address.
    pub transform_id: String,
    /// "Specifies configuration properties associated with this task run"
    /// (`TaskRun$Properties`), as the start that recorded the run described the task: its
    /// `TaskType` and the properties of that one kind of task.
    pub properties: TaskRunProperties,
    /// One of the modelled `TaskStatusType` values; see [`ML_TASK_RUN_STATUS_STARTING`].
    pub status: String,
    /// The instant the clock owes this run its next status; `None` once it has settled.
    pub settles_at: Option<SystemTime>,
    /// "The date and time that this task run started" (`TaskRun$StartedOn`).
    pub started_on: SystemTime,
    /// "The last point in time that the requested task run was updated"
    /// (`TaskRun$LastModifiedOn`).
    pub last_modified_on: SystemTime,
    /// "The last point in time that the requested task run was completed"
    /// (`TaskRun$CompletedOn`), absent until the run has finished.
    pub completed_on: Option<SystemTime>,
    /// "The list of error strings associated with this task run"
    /// (`TaskRun$ErrorString`), absent until the run has failed.
    pub error_string: Option<String>,
}

impl MlTaskRunEntry {
    /// Record a run of `transform_id` performing the task `properties` describes, in
    /// [`ML_TASK_RUN_STATUS_STARTING`] with its next status due
    /// [`ML_TASK_RUN_SETTLES_AFTER`] out.
    ///
    /// Taken as the modelled properties rather than as one start's request, so every
    /// operation that starts a task run of a transform records one the same way and the
    /// listing cannot tell them apart by anything but the properties themselves.
    pub fn start(
        transform_id: &str,
        id: String,
        properties: TaskRunProperties,
        now: SystemTime,
    ) -> Self {
        Self {
            id,
            transform_id: transform_id.to_string(),
            properties,
            status: ML_TASK_RUN_STATUS_STARTING.to_string(),
            settles_at: Some(now.checked_add(ML_TASK_RUN_SETTLES_AFTER).unwrap_or(now)),
            started_on: now,
            last_modified_on: now,
            completed_on: None,
            // Nothing has failed yet: a run carries an error string only once it has.
            error_string: None,
        }
    }

    /// This run's `TaskType`, the one a filter and a sort select and order on: the type
    /// recorded in its own properties, so there is one place a run's kind is held.
    pub fn task_type(&self) -> &str {
        self.properties.task_type.as_deref().unwrap_or_default()
    }

    /// Move this run into the status the clock owed it at `now`, if it owed one.
    ///
    /// The run was due to leave [`ML_TASK_RUN_STATUS_STARTING`] at [`Self::settles_at`]
    /// whether or not anybody called, and the instants recorded are that DUE instant, not
    /// the instant somebody read it — so two reads of a settled run report the same run.
    pub fn settle_if_due(&mut self, now: SystemTime) {
        let Some(due) = self.settles_at else {
            return;
        };
        if due > now {
            return;
        }
        self.settles_at = None;
        self.status = ML_TASK_RUN_STATUS_FAILED.to_string();
        self.error_string = Some(ML_TASK_RUN_NOT_EXECUTED_MESSAGE.to_string());
        self.completed_on = Some(due);
        self.last_modified_on = due;
    }

    /// `TaskRun$ExecutionTime`: "The amount of time (in seconds) that the task run
    /// consumed resources" — the whole seconds between the run starting and finishing, and
    /// the model's own default of 0 while it has not finished.
    pub(super) fn execution_time_seconds(&self) -> i32 {
        let Some(completed_on) = self.completed_on else {
            return 0;
        };
        let elapsed = completed_on
            .duration_since(self.started_on)
            .unwrap_or_default();
        i32::try_from(elapsed.as_secs()).unwrap_or(i32::MAX)
    }

    /// Whether this run satisfies every criterion `filter` names.
    ///
    /// A `TaskRunFilterCriteria` is a conjunction: each member it carries narrows the
    /// listing, and a member it leaves out narrows nothing. The two enum members are
    /// matched exactly, and the two instants bound the window the criteria describe
    /// ("filter on task runs started before this date"), so a run started exactly at a
    /// bound is outside it — the same reading the transform filter's instant windows take
    /// ([`MlTransformEntry::matches`]).
    pub fn matches(&self, filter: &TaskRunFilterCriteria) -> bool {
        if filter
            .task_run_type
            .as_deref()
            .is_some_and(|kind| kind != self.task_type())
        {
            return false;
        }
        if filter
            .status
            .as_deref()
            .is_some_and(|status| status != self.status.as_str())
        {
            return false;
        }
        let started = secs_since_epoch(self.started_on);
        if filter
            .started_before
            .as_ref()
            .is_some_and(|bound| started >= bound.as_secs_f64())
        {
            return false;
        }
        if filter
            .started_after
            .as_ref()
            .is_some_and(|bound| started <= bound.as_secs_f64())
        {
            return false;
        }
        true
    }

    /// This run's place in a listing ordered by `column`, as the continuation token
    /// renders it: the sorted value, then the run's own id so that two runs sharing that
    /// value still have one total order to page over.
    ///
    /// `None` is the order a request that names no sort criteria gets, which is the same
    /// key [`ML_TASK_RUN_SORT_COLUMN_STARTED`] names: the operation documents no default
    /// order for "a list of historic runs" (`GetMLTaskRuns`), so the listing falls back on
    /// the order the runs were started in rather than on however the store happens to hold
    /// them. A column outside the modelled list cannot reach here — the request layer
    /// validates `Sort$Column` against it — and is ordered the same way.
    pub fn ordering_key(&self, column: Option<&str>) -> String {
        let sorted = match column {
            Some(ML_TASK_RUN_SORT_COLUMN_TASK_RUN_TYPE) => self.task_type().to_string(),
            Some(ML_TASK_RUN_SORT_COLUMN_STATUS) => self.status.clone(),
            Some(ML_TASK_RUN_SORT_COLUMN_STARTED) | Some(_) | None => {
                ordered_instant(self.started_on)
            }
        };
        format!("{sorted}{LISTING_KEY_SEPARATOR}{}", self.id)
    }

    /// Rebuild the modelled `TaskRun` output shape from what is stored, so a listing
    /// reports the same run a single-run read does.
    pub fn to_task_run(&self) -> TaskRun {
        TaskRun {
            transform_id: Some(self.transform_id.clone()),
            task_run_id: Some(self.id.clone()),
            status: Some(self.status.clone()),
            // "The names of the log group for secure logging, associated with this task
            // run": no task runs here, so nothing writes this run's output anywhere and
            // there is no log group holding it to name.
            log_group_name: None,
            properties: Some(self.properties.clone()),
            error_string: self.error_string.clone(),
            started_on: Some(to_date_time(self.started_on)),
            last_modified_on: Some(to_date_time(self.last_modified_on)),
            completed_on: self.completed_on.map(to_date_time),
            execution_time: self.execution_time_seconds(),
        }
    }
}
