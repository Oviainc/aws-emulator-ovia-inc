// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iottwinmaker-awsiottwinmaker service.
//!
//! IoT TwinMaker's control plane is a two-level tree: a *workspace* owns the
//! *entities*, *component types*, *scenes* and *sync jobs* created inside it.
//! That containment is modelled literally here — `Workspace::entities`,
//! `Workspace::component_types`, `Workspace::scenes` and
//! `Workspace::sync_jobs` — so none of them can outlive its workspace and two
//! operations cannot read two copies of one resource.
//!
//! Nothing derivable is stored: `hasChildEntities` is computed by scanning the
//! workspace's entities for the id as a parent, and a workspace's
//! `linkedServices` by reading the sources of the sync jobs it holds
//! (`Workspace::linked_services`). A resource's `state`, by
//! contrast, IS stored. An entity's create settles it in `ACTIVE` within the
//! call that asked for it, so there is no provisioning instant for a read to
//! compute a transient state from; a component type's create answers the
//! non-terminal `CREATING`, so its record also carries the instant its
//! transition to `ACTIVE` falls due (`ComponentType::settles_at`), which a
//! reader materialises rather than sleeps for.
//!
//! Not everything is inside that tree: a *metadata transfer job* is addressed
//! with no workspace in its path and may name a different workspace at each
//! end of the transfer, so the jobs live beside the workspaces
//! (`metadata_transfer_jobs`). A job is accepted in the non-terminal
//! `VALIDATING` state and carries the instant its outcome falls due
//! (`MetadataTransferJob::settles_at`), the same way a component type carries
//! its own — and so does a *sync job* (`SyncJob::settles_at`), which
//! `CreateSyncJob` accepts in the non-terminal `CREATING` state.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, SystemTime};

/// The `ParentEntityId` sentinel the service uses for an entity with no parent.
/// Taken from the `ParentEntityId` pattern, which admits the literal `$ROOT`
/// beside a uuid and a plain id.
pub const ROOT_PARENT_ENTITY_ID: &str = "$ROOT";

/// The `State` enum values this slice produces. The full value list is
/// `STATE_VALUES` in `ops::types` (`CREATING`, `UPDATING`, `DELETING`, `ACTIVE`,
/// `ERROR`).
///
/// `ACTIVE` is the state an entity SETTLES in, and the one a stored entity
/// carries: a create completes within the call that asked for it, so no entity
/// is ever persisted in the non-terminal `CREATING` state — recorded traffic
/// reads a just-created entity back as `ACTIVE`, with no step anywhere in the
/// corpus observing it `CREATING`. The other values belong to transitions a
/// later slice owns (`UPDATING` for an update in flight, `ERROR` for a failed
/// one); `DELETING` is what `DeleteEntity` reports for the removal it performs.
pub const STATE_ACTIVE: &str = "ACTIVE";
/// See [`STATE_ACTIVE`].
pub const STATE_DELETING: &str = "DELETING";
/// See [`STATE_ACTIVE`]. A COMPONENT TYPE, unlike an entity, is reported in
/// this non-terminal state by the call that creates it and settles in
/// [`STATE_ACTIVE`] once [`COMPONENT_TYPE_SETTLE_SPAN`] has elapsed.
pub const STATE_CREATING: &str = "CREATING";
/// See [`STATE_ACTIVE`]. The non-terminal state an UPDATE in flight leaves a
/// component type in: `UpdateComponentType` answers with it, and the clock
/// settles the component type back in [`STATE_ACTIVE`] once
/// [`COMPONENT_TYPE_SETTLE_SPAN`] has elapsed, exactly as it does after a
/// create.
pub const STATE_UPDATING: &str = "UPDATING";

/// How long a component type stays in a non-terminal state — [`STATE_CREATING`]
/// after a create, [`STATE_UPDATING`] after an update — before the clock makes
/// its transition to [`STATE_ACTIVE`] due.
///
/// `CreateComponentType` answers with the component type's `state`, and the
/// state of a component type the service has just accepted is the non-terminal
/// `CREATING` — so there IS a provisioning window here, unlike an entity, which
/// is reported `ACTIVE` by its own create. `UpdateComponentType` opens the same
/// window with `UPDATING`: the call reports the change as in flight and the
/// component type is `ACTIVE` again once it has landed. AWS documents no
/// duration for either window, so this span is CHOSEN by this emulator; it is
/// deliberately non-zero, so a caller that reads the component type back in the
/// same breath as the create or the update still observes the transient state,
/// and deliberately short, so a client that polls reaches `ACTIVE` without a
/// long wait. Nothing sleeps for it: the instant it is due is stored on the
/// record (see [`ComponentType::settles_at`]) and a reader materialises the
/// transition the clock already owed.
pub const COMPONENT_TYPE_SETTLE_SPAN: Duration = Duration::from_millis(500);

/// The name of the function whose presence makes a component type's schema
/// initialized.
///
/// `isSchemaInitialized` is documented as "whether the component type has a
/// schema initializer and that the schema initializer has run": the schema
/// initializer is the entry of a component type's `functions` map under this
/// name, so a component type whose `functions` carry none has no schema to
/// initialize. See [`ComponentType::is_schema_initialized`].
pub const SCHEMA_INITIALIZER_FUNCTION: &str = "schemaInitializer";

/// The region an emulated ARN is built in when a request carries no SigV4
/// credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The largest page any list operation of this service serves, applied
/// SERVER-SIDE so a request cannot ask for an unbounded one, and the page size
/// served when a request names none.
///
/// Both come from the `maxResults` member's own documentation: "The maximum
/// number of results to return at one time. The default is 25." The ceiling is
/// the top of the member's modelled range (0..200), applied by the service
/// whatever the caller asks for, so the page is bounded by this code and not
/// only by the boundary validator.
pub const MAX_PAGE_SIZE: usize = 200;
/// See [`MAX_PAGE_SIZE`].
pub const DEFAULT_PAGE_SIZE: usize = 25;

/// The most components `GetEntity` reports for one entity, and the most
/// properties it reports for one component.
///
/// Both are documented on the flags that report whether the response was
/// truncated: "The maximum number of components returned is 30"
/// (`areAllComponentsReturned`) and "The maximum number of properties returned
/// is 800" (`areAllPropertiesReturned`).
pub const MAX_COMPONENTS_RETURNED: usize = 30;
/// See [`MAX_COMPONENTS_RETURNED`].
pub const MAX_PROPERTIES_RETURNED: usize = 800;

/// The most tags one taggable resource of this service carries.
///
/// Taken from the model, not chosen here: `TagMap` is `@length(min: 0, max:
/// 50)` (`smithy_query(query="shape",shape="TagMap")`), so fifty entries is the
/// largest tag set any of this service's shapes can carry on a resource. A
/// `TagResource` call that would leave a resource past it is refused with the
/// `TooManyTagsException` (HTTP 400) that operation models.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The most workspaces this emulator holds, the most entities one workspace
/// holds, and the most component types one workspace holds, before
/// `CreateWorkspace` / `CreateEntity` / `CreateComponentType` answer the
/// `ServiceQuotaExceededException` (HTTP 402) each operation models.
///
/// The real service's quotas are per-account and adjustable, so no fixed number
/// is AWS's; these are this emulator's own declared ceilings, so the in-memory
/// store cannot be grown without bound by a caller that keeps creating.
pub const MAX_WORKSPACES: usize = 50;
/// See [`MAX_WORKSPACES`].
pub const MAX_ENTITIES_PER_WORKSPACE: usize = 1000;
/// See [`MAX_WORKSPACES`].
pub const MAX_COMPONENT_TYPES_PER_WORKSPACE: usize = 1000;
/// See [`MAX_WORKSPACES`].
pub const MAX_SCENES_PER_WORKSPACE: usize = 1000;
/// See [`MAX_WORKSPACES`]. A metadata transfer job is an ACCOUNT-level
/// resource (`/metadata-transfer-jobs/{metadataTransferJobId}` names no
/// workspace), so its ceiling is per account rather than per workspace, and
/// `CreateMetadataTransferJob` answers the `ServiceQuotaExceededException` it
/// models once the account holds this many.
pub const MAX_METADATA_TRANSFER_JOBS: usize = 1000;

/// The `MetadataTransferJobState` a job the service has just accepted is in,
/// and the terminal state it reaches.
///
/// `VALIDATING` is the state `CreateMetadataTransferJob` answers with: the job
/// has been accepted and nothing has transferred yet, so it is non-terminal and
/// the clock owes its outcome ([`METADATA_TRANSFER_JOB_SETTLE_SPAN`]). `ERROR`
/// is the modelled failure state that outcome is in this emulator — see
/// [`MetadataTransferJob::settle`] — and `CANCELLED`/`CANCELLING` belong to
/// `CancelMetadataTransferJob`, which owns them. The full value list is
/// `METADATA_TRANSFER_JOB_STATE_VALUES` in `ops::types`.
pub const METADATA_TRANSFER_JOB_STATE_VALIDATING: &str = "VALIDATING";
/// See [`METADATA_TRANSFER_JOB_STATE_VALIDATING`].
pub const METADATA_TRANSFER_JOB_STATE_ERROR: &str = "ERROR";

/// How long a metadata transfer job stays in
/// [`METADATA_TRANSFER_JOB_STATE_VALIDATING`] before the clock makes its
/// outcome due.
///
/// AWS documents no duration for a metadata transfer, so this span is CHOSEN by
/// this emulator, the same way (and for the same reasons) as
/// [`COMPONENT_TYPE_SETTLE_SPAN`]: deliberately non-zero, so a caller that
/// reads the job back in the same breath as the create still observes the
/// non-terminal state the create answered with, and deliberately short, so a
/// client that polls reaches the terminal state without a long wait. Nothing
/// sleeps for it: the instant it is due is stored on the record
/// ([`MetadataTransferJob::settles_at`]) and a reader materialises the
/// transition the clock already owed.
pub const METADATA_TRANSFER_JOB_SETTLE_SPAN: Duration = Duration::from_millis(500);

/// The `ErrorCode` a metadata transfer job that never ran carries
/// (`ops::types::ERROR_CODE_VALUES`), and the message that says why.
///
/// Transferring metadata means reading an Amazon S3 object, an IoT SiteWise
/// asset model or another workspace's metadata document and writing what it
/// describes into the destination — work this emulator does not perform.
/// Nothing therefore runs for an accepted job, and a job nothing ran ends in
/// the service's own modelled failure state with the reason that says so,
/// rather than being reported `COMPLETED` for a transfer that never happened.
/// The message is this emulator's, not AWS's: it states the emulator's
/// limitation, which no AWS rule produces.
pub const METADATA_TRANSFER_JOB_ERROR_CODE: &str = "PROCESSING_ERROR";
/// See [`METADATA_TRANSFER_JOB_ERROR_CODE`].
pub const METADATA_TRANSFER_JOB_ERROR_MESSAGE: &str =
    "this emulator transfers no metadata: nothing was read from the job's sources and nothing \
     was written to its destination";

/// The resource portion of the ARN of the service-linked role a metadata
/// transfer job runs under.
///
/// `GetMetadataTransferJob` reports `metadataTransferJobRole` as `@required`,
/// but `CreateMetadataTransferJobRequest` has no role member at all, so the
/// role is the SERVICE's own and never the caller's input: the role IoT
/// TwinMaker assumes to transfer metadata. The ARN is built through
/// [`metadata_transfer_job_role`], so its account segment is the emulator's
/// canonical one.
pub const METADATA_TRANSFER_JOB_ROLE_PATH: &str =
    "role/aws-service-role/iottwinmaker.amazonaws.com/AWSServiceRoleForIoTTwinMaker";

