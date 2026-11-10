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
//! given instant, the way every other in-flight record of this service does. What one
//! operation did in each account and Region it acted on —
//! [`StoredStackSet::operation_results`], which `ListStackSetOperationResults` reports — is
//! read off the stack INSTANCE that target's work was performed on, so that read and
//! `ListStackInstances` cannot disagree about what became of a target; the operation's
//! conclusion freezes a copy ([`StoredStackSetOperation::results`]) so its record survives
//! the instances a deletion removes and the operations that run after it.
//!
//! ## What writes it
//!
//! A stack set is written by `CreateStackSet`, which records the configuration it was
//! created with — its template, parameters, capabilities, tags, roles, permission model and
//! deployment settings — so `DescribeStackSet` reports exactly what that call stated rather
//! than anything recomputed from a request. Its instances and the operations performed on it
//! are written by `CreateStackInstances`, by `DeleteStackInstances` — which takes the
//! instances it deleted back OUT of the stack set when the DELETE operation it runs as
//! concludes ([`StoredStackSet::settle`]) — by `UpdateStackSet` and `UpdateStackInstances`,
//! which record the UPDATE operation their mutation runs as, and by `DetectStackSetDrift`,
//! which records the operation a drift detection runs as and what that detection found.
//! `DeleteStackSet` is what retires a record: the stack set takes the DELETED status
//! ([`status`]), which leaves its summary for `ListStackSets` to report and takes it out of
//! everything that RESOLVES a stack set ([`StoredStackSet::is_addressed_by`]). The
//! ONE way a record enters this
//! store is
//! [`crate::state::ComAmazonawsCloudformationCloudformationState::record_stack_set`], so
//! those writers — and the suites that cover the reads over records no wire call can create
//! yet — go through it rather than reaching into the vector.

use std::time::SystemTime;

use super::StoredParameter;

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

/// The model's `StackSetStatus`: "The status of the StackSet"
/// (`smithy_query(query="shape", shape="StackSetStatus")`).
pub mod status {
    /// A StackSet that exists: what `CreateStackSet` leaves behind.
    pub const ACTIVE: &str = "ACTIVE";
    /// A StackSet `DeleteStackSet` has deleted.
    pub const DELETED: &str = "DELETED";
}

/// The model's `PermissionModels`: how "the IAM roles required for StackSet operations are
/// created" (`smithy_query(query="shape", shape="PermissionModels")`).
pub mod permission_model {
    /// "With self-managed permissions, you must create the administrator and execution
    /// roles required to deploy to target accounts."
    pub const SELF_MANAGED: &str = "SELF_MANAGED";
    /// "With service-managed permissions, StackSets automatically creates the IAM roles
    /// required to deploy to accounts managed by Organizations."
    pub const SERVICE_MANAGED: &str = "SERVICE_MANAGED";
}

/// The permission model a StackSet is created under when the request names none:
/// `CreateStackSetInput$PermissionModel` — "By default, SELF-MANAGED is specified."
pub const DEFAULT_PERMISSION_MODEL: &str = permission_model::SELF_MANAGED;

/// The execution role a self-managed StackSet uses when the request names none:
/// `CreateStackSetInput$ExecutionRoleName` — "If you do not specify an execution role,
/// CloudFormation uses the AWSCloudFormationStackSetExecutionRole role for the StackSet
/// operation."
pub const DEFAULT_EXECUTION_ROLE_NAME: &str = "AWSCloudFormationStackSetExecutionRole";

/// The model's `StackSetDriftDetectionStatus` value a COMPLETED detection carries:
/// "COMPLETED: The drift detection operation completed without failing on any stack
/// instances" (`smithy_query(query="shape", shape="StackSetDriftDetectionDetails",
/// member="DriftDetectionStatus")`).
pub const DRIFT_DETECTION_COMPLETED: &str = drift_detection_status::COMPLETED;

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

    /// Whether the operation's work in that account and Region has not concluded there: it
    /// has "yet to start" ([`PENDING`]) or is "currently in progress" ([`RUNNING`]).
    ///
    /// Every other value of the enum states what became of the target, so the operation's own
    /// conclusion is what those two are waiting for.
    pub fn is_unconcluded(detailed_status: &str) -> bool {
        detailed_status == PENDING || detailed_status == RUNNING
    }
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

    /// Whether an operation in this status has NOT concluded — the ONE reading of that,
    /// shared by the single-flight check ([`super::StoredStackSet::operation_in_flight`])
    /// and by the operations that may act only on an operation still in flight.
    ///
    /// Being performed, queued to be performed, or stopping at user request; every other
    /// `StackSetOperationStatus` is terminal.
    pub fn is_in_flight(status: &str) -> bool {
        status == RUNNING || status == QUEUED || status == STOPPING
    }
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

/// The model's `StackSetOperationResultStatus`: "the result status of the StackSet operation
/// for the given account in the given Region"
/// (`smithy_query(query="shape", shape="StackSetOperationResultStatus")`).
pub mod result_status {
    /// "PENDING: The operation in the specified account and Region has yet to start."
    pub const PENDING: &str = "PENDING";
    /// "RUNNING: The operation in the specified account and Region is currently in
    /// progress."
    pub const RUNNING: &str = "RUNNING";
    /// "SUCCEEDED: The operation in the specified account and Region completed
    /// successfully."
    pub const SUCCEEDED: &str = "SUCCEEDED";
    /// "FAILED: The operation in the specified account and Region failed."
    pub const FAILED: &str = "FAILED";
    /// "CANCELLED: The operation in the specified account and Region has been canceled.
    /// This is either because a user has stopped the StackSet operation, or because the
    /// failure tolerance of the StackSet operation has been exceeded."
    pub const CANCELLED: &str = "CANCELLED";
}

/// The model's `OperationResultFilterName`: the one thing a request may filter a stack-set
/// operation's results by (`smithy_query(query="shape", shape="OperationResultFilterName")`).
pub mod operation_result_filter_name {
    /// Filters on the result's `StackSetOperationResultSummary$Status`.
    pub const OPERATION_RESULT_STATUS: &str = "OPERATION_RESULT_STATUS";
}

/// The `StackSetOperationResultStatus` a stack instance's own
/// `StackInstanceStatus$DetailedStatus` is, for the operation that last acted on it.
///
/// The two enums report the SAME thing — what the operation did "in the specified account and
/// Region" — but `StackInstanceDetailedStatus` draws finer distinctions than the five result
/// statuses do, so each of its extra values folds onto the result status that states the same
/// outcome: a stack left unstable or an import that did not complete is an operation that
/// FAILED there, and an account the operation was not performed in at all is one it was
/// CANCELLED in.
pub fn result_status_of(detailed_status: &str) -> &'static str {
    match detailed_status {
        detailed_status::PENDING => result_status::PENDING,
        detailed_status::RUNNING => result_status::RUNNING,
        detailed_status::SUCCEEDED => result_status::SUCCEEDED,
        detailed_status::CANCELLED | detailed_status::SKIPPED_SUSPENDED_ACCOUNT => {
            result_status::CANCELLED
        }
        _ => result_status::FAILED,
    }
}

