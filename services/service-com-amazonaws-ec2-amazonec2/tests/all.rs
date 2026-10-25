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
// Declared FIRST rather than in sorted position: a new ec2 suite lands every few minutes
// and inserts its two lines in sorted order, so the sorted neighbourhood of this suite is
// the contended region of this file — this fix lost several serialized squash merges to
// exactly that. The lines above are the file header, which no landing edits.
#[path = "all/prefix_lists.rs"]
mod prefix_lists;
// The virtual private gateway slice: create, read back, delete. Declared here rather than in
// sorted position for the reason given above — the sorted neighbourhood is the contended
// region of this file, and this suite lost several serialized squash merges to it.
#[path = "all/vpn_gateways.rs"]
mod vpn_gateways;
// What `DescribeClientVpnTargetNetworks` reports for the target networks associated with one
// Client VPN endpoint, and how the ids, the filters and the page size narrow it. Declared
// here rather than in sorted position, for the reason the note above gives.
#[path = "all/client_vpn_target_networks.rs"]
mod client_vpn_target_networks;
// What `ResetInstanceAttribute` resets, to what, from which instance states, and what it
// refuses. Declared beside `prefix_lists` for the same reason: the sorted region is where
// every new ec2 suite inserts its lines.
#[path = "all/instance_attribute_reset.rs"]
mod instance_attribute_reset;
// The root volume replacement slice: create a task, then read it back. Declared beside the
// two above for the same reason — the sorted region is where every new ec2 suite inserts
// its lines, and this fix lost three serialized squash merges to exactly that.
#[path = "all/replace_root_volume_tasks.rs"]
mod replace_root_volume_tasks;
// Declared here rather than in sorted position, for the reason above: the sorted
// neighbourhood of this suite is the contended region of this file.
#[path = "all/local_gateway_routes.rs"]
mod local_gateway_routes;
// The transit gateway prefix list reference slice (create, then read it back) and the
// error envelope's request id. Declared beside the suites above for the reason they
// give: the sorted region is where every new ec2 suite inserts its lines, and these two
// lines have already lost several serialized squash merges there.
#[path = "all/transit_gateway_prefix_list_references.rs"]
mod transit_gateway_prefix_list_references;
#[path = "all/ec2_query_error_envelope.rs"]
mod ec2_query_error_envelope;
#[path = "all/address_transfers.rs"]
mod address_transfers;
#[path = "all/allowed_images_settings.rs"]
mod allowed_images_settings;
#[path = "all/application_status.rs"]
mod application_status;
#[path = "all/application_status_checks.rs"]
mod application_status_checks;
#[path = "all/associate_address.rs"]
mod associate_address;
#[path = "all/associate_subnet_cidr_block.rs"]
mod associate_subnet_cidr_block;
#[path = "all/authorize_security_group_egress.rs"]
mod authorize_security_group_egress;
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
#[path = "all/capacity_reservation_billing_requests.rs"]
mod capacity_reservation_billing_requests;
#[path = "all/capacity_reservation_fleets.rs"]
mod capacity_reservation_fleets;
#[path = "all/carrier_gateways.rs"]
mod carrier_gateways;
#[path = "all/coip_pools.rs"]
mod coip_pools;
#[path = "all/confirm_product_instance.rs"]
mod confirm_product_instance;
#[path = "all/copy_volumes.rs"]
mod copy_volumes;
#[path = "all/create_security_group.rs"]
mod create_security_group;
#[path = "all/dedicated_hosts.rs"]
mod dedicated_hosts;
#[path = "all/default_network.rs"]
mod default_network;
#[path = "all/default_network_per_region.rs"]
mod default_network_per_region;
#[path = "all/delete_coip_pool.rs"]
mod delete_coip_pool;
#[path = "all/delete_network_interface.rs"]
mod delete_network_interface;
#[path = "all/delete_security_group.rs"]
mod delete_security_group;
#[path = "all/delete_vpc.rs"]
mod delete_vpc;
#[path = "all/describe_images.rs"]
mod describe_images;
#[path = "all/describe_page_truncation.rs"]
mod describe_page_truncation;
#[path = "all/describe_security_groups.rs"]
mod describe_security_groups;
#[path = "all/describe_subnets.rs"]
mod describe_subnets;
#[path = "all/describe_vpc_attribute.rs"]
mod describe_vpc_attribute;
#[path = "all/describe_vpcs.rs"]
mod describe_vpcs;
// What `DetachVolume` refuses and what it writes beyond the detach path in
// `fpga_image_tasks_and_volume_io`: the `IncorrectState` wording, the owed `detached`
// instant, and an attachment to an Amazon Web Services-managed resource.
#[path = "all/detach_volume.rs"]
mod detach_volume;
// The DHCP option set a VPC inherits: the Region's one default set, and what
// `DescribeDhcpOptions` reports for it.
#[path = "all/dhcp_options.rs"]
mod dhcp_options;
#[path = "all/dhcp_options_association.rs"]
mod dhcp_options_association;
#[path = "all/dry_run_and_unknown_ids.rs"]
mod dry_run_and_unknown_ids;
// The account-level EBS encryption-by-default setting and the CreateVolume default it
// decides.
#[path = "all/ebs_encryption_by_default.rs"]
mod ebs_encryption_by_default;
// The shared ec2Query plumbing every suite drives the service through.
#[path = "all/ec2_query.rs"]
mod ec2_query;
// The ec2Query response envelope: the success flag and the media type every response
// carries, whichever operation answered.
#[path = "all/ec2_query_envelope.rs"]
mod ec2_query_envelope;
// Which request-id element each side of one operation's envelope reports: the pair on
// `CreateVpnConnectionRoute`, on top of the across-operations rule
// `ec2_query_error_envelope` owns.
#[path = "all/ec2_query_error_request_id.rs"]
mod ec2_query_error_request_id;
#[path = "all/elastic_ip_addresses.rs"]
mod elastic_ip_addresses;
#[path = "all/fast_launch_images.rs"]
mod fast_launch_images;
// What `DisableFastSnapshotRestores` answers per (snapshot, Availability Zone) pair.
#[path = "all/fast_snapshot_restores.rs"]
mod fast_snapshot_restores;
#[path = "all/filter_matcher_budget.rs"]
mod filter_matcher_budget;
#[path = "all/fleet_instances.rs"]
mod fleet_instances;
#[path = "all/fpga_image_tasks_and_volume_io.rs"]
mod fpga_image_tasks_and_volume_io;
#[path = "all/get_password_data.rs"]
mod get_password_data;
#[path = "all/host_reservation_offerings.rs"]
mod host_reservation_offerings;
#[path = "all/image_references.rs"]
mod image_references;
#[path = "all/image_usage_reports.rs"]
mod image_usage_reports;
#[path = "all/images.rs"]
mod images;
#[path = "all/import_image_tasks.rs"]
mod import_image_tasks;
#[path = "all/import_snapshot_tasks.rs"]
mod import_snapshot_tasks;
#[path = "all/instance_event_notification_attributes.rs"]
mod instance_event_notification_attributes;
#[path = "all/instance_event_windows.rs"]
mod instance_event_windows;
#[path = "all/instance_lifecycle.rs"]
mod instance_lifecycle;
#[path = "all/instance_metadata_defaults.rs"]
mod instance_metadata_defaults;
#[path = "all/instance_type_offerings.rs"]
mod instance_type_offerings;
#[path = "all/instance_types.rs"]
mod instance_types;
// Associating an ASN brought to IPAM with a BYOIP address range IPAM manages.
#[path = "all/ipam_byoasn_associations.rs"]
mod ipam_byoasn_associations;
// Allocating unused capacity out of a Capacity Reservation into an interruptible one.
#[path = "all/interruptible_capacity_reservation_allocations.rs"]
mod interruptible_capacity_reservation_allocations;
#[path = "all/ipam_lifecycle.rs"]
mod ipam_lifecycle;
#[path = "all/ipam_policies.rs"]
mod ipam_policies;
#[path = "all/ipam_pools.rs"]
mod ipam_pools;
#[path = "all/ipam_routing_policy_registrations.rs"]
mod ipam_routing_policy_registrations;
#[path = "all/launch_templates.rs"]
mod launch_templates;
#[path = "all/local_gateway_route_table_virtual_interface_group_associations.rs"]
mod local_gateway_route_table_virtual_interface_group_associations;
#[path = "all/local_gateway_virtual_interface_groups.rs"]
mod local_gateway_virtual_interface_groups;
#[path = "all/mac_dedicated_hosts.rs"]
mod mac_dedicated_hosts;
#[path = "all/modify_subnet_attribute.rs"]
mod modify_subnet_attribute;
#[path = "all/nat_gateway_addresses.rs"]
mod nat_gateway_addresses;
#[path = "all/network_insights_access_scope_analyses.rs"]
mod network_insights_access_scope_analyses;
#[path = "all/network_insights_access_scopes.rs"]
mod network_insights_access_scopes;
#[path = "all/network_interfaces.rs"]
mod network_interfaces;
#[path = "all/network_performance_data.rs"]
mod network_performance_data;
#[path = "all/network_performance_subscriptions.rs"]
mod network_performance_subscriptions;
#[path = "all/placement_groups.rs"]
mod placement_groups;
#[path = "all/principal_id_format.rs"]
mod principal_id_format;
#[path = "all/public_ipv4_pools.rs"]
mod public_ipv4_pools;
// What `DeleteQueuedReservedInstances` deletes and what it reports per id.
#[path = "all/queued_reserved_instances.rs"]
mod queued_reserved_instances;
// What `DescribeTransitGatewayMulticastDomains` reports out of the domain store, and how the
// id list, the documented filters and the page size narrow it. Declared HERE rather than in
// sorted position, for the reason this file's header note and the relocations at its head and
// tail record: a suite's own sorted neighbourhood is where concurrent ec2 landings insert
// their two lines, and the `transit_gateway_*` cluster is the busiest of them.
#[path = "all/transit_gateway_multicast_domains.rs"]
mod transit_gateway_multicast_domains;
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
#[path = "all/spot_datafeed_subscription.rs"]
mod spot_datafeed_subscription;
#[path = "all/spot_instance_requests.rs"]
mod spot_instance_requests;
#[path = "all/spot_placement_scores.rs"]
mod spot_placement_scores;
#[path = "all/stale_security_groups.rs"]
mod stale_security_groups;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/store_image_tasks.rs"]
mod store_image_tasks;
// Reserving part of a subnet's CIDR range, and reading the reservations of one subnet back.
#[path = "all/subnet_cidr_reservations.rs"]
mod subnet_cidr_reservations;
#[path = "all/transit_gateway_acceptance.rs"]
mod transit_gateway_acceptance;
#[path = "all/transit_gateway_connect_peers.rs"]
mod transit_gateway_connect_peers;
#[path = "all/transit_gateway_lifecycle.rs"]
mod transit_gateway_lifecycle;
#[path = "all/transit_gateway_policy_table_associations.rs"]
mod transit_gateway_policy_table_associations;
#[path = "all/transit_gateway_rejection.rs"]
mod transit_gateway_rejection;
#[path = "all/transit_gateway_route_table_propagation.rs"]
mod transit_gateway_route_table_propagation;
// The Verified Access ENDPOINT slice: create an endpoint on a stored group, then read it
// back through the describe.
#[path = "all/verified_access_endpoints.rs"]
mod verified_access_endpoints;
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
// The volume status check slice: what `DescribeVolumeStatus` reports for the stored
// volumes, and how `VolumeIds`, the filters and the page size narrow it.
#[path = "all/volume_status.rs"]
mod volume_status;
// `RestoreVolumeFromRecycleBin`: what the bin gives back, and what it refuses.
#[path = "all/volumes_in_recycle_bin.rs"]
mod volumes_in_recycle_bin;
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
#[path = "all/vpc_resources_blocking_encryption_enforcement.rs"]
mod vpc_resources_blocking_encryption_enforcement;
#[path = "all/vpn_connection_device_types.rs"]
mod vpn_connection_device_types;
#[path = "all/vpn_connection_routes.rs"]
mod vpn_connection_routes;
// Reading one VPC back out of an ec2Query response, on top of `ec2_query`'s plumbing.
#[path = "all/wire.rs"]
mod wire;
// Stopping the advertisement of a BYOIP address range, and what that refuses.
#[path = "all/withdraw_byoip_cidr.rs"]
mod withdraw_byoip_cidr;

