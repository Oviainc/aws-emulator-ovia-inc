// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mwaaserverless-amazonmwaaserverless service.
//!
//! The store holds the workflow control plane — the workflows a caller created, the
//! immutable versions of each one, and the runs started under them — plus the
//! idempotency ledger that lets a repeated `ClientToken` replay its first answer.
//! Every handler reads and writes THIS struct through [`service_state`]; nothing in
//! `src/ops/` keeps a store of its own.
//!
//! The store is PARTITIONED BY REGION: a workflow belongs to the endpoint it was
//! created through ([`request_region`], recorded as [`Workflow::region`]), and every
//! lookup, listing, name-uniqueness check and idempotency token is scoped to the
//! region of the calling request, because a regional AWS endpoint serves only the
//! resources created through it.

//! Every collection here is BOUNDED, because all three grow from caller requests:
//! `workflows` is capped at [`MAX_WORKFLOWS`], a workflow's `runs` at
//! [`MAX_RUNS_PER_WORKFLOW`] and its `versions` at [`MAX_VERSIONS_PER_WORKFLOW`] (a
//! create, a run start or an update past the ceiling is refused with the modelled
//! `ServiceQuotaExceededException`, so the workflows x runs and workflows x versions
//! products are finite and so is the caller-supplied data each record holds); `workflow_names` holds exactly one
//! key per stored workflow, released when the delete's teardown removes the record
//! ([`ComAmazonawsMwaaserverlessAmazonmwaaserverlessState::reap_deleted_workflows`]);
//! and the
//! idempotency ledger is capped and evicted on its own terms (see [`IdempotencyLedger`]).
//! A listing's response is bounded too, by the modelled `MaxResults` range.
//! A run's `task_instances` ([`TaskInstance`]) is NOT caller-driven growth: no modelled
//! operation creates a task instance — only executing a workflow definition does, and
//! this emulator has no Apache Airflow engine ([`NO_AIRFLOW_ENGINE`]) — so the map is
//! bounded by the number of runs that hold it.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
use ::aws_smithy_schema_server::constraints::{
    validate, ElementConstraintsTrait, EnumTrait, LengthTrait, PatternTrait, Violation,
};
use sha2::{Digest, Sha256};

use crate::ops::types::{
    Code, DefinitionS3Location, EncryptionConfiguration, LoggingConfiguration,
    NetworkConfiguration, ResourceNotFoundException, ValidationException,
    ValidationExceptionField, ENCRYPTION_TYPE_VALUES,
};

/// The region used in minted ARNs when the request carries no usable SigV4
/// credential scope. Every generated emulator defaults to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace (and SigV4 signing name) this service uses, per the model's
/// `awsService.arnNamespace`.
pub const ARN_NAMESPACE: &str = "airflow-serverless";

/// How long after `StartWorkflowRun` the run leaves `STARTING` for `QUEUED`: the
/// span the control plane's own admission of the run occupies. Deliberately
/// non-zero so a caller that reads the run straight back still observes the
/// `STARTING` its start reported.
pub const RUN_ADMITTED_AFTER: Duration = Duration::from_millis(400);

/// How long after a run is `QUEUED` it reaches its terminal status. The span is
/// deliberately non-zero so a caller that polls immediately still observes
/// `QUEUED`.
pub const RUN_SETTLE_AFTER: Duration = Duration::from_millis(300);

/// Why a run that was accepted never executes here. This emulator serves the
/// workflow control plane faithfully but has no Apache Airflow engine to run a
/// DAG definition with, so the run ends in the modelled failure state rather
/// than reporting a success nothing computed.
pub const NO_AIRFLOW_ENGINE: &str =
    "The emulator accepted this run but has no Apache Airflow execution engine, \
     so the workflow definition was never executed.";

/// `WorkflowStatus` as the model spells it
/// (`smithy_query(query="shape", shape="WorkflowStatus")` declares `READY | DELETING`).
///
/// "workflow is ready to run": the status a stored workflow reports until its
/// deletion starts.
pub const WORKFLOW_STATUS_READY: &str = "READY";

/// "workflow is being deleted": the status a workflow reports while the teardown
/// `DeleteWorkflow` started is still running. The ONLY other `WorkflowStatus` the
/// model declares, so a delete that removed the record synchronously would make it
/// unreachable.
pub const WORKFLOW_STATUS_DELETING: &str = "DELETING";

/// How long after `DeleteWorkflow` the workflow's teardown finishes and the record
/// disappears.
///
/// `DeleteWorkflow` is documented as a cleanup over the workflow's associated
/// resources — "stopping any running executions, removing scheduled triggers, and
/// cleaning up execution history" — which is the span `DELETING` covers. The length
/// is the emulator's own choice, not a fidelity claim about how long AWS takes, and
/// it is deliberately non-zero so a caller that describes or lists straight after
/// the delete still observes `DELETING` (and the workflow's name still taken) before
/// the record goes.
pub const WORKFLOW_DELETED_AFTER: Duration = Duration::from_millis(500);

/// `WorkflowRunStatus` values this slice uses.
///
/// The status a just-started run carries: "This is typically STARTING when you
/// first create the run" (`smithy_query(query="operation",
/// operation="StartWorkflowRun")`, `Status`).
pub const RUN_STATUS_STARTING: &str = "STARTING";
/// The status an admitted run waits in for an executor.
pub const RUN_STATUS_QUEUED: &str = "QUEUED";
/// The modelled failure state a run that could not execute ends in.
pub const RUN_STATUS_FAILED: &str = "FAILED";
/// The status a run carries while the shutdown a stop started is still running:
/// "transitions the workflow run to a STOPPING state and then to STOPPED once all
/// cleanup is complete" (`smithy_query(query="operation",
/// operation="StopWorkflowRun")`).
pub const RUN_STATUS_STOPPING: &str = "STOPPING";
/// The status a run that was stopped reports once that shutdown is complete.
pub const RUN_STATUS_STOPPED: &str = "STOPPED";
/// The modelled success state. No run of this emulator reaches it — nothing executes
/// a workflow definition here ([`NO_AIRFLOW_ENGINE`]) — but it is one of the statuses
/// a run never leaves, so it belongs to [`RUN_TERMINAL_STATUSES`].
pub const RUN_STATUS_SUCCESS: &str = "SUCCESS";
/// The modelled timeout state, likewise terminal and likewise never assigned here.
pub const RUN_STATUS_TIMEOUT: &str = "TIMEOUT";

/// The `WorkflowRunStatus` values a run has FINISHED in and never leaves, out of the
/// eight the model declares (`smithy_query(query="shape",
/// shape="WorkflowRunStatus")`: the other four — `STARTING`, `QUEUED`, `RUNNING` and
/// `STOPPING` — are each on their way somewhere).
///
/// What is terminal is what decides whether there is still an execution for a stop to
/// end ([`WorkflowRun::begin_stop`]).
pub const RUN_TERMINAL_STATUSES: &[&str] = &[
    RUN_STATUS_SUCCESS,
    RUN_STATUS_FAILED,
    RUN_STATUS_TIMEOUT,
    RUN_STATUS_STOPPED,
];

/// How long after `StopWorkflowRun` the run leaves [`RUN_STATUS_STOPPING`] for
/// [`RUN_STATUS_STOPPED`]: the span the documented shutdown — stopping task
/// scheduling and terminating the active workers — occupies. The length is the
/// emulator's own choice, not a fidelity claim about how long AWS takes, and it is
/// deliberately non-zero so a caller that reads the run straight back still observes
/// the `STOPPING` its stop reported.
pub const RUN_STOPPED_AFTER: Duration = Duration::from_millis(400);

/// `RunType` for a run a caller started explicitly.
pub const RUN_TYPE_ON_DEMAND: &str = "ON_DEMAND";

/// How many workflows this emulator will hold for the account.
///
/// The real service caps workflows per account with a service quota, and
/// `CreateWorkflow` models `ServiceQuotaExceededException` (HTTP 402) for exceeding it
/// ("This can occur when you attempt to create more workflows than allowed"). The number
/// here is NOT a fidelity claim about the published quota: it is the emulator's own
/// ceiling, set generously so no realistic workflow meets it, so that a caller looping on
/// create cannot grow this process's memory without bound.
pub const MAX_WORKFLOWS: usize = 1_000;

/// How many runs this emulator will hold for ONE workflow. Same reasoning, and
/// `StartWorkflowRun` models the same `ServiceQuotaExceededException` (the docs name
/// "concurrent workflow run limits" and "task execution limits" as its causes). With
/// [`MAX_WORKFLOWS`] this bounds the whole run store, including the caller-supplied
/// `OverrideParameters` each run holds.
pub const MAX_RUNS_PER_WORKFLOW: usize = 1_000;

/// How many immutable versions this emulator will hold for ONE workflow.
///
/// `UpdateWorkflow` appends a version per call ("automatically creates a new version with
/// the updated configuration"), so the version list is caller-driven growth holding a
/// whole configuration snapshot each; past this ceiling an update is refused with the
/// `ServiceQuotaExceededException` the operation already models. Same reasoning as
/// [`MAX_WORKFLOWS`]: the number is the emulator's own ceiling, not a claim about the
/// published quota.
pub const MAX_VERSIONS_PER_WORKFLOW: usize = 1_000;

/// The most `@idempotencyToken` entries this service remembers.
///
/// Tokens are caller-supplied, so the ledger is caller-driven growth holding no resource
/// of its own: an entry naming a deleted workflow is dropped with it
/// ([`IdempotencyLedger::forget_workflow`]), and beyond this many live entries the oldest
/// is evicted ([`IdempotencyLedger::record`]). An evicted token is indistinguishable from
/// one whose idempotency window has passed: the next call carrying it creates a new
/// resource rather than replaying.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 1_024;