/// The `MetadataTransferJobState` values `CancelMetadataTransferJob` owns, and
/// the third TERMINAL value of the enum.
///
/// `CANCELLING` is the state a cancellation the service has accepted puts the
/// job in: the transfer is being torn down, so it is non-terminal and the clock
/// owes `CANCELLED` ([`METADATA_TRANSFER_JOB_CANCEL_SPAN`]). `CANCELLED` and
/// `COMPLETED` are terminal, as [`METADATA_TRANSFER_JOB_STATE_ERROR`] is:
/// nothing moves a job out of one, so there is nothing left to cancel
/// ([`metadata_transfer_job_state_is_terminal`]). `COMPLETED` is a state no
/// operation of this emulator writes — it is the outcome of a transfer that ran,
/// and none does (see [`METADATA_TRANSFER_JOB_ERROR_CODE`]) — but it is named
/// here because the terminal rule holds for it too. The full value list is
/// `METADATA_TRANSFER_JOB_STATE_VALUES` in `ops::types`.
pub const METADATA_TRANSFER_JOB_STATE_CANCELLING: &str = "CANCELLING";
/// See [`METADATA_TRANSFER_JOB_STATE_CANCELLING`]. TERMINAL.
pub const METADATA_TRANSFER_JOB_STATE_CANCELLED: &str = "CANCELLED";
/// See [`METADATA_TRANSFER_JOB_STATE_CANCELLING`]. TERMINAL.
pub const METADATA_TRANSFER_JOB_STATE_COMPLETED: &str = "COMPLETED";

/// How long a metadata transfer job stays in
/// [`METADATA_TRANSFER_JOB_STATE_CANCELLING`] before the clock makes its
/// transition to [`METADATA_TRANSFER_JOB_STATE_CANCELLED`] due.
///
/// `CancelMetadataTransferJob` answers with the job's status, and the status of
/// a job whose cancellation the service has just accepted is the non-terminal
/// `CANCELLING` — the transfer is being torn down, and the job is `CANCELLED`
/// once that teardown has landed. AWS documents no duration for that window, so
/// this span is CHOSEN by this emulator, exactly as
/// [`METADATA_TRANSFER_JOB_SETTLE_SPAN`] and [`COMPONENT_TYPE_SETTLE_SPAN`] are:
/// deliberately non-zero, so a caller that reads the job back in the same breath
/// as the cancel still observes `CANCELLING`, and deliberately short, so a client
/// that polls reaches `CANCELLED` without a long wait. Nothing sleeps for it: the
/// instant it is due is stored on the record
/// ([`MetadataTransferJob::settles_at`]) and a reader materialises the
/// transition the clock already owed.
pub const METADATA_TRANSFER_JOB_CANCEL_SPAN: Duration = Duration::from_millis(500);

/// The one `SyncSource` this service supports.
///
/// Taken from the model, not chosen here: the `syncSource` member of every sync
/// operation documents "Currently the only supported syncSource is SITEWISE"
/// (`smithy_query(query="operation",operation="CreateSyncJob")`), and the
/// `SyncSource` shape itself is a plain `^[a-zA-Z_0-9]+$` string that cannot say
/// so — so a create naming any other source is refused with the
/// `ValidationException` the operation models rather than stored as a job
/// against a source nothing can sync from.
pub const SYNC_SOURCE_SITEWISE: &str = "SITEWISE";

/// The `SyncJobState` a sync job the service has just accepted is in, and the
/// terminal state it reaches.
///
/// `CREATING` is the state `CreateSyncJob` answers with: the job has been
/// accepted and nothing has been synchronised yet, so it is non-terminal and the
/// clock owes its outcome ([`SYNC_JOB_SETTLE_SPAN`]). `ERROR` is the modelled
/// failure state that outcome is in this emulator — see [`SyncJob::settle`] —
/// while `INITIALIZING` and `ACTIVE` belong to a synchronisation that really
/// ran and `DELETING` to `DeleteSyncJob`, which owns it. The full value list is
/// `SYNC_JOB_STATE_VALUES` in `ops::types`.
pub const SYNC_JOB_STATE_CREATING: &str = "CREATING";
/// See [`SYNC_JOB_STATE_CREATING`].
pub const SYNC_JOB_STATE_ERROR: &str = "ERROR";

/// How long a sync job stays in [`SYNC_JOB_STATE_CREATING`] before the clock
/// makes its outcome due.
///
/// AWS documents no duration for bringing a sync job up, so this span is CHOSEN
/// by this emulator, the same way (and for the same reasons) as
/// [`METADATA_TRANSFER_JOB_SETTLE_SPAN`]: deliberately non-zero, so a caller
/// that reads the job back in the same breath as the create still observes the
/// non-terminal state the create answered with, and deliberately short, so a
/// client that polls reaches the terminal state without a long wait. Nothing
/// sleeps for it: the instant it is due is stored on the record
/// ([`SyncJob::settles_at`]) and a reader materialises the transition the clock
/// already owed.
pub const SYNC_JOB_SETTLE_SPAN: Duration = Duration::from_millis(500);

/// The `ErrorCode` a sync job that never synchronised anything carries
/// (`ops::types::ERROR_CODE_VALUES`), and the message that says why.
///
/// Synchronising means reading the sync source — an IoT SiteWise asset model
/// hierarchy — and creating, updating or deleting the corresponding entities
/// and component types in the workspace, which is work this emulator does not
/// perform: it holds no link to the source service. Nothing therefore runs for
/// an accepted job, and a job nothing ran ends in the service's own modelled
/// failure state with the reason that says so, rather than being reported
/// `ACTIVE` for a synchronisation that is not happening. `SYNC_INITIALIZING_ERROR`
/// is the code for a job that failed bringing the sync up, which is where this
/// one stops. The message is this emulator's, not AWS's: it states the
/// emulator's limitation, which no AWS rule produces.
pub const SYNC_JOB_ERROR_CODE: &str = "SYNC_INITIALIZING_ERROR";
/// See [`SYNC_JOB_ERROR_CODE`].
pub const SYNC_JOB_ERROR_MESSAGE: &str =
    "this emulator synchronises nothing: no resource was read from the sync source and none was \
     created, updated or deleted in the workspace";

/// The `PricingMode` an account that has never changed its pricing plan is on,
/// and the `UpdateReason` such a plan carries.
///
/// `BASIC` is the first value of the model's `PricingMode` enum
/// (`ops::types::PRICING_MODE_VALUES`) and the pay-as-you-go mode the service
/// bills an untouched account in; `DEFAULT` is the `UpdateReason` value that
/// names exactly that — a plan nobody has updated
/// (`ops::types::UPDATE_REASON_VALUES`).
pub const PRICING_MODE_BASIC: &str = "BASIC";
/// See [`PRICING_MODE_BASIC`].
pub const UPDATE_REASON_DEFAULT: &str = "DEFAULT";

/// The `UpdateReason` a plan change records when it replaces a plan whose
/// `pricingMode` differs from the one in force
/// (`ops::types::UPDATE_REASON_VALUES`).
pub const UPDATE_REASON_PRICING_MODE_UPDATE: &str = "PRICING_MODE_UPDATE";
/// The `UpdateReason` a plan change records when the mode is unchanged, so the
/// bundles — and with them the tier the bundles place the account in — are all
/// the change can have moved. See [`UPDATE_REASON_PRICING_MODE_UPDATE`].
pub const UPDATE_REASON_PRICING_TIER_UPDATE: &str = "PRICING_TIER_UPDATE";
/// The `UpdateReason` a plan change records when it supersedes a plan change
/// that had been accepted but had not taken effect: that earlier change is
/// overwritten and never becomes the plan in force. See
/// [`UPDATE_REASON_PRICING_MODE_UPDATE`].
pub const UPDATE_REASON_OVERWRITTEN: &str = "OVERWRITTEN";

/// Whether `pricing_mode` names a BUNDLED pricing plan — one whose billing is
/// described by the bundles it was set with — rather than the pay-as-you-go
/// mode.
///
/// [`PRICING_MODE_BASIC`] is the pay-as-you-go mode, and it is the mode of the
/// plan an untouched account is on, which names no bundle at all
/// ([`PricingPlanRecord::default_plan`]). Every other `PricingMode` value
/// (`ops::types::PRICING_MODE_VALUES`) names a plan bought as bundles, and a
/// bundled plan is reported through `BundleInformation`, whose `bundleNames` is
/// `@required` with `@length(1..10)` — so there is no form of a bundled plan
/// that names no bundle, and no form of the pay-as-you-go plan that names one.
pub fn pricing_mode_is_bundled(pricing_mode: &str) -> bool {
    pricing_mode != PRICING_MODE_BASIC
}

/// Per-service state for com-amazonaws-iottwinmaker-awsiottwinmaker. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsIottwinmakerAwsiottwinmakerState {
    /// Workspaces by `workspaceId`.
    pub workspaces: BTreeMap<String, Workspace>,
    /// Every ARN this service has minted, so an operation whose input accepts an
    /// id *or* an ARN (`GetWorkspace`, and the tagging operations in a later
    /// slice) resolves one without scanning every workspace.
    pub arn_index: BTreeMap<String, ResourceRef>,
    /// The pricing plan this account is billed under for the current billing
    /// cycle — the one `GetPricingPlan` reports as `currentPricingPlan`.
    ///
    /// Account-wide, not per workspace: the pricing operations carry no
    /// workspace in their input, so there is ONE plan per account and region,
    /// held here beside the workspaces rather than on each of them. Present
    /// rather than optional, because an account always HAS a plan: until
    /// something changes it, that plan is the service default
    /// ([`PricingPlanRecord::default_plan`]), effective from the instant this
    /// store came into being — so a read never has to mint one, and two reads
    /// of an unchanged plan report the same instants.
    pub pricing_plan: PricingPlanRecord,
    /// The plan change that is accepted but not yet in force — what
    /// `GetPricingPlan` reports as `pendingPricingPlan` — and `None` while
    /// nothing is pending, which is what an account whose plan nobody has
    /// changed looks like.
    ///
    /// Nothing in this emulator leaves a change pending: `UpdatePricingPlan`
    /// applies one at once ([`Self::set_pricing_plan`]), so the only thing that
    /// call does to this field is CLEAR it, superseding a change that had not
    /// taken effect. The field stays because the plan in force and the plan
    /// pending are two distinct members of the API's output, and a reader has
    /// to report whichever the store holds rather than assume there is only
    /// ever one.
    pub pending_pricing_plan: Option<PricingPlanRecord>,
    /// The metadata transfer jobs this account holds, keyed by
    /// `metadataTransferJobId`.
    ///
    /// Account-wide, beside the workspaces rather than inside one: a job is
    /// addressed as `/metadata-transfer-jobs/{metadataTransferJobId}`, which
    /// names no workspace, and one job's source and destination may name
    /// different workspaces (or none at all, for an Amazon S3 or an IoT
    /// SiteWise end), so no single workspace can own it. A job's ARN is
    /// deliberately NOT in [`arn_index`](Self::arn_index): that index exists to
    /// resolve an id-or-ARN input and to find a taggable resource's tag map,
    /// and a metadata transfer job is addressed only by its id and carries no
    /// `tags` member on any of its shapes.
    pub metadata_transfer_jobs: BTreeMap<String, MetadataTransferJob>,
}

impl Default for ComAmazonawsIottwinmakerAwsiottwinmakerState {
    fn default() -> Self {
        Self {
            workspaces: BTreeMap::new(),
            arn_index: BTreeMap::new(),
            // The account's plan dates from the moment the account's store
            // exists, so the default plan's instants are captured here, ONCE,
            // rather than by whichever call happens to read it first.
            pricing_plan: PricingPlanRecord::default_plan(SystemTime::now()),
            pending_pricing_plan: None,
            metadata_transfer_jobs: BTreeMap::new(),
        }
    }
}

/// One stored pricing plan: the mode and bundles it was set to, and when.
///
/// `billableEntityCount` is NOT a field here. It is the account's current
/// billable entity count, which the stored entities already say
/// ([`ComAmazonawsIottwinmakerAwsiottwinmakerState::billable_entity_count`]) —
/// keeping a copy beside the plan would let an entity created or deleted after
/// the plan was set leave it stale, the same reason `hasChildEntities` is
/// derived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricingPlanRecord {
    /// A `PricingMode` value (`ops::types::PRICING_MODE_VALUES`).
    pub pricing_mode: String,
    /// An `UpdateReason` value (`ops::types::UPDATE_REASON_VALUES`): why this
    /// plan replaced the one before it.
    pub update_reason: String,
    /// The bundles this plan was set with, empty when it names none — a plan
    /// without bundles reports no `bundleInformation` at all, since
    /// `BundleInformation::bundleNames` is `@required` and `@length(1..10)`, so
    /// an empty list is not a value the shape admits.
    pub bundle_names: Vec<String>,
    /// The `PricingTier` the bundles place the account in
    /// (`ops::types::PRICING_TIER_VALUES`), absent when the plan carries no
    /// tier.
    pub pricing_tier: Option<String>,
    /// When this plan takes (or took) effect.
    pub effective_date_time: SystemTime,
    /// When this plan was set.
    pub update_date_time: SystemTime,
}

