// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-workspaces-workspacesservice service.
//!
//! This module owns the WorkSpace lifecycle: the persisted record, the ordering
//! pagination depends on, and the four transitions `CreateWorkspaces`,
//! `StartWorkspaces`, `StopWorkspaces` and `TerminateWorkspaces` request. The
//! transitions live here rather than in one of the operation files because all
//! four batch operations apply the same rules to the same records.
//!
//! It also owns the WorkSpace BUNDLE store `DescribeWorkspaceBundles` reports:
//! the Amazon-provided catalog every account can create WorkSpaces from, which
//! exists before any caller arrives and so is seeded when the store is built,
//! plus the bundles an account owns.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{
    ComputeType, RootStorage, Tag, UserStorage, Workspace, WorkspaceBundle, WorkspaceProperties,
    WorkspaceRequest, COMPUTE_VALUES,
};

/// How long a lifecycle transition the clock owes stays in its transient state.
///
/// `CreateWorkspaces`, `StartWorkspaces`, `StopWorkspaces` and
/// `TerminateWorkspaces` are documented as asynchronous: each returns while the
/// WorkSpace is still in a transient state (`PENDING`, `STARTING`, `STOPPING`,
/// `TERMINATING`). The span is deliberately non-zero so a caller that describes
/// a WorkSpace straight after the call still observes that transient state, and
/// short enough that a test settles quickly. It is a CHOSEN span, not a duration
/// AWS documents.
pub const TRANSITION_DELAY: Duration = Duration::from_millis(300);

/// The reserved `UserName` that marks a user-decoupled WorkSpace.
///
/// From the model's `WorkspaceRequest$UserName` doc: "The reserved keyword,
/// `[UNDEFINED]`, is used when creating user-decoupled WorkSpaces", and
/// `WorkspaceRequest$WorkspaceName`: "`WorkspaceName` is required if `UserName`
/// is `[UNDEFINED]` for user-decoupled WorkSpaces."
pub const USER_DECOUPLED_USER_NAME: &str = "[UNDEFINED]";

/// The page size `DescribeWorkspaces` uses when the caller sends no `Limit`.
///
/// The model caps `DescribeWorkspacesRequest$Limit` at 25 (`range 1..25`), so a
/// request without one cannot be served a larger page either.
pub const DEFAULT_DESCRIBE_PAGE_SIZE: usize = 25;

/// Per-item error code for a WorkSpace id that names no stored WorkSpace.
///
/// The batch operations report per-item failures inside a 200 response, so the
/// code is a string rather than a modelled error; each of the three below names
/// one of THIS service's own modelled error shapes.
pub const ERROR_CODE_NOT_FOUND: &str = "ResourceNotFoundException";
/// Per-item error code for a WorkSpace whose current state forbids the change.
pub const ERROR_CODE_INVALID_STATE: &str = "InvalidResourceStateException";
/// Per-item error code for a create request that collides with a stored WorkSpace.
pub const ERROR_CODE_ALREADY_EXISTS: &str = "ResourceAlreadyExistsException";
/// Per-item error code for a create request whose member combination is invalid.
pub const ERROR_CODE_INVALID_PARAMETERS: &str = "InvalidParameterValuesException";
/// Per-item error code for a create request the store has no room for.
pub const ERROR_CODE_LIMIT_EXCEEDED: &str = "ResourceLimitExceededException";

/// The most WorkSpaces this emulator stores at once.
///
/// AWS caps WorkSpaces with an adjustable service quota and reports an exceeded
/// cap as `ResourceLimitExceededException` ("Your resource limits have been
/// exceeded"), which `CreateWorkspaces` models. The NUMBER here is this
/// emulator's own bound, not an AWS quota: it keeps an in-memory store that only
/// a caller's terminate ever shrinks from growing without end, and sits far above
/// anything a test or a client script creates.
pub const MAX_STORED_WORKSPACES: usize = 2_000;

