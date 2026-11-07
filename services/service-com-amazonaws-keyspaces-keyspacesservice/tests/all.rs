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
#[path = "all/a_keyspace_takes_a_new_resource_before_it_settles.rs"]
mod a_keyspace_takes_a_new_resource_before_it_settles;
#[path = "all/auto_scaling_propagation.rs"]
mod auto_scaling_propagation;
#[path = "all/cdc_specification_tags_on_update.rs"]
mod cdc_specification_tags_on_update;
#[path = "all/cdc_stream_tag_lifetime.rs"]
mod cdc_stream_tag_lifetime;
#[path = "all/cdc_streams.rs"]
mod cdc_streams;
#[path = "all/client_error_status.rs"]
mod client_error_status;
#[path = "all/column_cap_precedes_column_scans.rs"]
mod column_cap_precedes_column_scans;
#[path = "all/column_name_identifier_identity.rs"]
mod column_name_identifier_identity;
#[path = "all/column_types_and_auto_scaling.rs"]
mod column_types_and_auto_scaling;
#[path = "all/create_table_replicas_and_warm_throughput.rs"]
mod create_table_replicas_and_warm_throughput;
#[path = "all/cross_region_auto_scaling_identity.rs"]
mod cross_region_auto_scaling_identity;
#[path = "all/delete_keyspace_in_flight_resources.rs"]
mod delete_keyspace_in_flight_resources;
#[path = "all/delete_keyspace_lifecycle.rs"]
mod delete_keyspace_lifecycle;
#[path = "all/delete_table_in_flight.rs"]
mod delete_table_in_flight;
#[path = "all/delete_type.rs"]
mod delete_type;
#[path = "all/delete_type_users_on_their_way_out.rs"]
mod delete_type_users_on_their_way_out;
#[path = "all/duplicate_column_precedes_type_resolution.rs"]
mod duplicate_column_precedes_type_resolution;
#[path = "all/encryption_key_identifier_mode.rs"]
mod encryption_key_identifier_mode;
#[path = "all/get_keyspace.rs"]
mod get_keyspace;
#[path = "all/get_table_auto_scaling_settings_unconfigured.rs"]
mod get_table_auto_scaling_settings_unconfigured;
#[path = "all/keyspace_table_lifecycle.rs"]
mod keyspace_table_lifecycle;
// Emptied, not deleted: this suite's tests moved to
// `all/a_keyspace_takes_a_new_resource_before_it_settles.rs` (the declaration above), and
// the layout check requires every file under `tests/all/` to be declared here.
#[path = "all/keyspace_takes_a_new_resource_only_when_active.rs"]
mod keyspace_takes_a_new_resource_only_when_active;
#[path = "all/list_keyspaces.rs"]
mod list_keyspaces;
#[path = "all/list_types.rs"]
mod list_types;
#[path = "all/multi_region_table_replicas.rs"]
mod multi_region_table_replicas;
#[path = "all/name_conflict_precedes_type_resolution.rs"]
mod name_conflict_precedes_type_resolution;
#[path = "all/name_validation_envelope.rs"]
mod name_validation_envelope;
#[path = "all/one_replication_region_rule.rs"]
mod one_replication_region_rule;
#[path = "all/page_token_codec.rs"]
mod page_token_codec;
#[path = "all/pagination_window.rs"]
mod pagination_window;
#[path = "all/point_in_time_recovery_window.rs"]
mod point_in_time_recovery_window;
#[path = "all/read_responses_serve_always_present_members.rs"]
mod read_responses_serve_always_present_members;
#[path = "all/refusal_sentences.rs"]
mod refusal_sentences;
#[path = "all/region_scoped_page_tokens.rs"]
mod region_scoped_page_tokens;
#[path = "all/regional_partitioning.rs"]
mod regional_partitioning;
#[path = "all/replication_region_catalog.rs"]
mod replication_region_catalog;
#[path = "all/replication_region_set.rs"]
mod replication_region_set;
#[path = "all/reported_replication_regions.rs"]
mod reported_replication_regions;
#[path = "all/reserved_tag_key_stage.rs"]
mod reserved_tag_key_stage;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/restore_table.rs"]
mod restore_table;
#[path = "all/restore_table_auto_scaling_as_of_now.rs"]
mod restore_table_auto_scaling_as_of_now;
#[path = "all/restore_table_restoring_status.rs"]
mod restore_table_restoring_status;
#[path = "all/restore_table_type_resolution.rs"]
mod restore_table_type_resolution;
#[path = "all/service_managed_catalog.rs"]
mod service_managed_catalog;
#[path = "all/single_region_replication.rs"]
mod single_region_replication;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/static_columns_require_clustering_keys.rs"]
mod static_columns_require_clustering_keys;
#[path = "all/system_keyspace_table_catalog.rs"]
mod system_keyspace_table_catalog;
#[path = "all/tag_count_limit_refusal.rs"]
mod tag_count_limit_refusal;
#[path = "all/tagging_a_deleting_resource.rs"]
mod tagging_a_deleting_resource;
#[path = "all/type_lifecycle.rs"]
mod type_lifecycle;
#[path = "all/update_keyspace.rs"]
mod update_keyspace;
#[path = "all/update_keyspace_unsettled_resources.rs"]
mod update_keyspace_unsettled_resources;
#[path = "all/update_table.rs"]
mod update_table;
#[path = "all/update_table_add_columns_types.rs"]
mod update_table_add_columns_types;
#[path = "all/update_table_capacity_mode_re_set.rs"]
mod update_table_capacity_mode_re_set;
#[path = "all/update_table_capacity_mode_replica_settings.rs"]
mod update_table_capacity_mode_replica_settings;
#[path = "all/update_table_scaling_settings.rs"]
mod update_table_scaling_settings;
