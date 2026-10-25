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
#[path = "all/archive_and_api_destination.rs"]
mod archive_and_api_destination;
#[path = "all/connection_lifecycle.rs"]
mod connection_lifecycle;
#[path = "all/entry_level_semantics.rs"]
mod entry_level_semantics;
#[path = "all/event_bus_configuration.rs"]
mod event_bus_configuration;
#[path = "all/event_bus_deletion.rs"]
mod event_bus_deletion;
#[path = "all/event_bus_lifecycle.rs"]
mod event_bus_lifecycle;
#[path = "all/input_validation.rs"]
mod input_validation;
#[path = "all/missing_resource_wording.rs"]
mod missing_resource_wording;
#[path = "all/partner_event_source.rs"]
mod partner_event_source;
#[path = "all/pattern_and_connection_filters.rs"]
mod pattern_and_connection_filters;
#[path = "all/reserved_event_source.rs"]
mod reserved_event_source;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/rule_lifecycle.rs"]
mod rule_lifecycle;
#[path = "all/rule_state_control.rs"]
mod rule_state_control;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/target_delivery.rs"]
mod target_delivery;
#[path = "all/target_dispatch.rs"]
mod target_dispatch;
#[path = "all/target_response_members.rs"]
mod target_response_members;
