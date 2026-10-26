// SPDX-License-Identifier: CC0-1.0

//! The STACK SETS this account holds, and the stack INSTANCES each one carries.
//!
//! A stack set is a group of stacks managed together from one template, deployed as one
//! stack INSTANCE per target account and Region pair. `ListStackInstances` "returns summary
//! information about stack instances that are associated with the specified StackSet", so
//! everything that read reports lives on the records here — the stack set the request
//! resolves and the instances that stack set holds — and the read computes its answer from
//! them rather than from the request.
//!
//! ## One namespace per Region
//!
//! A stack set belongs to the Region the administrator call that created it was signed for
//! ([`StoredStackSet::region`]), the way every other record of this service does, and the
//! store is read THROUGH that Region:
//! [`crate::state::ComAmazonawsCloudformationCloudformationState::resolve_stack_set`] takes
//! that Region, so no read resolves a stack set another Region holds. The Region on an
//! INSTANCE is a different thing — the Region the instance's own stack is deployed to, a
//! target the deploying request chooses, which may be any Region and is what
//! `StackInstanceRegion` filters on.
//!
//! ## The operations performed on a stack set
//!
//! A stack set also carries the OPERATIONS performed on it ([`StoredStackSetOperation`]):
//! `ListStackSetOperations` "returns summary information about operations performed on a
//! StackSet", so each one is a record of its own on the stack set that was operated on,
//! holding every member `StackSetOperationSummary` reports. An operation that is still
//! running concludes on a clock rather than on a read:
//! [`StoredStackSetOperation::settle`] materialises the terminal status that was owed at a
//! given instant, the way every other in-flight record of this service does.
//!
//! ## What writes it
//!
//! A stack set is written by `CreateStackSet`, and its instances and operations by
//! `CreateStackInstances`, `UpdateStackSet` and `DeleteStackInstances`; every one of those
//! operations is still a
//! scaffold answering 501, each with its own stub issue. The ONE way a record enters this
//! store is
//! [`crate::state::ComAmazonawsCloudformationCloudformationState::record_stack_set`], so
//! those writers — and the suites that cover the reads over records no wire call can create
//! yet — go through it rather than reaching into the vector.

use std::time::SystemTime;

/// How many stack sets this store holds, oldest reclaimed first.
///
/// Every growth site in this service's store is bounded. CloudFormation meters stack sets
/// per administrator account and models `LimitExceededException` on `CreateStackSet` for a
/// quota already reached; this is this store's own ceiling rather than that quota — chosen,
/// and far above any client workflow — and the refusal past it belongs to the write path.
pub const MAX_STACK_SETS: usize = 200;

/// How many instances ONE stack set holds, oldest reclaimed first.
///
/// A stack set's instances are the product of the accounts and the Regions a deployment
/// targets, both chosen by the caller, so the store bounds the product. Chosen, like every
/// other count here.
pub const MAX_INSTANCES_PER_STACK_SET: usize = 2_000;

/// How many operation records ONE stack set holds, oldest reclaimed first.
///
/// A stack set's operation history grows by one record per deployment call a caller makes
/// against it, so the store bounds it. Chosen, like every other count here, and far above
/// any client workflow.
pub const MAX_OPERATIONS_PER_STACK_SET: usize = 500;

/// The model's `StackInstanceStatus`: the instance's status "in terms of its synchronization
/// with its associated stack set" (`smithy_query(query="shape",
/// shape="StackInstanceSummary", member="Status")`).
pub mod instance_status {
    /// "CURRENT: The stack is currently up to date with the StackSet."
    pub const CURRENT: &str = "CURRENT";
    /// "OUTDATED: The stack isn't currently up to date with the StackSet."
    pub const OUTDATED: &str = "OUTDATED";
    /// "INOPERABLE: A `DeleteStackInstances` operation has failed and left the stack in an
    /// unstable state."
    pub const INOPERABLE: &str = "INOPERABLE";
}