/// The definition and configuration a workflow version froze at the moment it was
/// created. A version is immutable, so a read of that version answers from here
/// rather than from the workflow's current configuration.
#[derive(Debug, Clone)]
pub struct WorkflowSnapshot {
    pub definition_s3_location: DefinitionS3Location,
    pub code: Option<Code>,
    pub role_arn: String,
    pub description: Option<String>,
    pub encryption_configuration: Option<EncryptionConfiguration>,
    pub logging_configuration: Option<LoggingConfiguration>,
    pub engine_version: Option<i32>,
    pub network_configuration: Option<NetworkConfiguration>,
    pub trigger_mode: Option<String>,
}

/// One immutable version of a workflow.
#[derive(Debug, Clone)]
pub struct WorkflowVersionRecord {
    pub version: String,
    pub created_at: SystemTime,
    pub is_latest: bool,
    pub snapshot: WorkflowSnapshot,
    /// Where this version falls in the order its workflow's versions were
    /// appended: the value of the monotonic mint counter
    /// ([`ComAmazonawsMwaaserverlessAmazonmwaaserverlessState::mint_seq`]) at the
    /// moment the version was recorded. It only ever grows, and it is not reused
    /// when a version is removed, so it is the POSITION a version listing pages
    /// over ([`format_version_listing_key`]): a cursor carrying it still resolves
    /// to the same place in the remaining history after any other version has
    /// gone.
    pub sequence: u64,
}

/// A run started under a workflow.
#[derive(Debug, Clone)]
pub struct WorkflowRun {
    pub run_id: String,
    pub workflow_arn: String,
    pub workflow_version: String,
    pub run_type: String,
    pub status: String,
    pub override_parameters: BTreeMap<String, ::aws_smithy_types::Document>,
    pub created_at: SystemTime,
    pub started_on: SystemTime,
    pub completed_on: Option<SystemTime>,
    pub modified_at: SystemTime,
    pub duration: Option<i32>,
    pub error_message: Option<String>,
    /// The task instances this run produced, keyed by `TaskInstanceId` — a
    /// `BTreeMap` so a reader walks them in one total order. Empty for every run
    /// this emulator starts: a task instance exists only once something executed
    /// the workflow definition, and nothing here does (see [`NO_AIRFLOW_ENGINE`]),
    /// so `GetTaskInstance` answers the modelled `ResourceNotFoundException`
    /// rather than reporting a task nothing ran.
    pub task_instances: BTreeMap<String, TaskInstance>,
    /// The instant the clock owes this run's NEXT status change. Stored, never
    /// slept on, and advanced by [`WorkflowRun::settle`] as each change lands.
    pub next_status_at: SystemTime,
    /// Where this run falls in the store's creation order: the value of the
    /// monotonic mint counter ([`ComAmazonawsMwaaserverlessAmazonmwaaserverlessState::mint_seq`])
    /// at the moment the run was recorded. It breaks a tie between two runs the
    /// clock cannot separate, so [`WorkflowRun::listing_key`] is a TOTAL order
    /// that still only ever grows as runs are created.
    pub sequence: u64,
}

/// The position of a run in the order a run listing walks: its creation instant
/// (nanoseconds since the epoch) and, for two runs created at the same instant,
/// the creation sequence that separates them.
///
/// Ordering runs by this key — rather than by the identifier they are stored
/// under — is what makes a listing chronological, and what keeps a run created
/// after a page was served sorting AFTER that page's cursor instead of before it.
pub type RunListingKey = (u128, u64);

impl WorkflowRun {
    /// This run's position in the listing order (see [`RunListingKey`]).
    pub fn listing_key(&self) -> RunListingKey {
        (
            self.created_at
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            self.sequence,
        )
    }

    /// The `TaskInstanceId`s this run holds, in the store's order — what
    /// `GetWorkflowRun` reports as `RunDetail.TaskInstances`.
    pub fn task_instance_ids(&self) -> Vec<String> {
        self.task_instances.keys().cloned().collect()
    }

    /// Apply EVERY status change the clock has already made due, and report
    /// whether anything changed.
    ///
    /// This is the ONLY write a reporting operation may make: at `now` each
    /// transition below was owed whether or not anybody called, so the status a
    /// reader reports is the one the run would have reached unpolled. Every
    /// timestamp it writes comes from the DUE instant, never from `now`, so two
    /// reads after the same deadline answer identically.
    ///
    /// The schedule is the lifecycle the control plane itself drives:
    ///
    /// * `STARTING` -> `QUEUED` after [`RUN_ADMITTED_AFTER`]: the run has been
    ///   admitted and is waiting for an executor. Both are modelled
    ///   `WorkflowRunStatus` values and both are the control plane's own work,
    ///   which this emulator really does.
    /// * `QUEUED` -> `FAILED` after [`RUN_SETTLE_AFTER`], with
    ///   [`NO_AIRFLOW_ENGINE`] as `ErrorMessage`: nothing dequeues the run,
    ///   because executing an Apache Airflow workflow definition needs an
    ///   Airflow engine this emulator does not have. A run that nothing executed
    ///   never reaches `RUNNING` or `SUCCESS` here — those would be a status for
    ///   work nothing ran — so it ends in the service's modelled failure state
    ///   with a reason that says exactly that, and holds no task instance.
    /// * `STOPPING` -> `STOPPED` after [`RUN_STOPPED_AFTER`]: the shutdown a stop
    ///   started is complete, so the run is finished and carries the completion
    ///   instant and duration every finished run does. It records no `ErrorMessage`:
    ///   a run the caller stopped did not fail.
    ///
    /// A terminal status is never left: the loop stops at any status no
    /// transition is owed from, which is what makes a settled run stable across
    /// reads.
    pub fn settle(&mut self, now: SystemTime) -> bool {
        let mut changed = false;
        while now >= self.next_status_at {
            let due = self.next_status_at;
            match self.status.as_str() {
                RUN_STATUS_STARTING => {
                    self.status = RUN_STATUS_QUEUED.to_string();
                    self.modified_at = due;
                    self.next_status_at = due + RUN_SETTLE_AFTER;
                }
                RUN_STATUS_QUEUED => {
                    let elapsed = due.duration_since(self.started_on).unwrap_or_default();
                    self.status = RUN_STATUS_FAILED.to_string();
                    self.completed_on = Some(due);
                    self.modified_at = due;
                    self.duration = Some(duration_in_seconds(elapsed));
                    self.error_message = Some(NO_AIRFLOW_ENGINE.to_string());
                }
                RUN_STATUS_STOPPING => {
                    let elapsed = due.duration_since(self.started_on).unwrap_or_default();
                    self.status = RUN_STATUS_STOPPED.to_string();
                    self.completed_on = Some(due);
                    self.modified_at = due;
                    self.duration = Some(duration_in_seconds(elapsed));
                }
                // Terminal, or a status whose exit no clock owes.
                _ => break,
            }
            changed = true;
        }
        changed
    }

    /// Whether this run has finished: its status is one a run never leaves
    /// ([`RUN_TERMINAL_STATUSES`]), so no execution of it remains to be stopped.
    pub fn is_terminal(&self) -> bool {
        RUN_TERMINAL_STATUSES.contains(&self.status.as_str())
    }

    /// Start the shutdown `StopWorkflowRun` describes: a run that has not finished
    /// moves to [`RUN_STATUS_STOPPING`], and its arrival in [`RUN_STATUS_STOPPED`]
    /// falls due [`RUN_STOPPED_AFTER`] later — stored, never slept on, and
    /// materialised by [`WorkflowRun::settle`] like every other owed change. Reports
    /// whether this call moved the run.
    ///
    /// Idempotent, because the operation is `@idempotent` and models no conflict for a
    /// run there is nothing to stop: a run already stopping keeps the deadline it has,
    /// and a run that already finished keeps the status, completion and reason it
    /// finished with, so a repeated stop REPORTS the run rather than restarting it.
    pub fn begin_stop(&mut self, now: SystemTime) -> bool {
        if self.is_terminal() || self.status == RUN_STATUS_STOPPING {
            return false;
        }
        self.status = RUN_STATUS_STOPPING.to_string();
        self.modified_at = now;
        self.next_status_at = now + RUN_STOPPED_AFTER;
        true
    }
}

/// One task instance of a run: the execution record of a single task in the
/// workflow definition.
///
/// The fields are exactly what the modelled task-instance shapes report
/// (`smithy_query(query="shape", shape="GetTaskInstanceResponse,TaskInstanceSummary")`),
/// minus the three a reader takes from the run that owns it (`WorkflowArn`, `RunId`,
/// `WorkflowVersion`) and the `DurationInSeconds` a reader DERIVES from
/// `started_at`/`ended_at` ([`TaskInstance::duration_in_seconds`]).
///
/// Written only by whatever executes a workflow definition; this emulator has no
/// Apache Airflow engine, so nothing writes one and no stored run holds one.
#[derive(Debug, Clone)]
pub struct TaskInstance {
    /// `TaskInstanceId`: this instance's own identifier, the key it is stored under.
    pub task_instance_id: String,
    /// `TaskId`: the task in the workflow definition this instance executes.
    pub task_id: Option<String>,
    /// `Status`: a `TaskInstanceStatus` value (see [`TASK_INSTANCE_STATUS_NONE`]).
    pub status: String,
    /// `OperatorName`: the Apache Airflow operator the task uses.
    pub operator_name: Option<String>,
    /// `AttemptNumber`: which attempt this instance is.
    pub attempt_number: Option<i32>,
    /// `StartedAt`: `None` until the task starts, per the member's docs.
    pub started_at: Option<SystemTime>,
    /// `EndedAt`: `None` until the task completes, per the member's docs.
    pub ended_at: Option<SystemTime>,
    /// `ModifiedAt`: when this record last changed.
    pub modified_at: SystemTime,
    /// `ErrorMessage`: `None` when the task did not fail, per the member's docs.
    pub error_message: Option<String>,
    /// `LogStream`: the CloudWatch log stream this instance logs to.
    pub log_stream: Option<String>,
    /// `Xcom`: the cross-task data this instance exchanged. A `BTreeMap` so the
    /// entries have one order in the store.
    pub xcom: BTreeMap<String, String>,
}

