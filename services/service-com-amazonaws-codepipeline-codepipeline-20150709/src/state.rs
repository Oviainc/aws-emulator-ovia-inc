// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codepipeline-codepipeline-20150709 service,
//! plus the small helpers every handler of this crate shares.
//!
//! The pipeline definition lifecycle (`CreatePipeline` / `GetPipeline` /
//! `ListPipelines` / `DeletePipeline`), the custom action types
//! `CreateCustomActionType` mints and `ListActionTypes` reports, the execution
//! records `StartPipelineExecution` mints and the stage-transition states
//! `DisableStageTransition` / `EnableStageTransition` change all live in the one
//! store reached through [`service_state`], so every handler computes its answer
//! from persisted state rather than from the request it is answering.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use crate::ops::types::{
    ActionConfigurationProperty, ActionState, ActionType, ActionTypeId, ActionTypeSettings,
    ArtifactDetails, CurrentRevision, ExecutionDetails, ExecutionTrigger, PipelineDeclaration,
    PipelineExecution, PipelineExecutionSummary, PipelineMetadata, PipelineSummary,
    ResolvedPipelineVariable, SourceRevision, StageState, Tag, ThirdPartyJob, TransitionState,
};

/// The region an ARN carries when the request names none.
///
/// `emulator_server::emulator_arn` documents that "`region` is caller-supplied
/// (emulators default to `us-east-1`)", so an unsigned request — which carries no
/// SigV4 credential scope to read a region from — mints ARNs in that region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `codepipeline` ARN service namespace, as OPERATIONS.md records it
/// ("ARN namespace: `codepipeline`").
pub const ARN_SERVICE: &str = "codepipeline";

/// The `ActionOwner` value an action type carries when a PARTNER owns it — the only
/// kind of action type `PollForThirdPartyJobs` serves ("Used for partner actions
/// only"). One of the three values the model declares, as
/// `crate::ops::types::ACTION_OWNER_VALUES` lists them (`AWS`, `Custom`,
/// `ThirdParty`); named here because two state rules turn on this one value.
pub const ACTION_OWNER_THIRD_PARTY: &str = "ThirdParty";

/// The `PipelineType` values the model declares
/// (`smithy_query query="shape" shape="PipelineType"` -> `V1`, `V2`).
pub const PIPELINE_TYPES: [&str; 2] = ["V1", "V2"];

/// The `ExecutionMode` values the model declares
/// (`smithy_query query="shape" shape="ExecutionMode"` -> `QUEUED`, `SUPERSEDED`,
/// `PARALLEL`).
pub const EXECUTION_MODES: [&str; 3] = ["QUEUED", "SUPERSEDED", "PARALLEL"];

/// `PipelineDeclaration$executionMode` documents "The default mode is SUPERSEDED".
pub const DEFAULT_EXECUTION_MODE: &str = "SUPERSEDED";

/// The `PipelineExecutionStatus` value an execution carries while it has neither
/// completed nor been stopped, from the model's enum
/// (`smithy_query query="shape" shape="PipelineExecutionStatus"` -> `Cancelled`,
/// `InProgress`, `Stopped`, `Stopping`, `Succeeded`, `Superseded`, `Failed`).
pub const STATUS_IN_PROGRESS: &str = "InProgress";

/// The `TriggerType` a `StartPipelineExecution` call records, from the model's enum
/// (`smithy_query query="shape" shape="TriggerType"`), whose member names the
/// interaction that started the execution.
pub const TRIGGER_TYPE_START_PIPELINE_EXECUTION: &str = "StartPipelineExecution";

/// The `JobStatus` value a job carries while a job worker holds it and may still
/// report a result for it: `AcknowledgeJob`/`AcknowledgeThirdPartyJob` document
/// that acknowledging a job "confirms a job worker has received the specified job",
/// and the status it moves to is this one, from the model's enum
/// (`JOB_STATUS_VALUES` -> `Created`, `Dispatched`, `Failed`, `InProgress`,
/// `Queued`, `Succeeded`, `TimedOut`).
pub const JOB_STATUS_IN_PROGRESS: &str = "InProgress";

/// The `JobStatus` value a job carries once a job worker has reported success for
/// it, from the same enum. Terminal: nothing moves a job out of it.
pub const JOB_STATUS_SUCCEEDED: &str = "Succeeded";

/// The `JobStatus` value a job carries while it waits for a worker to poll for it
/// and acknowledge it, from the same enum. A queued job has no worker that owns its
/// outcome yet, so no result can be reported for it.
pub const JOB_STATUS_QUEUED: &str = "Queued";

/// The `ExecutionType` of an execution that runs the pipeline forward, from the
/// model's enum (`smithy_query query="shape" shape="ExecutionType"` -> `STANDARD`,
/// `ROLLBACK`).
pub const EXECUTION_TYPE_STANDARD: &str = "STANDARD";

/// The `StageTransitionType` values the model declares
/// (`smithy_query query="shape" shape="StageTransitionType"` -> `Inbound`,
/// `Outbound`), in the order the enum lists them.
pub const STAGE_TRANSITION_TYPES: [&str; 2] = ["Inbound", "Outbound"];

/// The `StageTransitionType` naming the transition artifacts cross INTO a stage —
/// the only one `StageState` models a reporting member for
/// (`StageState$inboundTransitionState`).
pub const TRANSITION_TYPE_INBOUND: &str = STAGE_TRANSITION_TYPES[0];

/// The refusal message for a `transitionType` the `StageTransitionType` enum does
/// not declare, or `None` when the value is one of them.
///
/// One source for both `DisableStageTransition` and `EnableStageTransition` — each
/// owns its error enum, but neither owns the rule — so the two cannot drift on which
/// values they accept or on what they say about a value they do not.
pub fn invalid_transition_type_message(transition_type: &str) -> Option<String> {
    (!STAGE_TRANSITION_TYPES.contains(&transition_type)).then(|| {
        format!(
            "transitionType: {transition_type} is not a StageTransitionType, which is one of {}",
            STAGE_TRANSITION_TYPES.join(" | ")
        )
    })
}

/// The version number a freshly created pipeline carries.
///
/// `PipelineDeclaration$version` documents "A new pipeline always has a version
/// number of 1. This number is incremented when a pipeline is updated."
pub const FIRST_PIPELINE_VERSION: i32 = 1;

/// The most declaration versions this emulator retains per pipeline; accepting
/// another `UpdatePipeline` drops the oldest version beyond this window, so a
/// `GetPipeline` naming it answers `PipelineVersionNotFoundException`.
///
/// A bound is required: `Pipeline::versions` grows by one on every accepted update
/// and is released only when the pipeline is deleted, so without a cap a caller
/// could grow the store without limit by updating one pipeline. AWS publishes no
/// retained-version count, so the figure is this emulator's bound rather than a
/// documented constant; the current version is always retained, since the window is
/// larger than one.
pub const MAX_PIPELINE_VERSIONS_RETAINED: usize = 1000;

/// The most pipelines this emulator holds for the account before `CreatePipeline`
/// refuses another one.
///
/// A bound is required: `pipelines` grows by one on every accepted create and is
/// released only by `DeletePipeline`, so without a cap a caller could grow the store
/// without limit. The refusal it answers with is the one the model declares for
/// exactly this condition — `LimitExceededException`, "The number of pipelines
/// associated with the Amazon Web Services account has exceeded the limit allowed
/// for the account" — and the figure matches AWS's published default quota of 1000
/// pipelines per Region per account. AWS adjusts that quota per account, so the
/// value here is this emulator's bound rather than a universal constant.
pub const MAX_PIPELINES_PER_ACCOUNT: usize = 1000;

