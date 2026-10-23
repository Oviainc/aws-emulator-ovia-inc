// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudformation-cloudformation service.
//!
//! One store holds every stack this emulator has ever created, in creation order, so
//! `CreateStack` / `UpdateStack` / `DeleteStack` write exactly the rows
//! `DescribeStacks` / `ListStacks` / `GetTemplate` / `DescribeStackEvents` /
//! `DescribeStackResources` read back.
//! A deleted stack is NOT dropped: CloudFormation keeps a deleted stack's summary,
//! template, events and resources addressable by its stack id (`ListStacks`: "Summary
//! information for stacks that have been deleted is kept for 90 days"; `GetTemplate`:
//! "You can get the template for running or deleted stacks"; `DescribeStackResources`:
//! "For deleted stacks, DescribeStackResources returns resource information for up to 90
//! days after the stack has been deleted"), so the single `stacks` vector carries
//! live and deleted stacks alike and `StoredStack::is_live` is what separates them.
//!
//! Beside the stacks it holds every CHANGE SET, which is a persisted plan for one stack:
//! `CreateChangeSet` writes the template and configuration it would apply plus the
//! resource-level change list it computed, `DescribeChangeSet` / `GetTemplate` read them
//! back, and `ExecuteChangeSet` is what copies the plan onto the stack. A change set lives
//! as long as its stack does — CloudFormation deletes a stack's change sets with the
//! stack, which
//! [`ComAmazonawsCloudformationCloudformationState::settle`] enforces.
//!
//! ## Bounds
//!
//! Every growth site in this store is bounded, because a caller chooses both how many
//! records it writes and how big each one is: [`MAX_STACKS`] live stacks (past which
//! `CreateStack` answers the modelled `LimitExceededException`),
//! [`MAX_DELETED_STACKS_RETAINED`] completed deletions (the oldest deletion is reclaimed
//! past that, which is this emulator's stand-in for CloudFormation's 90-day retention of
//! deleted-stack summaries), [`MAX_EVENTS_PER_STACK`] events per stack (oldest-first),
//! [`TEMPLATE_BODY_MAX_BYTES`] per stored template body (the documented `TemplateBody`
//! maximum, which the model carries no `@length` for), [`MAX_CHANGE_SETS_PER_STACK`]
//! change sets per stack and [`MAX_CHANGE_SETS`] across the store (past either
//! `CreateChangeSet` answers the modelled `LimitExceededException`), [`PAGE_SIZE`]
//! items per paginated response and [`MAX_DESCRIBED_STACK_RESOURCES`] resources per
//! `DescribeStackResources` response (the documented cap on that unpaginated read).
//!
//! Reading a stack's template body and evaluating what it declares is bounded by the
//! [`template`] module's own caps (nodes, depth, the documented per-template quotas and
//! one evaluation budget shared by every expression of one deployment).
//!
//! ## Status transitions the clock owes
//!
//! CloudFormation answers `CreateStack` / `UpdateStack` / `DeleteStack` while the
//! operation is still in progress and settles the stack a moment later. Each call
//! therefore records a [`PendingStatus`] with the instant its terminal status becomes
//! due, and every operation calls [`ComAmazonawsCloudformationCloudformationState::settle`]
//! first: a caller that reads straight after the write still sees `*_IN_PROGRESS`, and
//! one that reads after the due instant sees the terminal status and the event that
//! carries it. Nothing sleeps, and no read invents a transition the clock had not
//! already made due.
//!
//! A change set's own creation is asynchronous the same way ("After the CreateChangeSet
//! call successfully completes, CloudFormation starts creating the change set"), so it
//! carries a [`PendingChangeSetStatus`]: it is answered `CREATE_PENDING` and settles to
//! `CREATE_COMPLETE` — or to `FAILED`, when the plan it computed is empty — which is what
//! `ExecuteChangeSet` waits for.
//!
//! What a create or update DEPLOYS — the resources the template declares and the outputs
//! evaluated over them (see [`template`]) — rides on that same clock: the call computes
//! the deployment ([`StoredStack::deploy_template`]) and `settle` installs it with the
//! terminal status, so a stack always reports the resources and outputs of the template
//! that finished deploying.
//!
//! CREATING those resources in the services that own them is [`provisioning`], which the
//! deploying operation drives with no lock held — and which leaves the stack
//! `CREATE_FAILED` with the owning service's own refusal as the reason rather than
//! reporting a success whose resources do not exist.
//!
//! ## The extension registry
//!
//! Beside the stacks and their change sets the store holds this account and Region's
//! EXTENSION REGISTRY — the resource types, modules and Hooks it has registered or
//! activated, and the versions of each (see [`registry`]). It is a store of its own, not a
//! stack's: nothing in a registry record settles on the stack clock, and `DescribeType`
//! reads it the way `DescribeStacks` reads the stacks.
//!
//! ## Drift detection
//!
//! It also holds every DRIFT DETECTION a caller has started ([`StoredDriftDetection`]):
//! `DetectStackDrift` records one and hands back its id, [`drift`] is what compares each
//! covered resource against the service that owns it, and
//! `DescribeStackDriftDetectionStatus` reads the detection's own status back. That status
//! rides the stack clock — answered `DETECTION_IN_PROGRESS` and concluded
//! [`DRIFT_DETECTION_DELAY`] later, the way AWS answers a detection that is still running
//! — while what the check FOUND is recorded as of the check
//! ([`ComAmazonawsCloudformationCloudformationState::apply_drift_result`]) onto the stack
//! (`Stack$DriftInformation`) and each resource it covered
//! (`StackResource$DriftInformation`), where every stack read reports it.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

/// Creating the resources a stack's template declares in the services that own them, and
/// deleting them when the stack is deleted.
pub mod provisioning;
/// Reading a stack's template body, and evaluating the resources and outputs it
/// declares.
pub mod template;

/// One entry per resource type this emulator really creates: which service owns it, and
/// the create and delete calls that make its resources exist.
mod provisioner;

/// Detecting whether a stack's resources still match the configuration its template
/// declared, by reading each one from the service that owns it.
pub mod drift;

/// Which resource TYPES a template may declare at all: the catalogue CloudFormation
/// publishes, and the refusal a template declaring anything else is held to.
pub mod resource_types;

/// The extension registry: the registered and activated extensions this account and
/// Region holds, and the versions of each.
pub mod registry;

/// How long after the call that starts it a stack's terminal status becomes due.
///
/// Chosen (CloudFormation's real stack operations take as long as the resources do, so
/// there is no documented value to honour): long enough that a caller acting straight
/// after a create, update or delete observes the `*_IN_PROGRESS` status and the state
/// errors that go with it, short enough that a test can wait for it.
pub const STATUS_TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// The region an unsigned request is served as. The emulator's own default; a signed
/// request is served in the region of its SigV4 credential scope.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace (`awsService.arnNamespace` in the model).
pub const ARN_NAMESPACE: &str = "cloudformation";

/// The resource type CloudFormation reports on a stack-level event.
pub const STACK_RESOURCE_TYPE: &str = "AWS::CloudFormation::Stack";

/// `StackDriftStatus` for a stack no drift detection has run against yet:
/// "CloudFormation hasn't checked if the stack differs from its expected template
/// configuration."
pub const DRIFT_STATUS_NOT_CHECKED: &str = "NOT_CHECKED";

/// `StackDriftStatus` for a stack whose "actual configuration matches its expected
/// template configuration".
pub const DRIFT_STATUS_IN_SYNC: &str = "IN_SYNC";

/// `StackDriftStatus` for a stack that "differs from its expected template
/// configuration".
pub const DRIFT_STATUS_DRIFTED: &str = "DRIFTED";

/// `StackDriftStatus` for a stack CloudFormation "could not run drift detection for a
/// resource in".
pub const DRIFT_STATUS_UNKNOWN: &str = "UNKNOWN";

/// The `StackResourceDriftStatus` values this service leaves a resource's drift result in
/// (all from the model's `StackResourceDriftStatus` enum, documented member by member on
/// `StackResourceDriftInformation$StackResourceDriftStatus`).
pub mod resource_drift_status {
    /// "The resource's actual configuration matches its expected configuration."
    pub const IN_SYNC: &str = "IN_SYNC";
    /// "The resource differs from its expected configuration."
    pub const MODIFIED: &str = "MODIFIED";
    /// "The resource differs from its expected configuration in that it has been
    /// deleted."
    pub const DELETED: &str = "DELETED";
    /// "CloudFormation has not checked if the resource differs from its expected
    /// configuration. Any resources that do not currently support drift detection have a
    /// status of NOT_CHECKED."
    pub const NOT_CHECKED: &str = "NOT_CHECKED";
    /// The resource's check could not be run at all.
    pub const UNKNOWN: &str = "UNKNOWN";
}

/// The model's `StackDriftDetectionStatus`: the status of one drift-detection operation.
pub mod drift_detection_status {
    /// "The stack drift detection operation has not yet concluded."
    pub const DETECTION_IN_PROGRESS: &str = "DETECTION_IN_PROGRESS";
    /// "The stack drift detection operation has successfully completed for all resources
    /// in the stack that support drift detection."
    pub const DETECTION_COMPLETE: &str = "DETECTION_COMPLETE";
    /// "The stack drift detection operation has failed for at least one resource in the
    /// stack."
    pub const DETECTION_FAILED: &str = "DETECTION_FAILED";
}

/// How long after `DetectStackDrift` the detection's own completion becomes due.
///
/// Chosen ("DetectStackDrift can take up to several minutes, depending on the number of
/// resources contained within the stack", so there is no documented value to honour):
/// non-zero, so a caller polling `DescribeStackDriftDetectionStatus` straight after the
/// call still observes `DETECTION_IN_PROGRESS`, and short enough that the poll after it
/// sees the result.
pub const DRIFT_DETECTION_DELAY: Duration = Duration::from_millis(250);

/// The most drift-detection results this service retains, oldest reclaimed first.
///
/// `DetectStackDrift`: "CloudFormation generates new results, with a new drift detection
/// ID, each time this operation is run. However, the number of drift results
/// CloudFormation retains for any given stack, and for how long, may vary." A caller
/// chooses how many detections it runs, so the store bounds them by count. Chosen, far
/// above any client workflow.
pub const MAX_DRIFT_DETECTIONS: usize = 200;

/// `DeploymentConfig$Mode` for a stack operation that did not state one.
///
/// `DeploymentConfigMode`: "STANDARD — Use the standard deployment behavior, ensuring
/// resources are ready to serve traffic before completing the operation. This is the
/// default. You do not need to specify this value explicitly."
pub const DEPLOYMENT_MODE_STANDARD: &str = "STANDARD";

/// The most LIVE stacks this emulator holds.
///
/// CloudFormation meters stacks per Region and models `LimitExceededException` on
/// `CreateStack` for exactly that case ("The quota for the resource has already been
/// reached. For information about resource and stack limitations, see CloudFormation
/// quotas"); the documented default stack quota is 2,000 per Region and is adjustable
/// through Service Quotas, so this is that default as this emulator's own ceiling. It is
/// what bounds the live half of the store: one record, one template body and one event
/// log per live stack.
pub const MAX_STACKS: usize = 2_000;

/// The most stacks in `DELETE_COMPLETE` this emulator keeps.
///
/// CloudFormation keeps a deleted stack's summary, template and events addressable by
/// stack id for 90 days and then reclaims them. This emulator has no 90-day clock worth
/// emulating, so it bounds the same retention by COUNT instead: past this many completed
/// deletions the oldest deletion is reclaimed, so a caller that creates and deletes in a
/// loop cannot grow the store without limit. Chosen, like every count here; it sits far
/// above any client workflow.
pub const MAX_DELETED_STACKS_RETAINED: usize = 200;

/// The most events one stack retains, oldest evicted first.
///
/// CloudFormation bounds a stack's event history by age, not by count; a caller here can
/// drive an unbounded number of updates against one stack, so the emulator bounds the log
/// by count. Chosen: far more than the handful of events any client workflow reads.
pub const MAX_EVENTS_PER_STACK: usize = 1_000;

