// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iotsitewise-awsiotsitewise service.
//!
//! The asset-model / asset / property-value, gateway, dataset and Monitor-portal
//! slices of IoT SiteWise live here: one map per resource plus the indexes the
//! modelled lookup forms need
//! (`externalId:<id>` labels, property aliases, a gateway's platform device). A
//! resource's tags live on that resource's own record, reached by ARN through
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::tags_for_resource`], so the
//! tagging operations and the creates share one tag set per resource. A data
//! stream no asset property owns lives in its own map
//! ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::disassociated_time_series`]),
//! which is what `DisassociateTimeSeriesFromAssetProperty` leaves behind and what
//! alias-addressed ingestion creates. A workspace's pipelines live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::pipelines`], keyed by the
//! workspace and the pipeline name, each record carrying every version written
//! to it, bounded by [`MAX_PIPELINE_VERSIONS`], so a read naming an older
//! version still answers from it. A workspace's tasks live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::tasks`], keyed and versioned
//! the same way, bounded by [`MAX_TASK_VERSIONS`]. A workspace's dataset export
//! jobs live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::dataset_export_jobs`], keyed by
//! the workspace and the minted job id, each record carrying the request that
//! started it and the instant it started, from which a read DERIVES how far the
//! job has got. The data segments a dataset holds live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::dataset_data_segments`], keyed
//! by the dataset, each record naming the dataset the data came from, the time
//! series it covers and the half-open span it covers of it, which is what a
//! listing reports and a deletion addresses one by. A workspace's searches live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::searches`], keyed by the
//! workspace and the search id, each record carrying the request the search was
//! started from and the instants its lifecycle transitions fall due, so the
//! status a poll reports is derived rather than stored. A workspace's
//! asynchronous queries live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::queries`], keyed the same way,
//! each record carrying what running its statement produced, the instant it was
//! submitted and the instant a cancel was recorded on it, from which a read
//! DERIVES the status the clock has made due. A
//! workspace's enrichment jobs live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::enrichment_jobs`], keyed by the
//! workspace and the minted job id, each record carrying the configuration it
//! was created from, the instant it was created and the instant a cancel was
//! recorded on it, from which a read DERIVES
//! the job's type, the data it analyses and the status the clock has made due. An
//! execution of an action lives in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::executions`], keyed by its own
//! id, which is what `ListExecutions` summarises for the target resource a
//! request names. An action taken on a target resource lives in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::actions`], keyed by its own
//! `actionId`, which is what `ListActions` summarises for the target resource a
//! request names. The service's own settings live here too:
//! the logging options, one entry per scope a call named
//! ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::logging_options`]), and the
//! account's default encryption configuration
//! ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::default_encryption`]). A
//! workspace's capture sources — the video streams `GetCaptureData` reads, and
//! the time series that workspace holds — live
//! in [`ComAmazonawsIotsitewiseAwsiotsitewiseState::capture_sources`], keyed by
//! the workspace and the source's `timeSeriesId`, with
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::capture_source_aliases`] the
//! index the alias form of that lookup resolves through. The account's
//! computation models live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::computation_models`], keyed by
//! `computationModelId`, each record carrying the configuration and data
//! binding it was created with and the actions that configuration's computation
//! type exposes. The Monitor access
//! policies live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::access_policies`], keyed by the
//! minted `accessPolicyId`, each record naming the identity, the Monitor
//! resource and the permission the last write to it stated. An interface asset
//! model applied to an asset model lives in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::interface_relationships`],
//! keyed by the two models, carrying the property and hierarchy mappings the
//! application resolved to. The applications of a workspace live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::applications`], keyed by the
//! minted `applicationId` — which identifies one across every workspace — each
//! record naming the workspace it was created in, the Identity Center instance
//! the create named and the instant the `CREATING` -> `ACTIVE` transition it
//! owes falls due. The Monitor dashboards live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::dashboards`], keyed by the
//! minted `dashboardId` — which identifies one across every project — each
//! record naming the project it was created in, holding the definition
//! literal the last write to it stated and the idempotency token its create
//! carried, which is what a retry of that create is recognised by. The
//! account's bulk import jobs live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::bulk_import_jobs`], keyed by
//! the minted `jobId` — which identifies one across the account — each record
//! carrying the request that started it and the instant it was created, from
//! which a read DERIVES the status and the last-update date the clock has made
//! due. The mutating calls already served under a `clientToken` live in
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::idempotent_calls`], keyed by
//! the operation and the token, each entry naming the request that spent the
//! token and the resource that call acted on, which is what a retry of it
//! replays from. Every
//! handler reads and writes THIS struct through [`service_state`]; nothing in
//! `src/ops/` keeps state of its own.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    ActionDefinition, ActionPayload, ActionSummary, AggregatedValue, Aggregates, Alarms,
    AssetCompositeModelPathSegment, AssetCompositeModelSummary, AssetHierarchy,
    AssetModelCompositeModel,
    AssetModelCompositeModelPathSegment, AssetModelCompositeModelSummary, AssetModelHierarchy,
    AssetModelProperty, AssetModelPropertyDefinition, AssetModelStatus, AssetProperty,
    AssetPropertyPathSegment,
    AssetPropertyValue, AssetStatus, AssetSummary, ComputationModelConfiguration,
    ComputationModelDataBindingValue, ComputationModelStatus, ComputeNode,
    ComputeNodeExecutionDetails,
    ComputeNodeExecutionStatus, ConfigurationStatus, CustomerManagedS3Storage, DatasetConfig,
    DatasetSource, DatasetStatus, EnrichmentJobConfiguration, EnrichmentJobSummary,
    ErrorReportLocation,
    ExecutionEnvironmentVariables, ExecutionStatus, ExecutionSummary, ExportErrorReportLocation,
    ExpressionVariable, File, GatewayCapabilitySummary, GatewayPlatform, HierarchyMapping, Identity,
    ImageFile, ImageLocation, JobConfiguration, LoggingOptions, Mount, MountOverrides,
    MultiLayerStorage,
    PipelineExecutionStateDetails, PipelineExecutionStatus, PortalStatus,
    PortalTypeEntry, ProcessingInput, PropertyMapping, PropertyType, QueryStatistics, QuerySummary,
    ResolveTo, Resource, ResourceStatus, RetentionPeriod, SearchFilters, TargetResource,
    TaskConfiguration, TimeInNanos, TimeSeriesSummary, TrimSettings, Variant,
    WarmTierRetentionPeriod,
    WorkspaceEncryptionConfiguration, WorkspaceEncryptionConfigurationInfo, WorkspaceStatus,
    DISASSOCIATED_DATA_STORAGE_STATE_VALUES, ENCRYPTION_TYPE_VALUES, STORAGE_TYPE_VALUES,
    VIDEO_DATA_TYPE_VALUES, WARM_TIER_STATE_VALUES,
};

/// The `iotsitewise` ARN service namespace, as `OPERATIONS.md` reports it.
pub const ARN_SERVICE: &str = "iotsitewise";

/// The region every ARN falls back to when the request carries no SigV4
/// credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after the call that starts it this emulator takes to settle a
/// resource's owed transition — `CREATING` -> `ACTIVE` after a create, and
/// `UPDATING` -> `ACTIVE` after an update.
///
/// CHOSEN, not documented: AWS publishes no duration for these transitions, only
/// that `CreateAsset`/`CreateAssetModel` answer with the resource in `CREATING`,
/// that `UpdateAsset` answers with it in `UPDATING`, and that it becomes `ACTIVE`
/// later. The span is deliberately non-zero so a caller that reads the resource
/// straight after the call still observes the transient state the real service
/// reports.
pub const ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// `AssetState`/`AssetModelState` value a freshly created resource reports.
pub const STATE_CREATING: &str = "CREATING";
/// `AssetState`/`AssetModelState` value a settled resource reports.
pub const STATE_ACTIVE: &str = "ACTIVE";
/// `AssetState`/`AssetModelState` value a resource reports while a mutating call
/// it already accepted settles.
pub const STATE_UPDATING: &str = "UPDATING";
/// `AssetState`/`AssetModelState` value a deleted resource's last answer carries.
pub const STATE_DELETING: &str = "DELETING";
/// The state value a resource whose provisioning did not complete reports. This
/// emulator settles every owed transition, so nothing it holds reaches this
/// state; the operations whose rules are stated in terms of it still have to
/// recognise it (`UpdateWorkspace` may be called on a workspace in it).
pub const STATE_FAILED: &str = "FAILED";

/// The `LoggingLevel` a scope reports until a `PutLoggingOptions` sets one.
///
/// Logging is a setting, not a resource: a scope nobody has configured is simply
/// not logging, and `OFF` is the modelled verbosity level for that. A read of an
/// unset scope reports it WITHOUT storing anything, so the first read cannot
/// change what a later caller sees.
pub const DEFAULT_LOGGING_LEVEL: &str = "OFF";

/// The key [`ComAmazonawsIotsitewiseAwsiotsitewiseState::logging_options`] holds
/// the account-and-region-wide logging options under — the ones a call that names
/// no `workspaceName` sets and reads.
///
/// The empty string, which collides with no workspace-scoped entry: the modelled
/// `WorkspaceName` is `len 1..64`, so no workspace can be named by it.
pub const ACCOUNT_LOGGING_SCOPE: &str = "";

/// How many characters an `ETag` carries: the modelled type is
/// `^[\w-]{43}$`, so every tag this service reports is exactly this long.
pub const ETAG_LENGTH: usize = 43;

/// The composite model types `assetModelCompositeModelType` accepts.
///
/// The member is modelled as a free `Name` string rather than an enum, so the
/// catalog comes from its documentation, which both
/// `CreateAssetModelCompositeModel` and `DescribeAssetModelCompositeModel` state
/// as "The composite model type. Valid values are AWS/ALARM, CUSTOM, or
/// AWS/L4E_ANOMALY".
pub const COMPOSITE_MODEL_TYPES: [&str; 3] = ["AWS/ALARM", "CUSTOM", "AWS/L4E_ANOMALY"];

/// The `AssetModelType` value an asset model must carry to be reused as the
/// component of a component-model-based composite model, as
/// `CreateAssetModelCompositeModel` documents it: "specify the
/// composedAssetModelId of an existing asset model with assetModelType of
/// COMPONENT_MODEL".
pub const MODEL_TYPE_COMPONENT: &str = "COMPONENT_MODEL";

/// How many property definitions one composite model may carry.
///
/// A PUBLISHED AWS quota, stated by `assetModelCompositeModelProperties`: "You
/// can specify up to 200 properties per composite model."
pub const MAX_PROPERTIES_PER_COMPOSITE_MODEL: usize = 200;

/// The ceiling on how many composite models one asset model holds.
///
/// The same bound, and for the same reason, as [`MAX_ASSET_MODELS`]: a create is
/// the only thing that adds a composite model and nothing evicts one, so the
/// per-model list needs a maximum entry count. The emulator's own bound, not a
/// published AWS default, and high enough that no ordinary workflow meets it;
/// `CreateAssetModelCompositeModel` refuses past it with the modelled
/// `LimitExceededException` (HTTP 410).
pub const MAX_COMPOSITE_MODELS_PER_ASSET_MODEL: usize = 100;

/// How many property definitions one asset model may carry.
///
/// A PUBLISHED AWS quota, stated by both `CreateAssetModel`'s and
/// `UpdateAssetModel`'s `assetModelProperties` member: "You can specify up to 200
/// properties per asset model." A write that would leave a model carrying more is
/// refused with the modelled `LimitExceededException` (HTTP 410).
pub const MAX_PROPERTIES_PER_ASSET_MODEL: usize = 200;

/// How many hierarchy definitions one asset model may carry.
///
/// A PUBLISHED AWS quota, stated by both `CreateAssetModel`'s and
/// `UpdateAssetModel`'s `assetModelHierarchies` member: "You can specify up to 10
/// hierarchies per asset model." Refused the same way as
/// [`MAX_PROPERTIES_PER_ASSET_MODEL`].
pub const MAX_HIERARCHIES_PER_ASSET_MODEL: usize = 10;

/// The `AssetModelVersionType` value naming a model's latest version, and the
/// documented default of every `matchForVersionType` member.
pub const VERSION_TYPE_LATEST: &str = "LATEST";

/// The `AssetModelVersionType` value naming a model's active version.
pub const VERSION_TYPE_ACTIVE: &str = "ACTIVE";

/// The one value a `SelectAll` member (`ifNoneMatch`) admits: its modelled
/// pattern is `^\*$`.
pub const SELECT_ALL: &str = "*";

/// The prefix a `CustomID` label uses to name a resource by its external id.
pub const EXTERNAL_ID_PREFIX: &str = "externalId:";

/// The ceiling on how many asset models and how many assets this store holds:
/// the two caps that bound caller-driven growth of the resource maps.
///
/// A generated emulator keeps its resources in memory for the life of the process
/// and a create is the only thing that adds one, so the store needs a maximum
/// entry count — neither a TTL nor an eviction policy is available to it, because
/// `DescribeAssetModel`/`DescribeAsset` must still answer what the create created.
/// `CreateAssetModel` and `CreateAsset` refuse past these caps with the modelled
/// `LimitExceededException` (HTTP 410), which IoT SiteWise declares on exactly the
/// operations that grow the store and on no read. The numbers are the emulator's
/// own bound, not a published AWS default: AWS publishes both quotas as adjustable,
/// so no single value is the contract, and the caps are set high enough that no
/// ordinary workflow meets one.
pub const MAX_ASSET_MODELS: usize = 1_000;

/// See [`MAX_ASSET_MODELS`].
pub const MAX_ASSETS: usize = 10_000;

/// The greatest number of tags one IoT SiteWise resource holds.
///
/// The model's own ceiling: `TagMap` carries `@length(min: 1, max: 50)` wherever
/// it is used — `TagResource.tags`, `CreateAsset.tags`, `CreateAssetModel.tags`
/// and `ListTagsForResource.tags` — so no single call may carry more than this
/// many tags and no listing may report more.
///
/// The two refusals this bound drives are DIFFERENT failures and carry different
/// modelled errors. A single call carrying more than this many tags violates
/// `TagMap`'s own `@length`, which the boundary validator refuses before a handler
/// runs. A well-formed call whose tags would carry the TARGET RESOURCE's
/// accumulated count past the ceiling is a quota refusal, so it answers the
/// modelled `LimitExceededException` (HTTP 410) — "You've reached the quota for a
/// resource" — which is the error IoT SiteWise declares for every accumulated-count
/// ceiling in this service.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The `TagKey` bounds: `TagMap`'s key targets `TagKey`, which the model
/// declares as `@length(min: 1, max: 128)`.
pub const TAG_KEY_MIN_LENGTH: usize = 1;
/// See [`TAG_KEY_MIN_LENGTH`].
pub const TAG_KEY_MAX_LENGTH: usize = 128;

/// The ARN resource-type segment of an asset ARN, as [`asset_arn`] builds it.
pub const ASSET_RESOURCE_TYPE: &str = "asset";
/// The ARN resource-type segment of an asset-model ARN, as [`asset_model_arn`]
/// builds it.
pub const ASSET_MODEL_RESOURCE_TYPE: &str = "asset-model";
/// The ARN resource-type segment of a gateway ARN, as [`gateway_arn`] builds it.
pub const GATEWAY_RESOURCE_TYPE: &str = "gateway";
/// The ARN resource-type segment of a portal ARN, as [`portal_arn`] builds it.
pub const PORTAL_RESOURCE_TYPE: &str = "portal";
/// The ARN resource-type segment of a project ARN, as [`project_arn`] builds it.
pub const PROJECT_RESOURCE_TYPE: &str = "project";
/// The ARN resource-type segment of a pipeline ARN, as [`pipeline_arn`] builds it.
pub const PIPELINE_RESOURCE_TYPE: &str = "pipeline";

/// The ARN service namespace a workspace's customer managed key lives under.
pub const KMS_ARN_SERVICE: &str = "kms";

/// How many pipelines one workspace holds.
///
/// The same kind of bound, and for the same reason, as [`MAX_GATEWAYS`]: a create
/// is the only thing that adds a pipeline and nothing evicts one, so the map needs
/// a maximum entry count. The emulator's own bound, not a published AWS default,
/// and high enough that no ordinary workflow meets it; `CreatePipeline` refuses
/// past it with the modelled `LimitExceededException` (HTTP 410).
pub const MAX_PIPELINES_PER_WORKSPACE: usize = 100;

/// How many pipelines the store holds in total, across every workspace.
///
/// The per-workspace cap above does not bound the store on its own: it bounds one
/// workspace's pipelines, and the number of workspaces is bounded in turn by
/// [`MAX_WORKSPACES`], so this cap is what states the whole map's own limit
/// directly. This is the whole map's entry count, refused past the cap with the
/// modelled `LimitExceededException` (HTTP 410) — the same bound, and for the same
/// reason, as [`MAX_GATEWAYS`].
pub const MAX_PIPELINES: usize = 1_000;

/// How many versions one pipeline keeps.
///
/// Unlike the entry-count caps above, a pipeline's version history HAS an
/// eviction policy available to it: a read answers from the newest version unless
/// it names an older one, so the OLDEST superseded version is the one that can go.
/// Past this many versions [`PipelineRecord::record_update`] drops the oldest, so
/// repeated updates cannot grow one record without limit, and a read naming an
/// evicted version is the same not-found answer as one naming a version no write
/// ever produced. The emulator's own bound; AWS publishes no retained-version
/// count.
pub const MAX_PIPELINE_VERSIONS: usize = 50;

/// The `version` a freshly created pipeline reports: its first version, counted
/// from one, as `CreatePipeline`'s `version` member ("The version of the newly
/// created pipeline") and `UpdatePipeline`'s ("The new version of the pipeline
/// created by this update") describe the counter.
pub const FIRST_PIPELINE_VERSION: u64 = 1;

/// The ARN resource-type segment of a task ARN, as [`task_arn`] builds it.
pub const TASK_RESOURCE_TYPE: &str = "task";

/// How many tasks one workspace holds.
///
/// The same kind of bound, and for the same reason, as
/// [`MAX_PIPELINES_PER_WORKSPACE`]: a create is the only thing that adds a task
/// and nothing evicts one, so the map needs a maximum entry count. The
/// emulator's own bound, not a published AWS default; `CreateTask` refuses past
/// it with the modelled `LimitExceededException` (HTTP 410).
pub const MAX_TASKS_PER_WORKSPACE: usize = 100;

/// How many tasks the store holds in total, across every workspace.
///
/// The whole map's own limit, for the reason [`MAX_PIPELINES`] states: the
/// per-workspace cap above bounds one workspace's tasks, and this one bounds the
/// map every workspace's tasks share.
pub const MAX_TASKS: usize = 1_000;

/// How many versions one task keeps.
///
/// The same eviction policy, and for the same reason, as
/// [`MAX_PIPELINE_VERSIONS`]: a read answers from the newest version unless it
/// names an older one, so past this many versions
/// [`TaskRecord::record_update`] drops the OLDEST superseded version and no
/// number of updates grows one record without limit. The emulator's own bound;
/// AWS publishes no retained-version count.
pub const MAX_TASK_VERSIONS: usize = 50;

/// The `version` a freshly created task reports: its first version, counted from
/// one, as `CreateTask`'s `version` member ("The version of the newly created
/// task") and `UpdateTask`'s ("The new version of the task created by this
/// update") describe the counter.
pub const FIRST_TASK_VERSION: u64 = 1;

/// The ARN resource-type segment of a dataset ARN, as [`dataset_arn`] builds it.
pub const DATASET_RESOURCE_TYPE: &str = "dataset";
/// The ARN resource-type segment of a workspace ARN, as [`workspace_arn`] builds
/// it.
pub const WORKSPACE_RESOURCE_TYPE: &str = "workspace";
/// The ARN resource-type segment of an access policy's ARN, as
/// [`access_policy_arn`] builds it.
pub const ACCESS_POLICY_RESOURCE_TYPE: &str = "access-policy";

/// See [`MAX_ASSET_MODELS`]: the same bound, for the access-policy map.
/// `CreateAccessPolicy` is the only thing that adds an access policy and it
/// declares the modelled `LimitExceededException` (HTTP 410), so that is how this
/// store refuses past the cap. The emulator's own bound, not a published AWS
/// default.
pub const MAX_ACCESS_POLICIES: usize = 1_000;

/// See [`MAX_ASSET_MODELS`]: the same bound, for the portal map. `CreatePortal`
/// is the only thing that adds a portal and it declares the modelled
/// `LimitExceededException` (HTTP 410), so that is how this store refuses past the
/// cap. The emulator's own bound, not a published AWS default.
pub const MAX_PORTALS: usize = 1_000;

/// See [`MAX_ASSET_MODELS`]: the same bound, for the project map. `CreateProject`
/// is the only thing that adds a project and it declares the modelled
/// `LimitExceededException` (HTTP 410), so that is how this store refuses past the
/// cap. The emulator's own bound, not a published AWS default.
pub const MAX_PROJECTS: usize = 1_000;

/// How many assets one IoT SiteWise Monitor project holds.
///
/// The same kind of bound, and for the same reason, as
/// [`MAX_CHILDREN_PER_HIERARCHY`]: `BatchAssociateProjectAssets` is the only thing
/// that grows a project's asset membership, so the set needs a maximum size, and
/// that operation declares the modelled `LimitExceededException` (HTTP 410) — "this
/// can occur if you're trying to associate more than the allowed number of child
/// assets" — which is how this store refuses past the cap. The emulator's own
/// bound, not a published AWS default, and high enough that no ordinary workflow
/// meets it.
pub const MAX_ASSETS_PER_PROJECT: usize = 1_000;

/// The `AuthMode` value of a portal that authenticates its users through IAM
/// Identity Center, as `AuthMode` spells it (smithy_query query="shape"
/// shape="AuthMode": `IAM` | `SSO`).
///
/// The IAM Identity Center identifiers a portal reports — `ssoApplicationId` and
/// `portalClientId` — exist only for a portal in this mode: both members are
/// documented as present only "if the portal uses IAM Identity Center".
pub const PORTAL_AUTH_MODE_SSO: &str = "SSO";

/// The `portalAuthMode` a `CreatePortal` call that named none gets, as the
/// member's own documentation states: "Default: SSO" — so the default and the
/// identity-center test are one value and cannot drift apart.
pub const DEFAULT_PORTAL_AUTH_MODE: &str = PORTAL_AUTH_MODE_SSO;

/// See [`MAX_ASSET_MODELS`]: the same bound, for the dataset map. `CreateDataset`
/// is the only thing that adds a dataset and it declares the modelled
/// `LimitExceededException` (HTTP 410), so that is how this store refuses past the
/// cap. The emulator's own bound, not a published AWS default.
pub const MAX_DATASETS: usize = 1_000;

/// The `datasetVersion` a freshly created dataset reports.
///
/// A dataset's version counts the writes it has accepted — the create is the
/// first — so it is derived from the stored record rather than minted, and a
/// read never has to write for it to stay the same. The modelled `Version` type
/// is a decimal string (`^(0|([1-9]{1}\d*))$`), which is how the counter reaches
/// the wire.
pub const DATASET_INITIAL_VERSION: u32 = 1;

/// How many data segments ONE dataset holds.
///
/// A segment is added by a producer entry point and nothing evicts one, so the
/// writer refuses past this cap rather than dropping a segment a caller still
/// expects to find; together with [`MAX_DATASETS`] it bounds the whole registry.
/// The emulator's own bound, not a published AWS default.
pub const MAX_DATA_SEGMENTS_PER_DATASET: usize = 1_000;

/// See [`MAX_ASSET_MODELS`]: the same bound, for the gateway map. `CreateGateway`
/// is the only thing that adds a gateway and it declares the modelled
/// `LimitExceededException` (HTTP 410), so that is how this store refuses past the
/// cap. The emulator's own bound, not a published AWS default.
pub const MAX_GATEWAYS: usize = 1_000;

/// The `gatewayVersion` a `CreateGateway` call that named none gets, as the member's
/// own documentation states: "If not specified, the default is 2 (Classic streams, V2
/// gateway)".
pub const DEFAULT_GATEWAY_VERSION: &str = "2";

/// The `gatewayVersion` whose GreengrassV2 platform must also carry
/// `coreDeviceOperatingSystem`, as `GatewayPlatform`'s documentation states: "If
/// creating a V3 gateway (gatewayVersion=3), you must also specify the
/// coreDeviceOperatingSystem".
pub const GATEWAY_VERSION_V3: &str = "3";

/// The `CapabilitySyncStatus` a capability reports once the gateway has processed
/// the configuration the last write to it supplied, as the member documents it:
/// "IN_SYNC - The gateway is running with the latest configuration".
pub const CAPABILITY_SYNC_IN_SYNC: &str = "IN_SYNC";

/// The `CapabilitySyncStatus` a capability reports until then, as
/// `UpdateGatewayCapabilityConfiguration` documents it: "After updating a
/// capability configuration, the sync status becomes OUT_OF_SYNC until the
/// gateway processes the configuration".
pub const CAPABILITY_SYNC_OUT_OF_SYNC: &str = "OUT_OF_SYNC";

/// How many capability namespaces one gateway holds configurations for.
///
/// The same kind of bound, and for the same reason, as [`MAX_GATEWAYS`]: a write
/// naming a fresh namespace is the only thing that adds one and nothing evicts
/// one, so the list needs a maximum entry count. The emulator's own bound, not a
/// published AWS default, and high enough that no ordinary workflow meets it;
/// `UpdateGatewayCapabilityConfiguration` refuses past it with the modelled
/// `LimitExceededException` (HTTP 410).
pub const MAX_CAPABILITIES_PER_GATEWAY: usize = 100;

/// See [`MAX_ASSET_MODELS`]: the same bound, for the workspace map.
/// `CreateWorkspace` is the only thing that adds a workspace and it declares the
/// modelled `LimitExceededException` (HTTP 410), so that is how this store
/// refuses past the cap. The emulator's own bound, not a published AWS default.
pub const MAX_WORKSPACES: usize = 1_000;

/// See [`MAX_WORKSPACES`]: the same bound, for the pipeline-execution map.
/// `StartPipelineExecution` is the only thing that adds an execution and it
/// declares the modelled `LimitExceededException` (HTTP 410).
pub const MAX_PIPELINE_EXECUTIONS: usize = 1_000;

/// The `EncryptionType` that encrypts a workspace with the service's own key.
///
/// Source: `smithy_query(query="shape", shape="EncryptionType")` — the enum holds
/// exactly this value and [`ENCRYPTION_TYPE_KMS_BASED`].
pub const ENCRYPTION_TYPE_SITEWISE_DEFAULT: &str = "SITEWISE_DEFAULT_ENCRYPTION";

/// The `EncryptionType` that encrypts a workspace with a customer managed KMS
/// key; see [`ENCRYPTION_TYPE_SITEWISE_DEFAULT`].
pub const ENCRYPTION_TYPE_KMS_BASED: &str = "KMS_BASED_ENCRYPTION";

/// The `PipelineExecutionState` / `ComputeNodeExecutionState` value an execution
/// that has not begun reports. Both enums declare it
/// (`smithy_query(query="shape", shape="PipelineExecutionState,ComputeNodeExecutionState")`).
pub const EXECUTION_STATE_NOT_STARTED: &str = "NOT_STARTED";

/// The `PipelineExecutionState` an execution a cancel was recorded on reports
/// until that cancellation has taken effect: the value the enum carries for a
/// cancel in flight, which `CancelPipelineExecution` documents as the state such
/// an execution "transitions to" before it goes "asynchronously to CANCELLED".
pub const EXECUTION_STATE_CANCELLING: &str = "CANCELLING";

/// The `PipelineExecutionState` a cancelled execution reports once its
/// cancellation has taken effect. Terminal, and one of
/// [`EXECUTION_TERMINAL_STATES`].
pub const EXECUTION_STATE_CANCELLED: &str = "CANCELLED";

/// The `PipelineExecutionState` values an execution STAYS in once it reaches
/// one: the states in which the execution is over and nothing it does can move
/// it again.
///
/// Source: `smithy_query(query="shape", shape="PipelineExecutionState")` — the
/// enum holds `NOT_STARTED`, `RUNNING`, `SUCCEEDED`, `FAILED`, `CANCELLING` and
/// `CANCELLED`; the three below are the ones an execution does not leave, and
/// the other three are stages on the way to them. This is what
/// [`PipelineExecutionRecord::is_active`] reads, so "an execution that has not
/// finished" is one rule in one place.
pub const EXECUTION_TERMINAL_STATES: &[&str] = &["SUCCEEDED", "FAILED", "CANCELLED"];

/// The `executionPriority` an execution that named none runs at, as
/// `StartPipelineExecution`'s own member documents it: "Lower values indicate
/// higher priority. Defaults to 2 when not specified."
pub const DEFAULT_EXECUTION_PRIORITY: i32 = 2;

/// How many samples one `(assetId, propertyId)` series keeps.
///
/// Unlike the resource caps above, a time series DOES have an eviction policy
/// available to it: `GetAssetPropertyValue` reports the latest value, so the
/// OLDEST sample is the one that can go. Past this many samples
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::record_property_value`] drops the
/// oldest, so repeated writes cannot grow one series without limit. The emulator's
/// own bound; AWS publishes a retention period rather than a sample count.
pub const MAX_SAMPLES_PER_SERIES: usize = 1_000;

/// How many child assets one `(asset, hierarchy)` holds.
///
/// `AssociateAssets` is the only thing that adds one and it declares the modelled
/// `LimitExceededException` (HTTP 410) for exactly this case — "this can occur if
/// you're trying to associate more than the allowed number of child assets" — so
/// that is how this store refuses past the cap. The emulator's own bound, not a
/// published AWS default (AWS publishes the quota as adjustable), set high enough
/// that no ordinary hierarchy meets it.
pub const MAX_CHILDREN_PER_HIERARCHY: usize = 500;

/// How many DISASSOCIATED time series this store keeps.
///
/// Two operations put one here: `DisassociateTimeSeriesFromAssetProperty` keeps
/// the data stream it detaches (see [`DisassociatedTimeSeriesRecord`]), and
/// `BatchPutAssetPropertyValue` creates one for a `propertyAlias` no asset
/// property carries. The alias is the caller's to choose, so the collection needs
/// a bound of its own: the ingest path REFUSES past this many series with the
/// `LimitExceededException` entry error code (the operation declares it), and the
/// detach path — which declares no limit error to refuse with — drops the least
/// recently updated series instead. The emulator's own bound, not a published AWS
/// quota, and high enough that no ordinary workflow meets it.
pub const MAX_DISASSOCIATED_TIME_SERIES: usize = 1_000;

/// How many logging-option scopes this store keeps.
///
/// `PutLoggingOptions` is the only thing that adds one and the scope key is the
/// caller's own `workspaceName`, so the collection needs a bound of its own.
/// Unlike the resource caps above, this one has an eviction policy available to
/// it rather than a refusal: the operation declares no limit error, and a scope
/// dropped from the store simply reports [`DEFAULT_LOGGING_LEVEL`] again — the
/// answer an unconfigured scope already gets — so past this many scopes
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::set_logging_options`] drops the
/// least recently set WORKSPACE scope. The account-and-region-wide scope
/// ([`ACCOUNT_LOGGING_SCOPE`]) is exempt, so the unnamed scope is never dropped.
/// The emulator's own bound, not a published AWS quota, and high enough that no
/// ordinary workflow meets it.
pub const MAX_LOGGING_SCOPES: usize = 1_000;

/// How many capture sources this store keeps, across every workspace.
///
/// A capture source is created by the producer path that first writes video for
/// it ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::put_capture_segment`]), and
/// the only thing that removes one is a `DeleteTimeSeries` scoped to its
/// workspace
/// ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::forget_capture_source`]), so
/// the collection needs a bound of its own: past this many sources a write
/// naming a NEW one is refused rather than evicting a source a caller can still
/// read. The emulator's own bound, not a published AWS quota, and high enough
/// that no ordinary workspace meets it.
pub const MAX_CAPTURE_SOURCES: usize = 1_000;

/// How many video segments one capture source keeps.
///
/// Like a time series' samples ([`MAX_SAMPLES_PER_SERIES`]), a capture source's
/// video is an unbounded stream with an eviction policy available to it: the
/// OLDEST segment is the one that goes, so repeated ingestion cannot grow one
/// source without limit. The emulator's own bound; AWS publishes a retention
/// period rather than a segment count.
pub const MAX_CAPTURE_SEGMENTS_PER_SOURCE: usize = 1_000;

/// The shortest `TimeSeriesId` the model admits. Source:
/// `smithy_query(query="shape", shape="TimeSeriesId")` — length 36..73.
pub const MIN_TIME_SERIES_ID_LENGTH: usize = 36;

/// The longest `TimeSeriesId` the model admits; see
/// [`MIN_TIME_SERIES_ID_LENGTH`].
pub const MAX_TIME_SERIES_ID_LENGTH: usize = 73;

/// The shortest `AssetPropertyAlias` the model admits. Source:
/// `smithy_query(query="shape", shape="AssetPropertyAlias")` — length 1..2048.
pub const MIN_PROPERTY_ALIAS_LENGTH: usize = 1;

/// The longest `AssetPropertyAlias` the model admits; see
/// [`MIN_PROPERTY_ALIAS_LENGTH`].
pub const MAX_PROPERTY_ALIAS_LENGTH: usize = 2048;

/// The earliest `timeInSeconds` a `TimeInNanos` can carry. Source:
/// `smithy_query(query="shape", shape="TimeInNanos")` — `timeInSeconds` is
/// `range 1..9223372036854774`.
pub const MIN_TIME_IN_SECONDS: i64 = 1;

/// The latest `timeInSeconds` a `TimeInNanos` can carry; see
/// [`MIN_TIME_IN_SECONDS`].
pub const MAX_TIME_IN_SECONDS: i64 = 9_223_372_036_854_774;

/// The largest `offsetInNanos` a `TimeInNanos` can carry: the member is
/// `range 0..999999999`, one nanosecond short of a whole second.
pub const MAX_OFFSET_IN_NANOS: i32 = 999_999_999;

/// Whether `at` is a `TimeInNanos` the model can express — see
/// [`MIN_TIME_IN_SECONDS`] and [`MAX_OFFSET_IN_NANOS`].
///
/// What the producer entry point checks before it stores an instant, so no span
/// this store holds can be reported through a member its own model would refuse.
fn is_modelled_time(at: &TimeInNanos) -> bool {
    (MIN_TIME_IN_SECONDS..=MAX_TIME_IN_SECONDS).contains(&at.time_in_seconds)
        && (0..=MAX_OFFSET_IN_NANOS).contains(&at.offset_in_nanos.unwrap_or(0))
}

/// The instant ONE nanosecond after `at`, or `None` when that is past what the
/// modelled `TimeInNanos` can express.
///
/// `TimeInNanos` is nanosecond-precision, so this is the smallest instant later
/// than `at`: what the HALF-OPEN end of a span that has to cover `at` is.
fn next_nanosecond(at: &TimeInNanos) -> Option<TimeInNanos> {
    let offset = at.offset_in_nanos.unwrap_or(0);
    let next = if offset >= MAX_OFFSET_IN_NANOS {
        TimeInNanos {
            time_in_seconds: at.time_in_seconds.checked_add(1)?,
            offset_in_nanos: Some(0),
        }
    } else {
        TimeInNanos {
            time_in_seconds: at.time_in_seconds,
            offset_in_nanos: Some(offset + 1),
        }
    };
    is_modelled_time(&next).then_some(next)
}

/// One scope's logging options and the instant they were set.
///
/// The instant is what the bound on
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::logging_options`] reads: past
/// [`MAX_LOGGING_SCOPES`] scopes the least recently set workspace scope is the
/// one dropped.
#[derive(Debug, Clone)]
pub struct LoggingOptionsRecord {
    /// The options the last `PutLoggingOptions` on this scope stored.
    pub options: LoggingOptions,
    /// When that put landed.
    pub set_at: SystemTime,
}

/// A time series (data stream) that no asset property owns.
///
/// IoT SiteWise models a time series as a resource in its own right, separate
/// from the asset property it may be attached to: `ListTimeSeries`'s
/// `timeSeriesType` is `ASSOCIATED` or `DISASSOCIATED`
/// (`smithy_query(query="shape", shape="ListTimeSeriesType")`), a
/// `TimeSeriesSummary` carries `assetId` and `propertyId` only when it is
/// associated, and `DeleteTimeSeries` — not the disassociate call — is what
/// forgets one ("If the time series isn't associated with an asset property,
/// specify the alias of the time series"). So detaching a series from its asset
/// property leaves THIS record behind: the stream keeps the identity, alias, data
/// type and creation date it had while associated, and its last-update instant
/// moves to the call that detached it. Alias-addressed ingestion creates one the
/// same way, with the data type of the value that created it.
#[derive(Debug, Clone)]
pub struct DisassociatedTimeSeriesRecord {
    /// The alias that identifies the stream, and the only way to address it once
    /// no `(assetId, propertyId)` pair names it.
    pub alias: String,
    /// The stream's own `TimeSeriesId`: a fresh `ID`-shaped value, inside the
    /// modelled `TimeSeriesId` bound of 36..73 characters.
    ///
    /// Minted rather than carried over from the pair that owned it, because the
    /// asset property the stream left still has a series of its own that its ids
    /// name ([`time_series_id`]): two series are two ids, so a detached stream
    /// cannot keep reporting the pair-derived one. Stored, so it is stable across
    /// every read of this stream.
    pub time_series_id: String,
    /// A `PropertyDataType` value: the data type of the stream's samples.
    pub data_type: String,
    /// Set only for a `STRUCT` stream, as `TimeSeriesSummary$dataTypeSpec` states.
    pub data_type_spec: Option<String>,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    /// The stream's samples, kept sorted ascending by timestamp exactly as the
    /// property's own series is. A time series IS the data stream, so the values
    /// written while it was associated travel with it rather than staying on the
    /// asset property, and come back to a property the alias is bound to again.
    pub values: Vec<AssetPropertyValue>,
}

/// Why [`ComAmazonawsIotsitewiseAwsiotsitewiseState::put_capture_segment`]
/// refused to store a segment. Each variant is ONE rule, so a producer handler
/// maps a refusal to one modelled error rather than re-deriving the checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureWriteRefusal {
    /// The store holds no workspace of that name — a capture source lives in a
    /// workspace.
    NoSuchWorkspace,
    /// The `TimeSeriesId` is outside [`MIN_TIME_SERIES_ID_LENGTH`]..=[`MAX_TIME_SERIES_ID_LENGTH`].
    TimeSeriesIdLength,
    /// The alias is outside [`MIN_PROPERTY_ALIAS_LENGTH`]..=[`MAX_PROPERTY_ALIAS_LENGTH`].
    AliasLength,
    /// The data type is not a value the modelled `VideoDataType` declares.
    UnmodelledDataType,
    /// The source already holds media of ANOTHER `VideoDataType`. The modelled
    /// enum declares exactly one value today, so this is what keeps the rule
    /// true if it ever grows.
    DataTypeMismatch,
    /// A bound of the span is not an instant the modelled `TimeInNanos` can
    /// express; see [`MIN_TIME_IN_SECONDS`].
    UnmodelledInstant,
    /// The span covers no time: its start is not before its end.
    EmptySpan,
    /// The source does not exist yet and the store already holds
    /// [`MAX_CAPTURE_SOURCES`].
    SourceLimit,
}

/// One persisted span of a capture source's video: the bytes a producer ingested
/// and the span of time they cover.
///
/// A segment is ATOMIC — this emulator stores the media exactly as it was
/// ingested and decodes nothing, so a read that overlaps part of a segment is
/// answered with the whole segment and reports the span the segment really
/// covers, which is what `GetCaptureData`'s own output members are for ("The
/// actual start time of the returned data").
#[derive(Debug, Clone)]
pub struct CaptureSegment {
    /// When the video in [`Self::data`] begins.
    pub start_time: TimeInNanos,
    /// When it ends. Always later than [`Self::start_time`]: the writer refuses a
    /// segment covering no time at all.
    pub end_time: TimeInNanos,
    /// The media bytes, exactly as the producer ingested them.
    pub data: Vec<u8>,
}

/// A capture source inside a workspace: the video stream `GetCaptureData` reads.
///
/// A capture source is a time series of a workspace — `GetCaptureData` names one
/// by its `timeSeriesId` or by the `propertyAlias` bound to it, the same two
/// identifying forms every other time-series operation of this service accepts —
/// so the record carries both identities plus the `VideoDataType` of the media it
/// holds, which a read reports rather than assuming.
///
/// The PRODUCER side (video an edge gateway publishes for the source, and the
/// bulk import of `mp4` files a `CreateBulkImportJob` asks for) is not
/// implemented in this emulator yet, so today the only writer of this log is
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::put_capture_segment`] — the one
/// entry point a producer path (or a test seeding a capture source) calls.
/// `GetCaptureData` itself never appends a segment and never creates a source.
///
/// A capture source IS one of its workspace's time series, so it is what the
/// workspace-scoped forms of `ListTimeSeries`, `DescribeTimeSeries` and
/// `DeleteTimeSeries` enumerate, report and remove
/// ([`Self::time_series_summary`],
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::workspace_time_series`]).
///
/// WHY this is its own registry rather than a field on the records
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::property_alias_index`] and
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::disassociated_time_series`]
/// resolve: a capture source is addressed INSIDE a workspace, and those streams
/// belong to no workspace at all — nothing that creates or ingests one records
/// one for it. Keying a source by `(workspaceName, timeSeriesId)` is what makes
/// one identifier free in every other workspace, and it stays correct when the
/// operations that ingest a workspace's streams arrive: a stream that gains a
/// workspace gains a capture source under that key, rather than a read having to
/// guess which workspace an account-wide stream belongs to.
#[derive(Debug, Clone)]
pub struct CaptureSourceRecord {
    /// The workspace that contains the source. A source is addressable only
    /// inside it, which is what makes one identifier free in every other
    /// workspace.
    pub workspace_name: String,
    /// The source's `TimeSeriesId`.
    pub time_series_id: String,
    /// The `propertyAlias` bound to the source, when one is.
    pub alias: Option<String>,
    /// A `VideoDataType` value: the type of the media the segments hold.
    pub data_type: String,
    /// When the write that created the source landed — the instant this time
    /// series came into being, which `timeSeriesCreationDate` reports.
    pub creation_date: SystemTime,
    /// When the last write this source accepted landed, which
    /// `timeSeriesLastUpdateDate` reports.
    pub last_update_date: SystemTime,
    /// The source's video, kept sorted ascending by start time and
    /// single-valued at a start time, exactly as a time series' samples are.
    pub segments: Vec<CaptureSegment>,
}

impl CaptureSourceRecord {
    /// This source as the time series it is: the one summary every time-series
    /// operation of this service reports a workspace's stream through, so a
    /// listing and a describe of one stream cannot disagree about it.
    ///
    /// No asset property owns a workspace's video stream — nothing binds one to
    /// an `(assetId, propertyId)` pair — so the summary carries neither id,
    /// which is what makes it `DISASSOCIATED` in a listing. Its
    /// `PropertyDataType` is [`DATA_TYPE_VIDEO`]: the media's own
    /// `VideoDataType` is finer-grained than that member can express and is
    /// what `GetCaptureData` reports instead. `dataTypeSpec` stays absent —
    /// `TimeSeriesSummary$dataTypeSpec` is "required for time series that have
    /// the `STRUCT` data type".
    pub fn time_series_summary(&self, region: &str) -> TimeSeriesSummary {
        TimeSeriesSummary {
            asset_id: None,
            property_id: None,
            alias: self.alias.clone(),
            data_type: DATA_TYPE_VIDEO.to_string(),
            data_type_spec: None,
            time_series_creation_date: to_date_time(self.creation_date),
            time_series_last_update_date: to_date_time(self.last_update_date),
            time_series_arn: time_series_arn(region, &self.time_series_id),
            time_series_id: self.time_series_id.clone(),
        }
    }
}

/// Which identifying form a time-series lookup took.
///
/// `DescribeTimeSeries` and `DeleteTimeSeries` accept the same two —
/// "specify the alias of the time series" or "the assetId and propertyId that
/// identifies the asset property" — whatever scope they name, and a lookup
/// scoped to a workspace resolves both through
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::workspace_series`], so neither
/// handler re-derives it.
#[derive(Debug, Clone, Copy)]
pub enum SeriesLookup<'a> {
    /// The series the `propertyAlias` bound to it names.
    Alias(&'a str),
    /// The series of the asset property the pair of `CustomID` labels names.
    Pair(&'a str, &'a str),
}

/// Why [`ComAmazonawsIotsitewiseAwsiotsitewiseState::workspace_series`]
/// resolved no time series, carrying the identifier the request supplied for the
/// resource that is missing. Each variant is ONE absence, so a handler maps it
/// to one modelled error and one message
/// ([`workspace_series_miss_message`]) rather than re-deriving the lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceSeriesMiss {
    /// The store holds no workspace of that name.
    NoSuchWorkspace,
    /// No stored asset answers to the `assetId` label the variant carries.
    NoSuchAsset(String),
    /// The asset the first label names carries no property the second names.
    NoSuchProperty(String, String),
    /// The workspace holds no time series under the identifier the request
    /// supplied, which the variant carries: the alias as sent, or the
    /// [`time_series_id`] of the pair the request named.
    NoSuchSeries(String),
}

/// How a workspace-scoped time-series lookup states the absence it stopped at:
/// the missing resource named as the kind it is, echoing the identifier the
/// REQUEST supplied for it, through the one template every not-found path of
/// this service uses ([`not_found_message`]).
///
/// Shared by every operation that takes a `workspaceName`, so two of them cannot
/// state one store state differently.
pub fn workspace_series_miss_message(workspace_name: &str, miss: &WorkspaceSeriesMiss) -> String {
    match miss {
        WorkspaceSeriesMiss::NoSuchWorkspace => {
            not_found_message(RESOURCE_WORKSPACE, workspace_name)
        }
        WorkspaceSeriesMiss::NoSuchAsset(asset_label) => asset_not_found_message(asset_label),
        WorkspaceSeriesMiss::NoSuchProperty(asset_label, property_label) => {
            format!("The asset {asset_label} has no property {property_label}.")
        }
        WorkspaceSeriesMiss::NoSuchSeries(requested) => {
            not_found_message(RESOURCE_TIME_SERIES, requested)
        }
    }
}

/// A stored asset model: everything `DescribeAssetModel` reports, plus the
/// bookkeeping (`version`, `tags`, `active_at`) the other operations read.
///
/// The members below carry the model's LATEST version, and
/// [`Self::superseded_active`] the definition of its ACTIVE one for as long as
/// those are two different definitions — which is exactly while a write the
/// model already accepted is still settling.
#[derive(Debug, Clone)]
pub struct AssetModelRecord {
    pub id: String,
    pub external_id: Option<String>,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// An `AssetModelType` value: `ASSET_MODEL`, `COMPONENT_MODEL` or `INTERFACE`.
    pub model_type: String,
    pub properties: Vec<AssetModelProperty>,
    pub hierarchies: Vec<AssetModelHierarchy>,
    pub composite_models: Vec<AssetModelCompositeModel>,
    /// `assetModelCompositeModelId` -> the links the modelled
    /// `AssetModelCompositeModel` shape has no member for: the composite model
    /// it is nested inside, and the component model it reuses. Absent for a
    /// top-level, inline composite model, which is what every composite model
    /// defined by `CreateAssetModel` is.
    pub composite_model_links: HashMap<String, CompositeModelLink>,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    /// The instant the transition the clock owes — from [`Self::settling_state`]
    /// to `ACTIVE` — falls due.
    pub active_at: SystemTime,
    /// The `AssetModelState` this model reports until [`Self::active_at`] falls
    /// due: [`STATE_CREATING`] for a freshly created model, [`STATE_UPDATING`]
    /// while a mutating call it already accepted settles.
    pub settling_state: String,
    pub version: u32,
    /// The definition of the model's ACTIVE version while the members above are
    /// NOT it: the one the write that is still settling superseded.
    ///
    /// Recorded by [`Self::begin_update`] and consulted only while the
    /// transition that write owes is still pending — from then on the stored
    /// definition IS the active one. `None` for a model no version of which has
    /// ever been active, which is one still being created.
    ///
    /// Exactly ONE definition, replaced rather than appended to, so no number
    /// of writes grows the record: a model carries its latest definition and at
    /// most its active one. Past the due instant the value kept here is stale
    /// and INERT — [`Self::version_view`] consults it only while the stored
    /// definition is not the active one — so nothing clears it on a read.
    pub superseded_active: Option<AssetModelDefinition>,
    pub tags: HashMap<String, String>,
}

/// One version of an asset model's definition: the members a write to the model
/// replaces, as of the write that replaced them.
///
/// An asset model is VERSIONED — the reads carry an `assetModelVersion`
/// selecting `LATEST` or `ACTIVE`, and the writes a `matchForVersionType`
/// targeting one of the same two — so an accepted write KEEPS the definition it
/// superseded for as long as that definition is still the active version,
/// rather than leaving the two selectors naming one definition.
#[derive(Debug, Clone)]
pub struct AssetModelDefinition {
    /// This version's number, as `assetModelVersion` reports it.
    pub version: u32,
    pub external_id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub properties: Vec<AssetModelProperty>,
    pub hierarchies: Vec<AssetModelHierarchy>,
    pub composite_models: Vec<AssetModelCompositeModel>,
    pub composite_model_links: HashMap<String, CompositeModelLink>,
    /// When the call that wrote this version landed: what
    /// `assetModelLastUpdateDate` reports for it.
    pub last_update_date: SystemTime,
}

/// What a composite model is attached to, beside the asset model that holds it.
#[derive(Debug, Clone, Default)]
pub struct CompositeModelLink {
    /// The `assetModelCompositeModelId` of the composite model this one is
    /// nested inside, when it is not top-level.
    pub parent_id: Option<String>,
    /// The `assetModelId` of the `COMPONENT_MODEL` this composite model reuses,
    /// for a component-model-based composite model. An inline one has none.
    pub composed_asset_model_id: Option<String>,
}

impl AssetModelRecord {
    /// The model's status as of `now`: its [`Self::settling_state`] until the
    /// owed transition falls due, `ACTIVE` from then on. Derived, so a read
    /// never has to write.
    pub fn status(&self, now: SystemTime) -> AssetModelStatus {
        AssetModelStatus {
            state: self.state(now).to_string(),
            error: None,
        }
    }

    /// The model's `AssetModelState` as of `now`.
    pub fn state(&self, now: SystemTime) -> &str {
        state_at(&self.settling_state, self.active_at, now)
    }

    /// The definition this record carries as it stands — its LATEST version.
    ///
    /// Read BEFORE a mutating call applies its writes, so the definition that
    /// call supersedes can be handed to [`Self::begin_update`].
    pub fn definition(&self) -> AssetModelDefinition {
        AssetModelDefinition {
            version: self.version,
            external_id: self.external_id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            properties: self.properties.clone(),
            hierarchies: self.hierarchies.clone(),
            composite_models: self.composite_models.clone(),
            composite_model_links: self.composite_model_links.clone(),
            last_update_date: self.last_update_date,
        }
    }

    /// This model as it read while it carried `definition`: the same resource —
    /// its id, ARN, type, creation date and tags — reporting that version's
    /// definition, number and last update date, in the `ACTIVE` state that
    /// version had reached.
    ///
    /// A superseded definition is only ever kept while it is the ACTIVE version
    /// (see [`Self::begin_update`]), so the view's own owed transition has long
    /// fallen due: its `active_at` is the instant the call that wrote it landed.
    ///
    /// Every member is listed, so only the members the version does not carry
    /// are cloned off `self` and the definition's own lists are cloned once.
    fn as_of(&self, definition: &AssetModelDefinition) -> Self {
        Self {
            version: definition.version,
            external_id: definition.external_id.clone(),
            name: definition.name.clone(),
            description: definition.description.clone(),
            properties: definition.properties.clone(),
            hierarchies: definition.hierarchies.clone(),
            composite_models: definition.composite_models.clone(),
            composite_model_links: definition.composite_model_links.clone(),
            last_update_date: definition.last_update_date,
            settling_state: STATE_ACTIVE.to_string(),
            active_at: definition.last_update_date,
            superseded_active: None,
            // The members a version of the definition does not carry: this is
            // the same resource, read as an earlier version of itself.
            id: self.id.clone(),
            arn: self.arn.clone(),
            model_type: self.model_type.clone(),
            creation_date: self.creation_date,
            tags: self.tags.clone(),
        }
    }

    /// This model as the version the selector `version_type` names it as of
    /// `now` (`LATEST | ACTIVE`, defaulting to `LATEST`), or `None` when the
    /// model has no such version.
    ///
    /// `LATEST` is the stored definition. `ACTIVE` is that same definition once
    /// the transition the clock owes has fallen due, and the definition the
    /// still-settling write superseded until then; a model no version of which
    /// has ever been active — one still being created — has no active version
    /// at all. ONE place, so every operation carrying the selector, whether as
    /// `assetModelVersion` on a read or as `matchForVersionType` on a write's
    /// precondition, resolves it identically.
    ///
    /// Anything that is not `ACTIVE` is the latest version: the modelled
    /// `^(LATEST|ACTIVE)$` pattern, which the runtime refuses a request against
    /// before a handler runs, leaves `LATEST` and an absent selector as the
    /// only other values that reach here.
    pub fn version_view(
        &self,
        version_type: Option<&str>,
        now: SystemTime,
    ) -> Option<Cow<'_, Self>> {
        match version_type.unwrap_or(VERSION_TYPE_LATEST) {
            VERSION_TYPE_ACTIVE if self.state(now) != STATE_ACTIVE => self
                .superseded_active
                .as_ref()
                .map(|definition| Cow::Owned(self.as_of(definition))),
            _ => Some(Cow::Borrowed(self)),
        }
    }

    /// Record that a mutating call — a composite model added to this model, say
    /// — landed at `now`, superseding the definition `superseded`: the
    /// definition now stored carries a new version, and the model reports
    /// `UPDATING` until the transition that call owes falls due.
    ///
    /// `superseded` is the definition as it stood BEFORE this call's writes,
    /// taken with [`Self::definition`]. It becomes what a read scoped to the
    /// ACTIVE version is answered from until the new definition settles, since
    /// the version this call wrote is not active yet. A call that lands while an
    /// earlier one is STILL settling supersedes that pending version, so the
    /// active version is left where it is — and a model that has never been
    /// active still has none.
    ///
    /// The due instant is STORED, never slept on, exactly as
    /// [`AssetRecord::begin_update`] does it for an asset. A model that had not
    /// finished being created keeps reporting `CREATING`: the pending transition
    /// it already owed is the one that settles, and it is the same one.
    pub fn begin_update(&mut self, superseded: AssetModelDefinition, now: SystemTime) {
        if self.state(now) == STATE_ACTIVE {
            self.superseded_active = Some(superseded);
        }
        self.last_update_date = now;
        self.version += 1;
        if self.state(now) != STATE_CREATING {
            self.settling_state = STATE_UPDATING.to_string();
        }
        self.active_at = now + ACTIVATION_DELAY;
    }

    /// The composite model stored under `id`, if this model holds one.
    pub fn composite_model(&self, id: &str) -> Option<&AssetModelCompositeModel> {
        self.composite_models
            .iter()
            .find(|composite| composite.id.as_deref() == Some(id))
    }

    /// Resolve a `CustomID` label — a raw id or `externalId:<externalId>` — to
    /// the `assetModelCompositeModelId` of a composite model of THIS model.
    pub fn resolve_composite_model_id(&self, label: &str) -> Option<String> {
        match label.strip_prefix(EXTERNAL_ID_PREFIX) {
            Some(external) => self
                .composite_models
                .iter()
                .find(|composite| composite.external_id.as_deref() == Some(external))
                .and_then(|composite| composite.id.clone()),
            None => self.composite_model(label).and_then(|composite| composite.id.clone()),
        }
    }

    /// The id of the composite model `id` is nested inside, if any.
    pub fn composite_model_parent(&self, id: &str) -> Option<&str> {
        self.composite_model_links
            .get(id)?
            .parent_id
            .as_deref()
    }

    /// The `COMPONENT_MODEL` asset model composite model `id` reuses, if it is
    /// component-model-based.
    pub fn composed_asset_model_id(&self, id: &str) -> Option<&str> {
        self.composite_model_links
            .get(id)?
            .composed_asset_model_id
            .as_deref()
    }

    /// The ids of the composite models nested DIRECTLY inside `id`.
    pub fn composite_model_children(&self, id: &str) -> Vec<String> {
        self.composite_models
            .iter()
            .filter_map(|composite| {
                let child = composite.id.as_deref()?;
                (self.composite_model_parent(child) == Some(id)).then(|| child.to_string())
            })
            .collect()
    }

    /// `id` and every composite model nested inside it, at any depth.
    ///
    /// Exactly the composite models that stop being reachable when `id` goes
    /// away: a nested composite model is addressed only through the chain of
    /// parents that contains it, so `DeleteAssetModelCompositeModel` removes the
    /// whole subtree rather than leaving records no path reaches. Each id is
    /// listed once, so a link that somehow pointed into a cycle cannot spin here.
    pub fn composite_model_subtree(&self, id: &str) -> Vec<String> {
        let mut subtree = vec![id.to_string()];
        let mut at = 0;
        while at < subtree.len() {
            for child in self.composite_model_children(&subtree[at]) {
                if !subtree.contains(&child) {
                    subtree.push(child);
                }
            }
            at += 1;
        }
        subtree
    }

    /// The ids of the composite models this model holds directly: the ones that
    /// are nested inside no other composite model.
    pub fn top_level_composite_model_ids(&self) -> Vec<String> {
        self.composite_models
            .iter()
            .filter_map(|composite| {
                let id = composite.id.as_deref()?;
                self.composite_model_parent(id)
                    .is_none()
                    .then(|| id.to_string())
            })
            .collect()
    }

    /// The `AssetModelCompositeModelPath` of composite model `id`: one segment
    /// per composite model from the outermost ancestor down to `id` itself, so
    /// the path of a top-level composite model is the composite model alone.
    pub fn composite_model_path(&self, id: &str) -> Vec<AssetModelCompositeModelPathSegment> {
        let mut path = Vec::new();
        let mut at = Some(id.to_string());
        // Bounded by the number of stored composite models, so a link that
        // somehow pointed into a cycle cannot spin here.
        while let Some(current) = at.take() {
            let Some(composite) = self.composite_model(&current) else {
                break;
            };
            path.push(AssetModelCompositeModelPathSegment {
                id: Some(current.clone()),
                name: Some(composite.name.clone()),
            });
            if path.len() >= self.composite_models.len() {
                break;
            }
            at = self
                .composite_model_parent(&current)
                .map(ToString::to_string);
        }
        path.reverse();
        path
    }

    /// Resolve a `CustomID` label — a raw id or `externalId:<externalId>` — to
    /// the id of a property THIS model declares.
    ///
    /// The property-list counterpart of [`Self::resolve_composite_model_id`], so
    /// every operation that addresses a model property by label resolves it the
    /// same way.
    pub fn resolve_property_id(&self, label: &str) -> Option<String> {
        self.properties
            .iter()
            .find(|property| {
                matches_id_label(
                    property.id.as_deref(),
                    property.external_id.as_deref(),
                    label,
                )
            })
            .and_then(|property| property.id.clone())
    }

    /// The id of the property this model declares under `name`, if any.
    ///
    /// A property's name is the only identity two asset models can share — ids
    /// are minted per model — so this is how an interface's property is paired
    /// with the property of the model the interface is applied to.
    pub fn property_id_named(&self, name: &str) -> Option<String> {
        self.properties
            .iter()
            .find(|property| property.name == name)
            .and_then(|property| property.id.clone())
    }

    /// The id of the hierarchy this model declares under `name`, if any. The
    /// hierarchy counterpart of [`Self::property_id_named`].
    pub fn hierarchy_id_named(&self, name: &str) -> Option<String> {
        self.hierarchies
            .iter()
            .find(|hierarchy| hierarchy.name == name)
            .and_then(|hierarchy| hierarchy.id.clone())
    }

    /// Composite model `id` as the summary both `DescribeAssetModel` and
    /// `DescribeAssetModelCompositeModel` report it as, so the two views of one
    /// stored composite model cannot disagree.
    pub fn composite_model_summary(&self, id: &str) -> Option<AssetModelCompositeModelSummary> {
        let composite = self.composite_model(id)?;
        Some(AssetModelCompositeModelSummary {
            id: id.to_string(),
            external_id: composite.external_id.clone(),
            name: composite.name.clone(),
            r#type: composite.r#type.clone(),
            description: composite.description.clone(),
            path: Some(self.composite_model_path(id)),
        })
    }

    /// The [`Self::composite_model_path`] of composite model `id`, as the
    /// ASSET-side path segment shape reports it.
    ///
    /// An asset composite model is an instance of a composite model of this
    /// model, so the chain of composite models it sits in is the same chain —
    /// only the shape the two views carry it in differs.
    pub fn asset_composite_model_path(&self, id: &str) -> Vec<AssetCompositeModelPathSegment> {
        self.composite_model_path(id)
            .into_iter()
            .map(|segment| AssetCompositeModelPathSegment {
                id: segment.id,
                name: segment.name,
            })
            .collect()
    }

    /// Composite model `id` as the ASSET-side summary an asset instantiated from
    /// this model reports it as, derived from [`Self::composite_model_summary`]
    /// so the model view and the asset view of one stored composite model cannot
    /// disagree. The asset-side summary states its description and path
    /// unconditionally, and carries the path in the asset-side segment shape,
    /// which is the only difference.
    pub fn asset_composite_model_summary(&self, id: &str) -> Option<AssetCompositeModelSummary> {
        let summary = self.composite_model_summary(id)?;
        Some(AssetCompositeModelSummary {
            id: summary.id,
            external_id: summary.external_id,
            name: summary.name,
            r#type: summary.r#type,
            description: summary.description.unwrap_or_default(),
            path: self.asset_composite_model_path(id),
        })
    }
}

/// A stored interface relationship: an interface asset model applied to an
/// asset model, and the mappings that application resolved to.
///
/// The mappings are RESOLVED ONCE, by the call that applies the interface, and
/// stored: they pair ids of two different models, which nothing else in the
/// store relates, so a later read reports the pairing the application made
/// rather than re-deriving one from definitions that may have moved since. What
/// a later write to either definition does take away is a mapping whose half
/// that model no longer declares — see
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::prune_interface_relationship_mappings`]
/// — because such a mapping names nothing.
#[derive(Debug, Clone)]
pub struct InterfaceRelationshipRecord {
    /// The stored `assetModelId` of the model the interface is applied to.
    pub asset_model_id: String,
    /// The stored `assetModelId` of the interface asset model.
    pub interface_asset_model_id: String,
    /// Each property of the interface asset model, paired with the property of
    /// [`Self::asset_model_id`] it maps to.
    pub property_mappings: Vec<PropertyMapping>,
    /// Each hierarchy of the interface asset model that the applied-to model
    /// also declares, paired with that model's own hierarchy.
    pub hierarchy_mappings: Vec<HierarchyMapping>,
}

/// A stored asset. `properties` is materialised from the referenced model's
/// property definitions at create time, so property writes and reads resolve.
/// It is ONE flat list holding the asset's own (base) properties and the ones
/// each composite model of that model contributes, the latter carrying the
/// `path` that addresses them from the root of the asset; see
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::materialise_asset_properties`].
#[derive(Debug, Clone)]
pub struct AssetRecord {
    pub id: String,
    pub external_id: Option<String>,
    pub arn: String,
    pub name: String,
    pub asset_model_id: String,
    pub description: Option<String>,
    pub properties: Vec<AssetProperty>,
    pub hierarchies: Vec<AssetHierarchy>,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    /// The instant the transition the clock owes — from [`Self::settling_state`]
    /// to `ACTIVE` — falls due.
    pub active_at: SystemTime,
    /// The `AssetState` this asset reports until [`Self::active_at`] falls due:
    /// [`STATE_CREATING`] for a freshly created asset, [`STATE_UPDATING`] while a
    /// mutating call it already accepted settles.
    pub settling_state: String,
    pub tags: HashMap<String, String>,
}

impl AssetRecord {
    /// Where in `properties` the property a `CustomID` label names sits — a raw
    /// property id, or `externalId:<externalId>` for one that carries an external
    /// id — if this asset holds it.
    pub fn property_position(&self, label: &str) -> Option<usize> {
        match label.strip_prefix(EXTERNAL_ID_PREFIX) {
            Some(external) => self
                .properties
                .iter()
                .position(|property| property.external_id.as_deref() == Some(external)),
            None => self.properties.iter().position(|property| property.id == label),
        }
    }

    /// EVERY place in `properties` the property a `CustomID` label names sits,
    /// in list order, empty when this asset holds no such property.
    ///
    /// A property id names ONE property of an asset — one definition, one time
    /// series ([`AssetRecord::time_series_properties`]) and one alias binding —
    /// but the flat list carries one INSTANCE of it per composite model that
    /// contributes it, because two composite models may reuse the same component
    /// model and each instance sits under its own path. A write to the property
    /// therefore lands on every instance of it, so no two views of one asset can
    /// report different aliases, notification states or units for one property
    /// id; a read answers from the first ([`AssetRecord::property`]).
    pub fn property_positions(&self, label: &str) -> Vec<usize> {
        let Some(first) = self.property_position(label) else {
            return Vec::new();
        };
        let id = self.properties[first].id.as_str();
        self.properties
            .iter()
            .enumerate()
            .filter(|(_, property)| property.id == id)
            .map(|(at, _)| at)
            .collect()
    }

    /// The asset property `label` names, resolved the way every `CustomID` member
    /// is — the same lookup as [`AssetRecord::property_position`], for a caller
    /// that wants the property rather than where it sits.
    ///
    /// `DescribeTimeSeries` models its `propertyId` as `CustomID`, so both forms
    /// reach a handler and both must name the same stored property.
    pub fn property(&self, label: &str) -> Option<&AssetProperty> {
        self.properties.get(self.property_position(label)?)
    }

    /// The asset's own (BASE) properties: the ones materialised from its model's
    /// top-level property definitions.
    ///
    /// What `DescribeAsset` reports in `assetProperties`, which "doesn't include
    /// properties that you define in composite models", and what
    /// `ListAssetProperties` enumerates under its default `BASE` filter.
    pub fn base_properties(&self) -> Vec<AssetProperty> {
        self.properties
            .iter()
            .filter(|property| composite_model_of_property(property).is_none())
            .cloned()
            .collect()
    }

    /// The properties this asset's instance of composite model `composite_id`
    /// carries, in the order they were materialised.
    pub fn composite_properties(&self, composite_id: &str) -> Vec<AssetProperty> {
        self.properties
            .iter()
            .filter(|property| composite_model_of_property(property) == Some(composite_id))
            .cloned()
            .collect()
    }

    /// This asset's properties, ONE per distinct property id: what addresses a
    /// time series of this asset.
    ///
    /// Two composite models of one asset model may reuse the SAME component
    /// model, so the asset carries that component's definition once per
    /// composite model that contributes it; the instances share the definition's
    /// id, hence the one series that id addresses. Every enumeration of an
    /// asset's time series goes through here, so a shared definition is not
    /// listed as two streams.
    pub fn time_series_properties(&self) -> Vec<&AssetProperty> {
        let mut seen: HashSet<&str> = HashSet::new();
        self.properties
            .iter()
            .filter(|property| seen.insert(property.id.as_str()))
            .collect()
    }

    /// The asset's status as of `now`; see [`AssetModelRecord::status`].
    pub fn status(&self, now: SystemTime) -> AssetStatus {
        AssetStatus {
            state: self.state(now).to_string(),
            error: None,
        }
    }

    /// The hierarchy of this asset a `CustomID` label names: a raw hierarchy id,
    /// or `externalId:<externalId>` resolved against the hierarchy's external id.
    ///
    /// The same two label forms every other id member of this service accepts, so
    /// a hierarchy is addressable the way the model says ids are.
    pub fn hierarchy(&self, label: &str) -> Option<&AssetHierarchy> {
        self.hierarchies.iter().find(|hierarchy| {
            matches_id_label(hierarchy.id.as_deref(), hierarchy.external_id.as_deref(), label)
        })
    }

    /// The asset's `AssetState` as of `now`: its [`Self::settling_state`] until
    /// the owed transition falls due, `ACTIVE` from then on. Derived, so a read
    /// never has to write.
    pub fn state(&self, now: SystemTime) -> &str {
        state_at(&self.settling_state, self.active_at, now)
    }

    /// Record that a mutating call landed at `now`: the asset reports `UPDATING`
    /// until the transition that call owes falls due, and `ACTIVE` after it.
    ///
    /// The due instant is STORED, never slept on, so the caller's own call
    /// returns immediately and the transient state is still what a read taken
    /// straight afterwards observes.
    pub fn begin_update(&mut self, now: SystemTime) {
        self.last_update_date = now;
        self.settling_state = STATE_UPDATING.to_string();
        self.active_at = now + ACTIVATION_DELAY;
    }

    /// This asset as a `ListAssets` summary, as of `now`.
    pub fn summary(&self, now: SystemTime) -> AssetSummary {
        AssetSummary {
            id: self.id.clone(),
            external_id: self.external_id.clone(),
            arn: self.arn.clone(),
            name: self.name.clone(),
            asset_model_id: self.asset_model_id.clone(),
            creation_date: to_date_time(self.creation_date),
            last_update_date: to_date_time(self.last_update_date),
            status: self.status(now),
            hierarchies: self.hierarchies.clone(),
            description: self.description.clone(),
        }
    }
}

/// Whether a stored `(id, externalId)` pair is the one a `CustomID` label names:
/// the raw id, or `externalId:` followed by the external id.
///
/// ONE definition of the two label forms, so every nested resource this service
/// addresses by label — a hierarchy on an asset, a hierarchy on its model — is
/// resolved the same way.
pub fn matches_id_label(id: Option<&str>, external_id: Option<&str>, label: &str) -> bool {
    match label.strip_prefix(EXTERNAL_ID_PREFIX) {
        Some(external) => external_id == Some(external),
        None => id == Some(label),
    }
}

/// A `(parent asset, hierarchy, child asset)` triple resolved to stored ids —
/// what the association operations address.
#[derive(Debug, Clone)]
pub struct HierarchyTriple {
    /// The parent asset's stored `assetId`.
    pub asset_id: String,
    /// The hierarchy's stored id, taken from the parent asset's hierarchy list.
    pub hierarchy_id: String,
    /// The child asset's stored `assetId`.
    pub child_asset_id: String,
}

/// How far a hierarchy association recorded by `AssociateAssets` has got: the
/// state a SECOND call naming the same triple is judged against.
///
/// The two are different answers to the caller, so they are different values here:
/// a still-settling association is a transient conflict between two operations on
/// one resource (the modelled `ConflictingOperationException`, HTTP 409, "retry"),
/// while a settled one is a terminal statement about the request (the modelled
/// `InvalidRequestException`, HTTP 400, "stop").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssociationState {
    /// The association was recorded and the transition that call owes has not
    /// fallen due yet.
    InProgress,
    /// The association is recorded and settled.
    Established,
}

/// Why an association triple does not name anything the store can act on.
///
/// Described structurally rather than as one error type, because the two
/// association operations declare their own modelled error sets and each maps
/// these onto its own variants; [`TripleRefusal::message`] is the shared wording.
#[derive(Debug, Clone)]
pub enum TripleRefusal {
    /// No stored asset answers to this label (the parent or the child).
    AssetNotFound(String),
    /// The parent asset carries no hierarchy this label names.
    HierarchyNotFound(String),
}

impl TripleRefusal {
    /// What the caller is told. Every variant here is an absent resource, so the
    /// wording is the one this service states that resource's absence in.
    pub fn message(&self) -> String {
        match self {
            Self::AssetNotFound(label) => asset_not_found_message(label),
            Self::HierarchyNotFound(label) => hierarchy_not_found_message(label),
        }
    }
}

/// `settling` before the owed transition falls due, `ACTIVE` once it has.
fn state_at(settling: &str, active_at: SystemTime, now: SystemTime) -> &str {
    if now < active_at {
        settling
    } else {
        STATE_ACTIVE
    }
}

/// A stored gateway: everything `DescribeGateway` reports, plus the `tags` the
/// tagging operations read.
///
/// `capabilities` starts EMPTY: a freshly created gateway has no capability
/// configured, and only `UpdateGatewayCapabilityConfiguration` adds one. The
/// summaries `DescribeGateway` and `ListGateways` report are DERIVED from that
/// list ([`Self::capability_summaries`]), so a summary and the configuration
/// `DescribeGatewayCapabilityConfiguration` answers with can never disagree about
/// a namespace or its sync status.
#[derive(Debug, Clone)]
pub struct GatewayRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub platform: GatewayPlatform,
    pub version: String,
    pub capabilities: Vec<GatewayCapabilityRecord>,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    pub tags: HashMap<String, String>,
}

impl GatewayRecord {
    /// The capability configured under `namespace`, or `None` when no write to
    /// this gateway has named that namespace.
    ///
    /// A gateway holds one configuration per namespace and may hold several
    /// namespaces at once, so the namespace alone selects one.
    pub fn capability(&self, namespace: &str) -> Option<&GatewayCapabilityRecord> {
        self.capabilities
            .iter()
            .find(|capability| capability.namespace == namespace)
    }

    /// Every configured capability as the summary a gateway reader reports, in the
    /// order the namespaces were first configured in.
    pub fn capability_summaries(&self, now: SystemTime) -> Vec<GatewayCapabilitySummary> {
        self.capabilities
            .iter()
            .map(|capability| GatewayCapabilitySummary {
                capability_namespace: capability.namespace.clone(),
                capability_sync_status: capability.sync_status(now).to_string(),
            })
            .collect()
    }

    /// Configure `namespace` on this gateway with `configuration`, replacing
    /// whatever a previous write put there, and report the sync status the write
    /// answers with — or, past [`MAX_CAPABILITIES_PER_GATEWAY`], the message the
    /// quota refusal carries.
    ///
    /// A write is the last word on the namespace it names and says nothing about
    /// any other, so an existing entry is replaced IN PLACE — keeping the order a
    /// reader reports — and a namespace nothing has configured is appended. The
    /// gateway's `lastUpdateDate` moves with it, because the capability summaries
    /// a gateway reader reports are part of what this write changed.
    ///
    /// The bound lives HERE, at the only thing that lengthens the list: the
    /// namespace is caller-supplied and nothing evicts an entry, so without a
    /// ceiling at the writer a stream of writes naming fresh namespaces would
    /// grow one emulator process without limit. A write REPLACING a namespace
    /// already configured lengthens nothing, so it is never refused for the cap.
    /// The `Err` carries the refusal message, which the calling operation lifts
    /// into its own modelled `LimitExceededException`.
    pub fn set_capability(
        &mut self,
        namespace: &str,
        configuration: String,
        now: SystemTime,
    ) -> Result<&'static str, String> {
        let existing = self
            .capabilities
            .iter_mut()
            .find(|capability| capability.namespace == namespace);
        let record = GatewayCapabilityRecord {
            namespace: namespace.to_string(),
            configuration,
            in_sync_at: now + ACTIVATION_DELAY,
        };
        match existing {
            Some(existing) => *existing = record,
            None => {
                if self.capabilities.len() >= MAX_CAPABILITIES_PER_GATEWAY {
                    return Err(capability_limit_message());
                }
                self.capabilities.push(record);
            }
        }
        self.last_update_date = now;
        Ok(CAPABILITY_SYNC_OUT_OF_SYNC)
    }
}

/// The message the per-gateway capability quota is refused with: the ceiling
/// [`MAX_CAPABILITIES_PER_GATEWAY`] states, named as the refusal's own limit.
pub fn capability_limit_message() -> String {
    format!(
        "This emulator holds at most {MAX_CAPABILITIES_PER_GATEWAY} capability configurations \
         per gateway."
    )
}

/// One capability configured on a gateway: the namespace that selects it, the
/// JSON document the last write to it supplied, and the instant the gateway has
/// processed that document.
///
/// The sync status is DERIVED from that instant rather than stored, so a read
/// never has to write for the status to move on.
#[derive(Debug, Clone)]
pub struct GatewayCapabilityRecord {
    pub namespace: String,
    pub configuration: String,
    /// When the capability stops being out of sync — see [`Self::sync_status`].
    pub in_sync_at: SystemTime,
}

impl GatewayCapabilityRecord {
    /// This capability's `CapabilitySyncStatus` at `now`.
    ///
    /// `UpdateGatewayCapabilityConfiguration` documents the rule: after a write
    /// "the sync status becomes OUT_OF_SYNC until the gateway processes the
    /// configuration", which is the owed transition this emulator settles at
    /// [`ACTIVATION_DELAY`] — so the status is `OUT_OF_SYNC` until then and
    /// `IN_SYNC` ("The gateway is running with the latest configuration")
    /// afterwards.
    pub fn sync_status(&self, now: SystemTime) -> &'static str {
        if now < self.in_sync_at {
            CAPABILITY_SYNC_OUT_OF_SYNC
        } else {
            CAPABILITY_SYNC_IN_SYNC
        }
    }
}

/// A stored dataset: everything `DescribeDataset` reports, plus the `tags` the
/// tagging operations read.
///
/// `settling_state` holds a `DatasetState` value, which spells its states exactly
/// as `AssetState` does — `CREATING`, `ACTIVE`, `UPDATING`, `DELETING` (plus
/// `FAILED`, which nothing here produces) — so this record reuses
/// [`STATE_CREATING`] / [`STATE_UPDATING`] / [`STATE_ACTIVE`] and the one
/// [`state_at`] rule rather than a second spelling of the same values.
#[derive(Debug, Clone)]
pub struct DatasetRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// A `DatasetTypeEnum` value — `SESSION`, `CURATED` or `EXTERNAL` — when the
    /// create named one. The member is optional on every shape that carries it,
    /// so a dataset created without one has none rather than a guessed type.
    pub dataset_type: Option<String>,
    pub config: Option<DatasetConfig>,
    pub metadata: HashMap<String, String>,
    pub source: DatasetSource,
    /// The workspace that contains the dataset, for a dataset created in one.
    pub workspace_name: Option<String>,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    /// The instant the transition the clock owes — from [`Self::settling_state`]
    /// to `ACTIVE` — falls due.
    pub active_at: SystemTime,
    /// The `DatasetState` this dataset reports until [`Self::active_at`] falls
    /// due: [`STATE_CREATING`] for a freshly created dataset, [`STATE_UPDATING`]
    /// while a mutating call it already accepted settles.
    pub settling_state: String,
    /// How many writes this dataset has accepted, which is the `datasetVersion`
    /// it reports; see [`DATASET_INITIAL_VERSION`].
    pub version: u32,
    pub tags: HashMap<String, String>,
}

impl DatasetRecord {
    /// The dataset's status as of `now`; see [`AssetModelRecord::status`].
    pub fn status(&self, now: SystemTime) -> DatasetStatus {
        DatasetStatus {
            state: self.state(now).to_string(),
            error: None,
        }
    }

    /// The dataset's `DatasetState` as of `now`: its [`Self::settling_state`]
    /// until the owed transition falls due, `ACTIVE` from then on. Derived, so a
    /// read never has to write.
    pub fn state(&self, now: SystemTime) -> &str {
        state_at(&self.settling_state, self.active_at, now)
    }

    /// The `datasetVersion` this dataset reports: its accepted-write count as the
    /// decimal string the modelled `Version` type is.
    pub fn version_label(&self) -> String {
        self.version.to_string()
    }

    /// Record that a mutating call landed at `now`: the dataset reports
    /// `UPDATING` until the transition that call owes falls due and `ACTIVE`
    /// after it, and the write it accepted takes the dataset to its next version.
    ///
    /// The due instant is STORED, never slept on, so the caller's own call
    /// returns immediately and the transient state is still what a read taken
    /// straight afterwards observes.
    pub fn begin_update(&mut self, now: SystemTime) {
        self.last_update_date = now;
        self.settling_state = STATE_UPDATING.to_string();
        self.active_at = now + ACTIVATION_DELAY;
        self.version = self.version.saturating_add(1);
    }
}

/// One data segment a dataset holds: a span of ONE time series' data.
///
/// A session dataset "contains data segments of time series data" and a curated
/// dataset "curates data segments selected from source session datasets" (the
/// `CreateDataset` model doc), so a segment names the dataset it came FROM
/// ([`Self::source_dataset_id`]) beside the time series and the span it covers.
/// The time series and the span are a segment's IDENTITY inside the dataset that
/// holds it: that is exactly what `BatchDeleteDatasetDataSegments` addresses one
/// by, so a dataset holds at most one segment for a given series and span.
///
/// The span is HALF-OPEN — `[start_timestamp, end_timestamp)` — the way a capture
/// source's [`CaptureSegment`] span is read, so two adjacent segments cover a
/// continuous stream and the instant between them belongs to the later one alone.
///
/// The one writer of a dataset's segments is
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::put_dataset_data_segment`] — the
/// entry point every producer path (`BatchAssociateDataSegmentsToDataset`, the
/// bulk import of time series data into a session dataset
/// ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::ingest_data_segments`]), or a
/// test seeding a dataset) calls — and the removers are
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::delete_dataset_data_segment`]
/// and
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::disassociate_dataset_data_segment`],
/// which differ only in whether the caller also deletes the data the segment
/// covered.
#[derive(Debug, Clone)]
pub struct DataSegmentRecord {
    /// The dataset the segment's data came from: the holding dataset itself for a
    /// session dataset's own segment, and the source session dataset for a
    /// segment a curated dataset selected.
    pub source_dataset_id: String,
    /// The `TimeSeriesId` of the time series whose data the segment covers.
    pub time_series_id: String,
    /// When the covered data begins.
    pub start_timestamp: TimeInNanos,
    /// When it ends. Always later than [`Self::start_timestamp`]: the writer
    /// refuses a segment covering no time at all.
    pub end_timestamp: TimeInNanos,
}

/// A workspace, by the name that identifies it in this account and region.
#[derive(Debug, Clone)]
pub struct WorkspaceRecord {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    /// The encryption scheme the create stated, kept verbatim so a read reports
    /// what was accepted rather than a default.
    pub encryption: WorkspaceEncryptionConfiguration,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    /// The instant the transition the clock owes — from [`Self::settling_state`]
    /// to `ACTIVE` — falls due.
    pub active_at: SystemTime,
    /// The `WorkspaceState` this workspace reports until [`Self::active_at`]
    /// falls due.
    pub settling_state: String,
    pub tags: HashMap<String, String>,
}

impl WorkspaceRecord {
    /// The workspace's `WorkspaceState` as of `now`: its [`Self::settling_state`]
    /// until the owed transition falls due, `ACTIVE` from then on. Derived, so a
    /// read never has to write.
    pub fn state(&self, now: SystemTime) -> &str {
        state_at(&self.settling_state, self.active_at, now)
    }

    /// The workspace's status as of `now`.
    pub fn status(&self, now: SystemTime) -> WorkspaceStatus {
        WorkspaceStatus {
            state: self.state(now).to_string(),
            error: None,
        }
    }

    /// Record that an accepted update landed at `now`: the members it leaves the
    /// workspace with replace what it held, and the workspace reports `UPDATING`
    /// until the transition this call owes falls due.
    ///
    /// Every member is passed in resolved, so the caller is the one place that
    /// decides what an omitted member keeps. The due instant is STORED, never
    /// slept on, so the caller's own call returns immediately and the transient
    /// state is still what a read taken straight afterwards observes.
    pub fn record_update(
        &mut self,
        description: Option<String>,
        encryption: WorkspaceEncryptionConfiguration,
        now: SystemTime,
    ) {
        self.description = description;
        self.encryption = encryption;
        self.last_update_date = now;
        self.settling_state = STATE_UPDATING.to_string();
        self.active_at = now + ACTIVATION_DELAY;
    }
}

/// One compute node of a recorded pipeline execution: which task it runs, the
/// nodes it waits for, and the environment and mounts resolved for it when the
/// execution was accepted.
///
/// Resolved ONCE, at the call that starts the execution, so a later write to the
/// pipeline or to a task cannot change what a past execution reports.
#[derive(Debug, Clone)]
pub struct ComputeNodeExecutionRecord {
    pub compute_node_name: String,
    pub task_name: String,
    /// The ARN the task was minted with, copied rather than rebuilt, so the
    /// execution keeps naming the task it actually resolved.
    pub task_arn: String,
    /// The task version the execution resolved.
    pub task_version: String,
    pub depends_on: Vec<String>,
    /// A `ComputeNodeExecutionState` value.
    pub state: String,
    /// The fully resolved environment variables for this node.
    pub environment_variables: HashMap<String, String>,
    /// The fully resolved mounts for this node.
    pub mounts: Vec<Mount>,
    pub start_time: Option<SystemTime>,
    pub end_time: Option<SystemTime>,
}

impl ComputeNodeExecutionRecord {
    /// This node's execution detail, as `DescribePipelineExecution` reports it.
    pub fn details(&self) -> ComputeNodeExecutionDetails {
        ComputeNodeExecutionDetails {
            compute_node_name: self.compute_node_name.clone(),
            task_name: self.task_name.clone(),
            task_arn: self.task_arn.clone(),
            task_version: self.task_version.clone(),
            depends_on: self.depends_on.clone(),
            status: ComputeNodeExecutionStatus {
                state: self.state.clone(),
                state_details: None,
            },
            start_time: self.start_time.map(to_date_time),
            end_time: self.end_time.map(to_date_time),
            execution_environment_variables: Some(self.environment_variables.clone()),
            execution_mounts: Some(self.mounts.clone()),
        }
    }
}

/// One pipeline execution, by the `pipelineExecutionId` that identifies it.
///
/// A compute node of an execution runs its task's container image, which this
/// emulator has no runtime for, so an accepted execution is RECORDED and no node
/// is reported started, succeeded or failed: a state, a duration or an error for
/// work that nothing ran would be invented rather than observed. What the record
/// does hold is everything the request and the stored pipeline determine — the
/// pipeline version it ran against, the DAG order, and the environment and mounts
/// resolved per node.
///
/// A cancellation IS something a call does to an execution, so it is recorded
/// here ([`Self::cancel_requested_at`]) and the state it takes the execution to
/// is derived from the clock ([`Self::state`]) rather than stored, the way every
/// other owed transition in this store is.
#[derive(Debug, Clone)]
pub struct PipelineExecutionRecord {
    pub id: String,
    pub workspace_name: String,
    pub pipeline_name: String,
    /// The `version` the pipeline carried when the execution was accepted.
    pub pipeline_version: String,
    /// The `PipelineExecutionState` the execution's own RUN has reached.
    /// Nothing runs here, so it stays [`EXECUTION_STATE_NOT_STARTED`]; a cancel
    /// is held separately in [`Self::cancel_requested_at`], so what
    /// [`Self::state`] reports is derived from both rather than overwritten by
    /// either.
    pub run_state: String,
    /// The priority the execution runs at: the one the request named, or
    /// [`DEFAULT_EXECUTION_PRIORITY`].
    pub execution_priority: i32,
    /// The environment variable overrides the request carried, verbatim.
    pub request_environment_variables: ExecutionEnvironmentVariables,
    /// The mount overrides the request carried, when it carried any.
    pub request_mount_overrides: Option<MountOverrides>,
    /// When the execution was accepted. Distinct from [`Self::start_time`], which
    /// is when the execution itself began.
    pub accepted_at: SystemTime,
    pub start_time: Option<SystemTime>,
    pub end_time: Option<SystemTime>,
    /// When a cancel was recorded on this execution, if one was. A cancel is
    /// recorded only on an execution whose run has not reached a terminal
    /// state, and from that instant the rest of the execution's lifecycle is
    /// owed by the clock rather than by any call.
    pub cancel_requested_at: Option<SystemTime>,
    /// The `reason` the recorded cancel carried, if it carried one: "A message
    /// describing why the pipeline execution is being cancelled", which
    /// `CancelPipelineExecution` documents "is returned in the stateDetails
    /// field when you describe the execution".
    pub cancel_reason: Option<String>,
    /// The compute nodes, in the DAG order
    /// [`compute_node_execution_order`] put them in.
    pub compute_nodes: Vec<ComputeNodeExecutionRecord>,
}

impl PipelineExecutionRecord {
    /// The execution's `PipelineExecutionState` at `now`.
    ///
    /// An execution a cancel was recorded on reports
    /// [`EXECUTION_STATE_CANCELLING`] until that cancellation takes effect and
    /// [`EXECUTION_STATE_CANCELLED`] from then on; otherwise it reports the
    /// state its own run has reached. DERIVED rather than stored, because the
    /// second half of the cancellation is owed by the clock and not by any
    /// call, so two readers of one execution cannot disagree about where it has
    /// got to and a read can never move it on.
    pub fn state(&self, now: SystemTime) -> &str {
        match self.cancel_requested_at {
            Some(_) if self.cancelled_at(now).is_some() => EXECUTION_STATE_CANCELLED,
            Some(_) => EXECUTION_STATE_CANCELLING,
            None => self.run_state.as_str(),
        }
    }

    /// The instant the recorded cancellation took effect, and `None` until it
    /// has — or when no cancel was recorded at all. The transition falls due
    /// [`ACTIVATION_DELAY`] after the cancel was recorded, the span this
    /// service owes every other transition it settles, and the instant is
    /// DERIVED from the recorded one rather than slept for.
    pub fn cancelled_at(&self, now: SystemTime) -> Option<SystemTime> {
        let due = self.cancel_requested_at? + ACTIVATION_DELAY;
        (now >= due).then_some(due)
    }

    /// The execution's status at `now`: the state [`Self::state`] derives, and
    /// beside it the `stateDetails` — "additional information about the current
    /// execution status. Populated when the execution has terminated", whose
    /// `message` for a cancelled execution "is the reason you supplied when
    /// calling CancelPipelineExecution". So the details appear with the terminal
    /// state the cancellation reaches, and only when the cancel carried a
    /// reason; the `code` and the per-step entries beside it are a FAILED
    /// execution's ("present when the execution failed"), which a cancelled one
    /// is not.
    pub fn status(&self, now: SystemTime) -> PipelineExecutionStatus {
        let state = self.state(now).to_string();
        let state_details = self
            .cancel_reason
            .clone()
            .filter(|_| EXECUTION_TERMINAL_STATES.contains(&state.as_str()))
            .map(|message| PipelineExecutionStateDetails {
                code: None,
                message,
                details: None,
            });
        PipelineExecutionStatus {
            state,
            state_details,
        }
    }

    /// When the execution ended, and `None` while it has not: the instant its
    /// run finished, or the instant a recorded cancellation took effect, since
    /// that is when a cancelled execution reached the state it stays in.
    pub fn ended_at(&self, now: SystemTime) -> Option<SystemTime> {
        self.end_time.or_else(|| self.cancelled_at(now))
    }

    /// Whether this execution is still ACTIVE at `now`: in a
    /// `PipelineExecutionState` that is not one of
    /// [`EXECUTION_TERMINAL_STATES`], so the execution has not finished. An
    /// execution waiting to begin counts as active — it has not completed
    /// either — and one whose cancellation is still in flight does too.
    pub fn is_active(&self, now: SystemTime) -> bool {
        !EXECUTION_TERMINAL_STATES.contains(&self.state(now))
    }

    /// Whether a cancel may still be RECORDED on this execution: one whose run
    /// has not reached a terminal state. An execution a cancel was already
    /// recorded on is not cancelled again — the repeat is a no-op reporting the
    /// state it stands in — and one whose run ended cannot be cancelled at all.
    pub fn is_cancelable(&self) -> bool {
        self.cancel_requested_at.is_none()
            && !EXECUTION_TERMINAL_STATES.contains(&self.run_state.as_str())
    }

    /// Record a cancellation on this execution at `now`, carrying the `reason`
    /// the request supplied. The due instant of the `CANCELLING` ->
    /// `CANCELLED` transition is STORED (as the instant it is owed from), never
    /// slept on, so the caller's own call returns immediately and the transient
    /// state is still what a read taken straight afterwards observes.
    pub fn record_cancel(&mut self, reason: Option<String>, now: SystemTime) {
        self.cancel_requested_at = Some(now);
        self.cancel_reason = reason;
    }
}

/// The canonical ARN of a workspace in `region`.
pub fn workspace_arn(region: &str, workspace_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{WORKSPACE_RESOURCE_TYPE}/{workspace_name}"),
    )
}

/// The order a pipeline's compute nodes run in: the DAG order their `dependsOn`
/// edges define, as the positions of [`PipelineVersion::computations`].
///
/// "Nodes without dependencies start immediately, while dependent nodes wait for
/// all upstream nodes to complete successfully" — so a node is ordered after
/// every node it depends on. Ties keep declaration order, which makes the order
/// one request produces stable across reads and resumable by a page token.
///
/// `Err` carries the refusal message for a `computations` list that is not a DAG
/// at all, from [`check_compute_nodes`] — the one validator every write to a
/// pipeline's compute nodes already uses, so one list is judged by one rule. The
/// caller lifts it into its own modelled refusal.
///
/// Linear in the nodes and their dependencies, like that validator: the lookups
/// are hashed and each edge is walked once (Kahn's algorithm over a ready set
/// ordered by declaration position), so no shape of list — a chain declared
/// backwards included — makes the walk cost more than the list itself. How long
/// the list itself can be is bounded by the model: `ComputeNodeList` is
/// `@length(min: 0, max: 50)` and `ComputeNodeNameList` `@length(min: 0, max:
/// 10)`, both refused at the boundary before a handler sees the request, so no
/// request can grow this walk.
pub fn compute_node_execution_order(computations: &[ComputeNode]) -> Result<Vec<usize>, String> {
    check_compute_nodes(computations)?;

    // How many dependencies each node is still waiting on, by position, and
    // which positions wait on each node.
    let mut waiting: Vec<usize> = Vec::with_capacity(computations.len());
    let mut dependents: HashMap<&str, Vec<usize>> = HashMap::with_capacity(computations.len());
    for (at, node) in computations.iter().enumerate() {
        let mut outstanding = 0usize;
        for dependency in node.depends_on.iter().flatten() {
            outstanding += 1;
            dependents.entry(dependency.as_str()).or_default().push(at);
        }
        waiting.push(outstanding);
    }

    // The ready set is ordered BY POSITION, so of the nodes that could run next
    // the one the write declared first is ordered first.
    let mut ready: BTreeSet<usize> = waiting
        .iter()
        .enumerate()
        .filter(|(_, outstanding)| **outstanding == 0)
        .map(|(at, _)| at)
        .collect();
    let mut order = Vec::with_capacity(computations.len());
    while let Some(at) = ready.pop_first() {
        order.push(at);
        let released = computations[at].compute_node_name.as_str();
        for dependent in dependents.get(released).into_iter().flatten().copied() {
            // Decremented through a guard rather than in place: the count is a
            // `usize`, and a list where two nodes answer to one name would
            // release the same dependent twice, so the guard is what keeps that
            // count from wrapping and the dependent from being ordered twice.
            // Such a list is already refused by `check_compute_nodes` above;
            // this holds whatever reaches here.
            let Some(outstanding) = waiting.get_mut(dependent) else {
                continue;
            };
            if *outstanding == 0 {
                continue;
            }
            *outstanding -= 1;
            if *outstanding == 0 {
                ready.insert(dependent);
            }
        }
    }
    // Every node is placed: a list that would leave one waiting holds a cycle,
    // which `check_compute_nodes` above already refused. Asserted rather than
    // assumed, so a regression in that validator shows up as a failing test
    // instead of an execution quietly missing compute nodes.
    debug_assert_eq!(
        order.len(),
        computations.len(),
        "the DAG walk placed {} of {} compute nodes",
        order.len(),
        computations.len()
    );
    Ok(order)
}

/// The environment variables one compute node of one execution runs with: every
/// layer that can define them, folded lowest priority first.
///
/// The hierarchy is the one the members themselves document: a task defines
/// "environment variables passed to the container at runtime"; a pipeline's are
/// "shared across all compute nodes in the pipeline. Individual compute nodes can
/// override these values with their own environment variables"; and an
/// execution's overrides "take the highest priority in the environment variable
/// hierarchy", with `global` applying "to all compute nodes in the pipeline
/// execution" and `computeNodes` carrying "per-compute-node environment variable
/// overrides". A later layer replaces an earlier layer's value for the same key
/// and keeps every key it does not name, so the caller passes the layers it has
/// in that order, lowest first.
pub fn resolved_environment_variables(
    layers: &[Option<&HashMap<String, String>>],
) -> HashMap<String, String> {
    let mut resolved = HashMap::new();
    for layer in layers.iter().flatten() {
        for (key, value) in *layer {
            resolved.insert(key.clone(), value.clone());
        }
    }
    resolved
}

/// The mounts one compute node of one execution runs with: its task-defined
/// mounts with the execution's overrides for that node merged in.
///
/// The rule is the one `executionMountOverrides` documents: "Overrides are merged
/// by mount name into each listed compute node's task-defined mounts: a matching
/// name replaces the task-defined mount, a new name adds a mount, and
/// task-defined mounts not referenced remain unchanged. Compute nodes not listed
/// use their task-defined mounts as-is."
pub fn resolved_mounts(task_mounts: &[Mount], overrides: Option<&[Mount]>) -> Vec<Mount> {
    let overrides = overrides.unwrap_or(&[]);
    let mut resolved: Vec<Mount> = task_mounts
        .iter()
        .map(|mount| {
            overrides
                .iter()
                .rev()
                .find(|replacement| replacement.name == mount.name)
                .unwrap_or(mount)
                .clone()
        })
        .collect();
    for mount in overrides {
        if !task_mounts
            .iter()
            .any(|defined| defined.name == mount.name)
            && !resolved.iter().any(|added| added.name == mount.name)
        {
            resolved.push(mount.clone());
        }
    }
    resolved
}

/// The platform device a gateway runs on, as a single comparable key: the
/// Greengrass V1 group ARN, the Greengrass V2 core device thing name or the Siemens
/// Industrial Edge IoT Core thing name, whichever the platform names.
///
/// `None` when the platform names no device at all — a request
/// [`GatewayPlatform`]'s documentation refuses, since one gateway specifies exactly
/// one platform type.
pub fn gateway_device_key(platform: &GatewayPlatform) -> Option<String> {
    gateway_device_dimension(platform).map(|(member, value)| format!("{member}:{value}"))
}

/// The same device, as the REQUEST MEMBER that names it and that member's value:
/// `greengrass.groupArn`, `greengrassV2.coreDeviceThingName` or
/// `siemensIE.iotCoreThingName`.
///
/// A refusal about a device has to name the dimension the caller actually supplied
/// — a thing name, a group ARN — rather than an identifier of the gateway it
/// collided with, so the member and its value are kept together here and
/// [`gateway_device_key`] is derived from them.
pub fn gateway_device_dimension(platform: &GatewayPlatform) -> Option<(&'static str, String)> {
    if let Some(greengrass) = platform.greengrass.as_ref() {
        return Some(("greengrass.groupArn", greengrass.group_arn.clone()));
    }
    if let Some(greengrass_v2) = platform.greengrass_v2.as_ref() {
        return Some((
            "greengrassV2.coreDeviceThingName",
            greengrass_v2.core_device_thing_name.clone(),
        ));
    }
    platform.siemens_ie.as_ref().map(|siemens| {
        (
            "siemensIE.iotCoreThingName",
            siemens.iot_core_thing_name.clone(),
        )
    })
}

/// How many platform types `platform` names. Exactly one is valid; see
/// [`gateway_device_key`].
pub fn gateway_platform_count(platform: &GatewayPlatform) -> usize {
    [
        platform.greengrass.is_some(),
        platform.greengrass_v2.is_some(),
        platform.siemens_ie.is_some(),
    ]
    .into_iter()
    .filter(|named| *named)
    .count()
}

/// A stored IoT SiteWise Monitor portal: everything `DescribePortal` reports,
/// plus the `tags` the tagging operations read.
///
/// The identity-center identifiers (`sso_application_id`, `client_id`) and the
/// logo image are minted ONCE, by the call that created or last replaced them,
/// and read back from here: nothing a reader reports about a portal is rebuilt
/// per request, so two reads of one portal answer the same.
#[derive(Debug, Clone)]
pub struct PortalRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub contact_email: String,
    /// An `AuthMode` value: `IAM` or `SSO`.
    pub auth_mode: String,
    pub notification_sender_email: Option<String>,
    pub alarms: Option<Alarms>,
    /// A `PortalType` value, when the create named one. AWS documents no default
    /// for it, so a portal created without one reports none.
    pub portal_type: Option<String>,
    pub portal_type_configuration: Option<HashMap<String, PortalTypeEntry>>,
    pub role_arn: Option<String>,
    /// The portal's own URL, as [`portal_start_url`] builds it.
    pub start_url: String,
    /// The IAM Identity Center application id, or EMPTY for a portal that
    /// authenticates through IAM: the member is documented as the application id
    /// "if the portal uses IAM Identity Center", and such a portal has no
    /// application to name.
    ///
    /// The empty value is a deliberate DEVIATION from the modelled
    /// `SSOApplicationId` length (1..64): the member is modelled required, so
    /// there is no "absent" to report, and naming an application that does not
    /// exist would be worse than reporting none.
    pub sso_application_id: String,
    /// The IAM Identity Center generated client id, empty for the same reason as
    /// [`Self::sso_application_id`] — including the same deviation from the
    /// modelled `PortalClientId` length (1..256).
    pub client_id: String,
    /// The id of the logo image the portal was given, if any. The reported
    /// location is derived from it by [`portal_logo_image_url`].
    pub logo_image_id: Option<String>,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    /// The instant the transition the clock owes — from [`Self::settling_state`]
    /// to `ACTIVE` — falls due.
    pub active_at: SystemTime,
    /// The `PortalState` this portal reports until [`Self::active_at`] falls due.
    pub settling_state: String,
    pub tags: HashMap<String, String>,
}

impl PortalRecord {
    /// The portal's status as of `now`: its [`Self::settling_state`] until the
    /// owed transition falls due, `ACTIVE` from then on. Derived, so a read never
    /// has to write.
    pub fn status(&self, now: SystemTime) -> PortalStatus {
        PortalStatus {
            state: self.state(now).to_string(),
            error: None,
        }
    }

    /// The portal's `PortalState` as of `now`.
    ///
    /// Through the same [`state_at`] rule every record in this store derives its
    /// state from: `PortalState`'s `CREATING` and `ACTIVE` are spelled exactly as
    /// [`STATE_CREATING`] and [`STATE_ACTIVE`].
    pub fn state(&self, now: SystemTime) -> &str {
        state_at(&self.settling_state, self.active_at, now)
    }

    /// Where the portal's logo image is reported to live, when it has one.
    pub fn logo_image_location(&self) -> Option<ImageLocation> {
        self.logo_image_id.as_ref().map(|image_id| ImageLocation {
            id: image_id.clone(),
            url: portal_logo_image_url(&self.id, image_id),
        })
    }
}

/// A stored IoT SiteWise Monitor project: everything `DescribeProject` reports,
/// plus the `tags` the tagging operations read.
///
/// A project has no lifecycle state of its own — no operation on one answers a
/// status member — so, unlike a portal, nothing here is owed a later transition;
/// the two dates are what a read derives from. The portal the project was created
/// in is stored on the record because every reader reports it and no operation
/// moves a project between portals.
#[derive(Debug, Clone)]
pub struct ProjectRecord {
    pub id: String,
    pub arn: String,
    /// The portal the project lives in, as the create named it.
    pub portal_id: String,
    pub name: String,
    pub description: Option<String>,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    pub tags: HashMap<String, String>,
    /// The assets associated with the project, which `ListProjectAssets` reports
    /// and `BatchAssociateProjectAssets` grows.
    ///
    /// A SET: an asset is either in the project or it is not, so associating one
    /// that is already there adds nothing and a request naming the same asset
    /// twice associates it once. Ordered by id — a `BTreeSet` rather than a
    /// `HashSet` — because the listing pages through this membership and a
    /// resumable `nextToken` needs a stable total order. Held on the project's own
    /// record, so a deleted project takes its membership with it and no second map
    /// can disagree about which assets a project holds; the other end of the same
    /// association is pruned by [`Self::forget_asset`].
    pub asset_ids: BTreeSet<String>,
}

impl ProjectRecord {
    /// Replace the caller-owned members with what an update stated, and record
    /// that instant as the project's last update.
    ///
    /// The request states the project's new name and description rather than
    /// patching selected members, so an omitted optional description clears the
    /// stored one; the project's identity and provenance — its id, its ARN, the
    /// portal it lives in, its tags and its creation date — are not that call's to
    /// change, and no member of its input carries them.
    pub fn apply_update(&mut self, name: String, description: Option<String>, now: SystemTime) {
        self.name = name;
        self.description = description;
        self.last_update_date = now;
    }
}

/// One IoT SiteWise Monitor access policy: which identity may reach which Monitor
/// resource, and with which permission.
///
/// An access policy has no lifecycle state of its own — no operation on one
/// answers a status member — so, unlike a portal or a dataset, nothing here is
/// owed a later transition; the two dates are what a read derives from.
#[derive(Debug, Clone)]
pub struct AccessPolicyRecord {
    pub id: String,
    pub arn: String,
    /// The identity the policy grants access to, exactly as the call that last
    /// wrote the policy stated it — see [`check_access_policy_identity`].
    pub identity: Identity,
    /// The Monitor resource the policy is about: a portal or a project.
    pub resource: Resource,
    /// A `Permission` value: `ADMINISTRATOR` or `VIEWER`.
    pub permission: String,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    pub tags: HashMap<String, String>,
}

impl AccessPolicyRecord {
    /// Replace the caller-owned members with what an update stated, and record
    /// that instant as the policy's last update.
    ///
    /// Every member `UpdateAccessPolicy` carries is required, so the request
    /// states the policy's new identity, resource and permission in full rather
    /// than patching selected members; the policy's identity and provenance — its
    /// id, its ARN, its tags and its creation date — are not that call's to
    /// change, and no member of its input carries them.
    pub fn apply_update(
        &mut self,
        identity: Identity,
        resource: Resource,
        permission: String,
        now: SystemTime,
    ) {
        self.identity = identity;
        self.resource = resource;
        self.permission = permission;
        self.last_update_date = now;
    }
}

/// The members of the `Identity` shape, in the order it declares them. Source:
/// `smithy_query(query="shape", shape="Identity")`.
pub const IDENTITY_MEMBERS: [&str; 4] = ["user", "group", "iamUser", "iamRole"];

/// The members of the `Resource` shape, in the order it declares them. Source:
/// `smithy_query(query="shape", shape="Resource")`.
pub const MONITOR_RESOURCE_MEMBERS: [&str; 2] = ["portal", "project"];

/// How many of [`IDENTITY_MEMBERS`] an `Identity` names.
fn identity_member_count(identity: &Identity) -> usize {
    usize::from(identity.user.is_some())
        + usize::from(identity.group.is_some())
        + usize::from(identity.iam_user.is_some())
        + usize::from(identity.iam_role.is_some())
}

/// Refuse an `Identity` that does not name EXACTLY ONE identity.
///
/// `Identity` is a choice spelled as a structure of optional members — "Contains
/// an identity that can access an IoT SiteWise Monitor resource", whose own
/// members are documented as the alternatives to "choose" between — so a request
/// naming none of them, or more than one, names no single identity for the policy
/// to be about. The `Err` carries the refusal message, which the calling
/// operation lifts into its own modelled client error.
pub fn check_access_policy_identity(identity: &Identity) -> Result<(), String> {
    let named = identity_member_count(identity);
    if named == 1 {
        return Ok(());
    }
    Err(format!(
        "accessPolicyIdentity must name exactly one of {}; this request named {named}.",
        catalog_names(IDENTITY_MEMBERS.into_iter())
    ))
}

/// Refuse a `Resource` that does not name EXACTLY ONE Monitor resource.
///
/// The same rule as [`check_access_policy_identity`], for the shape the policy's
/// resource is stated in: "Contains an IoT SiteWise Monitor resource ID for a
/// portal or project", one or the other.
pub fn check_access_policy_resource(resource: &Resource) -> Result<(), String> {
    let named = usize::from(resource.portal.is_some()) + usize::from(resource.project.is_some());
    if named == 1 {
        return Ok(());
    }
    Err(format!(
        "accessPolicyResource must name exactly one of {}; this request named {named}.",
        catalog_names(MONITOR_RESOURCE_MEMBERS.into_iter())
    ))
}

/// One version of a pipeline: the caller-owned configuration as of the call that
/// wrote it.
///
/// A pipeline is VERSIONED — `UpdatePipeline` answers with "The new version of
/// the pipeline created by this update", and `DescribePipeline`'s
/// `pipelineVersion` reads "The version number of the pipeline to retrieve. If
/// not specified, returns the latest version" — so an accepted write appends one
/// of these instead of overwriting the configuration in place, and a read naming
/// an older version still answers from the configuration that version carried.
#[derive(Debug, Clone)]
pub struct PipelineVersion {
    /// This version's number, as the modelled `Version` string spells it.
    pub version: u64,
    pub description: Option<String>,
    /// Kept as an `Option` so a pipeline that was never given environment
    /// variables reports none, rather than an empty map it never carried.
    pub environment_variables: Option<HashMap<String, String>>,
    pub computations: Vec<ComputeNode>,
    /// When the call that wrote this version landed — what `updatedAt` reports
    /// for it.
    pub recorded_at: SystemTime,
}

/// A stored pipeline: its identity within a workspace, the version a read answers
/// from, every version a later write superseded, and the `tags` the tagging
/// operations read.
#[derive(Debug, Clone)]
pub struct PipelineRecord {
    /// The workspace this pipeline lives in. A pipeline name is "unique within
    /// the workspace", so the pair is the pipeline's identity.
    pub workspace_name: String,
    pub name: String,
    pub arn: String,
    /// The newest version: what a read naming no version answers from.
    pub current: PipelineVersion,
    /// The versions a later write replaced, ascending by version number.
    pub superseded: Vec<PipelineVersion>,
    pub created_at: SystemTime,
    /// The instant the transition the clock owes — from [`Self::settling_state`]
    /// to `ACTIVE` — falls due.
    pub active_at: SystemTime,
    /// The `ResourceState` this pipeline reports until [`Self::active_at`] falls
    /// due: [`STATE_CREATING`] for a freshly created pipeline, [`STATE_UPDATING`]
    /// while a mutating call it already accepted settles.
    pub settling_state: String,
    pub tags: HashMap<String, String>,
}

impl PipelineRecord {
    /// The pipeline's `version` as the modelled `Version` string.
    pub fn version_label(&self) -> String {
        self.current.version.to_string()
    }

    /// The version a `Version` label names — the current one or any it
    /// superseded — or `None` when no write ever produced it.
    ///
    /// Compared NUMERICALLY, after parsing the label the modelled `Version`
    /// pattern (`^(0|([1-9]{1}\d*))$`, at most 10 digits) admits.
    pub fn version(&self, label: &str) -> Option<&PipelineVersion> {
        let wanted: u64 = label.parse().ok()?;
        if self.current.version == wanted {
            return Some(&self.current);
        }
        self.superseded
            .iter()
            .find(|version| version.version == wanted)
    }

    /// When the pipeline was last written: the instant its current version was
    /// recorded.
    pub fn updated_at(&self) -> SystemTime {
        self.current.recorded_at
    }

    /// The pipeline's `ResourceState` as of `now`: its [`Self::settling_state`]
    /// until the owed transition falls due, `ACTIVE` from then on. Derived, so a
    /// read never has to write.
    pub fn state(&self, now: SystemTime) -> &str {
        state_at(&self.settling_state, self.active_at, now)
    }

    /// The pipeline's status as of `now`. No write has failed, so it carries no
    /// error.
    pub fn status(&self, now: SystemTime) -> ResourceStatus {
        ResourceStatus {
            state: Some(self.state(now).to_string()),
            error: None,
        }
    }

    /// Record that an accepted update landed at `now`: the configuration it
    /// carries becomes a NEW version, the version it replaces is kept for a read
    /// that names it, and the pipeline reports `UPDATING` until the transition
    /// this call owes falls due.
    ///
    /// The due instant is STORED, never slept on, so the caller's own call
    /// returns immediately and the transient state is still what a read taken
    /// straight afterwards observes.
    pub fn record_update(
        &mut self,
        description: Option<String>,
        environment_variables: Option<HashMap<String, String>>,
        computations: Vec<ComputeNode>,
        now: SystemTime,
    ) {
        let next = PipelineVersion {
            version: self.current.version + 1,
            description,
            environment_variables,
            computations,
            recorded_at: now,
        };
        let previous = ::std::mem::replace(&mut self.current, next);
        self.superseded.push(previous);
        // The retained history is BOUNDED: past [`MAX_PIPELINE_VERSIONS`] the
        // oldest superseded version is evicted, the way a time series evicts its
        // oldest sample, so no number of updates grows one record without limit.
        let retained = self.superseded.len() + 1;
        if retained > MAX_PIPELINE_VERSIONS {
            self.superseded.drain(..retained - MAX_PIPELINE_VERSIONS);
        }
        self.settling_state = STATE_UPDATING.to_string();
        self.active_at = now + ACTIVATION_DELAY;
    }
}

/// The key a pipeline is stored under in
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::pipelines`]: the workspace it
/// lives in and its own name, since the name is unique only within the workspace.
pub fn pipeline_key(workspace_name: &str, pipeline_name: &str) -> (String, String) {
    (workspace_name.to_string(), pipeline_name.to_string())
}

/// The key an interface relationship is stored under in
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::interface_relationships`]: the
/// asset model the interface is applied to, then the interface asset model.
/// Both are STORED ids, not labels.
pub fn interface_relationship_key(
    asset_model_id: &str,
    interface_asset_model_id: &str,
) -> (String, String) {
    (
        asset_model_id.to_string(),
        interface_asset_model_id.to_string(),
    )
}

/// The ceiling on how many interface relationships this store holds.
///
/// The same reasoning as [`MAX_ASSET_MODELS`]: applying an interface is the only
/// thing that adds one and nothing evicts one, so the map needs a maximum entry
/// count. The emulator's own bound, not a published AWS quota; the operation
/// that would exceed it is refused with the modelled `LimitExceededException`.
pub const MAX_INTERFACE_RELATIONSHIPS: usize = 1_000;

/// The `AssetModelType` value an asset model must carry to be applied to another
/// asset model as an interface: the `AssetModelType` enum's third value, which
/// `CreateAssetModel` documents as "INTERFACE - An interface is a type of model
/// that defines a standard structure".
pub const MODEL_TYPE_INTERFACE: &str = "INTERFACE";

/// The canonical ARN of a pipeline in `region`.
///
/// The resource section carries the WORKSPACE as well as the pipeline name: the
/// name is unique only within its workspace, so two workspaces may each hold a
/// pipeline of one name and the ARN has to tell them apart.
pub fn pipeline_arn(region: &str, workspace_name: &str, pipeline_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{PIPELINE_RESOURCE_TYPE}/{workspace_name}/{pipeline_name}"),
    )
}

/// The `SearchStatus` a search reports before it has begun running — the status
/// `StartSearch` answers with, as the operation documents it ("the response
/// returns immediately with a searchId and an initial status of QUEUED").
pub const SEARCH_QUEUED: &str = "QUEUED";
/// The `SearchStatus` a search reports while it runs.
pub const SEARCH_RUNNING: &str = "RUNNING";
/// The terminal `SearchStatus` of a search whose run finished, and the one
/// `GetSearchResults` is documented to need ("retrieve the results once the
/// search reaches SUCCEEDED").
pub const SEARCH_SUCCEEDED: &str = "SUCCEEDED";

/// The `searchType` a `StartSearch` call that named none runs under, as the
/// `SearchType` shape documents it: "When `searchType` is omitted on a request,
/// the search defaults to QUICK".
pub const DEFAULT_SEARCH_TYPE: &str = "QUICK";

/// How long after the `StartSearch` call the queued-to-running transition it
/// owes falls due.
///
/// CHOSEN, not documented: AWS publishes no duration for a search's lifecycle,
/// only that the call answers immediately with `QUEUED` and that the search
/// runs "in the background". Deliberately non-zero, and towards the long end of
/// the span this emulator allows itself, so a caller that polls straight after
/// the call reliably observes the `QUEUED` the real service reports.
pub const SEARCH_QUEUED_FOR: Duration = Duration::from_millis(600);

/// How long a running search takes to reach [`SEARCH_SUCCEEDED`], counted from
/// the instant it STARTED RUNNING — the transition this one owes is the one the
/// run itself starts, not the one the call starts.
///
/// CHOSEN for the same reason as [`SEARCH_QUEUED_FOR`], and non-zero for the
/// same reason: `RUNNING` is a status `DescribeSearch` is documented to report,
/// so a search has to be observable in it for long enough to be polled.
pub const SEARCH_RUNS_FOR: Duration = Duration::from_millis(600);

/// How many searches one workspace holds.
///
/// The same kind of bound, and for the same reason, as
/// [`MAX_PIPELINES_PER_WORKSPACE`]: `StartSearch` is the only thing that adds a
/// search and nothing evicts one, so the map needs a maximum entry count. The
/// emulator's own bound, not a published AWS quota, and high enough that no
/// ordinary workflow meets it; `StartSearch` refuses past it with the modelled
/// `LimitExceededException` (HTTP 410).
pub const MAX_SEARCHES_PER_WORKSPACE: usize = 100;

/// How many searches the store holds in total, across every workspace.
///
/// The per-workspace cap cannot bound the store on its own: the store holds up
/// to [`MAX_WORKSPACES`] workspaces and each of them its own searches, so the
/// whole map needs an entry count of its own, refused past the cap with the
/// modelled `LimitExceededException` (HTTP 410).
pub const MAX_SEARCHES: usize = 1_000;

/// A search `StartSearch` started: the request it was started from and the
/// instants its lifecycle transitions fall due.
///
/// The status is DERIVED from those instants ([`Self::status`]) rather than
/// stored, so polling a search never has to write and two readers at the same
/// instant see the same status.
#[derive(Debug, Clone)]
pub struct SearchRecord {
    /// The workspace the search runs against. A search is addressed by
    /// `(workspaceName, searchId)`, so the pair is its identity.
    pub workspace_name: String,
    pub search_id: String,
    /// The `queryStatement` the call carried, which `DescribeSearch` reports
    /// back and a results read answers from.
    pub query_statement: String,
    /// The search strategy this search runs under: the one the call named, or
    /// [`DEFAULT_SEARCH_TYPE`] when it named none.
    pub search_type: String,
    /// The filters the call carried, kept as an `Option` so a search started
    /// without any reports none rather than empty lists it never carried.
    pub search_filters: Option<SearchFilters>,
    pub group_id: Option<String>,
    /// When the call that started the search landed — what `startedAt` reports.
    pub started_at: SystemTime,
    /// The instant the owed `QUEUED` -> `RUNNING` transition falls due.
    pub running_at: SystemTime,
    /// The instant the owed `RUNNING` -> `SUCCEEDED` transition falls due.
    pub succeeded_at: SystemTime,
}

impl SearchRecord {
    /// The search's `SearchStatus` as of `now`: queued until its run falls due,
    /// running until its run finishes, succeeded from then on. Derived, so a
    /// poll never has to write.
    ///
    /// No run of this emulator fails, so `FAILED` — and the `statusReason` the
    /// model documents as "Populated when the search has FAILED" — is a status
    /// nothing here reports.
    ///
    /// This status is a RECORD of the lifecycle the start owed, not a report on
    /// work something performed: the search's query is persisted but nothing
    /// evaluates it here. The results read
    /// (`crate::ops::get_search_results`) is where that query is answered, and it
    /// holds to the same rule: it DERIVES the matching segments from this store
    /// and this record's own query and filters, and never synthesises results to
    /// match the status reached here — until this status is
    /// [`SEARCH_SUCCEEDED`] it answers no results at all, the way the operation
    /// documents it.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.running_at {
            SEARCH_QUEUED
        } else if now < self.succeeded_at {
            SEARCH_RUNNING
        } else {
            SEARCH_SUCCEEDED
        }
    }
}

/// The key a search is stored under in
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::searches`]: the workspace it
/// runs against and its own id, so a search is reachable only through the
/// workspace it belongs to.
pub fn search_key(workspace_name: &str, search_id: &str) -> (String, String) {
    (workspace_name.to_string(), search_id.to_string())
}

/// The resource type section a search's ARN carries, which is what the modelled
/// conflict refusal reports for one.
pub const SEARCH_RESOURCE_TYPE: &str = "search";

/// The canonical ARN of a search in `region`.
///
/// The resource section carries the WORKSPACE as well as the id, for the reason
/// [`query_arn`] states: a search is addressed only inside the workspace it was
/// started in.
pub fn search_arn(region: &str, workspace_name: &str, search_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{SEARCH_RESOURCE_TYPE}/{workspace_name}/{search_id}"),
    )
}

/// One version of a task: the caller-owned configuration as of the call that
/// wrote it.
///
/// A task is VERSIONED — `UpdateTask` answers with "The new version of the task
/// created by this update", and `DescribeTask`'s `taskVersion` reads "The
/// version number of the task to retrieve. If not specified, returns the latest
/// version" — so an accepted write appends one of these instead of overwriting
/// the configuration in place, and a read naming an older version still answers
/// from the configuration that version carried.
#[derive(Debug, Clone)]
pub struct TaskVersion {
    /// This version's number, as the modelled `Version` string spells it.
    pub version: u64,
    pub description: Option<String>,
    /// The `TaskConfiguration` this version carries, stored exactly as the call
    /// that wrote it sent it.
    pub configuration: TaskConfiguration,
    /// When the call that wrote this version landed — what `updatedAt` reports
    /// for it.
    pub recorded_at: SystemTime,
}

/// A stored task: its identity within a workspace, the version a read answers
/// from, every version a later write superseded, and the `tags` the tagging
/// operations read.
#[derive(Debug, Clone)]
pub struct TaskRecord {
    /// The workspace this task lives in. A task name "must be unique within the
    /// workspace", so the pair is the task's identity.
    pub workspace_name: String,
    pub name: String,
    pub arn: String,
    /// The newest version: what a read naming no version answers from.
    pub current: TaskVersion,
    /// The versions a later write replaced, ascending by version number.
    pub superseded: Vec<TaskVersion>,
    pub created_at: SystemTime,
    /// The instant the transition the clock owes — from [`Self::settling_state`]
    /// to `ACTIVE` — falls due.
    pub active_at: SystemTime,
    /// The `ResourceState` this task reports until [`Self::active_at`] falls
    /// due: [`STATE_CREATING`] for a freshly created task, [`STATE_UPDATING`]
    /// while a mutating call it already accepted settles.
    pub settling_state: String,
    pub tags: HashMap<String, String>,
}

impl TaskRecord {
    /// The task's `version` as the modelled `Version` string.
    pub fn version_label(&self) -> String {
        self.current.version.to_string()
    }

    /// The version a `Version` label names — the current one or any it
    /// superseded — or `None` when no write ever produced it.
    ///
    /// Compared NUMERICALLY, after parsing the label the modelled `Version`
    /// pattern (`^(0|([1-9]{1}\d*))$`, at most 10 digits) admits.
    pub fn version(&self, label: &str) -> Option<&TaskVersion> {
        let wanted: u64 = label.parse().ok()?;
        if self.current.version == wanted {
            return Some(&self.current);
        }
        self.superseded
            .iter()
            .find(|version| version.version == wanted)
    }

    /// When the task was last written: the instant its current version was
    /// recorded.
    pub fn updated_at(&self) -> SystemTime {
        self.current.recorded_at
    }

    /// The task's `ResourceState` as of `now`: its [`Self::settling_state`]
    /// until the owed transition falls due, `ACTIVE` from then on. Derived, so a
    /// read never has to write.
    pub fn state(&self, now: SystemTime) -> &str {
        state_at(&self.settling_state, self.active_at, now)
    }

    /// The task's status as of `now`. No write has failed, so it carries no
    /// error.
    pub fn status(&self, now: SystemTime) -> ResourceStatus {
        ResourceStatus {
            state: Some(self.state(now).to_string()),
            error: None,
        }
    }

    /// The environment variables the task's current configuration itself
    /// defines — the lowest layer of the hierarchy
    /// [`resolved_environment_variables`] folds.
    pub fn environment_variables(&self) -> Option<&HashMap<String, String>> {
        match &self.current.configuration {
            TaskConfiguration::containerTaskConfiguration(container) => {
                container.environment_variables.as_ref()
            }
        }
    }

    /// The mounts the task's current configuration itself defines — what an
    /// execution's mount overrides are merged into; see [`resolved_mounts`].
    pub fn mounts(&self) -> &[Mount] {
        match &self.current.configuration {
            TaskConfiguration::containerTaskConfiguration(container) => {
                container.mounts.as_deref().unwrap_or(&[])
            }
        }
    }

    /// Record that an accepted update landed at `now`: the configuration it
    /// carries becomes a NEW version, the version it replaces is kept for a read
    /// that names it, and the task reports `UPDATING` until the transition this
    /// call owes falls due.
    ///
    /// The due instant is STORED, never slept on, so the caller's own call
    /// returns immediately and the transient state is still what a read taken
    /// straight afterwards observes.
    pub fn record_update(
        &mut self,
        description: Option<String>,
        configuration: TaskConfiguration,
        now: SystemTime,
    ) {
        let next = TaskVersion {
            version: self.current.version + 1,
            description,
            configuration,
            recorded_at: now,
        };
        let previous = ::std::mem::replace(&mut self.current, next);
        self.superseded.push(previous);
        // The retained history is BOUNDED: past [`MAX_TASK_VERSIONS`] the oldest
        // superseded version is evicted, the way a pipeline's history is, so no
        // number of updates grows one record without limit.
        let retained = self.superseded.len() + 1;
        if retained > MAX_TASK_VERSIONS {
            self.superseded.drain(..retained - MAX_TASK_VERSIONS);
        }
        self.settling_state = STATE_UPDATING.to_string();
        self.active_at = now + ACTIVATION_DELAY;
    }
}

/// The key a task is stored under in
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::tasks`]: the workspace it lives
/// in and its own name, since the name is unique only within the workspace.
pub fn task_key(workspace_name: &str, task_name: &str) -> (String, String) {
    (workspace_name.to_string(), task_name.to_string())
}

/// The canonical ARN of a task in `region`.
///
/// The resource section carries the WORKSPACE as well as the task name, for the
/// reason [`pipeline_arn`] states: the name is unique only within its workspace,
/// so two workspaces may each hold a task of one name and the ARN has to tell
/// them apart.
pub fn task_arn(region: &str, workspace_name: &str, task_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{TASK_RESOURCE_TYPE}/{workspace_name}/{task_name}"),
    )
}

/// The `TargetResourceType` value that addresses an asset. Source:
/// `smithy_query(query="shape", shape="TargetResourceType")` — ASSET |
/// COMPUTATION_MODEL.
pub const TARGET_RESOURCE_TYPE_ASSET: &str = "ASSET";

/// The `TargetResourceType` value that addresses a computation model. See
/// [`TARGET_RESOURCE_TYPE_ASSET`] for the source of the pair.
pub const TARGET_RESOURCE_TYPE_COMPUTATION_MODEL: &str = "COMPUTATION_MODEL";

/// The `ExecutionState` an execution reports while the action it runs is still in
/// flight.
///
/// Source: `smithy_query(query="shape", shape="ExecutionState")` — RUNNING |
/// COMPLETED | FAILED, of which this is the only non-terminal value.
pub const EXECUTION_STATE_RUNNING: &str = "RUNNING";

/// The `ExecutionState` of an execution whose action finished successfully. See
/// [`EXECUTION_STATE_RUNNING`] for the source of the set.
pub const EXECUTION_STATE_COMPLETED: &str = "COMPLETED";

/// The `ExecutionState` of an execution whose action finished unsuccessfully. See
/// [`EXECUTION_STATE_RUNNING`] for the source of the set.
pub const EXECUTION_STATE_FAILED: &str = "FAILED";

/// Every `ExecutionState` an execution can report, in the order the modelled enum
/// declares them.
///
/// Source: `smithy_query(query="shape", shape="ExecutionState")` — RUNNING |
/// COMPLETED | FAILED. This is the catalog the execution summary of a computation
/// model is keyed by, so a summary reports every state the model declares rather
/// than only those some execution happens to be in.
pub const EXECUTION_STATES: [&str; 3] = [
    EXECUTION_STATE_RUNNING,
    EXECUTION_STATE_COMPLETED,
    EXECUTION_STATE_FAILED,
];

/// How long after the call that starts it an execution of an action takes to
/// settle: until then it reports [`EXECUTION_STATE_RUNNING`], and its outcome
/// from then on.
///
/// CHOSEN, not documented: AWS publishes no duration for an action's run, only
/// that the call starting one is ACCEPTED (HTTP 202) and that an execution
/// reports `RUNNING` while "the process" has not ended. The span is deliberately
/// non-zero, so a caller that reads the execution straight after the call still
/// observes the running state the real service reports.
pub const ACTION_EXECUTION_DELAY: Duration = Duration::from_millis(500);

/// The ceiling on how many actions this store holds.
///
/// Not a published AWS quota: the store is in memory, an execute is the only
/// thing that adds an action and nothing evicts one, so this is where the
/// emulator stops growing — the quota refusal `ExecuteAction` models
/// (`LimitExceededException`, "You've reached the quota for a resource"). Each
/// action starts exactly one execution, so this bounds the executions too.
pub const MAX_ACTIONS: usize = 1_000;

/// The members of the `TargetResource` shape, in the order it declares them.
/// Source: `smithy_query(query="shape", shape="TargetResource")`.
pub const TARGET_RESOURCE_MEMBERS: [&str; 2] = ["assetId", "computationModelId"];

/// The `(TargetResourceType, ID)` pair `target_resource` names, or the refusal a
/// structure naming no single resource draws.
///
/// `TargetResource` is a choice spelled as a structure of optional members —
/// "The resource the action will be taken on. This can include asset-based
/// resources and computation model resources" — so a request naming neither of
/// them, or both, names no one resource to act on. The pair this returns is the
/// same pair the listings scoped to a target resource take, so a resource is
/// addressed identically whether the request states it as a structure or as a
/// type and an id. The `Err` carries the refusal message, which the calling
/// operation lifts into its own modelled client error.
pub fn target_resource_reference(
    target_resource: &TargetResource,
) -> Result<(&'static str, &str), String> {
    match (
        target_resource.asset_id.as_deref(),
        target_resource.computation_model_id.as_deref(),
    ) {
        (Some(asset_id), None) => Ok((TARGET_RESOURCE_TYPE_ASSET, asset_id)),
        (None, Some(computation_model_id)) => Ok((
            TARGET_RESOURCE_TYPE_COMPUTATION_MODEL,
            computation_model_id,
        )),
        (asset_id, computation_model_id) => {
            let named =
                usize::from(asset_id.is_some()) + usize::from(computation_model_id.is_some());
            Err(format!(
                "targetResource must name exactly one of {}; this request named {named}.",
                catalog_names(TARGET_RESOURCE_MEMBERS.into_iter())
            ))
        }
    }
}

/// The member a request states the TYPE of the resolved resource with. Source:
/// `smithy_query(query="operation", operation="ListActions")` —
/// `resolveToResourceType (ResolveToResourceType, one of: ASSET)`.
pub const RESOLVE_TO_RESOURCE_TYPE_MEMBER: &str = "resolveToResourceType";

/// The member a request states the ID of the resolved resource with. See
/// [`RESOLVE_TO_RESOURCE_TYPE_MEMBER`] for the source of the pair.
pub const RESOLVE_TO_RESOURCE_ID_MEMBER: &str = "resolveToResourceId";

/// Why a `(resolveToResourceType, resolveToResourceId)` pair names no one
/// resolved resource.
///
/// Described structurally rather than as one error type, for the reason
/// [`TripleRefusal`] gives: each operation taking the pair declares its own
/// modelled error set and maps these onto its own variants — a pair stated by
/// halves onto its invalid-request error, an absent resource onto its not-found
/// error — while [`ResolveToRefusal::message`] is the shared wording.
#[derive(Debug, Clone)]
pub enum ResolveToRefusal {
    /// Only one half of the pair was stated, so it addresses nothing: `stated`
    /// is the member the request gave, `missing` the one it left out.
    StatedByHalves {
        stated: &'static str,
        missing: &'static str,
    },
    /// The pair addresses an asset this store does not hold.
    AssetNotFound(String),
}

impl ResolveToRefusal {
    /// What the caller is told.
    pub fn message(&self) -> String {
        match self {
            Self::StatedByHalves { stated, missing } => {
                format!("{missing} is required when {stated} is specified.")
            }
            Self::AssetNotFound(asset_id) => asset_not_found_message(asset_id),
        }
    }
}

impl ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// The asset a `(resolveToResourceType, resolveToResourceId)` pair addresses,
    /// `None` when the request stated neither member, or the refusal the pair
    /// draws.
    ///
    /// The pair is a resource reference — a type and an id that TOGETHER name one
    /// resolved resource — so half of it addresses nothing and is refused as the
    /// bad request it is, and an id this store holds no asset for draws the same
    /// absence refusal every other addressed resource does.
    /// `ResolveToResourceType` models the single value `ASSET`, which the runtime
    /// checks before any handler runs, so the id names an asset.
    ///
    /// The single definition for every operation that narrows to a resolved
    /// resource — the two listings scoped to a target resource and a computation
    /// model's execution summary — so a resolved resource is addressed
    /// identically wherever a request names one, exactly as
    /// [`target_resource_reference`] does for the sibling reference.
    pub fn resolve_to_asset<'a>(
        &self,
        resolve_to_resource_type: Option<&str>,
        resolve_to_resource_id: Option<&'a str>,
    ) -> Result<Option<&'a str>, ResolveToRefusal> {
        match (resolve_to_resource_type, resolve_to_resource_id) {
            (Some(_), Some(asset_id)) => {
                if self.assets.contains_key(asset_id) {
                    Ok(Some(asset_id))
                } else {
                    Err(ResolveToRefusal::AssetNotFound(asset_id.to_string()))
                }
            }
            (None, None) => Ok(None),
            (Some(_), None) => Err(ResolveToRefusal::StatedByHalves {
                stated: RESOLVE_TO_RESOURCE_TYPE_MEMBER,
                missing: RESOLVE_TO_RESOURCE_ID_MEMBER,
            }),
            (None, Some(_)) => Err(ResolveToRefusal::StatedByHalves {
                stated: RESOLVE_TO_RESOURCE_ID_MEMBER,
                missing: RESOLVE_TO_RESOURCE_TYPE_MEMBER,
            }),
        }
    }
}

/// One execution of an action on a target resource: what `ListExecutions`
/// summarises, keyed by its own `executionId` in
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::executions`].
///
/// An execution runs for a span rather than finishing inside the call that starts
/// it, so the record keeps the instant its run is owed to end
/// ([`Self::settles_at`]) and the terminal state it settles into
/// ([`Self::outcome`]) instead of a state field a read would have to move:
/// [`Self::state`] reports [`EXECUTION_STATE_RUNNING`] until that instant falls
/// due and the outcome from then on, so a listing never has to write to report
/// what the clock already made true.
#[derive(Debug, Clone)]
pub struct ExecutionRecord {
    /// The id this execution is addressed by.
    pub execution_id: String,
    /// The `actionType` of the action definition this execution runs, which is
    /// also what the listing's `actionType` filter matches on.
    pub action_type: Option<String>,
    /// The resource the action was taken on: an asset or a computation model.
    pub target_resource: TargetResource,
    /// The version of the target resource the action ran against.
    pub target_resource_version: String,
    /// The detailed resource this execution resolves to, which only an execution
    /// whose target resolves through a data binding carries.
    pub resolve_to: Option<ResolveTo>,
    /// When the execution started.
    pub execution_start_time: SystemTime,
    /// The instant this execution's run is owed to end — when it stops reporting
    /// [`EXECUTION_STATE_RUNNING`] and reports [`Self::outcome`] instead.
    pub settles_at: SystemTime,
    /// The terminal `ExecutionState` this execution settles into:
    /// [`EXECUTION_STATE_COMPLETED`] or [`EXECUTION_STATE_FAILED`].
    pub outcome: String,
    /// The execution entity version associated with the summary.
    pub execution_entity_version: Option<String>,
}

impl ExecutionRecord {
    /// The `ExecutionState` this execution reports at `now`: still running until
    /// [`Self::settles_at`] falls due, its outcome from then on.
    pub fn state(&self, now: SystemTime) -> &str {
        if now < self.settles_at {
            EXECUTION_STATE_RUNNING
        } else {
            &self.outcome
        }
    }

    /// When the execution ended, which is reported only once it has: "The time
    /// the process ended" is absent for one still running.
    pub fn execution_end_time(&self, now: SystemTime) -> Option<SystemTime> {
        (now >= self.settles_at).then_some(self.settles_at)
    }

    /// `true` when `resource_type` and `resource_id` address THIS execution's
    /// target resource — the pairing `ListExecutions` scopes its listing by.
    pub fn targets(&self, resource_type: &str, resource_id: &str) -> bool {
        target_resource_addresses(&self.target_resource, resource_type, resource_id)
    }

    /// This execution's status at `now`: the state [`Self::state`] derives, which
    /// is the one member the modelled status carries.
    ///
    /// Here rather than in a handler so the read of ONE execution and the listing
    /// that holds it report the same state for it.
    pub fn status(&self, now: SystemTime) -> ExecutionStatus {
        ExecutionStatus {
            state: self.state(now).to_string(),
        }
    }

    /// This execution as the summary a listing reports for it, at `now`.
    ///
    /// Derived here rather than in the handler so every view of one stored
    /// execution reports the same state, end time and versions.
    pub fn summary(&self, now: SystemTime) -> ExecutionSummary {
        ExecutionSummary {
            execution_id: self.execution_id.clone(),
            action_type: self.action_type.clone(),
            target_resource: self.target_resource.clone(),
            target_resource_version: self.target_resource_version.clone(),
            resolve_to: self.resolve_to.clone(),
            execution_start_time: to_date_time(self.execution_start_time),
            execution_end_time: self.execution_end_time(now).map(to_date_time),
            execution_status: self.status(now),
            execution_entity_version: self.execution_entity_version.clone(),
        }
    }
}

impl ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// The execution summary of the computation model `computation_model_id`, as
    /// of `now`: how many of the executions this store holds against that model
    /// are in each `ExecutionState`, as the decimal counts the modelled summary
    /// values are strings of.
    ///
    /// `resolved_asset_id` narrows the summary to the executions that resolve to
    /// ONE asset, which is what a request naming a resolved resource asks the
    /// summary to cover.
    ///
    /// The counts are DERIVED from the stored executions on every read, through
    /// the one [`ExecutionRecord::state`] rule a listing reports an execution by,
    /// so a summary follows what the clock has made of each run — an execution
    /// counts as running until its run is due to end and under its outcome from
    /// then on — and reading one writes nothing.
    ///
    /// The keys are [`EXECUTION_STATES`] and the values decimal counts, both of
    /// which the modelled `ComputationModelExecutionSummaryKey` and
    /// `ComputationModelExecutionSummaryValue` patterns admit.
    pub fn computation_model_execution_summary(
        &self,
        computation_model_id: &str,
        resolved_asset_id: Option<&str>,
        now: SystemTime,
    ) -> HashMap<String, String> {
        let mut counts: HashMap<&str, u64> =
            EXECUTION_STATES.iter().map(|state| (*state, 0)).collect();
        let covered = self.executions.values().filter(|record| {
            record.targets(TARGET_RESOURCE_TYPE_COMPUTATION_MODEL, computation_model_id)
                && resolved_asset_id.is_none_or(|asset_id| {
                    record
                        .resolve_to
                        .as_ref()
                        .is_some_and(|resolve_to| resolve_to.asset_id == asset_id)
                })
        });
        for record in covered {
            *counts.entry(record.state(now)).or_default() += 1;
        }
        counts
            .into_iter()
            .map(|(state, count)| (state.to_string(), count.to_string()))
            .collect()
    }

    /// The DISTINCT resources the executed actions of the computation model
    /// `computation_model_id` resolved to, as the asset ids that name them, in
    /// ascending id order.
    ///
    /// Executing an action on a resource leaves two records of it in this store —
    /// the action taken on that resource and the run of that action — and each
    /// carries the detailed resource the action resolved to, so both are read
    /// here and the ids are de-duplicated: a model acted on many times against
    /// one asset resolves to that ONE resource, and an action resolving to
    /// nothing contributes none. The order is the id's own, which is unique
    /// within the enumeration and stable across calls, so it also serves as the
    /// resume point of the listing that pages this.
    ///
    /// Derived from the stored records on every read, so nothing is written to
    /// report what the executes already made true.
    pub fn computation_model_resolved_assets(&self, computation_model_id: &str) -> Vec<String> {
        let from_executions = self
            .executions
            .values()
            .filter(|record| {
                record.targets(TARGET_RESOURCE_TYPE_COMPUTATION_MODEL, computation_model_id)
            })
            .filter_map(|record| record.resolve_to.as_ref());
        let from_actions = self
            .actions
            .values()
            .filter(|record| {
                record.targets(TARGET_RESOURCE_TYPE_COMPUTATION_MODEL, computation_model_id)
            })
            .filter_map(|record| record.resolve_to.as_ref());
        from_executions
            .chain(from_actions)
            .map(|resolve_to| resolve_to.asset_id.clone())
            .collect::<BTreeSet<String>>()
            .into_iter()
            .collect()
    }
}

/// `true` when the `(TargetResourceType, ID)` pair a request carries addresses
/// `target_resource`.
///
/// One place, so every listing scoped to a target resource — the executions of
/// one and the actions taken on one — resolves that pair the same way: a
/// `TargetResource` names an asset or a computation model, and the pair
/// addresses it only when the type picks the member the id matches.
pub fn target_resource_addresses(
    target_resource: &TargetResource,
    resource_type: &str,
    resource_id: &str,
) -> bool {
    let addressed = match resource_type {
        TARGET_RESOURCE_TYPE_ASSET => target_resource.asset_id.as_deref(),
        // A record of a computation model is addressed by that model's own id,
        // the same way a record of an asset is addressed by the asset's.
        TARGET_RESOURCE_TYPE_COMPUTATION_MODEL => target_resource.computation_model_id.as_deref(),
        _ => None,
    };
    addressed == Some(resource_id)
}

/// One action taken on a target resource: what `ListActions` summarises, keyed
/// by its own `actionId` in
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::actions`].
///
/// An action is a record of a request to act on a resource, so the instant it
/// ran ([`Self::execution_time`]) is stored rather than derived: it is what
/// orders the enumeration a listing pages through, and what the read form of a
/// single action reports.
#[derive(Debug, Clone)]
pub struct ActionRecord {
    /// The id this action is addressed by.
    pub action_id: String,
    /// The action definition this action runs.
    pub action_definition_id: String,
    /// The resource the action was taken on: an asset or a computation model.
    pub target_resource: TargetResource,
    /// The JSON payload the call that took this action supplied, which the read
    /// form of a single action reports back.
    pub action_payload: ActionPayload,
    /// The detailed resource this action resolves to, which only an action whose
    /// target resolves through a data binding carries.
    pub resolve_to: Option<ResolveTo>,
    /// When the action ran.
    pub execution_time: SystemTime,
    /// The `clientToken` the call that took this action carried, if it carried
    /// one, recorded as part of what that call asked for. What recognises a
    /// RETRY of that call is the store's registry of spent tokens
    /// ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::idempotent_verdict`]),
    /// which compares the request too: a token alone says only that some call
    /// used it, not that this one repeats it.
    pub client_token: Option<String>,
}

impl ActionRecord {
    /// `true` when `resource_type` and `resource_id` address THIS action's
    /// target resource — the pairing `ListActions` scopes its listing by.
    pub fn targets(&self, resource_type: &str, resource_id: &str) -> bool {
        target_resource_addresses(&self.target_resource, resource_type, resource_id)
    }

    /// This action as the summary a listing reports for it.
    ///
    /// Derived here rather than in the handler so every view of one stored
    /// action reports the same identifiers and references.
    pub fn summary(&self) -> ActionSummary {
        ActionSummary {
            action_id: Some(self.action_id.clone()),
            action_definition_id: Some(self.action_definition_id.clone()),
            target_resource: Some(self.target_resource.clone()),
            resolve_to: self.resolve_to.clone(),
        }
    }
}

/// The `kmsKeyArn` a workspace reports for the `kmsKeyId` it was given.
///
/// `kmsKeyId` "Accepts a key ID, key ARN, or key alias" while the read form
/// reports an ARN, so the three forms are normalised to one here: an ARN is
/// reported as it was supplied, and a bare key id or an `alias/<name>` becomes
/// the ARN of that same key in the request's region and this account. The id is
/// NOT resolved through KMS — this service holds no key registry — so an alias is
/// reported as the alias's own ARN rather than as the ARN of the key behind it.
pub fn kms_key_arn(region: &str, kms_key_id: &str) -> String {
    if kms_key_id.starts_with("arn:") {
        return kms_key_id.to_string();
    }
    let resource = if kms_key_id.starts_with("alias/") {
        kms_key_id.to_string()
    } else {
        format!("key/{kms_key_id}")
    };
    ::engine::emulator_arn(KMS_ARN_SERVICE, region, &resource)
}

/// The encryption configuration a read reports for `workspace`: the stored
/// `EncryptionType`, and the ARN of the key the create named.
///
/// Derived from the stored record alone — the region comes from the workspace's
/// own ARN rather than from the calling request — so every read of one workspace
/// reports the same value.
pub fn workspace_encryption_info(
    workspace: &WorkspaceRecord,
) -> WorkspaceEncryptionConfigurationInfo {
    WorkspaceEncryptionConfigurationInfo {
        encryption_type: workspace.encryption.encryption_type.clone(),
        kms_key_arn: workspace
            .encryption
            .kms_key_id
            .as_deref()
            .map(|kms_key_id| kms_key_arn(arn_region(&workspace.arn), kms_key_id)),
    }
}

/// The region section of an ARN this service minted, or [`DEFAULT_REGION`] for a
/// string that is not one.
fn arn_region(arn: &str) -> &str {
    // `arn:<partition>:<service>:<region>:<account>:<resource>`.
    arn.split(':').nth(3).unwrap_or(DEFAULT_REGION)
}

/// Refuse a workspace encryption configuration this service cannot accept, in the
/// wording the refusal carries; `Ok` for one it can.
///
/// The rules the shape states, in ONE place because the create and the update
/// both supply it: `encryptionType` is an `EncryptionType`, and `kmsKeyId` is
/// "Required for KMS_BASED_ENCRYPTION; must be omitted for
/// SITEWISE_DEFAULT_ENCRYPTION". The value set is the generated, model-synced
/// `ENCRYPTION_TYPE_VALUES`, the way `PutLoggingOptions` reads its own enum's.
pub fn check_workspace_encryption(
    configuration: &WorkspaceEncryptionConfiguration,
) -> Result<(), String> {
    check_encryption_selection(
        configuration.encryption_type.as_str(),
        configuration.kms_key_id.as_deref(),
    )
}

/// The same rules over the two members a caller selects encryption with,
/// wherever they are selected: the workspace's own configuration above, and the
/// account-wide default encryption configuration, which carries the same pair of
/// members at the top level.
pub fn check_encryption_selection(
    encryption_type: &str,
    kms_key_id: Option<&str>,
) -> Result<(), String> {
    if !ENCRYPTION_TYPE_VALUES.contains(&encryption_type) {
        return Err(format!(
            "The encryptionType {encryption_type} is not supported; it must be one of {}.",
            ENCRYPTION_TYPE_VALUES.join(" | ")
        ));
    }
    match (
        encryption_type == ENCRYPTION_TYPE_KMS_BASED,
        kms_key_id.is_some(),
    ) {
        (true, false) => Err(format!(
            "A kmsKeyId is required when encryptionType is {ENCRYPTION_TYPE_KMS_BASED}."
        )),
        (false, true) => Err(format!(
            "A kmsKeyId must be omitted when encryptionType is \
             {ENCRYPTION_TYPE_SITEWISE_DEFAULT}."
        )),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Dataset export jobs
//
// `CreateDatasetExportJob` "starts an asynchronous job that exports dataset and
// time-series data from a workspace to Amazon S3. The operation returns a jobId
// immediately; poll DescribeDatasetExportJob to track progress". So the job
// record and every rule over it live here ONCE: the lifecycle the CLOCK drives,
// and the identity a reader addresses a job by.
//
// What the record does NOT hold is the exported content: the destination is a
// bucket in the CALLER's account, which this emulator does not serve, and no
// operation of this API ever answers an exported object. The record — the job's
// identity, its request, and where its lifecycle has got to — is the whole of
// what a client of this API can observe.
// ---------------------------------------------------------------------------

/// The `DatasetExportJobStatus` value a job reports from the instant it is
/// created (`crate::ops::types::DATASET_EXPORT_JOB_STATUS_VALUES`).
pub const EXPORT_JOB_SUBMITTED: &str = "SUBMITTED";
/// The value a job reports once the clock has started it processing.
pub const EXPORT_JOB_RUNNING: &str = "RUNNING";
/// The value a job reports once the clock has finished it. Terminal.
pub const EXPORT_JOB_COMPLETED: &str = "COMPLETED";

/// How long each step of a dataset export job's lifecycle takes: a created job
/// stays `SUBMITTED` for this long, then reports `RUNNING` for this long, then is
/// `COMPLETED`.
///
/// CHOSEN, not documented: AWS publishes no duration for either step — an export
/// takes as long as the data it exports. The span is deliberately NON-ZERO, so a
/// caller that describes a job straight after creating it still observes the
/// transient status the real service reports, and short enough that a caller can
/// poll the job through to its terminal status.
pub const EXPORT_JOB_STEP: Duration = Duration::from_millis(500);

/// How many dataset export jobs one workspace holds.
///
/// CHOSEN: a create is the only thing that adds a job and nothing evicts one, so
/// without a bound a stream of creates would grow one emulator process without
/// limit.
pub const MAX_DATASET_EXPORT_JOBS_PER_WORKSPACE: usize = 100;

/// How many dataset export jobs the store holds in total, across every
/// workspace.
///
/// The whole map's own limit, for the reason [`MAX_PIPELINES`] states: the
/// per-workspace cap above bounds one workspace's jobs, and this one bounds the
/// map every workspace's jobs share.
pub const MAX_DATASET_EXPORT_JOBS: usize = 1_000;

/// One dataset export job, as `CreateDatasetExportJob` starts it and
/// `DescribeDatasetExportJob` reports it.
///
/// Its STATUS is not stored: it is DERIVED from the instant the job was created
/// ([`Self::status`]), because the lifecycle is owed by the clock rather than by
/// any call. So a reader reports the status the clock has made due without
/// writing to the store, and two readers of one job can never disagree about
/// where it has got to.
#[derive(Debug, Clone)]
pub struct DatasetExportJobRecord {
    /// The `jobId` this job was minted with ([`new_id`]).
    pub job_id: String,
    /// The workspace the job was created in. A read is scoped to a workspace, so
    /// the pair is the job's identity — see [`dataset_export_job_key`].
    pub workspace_name: String,
    /// The `destinationS3Uri` the request named: "The S3 URI where output clips
    /// will be written". This service does not write it.
    pub destination_s3_uri: String,
    /// The `errorReportLocation` the request named.
    pub error_report_location: ExportErrorReportLocation,
    /// "The processing input that was provided in the CreateDatasetExportJob
    /// request", kept verbatim so the reader answers the request's own input.
    pub input: ProcessingInput,
    /// The `clientToken` the create carried, if any, recorded as part of what
    /// that call asked for. What recognises a RETRY of that call is the store's
    /// registry of spent tokens
    /// ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::idempotent_verdict`]),
    /// which compares the request too: a token alone says only that some call
    /// used it, not that this one repeats it.
    pub client_token: Option<String>,
    /// When the job was created, which is the `startedAt` a read reports.
    pub started_at: SystemTime,
}

impl DatasetExportJobRecord {
    /// The job's `status` at `now`: `SUBMITTED` until the first
    /// [`EXPORT_JOB_STEP`] has elapsed, `RUNNING` until the second has,
    /// `COMPLETED` after that. `COMPLETED` is terminal, so the status never moves
    /// again.
    pub fn status(&self, now: SystemTime) -> &'static str {
        match self.completed_at(now) {
            Some(_) => EXPORT_JOB_COMPLETED,
            None if now >= self.started_at + EXPORT_JOB_STEP => EXPORT_JOB_RUNNING,
            None => EXPORT_JOB_SUBMITTED,
        }
    }

    /// "The timestamp when the job completed, or null if the job is still
    /// running": the instant the terminal status fell due, and `None` until it
    /// has.
    pub fn completed_at(&self, now: SystemTime) -> Option<SystemTime> {
        let completes_at = self.started_at + 2 * EXPORT_JOB_STEP;
        (now >= completes_at).then_some(completes_at)
    }
}

/// The key [`ComAmazonawsIotsitewiseAwsiotsitewiseState::dataset_export_jobs`]
/// holds a job under: the workspace it was created in and its own id, because
/// every operation on a job addresses it within a workspace, so a job of one
/// workspace is not reachable through another.
pub fn dataset_export_job_key(workspace_name: &str, job_id: &str) -> (String, String) {
    (workspace_name.to_string(), job_id.to_string())
}

/// The resource type section a dataset export job's ARN carries, which is what
/// the modelled conflict refusal reports for one.
pub const DATASET_EXPORT_JOB_RESOURCE_TYPE: &str = "dataset-export-job";

/// The canonical ARN of a dataset export job in `region`.
///
/// The resource section carries the WORKSPACE as well as the id, for the reason
/// [`query_arn`] states: a job is addressed only inside the workspace it was
/// created in.
pub fn dataset_export_job_arn(region: &str, workspace_name: &str, job_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{DATASET_EXPORT_JOB_RESOURCE_TYPE}/{workspace_name}/{job_id}"),
    )
}

/// Refuse a `ProcessingInput` whose items break a rule the shapes state, in the
/// wording the refusal carries; `Ok` for one that keeps them.
///
/// The rules come from the shapes themselves: a `TimeseriesItem` is "a single
/// timeseries item to process. Exactly one of timeSeriesId or propertyAlias must
/// be provided" (each member is also documented as "mutually exclusive" with the
/// other), and a `TrimSettings` end time "must be greater than startTime". Both
/// hold wherever the shapes appear, so they are checked in ONE place.
pub fn check_processing_input(input: &ProcessingInput) -> Result<(), String> {
    match input {
        ProcessingInput::Timeseries(items) => {
            for (at, item) in items.iter().enumerate() {
                match (item.time_series_id.as_deref(), item.property_alias.as_deref()) {
                    (Some(_), Some(_)) => {
                        return Err(format!(
                            "input.timeseries[{at}]: timeSeriesId and propertyAlias are mutually exclusive."
                        ));
                    }
                    (None, None) => {
                        return Err(format!(
                            "input.timeseries[{at}]: exactly one of timeSeriesId or propertyAlias must be provided."
                        ));
                    }
                    _ => {}
                }
                if let Some(trim) = item.trim_settings.as_ref() {
                    check_trim_settings(trim)
                        .map_err(|why| format!("input.timeseries[{at}]: {why}"))?;
                }
            }
            Ok(())
        }
        ProcessingInput::Dataset(item) => match item.trim_settings.as_ref() {
            Some(trim) => check_trim_settings(trim).map_err(|why| format!("input.dataset: {why}")),
            None => Ok(()),
        },
    }
}

/// Refuse a `TrimSettings` whose end time does not follow its start time: "Must
/// be greater than startTime".
fn check_trim_settings(trim: &TrimSettings) -> Result<(), String> {
    if timestamp_key(&trim.end_time) > timestamp_key(&trim.start_time) {
        Ok(())
    } else {
        Err("trimSettings.endTime must be greater than trimSettings.startTime.".to_string())
    }
}

/// Refuse a `computations` list that is not a pipeline DAG, in the wording the
/// refusal carries; `Ok` for one that is.
///
/// The three rules `CreatePipeline` and the `ComputeNode` shape state, enforced
/// in ONE place because both the create and the update supply the same list:
/// `computeNodeName` is "The unique name for this compute node within the
/// pipeline", `dependsOn` is "A list of compute node names that must complete
/// successfully before this node can start" — so a dependency names a node of
/// this same pipeline — and "Cyclic dependencies are not allowed".
///
/// Linear in the nodes and their dependencies: the names go into a hashed set,
/// and the acyclicity check is one in-degree pass plus a worklist (Kahn's
/// algorithm), so no shape of list makes the walk cost more than the list itself.
pub fn check_compute_nodes(computations: &[ComputeNode]) -> Result<(), String> {
    let mut names: HashSet<&str> = HashSet::with_capacity(computations.len());
    for node in computations {
        if !names.insert(node.compute_node_name.as_str()) {
            return Err(format!(
                "The compute node name {} is used by more than one compute node; \
                 a compute node name is unique within the pipeline.",
                node.compute_node_name
            ));
        }
    }

    // How many dependencies each node is still waiting on, and which nodes wait
    // on it — built in the same pass that checks every dependency names a node of
    // this pipeline.
    let mut waiting: HashMap<&str, usize> = HashMap::with_capacity(computations.len());
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::with_capacity(computations.len());
    for node in computations {
        let name = node.compute_node_name.as_str();
        let mut outstanding = 0usize;
        for dependency in node.depends_on.iter().flatten() {
            if !names.contains(dependency.as_str()) {
                return Err(format!(
                    "The compute node {name} depends on {dependency}, which is not a \
                     compute node of this pipeline."
                ));
            }
            outstanding += 1;
            dependents
                .entry(dependency.as_str())
                .or_default()
                .push(name);
        }
        waiting.insert(name, outstanding);
    }

    // Kahn's algorithm: start from the nodes waiting on nothing and release the
    // nodes whose last dependency has been released. A node still waiting when
    // the worklist empties sits on a cycle — including a node that depends on
    // itself, which waits on a dependency that is never released.
    let mut ready: Vec<&str> = waiting
        .iter()
        .filter(|(_, outstanding)| **outstanding == 0)
        .map(|(name, _)| *name)
        .collect();
    let mut settled = 0usize;
    while let Some(name) = ready.pop() {
        settled += 1;
        for dependent in dependents.get(name).into_iter().flatten().copied() {
            if let Some(outstanding) = waiting.get_mut(dependent) {
                *outstanding -= 1;
                if *outstanding == 0 {
                    ready.push(dependent);
                }
            }
        }
    }
    if settled < computations.len() {
        let mut cyclic: Vec<&str> = waiting
            .iter()
            .filter(|(_, outstanding)| **outstanding > 0)
            .map(|(name, _)| *name)
            .collect();
        cyclic.sort_unstable();
        return Err(format!(
            "The compute nodes {} form a cyclic dependency; a pipeline is a \
             directed acyclic graph.",
            cyclic.join(", ")
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// The asynchronous queries of a workspace: what `StartQuery` starts,
// `CancelQuery` cancels and `DescribeQuery` reports. A query's STATUS is derived
// from the instant it was submitted — and, once one is recorded, from the instant
// its cancel was asked for — exactly as a dataset export job's is, so a read
// reports what the clock has made due without writing anything.
// ---------------------------------------------------------------------------

/// The `QueryStatus` a query reports from the instant it is submitted: "The
/// initial query status. The value is always SUBMITTED upon creation"
/// (`crate::ops::types::QUERY_STATUS_VALUES`).
pub const QUERY_SUBMITTED: &str = "SUBMITTED";

/// The value a query reports once the clock has started it running.
pub const QUERY_RUNNING: &str = "RUNNING";

/// The value a query whose statement produced a result reports. Terminal.
pub const QUERY_COMPLETED: &str = "COMPLETED";

/// The value a query whose run did not finish reports, beside the message saying
/// why. Terminal.
pub const QUERY_FAILED: &str = "FAILED";

/// The value a query a cancel was recorded on reports until that cancellation has
/// taken effect: the `QueryStatus` value the enum carries for a cancel in flight.
pub const QUERY_CANCELING: &str = "CANCELING";

/// The value a cancelled query reports once its cancellation has taken effect.
/// Terminal.
pub const QUERY_CANCELED: &str = "CANCELED";

/// The resource type section a query's ARN carries, which is what the modelled
/// conflict refusal reports for one.
pub const QUERY_RESOURCE_TYPE: &str = "query";

/// The canonical ARN of a query in `region`.
///
/// The resource section carries the WORKSPACE as well as the id, for the reason
/// [`pipeline_arn`] states: a query is addressed only inside the workspace it was
/// started in.
pub fn query_arn(region: &str, workspace_name: &str, query_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{QUERY_RESOURCE_TYPE}/{workspace_name}/{query_id}"),
    )
}

/// How long each step of a query's lifecycle takes: a submitted query stays
/// `SUBMITTED` for this long, then reports `RUNNING` for this long, then is in its
/// terminal status.
///
/// CHOSEN, not documented: AWS publishes no duration for either step — a query
/// takes as long as the data it reads. The span is deliberately NON-ZERO, so a
/// caller that describes a query straight after starting it still observes the
/// transient statuses the real service reports, and short enough that a caller can
/// poll the query through to its terminal one.
pub const QUERY_STEP: Duration = Duration::from_millis(250);

/// How many queries one workspace holds.
///
/// CHOSEN: starting a query is the only thing that adds one and nothing evicts
/// one, so without a bound a stream of queries would grow one emulator process
/// without limit.
pub const MAX_QUERIES_PER_WORKSPACE: usize = 100;

/// How many queries the store holds in total, across every workspace. The
/// per-workspace cap cannot bound the store on its own, for the reason
/// [`MAX_DATASET_EXPORT_JOBS`] states.
pub const MAX_QUERIES: usize = 1_000;

/// One query of a workspace, as `StartQuery` started it.
///
/// The statement is RUN while `StartQuery` is serving the request, so what the run
/// produced is known from the moment the query exists and nothing has to be
/// computed again later. What is NOT stored is the query's status: that is DERIVED
/// from [`Self::submitted_at`] ([`Self::status`]), because the lifecycle is owed by
/// the clock rather than by any call, so two readers of one query can never
/// disagree about where it has got to and a read can never move it on.
#[derive(Debug, Clone)]
pub struct QueryRecord {
    /// The `queryId` this query was minted with ([`new_id`]).
    pub query_id: String,
    /// The workspace the query was started in. A read is scoped to a workspace,
    /// so the pair is the query's identity — see [`query_key`].
    pub workspace_name: String,
    /// The `queryStatement` the request carried. Modelled `@sensitive`, so no
    /// operation reports it back; it is kept because it is what the query IS, and
    /// what a reader of the query's results re-reads the store with.
    pub query_statement: String,
    /// The `clientToken` the request carried, if any, recorded as part of what
    /// that call asked for. What recognises a RETRY of that call is the store's
    /// registry of spent tokens
    /// ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::idempotent_verdict`]),
    /// which compares the request too: a token alone says only that some call
    /// used it, not that this one repeats it.
    pub client_token: Option<String>,
    /// When the query was submitted, which is the `submittedAt` a read reports.
    pub submitted_at: SystemTime,
    /// When a cancel was recorded on this query, if one was. A cancel is recorded
    /// only while the query has not reached a terminal status, and from then on it
    /// is this instant — not [`Self::submitted_at`] — the query's remaining
    /// lifecycle is owed from.
    pub cancel_requested_at: Option<SystemTime>,
    /// What running the statement produced.
    pub outcome: QueryOutcome,
}

/// What running one query's statement over the store produced.
#[derive(Debug, Clone)]
pub enum QueryOutcome {
    /// The run answered rows: how many there were, how many bytes of stored cells
    /// it read and how long it took. These are the `QueryStatistics` a read
    /// reports, each measured by the run itself.
    Completed {
        /// "The total number of rows returned by the query."
        row_count: i64,
        /// "The total number of bytes scanned during query execution."
        bytes_scanned: i64,
        /// "The total query execution time, in milliseconds."
        execution_time_in_millis: i64,
    },
    /// The run did not finish, and the message saying why — the `errorMessage` a
    /// read reports beside the `FAILED` status.
    Failed {
        /// "A human-readable error description for a failed query."
        error_message: String,
    },
}

impl QueryRecord {
    /// The query's `status` at `now`: `SUBMITTED` until the first [`QUERY_STEP`]
    /// has elapsed, `RUNNING` until the second has, and then the terminal status
    /// its run earned — `COMPLETED` for a run that answered rows, `FAILED` for one
    /// that did not finish. A terminal status never moves again.
    ///
    /// A cancelled query reports `CANCELING` until its cancellation takes effect
    /// and `CANCELED` from then on, whatever its run produced: the cancel is
    /// recorded only on a query that had not reached a terminal status, so the
    /// cancellation is the terminal state it reaches.
    pub fn status(&self, now: SystemTime) -> &'static str {
        match (self.cancel_requested_at, self.completed_at(now)) {
            (Some(_), None) => QUERY_CANCELING,
            (Some(_), Some(_)) => QUERY_CANCELED,
            (None, Some(_)) => match self.outcome {
                QueryOutcome::Completed { .. } => QUERY_COMPLETED,
                QueryOutcome::Failed { .. } => QUERY_FAILED,
            },
            (None, None) if now >= self.submitted_at + QUERY_STEP => QUERY_RUNNING,
            (None, None) => QUERY_SUBMITTED,
        }
    }

    /// "The date and time when the query reached a terminal state": the instant
    /// the terminal status fell due, and `None` until it has.
    pub fn completed_at(&self, now: SystemTime) -> Option<SystemTime> {
        let completes_at = self.terminal_at();
        (now >= completes_at).then_some(completes_at)
    }

    /// The instant this query's terminal status falls due: one step after the
    /// cancel that was recorded on it, and otherwise the end of the run-through
    /// span [`QUERY_STEP`] describes. A recorded cancel REPLACES the span the
    /// submission owed, because it is what the query now settles into.
    fn terminal_at(&self) -> SystemTime {
        match self.cancel_requested_at {
            Some(requested_at) => requested_at + QUERY_STEP,
            None => self.submitted_at + 2 * QUERY_STEP,
        }
    }

    /// Whether a cancel may still be recorded on this query at `now`: one the
    /// clock has not yet taken to a terminal status. A query a cancel was already
    /// recorded on is not cancelled again — the repeat is a no-op reporting the
    /// status it stands in.
    pub fn is_cancelable(&self, now: SystemTime) -> bool {
        self.cancel_requested_at.is_none() && self.completed_at(now).is_none()
    }

    /// The query's `statistics` at `now`: "statistics about a COMPLETED query
    /// execution", so a query that has not reached its terminal status yet, one
    /// whose run failed, and one that was cancelled report none.
    pub fn statistics(&self, now: SystemTime) -> Option<QueryStatistics> {
        match (&self.outcome, self.status(now)) {
            (
                QueryOutcome::Completed {
                    row_count,
                    bytes_scanned,
                    execution_time_in_millis,
                },
                QUERY_COMPLETED,
            ) => Some(QueryStatistics {
                row_count: *row_count,
                bytes_scanned: *bytes_scanned,
                execution_time_in_millis: *execution_time_in_millis,
            }),
            _ => None,
        }
    }

    /// The query's `errorMessage` at `now`: the message its failed run carried,
    /// and `None` unless that failure is the status the query reports — a query
    /// cancelled before its run's failure fell due reports no message, because the
    /// status it reached is the cancellation and not the failure.
    pub fn error_message(&self, now: SystemTime) -> Option<String> {
        match (&self.outcome, self.status(now)) {
            (QueryOutcome::Failed { error_message }, QUERY_FAILED) => Some(error_message.clone()),
            _ => None,
        }
    }

    /// This query as the summary a listing reports for it, at `now`.
    ///
    /// Derived here rather than in the handler so a listing and a read of one
    /// stored query cannot report different statuses or instants for it.
    pub fn summary(&self, now: SystemTime) -> QuerySummary {
        QuerySummary {
            query_id: self.query_id.clone(),
            status: self.status(now).to_string(),
            submitted_at: to_date_time(self.submitted_at),
            completed_at: self.completed_at(now).map(to_date_time),
        }
    }
}

/// The key [`ComAmazonawsIotsitewiseAwsiotsitewiseState::queries`] holds a query
/// under: the workspace it was started in and its own id, because every operation
/// on a query addresses it within a workspace, so a query of one workspace is not
/// reachable through another.
pub fn query_key(workspace_name: &str, query_id: &str) -> (String, String) {
    (workspace_name.to_string(), query_id.to_string())
}

/// The `ListTimeSeriesType` value of a series associated with an asset property.
pub const TIME_SERIES_TYPE_ASSOCIATED: &str = "ASSOCIATED";

/// The `ListTimeSeriesType` value of a series associated with no asset property,
/// as the member's own documentation states it: "DISASSOCIATED \u2013 The time series
/// isn't associated with any asset property."
pub const TIME_SERIES_TYPE_DISASSOCIATED: &str = "DISASSOCIATED";

/// A stored record the tagging operations reach by ARN: it knows the ARN it was
/// minted with and owns the one tag set every operation that touches its tags
/// reads and writes.
///
/// Implemented once per taggable record, so
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::tags_for_resource`] and its
/// `_mut` twin hold only the list of kinds and cannot drift in how a kind is
/// resolved.
pub trait TaggedRecord {
    /// The ARN this record was minted with.
    fn arn(&self) -> &str;
    /// The record's tag set.
    fn tags(&self) -> &HashMap<String, String>;
    /// The same tag set, for writing.
    fn tags_mut(&mut self) -> &mut HashMap<String, String>;
}

/// Implement [`TaggedRecord`] for a record carrying an `arn` and a `tags` field.
macro_rules! tagged_record {
    ($record:ty) => {
        impl TaggedRecord for $record {
            fn arn(&self) -> &str {
                &self.arn
            }

            fn tags(&self) -> &HashMap<String, String> {
                &self.tags
            }

            fn tags_mut(&mut self) -> &mut HashMap<String, String> {
                &mut self.tags
            }
        }
    };
}

tagged_record!(AssetRecord);
tagged_record!(AssetModelRecord);
tagged_record!(GatewayRecord);
tagged_record!(PortalRecord);
tagged_record!(ProjectRecord);
tagged_record!(PipelineRecord);
tagged_record!(TaskRecord);
tagged_record!(DatasetRecord);
tagged_record!(WorkspaceRecord);
tagged_record!(ComputationModelRecord);
tagged_record!(AccessPolicyRecord);

/// `record`'s tag set, but only when `record` is the one `arn` names exactly —
/// same partition, account and region, not merely the same id.
fn tags_of<'a, R: TaggedRecord>(
    record: Option<&'a R>,
    arn: &str,
) -> Option<&'a HashMap<String, String>> {
    record
        .filter(|record| record.arn() == arn)
        .map(TaggedRecord::tags)
}

/// The same, for writing. See [`tags_of`].
fn tags_of_mut<'a, R: TaggedRecord>(
    record: Option<&'a mut R>,
    arn: &str,
) -> Option<&'a mut HashMap<String, String>> {
    record
        .filter(|record| record.arn() == arn)
        .map(TaggedRecord::tags_mut)
}

/// Per-service state for com-amazonaws-iotsitewise-awsiotsitewise. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// Asset models by `assetModelId`.
    pub asset_models: HashMap<String, AssetModelRecord>,
    /// `assetModelExternalId` -> `assetModelId`, so `externalId:` labels resolve.
    pub asset_model_external_ids: HashMap<String, String>,
    /// Assets by `assetId`.
    pub assets: HashMap<String, AssetRecord>,
    /// `assetExternalId` -> `assetId`, so `externalId:` labels resolve.
    pub asset_external_ids: HashMap<String, String>,
    /// `(assetId, hierarchyId)` -> the child assets associated under that
    /// hierarchy. `ListAssets`'s `TOP_LEVEL` filter reads it to tell a root asset
    /// from one that is some other asset's child.
    pub asset_hierarchy_children: HashMap<(String, String), Vec<String>>,
    /// `(assetId, hierarchyId, childAssetId)` -> the instant the association
    /// recorded for that triple finishes settling.
    ///
    /// The association set above says WHICH associations exist; this says how far
    /// the one that created an entry has got, which is what tells a second call on
    /// the same triple whether it collides with an operation still in flight or
    /// with an association already established (see [`AssociationState`]). Written
    /// and pruned beside the set itself by [`Self::associate_child`] and
    /// [`Self::disassociate_child`], so the two cannot drift.
    pub asset_hierarchy_settles_at: HashMap<(String, String, String), SystemTime>,
    /// `assetId` -> `propertyId` -> the property's time series, kept sorted
    /// ascending by timestamp so the latest value is the last element.
    ///
    /// Nested rather than keyed by an `(assetId, propertyId)` tuple so a lookup
    /// borrows the two ids instead of allocating a pair of `String`s on every
    /// read and on every value of a batch write.
    pub property_values: HashMap<String, HashMap<String, Vec<AssetPropertyValue>>>,
    /// `propertyAlias` -> `(assetId, propertyId)`, so alias lookups resolve.
    pub property_alias_index: HashMap<String, (String, String)>,
    /// The time series no asset property owns, by the alias that identifies each
    /// one. Disjoint from [`Self::property_alias_index`] by construction: an
    /// alias names an associated stream or a disassociated one, never both — see
    /// [`Self::set_property_alias`] and [`Self::disassociate_time_series`].
    pub disassociated_time_series: HashMap<String, DisassociatedTimeSeriesRecord>,
    /// Gateways by `gatewayId`.
    pub gateways: HashMap<String, GatewayRecord>,
    /// [`gateway_device_key`] -> `gatewayId`: which gateway, if any, already runs
    /// on a given platform device. One device hosts at most one gateway, so this
    /// is what makes a second create naming the same device a conflict.
    pub gateway_devices: HashMap<String, String>,
    /// IoT SiteWise Monitor portals by `portalId`.
    pub portals: HashMap<String, PortalRecord>,
    /// IoT SiteWise Monitor projects by `projectId`. The id alone identifies one
    /// — every operation on a project addresses it by that id — and the record
    /// names the portal the project lives in, so the listing form filters on the
    /// record rather than on a second index.
    pub projects: HashMap<String, ProjectRecord>,
    /// IoT SiteWise Monitor access policies by `accessPolicyId`. The id alone
    /// identifies one, and the record names the identity and the Monitor
    /// resource the policy is about, so the listing forms filter on the record
    /// rather than on a second index.
    pub access_policies: HashMap<String, AccessPolicyRecord>,
    /// Pipelines by [`pipeline_key`] — the workspace they live in and their own
    /// name — because a pipeline name is unique only within its workspace.
    pub pipelines: HashMap<(String, String), PipelineRecord>,
    /// Tasks by [`task_key`] — the workspace they live in and their own name —
    /// because a task name is unique only within its workspace.
    pub tasks: HashMap<(String, String), TaskRecord>,
    /// The logging options each scope has been SET to, keyed by the
    /// `workspaceName` the call named and by [`ACCOUNT_LOGGING_SCOPE`] for a call
    /// that named none. An absent key is a scope no `PutLoggingOptions` has
    /// configured, which reports [`DEFAULT_LOGGING_LEVEL`] — see
    /// [`Self::logging_options_for`]. Bounded at [`MAX_LOGGING_SCOPES`] by
    /// evicting the least recently set workspace scope, which is why each entry
    /// carries the instant it was set.
    pub logging_options: HashMap<String, LoggingOptionsRecord>,
    /// Datasets by `datasetId`.
    pub datasets: HashMap<String, DatasetRecord>,
    /// The data segments of each dataset, by `datasetId` — a dataset id
    /// identifies one across every workspace, so the workspace a segment is
    /// addressed through is the one on the dataset's own record. Each dataset's
    /// segments are kept sorted by [`data_segment_key`] and single-valued on it,
    /// and bounded at [`MAX_DATA_SEGMENTS_PER_DATASET`]. Written by
    /// [`Self::put_dataset_data_segment`] and removed by
    /// [`Self::delete_dataset_data_segment`].
    pub dataset_data_segments: HashMap<String, Vec<DataSegmentRecord>>,
    /// Searches by [`search_key`] — the workspace they run against and their
    /// own id — because a search is addressed by that pair. Bounded at
    /// [`MAX_SEARCHES`] / [`MAX_SEARCHES_PER_WORKSPACE`], which `StartSearch`
    /// refuses past.
    pub searches: HashMap<(String, String), SearchRecord>,
    /// Executions of actions by `executionId`, which `ListExecutions`
    /// summarises for the target resource a request names.
    pub executions: HashMap<String, ExecutionRecord>,
    /// Actions taken on a target resource by `actionId`, which `ListActions`
    /// summarises for the target resource a request names.
    pub actions: HashMap<String, ActionRecord>,
    /// The account's default encryption configuration, as the last
    /// `PutDefaultEncryptionConfiguration` left it. `None` is an account nothing
    /// has configured, which reports [`ENCRYPTION_TYPE_SITEWISE_DEFAULT`] without storing
    /// anything — see [`Self::default_encryption_configuration`].
    pub default_encryption: Option<DefaultEncryptionRecord>,
    /// The account's storage configuration, as the last
    /// `PutStorageConfiguration` left it. `None` is an account nothing has
    /// configured, which reports the hot-tier default WITHOUT storing anything —
    /// see [`Self::storage_configuration`].
    pub storage: Option<StorageConfigurationRecord>,
    /// Dataset export jobs by [`dataset_export_job_key`] — the workspace they
    /// were created in and their own id — because every operation on a job
    /// addresses it within a workspace.
    pub dataset_export_jobs: HashMap<(String, String), DatasetExportJobRecord>,
    /// The asynchronous queries of every workspace, by [`query_key`] — the
    /// workspace they were started in and their own id — because every operation
    /// on a query addresses it within a workspace. Written by `StartQuery`,
    /// read by `DescribeQuery` and enumerated, one workspace at a time, by
    /// `ListQueries`; bounded by [`MAX_QUERIES`] and
    /// [`MAX_QUERIES_PER_WORKSPACE`].
    pub queries: HashMap<(String, String), QueryRecord>,
    /// The enrichment jobs of every workspace, by [`enrichment_job_key`] — the
    /// workspace they were created in and their own id — because every
    /// operation on a job addresses it within a workspace. Written by
    /// `CreateEnrichmentJob` and enumerated, one workspace at a time, by
    /// `ListEnrichmentJobs`; bounded by [`MAX_ENRICHMENT_JOBS`] and
    /// [`MAX_ENRICHMENT_JOBS_PER_WORKSPACE`].
    pub enrichment_jobs: HashMap<(String, String), EnrichmentJobRecord>,
    /// Workspaces by `workspaceName`, which is what identifies one in an
    /// account and region.
    pub workspaces: HashMap<String, WorkspaceRecord>,
    /// Applications by `applicationId`, which identifies one across every
    /// workspace; the workspace it was created in lives on its record, because
    /// the modelled reads address an application by `(workspaceName, id)`.
    /// Written by `CreateApplication` and read by `DescribeApplication`.
    pub applications: HashMap<String, ApplicationRecord>,
    /// Pipeline executions by `pipelineExecutionId`. The id alone identifies one,
    /// and the record names the workspace and pipeline it belongs to, so a read
    /// scoped to another pipeline resolves to nothing.
    pub pipeline_executions: HashMap<String, PipelineExecutionRecord>,
    /// The capture sources (video streams) of every workspace: the workspace
    /// they live in, then the source's own `timeSeriesId`, because a capture
    /// source is addressable only inside its workspace — nested so a lookup
    /// borrows the two names instead of allocating a key on every read. Written
    /// by [`Self::put_capture_segment`], read by `GetCaptureData` and by the
    /// workspace-scoped forms of the time-series operations, whose series these
    /// are, and removed by [`Self::forget_capture_source`].
    pub capture_sources: HashMap<String, HashMap<String, CaptureSourceRecord>>,
    /// `workspaceName` -> `propertyAlias` -> `timeSeriesId`, so the alias form of
    /// a capture-source lookup resolves to the same record the id form does.
    /// Written beside the source itself, and bounded by it: at most one entry per
    /// source that carries an alias.
    pub capture_source_aliases: HashMap<String, HashMap<String, String>>,
    /// Computation models by `computationModelId`.
    pub computation_models: HashMap<String, ComputationModelRecord>,
    /// The interface relationships, by [`interface_relationship_key`] — the asset
    /// model the interface is applied to and the interface asset model — because
    /// one asset model may implement several interfaces and one interface may be
    /// applied to several asset models. Bounded by
    /// [`MAX_INTERFACE_RELATIONSHIPS`].
    pub interface_relationships: HashMap<(String, String), InterfaceRelationshipRecord>,
    /// IoT SiteWise Monitor dashboards by `dashboardId`, which identifies one
    /// across every project; the project it was created in lives on its record,
    /// because that is what a read reports and what the listing form filters on.
    /// Written by `CreateDashboard` and `UpdateDashboard`, read by
    /// `DescribeDashboard`.
    pub dashboards: HashMap<String, DashboardRecord>,
    /// Bulk import jobs by `jobId`, which identifies one across the account:
    /// `DescribeBulkImportJob` addresses a job by that id alone and only
    /// NARROWS the read with an optional `workspaceName`, so the workspace a
    /// job was created for (a job ingesting into a session dataset has one,
    /// a job ingesting into time series outside a workspace has none) lives on
    /// the record. Written by `CreateBulkImportJob` and read by
    /// `DescribeBulkImportJob`; bounded by [`MAX_BULK_IMPORT_JOBS`]. A job whose
    /// destination is a session dataset is removed with that destination —
    /// [`Self::forget_dataset`] and [`Self::forget_workspace`] — because nothing
    /// deletes a job on its own and a read of one must not report a dataset or a
    /// workspace this store no longer holds.
    pub bulk_import_jobs: HashMap<String, BulkImportJobRecord>,
    /// The mutating calls this store has already served under a `clientToken`,
    /// by [`idempotency_key`] — the operation and the token — because a token is
    /// scoped to the operation it was sent to. Each entry says which request
    /// spent the token and which resource that call acted on, which is what
    /// lets a retry of it replay that answer instead of mutating a second time
    /// and a DIFFERENT request reusing the token be refused. Written by
    /// [`Self::record_idempotent_call`], read by [`Self::idempotent_verdict`]
    /// and bounded by [`MAX_IDEMPOTENT_CALLS`].
    pub idempotent_calls: HashMap<String, IdempotentCall>,
}

impl ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// The interface relationship between two STORED asset model ids, if one was
    /// applied. Resolve both labels with [`Self::resolve_asset_model_id`] first.
    pub fn interface_relationship(
        &self,
        asset_model_id: &str,
        interface_asset_model_id: &str,
    ) -> Option<&InterfaceRelationshipRecord> {
        self.interface_relationships
            .get(&interface_relationship_key(asset_model_id, interface_asset_model_id))
    }

    /// Drop every mapping of every stored interface relationship that names a
    /// property or a hierarchy `asset_model_id` no longer declares.
    ///
    /// A mapping pairs one member of the interface asset model with one member of
    /// the model the interface is applied to, so it is addressable only while BOTH
    /// models still declare their half of it: a mapping naming a member the model
    /// has since dropped would name nothing, exactly as a composite-model link
    /// pointing at a composite model its asset model no longer holds does. The
    /// model stands on either side of a relationship — it may implement
    /// interfaces and, being an interface, be implemented by other models — so
    /// both halves are checked against it. Asked after a call rewrites this
    /// model's definition; a model that is GONE takes its relationships whole
    /// ([`Self::forget_asset_model`]).
    ///
    /// Only the mappings are pruned: the relationship itself is the application of
    /// one model to another, which no change to a definition undoes, and mapping
    /// nothing is a state an application can already resolve to.
    pub fn prune_interface_relationship_mappings(&mut self, asset_model_id: &str) {
        let Some(record) = self.asset_models.get(asset_model_id) else {
            return;
        };
        let declared_properties: Vec<String> = record
            .properties
            .iter()
            .filter_map(|property| property.id.clone())
            .collect();
        let declared_hierarchies: Vec<String> = record
            .hierarchies
            .iter()
            .filter_map(|hierarchy| hierarchy.id.clone())
            .collect();
        for ((applied_to, interface), relationship) in self.interface_relationships.iter_mut() {
            let applied_to_side = applied_to == asset_model_id;
            let interface_side = interface == asset_model_id;
            if !applied_to_side && !interface_side {
                continue;
            }
            relationship.property_mappings.retain(|mapping| {
                (!applied_to_side
                    || declared_properties.contains(&mapping.asset_model_property_id))
                    && (!interface_side
                        || declared_properties
                            .contains(&mapping.interface_asset_model_property_id))
            });
            relationship.hierarchy_mappings.retain(|mapping| {
                (!applied_to_side
                    || declared_hierarchies.contains(&mapping.asset_model_hierarchy_id))
                    && (!interface_side
                        || declared_hierarchies
                            .contains(&mapping.interface_asset_model_hierarchy_id))
            });
        }
    }

    /// Resolve a `CustomID` label — a raw id or `externalId:<externalId>` — to a
    /// stored `assetModelId`.
    pub fn resolve_asset_model_id(&self, label: &str) -> Option<String> {
        match label.strip_prefix(EXTERNAL_ID_PREFIX) {
            Some(external) => self.asset_model_external_ids.get(external).cloned(),
            None => self.asset_models.contains_key(label).then(|| label.to_string()),
        }
    }

    /// The asset model, other than `except`, that already holds `name`.
    ///
    /// `assetModelName` is documented "A unique name for the asset model", so a
    /// name one asset model holds is not free for another: a create may not take
    /// it, and neither may a rename. `except` is the model the request is about —
    /// a model is never in conflict with the name it already carries — and is
    /// `None` for a create, which is about no stored model yet.
    ///
    /// Answers the RECORD, so the refusal names the holder by the id and the ARN
    /// that model carries rather than by anything rebuilt from the refused
    /// request. One place, so the create and the rename cannot read one rule two
    /// ways.
    pub fn asset_model_holding_name(
        &self,
        name: &str,
        except: Option<&str>,
    ) -> Option<&AssetModelRecord> {
        self.asset_models
            .values()
            .find(|record| record.name == name && Some(record.id.as_str()) != except)
    }

    /// Resolve a `CustomID` label — a raw id or `externalId:<externalId>` — to a
    /// stored `assetId`.
    pub fn resolve_asset_id(&self, label: &str) -> Option<String> {
        match label.strip_prefix(EXTERNAL_ID_PREFIX) {
            Some(external) => self.asset_external_ids.get(external).cloned(),
            None => self.assets.contains_key(label).then(|| label.to_string()),
        }
    }

    /// Make `alias` the one alias the `(asset_id, property_id)` property carries,
    /// or remove the property's alias with `None`.
    ///
    /// A binding lives in TWO places — the stored property's `alias` member, which
    /// `DescribeAsset` reports, and [`Self::property_alias_index`], which the alias
    /// form of a property lookup resolves through — so both move together here and
    /// nowhere else. The alias the property carried before is dropped from the
    /// index (only when the index still attributes it to THIS property, so a
    /// binding another property has since taken over survives).
    ///
    /// `asset_id` is a stored id, not a label: resolve it with
    /// [`Self::resolve_asset_id`] first. `property_id` takes either `CustomID`
    /// form. Answers `false` when no stored asset holds that property.
    ///
    /// A caller that binds an alias another property already carries displaces it:
    /// that property's own `alias` member is cleared here too, so no two stored
    /// properties can ever report one alias while the index names only the newer.
    /// `UpdateAssetProperty` refuses such a request with the modelled conflict
    /// before reaching this helper; this keeps the store consistent for any caller
    /// whose own rule allows the rebind.
    ///
    /// An asset's `last_update_date` is the instant of the last accepted write to
    /// it, and a property's alias binding is part of the asset that carries that
    /// property — so an accepted change to the binding moves the owning asset's
    /// instant to `now`, here and not in each caller, and the asset a displaced
    /// binding is taken FROM moves with it. A call that leaves the binding as it
    /// stood writes nothing, so a request that changes nothing cannot move it.
    pub fn set_property_alias(
        &mut self,
        asset_id: &str,
        property_id: &str,
        alias: Option<String>,
        now: SystemTime,
    ) -> bool {
        let Some(asset) = self.assets.get_mut(asset_id) else {
            return false;
        };
        // Every instance of the property that id names, so a definition two
        // composite models contribute cannot end up reporting the binding under
        // one of them and none under the other: the id names one property and
        // one stream, hence one binding.
        let positions = asset.property_positions(property_id);
        let Some(&first) = positions.first() else {
            return false;
        };
        let property = &asset.properties[first];
        let resolved_property_id = property.id.clone();
        let property_data_type = property.data_type.clone();
        let property_data_type_spec = property.data_type_spec.clone();
        let previous = property.alias.clone();
        for at in positions {
            asset.properties[at].alias = alias.clone();
        }
        if previous != alias {
            asset.last_update_date = now;
        }

        if let Some(previous) = previous {
            let ours = self
                .property_alias_index
                .get(&previous)
                .is_some_and(|(bound_asset, bound_property)| {
                    bound_asset == asset_id && *bound_property == resolved_property_id
                });
            if ours {
                self.property_alias_index.remove(&previous);
            }
        }
        if let Some(alias) = alias {
            // An alias names ONE stream: binding it to a property makes that
            // stream associated, so a disassociated one standing under the same
            // alias is that same stream — it stops being listed apart from the
            // property that now owns it, and the samples it holds become that
            // property's history again, merged through the quality-aware
            // single-value-per-timestamp write.
            //
            // Only a stream of the property's OWN data type is that property's
            // data: AWS requires the two to match to associate a time series with
            // an asset property, and every write path here checks a TQV's variant
            // against the property's `PropertyDataType`, so samples of another type
            // are never carried into a history that may not hold them.
            // A stream whose data type is not this property's is NOT destroyed
            // here: it stays where it is, samples and all. `UpdateAssetProperty`
            // refuses such a request with the modelled refusal before reaching
            // this helper — the same shape as the alias-conflict case above — so
            // the only way to arrive here is a caller whose own rule allows the
            // rebind, and nothing a caller stored is dropped on its behalf.
            let same_type = self
                .disassociated_time_series
                .get(&alias)
                .is_some_and(|series| {
                    series.data_type == property_data_type
                        && series.data_type_spec == property_data_type_spec
                });
            if same_type {
                if let Some(series) = self.disassociated_time_series.remove(&alias) {
                    for value in series.values {
                        self.record_property_value(asset_id, &resolved_property_id, value);
                    }
                }
            }
            // Whoever held this alias loses it, record and index together.
            if let Some((held_by_asset, held_by_property)) =
                self.property_alias_index.insert(
                    alias,
                    (asset_id.to_string(), resolved_property_id.clone()),
                )
            {
                let displaced = held_by_asset != asset_id
                    || held_by_property != resolved_property_id;
                if displaced {
                    if let Some(asset) = self.assets.get_mut(&held_by_asset) {
                        let positions = asset.property_positions(&held_by_property);
                        if !positions.is_empty() {
                            for at in positions {
                                asset.properties[at].alias = None;
                            }
                            asset.last_update_date = now;
                        }
                    }
                }
            }
        }
        true
    }

    /// The `(dataType, dataTypeSpec)` of the disassociated stream `alias` names,
    /// when one stands under it.
    ///
    /// What an operation that binds an alias to an asset property compares with
    /// that property's own data type: a time series is associated with an asset
    /// property only where the two match, so a binding that would mix another
    /// type's samples into the property's history is refused by the operation
    /// rather than resolved by dropping stored data.
    pub fn disassociated_stream_type(&self, alias: &str) -> Option<(&str, Option<&str>)> {
        self.disassociated_time_series.get(alias).map(|series| {
            (
                series.data_type.as_str(),
                series.data_type_spec.as_deref(),
            )
        })
    }

    /// Detach the time series the `(asset_id, property_id)` property's alias
    /// names from that property, and keep the series as a disassociated one.
    ///
    /// The alias leaves the property's own `alias` member and
    /// [`Self::property_alias_index`] through [`Self::set_property_alias`], so
    /// nothing resolves the property by it afterwards, while the stream itself
    /// survives as a [`DisassociatedTimeSeriesRecord`] addressable by that alias —
    /// what IoT SiteWise reports as a `DISASSOCIATED` time series, and what
    /// `DeleteTimeSeries` rather than this call forgets.
    ///
    /// The SAMPLES go with the stream, not with the property: a time series is the
    /// data stream, and an asset property reports the values of the stream it is
    /// associated with, so a property the stream has left reports no history until
    /// a stream is associated with it again. They are not dropped — re-binding the
    /// alias to an asset property brings them back ([`Self::set_property_alias`]).
    /// The detached stream gets its own `TimeSeriesId`, since the property's ids
    /// still name a series of their own.
    ///
    /// `asset_id` is a stored id, not a label: resolve it with
    /// [`Self::resolve_asset_id`] first. `property_id` takes either `CustomID`
    /// form. Answers `false` — writing nothing — when no stored asset holds that
    /// property or the property carries no alias, so there is no association to
    /// break.
    pub fn disassociate_time_series(
        &mut self,
        asset_id: &str,
        property_id: &str,
        now: SystemTime,
    ) -> bool {
        let Some(asset) = self.assets.get(asset_id) else {
            return false;
        };
        let Some(property) = asset.property(property_id) else {
            return false;
        };
        let Some(alias) = property.alias.clone() else {
            return false;
        };
        let resolved_property_id = property.id.clone();
        let record = DisassociatedTimeSeriesRecord {
            alias,
            time_series_id: new_id(),
            data_type: property.data_type.clone(),
            data_type_spec: property.data_type_spec.clone(),
            creation_date: asset.creation_date,
            last_update_date: now,
            values: self
                .property_values
                .get_mut(asset_id)
                .and_then(|properties| properties.remove(&resolved_property_id))
                .unwrap_or_default(),
        };
        self.set_property_alias(asset_id, property_id, None, now);
        self.keep_disassociated_time_series(record);
        true
    }

    /// Store `record` under its alias, bounded at
    /// [`MAX_DISASSOCIATED_TIME_SERIES`] by dropping the least recently updated
    /// series.
    ///
    /// `DeleteTimeSeries` is what a caller uses to forget one, so the eviction is
    /// the bound on the path that has no limit error to refuse with — detaching a
    /// stream from its property, which
    /// `DisassociateTimeSeriesFromAssetProperty` declares none for. The ingest
    /// path, whose operation DOES declare one, refuses past the cap instead and so
    /// never reaches the eviction.
    fn keep_disassociated_time_series(&mut self, record: DisassociatedTimeSeriesRecord) {
        self.disassociated_time_series
            .insert(record.alias.clone(), record);
        while self.disassociated_time_series.len() > MAX_DISASSOCIATED_TIME_SERIES {
            let Some(oldest) = self
                .disassociated_time_series
                .values()
                .min_by_key(|series| series.last_update_date)
                .map(|series| series.alias.clone())
            else {
                break;
            };
            self.disassociated_time_series.remove(&oldest);
        }
    }

    /// Whether ANY stored asset model holds a composite model under `id`.
    ///
    /// A composite model id is globally unique, so both the collision check on a
    /// caller-supplied id and the mint draw ask this one question.
    pub fn composite_model_id_taken(&self, id: &str) -> bool {
        self.asset_model_holding_composite_model(id).is_some()
    }

    /// The stored asset model that holds the composite model `id`, if any does.
    ///
    /// A composite model is addressable only through the asset model that holds
    /// it, and its id is globally unique, so this is how a caller holding only
    /// that id reaches the record it lives on.
    pub fn asset_model_holding_composite_model(&self, id: &str) -> Option<&AssetModelRecord> {
        self.asset_models
            .values()
            .find(|model| model.composite_model(id).is_some())
    }

    /// The property definitions composite model `composite_id` of `model`
    /// contributes: its OWN definitions, or, for a component-model-based
    /// composite model, the properties of the `COMPONENT_MODEL` it reuses.
    ///
    /// A component-model-based composite model stores no properties of its own —
    /// the component model it names provides them, and they are read from that
    /// model's stored record rather than copied at create time, so a later change
    /// to the component model shows through. One definition of "the properties of
    /// a composite model", so every view of one (the model's own, and an asset's
    /// instance of it) answers from the same place.
    pub fn composite_model_properties(
        &self,
        model: &AssetModelRecord,
        composite_id: &str,
    ) -> Vec<AssetModelProperty> {
        match model.composed_asset_model_id(composite_id) {
            Some(composed) => self
                .asset_models
                .get(composed)
                .map(|component| component.properties.clone())
                .unwrap_or_default(),
            None => model
                .composite_model(composite_id)
                .and_then(|composite| composite.properties.clone())
                .unwrap_or_default(),
        }
    }

    /// The property of asset `asset_id` that a `CustomID` label names — a raw
    /// property id, or `externalId:<externalId>` — over EVERY property that asset
    /// carries: the ones materialised from its model's own definitions and the ones
    /// the model's composite models contribute.
    ///
    /// A property a composite model of the asset's model declares is a property OF
    /// THAT ASSET, addressed by the same `(assetId, propertyId)` pair as one
    /// declared on the model directly, so this is the ONE definition of "the
    /// property that pair names" and every operation addressed that way resolves
    /// here. BOTH kinds are materialised onto the asset's own list by
    /// [`Self::materialise_asset_properties`], so the property a listing reports
    /// and the one a data-plane read resolves are the same STORED property, with
    /// the alias and the notification `UpdateAssetProperty` set on it.
    ///
    /// Owned rather than borrowed, so a caller can go on using the store while it
    /// holds the answer.
    pub fn resolve_asset_property(&self, asset_id: &str, label: &str) -> Option<AssetProperty> {
        self.assets.get(asset_id)?.property(label).cloned()
    }

    /// The asset-model property definition an asset property of a model-`model`
    /// asset was materialised from: one of the model's own top-level
    /// definitions, or one a composite model of it contributes.
    ///
    /// The inverse of [`Self::materialise_asset_properties`], asked by every
    /// operation that reports or defaults a member the asset property does not
    /// carry itself (a property's `type`, the `unit` a definition declares), so
    /// a composite model's property answers those from its definition exactly as
    /// a base property does.
    pub fn asset_model_property_definition(
        &self,
        model: &AssetModelRecord,
        property_id: &str,
    ) -> Option<AssetModelProperty> {
        if let Some(found) = model
            .properties
            .iter()
            .find(|property| property.id.as_deref() == Some(property_id))
        {
            return Some(found.clone());
        }
        model
            .composite_models
            .iter()
            .filter_map(|composite| composite.id.as_deref())
            .flat_map(|composite_id| self.composite_model_properties(model, composite_id))
            .find(|property| property.id.as_deref() == Some(property_id))
    }

    /// Every asset property an asset created from `model` carries, in the order
    /// a reader enumerates them, together with the value each one starts its
    /// series with.
    ///
    /// An asset instantiated from an asset model carries an INSTANCE of every
    /// composite model that model holds, so the list is the model's own
    /// top-level property definitions followed by the definitions each composite
    /// model contributes — through [`Self::composite_model_properties`], so a
    /// component-model-based composite model contributes the properties of the
    /// component model it reuses. One FLAT list, so every operation that
    /// addresses an asset property by id reaches a composite model's property
    /// exactly as it reaches a base one; which of the two a property is is read
    /// off the `path` it carries (see [`composite_model_of_property`]).
    ///
    /// Two composite models may reuse the same component model, so one
    /// definition can appear once per composite model that contributes it, each
    /// instance under its own path and all of them sharing the definition's id
    /// — hence the one time series that id addresses
    /// ([`AssetRecord::time_series_properties`]).
    ///
    /// Shared by the create that materialises an asset and the propagation that
    /// re-materialises it, so the two cannot disagree about what an asset of a
    /// model carries.
    pub fn materialise_asset_properties(
        &self,
        model: &AssetModelRecord,
        now: SystemTime,
    ) -> MaterialisedAssetProperties {
        let mut materialised = MaterialisedAssetProperties::default();
        let mut seen: HashSet<String> = HashSet::new();
        for definition in &model.properties {
            materialise_one(definition, None, now, &mut seen, &mut materialised);
        }
        // `composite_models` is the FLAT list of every composite model the asset
        // model holds, at any nesting depth — a nested one is linked to its
        // parent through `composite_model_links` rather than held inside it — so
        // walking it once reaches every composite model's properties, and the
        // path prefix carries the whole chain each sits in.
        for composite in &model.composite_models {
            let Some(composite_id) = composite.id.as_deref() else {
                continue;
            };
            let prefix: Vec<AssetPropertyPathSegment> = model
                .asset_composite_model_path(composite_id)
                .into_iter()
                .map(|segment| AssetPropertyPathSegment {
                    id: segment.id,
                    name: segment.name,
                })
                .collect();
            for definition in self.composite_model_properties(model, composite_id) {
                materialise_one(
                    &definition,
                    Some(&prefix),
                    now,
                    &mut seen,
                    &mut materialised,
                );
            }
        }
        materialised
    }

    /// Append `value` to the `(asset_id, property_id)` series, keeping it sorted
    /// ascending by timestamp.
    ///
    /// The insert itself is [`record_sample`], shared with the write path an
    /// alias-addressed ingest of a disassociated stream takes, so the
    /// quality-aware single-value-per-timestamp rule and the
    /// [`MAX_SAMPLES_PER_SERIES`] bound hold for every stream alike.
    pub fn record_property_value(
        &mut self,
        asset_id: &str,
        property_id: &str,
        value: AssetPropertyValue,
    ) {
        let series = self
            .property_values
            .entry(asset_id.to_string())
            .or_default()
            .entry(property_id.to_string())
            .or_default();
        record_sample(series, value);
    }

    /// Append `value` to the stream `key` names, answering `false` when no stream
    /// does.
    ///
    /// The one write path for a time series, so an alias-addressed ingest reaches
    /// a disassociated stream on exactly the terms
    /// [`Self::record_property_value`] writes an associated one on — the
    /// quality-aware single-value-per-timestamp rule and the
    /// [`MAX_SAMPLES_PER_SERIES`] bound.
    pub fn record_stream_value(&mut self, key: &SeriesKey, value: AssetPropertyValue) -> bool {
        match key {
            SeriesKey::Property(asset_id, property_id) => {
                let asset_id = asset_id.clone();
                let property_id = property_id.clone();
                self.record_property_value(&asset_id, &property_id, value);
                true
            }
            SeriesKey::Disassociated(alias) => {
                match self.disassociated_time_series.get_mut(alias) {
                    Some(series) => {
                        record_sample(&mut series.values, value);
                        true
                    }
                    None => false,
                }
            }
        }
    }

    /// The stream a `propertyAlias` names: the asset property it is associated
    /// with, or the disassociated series standing under it. `None` when no stream
    /// answers to that alias at all.
    ///
    /// An alias names ONE data stream whether or not an asset property is
    /// associated with it — IoT SiteWise reports a series as `ASSOCIATED` or
    /// `DISASSOCIATED` and `DeleteTimeSeries` identifies a disassociated one "if
    /// the time series isn't associated with an asset property, specify the alias
    /// of the time series" — so every operation addressed by an alias resolves it
    /// here and reaches the same stream.
    pub fn resolve_alias_series(&self, alias: &str) -> Option<SeriesKey> {
        if let Some((asset_id, property_id)) = self.property_alias_index.get(alias) {
            return Some(SeriesKey::Property(asset_id.clone(), property_id.clone()));
        }
        self.disassociated_time_series
            .contains_key(alias)
            .then(|| SeriesKey::Disassociated(alias.to_string()))
    }

    /// The samples of the stream `key` names, oldest first. Empty for a stream
    /// nothing has been written to, and for one `key` names no longer.
    pub fn series(&self, key: &SeriesKey) -> &[AssetPropertyValue] {
        match key {
            SeriesKey::Property(asset_id, property_id) => self
                .property_values
                .get(asset_id)
                .and_then(|properties| properties.get(property_id))
                .map_or(&[][..], Vec::as_slice),
            SeriesKey::Disassociated(alias) => self
                .disassociated_time_series
                .get(alias)
                .map_or(&[][..], |series| series.values.as_slice()),
        }
    }

    /// The `PropertyDataType` of the stream `key` names: the declared type of the
    /// asset property that owns it, or the type the disassociated record kept.
    /// `None` when the store holds no such stream.
    ///
    /// The counterpart of [`Self::series`] for a reader that needs the stream's
    /// TYPE rather than its samples: one data stream reports one data type whether
    /// an asset property is associated with it or not, so every reader takes it
    /// from here instead of reaching only into [`Self::assets`]. The property is
    /// resolved through [`Self::resolve_asset_property`], so a stream owned by a
    /// property one of the model's composite models declares reports its type too.
    pub fn series_data_type(&self, key: &SeriesKey) -> Option<String> {
        match key {
            SeriesKey::Property(asset_id, property_id) => self
                .resolve_asset_property(asset_id, property_id)
                .map(|property| property.data_type),
            SeriesKey::Disassociated(alias) => self
                .disassociated_stream_type(alias)
                .map(|(data_type, _)| data_type.to_string()),
        }
    }

    /// The `PropertyAlias` the stream `key` names answers to: the alias carried by
    /// the asset property that owns it, or the alias a disassociated stream is
    /// itself keyed by. `None` when the store holds no such stream, and for a
    /// stream whose owning asset property carries no alias at all.
    ///
    /// The counterpart of [`Self::series_data_type`] for a reader that needs the
    /// stream's ALIAS: one data stream answers to one alias whether an asset
    /// property is associated with it or not, so every reader takes it from here
    /// rather than reaching into [`Self::assets`] or
    /// [`Self::disassociated_time_series`] on its own.
    pub fn series_alias(&self, key: &SeriesKey) -> Option<&str> {
        match key {
            SeriesKey::Property(asset_id, property_id) => self
                .assets
                .get(asset_id)
                .and_then(|asset| asset.property(property_id))
                .and_then(|property| property.alias.as_deref()),
            SeriesKey::Disassociated(alias) => self
                .disassociated_time_series
                .get(alias)
                .map(|series| series.alias.as_str()),
        }
    }

    /// The highest-timestamp value stored for `(asset_id, property_id)`, if any.
    ///
    /// The property's OWN series, borrowing the two ids the way
    /// [`Self::property_values`] is nested for. A caller that wants the stream an
    /// alias names, associated or not, resolves it with
    /// [`Self::resolve_alias_series`] and reads [`Self::series`].
    pub fn latest_property_value(
        &self,
        asset_id: &str,
        property_id: &str,
    ) -> Option<&AssetPropertyValue> {
        self.property_values.get(asset_id)?.get(property_id)?.last()
    }

    /// Create the DISASSOCIATED time series an alias no asset property carries
    /// addresses, answering `false` when a stream already answers to that alias.
    ///
    /// This is what makes alias-addressed ingestion work: IoT SiteWise accepts a
    /// `propertyAlias` entry before any asset property carries that alias and
    /// implicitly creates the data stream it names, which `DescribeTimeSeries` and
    /// `ListTimeSeries` then report and `AssociateTimeSeriesToAssetProperty` binds.
    /// `data_type` is the type of the value that created it — the stream has no
    /// asset property to take one from — and a later value of another type does not
    /// change it.
    ///
    /// The caller checks [`MAX_DISASSOCIATED_TIME_SERIES`] first and refuses past
    /// it; the insert itself goes through the same bounded keeper the detach path
    /// uses, so the collection cannot grow past the cap either way.
    pub fn create_disassociated_time_series(
        &mut self,
        alias: &str,
        data_type: &str,
        now: SystemTime,
    ) -> bool {
        if self.resolve_alias_series(alias).is_some() {
            return false;
        }
        self.keep_disassociated_time_series(DisassociatedTimeSeriesRecord {
            alias: alias.to_string(),
            time_series_id: new_id(),
            data_type: data_type.to_string(),
            data_type_spec: None,
            creation_date: now,
            last_update_date: now,
            values: Vec::new(),
        });
        true
    }

    /// Forget the disassociated stream `alias` names, answering the record that was
    /// removed.
    ///
    /// What `DeleteTimeSeries` does to a stream no asset property owns: the stream
    /// goes, samples and all, and the alias names nothing afterwards.
    pub fn forget_disassociated_time_series(
        &mut self,
        alias: &str,
    ) -> Option<DisassociatedTimeSeriesRecord> {
        self.disassociated_time_series.remove(alias)
    }

    /// Forget the series an asset property owns: its alias binding and its stored
    /// samples, leaving the property itself in place.
    ///
    /// What `DeleteTimeSeries` does to an ASSOCIATED stream — "If you delete a time
    /// series that's associated with an asset property, the asset property still
    /// exists, but the time series will no longer be associated with this asset
    /// property" — so a series removed by its alias and the same series removed by
    /// its `(assetId, propertyId)` pair leave the store in one state.
    ///
    /// Dropping the binding is an accepted write to the asset that carries the
    /// property, so `now` is the instant its `last_update_date` moves to (see
    /// [`Self::set_property_alias`]).
    pub fn forget_property_series(&mut self, asset_id: &str, property_id: &str, now: SystemTime) {
        self.set_property_alias(asset_id, property_id, None, now);
        if let Some(properties) = self.property_values.get_mut(asset_id) {
            properties.remove(property_id);
        }
    }

    /// Store `segment` on the dataset `dataset_id`, answering `false` — writing
    /// nothing — when the store cannot hold it: a bound of its span is not an
    /// instant the modelled `TimeInNanos` can express, the span covers no time
    /// (its start is not before its end), or the dataset already holds
    /// [`MAX_DATA_SEGMENTS_PER_DATASET`] segments.
    ///
    /// This is the PRODUCER-side entry point — the one writer of a dataset's
    /// segments, which `BatchDeleteDatasetDataSegments` then removes from:
    /// `BatchAssociateDataSegmentsToDataset` stores every segment it accepts
    /// here, [`Self::ingest_data_segments`] stores the ones a bulk import
    /// brings into a session dataset, and a test seeding a dataset directly
    /// reaches the same writer, so everything a read or a deletion can find was
    /// written here and nowhere else.
    ///
    /// The log stays sorted by [`data_segment_key`] and single-valued on it:
    /// storing the same series' same span again REPLACES the entry held for it
    /// rather than holding the segment twice.
    pub fn put_dataset_data_segment(
        &mut self,
        dataset_id: &str,
        segment: DataSegmentRecord,
    ) -> bool {
        if !is_modelled_time(&segment.start_timestamp)
            || !is_modelled_time(&segment.end_timestamp)
        {
            return false;
        }
        if timestamp_key(&segment.start_timestamp) >= timestamp_key(&segment.end_timestamp) {
            return false;
        }
        let held = self.dataset_data_segments.entry(dataset_id.to_string()).or_default();
        let key = data_segment_key(&segment);
        match held.binary_search_by(|stored| data_segment_key(stored).cmp(&key)) {
            Ok(at) => held[at] = segment,
            Err(at) => {
                // Past the cap the write is REFUSED rather than evicting a
                // segment: a deletion addresses a segment by its own span, so a
                // silently dropped one would read as a segment that never
                // existed. The cap is at least one, so a dataset reaching it
                // already held segments and no empty entry is left behind here.
                if held.len() >= MAX_DATA_SEGMENTS_PER_DATASET {
                    return false;
                }
                held.insert(at, segment);
            }
        }
        true
    }

    /// Record the data segments a bulk import into the SESSION dataset
    /// `dataset_id` brings in from `files`, answering how many it stored.
    ///
    /// The PRODUCER of a session dataset's own segments. A session dataset
    /// "contains data segments of time series data" (the `CreateDataset` model
    /// doc) and an import names it as "the session dataset to ingest data into"
    /// (the `CreateBulkImportJob` doc), so an accepted import is what puts
    /// segments on one: each file names the time series its `alias` identifies
    /// ("the alias associated with the file's time series") and the instant its
    /// data begins ("the nanosecond-precision start time for the file data"),
    /// which is the span of that stream the dataset comes to hold.
    ///
    /// The files themselves are in Amazon S3, which this emulator does not
    /// reach, so nothing is read from them and NO sample is invented (see
    /// [`BulkImportJobRecord`]): the data a segment covers is the data this
    /// store really holds for the stream the file names. A file naming no
    /// alias, an alias no stored stream answers to, or an instant past every
    /// sample that stream holds therefore brings in nothing and records no
    /// segment.
    ///
    /// A span overlapping a segment this store already holds of that series
    /// records nothing either — "Data segments that belong to the same time
    /// series can't overlap in time, regardless of which dataset they belong
    /// to" (the `BatchAssociateDataSegmentsToDataset` doc) — which is also what
    /// makes a second import of the same file add no second segment.
    pub fn ingest_data_segments(&mut self, dataset_id: &str, files: &[File]) -> usize {
        let mut stored = 0usize;
        for file in files {
            let Some(segment) = self.imported_data_segment(dataset_id, file) else {
                continue;
            };
            if self
                .overlapping_data_segment(
                    &segment.time_series_id,
                    &segment.start_timestamp,
                    &segment.end_timestamp,
                )
                .is_some()
            {
                continue;
            }
            if self.put_dataset_data_segment(dataset_id, segment) {
                stored += 1;
            }
        }
        stored
    }

    /// The segment the import of `file` into `dataset_id` brings in, or `None`
    /// when it brings in nothing at all; see [`Self::ingest_data_segments`] for
    /// the rule.
    ///
    /// The span is HALF-OPEN, as every segment's span is, so it ends ONE
    /// nanosecond past the last sample it covers — the smallest instant the
    /// modelled `TimeInNanos` can express past it, so the segment covers that
    /// sample and no further data. Its start is the instant the file states, and
    /// the first covered sample's own instant for a file that states none.
    fn imported_data_segment(&self, dataset_id: &str, file: &File) -> Option<DataSegmentRecord> {
        let stream = self.resolve_alias_series(file.alias.as_deref()?)?;
        let time_series_id = self.series_time_series_id(&stream)?;
        let samples = self.series(&stream);
        let from = file.start_time.as_ref().map(timestamp_key);
        // The samples are kept sorted ascending, so the first one inside the
        // window is the one the span starts at and the series' last sample is
        // the one it ends after.
        let first = samples
            .iter()
            .map(|sample| &sample.timestamp)
            .find(|at| from.is_none_or(|from| timestamp_key(at) >= from))?;
        let start = match &file.start_time {
            Some(start) => start.clone(),
            None => first.clone(),
        };
        let end = next_nanosecond(&samples.last()?.timestamp)?;
        Some(DataSegmentRecord {
            // A session dataset's own ingested data came from the dataset that
            // holds it, which is what tells it apart from a segment another
            // dataset's data was selected into.
            source_dataset_id: dataset_id.to_string(),
            time_series_id,
            start_timestamp: start,
            end_timestamp: end,
        })
    }

    /// Every data segment of `time_series_id` this store holds whose span
    /// shares an instant with the half-open span `[start, end)`, paired with
    /// the dataset holding it.
    ///
    /// "Data segments that belong to the same time series can't overlap in
    /// time, regardless of which dataset they belong to" (the
    /// `BatchAssociateDataSegmentsToDataset` model doc), so the search spans
    /// EVERY dataset's segments rather than the one a request names.
    fn overlapping_data_segments<'a, 'k>(
        &'a self,
        time_series_id: &'k str,
        start: &TimeInNanos,
        end: &TimeInNanos,
    ) -> impl Iterator<Item = (&'a str, &'a DataSegmentRecord)> + 'k
    where
        'a: 'k,
    {
        let (start, end) = (timestamp_key(start), timestamp_key(end));
        self.dataset_data_segments
            .iter()
            .flat_map(|(dataset_id, held)| {
                held.iter()
                    .map(move |segment| (dataset_id.as_str(), segment))
            })
            .filter(move |(_, segment)| {
                segment.time_series_id == time_series_id
                    // Two half-open spans overlap exactly when each starts
                    // before the other ends, so spans that merely touch at one
                    // instant — the end of one, the start of the next — do not.
                    && timestamp_key(&segment.start_timestamp) < end
                    && timestamp_key(&segment.end_timestamp) > start
            })
    }

    /// The data segment of `time_series_id` ANY dataset holds whose span shares
    /// an instant with the half-open span `[start, end)`: the dataset that
    /// holds it, paired with the segment itself, or `None` when none does.
    ///
    /// What a PRODUCER of a segment reads the no-overlap rule through — a
    /// session dataset's own ingested data included, which is why this one asks
    /// about every segment and [`Self::overlapping_data_segment_association`]
    /// asks only about the associations. Answered in dataset-then-span order,
    /// so one store answers one way whatever order its map iterates in.
    pub fn overlapping_data_segment(
        &self,
        time_series_id: &str,
        start: &TimeInNanos,
        end: &TimeInNanos,
    ) -> Option<(&str, &DataSegmentRecord)> {
        self.overlapping_data_segments(time_series_id, start, end)
            .min_by(|(left_id, left), (right_id, right)| {
                (*left_id, data_segment_key(left)).cmp(&(*right_id, data_segment_key(right)))
            })
    }

    /// The ASSOCIATION of `time_series_id` whose span shares an instant with
    /// the half-open span `[start, end)`: the dataset that holds it, paired with
    /// the segment itself, or `None` when no dataset holds one.
    ///
    /// The no-overlap rule [`Self::overlapping_data_segment`] reads, narrowed to
    /// the segments an association put there: a segment whose data came from the
    /// dataset that HOLDS it is a source dataset's own data — what an
    /// association selects a slice of (see
    /// [`DataSegmentRecord::source_dataset_id`]) — rather than an association
    /// of it, so it is not one of the segments this rule compares.
    ///
    /// Answered in dataset-then-span order, so one store answers one way
    /// whatever order its map iterates in.
    pub fn overlapping_data_segment_association(
        &self,
        time_series_id: &str,
        start: &TimeInNanos,
        end: &TimeInNanos,
    ) -> Option<(&str, &DataSegmentRecord)> {
        self.overlapping_data_segments(time_series_id, start, end)
            .filter(|(dataset_id, segment)| segment.source_dataset_id != *dataset_id)
            .min_by(|(left_id, left), (right_id, right)| {
                (*left_id, data_segment_key(left)).cmp(&(*right_id, data_segment_key(right)))
            })
    }

    /// A data segment of `time_series_id` any dataset of this store holds,
    /// paired with the dataset holding it, or `None` when none does.
    ///
    /// The dependency `DeleteTimeSeries` refuses to break: "You can't delete a
    /// time series until all of its data segments have been deleted from session
    /// datasets" (its model doc). EVERY segment naming the series counts,
    /// whichever dataset holds it and whichever dataset its data came from
    /// ([`DataSegmentRecord::source_dataset_id`]) — a session dataset's own
    /// segment covers that stream's data and a curated dataset's association
    /// covers the same data from elsewhere, so either one left behind would
    /// name a stream the store no longer holds. That is the wider question
    /// [`Self::overlapping_data_segment_association`] does not ask: it compares
    /// one span against the ASSOCIATIONS of a series, while this asks whether
    /// anything at all still covers it.
    ///
    /// Answered in dataset-then-span order, so one store answers one way
    /// whatever order its map iterates in.
    pub fn data_segment_of_time_series(
        &self,
        time_series_id: &str,
    ) -> Option<(&str, &DataSegmentRecord)> {
        self.dataset_data_segments
            .iter()
            .flat_map(|(holder, held)| {
                held.iter().map(move |segment| (holder.as_str(), segment))
            })
            .filter(|(_, segment)| segment.time_series_id == time_series_id)
            .min_by(|(left_id, left), (right_id, right)| {
                (*left_id, data_segment_key(left)).cmp(&(*right_id, data_segment_key(right)))
            })
    }

    /// The stream among `time_series_ids` a dataset still holds a data segment
    /// of, as `(timeSeriesId, datasetId)`, or `None` when no dataset holds a
    /// segment of any of them.
    ///
    /// [`Self::data_segment_of_time_series`] asked of a GROUP of streams: a
    /// call that would drop SEVERAL streams at once — every property stream of
    /// an asset it removes, the streams of the properties a model update takes
    /// away — breaks the same dependency as a call that drops one, so it is
    /// refused by the same question rather than by a rule of its own. Answered
    /// in stream-then-dataset order, so one store answers one way whatever
    /// order its maps iterate in and whatever order the caller offers the
    /// streams in.
    ///
    /// One pass over the segment index, like the single-stream question: the
    /// streams offered become a set and each segment is tested against it once,
    /// so asking about many streams costs the same walk as asking about one
    /// rather than one walk per stream.
    pub fn data_segment_of_any_time_series<I>(
        &self,
        time_series_ids: I,
    ) -> Option<(String, String)>
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        let covered: HashSet<String> = time_series_ids
            .into_iter()
            .map(|id| id.as_ref().to_string())
            .collect();
        if covered.is_empty() {
            return None;
        }
        self.blocking_data_segment(|time_series_id| covered.contains(time_series_id))
    }

    /// The first segment, in stream-then-dataset order, whose covered stream
    /// `covers` accepts, as `(timeSeriesId, datasetId)`.
    ///
    /// The single walk of [`Self::dataset_data_segments`] every "is this stream
    /// still depended on" question takes, so each of them costs one pass over
    /// the index whatever shape its set of streams has.
    fn blocking_data_segment(
        &self,
        covers: impl Fn(&str) -> bool,
    ) -> Option<(String, String)> {
        let mut blocked: Option<(&str, &str)> = None;
        for (holder, held) in &self.dataset_data_segments {
            for segment in held {
                if !covers(&segment.time_series_id) {
                    continue;
                }
                let candidate = (segment.time_series_id.as_str(), holder.as_str());
                if blocked.is_none_or(|held| candidate < held) {
                    blocked = Some(candidate);
                }
            }
        }
        blocked.map(|(series, holder)| (series.to_string(), holder.to_string()))
    }

    /// The `TimeSeriesId` of every stream asset `asset_id` owns: one per
    /// property it carries, which is what the `(assetId, propertyId)` pair
    /// names ([`time_series_id`]).
    ///
    /// The streams a call that removes the asset would drop, so the streams it
    /// has to ask [`Self::data_segment_of_any_time_series`] about. Empty for an
    /// asset this store does not hold, and derived from the properties rather
    /// than from the samples written to them: a segment names the pair, which
    /// resolves through the property, not through the data behind it.
    pub fn asset_property_time_series_ids(&self, asset_id: &str) -> Vec<String> {
        self.assets
            .get(asset_id)
            .map(|asset| {
                asset
                    .properties
                    .iter()
                    .map(|property| time_series_id(asset_id, &property.id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The stream a dataset still holds a data segment of among the streams the
    /// assets of `asset_model_id` would lose were that model's properties to
    /// become `kept`, as `(timeSeriesId, datasetId)`.
    ///
    /// The same dependency, asked of the removal a model update propagates: "If
    /// you remove a property from an asset model, IoT SiteWise deletes all
    /// previous data for that property" (`UpdateAssetModel`'s doc), so a
    /// property the new definition drops takes the stream of every asset of
    /// that model with it — and a stream a dataset still holds a segment of
    /// cannot go. `None` when the new definition drops no property, when no
    /// asset was created from the model, or when nothing holds a segment of the
    /// streams it would drop; asked BEFORE the new definition is written, which
    /// is why the kept properties are passed in rather than read back.
    ///
    /// Answered from the SEGMENTS rather than from the streams, in the one pass
    /// [`Self::blocking_data_segment`] makes: a segment names a stream by the
    /// pair that derives it, so each one is resolved back to its asset and
    /// property and tested against the dropped set, which never materialises
    /// the product of the model's assets and its dropped properties.
    ///
    /// Every asset the propagation reaches counts, not only those created
    /// directly from this model: a composite model that reuses this one as a
    /// component model contributes its definitions, under their own ids, to the
    /// assets of the model holding it, so a property dropped here takes those
    /// assets' streams too — the same set of models
    /// [`Self::models_inheriting_definitions_of`] walks.
    pub fn data_segment_of_dropped_model_properties(
        &self,
        asset_model_id: &str,
        kept: &[AssetModelProperty],
    ) -> Option<(String, String)> {
        let dropped: HashSet<&str> = self
            .asset_models
            .get(asset_model_id)?
            .properties
            .iter()
            .filter_map(|property| property.id.as_deref())
            .filter(|id| !id.is_empty())
            .filter(|id| !kept.iter().any(|kept| kept.id.as_deref() == Some(*id)))
            .collect();
        if dropped.is_empty() {
            return None;
        }
        let inheriting = self.models_inheriting_definitions_of(asset_model_id);
        self.blocking_data_segment(|time_series_id| {
            let Some((asset_id, property_id)) =
                time_series_id.split_once(TIME_SERIES_ID_SEPARATOR)
            else {
                return false;
            };
            if !dropped.contains(property_id) {
                return false;
            }
            // The stream goes only if the asset really carries that property and
            // its own model inherits the definition being dropped — which is
            // exactly when the propagation re-materialises the asset without it.
            self.assets.get(asset_id).is_some_and(|asset| {
                inheriting.contains(&asset.asset_model_id)
                    && asset.property(property_id).is_some()
            })
        })
    }

    /// Forget the segment of `dataset_id` that `time_series_id` and the span
    /// `[start, end)` name EXACTLY, answering whether one was held.
    ///
    /// What `BatchDeleteDatasetDataSegments` does to one entry: a segment is
    /// addressed by its time series and its own span, so a span no segment of
    /// the dataset covers exactly removes nothing and the caller reports that
    /// entry as the absence it is.
    pub fn delete_dataset_data_segment(
        &mut self,
        dataset_id: &str,
        time_series_id: &str,
        start: &TimeInNanos,
        end: &TimeInNanos,
    ) -> bool {
        let Some(held) = self.dataset_data_segments.get(dataset_id) else {
            return false;
        };
        let Some(at) = data_segment_at(held, time_series_id, start, end) else {
            return false;
        };
        self.forget_dataset_data_segment_at(dataset_id, at);
        true
    }

    /// Forget the segment of `dataset_id` that `source_dataset_id`,
    /// `time_series_id` and the span `[start, end)` name EXACTLY, answering
    /// whether one was held.
    ///
    /// What `BatchDisassociateDataSegmentsFromDataset` does to one entry: the
    /// time series and the span are the segment's identity inside the dataset
    /// that holds it, and the source names the dataset the segment's data came
    /// from, so a segment the dataset holds under a different source is not the
    /// one addressed and nothing is removed. Only the association goes: the
    /// data the segment covered belongs to the source dataset and is untouched
    /// here, which is what separates this from
    /// [`Self::delete_dataset_data_segment`], whose caller deletes that data
    /// too.
    pub fn disassociate_dataset_data_segment(
        &mut self,
        dataset_id: &str,
        source_dataset_id: &str,
        time_series_id: &str,
        start: &TimeInNanos,
        end: &TimeInNanos,
    ) -> bool {
        let Some(held) = self.dataset_data_segments.get(dataset_id) else {
            return false;
        };
        let Some(at) = data_segment_at(held, time_series_id, start, end) else {
            return false;
        };
        if held[at].source_dataset_id != source_dataset_id {
            return false;
        }
        self.forget_dataset_data_segment_at(dataset_id, at);
        true
    }

    /// Drop the segment `dataset_id` holds at `at` in its sorted log — the one
    /// remover both paths above go through, so a dataset emptied by either is
    /// left in the same shape.
    fn forget_dataset_data_segment_at(&mut self, dataset_id: &str, at: usize) {
        let Some(held) = self.dataset_data_segments.get_mut(dataset_id) else {
            return;
        };
        held.remove(at);
        // A dataset that holds no segments holds no entry either, so the map does
        // not grow an empty vector per dataset a removal has emptied.
        if held.is_empty() {
            self.dataset_data_segments.remove(dataset_id);
        }
    }

    /// The dataset that holds a data segment whose data came FROM `dataset_id`,
    /// paired with that segment, or `None` when nothing references its data.
    ///
    /// A segment names the dataset its data came from
    /// ([`DataSegmentRecord::source_dataset_id`]), so a segment ANOTHER dataset
    /// holds under this one references this dataset's data — the reference
    /// `DeleteDataset` refuses to break: "You can't delete a session dataset
    /// while a curated dataset references its data segments." A segment whose
    /// data came from the dataset that HOLDS it is that dataset's own data and
    /// references nothing, read exactly as
    /// [`Self::overlapping_data_segment_association`] reads it.
    ///
    /// Answered in dataset-then-span order, so one store answers one way
    /// whatever order its map iterates in.
    pub fn dataset_data_reference(&self, dataset_id: &str) -> Option<(&str, &DataSegmentRecord)> {
        self.dataset_data_segments
            .iter()
            .flat_map(|(holder, held)| {
                held.iter().map(move |segment| (holder.as_str(), segment))
            })
            .filter(|(holder, segment)| {
                segment.source_dataset_id == dataset_id && *holder != dataset_id
            })
            .min_by(|(left_id, left), (right_id, right)| {
                (*left_id, data_segment_key(left)).cmp(&(*right_id, data_segment_key(right)))
            })
    }

    /// Forget a dataset and the data segments it held, answering the record it
    /// removed.
    ///
    /// A segment whose data came from the dataset that HELD it is that dataset's
    /// OWN data, so the samples its span covers go with it — "Deleting a session
    /// dataset also deletes the underlying time series data in the session" —
    /// through [`Self::forget_series_window`], the one window remover a single
    /// segment's deletion already goes through. A segment whose source is
    /// ANOTHER dataset only REFERENCES that dataset's data, so just the
    /// reference goes: "Deleting a curated dataset doesn't delete the underlying
    /// data in the source session datasets", the same split
    /// [`Self::disassociate_dataset_data_segment`] makes for one segment.
    ///
    /// The dataset's tags go with its record, which owns them. A dataset whose
    /// data another dataset still references is NOT refused here:
    /// [`Self::dataset_data_reference`] is what the caller refuses it with.
    ///
    /// A bulk import job that was created to ingest INTO this dataset goes too:
    /// its destination is the dataset, this service models no call that deletes
    /// a job on its own, and a job the store kept would report a `datasetId`
    /// this store no longer holds — the same reason
    /// [`Self::forget_workspace`] takes the jobs of a workspace with it.
    pub fn forget_dataset(&mut self, dataset_id: &str) -> Option<DatasetRecord> {
        let record = self.datasets.remove(dataset_id)?;
        self.bulk_import_jobs
            .retain(|_, job| job.dataset_id.as_deref() != Some(dataset_id));
        let held = self
            .dataset_data_segments
            .remove(dataset_id)
            .unwrap_or_default();
        for segment in held {
            if segment.source_dataset_id != dataset_id {
                continue;
            }
            // A segment whose time series the store no longer holds has no data
            // left to delete, which is no reason to keep the dataset.
            if let Some(stream) = self.series_by_time_series_id(&segment.time_series_id) {
                self.forget_series_window(
                    &stream,
                    &segment.start_timestamp,
                    &segment.end_timestamp,
                );
            }
        }
        Some(record)
    }

    /// The stored data stream whose own `TimeSeriesId` is `time_series_id`, or
    /// `None` when no stream of this store carries it.
    ///
    /// The inverse of [`time_series_id`]: a stream an asset property owns is
    /// named by its `(assetId, propertyId)` pair joined by
    /// [`TIME_SERIES_ID_SEPARATOR`], and a stream no property owns carries a
    /// `TimeSeriesId` of its own, minted when the stream appeared. One resolver,
    /// so an operation that addresses a time series by that id reaches the same
    /// stream the alias and the id-pair forms reach.
    pub fn series_by_time_series_id(&self, time_series_id: &str) -> Option<SeriesKey> {
        if let Some((asset_id, property_id)) = time_series_id.split_once(TIME_SERIES_ID_SEPARATOR)
        {
            return self
                .assets
                .get(asset_id)
                .filter(|asset| asset.property(property_id).is_some())
                .map(|_| SeriesKey::Property(asset_id.to_string(), property_id.to_string()));
        }
        self.disassociated_time_series
            .values()
            .find(|series| series.time_series_id == time_series_id)
            .map(|series| SeriesKey::Disassociated(series.alias.clone()))
    }

    /// The `TimeSeriesId` of the stream `key` names, or `None` when this store
    /// holds no such stream.
    ///
    /// The inverse of [`Self::series_by_time_series_id`], through the same two
    /// identities: the `(assetId, propertyId)` pair of a stream an asset
    /// property owns ([`time_series_id`]), and the id minted for a stream no
    /// property owns. One derivation, so a producer that records a stream's id
    /// and a reader that resolves it back cannot disagree about which stream it
    /// names.
    pub fn series_time_series_id(&self, key: &SeriesKey) -> Option<String> {
        match key {
            SeriesKey::Property(asset_id, property_id) => self
                .assets
                .get(asset_id)
                .filter(|asset| asset.property(property_id).is_some())
                .map(|_| time_series_id(asset_id, property_id)),
            SeriesKey::Disassociated(alias) => self
                .disassociated_time_series
                .get(alias)
                .map(|series| series.time_series_id.clone()),
        }
    }

    /// Forget every sample of the stream `key` names whose timestamp falls inside
    /// the HALF-OPEN window `[start, end)`, answering how many went.
    ///
    /// What "deleting a data segment deletes the underlying time series data for
    /// the segment's time range" (the `BatchDeleteDatasetDataSegments` model doc)
    /// does to the stream a segment covers: the samples inside that segment's own
    /// span go and every other sample of the stream stays, so the data of a
    /// neighbouring segment survives the deletion of this one. Answers 0 for a
    /// stream `key` names no longer, and leaves the series sorted ascending.
    pub fn forget_series_window(
        &mut self,
        key: &SeriesKey,
        start: &TimeInNanos,
        end: &TimeInNanos,
    ) -> usize {
        let series = match key {
            SeriesKey::Property(asset_id, property_id) => self
                .property_values
                .get_mut(asset_id)
                .and_then(|properties| properties.get_mut(property_id)),
            SeriesKey::Disassociated(alias) => self
                .disassociated_time_series
                .get_mut(alias)
                .map(|series| &mut series.values),
        };
        let Some(series) = series else {
            return 0;
        };
        let (start, end) = (timestamp_key(start), timestamp_key(end));
        let before = series.len();
        series.retain(|sample| {
            let at = timestamp_key(&sample.timestamp);
            at < start || at >= end
        });
        before - series.len()
    }

    /// The search `workspace` holds under `search_id`, or `None` when that
    /// workspace holds none: a search is reachable only through the workspace it
    /// was started in.
    pub fn search(&self, workspace_name: &str, search_id: &str) -> Option<&SearchRecord> {
        self.searches.get(&search_key(workspace_name, search_id))
    }

    /// The dashboard a create carrying `client_token` already minted, if any.
    ///
    /// What makes `CreateDashboard` idempotent on its token: a retry of a create
    /// that carried one answers from the dashboard that create minted. Not
    /// scoped to a project — a `dashboardId` identifies a dashboard across every
    /// project, so the token of the create that minted one is enough to
    /// recognise its retry.
    pub fn dashboard_for_client_token(&self, client_token: &str) -> Option<&DashboardRecord> {
        self.dashboards
            .values()
            .find(|record| record.matches_client_token(client_token))
    }

    /// How many searches `workspace` holds — what the per-workspace bound reads.
    pub fn searches_in_workspace(&self, workspace_name: &str) -> usize {
        self.searches
            .keys()
            .filter(|(workspace, _)| workspace == workspace_name)
            .count()
    }

    /// How many enrichment jobs `workspace_name` holds — what the per-workspace
    /// bound reads.
    pub fn enrichment_jobs_in_workspace(&self, workspace_name: &str) -> usize {
        self.enrichment_jobs
            .keys()
            .filter(|(workspace, _)| workspace == workspace_name)
            .count()
    }

    /// The enrichment job of `workspace_name` that is doing the same work as
    /// `configuration` and has not reached a terminal status at `now`, if any.
    ///
    /// What the duplicate-job rule reads: a create is refused while "a duplicate
    /// job (same workspace, dataset, property, and job type) is currently
    /// running", so two jobs are compared on the work they do
    /// ([`enrichment_job_target`]) rather than on the whole request.
    pub fn active_enrichment_job(
        &self,
        workspace_name: &str,
        configuration: &EnrichmentJobConfiguration,
        now: SystemTime,
    ) -> Option<&EnrichmentJobRecord> {
        let wanted = enrichment_job_target(configuration);
        self.enrichment_jobs.values().find(|record| {
            record.workspace_name == workspace_name
                && enrichment_job_target(&record.configuration) == wanted
                && record.is_active(now)
        })
    }

    /// Store `segment` on the capture source `(workspace_name, time_series_id)`
    /// names, creating the source on first write.
    ///
    /// This is the PRODUCER-side entry point — the one writer of a workspace's
    /// video, which `GetCaptureData` then reads. The producer paths themselves
    /// (video an edge gateway publishes for a capture source, and the bulk import
    /// of `mp4` files `CreateBulkImportJob` takes) are not implemented in this
    /// emulator yet, so a test seeding a capture source is today's only caller;
    /// everything a read of that video must answer from is written here and
    /// nowhere else.
    ///
    /// `alias` BINDS that alias to the source, so the alias form of a lookup
    /// reaches the same record; an alias another source of the same workspace
    /// already carries moves to this one, record and index together, exactly as a
    /// rebound property alias does.
    ///
    /// The segment log stays sorted ascending by start time and single-valued at
    /// a start time — re-ingesting the span replaces the bytes stored for it —
    /// and is bounded at [`MAX_CAPTURE_SEGMENTS_PER_SOURCE`] by dropping the
    /// oldest segment, the same eviction a time series' samples get.
    ///
    /// The `Err` names WHICH rule refused the write ([`CaptureWriteRefusal`]), so
    /// the producer handlers this serves can map one refusal to one modelled
    /// error instead of re-deriving the checks — and nothing is written on any of
    /// them.
    ///
    /// `now` is the instant the write lands: the source's creation date for the
    /// write that creates it, and its last-update date for every write it
    /// accepts, which is what the time series it is reports
    /// ([`CaptureSourceRecord::time_series_summary`]).
    pub fn put_capture_segment(
        &mut self,
        workspace_name: &str,
        time_series_id: &str,
        alias: Option<&str>,
        data_type: &str,
        segment: CaptureSegment,
        now: SystemTime,
    ) -> Result<(), CaptureWriteRefusal> {
        if !self.workspaces.contains_key(workspace_name) {
            return Err(CaptureWriteRefusal::NoSuchWorkspace);
        }
        let id_length = time_series_id.chars().count();
        if !(MIN_TIME_SERIES_ID_LENGTH..=MAX_TIME_SERIES_ID_LENGTH).contains(&id_length) {
            return Err(CaptureWriteRefusal::TimeSeriesIdLength);
        }
        if let Some(alias) = alias {
            let alias_length = alias.chars().count();
            if !(MIN_PROPERTY_ALIAS_LENGTH..=MAX_PROPERTY_ALIAS_LENGTH).contains(&alias_length) {
                return Err(CaptureWriteRefusal::AliasLength);
            }
        }
        if !VIDEO_DATA_TYPE_VALUES.contains(&data_type) {
            return Err(CaptureWriteRefusal::UnmodelledDataType);
        }
        if !is_modelled_time(&segment.start_time) || !is_modelled_time(&segment.end_time) {
            return Err(CaptureWriteRefusal::UnmodelledInstant);
        }
        if timestamp_key(&segment.start_time) >= timestamp_key(&segment.end_time) {
            return Err(CaptureWriteRefusal::EmptySpan);
        }
        // The data type is the media type the source's video is in: it is
        // established by the write that creates the source, so a later write
        // naming another type is refused rather than accepted and ignored — a
        // producer is never told it stored media of a type a read will not
        // report.
        if let Some(source) = self.capture_source_by_id(workspace_name, time_series_id) {
            if source.data_type != data_type {
                return Err(CaptureWriteRefusal::DataTypeMismatch);
            }
        } else if self.capture_source_count() >= MAX_CAPTURE_SOURCES {
            return Err(CaptureWriteRefusal::SourceLimit);
        }

        let source = self
            .capture_sources
            .entry(workspace_name.to_string())
            .or_default()
            .entry(time_series_id.to_string())
            .or_insert_with(|| CaptureSourceRecord {
                workspace_name: workspace_name.to_string(),
                time_series_id: time_series_id.to_string(),
                alias: None,
                data_type: data_type.to_string(),
                creation_date: now,
                last_update_date: now,
                segments: Vec::new(),
            });
        // An accepted write is a write to the series, so its last-update instant
        // moves to this call's — the creation instant stays the one the write
        // that created it stamped.
        source.last_update_date = now;

        // A source's `alias` and the alias index move together: the alias the
        // source carried before is dropped from the index below, so nothing
        // resolves the source by it afterwards.
        let previous_alias = match alias {
            Some(alias) if source.alias.as_deref() != Some(alias) => {
                source.alias.replace(alias.to_string())
            }
            _ => None,
        };

        let at = source.segments.binary_search_by(|stored| {
            timestamp_key(&stored.start_time).cmp(&timestamp_key(&segment.start_time))
        });
        match at {
            Ok(at) => source.segments[at] = segment,
            Err(at) => source.segments.insert(at, segment),
        }
        if source.segments.len() > MAX_CAPTURE_SEGMENTS_PER_SOURCE {
            let excess = source.segments.len() - MAX_CAPTURE_SEGMENTS_PER_SOURCE;
            source.segments.drain(..excess);
        }

        if let Some(alias) = alias {
            let aliases = self
                .capture_source_aliases
                .entry(workspace_name.to_string())
                .or_default();
            if let Some(previous) = previous_alias {
                aliases.remove(&previous);
            }
            // Whoever held this alias in this workspace loses it, record and
            // index together, so no two stored sources can report one alias
            // while the index names only the newer.
            let displaced = aliases.insert(alias.to_string(), time_series_id.to_string());
            if let Some(displaced) = displaced.filter(|held| held != time_series_id) {
                if let Some(source) = self
                    .capture_sources
                    .get_mut(workspace_name)
                    .and_then(|sources| sources.get_mut(&displaced))
                {
                    source.alias = None;
                }
            }
        }
        Ok(())
    }

    /// How many capture sources this store holds, across every workspace — what
    /// [`MAX_CAPTURE_SOURCES`] bounds.
    pub fn capture_source_count(&self) -> usize {
        self.capture_sources
            .values()
            .map(::std::collections::HashMap::len)
            .sum()
    }

    /// The capture source `workspace_name` holds under `time_series_id`, or
    /// `None` when it holds none — which a read of it answers as
    /// `ResourceNotFoundException`.
    pub fn capture_source_by_id(
        &self,
        workspace_name: &str,
        time_series_id: &str,
    ) -> Option<&CaptureSourceRecord> {
        self.capture_sources.get(workspace_name)?.get(time_series_id)
    }

    /// The capture source `workspace_name` holds under the `propertyAlias`
    /// `alias`. The alias index is what makes the two identifying forms resolve
    /// to one source.
    pub fn capture_source_by_alias(
        &self,
        workspace_name: &str,
        alias: &str,
    ) -> Option<&CaptureSourceRecord> {
        let time_series_id = self.capture_source_aliases.get(workspace_name)?.get(alias)?;
        self.capture_source_by_id(workspace_name, time_series_id)
    }

    /// Every time series `workspace_name` holds, in no particular order — what a
    /// listing scoped to that workspace enumerates.
    ///
    /// A workspace's time series ARE the capture sources stored under it: the
    /// streams ingestion recorded a workspace for, which `GetCaptureData` reads
    /// back by the same two identifiers a summary of one reports.
    pub fn workspace_time_series(
        &self,
        workspace_name: &str,
    ) -> impl Iterator<Item = &CaptureSourceRecord> {
        self.capture_sources
            .get(workspace_name)
            .into_iter()
            .flat_map(::std::collections::HashMap::values)
    }

    /// The one time series `workspace_name` holds that `lookup` names, or which
    /// absence stopped the lookup ([`WorkspaceSeriesMiss`]).
    ///
    /// ONE resolution for every operation scoped to a workspace, so the alias
    /// form and the asset-property pair form cannot drift between the handlers
    /// that accept them: the alias resolves through the workspace's alias index,
    /// and a pair resolves to the [`time_series_id`] that pair owns and is then
    /// looked for among the workspace's own series.
    pub fn workspace_series(
        &self,
        workspace_name: &str,
        lookup: SeriesLookup<'_>,
    ) -> Result<&CaptureSourceRecord, WorkspaceSeriesMiss> {
        if !self.workspaces.contains_key(workspace_name) {
            return Err(WorkspaceSeriesMiss::NoSuchWorkspace);
        }
        let (held, requested) = match lookup {
            SeriesLookup::Alias(alias) => (
                self.capture_source_by_alias(workspace_name, alias),
                alias.to_string(),
            ),
            SeriesLookup::Pair(asset_label, property_label) => {
                // Both halves are modelled `CustomID`s, so each can arrive as the
                // raw UUID or as `externalId:<externalId>`; an absent one is the
                // absence it is, exactly as it is for a lookup naming no
                // workspace.
                let asset_id = self
                    .resolve_asset_id(asset_label)
                    .ok_or_else(|| WorkspaceSeriesMiss::NoSuchAsset(asset_label.to_string()))?;
                let property_id = self
                    .assets
                    .get(&asset_id)
                    .and_then(|asset| asset.property(property_label))
                    .map(|property| property.id.clone())
                    .ok_or_else(|| {
                        WorkspaceSeriesMiss::NoSuchProperty(
                            asset_label.to_string(),
                            property_label.to_string(),
                        )
                    })?;
                let time_series_id = time_series_id(&asset_id, &property_id);
                (
                    self.capture_source_by_id(workspace_name, &time_series_id),
                    time_series_id,
                )
            }
        };
        held.ok_or(WorkspaceSeriesMiss::NoSuchSeries(requested))
    }

    /// Forget the time series `workspace_name` holds under `time_series_id`,
    /// answering the record that was removed.
    ///
    /// What `DeleteTimeSeries` scoped to a workspace does: the stream goes,
    /// video and all, and neither its id nor the alias bound to it names
    /// anything in that workspace afterwards.
    pub fn forget_capture_source(
        &mut self,
        workspace_name: &str,
        time_series_id: &str,
    ) -> Option<CaptureSourceRecord> {
        let sources = self.capture_sources.get_mut(workspace_name)?;
        let record = sources.remove(time_series_id)?;
        if sources.is_empty() {
            self.capture_sources.remove(workspace_name);
        }
        if let Some(aliases) = self.capture_source_aliases.get_mut(workspace_name) {
            aliases.retain(|_, held| held != time_series_id);
            if aliases.is_empty() {
                self.capture_source_aliases.remove(workspace_name);
            }
        }
        Some(record)
    }

    /// The logging options `workspace` reports: the ones last set for that scope,
    /// or [`DEFAULT_LOGGING_LEVEL`] while nothing has set any. `None` asks for the
    /// account-and-region-wide scope.
    ///
    /// A pure READ: an unconfigured scope is not materialised, so it still reports
    /// the default to the next caller.
    pub fn logging_options_for(&self, workspace: Option<&str>) -> LoggingOptions {
        self.logging_options
            .get(workspace.unwrap_or(ACCOUNT_LOGGING_SCOPE))
            .map(|record| record.options.clone())
            .unwrap_or_else(|| LoggingOptions {
                level: DEFAULT_LOGGING_LEVEL.to_string(),
            })
    }

    /// Set `workspace`'s logging options, replacing whatever that scope carried.
    /// `None` sets the account-and-region-wide scope. One scope's options are
    /// independent of every other's.
    ///
    /// Bounded at [`MAX_LOGGING_SCOPES`]: past that many scopes the least recently
    /// set WORKSPACE scope is dropped, so a stream of puts naming fresh workspaces
    /// cannot grow this map without limit. A dropped scope reports
    /// [`DEFAULT_LOGGING_LEVEL`] again, which is what it reported before anything
    /// set it; the account-and-region-wide scope is exempt from the eviction, so
    /// the unnamed scope survives any number of workspace-scoped puts.
    pub fn set_logging_options(
        &mut self,
        workspace: Option<&str>,
        options: LoggingOptions,
        now: SystemTime,
    ) {
        self.logging_options.insert(
            workspace.unwrap_or(ACCOUNT_LOGGING_SCOPE).to_string(),
            LoggingOptionsRecord {
                options,
                set_at: now,
            },
        );
        while self.logging_options.len() > MAX_LOGGING_SCOPES {
            let Some(oldest) = self
                .logging_options
                .iter()
                .filter(|(scope, _)| scope.as_str() != ACCOUNT_LOGGING_SCOPE)
                .min_by_key(|(_, record)| record.set_at)
                .map(|(scope, _)| scope.clone())
            else {
                break;
            };
            self.logging_options.remove(&oldest);
        }
    }

    /// The tag set of the resource `arn` names, for reading.
    ///
    /// Resolved through [`parse_resource_arn`] and then by exact ARN equality, so
    /// an ARN that names a stored id in another partition, account or region
    /// resolves to nothing — the resource it names is not one this store holds.
    /// Every record this store holds that carries tags is reachable: the arms
    /// below are the whole list of taggable kinds, and each is one line because
    /// the per-record work lives in [`TaggedRecord`].
    pub fn tags_for_resource(&self, arn: &str) -> Option<&HashMap<String, String>> {
        let (resource_type, id) = parse_resource_arn(arn)?;
        match resource_type {
            ASSET_RESOURCE_TYPE => tags_of(self.assets.get(id), arn),
            ASSET_MODEL_RESOURCE_TYPE => tags_of(self.asset_models.get(id), arn),
            GATEWAY_RESOURCE_TYPE => tags_of(self.gateways.get(id), arn),
            PORTAL_RESOURCE_TYPE => tags_of(self.portals.get(id), arn),
            PROJECT_RESOURCE_TYPE => tags_of(self.projects.get(id), arn),
            PIPELINE_RESOURCE_TYPE => {
                let (workspace, name) = id.split_once('/')?;
                tags_of(self.pipelines.get(&pipeline_key(workspace, name)), arn)
            }
            TASK_RESOURCE_TYPE => {
                let (workspace, name) = id.split_once('/')?;
                tags_of(self.tasks.get(&task_key(workspace, name)), arn)
            }
            DATASET_RESOURCE_TYPE => tags_of(self.datasets.get(id), arn),
            APPLICATION_RESOURCE_TYPE => tags_of(self.applications.get(id), arn),
            WORKSPACE_RESOURCE_TYPE => tags_of(self.workspaces.get(id), arn),
            COMPUTATION_MODEL_RESOURCE_TYPE => tags_of(self.computation_models.get(id), arn),
            ACCESS_POLICY_RESOURCE_TYPE => tags_of(self.access_policies.get(id), arn),
            DASHBOARD_RESOURCE_TYPE => tags_of(self.dashboards.get(id), arn),
            _ => None,
        }
    }

    /// The same tag set, for writing. See [`Self::tags_for_resource`].
    pub fn tags_for_resource_mut(&mut self, arn: &str) -> Option<&mut HashMap<String, String>> {
        let (resource_type, id) = parse_resource_arn(arn)?;
        match resource_type {
            ASSET_RESOURCE_TYPE => tags_of_mut(self.assets.get_mut(id), arn),
            ASSET_MODEL_RESOURCE_TYPE => tags_of_mut(self.asset_models.get_mut(id), arn),
            GATEWAY_RESOURCE_TYPE => tags_of_mut(self.gateways.get_mut(id), arn),
            PORTAL_RESOURCE_TYPE => tags_of_mut(self.portals.get_mut(id), arn),
            PROJECT_RESOURCE_TYPE => tags_of_mut(self.projects.get_mut(id), arn),
            PIPELINE_RESOURCE_TYPE => {
                let (workspace, name) = id.split_once('/')?;
                tags_of_mut(self.pipelines.get_mut(&pipeline_key(workspace, name)), arn)
            }
            TASK_RESOURCE_TYPE => {
                let (workspace, name) = id.split_once('/')?;
                tags_of_mut(self.tasks.get_mut(&task_key(workspace, name)), arn)
            }
            DATASET_RESOURCE_TYPE => tags_of_mut(self.datasets.get_mut(id), arn),
            APPLICATION_RESOURCE_TYPE => tags_of_mut(self.applications.get_mut(id), arn),
            WORKSPACE_RESOURCE_TYPE => tags_of_mut(self.workspaces.get_mut(id), arn),
            COMPUTATION_MODEL_RESOURCE_TYPE => {
                tags_of_mut(self.computation_models.get_mut(id), arn)
            }
            ACCESS_POLICY_RESOURCE_TYPE => tags_of_mut(self.access_policies.get_mut(id), arn),
            DASHBOARD_RESOURCE_TYPE => tags_of_mut(self.dashboards.get_mut(id), arn),
            _ => None,
        }
    }

    /// Forget an asset and everything keyed off it: its external-id entry, its
    /// property series, its property aliases, its hierarchy associations and its
    /// membership of every IoT SiteWise Monitor project.
    pub fn forget_asset(&mut self, asset_id: &str) -> Option<AssetRecord> {
        let record = self.assets.remove(asset_id)?;
        if let Some(external) = record.external_id.as_deref() {
            self.asset_external_ids.remove(external);
        }
        self.property_values.remove(asset_id);
        self.property_alias_index
            .retain(|_, (asset, _)| asset != asset_id);
        self.asset_hierarchy_children
            .retain(|(asset, _), _| asset != asset_id);
        for children in self.asset_hierarchy_children.values_mut() {
            children.retain(|child| child != asset_id);
        }
        self.asset_hierarchy_children
            .retain(|_, children| !children.is_empty());
        self.asset_hierarchy_settles_at
            .retain(|(asset, _, child), _| asset != asset_id && child != asset_id);
        // A project's membership names assets, so an asset this service no longer
        // holds is in no project: dropping the entry here is what keeps
        // `ListProjectAssets` from reporting an asset nothing can address.
        for project in self.projects.values_mut() {
            project.asset_ids.remove(asset_id);
        }
        Some(record)
    }

    /// How many executions of `(workspace_name, pipeline_name)` have not
    /// finished, as [`PipelineExecutionRecord::is_active`] decides it.
    ///
    /// A count rather than a list: the executions are held in a hash map, so
    /// any order this reported would be the map's, not the service's.
    pub fn active_pipeline_execution_count(
        &self,
        workspace_name: &str,
        pipeline_name: &str,
        now: SystemTime,
    ) -> usize {
        self.pipeline_executions
            .values()
            .filter(|execution| {
                execution.workspace_name == workspace_name
                    && execution.pipeline_name == pipeline_name
                    && execution.is_active(now)
            })
            .count()
    }

    /// The pipeline of `workspace_name` whose definition names `task_name` on a
    /// compute node, as `(pipelineName, computeNodeName)`, or `None` when no
    /// pipeline of that workspace references the task.
    ///
    /// A compute node "references a task" through its `taskName`, and the
    /// definition that counts is the pipeline's CURRENT version: that is the one
    /// a start of the pipeline runs its nodes from
    /// (`crate::ops::start_pipeline_execution`), so a version a later write
    /// superseded references nothing any more. The dependency a task carries as
    /// something a pipeline points at, which is what the operation that removes
    /// a task has to refuse on — the same question
    /// [`Self::hierarchy_parent`] answers for an asset.
    ///
    /// The pipelines are held in a hash map, so the smallest
    /// `(pipelineName, computeNodeName)` pair is reported rather than whichever
    /// the map yields first: one store state then always names one reference,
    /// whatever order the map iterates in.
    pub fn pipeline_referencing_task(
        &self,
        workspace_name: &str,
        task_name: &str,
    ) -> Option<(&str, &str)> {
        self.pipelines
            .values()
            .filter(|pipeline| pipeline.workspace_name == workspace_name)
            .flat_map(|pipeline| {
                pipeline
                    .current
                    .computations
                    .iter()
                    .filter(|node| node.task_name == task_name)
                    .map(|node| (pipeline.name.as_str(), node.compute_node_name.as_str()))
            })
            .min()
    }

    /// Forget a pipeline and everything keyed off it: every version it holds and
    /// its tags (both on the record) and the executions recorded against it — so
    /// no later read resolves to a pipeline this service no longer holds.
    pub fn forget_pipeline(
        &mut self,
        workspace_name: &str,
        pipeline_name: &str,
    ) -> Option<PipelineRecord> {
        let record = self
            .pipelines
            .remove(&pipeline_key(workspace_name, pipeline_name))?;
        let forgotten: HashSet<String> = self
            .pipeline_executions
            .values()
            .filter(|execution| {
                execution.workspace_name == workspace_name
                    && execution.pipeline_name == pipeline_name
            })
            .map(|execution| execution.id.clone())
            .collect();
        self.pipeline_executions
            .retain(|id, _| !forgotten.contains(id));
        Some(record)
    }

    /// Forget a gateway and everything keyed off it: its capability
    /// configurations and its tags (both on the record), and the
    /// [`Self::gateway_devices`] entry that reserved its platform device.
    ///
    /// The index entry goes with the record, so the device the gateway ran on is
    /// free for the next `CreateGateway`: one device hosts at most one gateway,
    /// and a gateway this service no longer holds occupies none. Every entry
    /// naming this gateway is dropped, rather than only the one
    /// [`gateway_device_key`] rebuilds from the stored platform, so no index
    /// entry can outlive the gateway it pointed at.
    pub fn forget_gateway(&mut self, gateway_id: &str) -> Option<GatewayRecord> {
        let record = self.gateways.remove(gateway_id)?;
        self.gateway_devices.retain(|_, id| id != gateway_id);
        Some(record)
    }

    /// Forget every access policy whose `Resource` the predicate names.
    ///
    /// A policy is ABOUT one Monitor resource — the portal or the project its
    /// `Resource` names — and both `CreateAccessPolicy` and
    /// `UpdateAccessPolicy` resolve that resource against this store before they
    /// write ([`Self::missing_monitor_resource`]), so a resource this service no
    /// longer holds can leave no policy behind for a read or a listing to
    /// report: no write of this crate could have produced such a record, and no
    /// caller could repair one, because the rewrite asks the same resolution and
    /// so refuses the only resource the stored policy names. The policy's tags
    /// live on its record, which owns them, so they go with it — the same reason
    /// [`Self::forget_asset`] drops what is keyed off an asset.
    ///
    /// The one place this is stated: a portal's delete and a project's delete
    /// both reach it, so the two cannot drift.
    fn forget_access_policies_about(&mut self, about: impl Fn(&Resource) -> bool) {
        self.access_policies
            .retain(|_, policy| !about(&policy.resource));
    }

    /// Forget an IoT SiteWise Monitor project, the dashboards created in it and
    /// the access policies granted on it.
    ///
    /// A dashboard is created IN a project and names that project on every read —
    /// the create refuses a project this store does not hold — so one whose
    /// project is gone could never have been created. The project's tags go with
    /// its record, which owns them. The access policies that NAME the project go
    /// through [`Self::forget_access_policies_about`], for the reason stated
    /// there.
    ///
    /// The one place this containment rule is stated: `DeleteProject` removes one
    /// project through it and [`Self::forget_portal`] removes every project of a
    /// portal through it, so the two cannot drift.
    pub fn forget_project(&mut self, project_id: &str) -> Option<ProjectRecord> {
        let record = self.projects.remove(project_id)?;
        self.dashboards
            .retain(|_, dashboard| dashboard.project_id != project_id);
        self.forget_access_policies_about(|resource| {
            resource
                .project
                .as_ref()
                .is_some_and(|project| project.id == project_id)
        });
        Some(record)
    }

    /// Forget an IoT SiteWise Monitor portal and everything this service holds
    /// only as a part of or a reference to it: the projects created in it, the
    /// dashboards created in those projects, and the access policies granted on
    /// the portal or on one of those projects.
    ///
    /// A project is created IN a portal — the create refuses a portal this store
    /// does not hold — so a portal the service no longer holds can leave none of
    /// its projects addressable behind. Dropping them here is what keeps a later
    /// read from resolving to a project or a dashboard whose portal is gone, the
    /// same reason [`Self::forget_asset`] drops what is keyed off an asset. Each
    /// project goes through [`Self::forget_project`], so a portal's delete and a
    /// project's own delete take exactly the same things with them — the policies
    /// granted on those projects included; the ones granted on the PORTAL go
    /// through [`Self::forget_access_policies_about`] here.
    pub fn forget_portal(&mut self, portal_id: &str) -> Option<PortalRecord> {
        let record = self.portals.remove(portal_id)?;
        let projects: Vec<String> = self
            .projects
            .values()
            .filter(|project| project.portal_id == portal_id)
            .map(|project| project.id.clone())
            .collect();
        for project in projects {
            self.forget_project(&project);
        }
        self.forget_access_policies_about(|resource| {
            resource
                .portal
                .as_ref()
                .is_some_and(|portal| portal.id == portal_id)
        });
        Some(record)
    }

    /// The IoT SiteWise Monitor resource an access policy is about, resolved
    /// against this store: `Some((kind, requested))` when the alternative the
    /// request names holds no record here, `None` when it resolves.
    ///
    /// A `Resource` names a portal OR a project — [`check_access_policy_resource`]
    /// is what holds a request to exactly one of them — and BOTH are resources
    /// this service holds, so each alternative is resolved against the map that
    /// holds it. The returned kind is the [`RESOURCE_PORTAL`] /
    /// [`RESOURCE_PROJECT`] label a not-found refusal names the absent resource
    /// by, and the identifier is the one the REQUEST supplied, so the caller is
    /// told which half of the choice resolved to nothing.
    ///
    /// The one place this resolution is stated: `CreateAccessPolicy` and
    /// `UpdateAccessPolicy` both ask it, so the two cannot drift.
    pub fn missing_monitor_resource<'a>(
        &self,
        resource: &'a Resource,
    ) -> Option<(&'static str, &'a str)> {
        if let Some(portal) = resource.portal.as_ref() {
            if !self.portals.contains_key(&portal.id) {
                return Some((RESOURCE_PORTAL, portal.id.as_str()));
            }
        }
        if let Some(project) = resource.project.as_ref() {
            if !self.projects.contains_key(&project.id) {
                return Some((RESOURCE_PROJECT, project.id.as_str()));
            }
        }
        None
    }

    /// Forget an asset model, its external-id entry and every interface
    /// relationship it stood on either side of.
    ///
    /// A relationship names two models, so a model that is gone leaves no
    /// relationship behind: the entries are dropped here rather than left to
    /// consume the [`MAX_INTERFACE_RELATIONSHIPS`] budget for a pair nothing can
    /// address any more.
    pub fn forget_asset_model(&mut self, asset_model_id: &str) -> Option<AssetModelRecord> {
        let record = self.asset_models.remove(asset_model_id)?;
        if let Some(external) = record.external_id.as_deref() {
            self.asset_model_external_ids.remove(external);
        }
        self.interface_relationships
            .retain(|(applied_to, interface), _| {
                applied_to != asset_model_id && interface != asset_model_id
            });
        Some(record)
    }

    /// Forget a computation model and everything this store holds only as a
    /// record OF it: the actions taken on it and the executions those actions
    /// started.
    ///
    /// An action and an execution each NAME the target resource they were taken
    /// on, and a computation model is one of the two kinds a target resource can
    /// be, so a model this service no longer holds can leave none of them behind
    /// for a listing to report or for the [`MAX_ACTIONS`] budget to count — the
    /// same reason [`Self::forget_asset`] drops what is keyed off an asset. They
    /// are addressed through the one
    /// [`target_resource_addresses`] rule the listings resolve a target by, so
    /// what goes with the model is exactly what was reported for it. The model's
    /// tags live on its record, which owns them, so they go with it.
    pub fn forget_computation_model(
        &mut self,
        computation_model_id: &str,
    ) -> Option<ComputationModelRecord> {
        let record = self.computation_models.remove(computation_model_id)?;
        self.actions.retain(|_, action| {
            !action.targets(
                TARGET_RESOURCE_TYPE_COMPUTATION_MODEL,
                computation_model_id,
            )
        });
        self.executions.retain(|_, execution| {
            !execution.targets(
                TARGET_RESOURCE_TYPE_COMPUTATION_MODEL,
                computation_model_id,
            )
        });
        Some(record)
    }

    /// The resources `workspace_name` CONTAINS, as `(resource kind, how many)`
    /// pairs in one fixed order, listing only the kinds that hold at least one.
    ///
    /// These are the kinds a caller creates INSIDE a workspace and addresses as
    /// resources of their own — each with its own create and its own delete — so
    /// they are what a delete of the container has to find empty. The records
    /// this store keys by a workspace but a caller cannot CREATE one at a time
    /// (a search, a query, an enrichment job, a dataset export job, a capture
    /// source) are NOT contents in this sense: they are what running the
    /// workspace produced, and they go with it in [`Self::forget_workspace`] —
    /// and so is a bulk import job, which a caller does start one at a time but
    /// can never DELETE, so counting it here would refuse the container's delete
    /// for good. A
    /// pipeline execution goes with its own PIPELINE
    /// ([`Self::forget_pipeline`]), which is a content and so has to be gone
    /// before the workspace can be deleted at all.
    pub fn workspace_contents(&self, workspace_name: &str) -> Vec<(&'static str, usize)> {
        let datasets = self
            .datasets
            .values()
            .filter(|dataset| dataset.workspace_name.as_deref() == Some(workspace_name))
            .count();
        let pipelines = self
            .pipelines
            .keys()
            .filter(|(workspace, _)| workspace == workspace_name)
            .count();
        let tasks = self
            .tasks
            .keys()
            .filter(|(workspace, _)| workspace == workspace_name)
            .count();
        let applications = self
            .applications
            .values()
            .filter(|application| application.workspace_name == workspace_name)
            .count();
        [
            (RESOURCE_DATASET, datasets),
            (RESOURCE_PIPELINE, pipelines),
            (RESOURCE_TASK, tasks),
            (RESOURCE_APPLICATION, applications),
        ]
        .into_iter()
        .filter(|(_, held)| *held > 0)
        .collect()
    }

    /// Forget a workspace and everything keyed off it: its tags (on the record),
    /// the searches, asynchronous queries, enrichment jobs, dataset export jobs
    /// and bulk import jobs started in it, the capture sources it holds together
    /// with the alias index they resolve through, and the logging scope set for
    /// it.
    ///
    /// Each of those is addressable ONLY inside the workspace, so a workspace
    /// this service no longer holds can leave none of them behind for a later
    /// read — or for the per-workspace bounds those maps are capped by. The
    /// logging options of the workspace scope go too, so a workspace later
    /// created under the same name reports the default level rather than
    /// inheriting what was set for the one that is gone; the
    /// account-and-region-wide scope ([`ACCOUNT_LOGGING_SCOPE`]) is keyed under a
    /// name no workspace can carry and so survives.
    ///
    /// The resources a caller addresses one at a time are NOT removed here:
    /// [`Self::workspace_contents`] is what refuses a delete while any of them
    /// stands.
    pub fn forget_workspace(&mut self, workspace_name: &str) -> Option<WorkspaceRecord> {
        let record = self.workspaces.remove(workspace_name)?;
        self.searches
            .retain(|(workspace, _), _| workspace != workspace_name);
        self.queries
            .retain(|(workspace, _), _| workspace != workspace_name);
        self.enrichment_jobs
            .retain(|(workspace, _), _| workspace != workspace_name);
        self.dataset_export_jobs
            .retain(|(workspace, _), _| workspace != workspace_name);
        // A bulk import job names the workspace its destination session dataset
        // lives in, and nothing deletes a job on its own, so the jobs of this
        // workspace go with it however the records were removed: a job the store
        // kept would answer a read with a `workspaceName` this store no longer
        // holds. A job ingesting into time series OUTSIDE any workspace names
        // none and so belongs to no workspace's delete.
        self.bulk_import_jobs
            .retain(|_, job| job.workspace_name.as_deref() != Some(workspace_name));
        self.capture_sources.remove(workspace_name);
        self.capture_source_aliases.remove(workspace_name);
        self.logging_options.remove(workspace_name);
        Some(record)
    }

    /// Re-materialise every asset created from asset model `asset_model_id`
    /// against that model's CURRENT property and hierarchy definitions.
    ///
    /// This is the inheritance `UpdateAssetModel` documents: "Updates an asset
    /// model and all of the assets that were created from the model. Each asset
    /// created from the model inherits the updated asset model's property and
    /// hierarchy definitions", and "If you remove a property from an asset model,
    /// IoT SiteWise deletes all previous data for that property".
    ///
    /// So, per asset of that model: a property the model still defines keeps the
    /// asset-level bindings the model does not define (its alias and its
    /// notification state) and takes the definition's name, data type and unit; a
    /// property the model no longer defines goes, and with it its stored time
    /// series and its alias binding; a hierarchy the model no longer defines goes,
    /// and with it the child associations recorded under it. Each affected asset
    /// reports `UPDATING` until the transition this call owes falls due, exactly as
    /// [`AssetRecord::begin_update`] records it for a direct `UpdateAsset`.
    ///
    /// "Property definitions" is everything the model declares a property by, so
    /// the definitions each COMPOSITE model of it contributes are propagated on
    /// the same terms as its own: a composite model added to the model reaches
    /// the assets already created from it, and one removed from it takes its
    /// per-asset properties with it.
    ///
    /// A composite model that names a `COMPONENT_MODEL` contributes THAT model's
    /// definitions, read from its stored record rather than copied when the
    /// composite model was created (see
    /// [`Self::composite_model_properties`]) — so a change to a component model
    /// changes what an asset of every model REUSING it carries, and those assets
    /// inherit it on the same terms as the assets of the model that changed.
    /// Walked transitively, because a model reused here may itself be reused
    /// elsewhere, and each model is re-materialised at most once so a reuse graph
    /// that closes on itself still terminates.
    ///
    /// `asset_model_id` is a stored id, not a label: resolve it with
    /// [`Self::resolve_asset_model_id`] first. Does nothing when no stored model
    /// has that id, or when no asset was created from it.
    pub fn propagate_asset_model_definitions(&mut self, asset_model_id: &str, now: SystemTime) {
        let mut pending = vec![asset_model_id.to_string()];
        let mut propagated: HashSet<String> = HashSet::new();
        while let Some(model_id) = pending.pop() {
            if !propagated.insert(model_id.clone()) {
                continue;
            }
            self.propagate_definitions_to_own_assets(&model_id, now);
            pending.extend(self.asset_models_reusing(&model_id));
        }
    }

    /// Every stored composite model that REUSES asset model `asset_model_id` as
    /// its component model, as `(the asset model holding it, its
    /// assetModelCompositeModelId)` pairs: the composition relationships of that
    /// model, read from the holder's side, which is the one place a composite
    /// model's reuse link is recorded.
    ///
    /// ONE definition of "which composite models reuse this asset model", so the
    /// listing that reports those relationships, the propagation that follows
    /// them and the delete that has to refuse while one stands all answer from
    /// the same enumeration. A composite model carrying no id holds no link and
    /// so is in no relationship.
    ///
    /// Ordered the way the relationships are reported: the holding asset models
    /// oldest first, ties broken by id so two created in the same instant still
    /// order deterministically, and within one holder the stored order of its
    /// composite models.
    pub fn composite_models_reusing(&self, asset_model_id: &str) -> Vec<(String, String)> {
        let mut holders: Vec<&AssetModelRecord> = self.asset_models.values().collect();
        holders.sort_by(|left, right| {
            left.creation_date
                .cmp(&right.creation_date)
                .then_with(|| left.id.cmp(&right.id))
        });
        holders
            .into_iter()
            .flat_map(|holder| {
                holder.composite_models.iter().filter_map(move |composite| {
                    let composite_id = composite.id.as_deref()?;
                    (holder.composed_asset_model_id(composite_id) == Some(asset_model_id))
                        .then(|| (holder.id.clone(), composite_id.to_string()))
                })
            })
            .collect()
    }

    /// The stored asset models holding a composite model whose component model is
    /// `asset_model_id`, each named once: the holders of its composition
    /// relationships ([`Self::composite_models_reusing`]).
    fn asset_models_reusing(&self, asset_model_id: &str) -> Vec<String> {
        let mut named: HashSet<String> = HashSet::new();
        self.composite_models_reusing(asset_model_id)
            .into_iter()
            .filter_map(|(holder, _)| named.insert(holder.clone()).then_some(holder))
            .collect()
    }

    /// `asset_model_id` together with every asset model whose assets carry a
    /// definition of it: the transitive closure of [`Self::asset_models_reusing`],
    /// which is exactly the set of models
    /// [`Self::propagate_asset_model_definitions`] re-materialises the assets of.
    ///
    /// Asked by the dependency question a removal from this model's definitions
    /// has to answer ([`Self::data_segment_of_dropped_model_properties`]): the
    /// streams such a removal drops are those of the assets that propagation
    /// reaches, not only of the assets created directly from the model. Each
    /// model is answered at most once, so a reuse graph that closes on itself
    /// still terminates.
    fn models_inheriting_definitions_of(&self, asset_model_id: &str) -> HashSet<String> {
        let mut pending = vec![asset_model_id.to_string()];
        let mut inheriting: HashSet<String> = HashSet::new();
        while let Some(model_id) = pending.pop() {
            if !inheriting.insert(model_id.clone()) {
                continue;
            }
            pending.extend(self.asset_models_reusing(&model_id));
        }
        inheriting
    }

    /// Re-materialise the assets created from asset model `asset_model_id` ONLY,
    /// which is one step of [`Self::propagate_asset_model_definitions`]: the
    /// models reusing this one are walked there.
    fn propagate_definitions_to_own_assets(&mut self, asset_model_id: &str, now: SystemTime) {
        let Some(model) = self.asset_models.get(asset_model_id) else {
            return;
        };
        // Materialised ONCE, through the shared rule, and owned, so the per-asset
        // writes below are not borrowing the model: what an asset of this model
        // carries does not depend on which asset it is. The value each property
        // STARTS with is taken from the same place, indexed by property id so
        // the per-property lookups below cost nothing: an asset carries one
        // property per definition of its model and of every composite model of
        // it, so a linear scan per property would be quadratic in that count.
        let materialised = self.materialise_asset_properties(model, now);
        let initial_values = materialised.initial_values_by_id();
        let hierarchy_definitions = model.hierarchies.clone();
        let asset_ids: Vec<String> = self
            .assets
            .values()
            .filter(|asset| asset.asset_model_id == asset_model_id)
            .map(|asset| asset.id.clone())
            .collect();

        for asset_id in asset_ids {
            let Some(asset) = self.assets.get_mut(&asset_id) else {
                continue;
            };
            let carried = ::std::mem::take(&mut asset.properties);
            // Where each property this asset already carried sits, so the pass
            // below is one constant-time lookup per property rather than a scan.
            // The FIRST occurrence of an id wins, because that is the one
            // [`AssetRecord::property_position`] resolves and so the one every
            // write of an alias or a notification landed on: an id two composite
            // models reusing a component model contribute twice names one
            // definition, one series and one binding.
            let mut held_at: HashMap<&str, usize> = HashMap::new();
            for (at, property) in carried.iter().enumerate() {
                held_at.entry(property.id.as_str()).or_insert(at);
            }
            // What a property this asset did not carry STARTS with: it is
            // materialised HERE for the first time, so it takes the sample its
            // definition declares exactly as a property materialised by a create
            // does (see [`initial_property_sample`]). A property the asset
            // already carried keeps the series written to it.
            let mut properties = materialised.properties.clone();
            let mut seeded: Vec<(String, AssetPropertyValue)> = Vec::new();
            let mut seeded_ids: HashSet<String> = HashSet::new();
            for property in &mut properties {
                let held = held_at
                    .get(property.id.as_str())
                    .and_then(|at| carried.get(*at));
                if held.is_none() && seeded_ids.insert(property.id.clone()) {
                    if let Some(sample) = initial_values.get(property.id.as_str()) {
                        seeded.push((property.id.clone(), (*sample).clone()));
                    }
                }
                // An alias and a notification are not inherited from a
                // definition, so a property the asset already carried keeps the
                // ones set on it by `UpdateAssetProperty`.
                property.alias = held.and_then(|held| held.alias.clone());
                property.notification = held.and_then(|held| held.notification.clone());
            }
            let kept: HashSet<String> = properties
                .iter()
                .map(|property| property.id.clone())
                .collect();
            let hierarchies: Vec<AssetHierarchy> = hierarchy_definitions
                .iter()
                .map(|hierarchy| AssetHierarchy {
                    id: hierarchy.id.clone(),
                    external_id: hierarchy.external_id.clone(),
                    name: hierarchy.name.clone(),
                })
                .collect();
            let dropped_hierarchies: Vec<String> = asset
                .hierarchies
                .iter()
                .filter_map(|hierarchy| hierarchy.id.clone())
                .filter(|id| {
                    !hierarchies
                        .iter()
                        .any(|kept| kept.id.as_deref() == Some(id.as_str()))
                })
                .collect();
            let dropped_properties: Vec<AssetProperty> = carried
                .into_iter()
                .filter(|property| !kept.contains(&property.id))
                .collect();
            asset.properties = properties;
            asset.hierarchies = hierarchies;
            asset.begin_update(now);

            for property in dropped_properties {
                if let Some(series) = self.property_values.get_mut(&asset_id) {
                    series.remove(&property.id);
                    if series.is_empty() {
                        self.property_values.remove(&asset_id);
                    }
                }
                if let Some(alias) = property.alias {
                    let ours = self
                        .property_alias_index
                        .get(&alias)
                        .is_some_and(|(bound_asset, bound_property)| {
                            *bound_asset == asset_id && *bound_property == property.id
                        });
                    if ours {
                        self.property_alias_index.remove(&alias);
                    }
                }
            }
            for hierarchy_id in dropped_hierarchies {
                self.asset_hierarchy_children
                    .remove(&(asset_id.clone(), hierarchy_id.clone()));
                // The settle clock of an association is keyed by the same triple,
                // so it goes with the association it belongs to.
                self.asset_hierarchy_settles_at
                    .retain(|(asset, hierarchy, _), _| {
                        asset != &asset_id || hierarchy != &hierarchy_id
                    });
            }
            for (property_id, value) in seeded {
                self.record_property_value(&asset_id, &property_id, value);
            }
        }
    }

    /// The child assets associated with `(asset_id, hierarchy_id)`.
    pub fn hierarchy_children(&self, asset_id: &str, hierarchy_id: &str) -> &[String] {
        self.asset_hierarchy_children
            .get(&(asset_id.to_string(), hierarchy_id.to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// Whether any child asset is associated under ANY hierarchy of `asset_id`.
    ///
    /// The dependency an asset carries as a PARENT: what makes it a resource other
    /// resources still point at, which is what the operations that remove an asset
    /// have to refuse on.
    pub fn has_hierarchy_children(&self, asset_id: &str) -> bool {
        self.asset_hierarchy_children
            .iter()
            .any(|((asset, _), children)| asset == asset_id && !children.is_empty())
    }

    /// The association `child_asset_id` is a child through, as
    /// `(parentAssetId, hierarchyId)`, or `None` when the asset is nobody's child.
    ///
    /// The inverse of [`Self::hierarchy_children`], and single-valued because
    /// `AssociateAssets` refuses an association for a child that already has a
    /// parent: the asset hierarchies form a TREE, so an asset appears as a child at
    /// most once in [`Self::asset_hierarchy_children`]. It lives here, beside the
    /// index it reads, because it is the one step every upward walk of that tree
    /// takes — `AssociateAssets`'s cycle check, `ListAssets`'s `TOP_LEVEL` filter,
    /// `ListAssetRelationships`'s `PATH_TO_ROOT` traversal and `DeleteAsset`'s
    /// refusal to remove a still-associated asset all take it.
    pub fn hierarchy_parent(&self, child_asset_id: &str) -> Option<(&str, &str)> {
        self.asset_hierarchy_children
            .iter()
            .find(|(_, children)| children.iter().any(|child| child == child_asset_id))
            .map(|((parent, hierarchy), _)| (parent.as_str(), hierarchy.as_str()))
    }

    /// Record `child_id` as a child of `asset_id` under `hierarchy_id`, after the
    /// children already associated there, and start the transition that call owes:
    /// the association reports [`AssociationState::InProgress`] until
    /// [`ACTIVATION_DELAY`] after `now` and [`AssociationState::Established`] from
    /// then on. The due instant is STORED, never slept on.
    ///
    /// The list stays a set: a child already associated under that hierarchy is not
    /// added twice. `AssociateAssets` refuses such a request before reaching here,
    /// so this keeps the stored invariant for any caller whose own rule allows it.
    pub fn associate_child(
        &mut self,
        asset_id: &str,
        hierarchy_id: &str,
        child_id: &str,
        now: SystemTime,
    ) {
        let children = self
            .asset_hierarchy_children
            .entry((asset_id.to_string(), hierarchy_id.to_string()))
            .or_default();
        if !children.iter().any(|child| child == child_id) {
            children.push(child_id.to_string());
        }
        self.asset_hierarchy_settles_at.insert(
            (
                asset_id.to_string(),
                hierarchy_id.to_string(),
                child_id.to_string(),
            ),
            now + ACTIVATION_DELAY,
        );
    }

    /// How far the association of `(asset_id, hierarchy_id, child_id)` has got as
    /// of `now`, or `None` when that triple is not associated.
    ///
    /// Derived from the stored due instant, so a read never has to write: an
    /// association whose due instant has passed is `Established`, and one recorded
    /// by a call that has not settled yet is `InProgress`.
    pub fn association_state(
        &self,
        asset_id: &str,
        hierarchy_id: &str,
        child_id: &str,
        now: SystemTime,
    ) -> Option<AssociationState> {
        let associated = self
            .hierarchy_children(asset_id, hierarchy_id)
            .iter()
            .any(|child| child == child_id);
        if !associated {
            return None;
        }
        let settles_at = self.asset_hierarchy_settles_at.get(&(
            asset_id.to_string(),
            hierarchy_id.to_string(),
            child_id.to_string(),
        ));
        match settles_at {
            Some(due) if now < *due => Some(AssociationState::InProgress),
            _ => Some(AssociationState::Established),
        }
    }

    /// Resolve a `(parent, hierarchyId, childAssetId)` triple of `CustomID` labels
    /// to the stored ids it names.
    ///
    /// Every label takes either form the model admits (a raw id or
    /// `externalId:<externalId>`). The hierarchy is resolved against the PARENT
    /// ASSET'S MODEL — the same place `AssociateAssets` resolves it, since the
    /// model is what defines which groupings exist — so the operation that records
    /// an association and the one that removes it address exactly one set of
    /// hierarchies, even once a model's definition has changed since the asset was
    /// created. The asset's own copied hierarchy list is the fallback for a parent
    /// whose model is no longer stored.
    pub fn resolve_hierarchy_triple(
        &self,
        asset_label: &str,
        hierarchy_label: &str,
        child_label: &str,
    ) -> Result<HierarchyTriple, TripleRefusal> {
        let asset_id = self
            .resolve_asset_id(asset_label)
            .ok_or_else(|| TripleRefusal::AssetNotFound(asset_label.to_string()))?;
        let asset = self
            .assets
            .get(&asset_id)
            .ok_or_else(|| TripleRefusal::AssetNotFound(asset_label.to_string()))?;
        let hierarchy_id = self
            .asset_models
            .get(&asset.asset_model_id)
            .and_then(|model| {
                model.hierarchies.iter().find(|hierarchy| {
                    matches_id_label(
                        hierarchy.id.as_deref(),
                        hierarchy.external_id.as_deref(),
                        hierarchy_label,
                    )
                })
            })
            .and_then(|hierarchy| hierarchy.id.clone())
            .or_else(|| {
                asset
                    .hierarchy(hierarchy_label)
                    .and_then(|hierarchy| hierarchy.id.clone())
            })
            .ok_or_else(|| TripleRefusal::HierarchyNotFound(hierarchy_label.to_string()))?;
        let child_asset_id = self
            .resolve_asset_id(child_label)
            .ok_or_else(|| TripleRefusal::AssetNotFound(child_label.to_string()))?;
        Ok(HierarchyTriple {
            asset_id,
            hierarchy_id,
            child_asset_id,
        })
    }

    /// Remove `child_id` from `(asset_id, hierarchy_id)`, answering whether an
    /// association was there to remove. `false` means the triple was never
    /// associated, which is the caller's cue to refuse rather than to report a
    /// removal that did not happen.
    ///
    /// The write side of the association set lives in `AssociateAssets`, which
    /// owns the hierarchy rules it has to enforce before recording one; this is
    /// the removal `DisassociateAssets` performs.
    pub fn disassociate_child(
        &mut self,
        asset_id: &str,
        hierarchy_id: &str,
        child_id: &str,
    ) -> bool {
        let key = (asset_id.to_string(), hierarchy_id.to_string());
        let Some(children) = self.asset_hierarchy_children.get_mut(&key) else {
            return false;
        };
        let Some(at) = children.iter().position(|child| child == child_id) else {
            return false;
        };
        children.remove(at);
        // An emptied hierarchy keeps no entry, so the map holds only live
        // associations and cannot grow with keys nothing reads.
        if children.is_empty() {
            self.asset_hierarchy_children.remove(&key);
        }
        self.asset_hierarchy_settles_at.remove(&(
            asset_id.to_string(),
            hierarchy_id.to_string(),
            child_id.to_string(),
        ));
        true
    }
}

/// The resource kind a `ResourceNotFoundException` / `ResourceAlreadyExistsException`
/// names for an asset, as AWS words it.
pub const RESOURCE_ASSET: &str = "Asset";

/// The resource kind those errors name for an asset model.
pub const RESOURCE_ASSET_MODEL: &str = "Asset Model";

/// The resource kind those errors name for a pipeline.
pub const RESOURCE_PIPELINE: &str = "Pipeline";

/// The resource kind those errors name for a task.
pub const RESOURCE_TASK: &str = "Task";

/// The resource kind those errors name for a dataset.
pub const RESOURCE_DATASET: &str = "Dataset";

/// The resource kind those errors name for a data segment a dataset holds.
pub const RESOURCE_DATA_SEGMENT: &str = "Data Segment";

/// The resource kind those errors name for a search.
pub const RESOURCE_SEARCH: &str = "Search";

/// The resource kind those errors name for a dataset export job.
pub const RESOURCE_DATASET_EXPORT_JOB: &str = "Dataset Export Job";

/// The resource kind those errors name for a workspace.
pub const RESOURCE_WORKSPACE: &str = "Workspace";

/// The resource kind those errors name for a pipeline execution.
pub const RESOURCE_PIPELINE_EXECUTION: &str = "Pipeline Execution";

/// The resource kind those errors name for a query of a workspace.
pub const RESOURCE_QUERY: &str = "Query";

/// The resource kind those errors name for a composite model of an ASSET — the
/// instance of an asset model's composite model that lives on one asset.
///
/// A composite model is not a top-level resource, so it is named as what it is
/// rather than as an "Asset". The composite model of an asset MODEL is not here:
/// the service states its absence in its own form, naming the model it was
/// looked for in ([`composite_model_not_found_message`]).
pub const RESOURCE_ASSET_COMPOSITE_MODEL: &str = "Asset Composite Model";

/// The resource kind those errors name for a gateway.
///
/// AWS's own wording was observed for the asset and asset-model paths; for this
/// kind and the three below, the label is the service's resource name under that
/// same one form, so every handler addressing one states the absence identically.
pub const RESOURCE_GATEWAY: &str = "Gateway";

/// The resource kind those errors name for one CAPABILITY of a gateway — the
/// configuration a gateway holds under one namespace.
///
/// A capability is not a top-level resource, so it is named as what it is rather
/// than as a "Gateway", and it lives here beside its siblings so the operations
/// that address one cannot each invent a kind of their own.
pub const RESOURCE_GATEWAY_CAPABILITY: &str = "Gateway Capability";

/// The resource kind those errors name for a data stream, which every operation
/// that addresses one addresses by its `alias`.
pub const RESOURCE_TIME_SERIES: &str = "Time Series";

/// The resource kind those errors name for an IoT SiteWise Monitor portal.
pub const RESOURCE_PORTAL: &str = "Portal";

/// The resource kind those errors name for an IoT SiteWise Monitor project.
pub const RESOURCE_PROJECT: &str = "Project";

/// The resource kind those errors name for an IoT SiteWise Monitor access policy.
pub const RESOURCE_ACCESS_POLICY: &str = "Access Policy";

/// The resource kind those errors name for a computation model.
pub const RESOURCE_COMPUTATION_MODEL: &str = "Computation Model";

/// The resource kind those errors name for one action taken on a target
/// resource — what `ExecuteAction` records and `DescribeAction` reads back.
pub const RESOURCE_ACTION: &str = "Action";

/// The resource kind those errors name for one EXECUTION of an action — the run
/// `ExecuteAction` starts, which `DescribeExecution` reads back and
/// `ListExecutions` summarises.
pub const RESOURCE_EXECUTION: &str = "Execution";

/// The resource kind those errors name for an action DEFINITION: one of the
/// actions a target resource exposes, which an execute addresses by its
/// `actionDefinitionId`.
///
/// A definition is not a top-level resource but one of the actions a resource
/// publishes, so it is named as what it is rather than as the resource it
/// belongs to.
pub const RESOURCE_ACTION_DEFINITION: &str = "Action Definition";

/// The message a not-found refusal carries: the resource kind, then the
/// identifier the REQUEST supplied, echoed exactly as sent — whatever form that
/// identifier took for the resource in question (a raw id, an `externalId:`
/// label, a stream alias, a workspace name).
///
/// The form every absence this service has no resource-specific wording for is
/// stated in, so no handler invents its own template: AWS states such an absence
/// as `<Resource> with ID <id> not found`, which is why the identifier is echoed
/// rather than replaced by whatever the handler resolved. The families whose own
/// wording the service uses instead each have one function of their own beside
/// this one ([`asset_not_found_message`],
/// [`composite_model_not_found_message`], [`alias_not_found_message`],
/// [`gateway_not_found_message`], [`gateway_not_found_for_update_message`],
/// [`hierarchy_not_found_message`], [`not_found_arn_message`]).
pub fn not_found_message(resource: &str, requested: &str) -> String {
    format!("{resource} with ID {requested} not found")
}

/// The message a not-found refusal for an ASSET carries.
///
/// An asset is addressed either by its own id or by the `externalId:` selector
/// the `CustomID` pattern admits. Which of the two the REQUEST used is what
/// failed to resolve, so it is what the refusal reports: a selector-addressed
/// request failed to resolve an EXTERNAL ID, which the service states with the
/// bare external id — the selector is how the request named the dimension, not
/// part of its value — while a request that supplied the id itself keeps the one
/// form [`not_found_message`] states every absence in.
///
/// One place, so every operation that resolves an asset label words the same
/// absence identically whichever form the label took.
pub fn asset_not_found_message(requested: &str) -> String {
    match requested.strip_prefix(EXTERNAL_ID_PREFIX) {
        Some(external_id) => format!("asset with externalId: {external_id} doesn't exist."),
        None => not_found_message(RESOURCE_ASSET, requested),
    }
}

/// The message a not-found refusal for one COMPOSITE MODEL of an asset model
/// carries: a composite model is addressable only WITHIN an asset model, and the
/// same composite-model id can be absent from many models, so the absence names
/// both halves of the address — the composite model asked for and the asset model
/// it was looked for in — each echoed as the request supplied it.
///
/// One place, so the read, the create of a nested one, the update and the delete
/// cannot word the same absence four ways.
pub fn composite_model_not_found_message(asset_model: &str, composite_model: &str) -> String {
    format!("CompositeModel: {composite_model} not found for AssetModel: {asset_model}")
}

/// The message a not-found refusal for a data stream addressed by its ALIAS
/// carries: an alias is resolved as a `dataAlias` within ONE account, so the
/// absence names the account the lookup ran in and the alias it ran for, rather
/// than reporting the alias as an id of the stream.
///
/// The account is the one every ARN this emulator mints carries, so a refusal and
/// the resources a caller can see agree about which account it is addressing.
/// One place, so every operation that resolves the `alias` of a time series —
/// the read, the delete and the binding of one to an asset property — words that
/// absence identically.
pub fn alias_not_found_message(alias: &str) -> String {
    let account = ::engine::EMULATOR_ACCOUNT_ID;
    format!(
        "dataAlias with accountId AwsAccountId{{value={account}}}, \
         alias PropertyAlias(super=PropertyAlias{{value={alias}}}) not found"
    )
}

/// The message a READ of an absent gateway is refused with: the id that resolved
/// to nothing, and the service's guidance to check it.
///
/// A gateway's `gatewayId` is modelled as a plain `ID`, so the only identifier a
/// refusal can name is the id the request path carried.
pub fn gateway_not_found_message(gateway_id: &str) -> String {
    format!(
        "No gateway with ID {gateway_id} was found. Please verify that you supplied the correct \
         gateway ID and try again."
    )
}

/// The message an UPDATE of an absent gateway is refused with. The service words
/// this one differently from the read above ([`gateway_not_found_message`]), so
/// the two are kept as the two sentences they are rather than collapsed into one.
pub fn gateway_not_found_for_update_message(gateway_id: &str) -> String {
    format!(
        "A gateway with ID {gateway_id} could not be found. Please verify you supplied the \
         correct gateway ID and try again."
    )
}

/// The message a half-open span covering no time is refused with: a span's
/// start must be strictly before its end, so the refusal names the two members
/// the request stated it with.
///
/// One wording for every operation that takes a span as a pair of modelled
/// members, so the two batch data-segment operations cannot each state the same
/// refusal differently.
pub fn empty_span_message() -> String {
    "endTimestamp must be later than startTimestamp.".to_string()
}

/// The message a call that would drop a data stream a dataset still holds a
/// data segment of is refused with: the stream that cannot go, and one dataset
/// that still holds a segment of it.
///
/// One wording for every call that removes a stream — the delete of the time
/// series itself, the delete of the asset whose property owns it, the model
/// update that takes that property away — so the same dependency cannot be
/// stated three ways. AWS words the rule as "You can't delete a time series
/// until all of its data segments have been deleted from session datasets"
/// (`DeleteTimeSeries`'s doc), so the refusal names the stream and the dataset
/// whose segments have to go first.
pub fn data_segments_remain_message(time_series_id: &str, holder: &str) -> String {
    format!(
        "The time series {time_series_id} has data segments in the dataset {holder}. \
         Delete them before deleting it."
    )
}

/// The message a uniqueness refusal carries: AWS names the DIMENSION of the
/// request that collided (`externalId`, `ID`, `name`), not an identifier the
/// rejected request would have minted.
pub fn already_using_message(resource: &str, dimension: &str) -> String {
    format!("Another {resource} is already using the same {dimension}")
}

/// The message an absent HIERARCHY carries. A hierarchy is not a top-level
/// resource but a grouping an asset model declares, and AWS words its absence as
/// a hierarchy that "doesn't exist"; the identifier is still the one the request
/// supplied, echoed exactly as sent.
///
/// One place, so the operations that address a hierarchy — the two association
/// operations and the listing over one — cannot word the same absence three ways.
pub fn hierarchy_not_found_message(requested: &str) -> String {
    format!("A hierarchy with ID {requested} doesn't exist")
}

/// The message a not-found refusal carries where the request addressed the
/// resource by its ARN rather than by an id: the dimension the request named is
/// the ARN, so that is what the refusal states.
pub fn not_found_arn_message(requested_arn: &str) -> String {
    format!("Resource with ARN {requested_arn} not found")
}

/// `a, b, c` — the accepted values a refusal names, so a message states the RULE
/// that refused the request rather than only that it was refused.
///
/// One place, so every operation that refuses a value outside a modelled
/// catalogue words the catalogue the same way.
pub fn catalog_names<'a>(values: impl Iterator<Item = &'a str>) -> String {
    values.collect::<Vec<&str>>().join(", ")
}

/// Turn `AssetModelPropertyDefinition`s into the stored `AssetModelProperty`s,
/// minting an id for every definition that did not supply one.
///
/// Shared by every operation that takes property definitions, so one rule is
/// enforced in one place. Two rules the shapes' own documentation states are
/// enforced here: a property definition carries exactly ONE property type ("You
/// can only specify one type in a property definition"), and a `STRUCT` property
/// also carries `dataTypeSpec` ("This parameter is required on properties that
/// have the STRUCT data type"). The `Err` carries the refusal message, which the
/// calling operation lifts into its own modelled `InvalidRequestException`.
pub fn property_definitions_to_properties(
    definitions: &[AssetModelPropertyDefinition],
) -> Result<Vec<AssetModelProperty>, String> {
    let mut properties = Vec::with_capacity(definitions.len());
    for definition in definitions {
        check_property_shape(
            &definition.name,
            &definition.data_type,
            definition.data_type_spec.as_deref(),
            &definition.r#type,
        )?;
        properties.push(AssetModelProperty {
            id: Some(definition.id.clone().unwrap_or_else(new_id)),
            external_id: definition.external_id.clone(),
            name: definition.name.clone(),
            data_type: definition.data_type.clone(),
            data_type_spec: definition.data_type_spec.clone(),
            unit: definition.unit.clone(),
            r#type: definition.r#type.clone(),
            path: None,
        });
    }
    Ok(properties)
}

/// One model property materialised as the asset property an asset created from
/// that model carries: the asset property INHERITS the definition's id, external
/// id, name, data type and unit, so a value written against the asset resolves
/// to the definition it came from. An alias and a notification are not inherited
/// — they are set per asset property, by `UpdateAssetProperty`.
///
/// `None` when the stored model property carries no usable id, which would
/// materialise an asset property no request could address.
///
/// Shared by every operation that reports an asset's properties, so the asset
/// properties a create materialises and the ones a read derives from the same
/// definitions cannot disagree.
pub fn asset_property_from_model(property: &AssetModelProperty) -> Option<AssetProperty> {
    let id = property.id.clone().filter(|id| !id.is_empty())?;
    Some(AssetProperty {
        id,
        external_id: property.external_id.clone(),
        name: property.name.clone(),
        alias: None,
        notification: None,
        data_type: property.data_type.clone(),
        data_type_spec: property.data_type_spec.clone(),
        unit: property.unit.clone(),
        path: None,
    })
}

/// What an asset created from one asset model carries: the flat property list
/// [`AssetRecord::properties`] holds, and the value each of those properties
/// starts its series with.
///
/// Produced by
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::materialise_asset_properties`]
/// — the one rule for "the properties of an asset of this model" — so the create
/// that materialises an asset and the propagation that re-materialises it cannot
/// disagree.
#[derive(Debug, Default, Clone)]
pub struct MaterialisedAssetProperties {
    /// Every property the asset carries: the model's own top-level definitions
    /// first, then the ones each composite model of it contributes.
    pub properties: Vec<AssetProperty>,
    /// `(propertyId, value)` for each property whose definition declares the
    /// value it starts with; see [`initial_property_sample`].
    pub initial_values: Vec<(String, AssetPropertyValue)>,
}

impl MaterialisedAssetProperties {
    /// The values properties start their series with, indexed by property id.
    ///
    /// What a caller re-materialising many assets from one model asks per
    /// property, so the lookup is constant-time rather than a scan of the list.
    pub fn initial_values_by_id(&self) -> HashMap<&str, &AssetPropertyValue> {
        self.initial_values
            .iter()
            .map(|(id, value)| (id.as_str(), value))
            .collect()
    }
}

/// Materialise ONE definition into `materialised`, under the composite-model
/// path `prefix` (`None` for one of the asset's own top-level definitions).
///
/// Two composite models may reuse the SAME component model, so one definition
/// can be contributed by more than one of them: the asset then carries that
/// property once per composite model that contributes it, each instance under
/// its own path, which is what the composite-model views of the asset report.
/// They share the definition's id, hence the one time series that id addresses,
/// so the value the series STARTS with is seeded once — `seen` is what keeps a
/// second instance from seeding it again.
fn materialise_one(
    definition: &AssetModelProperty,
    prefix: Option<&[AssetPropertyPathSegment]>,
    now: SystemTime,
    seen: &mut HashSet<String>,
    materialised: &mut MaterialisedAssetProperties,
) {
    let Some(mut property) = asset_property_from_model(definition) else {
        return;
    };
    let first = seen.insert(property.id.clone());
    // `path` is "the structured path to the property from the root of the
    // asset": the composite models the property sits inside, outermost first,
    // then the property itself. A property of the asset's own definitions sits
    // inside none, so it carries no path — which is also what tells the two
    // apart wherever a response has to (see [`composite_model_of_property`]).
    if let Some(prefix) = prefix {
        let mut path = prefix.to_vec();
        path.push(AssetPropertyPathSegment {
            id: Some(property.id.clone()),
            name: Some(property.name.clone()),
        });
        property.path = Some(path);
    }
    if first {
        if let Some(sample) = initial_property_sample(definition, now) {
            materialised
                .initial_values
                .push((property.id.clone(), sample));
        }
    }
    materialised.properties.push(property);
}

/// The `assetCompositeModelId` of the composite model that contributes
/// `property` to the asset carrying it, or `None` for one of the asset's own
/// (base) properties.
///
/// Read off the property's own `path` — "the structured path to the property
/// from the root of the asset", whose last segment is the property and whose
/// segment before it is the composite model that holds it — so the membership
/// and the path a response reports come from ONE stored value and cannot drift
/// apart. A base property carries no path.
pub fn composite_model_of_property(property: &AssetProperty) -> Option<&str> {
    let path = property.path.as_deref()?;
    let holder = path.len().checked_sub(2)?;
    path[holder].id.as_deref()
}

/// Which of the four property types a `PropertyType` sets, named as the shape's
/// own member; `None` when it sets none of them or more than one.
///
/// The ONE place this service decides what "the type of a property" is, so the
/// rule that a definition carries exactly one type and the rule that an existing
/// property's type cannot change ask the same question of the same shape.
pub fn property_type_kind(property_type: &PropertyType) -> Option<&'static str> {
    let kinds = [
        ("attribute", property_type.attribute.is_some()),
        ("measurement", property_type.measurement.is_some()),
        ("transform", property_type.transform.is_some()),
        ("metric", property_type.metric.is_some()),
    ];
    let mut found = None;
    for (name, set) in kinds {
        if set {
            if found.is_some() {
                return None;
            }
            found = Some(name);
        }
    }
    found
}

/// The two rules the property shapes' own documentation states, checked wherever
/// a model property arrives — as a definition on a create, or as a full
/// `AssetModelProperty` on `UpdateAssetModel`:
///
/// * exactly ONE property type ("You can only specify one type in a property
///   definition"), and
/// * `dataTypeSpec` on a `STRUCT` ("This parameter is required on properties that
///   have the STRUCT data type").
///
/// The `Err` carries the refusal message, which the calling operation lifts into
/// its own modelled `InvalidRequestException`.
pub fn check_property_shape(
    name: &str,
    data_type: &str,
    data_type_spec: Option<&str>,
    property_type: &PropertyType,
) -> Result<(), String> {
    if property_type_kind(property_type).is_none() {
        let set = [
            property_type.attribute.is_some(),
            property_type.measurement.is_some(),
            property_type.transform.is_some(),
            property_type.metric.is_some(),
        ]
        .into_iter()
        .filter(|set| *set)
        .count();
        return Err(format!(
            "The property definition {name} must specify exactly one property type \
             (attribute, measurement, transform or metric), but specifies {set}."
        ));
    }
    if data_type == DATA_TYPE_STRUCT && data_type_spec.is_none() {
        return Err(format!(
            "The property definition {name} has the STRUCT data type, so dataTypeSpec is required."
        ));
    }
    Ok(())
}

/// The one `PropertyDataType` value that also requires `dataTypeSpec`; see
/// [`check_property_shape`].
pub const DATA_TYPE_STRUCT: &str = "STRUCT";

/// The `PropertyDataType` of a time series whose samples are video — what a
/// workspace's capture source reports (see
/// [`CaptureSourceRecord::time_series_summary`]). Source:
/// `smithy_query(query="shape", shape="PropertyDataType")`, whose values are
/// STRING | INTEGER | DOUBLE | BOOLEAN | STRUCT | VIDEO | ANNOTATION | JSON.
pub const DATA_TYPE_VIDEO: &str = "VIDEO";

/// `id`, unless it is absent or empty.
pub fn non_empty_id(id: Option<&str>) -> Option<String> {
    id.filter(|id| !id.is_empty()).map(ToString::to_string)
}

/// Whether a request's `(id, externalId)` pair names the stored member whose own
/// pair is `(held_id, held_external_id)`.
///
/// A member of an asset model's definition — a property, a hierarchy, a composite
/// model — is addressed by its `id`, or by its `externalId` when the request
/// carries no id for it; a request that carries neither describes a NEW member,
/// which is why a pair of absent ids never matches anything.
pub fn same_definition_member(
    held_id: Option<&str>,
    held_external_id: Option<&str>,
    id: Option<&str>,
    external_id: Option<&str>,
) -> bool {
    match (non_empty_id(id), non_empty_id(external_id)) {
        (Some(id), _) => held_id == Some(id.as_str()),
        (None, Some(external_id)) => held_external_id == Some(external_id.as_str()),
        (None, None) => false,
    }
}

/// The property list a request's `AssetModelProperty`s become, resolved against
/// the `stored` properties they replace.
///
/// Each requested property is checked by the same rule every arriving model
/// property is ([`check_property_shape`]). A property the holder already carries
/// keeps its stored id — so its asset-level data and its alias survive — and may
/// change its name, unit and the inside of its type, but not its TYPE or its DATA
/// TYPE: "You can’t change the type or data type of an existing property". A
/// property the request carries no stored id for is new and gets one minted,
/// unless the request supplied one. A property the request does not name at all is
/// gone, which is how a caller removes one.
///
/// Shared by every operation that rewrites a stored property list from a full
/// `AssetModelProperty` list, so the properties of a model and the properties of
/// one of its composite models follow one rule. The `Err` carries the refusal
/// message, which the calling operation lifts into its own modelled
/// `InvalidRequestException`.
pub fn resolve_updated_properties(
    stored: &[AssetModelProperty],
    requested: &[AssetModelProperty],
) -> Result<Vec<AssetModelProperty>, String> {
    let mut resolved: Vec<AssetModelProperty> = Vec::with_capacity(requested.len());
    for property in requested {
        check_property_shape(
            &property.name,
            &property.data_type,
            property.data_type_spec.as_deref(),
            &property.r#type,
        )?;
        let held = stored.iter().find(|held| {
            same_definition_member(
                held.id.as_deref(),
                held.external_id.as_deref(),
                property.id.as_deref(),
                property.external_id.as_deref(),
            )
        });
        let id = match held {
            Some(held) => {
                // `dataTypeSpec` is part of a STRUCT property's data type ("the data
                // type of the structure for this property"), so it cannot move either.
                if held.data_type != property.data_type
                    || held.data_type_spec != property.data_type_spec
                {
                    return Err(format!(
                        "The existing property {} cannot change its data type from {} to {}.",
                        property.name,
                        describe_data_type(&held.data_type, held.data_type_spec.as_deref()),
                        describe_data_type(&property.data_type, property.data_type_spec.as_deref()),
                    ));
                }
                let held_kind = property_type_kind(&held.r#type);
                let kind = property_type_kind(&property.r#type);
                if held_kind != kind {
                    return Err(format!(
                        "The existing property {} cannot change its property type from {} to {}.",
                        property.name,
                        held_kind.unwrap_or("none"),
                        kind.unwrap_or("none"),
                    ));
                }
                held.id.clone().unwrap_or_else(new_id)
            }
            None => non_empty_id(property.id.as_deref()).unwrap_or_else(new_id),
        };
        resolved.push(AssetModelProperty {
            id: Some(id),
            external_id: property.external_id.clone(),
            name: property.name.clone(),
            data_type: property.data_type.clone(),
            data_type_spec: property.data_type_spec.clone(),
            unit: property.unit.clone(),
            r#type: property.r#type.clone(),
            // `path` is server-computed, never caller-supplied; the reads derive
            // what they report.
            path: None,
        });
    }
    // One request cannot describe the same stored property twice: both entries
    // would be that one property, so the later one would silently win.
    for (at, property) in resolved.iter().enumerate() {
        if resolved[..at]
            .iter()
            .any(|earlier| earlier.id == property.id)
        {
            return Err(format!(
                "The request names the property {} more than once.",
                property.id.as_deref().unwrap_or_default()
            ));
        }
    }
    Ok(resolved)
}

/// A property's data type as a refusal names it: the `PropertyDataType`, plus the
/// `dataTypeSpec` a STRUCT carries.
fn describe_data_type(data_type: &str, data_type_spec: Option<&str>) -> String {
    match data_type_spec {
        Some(spec) => format!("{data_type} ({spec})"),
        None => data_type.to_string(),
    }
}

/// The `ETag` the `If-Match` / `If-None-Match` preconditions of an asset-model
/// write are evaluated against, as of `now`, or `None` when the version
/// `version_type` names does not exist.
///
/// `matchForVersionType` targets the modelled `AssetModelVersionType` enum
/// (`LATEST | ACTIVE`) and [`VERSION_TYPE_LATEST`] is its documented default, so
/// the version it names is resolved by [`AssetModelRecord::version_view`] — the
/// same resolution a read's `assetModelVersion` goes through — and the tag is
/// the one of THAT version's definition. The `None` is a model that has no such
/// version, which a precondition can only evaluate to false. Shared by every
/// operation that carries those headers, so `CreateAssetModelCompositeModel`,
/// `UpdateAssetModel` and `DeleteAssetModel` cannot disagree about which tag a
/// precondition targets.
pub fn asset_model_target_etag(
    record: &AssetModelRecord,
    version_type: Option<&str>,
    now: SystemTime,
) -> Option<String> {
    record
        .version_view(version_type, now)
        .map(|view| asset_model_etag(&view))
}

/// The conditional request header an asset-model write carries the expected
/// entity tag in, spelled the way the model binds it (`@httpHeader("If-Match")`).
pub const HEADER_IF_MATCH: &str = "If-Match";

/// The conditional request header an asset-model write asks for the ABSENCE of
/// the targeted version with, spelled the way the model binds it
/// (`@httpHeader("If-None-Match")`).
pub const HEADER_IF_NONE_MATCH: &str = "If-None-Match";

/// The message a failed asset-model precondition carries on an operation that
/// WRITES the model's definition — the creates and the updates: the conditional
/// request header whose precondition evaluated to FALSE, and the version
/// `matchForVersionType` targeted ([`VERSION_TYPE_LATEST`] by default).
///
/// This is the sentence IoT SiteWise was observed answering those writes with.
/// Nothing of the REQUEST's own values goes into it — not the entity tag the
/// caller sent, which is the value the precondition rejected and which the
/// caller already holds, not the spelling it sent the header in, and not the
/// resource's own id — so the refusal states the condition that failed and the
/// version it was evaluated on, and leaves the caller to re-read the resource
/// for the tag it now carries.
///
/// One wording for every one of those writes, so `CreateAssetModelCompositeModel`,
/// `UpdateAssetModel` and `UpdateAssetModelCompositeModel` cannot word one
/// condition on one resource three ways. The REMOVALS answer the sentence of
/// their own that IoT SiteWise was observed answering them with; it is
/// [`precondition_failed_removal_message`].
pub fn precondition_failed_message(header: &str, version_type: Option<&str>) -> String {
    let version_type = version_type.unwrap_or(VERSION_TYPE_LATEST).to_lowercase();
    format!("Provided {header} value does not match the current {version_type} version ETag.")
}

/// The message a failed asset-model precondition carries on an operation that
/// REMOVES part of the model — `DeleteAssetModel` and
/// `DeleteAssetModelCompositeModel`: the conditional request header whose
/// precondition evaluated to FALSE, and nothing else.
///
/// This is the sentence IoT SiteWise was observed answering the removals with:
/// it names no version, so the version selector does not reach it either. As
/// with [`precondition_failed_message`], nothing of the REQUEST's own values
/// goes into it. One wording for both removals, so neither can word the
/// condition its own way.
pub fn precondition_failed_removal_message(header: &str) -> String {
    format!("Precondition failed for provided {header} value.")
}

/// The message a read or a precondition carries when the asset model has no
/// ACTIVE version: the model has never finished settling, so the only
/// definition it holds is one no version alias but `LATEST` names.
///
/// One wording for every operation that resolves the selector, so the reads of
/// a model, of its properties and of one of its composite models cannot word
/// the same absence three ways. `state` is the `AssetModelState` the model does
/// report, which is what tells the caller to wait rather than to look elsewhere.
pub fn no_active_version_message(asset_model_id: &str, state: &str) -> String {
    format!("The asset model {asset_model_id} has no ACTIVE version yet; it is {state}.")
}

/// The entity tag of an asset model's stored definition.
///
/// CONTENT-DERIVED, never minted: the SHA-256 digest of the fields a write to the
/// model changes, rendered as hex and cut to the 43 characters the modelled
/// `ETag` type fixes (`^[\w-]{43}$`, and a hex digit is a `\w`). Two reads of an
/// unchanged model therefore report the same tag and any change to the definition
/// reports a different one, which is what the `If-Match` / `If-None-Match`
/// preconditions on the asset-model writes compare against. 43 hex characters
/// carry 172 bits of the digest, and the tag is opaque to the caller: nothing but
/// equality is ever asked of it. Hex because this crate already mints its ids and
/// pagination tokens with `hex`, so the tag needs no further encoder.
///
/// The stored definition goes into the fingerprint through `{:?}`, which covers
/// every member of these shapes — a hand-listed set of fields would silently stop
/// covering a member the model later grows, and a tag that misses a change is the
/// one failure mode a precondition cannot tolerate. The exact bytes are not a
/// contract: a caller reads a tag and writes it back within one process.
///
/// The version's own NUMBER is part of the fingerprint, so the latest and the
/// active version of a model mid-update — two different definitions — report
/// two different tags, and a caller that read one writes back a precondition
/// targeting that one (see [`AssetModelRecord::version_view`]).
pub fn asset_model_etag(record: &AssetModelRecord) -> String {
    // The links are a map, so they go into the fingerprint in key order: two
    // models with the same definition must hash to the same tag whatever order
    // their maps happen to iterate in.
    let mut links: Vec<_> = record.composite_model_links.iter().collect();
    links.sort_by_key(|(id, _)| *id);
    let fingerprint = format!(
        "{}|{}|{}|{}|{}|{:?}|{:?}|{:?}|{:?}",
        record.id,
        record.version,
        record.name,
        record.description.as_deref().unwrap_or(""),
        record.model_type,
        record.properties,
        record.hierarchies,
        record.composite_models,
        links,
    );
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(fingerprint.as_bytes());
    let mut tag = ::hex::encode(digest);
    tag.truncate(ETAG_LENGTH);
    tag
}

/// The expression variables of a property's formula: those of its `transform`
/// or of its `metric`.
///
/// An `attribute` or a `measurement` carries no formula and so contributes no
/// variables. ONE definition of what a property's formula depends on, so every
/// rule about formula dependencies asks the same question of the same members.
pub fn formula_variables(
    property: &AssetModelProperty,
) -> impl Iterator<Item = &ExpressionVariable> {
    let transform = property
        .r#type
        .transform
        .iter()
        .flat_map(|transform| transform.variables.iter());
    let metric = property
        .r#type
        .metric
        .iter()
        .flat_map(|metric| metric.variables.iter().flatten());
    transform.chain(metric)
}

/// Total order on a `TimeInNanos`: whole seconds first, then the nanosecond
/// offset, exactly as the modelled members are defined.
pub fn timestamp_key(timestamp: &TimeInNanos) -> (i64, i32) {
    (
        timestamp.time_in_seconds,
        timestamp.offset_in_nanos.unwrap_or(0),
    )
}

/// Total order on a data segment inside the dataset that holds it: the time
/// series it covers, then its span.
///
/// This is the segment's IDENTITY — what `BatchDeleteDatasetDataSegments`
/// addresses one by — so the one comparison serves the sorted insert, the
/// single-valued replace and the lookup a deletion makes.
fn data_segment_key(segment: &DataSegmentRecord) -> (&str, (i64, i32), (i64, i32)) {
    (
        segment.time_series_id.as_str(),
        timestamp_key(&segment.start_timestamp),
        timestamp_key(&segment.end_timestamp),
    )
}

/// Where in a dataset's sorted segment log the segment `time_series_id` and the
/// span `[start, end)` name EXACTLY sits, or `None` when the log holds none.
///
/// The one lookup every operation addressing a single segment goes through —
/// the read and the removal alike — so a segment is found by its identity in
/// exactly one place.
fn data_segment_at(
    held: &[DataSegmentRecord],
    time_series_id: &str,
    start: &TimeInNanos,
    end: &TimeInNanos,
) -> Option<usize> {
    let wanted = (time_series_id, timestamp_key(start), timestamp_key(end));
    held.binary_search_by(|stored| data_segment_key(stored).cmp(&wanted))
        .ok()
}

/// A wall-clock instant as the modelled `TimeInNanos`: whole seconds since the
/// Unix epoch, plus the nanosecond offset inside that second.
///
/// An instant before the epoch reports second 0 — no stored instant this service
/// mints is one, since every one of them comes from the wall clock.
pub fn time_in_nanos(at: SystemTime) -> TimeInNanos {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    TimeInNanos {
        time_in_seconds: i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        offset_in_nanos: i32::try_from(since_epoch.subsec_nanos()).ok(),
    }
}

/// The initial sample an asset property materialised from `definition` carries,
/// or `None` when the definition declares none.
///
/// An `attribute` definition's `defaultValue` IS that property's value on every
/// asset made from the model — "All assets that you create from the asset model
/// contain this attribute value. You can update an attribute's value after you
/// create an asset" (`smithy_query(query="doc", shape="Attribute",
/// member="defaultValue")`) — so the property's series starts with that value at
/// the instant the property was materialised, with the quality an ordinary
/// reading carries ([`DEFAULT_QUALITY`]). A later write at a newer timestamp
/// takes over as the property's current value, which is what "you can update an
/// attribute's value" means.
///
/// `None` for every other property type (a measurement, transform or metric
/// declares no default), for an attribute that declares no `defaultValue`, and
/// for a default that does not read as the property's `PropertyDataType`: the
/// declared text is then no value of that type, and nothing is invented for it.
pub fn initial_property_sample(
    definition: &AssetModelProperty,
    at: SystemTime,
) -> Option<AssetPropertyValue> {
    let default = definition.r#type.attribute.as_ref()?.default_value.as_ref()?;
    Some(AssetPropertyValue {
        value: typed_variant(&definition.data_type, default)?,
        timestamp: time_in_nanos(at),
        quality: Some(DEFAULT_QUALITY.to_string()),
    })
}

/// `text` read as the one `Variant` member the `PropertyDataType` `data_type`
/// names, or `None` when it is no value of that type.
///
/// The pairing of data type to `Variant` member is the model's: the
/// `PropertyDataType` values are STRING | INTEGER | DOUBLE | BOOLEAN | STRUCT |
/// VIDEO | ANNOTATION | JSON (`smithy_query(query="shape",
/// shape="PropertyDataType,Variant")`), of which only the first four have a
/// `Variant` member to carry a scalar reading, so a default declared on any
/// other type reads as no value at all.
fn typed_variant(data_type: &str, text: &str) -> Option<Variant> {
    let variant = match data_type {
        "STRING" => Variant {
            string_value: Some(text.to_string()),
            ..Variant::default()
        },
        "INTEGER" => Variant {
            integer_value: Some(text.parse().ok()?),
            ..Variant::default()
        },
        // A non-finite reading (`inf`, `NaN`) parses as an `f64` but is no number
        // the JSON wire can carry, so it is no value of this type either.
        "DOUBLE" => Variant {
            double_value: Some(text.parse().ok().filter(|value: &f64| value.is_finite())?),
            ..Variant::default()
        },
        "BOOLEAN" => Variant {
            boolean_value: Some(text.parse().ok()?),
            ..Variant::default()
        },
        _ => return None,
    };
    Some(variant)
}

/// The opaque pagination token that resumes at the [`timestamp_key`] `resume` of
/// the enumeration `scope` identifies: the hex of `seconds.nanos` beside that
/// scope.
///
/// ONE recipe for every time-ranged read of this service that pages by the
/// instant it stopped at, so an encoding fix cannot drift between operations.
/// Hex keeps the text inside the `^[A-Za-z0-9+/=]+$` pattern the paging members
/// of this service carry.
///
/// The token carries `scope` — a [`pagination_scope`] of the request members that
/// decide WHICH entries are enumerated and in what order — beside the instant,
/// because an instant alone names a position in every enumeration that happens to
/// hold it. See [`pagination_scope`] for what that binding is for.
pub fn encode_timestamp_token(resume: (i64, i32), scope: &str) -> String {
    let (seconds, nanos) = resume;
    ::hex::encode(
        format!(
            "{seconds}{TIMESTAMP_TOKEN_SEPARATOR}{nanos}{TOKEN_SCOPE_SEPARATOR}{scope}"
        )
        .as_bytes(),
    )
}

/// The [`timestamp_key`] `token` resumes the enumeration `scope` identifies at,
/// or `None` for text this service never issued as a token OF THAT enumeration —
/// which its caller answers as the modelled refusal for an invalid `nextToken`.
pub fn decode_timestamp_token(token: &str, scope: &str) -> Option<(i64, i32)> {
    let decoded = decode_token_text(token)?;
    let (resume, carried) = decoded.rsplit_once(TOKEN_SCOPE_SEPARATOR)?;
    if carried != scope {
        return None;
    }
    let (seconds, nanos) = resume.split_once(TIMESTAMP_TOKEN_SEPARATOR)?;
    Some((token_number(seconds)?, token_number(nanos)?))
}

/// What joins the two halves of the resume point in a token
/// [`encode_timestamp_token`] builds.
const TIMESTAMP_TOKEN_SEPARATOR: char = '.';

/// What joins a token's resume point to the [`pagination_scope`] it was minted
/// in. Neither half can contain it: a resume point is digits and the separators
/// above, and a scope is hex.
const TOKEN_SCOPE_SEPARATOR: char = '|';

/// A short digest of what a paginated read asks of its ENUMERATION: the request
/// members that decide which entries it enumerates and in what order.
///
/// A pagination token carries this beside its resume point, so a token is only
/// honoured against the request that produced it. A resume point on its own —
/// an instant, a window start, a position — names something in any enumeration
/// that happens to hold it, so a token replayed against another one (another
/// stream, another filter, another ordering, another range) would silently serve
/// a slice of that other enumeration and drop everything before the point it
/// resumed at. Bound to the scope it was minted in, such a token names no page at
/// all and its caller refuses it. The page SIZE is not part of a scope: it
/// changes where a page ends, not which entries are enumerated or in what order.
///
/// `members` render through the shared [`request_fingerprint`], so one member
/// renders one way wherever it is bound.
pub fn pagination_scope(members: &[&dyn ::std::fmt::Debug]) -> String {
    let rendered = request_fingerprint(members, None);
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(rendered.as_bytes());
    let mut scope = ::hex::encode(digest);
    scope.truncate(PAGINATION_SCOPE_LENGTH);
    scope
}

/// How many hex characters of the digest a [`pagination_scope`] carries: short
/// enough to keep a token small, wide enough that two enumerations of one service
/// do not collide.
const PAGINATION_SCOPE_LENGTH: usize = 16;

/// Where a page of a BATCH read resumes: the entry still being answered and how
/// many of that entry's data points earlier pages already carried.
///
/// ONE recipe for every batch read of this service that pages ACROSS its entries
/// (`BatchGetAssetPropertyAggregates`, `BatchGetAssetPropertyValueHistory`), so
/// the token format and what counts as a token this service never issued cannot
/// drift between them.
#[derive(Debug, Clone, Copy)]
pub struct BatchCursor {
    /// Index into the request's `entries` of the first entry not yet finished.
    pub entry: usize,
    /// How many of THAT entry's data points previous pages already reported.
    pub value: usize,
}

impl BatchCursor {
    /// The cursor of a request that carries no `nextToken`.
    pub const START: Self = Self { entry: 0, value: 0 };

    /// This cursor as a `nextToken`: hex, so the token stays opaque to the caller
    /// and inside the `^[A-Za-z0-9+/=]+$` pattern the paging members of this
    /// service carry.
    pub fn encode(self) -> String {
        ::hex::encode(format!(
            "{}{BATCH_CURSOR_SEPARATOR}{}",
            self.entry, self.value
        ))
    }

    /// The cursor `token` encodes, or `None` for text this service never issued as
    /// one — which its caller answers as the modelled refusal for an invalid
    /// `nextToken`.
    pub fn decode(token: &str) -> Option<Self> {
        let decoded = decode_token_text(token)?;
        let (entry, value) = decoded.split_once(BATCH_CURSOR_SEPARATOR)?;
        Some(Self {
            entry: token_number(entry)?,
            value: token_number(value)?,
        })
    }
}

/// What joins the two halves of a token [`BatchCursor::encode`] builds.
const BATCH_CURSOR_SEPARATOR: char = ':';

/// One page of an ordered enumeration: the half-open `[start, end)` span of it
/// this call answers with, and the token the next call resumes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// Index of the first entry of this page in the ordered enumeration.
    pub start: usize,
    /// Index one past the last entry of this page.
    pub end: usize,
    /// The token that resumes at the entry after this page, or `None` when the
    /// page exhausts the enumeration — "the token for the next set of results,
    /// or null if there are no additional results".
    pub next_token: Option<String>,
}

/// The page of `items` a paginated listing answers with: resume where
/// `next_token` names, take at most `max_results` entries (or
/// `default_max_results` when the caller sends none), and mint the next token
/// from the entry that follows.
///
/// ONE recipe for every listing of this service that pages by a resume KEY taken
/// off its entries, so the token format, the page size and what counts as a
/// token this service never issued cannot drift between operations. `key` is the
/// entry's resume key — unique within the enumeration and stable across calls,
/// which is what makes a token resumable; it is carried in the token hex-encoded,
/// keeping the text inside the `^[A-Za-z0-9+/=]+$` pattern the paging members of
/// this service carry.
///
/// `None` for a `next_token` this service never issued — text that is not the hex
/// of a resume key, hex in a spelling [`encode_page_token`] does not write, or a
/// key no entry of this enumeration carries. Its caller
/// answers that with the modelled refusal for an invalid `nextToken`, rather than
/// with an empty page, which would report absence as success.
pub fn page_of<T, K, S>(
    items: &[T],
    next_token: Option<&str>,
    max_results: Option<i32>,
    default_max_results: i32,
    key: K,
) -> Option<Page>
where
    K: Fn(&T) -> S,
    S: AsRef<str>,
{
    let start = match next_token {
        Some(token) => {
            let resume = decode_page_token(token)?;
            items.iter().position(|item| key(item).as_ref() == resume)?
        }
        None => 0,
    };
    // A page size the model's own range forbids cannot reach a handler — the
    // runtime validates `maxResults` before one runs — so `max(1)` only keeps
    // the slice arithmetic below total.
    let max_results = max_results.unwrap_or(default_max_results).max(1) as usize;
    let end = start.saturating_add(max_results).min(items.len());
    let next_token = items
        .get(end)
        .map(|item| encode_page_token(key(item).as_ref()));
    Some(Page {
        start,
        end,
        next_token,
    })
}

/// A resume key as the opaque `nextToken` that resumes at it.
pub fn encode_page_token(resume: &str) -> String {
    ::hex::encode(resume.as_bytes())
}

/// The resume key `token` encodes, or `None` for text this service never issued
/// as a token.
pub fn decode_page_token(token: &str) -> Option<String> {
    decode_token_text(token)
}

/// The bytes `token` spells, or `None` unless `token` is the ONE spelling this
/// service writes for them.
///
/// Every token of this service is the hex of its resume payload, and hex writes a
/// byte exactly one way, so text is a token this service issued only when
/// re-encoding what it decodes gives back the very text that arrived. Decoding hex
/// alone would also read back every other spelling of the same bytes — the
/// uppercase and mixed-case ones, which no encoder here mints — and serve a page
/// for text the service never issued; the callers below lift the `None` into the
/// modelled refusal for an invalid `nextToken` instead.
pub fn decode_token_bytes(token: &str) -> Option<Vec<u8>> {
    let bytes = ::hex::decode(token).ok()?;
    (::hex::encode(&bytes) == token).then_some(bytes)
}

/// The text `token` spells, under the rule [`decode_token_bytes`] states.
pub fn decode_token_text(token: &str) -> Option<String> {
    String::from_utf8(decode_token_bytes(token)?).ok()
}

/// The number one field of a decoded token holds, or `None` unless `field` is the
/// one spelling this service writes for that number.
///
/// The same rule [`decode_token_bytes`] applies to a token's hex, applied to the
/// numbers inside it: a signed zero, a zero-padded or `+`-prefixed ordinal parses
/// as the same number, but is text no encoder of this service writes, so it is text
/// it never issued as a token.
pub fn token_number<T>(field: &str) -> Option<T>
where
    T: ::std::str::FromStr + ::std::fmt::Display,
{
    let value: T = field.parse().ok()?;
    (value.to_string() == field).then_some(value)
}

/// The `Quality` a TQV that names none carries.
///
/// `AssetPropertyValue$quality` is optional and the enum's values are `GOOD`,
/// `BAD` and `UNCERTAIN` (`smithy_query(query="shape", shape="Quality")`). An
/// ordinary device reading is `GOOD`, so a TQV ingested without an explicit
/// quality is treated as `GOOD`: an omitted quality and an explicit `GOOD` name
/// the SAME quality, and neither can shadow the other at a timestamp the series
/// already holds.
pub const DEFAULT_QUALITY: &str = "GOOD";

/// The quality `value` names, with [`DEFAULT_QUALITY`] standing in for an absent
/// one. This is the comparison the quality-aware write in
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::record_property_value`] makes.
pub fn quality_of(value: &AssetPropertyValue) -> &str {
    value.quality.as_deref().unwrap_or(DEFAULT_QUALITY)
}

/// Which stored data stream a request's identifiers name.
///
/// A time series is either ASSOCIATED with an asset property \u2014 the pair of ids
/// names it, and so does any alias bound to that property \u2014 or DISASSOCIATED,
/// which only its alias names. Resolved once in
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::resolve_alias_series`] so every
/// operation addressed by an alias reads and writes the same stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeriesKey {
    /// The stream of the `(assetId, propertyId)` asset property, by stored ids.
    Property(String, String),
    /// A stream no asset property owns, by the alias that identifies it.
    Disassociated(String),
}

/// Put `value` into `series`, which stays sorted ascending by timestamp.
///
/// A value at an already-stored timestamp is QUALITY-AWARE: "For each asset
/// property, IoT SiteWise overwrites TQVs with duplicate timestamps unless the
/// newer TQV has a different quality" (the `BatchPutAssetPropertyValue` model
/// doc), so the stored sample is replaced only when the two name the same
/// quality, and a differing quality leaves the series exactly as it was. Either
/// way the series stays single-valued at that timestamp.
///
/// The series is BOUNDED at [`MAX_SAMPLES_PER_SERIES`]: past that many samples the
/// oldest goes, so no sequence of writes can grow one series without limit. A
/// sample newer than everything stored \u2014 the shape real ingest takes \u2014 takes an
/// append-only fast path and costs no element move, so the sorted insert cannot be
/// driven to quadratic cost under the shared state lock.
fn record_sample(series: &mut Vec<AssetPropertyValue>, value: AssetPropertyValue) {
    let key = timestamp_key(&value.timestamp);
    if series
        .last()
        .is_none_or(|last| timestamp_key(&last.timestamp) < key)
    {
        series.push(value);
    } else {
        match series.binary_search_by(|existing| timestamp_key(&existing.timestamp).cmp(&key)) {
            Ok(at) => {
                if quality_of(&series[at]) != quality_of(&value) {
                    // Accepted and discarded: the stored TQV wins at a timestamp a
                    // differing-quality TQV re-puts.
                    return;
                }
                series[at] = value;
            }
            Err(at) => series.insert(at, value),
        }
    }
    if series.len() > MAX_SAMPLES_PER_SERIES {
        let excess = series.len() - MAX_SAMPLES_PER_SERIES;
        series.drain(..excess);
    }
}

/// A wall-clock instant as the modelled `Timestamp` type.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// The region to build this request's ARNs in: the one in the caller's SigV4
/// credential scope, or [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The canonical ARN of an asset model in `region`.
pub fn asset_model_arn(region: &str, asset_model_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("asset-model/{asset_model_id}"))
}

/// The canonical ARN of an asset in `region`.
pub fn asset_arn(region: &str, asset_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("asset/{asset_id}"))
}

impl ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// The ARN a refusal names the stored asset `asset_id` by: the one that
    /// asset CARRIES, read back from the record.
    ///
    /// An ARN is minted once, by the call that created the resource, and stamped
    /// with the region THAT call was signed for; it is then a member of the
    /// resource, reported identically to every later reader. So an error whose
    /// `resourceId`/`resourceArn` point at a resource of this store reports that
    /// stored ARN rather than one rebuilt from the refused request, which would
    /// name a resource no reader of this store can find whenever the two calls
    /// reached it through different regional endpoints.
    ///
    /// Only an id this store holds no asset for leaves nothing to read, and then
    /// the canonical ARN of that id in `region` is the one honest answer — the
    /// case a refusal naming a resource its own call removed is always in.
    pub fn stored_asset_arn(&self, region: &str, asset_id: &str) -> String {
        self.assets
            .get(asset_id)
            .map(|asset| asset.arn.clone())
            .unwrap_or_else(|| asset_arn(region, asset_id))
    }

    /// [`Self::stored_asset_arn`] for an asset model: the ARN the stored model
    /// carries, or the canonical one in `region` when this store holds no such
    /// model.
    pub fn stored_asset_model_arn(&self, region: &str, asset_model_id: &str) -> String {
        self.asset_models
            .get(asset_model_id)
            .map(|model| model.arn.clone())
            .unwrap_or_else(|| asset_model_arn(region, asset_model_id))
    }
}

/// The resource type and resource id an `AmazonResourceName` names, when the
/// string is an ARN of THIS service in the `<type>/<id>` resource form every
/// IoT SiteWise ARN takes (`asset/<assetId>`, `asset-model/<assetModelId>`,
/// `portal/<portalId>`, …).
///
/// `None` for anything that is not such an ARN — a string that is not an ARN at
/// all, or one whose service segment is another service's. The resource type is
/// returned as it was written rather than matched here, so a caller decides
/// which types it holds.
pub fn parse_resource_arn(arn: &str) -> Option<(&str, &str)> {
    // `arn:<partition>:<service>:<region>:<account>:<resource>` — the resource
    // section is the only one that may itself contain a `:`, so it takes the
    // remainder.
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let _partition = fields.next()?;
    if fields.next()? != ARN_SERVICE {
        return None;
    }
    let _region = fields.next()?;
    let _account = fields.next()?;
    let (resource_type, id) = fields.next()?.split_once('/')?;
    if resource_type.is_empty() || id.is_empty() {
        return None;
    }
    Some((resource_type, id))
}

/// The tag-key namespace Amazon Web Services keeps for the tags it applies
/// itself.
///
/// The tag restrictions the `tags` member of every create links to — "Tagging
/// your IoT SiteWise resources" in the IoT SiteWise User Guide — state that this
/// prefix is reserved for Amazon Web Services use and that a caller can neither
/// set nor remove a tag whose key carries it. `TagKey` declares `@length` and no
/// `@pattern`, so no boundary constraint can express the reservation and it is
/// enforced in [`check_tag_key`] below instead, which is what every operation
/// taking caller-supplied tag keys calls.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` lies in the namespace [`RESERVED_TAG_KEY_PREFIX`] names.
///
/// Matched without regard to ASCII case, because the reservation is of the
/// NAMESPACE: a differently cased spelling of the prefix names the same reserved
/// tags, so it may not be used to slip one past the check. The prefix is matched
/// on the key's leading bytes, so a key whose first characters are multi-byte —
/// and therefore cannot spell the ASCII prefix — is not in the namespace.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.as_bytes()
        .get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX.as_bytes()))
}

/// Why a caller-supplied `key` in the reserved namespace is refused, naming the
/// key the request carried so a caller sending several tags learns which one
/// failed.
pub fn reserved_tag_key_message(key: &str) -> String {
    format!(
        "The tag key '{key}' is not valid: tag keys beginning with \
         '{RESERVED_TAG_KEY_PREFIX}' are reserved for Amazon Web Services use."
    )
}

/// Refuse a tag key a caller may not supply: one outside the modelled `TagKey`
/// bounds, in the wording the boundary validator uses for a map's keys, or one in
/// the namespace [`RESERVED_TAG_KEY_PREFIX`] names.
///
/// The framework's constraint walk reaches a map's VALUES but not its KEYS, so
/// `TagMap`'s `@length(min: 1, max: 128)` on `TagKey` is the handler's to
/// enforce; the length is in characters, as `@length` counts them. The reserved
/// namespace is no constraint the model can carry at all, so it is enforced here
/// too — one function, so every operation that accepts caller-supplied tag keys
/// answers one key the same way, whether the keys arrive on a create or on
/// `TagResource`.
pub fn check_tag_key(key: &str) -> Result<(), String> {
    let length = key.chars().count();
    if !(TAG_KEY_MIN_LENGTH..=TAG_KEY_MAX_LENGTH).contains(&length) {
        return Err(format!(
            "1 validation error detected: Value at 'tags' failed to satisfy constraint: \
             Map keys must satisfy constraint: [Member must have length greater than or equal \
             to {TAG_KEY_MIN_LENGTH}, Member must have length less than or equal to \
             {TAG_KEY_MAX_LENGTH}]"
        ));
    }
    if is_reserved_tag_key(key) {
        return Err(reserved_tag_key_message(key));
    }
    Ok(())
}

/// Refuse a caller-supplied value for a member whose modelled documentation
/// states the value IS JSON — a dashboard's definition literal, a gateway
/// capability's configuration document — unless it parses as a JSON document.
///
/// Such a member is held verbatim and reported back verbatim, so a body that is
/// no JSON document defines nothing the client that later reads it could render
/// or apply. No modelled constraint can express the JSON form — the length
/// bounds and patterns the boundary validator enforces before a handler runs say
/// nothing about it — so it is the handler's to check, and one function so every
/// member the documented form holds for is answered the same way. `member` is
/// the member's wire name, which is what the refusal names.
pub fn check_json_document(member: &str, value: &str) -> Result<(), String> {
    if ::serde_json::from_str::<::serde_json::Value>(value).is_err() {
        return Err(format!("The {member} must be a JSON document."));
    }
    Ok(())
}

/// The canonical ARN of a gateway in `region`, in the form `CreateGateway`
/// documents: `arn:${Partition}:iotsitewise:${Region}:${Account}:gateway/${GatewayId}`.
pub fn gateway_arn(region: &str, gateway_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("gateway/{gateway_id}"))
}

/// The canonical ARN of a portal in `region`, in the form `CreatePortal`
/// documents: `arn:${Partition}:iotsitewise:${Region}:${Account}:portal/${PortalId}`.
pub fn portal_arn(region: &str, portal_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{PORTAL_RESOURCE_TYPE}/{portal_id}"),
    )
}

/// The canonical ARN of a project in `region`, in the form `CreateProject`
/// documents: `arn:${Partition}:iotsitewise:${Region}:${Account}:project/${ProjectId}`.
pub fn project_arn(region: &str, project_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{PROJECT_RESOURCE_TYPE}/{project_id}"),
    )
}

/// The host every IoT SiteWise Monitor portal is served under, as the one
/// `portalStartUrl` is built from.
///
/// A portal's URL is per-portal — `CreatePortal` documents `portalStartUrl` as
/// "The URL for the IoT SiteWise Monitor portal" — so the portal's own id is the
/// subdomain and this is the suffix it sits under.
const PORTAL_HOST_SUFFIX: &str = "app.iotsitewise.aws";

/// The `portalStartUrl` of the portal `portal_id` names.
///
/// DERIVED from the portal id rather than minted, so it is stable across reads
/// and unique per portal without anything being written on a read. Within the
/// modelled `Url` bounds (1..256, `^(http|https)://\S+$`) for every `ID`, which
/// is 36 characters.
pub fn portal_start_url(portal_id: &str) -> String {
    format!("https://{portal_id}.{PORTAL_HOST_SUFFIX}")
}

/// Where a portal's logo image is reported to live: a path under that portal's
/// own URL, keyed by the image id the create minted.
///
/// Derived for the same reason as [`portal_start_url`], and from the portal that
/// owns the image rather than from any store this emulator does not run.
pub fn portal_logo_image_url(portal_id: &str, image_id: &str) -> String {
    format!("{}/logo/{image_id}", portal_start_url(portal_id))
}

/// The canonical ARN of an access policy in `region`, in the form
/// `CreateAccessPolicy` documents:
/// `arn:${Partition}:iotsitewise:${Region}:${Account}:access-policy/${AccessPolicyId}`.
pub fn access_policy_arn(region: &str, access_policy_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{ACCESS_POLICY_RESOURCE_TYPE}/{access_policy_id}"),
    )
}

/// The canonical ARN of a dataset in `region`, in the form `CreateDataset`
/// documents: `arn:${Partition}:iotsitewise:${Region}:${Account}:dataset/${DatasetId}`.
pub fn dataset_arn(region: &str, dataset_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{DATASET_RESOURCE_TYPE}/{dataset_id}"),
    )
}

/// The character joining the two ids a `TimeSeriesId` is built from; see
/// [`time_series_id`].
///
/// A `.`, because the id is carried inside `timeSeriesArn` and the modelled `ARN`
/// pattern is `^arn:aws(-cn|-us-gov)?:[a-zA-Z0-9-:\/_\.]+$` — a separator outside
/// that character class would make every ARN this service reports violate a
/// constraint its own model states.
pub const TIME_SERIES_ID_SEPARATOR: char = '.';

/// The `TimeSeriesId` of the time series (data stream) an asset property owns.
///
/// Every asset property IS a data stream in IoT SiteWise's time-series store, so
/// the series it owns needs an id that is stable across reads and unique per
/// `(assetId, propertyId)` pair. The modelled `TimeSeriesId` is 36..73 characters
/// — one 36-character `ID`, or two of them joined by a single character — so the
/// pair that owns the series names it, derived rather than minted: nothing has to
/// be written on a read for the id to stay the same. Both ids are modelled `ID`s,
/// whose pattern admits neither [`TIME_SERIES_ID_SEPARATOR`] nor any other
/// character outside `[0-9a-f-]`, so one pair cannot be spelled as another.
pub fn time_series_id(asset_id: &str, property_id: &str) -> String {
    format!("{asset_id}{TIME_SERIES_ID_SEPARATOR}{property_id}")
}

/// The canonical ARN of a time series in `region`, in the form the
/// `TimeSeriesSummary$timeSeriesArn` doc states:
/// `arn:${Partition}:iotsitewise:${Region}:${Account}:time-series/${TimeSeriesId}`.
pub fn time_series_arn(region: &str, time_series_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("time-series/{time_series_id}"))
}

/// The MQTT topic IoT SiteWise publishes one asset property's value updates to.
///
/// The form is the one the IoT SiteWise User Guide documents for property-value
/// update notifications ("Interacting with other AWS services"):
/// `$aws/sitewise/asset-models/{assetModelId}/assets/{assetId}/properties/{propertyId}`.
/// Built from the three ids, so the topic a property reports always names the
/// property it belongs to rather than being stored and able to drift.
pub fn property_notification_topic(
    asset_model_id: &str,
    asset_id: &str,
    property_id: &str,
) -> String {
    format!(
        "$aws/sitewise/asset-models/{asset_model_id}/assets/{asset_id}/properties/{property_id}"
    )
}

/// The `PropertyNotificationState` a property takes when `UpdateAssetProperty`
/// omits one: the model's own documented default for that member.
pub const NOTIFICATION_STATE_DISABLED: &str = "DISABLED";

// ---------------------------------------------------------------------------
// Selecting one series' stored samples.
//
// A history read asks the same question of a data stream whether it names one
// property or a batch of them: which stored samples fall inside the queried
// range, which qualities are kept and which way round they are reported. The
// selection therefore lives here, beside the series it reads, so
// `GetAssetPropertyValueHistory` and `BatchGetAssetPropertyValueHistory` cannot
// disagree about what a window covers or what a quality filter keeps.
// ---------------------------------------------------------------------------

/// What one history read asks of one stored series.
///
/// Built only by [`HistoryQuery::new`], so a window whose end precedes its start
/// — which names no range at all — cannot reach a series: the constructor hands
/// back `None` for it instead.
#[derive(Debug, Clone, Copy)]
pub struct HistoryQuery<'a> {
    /// The EXCLUSIVE start of the queried range, in nanoseconds of Unix epoch
    /// time: "The exclusive start of the range from which to query historical
    /// data" (the `startDate` member of either history reader). `None` leaves
    /// that side of the range open.
    start_exclusive_nanos: Option<i128>,
    /// The INCLUSIVE end of the queried range: "The inclusive end of the range
    /// from which to query historical data" (the `endDate` member). `None`
    /// leaves that side open.
    end_inclusive_nanos: Option<i128>,
    /// The qualities a sample may carry to be reported — "the quality by which
    /// to filter asset data". A read that names none asks for every sample: a
    /// history read reports the samples themselves, so an absent filter filters
    /// nothing out.
    qualities: Option<&'a [String]>,
    /// `true` for `TimeOrdering` DESCENDING; ASCENDING is the modelled default.
    descending: bool,
}

impl<'a> HistoryQuery<'a> {
    /// The query a request's window, quality filter and ordering name, or `None`
    /// when `end_date` PRECEDES `start_date` — the caller's cue to refuse the
    /// request (or the batch entry) with the modelled `InvalidRequestException`
    /// rather than answer an impossible range with an empty history.
    pub fn new(
        start_date: Option<&::aws_smithy_types::DateTime>,
        end_date: Option<&::aws_smithy_types::DateTime>,
        qualities: Option<&'a [String]>,
        descending: bool,
    ) -> Option<Self> {
        let start_exclusive_nanos = start_date.map(date_time_nanos);
        let end_inclusive_nanos = end_date.map(date_time_nanos);
        if let (Some(start), Some(end)) = (start_exclusive_nanos, end_inclusive_nanos) {
            if end < start {
                return None;
            }
        }
        Some(Self {
            start_exclusive_nanos,
            end_inclusive_nanos,
            qualities,
            descending,
        })
    }

    /// Whether `sample` survives this read's quality filter. The quality a
    /// sample stored without one counts as comes from the shared
    /// [`quality_of`], so this filter and the quality-aware write in
    /// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::record_property_value`]
    /// cannot disagree about it.
    fn keeps_quality(&self, sample: &AssetPropertyValue) -> bool {
        match self.qualities {
            // A filter naming no quality — absent, or the empty list — asks for
            // all of them.
            None | Some([]) => true,
            Some(wanted) => {
                let quality = quality_of(sample);
                wanted.iter().any(|candidate| candidate == quality)
            }
        }
    }
}

/// The stored samples of the stream `stream` names that `query` selects, in the
/// order it asks for — what either history reader reports for one data stream,
/// associated with an asset property or not.
///
/// The stored series is kept sorted ascending by timestamp, so the range and the
/// quality filter select from it in order and ASCENDING needs no sort at all; a
/// series is bounded at [`MAX_SAMPLES_PER_SERIES`], so the work one read can ask
/// for is bounded however wide a range it names.
pub fn history_values(
    store: &ComAmazonawsIotsitewiseAwsiotsitewiseState,
    stream: &SeriesKey,
    query: &HistoryQuery<'_>,
) -> Vec<AssetPropertyValue> {
    let mut selected: Vec<AssetPropertyValue> = store
        .series(stream)
        .iter()
        .filter(|sample| {
            let at = timestamp_nanos(&sample.timestamp);
            query.start_exclusive_nanos.is_none_or(|start| at > start)
                && query.end_inclusive_nanos.is_none_or(|end| at <= end)
        })
        .filter(|sample| query.keeps_quality(sample))
        .cloned()
        .collect();
    if query.descending {
        selected.reverse();
    }
    selected
}

// ---------------------------------------------------------------------------
// Deriving one series' aggregates.
//
// HOISTED HERE from `src/ops/batch_get_asset_property_aggregates.rs`, which asked
// for exactly that once a second caller appeared: "the fixer that implements
// [`GetAssetPropertyAggregates`] should hoist this block into `crate::state` and
// call it from both". `GetAssetPropertyAggregates` and
// `BatchGetAssetPropertyAggregates` now read a series through this one engine, so
// the two cannot disagree about what a window covers or what it reduces to.
// ---------------------------------------------------------------------------

/// Every `Resolution` an aggregate query may name, with the length in seconds of
/// the window it names.
///
/// Source: the `Resolution` shape — `smithy_query(query="shape",
/// shape="Resolution")` reports length 2..3 and pattern `^1m|15m|1h|1d$`, i.e.
/// the four names below ("1m" one minute, "15m" fifteen minutes, "1h" one hour,
/// "1d" one day). The window LENGTHS are what the generated `Resolution` string
/// does not carry, which is why this table exists at all.
pub const RESOLUTIONS: [(&str, i64); 4] =
    [("1m", 60), ("15m", 900), ("1h", 3_600), ("1d", 86_400)];

/// The window length, in seconds, that `resolution` names — `None` when it is not
/// one of [`RESOLUTIONS`].
pub fn resolution_seconds(resolution: &str) -> Option<i64> {
    RESOLUTIONS
        .iter()
        .find(|(name, _)| *name == resolution)
        .map(|(_, seconds)| *seconds)
}

/// A `TimeInNanos` as whole nanoseconds since the Unix epoch, so a sample and a
/// modelled `Timestamp` bound compare at the precision the sample carries.
///
/// Also what an operation that has to do ARITHMETIC on a span reads it through
/// (the instant inside a data segment `GetSearchResults` reports), with
/// [`timestamp_at_nanos`] the inverse — the service's conversion pair, so a
/// handler that converts a whole instant uses these rather than writing the
/// arithmetic again. The interpolation path keeps its own inverse, because an
/// instant the model cannot express has to make that operation SKIP the interval
/// rather than report a saturated one.
pub fn timestamp_nanos(timestamp: &TimeInNanos) -> i128 {
    i128::from(timestamp.time_in_seconds) * NANOS_PER_SECOND
        + i128::from(timestamp.offset_in_nanos.unwrap_or(0))
}

/// How many nanoseconds one whole second is, as the two members of a
/// `TimeInNanos` divide an instant: `timeInSeconds` whole seconds plus an
/// `offsetInNanos` of `0..=999999999`.
const NANOS_PER_SECOND: i128 = 1_000_000_000;

/// The inverse of [`timestamp_nanos`]: whole nanoseconds since the Unix epoch as
/// the modelled `TimeInNanos` — whole seconds plus the nanosecond offset inside
/// that second.
///
/// The split is EUCLIDEAN, so the offset is never negative and never the
/// `1_000_000_000` the modelled `OffsetInNanos` range (0..999999999) excludes,
/// whichever side of the epoch the instant falls.
pub fn timestamp_at_nanos(nanos: i128) -> TimeInNanos {
    let seconds = nanos.div_euclid(NANOS_PER_SECOND);
    TimeInNanos {
        // Saturating in BOTH directions: an instant too far past the epoch reports
        // the largest second the member can carry and one too far before it the
        // smallest, rather than the wrong end of the range.
        time_in_seconds: i64::try_from(seconds)
            .unwrap_or(if seconds.is_negative() { i64::MIN } else { i64::MAX }),
        offset_in_nanos: i32::try_from(nanos.rem_euclid(NANOS_PER_SECOND)).ok(),
    }
}

/// A modelled `Timestamp` as whole nanoseconds since the Unix epoch; see
/// [`timestamp_nanos`].
pub fn date_time_nanos(at: &::aws_smithy_types::DateTime) -> i128 {
    i128::from(at.secs()) * NANOS_PER_SECOND + i128::from(at.subsec_nanos())
}

/// What one aggregate query asks of one stored series: the window grid to bucket
/// its samples into, the range to read, the quality to count and which aggregates
/// to compute.
///
/// Built only by [`AggregateQuery::new`], so `window_seconds` can only ever be a
/// length [`resolution_seconds`] returned — the invariant the bucketing arithmetic
/// relies on is therefore unforgeable rather than merely documented.
#[derive(Debug, Clone, Copy)]
pub struct AggregateQuery<'a> {
    /// The window length in seconds. Windows are aligned to the Unix epoch, so
    /// one window covers the same instants whoever asks. Always positive.
    window_seconds: i64,
    /// The EXCLUSIVE start of the queried range, in nanoseconds of Unix epoch
    /// time: "The exclusive start of the range from which to query historical
    /// data" (the `startDate` member of either aggregate operation).
    start_exclusive_nanos: i128,
    /// The INCLUSIVE end of the queried range: "The inclusive end of the range
    /// from which to query historical data" (the `endDate` member).
    end_inclusive_nanos: i128,
    /// The one `Quality` a sample must carry to be counted; an aggregate reports
    /// exactly one quality, and [`DEFAULT_QUALITY`] is what a request naming none
    /// asks for.
    quality: &'a str,
    /// The `AggregateType`s to compute. Every other member of `Aggregates` is
    /// left unset, because the caller did not ask for it.
    aggregate_types: &'a [String],
    /// `true` for `TimeOrdering` DESCENDING; ASCENDING is the modelled default.
    descending: bool,
}

impl<'a> AggregateQuery<'a> {
    /// The query a request's `resolution` names, or `None` when `resolution` is
    /// not one of [`RESOLUTIONS`] — which is the caller's cue to refuse the
    /// request with the modelled `InvalidRequestException` rather than answer it.
    pub fn new(
        resolution: &str,
        start_exclusive_nanos: i128,
        end_inclusive_nanos: i128,
        quality: &'a str,
        aggregate_types: &'a [String],
        descending: bool,
    ) -> Option<Self> {
        Some(Self {
            window_seconds: resolution_seconds(resolution)?,
            start_exclusive_nanos,
            end_inclusive_nanos,
            quality,
            aggregate_types,
            descending,
        })
    }
}

/// The aggregated values of the stream `stream` names under `query` — what either
/// aggregate operation reports for one data stream, associated with an asset
/// property or not.
///
/// DERIVED, never stored: the samples `BatchPutAssetPropertyValue` wrote are
/// bucketed into the epoch-aligned windows `query` names and each bucket is
/// reduced to the aggregates the caller asked for. A window that no in-range
/// sample of the asked-for quality falls in is not reported at all, so the length
/// of the answer is a function of the stored data and not of the width of the
/// queried range.
///
/// The pass is over the SAMPLES, not over the windows the range spans: a series is
/// bounded at [`MAX_SAMPLES_PER_SERIES`], so the work one query can ask for is
/// bounded however wide a range it names.
pub fn aggregated_values(
    store: &ComAmazonawsIotsitewiseAwsiotsitewiseState,
    stream: &SeriesKey,
    query: &AggregateQuery<'_>,
) -> Vec<AggregatedValue> {
    let series = store.series(stream);
    if series.is_empty() {
        return Vec::new();
    }

    // The series is kept sorted ascending, so one window's samples are contiguous
    // and a single `last_mut()` bucket is all the grouping needs.
    let mut windows: Vec<(i64, Bucket)> = Vec::new();
    for sample in series {
        let at = timestamp_nanos(&sample.timestamp);
        if at <= query.start_exclusive_nanos || at > query.end_inclusive_nanos {
            continue;
        }
        if quality_of(sample) != query.quality {
            continue;
        }
        // A `nullValue` reports a reading that is absent, so it is not a data
        // point of the window it falls in.
        if sample.value.null_value.is_some() {
            continue;
        }
        let seconds = sample.timestamp.time_in_seconds;
        // `window_seconds` is positive by construction (see
        // [`AggregateQuery::new`]), so this is total.
        let start = seconds.saturating_sub(seconds.rem_euclid(query.window_seconds));
        if windows.last().map(|(at, _)| *at) != Some(start) {
            windows.push((start, Bucket::default()));
        }
        if let Some((_, bucket)) = windows.last_mut() {
            bucket.count = bucket.count.saturating_add(1);
            if let Some(numeric) = numeric_value(&sample.value) {
                bucket.numeric.push(numeric);
            }
        }
    }

    let mut values: Vec<AggregatedValue> = windows
        .iter()
        .filter_map(|(start, bucket)| {
            let value = bucket.reduce(query.aggregate_types);
            reports_an_aggregate(&value).then(|| AggregatedValue {
                // A window is reported at the instant it starts.
                timestamp: ::aws_smithy_types::DateTime::from_secs(*start),
                quality: Some(query.quality.to_string()),
                value,
            })
        })
        .collect();
    // ASCENDING is the modelled default; DESCENDING is the same windows read the
    // other way.
    if query.descending {
        values.reverse();
    }
    values
}

/// One window's contributing samples, accumulated in a single pass over the
/// series.
#[derive(Debug, Default)]
struct Bucket {
    /// How many data points fell in the window, whatever their type: `count` is
    /// the count of the data points the window's aggregates were computed from.
    count: u64,
    /// The numeric value of each sample that has one. A `STRING` or `BOOLEAN`
    /// sample has none, so it is counted but cannot be summed or averaged.
    numeric: Vec<f64>,
}

impl Bucket {
    /// This window reduced to the requested aggregates. An aggregate the caller
    /// did not ask for is left unset, and so is a numeric aggregate that no sample
    /// in the window can feed.
    ///
    /// The arms name the `AggregateType` members the model declares, generated as
    /// `AGGREGATE_TYPE_VALUES` in `crate::ops::types` — which is what each
    /// operation checks a request against before a series is read.
    fn reduce(&self, aggregate_types: &[String]) -> Aggregates {
        let samples = self.numeric.len();
        let sum: f64 = self.numeric.iter().sum();
        let mean = (samples > 0).then(|| sum / samples as f64);
        let mut aggregates = Aggregates::default();
        for aggregate_type in aggregate_types {
            match aggregate_type.as_str() {
                "COUNT" => aggregates.count = Some(self.count as f64),
                "SUM" => aggregates.sum = (samples > 0).then_some(sum),
                "AVERAGE" => aggregates.average = mean,
                "MAXIMUM" => aggregates.maximum = self.numeric.iter().copied().reduce(f64::max),
                "MINIMUM" => aggregates.minimum = self.numeric.iter().copied().reduce(f64::min),
                // The POPULATION standard deviation: a window's data points are
                // all of that window's data, so nothing is being sampled from a
                // wider set and there is no degree of freedom to lose.
                "STANDARD_DEVIATION" => {
                    aggregates.standard_deviation = mean.map(|mean| {
                        (self
                            .numeric
                            .iter()
                            .map(|value| (value - mean) * (value - mean))
                            .sum::<f64>()
                            / samples as f64)
                            .sqrt()
                    });
                }
                // Unreachable: each operation checks a request's aggregate types
                // against the generated catalogue before the series is read.
                _ => {}
            }
        }
        aggregates
    }
}

/// Whether `aggregates` reports any value at all. A window that can feed none of
/// the requested aggregates is not reported, rather than reported as an empty
/// `Aggregates`.
fn reports_an_aggregate(aggregates: &Aggregates) -> bool {
    aggregates.average.is_some()
        || aggregates.count.is_some()
        || aggregates.maximum.is_some()
        || aggregates.minimum.is_some()
        || aggregates.sum.is_some()
        || aggregates.standard_deviation.is_some()
}

/// The numeric value a `Variant` holds, or `None` when it holds none. A `Variant`
/// sets exactly one member, and only `integerValue` and `doubleValue` are numbers.
fn numeric_value(value: &Variant) -> Option<f64> {
    value
        .double_value
        .or_else(|| value.integer_value.map(f64::from))
}

/// How many ids this process has minted. Part of every id's seed, so no two
/// minted ids collide even when two calls land in the same nanosecond.
static MINTED: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

/// A fresh UUIDv4 in the lowercase hyphenated form every `ID` member models.
///
/// Built the way the other services in this workspace mint ids (cf. `mint_hex` in
/// the ivs service): a seed of a monotonic mint counter, the wall clock and the
/// process id, hashed with SHA-256, then given RFC 4122's version-4 and variant
/// bits. Setting the version nibble also makes the all-zero UUID — the one value
/// the modelled `ID` pattern refuses — unreachable.
pub fn new_id() -> String {
    let minted = MINTED.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let seed = format!("iotsitewise-id|{minted}|{}|{nanos}", ::std::process::id());
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(seed.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The `ConfigurationState` of a configuration that is in force.
pub const CONFIGURATION_STATE_ACTIVE: &str = "ACTIVE";

/// The `ConfigurationState` of a configuration a put has accepted and the
/// service has not finished applying yet.
pub const CONFIGURATION_STATE_UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";

/// The resource prefix of a KMS alias, in both the bare `alias/<name>` form and
/// inside an alias ARN.
const KMS_ALIAS_PREFIX: &str = "alias/";

/// The resource prefix of a KMS key's ARN.
const KMS_KEY_PREFIX: &str = "key/";

/// The number of digits in an Amazon Web Services account id, which the account
/// field of an ARN carries.
const ACCOUNT_ID_DIGITS: usize = 12;

/// The number of dash-separated groups in the id KMS mints for a single-Region
/// key.
const KEY_ID_GROUPS: usize = 5;

/// The `mrk-` prefix of the id KMS mints for a multi-Region key, and the number
/// of hexadecimal digits that follow it.
const MULTI_REGION_KEY_PREFIX: &str = "mrk-";
const MULTI_REGION_KEY_ID_DIGITS: usize = 32;

/// The longest ARN this service reports, from the modelled `ARN` type
/// (`len 1..1600`): a key identifier whose ARN would be longer than this names a
/// key the service could not report at all, since `kmsKeyArn` is where it would
/// be reported.
pub const ARN_MAX_LENGTH: usize = 1600;

/// Whether `candidate` is a KMS key id.
///
/// The emulator resolves no KMS key, so only the FORM is judged, and no more
/// narrowly than the id's two shapes: [`KEY_ID_GROUPS`] non-empty groups of
/// hexadecimal digits for a single-Region key, or [`MULTI_REGION_KEY_PREFIX`]
/// and [`MULTI_REGION_KEY_ID_DIGITS`] hexadecimal digits for a multi-Region one.
fn is_kms_key_id(candidate: &str) -> bool {
    let hexadecimal =
        |group: &str| !group.is_empty() && group.bytes().all(|byte| byte.is_ascii_hexdigit());
    if let Some(digits) = candidate.strip_prefix(MULTI_REGION_KEY_PREFIX) {
        return digits.len() == MULTI_REGION_KEY_ID_DIGITS && hexadecimal(digits);
    }
    let mut groups = 0usize;
    for group in candidate.split('-') {
        if !hexadecimal(group) {
            return false;
        }
        groups += 1;
    }
    groups == KEY_ID_GROUPS
}

/// Whether `candidate` is a KMS alias path: [`KMS_ALIAS_PREFIX`] followed by a
/// non-empty name of the characters a KMS alias name admits (`[a-zA-Z0-9:/_-]`).
fn is_kms_alias(candidate: &str) -> bool {
    candidate.strip_prefix(KMS_ALIAS_PREFIX).is_some_and(|name| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b":/_-".contains(&byte))
    })
}

/// Whether `candidate` is the full ARN of a KMS key or alias —
/// `arn:<partition>:kms:<region>:<account>:key/<key id>` or `…:alias/<name>`.
fn is_kms_arn(candidate: &str) -> bool {
    let mut fields = candidate.splitn(6, ':');
    let (Some("arn"), Some(partition), Some(KMS_ARN_SERVICE), Some(region), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    !partition.is_empty()
        && !region.is_empty()
        && account.len() == ACCOUNT_ID_DIGITS
        && account.bytes().all(|byte| byte.is_ascii_digit())
        && (is_kms_alias(resource) || resource.strip_prefix(KMS_KEY_PREFIX).is_some_and(is_kms_key_id))
}

/// The ARN of the KMS key the identifier `key_id` selects, or `None` when it
/// selects none.
///
/// A key identifier comes in the forms AWS KMS accepts for one: a key id, an
/// `alias/<name>` path, or the full ARN of either. The ARN itself is built by
/// [`kms_key_arn`], the one normalisation this service has for a key identifier;
/// what this adds is the JUDGEMENT a member that must carry an ARN needs — the
/// emulator resolves no KMS key, so the form of the identifier is all there is to
/// judge, and an identifier in none of those forms, or one whose ARN names a
/// partition or a Region the modelled `ARN` pattern excludes (see
/// [`is_modelled_arn`]), selects nothing this service could report.
pub fn selected_kms_key_arn(region: &str, key_id: &str) -> Option<String> {
    if !is_kms_key_identifier(key_id) {
        return None;
    }
    let arn = kms_key_arn(region, key_id);
    is_modelled_arn(&arn).then_some(arn)
}

/// Whether `key_id` is a KMS key identifier at all: a key id, an `alias/<name>`
/// path, or the full ARN of either.
fn is_kms_key_identifier(key_id: &str) -> bool {
    if key_id.starts_with("arn:") {
        return is_kms_arn(key_id);
    }
    is_kms_alias(key_id) || is_kms_key_id(key_id)
}

/// The ARN prefixes the modelled `ARN` type's pattern admits, one per partition
/// it names: `^arn:aws(-cn|-us-gov)?:`.
const ARN_PARTITION_PREFIXES: [&str; 3] = ["arn:aws:", "arn:aws-cn:", "arn:aws-us-gov:"];

/// Whether `value` is an ARN this service may report: the whole modelled `ARN`
/// pattern, `^arn:aws(-cn|-us-gov)?:[a-zA-Z0-9-:\/_\.]+$`.
///
/// A caller-supplied ARN is read against this before it is reported back in an
/// ARN member, because a value outside the pattern is one no member of that type
/// can carry, whatever the resource it names.
pub fn is_modelled_arn(value: &str) -> bool {
    ARN_PARTITION_PREFIXES
        .into_iter()
        .find_map(|prefix| value.strip_prefix(prefix))
        .is_some_and(|rest| {
            !rest.is_empty()
                && rest
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-:/_.".contains(&byte))
        })
}

/// The default encryption configuration a put stored: which encryption the
/// account selected, the key it selected for it, and when the configuration the
/// put accepted falls due to be in force.
#[derive(Debug, Clone)]
pub struct DefaultEncryptionRecord {
    /// The modelled `EncryptionType` the put named.
    pub encryption_type: String,
    /// The ARN of the KMS key the put selected (see [`kms_key_arn`]); `None`
    /// under [`ENCRYPTION_TYPE_SITEWISE_DEFAULT`], which uses no customer managed key.
    pub kms_key_arn: Option<String>,
    /// When [`CONFIGURATION_STATE_UPDATE_IN_PROGRESS`] ->
    /// [`CONFIGURATION_STATE_ACTIVE`] falls due: the instant of the put plus
    /// [`ACTIVATION_DELAY`]. Stored rather than slept for, so nothing blocks and
    /// a caller reading straight after the put still observes the state the
    /// service reports while it applies the change.
    pub active_at: SystemTime,
}

/// The `ConfigurationState` a configuration whose transition falls due at
/// `active_at` reports at `now`: in force from that instant, and being applied
/// until it.
///
/// Every account-wide setting this service keeps settles the same way, so the
/// one rule is read from here rather than restated per setting.
pub fn configuration_state(active_at: SystemTime, now: SystemTime) -> &'static str {
    if now >= active_at {
        CONFIGURATION_STATE_ACTIVE
    } else {
        CONFIGURATION_STATE_UPDATE_IN_PROGRESS
    }
}

impl DefaultEncryptionRecord {
    /// The `ConfigurationState` this configuration reports at `now`.
    pub fn state(&self, now: SystemTime) -> &'static str {
        configuration_state(self.active_at, now)
    }
}

/// What the two default-encryption operations report: they answer with the same
/// three members, so they read them from one place.
#[derive(Debug, Clone)]
pub struct DefaultEncryptionConfiguration {
    /// The modelled `encryptionType`.
    pub encryption_type: String,
    /// The modelled `kmsKeyArn`, absent unless the configuration uses a customer
    /// managed key.
    pub kms_key_arn: Option<String>,
    /// The modelled `configurationStatus`.
    pub status: ConfigurationStatus,
}

impl ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// The default encryption configuration the account reports at `now`: the one
    /// the last put stored, or the service-owned-key default while nothing has
    /// stored any.
    ///
    /// A pure READ: an unconfigured account is not materialised, and a
    /// configuration still settling is reported from the instant its put already
    /// recorded, so reading cannot change what a later caller sees.
    pub fn default_encryption_configuration(
        &self,
        now: SystemTime,
    ) -> DefaultEncryptionConfiguration {
        let status = |state: &str| ConfigurationStatus {
            state: state.to_string(),
            // No error: nothing in this emulator fails to apply a configuration,
            // so the modelled `error` details have nothing to report.
            error: None,
        };
        match &self.default_encryption {
            Some(record) => DefaultEncryptionConfiguration {
                encryption_type: record.encryption_type.clone(),
                kms_key_arn: record.kms_key_arn.clone(),
                status: status(record.state(now)),
            },
            None => DefaultEncryptionConfiguration {
                encryption_type: ENCRYPTION_TYPE_SITEWISE_DEFAULT.to_string(),
                kms_key_arn: None,
                status: status(CONFIGURATION_STATE_ACTIVE),
            },
        }
    }

    /// Select `encryption_type` (and `kms_key_arn` with it) as the account's
    /// default encryption configuration, and report what that call answers with.
    ///
    /// The whole setting is replaced: a put is the last word on the account's
    /// encryption, so one naming the service-owned key drops the customer managed
    /// key a previous put selected. The configuration the put accepted falls due
    /// at [`ACTIVATION_DELAY`], the same span this service settles every other
    /// transition the clock owes at.
    pub fn set_default_encryption_configuration(
        &mut self,
        encryption_type: &str,
        kms_key_arn: Option<String>,
        now: SystemTime,
    ) -> DefaultEncryptionConfiguration {
        self.default_encryption = Some(DefaultEncryptionRecord {
            encryption_type: encryption_type.to_string(),
            kms_key_arn,
            active_at: now + ACTIVATION_DELAY,
        });
        self.default_encryption_configuration(now)
    }
}

// ---------------------------------------------------------------------------
// Computation models.
// ---------------------------------------------------------------------------

/// The ARN resource-type segment of a computation model's ARN, as the
/// `computationModelArn` member documents the form:
/// `arn:${Partition}:iotsitewise:${Region}:${Account}:computation-model/${ComputationModelId}`.
pub const COMPUTATION_MODEL_RESOURCE_TYPE: &str = "computation-model";

/// See [`MAX_ASSET_MODELS`]: the same bound, for the computation-model map.
/// `CreateComputationModel` is the only thing that adds a computation model and
/// it declares the modelled `LimitExceededException` (HTTP 410), so that is how
/// this store refuses past the cap. The emulator's own bound, not a published
/// AWS default.
pub const MAX_COMPUTATION_MODELS: usize = 1_000;

/// The `computationModelVersion` a freshly created computation model reports.
///
/// The version counts the writes the model has accepted — the create is the
/// first — so it is derived from the stored record rather than minted, and a
/// read never has to write for it to stay the same. The modelled `Version` type
/// is a decimal string (`^(0|([1-9]{1}\d*))$`), which is how the counter reaches
/// the wire.
pub const COMPUTATION_MODEL_INITIAL_VERSION: u64 = 1;

/// The `ComputationModelType` of a model whose configuration names anomaly
/// detection — the only value the modelled enum has (see
/// [`crate::ops::types::COMPUTATION_MODEL_TYPE_VALUES`]).
pub const COMPUTATION_MODEL_TYPE_ANOMALY_DETECTION: &str = "ANOMALY_DETECTION";

/// The `actionType` of each action an anomaly-detection computation model
/// exposes, as the model NAMES them: `DescribeExecution`'s `executionDetails`
/// documents "the training action (Amazon Web Services/ANOMALY_DETECTION_TRAINING)"
/// and "the inference action (Amazon Web Services/ANOMALY_DETECTION_INFERENCE)"
/// — the two actions such a model publishes, spelled the way the model spells
/// them rather than in a prefix form nothing in the model states.
///
/// The model carries no enum of action types, so this is the catalog for the one
/// [`COMPUTATION_MODEL_TYPE_ANOMALY_DETECTION`] computation type; a model of a
/// computation type this list does not cover exposes no action rather than a
/// guessed one.
pub const ANOMALY_DETECTION_ACTION_TYPES: [&str; 2] = [
    "Amazon Web Services/ANOMALY_DETECTION_TRAINING",
    "Amazon Web Services/ANOMALY_DETECTION_INFERENCE",
];

/// The opening and closing delimiters of the `${VariableName}` reference a
/// computation model's configuration names a data-binding variable with, as the
/// configuration members document the format ("Define the variable name
/// associated with input properties, with the following format
/// `${VariableName}`").
const BINDING_REFERENCE_OPEN: &str = "${";
const BINDING_REFERENCE_CLOSE: char = '}';

/// A stored computation model: everything `DescribeComputationModel` reports,
/// plus the `tags` the tagging operations read and the `clientToken` of the
/// create that made it.
///
/// `settling_state` holds a `ComputationModelState` value, which spells its
/// states exactly as `AssetState` does — `CREATING`, `ACTIVE`, `UPDATING`,
/// `DELETING`, `FAILED` — so this record reuses [`STATE_CREATING`] /
/// [`STATE_ACTIVE`] and the one [`state_at`] rule rather than a second spelling
/// of the same values.
#[derive(Debug, Clone)]
pub struct ComputationModelRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub configuration: ComputationModelConfiguration,
    pub data_binding: HashMap<String, ComputationModelDataBindingValue>,
    /// The actions this model exposes, materialised ONCE by the create from the
    /// computation type its configuration names (see
    /// [`action_definitions_for`]), so every read reports the same ids.
    pub action_definitions: Vec<ActionDefinition>,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    /// The instant the transition the clock owes — from [`Self::settling_state`]
    /// to `ACTIVE` — falls due.
    pub active_at: SystemTime,
    /// The `ComputationModelState` this model reports until [`Self::active_at`]
    /// falls due: [`STATE_CREATING`] for a freshly created model,
    /// [`STATE_UPDATING`] while a mutating call it already accepted settles.
    pub settling_state: String,
    /// How many writes this model has accepted, which is the
    /// `computationModelVersion` it reports; see
    /// [`COMPUTATION_MODEL_INITIAL_VERSION`].
    pub version: u64,
    /// The `clientToken` the create that made this model carried, if it carried
    /// one, recorded as part of what that call asked for. What recognises a
    /// RETRY of that call is the store's registry of spent tokens
    /// ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::idempotent_verdict`]),
    /// which compares the request too: a token alone says only that some call
    /// used it, not that this one repeats it.
    pub client_token: Option<String>,
    pub tags: HashMap<String, String>,
}

impl ComputationModelRecord {
    /// The model's `ComputationModelState` as of `now`: its
    /// [`Self::settling_state`] until the owed transition falls due, `ACTIVE`
    /// from then on. Derived, so a read never has to write.
    pub fn state(&self, now: SystemTime) -> &str {
        state_at(&self.settling_state, self.active_at, now)
    }

    /// The model's status as of `now`.
    pub fn status(&self, now: SystemTime) -> ComputationModelStatus {
        ComputationModelStatus {
            state: self.state(now).to_string(),
            // No error: nothing in this emulator fails to provision a
            // computation model, so the modelled `error` details have nothing
            // to report.
            error: None,
        }
    }

    /// The `computationModelVersion` this model reports: its accepted-write
    /// count as the decimal string the modelled `Version` type is.
    pub fn version_label(&self) -> String {
        self.version.to_string()
    }

    /// Record that a mutating call — an update of this model's definition —
    /// landed at `now`: the definition now stored counts one write more, so it
    /// carries the next version, and the model reports `UPDATING` until the
    /// transition that call owes falls due.
    ///
    /// The due instant is STORED, never slept on, exactly as
    /// [`AssetRecord::begin_update`] does it for an asset, so the state a later
    /// read reports stays derived from the clock.
    pub fn begin_update(&mut self, now: SystemTime) {
        self.last_update_date = now;
        self.version += 1;
        self.settling_state = STATE_UPDATING.to_string();
        self.active_at = now + ACTIVATION_DELAY;
    }

    /// Whether this model answers a read scoped to the version the
    /// `ComputationModelVersionFilter` `filter` names (`LATEST | ACTIVE | <n>`),
    /// as of `now`.
    ///
    /// This emulator keeps ONE version of a computation model, so that version
    /// is always its latest one — which is what an absent filter and `LATEST`
    /// ask for — and is also its ACTIVE one once the transition the clock owes
    /// has fallen due; while one is still pending the model has no active
    /// version at all, since nothing here keeps a definition a write replaced.
    /// An explicit number therefore names a version this store
    /// holds only when it is that one version's own number.
    pub fn serves_version(&self, filter: Option<&str>, now: SystemTime) -> bool {
        match filter {
            None | Some(VERSION_TYPE_LATEST) => true,
            Some(VERSION_TYPE_ACTIVE) => self.state(now) == STATE_ACTIVE,
            Some(explicit) => explicit == self.version_label().as_str(),
        }
    }
}

/// The canonical ARN of a computation model in `region`; see
/// [`COMPUTATION_MODEL_RESOURCE_TYPE`].
pub fn computation_model_arn(region: &str, computation_model_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{COMPUTATION_MODEL_RESOURCE_TYPE}/{computation_model_id}"),
    )
}

/// The `ComputationModelType` `configuration` names, or `None` when it names
/// none.
///
/// `ComputationModelConfiguration` carries one optional member per computation
/// type, so the type of a model is DERIVED from the configuration it was created
/// with rather than stored beside it and able to drift.
pub fn computation_model_type(
    configuration: &ComputationModelConfiguration,
) -> Option<&'static str> {
    configuration
        .anomaly_detection
        .as_ref()
        .map(|_| COMPUTATION_MODEL_TYPE_ANOMALY_DETECTION)
}

/// The action definitions a computation model of `model_type` exposes, each with
/// a freshly minted `actionDefinitionId`.
///
/// Called ONCE, by the create, and stored on the model: the ids are then stable
/// across reads and unique per model, and a read of the model's available
/// actions writes nothing.
pub fn action_definitions_for(model_type: &str) -> Vec<ActionDefinition> {
    let action_types: &[&str] = match model_type {
        COMPUTATION_MODEL_TYPE_ANOMALY_DETECTION => &ANOMALY_DETECTION_ACTION_TYPES,
        _ => &[],
    };
    action_types
        .iter()
        .map(|action_type| ActionDefinition {
            action_definition_id: new_id(),
            // The model documents `actionName` as "The name of the action
            // definition" and `actionType` as "The type of the action
            // definition", and an action of a computation model is addressed by
            // the kind of work it does, so each action is named for its type.
            action_name: (*action_type).to_string(),
            action_type: (*action_type).to_string(),
        })
        .collect()
}

/// The data-binding variable a `${VariableName}` reference names, or `None` when
/// `reference` is not in that form; see [`BINDING_REFERENCE_OPEN`].
fn binding_variable(reference: &str) -> Option<&str> {
    reference
        .strip_prefix(BINDING_REFERENCE_OPEN)?
        .strip_suffix(BINDING_REFERENCE_CLOSE)
        .filter(|variable| !variable.is_empty())
}

/// Refuse a computation model whose configuration and data binding do not
/// describe one model, in the one place every operation that supplies those two
/// shapes holds the rule.
///
/// Three things have to hold:
///
/// * the configuration names a computation type — it carries one optional member
///   per type, and a configuration naming none describes no computation at all;
/// * every `${VariableName}` the configuration references is a variable the data
///   binding defines, which is what the binding is documented to be ("Key is a
///   variable name defined in configuration");
/// * every binding value sets exactly one of its members, which is what
///   `ComputationModelDataBindingValue` documents ("can be one of
///   `assetModelProperty`, `list`") — checked through a `list` as well, since a
///   list's elements are binding values themselves, down to
///   [`MAX_BINDING_VALUE_DEPTH`], past which the value is refused rather than
///   walked further.
pub fn check_computation_model(
    configuration: &ComputationModelConfiguration,
    data_binding: &HashMap<String, ComputationModelDataBindingValue>,
) -> Result<(), String> {
    let Some(model_type) = computation_model_type(configuration) else {
        return Err("computationModelConfiguration must name exactly one computation type."
            .to_string());
    };
    for (member, reference) in configuration_references(configuration) {
        let Some(variable) = binding_variable(reference) else {
            return Err(format!(
                "computationModelConfiguration.{model_type}.{member} must reference a data \
                 binding variable in the form ${{VariableName}}, but is {reference}."
            ));
        };
        if !data_binding.contains_key(variable) {
            return Err(format!(
                "computationModelConfiguration.{model_type}.{member} references the variable \
                 {variable}, which computationModelDataBinding does not define."
            ));
        }
    }
    for (variable, value) in data_binding {
        check_binding_value(variable, value, 1)?;
    }
    Ok(())
}

/// Every `${VariableName}` reference `configuration` makes, as
/// `(member name, reference)` pairs — the members a refusal names.
fn configuration_references(
    configuration: &ComputationModelConfiguration,
) -> Vec<(&'static str, &str)> {
    match configuration.anomaly_detection.as_ref() {
        Some(anomaly_detection) => vec![
            ("inputProperties", anomaly_detection.input_properties.as_str()),
            ("resultProperty", anomaly_detection.result_property.as_str()),
        ],
        None => Vec::new(),
    }
}

/// How deeply a binding value's `list` members may nest.
///
/// A binding value's `list` holds binding values, so the shape a caller sends is
/// as deep as the caller made it; this is the depth past which a value is
/// refused instead of walked, so the walk below cannot be driven by a request
/// into unbounded recursion. The emulator's own bound, not a published AWS one.
pub const MAX_BINDING_VALUE_DEPTH: usize = 8;

/// Refuse a binding value that does not set exactly one member, or that nests
/// deeper than [`MAX_BINDING_VALUE_DEPTH`]; see [`check_computation_model`].
/// `depth` is this value's own nesting level, the outermost being 1.
fn check_binding_value(
    variable: &str,
    value: &ComputationModelDataBindingValue,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_BINDING_VALUE_DEPTH {
        return Err(format!(
            "The computationModelDataBinding value of {variable} nests lists more than \
             {MAX_BINDING_VALUE_DEPTH} deep."
        ));
    }
    let set = [
        value.asset_model_property.is_some(),
        value.asset_property.is_some(),
        value.list.is_some(),
    ]
    .into_iter()
    .filter(|member| *member)
    .count();
    if set != 1 {
        return Err(format!(
            "The computationModelDataBinding value of {variable} must set exactly one of \
             assetModelProperty, assetProperty and list, but sets {set}."
        ));
    }
    for element in value.list.iter().flatten() {
        check_binding_value(variable, element, depth + 1)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The account's storage configuration
//
// `PutStorageConfiguration` "configures storage settings for IoT SiteWise" and
// `DescribeStorageConfiguration` "retrieves information about the storage
// configuration": ONE account-wide setting rather than a resource, so the
// record it is kept in, the rules a selection is read against, and what the two
// operations report all live here once, the way the default encryption
// configuration above does.
// ---------------------------------------------------------------------------

/// The `StorageType` that keeps data in the service-managed hot tier.
///
/// Source: `smithy_query(query="shape", shape="StorageType")` — the enum holds
/// exactly this value and [`STORAGE_TYPE_MULTI_LAYER`]. It is also the tier an
/// account nothing has configured stores into, which is what the `storageType`
/// documentation states of it ("IoT SiteWise saves your data into the hot tier").
pub const STORAGE_TYPE_SITEWISE_DEFAULT: &str = "SITEWISE_DEFAULT_STORAGE";

/// The `StorageType` that keeps data in the hot tier AND in a customer managed
/// Amazon S3 bucket (the cold tier); see [`STORAGE_TYPE_SITEWISE_DEFAULT`].
pub const STORAGE_TYPE_MULTI_LAYER: &str = "MULTI_LAYER_STORAGE";

/// The `DisassociatedDataStorageState` / `WarmTierState` value that turns one of
/// those features on. Both enums hold exactly this value and
/// [`STORAGE_STATE_DISABLED`]
/// (`smithy_query(query="shape", shape="DisassociatedDataStorageState,WarmTierState")`).
pub const STORAGE_STATE_ENABLED: &str = "ENABLED";

/// The value that turns one of them off; see [`STORAGE_STATE_ENABLED`].
pub const STORAGE_STATE_DISABLED: &str = "DISABLED";

/// Whether NULL and NaN ingestion is disallowed for an account whose
/// configuration does not name it, as `disallowIngestNullNaN`'s own
/// documentation states it: "By default the feature is allowed. The feature is
/// disallowed if the value is true."
pub const DEFAULT_DISALLOW_INGEST_NULL_NAN: bool = false;

/// What a caller selects a storage configuration with: the members
/// `PutStorageConfiguration` carries, as ONE value, so the rules over them and
/// the store that keeps them name a single thing rather than seven.
#[derive(Debug, Clone, Default)]
pub struct StorageConfigurationSelection {
    /// The modelled `storageType`, required on the wire.
    pub storage_type: String,
    /// The modelled `multiLayerStorage` — the cold-tier destination, which only
    /// [`STORAGE_TYPE_MULTI_LAYER`] has one.
    pub multi_layer_storage: Option<MultiLayerStorage>,
    /// The modelled `disassociatedDataStorage`.
    pub disassociated_data_storage: Option<String>,
    /// The modelled `retentionPeriod` — how long the hot tier keeps data.
    pub retention_period: Option<RetentionPeriod>,
    /// The modelled `warmTier`.
    pub warm_tier: Option<String>,
    /// The modelled `warmTierRetentionPeriod`.
    pub warm_tier_retention_period: Option<WarmTierRetentionPeriod>,
    /// The modelled `disallowIngestNullNaN`.
    pub disallow_ingest_null_na_n: Option<bool>,
}

/// Refuse a storage configuration this service cannot accept, in the wording the
/// refusal carries; `Ok` for one it can.
///
/// The rules the shapes state, over the selection as a whole:
///
/// * `storageType` is a `StorageType`, `disassociatedDataStorage` a
///   `DisassociatedDataStorageState` and `warmTier` a `WarmTierState`, each read
///   against the generated, model-synced value table the way
///   `PutLoggingOptions` reads its own enum's;
/// * "If you specified MULTI_LAYER_STORAGE for the storage type, you must
///   specify a MultiLayerStorage object", whose two members carry an ARN each;
/// * a retention period names either a number of days or that it is unlimited —
///   "If you specified a value for this parameter, the `unlimited` parameter
///   must be `false`" and "If configured to `true`, you must not specify a value
///   for the `numberOfDays` parameter";
/// * a `warmTierRetentionPeriod` is set "only if cold tier is enabled", as the
///   member's own documentation states — the cold tier being the customer
///   managed bucket [`STORAGE_TYPE_MULTI_LAYER`] keeps a copy in.
///
/// Refusing a `multiLayerStorage` beside the hot-tier-only type is this
/// emulator's own choice rather than a documented rule, and the same one the
/// default encryption configuration makes over `kmsKeyId`: storing the
/// destination would leave the service either reporting a cold-tier destination
/// for a configuration that keeps no cold copy, or silently dropping a member
/// the caller set.
pub fn check_storage_selection(selection: &StorageConfigurationSelection) -> Result<(), String> {
    let storage_type = selection.storage_type.as_str();
    if !STORAGE_TYPE_VALUES.contains(&storage_type) {
        return Err(format!(
            "The storageType {storage_type} is not supported; it must be one of {}.",
            STORAGE_TYPE_VALUES.join(" | ")
        ));
    }
    match (
        storage_type == STORAGE_TYPE_MULTI_LAYER,
        selection.multi_layer_storage.as_ref(),
    ) {
        (true, None) => {
            return Err(format!(
                "A multiLayerStorage is required when storageType is {STORAGE_TYPE_MULTI_LAYER}."
            ))
        }
        (false, Some(_)) => {
            return Err(format!(
                "A multiLayerStorage must be omitted when storageType is \
                 {STORAGE_TYPE_SITEWISE_DEFAULT}."
            ))
        }
        (true, Some(storage)) => check_customer_managed_s3(&storage.customer_managed_s3_storage)?,
        (false, None) => {}
    }
    check_storage_state(
        "disassociatedDataStorage",
        selection.disassociated_data_storage.as_deref(),
        DISASSOCIATED_DATA_STORAGE_STATE_VALUES,
    )?;
    check_storage_state(
        "warmTier",
        selection.warm_tier.as_deref(),
        WARM_TIER_STATE_VALUES,
    )?;
    if let Some(period) = selection.retention_period.as_ref() {
        check_retention_period("retentionPeriod", period.number_of_days, period.unlimited)?;
    }
    if let Some(period) = selection.warm_tier_retention_period.as_ref() {
        if storage_type != STORAGE_TYPE_MULTI_LAYER {
            return Err(format!(
                "A warmTierRetentionPeriod can be set only when storageType is \
                 {STORAGE_TYPE_MULTI_LAYER}."
            ));
        }
        check_retention_period(
            "warmTierRetentionPeriod",
            period.number_of_days,
            period.unlimited,
        )?;
    }
    Ok(())
}

/// Refuse a value outside the enum `member` is modelled as, by that enum's
/// generated value table. An absent member selects nothing and is left to the
/// documented default.
fn check_storage_state(member: &str, value: Option<&str>, values: &[&str]) -> Result<(), String> {
    match value {
        Some(value) if !values.contains(&value) => Err(format!(
            "The {member} {value} is not supported; it must be one of {}.",
            values.join(" | ")
        )),
        _ => Ok(()),
    }
}

/// Refuse a retention period that names no period, or names two.
///
/// Both retention-period shapes carry the same pair of members with the same
/// rules, so they are read here rather than once per member: a number of days
/// goes with `unlimited` false, `unlimited` true goes with no number of days,
/// and a period that names neither selects no retention at all.
///
/// A count of days below one is no period either, whatever the member it is
/// given in: the modelled `NumberOfDays` is a plain integer with no range
/// (`smithy_query(query="shape", shape="NumberOfDays")`), so nothing in the
/// model or the member documentation states a minimum, and the only bound read
/// here is that a span of days is a positive number of them. This emulator's
/// own floor, not a published AWS one — a larger minimum is left to the
/// modelled constraint walk, which is where a documented one would land.
fn check_retention_period(
    member: &str,
    number_of_days: Option<i32>,
    unlimited: Option<bool>,
) -> Result<(), String> {
    match (number_of_days, unlimited.unwrap_or(false)) {
        (Some(_), true) => Err(format!(
            "A {member} must not specify numberOfDays when unlimited is true."
        )),
        (None, false) => Err(format!(
            "A {member} must specify either numberOfDays or unlimited true."
        )),
        (Some(days), false) if days < 1 => Err(format!(
            "The {member} numberOfDays {days} is not a number of days; it must be at least 1."
        )),
        _ => Ok(()),
    }
}

/// Refuse a cold-tier destination whose members do not carry what they are
/// modelled as: `s3ResourceArn` and `roleArn` are both a modelled `ARN`, so a
/// value outside that type is one this service could not report back.
fn check_customer_managed_s3(storage: &CustomerManagedS3Storage) -> Result<(), String> {
    for (member, value) in [
        ("s3ResourceArn", &storage.s3_resource_arn),
        ("roleArn", &storage.role_arn),
    ] {
        if !is_modelled_arn(value) || value.chars().count() > ARN_MAX_LENGTH {
            return Err(format!("The {member} {value} is not an ARN this service can store."));
        }
    }
    Ok(())
}

/// The storage configuration a put stored: the selection it named, when it
/// named it, and when the configuration it accepted falls due to be in force.
#[derive(Debug, Clone)]
pub struct StorageConfigurationRecord {
    /// What the put selected. Replaced whole by the next put.
    pub selection: StorageConfigurationSelection,
    /// The instant of the put, which `DescribeStorageConfiguration` reports as
    /// `lastUpdateDate` ("The date the storage configuration was last updated").
    pub updated_at: SystemTime,
    /// When [`CONFIGURATION_STATE_UPDATE_IN_PROGRESS`] ->
    /// [`CONFIGURATION_STATE_ACTIVE`] falls due: the instant of the put plus
    /// [`ACTIVATION_DELAY`]. Stored rather than slept for, so nothing blocks and
    /// a caller reading straight after the put still observes the state the
    /// service reports while it applies the change.
    pub active_at: SystemTime,
}

impl StorageConfigurationRecord {
    /// The `ConfigurationState` this configuration reports at `now`.
    pub fn state(&self, now: SystemTime) -> &'static str {
        configuration_state(self.active_at, now)
    }
}

/// What the two storage-configuration operations report: the same members, so
/// they read them from one place.
#[derive(Debug, Clone)]
pub struct StorageConfiguration {
    /// The configuration in force, with the documented default filled in for
    /// each member it does not name — see [`documented_defaults`].
    pub selection: StorageConfigurationSelection,
    /// The modelled `configurationStatus`.
    pub status: ConfigurationStatus,
    /// The modelled `lastUpdateDate`, absent while no put has configured the
    /// account: nothing has updated the configuration then.
    pub last_update_date: Option<SystemTime>,
}

/// `selection` with the documented default filled in for each member it leaves
/// unnamed: hot-tier data is "kept indefinitely" when no `retentionPeriod` names
/// a span, and NULL/NaN ingestion is allowed when no `disallowIngestNullNaN`
/// disallows it. The members whose documentation states no default
/// (`multiLayerStorage`, `disassociatedDataStorage`, `warmTier`, and the warm
/// tier's own retention period) are reported only when the configuration names
/// them.
fn documented_defaults(selection: &StorageConfigurationSelection) -> StorageConfigurationSelection {
    StorageConfigurationSelection {
        retention_period: selection.retention_period.clone().or(Some(RetentionPeriod {
            number_of_days: None,
            unlimited: Some(true),
        })),
        disallow_ingest_null_na_n: selection
            .disallow_ingest_null_na_n
            .or(Some(DEFAULT_DISALLOW_INGEST_NULL_NAN)),
        ..selection.clone()
    }
}

impl ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// The storage configuration the account reports at `now`: the one the last
    /// put stored, or the hot-tier default while nothing has stored any.
    ///
    /// A pure READ: an unconfigured account is not materialised, and a
    /// configuration still settling is reported from the instant its put already
    /// recorded, so reading cannot change what a later caller sees.
    pub fn storage_configuration(&self, now: SystemTime) -> StorageConfiguration {
        let status = |state: &str| ConfigurationStatus {
            state: state.to_string(),
            // No error: nothing in this emulator fails to apply a
            // configuration, so the modelled `error` details have nothing to
            // report.
            error: None,
        };
        match &self.storage {
            Some(record) => StorageConfiguration {
                selection: documented_defaults(&record.selection),
                status: status(record.state(now)),
                last_update_date: Some(record.updated_at),
            },
            None => StorageConfiguration {
                selection: documented_defaults(&StorageConfigurationSelection {
                    storage_type: STORAGE_TYPE_SITEWISE_DEFAULT.to_string(),
                    ..Default::default()
                }),
                status: status(CONFIGURATION_STATE_ACTIVE),
                last_update_date: None,
            },
        }
    }

    /// Make `selection` the account's storage configuration, and report what
    /// that call answers with; `Err` with the refusal's wording for a selection
    /// the configuration in force does not admit.
    ///
    /// The whole setting is replaced: a put is the last word on the account's
    /// storage, so one naming the hot tier alone drops the cold-tier destination
    /// an earlier put selected. The one member that does not go back is
    /// `disassociatedDataStorage` — "After the disassociatedDataStorage is
    /// enabled, you can't disable it" — so a selection that would leave it
    /// anything but enabled is refused here, under the same lock the write
    /// takes, rather than in the handler where two puts could race past it. The
    /// configuration the put accepts falls due at [`ACTIVATION_DELAY`], the span
    /// this service settles every transition the clock owes at.
    ///
    /// Read `selection` against [`check_storage_selection`] first: this stores
    /// what it is given.
    pub fn set_storage_configuration(
        &mut self,
        selection: StorageConfigurationSelection,
        now: SystemTime,
    ) -> Result<StorageConfiguration, String> {
        let in_force = self.storage_configuration(now);
        let enabled = |state: &StorageConfigurationSelection| {
            state.disassociated_data_storage.as_deref() == Some(STORAGE_STATE_ENABLED)
        };
        if enabled(&in_force.selection) && !enabled(&selection) {
            return Err(format!(
                "The disassociatedDataStorage is {STORAGE_STATE_ENABLED} and cannot be disabled; \
                 a put must keep naming {STORAGE_STATE_ENABLED}."
            ));
        }
        self.storage = Some(StorageConfigurationRecord {
            selection,
            updated_at: now,
            active_at: now + ACTIVATION_DELAY,
        });
        Ok(self.storage_configuration(now))
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIotsitewiseAwsiotsitewiseState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIotsitewiseAwsiotsitewiseState>)
}

// ---------------------------------------------------------------------------
// The application slice: the record `CreateApplication` writes and
// `DescribeApplication` reports, its bound, the resource kind a refusal names
// and the values an application derives (its ARN, its DNS subdomain and the
// Identity Center application it reports).
//
// One section, like the history-selection and aggregate sections above: what an
// application IS and every value derived from it sit beside each other, so the
// create and the read cannot disagree about either.
// ---------------------------------------------------------------------------

/// The ARN resource-type segment of an application ARN, as [`application_arn`]
/// builds it.
pub const APPLICATION_RESOURCE_TYPE: &str = "application";

/// See [`MAX_ASSET_MODELS`]: the same bound, for the application map.
/// `CreateApplication` is the only thing that adds an application and it declares
/// the modelled `LimitExceededException` (HTTP 410), so that is how this store
/// refuses past the cap. The emulator's own bound, not a published AWS default.
pub const MAX_APPLICATIONS: usize = 1_000;

/// The resource kind a not-found refusal names for an application of a
/// workspace; see [`not_found_message`].
pub const RESOURCE_APPLICATION: &str = "Application";

/// The resource segment an IAM Identity Center application ARN carries, which is
/// what an instance ARN's own `instance/` segment becomes in the ARN of an
/// application created in that instance; see [`idc_application_arn`].
const IDC_INSTANCE_SEGMENT: &str = "instance/";
/// See [`IDC_INSTANCE_SEGMENT`].
const IDC_APPLICATION_SEGMENT: &str = "application/";

/// A stored application of a workspace: everything `DescribeApplication`
/// reports, plus the `tags` the tagging operations read.
///
/// An application is minted with an id of its own and `ListApplications` spans
/// every workspace, so the id identifies it on its own; the workspace it was
/// created in is held here because the modelled reads address an application by
/// `(workspaceName, id)`.
#[derive(Debug, Clone)]
pub struct ApplicationRecord {
    pub id: String,
    pub arn: String,
    /// The workspace the create named. A read naming another workspace names no
    /// application this store holds, however the id resolves.
    pub workspace_name: String,
    pub name: String,
    pub description: Option<String>,
    /// The Identity Center instance ARN the create named — what
    /// [`Self::idc_application_arn`] was derived from.
    pub idc_instance_arn: String,
    /// The Identity Center application this application reports, derived once
    /// here so every later read reports the same ARN; see
    /// [`idc_application_arn`].
    pub idc_application_arn: String,
    pub dns_subdomain: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The instant the transition the clock owes — `CREATING` to `ACTIVE` —
    /// falls due.
    pub active_at: SystemTime,
    pub tags: HashMap<String, String>,
}

impl ApplicationRecord {
    /// The application's `ApplicationStatus` as of `now`: [`STATE_CREATING`]
    /// until the transition the create owes falls due, [`STATE_ACTIVE`] from
    /// then on. Derived, so a read never has to write.
    ///
    /// `ApplicationStatus` spells only `CREATING`, `ACTIVE` and `DELETING`
    /// (smithy_query query="shape" shape="ApplicationStatus"), so an application
    /// owes exactly this one transition and the settling state is the same for
    /// every record rather than stored per record.
    pub fn status(&self, now: SystemTime) -> String {
        state_at(STATE_CREATING, self.active_at, now).to_string()
    }
}

/// The canonical ARN of an application in `region`.
///
/// The resource section carries the application's own id and not its workspace:
/// unlike a pipeline or a task, an application is minted with an id that
/// identifies it across every workspace, which is also why `ListApplications`
/// takes no workspace.
pub fn application_arn(region: &str, application_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{APPLICATION_RESOURCE_TYPE}/{application_id}"),
    )
}

/// The `dnsSubdomain` an application reports.
///
/// DERIVED from the application's own id rather than minted, for the reason
/// [`portal_start_url`] states: it is then stable across reads and unique per
/// application without anything being written on a read. The modelled
/// `DnsSubdomain` is a DNS label (len 1..63, `^[a-z0-9]([a-z0-9-]*[a-z0-9])?$`)
/// and every id [`new_id`] mints is one already — 36 characters of lowercase hex
/// and hyphens, beginning and ending with a hex digit. Derived through this one
/// function so the form has a single place to change if a subdomain ever has to
/// differ from the id.
pub fn application_dns_subdomain(application_id: &str) -> String {
    application_id.to_string()
}

/// The `idcApplicationArn` an application reports: the Identity Center
/// application that was created in the instance `idc_instance_arn` names.
///
/// DERIVED from the two things that identify it — the instance the create named
/// and the application's own id — rather than minted, so it is stable across
/// reads and unique per application. Identity Center nests an application under
/// its instance, so the instance ARN's own resource segment becomes the
/// application one and the application's id is appended; an instance ARN
/// carrying no such segment gets the application segment and the id appended
/// instead, so every application still reports an ARN that names it uniquely.
///
/// Split at the FIRST `instance/`, which in an instance ARN is the one its
/// resource section opens with: an ARN's resource section is its last field, so
/// nothing ahead of that segment can be a second one.
pub fn idc_application_arn(idc_instance_arn: &str, application_id: &str) -> String {
    match idc_instance_arn.split_once(IDC_INSTANCE_SEGMENT) {
        Some((prefix, instance_id)) => {
            format!("{prefix}{IDC_APPLICATION_SEGMENT}{instance_id}/{application_id}")
        }
        None => format!("{idc_instance_arn}/{IDC_APPLICATION_SEGMENT}{application_id}"),
    }
}

// An application's tags live on its own record, reached by ARN like every other
// taggable resource's; the shared `TaggedRecord` seam is what
// `ComAmazonawsIotsitewiseAwsiotsitewiseState::tags_for_resource` resolves it
// through.
tagged_record!(ApplicationRecord);

// ---------------------------------------------------------------------------
// The dashboard slice: the record `CreateDashboard` writes, `UpdateDashboard`
// rewrites and `DescribeDashboard` reports, its bound, the resource kind a
// refusal names and its ARN.
//
// One section, like the application slice above: what a dashboard IS and every
// value derived from it sit beside each other, so the create, the update and the
// read cannot disagree about either.
// ---------------------------------------------------------------------------

/// The ARN resource-type segment of a dashboard ARN, as [`dashboard_arn`] builds
/// it.
pub const DASHBOARD_RESOURCE_TYPE: &str = "dashboard";

/// See [`MAX_ASSET_MODELS`]: the same bound, for the dashboard map.
/// `CreateDashboard` is the only thing that adds a dashboard and it declares the
/// modelled `LimitExceededException` (HTTP 410), so that is how this store
/// refuses past the cap. The emulator's own bound, not a published AWS default.
pub const MAX_DASHBOARDS: usize = 1_000;

/// The resource kind a not-found refusal names for an IoT SiteWise Monitor
/// dashboard; see [`not_found_message`].
pub const RESOURCE_DASHBOARD: &str = "Dashboard";

/// A stored IoT SiteWise Monitor dashboard: everything `DescribeDashboard`
/// reports, plus the `tags` the tagging operations read.
///
/// A dashboard has no lifecycle state of its own — no operation on one answers a
/// status member — so, unlike a portal or an application, nothing here is owed a
/// later transition; the two dates are what a read derives from.
#[derive(Debug, Clone)]
pub struct DashboardRecord {
    pub id: String,
    pub arn: String,
    /// The project the create named, which every read of the dashboard reports
    /// and which the listing form filters on. Not the update's to change: no
    /// member of `UpdateDashboard` carries a project.
    pub project_id: String,
    pub name: String,
    pub description: Option<String>,
    /// The dashboard definition JSON literal, held exactly as the last write
    /// stated it: the service stores the literal and reports it back unparsed,
    /// so each write path checks the member's documented form — see
    /// [`check_json_document`] — before a value reaches here.
    pub definition: String,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    /// The `clientToken` the create that minted this dashboard carried, if any:
    /// what a retry of that create is recognised by, so it answers from this
    /// record instead of minting a second dashboard. Not the update's to
    /// change.
    pub client_token: Option<String>,
    pub tags: HashMap<String, String>,
}

impl DashboardRecord {
    /// The dashboard a create carrying `client_token` already minted, if this
    /// store holds one — see [`ComAmazonawsIotsitewiseAwsiotsitewiseState::dashboard_for_client_token`].
    fn matches_client_token(&self, client_token: &str) -> bool {
        self.client_token.as_deref() == Some(client_token)
    }

    /// Replace the caller-owned members with what an update stated, and record
    /// that instant as the dashboard's last update.
    ///
    /// The request describes the dashboard's NEW state rather than patching
    /// selected members, so an omitted optional member clears the stored one —
    /// the same rule `UpdateAsset` applies to an asset's description. The
    /// dashboard's identity and provenance — its id, its ARN, its project, its
    /// tags and its creation date — are not that call's to change, and no member
    /// of its input carries them.
    pub fn apply_update(
        &mut self,
        name: String,
        description: Option<String>,
        definition: String,
        now: SystemTime,
    ) {
        self.name = name;
        self.description = description;
        self.definition = definition;
        self.last_update_date = now;
    }
}

/// The canonical ARN of a dashboard in `region`, in the form `CreateDashboard`
/// documents:
/// `arn:${Partition}:iotsitewise:${Region}:${Account}:dashboard/${DashboardId}`.
pub fn dashboard_arn(region: &str, dashboard_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{DASHBOARD_RESOURCE_TYPE}/{dashboard_id}"),
    )
}

// A dashboard's tags live on its own record, reached by ARN like every other
// taggable resource's; the shared `TaggedRecord` seam is what
// `ComAmazonawsIotsitewiseAwsiotsitewiseState::tags_for_resource` resolves it
// through.
tagged_record!(DashboardRecord);

// ---------------------------------------------------------------------------
// The bulk import job slice: the record `CreateBulkImportJob` writes and
// `DescribeBulkImportJob` reports, the lifecycle a created job is owed, the
// bounds the request is read against and the resource kind a refusal names.
//
// One section, like the slices above: what a job IS and every value derived
// from it sit beside each other, so the create and the read cannot disagree
// about either.
// ---------------------------------------------------------------------------

/// The `JobStatus` a job reports while the import it asked for has not started:
/// "PENDING – IoT SiteWise is waiting for the current bulk import job to
/// finish".
///
/// Source: `smithy_query(query="shape", shape="JobStatus")`, whose values are
/// also in `crate::ops::types::JOB_STATUS_VALUES`.
pub const BULK_IMPORT_JOB_PENDING: &str = "PENDING";

/// The `JobStatus` a job reports while it is being worked on: "RUNNING – IoT
/// SiteWise is processing your request to import your data from Amazon S3".
pub const BULK_IMPORT_JOB_RUNNING: &str = "RUNNING";

/// The terminal `JobStatus` a job reaches: "COMPLETED – IoT SiteWise
/// successfully completed your request to import data from Amazon S3".
///
/// WHY the terminal record is this value and not one of the failure ones: the
/// three remaining values report outcomes something would have had to DIAGNOSE.
/// `CANCELLED` needs a call that cancels a job, which this service models none
/// of. `FAILED` and `COMPLETED_WITH_FAILURES` state that the import was
/// attempted and that the reasons are "saved in the specified error report
/// location in Amazon S3" — a report nothing here writes — so reporting either
/// would invent a diagnosis, and would make every job a caller starts look
/// broken when nothing about the request was. The status this emulator records
/// is the lifecycle the create OWED (see [`BulkImportJobRecord`]), and the
/// lifecycle every other owed transition in this service settles into is its
/// success state, so this is the consistent terminal record for one.
pub const BULK_IMPORT_JOB_COMPLETED: &str = "COMPLETED";

/// How long each step of a bulk import job's lifecycle takes: a created job
/// stays [`BULK_IMPORT_JOB_PENDING`] for this long, then reports
/// [`BULK_IMPORT_JOB_RUNNING`] for this long, then is
/// [`BULK_IMPORT_JOB_COMPLETED`].
///
/// CHOSEN, not documented: AWS publishes no duration for either step — an import
/// takes as long as the data it moves. The span is deliberately NON-ZERO, so a
/// caller that describes a job straight after creating it still observes the
/// transient status the real service reports, and short enough that a caller can
/// poll the job through to its terminal status.
pub const BULK_IMPORT_JOB_STEP: Duration = Duration::from_millis(500);

/// How many files one bulk import job may name.
///
/// DOCUMENTED, not chosen: the `files` member states it — "You can specify up to
/// 100 files for each bulk import job" — and the list shape carries no length
/// constraint of its own, so the create is what enforces it.
pub const MAX_FILES_PER_BULK_IMPORT_JOB: usize = 100;

/// How many bulk import jobs the store holds.
///
/// The same kind of bound, and for the same reason, as [`MAX_DASHBOARDS`]: a
/// create is the only thing that adds a job and nothing evicts one, so without a
/// cap a stream of creates would grow one emulator process without limit. The
/// emulator's own bound, not a published AWS quota; `CreateBulkImportJob`
/// refuses past it with the modelled `LimitExceededException` (HTTP 410).
pub const MAX_BULK_IMPORT_JOBS: usize = 1_000;

/// The resource kind a not-found refusal names for a bulk import job; see
/// [`not_found_message`].
pub const RESOURCE_BULK_IMPORT_JOB: &str = "Bulk Import Job";

/// One bulk import job, as `CreateBulkImportJob` starts it and
/// `DescribeBulkImportJob` reports it.
///
/// Its STATUS is not stored: it is DERIVED from the instant the job was created
/// ([`Self::status`]), because the lifecycle is owed by the clock rather than by
/// any call. So a reader reports the status the clock has made due without
/// writing to the store, and two readers of one job can never disagree about
/// where it has got to.
///
/// That status is a RECORD of the lifecycle the create owed, not a report on
/// work something performed: the files a job names live in Amazon S3, which this
/// emulator does not reach, so nothing is read from them and no sample appears
/// because a job ran. What an accepted import DOES record is the destination
/// session dataset's data segments, taken from the data this store really holds
/// for the stream each file names ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::ingest_data_segments`]);
/// an operation that comes to ingest a job's files must likewise take them from
/// somewhere this emulator really holds, never synthesise values to match the
/// status reached here.
#[derive(Debug, Clone)]
pub struct BulkImportJobRecord {
    /// The `jobId` this job was minted with ([`new_id`]).
    pub job_id: String,
    /// "The unique name that helps identify the job request", as the create
    /// stated it.
    pub job_name: String,
    /// "The ARN of the IAM role that allows IoT SiteWise to read Amazon S3
    /// data". This service neither assumes nor resolves it.
    pub job_role_arn: String,
    /// "The files in the specified Amazon S3 bucket that contain your data",
    /// kept verbatim and in the order the request listed them, so the reader
    /// answers the request's own files. Bounded by
    /// [`MAX_FILES_PER_BULK_IMPORT_JOB`].
    pub files: Vec<File>,
    /// "The Amazon S3 destination where errors associated with the job creation
    /// request are saved". This service writes nothing to it.
    pub error_report_location: ErrorReportLocation,
    /// "Contains the configuration information of a job, such as the file format
    /// used to save data in Amazon S3" — reported only when the create named it.
    pub job_configuration: Option<JobConfiguration>,
    /// The modelled `adaptiveIngestion`, as the create stated it.
    pub adaptive_ingestion: Option<bool>,
    /// The modelled `deleteFilesAfterImport`, as the create stated it. This
    /// service deletes nothing: the files are in S3, which it does not reach.
    pub delete_files_after_import: Option<bool>,
    /// "The ID of the session dataset to ingest data into", for a job that
    /// ingests into one; `None` for a job ingesting into time series outside a
    /// workspace.
    pub dataset_id: Option<String>,
    /// The workspace the session dataset lives in — named together with
    /// [`Self::dataset_id`], and `None` for the same job that has no dataset.
    pub workspace_name: Option<String>,
    /// When the job was created, which is the `jobCreationDate` a read reports
    /// and the instant its whole lifecycle is owed from.
    pub created_at: SystemTime,
}

impl BulkImportJobRecord {
    /// The job's status at `now`, and the instant that status fell due.
    ///
    /// ONE place, so [`Self::status`] and [`Self::last_update_date`] cannot
    /// disagree about where the job has got to: the second is simply the instant
    /// the first became true.
    fn settled(&self, now: SystemTime) -> (&'static str, SystemTime) {
        let running_at = self.created_at + BULK_IMPORT_JOB_STEP;
        let completed_at = running_at + BULK_IMPORT_JOB_STEP;
        if now >= completed_at {
            (BULK_IMPORT_JOB_COMPLETED, completed_at)
        } else if now >= running_at {
            (BULK_IMPORT_JOB_RUNNING, running_at)
        } else {
            (BULK_IMPORT_JOB_PENDING, self.created_at)
        }
    }

    /// The job's `jobStatus` at `now`: [`BULK_IMPORT_JOB_PENDING`] until the
    /// first [`BULK_IMPORT_JOB_STEP`] has elapsed, [`BULK_IMPORT_JOB_RUNNING`]
    /// until the second has, [`BULK_IMPORT_JOB_COMPLETED`] after that.
    /// `COMPLETED` is terminal, so the status never moves again.
    pub fn status(&self, now: SystemTime) -> &'static str {
        self.settled(now).0
    }

    /// "The date the job was last updated": the instant of the most recent
    /// status transition the clock has made due, which for a job nothing has
    /// moved yet is the instant it was created.
    pub fn last_update_date(&self, now: SystemTime) -> SystemTime {
        self.settled(now).1
    }

    /// Whether this job answers a read scoped to `workspace`.
    ///
    /// A read may name a `workspaceName`, which NARROWS it to the jobs of that
    /// workspace: a job created for a session dataset in another workspace, and
    /// one created for no workspace at all, are not reachable through it. A read
    /// that names none is account-wide, so every job answers it.
    pub fn in_workspace(&self, workspace: Option<&str>) -> bool {
        match workspace {
            None => true,
            Some(workspace) => self.workspace_name.as_deref() == Some(workspace),
        }
    }
}

// ---------------------------------------------------------------------------
// The enrichment jobs of a workspace: what `CreateEnrichmentJob` starts and
// `ListEnrichmentJobs` enumerates. `CreateEnrichmentJob` "returns immediately
// with job details while processing continues in the background", so the job's
// STATUS — and the instant of its last status change — are DERIVED from the
// instant it was created, exactly as a dataset export job's and a query's are:
// a read reports what the clock has made due without writing anything, and two
// readers of one job cannot disagree about where it has got to.
//
// A cancellation IS something a call does to a job, so it is RECORDED on the
// record ([`EnrichmentJobRecord::cancelled_at`]) rather than derived, and from
// then on it is the terminal status every reader reports for that job: the
// cancel takes effect at the instant it is recorded, because
// `CancelEnrichmentJob` "responds immediately after recording the
// cancellation" with the status the job then stands in.
//
// What a record does NOT hold is an embedding of any video: nothing in this
// emulator analyses a time series, and no operation of this API answers an
// embedding. The job's identity, the configuration it was created from and
// where the lifecycle that create owed has got to are the whole of what a
// client of this API can observe. An operation that comes to answer the
// semantic search a completed job enables must derive its answer from the
// store, never synthesise one to match the status reached here.
// ---------------------------------------------------------------------------

/// The `EnrichmentJobStatus` a job reports from the instant it is created:
/// "Initial status of the enrichment job, typically PENDING"
/// (`crate::ops::types::ENRICHMENT_JOB_STATUS_VALUES`).
pub const ENRICHMENT_JOB_PENDING: &str = "PENDING";

/// The value a job reports once the clock has started it processing: "The job
/// will transition to RUNNING when processing begins".
pub const ENRICHMENT_JOB_RUNNING: &str = "RUNNING";

/// The value a job reports once the clock has finished processing it. Terminal.
pub const ENRICHMENT_JOB_COMPLETED: &str = "COMPLETED";

/// The value a job a cancel was recorded on reports from that instant on:
/// "Jobs in PENDING or RUNNING status transition to CANCELLED". Terminal.
pub const ENRICHMENT_JOB_CANCELLED: &str = "CANCELLED";

/// The `JobType` an `eventDetection` configuration makes a job: the type is
/// "derived from the job configuration union member", so it is derived here
/// rather than stored beside the configuration.
pub const JOB_TYPE_EVENT_DETECTION: &str = "EVENT_DETECTION";

/// How long each step of an enrichment job's lifecycle takes: a created job
/// stays `PENDING` for this long, then reports `RUNNING` for this long, then is
/// `COMPLETED`.
///
/// CHOSEN, not documented: AWS publishes no duration for either step — a job
/// takes as long as the video it analyses. The span is deliberately NON-ZERO,
/// so a caller that lists jobs straight after creating one still observes the
/// transient status the real service reports, and short enough that a caller
/// can poll a job through to its terminal status.
pub const ENRICHMENT_JOB_STEP: Duration = Duration::from_millis(250);

/// How many enrichment jobs one workspace holds.
///
/// CHOSEN: a create is the only thing that adds a job and nothing evicts one,
/// so without a bound a stream of creates would grow one emulator process
/// without limit. `CreateEnrichmentJob` refuses past it with the modelled
/// `LimitExceededException` (HTTP 410), which its own documentation names for
/// "Too many concurrent jobs or requests".
pub const MAX_ENRICHMENT_JOBS_PER_WORKSPACE: usize = 100;

/// How many enrichment jobs the store holds in total, across every workspace.
/// The per-workspace cap cannot bound the store on its own, for the reason
/// [`MAX_DATASET_EXPORT_JOBS`] states.
pub const MAX_ENRICHMENT_JOBS: usize = 1_000;

/// One enrichment job, as `CreateEnrichmentJob` started it.
#[derive(Debug, Clone)]
pub struct EnrichmentJobRecord {
    /// The `jobId` this job was minted with ([`new_id`]).
    pub job_id: String,
    /// The workspace the job was created in. Every operation on a job
    /// addresses it within a workspace, so the pair is the job's identity —
    /// see [`enrichment_job_key`].
    pub workspace_name: String,
    /// The `jobConfiguration` the create carried, kept verbatim: it is what the
    /// job IS, and every other value a reader reports about the job — its type,
    /// the dataset it analyses and the time series it names — is DERIVED from
    /// it rather than copied beside it, so the two cannot drift.
    pub configuration: EnrichmentJobConfiguration,
    /// The `clientToken` the create carried, if any — a record of how that call
    /// was identified. What makes a RETRY of it replay this job is the store's
    /// registry of the tokens an operation has already served
    /// ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::idempotent_verdict`]),
    /// which recognises the CALL and not the job it left behind.
    pub client_token: Option<String>,
    /// When the job was created, which is the `createdAt` a read reports and
    /// the instant its whole lifecycle is owed from.
    pub created_at: SystemTime,
    /// When a cancel was recorded on this job, if one was: the `cancelledAt` a
    /// read reports, and from that instant the terminal status every reader
    /// reports for the job. Held rather than derived, because a cancellation is
    /// something a CALL did to the job and not something the clock owed it.
    pub cancelled_at: Option<SystemTime>,
}

impl EnrichmentJobRecord {
    /// The dataset this job analyses: `EventDetection`'s "IoT SiteWise dataset
    /// ID containing the video time-series data to analyze".
    pub fn dataset_id(&self) -> &str {
        enrichment_job_target(&self.configuration).0
    }

    /// The `propertyAlias` the configuration named, if it identified the time
    /// series that way: "Present when the job was created using a
    /// propertyAlias".
    pub fn property_alias(&self) -> Option<&str> {
        enrichment_job_target(&self.configuration).1
    }

    /// The `timeSeriesId` the configuration named, if it identified the time
    /// series that way: "Present when the job was created using a
    /// timeSeriesId".
    pub fn time_series_id(&self) -> Option<&str> {
        enrichment_job_target(&self.configuration).2
    }

    /// The job's `jobType`, derived from the configuration's union member.
    pub fn job_type(&self) -> &'static str {
        enrichment_job_target(&self.configuration).3
    }

    /// The job's `status` at `now`: `CANCELLED` once a cancel was recorded on
    /// it, otherwise `PENDING` until the first [`ENRICHMENT_JOB_STEP`] has
    /// elapsed, `RUNNING` until the second has, `COMPLETED` after that. A
    /// terminal status never moves again.
    ///
    /// No job of this emulator fails or times out, so `FAILED` and `TIMED_OUT`
    /// — and the `failureMessage` the model documents beside them — are
    /// statuses nothing here reports.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if self.cancelled_at.is_some() {
            return ENRICHMENT_JOB_CANCELLED;
        }
        match self.completed_at(now) {
            Some(_) => ENRICHMENT_JOB_COMPLETED,
            None if now >= self.created_at + ENRICHMENT_JOB_STEP => ENRICHMENT_JOB_RUNNING,
            None => ENRICHMENT_JOB_PENDING,
        }
    }

    /// The instant the job COMPLETED, and `None` until it has — or at all for a
    /// job a cancel was recorded on, which stopped where that cancel found it
    /// and so never reaches the status this instant records (`completedAt` is
    /// "only present if status is COMPLETED").
    pub fn completed_at(&self, now: SystemTime) -> Option<SystemTime> {
        if self.cancelled_at.is_some() {
            return None;
        }
        let completes_at = self.created_at + 2 * ENRICHMENT_JOB_STEP;
        (now >= completes_at).then_some(completes_at)
    }

    /// "Timestamp of the last job status change": the instant a recorded
    /// cancellation took the job to its terminal status, else the instant of the
    /// latest transition the clock has made due, and `None` for a job still in
    /// the status its create answered with, which nothing has moved yet.
    pub fn updated_at(&self, now: SystemTime) -> Option<SystemTime> {
        if let Some(cancelled_at) = self.cancelled_at {
            return Some(cancelled_at);
        }
        let runs_at = self.created_at + ENRICHMENT_JOB_STEP;
        self.completed_at(now)
            .or_else(|| (now >= runs_at).then_some(runs_at))
    }

    /// Whether the status this job reports at `now` is one it can still move
    /// out of — what the duplicate-job rule reads, and what decides whether a
    /// cancel may still be recorded on the job.
    pub fn is_active(&self, now: SystemTime) -> bool {
        self.cancelled_at.is_none() && self.completed_at(now).is_none()
    }

    /// Record a cancellation on this job at `now`. The cancel takes effect from
    /// that instant: "the API responds immediately after recording the
    /// cancellation", with `CANCELLED` as the status it answers.
    pub fn record_cancel(&mut self, now: SystemTime) {
        self.cancelled_at = Some(now);
    }

    /// This job as the summary a listing reports for it, at `now`.
    ///
    /// Derived here rather than in the handler so a listing and any later read
    /// of one stored job cannot report different statuses or instants for it.
    pub fn summary(&self, now: SystemTime) -> EnrichmentJobSummary {
        EnrichmentJobSummary {
            job_id: self.job_id.clone(),
            status: self.status(now).to_string(),
            workspace_name: self.workspace_name.clone(),
            job_type: self.job_type().to_string(),
            dataset_id: self.dataset_id().to_string(),
            property_alias: self.property_alias().map(str::to_string),
            time_series_id: self.time_series_id().map(str::to_string),
            created_at: to_date_time(self.created_at),
            updated_at: self.updated_at(now).map(to_date_time),
        }
    }
}

/// The key [`ComAmazonawsIotsitewiseAwsiotsitewiseState::enrichment_jobs`]
/// holds a job under: the workspace it was created in and its own id, because
/// every operation on a job addresses it within a workspace, so a job of one
/// workspace is not reachable through another.
pub fn enrichment_job_key(workspace_name: &str, job_id: &str) -> (String, String) {
    (workspace_name.to_string(), job_id.to_string())
}

/// What identifies the WORK one enrichment job does, as the duplicate-job rule
/// compares two jobs: the dataset, the time series by whichever identifier the
/// configuration named, and the job type.
///
/// Also where every per-job value a reader derives comes from, so the record's
/// accessors and that rule read ONE definition of the configuration's shape.
pub fn enrichment_job_target(
    configuration: &EnrichmentJobConfiguration,
) -> (&str, Option<&str>, Option<&str>, &'static str) {
    match configuration {
        EnrichmentJobConfiguration::eventDetection(detection) => (
            detection.dataset_id.as_str(),
            detection.property_alias.as_deref(),
            detection.time_series_id.as_deref(),
            JOB_TYPE_EVENT_DETECTION,
        ),
    }
}

/// Refuse an `EnrichmentJobConfiguration` whose members break a rule its own
/// shapes state, in the wording the refusal carries; `Ok` for one that keeps
/// them.
///
/// The rules come from the shapes themselves: `EventDetection`'s time series is
/// identified by one of two members ("Specify either timeSeriesId or
/// propertyAlias, but not both"), and an `EnrichmentTrimSettings` end time
/// "must be greater than startTime".
pub fn check_enrichment_job_configuration(
    configuration: &EnrichmentJobConfiguration,
) -> Result<(), String> {
    match configuration {
        EnrichmentJobConfiguration::eventDetection(detection) => {
            if detection.time_series_id.is_some() && detection.property_alias.is_some() {
                return Err("jobConfiguration.eventDetection: timeSeriesId and propertyAlias \
                            are mutually exclusive."
                    .to_string());
            }
            if timestamp_key(&detection.trim_settings.end_time)
                <= timestamp_key(&detection.trim_settings.start_time)
            {
                return Err("jobConfiguration.eventDetection.trimSettings.endTime must be \
                            greater than trimSettings.startTime."
                    .to_string());
            }
            Ok(())
        }
    }
}

/// The resource kind a `ResourceNotFoundException` names for an enrichment job,
/// under the one form [`not_found_message`] states every absence in.
///
/// Declared here, in this section, rather than beside the other resource kinds:
/// an enrichment job is addressed by `DescribeEnrichmentJob` and
/// `CancelEnrichmentJob` alone, so the label belongs with the record those
/// operations read.
pub const RESOURCE_ENRICHMENT_JOB: &str = "Enrichment Job";

/// The ARN segment an enrichment job is named by in the places this API reports
/// a job's ARN: the required `resourceArn` of the conflict a duplicate create,
/// or a cancel of a job that already reached a terminal status, is refused with.
///
/// No modelled member of an enrichment job carries an ARN, so the form is this
/// service's own — the `<type>/<id>` resource form every IoT SiteWise ARN
/// takes, as [`parse_resource_arn`] reads them back.
pub const ENRICHMENT_JOB_RESOURCE_TYPE: &str = "enrichment-job";

/// The ARN of an enrichment job in `region`; see
/// [`ENRICHMENT_JOB_RESOURCE_TYPE`].
pub fn enrichment_job_arn(region: &str, job_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{ENRICHMENT_JOB_RESOURCE_TYPE}/{job_id}"),
    )
}

/// How many spent idempotency tokens the store remembers.
///
/// CHOSEN: a mutating call that carries a `clientToken` adds an entry and nothing
/// else removes one, so without a bound a stream of tokens would grow one emulator
/// process without limit. Past the cap the LEAST RECENTLY SPENT token is forgotten
/// first, so the tokens a client could still be retrying are the ones kept.
pub const MAX_IDEMPOTENT_CALLS: usize = 1_000;

/// What one mutating call did, remembered under the `clientToken` it carried, so a
/// retry of that call is answered from it instead of running a second time.
#[derive(Debug, Clone)]
pub struct IdempotentCall {
    /// The request the token was spent on, as [`request_fingerprint`] renders it:
    /// what tells a RETRY of that call from a different call reusing its token.
    pub request: String,
    /// The resource the call acted on — the id a create minted, or the id the
    /// request addressed — which is what its operation replays its answer from.
    pub resource_id: String,
    /// When the token was spent, which is what the bound above evicts by.
    pub spent_at: SystemTime,
    /// The answer a BATCH data-segment call gave, for the operations whose answer
    /// their own writes make unreadable afterwards: the per-entry outcome of a
    /// batch is not a property of the dataset the batch wrote to, so a retry of
    /// one has nothing in the store to rebuild it from. `None` for every
    /// operation whose answer is read back from the resource it acted on.
    ///
    /// BOUNDED by the request that produced it: one answer carries at most one
    /// failure per entry, and an entry list is already capped — by
    /// [`MAX_DATA_SEGMENTS_PER_DATASET`] for the association forms and by the
    /// model's own `@length` for the deletion form — so the memory the
    /// idempotency record holds is bounded by that cap times
    /// [`MAX_IDEMPOTENT_CALLS`].
    pub batch_answer: Option<DataSegmentBatchAnswer>,
}

/// One entry a batch data-segment call could not apply, as the three batch
/// data-segment operations each report it: the segment the entry addressed plus
/// the code and the message saying why.
///
/// One struct for all three, because their modelled failure shapes carry the same
/// members — `sourceDatasetId` only where the operation's entries name a source —
/// and each handler maps it back into its own.
#[derive(Debug, Clone)]
pub struct DataSegmentFailure {
    /// The dataset the segment's data came from, for the operations whose entries
    /// name one.
    pub source_dataset_id: Option<String>,
    pub time_series_id: String,
    pub start_timestamp: TimeInNanos,
    pub end_timestamp: TimeInNanos,
    pub error_code: String,
    pub error_message: String,
}

/// What one batch data-segment call answered: the dataset it addressed, the
/// version that dataset reported after the call, and the entries it could not
/// apply — the whole of what a retry of that call replays.
#[derive(Debug, Clone)]
pub struct DataSegmentBatchAnswer {
    pub dataset_id: String,
    pub dataset_version: String,
    pub failures: Vec<DataSegmentFailure>,
}

/// What the `clientToken` a mutating request carries means for that request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenVerdict {
    /// Serve the request: it carries no token, or none this store has seen.
    Serve,
    /// The SAME request already ran under this token, so this one is a retry of
    /// it: answer from the resource named here and write nothing.
    Replay(String),
    /// A DIFFERENT request — this operation, this token, other members — already
    /// spent this token, which names the resource that earlier call acted on.
    /// Such a request repeats no call, so what it gets depends on what the
    /// operation can say: one whose error list declares the modelled conflict
    /// refuses it, naming that resource, and one that declares none carries it
    /// out as the request it is, because its answer would otherwise assert a
    /// change nothing recorded.
    Spent(String),
}

/// The key [`ComAmazonawsIotsitewiseAwsiotsitewiseState::idempotent_calls`] holds
/// a spent token under: the operation and the token, because a token is scoped to
/// the operation it was sent to — one value sent to two operations is two tokens.
pub fn idempotency_key(operation: &str, client_token: &str) -> String {
    format!("{operation}/{client_token}")
}

/// A canonical rendering of the members a mutating request is recognised by, so
/// that a retry of it renders identically and any other request does not.
///
/// `members` are rendered in the order given — each through [`std::fmt::Debug`],
/// so a list or a nested structure renders in full — and `tags` from its pairs
/// SORTED, because a map has no order of its own and this rendering must not
/// depend on one. A request carrying no tags renders like one carrying an empty
/// set, which is the same thing it asks for.
pub fn request_fingerprint(
    members: &[&dyn ::std::fmt::Debug],
    tags: Option<&HashMap<String, String>>,
) -> String {
    let mut rendered: Vec<String> = members.iter().map(|member| format!("{member:?}")).collect();
    let mut pairs: Vec<String> = tags
        .into_iter()
        .flatten()
        .map(|(key, value)| format!("{key:?}={value:?}"))
        .collect();
    pairs.sort();
    rendered.push(pairs.join(","));
    // A unit separator: no member of this API may carry one (every string shape
    // that reaches here forbids the C0 controls), so no rendering of two members
    // can be read as a rendering of one.
    rendered.join("\u{1f}")
}

/// How a member the request did not carry renders in a [`request_fingerprint`].
/// No rendering of a member it DID carry can collide with it: those are `Debug`
/// forms, which start with a bracket, a quote or a member name.
pub const FINGERPRINT_ABSENT: &str = "<absent>";

/// A canonical rendering of a `portalTypeConfiguration` a request carried, for
/// the [`request_fingerprint`] of that request.
///
/// The configuration is a MAP, which has no order of its own, so its entries go
/// in SORTED by key: a retry of one call has to render identically to it whatever
/// order either request's map happens to iterate in. Shared by the operations
/// that take one, so one configuration renders one way.
pub fn portal_type_configuration_fingerprint(
    configuration: Option<&HashMap<String, PortalTypeEntry>>,
) -> String {
    match configuration {
        None => FINGERPRINT_ABSENT.to_string(),
        Some(configuration) => {
            let mut entries: Vec<(&String, &PortalTypeEntry)> = configuration.iter().collect();
            entries.sort_by_key(|(key, _)| *key);
            format!("{entries:?}")
        }
    }
}

/// A canonical and BOUNDED rendering of an uploaded image FILE, for the
/// [`request_fingerprint`] of the request that carried it.
///
/// The image's own BYTES are not rendered: `ImageFile$data` admits up to 1.5 MB
/// and a fingerprint is kept for as long as the token it was spent under, so an
/// uploaded file goes in as the SHA-256 digest of its contents beside its file
/// type. Two different files still render differently and the same file renders
/// identically, which is all a retry has to be told apart by. Shared by every
/// operation that takes an image, so one upload renders one way.
pub fn image_file_fingerprint(file: Option<&ImageFile>) -> String {
    match file {
        None => FINGERPRINT_ABSENT.to_string(),
        Some(file) => {
            let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(&file.data);
            format!("{}:{}", file.r#type, ::hex::encode(digest))
        }
    }
}

/// A canonical rendering of a string MAP a request carried — the environment
/// variables a pipeline, a compute node or a task declares — for the
/// [`request_fingerprint`] of that request.
///
/// A map has no order of its own, so its pairs go in SORTED by key: a retry of
/// one call has to render identically to it whatever order either request's map
/// happens to iterate in. Shared by every operation that takes one, so one map
/// renders one way.
pub fn string_map_fingerprint(map: Option<&HashMap<String, String>>) -> String {
    match map {
        None => FINGERPRINT_ABSENT.to_string(),
        Some(map) => {
            let mut pairs: Vec<(&String, &String)> = map.iter().collect();
            pairs.sort_by_key(|(key, _)| *key);
            format!("{pairs:?}")
        }
    }
}

/// A canonical rendering of the `computations` list a request carried, for the
/// [`request_fingerprint`] of that request.
///
/// The list keeps the ORDER the request stated it in — a list is ordered, so two
/// orderings are two requests — while each node's own environment variables go
/// in through [`string_map_fingerprint`], because a map has no order of its own.
pub fn compute_nodes_fingerprint(nodes: &[ComputeNode]) -> String {
    nodes
        .iter()
        .map(|node| {
            format!(
                "{:?}/{:?}/{:?}/{}",
                node.compute_node_name,
                node.task_name,
                node.depends_on,
                string_map_fingerprint(node.environment_variables.as_ref()),
            )
        })
        .collect::<Vec<String>>()
        .join(",")
}

/// A canonical rendering of the `executionEnvironmentVariableOverrides` a
/// request carried, for the [`request_fingerprint`] of that request.
///
/// The global overrides go in through [`string_map_fingerprint`] and the
/// per-compute-node ones as their node names SORTED, each node's own map through
/// that same helper: both members are maps, which have no order of their own, so
/// a retry of one call has to render identically to it whatever order either
/// request's maps happen to iterate in.
///
/// A `computeNodes` map the request omitted renders like one it sent empty,
/// because both override no node at all and so ask for the same run.
pub fn execution_environment_variables_fingerprint(
    overrides: Option<&ExecutionEnvironmentVariables>,
) -> String {
    match overrides {
        None => FINGERPRINT_ABSENT.to_string(),
        Some(given) => {
            let mut nodes: Vec<(&String, String)> = given
                .compute_nodes
                .iter()
                .flatten()
                .map(|(node, variables)| (node, string_map_fingerprint(Some(variables))))
                .collect();
            nodes.sort_by_key(|(node, _)| *node);
            format!(
                "{}/{nodes:?}",
                string_map_fingerprint(given.global.as_ref())
            )
        }
    }
}

/// A canonical rendering of the `executionMountOverrides` a request carried, for
/// the [`request_fingerprint`] of that request.
///
/// The compute nodes it names go in SORTED, because it addresses them through a
/// map; each node's own overrides keep the ORDER the request stated them in,
/// because a list is ordered and two orderings are two requests.
pub fn mount_overrides_fingerprint(overrides: Option<&MountOverrides>) -> String {
    match overrides {
        None => FINGERPRINT_ABSENT.to_string(),
        Some(given) => {
            let mut nodes: Vec<(&String, &Vec<Mount>)> = given.compute_nodes.iter().collect();
            nodes.sort_by_key(|(node, _)| *node);
            format!("{nodes:?}")
        }
    }
}

/// A canonical rendering of a `taskConfiguration` a request carried, for the
/// [`request_fingerprint`] of that request.
///
/// Every member but the environment variables renders through
/// [`std::fmt::Debug`]; those go in through [`string_map_fingerprint`], because
/// a map has no order of its own. Shared by every operation that takes one, so
/// one configuration renders one way.
pub fn task_configuration_fingerprint(configuration: &TaskConfiguration) -> String {
    match configuration {
        TaskConfiguration::containerTaskConfiguration(container) => format!(
            "containerTaskConfiguration({:?},{:?},{:?},{:?},{:?},{:?},{:?},{:?},{})",
            container.ecr_uri,
            container.task_execution_role,
            container.processing_type,
            container.processing_unit,
            container.ephemeral_storage_configuration,
            container.command,
            container.timeout_seconds,
            container.mounts,
            string_map_fingerprint(container.environment_variables.as_ref()),
        ),
    }
}

/// How a resource addressed by `(workspaceName, name)` — a pipeline, a task —
/// is NAMED in the idempotency registry, which remembers one string per spent
/// token: the resource's own name is unique only within its workspace, so the
/// workspace goes in beside it.
///
/// Both names target shapes bounded by `^[a-zA-Z0-9_-]+$`, so the `/` between
/// them occurs in neither and one rendering reads back as exactly one pair.
pub fn workspace_scoped_token_resource(workspace_name: &str, resource_name: &str) -> String {
    format!("{workspace_name}/{resource_name}")
}

/// The `(workspaceName, name)` pair a [`workspace_scoped_token_resource`]
/// rendering names — the key the resource is stored under.
///
/// A string carrying no separator names no workspace, so it resolves to no
/// stored resource while still naming what the registry remembers.
pub fn workspace_scoped_token_key(resource: &str) -> (String, String) {
    match resource.split_once('/') {
        Some((workspace_name, resource_name)) => {
            (workspace_name.to_string(), resource_name.to_string())
        }
        None => (String::new(), resource.to_string()),
    }
}

impl ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// What `operation` is to do with the request whose `clientToken` is `token`
    /// and whose identifying members render as `request`.
    ///
    /// A pure READ: it records nothing, so asking does not spend a token.
    pub fn idempotent_verdict(
        &self,
        operation: &str,
        token: Option<&str>,
        request: &str,
    ) -> TokenVerdict {
        let Some(token) = token else {
            return TokenVerdict::Serve;
        };
        match self.idempotent_calls.get(&idempotency_key(operation, token)) {
            Some(call) if call.request == request => TokenVerdict::Replay(call.resource_id.clone()),
            Some(call) => TokenVerdict::Spent(call.resource_id.clone()),
            None => TokenVerdict::Serve,
        }
    }

    /// Remember that `operation` served `request` under `token`, acting on
    /// `resource_id`, so a retry of that call replays this answer.
    ///
    /// Call it only where the call SUCCEEDED: a refused request changed nothing,
    /// so the token it carried was not spent and the caller may send it again.
    /// A request carrying no token records nothing.
    pub fn record_idempotent_call(
        &mut self,
        operation: &str,
        token: Option<&str>,
        request: String,
        resource_id: &str,
        now: SystemTime,
    ) {
        self.remember_idempotent_call(operation, token, request, resource_id, None, now);
    }

    /// [`Self::record_idempotent_call`] for a batch data-segment call, which
    /// remembers the `answer` it gave as well: its per-entry outcome is no
    /// property of the dataset it wrote to, so a retry has nothing else to
    /// rebuild that answer from.
    pub fn record_idempotent_batch_call(
        &mut self,
        operation: &str,
        token: Option<&str>,
        request: String,
        answer: DataSegmentBatchAnswer,
        now: SystemTime,
    ) {
        let resource_id = answer.dataset_id.clone();
        self.remember_idempotent_call(operation, token, request, &resource_id, Some(answer), now);
    }

    /// The answer the batch data-segment call `operation` gave under `token`, if
    /// this store still remembers it.
    pub fn idempotent_batch_answer(
        &self,
        operation: &str,
        token: Option<&str>,
    ) -> Option<&DataSegmentBatchAnswer> {
        self.idempotent_calls
            .get(&idempotency_key(operation, token?))?
            .batch_answer
            .as_ref()
    }

    fn remember_idempotent_call(
        &mut self,
        operation: &str,
        token: Option<&str>,
        request: String,
        resource_id: &str,
        batch_answer: Option<DataSegmentBatchAnswer>,
        now: SystemTime,
    ) {
        let Some(token) = token else {
            return;
        };
        let key = idempotency_key(operation, token);
        // Bounded by [`MAX_IDEMPOTENT_CALLS`], evicting the token spent longest
        // ago; re-recording a key already held replaces it and needs no room.
        while !self.idempotent_calls.contains_key(&key)
            && self.idempotent_calls.len() >= MAX_IDEMPOTENT_CALLS
        {
            let Some(oldest) = self
                .idempotent_calls
                .iter()
                .min_by_key(|(_, call)| call.spent_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.idempotent_calls.remove(&oldest);
        }
        self.idempotent_calls.insert(
            key,
            IdempotentCall {
                request,
                resource_id: resource_id.to_string(),
                spent_at: now,
                batch_answer,
            },
        );
    }
}
