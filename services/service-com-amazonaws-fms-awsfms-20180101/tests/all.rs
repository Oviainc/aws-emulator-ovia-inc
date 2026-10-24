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
#[path = "all/admin_account_disassociation.rs"]
mod admin_account_disassociation;
#[path = "all/admin_account_scopes.rs"]
mod admin_account_scopes;
#[path = "all/administrator_gated_operations.rs"]
mod administrator_gated_operations;
#[path = "all/apps_list_reporting.rs"]
mod apps_list_reporting;
#[path = "all/error_message_members.rs"]
mod error_message_members;
#[path = "all/member_accounts.rs"]
mod member_accounts;
#[path = "all/policy_document_validation.rs"]
mod policy_document_validation;
#[path = "all/policy_lifecycle.rs"]
mod policy_lifecycle;
#[path = "all/protection_status.rs"]
mod protection_status;
#[path = "all/protocols_list_lifecycle.rs"]
mod protocols_list_lifecycle;
#[path = "all/protocols_lists_enumeration.rs"]
mod protocols_lists_enumeration;
#[path = "all/resource_set_deletion.rs"]
mod resource_set_deletion;
#[path = "all/resource_set_listing.rs"]
mod resource_set_listing;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tagging_across_resource_kinds.rs"]
mod tagging_across_resource_kinds;
#[path = "all/tagging_and_managing_admins.rs"]
mod tagging_and_managing_admins;
