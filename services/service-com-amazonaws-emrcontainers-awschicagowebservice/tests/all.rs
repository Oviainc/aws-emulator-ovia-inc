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
#[path = "all/created_metadata_wire_form.rs"]
mod created_metadata_wire_form;
#[path = "all/job_template_idempotency.rs"]
mod job_template_idempotency;
#[path = "all/job_template_lifecycle.rs"]
mod job_template_lifecycle;
#[path = "all/security_configuration.rs"]
mod security_configuration;
#[path = "all/security_configuration_delete.rs"]
mod security_configuration_delete;
#[path = "all/security_configuration_list.rs"]
mod security_configuration_list;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/virtual_cluster_job_run_loop.rs"]
mod virtual_cluster_job_run_loop;
#[path = "all/virtual_cluster_scheduler_status.rs"]
mod virtual_cluster_scheduler_status;