impl TaskInstance {
    /// `DurationInSeconds`, derived rather than stored: the wall time between
    /// `started_at` and `ended_at`, and `None` while the task is not complete —
    /// "This value is null if the task is not complete"
    /// (`smithy_query(query="shape", shape="GetTaskInstanceResponse")`,
    /// `DurationInSeconds`).
    pub fn duration_in_seconds(&self) -> Option<i32> {
        let (started, ended) = (self.started_at?, self.ended_at?);
        Some(duration_in_seconds(
            ended.duration_since(started).unwrap_or_default(),
        ))
    }
}

/// The `TaskInstanceStatus` value for an instance that has no status yet, as the
/// model spells it (`smithy_query(query="shape", shape="TaskInstanceStatus")`).
///
/// Nothing in `src/ops/` writes a task instance (no Apache Airflow engine executes a
/// definition here), so this is the value a seeded record starts from in
/// `tests/all/task_instance_reads.rs` rather than one a handler assigns.
pub const TASK_INSTANCE_STATUS_NONE: &str = "NONE";

/// A run duration in whole seconds, rounded up so a sub-second run still reports
/// the second it occupied, and clamped into the modelled `Integer`.
fn duration_in_seconds(elapsed: Duration) -> i32 {
    let secs = elapsed.as_millis().div_ceil(1000);
    i32::try_from(secs).unwrap_or(i32::MAX)
}

/// A persisted workflow.
#[derive(Debug, Clone)]
pub struct Workflow {
    pub arn: String,
    /// The region whose endpoint this workflow was created through: the credential
    /// scope of the `CreateWorkflow` request ([`request_region`]), which is also the
    /// region segment of [`Workflow::arn`].
    ///
    /// A regional endpoint serves only the resources created through it, so this is
    /// the partition key EVERY lookup, listing and name-uniqueness check of this
    /// store is scoped by ([`ComAmazonawsMwaaserverlessAmazonmwaaserverlessState::workflow_in`]).
    pub region: String,
    pub name: String,
    pub status: String,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
    pub code_snapshotted_at: SystemTime,
    pub current_version: String,
    pub revision_id: String,
    pub versions: Vec<WorkflowVersionRecord>,
    pub tags: BTreeMap<String, String>,
    pub runs: BTreeMap<String, WorkflowRun>,
    /// The instant the clock owes this workflow's removal, once its teardown has
    /// been started by `DeleteWorkflow`, and `None` for a workflow no delete has
    /// touched. Stored, never slept on, and materialised by
    /// [`ComAmazonawsMwaaserverlessAmazonmwaaserverlessState::reap_deleted_workflows`].
    pub deletion_due_at: Option<SystemTime>,
}

impl Workflow {
    /// The version record for `version`, or the latest one when `version` is
    /// `None`.
    pub fn version(&self, version: Option<&str>) -> Option<&WorkflowVersionRecord> {
        match version {
            Some(v) => self.versions.iter().find(|rec| rec.version == v),
            None => self.versions.iter().find(|rec| rec.is_latest),
        }
    }

    /// The configuration this workflow currently reports.
    pub fn current_snapshot(&self) -> Option<&WorkflowSnapshot> {
        self.version(None).map(|rec| &rec.snapshot)
    }

    /// Whether this workflow's teardown has already been started, so it is on its
    /// way out and no longer accepts a change.
    pub fn is_deleting(&self) -> bool {
        self.deletion_due_at.is_some()
    }

    /// Start the teardown `DeleteWorkflow` describes: the workflow moves to
    /// [`WORKFLOW_STATUS_DELETING`] and its removal falls due
    /// [`WORKFLOW_DELETED_AFTER`] later. Nothing else is dropped here — the
    /// versions, runs, name and tokens go with the record when the removal lands,
    /// so a read during the teardown still reports the workflow.
    ///
    /// Idempotent: a second delete of a workflow already tearing down neither
    /// restarts the teardown nor moves its deadline.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        if self.is_deleting() {
            return;
        }
        self.status = WORKFLOW_STATUS_DELETING.to_string();
        self.modified_at = now;
        self.deletion_due_at = Some(now + WORKFLOW_DELETED_AFTER);
    }
}

/// What a `ClientToken` already produced, so repeating it replays rather than
/// creating a second resource.
#[derive(Debug, Clone)]
pub enum IdempotentResult {
    CreateWorkflow {
        request_fingerprint: String,
        workflow_arn: String,
    },
    StartWorkflowRun {
        request_fingerprint: String,
        workflow_arn: String,
        run_id: String,
    },
}

impl IdempotentResult {
    /// The fingerprint of the request that first used the token.
    pub fn request_fingerprint(&self) -> &str {
        match self {
            Self::CreateWorkflow {
                request_fingerprint,
                ..
            }
            | Self::StartWorkflowRun {
                request_fingerprint,
                ..
            } => request_fingerprint,
        }
    }

    /// The workflow the token's first use touched.
    pub fn workflow_arn(&self) -> &str {
        match self {
            Self::CreateWorkflow { workflow_arn, .. }
            | Self::StartWorkflowRun { workflow_arn, .. } => workflow_arn,
        }
    }
}

/// A bounded `<operation>:<ClientToken>` -> first-result ledger.
///
/// Insertion-ordered and capped at [`MAX_IDEMPOTENCY_TOKENS`]: recording the (cap + 1)-th
/// token forgets the oldest one rather than growing the map.
#[derive(Debug, Default)]
pub struct IdempotencyLedger {
    by_key: BTreeMap<String, IdempotentResult>,
    oldest_first: VecDeque<String>,
}

impl IdempotencyLedger {
    /// What a previous call recorded under `key`, if it is still remembered.
    pub fn get(&self, key: &str) -> Option<&IdempotentResult> {
        self.by_key.get(key)
    }

    /// Remember `result` under `key`, evicting the oldest entries to stay within
    /// [`MAX_IDEMPOTENCY_TOKENS`]. Re-recording a key moves it to the back, so the
    /// eviction order stays insertion-ordered by the LAST time a key was written.
    pub fn record(&mut self, key: String, result: IdempotentResult) {
        if self.by_key.insert(key.clone(), result).is_some() {
            self.oldest_first.retain(|recorded| recorded != &key);
        }
        self.oldest_first.push_back(key);
        while self.oldest_first.len() > MAX_IDEMPOTENCY_TOKENS {
            if let Some(evicted) = self.oldest_first.pop_front() {
                self.by_key.remove(&evicted);
            }
        }
    }

    /// How many tokens are remembered right now.
    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    /// Whether no token is remembered.
    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// Forget every token that names `workflow_arn` — called when that workflow is
    /// deleted, so a token record never outlives the resource it created.
    pub fn forget_workflow(&mut self, workflow_arn: &str) {
        self.by_key
            .retain(|_, result| result.workflow_arn() != workflow_arn);
        let live = &self.by_key;
        self.oldest_first.retain(|key| live.contains_key(key));
    }
}

/// Per-service state for com-amazonaws-mwaaserverless-amazonmwaaserverless. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMwaaserverlessAmazonmwaaserverlessState {
    /// Every persisted workflow, keyed by its ARN. A `BTreeMap` so `ListWorkflows`
    /// has one total order to paginate over. At most [`MAX_WORKFLOWS`] entries, and at
    /// most [`MAX_RUNS_PER_WORKFLOW`] runs inside each.
    ///
    /// A `WorkflowArn` embeds the region it was minted for, so one map holds every
    /// region's workflows without collision; a handler reaches only the ones the
    /// caller's endpoint serves, through [`Self::workflow_in`],
    /// [`Self::workflow_in_mut`] and [`Self::workflows_in_after`].
    pub workflows: BTreeMap<String, Workflow>,
    /// `(region, Name)` -> `WorkflowArn`, the uniqueness index `CreateWorkflow`
    /// conflicts on. The region is part of the key because a name is taken only at the
    /// endpoint the workflow was created through, so the same name is free in every
    /// other region. One key per stored workflow — including one that is `DELETING`,
    /// whose name stays taken until its teardown completes — so [`MAX_WORKFLOWS`]
    /// bounds this map too.
    pub workflow_names: BTreeMap<(String, String), String>,
    /// `<operation>:<ClientToken>` -> what that token already produced. Capped and
    /// evicted on its own terms; see [`IdempotencyLedger`].
    pub idempotency: IdempotencyLedger,
    /// Monotonic counter mixed into every minted identifier.
    pub mint_seq: u64,
}

