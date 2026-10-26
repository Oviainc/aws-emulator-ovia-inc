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
#[path = "all/app_registry_arn.rs"]
mod app_registry_arn;
#[path = "all/application_lifecycle.rs"]
mod application_lifecycle;
#[path = "all/application_reference_refusals.rs"]
mod application_reference_refusals;
#[path = "all/application_refresh.rs"]
mod application_refresh;
#[path = "all/component_cross_references.rs"]
mod component_cross_references;
#[path = "all/component_listing.rs"]
mod component_listing;
#[path = "all/component_reads.rs"]
mod component_reads;
#[path = "all/configuration_check_reads.rs"]
mod configuration_check_reads;
#[path = "all/configuration_check_rules.rs"]
mod configuration_check_rules;
#[path = "all/configuration_checks.rs"]
mod configuration_checks;
#[path = "all/delete_resource_permission.rs"]
mod delete_resource_permission;
#[path = "all/deregistration_window.rs"]
mod deregistration_window;
#[path = "all/ec2_instance_shutdown.rs"]
mod ec2_instance_shutdown;
#[path = "all/filter_validation.rs"]
mod filter_validation;
#[path = "all/get_database.rs"]
mod get_database;
#[path = "all/list_configuration_check_definitions.rs"]
mod list_configuration_check_definitions;
#[path = "all/list_configuration_check_operations.rs"]
mod list_configuration_check_operations;
#[path = "all/list_databases.rs"]
mod list_databases;
#[path = "all/list_operation_events.rs"]
mod list_operation_events;
#[path = "all/list_operations.rs"]
mod list_operations;
#[path = "all/pattern_violation_messages.rs"]
mod pattern_violation_messages;
#[path = "all/register_application_conditional_requirements.rs"]
mod register_application_conditional_requirements;
#[path = "all/registered_database_arn.rs"]
mod registered_database_arn;
#[path = "all/region_scoping.rs"]
mod region_scoping;
#[path = "all/registration_derived_state.rs"]
mod registration_derived_state;
#[path = "all/resource_permission_reads.rs"]
mod resource_permission_reads;
#[path = "all/resource_permissions.rs"]
mod resource_permissions;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/sub_check_result_cursor.rs"]
mod sub_check_result_cursor;
#[path = "all/sub_check_results.rs"]
mod sub_check_results;
#[path = "all/tag_resource.rs"]
mod tag_resource;
#[path = "all/untag_resource.rs"]
mod untag_resource;
#[path = "all/validation_message_rendering.rs"]
mod validation_message_rendering;