impl PricingPlanRecord {
    /// The plan an account that has never changed its pricing plan is on:
    /// [`PRICING_MODE_BASIC`] with no bundles, set and effective at `at` and
    /// carrying the [`UPDATE_REASON_DEFAULT`] reason.
    pub fn default_plan(at: SystemTime) -> Self {
        Self {
            pricing_mode: PRICING_MODE_BASIC.to_string(),
            update_reason: UPDATE_REASON_DEFAULT.to_string(),
            bundle_names: Vec::new(),
            pricing_tier: None,
            effective_date_time: at,
            update_date_time: at,
        }
    }

    /// This record as the modelled `PricingPlan`, reported with the account's
    /// current `billable_entity_count`.
    ///
    /// Shared by every reader of a stored plan, so the current and the pending
    /// plan — and `GetPricingPlan` and `UpdatePricingPlan` — report one plan the
    /// same way.
    pub fn as_pricing_plan(&self, billable_entity_count: i64) -> crate::ops::types::PricingPlan {
        crate::ops::types::PricingPlan {
            billable_entity_count: Some(billable_entity_count),
            // Omitted while the plan names no bundle: `bundleNames` is required
            // inside `BundleInformation`, so there is no empty form of it.
            bundle_information: (!self.bundle_names.is_empty()).then(|| {
                crate::ops::types::BundleInformation {
                    bundle_names: self.bundle_names.clone(),
                    pricing_tier: self.pricing_tier.clone(),
                }
            }),
            effective_date_time: timestamp(self.effective_date_time),
            pricing_mode: self.pricing_mode.clone(),
            update_date_time: timestamp(self.update_date_time),
            update_reason: self.update_reason.clone(),
        }
    }
}

/// What an ARN in [`ComAmazonawsIottwinmakerAwsiottwinmakerState::arn_index`] points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceRef {
    /// A workspace, by id.
    Workspace { workspace_id: String },
    /// An entity, by the workspace that owns it and its own id.
    Entity {
        workspace_id: String,
        entity_id: String,
    },
    /// A component type, by the workspace that owns it and its own id.
    ComponentType {
        workspace_id: String,
        component_type_id: String,
    },
    /// A scene, by the workspace that owns it and its own id.
    Scene {
        workspace_id: String,
        scene_id: String,
    },
    /// A sync job, by the workspace that owns it and the sync source that
    /// identifies it inside that workspace.
    SyncJob {
        workspace_id: String,
        sync_source: String,
    },
}

/// One stored workspace.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub workspace_id: String,
    pub arn: String,
    /// The workspace's description, the empty string when the create named
    /// none: `Description` is modelled `@length 0..2048`, so the empty string
    /// is a legal value, and the service carries the member on every resource
    /// rather than leaving it absent. See [`Entity::description`].
    pub description: String,
    pub s3_location: Option<String>,
    pub role: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub creation_date_time: SystemTime,
    pub update_date_time: SystemTime,
    /// The entities created in this workspace, keyed by `entityId`.
    pub entities: BTreeMap<String, Entity>,
    /// The component types created in this workspace, keyed by
    /// `componentTypeId`. Owned by the workspace for the same reason entities
    /// are: a component type is addressed as
    /// `/workspaces/{workspaceId}/component-types/{componentTypeId}`, so it
    /// cannot outlive the workspace that contains it, and two operations must
    /// not read two copies of one component type.
    pub component_types: BTreeMap<String, ComponentType>,
    /// The scenes created in this workspace, keyed by `sceneId`. Owned by the
    /// workspace for the same reason entities and component types are: a scene
    /// is addressed as `/workspaces/{workspaceId}/scenes/{sceneId}`, so it
    /// cannot outlive the workspace that contains it, and two operations must
    /// not read two copies of one scene.
    pub scenes: BTreeMap<String, Scene>,
    /// The sync jobs created in this workspace, keyed by `syncSource`.
    ///
    /// Owned by the workspace for the same reason its scenes are: a sync job is
    /// addressed as `/workspaces/{workspaceId}/sync-jobs/{syncSource}`, so it
    /// cannot outlive the workspace that contains it. The KEY is the sync
    /// source, because that label is the job's whole identity inside its
    /// workspace — the create, the read and the delete all address a job by it
    /// and nothing else — so one workspace holds at most one job per source.
    pub sync_jobs: BTreeMap<String, SyncJob>,
}

impl Workspace {
    /// The services linked into this workspace — what `GetWorkspace` and
    /// `ListWorkspaces` report as `linkedServices`, documented as "a list of
    /// services that are linked to the workspace".
    ///
    /// DERIVED from the workspace's sync jobs rather than stored beside them:
    /// what links another service into a workspace is a sync job against that
    /// service as its source (the sync role "is used by the SyncJob to read from
    /// the syncSource, and create, update, or delete the corresponding
    /// resources"), so the linked services ARE the sources of the jobs the
    /// workspace holds — and deriving them means a job created or deleted after
    /// a read cannot leave a stored list stale, the same choice
    /// [`has_child_entities`](Self::has_child_entities) makes.
    ///
    /// In sync-source order, because the map is keyed by source: two reads of an
    /// unchanged workspace report the same list.
    pub fn linked_services(&self) -> Vec<String> {
        self.sync_jobs.keys().cloned().collect()
    }

    /// Whether any stored entity names `entity_id` as its parent.
    ///
    /// Derived on every read rather than kept as a flag beside the entity, so a
    /// child created or deleted after the parent cannot leave it stale.
    pub fn has_child_entities(&self, entity_id: &str) -> bool {
        self.entities
            .values()
            .any(|entity| entity.parent_entity_id == entity_id)
    }

    /// The ids of the entities whose parent is `entity_id`, in id order.
    pub fn children_of(&self, entity_id: &str) -> Vec<String> {
        self.entities
            .values()
            .filter(|entity| entity.parent_entity_id == entity_id)
            .map(|entity| entity.entity_id.clone())
            .collect()
    }

    /// Everything stored in this workspace that still REFERENCES the component
    /// type `component_type_id`, each as a short description of where the
    /// reference lives, in a stable (id-ordered) order.
    ///
    /// A stored reference to a component type is one of:
    ///
    /// * an entity's component, through `ComponentRequest::componentTypeId` —
    ///   the component IS an instance of the type, so the type's definitions are
    ///   what give the component's properties their meaning;
    /// * another component type that `extendsFrom` it — the child inherits its
    ///   property definitions;
    /// * another component type that names it in its `compositeComponentTypes`
    ///   — the composite type is built out of it.
    ///
    /// A `RELATIONSHIP` property definition's `targetComponentTypeId` is NOT
    /// counted: it is a declared target no writer in this service requires to
    /// resolve (a create stores it without checking, and it may name one of the
    /// service's built-in types, of which this emulator holds no registry), so
    /// counting it would refuse deletes the real service allows.
    ///
    /// Derived on every call rather than kept as a count beside the component
    /// type, so a reference created or dropped afterwards cannot leave it stale
    /// — the same choice [`has_child_entities`](Self::has_child_entities) makes.
    pub fn component_type_references(&self, component_type_id: &str) -> Vec<String> {
        let mut references = Vec::new();
        for entity in self.entities.values() {
            for component in entity.components.values() {
                if component.component_type_id.as_deref() == Some(component_type_id) {
                    references.push(format!(
                        "entity {} component {}",
                        entity.entity_id, component.component_name
                    ));
                }
            }
        }
        for component_type in self.component_types.values() {
            // A component type referencing ITSELF is not something that keeps
            // it alive: the reference goes away with the record that carries it.
            if component_type.component_type_id == component_type_id {
                continue;
            }
            if component_type
                .extends_from
                .iter()
                .any(|parent| parent == component_type_id)
            {
                references.push(format!(
                    "componentType {} extendsFrom",
                    component_type.component_type_id
                ));
            }
            for (composite_name, composite) in &component_type.composite_component_types {
                if composite.component_type_id.as_deref() == Some(component_type_id) {
                    references.push(format!(
                        "componentType {} compositeComponentTypes.{composite_name}",
                        component_type.component_type_id
                    ));
                }
            }
        }
        references
    }

    /// Whether a declared reference to the component type `component_type_id`
    /// RESOLVES in this workspace — whether the service holds the component
    /// type the reference names.
    ///
    /// A reference resolves when this workspace stores a component type under
    /// the id (a component type is addressed inside its workspace, so a
    /// reference never reaches across one), or when the id is one of the
    /// service's own built-in types — see
    /// [`BUILT_IN_COMPONENT_TYPE_ID_PREFIX`], of which this emulator holds no
    /// registry.
    pub fn resolves_component_type(&self, component_type_id: &str) -> bool {
        self.component_types.contains_key(component_type_id)
            || built_in_component_type_id(component_type_id)
    }

    /// The transitive `extendsFrom` closure of `seeds`, in ONE walk: the stored
    /// component types it reaches (breadth-first, nearest first, each at most
    /// once), every id it reached whether stored or not, and whether any of
    /// those ids is NOT stored in this workspace — one of the service's
    /// built-in types, whose own definitions this emulator does not hold.
    ///
    /// The single walk over `extendsFrom` every reader of inherited structure
    /// shares, so an inheritance chain is resolved ONE way:
    /// [`component_type_chain`](Self::component_type_chain) takes the component
    /// types it reached,
    /// [`ancestor_component_type_ids`](Self::ancestor_component_type_ids) their
    /// ids.
    ///
    /// TERMINATION is structural, the same way [`subtree_of`](Self::subtree_of)
    /// gets it: an id is expanded only the first time it is seen and the ids
    /// come from this workspace's own component-type map, so the walk visits at
    /// most one id per stored component type plus the declared parents however
    /// the stored `extendsFrom` values point — a self-extending component type
    /// or an already-stored cycle cannot make it loop.
    fn component_type_closure(
        &self,
        seeds: impl IntoIterator<Item = String>,
    ) -> (Vec<&ComponentType>, BTreeSet<String>, bool) {
        let mut reached: BTreeSet<String> = BTreeSet::new();
        let mut queue: VecDeque<String> = seeds.into_iter().collect();
        let mut chain = Vec::new();
        let mut holds_unstored = false;
        while let Some(ancestor) = queue.pop_front() {
            if !reached.insert(ancestor.clone()) {
                continue;
            }
            match self.component_types.get(&ancestor) {
                Some(stored) => {
                    chain.push(stored);
                    queue.extend(stored.extends_from.iter().cloned());
                }
                None => holds_unstored = true,
            }
        }
        (chain, reached, holds_unstored)
    }

    /// Every component type `extends_from` reaches — the ids it names plus,
    /// transitively, the ids those stored component types themselves extend —
    /// and whether any of them is NOT stored in this workspace, i.e. is one of
    /// the service's built-in types, whose own definitions this emulator does
    /// not hold.
    pub fn ancestor_component_type_ids(&self, extends_from: &[String]) -> (BTreeSet<String>, bool) {
        let (_, reached, holds_unstored) =
            self.component_type_closure(extends_from.iter().cloned());
        (reached, holds_unstored)
    }

    /// The property names a component type extending `extends_from` INHERITS —
    /// the own definitions of every component type the inheritance chain
    /// reaches — and whether some ancestor's definitions are unknown to this
    /// emulator (a built-in type), in which case a name the set does not carry
    /// cannot be called undefined.
    pub fn inherited_property_names(&self, extends_from: &[String]) -> (BTreeSet<String>, bool) {
        let (ancestors, holds_unstored) = self.ancestor_component_type_ids(extends_from);
        let mut names: BTreeSet<String> = BTreeSet::new();
        for ancestor in &ancestors {
            if let Some(stored) = self.component_types.get(ancestor) {
                names.extend(stored.property_definitions.keys().cloned());
            }
        }
        (names, holds_unstored)
    }

    /// Every id that some stored entity names as its parent, in ONE pass.
    ///
    /// A listing needs `hasChildEntities` for each entity it reports; asking
    /// [`has_child_entities`](Self::has_child_entities) per reported entity would
    /// make the page cost quadratic in the workspace's size, so a list builds
    /// this set once and looks each id up in it.
    pub fn parent_ids(&self) -> std::collections::BTreeSet<&str> {
        self.entities
            .values()
            .map(|entity| entity.parent_entity_id.as_str())
            .collect()
    }

