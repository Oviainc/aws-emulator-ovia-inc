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
//
// Declared here, ahead of the sorted list, ON PURPOSE: every apigateway fix that lands
// inserts its suite into that list or appends after it, so both of those regions collide
// with whatever else lands in the same window — this fix lost several serialized squash
// merges to exactly that, as the `update_deployment` note at the end of this file records
// for another. Fold it into the sorted list whenever the list is next touched for another
// reason; the suite itself is an ordinary `tests/all/<name>.rs` file.
#[path = "all/documentation_part_creation.rs"]
mod documentation_part_creation;

#[path = "all/account_settings.rs"]
mod account_settings;
#[path = "all/api_key_collection_members.rs"]
mod api_key_collection_members;
#[path = "all/api_key_import.rs"]
mod api_key_import;
#[path = "all/api_key_lifecycle.rs"]
mod api_key_lifecycle;
#[path = "all/authorizer_lifecycle.rs"]
mod authorizer_lifecycle;
#[path = "all/delete_authorizer.rs"]
mod delete_authorizer;
#[path = "all/delete_integration.rs"]
mod delete_integration;
#[path = "all/delete_lifecycle.rs"]
mod delete_lifecycle;
#[path = "all/delete_method_response.rs"]
mod delete_method_response;
#[path = "all/delete_stage.rs"]
mod delete_stage;
#[path = "all/documentation_part_import_bulk.rs"]
mod documentation_part_import_bulk;
#[path = "all/documentation_parts.rs"]
mod documentation_parts;
#[path = "all/documentation_version_collection.rs"]
mod documentation_version_collection;
#[path = "all/documentation_versions.rs"]
mod documentation_versions;
#[path = "all/domain_name_access_association_reject.rs"]
mod domain_name_access_association_reject;
#[path = "all/domain_name_collection.rs"]
mod domain_name_collection;
#[path = "all/domain_name_create_bounds.rs"]
mod domain_name_create_bounds;
#[path = "all/domain_name_lifecycle.rs"]
mod domain_name_lifecycle;
#[path = "all/epoch_second_timestamps.rs"]
mod epoch_second_timestamps;
#[path = "all/execute_api_data_plane.rs"]
mod execute_api_data_plane;
#[path = "all/flush_stage_authorizers_cache.rs"]
mod flush_stage_authorizers_cache;
#[path = "all/flush_stage_cache.rs"]
mod flush_stage_cache;
#[path = "all/gateway_response.rs"]
mod gateway_response;
#[path = "all/gateway_response_collection.rs"]
mod gateway_response_collection;
#[path = "all/gateway_response_scope.rs"]
mod gateway_response_scope;
#[path = "all/integration_response.rs"]
mod integration_response;
#[path = "all/method_integration.rs"]
mod method_integration;
#[path = "all/method_response.rs"]
mod method_response;
#[path = "all/patch_updates.rs"]
mod patch_updates;
#[path = "all/query_value_coercion.rs"]
mod query_value_coercion;
#[path = "all/request_validator_lifecycle.rs"]
mod request_validator_lifecycle;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/rest_api_defaults.rs"]
mod rest_api_defaults;
#[path = "all/rest_api_identifier_refusals.rs"]
mod rest_api_identifier_refusals;
#[path = "all/rest_api_import.rs"]
mod rest_api_import;
#[path = "all/rest_api_lifecycle.rs"]
mod rest_api_lifecycle;
#[path = "all/sdk_types.rs"]
mod sdk_types;
#[path = "all/service_claim.rs"]
mod service_claim;
#[path = "all/stage_collection.rs"]
mod stage_collection;
#[path = "all/stage_export.rs"]
mod stage_export;
#[path = "all/stage_invoke.rs"]
mod stage_invoke;
#[path = "all/stage_lifecycle.rs"]
mod stage_lifecycle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_reads_by_arn.rs"]
mod tag_reads_by_arn;
#[path = "all/update_integration.rs"]
mod update_integration;
#[path = "all/update_method_response.rs"]
mod update_method_response;
#[path = "all/update_stage.rs"]
mod update_stage;
#[path = "all/usage_plan_lifecycle.rs"]
mod usage_plan_lifecycle;
#[path = "all/validation_message_rules.rs"]
mod validation_message_rules;
#[path = "all/vpc_links.rs"]
mod vpc_links;

// Declared after the sorted list on purpose: every apigateway fix that lands adds its
// suite inside that list, so a line inserted there collides with every other suite
// landing in the same window — this fix lost three serialized squash merges to exactly
// that. Appending keeps this change out of the contended region; the suite itself is an
// ordinary `tests/all/<name>.rs` file. Fold it into the sorted list whenever the list is
// next touched for another reason.
#[path = "all/update_deployment.rs"]
mod update_deployment;
#[path = "all/delete_documentation_part.rs"]
mod delete_documentation_part;
#[path = "all/update_documentation_part.rs"]
mod update_documentation_part;
#[path = "all/update_stage_setting_spellings.rs"]
mod update_stage_setting_spellings;
#[path = "all/usage_plan_key_lifecycle.rs"]
mod usage_plan_key_lifecycle;
#[path = "all/deployment_collection.rs"]
mod deployment_collection;
#[path = "all/authorizer_collection.rs"]
mod authorizer_collection;
#[path = "all/update_stage_readers.rs"]
mod update_stage_readers;
#[path = "all/execution_logging_prerequisites.rs"]
mod execution_logging_prerequisites;
#[path = "all/usage_plan_creation_bounds.rs"]
mod usage_plan_creation_bounds;
#[path = "all/control_plane_deletes.rs"]
mod control_plane_deletes;
#[path = "all/delete_deployment.rs"]
mod delete_deployment;
#[path = "all/rest_api_create_from_import.rs"]
mod rest_api_create_from_import;
#[path = "all/usage_plan_collection.rs"]
mod usage_plan_collection;
#[path = "all/domain_name_access_associations.rs"]
mod domain_name_access_associations;
#[path = "all/stage_invoke_endpoint.rs"]
mod stage_invoke_endpoint;
#[path = "all/delete_integration_response.rs"]
mod delete_integration_response;
#[path = "all/update_authorizer.rs"]
mod update_authorizer;
#[path = "all/update_gateway_response.rs"]
mod update_gateway_response;
#[path = "all/stage_invoke_shared_stage_names.rs"]
mod stage_invoke_shared_stage_names;
#[path = "all/request_validator_collection.rs"]
mod request_validator_collection;
#[path = "all/api_key_gate.rs"]
mod api_key_gate;
#[path = "all/update_request_validator.rs"]
mod update_request_validator;
#[path = "all/update_stage_variables.rs"]
mod update_stage_variables;
#[path = "all/update_vpc_link.rs"]
mod update_vpc_link;
#[path = "all/update_documentation_version.rs"]
mod update_documentation_version;
#[path = "all/refusal_message_constants.rs"]
mod refusal_message_constants;
#[path = "all/delete_documentation_version.rs"]
mod delete_documentation_version;
#[path = "all/delete_usage_plan.rs"]
mod delete_usage_plan;
#[path = "all/update_integration_response.rs"]
mod update_integration_response;
