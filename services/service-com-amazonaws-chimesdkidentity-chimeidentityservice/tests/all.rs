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
#[path = "all/app_instance_admin.rs"]
mod app_instance_admin;
#[path = "all/app_instance_admin_listing.rs"]
mod app_instance_admin_listing;
#[path = "all/app_instance_bot_description.rs"]
mod app_instance_bot_description;
#[path = "all/app_instance_bots.rs"]
mod app_instance_bots;
#[path = "all/app_instance_lifecycle.rs"]
mod app_instance_lifecycle;
#[path = "all/app_instance_retention_settings.rs"]
mod app_instance_retention_settings;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/resource_update.rs"]
mod resource_update;
#[path = "all/retention_settings_effect_instant.rs"]
mod retention_settings_effect_instant;
#[path = "all/signing_name_route_claim.rs"]
mod signing_name_route_claim;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/token_reuse_and_deleted_refusals.rs"]
mod token_reuse_and_deleted_refusals;
#[path = "all/user_endpoint_registration.rs"]
mod user_endpoint_registration;
#[path = "all/user_expiration_settings.rs"]
mod user_expiration_settings;
