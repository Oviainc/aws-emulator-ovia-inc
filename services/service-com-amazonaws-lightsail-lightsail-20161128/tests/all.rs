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
#[path = "all/alarm_configuration.rs"]
mod alarm_configuration;
#[path = "all/blueprint_catalog.rs"]
mod blueprint_catalog;
#[path = "all/bucket_access_keys.rs"]
mod bucket_access_keys;
#[path = "all/bucket_bundles.rs"]
mod bucket_bundles;
#[path = "all/bucket_lifecycle.rs"]
mod bucket_lifecycle;
#[path = "all/bucket_object_data_plane.rs"]
mod bucket_object_data_plane;
#[path = "all/bundle_catalog.rs"]
mod bundle_catalog;
#[path = "all/container_image_registry.rs"]
mod container_image_registry;
#[path = "all/container_service_lifecycle.rs"]
mod container_service_lifecycle;
#[path = "all/container_service_public_endpoint.rs"]
mod container_service_public_endpoint;
#[path = "all/disk_attachment.rs"]
mod disk_attachment;
#[path = "all/disk_block_storage.rs"]
mod disk_block_storage;
#[path = "all/disk_detach.rs"]
mod disk_detach;
#[path = "all/disk_restore_from_snapshot.rs"]
mod disk_restore_from_snapshot;
#[path = "all/disk_snapshot.rs"]
mod disk_snapshot;
#[path = "all/domain_entry_delete.rs"]
mod domain_entry_delete;
#[path = "all/domain_entry_recordset.rs"]
mod domain_entry_recordset;
#[path = "all/domain_entry_update.rs"]
mod domain_entry_update;
#[path = "all/domain_zone.rs"]
mod domain_zone;
#[path = "all/instance_error_envelope.rs"]
mod instance_error_envelope;
#[path = "all/instance_lifecycle.rs"]
mod instance_lifecycle;
#[path = "all/instance_metric_data.rs"]
mod instance_metric_data;
#[path = "all/instance_public_ports.rs"]
mod instance_public_ports;
#[path = "all/instance_reboot.rs"]
mod instance_reboot;
#[path = "all/load_balancer_provisioning.rs"]
mod load_balancer_provisioning;
#[path = "all/region_catalog.rs"]
mod region_catalog;
#[path = "all/relational_database_blueprints.rs"]
mod relational_database_blueprints;
#[path = "all/relational_database_bundles.rs"]
mod relational_database_bundles;
#[path = "all/relational_database_lifecycle.rs"]
mod relational_database_lifecycle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/static_ip_allocation.rs"]
mod static_ip_allocation;
