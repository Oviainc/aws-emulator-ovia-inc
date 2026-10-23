// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sagemaker-sagemaker service.
//!
//! The inference-deployment lifecycle (model -> endpoint config -> endpoint), the
//! pipeline lifecycle, the model-card governance resources, batch-transform jobs and
//! the AI benchmarking control plane's workload configurations and benchmark jobs are
//! kept here, in
//! `BTreeMap`s keyed by the resource name AWS keys them by, so `ListModels` has one
//! stable order to sort and paginate over and every `Describe*` replays exactly what
//! the matching `Create*` stored. The Studio AppImageConfig registry, the Git
//! repositories `CreateCodeRepository` registers and the processing jobs
//! `CreateProcessingJob` accepts are kept the same way, keyed by `AppImageConfigName`,
//! `CodeRepositoryName` and `ProcessingJobName`.
//!
//! Studio apps live here too, keyed by the [`AppKey`] their five input members name
//! rather than by a bare name, because that tuple — domain, owning user profile or
//! space, app type, app name — is what identifies one app. The ML Lineage entities
//! (artifacts, actions, trial components) are keyed by the ARN or name their own slices
//! document, and the associations BETWEEN them by the `(SourceArn, DestinationArn)` pair
//! that is one edge's whole identity.
//!
//! # A job's status is moved by the CLOCK, never by a read
//!
//! A resource AWS returns in a non-terminal status (an endpoint in `Creating`, a
//! processing job in `InProgress`) owes its next move to the clock: the instant it falls
//! due is stored at create time and the reporting operations only materialise what is
//! already due ([`Endpoint::settle_status`], [`ProcessingJob::settle_status`]), so a read
//! never changes what a later caller sees and the transient status is observable because
//! the span is never zero.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AdditionalInferenceSpecificationDefinition, AppSpecification, ArtifactSource,
    AsyncInferenceConfig, BatchDataCaptureConfig, CodeEditorAppImageConfig, ContainerDefinition,
    DataCaptureConfig, DataProcessing, DriftCheckBaselines, ExperimentConfig, ExplainerConfig,
    GitConfig, IamIdentity, InferenceExecutionConfig, InferenceSpecification,
    JupyterLabAppImageConfig, KernelGatewayImageConfig, ManagedConfiguration, MetadataProperties,
    MetricsConfig, ModelCardSecurityConfig, ModelClientConfig, ModelLifeCycle, ModelMetrics,
    ModelPackageModelCard, ModelPackageSecurityConfig, ModelPackageStatusDetails,
    ModelPackageStatusItem, ModelPackageValidationSpecification, NetworkConfig,
    ParallelismConfiguration, Parameter, ProcessingInput, ProcessingOutputConfig,
    ProcessingResources, ProcessingStoppingCondition, ProductionVariant, ProductionVariantSummary,
    ResourceSpec, SelectiveExecutionConfig, SourceAlgorithmSpecification, Tag, TransformInput,
    TransformOutput, TransformResources, UserContext, VpcConfig,
};

/// The ARN service namespace (`arnNamespace` in the model's `aws.api#service` trait,
/// also SageMaker's sigv4 signing name).
pub const ARN_SERVICE: &str = "sagemaker";

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. It matches the region the emulator's generated shared config
/// hands the AWS CLI.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after `CreateEndpoint` the clock owes the `Creating` -> `InService`
/// transition. A caller that describes the endpoint straight after creating it still
/// observes `Creating`; one that waits (as `aws sagemaker wait endpoint-in-service`
/// does) observes `InService`, so a waiter terminates. Real SageMaker takes minutes;
/// the span is compressed, never zero.
pub const ENDPOINT_IN_SERVICE_AFTER: Duration = Duration::from_millis(300);

/// How long after `CreateMlflowApp` the clock owes the `Creating` -> `Created`
/// transition of `MlflowAppStatus`.
///
/// A caller that describes the app straight after creating it still observes
/// `Creating`; one that waits observes `Created`. Real SageMaker takes minutes to stand
/// an MLflow app up; the span is compressed, never zero, and the due instant is stored
/// rather than slept on.
pub const MLFLOW_APP_CREATED_AFTER: Duration = Duration::from_millis(300);

/// How long after `CreateApp` the clock owes the `Pending` -> `InService` transition.
///
/// Same contract as [`ENDPOINT_IN_SERVICE_AFTER`]: a caller that describes the app
/// straight after creating it still observes `Pending`, one that waits observes
/// `InService`. Real Studio app launches take minutes; the span is compressed, never
/// zero.
pub const APP_IN_SERVICE_AFTER: Duration = Duration::from_millis(300);

/// The most records each of this service's stores holds, after which a create is
/// refused with the modelled `ResourceLimitExceeded`.
///
/// SageMaker publishes adjustable per-account quotas on the resources this service
/// owns, and this emulator models neither Service Quotas nor an
/// account's adjustments — so this is the emulator's OWN bound on each store rather than
/// a claim about any account's limit. It sits far above any workflow an emulator serves,
/// and it is what stops the state growing without end: every record it holds is released
/// by the matching `Delete*`.
pub const MODEL_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const ENDPOINT_CONFIG_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const ENDPOINT_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. A transform job is terminal and has no delete, so this is the
/// most transform jobs one emulator run holds.
pub const TRANSFORM_JOB_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. `CreateMlflowApp` models `ResourceLimitExceeded` for exactly
/// this refusal.
pub const MLFLOW_APP_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. Counts model packages of both kinds (unversioned packages and
/// the versions registered into model package groups), which is what `CreateModelPackage`
/// mints and `DeleteModelPackage` releases.
pub const MODEL_PACKAGE_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const PIPELINE_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const APP_IMAGE_CONFIG_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const CODE_REPOSITORY_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const APP_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const MODEL_PACKAGE_GROUP_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. Bounds the lineage-artifact store the same way, and is what
/// `CreateArtifact` answers `ResourceLimitExceeded` past.
pub const ARTIFACT_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. Bounds the model-card store, and is what `CreateModelCard`
/// answers `ResourceLimitExceeded` past.
pub const MODEL_CARD_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. Bounds how many versions ONE model card can hold, and is what
/// `UpdateModelCard` answers `ResourceLimitExceeded` past: every content update appends
/// a version, so without a bound one card could grow the store without end.
pub const MODEL_CARD_VERSION_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const PROCESSING_JOB_QUOTA: usize = 1_000;

/// The lineage group every lineage entity this account creates belongs to.
///
/// SageMaker ML Lineage Tracking gives an account one lineage group per Region,
/// `sagemaker-default-lineage-group`, and reports its ARN as the `LineageGroupArn` of
/// each entity in it; this emulator models no other lineage group, so every artifact
/// reports this one.
pub const DEFAULT_LINEAGE_GROUP: &str = "sagemaker-default-lineage-group";

/// How long after `CreateProcessingJob` the clock owes the job's move out of
/// `InProgress` into its terminal status.
///
/// A span chosen by this emulator (AWS documents no duration for a processing run),
/// inside the 250–750 ms band the project fixes for a transition the clock owes. It is
/// never zero, so a caller that describes the job straight after creating it still
/// observes `InProgress`, and one that waits (as `aws sagemaker wait
/// processing-job-completed` does) observes the terminal status, so a waiter terminates.
pub const PROCESSING_JOB_ENDS_AFTER: Duration = Duration::from_millis(400);

/// `ProcessingJobStatus`'s members this emulator moves a processing job through (the
/// modelled enum also carries `Completed`, `Stopping` and `Stopped`, which this slice
/// does not reach). Named once here so no handler compares a bare string literal.
pub const PROCESSING_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`PROCESSING_STATUS_IN_PROGRESS`].
pub const PROCESSING_STATUS_FAILED: &str = "Failed";

/// `FailureReason` of a processing job whose container image could not be run here.
///
/// A processing job runs a customer Docker image (`AppSpecification$ImageUri`). This
/// emulator has no container runtime, and the engine contract
/// (`vendor/engine/src/lib.rs`) forbids reporting a status, duration or exit code for
/// work nothing ran — so the run the caller asked for ends in the modelled `Failed`
/// state with this reason, and no `ProcessingStartTime` or `ExitMessage` is invented.
/// Everything around the run — accepting the job, validating it, minting its ARN,
/// replaying it, reporting its terminal status — is real.
pub const NO_CONTAINER_RUNTIME_REASON: &str =
    "This emulator has no container runtime, so the processing job's container image was \
     never started";

/// The instance count a processing cluster runs when the request names none, per
/// `ProcessingClusterConfig$InstanceCount`'s doc: "The default value is 1."
pub const DEFAULT_PROCESSING_INSTANCE_COUNT: i32 = 1;

/// How long after `DeletePipeline` the clock owes the pipeline's disappearance.
///
/// `DeletePipeline` is asynchronous: the pipeline reports `PipelineStatus` `Deleting`
/// (one of the two members the modelled `PipelineStatus` enum carries) before it is
/// gone, so a caller that describes it straight after deleting it still observes that
/// transient status, and one that waits observes the not-found refusal. Real SageMaker
/// takes as long as it takes; the span is compressed, never zero.
pub const PIPELINE_DELETED_AFTER: Duration = Duration::from_millis(300);

/// `PipelineStatus`'s members (`smithy_query(query="shape",shape="PipelineStatus")`
/// answers exactly `Active` and `Deleting`). Named once here so no handler compares a
/// bare string literal.
pub const PIPELINE_STATUS_ACTIVE: &str = "Active";
/// See [`PIPELINE_STATUS_ACTIVE`].
pub const PIPELINE_STATUS_DELETING: &str = "Deleting";

/// The one pipeline-definition schema version SageMaker Pipelines publishes, and the
/// value a definition's required `Version` key carries.
pub const PIPELINE_DEFINITION_VERSION: &str = "2020-12-01";

/// The `Type` of the one step kind that holds steps of its own, in the branches the
/// schema gives it (`IfSteps` / `ElseSteps`). Every other step's `Arguments` are its own
/// payload, so nothing inside them is walked as a pipeline step.
const PIPELINE_CONDITION_STEP_TYPE: &str = "Condition";

/// The version id a pipeline carries from the moment it is created. `UpdatePipeline`
/// mints later ones; nothing here does yet, so a created pipeline has exactly this one.
pub const PIPELINE_INITIAL_VERSION: i64 = 1;

/// `PipelineExecutionStatus`'s members (`smithy_query(query="shape",
/// shape="PipelineExecutionStatus")` answers `Executing`, `Stopping`, `Stopped`,
/// `Failed`, `Succeeded` and `CompletedWithFailures`). Named here so no handler compares
/// a bare string literal; the two an execution this emulator starts ever reports are
/// the non-terminal one it begins in and the terminal one [`PIPELINE_EXECUTION_NOT_RUN`]
/// explains.
pub const PIPELINE_EXECUTION_STATUS_EXECUTING: &str = "Executing";
/// See [`PIPELINE_EXECUTION_STATUS_EXECUTING`].
pub const PIPELINE_EXECUTION_STATUS_FAILED: &str = "Failed";

/// The most steps one selective run may name, and the least.
///
/// `SelectedStepList` is modelled `len 1..50`
/// (`smithy_query(query="shape",shape="SelectiveExecutionConfig")`), so a longer or an
/// empty list is outside the model. The bound is restated here because it is what keeps
/// the step-membership check, and the size of the record the run persists, proportional
/// to the model rather than to the request's own length.
pub const MIN_SELECTED_STEPS: usize = 1;
/// See [`MIN_SELECTED_STEPS`].
pub const MAX_SELECTED_STEPS: usize = 50;

/// How long after `StartPipelineExecution` the clock owes the execution's terminal
/// status.
///
/// Same contract as [`PROCESSING_JOB_ENDS_AFTER`]: a caller that describes the execution
/// straight after starting it still observes `Executing`, one that waits observes the
/// terminal status. The span is the emulator's own, compressed and never zero, and the
/// due instant is stored rather than slept on.
pub const PIPELINE_EXECUTION_ENDS_AFTER: Duration = Duration::from_millis(400);

/// Why an execution this emulator started ends [`PIPELINE_EXECUTION_STATUS_FAILED`].
///
/// A pipeline's steps are training, processing, transform, Lambda and callback work run
/// outside SageMaker's control plane, and this emulator has no runtime that performs any
/// of it. So an accepted execution is never reported `Succeeded`: nothing ran its steps,
/// and a success status, a step list or an output artifact for work nothing performed
/// would be fabricated. The execution instead ends in the modelled failure state with
/// this reason, which says exactly that. Within the modelled
/// `PipelineExecutionFailureReason` length (0..1300).
pub const PIPELINE_EXECUTION_NOT_RUN: &str =
    "The pipeline execution was accepted but no step of it ran: this emulator has no \
     runtime that performs a pipeline step's work (training, processing, transform, \
     Lambda or callback), so the execution ended Failed rather than reporting work that \
     did not happen.";

/// See [`MODEL_QUOTA`]. Bounds the pipeline-execution store, so a caller starting
/// executions cannot grow it without end; `StartPipelineExecution` refuses past it with
/// the modelled `ResourceLimitExceeded`.
pub const PIPELINE_EXECUTION_QUOTA: usize = 1_000;

/// How deeply a pipeline definition's steps may nest (a condition step's branches hold
/// steps, which may themselves be condition steps). A bound, so a definition cannot
/// drive the step walk below into unbounded recursion.
const PIPELINE_MAX_STEP_DEPTH: usize = 32;

/// The most steps one pipeline definition may declare, counting the steps nested in a
/// condition step's branches, and the most `DependsOn` edges those steps may declare
/// between them.
///
/// `PipelineDefinition` is modelled `len 1..1048576`, and a megabyte of JSON can declare
/// tens of thousands of steps and edges — the two factors the dependency walk's work is
/// the product of. These are the emulator's OWN bounds on one definition rather than a
/// claim about any account's published quota: each sits far above any pipeline an
/// emulator serves, and together they are what keeps validating a definition short and
/// bounded however large the document is. A definition past either bound is refused
/// before any graph work is done.
const PIPELINE_MAX_STEPS: usize = 512;
/// See [`PIPELINE_MAX_STEPS`].
const PIPELINE_MAX_STEP_DEPENDENCIES: usize = 4_096;

/// [`PIPELINE_MAX_STEPS`] and [`PIPELINE_MAX_STEP_DEPENDENCIES`], so a test can pin the
/// bound at the value this module enforces rather than repeating a literal.
pub const fn pipeline_definition_bounds() -> (usize, usize) {
    (PIPELINE_MAX_STEPS, PIPELINE_MAX_STEP_DEPENDENCIES)
}

/// The most records each of this service's paginated listings returns in one page,
/// including when `MaxResults` is absent or larger.
///
/// `MaxResults` is modelled `range 1..100` on these operations, so this is the service's
/// own server-side ceiling: one call can never serialise the whole store, and a caller
/// that wants the rest follows `NextToken`. Matches the ceiling the other paginated
/// services in this workspace apply.
pub const MAX_PAGE_SIZE: i32 = 50;

/// `SortBy`'s documented default on this service's listings ("Sorts the list of results.
/// The default is `CreationTime`." — the member doc on both `ListModels` and
/// `ListEndpoints`).
pub const DEFAULT_SORT_BY: &str = "CreationTime";

/// `SortOrder`'s documented default ("The sort order for results. The default is
/// `Descending`.").
pub const DEFAULT_SORT_ORDER: &str = ORDER_DESCENDING;

/// `OrderKey`'s descending member, named once here so no handler decides a sort
/// direction by comparing a bare string literal, nor by assuming the documented default
/// is the descending one.
pub const ORDER_DESCENDING: &str = "Descending";

/// A page boundary as an opaque `NextToken`: the hex of the resource name the page
/// stopped at.
///
/// The token names the item the page stopped at rather than an offset, so resuming lands
/// exactly after it even though the token is opaque to the caller. Stated once here so
/// every paginated listing in this service mints and reads one token format.
pub fn encode_page_token(resource_name: &str) -> String {
    hex::encode(resource_name.as_bytes())
}

/// The resource name a `NextToken` names, or `None` when the token was not minted by
/// [`encode_page_token`].
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// `MlflowAppStatus`'s members this emulator moves an MLflow app through (the modelled
/// enum also carries the update, delete and failure states, which creating one does not
/// reach). Named once here so no handler compares a bare string literal.
pub const MLFLOW_APP_CREATING: &str = "Creating";
/// See [`MLFLOW_APP_CREATING`].
pub const MLFLOW_APP_CREATED: &str = "Created";

/// `ModelRegistrationMode`'s member `CreateMlflowApp` stores when the request names
/// none: the operation's model doc states "If not specified, AutomaticModelRegistration
/// defaults to AutoModelRegistrationDisabled".
pub const AUTO_MODEL_REGISTRATION_DISABLED: &str = "AutoModelRegistrationDisabled";

/// `EndpointStatus`'s and `ClusterStatus`'s members this emulator moves a resource
/// through (both modelled enums also carry the update, rollback and delete states,
/// which this slice does not reach). Named once here so no handler compares a bare
/// string literal.
pub const STATUS_CREATING: &str = "Creating";
/// See [`STATUS_CREATING`].
pub const STATUS_IN_SERVICE: &str = "InService";

/// `TransformJobStatus`'s members this emulator moves a transform job through (the
/// modelled enum also carries `Completed`, `Stopping` and `Stopped`, which this slice
/// does not reach). Named once here so no handler compares a bare string literal.
pub const TRANSFORM_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`TRANSFORM_STATUS_IN_PROGRESS`].
pub const TRANSFORM_STATUS_FAILED: &str = "Failed";

/// How long after `CreateTransformJob` the clock owes the job's terminal status.
///
/// A caller that describes the job straight after creating it still observes
/// `InProgress`; one that waits (as `aws sagemaker wait transform-job-completed-or-stopped`
/// does) observes the terminal status. Real SageMaker takes minutes to provision the ML
/// instances; the span is compressed, never zero.
pub const TRANSFORM_JOB_SETTLES_AFTER: Duration = Duration::from_millis(300);

/// Why a batch transform job this emulator accepted ends in the modelled `Failed`
/// status.
///
/// A batch transform runs the model's inference container against every record of the
/// input dataset; this emulator runs no container, so no inference is performed and no
/// transformed object is written. Reporting `Completed` — with an `S3OutputPath` nothing
/// wrote — would claim work that never ran, so the job ends in the status the model
/// carries for a job that did not produce its output, with this reason saying why.
pub const TRANSFORM_NOT_RUN_FAILURE: &str =
    "This emulator runs no model inference container, so no records were transformed and \
     no output object was written to the job's S3OutputPath.";