impl ComAmazonawsMwaaserverlessAmazonmwaaserverlessState {
    /// A fresh nonce for an identifier mint: a monotonic counter plus the wall
    /// clock, so two mints never collide even within one nanosecond.
    pub fn next_nonce(&mut self, now: SystemTime) -> String {
        self.mint_seq += 1;
        let nanos = now
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("{}:{nanos}", self.mint_seq)
    }

    /// Forget every idempotency token whose resource is `workflow_arn`, so a
    /// replayed token can never resolve to a workflow that no longer exists.
    pub fn forget_tokens_for(&mut self, workflow_arn: &str) {
        self.idempotency.forget_workflow(workflow_arn);
    }

    /// Finish every workflow teardown the clock has already made due, and report
    /// whether anything was removed.
    ///
    /// This is the one write a reporting operation may make: the removal of a
    /// workflow whose [`Workflow::deletion_due_at`] has passed was owed at that
    /// instant whether or not anybody called, so what a reader reports is what the
    /// store would hold unpolled. Removing the record releases the name
    /// `CreateWorkflow` conflicts on and forgets the tokens that replayed onto it,
    /// and takes the workflow's versions and runs with it.
    ///
    /// Every handler that touches [`Self::workflows`] calls this first, so a
    /// workflow never outlives its teardown just because the next request happened
    /// to be a different operation.
    pub fn reap_deleted_workflows(&mut self, now: SystemTime) -> bool {
        let due: Vec<(String, String, String)> = self
            .workflows
            .values()
            .filter(|workflow| workflow.deletion_due_at.is_some_and(|at| now >= at))
            .map(|workflow| {
                (
                    workflow.arn.clone(),
                    workflow.region.clone(),
                    workflow.name.clone(),
                )
            })
            .collect();
        for (arn, region, name) in &due {
            self.workflows.remove(arn);
            // Only if the name still points at THIS workflow: the index is keyed by
            // region and name, and a workflow removed earlier may have freed it already.
            let name_key = (region.clone(), name.clone());
            if self.workflow_names.get(&name_key).is_some_and(|held| held == arn) {
                self.workflow_names.remove(&name_key);
            }
            self.forget_tokens_for(arn);
        }
        !due.is_empty()
    }

    /// The workflow `arn` names AS SEEN FROM `region`'s endpoint: the stored record
    /// when it lives in that region, and `None` otherwise.
    ///
    /// An AWS regional endpoint resolves only the resources created through it, so a
    /// reference to a workflow of another region is unresolvable here — the same miss
    /// as an ARN the store never held.
    pub fn workflow_in(&self, region: &str, arn: &str) -> Option<&Workflow> {
        self.workflows
            .get(arn)
            .filter(|workflow| workflow.region == region)
    }

    /// [`Self::workflow_in`], for a caller that has to write the record it resolves.
    pub fn workflow_in_mut(&mut self, region: &str, arn: &str) -> Option<&mut Workflow> {
        self.workflows
            .get_mut(arn)
            .filter(|workflow| workflow.region == region)
    }

    /// The workflows `region`'s endpoint holds, in ARN order, starting after the ARN
    /// `after` names — the walk a region-scoped listing pages over.
    pub fn workflows_in_after<'a>(
        &'a self,
        region: &'a str,
        after: Option<&'a str>,
    ) -> impl Iterator<Item = &'a Workflow> + 'a {
        let lower = match after {
            Some(last) => std::ops::Bound::Excluded(last),
            None => std::ops::Bound::Unbounded,
        };
        self.workflows
            .range::<str, _>((lower, std::ops::Bound::Unbounded))
            .map(|(_, workflow)| workflow)
            .filter(move |workflow| workflow.region == region)
    }

    /// The ARN of the workflow holding `name` at `region`'s endpoint, if one does.
    /// A name taken in one region says nothing about any other.
    pub fn workflow_arn_named(&self, region: &str, name: &str) -> Option<&String> {
        self.workflow_names
            .get(&(region.to_string(), name.to_string()))
    }

    /// Record that `arn` holds `name` at `region`'s endpoint.
    pub fn claim_workflow_name(&mut self, region: &str, name: &str, arn: &str) {
        self.workflow_names
            .insert((region.to_string(), name.to_string()), arn.to_string());
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>)
}

