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
#[path = "all/application_status.rs"]
mod application_status;
#[path = "all/availability_zones.rs"]
mod availability_zones;
#[path = "all/byoip_cidr_lifecycle.rs"]
mod byoip_cidr_lifecycle;
#[path = "all/capacity_blocks.rs"]
mod capacity_blocks;
#[path = "all/coip_pools.rs"]
mod coip_pools;
#[path = "all/copy_volumes.rs"]
mod copy_volumes;
#[path = "all/create_security_group.rs"]
mod create_security_group;
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
// The shared ec2Query plumbing every suite drives the service through.
#[path = "all/ec2_query.rs"]
mod ec2_query;
#[path = "all/elastic_ip_addresses.rs"]
mod elastic_ip_addresses;
#[path = "all/fast_launch_images.rs"]
mod fast_launch_images;
#[path = "all/images.rs"]
mod images;
#[path = "all/instance_lifecycle.rs"]
mod instance_lifecycle;
#[path = "all/instance_type_offerings.rs"]
mod instance_type_offerings;
#[path = "all/launch_templates.rs"]
mod launch_templates;
#[path = "all/mac_dedicated_hosts.rs"]
mod mac_dedicated_hosts;
#[path = "all/modify_subnet_attribute.rs"]
mod modify_subnet_attribute;
#[path = "all/security_group_filter_limits.rs"]
mod security_group_filter_limits;
#[path = "all/security_groups.rs"]
mod security_groups;
#[path = "all/snapshot_block_public_access.rs"]
mod snapshot_block_public_access;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/transit_gateway_lifecycle.rs"]
mod transit_gateway_lifecycle;
#[path = "all/volume_lifecycle.rs"]
mod volume_lifecycle;
#[path = "all/vpc_attributes.rs"]
mod vpc_attributes;
#[path = "all/vpc_endpoint_connections.rs"]
mod vpc_endpoint_connections;
#[path = "all/vpc_lifecycle.rs"]
mod vpc_lifecycle;
#[path = "all/vpc_network_lifecycle.rs"]
mod vpc_network_lifecycle;
// Reading one VPC back out of an ec2Query response, on top of `ec2_query`'s plumbing.
#[path = "all/wire.rs"]
mod wire;