/// The most action types this emulator registers for the account before another
/// registration is refused.
///
/// A bound is required for the same reason the pipeline cap is: `action_types`
/// grows by one on every accepted registration, each entry carrying the
/// caller-supplied `ActionTypeId` members and the action's configuration
/// properties, and it is released only by forgetting that id — so without a cap a
/// caller could grow the store without limit. The refusal a handler answers past it
/// is the one `CreateCustomActionType` models for exactly this condition,
/// `LimitExceededException`. AWS adjusts its custom-action quota per account, so the
/// figure here is this emulator's bound rather than a documented constant.
///
/// [`ComAmazonawsCodepipelineCodepipeline20150709State::register_action_type`] is the
/// one way into the map, so the bound holds whatever registers.
pub const MAX_ACTION_TYPES_PER_ACCOUNT: usize = 1000;

/// The most recent executions this emulator retains per pipeline; starting another
/// one drops the oldest beyond this window, together with the `clientRequestToken`
/// entries that named it.
///
/// A bound is required for the same reason: `executions`, `executions_by_pipeline`
/// and `idempotency` all grow on every accepted `StartPipelineExecution`. Retaining
/// a bounded window is what the real service does too — `ListPipelineExecutions`
/// "gets a summary of the most RECENT executions" and documents that "pipeline
/// history is limited to the most recent 12 months" — but AWS bounds it by age and
/// this emulator bounds it by count, so the figure is chosen, not documented. With
/// both caps in force every map is bounded by
/// `MAX_PIPELINES_PER_ACCOUNT * MAX_EXECUTION_HISTORY_PER_PIPELINE`.
pub const MAX_EXECUTION_HISTORY_PER_PIPELINE: usize = 1000;

/// The `ActionOwner` a custom action type carries, from the model's enum
/// (`smithy_query query="shape" shape="ActionOwner"` -> `AWS`, `ThirdParty`,
/// `Custom`). `ActionTypeId$owner` documents the three values and
/// `CreateCustomActionType` "creates a new custom action", so an action type this
/// service mints is owned by `Custom`.
pub const ACTION_OWNER_CUSTOM: &str = "Custom";

/// The most tags one resource of this service holds.
///
/// AWS's resource-tagging limit of 50 tags per resource, which is what the modelled
/// `TooManyTagsException` ("The tags limit for a resource has been exceeded")
/// reports when a request would carry a resource past it.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// Per-service state for com-amazonaws-codepipeline-codepipeline-20150709. A handler reaches it through [`service_state`].
///
/// Every map is a `BTreeMap`, so the key order a listing pages through is total and
/// stable: that order is what makes an opaque `nextToken` ([`encode_page_token`])
/// round-trip deterministically.
#[derive(Debug, Default)]
pub struct ComAmazonawsCodepipelineCodepipeline20150709State {
    /// Stored pipelines, keyed by `PipelineDeclaration$name`.
    pub pipelines: BTreeMap<String, Pipeline>,
    /// Stored executions, keyed by their generated `pipelineExecutionId`.
    pub executions: BTreeMap<String, PipelineExecutionRecord>,
    /// Execution ids per pipeline name, oldest first (so newest-first listing is
    /// this sequence reversed).
    pub executions_by_pipeline: BTreeMap<String, Vec<String>>,
    /// `(pipeline name, clientRequestToken)` -> the execution id that token minted,
    /// so repeating an idempotency token returns the same execution. An entry lives
    /// exactly as long as the execution it names.
    pub idempotency: BTreeMap<(String, String), String>,
    /// The action types this ACCOUNT registered — the custom ones
    /// `CreateCustomActionType` mints — keyed by their `ActionTypeId`. The action
    /// types the service itself provides are not account state and live in
    /// [`BUILT_IN_ACTION_TYPES`]; [`ComAmazonawsCodepipelineCodepipeline20150709State::action_type_catalog`]
    /// is the one place the two are put in a single order.
    ///
    /// PRIVATE, unlike the maps above, so that every registration goes through
    /// [`ComAmazonawsCodepipelineCodepipeline20150709State::register_action_type`]
    /// and the [`MAX_ACTION_TYPES_PER_ACCOUNT`] bound cannot be bypassed; read it
    /// through `action_type_catalog` or `registered_action_type`.
    action_types: BTreeMap<ActionTypeKey, ActionTypeRecord>,
    /// The partner integrations registered with this service: the identity of a
    /// `ThirdParty` action type -> the AWS account id of the partner whose job worker
    /// CodePipeline serves that action type's jobs to.
    ///
    /// This is what `PollForThirdPartyJobs` authorizes against, and it is NOT the
    /// action-type catalog: `BUILT_IN_ACTION_TYPES` names the `ThirdParty` action
    /// types a pipeline can USE, while this map names the account each one's jobs are
    /// served to — a real partner's account, never the single account this emulator
    /// serves. AWS registers a partner integration when the partner onboards, OUT OF
    /// BAND: the API surface models no operation that creates one
    /// (`CreateCustomActionType` mints an action type whose owner is `Custom`, and
    /// `UpdateActionType` only updates one that already exists), so in the assembled
    /// emulator the map is empty and no caller is a registered partner. Write it
    /// through
    /// [`register_third_party_partner`](ComAmazonawsCodepipelineCodepipeline20150709State::register_third_party_partner).
    pub third_party_partners: BTreeMap<ActionTypeKey, String>,
    /// The third-party jobs waiting for a partner's job worker, per action type,
    /// oldest first — what `PollForThirdPartyJobs` reports to the registered partner.
    ///
    /// Queued by the action-execution plane when an execution reaches a partner
    /// action; this slice does not run that plane (every operation of it —
    /// `GetPipelineState`, `PutThirdPartyJobSuccessResult`, the acknowledge
    /// operations — is still a scaffold with its own issue), so no request path
    /// writes this map and it needs no per-request bound.
    pub third_party_jobs: BTreeMap<ActionTypeKey, Vec<ThirdPartyJobRecord>>,
}

impl ComAmazonawsCodepipelineCodepipeline20150709State {
    /// A fresh `pipelineExecutionId` no stored execution already uses.
    ///
    /// 122 random bits make a collision against the at most
    /// `MAX_PIPELINES_PER_ACCOUNT * MAX_EXECUTION_HISTORY_PER_PIPELINE` retained ids
    /// unreachable in practice; checking the store anyway makes uniqueness a property
    /// of the state rather than of the odds. Bounded, so a request can never spin.
    pub fn new_execution_id(&self) -> String {
        let mut candidate = random_uuid_v4();
        for _ in 0..8 {
            if !self.executions.contains_key(&candidate) {
                break;
            }
            candidate = random_uuid_v4();
        }
        candidate
    }

    /// Record `record` as its pipeline's newest execution and return its id.
    ///
    /// The retained history is bounded by [`MAX_EXECUTION_HISTORY_PER_PIPELINE`]:
    /// the oldest records beyond that window are dropped here, and so are the
    /// idempotency tokens that named them — otherwise a token would keep naming an
    /// execution no caller can read, and the three maps would grow per request
    /// without limit.
    pub fn record_execution(
        &mut self,
        record: PipelineExecutionRecord,
        client_request_token: Option<String>,
    ) -> String {
        let pipeline_name = record.pipeline_name.clone();
        let execution_id = record.pipeline_execution_id.clone();
        self.executions.insert(execution_id.clone(), record);

        let evicted: Vec<String> = {
            let history = self
                .executions_by_pipeline
                .entry(pipeline_name.clone())
                .or_default();
            history.push(execution_id.clone());
            let overflow = history
                .len()
                .saturating_sub(MAX_EXECUTION_HISTORY_PER_PIPELINE);
            history.drain(..overflow).collect()
        };
        if !evicted.is_empty() {
            for id in &evicted {
                self.executions.remove(id);
            }
            self.idempotency.retain(|_, id| !evicted.contains(id));
        }

        if let Some(token) = client_request_token {
            self.idempotency
                .insert((pipeline_name, token), execution_id.clone());
        }
        execution_id
    }