/// A per-item batch failure: the wire `ErrorCode` and `ErrorMessage` a
/// `FailedCreateWorkspaceRequest` / `FailedWorkspaceChangeRequest` carries.
///
/// Also the (code, message) pair the bundle store reports a refusal with, so a
/// handler can lift it into the modelled error that code names.
pub type ItemFailure = (&'static str, String);

/// The `Owner` a bundle Amazon Web Services provides carries.
///
/// From `WorkspaceBundle$Owner`: "This is the account identifier of the owner,
/// or `AMAZON` if the bundle is provided by Amazon Web Services", and
/// `DescribeWorkspaceBundlesRequest$Owner`: "To describe the bundles provided by
/// Amazon Web Services, specify `AMAZON`."
pub const AMAZON_BUNDLE_OWNER: &str = "AMAZON";

/// The `WorkspaceBundleState` of a bundle WorkSpaces can be created from (one of
/// `WORKSPACE_BUNDLE_STATE_VALUES` in `ops::types`).
pub const BUNDLE_STATE_AVAILABLE: &str = "AVAILABLE";

/// The `BundleType` of an ordinary bundle, as opposed to the `STANDBY` type a
/// standby WorkSpace uses (one of `BUNDLE_TYPE_VALUES` in `ops::types`).
pub const BUNDLE_TYPE_REGULAR: &str = "REGULAR";

/// The page size `DescribeWorkspaceBundles` reports.
///
/// `DescribeWorkspaceBundlesRequest` models no `Limit` member, so the caller
/// cannot ask for a size and this number is the emulator's OWN choice. It is the
/// 25 the rest of this service's surface bounds a page at: every `Limit` member
/// is `range 1..25` and `BundleIdList` is `length 1..25`.
pub const DESCRIBE_BUNDLE_PAGE_SIZE: usize = 25;

/// The root volume size, in GiB, every bundle in the Amazon-provided catalog
/// carries.
///
/// AWS publishes its bundles with an 80 GB root volume paired with a 10, 50 or
/// 100 GB user volume, which is where this number and
/// [`catalog_user_capacity_gib`]'s three come from.
const CATALOG_ROOT_CAPACITY_GIB: u32 = 80;

/// The most bundles this emulator stores at once.
///
/// AWS caps the bundles an account owns with a service quota and reports an
/// exceeded cap as `ResourceLimitExceededException`, which
/// `CreateWorkspaceBundle` models. The NUMBER is this emulator's own bound on an
/// in-memory store, and sits far above the Amazon-provided catalog plus anything
/// a test or client script creates.
pub const MAX_STORED_BUNDLES: usize = 500;

/// The operational state of a stored WorkSpace.
///
/// Every variant renders to a value of the model's `WorkspaceState` enum (see
/// `WORKSPACE_STATE_VALUES` in `ops::types`); only the states this slice of the
/// lifecycle can reach are modelled here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceLifecycleState {
    /// The WorkSpace is being created.
    Pending,
    /// The WorkSpace is running.
    Available,
    /// The WorkSpace is starting up.
    Starting,
    /// The WorkSpace is being stopped.
    Stopping,
    /// The WorkSpace is stopped.
    Stopped,
    /// The WorkSpace is being deleted.
    Terminating,
    /// The WorkSpace has been deleted; it is dropped from the store.
    Terminated,
}

impl WorkspaceLifecycleState {
    /// The `WorkspaceState` value this state serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Available => "AVAILABLE",
            Self::Starting => "STARTING",
            Self::Stopping => "STOPPING",
            Self::Stopped => "STOPPED",
            Self::Terminating => "TERMINATING",
            Self::Terminated => "TERMINATED",
        }
    }
}

/// A lifecycle transition the clock owes: the state to move to, and when it is due.
#[derive(Debug, Clone, Copy)]
struct PendingTransition {
    to: WorkspaceLifecycleState,
    due: Instant,
}

/// One persisted WorkSpace.
#[derive(Debug, Clone)]
pub struct WorkspaceRecord {
    /// Creation sequence number. The only ordering `DescribeWorkspaces`
    /// paginates over, so a page boundary is stable across calls.
    pub seq: u64,
    pub workspace_id: String,
    pub directory_id: String,
    pub user_name: String,
    pub bundle_id: String,
    pub workspace_name: Option<String>,
    pub computer_name: Option<String>,
    pub ip_address: Option<String>,
    pub ipv6_address: Option<String>,
    pub state: WorkspaceLifecycleState,
    pub subnet_id: Option<String>,
    pub volume_encryption_key: Option<String>,
    pub user_volume_encryption_enabled: Option<bool>,
    pub root_volume_encryption_enabled: Option<bool>,
    pub workspace_properties: Option<WorkspaceProperties>,
    pub tags: Vec<Tag>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub created: SystemTime,
    pending: Option<PendingTransition>,
}