/// `MaxPayloadInMB`'s documented ceiling: "The value of MaxPayloadInMB cannot be greater
/// than 100 MB" and "the value of (MaxConcurrentTransforms * MaxPayloadInMB) also cannot
/// exceed 100 MB" (`CreateTransformJob$MaxPayloadInMB`'s model doc). The model constrains
/// the member only from below (`range 0..`), so the ceiling is enforced in the handler.
pub const MAX_PAYLOAD_IN_MB_CEILING: i32 = 100;

/// `MaxPayloadInMB` when the request leaves it unset: "The default value is 6 MB"
/// (`CreateTransformJob$MaxPayloadInMB`'s model doc).
pub const DEFAULT_MAX_PAYLOAD_IN_MB: i32 = 6;

/// `MaxConcurrentTransforms` when the request leaves it unset or zero: "If
/// MaxConcurrentTransforms is set to 0 or left unset, Amazon SageMaker checks the optional
/// execution-parameters to determine the settings for your chosen algorithm. If the
/// execution-parameters endpoint is not enabled, the default value is 1"
/// (`CreateTransformJob$MaxConcurrentTransforms`'s model doc). This emulator serves no
/// execution-parameters endpoint, so the documented fallback of 1 is what it resolves.
pub const DEFAULT_MAX_CONCURRENT_TRANSFORMS: i32 = 1;

/// `CompressionType` and `SplitType` when the request leaves them unset: "The default
/// value is `None`" (`TransformInput$CompressionType`) and "The default value for
/// SplitType is `None`" (`TransformInput$SplitType`).
pub const DEFAULT_NONE: &str = "None";

/// How long after `CreateModelPackage` the clock owes the `Pending` -> `Completed`
/// transition of `ModelPackageStatus`.
///
/// `CreateModelPackage` answers with an ARN while registration (and any validation the
/// request asked for) is still outstanding, so a caller that describes the package
/// straight away still observes `Pending`, and one that waits observes `Completed` —
/// which is what lets a waiter terminate. Real SageMaker takes anywhere from seconds to
/// minutes depending on the validation profiles; the span is compressed, never zero.
pub const MODEL_PACKAGE_COMPLETED_AFTER: Duration = Duration::from_millis(300);

/// The `ModelPackageStatus` members this emulator moves a model package through (the
/// modelled enum also carries `InProgress`, `Failed` and `Deleting`, which this slice
/// does not reach). Named once here so no handler compares a bare string literal, and
/// separately from [`STATUS_COMPLETED`], which is a `ModelPackageGroupStatus` member.
pub const MODEL_PACKAGE_STATUS_PENDING: &str = "Pending";
/// See [`MODEL_PACKAGE_STATUS_PENDING`].
pub const MODEL_PACKAGE_STATUS_COMPLETED: &str = "Completed";

/// The `DetailedModelPackageStatus` members a per-profile validation item reports while
/// the package is `Pending` and once it is `Completed`.
pub const DETAILED_MODEL_PACKAGE_STATUS_NOT_STARTED: &str = "NotStarted";
/// See [`DETAILED_MODEL_PACKAGE_STATUS_NOT_STARTED`].
pub const DETAILED_MODEL_PACKAGE_STATUS_COMPLETED: &str = "Completed";

/// The `ModelApprovalStatus` a registered model version carries when the request named
/// none: SageMaker's model registry documents a new version as awaiting manual approval,
/// and `CreateModelPackage` documents the member as optional for versioned models.
pub const MODEL_APPROVAL_PENDING_MANUAL: &str = "PendingManualApproval";

/// `AppStatus`'s members this emulator moves an app through. The modelled enum
/// (`smithy_query(query="shape", shape="AppStatus")`) also carries `Deleting`,
/// `Deleted` and `Failed`, which this slice does not reach: `DeleteApp` releases the
/// record, and nothing here fails a launch.
pub const APP_STATUS_PENDING: &str = "Pending";
/// See [`APP_STATUS_PENDING`].
pub const APP_STATUS_IN_SERVICE: &str = "InService";

/// The `ModelPackageGroupStatus` a model package group carries once `CreateModelPackageGroup`
/// has returned.
///
/// A model group is a registry container, not a provisioned resource: the call that creates
/// it is synchronous, and the group is immediately usable (a model package can be registered
/// into it on the very next call). So it is born in this TERMINAL status and the clock owes
/// it no later transition — the modelled enum's `Pending`/`InProgress` describe a group
/// whose creation is still in flight, which never happens here.
pub const STATUS_COMPLETED: &str = "Completed";

// ---------------------------------------------------------------------------
// The hub slice: the records `ListHubContents` reports. Kept here, beside the first
// resource record rather than at either end of the module, so it does not sit in the
// region every new resource slice is appended to.
// ---------------------------------------------------------------------------

/// A hub, as `DescribeHub` replays it and `ListHubContents` resolves it.
///
/// Only the hub's identity and lifecycle live here: `CreateHub` (its own open issue)
/// extends this record with the description, display name, search keywords and S3
/// storage configuration it stores.
#[derive(Debug, Clone)]
pub struct Hub {
    pub name: String,
    pub arn: String,
    /// A `HubStatus` member (the model's enum: `InService`, `Creating`, `Updating`,
    /// `Deleting`, `CreateFailed`, `UpdateFailed`, `DeleteFailed`).
    pub status: String,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

/// The identity of ONE stored hub content version.
///
/// A hub content is identified by its hub, its `HubContentType` and its
/// `HubContentName` — the three members every hub-content operation requires — and
/// versioned by `HubContentVersion`. The version is held BOTH as its three parsed dotted
/// components (see [`version_key`]), so the map orders `1.10.0` AFTER `1.9.0`, and
/// verbatim, so two versions whose components parse the same are still two records rather
/// than one overwriting the other.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct HubContentKey {
    pub hub_name: String,
    pub hub_content_type: String,
    pub hub_content_name: String,
    pub version: (u32, u32, u32),
    /// `HubContentVersion` verbatim — the tie-break that keeps the identity total.
    pub version_text: String,
}

/// One version of a hub content, as `ListHubContents` summarises it.
///
/// The members are `HubContentInfo`'s (the summary shape `ListHubContents` returns);
/// `ImportHubContent`, `CreateHubContentReference` and `DescribeHubContent` (each its own
/// open issue) extend this record with the document, markdown and dependencies they store.
#[derive(Debug, Clone)]
pub struct HubContent {
    pub name: String,
    pub arn: String,
    /// A `HubContentType` member: `Model`, `Notebook`, `ModelReference`, `DataSet` or
    /// `JsonDoc`.
    pub content_type: String,
    /// `HubContentVersion`, verbatim as stored, e.g. `1.0.0`.
    pub version: String,
    pub document_schema_version: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    /// A `HubContentSupportStatus` member: `Supported`, `Deprecated` or `Restricted`.
    pub support_status: Option<String>,
    pub search_keywords: Option<Vec<String>>,
    /// A `HubContentStatus` member: `Available`, `Importing`, `Deleting`,
    /// `ImportFailed`, `DeleteFailed`, `PendingImport` or `PendingDelete`.
    pub status: String,
    /// When THIS version was created.
    pub creation_time: SystemTime,
    /// When the content was first created, before any later version replaced it; absent
    /// on a content that was never revised.
    pub original_creation_time: Option<SystemTime>,
    /// The public hub content this one references, for a `ModelReference`.
    pub sage_maker_public_hub_content_arn: Option<String>,
}

/// A dotted `<major>.<minor>.<patch>` version (`HubContentVersion`,
/// `DocumentSchemaVersion`, both `^\d{1,4}.\d{1,4}.\d{1,4}$`) as a comparable key, so
/// `1.10.0` orders ABOVE `1.9.0` rather than below it as the strings would.
///
/// A component that is not a number counts as 0: the model refuses a malformed version
/// before a handler sees it, so this only keeps the ordering total.
pub fn version_key(version: &str) -> (u32, u32, u32) {
    let mut components = version.split('.').map(|part| part.parse::<u32>().unwrap_or(0));
    (
        components.next().unwrap_or(0),
        components.next().unwrap_or(0),
        components.next().unwrap_or(0),
    )
}

/// A model created by `CreateModel`, as `DescribeModel` and `ListModels` replay it.
#[derive(Debug, Clone)]
pub struct Model {
    pub name: String,
    pub arn: String,
    pub primary_container: Option<ContainerDefinition>,
    pub containers: Option<Vec<ContainerDefinition>>,
    pub inference_execution_config: Option<InferenceExecutionConfig>,
    pub execution_role_arn: Option<String>,
    pub vpc_config: Option<VpcConfig>,
    pub enable_network_isolation: Option<bool>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
}

/// An endpoint configuration created by `CreateEndpointConfig`, as
/// `DescribeEndpointConfig` replays it and `CreateEndpoint` reads it.
#[derive(Debug, Clone)]
pub struct EndpointConfig {
    pub name: String,
    pub arn: String,
    pub production_variants: Vec<ProductionVariant>,
    pub shadow_production_variants: Option<Vec<ProductionVariant>>,
    pub data_capture_config: Option<DataCaptureConfig>,
    pub async_inference_config: Option<AsyncInferenceConfig>,
    pub explainer_config: Option<ExplainerConfig>,
    pub metrics_config: Option<MetricsConfig>,
    pub kms_key_id: Option<String>,
    pub execution_role_arn: Option<String>,
    pub vpc_config: Option<VpcConfig>,
    pub enable_network_isolation: Option<bool>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
}

/// An endpoint created by `CreateEndpoint`, as `DescribeEndpoint` replays it.
///
/// `in_service_at` is the instant the clock owes the `Creating` -> `InService`
/// transition; it is stored, never slept on, and materialised by whichever reporting
/// call first runs at or after it.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub name: String,
    pub arn: String,
    pub config_name: String,
    pub status: String,
    pub failure_reason: Option<String>,
    pub production_variants: Vec<ProductionVariantSummary>,
    pub shadow_production_variants: Vec<ProductionVariantSummary>,
    pub data_capture_config: Option<DataCaptureConfig>,
    pub async_inference_config: Option<AsyncInferenceConfig>,
    pub explainer_config: Option<ExplainerConfig>,
    pub metrics_config: Option<MetricsConfig>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub in_service_at: SystemTime,
}

impl Endpoint {
    /// Materialise the `Creating` -> `InService` transition the clock already owes.
    ///
    /// This is the one write a reporting operation may make: the transition was due at
    /// `in_service_at` whether or not anybody called, so the observer makes no
    /// difference to the result. `LastModifiedTime` moves to the instant the
    /// transition was due, not to "now", so the value is stable across later reads.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == STATUS_CREATING && now >= self.in_service_at {
            self.status = STATUS_IN_SERVICE.to_string();
            self.last_modified_time = self.in_service_at;
            return true;
        }
        false
    }
}

/// A processing job created by `CreateProcessingJob`, as `DescribeProcessingJob`
/// replays it.
///
/// `ends_at` is the instant the clock owes the move out of `InProgress`; it is stored,
/// never slept on, and materialised by whichever reporting call first runs at or after
/// it (see [`ProcessingJob::settle_status`]).
#[derive(Debug, Clone)]
pub struct ProcessingJob {
    pub name: String,
    pub arn: String,
    pub role_arn: String,
    pub app_specification: AppSpecification,
    pub processing_inputs: Option<Vec<ProcessingInput>>,
    pub processing_output_config: Option<ProcessingOutputConfig>,
    /// The requested resources with the documented defaults applied and, when the
    /// request named `InstancePreferences`, the instance type the service selected
    /// from them.
    pub processing_resources: ProcessingResources,
    pub stopping_condition: Option<ProcessingStoppingCondition>,
    pub environment: Option<std::collections::HashMap<String, String>>,
    pub network_config: Option<NetworkConfig>,
    pub experiment_config: Option<ExperimentConfig>,
    pub tags: Vec<Tag>,
    pub status: String,
    pub failure_reason: Option<String>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub processing_end_time: Option<SystemTime>,
    pub ends_at: SystemTime,
}

impl ProcessingJob {
    /// Materialise the move out of `InProgress` the clock already owes.
    ///
    /// This is the one write a reporting operation may make: the move was due at
    /// `ends_at` whether or not anybody described the job, so the observer makes no
    /// difference to the result. `LastModifiedTime` and `ProcessingEndTime` take the
    /// instant the move was due, not "now", so both are stable across later reads.
    ///
    /// The terminal status is [`PROCESSING_STATUS_FAILED`] with
    /// [`NO_CONTAINER_RUNTIME_REASON`]: nothing ran the job's container image, so no
    /// `Completed` status, `ProcessingStartTime` or `ExitMessage` is invented for it.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == PROCESSING_STATUS_IN_PROGRESS && now >= self.ends_at {
            self.status = PROCESSING_STATUS_FAILED.to_string();
            self.failure_reason = Some(NO_CONTAINER_RUNTIME_REASON.to_string());
            // The instant the service gave up on the run, which is when the job
            // ended. No start time is recorded beside it: the container never started.
            self.processing_end_time = Some(self.ends_at);
            self.last_modified_time = self.ends_at;
            return true;
        }
        false
    }
}

/// A pipeline created by `CreatePipeline`, as `DescribePipeline` replays it and
/// `DeletePipeline` retires it.
///
/// `removal_due_at` is the instant the clock owes the record's disappearance after a
/// `DeletePipeline`; it is stored, never slept on, and materialised by
/// [`ComAmazonawsSagemakerSagemakerState::settle_pipeline_deletions`].
#[derive(Debug, Clone)]
pub struct Pipeline {
    pub name: String,
    pub arn: String,
    pub display_name: Option<String>,
    pub definition: String,
    pub description: Option<String>,
    pub role_arn: String,
    pub status: String,
    pub parallelism_configuration: Option<ParallelismConfiguration>,
    pub tags: Vec<Tag>,
    /// The `ClientRequestToken` the creating call presented, so a retry of that very
    /// call is completed "no more than one time" as the member documents.
    pub client_request_token: String,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    /// When an execution of this pipeline last started, which is what
    /// `StartPipelineExecution` stamps; `None` until one has.
    pub last_run_time: Option<SystemTime>,
    pub removal_due_at: Option<SystemTime>,
}

/// One execution of a pipeline, started by `StartPipelineExecution` and replayed by
/// `DescribePipelineExecution`.
///
/// Keyed in the store by its own ARN (`<pipeline arn>/execution/<id>`), the only
/// identifier `DescribePipelineExecution` resolves one by, the way the lineage stores
/// here are keyed by the ARN their create mints.
///
/// `ends_at` is the instant the clock owes the execution's terminal status; it is
/// stored, never slept on, and materialised by [`PipelineExecution::settle_status`].
#[derive(Debug, Clone)]
pub struct PipelineExecution {
    pub arn: String,
    /// The `PipelineName` this execution belongs to — the pipeline store's key, so the
    /// record is found again when the pipeline is deleted or the pipeline's executions
    /// are enumerated.
    pub pipeline_name: String,
    pub pipeline_arn: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub status: String,
    pub failure_reason: Option<String>,
    /// The parameter values the start call supplied, each naming a parameter the stored
    /// definition declares.
    pub parameters: Vec<Parameter>,
    pub parallelism_configuration: Option<ParallelismConfiguration>,
    pub selective_execution_config: Option<SelectiveExecutionConfig>,
    pub pipeline_version_id: i64,
    /// The `MlflowExperimentName` the start call supplied, which is the only member of
    /// the reported `MLflowConfig` this emulator knows: it serves no MLflow tracking
    /// server, so no resource ARN is invented beside it.
    pub mlflow_experiment_name: Option<String>,
    /// The `ClientRequestToken` the starting call presented, so a retry of that very
    /// call completes "no more than one time" as the member documents.
    pub client_request_token: String,
    pub created_by: UserContext,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub ends_at: SystemTime,
}

impl PipelineExecution {
    /// Materialise the move out of `Executing` the clock already owes.
    ///
    /// This is the one write a reporting operation may make: the move was due at
    /// `ends_at` whether or not anybody described the execution, so the observer makes
    /// no difference to the result. `LastModifiedTime` takes the instant the move was
    /// due, not "now", so it is stable across later reads.
    ///
    /// The terminal status is [`PIPELINE_EXECUTION_STATUS_FAILED`] with
    /// [`PIPELINE_EXECUTION_NOT_RUN`]: see that constant for why an execution this
    /// emulator accepted cannot end `Succeeded`.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == PIPELINE_EXECUTION_STATUS_EXECUTING && now >= self.ends_at {
            self.status = PIPELINE_EXECUTION_STATUS_FAILED.to_string();
            self.failure_reason = Some(PIPELINE_EXECUTION_NOT_RUN.to_string());
            self.last_modified_time = self.ends_at;
            return true;
        }
        false
    }
}

/// A batch transform job created by `CreateTransformJob`, as `DescribeTransformJob`
/// replays it.
///
/// `ends_at` is the instant the clock owes the job's terminal status; it is stored,
/// never slept on, and materialised by whichever reporting call first runs at or after
/// it.
#[derive(Debug, Clone)]
pub struct TransformJob {
    pub name: String,
    pub arn: String,
    pub model_name: String,
    pub status: String,
    pub failure_reason: Option<String>,
    pub max_concurrent_transforms: i32,
    pub model_client_config: Option<ModelClientConfig>,
    pub max_payload_in_mb: i32,
    pub batch_strategy: Option<String>,
    pub environment: Option<::std::collections::HashMap<String, String>>,
    pub transform_input: TransformInput,
    pub transform_output: TransformOutput,
    pub data_capture_config: Option<BatchDataCaptureConfig>,
    pub transform_resources: TransformResources,
    pub data_processing: Option<DataProcessing>,
    pub experiment_config: Option<ExperimentConfig>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub transform_end_time: Option<SystemTime>,
    pub ends_at: SystemTime,
}

impl TransformJob {
    /// Materialise the terminal status the clock already owes.
    ///
    /// This is the one write a reporting operation may make: the job was due to end at
    /// `ends_at` whether or not anybody called, so the observer makes no difference to
    /// the result. `TransformEndTime` is the instant the end was due, not "now", so the
    /// value is stable across later reads.
    ///
    /// The status is [`TRANSFORM_STATUS_FAILED`] with [`TRANSFORM_NOT_RUN_FAILURE`]: see
    /// that constant for why a job this emulator accepted cannot end `Completed`.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == TRANSFORM_STATUS_IN_PROGRESS && now >= self.ends_at {
            self.status = TRANSFORM_STATUS_FAILED.to_string();
            self.failure_reason = Some(TRANSFORM_NOT_RUN_FAILURE.to_string());
            self.transform_end_time = Some(self.ends_at);
            return true;
        }
        false
    }
}

