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
#[path = "all/change_progress.rs"]
mod change_progress;
#[path = "all/compatible_versions.rs"]
mod compatible_versions;
#[path = "all/cross_cluster_connections.rs"]
mod cross_cluster_connections;
#[path = "all/data_source_create_rules.rs"]
mod data_source_create_rules;
#[path = "all/data_source_registry.rs"]
mod data_source_registry;
#[path = "all/default_application_setting.rs"]
mod default_application_setting;
#[path = "all/domain_batch_describe.rs"]
mod domain_batch_describe;
#[path = "all/domain_health.rs"]
mod domain_health;
#[path = "all/domain_lifecycle.rs"]
mod domain_lifecycle;
#[path = "all/domain_nodes.rs"]
mod domain_nodes;
#[path = "all/index_registry.rs"]
mod index_registry;
#[path = "all/index_schema_updates.rs"]
mod index_schema_updates;
#[path = "all/insights.rs"]
mod insights;
#[path = "all/instance_type_catalog.rs"]
mod instance_type_catalog;
#[path = "all/instance_type_limits.rs"]
mod instance_type_limits;
#[path = "all/package_associations.rs"]
mod package_associations;
#[path = "all/package_registry.rs"]
mod package_registry;
#[path = "all/reserved_instance_offerings.rs"]
mod reserved_instance_offerings;
#[path = "all/reserved_instances.rs"]
mod reserved_instances;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/scheduled_actions.rs"]
mod scheduled_actions;
#[path = "all/service_software_rollback.rs"]
mod service_software_rollback;
#[path = "all/shared_es_signing_name_claim.rs"]
mod shared_es_signing_name_claim;
#[path = "all/single_package_association.rs"]
mod single_package_association;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/upgrade_history.rs"]
mod upgrade_history;
#[path = "all/vpc_endpoint_access.rs"]
mod vpc_endpoint_access;
