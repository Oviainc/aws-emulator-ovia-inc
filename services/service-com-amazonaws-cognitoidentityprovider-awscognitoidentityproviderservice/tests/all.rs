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
#[path = "all/add_custom_attributes.rs"]
mod add_custom_attributes;
#[path = "all/admin_group_listing.rs"]
mod admin_group_listing;
#[path = "all/admin_remove_user_from_group.rs"]
mod admin_remove_user_from_group;
#[path = "all/admin_respond_to_auth_challenge.rs"]
mod admin_respond_to_auth_challenge;
#[path = "all/admin_user_enabled_state.rs"]
mod admin_user_enabled_state;
#[path = "all/admin_user_pool_lifecycle.rs"]
mod admin_user_pool_lifecycle;
#[path = "all/clock_dependent_state.rs"]
mod clock_dependent_state;
#[path = "all/delete_group.rs"]
mod delete_group;
#[path = "all/delete_user_attributes.rs"]
mod delete_user_attributes;
#[path = "all/delete_user_pool_domain.rs"]
mod delete_user_pool_domain;
#[path = "all/device_remembering.rs"]
mod device_remembering;
#[path = "all/error_message_wording.rs"]
mod error_message_wording;
#[path = "all/get_identity_provider_by_identifier.rs"]
mod get_identity_provider_by_identifier;
#[path = "all/group_lifecycle.rs"]
mod group_lifecycle;
#[path = "all/group_membership.rs"]
mod group_membership;
#[path = "all/identity_provider_lifecycle.rs"]
mod identity_provider_lifecycle;
#[path = "all/initiate_auth.rs"]
mod initiate_auth;
#[path = "all/list_devices.rs"]
mod list_devices;
#[path = "all/list_groups.rs"]
mod list_groups;
#[path = "all/list_identity_providers.rs"]
mod list_identity_providers;
#[path = "all/list_user_pools.rs"]
mod list_user_pools;
#[path = "all/list_users_in_group.rs"]
mod list_users_in_group;
#[path = "all/resend_confirmation_code.rs"]
mod resend_confirmation_code;
#[path = "all/resource_server_lifecycle.rs"]
mod resource_server_lifecycle;
#[path = "all/self_service_sign_up.rs"]
mod self_service_sign_up;
#[path = "all/set_user_settings.rs"]
mod set_user_settings;
#[path = "all/sign_up_client_secret.rs"]
mod sign_up_client_secret;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/terms_documents.rs"]
mod terms_documents;
#[path = "all/token_signing.rs"]
mod token_signing;
#[path = "all/update_group.rs"]
mod update_group;
#[path = "all/update_terms.rs"]
mod update_terms;
#[path = "all/update_user_attributes.rs"]
mod update_user_attributes;
#[path = "all/update_user_pool.rs"]
mod update_user_pool;
#[path = "all/update_user_pool_client.rs"]
mod update_user_pool_client;
#[path = "all/user_attribute_verification_code.rs"]
mod user_attribute_verification_code;
#[path = "all/user_import_job_lifecycle.rs"]
mod user_import_job_lifecycle;
#[path = "all/user_pool_domain.rs"]
mod user_pool_domain;
#[path = "all/user_pool_mfa_config.rs"]
mod user_pool_mfa_config;
#[path = "all/verification_code_pipeline.rs"]
mod verification_code_pipeline;
