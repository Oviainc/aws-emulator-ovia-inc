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
#[path = "all/caller_named_identity_stores.rs"]
mod caller_named_identity_stores;
#[path = "all/delete_group.rs"]
mod delete_group;
#[path = "all/delete_user.rs"]
mod delete_user;
#[path = "all/error_message_rules.rs"]
mod error_message_rules;
#[path = "all/exception_envelope.rs"]
mod exception_envelope;
#[path = "all/get_group_id.rs"]
mod get_group_id;
#[path = "all/get_user_id.rs"]
mod get_user_id;
#[path = "all/group_membership_lookup.rs"]
mod group_membership_lookup;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/identity_store_lifecycle.rs"]
mod identity_store_lifecycle;
#[path = "all/identity_store_resolution_and_updates.rs"]
mod identity_store_resolution_and_updates;
#[path = "all/is_member_in_groups.rs"]
mod is_member_in_groups;
#[path = "all/list_group_memberships_for_member.rs"]
mod list_group_memberships_for_member;
#[path = "all/list_groups.rs"]
mod list_groups;
#[path = "all/list_users.rs"]
mod list_users;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/update_group.rs"]
mod update_group;
#[path = "all/update_user_attributes.rs"]
mod update_user_attributes;
