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
#[path = "all/core_network_lifecycle.rs"]
mod core_network_lifecycle;
#[path = "all/delete_dependency_refusal.rs"]
mod delete_dependency_refusal;
#[path = "all/device_lifecycle.rs"]
mod device_lifecycle;
#[path = "all/filter_id_list_maximum.rs"]
mod filter_id_list_maximum;
#[path = "all/global_network_lifecycle.rs"]
mod global_network_lifecycle;
#[path = "all/link_lifecycle.rs"]
mod link_lifecycle;
#[path = "all/list_tags_for_resource.rs"]
mod list_tags_for_resource;
#[path = "all/network_resource_counts.rs"]
mod network_resource_counts;
#[path = "all/resource_not_found_message.rs"]
mod resource_not_found_message;
#[path = "all/resource_policy_lifecycle.rs"]
mod resource_policy_lifecycle;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/unknown_parent_reference.rs"]
mod unknown_parent_reference;
