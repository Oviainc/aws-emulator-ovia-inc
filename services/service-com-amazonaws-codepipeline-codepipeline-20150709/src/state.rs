// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codepipeline-codepipeline-20150709 service,
//! plus the small helpers every handler of this crate shares.
//!
//! The pipeline definition lifecycle (`CreatePipeline` / `GetPipeline` /
//! `ListPipelines` / `DeletePipeline`), the tag set each pipeline carries
//! (`TagResource` / `ListTagsForResource`), the custom action types
//! `CreateCustomActionType` mints and `ListActionTypes` reports, the execution
//! records `StartPipelineExecution` mints, the stage-transition states
//! `DisableStageTransition` / `EnableStageTransition` change and the webhooks
//! `PutWebhook` defines all live in the one store reached through [`service_state`],
//! so every handler computes its answer from persisted state rather than from the
//! request it is answering.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    ActionConfiguration, ActionConfigurationProperty, ActionContext, ActionDeclaration,
    ActionExecutionDetail, ActionExecutionInput, ActionRevision, ActionType, ActionTypeId,
    ActionTypeSettings, Artifact, ArtifactDetail, ArtifactDetails, ArtifactLocation, Condition,
    ConditionState, CurrentRevision, EncryptionKey, ExecutionDetails, ExecutionTrigger, Job,
    JobData, ListWebhookItem, PipelineContext, PipelineDeclaration, PipelineExecution,
    PipelineExecutionSummary, PipelineMetadata, PipelineSummary, PipelineVariable,
    ResolvedPipelineVariable, RuleState, S3ArtifactLocation, SourceRevision, StageConditionState,
    StageContext, Tag, ThirdPartyJob, TransitionState, WebhookDefinition,
};

/// The execution plane's data model: what an execution's stages and actions are.
pub mod execution;
/// The driver that moves an execution forward and performs its artifact I/O.
pub mod driver;

