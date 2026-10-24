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
//! job has got. The service's own settings live here too:
//! the logging options, one entry per scope a call named
//! ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::logging_options`]), and the
//! account's default encryption configuration
//! ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::default_encryption`]). A
//! workspace's capture sources — the video streams `GetCaptureData` reads — live
//! in [`ComAmazonawsIotsitewiseAwsiotsitewiseState::capture_sources`], keyed by
//! the workspace and the source's `timeSeriesId`, with
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::capture_source_aliases`] the
//! index the alias form of that lookup resolves through. Every
//! handler reads and writes THIS struct through [`service_state`]; nothing in
//! `src/ops/` keeps state of its own.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AggregatedValue, Aggregates, Alarms, AssetHierarchy, AssetModelCompositeModel,
    AssetModelCompositeModelPathSegment, AssetModelCompositeModelSummary, AssetModelHierarchy,
    AssetModelProperty, AssetModelPropertyDefinition, AssetModelStatus, AssetProperty,
    AssetPropertyValue, AssetStatus, AssetSummary, ComputeNode, ComputeNodeExecutionDetails,
    ComputeNodeExecutionStatus, ConfigurationStatus, DatasetConfig, DatasetSource, DatasetStatus,
    ExecutionEnvironmentVariables, ExportErrorReportLocation, ExpressionVariable,
    GatewayCapabilitySummary, GatewayPlatform, ImageLocation, LoggingOptions, Mount,
    MountOverrides, PipelineExecutionStatus, PortalStatus, PortalTypeEntry, ProcessingInput,
    PropertyType, ResourceStatus, TaskConfiguration, TimeInNanos, TrimSettings, Variant,
    WorkspaceEncryptionConfiguration, WorkspaceEncryptionConfigurationInfo, WorkspaceStatus,
    ENCRYPTION_TYPE_VALUES, VIDEO_DATA_TYPE_VALUES,
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
/// The per-workspace cap above cannot bound the store on its own: `workspaceName`
/// is a caller-supplied string and this service holds no workspace registry to
/// resolve it against, so a caller naming a fresh workspace each time would open a
/// new per-workspace bucket. This is the whole map's entry count, refused past the
/// cap with the modelled `LimitExceededException` (HTTP 410) — the same bound, and
/// for the same reason, as [`MAX_GATEWAYS`].
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
/// The per-workspace cap above cannot bound the store on its own, for the reason
/// [`MAX_PIPELINES`] states: `workspaceName` is a caller-supplied string this
/// service holds no workspace registry to resolve, so a caller naming a fresh
/// workspace each time would open a new per-workspace bucket every time.
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

/// See [`MAX_ASSET_MODELS`]: the same bound, for the portal map. `CreatePortal`
/// is the only thing that adds a portal and it declares the modelled
/// `LimitExceededException` (HTTP 410), so that is how this store refuses past the
/// cap. The emulator's own bound, not a published AWS default.
pub const MAX_PORTALS: usize = 1_000;

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
/// nothing removes one, so the collection needs a bound of its own: past this
/// many sources a write naming a NEW one is refused rather than evicting a
/// source a caller can still read. The emulator's own bound, not a published AWS
/// quota, and high enough that no ordinary workspace meets it.
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
/// WHY this is its own registry rather than a field on the records
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::property_alias_index`] and
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::disassociated_time_series`]
/// resolve: a capture source is addressed INSIDE a workspace, and those streams
/// belong to no workspace at all — nothing that creates or ingests one records
/// one for it, which is why a workspace-scoped `DescribeTimeSeries` resolves to
/// nothing and a workspace-scoped `ListTimeSeries` is empty. Keying a source by
/// `(workspaceName, timeSeriesId)` is what makes one identifier free in every
/// other workspace, and it stays correct when the operations that ingest a
/// workspace's streams arrive: a stream that gains a workspace gains a capture
/// source under that key, rather than this read having to guess which workspace
/// an account-wide stream belongs to.
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
    /// The source's video, kept sorted ascending by start time and
    /// single-valued at a start time, exactly as a time series' samples are.
    pub segments: Vec<CaptureSegment>,
}

/// A stored asset model: everything `DescribeAssetModel` reports, plus the
/// bookkeeping (`version`, `tags`, `active_at`) the other operations read.
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
    pub tags: HashMap<String, String>,
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

    /// Record that a mutating call — a composite model added to this model, say
    /// — landed at `now`: the definition carries a new version, and the model
    /// reports `UPDATING` until the transition that call owes falls due.
    ///
    /// The due instant is STORED, never slept on, exactly as
    /// [`AssetRecord::begin_update`] does it for an asset. A model that had not
    /// finished being created keeps reporting `CREATING`: the pending transition
    /// it already owed is the one that settles, and it is the same one.
    pub fn begin_update(&mut self, now: SystemTime) {
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
}

/// A stored asset. `properties` is materialised from the referenced model's
/// property definitions at create time, so property writes and reads resolve.
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

    /// The asset property `label` names, resolved the way every `CustomID` member
    /// is — the same lookup as [`AssetRecord::property_position`], for a caller
    /// that wants the property rather than where it sits.
    ///
    /// `DescribeTimeSeries` models its `propertyId` as `CustomID`, so both forms
    /// reach a handler and both must name the same stored property.
    pub fn property(&self, label: &str) -> Option<&AssetProperty> {
        self.properties.get(self.property_position(label)?)
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
    /// wording is the service's not-found rule: the resource kind, then the
    /// identifier the request supplied.
    pub fn message(&self) -> String {
        match self {
            Self::AssetNotFound(label) => not_found_message(RESOURCE_ASSET, label),
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
/// `capability_summaries` starts EMPTY: a freshly created gateway has no
/// capability configured, and only `UpdateGatewayCapabilityConfiguration` adds one.
#[derive(Debug, Clone)]
pub struct GatewayRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub platform: GatewayPlatform,
    pub version: String,
    pub capability_summaries: Vec<GatewayCapabilitySummary>,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    pub tags: HashMap<String, String>,
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
    /// The `clientToken` of the create that made this workspace, if it carried
    /// one, so a retry of that same create replays its answer rather than
    /// colliding with the workspace it already made.
    pub client_token: Option<String>,
    /// The `clientToken` of the last accepted UPDATE, for the same reason: a
    /// retry carrying it "succeeds without performing any further actions".
    ///
    /// Only the LAST one is kept, so a retry of an update another accepted
    /// update has since superseded is treated as a fresh request. The retry a
    /// client actually makes is of its most recent call, which is the one this
    /// recognises.
    pub update_token: Option<String>,
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
        client_token: Option<String>,
        now: SystemTime,
    ) {
        self.description = description;
        self.encryption = encryption;
        self.update_token = client_token;
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
#[derive(Debug, Clone)]
pub struct PipelineExecutionRecord {
    pub id: String,
    pub workspace_name: String,
    pub pipeline_name: String,
    /// The `version` the pipeline carried when the execution was accepted.
    pub pipeline_version: String,
    /// A `PipelineExecutionState` value.
    pub state: String,
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
    /// The compute nodes, in the DAG order
    /// [`compute_node_execution_order`] put them in.
    pub compute_nodes: Vec<ComputeNodeExecutionRecord>,
    /// The `clientToken` of the start that made this execution, if it carried
    /// one, so a retry of that same start replays its answer rather than
    /// starting a second execution; see
    /// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::pipeline_execution_tokens`].
    pub client_token: Option<String>,
}