/// An MLflow app created by `CreateMlflowApp`, as `DescribeMlflowApp` replays it.
///
/// `created_at` is the instant the clock owes the `Creating` -> `Created` transition;
/// like an endpoint's, it is stored and materialised by whichever reporting call first
/// runs at or after it.
///
/// Every member mirrors what the create request carried, so a describe replays the
/// request rather than a value invented on the read path. `model_registration_mode` is
/// the one member with a DOCUMENTED default, so it is stored resolved; the others stay
/// `None` when the request named none, because the model documents no default for them.
///
/// `created_by` is stored at creation (through [`caller_user_context`], as
/// `CreateModelPackageGroup` does) rather than rebuilt per read, so every later read
/// reports the identity that actually created the app.
#[derive(Debug, Clone)]
pub struct MlflowApp {
    pub arn: String,
    pub name: String,
    pub artifact_store_uri: String,
    pub role_arn: String,
    pub kms_key_id: Option<String>,
    pub model_registration_mode: String,
    pub weekly_maintenance_window_start: Option<String>,
    pub account_default_status: Option<String>,
    pub default_domain_id_list: Option<Vec<String>>,
    pub tags: Vec<Tag>,
    pub status: String,
    pub created_by: UserContext,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub created_at: SystemTime,
}

impl MlflowApp {
    /// Materialise the `Creating` -> `Created` transition the clock already owes, the
    /// way [`Endpoint::settle_status`] does: it was due at `created_at` whether or not
    /// anybody described the app, and `LastModifiedTime` moves to the instant it was
    /// due rather than to "now", so the value is stable across later reads.
    ///
    /// Answers whether the transition happened on this call.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == MLFLOW_APP_CREATING && now >= self.created_at {
            self.status = MLFLOW_APP_CREATED.to_string();
            self.last_modified_time = self.created_at;
            return true;
        }
        false
    }
}

/// An AppImageConfig created by `CreateAppImageConfig`, as `DescribeAppImageConfig`
/// replays it.
///
/// AWS returns an AppImageConfig ready the moment it is created — the resource carries
/// no status member at all (`smithy_query` shape=AppImageConfigDetails) — so there is no
/// clock-owed transition to settle here, only the two timestamps.
#[derive(Debug, Clone)]
pub struct AppImageConfig {
    pub name: String,
    pub arn: String,
    pub kernel_gateway_image_config: Option<KernelGatewayImageConfig>,
    pub jupyter_lab_app_image_config: Option<JupyterLabAppImageConfig>,
    pub code_editor_app_image_config: Option<CodeEditorAppImageConfig>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

/// A Git repository registered by `CreateCodeRepository`, as `DescribeCodeRepository`
/// replays it.
///
/// `GitConfig` is required on create, so it is stored unconditionally; the reader's
/// member is optional only because the model marks it so. `tags` is stored so a later
/// `ListTags` reports what the create carried; no implemented operation surfaces it yet.
/// `last_modified_time` is carried separately from `creation_time` because the modelled
/// output reports both; a repository nothing has changed reports the creation instant.
#[derive(Debug, Clone)]
pub struct CodeRepository {
    pub name: String,
    pub arn: String,
    pub git_config: GitConfig,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

/// Which Studio principal a Studio app hangs off.
///
/// `CreateApp` / `DescribeApp` / `DeleteApp` all take `UserProfileName` and `SpaceName`
/// as optional members whose model docs say each must be set when the other is not
/// (`smithy_query(query="doc", shape="DeleteAppRequest", member="SpaceName")`), so
/// exactly one of the two identifies the app.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum AppOwner {
    /// `UserProfileName`.
    UserProfile(String),
    /// `SpaceName`.
    Space(String),
}

impl AppOwner {
    /// The owner's name, whichever member named it — the component an app ARN carries.
    pub fn name(&self) -> &str {
        match self {
            Self::UserProfile(name) | Self::Space(name) => name,
        }
    }

    /// `UserProfileName` as the app reports it, i.e. only for a user-profile app.
    pub fn user_profile_name(&self) -> Option<String> {
        match self {
            Self::UserProfile(name) => Some(name.clone()),
            Self::Space(_) => None,
        }
    }

    /// `SpaceName` as the app reports it, i.e. only for a space app.
    pub fn space_name(&self) -> Option<String> {
        match self {
            Self::Space(name) => Some(name.clone()),
            Self::UserProfile(_) => None,
        }
    }
}

/// What identifies one Studio app: the domain, the owning user profile or space, the
/// app type and the app name — the five members every app operation takes. Two apps
/// differing in any one of them are different apps, so this is the store's key.
///
/// Ordered (domain, owner, type, name) so the store has one stable order to list over.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AppKey {
    pub domain_id: String,
    pub owner: AppOwner,
    pub app_type: String,
    pub app_name: String,
}

/// Why a request naming neither or both of `UserProfileName` and `SpaceName` is refused.
///
/// Both members are optional in the model, so the boundary validator passes such a
/// request; the rule lives in their docs ("If this value is not set, then
/// UserProfileName must be set", and the mirror of it), and an app whose owner is
/// unnamed — or named twice — cannot be resolved.
pub const APP_OWNER_REQUIRED: &str =
    "Exactly one of UserProfileName or SpaceName must be specified.";

/// The key the app operations' five input members name, or [`APP_OWNER_REQUIRED`] when
/// the owner is not exactly one of the user profile and the space.
///
/// Shared by `CreateApp`, `DescribeApp` and `DeleteApp` so all three resolve an app the
/// same way: a create and the reader or delete that names the same five members must
/// agree on the key, or the resource is unreachable.
pub fn app_key(
    domain_id: &str,
    user_profile_name: Option<&str>,
    space_name: Option<&str>,
    app_type: &str,
    app_name: &str,
) -> Result<AppKey, &'static str> {
    let owner = match (user_profile_name, space_name) {
        (Some(user_profile), None) => AppOwner::UserProfile(user_profile.to_string()),
        (None, Some(space)) => AppOwner::Space(space.to_string()),
        (Some(_), Some(_)) | (None, None) => return Err(APP_OWNER_REQUIRED),
    };
    Ok(AppKey {
        domain_id: domain_id.to_string(),
        owner,
        app_type: app_type.to_string(),
        app_name: app_name.to_string(),
    })
}

/// A Studio app created by `CreateApp`, as `DescribeApp` replays it.
///
/// `in_service_at` is the instant the clock owes the `Pending` -> `InService`
/// transition; it is stored, never slept on, and materialised by whichever reporting
/// call first runs at or after it.
#[derive(Debug, Clone)]
pub struct App {
    pub arn: String,
    pub status: String,
    pub resource_spec: Option<ResourceSpec>,
    pub recovery_mode: Option<bool>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub in_service_at: SystemTime,
}

impl App {
    /// Materialise the `Pending` -> `InService` transition the clock already owes, the
    /// way [`Endpoint::settle_status`] does for an endpoint.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == APP_STATUS_PENDING && now >= self.in_service_at {
            self.status = APP_STATUS_IN_SERVICE.to_string();
            return true;
        }
        false
    }
}

/// An artifact created by `CreateArtifact`, as `DescribeArtifact` replays it.
///
/// `properties` keeps the caller's map as given — `None` for an absent member and
/// `Some(empty)` for an empty one — so a reader cannot tell them apart only because
/// the store flattened them. It is a `BTreeMap` so the map serialises in one stable
/// order whatever order the request arrived in.
#[derive(Debug, Clone)]
pub struct Artifact {
    pub arn: String,
    /// The Region the ARN names, i.e. the one the creating request signed for. Held
    /// separately so the per-Region name uniqueness rule does not have to parse the ARN.
    pub region: String,
    pub name: Option<String>,
    pub artifact_type: String,
    pub source: ArtifactSource,
    pub properties: Option<BTreeMap<String, String>>,
    pub metadata_properties: Option<MetadataProperties>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

/// A model package group created by `CreateModelPackageGroup`, as
/// `DescribeModelPackageGroup` replays it, and the container a VERSIONED model package
/// is registered into.
///
/// `created_by` is stored at creation rather than rebuilt per read, so every later read
/// reports the identity that actually created the group.
///
/// `resource_policy` is the resource-based policy `PutModelPackageGroupPolicy` attached to
/// the group, exactly as that call sent it, and `None` while the group carries none. It
/// lives ON the group record rather than in a store of its own, so the one group has one
/// policy whichever operation reads it, it is bounded by
/// [`MODEL_PACKAGE_GROUP_QUOTA`] with the record it belongs to, and
/// `DeleteModelPackageGroup` releases the policy with the group.
#[derive(Debug, Clone)]
pub struct ModelPackageGroup {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    pub managed_configuration: Option<ManagedConfiguration>,
    pub status: String,
    pub created_by: UserContext,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub resource_policy: Option<String>,
}

/// Why SageMaker refuses `policy` as a resource policy for a model group, or `None` when
/// it accepts it.
///
/// A resource policy is an IAM policy document: `PolicyString`'s own documentation calls
/// it "the resource policy for the model group", and IAM's policy grammar makes a document
/// a JSON OBJECT (the `Version`/`Statement` elements are its members). The model bounds
/// only the member's length and leaves its pattern `.*`, so the boundary validator passes
/// a document that is not JSON at all and the service is what refuses it.
///
/// Nothing beyond that structural claim is checked here: which elements a statement
/// carries is IAM's business, and a document this accepts is stored and handed back
/// BYTE-FOR-BYTE rather than reformatted, so a caller reads back exactly the text it sent.
///
/// The returned text is the refusal's message; the caller renders it as the failure that
/// operation models for a document it cannot take.
pub fn resource_policy_violation(policy: &str) -> Option<String> {
    match ::serde_json::from_str::<::serde_json::Value>(policy) {
        Ok(::serde_json::Value::Object(_)) => None,
        Ok(_) => Some(
            "ResourcePolicy must be a JSON object: an IAM policy document".to_string(),
        ),
        Err(error) => Some(format!("ResourcePolicy is not valid JSON: {error}")),
    }
}

/// The most notebook instance lifecycle configurations this service's store holds, after
/// which a create is refused with the modelled `ResourceLimitExceeded`. See
/// [`MODEL_QUOTA`] for why the bound is the emulator's own rather than an account's.
pub const NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG_QUOTA: usize = 1_000;

/// The `<kind>` component [`resource_arn`] takes for a notebook instance lifecycle
/// configuration, as the AWS ARN reference spells SageMaker's resource type for it.
/// Named once so the create and the describe mint and expect the same ARN.
pub const NOTEBOOK_LIFECYCLE_CONFIG_ARN_KIND: &str = "notebook-instance-lifecycle-config";

/// A notebook instance lifecycle configuration created by
/// `CreateNotebookInstanceLifecycleConfig`, as `DescribeNotebookInstanceLifecycleConfig`
/// replays it.
///
/// `OnCreate` and `OnStart` are modelled as lists of at most one hook each; they are
/// stored exactly as the create request carried them, because the scripts they hold are
/// opaque base64 the service only ever hands back.
#[derive(Debug, Clone)]
pub struct NotebookInstanceLifecycleConfig {
    pub name: String,
    pub arn: String,
    pub on_create: Option<Vec<crate::ops::types::NotebookInstanceLifecycleHook>>,
    pub on_start: Option<Vec<crate::ops::types::NotebookInstanceLifecycleHook>>,
    /// What the create carried, so a later `ListTags`/`AddTags` reports it; no
    /// implemented operation surfaces it yet (`DescribeNotebookInstanceLifecycleConfig`
    /// models no `Tags` member).
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another notebook instance lifecycle configuration fits, i.e. the store is
    /// below [`NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG_QUOTA`].
    ///
    /// Kept with the rest of this resource's slice rather than in the first `impl` block,
    /// as the lineage-action slice at the end of this module is.
    pub fn notebook_instance_lifecycle_configs_below_quota(&self) -> bool {
        self.notebook_instance_lifecycle_configs.len() < NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG_QUOTA
    }
}

/// A model package created by `CreateModelPackage`, as `DescribeModelPackage` replays it.
///
/// One record covers both documented kinds: an UNVERSIONED package (the request named
/// `ModelPackageName`, so `group_name` and `version` are absent) and a VERSIONED one (the
/// request named `ModelPackageGroupName`, so `name` is the group's name and `version` is
/// the number this registration was given inside that group).
///
/// `created_by` is stored at creation, as [`ModelPackageGroup`]'s is, so every later read
/// reports the identity that actually registered the package.
///
/// `client_token` is the idempotency token the registering call presented, if any. It is
/// held here rather than in a map of its own so it is bounded by the same
/// [`MODEL_PACKAGE_QUOTA`] as the record and is released with it.
///
/// `completed_at` is the instant the clock owes the `Pending` -> `Completed` transition;
/// it is stored, never slept on, and materialised by whichever reporting call first runs
/// at or after it.
#[derive(Debug, Clone)]
pub struct ModelPackage {
    pub name: String,
    pub group_name: Option<String>,
    pub version: Option<i32>,
    pub arn: String,
    pub description: Option<String>,
    pub registration_type: Option<String>,
    pub inference_specification: Option<InferenceSpecification>,
    pub validation_specification: Option<ModelPackageValidationSpecification>,
    pub source_algorithm_specification: Option<SourceAlgorithmSpecification>,
    pub certify_for_marketplace: Option<bool>,
    pub model_approval_status: Option<String>,
    pub approval_description: Option<String>,
    pub metadata_properties: Option<MetadataProperties>,
    pub model_metrics: Option<ModelMetrics>,
    pub domain: Option<String>,
    pub task: Option<String>,
    pub sample_payload_url: Option<String>,
    pub customer_metadata_properties: Option<HashMap<String, String>>,
    pub drift_check_baselines: Option<DriftCheckBaselines>,
    pub additional_inference_specifications: Option<Vec<AdditionalInferenceSpecificationDefinition>>,
    pub skip_model_validation: Option<String>,
    pub source_uri: Option<String>,
    pub security_config: Option<ModelPackageSecurityConfig>,
    pub model_card: Option<ModelPackageModelCard>,
    pub model_life_cycle: Option<ModelLifeCycle>,
    pub managed_storage_type: Option<String>,
    pub tags: Vec<Tag>,
    pub created_by: UserContext,
    pub client_token: Option<String>,
    pub status: String,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub completed_at: SystemTime,
}

impl ModelPackage {
    /// The key this package is stored under: its name for an unversioned package,
    /// `<group>/<version>` for a version registered into a group. It is also the
    /// resource part of the package's ARN, so one identifier serves both.
    pub fn key(&self) -> String {
        model_package_key(&self.name, self.version)
    }

