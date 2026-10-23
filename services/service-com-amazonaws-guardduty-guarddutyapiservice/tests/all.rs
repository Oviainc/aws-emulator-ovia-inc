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
#[path = "all/client_rejections.rs"]
mod client_rejections;
#[path = "all/detector_data_sources_projection.rs"]
mod detector_data_sources_projection;
#[path = "all/detector_lifecycle.rs"]
mod detector_lifecycle;
#[path = "all/investigation_lifecycle.rs"]
mod investigation_lifecycle;
#[path = "all/sample_finding_catalog.rs"]
mod sample_finding_catalog;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
