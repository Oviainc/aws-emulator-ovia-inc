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
#[path = "all/catalogue_published_urls.rs"]
mod catalogue_published_urls;
#[path = "all/create_deployment_status_sequence.rs"]
mod create_deployment_status_sequence;
#[path = "all/create_deployment_validation.rs"]
mod create_deployment_validation;
#[path = "all/delete_deployment_status_sequence.rs"]
mod delete_deployment_status_sequence;
#[path = "all/deployment_lifecycle.rs"]
mod deployment_lifecycle;
#[path = "all/deployment_pattern_versions.rs"]
mod deployment_pattern_versions;
#[path = "all/deployment_resource_group.rs"]
mod deployment_resource_group;
#[path = "all/deployment_update.rs"]
mod deployment_update;
#[path = "all/deployment_update_in_flight.rs"]
mod deployment_update_in_flight;
#[path = "all/dry_run_preflight.rs"]
mod dry_run_preflight;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/list_deployments_filter_pairs.rs"]
mod list_deployments_filter_pairs;
#[path = "all/not_found_contract.rs"]
mod not_found_contract;
#[path = "all/pattern_specification_schemas.rs"]
mod pattern_specification_schemas;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_key_rule_single_source.rs"]
mod tag_key_rule_single_source;
#[path = "all/validation_messages.rs"]
mod validation_messages;
#[path = "all/workload_catalogue.rs"]
mod workload_catalogue;
#[path = "all/workload_deployment_patterns.rs"]
mod workload_deployment_patterns;
