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
#[path = "all/control_plane_rules.rs"]
mod control_plane_rules;
#[path = "all/emulator.rs"]
mod emulator;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/scalable_target_lifecycle.rs"]
mod scalable_target_lifecycle;
#[path = "all/scalable_target_resolution.rs"]
mod scalable_target_resolution;
#[path = "all/scaling_activities.rs"]
mod scaling_activities;
#[path = "all/scheduled_action_lifecycle.rs"]
mod scheduled_action_lifecycle;
#[path = "all/scheduled_actions.rs"]
mod scheduled_actions;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tags.rs"]
mod tags;
#[path = "all/target_tracking_alarms.rs"]
mod target_tracking_alarms;