impl WorkspaceRecord {
    /// Enter `transient` now and owe `settled` once [`TRANSITION_DELAY`] elapses.
    fn begin(&mut self, transient: WorkspaceLifecycleState, settled: WorkspaceLifecycleState) {
        self.state = transient;
        self.pending = Some(PendingTransition {
            to: settled,
            due: Instant::now() + TRANSITION_DELAY,
        });
    }

    /// Apply the owed transition if `now` has reached its due instant.
    fn settle(&mut self, now: Instant) {
        let Some(pending) = self.pending else {
            return;
        };
        if now < pending.due {
            return;
        }
        self.state = pending.to;
        self.pending = None;
    }

    /// This record as the modelled `Workspace` structure.
    ///
    /// `SubnetId`, `IpAddress`, `ModificationStates`, `RelatedWorkspaces`,
    /// `DataReplicationSettings` and `StandbyWorkspacesProperties` stay absent:
    /// nothing in this emulator models a directory's subnets, an address
    /// allocation, a modification, a standby WorkSpace or data replication, so
    /// there is no state to report them from.
    pub fn to_workspace(&self) -> Workspace {
        Workspace {
            workspace_id: Some(self.workspace_id.clone()),
            directory_id: Some(self.directory_id.clone()),
            user_name: Some(self.user_name.clone()),
            ip_address: self.ip_address.clone(),
            ipv6_address: self.ipv6_address.clone(),
            state: Some(self.state.as_wire().to_string()),
            bundle_id: Some(self.bundle_id.clone()),
            subnet_id: self.subnet_id.clone(),
            error_message: self.error_message.clone(),
            error_code: self.error_code.clone(),
            computer_name: self.computer_name.clone(),
            volume_encryption_key: self.volume_encryption_key.clone(),
            user_volume_encryption_enabled: self.user_volume_encryption_enabled,
            root_volume_encryption_enabled: self.root_volume_encryption_enabled,
            workspace_name: self.workspace_name.clone(),
            workspace_properties: self.workspace_properties.clone(),
            modification_states: None,
            related_workspaces: None,
            data_replication_settings: None,
            standby_workspaces_properties: None,
        }
    }

    /// The directory slot this WorkSpace occupies.
    ///
    /// A directory user has at most one WorkSpace; the model's
    /// `WorkspaceRequest$UserName` doc states the user name "is not
    /// case-sensitive", so the comparison folds case. A user-decoupled WorkSpace
    /// (`UserName` = `[UNDEFINED]`) is identified by its `WorkspaceName` instead.
    fn occupancy_key(&self) -> String {
        occupancy_key(
            &self.directory_id,
            &self.user_name,
            self.workspace_name.as_deref(),
        )
    }
}

/// The directory slot a create request would occupy. See
/// [`WorkspaceRecord::occupancy_key`].
fn occupancy_key(directory_id: &str, user_name: &str, workspace_name: Option<&str>) -> String {
    if user_name == USER_DECOUPLED_USER_NAME {
        format!("{directory_id}\u{1f}name\u{1f}{}", workspace_name.unwrap_or(""))
    } else {
        format!(
            "{directory_id}\u{1f}user\u{1f}{}",
            user_name.to_lowercase()
        )
    }
}

/// One persisted WorkSpace bundle.
///
/// Volume sizes are held as GiB numbers and rendered into the modelled
/// `RootStorage$Capacity` / `UserStorage$Capacity` strings on the way out, so the
/// store never holds a value it cannot render.
#[derive(Debug, Clone)]
pub struct WorkspaceBundleRecord {
    /// Sequence number: the stable order [`ComAmazonawsWorkspacesWorkspacesserviceState::ordered_bundles`]
    /// reports and pagination resumes after.
    pub seq: u64,
    pub bundle_id: String,
    pub name: String,
    pub description: String,
    pub image_id: String,
    /// One of the model's `Compute` enum values (`COMPUTE_VALUES` in `ops::types`).
    pub compute_name: String,
    pub root_capacity_gib: u32,
    pub user_capacity_gib: u32,
    /// `AMAZON` for a bundle this service provides, otherwise the owning
    /// account id.
    pub owner: String,
    /// One of the model's `WorkspaceBundleState` values.
    pub state: &'static str,
    /// One of the model's `BundleType` values.
    pub bundle_type: &'static str,
    pub created: SystemTime,
    pub last_updated: SystemTime,
}