pub use driver::advance;
pub use execution::{
    artifact_store, is_terminal_status, plan_stages, stage_states, ActionRun, ArtifactRef,
    StageRun, StopRequest, MAX_ARCHIVE_ENTRIES, MAX_CONCURRENT_EXECUTIONS_PER_PIPELINE,
    MAX_EXTRACTED_BYTES, STATUS_FAILED, STATUS_STOPPED, STATUS_SUCCEEDED, STATUS_SUPERSEDED,
    TRANSITION_INBOUND, TRANSITION_OUTBOUND,
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

/// The `PipelineExecutionStatus` value an execution carries while it is being
/// stopped: it has been asked to stop but has not reached a terminal status yet.
pub const STATUS_STOPPING: &str = "Stopping";

/// The `PipelineExecutionStatus` values an execution can still leave, so an
/// execution carrying one of them still occupies one of its pipeline's concurrent
/// execution slots. Every other value the model declares (`Cancelled`, `Stopped`,
/// `Succeeded`, `Superseded`, `Failed`) is terminal.
pub const NON_TERMINAL_STATUSES: [&str; 2] = [STATUS_IN_PROGRESS, STATUS_STOPPING];

/// The `ActionExecutionStatus` value an action execution carries from the moment the
/// execution enters its stage until the action reports a result, from the model's enum
/// (`smithy_query query="shape" shape="ActionExecutionStatus"` -> `InProgress`,
/// `Abandoned`, `Succeeded`, `Failed`).
///
/// It is the only status an action execution reaches in this emulator: nothing here
/// RUNS an action — no source is fetched, no build or deployment is performed, and
/// every operation a job worker would report a result through
/// (`PutJobSuccessResult`, `PutJobFailureResult`, `PutApprovalResult`) is still a
/// scaffold with its own issue — so an action that has started stays started rather
/// than being moved to an outcome nothing produced. This mirrors the pipeline
/// execution that contains it, which stays [`STATUS_IN_PROGRESS`] for the same
/// reason.
pub const ACTION_STATUS_IN_PROGRESS: &str = "InProgress";

/// The `runOrder` an action runs in when its declaration names none.
///
/// `ActionDeclaration$runOrder` is "the order in which actions are run" and is
/// modelled `ActionRunOrder`, `@range(min: 1, max: 999)`, so the first order a stage
/// can run anything in is 1 — which is what an action that names no order runs in,
/// and what AWS's action-structure reference documents as the default.
pub const DEFAULT_RUN_ORDER: i32 = 1;

/// The `StartTimeRange` values the model declares
/// (`smithy_query query="shape" shape="StartTimeRange"` -> `Latest`, `All`), in the
/// order the enum lists them.
pub const START_TIME_RANGES: [&str; 2] = ["Latest", "All"];

/// The `StartTimeRange` value selecting only the most recent action execution of each
/// action in the pipeline execution the filter names, rather than every one of them.
pub const START_TIME_RANGE_LATEST: &str = START_TIME_RANGES[0];

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

/// The `JobStatus` value a job carries once a job worker has reported failure for
/// it, from the same enum. Terminal, like [`JOB_STATUS_SUCCEEDED`].
pub const JOB_STATUS_FAILED: &str = "Failed";

/// The `JobStatus` value a job carries once its worker's window to report an outcome
/// has run out, from the same enum. Terminal.
pub const JOB_STATUS_TIMED_OUT: &str = "TimedOut";

/// The `JobStatus` values whose work has ENDED: a job in one of them is being worked
/// on by nobody, so no worker can acknowledge it and no outcome can be reported for
/// it any more. The other four values of the enum are the stages of a job that is
/// still live (`Created`, `Queued`, `Dispatched`, `InProgress`).
pub const TERMINAL_JOB_STATUSES: [&str; 3] =
    [JOB_STATUS_SUCCEEDED, JOB_STATUS_FAILED, JOB_STATUS_TIMED_OUT];

/// How long one `PollForJobs` call withholds a job it reported from later polls.
///
/// `Job$nonce` is "a system-generated random number that CodePipeline uses to ensure
/// that the job is being worked on by only one job worker", so a poll that reports a
/// job hands it to that worker: until the worker's claim lapses, another poll is not
/// reported the same job. The job becoming pollable again is a transition the CLOCK
/// owes — nothing but the passage of time makes it due — and AWS publishes no figure
/// for how long a polled, unacknowledged job stays held, so the span is this
/// emulator's, chosen inside the 250-750 ms band this project fixes for a clock-owed
/// transition: a worker that polls twice in a row still observes the job withheld,
/// and a worker that abandons a job it polled does not strand it.
pub const JOB_POLL_LEASE: Duration = Duration::from_millis(500);

/// The most jobs this emulator keeps queued for custom-action job workers, across
/// every action type.
///
/// A bound is required for the same reason every other store has one: a job is queued
/// each time an execution reaches a `Custom` action, and the release paths (the
/// action finishing, its execution being evicted, its pipeline being deleted) are not
/// under the caller's control. Past the bound the OLDEST queued job is dropped, so
/// the queue cannot grow without limit however many executions run. AWS publishes no
/// such figure; it is this emulator's, sized like
/// [`MAX_EXECUTION_HISTORY_PER_PIPELINE`].
pub const MAX_QUEUED_JOBS: usize = 1000;

/// The `ArtifactLocationType` an artifact of this service's artifact store carries:
/// the model declares exactly one value
/// (`crate::ops::types::ARTIFACT_LOCATION_TYPE_VALUES` -> `S3`), and an artifact
/// store IS an S3 bucket (`ArtifactStore$location` is "the S3 bucket used for
/// storing the artifacts").
pub const ARTIFACT_LOCATION_TYPE_S3: &str = crate::ops::types::ARTIFACT_LOCATION_TYPE_VALUES[0];

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

/// The `TriggerType` an execution started by a new action revision records, from
/// the same enum (`smithy_query query="shape" shape="TriggerType"` lists
/// `PutActionRevision` among its members, which is what makes reporting a new source
/// revision a way for an execution to begin).
pub const TRIGGER_TYPE_PUT_ACTION_REVISION: &str = "PutActionRevision";

/// How many executions a pipeline in `SUPERSEDED` mode runs at once.
///
/// `SUPERSEDED` is the mode in which a newer execution takes the place of the one
/// already running — `PipelineDeclaration$executionMode` documents it as "the method
/// that the pipeline will use to handle multiple executions" and names this the
/// default — so such a pipeline is running exactly one execution at any instant.
/// Superseding the running execution is not something this emulator can do: it does
/// not run a pipeline's stages, so nothing here could tell how far the running
/// execution has got or move it to `Superseded` without reporting a transition for
/// work nothing ran. A trigger that would start a second concurrent execution is
/// therefore refused with the error the model declares for exactly this condition,
/// `ConcurrentPipelineExecutionsLimitExceededException` ("The pipeline has reached
/// the limit for concurrent pipeline executions").
pub const MAX_CONCURRENT_EXECUTIONS_SUPERSEDED: usize = 1;

/// How many executions a pipeline in `QUEUED` or `PARALLEL` mode runs at once.
///
/// Those two modes exist to hold more than one execution, so the bound is not 1; AWS
/// publishes the figure as an adjustable per-account quota rather than as part of the
/// API, so the value here is this emulator's bound. A bound is required all the same:
/// every accepted trigger adds a non-terminal execution and this slice runs no stage
/// that would ever complete one.
pub const MAX_CONCURRENT_EXECUTIONS_PARALLEL: usize = 50;

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

/// The most executions this emulator retains ACROSS every pipeline.
///
/// The per-pipeline window above leaves the store bounded only by its product with
/// [`MAX_PIPELINES_PER_ACCOUNT`] (a million records), and every record now retains the
/// stage/action plan its execution runs, so this store-wide window is what actually
/// bounds the execution store's memory: at most this many records, each with at most
/// [`MAX_ACTIONS_PER_PIPELINE`] action entries. Starting another execution past it drops
/// the oldest retained one, exactly as the per-pipeline window does. The figure is this
/// emulator's own, chosen the way the S3 emulator's service-wide object ceiling is.
pub const MAX_EXECUTIONS_TOTAL: usize = 10_000;

/// The most webhooks this emulator holds for the account before `PutWebhook`
/// refuses another one.
///
/// A bound is required for the same reason the pipeline one is: `webhooks` grows by
/// one on every accepted create and is released only by `DeleteWebhook`. The refusal
/// it answers with is the error the model declares for exactly this condition —
/// `LimitExceededException`. AWS publishes its own webhook quota per account and
/// adjusts it, so this figure is this emulator's bound rather than a documented
/// constant.
pub const MAX_WEBHOOKS_PER_ACCOUNT: usize = 1000;

/// The tag-key prefix AWS reserves for its own tags: "you cannot edit or delete a
/// tag with the `aws:` prefix", so a caller-supplied key carrying it — in any
/// letter case, since the reserved namespace is not case-sensitive — is refused with
/// `InvalidTagsException`.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// What a stored `SecretToken` renders as in a response.
///
/// `PutWebhook` and `ListWebhooks` both document "If a secret token was provided, it
/// will be redacted in the response", so the stored token never leaves the emulator;
/// the placeholder satisfies `WebhookAuthConfigurationSecretToken`'s modelled
/// 1..100 length.
pub const REDACTED_SECRET_TOKEN: &str = "****";

/// The `WebhookAuthenticationType` value whose configuration carries a GitHub HMAC
/// secret, from the model's enum (`WEBHOOK_AUTHENTICATION_TYPE_VALUES` ->
/// `GITHUB_HMAC`, `IP`, `UNAUTHENTICATED`).
pub const AUTHENTICATION_GITHUB_HMAC: &str = "GITHUB_HMAC";

/// The `WebhookAuthenticationType` value whose configuration carries an allowed IP
/// range.
pub const AUTHENTICATION_IP: &str = "IP";

/// The `WebhookAuthenticationType` value that "accepts all webhook trigger requests
/// regardless of origin", so its configuration carries neither property.
pub const AUTHENTICATION_UNAUTHENTICATED: &str = "UNAUTHENTICATED";

/// The `ActionOwner` a custom action type carries, from the model's enum
/// (`smithy_query query="shape" shape="ActionOwner"` -> `AWS`, `ThirdParty`,
/// `Custom`). `ActionTypeId$owner` documents the three values and
/// `CreateCustomActionType` "creates a new custom action", so an action type this
/// service mints is owned by `Custom`.
pub const ACTION_OWNER_CUSTOM: &str = "Custom";

/// The most tags one resource of this service holds before a tagging call is refused
/// with `TooManyTagsException` ("The tags limit for a resource has been exceeded").
///
/// `TagList` carries no `@length`, so the bound is enforced by the handlers. The
/// figure is AWS's resource-tagging limit of 50 tags per resource, which
/// CodePipeline's own "Tagging resources" guide states for a pipeline, custom action
/// or webhook. A bound is also required here: a resource's tag set grows on every
/// accepted `TagResource` call, so without one a caller could grow the store without
/// limit.
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
    /// Stored action executions, keyed by their generated `actionExecutionId` — what
    /// `ListActionExecutions` reports. An entry comes into being when the pipeline
    /// execution it belongs to starts the action
    /// ([`ActionExecutionRecord::start`]) and lives exactly as long as that
    /// execution's record, so a listing never reports an action execution of an
    /// execution no caller can read.
    pub action_executions: BTreeMap<String, ActionExecutionRecord>,
    /// Action execution ids per `pipelineExecutionId`, in the order the actions
    /// started — the same oldest-first shape [`Self::executions_by_pipeline`] has,
    /// so one order serves both the listing and the eviction.
    pub action_executions_by_execution: BTreeMap<String, Vec<String>>,
    /// The revision each action currently carries, keyed by `(pipeline name, stage
    /// name, action name)` — what `PutActionRevision` reports about a source and what
    /// the next report of the same source is compared against. Bounded by the stored
    /// declarations, and dropped with the pipeline in [`Self::forget_pipeline`].
    pub action_revisions: BTreeMap<(String, String, String), ActionRevision>,
    /// `(pipeline name, clientRequestToken)` -> the execution id that token minted,
    /// so repeating an idempotency token returns the same execution. An entry lives
    /// exactly as long as the execution it names.
    pub idempotency: BTreeMap<(String, String), String>,
    /// Stored webhooks, keyed by `WebhookDefinition$name`.
    pub webhooks: BTreeMap<String, Webhook>,
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
    /// The jobs waiting for a CUSTOM action's job worker, per action type, oldest
    /// first — what `PollForJobs` hands out.
    ///
    /// A `Custom` action type is run by the account's own job worker rather than by
    /// CodePipeline, so when an execution reaches such an action the execution plane
    /// queues a job here ([`driver`]) and the action stays `InProgress` until a worker
    /// reports its result. Write it through [`Self::enqueue_job`] (or
    /// [`enqueue_job_into`], the same path the driver takes while it holds only this
    /// field), which is where the [`MAX_QUEUED_JOBS`] bound lives.
    pub jobs: JobQueue,
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

    /// A fresh `actionExecutionId` neither the store nor `minted` already uses.
    ///
    /// The same rule [`Self::new_execution_id`] applies to an execution id: 122
    /// random bits, and the store checked anyway so uniqueness is a property of the
    /// state rather than of the odds. `minted` carries the ids already handed out in
    /// the same call but not yet stored, so one call minting several ids cannot hand
    /// out the same one twice either. Bounded, so a request can never spin.
    fn fresh_action_execution_id(&self, minted: &BTreeSet<String>) -> String {
        let mut candidate = random_uuid_v4();
        for _ in 0..8 {
            if !self.action_executions.contains_key(&candidate) && !minted.contains(&candidate) {
                break;
            }
            candidate = random_uuid_v4();
        }
        candidate
    }

    /// Record `record` as its pipeline's newest execution and return its id.
    ///
    /// The retained history is bounded TWICE: by
    /// [`MAX_EXECUTION_HISTORY_PER_PIPELINE`] per pipeline, and by
    /// [`MAX_EXECUTIONS_TOTAL`] across every pipeline — the per-pipeline window alone
    /// would leave the store bounded only by its product with
    /// [`MAX_PIPELINES_PER_ACCOUNT`], and each record retains a plan with one entry per
    /// action of its declaration (itself bounded by [`MAX_ACTIONS_PER_PIPELINE`]). The
    /// oldest records beyond either window are dropped here, and so are the idempotency
    /// tokens that named them — otherwise a token would keep naming an execution no
    /// caller can read, and the maps would grow per request without limit.
    ///
    /// Starting an execution also starts the actions it begins with, so the action
    /// executions those actions get are minted HERE rather than by each caller: this
    /// is the one way an execution enters the store, so `StartPipelineExecution` and
    /// `PutActionRevision` cannot disagree about what a started execution has
    /// running. The records are dropped with the execution they belong to, here and
    /// in [`Self::forget_pipeline`].
    pub fn record_execution(
        &mut self,
        record: PipelineExecutionRecord,
        client_request_token: Option<String>,
    ) -> String {
        let pipeline_name = record.pipeline_name.clone();
        let execution_id = record.pipeline_execution_id.clone();
        let started = self.starting_action_executions(&record);
        self.executions.insert(execution_id.clone(), record);
        for action_execution in started {
            let action_execution_id = action_execution.action_execution_id.clone();
            self.action_executions_by_execution
                .entry(execution_id.clone())
                .or_default()
                .push(action_execution_id.clone());
            self.action_executions
                .insert(action_execution_id, action_execution);
        }

        let mut evicted: Vec<String> = {
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
        // Then the store-wide window: the oldest retained executions of the pipelines
        // with the longest histories go first, so no caller can grow the store past
        // `MAX_EXECUTIONS_TOTAL` records whatever it does with pipelines.
        while self
            .executions_by_pipeline
            .values()
            .map(Vec::len)
            .sum::<usize>()
            > MAX_EXECUTIONS_TOTAL
        {
            let Some(longest) = self
                .executions_by_pipeline
                .iter_mut()
                .filter(|(_, history)| !history.is_empty())
                .max_by_key(|(_, history)| history.len())
                .map(|(_, history)| history)
            else {
                break;
            };
            evicted.push(longest.remove(0));
        }
        if !evicted.is_empty() {
            for id in &evicted {
                self.executions.remove(id);
                self.forget_action_executions(id);
            }
            self.idempotency.retain(|_, id| !evicted.contains(id));
            // A job waits for the execution that queued it, so an execution no caller
            // can read any more has no job waiting either.
            self.forget_jobs_of_executions(&evicted);
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

    /// Queue `job` as the newest job waiting for the custom action type `id`, and
    /// answer whether it was queued.
    ///
    /// Only a `Custom` action type has jobs of this kind — an `AWS` action type is run
    /// by AWS and a `ThirdParty` one by its partner (whose jobs are
    /// [`Self::third_party_jobs`]) — so any other owner is refused HERE, once, rather
    /// than by every reader. Bounded by [`MAX_QUEUED_JOBS`].
    pub fn enqueue_job(&mut self, id: &ActionTypeId, job: JobRecord) -> bool {
        enqueue_job_into(&mut self.jobs, id, job)
    }

    /// Hand at most `limit` of the custom action type `id`'s waiting jobs to the
    /// worker that is polling, oldest first, and report them as they were handed out.
    ///
    /// Each job handed out is LEASED: a fresh `nonce` is minted for it and it is
    /// withheld from later polls for [`JOB_POLL_LEASE`], which is what "only one job
    /// worker" means for the nonce the model documents. A job whose lease has lapsed
    /// is available again and is handed out with a NEW nonce, so the nonce a lapsed
    /// lease published no longer reaches the job.
    ///
    /// `query` is the `queryParam` pair the poll carried, if any: a job is handed out
    /// only when its action configuration maps that property to that value.
    pub fn lease_jobs(
        &mut self,
        id: &ActionTypeId,
        limit: usize,
        query: Option<(&str, &str)>,
        now: SystemTime,
    ) -> Vec<JobRecord> {
        let mut leased = Vec::new();
        let Some(queue) = self.jobs.get_mut(&ActionTypeKey::from_id(id)) else {
            return leased;
        };
        for job in queue.iter_mut() {
            if leased.len() >= limit {
                break;
            }
            if !job.is_available(now) || !job.matches_query(query) {
                continue;
            }
            job.lease(now);
            leased.push(job.clone());
        }
        leased
    }

    /// The job queued under `job_id`, whatever action type's queue it waits in.
    ///
    /// A worker's call names a job by id alone, so the search spans every action
    /// type's queue: a job id identifies one job across the service, and which queue
    /// it waits in is not something the caller knows or states.
    pub fn job(&self, job_id: &str) -> Option<&JobRecord> {
        self.jobs.values().flatten().find(|job| job.job_id == job_id)
    }

    /// Drop every queued job whose action is no longer waiting for a worker.
    ///
    /// A job is owed to a worker exactly while the action that queued it is still
    /// `InProgress`: once the action has been abandoned by a stop, or has reached an
    /// outcome, the job it was waiting on is not work any more and is forgotten. A job
    /// whose execution record this store no longer holds is left alone — it is kept by
    /// the paths that drop an execution ([`Self::forget_pipeline`] and the eviction in
    /// [`Self::record_execution`]) and by [`MAX_QUEUED_JOBS`], not by this sweep.
    pub fn retire_finished_jobs(&mut self) {
        let Self { jobs, executions, .. } = self;
        jobs.retain(|_, queue| {
            queue.retain(|job| {
                executions
                    .get(&job.pipeline_execution_id)
                    .and_then(|record| record.stages.get(job.stage))
                    .and_then(|stage| stage.actions.get(job.action))
                    .is_none_or(ActionRun::is_in_progress)
            });
            !queue.is_empty()
        });
    }

    /// The names of the pipelines with an execution that can still move — the ones a
    /// call materialises the clock's owed steps for when the request names no pipeline
    /// of its own.
    ///
    /// Each name appears once, in name order: an execution in a terminal status is
    /// owed nothing, so a pipeline with only those is not walked at all.
    pub fn pipelines_with_live_executions(&self) -> Vec<String> {
        self.executions
            .values()
            .filter(|record| !is_terminal_status(&record.status))
            .map(|record| record.pipeline_name.clone())
            .collect::<BTreeSet<String>>()
            .into_iter()
            .collect()
    }

    /// Drop every queued job of the pipeline executions named by `execution_ids`.
    fn forget_jobs_of_executions(&mut self, execution_ids: &[String]) {
        self.jobs.retain(|_, queue| {
            queue.retain(|job| !execution_ids.contains(&job.pipeline_execution_id));
            !queue.is_empty()
        });
    }

    /// The action executions a freshly started `execution` begins with: one per
    /// action the execution's pipeline version runs FIRST — the actions of its first
    /// stage at that stage's lowest `runOrder` — each `InProgress` as of the
    /// execution's start.
    ///
    /// An action execution exists only once the execution has reached that action, so
    /// nothing is minted for a later stage, for a higher `runOrder` in the first
    /// stage, or for a pipeline version that declares no stage or no action at all:
    /// those actions have not started, and an action that has not started has no
    /// execution to report. Nothing in this emulator finishes an action either (see
    /// [`ACTION_STATUS_IN_PROGRESS`]), so nothing later moves the execution on to the
    /// next `runOrder` or the next stage.
    ///
    /// The declaration read is the one the execution RUNS
    /// ([`Pipeline::declaration_at`]), not whatever version the pipeline carries now,
    /// so a later `UpdatePipeline` cannot change what an execution already started.
    fn starting_action_executions(
        &self,
        execution: &PipelineExecutionRecord,
    ) -> Vec<ActionExecutionRecord> {
        let Some(pipeline) = self.pipelines.get(&execution.pipeline_name) else {
            return Vec::new();
        };
        let Some(declaration) = pipeline.declaration_at(Some(execution.pipeline_version)) else {
            return Vec::new();
        };
        let Some(stage) = declaration.stages.first() else {
            return Vec::new();
        };
        let Some(first_run_order) = stage.actions.iter().map(run_order).min() else {
            return Vec::new();
        };
        // The action's Region, when its declaration names none, is the Region the
        // pipeline itself lives in, which is the one its stored ARN carries.
        let pipeline_region = parse_resource_arn(&pipeline.arn).map(|arn| arn.region);
        let mut minted: BTreeSet<String> = BTreeSet::new();
        let mut started = Vec::new();
        for action in &stage.actions {
            if run_order(action) != first_run_order {
                continue;
            }
            // Minted against the store and against the ids this call has already
            // handed out, so no two action executions share an id however many
            // executions the store holds.
            let action_execution_id = self.fresh_action_execution_id(&minted);
            minted.insert(action_execution_id.clone());
            started.push(ActionExecutionRecord::start(
                action_execution_id,
                execution,
                &stage.name,
                action,
                declaration,
                pipeline_region.clone(),
            ));
        }
        started
    }

    /// Drop every action execution recorded for the pipeline execution `execution_id`.
    fn forget_action_executions(&mut self, execution_id: &str) {
        for action_execution_id in self
            .action_executions_by_execution
            .remove(execution_id)
            .unwrap_or_default()
        {
            self.action_executions.remove(&action_execution_id);
        }
    }

    /// Every action execution of `pipeline_name`'s retained executions, most recently
    /// started first — the order `ListActionExecutions` pages through.
    ///
    /// An action execution starts when the execution containing it reaches the
    /// action, so the newest execution's action executions come first; within one
    /// execution the actions that started together are reported in the order the
    /// stage declares them, which is the order they were recorded in.
    pub fn action_executions_for_pipeline(
        &self,
        pipeline_name: &str,
    ) -> Vec<&ActionExecutionRecord> {
        self.executions_by_pipeline
            .get(pipeline_name)
            .into_iter()
            .flatten()
            .rev()
            .filter_map(|execution_id| self.action_executions_by_execution.get(execution_id))
            .flatten()
            .filter_map(|action_execution_id| self.action_executions.get(action_execution_id))
            .collect()
    }

    /// Forget a pipeline: the declaration, the executions still retained for it, the
    /// action executions those executions started, the idempotency tokens that named
    /// them, and the revisions its actions carried.
    pub fn forget_pipeline(&mut self, name: &str) {
        self.pipelines.remove(name);
        for id in self.executions_by_pipeline.remove(name).unwrap_or_default() {
            self.executions.remove(&id);
            self.forget_action_executions(&id);
        }
        // The jobs those executions queued for a custom action's worker go with them:
        // a deleted pipeline has no action left waiting for one.
        self.jobs.retain(|_, queue| {
            queue.retain(|job| job.pipeline_name != name);
            !queue.is_empty()
        });
        self.idempotency
            .retain(|(pipeline_name, _), _| pipeline_name != name);
        self.action_revisions
            .retain(|(pipeline_name, _, _), _| pipeline_name != name);
    }

    /// The pipeline's current execution — the most recently started one still
    /// retained — or `None` when the pipeline has never run.
    ///
    /// `executions_by_pipeline` is kept oldest-first, so the current execution is the
    /// last entry.
    pub fn current_execution_id(&self, pipeline_name: &str) -> Option<&String> {
        self.executions_by_pipeline
            .get(pipeline_name)?
            .last()
    }

    /// How many of the pipeline's retained executions are still in a non-terminal
    /// status ([`NON_TERMINAL_STATUSES`]), i.e. how many concurrent execution slots
    /// the pipeline currently occupies.
    pub fn concurrent_execution_count(&self, pipeline_name: &str) -> usize {
        self.executions_by_pipeline
            .get(pipeline_name)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.executions.get(id))
                    .filter(|record| {
                        NON_TERMINAL_STATUSES.contains(&record.status.as_str())
                    })
                    .count()
            })
            .unwrap_or_default()
    }

    /// The tags of the resource `arn` names.
    ///
    /// The ARN is resolved by [`Self::tag_target`], so the reader and the writer
    /// refuse the same strings with the same message, and the set is reported the
    /// same way whichever KIND of resource holds it — a pipeline's tags are stored as
    /// a [`TagSet`] already, an action type's as the `TagList` its create answered
    /// with.
    pub fn resource_tags(&self, arn: &str) -> Result<TagSet, TagTargetProblem> {
        let target = self.tag_target(arn)?;
        match &target {
            TagTarget::Pipeline(name) => self.pipelines.get(name).map(|p| p.tags.clone()),
            TagTarget::ActionType(key) => {
                self.action_types.get(key).map(|record| tag_set(&record.tags))
            }
            TagTarget::Webhook(name) => {
                self.webhooks.get(name).map(|webhook| tag_set(&webhook.tags))
            }
        }
        .ok_or_else(|| unresolved_after_lookup(&target))
    }

    /// Store `tags` as the tags of the resource `arn` names: the one seam through
    /// which a tagging operation changes a resource's tags, whichever kind it is.
    ///
    /// The caller computes the set to store from [`Self::resource_tags`] and judges
    /// it ([`validate_tags`], [`tag_count_after_merge`]) before calling this, so a
    /// refused request never reaches the store.
    pub fn set_resource_tags(&mut self, arn: &str, tags: TagSet) -> Result<(), TagTargetProblem> {
        let target = self.tag_target(arn)?;
        match &target {
            TagTarget::Pipeline(name) => match self.pipelines.get_mut(name) {
                Some(pipeline) => pipeline.tags = tags,
                None => return Err(unresolved_after_lookup(&target)),
            },
            TagTarget::ActionType(key) => match self.action_types.get_mut(key) {
                // An action type keeps its tags as the modelled `TagList`, so the
                // set is written back in its key order.
                Some(record) => record.tags = tag_list(&tags),
                None => return Err(unresolved_after_lookup(&target)),
            },
            TagTarget::Webhook(name) => match self.webhooks.get_mut(name) {
                // A webhook keeps its tags as the modelled `TagList` too.
                Some(webhook) => webhook.tags = tag_list(&tags),
                None => return Err(unresolved_after_lookup(&target)),
            },
        }
        Ok(())
    }

    /// The taggable resource `arn` names, as the store holds it.
    ///
    /// One rule, shared by every operation that reaches a resource BY ARN rather than
    /// by name: the ARN is first read as a `codepipeline` resource ARN
    /// ([`parse_resource_arn`]) — a string that is not one is refused as
    /// `InvalidArnException` — and then resolved against the store, where an ARN that
    /// names no resource held here is refused as `ResourceNotFoundException`, naming
    /// the resource and the account the ARN asked for. A resource resolves only
    /// within the scope its ARN names — on the ARN the service minted for it — so an
    /// ARN differing in partition, Region or account names a resource in another
    /// scope rather than this one.
    pub fn tag_target(&self, arn: &str) -> Result<TagTarget, TagTargetProblem> {
        let parsed = parse_resource_arn(arn).ok_or_else(|| {
            TagTargetProblem::InvalidArn(format!(
                "{arn} is not a valid CodePipeline resource ARN"
            ))
        })?;
        let account = parsed.account;
        match parsed.resource {
            ResourceRef::Pipeline(name) => {
                if self.pipelines.get(&name).is_some_and(|p| p.arn == arn) {
                    Ok(TagTarget::Pipeline(name))
                } else {
                    Err(TagTargetProblem::NotFound(format!(
                        "Pipeline with name {name} does not exist in account {account}."
                    )))
                }
            }
            // A custom action type is a taggable resource this service holds:
            // `CreateCustomActionType` registers it with the tags its caller applied,
            // and the ARN names it by its four `ActionTypeId` members. Only a
            // REGISTRATION is a resource of the account — the action types the
            // service itself provides belong to no caller, and carry no Region — and
            // it resolves, like a pipeline, only on the ARN the service would mint
            // for it, so partition, Region and account must all be the ones it was
            // registered in.
            ResourceRef::ActionType { identifier, key } => {
                let held = self.action_types.get(&key).is_some_and(|record| {
                    record
                        .region
                        .as_deref()
                        .is_some_and(|region| action_type_arn(region, &identifier) == arn)
                });
                if held {
                    Ok(TagTarget::ActionType(key))
                } else {
                    Err(TagTargetProblem::NotFound(format!(
                        "ActionType with id {identifier} does not exist in account {account}."
                    )))
                }
            }
            // A webhook is a taggable resource this service holds too: `PutWebhook`
            // stores it with the tags its caller applied, and the ARN names it by
            // `WebhookDefinition$name`. It resolves, like the other kinds, only on
            // the ARN the service minted for it.
            ResourceRef::Webhook(name) => {
                if self.webhooks.get(&name).is_some_and(|w| w.arn == arn) {
                    Ok(TagTarget::Webhook(name))
                } else {
                    Err(TagTargetProblem::NotFound(format!(
                        "Webhook with name {name} does not exist in account {account}."
                    )))
                }
            }
        }
    }

    /// A webhook trigger URL in `region` that no stored webhook already uses.
    ///
    /// Like [`Self::new_execution_id`]: the 122 random bits make a collision against
    /// the at most [`MAX_WEBHOOKS_PER_ACCOUNT`] stored URLs unreachable in practice,
    /// and the store is checked anyway. The check is best-effort and the loop is
    /// bounded, so a request can never spin: after eight collisions the last
    /// candidate is returned rather than failing a request over odds of roughly
    /// 2^-122 per draw.
    pub fn new_webhook_url(&self, region: &str) -> String {
        let mut candidate = webhook_url(region, &random_uuid_v4());
        for _ in 0..8 {
            if !self
                .webhooks
                .values()
                .any(|webhook| webhook.url == candidate)
            {
                break;
            }
            candidate = webhook_url(region, &random_uuid_v4());
        }
        candidate
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
                ThirdPartyJobProblem::NotFound(job_not_found_message(job_id))
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

/// The taggable resource an ARN resolved to, as the store holds it: which store, and
/// the key it is held under. Answered by
/// [`ComAmazonawsCodepipelineCodepipeline20150709State::tag_target`] only for a
/// resource that EXISTS, so the accessors built on it read and write the one the
/// caller's ARN named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagTarget {
    /// A pipeline, keyed by `PipelineDeclaration$name`.
    Pipeline(String),
    /// A custom action type this account registered, keyed by its identity.
    ActionType(ActionTypeKey),
    /// A webhook, keyed by `WebhookDefinition$name`.
    Webhook(String),
}

impl TagTarget {
    /// How a refusal names this resource.
    fn describe(&self) -> String {
        match self {
            Self::Pipeline(name) => format!("Pipeline with name {name}"),
            Self::ActionType(key) => format!("ActionType with id {}", key.cursor()),
            Self::Webhook(name) => format!("Webhook with name {name}"),
        }
    }
}

/// The URL a webhook is triggered on, in the shape CodePipeline generates
/// (`https://<region>.webhooks.aws/trigger?t=<opaque token>&v=1`).
///
/// `PutWebhook` documents that it "returns a unique webhook URL generated by
/// CodePipeline" to be "supplied to third party source hosting providers", and
/// `ListWebhookItem$url` is `@required`, so every stored webhook carries one. The
/// token is the only part that identifies the webhook and it is minted per webhook
/// by [`ComAmazonawsCodepipelineCodepipeline20150709State::new_webhook_url`], never
/// derived from the request. This emulator serves no webhook TRIGGER plane, so the
/// URL is an opaque identifier that nothing here dereferences.
fn webhook_url(region: &str, token: &str) -> String {
    format!("https://{region}.webhooks.aws/trigger?t={token}&v=1")
}

/// One stored webhook: the definition exactly as the caller sent it, plus the
/// service-owned members `PutWebhook` and `ListWebhooks` report.
#[derive(Debug, Clone)]
pub struct Webhook {
    /// The definition as stored, secret token included — [`Webhook::to_item`] is
    /// what redacts it on the way out.
    pub definition: WebhookDefinition,
    /// The trigger URL minted when the webhook was created, stable for its life.
    pub url: String,
    /// This webhook's ARN.
    pub arn: String,
    /// The tags applied to the webhook.
    pub tags: Vec<Tag>,
    /// When a trigger request last started the target pipeline, or `None` while the
    /// webhook has never fired.
    ///
    /// `ListWebhookItem` reports no created/updated instants, so none are kept: a
    /// webhook's only service-owned instant is this one.
    pub last_triggered: Option<SystemTime>,
}

impl Webhook {
    /// Store `definition` as a brand-new webhook triggered on `url`.
    pub fn create(
        definition: WebhookDefinition,
        url: String,
        arn: String,
        tags: Vec<Tag>,
    ) -> Self {
        Self {
            definition,
            url,
            arn,
            tags,
            // Nothing has triggered a freshly defined webhook, and this emulator
            // serves no trigger plane, so the member is absent rather than carrying
            // an invented instant.
            last_triggered: None,
        }
    }

    /// This webhook as `PutWebhook` and `ListWebhooks` report it, with the secret
    /// token redacted as both operations document.
    pub fn to_item(&self) -> ListWebhookItem {
        let mut definition = self.definition.clone();
        if definition
            .authentication_configuration
            .secret_token
            .is_some()
        {
            definition.authentication_configuration.secret_token =
                Some(REDACTED_SECRET_TOKEN.to_string());
        }
        ListWebhookItem {
            definition,
            url: self.url.clone(),
            // `errorCode`/`errorMessage` report the failure of the LAST webhook
            // trigger, and this emulator serves no trigger plane, so neither is
            // invented here.
            error_message: None,
            error_code: None,
            last_triggered: self.last_triggered.map(to_date_time),
            arn: Some(self.arn.clone()),
            tags: optional_list(self.tags.clone()),
        }
    }
}

/// The refusal for the case [`ComAmazonawsCodepipelineCodepipeline20150709State::tag_target`]
/// has already ruled out: it answers a target only for a resource it just found in
/// the same store, so the accessors above cannot miss it. Reported as the same
/// refusal rather than as an invented success, and stated ONCE so no caller invents
/// its own wording for an unreachable case.
fn unresolved_after_lookup(target: &TagTarget) -> TagTargetProblem {
    TagTargetProblem::NotFound(format!("{} does not exist.", target.describe()))
}

/// The identity an `actiontype` ARN's `<owner>/<category>/<provider>/<version>`
/// names, or `None` when it is not four non-empty members.
///
/// The ARN orders the members OWNER first, while [`ActionTypeKey`] is ordered the way
/// `ActionTypeId` declares them (category first), so the two renderings are not the
/// same string and this is the one place that maps between them. Called by
/// [`parse_resource_arn`], so a `ResourceRef::ActionType` always carries the identity
/// its ARN names.
fn action_type_arn_key(identifier: &str) -> Option<ActionTypeKey> {
    let mut parts = identifier.split('/');
    let key = ActionTypeKey {
        owner: parts.next()?.to_string(),
        category: parts.next()?.to_string(),
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

/// The ARN this service mints for the action type `identifier` names, registered in
/// `region` — what an ARN naming that registration has to equal.
fn action_type_arn(region: &str, identifier: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{ARN_TYPE_ACTION_TYPE}{identifier}"),
    )
}

/// A resource's tags: tag key -> tag value.
///
/// A tag SET, not a list: `TagResource` "adds to or modifies the tags of the given
/// resource", so a key a resource already carries is overwritten rather than
/// duplicated, and the key order a listing pages through is the total order of a
/// `BTreeMap` (what makes a tag cursor round-trip deterministically).
pub type TagSet = BTreeMap<String, String>;

/// Whether `key` is in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace, matched in
/// any letter case — the reading [`validate_tags`] enforces, so one rule decides it
/// for every caller.
pub fn is_reserved_tag_key(key: &str) -> bool {
    // `str::get` answers `None` rather than panicking when the key's first four
    // bytes are not a character boundary, so a key that cannot carry the prefix is
    // simply not one.
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The modelled `TagList` as a [`TagSet`]: a key named twice keeps the LAST value,
/// though no handler reaches that case — [`validate_tags`] refuses a list naming one
/// key twice before the set is built.
pub fn tag_set<'a>(tags: impl IntoIterator<Item = &'a Tag>) -> TagSet {
    tags.into_iter()
        .map(|tag| (tag.key.clone(), tag.value.clone()))
        .collect()
}

/// A [`TagSet`] as the modelled `TagList`, ordered by key.
pub fn tag_list(tags: &TagSet) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// How many tags a resource would hold once `incoming` is merged into `existing`:
/// a key already present is replaced, so only new keys add to the count.
pub fn tag_count_after_merge(existing: &TagSet, incoming: &TagSet) -> usize {
    existing.len() + incoming.keys().filter(|key| !existing.contains_key(*key)).count()
}

/// Why a tag operation cannot reach the resource an ARN names. Each variant carries
/// the message the refusal reports, built from the request so the caller can see
/// WHICH resource in WHICH account was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagTargetProblem {
    /// The string is not a `codepipeline` resource ARN: refused as
    /// `InvalidArnException`.
    InvalidArn(String),
    /// A well-formed ARN naming no resource this service holds: refused as
    /// `ResourceNotFoundException`.
    NotFound(String),
}

impl TagTargetProblem {
    /// Why the ARN could not be resolved.
    pub fn message(&self) -> &str {
        match self {
            Self::InvalidArn(message) | Self::NotFound(message) => message,
        }
    }
}

/// A defect in a `WebhookDefinition` that the model's own constraints cannot express,
/// so it has to be refused by the handler.
///
/// Each variant names the modelled error shape that states it, so every caller
/// refuses one defect in exactly one way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookProblem {
    /// Refused as `InvalidWebhookFilterPatternException`: one event filter rule is
    /// not in the format the member documents.
    FilterPattern(String),
    /// Refused as `InvalidWebhookAuthenticationParametersException`: the
    /// authentication configuration does not match the authentication type.
    Authentication(String),
}

