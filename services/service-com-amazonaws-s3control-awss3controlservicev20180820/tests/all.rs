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
#[path = "all/access_grants_instance.rs"]
mod access_grants_instance;
#[path = "all/access_point_lifecycle.rs"]
mod access_point_lifecycle;
#[path = "all/batch_job_lifecycle.rs"]
mod batch_job_lifecycle;
#[path = "all/multi_region_access_point_lifecycle.rs"]
mod multi_region_access_point_lifecycle;
#[path = "all/public_access_block.rs"]
mod public_access_block;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/storage_lens_configuration.rs"]
mod storage_lens_configuration;
#[path = "all/storage_lens_group.rs"]
mod storage_lens_group;