    /// Register `account_id` as the partner CodePipeline serves the action type `id`'s
    /// jobs to, and answer whether it was registered.
    ///
    /// Only a `ThirdParty` action type has a partner — an `AWS` action type is run by
    /// AWS and a `Custom` one by the account that created it — so any other owner is
    /// refused HERE, once, rather than by every reader.
    pub fn register_third_party_partner(&mut self, id: &ActionTypeId, account_id: &str) -> bool {
        if id.owner != ACTION_OWNER_THIRD_PARTY {
            return false;
        }
        self.third_party_partners
            .insert(ActionTypeKey::from_id(id), account_id.to_string());
        true
    }

    /// The account id registered as the partner for the action type `id`, when one is.
    ///
    /// `None` means there is no partner integration for it at all, which is the case
    /// for every action type in the assembled emulator.
    pub fn third_party_partner(&self, id: &ActionTypeId) -> Option<&str> {
        self.third_party_partners
            .get(&ActionTypeKey::from_id(id))
            .map(String::as_str)
    }

    /// Queue `job` as the newest third-party job waiting for the action type `id`, and
    /// answer whether it was queued: as with a partner registration, only a
    /// `ThirdParty` action type can have one.
    pub fn enqueue_third_party_job(&mut self, id: &ActionTypeId, job: ThirdPartyJobRecord) -> bool {
        if id.owner != ACTION_OWNER_THIRD_PARTY {
            return false;
        }
        self.third_party_jobs
            .entry(ActionTypeKey::from_id(id))
            .or_default()
            .push(job);
        true
    }

    /// The jobs waiting for the action type `id`, oldest first and at most `limit` of
    /// them, as `PollForThirdPartyJobs` reports them.
    ///
    /// A read: a polled job stays queued, because what claims a job is the
    /// `clientToken`/`nonce` pair a worker presents to `GetThirdPartyJobDetails` and
    /// `AcknowledgeThirdPartyJob`, not the poll that revealed it.
    pub fn third_party_jobs_for(&self, id: &ActionTypeId, limit: usize) -> Vec<ThirdPartyJob> {
        self.third_party_jobs
            .get(&ActionTypeKey::from_id(id))
            .into_iter()
            .flatten()
            .take(limit)
            .map(ThirdPartyJobRecord::to_job)
            .collect()
    }

    /// Forget a pipeline: the declaration, the executions still retained for it, and
    /// the idempotency tokens that named them.
    pub fn forget_pipeline(&mut self, name: &str) {
        self.pipelines.remove(name);
        for id in self.executions_by_pipeline.remove(name).unwrap_or_default() {
            self.executions.remove(&id);
        }
        self.idempotency
            .retain(|(pipeline_name, _), _| pipeline_name != name);
    }

    /// The third-party job `job_id`, for a worker that proved it holds the job's
    /// `clientToken`.
    ///
    /// A worker's call names a job by id alone, so the search spans every action
    /// type's queue: a job id identifies one job across the service, and which
    /// queue it waits in is not something the caller knows or states.
    ///
    /// The two refusals are ordered the way the lookup itself is: a job the service
    /// is not holding has no token to compare against, so an id that names no
    /// retained job is "not found" whatever token accompanies it, and only a job
    /// that WAS found can reject the token it was given. `ThirdPartyJobId` carries
    /// no modelled pattern, so the id's form is never what decides this — only
    /// whether a job is queued under it.
    pub fn third_party_job_mut(
        &mut self,
        job_id: &str,
        client_token: &str,
    ) -> Result<&mut ThirdPartyJobRecord, ThirdPartyJobProblem> {
        let job = self
            .third_party_jobs
            .values_mut()
            .flatten()
            .find(|job| job.job_id == job_id)
            .ok_or_else(|| {
                // The message AWS answers a missing job with, built from the
                // request: the id the caller asked for, and nothing else about the
                // store.
                ThirdPartyJobProblem::NotFound(format!("Job with id '{job_id}' does not exist."))
            })?;
        if job.client_token != client_token {
            return Err(ThirdPartyJobProblem::ClientToken(format!(
                "the clientToken given does not grant access to job {job_id}"
            )));
        }
        Ok(job)
    }

    /// Register `record` under `key`, answering whether the store took it.
    ///
    /// `false` means the account is already holding [`MAX_ACTION_TYPES_PER_ACCOUNT`]
    /// action types and this id is not one of them, so the registration must be
    /// refused with `LimitExceededException` rather than accepted — the same shape
    /// `CreatePipeline` answers at its own cap. Re-registering an id the store
    /// already holds REPLACES that record and is always accepted: it grows nothing.
    pub fn register_action_type(&mut self, key: ActionTypeKey, record: ActionTypeRecord) -> bool {
        if !self.action_types.contains_key(&key)
            && self.action_types.len() >= MAX_ACTION_TYPES_PER_ACCOUNT
        {
            return false;
        }
        self.action_types.insert(key, record);
        true
    }

    /// Forget the action type registered under `key`, answering whether one was
    /// registered. The release path the bound needs: without it a registration
    /// could never be given back.
    pub fn forget_action_type(&mut self, key: &ActionTypeKey) -> bool {
        self.action_types.remove(key).is_some()
    }

    /// The record registered under `key`, or `None` when this account registered no
    /// action type with that id. Reads the registrations only — the action types the
    /// service itself provides are in [`BUILT_IN_ACTION_TYPES`].
    pub fn registered_action_type(&self, key: &ActionTypeKey) -> Option<&ActionTypeRecord> {
        self.action_types.get(key)
    }

    /// How many action types this account has registered, of at most
    /// [`MAX_ACTION_TYPES_PER_ACCOUNT`].
    pub fn registered_action_type_count(&self) -> usize {
        self.action_types.len()
    }

    /// Every action type this account can use, in one total order over the
    /// `ActionTypeId` members: the catalog the service itself provides
    /// ([`BUILT_IN_ACTION_TYPES`]) overlaid with the action types registered in this
    /// store, so a registration of an id the catalog also names wins over it.
    ///
    /// One order over both is what lets a listing page through them with an opaque
    /// cursor that names a position rather than an offset.
    pub fn action_type_catalog(&self) -> BTreeMap<ActionTypeKey, ActionTypeRecord> {
        let mut catalog: BTreeMap<ActionTypeKey, ActionTypeRecord> = BUILT_IN_ACTION_TYPES
            .iter()
            .map(|entry| (entry.key(), entry.record()))
            .collect();
        for (key, record) in &self.action_types {
            catalog.insert(key.clone(), record.clone());
        }
        catalog
    }
}

/// Why a job worker's call naming a third-party job cannot be served, as
/// [`ComAmazonawsCodepipelineCodepipeline20150709State::third_party_job_mut`]
/// answers it.
///
/// Each variant names the modelled error shape that states it — every third-party
/// operation models both — so the whole worker-side surface refuses one condition in
/// exactly one way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThirdPartyJobProblem {
    /// Refused as `JobNotFoundException`: no job the service holds carries that id.
    NotFound(String),
    /// Refused as `InvalidClientTokenException`: the token is not the one the job
    /// was handed out with, so the caller is not the worker that holds it.
    ClientToken(String),
}

/// An action type's identity: the four `ActionTypeId` members, in the order the
/// shape declares them (`smithy_query query="shape" shape="ActionTypeId"` ->
/// `category`, `owner`, `provider`, `version`).
///
/// Ordered, so a map keyed by it pages deterministically, and the order a listing
/// reports is the same across calls.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ActionTypeKey {
    /// An `ActionCategory` value.
    pub category: String,
    /// An `ActionOwner` value.
    pub owner: String,
    /// The provider of the service the action calls.
    pub provider: String,
    /// The action type's version.
    pub version: String,
}

