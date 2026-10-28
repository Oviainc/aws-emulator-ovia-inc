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
// What `SendDiagnosticInterrupt` delivers to one instance, which states it can be sent
// from, and what it refuses. Declared beside `prefix_lists` for the reason the note above
// gives: the sorted neighbourhood and the tail of this file are where every new ec2 suite
// inserts its lines.
#[path = "all/send_diagnostic_interrupt.rs"]
mod send_diagnostic_interrupt;
// What `CreateLocalGatewayRoute` writes into one local gateway route table, what the clock
// then owes the route, and what the create refuses. Declared beside `prefix_lists` for the
// reason the note above gives: the sorted neighbourhood and the tail of this file are where
// every new ec2 suite inserts its lines.
#[path = "all/create_local_gateway_route.rs"]
mod create_local_gateway_route;
// The IAM instance profile an instance runs as: what the associate records, what the two
// describes report of it, and what the disassociate takes away. Declared beside
// `prefix_lists` for the reason the note above gives: the sorted neighbourhood and the tail
// of this file are where every new ec2 suite inserts its lines.
#[path = "all/iam_instance_profile_associations.rs"]
mod iam_instance_profile_associations;
// What `DeleteVolume` removes, what it refuses, and what a dry run of it performs. Declared
// beside `prefix_lists` for the reason the note above gives: the sorted neighbourhood is where
// every new ec2 suite inserts its two lines.
#[path = "all/delete_volume.rs"]
mod delete_volume;
// The endpoint services a caller can create a VPC endpoint to: the Region's catalogue and the
// account's own service configurations. Declared beside `prefix_lists` for the reason the note
// above gives — the sorted neighbourhood is where every new ec2 suite inserts its two lines.
#[path = "all/vpc_endpoint_services.rs"]
mod vpc_endpoint_services;
// The `domain-name` attribute of an Elastic IP address: what `ResetAddressAttribute` starts
// on one address's reverse DNS record, what it refuses, and what the report reads back.
// Declared beside `prefix_lists` for the reason the note above gives: the sorted
// neighbourhood and the tail of this file are where every new ec2 suite inserts its lines.
#[path = "all/address_attribute_reset.rs"]
mod address_attribute_reset;
// What `AllocateHosts` writes: how many hosts one call mints, the configuration each of them
// carries, and that an earlier call's hosts are left as stored. Declared here rather than in
// sorted position, for the reason the note above gives — the sorted neighbourhood is the
// contended region of this file.
#[path = "all/allocate_hosts.rs"]
mod allocate_hosts;
// What a delete does to the state behind it: the dependency refusals and the reads of a
// resource that is gone. Declared here rather than in sorted position, for the reason the
// note above gives — the sorted neighbourhood is the contended region of this file.
#[path = "all/deleted_resources_and_dependencies.rs"]
mod deleted_resources_and_dependencies;
// What `CreateVpcEndpoint` stores for each endpoint type, and what it refuses. Declared here
// rather than in sorted position, for the reason the note above gives: the sorted
// neighbourhood is where every new ec2 suite inserts its lines.
#[path = "all/create_vpc_endpoint.rs"]
mod create_vpc_endpoint;
// Associating a virtual interface group with a local gateway route table, and reading the
// association back. Declared beside `prefix_lists` for the reason the note above gives: the
// sorted region AND the tail of this file are where every new ec2 suite inserts its lines,
// and these two lines lost several serialized squash merges to the tail.
#[path = "all/create_local_gateway_route_table_virtual_interface_group_association.rs"]
mod create_local_gateway_route_table_virtual_interface_group_association;
// What `CreateClientVpnRoute` adds to one Client VPN endpoint's route table, what it refuses,
// and what `DescribeClientVpnRoutes` reports back. Declared BELOW the two suites above rather
// than straight after `prefix_lists`: that first position is where every new ec2 suite
// inserts its lines, and these two lost three serialized squash merges to exactly that.
#[path = "all/client_vpn_routes.rs"]
mod client_vpn_routes;
// The field vocabulary a `CreateFlowLogs` `LogFormat` draws from: a format naming a field a
// flow log record does not have is refused and stores nothing. Declared beside the suites
// above rather than in sorted position, for the reason this file's header note gives: the
// sorted neighbourhood is where every new ec2 suite inserts its two lines.
#[path = "all/flow_log_format_fields.rs"]
mod flow_log_format_fields;
// The message an authorize is refused with once a security group holds as many rules in one
// direction as the account's quota allows. Declared beside the suites above for the same
// reason.
#[path = "all/security_group_rule_quota_refusal.rs"]
mod security_group_rule_quota_refusal;
// What `ModifyReservedInstances` records for the Reserved Instances it names, what the
// clock's fulfilment of it replaces them with, and what it refuses. Declared beside the
// suites above rather than in sorted position, for the reason this file's header note
// gives: the sorted neighbourhood is where every new ec2 suite inserts its two lines.
#[path = "all/reserved_instances_modifications.rs"]
mod reserved_instances_modifications;
// Where a paged describe bounds its page size and where it does not: a `MaxResults` the
// model leaves unbounded above is served, not refused. Declared beside the suites above
// rather than in sorted position, for the reason this file's header note gives.
#[path = "all/unranged_max_results.rs"]
mod unranged_max_results;
// What `RunInstances` does with a `TagSpecifications` entry, per resource type. Declared
// beside the suites above for the same reason.
#[path = "all/run_instances_tag_specifications.rs"]
mod run_instances_tag_specifications;
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
// What `ReportInstanceStatus` collects, which instances it accepts feedback about, and what
// it refuses. Declared beside the suites above rather than in sorted position, for the reason
// this file's header note gives: the sorted neighbourhood is where every new ec2 suite
// inserts its two lines, and that is where a serialized squash merge is lost.
#[path = "all/report_instance_status.rs"]
mod report_instance_status;
// Declared here rather than in sorted position, for the reason above: the sorted
// neighbourhood of this suite is the contended region of this file.
#[path = "all/local_gateway_routes.rs"]
mod local_gateway_routes;
// The Reachability Analyzer path analysis slice: what `StartNetworkInsightsAnalysis` stores
// for one path, what the clock then does to it, and how `DescribeNetworkInsightsAnalyses`
// narrows, pages and refuses. Declared here rather than in sorted position, for the reason
// this file's header note gives: the sorted neighbourhood is the contended region.
#[path = "all/network_insights_analyses.rs"]
mod network_insights_analyses;
// What `DeleteNetworkInsightsPath` takes out of the path store, what it answers, what it
// refuses and what a dry run of it performs. Declared beside the analyses suite above —
// topically its neighbour — rather than in sorted position or at the tail, for the reason
// this file's header note gives: both are the contended regions where a serialized squash
// merge is lost.
#[path = "all/delete_network_insights_path.rs"]
mod delete_network_insights_path;
// The transit gateway prefix list reference slice (create, then read it back) and the
// error envelope's request id. Declared beside the suites above for the reason they
// give: the sorted region is where every new ec2 suite inserts its lines, and these two
// lines have already lost several serialized squash merges there.
#[path = "all/transit_gateway_prefix_list_references.rs"]
mod transit_gateway_prefix_list_references;
// What `DescribeTransitGatewayRouteTableAnnouncements` reports out of the announcement store,
// and how the id list, the filters and the page size narrow it. Declared beside the transit
// gateway suite above rather than in sorted position or at the tail, for the reason this
// file's header note gives: BOTH the sorted neighbourhood and the tail are where every new
// ec2 suite inserts its two lines, and these two lines lost four serialized squash merges at
// the tail.
#[path = "all/transit_gateway_route_table_announcements.rs"]
mod transit_gateway_route_table_announcements;
// What `CreateTransitGatewayRouteTableAnnouncement` advertises, and what it refuses.
#[path = "all/create_transit_gateway_route_table_announcement.rs"]
mod create_transit_gateway_route_table_announcement;
// What `RebootInstances` queues, which instances it can be queued for, and what it
// refuses. Declared beside the suites above rather than in sorted position or at the tail,
// for the reason this file's header note gives: both are contended regions where a
// serialized squash merge is lost.
#[path = "all/reboot_instances.rs"]
mod reboot_instances;
#[path = "all/ec2_query_error_envelope.rs"]
mod ec2_query_error_envelope;
// The same envelope rule, driven over seven further operations and the four refusal kinds
// they answer. Declared beside the suite that owns the rule rather than in sorted position,
// for the reason this file's header note gives: the sorted neighbourhood is where every new
// ec2 suite inserts its two lines.
#[path = "all/ec2_query_error_envelope_operations.rs"]
mod ec2_query_error_envelope_operations;
// What `CreatePublicIpv4Pool` mints and persists, read back through the describe. Declared
// here rather than in sorted position, for the reason the suites above give: the sorted
// neighbourhood is where every new ec2 suite inserts its lines, and these two lines have
// already lost several serialized squash merges there.
#[path = "all/create_public_ipv4_pool.rs"]
mod create_public_ipv4_pool;
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
#[path = "all/associate_client_vpn_target_network.rs"]
mod associate_client_vpn_target_network;
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
// The availability of the capacity of the account's Capacity Blocks, summed over the
// Capacity Reservations each block holds its capacity as.
#[path = "all/capacity_block_status.rs"]
mod capacity_block_status;
#[path = "all/capacity_blocks.rs"]
mod capacity_blocks;
// The account's EC2 Capacity Manager settings: what `EnableCapacityManager` records and
// what `GetCapacityManagerAttributes` reports back, including the ingestion the enable
// starts.
#[path = "all/capacity_manager.rs"]
mod capacity_manager;
// What `GetCapacityManagerAttributes` reports about those settings: every member of the
// result, over the account histories behind it and the Regions it is asked for in.
#[path = "all/capacity_manager_attributes.rs"]
mod capacity_manager_attributes;
#[path = "all/capacity_reservation_billing_requests.rs"]
mod capacity_reservation_billing_requests;
#[path = "all/capacity_reservation_fleets.rs"]
mod capacity_reservation_fleets;
#[path = "all/carrier_gateways.rs"]
mod carrier_gateways;
// What `DescribeClassicLinkInstances` reports out of the ClassicLink link store, and how the
// instance ids, the documented filters and the page size narrow it. Declared in the MIDDLE
// of the list rather than at either end: the head and the tail are both where concurrent ec2
// landings insert their two lines, and these two lost repeated serialized squash merges at
// the tail.
#[path = "all/classic_link_instances.rs"]
mod classic_link_instances;
// What `DeleteVpnConnection` starts on the Site-to-Site VPN connection it names, what it
// refuses, and what the clock then owes it. Declared in the MIDDLE of the list for the
// reason the suite above gives: the head and the tail are both where concurrent ec2
// landings insert their two lines, and these two already lost a serialized squash merge at
// the tail.
#[path = "all/delete_vpn_connection.rs"]
mod delete_vpn_connection;
// What `CreateSnapshots` makes of the volumes one instance holds — one snapshot each, the
// two exclusions, the tags, and the refusals. Declared in the MIDDLE of the list for the
// reason the suite above gives: the head and the tail are both where concurrent ec2 landings
// insert their two lines, and these two lost three serialized squash merges at the tail.
#[path = "all/create_snapshots.rs"]
mod create_snapshots;
#[path = "all/coip_pools.rs"]
mod coip_pools;
#[path = "all/confirm_product_instance.rs"]
mod confirm_product_instance;
#[path = "all/copy_volumes.rs"]
mod copy_volumes;
// What `DescribeVpcEndpointAssociations` reports out of the VPC endpoint association store,
// and how the endpoint ids, the documented filters and the page size narrow it. Declared in
// the MIDDLE of the list for the reason the two suites above give: the head and the tail are
// both where concurrent ec2 landings insert their two lines, and these two lost two
// serialized squash merges at the head.
#[path = "all/vpc_endpoint_associations.rs"]
mod vpc_endpoint_associations;
// What `CreateDhcpOptions` stores: the options a request names, the ones it refuses, and
// the set a caller created being one `AssociateDhcpOptions` resolves.
#[path = "all/create_dhcp_options.rs"]
mod create_dhcp_options;
#[path = "all/create_security_group.rs"]
mod create_security_group;
#[path = "all/dedicated_hosts.rs"]
mod dedicated_hosts;
#[path = "all/default_network.rs"]
mod default_network;
#[path = "all/default_network_per_region.rs"]
mod default_network_per_region;
// What `DeleteCarrierGateway` removes from the carrier gateway store, what it reports back,
// what it refuses and what it releases. Declared beside the other `delete_*` suites rather
// than at the tail of this file: the tail is where every concurrent ec2 landing appends its
// two lines, and these two lost repeated serialized squash merges there.
#[path = "all/delete_carrier_gateway.rs"]
mod delete_carrier_gateway;
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
// Why `DescribeElasticGpus` serves no report at all: the Elastic Graphics accelerator it
// reports on reached end of life, so EC2 refuses the call for the absent functionality.
#[path = "all/elastic_gpus.rs"]
mod elastic_gpus;
#[path = "all/elastic_ip_addresses.rs"]
mod elastic_ip_addresses;
// What `DescribeExportTasks` reports out of the export task store, and how `ExportTaskIds`
// and the filters narrow it.
#[path = "all/export_tasks.rs"]
mod export_tasks;
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
// What `CreateRestoreImageTask` refuses — a bucket name that reaches no bucket, read as a
// request parameter, and the stored AMI object this emulator holds none of — and what it
// answers ahead of that read. Declared HERE, beside the suite it took those tests from,
// rather than at the tail, for the reason the header note gives: the tail is where every
// concurrent ec2 landing inserts its two lines, and these two have already lost two
// serialized squash merges there.
#[path = "all/create_restore_image_task.rs"]
mod create_restore_image_task;
#[path = "all/get_password_data.rs"]
mod get_password_data;
// The JPG-format console screenshot `GetConsoleScreenshot` reports for one instance, what it
// refuses, and what `DryRun` and `WakeUp` do to it. Declared beside the suite above rather
// than in sorted position, for the reason this file's header note gives: the sorted
// neighbourhood and the tail of this list are where every new ec2 suite inserts its lines.
#[path = "all/get_console_screenshot.rs"]
mod get_console_screenshot;
// Which ONE attribute of a stored AMI `DescribeImageAttribute` reports, and what it
// refuses. Declared HERE rather than in sorted position or at the tail, for the reason the
// header note gives: both the sorted `image_*` neighbourhood and the tail of this list are
// where concurrent ec2 landings insert their two lines, and these two have already lost
// several serialized squash merges there.
#[path = "all/image_attributes.rs"]
mod image_attributes;
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
// Cancelling an import task of either kind: what `CancelImportTask` moves and what it
// refuses.
#[path = "all/import_task_cancellation.rs"]
mod import_task_cancellation;
#[path = "all/instance_connect_endpoints.rs"]
mod instance_connect_endpoints;
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
// Deleting the association between an IPAM and a Regional Internet Registry.
#[path = "all/ipam_internet_registry_associations.rs"]
mod ipam_internet_registry_associations;
// Reporting the associations between an IPAM and a Regional Internet Registry.
#[path = "all/ipam_internet_registry_association_reports.rs"]
mod ipam_internet_registry_association_reports;
#[path = "all/ipam_policies.rs"]
mod ipam_policies;
#[path = "all/ipam_pools.rs"]
mod ipam_pools;
// Creating one further IPAM scope in an IPAM, and reading the account's scopes back.
#[path = "all/ipam_scopes.rs"]
mod ipam_scopes;
#[path = "all/ipam_routing_policy_registrations.rs"]
mod ipam_routing_policy_registrations;
#[path = "all/launch_templates.rs"]
mod launch_templates;
#[path = "all/local_gateway_route_table_virtual_interface_group_associations.rs"]
mod local_gateway_route_table_virtual_interface_group_associations;
#[path = "all/local_gateway_route_tables.rs"]
mod local_gateway_route_tables;
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
#[path = "all/network_interface_permissions.rs"]
mod network_interface_permissions;
#[path = "all/network_performance_data.rs"]
mod network_performance_data;
#[path = "all/network_performance_subscriptions.rs"]
mod network_performance_subscriptions;
#[path = "all/placement_groups.rs"]
mod placement_groups;
#[path = "all/principal_id_format.rs"]
mod principal_id_format;
// What `CreateRouteServerEndpoint` resolves before it writes, what it replays for a repeated
// idempotency token, and what a dry run of it performs. Declared here rather than in sorted
// position, for the reason the note at the head of this file gives: the sorted neighbourhood
// and the head of the list are where every new ec2 suite inserts its two lines.
#[path = "all/create_route_server_endpoint.rs"]
mod create_route_server_endpoint;
// The public IPv4 pool slice: create a pool, then report it back out of the store.
#[path = "all/public_ipv4_pools.rs"]
mod public_ipv4_pools;
// What `PurchaseCapacityBlock` buys out of a Capacity Block offering, and the pair it stores:
// the block, and the Capacity Reservation that holds that block's capacity.
#[path = "all/purchase_capacity_block.rs"]
mod purchase_capacity_block;
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
// Declared in the MIDDLE of this list rather than at either end, for the reason the header
// note gives: the media type every ec2Query response declares, across the operations, the
// statuses and the layers that render one.
#[path = "all/response_media_type.rs"]
mod response_media_type;
#[path = "all/resource_specific_error_codes.rs"]
mod resource_specific_error_codes;
#[path = "all/revoke_security_group_ingress.rs"]
mod revoke_security_group_ingress;
#[path = "all/route_server_endpoints.rs"]
mod route_server_endpoints;
#[path = "all/route_server_peers.rs"]
mod route_server_peers;
#[path = "all/route_servers.rs"]
mod route_servers;
#[path = "all/search_local_gateway_routes.rs"]
mod search_local_gateway_routes;
#[path = "all/secondary_interfaces.rs"]
mod secondary_interfaces;
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
// What `CancelSpotFleetRequests` does to one Spot Fleet request and the Spot Instance requests
// it placed, and what it reports per fleet.
#[path = "all/spot_fleet_request_cancellation.rs"]
mod spot_fleet_request_cancellation;
#[path = "all/spot_fleet_requests.rs"]
mod spot_fleet_requests;
#[path = "all/spot_instance_requests.rs"]
mod spot_instance_requests;
#[path = "all/spot_placement_scores.rs"]
mod spot_placement_scores;
#[path = "all/spot_price_history.rs"]
mod spot_price_history;
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
// The Verified Access GROUP slice: create a group on a stored instance, then reach it
// through the operations that resolve a group.
#[path = "all/verified_access_groups.rs"]
mod verified_access_groups;
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
#[path = "all/vpc_endpoint_connection_notifications.rs"]
mod vpc_endpoint_connection_notifications;
#[path = "all/vpc_endpoint_connections.rs"]
mod vpc_endpoint_connections;
// The readiness poll a client makes on a created VPC endpoint: the loop on the non-terminal
// state terminates, at the terminal one.
#[path = "all/vpc_endpoint_readiness_poll.rs"]
mod vpc_endpoint_readiness_poll;
#[path = "all/vpc_endpoint_state.rs"]
mod vpc_endpoint_state;
// The vocabulary of VPC endpoint states: every value the filter publishes, against the
// members of the `State` enum the model generates.
#[path = "all/vpc_endpoint_states.rs"]
mod vpc_endpoint_states;
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
// Declared HERE, between two long-settled suites, rather than in sorted position or at the
// end of this file: the header records that both the sorted neighbourhood and the tail are
// where every new ec2 suite inserts its lines, and these two lines lost repeated serialized
// squash merges to the tail. What `DescribeCapacityReservationTopology` reports for the
// Capacity Reservations of one Region: the path through the topology tree each one's capacity
// hangs off, and how the ids, the two documented filters and the page size narrow it.
#[path = "all/capacity_reservation_topology.rs"]
mod capacity_reservation_topology;
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
// Declared LAST for the reason the header gives: what a stored network interface blocks
// (the group, subnet and VPC deletes it is a dependency of), and the public Systems Manager
// parameter catalog a `resolve:ssm:` launch-template `ImageId` resolves through.
#[path = "all/network_interface_dependencies.rs"]
mod network_interface_dependencies;
#[path = "all/launch_template_ssm_alias_catalog.rs"]
mod launch_template_ssm_alias_catalog;
// Declared LAST for the reason the header gives: what `DescribeIpamByoasn` reports out of
// the store of ASNs brought to IPAM, and how the page size and the token narrow it.
#[path = "all/ipam_byoasns.rs"]
mod ipam_byoasns;
// Declared LAST for the reason the header gives: the route server ASSOCIATION slice —
// associating a route server with a VPC, and reading one route server's associations back.
#[path = "all/route_server_associations.rs"]
mod route_server_associations;
// Declared LAST for the reason the header gives: the sorted region of this file is where
// every new ec2 suite inserts its lines. What `DeleteTrafficMirrorSession` deletes out of
// the Traffic Mirror session store, and what it refuses.
#[path = "all/traffic_mirror_sessions.rs"]
mod traffic_mirror_sessions;
// Declared LAST for the reason the header gives: the sorted region of this file is where
// every new ec2 suite inserts its lines. What `DescribeClientVpnEndpoints` reports out of the
// Client VPN endpoint store, and how the ids, the documented filters and the page size narrow
// it.
#[path = "all/client_vpn_endpoints.rs"]
mod client_vpn_endpoints;
// Declared LAST for the reason the header gives: the VPC flow log slice — turning flow
// logging on for a resource, reading the flow logs back, and deleting them in batches.
#[path = "all/flow_logs.rs"]
mod flow_logs;
// Declared LAST for the reason the header gives: the sorted region of this file is where
// every new ec2 suite inserts its lines. What `DescribeTransitGatewayPolicyTables` reports
// out of the policy table store, and how the id list, the filters and the page size narrow
// it.
#[path = "all/transit_gateway_policy_tables.rs"]
mod transit_gateway_policy_tables;
// Declared LAST for the same reason: what `DescribeTransitGatewayPeeringAttachments` reports
// out of the attachment store, and how the id list, the documented filters and the page size
// narrow it.
#[path = "all/transit_gateway_peering_attachments.rs"]
mod transit_gateway_peering_attachments;
// Declared LAST for the reason the header gives: the sorted region of this file is where
// every new ec2 suite inserts its lines. What `CreateNetworkInsightsAccessScope` persists —
// the scope and the content it was given — and what `GetNetworkInsightsAccessScopeContent`
// reports back out of that store.
#[path = "all/network_insights_access_scope_content.rs"]
mod network_insights_access_scope_content;
// Declared LAST for the reason the header gives: the sorted region of this file is where
// every new ec2 suite inserts its lines. What `CreateFpgaImage` refuses — the design
// checkpoint it reads, which this emulator holds no Amazon S3 object for — and what it
// answers ahead of that read.
#[path = "all/create_fpga_image.rs"]
mod create_fpga_image;
// Declared LAST for the reason the header gives: the sorted region of this file is where
// every new ec2 suite inserts its lines. Which ONE member of `FpgaImageAttribute`
// `DescribeFpgaImageAttribute` reports out of the stored AFI, and what it refuses.
#[path = "all/describe_fpga_image_attribute.rs"]
mod describe_fpga_image_attribute;
// Declared LAST for the reason the header gives: the sorted region of this file is where
// every new ec2 suite inserts its lines. The Verified Access INSTANCE slice: what
// `CreateVerifiedAccessInstance` mints and persists, and the group create that resolves it.
#[path = "all/verified_access_instances.rs"]
mod verified_access_instances;
// Declared LAST for the reason the header gives: the sorted region of this file is where
// every new ec2 suite inserts its lines. What `ValidateSecurityGroupQuotasForInterface`
// answers every caller of this emulator, and what it leaves the store holding.
// Declared LAST for the reason the header gives: the sorted region of this file is where
// every new ec2 suite inserts its lines. The managed prefix list slice: what
// `CreateManagedPrefixList` stores and refuses, and what `DescribeManagedPrefixLists`
// reports back — the account's own lists and the ones a Region publishes.
#[path = "all/managed_prefix_lists.rs"]
mod managed_prefix_lists;
#[path = "all/validate_security_group_quotas_for_interface.rs"]
mod validate_security_group_quotas_for_interface;
// Declared LAST for the reason the header gives: the sorted region of this file is where
// every new ec2 suite inserts its lines. The account's *block public access for AMIs*
// setting: what the enable puts in effect, what the disable takes back out, and what
// `GetImageBlockPublicAccessState` reports out of each Region's own setting.
#[path = "all/image_block_public_access.rs"]
mod image_block_public_access;
// Appended at the END of this file, like the suites above it and for the reason the header
// gives: the sorted region of this file is where every new ec2 suite inserts its lines, so a
// line added there loses serialized squash merges. What `CreateTransitGatewayRouteTable`
// mints and persists for the gateway it names, and what it refuses.
#[path = "all/create_transit_gateway_route_table.rs"]
mod create_transit_gateway_route_table;
// Appended at the END for the same reason. What `DescribeTransitGatewayMeteringPolicies`
// reports out of the metering policy store, and how the id list, the filters and the page
// size narrow it.
#[path = "all/transit_gateway_metering_policies.rs"]
mod transit_gateway_metering_policies;
// Appended at the END for the same reason. What `MoveCapacityReservationInstances` shifts
// between two Capacity Reservations, what it conserves, and what it refuses.
#[path = "all/move_capacity_reservation_instances.rs"]
mod move_capacity_reservation_instances;
// Appended at the END of this file, for the reason the header gives. The security group VPC
// association slice: what `AssociateSecurityGroupVpc` writes and refuses, and what
// `DescribeSecurityGroupVpcAssociations` reports out of that store.
#[path = "all/security_group_vpc_associations.rs"]
mod security_group_vpc_associations;
// Appended at the END of this file, for the reason the header gives. What
// `DescribeCustomerGateways` reports out of the customer gateway store, and how the id list,
// the documented filters and the clock narrow it.
#[path = "all/customer_gateways.rs"]
mod customer_gateways;
// Appended at the END of this file, for the reason the header gives. The enclave certificate
// slice: which values `AssociateEnclaveCertificateIamRole` takes for the certificate and the
// role it associates, what it stores for the pair, and what
// `GetAssociatedEnclaveCertificateIamRoles` reports back out of that store.
#[path = "all/enclave_certificate_iam_roles.rs"]
mod enclave_certificate_iam_roles;
