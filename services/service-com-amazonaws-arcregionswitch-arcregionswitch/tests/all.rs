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
#[path = "all/approve_plan_execution_step.rs"]
mod approve_plan_execution_step;
#[path = "all/execution_progression.rs"]
mod execution_progression;
#[path = "all/get_plan_in_region.rs"]
mod get_plan_in_region;
#[path = "all/plan_execution_listing.rs"]
mod plan_execution_listing;
#[path = "all/plan_lifecycle.rs"]
mod plan_lifecycle;
#[path = "all/plan_update.rs"]
mod plan_update;
#[path = "all/route53_health_checks.rs"]
mod route53_health_checks;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