/// ONE account-and-Region result of ONE stack-set operation, with every member
/// `StackSetOperationResultSummary` reports.
///
/// "The structure that contains information about a specified operation's results for a given
/// account in a given Region": a stack-set operation acts on one stack instance per target
/// account and Region pair, so a result is what that operation did to that pair.
#[derive(Debug, Clone)]
pub struct StoredOperationResult {
    /// `Account`: "the name of the Amazon Web Services account for this operation result".
    pub account: String,
    /// `Region`: "the name of the Amazon Web Services Region for this operation result".
    pub region: String,
    /// `Status`: [`result_status`] — what the operation did in that account and Region.
    pub status: String,
    /// `StatusReason`: "the reason for the assigned result status", for a result that
    /// carries one.
    pub status_reason: Option<String>,
    /// `OrganizationalUnitId`: "[Service-managed permissions] The organization root ID or
    /// organizational unit (OU) IDs" the target belongs to, absent for a self-managed stack
    /// set.
    pub organizational_unit_id: Option<String>,
}

impl StoredOperationResult {
    /// Whether the filter `name` with `value` selects this result.
    ///
    /// `OperationResultFilter$Values` is "the value to filter by": one value, compared with
    /// the member the filter names, so a result carrying another value is not selected. A
    /// name outside the modelled `OperationResultFilterName` enum cannot reach here — the
    /// boundary refuses it before a handler runs — and selects nothing if it ever did.
    pub fn matches_filter(&self, name: &str, value: &str) -> bool {
        match name {
            operation_result_filter_name::OPERATION_RESULT_STATUS => self.status == value,
            _ => false,
        }
    }
}

/// The model's `StackSetStatus`: "the status of the StackSet"
/// (`smithy_query(query="shape", shape="StackSetStatus")`).
///
/// An alias of [`status`], kept so a caller reading a StackSet's own status spells which
/// status it means.
pub use status as stack_set_status;

/// The model's `StackSetDriftDetectionStatus`: "the status of the StackSet drift detection
/// operation" (`smithy_query(query="shape", shape="StackSetDriftDetectionStatus")`).
pub mod drift_detection_status {
    /// "COMPLETED: The drift detection operation completed without failing on any stack
    /// instances."
    pub const COMPLETED: &str = "COMPLETED";
    /// "FAILED: The drift detection operation exceeded the specified failure tolerance."
    pub const FAILED: &str = "FAILED";
    /// "PARTIAL_SUCCESS: The drift detection operation completed without exceeding the
    /// failure tolerance for the operation."
    pub const PARTIAL_SUCCESS: &str = "PARTIAL_SUCCESS";
    /// "IN_PROGRESS: The drift detection operation is currently being performed."
    pub const IN_PROGRESS: &str = "IN_PROGRESS";
    /// "STOPPED: The user has canceled the drift detection operation."
    pub const STOPPED: &str = "STOPPED";
}

/// Mint a stack set's id: `<name>:<uuid>`, the form CloudFormation gives a stack set, with
/// the same 8-4-4-4-12 identifier every other id this service mints carries.
pub fn new_stack_set_id(stack_set_name: &str) -> String {
    format!("{stack_set_name}:{}", super::new_identifier(stack_set_name))
}

/// A stack set's ARN, which is what `StackSet$StackSetARN` reports: the stack set's id
/// under the `stackset/` resource path of this account and Region, built by the
/// emulator-wide ARN helper every other identifier of this service goes through.
pub fn new_stack_set_arn(region: &str, stack_set_id: &str) -> String {
    ::emulator_server::emulator_arn(
        super::ARN_NAMESPACE,
        region,
        &format!("stackset/{stack_set_id}"),
    )
}

/// The stack NAME CloudFormation gives the stack ONE instance of a stack set deploys:
/// `StackSet-<StackSetName>-<uuid>`, the form a stack instance's stack carries.
///
/// Minted per instance, like every other identifier this service hands out, so two
/// instances of one stack set never address the same stack.
pub fn new_instance_stack_name(stack_set_name: &str) -> String {
    format!(
        "StackSet-{stack_set_name}-{}",
        super::new_identifier(stack_set_name)
    )
}

/// The `StackSetOperationPreferences` a request supplied, as stored.
///
/// The ONE projection of the wire shape onto the record, so every operation that starts a
/// stack-set operation stores the same thing — and [`preferences_view`] is its inverse, so
/// a read reports exactly the members the request named.
pub fn stored_preferences(
    supplied: &crate::ops::types::StackSetOperationPreferences,
) -> StoredOperationPreferences {
    StoredOperationPreferences {
        region_concurrency_type: supplied.region_concurrency_type.clone(),
        region_order: supplied.region_order.clone().unwrap_or_default(),
        failure_tolerance_count: supplied.failure_tolerance_count,
        failure_tolerance_percentage: supplied.failure_tolerance_percentage,
        max_concurrent_count: supplied.max_concurrent_count,
        max_concurrent_percentage: supplied.max_concurrent_percentage,
        concurrency_mode: supplied.concurrency_mode.clone(),
    }
}

/// The stored preferences as the modelled `StackSetOperationPreferences`: the members the
/// request that started the operation named, and nothing else.
pub fn preferences_view(
    stored: &StoredOperationPreferences,
) -> crate::ops::types::StackSetOperationPreferences {
    crate::ops::types::StackSetOperationPreferences {
        region_concurrency_type: stored.region_concurrency_type.clone(),
        region_order: crate::state::non_empty(stored.region_order.clone()),
        failure_tolerance_count: stored.failure_tolerance_count,
        failure_tolerance_percentage: stored.failure_tolerance_percentage,
        max_concurrent_count: stored.max_concurrent_count,
        max_concurrent_percentage: stored.max_concurrent_percentage,
        concurrency_mode: stored.concurrency_mode.clone(),
    }
}

/// The refusal for a request that names BOTH targeting modes.
///
/// The RULE comes from the model — "You can specify Accounts or DeploymentTargets, but not
/// both" — and this is the sentence the service answers with, entirely fixed text with
/// nothing taken from the request, since the request's own shape is what is wrong.
pub const BOTH_TARGETING_MODES: &str = "Cannot specify both Accounts and DeploymentTargets";

/// The refusal for a request that names NEITHER targeting mode.
///
/// The RULE, likewise from the model: "You must specify at least one value for either
/// Accounts or DeploymentTargets."
pub const NO_TARGETING_MODE: &str =
    "You must specify at least one value for either Accounts or DeploymentTargets";

/// How a stack-set deployment request names its TARGETS: the two mutually exclusive
/// targeting members, read once for every operation that takes them.
#[derive(Debug, Clone, Default)]
pub struct RequestedTargets {
    /// The accounts the request names — through `Accounts`, or through
    /// `DeploymentTargets.Accounts`.
    pub accounts: Vec<String>,
    /// `DeploymentTargets$OrganizationalUnitIds`: "the organization root ID or
    /// organizational unit (OU) IDs where you want to perform stack operations".
    pub organizational_unit_ids: Vec<String>,
}