impl WorkspaceBundleRecord {
    /// This record as the modelled `WorkspaceBundle` structure.
    pub fn to_workspace_bundle(&self) -> WorkspaceBundle {
        WorkspaceBundle {
            bundle_id: Some(self.bundle_id.clone()),
            name: Some(self.name.clone()),
            owner: Some(self.owner.clone()),
            description: Some(self.description.clone()),
            image_id: Some(self.image_id.clone()),
            root_storage: Some(RootStorage {
                capacity: self.root_capacity_gib.to_string(),
            }),
            user_storage: Some(UserStorage {
                capacity: self.user_capacity_gib.to_string(),
            }),
            compute_type: Some(ComputeType {
                name: Some(self.compute_name.clone()),
            }),
            last_updated_time: Some(::aws_smithy_types::DateTime::from(self.last_updated)),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.created)),
            state: Some(self.state.to_string()),
            bundle_type: Some(self.bundle_type.to_string()),
        }
    }
}

/// The `BundleId` / `WorkspaceImageId` fragment that identifies the
/// Amazon-provided bundle of `compute`.
///
/// `BundleId` is `^wsb-[0-9a-z]{8,63}$` and `WorkspaceImageId` is
/// `^wsi-[0-9a-z]{9,63}$`; every `Compute` value is upper-case letters, digits
/// and underscores, so lower-casing it and dropping the underscores leaves the
/// `[0-9a-z]` the patterns allow, and the `amzn` prefix below carries the
/// shortest value (`VALUE`) past the nine characters the image id needs.
fn catalog_slug(compute: &str) -> String {
    format!("amzn{}", compute.to_lowercase().replace('_', ""))
}

/// The user volume size, in GiB, the Amazon-provided bundle of `compute` carries.
///
/// AWS publishes its bundles with the root volume [`CATALOG_ROOT_CAPACITY_GIB`]
/// paired with a 10, 50 or 100 GB user volume. WHICH of those three a given
/// compute type ships with is this emulator's own assignment — a function of the
/// compute type alone, so a bundle reports the same size on every read.
fn catalog_user_capacity_gib(compute: &str) -> u32 {
    match compute {
        "VALUE" => 10,
        "STANDARD" | "PERFORMANCE" => 50,
        _ => 100,
    }
}

/// Per-service state for com-amazonaws-workspaces-workspacesservice. A handler reaches it through [`service_state`].
///
/// The three maps below are one store with two indexes over it, so they are
/// PRIVATE and every insert and removal goes through [`Self::insert_record`] /
/// [`Self::remove_record`]: an index that outlived the record it points at would
/// make a directory slot permanently unavailable. Readers use
/// [`Self::workspace`], [`Self::workspace_count`] and
/// [`Self::ordered_workspaces`].
///
/// The bundle store below is the same shape: one map plus an index by
/// `BundleId`, both written only through [`Self::insert_bundle`]. A freshly
/// built store already holds the Amazon-provided catalog (see [`Self::default`]),
/// because those bundles exist in every account before any caller arrives —
/// `DescribeWorkspaceBundles` reports them, it does not create them.
#[derive(Debug)]
pub struct ComAmazonawsWorkspacesWorkspacesserviceState {
    /// Every stored WorkSpace, keyed by its `WorkspaceId`. Never larger than
    /// [`MAX_STORED_WORKSPACES`].
    workspaces: HashMap<String, WorkspaceRecord>,
    /// Which `WorkspaceId` holds each occupied directory slot (see
    /// [`WorkspaceRecord::occupancy_key`]), so deciding whether a slot is taken
    /// costs one lookup rather than a scan of the whole store under the shared
    /// lock.
    occupancy: HashMap<String, String>,
    /// `WorkspaceId` by creation sequence: the stable order
    /// [`Self::ordered_workspaces`] reports, held sorted so no call has to sort
    /// the store.
    order: BTreeMap<u64, String>,
    /// Monotonic counter behind both the minted `WorkspaceId` and the creation
    /// order [`Self::ordered_workspaces`] reports.
    pub next_workspace_seq: u64,
    /// Every stored bundle by sequence number: the stable order
    /// [`Self::ordered_bundles`] reports and `DescribeWorkspaceBundles`
    /// paginates over. Never larger than [`MAX_STORED_BUNDLES`].
    bundles: BTreeMap<u64, WorkspaceBundleRecord>,
    /// Which sequence number each `BundleId` names, so looking a bundle up by id
    /// costs one lookup rather than a scan.
    bundle_ids: HashMap<String, u64>,
    /// Monotonic counter behind both the minted `BundleId` and the order
    /// [`Self::ordered_bundles`] reports.
    next_bundle_seq: u64,
}

