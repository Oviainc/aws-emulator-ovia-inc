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
