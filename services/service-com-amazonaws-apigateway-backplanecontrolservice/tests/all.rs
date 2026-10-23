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
#[path = "all/api_key_collection_members.rs"]
mod api_key_collection_members;
#[path = "all/api_key_lifecycle.rs"]
mod api_key_lifecycle;
#[path = "all/delete_lifecycle.rs"]
mod delete_lifecycle;
#[path = "all/delete_method_response.rs"]
mod delete_method_response;
#[path = "all/epoch_second_timestamps.rs"]
mod epoch_second_timestamps;
#[path = "all/method_integration.rs"]
mod method_integration;
#[path = "all/method_response.rs"]
mod method_response;
#[path = "all/patch_updates.rs"]
mod patch_updates;
#[path = "all/query_value_coercion.rs"]
mod query_value_coercion;
#[path = "all/rest_api_defaults.rs"]
mod rest_api_defaults;
#[path = "all/rest_api_identifier_refusals.rs"]
mod rest_api_identifier_refusals;
#[path = "all/rest_api_import.rs"]
mod rest_api_import;
#[path = "all/rest_api_lifecycle.rs"]
mod rest_api_lifecycle;
#[path = "all/service_claim.rs"]
mod service_claim;
#[path = "all/stage_lifecycle.rs"]
mod stage_lifecycle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/validation_message_rules.rs"]
mod validation_message_rules;