/// The targeting members a stack-set deployment request names, held to the rules that are
/// about the REQUEST alone — the ones that are decided before any stack set is resolved.
///
/// "[Self-managed permissions] The account IDs of one or more Amazon Web Services accounts"
/// / "[Service-managed permissions] The Organizations accounts": "You can specify Accounts
/// or DeploymentTargets, but not both", and "you must specify at least one value for
/// either". The ONE reading of those two members, so every operation that targets accounts
/// and Regions reads a request the same way; what the targeting means for one stack set is
/// [`RequestedTargets::accounts_in`].
pub fn requested_targets(
    accounts: Option<&[String]>,
    deployment_targets: Option<&crate::ops::types::DeploymentTargets>,
) -> Result<RequestedTargets, String> {
    let named: Vec<String> = accounts.unwrap_or_default().to_vec();
    if !named.is_empty() && deployment_targets.is_some() {
        return Err(BOTH_TARGETING_MODES.to_string());
    }
    // "The location of the accounts" an `AccountsUrl` names is an Amazon S3 object, which
    // would have to be fetched over the network — this emulator makes no outbound
    // connections, so a request naming one is refused rather than served with the accounts
    // nobody read.
    if deployment_targets.is_some_and(|targets| targets.accounts_url.is_some()) {
        return Err(
            "DeploymentTargets.AccountsUrl names an Amazon S3 object this emulator does not \
             read"
                .to_string(),
        );
    }
    let targeted = |select: fn(&crate::ops::types::DeploymentTargets) -> Option<&Vec<String>>| {
        deployment_targets
            .and_then(select)
            .cloned()
            .unwrap_or_default()
    };
    let requested = RequestedTargets {
        accounts: distinct(if named.is_empty() {
            targeted(|targets| targets.accounts.as_ref())
        } else {
            named
        }),
        organizational_unit_ids: distinct(targeted(|targets| {
            targets.organizational_unit_ids.as_ref()
        })),
    };
    if requested.accounts.is_empty() && requested.organizational_unit_ids.is_empty() {
        return Err(NO_TARGETING_MODE.to_string());
    }
    Ok(requested)
}

/// Why the targeting a request named is not one the stack set it names can be deployed to.
///
/// The two cases are answered on DIFFERENT channels: a targeting the real service refuses
/// too is a bad request, while one the real service SERVES and this emulator holds no state
/// to carry out is not the caller's mistake, so it is answered as work nobody has written
/// rather than by telling a well-formed request that it is malformed.
#[derive(Debug, Clone)]
pub enum TargetingRefusal {
    /// A targeting the real service refuses too, in its own words.
    Invalid(String),
    /// A targeting the real service serves and this emulator holds no state to resolve.
    Unsupported,
}

impl TargetingRefusal {
    /// This refusal on the error channel `operation` answers with.
    pub fn refuse<E>(self, operation: &str) -> crate::scaffolded::Scaffolded<E> {
        match self {
            Self::Invalid(message) => crate::scaffolded::Scaffolded::validation_error(message),
            Self::Unsupported => crate::scaffolded::Scaffolded::not_implemented(operation),
        }
    }
}

impl RequestedTargets {
    /// The accounts a deployment of `stack_set` really targets, or why this targeting is
    /// not one that stack set can be deployed to.
    ///
    /// `OrganizationalUnitIds` selects targets by ORGANIZATION, which only a stack set
    /// whose permission model is service-managed has — "[Service-managed permissions] The
    /// Organizations accounts in which to create stack instances" — so a request naming OUs
    /// against a stack set of any other permission model is refused for that reason, naming
    /// the model the stack set carries, rather than read as naming no targeting mode at all.
    pub fn accounts_in(
        &self,
        stack_set: &StoredStackSet,
    ) -> Result<Vec<String>, TargetingRefusal> {
        if !self.organizational_unit_ids.is_empty() {
            if stack_set.permission_model != permission_model::SERVICE_MANAGED {
                return Err(TargetingRefusal::Invalid(format!(
                    "StackSets with {} permission model can only have accounts as target",
                    stack_set.permission_model
                )));
            }
            // A service-managed stack set's OU targeting selects the accounts the
            // organization holds under those OUs — "CloudFormation will perform operations on
            // accounts within these OUs and their child OUs" — and `AccountFilterType` is how
            // any `Accounts` the request names ALSO refines that set. The real service serves
            // this; this emulator holds no organization membership to expand an OU into
            // accounts, so the call is answered as work nobody has written rather than
            // silently over whichever accounts the request happened to list, or as a request
            // that named no targeting mode — it named one.
            return Err(TargetingRefusal::Unsupported);
        }
        if self.accounts.is_empty() {
            return Err(TargetingRefusal::Invalid(NO_TARGETING_MODE.to_string()));
        }
        Ok(self.accounts.clone())
    }
}

/// The first entry of `regions` that is not an Amazon Web Services Region, if any.
///
/// A stack instance is "a stack in a specific account and Region", so a Region a deployment
/// targets has to be a Region that exists: one that does not could never carry a stack, and
/// the whole request is refused rather than half of it carried out. The catalog is
/// `emulator_telemetry::policy::KNOWN_REGIONS` ("every AWS region the wire may carry"), the
/// one list of Region codes this emulator holds; the model's own `Region`
/// `@pattern("^[a-zA-Z0-9-]{1,128}$")` admits any such label, so the boundary cannot decide
/// this and the handler is what does.
pub fn unknown_target_region(regions: &[String]) -> Option<&str> {
    // The catalog is kept SORTED for exactly this lookup, so an entry costs a binary search
    // rather than a scan of the whole list.
    regions.iter().map(String::as_str).find(|region| {
        ::emulator_telemetry::policy::KNOWN_REGIONS
            .binary_search(region)
            .is_err()
    })
}

/// The refusal naming a Region AWS does not have, worded once for every operation that
/// takes a `RegionList`.
///
/// One variable part — the offending Region, as the request spelled it — inside fixed text;
/// nothing else of the request appears.
pub fn region_does_not_exist(region: &str) -> String {
    format!("Region {region} is not supported")
}