/// The region this request is scoped to: the SigV4 credential scope's region when it
/// is one the modelled `WorkflowArn` pattern accepts, else [`DEFAULT_REGION`].
///
/// It is both the region segment of an ARN minted for the request and the partition
/// every lookup, listing and name-uniqueness check is scoped by, because a regional
/// endpoint serves only the resources created through it.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_arn_region(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// True when `region` matches `([a-z]{2}-[a-z]+-[0-9]{1})`, the region segment of
/// the modelled `WorkflowArn` pattern (`smithy_query shape=WorkflowArn`).
fn is_arn_region(region: &str) -> bool {
    let mut parts = region.split('-');
    let (Some(area), Some(direction), Some(index), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    area.len() == 2
        && area.bytes().all(|b| b.is_ascii_lowercase())
        && !direction.is_empty()
        && direction.bytes().all(|b| b.is_ascii_lowercase())
        && index.len() == 1
        && index.bytes().all(|b| b.is_ascii_digit())
}

/// A lowercase-hex SHA-256 digest over `fields`, each field length-delimited so
/// `["a", "bc"]` and `["ab", "c"]` never collide.
pub fn digest(fields: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for field in fields {
        hasher.update(field.len().to_le_bytes());
        hasher.update(field.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// Mint a `WorkflowArn`: `arn:aws:airflow-serverless:<region>:<account>:workflow/<name>-<10 alnum>`,
/// the shape the modelled `WorkflowArn` pattern requires. The 10-character suffix
/// is the service-generated unique part the docs describe ("The service generates
/// a unique identifier that is appended").
pub fn mint_workflow_arn(region: &str, name: &str, nonce: &str) -> String {
    let suffix: String = digest(&["workflow-arn", name, nonce])
        .chars()
        .take(10)
        .collect();
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("workflow/{name}-{suffix}"))
}

/// Mint a `WorkflowVersion`: 32 lowercase hex characters, per the modelled
/// `^[a-z0-9]{32}$`. Derived from the workflow's identity and the configuration
/// the version freezes, so two different configurations never share a version id.
pub fn mint_workflow_version(workflow_arn: &str, fingerprint: &str, nonce: &str) -> String {
    digest(&["workflow-version", workflow_arn, fingerprint, nonce])
        .chars()
        .take(32)
        .collect()
}

/// Mint a `RevisionId` for a workflow configuration revision. Changes whenever
/// the version does, which is what makes it usable for optimistic concurrency.
pub fn mint_revision_id(workflow_arn: &str, version: &str) -> String {
    digest(&["workflow-revision", workflow_arn, version])
        .chars()
        .take(32)
        .collect()
}

/// Mint a `RunId` matching the modelled `^[a-zA-Z0-9]+[a-zA-Z0-9\.\-_]*$`.
pub fn mint_run_id(workflow_arn: &str, nonce: &str) -> String {
    let suffix: String = digest(&["workflow-run", workflow_arn, nonce])
        .chars()
        .take(16)
        .collect();
    format!("run-{suffix}")
}

/// The marker every pagination cursor this service issues carries, so a token it
/// did not mint (or one minted for a different list) is refused rather than
/// silently restarting the walk somewhere.
const PAGE_TOKEN_PREFIX: &str = "mwaaserverless.page.v1:";

/// Encode a pagination cursor so a client cannot read or construct one by hand:
/// the kind of list plus the key of the last item on the page, hex-encoded.
pub fn encode_page_token(kind: &str, last_key: &str) -> String {
    hex::encode(format!("{PAGE_TOKEN_PREFIX}{kind}\u{1f}{last_key}"))
}

/// Decode a pagination cursor [`encode_page_token`] issued for `kind`. `None`
/// when the token was not issued by this service for this list, which every
/// caller refuses as a `ValidationException`.
pub fn decode_page_token(kind: &str, token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    let decoded = String::from_utf8(bytes).ok()?;
    let rest = decoded.strip_prefix(PAGE_TOKEN_PREFIX)?;
    let (token_kind, key) = rest.split_once('\u{1f}')?;
    (token_kind == kind).then(|| key.to_string())
}

/// What separates a child-listing cursor key's POSITION from the workflow the
/// walk it was issued for is scoped to. Neither a zero-padded decimal position
/// nor a `WorkflowArn` (`^arn:…:workflow/[a-zA-Z0-9.\-_]+$`) can contain it, so
/// the split is unambiguous.
const LISTING_KEY_SCOPE_SEPARATOR: char = '@';

/// The cursor key for a position in ONE workflow's child listing: the position of
/// the last item on the page, and the workflow whose history the walk is over.
///
/// A child listing pages the history of exactly one parent, so the parent belongs
/// in the key: a cursor presented on a walk over ANOTHER workflow's history is
/// then recognisable as one not issued for that walk, instead of being applied as
/// a position in a history it says nothing about.
pub fn format_scoped_listing_key(workflow_arn: &str, position: &str) -> String {
    format!("{position}{LISTING_KEY_SCOPE_SEPARATOR}{workflow_arn}")
}

/// The POSITION a cursor of `kind` carries, for a walk scoped to `workflow_arn`.
///
/// `None` when the token is not one this service issued, when it was issued for
/// another of this service's lists, when its key is malformed, or when it was
/// issued on another workflow's history — each of which a listing refuses with
/// the same `ValidationException` ([`invalid_page_token`]), because none of them
/// is a position in the walk being asked for.
pub fn parse_scoped_page_token(kind: &str, token: &str, workflow_arn: &str) -> Option<String> {
    let key = decode_page_token(kind, token)?;
    let (position, scope) = key.split_once(LISTING_KEY_SCOPE_SEPARATOR)?;
    (scope == workflow_arn).then(|| position.to_string())
}

/// The cursor key for a position in ONE workflow's run listing: the two
/// components of a [`RunListingKey`], zero-padded so the key a cursor carries is
/// fixed-width and orders the same way the tuple does, scoped to the workflow the
/// walk is over.
pub fn format_run_listing_key(workflow_arn: &str, key: RunListingKey) -> String {
    let (nanos, sequence) = key;
    format_scoped_listing_key(workflow_arn, &format!("{nanos:039}.{sequence:020}"))
}

/// Read back the position part of a key [`format_run_listing_key`] wrote, as
/// [`parse_scoped_page_token`] hands it over. `None` for anything else, which a
/// listing refuses as a `ValidationException` just like a token it did not issue.
pub fn parse_run_listing_key(position: &str) -> Option<RunListingKey> {
    let (nanos, sequence) = position.split_once('.')?;
    Some((nanos.parse().ok()?, sequence.parse().ok()?))
}

/// The cursor key for a position in ONE workflow's version listing: the
/// [`WorkflowVersionRecord::sequence`] of the last version on the page,
/// zero-padded so the key is fixed-width, scoped to the workflow the walk is over.
///
/// The POSITION rather than the version's identifier, so a cursor still resolves
/// to the same place once the version it was issued at has been deleted.
pub fn format_version_listing_key(workflow_arn: &str, sequence: u64) -> String {
    format_scoped_listing_key(workflow_arn, &format!("{sequence:020}"))
}

/// Read back the position part of a key [`format_version_listing_key`] wrote, as
/// [`parse_scoped_page_token`] hands it over. `None` for anything else, which a
/// listing refuses as a `ValidationException` just like a token it did not issue.
///
/// A version listing's position is a single number, so this is only the pair of
/// [`format_version_listing_key`] — named so the version listing reads its cursor
/// through the same two steps the run listing does, not because the position has a
/// format of its own.
pub fn parse_version_listing_key(position: &str) -> Option<u64> {
    position.parse().ok()
}

/// What separates the workflow from the run in the scope a task-instance
/// listing's cursor is bound to. A `WorkflowArn` already contains `/`, but the
/// scope is only ever compared whole, so any separator a `RunId`
/// (`^[a-zA-Z0-9]+[a-zA-Z0-9\.\-_]*$`) cannot contain keeps two runs' scopes
/// distinct; this one spells the scope the way the operation's own URI does
/// (`/workflows/{WorkflowArn}/runs/{RunId}/tasks`).
const RUN_SCOPE_INFIX: &str = "/runs/";

/// The walk a task-instance cursor belongs to: the RUN whose task instances it
/// pages, inside the workflow that holds that run.
///
/// A task-instance listing pages the instances of exactly one run, so the run —
/// not just its workflow — is what a cursor is a position in: a cursor minted
/// while walking ANOTHER run's instances is then recognisable as one not issued
/// for this walk.
fn task_instance_listing_scope(workflow_arn: &str, run_id: &str) -> String {
    format!("{workflow_arn}{RUN_SCOPE_INFIX}{run_id}")
}

/// The cursor key for a position in ONE run's task-instance listing: the
/// identifier of the last instance on the page, scoped to the run the walk is
/// over.
///
/// A run holds its instances keyed by `TaskInstanceId`
/// ([`WorkflowRun::task_instances`]) and a listing walks them in that key's
/// order, so the identifier IS the position: resuming after it reports exactly
/// the instances that order places later, and an instance removed between pages
/// leaves the cursor resolving to the same place in what remains.
pub fn format_task_instance_listing_key(
    workflow_arn: &str,
    run_id: &str,
    last_task_instance_id: &str,
) -> String {
    format_scoped_listing_key(
        &task_instance_listing_scope(workflow_arn, run_id),
        last_task_instance_id,
    )
}

/// The POSITION a cursor of `kind` carries for the walk over ONE run's task
/// instances, as [`format_task_instance_listing_key`] wrote it.
///
/// `None` when the token is not one this service issued, when it was issued for
/// another of its lists, when it was issued on another run's instances, or when
/// it carries no position at all — each of which the listing refuses with
/// [`invalid_page_token`], because none of them is a position in the walk being
/// asked for.
pub fn parse_task_instance_page_token(
    kind: &str,
    token: &str,
    workflow_arn: &str,
    run_id: &str,
) -> Option<String> {
    parse_scoped_page_token(
        kind,
        token,
        &task_instance_listing_scope(workflow_arn, run_id),
    )
    .filter(|position| !position.is_empty())
}

/// The HTTP status this service answers a resource-lookup miss with.
///
/// The model gives `ResourceNotFoundException` `@httpError 404`
/// (`smithy_query shape=ResourceNotFoundException`), but the recorded service answers
/// a lookup for a workflow/version/run it does not hold with **HTTP 400** carrying
/// that same shape (fixture `mwaa-serverless-resource-not-found-errors`: GetWorkflow,
/// DeleteWorkflow, StartWorkflowRun and GetWorkflowRun each answer
/// `400 ResourceNotFoundException`). The recording is what a client sees, so the
/// shape keeps its modelled identity and only the status is overridden, through
/// [`crate::scaffolded::Scaffolded::ModeledWithStatus`].
pub const LOOKUP_MISS_STATUS: u16 = 400;

/// The entity an operation could not resolve.
///
/// The refusal names the entity THAT OPERATION had to resolve to do its work, which
/// is why `StartWorkflowRun` reports the missing workflow VERSION and `GetWorkflowRun`
/// the missing RUN for the very same absent workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnresolvedEntity {
    /// The workflow a `WorkflowArn` names.
    Workflow,
    /// The workflow's latest version, selected when a request names no
    /// `WorkflowVersion`.
    LatestWorkflowVersion,
    /// The version a `WorkflowVersion` names.
    NamedWorkflowVersion,
    /// The run a `RunId` names.
    WorkflowRun,
    /// The task instance a `TaskInstanceId` names, inside the run that holds it.
    TaskInstance,
}

impl UnresolvedEntity {
    /// The version entity a request selects: the one it named, or the latest.
    pub fn version_for(requested: Option<&str>) -> Self {
        if requested.is_some() {
            Self::NamedWorkflowVersion
        } else {
            Self::LatestWorkflowVersion
        }
    }

    /// What the refusal says. Entity-level and free of request values, so a client
    /// can tell "workflow absent" from "no version to run" from "run absent".
    pub fn message(self) -> &'static str {
        match self {
            Self::Workflow => "Resource not found",
            Self::LatestWorkflowVersion => "Unable to find latest version",
            Self::NamedWorkflowVersion => "Unable to find the requested version",
            Self::WorkflowRun => "Workflow run not found",
            // Not in any recording of this service (no fixture reads a task
            // instance); entity-level and free of request values, like the others,
            // so a client can tell which entity failed to resolve.
            Self::TaskInstance => "Task instance not found",
        }
    }

    /// The `ResourceType` the refusal reports for this entity.
    pub fn resource_type(self) -> &'static str {
        match self {
            Self::Workflow => "workflow",
            Self::LatestWorkflowVersion | Self::NamedWorkflowVersion => "workflow-version",
            Self::WorkflowRun => "workflow-run",
            Self::TaskInstance => "task-instance",
        }
    }
}

/// The modelled payload for a reference that resolves to nothing: the entity-level
/// message, plus the `ResourceId`/`ResourceType` the shape requires.
///
/// Every handler in this service funnels its lookup misses through here (wrapped by
/// [`lookup_miss`]), so one status and one vocabulary cover them all.
pub fn unresolved(entity: UnresolvedEntity, resource_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: entity.message().to_string(),
        resource_id: resource_id.to_string(),
        resource_type: entity.resource_type().to_string(),
    }
}

/// Serve a lookup miss the way the recorded service does: the modelled
/// `ResourceNotFoundException` the handler built, with [`LOOKUP_MISS_STATUS`].
pub fn lookup_miss<E>(error: E) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::modeled_with_status(error, LOOKUP_MISS_STATUS)
}

/// A `SystemTime` as the modelled `TimestampValue`.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

// ===========================================================================
// Request validation: the modelled `ValidationException` for a bad member value
// ===========================================================================
//
// Every operation of this service models `ValidationException` (HTTP 400) with
// `Message` and `Reason` @required and an optional `FieldList`
// (`smithy_query(query="shape", shape="ValidationException,ValidationExceptionField")`).
// A member value that fails a constraint the model declares for it is therefore
// answered with that shape — a top-level `Message`, `Reason` =
// `fieldValidationFailed`, and one `FieldList` entry per offending member — and
// NOT with the framework's generic refusal envelope, which carries none of them.
//
// NOTHING about constraint checking is re-implemented here. The constraints a
// member's `skip_validation` strips off its generated member schema are
// RE-DECLARED below in the framework's own representation — a
// [`TraitMap`](::aws_smithy_schema::TraitMap) of
// [`LengthTrait`]/[`PatternTrait`]/[`EnumTrait`]/[`ElementConstraintsTrait`], the
// exact form `vendor/smithy-macros` would have attached — hung on a member schema
// of the same name, and the check itself is
// [`::aws_smithy_schema_server::constraints::validate`], the SAME walk the request
// boundary runs. So the clause, the elision of a long value, the violation cap and
// the lowerCamel path are all the framework's, and a request refused here reads
// identically to one refused at the boundary.
//
// An operation declares `skip_validation = [...]` on its input struct in
// `src/ops/types/` and hands the matching member schemas from this module to
// [`validate_members`]; its violations become the modelled error through
// [`constraint_validation_exception`].
//
// The ARN-typed members are the one class whose REPORTING is this service's own
// rather than the framework's: a request's ARNs are parsed by the service's own
// ARN validators, which state their own field name and message. The acceptance
// test is still the modelled `@pattern`, run by the same walk; only the refusal
// is worded by this service. See [`arn_validation_exception`].
//
// ONE member is deliberately NOT covered: the paginated operations' `MaxResults`
// and its `@range(1..100)`. The recorded AWS answer for an out-of-range page size
// is the request-validation layer's own envelope — a 400 whose body carries a
// lowercase `message` and neither `Reason` nor `FieldList` — so that bound is
// left where the framework boundary already refuses it, exactly as recorded.