/// The most bytes of template body this service stores.
///
/// `CreateStackInput$TemplateBody`: "Structure that contains the template body with a
/// minimum length of 1 byte and a maximum length of 51,200 bytes." The model carries only
/// `@length(min: 1)`, so the handler is what enforces the documented maximum.
pub const TEMPLATE_BODY_MAX_BYTES: usize = 51_200;

/// How many items one page of a paginated read carries.
///
/// CloudFormation pages `DescribeStacks`, `ListStacks` and `DescribeStackEvents` by
/// response SIZE ("If the output exceeds 1 MB in size"), which is not a count a client can
/// depend on, so this emulator pages by a fixed count instead — chosen, and the thing that
/// keeps one response bounded however large the store is.
pub const PAGE_SIZE: usize = 100;

/// The most resources one `DescribeStackResources` response carries.
///
/// `DescribeStackResources`: "Only the first 100 resources will be returned. If your
/// stack has more resources than this, you should use ListStackResources instead." The
/// operation is not `@paginated`, so this documented cap is the whole of one answer —
/// a stack may declare up to [`template::MAX_RESOURCES_PER_TEMPLATE`] of them.
pub const MAX_DESCRIBED_STACK_RESOURCES: usize = 100;

/// `Err(message)` for a `TemplateBody` past its documented maximum, naming the member and
/// the constraint it failed.
pub fn validate_template_body(template_body: &str) -> Result<(), String> {
    if template_body.len() > TEMPLATE_BODY_MAX_BYTES {
        return Err(format!(
            "1 validation error detected: Value at 'templateBody' failed to satisfy \
             constraint: Member must have length less than or equal to \
             {TEMPLATE_BODY_MAX_BYTES}"
        ));
    }
    Ok(())
}

/// `Err(message)` for a template body declaring a resource type CloudFormation does not
/// recognise — the rule every operation that SUBMITS a template is held to, before
/// anything is created ([`resource_types`] is where the recognised set lives).
///
/// `Ok(())` for a body this service's reader will not read at all: a body that is neither
/// JSON nor YAML, or one past a documented per-template quota, declares no resource to
/// hold to the catalogue and is answered by the template-format rules instead.
pub fn validate_resource_types(template_body: &str) -> Result<(), String> {
    match template::parse(template_body) {
        Some(template) => resource_types::refuse_unrecognized(&template),
        None => Ok(()),
    }
}

/// `StackStatus` values this service puts a stack into (all from the model's
/// `StackStatus` enum).
pub mod status {
    pub const CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
    pub const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
    pub const CREATE_FAILED: &str = "CREATE_FAILED";
    pub const UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";
    pub const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";
    pub const UPDATE_FAILED: &str = "UPDATE_FAILED";
    pub const DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";
    pub const DELETE_COMPLETE: &str = "DELETE_COMPLETE";
    pub const DELETE_FAILED: &str = "DELETE_FAILED";
    /// The status of a stack a `CREATE` change set brought into existence and nothing has
    /// executed yet (`CreateChangeSetInput$ChangeSetType`: "If you create a change set for
    /// a new stack, CloudFormation creates a stack with a unique stack ID, but no template
    /// or resources. The stack will be in the REVIEW_IN_PROGRESS state until you execute
    /// the change set").
    pub const REVIEW_IN_PROGRESS: &str = "REVIEW_IN_PROGRESS";
}

/// `ResourceStatus` values this service leaves a stack's resource in (all from the
/// model's `ResourceStatus` enum).
///
/// A stack operation of this service deploys a whole template at once and never fails
/// part-way, so the only statuses a resource reaches here are the completions of the
/// operation that created, updated or deleted it.
pub mod resource_status {
    pub const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
    pub const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";
    pub const DELETE_COMPLETE: &str = "DELETE_COMPLETE";
}

/// `ChangeSetStatus` values this service puts a change set into (all from the model's
/// `ChangeSetStatus` enum).
pub mod change_set_status {
    pub const CREATE_PENDING: &str = "CREATE_PENDING";
    pub const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
    pub const FAILED: &str = "FAILED";
}

/// `ExecutionStatus` values this service puts a change set into (all from the model's
/// `ExecutionStatus` enum).
pub mod execution_status {
    /// The change set is not (yet, or ever) executable: its creation has not completed, or
    /// it failed.
    pub const UNAVAILABLE: &str = "UNAVAILABLE";
    pub const AVAILABLE: &str = "AVAILABLE";
    pub const EXECUTE_IN_PROGRESS: &str = "EXECUTE_IN_PROGRESS";
    pub const EXECUTE_COMPLETE: &str = "EXECUTE_COMPLETE";
}

/// `ChangeSetType` values (the model's `ChangeSetType` enum).
pub mod change_set_type {
    pub const CREATE: &str = "CREATE";
    pub const UPDATE: &str = "UPDATE";
    pub const IMPORT: &str = "IMPORT";
}

/// `ChangeAction` values this service computes (from the model's `ChangeAction` enum).
pub mod change_action {
    pub const ADD: &str = "Add";
    pub const MODIFY: &str = "Modify";
    pub const REMOVE: &str = "Remove";
}

/// `OperationType` values this service records for a stack operation (all from the
/// model's `OperationType` enum), one per call that acts on the stack.
pub mod operation_type {
    pub const CREATE_STACK: &str = "CREATE_STACK";
    pub const UPDATE_STACK: &str = "UPDATE_STACK";
    pub const DELETE_STACK: &str = "DELETE_STACK";
    pub const CREATE_CHANGESET: &str = "CREATE_CHANGESET";
}

/// The `OperationType` of the stack operation that moves a stack into `in_progress`, or
/// `None` for a status no stack operation of this service opens.
pub fn operation_type_of(in_progress: &str) -> Option<&'static str> {
    match in_progress {
        status::CREATE_IN_PROGRESS => Some(operation_type::CREATE_STACK),
        status::UPDATE_IN_PROGRESS => Some(operation_type::UPDATE_STACK),
        status::DELETE_IN_PROGRESS => Some(operation_type::DELETE_STACK),
        _ => None,
    }
}

/// The `Change$Type` every change this service computes carries: the model's `ChangeType`
/// enum has exactly one value, and a change set here describes resources.
pub const CHANGE_TYPE_RESOURCE: &str = "Resource";

/// The reason CloudFormation records on the event that opens a stack operation.
pub const USER_INITIATED: &str = "User Initiated";

/// The `StatusReason` CloudFormation puts on a change set that would change nothing —
/// the message it answers for a change set whose computed change list is empty.
pub const NO_CHANGES_REASON: &str = "The submitted information didn't contain changes. \
                                     Submit different information to create a change set.";

/// The most change sets one stack holds.
///
/// CloudFormation meters change sets per stack and models `LimitExceededException` on
/// `CreateChangeSet` for exactly that case; the documented default quota is 20 change sets
/// per stack and is adjustable through Service Quotas, so this is that default as this
/// emulator's own ceiling.
pub const MAX_CHANGE_SETS_PER_STACK: usize = 20;

/// The most change sets this service holds across every stack.
///
/// The per-stack ceiling alone would let one caller hold `MAX_STACKS` × that many stored
/// templates, so the store carries a second, global bound on the same records. Chosen,
/// like every count here, and far above any client workflow.
pub const MAX_CHANGE_SETS: usize = 2_000;

/// A resolved stack parameter: the key and the value in force on the stack.
///
/// `Parameter$UsePreviousValue` is a request-time instruction, not stored state — it is
/// resolved against the stored value when an update is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredParameter {
    pub key: String,
    pub value: String,
}

/// One resource a stack's template declares, as this emulator records it.
///
/// The physical resource id and the attributes are what CloudFormation's documented
/// return values for that resource type give it (see [`template::provision`]); they are
/// what `Ref` and `Fn::GetAtt` resolve to when the stack's outputs are evaluated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredResource {
    pub logical_id: String,
    pub resource_type: String,
    pub physical_resource_id: String,
    /// The attributes `Fn::GetAtt` reads off this resource, keyed by attribute name.
    pub attributes: BTreeMap<String, String>,
    /// The content of the `Metadata` resource attribute the template declared for this
    /// resource, as JSON — what `StackResourceDetail$Metadata` reports ("The content of
    /// the `Metadata` attribute declared for the resource"). `None` for a resource whose
    /// declaration carries no `Metadata`, since the member is optional and this emulator
    /// reports what the template declared rather than an invented value.
    ///
    /// Captured where the template is DEPLOYED ([`template::provision`]), not on a read:
    /// a read must not re-parse a caller-sized body under the store lock, and the value
    /// belongs to the template that finished deploying rather than to the one in force.
    pub metadata: Option<String>,
    /// The resource's declared `Properties` with every intrinsic function EVALUATED — the
    /// request the owning service's create API takes
    /// ([`provisioning`](crate::state::provisioning)). `Null` for a resource that declared
    /// none.
    pub properties: ::serde_json::Value,
    /// Whether the resource was CREATED in the service that owns it, so the stack's
    /// deletion has to delete it there. `false` for a resource type this emulator has no
    /// provisioner for: nothing was created for it anywhere (see
    /// [`provisioning`](crate::state::provisioning)).
    pub materialised: bool,
}

/// One resource a stack has DEPLOYED: what the template declared it as, plus the
/// resource-level status the stack operation that last touched it left it in.
///
/// A resource only reaches a status by way of an operation COMPLETING against it, so
/// these two members are filled in by [`StoredStack::settle`] — never by a read — and a
/// resource a completed operation left exactly as it found it keeps the status and the
/// instant it already carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployedResource {
    pub resource: StoredResource,
    /// The model's `ResourceStatus`, as `StackResource$ResourceStatus` reports it
    /// ("Current status of the resource").
    pub status: String,
    /// `StackResource$Timestamp`: "Time the status was updated" — the instant the stack
    /// operation that left this resource in `status` completed.
    pub status_timestamp: SystemTime,
    /// `StackResourceDriftInformation$StackResourceDriftStatus`: this resource's own drift
    /// result, as the last drift detection that COMPLETED over it left it
    /// ([`drift`]). [`resource_drift_status::NOT_CHECKED`] until one has.
    ///
    /// A deployment that re-created or changed the resource leaves a new record, so the
    /// result belongs to the resource as it is now; one a deployment left untouched keeps
    /// the result it already carried.
    pub drift_status: String,
    /// `StackResourceDriftInformation$LastCheckTimestamp`: "when CloudFormation last
    /// checked if the resource had drifted from its expected configuration".
    pub drift_checked_at: Option<SystemTime>,
}

impl DeployedResource {
    /// A resource record as a stack operation installs it: with the status that operation
    /// left it in, and no drift result, since nothing has checked it for drift.
    pub fn new(resource: StoredResource, status: &str, at: SystemTime) -> Self {
        Self {
            resource,
            status: status.to_string(),
            status_timestamp: at,
            drift_status: resource_drift_status::NOT_CHECKED.to_string(),
            drift_checked_at: None,
        }
    }
}

/// One evaluated stack output: an `Outputs` entry of the stack's template with its
/// `Value` resolved against the stack's parameters and resources.
///
/// `value` is `None` for a declared output whose value this emulator cannot evaluate —
/// the output exists because the template declares it, and `Output$OutputValue` is
/// optional, so an un-evaluable output is reported without a value rather than with an
/// invented one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredOutput {
    pub key: String,
    pub value: Option<String>,
    pub description: Option<String>,
    pub export_name: Option<String>,
}

/// One stack operation, as `Stack$LastOperations` reports it: the id the call handed back
/// and the `OperationType` it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredOperation {
    pub operation_id: String,
    pub operation_type: String,
}


/// A stack tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTag {
    pub key: String,
    pub value: String,
}

/// `DeploymentConfig` as supplied on the call that last set it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredDeploymentConfig {
    pub mode: Option<String>,
    pub disable_rollback: Option<bool>,
}

/// One `RollbackTrigger` of a stack's rollback configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRollbackTrigger {
    pub arn: String,
    pub trigger_type: String,
}

/// `RollbackConfiguration` as supplied on the call that last set it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRollbackConfiguration {
    pub triggers: Vec<StoredRollbackTrigger>,
    pub monitoring_time_in_minutes: Option<i32>,
}