    /// `entity_id` plus every entity below it, parents before children.
    ///
    /// TERMINATION is structural, not assumed: an id is expanded only the first
    /// time it is seen, and the ids come from this workspace's own entity map,
    /// so the walk visits at most `entities.len() + 1` of them however the
    /// stored `parent_entity_id` values point — a self-parenting entity or a
    /// cycle cannot make it loop or grow without bound. The explicit cap below
    /// makes that bound local to this function rather than a property a reader
    /// has to derive.
    pub fn subtree_of(&self, entity_id: &str) -> Vec<String> {
        let cap = self.entities.len() + 1;
        let mut seen: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::from([entity_id.to_string()]);
        let mut queue: std::collections::VecDeque<String> =
            std::collections::VecDeque::from([entity_id.to_string()]);
        let mut ordered = Vec::new();
        while let Some(parent) = queue.pop_front() {
            if ordered.len() >= cap {
                break;
            }
            ordered.push(parent.clone());
            for child in self.children_of(&parent) {
                if seen.insert(child.clone()) {
                    queue.push_back(child);
                }
            }
        }
        ordered
    }

    /// The component type `component_type_id` and every component type it
    /// extends, nearest first.
    ///
    /// `extendsFrom` is where a property definition a component type does not
    /// declare itself comes from, so a reader that has to resolve one walks this
    /// chain rather than a single map. The walk is
    /// [`component_type_closure`](Self::component_type_closure)'s, seeded with
    /// the component type itself; a parent this workspace does not hold (a
    /// built-in type, of which this emulator holds no registry) contributes
    /// nothing rather than failing the walk.
    pub fn component_type_chain(&self, component_type_id: &str) -> Vec<&ComponentType> {
        let (chain, _, _) = self.component_type_closure([component_type_id.to_string()]);
        chain
    }

    /// Every component type an instance of `component_type_id` may name: the id
    /// itself plus every stored component type that extends it, directly or
    /// through another.
    ///
    /// The scope of a query addressed to a component TYPE, resolved in ONE pass
    /// over this workspace's component types (their `extendsFrom` edges,
    /// reversed) plus a walk of the ids those edges reach, so a scope is
    /// resolved once per request rather than once per component tested against
    /// it. Empty when this workspace holds no such component type, which is
    /// refused before a scope is wanted.
    ///
    /// TERMINATION is structural, as in
    /// [`component_type_closure`](Self::component_type_closure): an id is
    /// expanded only the first time it is seen and every id comes from this
    /// workspace's own component-type map.
    pub fn component_type_instance_ids(&self, component_type_id: &str) -> BTreeSet<&str> {
        let Some(named) = self.component_types.get(component_type_id) else {
            return BTreeSet::new();
        };
        let mut extenders: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for component_type in self.component_types.values() {
            for parent in &component_type.extends_from {
                extenders
                    .entry(parent.as_str())
                    .or_default()
                    .push(component_type.component_type_id.as_str());
            }
        }
        let mut reached: BTreeSet<&str> = BTreeSet::new();
        let mut queue: VecDeque<&str> = VecDeque::from([named.component_type_id.as_str()]);
        while let Some(id) = queue.pop_front() {
            if !reached.insert(id) {
                continue;
            }
            if let Some(children) = extenders.get(id) {
                queue.extend(children.iter().copied());
            }
        }
        reached
    }

    /// The definition that gives one property of a component its meaning: the
    /// component's OWN definition where it declares one, else the definition it
    /// inherits from the component type the component instantiates (or from a
    /// type that type extends).
    ///
    /// A component may carry a property its entity's create never restated —
    /// naming the component's `componentTypeId` is what gives the component that
    /// type's properties — so a reader of a property's definition, its default
    /// value or its data type resolves it through here rather than off the
    /// component alone.
    pub fn effective_property_definition<'a>(
        &'a self,
        component_type_id: Option<&str>,
        properties: &'a BTreeMap<String, PropertyRecord>,
        property_name: &str,
    ) -> Option<&'a crate::ops::types::PropertyDefinitionRequest> {
        if let Some(definition) = properties
            .get(property_name)
            .and_then(|property| property.definition.as_ref())
        {
            return Some(definition);
        }
        self.component_type_chain(component_type_id?)
            .into_iter()
            .find_map(|component_type| component_type.property_definitions.get(property_name))
    }

    /// The property group `group_name` as it applies to a component: the group
    /// the component itself declares, else the one it inherits from the
    /// component type it instantiates — the same resolution
    /// [`effective_property_definition`](Self::effective_property_definition)
    /// makes for a definition.
    pub fn effective_property_group<'a>(
        &'a self,
        component_type_id: Option<&str>,
        property_groups: &'a BTreeMap<String, PropertyGroupRecord>,
        group_name: &str,
    ) -> Option<&'a PropertyGroupRecord> {
        if let Some(group) = property_groups.get(group_name) {
            return Some(group);
        }
        self.component_type_chain(component_type_id?)
            .into_iter()
            .find_map(|component_type| component_type.property_groups.get(group_name))
    }
}

/// One stored scene.
///
/// A scene has no lifecycle `state` member anywhere in its API — `CreateScene`
/// answers only the ARN and the creation instant, and `GetScene` reports no
/// status — so a scene is READY as soon as it is created and there is no
/// transition for the clock to owe. Its only reported failure channel is
/// `GetScene`'s `error` (`SceneError`), whose single modelled code
/// (`MATTERPORT_ERROR`) belongs to a Matterport import this emulator never
/// performs, so a scene it stores carries none.
#[derive(Debug, Clone)]
pub struct Scene {
    pub scene_id: String,
    pub arn: String,
    pub workspace_id: String,
    /// The `s3://…` path of the scene's content definition file. Required on
    /// both the create and the read, so it is stored, never derived.
    pub content_location: String,
    /// The scene's description, absent when the create named none:
    /// `description` is optional on `CreateSceneRequest` AND on
    /// `GetSceneResponse`, so unlike a workspace's it is carried as an option
    /// rather than resolved to the empty string.
    pub description: Option<String>,
    /// The capabilities the scene renders itself with, in the order the create
    /// gave them: `SceneCapabilities` is a LIST, so its order is part of the
    /// value.
    pub capabilities: Vec<String>,
    /// The scene metadata the caller supplied (`sceneMetadata`), which
    /// `GetScene` reports back as the response metadata.
    pub scene_metadata: BTreeMap<String, String>,
    pub tags: BTreeMap<String, String>,
    pub creation_date_time: SystemTime,
    pub update_date_time: SystemTime,
}

/// One stored sync job.
///
/// A sync job is a workspace's standing link to another service: its
/// `syncSource` says which service's resources are synchronised into the
/// workspace, and its `syncRole` is the role the synchronisation would run
/// under. The source is the job's identity inside the workspace, so it is the
/// key of [`Workspace::sync_jobs`] and is not repeated here.
///
/// Like a [`MetadataTransferJob`], the job is accepted in a non-terminal state
/// and carries the instant its outcome falls due ([`settles_at`](Self::settles_at)),
/// which a reader materialises rather than sleeps for.
#[derive(Debug, Clone)]
pub struct SyncJob {
    pub sync_source: String,
    pub arn: String,
    pub workspace_id: String,
    /// The ARN of the IAM role the create supplied: "this IAM role is used by
    /// the SyncJob to read from the syncSource, and create, update, or delete
    /// the corresponding resources". Required on both the create and the read,
    /// so it is stored, never derived.
    pub sync_role: String,
    /// The tags the create supplied. `CreateSyncJob` accepts a `tags` map, so a
    /// sync job is a taggable resource of this service: its ARN is indexed in
    /// [`ComAmazonawsIottwinmakerAwsiottwinmakerState::arn_index`] and the
    /// tagging operations reach this map through it, exactly as they reach a
    /// scene's.
    pub tags: BTreeMap<String, String>,
    pub creation_date_time: SystemTime,
    /// When the job last changed — the create, until the clock settles its
    /// outcome, which moves it to the instant that outcome fell due.
    pub update_date_time: SystemTime,
    /// The job's `SyncJobState`, as STORED: [`SYNC_JOB_STATE_CREATING`] from the
    /// create until [`settles_at`](Self::settles_at) passes, then the terminal
    /// state [`settle`](Self::settle) writes.
    pub state: String,
    /// Why the job is in a failed state, and `None` while nothing has failed.
    pub error: Option<crate::ops::types::ErrorDetails>,
    /// The instant the transition out of [`SYNC_JOB_STATE_CREATING`] falls due
    /// — the instant of the create plus [`SYNC_JOB_SETTLE_SPAN`]. Stored, never
    /// slept for, so a reader can materialise a transition the clock already
    /// owed without any caller having had to poll.
    pub settles_at: SystemTime,
}

impl SyncJob {
    /// The job a `CreateSyncJob` call accepted at `now`: accepted, nothing
    /// synchronised yet, and its outcome due [`SYNC_JOB_SETTLE_SPAN`] later.
    pub fn accepted(
        sync_source: String,
        arn: String,
        workspace_id: String,
        sync_role: String,
        tags: BTreeMap<String, String>,
        now: SystemTime,
    ) -> Self {
        Self {
            sync_source,
            arn,
            workspace_id,
            sync_role,
            tags,
            creation_date_time: now,
            // Nothing has changed the job yet, so it was last updated when it
            // was created.
            update_date_time: now,
            state: SYNC_JOB_STATE_CREATING.to_string(),
            error: None,
            settles_at: now + SYNC_JOB_SETTLE_SPAN,
        }
    }

    /// Materialise the lifecycle transition the CLOCK has already made due.
    ///
    /// Called by a reader before it reports this job, exactly as
    /// [`MetadataTransferJob::settle`] is: the outcome was owed at
    /// [`settles_at`](Self::settles_at) whether or not anybody called, so
    /// writing it here makes the record say what was already true rather than
    /// changing anything a read caused.
    ///
    /// The outcome is the modelled failure state: nothing was synchronised,
    /// because this emulator synchronises nothing (see [`SYNC_JOB_ERROR_CODE`]),
    /// so the job carries the error that says so instead of an `ACTIVE` nothing
    /// earned. Idempotent, a no-op before the instant falls due, and a no-op on
    /// a job some other operation has already moved out of the non-terminal
    /// state.
    pub fn settle(&mut self, now: SystemTime) {
        if self.state != SYNC_JOB_STATE_CREATING || now < self.settles_at {
            return;
        }
        self.state = SYNC_JOB_STATE_ERROR.to_string();
        self.error = Some(crate::ops::types::ErrorDetails {
            code: Some(SYNC_JOB_ERROR_CODE.to_string()),
            message: Some(SYNC_JOB_ERROR_MESSAGE.to_string()),
        });
        // The change fell due at `settles_at`, not at the instant a caller
        // happened to ask, so that is when the job was last updated: two reads
        // of a settled job report the same instant.
        self.update_date_time = self.settles_at;
    }

    /// The job's modelled `SyncJobStatus`: its stored state and, when it has
    /// failed, why.
    ///
    /// The `error` structure is reported WHOLE, the way every other status of
    /// this service is (see [`status`]): a job that has not failed carries an
    /// `ErrorDetails` with neither a code nor a message, so a client that reads
    /// `status.error` needs no null guard.
    pub fn status(&self) -> crate::ops::types::SyncJobStatus {
        crate::ops::types::SyncJobStatus {
            state: Some(self.state.clone()),
            error: Some(self.error.clone().unwrap_or_default()),
        }
    }
}

