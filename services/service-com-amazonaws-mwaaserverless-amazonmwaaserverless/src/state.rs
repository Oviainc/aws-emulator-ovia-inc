// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mwaaserverless-amazonmwaaserverless service.
//!
//! The store holds the workflow control plane — the workflows a caller created, the
//! immutable versions of each one, and the runs started under them — plus the
//! idempotency ledger that lets a repeated `ClientToken` replay its first answer.
//! Every handler reads and writes THIS struct through [`service_state`]; nothing in
//! `src/ops/` keeps a store of its own.

//! Every collection here is BOUNDED, because all three grow from caller requests:
//! `workflows` is capped at [`MAX_WORKFLOWS`] and a workflow's `runs` at
//! [`MAX_RUNS_PER_WORKFLOW`] (a create or a run start past the ceiling is refused with the
//! modelled `ServiceQuotaExceededException`, so the workflows x runs product is finite and
//! so is the caller-supplied data each record holds); `workflow_names` holds exactly one
//! key per stored workflow, released by the delete that removes the record; and the
//! idempotency ledger is capped and evicted on its own terms (see [`IdempotencyLedger`]).
//! A listing's response is bounded too, by the modelled `MaxResults` range.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    Code, DefinitionS3Location, EncryptionConfiguration, LoggingConfiguration,
    NetworkConfiguration, ResourceNotFoundException,
};

/// The region used in minted ARNs when the request carries no usable SigV4
/// credential scope. Every generated emulator defaults to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace (and SigV4 signing name) this service uses, per the model's
/// `awsService.arnNamespace`.
pub const ARN_NAMESPACE: &str = "airflow-serverless";

/// How long after `StartWorkflowRun` the run leaves its initial non-terminal
/// status. The span is deliberately non-zero so a caller that reads the run
/// straight back still observes `QUEUED`.
pub const RUN_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// Why a run that was accepted never executes here. This emulator serves the
/// workflow control plane faithfully but has no Apache Airflow engine to run a
/// DAG definition with, so the run ends in the modelled failure state rather
/// than reporting a success nothing computed.
pub const NO_AIRFLOW_ENGINE: &str =
    "The emulator accepted this run but has no Apache Airflow execution engine, \
     so the workflow definition was never executed.";

/// `WorkflowStatus` as the model spells it.
pub const WORKFLOW_STATUS_READY: &str = "READY";

/// `WorkflowRunStatus` values this slice uses.
pub const RUN_STATUS_QUEUED: &str = "QUEUED";
/// The modelled failure state a run that could not execute ends in.
pub const RUN_STATUS_FAILED: &str = "FAILED";

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
    pub task_instances: Vec<String>,
    /// The instant the clock owes this run's exit from its initial non-terminal
    /// status. Stored, never slept on.
    pub settles_at: SystemTime,
}

impl WorkflowRun {
    /// Apply the status change the clock already made due, and report whether
    /// anything changed.
    ///
    /// This is the ONLY write a reporting operation may make: at `now` the
    /// transition was owed whether or not anybody called.
    pub fn settle(&mut self, now: SystemTime) -> bool {
        if self.status != RUN_STATUS_QUEUED || now < self.settles_at {
            return false;
        }
        let elapsed = self
            .settles_at
            .duration_since(self.started_on)
            .unwrap_or_default();
        self.status = RUN_STATUS_FAILED.to_string();
        self.completed_on = Some(self.settles_at);
        self.modified_at = self.settles_at;
        self.duration = Some(duration_in_seconds(elapsed));
        self.error_message = Some(NO_AIRFLOW_ENGINE.to_string());
        true
    }
}

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
    pub workflows: BTreeMap<String, Workflow>,
    /// `Name` -> `WorkflowArn`, the uniqueness index `CreateWorkflow` conflicts on. One
    /// key per stored workflow, so [`MAX_WORKFLOWS`] bounds this map too.
    pub workflow_names: BTreeMap<String, String>,
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

/// The region an ARN minted for this request carries: the SigV4 credential
/// scope's region when it is one the modelled `WorkflowArn` pattern accepts,
/// else [`DEFAULT_REGION`].
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
        }
    }

    /// The `ResourceType` the refusal reports for this entity.
    pub fn resource_type(self) -> &'static str {
        match self {
            Self::Workflow => "workflow",
            Self::LatestWorkflowVersion | Self::NamedWorkflowVersion => "workflow-version",
            Self::WorkflowRun => "workflow-run",
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