    /// Materialise the `Pending` -> `Completed` transition the clock already owes.
    ///
    /// The same single write a reporting operation may make as [`Endpoint::settle_status`]:
    /// the transition was due at `completed_at` whether or not anybody described the
    /// package, so the observer makes no difference to the result, and
    /// `LastModifiedTime` moves to the instant it was due rather than to "now" so the
    /// value is stable across later reads.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == MODEL_PACKAGE_STATUS_PENDING && now >= self.completed_at {
            self.status = MODEL_PACKAGE_STATUS_COMPLETED.to_string();
            self.last_modified_time = self.completed_at;
            return true;
        }
        false
    }

    /// `ModelPackageStatusDetails` for this package: one validation item per profile the
    /// request's `ValidationSpecification` named (no profiles, no items), each reporting
    /// `NotStarted` while the package is `Pending` and `Completed` once it has settled.
    ///
    /// `ImageScanStatuses` stays absent: this emulator scans no container image, so it
    /// reports no scan it did not run.
    pub fn status_details(&self) -> ModelPackageStatusDetails {
        let item_status = if self.status == MODEL_PACKAGE_STATUS_COMPLETED {
            DETAILED_MODEL_PACKAGE_STATUS_COMPLETED
        } else {
            DETAILED_MODEL_PACKAGE_STATUS_NOT_STARTED
        };
        let validation_statuses = self
            .validation_specification
            .as_ref()
            .map(|spec| {
                spec.validation_profiles
                    .iter()
                    .map(|profile| ModelPackageStatusItem {
                        name: profile.profile_name.clone(),
                        status: item_status.to_string(),
                        failure_reason: None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        ModelPackageStatusDetails {
            validation_statuses,
            image_scan_statuses: None,
        }
    }
}

/// The store key (and ARN resource part) of a model package: `name` on its own for an
/// unversioned package, `name/version` for a version inside a model package group.
///
/// The two forms cannot collide: `name` is an `EntityName` / `ModelPackageGroupName`,
/// whose modelled pattern (`^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`) admits no `/`, and the
/// boundary validator refuses a name outside it before any handler runs.
pub fn model_package_key(name: &str, version: Option<i32>) -> String {
    match version {
        Some(version) => format!("{name}/{version}"),
        None => name.to_string(),
    }
}

/// The store key a caller-supplied model package identifier names.
///
/// `DescribeModelPackage` takes a `VersionedArnOrName`, whose modelled pattern admits
/// exactly three forms: a bare name, a `name/version` pair, or this service's
/// `...:model-package/<key>` ARN — so an identifier reaching here is one of them. An ARN
/// resolves to everything after its `:model-package/` segment, which is exactly the key
/// form above; anything else is already a key.
pub fn model_package_lookup_key(identifier: &str) -> &str {
    identifier
        .split_once(":model-package/")
        .map_or(identifier, |(_, resource)| resource)
}

/// Per-service state for com-amazonaws-sagemaker-sagemaker. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSagemakerSagemakerState {
    /// Keyed by [`entity_key`] of `ExperimentName`. See [`Experiment`], with the rest
    /// of the experiment-management slice.
    pub experiments: BTreeMap<String, Experiment>,
    /// Whether this account's SageMaker Service Catalog portfolio is enabled, as
    /// `EnableSagemakerServicecatalogPortfolio` and
    /// `DisableSagemakerServicecatalogPortfolio` set it and
    /// `GetSagemakerServicecatalogPortfolioStatus` reports it, through
    /// [`Self::servicecatalog_portfolio_status`].
    ///
    /// Account-level, so it is a single flag rather than a map: the three operations take
    /// no input and name no resource. `Default` is `false` — an account that has never
    /// enabled the portfolio reads `Disabled`, the only other member of the modelled enum.
    pub servicecatalog_portfolio_enabled: bool,
    /// Keyed by `ModelName`.
    pub models: BTreeMap<String, Model>,
    /// Keyed by `EndpointConfigName`.
    pub endpoint_configs: BTreeMap<String, EndpointConfig>,
    /// Keyed by `EndpointName`.
    pub endpoints: BTreeMap<String, Endpoint>,
    /// Keyed by `PipelineName`.
    pub pipelines: BTreeMap<String, Pipeline>,
    /// Pipeline executions, keyed by `PipelineExecutionArn` — the one identifier
    /// `DescribePipelineExecution` resolves an execution by, and the one
    /// `StartPipelineExecution` mints.
    pub pipeline_executions: BTreeMap<String, PipelineExecution>,
    /// Keyed by `TransformJobName`.
    pub transform_jobs: BTreeMap<String, TransformJob>,
    /// Keyed by `MlflowAppArn` — the app's ARN is its identity, since the model
    /// documents that an MLflow app's NAME is not part of its ARN.
    pub mlflow_apps: BTreeMap<String, MlflowApp>,
    /// Keyed by `NotebookInstanceLifecycleConfigName`.
    pub notebook_instance_lifecycle_configs: BTreeMap<String, NotebookInstanceLifecycleConfig>,
    /// Keyed by `AppImageConfigName`, which AWS documents as unique to the account.
    pub app_image_configs: BTreeMap<String, AppImageConfig>,
    /// Keyed by `CodeRepositoryName`.
    pub code_repositories: BTreeMap<String, CodeRepository>,
    /// Keyed by [`AppKey`] — an app is named by five members, not by one name.
    pub apps: BTreeMap<AppKey, App>,
    /// Keyed by `ModelPackageGroupName`.
    pub model_package_groups: BTreeMap<String, ModelPackageGroup>,
    /// Keyed by [`model_package_key`], so an unversioned package and the versions of a
    /// group share one store and one stable order.
    pub model_packages: BTreeMap<String, ModelPackage>,
    /// Lineage artifacts, keyed by `ArtifactArn` — the only identifier `DescribeArtifact`
    /// resolves one by, and the one `CreateArtifact` mints.
    pub artifacts: BTreeMap<String, Artifact>,
    /// Keyed by `FeatureGroupName`. Served by the "Feature Store control plane" section
    /// of this module, which holds the whole feature-group slice — its constants, its
    /// record and its quota helper — in one block.
    pub feature_groups: BTreeMap<String, FeatureGroup>,
    /// Keyed by `NotebookInstanceName`. See [`NotebookInstance`], at the end of this
    /// module with the rest of the notebook-instance slice.
    pub notebook_instances: BTreeMap<String, NotebookInstance>,
    /// Lineage actions, keyed by `ActionArn` — the identifier that carries both the
    /// Region and the name, which is what `ActionName` is documented unique within, so a
    /// name taken in one Region leaves it free in another. See [`Action`], at the end of
    /// this module with the rest of the lineage-action slice.
    pub actions: BTreeMap<String, Action>,
    /// Keyed by `ModelCardName`. See [`ModelCard`], at the end of this module with the
    /// rest of the model-card slice.
    pub model_cards: BTreeMap<String, ModelCard>,
    /// Keyed by `AIWorkloadConfigName`. See [`AIWorkloadConfig`], at the end of this
    /// module with the rest of the AI-benchmarking workload-configuration slice.
    pub ai_workload_configs: BTreeMap<String, AIWorkloadConfig>,
    /// Keyed by `AIBenchmarkJobName`. See [`AIBenchmarkJob`], at the end of this module
    /// with the rest of the AI-benchmarking slice.
    pub ai_benchmark_jobs: BTreeMap<String, AIBenchmarkJob>,
    /// Keyed by `HubName`. See [`Hub`].
    pub hubs: BTreeMap<String, Hub>,
    /// Every stored hub content VERSION, keyed by [`HubContentKey`].
    pub hub_contents: BTreeMap<HubContentKey, HubContent>,
    /// Keyed by `TrialComponentName`, case-folded with [`entity_key`] (the name "is not
    /// case-sensitive" per its model doc), so one component cannot be stored twice under
    /// two spellings. See [`TrialComponent`], at the end of this module with the rest of
    /// the experiment-tracking slice.
    pub trial_components: BTreeMap<String, TrialComponent>,
    /// Keyed by `ProcessingJobName`.
    pub processing_jobs: BTreeMap<String, ProcessingJob>,
    /// Keyed by the partner app's `app-...` id. See [`PartnerApp`], at the end of this
    /// module with the rest of the partner-AI-app slice.
    pub partner_apps: BTreeMap<String, PartnerApp>,
    /// Keyed by `ImageName`. See [`Image`], at the end of this module with the rest of
    /// the custom-image slice.
    pub images: BTreeMap<String, Image>,
    /// Every stored image VERSION, keyed by `(ImageName, Version)` so the versions of
    /// one image sit together in ascending version order and the latest one is the last
    /// of that range. See [`ImageVersion`].
    pub image_versions: BTreeMap<(String, i32), ImageVersion>,
    /// Keyed by `ClusterName`. See [`Cluster`], at the end of this module with the rest
    /// of the HyperPod cluster slice.
    pub clusters: BTreeMap<String, Cluster>,
    /// Lineage associations, keyed by the `(SourceArn, DestinationArn)` pair that IS one
    /// association's identity — the only identifier `AddAssociation` and
    /// `DeleteAssociation` address one by. See [`Association`], at the end of this module
    /// with the rest of the lineage-association slice.
    pub associations: BTreeMap<(String, String), Association>,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// The account's Service Catalog portfolio status, as the modelled
    /// `SagemakerServicecatalogStatus` enum names it: its two members are `Enabled` and
    /// `Disabled` (`ops::types::SAGEMAKER_SERVICECATALOG_STATUS_VALUES`, generated from
    /// the model shape), and this is the one place either is named.
    pub fn servicecatalog_portfolio_status(&self) -> &'static str {
        if self.servicecatalog_portfolio_enabled {
            "Enabled"
        } else {
            "Disabled"
        }
    }

    /// Whether another model fits, i.e. the store is below [`MODEL_QUOTA`].
    pub fn models_below_quota(&self) -> bool {
        self.models.len() < MODEL_QUOTA
    }

    /// Whether another endpoint configuration fits, i.e. the store is below
    /// [`ENDPOINT_CONFIG_QUOTA`].
    pub fn endpoint_configs_below_quota(&self) -> bool {
        self.endpoint_configs.len() < ENDPOINT_CONFIG_QUOTA
    }

    /// Whether another endpoint fits, i.e. the store is below [`ENDPOINT_QUOTA`].
    pub fn endpoints_below_quota(&self) -> bool {
        self.endpoints.len() < ENDPOINT_QUOTA
    }

    /// Whether another pipeline fits, i.e. the store is below [`PIPELINE_QUOTA`].
    pub fn pipelines_below_quota(&self) -> bool {
        self.pipelines.len() < PIPELINE_QUOTA
    }

    /// Whether another pipeline execution fits, i.e. the store is below
    /// [`PIPELINE_EXECUTION_QUOTA`].
    pub fn pipeline_executions_below_quota(&self) -> bool {
        self.pipeline_executions.len() < PIPELINE_EXECUTION_QUOTA
    }

    /// The execution of `pipeline_name` the `ClientRequestToken` `token` started, if one
    /// is still stored.
    ///
    /// How `StartPipelineExecution` completes "no more than one time": a retry of a
    /// start call answers the execution that call started rather than starting a second
    /// one. The token is scoped to the pipeline, so the same token under another
    /// pipeline's name starts that pipeline's own execution.
    pub fn pipeline_execution_by_token(
        &self,
        pipeline_name: &str,
        token: &str,
    ) -> Option<&PipelineExecution> {
        self.pipeline_executions.values().find(|execution| {
            execution.pipeline_name == pipeline_name && execution.client_request_token == token
        })
    }

    /// Drop every pipeline whose removal the clock already owes.
    ///
    /// `DeletePipeline` starts an asynchronous removal: the pipeline reports
    /// `PipelineStatus` `Deleting` (one of the two members the modelled `PipelineStatus`
    /// enum carries) until [`PIPELINE_DELETED_AFTER`] has passed, after which it is gone.
    /// That disappearance was due at `removal_due_at` whether or not anybody called, so
    /// materialising it here is the one write a reporting operation may make: an observer
    /// makes no difference to the result.
    ///
    /// Keeps whatever the clock does not owe yet, dropping the rest in one pass — no
    /// caller needs the names it removed, so none is collected.
    /// A pipeline's executions go with it: an execution belongs to the pipeline that ran
    /// it, so once the pipeline is gone its executions are no longer resolvable either —
    /// and leaving them would let a pipeline recreated under the freed name inherit the
    /// executions of the one deleted before it.
    pub fn settle_pipeline_deletions(&mut self, now: SystemTime) {
        let mut removed: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        self.pipelines.retain(|name, pipeline| {
            let keep = pipeline.removal_due_at.is_none_or(|at| now < at);
            if !keep {
                removed.insert(name.clone());
            }
            keep
        });
        if !removed.is_empty() {
            self.pipeline_executions
                .retain(|_, execution| !removed.contains(&execution.pipeline_name));
        }
    }

    /// Whether another transform job fits, i.e. the store is below
    /// [`TRANSFORM_JOB_QUOTA`].
    pub fn transform_jobs_below_quota(&self) -> bool {
        self.transform_jobs.len() < TRANSFORM_JOB_QUOTA
    }

    /// Whether another MLflow app fits, i.e. the store is below [`MLFLOW_APP_QUOTA`].
    pub fn mlflow_apps_below_quota(&self) -> bool {
        self.mlflow_apps.len() < MLFLOW_APP_QUOTA
    }

    /// Whether another AppImageConfig fits, i.e. the store is below
    /// [`APP_IMAGE_CONFIG_QUOTA`].
    pub fn app_image_configs_below_quota(&self) -> bool {
        self.app_image_configs.len() < APP_IMAGE_CONFIG_QUOTA
    }

    /// Whether another model package fits, i.e. the store is below
    /// [`MODEL_PACKAGE_QUOTA`].
    pub fn model_packages_below_quota(&self) -> bool {
        self.model_packages.len() < MODEL_PACKAGE_QUOTA
    }

    /// The version number the next registration into `group` gets: one past the highest
    /// version the group currently holds, so a group's versions count up from 1 and each
    /// registration into it gets its own.
    pub fn next_model_package_version(&self, group: &str) -> i32 {
        self.model_packages
            .values()
            .filter(|package| package.group_name.as_deref() == Some(group))
            .filter_map(|package| package.version)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
    }

    /// Whether another code repository fits, i.e. the store is below
    /// [`CODE_REPOSITORY_QUOTA`].
    pub fn code_repositories_below_quota(&self) -> bool {
        self.code_repositories.len() < CODE_REPOSITORY_QUOTA
    }

    /// Whether another app fits, i.e. the store is below [`APP_QUOTA`].
    pub fn apps_below_quota(&self) -> bool {
        self.apps.len() < APP_QUOTA
    }

    /// Whether another model package group fits, i.e. the store is below
    /// [`MODEL_PACKAGE_GROUP_QUOTA`].
    pub fn model_package_groups_below_quota(&self) -> bool {
        self.model_package_groups.len() < MODEL_PACKAGE_GROUP_QUOTA
    }

    /// Whether another artifact fits, i.e. the store is below [`ARTIFACT_QUOTA`].
    pub fn artifacts_below_quota(&self) -> bool {
        self.artifacts.len() < ARTIFACT_QUOTA
    }

    /// The artifact called `name` in `region`, if one is stored.
    ///
    /// `CreateArtifact`'s `ArtifactName` is documented unique to an account in a Region,
    /// so this is what a create checks before taking a name. Artifacts are keyed by ARN
    /// (the name is optional), hence the scan over a store bounded by [`ARTIFACT_QUOTA`].
    pub fn artifact_named(&self, region: &str, name: &str) -> Option<&Artifact> {
        self.artifacts
            .values()
            .find(|artifact| artifact.region == region && artifact.name.as_deref() == Some(name))
    }

    /// The ARN of the artifact in `region` whose source URI is `source_uri`, if one is
    /// stored — the OLDEST such artifact when several share the URI.
    ///
    /// `DeleteArtifact` accepts either an `ArtifactArn` or a `Source`, and `ListArtifacts`
    /// filters artifacts by `SourceUri` alone: the URI is what identifies a source, so a
    /// source-addressed request resolves on it rather than on the optional
    /// `SourceTypes` beside it. Nothing stops two creates recording the same URI (the
    /// store is keyed by ARN), so the match is made deterministic by creation time, with
    /// the ARN breaking a tie. Same scan, and same reason, as [`Self::artifact_named`].
    pub fn artifact_arn_for_source(&self, region: &str, source_uri: &str) -> Option<String> {
        self.artifacts
            .values()
            .filter(|artifact| {
                artifact.region == region && artifact.source.source_uri == source_uri
            })
            .min_by(|left, right| {
                left.creation_time
                    .cmp(&right.creation_time)
                    .then_with(|| left.arn.cmp(&right.arn))
            })
            .map(|artifact| artifact.arn.clone())
    }

    /// Whether another processing job fits, i.e. the store is below
    /// [`PROCESSING_JOB_QUOTA`].
    pub fn processing_jobs_below_quota(&self) -> bool {
        self.processing_jobs.len() < PROCESSING_JOB_QUOTA
    }

    /// The model package the call that presented `token` registered under the same
    /// identity (`name` plus `group`), if one is stored.
    ///
    /// This is what makes `CreateModelPackage` idempotent: a retry carrying the same
    /// `ClientToken` answers the ARN of the package found here instead of registering
    /// another. The match is scoped to the identity the retry asks for, so a token reused
    /// for a DIFFERENT package is not answered with an unrelated package's ARN.
    ///
    /// The token lives ON the package record rather than in a map of its own, so it is
    /// bounded by [`MODEL_PACKAGE_QUOTA`] and released with the record it belongs to —
    /// hence the scan, as [`Self::artifact_named`] does for the same reason.
    pub fn model_package_for_client_token(
        &self,
        token: &str,
        name: &str,
        group: Option<&str>,
    ) -> Option<&ModelPackage> {
        self.model_packages.values().find(|package| {
            package.client_token.as_deref() == Some(token)
                && package.name == name
                && package.group_name.as_deref() == group
        })
    }

    /// Whether another model card fits, i.e. the store is below [`MODEL_CARD_QUOTA`].
    pub fn model_cards_below_quota(&self) -> bool {
        self.model_cards.len() < MODEL_CARD_QUOTA
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSagemakerSagemakerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSagemakerSagemakerState>)
}

/// The message a handler refuses with when the service is mounted on an engine that
/// holds no store for it. Unreachable in the assembled emulator (the router mounts
/// every service on the one shared `InMemoryEngine`); stated once so no handler
/// invents its own wording — and never by building a private engine, which would
/// drop the request's writes.
pub const NO_STATE: &str =
    "SageMaker state is unavailable: this service is not mounted on the shared in-memory engine";

/// The message SageMaker's `ResourceInUse` carries when a create names a resource whose
/// name is already taken.
///
/// It is a CONSTANT of the service, not built from the request: SageMaker answers the
/// same sentence whatever resource kind and whatever name the caller asked for, and the
/// name is not repeated back. Stated once here so each create that MODELS
/// `ResourceInUse` refuses with the same wording — today `CreateAppImageConfig`,
/// `CreateAIWorkloadConfig` and `CreateFeatureGroup`; an operation whose model does not
/// bind `ResourceInUse` (`CreateModel`, `CreateModelPackageGroup`) refuses with its own
/// `ValidationException` text instead and does not use this.
pub const RESOURCE_IN_USE_MESSAGE: &str = "The ID or Name specified is already in use.";

/// Whether `value` is a well-formed AWS region token: lowercase alphanumerics and
/// hyphens, at most `emulator_telemetry::policy::REGION_MAX_LEN` (32) bytes.
///
/// An ARN's region component is positional, so a scope carrying `:` or `/` would mint a
/// structurally malformed ARN. Anything outside the grammar is not accepted as a region.
fn is_region_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The region this request's SigV4 credential scope names, or [`DEFAULT_REGION`] when
/// the request is unsigned or its scope region is not a region token. Every ARN this
/// service mints takes its region from here, so a client signing for one region never
/// reads back an ARN naming another.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_token(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Record a call that changed this service's state, or a status transition it
/// materialised, so an emulator run is auditable the way CloudTrail makes a real one.
///
/// Carries only request metadata: the operation, the resource ARN it changed and the
/// access-key id the REQUEST presented (`emulator_server::extract_akid`). The
/// emulator's own credentials and every secret stay out of it.
pub fn record_state_change(
    operation: &str,
    resource_arn: &str,
    ctx: &smithy_server_runtime::OperationContext<'_>,
) {
    tracing::info!(
        operation,
        resource = resource_arn,
        access_key_id = %::emulator_server::extract_akid(&ctx.parts().headers),
        "sagemaker state change"
    );
}

/// `arn:aws:sagemaker:<region>:<account>:<kind>/<name>` — the one ARN form this
/// service mints, for the resource kinds it owns.
pub fn resource_arn(region: &str, kind: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(ARN_SERVICE, region, &format!("{kind}/{name}"))
}

/// Why SageMaker refuses this pipeline definition, or `None` when it accepts it.
///
/// A pipeline definition is a JSON document in the schema SageMaker Pipelines
/// publishes: a `Version` (the only published one is [`PIPELINE_DEFINITION_VERSION`])
/// and a `Steps` list whose entries each carry a `Name` and a `Type`, optionally
/// depending on other steps of the same definition through `DependsOn`. The service
/// resolves that dependency graph when the pipeline is written — not when it runs — so
/// a definition is refused unless
///
/// * it parses as a JSON object carrying those two keys,
/// * every step declares a name no other step in the definition declares,
/// * every `DependsOn` entry names a step the definition declares, and the resulting
///   graph is acyclic, and
/// * it declares at most [`PIPELINE_MAX_STEPS`] steps and
///   [`PIPELINE_MAX_STEP_DEPENDENCIES`] dependency edges between them, the two counts
///   the walk's work is the product of — so validating a definition stays short whatever
///   the megabyte the member's modelled length admits is filled with.
///
/// The returned text is the refusal's message; the caller renders it as the 400
/// `ValidationException` that an unsatisfiable definition is refused with.
pub fn pipeline_definition_violation(definition: &str) -> Option<String> {
    let parsed = match ::serde_json::from_str::<::serde_json::Value>(definition) {
        Ok(parsed) => parsed,
        Err(error) => {
            return Some(format!("PipelineDefinition is not valid JSON: {error}"));
        }
    };
    let Some(document) = parsed.as_object() else {
        return Some(
            "PipelineDefinition must be a JSON object carrying the keys Version and Steps"
                .to_string(),
        );
    };
    match document.get("Version").and_then(::serde_json::Value::as_str) {
        None => {
            return Some(format!(
                "PipelineDefinition is missing the required key Version, whose only \
                 supported value is {PIPELINE_DEFINITION_VERSION}"
            ));
        }
        Some(version) if version != PIPELINE_DEFINITION_VERSION => {
            return Some(format!(
                "PipelineDefinition declares the unsupported Version {version}; the only \
                 supported value is {PIPELINE_DEFINITION_VERSION}"
            ));
        }
        Some(_) => {}
    }
    let Some(steps) = document.get("Steps").and_then(::serde_json::Value::as_array) else {
        return Some(
            "PipelineDefinition is missing the required key Steps, a list of the pipeline's \
             steps"
                .to_string(),
        );
    };
    if steps.is_empty() {
        return Some(
            "PipelineDefinition declares no steps: Steps must hold at least one step"
                .to_string(),
        );
    }
    // The cheap pre-check, before any graph work: the step list alone is already past
    // the bound, so nothing below needs to look at it.
    if steps.len() > PIPELINE_MAX_STEPS {
        return Some(too_many_steps(steps.len()));
    }

    let mut declared = DeclaredSteps::default();
    if let Some(violation) = collect_pipeline_steps(steps, 1, &mut declared) {
        return Some(violation);
    }
    // Resolution and the cycle walk both reach a dependency through `by_name`, so each
    // is O((steps + edges) log steps) over counts the two bounds above cap.
    for step in &declared.steps {
        for dependency in &step.depends_on {
            if !declared.by_name.contains_key(dependency) {
                return Some(format!(
                    "step {} depends on step {dependency}, which this pipeline definition \
                     does not declare",
                    step.name
                ));
            }
        }
    }
    first_pipeline_cycle(&declared).map(|cycle| {
        format!(
            "PipelineDefinition declares a circular dependency between its steps: {}",
            cycle.join(" -> ")
        )
    })
}

/// The refusal for a definition declaring more steps than [`PIPELINE_MAX_STEPS`].
fn too_many_steps(declared: usize) -> String {
    format!(
        "PipelineDefinition declares {declared} steps, more than the {PIPELINE_MAX_STEPS} \
         steps one pipeline definition may hold"
    )
}

/// `<pipeline arn>/execution/<id>` — the one `PipelineExecutionArn` form this service
/// mints, and the form the modelled pattern
/// `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:pipeline\/.*\/execution\/.*$`
/// describes: an execution's ARN extends the ARN of the pipeline it runs.
pub fn pipeline_execution_arn(pipeline_arn: &str, execution_id: &str) -> String {
    format!("{pipeline_arn}/execution/{execution_id}")
}

/// Every step name a pipeline definition declares, in declaration order, counting the
/// steps nested in a condition step's branches — the one namespace a step name lives in.
///
/// Reads the SAME walk `pipeline_definition_violation` validates with, so what a stored
/// definition declares is never computed twice from two readings of the schema. A
/// definition the walk cannot resolve declares nothing addressable, so the answer is
/// then whatever it resolved before the violation; a definition that reached the store
/// was accepted by that validator, so this walk reports none.
pub fn pipeline_definition_step_names(definition: &str) -> Vec<String> {
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(definition) else {
        return Vec::new();
    };
    let Some(steps) = parsed.get("Steps").and_then(::serde_json::Value::as_array) else {
        return Vec::new();
    };
    let mut declared = DeclaredSteps::default();
    let _ = collect_pipeline_steps(steps, 1, &mut declared);
    declared
        .steps
        .iter()
        .map(|step| step.name.to_string())
        .collect()
}

/// Every parameter name a pipeline definition declares, in declaration order.
///
/// A definition's optional top-level `Parameters` list is what names the parameters an
/// execution may assign values to — `Parameter$Name`'s model doc: "This parameter name
/// must match a named parameter in the pipeline definition." A definition declaring no
/// `Parameters` names none, so every value a start call supplies for it is unmatched.
pub fn pipeline_definition_parameter_names(definition: &str) -> Vec<String> {
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(definition) else {
        return Vec::new();
    };
    parsed
        .get("Parameters")
        .and_then(::serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("Name").and_then(::serde_json::Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// One step of a pipeline definition, reduced to what the dependency graph is built
/// from: the name it declares and the step names it depends on, borrowed from the parsed
/// document so a large definition is not copied to walk it.
struct PipelineStep<'a> {
    name: &'a str,
    depends_on: Vec<&'a str>,
}

/// Every step one definition declares, in declaration order, plus the index that makes
/// a name lookup logarithmic instead of a scan of everything declared so far — the
/// uniqueness check, dependency resolution and the cycle walk all go through it, and
/// `edges` is the running total of `DependsOn` entries, which is bounded as it grows.
#[derive(Default)]
struct DeclaredSteps<'a> {
    steps: Vec<PipelineStep<'a>>,
    by_name: BTreeMap<&'a str, usize>,
    edges: usize,
}

/// Walk `steps` (and the branches of any condition step among them) into `out`,
/// answering the first violation of the per-step rules, if any.
///
/// A condition step's `IfSteps`/`ElseSteps` are steps of the same pipeline: they share
/// the one step-name namespace and may depend on steps outside their branch, so the
/// graph is built over all of them. SageMaker nests them under the step's `Arguments`;
/// both placements are walked, so a definition written either way resolves the same.
///
/// The walk stops as soon as the definition is past [`PIPELINE_MAX_STEPS`] or
/// [`PIPELINE_MAX_STEP_DEPENDENCIES`], so the work it does is bounded by those two
/// counts rather than by the megabyte the member's modelled length admits.
fn collect_pipeline_steps<'a>(
    steps: &'a [::serde_json::Value],
    depth: usize,
    out: &mut DeclaredSteps<'a>,
) -> Option<String> {
    if depth > PIPELINE_MAX_STEP_DEPTH {
        return Some(format!(
            "PipelineDefinition nests steps more than {PIPELINE_MAX_STEP_DEPTH} levels deep"
        ));
    }
    for step in steps {
        let Some(member) = step.as_object() else {
            return Some("every entry of Steps must be a JSON object".to_string());
        };
        let Some(name) = member
            .get("Name")
            .and_then(::serde_json::Value::as_str)
            .filter(|name| !name.is_empty())
        else {
            return Some("a step of PipelineDefinition is missing the required key Name".to_string());
        };
        let Some(step_type) = member.get("Type").and_then(::serde_json::Value::as_str) else {
            return Some(format!(
                "step {name} of PipelineDefinition is missing the required key Type"
            ));
        };
        if out.steps.len() == PIPELINE_MAX_STEPS {
            return Some(too_many_steps(out.steps.len() + 1));
        }
        if out.by_name.contains_key(name) {
            return Some(format!(
                "step name {name} is declared more than once; a pipeline definition's step \
                 names must be unique"
            ));
        }
        let mut depends_on = Vec::new();
        match member.get("DependsOn") {
            None | Some(::serde_json::Value::Null) => {}
            Some(::serde_json::Value::Array(entries)) => {
                if out.edges + entries.len() > PIPELINE_MAX_STEP_DEPENDENCIES {
                    return Some(format!(
                        "PipelineDefinition declares more than the \
                         {PIPELINE_MAX_STEP_DEPENDENCIES} DependsOn entries one pipeline \
                         definition may hold"
                    ));
                }
                out.edges += entries.len();
                for entry in entries {
                    match entry.as_str() {
                        Some(dependency) => depends_on.push(dependency),
                        None => {
                            return Some(format!(
                                "step {name} declares a DependsOn entry that is not a step name"
                            ));
                        }
                    }
                }
            }
            Some(_) => {
                return Some(format!(
                    "step {name} declares DependsOn as something other than a list of step names"
                ));
            }
        }
        out.by_name.insert(name, out.steps.len());
        out.steps.push(PipelineStep { name, depends_on });

        // Only a condition step holds steps of its own, and only under the two branch
        // keys the schema gives it: any other step's `Arguments` are its own payload,
        // so a list that merely happens to be named `Steps` there is NOT walked as
        // pipeline steps.
        if step_type == PIPELINE_CONDITION_STEP_TYPE {
            for branch in ["IfSteps", "ElseSteps"] {
                let placements = [
                    member.get(branch),
                    member.get("Arguments").and_then(|args| args.get(branch)),
                ];
                for nested in placements
                    .into_iter()
                    .flatten()
                    .filter_map(::serde_json::Value::as_array)
                {
                    if let Some(violation) = collect_pipeline_steps(nested, depth + 1, out) {
                        return Some(violation);
                    }
                }
            }
        }
    }
    None
}

/// The first cycle the steps' `DependsOn` edges form, as the step names along it, or
/// `None` when the graph is acyclic.
///
/// A depth-first walk that carries its own stack rather than recursing, so a definition
/// with a long dependency chain cannot exhaust the call stack, and that resolves each
/// edge through the name index rather than scanning the steps. A dependency no step
/// declares is skipped here: it is refused before this runs.
fn first_pipeline_cycle(declared: &DeclaredSteps<'_>) -> Option<Vec<String>> {
    /// Not yet visited.
    const UNSEEN: u8 = 0;
    /// On the current walk's path.
    const ON_PATH: u8 = 1;
    /// Visited, and every path out of it was walked.
    const SETTLED: u8 = 2;

    let steps = &declared.steps;
    let mut colour = vec![UNSEEN; steps.len()];
    for start in 0..steps.len() {
        if colour[start] != UNSEEN {
            continue;
        }
        colour[start] = ON_PATH;
        let mut path: Vec<(usize, usize)> = vec![(start, 0)];
        while let Some(&(node, cursor)) = path.last() {
            let Some(dependency) = steps[node].depends_on.get(cursor) else {
                colour[node] = SETTLED;
                path.pop();
                continue;
            };
            if let Some(frame) = path.last_mut() {
                frame.1 += 1;
            }
            let Some(&next) = declared.by_name.get(dependency) else {
                continue;
            };
            match colour[next] {
                ON_PATH => {
                    let from = path
                        .iter()
                        .position(|frame| frame.0 == next)
                        .unwrap_or_default();
                    let mut cycle: Vec<String> = path[from..]
                        .iter()
                        .map(|frame| steps[frame.0].name.to_string())
                        .collect();
                    cycle.push(steps[next].name.to_string());
                    return Some(cycle);
                }
                UNSEEN => {
                    colour[next] = ON_PATH;
                    path.push((next, 0));
                }
                _ => {}
            }
        }
    }
    None
}

/// An app's ARN: `...:app/<DomainId>/<owner>/<AppType>/<AppName>`.
///
/// A Studio app is identified by five members, so its ARN names all of them — a single
/// `AppName` would not distinguish two apps of different types under one user profile.
/// Each component is carried exactly as the request supplied it, which is safe for a
/// positional ARN segment because every one of them is already refused at the boundary
/// unless it matches its modelled constraint: `AppName`, `UserProfileName` and
/// `SpaceName` are `len 0..63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`, `DomainId`
/// is `len 0..63, pattern ^d-(-*[a-z0-9]){1,61}$` and `AppType` is an enum
/// (`smithy_query(query="operation", operation="CreateApp")`), so none can carry a `:`,
/// a `/` or a control character and the result always matches `AppArn`'s
/// `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:app/`. The real service composes
/// the ARN from the same five members, so its `len 0..256` bound binds the same inputs.
pub fn app_arn(region: &str, key: &AppKey) -> String {
    resource_arn(
        region,
        "app",
        &format!(
            "{}/{}/{}/{}",
            key.domain_id,
            key.owner.name(),
            key.app_type,
            key.app_name
        ),
    )
}

/// The ARN of the Region's default lineage group (see [`DEFAULT_LINEAGE_GROUP`]), as
/// `LineageGroupArn` carries it.
pub fn lineage_group_arn(region: &str) -> String {
    resource_arn(region, "lineage-group", DEFAULT_LINEAGE_GROUP)
}

/// Mint the opaque id an ARN carries for a resource whose ARN does NOT contain the
/// resource's name: 32 lowercase hex digits, the form SageMaker's `artifact/<id>` and
/// `mlflow-app/<id>` ARNs take. The caller never supplies it — such a resource is named
/// by its ARN, so the id is the server's to generate, and it is derived from nothing in
/// the request. (`MlflowAppName` is `len 1..256` while `MlflowAppArn` is `len 1..128`,
/// and the `CreateMlflowApp` model doc says the name "is not part of the ... ARN": an
/// id-bearing ARN is what keeps a minted ARN inside the modelled length whatever the
/// name's length.)
///
/// Emulator-grade and deliberately NOT cryptographic: the bits come from SHA-256 over
/// three sources, which make an id unique within a run but no more unpredictable than
/// what they carry — a `RandomState` hash of the counter (std's hash-DoS seed, not a
/// CSPRNG), a monotonic counter, so two ids minted in the same nanosecond on different
/// threads differ by construction, and the wall clock in nanoseconds, so ids do not
/// repeat across processes. Such an id is an identifier inside an authenticated store,
/// never a secret; nothing here may be relied on as one.
pub fn mint_opaque_resource_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    static MINTED: AtomicU64 = AtomicU64::new(0);
    let counter = MINTED.fetch_add(1, Ordering::Relaxed);

    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(counter);
    let entropy = hasher.finish();

    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_nanos())
        .unwrap_or_default();

    let mut digest = <::sha2::Sha256 as ::sha2::Digest>::new();
    ::sha2::Digest::update(&mut digest, entropy.to_le_bytes());
    ::sha2::Digest::update(&mut digest, counter.to_le_bytes());
    ::sha2::Digest::update(&mut digest, nanos.to_le_bytes());
    let hashed = ::sha2::Digest::finalize(digest);

    // 16 bytes of the digest, hex-rendered: the 32-digit form the real ARNs carry.
    ::hex::encode(&hashed[..16])
}