impl Default for ComAmazonawsWorkspacesWorkspacesserviceState {
    /// An empty WorkSpace store that already holds the Amazon-provided bundle
    /// catalog.
    ///
    /// The catalog is seeded HERE, when the engine builds this service's state,
    /// rather than by the first `DescribeWorkspaceBundles` call: those bundles
    /// are AWS's own and are describable in a brand-new account, so reporting
    /// them must not be a write. Its `CreationTime` is the instant the store was
    /// built, which is stable for the life of the emulator.
    fn default() -> Self {
        let mut state = Self {
            workspaces: HashMap::new(),
            occupancy: HashMap::new(),
            order: BTreeMap::new(),
            next_workspace_seq: 0,
            bundles: BTreeMap::new(),
            bundle_ids: HashMap::new(),
            next_bundle_seq: 0,
        };
        state.seed_amazon_bundle_catalog(SystemTime::now());
        state
    }
}

impl ComAmazonawsWorkspacesWorkspacesserviceState {
    /// Materialise every lifecycle transition the clock already owes, and drop
    /// the WorkSpaces whose termination has completed.
    ///
    /// Due at the same instant whether or not anybody called, so a reporting
    /// operation may run it.
    pub fn settle(&mut self) {
        self.settle_at(Instant::now());
    }

    /// [`Self::settle`] against an explicit clock reading.
    ///
    /// Handlers call `settle()`, which reads the real clock; this exists so a
    /// test can state the instant it is asking about instead of sleeping.
    pub fn settle_at(&mut self, now: Instant) {
        for record in self.workspaces.values_mut() {
            record.settle(now);
        }
        let completed: Vec<String> = self
            .workspaces
            .values()
            .filter(|record| record.state == WorkspaceLifecycleState::Terminated)
            .map(|record| record.workspace_id.clone())
            .collect();
        for workspace_id in completed {
            self.remove_record(&workspace_id);
        }
    }

    /// Every stored WorkSpace in creation order.
    pub fn ordered_workspaces(&self) -> Vec<&WorkspaceRecord> {
        self.order
            .values()
            .filter_map(|workspace_id| self.workspaces.get(workspace_id))
            .collect()
    }

    /// How many WorkSpaces are stored.
    pub fn workspace_count(&self) -> usize {
        self.workspaces.len()
    }

    /// The stored WorkSpace `workspace_id` names, if any.
    pub fn workspace(&self, workspace_id: &str) -> Option<&WorkspaceRecord> {
        self.workspaces.get(workspace_id)
    }

    /// Store `record` and point both indexes at it.
    fn insert_record(&mut self, record: WorkspaceRecord) {
        self.occupancy
            .insert(record.occupancy_key(), record.workspace_id.clone());
        self.order.insert(record.seq, record.workspace_id.clone());
        self.workspaces.insert(record.workspace_id.clone(), record);
    }

    /// Drop the record `workspace_id` names and every index entry for it.
    fn remove_record(&mut self, workspace_id: &str) {
        if let Some(record) = self.workspaces.remove(workspace_id) {
            self.occupancy.remove(&record.occupancy_key());
            self.order.remove(&record.seq);
        }
    }