/// The `ValidationExceptionReason` a bad member value carries.
///
/// `smithy_query(query="shape", shape="ValidationExceptionReason")` lists
/// `unknownOperation | cannotParse | fieldValidationFailed | other`; a value
/// that failed the constraint declared for its member is the third.
pub const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// This service's Smithy namespace, for the shadow schemas below.
const NAMESPACE: &str = "com.amazonaws.mwaaserverless";

/// The workflow-ARN `@pattern`, shared by `WorkflowArn` and
/// `TaggableResourceArn` (`smithy_query(query="shape", shape="WorkflowArn")`).
const WORKFLOW_ARN_PATTERN: &str = r"^arn:aws(?:-(?:cn|us-gov|iso|iso-b|iso-e|iso-f))?:airflow-serverless:([a-z]{2}-[a-z]+-[0-9]{1}):([0-9]{12}):workflow/([a-zA-Z0-9][a-zA-Z0-9\.\-_]{0,254}-[a-zA-Z0-9]{10})$";

/// The identifier `@pattern`, shared by `NameString` and `IdString`.
const IDENTIFIER_PATTERN: &str = r"^[a-zA-Z0-9]+[a-zA-Z0-9\.\-_]*$";

/// `WorkflowArn`: `@length(1..2048)` plus the workflow-ARN `@pattern`
/// (`smithy_query(query="shape", shape="WorkflowArn")`).
static WORKFLOW_ARN_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(2048))));
    traits.insert(Box::new(PatternTrait::new(WORKFLOW_ARN_PATTERN)));
    traits
});

/// `TaggableResourceArn`: `@length(1..1011)` plus the workflow-ARN `@pattern`.
static TAGGABLE_RESOURCE_ARN_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(1011))));
    traits.insert(Box::new(PatternTrait::new(WORKFLOW_ARN_PATTERN)));
    traits
});

/// `RoleARN`: `@length(1..2048)` plus the IAM role-ARN `@pattern`.
static ROLE_ARN_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(2048))));
    traits.insert(Box::new(PatternTrait::new(
        r"^arn:aws(?:-(?:cn|us-gov|iso|iso-b|iso-e|iso-f))?:iam::[0-9]{12}:role(/[a-zA-Z0-9+=,.@_\-]{1,512})*?/[a-zA-Z0-9+=,.@_\-]{1,64}$",
    )));
    traits
});

/// `NameString`: `@length(1..255)` plus the identifier `@pattern`.
static NAME_STRING_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(255))));
    traits.insert(Box::new(PatternTrait::new(IDENTIFIER_PATTERN)));
    traits
});

/// `IdString`: `@length(1..255)` plus the identifier `@pattern`.
static ID_STRING_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(255))));
    traits.insert(Box::new(PatternTrait::new(IDENTIFIER_PATTERN)));
    traits
});

/// `IdempotencyTokenString`: `@length(1..128)` plus printable-ASCII `@pattern`.
static IDEMPOTENCY_TOKEN_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(128))));
    traits.insert(Box::new(PatternTrait::new(r"^[\x21-\x7E]+$")));
    traits
});

/// `DescriptionString`: `@length(1..1024)` plus `@pattern(^.+$)`.
static DESCRIPTION_STRING_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(1024))));
    traits.insert(Box::new(PatternTrait::new(r"^.+$")));
    traits
});

/// `GenericString`: `@length(1..255)` plus `@pattern(^.*$)`.
static GENERIC_STRING_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(255))));
    traits.insert(Box::new(PatternTrait::new(r"^.*$")));
    traits
});

/// `VersionId`: `@length(1..2048)` plus `@pattern(^.+$)`.
static VERSION_ID_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(2048))));
    traits.insert(Box::new(PatternTrait::new(r"^.+$")));
    traits
});

/// `WorkflowVersion`: `@length(32..32)` plus `@pattern(^[a-z0-9]{32}$)`.
static WORKFLOW_VERSION_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(32), Some(32))));
    traits.insert(Box::new(PatternTrait::new(r"^[a-z0-9]{32}$")));
    traits
});

/// `TagKey`: `@length(1..128)` plus `@pattern(^.*$)`.
static TAG_KEY_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(128))));
    traits.insert(Box::new(PatternTrait::new(r"^.*$")));
    traits
});

/// `TagValue`: `@length(0..256)` plus `@pattern(^.*$)`.
static TAG_VALUE_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(0), Some(256))));
    traits.insert(Box::new(PatternTrait::new(r"^.*$")));
    traits
});

/// `SecurityGroupString`: `@length(1..255)` plus `@pattern(^sg-[a-z0-9]*$)`.
static SECURITY_GROUP_STRING_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(255))));
    traits.insert(Box::new(PatternTrait::new(r"^sg-[a-z0-9]*$")));
    traits
});

/// `SubnetString`: `@length(1..255)` plus `@pattern(^subnet-[a-z0-9]*$)`.
static SUBNET_STRING_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(255))));
    traits.insert(Box::new(PatternTrait::new(r"^subnet-[a-z0-9]*$")));
    traits
});

/// `S3Location$Bucket`: `@length(3..63)`, no pattern
/// (`smithy_query(query="shape", shape="S3Location")`).
static S3_BUCKET_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(3), Some(63))));
    traits
});

/// `S3Location$ObjectKey`: `@length(1..1024)`, no pattern.
static S3_OBJECT_KEY_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(1024))));
    traits
});

/// `S3Location$VersionId`: `@length(1..1024)`, no pattern.
static S3_VERSION_ID_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(1024))));
    traits
});

/// `EncryptionConfiguration$Type`: the `EncryptionType` value set.
static ENCRYPTION_TYPE_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(EnumTrait::new(ENCRYPTION_TYPE_VALUES)));
    traits
});

/// `Tags`: `@length(0..50)` ENTRIES, with `TagValue`'s own constraints carried
/// for each value exactly as codegen carries them
/// (`smithy_query(query="shape", shape="Tags")`). A map KEY is not walked, which
/// is the framework's own rule, not a choice made here.
static TAGS_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(0), Some(50))));
    traits.insert(Box::new(ElementConstraintsTrait::new(&TAG_VALUE_TRAITS)));
    traits
});

/// `TagKeys`: `@length(1..50)` ENTRIES, with `TagKey`'s own constraints carried
/// for each element (`smithy_query(query="shape", shape="TagKeys")`).
static TAG_KEYS_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(50))));
    traits.insert(Box::new(ElementConstraintsTrait::new(&TAG_KEY_TRAITS)));
    traits
});

/// `SecurityGroupIds`: no entry-count bound; each element carries
/// `SecurityGroupString`'s constraints.
static SECURITY_GROUP_IDS_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(ElementConstraintsTrait::new(
        &SECURITY_GROUP_STRING_TRAITS,
    )));
    traits
});

/// `SubnetIds`: no entry-count bound; each element carries `SubnetString`'s
/// constraints.
static SUBNET_IDS_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(ElementConstraintsTrait::new(&SUBNET_STRING_TRAITS)));
    traits
});

// --- The member schemas the walk reads ------------------------------------
//
// One per (member name, target shape) pair this service hands to the handler,
// shared across every operation that declares that member: the walk reads only
// a member schema's NAME (for the violation path) and its TRAIT MAP, so one
// declaration serves every operation whose model gives that member the same
// target shape. `WorkflowVersion` appears twice because two operations give it
// two different target shapes.

/// `WorkflowArn`, as `CreateWorkflow`'s sibling operations declare it.
pub static M_WORKFLOW_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$WorkflowArn",
        NAMESPACE,
        "WorkflowArn",
    ),
    ShapeType::String,
    "WorkflowArn",
    0,
)
.with_traits(&WORKFLOW_ARN_TRAITS);

/// `ResourceArn`, which targets `TaggableResourceArn`.
pub static M_RESOURCE_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$ResourceArn",
        NAMESPACE,
        "ResourceArn",
    ),
    ShapeType::String,
    "ResourceArn",
    1,
)
.with_traits(&TAGGABLE_RESOURCE_ARN_TRAITS);

/// `RoleArn`, which targets `RoleARN`.
pub static M_ROLE_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$RoleArn",
        NAMESPACE,
        "RoleArn",
    ),
    ShapeType::String,
    "RoleArn",
    2,
)
.with_traits(&ROLE_ARN_TRAITS);

/// `Name`, which targets `NameString`.
pub static M_NAME: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$Name",
        NAMESPACE,
        "Name",
    ),
    ShapeType::String,
    "Name",
    3,
)
.with_traits(&NAME_STRING_TRAITS);

