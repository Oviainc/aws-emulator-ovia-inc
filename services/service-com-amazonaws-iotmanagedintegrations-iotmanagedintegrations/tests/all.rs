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
#[path = "all/default_encryption_configuration.rs"]
mod default_encryption_configuration;
#[path = "all/enum_value_set_rendering.rs"]
mod enum_value_set_rendering;
#[path = "all/hub_configuration.rs"]
mod hub_configuration;
#[path = "all/managed_thing_lifecycle.rs"]
mod managed_thing_lifecycle;
#[path = "all/managed_thing_read_side_attributes.rs"]
mod managed_thing_read_side_attributes;
#[path = "all/not_found_messages.rs"]
mod not_found_messages;
#[path = "all/ota_task_configuration_lifecycle.rs"]
mod ota_task_configuration_lifecycle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/validation_error_messages.rs"]
mod validation_error_messages;
