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
// An account's internet gateways: what `CreateInternetGateway` stores and
// `DescribeInternetGateways` reports back out of that store — the id, the owning account, the
// empty attachment set, the tags, and how the id list, the documented filters and the page
// size narrow the report. Declared beside `prefix_lists` for the reason the note above gives.
#[path = "all/internet_gateways.rs"]
mod internet_gateways;
// What `DescribeNatGateways` reports out of the NAT gateway store: every member a stored
// gateway holds, how the id list, the documented filters and the page size narrow it, what it
// refuses, and the address status the clock owes a gateway it is read through. Declared beside
// `prefix_lists` for the reason the note above gives: the sorted neighbourhood and the tail of
// this file are where every new ec2 suite inserts its lines.
#[path = "all/describe_nat_gateways.rs"]
mod describe_nat_gateways;
// The FIXED TEXT of a refusal's message, per class: the sentence the service words that error
// class with, with only the variable parts it interpolates from the request and the store —
// no restatement of the rule the value missed, no identifier out of the account's own state,
// and the holder set of a `VolumeInUse` rendered as the set it is. Declared beside
// `prefix_lists` for the reason the note above gives.
#[path = "all/refusal_fixed_text.rs"]
mod refusal_fixed_text;
// What `GetManagedPrefixListEntries` reports: the entry set of the one prefix list a request
// names — a list the account created or one the Region publishes — in CIDR order, how it
// pages, which version it reports and what it refuses. Declared beside `prefix_lists` for the
// reason the note above gives: the sorted neighbourhood and the tail of this file are where
// every new ec2 suite inserts its lines.
#[path = "all/get_managed_prefix_list_entries.rs"]
mod get_managed_prefix_list_entries;
// What `DescribeIpamPools` reports out of the IPAM pool store: every member the create stored,
// how the id list, the documented filters and the page size narrow it, what it refuses, and the
// state move the clock owes a pool it is read through. Declared after the suite above rather
// than directly beside `prefix_lists`: that anchor is where every new ec2 suite inserts its
// lines, so these two sit one block further down with context between them.
#[path = "all/describe_ipam_pools.rs"]
mod describe_ipam_pools;
// The entries (rules) of a network ACL: what `CreateNetworkAclEntry` stores on a stored ACL
// and in which order, what `DeleteNetworkAclEntry` takes off it, and the not-found answer
// every read of a `DeleteNetworkAcl`-ed ACL gets. Declared beside `prefix_lists` for the
// reason the note above gives: the sorted neighbourhood and the tail of this file are where
// every new ec2 suite inserts its lines.
#[path = "all/network_acl_entries.rs"]
mod network_acl_entries;
// What `CancelCapacityReservation` releases: the reservation's own `cancelled` state in every
// reader of it, and the terminal status cancelling either side gives an interruptible
// allocation. Declared beside `prefix_lists` for the reason the note above gives.
#[path = "all/capacity_reservation_cancellation.rs"]
mod capacity_reservation_cancellation;
// `DescribeAddressesAttribute` as the projection of an Elastic IP address's `domain-name`
// attribute state it is: which addresses carry a row, how `AllocationIds` filters them, what
// it refuses, and how it pages. Declared beside `prefix_lists` for the reason the note above
// gives: the sorted neighbourhood and the tail of this file are where every new ec2 suite
// inserts its lines.
#[path = "all/describe_addresses_attribute.rs"]
mod describe_addresses_attribute;
// What `CancelConversionTask` cancels — the form it judges a conversion task id by, the
// states that admit a cancel, the `cancelling` -> `cancelled` move the clock owes an
// admitted one, and what `DryRun` answers. Declared beside `prefix_lists` for the reason the
// note above gives: the sorted neighbourhood and the tail of this file are where every new
// ec2 suite inserts its lines.
#[path = "all/conversion_task_cancellation.rs"]
mod conversion_task_cancellation;
// The two forms an instance id is read in before any lookup, and the two answers a value
// inside and outside them gets, across a describe, a password read and a mutation. Declared
// beside `prefix_lists` for the reason the note above gives: the sorted neighbourhood and
// the tail of this file are where every new ec2 suite inserts its lines.
#[path = "all/instance_id_forms.rs"]
mod instance_id_forms;
// What `CreateImageUsageReport` computes one AMI usage report from — the image, resource
// types, scope options, accounts and tags the request names — the `pending` -> `available`
// move the clock owes it, the 30-day deletion it carries, its `ClientToken` idempotency and
// what it refuses. Declared beside `prefix_lists` for the reason the note above gives: the
// sorted neighbourhood and the tail of this file are where every new ec2 suite inserts its
// lines.
#[path = "all/create_image_usage_report.rs"]
mod create_image_usage_report;
// What `EnableFastSnapshotRestores` stores for each (snapshot, Availability Zone) pair, the
// lifecycle the clock carries that pair through, what `DescribeFastSnapshotRestores` reports,
// narrows, pages and refuses, and what `DisableFastSnapshotRestores` leaves behind. Declared
// HERE rather than in sorted position or beside `prefix_lists`: both of those are the
// contended regions the note at the top of this file describes, and this fix lost three
// serialized squash merges to the lines just below `prefix_lists`.
#[path = "all/fast_snapshot_restore_lifecycle.rs"]
mod fast_snapshot_restore_lifecycle;
// What `CopySnapshot` mints from a stored snapshot, which members it reads off the source and
// which off the request, and what it refuses. Declared beside `prefix_lists` for the reason
// the note above gives: the sorted neighbourhood and the tail of this file are where every new
// ec2 suite inserts its lines.
#[path = "all/copy_snapshot.rs"]
mod copy_snapshot;
// What `ModifyVerifiedAccessInstanceLoggingConfiguration` sets on one Verified Access
// instance, what an instance nothing has configured carries, and what
// `DescribeVerifiedAccessInstanceLoggingConfigurations` reports, narrows, pages and refuses.
// Declared HERE rather than in sorted position or beside `prefix_lists`: both of those are the
// contended regions the note at the top of this file describes, and this fix lost two
// serialized squash merges to the lines just below `prefix_lists`.
#[path = "all/verified_access_instance_logging.rs"]
mod verified_access_instance_logging;
// How the interruptible-allocation pair screens a Capacity Reservation id: the shape it reads
// one by, the not-found answer every value of that shape gets and the malformed answer every
// value outside it gets, across both operations. Declared HERE rather than in sorted position
// or beside `prefix_lists`, for the reason the note at the top of this file gives: both of
// those are the contended regions every new ec2 suite inserts its lines into.
#[path = "all/interruptible_allocation_capacity_reservation_ids.rs"]
mod interruptible_allocation_capacity_reservation_ids;
// What `CreateTransitGatewayConnect` computes a Connect attachment from — the transport
// attachment it names, that attachment's own transit gateway, the requested tunnel protocol
// and tags — the `pending` -> `available` move the clock owes it, and what it refuses.
// Declared beside `prefix_lists` for the reason the note above gives: the sorted
// neighbourhood and the tail of this file are where every new ec2 suite inserts its lines.
#[path = "all/create_transit_gateway_connect.rs"]
mod create_transit_gateway_connect;
// What `DeleteVpnConcentrator` takes away, what it refuses and what it leaves alone.
// Declared beside `prefix_lists` for the reason the note above gives: the sorted
// neighbourhood and the tail of this file are where every new ec2 suite inserts its lines.
#[path = "all/delete_vpn_concentrator.rs"]
mod delete_vpn_concentrator;
// What `DisassociateSubnetCidrBlock` lets go of: the association it names, wherever in the
// account's subnets it is held, the transient state it answers, the clock-owed terminal
// state, and what it refuses. Declared beside `prefix_lists` for the reason the note above
// gives: the sorted neighbourhood and the tail of this file are where every new ec2 suite
// inserts its lines.
#[path = "all/disassociate_subnet_cidr_block.rs"]
mod disassociate_subnet_cidr_block;
// The sentence this service refuses a request with, per refusal CLASS: the resource named in
// that class's own words, the request's value interpolated in that class's own quoting and
// position, and nothing repeated back by a class whose sentence interpolates nothing.
// Declared beside `prefix_lists` for the reason the note above gives.
#[path = "all/per_resource_refusal_wording.rs"]
mod per_resource_refusal_wording;
// What `ModifyTransitGateway` applies to a stored transit gateway, what it leaves alone,
// what it refuses, and the `modifying` -> `available` move the clock owes the gateway.
// Declared beside `prefix_lists` for the reason the note above gives: the sorted
// neighbourhood and the tail of this file are where every new ec2 suite inserts its lines.
#[path = "all/modify_transit_gateway.rs"]
mod modify_transit_gateway;
// The primary network interface a launch gives its instance, the subnet and VPC it puts the
// instance in, and the filters that select on them. Declared beside `prefix_lists` for the
// reason the note above gives: the sorted neighbourhood and the tail of this file are where
// every new ec2 suite inserts its lines.
#[path = "all/instance_network_interfaces.rs"]
mod instance_network_interfaces;
// How `DescribeInstances` pages: the instances `MaxResults` caps and the token that walks to
// the full set, over stores whose instances are spread differently over reservations.
// Declared beside `prefix_lists` for the reason the note above gives.
#[path = "all/describe_instances_pages.rs"]
mod describe_instances_pages;
// The EBS-optimized performance, the NVMe-for-EBS answer and the hibernation support
// `DescribeInstanceTypes` reports, across families, generations and sizes. Declared beside
// `prefix_lists` for the reason the note above gives.
#[path = "all/instance_type_ebs_and_hibernation.rs"]
mod instance_type_ebs_and_hibernation;
// The deprecated images of the published AMI catalog: their age, when they were deprecated,
// and what a name-filtered `DescribeImages` does with them. Declared beside `prefix_lists`
// for the reason the note above gives.
#[path = "all/deprecated_catalog_images.rs"]
mod deprecated_catalog_images;
// The code and the request id a refusal of the instance read and write paths carries.
// Declared beside `prefix_lists` for the reason the note above gives.
#[path = "all/instance_read_path_refusals.rs"]
mod instance_read_path_refusals;
// What EC2 refuses about a REQUEST before it reads any state: a page size beside an id
// selection, a Capacity Block search that names both dimensions or neither, an idempotency
// token replayed with different parameters, and an AMI id a launch could not have been
// issued. Declared beside `prefix_lists` for the reason the note above gives: the sorted
// neighbourhood and the tail of this file are where every new ec2 suite inserts its lines.
#[path = "all/request_validation_before_state.rs"]
mod request_validation_before_state;
// What `DeleteCapacityManagerDataExport` takes out of the account's Capacity Manager data
// export configurations, what the reader then counts, and what the delete refuses. Declared
// beside `prefix_lists` for the reason the note above gives: the sorted neighbourhood and
// the tail of this file are where every new ec2 suite inserts its lines.
#[path = "all/capacity_manager_data_exports.rs"]
mod capacity_manager_data_exports;
// The form an Amazon FPGA Image id is read in before any lookup, and the two answers an id
// outside and inside it gets. Declared beside `prefix_lists` for the reason the note above
// gives: the sorted neighbourhood and the tail of this file are where every new ec2 suite
// inserts its lines.
#[path = "all/fpga_image_id_form.rs"]
mod fpga_image_id_form;
// The buckets the rules-per-security-group quota is counted in — one per group, direction
// and address family — and what a filled bucket refuses. Declared beside `prefix_lists` for
// the reason the note above gives: the sorted neighbourhood and the tail of this file are
// where every new ec2 suite inserts its lines.
#[path = "all/security_group_rule_quota_buckets.rs"]
mod security_group_rule_quota_buckets;
// What `AcceptReservedInstancesExchangeQuote` exchanges: which Convertible Reserved Instances
// it retires, the purchases of the target offerings it mints in their place, and what it
// refuses. Declared beside `prefix_lists` for the reason the note above gives: the sorted
// neighbourhood and the tail of this file are where every new ec2 suite inserts its lines.
#[path = "all/reserved_instances_exchanges.rs"]
mod reserved_instances_exchanges;
// What `ModifyCapacityReservationFleet` changes about a stored Capacity Reservation Fleet,
// what a describe then reports of it, and what the modification refuses. Declared beside
// `prefix_lists` for the reason the note above gives: the sorted neighbourhood and the tail
// of this file are where every new ec2 suite inserts its lines.
#[path = "all/modify_capacity_reservation_fleet.rs"]
mod modify_capacity_reservation_fleet;
// What `RevokeSecurityGroupEgress` removes from a group, which requests it admits and
// refuses. Declared beside `prefix_lists` for the reason the note above gives: the sorted
// neighbourhood and the tail of this file are where every new ec2 suite inserts its lines.
#[path = "all/revoke_security_group_egress.rs"]
mod revoke_security_group_egress;
// The one sentence every `Invalid<Kind>.Malformed` of this service words an unreadable
// resource id with, across the id kinds and the operations that resolve them. Declared
// beside `prefix_lists` for the reason the note above gives: the sorted neighbourhood and
// the tail of this file are where every new ec2 suite inserts its lines.
#[path = "all/malformed_id_message_template.rs"]
mod malformed_id_message_template;
// What `AttachVolume` records on a volume, which requests it admits and refuses, and the
// `attaching` -> `attached` move the clock owes the attachment. Declared beside
// `prefix_lists` for the reason the note above gives: the sorted neighbourhood and the tail
// of this file are where every new ec2 suite inserts its lines.
#[path = "all/attach_volume.rs"]
mod attach_volume;
// The ancestry chain `GetImageAncestry` walks over the parent link of the stored AMIs, and
// what it refuses. Declared beside `prefix_lists` for the reason the note above gives: the
// sorted neighbourhood and the tail of this file are where every new ec2 suite inserts its
// lines.
#[path = "all/image_ancestry.rs"]
mod image_ancestry;
// What `AssociateIpamResourceDiscovery` links, what the IPAM then counts, and what the
// associate refuses. Declared beside `prefix_lists` for the reason the note above gives: the
// sorted neighbourhood and the tail of this file are where every new ec2 suite inserts its
// lines.
#[path = "all/ipam_resource_discovery_associations.rs"]
mod ipam_resource_discovery_associations;
// What `SendDiagnosticInterrupt` delivers to one instance, which states it can be sent
// from, and what it refuses. Declared beside `prefix_lists` for the reason the note above
// gives: the sorted neighbourhood and the tail of this file are where every new ec2 suite
// inserts its lines.
#[path = "all/send_diagnostic_interrupt.rs"]
mod send_diagnostic_interrupt;
// Which suppression `DisableApplicationStatusCheckSuppression` lifts, what a reader then
// reports for that instance, and what the call refuses. Declared beside `prefix_lists` for
// the reason the note above gives: the sorted neighbourhood and the tail of this file are
// where every new ec2 suite inserts its lines.
#[path = "all/disable_application_status_check_suppression.rs"]
mod disable_application_status_check_suppression;
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
// What `CreateVpcEndpointServiceConfiguration` stores for an endpoint service the account offers
// — the load balancers behind it, the acceptance, address types, Regions, private DNS name and
// tags it was configured with, and the name, base DNS name and verification record derived from
// the `ServiceId` it mints — what it refuses, and what `DescribeVpcEndpointServiceConfigurations`
// and `DescribeVpcEndpointServices` then report out of that store. Declared beside the endpoint
// suite above rather than at the tail or in sorted position, for the reason the header note
// gives: both of those are where every new ec2 suite inserts its lines.
#[path = "all/create_vpc_endpoint_service_configuration.rs"]
mod create_vpc_endpoint_service_configuration;
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
// The Reserved Instance Marketplace listings of those purchases: what
// `CreateReservedInstancesListing` puts up for sale, what `CancelReservedInstancesListing`
// withdraws, what `DescribeReservedInstancesListings` reports back and narrows by, and what
// each refuses. Declared beside the modifications suite it shares a subject with, rather
// than in sorted position, for the reason this file's header note gives.
#[path = "all/reserved_instances_listings.rs"]
mod reserved_instances_listings;
// Where a paged describe bounds its page size and where it does not: a `MaxResults` the
// model leaves unbounded above is served, not refused. Declared beside the suites above
// rather than in sorted position, for the reason this file's header note gives.
#[path = "all/unranged_max_results.rs"]
mod unranged_max_results;
// What `RunInstances` does with a `TagSpecifications` entry, per resource type. Declared
// beside the suites above for the same reason.
#[path = "all/run_instances_tag_specifications.rs"]
mod run_instances_tag_specifications;
// What `RunInstances` does with an `IamInstanceProfile`, and the sentence the association
// describe answers for a filter name it does not document. Declared beside the launch suite
// above rather than in sorted position, for the reason this file's header note gives.
#[path = "all/run_instances_iam_instance_profile.rs"]
mod run_instances_iam_instance_profile;
// What a LAUNCH does with an `ImageId` that names a Systems Manager parameter instead of an
// AMI, and the EBS attachment lifecycle such a launch feeds: attach, the describe that reports
// it, and the detach. Declared beside the two launch suites above rather than at the end of
// this file: the end is where every new ec2 suite now appends, so a line added there loses
// serialized squash merges.
#[path = "all/launch_from_ssm_image_alias.rs"]
mod launch_from_ssm_image_alias;
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
// What `DescribeNetworkInsightsPaths` reports out of the path store, how the id list, the
// documented filters and the page size narrow it, and what it refuses. Declared beside the
// two path suites above — topically its neighbours — rather than in sorted position or at the
// tail, for the reason this file's header note gives: both are contended regions where a
// serialized squash merge is lost.
#[path = "all/describe_network_insights_paths.rs"]
mod describe_network_insights_paths;
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
// What `CreateApplicationStatusCheck` stores, the documented defaults it fills in, the
// published bounds it refuses past, and the check it answers read back through the describe.
#[path = "all/create_application_status_check.rs"]
mod create_application_status_check;
// What `AssociateApplicationStatusCheck` associates one check with — tag pairs on the check
// itself and the monitoring that begins for each named instance — read back through
// `DescribeApplicationStatusChecks` and `DescribeApplicationStatus`.
#[path = "all/associate_application_status_check.rs"]
mod associate_application_status_check;
// What `AssignIpv6Addresses` takes out of the IPv6 block of the subnet a network interface
// sits in, and records on that interface — named addresses and prefixes against a count of
// each, read back through `DescribeNetworkInterfaces`.
#[path = "all/assign_ipv6_addresses.rs"]
mod assign_ipv6_addresses;
#[path = "all/associate_address.rs"]
mod associate_address;
#[path = "all/associate_client_vpn_target_network.rs"]
mod associate_client_vpn_target_network;
#[path = "all/associate_subnet_cidr_block.rs"]
mod associate_subnet_cidr_block;
// What `AssociateTransitGatewayMulticastDomain` persists: one association per subnet the
// request names, through an `available` attachment, in the transient state the clock carries
// to `associated` — and the refusal each resource it cannot resolve gets.
#[path = "all/associate_transit_gateway_multicast_domain.rs"]
mod associate_transit_gateway_multicast_domain;
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
// The Capacity Block extension offered for one stored Capacity Block reservation: the window
// that continues it, the capacity and tenancy it already holds, and what the search refuses.
#[path = "all/capacity_block_extension_offerings.rs"]
mod capacity_block_extension_offerings;
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
// What `DisableCapacityManager` records: the account's status and Organizations access
// going off, the ingestion it stops, and what an enable after one starts.
#[path = "all/capacity_manager_disable.rs"]
mod capacity_manager_disable;
// What `GetCapacityManagerAttributes` reports about those settings: every member of the
// result, over the account histories behind it and the Regions it is asked for in.
#[path = "all/capacity_manager_attributes.rs"]
mod capacity_manager_attributes;
#[path = "all/capacity_reservation_billing_requests.rs"]
mod capacity_reservation_billing_requests;
// What `CreateCapacityReservationCancellationQuote` quotes for cancelling a future-dated
// Capacity Reservation inside its commitment, and what
// `DescribeCapacityReservationCancellationQuotes` reports back out of that store.
#[path = "all/capacity_reservation_cancellation_quotes.rs"]
mod capacity_reservation_cancellation_quotes;
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
// What `DescribeSnapshots` reports of the snapshots the account holds — the members, the
// narrowings, the paging, the refusals and the clock-owed completion. Declared in the MIDDLE
// of the list for the reason the suite above gives: the head and the tail are both where
// concurrent ec2 landings insert their two lines.
#[path = "all/describe_snapshots.rs"]
mod describe_snapshots;
// Which other VPCs `DescribeSecurityGroupReferences` reports as referencing one of the
// account's security groups, and what it refuses. Declared in the MIDDLE of the list for the
// reason the suites above give: the head and the tail are both where concurrent ec2 landings
// insert their two lines, and these two lost two serialized squash merges at the head.
#[path = "all/security_group_references.rs"]
mod security_group_references;
// What `DeleteCoipCidr` takes out of one customer-owned address pool's ranges, what it
// reports back, and the requests that resolve to no range at all. Declared HERE, beside the
// `coip_pools` fixtures its rows are written against, rather than in the `delete_*` block
// below: that block invites every concurrent `delete_*` suite to insert its two lines in the
// same few lines of context, and this one lost repeated serialized squash merges there.
#[path = "all/delete_coip_cidr.rs"]
mod delete_coip_cidr;
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
// The account's default credit option for CPU usage of a burstable instance family, per
// Region and per family: `ModifyDefaultCreditSpecification` and
// `GetDefaultCreditSpecification`.
#[path = "all/default_credit_specification.rs"]
mod default_credit_specification;
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
// What `DeleteNetworkAcl` takes out of the store: the one ACL its id names, the blockers the
// operation documents, and the store a refused or dry-run delete leaves untouched.
#[path = "all/delete_network_acl.rs"]
mod delete_network_acl;
#[path = "all/delete_network_interface.rs"]
mod delete_network_interface;
#[path = "all/delete_security_group.rs"]
mod delete_security_group;
#[path = "all/delete_vpc.rs"]
mod delete_vpc;
#[path = "all/describe_images.rs"]
mod describe_images;
// What `DescribeLaunchTemplates` reports out of the launch template store: the templates a
// create stored, the narrowing its ids, names, filters and pages do, and the refusal an id
// the account holds no template under gets. Declared HERE, in sorted position inside the
// long-settled block rather than at the head of the file, where every new ec2 suite inserts
// its lines and merges contend.
#[path = "all/describe_launch_templates.rs"]
mod describe_launch_templates;
// What `DescribeNetworkAcls` reports out of the network ACL store: the default ACL every VPC
// comes with, the subnets associated with it, and the narrowing its ids, filters and pages
// do. Declared HERE, in sorted position inside the long-settled block rather than beside
// `prefix_lists`: the head of this file is where every new ec2 suite inserts its lines, and
// this fix lost two serialized squash merges to exactly that contention.
#[path = "all/describe_network_acls.rs"]
mod describe_network_acls;
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
// What `DetachNetworkInterface` takes down and what it refuses: the `detaching` state and the
// completion the clock owes it, the malformed/not-found split on an `AttachmentId`, and the
// attachments that cannot be detached at all. Declared HERE, in sorted position inside the
// long-settled block rather than at the head of the file, where every new ec2 suite inserts
// its lines and merges contend.
#[path = "all/detach_network_interface.rs"]
mod detach_network_interface;
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
// The security groups a network interface in one VPC can be given: the groups created in it,
// the groups another VPC's association makes usable there, the filters that narrow the report
// and the refusals.
#[path = "all/get_security_groups_for_vpc.rs"]
mod get_security_groups_for_vpc;
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
// The fixed detail every instance reports, and the monitoring state a caller moves.
#[path = "all/instance_detail.rs"]
mod instance_detail;
#[path = "all/instance_event_notification_attributes.rs"]
mod instance_event_notification_attributes;
#[path = "all/instance_event_window_associations.rs"]
mod instance_event_window_associations;
#[path = "all/instance_event_windows.rs"]
mod instance_event_windows;
#[path = "all/instance_lifecycle.rs"]
mod instance_lifecycle;
#[path = "all/instance_metadata_defaults.rs"]
mod instance_metadata_defaults;
// The SQL Server High Availability state every enabled instance is reported in.
#[path = "all/instance_sql_ha_states.rs"]
mod instance_sql_ha_states;
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
// What an IPAM resource discovery samples as a Region's public addresses: which Elastic IP
// addresses of the account are in the sample, what the resource holding one contributes to it,
// how `Filters` narrow it and how `MaxResults` pages it.
#[path = "all/ipam_discovered_public_addresses.rs"]
mod ipam_discovered_public_addresses;
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
// Deleting an IPAM pool, what it refuses while the pool still holds resources, and what
// `Cascade` takes with it.
#[path = "all/ipam_pool_deletion.rs"]
mod ipam_pool_deletion;
// Creating one further IPAM scope in an IPAM, and reading the account's scopes back.
#[path = "all/ipam_scopes.rs"]
mod ipam_scopes;
// Creating an IPAM prefix list resolver and reading the account's resolvers back.
#[path = "all/ipam_prefix_list_resolvers.rs"]
mod ipam_prefix_list_resolvers;
// Associating a customer-managed prefix list with an IPAM prefix list resolver, and reading
// the account's resolver targets back.
#[path = "all/ipam_prefix_list_resolver_targets.rs"]
mod ipam_prefix_list_resolver_targets;
#[path = "all/ipam_routing_policy_registrations.rs"]
mod ipam_routing_policy_registrations;
// What `ModifyHosts` applies to the Dedicated Hosts it names, what it leaves as it was, which
// hosts it reports unsuccessful, and what the request itself is refused for. Declared here
// rather than in sorted position for the reason the header gives: the `modify_*` neighbourhood
// and the tail of this file are where other suites insert their lines, and this declaration
// lost several serialized squash merges to exactly that.
#[path = "all/modify_hosts.rs"]
mod modify_hosts;
#[path = "all/launch_templates.rs"]
mod launch_templates;
#[path = "all/local_gateway_route_table_virtual_interface_group_associations.rs"]
mod local_gateway_route_table_virtual_interface_group_associations;
#[path = "all/local_gateway_route_tables.rs"]
mod local_gateway_route_tables;
#[path = "all/local_gateway_virtual_interface_groups.rs"]
mod local_gateway_virtual_interface_groups;
#[path = "all/local_gateway_virtual_interfaces.rs"]
mod local_gateway_virtual_interfaces;
#[path = "all/mac_dedicated_hosts.rs"]
mod mac_dedicated_hosts;
#[path = "all/mac_modification_tasks.rs"]
mod mac_modification_tasks;
// The order in which an operation reads a caller-supplied identifier or format-constrained
// parameter — its form first, the store second — read across the cancel of an export task, the
// deletes of an AFI, an AMI usage report and a secondary network, and the restore task's bucket
// name. Declared here, in sorted position inside the long-settled block.
#[path = "all/malformed_before_lookup.rs"]
mod malformed_before_lookup;
#[path = "all/modify_route_server.rs"]
mod modify_route_server;
#[path = "all/modify_subnet_attribute.rs"]
mod modify_subnet_attribute;
#[path = "all/nat_gateway_addresses.rs"]
mod nat_gateway_addresses;
#[path = "all/network_insights_access_scope_analyses.rs"]
mod network_insights_access_scope_analyses;
#[path = "all/network_insights_access_scopes.rs"]
mod network_insights_access_scopes;
#[path = "all/network_interface_attributes.rs"]
mod network_interface_attributes;
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
// The Region-wide audit of the same longer-ID settings `principal_id_format` writes and
// reports per principal. Declared beside its sibling suite rather than in sorted position,
// for the reason the note at the head of this file gives: the sorted neighbourhood and the
// head of the list are where every new ec2 suite inserts its two lines.
#[path = "all/aggregate_id_format.rs"]
mod aggregate_id_format;
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
// What `DescribeScheduledInstanceAvailability` computes a schedule search from — the Scheduled
// Instance capacity the signed Region has set aside, the documented filters that narrow it, the
// recurrence and slot bounds that fix each reported slot and the term's hours, how it pages, and
// what it refuses. Declared in its SORTED position rather than beside `prefix_lists`: the head of
// this file is where every concurrently landing ec2 suite inserts, and this fix lost several
// serialized squash merges to exactly that.
#[path = "all/scheduled_instance_availability.rs"]
mod scheduled_instance_availability;
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
// The running instances `DescribeSpotFleetInstances` reports for one Spot Fleet request.
#[path = "all/spot_fleet_instances.rs"]
mod spot_fleet_instances;
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
// `DisableVpcClassicLink` and the reader it is proved through, `DescribeVpcClassicLink`: the
// ClassicLink enablement the store keeps per VPC, what a disable changes and what it refuses.
#[path = "all/vpc_classic_link.rs"]
mod vpc_classic_link;
// `DescribeVpcClassicLinkDnsSupport`: the ClassicLink DNS support the store keeps per VPC,
// what the describe reports for each VPC it resolves, how it pages, and what it refuses.
#[path = "all/vpc_classic_link_dns_support.rs"]
mod vpc_classic_link_dns_support;
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
// What `CreateVpcPeeringConnection` records between two VPCs and how
// `DescribeVpcPeeringConnections` reports it: the status the clock moves, the expiry of an
// unanswered request, the overlapping-range failure, and the ids, filters and pages.
#[path = "all/vpc_peering_connections.rs"]
mod vpc_peering_connections;
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
// Appended at the END of this file, for the reason the header gives. The CREATE side of the
// network ACL slice: what `CreateNetworkAcl` mints and persists for the VPC it names, what it
// refuses, and what a created ACL reads back as (the describe's own cover is
// `describe_network_acls`).
#[path = "all/network_acls.rs"]
mod network_acls;
// Appended at the END of this file, for the reason the header gives. Giving one ASN brought
// to IPAM back out of the IPAM that holds it: what `DeprovisionIpamByoasn` leaves the store
// holding, what the clock then decides the release ended as, and what it refuses.
#[path = "all/ipam_byoasn_deprovisioning.rs"]
mod ipam_byoasn_deprovisioning;
// Appended at the END of this file, for the reason the header gives. What
// `DescribeTransitGatewayVpcAttachments` reports out of the attachment store, how the id list,
// the documented filters and the page size narrow it, and that an acceptance's write and the
// move the clock owes it are both visible through it.
#[path = "all/transit_gateway_vpc_attachments.rs"]
mod transit_gateway_vpc_attachments;
// Appended at the END of this file, for the reason the header gives. What
// `CreateLocalGatewayRouteTable` mints and persists for the local gateway it names, the move
// the clock then owes the new table, and what it refuses.
#[path = "all/create_local_gateway_route_table.rs"]
mod create_local_gateway_route_table;
// Appended at the END of this file, for the reason the header gives. What `GetCoipPoolUsage`
// reports out of the addresses allocated from one customer-owned address pool, and how the
// documented filters and the page size narrow it.
#[path = "all/get_coip_pool_usage.rs"]
mod get_coip_pool_usage;
// Appended at the END of this file, for the reason the header gives. What
// `DescribeTransitGatewayConnects` reports out of the attachment store, and how the id list,
// the documented filters and the page size narrow it.
#[path = "all/describe_transit_gateway_connects.rs"]
mod describe_transit_gateway_connects;
// Appended at the END of this file, for the reason the header gives. What
// `DescribeTransitGatewayRouteTables` reports out of the transit gateway route table store,
// how the id list, the documented filters and the page size narrow it, and that the tables
// the implemented creates store — with the moves the clock owes them — read back through it.
#[path = "all/transit_gateway_route_tables.rs"]
mod transit_gateway_route_tables;
// Appended at the END of this file, for the reason the header gives. What
// `DescribeTransitGatewayAttachments` reports out of the attachment store — every kind of
// attachment, as the kind-independent shape — how the id list, the documented filters and the
// page size narrow it, and that the association a writer stored reads back through it.
#[path = "all/describe_transit_gateway_attachments.rs"]
mod describe_transit_gateway_attachments;
// Appended at the END of this file, for the reason the header gives. What
// `ModifyVerifiedAccessInstance` applies to a stored Verified Access instance, what it leaves
// as it was, what it refuses, and how one `ClientToken` bounds it to one modification.
#[path = "all/modify_verified_access_instance.rs"]
mod modify_verified_access_instance;
// Appended at the END of this file, for the reason the header gives. What
// `AssociateTrunkInterface` answers a caller whose account may not use interface trunking —
// one refusal for every shape of request, in any Region, with nothing stored.
#[path = "all/trunk_interface_associations.rs"]
mod trunk_interface_associations;
// Appended at the END of this file, for the reason the header gives. The account's *managed
// resource visibility* setting: the word `ModifyManagedResourceVisibility` puts in effect and
// the word `GetManagedResourceVisibility` reports out of the account's own setting.
#[path = "all/managed_resource_visibility.rs"]
mod managed_resource_visibility;
// Appended at the END of this file, for the reason the header gives. What
// `CreateTransitGatewayMeteringPolicy` mints and persists for the transit gateway it names,
// the move the clock then owes the new policy, and what it refuses.
#[path = "all/create_transit_gateway_metering_policy.rs"]
mod create_transit_gateway_metering_policy;
// Appended at the END of this file, for the reason the header gives. What
// `AssociateTransitGatewayRouteTable` stores for the (route table, attachment) pair it names,
// the move the clock then owes the new association, what it refuses, and how both reports of
// an association — the route table's own and the attachment's — answer with that one record.
#[path = "all/transit_gateway_route_table_associations.rs"]
mod transit_gateway_route_table_associations;
// Appended at the END of this file, for the reason the header gives. What
// `CreateTransitGatewayVpcAttachment` computes a VPC attachment from — the gateway, the VPC and
// the subnets the request names, the VPC's stored owner, and the options it asked for or
// inherited from the gateway — the `pending` -> `available` move the clock owes it, and what it
// refuses.
#[path = "all/create_transit_gateway_vpc_attachment.rs"]
mod create_transit_gateway_vpc_attachment;
// Appended at the END of this file, for the reason the header gives. The entries (rules) of a
// transit gateway metering policy: what `CreateTransitGatewayMeteringPolicyEntry` stores on a
// stored policy, how one rule number names one entry of one policy, and what
// `GetTransitGatewayMeteringPolicyEntries` reports, filters and pages out of that store.
#[path = "all/transit_gateway_metering_policy_entries.rs"]
mod transit_gateway_metering_policy_entries;
// Appended at the END of this file, for the reason the header gives: the sorted region of this
// file is where every new ec2 suite inserts its lines. What `DescribeReservedInstances` reports
// out of the Reserved Instances store — every member of a stored purchase, and how the id list,
// the offering class and type, and the documented filters narrow it — and that what a writer of
// a purchase stored is what the report answers with.
#[path = "all/describe_reserved_instances.rs"]
mod describe_reserved_instances;
// Appended at the END of this file, for the reason the header gives: the sorted region of this
// file is where every new ec2 suite inserts its lines. What `DeleteVerifiedAccessInstance`
// removes — the Verified Access instance the request names and the logging configuration kept
// beside it — what it answers, what still naming the instance refuses it, and how one
// `ClientToken` bounds it to one delete.
#[path = "all/delete_verified_access_instance.rs"]
mod delete_verified_access_instance;
// Appended at the END of this file, for the reason the header gives: the sorted region of this
// file is where every new ec2 suite inserts its lines. The accepting half of the Elastic IP
// address transfer handshake: what `AcceptAddressTransfer` takes out of a standing offer, what
// it refuses for an address nothing has on offer, and where the tags it carries land.
#[path = "all/accept_address_transfer.rs"]
mod accept_address_transfer;
// Appended at the END of this file, for the reason the header gives: the sorted region of this
// file is where every new ec2 suite inserts its lines. What `UnmonitorInstances` turns off: the
// `disabling` -> `disabled` move it starts for every distinct instance it names, read back
// through `DescribeInstances`, the instance it leaves where it was, and the states, ids, batch
// sizes and dry run it refuses without changing any instance's monitoring.
#[path = "all/unmonitor_instances.rs"]
mod unmonitor_instances;
