// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sagemaker-sagemaker service.
//!
//! The inference-deployment lifecycle (model -> endpoint config -> endpoint) and the
//! model-card governance resources, batch-transform jobs and the AI benchmarking
//! control plane's workload configurations are kept here, in `BTreeMap`s keyed by the
//! resource name AWS keys them by, so `ListModels` has one stable order to sort and
//! paginate over and every `Describe*` replays exactly what the matching `Create*`
//! stored. The Studio
//! AppImageConfig registry and the Git repositories `CreateCodeRepository` registers
//! are kept the same way, keyed by `AppImageConfigName` and `CodeRepositoryName`.
//!
//! Studio apps live here too, keyed by the [`AppKey`] their five input members name
//! rather than by a bare name, because that tuple — domain, owning user profile or
//! space, app type, app name — is what identifies one app.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AdditionalInferenceSpecificationDefinition, ArtifactSource, AsyncInferenceConfig,
    BatchDataCaptureConfig, CodeEditorAppImageConfig, ContainerDefinition, DataCaptureConfig,
    DataProcessing, DriftCheckBaselines, ExperimentConfig, ExplainerConfig, GitConfig, IamIdentity,
    InferenceExecutionConfig, InferenceSpecification, JupyterLabAppImageConfig,
    KernelGatewayImageConfig, ManagedConfiguration, MetadataProperties, MetricsConfig,
    ModelCardSecurityConfig, ModelClientConfig, ModelLifeCycle, ModelMetrics,
    ModelPackageModelCard, ModelPackageSecurityConfig, ModelPackageStatusDetails,
    ModelPackageStatusItem, ModelPackageValidationSpecification, ProductionVariant,
    ProductionVariantSummary, ResourceSpec, SourceAlgorithmSpecification, Tag, TransformInput,
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

/// The lineage group every lineage entity this account creates belongs to.
///
/// SageMaker ML Lineage Tracking gives an account one lineage group per Region,
/// `sagemaker-default-lineage-group`, and reports its ARN as the `LineageGroupArn` of
/// each entity in it; this emulator models no other lineage group, so every artifact
/// reports this one.
pub const DEFAULT_LINEAGE_GROUP: &str = "sagemaker-default-lineage-group";

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

/// `EndpointStatus`'s members this emulator moves an endpoint through (the modelled
/// enum also carries the update, rollback and delete states, which this slice does not
/// reach). Named once here so no handler compares a bare string literal.
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
    /// Keyed by `ModelName`.
    pub models: BTreeMap<String, Model>,
    /// Keyed by `EndpointConfigName`.
    pub endpoint_configs: BTreeMap<String, EndpointConfig>,
    /// Keyed by `EndpointName`.
    pub endpoints: BTreeMap<String, Endpoint>,
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
    /// Keyed by `HubName`. See [`Hub`].
    pub hubs: BTreeMap<String, Hub>,
    /// Every stored hub content VERSION, keyed by [`HubContentKey`].
    pub hub_contents: BTreeMap<HubContentKey, HubContent>,
}

impl ComAmazonawsSagemakerSagemakerState {
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
/// `ResourceInUse` refuses with the same wording — today `CreateAppImageConfig` and
/// `CreateAIWorkloadConfig`; an operation whose model does not bind
/// `ResourceInUse` (`CreateModel`, `CreateModelPackageGroup`) refuses with its own
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
// Model cards. `CreateModelCard` stores one here and `DescribeModelCard` replays it;
// the store itself is `ComAmazonawsSagemakerSagemakerState::model_cards` above. Kept
// together at the end of the file so a model-card change touches one span of it.
// ---------------------------------------------------------------------------

/// The version number a model card's FIRST version carries. `DescribeModelCard`'s
/// `ModelCardVersion` is modelled as an `Integer` and the API's version history starts
/// at one, so `CreateModelCard` stores version 1 and each `UpdateModelCard` appends the
/// next integer.
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
            resource_arn(region, "action", identifier)
        }
    }
}

// ---------------------------------------------------------------------------
// The AI-benchmarking workload-configuration slice: `CreateAIWorkloadConfig` and
// `DescribeAIWorkloadConfig`. Kept together at the end of this module, rather than
// spread through the blocks above, so this one resource kind reads as one slice.
// ---------------------------------------------------------------------------

/// The ARN resource kind an AI workload configuration carries, i.e. the `<kind>` in
/// [`resource_arn`]. Fixed by `AIWorkloadConfigArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:ai-workload-config/...`), and named
/// once here so the create and the describe mint and match the same ARN.
pub const AI_WORKLOAD_CONFIG_KIND: &str = "ai-workload-config";

/// See [`MODEL_QUOTA`]. Bounds the AI workload configuration store the same way, and is
/// what `CreateAIWorkloadConfig` answers `ResourceLimitExceeded` past.
pub const AI_WORKLOAD_CONFIG_QUOTA: usize = 1_000;

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
}
