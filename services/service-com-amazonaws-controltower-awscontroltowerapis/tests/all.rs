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
#[path = "all/baseline_catalog.rs"]
mod baseline_catalog;
#[path = "all/baseline_operations.rs"]
mod baseline_operations;
#[path = "all/control_operations.rs"]
mod control_operations;
#[path = "all/disable_baseline.rs"]
mod disable_baseline;
#[path = "all/enable_baseline_persistence.rs"]
mod enable_baseline_persistence;
#[path = "all/enable_baseline_validation.rs"]
mod enable_baseline_validation;
#[path = "all/enable_control_persistence.rs"]
mod enable_control_persistence;
#[path = "all/enable_control_validation.rs"]
mod enable_control_validation;
#[path = "all/enabled_baseline_update.rs"]
mod enabled_baseline_update;
#[path = "all/enabled_control_lifecycle.rs"]
mod enabled_control_lifecycle;
#[path = "all/enabled_control_reset.rs"]
mod enabled_control_reset;
#[path = "all/enabled_control_update.rs"]
mod enabled_control_update;
#[path = "all/front_door_validation.rs"]
mod front_door_validation;
#[path = "all/get_enabled_control.rs"]
mod get_enabled_control;
#[path = "all/landing_zone_identifier.rs"]
mod landing_zone_identifier;
#[path = "all/landing_zone_lifecycle.rs"]
mod landing_zone_lifecycle;
#[path = "all/landing_zone_reset.rs"]
mod landing_zone_reset;
#[path = "all/landing_zone_update.rs"]
mod landing_zone_update;
#[path = "all/landing_zone_validation.rs"]
mod landing_zone_validation;
#[path = "all/list_baselines.rs"]
mod list_baselines;
#[path = "all/list_enabled_baselines.rs"]
mod list_enabled_baselines;
#[path = "all/list_tags_for_resource.rs"]
mod list_tags_for_resource;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
