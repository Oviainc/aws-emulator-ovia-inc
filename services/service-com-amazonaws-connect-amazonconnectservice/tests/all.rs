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
#[path = "all/contact_flow_catalog.rs"]
mod contact_flow_catalog;
#[path = "all/contact_flow_lifecycle.rs"]
mod contact_flow_lifecycle;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/hours_of_operation_lifecycle.rs"]
mod hours_of_operation_lifecycle;
#[path = "all/instance_default_resources.rs"]
mod instance_default_resources;
#[path = "all/instance_not_found_refusal.rs"]
mod instance_not_found_refusal;
#[path = "all/instance_queue_lifecycle.rs"]
mod instance_queue_lifecycle;
#[path = "all/instance_registry_seam.rs"]
mod instance_registry_seam;
#[path = "all/instance_storage_config.rs"]
mod instance_storage_config;
#[path = "all/phone_number_inventory.rs"]
mod phone_number_inventory;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