/// One stack event, appended (never overwritten) by the operation that caused it.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub event_id: String,
    pub stack_id: String,
    pub stack_name: String,
    pub operation_id: Option<String>,
    pub logical_resource_id: String,
    pub physical_resource_id: Option<String>,
    pub resource_type: String,
    pub timestamp: SystemTime,
    pub resource_status: String,
    pub resource_status_reason: Option<String>,
    pub client_request_token: Option<String>,
}

/// The terminal status a stack operation is headed for, and the instant it is due.
#[derive(Debug, Clone)]
pub struct PendingStatus {
    pub status: String,
    /// The `StackStatusReason` that status carries, for an operation that is headed for a
    /// FAILED one: a failure has to say what failed, and a success states no reason.
    pub status_reason: Option<String>,
    pub due_at: SystemTime,
    pub operation_id: Option<String>,
    pub client_request_token: Option<String>,
}

/// The resources and outputs an in-flight stack operation will leave the stack with,
/// once the clock makes its terminal status due.
///
/// CloudFormation reports a stack's resources and outputs as of the template that
/// finished deploying, so a create or update computes them when it starts and they take
/// effect when it completes.
#[derive(Debug, Clone, Default)]
pub struct PendingDeployment {
    pub resources: Vec<StoredResource>,
    pub outputs: Vec<StoredOutput>,
}

/// One stack, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct StoredStack {
    pub stack_id: String,
    pub stack_name: String,
    /// The Region the stack lives in: the one the call that created it was signed for.
    /// Every identifier the stack's resources carry is Regional, so this is what the
    /// template's `AWS::Region` pseudo parameter and every minted ARN resolve to.
    pub region: String,
    /// The template in force on the stack, or `None` for a stack that has none yet: a
    /// `CREATE` change set creates its stack "with a unique stack ID, but no template or
    /// resources", and the template becomes the stack's when the change set is executed.
    pub template_body: Option<String>,
    /// The template's own `Description`, parsed once when the body is stored rather than
    /// on every read: this is where CloudFormation takes both `Stack$Description` and
    /// `StackSummary$TemplateDescription` from, and a read must not re-parse a
    /// caller-sized body under the store lock.
    pub template_description: Option<String>,
    /// The change set that last acted on this stack (`Stack$ChangeSetId`): the one that
    /// created it in `REVIEW_IN_PROGRESS`, or the one `ExecuteChangeSet` last applied.
    pub change_set_id: Option<String>,
    pub parameters: Vec<StoredParameter>,
    pub capabilities: Vec<String>,
    pub tags: Vec<StoredTag>,
    pub notification_arns: Vec<String>,
    pub role_arn: Option<String>,
    pub timeout_in_minutes: Option<i32>,
    pub disable_rollback: bool,
    pub enable_termination_protection: bool,
    pub retain_except_on_create: Option<bool>,
    pub deployment_config: Option<StoredDeploymentConfig>,
    pub rollback_configuration: Option<StoredRollbackConfiguration>,
    pub deletion_mode: Option<String>,
    /// The stack policy body in force, as `CreateStack`/`UpdateStack` last supplied it.
    ///
    /// A stack policy governs which of a stack's RESOURCES an update may change; this
    /// service records the resources a template declares but performs no resource
    /// updates against them, so the policy has nothing to protect yet and is stored (and
    /// reported by `GetStackPolicy`) rather than enforced.
    pub stack_policy_body: Option<String>,
    /// `ResourceTypes`: the resource types this stack's operations may work with. Stored
    /// as supplied; it constrains which resource types a stack operation may create,
    /// which this service does not refuse on.
    pub resource_types: Vec<String>,
    pub status: String,
    pub status_reason: Option<String>,
    pub pending: Option<PendingStatus>,
    /// The resources the stack's last COMPLETED create or update materialised (see
    /// [`StoredStack::deploy_template`]), each with the status that operation left it in.
    /// A completed DELETE leaves them in place, in `DELETE_COMPLETE`: CloudFormation
    /// reports a deleted stack's resources for as long as it reports the stack.
    pub resources: Vec<DeployedResource>,
    /// The stack's evaluated outputs, in `OutputKey` order, as of that same completed
    /// operation — what `DescribeStacks` reports as `Stack$Outputs`.
    pub outputs: Vec<StoredOutput>,
    /// What the create or update currently in flight will leave the two fields above
    /// with when it completes.
    pub pending_deployment: Option<PendingDeployment>,
    /// The most recent stack operation, as `Stack$LastOperations` reports it. Recorded by
    /// [`StoredStack::begin_transition`], so every call that opens an operation records
    /// exactly one.
    pub last_operation: Option<StoredOperation>,
    pub creation_time: SystemTime,
    pub last_updated_time: Option<SystemTime>,
    pub deletion_time: Option<SystemTime>,
    pub client_request_token: Option<String>,
    pub events: ::std::collections::VecDeque<StoredEvent>,
    /// `Stack$DriftInformation`'s `StackDriftStatus`: the stack-level result the last
    /// drift detection that COMPLETED against this stack left ([`drift`]).
    /// [`DRIFT_STATUS_NOT_CHECKED`] until one has, since "NOT_CHECKED: CloudFormation
    /// hasn't checked if the stack differs from its expected template configuration".
    pub drift_status: String,
    /// `StackDriftInformation$LastCheckTimestamp`: "most recent time when a drift
    /// detection operation was INITIATED on the stack", so it is set by the call that
    /// starts one rather than by the result landing.
    pub drift_last_check: Option<SystemTime>,
}

impl StoredStack {
    /// A stack as `CreateStack` first stores it: the identity and the template, with every
    /// optional member at its default. The caller fills in what the request supplied and
    /// then opens the create transition.
    pub fn new(
        stack_id: String,
        stack_name: String,
        template_body: Option<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            stack_id,
            stack_name,
            region: DEFAULT_REGION.to_string(),
            template_description: template_body.as_deref().and_then(template::description),
            template_body,
            change_set_id: None,
            parameters: Vec::new(),
            capabilities: Vec::new(),
            tags: Vec::new(),
            notification_arns: Vec::new(),
            role_arn: None,
            timeout_in_minutes: None,
            disable_rollback: false,
            enable_termination_protection: false,
            retain_except_on_create: None,
            deployment_config: None,
            rollback_configuration: None,
            deletion_mode: None,
            stack_policy_body: None,
            resource_types: Vec::new(),
            status: status::CREATE_IN_PROGRESS.to_string(),
            status_reason: None,
            pending: None,
            resources: Vec::new(),
            outputs: Vec::new(),
            pending_deployment: None,
            last_operation: None,
            creation_time: now,
            last_updated_time: None,
            deletion_time: None,
            client_request_token: None,
            events: ::std::collections::VecDeque::new(),
            drift_status: DRIFT_STATUS_NOT_CHECKED.to_string(),
            drift_last_check: None,
        }
    }

    /// Replace the stored template body, re-deriving the description it carries.
    pub fn set_template_body(&mut self, template_body: String) {
        self.template_description = template::description(&template_body);
        self.template_body = Some(template_body);
    }

    /// Materialise what the stored template declares: one record per declared resource,
    /// and the stack's outputs evaluated against those resources, the stack's parameters
    /// and the pseudo parameters.
    ///
    /// Called by the operation that DEPLOYS the template (`CreateStack`, `UpdateStack`),
    /// never by a read: the values are a function of the template body and the
    /// parameters in force, so they are computed once where those change and read back
    /// unchanged afterwards.
    ///
    /// The result is held as the operation's PENDING deployment and takes effect when
    /// [`Self::settle`] completes that operation — a stack reports the resources and
    /// outputs of the template that finished deploying, so a caller reading a stack whose
    /// create is still in progress sees none, and one reading mid-update still sees the
    /// values in force rather than the ones the update is about to install. A template
    /// this service cannot read declares nothing, and leaves the stack with no resources
    /// and no outputs rather than with the previous template's.
    ///
    /// What this computes is what the resources ARE; CREATING them in the services that
    /// own them is [`provisioning::reconcile`](provisioning::reconcile), which the
    /// deploying operation calls with these records once the store is unlocked.
    pub fn deploy_template(&mut self) {
        let Some(declared) = self
            .template_body
            .as_deref()
            .and_then(template::parse)
        else {
            // No template at all (a stack a CREATE change set created and nothing has
            // executed yet), or one this service will not read: either way the stack is
            // left with no resources and no outputs rather than with invented ones.
            self.pending_deployment = Some(PendingDeployment::default());
            return;
        };
        // "Default: A value of the appropriate type for the template to use if no value
        // is specified when a stack is created" — the request's values win over the
        // template's declared defaults.
        let mut parameters = declared.parameter_defaults();
        for parameter in &self.parameters {
            parameters.insert(parameter.key.clone(), parameter.value.clone());
        }
        let stack = template::StackScope {
            region: &self.region,
            stack_name: &self.stack_name,
            stack_id: &self.stack_id,
        };
        // ONE budget for the whole deployment: the resource names and every output are
        // evaluated out of it, so the total work one request can ask for is bounded.
        let mut budget = template::Budget::new();
        let no_resources = BTreeMap::new();
        let parameter_types = declared.parameter_types();
        // The deployment's scope with no resource resolved yet, which is what
        // `provision` turns into the stack's resources.
        let scope = template::Scope {
            stack,
            parameters: &parameters,
            parameter_types: &parameter_types,
            resources: &no_resources,
            mappings: &declared.mappings,
            conditions: &declared.conditions,
        };
        let mut resources = template::provision(&declared, &scope, &mut budget);
        let by_logical_id: BTreeMap<String, StoredResource> = resources
            .iter()
            .map(|resource| (resource.logical_id.clone(), resource.clone()))
            .collect();
        // Everything resolved against the whole set, now that every resource's identity
        // is known.
        let resolved = template::Scope {
            resources: &by_logical_id,
            ..scope
        };
        // A second pass: each resource's properties are evaluated against the whole set,
        // so a property naming another resource (a stream's `RoleARN` from an
        // `Fn::GetAtt`) carries that resource's value. This is the request the service
        // that owns the resource is asked to create it with.
        template::resolve_properties(&declared, &mut resources, &resolved, &mut budget);
        // A resource this stack ALREADY created in the service that owns it, and that this
        // template declares again unchanged, is still that same created resource: the
        // record carries forward both the fact that it exists there and what that service
        // reported about it, so a later deletion still deletes it. Creating what is new
        // and deleting what this deployment replaced is
        // [`provisioning::reconcile`](provisioning::reconcile), which the deploying
        // operation calls with these records.
        let already_created = self.current_resources();
        for resource in &mut resources {
            if let Some(existing) = already_created.iter().find(|existing| {
                existing.materialised && provisioning::is_same_resource(existing, resource)
            }) {
                // Deliberately NOT `provisioning::carry_forward`: that one also carries
                // the properties the resource was created with, and this record's
                // properties are what `reconcile` compares against them to see whether
                // the deployment asks for a DIFFERENT configuration of a resource that
                // already exists. Only the fact that it exists, and what the owning
                // service reported about it, carry forward here.
                resource.materialised = true;
                for (name, value) in &existing.attributes {
                    resource.attributes.insert(name.clone(), value.clone());
                }
            }
        }
        let outputs = template::evaluate_outputs(&declared, &resolved, &mut budget);
        self.pending_deployment = Some(PendingDeployment { resources, outputs });
    }

    /// Whether this stack is still part of the live set — the set addressable by stack
    /// NAME. A stack whose deletion has completed is reachable only by its stack id
    /// (`DescribeStacksInput$StackName`: "Deleted stacks: You must specify the unique
    /// stack ID").
    pub fn is_live(&self) -> bool {
        self.status != status::DELETE_COMPLETE
    }

    /// The stack's `Description`: the template's own `Description`, which is where
    /// CloudFormation takes both `Stack$Description` and
    /// `StackSummary$TemplateDescription` from.
    pub fn description(&self) -> Option<String> {
        self.template_description.clone()
    }

    /// Append a stack-level event. `LogicalResourceId` is the stack's name and
    /// `PhysicalResourceId` its id, as CloudFormation reports for the stack's own
    /// resource.
    /// The log is bounded at [`MAX_EVENTS_PER_STACK`]: past that the OLDEST event is
    /// reclaimed, so one caller cannot grow a single stack's history without limit.
    pub fn record_event(
        &mut self,
        at: SystemTime,
        resource_status: &str,
        resource_status_reason: Option<String>,
        operation_id: Option<String>,
        client_request_token: Option<String>,
    ) {
        // Oldest-first eviction off the FRONT of a deque, so the bound costs O(1) rather
        // than a shift of the whole log.
        while self.events.len() >= MAX_EVENTS_PER_STACK {
            self.events.pop_front();
        }
        self.events.push_back(StoredEvent {
            event_id: new_identifier(&self.stack_id),
            stack_id: self.stack_id.clone(),
            stack_name: self.stack_name.clone(),
            operation_id,
            logical_resource_id: self.stack_name.clone(),
            physical_resource_id: Some(self.stack_id.clone()),
            resource_type: STACK_RESOURCE_TYPE.to_string(),
            timestamp: at,
            resource_status: resource_status.to_string(),
            resource_status_reason,
            client_request_token,
        });
    }

    /// Record the operation now acting on this stack, which is the one
    /// `Stack$LastOperations` reports until the next one starts.
    pub fn record_operation(&mut self, operation_type: &str, operation_id: &str) {
        self.last_operation = Some(StoredOperation {
            operation_type: operation_type.to_string(),
            operation_id: operation_id.to_string(),
        });
    }

    /// Materialise the status transition the clock has already made due, if any.
    ///
    /// Only ever applies a transition whose due instant has passed, so it reports what
    /// was owed at `now` even if nobody had called — it never advances a stack because
    /// it was read.
    pub fn settle(&mut self, now: SystemTime) {
        if self.pending.as_ref().is_none_or(|p| p.due_at > now) {
            return;
        }
        let pending = self.pending.take().expect("checked just above");
        self.status = pending.status;
        self.status_reason = pending.status_reason.clone();
        if self.status == status::DELETE_COMPLETE {
            self.deletion_time = Some(pending.due_at);
            // The deletion completed, so every resource of the stack was deleted as of
            // that instant — reported, not dropped: "For deleted stacks,
            // DescribeStackResources returns resource information for up to 90 days after
            // the stack has been deleted." The output VALUES go, since they were
            // evaluated against resources that no longer exist.
            for deployed in &mut self.resources {
                deployed.status = resource_status::DELETE_COMPLETE.to_string();
                deployed.status_timestamp = pending.due_at;
            }
            self.outputs.clear();
            self.pending_deployment = None;
        } else if let Some(deployed) = self.pending_deployment.take() {
            // The create or update completed: the template it deployed is what the stack
            // reports resources and outputs for from here on.
            self.resources = installed_resources(&self.resources, deployed.resources, pending.due_at);
            self.outputs = deployed.outputs;
        }
        let status = self.status.clone();
        let reason = pending.status_reason;
        self.record_event(
            pending.due_at,
            &status,
            reason,
            pending.operation_id,
            pending.client_request_token,
        );
    }

    /// Start a stack operation: move the stack to its `*_IN_PROGRESS` status, record the
    /// opening event, and make the terminal status due one [`STATUS_TRANSITION_DELAY`]
    /// from now.
    pub fn begin_transition(
        &mut self,
        now: SystemTime,
        in_progress: &str,
        settles_to: &str,
        operation_id: Option<String>,
        client_request_token: Option<String>,
    ) {
        self.status = in_progress.to_string();
        self.status_reason = Some(USER_INITIATED.to_string());
        // `Stack$LastOperations`: "Information about the most recent operations performed
        // on this stack" — this call is that operation, and the type is the one the
        // status it opens names.
        if let (Some(operation_type), Some(id)) =
            (operation_type_of(in_progress), operation_id.clone())
        {
            self.last_operation = Some(StoredOperation {
                operation_id: id,
                operation_type: operation_type.to_string(),
            });
        }
        self.record_event(
            now,
            in_progress,
            Some(USER_INITIATED.to_string()),
            operation_id.clone(),
            client_request_token.clone(),
        );
        self.pending = Some(PendingStatus {
            status: settles_to.to_string(),
            status_reason: None,
            due_at: now + STATUS_TRANSITION_DELAY,
            operation_id,
            client_request_token,
        });
    }

    /// The resources the stack has RIGHT NOW: the ones the operation in flight is about to
    /// install while it is still in flight, else the ones its last completed operation
    /// left. This is the set a stack's deletion has to delete.
    pub fn current_resources(&self) -> Vec<StoredResource> {
        match &self.pending_deployment {
            Some(deployment) => deployment.resources.clone(),
            None => self
                .resources
                .iter()
                .map(|deployed| deployed.resource.clone())
                .collect(),
        }
    }

    /// Replace the resources of [`Self::current_resources`] with what the services that
    /// own them answered — the attributes they reported, and whether each one was really
    /// created there.
    ///
    /// For an operation still in flight that is the pending deployment, which
    /// [`Self::settle`] installs when the operation completes. For one that has already
    /// settled (a deletion acting on a stack whose create completed), the installed
    /// records are rewritten in place, each keeping the resource STATUS and timestamp its
    /// deployment gave it: nothing about a resource's status changed because its
    /// provisioning record did.
    pub fn record_materialised_resources(&mut self, resources: Vec<StoredResource>) {
        if let Some(deployment) = &mut self.pending_deployment {
            deployment.resources = resources;
            return;
        }
        for resource in resources {
            match self
                .resources
                .iter_mut()
                .find(|deployed| deployed.resource.logical_id == resource.logical_id)
            {
                Some(deployed) => deployed.resource = resource,
                // A resource the stack did not have installed under that logical id: it
                // takes the status of the operation that recorded it.
                None => self.resources.push(DeployedResource::new(
                    resource,
                    resource_status::CREATE_COMPLETE,
                    SystemTime::now(),
                )),
            }
        }
    }

    /// Record that the operation in flight could not be carried out: the stack takes the
    /// failed status with the reason, the clock is owed nothing more, and the event log
    /// carries the failure.
    ///
    /// This is how CloudFormation reports a stack whose resources an owning service
    /// refused to create or delete (`CREATE_FAILED` / `DELETE_FAILED`) — the operation
    /// does not stay pending and no terminal status it never reached is reported.
    pub fn fail_transition(&mut self, now: SystemTime, failed_status: &str, reason: String) {
        let pending = self.pending.take();
        self.status = failed_status.to_string();
        self.status_reason = Some(reason.clone());
        self.record_event(
            now,
            failed_status,
            Some(reason),
            pending.as_ref().and_then(|p| p.operation_id.clone()),
            pending.and_then(|p| p.client_request_token),
        );
    }
}

