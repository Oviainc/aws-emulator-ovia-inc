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
#[path = "all/activity_type_registration.rs"]
mod activity_type_registration;
#[path = "all/domain_deprecation.rs"]
mod domain_deprecation;
#[path = "all/domain_tagging.rs"]
mod domain_tagging;
#[path = "all/list_open_workflow_executions.rs"]
mod list_open_workflow_executions;
#[path = "all/request_cancel_workflow_execution.rs"]
mod request_cancel_workflow_execution;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/workflow_execution_lifecycle.rs"]
mod workflow_execution_lifecycle;