/// The resource NAME an `ArnOrName`-typed member addresses.
///
/// Several SageMaker readers accept either a bare name or the resource's full ARN for the
/// same resource (the `ArnOrName` and `HubNameOrArn` shapes, the latter modelled
/// `^(arn:…:hub\/)?<name>$`), and the store is keyed by the name — so an ARN is
/// reduced to the resource portion after its final `/`. A value that is not an ARN is
/// already a name and is returned unchanged.
pub fn name_from_arn_or_name(value: &str) -> &str {
    match value.strip_prefix("arn:") {
        Some(_) => value.split('/').next_back().unwrap_or(value),
        None => value,
    }
}

/// The IAM user name the request's SigV4 credential names, or a fixed `emulator`
/// principal when the request carries no usable access key id.
///
/// The access key id is request text and the value is spliced into an ARN, whose segments
/// are positional, so anything not shaped like an access key id is replaced rather than
/// interpolated. `extract_akid` performs no verification — the emulator verifies signatures
/// at its boundary.
fn caller_principal(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    let akid = ::emulator_server::extract_akid(&ctx.parts().headers);
    let shaped_like_an_access_key_id = !akid.is_empty()
        && akid.len() <= 128
        && akid.chars().all(|character| character.is_ascii_alphanumeric());
    if shaped_like_an_access_key_id {
        akid
    } else {
        "emulator".to_string()
    }
}

/// The `UserContext` SageMaker records as the creator of a resource.
///
/// The emulator serves no Studio domain, so the Studio members (`UserProfileArn`,
/// `UserProfileName`, `DomainId`) are ABSENT rather than empty: a caller cannot tell an
/// empty profile name from a missing one. The IAM identity is the one the request signed
/// with. `PrincipalId` and `SourceIdentity` stay absent — this emulator models no IAM
/// principal ids, and inventing one would report an identity that does not exist.
pub fn caller_user_context(ctx: &smithy_server_runtime::OperationContext<'_>) -> UserContext {
    let principal = caller_principal(ctx);
    UserContext {
        user_profile_arn: None,
        user_profile_name: None,
        domain_id: None,
        iam_identity: Some(IamIdentity {
            // IAM is global, so the region segment is empty.
            arn: Some(::emulator_server::emulator_arn(
                "iam",
                "",
                &format!("user/{principal}"),
            )),
            principal_id: None,
            source_identity: None,
        }),
    }
}

/// A stored `SystemTime` as the modelled `Timestamp` the wire carries.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    match at.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since_epoch) => ::aws_smithy_types::DateTime::from_secs_and_nanos(
            i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
            since_epoch.subsec_nanos(),
        ),
        // A host clock set before the epoch: keep the sign rather than clamping the
        // instant to zero and reporting a time the resource was not created at. Built
        // from whole seconds plus a non-negative nanos remainder, so this branch is as
        // lossless as the one above.
        Err(before_epoch) => {
            let magnitude = before_epoch.duration();
            let secs = i64::try_from(magnitude.as_secs()).unwrap_or(i64::MAX);
            match magnitude.subsec_nanos() {
                0 => ::aws_smithy_types::DateTime::from_secs(-secs),
                nanos => ::aws_smithy_types::DateTime::from_secs_and_nanos(
                    -secs - 1,
                    1_000_000_000 - nanos,
                ),
            }
        }
    }
}