/// `base` with `overrides` applied by parameter key: an overridden parameter takes the
/// override's value, one the overrides do not name keeps the base's, and an override of a
/// key the base does not carry is kept too.
///
/// What a stack instance's own stack is parameterised with: "any overridden parameter values
/// will be applied to all stack instances in the specified accounts and Regions", over the
/// StackSet's own `Parameters`. The base's order is kept, so two reads of one instance
/// cannot disagree about it.
///
/// Linear in the two counts: the overrides are indexed by key in ONE pass and the base is
/// walked once, so the two caller-controlled counts cannot multiply — the same bound
/// [`crate::state::parameters_in_force`] holds itself to. Both counts are refused past
/// [`MAX_SUPPLIED_PARAMETERS`] before they reach here.
pub fn parameters_with_overrides(
    base: &[StoredParameter],
    overrides: &[StoredParameter],
) -> Vec<StoredParameter> {
    let mut by_key: ::std::collections::BTreeMap<&str, &StoredParameter> = overrides
        .iter()
        .map(|parameter| (parameter.key.as_str(), parameter))
        .collect();
    let mut merged: Vec<StoredParameter> = base
        .iter()
        .map(|parameter| match by_key.remove(parameter.key.as_str()) {
            // Whether the parameter is MASKED is a property of the template that declares
            // it, not of the value an override carries, so the base's flag is kept.
            Some(overridden) => StoredParameter {
                key: parameter.key.clone(),
                value: overridden.value.clone(),
                no_echo: parameter.no_echo,
                // The override states a NAME where the parameter is SSM-typed, so the
                // value it resolves to is read where the instance is deployed.
                resolved_value: None,
            },
            None => parameter.clone(),
        })
        .collect();
    // An override naming a parameter the base states no value for is kept rather than
    // dropped: whether the template declares it is what `parameters_in_force` resolves.
    merged.extend(by_key.into_values().cloned());
    merged
}

/// The most `Parameters` / `ParameterOverrides` entries one request may supply.
///
/// A parameter a request supplies is a parameter of the template it is resolved against, and
/// "the maximum number of parameters that you can declare in a template" is the documented
/// CloudFormation quota ([`crate::state::template::MAX_PARAMETERS_PER_TEMPLATE`]): a request
/// naming more than that names parameters no template could declare. Every operation that
/// takes such a list is held to it BEFORE the store is read, so no caller-sized parameter
/// list drives work under the store lock.
pub const MAX_SUPPLIED_PARAMETERS: usize = crate::state::template::MAX_PARAMETERS_PER_TEMPLATE;

/// `Err(message)` when `supplied` carries more parameters than a template can declare.
///
/// The ONE reading of that bound, so every operation taking a caller's parameter list
/// refuses an over-long one identically, naming the member that carried it. Worded as
/// [`crate::state::template::quota_violation`] words the same documented quota — the count,
/// the maximum — without its `Template format error:` prefix, which belongs to a refusal of
/// a template BODY rather than of a request's own list.
pub fn refuse_oversized_parameters(
    member: &str,
    supplied: Option<&[crate::ops::types::Parameter]>,
) -> Result<(), String> {
    let count = supplied.unwrap_or_default().len();
    if count > MAX_SUPPLIED_PARAMETERS {
        return Err(format!(
            "{member} count {count} is greater than max allowed {MAX_SUPPLIED_PARAMETERS}"
        ));
    }
    Ok(())
}

/// The message CloudFormation answers when the `OperationId` a request named already names
/// an operation of that stack set: "The specified operation ID already exists."
///
/// One builder for the five mutations that take the token, so the wording cannot drift. The
/// id is the one the request carried, which is the only variable part of the sentence.
pub fn operation_id_already_exists(operation_id: &str) -> String {
    format!("Operation with Id {operation_id} already exists")
}

/// The `NameAlreadyExistsException` message a `CreateStackSet` for a name the administrator
/// account and Region already holds a stack set under is refused with.
///
/// A FIXED sentence: CloudFormation words this refusal without naming the stack set, so the
/// answer a client compares against carries no part of the request in it.
pub const STACK_SET_ALREADY_EXISTS: &str = "StackSet already exists.";

/// The `InvalidOperationException` message a `StopStackSetOperation` for an operation that is
/// not in a stoppable state is refused with, naming the status the operation is in.
///
/// "Stops an in-progress operation": the status is the one the operation record holds at that
/// instant, which is the status a read of the operation reports alongside this refusal.
pub fn cannot_stop_operation(status: &str) -> String {
    format!("Cannot Stop since Operation is in {status}")
}

/// The message for a stack set that declares no template, so there is nothing for an
/// instance of it to deploy: "The specified operation isn't valid."
pub fn stack_set_has_no_template(stack_set_name: &str) -> String {
    format!("StackSet {stack_set_name} has no template to deploy")
}

