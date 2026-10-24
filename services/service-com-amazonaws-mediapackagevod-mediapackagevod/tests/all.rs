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
#[path = "all/arn_documented_members.rs"]
mod arn_documented_members;
#[path = "all/asset_listing.rs"]
mod asset_listing;
#[path = "all/configure_logs.rs"]
mod configure_logs;
#[path = "all/describe_packaging_configuration.rs"]
mod describe_packaging_configuration;
#[path = "all/error_responses.rs"]
mod error_responses;
#[path = "all/packaging_configuration_deletion.rs"]
mod packaging_configuration_deletion;
#[path = "all/packaging_configuration_listing.rs"]
mod packaging_configuration_listing;
#[path = "all/packaging_group_deletion.rs"]
mod packaging_group_deletion;
#[path = "all/packaging_group_update.rs"]
mod packaging_group_update;
#[path = "all/packaging_lifecycle.rs"]
mod packaging_lifecycle;
#[path = "all/region_scoping.rs"]
mod region_scoping;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/success_path_projections.rs"]
mod success_path_projections;