/// The resources a completed create or update leaves the stack with, each carrying the
/// resource-level status that operation left it in.
///
/// `StackResource$ResourceStatus` is the resource's "Current status" and
/// `StackResource$Timestamp` the "Time the status was updated", so a resource the
/// operation materialised for the first time reports the create's completion, one it
/// changed reports that completion as an update, and one it left exactly as it found it
/// keeps the status and the instant it already carried: nothing about that resource's
/// status was updated, so its timestamp must not move.
fn installed_resources(
    previous: &[DeployedResource],
    deployed: Vec<StoredResource>,
    at: SystemTime,
) -> Vec<DeployedResource> {
    deployed
        .into_iter()
        .map(|resource| {
            match previous
                .iter()
                .find(|before| before.resource.logical_id == resource.logical_id)
            {
                // Deployed before and identical: this operation did not touch it.
                Some(before) if before.resource == resource => before.clone(),
                // Deployed before under this logical id and now different: the operation
                // updated it.
                Some(_) => DeployedResource::new(resource, resource_status::UPDATE_COMPLETE, at),
                // Not deployed before: the operation created it.
                None => DeployedResource::new(resource, resource_status::CREATE_COMPLETE, at),
            }
        })
        .collect()
}

/// One resource-level change a change set would make, computed once when the change set
/// is created by comparing the submitted template with the stack's current one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredResourceChange {
    /// The model's `ChangeAction`: `Add`, `Modify` or `Remove`.
    pub action: String,
    pub logical_resource_id: String,
    pub resource_type: String,
    /// The physical id of the resource the change acts on, for a change to one the stack
    /// has already deployed (a `Modify` or a `Remove`). `None` for an `Add`: the resource
    /// does not exist yet, so it has no physical id to report.
    pub physical_resource_id: Option<String>,
    /// For a `Modify`, the resource attributes that differ, spelled as the model's
    /// `ResourceAttribute` enum spells them and in that enum's order. Empty for an `Add`
    /// or a `Remove`, which change the whole resource rather than an attribute of it.
    pub scope: Vec<String>,
}

/// The change-set statuses a change set's creation is headed for, and the instant they
/// are due.
///
/// `CreateChangeSet` answers while CloudFormation is still computing the change set ("After
/// the CreateChangeSet call successfully completes, CloudFormation starts creating the
/// change set"), so the terminal `Status`/`ExecutionStatus` pair is recorded as due a
/// moment later and materialised by [`StoredChangeSet::settle`].
#[derive(Debug, Clone)]
pub struct PendingChangeSetStatus {
    /// The `ChangeSetStatus` that becomes due, if this transition moves it.
    pub status: Option<String>,
    /// The `StatusReason` that comes with that status (the reason a creation that would
    /// change nothing failed, say).
    pub status_reason: Option<String>,
    /// The `ExecutionStatus` that becomes due, if this transition moves it.
    pub execution_status: Option<String>,
    pub due_at: SystemTime,
}

/// One change set, as this emulator persists it.
///
/// The change set owns the template and the configuration the request submitted;
/// `ExecuteChangeSet` is what copies them onto the stack. Nothing here is derived on a
/// read: the change list is computed once, when the change set is created.
#[derive(Debug, Clone)]
pub struct StoredChangeSet {
    pub change_set_id: String,
    pub change_set_name: String,
    /// The stack this change set belongs to, by id: a change set outlives its stack's
    /// name changes and is never re-keyed.
    pub stack_id: String,
    pub stack_name: String,
    /// `ChangeSetType`: `CREATE` for a change set that creates its stack, `UPDATE` for one
    /// that updates an existing stack.
    pub change_set_type: String,
    pub description: Option<String>,
    /// The template the change set would put on the stack.
    pub template_body: String,
    pub parameters: Vec<StoredParameter>,
    pub capabilities: Vec<String>,
    pub tags: Vec<StoredTag>,
    pub notification_arns: Vec<String>,
    pub role_arn: Option<String>,
    pub rollback_configuration: Option<StoredRollbackConfiguration>,
    pub deployment_config: Option<StoredDeploymentConfig>,
    pub deployment_mode: Option<String>,
    pub resource_types: Vec<String>,
    pub on_stack_failure: Option<String>,
    pub include_nested_stacks: bool,
    pub import_existing_resources: bool,
    pub client_token: Option<String>,
    pub creation_time: SystemTime,
    /// The model's `ChangeSetStatus`.
    pub status: String,
    pub status_reason: Option<String>,
    /// The model's `ExecutionStatus`.
    pub execution_status: String,
    /// `DescribeChangeSetOutput$StackDriftStatus`: "the drift status of the stack WHEN THE
    /// CHANGE SET WAS CREATED" — so it is captured off the stack here, at creation, and
    /// never re-read later.
    pub stack_drift_status: String,
    pub pending: Option<PendingChangeSetStatus>,
    pub changes: Vec<StoredResourceChange>,
}

impl StoredChangeSet {
    /// Materialise the change-set transition the clock has already made due, if any.
    ///
    /// Like [`StoredStack::settle`], it only ever applies a transition whose due instant
    /// has passed: it reports what was owed at `now` even if nobody had called.
    pub fn settle(&mut self, now: SystemTime) {
        if self.pending.as_ref().is_none_or(|p| p.due_at > now) {
            return;
        }
        let pending = self.pending.take().expect("checked just above");
        if let Some(status) = pending.status {
            self.status = status;
            self.status_reason = pending.status_reason;
        }
        if let Some(execution_status) = pending.execution_status {
            self.execution_status = execution_status;
        }
    }

