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
#[path = "all/action_type_catalog.rs"]
mod action_type_catalog;
#[path = "all/pipeline_lifecycle.rs"]
mod pipeline_lifecycle;
#[path = "all/stage_transitions.rs"]
mod stage_transitions;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/third_party_jobs.rs"]
mod third_party_jobs;
#[path = "all/update_pipeline.rs"]
mod update_pipeline;
