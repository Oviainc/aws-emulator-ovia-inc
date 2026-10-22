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
#[path = "all/as2_profile_listing.rs"]
mod as2_profile_listing;
#[path = "all/connector_configuration.rs"]
mod connector_configuration;
#[path = "all/connector_listing.rs"]
mod connector_listing;
#[path = "all/error_message_rules.rs"]
mod error_message_rules;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/host_key_lifecycle.rs"]
mod host_key_lifecycle;
#[path = "all/host_key_listing.rs"]
mod host_key_listing;
#[path = "all/host_keys.rs"]
mod host_keys;
#[path = "all/input_validation.rs"]
mod input_validation;
#[path = "all/not_found_errors.rs"]
mod not_found_errors;
#[path = "all/profile_lifecycle.rs"]
mod profile_lifecycle;
#[path = "all/resource_resolution_refusals.rs"]
mod resource_resolution_refusals;
#[path = "all/security_policy_catalog.rs"]
mod security_policy_catalog;
#[path = "all/server_config_and_state.rs"]
mod server_config_and_state;
#[path = "all/server_logging_configuration.rs"]
mod server_logging_configuration;
#[path = "all/server_user_lifecycle.rs"]
mod server_user_lifecycle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/user_properties_and_keys.rs"]
mod user_properties_and_keys;
#[path = "all/user_state_persistence.rs"]
mod user_state_persistence;
#[path = "all/web_app_lifecycle.rs"]
mod web_app_lifecycle;
#[path = "all/workflow_lifecycle.rs"]
mod workflow_lifecycle;
