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
#[path = "all/add_on_disable.rs"]
mod add_on_disable;
#[path = "all/alarm_configuration.rs"]
mod alarm_configuration;
#[path = "all/blueprint_catalog.rs"]
mod blueprint_catalog;
#[path = "all/bucket_access_keys.rs"]
mod bucket_access_keys;
#[path = "all/bucket_bundles.rs"]
mod bucket_bundles;
#[path = "all/bucket_error_envelope.rs"]
mod bucket_error_envelope;
#[path = "all/bucket_lifecycle.rs"]
mod bucket_lifecycle;
#[path = "all/bucket_object_data_plane.rs"]
mod bucket_object_data_plane;
#[path = "all/bundle_catalog.rs"]
mod bundle_catalog;
#[path = "all/contact_method_notifications.rs"]
mod contact_method_notifications;
#[path = "all/container_image_registry.rs"]
mod container_image_registry;
#[path = "all/container_image_registry_login.rs"]
mod container_image_registry_login;
#[path = "all/container_service_lifecycle.rs"]
mod container_service_lifecycle;
#[path = "all/container_service_metric_data.rs"]
mod container_service_metric_data;
#[path = "all/container_service_public_endpoint.rs"]
mod container_service_public_endpoint;
#[path = "all/cost_estimate.rs"]
mod cost_estimate;
#[path = "all/disk_attachment.rs"]
mod disk_attachment;
#[path = "all/disk_block_storage.rs"]
mod disk_block_storage;
#[path = "all/disk_delete.rs"]
mod disk_delete;
#[path = "all/disk_detach.rs"]
mod disk_detach;
#[path = "all/disk_restore_from_snapshot.rs"]
mod disk_restore_from_snapshot;
#[path = "all/disk_restore_snapshot_completeness.rs"]
mod disk_restore_snapshot_completeness;
#[path = "all/disk_snapshot.rs"]
mod disk_snapshot;
#[path = "all/disk_snapshot_delete.rs"]
mod disk_snapshot_delete;
#[path = "all/disk_snapshot_listing.rs"]
mod disk_snapshot_listing;
#[path = "all/distribution_cdn.rs"]
mod distribution_cdn;
#[path = "all/distribution_metric_data.rs"]
mod distribution_metric_data;
#[path = "all/domain_delete.rs"]
mod domain_delete;
#[path = "all/domain_entry_delete.rs"]
mod domain_entry_delete;
#[path = "all/domain_entry_recordset.rs"]
mod domain_entry_recordset;
#[path = "all/domain_entry_update.rs"]
mod domain_entry_update;
#[path = "all/domain_region_restriction.rs"]
mod domain_region_restriction;
#[path = "all/domain_zone.rs"]
mod domain_zone;
#[path = "all/export_snapshot_records.rs"]
mod export_snapshot_records;
#[path = "all/gui_session_access_details.rs"]
mod gui_session_access_details;
#[path = "all/instance_delete_attachments.rs"]
mod instance_delete_attachments;
#[path = "all/instance_error_envelope.rs"]
mod instance_error_envelope;
#[path = "all/instance_https_setup.rs"]
mod instance_https_setup;
#[path = "all/instance_lifecycle.rs"]
mod instance_lifecycle;
#[path = "all/instance_metric_data.rs"]
mod instance_metric_data;
#[path = "all/instance_public_ports.rs"]
mod instance_public_ports;
#[path = "all/instance_reboot.rs"]
mod instance_reboot;
#[path = "all/instance_restore_from_snapshot.rs"]
mod instance_restore_from_snapshot;
#[path = "all/instance_snapshot_listing.rs"]
mod instance_snapshot_listing;
#[path = "all/instance_snapshot_read.rs"]
mod instance_snapshot_read;
#[path = "all/load_balancer_delete.rs"]
mod load_balancer_delete;
#[path = "all/load_balancer_instance_attachment.rs"]
mod load_balancer_instance_attachment;
#[path = "all/load_balancer_listing.rs"]
mod load_balancer_listing;
#[path = "all/load_balancer_provisioning.rs"]
mod load_balancer_provisioning;
#[path = "all/load_balancer_tls_certificate_delete.rs"]
mod load_balancer_tls_certificate_delete;
#[path = "all/region_catalog.rs"]
mod region_catalog;
#[path = "all/relational_database_blueprints.rs"]
mod relational_database_blueprints;
#[path = "all/relational_database_bundles.rs"]
mod relational_database_bundles;
#[path = "all/relational_database_delete.rs"]
mod relational_database_delete;
#[path = "all/relational_database_lifecycle.rs"]
mod relational_database_lifecycle;
#[path = "all/relational_database_listing.rs"]
mod relational_database_listing;
#[path = "all/relational_database_log_streams.rs"]
mod relational_database_log_streams;
#[path = "all/relational_database_parameter_updates.rs"]
mod relational_database_parameter_updates;
#[path = "all/relational_database_parameters.rs"]
mod relational_database_parameters;
#[path = "all/relational_database_reboot.rs"]
mod relational_database_reboot;
#[path = "all/relational_database_restore_from_snapshot.rs"]
mod relational_database_restore_from_snapshot;
#[path = "all/relational_database_snapshot_listing.rs"]
mod relational_database_snapshot_listing;
#[path = "all/relational_database_snapshot_read.rs"]
mod relational_database_snapshot_read;
#[path = "all/relational_database_start_stop.rs"]
mod relational_database_start_stop;
#[path = "all/relational_database_update.rs"]
mod relational_database_update;
#[path = "all/resource_add_ons.rs"]
mod resource_add_ons;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/setup_history_reporting.rs"]
mod setup_history_reporting;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/static_ip_allocation.rs"]
mod static_ip_allocation;
#[path = "all/vpc_peering.rs"]
mod vpc_peering;