/// One stored metadata transfer job.
///
/// The job's `sources` and `destination` are kept as the modelled REQUEST
/// shapes the caller supplied — the same choice [`ComponentType`] makes for a
/// component type's definitions — so a read reports back exactly what was
/// persisted rather than a lossy projection of it.
#[derive(Debug, Clone)]
pub struct MetadataTransferJob {
    pub metadata_transfer_job_id: String,
    pub arn: String,
    /// The job's description, absent when the create named none: `description`
    /// is optional on `CreateMetadataTransferJobRequest` AND on
    /// `GetMetadataTransferJobResponse`, so it is carried as an option rather
    /// than resolved to the empty string, as a [`Scene`]'s is.
    pub description: Option<String>,
    /// The job's sources, in the order the create gave them
    /// (`SourceConfigurations` is a `@length(1..1)` LIST).
    pub sources: Vec<crate::ops::types::SourceConfiguration>,
    pub destination: crate::ops::types::DestinationConfiguration,
    /// The ARN of the role the job runs under
    /// ([`metadata_transfer_job_role`]): the service's own, since the create
    /// accepts no role.
    pub metadata_transfer_job_role: String,
    pub creation_date_time: SystemTime,
    /// When the job last changed — the create, until the clock settles its
    /// outcome, which moves it to the instant that outcome fell due.
    pub update_date_time: SystemTime,
    /// The job's `MetadataTransferJobState`, as STORED:
    /// [`METADATA_TRANSFER_JOB_STATE_VALIDATING`] from the create until
    /// [`settles_at`](Self::settles_at) passes, then the terminal state
    /// [`settle`](Self::settle) writes.
    pub state: String,
    /// Why the job is in a failed state, and `None` while nothing has failed.
    pub error: Option<crate::ops::types::ErrorDetails>,
    /// The instant the transition out of the job's current NON-TERMINAL state
    /// falls due — the instant of the create plus
    /// [`METADATA_TRANSFER_JOB_SETTLE_SPAN`] for the outcome a created job is
    /// owed, or the instant of the cancel plus
    /// [`METADATA_TRANSFER_JOB_CANCEL_SPAN`] once a cancellation has been
    /// accepted ([`begin_cancel`](Self::begin_cancel)). Stored, never slept
    /// for, so a reader can materialise a transition the clock already owed
    /// without any caller having had to poll.
    pub settles_at: SystemTime,
    /// The `MetadataTransferJobState` the transition due at
    /// [`settles_at`](Self::settles_at) puts the job in:
    /// [`METADATA_TRANSFER_JOB_STATE_ERROR`] for the outcome of a job nothing
    /// transferred, or [`METADATA_TRANSFER_JOB_STATE_CANCELLED`] once a
    /// cancellation has been accepted. It is read only while the job is
    /// non-terminal: a job that has settled owes nothing more.
    pub settles_to: String,
}

impl MetadataTransferJob {
    /// The job a `CreateMetadataTransferJob` call accepted at `now`: accepted,
    /// nothing transferred yet, and its outcome due
    /// [`METADATA_TRANSFER_JOB_SETTLE_SPAN`] later.
    pub fn accepted(
        metadata_transfer_job_id: String,
        arn: String,
        description: Option<String>,
        sources: Vec<crate::ops::types::SourceConfiguration>,
        destination: crate::ops::types::DestinationConfiguration,
        now: SystemTime,
    ) -> Self {
        Self {
            metadata_transfer_job_id,
            arn,
            description,
            sources,
            destination,
            metadata_transfer_job_role: metadata_transfer_job_role(),
            creation_date_time: now,
            // Nothing has changed the job yet, so it was last updated when it
            // was created.
            update_date_time: now,
            state: METADATA_TRANSFER_JOB_STATE_VALIDATING.to_string(),
            error: None,
            settles_at: now + METADATA_TRANSFER_JOB_SETTLE_SPAN,
            // The outcome an accepted job is owed: the modelled failure state,
            // because this emulator transfers no metadata.
            settles_to: METADATA_TRANSFER_JOB_STATE_ERROR.to_string(),
        }
    }

    /// Whether this job has finished for good: see
    /// [`metadata_transfer_job_state_is_terminal`]. A job that has not is one
    /// the clock still owes the transition at [`settles_at`](Self::settles_at).
    pub fn is_terminal(&self) -> bool {
        metadata_transfer_job_state_is_terminal(&self.state)
    }

    /// Materialise the lifecycle transition the CLOCK has already made due.
    ///
    /// Called by a reader before it reports this job, exactly as
    /// [`ComponentType::settle`] is: the outcome was owed at
    /// [`settles_at`](Self::settles_at) whether or not anybody called, so
    /// writing it here makes the record say what was already true rather than
    /// changing anything a read caused.
    ///
    /// The outcome of a CREATED job is the modelled failure state: no metadata
    /// was transferred, because this emulator transfers none (see
    /// [`METADATA_TRANSFER_JOB_ERROR_CODE`]), so the job carries the error that
    /// says so instead of a `COMPLETED` nothing earned. The outcome of a job
    /// whose CANCELLATION was accepted is
    /// [`METADATA_TRANSFER_JOB_STATE_CANCELLED`], which carries no error:
    /// nothing failed, a caller asked the transfer to stop. Which of the two is
    /// due is [`settles_to`](Self::settles_to). Idempotent, a no-op before the
    /// instant falls due, and a no-op on a job that has already settled.
    pub fn settle(&mut self, now: SystemTime) {
        if self.is_terminal() || now < self.settles_at {
            return;
        }
        self.state = self.settles_to.clone();
        if self.state == METADATA_TRANSFER_JOB_STATE_ERROR {
            self.error = Some(crate::ops::types::ErrorDetails {
                code: Some(METADATA_TRANSFER_JOB_ERROR_CODE.to_string()),
                message: Some(METADATA_TRANSFER_JOB_ERROR_MESSAGE.to_string()),
            });
        }
        // The change fell due at `settles_at`, not at the instant a caller
        // happened to ask, so that is when the job was last updated: two reads
        // of a settled job report the same instant.
        self.update_date_time = self.settles_at;
    }

    /// Accept a cancellation at `now`: the job leaves whichever non-terminal
    /// state it was in for the non-terminal
    /// [`METADATA_TRANSFER_JOB_STATE_CANCELLING`] — the teardown is in flight —
    /// and the instant the clock owes
    /// [`METADATA_TRANSFER_JOB_STATE_CANCELLED`] is stored beside it, never
    /// slept for. See [`settle`](Self::settle).
    ///
    /// A job that has FINISHED is left exactly as it is, and so is one whose
    /// cancellation the service has ALREADY accepted: the transition the clock
    /// already owed is materialised first, a job that is terminal once it has
    /// has nothing left to cancel, and a job already in
    /// [`METADATA_TRANSFER_JOB_STATE_CANCELLING`] is being torn down under the
    /// instant the clock already owes it — so neither that instant nor
    /// `updateDateTime` moves when the same cancellation arrives twice. Both
    /// rules live here rather than in the caller;
    /// `CancelMetadataTransferJob` tests terminality too, because that is what
    /// it answers the caller's `ConflictException` for.
    pub fn begin_cancel(&mut self, now: SystemTime) {
        self.settle(now);
        if self.is_terminal() || self.state == METADATA_TRANSFER_JOB_STATE_CANCELLING {
            return;
        }
        self.state = METADATA_TRANSFER_JOB_STATE_CANCELLING.to_string();
        self.update_date_time = now;
        self.settles_at = now + METADATA_TRANSFER_JOB_CANCEL_SPAN;
        self.settles_to = METADATA_TRANSFER_JOB_STATE_CANCELLED.to_string();
    }

    /// The job's modelled `MetadataTransferJobStatus`: its stored state and,
    /// when it has failed, why.
    ///
    /// The `error` structure is reported WHOLE, the way every other status of
    /// this service is (see [`status`]): a job that has not failed carries an
    /// `ErrorDetails` with neither a code nor a message, so a client that reads
    /// `status.error` needs no null guard.
    ///
    /// `queuedPosition` is left absent: it is a job's place in the queue of
    /// jobs waiting to run, and this emulator queues no transfer behind
    /// another, so there is no position to report.
    pub fn status(&self) -> crate::ops::types::MetadataTransferJobStatus {
        crate::ops::types::MetadataTransferJobStatus {
            state: Some(self.state.clone()),
            error: Some(self.error.clone().unwrap_or_default()),
            queued_position: None,
        }
    }
}

/// One stored entity.
#[derive(Debug, Clone)]
pub struct Entity {
    pub entity_id: String,
    pub entity_name: String,
    pub arn: String,
    pub workspace_id: String,
    /// The entity's description, the empty string when the create named none.
    ///
    /// A description is a member every entity carries rather than one it may
    /// lack: `Description` admits the empty string (`@length 0..2048`), and a
    /// read reports the member for an entity whose description is empty instead
    /// of omitting it, so a client that reads it back needs no null guard.
    pub description: String,
    /// [`ROOT_PARENT_ENTITY_ID`] when the entity was created without a parent.
    pub parent_entity_id: String,
    /// The entity's components, keyed by component name.
    pub components: BTreeMap<String, ComponentRecord>,
    pub tags: BTreeMap<String, String>,
    pub creation_date_time: SystemTime,
    pub update_date_time: SystemTime,
    /// The entity's `State`: the state the STORED record carries, so every
    /// reader reports the same one and no read has to recompute it. A create
    /// settles the entity in [`STATE_ACTIVE`] (see its docs); the operations a
    /// later slice implements are what write any other value here.
    pub state: String,
}

impl Entity {
    /// The entity's modelled `Status`: its stored state, with the `error`
    /// structure every reported status carries. See [`status`].
    pub fn status(&self) -> crate::ops::types::Status {
        status(&self.state)
    }
}

/// One stored component type.
///
/// A component type's property definitions, functions and composite component
/// types are kept as the modelled REQUEST shapes the caller supplied — the same
/// choice [`ComponentRecord`] makes for an entity's properties — so a read
/// reports back exactly what was persisted rather than a lossy projection of
/// it.
#[derive(Debug, Clone)]
pub struct ComponentType {
    pub component_type_id: String,
    pub arn: String,
    pub workspace_id: String,
    /// The friendly name the create supplied, absent when it named none.
    pub component_type_name: Option<String>,
    /// The description the create supplied, absent when it named none.
    pub description: Option<String>,
    /// "Whether an entity can have more than one component of this type" — the
    /// member's own documentation. Stored resolved: a create that names none
    /// leaves it `false`.
    pub is_singleton: bool,
    /// The parent component types this one extends, in the order the create
    /// gave them.
    pub extends_from: Vec<String>,
    pub property_definitions: BTreeMap<String, crate::ops::types::PropertyDefinitionRequest>,
    pub functions: BTreeMap<String, crate::ops::types::FunctionRequest>,
    pub property_groups: BTreeMap<String, PropertyGroupRecord>,
    pub composite_component_types:
        BTreeMap<String, crate::ops::types::CompositeComponentTypeRequest>,
    pub tags: BTreeMap<String, String>,
    pub creation_date_time: SystemTime,
    pub update_date_time: SystemTime,
    /// The component type's `State`, as STORED: [`STATE_CREATING`] from the
    /// create (or [`STATE_UPDATING`] from an update) until
    /// [`settles_at`](Self::settles_at) passes, then [`STATE_ACTIVE`].
    pub state: String,
    /// The instant the transition out of [`STATE_CREATING`] or
    /// [`STATE_UPDATING`] falls due — the instant of the call that opened the
    /// window plus [`COMPONENT_TYPE_SETTLE_SPAN`]. Stored, never slept for, so
    /// a reader can materialise a transition the clock already owed without any
    /// caller having had to poll.
    pub settles_at: SystemTime,
}

impl ComponentType {
    /// Materialise the lifecycle transition the CLOCK has already made due.
    ///
    /// Called by a reader before it reports this component type: the
    /// `CREATING` -> `ACTIVE` (or `UPDATING` -> `ACTIVE`) transition was owed at
    /// [`settles_at`](Self::settles_at) whether or not anybody called, so
    /// writing it here is making the record say what was already true, not a
    /// change a read caused. Idempotent, and a no-op before the instant falls
    /// due.
    pub fn settle(&mut self, now: SystemTime) {
        let in_flight = self.state == STATE_CREATING || self.state == STATE_UPDATING;
        if in_flight && now >= self.settles_at {
            self.state = STATE_ACTIVE.to_string();
        }
    }

    /// Open the provisioning window an UPDATE owes.
    ///
    /// The update has landed in the store by the time this is called, so the
    /// record's `updateDateTime` moves to the instant of the call that made the
    /// change; its state leaves [`STATE_ACTIVE`] for the non-terminal
    /// [`STATE_UPDATING`] the update answers with, and the instant the clock
    /// owes the return to [`STATE_ACTIVE`] is stored beside it — the same span a
    /// create opens, and never slept for. See [`settle`](Self::settle).
    pub fn begin_update(&mut self, now: SystemTime) {
        self.update_date_time = now;
        self.state = STATE_UPDATING.to_string();
        self.settles_at = now + COMPONENT_TYPE_SETTLE_SPAN;
    }