/// The model's `StackInstanceDetailedStatus`: the status of the stack-set operation "in the
/// specified account and Region", which is what the `DETAILED_STATUS` filter selects on.
pub mod detailed_status {
    /// "PENDING: The operation in the specified account and Region has yet to start."
    pub const PENDING: &str = "PENDING";
    /// "RUNNING: The operation in the specified account and Region is currently in
    /// progress."
    pub const RUNNING: &str = "RUNNING";
    /// "SUCCEEDED": the operation in that account and Region completed.
    pub const SUCCEEDED: &str = "SUCCEEDED";
    /// "FAILED: The operation in the specified account and Region failed."
    pub const FAILED: &str = "FAILED";
    /// "CANCELLED: The operation in the specified account and Region has been canceled."
    pub const CANCELLED: &str = "CANCELLED";
    /// "INOPERABLE: A `DeleteStackInstances` operation has failed and left the stack in an
    /// unstable state."
    pub const INOPERABLE: &str = "INOPERABLE";
    /// "SKIPPED_SUSPENDED_ACCOUNT": the instance's account is suspended, so it was skipped.
    pub const SKIPPED_SUSPENDED_ACCOUNT: &str = "SKIPPED_SUSPENDED_ACCOUNT";
    /// "FAILED_IMPORT: The import of the stack instance in the specified account and Region
    /// failed."
    pub const FAILED_IMPORT: &str = "FAILED_IMPORT";
}

/// The model's `StackSetOperationAction`: "the type of operation" a stack-set operation is
/// (`smithy_query(query="shape", shape="StackSetOperationAction")`).
pub mod operation_action {
    /// "CREATE": an operation that creates stack instances of the stack set.
    pub const CREATE: &str = "CREATE";
    /// "UPDATE": an operation that affects "both the StackSet itself and all associated
    /// StackSet instances".
    pub const UPDATE: &str = "UPDATE";
    /// "DELETE": an operation that deletes stack instances of the stack set.
    pub const DELETE: &str = "DELETE";
    /// "DETECT_DRIFT": a drift detection run across the stack set.
    pub const DETECT_DRIFT: &str = "DETECT_DRIFT";
}

/// The model's `StackSetOperationStatus`: "the overall status of the operation"
/// (`smithy_query(query="shape", shape="StackSetOperationStatus")`).
pub mod operation_status {
    /// "RUNNING: The operation is currently being performed."
    pub const RUNNING: &str = "RUNNING";
    /// "SUCCEEDED: The operation completed creating or updating all the specified stacks
    /// without exceeding the failure tolerance for the operation."
    pub const SUCCEEDED: &str = "SUCCEEDED";
    /// "FAILED: The operation exceeded the specified failure tolerance."
    pub const FAILED: &str = "FAILED";
    /// "STOPPING: The operation is in the process of stopping, at user request."
    pub const STOPPING: &str = "STOPPING";
    /// "STOPPED: The user has canceled the operation."
    pub const STOPPED: &str = "STOPPED";
    /// "QUEUED: [Service-managed permissions] For automatic deployments that require a
    /// sequence of operations, the operation is queued to be performed."
    pub const QUEUED: &str = "QUEUED";
}

/// The model's `StackInstanceFilterName`: the three things a request may filter stack
/// instances by (`smithy_query(query="shape", shape="StackInstanceFilterName")`).
pub mod filter_name {
    /// Filters on the instance's `StackInstanceStatus$DetailedStatus`.
    pub const DETAILED_STATUS: &str = "DETAILED_STATUS";
    /// Filters on "the last unique ID of a StackSet operation performed on a stack
    /// instance".
    pub const LAST_OPERATION_ID: &str = "LAST_OPERATION_ID";
    /// Filters on the instance's `DriftStatus`.
    pub const DRIFT_STATUS: &str = "DRIFT_STATUS";
}

/// Mint a stack set's id: `<name>:<uuid>`, the form CloudFormation gives a stack set, with
/// the same 8-4-4-4-12 identifier every other id this service mints carries.
pub fn new_stack_set_id(stack_set_name: &str) -> String {
    format!("{stack_set_name}:{}", super::new_identifier(stack_set_name))
}

/// ONE stack set: how it is addressed, and the instances it carries.
#[derive(Debug, Clone)]
pub struct StoredStackSet {
    /// `StackSetId`, which is also what `StackInstanceSummary$StackSetId` reports: a stack
    /// set is addressable by this as well as by its name.
    pub stack_set_id: String,
    /// `StackSetName`: "the name of the StackSet", unique within the administrator account
    /// and Region that holds it.
    pub stack_set_name: String,
    /// The Region the stack set was created in. A read answers for the Region its request
    /// was signed for, the way every other read of this service does.
    pub region: String,
    /// The stack set's instances, in the order they entered it, so a paginated read is a
    /// stable window over them. Bounded at [`MAX_INSTANCES_PER_STACK_SET`].
    pub instances: Vec<StoredStackInstance>,
    /// The operations performed on this stack set, in the order they were INITIATED, which
    /// is the order `StoredStackSet::operations_newest_first` reverses for the read.
    /// Bounded at [`MAX_OPERATIONS_PER_STACK_SET`].
    pub operations: Vec<StoredStackSetOperation>,
}

