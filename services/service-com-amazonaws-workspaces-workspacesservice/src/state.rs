// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-workspaces-workspacesservice service.
//!
//! This module owns the WorkSpace lifecycle: the persisted record, the ordering
//! pagination depends on, and the four transitions `CreateWorkspaces`,
//! `StartWorkspaces`, `StopWorkspaces` and `TerminateWorkspaces` request. The
//! transitions live here rather than in one of the operation files because all
//! four batch operations apply the same rules to the same records.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{Tag, Workspace, WorkspaceProperties, WorkspaceRequest};

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
pub type ItemFailure = (&'static str, String);

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

/// Per-service state for com-amazonaws-workspaces-workspacesservice. A handler reaches it through [`service_state`].
///
/// The three maps below are one store with two indexes over it, so they are
/// PRIVATE and every insert and removal goes through [`Self::insert_record`] /
/// [`Self::remove_record`]: an index that outlived the record it points at would
/// make a directory slot permanently unavailable. Readers use
/// [`Self::workspace`], [`Self::workspace_count`] and
/// [`Self::ordered_workspaces`].
#[derive(Debug, Default)]
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
