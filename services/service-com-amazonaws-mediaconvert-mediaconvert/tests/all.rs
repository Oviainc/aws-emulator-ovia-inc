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
#[path = "all/account_endpoints.rs"]
mod account_endpoints;
#[path = "all/error_envelope.rs"]
mod error_envelope;
#[path = "all/job_lifecycle.rs"]
mod job_lifecycle;
#[path = "all/job_template_crud.rs"]
mod job_template_crud;
#[path = "all/job_templates.rs"]
mod job_templates;
#[path = "all/jobs_query.rs"]
mod jobs_query;
#[path = "all/policy_lifecycle.rs"]
mod policy_lifecycle;
#[path = "all/preset_lifecycle.rs"]
mod preset_lifecycle;
#[path = "all/preset_update.rs"]
mod preset_update;
#[path = "all/queue_deletion.rs"]
mod queue_deletion;
#[path = "all/resource_projection_defaults.rs"]
mod resource_projection_defaults;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
