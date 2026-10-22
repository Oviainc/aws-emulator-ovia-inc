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
#[path = "all/delete_slack_workspace_configuration.rs"]
mod delete_slack_workspace_configuration;
#[path = "all/error_message_templates.rs"]
mod error_message_templates;
#[path = "all/resource_preconditions.rs"]
mod resource_preconditions;
#[path = "all/slack_channel_configurations.rs"]
mod slack_channel_configurations;
#[path = "all/slack_workspace_configurations.rs"]
mod slack_workspace_configurations;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
