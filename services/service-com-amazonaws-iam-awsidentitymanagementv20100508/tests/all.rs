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
#[path = "all/access_key_deletion.rs"]
mod access_key_deletion;
#[path = "all/access_key_listing.rs"]
mod access_key_listing;
#[path = "all/access_keys.rs"]
mod access_keys;
#[path = "all/account_aliases.rs"]
mod account_aliases;
#[path = "all/aws_managed_policy_catalog.rs"]
mod aws_managed_policy_catalog;
#[path = "all/entities_for_policy.rs"]
mod entities_for_policy;
#[path = "all/instance_profile_role_binding.rs"]
mod instance_profile_role_binding;
#[path = "all/instance_profile_role_removal.rs"]
mod instance_profile_role_removal;
#[path = "all/instance_profile_roles.rs"]
mod instance_profile_roles;
#[path = "all/instance_profile_tags.rs"]
mod instance_profile_tags;
#[path = "all/instance_profiles.rs"]
mod instance_profiles;
#[path = "all/login_profiles.rs"]
mod login_profiles;
#[path = "all/managed_policies.rs"]
mod managed_policies;
#[path = "all/open_id_connect_providers.rs"]
mod open_id_connect_providers;
#[path = "all/principal_policy_context_keys.rs"]
mod principal_policy_context_keys;
#[path = "all/role_attached_policies.rs"]
mod role_attached_policies;
#[path = "all/role_deletion.rs"]
mod role_deletion;
#[path = "all/role_inline_policies.rs"]
mod role_inline_policies;
#[path = "all/role_inline_policy_rotation.rs"]
mod role_inline_policy_rotation;
#[path = "all/role_lifecycle.rs"]
mod role_lifecycle;
#[path = "all/role_listing.rs"]
mod role_listing;
#[path = "all/role_permissions_boundary.rs"]
mod role_permissions_boundary;
#[path = "all/role_policy_listing.rs"]
mod role_policy_listing;
#[path = "all/role_policy_removal.rs"]
mod role_policy_removal;
#[path = "all/role_tags.rs"]
mod role_tags;
#[path = "all/role_trust_policy.rs"]
mod role_trust_policy;
#[path = "all/saml_providers.rs"]
mod saml_providers;
#[path = "all/service_linked_roles.rs"]
mod service_linked_roles;
#[path = "all/service_provisioned_roles.rs"]
mod service_provisioned_roles;
#[path = "all/service_specific_credentials.rs"]
mod service_specific_credentials;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/user_attached_policies.rs"]
mod user_attached_policies;
#[path = "all/user_lifecycle.rs"]
mod user_lifecycle;
#[path = "all/user_permissions_boundary.rs"]
mod user_permissions_boundary;
