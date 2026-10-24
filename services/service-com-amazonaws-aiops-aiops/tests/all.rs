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
#[path = "all/error_message_rules.rs"]
mod error_message_rules;
#[path = "all/investigation_group_policy_delete.rs"]
mod investigation_group_policy_delete;
#[path = "all/investigation_group_singleton.rs"]
mod investigation_group_singleton;
#[path = "all/investigation_groups.rs"]
mod investigation_groups;
#[path = "all/regional_identifier_resolution.rs"]
mod regional_identifier_resolution;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tagging_frontend_errors.rs"]
mod tagging_frontend_errors;