impl StoredStackSet {
    /// A stack set of `stack_set_name` in `region`, with no instances yet and a freshly
    /// minted id.
    pub fn new(stack_set_name: impl Into<String>, region: impl Into<String>) -> Self {
        let stack_set_name = stack_set_name.into();
        Self {
            stack_set_id: new_stack_set_id(&stack_set_name),
            stack_set_name,
            region: region.into(),
            instances: Vec::new(),
            operations: Vec::new(),
        }
    }

    /// Whether `name_or_id` addresses this stack set in `region`: `StackSetName` is "the
    /// name or unique ID of the StackSet", and nothing outside that Region is addressable.
    pub fn is_addressed_by(&self, region: &str, name_or_id: &str) -> bool {
        self.region == region
            && (self.stack_set_name == name_or_id || self.stack_set_id == name_or_id)
    }

    /// Put `instance` in this stack set, replacing the one for the same account and Region
    /// if it already holds one — an account and Region pair carries at most ONE instance of
    /// a stack set, which is why `DescribeStackInstance` addresses one by that pair alone.
    ///
    /// Bounded at [`MAX_INSTANCES_PER_STACK_SET`], oldest reclaimed first.
    pub fn record_instance(&mut self, instance: StoredStackInstance) {
        match self
            .instances
            .iter_mut()
            .find(|held| held.is_in(&instance.account, &instance.region))
        {
            Some(held) => *held = instance,
            None => self.instances.push(instance),
        }
        if self.instances.len() > MAX_INSTANCES_PER_STACK_SET {
            let excess = self.instances.len() - MAX_INSTANCES_PER_STACK_SET;
            self.instances.drain(..excess);
        }
    }

    /// Record one operation performed on this stack set, replacing the record of the same
    /// `OperationId` if it already holds one — an operation id identifies one operation of
    /// a stack set, which is why `DescribeStackSetOperation` addresses one by it alone.
    ///
    /// Bounded at [`MAX_OPERATIONS_PER_STACK_SET`], oldest reclaimed first.
    pub fn record_operation(&mut self, operation: StoredStackSetOperation) {
        match self
            .operations
            .iter_mut()
            .find(|held| held.operation_id == operation.operation_id)
        {
            Some(held) => *held = operation,
            None => self.operations.push(operation),
        }
        if self.operations.len() > MAX_OPERATIONS_PER_STACK_SET {
            let excess = self.operations.len() - MAX_OPERATIONS_PER_STACK_SET;
            self.operations.drain(..excess);
        }
    }

    /// The operations performed on this stack set, most recently initiated first.
    ///
    /// The model documents no order for `ListStackSetOperations`, so this store reports an
    /// operation HISTORY the way it reports the other one it holds (`ListResourceScans`:
    /// "list the resource scans from newest to oldest"): the operation in flight, and the
    /// one that concluded most recently, are on the first page however long the history is.
    /// The stored order is the order the operations were initiated in, so reversing it is
    /// deterministic even for two operations recorded in the same instant.
    pub fn operations_newest_first(&self) -> impl Iterator<Item = &StoredStackSetOperation> {
        self.operations.iter().rev()
    }

    /// Materialise every status transition the clock has made due on this stack set's
    /// operations.
    pub fn settle(&mut self, now: SystemTime) {
        for operation in &mut self.operations {
            operation.settle(now);
        }
    }
}

/// The terminal status a stack-set operation still in flight is headed for, and the instant
/// it is due.
#[derive(Debug, Clone)]
pub struct PendingOperationOutcome {
    /// [`operation_status`]: the terminal status this operation reaches.
    pub status: String,
    /// `StatusReason`: "the status of the operation in details", for a conclusion that has
    /// one.
    pub status_reason: Option<String>,
    /// `StatusDetails$FailedStackInstancesCount`: "the number of stack instances for which
    /// the StackSet operation failed", as of that conclusion.
    pub failed_stack_instances_count: Option<i32>,
    /// The instant the conclusion became due; also the `EndTimestamp` it is stamped with,
    /// since the operation ENDING is what stamps it.
    pub due_at: SystemTime,
}

