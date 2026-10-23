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
#[path = "all/batch_get_application_revisions.rs"]
mod batch_get_application_revisions;
#[path = "all/batch_get_applications.rs"]
mod batch_get_applications;
#[path = "all/batch_get_deployments.rs"]
mod batch_get_deployments;
#[path = "all/client_error_messages.rs"]
mod client_error_messages;
#[path = "all/codedeploy_deployment_outcome.rs"]
mod codedeploy_deployment_outcome;
#[path = "all/codedeploy_lifecycle.rs"]
mod codedeploy_lifecycle;
#[path = "all/deployment_configs.rs"]
mod deployment_configs;
#[path = "all/deployment_group_defaults.rs"]
mod deployment_group_defaults;
#[path = "all/deployment_group_update.rs"]
mod deployment_group_update;
#[path = "all/git_hub_account_token_names.rs"]
mod git_hub_account_token_names;
#[path = "all/github_account_tokens.rs"]
mod github_account_tokens;
#[path = "all/lifecycle_event_hook_status.rs"]
mod lifecycle_event_hook_status;
#[path = "all/list_deployment_groups.rs"]
mod list_deployment_groups;
#[path = "all/on_premises_instances.rs"]
mod on_premises_instances;
#[path = "all/on_premises_revisions_and_blue_green.rs"]
mod on_premises_revisions_and_blue_green;
#[path = "all/on_premises_tag_removal.rs"]
mod on_premises_tag_removal;
#[path = "all/on_premises_tagging.rs"]
mod on_premises_tagging;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stop_deployment.rs"]
mod stop_deployment;
#[path = "all/update_application.rs"]
mod update_application;