    /// Create one WorkSpace from `request`, or report why it could not be created.
    ///
    /// The returned record is a snapshot of what was persisted, so the caller can
    /// serialize it into `PendingRequests` without holding the map borrow.
    pub fn create_workspace(
        &mut self,
        request: &WorkspaceRequest,
    ) -> Result<WorkspaceRecord, ItemFailure> {
        if request.user_name == USER_DECOUPLED_USER_NAME && request.workspace_name.is_none() {
            return Err((
                ERROR_CODE_INVALID_PARAMETERS,
                format!(
                    "WorkspaceName is required when UserName is {USER_DECOUPLED_USER_NAME}."
                ),
            ));
        }

        let wanted = occupancy_key(
            &request.directory_id,
            &request.user_name,
            request.workspace_name.as_deref(),
        );
        if let Some(existing) = self
            .occupancy
            .get(&wanted)
            .and_then(|workspace_id| self.workspaces.get(workspace_id))
        {
            return Err((
                ERROR_CODE_ALREADY_EXISTS,
                format!(
                    "WorkSpace {} already exists for user {} in directory {}.",
                    existing.workspace_id, existing.user_name, existing.directory_id
                ),
            ));
        }

        // The store is bounded: refuse rather than grow without end.
        if self.workspaces.len() >= MAX_STORED_WORKSPACES {
            return Err((
                ERROR_CODE_LIMIT_EXCEEDED,
                format!(
                    "Your resource limits have been exceeded: at most {MAX_STORED_WORKSPACES} WorkSpaces can exist."
                ),
            ));
        }

        let seq = self.next_seq();
        let workspace_id = format!("ws-{seq:08x}");
        let mut record = WorkspaceRecord {
            seq,
            // The OS-visible name AWS assigns; its documented format "varies",
            // so it is derived from the minted id to stay stable across reads.
            computer_name: Some(format!("WSAMZN-{seq:08X}")),
            workspace_id: workspace_id.clone(),
            directory_id: request.directory_id.clone(),
            user_name: request.user_name.clone(),
            bundle_id: request.bundle_id.clone(),
            workspace_name: request.workspace_name.clone(),
            ip_address: None,
            ipv6_address: request.ipv6_address.clone(),
            state: WorkspaceLifecycleState::Pending,
            subnet_id: None,
            volume_encryption_key: request.volume_encryption_key.clone(),
            user_volume_encryption_enabled: request.user_volume_encryption_enabled,
            root_volume_encryption_enabled: request.root_volume_encryption_enabled,
            workspace_properties: request.workspace_properties.clone(),
            tags: request.tags.clone().unwrap_or_default(),
            error_code: None,
            error_message: None,
            created: SystemTime::now(),
            pending: None,
        };
        // Creation is asynchronous: PENDING now, AVAILABLE once the clock owes it.
        record.begin(
            WorkspaceLifecycleState::Pending,
            WorkspaceLifecycleState::Available,
        );
        self.insert_record(record.clone());
        Ok(record)
    }

    /// Start the WorkSpace named by `workspace_id`.
    ///
    /// `StartWorkspaces` documents that a WorkSpace can only be started from
    /// `STOPPED`; a start already under way is accepted as a no-op.
    pub fn request_start(&mut self, workspace_id: &str) -> Result<(), ItemFailure> {
        let record = self.record_mut(workspace_id)?;
        match record.state {
            WorkspaceLifecycleState::Stopped => {
                record.begin(
                    WorkspaceLifecycleState::Starting,
                    WorkspaceLifecycleState::Available,
                );
                Ok(())
            }
            WorkspaceLifecycleState::Starting => Ok(()),
            other => Err(invalid_state(workspace_id, other, "started")),
        }
    }

    /// Stop the WorkSpace named by `workspace_id`.
    ///
    /// A running WorkSpace stops; one already stopped or stopping is accepted as
    /// a no-op. Any other state (a create or start still under way, a
    /// termination) cannot be stopped.
    pub fn request_stop(&mut self, workspace_id: &str) -> Result<(), ItemFailure> {
        let record = self.record_mut(workspace_id)?;
        match record.state {
            WorkspaceLifecycleState::Available => {
                record.begin(
                    WorkspaceLifecycleState::Stopping,
                    WorkspaceLifecycleState::Stopped,
                );
                Ok(())
            }
            WorkspaceLifecycleState::Stopping | WorkspaceLifecycleState::Stopped => Ok(()),
            other => Err(invalid_state(workspace_id, other, "stopped")),
        }
    }