/// `values` with repeats removed and first-seen order kept.
///
/// A repeated account or Region names the SAME target — an account and Region pair carries at
/// most one instance of a stack set — so what a request selects is the product of the
/// DISTINCT values it names, which is also what bounds the work that selection drives.
pub fn distinct(values: Vec<String>) -> Vec<String> {
    let mut seen: ::std::collections::HashSet<String> = ::std::collections::HashSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

/// `Err(message)` when `accounts` accounts and `regions` Regions select more account and
/// Region pairs than one stack set can hold instances for.
///
/// The ONE reading of that bound. Every operation that selects instances is held to it
/// BEFORE the store is read, over the distinct values the request names, and again once the
/// stack set has resolved what those targets really select — so neither an expanded target
/// set nor a repeated value can drive unbounded work under the store lock.
pub fn refuse_oversized_selection(accounts: usize, regions: usize) -> Result<(), String> {
    if accounts.saturating_mul(regions) > MAX_INSTANCES_PER_STACK_SET {
        return Err(selection_wider_than_a_stack_set());
    }
    Ok(())
}

/// The message for a selection naming more account and Region pairs than one stack set can
/// hold instances for — the ceiling the store bounds a stack set's instances at.
pub fn selection_wider_than_a_stack_set() -> String {
    format!(
        "Accounts and Regions select more than the {MAX_INSTANCES_PER_STACK_SET} stack \
         instances a StackSet can hold"
    )
}

/// The `OperationInProgressException` message CloudFormation answers a request that asks a
/// stack set to perform an operation while it is already performing one: the stack set the
/// request named, by ARN, and the `OperationId` of the operation that still holds it — both
/// taken from the record the store holds, never from any one run.
pub fn operation_in_progress_message(stack_set_arn: &str, operation_id: &str) -> String {
    format!("Another Operation on StackSet {stack_set_arn} is in progress: {operation_id}")
}

/// The `StackSetNotEmptyException` message CloudFormation answers a request that asks for a
/// stack set that still carries stack instances to be deleted.
///
/// "Before you can delete a StackSet, all its member stack instances must be deleted", and
/// the refusal is entirely FIXED text — the condition is about the stack set as a whole,
/// which is why the modelled exception carries no member naming an instance — so nothing of
/// the request or of any one run goes into it.
pub const STACK_SET_NOT_EMPTY: &str = "StackSet is not empty";

/// One resource of one stack instance, as the drift detection that covered it found it —
/// every member `StackInstanceResourceDriftsSummary` reports.
#[derive(Debug, Clone)]
pub struct StoredResourceDrift {
    /// `LogicalResourceId`: "the logical name of the resource specified in the template".
    pub logical_resource_id: String,
    /// `PhysicalResourceId`: "the name or unique identifier that corresponds to a physical
    /// instance ID of a resource supported by CloudFormation".
    pub physical_resource_id: Option<String>,
    /// `ResourceType`: the resource's type as the template declares it.
    pub resource_type: String,
    /// `StackResourceDriftStatus`: "the drift status of the resource in a stack instance"
    /// ([`crate::state::resource_drift_status`]).
    pub drift_status: String,
    /// `PropertyDifferences`: "status of the actual configuration of the resource compared
    /// to its expected configuration", present "only for resources whose
    /// `StackInstanceResourceDriftStatus` is `MODIFIED`".
    pub differences: Vec<super::drift::PropertyDiff>,
}

/// What one drift detection found for ONE stack instance of a stack set.
///
/// `ListStackInstanceResourceDrifts` reads these back by the account, the Region and the
/// `OperationId` of the detection that produced them, so the per-resource rows of one
/// detection are never served for another detection's id.
#[derive(Debug, Clone)]
pub struct StoredInstanceDrift {
    /// The target account the instance is deployed in.
    pub account: String,
    /// The target Region the instance is deployed in.
    pub region: String,
    /// `StackId`: "the ID of the stack instance" the detection read.
    pub stack_id: String,
    /// The instance's own `DriftStatus`, which is what the per-resource rows add up to.
    pub drift_status: String,
    /// `Timestamp`: "time at which the stack instance drift detection operation was
    /// initiated".
    pub checked_at: SystemTime,
    /// One row per resource the detection covered, in the order it checked them.
    pub resources: Vec<StoredResourceDrift>,
}

/// `AutoDeployment` as the call that set it supplied: "whether StackSets automatically
/// deploys to Organizations accounts that are added to a target organization or
/// organizational unit (OU)".
#[derive(Debug, Clone, Default)]
pub struct StoredAutoDeployment {
    /// `Enabled`: "If set to true, StackSets automatically deploys additional stack
    /// instances to Organizations accounts that are added to a target organization or
    /// organizational unit (OU) in the specified Regions."
    pub enabled: Option<bool>,
    /// `RetainStacksOnAccountRemoval`: "If set to true, stack resources are retained when
    /// an account is removed from a target organization or OU."
    pub retain_stacks_on_account_removal: Option<bool>,
    /// `DependsOn`: "A list of StackSet ARNs that this StackSet depends on for
    /// auto-deployment operations."
    pub depends_on: Vec<String>,
}

/// ONE stack set: how it is addressed, the configuration it was created with, and the
/// instances it carries.
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
    /// `StackSet$StackSetARN`: "The Amazon Resource Name (ARN) of the StackSet", minted
    /// once with the record by [`new_stack_set_arn`] so every read reports the same ARN.
    pub stack_set_arn: String,
    /// `StackSet$Status`: the model's `StackSetStatus` ([`status`]). A stack set
    /// `CreateStackSet` has created is ACTIVE — CloudFormation answers that call with a
    /// stack set that already exists, so there is no transient status to settle.
    pub status: String,
    /// `StackSet$Description`: "A description of the StackSet that you specify when the
    /// StackSet is created or updated."
    pub description: Option<String>,
    /// `StackSet$TemplateBody`: "The structure that contains the body of the template that
    /// was used to create or update the StackSet." `None` for a record written without one.
    pub template_body: Option<String>,
    /// `StackSet$Parameters`: "A list of input parameters for a StackSet", resolved against
    /// the template in force the way a stack's are.
    pub parameters: Vec<super::StoredParameter>,
    /// `StackSet$Capabilities`: "The capabilities that are allowed in the StackSet."
    pub capabilities: Vec<String>,
    /// `StackSet$Tags`: "A list of tags that specify information about the StackSet."
    pub tags: Vec<super::StoredTag>,
    /// `StackSet$AdministrationRoleARN`: "The Amazon Resource Name (ARN) of the IAM role
    /// used to create or update the StackSet", as the call that set it named it.
    pub administration_role_arn: Option<String>,
    /// `StackSet$ExecutionRoleName`: "The name of the IAM execution role used to create or
    /// update the StackSet."
    pub execution_role_name: Option<String>,
    /// `StackSet$PermissionModel`: the model's `PermissionModels` ([`permission_model`]).
    pub permission_model: String,
    /// `StackSet$AutoDeployment`, as supplied: "Valid only if the StackSet uses
    /// service-managed permissions."
    pub auto_deployment: Option<StoredAutoDeployment>,
    /// `StackSet$ManagedExecution$Active`: "When true, CloudFormation performs
    /// non-conflicting operations concurrently and queues conflicting operations [...] When
    /// false (default), StackSets performs one operation at a time in request order."
    pub managed_execution_active: bool,
    /// The `ClientRequestToken` the call that created this stack set carried — "Specify this
    /// token if you plan to retry requests so that CloudFormation knows that you're not
    /// attempting to create another StackSet with the same name", which is what makes a
    /// retry of that same call idempotent rather than a name collision.
    pub client_request_token: Option<String>,
    /// The stack set's instances, in the order they entered it, so a paginated read is a
    /// stable window over them. Bounded at [`MAX_INSTANCES_PER_STACK_SET`].
    pub instances: Vec<StoredStackInstance>,
    /// The operations performed on this stack set, in the order they were INITIATED, which
    /// is the order `StoredStackSet::operations_newest_first` reverses for the read.
    /// Bounded at [`MAX_OPERATIONS_PER_STACK_SET`].
    pub operations: Vec<StoredStackSetOperation>,
    /// The status-change events this stack set's lifecycle owes the account's default event
    /// bus and nothing has published yet, in the order the changes happened
    /// ([`super::stack_set_events`]).
    ///
    /// A change is recorded under this store's lock and published with it released, so the
    /// event waits here in between; claiming it is taking it, so each is published exactly
    /// once. Bounded at [`super::stack_set_events::MAX_PENDING_EVENTS`], oldest reclaimed
    /// first.
    pub pending_events: Vec<super::stack_set_events::StackSetEvent>,
}

impl StoredStackSet {
    /// A stack set of `stack_set_name` in `region`, with no instances yet, a freshly minted
    /// id and ARN, and the configuration defaults the model documents for a request that
    /// names nothing: ACTIVE, self-managed, one operation at a time.
    pub fn new(stack_set_name: impl Into<String>, region: impl Into<String>) -> Self {
        let stack_set_name = stack_set_name.into();
        let region = region.into();
        let stack_set_id = new_stack_set_id(&stack_set_name);
        Self {
            stack_set_arn: new_stack_set_arn(&region, &stack_set_id),
            stack_set_id,
            stack_set_name,
            region,
            status: status::ACTIVE.to_string(),
            description: None,
            template_body: None,
            parameters: Vec::new(),
            capabilities: Vec::new(),
            tags: Vec::new(),
            administration_role_arn: None,
            execution_role_name: None,
            permission_model: DEFAULT_PERMISSION_MODEL.to_string(),
            auto_deployment: None,
            managed_execution_active: false,
            client_request_token: None,
            instances: Vec::new(),
            operations: Vec::new(),
            pending_events: Vec::new(),
        }
    }