/// A modelled `Timestamp` as a comparable key, so a stored time and a filter bound
/// order against each other without going through a lossy float.
pub fn timestamp_key(at: ::aws_smithy_types::DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

// ---------------------------------------------------------------------------
// Feature Store control plane
//
// Everything the feature-group slice of this service needs — its constants, its record,
// its status transition and its quota helper — is kept together in this ONE section, the
// way the lineage-action and AI-workload-configuration slices below are, so one
// resource's state reads as one block. Its shapes are named by their full
// `crate::ops::types::` path rather than through a `use`, so the slice carries no import
// of its own.
// ---------------------------------------------------------------------------

/// How long after `CreateFeatureGroup` the clock owes the `Creating` -> `Created`
/// transition.
///
/// Same contract as [`ENDPOINT_IN_SERVICE_AFTER`]: a caller that describes the feature
/// group straight after creating it still observes `Creating`, and one that polls (as
/// `aws sagemaker wait` and the Feature Store SDK's own wait loop do) observes
/// `Created`. The model's doc puts the real service at "approximately 10-15 minutes to
/// provision an OnlineStore FeatureGroup with the InMemory StorageType", so the span is
/// compressed, never zero.
pub const FEATURE_GROUP_CREATED_AFTER: Duration = Duration::from_millis(300);

/// See [`MODEL_QUOTA`]. `CreateFeatureGroup`'s doc points at the published quota
/// ("Check Amazon Web Services service quotas to see the FeatureGroups quota for your
/// Amazon Web Services account"), which is per-account and adjustable, so this is the
/// emulator's own bound rather than a claim about any account's limit.
pub const FEATURE_GROUP_QUOTA: usize = 1_000;

/// `FeatureGroupStatus`'s members this emulator moves a feature group through (the
/// modelled enum also carries `CreateFailed`, `Deleting` and `DeleteFailed`, which this
/// slice does not reach). Named once here so no handler compares a bare string literal.
/// Deliberately separate from [`STATUS_CREATING`] even though the two spell the same
/// value today: they are members of two different modelled enums, and either may be
/// re-spelled without the other.
pub const FEATURE_GROUP_STATUS_CREATING: &str = "Creating";
/// See [`FEATURE_GROUP_STATUS_CREATING`].
pub const FEATURE_GROUP_STATUS_CREATED: &str = "Created";

/// The `OfflineStoreStatusValue` an offline store this emulator holds reports: the
/// replication it describes never fails here, so it is never `Blocked`.
pub const OFFLINE_STORE_STATUS_ACTIVE: &str = "Active";

/// The feature names a `FeatureGroup` may not declare, from `FeatureDefinitions`' own
/// doc: "FeatureNames cannot be any of the following: is_deleted, write_time,
/// api_invocation_time" — Feature Store writes those three columns itself.
pub const RESERVED_FEATURE_NAMES: &[&str] = &["is_deleted", "write_time", "api_invocation_time"];

/// A feature group created by `CreateFeatureGroup`, as `DescribeFeatureGroup` replays
/// it.
///
/// `created_due_at` is the instant the clock owes the `Creating` -> `Created` transition;
/// like [`Endpoint::in_service_at`] it is stored, never slept on, and materialised by
/// whichever reporting call first runs at or after it.
#[derive(Debug, Clone)]
pub struct FeatureGroup {
    pub name: String,
    pub arn: String,
    pub record_identifier_feature_name: String,
    pub event_time_feature_name: String,
    pub feature_definitions: Vec<crate::ops::types::FeatureDefinition>,
    pub online_store_config: Option<crate::ops::types::OnlineStoreConfig>,
    pub offline_store_config: Option<crate::ops::types::OfflineStoreConfig>,
    pub throughput_config: Option<crate::ops::types::ThroughputConfig>,
    pub role_arn: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<Tag>,
    pub status: String,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    /// WHEN the `Created` status falls due, not when the group was created — that is
    /// `creation_time`. Named the way [`Endpoint::in_service_at`] names the same thing.
    pub created_due_at: SystemTime,
}

impl FeatureGroup {
    /// Materialise the `Creating` -> `Created` transition the clock already owes.
    ///
    /// The one write a reporting operation may make, for the reason spelled out on
    /// [`Endpoint::settle_status`]: the transition was due at `created_due_at` whether or
    /// not anybody described the group.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == FEATURE_GROUP_STATUS_CREATING && now >= self.created_due_at {
            self.status = FEATURE_GROUP_STATUS_CREATED.to_string();
            self.last_modified_time = self.created_due_at;
            return true;
        }
        false
    }

    /// This group's throughput configuration as `DescribeFeatureGroup` reports it.
    ///
    /// `ThroughputConfig` (what a create states) and `ThroughputConfigDescription`
    /// (what a describe reports) are two shapes with the same three members, so the
    /// active configuration is the one the create stated, re-shaped. A group created
    /// without one has no mode to report and reports none.
    pub fn throughput_config_description(
        &self,
    ) -> Option<crate::ops::types::ThroughputConfigDescription> {
        self.throughput_config.as_ref().map(|config| {
            crate::ops::types::ThroughputConfigDescription {
                throughput_mode: config.throughput_mode.clone(),
                provisioned_read_capacity_units: config.provisioned_read_capacity_units,
                provisioned_write_capacity_units: config.provisioned_write_capacity_units,
            }
        })
    }
}

/// The feature-group half of the store's API, in its own `impl` block for the same
/// reason as the section: a resource's state reads as one block.
impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another feature group fits, i.e. the store is below
    /// [`FEATURE_GROUP_QUOTA`].
    pub fn feature_groups_below_quota(&self) -> bool {
        self.feature_groups.len() < FEATURE_GROUP_QUOTA
    }
}

// ---------------------------------------------------------------------------
// Notebook instances
//
// Everything `CreateNotebookInstance`, `StopNotebookInstance` and
// `DescribeNotebookInstance` need, kept in one
// block: the record the store holds, the spans and defaults those operations apply,
// and the two values the create derives (the Jupyter host and the network-interface id).
// ---------------------------------------------------------------------------

/// How long after `CreateNotebookInstance` the clock owes the `Pending` -> `InService`
/// transition. Same shape as [`ENDPOINT_IN_SERVICE_AFTER`]: a caller that describes the
/// notebook instance straight after creating it still observes `Pending`, one that waits
/// (as `aws sagemaker wait notebook-instance-in-service` does) observes `InService`.
/// Real SageMaker takes minutes; the span is compressed, never zero.
pub const NOTEBOOK_INSTANCE_IN_SERVICE_AFTER: Duration = Duration::from_millis(300);

/// See [`MODEL_QUOTA`]: the same emulator-owned bound, for the notebook-instance store.
pub const NOTEBOOK_INSTANCE_QUOTA: usize = 1_000;

/// The `NotebookInstanceStatus` member a fresh instance carries. The modelled enum also
/// carries the update, delete and maintenance states, which this slice does not
/// reach; `InService` is spelled the same as an endpoint's, so [`STATUS_IN_SERVICE`]
/// stands for both.
pub const NOTEBOOK_INSTANCE_STATUS_PENDING: &str = "Pending";

/// The `NotebookInstanceStatus` an instance carries while `StopNotebookInstance` is
/// terminating its ML compute instance, from the modelled `NotebookInstanceStatus` enum
/// (`smithy_query shape=NotebookInstanceStatus`).
pub const NOTEBOOK_INSTANCE_STATUS_STOPPING: &str = "Stopping";

/// The `NotebookInstanceStatus` an instance carries once its ML compute instance is
/// terminated, from the modelled `NotebookInstanceStatus` enum.
pub const NOTEBOOK_INSTANCE_STATUS_STOPPED: &str = "Stopped";

/// How long after `StopNotebookInstance` the clock owes the `Stopping` -> `Stopped`
/// transition. Same contract as [`NOTEBOOK_INSTANCE_IN_SERVICE_AFTER`]: a caller that
/// describes the instance straight after stopping it still observes `Stopping`, one that
/// waits (as `aws sagemaker wait notebook-instance-stopped` does) observes `Stopped`.
/// Real SageMaker takes minutes to detach the ML storage volume and terminate the
/// compute instance; the span is this emulator's own compressed choice, never zero, and
/// the due instant is stored rather than slept on.
pub const NOTEBOOK_INSTANCE_STOPPED_AFTER: Duration = Duration::from_millis(300);

/// The statuses a `StopNotebookInstance` transitions an instance OUT of: the ones where
/// an ML compute instance exists to terminate, because it is serving (`InService`) or
/// coming up (`Pending`). From every other `NotebookInstanceStatus` there is nothing to
/// stop, so the call is refused rather than moving the status.
pub const NOTEBOOK_INSTANCE_STOPPABLE_STATUSES: [&str; 2] =
    [NOTEBOOK_INSTANCE_STATUS_PENDING, STATUS_IN_SERVICE];

/// The IMDS version a notebook instance reports when the create named none:
/// `MinimumInstanceMetadataServiceVersion`'s doc says that "when passed as part of
/// `CreateNotebookInstance`, if no value is selected, then it defaults to IMDSv1".
pub const DEFAULT_IMDS_VERSION: &str = "1";

/// `DirectInternetAccess`'s default. Its `CreateNotebookInstance` doc points at
/// "Notebook Instances Are Internet-Enabled by Default" and allows `Disabled` only
/// alongside a `SubnetId`.
pub const DEFAULT_DIRECT_INTERNET_ACCESS: &str = "Enabled";

/// `RootAccess`'s default: its `CreateNotebookInstance` doc says "the default value is
/// Enabled".
pub const DEFAULT_ROOT_ACCESS: &str = "Enabled";

/// `IpAddressType`'s default. AWS's `CreateNotebookInstance` API reference documents the
/// member as defaulting to IPv4-only connectivity when the request names no value.
pub const DEFAULT_IP_ADDRESS_TYPE: &str = "ipv4";

/// `PlatformIdentifier`'s default: its `CreateNotebookInstance` doc says "the default
/// value is notebook-al2023-v1".
pub const DEFAULT_PLATFORM_IDENTIFIER: &str = "notebook-al2023-v1";

/// `VolumeSizeInGB`'s default: its `CreateNotebookInstance` doc says "the default value
/// is 5 GB".
pub const DEFAULT_VOLUME_SIZE_IN_GB: i32 = 5;

/// The resource kind a notebook instance's ARN carries, as `NotebookInstanceArn`'s
/// documented `arn:aws:sagemaker:<region>:<account>:notebook-instance/<name>` form
/// spells it. Named once so the create and the reader agree; both pass it to
/// [`resource_arn`].
pub const NOTEBOOK_INSTANCE_ARN_KIND: &str = "notebook-instance";

/// The Jupyter host a notebook instance serves on:
/// `<name>.notebook.<region>.sagemaker.aws`, the host `DescribeNotebookInstance`'s
/// `Url` carries and `CreatePresignedNotebookInstanceUrl` signs against.
///
/// Derived from the instance name and the region the caller signed for, so it is stable
/// across reads and distinct per instance.
pub fn notebook_instance_url(region: &str, name: &str) -> String {
    format!("{name}.notebook.{region}.sagemaker.aws")
}

/// The id of the network interface `CreateNotebookInstance` reports it created:
/// `eni-` plus 17 lowercase hex digits, the form EC2 network-interface ids take.
///
/// The digits come from this module's one id mint, [`mint_opaque_resource_id`], so an ENI
/// id is server-generated and derived from nothing in the request, with the same
/// emulator-grade, non-cryptographic caveats that mint documents: it identifies a record
/// in this store and is never a secret. `CreateNotebookInstance` mints it ONCE and stores
/// it, so every later read of the same instance reports the same id.
pub fn mint_network_interface_id() -> String {
    let digits = mint_opaque_resource_id();
    // The mint renders a 32-digit hex id, but the slice is taken defensively so this
    // helper does not depend on that width: a shorter id is used whole.
    format!("eni-{}", digits.get(..17).unwrap_or(&digits))
}

/// A notebook instance created by `CreateNotebookInstance`, as
/// `DescribeNotebookInstance` replays it.
///
/// Every member the create carried is kept as given, so the reader replays the request
/// rather than re-deriving it; the members the create left out are kept at the default
/// the model documents for them (the `DEFAULT_*` constants above). `in_service_at` is the
/// instant the clock owes the `Pending` -> `InService` transition; it is stored, never
/// slept on, and materialised by whichever reporting call first runs at or after it.
#[derive(Debug, Clone)]
pub struct NotebookInstance {
    pub name: String,
    pub arn: String,
    pub status: String,
    /// Why the instance failed, for the `Failed` status. No implemented operation moves
    /// an instance there yet (the start and update operations are still stubs), so
    /// it is stored `None` and reported absent.
    pub failure_reason: Option<String>,
    /// The Jupyter host the instance serves on, minted at create from the name and the
    /// signed region (see [`notebook_instance_url`]).
    pub url: String,
    pub instance_type: String,
    pub ip_address_type: String,
    pub subnet_id: Option<String>,
    pub security_groups: Vec<String>,
    pub role_arn: String,
    pub kms_key_id: Option<String>,
    /// The id of the network interface the create minted for the instance.
    pub network_interface_id: String,
    pub lifecycle_config_name: Option<String>,
    pub direct_internet_access: String,
    pub volume_size_in_gb: i32,
    pub accelerator_types: Vec<String>,
    pub default_code_repository: Option<String>,
    pub additional_code_repositories: Vec<String>,
    pub root_access: String,
    pub platform_identifier: String,
    pub instance_metadata_service_configuration:
        crate::ops::types::InstanceMetadataServiceConfiguration,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub in_service_at: SystemTime,
    /// The instant the clock owes the `Stopping` -> `Stopped` transition, set by
    /// `StopNotebookInstance` and cleared once the transition is materialised. `None`
    /// while no stop is in flight.
    pub stopped_at: Option<SystemTime>,
    /// Whether the instance ever finished provisioning, i.e. the `Pending` ->
    /// `InService` transition was materialised. Kept beside the status rather than
    /// derived from it, because a stop that lands while the instance is still `Pending`
    /// leaves it `Stopping` and then `Stopped` without it ever having come up — and an
    /// instance that never came up has no Jupyter endpoint and no network interface to
    /// report (see [`NotebookInstance::is_provisioned`]).
    pub provisioned: bool,
}

impl NotebookInstance {
    /// Materialise whichever status transition the clock already owes, the way
    /// [`Endpoint::settle_status`] does for an endpoint: `Pending` -> `InService` after a
    /// create, `Stopping` -> `Stopped` after a stop. Each was due at its stored instant
    /// whether or not anybody called, and `LastModifiedTime` moves to the instant it was
    /// due rather than to "now", so the value is stable across later reads.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == NOTEBOOK_INSTANCE_STATUS_PENDING && now >= self.in_service_at {
            self.status = STATUS_IN_SERVICE.to_string();
            self.last_modified_time = self.in_service_at;
            self.provisioned = true;
            return true;
        }
        // The stop's own due instant, kept beside the create's so a stop that arrives
        // while the instance is still `Pending` does not later snap back to `InService`.
        let stop_due = match self.stopped_at {
            Some(due) if self.status == NOTEBOOK_INSTANCE_STATUS_STOPPING && now >= due => {
                Some(due)
            }
            _ => None,
        };
        if let Some(due) = stop_due {
            self.status = NOTEBOOK_INSTANCE_STATUS_STOPPED.to_string();
            self.last_modified_time = due;
            self.stopped_at = None;
            return true;
        }
        false
    }

    /// Whether `StopNotebookInstance` has an ML compute instance to terminate, i.e. the
    /// status is one of [`NOTEBOOK_INSTANCE_STOPPABLE_STATUSES`].
    pub fn is_stoppable(&self) -> bool {
        NOTEBOOK_INSTANCE_STOPPABLE_STATUSES.contains(&self.status.as_str())
    }

    /// Start terminating the ML compute instance: the status moves to `Stopping` now and
    /// the clock owes `Stopped` at [`NOTEBOOK_INSTANCE_STOPPED_AFTER`] from `now`, which
    /// [`NotebookInstance::settle_status`] materialises once it comes due.
    ///
    /// Every stored member is left as it stands — SageMaker preserves the ML storage
    /// volume across a stop — so a later start replays the same configuration.
    pub fn begin_stop(&mut self, now: SystemTime) {
        self.status = NOTEBOOK_INSTANCE_STATUS_STOPPING.to_string();
        self.last_modified_time = now;
        self.stopped_at = Some(now + NOTEBOOK_INSTANCE_STOPPED_AFTER);
    }

    /// Whether the instance has finished provisioning, i.e. the `Pending` ->
    /// `InService` transition the create owed has been materialised.
    ///
    /// An instance that has not come up has no Jupyter endpoint and no network interface
    /// to report, so `DescribeNotebookInstance` omits `Url` and `NetworkInterfaceId`
    /// until it is provisioned — including for one stopped before it ever came up.
    pub fn is_provisioned(&self) -> bool {
        self.provisioned
    }
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another notebook instance fits, i.e. the store is below
    /// [`NOTEBOOK_INSTANCE_QUOTA`].
    pub fn notebook_instances_below_quota(&self) -> bool {
        self.notebook_instances.len() < NOTEBOOK_INSTANCE_QUOTA
    }
}

// ---------------------------------------------------------------------------
// Model cards. `CreateModelCard` stores one here and `DescribeModelCard` replays it;
// the store itself is `ComAmazonawsSagemakerSagemakerState::model_cards` above. Kept
// together at the end of the file so a model-card change touches one span of it.
// ---------------------------------------------------------------------------

/// The version number a model card's FIRST version carries. `DescribeModelCard`'s
/// `ModelCardVersion` is modelled as an `Integer` and the API's version history starts
/// at one, so `CreateModelCard` stores version 1 and an `UpdateModelCard` that carries
/// new `Content` appends the next integer ([`ModelCard::next_version`]).
pub const FIRST_MODEL_CARD_VERSION: i32 = 1;

/// One version of a model card, as `CreateModelCard` stores the first one and
/// `DescribeModelCard` replays it.
///
/// A model card is versioned: `Content`, `ModelCardStatus` and `SecurityConfig` belong
/// to a version, while the name, the ARN and the tags belong to the card.
///
/// `created_by` is stored when the version is created rather than rebuilt per read, so
/// every later read reports the identity that actually created it; `last_modified_by`
/// stays absent until a call modifies the version.
#[derive(Debug, Clone)]
pub struct ModelCardVersion {
    pub version: i32,
    pub content: String,
    pub status: String,
    pub security_config: Option<ModelCardSecurityConfig>,
    pub creation_time: SystemTime,
    pub created_by: UserContext,
    pub last_modified_time: SystemTime,
    pub last_modified_by: Option<UserContext>,
}

/// A model card created by `CreateModelCard`, with every version it has had.
///
/// `versions` is ordered by `version` ascending and is never empty: a card exists only
/// once its first version does.
#[derive(Debug, Clone)]
pub struct ModelCard {
    pub name: String,
    pub arn: String,
    pub tags: Vec<Tag>,
    pub versions: Vec<ModelCardVersion>,
}

impl ModelCard {
    /// A card holding exactly `first`, the version `CreateModelCard` minted.
    pub fn new(name: String, arn: String, tags: Vec<Tag>, first: ModelCardVersion) -> Self {
        Self {
            name,
            arn,
            tags,
            versions: vec![first],
        }
    }

    /// The highest-numbered version — what a reader that names no version describes.
    pub fn latest_version(&self) -> Option<&ModelCardVersion> {
        self.versions.last()
    }

    /// The version numbered `version`, or `None` when the card never had it.
    pub fn version(&self, version: i32) -> Option<&ModelCardVersion> {
        self.versions.iter().find(|held| held.version == version)
    }

    /// The highest-numbered version, mutably — the one version an in-place change (a
    /// status update) applies to; the versions before it are history and stay as they
    /// were recorded.
    pub fn latest_version_mut(&mut self) -> Option<&mut ModelCardVersion> {
        self.versions.last_mut()
    }

    /// The number the NEXT version of this card carries: one past the highest it holds,
    /// so a card's version history is the consecutive run starting at
    /// [`FIRST_MODEL_CARD_VERSION`].
    pub fn next_version(&self) -> i32 {
        self.versions
            .last()
            .map_or(FIRST_MODEL_CARD_VERSION, |held| {
                held.version.saturating_add(1)
            })
    }

