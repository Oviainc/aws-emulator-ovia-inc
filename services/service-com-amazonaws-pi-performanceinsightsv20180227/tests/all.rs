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
#[path = "all/authorization_gate.rs"]
mod authorization_gate;
#[path = "all/available_resource_metrics.rs"]
mod available_resource_metrics;
#[path = "all/input_validation.rs"]
mod input_validation;
#[path = "all/performance_analysis_reports.rs"]
mod performance_analysis_reports;
#[path = "all/pi_harness.rs"]
mod pi_harness;
#[path = "all/resource_metrics.rs"]
mod resource_metrics;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
