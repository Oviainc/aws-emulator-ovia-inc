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
#[path = "all/account_settings.rs"]
mod account_settings;
#[path = "all/api_key_collection_members.rs"]
mod api_key_collection_members;
#[path = "all/api_key_lifecycle.rs"]
mod api_key_lifecycle;
#[path = "all/authorizer_lifecycle.rs"]
mod authorizer_lifecycle;
#[path = "all/delete_integration.rs"]
mod delete_integration;
#[path = "all/delete_lifecycle.rs"]
mod delete_lifecycle;
#[path = "all/delete_method_response.rs"]
mod delete_method_response;
#[path = "all/documentation_parts.rs"]
mod documentation_parts;
#[path = "all/documentation_versions.rs"]
mod documentation_versions;
#[path = "all/domain_name_collection.rs"]
mod domain_name_collection;
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

// Declared after the sorted list on purpose: every apigateway fix that lands adds its
// suite inside that list, so a line inserted there collides with every other suite
// landing in the same window — this fix lost three serialized squash merges to exactly
// that. Appending keeps this change out of the contended region; the suite itself is an
// ordinary `tests/all/<name>.rs` file. Fold it into the sorted list whenever the list is
// next touched for another reason.
#[path = "all/update_deployment.rs"]
mod update_deployment;
