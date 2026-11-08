// SPDX-License-Identifier: CC0-1.0

//! The Hook INVOCATION RESULTS this account and Region holds: one record per Hook
//! invocation, which `ListHookResults` reports summaries of.
//!
//! A Hook invocation result is a record of its own, not part of a stack: `ListHookResults`
//! "returns summaries of invoked Hooks" for the whole account and Region, filtered by the
//! Hook (`TypeArn`, optionally with `Status`) or by the invocation TARGET (`TargetId` with
//! `TargetType`), and `GetHookResult` "retrieves detailed information and remediation
//! guidance for a Hook invocation result" — ONE record, addressed by its
//! `HookResultId`. Everything either read reports therefore lives on one
//! [`StoredHookResult`], so a read computes its answer from the record rather than from the
//! request: the summary members `ListHookResults` reports, plus the ones only the detailed
//! read carries ([`StoredHookResult::original_type_name`], [`StoredHookResult::target`] and
//! [`StoredHookResult::annotations`]).
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

/// How many annotations ONE invocation result keeps, oldest reclaimed first.
///
/// A record's annotations are a growth site of their own — an invocation produces one per
/// piece of "additional information and guidance" its evaluation reports — so the count is
/// bounded beside [`MAX_HOOK_RESULTS`]. The model documents no quota on the list
/// (`AnnotationList` carries no `@length`), so this is a bound on the store rather than a
/// claim about the real service.
pub const MAX_ANNOTATIONS_PER_RESULT: usize = 50;

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

/// The message CloudFormation answers a request that filters on a `TargetType` the model
/// admits but the operation does not FILTER on.
///
/// `ListHookResultsTargetType` carries four values, so such a value is a well-formed one the
/// operation refuses at RUN time: the sentence is the fixed text `Invalid TargetType: ` and
/// the value the request named — the one part of it that comes from the request — and it
/// states nothing about which values are filterable, because the service's own refusal does
/// not ([`FILTERABLE_TARGET_TYPES`] is what decides, not what is reported).
pub fn invalid_target_type(target_type: &str) -> String {
    format!("Invalid TargetType: {target_type}")
}

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

/// The model's `HookTargetAction` enum: "the action that invoked the Hook"
/// (`smithy_query(query="shape", shape="HookTargetAction")`).
pub mod target_action {
    pub const CREATE: &str = "CREATE";
    pub const UPDATE: &str = "UPDATE";
    pub const DELETE: &str = "DELETE";
    pub const IMPORT: &str = "IMPORT";
}

/// The model's `AnnotationStatus` enum: "the status of the Hook invocation from the
/// downstream service" (`smithy_query(query="shape", shape="AnnotationStatus")`).
pub mod annotation_status {
    pub const PASSED: &str = "PASSED";
    pub const FAILED: &str = "FAILED";
    pub const SKIPPED: &str = "SKIPPED";
}

/// The model's `AnnotationSeverityLevel` enum: "the relative risk associated with any
/// violations of this type"
/// (`smithy_query(query="shape", shape="AnnotationSeverityLevel")`).
pub mod annotation_severity_level {
    pub const INFORMATIONAL: &str = "INFORMATIONAL";
    pub const LOW: &str = "LOW";
    pub const MEDIUM: &str = "MEDIUM";
    pub const HIGH: &str = "HIGH";
    pub const CRITICAL: &str = "CRITICAL";
}

/// The TARGET one invocation was evaluated against, as the modelled `HookTarget` reports
/// it: "information about the target of the Hook invocation", whose four members the model
/// all marks `@required`.
///
/// Distinct from [`StoredHookResult::target_type`] / [`StoredHookResult::target_id`], which
/// are the invocation's `ListHookResultsTargetType` target (the change set or Cloud Control
/// operation the invocation belongs to): this is the single RESOURCE the Hook's evaluation
/// ran against, the way a planned invocation's `ResourceTargetDetails` names one
/// ([`crate::state::change_set_hooks`]).
#[derive(Debug, Clone)]
pub struct StoredHookTarget {
    /// `TargetType`: "the target type", the model's `HookTargetType`
    /// ([`crate::state::change_set_hooks::target_type`], which is that same enum).
    pub target_type: String,
    /// `TargetTypeName`: "the target name, for example, `AWS::S3::Bucket`".
    pub target_type_name: String,
    /// `TargetId`: "the unique identifier of the Hook invocation target".
    pub target_id: String,
    /// `Action`: [`target_action`].
    pub action: String,
}

impl StoredHookTarget {
    /// The target of one invocation: the resource type and id the Hook was evaluated
    /// against, and the action that invoked it.
    pub fn new(
        target_type_name: impl Into<String>,
        target_id: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        Self {
            target_type: crate::state::change_set_hooks::target_type::RESOURCE.to_string(),
            target_type_name: target_type_name.into(),
            target_id: target_id.into(),
            action: action.into(),
        }
    }
}

/// ONE annotation of an invocation result, as the modelled `Annotation` reports it: "a
/// `GetHookResult` call returns detailed information and remediation guidance from Control
/// Tower, Guard, Lambda, or custom Hooks for a Hook invocation result."
#[derive(Debug, Clone)]
pub struct StoredAnnotation {
    /// `AnnotationName`: "an identifier for the evaluation logic that was used when
    /// invoking the Hook" — the control id, the rule id, or the Hook's own identifier.
    pub annotation_name: Option<String>,
    /// `Status`: [`annotation_status`].
    pub status: Option<String>,
    /// `StatusMessage`: "the explanation for the specific status assigned to this Hook
    /// invocation".
    pub status_message: Option<String>,
    /// `RemediationMessage`: "suggests what to change if your Hook returns a `FAILED`
    /// status".
    pub remediation_message: Option<String>,
    /// `RemediationLink`: "a URL that you can access for additional remediation guidance".
    pub remediation_link: Option<String>,
    /// `SeverityLevel`: [`annotation_severity_level`].
    pub severity_level: Option<String>,
}

impl StoredAnnotation {
    /// The annotation `annotation_name`'s evaluation produced, in `status`, with the
    /// guidance members left for a caller to fill in.
    pub fn new(annotation_name: impl Into<String>, status: impl Into<String>) -> Self {
        Self {
            annotation_name: Some(annotation_name.into()),
            status: Some(status.into()),
            status_message: None,
            remediation_message: None,
            remediation_link: None,
            severity_level: None,
        }
    }
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
    /// `OriginalTypeName`: "the original public type name of the Hook when an alias is
    /// used", which only an invocation of an aliased Hook carries.
    pub original_type_name: Option<String>,
    /// `Target`: "information about the target of the Hook invocation", reported by the
    /// detailed read only.
    pub target: Option<StoredHookTarget>,
    /// `Annotations`: "a list of objects with additional information and guidance that can
    /// help you resolve a failed Hook invocation", bounded by
    /// [`MAX_ANNOTATIONS_PER_RESULT`].
    pub annotations: Vec<StoredAnnotation>,
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
            original_type_name: None,
            target: None,
            annotations: Vec::new(),
        }
    }

    /// Whether this record is the one `hook_result_id` addresses: "the unique identifier
    /// (ID) of the Hook invocation result that you want details about."
    pub fn has_id(&self, hook_result_id: &str) -> bool {
        self.hook_result_id == hook_result_id
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
