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
#[path = "all/address_validation.rs"]
mod address_validation;
#[path = "all/cluster_cancellation.rs"]
mod cluster_cancellation;
#[path = "all/cluster_creation.rs"]
mod cluster_creation;
#[path = "all/cluster_device_capability.rs"]
mod cluster_device_capability;
#[path = "all/cluster_job_listing.rs"]
mod cluster_job_listing;
#[path = "all/cluster_node_updates.rs"]
mod cluster_node_updates;
#[path = "all/cluster_updates.rs"]
mod cluster_updates;
#[path = "all/export_job_parts.rs"]
mod export_job_parts;
#[path = "all/job_lifecycle.rs"]
mod job_lifecycle;
#[path = "all/job_manifest.rs"]
mod job_manifest;
#[path = "all/job_order_validation.rs"]
mod job_order_validation;
#[path = "all/job_unlock_code.rs"]
mod job_unlock_code;
#[path = "all/job_updates.rs"]
mod job_updates;
#[path = "all/return_shipping_labels.rs"]
mod return_shipping_labels;
#[path = "all/service_versions.rs"]
mod service_versions;
#[path = "all/software_updates.rs"]
mod software_updates;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
