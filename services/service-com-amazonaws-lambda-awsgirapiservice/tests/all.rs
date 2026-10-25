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
#[path = "all/alias_listing.rs"]
mod alias_listing;
#[path = "all/alias_name_constraint.rs"]
mod alias_name_constraint;
#[path = "all/async_invocation.rs"]
mod async_invocation;
#[path = "all/capacity_provider_bounds.rs"]
mod capacity_provider_bounds;
#[path = "all/capacity_provider_listing.rs"]
mod capacity_provider_listing;
#[path = "all/capacity_provider_naming.rs"]
mod capacity_provider_naming;
#[path = "all/capacity_provider_update.rs"]
mod capacity_provider_update;
#[path = "all/capacity_providers.rs"]
mod capacity_providers;
#[path = "all/code_signing_config_update.rs"]
mod code_signing_config_update;
#[path = "all/code_signing_configs.rs"]
mod code_signing_configs;
#[path = "all/event_source_mapping_deletion.rs"]
mod event_source_mapping_deletion;
#[path = "all/event_source_mappings.rs"]
mod event_source_mappings;
#[path = "all/function_aliases.rs"]
mod function_aliases;
#[path = "all/function_code_update.rs"]
mod function_code_update;
#[path = "all/function_concurrency.rs"]
mod function_concurrency;
#[path = "all/function_concurrency_removal.rs"]
mod function_concurrency_removal;
#[path = "all/function_configuration.rs"]
mod function_configuration;
#[path = "all/function_lifecycle.rs"]
mod function_lifecycle;
#[path = "all/function_recursion_config.rs"]
mod function_recursion_config;
#[path = "all/function_scaling_config.rs"]
mod function_scaling_config;
#[path = "all/function_url_config.rs"]
mod function_url_config;
#[path = "all/function_url_deletion.rs"]
mod function_url_deletion;
#[path = "all/function_version_listing.rs"]
mod function_version_listing;
#[path = "all/layer_listing.rs"]
mod layer_listing;
#[path = "all/layer_version_by_arn.rs"]
mod layer_version_by_arn;
#[path = "all/layer_version_listing.rs"]
mod layer_version_listing;
#[path = "all/layer_version_permissions.rs"]
mod layer_version_permissions;
#[path = "all/layer_versions.rs"]
mod layer_versions;
#[path = "all/missing_policy_refusal.rs"]
mod missing_policy_refusal;
#[path = "all/policy_versions_and_resources.rs"]
mod policy_versions_and_resources;
#[path = "all/provisioned_concurrency.rs"]
mod provisioned_concurrency;
#[path = "all/provisioned_concurrency_resolution.rs"]
mod provisioned_concurrency_resolution;
#[path = "all/read_representation_and_download_plane.rs"]
mod read_representation_and_download_plane;
#[path = "all/resource_policy.rs"]
mod resource_policy;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/runtime_management_config.rs"]
mod runtime_management_config;
#[path = "all/signing_name_routing.rs"]
mod signing_name_routing;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_resource_arn_constraint.rs"]
mod tag_resource_arn_constraint;
#[path = "all/untrusted_name_parsing.rs"]
mod untrusted_name_parsing;
#[path = "all/update_function_configuration.rs"]
mod update_function_configuration;
#[path = "all/version_publishing.rs"]
mod version_publishing;
