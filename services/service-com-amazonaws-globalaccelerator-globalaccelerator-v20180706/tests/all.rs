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
#[path = "all/accelerator_attributes.rs"]
mod accelerator_attributes;
#[path = "all/accelerator_lifecycle.rs"]
mod accelerator_lifecycle;
#[path = "all/endpoint_groups.rs"]
mod endpoint_groups;
#[path = "all/error_message_member.rs"]
mod error_message_member;
#[path = "all/input_refusals.rs"]
mod input_refusals;
#[path = "all/not_found_error_status.rs"]
mod not_found_error_status;
#[path = "all/resource_projection.rs"]
mod resource_projection;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