impl ActionTypeKey {
    /// The identity a request's `ActionTypeId` names.
    pub fn from_id(id: &ActionTypeId) -> Self {
        Self {
            category: id.category.clone(),
            owner: id.owner.clone(),
            provider: id.provider.clone(),
            version: id.version.clone(),
        }
    }

    /// This identity as the modelled `ActionTypeId` a response carries.
    pub fn to_id(&self) -> ActionTypeId {
        ActionTypeId {
            category: self.category.clone(),
            owner: self.owner.clone(),
            provider: self.provider.clone(),
            version: self.version.clone(),
        }
    }

    /// This identity as the ordered key a pagination cursor names.
    ///
    /// `/` separates the members because neither `ActionProvider` nor `Version` can
    /// contain one (both are `@pattern("^[0-9A-Za-z_-]+$")`) and `ActionCategory` and
    /// `ActionOwner` are enums of bare words, so the rendering is unambiguous and
    /// [`ActionTypeKey::from_cursor`] recovers exactly the four members.
    pub fn cursor(&self) -> String {
        format!(
            "{}/{}/{}/{}",
            self.category, self.owner, self.provider, self.version
        )
    }

    /// The identity a cursor key names, or `None` when it is not one
    /// [`ActionTypeKey::cursor`] produced.
    pub fn from_cursor(cursor: &str) -> Option<Self> {
        let mut parts = cursor.split('/');
        let key = Self {
            category: parts.next()?.to_string(),
            owner: parts.next()?.to_string(),
            provider: parts.next()?.to_string(),
            version: parts.next()?.to_string(),
        };
        if parts.next().is_some()
            || [&key.category, &key.owner, &key.provider, &key.version]
                .iter()
                .any(|member| member.is_empty())
        {
            return None;
        }
        Some(key)
    }
}

/// One action type as this service holds it: everything `ActionType` reports beside
/// its id (which is the key it is stored under), plus the Region it is available in.
#[derive(Debug, Clone, Default)]
pub struct ActionTypeRecord {
    /// URLs that tell a user about this action type, when it has any.
    pub settings: Option<ActionTypeSettings>,
    /// The configuration properties the action accepts.
    pub configuration_properties: Vec<ActionConfigurationProperty>,
    /// How many input artifacts the action takes.
    pub input_artifact_details: ArtifactDetails,
    /// How many output artifacts the action produces.
    pub output_artifact_details: ArtifactDetails,
    /// The Region this action type is registered in, or `None` for one the service
    /// offers in every Region. `ListActionTypes$regionFilter` ("The Region to filter
    /// on for the list of action types") is what reads it.
    pub region: Option<String>,
    /// The tags applied to this action type, as `CreateCustomActionType` stored
    /// them. Empty for an action type the service provides itself, which no caller
    /// tagged. `ActionType` carries no `tags` member, so only an operation that
    /// reports a resource's tags reads it: `CreateCustomActionType`'s own answer
    /// today, and `ListTagsForResource` once that stub is written.
    pub tags: Vec<Tag>,
}

impl ActionTypeRecord {
    /// Whether this action type is one a caller in `region` can use: a registration
    /// belongs to the Region it was made in, and an action type the service offers
    /// everywhere belongs to all of them.
    pub fn available_in(&self, region: &str) -> bool {
        self.region.as_deref().is_none_or(|own| own == region)
    }

    /// This action type as `ListActionTypes` and `CreateCustomActionType` report it.
    pub fn to_action_type(&self, key: &ActionTypeKey) -> ActionType {
        ActionType {
            id: key.to_id(),
            settings: self.settings.clone(),
            // An absent list and an empty one state the same fact, and AWS omits the
            // member.
            action_configuration_properties: optional_list(self.configuration_properties.clone()),
            input_artifact_details: self.input_artifact_details.clone(),
            output_artifact_details: self.output_artifact_details.clone(),
        }
    }
}

/// One entry of the catalog of action types this service provides itself: the four
/// `ActionTypeId` members and the artifact counts the action's reference page
/// documents.
#[derive(Debug, Clone, Copy)]
pub struct BuiltInActionType {
    /// An `ActionCategory` value.
    pub category: &'static str,
    /// An `ActionOwner` value — `AWS` for an action the service runs itself,
    /// `ThirdParty` for one a partner integration runs.
    pub owner: &'static str,
    /// The provider of the service the action calls.
    pub provider: &'static str,
    /// The action type's version.
    pub version: &'static str,
    /// `(minimumCount, maximumCount)` of the action's input artifacts.
    pub input_artifacts: (i32, i32),
    /// `(minimumCount, maximumCount)` of the action's output artifacts.
    pub output_artifacts: (i32, i32),
}

impl BuiltInActionType {
    /// This entry's identity.
    pub fn key(&self) -> ActionTypeKey {
        ActionTypeKey {
            category: self.category.to_string(),
            owner: self.owner.to_string(),
            provider: self.provider.to_string(),
            version: self.version.to_string(),
        }
    }

    /// This entry as a stored record: available in every Region, and carrying no
    /// `settings` or configuration properties, because those are per-provider detail
    /// this emulator does not model rather than values to invent.
    pub fn record(&self) -> ActionTypeRecord {
        ActionTypeRecord {
            settings: None,
            configuration_properties: Vec::new(),
            input_artifact_details: ArtifactDetails {
                minimum_count: self.input_artifacts.0,
                maximum_count: self.input_artifacts.1,
            },
            output_artifact_details: ArtifactDetails {
                minimum_count: self.output_artifacts.0,
                maximum_count: self.output_artifacts.1,
            },
            region: None,
            // Nobody tagged an action type the service provides itself.
            tags: Vec::new(),
        }
    }
}

/// The action types this service provides on its own — the ones no caller created —
/// as `ListActionTypes` ("a summary of all CodePipeline action types associated with
/// your account") reports them beside the account's own registrations.
///
/// Source: the page `ActionTypeId$provider` and `ActionTypeId$owner` both link to in
/// their model documentation, "Valid action types and providers in CodePipeline"
/// (<https://docs.aws.amazon.com/codepipeline/latest/userguide/reference-pipeline-structure.html#actions-valid-providers>),
/// which lists each category's providers and their owners; the artifact counts are
/// the input- and output-artifact counts that page's per-action structure reference
/// documents for each one. Every version is `1`, the only version those actions
/// publish. Providers whose documented input-artifact count exceeds
/// `MaximumArtifactCount`'s modelled `@range(max: 5)` are left out rather than
/// reported with a count the model cannot carry.
pub const BUILT_IN_ACTION_TYPES: [BuiltInActionType; 14] = [
    BuiltInActionType {
        category: "Approval",
        owner: "AWS",
        provider: "Manual",
        version: "1",
        input_artifacts: (0, 0),
        output_artifacts: (0, 0),
    },
    BuiltInActionType {
        category: "Build",
        owner: "AWS",
        provider: "CodeBuild",
        version: "1",
        input_artifacts: (1, 5),
        output_artifacts: (0, 5),
    },
    BuiltInActionType {
        category: "Deploy",
        owner: "AWS",
        provider: "CodeDeploy",
        version: "1",
        input_artifacts: (1, 1),
        output_artifacts: (0, 0),
    },
    BuiltInActionType {
        category: "Deploy",
        owner: "AWS",
        provider: "ECS",
        version: "1",
        input_artifacts: (1, 1),
        output_artifacts: (0, 0),
    },
    BuiltInActionType {
        category: "Deploy",
        owner: "AWS",
        provider: "ElasticBeanstalk",
        version: "1",
        input_artifacts: (1, 1),
        output_artifacts: (0, 0),
    },
    BuiltInActionType {
        category: "Deploy",
        owner: "AWS",
        provider: "S3",
        version: "1",
        input_artifacts: (1, 1),
        output_artifacts: (0, 0),
    },
    BuiltInActionType {
        category: "Invoke",
        owner: "AWS",
        provider: "Lambda",
        version: "1",
        input_artifacts: (0, 5),
        output_artifacts: (0, 5),
    },
    BuiltInActionType {
        category: "Source",
        owner: "AWS",
        provider: "CodeCommit",
        version: "1",
        input_artifacts: (0, 0),
        output_artifacts: (1, 1),
    },
    BuiltInActionType {
        category: "Source",
        owner: "AWS",
        provider: "CodeStarSourceConnection",
        version: "1",
        input_artifacts: (0, 0),
        output_artifacts: (1, 1),
    },
    BuiltInActionType {
        category: "Source",
        owner: "AWS",
        provider: "ECR",
        version: "1",
        input_artifacts: (0, 0),
        output_artifacts: (1, 1),
    },
    BuiltInActionType {
        category: "Source",
        owner: "AWS",
        provider: "S3",
        version: "1",
        input_artifacts: (0, 0),
        output_artifacts: (1, 1),
    },
    BuiltInActionType {
        category: "Source",
        owner: "ThirdParty",
        provider: "GitHub",
        version: "1",
        input_artifacts: (0, 0),
        output_artifacts: (1, 1),
    },
    BuiltInActionType {
        category: "Test",
        owner: "AWS",
        provider: "CodeBuild",
        version: "1",
        input_artifacts: (1, 5),
        output_artifacts: (0, 5),
    },
    BuiltInActionType {
        category: "Test",
        owner: "AWS",
        provider: "DeviceFarm",
        version: "1",
        input_artifacts: (1, 1),
        output_artifacts: (0, 0),
    },
];

