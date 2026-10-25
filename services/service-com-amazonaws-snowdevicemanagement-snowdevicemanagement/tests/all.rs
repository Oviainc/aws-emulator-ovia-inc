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
#[path = "all/account_device_gate.rs"]
mod account_device_gate;
#[path = "all/create_task_tag_validation.rs"]
mod create_task_tag_validation;
#[path = "all/device_ec2_instances.rs"]
mod device_ec2_instances;
#[path = "all/device_resources.rs"]
mod device_resources;
#[path = "all/device_task_lifecycle.rs"]
mod device_task_lifecycle;
#[path = "all/empty_inventory_refusals.rs"]
mod empty_inventory_refusals;
#[path = "all/resource_tag_removal.rs"]
mod resource_tag_removal;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/unknown_resource_id.rs"]
mod unknown_resource_id;