impl WebhookProblem {
    /// Why the definition was refused. Built from the request, so the caller can fix
    /// the definition from this text alone.
    pub fn message(&self) -> &str {
        match self {
            Self::FilterPattern(message) | Self::Authentication(message) => message,
        }
    }
}

/// A well-formed `codepipeline` resource ARN, split into the parts a refusal and a
/// lookup need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceArnRef {
    /// The Region segment.
    pub region: String,
    /// The 12-digit account segment.
    pub account: String,
    /// The resource the ARN names.
    pub resource: ResourceRef,
}

/// What a well-formed `codepipeline` resource ARN names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceRef {
    /// `arn:<partition>:codepipeline:<region>:<account>:<pipeline-name>` — a pipeline
    /// ARN carries the name directly, with no resource-type prefix.
    Pipeline(String),
    /// `arn:<partition>:codepipeline:<region>:<account>:actiontype:<owner>/<category>/<provider>/<version>`,
    /// carrying both the identifier as written and the identity it names.
    ActionType {
        /// The `<owner>/<category>/<provider>/<version>` the ARN carries.
        identifier: String,
        /// The identity those four members name.
        key: ActionTypeKey,
    },
    /// `arn:<partition>:codepipeline:<region>:<account>:webhook:<webhook-name>`.
    Webhook(String),
}

