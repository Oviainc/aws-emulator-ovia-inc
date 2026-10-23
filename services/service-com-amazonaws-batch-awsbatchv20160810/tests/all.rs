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
#[path = "all/bounds.rs"]
mod bounds;
#[path = "all/cancel_job.rs"]
mod cancel_job;
#[path = "all/consumable_resources.rs"]
mod consumable_resources;
#[path = "all/job_definition_deregistration.rs"]
mod job_definition_deregistration;
#[path = "all/job_definition_registry.rs"]
mod job_definition_registry;
#[path = "all/job_lifecycle.rs"]
mod job_lifecycle;
#[path = "all/job_queue_updates.rs"]
mod job_queue_updates;
#[path = "all/scheduling_policies.rs"]
mod scheduling_policies;
#[path = "all/service_environment.rs"]
mod service_environment;
#[path = "all/service_job_termination.rs"]
mod service_job_termination;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