    /// Whether `ExecuteChangeSet` can apply this change set: CloudFormation executes a
    /// change set only once its creation has completed and nothing has executed it yet
    /// (`InvalidChangeSetStatusException`: "the change set status might be
    /// CREATE_IN_PROGRESS").
    pub fn is_executable(&self) -> bool {
        self.status == change_set_status::CREATE_COMPLETE
            && self.execution_status == execution_status::AVAILABLE
    }
}

/// One drift-detection operation on one stack: what `DetectStackDrift` started and
/// `DescribeStackDriftDetectionStatus` reads back.
///
/// The detection's own outcome is asynchronous ("DetectStackDrift can take up to several
/// minutes"), so a record is answered `DETECTION_IN_PROGRESS` and carries the outcome the
/// clock owes in [`Self::pending`]; [`ComAmazonawsCloudformationCloudformationState::settle`]
/// is what materialises it, onto this record and onto the stack and resources it describes.
#[derive(Debug, Clone)]
pub struct StoredDriftDetection {
    /// `StackDriftDetectionId`, the id the call handed back.
    pub detection_id: String,
    /// The stack the detection ran against, by stack id: "the ID of the stack".
    pub stack_id: String,
    /// `Timestamp`: "time at which the stack drift detection operation was initiated".
    pub initiated_at: SystemTime,
    /// [`drift_detection_status`].
    pub detection_status: String,
    pub detection_status_reason: Option<String>,
    /// The stack-level `StackDriftStatus` the detection found, once it has completed.
    /// `None` while it is in progress: "this value will be NULL for any stack drift
    /// detection operation that has not yet concluded".
    pub stack_drift_status: Option<String>,
    /// `DriftedStackResourceCount`: "this is NULL until the drift detection operation
    /// reaches a status of DETECTION_COMPLETE".
    pub drifted_resource_count: Option<i32>,
    /// The outcome computed for this detection, and the instant reporting it is due.
    pub pending: Option<PendingDriftOutcome>,
}

/// What a drift detection found, held until the clock makes reporting the detection's own
/// conclusion due.
#[derive(Debug, Clone)]
pub struct PendingDriftOutcome {
    /// [`drift_detection_status`]: `DETECTION_COMPLETE`, or `DETECTION_FAILED` where an
    /// owning service's refusal left a resource unchecked.
    pub detection_status: String,
    pub detection_status_reason: Option<String>,
    pub stack_drift_status: String,
    pub drifted_resource_count: i32,
    pub due_at: SystemTime,
}

/// Per-service state for com-amazonaws-cloudformation-cloudformation. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCloudformationCloudformationState {
    /// Every stack, in creation order, so `DescribeStacks` and `ListStacks` are
    /// deterministic. Includes deleted stacks (see this module's docs).
    pub stacks: Vec<StoredStack>,
    /// Every change set of every stack, in creation order, so `DescribeChangeSet` reads
    /// back exactly what `CreateChangeSet` wrote. A change set lives as long as its stack
    /// does: CloudFormation deletes a stack's change sets with the stack, which is what
    /// [`Self::settle`] enforces here.
    pub change_sets: Vec<StoredChangeSet>,
    /// This account and Region's extension registry, in the order its records entered it,
    /// so `DescribeType` reads back exactly what a registry write wrote (see
    /// [`registry`]). A registry record has nothing to do with a stack's lifecycle, so it
    /// is neither settled nor reclaimed with one.
    pub extensions: Vec<registry::StoredExtension>,
    /// Every drift-detection operation this service has run, in the order they were
    /// started, so `DescribeStackDriftDetectionStatus` reads back the result of the id
    /// `DetectStackDrift` handed out. Bounded at [`MAX_DRIFT_DETECTIONS`], oldest
    /// reclaimed first.
    pub drift_detections: Vec<StoredDriftDetection>,
}

impl ComAmazonawsCloudformationCloudformationState {
    /// Materialise every status transition the clock has made due, then enforce the
    /// retention bound on completed deletions. Called first by every operation, so no
    /// caller can observe a stack that should have settled already, or one the retention
    /// policy had already reclaimed.
    pub fn settle(&mut self, now: SystemTime) {
        for stack in &mut self.stacks {
            stack.settle(now);
        }
        for change_set in &mut self.change_sets {
            change_set.settle(now);
        }
        self.settle_drift_detections(now);
        self.reclaim_retired_stacks();
        self.reclaim_orphaned_change_sets();
    }

    /// Materialise every drift-detection STATUS whose due instant has passed.
    ///
    /// What the clock owes here is the detection operation's own status: AWS answers
    /// `DetectStackDrift` while the detection is still running
    /// ("DetectStackDrift can take up to several minutes"), so
    /// `DescribeStackDriftDetectionStatus` answers `DETECTION_IN_PROGRESS` with no result
    /// until this makes the conclusion due — and only ever an outcome already computed, so
    /// nothing is advanced because it was polled.
    ///
    /// The RESULT itself is not owed by the clock: the comparison against each owning
    /// service is what `DetectStackDrift` performs, and
    /// [`Self::apply_drift_result`] records what it found on the stack and its resources
    /// as of that check.
    fn settle_drift_detections(&mut self, now: SystemTime) {
        for detection in &mut self.drift_detections {
            if detection
                .pending
                .as_ref()
                .is_none_or(|pending| pending.due_at > now)
            {
                continue;
            }
            let pending = detection.pending.take().expect("checked just above");
            detection.detection_status = pending.detection_status;
            detection.detection_status_reason = pending.detection_status_reason;
            detection.stack_drift_status = Some(pending.stack_drift_status);
            detection.drifted_resource_count = Some(pending.drifted_resource_count);
        }
    }

    /// Record what a drift detection FOUND on the stack it ran against and on each
    /// resource it covered.
    ///
    /// `Stack$DriftInformation` is a statement about the stack as of its last check —
    /// "status of the stack's actual configuration compared to its expected template
    /// configuration" — not the status of a detection operation, so it carries the result
    /// of the check that ran rather than waiting on the detection record's own conclusion.
    /// `checked_at` is the instant the detection was initiated, which is what both
    /// `LastCheckTimestamp` members report.
    ///
    /// A resource of the stack the detection did NOT cover keeps the result it already
    /// carried: a filtered detection reports on what it checked and says nothing about the
    /// rest.
    pub fn apply_drift_result(
        &mut self,
        stack_id: &str,
        checked_at: SystemTime,
        stack_drift_status: &str,
        results: &[drift::ResourceDrift],
    ) {
        let Some(stack) = self
            .stacks
            .iter_mut()
            .find(|stack| stack.stack_id == stack_id)
        else {
            // The stack was deleted while its detection was running: there is nothing left
            // to report the result on, and the detection record keeps it.
            return;
        };
        stack.drift_status = stack_drift_status.to_string();
        stack.drift_last_check = Some(checked_at);
        for result in results {
            if let Some(deployed) = stack
                .resources
                .iter_mut()
                .find(|deployed| deployed.resource.logical_id == result.logical_id)
            {
                deployed.drift_status = result.status.clone();
                deployed.drift_checked_at = Some(checked_at);
            }
        }
    }

    /// Record a drift detection that has just been started, and hand back nothing: the
    /// caller already minted the id it answers with. The oldest record is reclaimed past
    /// [`MAX_DRIFT_DETECTIONS`], so a caller cannot grow this store without limit.
    pub fn record_drift_detection(&mut self, detection: StoredDriftDetection) {
        while self.drift_detections.len() >= MAX_DRIFT_DETECTIONS {
            self.drift_detections.remove(0);
        }
        self.drift_detections.push(detection);
    }

    /// The drift detection with this `StackDriftDetectionId`, if this store still holds it.
    pub fn drift_detection(&self, detection_id: &str) -> Option<&StoredDriftDetection> {
        self.drift_detections
            .iter()
            .find(|detection| detection.detection_id == detection_id)
    }

    /// The same record, for the call that fills in the outcome it computed.
    pub fn drift_detection_mut(
        &mut self,
        detection_id: &str,
    ) -> Option<&mut StoredDriftDetection> {
        self.drift_detections
            .iter_mut()
            .find(|detection| detection.detection_id == detection_id)
    }

    /// Drop the change sets whose stack is no longer live.
    ///
    /// Not a mutation a caller asked for: a change set describes changes to a stack, so it
    /// is gone the moment that stack's deletion completes — which was owed at the same
    /// instant whether or not anybody called. It is also what keeps the change-set store
    /// from outliving the stacks it counts against.
    fn reclaim_orphaned_change_sets(&mut self) {
        if self.change_sets.is_empty() {
            return;
        }
        // Disjoint fields: the live-id set borrows `stacks`, the retain mutates
        // `change_sets`.
        let live: ::std::collections::HashSet<&str> = self
            .stacks
            .iter()
            .filter(|stack| stack.is_live())
            .map(|stack| stack.stack_id.as_str())
            .collect();
        self.change_sets
            .retain(|change_set| live.contains(change_set.stack_id.as_str()));
    }