    /// The component type's modelled `Status`: its stored state plus the
    /// `error` structure every reported status carries. See [`status`].
    pub fn status(&self) -> crate::ops::types::Status {
        status(&self.state)
    }

    /// Whether the component type "has a schema initializer and the schema
    /// initializer has run" (the `isSchemaInitialized` member's own
    /// documentation): it has one when its `functions` carry
    /// [`SCHEMA_INITIALIZER_FUNCTION`], and that initializer has run once the
    /// component type has settled, which is exactly when the stored state is
    /// [`STATE_ACTIVE`]: neither a create nor an update still in flight has
    /// finished running it.
    pub fn is_schema_initialized(&self) -> bool {
        self.functions.contains_key(SCHEMA_INITIALIZER_FUNCTION) && self.state == STATE_ACTIVE
    }

    /// Whether the component type is ABSTRACT — a base type an entity cannot
    /// hold a component of.
    ///
    /// False for every component type this service stores, and derived HERE
    /// rather than at each reader so the value `GetComponentType` reports and
    /// the set a `ListComponentTypes` `isAbstract` filter selects cannot drift
    /// apart. `isAbstract` is not a member of `CreateComponentTypeRequest` and
    /// not derivable from one that is: the API offers no way to declare a
    /// component type abstract, and the service's own abstract base types are
    /// built in, of which this emulator holds no registry. Every component type
    /// it stores was created through this API, so every one of them is concrete.
    ///
    /// Takes `&self` though today's answer does not depend on the record: a
    /// writer that ever persists abstractness has exactly one place to make
    /// every reader of it agree.
    pub fn is_abstract(&self) -> bool {
        false
    }
}

/// Whether a `MetadataTransferJobState` is TERMINAL — a state the job has
/// finished in and never leaves, so there is nothing left for a cancellation to
/// stop.
///
/// The model's `MetadataTransferJobState` enum
/// (`ops::types::METADATA_TRANSFER_JOB_STATE_VALUES`) holds three such values:
/// the transfer ran to the end (`COMPLETED`), it was cancelled (`CANCELLED`),
/// or it failed (`ERROR`). Every other value is a state the job is still
/// moving through.
pub fn metadata_transfer_job_state_is_terminal(state: &str) -> bool {
    matches!(
        state,
        METADATA_TRANSFER_JOB_STATE_COMPLETED
            | METADATA_TRANSFER_JOB_STATE_CANCELLED
            | METADATA_TRANSFER_JOB_STATE_ERROR
    )
}

/// The modelled `Status` of a resource in `state`.
///
/// Both of `Status`'s members are optional in the model, but the service
/// reports the structure WHOLE: `error` is a present `ErrorDetails` — carrying
/// neither a code nor a message while nothing has failed — rather than an
/// absent member, so a client that reads `status.error` without a null guard
/// sees an empty structure instead of nothing. A failed resource is the same
/// structure with its `code`/`message` filled in.
pub fn status(state: &str) -> crate::ops::types::Status {
    crate::ops::types::Status {
        state: Some(state.to_string()),
        error: Some(crate::ops::types::ErrorDetails {
            code: None,
            message: None,
        }),
    }
}

/// The single value the model's `GroupType` enum defines
/// (`ops::types::GROUP_TYPE_VALUES`).
///
/// `groupType` is required on every property-group RESPONSE but optional on the
/// request, so a group stored without one is reported with the only value the
/// model admits rather than with an invented string.
pub const TABULAR_GROUP_TYPE: &str = "TABULAR";

/// Whether storing this property definition would leave a component type that
/// `extends_from` these parents holding a definition no read can report.
///
/// `dataType` is optional on `PropertyDefinitionRequest` but REQUIRED on the
/// `PropertyDefinitionResponse` a read reports (see
/// [`property_definition_response`]), so a definition may omit it only where it
/// refines one it inherits. A component type that extends nothing inherits
/// nothing, so each of its own definitions must carry one, and a writer refuses
/// the request rather than storing a definition a reader would have to drop.
/// Shared by every writer of a definition so a create and an update hold one
/// definition to one rule.
pub fn property_definition_needs_data_type(
    extends_from: &[String],
    definition: &crate::ops::types::PropertyDefinitionRequest,
) -> bool {
    extends_from.is_empty() && definition.data_type.is_none()
}

/// The id prefix the service's own BUILT-IN component types carry.
///
/// IoT TwinMaker publishes the component types it ships under the reserved
/// `com.amazon.` namespace — `com.amazon.iottwinmaker.alarm.basic`,
/// `com.amazon.iotsitewise.connector` and the rest of the user guide's
/// "built-in component types" — while a caller's own ids live outside it. This
/// emulator holds no registry of those types (nothing creates them and
/// `ListComponentTypes` does not report them), so an id inside the namespace is
/// treated as RESOLVING to a built-in instead of being refused: refusing it
/// would reject inheritance the real service accepts, which is the opposite
/// error from the dangling reference this rule exists to catch.
pub const BUILT_IN_COMPONENT_TYPE_ID_PREFIX: &str = "com.amazon.";

/// Whether `component_type_id` names one of the service's built-in component
/// types. See [`BUILT_IN_COMPONENT_TYPE_ID_PREFIX`].
pub fn built_in_component_type_id(component_type_id: &str) -> bool {
    component_type_id.starts_with(BUILT_IN_COMPONENT_TYPE_ID_PREFIX)
}

/// Why `extends_from` cannot be stored as the inheritance of
/// `component_type_id`, as the message a `ValidationException` carries — `None`
/// when it can.
///
/// A component type's declared inheritance has to be DESCRIBABLE: a read
/// reports each property definition's required `dataType` and `isInherited`,
/// which is only answerable when the service holds every component type the
/// chain names, and an id that reaches back to the component type itself is an
/// inheritance cycle no chain can be walked up. Shared by every writer of a
/// component type's `extendsFrom`, so one declaration meets one rule wherever
/// it is written.
pub fn extends_from_violation(
    workspace: &Workspace,
    component_type_id: &str,
    extends_from: &[String],
) -> Option<String> {
    for parent in extends_from {
        // The self-reference check comes FIRST, and so gets its own message
        // whether or not the id is stored: a writer that mints the component
        // type (a create) has not stored it yet, so resolving first would tell
        // such a caller its parent "does not exist" rather than that a
        // component type cannot extend itself.
        if parent == component_type_id {
            return Some(format!(
                "extendsFrom names component type {component_type_id} itself; a component type \
                 cannot extend itself"
            ));
        }
        if !workspace.resolves_component_type(parent) {
            return Some(format!(
                "extendsFrom names component type {parent}, which does not exist in workspace {}",
                workspace.workspace_id
            ));
        }
    }
    let (ancestors, _) = workspace.ancestor_component_type_ids(extends_from);
    if ancestors.contains(component_type_id) {
        return Some(format!(
            "extendsFrom names a component type that already extends \
             {component_type_id}; inheritance cannot form a cycle"
        ));
    }
    None
}

/// Why these `compositeComponentTypes` cannot be stored, as the message a
/// `ValidationException` carries — `None` when they can.
///
/// A composite component type IS the component type its `componentTypeId`
/// refers to, so the service has to hold that one for the composition to mean
/// anything; an entry that names none declares no reference to resolve.
/// Shared by every writer of a component type's `compositeComponentTypes`.
pub fn composite_component_types_violation(
    workspace: &Workspace,
    composite_component_types: &BTreeMap<String, crate::ops::types::CompositeComponentTypeRequest>,
) -> Option<String> {
    for (composite_name, composite) in composite_component_types {
        let Some(referenced) = composite.component_type_id.as_deref() else {
            continue;
        };
        if !workspace.resolves_component_type(referenced) {
            return Some(format!(
                "compositeComponentTypes.{composite_name}.componentTypeId names component type \
                 {referenced}, which does not exist in workspace {}",
                workspace.workspace_id
            ));
        }
    }
    None
}

/// Why `record`'s `propertyGroups` cannot be stored, as the message a
/// `ValidationException` carries — `None` when they can.
///
/// A property group GROUPS properties of the component type, so each name it
/// carries has to be one the component type defines: one of its own
/// definitions, or one it inherits from the component types it extends. The
/// groups and the names are both read off `record` as the write leaves it, so a
/// group may name a property the same request defines and no caller can check
/// one component type's groups against another's definitions. Where the chain
/// reaches a built-in type, this emulator does not hold its definitions, so a
/// name it does not otherwise account for cannot be called undefined and the
/// group stands. Shared by every writer of a component type's `propertyGroups`.
pub fn property_groups_violation(workspace: &Workspace, record: &ComponentType) -> Option<String> {
    let (inherited, inherits_unknown_definitions) =
        workspace.inherited_property_names(&record.extends_from);
    if inherits_unknown_definitions {
        return None;
    }
    for (group_name, group) in &record.property_groups {
        for property_name in &group.property_names {
            if record.property_definitions.contains_key(property_name)
                || inherited.contains(property_name)
            {
                continue;
            }
            return Some(format!(
                "propertyGroups.{group_name}.propertyNames names property {property_name}, which \
                 component type {} does not define",
                record.component_type_id
            ));
        }
    }
    None
}

/// One stored `PropertyDefinitionRequest` as the modelled
/// `PropertyDefinitionResponse` a read reports.
///
/// Shared by every reader of a stored definition — an entity's component
/// properties (`GetEntity`) and a component type's own property definitions
/// (`GetComponentType`) — so both report one definition the same way.
///
/// `None` when the stored definition carries no `dataType`, which the response
/// shape requires: the writers refuse a new definition without one, so this
/// stays fallible rather than inventing a `Type` the model does not define.
/// Nothing imports, finalises or inherits a definition yet: `isImported`,
/// `isFinal` and `isInherited` all describe a definition that came from
/// somewhere else, and every definition stored here was written by the create
/// that declared it.
pub fn property_definition_response(
    definition: &crate::ops::types::PropertyDefinitionRequest,
) -> Option<crate::ops::types::PropertyDefinitionResponse> {
    let Some(data_type) = definition.data_type.clone() else {
        // Logged rather than dropped silently so a regression in a writer that
        // is supposed to refuse a definition without a `dataType` is visible
        // instead of quietly costing the member.
        ::tracing::warn!("a stored property definition carries no dataType; omitting it");
        return None;
    };
    Some(crate::ops::types::PropertyDefinitionResponse {
        data_type,
        is_time_series: definition.is_time_series.unwrap_or(false),
        is_required_in_entity: definition.is_required_in_entity.unwrap_or(false),
        is_external_id: definition.is_external_id.unwrap_or(false),
        is_stored_externally: definition.is_stored_externally.unwrap_or(false),
        is_imported: false,
        is_final: false,
        is_inherited: false,
        default_value: definition.default_value.clone(),
        configuration: definition.configuration.clone(),
        display_name: definition.display_name.clone(),
    })
}

/// The value a read reports for ONE property: the value written to the
/// property, else the DEFAULT its definition declares, and nothing at all when
/// neither exists.
///
/// `defaultValue` is "an object that contains the default value" of the
/// property, so a property nothing has written to still reads as the value its
/// definition gave it, and a write to the property replaces it. Shared by every
/// reader of a property's current value, so one property reads the same way
/// wherever it is reported; `definition` is the one
/// [`Workspace::effective_property_definition`] resolved, which is what makes a
/// default declared only on the component TYPE apply to a component that
/// restates nothing.
pub fn latest_property_value(
    property: Option<&PropertyRecord>,
    definition: Option<&crate::ops::types::PropertyDefinitionRequest>,
) -> Option<crate::ops::types::DataValue> {
    property
        .and_then(|property| property.value.clone())
        .or_else(|| definition.and_then(|definition| definition.default_value.clone()))
}

