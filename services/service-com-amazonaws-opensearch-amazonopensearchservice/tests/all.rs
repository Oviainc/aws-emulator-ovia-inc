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
#[path = "all/applications.rs"]
mod applications;
#[path = "all/change_progress.rs"]
mod change_progress;
#[path = "all/collection_element_refusals.rs"]
mod collection_element_refusals;
#[path = "all/compatible_versions.rs"]
mod compatible_versions;
#[path = "all/cross_cluster_connections.rs"]
mod cross_cluster_connections;
#[path = "all/data_source_create_rules.rs"]
mod data_source_create_rules;
#[path = "all/data_source_registry.rs"]
mod data_source_registry;
#[path = "all/data_source_updates.rs"]
mod data_source_updates;
#[path = "all/default_application_setting.rs"]
mod default_application_setting;
#[path = "all/domain_auto_tunes.rs"]
mod domain_auto_tunes;
#[path = "all/domain_batch_describe.rs"]
mod domain_batch_describe;
#[path = "all/domain_configuration_rules.rs"]
mod domain_configuration_rules;
#[path = "all/domain_health.rs"]
mod domain_health;
#[path = "all/domain_lifecycle.rs"]
mod domain_lifecycle;
#[path = "all/domain_nodes.rs"]
mod domain_nodes;
#[path = "all/domain_package_listing_cursor.rs"]
mod domain_package_listing_cursor;
#[path = "all/engine_version_resolution.rs"]
mod engine_version_resolution;
#[path = "all/inbound_connections.rs"]
mod inbound_connections;
#[path = "all/index_deletion.rs"]
mod index_deletion;
#[path = "all/index_registry.rs"]
mod index_registry;
#[path = "all/index_schema_updates.rs"]
mod index_schema_updates;
#[path = "all/insight_details.rs"]
mod insight_details;
#[path = "all/insights.rs"]
mod insights;
#[path = "all/instance_type_catalog.rs"]
mod instance_type_catalog;
#[path = "all/instance_type_engine_refusal.rs"]
mod instance_type_engine_refusal;
#[path = "all/instance_type_limits.rs"]
mod instance_type_limits;
#[path = "all/package_association_listings.rs"]
mod package_association_listings;
#[path = "all/package_associations.rs"]
mod package_associations;
#[path = "all/package_dependency_lifecycle.rs"]
mod package_dependency_lifecycle;
#[path = "all/package_dissociation.rs"]
mod package_dissociation;
#[path = "all/package_dissociation_batches.rs"]
mod package_dissociation_batches;
#[path = "all/package_listing_page_bounds.rs"]
mod package_listing_page_bounds;
#[path = "all/package_registry.rs"]
mod package_registry;
#[path = "all/package_scope.rs"]
mod package_scope;
#[path = "all/package_version_history.rs"]
mod package_version_history;
#[path = "all/package_versions.rs"]
mod package_versions;
#[path = "all/reserved_instance_offerings.rs"]
mod reserved_instance_offerings;
#[path = "all/reserved_instances.rs"]
mod reserved_instances;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/scheduled_action_updates.rs"]
mod scheduled_action_updates;
#[path = "all/scheduled_actions.rs"]
mod scheduled_actions;
#[path = "all/service_software_rollback.rs"]
mod service_software_rollback;
#[path = "all/service_software_update.rs"]
mod service_software_update;
#[path = "all/shared_es_signing_name_claim.rs"]
mod shared_es_signing_name_claim;
#[path = "all/single_package_association.rs"]
mod single_package_association;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/upgrade_eligibility_checks.rs"]
mod upgrade_eligibility_checks;
#[path = "all/upgrade_history.rs"]
mod upgrade_history;
#[path = "all/upgrade_status.rs"]
mod upgrade_status;
#[path = "all/upgrade_status_reporting.rs"]
mod upgrade_status_reporting;
#[path = "all/version_catalog.rs"]
mod version_catalog;
#[path = "all/vpc_access_endpoints.rs"]
mod vpc_access_endpoints;
#[path = "all/vpc_endpoint_access.rs"]
mod vpc_endpoint_access;
#[path = "all/vpc_endpoint_updates.rs"]
mod vpc_endpoint_updates;
#[path = "all/vpc_endpoints.rs"]
mod vpc_endpoints;
#[path = "all/vpc_endpoints_for_domain.rs"]
mod vpc_endpoints_for_domain;
#[path = "all/vpc_endpoints_for_domain_status.rs"]
mod vpc_endpoints_for_domain_status;