    /// Terminate the WorkSpace named by `workspace_id`.
    ///
    /// Terminating is allowed from every state this emulator stores; the record
    /// enters `TERMINATING` and [`Self::settle`] drops it once the clock owes it.
    pub fn request_terminate(&mut self, workspace_id: &str) -> Result<(), ItemFailure> {
        let record = self.record_mut(workspace_id)?;
        if record.state != WorkspaceLifecycleState::Terminating {
            record.begin(
                WorkspaceLifecycleState::Terminating,
                WorkspaceLifecycleState::Terminated,
            );
        }
        Ok(())
    }

    fn record_mut(&mut self, workspace_id: &str) -> Result<&mut WorkspaceRecord, ItemFailure> {
        self.workspaces
            .get_mut(workspace_id)
            .ok_or_else(|| not_found(workspace_id))
    }

    /// The next creation sequence number.
    ///
    /// Strictly increasing and never reset, and the minted `WorkspaceId` is a
    /// function of it alone, so no sequence number — and no id — can repeat
    /// within one run of the emulator.
    fn next_seq(&mut self) -> u64 {
        self.next_workspace_seq += 1;
        self.next_workspace_seq
    }

    /// Every stored bundle in the order [`Self::ordered_bundles`] documents: the
    /// Amazon-provided catalog first, then the bundles this account created, in
    /// creation order.
    pub fn ordered_bundles(&self) -> Vec<&WorkspaceBundleRecord> {
        self.bundles.values().collect()
    }

    /// The stored bundle `bundle_id` names, if any.
    pub fn bundle(&self, bundle_id: &str) -> Option<&WorkspaceBundleRecord> {
        self.bundle_ids
            .get(bundle_id)
            .and_then(|seq| self.bundles.get(seq))
    }

    /// How many bundles are stored, the Amazon-provided catalog included.
    pub fn bundle_count(&self) -> usize {
        self.bundles.len()
    }

    /// Create one bundle owned by this account, or report why it could not be
    /// created.
    ///
    /// The writer side of the bundle store: `CreateWorkspaceBundle` is the
    /// operation that calls it (it is still a scaffold stub, so this service's
    /// own test suite is its only caller today). The returned record is a
    /// snapshot of what was persisted.
    pub fn create_bundle(
        &mut self,
        request: &NewBundle,
    ) -> Result<WorkspaceBundleRecord, ItemFailure> {
        let account = ::engine::emulator_account_id();
        if let Some(existing) = self
            .bundles
            .values()
            .find(|record| record.owner == account && record.name == request.name)
        {
            return Err((
                ERROR_CODE_ALREADY_EXISTS,
                format!(
                    "Bundle {} already exists with the name {}.",
                    existing.bundle_id, existing.name
                ),
            ));
        }
        if self.bundles.len() >= MAX_STORED_BUNDLES {
            return Err((
                ERROR_CODE_LIMIT_EXCEEDED,
                format!(
                    "Your resource limits have been exceeded: at most {MAX_STORED_BUNDLES} bundles can exist."
                ),
            ));
        }

        let seq = self.next_bundle_seq();
        let now = SystemTime::now();
        let record = WorkspaceBundleRecord {
            seq,
            bundle_id: format!("wsb-{seq:08x}"),
            name: request.name.clone(),
            description: request.description.clone(),
            image_id: request.image_id.clone(),
            compute_name: request.compute_name.clone(),
            root_capacity_gib: request.root_capacity_gib,
            user_capacity_gib: request.user_capacity_gib,
            owner: account.to_string(),
            state: BUNDLE_STATE_AVAILABLE,
            bundle_type: BUNDLE_TYPE_REGULAR,
            created: now,
            last_updated: now,
        };
        self.insert_bundle(record.clone());
        Ok(record)
    }

