// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/describe_vpcs.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/address_transfers.rs"]
mod address_transfers;
#[path = "all/allowed_images_settings.rs"]
mod allowed_images_settings;
#[path = "all/application_status.rs"]
mod application_status;
#[path = "all/associate_address.rs"]
mod associate_address;
#[path = "all/availability_zones.rs"]
mod availability_zones;
#[path = "all/byoip_cidr_lifecycle.rs"]
mod byoip_cidr_lifecycle;
#[path = "all/byoip_cidrs.rs"]
mod byoip_cidrs;
#[path = "all/capacity_block_offerings.rs"]
mod capacity_block_offerings;
#[path = "all/capacity_blocks.rs"]
mod capacity_blocks;
#[path = "all/capacity_reservation_fleets.rs"]
mod capacity_reservation_fleets;
#[path = "all/coip_pools.rs"]
mod coip_pools;
#[path = "all/copy_volumes.rs"]
mod copy_volumes;
#[path = "all/create_security_group.rs"]
mod create_security_group;
#[path = "all/dedicated_hosts.rs"]
mod dedicated_hosts;
#[path = "all/default_network.rs"]
mod default_network;
#[path = "all/delete_vpc.rs"]
mod delete_vpc;
#[path = "all/describe_images.rs"]
mod describe_images;
#[path = "all/describe_security_groups.rs"]
mod describe_security_groups;
#[path = "all/describe_subnets.rs"]
mod describe_subnets;
#[path = "all/describe_vpc_attribute.rs"]
mod describe_vpc_attribute;
#[path = "all/describe_vpcs.rs"]
mod describe_vpcs;
#[path = "all/dry_run_and_unknown_ids.rs"]
mod dry_run_and_unknown_ids;
// The shared ec2Query plumbing every suite drives the service through.
#[path = "all/ec2_query.rs"]
mod ec2_query;
// The ec2Query response envelope: the success flag and the media type every response
// carries, whichever operation answered.
#[path = "all/ec2_query_envelope.rs"]
mod ec2_query_envelope;
#[path = "all/elastic_ip_addresses.rs"]
mod elastic_ip_addresses;
#[path = "all/fast_launch_images.rs"]
mod fast_launch_images;
#[path = "all/fpga_image_tasks_and_volume_io.rs"]
mod fpga_image_tasks_and_volume_io;
#[path = "all/get_password_data.rs"]
mod get_password_data;
#[path = "all/image_usage_reports.rs"]
mod image_usage_reports;
#[path = "all/images.rs"]
mod images;
#[path = "all/import_image_tasks.rs"]
mod import_image_tasks;
#[path = "all/instance_lifecycle.rs"]
mod instance_lifecycle;
#[path = "all/instance_type_offerings.rs"]
mod instance_type_offerings;
#[path = "all/instance_types.rs"]
mod instance_types;
#[path = "all/ipam_routing_policy_registrations.rs"]
mod ipam_routing_policy_registrations;
#[path = "all/launch_templates.rs"]
mod launch_templates;
#[path = "all/local_gateway_route_table_virtual_interface_group_associations.rs"]
mod local_gateway_route_table_virtual_interface_group_associations;
#[path = "all/mac_dedicated_hosts.rs"]
mod mac_dedicated_hosts;
#[path = "all/modify_subnet_attribute.rs"]
mod modify_subnet_attribute;
#[path = "all/network_insights_access_scope_analyses.rs"]
mod network_insights_access_scope_analyses;
#[path = "all/network_performance_data.rs"]
mod network_performance_data;
#[path = "all/placement_groups.rs"]
mod placement_groups;
#[path = "all/principal_id_format.rs"]
mod principal_id_format;
#[path = "all/regions.rs"]
mod regions;
#[path = "all/resource_specific_error_codes.rs"]
mod resource_specific_error_codes;
#[path = "all/route_server_peers.rs"]
mod route_server_peers;
#[path = "all/route_servers.rs"]
mod route_servers;
#[path = "all/secondary_networks.rs"]
mod secondary_networks;
#[path = "all/secondary_subnets.rs"]
mod secondary_subnets;
#[path = "all/security_group_filter_limits.rs"]
mod security_group_filter_limits;
#[path = "all/security_groups.rs"]
mod security_groups;
#[path = "all/snapshot_block_public_access.rs"]
mod snapshot_block_public_access;
#[path = "all/spot_instance_requests.rs"]
mod spot_instance_requests;
#[path = "all/spot_placement_scores.rs"]
mod spot_placement_scores;
#[path = "all/stale_security_groups.rs"]
mod stale_security_groups;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/transit_gateway_acceptance.rs"]
mod transit_gateway_acceptance;
#[path = "all/transit_gateway_lifecycle.rs"]
mod transit_gateway_lifecycle;
#[path = "all/transit_gateway_policy_table_associations.rs"]
mod transit_gateway_policy_table_associations;
#[path = "all/transit_gateway_route_table_propagation.rs"]
mod transit_gateway_route_table_propagation;
// The Verified Access trust provider slice: create, then read it back.
#[path = "all/verified_access_trust_providers.rs"]
mod verified_access_trust_providers;
// The volume attribute slice: modify one attribute of a volume, then describe it back.
#[path = "all/volume_attributes.rs"]
mod volume_attributes;
#[path = "all/volume_lifecycle.rs"]
mod volume_lifecycle;
#[path = "all/volume_modifications.rs"]
mod volume_modifications;
#[path = "all/vpc_attributes.rs"]
mod vpc_attributes;
#[path = "all/vpc_block_public_access_exclusions.rs"]
mod vpc_block_public_access_exclusions;
#[path = "all/vpc_endpoint_connections.rs"]
mod vpc_endpoint_connections;
#[path = "all/vpc_endpoints_and_byoip.rs"]
mod vpc_endpoints_and_byoip;
#[path = "all/vpc_lifecycle.rs"]
mod vpc_lifecycle;
#[path = "all/vpc_network_lifecycle.rs"]
mod vpc_network_lifecycle;
#[path = "all/vpn_connection_device_types.rs"]
mod vpn_connection_device_types;
// Reading one VPC back out of an ec2Query response, on top of `ec2_query`'s plumbing.
#[path = "all/wire.rs"]
mod wire;
// Stopping the advertisement of a BYOIP address range, and what that refuses.
#[path = "all/withdraw_byoip_cidr.rs"]
mod withdraw_byoip_cidr;
