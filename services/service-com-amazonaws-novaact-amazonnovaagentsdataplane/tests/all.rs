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
#[path = "all/act_listing.rs"]
mod act_listing;
#[path = "all/act_status_enum_refusal.rs"]
mod act_status_enum_refusal;
#[path = "all/act_status_update.rs"]
mod act_status_update;
#[path = "all/model_catalog.rs"]
mod model_catalog;
#[path = "all/not_found_resource_identifier.rs"]
mod not_found_resource_identifier;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/timestamp_wire_form.rs"]
mod timestamp_wire_form;
#[path = "all/validation_message_wording.rs"]
mod validation_message_wording;
#[path = "all/workflow_definition_listing.rs"]
mod workflow_definition_listing;
#[path = "all/workflow_lifecycle.rs"]
mod workflow_lifecycle;