/// One stored pipeline: the declaration exactly as the caller sent it (with the
/// members the service itself resolves filled in), plus the service-owned metadata
/// `GetPipeline` reports and every version of the declaration.
#[derive(Debug, Clone)]
pub struct Pipeline {
    /// The current declaration, as stored.
    pub declaration: PipelineDeclaration,
    /// The current version number.
    pub version: i32,
    /// This pipeline's ARN.
    pub arn: String,
    /// When the pipeline was created.
    pub created: SystemTime,
    /// When the pipeline was last updated.
    pub updated: SystemTime,
    /// The tags applied to the pipeline.
    pub tags: Vec<Tag>,
    /// Every version of the declaration, keyed by version number.
    pub versions: BTreeMap<i32, PipelineDeclaration>,
    /// The transitions a caller has changed the state of, keyed by
    /// `(stage name, StageTransitionType)`. A transition with no entry has never
    /// been changed, which is the enabled state — so the map holds exactly the
    /// changes `DisableStageTransition` / `EnableStageTransition` persisted.
    pub transitions: BTreeMap<(String, String), StageTransition>,
}

/// The state of ONE stage transition, as the call that last changed it left it.
#[derive(Debug, Clone)]
pub struct StageTransition {
    /// Whether artifacts may cross this transition.
    pub enabled: bool,
    /// The `reason` the disabling call gave; absent on an enabled transition,
    /// which no caller gave a reason for.
    pub disabled_reason: Option<String>,
    /// When the transition's state was last changed.
    pub last_changed_at: SystemTime,
}

impl Pipeline {
    /// Store `declaration` as a brand-new pipeline at version
    /// [`FIRST_PIPELINE_VERSION`].
    ///
    /// The members the service resolves rather than the caller — `version`,
    /// `pipelineType`, `executionMode` — are filled in before the declaration is
    /// stored, so a later `GetPipeline` reports the same declaration the create
    /// returned.
    pub fn create(
        declaration: PipelineDeclaration,
        arn: String,
        now: SystemTime,
        tags: Vec<Tag>,
    ) -> Self {
        let pipeline_type = resolved_pipeline_type(&declaration);
        let execution_mode = resolved_execution_mode(&declaration);
        let mut declaration = declaration;
        declaration.version = Some(FIRST_PIPELINE_VERSION);
        declaration.pipeline_type = Some(pipeline_type);
        declaration.execution_mode = Some(execution_mode);
        let mut versions = BTreeMap::new();
        versions.insert(FIRST_PIPELINE_VERSION, declaration.clone());
        Self {
            declaration,
            version: FIRST_PIPELINE_VERSION,
            arn,
            created: now,
            updated: now,
            tags,
            versions,
            // A new pipeline has changed no transition, so every one of them is
            // enabled.
            transitions: BTreeMap::new(),
        }
    }

    /// Replace this pipeline's structure with `declaration` and return the
    /// declaration as stored.
    ///
    /// `UpdatePipeline` documents "Use a JSON file with the pipeline structure and
    /// UpdatePipeline to provide the full structure of the pipeline. Updating the
    /// pipeline increases the version number of the pipeline by 1" — so the stored
    /// declaration is REPLACED rather than merged, the version is the previous one
    /// plus 1 (whatever `version` the caller's declaration carried, which may be a
    /// stale one read back from `GetPipeline`), and the new declaration is kept
    /// beside the earlier versions so a `GetPipeline` naming one of them still reads
    /// it. `created` is untouched and `updated` moves to `now`.
    ///
    /// A stage the replacement structure no longer declares also takes its persisted
    /// `transitions` record with it: a transition belongs to a declared stage, so a
    /// later structure reusing that stage name must not inherit an enablement a call
    /// against a different structure left behind.
    ///
    /// The members the service resolves rather than the caller — `version`,
    /// `pipelineType`, `executionMode` — are resolved from the declaration the same
    /// way a create resolves them, which is what `PipelineDeclaration$pipelineType`
    /// documents ("Including V2 parameters ... when creating or updating a pipeline
    /// will result in the pipeline having the V2 type of pipeline").
    pub fn update(
        &mut self,
        declaration: PipelineDeclaration,
        now: SystemTime,
    ) -> PipelineDeclaration {
        let pipeline_type = resolved_pipeline_type(&declaration);
        let execution_mode = resolved_execution_mode(&declaration);
        // A saturating step keeps `version` inside `PipelineVersion`'s modelled
        // range instead of wrapping; reaching `i32::MAX` updates is not a path a
        // caller can walk, so the saturation is a safety net, not a behaviour.
        let version = self.version.saturating_add(1);
        let mut declaration = declaration;
        declaration.version = Some(version);
        declaration.pipeline_type = Some(pipeline_type);
        declaration.execution_mode = Some(execution_mode);

        self.versions.insert(version, declaration.clone());
        // Oldest first: `versions` is keyed by the ascending version number.
        while self.versions.len() > MAX_PIPELINE_VERSIONS_RETAINED {
            self.versions.pop_first();
        }

        self.declaration = declaration;
        self.version = version;
        self.updated = now;

        // A persisted transition belongs to a stage the pipeline DECLARES, so a
        // stage the replacement structure no longer names takes its transition
        // record with it: otherwise a later structure that reuses that stage name
        // would inherit an `enabled`/`disabledReason` a call against a different
        // structure left behind, and the first stage — which has no inbound edge —
        // could report an inbound transition state.
        let declared: BTreeSet<String> = self
            .declaration
            .stages
            .iter()
            .map(|stage| stage.name.clone())
            .collect();
        self.transitions
            .retain(|(stage_name, _), _| declared.contains(stage_name));

        self.declaration.clone()
    }

    /// Whether this pipeline declares a stage named `stage_name`.
    pub fn declares_stage(&self, stage_name: &str) -> bool {
        self.declaration
            .stages
            .iter()
            .any(|stage| stage.name == stage_name)
    }

