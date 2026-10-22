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
#[path = "all/bill_scenario_lifecycle.rs"]
mod bill_scenario_lifecycle;
#[path = "all/preferences.rs"]
mod preferences;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/validation_refusal_framing.rs"]
mod validation_refusal_framing;
#[path = "all/workload_estimate_deletion.rs"]
mod workload_estimate_deletion;
#[path = "all/workload_estimate_lifecycle.rs"]
mod workload_estimate_lifecycle;
#[path = "all/workload_estimate_listing.rs"]
mod workload_estimate_listing;
#[path = "all/workload_estimate_usage.rs"]
mod workload_estimate_usage;
