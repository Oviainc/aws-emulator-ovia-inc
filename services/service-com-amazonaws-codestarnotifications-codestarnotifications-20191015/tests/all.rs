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
#[path = "all/create_rule_input_validation.rs"]
mod create_rule_input_validation;
#[path = "all/delete_target.rs"]
mod delete_target;
#[path = "all/describe_notification_rule_fidelity.rs"]
mod describe_notification_rule_fidelity;
#[path = "all/error_message_rules.rs"]
mod error_message_rules;
#[path = "all/event_type_catalog_and_next_token.rs"]
mod event_type_catalog_and_next_token;
#[path = "all/list_event_types.rs"]
mod list_event_types;
#[path = "all/list_targets.rs"]
mod list_targets;
#[path = "all/notification_rule_lifecycle.rs"]
mod notification_rule_lifecycle;
#[path = "all/rule_arn_authorization.rs"]
mod rule_arn_authorization;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_resource.rs"]
mod tag_resource;