    /// Record that `transition_type` of `stage_name` is now enabled or disabled,
    /// with the reason the disabling caller gave, changed at `now`.
    ///
    /// One writer for both `DisableStageTransition` and `EnableStageTransition`, so
    /// the two cannot disagree about where the state lives: the later call replaces
    /// the earlier one's record for that transition and leaves every other
    /// transition of the pipeline alone.
    pub fn set_transition(
        &mut self,
        stage_name: &str,
        transition_type: &str,
        enabled: bool,
        disabled_reason: Option<String>,
        now: SystemTime,
    ) {
        self.transitions.insert(
            (stage_name.to_string(), transition_type.to_string()),
            StageTransition {
                enabled,
                disabled_reason,
                last_changed_at: now,
            },
        );
    }

    /// The `TransitionState` of `transition_type` of `stage_name`, as stored.
    ///
    /// A transition no call has changed is enabled and carries neither a change
    /// instant nor a reason, so the members stay absent rather than carrying an
    /// invented instant.
    pub fn transition_state(&self, stage_name: &str, transition_type: &str) -> TransitionState {
        match self
            .transitions
            .get(&(stage_name.to_string(), transition_type.to_string()))
        {
            Some(transition) => TransitionState {
                enabled: transition.enabled,
                // `TransitionState$lastChangedBy` carries the ID of the user who
                // changed the transition; this emulator does not resolve a caller
                // identity, so the member is absent rather than invented.
                last_changed_by: None,
                last_changed_at: Some(to_date_time(transition.last_changed_at)),
                disabled_reason: transition.disabled_reason.clone(),
            },
            None => TransitionState {
                enabled: true,
                last_changed_by: None,
                last_changed_at: None,
                disabled_reason: None,
            },
        }
    }

    /// This pipeline's `StageState` list, as `GetPipelineState` reports it, in the
    /// order the declaration lists the stages.
    ///
    /// Every member is read from stored state: the stage and action names from the
    /// stored declaration, the inbound transition state from what the transition
    /// calls persisted. The members the stage- and action-execution plane owns
    /// (`latestExecution`, `inboundExecution(s)`, an action's revision and
    /// execution, the condition states) are absent, because this slice runs no such
    /// plane and so has no outcome to report for them.
    pub fn stage_states(&self) -> Vec<StageState> {
        self.declaration
            .stages
            .iter()
            .enumerate()
            .map(|(index, stage)| StageState {
                stage_name: Some(stage.name.clone()),
                inbound_execution: None,
                inbound_executions: None,
                inbound_transition_state: self.inbound_transition_state(index, &stage.name),
                action_states: Some(
                    stage
                        .actions
                        .iter()
                        .map(|action| ActionState {
                            action_name: Some(action.name.clone()),
                            current_revision: None,
                            latest_execution: None,
                            entity_url: None,
                            revision_url: None,
                        })
                        .collect(),
                ),
                latest_execution: None,
                before_entry_condition_state: None,
                on_success_condition_state: None,
                on_failure_condition_state: None,
                retry_stage_metadata: None,
            })
            .collect()
    }

    /// The inbound `TransitionState` to report for the stage at `index`.
    ///
    /// An inbound transition is the edge artifacts cross INTO a stage from the stage
    /// before it, so the first stage has none to report — unless a caller has
    /// changed one for it, in which case the state that call persisted is what the
    /// reader reports rather than silently dropping it.
    fn inbound_transition_state(&self, index: usize, stage_name: &str) -> Option<TransitionState> {
        let changed = self
            .transitions
            .contains_key(&(stage_name.to_string(), TRANSITION_TYPE_INBOUND.to_string()));
        (index > 0 || changed).then(|| self.transition_state(stage_name, TRANSITION_TYPE_INBOUND))
    }

    /// The declaration at `version`, or the current one when `version` is absent.
    /// `None` means that version was never created.
    pub fn declaration_at(&self, version: Option<i32>) -> Option<&PipelineDeclaration> {
        match version {
            None => Some(&self.declaration),
            Some(version) => self.versions.get(&version),
        }
    }

    /// This pipeline's `PipelineMetadata`, as `GetPipeline` reports it.
    pub fn metadata(&self) -> PipelineMetadata {
        PipelineMetadata {
            pipeline_arn: Some(self.arn.clone()),
            created: Some(to_date_time(self.created)),
            updated: Some(to_date_time(self.updated)),
            // Polling is never disabled in this emulator, so the member is absent
            // rather than carrying an invented instant.
            polling_disabled_at: None,
        }
    }

    /// This pipeline's `PipelineSummary`, as `ListPipelines` reports it.
    pub fn summary(&self) -> PipelineSummary {
        PipelineSummary {
            name: Some(self.declaration.name.clone()),
            version: Some(self.version),
            pipeline_type: self.declaration.pipeline_type.clone(),
            execution_mode: self.declaration.execution_mode.clone(),
            created: Some(to_date_time(self.created)),
            updated: Some(to_date_time(self.updated)),
        }
    }
}

/// A tag set this service refuses, and the modelled error shape that states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagProblem {
    /// Refused as `TooManyTagsException`: the request carries more tags than a
    /// resource may hold.
    TooMany(String),
    /// Refused as `InvalidTagsException`: the tag set itself is not well formed.
    Invalid(String),
}

impl TagProblem {
    /// Why the tags were refused. Built from the request, so the caller can fix the
    /// request from this text alone.
    pub fn message(&self) -> &str {
        match self {
            Self::TooMany(message) | Self::Invalid(message) => message,
        }
    }
}

/// Refuse a tag set no resource of this service can carry.
///
/// Neither rule is expressible as a Smithy constraint (`TagList` carries no length
/// bound and a list cannot require distinct keys):
///
/// * at most [`MAX_TAGS_PER_RESOURCE`] tags on one resource;
/// * one key at most once — a map from key to value cannot hold a key twice, so a
///   list naming one key twice does not describe a tag set.
pub fn validate_tags(tags: &[Tag]) -> Result<(), TagProblem> {
    if tags.len() > MAX_TAGS_PER_RESOURCE {
        return Err(TagProblem::TooMany(format!(
            "tags must hold at most {MAX_TAGS_PER_RESOURCE} entries, but this request carries {}",
            tags.len()
        )));
    }
    let mut seen = BTreeSet::new();
    for tag in tags {
        if !seen.insert(tag.key.as_str()) {
            return Err(TagProblem::Invalid(format!(
                "tags names the key {} more than once",
                tag.key
            )));
        }
    }
    Ok(())
}

/// Refuse an action type's `configurationProperties` list that the service's own
/// documented rules rule out, reporting why. `Ok(())` means the list is storable.
///
/// Neither rule is a Smithy constraint, and they differ in how directly the model
/// states them:
///
/// * STATED by `ActionConfigurationProperty$queryable`: "When creating a custom
///   action, an action can have up to one queryable property. If it has one, that
///   property must be both required and not secret."
/// * INFERRED from `CreateCustomActionTypeInput$configurationProperties`: "You can
///   refer to a name in the configuration properties of the custom action within the
///   URL templates by following the format of {Config:name}" — a reference by name
///   only resolves when each name appears once, so a list naming one twice does not
///   describe a usable action type. The model does not say this in so many words.
pub fn validate_configuration_properties(
    properties: &[ActionConfigurationProperty],
) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for property in properties {
        if !seen.insert(property.name.as_str()) {
            return Err(format!(
                "configurationProperties names the property {} more than once",
                property.name
            ));
        }
    }
    let queryable: Vec<&ActionConfigurationProperty> = properties
        .iter()
        .filter(|property| property.queryable)
        .collect();
    if queryable.len() > 1 {
        return Err(format!(
            "configurationProperties may declare at most one queryable property, but {} are queryable",
            queryable.len()
        ));
    }
    if let Some(property) = queryable.first() {
        if !property.required || property.secret {
            return Err(format!(
                "the queryable configuration property {} must be required and not secret",
                property.name
            ));
        }
    }
    Ok(())
}