    /// Seed the Amazon-provided bundle catalog, one bundle per compute type.
    ///
    /// The catalog's MEMBERSHIP comes from the model: `COMPUTE_VALUES` in
    /// `ops::types` is the `Compute` enum, and AWS offers a bundle for each of
    /// those compute types. The volume sizes come from
    /// [`CATALOG_ROOT_CAPACITY_GIB`] and [`catalog_user_capacity_gib`], and the
    /// ids are a function of the compute type (see [`catalog_slug`]) so they are
    /// the same on every read and in every run.
    fn seed_amazon_bundle_catalog(&mut self, created: SystemTime) {
        for compute in COMPUTE_VALUES {
            let slug = catalog_slug(compute);
            let user_capacity_gib = catalog_user_capacity_gib(compute);
            let seq = self.next_bundle_seq();
            self.insert_bundle(WorkspaceBundleRecord {
                seq,
                bundle_id: format!("wsb-{slug}"),
                name: format!("Amazon {compute}"),
                description: format!(
                    "Amazon-provided WorkSpaces bundle: {compute} compute, a {CATALOG_ROOT_CAPACITY_GIB} GiB root volume and a {user_capacity_gib} GiB user volume."
                ),
                image_id: format!("wsi-{slug}"),
                compute_name: (*compute).to_string(),
                root_capacity_gib: CATALOG_ROOT_CAPACITY_GIB,
                user_capacity_gib,
                owner: AMAZON_BUNDLE_OWNER.to_string(),
                state: BUNDLE_STATE_AVAILABLE,
                bundle_type: BUNDLE_TYPE_REGULAR,
                created,
                last_updated: created,
            });
        }
    }

    /// Store `record` and point the `BundleId` index at it.
    fn insert_bundle(&mut self, record: WorkspaceBundleRecord) {
        self.bundle_ids.insert(record.bundle_id.clone(), record.seq);
        self.bundles.insert(record.seq, record);
    }

    /// The next bundle sequence number. Strictly increasing and never reset, as
    /// [`Self::next_seq`] is for WorkSpaces.
    fn next_bundle_seq(&mut self) -> u64 {
        self.next_bundle_seq += 1;
        self.next_bundle_seq
    }
}

/// What a bundle this account owns is created from: the members
/// `CreateWorkspaceBundle` takes, with the modelled `Capacity` strings already
/// parsed into the GiB numbers the store holds.
#[derive(Debug, Clone)]
pub struct NewBundle {
    pub name: String,
    pub description: String,
    pub image_id: String,
    pub compute_name: String,
    pub root_capacity_gib: u32,
    pub user_capacity_gib: u32,
}

/// The per-item failure for a WorkSpace id that names no stored WorkSpace.
fn not_found(workspace_id: &str) -> ItemFailure {
    (
        ERROR_CODE_NOT_FOUND,
        format!("The WorkSpace {workspace_id} could not be found."),
    )
}

/// The per-item failure for a WorkSpace whose state forbids `action`.
fn invalid_state(
    workspace_id: &str,
    state: WorkspaceLifecycleState,
    action: &str,
) -> ItemFailure {
    (
        ERROR_CODE_INVALID_STATE,
        format!(
            "The WorkSpace {workspace_id} is in state {} and cannot be {action}.",
            state.as_wire()
        ),
    )
}

/// Encode "resume after this creation sequence number" as a `NextToken`.
///
/// The token names the LAST WorkSpace a page reported, by the creation sequence
/// that record carries, rather than an offset into the list: a WorkSpace
/// terminated between two pages then shifts nothing, so a paginating client
/// cannot skip an item. The encoding is a fixed-width lowercase-hex rendering of
/// that sequence number — deterministic, so resuming needs no stored token, and
/// 16 characters long, inside the model's `PaginationToken` length of 1..2048.
pub fn encode_page_token(after_seq: u64) -> String {
    format!("{after_seq:016x}")
}

/// Decode a `NextToken` produced by [`encode_page_token`], or `None` when the
/// token was not produced by it.
pub fn decode_page_token(token: &str) -> Option<u64> {
    // Fixed width is the load-bearing check; `from_str_radix` rejects anything
    // that is not hex, and the lowercase class rejects an upper-case spelling
    // this service never mints.
    if token.len() != 16 || !token.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    u64::from_str_radix(token, 16).ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsWorkspacesWorkspacesserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsWorkspacesWorkspacesserviceState>)
}