/// The `actiontype` resource-type prefix in a `codepipeline` resource ARN.
const ARN_TYPE_ACTION_TYPE: &str = "actiontype:";

/// The `webhook` resource-type prefix in a `codepipeline` resource ARN.
const ARN_TYPE_WEBHOOK: &str = "webhook:";

/// The resource `arn` names, or `None` when `arn` is not a `codepipeline` resource
/// ARN at all.
///
/// `ResourceArn`'s modelled pattern
/// (`^arn:aws(-[\w]+)*:codepipeline:.+:[0-9]{12}:.+$`) is already enforced at the
/// boundary, so it is not re-checked here; what this adds is the structure the
/// pattern cannot state — that the region segment is a region token, and that the
/// resource segment names one of the resource forms CodePipeline defines. A resource
/// segment with no type prefix this service defines is read as a pipeline name
/// (CodePipeline's pipeline ARNs carry it bare), so a name no pipeline uses resolves
/// as "no such resource" rather than as a malformed ARN.
pub fn parse_resource_arn(arn: &str) -> Option<ResourceArnRef> {
    // Six segments: the resource portion is the rest, because it may itself contain
    // `:` (the `actiontype:`/`webhook:` forms do).
    let mut segments = arn.splitn(6, ':');
    if segments.next()? != "arn" {
        return None;
    }
    if !is_arn_partition(segments.next()?) {
        return None;
    }
    if segments.next()? != ARN_SERVICE {
        return None;
    }
    let region = segments.next()?;
    if !is_region_token(region) {
        return None;
    }
    let account = segments.next()?;
    if account.len() != 12 || !account.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let resource = segments.next()?;

    let resource = if let Some(identifier) = resource.strip_prefix(ARN_TYPE_ACTION_TYPE) {
        // The four `ActionTypeId` members, or no action type at all.
        Some(ResourceRef::ActionType {
            identifier: identifier.to_string(),
            key: action_type_arn_key(identifier)?,
        })
    } else if let Some(name) = resource.strip_prefix(ARN_TYPE_WEBHOOK) {
        (!name.is_empty()).then(|| ResourceRef::Webhook(name.to_string()))
    } else {
        (!resource.is_empty()).then(|| ResourceRef::Pipeline(resource.to_string()))
    }?;
    Some(ResourceArnRef {
        region: region.to_string(),
        account: account.to_string(),
        resource,
    })
}

/// Whether `partition` is an ARN partition: `aws`, or `aws-` followed by one or more
/// `-`-separated word segments (`aws-cn`, `aws-us-gov`), which is what
/// `ResourceArn`'s `aws(-[\w]+)*` states.
fn is_arn_partition(partition: &str) -> bool {
    match partition.strip_prefix("aws") {
        None => false,
        Some("") => true,
        Some(rest) => rest.strip_prefix('-').is_some_and(|rest| {
            rest.split('-')
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        }),
    }
}

