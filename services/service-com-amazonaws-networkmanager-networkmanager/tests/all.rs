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
#[path = "all/arn_scoping.rs"]
mod arn_scoping;
#[path = "all/associate_customer_gateway_scoping.rs"]
mod associate_customer_gateway_scoping;
#[path = "all/attachment_acceptance.rs"]
mod attachment_acceptance;
#[path = "all/attachment_lifecycle.rs"]
mod attachment_lifecycle;
#[path = "all/attachment_routing_policy_associations.rs"]
mod attachment_routing_policy_associations;
#[path = "all/connect_peer_association_lifecycle.rs"]
mod connect_peer_association_lifecycle;
#[path = "all/connect_peer_delete.rs"]
mod connect_peer_delete;
#[path = "all/connect_peer_lifecycle.rs"]
mod connect_peer_lifecycle;
#[path = "all/connection_lifecycle.rs"]
mod connection_lifecycle;
#[path = "all/core_network_change_events.rs"]
mod core_network_change_events;
#[path = "all/core_network_change_set.rs"]
mod core_network_change_set;
#[path = "all/core_network_lifecycle.rs"]
mod core_network_lifecycle;
#[path = "all/core_network_policy_version_listing.rs"]
mod core_network_policy_version_listing;
#[path = "all/core_network_policy_version_restore.rs"]
mod core_network_policy_version_restore;
#[path = "all/core_network_policy_versions.rs"]
mod core_network_policy_versions;
#[path = "all/core_network_prefix_list_association.rs"]
mod core_network_prefix_list_association;
#[path = "all/core_network_prefix_list_associations.rs"]
mod core_network_prefix_list_associations;
#[path = "all/core_network_segment_actions.rs"]
mod core_network_segment_actions;
#[path = "all/create_vpc_attachment_bounds.rs"]
mod create_vpc_attachment_bounds;
#[path = "all/customer_gateway_association_lifecycle.rs"]
mod customer_gateway_association_lifecycle;
#[path = "all/delete_attachment.rs"]
mod delete_attachment;
#[path = "all/delete_connection.rs"]
mod delete_connection;
#[path = "all/delete_dependency_refusal.rs"]
mod delete_dependency_refusal;
#[path = "all/delete_device.rs"]
mod delete_device;
#[path = "all/delete_link.rs"]
mod delete_link;
#[path = "all/describe_global_networks_id_resolution.rs"]
mod describe_global_networks_id_resolution;
#[path = "all/device_lifecycle.rs"]
mod device_lifecycle;
#[path = "all/direct_connect_gateway_attachment_lifecycle.rs"]
mod direct_connect_gateway_attachment_lifecycle;
#[path = "all/filter_id_list_maximum.rs"]
mod filter_id_list_maximum;
#[path = "all/get_connect_attachment.rs"]
mod get_connect_attachment;
#[path = "all/get_connections.rs"]
mod get_connections;
#[path = "all/global_network_lifecycle.rs"]
mod global_network_lifecycle;
#[path = "all/link_association_lifecycle.rs"]
mod link_association_lifecycle;
#[path = "all/link_lifecycle.rs"]
mod link_lifecycle;
#[path = "all/list_attachments.rs"]
mod list_attachments;
#[path = "all/list_tags_for_resource.rs"]
mod list_tags_for_resource;
#[path = "all/network_resource_counts.rs"]
mod network_resource_counts;
#[path = "all/network_resource_regions.rs"]
mod network_resource_regions;
#[path = "all/network_resources.rs"]
mod network_resources;
#[path = "all/network_telemetry.rs"]
mod network_telemetry;
#[path = "all/organization_service_access.rs"]
mod organization_service_access;
#[path = "all/resource_not_found_message.rs"]
mod resource_not_found_message;
#[path = "all/resource_policy_lifecycle.rs"]
mod resource_policy_lifecycle;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/route_analysis.rs"]
mod route_analysis;
#[path = "all/site_to_site_vpn_attachment_lifecycle.rs"]
mod site_to_site_vpn_attachment_lifecycle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/transit_gateway_connect_peer_association_lifecycle.rs"]
mod transit_gateway_connect_peer_association_lifecycle;
#[path = "all/transit_gateway_peering.rs"]
mod transit_gateway_peering;
#[path = "all/transit_gateway_peering_create_bounds.rs"]
mod transit_gateway_peering_create_bounds;
#[path = "all/transit_gateway_registration_lifecycle.rs"]
mod transit_gateway_registration_lifecycle;
#[path = "all/transit_gateway_route_table_attachment.rs"]
mod transit_gateway_route_table_attachment;
#[path = "all/transit_gateway_route_table_attachment_lifecycle.rs"]
mod transit_gateway_route_table_attachment_lifecycle;
#[path = "all/unknown_parent_reference.rs"]
mod unknown_parent_reference;
#[path = "all/update_link.rs"]
mod update_link;
#[path = "all/update_site.rs"]
mod update_site;
#[path = "all/vpc_attachment_lifecycle.rs"]
mod vpc_attachment_lifecycle;
