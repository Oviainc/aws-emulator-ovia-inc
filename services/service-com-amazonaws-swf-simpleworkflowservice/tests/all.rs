// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/activity_type_lifecycle.rs"]
mod activity_type_lifecycle;
#[path = "all/activity_type_registration.rs"]
mod activity_type_registration;
#[path = "all/count_open_workflow_executions.rs"]
mod count_open_workflow_executions;
#[path = "all/count_pending_decision_tasks.rs"]
mod count_pending_decision_tasks;
#[path = "all/delete_workflow_type.rs"]
mod delete_workflow_type;
#[path = "all/deprecate_activity_type.rs"]
mod deprecate_activity_type;
#[path = "all/deprecate_workflow_type.rs"]
mod deprecate_workflow_type;
#[path = "all/domain_deprecation.rs"]
mod domain_deprecation;
#[path = "all/domain_tag_resource.rs"]
mod domain_tag_resource;
#[path = "all/domain_tagging.rs"]
mod domain_tagging;
#[path = "all/domain_undeprecation.rs"]
mod domain_undeprecation;
#[path = "all/list_closed_workflow_executions.rs"]
mod list_closed_workflow_executions;
#[path = "all/list_open_workflow_executions.rs"]
mod list_open_workflow_executions;
#[path = "all/pending_activity_task_count.rs"]
mod pending_activity_task_count;
#[path = "all/poll_for_decision_task.rs"]
mod poll_for_decision_task;
#[path = "all/request_cancel_workflow_execution.rs"]
mod request_cancel_workflow_execution;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_resource_value_fidelity.rs"]
mod tag_resource_value_fidelity;
#[path = "all/unknown_resource_messages.rs"]
mod unknown_resource_messages;
#[path = "all/workflow_execution_lifecycle.rs"]
mod workflow_execution_lifecycle;
#[path = "all/workflow_type_undeprecation.rs"]
mod workflow_type_undeprecation;
