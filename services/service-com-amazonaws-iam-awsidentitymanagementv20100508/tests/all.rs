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
#[path = "all/access_keys.rs"]
mod access_keys;
#[path = "all/instance_profile_role_binding.rs"]
mod instance_profile_role_binding;
#[path = "all/instance_profiles.rs"]
mod instance_profiles;
#[path = "all/managed_policies.rs"]
mod managed_policies;
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
#[path = "all/role_policy_removal.rs"]
mod role_policy_removal;
#[path = "all/role_tags.rs"]
mod role_tags;
#[path = "all/service_linked_roles.rs"]
mod service_linked_roles;
#[path = "all/service_specific_credentials.rs"]
mod service_specific_credentials;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/user_lifecycle.rs"]
mod user_lifecycle;
