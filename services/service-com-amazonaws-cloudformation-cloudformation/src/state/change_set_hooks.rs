// SPDX-License-Identifier: CC0-1.0

//! The Hook invocations PLANNED for one change set, which `DescribeChangeSetHooks`
//! reports: "Returns Hook-related information for the change set and a list of changes that
//! CloudFormation makes when you run the change set."
//!
//! A planned invocation belongs to the change set it was planned for, not to the account:
//! it names the Hook that would run, the point it would run at, what happens if it fails,
//! and the TARGET it would be evaluated against — one resource of the change set's change
//! list. Everything `DescribeChangeSetHooks` reports about one therefore lives on a single
//! [`StoredChangeSetHook`], held on [`crate::state::StoredChangeSet::hooks`], so the read
//! computes its answer from the record rather than from the request.
//!
//! ## What writes it
//!
//! CloudFormation plans the invocations while it CREATES the change set, one per
//! (activated Hook, targeted resource) pair: a Hook is planned only when the account has
//! activated it and its configuration targets the operation the change set would perform.
//! Nothing in this emulator can put a Hook in that position — the registry writers that
//! activate an extension and set its configuration are not implemented, and a Hook's
//! verdict comes from running its own handler, which cannot run here (see
//! [`crate::state::hook_results`] for the same reasoning about an invocation's RESULT). A
//! change set created here therefore plans no invocation, and that is what the read
//! reports: the empty list, with the planning status that goes with it
//! ([`crate::state::StoredChangeSet::hooks_status`]).
//!
//! [`crate::state::StoredChangeSet::plan_hook`] is the one way a record enters a change
//! set, so the planning path this emulator grows later — and the suites that cover the read
//! over records no wire call can create — write through it rather than reaching into the
//! vector.

/// How many planned invocations one change set holds, oldest reclaimed first.
///
/// Every growth site in this service's store is bounded. CloudFormation documents no
/// quota on the invocations one change set plans, so this is a COUNT chosen for this
/// store: a Hook may be planned against every resource of a change set, so it is above
/// [`crate::state::template::MAX_RESOURCES_PER_TEMPLATE`] rather than near it.
pub const MAX_CHANGE_SET_HOOKS: usize = 1_000;

/// The model's `HookTargetType` enum: the kind of thing a planned invocation is evaluated
/// against (`smithy_query(query="shape", shape="HookTargetType")` — one value).
///
/// A DIFFERENT enum from the `ListHookResultsTargetType` of
/// [`crate::state::hook_results::target_type`], which a recorded invocation RESULT carries:
/// the two share a value name and nothing else, so neither module may be folded into the
/// other.
pub mod target_type {
    pub const RESOURCE: &str = "RESOURCE";
}

/// The model's `ChangeSetHooksStatus` enum: "Provides the status of the change set Hook."
///
/// The plan is computed while the change set is created, so the status is a statement about
/// THAT work: it is being computed, it has been computed, or there is no computed plan to
/// report ([`crate::state::StoredChangeSet::hooks_status`] is where a change set is mapped
/// onto one of them).
pub mod hooks_status {
    /// The plan is still being computed, because the change set's own creation is.
    pub const PLANNING: &str = "PLANNING";
    /// The plan has been computed, and is what the response carries.
    pub const PLANNED: &str = "PLANNED";
    /// There is no computed plan: the change set's creation never completed, so nothing
    /// planned invocations for it.
    pub const UNAVAILABLE: &str = "UNAVAILABLE";
}

/// ONE planned Hook invocation of one change set, carrying every member the modelled
/// `ChangeSetHook` reports.
#[derive(Debug, Clone)]
pub struct StoredChangeSetHook {
    /// `InvocationPoint`: the point "in the provisioning process where the Hook is
    /// invoked" ([`crate::state::hook_results::invocation_point`]).
    pub invocation_point: String,
    /// `FailureMode`: what happens to the operation when the Hook fails
    /// ([`crate::state::hook_results::failure_mode`]).
    pub failure_mode: String,
    /// `TypeName`: the name of the Hook that would be invoked.
    pub type_name: String,
    /// `TypeVersionId`: the version of that Hook.
    pub type_version_id: String,
    /// `TypeConfigurationVersionId`: the version of the Hook's configuration, where the
    /// plan carries one.
    pub type_configuration_version_id: Option<String>,
    /// `TargetDetails$TargetType`: [`target_type`].
    pub target_type: String,
    /// `ResourceTargetDetails$LogicalResourceId`: the logical id of the resource this
    /// invocation is planned against, which is also what the request's `LogicalResourceId`
    /// filters on.
    pub logical_resource_id: String,
    /// `ResourceTargetDetails$ResourceType`: that resource's type.
    pub resource_type: String,
    /// `ResourceTargetDetails$ResourceAction`: the `ChangeAction` the change set would
    /// perform on that resource.
    pub resource_action: String,
}

impl StoredChangeSetHook {
    /// A planned invocation of `type_name` against one resource of the change set, at the
    /// one invocation point the model admits and with the failure mode that fails the
    /// operation; the optional configuration version is left for a caller to fill in.
    pub fn new(
        type_name: impl Into<String>,
        type_version_id: impl Into<String>,
        logical_resource_id: impl Into<String>,
        resource_type: impl Into<String>,
        resource_action: impl Into<String>,
    ) -> Self {
        Self {
            invocation_point: crate::state::hook_results::invocation_point::PRE_PROVISION
                .to_string(),
            failure_mode: crate::state::hook_results::failure_mode::FAIL.to_string(),
            type_name: type_name.into(),
            type_version_id: type_version_id.into(),
            type_configuration_version_id: None,
            target_type: target_type::RESOURCE.to_string(),
            logical_resource_id: logical_resource_id.into(),
            resource_type: resource_type.into(),
            resource_action: resource_action.into(),
        }
    }

    /// Whether this record is one the request's `LogicalResourceId` selects: "If specified,
    /// lists only the Hooks related to the specified LogicalResourceId."
    pub fn targets(&self, logical_resource_id: &str) -> bool {
        self.logical_resource_id == logical_resource_id
    }
}
