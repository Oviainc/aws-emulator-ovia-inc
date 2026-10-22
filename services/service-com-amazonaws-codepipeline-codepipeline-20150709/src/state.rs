// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codepipeline-codepipeline-20150709 service,
//! plus the small helpers every handler of this crate shares.
//!
//! The pipeline definition lifecycle (`CreatePipeline` / `GetPipeline` /
//! `ListPipelines` / `DeletePipeline`) and the execution records
//! `StartPipelineExecution` mints both live in the one store reached through
//! [`service_state`], so every handler computes its answer from persisted state
//! rather than from the request it is answering.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{
    ActionConfigurationProperty, ActionType, ActionTypeId, ActionTypeSettings, ArtifactDetails,
    ExecutionTrigger, PipelineDeclaration, PipelineExecution, PipelineExecutionSummary,
    PipelineMetadata, PipelineSummary, ResolvedPipelineVariable, SourceRevision, Tag,
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

/// The `ExecutionType` of an execution that runs the pipeline forward, from the
/// model's enum (`smithy_query query="shape" shape="ExecutionType"` -> `STANDARD`,
/// `ROLLBACK`).
pub const EXECUTION_TYPE_STANDARD: &str = "STANDARD";

/// The version number a freshly created pipeline carries.
///
/// `PipelineDeclaration$version` documents "A new pipeline always has a version
/// number of 1. This number is incremented when a pipeline is updated."
pub const FIRST_PIPELINE_VERSION: i32 = 1;

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
    pub action_types: BTreeMap<ActionTypeKey, ActionTypeRecord>,
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
        }
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
/// Both listings apply ONE rule to a cursor: the page resumes at the first item
/// still in the listing's order at or after the key, and the token is refused with
/// `InvalidNextTokenException` only when it is not a cursor that listing issued or
/// no such position exists. The two differ only in what their order makes possible:
/// `ListPipelines` is ordered BY the key, so a pipeline deleted between pages still
/// has a position (the next name) and paging continues; `ListPipelineExecutions` is
/// ordered by when each execution started, so an id that is no longer retained has
/// no position at all and its cursor is refused.
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