    /// The change sets of one stack, in creation order: what `ListChangeSets` reports and
    /// what [`MAX_CHANGE_SETS_PER_STACK`] bounds.
    pub fn change_sets_of<'a>(
        &'a self,
        stack_id: &'a str,
    ) -> impl Iterator<Item = &'a StoredChangeSet> {
        self.change_sets
            .iter()
            .filter(move |change_set| change_set.stack_id == stack_id)
    }

    /// Resolve the `ChangeSetName` input member, which CloudFormation accepts as either a
    /// change set name or its ARN: an ARN names the change set on its own, while a name is
    /// resolved among the change sets of the stack the request names.
    pub fn resolve_change_set(
        &self,
        name_or_id: &str,
        stack_name_or_id: Option<&str>,
    ) -> Option<&StoredChangeSet> {
        self.change_set_position(name_or_id, stack_name_or_id)
            .map(|index| &self.change_sets[index])
    }

    /// [`Self::resolve_change_set`], for a caller that mutates the change set it resolved.
    pub fn resolve_change_set_mut(
        &mut self,
        name_or_id: &str,
        stack_name_or_id: Option<&str>,
    ) -> Option<&mut StoredChangeSet> {
        self.change_set_position(name_or_id, stack_name_or_id)
            .map(|index| &mut self.change_sets[index])
    }

    fn change_set_position(
        &self,
        name_or_id: &str,
        stack_name_or_id: Option<&str>,
    ) -> Option<usize> {
        // The change set's own ARN identifies it; a stack named beside it must be the
        // stack it belongs to, or the pair names nothing.
        if let Some(index) = self
            .change_sets
            .iter()
            .position(|change_set| change_set.change_set_id == name_or_id)
        {
            let change_set = &self.change_sets[index];
            let stack_matches = stack_name_or_id.is_none_or(|stack| {
                stack == change_set.stack_id || stack == change_set.stack_name
            });
            return stack_matches.then_some(index);
        }
        // A bare name is resolved within one stack ("If you specify the name of a change
        // set, you must also specify the stack name").
        let stack_name_or_id = stack_name_or_id?;
        let stack_id = self.resolve(stack_name_or_id)?.stack_id.clone();
        self.change_sets.iter().position(|change_set| {
            change_set.stack_id == stack_id && change_set.change_set_name == name_or_id
        })
    }

    /// Drop the oldest completed deletions past [`MAX_DELETED_STACKS_RETAINED`].
    ///
    /// Retention, not a mutation a caller asked for: a deleted stack is over the bound the
    /// moment the deletion that pushed the count past it completed, whether or not anybody
    /// called. Live stacks are never touched.
    fn reclaim_retired_stacks(&mut self) {
        // Early-out before allocating anything: all but the rare call that completes a
        // deletion past the bound stops here.
        let retired_count = self.stacks.iter().filter(|stack| !stack.is_live()).count();
        if retired_count <= MAX_DELETED_STACKS_RETAINED {
            return;
        }
        let mut retired: Vec<(SystemTime, usize)> = self
            .stacks
            .iter()
            .enumerate()
            .filter(|(_, stack)| !stack.is_live())
            .map(|(index, stack)| (stack.deletion_time.unwrap_or(stack.creation_time), index))
            .collect();
        // Oldest deletion first, so the rows reclaimed are the ones furthest past the
        // retention bound.
        retired.sort_by_key(|(deleted_at, index)| (*deleted_at, *index));
        let over = retired.len() - MAX_DELETED_STACKS_RETAINED;
        let reclaimed: ::std::collections::HashSet<usize> =
            retired.into_iter().take(over).map(|(_, index)| index).collect();
        // Rebuilt by index rather than driven by a counter beside `retain`, so nothing
        // depends on the order `retain` happens to visit elements in.
        self.stacks = ::std::mem::take(&mut self.stacks)
            .into_iter()
            .enumerate()
            .filter(|(index, _)| !reclaimed.contains(index))
            .map(|(_, stack)| stack)
            .collect();
    }

    /// Resolve the `StackName` input member, which CloudFormation accepts as either a
    /// stack name or a stack id: a stack id matches any stack, live or deleted, while a
    /// name matches only a live one.
    pub fn resolve(&self, name_or_id: &str) -> Option<&StoredStack> {
        self.position_of(name_or_id).map(|i| &self.stacks[i])
    }

    /// [`Self::resolve`], for a caller that mutates the stack it resolved.
    pub fn resolve_mut(&mut self, name_or_id: &str) -> Option<&mut StoredStack> {
        self.position_of(name_or_id).map(|i| &mut self.stacks[i])
    }

    /// The live stacks, in creation order: what `DescribeStacks` reports when no
    /// `StackName` is given ("Deleted stacks don't show up in DescribeStacks if the
    /// deletion has been completed successfully").
    pub fn live_stacks(&self) -> impl Iterator<Item = &StoredStack> {
        self.stacks.iter().filter(|stack| stack.is_live())
    }

    /// The stack a resource with this physical id belongs to, which is what
    /// `DescribeStackResources` resolves its `PhysicalResourceId` member to ("You can pass
    /// the EC2 InstanceId to DescribeStackResources to find which stack the instance
    /// belongs to and what other resources are part of the stack").
    ///
    /// Searched over every stack, live or deleted, in creation order: a deleted stack's
    /// resources stay addressable for as long as the stack does. The first stack carrying
    /// the id wins, and this emulator derives a generated physical id from the stack id, so
    /// two stacks collide only where their templates NAME the same resource.
    pub fn stack_of_physical_resource(&self, physical_resource_id: &str) -> Option<&StoredStack> {
        self.stacks.iter().find(|stack| {
            stack
                .resources
                .iter()
                .any(|deployed| deployed.resource.physical_resource_id == physical_resource_id)
        })
    }

    /// The registry record a `Type`-and-`TypeName` request names, in the Region the
    /// request was signed for: `DescribeType` reads an extension "from the CloudFormation
    /// registry in your current account and Region", so the same name in another Region
    /// names no record.
    pub fn extension_by_name(
        &self,
        region: &str,
        type_kind: &str,
        type_name: &str,
    ) -> Option<&registry::StoredExtension> {
        self.extensions.iter().find(|extension| {
            extension.region == region
                && extension.type_kind == type_kind
                && extension.type_name == type_name
        })
    }

    /// The registry record an `Arn` request names, in the Region the request was signed
    /// for. An extension's ARN carries its Region and its kind already, and the registry
    /// is regional like the records it holds, so an ARN minted for another Region names no
    /// record HERE — the same gate [`Self::extension_by_name`] applies to the name form.
    pub fn extension_by_arn(&self, region: &str, arn: &str) -> Option<&registry::StoredExtension> {
        self.extensions
            .iter()
            .find(|extension| extension.region == region && extension.arn == arn)
    }

    fn position_of(&self, name_or_id: &str) -> Option<usize> {
        self.stacks
            .iter()
            .position(|stack| stack.stack_id == name_or_id)
            .or_else(|| {
                self.stacks
                    .iter()
                    .position(|stack| stack.stack_name == name_or_id && stack.is_live())
            })
    }
}

/// Stored outputs as the modelled `Outputs` member.
///
/// A declared output whose value this service could not evaluate carries no
/// `OutputValue`: the member is optional, and an absent value is the truthful answer
/// where an invented identifier would not be.
pub fn output_views(outputs: &[StoredOutput]) -> Option<Vec<crate::ops::types::Output>> {
    non_empty(
        outputs
            .iter()
            .map(|output| crate::ops::types::Output {
                output_key: Some(output.key.clone()),
                output_value: output.value.clone(),
                description: output.description.clone(),
                export_name: output.export_name.clone(),
            })
            .collect(),
    )
}

/// The `ResourceAttribute`s one resource declaration states differently from another, in
/// the order the model's enum lists them.
///
/// Only attributes the model can name are reported, which is what `ResourceChange$Scope`
/// carries: the whole resource declaration is compared, not just its `Properties`.
fn differing_attributes(
    resource: &template::DeclaredResource,
    other: &template::DeclaredResource,
) -> Vec<String> {
    crate::ops::types::RESOURCE_ATTRIBUTE_VALUES
        .iter()
        .filter(|attribute| {
            resource.declaration.get(**attribute) != other.declaration.get(**attribute)
        })
        .map(|attribute| (*attribute).to_string())
        .collect()
}

/// The resources a template body declares, by logical id, for comparing two templates.
///
/// Read by the SAME reader a deployment uses ([`template::parse`], which takes a JSON or a
/// YAML body under this service's template bounds), then SORTED by logical id — and that
/// sort is load-bearing rather than a preference: the parsed object iterates in insertion
/// order when anything in the build enables `serde_json`'s `preserve_order` feature
/// (`vendor/smithy-macros` does) and in sorted order otherwise, so without it one change
/// set would report its changes in two different orders depending on how the workspace was
/// built.
///
/// `None` for a body this service will not read at all; `Some([])` for one that declares no
/// resources — which `CreateChangeSet` refuses, since a change set is a plan over resources.
pub fn template_resources(template_body: &str) -> Option<Vec<template::DeclaredResource>> {
    let mut declared = template::parse(template_body)?.resources;
    declared.sort_by(|left, right| left.logical_id.cmp(&right.logical_id));
    Some(declared)
}

/// The resource-level changes that applying `next` to a stack whose template declares
/// `previous` would make, against the resources that stack has `deployed`.
///
/// A resource only `next` declares is an `Add`, one only `previous` declares is a `Remove`,
/// and one both declare differently is a `Modify` scoped to the attributes that differ —
/// except when its `Type` itself changed, which cannot be modified into place and so is
/// reported as the `Remove` of the old resource and the `Add` of the new one. A change to a
/// resource the stack has already deployed carries that resource's physical id. The order is
/// deterministic: the submitted template's resources (in the order
/// [`template_resources`] yields them, which is by logical id), then the removals in the
/// stack template's.
pub fn computed_changes(
    previous: &[template::DeclaredResource],
    next: &[template::DeclaredResource],
    deployed: &[DeployedResource],
) -> Vec<StoredResourceChange> {
    let physical_id = |logical_id: &str| {
        deployed
            .iter()
            .find(|deployed| deployed.resource.logical_id == logical_id)
            .map(|deployed| deployed.resource.physical_resource_id.clone())
    };
    let change =
        |action: &str, resource: &template::DeclaredResource, scope: Vec<String>| {
            StoredResourceChange {
                action: action.to_string(),
                logical_resource_id: resource.logical_id.clone(),
                resource_type: resource.resource_type.clone(),
                // Only a change to a resource the stack has deployed has one; an addition
                // has nothing deployed under that logical id yet.
                physical_resource_id: (action != change_action::ADD)
                    .then(|| physical_id(&resource.logical_id))
                    .flatten(),
                scope,
            }
        };
    let mut changes = Vec::new();
    for resource in next {
        match previous
            .iter()
            .find(|candidate| candidate.logical_id == resource.logical_id)
        {
            None => changes.push(change(change_action::ADD, resource, Vec::new())),
            Some(before) if before.declaration == resource.declaration => {}
            Some(before) if before.resource_type != resource.resource_type => {
                changes.push(change(change_action::REMOVE, before, Vec::new()));
                changes.push(change(change_action::ADD, resource, Vec::new()));
            }
            Some(before) => changes.push(change(
                change_action::MODIFY,
                resource,
                differing_attributes(resource, before),
            )),
        }
    }
    for resource in previous {
        let still_declared = next
            .iter()
            .any(|candidate| candidate.logical_id == resource.logical_id);
        if !still_declared {
            changes.push(change(change_action::REMOVE, resource, Vec::new()));
        }
    }
    changes
}

/// Resolve the request's `Parameters` against the values the stack already carries.
///
/// A request with no `Parameters` at all leaves the stack's parameters as they are;
/// otherwise the request states the parameter set, and each entry either carries a new
/// value or asks (`UsePreviousValue`) for the one the stack is already using. "If you
/// specify true, do not specify a parameter value" — so a request doing both is refused,
/// and so is one asking for a previous value the stack does not have.
///
/// Shared by `UpdateStack` and `CreateChangeSet`, which resolve the member the same way:
/// a change set carries the parameters the update it describes would apply.
pub fn merged_parameters(
    previous: &[StoredParameter],
    requested: Option<&[crate::ops::types::Parameter]>,
) -> Result<Vec<StoredParameter>, String> {
    let Some(requested) = requested else {
        return Ok(previous.to_vec());
    };
    requested
        .iter()
        .map(|parameter| {
            let key = parameter
                .parameter_key
                .clone()
                .filter(|key| !key.is_empty())
                .ok_or_else(|| {
                    "Parameters: every parameter must have a ParameterKey".to_string()
                })?;
            if parameter.use_previous_value == Some(true) {
                if parameter.parameter_value.is_some() {
                    return Err(format!(
                        "Parameters: [{key}] cannot specify both a ParameterValue and \
                         UsePreviousValue"
                    ));
                }
                let value = previous
                    .iter()
                    .find(|stored| stored.key == key)
                    .map(|stored| stored.value.clone())
                    .ok_or_else(|| {
                        format!("Parameters: [{key}] has no previous value on this stack")
                    })?;
                return Ok(StoredParameter { key, value });
            }
            let value = parameter
                .parameter_value
                .clone()
                .ok_or_else(|| format!("Parameters: [{key}] must have values"))?;
            Ok(StoredParameter { key, value })
        })
        .collect()
}

/// `CreateStackInput$StackName`'s documented rule: "A stack name can contain only
/// alphanumeric characters (case sensitive) and hyphens. It must start with an
/// alphabetical character and can't be longer than 128 characters." The model carries no
/// `@pattern` or `@length` for `StackName`, so the handler is what enforces it.
pub const STACK_NAME_PATTERN: &str = "[a-zA-Z][-a-zA-Z0-9]*";
pub const STACK_NAME_MAX_LEN: usize = 128;

/// `Err(message)` naming the member and the constraint the name fails, in the form the
/// service's own input validation words it.
///
/// Shared by `CreateStack` and `CreateChangeSet`, which both name a stack that does not
/// exist yet and so both have to hold its name to the same rule.
pub fn validate_stack_name(name: &str) -> Result<(), String> {
    if name.len() > STACK_NAME_MAX_LEN {
        return Err(format!(
            "1 validation error detected: Value '{name}' at 'stackName' failed to satisfy \
             constraint: Member must have length less than or equal to {STACK_NAME_MAX_LEN}"
        ));
    }
    let well_formed = name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !well_formed {
        return Err(format!(
            "1 validation error detected: Value '{name}' at 'stackName' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: {STACK_NAME_PATTERN}"
        ));
    }
    Ok(())
}

