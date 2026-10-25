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
#[path = "all/bandwidth_rate_limit_schedule.rs"]
mod bandwidth_rate_limit_schedule;
#[path = "all/gateway_activation.rs"]
mod gateway_activation;
#[path = "all/gateway_lifecycle.rs"]
mod gateway_lifecycle;
#[path = "all/gateway_server_association.rs"]
mod gateway_server_association;
#[path = "all/gateway_server_disassociation.rs"]
mod gateway_server_disassociation;
#[path = "all/hypervisor_configuration_test.rs"]
mod hypervisor_configuration_test;
#[path = "all/hypervisor_host_is_not_a_key.rs"]
mod hypervisor_host_is_not_a_key;
#[path = "all/hypervisor_import_concurrency.rs"]
mod hypervisor_import_concurrency;
#[path = "all/hypervisor_import_is_not_deduplicated.rs"]
mod hypervisor_import_is_not_deduplicated;
#[path = "all/hypervisor_listing.rs"]
mod hypervisor_listing;
#[path = "all/hypervisor_property_mappings.rs"]
mod hypervisor_property_mappings;
#[path = "all/hypervisor_reads.rs"]
mod hypervisor_reads;
#[path = "all/hypervisor_update.rs"]
mod hypervisor_update;
#[path = "all/maintenance_start_time.rs"]
mod maintenance_start_time;
#[path = "all/metadata_sync.rs"]
mod metadata_sync;
#[path = "all/metadata_sync_over_the_wire.rs"]
mod metadata_sync_over_the_wire;
#[path = "all/negative_paths.rs"]
mod negative_paths;
#[path = "all/put_bandwidth_rate_limit_schedule.rs"]
mod put_bandwidth_rate_limit_schedule;
#[path = "all/put_hypervisor_property_mappings.rs"]
mod put_hypervisor_property_mappings;
#[path = "all/resource_untagging.rs"]
mod resource_untagging;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/virtual_machine_inventory_sync.rs"]
mod virtual_machine_inventory_sync;
#[path = "all/virtual_machine_listing.rs"]
mod virtual_machine_listing;
#[path = "all/virtual_machine_reads.rs"]
mod virtual_machine_reads;