/// The `StackSetOperationPreferences` one operation ran under, as stored.
///
/// "The user-specified preferences for how CloudFormation performs a StackSet operation":
/// what the request that started the operation supplied, which is what
/// `StackSetOperationSummary$OperationPreferences` reports back. Only the members the
/// request named are held — the model makes every one optional and a pair of them mutually
/// exclusive, so an absent member is not the same as a zero one.
#[derive(Debug, Clone, Default)]
pub struct StoredOperationPreferences {
    /// `RegionConcurrencyType`: "the concurrency type of deploying StackSets operations in
    /// Regions, could be in parallel or one Region at a time".
    pub region_concurrency_type: Option<String>,
    /// `RegionOrder`: "the order of the Regions where you want to perform the stack
    /// operation".
    pub region_order: Vec<String>,
    /// `FailureToleranceCount`: "the number of accounts per Region this operation can fail
    /// in before CloudFormation stops the operation in that Region".
    pub failure_tolerance_count: Option<i32>,
    /// `FailureTolerancePercentage`: the same tolerance expressed as a percentage.
    pub failure_tolerance_percentage: Option<i32>,
    /// `MaxConcurrentCount`: "the maximum number of accounts in which to perform this
    /// operation at one time".
    pub max_concurrent_count: Option<i32>,
    /// `MaxConcurrentPercentage`: the same maximum expressed as a percentage.
    pub max_concurrent_percentage: Option<i32>,
    /// `ConcurrencyMode`: "specifies how the concurrency level behaves during the operation
    /// execution".
    pub concurrency_mode: Option<String>,
}

/// ONE operation performed on a stack set, with every member `StackSetOperationSummary`
/// reports.
#[derive(Debug, Clone)]
pub struct StoredStackSetOperation {
    /// `OperationId`: "the unique ID of the StackSet operation", a `ClientRequestToken`
    /// (`@pattern("^[a-zA-Z0-9][-a-zA-Z0-9]*$")`) — either the token the request supplied or
    /// the one the service minted for it.
    pub operation_id: String,
    /// `Action`: [`operation_action`] — "the type of operation".
    pub action: String,
    /// `Status`: [`operation_status`] — "the overall status of the operation".
    pub status: String,
    /// `CreationTimestamp`: "the time at which the operation was initiated".
    pub creation: SystemTime,
    /// `EndTimestamp`: "the time at which the StackSet operation ended, across all accounts
    /// and Regions specified". Absent while the operation has not ended.
    pub end: Option<SystemTime>,
    /// `StatusReason`: "the status of the operation in details".
    pub status_reason: Option<String>,
    /// `StatusDetails$FailedStackInstancesCount`: "the number of stack instances for which
    /// the StackSet operation failed". Absent for an operation whose details the service
    /// holds none of.
    pub failed_stack_instances_count: Option<i32>,
    /// `OperationPreferences`: what the request that started the operation asked for, absent
    /// for one that asked for nothing.
    pub preferences: Option<StoredOperationPreferences>,
    /// The terminal status the clock owes this operation, until [`Self::settle`]
    /// materialises it.
    pub pending: Option<PendingOperationOutcome>,
}

impl StoredStackSetOperation {
    /// An operation as the call that starts it records it: RUNNING, with no end instant yet
    /// and its conclusion due one [`crate::state::STATUS_TRANSITION_DELAY`] from that call,
    /// so a caller listing straight after the call still sees the operation in flight.
    pub fn running(
        operation_id: impl Into<String>,
        action: impl Into<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            operation_id: operation_id.into(),
            action: action.into(),
            status: operation_status::RUNNING.to_string(),
            creation: now,
            end: None,
            status_reason: None,
            failed_stack_instances_count: None,
            preferences: None,
            pending: Some(PendingOperationOutcome {
                status: operation_status::SUCCEEDED.to_string(),
                status_reason: None,
                failed_stack_instances_count: Some(0),
                due_at: now + crate::state::STATUS_TRANSITION_DELAY,
            }),
        }
    }

    /// Materialise the status transition the clock has already made due, if any.
    ///
    /// Only ever applies a transition whose due instant has passed, so it reports what was
    /// owed at `now` even if nobody had called. The operation ENDING is what stamps
    /// `EndTimestamp`, so that instant is the one that was due rather than now.
    pub fn settle(&mut self, now: SystemTime) {
        if self.pending.as_ref().is_none_or(|owed| owed.due_at > now) {
            return;
        }
        let owed = self.pending.take().expect("checked just above");
        self.status = owed.status;
        self.status_reason = owed.status_reason;
        self.failed_stack_instances_count = owed.failed_stack_instances_count;
        self.end = Some(owed.due_at);
    }
}