/// The parameter values in force on a stack: what the request supplied, plus the
/// template's own `Default` for every parameter the template declares and the request did
/// not pass a value for.
///
/// `Parameter$Default` is what CloudFormation uses when a stack operation passes no value
/// for a declared parameter, so the stack reports the parameter either way — the declared
/// defaults first, then any value supplied for a parameter the template does not declare
/// (refusing one of those is template validation, which this service does not do). The
/// declarations come from [`template::parse`], the one reader of a stack's template body,
/// which is also where the documented per-template parameter quota is enforced.
///
/// Linear in the supplied count: the supplied values are indexed once rather than
/// rescanned per declaration, so two caller-controlled counts cannot multiply.
pub fn parameters_in_force(
    template_body: &str,
    supplied: Vec<StoredParameter>,
) -> Vec<StoredParameter> {
    let declared = template::parse(template_body)
        .map(|parsed| parsed.parameter_defaults())
        .unwrap_or_default();
    let supplied_by_key: ::std::collections::HashMap<&str, &StoredParameter> = supplied
        .iter()
        .map(|parameter| (parameter.key.as_str(), parameter))
        .collect();
    let mut in_force: Vec<StoredParameter> = Vec::new();
    let mut keys: ::std::collections::HashSet<String> = ::std::collections::HashSet::new();
    for (key, default) in &declared {
        let value = match supplied_by_key.get(key.as_str()) {
            Some(parameter) => parameter.value.clone(),
            None => default.clone(),
        };
        if keys.insert(key.clone()) {
            in_force.push(StoredParameter {
                key: key.clone(),
                value,
            });
        }
    }
    for parameter in supplied {
        if keys.insert(parameter.key.clone()) {
            in_force.push(parameter);
        }
    }
    in_force
}

/// The region the request was signed for, or [`DEFAULT_REGION`] for an unsigned one.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Mint a fresh identifier in the 8-4-4-4-12 lowercase-hex form CloudFormation's stack
/// ids, event ids and operation ids carry.
///
/// The value is a SHA-256 digest of a per-process counter, the current instant and the
/// caller's seed: the counter alone makes every call distinct, so two ids minted in the
/// same nanosecond still differ, and the digest gives the opaque 128-bit shape a client
/// sees. It is an identifier, never a secret — nothing authenticates with it, and the
/// crate's existing `sha2`/`hex` dependencies are all it needs.
fn new_identifier(seed: &str) -> String {
    use ::sha2::{Digest, Sha256};

    static MINTED: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);
    let counter = MINTED.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    hasher.update(counter.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    let digest = ::hex::encode(hasher.finalize());

    format!(
        "{}-{}-{}-{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..16],
        &digest[16..20],
        &digest[20..32]
    )
}

/// Mint a stack id: `arn:aws:cloudformation:<region>:<account>:stack/<name>/<uuid>`,
/// the form CloudFormation gives a stack. The account segment comes from the
/// emulator-wide [`::emulator_server::emulator_arn`] helper.
pub fn new_stack_id(region: &str, stack_name: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("stack/{stack_name}/{}", new_identifier(stack_name)),
    )
}

/// Mint an id for one stack operation (`CreateStackOutput$OperationId` and the
/// `StackEvent$OperationId` of every event that operation causes).
pub fn new_operation_id() -> String {
    new_identifier("operation")
}

/// Mint an id for one drift-detection operation (`DetectStackDriftOutput`'s
/// `StackDriftDetectionId`, which `DescribeStackDriftDetectionStatus` reads a result back
/// by): the same 8-4-4-4-12 form as every other identifier this service mints, which is 36
/// characters and so satisfies the modelled `StackDriftDetectionId` length (1..36).
pub fn new_drift_detection_id() -> String {
    new_identifier("drift")
}

/// Mint a change set id: `arn:aws:cloudformation:<region>:<account>:changeSet/<name>/<uuid>`,
/// the form CloudFormation gives a change set (`ChangeSetId` is modelled
/// `@pattern("^arn:[-a-zA-Z0-9:/]*$")`).
pub fn new_change_set_id(region: &str, change_set_name: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("changeSet/{change_set_name}/{}", new_identifier(change_set_name)),
    )
}

/// The message CloudFormation answers when a request names a change set that is not
/// addressable: no change set of the named stack carries that name and no change set has
/// that ARN.
pub fn change_set_does_not_exist(name_or_id: &str) -> String {
    format!("ChangeSet [{name_or_id}] does not exist")
}

/// The message CloudFormation answers when a request would act on a stack whose current
/// status does not allow it — an operation still in flight on it, or a stack a `CREATE`
/// change set created that nothing has executed yet.
///
/// Shared by `UpdateStack` and `CreateChangeSet`, which refuse the same states, so the two
/// cannot drift apart.
pub fn stack_cannot_be_updated(stack_id: &str, status: &str) -> String {
    format!("Stack:{stack_id} is in {status} state and can not be updated.")
}

/// `Err(message)` for a request that names a change set by NAME without the stack it
/// belongs to.
///
/// `ChangeSetNameOrId` is "the name or Amazon Resource Name (ARN) of the change set", and
/// every operation that takes one documents the same constraint on its `StackName`: "If you
/// specify the name of a change set, you must also specify the stack name". A bare name
/// with no stack is therefore a bad request, not a change set that does not exist — only an
/// ARN names a change set on its own.
pub fn require_stack_for_change_set_name(
    change_set_name: &str,
    stack_name: Option<&str>,
) -> Result<(), String> {
    let named_by_arn = change_set_name.starts_with("arn:");
    if named_by_arn || stack_name.is_some() {
        return Ok(());
    }
    Err(format!(
        "StackName must be specified if ChangeSetName is not specified as an ARN: \
         [{change_set_name}]"
    ))
}

/// Mint the request id one response carries — the `<RequestId>` element of an awsQuery
/// `<ErrorResponse>` envelope, the counterpart of the `ResponseMetadata/RequestId` a
/// successful response carries.
///
/// Same 8-4-4-4-12 lowercase-hex form and the same per-response freshness as every other
/// identifier this service mints: it names ONE request for correlation, so two responses
/// must be distinguishable, and it is not a secret (nothing authenticates with it).
pub fn new_request_id() -> String {
    new_identifier("request")
}

/// The message CloudFormation answers when a request names a stack that is not
/// addressable: no live stack has that name and no stack has that id.
pub fn stack_does_not_exist(name_or_id: &str) -> String {
    format!("Stack with id {name_or_id} does not exist")
}

/// The message answered when a request names a `LogicalResourceId` the stack it selected
/// has not deployed: the stack is addressable, so what does not exist is the resource
/// under that logical id in that stack. Built from what the request named, the way
/// [`stack_does_not_exist`] is.
pub fn resource_does_not_exist(logical_resource_id: &str, stack_name: &str) -> String {
    format!("Resource {logical_resource_id} does not exist for stack {stack_name}")
}

/// The message answered when a request names an extension the registry of this account and
/// Region does not hold — an unknown name or ARN, a name registered under another kind or
/// in another Region, a publisher that published no such extension, or a version the
/// record does not carry. Built from what the request named, the way
/// [`stack_does_not_exist`] is.
pub fn extension_does_not_exist(identifier: &str) -> String {
    format!("Extension with identifier {identifier} does not exist")
}

/// The message answered when a request names a `StackDriftDetectionId` this account has
/// not run a drift detection under — an id nothing issued, or one whose result this store
/// no longer retains ("the number of drift results CloudFormation retains for any given
/// stack, and for how long, may vary"). Built from what the request named, the way
/// [`stack_does_not_exist`] is.
pub fn drift_detection_does_not_exist(detection_id: &str) -> String {
    format!("Drift detection with id {detection_id} does not exist")
}

/// The message CloudFormation answers when a request names a `PhysicalResourceId` no
/// stack has deployed: such a request names its stack only through that resource, so what
/// is not addressable is the stack the resource would belong to.
pub fn stack_for_physical_resource_does_not_exist(physical_resource_id: &str) -> String {
    format!("Stack for {physical_resource_id} does not exist")
}

/// `Some(list)` unless the list is empty, which the wire shapes spell as an absent
/// member rather than an empty one.
pub fn non_empty<T>(list: Vec<T>) -> Option<Vec<T>> {
    (!list.is_empty()).then_some(list)
}

/// Which paginated read a cursor belongs to. A token one operation issued must not
/// resume a page of another operation's collection, so the cursor carries the collection
/// it counts into and a token from elsewhere is refused rather than silently applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageOf {
    /// `DescribeStacks`'s `Stacks`.
    Stacks,
    /// `ListStacks`'s `StackSummaries`.
    StackSummaries,
    /// `DescribeStackEvents`'s `StackEvents`.
    StackEvents,
    /// `ListStackResources`'s `StackResourceSummaries`.
    StackResourceSummaries,
    /// `DescribeChangeSet`'s `Changes`.
    Changes,
}

impl PageOf {
    /// The discriminant the cursor carries.
    fn tag(self) -> &'static str {
        match self {
            Self::Stacks => "stacks",
            Self::StackSummaries => "summaries",
            Self::StackEvents => "events",
            Self::StackResourceSummaries => "resources",
            Self::Changes => "changes",
        }
    }
}

/// Encode a page cursor as an opaque, self-describing `NextToken`.
///
/// Hex of the JSON cursor: it satisfies the modelled `NextToken` length (1..1024) and
/// carries the whole cursor, so paging needs no server-side token table — a reporting
/// operation must not write to state just because it was read.
pub fn encode_next_token(page_of: PageOf, offset: usize) -> String {
    ::hex::encode(::serde_json::json!({ "c": page_of.tag(), "o": offset }).to_string())
}

/// The offset a `NextToken` resumes at, or `None` when the token is not one this service
/// issued for THIS collection.
pub fn decode_next_token(page_of: PageOf, token: &str) -> Option<usize> {
    let bytes = ::hex::decode(token).ok()?;
    let cursor: ::serde_json::Value = ::serde_json::from_slice(&bytes).ok()?;
    if cursor.get("c")?.as_str()? != page_of.tag() {
        return None;
    }
    usize::try_from(cursor.get("o")?.as_u64()?).ok()
}

/// One page of a paginated read: the items from the request's cursor on, at most
/// [`PAGE_SIZE`] of them, plus the `NextToken` that resumes just past them (`None` on the
/// last page).
///
/// `Err(message)` for a token this service never issued for this collection — a bad
/// request, not an empty page.
pub fn paginate<T>(
    page_of: PageOf,
    items: Vec<T>,
    token: Option<&str>,
) -> Result<(Vec<T>, Option<String>), String> {
    let offset = match token {
        None => 0,
        Some(token) => decode_next_token(page_of, token)
            .filter(|offset| *offset <= items.len())
            .ok_or_else(|| format!("Invalid NextToken: {token}"))?,
    };
    let remaining = items.len() - offset;
    let next_token =
        (remaining > PAGE_SIZE).then(|| encode_next_token(page_of, offset + PAGE_SIZE));
    let page = items.into_iter().skip(offset).take(PAGE_SIZE).collect();
    Ok((page, next_token))
}

/// A stored instant as the modelled timestamp type.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// The request's `Tags` as stored tags.
pub fn stored_tags(tags: &[crate::ops::types::Tag]) -> Vec<StoredTag> {
    tags.iter()
        .map(|tag| StoredTag {
            key: tag.key.clone(),
            value: tag.value.clone(),
        })
        .collect()
}

/// Stored tags as the modelled `Tags` member.
///
/// CloudFormation reports `Tags` on every stack, as an empty element for a stack that
/// carries none, so this is always present: a client reading the stack sees the same
/// shape whether or not the stack was tagged.
pub fn tag_views(tags: &[StoredTag]) -> Option<Vec<crate::ops::types::Tag>> {
    Some(
        tags.iter()
            .map(|tag| crate::ops::types::Tag {
                key: tag.key.clone(),
                value: tag.value.clone(),
            })
            .collect(),
    )
}

/// The stack's recorded operation as the modelled `LastOperations` member, always
/// present: "Information about the most recent operations performed on this stack", which
/// for this emulator is the one operation it last opened on the stack.
pub fn last_operation_views(
    last_operation: Option<&StoredOperation>,
) -> Option<Vec<crate::ops::types::OperationEntry>> {
    Some(
        last_operation
            .map(|operation| crate::ops::types::OperationEntry {
                operation_type: Some(operation.operation_type.clone()),
                operation_id: Some(operation.operation_id.clone()),
            })
            .into_iter()
            .collect(),
    )
}