    /// Queue the event `change` reports about this stack set, as of `at`
    /// ([`super::stack_set_events`]).
    ///
    /// Bounded at [`super::stack_set_events::MAX_PENDING_EVENTS`]: a queue nothing drains —
    /// an emulator with no EventBridge mounted — reclaims its oldest entries rather than
    /// growing without limit.
    pub fn queue_event(&mut self, change: super::stack_set_events::StackSetChange, at: SystemTime) {
        self.pending_events
            .push(super::stack_set_events::StackSetEvent {
                region: self.region.clone(),
                stack_set_arn: self.stack_set_arn.clone(),
                change,
                at,
            });
        if self.pending_events.len() > super::stack_set_events::MAX_PENDING_EVENTS {
            let excess = self.pending_events.len() - super::stack_set_events::MAX_PENDING_EVENTS;
            self.pending_events.drain(..excess);
        }
    }

    /// Queue the event the `StackSetStatus` this record NOW carries owes, as of `at`: what
    /// the call that created this stack set, or the one that deleted it, has just written
    /// ([`status`]).
    pub fn note_status_change(&mut self, at: SystemTime) {
        let status = self.status.clone();
        self.queue_event(
            super::stack_set_events::StackSetChange::StackSet { status },
            at,
        );
    }

    /// The operation this stack set holds under `operation_id`, if it holds one.
    pub fn operation(&self, operation_id: &str) -> Option<&StoredStackSetOperation> {
        self.operations
            .iter()
            .find(|operation| operation.operation_id == operation_id)
    }

    /// The operation still in flight on this stack set, if one is — the operation
    /// `OperationInProgressException` names ("another operation is currently in progress for
    /// this StackSet").
    pub fn operation_in_flight(&self) -> Option<&StoredStackSetOperation> {
        self.operations
            .iter()
            .find(|operation| operation_status::is_in_flight(&operation.status))
    }

    /// Whether an operation on this stack set is still RUNNING — the state
    /// `OperationInProgressException` reports ("another operation is currently in progress
    /// for this StackSet").
    pub fn operation_in_progress(&self) -> bool {
        self.operation_in_flight().is_some()
    }

    /// The instance deployed in `account` and `region`, if this stack set holds one.
    pub fn instance(&self, account: &str, region: &str) -> Option<&StoredStackInstance> {
        self.instances
            .iter()
            .find(|instance| instance.is_in(account, region))
    }

    /// Whether `name_or_id` addresses this stack set in `region`: `StackSetName` is "the
    /// name or unique ID of the StackSet", and nothing outside that Region is addressable.
    ///
    /// A stack set a deletion has already taken is addressed by NOTHING: its record stays in
    /// the store so `ListStackSets` keeps reporting its summary under the DELETED status the
    /// model declares for it — until a new stack set takes that name in that Region, which
    /// [`crate::state::ComAmazonawsCloudformationCloudformationState::record_stack_set`]
    /// replaces the retired record with — but the name and the id it was addressed by resolve
    /// to no stack set any more: a read of it is "the specified StackSet doesn't exist", a
    /// mutation of it likewise, its name is free for a new stack set, and a second deletion of
    /// it has nothing left to delete.
    pub fn is_addressed_by(&self, region: &str, name_or_id: &str) -> bool {
        self.status != status::DELETED
            && self.region == region
            && (self.stack_set_name == name_or_id || self.stack_set_id == name_or_id)
    }

    /// `StackSet$Regions`: "a list of all Amazon Web Services Regions the given StackSet has
    /// stack instances deployed in" — read off the instances it holds, first-seen order,
    /// each Region once however many accounts it is deployed to there.
    pub fn deployed_regions(&self) -> Vec<String> {
        let mut regions: Vec<String> = Vec::new();
        for instance in &self.instances {
            if !regions.iter().any(|held| held == &instance.region) {
                regions.push(instance.region.clone());
            }
        }
        regions
    }

