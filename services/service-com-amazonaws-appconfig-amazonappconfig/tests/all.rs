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
#[path = "all/application_name_uniqueness.rs"]
mod application_name_uniqueness;
#[path = "all/aws_authored_extensions.rs"]
mod aws_authored_extensions;
#[path = "all/configuration_profile_deletion_protection.rs"]
mod configuration_profile_deletion_protection;
#[path = "all/configuration_profile_type.rs"]
mod configuration_profile_type;
#[path = "all/control_plane_fidelity.rs"]
mod control_plane_fidelity;
#[path = "all/create_deployment_strategy.rs"]
mod create_deployment_strategy;
#[path = "all/delete_application.rs"]
mod delete_application;
#[path = "all/delete_configuration_profile.rs"]
mod delete_configuration_profile;
#[path = "all/delete_experiment_definition.rs"]
mod delete_experiment_definition;
#[path = "all/delete_hosted_configuration_version.rs"]
mod delete_hosted_configuration_version;
#[path = "all/deployment_wire_fidelity.rs"]
mod deployment_wire_fidelity;
#[path = "all/environment_state_wire_form.rs"]
mod environment_state_wire_form;
#[path = "all/environment_update.rs"]
mod environment_update;
#[path = "all/experiment_definitions.rs"]
mod experiment_definitions;
#[path = "all/extension_associations.rs"]
mod extension_associations;
#[path = "all/extensions.rs"]
mod extensions;
#[path = "all/feature_flag_metadata.rs"]
mod feature_flag_metadata;
#[path = "all/list_applications.rs"]
mod list_applications;
#[path = "all/list_configuration_profiles.rs"]
mod list_configuration_profiles;
#[path = "all/list_deployment_strategies.rs"]
mod list_deployment_strategies;
#[path = "all/list_environments.rs"]
mod list_environments;
#[path = "all/list_extension_associations.rs"]
mod list_extension_associations;
#[path = "all/list_extensions.rs"]
mod list_extensions;
#[path = "all/list_hosted_configuration_versions.rs"]
mod list_hosted_configuration_versions;
#[path = "all/pattern_refusal_quoting.rs"]
mod pattern_refusal_quoting;
#[path = "all/refusal_message_templates.rs"]
mod refusal_message_templates;
#[path = "all/resource_deletes.rs"]
mod resource_deletes;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/start_experiment_run.rs"]
mod start_experiment_run;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stop_deployment.rs"]
mod stop_deployment;
#[path = "all/update_application.rs"]
mod update_application;
#[path = "all/update_configuration_profile.rs"]
mod update_configuration_profile;
#[path = "all/update_deployment_strategy.rs"]
mod update_deployment_strategy;
#[path = "all/update_experiment_definition.rs"]
mod update_experiment_definition;
#[path = "all/update_experiment_run.rs"]
mod update_experiment_run;
#[path = "all/update_extension.rs"]
mod update_extension;
#[path = "all/update_extension_association.rs"]
mod update_extension_association;
