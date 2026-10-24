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
#[path = "all/action_catalog.rs"]
mod action_catalog;
#[path = "all/control_plane_rules.rs"]
mod control_plane_rules;
#[path = "all/delete_target_account_configuration.rs"]
mod delete_target_account_configuration;
#[path = "all/experiment_lifecycle.rs"]
mod experiment_lifecycle;
#[path = "all/experiment_options_decide_the_run.rs"]
mod experiment_options_decide_the_run;
#[path = "all/experiment_run.rs"]
mod experiment_run;
#[path = "all/experiment_target_account_configurations.rs"]
mod experiment_target_account_configurations;
#[path = "all/experiment_template_not_found.rs"]
mod experiment_template_not_found;
#[path = "all/list_experiment_resolved_targets.rs"]
mod list_experiment_resolved_targets;
#[path = "all/list_experiment_target_account_configurations.rs"]
mod list_experiment_target_account_configurations;
#[path = "all/list_target_account_configurations.rs"]
mod list_target_account_configurations;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/target_account_configuration_errors.rs"]
mod target_account_configuration_errors;
#[path = "all/target_account_configurations.rs"]
mod target_account_configurations;
#[path = "all/target_account_role_ownership.rs"]
mod target_account_role_ownership;