/// ONE stack instance: "a reference to a stack in a target account within a Region", with
/// every member `StackInstanceSummary` reports.
#[derive(Debug, Clone)]
pub struct StoredStackInstance {
    /// `Account`: "the name of the Amazon Web Services account that the stack instance is
    /// associated with", which is what `StackInstanceAccount` filters on.
    pub account: String,
    /// `Region`: "the name of the Amazon Web Services Region that the stack instance is
    /// associated with", which is what `StackInstanceRegion` filters on.
    pub region: String,
    /// `StackId`: "the ID of the stack instance" — the stack deployed in that account and
    /// Region, absent while the deployment has not produced one.
    pub stack_id: Option<String>,
    /// `Status`: [`instance_status`].
    pub status: String,
    /// `StackInstanceStatus$DetailedStatus`: [`detailed_status`].
    pub detailed_status: String,
    /// `StatusReason`: "the explanation for the specific status code assigned to this stack
    /// instance".
    pub status_reason: Option<String>,
    /// `DriftStatus`: the instance's configuration "compared to the expected template and
    /// parameter configuration of the StackSet it belongs to"
    /// (`crate::state::DRIFT_STATUS_NOT_CHECKED` until a detection has run against it).
    pub drift_status: String,
    /// `LastDriftCheckTimestamp`: "most recent time when CloudFormation performed a drift
    /// detection operation on the stack instance. This value will be NULL for any stack
    /// instance that drift detection hasn't yet been performed on."
    pub last_drift_check: Option<SystemTime>,
    /// `LastOperationId`: "the last unique ID of a StackSet operation performed on a stack
    /// instance", which is what the `LAST_OPERATION_ID` filter selects on.
    pub last_operation_id: String,
    /// `OrganizationalUnitId`: "[Service-managed permissions] The organization root ID or
    /// organizational unit (OU) IDs" the instance was deployed to, absent for a stack set
    /// with self-managed permissions.
    pub organizational_unit_id: Option<String>,
}

impl StoredStackInstance {
    /// An instance in `account` and `region` that a stack-set operation has not finished
    /// deploying yet: PENDING, so not yet up to date with its stack set, and never drift
    /// checked.
    pub fn new(
        account: impl Into<String>,
        region: impl Into<String>,
        last_operation_id: impl Into<String>,
    ) -> Self {
        Self {
            account: account.into(),
            region: region.into(),
            stack_id: None,
            status: instance_status::OUTDATED.to_string(),
            detailed_status: detailed_status::PENDING.to_string(),
            status_reason: None,
            drift_status: super::DRIFT_STATUS_NOT_CHECKED.to_string(),
            last_drift_check: None,
            last_operation_id: last_operation_id.into(),
            organizational_unit_id: None,
        }
    }

    /// Whether this instance is the one deployed in `account` and `region` — the pair that
    /// identifies an instance within its stack set.
    pub fn is_in(&self, account: &str, region: &str) -> bool {
        self.account == account && self.region == region
    }

    /// Whether the filter `name` with `value` selects this instance.
    ///
    /// `StackInstanceFilter$Values` is "the status to filter by": one value, compared with
    /// the member the filter names, so an instance carrying another value is not selected.
    /// A name outside the modelled `StackInstanceFilterName` enum cannot reach here — the
    /// boundary refuses it before a handler runs — and selects nothing if it ever did.
    pub fn matches_filter(&self, name: &str, value: &str) -> bool {
        match name {
            filter_name::DETAILED_STATUS => self.detailed_status == value,
            filter_name::LAST_OPERATION_ID => self.last_operation_id == value,
            filter_name::DRIFT_STATUS => self.drift_status == value,
            _ => false,
        }
    }
}
