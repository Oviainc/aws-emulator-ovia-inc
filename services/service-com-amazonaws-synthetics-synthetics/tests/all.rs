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
#[path = "all/canary_lifecycle.rs"]
mod canary_lifecycle;
#[path = "all/group_deletion.rs"]
mod group_deletion;
#[path = "all/group_lifecycle.rs"]
mod group_lifecycle;
#[path = "all/group_membership.rs"]
mod group_membership;
#[path = "all/input_validation.rs"]
mod input_validation;
#[path = "all/missing_resource_errors.rs"]
mod missing_resource_errors;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/runtime_versions.rs"]
mod runtime_versions;
#[path = "all/server_side_validation.rs"]
mod server_side_validation;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
