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
#[path = "all/asset_types.rs"]
mod asset_types;
#[path = "all/batch_delete_connection.rs"]
mod batch_delete_connection;
#[path = "all/batch_get_jobs.rs"]
mod batch_get_jobs;
#[path = "all/batch_get_partition.rs"]
mod batch_get_partition;
#[path = "all/batch_stop_job_run.rs"]
mod batch_stop_job_run;
#[path = "all/catalog_encryption.rs"]
mod catalog_encryption;
#[path = "all/catalog_identity.rs"]
mod catalog_identity;
#[path = "all/catalog_mutations.rs"]
mod catalog_mutations;
#[path = "all/catalog_projections.rs"]
mod catalog_projections;
#[path = "all/classifiers.rs"]
mod classifiers;
#[path = "all/column_statistics_task_conflicts.rs"]
mod column_statistics_task_conflicts;
#[path = "all/column_statistics_task_run_history.rs"]
mod column_statistics_task_run_history;
#[path = "all/column_statistics_task_runs.rs"]
mod column_statistics_task_runs;
#[path = "all/column_statistics_task_settings.rs"]
mod column_statistics_task_settings;
#[path = "all/connection_deletion.rs"]
mod connection_deletion;
#[path = "all/connection_listing.rs"]
mod connection_listing;
#[path = "all/connection_not_found.rs"]
mod connection_not_found;
#[path = "all/connection_updates.rs"]
mod connection_updates;
#[path = "all/crawler_event_queue.rs"]
mod crawler_event_queue;
#[path = "all/crawler_schedule_suspension.rs"]
mod crawler_schedule_suspension;
#[path = "all/crawler_schedules.rs"]
mod crawler_schedules;
#[path = "all/crawler_updates.rs"]
mod crawler_updates;
#[path = "all/crawlers.rs"]
mod crawlers;
#[path = "all/create_partition.rs"]
mod create_partition;
#[path = "all/custom_entity_type_listing.rs"]
mod custom_entity_type_listing;
#[path = "all/custom_entity_types.rs"]
mod custom_entity_types;
#[path = "all/data_catalog.rs"]
mod data_catalog;
#[path = "all/data_catalog_encryption_settings.rs"]
mod data_catalog_encryption_settings;
#[path = "all/data_quality_rule_recommendation_runs.rs"]
mod data_quality_rule_recommendation_runs;
#[path = "all/database_listing.rs"]
mod database_listing;
#[path = "all/error_messages.rs"]
mod error_messages;
#[path = "all/etl_connections.rs"]
mod etl_connections;
#[path = "all/etl_jobs_and_connections.rs"]
mod etl_jobs_and_connections;
#[path = "all/form_type_deletion.rs"]
mod form_type_deletion;
#[path = "all/form_types.rs"]
mod form_types;
#[path = "all/get_mapping.rs"]
mod get_mapping;
#[path = "all/get_plan.rs"]
mod get_plan;
#[path = "all/get_table_entity_not_found.rs"]
mod get_table_entity_not_found;
#[path = "all/glossaries.rs"]
mod glossaries;
#[path = "all/glossary_term_listing.rs"]
mod glossary_term_listing;
#[path = "all/glossary_terms.rs"]
mod glossary_terms;
#[path = "all/integration_resource_properties.rs"]
mod integration_resource_properties;
#[path = "all/job_deletion.rs"]
mod job_deletion;
#[path = "all/job_listing.rs"]
mod job_listing;
#[path = "all/job_regions.rs"]
mod job_regions;
#[path = "all/job_run_listing.rs"]
mod job_run_listing;
#[path = "all/job_runs.rs"]
mod job_runs;
#[path = "all/job_updates.rs"]
mod job_updates;
#[path = "all/jobs.rs"]
mod jobs;
#[path = "all/ml_transform_reads.rs"]
mod ml_transform_reads;
#[path = "all/ml_transform_updates.rs"]
mod ml_transform_updates;
#[path = "all/ml_transforms.rs"]
mod ml_transforms;
#[path = "all/partitions.rs"]
mod partitions;
#[path = "all/registries.rs"]
mod registries;
#[path = "all/resource_policies.rs"]
mod resource_policies;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/schema_registry.rs"]
mod schema_registry;
#[path = "all/search_tables.rs"]
mod search_tables;
#[path = "all/security_configurations.rs"]
mod security_configurations;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/store_capacity.rs"]
mod store_capacity;
#[path = "all/table_versions.rs"]
mod table_versions;
#[path = "all/trigger_starts.rs"]
mod trigger_starts;
#[path = "all/triggers.rs"]
mod triggers;
#[path = "all/unfiltered_metadata.rs"]
mod unfiltered_metadata;
#[path = "all/usage_profiles.rs"]
mod usage_profiles;
#[path = "all/wire.rs"]
mod wire;
#[path = "all/workflow_graph.rs"]
mod workflow_graph;
#[path = "all/workflow_run_properties.rs"]
mod workflow_run_properties;
#[path = "all/workflow_runs.rs"]
mod workflow_runs;
#[path = "all/workflows.rs"]
mod workflows;
