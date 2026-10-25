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
#[path = "all/arn_refusal_single_source.rs"]
mod arn_refusal_single_source;
#[path = "all/control_plane_fidelity.rs"]
mod control_plane_fidelity;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/plan_capacity_quota.rs"]
mod plan_capacity_quota;
#[path = "all/plan_capacity_units.rs"]
mod plan_capacity_units;
#[path = "all/plan_update.rs"]
mod plan_update;
#[path = "all/regional_isolation.rs"]
mod regional_isolation;
#[path = "all/rescore_execution_plan_lifecycle.rs"]
mod rescore_execution_plan_lifecycle;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_character_set.rs"]
mod tag_character_set;
#[path = "all/wire_error_status.rs"]
mod wire_error_status;
