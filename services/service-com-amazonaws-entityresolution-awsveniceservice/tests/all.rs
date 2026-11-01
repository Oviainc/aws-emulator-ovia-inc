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
#[path = "all/enum_value_set_order.rs"]
mod enum_value_set_order;
#[path = "all/id_mapping_workflow_delete.rs"]
mod id_mapping_workflow_delete;
#[path = "all/id_mapping_workflow_lifecycle.rs"]
mod id_mapping_workflow_lifecycle;
#[path = "all/id_mapping_workflow_listing.rs"]
mod id_mapping_workflow_listing;
#[path = "all/id_mapping_workflow_update.rs"]
mod id_mapping_workflow_update;
#[path = "all/id_namespace_delete.rs"]
mod id_namespace_delete;
#[path = "all/id_namespace_lifecycle.rs"]
mod id_namespace_lifecycle;
#[path = "all/id_namespace_update.rs"]
mod id_namespace_update;
#[path = "all/match_id_generation.rs"]
mod match_id_generation;
#[path = "all/matching_job_lifecycle.rs"]
mod matching_job_lifecycle;
#[path = "all/matching_workflow_update.rs"]
mod matching_workflow_update;
#[path = "all/policy_statements.rs"]
mod policy_statements;
#[path = "all/policy_token_conflict.rs"]
mod policy_token_conflict;
#[path = "all/provider_service_catalog.rs"]
mod provider_service_catalog;
#[path = "all/resource_policy.rs"]
mod resource_policy;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/response_fidelity.rs"]
mod response_fidelity;
#[path = "all/response_messages.rs"]
mod response_messages;
#[path = "all/role_resolution.rs"]
mod role_resolution;
#[path = "all/schema_mapping_update.rs"]
mod schema_mapping_update;
#[path = "all/schema_mapping_workflow_lifecycle.rs"]
mod schema_mapping_workflow_lifecycle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/unique_id_deletion.rs"]
mod unique_id_deletion;