// Declared LAST rather than in sorted position, for the reason the header gives: the
// sorted neighbourhood of these two suites is where other ec2 landings insert their lines,
// and the head of the list has become contended too. What `DescribeSecurityGroupRules`
// reports out of the rule store, and the byte and record ceilings every store of this
// crate is held to.
#[path = "all/security_group_rules.rs"]
mod security_group_rules;
#[path = "all/store_ceilings.rs"]
mod store_ceilings;
// Declared LAST rather than in sorted position, for the reason the note at the top of this
// file records for `prefix_lists`: a new ec2 suite lands every few minutes and inserts its
// two lines in sorted order, so a suite's own sorted neighbourhood is the contended region
// of this file — and this one lost repeated serialized squash merges to exactly that.
#[path = "all/traffic_mirror_filters.rs"]
mod traffic_mirror_filters;
// What `CreateCapacityReservation` reserves and what every reader of a Capacity
// Reservation then reports. Declared LAST rather than in sorted position, for the reason
// the note at the top of this file records: a new ec2 suite lands every few minutes and
// inserts its two lines in sorted order, so the sorted neighbourhood of this suite is the
// contended region of this file — and these two lines have already lost repeated
// serialized squash merges there.
#[path = "all/capacity_reservations.rs"]
mod capacity_reservations;
// What `CancelExportTask` cancels, what it refuses and what it leaves alone. Declared LAST
// rather than in sorted position, for the reason the header records: a suite's own sorted
// neighbourhood is where every new ec2 suite inserts its lines.
#[path = "all/export_task_cancellation.rs"]
mod export_task_cancellation;
// Declared LAST for the same reason as the two suites above: the sorted region of this file
// is where every new ec2 suite inserts its lines, and a landing that loses the squash merge
// has to redo it.
// Associating a VPC with a local gateway route table, and reading the association back.
#[path = "all/local_gateway_route_table_vpc_associations.rs"]
mod local_gateway_route_table_vpc_associations;
// Declared LAST for the reason the header gives: the sorted region of this file is where
// every new ec2 suite inserts its lines. What `DeleteTags` removes from a resource of any
// kind, and what it refuses.
#[path = "all/resource_tags.rs"]
mod resource_tags;
// Giving Dedicated Hosts back: which of the named hosts `ReleaseHosts` releases, what it
// reports per id, and what a released host still looks like through `DescribeHosts`.
// Declared LAST for the reason the header gives.
#[path = "all/release_hosts.rs"]
mod release_hosts;
// Declared LAST for the reason the header gives: taking one AMI back out of the Recycle
// Bin, and what that refuses.
#[path = "all/image_recycle_bin.rs"]
mod image_recycle_bin;
// Declared LAST for the reason the header gives: copying one Amazon FPGA Image, what the
// copy computes from the stored source, and what it refuses.
#[path = "all/copy_fpga_image.rs"]
mod copy_fpga_image;