/// `ClientToken`, which targets `IdempotencyTokenString`.
pub static M_CLIENT_TOKEN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$ClientToken",
        NAMESPACE,
        "ClientToken",
    ),
    ShapeType::String,
    "ClientToken",
    4,
)
.with_traits(&IDEMPOTENCY_TOKEN_TRAITS);

/// `Description`, which targets `DescriptionString`.
pub static M_DESCRIPTION: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$Description",
        NAMESPACE,
        "Description",
    ),
    ShapeType::String,
    "Description",
    5,
)
.with_traits(&DESCRIPTION_STRING_TRAITS);

/// `TriggerMode`, which targets `GenericString`.
pub static M_TRIGGER_MODE: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$TriggerMode",
        NAMESPACE,
        "TriggerMode",
    ),
    ShapeType::String,
    "TriggerMode",
    6,
)
.with_traits(&GENERIC_STRING_TRAITS);

/// `RunId`, which targets `IdString`.
pub static M_RUN_ID: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$RunId",
        NAMESPACE,
        "RunId",
    ),
    ShapeType::String,
    "RunId",
    7,
)
.with_traits(&ID_STRING_TRAITS);

/// `TaskInstanceId`, which targets `IdString`.
pub static M_TASK_INSTANCE_ID: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$TaskInstanceId",
        NAMESPACE,
        "TaskInstanceId",
    ),
    ShapeType::String,
    "TaskInstanceId",
    8,
)
.with_traits(&ID_STRING_TRAITS);

/// `WorkflowVersion` where the operation targets `WorkflowVersion` — a version
/// IDENTIFIER, exactly 32 characters (`GetWorkflow`, `DeleteWorkflow`).
pub static M_WORKFLOW_VERSION_ID: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$WorkflowVersionId",
        NAMESPACE,
        "WorkflowVersion",
    ),
    ShapeType::String,
    "WorkflowVersion",
    9,
)
.with_traits(&WORKFLOW_VERSION_TRAITS);

/// `WorkflowVersion` where the operation targets `VersionId` — a version
/// SELECTOR, 1..2048 characters (`ListWorkflowRuns`, `StartWorkflowRun`).
pub static M_WORKFLOW_VERSION_SELECTOR: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$WorkflowVersionSelector",
        NAMESPACE,
        "WorkflowVersion",
    ),
    ShapeType::String,
    "WorkflowVersion",
    10,
)
.with_traits(&VERSION_ID_TRAITS);

/// `Tags`, the `Tags` map.
pub static M_TAGS: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$Tags",
        NAMESPACE,
        "Tags",
    ),
    ShapeType::Map,
    "Tags",
    11,
)
.with_traits(&TAGS_TRAITS);

/// `TagKeys`, the `TagKeys` list.
pub static M_TAG_KEYS: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$TagKeys",
        NAMESPACE,
        "TagKeys",
    ),
    ShapeType::List,
    "TagKeys",
    12,
)
.with_traits(&TAG_KEYS_TRAITS);

/// `Code`, the union whose `S3Location` members carry the constraints.
pub static M_CODE: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$Code",
        NAMESPACE,
        "Code",
    ),
    ShapeType::Union,
    "Code",
    13,
);

/// `EncryptionConfiguration`, whose `Type` carries the value set.
pub static M_ENCRYPTION_CONFIGURATION: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$EncryptionConfiguration",
        NAMESPACE,
        "EncryptionConfiguration",
    ),
    ShapeType::Structure,
    "EncryptionConfiguration",
    14,
);

/// `NetworkConfiguration`, whose two id lists carry their element constraints.
pub static M_NETWORK_CONFIGURATION: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$NetworkConfiguration",
        NAMESPACE,
        "NetworkConfiguration",
    ),
    ShapeType::Structure,
    "NetworkConfiguration",
    15,
);

/// `Code$S3Location`.
pub static M_S3_LOCATION: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#Code$S3Location",
        NAMESPACE,
        "S3Location",
    ),
    ShapeType::Structure,
    "S3Location",
    0,
);

/// `S3Location$Bucket`.
pub static M_BUCKET: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#S3Location$Bucket",
        NAMESPACE,
        "Bucket",
    ),
    ShapeType::String,
    "Bucket",
    0,
)
.with_traits(&S3_BUCKET_TRAITS);

/// `S3Location$ObjectKey`.
pub static M_OBJECT_KEY: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#S3Location$ObjectKey",
        NAMESPACE,
        "ObjectKey",
    ),
    ShapeType::String,
    "ObjectKey",
    1,
)
.with_traits(&S3_OBJECT_KEY_TRAITS);

/// `S3Location$VersionId`.
pub static M_S3_VERSION_ID: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#S3Location$VersionId",
        NAMESPACE,
        "VersionId",
    ),
    ShapeType::String,
    "VersionId",
    2,
)
.with_traits(&S3_VERSION_ID_TRAITS);

/// `EncryptionConfiguration$Type`.
pub static M_ENCRYPTION_TYPE: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#EncryptionConfiguration$Type",
        NAMESPACE,
        "Type",
    ),
    ShapeType::String,
    "Type",
    0,
)
.with_traits(&ENCRYPTION_TYPE_TRAITS);

/// `NetworkConfiguration$SecurityGroupIds`.
pub static M_SECURITY_GROUP_IDS: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#NetworkConfiguration$SecurityGroupIds",
        NAMESPACE,
        "SecurityGroupIds",
    ),
    ShapeType::List,
    "SecurityGroupIds",
    0,
)
.with_traits(&SECURITY_GROUP_IDS_TRAITS);

/// `NetworkConfiguration$SubnetIds`.
pub static M_SUBNET_IDS: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#NetworkConfiguration$SubnetIds",
        NAMESPACE,
        "SubnetIds",
    ),
    ShapeType::List,
    "SubnetIds",
    1,
)
.with_traits(&SUBNET_IDS_TRAITS);

/// The root of the shadow input the walk is driven over. Its member list is
/// empty on purpose: the walk visits what [`ConstrainedMembers`] WRITES, which
/// is whichever members the calling operation handed over, and a root struct
/// schema carries no member name so it contributes no path segment.
static CONSTRAINED_INPUT_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput",
        NAMESPACE,
        "ConstrainedInput",
    ),
    ShapeType::Structure,
    &[],
);

/// One value handed to the validating walk, written the way the codec writes
/// that shape so the walk builds the same path and runs the same checks.
pub enum ConstrainedValue<'a> {
    /// A string leaf.
    Text(&'a str),
    /// A list of strings; the walk names each element `<member>.<n>.member`.
    Texts(&'a [String]),
    /// A `String -> String` map; the walk names each value `<member>.<key>.member`.
    Entries(&'a HashMap<String, String>),
    /// A nested structure or union that writes its own leaves.
    Nested(&'a dyn SerializableStruct),
}

/// The members of ONE request whose constraints their operation handed over,
/// written for [`::aws_smithy_schema_server::constraints::validate`].
pub struct ConstrainedMembers<'a> {
    members: &'a [(&'static Schema, ConstrainedValue<'a>)],
}

impl SerializableStruct for ConstrainedMembers<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        for (schema, value) in self.members {
            write_constrained(s, schema, value)?;
        }
        Ok(())
    }
}

/// Write one member for the walk. Mirrors what `vendor/smithy-macros` emits for
/// each shape kind, including passing the collection member schema for a scalar
/// element or map value — which is how the walk tells a collection's entry-count
/// bound from its elements' own bounds.
fn write_constrained(
    s: &mut dyn ShapeSerializer,
    schema: &'static Schema,
    value: &ConstrainedValue<'_>,
) -> Result<(), SerdeError> {
    match value {
        ConstrainedValue::Text(text) => s.write_string(schema, text),
        ConstrainedValue::Texts(texts) => s.write_list(schema, &|sl| {
            for text in texts.iter() {
                sl.write_string(schema, text)?;
            }
            Ok(())
        }),
        ConstrainedValue::Entries(entries) => s.write_map(schema, &|sm| {
            for (key, entry) in entries.iter() {
                sm.write_string(schema, key)?;
                sm.write_string(schema, entry)?;
            }
            Ok(())
        }),
        ConstrainedValue::Nested(nested) => s.write_struct(schema, *nested),
    }
}

/// Run the framework's own constraint walk over the members an operation handed
/// over, and report every violation it found.
pub fn validate_members(
    members: &[(&'static Schema, ConstrainedValue<'_>)],
) -> Vec<Violation> {
    validate(&ConstrainedMembers { members }, &CONSTRAINED_INPUT_SCHEMA)
}

/// `Code`'s `S3Location` variant, for the walk.
pub struct ConstrainedS3Location<'a> {
    /// `S3Location$Bucket`.
    pub bucket: &'a str,
    /// `S3Location$ObjectKey`.
    pub object_key: &'a str,
    /// `S3Location$VersionId`, when the request carries one.
    pub version_id: Option<&'a str>,
}

impl SerializableStruct for ConstrainedS3Location<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&M_BUCKET, self.bucket)?;
        s.write_string(&M_OBJECT_KEY, self.object_key)?;
        if let Some(version_id) = self.version_id {
            s.write_string(&M_S3_VERSION_ID, version_id)?;
        }
        Ok(())
    }
}

/// The `Code` union, for the walk.
pub struct ConstrainedCode<'a> {
    /// The union's one modelled variant.
    pub s3_location: ConstrainedS3Location<'a>,
}

impl SerializableStruct for ConstrainedCode<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_struct(&M_S3_LOCATION, &self.s3_location)
    }
}