impl PipelineExecutionRecord {
    /// The execution's status.
    pub fn status(&self) -> PipelineExecutionStatus {
        PipelineExecutionStatus {
            state: self.state.clone(),
            state_details: None,
        }
    }
}

/// The key a started execution's `clientToken` is indexed under: the pipeline it
/// started and the token itself.
///
/// Scoped to the pipeline because a token makes ONE call idempotent, and the
/// token is last so the two names cannot be confused with it (both are modelled
/// `^[a-zA-Z0-9_-]+$`, which admits no `/`).
pub fn execution_token_key(workspace_name: &str, pipeline_name: &str, client_token: &str) -> String {
    format!("{workspace_name}/{pipeline_name}/{client_token}")
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
            waiting[dependent] -= 1;
            if waiting[dependent] == 0 {
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
    /// The `clientToken` of the create that made this pipeline, if it carried
    /// one, so a retry of that same create replays its answer rather than
    /// colliding with the pipeline it already made.
    pub client_token: Option<String>,
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
    /// The `clientToken` of the create that made this task, if it carried one,
    /// so a retry of that same create replays its answer rather than colliding
    /// with the task it already made.
    pub client_token: Option<String>,
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
/// The per-workspace cap above cannot bound the store on its own, for the reason
/// [`MAX_PIPELINES`] states: `workspaceName` is a caller-supplied string this
/// service holds no workspace registry to resolve, so a caller naming a fresh
/// workspace each time would open one more per-workspace bucket every time.
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
    /// The `clientToken` the create carried, if any: what makes a retry of that
    /// same create replay this job rather than start a second one.
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
tagged_record!(PipelineRecord);
tagged_record!(TaskRecord);
tagged_record!(DatasetRecord);
tagged_record!(WorkspaceRecord);

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
    /// The account's default encryption configuration, as the last
    /// `PutDefaultEncryptionConfiguration` left it. `None` is an account nothing
    /// has configured, which reports [`ENCRYPTION_TYPE_SITEWISE_DEFAULT`] without storing
    /// anything — see [`Self::default_encryption_configuration`].
    pub default_encryption: Option<DefaultEncryptionRecord>,
    /// Dataset export jobs by [`dataset_export_job_key`] — the workspace they
    /// were created in and their own id — because every operation on a job
    /// addresses it within a workspace.
    pub dataset_export_jobs: HashMap<(String, String), DatasetExportJobRecord>,
    /// Workspaces by `workspaceName`, which is what identifies one in an
    /// account and region.
    pub workspaces: HashMap<String, WorkspaceRecord>,
    /// Pipeline executions by `pipelineExecutionId`. The id alone identifies one,
    /// and the record names the workspace and pipeline it belongs to, so a read
    /// scoped to another pipeline resolves to nothing.
    pub pipeline_executions: HashMap<String, PipelineExecutionRecord>,
    /// `clientToken` -> `pipelineExecutionId`, keyed by
    /// [`execution_token_key`], so a retried `StartPipelineExecution` carrying
    /// the token of an execution it already started replays that answer instead
    /// of starting a second one. Written beside the execution itself, and
    /// bounded by it: at most one entry per execution that carried a token.
    pub pipeline_execution_tokens: HashMap<String, String>,
    /// The capture sources (video streams) of every workspace: the workspace
    /// they live in, then the source's own `timeSeriesId`, because a capture
    /// source is addressable only inside its workspace — nested so a lookup
    /// borrows the two names instead of allocating a key on every read. Written
    /// by [`Self::put_capture_segment`] and read by `GetCaptureData`.
    pub capture_sources: HashMap<String, HashMap<String, CaptureSourceRecord>>,
    /// `workspaceName` -> `propertyAlias` -> `timeSeriesId`, so the alias form of
    /// a capture-source lookup resolves to the same record the id form does.
    /// Written beside the source itself, and bounded by it: at most one entry per
    /// source that carries an alias.
    pub capture_source_aliases: HashMap<String, HashMap<String, String>>,
}

impl ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// Resolve a `CustomID` label — a raw id or `externalId:<externalId>` — to a
    /// stored `assetModelId`.
    pub fn resolve_asset_model_id(&self, label: &str) -> Option<String> {
        match label.strip_prefix(EXTERNAL_ID_PREFIX) {
            Some(external) => self.asset_model_external_ids.get(external).cloned(),
            None => self.asset_models.contains_key(label).then(|| label.to_string()),
        }
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
    pub fn set_property_alias(
        &mut self,
        asset_id: &str,
        property_id: &str,
        alias: Option<String>,
    ) -> bool {
        let Some(asset) = self.assets.get_mut(asset_id) else {
            return false;
        };
        let Some(at) = asset.property_position(property_id) else {
            return false;
        };
        let property = &mut asset.properties[at];
        let resolved_property_id = property.id.clone();
        let property_data_type = property.data_type.clone();
        let property_data_type_spec = property.data_type_spec.clone();
        let previous = property.alias.take();
        property.alias = alias.clone();

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
                        if let Some(at) = asset.property_position(&held_by_property) {
                            asset.properties[at].alias = None;
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
        self.set_property_alias(asset_id, property_id, None);
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
        self.asset_models
            .values()
            .any(|model| model.composite_model(id).is_some())
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
    pub fn forget_property_series(&mut self, asset_id: &str, property_id: &str) {
        self.set_property_alias(asset_id, property_id, None);
        if let Some(properties) = self.property_values.get_mut(asset_id) {
            properties.remove(property_id);
        }
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
    pub fn put_capture_segment(
        &mut self,
        workspace_name: &str,
        time_series_id: &str,
        alias: Option<&str>,
        data_type: &str,
        segment: CaptureSegment,
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
                segments: Vec::new(),
            });

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
            PIPELINE_RESOURCE_TYPE => {
                let (workspace, name) = id.split_once('/')?;
                tags_of(self.pipelines.get(&pipeline_key(workspace, name)), arn)
            }
            TASK_RESOURCE_TYPE => {
                let (workspace, name) = id.split_once('/')?;
                tags_of(self.tasks.get(&task_key(workspace, name)), arn)
            }
            DATASET_RESOURCE_TYPE => tags_of(self.datasets.get(id), arn),
            WORKSPACE_RESOURCE_TYPE => tags_of(self.workspaces.get(id), arn),
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
            PIPELINE_RESOURCE_TYPE => {
                let (workspace, name) = id.split_once('/')?;
                tags_of_mut(self.pipelines.get_mut(&pipeline_key(workspace, name)), arn)
            }
            TASK_RESOURCE_TYPE => {
                let (workspace, name) = id.split_once('/')?;
                tags_of_mut(self.tasks.get_mut(&task_key(workspace, name)), arn)
            }
            DATASET_RESOURCE_TYPE => tags_of_mut(self.datasets.get_mut(id), arn),
            WORKSPACE_RESOURCE_TYPE => tags_of_mut(self.workspaces.get_mut(id), arn),
            _ => None,
        }
    }

    /// Forget an asset and everything keyed off it: its external-id entry, its
    /// property series, its property aliases and its hierarchy associations.
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
        Some(record)
    }

    /// Forget an asset model and its external-id entry.
    pub fn forget_asset_model(&mut self, asset_model_id: &str) -> Option<AssetModelRecord> {
        let record = self.asset_models.remove(asset_model_id)?;
        if let Some(external) = record.external_id.as_deref() {
            self.asset_model_external_ids.remove(external);
        }
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
    /// `asset_model_id` is a stored id, not a label: resolve it with
    /// [`Self::resolve_asset_model_id`] first. Does nothing when no stored model
    /// has that id, or when no asset was created from it.
    pub fn propagate_asset_model_definitions(&mut self, asset_model_id: &str, now: SystemTime) {
        let Some(model) = self.asset_models.get(asset_model_id) else {
            return;
        };
        // Cloned so the per-asset writes below are not borrowing the model.
        let definitions = model.properties.clone();
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
            // A stored model property without an id would make an unaddressable
            // asset property, so it is skipped rather than materialised with an
            // empty id — the same filter `CreateAsset` applies.
            //
            // The same pass collects what a property this asset did not carry
            // STARTS with: it is materialised HERE for the first time, so it takes
            // the sample its definition declares exactly as a property materialised
            // by a create does (see [`initial_property_sample`]). A property the
            // asset already carried keeps the series written to it.
            let mut properties: Vec<AssetProperty> = Vec::with_capacity(definitions.len());
            let mut seeded: Vec<(String, AssetPropertyValue)> = Vec::new();
            for definition in &definitions {
                let Some(id) = definition.id.clone().filter(|id| !id.is_empty()) else {
                    continue;
                };
                let held = carried.iter().find(|property| property.id == id);
                if held.is_none() {
                    if let Some(sample) = initial_property_sample(definition, now) {
                        seeded.push((id.clone(), sample));
                    }
                }
                properties.push(AssetProperty {
                    id,
                    external_id: definition.external_id.clone(),
                    name: definition.name.clone(),
                    alias: held.and_then(|property| property.alias.clone()),
                    notification: held.and_then(|property| property.notification.clone()),
                    data_type: definition.data_type.clone(),
                    data_type_spec: definition.data_type_spec.clone(),
                    unit: definition.unit.clone(),
                    path: None,
                });
            }
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
                .filter(|property| {
                    !properties.iter().any(|kept| kept.id == property.id)
                })
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

/// The resource kind those errors name for a dataset export job.
pub const RESOURCE_DATASET_EXPORT_JOB: &str = "Dataset Export Job";

/// The resource kind those errors name for a workspace.
pub const RESOURCE_WORKSPACE: &str = "Workspace";

/// The resource kind those errors name for a pipeline execution.
pub const RESOURCE_PIPELINE_EXECUTION: &str = "Pipeline Execution";

/// The resource kind those errors name for a composite model of an asset model.
///
/// A composite model is not a top-level resource, so it is named as what it is
/// rather than as an "Asset Model"; it lives here beside its siblings so the
/// operations that address one cannot each invent a kind of their own.
pub const RESOURCE_ASSET_MODEL_COMPOSITE_MODEL: &str = "Asset Model Composite Model";

/// The resource kind those errors name for a gateway.
///
/// AWS's own wording was observed for the asset and asset-model paths; for this
/// kind and the three below, the label is the service's resource name under that
/// same one form, so every handler addressing one states the absence identically.
pub const RESOURCE_GATEWAY: &str = "Gateway";

/// The resource kind those errors name for a data stream, which every operation
/// that addresses one addresses by its `alias`.
pub const RESOURCE_TIME_SERIES: &str = "Time Series";

/// The resource kind those errors name for an IoT SiteWise Monitor portal.
pub const RESOURCE_PORTAL: &str = "Portal";

/// The message a not-found refusal carries: the resource kind, then the
/// identifier the REQUEST supplied, echoed exactly as sent — whatever form that
/// identifier took for the resource in question (a raw id, an `externalId:`
/// label, a stream alias, a workspace name).
///
/// One rule for every not-found path in this service, so no handler invents its
/// own template: AWS states the absence as `<Resource> with ID <id> not found`,
/// which is why the identifier is echoed rather than replaced by whatever the
/// handler resolved.
pub fn not_found_message(resource: &str, requested: &str) -> String {
    format!("{resource} with ID {requested} not found")
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

/// The `ETag` the `If-Match` / `If-None-Match` preconditions of an asset-model
/// write are evaluated against, as of `now`, or `None` when the version
/// `version_type` names does not exist.
///
/// `matchForVersionType` targets the modelled `AssetModelVersionType` enum
/// (`LATEST | ACTIVE`) and [`VERSION_TYPE_LATEST`] is its documented default.
/// This emulator keeps ONE version of an asset model, so that version is always
/// its latest one and is also its ACTIVE one once the transition the clock owes
/// has fallen due; while one is still pending the model has no active version at
/// all, which is the `None` here. Shared by every operation that carries those
/// headers, so `CreateAssetModelCompositeModel`, `UpdateAssetModel` and
/// `DeleteAssetModel` cannot disagree about which tag a precondition targets.
pub fn asset_model_target_etag(
    record: &AssetModelRecord,
    version_type: Option<&str>,
    now: SystemTime,
) -> Option<String> {
    let version_type = version_type.unwrap_or(VERSION_TYPE_LATEST);
    if version_type == VERSION_TYPE_ACTIVE && record.state(now) != STATE_ACTIVE {
        return None;
    }
    Some(asset_model_etag(record))
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
/// This emulator keeps ONE version of each asset model, so its latest and its
/// active version share this tag; while an owed transition is still pending the
/// model has no active version at all (see [`AssetModelRecord::state`]).
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

/// The opaque pagination token that resumes at the [`timestamp_key`] `resume`:
/// the hex of `seconds.nanos`.
///
/// ONE recipe for every time-ranged read of this service that pages by the
/// instant it stopped at, so an encoding fix cannot drift between operations.
/// Hex keeps the text inside the `^[A-Za-z0-9+/=]+$` pattern the paging members
/// of this service carry.
pub fn encode_timestamp_token(resume: (i64, i32)) -> String {
    let (seconds, nanos) = resume;
    ::hex::encode(format!("{seconds}{TIMESTAMP_TOKEN_SEPARATOR}{nanos}").as_bytes())
}

/// The [`timestamp_key`] `token` encodes, or `None` for text this service never
/// issued as a token — which its caller answers as the modelled refusal for an
/// invalid `nextToken`.
pub fn decode_timestamp_token(token: &str) -> Option<(i64, i32)> {
    let decoded = ::hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())?;
    let (seconds, nanos) = decoded.split_once(TIMESTAMP_TOKEN_SEPARATOR)?;
    Some((seconds.parse().ok()?, nanos.parse().ok()?))
}

/// What joins the two halves of a token [`encode_timestamp_token`] builds.
const TIMESTAMP_TOKEN_SEPARATOR: char = '.';

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
        let decoded = ::hex::decode(token)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())?;
        let (entry, value) = decoded.split_once(BATCH_CURSOR_SEPARATOR)?;
        Some(Self {
            entry: entry.parse().ok()?,
            value: value.parse().ok()?,
        })
    }
}

/// What joins the two halves of a token [`BatchCursor::encode`] builds.
const BATCH_CURSOR_SEPARATOR: char = ':';

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

/// Refuse a tag key outside the modelled `TagKey` bounds, in the wording the
/// boundary validator uses for a map's keys.
///
/// The framework's constraint walk reaches a map's VALUES but not its KEYS, so
/// `TagMap`'s `@length(min: 1, max: 128)` on `TagKey` is the handler's to
/// enforce; the length is in characters, as `@length` counts them.
pub fn check_tag_key(key: &str) -> Result<(), String> {
    let length = key.chars().count();
    if (TAG_KEY_MIN_LENGTH..=TAG_KEY_MAX_LENGTH).contains(&length) {
        return Ok(());
    }
    Err(format!(
        "1 validation error detected: Value at 'tags' failed to satisfy constraint: \
         Map keys must satisfy constraint: [Member must have length greater than or equal to \
         {TAG_KEY_MIN_LENGTH}, Member must have length less than or equal to {TAG_KEY_MAX_LENGTH}]"
    ))
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
fn timestamp_nanos(timestamp: &TimeInNanos) -> i128 {
    i128::from(timestamp.time_in_seconds) * 1_000_000_000
        + i128::from(timestamp.offset_in_nanos.unwrap_or(0))
}

/// A modelled `Timestamp` as whole nanoseconds since the Unix epoch; see
/// [`timestamp_nanos`].
pub fn date_time_nanos(at: &::aws_smithy_types::DateTime) -> i128 {
    i128::from(at.secs()) * 1_000_000_000 + i128::from(at.subsec_nanos())
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

impl DefaultEncryptionRecord {
    /// The `ConfigurationState` this configuration reports at `now`.
    pub fn state(&self, now: SystemTime) -> &'static str {
        if now >= self.active_at {
            CONFIGURATION_STATE_ACTIVE
        } else {
            CONFIGURATION_STATE_UPDATE_IN_PROGRESS
        }
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