/// What the service states when the caller is not whitelisted for an action type: the
/// four identifying fields of the action type the REQUEST asked about, in the order
/// `ActionTypeId` declares them, each quoted.
///
/// A partner integration is a WHITELIST of the accounts an action type's jobs are
/// served to, and the refusal is built from the request, so this is a template over
/// the request's own values — never text naming one particular action type.
pub fn not_whitelisted_message(id: &ActionTypeId) -> String {
    format!(
        "Not whitelisted for accessing ActionType.Category='{}' ActionType.Owner='{}' \
         ActionType.Provider='{}' ActionType.Version='{}'",
        id.category, id.owner, id.provider, id.version
    )
}

/// One third-party job waiting for a partner's job worker.
#[derive(Debug, Clone)]
pub struct ThirdPartyJobRecord {
    /// The job's id in CodePipeline, matching `ThirdPartyJob$jobId`'s modelled pattern
    /// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`).
    pub job_id: String,
    /// The `clientId` a worker is given for the job, matching
    /// `ThirdPartyJob$clientId`'s modelled pattern (the same uuid form). It is the
    /// first half of the `clientId`/`clientToken` pair the model documents: the
    /// worker presents the matching `clientToken` to `GetThirdPartyJobDetails` and
    /// `AcknowledgeThirdPartyJob`.
    pub client_id: String,
    /// The `clientToken` half of that pair: the token a worker's call has to carry
    /// to reach this job, as every third-party operation's `clientToken` member
    /// documents ("used to verify that the calling entity is allowed access to the
    /// job and its details"). `ClientToken` carries no modelled pattern, so this is
    /// an opaque value minted with the job.
    pub client_token: String,
    /// A `JobStatus` value: where the job is between being queued and a worker
    /// reporting its outcome.
    pub status: String,
    /// The continuation token the last successful result supplied, identifying a
    /// partner action still in progress. `PutThirdPartyJobSuccessResult$continuationToken`
    /// documents that "future jobs use this token to identify the running instance
    /// of the action", and that "when the action is complete, no continuation token
    /// should be supplied" — so an absent token here means the action finished with
    /// this job.
    pub continuation_token: Option<String>,
    /// The revision the worker reported as current for the artifact it worked on.
    pub current_revision: Option<CurrentRevision>,
    /// The execution details the worker reported with its result.
    pub execution_details: Option<ExecutionDetails>,
    /// When this job's status last changed.
    pub last_status_change: SystemTime,
}

impl ThirdPartyJobRecord {
    /// A record of the job identified by `job_id`, to be served to the worker holding
    /// `client_id` — what the action-execution plane queues when an execution reaches
    /// a partner action.
    ///
    /// The job starts at [`JOB_STATUS_QUEUED`]: it is waiting for a worker, and no
    /// worker owns its outcome until one acknowledges it. The `clientToken` half of
    /// the pair is minted here, so a queued job always has the one token that
    /// reaches it.
    pub fn new(job_id: impl Into<String>, client_id: impl Into<String>) -> Self {
        Self {
            job_id: job_id.into(),
            client_id: client_id.into(),
            client_token: random_uuid_v4(),
            status: JOB_STATUS_QUEUED.to_string(),
            continuation_token: None,
            current_revision: None,
            execution_details: None,
            last_status_change: SystemTime::now(),
        }
    }

    /// This job as `PollForThirdPartyJobs` reports it.
    pub fn to_job(&self) -> ThirdPartyJob {
        ThirdPartyJob {
            client_id: Some(self.client_id.clone()),
            job_id: Some(self.job_id.clone()),
        }
    }

    /// Whether a job worker may still report a result for this job.
    ///
    /// Only a job the worker has acknowledged and not yet completed: a job still
    /// being handed out (`Created`, `Queued`, `Dispatched`) has no worker that owns
    /// its outcome, and one that already reached a terminal status (`Succeeded`,
    /// `Failed`, `TimedOut`) has an outcome that cannot be replaced.
    pub fn accepts_result(&self) -> bool {
        self.status == JOB_STATUS_IN_PROGRESS
    }

    /// Record a worker's successful result: the job reaches
    /// [`JOB_STATUS_SUCCEEDED`] and keeps what the worker reported with it.
    pub fn record_success(
        &mut self,
        continuation_token: Option<String>,
        current_revision: Option<CurrentRevision>,
        execution_details: Option<ExecutionDetails>,
        now: SystemTime,
    ) {
        self.status = JOB_STATUS_SUCCEEDED.to_string();
        self.continuation_token = continuation_token;
        self.current_revision = current_revision;
        self.execution_details = execution_details;
        self.last_status_change = now;
    }
}

/// One stored pipeline execution.
#[derive(Debug, Clone)]
pub struct PipelineExecutionRecord {
    /// The generated id, matching `PipelineExecutionId`'s modelled pattern.
    pub pipeline_execution_id: String,
    /// The pipeline this execution belongs to.
    pub pipeline_name: String,
    /// The pipeline version this execution runs.
    pub pipeline_version: i32,
    /// A `PipelineExecutionStatus` value.
    pub status: String,
    /// A description of the status, when the service has one.
    pub status_summary: Option<String>,
    /// When the execution started.
    pub start_time: SystemTime,
    /// When the execution record last changed.
    pub last_update_time: SystemTime,
    /// What started the execution.
    pub trigger: ExecutionTrigger,
    /// An `ExecutionMode` value, inherited from the pipeline declaration.
    pub execution_mode: String,
    /// An `ExecutionType` value.
    pub execution_type: String,
    /// The pipeline variables resolved for this execution.
    pub variables: Vec<ResolvedPipelineVariable>,
    /// The source revisions this execution was started with.
    pub source_revisions: Vec<SourceRevision>,
}

impl PipelineExecutionRecord {
    /// This execution as `GetPipelineExecution` reports it.
    pub fn to_execution(&self) -> PipelineExecution {
        PipelineExecution {
            pipeline_name: Some(self.pipeline_name.clone()),
            pipeline_version: Some(self.pipeline_version),
            pipeline_execution_id: Some(self.pipeline_execution_id.clone()),
            status: Some(self.status.clone()),
            status_summary: self.status_summary.clone(),
            // Artifact revisions are recorded by the action-execution plane, which
            // this slice does not run, so none are reported rather than invented.
            artifact_revisions: None,
            variables: optional_list(self.variables.clone()),
            trigger: Some(self.trigger.clone()),
            execution_mode: Some(self.execution_mode.clone()),
            execution_type: Some(self.execution_type.clone()),
            // Only a rollback execution carries rollback metadata.
            rollback_metadata: None,
        }
    }

    /// This execution as `ListPipelineExecutions` reports it.
    pub fn to_summary(&self) -> PipelineExecutionSummary {
        PipelineExecutionSummary {
            pipeline_execution_id: Some(self.pipeline_execution_id.clone()),
            status: Some(self.status.clone()),
            status_summary: self.status_summary.clone(),
            start_time: Some(to_date_time(self.start_time)),
            last_update_time: Some(to_date_time(self.last_update_time)),
            source_revisions: optional_list(self.source_revisions.clone()),
            trigger: Some(self.trigger.clone()),
            // Set only by StopPipelineExecution, which this slice does not serve.
            stop_trigger: None,
            execution_mode: Some(self.execution_mode.clone()),
            execution_type: Some(self.execution_type.clone()),
            rollback_metadata: None,
        }
    }
}

/// `Some(list)` unless the list is empty, in which case the member is absent —
/// an absent list and an empty one are the same fact, and AWS omits it.
fn optional_list<T>(list: Vec<T>) -> Option<Vec<T>> {
    (!list.is_empty()).then_some(list)
}

