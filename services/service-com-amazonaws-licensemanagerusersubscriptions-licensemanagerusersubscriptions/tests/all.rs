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
#[path = "all/associations_follow_their_registration.rs"]
mod associations_follow_their_registration;
#[path = "all/filter_operations.rs"]
mod filter_operations;
#[path = "all/identity_provider_deregistration.rs"]
mod identity_provider_deregistration;
#[path = "all/identity_provider_subscriptions.rs"]
mod identity_provider_subscriptions;
#[path = "all/instance_products_follow_the_registrations.rs"]
mod instance_products_follow_the_registrations;
#[path = "all/instance_status_check_date.rs"]
mod instance_status_check_date;
#[path = "all/license_based_subscription_expiry.rs"]
mod license_based_subscription_expiry;
#[path = "all/license_server_endpoint_delete_resolution.rs"]
mod license_server_endpoint_delete_resolution;
#[path = "all/license_server_endpoint_deletion.rs"]
mod license_server_endpoint_deletion;
#[path = "all/license_server_endpoints.rs"]
mod license_server_endpoints;
#[path = "all/list_instances.rs"]
mod list_instances;
#[path = "all/optional_settings_registration.rs"]
mod optional_settings_registration;
#[path = "all/refusals.rs"]
mod refusals;
#[path = "all/resource_resolution_refusals.rs"]
mod resource_resolution_refusals;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_validation.rs"]
mod tag_validation;
#[path = "all/user_associations.rs"]
mod user_associations;
#[path = "all/user_disassociation.rs"]
mod user_disassociation;