    /// `StackSet$OrganizationalUnitIds`: "[Service-managed permissions] The organization
    /// root ID or organizational unit (OU) IDs" its instances were deployed to, each once.
    /// A self-managed StackSet's instances carry none, so the list is empty for one.
    pub fn organizational_unit_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        for id in self
            .instances
            .iter()
            .filter_map(|instance| instance.organizational_unit_id.as_ref())
        {
            if !ids.iter().any(|held| held == id) {
                ids.push(id.clone());
            }
        }
        ids
    }

    /// This stack set's DEPLOYMENT TARGETS, which `ListStackSetAutoDeploymentTargets`
    /// reports as `StackSetAutoDeploymentTargetSummary`: each
    /// `OrganizationalUnitId` — "the organization root ID or organizational unit (OU) IDs
    /// where the StackSet is targeted" — once in first-seen order, paired with `Regions`,
    /// "the list of Regions targeted for this organization or OU", likewise each once in
    /// first-seen order.
    ///
    /// Read off the instances the stack set holds, the way [`Self::organizational_unit_ids`]
    /// and [`Self::deployed_regions`] are, so a target is reported exactly while the stack
    /// set has a deployment under it and the Regions reported for it are the Regions it is
    /// deployed to there. A self-managed stack set's instances carry no OU, so it has no
    /// targets.
    pub fn deployment_targets(&self) -> Vec<(String, Vec<String>)> {
        let mut targets: Vec<(String, Vec<String>)> = Vec::new();
        for instance in &self.instances {
            let Some(id) = instance.organizational_unit_id.as_ref() else {
                continue;
            };
            let index = match targets.iter().position(|(held, _)| held == id) {
                Some(index) => index,
                None => {
                    targets.push((id.clone(), Vec::new()));
                    targets.len() - 1
                }
            };
            let regions = &mut targets[index].1;
            if !regions.iter().any(|held| held == &instance.region) {
                regions.push(instance.region.clone());
            }
        }
        targets
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
        // The operation STARTING is a status change of its own, published the way its
        // conclusion is ([`super::stack_set_events`]): recorded as of the instant the
        // operation was initiated, which is before the conclusion the clock owes it.
        let started = super::stack_set_events::StackSetChange::Operation {
            operation_id: operation.operation_id.clone(),
            action: operation.action.clone(),
            status: operation.status.clone(),
        };
        let creation = operation.creation;
        match self
            .operations
            .iter_mut()
            .find(|held| held.operation_id == operation.operation_id)
        {
            Some(held) => *held = operation,
            None => self.operations.push(operation),
        }
        self.queue_event(started, creation);
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

    /// What `operation` did in each account and Region it acted on, which is what
    /// `ListStackSetOperationResults` reports.
    ///
    /// A stack-set operation acts on one stack INSTANCE per target account and Region pair,
    /// and that instance is where what the operation did to the pair lives
    /// ([`StoredStackInstance::last_operation_id`] is "the last unique ID of a StackSet
    /// operation performed on a stack instance"), so a result is read off the instance while
    /// that instance is still the one this operation last acted on. What its CONCLUSION
    /// froze ([`StoredStackSetOperation::results`]) is what the operation's record of a
    /// target falls back to once the instance is no longer that: an instance a deletion
    /// removed, or one a later operation has since acted on. So the targets an operation
    /// reports are the ones it acted on however many operations ran after it, and nothing
    /// reports a success for a deployment a late failure corrected.
    ///
    /// The order is the order its conclusion recorded the targets in — the order the stack
    /// set holds its instances in — so a paginated read is a stable window over the results;
    /// the model documents no other order for them.
    pub fn operation_results(
        &self,
        operation: &StoredStackSetOperation,
    ) -> Vec<StoredOperationResult> {
        let current = self.results_in_flight(&operation.operation_id);
        let Some(frozen) = operation.results.as_ref() else {
            return current;
        };
        let current: ::std::collections::BTreeMap<(&str, &str), &StoredOperationResult> = current
            .iter()
            .map(|result| ((result.account.as_str(), result.region.as_str()), result))
            .collect();
        frozen
            .iter()
            .map(
                |held| match current.get(&(held.account.as_str(), held.region.as_str())) {
                    Some(now) => (*now).clone(),
                    None => held.clone(),
                },
            )
            .collect()
    }

    /// One result per instance the operation `operation_id` last acted on, as those
    /// instances stand right now.
    fn results_in_flight(&self, operation_id: &str) -> Vec<StoredOperationResult> {
        self.instances
            .iter()
            .filter(|instance| instance.last_operation_id == operation_id)
            .map(|instance| StoredOperationResult {
                account: instance.account.clone(),
                region: instance.region.clone(),
                status: result_status_of(&instance.detailed_status).to_string(),
                status_reason: instance.status_reason.clone(),
                organizational_unit_id: instance.organizational_unit_id.clone(),
            })
            .collect()
    }

    /// Materialise every status transition the clock has made due on this stack set's
    /// operations, and the per-instance state each operation that CONCLUDES owes.
    ///
    /// An operation's conclusion is what makes its outcome the stack set's: the instances a
    /// deployment created are up to date with the stack set once it finishes, and a drift
    /// detection's findings become the instances' own drift results — which is what
    /// [`drift_detection_details`] counts the stack set's `StackSetDriftDetectionDetails`
    /// off, so "information about drift operations in-progress isn't included" holds without
    /// a second copy of the aggregate.
    pub fn settle(&mut self, now: SystemTime) {
        for index in 0..self.operations.len() {
            if !self.operations[index].settle(now) {
                continue;
            }
            let concluded = self.operations[index].clone();
            // "Deletes stack instances for the specified accounts, in the specified Amazon
            // Web Services Regions": an instance the deletion carried out is no longer one
            // of the stack set's, so it leaves the stack set when the operation that deleted
            // it concludes — and the removal having been carried out there is "SUCCEEDED:
            // The operation in the specified account and Region completed successfully",
            // which the operation's own results keep once the instance is gone. A target the
            // deletion had not concluded in — one it had yet to start, or one it was carrying
            // out — is carried out by that conclusion; an instance the deletion FAILED on, or
            // one a stop CANCELLED, is left where it is, in the state that put it there, so a
            // partly failed deletion reports exactly which instances it could not remove.
            if concluded.action == operation_action::DELETE {
                for instance in &mut self.instances {
                    if instance.last_operation_id == concluded.operation_id
                        && detailed_status::is_unconcluded(&instance.detailed_status)
                    {
                        instance.detailed_status = detailed_status::SUCCEEDED.to_string();
                    }
                }
            }
            // "STOPPED: The user has canceled the drift detection operation": a detection
            // the caller stopped reports that rather than the status its own reads had
            // reached, so nothing reports a completed detection for an operation the caller
            // canceled.
            if concluded.status == operation_status::STOPPED
                && concluded.action == operation_action::DETECT_DRIFT
            {
                self.operations[index].drift_detection_status =
                    Some(drift_detection_status::STOPPED.to_string());
            }
            // What a drift detection FOUND becomes the instances' own result only when the
            // detection itself concluded: one the caller canceled reports STOPPED rather
            // than a finding about the stack set.
            if concluded.status == operation_status::SUCCEEDED {
                for drift in &concluded.instance_drifts {
                    if let Some(instance) = self
                        .instances
                        .iter_mut()
                        .find(|held| held.is_in(&drift.account, &drift.region))
                    {
                        instance.drift_status = drift.drift_status.clone();
                        instance.last_drift_check = Some(drift.checked_at);
                    }
                }
            }
            // A deployment that completed leaves the instances it acted on up to date with
            // the stack set: "CURRENT: The stack is currently up to date with the StackSet."
            // A deployment the caller STOPPED owes them the same, for the deployments that
            // were already under way when it was stopped: "StackSets will cancel all the
            // unstarted stack instance deployments and wait for those are in-progress to
            // complete." The ones it cancelled are not among them — a cancelled deployment
            // never completed, so it keeps the CANCELLED state the stop put it in.
            if (concluded.status == operation_status::SUCCEEDED
                || concluded.status == operation_status::STOPPED)
                && (concluded.action == operation_action::CREATE
                    || concluded.action == operation_action::UPDATE)
            {
                for instance in &mut self.instances {
                    if instance.last_operation_id != concluded.operation_id
                        || instance.detailed_status == detailed_status::CANCELLED
                    {
                        continue;
                    }
                    instance.status = instance_status::CURRENT.to_string();
                    instance.detailed_status = detailed_status::SUCCEEDED.to_string();
                }
            }
            // What the operation did in each account and Region it acted on is a record of
            // what it DID, so the conclusion is what freezes it: the results stop moving
            // when the operation ends, and a deletion's results outlive the instances its
            // conclusion takes out of the stack set just below.
            let results = self.results_in_flight(&concluded.operation_id);
            self.operations[index].results = Some(results);
            if concluded.action == operation_action::DELETE {
                self.instances.retain(|instance| {
                    instance.last_operation_id != concluded.operation_id
                        || instance.detailed_status != detailed_status::SUCCEEDED
                });
            }
            // What this conclusion owes the account's default event bus
            // ([`super::stack_set_events`]): the operation's own terminal status, and the
            // status every instance it acted on was left in — each stamped with the instant
            // the conclusion fell due, which is when the changes happened.
            let at = concluded.end.unwrap_or(now);
            let mut owed: Vec<super::stack_set_events::StackSetChange> = Vec::new();
            // A DETECTION reports what an instance has DRIFTED to rather than moving its
            // deployment status, so its conclusion changes no instance status and publishes
            // no instance event; and an instance a deletion removed above has no record
            // left to report a status off.
            if concluded.action != operation_action::DETECT_DRIFT {
                for instance in &self.instances {
                    if instance.last_operation_id != concluded.operation_id {
                        continue;
                    }
                    let Some(stack_id) = instance.stack_id.clone() else {
                        continue;
                    };
                    owed.push(super::stack_set_events::StackSetChange::StackInstance {
                        stack_id,
                        status: instance.status.clone(),
                        detailed_status: instance.detailed_status.clone(),
                        status_reason: instance.status_reason.clone(),
                    });
                }
            }
            owed.push(super::stack_set_events::StackSetChange::Operation {
                operation_id: concluded.operation_id.clone(),
                action: concluded.action.clone(),
                status: concluded.status.clone(),
            });
            for change in owed {
                self.queue_event(change, at);
            }
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
    /// `AdministrationRoleARN`: "the Amazon Resource Name (ARN) of the IAM role used to
    /// perform this StackSet operation", as the stack set carried it when the operation
    /// started.
    pub administration_role_arn: Option<String>,
    /// `ExecutionRoleName`: "the name of the IAM execution role used to create or update
    /// the StackSet", likewise.
    pub execution_role_name: Option<String>,
    /// `RetainStacks`: "for StackSet operations of action type DELETE, specifies whether to
    /// remove the stack instances from the specified StackSet, but doesn't delete the
    /// stacks".
    pub retain_stacks: Option<bool>,
    /// `StackSetDriftDetectionDetails$DriftDetectionStatus` as this operation's own
    /// detection concluded — [`drift_detection_status`]. "This information will only be
    /// present for StackSet operations whose Action type is DETECT_DRIFT", so it is `None`
    /// for every other action and for a detection that has not concluded.
    pub drift_detection_status: Option<String>,
    /// What a `DETECT_DRIFT` operation found, one entry per stack instance it covered —
    /// which is what `ListStackInstanceResourceDrifts` reads back by this operation's id.
    pub instance_drifts: Vec<StoredInstanceDrift>,
    /// What this operation did in each account and Region it acted on, FROZEN at its
    /// conclusion — `None` while it has not concluded.
    ///
    /// An operation's results are a record of what it did, so they stop moving when it
    /// ends: a later operation over the same stack instances cannot rewrite them, and a
    /// deletion's results outlive the instances it removed. While the operation is still in
    /// flight there is nothing to freeze — the results are the live state of the instances
    /// it is acting on, which is what [`StoredStackSet::operation_results`] reads.
    ///
    /// Bounded by the instances the stack set held when the operation concluded, so at most
    /// [`MAX_INSTANCES_PER_STACK_SET`] entries per operation and
    /// [`MAX_OPERATIONS_PER_STACK_SET`] operations hold one: this record adds no growth site
    /// of its own beyond the two ceilings already on them.
    pub results: Option<Vec<StoredOperationResult>>,
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
            administration_role_arn: None,
            execution_role_name: None,
            retain_stacks: None,
            drift_detection_status: None,
            instance_drifts: Vec::new(),
            results: None,
            pending: Some(PendingOperationOutcome {
                status: operation_status::SUCCEEDED.to_string(),
                status_reason: None,
                failed_stack_instances_count: Some(0),
                due_at: now + crate::state::STATUS_TRANSITION_DELAY,
            }),
        }
    }

    /// Materialise the status transition the clock has already made due, if any, and answer
    /// whether this call is the one that applied it.
    ///
    /// Only ever applies a transition whose due instant has passed, so it reports what was
    /// owed at `now` even if nobody had called. The operation ENDING is what stamps
    /// `EndTimestamp`, so that instant is the one that was due rather than now. The answer
    /// is what lets the stack set apply, exactly once, what the operation's CONCLUSION owes
    /// the instances and the stack set itself.
    pub fn settle(&mut self, now: SystemTime) -> bool {
        if self.pending.as_ref().is_none_or(|owed| owed.due_at > now) {
            return false;
        }
        let owed = self.pending.take().expect("checked just above");
        self.status = owed.status;
        self.status_reason = owed.status_reason;
        self.failed_stack_instances_count = owed.failed_stack_instances_count;
        self.end = Some(owed.due_at);
        true
    }
}

