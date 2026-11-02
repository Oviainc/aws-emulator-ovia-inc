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
#[path = "all/acl_membership.rs"]
mod acl_membership;
#[path = "all/acl_pending_changes.rs"]
mod acl_pending_changes;
#[path = "all/arn_addressed_refusals.rs"]
mod arn_addressed_refusals;
#[path = "all/aws_message_templates.rs"]
mod aws_message_templates;
#[path = "all/canonical_access_string.rs"]
mod canonical_access_string;
#[path = "all/cluster_lifecycle.rs"]
mod cluster_lifecycle;
#[path = "all/default_parameter_group_catalog.rs"]
mod default_parameter_group_catalog;
#[path = "all/default_parameter_groups.rs"]
mod default_parameter_groups;
#[path = "all/engine_versions.rs"]
mod engine_versions;
#[path = "all/events.rs"]
mod events;
#[path = "all/fault_http_status.rs"]
mod fault_http_status;
#[path = "all/page_size_window.rs"]
mod page_size_window;
#[path = "all/page_size_windows.rs"]
mod page_size_windows;
#[path = "all/parameter_catalog.rs"]
mod parameter_catalog;
#[path = "all/parameter_group_reset.rs"]
mod parameter_group_reset;
#[path = "all/parameter_groups.rs"]
mod parameter_groups;
#[path = "all/parameters.rs"]
mod parameters;
#[path = "all/refusal_fault_codes.rs"]
mod refusal_fault_codes;
#[path = "all/refusal_messages.rs"]
mod refusal_messages;
#[path = "all/request_level_guards.rs"]
mod request_level_guards;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/user_deletion.rs"]
mod user_deletion;
#[path = "all/user_lifecycle.rs"]
mod user_lifecycle;
#[path = "all/user_updates.rs"]
mod user_updates;
#[path = "all/users.rs"]
mod users;
#[path = "all/users_acls_name_rules.rs"]
mod users_acls_name_rules;
