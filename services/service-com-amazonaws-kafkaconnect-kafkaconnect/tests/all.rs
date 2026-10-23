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
#[path = "all/custom_plugin_and_connector_lifecycle.rs"]
mod custom_plugin_and_connector_lifecycle;
#[path = "all/custom_plugin_file_location.rs"]
mod custom_plugin_file_location;
#[path = "all/duplicate_name_conflict_message.rs"]
mod duplicate_name_conflict_message;
#[path = "all/iso8601_timestamps.rs"]
mod iso8601_timestamps;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/validation_messages.rs"]
mod validation_messages;
#[path = "all/worker_configuration_create_and_describe.rs"]
mod worker_configuration_create_and_describe;
#[path = "all/worker_configuration_delete_and_tags.rs"]
mod worker_configuration_delete_and_tags;