/// A fresh version-4 UUID, rendered the way `PipelineExecutionId`'s modelled pattern
/// requires (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`).
///
/// The bits come from `RandomState`, the standard library's OS-seeded hash-seed
/// source: each `RandomState::new()` carries its own keys, so hashing through two of
/// them yields 128 bits that no caller can predict or repeat, with no random-number
/// dependency. The version and variant nibbles are then set as RFC 4122 requires, so
/// the value is a well-formed v4 UUID rather than an arbitrary hex string. An
/// execution id names an execution for correlation and is not a credential, so a
/// non-cryptographic source is the right one.
fn random_uuid_v4() -> String {
    use ::std::collections::hash_map::RandomState;
    use ::std::fmt::Write as _;
    use ::std::hash::BuildHasher as _;

    let mut bytes = [0u8; 16];
    for (half, chunk) in bytes.chunks_mut(8).enumerate() {
        let value = RandomState::new().hash_one(half as u64);
        chunk.copy_from_slice(&value.to_ne_bytes());
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let mut rendered = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            rendered.push('-');
        }
        let _ = write!(rendered, "{byte:02x}");
    }
    rendered
}

/// A `SystemTime` as the wire timestamp type every modelled member uses.
pub fn to_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    // A `u64` second count past `i64::MAX` is not a reachable instant (it is some
    // 2.9e11 years away) and `as` would wrap it into a negative timestamp, so the
    // conversion saturates instead: every representable instant converts exactly.
    let secs = i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX);
    ::aws_smithy_types::DateTime::from_secs_and_nanos(secs, since_epoch.subsec_nanos())
}

/// The region to mint a resource's ARN in: the one the request's SigV4 credential
/// scope names, else [`DEFAULT_REGION`].
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The `pipelineType` a declaration resolves to.
///
/// An explicit `pipelineType` is kept. Otherwise `PipelineDeclaration$pipelineType`
/// documents the rule: "Including V2 parameters, such as triggers on Git tags, in
/// the pipeline JSON when creating or updating a pipeline will result in the
/// pipeline having the V2 type of pipeline" — so a declaration carrying a V2-only
/// parameter (`variables` or `triggers`) resolves to `V2`, and one carrying neither
/// to `V1`.
pub fn resolved_pipeline_type(declaration: &PipelineDeclaration) -> String {
    if let Some(declared) = declaration
        .pipeline_type
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        return declared.to_string();
    }
    let carries_v2_parameters = declaration
        .variables
        .as_ref()
        .is_some_and(|variables| !variables.is_empty())
        || declaration
            .triggers
            .as_ref()
            .is_some_and(|triggers| !triggers.is_empty());
    if carries_v2_parameters {
        PIPELINE_TYPES[1].to_string()
    } else {
        PIPELINE_TYPES[0].to_string()
    }
}

/// The `executionMode` a declaration resolves to: the one it states, else
/// [`DEFAULT_EXECUTION_MODE`].
pub fn resolved_execution_mode(declaration: &PipelineDeclaration) -> String {
    declaration
        .execution_mode
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_EXECUTION_MODE)
        .to_string()
}

/// A structural defect in a `PipelineDeclaration` that the model's own constraints
/// cannot express, so it has to be refused by the handler.
///
/// Each variant names the modelled error shape that states it, so every caller
/// refuses one defect in exactly one way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclarationProblem {
    /// Refused as `InvalidStructureException`: the pipeline as a whole is malformed.
    Structure(String),
    /// Refused as `InvalidStageDeclarationException`: one stage is malformed.
    Stage(String),
}

impl DeclarationProblem {
    /// Why the declaration was refused. Built from the request, so the caller can
    /// fix the declaration from this text alone.
    pub fn message(&self) -> &str {
        match self {
            Self::Structure(message) | Self::Stage(message) => message,
        }
    }
}

/// Refuse a `PipelineDeclaration` whose structure cannot describe a pipeline.
///
/// Two rules, both documented and neither expressible as a Smithy constraint:
///
/// * `CreatePipeline` documents "you must include either `artifactStore` or
///   `artifactStores` in your pipeline, but you cannot use both".
/// * `stages` and `StageDeclaration$actions` are `@required`, which only makes the
///   members mandatory — an EMPTY list satisfies the constraint, yet a pipeline
///   with no stage, or a stage with no action, has nothing to run.
pub fn validate_declaration(
    declaration: &PipelineDeclaration,
) -> Result<(), DeclarationProblem> {
    let has_store = declaration.artifact_store.is_some();
    let has_stores = declaration
        .artifact_stores
        .as_ref()
        .is_some_and(|stores| !stores.is_empty());
    if has_store && has_stores {
        return Err(DeclarationProblem::Structure(
            "pipeline must include either artifactStore or artifactStores, but not both"
                .to_string(),
        ));
    }
    if !has_store && !has_stores {
        return Err(DeclarationProblem::Structure(
            "pipeline must include either artifactStore or artifactStores".to_string(),
        ));
    }
    if declaration.stages.is_empty() {
        return Err(DeclarationProblem::Structure(
            "pipeline must include at least one stage in stages".to_string(),
        ));
    }
    if let Some(stage) = declaration
        .stages
        .iter()
        .find(|stage| stage.actions.is_empty())
    {
        return Err(DeclarationProblem::Stage(format!(
            "stage {} must include at least one action in actions",
            stage.name
        )));
    }
    Ok(())
}

/// The cursor kind a `ListPipelines` token carries.
pub const CURSOR_PIPELINE: &str = "pipeline";

/// The cursor kind a `ListPipelineExecutions` token carries.
pub const CURSOR_EXECUTION: &str = "execution";

/// The cursor kind a `ListActionTypes` token carries.
pub const CURSOR_ACTION_TYPE: &str = "actionType";

/// An opaque pagination cursor naming the ordered key the next page starts at.
///
/// Base64 of `<kind>:<key>` (the encoding AWS's own opaque tokens use, from
/// `aws_smithy_types::base64`): opaque to the caller, round-trips exactly, and stays
/// inside `NextToken`'s modelled 1..2048 length for every key this service pages
/// over (a pipeline name is at most 100 characters, an execution id 36, and an
/// action type's four `ActionTypeId` members at most 8 + 10 + 35 + 9). The kind
/// prefix is what lets [`decode_page_token`] reject a token this listing never
/// issued; neither a `PipelineName` nor a `PipelineExecutionId` can contain `:`,
/// so the prefix is unambiguous.
///
/// Every listing applies ONE rule to a cursor: the page resumes at the first item
/// still in the listing's order at or after the key, and the token is refused with
/// `InvalidNextTokenException` only when it is not a cursor that listing issued or
/// no such position exists. They differ only in what their order makes possible:
/// `ListPipelines` and `ListActionTypes` are ordered BY the key, so an item deleted
/// between pages still has a position (the next key) and paging continues;
/// `ListPipelineExecutions` is ordered by when each execution started, so an id that
/// is no longer retained has no position at all and its cursor is refused.
pub fn encode_page_token(kind: &str, key: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{kind}:{key}"))
}

/// The ordered key an opaque cursor of `kind` names, or `None` when the cursor was
/// not one this listing issued.
pub fn decode_page_token(kind: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()?;
    let (decoded_kind, key) = decoded.split_once(':')?;
    (decoded_kind == kind).then(|| key.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCodepipelineCodepipeline20150709State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCodepipelineCodepipeline20150709State>)
}

/// The store, or the refusal a handler answers when this service was mounted on an
/// engine whose state it cannot share.
///
/// Unreachable in the assembled emulator — `core/router` mounts every service on the
/// one `InMemoryEngine` — and every error this service's operations model is a client
/// 400, so the impossible case is reported as a refusal rather than as an invented
/// 5xx or as a 501 (which would claim the operation is unwritten).
pub fn require_state<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
) -> Result<
    ::engine::ServiceState<ComAmazonawsCodepipelineCodepipeline20150709State>,
    crate::scaffolded::Scaffolded<E>,
> {
    service_state(engine).ok_or_else(|| {
        crate::scaffolded::Scaffolded::validation(
            "this service is not mounted on the emulator's shared in-memory engine",
        )
    })
}