/// Stored parameters as the modelled `Parameters` member. `UsePreviousValue` is a
/// request-time instruction, so it is never reported back; `ResolvedValue` belongs to
/// SSM-typed parameters, which this emulator does not resolve.
pub fn parameter_views(
    parameters: &[StoredParameter],
) -> Option<Vec<crate::ops::types::Parameter>> {
    non_empty(
        parameters
            .iter()
            .map(|parameter| crate::ops::types::Parameter {
                parameter_key: Some(parameter.key.clone()),
                parameter_value: Some(parameter.value.clone()),
                use_previous_value: None,
                resolved_value: None,
            })
            .collect(),
    )
}

/// The deployment mode a stack operation runs in when the request names none.
///
/// `DeploymentConfig$Mode`: "STANDARD - Use the standard deployment behavior, ensuring
/// resources are ready to serve traffic before completing the operation. This is the
/// default. You do not need to specify this value explicitly." The operation therefore ran
/// in this mode whether or not the request spelled it, which is why the member is reported
/// on every stack and change set rather than only on the ones that named it.
pub const DEPLOYMENT_CONFIG_MODE_STANDARD: &str = "STANDARD";

/// The deployment configuration in force on a stack or a change set, as the modelled
/// member.
///
/// Unlike [`deployment_config_view`], which answers only what a request supplied, this is
/// what a READ of the resource reports: CloudFormation carries the member on every stack
/// and change set, with the default mode for one that named no mode.
pub fn deployment_config_in_force(
    config: Option<&StoredDeploymentConfig>,
) -> Option<crate::ops::types::DeploymentConfig> {
    Some(crate::ops::types::DeploymentConfig {
        mode: config
            .and_then(|config| config.mode.clone())
            .or_else(|| Some(DEPLOYMENT_CONFIG_MODE_STANDARD.to_string())),
        // `DeploymentConfig$DisableRollback`: "Specifies whether to disable rollback of
        // the stack if the stack operation fails. Default: false." The operation ran with
        // that default when the request named no value.
        disable_rollback: config
            .and_then(|config| config.disable_rollback)
            .or(Some(false)),
    })
}

/// The rollback configuration in force on a change set, as the modelled member.
///
/// CloudFormation carries the member on every change set it describes, empty for one whose
/// request named no rollback triggers.
pub fn rollback_configuration_in_force(
    configuration: Option<&StoredRollbackConfiguration>,
) -> Option<crate::ops::types::RollbackConfiguration> {
    Some(rollback_configuration_view(configuration).unwrap_or(
        crate::ops::types::RollbackConfiguration {
            rollback_triggers: None,
            monitoring_time_in_minutes: None,
        },
    ))
}

/// The request's `RollbackConfiguration` as stored configuration.
pub fn stored_rollback_configuration(
    configuration: Option<&crate::ops::types::RollbackConfiguration>,
) -> Option<StoredRollbackConfiguration> {
    configuration.map(|configuration| StoredRollbackConfiguration {
        triggers: configuration
            .rollback_triggers
            .iter()
            .flatten()
            .map(|trigger| StoredRollbackTrigger {
                arn: trigger.arn.clone(),
                trigger_type: trigger.r#type.clone(),
            })
            .collect(),
        monitoring_time_in_minutes: configuration.monitoring_time_in_minutes,
    })
}

/// Stored rollback configuration as the modelled member.
///
/// CloudFormation reports `RollbackConfiguration` on every stack, as an empty element for
/// a stack that has none, so this is always present; the triggers inside it appear only
/// when the stack has some.
pub fn rollback_configuration_view(
    configuration: Option<&StoredRollbackConfiguration>,
) -> Option<crate::ops::types::RollbackConfiguration> {
    Some(crate::ops::types::RollbackConfiguration {
        rollback_triggers: configuration.and_then(|configuration| {
            non_empty(
                configuration
                    .triggers
                    .iter()
                    .map(|trigger| crate::ops::types::RollbackTrigger {
                        arn: trigger.arn.clone(),
                        r#type: trigger.trigger_type.clone(),
                    })
                    .collect(),
            )
        }),
        monitoring_time_in_minutes: configuration
            .and_then(|configuration| configuration.monitoring_time_in_minutes),
    })
}

/// The request's `DeploymentConfig` as stored configuration.
pub fn stored_deployment_config(
    config: Option<&crate::ops::types::DeploymentConfig>,
) -> Option<StoredDeploymentConfig> {
    config.map(|config| StoredDeploymentConfig {
        mode: config.mode.clone(),
        disable_rollback: config.disable_rollback,
    })
}

/// Stored deployment configuration as the modelled member, with the documented defaults
/// filled in.
///
/// CloudFormation reports `DeploymentConfig` on every stack — it is the configuration the
/// stack's operations run under, which every stack has. A stack whose operations never
/// stated one runs under the defaults: [`DEPLOYMENT_MODE_STANDARD`] ("This is the default.
/// You do not need to specify this value explicitly") and, for `DisableRollback`, the
/// rollback setting in force on the stack — the same flag `Stack$DisableRollback` reports.
pub fn deployment_config_view(
    config: Option<&StoredDeploymentConfig>,
    disable_rollback: bool,
) -> Option<crate::ops::types::DeploymentConfig> {
    Some(crate::ops::types::DeploymentConfig {
        mode: Some(
            config
                .and_then(|config| config.mode.clone())
                .unwrap_or_else(|| DEPLOYMENT_MODE_STANDARD.to_string()),
        ),
        disable_rollback: Some(
            config
                .and_then(|config| config.disable_rollback)
                .unwrap_or(disable_rollback),
        ),
    })
}

/// A stack's drift result as the modelled `Stack$DriftInformation`.
///
/// CloudFormation reports `DriftInformation` on every stack — a stack nothing has checked
/// reports `NOT_CHECKED`, which is a result in its own right — so this is always present.
/// `LastCheckTimestamp` is the instant the most recent detection on the stack was
/// INITIATED, and is absent exactly while no detection has been.
pub fn stack_drift_information(
    stack: &StoredStack,
) -> Option<crate::ops::types::StackDriftInformation> {
    Some(crate::ops::types::StackDriftInformation {
        stack_drift_status: stack.drift_status.clone(),
        last_check_timestamp: stack.drift_last_check.map(timestamp),
    })
}

/// The same result as `StackSummary$DriftInformation`, the SUMMARY spelling of the same
/// two members — so `ListStacks` and `DescribeStacks` cannot disagree about one stack's
/// drift result.
pub fn stack_drift_information_summary(
    stack: &StoredStack,
) -> Option<crate::ops::types::StackDriftInformationSummary> {
    Some(crate::ops::types::StackDriftInformationSummary {
        stack_drift_status: stack.drift_status.clone(),
        last_check_timestamp: stack.drift_last_check.map(timestamp),
    })
}

/// One resource's drift result as the modelled `StackResource$DriftInformation` /
/// `StackResourceDetail$DriftInformation`, reported on every resource for the same reason.
pub fn resource_drift_information(
    deployed: &DeployedResource,
) -> Option<crate::ops::types::StackResourceDriftInformation> {
    Some(crate::ops::types::StackResourceDriftInformation {
        stack_resource_drift_status: deployed.drift_status.clone(),
        last_check_timestamp: deployed.drift_checked_at.map(timestamp),
    })
}

/// The same result as `StackResourceSummary$DriftInformation`, the SUMMARY spelling of the
/// same two members.
pub fn resource_drift_information_summary(
    deployed: &DeployedResource,
) -> Option<crate::ops::types::StackResourceDriftInformationSummary> {
    Some(crate::ops::types::StackResourceDriftInformationSummary {
        stack_resource_drift_status: deployed.drift_status.clone(),
        last_check_timestamp: deployed.drift_checked_at.map(timestamp),
    })
}

/// One JSON scalar as the single text an API member takes it as, or `None` for a
/// composite.
///
/// The three scalars a template property can carry each have exactly one textual
/// spelling, and that spelling is what a stack's provisioning sends and what a drift
/// comparison reads back ([`drift`]), so both sides are held to this one function.
pub fn scalar_text(value: &::serde_json::Value) -> Option<String> {
    match value {
        ::serde_json::Value::String(text) => Some(text.clone()),
        ::serde_json::Value::Number(number) => Some(number.to_string()),
        ::serde_json::Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCloudformationCloudformationState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCloudformationCloudformationState>)
}

/// The IAM store a stack operation resolves its `RoleARN` against.
///
/// It is the IAM service's OWN state struct, reached through the SAME shared engine this
/// service's store lives in, so CloudFormation reads the very records `CreateRole` wrote
/// rather than a second copy of them.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The service principal CloudFormation assumes a stack's service role as.
///
/// `CreateStackInput$RoleARN`: "The Amazon Resource Name (ARN) of an IAM role that
/// CloudFormation assumes to create the stack. CloudFormation uses the role's credentials
/// to make calls on your behalf." A role it is to assume has to trust this principal for
/// `sts:AssumeRole`.
pub const SERVICE_PRINCIPAL: &str = "cloudformation.amazonaws.com";

/// The IAM store in the SAME shared engine, which is where the account's roles live.
///
/// This is the IAM service's OWN accessor, called rather than copied, so CloudFormation
/// reads the very store that service's handlers write. `None` has the same meaning as for
/// [`service_state`]: the engine carries no such store.
pub fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IamState>> {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}

/// `None` when this emulator has no reason to think CloudFormation could not assume
/// `role_arn`; otherwise the `ValidationError` message that refuses the call.
///
/// A stack operation acts as the `RoleARN` it is handed, so it resolves that role up front
/// and refuses the call when the role's trust policy does not let
/// [`SERVICE_PRINCIPAL`] assume it. The role is resolved against the account's IAM records
/// — the same records `CreateRole` wrote — by its ARN. An ARN the IAM store holds no role
/// for is NOT evidence of a missing trust (the role may belong to another account, or to a
/// client that never told this emulator about it), so the operation goes ahead; only a role
/// this emulator holds and whose trust policy withholds the grant is refused.
///
/// The message names the submitted ARN, which is the rule the real service follows: the
/// refusal is built from the request's own `RoleARN`.
pub fn role_trust_refusal(role_arn: &str, iam: &IamState) -> Option<String> {
    let role = iam.roles.values().find(|role| role.arn == role_arn)?;
    let document = role.assume_role_policy_document.as_deref()?;
    // The rule itself is IAM's, not this service's, so it is read through the ONE
    // evaluator every service that resolves a caller-supplied service role shares: the
    // statement must name the principal and `sts:AssumeRole`, an explicit `Deny` wins
    // over any `Allow`, and the document is percent-decoded before it is parsed. A
    // document this emulator cannot read as a policy at all is no evidence either way.
    match ::service_common::iam_trust_policy::admits_service_principal(document, SERVICE_PRINCIPAL)
    {
        None | Some(true) => None,
        Some(false) => Some(format!("Role {role_arn} is invalid or cannot be assumed")),
    }
}

/// Refuse the call when the account's IAM records show CloudFormation cannot assume the
/// service role this request named, so nothing is written for a stack that could never
/// deploy under it.
///
/// Call it with the request's OWN `RoleARN` (`None` when it named none — "If you don't
/// specify a value, CloudFormation uses the role that was previously associated with the
/// stack", which was checked when that request named it) and with the handler's
/// `&*self.engine`, holding no store lock. Generic over the operation's error enum, so
/// every operation that takes a `RoleARN` refuses it the same way: HTTP 400 with the wire
/// code `ValidationError`.
///
/// An engine carrying no IAM store carries no CloudFormation store either, so the caller's
/// own [`service_state`] lookup is what answers that case; here it is simply no evidence.
///
/// Called BEFORE the store is read, so a request naming a role CloudFormation cannot assume
/// can never reach a write — which also means it is answered ahead of a stack-not-found or
/// already-exists refusal when a request earns both. Both are `ValidationError` on the wire,
/// and the model fixes no precedence between them, so the earlier refusal is the one whose
/// check needs no store.
pub fn refuse_unassumable_role<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    role_arn: Option<&str>,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    let Some(role_arn) = role_arn else {
        return Ok(());
    };
    let Some(iam) = iam_state(engine) else {
        return Ok(());
    };
    let refusal = role_trust_refusal(role_arn, &iam.lock());
    match refusal {
        Some(message) => Err(crate::scaffolded::Scaffolded::validation_error(message)),
        None => Ok(()),
    }
}
