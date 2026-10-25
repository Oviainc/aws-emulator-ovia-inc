// SPDX-License-Identifier: CC0-1.0

//! The Hook INVOCATION RESULTS this account and Region holds: one record per Hook
//! invocation, which `ListHookResults` reports summaries of.
//!
//! A Hook invocation result is a record of its own, not part of a stack: `ListHookResults`
//! "returns summaries of invoked Hooks" for the whole account and Region, filtered by the
//! Hook (`TypeArn`, optionally with `Status`) or by the invocation TARGET (`TargetId` with
//! `TargetType`). Everything either read reports therefore lives on one
//! [`StoredHookResult`], so a read computes its answer from the record rather than from the
//! request.
//!
//! ## What writes it
//!
//! CloudFormation records a result when it INVOKES a Hook — which happens only for a Hook
//! the account has activated, at the invocation points that Hook's configuration targets.
//! Nothing in this emulator can invoke one: a Hook's verdict is produced by running the
//! Hook's own handler (a Guard rule set, a Lambda function, a registered Hook's handler
//! package), and no such handler exists here, so recording a status for one would be
//! reporting work that nothing ran. An account whose Hooks have never been invoked is
//! exactly what this store holds, and that is what the reads report: the empty list, for
//! every filter a request can name.
//!
//! [`crate::state::ComAmazonawsCloudformationCloudformationState::record_hook_result`] is
//! the one way a record enters the store, so the invocation path that this emulator grows
//! later — and the suites that cover the reads over records a bound's worth of wire calls
//! cannot create — write through it rather than reaching into the vector.

use std::time::SystemTime;

/// How many invocation results this store keeps, oldest reclaimed first.
///
/// Every growth site in this service's store is bounded. The model documents no retention
/// for an invocation result, so this is a COUNT rather than an age: a bound on the store,
/// not a claim about how long the real service keeps a result.
pub const MAX_HOOK_RESULTS: usize = 500;

/// The model's `HookStatus` enum: "HOOK_IN_PROGRESS: The Hook is currently running.
/// HOOK_COMPLETE_SUCCEEDED: The Hook completed successfully. HOOK_COMPLETE_FAILED: The Hook
/// completed but failed validation. HOOK_FAILED: The Hook encountered an error during
/// execution."
pub mod hook_status {
    pub const IN_PROGRESS: &str = "HOOK_IN_PROGRESS";
    pub const COMPLETE_SUCCEEDED: &str = "HOOK_COMPLETE_SUCCEEDED";
    pub const COMPLETE_FAILED: &str = "HOOK_COMPLETE_FAILED";
    pub const FAILED: &str = "HOOK_FAILED";
}

/// The model's `ListHookResultsTargetType` enum
/// (`smithy_query(query="shape", shape="ListHookResultsTargetType")`).
pub mod target_type {
    pub const CHANGE_SET: &str = "CHANGE_SET";
    pub const STACK: &str = "STACK";
    pub const RESOURCE: &str = "RESOURCE";
    pub const CLOUD_CONTROL: &str = "CLOUD_CONTROL";
}

/// The target types that may be used as a FILTER, which is fewer than the ones a record can
/// carry: "Currently, only CHANGE_SET and CLOUD_CONTROL are supported filter options"
/// (`ListHookResultsInput$TargetType`).
pub const FILTERABLE_TARGET_TYPES: &[&str] = &[target_type::CHANGE_SET, target_type::CLOUD_CONTROL];

/// The model's `HookInvocationPoint` enum: the single point "in the provisioning process
/// where the Hook is invoked".
pub mod invocation_point {
    pub const PRE_PROVISION: &str = "PRE_PROVISION";
}

/// The model's `HookFailureMode` enum: "FAIL: ... fails the operation" / "WARN: ... allows
/// the operation to continue".
pub mod failure_mode {
    pub const FAIL: &str = "FAIL";
    pub const WARN: &str = "WARN";
}

/// ONE recorded Hook invocation, carrying every member `HookResultSummary` reports and the
/// Region whose account holds it.
#[derive(Debug, Clone)]
pub struct StoredHookResult {
    /// `HookResultId`: "the unique identifier for this Hook invocation result", in the
    /// modelled `HookInvocationId` 8-4-4-4-12 form.
    pub hook_result_id: String,
    /// The Region this invocation was recorded in. A read answers for the Region its
    /// request was signed for, the way every other read of this service does.
    pub region: String,
    /// `InvocationPoint`: [`invocation_point`].
    pub invocation_point: String,
    /// `FailureMode`: [`failure_mode`].
    pub failure_mode: String,
    /// `TypeName`: "the name of the Hook that was invoked".
    pub type_name: String,
    /// `TypeArn`: "the ARN of the Hook that was invoked", which is also what `TypeArn`
    /// filters on.
    pub type_arn: String,
    /// `TypeVersionId`: "the version of the Hook that was invoked".
    pub type_version_id: String,
    /// `TypeConfigurationVersionId`: "the version of the Hook configuration".
    pub type_configuration_version_id: Option<String>,
    /// `Status`: [`hook_status`].
    pub status: String,
    /// `HookStatusReason`: "a description of the Hook results status".
    pub hook_status_reason: Option<String>,
    /// `InvokedAt`: "the timestamp when the Hook was invoked".
    pub invoked_at: SystemTime,
    /// `TargetType`: "the target type that the Hook was invoked against"
    /// ([`target_type`]).
    pub target_type: String,
    /// `TargetId`: "the unique identifier of the Hook invocation target" — the change set
    /// ARN for a change set, the `HookRequestToken` for a Cloud Control API operation.
    pub target_id: String,
    /// `HookExecutionTarget`: "the Amazon Resource Name (ARN) of the target stack or
    /// request token of the Cloud Control API operation".
    pub hook_execution_target: Option<String>,
}

impl StoredHookResult {
    /// A record carrying the members every invocation has, with the optional ones left
    /// unset for a caller to fill in.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        hook_result_id: impl Into<String>,
        region: impl Into<String>,
        type_name: impl Into<String>,
        type_arn: impl Into<String>,
        type_version_id: impl Into<String>,
        status: impl Into<String>,
        target_type: impl Into<String>,
        target_id: impl Into<String>,
        invoked_at: SystemTime,
    ) -> Self {
        Self {
            hook_result_id: hook_result_id.into(),
            region: region.into(),
            invocation_point: invocation_point::PRE_PROVISION.to_string(),
            failure_mode: failure_mode::FAIL.to_string(),
            type_name: type_name.into(),
            type_arn: type_arn.into(),
            type_version_id: type_version_id.into(),
            type_configuration_version_id: None,
            status: status.into(),
            hook_status_reason: None,
            invoked_at,
            target_type: target_type.into(),
            target_id: target_id.into(),
            hook_execution_target: None,
        }
    }

    /// Whether this record is one the `TypeArn` filter selects: "Filters results by the ARN
    /// of the Hook."
    pub fn matches_type_arn(&self, type_arn: &str) -> bool {
        self.type_arn == type_arn
    }

    /// Whether this record is one the `TargetId`/`TargetType` filter selects: "Filters
    /// results by the unique identifier of the target the Hook was invoked against" at
    /// "a specific Hook invocation target".
    pub fn matches_target(&self, target_type: &str, target_id: &str) -> bool {
        self.target_type == target_type && self.target_id == target_id
    }
}