/// One component of an entity.
///
/// A component's properties and property groups are stored as the modelled
/// request shapes the caller supplied, so `GetEntity` reports back exactly the
/// definition and value that were persisted rather than a lossy projection of
/// them. Inheritance from a component *type* is not resolved: every definition
/// stored here is the entity-local one, which is why every `isInherited` this
/// service reports is `false`.
#[derive(Debug, Clone, Default)]
pub struct ComponentRecord {
    pub component_name: String,
    pub component_type_id: Option<String>,
    pub description: Option<String>,
    /// The component's properties, keyed by property name.
    pub properties: BTreeMap<String, PropertyRecord>,
    /// The component's property groups, keyed by group name.
    pub property_groups: BTreeMap<String, PropertyGroupRecord>,
    /// The composite components created INSIDE this component, keyed by the
    /// part of their `ComponentPath` below this component (`inner`, or
    /// `inner/deeper` for one nested a level further down) — the path with its
    /// first segment, which is this component's own name, removed. Keyed that
    /// way because the path below the component is what identifies a composite
    /// component within it, whatever depth it sits at. See
    /// [`resolve_composite_component_path`].
    pub composite_components: BTreeMap<String, CompositeComponentRecord>,
}

/// One composite component of an entity's component: an instance of the
/// component type that the holding component's own component type declares
/// under this name in its `compositeComponentTypes`.
///
/// Its properties and property groups are stored as the modelled request shapes
/// the caller supplied, the same choice [`ComponentRecord`] makes.
#[derive(Debug, Clone, Default)]
pub struct CompositeComponentRecord {
    /// The whole `ComponentPath` the create addressed this composite component
    /// by, starting at the entity's top-level component. Reported as
    /// `ComponentSummary::componentPath`.
    pub component_path: String,
    /// This composite component's own name: the LAST segment of its path, which
    /// is what `GetEntity` keys `ComponentResponse::compositeComponents` by
    /// (`CompositeComponentResponse`'s key is a `Name`, whose pattern admits no
    /// `/`).
    pub component_name: String,
    /// The component type this composite component is an instance of — the one
    /// the declaration its path resolved to names.
    pub component_type_id: String,
    pub description: Option<String>,
    /// The composite component's properties, keyed by property name.
    pub properties: BTreeMap<String, PropertyRecord>,
    /// The composite component's property groups, keyed by group name.
    pub property_groups: BTreeMap<String, PropertyGroupRecord>,
}

/// The separator between the segments of a `ComponentPath`.
///
/// A composite component is addressed by the path of names that leads to it:
/// the entity's top-level component, then one name per
/// `compositeComponentTypes` declaration walked through. `ComponentPath`'s
/// pattern (`^[a-zA-Z_\-0-9/]+$`) is a `Name`'s (`^[a-zA-Z_\-0-9]+$`) plus
/// `/`, so `/` is what joins those names.
pub const COMPONENT_PATH_SEPARATOR: &str = "/";

/// A `ComponentPath` resolved against an entity's components and its
/// workspace's component types. See [`resolve_composite_component_path`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedComponentPath {
    /// The entity's top-level component the path starts at: its first segment.
    pub component_name: String,
    /// The path BELOW that component — the remaining segments, still joined by
    /// [`COMPONENT_PATH_SEPARATOR`]. This is the key the composite component is
    /// held under in [`ComponentRecord::composite_components`].
    pub composite_path: String,
    /// The composite component's own name: the path's last segment.
    pub composite_name: String,
    /// The component type the declaration the path ends at names: the type the
    /// composite component is an instance of.
    pub component_type_id: String,
}

/// Resolve a `ComponentPath` that addresses a COMPOSITE component of an entity.
///
/// A composite component path is valid when its first segment names one of the
/// entity's components and each following segment names an entry in the
/// `compositeComponentTypes` of the component type reached by the segment
/// before it — which is why resolution needs both the entity's components and
/// the workspace's component types. A path that resolves yields the component it
/// hangs under and the component type it instantiates; one that does not yields
/// the reason, built from the request so the refusal names the member and the
/// segment that failed to resolve.
///
/// Shared by every writer of an entity's composite components, so a create and
/// an update hold one path to one rule.
///
/// Two limits are deliberate, both inherited from how this service stores
/// component types. A path of a single segment names a top-level component and
/// no composite component, so it does not resolve; and resolution walks only
/// the component type's OWN `compositeComponentTypes`, because this service
/// resolves no inheritance (every `isInherited` it reports is `false`) and holds
/// no registry of the built-in component types a real workspace carries — so a
/// path through a component type this workspace does not hold cannot be
/// resolved, and is refused rather than guessed at.
pub fn resolve_composite_component_path(
    components: &BTreeMap<String, ComponentRecord>,
    component_types: &BTreeMap<String, ComponentType>,
    component_path: &str,
) -> Result<ResolvedComponentPath, String> {
    let segments: Vec<&str> = component_path.split(COMPONENT_PATH_SEPARATOR).collect();
    if segments.len() < 2 || segments.iter().any(|segment| segment.is_empty()) {
        return Err(format!(
            "compositeComponents.{component_path} is not the path of a composite component: a \
             component path names one of the entity's components and at least one \
             compositeComponentType declared inside it, separated by \
             '{COMPONENT_PATH_SEPARATOR}'"
        ));
    }
    let component_name = segments[0];
    let Some(component) = components.get(component_name) else {
        return Err(format!(
            "compositeComponents.{component_path} names component {component_name}, which the \
             entity does not have"
        ));
    };
    let Some(mut component_type_id) = component.component_type_id.clone() else {
        return Err(format!(
            "compositeComponents.{component_path} names component {component_name}, which \
             instantiates no componentType and so declares no composite component"
        ));
    };
    for segment in &segments[1..] {
        let Some(parent) = component_types.get(&component_type_id) else {
            return Err(format!(
                "compositeComponents.{component_path}: componentType {component_type_id} does \
                 not exist in this workspace, so {segment} cannot be resolved against it"
            ));
        };
        let Some(declaration) = parent.composite_component_types.get(*segment) else {
            return Err(format!(
                "compositeComponents.{component_path}: componentType {component_type_id} \
                 declares no compositeComponentTypes.{segment}"
            ));
        };
        let Some(declared) = declaration.component_type_id.clone() else {
            return Err(format!(
                "compositeComponents.{component_path}: componentType {component_type_id} \
                 declares compositeComponentTypes.{segment} with no componentTypeId, so it names \
                 no component type to instantiate"
            ));
        };
        component_type_id = declared;
    }
    Ok(ResolvedComponentPath {
        component_name: component_name.to_string(),
        composite_path: segments[1..].join(COMPONENT_PATH_SEPARATOR),
        composite_name: segments[segments.len() - 1].to_string(),
        component_type_id,
    })
}

impl ComponentRecord {
    /// The external id this component carries, if any: the string value of the
    /// property its definition marked `isExternalId`. `ListEntities`'
    /// `externalId` filter matches on this.
    pub fn external_ids(&self) -> impl Iterator<Item = &str> {
        self.properties
            .values()
            .filter_map(PropertyRecord::external_id)
    }
}

/// One property of a component: the definition and value the request gave it.
#[derive(Debug, Clone, Default)]
pub struct PropertyRecord {
    pub definition: Option<crate::ops::types::PropertyDefinitionRequest>,
    pub value: Option<crate::ops::types::DataValue>,
}

impl PropertyRecord {
    /// Whether this property's definition marks it as the external id — the
    /// primary key of an external storage system.
    pub fn is_external_id(&self) -> bool {
        self.definition
            .as_ref()
            .and_then(|definition| definition.is_external_id)
            .unwrap_or(false)
    }

    /// This property's value as an external id: its string value, and only when
    /// its definition marks it as the external id.
    pub fn external_id(&self) -> Option<&str> {
        if !self.is_external_id() {
            return None;
        }
        self.value.as_ref()?.string_value.as_deref()
    }
}

/// One property group of a component or of a component type.
#[derive(Debug, Clone, Default)]
pub struct PropertyGroupRecord {
    pub group_type: Option<String>,
    pub property_names: Vec<String>,
}

impl PropertyGroupRecord {
    /// One modelled `PropertyGroupRequest` as the record the store holds.
    ///
    /// Shared by every writer of a COMPONENT TYPE's property groups
    /// (`CreateComponentType`, `UpdateComponentType`), so one shape has one
    /// conversion. `propertyNames` is stored RESOLVED, so an absent list is an
    /// empty one rather than a missing collection every reader has to guard
    /// against. An entity component's groups are a different modelled shape
    /// (`ComponentPropertyGroupRequest`, which also carries the `updateType`
    /// that says what to do with the group), so `CreateEntity` builds its
    /// records from that shape instead.
    pub fn from_request(request: crate::ops::types::PropertyGroupRequest) -> Self {
        Self {
            group_type: request.group_type,
            property_names: request.property_names.unwrap_or_default(),
        }
    }
}

impl ComAmazonawsIottwinmakerAwsiottwinmakerState {
    /// The account's billable entity count: how many entities this service
    /// currently holds, across every workspace.
    ///
    /// IoT TwinMaker bills the entities an account keeps in its workspaces, so
    /// the count a pricing plan is reported with is a count of the stored
    /// entities — derived on every read, never a counter something bumps, so a
    /// create or a delete cannot leave it stale and reading the plan does not
    /// change it. Saturating rather than panicking on the (unreachable, given
    /// [`MAX_WORKSPACES`] × [`MAX_ENTITIES_PER_WORKSPACE`]) overflow of the
    /// modelled `Long`.
    pub fn billable_entity_count(&self) -> i64 {
        let count: usize = self
            .workspaces
            .values()
            .map(|workspace| workspace.entities.len())
            .sum();
        i64::try_from(count).unwrap_or(i64::MAX)
    }

    /// Set the account's pricing plan to `pricing_mode` with `bundle_names` at
    /// `now`, and hand back the plan that is now in force.
    ///
    /// The change takes effect AT ONCE: the new plan's `effectiveDateTime` is
    /// the instant of the call that set it, the same instant as its
    /// `updateDateTime`. The service publishes no rule for deferring an accepted
    /// change to some later instant, so this emulator defers none — which also
    /// means a change that was accepted but had not taken effect
    /// ([`ComAmazonawsIottwinmakerAwsiottwinmakerState::pending_pricing_plan`])
    /// is SUPERSEDED here rather than left to land later, and after this call
    /// nothing is pending.
    ///
    /// The `UpdateReason` recorded on the new plan says what this change was,
    /// read off the plan it replaces — never off the request alone:
    ///
    /// * [`UPDATE_REASON_OVERWRITTEN`] when it superseded a pending change,
    ///   which is the most specific thing that can be said about it;
    /// * else [`UPDATE_REASON_PRICING_MODE_UPDATE`] when the mode differs from
    ///   the mode in force;
    /// * else [`UPDATE_REASON_PRICING_TIER_UPDATE`], the mode being unchanged,
    ///   so the bundles — and the tier they place the account in — are all the
    ///   change can have moved.
    ///
    /// The two remaining `UpdateReason` values are deliberately NOT produced
    /// here, because neither describes a change a caller asked for:
    /// [`UPDATE_REASON_DEFAULT`] marks the plan of an account nobody has
    /// updated, and `ENTITY_COUNT_UPDATE` a tier the service moved on its own
    /// when the account's entity count crossed a threshold.
    ///
    /// `pricingTier` is left unset. The tier a bundled plan sits in is a
    /// function of AWS's published tiered-bundle thresholds, which this
    /// service's model does not carry; the member is optional precisely so a
    /// plan whose tier is not determined is representable, so the plan is
    /// reported without one rather than with a threshold table this emulator
    /// would have invented.
    pub fn set_pricing_plan(
        &mut self,
        pricing_mode: String,
        bundle_names: Vec<String>,
        now: SystemTime,
    ) -> &PricingPlanRecord {
        let superseded_pending = self.pending_pricing_plan.take().is_some();
        let update_reason = if superseded_pending {
            UPDATE_REASON_OVERWRITTEN
        } else if pricing_mode != self.pricing_plan.pricing_mode {
            UPDATE_REASON_PRICING_MODE_UPDATE
        } else {
            UPDATE_REASON_PRICING_TIER_UPDATE
        };
        self.pricing_plan = PricingPlanRecord {
            pricing_mode,
            update_reason: update_reason.to_string(),
            bundle_names,
            pricing_tier: None,
            effective_date_time: now,
            update_date_time: now,
        };
        &self.pricing_plan
    }