    /// Whether this card can gain another version. See [`MODEL_CARD_VERSION_QUOTA`].
    pub fn versions_below_quota(&self) -> bool {
        self.versions.len() < MODEL_CARD_VERSION_QUOTA
    }
}

/// Whether `content` can be a model card's `Content`.
///
/// `Content`'s own documentation says it "must be in model card JSON schema and provided
/// as a string", so a value that is not even a JSON object is not a model card. The
/// model constrains only the member's length, so the boundary validator passes such a
/// value and the service refuses it. The schema's own required members are NOT checked
/// here: this is the one structural claim the model's documentation makes.
pub fn is_model_card_content(content: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(content).is_ok_and(|parsed| parsed.is_object())
}

// ---------------------------------------------------------------------------
// Lineage actions (`CreateAction` / `DescribeAction` / `DeleteAction`).
//
// Kept together at the end of the file, rather than interleaved with the stores
// above, so this slice is one block to read — and one block to merge.
// ---------------------------------------------------------------------------

/// See [`MODEL_QUOTA`]. Bounds the lineage actions `CreateAction` registers; every
/// record it holds is released by `DeleteAction`.
pub const ACTION_QUOTA: usize = 1_000;

/// A lineage action created by `CreateAction`, as `DescribeAction` replays it and
/// `DeleteAction` releases it.
///
/// Every optional member is held as an `Option` rather than flattened to a default, so
/// a reader can tell "the caller sent nothing" from "the caller sent an empty value";
/// `properties` is a `BTreeMap` so the map serialises in one stable order whatever
/// order the request arrived in.
#[derive(Debug, Clone)]
pub struct Action {
    pub name: String,
    pub arn: String,
    /// The Region the ARN names, i.e. the one the creating request signed for. Held
    /// separately so the lineage group reported for this action does not have to be
    /// parsed back out of the ARN.
    pub region: String,
    pub source: crate::ops::types::ActionSource,
    pub action_type: String,
    pub description: Option<String>,
    pub status: Option<String>,
    pub properties: Option<BTreeMap<String, String>>,
    pub metadata_properties: Option<MetadataProperties>,
    /// What the create carried, so a later `ListTags`/`AddTags` reports it; no
    /// implemented operation surfaces it yet (`DescribeAction` models no `Tags` member).
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another lineage action fits, i.e. the store is below [`ACTION_QUOTA`].
    pub fn actions_below_quota(&self) -> bool {
        self.actions.len() < ACTION_QUOTA
    }

    /// The key [`ComAmazonawsSagemakerSagemakerState::actions`] holds an action under,
    /// for an identifier the caller gave as either a name or an ARN (the lineage
    /// readers' `ExperimentEntityNameOrArn`).
    ///
    /// The store is keyed by `ActionArn`, which carries both the Region the name is
    /// unique within and the name itself — so a bare name is resolved against `region`,
    /// the Region this request signed for, while an ARN is matched as a WHOLE: it
    /// addresses the one action that ARN names, and one differing in the entity kind,
    /// the account, the partition or the Region addresses a different action.
    pub fn action_key(region: &str, identifier: &str) -> String {
        if identifier.starts_with("arn:") {
            identifier.to_string()
        } else {
            resource_arn(region, ACTION_KIND, identifier)
        }
    }
}

// ---------------------------------------------------------------------------
// The AI-benchmarking workload-configuration slice: `CreateAIWorkloadConfig`,
// `DescribeAIWorkloadConfig` and the `DeleteAIWorkloadConfig` that releases the record.
// Kept together at the end of this module, rather than spread through the blocks above,
// so this one resource kind reads as one slice.
// ---------------------------------------------------------------------------

/// The ARN resource kind an AI workload configuration carries, i.e. the `<kind>` in
/// [`resource_arn`]. Fixed by `AIWorkloadConfigArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:ai-workload-config/...`), and named
/// once here so the create and the describe mint and match the same ARN.
pub const AI_WORKLOAD_CONFIG_KIND: &str = "ai-workload-config";

/// See [`MODEL_QUOTA`]. Bounds the AI workload configuration store the same way, and is
/// what `CreateAIWorkloadConfig` answers `ResourceLimitExceeded` past.
pub const AI_WORKLOAD_CONFIG_QUOTA: usize = 1_000;

/// Why a reader or the delete could not resolve an AI workload configuration, named once
/// here so `DescribeAIWorkloadConfig` and `DeleteAIWorkloadConfig` refuse an unknown name
/// with the same sentence, naming the ARN they could not resolve.
pub fn ai_workload_config_not_found(arn: &str) -> String {
    format!("Could not find AI workload config \"{arn}\".")
}

/// An AI workload configuration created by `CreateAIWorkloadConfig`, as
/// `DescribeAIWorkloadConfig` replays it.
///
/// A workload configuration is inert data — a dataset configuration plus a benchmark
/// tool specification — so it carries no status: the service returns it ready, and
/// there is no transition for the clock to owe.
#[derive(Debug, Clone)]
pub struct AIWorkloadConfig {
    pub name: String,
    pub arn: String,
    pub dataset_config: Option<crate::ops::types::AIDatasetConfig>,
    pub workload_configs: Option<crate::ops::types::AIWorkloadConfigs>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another AI workload configuration fits, i.e. the store is below
    /// [`AI_WORKLOAD_CONFIG_QUOTA`]. A second inherent `impl` block, beside this slice's
    /// own types rather than in the one above.
    pub fn ai_workload_configs_below_quota(&self) -> bool {
        self.ai_workload_configs.len() < AI_WORKLOAD_CONFIG_QUOTA
    }

    /// Whether another AI benchmark job fits, i.e. the store is below
    /// [`AI_BENCHMARK_JOB_QUOTA`].
    pub fn ai_benchmark_jobs_below_quota(&self) -> bool {
        self.ai_benchmark_jobs.len() < AI_BENCHMARK_JOB_QUOTA
    }
}

// ---------------------------------------------------------------------------
// The AI benchmark job slice: what `CreateAIBenchmarkJob` writes and
// `DescribeAIBenchmarkJob` replays, beside the workload-configuration slice the jobs
// reference.
// ---------------------------------------------------------------------------

/// The ARN resource kind an AI benchmark job carries, i.e. the `<kind>` in
/// [`resource_arn`]. Fixed by `AIBenchmarkJobArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:ai-benchmark-job/...`), and named
/// once here so the create and the describe mint and match the same ARN.
pub const AI_BENCHMARK_JOB_KIND: &str = "ai-benchmark-job";

/// See [`MODEL_QUOTA`]. Bounds the AI benchmark job store the same way, and is what
/// `CreateAIBenchmarkJob` answers `ResourceLimitExceeded` past.
pub const AI_BENCHMARK_JOB_QUOTA: usize = 1_000;

/// `AIBenchmarkJobStatus`'s members this emulator moves a benchmark job through
/// (`smithy_query(query="shape",shape="AIBenchmarkJobStatus")` answers `InProgress`,
/// `Completed`, `Failed`, `Stopping` and `Stopped`; the `Completed` and stop states are
/// not reached by creating one). Named once here so no handler compares a bare string
/// literal.
pub const AI_BENCHMARK_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`AI_BENCHMARK_STATUS_IN_PROGRESS`].
pub const AI_BENCHMARK_STATUS_FAILED: &str = "Failed";

/// How long after `CreateAIBenchmarkJob` the clock owes the job's move out of
/// `InProgress` into its terminal status.
///
/// A span chosen by this emulator (AWS documents no duration for a benchmark run),
/// inside the 250–750 ms band the project fixes for a transition the clock owes. It is
/// never zero, so a caller that describes the job straight after creating it still
/// observes `InProgress`, and one that waits observes the terminal status, so a waiter
/// terminates.
pub const AI_BENCHMARK_JOB_ENDS_AFTER: Duration = Duration::from_millis(400);

/// `FailureReason` of a benchmark job whose run nothing could perform here.
///
/// A benchmark job drives real traffic at a live inference endpoint and measures its
/// latency, throughput and cost. This emulator serves no inference data plane and runs
/// no benchmark tool, and the engine contract (`vendor/engine/src/lib.rs`) forbids
/// reporting a status, duration or measurement for work nothing ran — so the run the
/// caller asked for ends in the modelled `Failed` state with this reason, and no
/// `StartTime`, `Completed` status or benchmark result is invented. Everything around
/// the run — accepting the job, resolving its workload configuration, minting its ARN,
/// replaying it, reporting its terminal status — is real.
pub const NO_BENCHMARK_RUNTIME_REASON: &str =
    "This emulator serves no inference data plane and runs no benchmark tool, so the \
     benchmark job's run was never started";

/// An AI benchmark job created by `CreateAIBenchmarkJob`, as `DescribeAIBenchmarkJob`
/// replays it.
///
/// `ends_at` is the instant the clock owes the move out of `InProgress`; it is stored,
/// never slept on, and materialised by whichever reporting call first runs at or after
/// it (see [`AIBenchmarkJob::settle_status`]).
#[derive(Debug, Clone)]
pub struct AIBenchmarkJob {
    pub name: String,
    pub arn: String,
    /// The `AIWorkloadConfigIdentifier` the request presented, kept VERBATIM (a bare
    /// name or the configuration's ARN) so the read replays what the caller sent rather
    /// than a form it did not use.
    pub workload_config_identifier: String,
    pub benchmark_target: crate::ops::types::AIBenchmarkTarget,
    /// What the request's `OutputConfig` carried. `DescribeAIBenchmarkJob` reports it as
    /// an `AIBenchmarkOutputResult`, which adds the log streams of a run — and nothing
    /// ran here, so that member stays absent.
    pub output_config: crate::ops::types::AIBenchmarkOutputConfig,
    pub network_config: Option<crate::ops::types::AIBenchmarkNetworkConfig>,
    pub role_arn: String,
    pub tags: Vec<Tag>,
    pub status: String,
    pub failure_reason: Option<String>,
    pub creation_time: SystemTime,
    pub end_time: Option<SystemTime>,
    pub ends_at: SystemTime,
}

impl AIBenchmarkJob {
    /// Materialise the move out of `InProgress` the clock already owes.
    ///
    /// This is the one write a reporting operation may make: the move was due at
    /// `ends_at` whether or not anybody described the job, so the observer makes no
    /// difference to the result. `EndTime` takes the instant the move was due, not
    /// "now", so it is stable across later reads.
    ///
    /// The terminal status is [`AI_BENCHMARK_STATUS_FAILED`] with
    /// [`NO_BENCHMARK_RUNTIME_REASON`]: nothing benchmarked the target, so no
    /// `Completed` status and no `StartTime` is invented for it.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == AI_BENCHMARK_STATUS_IN_PROGRESS && now >= self.ends_at {
            self.status = AI_BENCHMARK_STATUS_FAILED.to_string();
            self.failure_reason = Some(NO_BENCHMARK_RUNTIME_REASON.to_string());
            // The instant the service gave up on the run, which is when the job ended.
            // No start time is recorded beside it: the run never started.
            self.end_time = Some(self.ends_at);
            return true;
        }
        false
    }
}

// ---------------------------------------------------------------------------
// SageMaker Partner AI Apps: the store `ListPartnerApps` pages. Kept together at the
// end of this module, the way the lineage-action and workload-configuration slices
// above are, so this resource kind reads as one slice — and merges as one block.
// ---------------------------------------------------------------------------

/// `PartnerAppStatus`'s members this emulator moves a partner AI app through (both are
/// members of `crate::ops::types::PARTNER_APP_STATUS_VALUES`; the modelled enum also
/// carries the update, failure and delete states, which this slice does not reach).
/// A separate pair from [`STATUS_CREATING`], which names `EndpointStatus`'s members.
pub const PARTNER_APP_STATUS_CREATING: &str = "Creating";
/// See [`PARTNER_APP_STATUS_CREATING`].
pub const PARTNER_APP_STATUS_AVAILABLE: &str = "Available";

/// `ListPartnerApps`' documented `MaxResults` default, applied when the caller names
/// none: "The default value for MaxResults is 10."
/// (`smithy_query(query="doc",shape="ListPartnerAppsRequest",member="MaxResults")`).
/// [`MAX_PAGE_SIZE`] still caps the page above it, as it does for every listing here.
pub const PARTNER_APP_DEFAULT_PAGE_SIZE: i32 = 10;

/// A SageMaker Partner AI App, as `ListPartnerApps` summarises it.
///
/// Keyed in state by [`PartnerApp::app_id`] (the ARN's `app-...` resource id), which AWS
/// mints and every later call addresses the app by. `CreatePartnerApp` is still a
/// scaffold stub, so nothing in this emulator fills the store yet; `ListPartnerApps`
/// pages whatever it holds, and the creating, updating and deleting handlers extend
/// [`ComAmazonawsSagemakerSagemakerState::partner_apps`] rather than adding a second map.
///
/// `available_at` is the instant the clock owes the `Creating` -> `Available` transition;
/// it is stored, never slept on, and materialised by whichever reporting call first runs
/// at or after it.
#[derive(Debug, Clone)]
pub struct PartnerApp {
    pub app_id: String,
    pub arn: String,
    pub name: String,
    pub app_type: String,
    pub status: String,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub available_at: SystemTime,
}

impl PartnerApp {
    /// Materialise the `Creating` -> `Available` transition the clock already owes, the
    /// same way [`Endpoint::settle_status`] does for an endpoint: it was due at
    /// `available_at` whether or not anybody listed the apps, so the observer makes no
    /// difference to the result, and `LastModifiedTime` moves to the instant the
    /// transition was due rather than to "now".
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == PARTNER_APP_STATUS_CREATING && now >= self.available_at {
            self.status = PARTNER_APP_STATUS_AVAILABLE.to_string();
            self.last_modified_time = self.available_at;
            return true;
        }
        false
    }
}

// ---------------------------------------------------------------------------
// The SageMaker HyperPod cluster slice: everything `CreateCluster` writes and
// `ListClusters` reports, kept together at the end of this module the way the
// lineage-action and AI-workload slices above are, with its own quota, its own
// clock-owed span and its own inherent `impl` block on the state struct.
// ---------------------------------------------------------------------------

/// How long after `CreateCluster` the clock owes the `Creating` -> `InService`
/// transition of a HyperPod cluster. Same compressed, never-zero span as
/// [`ENDPOINT_IN_SERVICE_AFTER`]: a caller that lists clusters straight after creating
/// one still observes `Creating`, one that waits observes `InService`. Real HyperPod
/// provisioning takes many minutes; the span is the emulator's own choice.
pub const CLUSTER_IN_SERVICE_AFTER: Duration = Duration::from_millis(300);

/// See [`MODEL_QUOTA`]; bounds the cluster store, and is released by `DeleteCluster`.
pub const CLUSTER_QUOTA: usize = 1_000;

/// `ClusterSortBy`'s documented default ("The field by which to sort results. The
/// default value is `CREATION_TIME`." — `ListClusters`' member doc). `ListClusters` has
/// its own default and its own enum tokens, which is why it does not share
/// [`DEFAULT_SORT_BY`].
pub const CLUSTER_SORT_BY_CREATION_TIME: &str = "CREATION_TIME";
/// `ClusterSortBy`'s other member.
pub const CLUSTER_SORT_BY_NAME: &str = "NAME";
/// `SortOrder`'s documented default for `ListClusters` ("The sort order for results. The
/// default value is `Ascending`.") — the opposite of [`DEFAULT_SORT_ORDER`], which is
/// what `ListModels` documents. The descending token is [`ORDER_DESCENDING`].
pub const CLUSTER_DEFAULT_SORT_ORDER: &str = "Ascending";

/// A SageMaker HyperPod cluster created by `CreateCluster`, as `ListClusters`
/// summarises it and `DescribeCluster` replays it.
///
/// The whole creation spec is stored, not only the members `ListClusters` reports: the
/// record is the resource, so the readers of the rest (`DescribeCluster`,
/// `ListClusterNodes`) read it from here rather than re-deriving it.
///
/// `in_service_at` is the instant the clock owes the `Creating` -> `InService`
/// transition; it is stored, never slept on, and materialised by whichever reporting
/// call first runs at or after it.
#[derive(Debug, Clone)]
pub struct Cluster {
    pub name: String,
    /// The 12-character id the `ClusterArn` pattern carries
    /// (`…:cluster/[a-z0-9]{12}`), minted once at create time.
    pub id: String,
    pub arn: String,
    pub status: String,
    pub instance_groups: Vec<crate::ops::types::ClusterInstanceGroupSpecification>,
    pub restricted_instance_groups:
        Vec<crate::ops::types::ClusterRestrictedInstanceGroupSpecification>,
    pub restricted_instance_groups_config:
        Option<crate::ops::types::ClusterRestrictedInstanceGroupsConfig>,
    pub vpc_config: Option<VpcConfig>,
    pub orchestrator: Option<crate::ops::types::ClusterOrchestrator>,
    pub node_recovery: Option<String>,
    pub tiered_storage_config: Option<crate::ops::types::ClusterTieredStorageConfig>,
    pub node_provisioning_mode: Option<String>,
    pub cluster_role: Option<String>,
    pub auto_scaling: Option<crate::ops::types::ClusterAutoScalingConfig>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub in_service_at: SystemTime,
}

impl Cluster {
    /// Materialise the `Creating` -> `InService` transition the clock already owes.
    ///
    /// The one write a reporting operation may make: the transition was due at
    /// `in_service_at` whether or not anybody listed the cluster, so the observer makes
    /// no difference to the result. Answers whether it happened on this call.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == STATUS_CREATING && now >= self.in_service_at {
            self.status = STATUS_IN_SERVICE.to_string();
            return true;
        }
        false
    }

    /// The training plans this cluster's capacity is reserved against, in first-seen
    /// order with duplicates dropped.
    ///
    /// `ClusterSummary::TrainingPlanArns` is "a list of ARNs of the training plans
    /// associated with this cluster", and a cluster is associated with a plan through
    /// the `TrainingPlanArn` of an instance group it was created with — so the list is
    /// computed from the stored groups rather than stored a second time.
    pub fn training_plan_arns(&self) -> Vec<String> {
        let mut arns: Vec<String> = Vec::new();
        let from_groups = self
            .instance_groups
            .iter()
            .map(|group| group.training_plan_arn.as_ref())
            .chain(
                self.restricted_instance_groups
                    .iter()
                    .map(|group| group.training_plan_arn.as_ref()),
            );
        for arn in from_groups.flatten() {
            if !arns.iter().any(|seen| seen == arn) {
                arns.push(arn.clone());
            }
        }
        arns
    }
}

