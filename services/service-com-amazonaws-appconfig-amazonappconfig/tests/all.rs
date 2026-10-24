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
#[path = "all/account_settings_document.rs"]
mod account_settings_document;
#[path = "all/appconfig_workflow.rs"]
mod appconfig_workflow;
#[path = "all/configuration_profile_type.rs"]
mod configuration_profile_type;
#[path = "all/control_plane_fidelity.rs"]
mod control_plane_fidelity;
#[path = "all/create_deployment_strategy.rs"]
mod create_deployment_strategy;
#[path = "all/delete_hosted_configuration_version.rs"]
mod delete_hosted_configuration_version;
#[path = "all/deployment_wire_fidelity.rs"]
mod deployment_wire_fidelity;
#[path = "all/environment_update.rs"]
mod environment_update;
#[path = "all/experiment_definitions.rs"]
mod experiment_definitions;
#[path = "all/extensions.rs"]
mod extensions;
#[path = "all/feature_flag_metadata.rs"]
mod feature_flag_metadata;
#[path = "all/list_configuration_profiles.rs"]
mod list_configuration_profiles;
#[path = "all/list_deployment_strategies.rs"]
mod list_deployment_strategies;
#[path = "all/list_environments.rs"]
mod list_environments;
#[path = "all/list_extensions.rs"]
mod list_extensions;
#[path = "all/refusal_message_templates.rs"]
mod refusal_message_templates;
#[path = "all/resource_deletes.rs"]
mod resource_deletes;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/update_application.rs"]
mod update_application;
#[path = "all/update_configuration_profile.rs"]
mod update_configuration_profile;
#[path = "all/update_deployment_strategy.rs"]
mod update_deployment_strategy;