    /// Resolve an `IdOrArn` input to a stored `workspaceId`.
    ///
    /// `GetWorkspace`'s label accepts the workspace id or its ARN (its
    /// `IdOrArn` pattern is the union of both), and both must resolve to the one
    /// stored record.
    pub fn resolve_workspace_id(&self, id_or_arn: &str) -> Option<String> {
        if self.workspaces.contains_key(id_or_arn) {
            return Some(id_or_arn.to_string());
        }
        match self.arn_index.get(id_or_arn) {
            Some(ResourceRef::Workspace { workspace_id }) => Some(workspace_id.clone()),
            _ => None,
        }
    }

    /// The stored tag map of the resource `resource_arn` names, or `None` when
    /// this service holds no resource under that ARN.
    ///
    /// ONE resolution path for every taggable resource: the tags live on the
    /// resource record itself (never in a second map beside it), and the ARN
    /// index says which record an ARN points at, so a workspace, an entity and
    /// a component type are reached the same way and a reader and a writer see
    /// one tag set.
    pub fn tags_for_resource(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        match self.arn_index.get(resource_arn)? {
            ResourceRef::Workspace { workspace_id } => {
                Some(&self.workspaces.get(workspace_id)?.tags)
            }
            ResourceRef::Entity {
                workspace_id,
                entity_id,
            } => Some(
                &self
                    .workspaces
                    .get(workspace_id)?
                    .entities
                    .get(entity_id)?
                    .tags,
            ),
            ResourceRef::ComponentType {
                workspace_id,
                component_type_id,
            } => Some(
                &self
                    .workspaces
                    .get(workspace_id)?
                    .component_types
                    .get(component_type_id)?
                    .tags,
            ),
            ResourceRef::Scene {
                workspace_id,
                scene_id,
            } => Some(&self.workspaces.get(workspace_id)?.scenes.get(scene_id)?.tags),
            ResourceRef::SyncJob {
                workspace_id,
                sync_source,
            } => Some(
                &self
                    .workspaces
                    .get(workspace_id)?
                    .sync_jobs
                    .get(sync_source)?
                    .tags,
            ),
        }
    }

    /// [`tags_for_resource`](Self::tags_for_resource), for a caller that writes.
    pub fn tags_for_resource_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        // Cloned out of the index first: the index and the resource records are
        // both fields of `self`, so holding a borrow of one would forbid the
        // mutable borrow of the other.
        let resource = self.arn_index.get(resource_arn)?.clone();
        match resource {
            ResourceRef::Workspace { workspace_id } => {
                Some(&mut self.workspaces.get_mut(&workspace_id)?.tags)
            }
            ResourceRef::Entity {
                workspace_id,
                entity_id,
            } => Some(
                &mut self
                    .workspaces
                    .get_mut(&workspace_id)?
                    .entities
                    .get_mut(&entity_id)?
                    .tags,
            ),
            ResourceRef::ComponentType {
                workspace_id,
                component_type_id,
            } => Some(
                &mut self
                    .workspaces
                    .get_mut(&workspace_id)?
                    .component_types
                    .get_mut(&component_type_id)?
                    .tags,
            ),
            ResourceRef::Scene {
                workspace_id,
                scene_id,
            } => Some(
                &mut self
                    .workspaces
                    .get_mut(&workspace_id)?
                    .scenes
                    .get_mut(&scene_id)?
                    .tags,
            ),
            ResourceRef::SyncJob {
                workspace_id,
                sync_source,
            } => Some(
                &mut self
                    .workspaces
                    .get_mut(&workspace_id)?
                    .sync_jobs
                    .get_mut(&sync_source)?
                    .tags,
            ),
        }
    }
}

/// The message for a resource no ARN in this service's index names.
///
/// Built from the request, naming the ARN that was looked for — the same
/// construction as [`workspace_not_found_message`], one level up from an id to
/// the ARN the tagging operations address a resource by.
pub fn resource_not_found_message(resource_arn: &str) -> String {
    format!("Resource {resource_arn} not found.")
}

/// The message for an entity the workspace the request named does not hold.
///
/// Built from the request: it names the entity that was looked for and the
/// workspace it was looked for in, which is the construction the service's own
/// `ResourceNotFoundException` for an entity uses.
pub fn entity_not_found_message(workspace_id: &str, entity_id: &str) -> String {
    format!("Entity {entity_id} in workspace {workspace_id} not found.")
}

/// The message for a component type the workspace the request named does not
/// hold. Built from the request, naming what was looked for and where, the same
/// construction as [`entity_not_found_message`] one resource across.
pub fn component_type_not_found_message(workspace_id: &str, component_type_id: &str) -> String {
    format!("ComponentType {component_type_id} in workspace {workspace_id} not found.")
}

/// The message for a scene the workspace the request named does not hold.
/// Built from the request, naming what was looked for and where — the same
/// construction as [`entity_not_found_message`] one resource across.
pub fn scene_not_found_message(workspace_id: &str, scene_id: &str) -> String {
    format!("Scene {scene_id} in workspace {workspace_id} not found.")
}

/// The message for a cancellation of a job that has already finished. Built
/// from the request and the store: it names the job and the terminal state it
/// finished in, which is what makes the conflict actionable for a caller that
/// polled a moment too late.
pub fn metadata_transfer_job_terminal_message(
    metadata_transfer_job_id: &str,
    state: &str,
) -> String {
    format!(
        "MetadataTransferJob {metadata_transfer_job_id} is {state} and can no longer be cancelled."
    )
}

/// The message for a sync job the workspace the request named does not hold.
/// Built from the request, naming the SOURCE that was looked for and the
/// workspace it was looked for in — a sync job has no id of its own, the source
/// being its identity inside the workspace. Same construction as
/// [`scene_not_found_message`] one resource across.
pub fn sync_job_not_found_message(workspace_id: &str, sync_source: &str) -> String {
    format!("SyncJob for sync source {sync_source} in workspace {workspace_id} not found.")
}

/// The message for a workspace this service does not hold: built from the
/// request, naming the workspace that was looked for. Same construction as
/// [`entity_not_found_message`], one level up the resource tree.
pub fn workspace_not_found_message(workspace_id: &str) -> String {
    format!("Workspace {workspace_id} not found.")
}

/// The region to build an ARN in for this request: the region of its SigV4
/// credential scope, or [`DEFAULT_REGION`] when it is unsigned.
///
/// A scope region that is not of the shape the modelled `TwinMakerArn` admits
/// for its region segment (`[a-z0-9-]+`) is replaced by the default, so a
/// caller cannot drive this service into minting an ARN its own model forbids.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a workspace.
pub fn workspace_arn(region: &str, workspace_id: &str) -> String {
    ::engine::emulator_arn(
        "iottwinmaker",
        region,
        &format!("{WORKSPACE_ARN_RESOURCE_PREFIX}{workspace_id}"),
    )
}

/// The resource prefix of a workspace's ARN, shared by the ARN [`workspace_arn`]
/// mints and the id [`workspace_id_in_arn`] reads back out of it, so one
/// definition says what the shape is.
const WORKSPACE_ARN_RESOURCE_PREFIX: &str = "workspace/";

/// The `workspaceId` inside a workspace ARN, or `None` when the value is not
/// one: the inverse of [`workspace_arn`], for a caller holding a stored ARN and
/// comparing it against an id.
///
/// Unlike
/// [`resolve_workspace_id`](ComAmazonawsIottwinmakerAwsiottwinmakerState::resolve_workspace_id),
/// this reads the id out of the ARN rather than resolving it against the stored
/// workspaces, so it still names the workspace of an ARN whose workspace has
/// been deleted. An ARN's resource portion is its sixth colon-separated field.
pub fn workspace_id_in_arn(arn: &str) -> Option<&str> {
    arn.splitn(6, ':')
        .nth(5)?
        .strip_prefix(WORKSPACE_ARN_RESOURCE_PREFIX)
}

/// The ARN of an entity, nested under its workspace's resource path.
pub fn entity_arn(region: &str, workspace_id: &str, entity_id: &str) -> String {
    ::engine::emulator_arn(
        "iottwinmaker",
        region,
        &format!("workspace/{workspace_id}/entity/{entity_id}"),
    )
}

/// The ARN of a component type, nested under its workspace's resource path the
/// way [`entity_arn`] nests an entity's.
pub fn component_type_arn(region: &str, workspace_id: &str, component_type_id: &str) -> String {
    ::engine::emulator_arn(
        "iottwinmaker",
        region,
        &format!("workspace/{workspace_id}/componentType/{component_type_id}"),
    )
}

/// The ARN of a scene, nested under its workspace's resource path the way
/// [`entity_arn`] nests an entity's.
pub fn scene_arn(region: &str, workspace_id: &str, scene_id: &str) -> String {
    ::engine::emulator_arn(
        "iottwinmaker",
        region,
        &format!("workspace/{workspace_id}/scene/{scene_id}"),
    )
}

/// The ARN of a sync job, nested under its workspace's resource path the way
/// [`scene_arn`] nests a scene's. The sync SOURCE takes the place of the id,
/// being the job's identity inside its workspace.
pub fn sync_job_arn(region: &str, workspace_id: &str, sync_source: &str) -> String {
    ::engine::emulator_arn(
        "iottwinmaker",
        region,
        &format!("workspace/{workspace_id}/sync-job/{sync_source}"),
    )
}

/// The ARN of a metadata transfer job. An ACCOUNT-level resource, so — unlike
/// [`scene_arn`] — its resource portion nests under no workspace.
pub fn metadata_transfer_job_arn(region: &str, metadata_transfer_job_id: &str) -> String {
    ::engine::emulator_arn(
        "iottwinmaker",
        region,
        &format!("metadata-transfer-job/{metadata_transfer_job_id}"),
    )
}

/// The ARN of the role a metadata transfer job runs under: the service-linked
/// role at [`METADATA_TRANSFER_JOB_ROLE_PATH`], in this emulator's account.
///
/// IAM is global, so the ARN carries no region — which is also what the
/// modelled `RoleArn` pattern (`^arn:…:iam::[0-9]{12}:role/`) requires.
pub fn metadata_transfer_job_role() -> String {
    ::engine::emulator_arn("iam", "", METADATA_TRANSFER_JOB_ROLE_PATH)
}

/// The message for a metadata transfer job this account does not hold. Built
/// from the request, naming the job that was looked for — the same construction
/// as [`workspace_not_found_message`], one resource across.
pub fn metadata_transfer_job_not_found_message(metadata_transfer_job_id: &str) -> String {
    format!("MetadataTransferJob {metadata_transfer_job_id} not found.")
}

/// A stored instant as the modelled `Timestamp` the wire carries.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// The opaque `nextToken` every paginated operation of this service mints.
///
/// One shape, shared by all of them so a token means the same thing wherever it
/// is read: hex (so the token is opaque to a caller and safe in a JSON body)
/// over an `envelope` naming the LISTING it belongs to, the resume `key` it
/// resumes at, and the `scope` the listing was taken in — the parent resource
/// whose page it is, which makes a token minted for another resource, by another
/// listing, or by hand recognisably not this listing's.
///
/// The parts are separated by newlines, which the ids, ARNs and tag keys this
/// service's listings resume at all exclude; the LAST scope part keeps any
/// newline it carries (see [`decode_page_token`]), so a scope part that is free
/// text — a query statement — goes last.
pub fn encode_page_token(envelope: &str, scope: &[&str], key: &str) -> String {
    let mut body = format!("{envelope}{key}");
    for part in scope {
        body.push('\n');
        body.push_str(part);
    }
    ::hex::encode(body)
}

/// The resume key inside a `nextToken` [`encode_page_token`] minted for this
/// `envelope` and `scope`, or `None` when the token is not one of them.
///
/// The key is split off first and each scope part is compared in turn, so a
/// token whose envelope, arity or scope disagrees is rejected rather than
/// resumed; the final part absorbs the remainder, so the last scope part may
/// itself contain newlines. A token carrying an EMPTY key is rejected too: a
/// listing never mints one, since a page is only resumed at a row that exists.
pub fn decode_page_token(envelope: &str, scope: &[&str], token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let body = decoded.strip_prefix(envelope)?;
    let mut parts = body.splitn(scope.len() + 1, '\n');
    let key = parts.next()?;
    for expected in scope {
        if parts.next()? != *expected {
            return None;
        }
    }
    (!key.is_empty()).then(|| key.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIottwinmakerAwsiottwinmakerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIottwinmakerAwsiottwinmakerState>)
}