/// Whether `region` is a region token: non-empty, lower-case alphanumerics and `-`
/// (`us-east-1`). An ARN whose fourth segment is anything else is not addressing an
/// AWS Region, so it names no resource.
///
/// Every ARN this service mints satisfies it: the region comes from
/// [`request_region`], which reads the SigV4 credential scope (whose region is
/// lower-case by the signing specification) or falls back to [`DEFAULT_REGION`].
fn is_region_token(region: &str) -> bool {
    !region.is_empty()
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Refuse a `WebhookDefinition` whose filters or authentication parameters cannot
/// describe a webhook.
///
/// Two documented rules, neither expressible as a Smithy constraint:
///
/// * `WebhookFilterRule$jsonPath` is "a JsonPath expression that is applied to the
///   body/payload of the webhook", so a value that is not a JsonPath expression at
///   all can never select anything.
/// * `WebhookAuthConfiguration` documents which property each authentication type
///   uses: "For `GITHUB_HMAC`, only the `SecretToken` property must be set", "For
///   `IP`, only the `AllowedIPRange` property must be set. This property must be set
///   to a valid CIDR range", and `UNAUTHENTICATED` "accepts all webhook trigger
///   requests regardless of origin", so it uses neither.
pub fn validate_webhook_definition(
    definition: &WebhookDefinition,
) -> Result<(), WebhookProblem> {
    // What reaches the expression parser is bounded by the model, and the boundary
    // validator enforces both bounds BEFORE this runs: `WebhookFilters` is
    // `@length(min: 0, max: 5)` and `JsonPath` is `@length(min: 1, max: 150)`
    // (`smithy_query query="shape" shape="WebhookFilters,JsonPath"`), so at most five
    // strings of at most 150 characters each are parsed per request — no part of the
    // request body's size reaches the parser.
    for (index, filter) in definition.filters.iter().enumerate() {
        if ::serde_json_path::JsonPath::parse(&filter.json_path).is_err() {
            return Err(WebhookProblem::FilterPattern(format!(
                "filters[{index}].jsonPath {:?} is not a JsonPath expression",
                filter.json_path
            )));
        }
    }

    let configuration = &definition.authentication_configuration;
    let secret_token = configuration
        .secret_token
        .as_deref()
        .filter(|token| !token.is_empty());
    let allowed_ip_range = configuration
        .allowed_ip_range
        .as_deref()
        .filter(|range| !range.is_empty());
    let authentication = definition.authentication.as_str();
    let (wants_secret_token, wants_allowed_ip_range) = match authentication {
        AUTHENTICATION_GITHUB_HMAC => (true, false),
        AUTHENTICATION_IP => (false, true),
        AUTHENTICATION_UNAUTHENTICATED => (false, false),
        // `WebhookAuthenticationType` is a modelled enum, so the boundary validator
        // refuses any other value before this runs; a value that reaches here is one
        // no authentication configuration can be checked against.
        other => {
            return Err(WebhookProblem::Authentication(format!(
                "authentication {other:?} is not a supported authentication type"
            )))
        }
    };

    for (member, supplied, wanted) in [
        ("SecretToken", secret_token.is_some(), wants_secret_token),
        (
            "AllowedIPRange",
            allowed_ip_range.is_some(),
            wants_allowed_ip_range,
        ),
    ] {
        if supplied != wanted {
            let requirement = if wanted { "requires" } else { "does not use" };
            return Err(WebhookProblem::Authentication(format!(
                "authentication {authentication} {requirement} authenticationConfiguration.{member}"
            )));
        }
    }

    if let Some(range) = allowed_ip_range.filter(|range| !is_cidr_range(range)) {
        return Err(WebhookProblem::Authentication(format!(
            "authenticationConfiguration.AllowedIPRange {range:?} is not a valid CIDR range"
        )));
    }
    Ok(())
}

/// Whether `range` is a CIDR range — an IP address and a prefix length the address
/// family allows (32 bits for IPv4, 128 for IPv6) — as
/// `WebhookAuthConfiguration$AllowedIPRange` requires ("This property must be set to
/// a valid CIDR range").
///
/// The address is parsed by `std::net::IpAddr`, so no hand-written address grammar
/// decides what an address is.
fn is_cidr_range(range: &str) -> bool {
    let Some((address, prefix_length)) = range.split_once('/') else {
        return false;
    };
    let Ok(address) = address.parse::<::std::net::IpAddr>() else {
        return false;
    };
    let Ok(prefix_length) = prefix_length.parse::<u8>() else {
        return false;
    };
    let bits = match address {
        ::std::net::IpAddr::V4(_) => 32,
        ::std::net::IpAddr::V6(_) => 128,
    };
    prefix_length <= bits
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
/// its id (which is the key it is stored under), plus the Region it is available in
/// and the tags a create applied to it.
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
    /// The tags applied to this action type, as `CreateCustomActionType` stored them
    /// and a later `TagResource` call merged into them. Empty for an action type the
    /// service provides itself, which no caller tagged (and which no caller can tag:
    /// it belongs to no account). `ActionType` carries no `tags` member, so only an
    /// operation that reports a resource's tags reads it: `CreateCustomActionType`'s
    /// own answer and `ListTagsForResource`.
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
    /// The tags applied to the pipeline, keyed by tag key (see [`TagSet`]).
    pub tags: TagSet,
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
        tags: TagSet,
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

    /// The inbound `TransitionState` to report for the stage at `index`.
    ///
    /// An inbound transition is the edge artifacts cross INTO a stage from the stage
    /// before it, so the first stage has none to report — unless a caller has
    /// changed one for it, in which case the state that call persisted is what the
    /// reader reports rather than silently dropping it.
    ///
    /// The ONE place this rule lives: `GetPipelineState` reports a stage through
    /// [`execution::stage_states`], which reads it from here, so the stored-pipeline
    /// view and the execution view cannot disagree about it.
    pub fn inbound_transition_state(
        &self,
        index: usize,
        stage_name: &str,
    ) -> Option<TransitionState> {
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

    /// This pipeline's tags as the modelled `TagList`, ordered by key.
    pub fn tag_list(&self) -> Vec<Tag> {
        tag_list(&self.tags)
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

/// The `StageConditionState` of one declared condition set: one `ConditionState` per
/// declared condition, each naming the condition's rules.
///
/// A condition is EVALUATED by the rule plane, which this slice does not run, so
/// neither the set, nor a condition in it, nor a rule of it carries an execution: what
/// is reported is the declaration the store holds, exactly as a stage's action states
/// report the declared actions by name.
pub fn condition_state(conditions: &[Condition]) -> StageConditionState {
    StageConditionState {
        latest_execution: None,
        condition_states: Some(
            conditions
                .iter()
                .map(|condition| ConditionState {
                    latest_execution: None,
                    // `Condition$rules` is optional, so a condition declaring no rule
                    // reports no rule states rather than an empty list — the same way
                    // a stage declaring no condition set reports none.
                    rule_states: condition.rules.as_ref().map(|rules| {
                        rules
                            .iter()
                            .map(|rule| RuleState {
                                rule_name: Some(rule.name.clone()),
                                current_revision: None,
                                latest_execution: None,
                                entity_url: None,
                                revision_url: None,
                            })
                            .collect()
                    }),
                })
                .collect(),
        ),
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
/// None of the rules is expressible as a Smithy constraint (`TagList` carries no
/// length bound, a list cannot require distinct keys, and `TagKey` carries no
/// `@pattern`):
///
/// * at most [`MAX_TAGS_PER_RESOURCE`] tags on one resource;
/// * one key at most once — a map from key to value cannot hold a key twice, so a
///   list naming one key twice does not describe a tag set;
/// * no key in AWS's reserved [`RESERVED_TAG_KEY_PREFIX`] namespace, which a caller
///   may not set, matched in any letter case.
///
/// Every operation that sets tags on a resource of this service applies it, so one
/// request is refused the same way whichever resource it addresses.
pub fn validate_tags(tags: &[Tag]) -> Result<(), TagProblem> {
    if tags.len() > MAX_TAGS_PER_RESOURCE {
        return Err(TagProblem::TooMany(format!(
            "tags must hold at most {MAX_TAGS_PER_RESOURCE} entries, but this request carries {}",
            tags.len()
        )));
    }
    let mut seen = BTreeSet::new();
    for tag in tags {
        if is_reserved_tag_key(&tag.key) {
            return Err(TagProblem::Invalid(format!(
                "tags names the key {} in the reserved {RESERVED_TAG_KEY_PREFIX} namespace",
                tag.key
            )));
        }
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

/// The jobs waiting for each custom action type's job worker, oldest first.
pub type JobQueue = BTreeMap<ActionTypeKey, Vec<JobRecord>>;

/// Queue `job` as the newest job waiting for the custom action type `id`, answering
/// whether the queue took it.
///
/// The ONE write path into a [`JobQueue`], so the owner rule and the
/// [`MAX_QUEUED_JOBS`] bound hold whatever queues a job: the state method
/// ([`ComAmazonawsCodepipelineCodepipeline20150709State::enqueue_job`]) and the
/// execution driver, which reaches only this field while it writes an execution's
/// record, both come through here. Past the bound the OLDEST queued job across every
/// action type is dropped, so a queue entry can never be added without one being
/// releasable.
pub fn enqueue_job_into(queue: &mut JobQueue, id: &ActionTypeId, job: JobRecord) -> bool {
    if id.owner != ACTION_OWNER_CUSTOM {
        return false;
    }
    queue
        .entry(ActionTypeKey::from_id(id))
        .or_default()
        .push(job);
    while queued_job_count(queue) > MAX_QUEUED_JOBS {
        // The oldest job of each action type is its queue's first entry (the queues are
        // kept oldest first), so the oldest job overall is the oldest of those.
        let oldest = queue
            .iter()
            .filter_map(|(key, jobs)| jobs.first().map(|job| (job.queued_at, key.clone())))
            .min();
        let Some((_, key)) = oldest else { break };
        if let Some(jobs) = queue.get_mut(&key) {
            jobs.remove(0);
            if jobs.is_empty() {
                queue.remove(&key);
            }
        }
    }
    true
}

/// How many jobs a [`JobQueue`] is holding, across every action type.
pub fn queued_job_count(queue: &JobQueue) -> usize {
    queue.values().map(Vec::len).sum()
}

/// How many jobs a poll asks for: `maxBatchSize` when it carries a count, and every
/// job waiting when the member is absent.
///
/// `MaxBatchSize` is modelled `range 1..` and the framework refuses anything below 1
/// before a handler runs, so the conversion cannot actually fail. It is mapped to 0 —
/// no jobs — rather than to "every job" deliberately: a cap that cannot be honoured
/// must never WIDEN the answer. Shared by `PollForJobs` and
/// `PollForThirdPartyJobs`, which model the same member.
pub fn poll_limit(max_batch_size: Option<i32>) -> usize {
    max_batch_size.map_or(usize::MAX, |size| usize::try_from(size).unwrap_or(0))
}

/// What `ActionTypeNotFoundException` states: the four identifying fields of the
/// action type the REQUEST named, in the order `ActionTypeId` declares them.
///
/// Built from the request, so it is a template over the caller's own values rather
/// than text naming one particular action type, and the caller can tell from it which
/// of the four fields it has to correct.
pub fn action_type_not_found_message(id: &ActionTypeId) -> String {
    format!(
        "ActionType (Category='{}', Owner='{}', Provider='{}', Version='{}') is not \
         registered in account '{}'",
        id.category,
        id.owner,
        id.provider,
        id.version,
        ::engine::emulator_account_id()
    )
}

/// One job waiting for a CUSTOM action's job worker — what `PollForJobs` hands out.
///
/// `PollForJobs` "is valid only for action types with Custom in the owner field":
/// CodePipeline does not run a custom action, the account's own job worker does. So
/// when an execution reaches such an action the execution plane queues this record and
/// the action stays `InProgress` until a worker reports its result; what the worker
/// needs to do the work (the action's configuration, the artifacts it consumes and the
/// ones it is to produce, the pipeline it belongs to) is captured here from the
/// declaration the execution pinned, so the job a poll reports is computed from stored
/// state rather than from the poll.
#[derive(Debug, Clone)]
pub struct JobRecord {
    /// The job's id, matching `JobId`'s modelled pattern
    /// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`).
    pub job_id: String,
    /// The custom action type whose worker this job waits for.
    pub action_type: ActionTypeId,
    /// The pipeline the action belongs to.
    pub pipeline_name: String,
    /// That pipeline's ARN — `JobData$pipelineContext` documents that it "includes
    /// pipelineArn and pipelineExecutionId for custom jobs".
    pub pipeline_arn: String,
    /// The execution that reached the action.
    pub pipeline_execution_id: String,
    /// The stage that contains the action.
    pub stage_name: String,
    /// The declared action's name.
    pub action_name: String,
    /// The id the action's execution is reported under.
    pub action_execution_id: String,
    /// Where the action sits in its execution's plan: the stage's index.
    pub stage: usize,
    /// Where the action sits in that stage: the action's index.
    pub action: usize,
    /// The action's `configuration`, with the execution's variables resolved — what
    /// `JobData$actionConfiguration` reports, and what a `queryParam` poll selects on.
    pub configuration: BTreeMap<String, String>,
    /// The artifacts supplied to the job, at the artifact-store locations the earlier
    /// actions produced them in.
    pub input_artifacts: Vec<Artifact>,
    /// The artifacts the job is to produce, at the artifact-store locations they are
    /// to be written to.
    pub output_artifacts: Vec<Artifact>,
    /// The customer managed key the pipeline's artifact store declares, when it
    /// declares one: the worker needs it to read and write those artifacts.
    pub encryption_key: Option<EncryptionKey>,
    /// A `JobStatus` value: where the job is between being queued and a worker
    /// reporting its outcome.
    pub status: String,
    /// The `nonce` the newest poll published for this job, or `None` while no poll
    /// has reported it. The worker presents it to `AcknowledgeJob`.
    pub nonce: Option<String>,
    /// When the claim the newest poll granted lapses, so the job may be reported to
    /// another worker again (see [`JOB_POLL_LEASE`]).
    pub leased_until: Option<SystemTime>,
    /// The continuation token the last result supplied, identifying a custom action
    /// still in progress.
    pub continuation_token: Option<String>,
    /// When the job was queued — the order the jobs of one action type are handed out
    /// in, and which job the [`MAX_QUEUED_JOBS`] bound drops first.
    pub queued_at: SystemTime,
    /// When this job's status last changed.
    pub last_status_change: SystemTime,
}

impl JobRecord {
    /// A job of `action_type` queued at `now`, with nothing of a pipeline recorded
    /// against it yet.
    ///
    /// The job starts at [`JOB_STATUS_QUEUED`] and unleased: it is waiting for a
    /// worker, and no worker owns its outcome until one polls for it and
    /// acknowledges it. The execution plane fills in the pipeline context it queues
    /// the job with.
    pub fn new(job_id: impl Into<String>, action_type: ActionTypeId, now: SystemTime) -> Self {
        Self {
            job_id: job_id.into(),
            action_type,
            pipeline_name: String::new(),
            pipeline_arn: String::new(),
            pipeline_execution_id: String::new(),
            stage_name: String::new(),
            action_name: String::new(),
            action_execution_id: String::new(),
            stage: 0,
            action: 0,
            configuration: BTreeMap::new(),
            input_artifacts: Vec::new(),
            output_artifacts: Vec::new(),
            encryption_key: None,
            status: JOB_STATUS_QUEUED.to_string(),
            nonce: None,
            leased_until: None,
            continuation_token: None,
            queued_at: now,
            last_status_change: now,
        }
    }

    /// This job as `PollForJobs` reports it to the worker it was just handed to.
    pub fn to_job(&self) -> Job {
        Job {
            id: Some(self.job_id.clone()),
            data: Some(self.to_data()),
            nonce: self.nonce.clone(),
            // "The ID of the Amazon Web Services account to use when performing the
            // job": the account the pipeline lives in, which is the one account this
            // emulator serves.
            account_id: Some(::engine::emulator_account_id().to_string()),
        }
    }

    /// This job as `GetJobDetails` reports it.
    ///
    /// The same job, read back: `JobDetails` carries everything `Job` does except the
    /// `nonce`, which is the claim one poll granted rather than a property of the job.
    pub fn to_details(&self) -> crate::ops::types::JobDetails {
        crate::ops::types::JobDetails {
            id: Some(self.job_id.clone()),
            data: Some(self.to_data()),
            account_id: Some(::engine::emulator_account_id().to_string()),
        }
    }

    /// What a worker needs to perform this job, as `JobData` models it.
    pub fn to_data(&self) -> JobData {
        JobData {
            action_type_id: Some(self.action_type.clone()),
            action_configuration: Some(ActionConfiguration {
                configuration: (!self.configuration.is_empty()).then(|| {
                    self.configuration
                        .iter()
                        .map(|(name, value)| (name.clone(), value.clone()))
                        .collect()
                }),
            }),
            pipeline_context: Some(PipelineContext {
                pipeline_name: Some(self.pipeline_name.clone()),
                stage: Some(StageContext {
                    name: Some(self.stage_name.clone()),
                }),
                action: Some(ActionContext {
                    name: Some(self.action_name.clone()),
                    action_execution_id: Some(self.action_execution_id.clone()),
                }),
                pipeline_arn: Some(self.pipeline_arn.clone()),
                pipeline_execution_id: Some(self.pipeline_execution_id.clone()),
            }),
            input_artifacts: optional_list(self.input_artifacts.clone()),
            output_artifacts: optional_list(self.output_artifacts.clone()),
            // `artifactCredentials` is an STS session SCOPED to the artifact bucket
            // ("temporary credentials that are issued by Amazon Web Services Secure
            // Token Service"). This emulator's credential store holds no policies, so
            // a session it minted would carry no scope at all — it would authorize
            // every request of every service, which is not what this member stands
            // for — and nothing would revoke it either: a job is released by paths
            // with no credential store in reach (its execution being evicted, its
            // pipeline deleted, the queue bound dropping it). So no credential is
            // invented here and the member is absent; a worker reaches the artifact
            // locations above with the credential it already signs its calls with.
            artifact_credentials: None,
            continuation_token: self.continuation_token.clone(),
            encryption_key: self.encryption_key.clone(),
        }
    }

    /// Whether this job may be handed to a polling worker at `now`: it is still
    /// waiting for one, and no claim on it is in force.
    pub fn is_available(&self, now: SystemTime) -> bool {
        self.status == JOB_STATUS_QUEUED
            && self.leased_until.is_none_or(|until| until <= now)
    }

    /// Hand this job to the worker that is polling: mint the `nonce` that worker
    /// presents to `AcknowledgeJob`, and hold the job for [`JOB_POLL_LEASE`].
    ///
    /// The nonce is minted per lease and is unpredictable, so the claim it grants
    /// cannot be taken by a worker that was never handed the job, and the nonce of a
    /// lease that lapsed no longer reaches it.
    pub fn lease(&mut self, now: SystemTime) {
        self.nonce = Some(random_uuid_v4());
        self.leased_until = Some(now + JOB_POLL_LEASE);
    }

    /// Whether this job's action configuration satisfies a poll's `queryParam` pair.
    ///
    /// `PollForJobs` documents it: "only jobs whose action configuration matches the
    /// mapped value are returned". A poll that supplied no pair selects every job.
    pub fn matches_query(&self, query: Option<(&str, &str)>) -> bool {
        query.is_none_or(|(name, value)| {
            self.configuration.get(name).map(String::as_str) == Some(value)
        })
    }
}

/// What `JobNotFoundException` states, for every operation that raises it: the id the
/// caller asked for, and nothing else about the store. One source, so the two job
/// planes cannot drift on what a missing job is reported as.
pub fn job_not_found_message(job_id: &str) -> String {
    format!("Job with id '{job_id}' does not exist.")
}

/// One artifact of a job, named and placed where its bytes live — or, for an artifact
/// the job is still to produce, where they are to be written.
///
/// `ArtifactLocation` is how `JobData` tells a worker where to read and write: the
/// artifact store is an S3 bucket, so the location is the bucket and object key the
/// execution plane assigned the artifact.
pub fn job_artifact(name: &str, location: &ArtifactRef) -> Artifact {
    Artifact {
        name: Some(name.to_string()),
        revision: location.revision.clone(),
        location: Some(ArtifactLocation {
            r#type: Some(ARTIFACT_LOCATION_TYPE_S3.to_string()),
            s3_location: Some(S3ArtifactLocation {
                bucket_name: location.bucket.clone(),
                object_key: location.key.clone(),
            }),
        }),
    }
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
    /// The job's current `Nonce`: "a system-generated random number that CodePipeline
    /// uses to ensure that the job is being worked on by only one job worker"
    /// (`AcknowledgeThirdPartyJob$nonce`), which a worker reads off
    /// `GetThirdPartyJobDetails` and presents to `AcknowledgeThirdPartyJob`. Minted
    /// with the job and well inside `Nonce`'s modelled 1..50 length, so a queued job
    /// always has exactly one value that admits a worker to it.
    pub nonce: String,
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
    /// reaches it, and so is the `nonce` that admits one worker to it.
    ///
    /// Both come from [`random_uuid_v4`], whose bits are hash-seed material rather
    /// than a CSPRNG's: they are FIXTURE-GRADE values that make one queued job's
    /// token and nonce distinct from another's, not a secret an attacker is kept out
    /// by. Nothing in this emulator is protected by them — the store is in-memory and
    /// per-process, and a real worker credential is minted by the emulator's
    /// credential store, never here — so the `clientToken`/`nonce` check they feed is
    /// the service's modelled behaviour, not an authorization boundary.
    pub fn new(job_id: impl Into<String>, client_id: impl Into<String>) -> Self {
        Self {
            job_id: job_id.into(),
            client_id: client_id.into(),
            client_token: random_uuid_v4(),
            nonce: random_uuid_v4(),
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

    /// Whether this job's work has ended ([`TERMINAL_JOB_STATUSES`]).
    pub fn is_terminal(&self) -> bool {
        TERMINAL_JOB_STATUSES.contains(&self.status.as_str())
    }

    /// Whether `nonce` is the value that admits a worker to this job.
    ///
    /// The job's current nonce, and only while the job is still live: the nonce is
    /// there to "ensure that the job is being worked on by only one job worker", and a
    /// job whose work has ended is being worked on by none, so no nonce admits it.
    pub fn admits_nonce(&self, nonce: &str) -> bool {
        !self.is_terminal() && self.nonce == nonce
    }

    /// Record that a job worker has confirmed receipt of this job, which is what
    /// `AcknowledgeThirdPartyJob` does: the job is now being worked on, so it carries
    /// [`JOB_STATUS_IN_PROGRESS`] and its outcome is that worker's to report
    /// ([`Self::accepts_result`]).
    ///
    /// Idempotent: the worker holding the nonce may repeat the acknowledgement, and a
    /// job already `InProgress` stays exactly where it is — including the instant it
    /// last changed, since nothing about it changed.
    pub fn acknowledge(&mut self, now: SystemTime) {
        if self.status != JOB_STATUS_IN_PROGRESS {
            self.status = JOB_STATUS_IN_PROGRESS.to_string();
            self.last_status_change = now;
        }
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
    /// The source revisions this execution was started with, plus the ones its source
    /// actions resolved as they ran.
    pub source_revisions: Vec<SourceRevision>,
    /// The stages and actions this execution runs, pinned from the declaration at the
    /// instant it started.
    pub stages: Vec<StageRun>,
    /// The artifact-store bucket this execution's artifacts are written to.
    pub artifact_bucket: String,
    /// The customer managed key the artifact store declares, when it declares one.
    pub artifact_encryption_key: Option<String>,
    /// True while the execution has not been admitted to run — the state a start takes
    /// in `QUEUED` and `SUPERSEDED` mode while another execution of the pipeline runs.
    pub queued: bool,
    /// The `StopPipelineExecution` request recorded against this execution, if any.
    pub stop: Option<StopRequest>,
    /// The artifacts produced so far, keyed by `OutputArtifact$name`.
    pub artifacts: BTreeMap<String, ArtifactRef>,
    /// The variables actions of this execution may resolve `#{namespace.key}`
    /// placeholders against: the pipeline variables under the `variables` namespace,
    /// plus each action's published output variables under its own namespace.
    pub variables_map: BTreeMap<String, String>,
    /// How many times `RetryStageExecution` has retried a stage of this execution.
    pub manual_retry_attempts: i32,
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
            artifact_revisions: (!self.artifacts.is_empty()).then(|| {
                self.artifacts
                    .iter()
                    .map(|(name, location)| crate::ops::types::ArtifactRevision {
                        name: Some(name.clone()),
                        revision_id: location.revision.clone(),
                        revision_change_identifier: None,
                        revision_summary: None,
                        created: None,
                        revision_url: None,
                    })
                    .collect()
            }),
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
            stop_trigger: self.stop.as_ref().map(|stop| crate::ops::types::StopExecutionTrigger {
                reason: stop.reason.clone(),
            }),
            execution_mode: Some(self.execution_mode.clone()),
            execution_type: Some(self.execution_type.clone()),
            rollback_metadata: None,
        }
    }
}

/// One stored action execution: an action of a pipeline execution that the execution
/// has STARTED, as `ListActionExecutions` reports it.
///
/// Every member is either read from the declaration the execution runs or recorded
/// when the action started; nothing about an outcome is held, because nothing in this
/// emulator runs an action (see [`ACTION_STATUS_IN_PROGRESS`]).
#[derive(Debug, Clone)]
pub struct ActionExecutionRecord {
    /// The generated id, modelled `ActionExecutionId`.
    pub action_execution_id: String,
    /// The execution this action execution belongs to.
    pub pipeline_execution_id: String,
    /// The pipeline whose execution it is, so the listing can select by pipeline.
    pub pipeline_name: String,
    /// The pipeline version the action ran at — the execution's version.
    pub pipeline_version: i32,
    /// The stage that contains the action.
    pub stage_name: String,
    /// The declared action's name.
    pub action_name: String,
    /// An `ActionExecutionStatus` value.
    pub status: String,
    /// When the action started.
    pub start_time: SystemTime,
    /// When this record last changed.
    pub last_update_time: SystemTime,
    /// The action's input, as its declaration configures it.
    pub input: ActionExecutionInput,
}

impl ActionExecutionRecord {
    /// The record for `action` of `stage` starting as part of `execution`.
    ///
    /// The input is the declaration's: the action's type id, its configured
    /// `configuration`, the role it assumes (its own `roleArn` when it declares one,
    /// else the pipeline's, which `ActionExecutionInput$roleArn` documents as "assumed
    /// through the roleArn for the pipeline"), its Region, its variable namespace and
    /// the input artifacts it consumes — each by the name the declaration gives it,
    /// with no `s3location`, because this emulator stores no artifact and so has no
    /// location to report for one.
    ///
    /// `output` and `updatedBy` are absent: the first is what the action produced,
    /// which nothing has produced yet, and the second is the identity of the user who
    /// last changed the execution, which this emulator does not resolve (the same
    /// reason `ExecutionTrigger$triggerDetail` is absent).
    fn start(
        action_execution_id: String,
        execution: &PipelineExecutionRecord,
        stage_name: &str,
        action: &ActionDeclaration,
        declaration: &PipelineDeclaration,
        pipeline_region: Option<String>,
    ) -> Self {
        let configuration = action.configuration.clone();
        Self {
            action_execution_id,
            pipeline_execution_id: execution.pipeline_execution_id.clone(),
            pipeline_name: execution.pipeline_name.clone(),
            pipeline_version: execution.pipeline_version,
            stage_name: stage_name.to_string(),
            action_name: action.name.clone(),
            status: ACTION_STATUS_IN_PROGRESS.to_string(),
            start_time: execution.start_time,
            last_update_time: execution.start_time,
            input: ActionExecutionInput {
                action_type_id: Some(action.action_type_id.clone()),
                resolved_configuration: configuration
                    .as_ref()
                    .and_then(|configuration| {
                        resolved_configuration(configuration, &execution.variables)
                    }),
                configuration,
                role_arn: Some(
                    action
                        .role_arn
                        .clone()
                        .unwrap_or_else(|| declaration.role_arn.clone()),
                ),
                region: action.region.clone().or(pipeline_region),
                input_artifacts: action.input_artifacts.as_ref().map(|artifacts| {
                    artifacts
                        .iter()
                        .map(|artifact| ArtifactDetail {
                            name: Some(artifact.name.clone()),
                            s3location: None,
                        })
                        .collect()
                }),
                namespace: action.namespace.clone(),
            },
        }
    }

    /// This action execution as `ListActionExecutions` reports it.
    pub fn to_detail(&self) -> ActionExecutionDetail {
        ActionExecutionDetail {
            pipeline_execution_id: Some(self.pipeline_execution_id.clone()),
            action_execution_id: Some(self.action_execution_id.clone()),
            pipeline_version: Some(self.pipeline_version),
            stage_name: Some(self.stage_name.clone()),
            action_name: Some(self.action_name.clone()),
            start_time: Some(to_date_time(self.start_time)),
            last_update_time: Some(to_date_time(self.last_update_time)),
            // The ARN of the user who last changed the execution: not resolved here,
            // so absent rather than invented.
            updated_by: None,
            status: Some(self.status.clone()),
            input: Some(self.input.clone()),
            // What the action produced. Nothing in this emulator runs an action, so
            // there is no result, no output artifact and no output variable to report.
            output: None,
        }
    }
}

/// The `runOrder` `action` runs in: the one it declares, else [`DEFAULT_RUN_ORDER`].
pub fn run_order(action: &ActionDeclaration) -> i32 {
    action.run_order.unwrap_or(DEFAULT_RUN_ORDER)
}

/// The most bytes one resolved configuration value may grow to before the
/// configuration is reported as having no resolved form.
///
/// Substitution is a SINGLE pass over the stored text ([`substitute_variables`]), so
/// the resolved value is bounded by the stored value's own modelled maximum
/// (`ActionConfigurationValue`, len 1..1000) times the longest replacement one
/// reference can carry (`PipelineVariableValue`, len 1..1000) — this product. Every
/// configuration AWS's own constraints allow therefore resolves, while the budget is
/// still checked as the text is built, so the allocation one request can drive is
/// bounded by a stated figure rather than by that arithmetic holding. Past it the
/// configuration has no resolved form, exactly as an unresolvable reference leaves it
/// with none.
pub const MAX_RESOLVED_CONFIGURATION_VALUE_BYTES: usize = 1_000 * 1_000;

/// `configuration` with every pipeline-variable reference replaced by the value this
/// execution resolved for that variable, or `None` when a reference is left that this
/// emulator cannot resolve.
///
/// CodePipeline's variable syntax is `#{namespace.variable}`, and a pipeline-level
/// variable — the kind `PipelineDeclaration$variables` declares and
/// `StartPipelineExecution` overrides — lives in the `variables` namespace. Those are
/// exactly the variables an execution record resolves ([`resolve_execution_variables`]),
/// so they are substituted here.
///
/// Every other namespace is an ACTION's: its values are the output variables the
/// action produced, and nothing in this emulator runs an action. A reference in the
/// STORED configuration that this emulator cannot resolve therefore leaves the
/// configuration with no resolved form at all, and
/// `ActionExecutionInput$resolvedConfiguration` is reported absent rather than
/// half-resolved. A configuration with no reference in it resolves to itself.
///
/// The resolved variables are indexed by name ONCE and each value is then resolved in
/// a single pass, so resolution is linear in the configuration's size — not one
/// `String::replace` over the accumulating text per declared variable, which re-scans
/// what the previous substitution produced and lets a request-supplied value that
/// looks like a reference expand transitively. A replacement's OWN text is therefore
/// never read as a reference: a variable whose resolved value is itself `#{...}` text
/// is substituted literally, so the reported `resolvedConfiguration` can carry that
/// text even though no reference of the stored configuration was left unresolved.
fn resolved_configuration(
    configuration: &HashMap<String, String>,
    variables: &[ResolvedPipelineVariable],
) -> Option<HashMap<String, String>> {
    let by_name: HashMap<&str, &str> = variables
        .iter()
        .filter_map(|variable| {
            Some((
                variable.name.as_deref()?,
                variable.resolved_value.as_deref()?,
            ))
        })
        .collect();

    let mut resolved = HashMap::with_capacity(configuration.len());
    for (key, value) in configuration {
        resolved.insert(key.clone(), substitute_variables(value, &by_name)?);
    }
    Some(resolved)
}

/// `value` with every `#{variables.<name>}` reference replaced by `by_name`'s entry
/// for that name, in ONE pass over `value`.
///
/// `None` — the configuration has no resolved form — when a reference is not one this
/// emulator can resolve (an unterminated `#{`, a namespace other than `variables`, or
/// a name this execution resolved no value for) or when the resolved text would pass
/// [`MAX_RESOLVED_CONFIGURATION_VALUE_BYTES`].
///
/// A replacement is never re-scanned: a variable whose value is itself `#{...}` text
/// is substituted literally, the way a single-pass substitution does, so no chain of
/// references can amplify one request into unbounded allocation.
fn substitute_variables(value: &str, by_name: &HashMap<&str, &str>) -> Option<String> {
    /// The namespace a pipeline-level variable's references carry.
    const VARIABLES_NAMESPACE: &str = "variables.";

    let mut resolved = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("#{") {
        let (before, reference) = (&rest[..start], &rest[start + 2..]);
        let end = reference.find('}')?;
        let name = reference[..end].strip_prefix(VARIABLES_NAMESPACE)?;
        let substitution = *by_name.get(name)?;
        if resolved.len() + before.len() + substitution.len()
            > MAX_RESOLVED_CONFIGURATION_VALUE_BYTES
        {
            return None;
        }
        resolved.push_str(before);
        resolved.push_str(substitution);
        rest = &reference[end + 1..];
    }
    (resolved.len() + rest.len() <= MAX_RESOLVED_CONFIGURATION_VALUE_BYTES).then(|| {
        resolved.push_str(rest);
        resolved
    })
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
/// them yields 128 bits that are distinct per value with no random-number dependency.
/// They are NOT cryptographically strong — std advances its per-thread seed
/// deterministically, so values minted in sequence are correlated rather than
/// independent. The version and variant nibbles are then set as RFC 4122 requires, so
/// the value is a well-formed v4 UUID rather than an arbitrary hex string. Every value
/// minted here names something for correlation (an execution, a job, a webhook URL,
/// the fixture-grade `clientToken`/`nonce` of a queued job) and none of them is a
/// credential this emulator protects anything with, so a non-cryptographic source is
/// the right one; a credential a caller is given is minted by the emulator's
/// credential store instead.
pub fn random_uuid_v4() -> String {
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

/// Whether IAM positively reports that `role_arn` names no role.
///
/// `PipelineDeclaration$roleArn` is the role CodePipeline assumes to run the pipeline's
/// actions, so a declaration naming a role that does not exist describes a pipeline that
/// cannot run — which is what `InvalidStructureException` reports. Shared by
/// `CreatePipeline` and `UpdatePipeline`, so a declaration one of them refuses is not
/// accepted by the other.
///
/// `true` only for IAM's own `NoSuchEntity`: any other answer (IAM not reachable, a
/// malformed ARN this emulator cannot resolve to a role name, a transport failure)
/// leaves the question unanswered, and an unanswered question is not a refusal.
pub async fn role_is_absent(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    role_arn: &str,
) -> bool {
    // An IAM role ARN is `arn:<partition>:iam::<account>:role/<path/><name>`, and
    // `GetRole` takes the NAME (the last path segment).
    let Some(role_name) = role_arn
        .split_once(":role/")
        .map(|(_, path)| path)
        .and_then(|path| path.rsplit('/').next())
        .filter(|name| !name.is_empty())
    else {
        return false;
    };
    let region = request_region(&ctx.parts().headers);
    let config = ::aws_sdk_iam::Config::builder()
        .behavior_version(::aws_sdk_iam::config::BehaviorVersion::latest())
        .region(::aws_sdk_iam::config::Region::new(region))
        // The endpoint is a FIXED literal (see [`in_process_endpoint`]): no
        // request-derived text reaches the host position, so nothing a caller sends can
        // re-point this call, which carries the emulator's own credentials. The
        // request's region is only the signing scope, never part of the URL.
        .endpoint_url(in_process_endpoint("iam"))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: nothing to retry, time out or watch for a
        // stalled stream, and each would need a sleep implementation this client is
        // not given.
        .retry_config(::aws_sdk_iam::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_iam::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_iam::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_iam::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    match ::aws_sdk_iam::Client::from_conf(config)
        .get_role()
        .role_name(role_name)
        .send()
        .await
    {
        Ok(_) => false,
        Err(err) => err
            .as_service_error()
            .is_some_and(::aws_sdk_iam::operation::get_role::GetRoleError::is_no_such_entity_exception),
    }
}

/// The endpoint URL an in-process call to another emulated service is addressed at.
///
/// The host is built from `service` and the FIXED [`DEFAULT_REGION`] literal, never from
/// request-derived text: the framework's callback routes such a call in process on the
/// service label, and the client carries the emulator's own credentials, so no value a
/// caller sends may reach the host position. A request's own region still decides what
/// the call is ABOUT (which artifact store, which ARN) and is the client's signing
/// scope; it is not part of the destination.
pub fn in_process_endpoint(service: &str) -> String {
    format!("https://{service}.{DEFAULT_REGION}.amazonaws.com")
}

/// The region to mint a resource's ARN in, and the signing scope of the in-process
/// calls this service makes: the one the request's SigV4 credential scope names, else
/// [`DEFAULT_REGION`].
///
/// The scope's region is accepted only when it is a region token
/// ([`is_region_token`]) — the same rule [`parse_resource_arn`] applies when reading
/// an ARN back. That makes "every ARN this service mints parses again" an invariant
/// rather than an assumption: a credential scope naming something that is not a
/// Region (an upper-case or punctuated string) would otherwise mint an ARN that no
/// ARN-addressed operation could ever resolve, so the request is answered in the
/// default Region instead. It is also what keeps this REQUEST-controlled text out of
/// anything that resolves a destination (see [`in_process_endpoint`], whose host is a
/// fixed literal).
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_token(region))
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

/// How many executions a pipeline whose `executionMode` is `execution_mode` runs
/// concurrently: one in `SUPERSEDED` mode, [`MAX_CONCURRENT_EXECUTIONS_PARALLEL`] in
/// the modes that hold several (`QUEUED`, `PARALLEL`).
///
/// An unrecognised value cannot reach here through the wire — `ExecutionMode` is a
/// modelled enum and the boundary validator refuses anything else — and is treated as
/// the default mode, which is what a declaration carrying no mode resolves to.
pub fn max_concurrent_executions(execution_mode: &str) -> usize {
    if execution_mode == EXECUTION_MODES[0] || execution_mode == EXECUTION_MODES[2] {
        MAX_CONCURRENT_EXECUTIONS_PARALLEL
    } else {
        MAX_CONCURRENT_EXECUTIONS_SUPERSEDED
    }
}

/// The variables an execution of `declaration` runs with: every variable the
/// declaration declares takes `overrides`'s value for it when there is one, else its
/// declared `defaultValue`.
///
/// `Err` carries the message the caller refuses the request with, for the two cases
/// that cannot be resolved at all:
///
/// * an override naming a variable the declaration does not carry — there is nothing
///   for it to override;
/// * a declared variable with neither a `defaultValue` nor an override — the
///   execution would have no value for a variable its pipeline declares.
///
/// Both are refused rather than quietly leaving a variable out of the execution, and
/// both are refused the same way whichever call starts the execution.
pub fn resolve_execution_variables(
    declaration: &PipelineDeclaration,
    overrides: &[PipelineVariable],
) -> Result<Vec<ResolvedPipelineVariable>, String> {
    let declared = declaration.variables.clone().unwrap_or_default();
    if let Some(unknown) = overrides
        .iter()
        .find(|variable| !declared.iter().any(|d| d.name == variable.name))
    {
        return Err(format!(
            "variables: pipeline {} declares no variable named {}",
            declaration.name, unknown.name
        ));
    }
    let mut variables: Vec<ResolvedPipelineVariable> = Vec::with_capacity(declared.len());
    for declared_variable in &declared {
        let Some(resolved) = overrides
            .iter()
            .find(|variable| variable.name == declared_variable.name)
            .map(|variable| variable.value.clone())
            .or_else(|| declared_variable.default_value.clone())
        else {
            return Err(format!(
                "variables: pipeline {} declares variable {} with no defaultValue, \
                 so a value must be given for it",
                declaration.name, declared_variable.name
            ));
        };
        variables.push(ResolvedPipelineVariable {
            name: Some(declared_variable.name.clone()),
            resolved_value: Some(resolved),
        });
    }
    Ok(variables)
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
    /// Refused as `LimitExceededException`: the declaration is past a quota rather than
    /// malformed — more stages, actions or configuration entries than this service runs.
    Limit(String),
}

impl DeclarationProblem {
    /// Why the declaration was refused. Built from the request, so the caller can
    /// fix the declaration from this text alone.
    pub fn message(&self) -> &str {
        match self {
            Self::Structure(message) | Self::Stage(message) | Self::Limit(message) => message,
        }
    }
}

/// The most stages one pipeline declaration may carry.
///
/// AWS's published CodePipeline quota ("Maximum number of stages in a pipeline: 50").
/// It is enforced BEFORE the declaration is walked, because every later check — and
/// every execution's retained plan — is sized by this count.
pub const MAX_STAGES_PER_PIPELINE: usize = 50;

/// The most actions one stage may carry: AWS's published quota ("Maximum number of
/// actions in a stage: 50").
pub const MAX_ACTIONS_PER_STAGE: usize = 50;

/// The most actions one pipeline may carry in total.
///
/// This emulator's OWN bound, not an AWS quota (AWS's two quotas above would allow
/// 2,500): every execution of the pipeline retains a plan with one entry per action, so
/// this figure times [`MAX_EXECUTION_HISTORY_PER_PIPELINE`] times
/// [`MAX_PIPELINES_PER_ACCOUNT`] is what bounds the execution store's memory.
pub const MAX_ACTIONS_PER_PIPELINE: usize = 100;

/// The most `configuration` entries one action may declare.
///
/// This emulator's own bound as well: `ActionConfigurationMap` carries no `@length` of
/// its own (only its key and value do, 1..50 and 1..1000), so without this an action's
/// configuration — which the validation walks and the handler stores — would be sized
/// only by the request body.
pub const MAX_CONFIGURATION_ENTRIES_PER_ACTION: usize = 100;

/// The `ActionCategory` of an action that brings a revision INTO the pipeline.
pub const CATEGORY_SOURCE: &str = "Source";

/// The message `PipelineNotFoundException` carries, for every operation that raises it.
///
/// The text is built from the request and the caller's account — the lookup is scoped
/// to one account, so the refusal names the account it did not find the pipeline in.
pub fn pipeline_not_found_message(name: &str) -> String {
    format!(
        "Account '{}' does not have a pipeline with name '{name}'",
        ::engine::emulator_account_id()
    )
}

/// The message `PipelineNameInUseException` carries.
///
/// Built the same way: the name the request asked for, and the account the collision
/// happened in.
pub fn pipeline_name_in_use_message(name: &str) -> String {
    format!(
        "A pipeline with the name '{name}' already exists in account '{}'",
        ::engine::emulator_account_id()
    )
}

/// Refuse a `PipelineDeclaration` whose structure cannot describe a pipeline.
///
/// Every rule here is one the model's own constraints cannot express, and each refuses
/// a declaration that could never execute:
///
/// * `CreatePipeline` documents "you must include either `artifactStore` or
///   `artifactStores` in your pipeline, but you cannot use both".
/// * `stages` and `StageDeclaration$actions` are `@required`, which only makes the
///   members mandatory — an EMPTY list satisfies the constraint, yet a pipeline
///   with no stage, or a stage with no action, has nothing to run.
/// * an `inputArtifacts` entry must name an artifact some EARLIER action outputs (an
///   earlier stage, or the same stage at a lower `runOrder`): an action cannot consume
///   an artifact nothing produces, and two actions cannot output the same name.
/// * the first stage is the pipeline's source stage: it carries at least one `Source`
///   action and nothing else, and no later stage carries a `Source` action.
/// * a declaration that states `pipelineType` `V1` cannot carry the V2-only members
///   `variables` or `triggers` — `PipelineDeclaration$pipelineType` documents that
///   "including V2 parameters … in the pipeline JSON" is what makes a pipeline V2, so
///   carrying them while declaring V1 is a contradiction.
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

    // The quotas come FIRST: every walk below is sized by the stage and action counts,
    // and so is the plan each execution of this pipeline retains, so a declaration past
    // these bounds is refused before any of that work is multiplied by it.
    if declaration.stages.len() > MAX_STAGES_PER_PIPELINE {
        return Err(DeclarationProblem::Limit(format!(
            "pipeline declares {} stages, past the limit of {MAX_STAGES_PER_PIPELINE} \
             stages for a pipeline",
            declaration.stages.len()
        )));
    }
    if let Some(stage) = declaration
        .stages
        .iter()
        .find(|stage| stage.actions.len() > MAX_ACTIONS_PER_STAGE)
    {
        return Err(DeclarationProblem::Limit(format!(
            "stage {} declares {} actions, past the limit of {MAX_ACTIONS_PER_STAGE} \
             actions for a stage",
            stage.name,
            stage.actions.len()
        )));
    }
    let actions: usize = declaration
        .stages
        .iter()
        .map(|stage| stage.actions.len())
        .sum();
    if actions > MAX_ACTIONS_PER_PIPELINE {
        return Err(DeclarationProblem::Limit(format!(
            "pipeline declares {actions} actions, past the limit of \
             {MAX_ACTIONS_PER_PIPELINE} actions for a pipeline"
        )));
    }
    if let Some((stage, action)) = declaration.stages.iter().find_map(|stage| {
        stage
            .actions
            .iter()
            .find(|action| {
                action
                    .configuration
                    .as_ref()
                    .is_some_and(|config| config.len() > MAX_CONFIGURATION_ENTRIES_PER_ACTION)
            })
            .map(|action| (stage, action))
    }) {
        return Err(DeclarationProblem::Limit(format!(
            "action {} in stage {} declares more than the limit of \
             {MAX_CONFIGURATION_ENTRIES_PER_ACTION} configuration entries for an action",
            action.name, stage.name
        )));
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

    // The first stage is the source stage: it carries at least one Source action and
    // nothing else, and no later stage carries one.
    let (first, rest) = declaration.stages.split_at(1);
    let first = &first[0];
    if !first
        .actions
        .iter()
        .any(|action| action.action_type_id.category == CATEGORY_SOURCE)
    {
        return Err(DeclarationProblem::Structure(format!(
            "the first stage of a pipeline must contain at least one {CATEGORY_SOURCE} \
             action, but stage {} contains none",
            first.name
        )));
    }
    if let Some(action) = first
        .actions
        .iter()
        .find(|action| action.action_type_id.category != CATEGORY_SOURCE)
    {
        return Err(DeclarationProblem::Structure(format!(
            "the first stage of a pipeline can contain only {CATEGORY_SOURCE} actions, \
             but action {} in stage {} is a {} action",
            action.name, first.name, action.action_type_id.category
        )));
    }
    for stage in rest {
        if let Some(action) = stage
            .actions
            .iter()
            .find(|action| action.action_type_id.category == CATEGORY_SOURCE)
        {
            return Err(DeclarationProblem::Structure(format!(
                "a {CATEGORY_SOURCE} action can only appear in the first stage, but \
                 action {} is in stage {}",
                action.name, stage.name
            )));
        }
    }

    // Every consumed artifact must be produced by an earlier action, and no name is
    // produced twice. LINEAR in the declaration: the actions of a stage are grouped by
    // `runOrder` in one pass and the produced names are a SET, so no part of this walk
    // rescans the stage or the names already seen.
    let mut produced: ::std::collections::BTreeSet<&str> = ::std::collections::BTreeSet::new();
    for stage in &declaration.stages {
        let mut groups: BTreeMap<i32, Vec<&crate::ops::types::ActionDeclaration>> =
            BTreeMap::new();
        for action in &stage.actions {
            groups
                .entry(action.run_order.unwrap_or(1))
                .or_default()
                .push(action);
        }
        for group in groups.values() {
            let mut outputs: Vec<&str> = Vec::new();
            for action in group {
                for input in action.input_artifacts.iter().flatten() {
                    if !produced.contains(input.name.as_str()) {
                        return Err(DeclarationProblem::Structure(format!(
                            "action {} in stage {} declares the input artifact {}, which \
                             no earlier action outputs",
                            action.name, stage.name, input.name
                        )));
                    }
                }
                for output in action.output_artifacts.iter().flatten() {
                    if produced.contains(output.name.as_str())
                        || outputs.contains(&output.name.as_str())
                    {
                        return Err(DeclarationProblem::Structure(format!(
                            "the output artifact {} is declared by more than one action; \
                             an artifact name is produced exactly once",
                            output.name
                        )));
                    }
                    outputs.push(output.name.as_str());
                }
            }
            produced.extend(outputs);
        }
    }

    // A declaration that states V1 cannot carry the V2-only members.
    if declaration.pipeline_type.as_deref() == Some(PIPELINE_TYPES[0]) {
        let v2_only = [
            (
                "variables",
                declaration
                    .variables
                    .as_ref()
                    .is_some_and(|variables| !variables.is_empty()),
            ),
            (
                "triggers",
                declaration
                    .triggers
                    .as_ref()
                    .is_some_and(|triggers| !triggers.is_empty()),
            ),
        ];
        if let Some((member, _)) = v2_only.iter().find(|(_, present)| *present) {
            return Err(DeclarationProblem::Structure(format!(
                "pipeline.{member} is only supported on a V2 pipeline, but this \
                 declaration states pipelineType {}",
                PIPELINE_TYPES[0]
            )));
        }
    }
    Ok(())
}

/// The cursor kind a `ListPipelines` token carries.
pub const CURSOR_PIPELINE: &str = "pipeline";

/// The cursor kind a `ListPipelineExecutions` token carries.
pub const CURSOR_EXECUTION: &str = "execution";

/// The cursor kind a `ListTagsForResource` token carries.
pub const CURSOR_TAG: &str = "tag";

/// The cursor kind a `ListWebhooks` token carries.
pub const CURSOR_WEBHOOK: &str = "webhook";

/// The cursor kind a `ListActionTypes` token carries.
pub const CURSOR_ACTION_TYPE: &str = "actionType";

/// The cursor kind a `ListActionExecutions` token carries.
pub const CURSOR_ACTION_EXECUTION: &str = "actionExecution";

/// An opaque pagination cursor naming the ordered key the next page starts at.
///
/// Base64 of `<kind>:<key>` (the encoding AWS's own opaque tokens use, from
/// `aws_smithy_types::base64`): opaque to the caller, round-trips exactly, and stays
/// inside `NextToken`'s modelled 1..2048 length for every key this service pages
/// over (a pipeline or webhook name is at most 100 characters, an execution id 36, a
/// `TagKey` 128, and an action type's four `ActionTypeId` members at most
/// 8 + 10 + 35 + 9). The kind prefix is what lets [`decode_page_token`] reject a
/// token this listing never issued, and it is unambiguous because no kind contains
/// `:` and [`decode_page_token`] splits on the FIRST one — so a key that itself
/// contains `:` (a `TagKey` may) still round-trips exactly.
///
/// Every listing applies ONE rule to a cursor: the page resumes at the first item
/// still in the listing's order at or after the key, and the token is refused with
/// `InvalidNextTokenException` only when it is not a cursor that listing issued or
/// no such position exists. They differ only in what their order makes possible:
/// `ListPipelines`, `ListTagsForResource`, `ListWebhooks` and `ListActionTypes` are
/// ordered BY the key, so an item deleted between pages still has a position (the
/// next key) and paging continues; `ListPipelineExecutions` is ordered by when each
/// execution started, so an id that is no longer retained has no position at all and
/// its cursor is refused.
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