/// What a read reports as a stack set's `StackSetDriftDetectionDetails`: "information about
/// the last *completed* drift operation performed on the StackSet".
///
/// Every member is counted off the INSTANCES the stack set holds, which is where a drift
/// result for one lives, so this read and `ListStackInstances` cannot disagree about which
/// instances have drifted. A stack set no detection has covered reports NOT_CHECKED with no
/// detection status and no instant — "this value will be NULL for any StackSet that drift
/// detection hasn't yet been performed on" — and nothing here is in progress, since a
/// detection is in flight only while the operation that started it is.
pub fn drift_detection_details(
    stack_set: &StoredStackSet,
) -> crate::ops::types::StackSetDriftDetectionDetails {
    let instances = &stack_set.instances;
    let counted = |drift_status: &str| -> i32 {
        instances
            .iter()
            .filter(|instance| instance.drift_status == drift_status)
            .count()
            .try_into()
            .unwrap_or(i32::MAX)
    };
    let drifted = counted(super::DRIFT_STATUS_DRIFTED);
    // "FAILED: the drift detection operation failed" for that instance, which is the
    // instance whose own drift result could not be determined.
    let failed = counted(super::DRIFT_STATUS_UNKNOWN);
    let checked = instances
        .iter()
        .filter(|instance| instance.last_drift_check.is_some())
        .count();
    let last_drift_check = instances
        .iter()
        .filter_map(|instance| instance.last_drift_check)
        .max();
    // "DRIFTED: One or more of the stack instances belonging to the StackSet differs from
    // the expected template and parameter configuration"; "IN_SYNC: All of the stack
    // instances belonging to the StackSet stack match"; "NOT_CHECKED: CloudFormation hasn't
    // checked the StackSet for drift."
    let drift_status = if drifted > 0 {
        super::DRIFT_STATUS_DRIFTED
    } else if checked > 0 && checked == instances.len() {
        super::DRIFT_STATUS_IN_SYNC
    } else {
        super::DRIFT_STATUS_NOT_CHECKED
    };
    crate::ops::types::StackSetDriftDetectionDetails {
        drift_status: Some(drift_status.to_string()),
        // The status of the drift-detection OPERATION, which exists only once one has run
        // over this stack set's instances.
        drift_detection_status: last_drift_check
            .map(|_| DRIFT_DETECTION_COMPLETED.to_string()),
        last_drift_check_timestamp: last_drift_check.map(super::timestamp),
        total_stack_instances_count: Some(instances.len().try_into().unwrap_or(i32::MAX)),
        drifted_stack_instances_count: Some(drifted),
        in_sync_stack_instances_count: Some(counted(super::DRIFT_STATUS_IN_SYNC)),
        // Nothing is being checked: a stack instance is in progress only while the
        // detection operation covering it is, and such an operation does not outlive the
        // call that started it here.
        in_progress_stack_instances_count: Some(0),
        failed_stack_instances_count: Some(failed),
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
    /// `ParameterOverrides`: "a list of parameters from the StackSet template whose values
    /// have been overridden in this stack instance" — what the deployment that created it
    /// supplied, which is also what the instance's own stack was deployed with.
    pub parameter_overrides: Vec<StoredParameter>,
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
            parameter_overrides: Vec::new(),
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
