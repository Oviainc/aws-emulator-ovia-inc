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
#[path = "all/cost_optimization_hub.rs"]
mod cost_optimization_hub;
#[path = "all/management_account_gating.rs"]
mod management_account_gating;
#[path = "all/max_results_bounds.rs"]
mod max_results_bounds;
#[path = "all/preference_writes.rs"]
mod preference_writes;
#[path = "all/recommendation_summaries.rs"]
mod recommendation_summaries;
#[path = "all/rejection_fidelity.rs"]
mod rejection_fidelity;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/summary_metrics_and_refusals.rs"]
mod summary_metrics_and_refusals;
#[path = "all/validation_exception_fields.rs"]
mod validation_exception_fields;