/// The `Code` union of a request, ready for the walk, or `None` when the
/// request carries none.
pub fn constrained_code(code: Option<&Code>) -> Option<ConstrainedCode<'_>> {
    let Code::S3Location(location) = code?;
    Some(ConstrainedCode {
        s3_location: ConstrainedS3Location {
            bucket: &location.bucket,
            object_key: &location.object_key,
            version_id: location.version_id.as_deref(),
        },
    })
}

/// `EncryptionConfiguration`, for the walk. `KmsKeyId` targets a plain `String`
/// and declares no constraint, so it is not written.
pub struct ConstrainedEncryptionConfiguration<'a> {
    /// `EncryptionConfiguration$Type`.
    pub r#type: &'a str,
}

impl SerializableStruct for ConstrainedEncryptionConfiguration<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&M_ENCRYPTION_TYPE, self.r#type)
    }
}

/// A request's `EncryptionConfiguration`, ready for the walk.
pub fn constrained_encryption_configuration(
    config: Option<&EncryptionConfiguration>,
) -> Option<ConstrainedEncryptionConfiguration<'_>> {
    config.map(|config| ConstrainedEncryptionConfiguration {
        r#type: &config.r#type,
    })
}

/// `NetworkConfiguration`, for the walk.
pub struct ConstrainedNetworkConfiguration<'a> {
    /// `NetworkConfiguration$SecurityGroupIds`.
    pub security_group_ids: Option<&'a [String]>,
    /// `NetworkConfiguration$SubnetIds`.
    pub subnet_ids: Option<&'a [String]>,
}

impl SerializableStruct for ConstrainedNetworkConfiguration<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        if let Some(ids) = self.security_group_ids {
            write_constrained(s, &M_SECURITY_GROUP_IDS, &ConstrainedValue::Texts(ids))?;
        }
        if let Some(ids) = self.subnet_ids {
            write_constrained(s, &M_SUBNET_IDS, &ConstrainedValue::Texts(ids))?;
        }
        Ok(())
    }
}

/// A request's `NetworkConfiguration`, ready for the walk.
pub fn constrained_network_configuration(
    config: Option<&NetworkConfiguration>,
) -> Option<ConstrainedNetworkConfiguration<'_>> {
    config.map(|config| ConstrainedNetworkConfiguration {
        security_group_ids: config.security_group_ids.as_deref(),
        subnet_ids: config.subnet_ids.as_deref(),
    })
}

// ---------------------------------------------------------------------------
// The service's own ARN validation
// ---------------------------------------------------------------------------
//
// An ARN-typed input member is not reported as a member-constraint failure.
// This service parses the ARNs a request carries with its OWN validators,
// before any member constraint is rendered, and each validator reports its own
// result:
//
// * a resource ARN of this service (a member targeting `WorkflowArn` or
//   `TaggableResourceArn`) is reported under the generic field name `Arn` with
//   the validator's short message, and the top-level `Message` is composed as
//   the service-level prefix plus that field message — so a client attributes
//   an ARN failure to one field name whichever operation refused it;
// * the IAM role ARN (a member targeting `RoleARN`) is reported under the name
//   the model gives the member, with a message built from the request: the
//   fixed prefix plus the rejected value as supplied, which is also the
//   top-level `Message`.
//
// Neither carries the modelled `@pattern` source, which is internal.

/// `FieldList$Name` for a resource ARN this service refused: the generic field
/// name its ARN validator reports, not the member the value arrived in.
const ARN_FIELD_NAME: &str = "Arn";

/// The field message a malformed resource ARN carries.
const INVALID_ARN_MESSAGE: &str = "Invalid arn";

/// How a service-level validation failure composes its top-level `Message`:
/// this fixed prefix followed by the offending field's own message.
const VALIDATION_FAILED_PREFIX: &str = "Validation failed. ";

/// The fixed prefix of the role-ARN validator's message, followed by the
/// rejected value as supplied.
const INVALID_ROLE_ARN_PREFIX: &str = "Invalid Role ARN: ";

/// Which of this service's ARN validators owns an input member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArnValidator {
    /// A member naming a resource OF THIS SERVICE (`WorkflowArn`,
    /// `TaggableResourceArn`).
    Resource,
    /// A member naming an IAM role (`RoleARN`).
    Role,
}

/// Every ARN-typed input member of this service, with the validator that owns
/// it — keyed by the member SCHEMA the walk reads, so this list cannot drift
/// from the member names the violation paths carry.
static ARN_MEMBERS: &[(&Schema, ArnValidator)] = &[
    (&M_WORKFLOW_ARN, ArnValidator::Resource),
    (&M_RESOURCE_ARN, ArnValidator::Resource),
    (&M_ROLE_ARN, ArnValidator::Role),
];

/// The ARN validator that owns the member a violation `path` names, or `None`
/// when the path is not one of this service's ARN members. A path is compared
/// against the framework's own lowerCamel member spelling, the exact form it
/// builds a path segment with.
fn arn_validator_for(path: &str) -> Option<ArnValidator> {
    ARN_MEMBERS.iter().find_map(|(schema, validator)| {
        let member = schema.member_name()?;
        (path == ::aws_smithy_schema_server::constraints::path::member(member))
            .then_some(*validator)
    })
}

/// The ARN validator's own `ValidationException` when a request carried a
/// malformed ARN, or `None` when no ARN member was at fault.
///
/// The validators run before member-constraint rendering, so an ARN failure is
/// answered on its own: the `FieldList` carries the ARN field alone, and the
/// `Message` is the one that validator states. A resource ARN names the
/// resource a call targets, so it is parsed before the role ARN a create or
/// update also carries.
fn arn_validation_exception(violations: &[Violation]) -> Option<ValidationException> {
    for owner in [ArnValidator::Resource, ArnValidator::Role] {
        for violation in violations {
            // An ABSENT member is not a malformed ARN: there is no value to
            // parse, so it keeps the missing-member refusal the walk states.
            let Some(value) = violation.value.as_deref() else {
                continue;
            };
            if arn_validator_for(&violation.path) != Some(owner) {
                continue;
            }
            let field = match owner {
                ArnValidator::Resource => ValidationExceptionField {
                    name: ARN_FIELD_NAME.to_string(),
                    message: INVALID_ARN_MESSAGE.to_string(),
                },
                ArnValidator::Role => ValidationExceptionField {
                    name: model_member_name(&violation.path),
                    message: format!("{INVALID_ROLE_ARN_PREFIX}{value}"),
                },
            };
            let message = match owner {
                // The service-level field validation prefixes its field
                // message; the role-ARN check states its message directly.
                ArnValidator::Resource => format!("{VALIDATION_FAILED_PREFIX}{}", field.message),
                ArnValidator::Role => field.message.clone(),
            };
            return Some(ValidationException {
                message,
                reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
                field_list: Some(vec![field]),
            });
        }
    }
    None
}

/// This service's modelled `ValidationException` for the violations the walk
/// reported.
///
/// An ARN member at fault is answered by [`arn_validation_exception`] — this
/// service validates the ARNs a request carries itself, and its validators'
/// results are not the generic constraint rendering.
///
/// Otherwise the `Message` is the framework's own envelope, so a refusal made
/// here reads exactly like one the boundary made; `Reason` is the `@required`
/// `fieldValidationFailed` the boundary cannot carry; and `FieldList` names
/// every member that failed.
///
/// A violation `path` is the walk's lowerCamel rendering (`name`,
/// `subnetIds.0.member`), which is the form AWS validation messages use, so it
/// is kept verbatim in the clause. `FieldList$Name` is documented as "the name
/// of the field that failed validation", so its FIRST segment is restored to the
/// spelling the model gives that member by [`model_member_name`].
pub fn constraint_validation_exception(violations: &[Violation]) -> ValidationException {
    if let Some(exception) = arn_validation_exception(violations) {
        return exception;
    }
    ValidationException {
        message: ::aws_smithy_schema_server::envelope(violations)
            .unwrap_or_else(|| "invalid request".to_string()),
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(
            violations
                .iter()
                .map(|violation| ValidationExceptionField {
                    name: model_member_name(&violation.path),
                    message: violation.message(),
                })
                .collect(),
        ),
    }
}

/// A violation path whose FIRST segment is spelled as the model spells that
/// member (`roleArn` -> `RoleArn`), which is the inverse of the walk's single
/// first-character lowering and so exact for this service, whose every member
/// name begins with an uppercase letter.
///
/// Only the first segment is restored. A later segment may be a map KEY the
/// CALLER chose, which must be echoed exactly as sent — the same rule the
/// framework states where it builds those paths — and re-casing it would report
/// a tag key the request never carried.
fn model_member_name(path: &str) -> String {
    let (head, rest) = match path.split_once('.') {
        Some((head, rest)) => (head, Some(rest)),
        None => (path, None),
    };
    let mut chars = head.chars();
    let head: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    };
    match rest {
        Some(rest) => format!("{head}.{rest}"),
        None => head,
    }
}

/// The modelled `ValidationException` for a pagination cursor this service did
/// not issue.
///
/// `NextToken` targets a plain `String` and so declares no constraint the
/// boundary could check; the token's validity is a service rule, and it is
/// refused with the same shape, reason and `FieldList` as a constraint failure.
pub fn invalid_page_token(operation: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "1 validation error detected: Value at 'NextToken' failed to satisfy \
             constraint: Member must be a pagination token issued by {operation}"
        ),
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: "NextToken".to_string(),
            message: format!(
                "Value at 'NextToken' failed to satisfy constraint: Member must be a \
                 pagination token issued by {operation}"
            ),
        }]),
    }
}