/// Mint the id a `ClusterArn` carries: the first 12 of the 32 lowercase hex digits
/// [`mint_opaque_resource_id`] produces, which is exactly the `[a-z0-9]{12}` the
/// modelled `ClusterArn` pattern admits.
///
/// One minter for the service: the entropy, its bounds and the reasons it is not a
/// secret are stated once, on [`mint_opaque_resource_id`]. `CreateCluster` still
/// re-mints on a collision rather than assuming uniqueness.
pub fn mint_cluster_id() -> String {
    mint_opaque_resource_id().chars().take(12).collect()
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another cluster fits, i.e. the store is below [`CLUSTER_QUOTA`]. A second
    /// inherent `impl` block, beside this slice's own types rather than in the one above.
    pub fn clusters_below_quota(&self) -> bool {
        self.clusters.len() < CLUSTER_QUOTA
    }
}

// ---------------------------------------------------------------------------
// The experiment-tracking slice: `CreateTrialComponent` and `DescribeTrialComponent`.
// Kept together at the end of this module, the way the AI-benchmarking slice above is,
// rather than spread through the blocks at the top.
// ---------------------------------------------------------------------------

/// The ARN resource kind a trial component carries, i.e. the `<kind>` in
/// [`resource_arn`]. Fixed by `TrialComponentArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:experiment-trial-component/`), and
/// named once here so the create and the describe mint and match the same ARN.
pub const TRIAL_COMPONENT_KIND: &str = "experiment-trial-component";

/// See [`MODEL_QUOTA`]. Bounds the trial-component store the same way, and is what
/// `CreateTrialComponent` answers `ResourceLimitExceeded` past.
pub const TRIAL_COMPONENT_QUOTA: usize = 1_000;

/// A trial component created by `CreateTrialComponent`, as `DescribeTrialComponent`
/// replays it.
///
/// `start_time` / `end_time` keep the modelled `Timestamp` the CALLER sent rather than a
/// `SystemTime`, so a describe replays the instant bit-for-bit however much precision the
/// request carried; `creation_time` / `last_modified_time` are the emulator's own clock
/// readings, so they are `SystemTime` like every other record here. `created_by` is stored
/// at creation rather than rebuilt per read, so every later read reports the identity that
/// actually created the component.
#[derive(Debug, Clone)]
pub struct TrialComponent {
    pub name: String,
    pub arn: String,
    /// Never `None`: `DisplayName`'s model doc says "If DisplayName isn't specified,
    /// TrialComponentName is displayed", so a create without one stores the name.
    pub display_name: String,
    pub status: Option<crate::ops::types::TrialComponentStatus>,
    pub start_time: Option<::aws_smithy_types::DateTime>,
    pub end_time: Option<::aws_smithy_types::DateTime>,
    pub parameters: Option<HashMap<String, crate::ops::types::TrialComponentParameterValue>>,
    pub input_artifacts: Option<HashMap<String, crate::ops::types::TrialComponentArtifact>>,
    pub output_artifacts: Option<HashMap<String, crate::ops::types::TrialComponentArtifact>>,
    pub metadata_properties: Option<MetadataProperties>,
    pub created_by: UserContext,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another trial component fits, i.e. the store is below
    /// [`TRIAL_COMPONENT_QUOTA`]. A second inherent `impl` block, beside this slice's own
    /// types rather than in the one above.
    pub fn trial_components_below_quota(&self) -> bool {
        self.trial_components.len() < TRIAL_COMPONENT_QUOTA
    }
}

/// The store key a `TrialComponentName` (or any other `ExperimentEntityName`) maps to.
///
/// The member's model doc says the name "must be unique in your Amazon Web Services
/// account and is not case-sensitive", so two names differing only in case are ONE
/// entity: the store is keyed by the case-folded name, while the record keeps the casing
/// the caller sent so a reader replays it. `ExperimentEntityName` is ASCII (pattern
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$`), so ASCII folding is exact for every name the
/// boundary validator lets through.
pub fn entity_key(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// The entity NAME a member modelled `ExperimentEntityNameOrArn` denotes, or `None` when
/// the value is an ARN that denotes something else.
///
/// Those members accept either the bare name or the entity's ARN, so a reader resolves
/// both forms to one store key rather than keying the store by the string the caller
/// typed. An ARN denotes an entity of THIS service only when it is an ARN this service
/// mints: `arn:aws:sagemaker:<the request's region>:<the emulator's account>:<kind>/<name>`.
/// An ARN naming another service, account, region or resource kind names no entity here —
/// the caller gets the reader's not-found error rather than whatever entity happens to
/// share the ARN's trailing path segment. That validation is why this is stricter than
/// [`name_from_arn_or_name`], which serves the `ArnOrName` readers whose pattern admits
/// one resource kind only: `ExperimentEntityNameOrArn` also admits the ARNs of OTHER
/// entity kinds (experiment, trial, artifact, action, context), so the kind has to be
/// checked rather than assumed.
pub fn entity_name_from_name_or_arn<'v>(
    value: &'v str,
    region: &str,
    kind: &str,
) -> Option<&'v str> {
    if !value.starts_with("arn:") {
        return Some(value);
    }
    let fields: Vec<&str> = value.splitn(6, ':').collect();
    let [prefix, partition, service, arn_region, account, resource] = fields[..] else {
        return None;
    };
    // Only the `aws` partition: that is the one every ARN this emulator mints carries,
    // and a partition is a closed world — an `aws-cn` ARN names no entity in `aws`.
    if prefix != "arn"
        || partition != "aws"
        || service != ARN_SERVICE
        || arn_region != region
        || account != ::emulator_server::EMULATOR_ACCOUNT_ID
    {
        return None;
    }
    let name = resource.strip_prefix(kind)?.strip_prefix('/')?;
    (!name.is_empty() && !name.contains('/')).then_some(name)
}

// ---------------------------------------------------------------------------
// The experiment-management slice: `CreateExperiment` and `DescribeExperiment`. Kept
// together as one span, the way the slices above are, so this one resource kind reads
// as one block. The store key is [`entity_key`], shared with the experiment-tracking
// slice: an `ExperimentEntityName` is unique per account and not case-sensitive.
// ---------------------------------------------------------------------------

/// The ARN resource kind an experiment carries, as the modelled `ExperimentArn` pattern
/// spells it (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:experiment/`).
///
/// Named once, the way the lineage slices name theirs, so `CreateExperiment`'s ARN
/// minting and the ARN PARSING that resolves an `experiment/...` ARN back to a stored
/// record cannot drift apart.
pub const EXPERIMENT_KIND: &str = "experiment";

/// See [`MODEL_QUOTA`]. Bounds the experiment store the same way, and is what
/// `CreateExperiment` answers `ResourceLimitExceeded` past.
pub const EXPERIMENT_QUOTA: usize = 1_000;

/// An experiment created by `CreateExperiment`, as `DescribeExperiment` replays it.
///
/// `name` keeps the spelling the creating request used; the store's key is
/// [`entity_key`] of that name, so a later create or read that differs only in case
/// resolves to this same record. `created_by` is stored at creation rather than rebuilt
/// per read, so every later read reports the identity that actually created it.
#[derive(Debug, Clone)]
pub struct Experiment {
    pub name: String,
    pub arn: String,
    /// What `DescribeExperiment` reports as `DisplayName`: the value the request
    /// supplied, or `ExperimentName` when it supplied none — the model documents that
    /// "if DisplayName isn't specified, ExperimentName is displayed".
    pub display_name: String,
    pub description: Option<String>,
    pub created_by: UserContext,
    /// The tags the create carried, which `AddTags`/`DeleteTags`/`ListTags` and `Search`
    /// read once those operations are implemented (they are scaffolded stubs today).
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another experiment fits, i.e. the store is below [`EXPERIMENT_QUOTA`].
    pub fn experiments_below_quota(&self) -> bool {
        self.experiments.len() < EXPERIMENT_QUOTA
    }

    /// The experiment `name` identifies, whatever case it is written in — the name is
    /// unique per account and not case-sensitive, so the lookup folds case the way
    /// [`entity_key`] does.
    pub fn experiment(&self, name: &str) -> Option<&Experiment> {
        self.experiments.get(&entity_key(name))
    }
}

// ---------------------------------------------------------------------------
// The custom-image slice: `CreateImage` / `CreateImageVersion` and the two
// `Describe*` readers. Kept together at the end of this module, the way the
// lineage-action, model-card and notebook-instance slices are, so one slice's
// records, constants and helpers read as one unit.
// ---------------------------------------------------------------------------

/// How long after `CreateImage` / `CreateImageVersion` the clock owes the
/// `CREATING` -> `CREATED` transition.
///
/// SageMaker AI returns a freshly created image and image version in `CREATING` and
/// moves it to `CREATED` once the registry path is resolved, so a caller that describes
/// straight after creating still observes `CREATING` and one that waits observes
/// `CREATED`. Real SageMaker takes seconds; the span is compressed, never zero.
pub const IMAGE_CREATED_AFTER: Duration = Duration::from_millis(300);

/// See [`MODEL_QUOTA`].
pub const IMAGE_QUOTA: usize = 1_000;

/// See [`MODEL_QUOTA`]: the bound is per image, since versions are numbered within one
/// image and `CreateImageVersion` is the only call that adds one.
pub const IMAGE_VERSION_QUOTA: usize = 1_000;

/// The most aliases one image version carries, after which `CreateImageVersion` is
/// refused with the modelled `ResourceLimitExceeded`.
///
/// `SageMakerImageVersionAliases` is modelled with no length, so nothing at the request
/// boundary bounds the list: without this the request would choose both how much is
/// stored per version and how much work the alias-collision check does. It is the
/// emulator's OWN bound rather than a claim about any account's limit, far above any
/// workflow an emulator serves (a version is normally tagged with a handful of aliases).
pub const MAX_ALIASES_PER_IMAGE_VERSION: usize = 50;

/// The `ImageStatus` / `ImageVersionStatus` members this emulator moves an image and an
/// image version through. Both modelled enums spell the creating and created states the
/// same way (`CREATING`, `CREATED`) and also carry the failure, update and delete states,
/// which this slice does not reach. Named once here so no handler compares a bare string
/// literal.
pub const IMAGE_STATUS_CREATING: &str = "CREATING";
/// See [`IMAGE_STATUS_CREATING`].
pub const IMAGE_STATUS_CREATED: &str = "CREATED";

/// `VendorGuidance`'s member for a version whose maintainer named no stability, which
/// the model documents as "The maintainers did not provide a status for image version
/// stability" — so it is what a version created without `VendorGuidance` reports.
pub const VENDOR_GUIDANCE_NOT_PROVIDED: &str = "NOT_PROVIDED";

/// A custom SageMaker AI image created by `CreateImage`, as `DescribeImage` replays it
/// and `CreateImageVersion` reads it.
///
/// `created_at` is the instant the clock owes the `CREATING` -> `CREATED` transition;
/// it is stored, never slept on, and materialised by whichever reporting call first
/// runs at or after it. `next_version` is the number the NEXT version of this image
/// takes: it only ever increases, so a version number — which an image version's ARN
/// carries — is never handed to two different container images.
#[derive(Debug, Clone)]
pub struct Image {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub role_arn: String,
    pub status: String,
    pub failure_reason: Option<String>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub created_at: SystemTime,
    pub next_version: i32,
}

/// A version of an image created by `CreateImageVersion`, as `DescribeImageVersion`
/// replays it.
///
/// `created_at` carries the same owed transition as [`Image::created_at`].
/// `client_token` is the request's `@idempotencyToken`, kept so a retried create
/// answers with the version the first call minted instead of minting a second one.
#[derive(Debug, Clone)]
pub struct ImageVersion {
    pub image_name: String,
    pub image_arn: String,
    pub version: i32,
    pub arn: String,
    pub base_image: String,
    pub client_token: String,
    pub aliases: Vec<String>,
    pub vendor_guidance: String,
    pub job_type: Option<String>,
    pub ml_framework: Option<String>,
    pub programming_lang: Option<String>,
    pub processor: Option<String>,
    pub horovod: Option<bool>,
    pub release_notes: Option<String>,
    pub status: String,
    pub failure_reason: Option<String>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub created_at: SystemTime,
}

/// Materialise the `CREATING` -> `CREATED` transition the clock already owes to the
/// record whose `status` is `*status` and whose transition was due at `created_at`.
///
/// This is the one write a reporting operation may make: the transition was due
/// whether or not anybody described the resource, so the observer makes no difference
/// to the result. `LastModifiedTime` moves to the instant the transition was due, not
/// to "now", so the value is stable across later reads.
///
/// Answers whether the transition happened on this call, so the caller can record it.
fn settle_created(
    status: &mut String,
    last_modified_time: &mut SystemTime,
    created_at: SystemTime,
    now: SystemTime,
) -> bool {
    if status == IMAGE_STATUS_CREATING && now >= created_at {
        *status = IMAGE_STATUS_CREATED.to_string();
        *last_modified_time = created_at;
        return true;
    }
    false
}

impl Image {
    /// See [`settle_created`].
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        settle_created(
            &mut self.status,
            &mut self.last_modified_time,
            self.created_at,
            now,
        )
    }
}

impl ImageVersion {
    /// See [`settle_created`].
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        settle_created(
            &mut self.status,
            &mut self.last_modified_time,
            self.created_at,
            now,
        )
    }
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another image fits, i.e. the store is below [`IMAGE_QUOTA`].
    pub fn images_below_quota(&self) -> bool {
        self.images.len() < IMAGE_QUOTA
    }

    /// Every stored version of `image_name`, in ascending version order.
    pub fn versions_of<'store>(
        &'store self,
        image_name: &str,
    ) -> impl DoubleEndedIterator<Item = &'store ImageVersion> + 'store {
        self.image_versions
            .range((image_name.to_string(), i32::MIN)..=(image_name.to_string(), i32::MAX))
            .map(|(_, version)| version)
    }

    /// Whether another version of `image_name` fits, i.e. the image holds fewer than
    /// [`IMAGE_VERSION_QUOTA`] of them.
    pub fn image_versions_below_quota(&self, image_name: &str) -> bool {
        self.versions_of(image_name).count() < IMAGE_VERSION_QUOTA
    }

    /// The highest-numbered stored version of `image_name` — what
    /// `DescribeImageVersion` describes when the request names neither a version nor an
    /// alias.
    pub fn latest_version_of(&self, image_name: &str) -> Option<&ImageVersion> {
        self.versions_of(image_name).next_back()
    }
}

// ---------------------------------------------------------------------------
// The lineage-association slice: `AddAssociation` and the `DeleteAssociation` that
// releases one. Kept together at the end of this module, the way the lineage-action,
// model-card and custom-image slices are, so one slice's record, constants and helpers
// read as one unit.
// ---------------------------------------------------------------------------

/// The `<kind>` component of an `artifact/<id>` ARN, i.e. the kind `CreateArtifact`
/// mints and the one an `AssociationEntityArn` names an artifact by. Named here so the
/// create and the association slice mint and resolve the same kind.
pub const ARTIFACT_KIND: &str = "artifact";

/// The `<kind>` component of an `action/<name>` ARN. See [`ARTIFACT_KIND`].
pub const ACTION_KIND: &str = "action";

/// See [`MODEL_QUOTA`] for why the bound is the emulator's OWN rather than a claim about
/// any account's limit. It bounds the association store the same way, and is what
/// `AddAssociation` answers `ResourceLimitExceeded` past. It is higher than the
/// per-resource bounds because an association is an EDGE: the store grows with the PAIRS
/// of lineage entities a caller connects, not with the entities themselves, and every
/// entry it holds is released by `DeleteAssociation`.
pub const ASSOCIATION_QUOTA: usize = 10_000;

/// One lineage association created by `AddAssociation`, as `DeleteAssociation` releases
/// it.
///
/// The `(source_arn, destination_arn)` pair is the association's identity — SageMaker
/// mints no id for an association and no operation addresses one by anything else — so
/// the pair is the store key and these two fields repeat it, the way every other record
/// here keeps the identifier it is filed under. `association_type` stays an `Option`
/// because `AssociationType` is an optional member: an association created without one
/// carries none, which is distinguishable on the wire from any of the modelled edge
/// types. `creation_time` is the instant the association was FIRST added, so a repeated
/// add does not pretend the edge is new.
#[derive(Debug, Clone)]
pub struct Association {
    pub source_arn: String,
    pub destination_arn: String,
    pub association_type: Option<String>,
    pub creation_time: SystemTime,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another association fits, i.e. the store is below [`ASSOCIATION_QUOTA`].
    pub fn associations_below_quota(&self) -> bool {
        self.associations.len() < ASSOCIATION_QUOTA
    }

    /// Whether `arn` names a lineage entity this service holds, i.e. one an association
    /// may point at.
    ///
    /// `AssociationEntityArn`'s modelled pattern admits five entity kinds — experiment,
    /// experiment-trial-component, artifact, action and context. Each is resolved against
    /// the store the matching create writes to, through
    /// [`entity_name_from_name_or_arn`], so an ARN naming another partition, service,
    /// Region or account names no entity here. The context store does not exist yet —
    /// `CreateContext` is still a scaffold answering 501, so nothing was ever created for
    /// a `context/...` ARN to name and such an ARN names no entity.
    pub fn lineage_entity_exists(&self, region: &str, arn: &str) -> bool {
        // Every `AssociationEntityArn` IS an ARN (its pattern starts `^arn:`), so the
        // bare-name branch of `entity_name_from_name_or_arn` is unreachable from an
        // association request; guarded rather than assumed, so a bare name can never
        // resolve to an entity through this helper.
        if !arn.starts_with("arn:") {
            return false;
        }
        if entity_name_from_name_or_arn(arn, region, ARTIFACT_KIND).is_some() {
            // Both lineage stores are keyed by the WHOLE ARN the create minted, which
            // already carries this request's Region and the emulator's account.
            return self.artifacts.contains_key(arn);
        }
        if entity_name_from_name_or_arn(arn, region, ACTION_KIND).is_some() {
            return self.actions.contains_key(arn);
        }
        if let Some(name) = entity_name_from_name_or_arn(arn, region, TRIAL_COMPONENT_KIND) {
            // The trial-component store is keyed by the case-folded NAME (the name "is
            // not case-sensitive" per its model doc), so the lookup goes through the same
            // key `CreateTrialComponent` stored the record under.
            return self.trial_components.contains_key(&entity_key(name));
        }
        if let Some(name) = entity_name_from_name_or_arn(arn, region, EXPERIMENT_KIND) {
            // Same keying as the trial-component store, and for the same reason: an
            // `ExperimentEntityName` is unique per account and not case-sensitive, so
            // `CreateExperiment` filed the record under [`entity_key`] of the name.
            return self.experiments.contains_key(&entity_key(name));
        }
        false
    }
}
